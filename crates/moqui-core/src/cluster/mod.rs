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
//! the live peers, as `plans/archive/Cluster.md` says. An invalidation joins the one already waiting for its entity, which
//! then carries every key written, up to [`KEYS_PER_ENTITY`], and the whole entity past it ([`Queue::push`],
//! plans/archive/High-Level-Review.md HL-017). A peer that may have missed an invalidation (a failed post, a skip while it was
//! down, a return to the live set) gets a clear of every entity cache before anything else ([`PeerCache::dirty`],
//! HL-018). Each peer gets what was waiting as one batch, in order
//! ([`Sender::post_batch`]: one gRPC call, or one POST after another), and several peers are posted to at once. The
//! peer list is kept and read again on each heartbeat.
//!
//! The clear of every entity cache is the recovery of a gap in any topic, so a peer is marked for it whatever it missed:
//! a failed or skipped batch of any topic, and every peer when a message of another topic is dropped from a full queue.
//! A member that receives it ([`recover_missed`]) clears its entity caches and retires the live sessions whose rows are
//! gone, which covers a lost `session-end`. The other topics need no more: a co-edited document's open rooms read the
//! state other servers stored at their next tick (moqui-collab `tick`), so a lost `collab-update` is merged and sent to
//! the room's clients within seconds; a notification whose topic persists it is stored before it is pushed, and a client
//! reads its list on load, while one not persisted is a live hint that a closed socket misses on one server too; an RTC
//! signal goes to one member ([`send_to`]) and a call that misses one is set up again by its client; a live setting is in
//! memory only and expires within 24 hours, and an instance that missed one keeps its on-disk value; a component
//! deploy's files are written from the shared repository at the next start of a member that missed it.
//!
//! Each factory keeps its own state (sender, member id, queue, peers): a write or notification fans out only to the
//! peers of the factory whose entity facade made it, so two factories in one process do not post for each other.

mod direct;
#[cfg(all(test, feature = "cluster-tls"))]
mod tests;

pub use direct::{for_this_member, send_to, DIRECT_LIMIT};

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
use moqui_lib::sync::{lock, wait_timeout};
use sha2::{Digest, Sha256};

use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::driver::DbRow;
use crate::entity::exec::with_reserve_tx;
use crate::entity::EntityFacade;
use crate::notify::{self, TopicMessage, ENTITY_CACHE_INVALIDATE};
use crate::user::{now_millis, now_value, value_millis};

const MEMBER: &str = "moqui.server.ClusterMember";
const MESSAGE: &str = "moqui.server.ClusterMessage";
/// The base URL every `cluster_transport=notify` member advertises. One post to it reaches all of them.
pub const NOTIFY_BASE_URL: &str = "pg-notify:";
/// Maximum serialized topic envelope, shared by every cluster transport.
pub const MAX_CLUSTER_BODY: usize = 16 * 1024 * 1024;
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
/// Messages of topics other than the entity cache invalidation waiting for the sender thread. Past this one is dropped
/// and counted. Invalidations are bounded apart, so a burst of other topics does not push them out.
const OUTBOX_LIMIT: usize = 10_000;
/// Entities with an invalidation waiting. Past this the waiting invalidations become one clear of every entity cache.
const INVALIDATION_ENTITIES: usize = 2_000;
/// Keys one waiting invalidation carries. Past this it invalidates the whole entity.
const KEYS_PER_ENTITY: usize = 100;
/// A peer whose post failed every try is skipped this long, so a peer that is down does not slow every message.
const PEER_BACKOFF: Duration = Duration::from_secs(5);
/// A member row with no heartbeat in this many liveness windows is deleted.
const DEAD_WINDOWS: i64 = 10;

/// How long a read that found no live peer is trusted. A member that joins is then seen at the next write after this,
/// rather than at the next heartbeat or liveness window: until then, writes here are not posted to it.
const EMPTY_PEERS_TRUSTED_MS: i64 = 250;

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
    /// The row [`start`] wrote, which the heartbeat writes again when peers deleted it.
    member: Mutex<Option<Member>>,
    /// The heartbeat's interval instead of the scheduler's, for tests.
    #[cfg(test)]
    heartbeat_every: Mutex<Option<Duration>>,
    /// The messages for one member each ([`send_to`]), and their thread.
    direct: Mutex<direct::DirectQueue>,
    direct_ready: Condvar,
}

