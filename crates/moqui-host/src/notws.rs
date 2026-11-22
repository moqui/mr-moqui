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
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::Response;
use moqui_core::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use moqui_core::notify::{TopicMessage, NOTIFICATION_MESSAGE};
use moqui_core::user::{session_cookie_name, verify_session_cookie};
use moqui_core::web::cookie_value;
use moqui_core::web::origin::{origin_credentialed_by, own_origins, request_site, SiteOrigin};
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use crate::http::{plain, HttpState, HttpsRequest};

// CalcGrid's rooms (moqui-calc), linked under the xlsx feature
#[cfg(feature = "xlsx")]
pub(crate) mod calc;
#[cfg(not(feature = "xlsx"))]
#[path = "notws/calc_off.rs"]
pub(crate) mod calc;
pub(crate) mod collab;
#[cfg(feature = "rtc")]
pub(crate) mod rtc;
#[cfg(not(feature = "rtc"))]
#[path = "notws/rtc_off.rs"]
pub(crate) mod rtc;

type CoreError = moqui_core::context::Error;

static PUSH: OnceLock<moqui_core::notify::Subscription> = OnceLock::new();
static NEXT_SOCKET: AtomicU64 = AtomicU64::new(1);

/// The open sockets of this node: by id, for direct call signaling ([`push_to_socket`]), and by user, so a notification
/// for a few users visits only their sockets, not every socket of the node (plans/archive/High-Level-Review.md HL-009). Both
/// change together under the one lock.
#[derive(Default)]
struct Registry {
    by_id: HashMap<u64, Arc<Slot>>,
    by_user: HashMap<String, Vec<u64>>,
}

impl Registry {
    fn insert(&mut self, slot: &Arc<Slot>) {
        self.by_id.insert(slot.id, Arc::clone(slot));
        self.by_user.entry(slot.user_id.clone()).or_default().push(slot.id);
    }

    /// Remove socket `id`, whenever it goes: closed, or its queue overflowed during a fan-out. A second removal does
    /// nothing.
    fn remove(&mut self, id: u64) {
        let Some(slot) = self.by_id.remove(&id) else {
            return;
        };
        if let Some(ids) = self.by_user.get_mut(&slot.user_id) {
            ids.retain(|known| *known != id);
            if ids.is_empty() {
                self.by_user.remove(&slot.user_id);
            }
        }
    }

    /// The sockets of `users`, each once.
    fn of_users(&self, users: &[String]) -> Vec<Arc<Slot>> {
        let mut seen = HashSet::new();
        users
            .iter()
            .filter(|user_id| seen.insert(user_id.as_str()))
            .filter_map(|user_id| self.by_user.get(user_id))
            .flatten()
            .filter_map(|id| self.by_id.get(id).cloned())
            .collect()
    }
}

fn sockets() -> &'static Mutex<Registry> {
    static SOCKETS: OnceLock<Mutex<Registry>> = OnceLock::new();
    SOCKETS.get_or_init(|| Mutex::new(Registry::default()))
}

/// Notifications waiting for one socket. A client that falls this far behind is closed rather than letting its queue
/// grow without end; the client reopens the socket and the notifications are in its list.
const SOCKET_QUEUE: usize = 256;
/// A ping goes out this often, so a dead peer is noticed and proxies keep the connection.
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// While the socket carries a call, this often instead: a mobile network's NAT that drops a quiet TCP mapping sooner
/// keeps it, and a dead socket is noticed in time for the leg's reconnect (plans/Comms-Messaging.md §8.3 item 13).
const PING_IN_CALL: std::time::Duration = std::time::Duration::from_secs(10);
const SESSION_CHECK: std::time::Duration = std::time::Duration::from_secs(60);
/// A send to a socket whose client takes nothing for this long fails, and the socket closes, so a blocked write does
/// not hold up the loop's own timeouts (`plans/archive/High-Level-Review.md` HL-027). The connection's write timeout does the
/// same under HTTP/1.1; this covers a socket over HTTP/2, where one stream's window does not block the connection.
pub(crate) const SEND_STALL: std::time::Duration = std::time::Duration::from_secs(60);
/// Topics one socket may follow; a subscribe past it is ignored, so one socket cannot grow its set without end.
const MAX_TOPICS: usize = 256;

