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

//! Cluster membership, the shared certificate, and fan-out of the two in-process topics.
//! The HTTPS listener and the peer client are installed later. This wave posts through [`Sender`].
//!
//! Fan-out is not done on the thread that published: the message goes on a queue, and one sender thread posts it to
//! the live peers, as `plans/archive/Cluster.md` says. Invalidations waiting together are sent once per entity, since a peer
//! clears the entity's whole cache for one. The peer list is kept and read again on the heartbeat tick.
//!
//! Each factory keeps its own state (sender, member id, queue, peers): a write or notification fans out only to the
//! peers of the factory whose entity facade made it, so two factories in one process do not post for each other.

#[cfg(all(test, feature = "cluster-tls"))]
mod tests;

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_db::sql::{
    delete as sql_delete, insert as sql_insert, select as sql_select, update as sql_update, Condition, FieldRef,
    LimitOffset,
};
use moqui_lib::codec::hex_encode;
use moqui_lib::env;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use sha2::{Digest, Sha256};

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::driver::DbRow;
use crate::entity::exec::with_reserve_tx;
use crate::entity::EntityFacade;
use crate::notify::{self, TopicMessage, COLLAB_UPDATE, ENTITY_CACHE_INVALIDATE, NOTIFICATION_MESSAGE};
use crate::user::{now_millis, now_value, value_millis};

const MEMBER: &str = "moqui.server.ClusterMember";
const MESSAGE: &str = "moqui.server.ClusterMessage";
/// The base URL every `cluster_transport=notify` member advertises. One post to it reaches all of them.
pub const NOTIFY_BASE_URL: &str = "pg-notify:";
const MEMBER_COLUMNS: &[&str] = &[
    "instanceId",
    "clusterBaseUrl",
    "heartbeat",
    "sessionKeyFingerprint",
    "certFingerprint",
    "sshHostKeyFingerprint",
];
const DEFAULT_PORT: u16 = 8444;
const RETRIES: u32 = 3;
/// Waits between tries of one post: a busy peer answers 503 and is given a moment.
const RETRY_WAITS: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(500)];
/// Messages waiting for the sender thread. Past this a message is dropped and counted; an invalidation is safe to
/// lose this way, since every peer still drops its own cache on its own writes and on the next invalidation.
const OUTBOX_LIMIT: usize = 10_000;
/// A peer whose post failed every try is skipped this long, so a peer that is down does not slow every message.
const PEER_BACKOFF: Duration = Duration::from_secs(5);
/// A member row with no heartbeat in this many liveness windows is deleted.
const DEAD_WINDOWS: i64 = 10;

/// How long the sender thread waits for more before it ends. The next message starts another.
const SENDER_IDLE: Duration = Duration::from_secs(30);

/// One factory's cluster: its sender, its member id, and the queue its sender thread posts from. Kept with
/// [`ExecutionContextFactory::extension`], so two factories in one process fan out each to its own peers.
pub(crate) struct ClusterState {
    factory: Mutex<Weak<ExecutionContextFactory>>,
    /// This member's id, set by [`start`]. Empty before.
    instance_id: Mutex<String>,
    sender: Mutex<Option<Arc<dyn Sender>>>,
    no_sender_logged: AtomicBool,
    queue: Mutex<Queue>,
    ready: Condvar,
    peers: Mutex<PeerCache>,
    dropped: AtomicU64,
    /// Batches the sender thread has taken and not finished, so a test can wait for the posts.
    in_flight: AtomicUsize,
    loopback_warned: AtomicBool,
}

/// The messages waiting for the sender thread, and whether that thread is running.
#[derive(Default)]
struct Queue {
    messages: VecDeque<(&'static str, TopicMessage)>,
    sending: bool,
}

#[derive(Default)]
struct PeerCache {
    urls: Vec<String>,
    /// When the list was read, by the clock of `now_millis`; zero is never.
    read_at: i64,
    /// Peers to skip until the time given, after every try of a post failed.
    down_until: HashMap<String, Instant>,
}

/// The entity facade's link to the cluster state of the factory that started it, for [`note_write`].
#[derive(Default)]
struct ClusterLink(Weak<ClusterState>);

impl ClusterState {
    fn new() -> Self {
        Self {
            factory: Mutex::new(Weak::new()),
            instance_id: Mutex::new(String::new()),
            sender: Mutex::new(None),
            no_sender_logged: AtomicBool::new(false),
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
            peers: Mutex::new(PeerCache::default()),
            dropped: AtomicU64::new(0),
            in_flight: AtomicUsize::new(0),
            loopback_warned: AtomicBool::new(false),
        }
    }

    fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|_| Self::new())
    }

    fn instance_id(&self) -> String {
        lock(&self.instance_id).clone()
    }

    fn sender(&self) -> Option<Arc<dyn Sender>> {
        lock(&self.sender).clone()
    }
}

/// Posts one topic message to a peer. The host installs the HTTPS client. Tests install a recorder.
pub trait Sender: Send + Sync {
    /// `base_url` has no trailing slash. `topic` is the topic name, not a path.
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<(), Error>;
}

/// Replace `factory`'s sender. A later call replaces the previous one.
pub fn install_sender(factory: &ExecutionContextFactory, sender: Arc<dyn Sender>) {
    let state = ClusterState::of(factory);
    *lock(&state.sender) = Some(sender);
    state.no_sender_logged.store(false, Ordering::Relaxed);
}

/// This member's id once [`start`] has run for `factory`.
pub fn instance_id(factory: &ExecutionContextFactory) -> Option<String> {
    Some(ClusterState::of(factory).instance_id()).filter(|id| !id.is_empty())
}

/// Register this instance, check live peers, and link the entity facade to `factory`'s fan-out.
///
/// The host calls this after [`ExecutionContextFactory::set_entity`]. A second call refreshes the
/// member row and does not add another heartbeat hook.
///
/// # Errors
///
/// Returns an error when the entity facade is missing, the certificate pair does not match, or a
/// live peer's fingerprint differs.
pub fn start(factory: &Arc<ExecutionContextFactory>) -> Result<(), Error> {
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))?;
    let instance_id = ensure_instance_id(factory)?;
    let fingerprints = configured_fingerprints()?;
    let base_url = cluster_base_url()?;
    reject_peer_mismatch(&facade, factory, &instance_id, &fingerprints)?;
    write_member(
        &facade,
        &Member {
            instance_id: instance_id.clone(),
            base_url,
            heartbeat: millis_value(db_now(&facade)),
            session_fp: fingerprints.session,
            cert_fp: fingerprints.cert,
            ssh_fp: env::get_prop_or_env(SSH_FINGERPRINT_PROPERTY),
        },
    )?;
    let state = ClusterState::of(factory);
    *lock(&state.factory) = Arc::downgrade(factory);
    lock(&state.instance_id).clone_from(&instance_id);
    // The member list is read again on the next send: this start may have joined a cluster or left one.
    *lock(&state.peers) = PeerCache::default();
    facade.with_slot(ClusterLink::default, |link| link.0 = Arc::downgrade(&state));
    install_heartbeat(factory, instance_id);
    Ok(())
}