/// The messages waiting for the sender thread, and whether that thread is running.
#[derive(Default)]
struct Queue {
    messages: VecDeque<(&'static str, TopicMessage)>,
    /// Where in `messages` each entity's invalidation is, so a later write to the entity joins it.
    invalidations: HashMap<String, usize>,
    /// Where in `messages` a clear of every entity cache is: it covers every invalidation queued after it.
    clear_all: Option<usize>,
    /// Messages of other topics in `messages`, against [`OUTBOX_LIMIT`].
    others: usize,
    /// A pass is wanted with nothing queued, to send the dirty peers their clear ([`request_resync`]).
    resync: bool,
    sending: bool,
}

/// The bounds of a [`Queue`]: [`OUTBOX_LIMIT`], [`INVALIDATION_ENTITIES`] and [`KEYS_PER_ENTITY`], smaller in tests.
#[derive(Clone, Copy)]
struct Limits {
    others: usize,
    entities: usize,
    keys: usize,
}

const LIMITS: Limits = Limits {
    others: OUTBOX_LIMIT,
    entities: INVALIDATION_ENTITIES,
    keys: KEYS_PER_ENTITY,
};

/// What [`Queue::push`] did with a message.
#[derive(Debug, PartialEq, Eq)]
enum Queued {
    /// A new message, or one joined to the invalidation of its entity or a waiting clear of every entity.
    Taken,
    /// Dropped: the queue holds [`Limits::others`] messages of other topics.
    Dropped,
}

impl Queue {
    /// Queue `message` (plans/archive/High-Level-Review.md HL-017). An entity cache invalidation joins the one waiting for its
    /// entity: that one then carries every key, in `primaryKeys`, up to `limits.keys`, and no key (the whole entity) past
    /// it or when a write had no usable key. A clear of every entity cache waiting covers it. With `limits.entities`
    /// entities waiting, the invalidations become one clear of every entity cache. Other topics are kept in order, up to
    /// `limits.others`.
    fn push(&mut self, topic: &'static str, message: TopicMessage, limits: Limits) -> Queued {
        if topic != ENTITY_CACHE_INVALIDATE {
            if self.others >= limits.others {
                return Queued::Dropped;
            }
            self.others += 1;
            self.messages.push_back((topic, message));
            return Queued::Taken;
        }
        if self.clear_all.is_some() {
            return Queued::Taken;
        }
        if is_clear_all(&message.body) {
            self.collapse(message);
            return Queued::Taken;
        }
        let name = entity_of(&message.body).to_string();
        if let Some(&at) = self.invalidations.get(&name) {
            if let Some((_, waiting)) = self.messages.get_mut(at) {
                join_keys(&mut waiting.body, &message.body, limits.keys);
            }
            return Queued::Taken;
        }
        if self.invalidations.len() >= limits.entities {
            let all = TopicMessage {
                body: serde_json::json!({ "clearAll": true }),
                ..message
            };
            self.collapse(all);
            return Queued::Taken;
        }
        let mut message = message;
        whole_without_key(&mut message.body);
        self.invalidations.insert(name, self.messages.len());
        self.messages.push_back((topic, message));
        Queued::Taken
    }

    /// Put one clear of every entity cache, `all`, in the place of the waiting invalidations.
    fn collapse(&mut self, all: TopicMessage) {
        self.messages.retain(|(topic, _)| *topic != ENTITY_CACHE_INVALIDATE);
        self.invalidations.clear();
        self.clear_all = Some(self.messages.len());
        self.messages.push_back((ENTITY_CACHE_INVALIDATE, all));
    }

