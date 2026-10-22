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

//! `GET /notws`. The socket is checked with the session cookie, then pushed notification JSON.
//! No execution context is kept for the life of the socket. Each socket of a session (a page in
//! each tab) is registered and receives the notifications of its topics. When the last socket of a user closes and none
//! comes back within [`OFFLINE_GRACE`], the user is marked offline (`ChatServices.disconnect#UserStatus`) rather than
//! when the chat heartbeat runs out; per node, so in a cluster a page on another node keeps the user online with its
//! next heartbeat.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use moqui_core::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use moqui_core::notify::{TopicMessage, NOTIFICATION_MESSAGE};
use moqui_core::user::{verify_session_cookie, SESSION_COOKIE_NAME};
use moqui_core::web::cookie_value;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use crate::http::{plain, HttpState};

pub(crate) mod collab;

type CoreError = moqui_core::context::Error;

static PUSH: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();
static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);

fn sockets() -> &'static Mutex<HashMap<u64, Arc<Slot>>> {
    static SOCKETS: OnceLock<Mutex<HashMap<u64, Arc<Slot>>>> = OnceLock::new();
    SOCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Notifications waiting for one socket. A client that falls this far behind is closed rather than letting its queue
/// grow without end; the client reopens the socket and the notifications are in its list.
const SOCKET_QUEUE: usize = 256;
/// A ping goes out this often, so a dead peer is noticed and proxies keep the connection.
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// Set to `/notws` while this server serves the socket; the html and qvt clients (`Header.html.fancy`, `qapps.xml`)
/// connect when it is.
pub(crate) const ENDPOINT_PROPERTY: &str = "webapp_notification_endpoint";
/// Java's `/notws` endpoint timeout: an hour with nothing from the client closes the socket.
const DEFAULT_IDLE: std::time::Duration = std::time::Duration::from_secs(3600);
/// How long after a user's last socket closed the user is marked offline, unless a socket of theirs came back: a page
/// reload opens its socket again well within it, so presence does not flicker.
const OFFLINE_GRACE: std::time::Duration = std::time::Duration::from_secs(20);
/// Marks a user offline at once and publishes it on the userStatus topic.
const DISCONNECT_SERVICE: &str = "org.moqui.impl.ChatServices.disconnect#UserStatus";

struct Slot {
    id: u64,
    user_id: String,
    session_id: String,
    topics: Mutex<HashSet<String>>,
    tx: tokio::sync::mpsc::Sender<Push>,
    /// Set when the queue overflowed, so the socket loop closes the socket.
    overflow: tokio::sync::Notify,
}

struct Push {
    text: String,
    message_id: String,
}

enum TopicCommand {
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
}

/// Subscribe once to `notification-message`. A second boot keeps the first listener.
pub(crate) fn install_push() -> Result<(), CoreError> {
    if PUSH.get().is_some() {
        return Ok(());
    }
    let subscription = moqui_core::notify::subscribe(NOTIFICATION_MESSAGE, Arc::new(deliver_notice))?;
    let _ = PUSH.set(subscription);
    Ok(())
}

pub(crate) async fn upgrade(ws: WebSocketUpgrade, headers: HeaderMap, State(state): State<Arc<HttpState>>) -> Response {
    if !origin_allowed(&state.factory, &headers, "notification") {
        return plain(403, "origin not allowed");
    }
    // A browser has its session cookie; the terminal client (`moqui tui --server`) has its login key, and no session.
    let factory = Arc::clone(&state.factory);
    let (session_id, loaded) = match session_cookie(&headers) {
        Some(cookie) => {
            let session_id = match verify_session_cookie(&cookie) {
                Ok(Some(id)) if !id.is_empty() => id,
                Ok(_) | Err(_) => return plain(401, "unauthorized"),
            };
            let lookup_id = session_id.clone();
            (
                session_id,
                tokio::task::spawn_blocking(move || session_user(factory, lookup_id)).await,
            )
        }
        None => {
            let Some(key) = headers
                .get("login_key")
                .and_then(|value| value.to_str().ok())
                .filter(|key| !key.is_empty())
                .map(str::to_string)
            else {
                return plain(401, "unauthorized");
            };
            (
                "login-key".to_string(),
                tokio::task::spawn_blocking(move || key_user(factory, key)).await,
            )
        }
    };
    let user_id = match loaded {
        Ok(Ok(Some(user_id))) => user_id,
        Ok(Ok(None)) => return plain(401, "unauthorized"),
        Ok(Err(err)) if err.kind() == ErrorKind::Unavailable => return state.guard.unavailable("service unavailable"),
        Ok(Err(err)) => return crate::http::internal_error(err.message()),
        Err(_) => return plain(500, "request worker stopped"),
    };
    let shutdown = state.shutdown.clone();
    let factory = Arc::clone(&state.factory);
    ws.on_upgrade(move |socket| run_socket(socket, factory, session_id, user_id, shutdown))
}