/// Send `message` on `socket` within [`SEND_STALL`]. False when the socket is gone or the client took nothing.
pub(crate) async fn send_within(socket: &mut WebSocket, message: Message) -> bool {
    matches!(tokio::time::timeout(SEND_STALL, socket.send(message)).await, Ok(Ok(())))
}

#[derive(Clone)]
enum Credential {
    Session(String),
    LoginKeyHash(String),
}

impl Credential {
    /// The user the credential is for now, from a request worker, awaited without a waiting thread.
    async fn user(&self, factory: &Arc<ExecutionContextFactory>) -> Result<Option<String>, CoreError> {
        match self {
            // the session's user, while the account is not disabled for good or past its end date (impl-admin F42): a
            // live socket of an account disabled since it opened closes at the next check, as a revoked login key does
            Self::Session(id) => {
                let (looked, id) = (Arc::clone(factory), id.clone());
                on_pool(factory, move || {
                    let user = user_of(&looked, &id)?;
                    match user {
                        Some(user_id) if !moqui_core::user::user_account_closed(&user_id)? => Ok(Some(user_id)),
                        _ => Ok(None),
                    }
                })
                .await
            }
            Self::LoginKeyHash(hash) => {
                let hash = hash.clone();
                on_pool(factory, move || moqui_core::user::login_key_user_by_hash(&hash)).await
            }
        }
    }
}
/// Set to `/notws` while this server serves the socket; the html and qvt clients (`Header.html.fancy`, `qapps.xml`)
/// connect when it is.
pub(crate) const ENDPOINT_PROPERTY: &str = "webapp_notification_endpoint";
/// Java's `/notws` endpoint timeout: an hour with nothing from the client closes the socket.
const DEFAULT_IDLE: std::time::Duration = std::time::Duration::from_secs(3600);
/// How long after a user's last socket closed the user is marked offline, unless a socket of theirs came back: a page
/// reload opens its socket again well within it, so presence does not flicker.
const OFFLINE_GRACE: std::time::Duration = std::time::Duration::from_secs(20);
/// Marks a user offline at once and publishes it on the userStatus topic.
const DISCONNECT_SERVICE: &str = "org.moqui.chat.ChatServices.disconnect#UserStatus";

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

/// What one socket may hold of presence (SEC-TLS-113), from the webroot webapp: the clients it keeps in one CalcGrid
/// room or co-edited page (`socket-room-clients`), the bytes of one awareness (`socket-awareness-bytes`), and the
/// joins it makes a minute (`socket-joins-per-minute`), and the joins one user's sockets make a minute together
/// (`socket-user-joins-per-minute`).
#[derive(Clone, Copy)]
struct PresenceCaps {
    room_clients: usize,
    awareness_bytes: usize,
    joins_per_minute: u32,
    user_joins_per_minute: u32,
}

impl PresenceCaps {
    fn of(factory: &ExecutionContextFactory) -> Self {
        let number = |name: &str, default: usize| {
            factory
                .webapp_setting("webroot", name)
                .parse::<usize>()
                .ok()
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        Self {
            room_clients: number("socket-room-clients", 4),
            awareness_bytes: number("socket-awareness-bytes", 4096),
            joins_per_minute: u32::try_from(number("socket-joins-per-minute", 60)).unwrap_or(u32::MAX),
            user_joins_per_minute: u32::try_from(number("socket-user-joins-per-minute", 240)).unwrap_or(u32::MAX),
        }
    }
}

/// A token bucket a minute wide: `capacity` takes at once, given back at `capacity` a minute, so no burst at a
/// minute's edge gets twice the rate.
#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    at: std::time::Instant,
}

