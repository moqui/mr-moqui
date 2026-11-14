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

//! Replication (plans/archive/Edge-Node.md §5.2): a local node copies the sets its conf names
//! (`<edge><replicate set=".."><entity name=".." fields=".."/></replicate></edge>`) to its edge node, read-only there,
//! as `moqui.edge.EdgeReplicaValue` rows (the edge node's schema keeps none of the business entities).
//!
//! **On the local node** ([`Replica`]) a write to an entity of a set notes the record's key when its transaction commits
//! (the entity write hook, as the cluster's cache invalidation; nothing on a rollback). The host's link drains the
//! noted keys of each set into a batch, reads each record as it is then (a record gone is a delete), and sends it after
//! the commit. There is no change log: the keys wait in memory while the link is down, and a set whose keys were lost
//! (this process restarted, or too many waited) is sent whole again. That is enough for the small configuration sets
//! replication carries, and needs no table or sequence on the local node.
//!
//! **The cursor.** Each full send starts an epoch (a new UUID), and each batch has a sequence in it. The edge node
//! keeps the last applied epoch and sequence of each set (`moqui.edge.EdgeReplicaCursor`), written with the batch; a
//! batch that does not follow it, as after the edge node lost its database or the local node restarted, is answered
//! with a request for a full resync. On each new link the local node sends an empty batch first, which checks the
//! cursor. A full resync writes every record under the new epoch, then removes the set's records of older ones.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[path = "replica_sender.rs"]
pub mod sender;
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::json::{canonical, canonical_sha256, to_json};
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::ReplicaSet;
use crate::context::ExecutionContext;
use crate::entity::{Condition, EntityFacade, EntityValue, FieldMap};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value, value_millis};

pub const VALUE: &str = "moqui.edge.EdgeReplicaValue";
pub const CURSOR: &str = "moqui.edge.EdgeReplicaCursor";
/// The noted keys one set keeps waiting; past it the set is sent whole again instead.
const MAX_WAITING: usize = 10_000;
/// The records one batch carries at most.
pub const BATCH: usize = 500;

/// Set once a facade of this process replicates, so the write hook of every other one returns at once.
static REPLICATING: AtomicBool = AtomicBool::new(false);

/// One record's change: its entity and canonical key JSON, and its set's fields as JSON, `None` for a delete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub entity_name: String,
    pub pk_json: String,
    pub fields_json: Option<String>,
}

/// What the link sends next for a set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Work {
    /// Nothing waits.
    Idle,
    /// An empty batch at the cursor, the first on a new link: the edge node answers resync when it has lost it.
    Probe { epoch: String, acked: u64 },
    /// The keys noted since the last batch.
    Changes {
        epoch: String,
        acked: u64,
        keys: Vec<(String, String)>,
    },
    /// The whole set, under a new epoch. `resyncs` is the set's resync count when it began: a resync asked for while
    /// it is on its way (by the operator, or for keys that overflowed) sends the set whole again after it.
    Snapshot { epoch: String, resyncs: u64 },
}

/// One set's state on the local node.
#[derive(Clone, Debug, Default)]
struct SetState {
    /// Empty until the first full send.
    epoch: String,
    acked: u64,
    snapshot_needed: bool,
    /// How many times a full send was asked for; [`Replica::snapshot_done`] clears `snapshot_needed` only when none
    /// was asked for during the send.
    resyncs: u64,
    /// Checked on this link.
    probed: bool,
    waiting: BTreeSet<(String, String)>,
    /// The keys of the batch on its way, until it is acknowledged; they wait again when it is not, or its link ends.
    in_flight: Vec<(String, String)>,
    /// When the oldest waiting key was noted.
    waiting_since: Option<i64>,
    last_ack: Option<i64>,
    last_snapshot: Option<i64>,
    /// The last full send had no records: the set was held back (`super::sip::withheld`), so the edge node's copy is
    /// empty.
    sent_empty: bool,
}

/// A set's state for the status screen and the metrics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetStatus {
    pub name: String,
    pub epoch: String,
    pub acked: u64,
    pub waiting: usize,
    pub waiting_since: Option<i64>,
    pub snapshot_needed: bool,
    pub last_ack: Option<i64>,
    pub last_snapshot: Option<i64>,
}

