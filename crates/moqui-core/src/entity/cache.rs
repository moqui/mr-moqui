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

//! Entity one-cache, the per-frame transaction cache, and the sim overlay.
//!
//! A find is stored only when the entity `cache` attribute is `true`. A transaction frame with the
//! cache on (`cache` and `force-cache`) holds writes in memory until its commit flush. Each frame
//! has its own held rows, as Java has one cache per suspended level: a suspended frame's rows are
//! not read, flushed, or dropped by the frame above it. Sim writes go to the overlay pool and stay
//! off the main schema.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use moqui_db::dialect::Dialect;
use moqui_db::model::EntityDefinition;
use moqui_db::sql::{
    create_table, delete, insert, insert_or_update, select, update, CompareOp, Condition, FieldRef, LimitOffset, Nulls,
    OrderBy,
};
use moqui_lib::cache::{CacheConfig, MCache};
use moqui_lib::error::Error;
use moqui_lib::object::{compare_like, compare_obj, to_plain_string, Value};

use super::value::FieldMap;
use super::exec::{execute_conn, execute_script, query_conn, query_tx};
use super::facade::EntityFacade;
use super::field_rw;
use super::params::{entity_columns, map_row};
use super::value::{EntityList, EntityValue};
use crate::driver::{DbConnection, DbRow};
use crate::transaction::TransactionManager;

const LIST_MAX: usize = 10_000;
const PK_SEP: char = '\u{1f}';

static SIM_SEQ: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Held rows by transaction frame token.
    static TX_HOLD: RefCell<HashMap<u64, Vec<Held>>> = RefCell::new(HashMap::new());
    /// Entities written by each open transaction frame, by frame token. Their finds skip the one-cache until the
    /// frame ends, since a find on the enlisted connection sees the frame's uncommitted rows.
    static TX_WRITTEN: RefCell<HashMap<u64, HashSet<String>>> = RefCell::new(HashMap::new());
    /// The writes of each frame, by entity: `None` once there were too many to keep. See [`FRAME_WRITES_MAX`].
    static TX_WRITES: RefCell<HashMap<u64, FrameWrites>> = RefCell::new(HashMap::new());
    static SIM: RefCell<Vec<SimSession>> = const { RefCell::new(Vec::new()) };
    /// Sim mode on this thread without a schema, which tests set through `set_sim_active`.
    static SIM_FLAG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The find `list` and `iterator` ask the cache about. Owned so a cache can store the key.
pub struct FindLookup {
    pub entity_name: String,
    /// `useCache` on the find: `Some(true)` caches any entity not marked `cache="never"`, `Some(false)` none; `None` follows
    /// the entity's `cache` attribute (Java's `shouldUseCache`).
    pub use_cache: Option<bool>,
    pub condition: Option<Condition>,
    pub order: Vec<OrderBy>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    /// The user's authorization filters narrowed the condition. Such a find is not cached, as Java's `shouldCache`.
    pub filtered: bool,
}

/// Whether the caller should run the write on the enlisted connection.
///
/// `Handled` means this module stored the write or ran it itself. Return that only when the write
/// will not be lost: a transaction cache inside an open frame, or a sim write already sent to the
/// overlay. `on_write` still runs after either variant and is the one-cache invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteGate {
    Proceed,
    Handled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Create,
    Update,
    Delete,
    Upsert,
}

#[derive(Clone)]
struct Held {
    entity_name: String,
    operation: Op,
    plain: FieldMap,
    sealed: FieldMap,
    pk: String,
}

struct SealedWrite {
    full_name: String,
    pk: String,
    sealed: FieldMap,
}

struct ReadMeta {
    full_name: String,
    is_view: bool,
    cache: bool,
    cache_never: bool,
    pk_fields: Vec<String>,
}

impl ReadMeta {
    /// Java `shouldCache` and the list find: no cache for a find with no condition, a limit or offset, or the user's
    /// authorization filters.
    fn caches(&self, lookup: &FindLookup) -> bool {
        if lookup.condition.is_none() || lookup.limit.is_some() || lookup.offset.is_some() || lookup.filtered {
            return false;
        }
        match lookup.use_cache {
            Some(true) => !self.cache_never,
            Some(false) => false,
            None => self.cache,
        }
    }
}

struct MemRow {
    fields: FieldMap,
    trusted: bool,
}

struct SimSession {
    schema: String,
    tables: HashSet<String>,
    tombstones: HashMap<String, HashSet<String>>,
    written: HashSet<String>,
    /// Java's `TransactionCacheDb.seqBanks`: the sim's own sequenced ids, so a sim uses no production ids.
    seq_banks: HashMap<String, i64>,
    /// Rows copied unchanged from production so a view read can join them with the sim's own rows, by row key. Neither
    /// written nor deleted in the sim. See [`super::sim_view`].
    replicas: HashSet<String>,
    /// Rows copied from production in this session, against [`SIM_TOTAL_COPY_CAP`].
    copied: usize,
    /// Java's `poisonedReason`: set when a transaction that wrote to the sim rolled back. Every later read or write of
    /// the sim fails with it.
    poisoned: Option<String>,
    /// The transactions (frame tokens) with a rollback hook for this sim, Java's `trackedTransactions`.
    tracked: HashSet<u64>,
}

impl SimSession {
    fn new(schema: String) -> Self {
        Self {
            schema,
            tables: HashSet::new(),
            tombstones: HashMap::new(),
            written: HashSet::new(),
            seq_banks: HashMap::new(),
            replicas: HashSet::new(),
            copied: 0,
            poisoned: None,
            tracked: HashSet::new(),
        }
    }
}

const SIM_ROLLED_BACK: &str =
    "A transaction that wrote to the sim was rolled back, and sim writes are not undone, so the sim \
     state can no longer be trusted. The sim has been terminated. Start a new sim to continue testing.";

/// [`sim_check_open`] for a find of `entity_name`, which finds call first, since a failed overlay read otherwise falls
/// back to production. The entities a sim leaves on production ([`SIM_PRODUCTION_ENTITIES`]) are not the sim's, as Java's
/// `handles`, so they are still read.
pub(super) fn sim_check_find(facade: &EntityFacade, entity_name: &str) -> Result<(), Error> {
    if !sim_session_open() {
        return Ok(());
    }
    let production = facade
        .with_entity(entity_name, |entity, _, _| {
            Ok(SIM_PRODUCTION_ENTITIES.contains(&entity.full_name.as_str()))
        })
        .unwrap_or(false);
    if production {
        return Ok(());
    }
    sim_check_open()
}

/// Java's `TransactionCacheDb.checkOpen`: an error once this thread's sim is poisoned.
fn sim_check_open() -> Result<(), Error> {
    SIM.with(
        |slot| match slot.borrow().last().and_then(|session| session.poisoned.clone()) {
            Some(reason) => Err(Error::new(reason)),
            None => Ok(()),
        },
    )
}

/// Java's `TransactionCacheDb.noteWrite`, before each sim write: once per transaction, a hook that poisons the sim when
/// that transaction rolls back, since what it wrote to the sim is not undone. A write outside a transaction is not
/// tracked. A hook that cannot be added poisons the sim at once.
fn note_sim_write(tx: &TransactionManager) -> Result<(), Error> {
    sim_check_open()?;
    let Some(token) = tx.frame_token() else {
        return Ok(());
    };
    let schema = SIM.with(|slot| {
        let mut sessions = slot.borrow_mut();
        let session = sessions.last_mut()?;
        session.tracked.insert(token).then(|| session.schema.clone())
    });
    let Some(schema) = schema else {
        return Ok(());
    };
    let on_commit = schema.clone();
    let registered = tx
        .on_rollback(move || end_sim_tracking(&schema, token, Some(SIM_ROLLED_BACK)))
        .and_then(|()| tx.on_commit(move || end_sim_tracking(&on_commit, token, None)));
    if let Err(err) = registered {
        let reason =
            format!("The sim could not track its transaction ({err}), so it has been terminated. Start a new sim.");
        let schema = SIM.with(|slot| slot.borrow().last().map(|session| session.schema.clone()));
        if let Some(schema) = schema {
            end_sim_tracking(&schema, token, Some(&reason));
        }
        return Err(Error::new(reason));
    }
    Ok(())
}

/// The end of a tracked transaction of the sim `schema` on this thread; `poison` ends the sim with that reason.
fn end_sim_tracking(schema: &str, token: u64, poison: Option<&str>) {
    SIM.with(|slot| {
        let mut sessions = slot.borrow_mut();
        let Some(session) = sessions.iter_mut().rev().find(|session| session.schema == schema) else {
            return;
        };
        session.tracked.remove(&token);
        if let (Some(reason), None) = (poison, &session.poisoned) {
            tracing::warn!("Sim overlay terminated: {reason}");
            session.poisoned = Some(reason.to_string());
        }
    });
}

/// Java's `TransactionCacheDb.TOTAL_COPY_CAP`: rows copied from production over one sim, which bounds its size.
const SIM_TOTAL_COPY_CAP: usize = 200_000;

/// Java's `TransactionCacheDb.SEQ_START`: where the ids of a sim's sequences start.
const SIM_SEQ_START: i64 = 900_000_000;

/// Java's `TransactionCacheDb.PRODUCTION_ENTITIES`: written to the real database during a sim. The agent's bookkeeping
/// (a preview runs inside a run: its conversation, run, steps, events, messages, tool calls and approvals, and the
/// provider calls and their usage, plans/LLM-A2A-Assist.md section 5) and the sequence banks.
const SIM_PRODUCTION_ENTITIES: &[&str] = &[
    "moqui.agent.AgentConversation",
    "moqui.agent.AgentRun",
    "moqui.agent.AgentRunStep",
    "moqui.agent.AgentRunEvent",
    "moqui.agent.AgentMessage",
    "moqui.agent.AgentMessagePart",
    "moqui.agent.AgentToolCall",
    "moqui.agent.AgentApproval",
    "moqui.llm.LlmCall",
    "moqui.llm.LlmUsageDaily",
    "moqui.entity.SequenceValueItem",
];

/// The next id of `seq_name` from the sim's own bank, when a sim session is open on this thread.
pub(crate) fn sim_next_seq(seq_name: &str) -> Option<i64> {
    SIM.with(|slot| {
        let mut sessions = slot.borrow_mut();
        let session = sessions.last_mut()?;
        let next = session.seq_banks.entry(seq_name.to_string()).or_insert(SIM_SEQ_START);
        *next += 1;
        Some(*next)
    })
}

struct SimHit {
    schema: String,
    table: bool,
    tombstone: bool,
    written: bool,
    /// A copy of the production row is in the sim table. See [`SimSession::replicas`].
    replica: bool,
}

struct SimView {
    schema: String,
    table: bool,
    hide: HashSet<String>,
}

#[derive(Default)]
struct OneCaches {
    lists: HashMap<String, MCache<String, CachedList>>,
    /// Java's `entity.record.count.{entity}`: the count of each cached condition.
    counts: HashMap<String, MCache<String, CachedCount>>,
    /// The view entities that have each entity as a member, made on first use.
    views_by_member: Option<Arc<HashMap<String, Vec<String>>>>,
}

/// A cached find: its condition, which a write compares with what it wrote, and its rows.
#[derive(Clone)]
struct CachedList {
    condition: Option<Condition>,
    rows: EntityList,
}

#[derive(Clone)]
struct CachedCount {
    condition: Option<Condition>,
    count: i64,
}

/// One frame's writes by entity; `None` once there were too many to keep.
type FrameWrites = HashMap<String, Option<Vec<FrameWrite>>>;

/// How an overlay gets the stored row a held update applies to.
type StoredRow<'a> = &'a mut dyn FnMut(&Held) -> Result<Option<FieldMap>, Error>;

/// Writes of an open frame, by entity, replayed against the caches when it ends. Past this many the entity's caches are
/// cleared whole instead.
const FRAME_WRITES_MAX: usize = 200;

/// Look up a cached list, or rows held by the active transaction frame or the sim overlay.
///
/// `Some` skips SQL. Plaintext only: `after_find` stored decrypted rows, and held rows were
/// normalized before `gate_write`.
pub fn before_find(facade: &EntityFacade, lookup: &FindLookup) -> Option<EntityList> {
    let meta = match read_meta(facade, &lookup.entity_name) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::error!(error = %err, "entity cache find skipped");
            return None;
        }
    };
    if meta.is_view {
        // A view is read from the database, so the frame's held rows of its members go there first.
        if let Err(err) = flush_for_view(facade, &lookup.entity_name) {
            tracing::error!(error = %err, entity = %meta.full_name, "could not flush held rows for a view read");
        }
    } else {
        match overlay_rows(facade, &meta, lookup.condition.as_ref(), &lookup.order) {
            Ok(Some(rows)) => {
                return Some(EntityList::new(page_rows(rows, lookup.limit, lookup.offset)));
            }
            Ok(None) => {}
            Err(err) => {
                tracing::error!(error = %err, entity = %meta.full_name, "entity cache find skipped");
                return None;
            }
        }
    }
    if meta.caches(lookup) && !frame_wrote(facade, &lookup.entity_name, &meta) {
        return cache_get(facade, &meta.full_name, lookup);
    }
    None
}

/// Count held and sim rows when this thread has overlaid that entity. Otherwise miss.
///
/// `Some` is the count itself. The merged list is unlimited, so its length is that count.
pub fn before_count(facade: &EntityFacade, lookup: &FindLookup) -> Option<i64> {
    let meta = match read_meta(facade, &lookup.entity_name) {
        Ok(meta) => meta,
        Err(err) => {
            tracing::error!(error = %err, "entity cache count skipped");
            return None;
        }
    };
    if meta.is_view {
        if let Err(err) = flush_for_view(facade, &lookup.entity_name) {
            tracing::error!(error = %err, entity = %meta.full_name, "could not flush held rows for a view count");
        }
    } else {
        match overlay_rows(facade, &meta, lookup.condition.as_ref(), &[]) {
            Ok(Some(rows)) => return Some(i64::try_from(rows.len()).unwrap_or(i64::MAX)),
            Ok(None) => {}
            Err(err) => {
                tracing::error!(error = %err, entity = %meta.full_name, "entity cache count skipped");
                return None;
            }
        }
    }
    if meta.caches(lookup) && !frame_wrote(facade, &lookup.entity_name, &meta) {
        let cache = facade.with_slot_read(OneCaches::default, |caches| caches.counts.get(&meta.full_name).cloned())?;
        return with_lookup_key(lookup, |key| {
            cache.get_arc(key).ok().flatten().map(|cached| cached.count)
        });
    }
    None
}