    /// Everything waiting, in order, leaving the queue empty.
    fn take(&mut self) -> Vec<(&'static str, TopicMessage)> {
        self.invalidations.clear();
        self.clear_all = None;
        self.others = 0;
        self.resync = false;
        self.messages.drain(..).collect()
    }
}

/// The entity an invalidation body names; empty for none.
fn entity_of(body: &serde_json::Value) -> &str {
    body.get("entityName").and_then(serde_json::Value::as_str).unwrap_or("")
}

/// A body that clears every entity cache.
fn is_clear_all(body: &serde_json::Value) -> bool {
    body.get("clearAll").and_then(serde_json::Value::as_bool) == Some(true)
}

/// A body whose `primaryKey` is empty invalidates the whole entity: drop the key, so a later write does not narrow it.
fn whole_without_key(body: &mut serde_json::Value) {
    let Some(map) = body.as_object_mut() else {
        return;
    };
    let empty = map
        .get("primaryKey")
        .is_some_and(|key| key.as_object().is_none_or(serde_json::Map::is_empty));
    if empty {
        map.remove("primaryKey");
    }
}

/// Add the keys of `added` (its `primaryKey`, or each of its `primaryKeys`) to the waiting invalidation `waiting`: it
/// then lists its keys in `primaryKeys`. With no usable key, or past `cap` keys, it carries none and invalidates the
/// whole entity; a whole one stays whole.
fn join_keys(waiting: &mut serde_json::Value, added: &serde_json::Value, cap: usize) {
    let Some(map) = waiting.as_object_mut() else {
        return;
    };
    let Some(mut keys) = (match (map.remove("primaryKeys"), map.remove("primaryKey")) {
        (Some(serde_json::Value::Array(keys)), _) => Some(keys),
        (None, Some(key)) => Some(vec![key]),
        _ => None,
    }) else {
        return;
    };
    let added_keys: Vec<&serde_json::Value> = match added.get("primaryKeys").and_then(serde_json::Value::as_array) {
        Some(list) => list.iter().collect(),
        None => added.get("primaryKey").into_iter().collect(),
    };
    let usable = |key: &&serde_json::Value| key.as_object().is_some_and(|key| !key.is_empty());
    if added_keys.is_empty() || !added_keys.iter().all(usable) {
        return;
    }
    for key in added_keys {
        if keys.len() > cap {
            break;
        }
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    if keys.len() <= cap {
        map.insert("primaryKeys".to_string(), serde_json::Value::Array(keys));
    }
}

#[derive(Default)]
struct PeerCache {
    urls: Vec<String>,
    /// When the list was read, by the clock of `now_millis`; zero is never.
    read_at: i64,
    /// Peers to skip until the time given, after every try of a post failed.
    down_until: HashMap<String, Instant>,
    /// Peers that may have missed a message (plans/archive/High-Level-Review.md HL-018): a post that failed every try, a batch
    /// skipped while the peer was down, a message dropped from a full queue, or a return to the live set. Each gets a
    /// clear of every entity
    /// cache ahead of its next batch, and stays here until one is delivered; the heartbeat asks for that pass, so no
    /// write is needed. The value is the gap's number, so a gap found while a clear was on its way is not forgotten.
    dirty: HashMap<String, u64>,
    gaps: u64,
}

impl PeerCache {
    fn mark_dirty(&mut self, base_url: &str) {
        self.gaps += 1;
        self.dirty.insert(base_url.to_string(), self.gaps);
    }
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
            member: Mutex::new(None),
            #[cfg(test)]
            heartbeat_every: Mutex::new(None),
            direct: Mutex::new(direct::DirectQueue::default()),
            direct_ready: Condvar::new(),
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

/// A delivered message or a permanent refusal that does not imply the peer is down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostOutcome {
    Accepted,
    Rejected(u16),
}

impl PostOutcome {
    /// Classify a transport's response without treating a refused message as a failed peer.
    pub fn from_status(status: u16) -> Result<Self, Error> {
        match status {
            200..=299 => Ok(Self::Accepted),
            400..=499 => Ok(Self::Rejected(status)),
            _ => Err(Error::new(format!("cluster peer returned {status}"))),
        }
    }
}

/// What a peer did with a batch from [`Sender::post_batch`]: the outcome of each of the first `outcomes.len()` messages,
/// in order, and, when not every message has one, why the rest were not applied (a transport failure or a busy peer),
/// so they are sent again, still in order.
#[derive(Debug)]
pub struct BatchOutcome {
    pub outcomes: Vec<PostOutcome>,
    pub failed: Option<Error>,
}

/// Posts topic messages to a peer. Errors represent retryable transport or server failures.
pub trait Sender: Send + Sync {
    /// `base_url` has no trailing slash. `topic` is the topic name, not a path.
    fn post(&self, base_url: &str, topic: &str, payload: &[u8]) -> Result<PostOutcome, Error>;

    /// Post `messages` (topic, payload) to one peer, in order, stopping at the first that fails retryably: those after it
    /// are not applied. This posts them one at a time; a transport that carries several in one exchange (gRPC) does so.
    fn post_batch(&self, base_url: &str, messages: &[(&str, &[u8])]) -> BatchOutcome {
        let mut outcomes = Vec::with_capacity(messages.len());
        for (topic, payload) in messages {
            match self.post(base_url, topic, payload) {
                Ok(outcome) => outcomes.push(outcome),
                Err(err) => {
                    return BatchOutcome {
                        outcomes,
                        failed: Some(err),
                    }
                }
            }
        }
        BatchOutcome { outcomes, failed: None }
    }
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
/// member row and does not start another heartbeat thread.
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
    let member = Member {
        instance_id: instance_id.clone(),
        base_url,
        heartbeat: millis_value(db_now(&facade)),
        session_fp: fingerprints.session,
        cert_fp: fingerprints.cert,
        ssh_fp: env::get_prop_or_env(SSH_FINGERPRINT_PROPERTY),
    };
    write_member(&facade, &member)?;
    let state = ClusterState::of(factory);
    *lock(&state.member) = Some(member);
    *lock(&state.factory) = Arc::downgrade(factory);
    lock(&state.instance_id).clone_from(&instance_id);
    // The member list is read again on the next send: this start may have joined a cluster or left one. A peer that
    // missed an invalidation still gets its clear.
    {
        let mut peers = lock(&state.peers);
        let dirty = std::mem::take(&mut peers.dirty);
        let gaps = peers.gaps;
        *peers = PeerCache {
            dirty,
            gaps,
            ..PeerCache::default()
        };
    }
    facade.with_slot(ClusterLink::default, |link| link.0 = Arc::downgrade(&state));
    install_heartbeat(factory);
    Ok(())
}

/// Wait until `factory`'s sender thread has posted everything queued, for tests.
#[cfg(all(test, feature = "cluster-tls"))]
pub(crate) fn wait_for_sends(factory: &ExecutionContextFactory, timeout: Duration) -> bool {
    let state = ClusterState::of(factory);
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        let empty = {
            let queue = lock(&state.queue);
            queue.messages.is_empty() && !queue.resync
        };
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
        apply_invalidation(facade, &message.body)?;
    } else if !notify::TOPICS.contains(&topic) {
        return Err(Error::new(format!("unknown topic {topic}")));
    }
    notify::publish(topic, message)
}

/// An `entity-cache-invalidate` body: `clearAll` clears every entity cache (a sender that may have lost invalidations,
/// plans/archive/High-Level-Review.md HL-018); `primaryKeys` evicts by each key (several writes joined, HL-017), `primaryKey` by
/// one; with no usable key the entity's caches go whole. A key that does not apply is logged and the others still are.
fn apply_invalidation(facade: &EntityFacade, body: &serde_json::Value) -> Result<(), Error> {
    if is_clear_all(body) {
        let factory = linked_state(facade)
            .and_then(|state| lock(&state.factory).upgrade())
            .or_else(|| ExecutionContext::with_current(|ec| Arc::clone(ec.factory())));
        return recover_missed(factory.as_deref(), facade);
    }
    let name = Some(entity_of(body))
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::new("entity-cache-invalidate body has no entityName"))?;
    let key_of = |key: &serde_json::Value| -> Option<crate::entity::FieldMap> {
        key.as_object().filter(|keys| !keys.is_empty()).map(|keys| {
            keys.iter()
                .map(|(name, value)| (std::sync::Arc::from(name.as_str()), moqui_lib::json::from_json(value)))
                .collect()
        })
    };
    let keys: Vec<Option<crate::entity::FieldMap>> = match body.get("primaryKeys").and_then(serde_json::Value::as_array)
    {
        Some(keys) if !keys.is_empty() => keys.iter().map(key_of).collect(),
        // With the written row's key, only the cached finds it may have changed go.
        _ => vec![body.get("primaryKey").and_then(key_of)],
    };
    // Several keys go in one sweep of the entity's caches (plans/archive/High-Level-Review.md HL-002); a key that is not
    // usable clears the entity whole, which covers the others.
    let cleared = match keys.into_iter().collect::<Option<Vec<_>>>() {
        Some(keys) if keys.len() > 1 => crate::entity::cache::clear_keys_cache(facade, name, &keys),
        Some(keys) => crate::entity::cache::clear_one_cache(facade, name, keys.first()),
        None => crate::entity::cache::clear_one_cache(facade, name, None),
    };
    cleared.map_err(|err| {
        tracing::warn!("cluster invalidation of {name} failed: {err}");
        Error::from(err)
    })
}

