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
//!
//! `?session=<id>` attaches to a kept session the Terminal screen opened (plans/Terminal-Fancy.md §5.3), one of the
//! cookie's user's own: the first message is `{"type":"attach","cols","rows","have"}`, with how many of the session's
//! bytes the page already drew, and the server answers `{"type":"attached",...}`, then the bytes, `state` changes and
//! `ended`. The page acknowledges what it drew (`{"type":"ack","count"}`). The socket closing leaves the session; a
//! logout closes the socket and leaves it too (D2). Input on the socket counts as use of the HTTP session.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, RawQuery, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::Response;
use moqui_core::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use moqui_core::user::verify_session_cookie;
use moqui_core::web::RequestVia;
use moqui_lib::object::Value;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::http::{plain, HttpState};
use crate::notws::{origin_allowed, session_cookie, session_user, RequestSite};
use crate::term::access::{self, Facts, Refusal};
use crate::term::registry::Registry;
use crate::term::session::{Kind, Out};
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

/// What the upgrade request says about where it came from, for a command or host session (plans/Terminal-Fancy.md
/// §8.2).
pub(crate) struct UpgradeFacts(Facts);

impl FromRequestParts<Arc<HttpState>> for UpgradeFacts {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<HttpState>) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        let via = parts.extensions.get::<RequestVia>().copied().unwrap_or_default();
        let https = parts.extensions.get::<crate::http::HttpsRequest>().is_some();
        let conf = state.guard.conf();
        let addressing = conf.addressing();
        let client_ip = peer
            .map(|peer| state.guard.client_of(peer, &parts.headers).to_string())
            .unwrap_or_default();
        let headers = &parts.headers;
        Ok(Self(Facts::of(
            peer,
            via,
            https,
            addressing.header(),
            client_ip,
            |name| headers.get(name).and_then(|value| value.to_str().ok()),
        )))
    }
}

pub(crate) async fn upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    site: RequestSite,
    UpgradeFacts(facts): UpgradeFacts,
    RawQuery(query): RawQuery,
    State(state): State<Arc<HttpState>>,
) -> Response {
    let session = query.as_deref().and_then(session_of);
    if !origin_allowed(&state.factory, &headers, &site, "terminal") {
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
                session,
                facts,
            },
            shutdown,
        )
    })
}

/// The `session` of the query: a kept session's id, URL-safe characters only.
fn session_of(query: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix("session="))
        .filter(|id| !id.is_empty() && id.len() <= 64)
        .filter(|id| {
            id.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
        .map(str::to_string)
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
    /// The kept session to attach to; none runs one that ends with the socket.
    session: Option<String>,
    /// Where the socket came from, for a command or host session.
    facts: Facts,
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
    let _ = crate::notws::send_within(
        socket,
        Message::Close(Some(CloseFrame {
            code: 1000,
            reason: reason[..end].into(),
        })),
    )
    .await;
}

/// May the HTTP session `session_id`'s user attach to a session of `kind` from where `facts` say: its permission,
/// and from anywhere but this machine a recent sign-in, read on the request pool with the HTTP session bound.
async fn may_attach(
    factory: &Arc<ExecutionContextFactory>,
    session_id: &str,
    kind: Kind,
    facts: Facts,
) -> Result<(), Refusal> {
    let session_id = session_id.to_string();
    let https = facts.https;
    let checked = crate::worker::on_pool(&Arc::downgrade(factory), move |factory| {
        let strong = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
            .ok_or_else(|| CoreError::new("no execution context is entered"))?;
        let _bound = moqui_core::user::session::bind_id(&strong, Some(&session_id), https)?;
        let permitted = |permission: &str| moqui_core::user::has_permission(permission).unwrap_or(false);
        Ok(access::check(
            access::Switches::of(factory),
            kind,
            &facts,
            &permitted,
            &moqui_core::user::recent_auth,
        ))
    })
    .await;
    checked.unwrap_or(Err(Refusal::NotAvailable))
}

/// An attach message: `{"type":"attach","cols","rows","have"}`, or a first `resize`, taken as an attach of a page that
/// drew nothing.
fn parse_attach(text: &str) -> Option<(u16, u16, Option<u64>)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let kind = value.get("type")?.as_str()?;
    if kind != "attach" && kind != "resize" {
        return None;
    }
    let dimension = |name: &str, least: u16| {
        let number = u16::try_from(value.get(name)?.as_u64()?).ok()?;
        Some(number.clamp(least, 500))
    };
    let have = value.get("have").and_then(serde_json::Value::as_u64);
    Some((dimension("cols", 20)?, dimension("rows", 10)?, have))
}

