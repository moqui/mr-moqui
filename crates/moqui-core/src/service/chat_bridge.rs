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

//! Chat actors and the bridge layer's Rust half (plans/Comms-Framework.md §4.1, §4.3, §4.4; C1). The entities are
//! `framework/entity/ChatEntities.xml`; the inbound side (`ingest#Event`) and the screens' reads are
//! `org.moqui.impl.ChatBridgeServices`.
//!
//! - `get#LocalActors`: the `ChatActor` of each local user, made on first use. A local actor's id is its userId (an
//!   external actor's id is sequenced with the prefix `XA`), so two callers that make one user's actor at once both
//!   write the same row, and a reader by user needs no lookup.
//! - The adapter contract: a [`ChatBridgeAdapter`] serves a `ChatBridge` row whose `adapterName` (or its network's code)
//!   is the adapter's [`name`](ChatBridgeAdapter::name). A framework crate lists its adapters in
//!   [`crate::extension::Extension::chat_bridge_adapters`]; [`register_adapter`] adds one to a factory at run time (the
//!   tests' in-memory adapter). It answers its [`Capabilities`], starts and stops with its bridge, and delivers one event
//!   at a time. Protocol I/O is the adapter's, above this crate; `deliver` is called on a worker thread and may block.
//! - Outbound delivery. `append#Event` writes a `ChatBridgeDelivery` row for each mapping the event goes to, in the
//!   event's transaction, and `wake#BridgeDelivery` wakes this node's worker once that transaction commits. The worker
//!   ([`start`]: a thread per node, woken, and once a minute while any bridge exists, and once at start, so rows a
//!   crash left are picked up) does a [`round`]: each mapping's oldest pending row, claimed with a conditional update
//!   that takes a lease ([`LEASE_MILLIS`]), so one node sends it; rows of other mappings go on when one fails. Every
//!   attempt carries the row's stable idempotency key. A row whose last attempt was lost (its lease ran out with no
//!   outcome: a crash) is sent again to an adapter that dedupes by that key, and ends `CdsUncertain` for one that
//!   cannot. A failure waits [`BACKOFF`], up to [`MAX_ATTEMPTS`]; a permanent one fails at once. A failed or uncertain
//!   row stays visible with an authorized retry (`ChatBridgeServices.retry#Delivery`). Sends to a bridge are capped
//!   per minute on each node (the bridge's `sendsPerMinute`, else the adapter's). An adapter that panics is taken as a
//!   crash: the row keeps its lease and is tried again when it runs out.
//! - `get#BridgeCapabilities` (what each bridge's adapter supports, so an edit, delete or reaction it cannot carry is
//!   refused before the change), `check#BridgeToken` (an adapter's inbound token against the bridge's hash) and
//!   `run#BridgeDeliveries` (one round now, at a given time: the tests' simulated clock).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock, Weak};
use std::time::Duration;

use indexmap::IndexMap;
use moqui_db::sql::Condition;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use moqui_lib::sync::{lock, read, write};
use sha2::Digest;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, get_local_actors)?;
    rust_service!(map, wake_bridge_delivery)?;
    rust_service!(map, run_bridge_deliveries)?;
    rust_service!(map, get_bridge_capabilities)?;
    rust_service!(map, check_bridge_token)?;
    web::register(map)?;
    Ok(())
}

const ACTOR: &str = "moqui.chat.ChatActor";
const BRIDGE: &str = "moqui.chat.ChatBridge";
const MAPPING: &str = "moqui.chat.ChatRoomBridge";
const DELIVERY: &str = "moqui.chat.ChatBridgeDelivery";
const EVENT: &str = "moqui.chat.ChatEvent";
const EXTERNAL_EVENT: &str = "moqui.chat.ChatEventExternal";

const PENDING: &str = "CdsPending";
const ACCEPTED: &str = "CdsAccepted";
const FAILED: &str = "CdsFailed";
const UNCERTAIN: &str = "CdsUncertain";

/// How long a worker holds a row it is sending.
pub const LEASE_MILLIS: i64 = 60_000;
/// The waits after each failed attempt; the last repeats.
pub const BACKOFF: [i64; 6] = [5_000, 30_000, 120_000, 600_000, 1_800_000, 3_600_000];
/// Attempts of a row in all, before it fails.
pub const MAX_ATTEMPTS: i64 = 8;
/// The rows of one mapping a round sends at most, so one busy mapping does not hold the others up.
const PER_MAPPING: usize = 20;
/// How often the worker looks without being woken, while bridges exist.
const SWEEP: Duration = Duration::from_secs(60);

// ---- the adapter contract ----

/// A bridge as its adapter sees it: the `ChatBridge` row, with `configJson` read (decrypted) and parsed.
#[derive(Clone, Debug)]
pub struct BridgeInfo {
    pub chat_bridge_id: String,
    pub type_enum_id: String,
    pub adapter_name: String,
    pub name: String,
    pub config: serde_json::Value,
}

/// What an adapter supports. An operation it does not support is refused before the local change, with the reason
/// code `bridgeUnsupported`, and the screens leave its action out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub edit: bool,
    pub delete: bool,
    pub reaction: bool,
    /// The destination dedupes a send by its idempotency key, so an attempt whose outcome was lost is sent again; else
    /// it ends uncertain.
    pub idempotent: bool,
    /// Sends a node makes to the bridge a minute; 0 for no limit.
    pub sends_per_minute: u32,
}

/// What a delivery carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryKind {
    Message,
    Edit,
    Reaction,
    /// A delete of a message, or of a reaction (`reaction_key` set).
    Redaction,
}

/// A file of a message that goes out with it (an MMS picture): its `ChatAttachment`. An adapter that sends files by
/// address gives the network [`media_url`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryAttachment {
    pub chat_attachment_id: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub file_size: Option<i64>,
}

