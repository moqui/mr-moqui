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

//! The link's queue (plans/Edge-Node.md §5.2): durable and ordered, delivered at least once and applied once by
//! receipt, either way. The sender keeps `moqui.edge.EdgeQueueItem` rows (on an edge node the items for a local node,
//! on a local node its outbox); the host's link takes the head of each conversation key in order ([`due_heads`],
//! [`claim`]), sends it, and records the answer ([`finish`]). The receiver writes the item's `moqui.edge.EdgeReceipt`
//! first and then applies it by its kind's handler service, in one transaction ([`apply`]): a redelivery whose
//! acknowledgement was lost finds the receipt and is only acknowledged, and one that overlaps the first apply waits on
//! the receipt's key.
//!
//! Head of line: the items of one conversation key go one at a time in `sequenceNum` order. A head the receiver refused
//! waits for its retry and holds the items after it, until it is delivered or fails for good (after
//! `queue-max-attempts` refusals, or past the retention period or the size cap); then the next one goes. Not counted as
//! a refusal: an attempt with no answer (the link dropped), and a receiver that is paused, busy or stopping (tried again
//! after `queue-retry-seconds`; an unavailable answer, such as a handler's `retryLater`, waits twice as long each time in
//! a row, to the 10-minute cap) or has no handler or grant for the kind (a version skew: tried again every
//! 10 minutes). Modeled on `ChatBridgeDelivery` (`crate::service::chat_bridge`): the claim is a conditional update of
//! the attempt count and the lease.
//!
//! Content: a body too large for the payload (a mail message, a media file) is the item's content. The sender keeps it
//! under [`CONTENT_OUT`] by item id from [`enqueue`] until retention removes the item; before each delivery the link
//! sends it to the receiver ([`read_content`]), which keeps it under [`CONTENT_IN`] by node and item id
//! ([`store_received`]) and names that location to the handler as `contentLocation`. The receiver removes its copy once
//! the delivery is answered, applied or not: each attempt sends the content again. A copy whose delivery never came is
//! removed by the retention pass. Content counts toward the size cap with the payload.
//!
//! On a clustered local node only the member that holds the link sends its outbox (the others' items wait for it, by
//! lease); a lock row for that is later work (plans/Edge-Node.md §2).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::conf::MAX_RETRY;
use super::EdgeSettings;
use crate::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value, value_millis};

pub const QUEUE_ITEM: &str = "moqui.edge.EdgeQueueItem";
pub const RECEIPT: &str = "moqui.edge.EdgeReceipt";
const HEAD: &str = "moqui.edge.EdgeQueueHead";
const COUNT: &str = "moqui.edge.EdgeQueueCount";
/// An item an edge node keeps for its local node.
pub const TO_LOCAL: &str = "EqdToLocal";
/// An item of a local node's outbox, for its edge node.
pub const TO_EDGE: &str = "EqdToEdge";
pub const PENDING: &str = "EqsPending";
pub const DELIVERED: &str = "EqsDelivered";
pub const FAILED: &str = "EqsFailed";
/// How long a delivery holds an item while it waits for the answer.
pub const LEASE_MILLIS: i64 = 60_000;
/// The largest payload an item carries: 1 MiB. A larger body goes as the item's content.
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// The largest content an item carries: 64 MiB (a mail message is at most `smtp-server` `max-message-size`, 25 MiB by
/// default).
pub const MAX_CONTENT_BYTES: usize = 64 * 1024 * 1024;
/// Where a sender keeps the content of its items, by item id.
pub const CONTENT_OUT: &str = "dbresource://moqui/edge/content/out/";
/// Where a receiver keeps the content sent to it, by paired node and item id, until the item's delivery is answered.
pub const CONTENT_IN: &str = "dbresource://moqui/edge/content/in/";
/// The heads one round of delivery takes at most, the oldest first.
const HEADS: usize = 200;
/// The rows one retention step reads or removes at a time.
const CLEAN_PAGE: i64 = 500;

/// What [`enqueue`] stores.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewItem {
    pub kind: String,
    pub conversation_key: String,
    /// Unique per node and direction; the item's id when empty.
    pub idempotency_key: String,
    pub payload: Option<String>,
    /// The item's content, at most [`MAX_CONTENT_BYTES`].
    pub content: Option<Vec<u8>>,
    /// A location whose bytes are the item's content, when `content` is `None`: read at enqueue and kept with the
    /// item, so the caller's file may go.
    pub content_location: Option<String>,
}

/// An item as it crosses the link. A head read for a round has no payload; [`claim`] reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueItem {
    pub queue_item_id: String,
    pub kind: String,
    pub conversation_key: String,
    pub idempotency_key: String,
    pub payload: String,
    /// On the sender its own copy of the content (under [`CONTENT_OUT`]); on the receiver where it kept what was sent
    /// (under [`CONTENT_IN`]). Empty for an item with no content.
    pub content_location: String,
    /// The attempts made before this one.
    pub attempts: u32,
    /// The uncounted "unavailable" answers in a row before this attempt: each doubles the wait, as counted refusals do.
    pub waits: u32,
}

/// The receiver's answer to a delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Applied now, with its receipt.
    Applied,
    /// Applied before: its receipt is there.
    Duplicate,
    /// The handler refused it or failed: try again later, counted.
    Failed(String),
    /// The receiver's delivery is paused: try again soon, not counted.
    Paused,
    /// The receiver cannot take it now (its pool is full, it is stopping, its delegate user cannot log in): try again
    /// soon, not counted.
    Unavailable(String),
    /// The receiver has no handler for the kind, or no grant for it: a version or conf skew, tried again every
    /// 10 minutes, not counted.
    NotHandled(String),
}

/// How a delivery ended, for [`finish`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Answered(Applied),
    /// No answer: the link dropped or timed out. Not counted as an attempt.
    Lost(String),
}

/// The direction this node's own items go: an edge node's to its local node, a local node's to its edge node.
pub fn outbound(settings: &EdgeSettings) -> &'static str {
    if settings.edge_node {
        TO_LOCAL
    } else {
        TO_EDGE
    }
}

