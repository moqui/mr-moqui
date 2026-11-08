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
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use moqui_db::dialect::Dialect;
use moqui_db::model::EntityDefinition;
use moqui_db::sql::{
    create_table, delete, insert, insert_or_update, select_with_head, update, CompareOp, Condition, FieldRef,
    LimitOffset, Nulls, OrderBy,
};
use moqui_lib::cache::{CacheConfig, MCache};
use moqui_lib::error::Error;
use moqui_lib::object::{compare_like, compare_obj, to_plain_string, Value};

use super::exec::{execute_conn, execute_script, query_conn, query_tx};
use super::facade::EntityFacade;
use super::field_rw;
use super::params::{entity_columns, map_row};
use super::value::FieldMap;
use super::value::{EntityList, EntityValue};
use crate::driver::{DbConnection, DbRow};
use crate::transaction::{CacheMode, TransactionManager};

mod batch;
mod hold;
mod stats;
mod tx;

use hold::Held;
pub(crate) use stats::{census_frame, census_measures, end_frame as end_frame_stats};
pub use stats::{tx_cache_stats, FlushCause, TxCacheStats};
pub(crate) use tx::member_shadows;
use tx::overlay_rows;
pub(crate) use tx::read_shadows;
pub use tx::{flush_and_disable, flush_before_sql, register_cache_flush};

const LIST_MAX: usize = 10_000;
const PK_SEP: char = '\u{1f}';

static SIM_SEQ: AtomicU64 = AtomicU64::new(1);

thread_local! {
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
/// overlay. `on_write` still runs after either variant and is the one-cache invalidation. `Missed` is a sim write that
/// wrote nothing, as on the database: an update probe that matched no row, or a create-if-missing of a row that is there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteGate {
    Proceed,
    Handled,
    Missed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Create,
    Update,
    Delete,
    Upsert,
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
    /// Each type-one relationship: the related entity and the key pairs (this field, related field), for a list find
    /// whose condition pins one to a row the frame created (plans/TX-Cache.md D3).
    one_keys: Vec<(String, Vec<(String, String)>)>,
}

impl ReadMeta {
    /// Java `shouldCache` and the list find: no cache for a find with no condition, a limit or offset, or the user's
    /// authorization filters.
    fn caches(&self, lookup: &FindLookup) -> bool {
        lookup.condition.is_some() && self.caches_count(lookup)
    }

    /// [`Self::caches`] for a count, which is kept with no condition too: a count of all rows is one number, and every
    /// write of the entity drops it (an unconditioned count matches any row).
    fn caches_count(&self, lookup: &FindLookup) -> bool {
        if lookup.limit.is_some() || lookup.offset.is_some() || lookup.filtered {
            return false;
        }
        match lookup.use_cache {
            Some(true) => !self.cache_never,
            Some(false) => false,
            None => self.cache,
        }
    }
}

/// A row of a find in memory: shared with the frame's kept rows and the values it gives out until a held write
/// changes it.
struct MemRow {
    fields: Arc<FieldMap>,
    trusted: bool,
}

struct SimSession {
    schema: String,
    tables: HashSet<String>,
    tombstones: HashMap<String, HashSet<String>>,
    written: HashSet<String>,
    /// Java's `TransactionCacheDb.seqBanks`: the sim's own sequenced ids, so a sim uses no production ids. Each is the
    /// last id given out for its name, counting down from [`SIM_SEQ_START`].
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
    /// Step savepoints open on this sim ([`super::sim_step`]). While one is, a rolled-back transaction fails the step
    /// instead of ending the sim: the step's savepoint undoes what it wrote.
    steps: usize,
}

/// What a step savepoint restores of a sim session ([`super::sim_step`]): which tables, rows, replicas and tombstones
/// the sim has, and whether it is poisoned. Not the sequence banks: an id a rolled-back step took is not given out
/// again, so an id names one step's row over the whole run.
#[derive(Clone)]
pub(super) struct SimState {
    tables: HashSet<String>,
    tombstones: HashMap<String, HashSet<String>>,
    written: HashSet<String>,
    replicas: HashSet<String>,
    copied: usize,
    poisoned: Option<String>,
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
            steps: 0,
        }
    }
}

/// The innermost sim session's state, for a step savepoint to restore; none without a session.
pub(super) fn sim_state() -> Option<SimState> {
    SIM.with(|slot| {
        slot.borrow().last().map(|session| SimState {
            tables: session.tables.clone(),
            tombstones: session.tombstones.clone(),
            written: session.written.clone(),
            replicas: session.replicas.clone(),
            copied: session.copied,
            poisoned: session.poisoned.clone(),
        })
    })
}

/// Put back the innermost sim session's state as [`sim_state`] took it, after its step was rolled back to the savepoint.
pub(super) fn restore_sim_state(state: SimState) {
    SIM.with(|slot| {
        if let Some(session) = slot.borrow_mut().last_mut() {
            session.tables = state.tables;
            session.tombstones = state.tombstones;
            session.written = state.written;
            session.replicas = state.replicas;
            session.copied = state.copied;
            session.poisoned = state.poisoned;
        }
    });
}

/// Count a step savepoint opened (`true`) or ended (`false`) on the innermost sim session.
pub(super) fn note_sim_step(open: bool) {
    SIM.with(|slot| {
        if let Some(session) = slot.borrow_mut().last_mut() {
            session.steps = if open {
                session.steps + 1
            } else {
                session.steps.saturating_sub(1)
            };
        }
    });
}

/// Poison the innermost sim session with `reason` unless it is poisoned already.
pub(super) fn poison_sim(reason: &str) {
    SIM.with(|slot| {
        if let Some(session) = slot.borrow_mut().last_mut() {
            if session.poisoned.is_none() {
                tracing::warn!("Sim overlay terminated: {reason}");
                session.poisoned = Some(reason.to_string());
            }
        }
    });
}

const SIM_ROLLED_BACK: &str =
    "A transaction that wrote to the sim was rolled back, and sim writes are not undone, so the sim \
     state can no longer be trusted. The sim has been terminated. Start a new sim to continue testing.";

/// The poison of a sim inside a step savepoint: the step fails and its savepoint undoes what it wrote.
const SIM_STEP_ROLLED_BACK: &str = "A transaction that wrote to the sim was rolled back, so the step was rolled back.";

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
            if session.steps > 0 {
                tracing::info!("Sim step failed: {reason}");
                session.poisoned = Some(SIM_STEP_ROLLED_BACK.to_string());
            } else {
                tracing::warn!("Sim overlay terminated: {reason}");
                session.poisoned = Some(reason.to_string());
            }
        }
    });
}

/// Java's `TransactionCacheDb.TOTAL_COPY_CAP`: rows copied from production over one sim, which bounds its size.
const SIM_TOTAL_COPY_CAP: usize = 200_000;

/// Where the ids of a sim's sequences start: each name counts down from here (-1, -2, ...). Java's
/// `TransactionCacheDb.SEQ_START` counts up from 900,000,000, which a production sequence can reach; then a sim's
/// create gets the id of a production row, its row shadows that one, and a preview shows an update of the real record
/// instead of a create. A production sequence never hands out an id below 1.
const SIM_SEQ_START: i64 = 0;