/// One delivery attempt.
#[derive(Clone, Debug)]
pub struct DeliveryRequest {
    pub chat_bridge_delivery_id: String,
    /// The same on every attempt of this delivery.
    pub idempotency_key: String,
    pub chat_room_id: String,
    pub event_seq: i64,
    pub external_room_id: String,
    pub kind: DeliveryKind,
    /// The text (a message, the new text of an edit), plain.
    pub text: Option<String>,
    /// The same as safe HTML, as the room keeps it.
    pub html: Option<String>,
    pub sender_name: Option<String>,
    /// The local user who sent it (none for an external actor's event); an adapter checks its own sending rules by it.
    pub sender_user_id: Option<String>,
    /// The message's files, in order.
    pub attachments: Vec<DeliveryAttachment>,
    /// The network's id of the message an edit, reaction or delete is about (of the reaction a reaction delete takes
    /// back).
    pub target_remote_id: Option<String>,
    pub reaction_key: Option<String>,
    /// This attempt's number, from 1.
    pub attempt: i64,
}

/// The outcome of an attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// The network took it; its id there.
    Accepted { remote_event_id: String },
    /// A failure worth trying again. `not_connected` marks the bridge not connected until a send gets through.
    Retry { error: String, not_connected: bool },
    /// A failure that another attempt would not change, with its reason code.
    Failed { reason_code: String, error: String },
    /// The attempt may or may not have reached the network, and it cannot tell.
    Uncertain { error: String },
}

/// A network adapter. See the module notes.
pub trait ChatBridgeAdapter: Send + Sync {
    /// The name a `ChatBridge` row selects it by (`adapterName`, else the network's enumeration code).
    fn name(&self) -> &str;
    fn capabilities(&self, bridge: &BridgeInfo) -> Capabilities;
    /// Connect the bridge on this node. An error marks it not connected, with the error as its last error, and the
    /// next round tries again.
    fn start(&self, bridge: &BridgeInfo) -> Result<(), String> {
        let _ = bridge;
        Ok(())
    }
    /// Disconnect it (disabled, or the server stopping).
    fn stop(&self, chat_bridge_id: &str) {
        let _ = chat_bridge_id;
    }
    fn deliver(&self, bridge: &BridgeInfo, request: &DeliveryRequest) -> DeliveryOutcome;
    /// A request to the bridge's webhook route (`/channels/{chatBridgeId}/webhook` on the host, [`webhook`]): the raw
    /// body, read within its bound, and the headers. The adapter checks the network's signature over the raw bytes
    /// before it parses anything, then hands what came to `ChatBridgeServices.ingest#Event`. Called on a request worker
    /// in a fresh context; the default answers as a request with no signature ([`WebhookResponse::not_signed`]: the
    /// network sends no webhooks).
    fn webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> WebhookResponse {
        let _ = (bridge, request);
        WebhookResponse::not_signed()
    }
    /// The signature step of [`Self::webhook`] alone, for a disabled bridge: None when the request is the network's,
    /// else the refusal [`Self::webhook`] would give. A disabled bridge then answers 503 only to its network, and to
    /// anyone else as an unknown bridge does, so bridge ids cannot be probed. The default refuses every request.
    fn verify_webhook(&self, bridge: &BridgeInfo, request: &WebhookRequest) -> Option<WebhookResponse> {
        let _ = (bridge, request);
        Some(WebhookResponse::not_signed())
    }
    /// The bytes of a file a message from the network names (`ChatBridgeServices.store#BridgeMedia`), up to
    /// `max_bytes`. The default fetches nothing.
    ///
    /// # Errors
    ///
    /// Returns why the file could not be fetched.
    fn fetch_media(&self, bridge: &BridgeInfo, url: &str, max_bytes: u64) -> Result<FetchedMedia, String> {
        let _ = (bridge, url, max_bytes);
        Err("this network's files are not fetched".to_string())
    }
}

/// The adapters of a factory: the linked crates' ([`crate::extension::Extension::chat_bridge_adapters`]) and those
/// added with [`register_adapter`].
#[derive(Default)]
struct Adapters {
    linked_read: AtomicBool,
    by_name: RwLock<HashMap<String, Arc<dyn ChatBridgeAdapter>>>,
}

fn adapters(factory: &ExecutionContextFactory) -> Arc<Adapters> {
    let adapters = factory.extension(|_| Adapters::default());
    if !adapters.linked_read.swap(true, Ordering::SeqCst) {
        let mut map = write(&adapters.by_name);
        for extension in crate::extension::linked() {
            for adapter in (extension.chat_bridge_adapters)() {
                map.entry(adapter.name().to_string()).or_insert(adapter);
            }
        }
    }
    adapters
}

/// Add an adapter to this factory, replacing one of the same name.
pub fn register_adapter(factory: &ExecutionContextFactory, adapter: Arc<dyn ChatBridgeAdapter>) {
    let adapters = adapters(factory);
    write(&adapters.by_name).insert(adapter.name().to_string(), adapter);
}

fn adapter_named(factory: &ExecutionContextFactory, name: &str) -> Option<Arc<dyn ChatBridgeAdapter>> {
    read(&adapters(factory).by_name).get(name).cloned()
}

// ---- helpers ----