/// An acknowledgement: `{"type":"ack","count":N}`.
fn parse_ack(text: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    (value.get("type")?.as_str()? == "ack").then(|| value.get("count")?.as_u64())?
}

/// The page's first message within [`FIRST_SIZE_WAIT`], read by `parse`; `Err` when the server stopped first.
async fn first_message<T>(
    socket: &mut WebSocket,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Option<T>, ()> {
    tokio::select! {
        first = tokio::time::timeout(FIRST_SIZE_WAIT, async {
            loop {
                match socket.recv().await {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(first) = parse(&text) {
                            return Some(first);
                        }
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return None,
                    Some(Ok(_)) => {}
                }
            }
        }) => Ok(first.ok().flatten()),
        _ = shutdown.wait_for(|stopped| *stopped) => Err(()),
    }
}

async fn run_socket(socket: WebSocket, opened: Opened, shutdown: tokio::sync::watch::Receiver<bool>) {
    if opened.session.is_some() {
        run_attached(socket, opened, shutdown).await;
    } else {
        run_ephemeral(socket, opened, shutdown).await;
    }
}

/// A socket on a kept session: attached until the page leaves, the session ends, another page takes it, the user logs
/// out or the server stops.
async fn run_attached(mut socket: WebSocket, opened: Opened, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let Opened {
        factory,
        session_id,
        user_id,
        username,
        session,
        facts,
    } = opened;
    let id = session.unwrap_or_default();
    let label = format!("browser terminal {id} of {username}");
    // a session the holder kept from the server before is found once it is taken back
    #[cfg(all(feature = "devtools", unix))]
    if Registry::of(&factory).find(&id, &username).is_none() {
        let holder = crate::term::held::Holder::of(&factory);
        holder.adopt(Arc::downgrade(&factory)).await;
    }
    // Another user's id answers as one that does not exist.
    let Some(kept) = Registry::of(&factory).find(&id, &username) else {
        close(&mut socket, "no such terminal session").await;
        return;
    };
    // an attach to a running command line or shell is worth as much as a new one: asked again (§8.2)
    if kept.kind != Kind::Apps {
        if let Err(refusal) = may_attach(&factory, &session_id, kept.kind, facts).await {
            tracing::info!("{label}: attach refused: {refusal:?}");
            close(&mut socket, refusal.code()).await;
            return;
        }
    }
    let first = match first_message(&mut socket, &mut shutdown, parse_attach).await {
        Ok(Some(first)) => first,
        Ok(None) => {
            tracing::debug!("{label}: the page sent no attach");
            return;
        }
        Err(()) => {
            close(&mut socket, "the server is stopping").await;
            return;
        }
    };
    let (cols, rows, have) = first;
    let Some(attached) = kept.attach(have, cols, rows).await else {
        close(&mut socket, "the session ended").await;
        return;
    };
    let client = attached.client;
    let mut out = attached.out;
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut check = tokio::time::interval(SESSION_CHECK);
    check.tick().await;
    let mut typed = false;
    let mut reason = String::new();
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    reason = "the server is stopping".to_string();
                    break;
                }
            }
            next = out.recv() => match next {
                Some(Out::Bytes(bytes)) => {
                    if !crate::notws::send_within(&mut socket, Message::Binary(bytes.into())).await {
                        break;
                    }
                }
                Some(Out::Text(text)) => {
                    if !crate::notws::send_within(&mut socket, Message::Text(text.into())).await {
                        break;
                    }
                }
                Some(Out::Close(why)) => {
                    reason = why;
                    break;
                }
                None => {
                    reason = "the window fell behind".to_string();
                    break;
                }
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Binary(bytes))) => {
                    typed = true;
                    if !kept.write(bytes.to_vec()).await {
                        // the program no longer reads: its end comes through `out`
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if let Some(count) = parse_ack(&text) {
                        kept.ack(client, count).await;
                    } else if let Some((cols, rows)) = parse_resize(&text) {
                        kept.resize(client, cols, rows).await;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    let _ = crate::notws::send_within(&mut socket, Message::Pong(payload)).await;
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = ping.tick() => {
                if !crate::notws::send_within(&mut socket, Message::Ping(Vec::new().into())).await {
                    break;
                }
            }
            _ = check.tick() => {
                let lookup = (Arc::clone(&factory), session_id.clone());
                let current = tokio::task::spawn_blocking(move || session_user(lookup.0, lookup.1)).await;
                // A pool that is busy or a read that fails is not a logout; only a session with another or no user is.
                if let Ok(Ok(current)) = current {
                    if current.as_deref() != Some(user_id.as_str()) {
                        tracing::info!("{label} closed: the HTTP session logged out; the session is kept");
                        reason = "logged out".to_string();
                        break;
                    }
                }
                // Typing is use: a developer in a terminal all afternoon is not logged out for it (§5.2).
                if std::mem::take(&mut typed) {
                    moqui_core::user::session::note_use(&factory, &session_id);
                }
            }
        }
    }
    kept.detach(client).await;
    if reason.is_empty() {
        return;
    }
    close(&mut socket, &reason).await;
}