/// A page of another site cannot open a socket that carries this site's cookie, as Java's modifyHandshake. `what` names
/// the socket in the warning.
pub(crate) fn origin_allowed(factory: &ExecutionContextFactory, headers: &HeaderMap, what: &str) -> bool {
    let header = |name: axum::http::header::HeaderName| headers.get(name).and_then(|value| value.to_str().ok());
    let allowed = moqui_core::web::origin_allowed_by(
        Some(factory),
        header(axum::http::header::ORIGIN),
        header(axum::http::header::HOST),
    );
    if !allowed {
        tracing::warn!(origin = ?header(axum::http::header::ORIGIN), "{what} socket refused for its origin");
    }
    allowed
}

pub(crate) fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookies: Vec<&str> = headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    cookie_value(&cookies.join("; "), SESSION_COOKIE_NAME).filter(|value| !value.is_empty())
}

/// The userId of an HTTP session, or none when no one is logged in to it. Blocks on the request pool.
pub(crate) fn session_user(
    factory: Arc<ExecutionContextFactory>,
    session_id: String,
) -> Result<Option<String>, CoreError> {
    let owner = Arc::clone(&factory);
    let looked = Arc::clone(&factory);
    let rx = factory
        .request_pool()
        .try_submit(owner, move || user_of(&looked, &session_id))?;
    match rx.recv() {
        Ok(result) => result,
        Err(_) => Err(CoreError::new("request worker stopped")),
    }
}

/// The user of a login key, or none for a key that is unknown, expired or revoked.
pub(crate) fn key_user(factory: Arc<ExecutionContextFactory>, key: String) -> Result<Option<String>, CoreError> {
    let owner = Arc::clone(&factory);
    let rx = factory.request_pool().try_submit(owner, move || {
        if moqui_core::user::login_user_key(&key).is_err() {
            return Ok(None);
        }
        Ok(moqui_core::user::current_user_id()?.filter(|id| !id.is_empty()))
    })?;
    match rx.recv() {
        Ok(result) => result,
        Err(_) => Err(CoreError::new("request worker stopped")),
    }
}

fn user_of(factory: &ExecutionContextFactory, session_id: &str) -> Result<Option<String>, CoreError> {
    // a login the eventual flush has not written yet is in the live session of this node
    if let Some(user_id) = moqui_core::user::live_session_user_id(factory, session_id) {
        return Ok(user_id);
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz)
        .ok_or_else(|| CoreError::new("no execution context is entered"))?;
    let facade = factory
        .entity()
        .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
    let row = facade
        .find("moqui.server.HttpSession")?
        .eq("sessionId", Value::string(session_id))
        .one()?;
    Ok(row.and_then(|row| {
        row.get("userId")
            .and_then(Value::as_str)
            .filter(|user_id| !user_id.is_empty())
            .map(str::to_string)
    }))
}

