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

//! The link's queue delivery (plans/Edge-Node.md §5.2): while a link is up, one task per direction takes the head of
//! each conversation key in order (`moqui_core::edge::queue`) and sends it, on an edge node down to the local node
//! (`LocalLink.Deliver`), on a local node up to the edge node (`Link.Deliver`). The receiver applies it with its
//! receipt and answers applied, duplicate, failed or paused; no answer (the link dropped) is not counted as an attempt.
//! An item with content sends it first, in chunks (`LocalLink.PutContent`, `Link.PutContent`), and its delivery names
//! where the receiver kept it. Each item's failure is logged with its id and the loop goes on (AGENTS.md); a panic in a
//! round is caught. Also the receiving side's handlers of content and a delivery, the queue's metrics and its
//! retention pass.

use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::FutureExt as _;
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::queue::{self, Applied, Outcome, QueueItem, Step};
use moqui_core::telemetry::Telemetry;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use tokio::sync::Notify;

use super::proto;

/// How long a delivery waits for the receiver's answer.
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest the delivery sleeps with nothing due, in case a wake was missed.
const IDLE_POLL: Duration = Duration::from_secs(2);
/// How often the retention pass runs, and the metrics' snapshot of the queue is read again.
const MAINTENANCE: Duration = Duration::from_secs(15);
/// How many maintenance rounds go between retention passes.
const CLEAN_EVERY: u32 = 40;
/// The bytes of one `PutContent` chunk, well under the link's message limit.
const CHUNK: usize = 1024 * 1024;
/// The slowest uplink a delivery with content allows for (bytes a second): the answer timeout grows by the time its
/// content takes at this rate, so a large body on a slow link is not cut off and sent again forever.
const SLOWEST_UPLINK: u64 = 32 * 1024;

/// How long a delivery with `content_size` bytes of content may take: [`ANSWER_TIMEOUT`] for the answer, and the
/// content's time at [`SLOWEST_UPLINK`].
fn delivery_timeout(content_size: usize) -> Duration {
    ANSWER_TIMEOUT + Duration::from_secs(content_size as u64 / SLOWEST_UPLINK)
}

pub(crate) fn to_proto(item: &QueueItem) -> proto::QueueItem {
    proto::QueueItem {
        queue_item_id: item.queue_item_id.clone(),
        kind: item.kind.clone(),
        conversation_key: item.conversation_key.clone(),
        idempotency_key: item.idempotency_key.clone(),
        payload: item.payload.clone(),
        content_location: item.content_location.clone(),
        attempts: item.attempts,
    }
}

pub(crate) fn from_proto(item: proto::QueueItem) -> QueueItem {
    QueueItem {
        queue_item_id: item.queue_item_id,
        kind: item.kind,
        conversation_key: item.conversation_key,
        idempotency_key: item.idempotency_key,
        payload: item.payload,
        content_location: item.content_location,
        attempts: item.attempts,
        // the sender's own count of uncounted waits; the receiver does not use it
        waits: 0,
    }
}

pub(crate) fn reply(applied: &Applied) -> proto::DeliverReply {
    use proto::deliver_reply::Outcome as Wire;
    let (outcome, error) = match applied {
        Applied::Applied => (Wire::Applied, String::new()),
        Applied::Duplicate => (Wire::Duplicate, String::new()),
        Applied::Failed(error) => (Wire::Failed, error.clone()),
        Applied::Paused => (Wire::Paused, String::new()),
        Applied::Unavailable(error) => (Wire::Unavailable, error.clone()),
        Applied::NotHandled(error) => (Wire::NotHandled, error.clone()),
    };
    proto::DeliverReply {
        outcome: outcome as i32,
        error,
    }
}

pub(crate) fn applied(reply: &proto::DeliverReply) -> Applied {
    use proto::deliver_reply::Outcome as Wire;
    match Wire::try_from(reply.outcome) {
        Ok(Wire::Applied) => Applied::Applied,
        Ok(Wire::Duplicate) => Applied::Duplicate,
        Ok(Wire::Paused) => Applied::Paused,
        Ok(Wire::Failed) => Applied::Failed(reply.error.clone()),
        Ok(Wire::Unavailable) => Applied::Unavailable(reply.error.clone()),
        Ok(Wire::NotHandled) => Applied::NotHandled(reply.error.clone()),
        // a newer receiver's answer this node does not know: not counted, tried again later
        _ => Applied::NotHandled(format!("the receiver answered outcome {}", reply.outcome)),
    }
}

