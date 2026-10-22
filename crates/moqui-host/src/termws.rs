/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! `GET /termws`: the terminal client in the browser (plans/archive/Screen-TUI-Impl.md 4.4), for the Terminal screen's
//! xterm.js (`/qapps/Terminal`). The socket is checked as `/notws` checks it, by origin and session cookie, and the
//! session runs as that user ([`crate::term`]), as `apps` does over SSH, with no permission of its own: every screen is
//! still checked as the user.
//!
//! Binary messages are the terminal's bytes, both ways. A text message from the page is JSON control, of which there is
//! one, `{"type":"resize","cols":N,"rows":N}`; the session starts on the first. The server ends the socket with a close
//! frame whose reason the page shows: a refusal (the session limits), a timeout, the user logged out, the server
//! stopping. The HTTP session is checked again each minute, so a logout closes the terminal too.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use moqui_core::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use moqui_core::user::verify_session_cookie;
use moqui_lib::object::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::http::{plain, HttpState};
use crate::notws::{origin_allowed, session_cookie, session_user};
use crate::term::{Sessions, Start, Terminal, Who};

type CoreError = moqui_core::context::Error;

/// Set to `/termws` while this server serves the socket; the shell (`qapps.xml`) offers the Terminal UI when it is.
pub(crate) const ENDPOINT_PROPERTY: &str = "webapp_terminal_endpoint";

/// A ping goes out this often, so a dead peer is noticed and proxies keep the connection.
const PING_EVERY: Duration = Duration::from_secs(30);
/// How often the HTTP session is checked for a logout.
const SESSION_CHECK: Duration = Duration::from_secs(60);
/// How long the page has to send its first size.
const FIRST_SIZE_WAIT: Duration = Duration::from_secs(30);
/// The pipe between the socket and the session, each way. A paste larger than this waits for the session to read it.
const PIPE: usize = 1 << 20;

pub(crate) async fn upgrade(ws: WebSocketUpgrade, headers: HeaderMap, State(state): State<Arc<HttpState>>) -> Response {
    if !origin_allowed(&state.factory, &headers, "terminal") {
        return plain(403, "origin not allowed");
    }
    let Some(cookie) = session_cookie(&headers) else {
        return plain(401, "unauthorized");
    };
    let session_id = match verify_session_cookie(&cookie) {
        Ok(Some(id)) if !id.is_empty() => id,
        Ok(_) | Err(_) => return plain(401, "unauthorized"),
    };
    let factory = Arc::clone(&state.factory);
    let lookup_id = session_id.clone();
    let loaded = tokio::task::spawn_blocking(move || -> Result<Option<(String, String)>, CoreError> {
        let Some(user_id) = session_user(Arc::clone(&factory), lookup_id)? else {
            return Ok(None);
        };
        Ok(username_of(&factory, &user_id)?.map(|username| (user_id, username)))
    })
    .await;
    let (user_id, username) = match loaded {
        Ok(Ok(Some(found))) => found,
        Ok(Ok(None)) => return plain(401, "unauthorized"),
        Ok(Err(err)) if err.kind() == ErrorKind::Unavailable => return state.guard.unavailable("service unavailable"),
        Ok(Err(err)) => return crate::http::internal_error(err.message()),
        Err(_) => return plain(500, "request worker stopped"),
    };
    let shutdown = state.shutdown.clone();
    let factory = Arc::clone(&state.factory);
    ws.on_upgrade(move |socket| {
        run_socket(
            socket,
            Opened {
                factory,
                session_id,
                user_id,
                username,
            },
            shutdown,
        )
    })
}

/// The username of `user_id`, read on the request pool with authz off.
fn username_of(factory: &Arc<ExecutionContextFactory>, user_id: &str) -> Result<Option<String>, CoreError> {
    let owner = Arc::clone(factory);
    let looked = Arc::clone(factory);
    let user_id = user_id.to_string();
    let rx = factory.request_pool().try_submit(owner, move || {
        let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz)
            .ok_or_else(|| CoreError::new("no execution context is entered"))?;
        let facade = looked
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        let row = facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(&user_id))
            .one()?;
        Ok(row.and_then(|row| {
            row.get("username")
                .map(moqui_lib::object::to_plain_string)
                .filter(|username| !username.is_empty())
        }))
    })?;
    match rx.recv() {
        Ok(result) => result,
        Err(_) => Err(CoreError::new("request worker stopped")),
    }
}

/// Who opened a socket.
struct Opened {
    factory: Arc<ExecutionContextFactory>,
    session_id: String,
    user_id: String,
    username: String,
}