fn text_of(row: &EntityValue, field: &str) -> Option<String> {
    row.get(field)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn int_of(row: &EntityValue, field: &str) -> Option<i64> {
    match row.get(field)? {
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        Value::Null => None,
        other => to_plain_string(other).trim().parse().ok(),
    }
}

fn millis_of(row: &EntityValue, field: &str) -> Option<i64> {
    match row.get(field)? {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        Value::I64(n) => Some(*n),
        Value::Null => None,
        other => to_plain_string(other).trim().parse().ok(),
    }
}

fn param(parameters: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn stamp(millis: i64) -> Value {
    crate::user::stamp_value(millis)
}

fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

fn current_factory() -> Option<Arc<ExecutionContextFactory>> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
}

fn force_new<T>(
    facade: &EntityFacade,
    entity: &str,
    body: impl FnOnce() -> Result<T, LibError>,
) -> Result<T, LibError> {
    facade
        .transactions_for(entity)?
        .run(TransactionAttribute::ForceNew, None, body)
}

fn bridge_info(row: &EntityValue) -> BridgeInfo {
    let config = text_of(row, "configJson")
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    BridgeInfo {
        chat_bridge_id: text_of(row, "chatBridgeId").unwrap_or_default(),
        type_enum_id: text_of(row, "typeEnumId").unwrap_or_default(),
        adapter_name: text_of(row, "adapterName").unwrap_or_default(),
        name: text_of(row, "name").unwrap_or_default(),
        config,
    }
}

/// The adapter of a bridge: its `adapterName`, else its network's enumeration code.
fn adapter_of(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    bridge: &BridgeInfo,
) -> Option<Arc<dyn ChatBridgeAdapter>> {
    if !bridge.adapter_name.is_empty() {
        return adapter_named(factory, &bridge.adapter_name);
    }
    let code = facade
        .find("moqui.basic.Enumeration")
        .and_then(|find| {
            find.eq("enumId", Value::string(&bridge.type_enum_id))
                .disable_authz()
                .one()
        })
        .ok()
        .flatten()
        .and_then(|row| text_of(&row, "enumCode"))?;
    adapter_named(factory, &code)
}

// ---- local actors ----

/// `get#LocalActors`: the `ChatActor` of each user of `userIdList`, made when missing. Out: `actorMap` userId to
/// chatActorId.
fn get_local_actors(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let users = super::shell::user_ids(parameters.get("userIdList"));
    let actors = local_actors(facade, &users)?;
    let map: IndexMap<String, Value> = actors
        .into_iter()
        .map(|(user, actor)| (user, Value::string(actor)))
        .collect();
    Ok(BTreeMap::from([("actorMap".to_string(), Value::Map(map))]))
}

/// The actor of each user: a local user's actor id is its userId, so the row is made when missing with one
/// create-or-update (an insert that another transaction's insert of the same row does not fail), in the caller's
/// transaction.
///
/// # Errors
///
/// Returns an error when the actors cannot be read or written, or a user's id is an external actor's.
pub fn local_actors(facade: &EntityFacade, users: &[String]) -> Result<IndexMap<String, String>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut out = IndexMap::new();
    let mut found: HashMap<String, Option<String>> = HashMap::new();
    for chunk in users.chunks(500) {
        let rows = facade
            .find(ACTOR)?
            .condition(Condition::is_in(
                "chatActorId",
                chunk.iter().map(Value::string).collect(),
            ))
            .disable_authz()
            .list()?;
        for row in rows.values() {
            if let Some(actor) = text_of(row, "chatActorId") {
                found.insert(actor, text_of(row, "userId"));
            }
        }
    }
    for user in users {
        match found.get(user) {
            Some(Some(owner)) if owner == user => {}
            Some(_) => {
                return Err(LibError::new(format!(
                    "Chat actor {user} is not the actor of user {user}"
                )));
            }
            None => {
                let mut row = facade.make_value(ACTOR)?;
                row.set("chatActorId", Value::string(user));
                row.set("kindEnumId", Value::string("CakLocal"));
                row.set("userId", Value::string(user));
                row.set("createdDate", stamp(now_millis()));
                row.create_or_update(facade)?;
            }
        }
        out.insert(user.clone(), user.clone());
    }
    Ok(out)
}

// ---- tokens ----

fn sha256_hex(text: &str) -> String {
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `check#BridgeToken`: whether `token` is the inbound token of bridge `chatBridgeId` (its SHA-256 against the row's
/// `inboundTokenHash`, compared in constant time). A bridge with no hash takes no token. Out: `valid`, and
/// `tokenHash` when `hashOnly` (for storing a new token).
fn check_bridge_token(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let token = param(parameters, "token").unwrap_or_default();
    if parameters
        .get("hashOnly")
        .is_some_and(|value| to_plain_string(value) == "true")
    {
        return Ok(BTreeMap::from([(
            "tokenHash".to_string(),
            Value::string(sha256_hex(&token)),
        )]));
    }
    let bridge_id = param(parameters, "chatBridgeId").unwrap_or_default();
    let stored = facade
        .find(BRIDGE)?
        .eq("chatBridgeId", Value::string(&bridge_id))
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "inboundTokenHash"));
    let valid = match stored {
        Some(stored) if !token.is_empty() => {
            let given = sha256_hex(&token);
            given.len() == stored.len()
                && given
                    .bytes()
                    .zip(stored.to_ascii_lowercase().bytes())
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
        }
        _ => false,
    };
    Ok(BTreeMap::from([("valid".to_string(), Value::Bool(valid))]))
}

// ---- capabilities ----

/// `get#BridgeCapabilities`: for each bridge of `chatBridgeIdList`, `{edit, delete, reaction, idempotent, adapter}`
/// (`adapter` false when this build has no adapter for it: then nothing is supported).
fn get_bridge_capabilities(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let ids = super::shell::user_ids(parameters.get("chatBridgeIdList"));
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let mut map = IndexMap::new();
    for id in ids {
        let Some(row) = facade
            .find(BRIDGE)?
            .eq("chatBridgeId", Value::string(&id))
            .disable_authz()
            .one()?
        else {
            continue;
        };
        let info = bridge_info(&row);
        let adapter = adapter_of(&factory, facade, &info);
        let caps = adapter
            .as_ref()
            .map(|adapter| adapter.capabilities(&info))
            .unwrap_or_default();
        let entry: IndexMap<String, Value> = [
            ("edit", Value::Bool(caps.edit)),
            ("delete", Value::Bool(caps.delete)),
            ("reaction", Value::Bool(caps.reaction)),
            ("idempotent", Value::Bool(caps.idempotent)),
            ("adapter", Value::Bool(adapter.is_some())),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
        map.insert(id, Value::Map(entry));
    }
    Ok(BTreeMap::from([("capabilityMap".to_string(), Value::Map(map))]))
}

// ---- the worker ----

/// This node's worker: whether it runs, what wakes it, the bridges it started, and the sends of the last minute per
/// bridge.
#[derive(Default)]
struct Worker {
    started: AtomicBool,
    /// Any bridge exists (seen at start or on a wake): the worker then also looks once a minute.
    active: AtomicBool,
    wake: Mutex<bool>,
    signal: Condvar,
    state: Mutex<WorkerState>,
    /// The worker thread, joined by [`shutdown`].
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[derive(Default)]
struct WorkerState {
    started_bridges: HashSet<String>,
    sends: HashMap<String, VecDeque<i64>>,
    cleaned: Option<i64>,
}

fn worker(factory: &ExecutionContextFactory) -> Arc<Worker> {
    factory.extension(|_| Worker::default())
}

/// Wake this node's worker (a new delivery row committed, or a retry).
pub fn wake(factory: &ExecutionContextFactory) {
    let worker = worker(factory);
    worker.active.store(true, Ordering::SeqCst);
    *lock(&worker.wake) = true;
    worker.signal.notify_all();
}

/// `wake#BridgeDelivery`: wake the worker once the caller's transaction commits (at once outside one; nothing on a
/// rollback).
fn wake_bridge_delivery(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let Some(factory) = current_factory() else {
        return Ok(BTreeMap::new());
    };
    let weak = Arc::downgrade(&factory);
    let send = move || {
        if let Some(factory) = weak.upgrade() {
            wake(&factory);
        }
    };
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(send) {
                tracing::warn!("Could not wait for the commit to wake the chat bridge worker: {err}");
                wake(&factory);
            }
        }
        _ => send(),
    }
    Ok(BTreeMap::new())
}