/// The direction of the items this node receives.
pub fn inbound(settings: &EdgeSettings) -> &'static str {
    if settings.edge_node {
        TO_EDGE
    } else {
        TO_LOCAL
    }
}

/// A local node's own id in its queue rows: its instance id (the edge node keys its rows by the id it paired with).
pub fn local_node_id(factory: &ExecutionContextFactory) -> String {
    crate::cluster::instance_id(factory).unwrap_or_else(|| "local".to_string())
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

fn number(row: &EntityValue, field: &str) -> i64 {
    match row.get(field) {
        Some(Value::I64(number)) => *number,
        Some(value) => {
            let text = to_plain_string(value);
            let parsed: Option<i64> = text.parse().ok();
            parsed
                .or_else(|| text.split('.').next().and_then(|whole| whole.parse().ok()))
                .unwrap_or(0)
        }
        None => 0,
    }
}

fn millis(row: &EntityValue, field: &str) -> Option<i64> {
    row.get(field).and_then(value_millis)
}

fn transaction<T>(
    facade: &EntityFacade,
    attribute: TransactionAttribute,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .transactions_for(QUEUE_ITEM)?
        .run(attribute, Some(Duration::from_secs(30)), body)
}

/// The only active paired node of an edge node, for an item that names none.
fn only_paired_node(facade: &EntityFacade) -> Result<String, Error> {
    let rows = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .disable_authz()
        .limit(2)
        .list()?;
    match rows.values() {
        [only] => Ok(text(only, "pairedNodeId")),
        [] => Err(Error::new("no local node is paired with this edge node")),
        _ => Err(Error::new("several local nodes are paired: name the pairedNodeId")),
    }
}

/// The id of an item with an idempotency key: derived from the node, direction and key, so two enqueues of one key
/// make one row even when they race (the second insert finds the first's key), and a key queued again after the
/// sender's database was lost is the same item to the receiver's receipts.
fn keyed_id(node: &str, direction: &str, key: &str) -> String {
    let hash = moqui_lib::json::canonical_sha256(&serde_json::json!([node, direction, key]));
    format!("k{}", hash.get(..39).unwrap_or(&hash))
}

/// The id of a queued item and whether this call stored it (false: its idempotency key was there already).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enqueued {
    pub queue_item_id: String,
    pub created: bool,
}

/// Queue `item` for the other end of the link: on an edge node for `paired_node_id` (the only paired node when
/// `None`), on a local node for its edge node. The kind must be one this node's conf lists (`queue-kind`), the payload
/// at most [`MAX_PAYLOAD_BYTES`] and the content at most [`MAX_CONTENT_BYTES`]; the content is kept under
/// [`CONTENT_OUT`] in the transaction of the row. An idempotency key already queued returns that item.
pub fn enqueue(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    paired_node_id: Option<&str>,
    item: &NewItem,
) -> Result<Enqueued, Error> {
    let kind = item.kind.trim();
    if kind.is_empty() || item.conversation_key.trim().is_empty() {
        return Err(Error::new("an edge queue item needs a kind and a conversationKey"));
    }
    let settings = super::settings(factory);
    if !settings.kinds.contains_key(kind) {
        return Err(Error::new(format!(
            "{kind} is not a queue kind of this node's edge conf (queue-kind)"
        )));
    }
    let payload_size = item.payload.as_deref().map_or(0, str::len);
    if payload_size > MAX_PAYLOAD_BYTES {
        return Err(Error::new(format!(
            "an edge queue item's payload is at most {MAX_PAYLOAD_BYTES} bytes, and this one is {payload_size}; send a \
             larger body as its content"
        )));
    }
    let direction = outbound(&settings);
    let node = match (
        settings.edge_node,
        paired_node_id.map(str::trim).filter(|id| !id.is_empty()),
    ) {
        (true, Some(node)) => node.to_string(),
        (true, None) => only_paired_node(facade)?,
        (false, _) => local_node_id(factory),
    };
    let key = item.idempotency_key.trim();
    let (id, key) = if key.is_empty() {
        let id = crate::entity::uuid_v4()?;
        (id.clone(), id)
    } else {
        (keyed_id(&node, direction, key), key.to_string())
    };
    // a key queued already is that item: its content is not read again (E8: a greeting queued each minute)
    if !item.idempotency_key.trim().is_empty() {
        let existing = transaction(facade, TransactionAttribute::UseOrBegin, || {
            facade
                .find(QUEUE_ITEM)?
                .eq("queueItemId", Value::string(&id))
                .select_field("queueItemId")
                .disable_authz()
                .one()
        })?;
        if existing.is_some() {
            return Ok(Enqueued {
                queue_item_id: id,
                created: false,
            });
        }
    }
    let content = match (&item.content, item.content_location.as_deref().map(str::trim)) {
        (Some(content), _) => Some(std::borrow::Cow::Borrowed(content.as_slice())),
        (None, Some(location)) if !location.is_empty() => {
            let bytes = factory
                .resource()
                .location_bytes(location)
                .map_err(|err| Error::new(format!("the content of an edge queue item, {location}: {err}")))?
                .ok_or_else(|| Error::new(format!("the content of an edge queue item, {location}, is not there")))?;
            Some(std::borrow::Cow::Owned(bytes))
        }
        _ => None,
    };
    let content_size = content.as_ref().map_or(0, |content| content.len());
    if content_size > MAX_CONTENT_BYTES {
        return Err(Error::new(format!(
            "an edge queue item's content is at most {MAX_CONTENT_BYTES} bytes, and this one is {content_size}"
        )));
    }
    let size = payload_size + content_size;
    let created = transaction(facade, TransactionAttribute::UseOrBegin, || {
        let existing = facade
            .find(QUEUE_ITEM)?
            .eq("queueItemId", Value::string(&id))
            .select_field("queueItemId")
            .disable_authz()
            .one()?;
        if existing.is_some() {
            return Ok(false);
        }
        let sequence = crate::entity::sequenced_id_primary(facade, "moqui.edge.EdgeQueueItem.sequenceNum", None, None)?;
        let mut row = facade.make_value(QUEUE_ITEM)?;
        row.set("queueItemId", Value::string(&id));
        row.set("pairedNodeId", Value::string(&node));
        row.set("directionEnumId", Value::string(direction));
        row.set("sequenceNum", Value::I64(sequence.parse().unwrap_or(0)));
        row.set("kind", Value::string(kind));
        row.set("conversationKey", Value::string(item.conversation_key.trim()));
        row.set("idempotencyKey", Value::string(&key));
        row.set("payload", item.payload.as_deref().map_or(Value::Null, Value::string));
        let content_location = content.as_ref().map(|_| format!("{CONTENT_OUT}{id}"));
        row.set(
            "contentLocation",
            content_location.as_deref().map_or(Value::Null, Value::string),
        );
        // the size cap counts the content with the payload
        row.set("payloadSize", Value::I64(i64::try_from(size).unwrap_or(i64::MAX)));
        row.set("statusEnumId", Value::string(PENDING));
        row.set("attemptCount", Value::I64(0));
        row.set("createdDate", stamp_value(now_millis()));
        // a concurrent enqueue of the same key made the row first: this one is that item
        if !row.create_if_missing(facade)? {
            return Ok(false);
        }
        if let (Some(location), Some(content)) = (&content_location, &content) {
            facade.write_db_resource(location, content)?;
        }
        Ok(true)
    })?;
    if created {
        wake_after_commit(facade);
    }
    Ok(Enqueued {
        queue_item_id: id,
        created,
    })
}