/// Store a count the database gave, as Java's count cache, when the entity's finds are cached.
pub fn after_count(facade: &EntityFacade, lookup: &FindLookup, count: i64) -> Result<(), Error> {
    let meta = read_meta(facade, &lookup.entity_name)?;
    if !meta.caches(lookup) || frame_wrote(facade, &lookup.entity_name, &meta) {
        return Ok(());
    }
    let key = owned_lookup_key(lookup);
    let name = &meta.full_name;
    let known = facade.with_slot_read(OneCaches::default, |caches| caches.counts.get(name).cloned());
    let cache = known.unwrap_or_else(|| {
        facade.with_slot(OneCaches::default, |caches| {
            caches
                .counts
                .entry(name.clone())
                .or_insert_with(|| count_cache(name))
                .clone()
        })
    });
    cache.put(
        key,
        CachedCount {
            condition: lookup.condition.clone(),
            count,
        },
    )
}

/// Store a find result when the entity cache attribute is `true`. Rows are already plaintext.
///
/// Not stored while the active frame has written the entity (any entity, for a view): those rows may be
/// uncommitted.
pub fn after_find(facade: &EntityFacade, lookup: &FindLookup, rows: &EntityList) -> Result<(), Error> {
    let meta = read_meta(facade, &lookup.entity_name)?;
    if !meta.caches(lookup) || frame_wrote(facade, &lookup.entity_name, &meta) {
        return Ok(());
    }
    let key = owned_lookup_key(lookup);
    let name = &meta.full_name;
    let known = facade.with_slot_read(OneCaches::default, |caches| caches.lists.get(name).cloned());
    let cache = known.unwrap_or_else(|| {
        facade.with_slot(OneCaches::default, |caches| {
            caches
                .lists
                .entry(name.clone())
                .or_insert_with(|| list_cache(name))
                .clone()
        })
    });
    cache.put(
        key,
        CachedList {
            condition: lookup.condition.clone(),
            rows: rows.clone(),
        },
    )
}

/// True when this thread's active frame for the entity has written it. For a view, any write in the frame counts,
/// since member writes do not name the view.
fn frame_wrote(facade: &EntityFacade, entity_name: &str, meta: &ReadMeta) -> bool {
    if TX_WRITTEN.with(|slot| slot.borrow().is_empty()) {
        return false;
    }
    let Some(token) = facade
        .transactions_for(entity_name)
        .ok()
        .and_then(|tx| tx.frame_token())
    else {
        return false;
    };
    TX_WRITTEN.with(|slot| {
        slot.borrow().get(&token).is_some_and(|names| {
            if meta.is_view {
                !names.is_empty()
            } else {
                names.contains(&meta.full_name)
            }
        })
    })
}

/// Inside a transaction, record the write on the frame and clear the one-cache again when the frame commits or
/// rolls back, so rows read and cached by other threads before the commit do not stay. Hooks go on once per entity
/// per frame.
/// `write` runs only inside a frame, so a write with none does not copy its values. When it gives `None` the entity's
/// writes in the frame are not kept, and its caches are cleared whole at the end.
fn note_frame_write(
    facade: &EntityFacade,
    entity_name: &str,
    full_name: &str,
    write: impl FnOnce() -> Option<FrameWrite>,
) -> Result<(), Error> {
    let tx = facade.transactions_for(entity_name)?;
    let Some(token) = tx.frame_token() else {
        return Ok(());
    };
    let write = write();
    TX_WRITES.with(|slot| {
        let mut frames = slot.borrow_mut();
        let entry = frames
            .entry(token)
            .or_default()
            .entry(full_name.to_string())
            .or_insert_with(|| Some(Vec::new()));
        match (entry.as_mut(), write) {
            (Some(writes), Some(write)) if writes.len() < FRAME_WRITES_MAX => writes.push(write),
            _ => *entry = None,
        }
    });
    if TX_WRITTEN.with(|slot| slot.borrow().get(&token).is_some_and(|names| names.contains(full_name))) {
        return Ok(());
    }
    let (commit_facade, commit_name) = (facade.clone(), full_name.to_string());
    tx.on_commit(move || end_frame_write(&commit_facade, token, &commit_name))?;
    let (rollback_facade, rollback_name) = (facade.clone(), full_name.to_string());
    tx.on_rollback(move || end_frame_write(&rollback_facade, token, &rollback_name))?;
    TX_WRITTEN.with(|slot| {
        slot.borrow_mut()
            .entry(token)
            .or_default()
            .insert(full_name.to_string());
    });
    Ok(())
}

fn end_frame_write(facade: &EntityFacade, token: u64, full_name: &str) {
    TX_WRITTEN.with(|slot| {
        let mut written = slot.borrow_mut();
        if let Some(names) = written.get_mut(&token) {
            names.remove(full_name);
            if names.is_empty() {
                written.remove(&token);
            }
        }
    });
    let writes = TX_WRITES.with(|slot| {
        let mut frames = slot.borrow_mut();
        let writes = frames.get_mut(&token).and_then(|entities| entities.remove(full_name));
        if frames.get(&token).is_some_and(HashMap::is_empty) {
            frames.remove(&token);
        }
        writes
    });
    note_feed_definition(facade, full_name);
    // The writes again, now that other threads may have read and cached the rows as they were before the end.
    let result = match (writes.flatten(), read_meta(facade, full_name)) {
        (Some(writes), Ok(meta)) => writes
            .iter()
            .try_for_each(|write| evict(facade, &meta, write.operation, &write.values).map(|_| ())),
        _ => clear_named(facade, full_name),
    };
    if let Err(err) = result {
        tracing::warn!(error = %err, entity = %full_name, "entity cache clear at transaction end failed");
    }
    // After the evict, so a definition built meanwhile does not keep rows read from the cache before it.
    super::derived::note_write(facade, full_name);
}

/// One write of a frame, kept to evict again at the frame's end.
struct FrameWrite {
    operation: &'static str,
    values: Arc<FieldMap>,
}

/// `operation` as the static name [`FrameWrite`] keeps; `None` for one [`evict`] does not name.
fn frame_operation(operation: &str) -> Option<&'static str> {
    match operation {
        "create" => Some("create"),
        "update" => Some("update"),
        "upsert" => Some("upsert"),
        "delete" => Some("delete"),
        _ => None,
    }
}

/// Java's `EntityCache.clearCacheForValue`: each cached find of the entity that the write may have changed (it held
/// the row, or its condition may match the values written; a field the write did not set counts as a match), each
/// count it may have changed, and every cached find of a view that has the entity as a member. True when the entity
/// has a cache of finds or of counts.
fn evict(
    facade: &EntityFacade,
    meta: &ReadMeta,
    operation: &str,
    values: &FieldMap,
) -> Result<bool, Error> {
    let (lists, counts) = facade.with_slot_read(OneCaches::default, |caches| {
        (
            caches.lists.get(&meta.full_name).cloned(),
            caches.counts.get(&meta.full_name).cloned(),
        )
    });
    let pk: Option<Vec<(&str, &Value)>> = meta
        .pk_fields
        .iter()
        .map(|name| {
            values
                .get(name.as_str())
                .filter(|value| !value.is_null())
                .map(|value| (name.as_str(), value))
        })
        .collect();
    let cached = lists.is_some() || counts.is_some();
    if let Some(cache) = lists {
        cache.remove_where(|_, cached| list_affected(cached, operation, values, pk.as_deref()))?;
    }
    if let Some(cache) = counts {
        // A count has no rows to tell whether the row was in it: an update or delete clears them all.
        if operation == "create" {
            cache.remove_where(|_, cached| may_match(cached.condition.as_ref(), values))?;
        } else {
            cache.clear()?;
        }
    }
    let views = views_by_member(facade);
    for view in views.get(&meta.full_name).into_iter().flatten() {
        clear_named(facade, view)?;
    }
    Ok(cached)
}

/// Whether a write may change a cached list: a delete when the list held the row, a create when the row may match,
/// an update (or upsert) for either. A write whose primary key is not complete may change any list.
fn list_affected(
    cached: &CachedList,
    operation: &str,
    values: &FieldMap,
    pk: Option<&[(&str, &Value)]>,
) -> bool {
    let Some(pk) = pk else {
        return true;
    };
    let held = || {
        cached.rows.iter().any(|row| {
            pk.iter().all(|(name, value)| {
                row.get(name)
                    .is_some_and(|stored| stored == *value || to_plain_string(stored) == to_plain_string(value))
            })
        })
    };
    match operation {
        "delete" => held(),
        "create" => may_match(cached.condition.as_ref(), values),
        _ => held() || may_match(cached.condition.as_ref(), values),
    }
}

/// False only when the condition surely does not match the values: a field they do not hold, a comparison this
/// cannot judge, or values of different types count as a possible match.
fn may_match(condition: Option<&Condition>, values: &FieldMap) -> bool {
    condition.is_none_or(|condition| judge(condition, values) != Some(false))
}

/// A three-valued match: `None` when it cannot be told from `values`.
fn judge(condition: &Condition, values: &FieldMap) -> Option<bool> {
    let field = |field: &FieldRef| -> Option<&Value> {
        if field.entity_alias.is_some() {
            return None;
        }
        values.get(field.name.as_str())
    };
    let same = |left: &Value, right: &Value| -> Option<bool> {
        if left == right || to_plain_string(left) == to_plain_string(right) {
            Some(true)
        } else if std::mem::discriminant(left) == std::mem::discriminant(right) {
            Some(false)
        } else {
            None
        }
    };
    match condition {
        Condition::Equals(name, expected) => {
            let got = field(name)?;
            if expected.is_null() {
                return Some(got.is_null());
            }
            if got.is_null() {
                return Some(false);
            }
            same(got, expected)
        }
        Condition::NotEquals(name, expected) => {
            let got = field(name)?;
            if got.is_null() {
                return Some(false);
            }
            same(got, expected).map(|equal| !equal)
        }
        Condition::In(name, list) => {
            let got = field(name)?;
            if got.is_null() || list.is_empty() {
                return Some(false);
            }
            let mut unknown = false;
            for item in list {
                match same(got, item) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
            }
            (!unknown).then_some(false)
        }
        Condition::NotIn(name, list) => judge(&Condition::In(name.clone(), list.clone()), values).map(|found| {
            let got_null = field(name).is_none_or(Value::is_null);
            !found && !got_null
        }),
        Condition::IsNull(name) => field(name).map(Value::is_null),
        Condition::IsNotNull(name) => field(name).map(|got| !got.is_null()),
        Condition::And(list) => {
            let mut unknown = false;
            for child in list {
                match judge(child, values) {
                    Some(false) => return Some(false),
                    Some(true) => {}
                    None => unknown = true,
                }
            }
            (!unknown).then_some(true)
        }
        Condition::Or(list) => {
            let mut unknown = false;
            for child in list {
                match judge(child, values) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
            }
            (!unknown).then_some(false)
        }
        Condition::Not(inner) => judge(inner, values).map(|matched| !matched),
        _ => None,
    }
}

/// Each entity to the view entities that have it as a member, at any depth.
fn views_by_member(facade: &EntityFacade) -> Arc<HashMap<String, Vec<String>>> {
    let known = facade.with_slot_read(OneCaches::default, |caches| caches.views_by_member.clone());
    match known {
        Some(map) => map,
        None => {
            let map = Arc::new(build_views_by_member(facade));
            facade.with_slot(OneCaches::default, |caches| {
                caches.views_by_member = Some(Arc::clone(&map));
            });
            map
        }
    }
}

fn build_views_by_member(facade: &EntityFacade) -> HashMap<String, Vec<String>> {
    let model = facade.inner.model.model();
    let full = |name: &str| {
        model
            .entity(name)
            .map(|entity| entity.full_name.clone())
            .unwrap_or_else(|_| name.to_string())
    };
    let views: Vec<(String, Vec<String>)> = model
        .entities()
        .iter()
        .filter(|entity| entity.is_view)
        .map(|view| {
            let members = view
                .member_entities
                .iter()
                .map(|member| full(&member.entity_name))
                .collect();
            (view.full_name.clone(), members)
        })
        .collect();
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for (view, members) in &views {
        // A member that is itself a view reaches its own members, a few levels down.
        let mut pending: Vec<String> = members.clone();
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(member) = pending.pop() {
            if !seen.insert(member.clone()) {
                continue;
            }
            if let Some((_, inner)) = views.iter().find(|(name, _)| *name == member) {
                pending.extend(inner.iter().cloned());
            }
            map.entry(member).or_default().push(view.clone());
        }
    }
    map
}