/// Start this node's worker thread, once (the host calls it after the cluster starts, so leases name the instance).
/// Its first round runs at once: the rows a crash left.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if factory.entity().is_none() {
        return;
    }
    let state = worker(factory);
    if state.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-chat-bridge".to_string())
        .spawn(move || {
            let mut first = true;
            loop {
                // The factory is held only for a round, not while waiting, so a stopped server releases it.
                let worker = {
                    let Some(factory) = weak.upgrade() else {
                        return;
                    };
                    if factory.is_shutting_down() {
                        stop_all(&factory);
                        return;
                    }
                    worker(&factory)
                };
                if !first {
                    let woken = {
                        let guard = lock(&worker.wake);
                        let (mut guard, _) = worker
                            .signal
                            .wait_timeout_while(guard, SWEEP, |woken| !*woken)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        std::mem::take(&mut *guard)
                    };
                    if !woken && !worker.active.load(Ordering::SeqCst) {
                        continue;
                    }
                }
                first = false;
                let Some(factory) = weak.upgrade() else {
                    return;
                };
                if factory.is_shutting_down() {
                    stop_all(&factory);
                    return;
                }
                let Some(facade) = factory.entity() else {
                    return;
                };
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                match round(&factory, &facade, now_millis()) {
                    Ok(report) if report.sent > 0 || report.failed > 0 => tracing::debug!(
                        "chat bridge round: {} sent, {} failed, {} waiting",
                        report.sent,
                        report.failed,
                        report.waiting
                    ),
                    Ok(_) => {}
                    Err(error) => tracing::error!("the chat bridge round failed: {error}"),
                }
            }
        });
    match spawned {
        Ok(handle) => *lock(&state.thread) = Some(handle),
        Err(error) => tracing::error!("the chat bridge worker did not start: {error}"),
    }
}

/// Wake this node's worker to stop its adapters and end, and wait for it. The factory calls it as it shuts down.
pub(crate) fn shutdown(factory: &ExecutionContextFactory) {
    let worker = worker(factory);
    let Some(handle) = lock(&worker.thread).take() else {
        return;
    };
    *lock(&worker.wake) = true;
    worker.signal.notify_all();
    if handle.thread().id() != std::thread::current().id() && handle.join().is_err() {
        tracing::error!("the chat bridge worker panicked");
    }
}

fn stop_all(factory: &ExecutionContextFactory) {
    let started: Vec<String> = lock(&worker(factory).state).started_bridges.drain().collect();
    let Some(facade) = factory.entity() else {
        return;
    };
    for id in started {
        let row = facade
            .find(BRIDGE)
            .and_then(|find| find.eq("chatBridgeId", Value::string(&id)).disable_authz().one());
        if let Ok(Some(row)) = row {
            let info = bridge_info(&row);
            if let Some(adapter) = adapter_of(factory, &facade, &info) {
                adapter.stop(&id);
            }
        }
    }
}

/// What a round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoundReport {
    /// Rows the network took.
    pub sent: usize,
    /// Rows that failed for good or ended uncertain.
    pub failed: usize,
    /// Rows tried that will be tried again.
    pub retried: usize,
    /// Rows held back: a rate limit, a disabled bridge, no adapter, a lease of another worker, a wait.
    pub waiting: usize,
}

/// `run#BridgeDeliveries`: one round now (or at `nowMillis`, a test's clock). Out: sent, failed, retried, waiting.
fn run_bridge_deliveries(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let factory = current_factory().ok_or_else(|| LibError::new("no execution context"))?;
    let now = param(parameters, "nowMillis")
        .and_then(|text| text.parse::<i64>().ok())
        .unwrap_or_else(now_millis);
    let report = round(&factory, facade, now)?;
    let as_value = |count: usize| Value::I64(i64::try_from(count).unwrap_or(i64::MAX));
    Ok(BTreeMap::from([
        ("sent".to_string(), as_value(report.sent)),
        ("failed".to_string(), as_value(report.failed)),
        ("retried".to_string(), as_value(report.retried)),
        ("waiting".to_string(), as_value(report.waiting)),
    ]))
}

fn node_id(factory: &ExecutionContextFactory) -> String {
    crate::cluster::instance_id(factory).unwrap_or_else(|| format!("node-{}", std::process::id()))
}

