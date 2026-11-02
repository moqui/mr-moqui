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

//! Replication over the link (plans/Edge-Node.md §5.2), the local node's sender: while a link is up, one task sends
//! each set's batches up (`Link.Replicate`): first an empty batch at the cursor, then the keys noted since, read as
//! they are after their commits, and the whole set when the edge node asks for it or none was sent in this process
//! yet. The state (noted keys, epochs, cursors) is `moqui_core::edge::replica`; the edge node's side applies a batch
//! with `moqui_core::edge::replica::apply_batch`.

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::FutureExt as _;
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::replica::{self, Batch, BatchAnswer, Change, Replica, Work};
use tokio::sync::Notify;

use super::proto::{ReplicaChange, ReplicateRequest};
use super::LinkClient;

/// The wait after a set's batch failed (the edge node's database, a timeout), doubling with each failure in a row up
/// to [`MAX_RETRY`], before the set is tried again: a full send that keeps failing does not run again at once.
const RETRY: Duration = Duration::from_secs(1);
const MAX_RETRY: Duration = Duration::from_secs(60);
/// The longest the sender sleeps with nothing to send, in case a wake was missed.
const IDLE_POLL: Duration = Duration::from_secs(2);

pub(crate) fn to_proto(change: &Change) -> ReplicaChange {
    ReplicaChange {
        entity_name: change.entity_name.clone(),
        pk_json: change.pk_json.clone(),
        fields_json: change.fields_json.clone().unwrap_or_default(),
        deleted: change.fields_json.is_none(),
    }
}

pub(crate) fn batch_of(request: ReplicateRequest) -> Batch {
    Batch {
        set_name: request.set_name,
        epoch: request.epoch,
        base_seq: request.base_seq,
        seq: request.seq,
        snapshot: request.snapshot,
        snapshot_end: request.snapshot_end,
        changes: request
            .changes
            .into_iter()
            .map(|change| Change {
                entity_name: change.entity_name,
                pk_json: change.pk_json,
                fields_json: (!change.deleted).then_some(change.fields_json),
            })
            .collect(),
    }
}

async fn on_pool<T, F>(factory: &Weak<ExecutionContextFactory>, job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&moqui_core::entity::EntityFacade) -> Result<T, moqui_lib::error::Error> + Send + 'static,
{
    crate::worker::on_pool_without_authz(factory, move |factory| {
        let facade = factory
            .entity()
            .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
        job(&facade).map_err(CoreError::from)
    })
    .await
    .map_err(|err| err.message().to_string())
}

/// A batch to send: the set, the epoch, where it follows, and its records.
struct Outgoing<'a> {
    set: &'a str,
    epoch: &'a str,
    base_seq: u64,
    seq: u64,
    snapshot: bool,
    snapshot_end: bool,
    changes: Vec<Change>,
}

/// The local node's sender of one link: runs until the task is dropped (its link ended).
pub(crate) struct Sender {
    pub factory: Weak<ExecutionContextFactory>,
    pub replica: Arc<Replica>,
    pub client: LinkClient,
    pub timeout: Duration,
}

impl Sender {
    pub(crate) async fn run(mut self) {
        let wake = Arc::new(Notify::new());
        let weak = Arc::downgrade(&wake);
        self.replica.on_wake(Arc::new(move || {
            if let Some(wake) = weak.upgrade() {
                wake.notify_one();
            }
        }));
        self.replica.link_up();
        let sets = self.replica.sets().len();
        // per set: consecutive failures, and when it may be tried again
        let mut failures = vec![0_u32; sets];
        let mut not_before = vec![tokio::time::Instant::now(); sets];
        loop {
            let notified = wake.notified();
            let mut busy = false;
            for index in 0..sets {
                let now = tokio::time::Instant::now();
                if not_before[index] > now {
                    continue;
                }
                // a panic in a step is caught and logged, and the set is tried again (docs/dev/Practices-Rust.md §8)
                let stepped = std::panic::AssertUnwindSafe(self.step(index)).catch_unwind().await;
                let error = match stepped {
                    Ok(Ok(worked)) => {
                        busy |= worked;
                        failures[index] = 0;
                        continue;
                    }
                    Ok(Err(err)) => err,
                    Err(_) => "the step panicked".to_string(),
                };
                let name = self
                    .replica
                    .sets()
                    .get(index)
                    .map(|set| set.name.clone())
                    .unwrap_or_default();
                failures[index] = failures[index].saturating_add(1);
                let wait = RETRY.saturating_mul(1_u32 << failures[index].min(6)).min(MAX_RETRY);
                not_before[index] = now + wait;
                tracing::warn!("Edge replication of set {name}: {error}; trying again in {wait:?}");
            }
            if busy {
                continue;
            }
            let now = tokio::time::Instant::now();
            let pause = not_before
                .iter()
                .filter(|at| **at > now)
                .map(|at| *at - now)
                .min()
                .map_or(IDLE_POLL, |wait| wait.min(IDLE_POLL));
            tokio::select! {
                () = notified => {}
                () = tokio::time::sleep(pause) => {}
            }
        }
    }