/// Hold the write, or send it to the sim overlay. `Proceed` leaves SQL to the caller.
pub fn gate_write(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: &FieldMap,
) -> Result<WriteGate, Error> {
    let op = parse_op(operation)?;
    if super::fence::active() {
        let full_name = facade
            .with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()))
            .unwrap_or_else(|_| entity_name.to_string());
        super::fence::check_write(&full_name, operation)?;
    }
    // A sim leaves its own records and the sequence banks on the real database, as Java's `allowsProductionWrite`.
    let sim = sim_session_open()
        && !facade
            .with_entity(entity_name, |entity, _, _| {
                Ok(SIM_PRODUCTION_ENTITIES.contains(&entity.full_name.as_str()))
            })
            .unwrap_or(false);
    let tx = facade.transactions_for(entity_name)?;
    if !sim && !tx.transaction_cache_active() {
        return Ok(WriteGate::Proceed);
    }
    let prepared = prepare_handled(facade, entity_name, operation, values)?;
    if sim {
        if tx.in_transaction() {
            tx.with_connection(|_| Ok(()))?;
        }
        note_sim_write(&tx)?;
        write_sim(facade, entity_name, op, values, &prepared)?;
        return Ok(WriteGate::Handled);
    }
    tx.with_connection(|_| Ok(()))?;
    let token = tx.frame_token().ok_or_else(|| Error::new("no transaction in place"))?;
    // The flush goes on the frame before the first held row, so no held row is left without one.
    if !tx.has_cache_flush() {
        register_cache_flush(facade, &tx)?;
    }
    push_hold(
        token,
        Held {
            entity_name: prepared.full_name,
            operation: op,
            plain: values.clone(),
            sealed: prepared.sealed,
            pk: prepared.pk,
        },
    )?;
    Ok(WriteGate::Handled)
}

/// Counts writes to the screen configuration entities (`SubscreensItem`, `SubscreensDefault`), here or from a cluster
/// peer, so a screen definition read before the last one is loaded again. Kept with the facade: the rows are its
/// database's, and another facade's writes do not make these screens stale. Reading it takes no lock.
pub(crate) fn screen_config_generation(facade: &EntityFacade) -> u64 {
    facade.inner.screen_config_generation.load(AtomicOrdering::Acquire)
}

fn note_screen_config(facade: &EntityFacade, full_name: &str) {
    if full_name.starts_with("moqui.screen.Subscreens") {
        facade
            .inner
            .screen_config_generation
            .fetch_add(1, AtomicOrdering::AcqRel);
    }
}

/// Counts writes to the data feed and data document definitions, here or from a cluster peer, so the feed insert
/// and the document definitions read them again. A write inside a transaction counts again at its commit or rollback,
/// so a definition read from another frame while it was open is not kept. Reading it takes no lock.
pub(crate) fn feed_definition_generation(facade: &EntityFacade) -> u64 {
    facade.inner.feed_definition_generation.load(AtomicOrdering::Acquire)
}

/// A data feed or data document definition entity: `DataFeed`, `DataFeedDocument`, or any `moqui.entity.document`
/// entity. `DataFeedPending` rows are work, not definitions.
pub(crate) fn is_feed_definition(full_name: &str) -> bool {
    full_name.starts_with("moqui.entity.document.")
        || full_name == "moqui.entity.feed.DataFeed"
        || full_name == "moqui.entity.feed.DataFeedDocument"
}

fn note_feed_definition(facade: &EntityFacade, full_name: &str) {
    if is_feed_definition(full_name) {
        facade
            .inner
            .feed_definition_generation
            .fetch_add(1, AtomicOrdering::AcqRel);
    }
}

/// Clear this entity's one-cache, then publish `entity-cache-invalidate` for the primary key.
/// The transaction flush is separate and runs at commit. The local clear stays the whole entity. Inside a
/// transaction the clear runs again at commit or rollback, and the publish waits for commit.
pub fn on_write(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: &FieldMap,
) -> Result<(), Error> {
    write_hooks(facade, entity_name, operation, values, || Arc::new(values.clone()))
}

/// [`on_write`] for values the write's other hooks share, so a transaction frame keeps them without a copy.
pub(crate) fn on_write_shared(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: &Arc<FieldMap>,
) -> Result<(), Error> {
    write_hooks(facade, entity_name, operation, values, || Arc::clone(values))
}

fn write_hooks(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: &FieldMap,
    shared: impl FnOnce() -> Arc<FieldMap>,
) -> Result<(), Error> {
    let meta = read_meta(facade, entity_name)?;
    // With no cache of the entity's finds or counts there is nothing to evict the write from at the frame's end
    // either, unless one is made meanwhile; the entity is then cleared whole, and its values are not kept.
    let cached = evict(facade, &meta, operation, values)?;
    note_frame_write(facade, entity_name, &meta.full_name, || {
        let operation = frame_operation(operation).filter(|_| cached)?;
        Some(FrameWrite {
            operation,
            values: shared(),
        })
    })?;
    note_screen_config(facade, &meta.full_name);
    note_feed_definition(facade, &meta.full_name);
    super::derived::note_write(facade, &meta.full_name);
    crate::cluster::note_write(facade, entity_name, &meta.pk_fields, values).map_err(Error::from)
}

/// For another server's write, and do not publish: with its primary key, the cached finds that held the row or may
/// match the key (as an update), else the entity's caches whole. Remote apply uses this.
pub(crate) fn clear_one_cache(
    facade: &EntityFacade,
    entity_name: &str,
    primary_key: Option<&FieldMap>,
) -> Result<(), Error> {
    let meta = read_meta(facade, entity_name)?;
    note_screen_config(facade, &meta.full_name);
    note_feed_definition(facade, &meta.full_name);
    let cleared = match primary_key.filter(|keys| meta.pk_fields.iter().all(|name| keys.contains_key(name.as_str()))) {
        Some(keys) => evict(facade, &meta, "update", keys).map(|_| ()),
        None => clear_named(facade, &meta.full_name),
    };
    super::derived::note_write(facade, &meta.full_name);
    cleared
}

/// Clear the list and count caches of `full_name`, and those of the views it is a member of.
fn clear_named(facade: &EntityFacade, full_name: &str) -> Result<(), Error> {
    let (lists, counts) = facade.with_slot(OneCaches::default, |caches| {
        (
            caches.lists.get(full_name).cloned(),
            caches.counts.get(full_name).cloned(),
        )
    });
    if let Some(cache) = lists {
        cache.clear()?;
    }
    if let Some(cache) = counts {
        cache.clear()?;
    }
    Ok(())
}

/// The list cache of each entity that has cached a find, for the cache admin screen. The names are
/// `entity.record.list.{entity}`, as in Java.
pub fn list_cache_infos(facade: &EntityFacade) -> Vec<moqui_lib::cache::CacheInfo> {
    facade.with_slot(OneCaches::default, |caches| {
        let mut infos: Vec<_> = caches
            .lists
            .values()
            .map(MCache::info)
            .chain(caches.counts.values().map(MCache::info))
            .collect();
        infos.sort_by(|left, right| left.name.cmp(&right.name));
        infos
    })
}

/// Clear the list cache named `cache_name`. `false` when no entity has a list cache of that name.
pub fn clear_list_cache(facade: &EntityFacade, cache_name: &str) -> Result<bool, Error> {
    facade.with_slot(OneCaches::default, |caches| {
        if let Some(cache) = caches.lists.values().find(|cache| cache.name() == cache_name) {
            return cache.clear().map(|()| true);
        }
        match caches.counts.values().find(|cache| cache.name() == cache_name) {
            Some(cache) => cache.clear().map(|()| true),
            None => Ok(false),
        }
    })
}

/// Clear every entity list cache.
pub fn clear_all_list_caches(facade: &EntityFacade) -> Result<(), Error> {
    facade.with_slot(OneCaches::default, |caches| {
        for cache in caches.lists.values() {
            cache.clear()?;
        }
        for cache in caches.counts.values() {
            cache.clear()?;
        }
        Ok(())
    })
}

/// Register the commit flush and the rollback drop for this frame.
///
/// Both touch only this frame's held rows. The flush reads them when it runs, so later writes in
/// the frame are included. It uses the enlisted connection directly.
pub fn register_cache_flush(facade: &EntityFacade, tx: &TransactionManager) -> Result<(), Error> {
    let Some(token) = tx.frame_token() else {
        return Err(Error::new("no transaction in place"));
    };
    let facade = facade.clone();
    tx.on_cache_flush(move |conn| flush_held(&facade, token, conn))?;
    tx.on_rollback(move || {
        TX_HOLD.with(|slot| slot.borrow_mut().remove(&token));
    })?;
    Ok(())
}

/// Write this frame's held rows now and stop holding, as Java's `flushAndDisableTransactionCache`
/// (`makeReadOnly`): later writes in the frame go straight to the database. The commit hook that
/// [`register_cache_flush`] added then finds nothing left. No transaction cache is a no-op.
pub fn flush_and_disable(facade: &EntityFacade, tx: &TransactionManager) -> Result<(), Error> {
    if !tx.transaction_cache_active() {
        return Ok(());
    }
    let Some(token) = tx.frame_token() else {
        return Ok(());
    };
    tx.with_connection(|conn| flush_held(facade, token, conn))?;
    tx.set_cache_hold(false)
}

/// Open a sim schema on the overlay pool and mark sim active.
///
/// A second call on this thread pushes another schema. Reads and writes use the innermost one.
/// The first call in the process starts the overlay cluster through the facade's sim starter.
/// Fails without changing `sim_active` when there is no overlay pool and no starter.
pub fn enter_sim(facade: &EntityFacade) -> Result<(), Error> {
    let sim = facade.sim_transactions_or_start()?;
    let schema = next_schema();
    if !schema_name_ok(&schema) {
        return Err(Error::new("invalid sim schema"));
    }
    sim.with_connection(|conn| execute_conn(conn, &format!("CREATE SCHEMA {schema}"), &[]))?;
    SIM.with(|slot| {
        slot.borrow_mut().push(SimSession::new(schema));
    });
    Ok(())
}

/// Drop this thread's innermost sim schema. Sim stays active while an outer schema remains.
///
/// No session is a no-op.
pub fn exit_sim(facade: &EntityFacade) -> Result<(), Error> {
    let Some(session) = SIM.with(|slot| slot.borrow_mut().pop()) else {
        return Ok(());
    };
    let Some(sim) = facade.sim_transactions() else {
        return Err(Error::new("sim transactions are not configured"));
    };
    if !schema_name_ok(&session.schema) {
        return Err(Error::new("refusing to drop an invalid sim schema"));
    }
    sim.with_connection(|conn| {
        execute_conn(conn, &format!("DROP SCHEMA IF EXISTS {} CASCADE", session.schema), &[])?;
        Ok(())
    })
}

/// True when this thread has a sim schema open, as Java's `isTxCacheActive`.
#[must_use]
pub fn sim_entered() -> bool {
    sim_session_open()
}

/// The innermost sim schema of this thread, when a sim session is open.
pub(super) fn sim_schema() -> Option<String> {
    SIM.with(|slot| slot.borrow().last().map(|session| session.schema.clone()))
}

/// Whether the sim wrote or deleted a row of `entity` (a full name).
pub(super) fn sim_changed(entity: &str) -> bool {
    let prefix = format!("{entity}{PK_SEP}");
    SIM.with(|slot| {
        slot.borrow().last().is_some_and(|session| {
            session.tombstones.get(entity).is_some_and(|set| !set.is_empty())
                || session.written.iter().any(|key| key.starts_with(&prefix))
        })
    })
}

/// Rows deleted in the sim, as Java's `deletedCount`: production still returns them, so they shrink a copied page.
pub(super) fn sim_deleted_count() -> usize {
    SIM.with(|slot| {
        slot.borrow()
            .last()
            .map_or(0, |session| session.tombstones.values().map(HashSet::len).sum())
    })
}

/// The rows the sim wrote of `entity_name`, plain, read from its table. Not its replicas.
pub(super) fn sim_written_rows(
    facade: &EntityFacade,
    entity_name: &str,
) -> Result<Vec<FieldMap>, Error> {
    let meta = read_meta(facade, entity_name)?;
    let view = sim_view(&meta.full_name);
    if !view.table {
        return Ok(Vec::new());
    }
    let rows = load_plain(facade, &meta, None, &[], Some(&view.schema))?;
    Ok(rows
        .into_iter()
        .filter(|row| pk_of(row, &meta.pk_fields).is_some_and(|pk| view.hide.contains(&row_key(&meta.full_name, &pk))))
        .collect())
}

/// A row this thread's sim changed: its entity (full name), its primary key as `(field, value)` pairs, and its values in
/// the sim, none for a row it deleted.
#[derive(Clone, Debug, PartialEq)]
pub struct SimChange {
    pub entity: String,
    pub key: Vec<(String, String)>,
    pub values: Option<std::collections::BTreeMap<String, Value>>,
}

/// What the innermost sim of this thread wrote and deleted, by entity and key, beside [`sim_written_rows`]: each written
/// row with its values in the sim, each deleted one with its key. An agent's approval preview compares them with
/// production (plans/LLM-A2A-Assist.md 4.8). The entities a sim leaves on production are not among them.
///
/// # Errors
///
/// Returns an error when a written table cannot be read.
pub fn sim_changes(facade: &EntityFacade) -> Result<Vec<SimChange>, Error> {
    let (written, deleted) = SIM.with(|slot| {
        let guard = slot.borrow();
        let Some(session) = guard.last() else {
            return (Vec::new(), Vec::new());
        };
        let mut written: Vec<(String, String)> = session
            .written
            .iter()
            .filter_map(|key| key.split_once(PK_SEP).map(|(entity, pk)| (entity.to_string(), pk.to_string())))
            .collect();
        written.sort();
        let mut deleted: Vec<(String, String)> = session
            .tombstones
            .iter()
            .flat_map(|(entity, pks)| pks.iter().map(move |pk| (entity.clone(), pk.clone())))
            .collect();
        deleted.sort();
        (written, deleted)
    });
    let key_of = |pk: &str| -> Vec<(String, String)> {
        pk.split(PK_SEP)
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    };
    let mut out = Vec::new();
    let mut entities: Vec<&String> = written.iter().map(|(entity, _)| entity).collect();
    entities.dedup();
    for entity in entities {
        let meta = read_meta(facade, entity)?;
        let rows = sim_written_rows(facade, entity)?;
        for (_, pk) in written.iter().filter(|(name, _)| name == entity) {
            let values = rows
                .iter()
                .find(|row| pk_of(row, &meta.pk_fields).as_deref() == Some(pk.as_str()))
                .map(|row| row.iter().map(|(name, value)| (name.to_string(), value.clone())).collect());
            out.push(SimChange {
                entity: entity.clone(),
                key: key_of(pk),
                values,
            });
        }
    }
    for (entity, pk) in deleted {
        out.push(SimChange {
            entity,
            key: key_of(&pk),
            values: None,
        });
    }
    Ok(out)
}