/// A Deliver or PutContent call's gRPC error: a request the receiver refused for itself (too large, malformed) is a
/// counted refusal; a receiver without the call (an older version) does not handle it yet; anything else (the link, a
/// timeout) is no answer.
pub(crate) fn delivery_error(status: &tonic::Status) -> Result<Applied, String> {
    match status.code() {
        tonic::Code::ResourceExhausted | tonic::Code::InvalidArgument | tonic::Code::OutOfRange => Ok(Applied::Failed(
            format!("the receiver refused it: {}", status.message()),
        )),
        tonic::Code::Unimplemented => Ok(Applied::NotHandled(format!(
            "the receiver does not take it: {}",
            status.message()
        ))),
        _ => Err(format!("Deliver: {}", status.message())),
    }
}

/// An item's content as the chunks of a `PutContent` call on link generation `generation`.
pub(crate) fn content_chunks(
    generation: u64,
    queue_item_id: &str,
    content: &[u8],
) -> impl futures_util::Stream<Item = proto::ContentChunk> + Send + 'static {
    let total_size = content.len() as u64;
    let mut chunks: Vec<proto::ContentChunk> = content
        .chunks(CHUNK)
        .map(|data| proto::ContentChunk {
            generation,
            queue_item_id: queue_item_id.to_string(),
            data: data.to_vec(),
            total_size: 0,
        })
        .collect();
    match chunks.first_mut() {
        Some(first) => first.total_size = total_size,
        // empty content is still content: one empty chunk names the item
        None => chunks.push(proto::ContentChunk {
            generation,
            queue_item_id: queue_item_id.to_string(),
            data: Vec::new(),
            total_size: 0,
        }),
    }
    futures_util::stream::iter(chunks)
}

/// Take the content of an item local node `node`'s link sends before its delivery, and keep it on this node: the
/// location the delivery names. `generation` checks each chunk's link generation (the local node's own, on the down
/// connection). A chunk of another item, content over [`queue::MAX_CONTENT_BYTES`] or an id that cannot name a file
/// is refused for itself (a counted refusal on the sender).
pub(crate) async fn receive_content(
    factory: &Weak<ExecutionContextFactory>,
    node: &str,
    mut stream: tonic::Streaming<proto::ContentChunk>,
    generation: impl Fn(u64) -> Result<(), tonic::Status>,
) -> Result<proto::PutContentReply, tonic::Status> {
    let mut id: Option<String> = None;
    let mut content = Vec::new();
    while let Some(chunk) = stream.message().await? {
        generation(chunk.generation)?;
        match &id {
            None => {
                queue::received_location(node, &chunk.queue_item_id)
                    .map_err(|err| tonic::Status::invalid_argument(err.message().to_string()))?;
                let total = usize::try_from(chunk.total_size).unwrap_or(usize::MAX);
                if total > queue::MAX_CONTENT_BYTES {
                    return Err(tonic::Status::resource_exhausted(format!(
                        "an item's content is at most {} bytes, and this one is {total}",
                        queue::MAX_CONTENT_BYTES
                    )));
                }
                content.reserve(total);
                id = Some(chunk.queue_item_id);
            }
            Some(first) if *first != chunk.queue_item_id => {
                return Err(tonic::Status::invalid_argument(
                    "the chunks of one call are of one item",
                ));
            }
            Some(_) => {}
        }
        if content.len() + chunk.data.len() > queue::MAX_CONTENT_BYTES {
            return Err(tonic::Status::resource_exhausted(format!(
                "an item's content is at most {} bytes",
                queue::MAX_CONTENT_BYTES
            )));
        }
        content.extend_from_slice(&chunk.data);
    }
    let Some(id) = id else {
        return Err(tonic::Status::invalid_argument("no content was sent"));
    };
    let keeping = node.to_string();
    let item = id.clone();
    let kept = on_pool(factory, move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
        queue::store_received(&facade, &keeping, &item, &content)
    })
    .await
    .map_err(|err| {
        tracing::error!("Edge queue item {id} of {node}: its content was not kept: {err}");
        tonic::Status::unavailable("the content could not be kept now")
    })?;
    Ok(proto::PutContentReply { content_location: kept })
}