/// What a member does when it may have missed cluster messages: a peer's clear of every entity cache (a gap in its
/// sends) or a transport that lost its connection. Every entity list cache is cleared, and the live sessions whose rows
/// are gone are retired, so a `session-end` lost in the gap does not leave an ended session served here
/// (plans/archive/High-Level-Review.md HL-018). The other topics need nothing: see the module documentation. The session step
/// runs only with `factory`, and its failure is logged.
///
/// # Errors
///
/// When the entity caches cannot be cleared.
pub fn recover_missed(factory: Option<&ExecutionContextFactory>, facade: &EntityFacade) -> Result<(), Error> {
    let cleared = crate::entity::cache::clear_all_list_caches(facade).map_err(Error::from);
    if let Some(factory) = factory {
        match crate::user::session::retire_gone(factory) {
            Ok(0) => {}
            Ok(retired) => tracing::info!("cluster recovery: {retired} sessions ended on another server were retired"),
            Err(err) => tracing::error!("cluster recovery: the live sessions could not be checked: {err}"),
        }
    }
    cleared
}

/// Publish `message` on this process's bus, then queue it for the peers of the factory that started `facade`. A
/// message with no `message_id` is given one, as a peer refuses a post without it.
///
/// # Errors
///
/// Returns an error when `topic` is not a known topic, or when no id can be made for a message without one.
pub fn publish(facade: &EntityFacade, topic: &'static str, message: &TopicMessage) -> Result<(), Error> {
    let identified;
    let message = if message.message_id.is_empty() {
        identified = TopicMessage {
            message_id: crate::entity::uuid_v4().map_err(Error::from)?,
            ..message.clone()
        };
        &identified
    } else {
        message
    };
    notify::publish(topic, message)?;
    if let Some(state) = linked_state(facade) {
        fan_out(&state, topic, message);
    }
    Ok(())
}