/// One round at `now`: start the bridges' adapters on this node, work the inbound batches left waiting
/// (`ChatBridgeServices.process#PendingInbound`), then send the due deliveries. Each bridge, batch and row is on its
/// own: one that fails is logged and the rest go on.
///
/// # Errors
///
/// Returns an error when the deliveries cannot be read.
pub fn round(factory: &ExecutionContextFactory, facade: &EntityFacade, now: i64) -> Result<RoundReport, LibError> {
    let worker = worker(factory);
    let bridges = facade.find(BRIDGE)?.disable_authz().list()?;
    if !bridges.values().is_empty() {
        worker.active.store(true, Ordering::SeqCst);
    }
    let mut infos: HashMap<String, Bridge> = HashMap::new();
    for row in bridges.values() {
        let info = bridge_info(row);
        let status = text_of(row, "statusId").unwrap_or_default();
        let last_error = text_of(row, "lastErrorText");
        lifecycle(factory, facade, &worker, &info, (&status, last_error.as_deref()), now);
        let sends_per_minute = int_of(row, "sendsPerMinute").and_then(|n| u32::try_from(n).ok());
        infos.insert(
            info.chat_bridge_id.clone(),
            Bridge {
                info,
                status,
                sends_per_minute,
            },
        );
    }
    if !infos.is_empty() {
        if let Err(error) = super::shell::call_tolerant(
            "org.moqui.impl.ChatBridgeServices.process#PendingInbound",
            &BTreeMap::from([("nowMillis".to_string(), Value::I64(now))]),
        ) {
            tracing::error!("the chat bridges' waiting input could not be worked: {error}");
        }
        clean(&worker, facade, now);
    }
    let mut report = RoundReport::default();
    // the mappings with a pending row, each worked from its oldest
    let mappings: Vec<String> = facade
        .find(DELIVERY)?
        .eq("statusEnumId", Value::string(PENDING))
        .select_field("chatRoomBridgeId")
        .disable_authz()
        .list()?
        .values()
        .iter()
        .filter_map(|row| text_of(row, "chatRoomBridgeId"))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let node = node_id(factory);
    for mapping in mappings {
        for _ in 0..PER_MAPPING {
            let head = facade
                .find(DELIVERY)?
                .eq("chatRoomBridgeId", Value::string(&mapping))
                .eq("statusEnumId", Value::string(PENDING))
                .order_by("eventSeq", false)
                .limit(1)
                .disable_authz()
                .list()?;
            let Some(row) = head.values().first().cloned() else {
                break;
            };
            match deliver_head(factory, facade, &worker, &infos, &row, &node, now) {
                Ok(Step::Sent) => report.sent += 1,
                Ok(Step::Done) => report.failed += 1,
                Ok(Step::Retry) => {
                    report.retried += 1;
                    break;
                }
                Ok(Step::Hold) => {
                    report.waiting += 1;
                    break;
                }
                Err(error) => {
                    tracing::error!(
                        "chat delivery {} could not be worked: {error}",
                        text_of(&row, "chatBridgeDeliveryId").unwrap_or_default()
                    );
                    break;
                }
            }
        }
    }
    Ok(report)
}

/// Start an active bridge's adapter on this node once (stop a disabled one), keeping its status: not connected with
/// the error when its adapter is missing or will not start.
fn lifecycle(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    worker: &Worker,
    info: &BridgeInfo,
    (status, last_error): (&str, Option<&str>),
    now: i64,
) {
    let started = lock(&worker.state).started_bridges.contains(&info.chat_bridge_id);
    let adapter = adapter_of(factory, facade, info);
    if status == "CbsDisabled" {
        if started {
            if let Some(adapter) = adapter {
                adapter.stop(&info.chat_bridge_id);
            }
            lock(&worker.state).started_bridges.remove(&info.chat_bridge_id);
        }
        return;
    }
    if started {
        return;
    }
    let outcome = match adapter {
        None => Err(format!(
            "No chat bridge adapter {} in this build",
            if info.adapter_name.is_empty() {
                &info.type_enum_id
            } else {
                &info.adapter_name
            }
        )),
        Some(adapter) => match std::panic::catch_unwind(AssertUnwindSafe(|| adapter.start(info))) {
            Ok(result) => result,
            Err(_) => Err("the adapter failed to start".to_string()),
        },
    };
    match outcome {
        Ok(()) => {
            lock(&worker.state).started_bridges.insert(info.chat_bridge_id.clone());
            set_status(facade, &info.chat_bridge_id, "CbsConnected", None, now);
        }
        // the same failure again (each round tries) is not logged or written again
        Err(error) if status == "CbsNotConnected" && last_error == Some(error.as_str()) => {}
        Err(error) => {
            tracing::warn!("chat bridge {} did not start: {error}", info.chat_bridge_id);
            set_status(facade, &info.chat_bridge_id, "CbsNotConnected", Some(&error), now);
        }
    }
}

/// Keep a bridge's status (and its last error), unless it is disabled or already so.
fn set_status(facade: &EntityFacade, bridge_id: &str, status: &str, error: Option<&str>, now: i64) {
    let result = force_new(facade, BRIDGE, || {
        let mut fields = vec![
            ("statusId".to_string(), Value::string(status)),
            ("lastStatusDate".to_string(), stamp(now)),
        ];
        if let Some(error) = error {
            fields.push(("lastErrorText".to_string(), Value::string(error)));
            fields.push(("lastErrorDate".to_string(), stamp(now)));
        }
        let mut find = facade
            .find(BRIDGE)?
            .eq("chatBridgeId", Value::string(bridge_id))
            .condition(Condition::or(vec![
                Condition::is_null("statusId"),
                Condition::not_equals("statusId", Value::string("CbsDisabled")),
            ]));
        if error.is_none() {
            find = find.condition(Condition::or(vec![
                Condition::is_null("statusId"),
                Condition::not_equals("statusId", Value::string(status)),
            ]));
        }
        find.disable_authz().update_all(fields)
    });
    if let Err(error) = result {
        tracing::error!("Could not keep the status of chat bridge {bridge_id}: {error}");
    }
}

/// Remove the inbound batches done more than 30 days ago, once an hour.
fn clean(worker: &Worker, facade: &EntityFacade, now: i64) {
    {
        let mut state = lock(&worker.state);
        if state.cleaned.is_some_and(|at| now - at < 3_600_000) {
            return;
        }
        state.cleaned = Some(now);
    }
    let result = force_new(facade, "moqui.chat.ChatBridgeInbound", || {
        facade
            .find("moqui.chat.ChatBridgeInbound")?
            .eq("statusEnumId", Value::string("CbiDone"))
            .condition(Condition::less("receivedDate", stamp(now - 30 * 86_400_000)))
            .disable_authz()
            .delete_all()
    });
    if let Err(error) = result {
        tracing::error!("Could not remove the old chat bridge batches: {error}");
    }
}

/// A bridge as a round reads it.
struct Bridge {
    info: BridgeInfo,
    status: String,
    sends_per_minute: Option<u32>,
}

enum Step {
    /// The network took it.
    Sent,
    /// Failed for good, or uncertain: the next row of the mapping may go.
    Done,
    /// Will be tried again later: the mapping waits.
    Retry,
    /// Not sent now (rate limit, disabled, no adapter, held by another worker, not due).
    Hold,
}

