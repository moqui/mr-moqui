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

//! Replication over the link (plans/archive/Edge-Node.md §5.2), the local node's sender: while a link is up, one task sends
//! each set's batches up (`Link.Replicate`): first an empty batch at the cursor, then the keys noted since, read as
//! they are after their commits, and the whole set when the edge node asks for it or none was sent in this process
//! yet. The state (noted keys, epochs, cursors) is `moqui_core::edge::replica`; the edge node's side applies a batch
//! with `moqui_core::edge::replica::apply_batch`. A set the local node holds back (`moqui_core::edge::sip::withheld`:
//! the trunks and fallbacks while phones are off, plans/archive/Edge-Node.md §5.7 E10 phones) is sent empty, so the edge node's
//! copy is emptied, and its changes are dropped; it is sent whole again when that changes.
//!
//! **From a pool** (plans/Edge-Node2.md §4.4): every member of a clustered local node links, and one sends. Each link
//! runs [`keep_sender_lock`], which claims the lock row `moqui.edge.EdgeSenderLock` (`moqui_core::edge::replica::sender`)
//! and tells this member's link tasks whether it holds it ([`sender_role`]): only the holder's [`Sender`] sends, and
//! only its link keeps the edge node's `CallMediaNode` and `SipNode` rows and runs the minute's
//! `refresh#PhoneFallbacks` (`dial.rs`, `keep_media_node`). The others pass their notes to it on a cluster topic.

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures_util::FutureExt as _;
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::replica::{self, Batch, BatchAnswer, Change, Replica, Work};
use tokio::sync::{watch, Notify};

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
        sender_token: request.sender_token,
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

/// Whether this member is the sender, per factory, for its link tasks to wait on.
struct SenderRole(watch::Sender<Option<u64>>);

/// This member's sender role: its token ([`moqui_core::edge::replica::sender::Claim::token`]) while it holds the sender
/// lock, as the last claim found, else `None`. Each link's tasks that only the sender runs wait on it.
pub(crate) fn sender_role(factory: &ExecutionContextFactory) -> watch::Receiver<Option<u64>> {
    factory.extension(|_| SenderRole(watch::channel(None).0)).0.subscribe()
}

/// What one claim of the sender lock found: its token while this member holds it. `None` when it did not hold it, when
/// the claim failed (the database: two senders at once would cross the edge node's cursors, and the lease of a member
/// that cannot renew lapses anyway), and when it did not come back in time (`None` from the timeout), so a member whose
/// renewals stall stops sending before its lease can lapse and another can take the lock.
fn held_after(claimed: Option<&Result<moqui_core::edge::replica::sender::Claim, String>>) -> Option<u64> {
    match claimed {
        Some(Ok(claim)) => claim.token(),
        Some(Err(err)) => {
            tracing::warn!("Edge replication: the sender lock could not be claimed: {err}");
            None
        }
        None => {
            tracing::warn!("Edge replication: the sender lock's claim did not come back before the lease; this member stops sending");
            None
        }
    }
}

/// How long a holder may go without a claim coming back: its lease less a third, so it stops sending (by its own clock)
/// before another member (by the database's) can take the lock over.
fn hold_for(lease: Duration) -> Duration {
    lease.saturating_sub(lease / 3)
}