/// What the link wakes on: a new item, a resume.
#[derive(Default)]
struct Wakers(Mutex<Vec<Arc<dyn Fn() + Send + Sync>>>);

/// Call `wake` when this node queues an item or resumes delivery, so the link's delivery need not poll often.
pub fn on_wake(facade: &EntityFacade, wake: Arc<dyn Fn() + Send + Sync>) {
    facade.with_slot(Wakers::default, |wakers| lock(&wakers.0).push(wake));
}

fn wake(facade: &EntityFacade) {
    let wakers: Vec<_> = facade.with_slot_read(Wakers::default, |wakers| lock(&wakers.0).clone());
    for wake in wakers {
        wake();
    }
}

fn wake_after_commit(facade: &EntityFacade) {
    let in_transaction = facade
        .transactions_for(QUEUE_ITEM)
        .ok()
        .filter(|tx| tx.in_transaction());
    match in_transaction {
        Some(tx) => {
            let facade = facade.clone();
            if let Err(err) = tx.on_commit(move || wake(&facade)) {
                tracing::debug!("edge queue: no wake after commit: {err}");
            }
        }
        None => wake(facade),
    }
}

/// The receiving side's pause (System › Edge Node, Pause): it answers every delivery "paused", and its own outbox
/// waits. Kept in memory: a restart resumes.
#[derive(Default)]
struct Pause(AtomicBool);

pub fn set_paused(facade: &EntityFacade, paused: bool) {
    facade.with_slot(Pause::default, |pause| pause.0.store(paused, Ordering::Release));
    if !paused {
        wake(facade);
    }
}

pub fn paused(facade: &EntityFacade) -> bool {
    facade.with_slot_read(Pause::default, |pause| pause.0.load(Ordering::Acquire))
}

/// The fields of a head, without its payload.
const HEAD_FIELDS: [&str; 9] = [
    "queueItemId",
    "kind",
    "conversationKey",
    "idempotencyKey",
    "attemptCount",
    "waitCount",
    "nextAttemptDate",
    "leaseExpiresDate",
    "sequenceNum",
];

fn queue_item(row: &EntityValue) -> QueueItem {
    QueueItem {
        queue_item_id: text(row, "queueItemId"),
        kind: text(row, "kind"),
        conversation_key: text(row, "conversationKey"),
        idempotency_key: text(row, "idempotencyKey"),
        payload: String::new(),
        content_location: String::new(),
        attempts: u32::try_from(number(row, "attemptCount")).unwrap_or(0),
        waits: u32::try_from(number(row, "waitCount")).unwrap_or(0),
    }
}

/// The items to send now, and when the next one waiting for its retry is due.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heads {
    /// The head of each conversation key that is due, oldest first, without payloads.
    pub due: Vec<QueueItem>,
    /// The earliest retry or lease end of a head that is not due (epoch milliseconds).
    pub next_due: Option<i64>,
}

/// The heads of `node`'s pending items going `direction`: the oldest pending item of each conversation key (one
/// grouped query, so a long backlog of one key does not hide another key's head), the oldest [`HEADS`] of them, each
/// when it is due (no retry wait and no live lease). A key whose head waits is held whole.
pub fn due_heads(facade: &EntityFacade, node: &str, direction: &str, now: i64) -> Result<Heads, Error> {
    transaction(facade, TransactionAttribute::UseOrBegin, || {
        let mut sequences: Vec<i64> = facade
            .find(HEAD)?
            .eq("pairedNodeId", Value::string(node))
            .eq("directionEnumId", Value::string(direction))
            .eq("statusEnumId", Value::string(PENDING))
            .select_field("conversationKey")
            .select_field("sequenceNum")
            .disable_authz()
            .list()?
            .values()
            .iter()
            .map(|row| number(row, "sequenceNum"))
            .collect();
        sequences.sort_unstable();
        sequences.truncate(HEADS);
        if sequences.is_empty() {
            return Ok(Heads::default());
        }
        let mut find = facade
            .find(QUEUE_ITEM)?
            .eq("pairedNodeId", Value::string(node))
            .eq("directionEnumId", Value::string(direction))
            .eq("statusEnumId", Value::string(PENDING))
            .condition(Condition::is_in(
                "sequenceNum",
                sequences.into_iter().map(Value::I64).collect(),
            ))
            .order_by("sequenceNum", false)
            .disable_authz();
        for field in HEAD_FIELDS {
            find = find.select_field(field);
        }
        let mut heads = Heads::default();
        for row in find.list()?.values() {
            let waits = [millis(row, "nextAttemptDate"), millis(row, "leaseExpiresDate")]
                .into_iter()
                .flatten()
                .filter(|at| *at > now)
                .max();
            match waits {
                Some(at) => heads.next_due = Some(heads.next_due.map_or(at, |next| next.min(at))),
                None => heads.due.push(queue_item(row)),
            }
        }
        Ok(heads)
    })
}