/// Whether `facade`'s factory posts to cluster peers, whose sockets may have the users of a notification.
/// The members of this factory's cluster whose heartbeat is within the window, this one among them.
///
/// # Errors
///
/// When the member rows cannot be read.
pub fn live_member_count(factory: &ExecutionContextFactory, facade: &EntityFacade) -> Result<usize, Error> {
    let now = db_now(facade);
    let window = window_millis(factory);
    Ok(load_members(facade)?
        .iter()
        .filter(|member| is_live(member.heartbeat, now, window))
        .count())
}

/// Whether `instance_id` is a live member of this factory's cluster (its heartbeat within the window): an edge node
/// that names a member's id is refused (plans/Edge-Node.md §5.4).
///
/// # Errors
///
/// When the member rows cannot be read.
pub fn is_live_member(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    instance_id: &str,
) -> Result<bool, Error> {
    let now = db_now(facade);
    let window = window_millis(factory);
    Ok(load_members(facade)?
        .iter()
        .any(|member| member.instance_id == instance_id && is_live(member.heartbeat, now, window)))
}

pub fn is_linked(facade: &EntityFacade) -> bool {
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
    note_invalidation(facade, entity_name, || {
        invalidation_body(entity_name, pk_fields, values)
    })
}

/// [`note_write`] for the rows one statement wrote, by their keys (a bulk delete's chunk, plans/archive/High-Level-Review.md
/// HL-033): one invalidation that lists every key in `primaryKeys`, or past [`KEYS_PER_ENTITY`] keys none, which
/// invalidates the whole entity, as a waiting invalidation joined past that many does.
pub(crate) fn note_writes(
    facade: &EntityFacade,
    entity_name: &str,
    pk_fields: &[String],
    keys: &[Arc<crate::entity::FieldMap>],
) -> Result<(), Error> {
    if keys.is_empty() {
        return Ok(());
    }
    note_invalidation(facade, entity_name, || {
        keys_body(entity_name, pk_fields, keys, LIMITS.keys)
    })
}

/// The invalidation body of the rows `keys`: each key in `primaryKeys`, or none (the whole entity) past `cap` keys or
/// for a key with no usable field.
fn keys_body(
    entity_name: &str,
    pk_fields: &[String],
    keys: &[Arc<crate::entity::FieldMap>],
    cap: usize,
) -> serde_json::Value {
    let whole = || serde_json::json!({ "entityName": entity_name });
    let mut listed = Vec::with_capacity(keys.len().min(cap));
    for key in keys {
        let mut body = invalidation_body(entity_name, pk_fields, key);
        let Some(key) = body
            .get_mut("primaryKey")
            .map(serde_json::Value::take)
            .filter(|key| key.as_object().is_some_and(|key| !key.is_empty()))
        else {
            return whole();
        };
        if !listed.contains(&key) {
            if listed.len() >= cap {
                return whole();
            }
            listed.push(key);
        }
    }
    serde_json::json!({ "entityName": entity_name, "primaryKeys": listed })
}

/// Publish the invalidation `body` gives, after commit when a transaction is open.
fn note_invalidation(
    facade: &EntityFacade,
    entity_name: &str,
    body: impl FnOnce() -> serde_json::Value,
) -> Result<(), Error> {
    // No member caches what this entity's writes change (`cache="never"` and in no cacheable view): the peers are not
    // told (plans/archive/High-Level-Review.md HL-017), though a local listener still is.
    let to_peers = is_linked(facade)
        && (crate::user::is_authz_entity(entity_name) || crate::entity::cache::peers_may_cache(facade, entity_name));
    // No peer and no local listener: there is no one to tell, so the message (a UUID and a JSON body) is not built.
    if !to_peers && !notify::has_listeners(ENTITY_CACHE_INVALIDATE) {
        return Ok(());
    }
    let message = TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4().map_err(Error::from)?,
        body: body(),
    };
    let tx = facade.transactions_for(entity_name).map_err(Error::from)?;
    if tx.in_transaction() {
        let facade = facade.clone();
        tx.on_commit(move || {
            if let Err(err) = publish_invalidation(&facade, &message, to_peers) {
                tracing::warn!("cluster cache publish failed: {err}");
            }
        })
        .map_err(Error::from)?;
        return Ok(());
    }
    publish_invalidation(facade, &message, to_peers)
}