/// Java's `TransactionCacheDb.PRODUCTION_ENTITIES`: written to the real database during a sim. The agent's bookkeeping
/// (a preview runs inside a run: its conversation, run, steps, events, messages, tool calls and approvals, and the
/// provider calls and their usage, plans/LLM-A2A-Assist.md section 5), the Constitution snapshot a run names by its
/// hash (stored once per hash, never changed), and the sequence banks.
const SIM_PRODUCTION_ENTITIES: &[&str] = &[
    "moqui.agent.AgentConversation",
    "moqui.agent.AgentRun",
    "moqui.agent.AgentConstitutionSnapshot",
    "moqui.agent.AgentRunStep",
    "moqui.agent.AgentRunEvent",
    "moqui.agent.AgentMessage",
    "moqui.agent.AgentMessagePart",
    "moqui.agent.AgentToolCall",
    "moqui.agent.AgentApproval",
    "moqui.llm.LlmCall",
    "moqui.llm.LlmUsageDaily",
    "moqui.llm.LlmQuotaReservation",
    "moqui.llm.LlmQuotaLock",
    "moqui.entity.SequenceValueItem",
];

/// The next id of `seq_name` from the sim's own bank, when a sim session is open on this thread: -1, then -2, and so
/// on, below every production id. See [`SIM_SEQ_START`]. An entity the sim leaves on production
/// ([`SIM_PRODUCTION_ENTITIES`]) takes its ids from the production sequence, since its rows are real: a sim id there
/// would be the same -1 the next sim gives out.
pub(crate) fn sim_next_seq(seq_name: &str) -> Option<i64> {
    if SIM_PRODUCTION_ENTITIES.contains(&seq_name) {
        return None;
    }
    SIM.with(|slot| {
        let mut sessions = slot.borrow_mut();
        let session = sessions.last_mut()?;
        let next = session.seq_banks.entry(seq_name.to_string()).or_insert(SIM_SEQ_START);
        *next -= 1;
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
    /// The store fence of each entity whose find has taken one ([`store_fence`]): moved before each eviction or clear
    /// of its caches. An entity with none has no find between its fence and its store, so a write need not move one.
    generations: HashMap<String, Arc<AtomicU64>>,
    /// Moved before every cache is cleared at once ([`clear_all_list_caches`]).
    epoch: Arc<AtomicU64>,
}

/// What a find took of its entity's store fence before its query (plans/High-Level-Review.md HL-023). Its result is
/// stored only when no eviction or clear of the entity's caches began since: under the cache's write lock, which the
/// eviction takes after moving the fence, so rows read before a commit are either refused or evicted after their
/// store. As [`super::derived`] does for the definitions it builds.
pub struct StoreFence {
    full_name: String,
    generation: Arc<AtomicU64>,
    epoch: Arc<AtomicU64>,
    taken: u64,
}

impl StoreFence {
    /// Both counts only grow, so their sum moves when either does.
    fn current(generation: &AtomicU64, epoch: &AtomicU64) -> u64 {
        generation
            .load(AtomicOrdering::SeqCst)
            .wrapping_add(epoch.load(AtomicOrdering::SeqCst))
    }

    fn holds(&self) -> bool {
        Self::current(&self.generation, &self.epoch) == self.taken
    }
}

/// The store fence of a find, taken before its query; `None` when its result is not to be stored: the entity's finds
/// are not cached ([`ReadMeta::caches`]), this frame wrote the entity (its rows may be uncommitted), or the frame reads
/// one snapshot ([`EntityFacade::snapshot_frame`]), whose rows may predate a commit already evicted for.
pub fn store_fence(facade: &EntityFacade, lookup: &FindLookup) -> Option<StoreFence> {
    store_fence_of(facade, lookup, false)
}

/// [`store_fence`] for a count, which is kept with no condition too ([`ReadMeta::caches_count`]).
pub fn store_count_fence(facade: &EntityFacade, lookup: &FindLookup) -> Option<StoreFence> {
    store_fence_of(facade, lookup, true)
}

fn store_fence_of(facade: &EntityFacade, lookup: &FindLookup, count: bool) -> Option<StoreFence> {
    let meta = read_meta(facade, &lookup.entity_name).ok()?;
    let caches = if count {
        meta.caches_count(lookup)
    } else {
        meta.caches(lookup)
    };
    if !caches || frame_wrote(facade, &lookup.entity_name, &meta) || facade.snapshot_frame(&lookup.entity_name) {
        return None;
    }
    let name = &meta.full_name;
    let (known, epoch) = facade.with_slot_read(OneCaches::default, |caches| {
        (caches.generations.get(name).cloned(), Arc::clone(&caches.epoch))
    });
    // Made under the slot's write lock, which a writer's move reads under: a write that saw no fence committed before
    // this one was made, so the query below sees it.
    let generation = known.unwrap_or_else(|| {
        facade.with_slot(OneCaches::default, |caches| {
            Arc::clone(caches.generations.entry(name.clone()).or_default())
        })
    });
    let taken = StoreFence::current(&generation, &epoch);
    Some(StoreFence {
        full_name: name.clone(),
        generation,
        epoch,
        taken,
    })
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

/// Writes of an open frame, by entity, replayed against the caches when it ends. Past this many the entity's caches are
/// cleared whole instead.
const FRAME_WRITES_MAX: usize = 200;

/// Look up a cached list, or rows held by the active transaction frame or the sim overlay.
///
/// `Some` skips SQL. Plaintext only: `after_find` stored decrypted rows, and held rows were
/// normalized before `gate_write`. An error of the overlay (a held create of a row the database has, plans/TX-Cache.md
/// G5) or of a view's early flush fails the find: the database alone would answer without the frame's held rows.
pub fn before_find(facade: &EntityFacade, lookup: &FindLookup) -> Result<Option<EntityList>, Error> {
    let meta = read_meta(facade, &lookup.entity_name)?;
    // A view is read from the database: its read sees the frame's held rows of its members through shadowed member
    // tables, or with them written first (`find::plan`, `tx::view_shadows`).
    if !meta.is_view {
        let page = LimitOffset {
            limit: lookup.limit,
            offset: lookup.offset,
        };
        if let Some(rows) = overlay_rows(
            facade,
            &meta,
            lookup.condition.as_ref(),
            &lookup.order,
            page,
            None,
            Some(lookup),
        )? {
            return Ok(Some(EntityList::new(page_rows(rows, lookup.limit, lookup.offset))));
        }
    }
    if meta.caches(lookup) && !frame_wrote(facade, &lookup.entity_name, &meta) {
        return Ok(cache_get(facade, &meta.full_name, lookup));
    }
    Ok(None)
}

/// Count held and sim rows when this thread has overlaid that entity. Otherwise miss.
///
/// `Some` is the count itself. The merged list is unlimited, so its length is that count. Errors as [`before_find`].
pub fn before_count(facade: &EntityFacade, lookup: &FindLookup) -> Result<Option<i64>, Error> {
    let meta = read_meta(facade, &lookup.entity_name)?;
    if meta.is_view {
        // read as `before_find` reads a view
    } else if let Some(rows) = overlay_rows(
        facade,
        &meta,
        lookup.condition.as_ref(),
        &[],
        LimitOffset::default(),
        None,
        None,
    )? {
        return Ok(Some(i64::try_from(rows.len()).unwrap_or(i64::MAX)));
    }
    if meta.caches_count(lookup) && !frame_wrote(facade, &lookup.entity_name, &meta) {
        let Some(cache) =
            facade.with_slot_read(OneCaches::default, |caches| caches.counts.get(&meta.full_name).cloned())
        else {
            return Ok(None);
        };
        return Ok(with_lookup_key(lookup, |key| {
            cache.get_arc(key).ok().flatten().map(|cached| cached.count)
        }));
    }
    Ok(None)
}

/// What a find asks beyond its condition, order and page, which the cache and the overlay do not key on.
#[derive(Clone, Debug, Default)]
pub struct Selective {
    /// `for-update`, and with it `skip_locked`.
    pub lock: Option<bool>,
    /// The fields selected; empty for every field.
    pub fields: Vec<String>,
    /// `distinct`, a `having` condition or an aggregate: the rows the database gives are not the entity's rows.
    pub aggregate: bool,
}

/// A selective find (plans/TX-Cache.md D4, G1) in a frame that holds rows: a view writes the held rows of its members
/// first, as any view read; a `distinct`, `having` or aggregate find of an entity the frame holds rows of writes the
/// held writes up to the last of that entity first (an early flush, cause "selective find") and leaves the read to the
/// database; a `for-update` or select-field find reads whole rows through the overlay (locking them, for update) and
/// returns the fields asked for. `None` when the database answers alone.
pub fn before_selective_find(
    facade: &EntityFacade,
    lookup: &FindLookup,
    selective: &Selective,
) -> Result<Option<EntityList>, Error> {
    let meta = read_meta(facade, &lookup.entity_name)?;
    if meta.is_view {
        // read as `before_find` reads a view
        return Ok(None);
    }
    // A locking find goes through the frame even when it holds nothing of the entity, so the frame keeps the rows it
    // locked and knows them locked (a held update of one needs no check at the commit).
    let holds = tx::holds_entity(facade, &meta.full_name);
    if !holds && projected_key(lookup, selective).is_some() {
        return Ok(tx::projected_rows(facade, &meta, lookup, selective));
    }
    if !holds && (selective.lock.is_none() || selective.aggregate) {
        return Ok(None);
    }
    if selective.aggregate {
        // read by the database with the held rows shadowed, or written first (`find::plan`, `tx::entity_shadows`)
        return Ok(None);
    }
    let page = LimitOffset {
        limit: lookup.limit,
        offset: lookup.offset,
    };
    let Some(rows) = overlay_rows(
        facade,
        &meta,
        lookup.condition.as_ref(),
        &lookup.order,
        page,
        selective.lock,
        None,
    )?
    else {
        return Ok(None);
    };
    let mut rows = page_rows(rows, lookup.limit, lookup.offset);
    if !selective.fields.is_empty() {
        let name: Arc<str> = Arc::from(meta.full_name.as_str());
        rows = rows
            .into_iter()
            .map(|row| {
                let fields = row
                    .into_fields()
                    .into_iter()
                    .filter(|(field, _)| selective.fields.iter().any(|wanted| wanted == &**field))
                    .collect();
                EntityValue::from_fields(Arc::clone(&name), fields)
            })
            .collect();
    }
    Ok(Some(EntityList::new(rows)))
}

/// Drop what the transaction-cache frame of `entity_name` read of that entity, when there is one: a transaction of its
/// own committed rows of it the frame read as missing (a directory made beside the frame), and a read again sees them.
/// Held writes stay.
pub fn forget_frame_reads(facade: &EntityFacade, entity_name: &str) {
    let Ok(tx) = facade.transactions_for(entity_name) else {
        return;
    };
    let Some((token, _)) = tx.cache_frame() else {
        return;
    };
    if let Ok(meta) = read_meta(facade, entity_name) {
        tx::forget_reads(token, Some(&meta.full_name));
    }
}

/// `values` as the database of `entity_name` will give them back: time stamps at the precision it keeps
/// ([`stored_timestamp`]), and with `every_field`, every field of the entity, null where it has none.
fn as_stored(
    facade: &EntityFacade,
    entity_name: &str,
    values: &FieldMap,
    every_field: bool,
) -> Result<FieldMap, Error> {
    facade.with_entity(entity_name, |entity, dialect, _| {
        let mut all = values.clone();
        if let Some(digits) = dialect.timestamp_digits() {
            for value in all.values_mut() {
                if let Value::Timestamp(stamp) = value {
                    *stamp = stored_timestamp(*stamp, digits, dialect.name.starts_with("postgres"));
                }
            }
        }
        if every_field {
            for field in &entity.fields {
                if !all.contains_key(field.name.as_str()) {
                    all.insert(Arc::from(field.name.as_str()), Value::Null);
                }
            }
        }
        Ok(all)
    })
}

/// `stamp` as a column keeping `digits` fractional digits of seconds stores it. Postgres rounds to the nearest
/// microsecond, a half to even (`rint`); MySQL is sent six digits (cut) and rounds those half up to its own.
fn stored_timestamp(stamp: jiff::Timestamp, digits: u8, postgres: bool) -> jiff::Timestamp {
    use jiff::{RoundMode, TimestampRound, Unit};
    let round = |stamp: jiff::Timestamp, unit: Unit, increment: i64, mode: RoundMode| {
        stamp
            .round(TimestampRound::new().smallest(unit).increment(increment).mode(mode))
            .unwrap_or(stamp)
    };
    if postgres {
        return round(stamp, Unit::Microsecond, 1, RoundMode::HalfEven);
    }
    let cut = round(stamp, Unit::Microsecond, 1, RoundMode::Trunc);
    match digits {
        0 => round(cut, Unit::Second, 1, RoundMode::HalfExpand),
        1 => round(cut, Unit::Millisecond, 100, RoundMode::HalfExpand),
        2 => round(cut, Unit::Millisecond, 10, RoundMode::HalfExpand),
        3 => round(cut, Unit::Millisecond, 1, RoundMode::HalfExpand),
        4 => round(cut, Unit::Microsecond, 100, RoundMode::HalfExpand),
        5 => round(cut, Unit::Microsecond, 10, RoundMode::HalfExpand),
        _ => cut,
    }
}

/// The key a select-field find is kept by in a cache frame (`tx::projected_rows`): the find's own key and its fields.
/// `None` for one that is not kept: a lock, an aggregate, a page, or no fields named.
fn projected_key(lookup: &FindLookup, selective: &Selective) -> Option<String> {
    if selective.lock.is_some()
        || selective.aggregate
        || selective.fields.is_empty()
        || lookup.limit.is_some()
        || lookup.offset.is_some()
    {
        return None;
    }
    let mut key = owned_lookup_key(lookup);
    key.push_str("\u{1f}fields:");
    for field in &selective.fields {
        key.push_str(field);
        key.push(',');
    }
    Some(key)
}

/// After a select-field find read the database in a cache frame that holds no write of the entity: keep its rows, so
/// the same find again in the frame is answered from memory (a rule that runs for each row a service writes reads the
/// same configuration each time). Not kept outside a cache frame, or past the frame's first write of the entity.
pub fn after_selective_find(facade: &EntityFacade, lookup: &FindLookup, selective: &Selective, list: &EntityList) {
    let Some(key) = projected_key(lookup, selective) else {
        return;
    };
    let Ok(meta) = read_meta(facade, &lookup.entity_name) else {
        return;
    };
    if meta.is_view {
        return;
    }
    tx::keep_projected(facade, &meta, key, list);
}

/// A count with `distinct` or `having` in a frame that holds rows of the entity (or a view of it): the held writes go
/// to the database first, as for [`before_selective_find`].
pub fn before_selective_count(facade: &EntityFacade, entity_name: &str) -> Result<(), Error> {
    let meta = read_meta(facade, entity_name)?;
    if meta.is_view {
        // read as `before_find` reads a view
        return Ok(());
    }
    // read by the database with the held rows shadowed, or written first (`find::plan`, `tx::entity_shadows`)
    Ok(())
}

/// Store a count the database gave, as Java's count cache, unless the entity's caches were evicted or cleared since
/// `fence` was taken. True when stored.
pub fn after_count(facade: &EntityFacade, lookup: &FindLookup, fence: &StoreFence, count: i64) -> Result<bool, Error> {
    let key = owned_lookup_key(lookup);
    let name = &fence.full_name;
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
    let value = CachedCount {
        condition: lookup.condition.clone(),
        count,
    };
    cache.put_if(key, value, || fence.holds())
}

/// Store a find result, unless the entity's caches were evicted or cleared since `fence` was taken ([`store_fence`]
/// says which finds are stored). Rows are already plaintext. True when stored.
pub fn after_find(
    facade: &EntityFacade,
    lookup: &FindLookup,
    fence: &StoreFence,
    rows: &EntityList,
) -> Result<bool, Error> {
    let key = owned_lookup_key(lookup);
    let name = &fence.full_name;
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
    let value = CachedList {
        condition: lookup.condition.clone(),
        rows: rows.clone(),
    };
    cache.put_if(key, value, || fence.holds())
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
    tx.on_commit(move || {
        // A frame of its own committed under a frame suspended on this thread (a `force-new` call, a probe in a
        // transaction of its own): what that frame read of the entity is no longer what the database has.
        tx::forget_reads_of_others(token, &commit_name);
        end_frame_write(&commit_facade, token, &commit_name)
    })?;
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
    note_localized(facade, full_name);
    // The writes again, now that other threads may have read and cached the rows as they were before the end: one
    // sweep for all of them.
    let result = match (writes.flatten(), read_meta(facade, full_name)) {
        (Some(writes), Ok(meta)) => evict_frame(facade, &meta, &writes),
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
fn evict(facade: &EntityFacade, meta: &ReadMeta, operation: &str, values: &FieldMap) -> Result<bool, Error> {
    let (lists, counts) = facade.with_slot_read(OneCaches::default, |caches| {
        // The fence first: a find whose query began before this write is not stored after the eviction below.
        if let Some(generation) = caches.generations.get(&meta.full_name) {
            generation.fetch_add(1, AtomicOrdering::SeqCst);
        }
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

#[cfg(test)]
thread_local! {
    /// Frame-end sweeps on this thread, for the tests.
    static FRAME_SWEEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Frame-end sweeps run on this thread so far, for the tests.
#[cfg(test)]
fn frame_sweeps() -> usize {
    FRAME_SWEEPS.with(std::cell::Cell::get)
}

/// [`evict`] for every write a frame kept of one entity, in one sweep of each cache (plans/High-Level-Review.md
/// HL-002): a cached list goes when one of its rows has the key of a delete, update or upsert (keys compared as
/// text, through one buffer), or when its condition may match the values of a create, update or upsert. A write with
/// no whole primary key may change any list, so the lists are cleared. Counts go as [`evict`] takes them; the views
/// of the entity are cleared once. The store fence moves first, once.
fn evict_frame(facade: &EntityFacade, meta: &ReadMeta, writes: &[FrameWrite]) -> Result<(), Error> {
    #[cfg(test)]
    FRAME_SWEEPS.with(|sweeps| sweeps.set(sweeps.get() + 1));
    if writes.is_empty() {
        return Ok(());
    }
    let (lists, counts) = facade.with_slot_read(OneCaches::default, |caches| {
        if let Some(generation) = caches.generations.get(&meta.full_name) {
            generation.fetch_add(1, AtomicOrdering::SeqCst);
        }
        (
            caches.lists.get(&meta.full_name).cloned(),
            caches.counts.get(&meta.full_name).cloned(),
        )
    });
    let mut held_keys: HashSet<String> = HashSet::new();
    let mut any_list = false;
    let mut matching: Vec<&FieldMap> = Vec::new();
    for write in writes {
        let values = &*write.values;
        if write.operation != "create" {
            match pk_of(values, &meta.pk_fields) {
                Some(key) => {
                    held_keys.insert(key);
                }
                None => any_list = true,
            }
        }
        if write.operation != "delete" {
            matching.push(values);
        }
    }
    if let Some(cache) = lists {
        if any_list {
            cache.clear()?;
        } else {
            let mut buffer = String::new();
            cache.remove_where(|_, cached| {
                let held = !held_keys.is_empty()
                    && cached.rows.iter().any(|row| {
                        buffer.clear();
                        write_pk(row.fields(), &meta.pk_fields, &mut buffer) && held_keys.contains(buffer.as_str())
                    });
                held || matching
                    .iter()
                    .any(|values| may_match(cached.condition.as_ref(), values))
            })?;
        }
    }
    if let Some(cache) = counts {
        if writes.iter().any(|write| write.operation != "create") {
            cache.clear()?;
        } else {
            cache.remove_where(|_, cached| {
                matching
                    .iter()
                    .any(|values| may_match(cached.condition.as_ref(), values))
            })?;
        }
    }
    let views = views_by_member(facade);
    for view in views.get(&meta.full_name).into_iter().flatten() {
        clear_named(facade, view)?;
    }
    Ok(())
}

/// Whether a write may change a cached list: a delete when the list held the row, a create when the row may match,
/// an update (or upsert) for either. A write whose primary key is not complete may change any list.
fn list_affected(cached: &CachedList, operation: &str, values: &FieldMap, pk: Option<&[(&str, &Value)]>) -> bool {
    let Some(pk) = pk else {
        return true;
    };
    let held = || {
        cached.rows.iter().any(|row| {
            pk.iter()
                .all(|(name, value)| row.get(name).is_some_and(|stored| same_key(stored, value)))
        })
    };
    match operation {
        "delete" => held(),
        "create" => may_match(cached.condition.as_ref(), values),
        _ => held() || may_match(cached.condition.as_ref(), values),
    }
}

/// Whether a cached row's key field holds the written value: equal values, or the same text when the types differ (a
/// key written as text and read as a number). Two values of one type compare as values, with no strings made: the
/// eviction sweep runs this for every cached row that is not the written one (plans/High-Level-Review.md HL-002).
fn same_key(stored: &Value, value: &Value) -> bool {
    if stored == value {
        return true;
    }
    if std::mem::discriminant(stored) == std::mem::discriminant(value) {
        return false;
    }
    match (stored, value) {
        (Value::String(text), other) | (other, Value::String(text)) => *text == to_plain_string(other),
        _ => to_plain_string(stored) == to_plain_string(value),
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

/// Hold the write, or send it to the sim overlay. `Proceed` leaves SQL to the caller. `quiet` is an update that matches
/// no row, or a create of a row that is there, without an error: `update_if_exists` and `create_if_missing`. `stored` is
/// the row an update read first, which its audit log compares with (plans/TX-Cache.md G6).
///
/// A held write checks what the frame knows of its row (plans/TX-Cache.md G4): an update of a row the frame deleted, or
/// read as missing, fails now, and a quiet update or create that would write nothing is `Missed`; a quiet one of a row
/// the frame knows nothing of reads it first.
pub fn gate_write(
    facade: &EntityFacade,
    entity_name: &str,
    operation: &str,
    values: &FieldMap,
    quiet: bool,
    stored: Option<&FieldMap>,
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
        // A read-through frame writes to the database at once: what it read of the entity is stale.
        if let Some((token, CacheMode::ReadThrough)) = tx.cache_frame() {
            tx::forget_reads(token, Some(&read_meta(facade, entity_name)?.full_name));
        }
        return Ok(WriteGate::Proceed);
    }
    let prepared = prepare_handled(facade, entity_name, operation, values, stored)?;
    if sim {
        if tx.in_transaction() {
            tx.with_connection(|_| Ok(()))?;
        }
        note_sim_write(&tx)?;
        let written = write_sim(facade, entity_name, op, values, &prepared, quiet)?;
        return Ok(if written { WriteGate::Handled } else { WriteGate::Missed });
    }
    if matches!(op, Op::Update | Op::Create) {
        let mut exists = tx::row_known(&tx, &prepared.full_name, &prepared.pk);
        if quiet && exists.is_none() {
            exists = Some(load_by_pk(facade, entity_name, values)?.is_some());
        }
        match (op, exists) {
            (Op::Update, Some(false)) if quiet => return Ok(WriteGate::Missed),
            (Op::Update, Some(false)) => {
                return Err(Error::new(format!(
                    "Could not update {} [{}]: the record was not found in this transaction",
                    prepared.full_name,
                    prepared.pk.replace(PK_SEP, ", ")
                )));
            }
            (Op::Create, Some(true)) if quiet => return Ok(WriteGate::Missed),
            _ => {}
        }
    }
    let service = (crate::hooks::get().artifact_name)().artifact.map(Arc::from);
    // A held row reads back as the database will give it: time stamps at its precision, and a created row with every
    // field, null where the create set none (its insert writes them so too, [`batch::cached_statement`]).
    let plain = as_stored(facade, entity_name, values, op == Op::Create)?;
    tx::hold(
        &tx,
        facade,
        Held {
            entity_name: prepared.full_name,
            operation: op,
            plain,
            sealed: prepared.sealed,
            pk: prepared.pk,
            service,
        },
    )?;
    Ok(WriteGate::Handled)
}

/// A find or count that queried the database, counted for the active frame of `tx` when it has the transaction
/// cache on (plans/TX-Cache.md D1).
pub(crate) fn note_db_read(tx: &TransactionManager) {
    stats::note_read(tx, false);
}

/// Counts writes to the screen configuration entities (`SubscreensItem`, `SubscreensDefault`), here or from a cluster
/// peer, so a screen definition read before the last one is loaded again. Kept with the facade: the rows are its
/// database's, and another facade's writes do not make these screens stale. Reading it takes no lock.
pub fn screen_config_generation(facade: &EntityFacade) -> u64 {
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
pub fn feed_definition_generation(facade: &EntityFacade) -> u64 {
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

/// Counts writes to `LocalizedEntityField` and `LocalizedMessage`, here or from a cluster peer, so a localized field
/// value remembered before the last one is looked up again (`field_rw::localize_field`). Counted again at the end of
/// the write's transaction, as [`feed_definition_generation`] is. Reading it takes no lock.
pub fn localized_generation(facade: &EntityFacade) -> u64 {
    facade.inner.localized_generation.load(AtomicOrdering::Acquire)
}

fn note_localized(facade: &EntityFacade, full_name: &str) {
    if full_name == "moqui.basic.LocalizedEntityField" || full_name == "moqui.basic.LocalizedMessage" {
        facade.inner.localized_generation.fetch_add(1, AtomicOrdering::AcqRel);
    }
}

/// Clear this entity's one-cache, then publish `entity-cache-invalidate` for the primary key.
/// The transaction flush is separate and runs at commit. The local clear stays the whole entity. Inside a
/// transaction the clear runs again at commit or rollback, and the publish waits for commit.
pub fn on_write(facade: &EntityFacade, entity_name: &str, operation: &str, values: &FieldMap) -> Result<(), Error> {
    let result = write_hooks(facade, entity_name, operation, values, || Arc::new(values.clone()));
    note_changed(facade, Some(entity_name));
    result
}

/// [`on_write`] for values the write's other hooks share, so a transaction frame keeps them without a copy. The
/// entity value write that calls it sends the entity-changed notice ([`note_changed`]) itself, when it is over.
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
    (crate::hooks::get().cluster_note_write)(facade, entity_name, &meta.pk_fields, values).map_err(Error::from)
}

/// The entity-changed notice (plans/High-Level-Review.md HL-024): moves every cache kept from entity rows outside the
/// list and count caches, from this one place, for a write here (when it is over), a cluster peer's invalidation
/// ([`clear_one_cache`]) and a whole-cache clear (`None`, every entity: [`clear_all_list_caches`]). The screen
/// configuration, feed definition and localized generations, the definitions built at runtime, and through the
/// `note_write` hook the authorization generation (group membership, tarpit rules and entity filter sets).
pub(crate) fn note_changed(facade: &EntityFacade, entity_name: Option<&str>) {
    let Some(entity_name) = entity_name else {
        for generation in [
            &facade.inner.screen_config_generation,
            &facade.inner.feed_definition_generation,
            &facade.inner.localized_generation,
        ] {
            generation.fetch_add(1, AtomicOrdering::AcqRel);
        }
        super::derived::clear(facade);
        (crate::hooks::get().note_write)(None);
        return;
    };
    // A name the model does not know (a write that failed on it) is passed on as given.
    let meta = read_meta(facade, entity_name).ok();
    let full_name = meta.as_ref().map_or(entity_name, |meta| meta.full_name.as_str());
    note_screen_config(facade, full_name);
    note_feed_definition(facade, full_name);
    note_localized(facade, full_name);
    super::derived::note_write(facade, full_name);
    (crate::hooks::get().note_write)(Some(full_name));
}

/// Whether a cluster peer may hold something a write of `entity_name` changes, so the write is worth an invalidation
/// (plans/High-Level-Review.md HL-017): a cached find of it (any entity not marked `cache="never"`, since a find may ask
/// for the cache), a cached find of a view it is a member of, or a cache the entity-changed notice ([`note_changed`])
/// moves for it. True for a name the model does not know.
pub fn peers_may_cache(facade: &EntityFacade, entity_name: &str) -> bool {
    let Ok(meta) = read_meta(facade, entity_name) else {
        return true;
    };
    if !meta.cache_never {
        return true;
    }
    let name = meta.full_name.as_str();
    let noticed = name.starts_with("moqui.screen.Subscreens")
        || is_feed_definition(name)
        || name == "moqui.basic.LocalizedEntityField"
        || name == "moqui.basic.LocalizedMessage"
        || name.starts_with("moqui.entity.view.DbViewEntity")
        // Kept in memory by the user's authorization (`moqui_core::user::authz`), moved by the entity-changed notice.
        || name == "moqui.security.ArtifactTarpitLock";
    noticed
        || views_by_member(facade).get(name).is_some_and(|views| {
            views
                .iter()
                .any(|view| read_meta(facade, view).map_or(true, |meta| !meta.cache_never))
        })
}

/// For another server's write, and do not publish: with its primary key, the cached finds that held the row or may
/// match the key (as an update), else the entity's caches whole. Remote apply uses this.
pub fn clear_one_cache(facade: &EntityFacade, entity_name: &str, primary_key: Option<&FieldMap>) -> Result<(), Error> {
    let meta = read_meta(facade, entity_name)?;
    let cleared = match primary_key.filter(|keys| meta.pk_fields.iter().all(|name| keys.contains_key(name.as_str()))) {
        Some(keys) => evict(facade, &meta, "update", keys).map(|_| ()),
        None => clear_named(facade, &meta.full_name),
    };
    // After the evict, as a local write's notice comes after its own.
    note_changed(facade, Some(&meta.full_name));
    cleared
}

/// Clear the list and count caches of `full_name`, and those of the views it is a member of.
fn clear_named(facade: &EntityFacade, full_name: &str) -> Result<(), Error> {
    let (lists, counts) = facade.with_slot(OneCaches::default, |caches| {
        // The fence first, as in `evict`.
        if let Some(generation) = caches.generations.get(full_name) {
            generation.fetch_add(1, AtomicOrdering::SeqCst);
        }
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
        caches.epoch.fetch_add(1, AtomicOrdering::SeqCst);
        if let Some(cache) = caches.lists.values().find(|cache| cache.name() == cache_name) {
            return cache.clear().map(|()| true);
        }
        match caches.counts.values().find(|cache| cache.name() == cache_name) {
            Some(cache) => cache.clear().map(|()| true),
            None => Ok(false),
        }
    })
}

/// After the model was swapped (plans/design-dev-aide.md F5): the views of each member built again on next use, and
/// every list and count cache cleared, as their rows were read with the old definitions.
pub(crate) fn forget_model(facade: &EntityFacade) {
    facade.with_slot(OneCaches::default, |caches| caches.views_by_member = None);
    if let Err(err) = clear_all_list_caches(facade) {
        tracing::warn!(error = %err, "Could not clear the entity list caches after the model changed");
    }
}

/// Clear every entity list and count cache, moving every store fence first, then send the entity-changed notice for
/// every entity ([`note_changed`]): a LISTEN reconnect or a busy NOTIFY may have dropped any peer's invalidation.
pub fn clear_all_list_caches(facade: &EntityFacade) -> Result<(), Error> {
    let cleared = facade.with_slot(OneCaches::default, |caches| {
        caches.epoch.fetch_add(1, AtomicOrdering::SeqCst);
        for cache in caches.lists.values() {
            cache.clear()?;
        }
        for cache in caches.counts.values() {
            cache.clear()?;
        }
        Ok(())
    });
    note_changed(facade, None);
    cleared
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
pub(super) fn sim_written_rows(facade: &EntityFacade, entity_name: &str) -> Result<Vec<FieldMap>, Error> {
    let meta = read_meta(facade, entity_name)?;
    let view = sim_view(&meta.full_name);
    if !view.table {
        return Ok(Vec::new());
    }
    let rows = load_plain(facade, &meta, None, &[], None, Some(&view.schema), None)?;
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
            .filter_map(|key| {
                key.split_once(PK_SEP)
                    .map(|(entity, pk)| (entity.to_string(), pk.to_string()))
            })
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
                .map(|row| {
                    row.iter()
                        .map(|(name, value)| (name.to_string(), value.clone()))
                        .collect()
                });
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
pub(super) fn copy_replica(facade: &EntityFacade, entity_name: &str, plain: FieldMap) -> Result<bool, Error> {
    let owner = facade.clone();
    let (full_name, pk, sealed) = facade.with_entity(entity_name, |entity, _, _| {
        let pk = identity(entity, &plain)?;
        let sealed = field_rw::seal_stored(&owner, entity, plain)?;
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
    let held = sim.in_transaction();
    // Held (super::sim_step): a failed read would abort the held transaction and mark it rollback-only, so it runs
    // under a savepoint of its own and its error is returned past `with_connection`.
    sim.with_connection(|conn| {
        if !held {
            return Ok(read_overlay(conn, schema, sql, params));
        }
        conn.savepoint(SIM_STATEMENT_SAVEPOINT)?;
        let result = read_overlay(conn, schema, sql, params);
        end_statement_savepoint(conn, result.is_ok())?;
        Ok(result)
    })?
}

fn read_overlay(conn: &mut dyn DbConnection, schema: &str, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
    query_conn(
        conn,
        "SELECT set_config('search_path', ?, false)",
        &[Value::string(schema)],
    )?;
    query_conn(conn, sql, params)
}

/// The savepoint one overlay read or write runs under while the overlay transaction is held.
const SIM_STATEMENT_SAVEPOINT: &str = "moqui_sim_statement";

/// Release the statement savepoint, or roll back to it after a failure, which leaves the held transaction usable.
fn end_statement_savepoint(conn: &mut dyn DbConnection, ok: bool) -> Result<(), Error> {
    if ok {
        conn.release_savepoint(SIM_STATEMENT_SAVEPOINT)
    } else {
        conn.rollback_to_savepoint(SIM_STATEMENT_SAVEPOINT)?;
        conn.release_savepoint(SIM_STATEMENT_SAVEPOINT)
    }
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
    tx::clear_holds();
    TX_WRITTEN.with(|slot| slot.borrow_mut().clear());
}

pub(super) fn set_sim_flag(value: bool) {
    SIM_FLAG.with(|flag| flag.set(value));
}

pub(super) fn sim_flag() -> bool {
    SIM_FLAG.with(std::cell::Cell::get) || sim_session_open()
}

fn write_sim(
    facade: &EntityFacade,
    entity_name: &str,
    op: Op,
    plain: &FieldMap,
    prepared: &SealedWrite,
    quiet: bool,
) -> Result<bool, Error> {
    let hit = sim_hit(&prepared.full_name, &prepared.pk)?;
    // The overlay dialect rejects insert-or-update. A new sim row is an insert; a written row is an update.
    // A replica row is in the sim table already, as a written one is.
    let present = hit.written || hit.replica;
    // A row the sim has not written or deleted is the database's: the write sees it as the database would.
    let stored = if !present && !hit.tombstone && op != Op::Delete {
        load_by_pk(facade, entity_name, plain)?
    } else {
        None
    };
    let op = match op {
        Op::Upsert if present || stored.is_some() => Op::Update,
        Op::Upsert => Op::Create,
        other => other,
    };
    // As on the database: an update of a row that is not there (or that the sim deleted) and a create of a row that is
    // there are errors, or write nothing when quiet.
    if op == Op::Update && !present && stored.is_none() {
        if quiet {
            return Ok(false);
        }
        return Err(Error::new(facade.missing_update(entity_name, plain)));
    }
    if op == Op::Create && (present || stored.is_some()) {
        if quiet {
            return Ok(false);
        }
        return Err(Error::new(format!(
            "Tried to create a value that already exists [{}: {}]",
            prepared.full_name, prepared.pk
        )));
    }
    if op == Op::Delete && !hit.table {
        note_sim(&prepared.full_name, &prepared.pk, Op::Delete, false);
        return Ok(true);
    }
    let sealed_copy = match stored {
        Some(row) if op == Op::Update => Some(seal(facade, entity_name, "create", row)?),
        _ => None,
    };
    let ddl = if hit.table {
        None
    } else {
        Some(create_sql(facade, entity_name)?)
    };
    let statements = sim_statements(facade, entity_name, op, &prepared.sealed, sealed_copy.as_ref())?;
    let sim = facade
        .sim_transactions()
        .ok_or_else(|| Error::new("sim transactions are not configured"))?;
    run_overlay(&sim, &hit.schema, ddl.as_deref(), &statements)?;
    note_sim(&prepared.full_name, &prepared.pk, op, ddl.is_some());
    Ok(true)
}

fn sim_statements(
    facade: &EntityFacade,
    entity_name: &str,
    op: Op,
    sealed: &FieldMap,
    sealed_copy: Option<&FieldMap>,
) -> Result<Vec<(String, Vec<Value>)>, Error> {
    let mut statements = Vec::new();
    // An update of a database row the sim has not written copies the row into the sim first
    if let Some(copied) = sealed_copy {
        push_statement(&mut statements, dml(facade, entity_name, Op::Create, copied)?);
        push_statement(&mut statements, dml(facade, entity_name, Op::Update, sealed)?);
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
    // Held (super::sim_step): the write is part of the held transaction, under a savepoint of its own so a failed
    // statement leaves the rest; otherwise each write commits on its own.
    let held = sim.in_transaction();
    sim.with_connection(|conn| {
        if held {
            conn.savepoint(SIM_STATEMENT_SAVEPOINT)?;
            let result = apply_overlay(conn, schema, ddl, statements);
            end_statement_savepoint(conn, result.is_ok())?;
            return Ok(result);
        }
        conn.begin()?;
        let result = apply_overlay(conn, schema, ddl, statements);
        Ok(match result {
            Ok(()) => conn.commit(),
            Err(err) => {
                let _ = conn.rollback();
                Err(err)
            }
        })
    })?
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

fn load_plain(
    facade: &EntityFacade,
    meta: &ReadMeta,
    condition: Option<&Condition>,
    order: &[OrderBy],
    limit: Option<i64>,
    overlay_schema: Option<&str>,
    lock: Option<bool>,
) -> Result<Vec<FieldMap>, Error> {
    let plan = plan_select(facade, &meta.full_name, condition, order, limit, lock)?;
    let db_rows = if let Some(schema) = overlay_schema {
        query_sim(facade, schema, &plan.sql, &plan.params)?
    } else {
        query_tx(&plan.tx, &plan.sql, &plan.params)?
    };
    rows_plain(facade, &meta.full_name, &plan.columns, db_rows)
}

/// The stored row of the key in `values`. The statement is rendered once per entity and dialect (`sql_cache`): its
/// parameters are the key's values, in key order.
fn load_by_pk(facade: &EntityFacade, entity_name: &str, values: &FieldMap) -> Result<Option<FieldMap>, Error> {
    let owner = facade.clone();
    let plan = facade.with_group(entity_name, |entity, dialect, _, tx| {
        let shape = super::sql_cache::read_shape(&owner, entity, dialect);
        let mut params = Vec::new();
        for name in entity.pk_field_names() {
            let value = values
                .get(name)
                .filter(|value| !value.is_null())
                .ok_or_else(|| Error::new(format!("primary key {name} is required on {}", entity.full_name)))?;
            // as the condition the statement was rendered from would bind it
            params.push(moqui_db::sql::typed_value(entity.java_type_of(name), value.clone()));
        }
        let key = format!("{}|{}|txc-pk", entity.full_name, dialect.name);
        let sql = super::sql_cache::cached(&owner, &key, || {
            let condition = pk_condition(entity, values)?;
            let options = moqui_db::sql::SelectOptions::default();
            Ok(select_with_head(
                &shape.head,
                entity,
                dialect,
                Some(&condition),
                &[],
                LimitOffset::default(),
                &options,
            )?
            .sql)
        })?;
        Ok(SelectPlan {
            sql: sql.to_string(),
            params,
            columns: Arc::clone(&shape.columns),
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
    columns: Arc<[(Arc<str>, String)]>,
    tx: TransactionManager,
}

fn plan_select(
    facade: &EntityFacade,
    entity_name: &str,
    condition: Option<&Condition>,
    order: &[OrderBy],
    limit: Option<i64>,
    lock: Option<bool>,
) -> Result<SelectPlan, Error> {
    facade.with_group(entity_name, |entity, dialect, _, tx| {
        if entity.is_view {
            return Err(Error::new(format!("cannot overlay view-entity {}", entity.full_name)));
        }
        // Every column, so the head (`SELECT ... FROM`) and the columns are the entity's read shape, rendered once.
        let shape = super::sql_cache::read_shape(facade, entity, dialect);
        let page = LimitOffset { limit, offset: None };
        let options = moqui_db::sql::SelectOptions {
            for_update: lock.is_some(),
            skip_locked: lock.unwrap_or(false),
            ..Default::default()
        };
        // An encrypted field is compared with its stored ciphertext, as a find outside the frame compares it.
        let encrypted = field_rw::encrypt_condition(facade, entity, condition.cloned())?;
        let statement = select_with_head(&shape.head, entity, dialect, encrypted.as_ref(), order, page, &options)?;
        Ok(SelectPlan {
            sql: statement.sql,
            params: statement.params,
            columns: Arc::clone(&shape.columns),
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
    stored: Option<&FieldMap>,
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
        let sealed = field_rw::before_write_known(&owner, entity, seal_operation, values.clone(), stored)?;
        Ok(SealedWrite {
            full_name: entity.full_name.clone(),
            pk,
            sealed,
        })
    })
}

fn seal(facade: &EntityFacade, entity_name: &str, operation: &str, values: FieldMap) -> Result<FieldMap, Error> {
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
pub fn pk_field_names(facade: &EntityFacade, entity_name: &str) -> Result<PkNames, Error> {
    read_meta(facade, entity_name).map(PkNames)
}

/// See [`pk_field_names`].
pub struct PkNames(Arc<ReadMeta>);

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
            one_keys: entity
                .relationships
                .iter()
                .filter(|relationship| {
                    relationship.relationship_type.starts_with("one") && !relationship.key_pairs.is_empty()
                })
                .map(|relationship| (relationship.related.clone(), relationship.key_pairs.clone()))
                .collect(),
        })
    })?);
    facade.with_slot(ReadMetas::default, |metas| {
        metas.insert(entity_name, generation, Arc::clone(&meta))
    });
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
    let mut key = String::new();
    write_pk(fields, pk_fields, &mut key).then_some(key)
}

/// Write the primary key text of `fields` into `out`, as [`pk_of`] gives it; false (with `out` partly written) when a
/// key field is missing or null, or the entity has no primary key.
fn write_pk(fields: &FieldMap, pk_fields: &[String], out: &mut String) -> bool {
    if pk_fields.is_empty() {
        return false;
    }
    for (index, name) in pk_fields.iter().enumerate() {
        let Some(value) = fields.get(name.as_str()).filter(|value| !value.is_null()) else {
            return false;
        };
        if index > 0 {
            out.push(PK_SEP);
        }
        out.push_str(name);
        out.push('=');
        match value {
            Value::String(text) => out.push_str(text),
            other => out.push_str(&to_plain_string(other)),
        }
    }
    true
}

fn row_key(entity: &str, pk: &str) -> String {
    format!("{entity}{PK_SEP}{pk}")
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
                let value = row
                    .fields
                    .get(entry.field.name.as_str())
                    .cloned()
                    .unwrap_or(Value::Null);
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

/// `condition` with each text value converted to its field's date, time or number type, as a condition binds it in
/// SQL (`moqui_db::sql::typed_value`): a script passes a time as epoch millis text, and a held row compared with it in
/// memory has the time. `None` when the condition needs no change.
pub(crate) fn typed_condition(
    facade: &EntityFacade,
    entity_name: &str,
    condition: Option<&Condition>,
) -> Option<Condition> {
    let condition = condition?;
    facade
        .with_entity(entity_name, |entity, _, _| Ok(retype(entity, condition)))
        .ok()
        .flatten()
}

/// [`typed_condition`] on one condition; `None` when nothing in it changes.
fn retype(entity: &moqui_db::model::EntityDefinition, condition: &Condition) -> Option<Condition> {
    let typed = |field: &FieldRef, value: &Value| -> Option<Value> {
        if field.entity_alias.is_some() || !matches!(value, Value::String(_)) {
            return None;
        }
        let converted = moqui_db::sql::typed_value(entity.java_type_of(&field.name), value.clone());
        (converted != *value).then_some(converted)
    };
    let one = |field: &FieldRef, value: &Value, make: fn(FieldRef, Value) -> Condition| {
        typed(field, value).map(|value| make(field.clone(), value))
    };
    let list = |values: &[Value], field: &FieldRef| -> Option<Vec<Value>> {
        let converted: Vec<Option<Value>> = values.iter().map(|value| typed(field, value)).collect();
        converted.iter().any(Option::is_some).then(|| {
            converted
                .into_iter()
                .zip(values)
                .map(|(converted, value)| converted.unwrap_or_else(|| value.clone()))
                .collect()
        })
    };
    let all = |parts: &[Condition]| -> Option<Vec<Condition>> {
        let converted: Vec<Option<Condition>> = parts.iter().map(|part| retype(entity, part)).collect();
        converted.iter().any(Option::is_some).then(|| {
            converted
                .into_iter()
                .zip(parts)
                .map(|(converted, part)| converted.unwrap_or_else(|| part.clone()))
                .collect()
        })
    };
    match condition {
        Condition::Equals(field, value) => one(field, value, Condition::Equals),
        Condition::NotEquals(field, value) => one(field, value, Condition::NotEquals),
        Condition::Less(field, value) => one(field, value, Condition::Less),
        Condition::Greater(field, value) => one(field, value, Condition::Greater),
        Condition::LessEqual(field, value) => one(field, value, Condition::LessEqual),
        Condition::GreaterEqual(field, value) => one(field, value, Condition::GreaterEqual),
        Condition::In(field, values) => list(values, field).map(|values| Condition::In(field.clone(), values)),
        Condition::NotIn(field, values) => list(values, field).map(|values| Condition::NotIn(field.clone(), values)),
        Condition::Between(field, low, high) | Condition::NotBetween(field, low, high) => {
            let (new_low, new_high) = (typed(field, low), typed(field, high));
            if new_low.is_none() && new_high.is_none() {
                return None;
            }
            let (low, high) = (
                new_low.unwrap_or_else(|| low.clone()),
                new_high.unwrap_or_else(|| high.clone()),
            );
            Some(if matches!(condition, Condition::Between(..)) {
                Condition::Between(field.clone(), low, high)
            } else {
                Condition::NotBetween(field.clone(), low, high)
            })
        }
        Condition::And(parts) => all(parts).map(Condition::And),
        Condition::Or(parts) => all(parts).map(Condition::Or),
        Condition::Not(inner) => retype(entity, inner).map(|inner| Condition::Not(Box::new(inner))),
        _ => None,
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
        Condition::Equals(field, expected) => same_value(&field_value(fields, &field.name), expected),
        Condition::NotEquals(field, expected) => {
            let got = field_value(fields, &field.name);
            !got.is_null() && !same_value(&got, expected)
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
            values.iter().any(|value| same_value(&got, value))
        }
        Condition::NotIn(field, values) => {
            if values.is_empty() {
                return true;
            }
            let got = field_value(fields, &field.name);
            !got.is_null() && !values.iter().any(|value| same_value(&got, value))
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

/// Whether a row's value equals a condition's as the database would compare them: the same value, or of other types
/// (a number given as text or as a float, plans/TX-Cache.md) the same by ordering or as text.
fn same_value(got: &Value, expected: &Value) -> bool {
    if got == expected {
        return true;
    }
    if got.is_null() || expected.is_null() {
        return false;
    }
    cmp_present(got, expected) == Some(Ordering::Equal) || same_key(got, expected)
}

/// Two values ordered as the database orders them: numbers of different kinds (an integer against a decimal) by their
/// amounts, as SQL compares a number column with a number of any type.
fn cmp_present(left: &Value, right: &Value) -> Option<Ordering> {
    if left.is_null() || right.is_null() {
        return None;
    }
    if let Ok(order) = compare_obj(left, right) {
        return Some(order);
    }
    let number = |value: &Value| -> Option<Value> {
        match value {
            Value::I32(_) | Value::I64(_) | Value::Decimal(_) => {
                Some(moqui_lib::object::basic_convert(value, "BigDecimal").value)
                    .filter(|v| matches!(v, Value::Decimal(_)))
            }
            _ => None,
        }
    };
    match (left, right) {
        (Value::F64(a), other) | (other, Value::F64(a)) => {
            let b = match other {
                Value::I32(b) => f64::from(*b),
                #[allow(clippy::cast_precision_loss)]
                Value::I64(b) => *b as f64,
                Value::Decimal(_) => moqui_lib::object::to_plain_string(other).parse::<f64>().ok()?,
                _ => return None,
            };
            let order = a.partial_cmp(&b)?;
            Some(if matches!(left, Value::F64(_)) {
                order
            } else {
                order.reverse()
            })
        }
        _ => compare_obj(&number(left)?, &number(right)?).ok(),
    }
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
#[path = "cache_tests.rs"]
mod tests;