/// Apply a delivered item on this node's request pool, for local node `node`.
pub(crate) async fn receive(factory: &Weak<ExecutionContextFactory>, node: &str, item: QueueItem) -> Applied {
    let node = node.to_string();
    let applied = crate::worker::on_pool(factory, move |factory| Ok(queue::apply(factory, &node, &item))).await;
    // a full pool, a stopping server or a panic: not the item's fault, so not counted
    applied.unwrap_or_else(|err| Applied::Unavailable(err.message().to_string()))
}

/// The wake of this node's deliveries, registered with its entity facade once.
struct Wake(Arc<Notify>);

fn wake_of(factory: &ExecutionContextFactory) -> Arc<Notify> {
    let wake = factory.extension(|factory| {
        let notify = Arc::new(Notify::new());
        if let Some(facade) = factory.entity() {
            let weak = Arc::downgrade(&notify);
            queue::on_wake(
                &facade,
                Arc::new(move || {
                    if let Some(notify) = weak.upgrade() {
                        notify.notify_waiters();
                    }
                }),
            );
        }
        Wake(notify)
    });
    Arc::clone(&wake.0)
}

fn attributes(direction: &str, kind: &str) -> moqui_core::entity::document::Document {
    let mut attributes = moqui_core::entity::document::Document::new();
    attributes.insert("moqui.edgelink.direction".to_string(), Value::string(direction));
    attributes.insert("moqui.edgelink.kind".to_string(), Value::string(kind));
    attributes
}

async fn on_pool<T, F>(factory: &Weak<ExecutionContextFactory>, job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&ExecutionContextFactory) -> Result<T, moqui_lib::error::Error> + Send + 'static,
{
    crate::worker::on_pool_without_authz(factory, move |factory| job(factory).map_err(CoreError::from))
        .await
        .map_err(|err| err.message().to_string())
}

/// Deliver `node`'s items going `direction` through `send` until the task is dropped (its link ended). `owner` names
/// this delivery in the items' leases.
pub(crate) async fn deliver<S, F>(
    factory: Weak<ExecutionContextFactory>,
    node: String,
    direction: &'static str,
    owner: String,
    telemetry: Option<Arc<Telemetry>>,
    mut send: S,
) where
    S: FnMut(QueueItem, Option<Vec<u8>>) -> F + Send,
    F: Future<Output = Result<Applied, String>> + Send,
{
    let Some(wake) = factory.upgrade().map(|factory| wake_of(&factory)) else {
        return;
    };
    let releasing = node.clone();
    let released = on_pool(&factory, move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
        queue::release_leases(&facade, &releasing, direction)
    })
    .await;
    if let Err(err) = released {
        tracing::error!("Edge queue delivery to {node} ({direction}): the old leases were not released: {err}");
    }
    loop {
        let notified = wake.notified();
        // a panic in a round is caught and logged, and the next round goes on (docs/dev/Practices-Rust.md §8)
        let round = round(&factory, &node, direction, &owner, telemetry.as_deref(), &mut send);
        let pause = match std::panic::AssertUnwindSafe(round).catch_unwind().await {
            Ok(Ok(Some(wait))) => wait,
            Ok(Ok(None)) => continue,
            Ok(Err(err)) => {
                tracing::error!("Edge queue delivery to {node} ({direction}): {err}");
                IDLE_POLL
            }
            Err(_) => {
                tracing::error!("Edge queue delivery to {node} ({direction}) panicked; the next round goes on");
                IDLE_POLL
            }
        };
        tokio::select! {
            () = notified => {}
            () = tokio::time::sleep(pause) => {}
        }
    }
}