fn publish_invalidation(facade: &EntityFacade, message: &TopicMessage, to_peers: bool) -> Result<(), Error> {
    if to_peers {
        publish(facade, ENTITY_CACHE_INVALIDATE, message)
    } else {
        notify::publish(ENTITY_CACHE_INVALIDATE, message)
    }
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

#[derive(Clone)]
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

/// Start the heartbeat thread: [`heartbeat`] every scheduler interval. It runs on a thread of its own, not as a
/// scheduler hook, so a long job pass or maintenance hook does not hold it past the liveness window and drop this member
/// out of its peers' fan-out (`plans/archive/High-Level-Review.md` HL-020). Pools with no threads (inline, idle) run no
/// scheduler, so they get no heartbeat, as before.
fn install_heartbeat(factory: &Arc<ExecutionContextFactory>) {
    if !factory.service_pools().is_threaded() || factory.cluster_heartbeat.swap(true, Ordering::AcqRel) {
        return;
    }
    let weak = Arc::downgrade(factory);
    let spawned = factory.spawn_background("moqui-cluster-heartbeat", move || loop {
        let every = match weak.upgrade() {
            Some(factory) => heartbeat_interval(&factory),
            None => return,
        };
        let Some(factory) = ExecutionContextFactory::sleep_while_running(&weak, every) else {
            return;
        };
        let context = factory.new_context();
        let _entered = context.enter();
        heartbeat(&factory);
    });
    if let Err(err) = spawned {
        factory.cluster_heartbeat.store(false, Ordering::Release);
        tracing::error!("the cluster heartbeat did not start: {err}");
    }
}

fn heartbeat_interval(factory: &ExecutionContextFactory) -> Duration {
    #[cfg(test)]
    if let Some(every) = *lock(&ClusterState::of(factory).heartbeat_every) {
        return every;
    }
    factory.service_pools().check_interval()
}

/// One heartbeat: move this member's heartbeat, or write its row again when peers deleted it as dead; delete members
/// dead for [`DEAD_WINDOWS`] windows; and read the live peers again. A step that fails is logged and the next still runs.
/// The heartbeat thread runs it every scheduler interval; a test can run it at once.
pub fn heartbeat(factory: &ExecutionContextFactory) {
    let Some(facade) = factory.entity() else {
        tracing::warn!("cluster heartbeat has no entity facade");
        return;
    };
    let state = ClusterState::of(factory);
    let instance_id = state.instance_id();
    if instance_id.is_empty() {
        return;
    }
    // The database's clock, so members whose clocks differ still see each other as live.
    let now = db_now(&facade);
    match touch_member(&facade, &instance_id, now) {
        Ok(0) => rejoin(&state, &facade, now),
        Ok(_) => {}
        Err(err) => tracing::warn!("cluster heartbeat failed: {err}"),
    }
    if let Err(err) = delete_dead_members(&facade, now.saturating_sub(window_millis(factory) * DEAD_WINDOWS)) {
        tracing::warn!("cluster member cleanup failed: {err}");
    }
    refresh_peers(&state, factory);
    warn_loopback_peers(&state);
    request_resync(&state);
}

/// Start a send pass for the peers that may have missed an invalidation, with nothing else queued, so they get their
/// clear without waiting for another write (plans/archive/High-Level-Review.md HL-018).
fn request_resync(state: &Arc<ClusterState>) {
    if lock(&state.peers).dirty.is_empty() {
        return;
    }
    let mut queue = lock(&state.queue);
    queue.resync = true;
    wake_sender(state, &mut queue);
}

/// The heartbeat found no row: peers deleted it after this member's heartbeat stopped for [`DEAD_WINDOWS`] windows.
/// Write it again, so the member is back in their fan-out without a restart.
fn rejoin(state: &ClusterState, facade: &EntityFacade, now: i64) {
    let Some(mut member) = lock(&state.member).clone() else {
        return;
    };
    member.heartbeat = millis_value(now);
    match write_member(facade, &member) {
        Ok(()) => tracing::warn!(
            "cluster member {} had no row (peers took it for dead); registered it again",
            member.instance_id
        ),
        Err(err) => tracing::error!("cluster member {} could not register again: {err}", member.instance_id),
    }
}

/// Run `factory`'s heartbeat every `every` instead of the scheduler interval. Call before [`start`].
#[cfg(all(test, feature = "cluster-tls"))]
pub(crate) fn set_heartbeat_interval(factory: &ExecutionContextFactory, every: Duration) {
    *lock(&ClusterState::of(factory).heartbeat_every) = Some(every);
}

fn fan_out(state: &Arc<ClusterState>, topic: &'static str, message: &TopicMessage) {
    {
        // A single member has no one to tell: with no peers at a recent read, nothing is queued. An older read is not
        // trusted, so a member that joined since is found by the send (and first sent a clear, as a new peer).
        let peers = lock(&state.peers);
        if peers.read_at != 0
            && peers.urls.is_empty()
            && now_millis().saturating_sub(peers.read_at) < EMPTY_PEERS_TRUSTED_MS
        {
            return;
        }
    }
    let mut queue = lock(&state.queue);
    if queue.push(topic, message.clone(), LIMITS) == Queued::Dropped {
        drop(queue);
        let dropped = state.dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped.is_power_of_two() {
            tracing::warn!("cluster fan-out queue is full; {dropped} messages dropped");
        }
        // Every peer misses it: each gets the recovery of a gap ahead of its next batch (HL-018).
        let mut peers = lock(&state.peers);
        for base_url in peers.urls.clone() {
            peers.mark_dirty(&base_url);
        }
        return;
    }
    wake_sender(state, &mut queue);
}

/// Wake the sender thread for what `queue` holds, or start it.
fn wake_sender(state: &Arc<ClusterState>, queue: &mut Queue) {
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
            while queue.messages.is_empty() && !queue.resync {
                let (next, waited) = wait_timeout(&state.ready, queue, SENDER_IDLE);
                queue = next;
                if waited.timed_out() && queue.messages.is_empty() && !queue.resync {
                    queue.sending = false;
                    return;
                }
            }
            state.in_flight.fetch_add(1, Ordering::SeqCst);
            queue.take()
        };
        // A panic in one batch must not end the thread that every later message waits on.
        let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send_batch(state, &batch)));
        if sent.is_err() {
            tracing::error!("cluster fan-out batch panicked");
        }
        state.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