/// A local node's replication: its sets, and the keys noted for each.
pub struct Replica {
    sets: Vec<ReplicaSet>,
    /// The sets of each entity, by full name.
    by_entity: HashMap<String, Vec<usize>>,
    state: Mutex<Vec<SetState>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Whether this member keeps its notes: true until a claim of the sender lock says another member is the sender
    /// ([`sender::set_sender`]), whose notes it then takes in place of this one's.
    keeping: AtomicBool,
}

#[derive(Default)]
struct Slot(Option<Arc<Replica>>);

/// Start replicating `sets` from `facade`'s writes, or the replication already started there (its noted keys outlive
/// a link, and a dialer started again). `None` with no sets.
pub fn enable(facade: &EntityFacade, sets: &[ReplicaSet]) -> Option<Arc<Replica>> {
    if sets.is_empty() {
        return None;
    }
    let replica = facade.with_slot(Slot::default, |slot| {
        Arc::clone(slot.0.get_or_insert_with(|| {
            let mut by_entity: HashMap<String, Vec<usize>> = HashMap::new();
            for (index, set) in sets.iter().enumerate() {
                for entity in &set.entities {
                    let name = crate::entity::info::entity_full_name(facade, &entity.name).unwrap_or_else(|| {
                        tracing::error!("edge replicate set {}: no entity {}", set.name, entity.name);
                        entity.name.clone()
                    });
                    by_entity.entry(name).or_default().push(index);
                }
            }
            let state = sets
                .iter()
                .map(|_| SetState {
                    snapshot_needed: true,
                    ..SetState::default()
                })
                .collect();
            Arc::new(Replica {
                sets: sets.to_vec(),
                by_entity,
                state: Mutex::new(state),
                wake: Mutex::new(None),
                keeping: AtomicBool::new(true),
            })
        }))
    });
    REPLICATING.store(true, Ordering::Release);
    Some(replica)
}

/// The replication started on `facade`, if any.
pub fn replica(facade: &EntityFacade) -> Option<Arc<Replica>> {
    if !REPLICATING.load(Ordering::Acquire) {
        return None;
    }
    facade.with_slot_read(Slot::default, |slot| slot.0.clone())
}

/// The entity write hook: note the record's key for each set of the entity when the write's transaction commits.
pub(crate) fn note_write(facade: &EntityFacade, entity_name: &str, pk_fields: &[String], values: &FieldMap) {
    let Some(replica) = replica(facade) else {
        return;
    };
    let full_name = if replica.by_entity.contains_key(entity_name) {
        entity_name.to_string()
    } else {
        match crate::entity::info::entity_full_name(facade, entity_name) {
            Some(name) if replica.by_entity.contains_key(&name) => name,
            _ => return,
        }
    };
    let key = pk_json(pk_fields, values);
    let tx = facade
        .transactions_for(entity_name)
        .ok()
        .filter(|tx| tx.in_transaction());
    let Some(tx) = tx else {
        replica.note(facade, &full_name, key);
        return;
    };
    let noting = Arc::clone(&replica);
    let name = full_name.clone();
    let on = facade.clone();
    if let Err(err) = tx.on_commit(move || noting.note(&on, &name, key)) {
        // the key cannot wait for the commit: send the set whole, which reads it as it is then
        tracing::error!("edge replication: a write to {entity_name} was not noted ({err}); its set goes whole");
        replica.note(facade, &full_name, None);
    }
}

/// Stop replicating from `facade`'s writes (its server stops): the noted keys are forgotten, so the next
/// [`enable`] starts with nothing noted and each set is sent whole, as after a restart.
pub fn disable(facade: &EntityFacade) {
    sender::let_go(facade);
    facade.with_slot(Slot::default, |slot| slot.0 = None);
}

/// The canonical key JSON of a record: its key fields as text, or `None` when one is missing.
fn pk_json(pk_fields: &[String], values: &FieldMap) -> Option<String> {
    let mut key = serde_json::Map::new();
    for field in pk_fields {
        let value = values
            .get(field.as_str())
            .filter(|value| !matches!(value, Value::Null))?;
        key.insert(field.clone(), serde_json::Value::String(to_plain_string(value)));
    }
    Some(canonical(&serde_json::Value::Object(key)))
}

impl Replica {
    pub fn sets(&self) -> &[ReplicaSet] {
        &self.sets
    }