/// Why this thread's innermost sim can no longer be trusted (a rolled-back transaction wrote to it), if it cannot.
pub fn sim_poisoned() -> Option<String> {
    SIM.with(|slot| slot.borrow().last().and_then(|session| session.poisoned.clone()))
}

/// Create the table of `entity_name` in the sim schema when the sim has none yet.
pub(super) fn ensure_sim_table(facade: &EntityFacade, entity_name: &str) -> Result<(), Error> {
    let full_name = facade.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()))?;
    let (schema, exists) = SIM.with(|slot| {
        let guard = slot.borrow();
        let session = guard.last().ok_or_else(|| Error::new("sim session is missing"))?;
        Ok::<_, Error>((session.schema.clone(), session.tables.contains(&full_name)))
    })?;
    if exists {
        return Ok(());
    }
    let ddl = create_sql(facade, entity_name)?;
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    run_overlay(&sim, &schema, Some(&ddl), &[])?;
    SIM.with(|slot| {
        if let Some(session) = slot.borrow_mut().last_mut() {
            session.tables.insert(full_name);
        }
    });
    Ok(())
}

/// Copy a production row of `entity_name` into the sim schema as a replica, as Java's `copyFromProduction`. A row
/// the sim wrote, deleted or already copied is left as it is. Returns whether it was copied.
///
/// # Errors
///
/// Fails when the sim has copied [`SIM_TOTAL_COPY_CAP`] rows, as Java fails the sim, or the copy cannot be written.
pub(super) fn copy_replica(
    facade: &EntityFacade,
    entity_name: &str,
    plain: FieldMap,
) -> Result<bool, Error> {
    let owner = facade.clone();
    let (full_name, pk, sealed) = facade.with_entity(entity_name, |entity, _, _| {
        let pk = identity(entity, &plain)?;
        let sealed = field_rw::seal_stored(&owner, entity, plain.clone())?;
        Ok((entity.full_name.clone(), pk, sealed))
    })?;
    let key = row_key(&full_name, &pk);
    let skip = SIM.with(|slot| {
        let mut guard = slot.borrow_mut();
        let session = guard.last_mut().ok_or_else(|| Error::new("sim session is missing"))?;
        if session.written.contains(&key)
            || session.replicas.contains(&key)
            || session.tombstones.get(&full_name).is_some_and(|set| set.contains(&pk))
        {
            return Ok(true);
        }
        session.copied += 1;
        if session.copied > SIM_TOTAL_COPY_CAP {
            return Err(Error::new(format!(
                "The sim copied more than {SIM_TOTAL_COPY_CAP} production rows. Narrow the query or end the sim."
            )));
        }
        Ok::<_, Error>(false)
    })?;
    if skip {
        return Ok(false);
    }
    ensure_sim_table(facade, entity_name)?;
    let schema = sim_schema().ok_or_else(|| Error::new("sim session is missing"))?;
    let mut statements = Vec::new();
    push_statement(&mut statements, dml(facade, entity_name, Op::Create, &sealed)?);
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    run_overlay(&sim, &schema, None, &statements)?;
    SIM.with(|slot| {
        if let Some(session) = slot.borrow_mut().last_mut() {
            session.replicas.insert(key);
        }
    });
    Ok(true)
}

/// Run a read on the sim schema `schema`.
pub(super) fn query_sim(facade: &EntityFacade, schema: &str, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
    sim_check_open()?;
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    sim.with_connection(|conn| {
        query_conn(
            conn,
            "SELECT set_config('search_path', ?, false)",
            &[Value::string(schema)],
        )?;
        query_conn(conn, sql, params)
    })
}

/// End every sim session this thread still has open, dropping its schema, and clear the sim flag. A failed drop is
/// logged; the session is gone from the thread either way.
pub(super) fn clear_sim(facade: &EntityFacade) {
    while sim_session_open() {
        if let Err(err) = exit_sim(facade) {
            tracing::warn!(error = %err, "could not drop a sim schema left open");
        }
    }
    SIM_FLAG.with(|flag| flag.set(false));
}

/// Forget the held rows and written entities of every transaction frame on this thread. Only for a thread with no
/// frame left, when these belong to frames that ended without their hooks.
pub(super) fn clear_frame_state() {
    TX_HOLD.with(|slot| slot.borrow_mut().clear());
    TX_WRITTEN.with(|slot| slot.borrow_mut().clear());
}

pub(super) fn set_sim_flag(value: bool) {
    SIM_FLAG.with(|flag| flag.set(value));
}

pub(super) fn sim_flag() -> bool {
    SIM_FLAG.with(std::cell::Cell::get) || sim_session_open()
}

fn flush_held(facade: &EntityFacade, token: u64, conn: &mut dyn DbConnection) -> Result<(), Error> {
    let held_rows = TX_HOLD
        .with(|slot| slot.borrow_mut().remove(&token))
        .unwrap_or_default();
    for row in held_rows {
        let statement = facade.with_group(&row.entity_name, |entity, dialect, _, _| {
            statement_for(entity, dialect, row.operation, &row.sealed)
        })?;
        if let Some((sql, params)) = statement {
            conn.execute(&sql, &params)?;
        }
    }
    Ok(())
}

fn push_hold(token: u64, held: Held) -> Result<(), Error> {
    TX_HOLD.with(|slot| {
        let mut map = slot.borrow_mut();
        let list = map.entry(token).or_default();
        let mut first = None;
        let mut last = None;
        for existing in list.iter() {
            if existing.entity_name == held.entity_name && existing.pk == held.pk {
                if first.is_none() {
                    first = Some(existing.operation);
                }
                last = Some(existing.operation);
            }
        }
        match held.operation {
            Op::Create => {
                if last.is_some_and(|op| op != Op::Delete) {
                    return Err(Error::new(format!(
                        "create of existing transaction cache row {} {}",
                        held.entity_name, held.pk
                    )));
                }
                list.push(held);
                Ok(())
            }
            Op::Update | Op::Upsert => {
                list.push(held);
                Ok(())
            }
            Op::Delete => {
                if first == Some(Op::Create) {
                    list.retain(|existing| existing.entity_name != held.entity_name || existing.pk != held.pk);
                    Ok(())
                } else {
                    list.push(held);
                    Ok(())
                }
            }
        }
    })
}

fn write_sim(
    facade: &EntityFacade,
    entity_name: &str,
    op: Op,
    plain: &FieldMap,
    prepared: &SealedWrite,
) -> Result<(), Error> {
    let hit = sim_hit(&prepared.full_name, &prepared.pk)?;
    // The overlay dialect rejects insert-or-update. A new sim row is an insert; a written row is an update.
    // A replica row is in the sim table already, as a written one is.
    let present = hit.written || hit.replica;
    let op = match op {
        Op::Upsert if present => Op::Update,
        Op::Upsert => Op::Create,
        other => other,
    };
    if op == Op::Update && hit.tombstone {
        return Ok(());
    }
    if op == Op::Create && present {
        return Err(Error::new(format!(
            "create of existing sim row {} {}",
            prepared.full_name, prepared.pk
        )));
    }
    if op == Op::Delete && !hit.table {
        note_sim(&prepared.full_name, &prepared.pk, Op::Delete, false);
        return Ok(());
    }
    let sealed_copy = if op == Op::Update && !present {
        match load_by_pk(facade, entity_name, plain)? {
            Some(row) => Some(seal(facade, entity_name, "create", row)?),
            None => None,
        }
    } else {
        None
    };
    let ddl = if hit.table {
        None
    } else {
        Some(create_sql(facade, entity_name)?)
    };
    let statements = sim_statements(facade, entity_name, op, &hit, &prepared.sealed, sealed_copy.as_ref())?;
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    run_overlay(&sim, &hit.schema, ddl.as_deref(), &statements)?;
    note_sim(&prepared.full_name, &prepared.pk, op, ddl.is_some());
    Ok(())
}

fn sim_statements(
    facade: &EntityFacade,
    entity_name: &str,
    op: Op,
    hit: &SimHit,
    sealed: &FieldMap,
    sealed_copy: Option<&FieldMap>,
) -> Result<Vec<(String, Vec<Value>)>, Error> {
    let mut statements = Vec::new();
    if let Some(copied) = sealed_copy {
        push_statement(&mut statements, dml(facade, entity_name, Op::Create, copied)?);
        push_statement(&mut statements, dml(facade, entity_name, Op::Update, sealed)?);
    } else if op == Op::Update && !hit.written && !hit.replica {
        push_statement(&mut statements, dml(facade, entity_name, Op::Create, sealed)?);
    } else {
        push_statement(&mut statements, dml(facade, entity_name, op, sealed)?);
    }
    Ok(statements)
}

fn push_statement(statements: &mut Vec<(String, Vec<Value>)>, statement: Option<(String, Vec<Value>)>) {
    if let Some(statement) = statement {
        statements.push(statement);
    }
}

fn run_overlay(
    sim: &TransactionManager,
    schema: &str,
    ddl: Option<&str>,
    statements: &[(String, Vec<Value>)],
) -> Result<(), Error> {
    sim.with_connection(|conn| {
        conn.begin()?;
        let result = apply_overlay(conn, schema, ddl, statements);
        match result {
            Ok(()) => conn.commit(),
            Err(err) => {
                let _ = conn.rollback();
                Err(err)
            }
        }
    })
}

fn apply_overlay(
    conn: &mut dyn DbConnection,
    schema: &str,
    ddl: Option<&str>,
    statements: &[(String, Vec<Value>)],
) -> Result<(), Error> {
    query_conn(
        conn,
        "SELECT set_config('search_path', ?, false)",
        &[Value::string(schema)],
    )?;
    if let Some(ddl) = ddl {
        execute_script(conn, ddl)?;
    }
    for (sql, params) in statements {
        execute_conn(conn, sql, params)?;
    }
    Ok(())
}

fn note_sim(entity: &str, pk: &str, op: Op, created_table: bool) {
    let row_key = row_key(entity, pk);
    SIM.with(|slot| {
        let mut guard = slot.borrow_mut();
        let Some(session) = guard.last_mut() else {
            return;
        };
        if created_table {
            session.tables.insert(entity.to_string());
        }
        session.replicas.remove(&row_key);
        match op {
            Op::Create | Op::Update | Op::Upsert => {
                if let Some(set) = session.tombstones.get_mut(entity) {
                    set.remove(pk);
                }
                session.written.insert(row_key);
            }
            Op::Delete => {
                session.written.remove(&row_key);
                session
                    .tombstones
                    .entry(entity.to_string())
                    .or_default()
                    .insert(pk.to_string());
            }
        }
    });
}

fn overlay_rows(
    facade: &EntityFacade,
    meta: &ReadMeta,
    condition: Option<&Condition>,
    order: &[OrderBy],
) -> Result<Option<Vec<EntityValue>>, Error> {
    let mut holds = holds_for(facade, &meta.full_name);
    let sim = if sim_session_open() {
        sim_view(&meta.full_name)
    } else {
        SimView::empty()
    };
    if !sim.affects() {
        // Nothing held and no overlay: the database answers alone.
        if holds.is_empty() {
            return Ok(None);
        }
        // A find by the whole primary key sees only the held rows of that key.
        if let Some(pk) = pinned_pk(condition, &meta.pk_fields) {
            holds.retain(|held| held.pk == pk);
        }
        // With no held row that can be in the result, the database answers alone, with its limit and offset.
        if holds
            .iter()
            .all(|held| held.operation == Op::Create && !may_match(condition, &held.plain))
        {
            return Ok(None);
        }
    }
    if holds.is_empty() && !sim.affects() {
        return Ok(None);
    }
    let loaded = load_plain(facade, meta, condition, order, None)?;
    let mut rows: Vec<MemRow> = loaded
        .into_iter()
        .map(|fields| MemRow { fields, trusted: true })
        .collect();
    // An update of a row the condition did not load starts from the whole stored row, not the fields it set.
    let mut stored = |held: &Held| load_by_pk(facade, &meta.full_name, &held.plain);
    apply_holds(&mut rows, &holds, &meta.pk_fields, &mut stored)?;
    rows.retain(|row| row.trusted || row_matches(condition, &row.fields));
    if sim.affects() {
        rows.retain(|row| match pk_of(&row.fields, &meta.pk_fields) {
            Some(pk) => !sim.hide.contains(&row_key(&meta.full_name, &pk)),
            None => true,
        });
        if sim.table {
            let extra = load_plain(facade, meta, condition, order, Some(&sim.schema))?;
            for fields in extra {
                if let Some(pk) = pk_of(&fields, &meta.pk_fields) {
                    rows.retain(|row| pk_of(&row.fields, &meta.pk_fields).as_deref() != Some(pk.as_str()));
                }
                rows.push(MemRow { fields, trusted: true });
            }
        }
    }
    if !order.is_empty() {
        sort_rows(&mut rows, order);
    }
    let name: Arc<str> = Arc::from(meta.full_name.as_str());
    let values = rows
        .into_iter()
        .map(|row| EntityValue::from_fields(Arc::clone(&name), row.fields))
        .collect();
    Ok(Some(values))
}