/// Take one from the bucket of `key` in `buckets`; false when it is empty. The map keeps at most 4,096 keys: past that
/// the full ones (unused for a minute) go.
fn take<K: std::hash::Hash + Eq + Clone>(buckets: &Mutex<HashMap<K, Bucket>>, key: &K, capacity: u32) -> bool {
    let capacity = f64::from(capacity.max(1));
    let now = std::time::Instant::now();
    let mut buckets = lock(buckets);
    if buckets.len() >= 4096 && !buckets.contains_key(key) {
        buckets.retain(|_, bucket| bucket.tokens + bucket.at.elapsed().as_secs_f64() * capacity / 60.0 < capacity);
    }
    let bucket = buckets.entry(key.clone()).or_insert(Bucket {
        tokens: capacity,
        at: now,
    });
    bucket.tokens = (bucket.tokens + now.duration_since(bucket.at).as_secs_f64() * capacity / 60.0).min(capacity);
    bucket.at = now;
    if bucket.tokens < 1.0 {
        return false;
    }
    bucket.tokens -= 1.0;
    true
}

/// Whether the socket `slot_id` of `user_id` may join a room or a page once more: the socket's own rate
/// ([`PresenceCaps::joins_per_minute`]) and its user's over all their sockets ([`PresenceCaps::user_joins_per_minute`]).
fn join_allowed(slot_id: u64, user_id: &str, caps: &PresenceCaps) -> bool {
    static SOCKETS: OnceLock<Mutex<HashMap<u64, Bucket>>> = OnceLock::new();
    static USERS: OnceLock<Mutex<HashMap<String, Bucket>>> = OnceLock::new();
    take(SOCKETS.get_or_init(Default::default), &slot_id, caps.joins_per_minute)
        && take(
            USERS.get_or_init(Default::default),
            &user_id.to_string(),
            caps.user_joins_per_minute,
        )
}

/// The joins a socket asked for in one room or page that are not members yet (SEC-TLS-113): counted with the members,
/// so a burst of joins in flight together cannot pass the room cap.
fn pending_joins() -> &'static Mutex<HashMap<(u64, String), usize>> {
    static PENDING: OnceLock<Mutex<HashMap<(u64, String), usize>>> = OnceLock::new();
    PENDING.get_or_init(Default::default)
}

/// A place held for a join in flight; given back when dropped, after the member is in or the join failed.
struct Reserved((u64, String));

impl Drop for Reserved {
    fn drop(&mut self) {
        let mut pending = lock(pending_joins());
        if let Some(count) = pending.get_mut(&self.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                pending.remove(&self.0);
            }
        }
    }
}

