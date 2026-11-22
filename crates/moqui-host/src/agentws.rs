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

//! `GET /agentws`: the Agent Code socket (plans/agent-code/09-Wire-API.md section 2) and `GET
//! /.well-known/moqui/agent-code`, its environment descriptor (section 6). This file is the transport only; the frames,
//! scopes and limits are `moqui_code::ws`.
//!
//! The socket is checked as `/termws` is: by origin, then by the session cookie or, for the Desktop and Mobile apps'
//! host, the `login_key` header (a full-account key; a scoped key is refused until the key scopes of phase P6). The
//! user needs AgentCodeUse. `?protocol=N` names the client's protocol; a different one is refused with the reason.
//!
//! Each text frame and each wake of the change feed runs on a request worker in a fresh context entered as the user,
//! as a REST call does. The server sends `{"ping":n}` every 25 seconds and closes a socket it has heard nothing from
//! for 60 seconds. The sign-in is checked again each minute, so a logout closes the socket.
//!
//! The endpoint is `<endpoint path="/agentws" enabled="..."/>` of the webroot webapp: `true`, `false`, or `dev`
//! (the default: on only on a dev instance). The descriptor answers 404 while the socket is off.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{RawQuery, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use moqui_code::scopes::Grant;
use moqui_code::ws::{Env, Session, MAX_FRAME, PING_SECONDS, SILENT_SECONDS};
use moqui_core::context::{ErrorKind, ExecutionContextFactory};
use moqui_core::user::verify_session_cookie;

use crate::http::{plain, HttpState};
use crate::notws::{origin_allowed, session_cookie, session_user, RequestSite};

type CoreError = moqui_core::context::Error;

/// How often the sign-in is checked for a logout.
const SESSION_CHECK: Duration = Duration::from_secs(60);

/// Whether the socket is on: the endpoint's `enabled`, where `dev` (and no value) means on a dev instance only.
pub(crate) fn enabled(factory: &ExecutionContextFactory) -> bool {
    let value = factory.webapp_child_setting("webroot", "endpoint /agentws", "enabled");
    match value.trim() {
        "true" => true,
        "false" => false,
        _ => moqui_core::devtools::is_dev_instance(),
    }
}

/// What a socket signed in with, which it is checked again by.
#[derive(Clone)]
enum Credential {
    Session(String),
    KeyHash(String),
}

/// `GET /.well-known/moqui/agent-code`: the environment descriptor, with no user data.
pub(crate) async fn descriptor(State(state): State<Arc<HttpState>>) -> Response {
    let factory = Arc::downgrade(&state.factory);
    let found = crate::worker::on_pool(&factory, |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        let _authz = moqui_core::context::ExecutionContext::with_current(
            moqui_core::context::ExecutionContext::disable_authz,
        );
        moqui_code::environment::descriptor(&facade, factory.runtime_dir()).map_err(|err| CoreError::new(err.to_string()))
    })
    .await;
    match found {
        Ok(descriptor) => (
            [(axum::http::header::CONTENT_TYPE, "application/json"), (axum::http::header::CACHE_CONTROL, "no-store")],
            descriptor.to_string(),
        )
            .into_response(),
        Err(err) if err.kind() == ErrorKind::Unavailable => state.guard.unavailable("service unavailable"),
        Err(err) => crate::http::internal_error(err.message()),
    }
}

/// The `protocol` of the query, when it is a number.
fn protocol_of(query: Option<&str>) -> Result<Option<u32>, ()> {
    let Some(value) = query
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("protocol="))
    else {
        return Ok(None);
    };
    value.parse().map(Some).map_err(|_| ())
}