fn apply_holds(
    rows: &mut Vec<MemRow>,
    holds: &[Held],
    pk_fields: &[String],
    stored: StoredRow<'_>,
) -> Result<(), Error> {
    for held in holds {
        match held.operation {
            Op::Delete => rows.retain(|row| pk_of(&row.fields, pk_fields).as_deref() != Some(held.pk.as_str())),
            Op::Create => {
                rows.retain(|row| pk_of(&row.fields, pk_fields).as_deref() != Some(held.pk.as_str()));
                rows.push(MemRow {
                    fields: held.plain.clone(),
                    trusted: false,
                });
            }
            Op::Update | Op::Upsert => {
                if let Some(row) = rows
                    .iter_mut()
                    .find(|row| pk_of(&row.fields, pk_fields).as_deref() == Some(held.pk.as_str()))
                {
                    for (key, value) in &held.plain {
                        row.fields.insert(key.clone(), value.clone());
                    }
                    row.trusted = false;
                } else {
                    // Not loaded: the stored row with this write on it, if there is one. An update of a row that is
                    // not there writes nothing; an upsert of one is its whole row.
                    let base = stored(held)?;
                    let fields = match (base, held.operation) {
                        (Some(mut base), _) => {
                            base.extend(held.plain.iter().map(|(key, value)| (key.clone(), value.clone())));
                            base
                        }
                        (None, Op::Upsert) => held.plain.clone(),
                        (None, _) => continue,
                    };
                    rows.push(MemRow { fields, trusted: false });
                }
            }
        }
    }
    Ok(())
}

/// The primary key text a condition pins: every key field compared by equality at the top level.
fn pinned_pk(condition: Option<&Condition>, pk_fields: &[String]) -> Option<String> {
    let condition = condition?;
    let parts: Vec<&Condition> = match condition {
        Condition::And(list) => list.iter().collect(),
        other => vec![other],
    };
    let mut keys = BTreeMap::new();
    for part in parts {
        if let Condition::Equals(field, value) = part {
            if field.entity_alias.is_none() && pk_fields.contains(&field.name) && !value.is_null() {
                keys.insert(Arc::from(field.name.as_str()), value.clone());
            }
        }
    }
    pk_of(&keys, pk_fields)
}

/// Before a view read: when the frame holds rows of one of the view's members, write every held row now (in order,
/// so a child row follows its parent), as Java's bypass of the transaction cache for a view.
fn flush_for_view(facade: &EntityFacade, view: &str) -> Result<(), Error> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return Ok(());
    }
    let tx = facade.transactions_for(view)?;
    let Some(token) = tx.frame_token() else {
        return Ok(());
    };
    let members: Vec<String> = facade.with_entity(view, |entity, _, model| {
        Ok(entity
            .member_entities
            .iter()
            .map(|member| {
                model
                    .entity(&member.entity_name)
                    .map(|entity| entity.full_name.clone())
                    .unwrap_or_else(|_| member.entity_name.clone())
            })
            .collect())
    })?;
    let held = TX_HOLD.with(|slot| {
        slot.borrow()
            .get(&token)
            .is_some_and(|list| list.iter().any(|held| members.contains(&held.entity_name)))
    });
    if held {
        tx.with_connection(|conn| flush_held(facade, token, conn))?;
    }
    Ok(())
}

fn load_plain(
    facade: &EntityFacade,
    meta: &ReadMeta,
    condition: Option<&Condition>,
    order: &[OrderBy],
    overlay_schema: Option<&str>,
) -> Result<Vec<FieldMap>, Error> {
    let plan = plan_select(facade, &meta.full_name, condition, order)?;
    let db_rows = if let Some(schema) = overlay_schema {
        query_sim(facade, schema, &plan.sql, &plan.params)?
    } else {
        query_tx(&plan.tx, &plan.sql, &plan.params)?
    };
    rows_plain(facade, &meta.full_name, &plan.columns, db_rows)
}

fn load_by_pk(
    facade: &EntityFacade,
    entity_name: &str,
    values: &FieldMap,
) -> Result<Option<FieldMap>, Error> {
    let owner = facade.clone();
    let plan = facade.with_group(entity_name, |entity, dialect, model, tx| {
        let condition = pk_condition(entity, values)?;
        let fields: Vec<FieldRef> = entity.fields.iter().map(|field| FieldRef::new(&field.name)).collect();
        let statement = select(entity, dialect, &fields, Some(&condition), &[], LimitOffset::default())?;
        Ok(SelectPlan {
            sql: statement.sql,
            params: statement.params,
            columns: entity_columns(entity, model),
            tx: tx.clone(),
        })
    })?;
    let db_rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
    let mut plain = rows_plain(&owner, entity_name, &plan.columns, db_rows)?;
    Ok(plain.pop())
}

struct SelectPlan {
    sql: String,
    params: Vec<Value>,
    columns: Vec<(Arc<str>, String)>,
    tx: TransactionManager,
}

fn plan_select(
    facade: &EntityFacade,
    entity_name: &str,
    condition: Option<&Condition>,
    order: &[OrderBy],
) -> Result<SelectPlan, Error> {
    facade.with_group(entity_name, |entity, dialect, model, tx| {
        if entity.is_view {
            return Err(Error::new(format!("cannot overlay view-entity {}", entity.full_name)));
        }
        let fields: Vec<FieldRef> = entity.fields.iter().map(|field| FieldRef::new(&field.name)).collect();
        let statement = select(entity, dialect, &fields, condition, order, LimitOffset::default())?;
        Ok(SelectPlan {
            sql: statement.sql,
            params: statement.params,
            columns: entity_columns(entity, model),
            tx: tx.clone(),
        })
    })
}

fn rows_plain(
    facade: &EntityFacade,
    entity_name: &str,
    columns: &[(Arc<str>, String)],
    rows: Vec<DbRow>,
) -> Result<Vec<FieldMap>, Error> {
    let owner = facade.clone();
    facade.with_entity(entity_name, |entity, _, _| {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mapped = map_row(columns, row);
            out.push(field_rw::after_read(&owner, entity, mapped)?);
        }
        Ok(out)
    })
}

fn create_sql(facade: &EntityFacade, entity_name: &str) -> Result<String, Error> {
    facade.with_group(entity_name, |entity, dialect, model, _| {
        create_table(entity, model, dialect)
    })
}

fn dml(
    facade: &EntityFacade,
    entity_name: &str,
    op: Op,
    values: &FieldMap,
) -> Result<Option<(String, Vec<Value>)>, Error> {
    facade.with_group(entity_name, |entity, dialect, _, _| {
        statement_for(entity, dialect, op, values)
    })
}

fn statement_for(
    entity: &EntityDefinition,
    dialect: &Dialect,
    operation: Op,
    values: &FieldMap,
) -> Result<Option<(String, Vec<Value>)>, Error> {
    match operation {
        Op::Delete => {
            let condition = pk_condition(entity, values)?;
            let statement = delete(entity, dialect, Some(&condition))?;
            Ok(Some((statement.sql, statement.params)))
        }
        Op::Update => {
            let columns = present_columns(entity, values, true, true);
            if columns.is_empty() {
                return Ok(None);
            }
            let condition = pk_condition(entity, values)?;
            let statement = update(entity, dialect, &columns, Some(&condition))?;
            let mut params = bind_columns(&columns, values);
            params.extend(statement.params);
            Ok(Some((statement.sql, params)))
        }
        Op::Create => {
            let columns = present_columns(entity, values, false, false);
            if columns.is_empty() {
                return Err(Error::new(format!("insert has no columns for {}", entity.full_name)));
            }
            let sql = insert(entity, dialect, &columns)?;
            Ok(Some((sql, bind_columns(&columns, values))))
        }
        Op::Upsert => {
            let columns = present_columns(entity, values, false, false);
            if columns.is_empty() {
                return Err(Error::new(format!("upsert has no columns for {}", entity.full_name)));
            }
            let sql = insert_or_update(entity, dialect, &columns)?;
            Ok(Some((sql, bind_columns(&columns, values))))
        }
    }
}

fn present_columns<'a>(
    entity: &'a EntityDefinition,
    values: &FieldMap,
    include_null: bool,
    skip_pk: bool,
) -> Vec<&'a str> {
    entity
        .fields
        .iter()
        .filter(|field| {
            if skip_pk && field.is_pk {
                return false;
            }
            matches!(values.get(field.name.as_str()), Some(value) if include_null || !value.is_null())
        })
        .map(|field| field.name.as_str())
        .collect()
}

fn bind_columns(columns: &[&str], values: &FieldMap) -> Vec<Value> {
    columns
        .iter()
        .map(|column| values.get(*column).cloned().unwrap_or(Value::Null))
        .collect()
}

fn pk_condition(entity: &EntityDefinition, values: &FieldMap) -> Result<Condition, Error> {
    let mut parts = Vec::new();
    for name in entity.pk_field_names() {
        let value = values
            .get(name)
            .filter(|value| !value.is_null())
            .cloned()
            .ok_or_else(|| Error::new(format!("primary key {name} is required on {}", entity.full_name)))?;
        parts.push(Condition::equals(name, value));
    }
    if parts.is_empty() {
        return Err(Error::new(format!("{} has no primary key", entity.full_name)));
    }
    if parts.len() == 1 {
        return Ok(parts.remove(0));
    }
    Ok(Condition::and(parts))
}

fn prepare_handled(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: &FieldMap,
) -> Result<SealedWrite, Error> {
    let owner = facade.clone();
    let operation = operation.to_string();
    facade.with_entity(entity_name, |entity, _, _| {
        let pk = identity(entity, values)?;
        let seal_operation = if operation == "upsert" {
            "create"
        } else {
            operation.as_str()
        };
        let sealed = field_rw::before_write(&owner, entity, seal_operation, values.clone())?;
        Ok(SealedWrite {
            full_name: entity.full_name.clone(),
            pk,
            sealed,
        })
    })
}

fn seal(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: FieldMap,
) -> Result<FieldMap, Error> {
    let owner = facade.clone();
    let operation = operation.to_string();
    facade.with_entity(entity_name, |entity, _, _| {
        field_rw::before_write(&owner, entity, &operation, values)
    })
}

/// The cache facts of each entity looked up, by the name it was looked up by.
type ReadMetas = super::derived::ByDefinition<Arc<ReadMeta>>;

/// Read once per entity name: every find and write asks.
/// The primary key field names of `entity_name`, read once per entity.
pub(crate) fn pk_field_names(facade: &EntityFacade, entity_name: &str) -> Result<PkNames, Error> {
    read_meta(facade, entity_name).map(PkNames)
}

/// See [`pk_field_names`].
pub(crate) struct PkNames(Arc<ReadMeta>);

impl std::ops::Deref for PkNames {
    type Target = [String];

    fn deref(&self) -> &[String] {
        &self.0.pk_fields
    }
}

fn read_meta(facade: &EntityFacade, entity_name: &str) -> Result<Arc<ReadMeta>, Error> {
    let generation = super::derived::generation(facade);
    if let Some(meta) = facade.with_slot_read(ReadMetas::default, |metas| metas.get(entity_name, generation)) {
        return Ok(meta);
    }
    let meta = Arc::new(facade.with_entity(entity_name, |entity, _, _| {
        Ok(ReadMeta {
            full_name: entity.full_name.clone(),
            is_view: entity.is_view,
            cache: entity.cache == "true",
            cache_never: entity.cache == "never",
            pk_fields: entity.pk_field_names().into_iter().map(str::to_string).collect(),
        })
    })?);
    facade.with_slot(ReadMetas::default, |metas| metas.insert(entity_name, generation, Arc::clone(&meta)));
    Ok(meta)
}

fn identity(entity: &EntityDefinition, values: &FieldMap) -> Result<String, Error> {
    let names = entity.pk_field_names();
    if names.is_empty() {
        return Err(Error::new(format!("{} has no primary key", entity.full_name)));
    }
    let mut key = String::new();
    for (index, name) in names.iter().enumerate() {
        let value = values
            .get(*name)
            .filter(|value| !value.is_null())
            .ok_or_else(|| Error::new(format!("primary key {name} is required on {}", entity.full_name)))?;
        if index > 0 {
            key.push(PK_SEP);
        }
        key.push_str(name);
        key.push('=');
        key.push_str(&to_plain_string(value));
    }
    Ok(key)
}

fn pk_of(fields: &FieldMap, pk_fields: &[String]) -> Option<String> {
    if pk_fields.is_empty() {
        return None;
    }
    let mut key = String::new();
    for (index, name) in pk_fields.iter().enumerate() {
        let value = fields.get(name.as_str()).filter(|value| !value.is_null())?;
        if index > 0 {
            key.push(PK_SEP);
        }
        key.push_str(name);
        key.push('=');
        key.push_str(&to_plain_string(value));
    }
    Some(key)
}

fn row_key(entity: &str, pk: &str) -> String {
    format!("{entity}{PK_SEP}{pk}")
}

/// The active frame's held rows of `entity`. A suspended frame's rows are not visible.
fn holds_for(facade: &EntityFacade, entity: &str) -> Vec<Held> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return Vec::new();
    }
    let Some(token) = facade.transactions_for(entity).ok().and_then(|tx| tx.frame_token()) else {
        return Vec::new();
    };
    TX_HOLD.with(|slot| {
        slot.borrow()
            .get(&token)
            .map(|list| list.iter().filter(|held| held.entity_name == entity).cloned().collect())
            .unwrap_or_default()
    })
}

fn sim_session_open() -> bool {
    SIM.with(|slot| !slot.borrow().is_empty())
}

fn sim_hit(entity: &str, pk: &str) -> Result<SimHit, Error> {
    let key = row_key(entity, pk);
    SIM.with(|slot| {
        let guard = slot.borrow();
        let session = guard.last().ok_or_else(|| Error::new("sim session is missing"))?;
        Ok(SimHit {
            schema: session.schema.clone(),
            table: session.tables.contains(entity),
            tombstone: session.tombstones.get(entity).is_some_and(|set| set.contains(pk)),
            written: session.written.contains(&key),
            replica: session.replicas.contains(&key),
        })
    })
}

fn sim_view(entity: &str) -> SimView {
    SIM.with(|slot| {
        let guard = slot.borrow();
        let Some(session) = guard.last() else {
            return SimView::empty();
        };
        let mut hide = HashSet::new();
        if let Some(set) = session.tombstones.get(entity) {
            for pk in set {
                hide.insert(row_key(entity, pk));
            }
        }
        let prefix = format!("{entity}{PK_SEP}");
        let mut written = false;
        for key in &session.written {
            if key.starts_with(&prefix) {
                hide.insert(key.clone());
                written = true;
            }
        }
        // A table that holds only replicas holds production's rows, so production answers alone.
        SimView {
            schema: session.schema.clone(),
            table: written && session.tables.contains(entity),
            hide,
        }
    })
}