/// Hold `item` for a delivery by `owner` when it is still pending, at the attempt count read, and not held by another;
/// its attempt count goes up by one. The item with its payload, or `None` when another holds it.
pub fn claim(facade: &EntityFacade, item: &QueueItem, owner: &str, now: i64) -> Result<Option<QueueItem>, Error> {
    let claimed = transaction(facade, TransactionAttribute::ForceNew, || {
        facade
            .find(QUEUE_ITEM)?
            .eq("queueItemId", Value::string(&item.queue_item_id))
            .eq("statusEnumId", Value::string(PENDING))
            .eq("attemptCount", Value::I64(i64::from(item.attempts)))
            .condition(Condition::or(vec![
                Condition::is_null("leaseExpiresDate"),
                Condition::less_equal("leaseExpiresDate", stamp_value(now)),
            ]))
            .disable_authz()
            .update_all([
                ("attemptCount".to_string(), Value::I64(i64::from(item.attempts) + 1)),
                ("leaseOwner".to_string(), Value::string(owner)),
                ("leaseExpiresDate".to_string(), stamp_value(now + LEASE_MILLIS)),
                ("lastAttemptDate".to_string(), stamp_value(now)),
            ])
    })?;
    if claimed != 1 {
        return Ok(None);
    }
    let body = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade
            .find(QUEUE_ITEM)?
            .eq("queueItemId", Value::string(&item.queue_item_id))
            .select_field("payload")
            .select_field("contentLocation")
            .disable_authz()
            .one()
    })?;
    let mut claimed = item.clone();
    if let Some(body) = body {
        claimed.payload = text(&body, "payload");
        claimed.content_location = text(&body, "contentLocation");
    }
    Ok(Some(claimed))
}

/// The content of a claimed item, to send before its delivery: `None` when it has none. Only this node's own copy
/// (under [`CONTENT_OUT`]) is read.
pub fn read_content(facade: &EntityFacade, item: &QueueItem) -> Result<Option<Vec<u8>>, Error> {
    let location = item.content_location.trim();
    if location.is_empty() {
        return Ok(None);
    }
    if !location.starts_with(CONTENT_OUT) {
        return Err(Error::new(format!(
            "the content of edge queue item {} is at {location}, not under {CONTENT_OUT}",
            item.queue_item_id
        )));
    }
    let bytes = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade.read_db_resource_bytes(location)
    })?;
    bytes.map(Some).ok_or_else(|| {
        Error::new(format!(
            "the content of edge queue item {}, {location}, is not there",
            item.queue_item_id
        ))
    })
}

/// One path segment of a content location: letters, digits, `-`, `_` and `.`, not only dots.
fn segment(text: &str) -> Option<&str> {
    let ok = !text.is_empty()
        && text.len() <= 100
        && text.chars().any(|ch| ch != '.')
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    ok.then_some(text)
}

/// Where this node keeps the content local node `node`'s link sent for item `queue_item_id`: under [`CONTENT_IN`],
/// each upload at this prefix with a suffix of its own, so a delivery that ended late cannot remove a newer upload.
pub fn received_location(node: &str, queue_item_id: &str) -> Result<String, Error> {
    match (segment(node), segment(queue_item_id)) {
        (Some(node), Some(id)) => Ok(format!("{CONTENT_IN}{node}/{id}.")),
        _ => Err(Error::new(format!(
            "content for edge queue item {} of node {} cannot be kept: an id has a character other than a letter, a \
             digit, '-', '_' or '.'",
            queue_item_id.chars().take(80).collect::<String>(),
            node.chars().take(80).collect::<String>()
        ))),
    }
}

/// Keep the content the other end sent for item `queue_item_id` before its delivery, as an upload of its own; the
/// location the delivery names.
pub fn store_received(facade: &EntityFacade, node: &str, queue_item_id: &str, content: &[u8]) -> Result<String, Error> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(Error::new(format!(
            "an edge queue item's content is at most {MAX_CONTENT_BYTES} bytes, and this one is {}",
            content.len()
        )));
    }
    let location = format!(
        "{}{}",
        received_location(node, queue_item_id)?,
        crate::entity::uuid_v4()?
    );
    transaction(facade, TransactionAttribute::ForceNew, || {
        facade.write_db_resource(&location, content)
    })?;
    Ok(location)
}

/// Whether `location` is an upload this node kept for item `queue_item_id` of node `node` ([`store_received`]).
fn is_received(node: &str, queue_item_id: &str, location: &str) -> bool {
    let Ok(prefix) = received_location(node, queue_item_id) else {
        return false;
    };
    location
        .strip_prefix(&prefix)
        .is_some_and(|suffix| segment(suffix).is_some())
}

/// Remove the content at `location`, logged with `what` when it fails: true when it is gone.
fn remove_content(facade: &EntityFacade, location: &str, what: &str) -> bool {
    match transaction(facade, TransactionAttribute::ForceNew, || {
        facade.delete_db_resource(location)
    }) {
        Ok(_) => true,
        Err(err) => {
            tracing::error!("The content of {what}, {location}, was not removed: {err}");
            false
        }
    }
}

/// Let go of the leases on `node`'s items going `direction`: a delivery starting on a new link takes over from one an
/// ended link cut off, whose items would otherwise wait for their leases to run out. One delivery runs per node and
/// direction at a time.
pub fn release_leases(facade: &EntityFacade, node: &str, direction: &str) -> Result<i64, Error> {
    transaction(facade, TransactionAttribute::ForceNew, || {
        facade
            .find(QUEUE_ITEM)?
            .eq("pairedNodeId", Value::string(node))
            .eq("directionEnumId", Value::string(direction))
            .eq("statusEnumId", Value::string(PENDING))
            .condition(Condition::is_not_null("leaseOwner"))
            .select_field("queueItemId")
            .disable_authz()
            .update_all([
                ("leaseOwner".to_string(), Value::Null),
                ("leaseExpiresDate".to_string(), Value::Null),
            ])
    })
}