pub(crate) async fn upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    site: RequestSite,
    RawQuery(query): RawQuery,
    State(state): State<Arc<HttpState>>,
) -> Response {
    if !origin_allowed(&state.factory, &headers, &site, "agent code") {
        return plain(403, "origin not allowed");
    }
    let Ok(protocol) = protocol_of(query.as_deref()) else {
        return plain(400, "protocol is not a number");
    };
    if let Err(reason) = moqui_code::ws::check_protocol(protocol) {
        return plain(426, &reason);
    }
    let (credential, key) = match session_cookie(&headers, site.https()) {
        Some(cookie) => match verify_session_cookie(&cookie) {
            Ok(Some(id)) if !id.is_empty() => (Credential::Session(id), None),
            Ok(_) | Err(_) => return plain(401, "unauthorized"),
        },
        None => {
            let Some(key) = headers
                .get("login_key")
                .and_then(|value| value.to_str().ok())
                .filter(|key| !key.is_empty())
                .map(str::to_string)
            else {
                return plain(401, "unauthorized");
            };
            let Ok(hash) = moqui_core::user::hash_login_key(&key) else {
                return plain(401, "unauthorized");
            };
            (Credential::KeyHash(hash), Some(key))
        }
    };
    let factory = Arc::clone(&state.factory);
    let lookup = credential.clone();
    let loaded = tokio::task::spawn_blocking(move || -> Result<Option<String>, CoreError> {
        match (key, lookup) {
            (Some(key), _) => Ok(crate::notws::key_user(factory, key)?.0),
            (None, Credential::Session(id)) => session_user(factory, id),
            (None, Credential::KeyHash(_)) => Ok(None),
        }
    })
    .await;
    let user_id = match loaded {
        Ok(Ok(Some(user_id))) => user_id,
        Ok(Ok(None)) => return plain(401, "unauthorized"),
        Ok(Err(err)) if err.kind() == ErrorKind::Unavailable => return state.guard.unavailable("service unavailable"),
        Ok(Err(err)) => return crate::http::internal_error(err.message()),
        Err(_) => return plain(500, "request worker stopped"),
    };
    let grant = Grant {
        key_scopes: None,
        is_browser_session: matches!(credential, Credential::Session(_)),
    };
    let factory = Arc::clone(&state.factory);
    let shutdown = state.shutdown.clone();
    ws.max_message_size(MAX_FRAME)
        .on_upgrade(move |socket| run_socket(socket, factory, Session::new(user_id, grant), credential, shutdown))
}

/// What one call on the pool answers: the frames to send, or why to close.
enum Step {
    Frames(Vec<String>),
    Close(String),
}

/// Run `work` on a request worker as the socket's user, with the session's state.
async fn as_user(
    factory: &Arc<ExecutionContextFactory>,
    session: &Arc<Mutex<Session>>,
    work: impl FnOnce(&mut Session, &Env<'_>) -> Step + Send + 'static,
) -> Step {
    let session = Arc::clone(session);
    let ran = crate::worker::on_pool(&Arc::downgrade(factory), move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        let runtime = factory.runtime_dir().to_path_buf();
        let mut session = session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let user_id = session.user_id.clone();
        moqui_core::user::as_user(&user_id, || {
            let env = Env {
                facade: &facade,
                runtime: &runtime,
            };
            work(&mut session, &env)
        })
        .ok_or_else(|| CoreError::new("the user is not found"))
    })
    .await;
    match ran {
        Ok(step) => step,
        Err(err) => {
            tracing::warn!("Agent Code socket call failed: {err}");
            Step::Close("server error".to_string())
        }
    }
}

/// Whether the socket's user is still signed in. A busy pool or a failed read is not a logout.
async fn still_signed_in(factory: &Arc<ExecutionContextFactory>, credential: &Credential, user_id: &str) -> bool {
    let current = match credential.clone() {
        Credential::KeyHash(hash) => {
            crate::worker::on_pool(&Arc::downgrade(factory), move |_| moqui_core::user::login_key_user_by_hash(&hash))
                .await
        }
        Credential::Session(id) => {
            let factory = Arc::clone(factory);
            match tokio::task::spawn_blocking(move || session_user(factory, id)).await {
                Ok(result) => result,
                Err(_) => return true,
            }
        }
    };
    match current {
        Ok(current) => current.as_deref() == Some(user_id),
        Err(_) => true,
    }
}