/// Wait until `factory`'s sender thread has posted everything queued, for tests.
#[cfg(all(test, feature = "cluster-tls"))]
pub(crate) fn wait_for_sends(factory: &ExecutionContextFactory, timeout: Duration) -> bool {
    let state = ClusterState::of(factory);
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        let empty = lock(&state.queue).messages.is_empty();
        if empty && state.in_flight.load(Ordering::SeqCst) == 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Clear the named entity's one-cache for `entity-cache-invalidate`, then publish `message` on this process's bus.
/// It is not posted onward: only [`publish`] fans out.
///
/// # Errors
///
/// Returns an error when `topic` is unknown, or the cache body has no entity name.
pub fn apply_remote(facade: &EntityFacade, topic: &str, message: &TopicMessage) -> Result<(), Error> {
    if topic == ENTITY_CACHE_INVALIDATE {
        let name = message
            .body
            .get("entityName")
            .and_then(serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Error::new("entity-cache-invalidate body has no entityName"))?;
        // With the written row's key, only the cached finds it may have changed go.
        let primary_key: Option<crate::entity::FieldMap> = message
            .body
            .get("primaryKey")
            .and_then(serde_json::Value::as_object)
            .filter(|keys| !keys.is_empty())
            .map(|keys| {
                keys.iter()
                    .map(|(name, value)| (std::sync::Arc::from(name.as_str()), moqui_lib::json::from_json(value)))
                    .collect()
            });
        crate::entity::cache::clear_one_cache(facade, name, primary_key.as_ref()).map_err(Error::from)?;
    } else if topic != NOTIFICATION_MESSAGE && topic != COLLAB_UPDATE {
        return Err(Error::new(format!("unknown topic {topic}")));
    }
    notify::publish(topic, message)
}

/// Publish `message` on this process's bus, then queue it for the peers of the factory that started `facade`.
///
/// # Errors
///
/// Returns an error when `topic` is not a known topic.
pub fn publish(facade: &EntityFacade, topic: &'static str, message: &TopicMessage) -> Result<(), Error> {
    notify::publish(topic, message)?;
    if let Some(state) = linked_state(facade) {
        fan_out(&state, topic, message);
    }
    Ok(())
}

/// Whether `facade`'s factory posts to cluster peers, whose sockets may have the users of a notification.
pub(crate) fn is_linked(facade: &EntityFacade) -> bool {
    linked_state(facade).is_some()
}

fn linked_state(facade: &EntityFacade) -> Option<Arc<ClusterState>> {
    facade.with_slot_read(ClusterLink::default, |link| link.0.upgrade())
}

/// Publish a cache invalidation after the local clear. An open transaction waits for commit.
pub(crate) fn note_write(
    facade: &EntityFacade,
    entity_name: &str,
    pk_fields: &[String],
    values: &crate::entity::FieldMap,
) -> Result<(), Error> {
    // No peer and no local listener: there is no one to tell, so the message (a UUID and a JSON body) is not built.
    if !is_linked(facade) && !notify::has_listeners(ENTITY_CACHE_INVALIDATE) {
        return Ok(());
    }
    let message = TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4().map_err(Error::from)?,
        body: invalidation_body(entity_name, pk_fields, values),
    };
    let tx = facade.transactions_for(entity_name).map_err(Error::from)?;
    if tx.in_transaction() {
        let facade = facade.clone();
        tx.on_commit(move || {
            if let Err(err) = publish(&facade, ENTITY_CACHE_INVALIDATE, &message) {
                tracing::warn!("cluster cache publish failed: {err}");
            }
        })
        .map_err(Error::from)?;
        return Ok(());
    }
    publish(facade, ENTITY_CACHE_INVALIDATE, &message)
}

/// One self-signed pair. Both values are standard base64 of DER, one line each.
#[cfg(feature = "cluster-tls")]
pub(crate) fn generate_cert_pair() -> Result<(String, String), Error> {
    let key = rcgen::KeyPair::generate().map_err(cert_err)?;
    let mut params = rcgen::CertificateParams::new(vec!["moqui-cluster".to_string()]).map_err(cert_err)?;
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "moqui-cluster");
    params.distinguished_name = name;
    let today = jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date();
    let until = today
        .checked_add(jiff::Span::new().years(10))
        .map_err(|err| Error::new(format!("could not generate cluster tls material: {err}")))?;
    params.not_before = rcgen::date_time_ymd(ymd_year(today), ymd_month(today), ymd_day(today));
    params.not_after = rcgen::date_time_ymd(ymd_year(until), ymd_month(until), ymd_day(until));
    let cert = params.self_signed(&key).map_err(cert_err)?;
    let cert_b64 = STANDARD.encode(cert.der());
    let key_b64 = STANDARD.encode(key.serialized_der());
    Ok((cert_b64, key_b64))
}

#[cfg(feature = "cluster-tls")]
fn ymd_year(date: jiff::civil::Date) -> i32 {
    i32::from(date.year())
}

#[cfg(feature = "cluster-tls")]
fn ymd_month(date: jiff::civil::Date) -> u8 {
    u8::try_from(date.month()).unwrap_or(1)
}

#[cfg(feature = "cluster-tls")]
fn ymd_day(date: jiff::civil::Date) -> u8 {
    u8::try_from(date.day()).unwrap_or(1)
}

#[cfg(feature = "cluster-tls")]
fn cert_err(err: impl std::fmt::Display) -> Error {
    Error::new(format!("could not generate cluster tls material: {err}"))
}

struct Fingerprints {
    session: String,
    cert: String,
}

struct Member {
    instance_id: String,
    base_url: String,
    heartbeat: Value,
    session_fp: String,
    cert_fp: String,
    /// This member's SSH host key, which a peer pins for the `ssh://` cluster channel. Empty without one.
    ssh_fp: String,
}