/// A socket with its own session, today's terminal: the session ends with the socket.
async fn run_ephemeral(mut socket: WebSocket, opened: Opened, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let Opened {
        factory,
        session_id,
        user_id,
        username,
        ..
    } = opened;
    let label = format!("browser terminal of {username}");
    // The session starts at the page's first size.
    // A stop does not wait for the page's size either: the HTTP listener's drain waits for this socket (HL-027).
    let first = match first_message(&mut socket, &mut shutdown, parse_resize).await {
        Ok(Some(first)) => first,
        Ok(None) => {
            tracing::debug!("{label}: the page sent no size");
            return;
        }
        Err(()) => {
            close(&mut socket, "the server is stopping").await;
            return;
        }
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
        admission: crate::term::Admission::Take,
        redraw: None,
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
                    if open && !crate::notws::send_within(&mut socket, Message::Binary(buffer[..read].to_vec().into())).await {
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
                    let _ = crate::notws::send_within(&mut socket, Message::Pong(payload)).await;
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    open = false;
                    let _ = to_session.shutdown().await;
                }
                Some(Ok(_)) => {}
            },
            _ = ping.tick(), if open => {
                if !crate::notws::send_within(&mut socket, Message::Ping(Vec::new().into())).await {
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
    use super::{parse_ack, parse_attach, parse_resize, session_of};

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

    #[test]
    fn an_attach_names_its_size_and_what_it_drew() {
        assert_eq!(
            parse_attach(r#"{"type":"attach","cols":100,"rows":30,"have":4096}"#),
            Some((100, 30, Some(4096)))
        );
        assert_eq!(
            parse_attach(r#"{"type":"attach","cols":100,"rows":30,"have":null}"#),
            Some((100, 30, None))
        );
        assert_eq!(
            parse_attach(r#"{"type":"resize","cols":80,"rows":24}"#),
            Some((80, 24, None))
        );
        assert_eq!(parse_attach(r#"{"type":"ack","count":1}"#), None);
        assert_eq!(parse_ack(r#"{"type":"ack","count":100000}"#), Some(100_000));
        assert_eq!(parse_ack(r#"{"type":"attach","count":1}"#), None);
    }

    #[test]
    fn a_session_id_is_taken_from_the_query_only_when_it_is_plain() {
        assert_eq!(session_of("session=abc-DEF_123").as_deref(), Some("abc-DEF_123"));
        assert_eq!(session_of("x=1&session=abc").as_deref(), Some("abc"));
        assert_eq!(session_of("session=a%20b"), None);
        assert_eq!(session_of("session="), None);
        assert_eq!(session_of("other=1"), None);
    }
}