/// Hold a place for a join of the socket `slot_id` in `room`, where it has `members` clients already: none when those
/// and its joins in flight fill [`PresenceCaps::room_clients`].
fn reserve(slot_id: u64, room: &str, members: usize, max: usize) -> Option<Reserved> {
    let key = (slot_id, room.to_string());
    let mut pending = lock(pending_joins());
    let count = pending.entry(key.clone()).or_insert(0);
    if members + *count >= max {
        if *count == 0 {
            pending.remove(&key);
        }
        return None;
    }
    *count += 1;
    Some(Reserved(key))
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

pub(crate) async fn upgrade(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    site: RequestSite,
    State(state): State<Arc<HttpState>>,
) -> Response {
    if !origin_allowed(&state.factory, &headers, &site, "notification") {
        return plain(403, "origin not allowed");
    }
    // A browser has its session cookie; the terminal client (`moqui tui --server`) has its login key, and no session.
    let factory = Arc::clone(&state.factory);
    let (session_id, credential, loaded) = match session_cookie(&headers, site.https()) {
        Some(cookie) => {
            let session_id = match verify_session_cookie(&cookie) {
                Ok(Some(id)) if !id.is_empty() => id,
                Ok(_) | Err(_) => return plain(401, "unauthorized"),
            };
            let lookup_id = session_id.clone();
            let looked = Arc::clone(&factory);
            let found = on_pool(&factory, move || user_of(&looked, &lookup_id).map(|user| (user, false))).await;
            (
                session_id.clone(),
                Credential::Session(session_id),
                Ok::<_, tokio::task::JoinError>(found),
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
            let hashed = match moqui_core::user::hash_login_key(&key) {
                Ok(hash) => hash,
                Err(_) => return plain(401, "unauthorized"),
            };
            (
                "login-key".to_string(),
                Credential::LoginKeyHash(hashed),
                Ok::<_, tokio::task::JoinError>(on_pool(&factory, move || key_login(&key)).await),
            )
        }
    };
    let loaded = loaded.map(|result| {
        result.map(|(user, failed)| {
            if failed {
                site.record_auth_failure(&state);
            }
            user
        })
    });
    let user_id = match loaded {
        Ok(Ok(Some(user_id))) => user_id,
        Ok(Ok(None)) => return plain(401, "unauthorized"),
        Ok(Err(err)) if err.kind() == ErrorKind::Unavailable => return state.guard.unavailable("service unavailable"),
        Ok(Err(err)) => return crate::http::internal_error(err.message()),
        Err(_) => return plain(500, "request worker stopped"),
    };
    let shutdown = state.shutdown.clone();
    let factory = Arc::clone(&state.factory);
    ws.max_message_size(moqui_core::cluster::MAX_CLUSTER_BODY)
        .max_frame_size(moqui_core::cluster::MAX_CLUSTER_BODY)
        .on_upgrade(move |socket| run_socket(socket, factory, session_id, user_id, credential, shutdown))
}

/// The origin a request was made to (scheme, host and port), as the screens' CORS check works it out
/// (`web::origin::request_site`): https over this listener's TLS or a believed proxy's `X-Forwarded-Proto`, and the
/// host and port of a believed proxy's `X-Forwarded-Host` and `X-Forwarded-Port`, else of `Host`.
pub(crate) struct RequestSite(Option<SiteOrigin>, Option<std::net::IpAddr>, bool);

impl RequestSite {
    /// Whether the request is https (this listener's TLS, or a believed proxy's `X-Forwarded-Proto`): its session
    /// cookie's name ([`moqui_core::user::session_cookie_name`]).
    pub(crate) fn https(&self) -> bool {
        self.2
    }

    /// The client's address as web protection sees it (a believed proxy's client header, else the peer): the call
    /// socket's (`rtc.rs`).
    #[cfg(feature = "rtc")]
    pub(crate) fn client(&self) -> Option<std::net::IpAddr> {
        self.1
    }

    pub(crate) fn record_auth_failure(&self, state: &HttpState) {
        if let Some(client) = self.1 {
            state.guard.record(client, crate::guard::Event::AuthFailure);
        }
    }
}

impl FromRequestParts<Arc<HttpState>> for RequestSite {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<HttpState>) -> Result<Self, Self::Rejection> {
        let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok());
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip());
        // set by the listener: TLS, or a believed proxy's X-Forwarded-Proto of https
        let https = parts.extensions.get::<HttpsRequest>().is_some();
        let conf = state.guard.conf();
        let believed = peer.is_some_and(|peer| conf.addressing().believes(peer));
        // a believed proxy's own values, from the hop the client address is read from (SEC-WEB-91)
        let forwarded =
            |name: &str| peer.and_then(|peer| crate::http::forwarded(&parts.headers, peer, conf.addressing(), name));
        Ok(Self(
            request_site(
                https,
                believed,
                header("host"),
                forwarded("x-forwarded-host").as_deref(),
                forwarded("x-forwarded-port").as_deref(),
            ),
            peer.map(|peer| state.guard.client_of(peer, &parts.headers)),
            https,
        ))
    }
}