fn send_batch(state: &ClusterState, batch: &[(&'static str, TopicMessage)]) {
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
    let payloads: Vec<(&str, Vec<u8>)> = batch
        .iter()
        .filter_map(|(topic, message)| match payload_bytes(message) {
            Ok(bytes) => Some((*topic, bytes)),
            Err(err) => {
                tracing::warn!(topic, error = %err, "cluster fan-out message was skipped");
                None
            }
        })
        .collect();
    let mut live: Vec<&str> = Vec::new();
    for base_url in &peers {
        if !peer_down(state, base_url) {
            live.push(base_url);
        } else if !payloads.is_empty() {
            // Skipped while down: it misses these messages.
            lock(&state.peers).mark_dirty(base_url);
        }
    }
    let any_dirty = !lock(&state.peers).dirty.is_empty();
    let clear_all = if any_dirty {
        match clear_all_payload() {
            Ok(payload) => Some(payload),
            Err(err) => {
                tracing::warn!("cluster clear of every entity cache could not be made: {err}");
                None
            }
        }
    } else {
        None
    };
    let send = |base_url: &str| {
        // A peer that may have missed an invalidation gets a clear of every entity cache first.
        let gap = lock(&state.peers).dirty.get(base_url).copied();
        let messages: Vec<(&str, Vec<u8>)> = match (gap, &clear_all) {
            (Some(_), Some(all)) => std::iter::once((ENTITY_CACHE_INVALIDATE, all.clone()))
                .chain(payloads.iter().cloned())
                .collect(),
            _ => payloads.clone(),
        };
        if messages.is_empty() {
            return;
        }
        if post_batch_with_retry(&sender, base_url, &messages) {
            let mut peers = lock(&state.peers);
            if gap.is_some() && clear_all.is_some() && peers.dirty.get(base_url) == gap.as_ref() {
                peers.dirty.remove(base_url);
            }
        } else {
            // Whatever the batch held, the peer may have missed some of it.
            mark_peer_down(state, base_url);
            lock(&state.peers).mark_dirty(base_url);
        }
    };
    // Each peer gets the whole batch in order; with more than one they are posted to at once, so a slow or unreachable
    // peer does not hold up the others.
    match live.as_slice() {
        [] => {}
        [only] => send(only),
        several => std::thread::scope(|scope| {
            for base_url in several {
                let spawned = std::thread::Builder::new()
                    .name("moqui-cluster-peer".to_string())
                    .spawn_scoped(scope, || send(base_url));
                if let Err(err) = spawned {
                    tracing::warn!("cluster peer send thread did not start, so {base_url} is posted to inline: {err}");
                    send(base_url);
                }
            }
        }),
    }
}

/// The payload of a clear of every entity cache, for a peer that may have missed an invalidation.
fn clear_all_payload() -> Result<Vec<u8>, Error> {
    payload_bytes(&TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4().map_err(Error::from)?,
        body: serde_json::json!({ "clearAll": true }),
    })
}

/// The live peers, read again when the heartbeat has not refreshed them within a liveness window, or when the last
/// read found none and is older than [`EMPTY_PEERS_TRUSTED_MS`].
fn current_peers(state: &ClusterState, factory: &ExecutionContextFactory) -> Vec<String> {
    let window = window_millis(factory);
    {
        let peers = lock(&state.peers);
        let age = now_millis().saturating_sub(peers.read_at);
        let trusted = if peers.urls.is_empty() {
            EMPTY_PEERS_TRUSTED_MS
        } else {
            window
        };
        if peers.read_at != 0 && age < trusted {
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
            // A peer not in the last list was out of this member's fan-out (its heartbeat fell behind, or its row was
            // deleted and written again) and may have missed invalidations; a new member's clear costs nothing.
            if peers.read_at != 0 {
                let returned: Vec<String> = urls.iter().filter(|url| !peers.urls.contains(url)).cloned().collect();
                for url in returned {
                    peers.mark_dirty(&url);
                }
            }
            // A peer that left is marked again if it returns, so its marker need not be kept meanwhile.
            peers.dirty.retain(|url, _| urls.contains(url));
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

/// Assign a message identity and validate its complete envelope before applying a collaborative edit.
pub fn prepare_message(body: serde_json::Value) -> Result<TopicMessage, Error> {
    let message = TopicMessage {
        origin_id: notify::origin_id().to_string(),
        message_id: crate::entity::uuid_v4().map_err(Error::from)?,
        body,
    };
    payload_bytes(&message)?;
    Ok(message)
}

fn payload_bytes(message: &TopicMessage) -> Result<Vec<u8>, Error> {
    let value = serde_json::json!({
        "origin_id": message.origin_id,
        "message_id": message.message_id,
        "body": message.body,
    });
    let bytes =
        serde_json::to_vec(&value).map_err(|err| Error::new(format!("could not encode cluster message: {err}")))?;
    if bytes.len() > MAX_CLUSTER_BODY {
        return Err(Error::new(format!(
            "cluster message is {} bytes; maximum is {MAX_CLUSTER_BODY}",
            bytes.len()
        )));
    }
    Ok(bytes)
}

/// Post `messages` to one peer in order, sending again, after a short and then a longer wait, the ones a transport
/// failure or a busy peer left unapplied. Each message gets [`RETRIES`] tries: the count starts again when a try gets
/// further. A refused message (4xx) is logged and not sent again. False when the peer took nothing on the last try.
fn post_batch_with_retry(sender: &Arc<dyn Sender>, base_url: &str, messages: &[(&str, Vec<u8>)]) -> bool {
    let mut done = 0;
    let mut tries = 0;
    loop {
        let pending: Vec<(&str, &[u8])> = messages[done..]
            .iter()
            .map(|(topic, payload)| (*topic, payload.as_slice()))
            .collect();
        let BatchOutcome { outcomes, failed } = sender.post_batch(base_url, &pending);
        let taken = outcomes.len().min(pending.len());
        for ((topic, _), outcome) in pending.iter().zip(&outcomes) {
            if let PostOutcome::Rejected(status) = outcome {
                tracing::warn!(
                    base_url,
                    topic,
                    status,
                    "cluster message rejected; peer remains available"
                );
            }
        }
        done += taken;
        if done >= messages.len() {
            return true;
        }
        tries = if taken > 0 { 1 } else { tries + 1 };
        if tries >= RETRIES {
            let err = failed.unwrap_or_else(|| Error::new("the peer answered for fewer messages than it was sent"));
            let topic = messages[done].0;
            let left = messages.len() - done;
            tracing::warn!("cluster post to {base_url} failed for topic {topic} ({left} messages left): {err}");
            return false;
        }
        let wait = usize::try_from(tries - 1).unwrap_or(0).min(RETRY_WAITS.len() - 1);
        std::thread::sleep(RETRY_WAITS[wait]);
    }
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
pub(crate) fn db_now(facade: &EntityFacade) -> i64 {
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

/// Move this member's heartbeat. Out: the rows changed, zero when the row is gone.
fn touch_member(facade: &EntityFacade, instance_id: &str, now: i64) -> Result<u64, Error> {
    let _authz = entered_authz();
    let condition = Condition::equals("instanceId", Value::string(instance_id));
    let sql = facade
        .with_entity(MEMBER, |entity, dialect, _| {
            sql_update(entity, dialect, &["heartbeat"], Some(&condition)).map(|statement| statement.sql)
        })
        .map_err(Error::from)?;
    let params = [millis_value(now), Value::string(instance_id)];
    let tx = facade.transactions_for(MEMBER).map_err(Error::from)?;
    with_reserve_tx(&tx, |conn| conn.execute(&sql, &params)).map_err(Error::from)
}

/// Keep a message too large for a NOTIFY payload, and answer the id the NOTIFY carries instead.
///
/// # Errors
///
/// Returns an error when the row cannot be written.
pub fn store_message(facade: &EntityFacade, topic: &str, payload: &str) -> Result<String, Error> {
    if payload.len() > MAX_CLUSTER_BODY {
        return Err(Error::new("cluster message exceeds the body limit"));
    }
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