/// One round: every due head once. `None` when something was delivered (look again at once), else how long to wait.
async fn round<S, F>(
    factory: &Weak<ExecutionContextFactory>,
    node: &str,
    direction: &'static str,
    owner: &str,
    telemetry: Option<&Telemetry>,
    send: &mut S,
) -> Result<Option<Duration>, String>
where
    S: FnMut(QueueItem, Option<Vec<u8>>) -> F,
    F: Future<Output = Result<Applied, String>>,
{
    let lookup = node.to_string();
    let (heads, settings, paused) = on_pool(factory, move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
        let settings = moqui_core::edge::settings(factory);
        if queue::paused(&facade) {
            return Ok((queue::Heads::default(), settings, true));
        }
        let heads = queue::due_heads(&facade, &lookup, direction, moqui_core::user::now_millis())?;
        Ok((heads, settings, false))
    })
    .await?;
    if paused {
        return Ok(Some(IDLE_POLL));
    }
    let mut delivered = false;
    let mut retried = false;
    for item in heads.due {
        let id = item.queue_item_id.clone();
        let kind = item.kind.clone();
        let claim = item.clone();
        let holder = owner.to_string();
        let claimed = on_pool(factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
            queue::claim(&facade, &claim, &holder, moqui_core::user::now_millis())
        })
        .await;
        let item = match claimed {
            Ok(Some(item)) => item,
            Ok(None) => continue,
            Err(err) => {
                tracing::error!("Edge queue item {id} ({kind}) could not be claimed: {err}");
                continue;
            }
        };
        let reading = item.clone();
        let content = on_pool(factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
            queue::read_content(&facade, &reading)
        })
        .await;
        let outcome = match content {
            // the item's own content is missing or unreadable: its fault, so counted
            Err(err) => Outcome::Answered(Applied::Failed(err)),
            Ok(content) => match tokio::time::timeout(
                delivery_timeout(content.as_ref().map_or(0, Vec::len)),
                send(item.clone(), content),
            )
            .await
            {
                Ok(Ok(applied)) => Outcome::Answered(applied),
                Ok(Err(error)) => Outcome::Lost(error),
                Err(_) => Outcome::Lost("the receiver did not answer in time".to_string()),
            },
        };
        let finishing = (item, outcome, Arc::clone(&settings));
        let step = on_pool(factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
            let (item, outcome, settings) = finishing;
            queue::finish(&facade, &settings, &item, &outcome, moqui_core::user::now_millis())
        })
        .await;
        match step {
            Ok(Step::Delivered) => {
                delivered = true;
                if let Some(telemetry) = telemetry {
                    telemetry.add_counter("moqui.edgelink.queue.delivered", 1, attributes(direction, &kind));
                }
            }
            Ok(Step::Failed) => {
                if let Some(telemetry) = telemetry {
                    telemetry.add_counter("moqui.edgelink.queue.failed", 1, attributes(direction, &kind));
                }
            }
            Ok(Step::Retry) => {
                retried = true;
                if let Some(telemetry) = telemetry {
                    telemetry.add_counter("moqui.edgelink.queue.retried", 1, attributes(direction, &kind));
                }
            }
            Err(err) => tracing::error!("Edge queue item {id} ({kind}): its answer was not recorded: {err}"),
        }
    }
    if delivered {
        return Ok(None);
    }
    // an item that waits for its retry now was not in the round's heads: look again when the first retry is due
    let now = moqui_core::user::now_millis();
    let retry = retried.then_some(settings.retry_first);
    let next = heads
        .next_due
        .map(|at| Duration::from_millis(u64::try_from((at - now).max(1)).unwrap_or(1)));
    let wait = [retry, next].into_iter().flatten().min().unwrap_or(IDLE_POLL);
    Ok(Some(wait.min(IDLE_POLL)))
}

/// Whether a server's maintenance task runs.
#[derive(Default)]
struct Maintained(std::sync::atomic::AtomicBool);

/// The queue's depth and oldest item by direction and kind, as the metrics last read them.
#[derive(Default)]
struct Snapshot(Mutex<Vec<queue::KindStats>>);