async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    let mut end = reason.len().min(123);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    let _ = crate::notws::send_within(
        socket,
        Message::Close(Some(CloseFrame {
            code,
            reason: reason[..end].into(),
        })),
    )
    .await;
}

/// Send `frames`; false when the peer stopped taking them.
async fn send_all(socket: &mut WebSocket, frames: Vec<String>) -> bool {
    for frame in frames {
        if !crate::notws::send_within(socket, Message::Text(frame.into())).await {
            return false;
        }
    }
    true
}

async fn run_socket(
    mut socket: WebSocket,
    factory: Arc<ExecutionContextFactory>,
    session: Session,
    credential: Credential,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let user_id = session.user_id.clone();
    let session = Arc::new(Mutex::new(session));
    let wakes = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&wakes);
    // held for the socket's life; the feed keeps it weakly
    let waker: Arc<dyn Fn() + Send + Sync> = Arc::new(move || notify.notify_one());
    let hello = {
        let waker = Arc::clone(&waker);
        as_user(&factory, &session, move |session, env| {
            if !moqui_core::user::has_permission(moqui_code::USE_PERMISSION).unwrap_or(false) {
                return Step::Close(format!("this needs permission {}", moqui_code::USE_PERMISSION));
            }
            moqui_code::change::on_change(env.facade, &waker);
            match session.hello(env) {
                Ok(frame) => Step::Frames(vec![frame]),
                Err(err) => {
                    tracing::warn!("Agent Code hello failed: {err}");
                    Step::Close("server error".to_string())
                }
            }
        })
        .await
    };
    match hello {
        Step::Frames(frames) => {
            if !send_all(&mut socket, frames).await {
                return;
            }
        }
        Step::Close(reason) => {
            close(&mut socket, 1008, &reason).await;
            return;
        }
    }
    let mut ping = tokio::time::interval(Duration::from_secs(PING_SECONDS));
    ping.tick().await;
    let mut session_check = tokio::time::interval(SESSION_CHECK);
    session_check.tick().await;
    let mut heard = Instant::now();
    let mut pings: u64 = 0;
    loop {
        let step = tokio::select! {
            received = socket.recv() => {
                let Some(Ok(message)) = received else { return };
                heard = Instant::now();
                match message {
                    Message::Text(text) => {
                        let text = text.to_string();
                        as_user(&factory, &session, move |session, env| Step::Frames(session.on_text(env, &text))).await
                    }
                    Message::Close(_) => return,
                    Message::Binary(_) => Step::Close("binary frames are not used".to_string()),
                    Message::Ping(_) | Message::Pong(_) => continue,
                }
            }
            () = wakes.notified() => {
                as_user(&factory, &session, |session, env| Step::Frames(session.on_wake(env))).await
            }
            _ = ping.tick() => {
                if heard.elapsed() > Duration::from_secs(SILENT_SECONDS) {
                    Step::Close("no frames for 60 seconds".to_string())
                } else {
                    pings += 1;
                    Step::Frames(vec![serde_json::json!({"ping": pings}).to_string()])
                }
            }
            _ = session_check.tick() => {
                if still_signed_in(&factory, &credential, &user_id).await {
                    continue;
                }
                Step::Close("signed out".to_string())
            }
            // the guard of wait_for is not Send: drop it inside the arm's own future
            () = async { let _ = shutdown.wait_for(|stopped| *stopped).await; } => Step::Close("server stopping".to_string()),
        };
        match step {
            Step::Frames(frames) => {
                if !send_all(&mut socket, frames).await {
                    return;
                }
            }
            Step::Close(reason) => {
                let code = if reason == "server stopping" { 1001 } else { 1008 };
                close(&mut socket, code, &reason).await;
                return;
            }
        }
    }
}

#[cfg(test)]
#[path = "agentws_tests.rs"]
mod tests;