    /// Call `wake` when a key is noted or a resync is asked for.
    pub fn on_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.wake) = Some(wake);
    }

    fn wake(&self) {
        let wake = lock(&self.wake).clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Note a committed write of this member's to `entity`'s record `key` (`None`: a write whose key is not known,
    /// which sends the set whole): kept here while this member keeps its notes, and sent to the other members of its
    /// cluster, so the sender has it whichever member that is ([`sender::forward_note`]).
    fn note(&self, facade: &EntityFacade, entity: &str, key: Option<String>) {
        if !self.by_entity.contains_key(entity) {
            return;
        }
        sender::forward_note(facade, entity, key.clone());
        self.note_local(entity, key);
    }

    /// Keep a note of `entity`'s record `key` (this member's, or another member's when this one is the sender), unless
    /// this member keeps no notes now. True when it was kept.
    fn note_local(&self, entity: &str, key: Option<String>) -> bool {
        let Some(indexes) = self.by_entity.get(entity) else {
            return false;
        };
        if !self.keeping.load(Ordering::Acquire) {
            return false;
        }
        {
            let mut state = lock(&self.state);
            let now = now_millis();
            for index in indexes {
                let Some(set) = state.get_mut(*index) else { continue };
                match &key {
                    Some(key) if set.waiting.len() < MAX_WAITING => {
                        set.waiting.insert((entity.to_string(), key.clone()));
                        set.waiting_since.get_or_insert(now);
                    }
                    _ => {
                        set.snapshot_needed = true;
                        set.resyncs += 1;
                        set.waiting.clear();
                        set.waiting_since.get_or_insert(now);
                    }
                }
            }
        }
        self.wake();
        true
    }

    /// Whether this member keeps notes, as the sender does ([`sender::set_sender`]). One that stops forgets what it
    /// kept and what was on its way, and sends each set whole if it is the sender again.
    pub fn set_keeping(&self, keeping: bool) {
        if self.keeping.swap(keeping, Ordering::AcqRel) == keeping || keeping {
            self.wake();
            return;
        }
        for set in lock(&self.state).iter_mut() {
            set.waiting.clear();
            set.in_flight.clear();
            set.waiting_since = None;
            set.snapshot_needed = true;
            set.resyncs += 1;
        }
        self.wake();
    }

    /// Whether this member keeps notes now.
    pub fn keeps_notes(&self) -> bool {
        self.keeping.load(Ordering::Acquire)
    }

    /// A new link: each set's cursor is checked again before its next batch, and a batch an ended link left
    /// unacknowledged waits again.
    pub fn link_up(&self) {
        let now = now_millis();
        for set in lock(&self.state).iter_mut() {
            set.probed = false;
            if !set.in_flight.is_empty() {
                set.waiting_since.get_or_insert(now);
                let keys = std::mem::take(&mut set.in_flight);
                set.waiting.extend(keys);
            }
        }
    }

    /// What to send next for set `index`. The keys of [`Work::Changes`] are on their way until [`Replica::acked`];
    /// [`Replica::put_back`] or the next [`Replica::link_up`] makes them wait again.
    pub fn next(&self, index: usize) -> Work {
        let mut state = lock(&self.state);
        let Some(set) = state.get_mut(index) else {
            return Work::Idle;
        };
        if set.snapshot_needed || set.epoch.is_empty() {
            set.waiting.clear();
            set.in_flight.clear();
            set.waiting_since = None;
            return match crate::entity::uuid_v4() {
                Ok(epoch) => Work::Snapshot {
                    epoch,
                    resyncs: set.resyncs,
                },
                Err(err) => {
                    tracing::error!("edge replication: no epoch for a full send: {err}");
                    Work::Idle
                }
            };
        }
        if !set.probed {
            return Work::Probe {
                epoch: set.epoch.clone(),
                acked: set.acked,
            };
        }
        if set.waiting.is_empty() {
            return Work::Idle;
        }
        let mut keys = Vec::with_capacity(BATCH.min(set.waiting.len()));
        while keys.len() < BATCH {
            let Some(key) = set.waiting.pop_first() else { break };
            keys.push(key);
        }
        if set.waiting.is_empty() {
            set.waiting_since = None;
        }
        set.in_flight.clone_from(&keys);
        Work::Changes {
            epoch: set.epoch.clone(),
            acked: set.acked,
            keys,
        }
    }

    /// The edge node applied part `seq` of a batch split by size, of `epoch`; the rest is still on its way.
    pub fn advanced(&self, index: usize, epoch: &str, seq: u64) {
        if let Some(set) = lock(&self.state).get_mut(index) {
            if set.epoch == epoch {
                set.acked = seq;
            }
        }
    }

    /// The edge node applied the batch `seq` of `epoch` (a probe: the cursor holds).
    pub fn acked(&self, index: usize, epoch: &str, seq: u64) {
        if let Some(set) = lock(&self.state).get_mut(index) {
            if set.epoch == epoch {
                set.acked = seq;
                set.probed = true;
                set.in_flight.clear();
                set.last_ack = Some(now_millis());
            }
        }
    }

    /// A full send of `epoch`, begun at resync count `resyncs`, finished at batch `seq`. The set is up to date unless
    /// a resync was asked for since it began.
    pub fn snapshot_done(&self, index: usize, epoch: &str, seq: u64, resyncs: u64) {
        if let Some(set) = lock(&self.state).get_mut(index) {
            set.epoch = epoch.to_string();
            set.acked = seq;
            if set.resyncs == resyncs {
                set.snapshot_needed = false;
            }
            set.probed = true;
            let now = now_millis();
            set.last_ack = Some(now);
            set.last_snapshot = Some(now);
        }
    }

    /// The last full send of set `index` had no records (`empty`), or had them: what [`Replica::hold`] compares with.
    pub fn sent_whole(&self, index: usize, empty: bool) {
        if let Some(set) = lock(&self.state).get_mut(index) {
            set.sent_empty = empty;
        }
    }

    /// Whether set `index` is to go empty now (`empty`, the set held back) or whole: when that differs from its last
    /// full send, and none is due already, a full send is due. True when it made one due.
    pub fn hold(&self, index: usize, empty: bool) -> bool {
        let due = match lock(&self.state).get_mut(index) {
            Some(set) if set.sent_empty != empty && !set.snapshot_needed && !set.epoch.is_empty() => {
                set.snapshot_needed = true;
                set.resyncs += 1;
                true
            }
            _ => false,
        };
        if due {
            self.wake();
        }
        due
    }

    /// Send set `index` whole again: the edge node asked, or the operator did.
    pub fn resync(&self, index: usize) {
        if let Some(set) = lock(&self.state).get_mut(index) {
            set.snapshot_needed = true;
            set.resyncs += 1;
        }
        self.wake();
    }

    /// Every set whole again (System › Edge Node, Resync).
    pub fn resync_all(&self) {
        for set in lock(&self.state).iter_mut() {
            set.snapshot_needed = true;
            set.resyncs += 1;
        }
        self.wake();
    }

    /// The batch on its way was not acknowledged: its keys wait again.
    pub fn put_back(&self, index: usize) {
        if let Some(set) = lock(&self.state).get_mut(index) {
            if !set.in_flight.is_empty() {
                set.waiting_since.get_or_insert(now_millis());
                let keys = std::mem::take(&mut set.in_flight);
                set.waiting.extend(keys);
            }
        }
    }

    pub fn status(&self) -> Vec<SetStatus> {
        let state = lock(&self.state);
        self.sets
            .iter()
            .zip(state.iter())
            .map(|(set, state)| SetStatus {
                name: set.name.clone(),
                epoch: state.epoch.clone(),
                acked: state.acked,
                waiting: state.waiting.len() + state.in_flight.len(),
                waiting_since: state.waiting_since,
                snapshot_needed: state.snapshot_needed || state.epoch.is_empty(),
                last_ack: state.last_ack,
                last_snapshot: state.last_snapshot,
            })
            .collect()
    }
}

fn transaction<T>(
    facade: &EntityFacade,
    entity: &str,
    attribute: TransactionAttribute,
    body: impl FnOnce() -> Result<T, Error>,
) -> Result<T, Error> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    facade
        .transactions_for(entity)?
        .run(attribute, Some(Duration::from_secs(60)), body)
}