async fn run_socket(
    mut socket: WebSocket,
    factory: Arc<ExecutionContextFactory>,
    session_id: String,
    user_id: String,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(SOCKET_QUEUE);
    let id = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
    let slot = Arc::new(Slot {
        id,
        user_id,
        session_id,
        topics: Mutex::new(HashSet::new()),
        tx,
        overflow: tokio::sync::Notify::new(),
    });
    register(&slot);
    tracing::debug!(session_id = %slot.session_id, user_id = %slot.user_id, "notification socket open");
    if *shutdown.borrow() {
        unregister(id);
        return;
    }
    let idle = idle_timeout(&factory);
    let mut last_heard = tokio::time::Instant::now();
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            () = slot.overflow.notified() => {
                tracing::warn!(session_id = %slot.session_id, "notification socket closed: the client fell behind");
                let _ = socket
                    .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 1008,
                        reason: "too many notifications waiting".into(),
                    })))
                    .await;
                break;
            }
            incoming = socket.recv() => {
                last_heard = tokio::time::Instant::now();
                if !handle_incoming(&mut socket, &factory, &slot, incoming).await {
                    break;
                }
            }
            push = rx.recv() => {
                let Some(push) = push else {
                    break;
                };
                if socket.send(Message::Text(push.text.into())).await.is_err() {
                    break;
                }
                // Marking it sent is a database write; it does not hold up the next notification.
                mark_after_send(&factory, &slot.user_id, push.message_id);
            }
            _ = ping.tick() => {
                if last_heard.elapsed() >= idle {
                    tracing::debug!(session_id = %slot.session_id, "notification socket idle, closed");
                    break;
                }
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
    tracing::debug!(session_id = %slot.session_id, user_id = %slot.user_id, "notification socket closed");
    unregister(id);
    collab::socket_closed(&factory, id);
    offline_when_gone(&factory, &slot.user_id);
}

/// The `webroot` webapp's `/notws` `endpoint` `timeout` in milliseconds, else an hour.
fn idle_timeout(factory: &ExecutionContextFactory) -> std::time::Duration {
    factory
        .webapp_child_setting("webroot", "endpoint /notws", "timeout")
        .parse::<u64>()
        .ok()
        .filter(|millis| *millis > 0)
        .map_or(DEFAULT_IDLE, std::time::Duration::from_millis)
}

async fn handle_incoming(
    socket: &mut WebSocket,
    factory: &Arc<ExecutionContextFactory>,
    slot: &Arc<Slot>,
    incoming: Option<Result<Message, axum::Error>>,
) -> bool {
    match incoming {
        Some(Ok(Message::Text(text))) => {
            // a co-editing frame (JSON with `collab`)
            if collab::handle(factory, slot, &text) {
                return true;
            }
            // A topic change is a set update on this socket; it needs no execution context or worker.
            if let Some(command) = parse_command(&text) {
                apply_command(slot, command);
            }
            true
        }
        Some(Ok(Message::Ping(payload))) => socket.send(Message::Pong(payload)).await.is_ok(),
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => false,
        Some(Ok(_)) => true,
    }
}

fn parse_command(text: &str) -> Option<TopicCommand> {
    let text = text.trim();
    let (subscribe, names) = if let Some(rest) = text.strip_prefix("subscribe:") {
        (true, rest)
    } else {
        (false, text.strip_prefix("unsubscribe:")?)
    };
    let names = names
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    if subscribe {
        Some(TopicCommand::Subscribe(names))
    } else {
        Some(TopicCommand::Unsubscribe(names))
    }
}

fn apply_command(slot: &Slot, command: TopicCommand) {
    let mut topics = lock(&slot.topics);
    match command {
        TopicCommand::Subscribe(names) => {
            for name in names {
                topics.insert(name);
            }
        }
        TopicCommand::Unsubscribe(names) => {
            for name in names {
                topics.remove(&name);
            }
        }
    }
}

fn mark_after_send(factory: &Arc<ExecutionContextFactory>, user_id: &str, message_id: String) {
    if message_id.is_empty() {
        return;
    }
    let factory = Arc::clone(factory);
    let user_id = user_id.to_string();
    tokio::spawn(async move {
        let ran = tokio::task::spawn_blocking(move || {
            factory
                .service_pools()
                .call(move || moqui_core::notify::mark_sent(&message_id, &user_id))
        })
        .await;
        if let Err(err) = flatten_call(ran) {
            tracing::warn!(error = %err, "could not mark notification sent");
        }
    });
}