/// Fail every pending item going `direction` with `reason`, kept for inspection the retention period: on a local node
/// that unpairs, its outbox, which is not for whichever edge node it pairs with next. Each kind's failed-service is
/// told. The count failed.
pub fn fail_pending(
    facade: &EntityFacade,
    settings: &EdgeSettings,
    direction: &str,
    reason: &str,
    now: i64,
) -> Result<i64, Error> {
    let told: Vec<String> = if settings.failed.is_empty() {
        Vec::new()
    } else {
        transaction(facade, TransactionAttribute::UseOrBegin, || {
            Ok(facade
                .find(QUEUE_ITEM)?
                .eq("directionEnumId", Value::string(direction))
                .eq("statusEnumId", Value::string(PENDING))
                .condition(Condition::is_in(
                    "kind",
                    settings.failed.keys().map(Value::string).collect(),
                ))
                .select_field("queueItemId")
                .disable_authz()
                .list()?
                .values()
                .iter()
                .map(|row| text(row, "queueItemId"))
                .collect())
        })?
    };
    let failed = transaction(facade, TransactionAttribute::ForceNew, || {
        facade
            .find(QUEUE_ITEM)?
            .eq("directionEnumId", Value::string(direction))
            .eq("statusEnumId", Value::string(PENDING))
            .select_field("queueItemId")
            .disable_authz()
            .update_all([
                ("statusEnumId".to_string(), Value::string(FAILED)),
                ("failedDate".to_string(), stamp_value(now)),
                ("lastErrorText".to_string(), Value::string(reason)),
                ("nextAttemptDate".to_string(), Value::Null),
                ("leaseOwner".to_string(), Value::Null),
                ("leaseExpiresDate".to_string(), Value::Null),
            ])
    })?;
    for id in told {
        notify_failed(facade, settings, &id, reason);
    }
    Ok(failed)
}

/// Tell the failed-service of item `id`'s kind (`queue-kind` `failed-service`), on this sender, that it failed for
/// good with `reason`, so the failure is not silent. A kind without one, or a call that fails, is logged.
fn notify_failed(facade: &EntityFacade, settings: &EdgeSettings, id: &str, reason: &str) {
    if settings.failed.is_empty() {
        return;
    }
    let row = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade
            .find(QUEUE_ITEM)?
            .eq("queueItemId", Value::string(id))
            .select_field("kind")
            .select_field("conversationKey")
            .select_field("payload")
            .disable_authz()
            .one()
    });
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(err) => {
            tracing::error!("Edge queue item {id} failed for good, and was not read to tell its kind's service: {err}");
            return;
        }
    };
    let kind = text(&row, "kind");
    let Some(service) = settings.failed.get(&kind) else {
        return;
    };
    let parameters = BTreeMap::from([
        ("queueItemId".to_string(), Value::string(id)),
        ("kind".to_string(), Value::string(&kind)),
        (
            "conversationKey".to_string(),
            Value::string(text(&row, "conversationKey")),
        ),
        ("payload".to_string(), Value::string(text(&row, "payload"))),
        ("error".to_string(), Value::string(reason)),
    ]);
    let told = ExecutionContext::with_current(|ctx| {
        let _authz = ctx.disable_authz();
        let called = crate::service::sync(service, &parameters).map_err(|err| err.message().to_string());
        match (called, crate::service::util::take_errors_string()) {
            (Err(err), _) | (Ok(_), Some(err)) => Err(err),
            (Ok(_), None) => Ok(()),
        }
    });
    match told {
        Some(Ok(())) => {}
        Some(Err(err)) => tracing::error!("Edge queue item {id} ({kind}) failed for good; {service} failed: {err}"),
        None => tracing::error!("Edge queue item {id} ({kind}) failed for good; {service} was not called: no context"),
    }
}

/// What [`finish`] did with a claimed item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Delivered,
    /// It waits for a retry (refused, paused, unavailable, not handled, or no answer).
    Retry,
    /// It failed for good.
    Failed,
}