/// `instance_id` when it is set, else the id in `runtime/db/instance_id`, else a new one written there. It is not
/// written to `runtime/.env`, which `plans/archive/Cluster.md` says to copy between members: two members with one id would
/// each take the other's member row for its own.
/// Nor is it set as a property, which another factory in this process would then take for its own.
fn ensure_instance_id(factory: &ExecutionContextFactory) -> Result<String, Error> {
    let current = env::get_prop_or_env("instance_id");
    if !current.is_empty() {
        return Ok(current);
    }
    let file = factory.runtime_dir().join("db").join("instance_id");
    if let Ok(text) = std::fs::read_to_string(&file) {
        let kept = text.trim().to_string();
        if !kept.is_empty() {
            return Ok(kept);
        }
    }
    let id = crate::entity::uuid_v4().map_err(Error::from)?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::new(format!("could not make {}: {err}", parent.display())))?;
    }
    std::fs::write(&file, format!("{id}\n"))
        .map_err(|err| Error::new(format!("could not write {}: {err}", file.display())))?;
    Ok(id)
}

fn configured_fingerprints() -> Result<Fingerprints, Error> {
    let session_key = require_configured("session_cookie_key")?;
    let cert_b64 = require_configured("cluster_tls_cert")?;
    let _key = require_configured("cluster_tls_key")?;
    let der = decode_der("cluster_tls_cert", &cert_b64)?;
    Ok(Fingerprints {
        session: sha256_hex(session_key.as_bytes()),
        cert: sha256_hex(&der),
    })
}

fn require_configured(name: &str) -> Result<String, Error> {
    let value = env::get_prop_or_env(name);
    if value.is_empty() || value.ends_with("CHANGEME") {
        return Err(Error::new(format!("{name} is not configured")));
    }
    Ok(value)
}