/// A page of another site cannot open a socket that carries this site's cookie, as Java's modifyHandshake, but with
/// the whole origin compared (scheme, host and port; Java compares the host): the request's own origin, the conf
/// webapp's URLs or the webroot's `allow-origins`. An `allow-origins` of `*` does not count: a browser opens the socket
/// with the session cookie, and `*` never comes with credentials (SEC-WEB-10). `what` names the socket in the warning.
pub(crate) fn origin_allowed(
    factory: &ExecutionContextFactory,
    headers: &HeaderMap,
    site: &RequestSite,
    what: &str,
) -> bool {
    let header = |name: axum::http::header::HeaderName| headers.get(name).and_then(|value| value.to_str().ok());
    let own = own_origins(factory, site.0.clone());
    let allowed = origin_credentialed_by(Some(factory), header(axum::http::header::ORIGIN), &own);
    if !allowed {
        tracing::warn!(origin = ?header(axum::http::header::ORIGIN), "{what} socket refused for its origin");
    }
    allowed
}

/// The session cookie of a request, by the name of its scheme: over https only `__Host-moqui.session` (SEC-WEB-05).
pub(crate) fn session_cookie(headers: &HeaderMap, https: bool) -> Option<String> {
    let cookies: Vec<&str> = headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    cookie_value(&cookies.join("; "), session_cookie_name(https)).filter(|value| !value.is_empty())
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
pub(crate) fn key_user(
    factory: Arc<ExecutionContextFactory>,
    key: String,
) -> Result<(Option<String>, bool), CoreError> {
    let owner = Arc::clone(&factory);
    let rx = factory.request_pool().try_submit(owner, move || key_login(&key))?;
    match rx.recv() {
        Ok(result) => result,
        Err(_) => Err(CoreError::new("request worker stopped")),
    }
}

/// On a request worker: log in with `key`; the user, and whether the attempt counts as an authentication failure.
pub(crate) fn key_login(key: &str) -> Result<(Option<String>, bool), CoreError> {
    let login = moqui_core::user::login_user_key(key);
    let failed = moqui_core::web::take_auth_failure();
    let user = if login.is_ok() {
        moqui_core::user::current_user_id()?.filter(|id| !id.is_empty())
    } else {
        None
    };
    Ok((user, failed))
}

pub(crate) fn user_of(factory: &ExecutionContextFactory, session_id: &str) -> Result<Option<String>, CoreError> {
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
    credential: Credential,
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
    let mut ping = tokio::time::interval(PING_IN_CALL);
    ping.tick().await;
    let mut pinged = tokio::time::Instant::now();
    let mut check = tokio::time::interval(SESSION_CHECK);
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
                let _ = send_within(&mut socket, Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 1008,
                    reason: "too many notifications waiting".into(),
                })))
                .await;
                break;
            }
            _ = check.tick() => {
                let current = credential.user(&factory).await;
                if !matches!(current, Ok(Some(ref user)) if user == &slot.user_id) {
                    let _ = send_within(&mut socket, Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 1008,
                        reason: "authentication is no longer valid".into(),
                    }))).await;
                    break;
                }
            }
            incoming = socket.recv() => {
                if matches!(incoming, Some(Ok(Message::Text(_) | Message::Binary(_)))) {
                    last_heard = tokio::time::Instant::now();
                }
                if !handle_incoming(&mut socket, &factory, &slot, incoming).await {
                    break;
                }
            }
            push = rx.recv() => {
                let Some(push) = push else {
                    break;
                };
                if !collab::may_deliver(&slot, &push.text, false) { continue; }
                if !send_within(&mut socket, Message::Text(push.text.into())).await {
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
                if pinged.elapsed() + PING_IN_CALL / 2 < PING_EVERY && !rtc::in_call(id) {
                    continue;
                }
                pinged = tokio::time::Instant::now();
                if !send_within(&mut socket, Message::Ping(Vec::new().into())).await {
                    break;
                }
            }
        }
    }
    tracing::debug!(session_id = %slot.session_id, user_id = %slot.user_id, "notification socket closed");
    end_socket(&factory, id, &mut rx);
    offline_when_gone(&factory, &slot.user_id);
}