/// Work the oldest pending row of a mapping.
#[allow(clippy::too_many_arguments)]
fn deliver_head(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    worker: &Worker,
    infos: &HashMap<String, Bridge>,
    row: &EntityValue,
    node: &str,
    now: i64,
) -> Result<Step, LibError> {
    let delivery_id = text_of(row, "chatBridgeDeliveryId").unwrap_or_default();
    let attempts = int_of(row, "attemptCount").unwrap_or(0);
    if millis_of(row, "nextAttemptDate").is_some_and(|at| at > now)
        || (text_of(row, "leaseOwner").is_some() && millis_of(row, "leaseExpiresDate").is_some_and(|at| at > now))
    {
        return Ok(Step::Hold);
    }
    let bridge_id = text_of(row, "chatBridgeId").unwrap_or_default();
    let Some(Bridge {
        info,
        status,
        sends_per_minute,
    }) = infos.get(&bridge_id)
    else {
        finish(
            facade,
            &delivery_id,
            attempts,
            FAILED,
            Some("bridgeGone"),
            Some("The bridge is gone"),
            None,
            now,
        )?;
        return Ok(Step::Done);
    };
    if status == "CbsDisabled" {
        return Ok(Step::Hold);
    }
    let Some(adapter) = adapter_of(factory, facade, info) else {
        return Ok(Step::Hold);
    };
    let caps = adapter.capabilities(info);
    let limit = sends_per_minute.unwrap_or(caps.sends_per_minute);
    if limit > 0 {
        let mut state = lock(&worker.state);
        let sends = state.sends.entry(bridge_id.clone()).or_default();
        while sends.front().is_some_and(|at| now - at >= 60_000) {
            sends.pop_front();
        }
        if sends.len() >= limit as usize {
            return Ok(Step::Hold);
        }
    }
    // the request: what the event carries, and the network's id of what it is about
    let room_id = text_of(row, "chatRoomId").unwrap_or_default();
    let seq = int_of(row, "eventSeq").unwrap_or(0);
    let mapping = facade
        .find(MAPPING)?
        .eq(
            "chatRoomBridgeId",
            Value::string(text_of(row, "chatRoomBridgeId").unwrap_or_default()),
        )
        .disable_authz()
        .one()?;
    let Some(mapping) = mapping.filter(|mapping| text_of(mapping, "thruDate").is_none()) else {
        finish(
            facade,
            &delivery_id,
            attempts,
            FAILED,
            Some("mappingGone"),
            Some("The room is no longer mapped"),
            None,
            now,
        )?;
        push_state(&room_id, seq);
        return Ok(Step::Done);
    };
    let event = facade
        .find(EVENT)?
        .eq("chatRoomId", Value::string(&room_id))
        .eq("eventSeq", Value::I64(seq))
        .disable_authz()
        .one()?;
    let Some(event) = event else {
        finish(
            facade,
            &delivery_id,
            attempts,
            FAILED,
            Some("eventGone"),
            Some("The message is gone"),
            None,
            now,
        )?;
        return Ok(Step::Done);
    };
    let request = match request_of(facade, row, &mapping, &event, &bridge_id)? {
        Ok(request) => request,
        Err((reason, text)) => {
            finish(
                facade,
                &delivery_id,
                attempts,
                FAILED,
                Some(&reason),
                Some(&text),
                None,
                now,
            )?;
            return Ok(Step::Done);
        }
    };
    // what the adapter does not carry is not sent (an inbound edit relayed to a network without edits)
    let supported = match request.kind {
        DeliveryKind::Message => true,
        DeliveryKind::Edit => caps.edit,
        DeliveryKind::Reaction => caps.reaction,
        DeliveryKind::Redaction if request.reaction_key.is_some() => caps.reaction,
        DeliveryKind::Redaction => caps.delete,
    };
    if !supported {
        finish(
            facade,
            &delivery_id,
            attempts,
            FAILED,
            Some("bridgeUnsupported"),
            Some("This network does not carry it"),
            None,
            now,
        )?;
        push_state(&room_id, seq);
        return Ok(Step::Done);
    }
    // an earlier attempt whose lease ran out with no outcome was lost (a crash): sent again only where it dedupes
    let lost = text_of(row, "leaseOwner").is_some();
    if lost && !caps.idempotent {
        finish(
            facade,
            &delivery_id,
            attempts,
            UNCERTAIN,
            Some("lostInFlight"),
            Some("An attempt was cut off and this network cannot tell whether it arrived"),
            None,
            now,
        )?;
        push_state(&room_id, seq);
        return Ok(Step::Done);
    }
    let claimed = force_new(facade, DELIVERY, || {
        facade
            .find(DELIVERY)?
            .eq("chatBridgeDeliveryId", Value::string(&delivery_id))
            .eq("statusEnumId", Value::string(PENDING))
            .eq("attemptCount", Value::I64(attempts))
            .condition(Condition::or(vec![
                Condition::is_null("leaseExpiresDate"),
                Condition::less_equal("leaseExpiresDate", stamp(now)),
            ]))
            .disable_authz()
            .update_all([
                ("attemptCount".to_string(), Value::I64(attempts + 1)),
                ("leaseOwner".to_string(), Value::string(node)),
                ("leaseExpiresDate".to_string(), stamp(now + LEASE_MILLIS)),
                ("lastAttemptDate".to_string(), stamp(now)),
            ])
    })?;
    if claimed == 0 {
        return Ok(Step::Hold);
    }
    if limit > 0 {
        lock(&worker.state)
            .sends
            .entry(bridge_id.clone())
            .or_default()
            .push_back(now);
    }
    let request = DeliveryRequest {
        attempt: attempts + 1,
        ..request
    };
    let Ok(outcome) = std::panic::catch_unwind(AssertUnwindSafe(|| adapter.deliver(info, &request))) else {
        // taken as a crash: the row keeps its lease and goes again once it runs out
        tracing::error!(
            "the chat bridge adapter {} failed on delivery {delivery_id}",
            adapter.name()
        );
        return Ok(Step::Retry);
    };
    let attempt = attempts + 1;
    let step = match outcome {
        DeliveryOutcome::Accepted { remote_event_id } => {
            finish(
                facade,
                &delivery_id,
                attempt,
                ACCEPTED,
                None,
                None,
                Some(&remote_event_id),
                now,
            )?;
            record_external(facade, &room_id, seq, &bridge_id, &mapping, &remote_event_id, now);
            set_status(facade, &bridge_id, "CbsConnected", None, now);
            Step::Sent
        }
        DeliveryOutcome::Retry { error, not_connected } => {
            if not_connected {
                set_status(facade, &bridge_id, "CbsNotConnected", Some(&error), now);
            }
            if attempt >= MAX_ATTEMPTS {
                finish(
                    facade,
                    &delivery_id,
                    attempt,
                    FAILED,
                    Some("attempts"),
                    Some(&error),
                    None,
                    now,
                )?;
                Step::Done
            } else {
                let wait = BACKOFF[usize::try_from(attempt - 1).unwrap_or(0).min(BACKOFF.len() - 1)];
                force_new(facade, DELIVERY, || {
                    // only while the row is still this attempt's (see finish)
                    facade
                        .find(DELIVERY)?
                        .eq("chatBridgeDeliveryId", Value::string(&delivery_id))
                        .eq("statusEnumId", Value::string(PENDING))
                        .eq("attemptCount", Value::I64(attempt))
                        .disable_authz()
                        .update_all([
                            ("lastErrorText".to_string(), Value::string(&error)),
                            ("nextAttemptDate".to_string(), stamp(now + wait)),
                            ("leaseOwner".to_string(), Value::Null),
                            ("leaseExpiresDate".to_string(), Value::Null),
                        ])
                })?;
                Step::Retry
            }
        }
        DeliveryOutcome::Failed { reason_code, error } => {
            finish(
                facade,
                &delivery_id,
                attempt,
                FAILED,
                Some(&reason_code),
                Some(&error),
                None,
                now,
            )?;
            Step::Done
        }
        DeliveryOutcome::Uncertain { error } => {
            finish(
                facade,
                &delivery_id,
                attempt,
                UNCERTAIN,
                Some("uncertain"),
                Some(&error),
                None,
                now,
            )?;
            Step::Done
        }
    };
    if !matches!(step, Step::Retry) {
        push_state(&room_id, seq);
    }
    Ok(step)
}