    /// Send what set `index` has waiting: true when something was sent.
    async fn step(&mut self, index: usize) -> Result<bool, String> {
        let Some(set) = self.replica.sets().get(index).cloned() else {
            return Ok(false);
        };
        match self.replica.next(index) {
            Work::Idle => Ok(false),
            Work::Probe { epoch, acked } => {
                let probe = Outgoing {
                    set: &set.name,
                    epoch: &epoch,
                    base_seq: acked,
                    seq: acked,
                    snapshot: false,
                    snapshot_end: false,
                    changes: Vec::new(),
                };
                match self.send(probe).await? {
                    BatchAnswer::Applied => self.replica.acked(index, &epoch, acked),
                    BatchAnswer::Resync => {
                        tracing::info!("Edge replication of set {}: the edge node asks for it whole", set.name);
                        self.replica.resync(index);
                    }
                }
                Ok(true)
            }
            Work::Changes { epoch, acked, keys } => {
                let reading = (set.clone(), keys);
                let changes = on_pool(&self.factory, move |facade| {
                    replica::read_changes(facade, &reading.0, &reading.1)
                })
                .await;
                let changes = match changes {
                    Ok(changes) => changes,
                    Err(err) => {
                        self.replica.put_back(index);
                        return Err(err);
                    }
                };
                // a large batch goes in parts, each acknowledged before the next
                let mut seq = acked;
                for part in replica::split_by_size(changes, replica::MAX_BATCH_BYTES) {
                    let batch = Outgoing {
                        set: &set.name,
                        epoch: &epoch,
                        base_seq: seq,
                        seq: seq + 1,
                        snapshot: false,
                        snapshot_end: false,
                        changes: part,
                    };
                    match self.send(batch).await {
                        Ok(BatchAnswer::Applied) => {
                            seq += 1;
                            self.replica.advanced(index, &epoch, seq);
                        }
                        Ok(BatchAnswer::Resync) => {
                            tracing::info!("Edge replication of set {}: the edge node asks for it whole", set.name);
                            self.replica.resync(index);
                            return Ok(true);
                        }
                        Err(err) => {
                            self.replica.put_back(index);
                            return Err(err);
                        }
                    }
                }
                self.replica.acked(index, &epoch, seq);
                Ok(true)
            }
            Work::Snapshot { epoch, resyncs } => {
                match self.snapshot(&set, &epoch).await {
                    Ok(seq) => {
                        tracing::info!("Edge replication: set {} sent whole ({seq} batches)", set.name);
                        self.replica.snapshot_done(index, &epoch, seq, resyncs);
                    }
                    Err(err) => {
                        self.replica.resync(index);
                        return Err(err);
                    }
                }
                Ok(true)
            }
        }
    }

    /// Send `set` whole under `epoch`, page by page in key order (each page in parts by size), then the end: the
    /// last batch's sequence.
    async fn snapshot(&mut self, set: &moqui_core::edge::ReplicaSet, epoch: &str) -> Result<u64, String> {
        let mut seq = 0_u64;
        for entity_index in 0..set.entities.len() {
            let mut after: Option<Vec<(String, moqui_lib::object::Value)>> = None;
            loop {
                let paging = (set.clone(), after.take());
                let page = on_pool(&self.factory, move |facade| {
                    replica::snapshot_page(facade, &paging.0, entity_index, paging.1.as_deref())
                })
                .await?;
                let count = page.changes.len();
                for part in replica::split_by_size(page.changes, replica::MAX_BATCH_BYTES) {
                    seq += 1;
                    let batch = Outgoing {
                        set: &set.name,
                        epoch,
                        base_seq: seq - 1,
                        seq,
                        snapshot: true,
                        snapshot_end: false,
                        changes: part,
                    };
                    if self.send(batch).await? == BatchAnswer::Resync {
                        return Err("the edge node lost the full send's cursor".to_string());
                    }
                }
                if count < replica::BATCH {
                    break;
                }
                after = page.last;
                if after.is_none() {
                    break;
                }
            }
        }
        seq += 1;
        let end = Outgoing {
            set: &set.name,
            epoch,
            base_seq: seq - 1,
            seq,
            snapshot: true,
            snapshot_end: true,
            changes: Vec::new(),
        };
        if self.send(end).await? == BatchAnswer::Resync {
            return Err("the edge node lost the full send's cursor".to_string());
        }
        Ok(seq)
    }

    async fn send(&mut self, batch: Outgoing<'_>) -> Result<BatchAnswer, String> {
        let request = ReplicateRequest {
            set_name: batch.set.to_string(),
            epoch: batch.epoch.to_string(),
            base_seq: batch.base_seq,
            seq: batch.seq,
            snapshot: batch.snapshot,
            snapshot_end: batch.snapshot_end,
            changes: batch.changes.iter().map(to_proto).collect(),
        };
        match tokio::time::timeout(self.timeout, self.client.replicate(request)).await {
            Ok(Ok(reply)) => Ok(if reply.into_inner().resync {
                BatchAnswer::Resync
            } else {
                BatchAnswer::Applied
            }),
            Ok(Err(status)) => Err(format!("Replicate: {}", status.message())),
            Err(_) => Err("the edge node did not answer Replicate in time".to_string()),
        }
    }
}