/// Let a closed socket go: its receiver is closed first, so a join still in flight that admits a member after the
/// members are let go here finds the socket closed (`slot.tx.is_closed()`) and leaves at once (collab and calc `admit`,
/// SEC-TLS-111); then its slot, its co-editing and sheet members and its calls.
fn end_socket(factory: &Arc<ExecutionContextFactory>, id: u64, rx: &mut tokio::sync::mpsc::Receiver<Push>) {
    rx.close();
    unregister(id);
    collab::socket_closed(factory, id);
    calc::socket_closed(factory, id);
    rtc::socket_closed(factory, id);
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
            if collab::handle(factory, slot, &text, None).await {
                return true;
            }
            // a CalcGrid frame (JSON with `calc`)
            if calc::handle(factory, slot, &text) {
                return true;
            }
            // a call signaling frame (JSON with `rtc`)
            if rtc::handle(factory, slot, &text) {
                return true;
            }
            // A topic change is a set update on this socket; it needs no execution context or worker.
            if let Some(command) = parse_command(&text) {
                apply_command(slot, command);
            }
            true
        }
        Some(Ok(Message::Ping(payload))) => send_within(socket, Message::Pong(payload)).await,
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
                if topics.len() >= MAX_TOPICS && !topics.contains(&name) {
                    tracing::warn!(session_id = %slot.session_id, "notification socket over {MAX_TOPICS} topics; {name} not followed");
                    continue;
                }
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

#[cfg_attr(not(feature = "rtc"), allow(dead_code))]
/// Sends `text` to the socket `slot_id` of this server, when it is open; false when it is not, or its queue is full
/// (a call frame is dropped then, and the client renegotiates).
pub(crate) fn push_to_socket(slot_id: u64, text: String) -> bool {
    let Some(slot) = lock(sockets()).by_id.get(&slot_id).cloned() else {
        return false;
    };
    slot.tx
        .try_send(Push {
            text,
            message_id: String::new(),
        })
        .is_ok()
}

fn has_socket(user_id: &str) -> bool {
    lock(sockets()).by_user.contains_key(user_id)
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

/// Run `job` on a request worker in a fresh context, without blocking the async runtime: the host's one async adapter
/// ([`crate::worker::submit`]).
async fn on_pool<T, F>(factory: &Arc<ExecutionContextFactory>, job: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CoreError> + Send + 'static,
{
    crate::worker::submit(Arc::clone(factory), job).await
}

fn deliver_notice(message: &TopicMessage) {
    if catch_unwind(AssertUnwindSafe(|| fan_out_notice(message))).is_err() {
        tracing::warn!("notification websocket fan-out panicked");
    }
}

/// Queue the notification on the sockets of its recipients that follow its topic. Out: the sockets visited, which are
/// only the recipients' (none for no recipients), whatever else is open.
fn fan_out_notice(message: &TopicMessage) -> usize {
    let users = json_users(&message.body);
    if users.is_empty() {
        return 0;
    }
    // only the recipients' handles are taken under the registry's lock
    let slots = lock(sockets()).of_users(&users);
    if slots.is_empty() {
        return 0;
    }
    // The client gets Java's wrapped message: the recipients are for routing here only.
    let mut client_body = message.body.clone();
    if let Some(object) = client_body.as_object_mut() {
        object.remove(moqui_core::notify::ROUTE_USER_IDS);
    }
    let text = client_body.to_string();
    let topic = json_str(&message.body, "topic");
    let message_id = json_str(&message.body, "notificationMessageId");
    let visited = slots.len();
    for slot in slots {
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
    visited
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
    lock(sockets()).insert(slot);
}

fn unregister(id: u64) {
    lock(sockets()).remove(id);
}

#[cfg(test)]
#[path = "notws_tests.rs"]
mod tests;