/// Claim the sender lock for this member while its link lasts (`instance_id`, its cluster member id), every
/// `SenderTiming::poll` while another holds it and every `SenderTiming::renew` (at most [`hold_for`] the lease) while
/// it does, and tell this member's link tasks ([`sender_role`]) and its replication
/// (`moqui_core::edge::replica::sender::set_sender`) what it found ([`held_after`]). A new link starts as not the sender
/// until its first claim: what an earlier link found is not taken on trust. The lock is let go when the server stops or
/// unpairs (`moqui_core::edge::replica::disable`); a link that ends keeps it until its lease lapses, so a link back soon
/// goes on where it was.
pub(crate) async fn keep_sender_lock(factory: Weak<ExecutionContextFactory>, instance_id: String) {
    let Some(role) = factory.upgrade().map(|factory| {
        moqui_core::edge::replica::sender::install_note_listener(&factory);
        factory.extension(|_| SenderRole(watch::channel(None).0))
    }) else {
        return;
    };
    role.0.send_replace(None);
    let mut held_since: Option<tokio::time::Instant> = None;
    loop {
        let Some(timing) = factory
            .upgrade()
            .map(|factory| moqui_core::edge::replica::sender::timing(&factory))
        else {
            return;
        };
        let started = tokio::time::Instant::now();
        let instance = instance_id.clone();
        let lease = timing.lease;
        let claiming = crate::worker::on_pool_without_authz(&factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
            Ok(moqui_core::edge::replica::sender::claim(
                &facade,
                moqui_core::edge::replica::sender::REPLICATE,
                &instance,
                lease,
            )
            .map_err(|err| err.to_string()))
        });
        let claimed = match held_since {
            // a holder: the claim must come back before its hold runs out
            Some(since) => tokio::time::timeout_at(since + hold_for(lease), claiming).await.ok(),
            None => Some(claiming.await),
        };
        let claimed = match claimed {
            Some(Ok(claim)) => Some(claim),
            // the server is stopping
            Some(Err(_)) => return,
            None => None,
        };
        let held = held_after(claimed.as_ref());
        if let Some(moqui_core::edge::replica::sender::Claim::Taken(_)) =
            claimed.as_ref().and_then(|claim| claim.as_ref().ok())
        {
            tracing::info!("Edge replication: member {instance_id} took the sender lock");
        }
        if let Some(facade) = factory.upgrade().and_then(|factory| factory.entity()) {
            moqui_core::edge::replica::sender::set_sender(&facade, held.map(|_| instance_id.as_str()));
        }
        held_since = held.map(|_| started);
        role.0
            .send_if_modified(|current| std::mem::replace(current, held) != held);
        let pause = if held.is_some() {
            timing.renew.min(hold_for(lease) / 2)
        } else {
            timing.poll
        };
        tokio::time::sleep(pause).await;
    }
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
    /// This member's token while it is the sender ([`sender_role`]): while it is not, nothing is sent.
    pub role: watch::Receiver<Option<u64>>,
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
        let held = |index: usize, replica: &Replica| {
            replica
                .sets()
                .get(index)
                .is_some_and(|set| moqui_core::edge::sip::withheld(&set.name))
        };
        loop {
            let notified = wake.notified();
            if self.role.borrow_and_update().is_none() {
                // another member sends: wait until this one is the sender, which then sends each set whole
                if self.role.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let mut busy = false;
            for index in 0..sets {
                // a set held back, or let go again, goes whole (empty while held back): compared with what its last
                // full send was, kept with the set's state, so no change of the setting is missed
                let holding = held(index, &self.replica);
                self.replica.hold(index, holding);
                let now = tokio::time::Instant::now();
                if not_before[index] > now {
                    continue;
                }
                // a panic in a step is caught and logged, and the set is tried again (docs/dev/Practices-Rust.md §8)
                let stepped = std::panic::AssertUnwindSafe(self.step(index, holding))
                    .catch_unwind()
                    .await;
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

    /// Send what set `index` has waiting: true when something was sent. A set `withheld` sends no records: its full
    /// send is empty and its changes are dropped.
    async fn step(&mut self, index: usize, withheld: bool) -> Result<bool, String> {
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
            Work::Changes { epoch, acked, .. } if withheld => {
                // nothing of a set held back goes: its copy there is empty
                self.replica.acked(index, &epoch, acked);
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
                match self.snapshot(&set, &epoch, withheld).await {
                    Ok(seq) if withheld => {
                        tracing::info!("Edge replication: set {} sent empty, as it is held back", set.name);
                        self.replica.sent_whole(index, true);
                        self.replica.snapshot_done(index, &epoch, seq, resyncs);
                    }
                    Ok(seq) => {
                        tracing::info!("Edge replication: set {} sent whole ({seq} batches)", set.name);
                        self.replica.sent_whole(index, false);
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
    /// last batch's sequence. With `empty`, only the end goes, which removes every record of the set there.
    async fn snapshot(&mut self, set: &moqui_core::edge::ReplicaSet, epoch: &str, empty: bool) -> Result<u64, String> {
        let mut seq = 0_u64;
        let entities = if empty { 0 } else { set.entities.len() };
        for entity_index in 0..entities {
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
            sender_token: (*self.role.borrow()).unwrap_or(0),
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