/// The set's fields of a record, as a JSON object.
fn fields_json(row: &EntityValue, fields: &[String]) -> String {
    let object: serde_json::Map<String, serde_json::Value> = fields
        .iter()
        .map(|field| (field.clone(), row.get(field).map_or(serde_json::Value::Null, to_json)))
        .collect();
    canonical(&serde_json::Value::Object(object))
}

/// The records of `keys` as they are now, in set `set`: a record that is gone is a delete. On the local node.
pub fn read_changes(facade: &EntityFacade, set: &ReplicaSet, keys: &[(String, String)]) -> Result<Vec<Change>, Error> {
    transaction(facade, VALUE, TransactionAttribute::UseOrBegin, || {
        let mut changes = Vec::with_capacity(keys.len());
        for (entity, key) in keys {
            let Some(fields) = set.entities.iter().find(|candidate| {
                candidate.name == *entity
                    || crate::entity::info::entity_full_name(facade, &candidate.name).as_deref() == Some(entity)
            }) else {
                continue;
            };
            let Ok(serde_json::Value::Object(pk)) = serde_json::from_str::<serde_json::Value>(key) else {
                tracing::error!("edge replication: a bad key {key} of {entity} is left out");
                continue;
            };
            let mut find = facade.find(entity)?.use_cache(false).disable_authz();
            for (field, value) in &pk {
                find = find.eq(field, Value::string(value.as_str().unwrap_or_default()));
            }
            let row = find.one()?;
            changes.push(Change {
                entity_name: entity.clone(),
                pk_json: key.clone(),
                fields_json: row.map(|row| fields_json(&row, &fields.fields)),
            });
        }
        Ok(changes)
    })
}