impl SimView {
    fn empty() -> Self {
        Self {
            schema: String::new(),
            table: false,
            hide: HashSet::new(),
        }
    }

    fn affects(&self) -> bool {
        self.table || !self.hide.is_empty()
    }
}

fn cache_get(facade: &EntityFacade, entity_name: &str, lookup: &FindLookup) -> Option<EntityList> {
    let cache = facade.with_slot_read(OneCaches::default, |caches| caches.lists.get(entity_name).cloned())?;
    with_lookup_key(lookup, |key| {
        cache.get_arc(key).ok().flatten().map(|cached| cached.rows.clone())
    })
}

fn list_cache(name: &str) -> MCache<String, CachedList> {
    MCache::new(
        format!("entity.record.list.{name}"),
        CacheConfig::new().max_entries(LIST_MAX).background(true),
    )
}

fn count_cache(name: &str) -> MCache<String, CachedCount> {
    MCache::new(
        format!("entity.record.count.{name}"),
        CacheConfig::new().max_entries(LIST_MAX).background(true),
    )
}

thread_local! {
    /// Reused find-key text. A hit looks the key up as `&str` and does not allocate it again.
    static LOOKUP_KEY: RefCell<String> = RefCell::new(String::with_capacity(128));
}

/// Run `body` with the canonical key of `lookup`. `body` must not look up another find key.
fn with_lookup_key<T>(lookup: &FindLookup, body: impl FnOnce(&str) -> T) -> T {
    LOOKUP_KEY.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        write_lookup_key(lookup, &mut buf);
        body(buf.as_str())
    })
}

/// The key stored on a miss. Hits use [`with_lookup_key`] and do not allocate this.
fn owned_lookup_key(lookup: &FindLookup) -> String {
    let mut key = String::with_capacity(128);
    write_lookup_key(lookup, &mut key);
    key
}

/// The key of a find. The parts of each `and` and `or` are written in one order, so the same condition built another
/// way is the same entry. A single comparison writes straight into `out`.
fn write_lookup_key(lookup: &FindLookup, out: &mut String) {
    match &lookup.condition {
        Some(condition) => write_condition(condition, out),
        None => out.push_str("none"),
    }
    out.push('|');
    for entry in &lookup.order {
        write_field(&entry.field, out);
        out.push(if entry.descending { 'D' } else { 'A' });
        match entry.nulls {
            Some(Nulls::First) => out.push('F'),
            Some(Nulls::Last) => out.push('L'),
            None => out.push('-'),
        }
        match entry.case {
            Some(true) => out.push('U'),
            Some(false) => out.push('L'),
            None => out.push('-'),
        }
        out.push(';');
    }
    out.push('|');
    push_opt_i64(out, lookup.limit);
    out.push('|');
    push_opt_i64(out, lookup.offset);
}

fn push_opt_i64(out: &mut String, value: Option<i64>) {
    match value {
        Some(number) => {
            let _ = write!(out, "s{number}");
        }
        None => out.push('n'),
    }
}

fn write_condition(condition: &Condition, out: &mut String) {
    match condition {
        Condition::Equals(field, value) => write_cmp("eq", field, std::slice::from_ref(value), out),
        Condition::NotEquals(field, value) => write_cmp("ne", field, std::slice::from_ref(value), out),
        Condition::Less(field, value) => write_cmp("lt", field, std::slice::from_ref(value), out),
        Condition::Greater(field, value) => write_cmp("gt", field, std::slice::from_ref(value), out),
        Condition::LessEqual(field, value) => write_cmp("le", field, std::slice::from_ref(value), out),
        Condition::GreaterEqual(field, value) => write_cmp("ge", field, std::slice::from_ref(value), out),
        Condition::Like(field, value) => write_cmp("like", field, std::slice::from_ref(value), out),
        Condition::NotLike(field, value) => write_cmp("nlike", field, std::slice::from_ref(value), out),
        Condition::Between(field, low, high) => write_cmp("between", field, &[low.clone(), high.clone()], out),
        Condition::NotBetween(field, low, high) => write_cmp("nbetween", field, &[low.clone(), high.clone()], out),
        Condition::In(field, values) => write_cmp("in", field, values, out),
        Condition::NotIn(field, values) => write_cmp("nin", field, values, out),
        Condition::IsNull(field) => write_cmp("null", field, &[], out),
        Condition::IsNotNull(field) => write_cmp("nnull", field, &[], out),
        Condition::Matches(field, value) => write_cmp("match", field, std::slice::from_ref(value), out),
        Condition::CompareFields(left, op, right) => {
            out.push_str("cmpf(");
            write_field(left, out);
            out.push(',');
            out.push_str(match op {
                CompareOp::Equals => "eq",
                CompareOp::NotEquals => "ne",
                CompareOp::Less => "lt",
                CompareOp::Greater => "gt",
                CompareOp::LessEqual => "le",
                CompareOp::GreaterEqual => "ge",
            });
            out.push(',');
            write_field(right, out);
            out.push(')');
        }
        Condition::IgnoreCase(inner) => {
            out.push_str("ci(");
            write_condition(inner, out);
            out.push(')');
        }
        Condition::Not(inner) => {
            out.push_str("not(");
            write_condition(inner, out);
            out.push(')');
        }
        Condition::Where(sql) => {
            out.push_str("where(");
            push_text(out, sql);
            out.push(')');
        }
        Condition::Bound(field, op) => {
            out.push_str("bound(");
            write_field(field, out);
            out.push(',');
            out.push_str(match op {
                CompareOp::Equals => "eq",
                CompareOp::NotEquals => "ne",
                CompareOp::Less => "lt",
                CompareOp::Greater => "gt",
                CompareOp::LessEqual => "le",
                CompareOp::GreaterEqual => "ge",
            });
            out.push(')');
        }
        Condition::And(list) => write_group("and", list, out),
        Condition::Or(list) => write_group("or", list, out),
    }
}

/// `and` and `or` children in one order, so the same condition built another way is one cache entry.
fn write_group(op: &str, list: &[Condition], out: &mut String) {
    let mut parts: Vec<String> = list
        .iter()
        .map(|child| {
            let mut part = String::new();
            write_condition(child, &mut part);
            part
        })
        .collect();
    parts.sort();
    out.push_str(op);
    out.push('(');
    for part in &parts {
        out.push_str(part);
        out.push(';');
    }
    out.push(')');
}

fn write_cmp(op: &str, field: &FieldRef, values: &[Value], out: &mut String) {
    out.push_str(op);
    out.push('(');
    write_field(field, out);
    for value in values {
        out.push(',');
        write_value(value, out);
    }
    out.push(')');
}

fn write_field(field: &FieldRef, out: &mut String) {
    if let Some(alias) = &field.entity_alias {
        push_text(out, alias);
        out.push('.');
    }
    push_text(out, &field.name);
    if let Some(expr) = &field.expr {
        out.push('=');
        push_text(out, expr);
    }
}

/// The value as the condition was built. `String("1")` and `I64(1)` stay different keys.
fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push('N'),
        Value::String(text) => {
            out.push('S');
            push_text(out, text);
        }
        Value::Bool(flag) => out.push(if *flag { 'T' } else { 'F' }),
        Value::I32(number) => {
            let _ = write!(out, "i{number}");
        }
        Value::I64(number) => {
            let _ = write!(out, "l{number}");
        }
        Value::F64(number) => {
            let _ = write!(out, "f{}", number.to_bits());
        }
        Value::Decimal(number) => {
            out.push('d');
            push_text(out, &number.to_string());
        }
        // Every digit the value holds: two moments in one millisecond are different finds.
        Value::Timestamp(stamp) => {
            let _ = write!(out, "ts{}", stamp.as_nanosecond());
        }
        Value::Date(date) => {
            out.push_str("dt");
            push_text(out, &date.to_string());
        }
        Value::Time(time) => {
            out.push_str("tm");
            push_text(out, &time.to_string());
        }
        Value::Bytes(bytes) => {
            let _ = write!(out, "b{}:", bytes.len());
            for byte in bytes {
                let _ = write!(out, "{byte:02x}");
            }
        }
        Value::List(list) => {
            out.push('[');
            for item in list {
                write_value(item, out);
                out.push(';');
            }
            out.push(']');
        }
        Value::Map(map) => {
            out.push('{');
            for (name, item) in map {
                push_text(out, name);
                out.push(':');
                write_value(item, out);
                out.push(';');
            }
            out.push('}');
        }
        Value::Object(_) => out.push('O'),
    }
}

fn push_text(out: &mut String, text: &str) {
    let _ = write!(out, "{}:", text.len());
    out.push_str(text);
    out.push(';');
}

fn page_rows(mut rows: Vec<EntityValue>, limit: Option<i64>, offset: Option<i64>) -> Vec<EntityValue> {
    let start = usize::try_from(offset.unwrap_or(0).max(0)).unwrap_or(usize::MAX);
    if start >= rows.len() {
        return Vec::new();
    }
    if start > 0 {
        rows.drain(..start);
    }
    if let Some(limit) = limit {
        let limit = usize::try_from(limit.max(0)).unwrap_or(0);
        if rows.len() > limit {
            rows.truncate(limit);
        }
    }
    rows
}