/// The request of a row, or the reason it cannot be sent.
fn request_of(
    facade: &EntityFacade,
    row: &EntityValue,
    mapping: &EntityValue,
    event: &EntityValue,
    bridge_id: &str,
) -> Result<Result<DeliveryRequest, (String, String)>, LibError> {
    let room_id = text_of(row, "chatRoomId").unwrap_or_default();
    let content: serde_json::Value = text_of(event, "contentJson")
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    let kind = match text_of(event, "eventTypeEnumId").as_deref() {
        Some("CetMessage") => DeliveryKind::Message,
        Some("CetEdit") => DeliveryKind::Edit,
        Some("CetReaction") => DeliveryKind::Reaction,
        Some("CetRedaction") => DeliveryKind::Redaction,
        other => {
            return Ok(Err((
                "unsupported".to_string(),
                format!("A {} event is not sent to a bridge", other.unwrap_or("blank")),
            )));
        }
    };
    if kind == DeliveryKind::Message && text_of(event, "redactedDate").is_some() {
        return Ok(Err((
            "redacted".to_string(),
            "The message was deleted before it was sent".to_string(),
        )));
    }
    let target_seq = match kind {
        DeliveryKind::Message => None,
        DeliveryKind::Edit => int_of(event, "editsEventSeq"),
        DeliveryKind::Reaction | DeliveryKind::Redaction => int_of(event, "relatesToEventSeq"),
    };
    // a reaction delete names the reaction; its key is in the redaction's content
    let reaction_key = match kind {
        DeliveryKind::Reaction => content.get("key").and_then(|key| key.as_str()).map(str::to_string),
        DeliveryKind::Redaction if content.get("target").and_then(|target| target.as_str()) == Some("reaction") => {
            content.get("key").and_then(|key| key.as_str()).map(str::to_string)
        }
        _ => None,
    };
    let mut target_remote_id = None;
    if let Some(target) = target_seq {
        target_remote_id = remote_id_of(facade, row, &room_id, target, bridge_id)?;
        if target_remote_id.is_none() {
            return Ok(Err((
                "targetNotDelivered".to_string(),
                "What it is about never reached this network".to_string(),
            )));
        }
    }
    let html = match kind {
        DeliveryKind::Message | DeliveryKind::Edit => content
            .get("body")
            .and_then(|body| body.as_str())
            .map(str::to_string)
            .or_else(|| {
                content
                    .get("poll")
                    .and_then(|poll| poll.get("question"))
                    .and_then(|question| question.as_str())
                    .map(str::to_string)
            }),
        _ => None,
    };
    let sender_name = sender_name(facade, event)?;
    let attachments = match kind {
        DeliveryKind::Message => attachments_of(facade, &room_id, int_of(row, "eventSeq").unwrap_or(0))?,
        _ => Vec::new(),
    };
    Ok(Ok(DeliveryRequest {
        chat_bridge_delivery_id: text_of(row, "chatBridgeDeliveryId").unwrap_or_default(),
        idempotency_key: text_of(row, "idempotencyKey").unwrap_or_default(),
        chat_room_id: room_id,
        event_seq: int_of(row, "eventSeq").unwrap_or(0),
        external_room_id: text_of(mapping, "externalRoomId").unwrap_or_default(),
        kind,
        text: text_of(event, "bodyText"),
        html,
        sender_name,
        sender_user_id: text_of(event, "senderUserId"),
        attachments,
        target_remote_id,
        reaction_key,
        attempt: 0,
    }))
}

/// The files of a message, in their order.
fn attachments_of(facade: &EntityFacade, room_id: &str, seq: i64) -> Result<Vec<DeliveryAttachment>, LibError> {
    Ok(facade
        .find("moqui.chat.ChatAttachment")?
        .eq("chatRoomId", Value::string(room_id))
        .eq("eventSeq", Value::I64(seq))
        .order_by("attachmentSeq", false)
        .disable_authz()
        .list()?
        .values()
        .iter()
        .map(|row| DeliveryAttachment {
            chat_attachment_id: text_of(row, "chatAttachmentId").unwrap_or_default(),
            file_name: text_of(row, "fileName"),
            content_type: text_of(row, "contentType"),
            file_size: int_of(row, "fileSize"),
        })
        .collect())
}