/// A page of a full send: its records, and the key of the last, which the next page starts after.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub changes: Vec<Change>,
    /// The last record's key fields, in key order; `None` on a page with no records.
    pub last: Option<Vec<(String, Value)>>,
}

/// The records of entity `entity_index` of `set` after key `after` (from the start with `None`), in key order, at most
/// [`BATCH`], for a full send. Keyset paging: a record created or deleted during the send moves no other record from
/// its page. On the local node.
pub fn snapshot_page(
    facade: &EntityFacade,
    set: &ReplicaSet,
    entity_index: usize,
    after: Option<&[(String, Value)]>,
) -> Result<Page, Error> {
    let Some(entity) = set.entities.get(entity_index) else {
        return Ok(Page::default());
    };
    let full_name = crate::entity::info::entity_full_name(facade, &entity.name).unwrap_or_else(|| entity.name.clone());
    let pk_fields: Vec<String> = crate::entity::cache::pk_field_names(facade, &full_name)?.to_vec();
    transaction(facade, VALUE, TransactionAttribute::UseOrBegin, || {
        let mut find = facade.find(&full_name)?.use_cache(false).disable_authz();
        for field in &pk_fields {
            find = find.order_by(field, false);
        }
        if let Some(after) = after {
            find = find.condition(after_key(after));
        }
        let rows = find.limit(i64::try_from(BATCH).unwrap_or(500)).list()?;
        let last = rows.values().last().map(|row| {
            pk_fields
                .iter()
                .map(|field| (field.clone(), row.get(field).cloned().unwrap_or(Value::Null)))
                .collect()
        });
        let changes = rows
            .values()
            .iter()
            .filter_map(|row| {
                let key = pk_json(&pk_fields, row.fields())?;
                Some(Change {
                    entity_name: full_name.clone(),
                    pk_json: key,
                    fields_json: Some(fields_json(row, &entity.fields)),
                })
            })
            .collect();
        Ok(Page { changes, last })
    })
}

/// The records whose key comes after `key` in key order: `(a > x) or (a = x and b > y) or ...`.
fn after_key(key: &[(String, Value)]) -> Condition {
    let alternatives = (0..key.len())
        .map(|split| {
            let mut parts: Vec<Condition> = key
                .iter()
                .take(split)
                .map(|(field, value)| Condition::equals(field.as_str(), value.clone()))
                .collect();
            if let Some((field, value)) = key.get(split) {
                parts.push(Condition::greater(field.as_str(), value.clone()));
            }
            Condition::and(parts)
        })
        .collect();
    Condition::or(alternatives)
}

/// The most bytes of records one batch carries (well under the link's message limit); a larger batch goes in parts.
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// `changes` in parts of at most `max_bytes` each (a record larger than that alone in its part), in order.
pub fn split_by_size(changes: Vec<Change>, max_bytes: usize) -> Vec<Vec<Change>> {
    let mut parts: Vec<Vec<Change>> = Vec::new();
    let mut size = 0;
    for change in changes {
        let bytes =
            change.entity_name.len() + change.pk_json.len() + change.fields_json.as_ref().map_or(0, String::len);
        match parts.last_mut() {
            Some(part) if !part.is_empty() && size + bytes <= max_bytes => {
                size += bytes;
                part.push(change);
            }
            _ => {
                size = bytes;
                parts.push(vec![change]);
            }
        }
    }
    parts
}