fn sort_rows(rows: &mut [MemRow], order: &[OrderBy]) {
    rows.sort_by(|left, right| {
        for entry in order {
            let read = |row: &MemRow| {
                let value = row.fields.get(entry.field.name.as_str()).cloned().unwrap_or(Value::Null);
                match (entry.case, value) {
                    (Some(true), Value::String(text)) => Value::String(text.to_uppercase()),
                    (Some(false), Value::String(text)) => Value::String(text.to_lowercase()),
                    (_, value) => value,
                }
            };
            let nulls_first = entry.nulls == Some(Nulls::First);
            let ord = cmp_order(&read(left), &read(right), entry.descending, nulls_first);
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    });
}

/// The order the SQL writes: nulls last unless asked first, whatever the direction.
fn cmp_order(left: &Value, right: &Value, descending: bool, nulls_first: bool) -> Ordering {
    let null_side = if nulls_first { Ordering::Less } else { Ordering::Greater };
    match (left.is_null(), right.is_null()) {
        (true, true) => Ordering::Equal,
        (true, false) => null_side,
        (false, true) => null_side.reverse(),
        (false, false) => {
            let ord = compare_obj(left, right).unwrap_or(Ordering::Equal);
            if descending {
                ord.reverse()
            } else {
                ord
            }
        }
    }
}

fn row_matches(condition: Option<&Condition>, fields: &FieldMap) -> bool {
    match condition {
        None => true,
        Some(condition) => eval_condition(condition, fields),
    }
}

fn eval_condition(condition: &Condition, fields: &FieldMap) -> bool {
    match condition {
        Condition::Equals(field, expected) => field_value(fields, &field.name) == *expected,
        Condition::NotEquals(field, expected) => {
            let got = field_value(fields, &field.name);
            !got.is_null() && got != *expected
        }
        // A null value is IS NULL and IS NOT NULL in the SQL, as Java writes it.
        Condition::Like(field, pattern) | Condition::Between(field, pattern, _) if pattern.is_null() => {
            field_value(fields, &field.name).is_null()
        }
        Condition::NotLike(field, pattern) | Condition::NotBetween(field, pattern, _) if pattern.is_null() => {
            !field_value(fields, &field.name).is_null()
        }
        Condition::Less(field, expected) => {
            cmp_present(&field_value(fields, &field.name), expected) == Some(Ordering::Less)
        }
        Condition::Greater(field, expected) => {
            cmp_present(&field_value(fields, &field.name), expected) == Some(Ordering::Greater)
        }
        Condition::LessEqual(field, expected) => {
            cmp_present(&field_value(fields, &field.name), expected).is_some_and(|ord| ord != Ordering::Greater)
        }
        Condition::GreaterEqual(field, expected) => {
            cmp_present(&field_value(fields, &field.name), expected).is_some_and(|ord| ord != Ordering::Less)
        }
        Condition::In(field, values) => {
            if values.is_empty() {
                return false;
            }
            let got = field_value(fields, &field.name);
            values.contains(&got)
        }
        Condition::NotIn(field, values) => {
            if values.is_empty() {
                return true;
            }
            let got = field_value(fields, &field.name);
            !got.is_null() && !values.contains(&got)
        }
        Condition::Between(field, low, high) => {
            let got = field_value(fields, &field.name);
            let above = cmp_present(&got, low).is_some_and(|ord| ord != Ordering::Less);
            let below = cmp_present(&got, high).is_some_and(|ord| ord != Ordering::Greater);
            above && below
        }
        Condition::NotBetween(field, low, high) => {
            let got = field_value(fields, &field.name);
            let below = cmp_present(&got, low) == Some(Ordering::Less);
            let above = cmp_present(&got, high) == Some(Ordering::Greater);
            below || above
        }
        // A caller-bound placeholder has no value to compare.
        Condition::Bound(..) => false,
        Condition::Like(field, pattern) => compare_like(&field_value(fields, &field.name), pattern),
        Condition::NotLike(field, pattern) => !compare_like(&field_value(fields, &field.name), pattern),
        Condition::IsNull(field) => field_value(fields, &field.name).is_null(),
        Condition::IsNotNull(field) => !field_value(fields, &field.name).is_null(),
        Condition::And(list) => list.iter().all(|child| eval_condition(child, fields)),
        Condition::Or(list) => list.iter().any(|child| eval_condition(child, fields)),
        Condition::Not(inner) => !eval_condition(inner, fields),
        Condition::Matches(_, _) => false,
        // As Java's `WhereCondition.mapMatches`: raw SQL cannot be checked here, so the row stays.
        Condition::Where(_) => true,
        Condition::CompareFields(left, op, right) => {
            let ordering = cmp_present(&field_value(fields, &left.name), &field_value(fields, &right.name));
            match (op, ordering) {
                (CompareOp::Equals, _) => field_value(fields, &left.name) == field_value(fields, &right.name),
                (CompareOp::NotEquals, ordering) => ordering.is_some_and(|ord| ord != Ordering::Equal),
                (CompareOp::Less, ordering) => ordering == Some(Ordering::Less),
                (CompareOp::Greater, ordering) => ordering == Some(Ordering::Greater),
                (CompareOp::LessEqual, ordering) => ordering.is_some_and(|ord| ord != Ordering::Greater),
                (CompareOp::GreaterEqual, ordering) => ordering.is_some_and(|ord| ord != Ordering::Less),
            }
        }
        Condition::IgnoreCase(inner) => {
            let upper: FieldMap = fields
                .iter()
                .map(|(name, value)| match value {
                    Value::String(text) => (name.clone(), Value::String(text.to_uppercase())),
                    other => (name.clone(), other.clone()),
                })
                .collect();
            eval_condition(&upper_values(inner.as_ref().clone()), &upper)
        }
    }
}

/// The condition with every text value upper-cased, to compare against upper-cased fields.
fn upper_values(condition: Condition) -> Condition {
    let upper = |value: Value| match value {
        Value::String(text) => Value::String(text.to_uppercase()),
        other => other,
    };
    match condition {
        Condition::Equals(field, value) => Condition::Equals(field, upper(value)),
        Condition::NotEquals(field, value) => Condition::NotEquals(field, upper(value)),
        Condition::Less(field, value) => Condition::Less(field, upper(value)),
        Condition::Greater(field, value) => Condition::Greater(field, upper(value)),
        Condition::LessEqual(field, value) => Condition::LessEqual(field, upper(value)),
        Condition::GreaterEqual(field, value) => Condition::GreaterEqual(field, upper(value)),
        Condition::Like(field, value) => Condition::Like(field, upper(value)),
        Condition::NotLike(field, value) => Condition::NotLike(field, upper(value)),
        Condition::In(field, values) => Condition::In(field, values.into_iter().map(upper).collect()),
        Condition::NotIn(field, values) => Condition::NotIn(field, values.into_iter().map(upper).collect()),
        Condition::Between(field, low, high) => Condition::Between(field, upper(low), upper(high)),
        Condition::NotBetween(field, low, high) => Condition::NotBetween(field, upper(low), upper(high)),
        other => other,
    }
}

fn field_value(fields: &FieldMap, name: &str) -> Value {
    fields.get(name).cloned().unwrap_or(Value::Null)
}

fn cmp_present(left: &Value, right: &Value) -> Option<Ordering> {
    if left.is_null() || right.is_null() {
        return None;
    }
    compare_obj(left, right).ok()
}

fn parse_op(operation: &str) -> Result<Op, Error> {
    match operation {
        "create" => Ok(Op::Create),
        "update" => Ok(Op::Update),
        "delete" => Ok(Op::Delete),
        "upsert" => Ok(Op::Upsert),
        other => Err(Error::new(format!("unknown entity write {other}"))),
    }
}

fn next_schema() -> String {
    let seq = SIM_SEQ.fetch_add(1, AtomicOrdering::Relaxed);
    format!("sim_{}_{seq}", std::process::id())
}

fn schema_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {
            chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') && name.len() <= 63
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use moqui_db::sql::{CompareOp, Condition};
    use moqui_lib::error::Error;
    use moqui_lib::object::Value;

    use super::{eval_condition, judge, may_match, FindLookup, ReadMeta, WriteGate};
    use crate::entity::EntityFacade;
    use crate::testing;
    use crate::transaction::{TransactionAttribute, TransactionManager};

    fn row(pairs: &[(&str, Value)]) -> crate::entity::FieldMap {
        pairs
            .iter()
            .map(|(name, value)| ((*name).into(), value.clone()))
            .collect()
    }

    /// The eviction check: false only when the condition surely does not match what was written.
    #[test]
    fn a_write_may_match_unless_the_condition_surely_excludes_it() {
        let written = row(&[("enumTypeId", Value::string("B")), ("enumId", Value::string("E9"))]);
        let typed = || Condition::equals("enumTypeId", Value::string("A"));
        assert!(!may_match(Some(&typed()), &written));
        assert!(may_match(
            Some(&Condition::equals("enumTypeId", Value::string("B"))),
            &written
        ));
        // A field the write did not set, or a comparison this cannot judge, may match.
        assert!(may_match(
            Some(&Condition::equals("description", Value::string("x"))),
            &written
        ));
        assert!(may_match(
            Some(&Condition::Like("enumTypeId".into(), Value::string("A%"))),
            &written
        ));
        // Different types are not judged.
        assert_eq!(judge(&Condition::equals("enumTypeId", Value::I64(1)), &written), None);
        assert_eq!(
            judge(
                &Condition::and(vec![typed(), Condition::equals("description", Value::string("x"))]),
                &written
            ),
            Some(false)
        );
        assert_eq!(
            judge(
                &Condition::is_in("enumTypeId", vec![Value::string("A"), Value::string("B")]),
                &written
            ),
            Some(true)
        );
    }

    /// Java's `EntityCache.clearCacheForValue`: a write evicts the cached finds it may change (one holding the row, or
    /// one whose condition may match it), keeps the others, and clears the finds of a view of the entity.
    #[test]
    fn a_write_evicts_only_the_cached_finds_it_may_change() {
        let facade = testing::open();
        let make = |id: &str, kind: &str| {
            let mut value = facade.make_value("moqui.basic.Enumeration").unwrap();
            value.set("enumId", Value::string(id));
            value.set("enumTypeId", Value::string(kind));
            value
        };
        make("E1", "A").create(&facade).unwrap();
        make("E2", "B").create(&facade).unwrap();
        let cached = |kind: &str| {
            facade
                .find("moqui.basic.Enumeration")
                .unwrap()
                .eq("enumTypeId", Value::string(kind))
                .use_cache(true)
                .list()
                .unwrap()
                .len()
        };
        let entries = || {
            super::list_cache_infos(&facade)
                .into_iter()
                .find(|info| info.name == "entity.record.list.moqui.basic.Enumeration")
                .map_or(0, |info| info.size)
        };
        assert_eq!((cached("A"), cached("B")), (1, 1));
        assert_eq!(entries(), 2);
        // A new row of type B leaves the cached list of type A.
        make("E3", "B").create(&facade).unwrap();
        assert_eq!(entries(), 1);
        assert_eq!(cached("B"), 2);
        // An update of E1, which the list of type A holds, takes that list.
        let mut moved = make("E1", "C");
        moved.set("description", Value::string("moved"));
        moved.update(&facade).unwrap();
        assert_eq!(cached("A"), 0);
        // The cached finds of a view of the entity go when a member is written.
        let view_rows = || {
            facade
                .find("moqui.basic.EnumerationAndType")
                .unwrap()
                .eq("enumTypeId", Value::string("B"))
                .use_cache(true)
                .list()
                .unwrap()
                .len()
        };
        if facade
            .with_entity("moqui.basic.EnumerationAndType", |_, _, _| Ok(()))
            .is_ok()
        {
            let mut kind = facade.make_value("moqui.basic.EnumerationType").unwrap();
            kind.set("enumTypeId", Value::string("B"));
            kind.create(&facade).unwrap();
            assert_eq!(view_rows(), 2);
            make("E4", "B").create(&facade).unwrap();
            assert_eq!(view_rows(), 3);
        }
    }

    /// Java `shouldCache` and the list find: a find with no condition, a limit or offset, or the user's
    /// authorization filters is read from the database.
    #[test]
    fn only_a_plain_conditioned_find_is_cached() {
        let meta = ReadMeta {
            full_name: "E".to_string(),
            is_view: false,
            cache: true,
            cache_never: false,
            pk_fields: Vec::new(),
        };
        let lookup =
            |condition: Option<Condition>, limit: Option<i64>, offset: Option<i64>, filtered: bool| FindLookup {
                entity_name: "E".to_string(),
                use_cache: None,
                condition,
                order: Vec::new(),
                limit,
                offset,
                filtered,
            };
        let some = || Some(Condition::equals("a", Value::string("1")));
        assert!(meta.caches(&lookup(some(), None, None, false)));
        assert!(!meta.caches(&lookup(None, None, None, false)));
        assert!(!meta.caches(&lookup(some(), Some(10), None, false)));
        assert!(!meta.caches(&lookup(some(), None, Some(0), false)));
        assert!(!meta.caches(&lookup(some(), None, None, true)));
    }

    #[test]
    fn held_rows_answer_ignore_case_and_field_comparisons() {
        let fields = row(&[
            ("name", Value::string("Gamma")),
            ("low", Value::I32(1)),
            ("high", Value::I32(5)),
        ]);
        let same_case = Condition::ignore_case(Condition::equals("name", Value::string("gAMMA")));
        assert!(eval_condition(&same_case, &fields));
        let other = Condition::ignore_case(Condition::like("name", Value::string("GAM%")));
        assert!(eval_condition(&other, &fields));
        assert!(!eval_condition(
            &Condition::ignore_case(Condition::equals("name", Value::string("beta"))),
            &fields
        ));
        assert!(eval_condition(
            &Condition::compare_fields("low", CompareOp::Less, "high"),
            &fields
        ));
        assert!(!eval_condition(
            &Condition::compare_fields("low", CompareOp::GreaterEqual, "high"),
            &fields
        ));
        assert!(eval_condition(
            &Condition::compare_fields("low", CompareOp::NotEquals, "high"),
            &fields
        ));
    }

    #[test]
    fn handled_is_a_write_gate() {
        assert_eq!(WriteGate::Handled, WriteGate::Handled);
        assert_ne!(WriteGate::Proceed, WriteGate::Handled);
    }

    const TEST: &str = "moqui.test.TestEntity";
    const TYPE: &str = "moqui.basic.EnumerationType";

    fn put(facade: &EntityFacade, entity: &str, key: &str, id: &str) {
        let mut value = facade.make_value(entity).unwrap();
        value.set(key, Value::string(id));
        value.create(facade).unwrap();
    }

    fn found(facade: &EntityFacade, entity: &str, key: &str, id: &str) -> bool {
        facade
            .find(entity)
            .unwrap()
            .eq(key, Value::string(id))
            .one()
            .unwrap()
            .is_some()
    }

    fn run<T>(
        tx: &TransactionManager,
        attr: TransactionAttribute,
        body: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        tx.run(attr, None, body)
    }

    fn statements(tape: &Arc<Mutex<Vec<String>>>, needle: &str) -> usize {
        tape.lock().unwrap().iter().filter(|sql| sql.contains(needle)).count()
    }

    /// The overlay: an update of a row the condition did not load comes back whole (stored row plus the update), a
    /// find by key sees the held write, and a view read writes the held rows first.
    #[test]
    fn held_updates_merge_onto_the_stored_row_and_a_view_read_flushes_them() {
        let facade = testing::open();
        let entity = "moqui.basic.Enumeration";
        for (id, kind) in [("E1", "A"), ("E2", "A")] {
            let mut value = facade.make_value(entity).unwrap();
            value.set("enumId", Value::string(id));
            value.set("enumTypeId", Value::string(kind));
            value.set("description", Value::string("stored"));
            value.create(&facade).unwrap();
        }
        let mut kind = facade.make_value("moqui.basic.EnumerationType").unwrap();
        kind.set("enumTypeId", Value::string("B"));
        kind.create(&facade).unwrap();
        let tx = facade.transactions_for(entity).unwrap();
        run(&tx, TransactionAttribute::Cache, || {
            let mut moved = facade.make_value(entity).unwrap();
            moved.set("enumId", Value::string("E1"));
            moved.set("enumTypeId", Value::string("B"));
            moved.update(&facade)?;
            let of_b = facade.find(entity)?.eq("enumTypeId", Value::string("B")).list()?;
            assert_eq!(of_b.len(), 1);
            assert_eq!(of_b.values()[0].get("description"), Some(&Value::string("stored")));
            let by_key = facade.find(entity)?.eq("enumId", Value::string("E1")).one()?.unwrap();
            assert_eq!(by_key.get("enumTypeId"), Some(&Value::string("B")));
            // A row no held write touches is read from the database by its key.
            let other = facade.find(entity)?.eq("enumId", Value::string("E2")).one()?.unwrap();
            assert_eq!(other.get("enumTypeId"), Some(&Value::string("A")));
            let view = facade
                .find("moqui.basic.EnumerationAndType")?
                .eq("enumTypeId", Value::string("B"))
                .list()?;
            assert_eq!(view.len(), 1);
            Ok(())
        })
        .unwrap();
    }

    /// Java's `TransactionCacheDb.nextSeq`: a sim takes ids from its own bank, not the production sequence.
    #[test]
    fn a_sim_takes_its_ids_from_its_own_bank() {
        assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), None);
        super::SIM.with(|slot| {
            slot.borrow_mut().push(super::SimSession::new("sim_test".to_string()));
        });
        assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(900_000_001));
        assert_eq!(super::sim_next_seq("moqui.test.TestEntity"), Some(900_000_002));
        super::SIM.with(|slot| slot.borrow_mut().clear());
    }

    /// Java's `TransactionCacheDb.noteWrite`: a transaction that wrote to the sim and rolled back ends the sim; one
    /// that committed, or a write with no transaction, does not.
    #[test]
    fn a_rolled_back_sim_write_poisons_the_sim() {
        let facade = testing::open();
        let tx = facade.transactions_for(TEST).unwrap();
        super::SIM.with(|slot| {
            slot.borrow_mut().push(super::SimSession::new("sim_poison".to_string()));
        });
        super::note_sim_write(&tx).unwrap();
        tx.begin(None).unwrap();
        super::note_sim_write(&tx).unwrap();
        super::note_sim_write(&tx).unwrap();
        assert_eq!(super::SIM.with(|slot| slot.borrow().last().unwrap().tracked.len()), 1);
        tx.commit().unwrap();
        assert!(super::sim_check_open().is_ok());
        assert_eq!(super::SIM.with(|slot| slot.borrow().last().unwrap().tracked.len()), 0);
        // A transaction that never wrote to the sim is not tracked.
        tx.begin(None).unwrap();
        tx.rollback().unwrap();
        assert!(super::sim_check_open().is_ok());
        tx.begin(None).unwrap();
        super::note_sim_write(&tx).unwrap();
        tx.rollback().unwrap();
        let err = super::sim_check_open().unwrap_err();
        assert!(err.to_string().contains("rolled back"), "{err}");
        assert!(super::note_sim_write(&tx).is_err());
        let err = super::query_sim(&facade, "sim_poison", "SELECT 1", &[]).unwrap_err();
        assert!(err.to_string().contains("terminated"), "{err}");
        assert!(super::sim_check_find(&facade, TEST).is_err());
        // The sim's own records on production are still read.
        assert!(super::sim_check_find(&facade, "moqui.agent.AgentRun").is_ok());
        super::SIM.with(|slot| slot.borrow_mut().clear());
    }

    #[test]
    fn a_failed_inner_force_new_leaves_the_outer_held_rows() {
        let facade = testing::open();
        let tx = facade.transactions_for(TEST).unwrap();
        run(&tx, TransactionAttribute::Cache, || {
            put(&facade, TEST, "testId", "X");
            let inner = run(&tx, TransactionAttribute::ForceNew, || -> Result<(), Error> {
                put(&facade, TYPE, "enumTypeId", "Y");
                Err(Error::new("inner fails"))
            });
            assert!(inner.is_err());
            assert!(found(&facade, TEST, "testId", "X"));
            Ok(())
        })
        .unwrap();
        assert!(found(&facade, TEST, "testId", "X"));
        assert!(!found(&facade, TYPE, "enumTypeId", "Y"));
    }

    #[test]
    fn an_inner_force_new_commit_does_not_flush_the_outer_held_rows() {
        let facade = testing::open();
        let tx = facade.transactions_for(TEST).unwrap();
        let outer = run(&tx, TransactionAttribute::Cache, || -> Result<(), Error> {
            put(&facade, TEST, "testId", "X");
            run(&tx, TransactionAttribute::ForceCache, || {
                assert!(
                    !found(&facade, TEST, "testId", "X"),
                    "a suspended frame's rows are not read"
                );
                put(&facade, TYPE, "enumTypeId", "Y");
                Ok(())
            })?;
            assert!(found(&facade, TEST, "testId", "X"));
            Err(Error::new("outer fails"))
        });
        assert!(outer.is_err());
        assert!(!found(&facade, TEST, "testId", "X"));
        assert!(found(&facade, TYPE, "enumTypeId", "Y"));
    }

    /// On a database, the inner flush of the outer update would wait on the outer row lock forever.
    #[test]
    fn a_held_update_of_a_locked_row_stays_with_its_frame() {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = testing::open_with_tape(Arc::clone(&tape));
        put(&facade, TEST, "testId", "R");
        let tx = facade.transactions_for(TEST).unwrap();
        run(&tx, TransactionAttribute::Cache, || {
            let mut locked = facade
                .find(TEST)?
                .eq("testId", Value::string("R"))
                .for_update(true)
                .one()?
                .expect("R");
            locked.set("testMedium", Value::string("held"));
            locked.update(&facade)?;
            run(&tx, TransactionAttribute::ForceNew, || {
                put(&facade, TYPE, "enumTypeId", "Y");
                Ok(())
            })?;
            assert_eq!(
                statements(&tape, "UPDATE \"test_entity\""),
                0,
                "{:?}",
                tape.lock().unwrap()
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(statements(&tape, "UPDATE \"test_entity\""), 1);
        let row = facade
            .find(TEST)
            .unwrap()
            .eq("testId", Value::string("R"))
            .one()
            .unwrap();
        assert_eq!(
            row.and_then(|row| row.get("testMedium").cloned()),
            Some(Value::string("held"))
        );
    }

    #[test]
    fn the_cache_attribute_holds_until_commit_and_conf_false_turns_it_off() {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = testing::open_with_tape(Arc::clone(&tape));
        let tx = facade.transactions_for(TEST).unwrap();
        run(&tx, TransactionAttribute::UseOrBegin, || {
            put(&facade, TEST, "testId", "Plain");
            assert_eq!(statements(&tape, "INSERT"), 1, "use-or-begin writes through");
            Ok(())
        })
        .unwrap();
        run(&tx, TransactionAttribute::Cache, || {
            put(&facade, TEST, "testId", "Held");
            assert_eq!(statements(&tape, "INSERT"), 1, "cache holds the insert");
            assert!(found(&facade, TEST, "testId", "Held"));
            Ok(())
        })
        .unwrap();
        assert_eq!(statements(&tape, "INSERT"), 2);
        assert!(found(&facade, TEST, "testId", "Held"));

        tx.set_use_transaction_cache(false);
        run(&tx, TransactionAttribute::Cache, || {
            assert!(!tx.transaction_cache_active());
            put(&facade, TEST, "testId", "Off");
            assert_eq!(
                statements(&tape, "INSERT"),
                3,
                "use-transaction-cache false writes through"
            );
            Ok(())
        })
        .unwrap();
        assert!(found(&facade, TEST, "testId", "Off"));
    }

    #[test]
    fn flush_and_disable_writes_the_held_rows_once_and_stops_holding() {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = testing::open_with_tape(Arc::clone(&tape));
        let tx = facade.transactions_for(TEST).unwrap();
        run(&tx, TransactionAttribute::Cache, || {
            put(&facade, TEST, "testId", "Held");
            assert_eq!(statements(&tape, "INSERT"), 0, "cache holds the insert");
            super::flush_and_disable(&facade, &tx)?;
            assert!(!tx.transaction_cache_active());
            assert_eq!(statements(&tape, "INSERT"), 1, "the held insert is written now");
            put(&facade, TEST, "testId", "After");
            assert_eq!(statements(&tape, "INSERT"), 2, "a later write goes through");
            Ok(())
        })
        .unwrap();
        assert_eq!(statements(&tape, "INSERT"), 2, "the commit writes nothing twice");
        assert!(found(&facade, TEST, "testId", "Held"));
        assert!(found(&facade, TEST, "testId", "After"));
    }

    #[test]
    fn a_cached_enumeration_is_shared_until_set() {
        let facade = testing::open();
        let mut made = facade.make_value("moqui.basic.Enumeration").unwrap();
        made.set("enumId", Value::string("Share1"));
        made.set("enumTypeId", Value::string("ShareType"));
        made.create(&facade).unwrap();
        let load = || {
            facade
                .find("moqui.basic.Enumeration")
                .unwrap()
                .eq("enumId", Value::string("Share1"))
                .use_cache(true)
                .one()
                .unwrap()
                .unwrap()
        };
        let mut first = load();
        let second = load();
        assert!(first.shares_fields(&second), "a cache hit shares the stored row");
        first.set("description", Value::string("changed"));
        assert!(!first.shares_fields(&second), "set copies the row");
        assert!(second.get("description").is_none_or(Value::is_null));
        assert_eq!(first.get("description").and_then(Value::as_str), Some("changed"));
        let third = load();
        assert!(second.shares_fields(&third), "the cache keeps the unmodified row");
        assert!(!first.shares_fields(&third));
    }

    #[test]
    fn find_keys_keep_sub_millisecond_timestamps_and_value_types_apart() {
        let key = |value: Value| {
            super::owned_lookup_key(&FindLookup {
                entity_name: "moqui.test.TestEntity".to_string(),
                use_cache: Some(true),
                condition: Some(Condition::equals("testDateTime", value)),
                order: Vec::new(),
                limit: None,
                offset: None,
                filtered: false,
            })
        };
        let stamp = |micros: i64| Value::Timestamp(jiff::Timestamp::from_microsecond(micros).unwrap());
        assert_ne!(key(stamp(1_700_000_000_000_001)), key(stamp(1_700_000_000_000_002)));
        assert_eq!(key(stamp(1_700_000_000_000_001)), key(stamp(1_700_000_000_000_001)));
        assert_ne!(key(Value::string("1")), key(Value::I64(1)));
    }

    #[test]
    fn an_and_in_either_order_is_one_cache_entry() {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = testing::open_with_tape(Arc::clone(&tape));
        let mut made = facade.make_value("moqui.basic.Enumeration").unwrap();
        made.set("enumId", Value::string("And1"));
        made.set("enumTypeId", Value::string("AndType"));
        made.set("description", Value::string("hello"));
        made.create(&facade).unwrap();
        let find = |first_field: &str, first: &str, second_field: &str, second: &str| {
            facade
                .find("moqui.basic.Enumeration")
                .unwrap()
                .condition(Condition::and(vec![
                    Condition::equals(first_field, Value::string(first)),
                    Condition::equals(second_field, Value::string(second)),
                ]))
                .use_cache(true)
                .list()
                .unwrap()
                .len()
        };
        assert_eq!(find("enumId", "And1", "enumTypeId", "AndType"), 1);
        let selects = statements(&tape, "SELECT");
        assert_eq!(find("enumTypeId", "AndType", "enumId", "And1"), 1);
        assert_eq!(statements(&tape, "SELECT"), selects, "the reversed and hits the cache");
    }

    #[test]
    fn a_cached_find_of_an_entity_the_frame_wrote_is_not_kept_after_rollback() {
        let facade = testing::open();
        let tx = facade.transactions_for(TYPE).unwrap();
        assert!(tx.begin(None).unwrap());
        put(&facade, TYPE, "enumTypeId", "PHANTOM");
        assert!(
            found(&facade, TYPE, "enumTypeId", "PHANTOM"),
            "the frame reads its own write"
        );
        tx.rollback().unwrap();
        assert!(
            !found(&facade, TYPE, "enumTypeId", "PHANTOM"),
            "the rolled-back row was not cached"
        );
        assert!(super::TX_WRITTEN.with(|slot| slot.borrow().is_empty()));
    }

    #[test]
    fn a_commit_clears_rows_another_thread_cached_before_it() {
        let facade = testing::open();
        assert!(!found(&facade, TYPE, "enumTypeId", "NEW"), "caches the empty result");
        let tx = facade.transactions_for(TYPE).unwrap();
        assert!(tx.begin(None).unwrap());
        put(&facade, TYPE, "enumTypeId", "NEW");
        let reader = facade.clone();
        let before_commit = std::thread::spawn(move || found(&reader, TYPE, "enumTypeId", "NEW"))
            .join()
            .unwrap();
        assert!(
            !before_commit,
            "another thread reads the committed rows and caches them"
        );
        assert!(
            found(&facade, TYPE, "enumTypeId", "NEW"),
            "the writing frame does not read the other thread's cached rows"
        );
        tx.commit().unwrap();
        assert!(
            found(&facade, TYPE, "enumTypeId", "NEW"),
            "the commit cleared the cache"
        );
        assert!(super::TX_WRITTEN.with(|slot| slot.borrow().is_empty()));
    }

    /// The writes of each open frame kept for `full_name`: `None` when the frame keeps none (clears it whole), else
    /// how many.
    fn kept_writes(full_name: &str) -> Vec<Option<usize>> {
        super::TX_WRITES.with(|slot| {
            slot.borrow()
                .values()
                .filter_map(|entities| entities.get(full_name).map(|writes| writes.as_ref().map(Vec::len)))
                .collect()
        })
    }

    fn found_by(facade: &EntityFacade, field: &str, value: &str) -> usize {
        facade
            .find(TYPE)
            .unwrap()
            .eq(field, Value::string(value))
            .use_cache(true)
            .list()
            .unwrap()
            .len()
    }

    /// A frame's update of an entity with a cached find keeps the values written, and the commit evicts a find by a
    /// field outside the primary key that another thread cached before it.
    #[test]
    fn an_update_evicts_a_find_by_another_field_at_commit() {
        let facade = testing::open();
        let mut row = facade.make_value(TYPE).unwrap();
        row.set("enumTypeId", Value::string("UPD"));
        row.set("description", Value::string("before"));
        row.create(&facade).unwrap();
        assert_eq!(found_by(&facade, "description", "after"), 0, "caches the empty result");
        let tx = facade.transactions_for(TYPE).unwrap();
        assert!(tx.begin(None).unwrap());
        row.set("description", Value::string("after"));
        row.update(&facade).unwrap();
        assert_eq!(kept_writes(TYPE), vec![Some(1)]);
        let reader = facade.clone();
        let before_commit = std::thread::spawn(move || found_by(&reader, "description", "after"))
            .join()
            .unwrap();
        assert_eq!(before_commit, 0, "another thread caches the committed rows");
        tx.commit().unwrap();
        assert_eq!(found_by(&facade, "description", "after"), 1, "the commit evicted the find");
    }

    /// A frame's write of an entity with no cached find or count keeps no values; a find another thread caches during
    /// the frame, by a field the write set, is still cleared at the commit.
    #[test]
    fn a_find_cached_during_the_frame_is_cleared_at_commit_without_kept_values() {
        let facade = testing::open();
        let tx = facade.transactions_for(TYPE).unwrap();
        assert!(tx.begin(None).unwrap());
        let mut row = facade.make_value(TYPE).unwrap();
        row.set("enumTypeId", Value::string("LATE"));
        row.set("description", Value::string("late"));
        row.create(&facade).unwrap();
        assert_eq!(kept_writes(TYPE), vec![None], "nothing cached, nothing kept");
        let reader = facade.clone();
        let before_commit = std::thread::spawn(move || found_by(&reader, "description", "late"))
            .join()
            .unwrap();
        assert_eq!(before_commit, 0, "another thread caches the committed rows");
        tx.commit().unwrap();
        assert_eq!(found_by(&facade, "description", "late"), 1, "the commit cleared the entity's caches");
        assert!(kept_writes(TYPE).is_empty());
    }
}