/// Record the answer to the delivery of `item` (claimed, so its attempt count is `item.attempts + 1`).
pub fn finish(
    facade: &EntityFacade,
    settings: &EdgeSettings,
    item: &QueueItem,
    outcome: &Outcome,
    now: i64,
) -> Result<Step, Error> {
    let attempt = item.attempts + 1;
    // an answer that is not counted: the attempt goes back, and the item goes again after `wait`
    let uncounted = |error: String, wait: Duration| {
        (
            Step::Retry,
            vec![
                ("attemptCount", Value::I64(i64::from(item.attempts))),
                ("lastErrorText", Value::string(error)),
                ("nextAttemptDate", stamp_value(now + duration_millis(wait))),
            ],
        )
    };
    let (step, fields): (Step, Vec<(&str, Value)>) = match outcome {
        Outcome::Answered(Applied::Applied | Applied::Duplicate) => (
            Step::Delivered,
            vec![
                ("statusEnumId", Value::string(DELIVERED)),
                ("deliveredDate", stamp_value(now)),
                ("nextAttemptDate", Value::Null),
            ],
        ),
        Outcome::Answered(Applied::Failed(error)) if attempt >= settings.max_attempts => {
            tracing::error!(
                "Edge queue item {} ({} for {}) failed for good after {attempt} attempts: {error}",
                item.queue_item_id,
                item.kind,
                item.conversation_key
            );
            (
                Step::Failed,
                vec![
                    ("statusEnumId", Value::string(FAILED)),
                    ("failedDate", stamp_value(now)),
                    ("lastErrorText", Value::string(error)),
                    ("nextAttemptDate", Value::Null),
                ],
            )
        }
        Outcome::Answered(Applied::Failed(error)) => {
            let wait = settings.retry_after(attempt);
            tracing::warn!(
                "Edge queue item {} ({} for {}) refused, attempt {attempt}; again in {wait:?}: {error}",
                item.queue_item_id,
                item.kind,
                item.conversation_key
            );
            (
                Step::Retry,
                vec![
                    ("lastErrorText", Value::string(error)),
                    ("nextAttemptDate", stamp_value(now + duration_millis(wait))),
                ],
            )
        }
        Outcome::Answered(Applied::NotHandled(error)) => {
            tracing::warn!(
                "Edge queue item {} ({} for {}) is not handled by the receiver; again in {MAX_RETRY:?}: {error}",
                item.queue_item_id,
                item.kind,
                item.conversation_key
            );
            uncounted(error.clone(), MAX_RETRY)
        }
        Outcome::Answered(Applied::Paused) => {
            uncounted("the receiver's delivery is paused".to_string(), settings.retry_first)
        }
        // a receiver that cannot take it now (a handler's retryLater: a relay or a database away) is asked again after
        // a wait that doubles with each such answer in a row, up to the cap, still not counted
        Outcome::Answered(Applied::Unavailable(error)) => {
            let waits = item.waits.saturating_add(1);
            let (step, mut fields) = uncounted(error.clone(), settings.retry_after(waits));
            fields.push(("waitCount", Value::I64(i64::from(waits))));
            (step, fields)
        }
        Outcome::Lost(error) => uncounted(error.clone(), settings.retry_first),
    };
    let mut fields: Vec<(String, Value)> = fields
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
    if !matches!(outcome, Outcome::Answered(Applied::Unavailable(_))) {
        // any other answer ends a run of unavailable ones
        fields.push(("waitCount".to_string(), Value::I64(0)));
    }
    fields.push(("leaseOwner".to_string(), Value::Null));
    fields.push(("leaseExpiresDate".to_string(), Value::Null));
    transaction(facade, TransactionAttribute::ForceNew, || {
        // only while the row is still this attempt's
        facade
            .find(QUEUE_ITEM)?
            .eq("queueItemId", Value::string(&item.queue_item_id))
            .eq("statusEnumId", Value::string(PENDING))
            .eq("attemptCount", Value::I64(i64::from(attempt)))
            .select_field("queueItemId")
            .disable_authz()
            .update_all(fields)?;
        Ok(())
    })?;
    if step == Step::Failed {
        let reason = match outcome {
            Outcome::Answered(Applied::Failed(error)) => error.as_str(),
            _ => "failed",
        };
        notify_failed(facade, settings, &item.queue_item_id, reason);
    }
    Ok(step)
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Apply an item from the other end of the link on this node (a request worker with a fresh context), for local node
/// `paired_node_id`: the receipt is inserted first (a concurrent apply of the same item waits on its key, then finds
/// it), then the kind's handler service runs as `_EDGE_`, with authorization, in the same transaction; an error the
/// handler adds rolls both back. A receipt that is there answers [`Applied::Duplicate`]. The handler gets the
/// paired node's id, and the item's content where this node kept it ([`store_received`]); the content is removed
/// once the delivery is answered.
pub fn apply(factory: &ExecutionContextFactory, paired_node_id: &str, item: &QueueItem) -> Applied {
    let Some(facade) = factory.entity() else {
        return Applied::Unavailable("the entity facade is not installed".to_string());
    };
    if item.content_location.is_empty() {
        return apply_item(factory, &facade, paired_node_id, item);
    }
    // only the content this node kept for this item: a sender cannot name another of this node's files
    if !is_received(paired_node_id, &item.queue_item_id, &item.content_location) {
        return Applied::Failed(format!(
            "the content location of edge queue item {} is not where this node keeps its content",
            item.queue_item_id
        ));
    }
    let applied = apply_item(factory, &facade, paired_node_id, item);
    remove_content(
        &facade,
        &item.content_location,
        &format!("edge queue item {}", item.queue_item_id),
    );
    applied
}

fn apply_item(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    paired_node_id: &str,
    item: &QueueItem,
) -> Applied {
    let facade = facade.clone();
    if paused(&facade) {
        return Applied::Paused;
    }
    let settings = super::settings(factory);
    let Some(handler) = settings.kinds.get(&item.kind).cloned() else {
        return Applied::NotHandled(format!("this node has no handler for queue kind {}", item.kind));
    };
    if !crate::service::is_service_defined(&handler, Some(&facade)) {
        return Applied::NotHandled(format!(
            "the handler {handler} of queue kind {} is not defined here",
            item.kind
        ));
    }
    if let Err(err) = crate::user::internal_login_user(super::delegate::EDGE_USERNAME) {
        tracing::error!("Edge queue: the delegate user could not log in: {err}");
        return Applied::Unavailable("this node's delegate user is not available".to_string());
    }
    let direction = inbound(&settings);
    let mut kind = ErrorKind::General;
    let applied = facade.transactions_for(RECEIPT).and_then(|tx| {
        tx.run(TransactionAttribute::ForceNew, Some(Duration::from_secs(60)), || {
            {
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                let there = facade
                    .find(RECEIPT)?
                    .eq("pairedNodeId", Value::string(paired_node_id))
                    .eq("queueItemId", Value::string(&item.queue_item_id))
                    .select_field("queueItemId")
                    .disable_authz()
                    .one()?;
                if there.is_some() {
                    return Ok(Applied::Duplicate);
                }
                let mut row = facade.make_value(RECEIPT)?;
                row.set("pairedNodeId", Value::string(paired_node_id));
                row.set("queueItemId", Value::string(&item.queue_item_id));
                row.set("directionEnumId", Value::string(direction));
                row.set("kind", Value::string(&item.kind));
                row.set("conversationKey", Value::string(&item.conversation_key));
                row.set("idempotencyKey", Value::string(&item.idempotency_key));
                row.set("appliedDate", stamp_value(now_millis()));
                if !row.create_if_missing(&facade)? {
                    return Ok(Applied::Duplicate);
                }
            }
            let parameters = BTreeMap::from([
                ("queueItemId".to_string(), Value::string(&item.queue_item_id)),
                ("pairedNodeId".to_string(), Value::string(paired_node_id)),
                ("kind".to_string(), Value::string(&item.kind)),
                ("conversationKey".to_string(), Value::string(&item.conversation_key)),
                ("idempotencyKey".to_string(), Value::string(&item.idempotency_key)),
                ("payload".to_string(), Value::string(&item.payload)),
                ("contentLocation".to_string(), Value::string(&item.content_location)),
            ]);
            let out = match crate::service::sync(&handler, &parameters) {
                Ok(out) => out,
                Err(err) => {
                    kind = err.kind();
                    return Err(Error::new(err.message().to_string()));
                }
            };
            if let Some(errors) = crate::service::util::take_errors_string() {
                return Err(Error::new(errors));
            }
            // the handler cannot apply it now for a reason that is not the item's (its relay or database is away):
            // rolled back, and the item waits, not counted, until the retention period
            let retry_later = out.get("retryLater").map(to_plain_string).unwrap_or_default();
            if !retry_later.trim().is_empty() {
                kind = ErrorKind::Unavailable;
                return Err(Error::new(retry_later));
            }
            Ok(Applied::Applied)
        })
    });
    match applied {
        Ok(applied) => applied,
        Err(err) => {
            tracing::warn!(
                "Edge queue item {} ({} for {}) was not applied: {err}",
                item.queue_item_id,
                item.kind,
                item.conversation_key
            );
            match kind {
                ErrorKind::Unauthorized | ErrorKind::AuthenticationRequired | ErrorKind::NotFound => {
                    Applied::NotHandled(err.message().to_string())
                }
                ErrorKind::Unavailable => Applied::Unavailable(err.message().to_string()),
                _ => Applied::Failed(err.message().to_string()),
            }
        }
    }
}

/// The items of one node, direction and kind, for the status screens and the metrics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KindStats {
    pub paired_node_id: String,
    pub direction: String,
    pub kind: String,
    pub pending: i64,
    pub failed: i64,
    /// When the oldest pending item was queued (epoch milliseconds).
    pub oldest_pending: Option<i64>,
}