fn decode_der(name: &str, text: &str) -> Result<Vec<u8>, Error> {
    STANDARD
        .decode(text.trim())
        .map_err(|_| Error::new(format!("{name} is not base64 der")))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

/// The property the host sets to this member's SSH host key fingerprint, for the member row.
pub const SSH_FINGERPRINT_PROPERTY: &str = "cluster_ssh_host_key_fingerprint";
/// The property the host sets to the address its SSH server bound, for the default `ssh://` base URL.
pub const SSH_ADDRESS_PROPERTY: &str = "cluster_ssh_address";

fn cluster_base_url() -> Result<String, Error> {
    let configured = env::get_prop_or_env("cluster_base_url");
    let url = if !configured.is_empty() {
        configured
    } else if env::get_prop_or_env("cluster_transport") == "notify" {
        NOTIFY_BASE_URL.to_string()
    } else if env::get_prop_or_env("cluster_transport") == "ssh" {
        let address = env::get_prop_or_env(SSH_ADDRESS_PROPERTY);
        if address.is_empty() {
            return Err(Error::new(
                "cluster_transport is ssh but the SSH server is not listening; set ssh_port",
            ));
        }
        format!("ssh://{address}")
    } else {
        format!("https://127.0.0.1:{}", cluster_port()?)
    };
    Ok(trim_base(&url))
}

fn cluster_port() -> Result<u16, Error> {
    let raw = env::get_prop_or_env("cluster_https_port");
    if raw.is_empty() {
        return Ok(DEFAULT_PORT);
    }
    raw.parse().map_err(|_| Error::new("cluster_https_port is not a port"))
}

fn trim_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn window_millis(factory: &ExecutionContextFactory) -> i64 {
    let millis = i64::try_from(factory.service_pools().check_interval().as_millis()).unwrap_or(60_000);
    millis.saturating_mul(3)
}

fn is_live(heartbeat: Option<i64>, now: i64, window: i64) -> bool {
    let Some(stamp) = heartbeat else {
        return false;
    };
    now.saturating_sub(stamp) <= window
}

fn reject_peer_mismatch(
    facade: &EntityFacade,
    factory: &ExecutionContextFactory,
    instance_id: &str,
    fingerprints: &Fingerprints,
) -> Result<(), Error> {
    let now = db_now(facade);
    let window = window_millis(factory);
    for member in load_members(facade)? {
        if member.instance_id == instance_id || !is_live(member.heartbeat, now, window) {
            continue;
        }
        if member.session_fp != fingerprints.session {
            return Err(Error::new("session_cookie_key does not match a live cluster member"));
        }
        if member.cert_fp != fingerprints.cert {
            return Err(Error::new("cluster_tls_cert does not match a live cluster member"));
        }
    }
    Ok(())
}

fn install_heartbeat(factory: &Arc<ExecutionContextFactory>, instance_id: String) {
    if factory.cluster_heartbeat.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(factory);
    factory.service_pools().add_tick_hook(move || {
        let Some(factory) = weak.upgrade() else {
            return;
        };
        let Some(facade) = factory.entity() else {
            tracing::warn!("cluster heartbeat has no entity facade");
            return;
        };
        // The database's clock, so members whose clocks differ still see each other as live.
        let now = db_now(&facade);
        if let Err(err) = touch_member(&facade, &instance_id, now) {
            tracing::warn!("cluster heartbeat failed: {err}");
        }
        if let Err(err) = delete_dead_members(&facade, now.saturating_sub(window_millis(&factory) * DEAD_WINDOWS)) {
            tracing::warn!("cluster member cleanup failed: {err}");
        }
        let state = ClusterState::of(&factory);
        refresh_peers(&state, &factory);
        warn_loopback_peers(&state);
    });
}

fn fan_out(state: &Arc<ClusterState>, topic: &'static str, message: &TopicMessage) {
    {
        // A single member has no one to tell: with no peers at the last read, nothing is queued.
        let peers = lock(&state.peers);
        if peers.read_at != 0 && peers.urls.is_empty() {
            return;
        }
    }
    let mut queue = lock(&state.queue);
    if queue.messages.len() >= OUTBOX_LIMIT {
        let dropped = state.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped.is_power_of_two() {
            tracing::warn!("cluster fan-out queue is full; {dropped} messages dropped");
        }
        return;
    }
    queue.messages.push_back((topic, message.clone()));
    if queue.sending {
        state.ready.notify_one();
        return;
    }
    let running = Arc::clone(state);
    let spawned = std::thread::Builder::new()
        .name("moqui-cluster-send".to_string())
        .spawn(move || send_loop(&running));
    match spawned {
        Ok(_) => queue.sending = true,
        // The message stays queued for the next try to start the thread.
        Err(err) => tracing::error!("cluster sender thread did not start: {err}"),
    }
}

/// The sender thread: take what is waiting, collapse the invalidations, and post each to every live peer. It ends
/// after [`SENDER_IDLE`] with nothing to send.
fn send_loop(state: &ClusterState) {
    loop {
        let batch: Vec<(&'static str, TopicMessage)> = {
            let mut queue = lock(&state.queue);
            while queue.messages.is_empty() {
                let (next, waited) = state
                    .ready
                    .wait_timeout(queue, SENDER_IDLE)
                    .unwrap_or_else(|err| err.into_inner());
                queue = next;
                if waited.timed_out() && queue.messages.is_empty() {
                    queue.sending = false;
                    return;
                }
            }
            state.in_flight.fetch_add(1, Ordering::SeqCst);
            queue.messages.drain(..).collect()
        };
        // A panic in one batch must not end the thread that every later message waits on.
        let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send_batch(state, coalesce(batch))));
        if sent.is_err() {
            tracing::error!("cluster fan-out batch panicked");
        }
        state.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

fn send_batch(state: &ClusterState, batch: Vec<(&'static str, TopicMessage)>) {
    let Some(factory) = lock(&state.factory).upgrade() else {
        return;
    };
    let peers = current_peers(state, &factory);
    if peers.is_empty() {
        return;
    }
    let Some(sender) = state.sender() else {
        if !state.no_sender_logged.swap(true, Ordering::Relaxed) {
            tracing::warn!("cluster fan-out has no sender");
        }
        return;
    };
    for (topic, message) in &batch {
        let payload = match payload_bytes(message) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!("cluster fan-out could not encode a message: {err}");
                continue;
            }
        };
        for base_url in &peers {
            if peer_down(state, base_url) {
                continue;
            }
            if !post_with_retry(&sender, base_url, topic, &payload) {
                mark_peer_down(state, base_url);
            }
        }
    }
}