/// Register the queue's gauges (`moqui.edgelink.queue.depth`, `.oldest_seconds`) and, on a local node, replication's
/// (`moqui.edgelink.replication.waiting`, `.lag_seconds`), and run the maintenance task until `shutdown`: the gauges'
/// snapshot every 15 s and the retention pass every 10 minutes. Once per server; later calls do nothing.
pub(crate) fn maintain(factory: &Arc<ExecutionContextFactory>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    // once per server: an edge node at start, a local node when it pairs (at start or later)
    let started = factory.extension(|_| Maintained::default());
    if started.0.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let telemetry = moqui_core::telemetry::for_factory(factory);
    let snapshot = Arc::new(Snapshot::default());
    let depth = Arc::downgrade(&snapshot);
    telemetry.observe_gauge_series("moqui.edgelink.queue.depth", move || {
        let Some(snapshot) = depth.upgrade() else {
            return Vec::new();
        };
        let stats = lock(&snapshot.0);
        stats
            .iter()
            .map(|kind| (attributes(&kind.direction, &kind.kind), kind.pending as f64))
            .collect()
    });
    let oldest = Arc::downgrade(&snapshot);
    telemetry.observe_gauge_series("moqui.edgelink.queue.oldest_seconds", move || {
        let Some(snapshot) = oldest.upgrade() else {
            return Vec::new();
        };
        let now = moqui_core::user::now_millis();
        let stats = lock(&snapshot.0);
        stats
            .iter()
            .filter_map(|kind| {
                let oldest = kind.oldest_pending?;
                Some((
                    attributes(&kind.direction, &kind.kind),
                    (now - oldest).max(0) as f64 / 1000.0,
                ))
            })
            .collect()
    });
    let weak = Arc::downgrade(factory);
    let replication = weak.clone();
    let set_series = move |value: fn(&moqui_core::edge::replica::SetStatus, i64) -> f64| {
        let replication = replication.clone();
        move || {
            let Some(replica) = replication
                .upgrade()
                .and_then(|factory| factory.entity())
                .and_then(|facade| moqui_core::edge::replica::replica(&facade))
            else {
                return Vec::new();
            };
            let now = moqui_core::user::now_millis();
            replica
                .status()
                .iter()
                .map(|set| {
                    let mut attributes = moqui_core::entity::document::Document::new();
                    attributes.insert("moqui.edgelink.set".to_string(), Value::string(&set.name));
                    (attributes, value(set, now))
                })
                .collect()
        }
    };
    telemetry.observe_gauge_series(
        "moqui.edgelink.replication.waiting",
        set_series(|set, _| set.waiting as f64),
    );
    telemetry.observe_gauge_series(
        "moqui.edgelink.replication.lag_seconds",
        set_series(|set, now| {
            set.waiting_since
                .map_or(0.0, |since| (now - since).max(0) as f64 / 1000.0)
        }),
    );
    tokio::spawn(async move {
        let mut rounds: u32 = 0;
        loop {
            let clean = rounds.is_multiple_of(CLEAN_EVERY);
            rounds = rounds.wrapping_add(1);
            let read = on_pool(&weak, move |factory| {
                let facade = factory
                    .entity()
                    .ok_or_else(|| moqui_lib::error::Error::new("the entity facade is not installed"))?;
                if clean {
                    let settings = moqui_core::edge::settings(factory);
                    match queue::clean(&facade, &settings, moqui_core::user::now_millis()) {
                        Ok(cleaned) if cleaned != queue::Cleaned::default() => {
                            tracing::info!("Edge queue retention: {cleaned:?}");
                        }
                        Ok(_) => {}
                        Err(err) => tracing::error!("Edge queue retention pass failed: {err}"),
                    }
                    // the chat files an edge node serves for its local nodes (MMS, E6) that ran out
                    if settings.takes_links() {
                        let removed =
                            moqui_core::service::chat_bridge::sweep_edge_media(&facade, moqui_core::user::now_millis());
                        if removed > 0 {
                            tracing::info!("Edge chat files that ran out removed: {removed}");
                        }
                    }
                    // an all-in-one node's replica copies held back for a claim of the server's own (E12): one whose
                    // claim is gone is applied now
                    if settings.all_in_one() {
                        match moqui_core::edge::replica::conflicts::recheck_conflicts(factory, &facade, None) {
                            Ok(applied) if applied > 0 => {
                                tracing::info!("Edge replica conflicts rechecked: {applied} applied");
                            }
                            Ok(_) => {}
                            Err(err) => tracing::error!("Edge replica conflicts recheck failed: {err}"),
                        }
                    }
                }
                queue::stats(&facade, None)
            })
            .await;
            match read {
                Ok(stats) => *lock(&snapshot.0) = stats,
                Err(err) => tracing::debug!("Edge queue metrics not read: {err}"),
            }
            tokio::select! {
                () = tokio::time::sleep(MAINTENANCE) => {}
                _ = shutdown.wait_for(|stopped| *stopped) => return,
            }
            if weak.strong_count() == 0 {
                return;
            }
        }
    });
}