/// The network's id of event `target`: the remote id of its delivery to this mapping (the row's dependency), else its
/// external mapping on this bridge (an event that came from it).
fn remote_id_of(
    facade: &EntityFacade,
    row: &EntityValue,
    room_id: &str,
    target: i64,
    bridge_id: &str,
) -> Result<Option<String>, LibError> {
    if let Some(depends) = text_of(row, "dependsOnDeliveryId") {
        if let Some(dependency) = facade
            .find(DELIVERY)?
            .eq("chatBridgeDeliveryId", Value::string(&depends))
            .disable_authz()
            .one()?
        {
            if text_of(&dependency, "statusEnumId").as_deref() == Some(ACCEPTED) {
                return Ok(text_of(&dependency, "remoteEventId"));
            }
        }
    }
    Ok(facade
        .find(EXTERNAL_EVENT)?
        .eq("chatRoomId", Value::string(room_id))
        .eq("eventSeq", Value::I64(target))
        .eq("chatBridgeId", Value::string(bridge_id))
        .order_by("createdDate", false)
        .limit(1)
        .disable_authz()
        .list()?
        .values()
        .first()
        .and_then(|row| text_of(row, "externalEventId")))
}

/// The name of an event's sender: the local user's, or the external actor's.
fn sender_name(facade: &EntityFacade, event: &EntityValue) -> Result<Option<String>, LibError> {
    if let Some(user) = text_of(event, "senderUserId") {
        let account = facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string(&user))
            .disable_authz()
            .one()?;
        return Ok(account
            .and_then(|row| text_of(&row, "userFullName").or_else(|| text_of(&row, "username")))
            .or(Some(user)));
    }
    let Some(actor) = text_of(event, "senderActorId") else {
        return Ok(None);
    };
    let external = facade
        .find(ACTOR)?
        .eq("chatActorId", Value::string(&actor))
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "chatExternalUserId"));
    let Some(external) = external else {
        return Ok(None);
    };
    Ok(facade
        .find("moqui.chat.ChatExternalUser")?
        .eq("chatExternalUserId", Value::string(&external))
        .disable_authz()
        .one()?
        .and_then(|row| text_of(&row, "displayName").or_else(|| text_of(&row, "externalUserId"))))
}

/// End a row's attempt: its status, reason and error, the remote id, and no lease. Only while the row is still pending
/// on `attempt` (the attempt this worker claimed, or the count it read before a claim), so a worker whose lease ran out
/// while it sent does not overwrite the outcome of the worker that took the row over; an acceptance is kept over any
/// state but accepted, as the network has the message whoever else tried.
#[allow(clippy::too_many_arguments)]
fn finish(
    facade: &EntityFacade,
    delivery_id: &str,
    attempt: i64,
    status: &str,
    reason: Option<&str>,
    error: Option<&str>,
    remote_id: Option<&str>,
    now: i64,
) -> Result<(), LibError> {
    let text = |value: Option<&str>| value.map_or(Value::Null, Value::string);
    let mut fields = vec![
        ("statusEnumId".to_string(), Value::string(status)),
        ("reasonCode".to_string(), text(reason)),
        ("leaseOwner".to_string(), Value::Null),
        ("leaseExpiresDate".to_string(), Value::Null),
        ("nextAttemptDate".to_string(), Value::Null),
    ];
    if error.is_some() {
        fields.push(("lastErrorText".to_string(), text(error)));
    }
    if status == ACCEPTED {
        fields.push(("remoteEventId".to_string(), text(remote_id)));
        fields.push(("acceptedDate".to_string(), stamp(now)));
        fields.push(("lastErrorText".to_string(), Value::Null));
    }
    force_new(facade, DELIVERY, || {
        let find = facade
            .find(DELIVERY)?
            .eq("chatBridgeDeliveryId", Value::string(delivery_id));
        let find = if status == ACCEPTED {
            find.condition(Condition::not_equals("statusEnumId", Value::string(ACCEPTED)))
        } else {
            find.eq("statusEnumId", Value::string(PENDING))
                .eq("attemptCount", Value::I64(attempt))
        };
        find.disable_authz().update_all(fields)
    })?;
    Ok(())
}

/// Keep the network's id of an event this node sent (origin outbound), so the network's echo of it is suppressed and
/// later edits find it. One already kept (an echo that came first) stays.
fn record_external(
    facade: &EntityFacade,
    room_id: &str,
    seq: i64,
    bridge_id: &str,
    mapping: &EntityValue,
    remote_id: &str,
    now: i64,
) {
    let external_room = text_of(mapping, "externalRoomId").unwrap_or_default();
    let result = force_new(facade, EXTERNAL_EVENT, || {
        let known = facade
            .find(EXTERNAL_EVENT)?
            .eq("chatBridgeId", Value::string(bridge_id))
            .eq("externalRoomId", Value::string(&external_room))
            .eq("externalEventId", Value::string(remote_id))
            .disable_authz()
            .count()?;
        if known > 0 {
            return Ok(());
        }
        let mut row = facade.make_value(EXTERNAL_EVENT)?;
        row.set_sequenced_id_primary(facade)?;
        row.set("chatRoomId", Value::string(room_id));
        row.set("eventSeq", Value::I64(seq));
        row.set("chatBridgeId", Value::string(bridge_id));
        row.set(
            "chatRoomBridgeId",
            text_of(mapping, "chatRoomBridgeId").map_or(Value::Null, Value::string),
        );
        row.set("externalRoomId", Value::string(&external_room));
        row.set("externalEventId", Value::string(remote_id));
        row.set("originEnumId", Value::string("CeoOutbound"));
        row.set("originDate", stamp(now));
        row.set("createdDate", stamp(now));
        row.create(facade)
    });
    if let Err(error) = result {
        tracing::warn!("Could not keep the network id {remote_id} of chat event {room_id}::{seq}: {error}");
    }
}

/// Push a message's delivery state to its room (`ChatBridgeServices.push#DeliveryState`); a failure is logged.
fn push_state(room_id: &str, seq: i64) {
    let parameters = BTreeMap::from([
        ("chatRoomId".to_string(), Value::string(room_id)),
        ("eventSeq".to_string(), Value::I64(seq)),
    ]);
    if let Err(error) = super::shell::call_tolerant("org.moqui.impl.ChatBridgeServices.push#DeliveryState", &parameters)
    {
        tracing::warn!("Could not push the delivery state of chat event {room_id}::{seq}: {error}");
    }
}

#[path = "chat_bridge_web.rs"]
mod web;
pub use web::{
    media, media_token_valid, media_url, replay_finish, replay_take, replay_window_ms, webhook, FetchedMedia,
    MediaFile, Replay, WebhookRequest, WebhookResponse,
};

#[cfg(test)]
#[path = "chat_bridge_tests.rs"]
mod tests;