/// One invalidation for each entity, in the place of the first: a peer clears the entity's cache whatever the key.
/// Notifications are all kept, in order.
fn coalesce(batch: Vec<(&'static str, TopicMessage)>) -> Vec<(&'static str, TopicMessage)> {
    let mut seen = std::collections::HashSet::new();
    batch
        .into_iter()
        .filter(|(topic, message)| {
            *topic != ENTITY_CACHE_INVALIDATE
                || seen.insert(
                    message
                        .body
                        .get("entityName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                )
        })
        .collect()
}

/// The live peers, read again when the heartbeat tick has not refreshed them within a liveness window.
fn current_peers(state: &ClusterState, factory: &ExecutionContextFactory) -> Vec<String> {
    let window = window_millis(factory);
    {
        let peers = lock(&state.peers);
        if peers.read_at != 0 && now_millis().saturating_sub(peers.read_at) < window {
            return peers.urls.clone();
        }
    }
    refresh_peers(state, factory);
    lock(&state.peers).urls.clone()
}

fn refresh_peers(state: &ClusterState, factory: &ExecutionContextFactory) {
    let Some(facade) = factory.entity() else {
        return;
    };
    match live_peer_urls(state, factory, &facade) {
        Ok(urls) => {
            let mut peers = lock(&state.peers);
            peers.urls = urls;
            peers.read_at = now_millis();
        }
        Err(err) => tracing::warn!("cluster fan-out could not load members: {err}"),
    }
}

fn peer_down(state: &ClusterState, base_url: &str) -> bool {
    let mut peers = lock(&state.peers);
    match peers.down_until.get(base_url) {
        Some(until) if Instant::now() < *until => true,
        Some(_) => {
            peers.down_until.remove(base_url);
            false
        }
        None => false,
    }
}

fn mark_peer_down(state: &ClusterState, base_url: &str) {
    lock(&state.peers)
        .down_until
        .insert(base_url.to_string(), Instant::now() + PEER_BACKOFF);
}

fn live_peer_urls(
    state: &ClusterState,
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
) -> Result<Vec<String>, Error> {
    let instance_id = state.instance_id();
    let now = db_now(facade);
    let window = window_millis(factory);
    let mut bases = Vec::new();
    for member in load_members(facade)? {
        if member.instance_id == instance_id || !is_live(member.heartbeat, now, window) {
            continue;
        }
        let base = trim_base(&member.base_url);
        // Every notify member is reached by the one NOTIFY, so they are one peer here.
        let base = if base.starts_with(NOTIFY_BASE_URL) {
            NOTIFY_BASE_URL.to_string()
        } else {
            base
        };
        if !base.is_empty() && !bases.contains(&base) {
            bases.push(base);
        }
    }
    Ok(bases)
}

fn payload_bytes(message: &TopicMessage) -> Result<Vec<u8>, Error> {
    let value = serde_json::json!({
        "origin_id": message.origin_id,
        "message_id": message.message_id,
        "body": message.body,
    });
    serde_json::to_vec(&value).map_err(|err| Error::new(format!("could not encode cluster message: {err}")))
}

/// Post, trying again after a short and then a longer wait. False when every try failed.
fn post_with_retry(sender: &Arc<dyn Sender>, base_url: &str, topic: &str, payload: &[u8]) -> bool {
    for attempt in 0..RETRIES {
        match sender.post(base_url, topic, payload) {
            Ok(()) => return true,
            Err(err) if attempt + 1 == RETRIES => {
                tracing::warn!("cluster post to {base_url} failed for topic {topic}: {err}");
            }
            Err(_) => std::thread::sleep(RETRY_WAITS[usize::try_from(attempt).unwrap_or(0).min(RETRY_WAITS.len() - 1)]),
        }
    }
    false
}

fn invalidation_body(entity_name: &str, pk_fields: &[String], values: &crate::entity::FieldMap) -> serde_json::Value {
    let mut primary_key = serde_json::Map::new();
    for name in pk_fields {
        if let Some(value) = values.get(name.as_str()).filter(|value| !value.is_null()) {
            primary_key.insert(name.clone(), moqui_lib::json::to_json(value));
        }
    }
    serde_json::json!({
        "entityName": entity_name,
        "primaryKey": primary_key,
    })
}

struct LoadedMember {
    instance_id: String,
    base_url: String,
    heartbeat: Option<i64>,
    session_fp: String,
    cert_fp: String,
    ssh_fp: String,
}

fn load_members(facade: &EntityFacade) -> Result<Vec<LoadedMember>, Error> {
    let fields: Vec<FieldRef> = MEMBER_COLUMNS.iter().copied().map(FieldRef::new).collect();
    let sql = facade
        .with_entity(MEMBER, |entity, dialect, _| {
            sql_select(entity, dialect, &fields, None, &[], LimitOffset::default()).map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    let tx = facade.transactions_for(MEMBER).map_err(Error::from)?;
    let rows = with_reserve_tx(&tx, |conn| conn.query(&sql, &[])).map_err(Error::from)?;
    Ok(rows.iter().map(member_from_row).collect())
}

fn member_from_row(row: &DbRow) -> LoadedMember {
    LoadedMember {
        instance_id: row_text(row, "instanceId"),
        base_url: row_text(row, "clusterBaseUrl"),
        heartbeat: row.get("heartbeat").and_then(value_millis),
        session_fp: row_text(row, "sessionKeyFingerprint"),
        cert_fp: row_text(row, "certFingerprint"),
        ssh_fp: row_text(row, "sshHostKeyFingerprint"),
    }
}

/// The SSH host key fingerprint the member at `base_url` registered, for a sender that pins it.
///
/// # Errors
///
/// Returns an error when the member rows cannot be read.
pub fn peer_ssh_fingerprint(facade: &EntityFacade, base_url: &str) -> Result<Option<String>, Error> {
    let base = trim_base(base_url);
    Ok(load_members(facade)?
        .into_iter()
        .find(|member| trim_base(&member.base_url) == base)
        .map(|member| member.ssh_fp)
        .filter(|fingerprint| !fingerprint.is_empty()))
}

fn row_text(row: &DbRow, name: &str) -> String {
    match row.get(name) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => moqui_lib::object::to_plain_string(other),
    }
}

fn write_member(facade: &EntityFacade, member: &Member) -> Result<(), Error> {
    let _authz = entered_authz();
    let fields: Vec<FieldRef> = MEMBER_COLUMNS.iter().copied().map(FieldRef::new).collect();
    let by_id = Condition::equals("instanceId", Value::string(&member.instance_id));
    let (select_sql, insert_sql, update_sql) = facade
        .with_entity(MEMBER, |entity, dialect, _| {
            let select_sql = sql_select(entity, dialect, &fields, Some(&by_id), &[], LimitOffset::default())?.sql;
            let insert_sql = sql_insert(entity, dialect, MEMBER_COLUMNS)?;
            let update_sql = sql_update(
                entity,
                dialect,
                &[
                    "clusterBaseUrl",
                    "heartbeat",
                    "sessionKeyFingerprint",
                    "certFingerprint",
                    "sshHostKeyFingerprint",
                ],
                Some(&by_id),
            )?
            .sql;
            Ok((select_sql, insert_sql, update_sql))
        })
        .map_err(Error::from)?;
    let instance = Value::string(&member.instance_id);
    let insert_params = vec![
        instance.clone(),
        Value::string(&member.base_url),
        member.heartbeat.clone(),
        Value::string(&member.session_fp),
        Value::string(&member.cert_fp),
        Value::string(&member.ssh_fp),
    ];
    let update_params = vec![
        Value::string(&member.base_url),
        member.heartbeat.clone(),
        Value::string(&member.session_fp),
        Value::string(&member.cert_fp),
        Value::string(&member.ssh_fp),
        instance.clone(),
    ];
    let tx = facade.transactions_for(MEMBER).map_err(Error::from)?;
    with_reserve_tx(&tx, |conn| {
        let rows = conn.query(&select_sql, &[instance])?;
        if rows.is_empty() {
            conn.execute(&insert_sql, &insert_params)?;
        } else {
            conn.execute(&update_sql, &update_params)?;
        }
        Ok(())
    })
    .map_err(Error::from)
}

/// The database server's time in epoch milliseconds, else this host's.
fn db_now(facade: &EntityFacade) -> i64 {
    let read = facade
        .transactions_for(MEMBER)
        .map_err(Error::from)
        .and_then(|tx| with_reserve_tx(&tx, |conn| conn.query("SELECT CURRENT_TIMESTAMP", &[])).map_err(Error::from));
    read.ok()
        .and_then(|rows| {
            rows.first()
                .and_then(|row| row.columns.first().map(|(_, value)| value.clone()))
        })
        .and_then(|value| value_millis(&value))
        .unwrap_or_else(now_millis)
}

fn millis_value(millis: i64) -> Value {
    jiff::Timestamp::from_millisecond(millis).map_or_else(|_| now_value(), Value::Timestamp)
}

/// Remove the rows of members that stopped long ago, so the table does not keep every instance that ever ran.
fn delete_dead_members(facade: &EntityFacade, before: i64) -> Result<(), Error> {
    let _authz = entered_authz();
    let condition = Condition::less("heartbeat", millis_value(before));
    let statement = facade
        .with_entity(MEMBER, |entity, dialect, _| {
            sql_delete(entity, dialect, Some(&condition))
        })
        .map_err(Error::from)?;
    let tx = facade.transactions_for(MEMBER).map_err(Error::from)?;
    with_reserve_tx(&tx, |conn| {
        conn.execute(&statement.sql, &statement.params)?;
        Ok(())
    })
    .map_err(Error::from)
}

/// A live member whose base URL is a loopback address cannot be reached from another host: say so once, since the
/// fan-out to it fails without a sign otherwise.
fn warn_loopback_peers(state: &ClusterState) {
    let peers = lock(&state.peers).urls.clone();
    if peers.is_empty() || state.loopback_warned.load(Ordering::Relaxed) {
        return;
    }
    let own = cluster_base_url().unwrap_or_default();
    let loopback = |url: &str| url.contains("://127.") || url.contains("://localhost") || url.contains("://[::1]");
    if loopback(&own) || peers.iter().any(|url| loopback(url)) {
        state.loopback_warned.store(true, Ordering::Relaxed);
        tracing::error!(
            "a cluster member has a loopback cluster_base_url ({own}, peers {peers:?}); set cluster_base_url on each \
             member to an address the others can reach"
        );
    }
}

fn touch_member(facade: &EntityFacade, instance_id: &str, now: i64) -> Result<(), Error> {
    let _authz = entered_authz();
    let condition = Condition::equals("instanceId", Value::string(instance_id));
    let sql = facade
        .with_entity(MEMBER, |entity, dialect, _| {
            sql_update(entity, dialect, &["heartbeat"], Some(&condition)).map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    let params = [millis_value(now), Value::string(instance_id)];
    let tx = facade.transactions_for(MEMBER).map_err(Error::from)?;
    with_reserve_tx(&tx, |conn| {
        conn.execute(&sql, &params)?;
        Ok(())
    })
    .map_err(Error::from)
}

/// Keep a message too large for a NOTIFY payload, and answer the id the NOTIFY carries instead.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn store_message(facade: &EntityFacade, topic: &str, payload: &str) -> Result<String, Error> {
    let _authz = entered_authz();
    let id = crate::entity::uuid_v4().map_err(Error::from)?;
    let sql = facade
        .with_entity(MESSAGE, |entity, dialect, _| {
            sql_insert(entity, dialect, &["messageId", "topic", "payload", "createdDate"])
        })
        .map_err(Error::from)?;
    let params = [
        Value::string(&id),
        Value::string(topic),
        Value::string(payload),
        millis_value(db_now(facade)),
    ];
    let tx = facade.transactions_for(MESSAGE).map_err(Error::from)?;
    with_reserve_tx(&tx, |conn| {
        conn.execute(&sql, &params)?;
        Ok(())
    })
    .map_err(Error::from)?;
    Ok(id)
}

/// The topic and payload of a message [`store_message`] kept. `None` once it has been pruned.
///
/// # Errors
///
/// Returns an error when the row cannot be read.
pub fn load_message(facade: &EntityFacade, id: &str) -> Result<Option<(String, String)>, Error> {
    let _authz = entered_authz();
    let fields = [FieldRef::new("topic"), FieldRef::new("payload")];
    let by_id = Condition::equals("messageId", Value::string(id));
    let statement = facade
        .with_entity(MESSAGE, |entity, dialect, _| {
            sql_select(entity, dialect, &fields, Some(&by_id), &[], LimitOffset::default())
        })
        .map_err(Error::from)?;
    let tx = facade.transactions_for(MESSAGE).map_err(Error::from)?;
    let rows = with_reserve_tx(&tx, |conn| conn.query(&statement.sql, &statement.params)).map_err(Error::from)?;
    Ok(rows
        .first()
        .map(|row| (row_text(row, "topic"), row_text(row, "payload"))))
}

/// Delete kept messages older than the cluster's liveness window: every live member has read them by then.
///
/// # Errors
///
/// Returns an error when the rows cannot be deleted.
pub fn prune_messages(factory: &ExecutionContextFactory, facade: &EntityFacade) -> Result<(), Error> {
    let _authz = entered_authz();
    let before = db_now(facade).saturating_sub(window_millis(factory));
    let condition = Condition::less("createdDate", millis_value(before));
    let statement = facade
        .with_entity(MESSAGE, |entity, dialect, _| {
            sql_delete(entity, dialect, Some(&condition))
        })
        .map_err(Error::from)?;
    let tx = facade.transactions_for(MESSAGE).map_err(Error::from)?;
    with_reserve_tx(&tx, |conn| {
        conn.execute(&statement.sql, &statement.params)?;
        Ok(())
    })
    .map_err(Error::from)
}

fn entered_authz() -> Option<crate::context::AuthzGuard> {
    ExecutionContext::with_current(ExecutionContext::disable_authz)
}