/// A resize message: `{"type":"resize","cols":N,"rows":N}`, at least 20 by 10 and at most 500 by 500.
fn parse_resize(text: &str) -> Option<(u16, u16)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("type")?.as_str()? != "resize" {
        return None;
    }
    let dimension = |name: &str, least: u16| {
        let number = u16::try_from(value.get(name)?.as_u64()?).ok()?;
        Some(number.clamp(least, 500))
    };
    Some((dimension("cols", 20)?, dimension("rows", 10)?))
}

async fn close(socket: &mut WebSocket, reason: &str) {
    // A close reason is at most 123 bytes.
    let mut end = reason.len().min(123);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 1000,
            reason: reason[..end].into(),
        })))
        .await;
}

async fn run_socket(mut socket: WebSocket, opened: Opened, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let Opened {
        factory,
        session_id,
        user_id,
        username,
    } = opened;
    let label = format!("browser terminal of {username}");
    // The session starts at the page's first size.
    let first = tokio::time::timeout(FIRST_SIZE_WAIT, async {
        loop {
            match socket.recv().await {
                Some(Ok(Message::Text(text))) => {
                    if let Some(size) = parse_resize(&text) {
                        return Some(size);
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    let Ok(Some(first)) = first else {
        tracing::debug!("{label}: the page sent no size");
        return;
    };
    let (size, sized) = tokio::sync::watch::channel(first);
    let (ours, mut theirs) = tokio::io::duplex(PIPE);
    let (mut from_session, mut to_session) = tokio::io::split(ours);
    let mut start = Start {
        factory: Arc::downgrade(&factory),
        sessions: Sessions::of(&factory),
        who: Who::User(username.clone()),
        terminal: Terminal {
            term: "xterm-256color".to_string(),
            env: BTreeMap::from([
                ("COLORTERM".to_string(), "truecolor".to_string()),
                ("LANG".to_string(), "C.UTF-8".to_string()),
            ]),
        },
        size: sized,
        label: label.clone(),
        // The user logged in over HTTP, so this session has no login dialog to check an address for.
        client_ip: String::new(),
    };
    let session = tokio::spawn(async move { crate::term::serve(&mut theirs, &mut start).await });
    let mut buffer = vec![0u8; 64 * 1024];
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut check = tokio::time::interval(SESSION_CHECK);
    check.tick().await;
    let mut reason: Option<String> = None;
    // Until the page closes, input goes to the session; after, the session's output is still read to its end.
    let mut open = true;
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed(), if open => {
                if changed.is_err() || *shutdown.borrow() {
                    reason = Some("the server is stopping".to_string());
                    open = false;
                    let _ = to_session.shutdown().await;
                }
            }
            read = from_session.read(&mut buffer) => match read {
                // The session ended and closed its end of the pipe.
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if open && socket.send(Message::Binary(buffer[..read].to_vec().into())).await.is_err() {
                        open = false;
                        let _ = to_session.shutdown().await;
                    }
                }
            },
            incoming = socket.recv(), if open => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    if to_session.write_all(&bytes).await.is_err() {
                        open = false;
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if let Some(columns_rows) = parse_resize(&text) {
                        size.send_replace(columns_rows);
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    let _ = socket.send(Message::Pong(payload)).await;
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    open = false;
                    let _ = to_session.shutdown().await;
                }
                Some(Ok(_)) => {}
            },
            _ = ping.tick(), if open => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    open = false;
                    let _ = to_session.shutdown().await;
                }
            }
            _ = check.tick(), if open => {
                let lookup = (Arc::clone(&factory), session_id.clone());
                let current = tokio::task::spawn_blocking(move || session_user(lookup.0, lookup.1)).await;
                // A pool that is busy or a read that fails is not a logout; only a session with another or no user is.
                if let Ok(Ok(current)) = current {
                    if current.as_deref() != Some(user_id.as_str()) {
                        tracing::info!("{label} closed: the HTTP session logged out");
                        reason = Some("logged out".to_string());
                        open = false;
                        let _ = to_session.shutdown().await;
                    }
                }
            }
        }
    }
    let ended = session.await.ok();
    let note = reason.or_else(|| ended.and_then(|ended| ended.note));
    close(&mut socket, note.as_deref().unwrap_or("the session ended")).await;
}

#[cfg(test)]
mod tests {
    use super::parse_resize;

    #[test]
    fn a_resize_is_read_and_bounded() {
        assert_eq!(
            parse_resize(r#"{"type":"resize","cols":132,"rows":43}"#),
            Some((132, 43))
        );
        assert_eq!(
            parse_resize(r#"{"type":"resize","cols":2,"rows":9000}"#),
            Some((20, 500))
        );
        assert_eq!(parse_resize(r#"{"type":"other","cols":80,"rows":24}"#), None);
        assert_eq!(parse_resize(r#"{"type":"resize","cols":-1,"rows":24}"#), None);
        assert_eq!(parse_resize("not json"), None);
    }
}