/// A batch of one set as the edge node receives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub set_name: String,
    pub epoch: String,
    /// The sequence of the batch before this one (0 starts a full send).
    pub base_seq: u64,
    pub seq: u64,
    /// Part of a full send.
    pub snapshot: bool,
    /// The last part of a full send: the records of older epochs go.
    pub snapshot_end: bool,
    pub changes: Vec<Change>,
    /// When the sending member took the sender lock (`sender`, epoch milliseconds by the database's clock): the edge
    /// node refuses a batch of a set whose cursor a later sender has written, so a member that lost the lock and has
    /// not noticed yet cannot cross the new sender's batches. 0 from a sender that sends none.
    pub sender_token: u64,
}

/// The edge node's answer to a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchAnswer {
    /// Applied (or applied before), and the cursor is at it.
    Applied,
    /// It does not follow the cursor: send the set whole.
    Resync,
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

fn number(row: &EntityValue, field: &str) -> u64 {
    row.get(field)
        .map(to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

/// The batches each set had applied on this edge node, counted from its start, and how many times every set was
/// counted as changed at once ([`note_all_applied`]).
#[derive(Default)]
struct AppliedCounts(Mutex<(BTreeMap<String, u64>, u64)>);

/// How many batches of set `set_name` this edge node has applied since it started (of any node), here or on another
/// member of its edge cluster ([`note_applied`], plans/Edge-Node2.md §4.6): a reader that keeps something built from
/// the set's copies (the mail listener's recipients, the SIP engine's trunks) builds it again when the count moves.
pub fn applied_count(facade: &EntityFacade, set_name: &str) -> u64 {
    facade.with_slot_read(AppliedCounts::default, |counts| {
        let counts = lock(&counts.0);
        counts.0.get(set_name).copied().unwrap_or(0).wrapping_add(counts.1)
    })
}

/// Count a batch of `set_name` as applied here: one this member applied, or one another member of its edge cluster
/// applied to the shared copies (`edge::cluster_state`), so what is built from the set here is built again.
pub fn note_applied(facade: &EntityFacade, set_name: &str) {
    facade.with_slot(AppliedCounts::default, |counts| {
        *lock(&counts.0).0.entry(set_name.to_string()).or_insert(0) += 1;
    });
}

/// Count every set as changed: this member may have missed another member's notices (`edge::cluster_state`).
pub fn note_all_applied(facade: &EntityFacade) {
    facade.with_slot(AppliedCounts::default, |counts| {
        let mut counts = lock(&counts.0);
        counts.1 = counts.1.wrapping_add(1);
    });
}

/// Apply a batch from local node `node` on the edge node, with its cursor, in one transaction; once it commits, the
/// set's [`applied_count`] moves.
pub fn apply_batch(facade: &EntityFacade, node: &str, batch: &Batch) -> Result<BatchAnswer, Error> {
    let answer = apply_batch_rows(facade, node, batch)?;
    if answer == BatchAnswer::Applied {
        note_applied(facade, &batch.set_name);
        // the other members of an edge cluster build what they keep from this set again (plans/Edge-Node2.md §4.6)
        super::cluster_state::tell(facade, false, &[batch.set_name.as_str()]);
    }
    Ok(answer)
}

fn apply_batch_rows(facade: &EntityFacade, node: &str, batch: &Batch) -> Result<BatchAnswer, Error> {
    // a record that claims an item the server (on an all-in-one node, §10) or another pairing (Edge-Node2 §4.1)
    // holds is not applied
    let owners = conflicts::owners(facade);
    transaction(facade, VALUE, TransactionAttribute::ForceNew, || {
        let cursor = facade
            .find(CURSOR)?
            .eq("pairedNodeId", Value::string(node))
            .eq("setName", Value::string(&batch.set_name))
            .for_update(true)
            .disable_authz()
            .one()?;
        // a later sender has written this set's cursor: this batch is from a member that lost the lock
        let fence = cursor.as_ref().map_or(0, |row| number(row, "senderToken"));
        if batch.sender_token < fence {
            return Err(Error::new(format!(
                "set {}: a newer replication sender of this pairing has sent it; this member is no longer its sender",
                batch.set_name
            )));
        }
        let at = cursor
            .as_ref()
            .map(|row| (text(row, "replicaEpoch"), number(row, "sequenceNum")));
        let starts = batch.snapshot && batch.base_seq == 0;
        // a batch applied before (its answer was lost) is applied again: it carries the records as they are now
        let follows =
            |(epoch, seq): &(String, u64)| *epoch == batch.epoch && (*seq == batch.base_seq || *seq == batch.seq);
        if !starts && !at.as_ref().is_some_and(follows) {
            return Ok(BatchAnswer::Resync);
        }
        let now = stamp_value(now_millis());
        if let Some((owners, factory)) = owners.as_ref().and_then(|owners| Some((owners, owners.factory()?))) {
            conflicts::recheck_in(factory, owners, facade, Some(node), Some(&batch.set_name));
        }
        for change in &batch.changes {
            let pk: serde_json::Value = serde_json::from_str(&change.pk_json)
                .map_err(|err| Error::new(format!("a bad key in the batch: {err}")))?;
            let hash = canonical_sha256(&pk);
            if let Some(owners) = &owners {
                if conflicts::skipped(owners, facade, node, batch, change, &pk, &hash, &now)? {
                    continue;
                }
            }
            let mut row = facade.make_value(VALUE)?;
            row.set("pairedNodeId", Value::string(node));
            row.set("setName", Value::string(&batch.set_name));
            row.set("entityName", Value::string(&change.entity_name));
            row.set("pkHash", Value::string(&hash));
            match &change.fields_json {
                Some(fields) => {
                    row.set("pkJson", Value::string(canonical(&pk)));
                    row.set("fieldsJson", Value::string(fields));
                    row.set("replicaEpoch", Value::string(&batch.epoch));
                    row.set("updatedDate", now.clone());
                    row.create_or_update(facade)?;
                }
                None => {
                    facade
                        .find(VALUE)?
                        .eq("pairedNodeId", Value::string(node))
                        .eq("setName", Value::string(&batch.set_name))
                        .eq("entityName", Value::string(&change.entity_name))
                        .eq("pkHash", Value::string(&hash))
                        .disable_authz()
                        .delete_all()?;
                }
            }
        }
        if batch.snapshot_end {
            facade
                .find(VALUE)?
                .eq("pairedNodeId", Value::string(node))
                .eq("setName", Value::string(&batch.set_name))
                .condition(Condition::or(vec![
                    Condition::is_null("replicaEpoch"),
                    Condition::not_equals("replicaEpoch", Value::string(&batch.epoch)),
                ]))
                .disable_authz()
                .delete_all()?;
            if owners.is_some() {
                conflicts::drop_older(facade, node, batch)?;
            }
        }
        let mut row = facade.make_value(CURSOR)?;
        row.set("pairedNodeId", Value::string(node));
        row.set("setName", Value::string(&batch.set_name));
        row.set("replicaEpoch", Value::string(&batch.epoch));
        row.set("sequenceNum", Value::I64(i64::try_from(batch.seq).unwrap_or(i64::MAX)));
        row.set(
            "senderToken",
            Value::I64(i64::try_from(batch.sender_token.max(fence)).unwrap_or(i64::MAX)),
        );
        row.set("appliedDate", now.clone());
        if batch.snapshot_end {
            row.set("snapshotDate", now);
        }
        row.create_or_update(facade)?;
        Ok(BatchAnswer::Applied)
    })
}

/// A record of a replicated set, as the edge node keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplicaValue {
    pub paired_node_id: String,
    pub set_name: String,
    pub entity_name: String,
    pub pk: serde_json::Value,
    pub fields: serde_json::Value,
}

fn replica_value(row: &EntityValue) -> ReplicaValue {
    let json = |field: &str| serde_json::from_str(&text(row, field)).unwrap_or(serde_json::Value::Null);
    ReplicaValue {
        paired_node_id: text(row, "pairedNodeId"),
        set_name: text(row, "setName"),
        entity_name: text(row, "entityName"),
        pk: json("pkJson"),
        fields: json("fieldsJson"),
    }
}

/// On the edge node, the copy of `entity`'s record `pk` (its key fields, as text) in set `set_name`, of
/// `paired_node_id` or of any paired node.
pub fn get_value(
    facade: &EntityFacade,
    paired_node_id: Option<&str>,
    set_name: &str,
    entity: &str,
    pk: &BTreeMap<String, String>,
) -> Result<Option<ReplicaValue>, Error> {
    let key: serde_json::Map<String, serde_json::Value> = pk
        .iter()
        .map(|(field, value)| (field.clone(), serde_json::Value::String(value.clone())))
        .collect();
    let hash = canonical_sha256(&serde_json::Value::Object(key));
    let full_name = crate::entity::info::entity_full_name(facade, entity).unwrap_or_else(|| entity.to_string());
    transaction(facade, VALUE, TransactionAttribute::UseOrBegin, || {
        let mut find = facade
            .find(VALUE)?
            .eq("setName", Value::string(set_name))
            .eq("entityName", Value::string(&full_name))
            .eq("pkHash", Value::string(&hash))
            .disable_authz();
        if let Some(node) = paired_node_id {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        Ok(find.list()?.values().first().map(replica_value))
    })
}

/// On the edge node, the copies of set `set_name` (of one entity when given), of `paired_node_id` or of any paired
/// node, at most `limit`.
pub fn list_values(
    facade: &EntityFacade,
    paired_node_id: Option<&str>,
    set_name: &str,
    entity: Option<&str>,
    limit: usize,
) -> Result<Vec<ReplicaValue>, Error> {
    let full_name = entity
        .map(|entity| crate::entity::info::entity_full_name(facade, entity).unwrap_or_else(|| entity.to_string()));
    transaction(facade, VALUE, TransactionAttribute::UseOrBegin, || {
        let mut find = facade
            .find(VALUE)?
            .eq("setName", Value::string(set_name))
            .order_by("entityName", false)
            .order_by("pkHash", false)
            .limit(i64::try_from(limit).unwrap_or(i64::MAX))
            .disable_authz();
        if let Some(node) = paired_node_id {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        if let Some(name) = &full_name {
            find = find.eq("entityName", Value::string(name));
        }
        Ok(find.list()?.values().iter().map(replica_value).collect())
    })
}

/// [`list_values`] reading only each copy's key (`pkJson`, so `fieldsJson` is neither read nor decrypted): the keys of
/// set `set_name`'s copies of `entity` of `paired_node_id`, at most `limit`.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn list_keys(
    facade: &EntityFacade,
    paired_node_id: &str,
    set_name: &str,
    entity: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>, Error> {
    let full_name = crate::entity::info::entity_full_name(facade, entity).unwrap_or_else(|| entity.to_string());
    transaction(facade, VALUE, TransactionAttribute::UseOrBegin, || {
        let rows = facade
            .find(VALUE)?
            .eq("pairedNodeId", Value::string(paired_node_id))
            .eq("setName", Value::string(set_name))
            .eq("entityName", Value::string(&full_name))
            .select_field("pkJson")
            .order_by("pkHash", false)
            .limit(i64::try_from(limit).unwrap_or(i64::MAX))
            .disable_authz()
            .list()?;
        Ok(rows
            .values()
            .iter()
            .map(|row| serde_json::from_str(&text(row, "pkJson")).unwrap_or(serde_json::Value::Null))
            .collect())
    })
}

/// An edge node's cursor of one node's set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub paired_node_id: String,
    pub set_name: String,
    pub epoch: String,
    pub seq: u64,
    pub values: i64,
    pub snapshot_date: Option<i64>,
    pub applied_date: Option<i64>,
}

/// On the edge node, the cursors of the replicated sets, of one node when given, with each set's count of records.
pub fn cursors(facade: &EntityFacade, paired_node_id: Option<&str>) -> Result<Vec<Cursor>, Error> {
    transaction(facade, CURSOR, TransactionAttribute::UseOrBegin, || {
        let mut find = facade
            .find(CURSOR)?
            .order_by("pairedNodeId", false)
            .order_by("setName", false)
            .disable_authz();
        if let Some(node) = paired_node_id {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        let rows = find.list()?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows.values() {
            let node = text(row, "pairedNodeId");
            let set = text(row, "setName");
            let values = facade
                .find(VALUE)?
                .eq("pairedNodeId", Value::string(&node))
                .eq("setName", Value::string(&set))
                .disable_authz()
                .count()?;
            out.push(Cursor {
                paired_node_id: node,
                set_name: set,
                epoch: text(row, "replicaEpoch"),
                seq: number(row, "sequenceNum"),
                values,
                snapshot_date: row.get("snapshotDate").and_then(value_millis),
                applied_date: row.get("appliedDate").and_then(value_millis),
            });
        }
        Ok(out)
    })
}

#[path = "replica_conflicts.rs"]
pub mod conflicts;

#[cfg(test)]
#[path = "replica_tests.rs"]
mod tests;