/// The queue's depth by node, direction and kind (of one node when `paired_node_id` is given), from one grouped count.
pub fn stats(facade: &EntityFacade, paired_node_id: Option<&str>) -> Result<Vec<KindStats>, Error> {
    let rows = transaction(facade, TransactionAttribute::UseOrBegin, || {
        let mut find = facade
            .find(COUNT)?
            .condition(Condition::is_in(
                "statusEnumId",
                vec![Value::string(PENDING), Value::string(FAILED)],
            ))
            .select_field("pairedNodeId")
            .select_field("directionEnumId")
            .select_field("kind")
            .select_field("statusEnumId")
            .select_field("itemCount")
            .select_field("oldestCreatedDate")
            .disable_authz();
        if let Some(node) = paired_node_id {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        find.list()
    })?;
    let mut by_kind: BTreeMap<(String, String, String), KindStats> = BTreeMap::new();
    for row in rows.values() {
        let key = (
            text(row, "pairedNodeId"),
            text(row, "directionEnumId"),
            text(row, "kind"),
        );
        let stats = by_kind.entry(key.clone()).or_insert_with(|| KindStats {
            paired_node_id: key.0,
            direction: key.1,
            kind: key.2,
            ..KindStats::default()
        });
        let count = number(row, "itemCount");
        if text(row, "statusEnumId") == PENDING {
            stats.pending = count;
            stats.oldest_pending = millis(row, "oldestCreatedDate");
        } else {
            stats.failed = count;
        }
    }
    Ok(by_kind.into_values().collect())
}

/// What a [`clean`] pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    /// Delivered and failed items removed for their age.
    pub removed: i64,
    pub receipts_removed: i64,
    /// Pending items failed for their age or for the size cap.
    pub expired: i64,
}

/// Remove the rows `find` matches, a page at a time (its key only), until none is left.
fn remove_all(
    facade: &EntityFacade,
    entity: &str,
    pk: &[&str],
    condition: impl Fn() -> Condition,
) -> Result<i64, Error> {
    let mut removed = 0;
    loop {
        let page = transaction(facade, TransactionAttribute::ForceNew, || {
            let mut find = facade
                .find(entity)?
                .condition(condition())
                .limit(CLEAN_PAGE)
                .disable_authz();
            for field in pk {
                find = find.select_field(field);
            }
            find.delete_all()
        })?;
        removed += page;
        if page < CLEAN_PAGE {
            return Ok(removed);
        }
    }
}

/// Fail a pending item with `reason`, logged with its name: true when it was still pending.
fn expire(facade: &EntityFacade, row: &EntityValue, reason: &str, now: i64) -> bool {
    let id = text(row, "queueItemId");
    let failed = transaction(facade, TransactionAttribute::ForceNew, || {
        facade
            .find(QUEUE_ITEM)?
            .eq("queueItemId", Value::string(&id))
            .eq("statusEnumId", Value::string(PENDING))
            .select_field("queueItemId")
            .disable_authz()
            .update_all([
                ("statusEnumId".to_string(), Value::string(FAILED)),
                ("failedDate".to_string(), stamp_value(now)),
                ("lastErrorText".to_string(), Value::string(reason)),
                ("leaseOwner".to_string(), Value::Null),
                ("leaseExpiresDate".to_string(), Value::Null),
            ])
    });
    match failed {
        Ok(count) => {
            tracing::error!(
                "Edge queue item {id} ({} for {}, node {}) failed: {reason}",
                text(row, "kind"),
                text(row, "conversationKey"),
                text(row, "pairedNodeId")
            );
            count > 0
        }
        Err(err) => {
            tracing::error!("Edge queue item {id} could not be failed: {err}");
            false
        }
    }
}

/// The pending items, oldest first after sequence `after`, a page at a time, without payloads.
fn pending_page(facade: &EntityFacade, after: i64, condition: Option<Condition>) -> Result<Vec<EntityValue>, Error> {
    transaction(facade, TransactionAttribute::UseOrBegin, || {
        let mut find = facade
            .find(QUEUE_ITEM)?
            .eq("statusEnumId", Value::string(PENDING))
            .condition(Condition::greater("sequenceNum", Value::I64(after)))
            .select_field("queueItemId")
            .select_field("pairedNodeId")
            .select_field("kind")
            .select_field("conversationKey")
            .select_field("payloadSize")
            .select_field("sequenceNum")
            .order_by("sequenceNum", false)
            .limit(CLEAN_PAGE)
            .disable_authz();
        if let Some(condition) = condition {
            find = find.condition(condition);
        }
        Ok(find.list()?.values().to_vec())
    })
}

