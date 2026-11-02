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
//! after `queue-retry-seconds`) or has no handler or grant for the kind (a version skew: tried again every
//! 10 minutes). Modeled on `ChatBridgeDelivery` (`crate::service::chat_bridge`): the claim is a conditional update of
//! the attempt count and the lease.
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
/// The largest payload an item carries: 1 MiB. A larger body is refused at enqueue; `contentLocation` names a body
/// the receiver reads itself (moving such bodies over the link is later work).
pub const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
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
    pub content_location: String,
    /// The attempts made before this one.
    pub attempts: u32,
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
/// `None`), on a local node for its edge node. The kind must be one this node's conf lists (`queue-kind`), and the
/// payload at most [`MAX_PAYLOAD_BYTES`]. An idempotency key already queued returns that item.
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
    let size = item.payload.as_deref().map_or(0, str::len);
    if size > MAX_PAYLOAD_BYTES {
        return Err(Error::new(format!(
            "an edge queue item's payload is at most {MAX_PAYLOAD_BYTES} bytes, and this one is {size}; name a larger \
             body with contentLocation"
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
        row.set(
            "contentLocation",
            item.content_location.as_deref().map_or(Value::Null, Value::string),
        );
        row.set("payloadSize", Value::I64(i64::try_from(size).unwrap_or(i64::MAX)));
        row.set("statusEnumId", Value::string(PENDING));
        row.set("attemptCount", Value::I64(0));
        row.set("createdDate", stamp_value(now_millis()));
        // a concurrent enqueue of the same key made the row first: this one is that item
        row.create_if_missing(facade)
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
const HEAD_FIELDS: [&str; 8] = [
    "queueItemId",
    "kind",
    "conversationKey",
    "idempotencyKey",
    "attemptCount",
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
        Outcome::Answered(Applied::Unavailable(error)) | Outcome::Lost(error) => {
            uncounted(error.clone(), settings.retry_first)
        }
    };
    let mut fields: Vec<(String, Value)> = fields
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
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
    Ok(step)
}

fn duration_millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Apply an item from the other end of the link on this node (a request worker with a fresh context), for local node
/// `paired_node_id`: the receipt is inserted first (a concurrent apply of the same item waits on its key, then finds
/// it), then the kind's handler service runs as `_EDGE_`, with authorization, in the same transaction; an error the
/// handler adds rolls both back. A receipt that is there answers [`Applied::Duplicate`].
pub fn apply(factory: &ExecutionContextFactory, paired_node_id: &str, item: &QueueItem) -> Applied {
    let Some(facade) = factory.entity() else {
        return Applied::Unavailable("the entity facade is not installed".to_string());
    };
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
                ("kind".to_string(), Value::string(&item.kind)),
                ("conversationKey".to_string(), Value::string(&item.conversation_key)),
                ("idempotencyKey".to_string(), Value::string(&item.idempotency_key)),
                ("payload".to_string(), Value::string(&item.payload)),
                ("contentLocation".to_string(), Value::string(&item.content_location)),
            ]);
            if let Err(err) = crate::service::sync(&handler, &parameters) {
                kind = err.kind();
                return Err(Error::new(err.message().to_string()));
            }
            if let Some(errors) = crate::service::util::take_errors_string() {
                return Err(Error::new(errors));
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

/// The retention pass. Delivered items whose delivery, and failed items whose failure, is older than the retention
/// period go, and so do receipts older than it (both nodes should keep the same period, so a receipt outlives every
/// redelivery of its item). A pending item older than the period fails, and so do the oldest pending ones while the
/// pending payloads pass the size cap, each with an error logged naming it.
pub fn clean(facade: &EntityFacade, settings: &EdgeSettings, now: i64) -> Result<Cleaned, Error> {
    let cutoff_millis = now - i64::from(settings.retention_days) * 86_400_000;
    let cutoff = stamp_value(cutoff_millis);
    let removed = remove_all(facade, QUEUE_ITEM, &["queueItemId"], || {
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
    })?;
    let receipts_removed = remove_all(facade, RECEIPT, &["pairedNodeId", "queueItemId"], || {
        Condition::less("appliedDate", cutoff.clone())
    })?;
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