/// After a socket of `user_id` closed: when no socket of the user is left, wait [`OFFLINE_GRACE`], check again, then mark
/// the user offline on a service worker, as [`mark_after_send`] marks a notification sent.
fn offline_when_gone(factory: &Arc<ExecutionContextFactory>, user_id: &str) {
    if has_socket(user_id) {
        return;
    }
    let factory = Arc::clone(factory);
    let user_id = user_id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(OFFLINE_GRACE).await;
        if has_socket(&user_id) {
            return;
        }
        let called = user_id.clone();
        let ran =
            tokio::task::spawn_blocking(move || factory.service_pools().call(move || disconnect_user(&called))).await;
        if let Err(err) = flatten_call(ran) {
            tracing::warn!(error = %err, user_id = %user_id, "could not mark the user offline");
        }
    });
}

fn has_socket(user_id: &str) -> bool {
    lock(sockets()).values().any(|slot| slot.user_id == user_id)
}

/// `disconnect#UserStatus` of `user_id`, as internal work. Its errors are the call's error.
fn disconnect_user(user_id: &str) -> Result<(), CoreError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz)
        .ok_or_else(|| CoreError::new("no execution context is entered"))?;
    let parameters = BTreeMap::from([("userId".to_string(), Value::string(user_id))]);
    let called = moqui_core::service::sync(DISCONNECT_SERVICE, &parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let errors = ec.message().errors().join("; ");
        ec.message().clear_errors();
        errors
    })
    .unwrap_or_default();
    called?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CoreError::new(errors))
    }
}

fn flatten_call(ran: Result<Result<(), CoreError>, tokio::task::JoinError>) -> Result<(), CoreError> {
    match ran {
        Ok(result) => result,
        Err(_) => Err(CoreError::new("service worker stopped")),
    }
}

/// Run `job` on a request worker in a fresh context, without blocking the async runtime.
async fn on_pool<T, F>(factory: &Arc<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CoreError> + Send + 'static,
{
    let factory = Arc::clone(factory);
    tokio::task::spawn_blocking(move || {
        let rx = factory.request_pool().try_submit(Arc::clone(&factory), job)?;
        rx.recv().map_err(|_| CoreError::new("request worker stopped"))?
    })
    .await
    .map_err(|_| CoreError::new("request worker panicked"))?
}

fn deliver_notice(message: &TopicMessage) {
    if catch_unwind(AssertUnwindSafe(|| fan_out_notice(message))).is_err() {
        tracing::warn!("notification websocket fan-out panicked");
    }
}

fn fan_out_notice(message: &TopicMessage) {
    // The client gets Java's wrapped message: the recipients are for routing here only.
    let mut client_body = message.body.clone();
    if let Some(object) = client_body.as_object_mut() {
        object.remove(moqui_core::notify::ROUTE_USER_IDS);
    }
    let text = client_body.to_string();
    let topic = json_str(&message.body, "topic");
    let message_id = json_str(&message.body, "notificationMessageId");
    let users = json_users(&message.body);
    let slots: Vec<Arc<Slot>> = lock(sockets()).values().cloned().collect();
    for slot in slots {
        if !users.iter().any(|user_id| user_id == &slot.user_id) {
            continue;
        }
        let wanted = {
            let topics = lock(&slot.topics);
            topics.contains("ALL") || (!topic.is_empty() && topics.contains(&topic))
        };
        if !wanted {
            continue;
        }
        let push = Push {
            text: text.clone(),
            message_id: message_id.clone(),
        };
        match slot.tx.try_send(push) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                unregister(slot.id);
                slot.overflow.notify_one();
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => unregister(slot.id),
        }
    }
}

fn json_str(body: &serde_json::Value, field: &str) -> String {
    body.get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn json_users(body: &serde_json::Value) -> Vec<String> {
    body.get(moqui_core::notify::ROUTE_USER_IDS)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every socket is registered, so each tab of a session gets the notifications. Java keys its endpoints by the WebSocket
/// session, which is one per connection.
fn register(slot: &Arc<Slot>) {
    lock(sockets()).insert(slot.id, Arc::clone(slot));
}

fn unregister(id: u64) {
    lock(sockets()).remove(&id);
}