/// Remove the items `condition` matches that have content, with their content, a page at a time; a content that is
/// not removed is logged and its item goes anyway. The count of items removed.
fn remove_with_content(facade: &EntityFacade, condition: impl Fn() -> Condition) -> Result<i64, Error> {
    let mut removed = 0;
    loop {
        let page = transaction(facade, TransactionAttribute::UseOrBegin, || {
            Ok(facade
                .find(QUEUE_ITEM)?
                .condition(condition())
                .condition(Condition::is_not_null("contentLocation"))
                .select_field("queueItemId")
                .select_field("contentLocation")
                .limit(CLEAN_PAGE)
                .disable_authz()
                .list()?
                .values()
                .to_vec())
        })?;
        if page.is_empty() {
            return Ok(removed);
        }
        let mut ids = Vec::with_capacity(page.len());
        for row in &page {
            let id = text(row, "queueItemId");
            remove_content(facade, &text(row, "contentLocation"), &format!("edge queue item {id}"));
            ids.push(Value::string(id));
        }
        removed += transaction(facade, TransactionAttribute::ForceNew, || {
            facade
                .find(QUEUE_ITEM)?
                .condition(Condition::is_in("queueItemId", ids))
                .disable_authz()
                .delete_all()
        })?;
    }
}

/// Remove the content kept for deliveries that were never answered (the link dropped between the content and its
/// delivery, and the item was not sent again): what is under [`CONTENT_IN`] and was last written before `cutoff`.
fn remove_stale_received(facade: &EntityFacade, cutoff: i64) -> Result<i64, Error> {
    let root = CONTENT_IN.trim_end_matches('/');
    let nodes = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade.read_db_resource(root)
    })?;
    let mut removed = 0;
    for node in nodes.children.iter().filter(|child| !child.is_file) {
        let directory = format!("{CONTENT_IN}{}", node.name);
        let files = match transaction(facade, TransactionAttribute::UseOrBegin, || {
            facade.read_db_resource(&directory)
        }) {
            Ok(files) => files,
            Err(err) => {
                tracing::error!("Edge queue retention: {directory} was not read: {err}");
                continue;
            }
        };
        for file in files.children.iter().filter(|child| child.is_file) {
            let location = format!("{directory}/{}", file.name);
            let stat = transaction(facade, TransactionAttribute::UseOrBegin, || {
                facade.stat_db_resource(&location)
            });
            let stale = match stat {
                Ok(Some(stat)) => stat.last_updated.is_none_or(|at| at < cutoff),
                Ok(None) => false,
                Err(err) => {
                    tracing::error!("Edge queue retention: {location} was not read: {err}");
                    false
                }
            };
            if stale && remove_content(facade, &location, "an undelivered edge queue item") {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// The retention pass. Delivered items whose delivery, and failed items whose failure, is older than the retention
/// period go with their content, and so do receipts older than it (both nodes should keep the same period, so a
/// receipt outlives every redelivery of its item), and received content older than it. A pending item older than the
/// period fails, and so do the oldest pending ones while the pending payloads and content pass the size cap, each with
/// an error logged naming it.
pub fn clean(facade: &EntityFacade, settings: &EdgeSettings, now: i64) -> Result<Cleaned, Error> {
    let cutoff_millis = now - i64::from(settings.retention_days) * 86_400_000;
    let cutoff = stamp_value(cutoff_millis);
    let old = || {
        Condition::or(vec![
            Condition::and(vec![
                Condition::equals("statusEnumId", Value::string(DELIVERED)),
                Condition::less("deliveredDate", cutoff.clone()),
            ]),
            Condition::and(vec![
                Condition::equals("statusEnumId", Value::string(FAILED)),
                Condition::less("failedDate", cutoff.clone()),
            ]),
        ])
    };
    let mut removed = remove_with_content(facade, old)?;
    removed += remove_all(facade, QUEUE_ITEM, &["queueItemId"], old)?;
    let receipts_removed = remove_all(facade, RECEIPT, &["pairedNodeId", "queueItemId"], || {
        Condition::less("appliedDate", cutoff.clone())
    })?;
    match remove_stale_received(facade, cutoff_millis) {
        Ok(0) => {}
        Ok(count) => tracing::info!("Edge queue retention: {count} undelivered contents removed"),
        Err(err) => tracing::error!("Edge queue retention: the received content was not cleaned: {err}"),
    }
    let mut expired = 0;
    // waited past the retention period
    let reason = format!(
        "undelivered after the retention period of {} days",
        settings.retention_days
    );
    let mut after = i64::MIN;
    loop {
        let page = pending_page(facade, after, Some(Condition::less("createdDate", cutoff.clone())))?;
        for row in &page {
            after = after.max(number(row, "sequenceNum"));
            if expire(facade, row, &reason, now) {
                expired += 1;
                notify_failed(facade, settings, &text(row, "queueItemId"), &reason);
            }
        }
        if page.len() < usize::try_from(CLEAN_PAGE).unwrap_or(500) {
            break;
        }
    }
    // the size cap, from one grouped sum
    let total = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade
            .find(COUNT)?
            .eq("statusEnumId", Value::string(PENDING))
            .select_field("payloadBytes")
            .disable_authz()
            .one()
    })?;
    let mut total = total.map_or(0, |row| u64::try_from(number(&row, "payloadBytes")).unwrap_or(0));
    if total > settings.max_bytes {
        let reason = format!("the waiting items passed the size cap of {} bytes", settings.max_bytes);
        let mut after = i64::MIN;
        'cap: loop {
            let page = pending_page(facade, after, None)?;
            for row in &page {
                if total <= settings.max_bytes {
                    break 'cap;
                }
                after = after.max(number(row, "sequenceNum"));
                if expire(facade, row, &reason, now) {
                    expired += 1;
                    notify_failed(facade, settings, &text(row, "queueItemId"), &reason);
                    total = total.saturating_sub(u64::try_from(number(row, "payloadSize")).unwrap_or(0));
                }
            }
            if page.len() < usize::try_from(CLEAN_PAGE).unwrap_or(500) {
                break;
            }
        }
    }
    Ok(Cleaned {
        removed,
        receipts_removed,
        expired,
    })
}

#[cfg(test)]
#[path = "queue_tests.rs"]
mod tests;
