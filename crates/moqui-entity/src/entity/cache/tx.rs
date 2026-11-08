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

//! The transaction cache (plans/archive/TX-Cache.md): a `cache` or `force-cache` frame holds its writes in memory
//! ([`super::hold`]) and writes them at its commit; while it holds them, its finds see them through the overlay.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use crate::entity::EntityList;
use std::sync::Arc;

use moqui_db::sql::{select, Condition, FieldRef, LimitOffset, OrderBy, TableShadow};
use moqui_lib::error::Error;

use super::batch::{flush_writes, Flushed};
use super::hold::{FrameHold, Held, KeptList, Overlay};
use super::stats::{self, FlushCause};
use super::{
    entity_columns, load_by_pk, load_plain, may_match, owned_lookup_key, pk_condition, pk_of, query_tx, row_key,
    row_matches, rows_plain, sim_session_open, sim_view, sort_rows, with_lookup_key, write_pk, EntityFacade,
    EntityValue, FieldMap, FindLookup, MemRow, Op, ReadMeta, SelectPlan, SimView,
};
use crate::driver::DbConnection;
use crate::transaction::{CacheMode, TransactionManager};
use moqui_lib::object::Value;

thread_local! {
    /// Held rows by transaction frame token.
    static TX_HOLD: RefCell<HashMap<u64, FrameHold>> = RefCell::new(HashMap::new());
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
    let owner = tx.clone();
    tx.on_cache_flush(move |conn| {
        let round_trips = crate::driver::thread_round_trips();
        // The commit's own wait reports the batch (the transaction manager waits first when a hook comes between).
        let flushed = flush_held(&facade, token, conn, false);
        match &flushed {
            Ok(batch) => stats::note_batch(
                token,
                batch.statements,
                batch.rows,
                crate::driver::thread_round_trips() - round_trips,
            ),
            Err(err) => stats::note_flush_failed(&owner, err),
        }
        flushed.map(|_| ())
    })?;
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
    flush_early(facade, tx, token, FlushCause::NoTxCache, "")?;
    tx.set_cache_mode(CacheMode::ReadThrough)
}

/// Write the held rows of the frame `token` of `tx` before its commit, and go on as the frame was. Counted and
/// logged as an early flush of `cause`; nothing when the frame holds no rows.
pub(super) fn flush_early(
    facade: &EntityFacade,
    tx: &TransactionManager,
    token: u64,
    cause: FlushCause,
    entity: &str,
) -> Result<(), Error> {
    let holds = TX_HOLD.with(|slot| slot.borrow().get(&token).is_some_and(|frame| !frame.is_empty()));
    if !holds {
        return Ok(());
    }
    stats::note_early_flush(tx, token, cause, entity);
    tx.with_connection(|conn| flush_held(facade, token, conn, true))
        .map(|_| ())
}

/// Write the frame's held rows; with `wait`, wait for them, else leave them for the next statement that waits.
pub(super) fn flush_held(
    facade: &EntityFacade,
    token: u64,
    conn: &mut dyn DbConnection,
    wait: bool,
) -> Result<Flushed, Error> {
    let held_rows = TX_HOLD
        .with(|slot| slot.borrow_mut().remove(&token))
        .unwrap_or_default();
    let (writes, locked) = held_rows.into_parts();
    flush_writes(facade, writes, &locked, conn, wait)
}

/// What the frame of `tx` knows of one row: `Some(true)` when its last held write leaves the row there, `Some(false)` when
/// it deleted it or read it as missing, `None` when it knows nothing.
pub(super) fn row_known(tx: &TransactionManager, entity: &str, pk: &str) -> Option<bool> {
    let token = tx.frame_token()?;
    TX_HOLD.with(|slot| {
        let map = slot.borrow();
        let frame = map.get(&token)?;
        match frame.last_op(entity, pk) {
            Some(Op::Delete) => Some(false),
            Some(_) => Some(true),
            None => frame.stored(entity, pk).map(|row| row.is_some()),
        }
    })
}

/// Whether the active frame of `entity` holds a write of it.
pub(super) fn holds_entity(facade: &EntityFacade, entity: &str) -> bool {
    let Some(token) = hold_token(facade, entity) else {
        return false;
    };
    TX_HOLD.with(|slot| {
        slot.borrow()
            .get(&token)
            .is_some_and(|frame| frame.holds_entity(entity))
    })
}

/// Write the active frame's held writes up to and including its last of `entity`, and go on holding the rest: the
/// early flush of a selective find (plans/archive/TX-Cache.md D4).
pub(super) fn flush_through(facade: &EntityFacade, entity: &str) -> Result<(), Error> {
    let tx = facade.transactions_for(entity)?;
    let Some(token) = tx.frame_token() else {
        return Ok(());
    };
    let Some(prefix) = TX_HOLD.with(|slot| {
        slot.borrow_mut()
            .get_mut(&token)
            .and_then(|frame| frame.split_through(entity))
    }) else {
        return Ok(());
    };
    stats::note_early_flush(&tx, token, FlushCause::SelectiveFind, entity);
    tx.with_connection(|conn| flush_writes(facade, prefix.into_iter(), &HashSet::new(), conn, true))
        .map(|_| ())
}

pub(super) fn push_hold(token: u64, held: Held) -> Result<(), Error> {
    TX_HOLD.with(|slot| slot.borrow_mut().entry(token).or_default().push(held))
}

/// Hold one prepared write on the active frame of `tx`, registering the frame's commit flush before its first held
/// row.
pub(super) fn hold(tx: &TransactionManager, facade: &EntityFacade, held: Held) -> Result<(), Error> {
    tx.with_connection(|_| Ok(()))?;
    let token = tx.frame_token().ok_or_else(|| Error::new("no transaction in place"))?;
    // The flush goes on the frame before the first held row, so no held row is left without one.
    if !tx.has_cache_flush() {
        register_cache_flush(facade, tx)?;
    }
    stats::note_held(token, held.operation);
    push_hold(token, held)?;
    // Past the cap the hold is written and the frame goes on holding, so a long batch job does not grow without bound
    // (plans/archive/TX-Cache.md D6).
    let held = TX_HOLD.with(|slot| slot.borrow().get(&token).map_or(0, FrameHold::len));
    if held >= HOLD_CAP {
        flush_early(facade, tx, token, FlushCause::Size, "")?;
    }
    Ok(())
}

/// Held writes a frame keeps before it writes them early ([`FlushCause::Size`]).
const HOLD_CAP: usize = 10_000;

/// Forget the held rows and counts of every frame on this thread.
pub(super) fn clear_holds() {
    TX_HOLD.with(|slot| slot.borrow_mut().clear());
    stats::clear_frames();
}

/// Before raw SQL in the active frame of `tx`: its held rows go to the database first (an early flush), since the
/// statement reads or writes the tables without them.
pub fn flush_before_sql(facade: &EntityFacade, tx: &TransactionManager) -> Result<(), Error> {
    match tx.cache_frame() {
        Some((token, CacheMode::Hold)) => flush_early(facade, tx, token, FlushCause::RawSql, ""),
        // The statement may change any row the frame read.
        Some((token, CacheMode::ReadThrough)) => {
            forget_reads(token, None);
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The rows of a select-field find the cache frame of `meta`'s entity kept ([`super::after_selective_find`]), when it
/// holds no write of the entity; `None` for the database to answer.
pub(super) fn projected_rows(
    facade: &EntityFacade,
    meta: &ReadMeta,
    lookup: &FindLookup,
    selective: &super::Selective,
) -> Option<EntityList> {
    let (token, _) = facade.transactions_for(&meta.full_name).ok()?.cache_frame()?;
    let key = super::projected_key(lookup, selective)?;
    let rows = TX_HOLD.with(|slot| slot.borrow().get(&token)?.projected(&meta.full_name, &key))?;
    stats::note_read_of(token, true);
    let name: Arc<str> = Arc::from(meta.full_name.as_str());
    Some(EntityList::new(
        rows.iter()
            .map(|row| EntityValue::from_shared(Arc::clone(&name), Arc::clone(row)))
            .collect(),
    ))
}

/// Keep the rows of a select-field find in the cache frame of `meta`'s entity ([`super::after_selective_find`]).
pub(super) fn keep_projected(facade: &EntityFacade, meta: &ReadMeta, key: String, list: &EntityList) {
    let Some((token, _)) = facade
        .transactions_for(&meta.full_name)
        .ok()
        .and_then(|tx| tx.cache_frame())
    else {
        return;
    };
    let rows: KeptList = Rc::new(
        list.values()
            .iter()
            .map(|value| Arc::new(value.fields().clone()))
            .collect(),
    );
    TX_HOLD.with(|slot| {
        slot.borrow_mut()
            .entry(token)
            .or_default()
            .keep_projected(&meta.full_name, key, rows)
    });
}

/// Drop what every frame on this thread but `token` read of `entity`: `token` committed writes of it, and the others
/// (suspended under it, or beside it) read the entity as it was before.
pub(super) fn forget_reads_of_others(token: u64, entity: &str) {
    TX_HOLD.with(|slot| {
        for (other, frame) in slot.borrow_mut().iter_mut() {
            if *other != token {
                frame.forget_reads(Some(entity));
            }
        }
    });
}

/// Drop what the frame `token` read of `entity`, or of every entity (plans/archive/TX-Cache.md D2).
pub(super) fn forget_reads(token: u64, entity: Option<&str>) {
    TX_HOLD.with(|slot| {
        if let Some(frame) = slot.borrow_mut().get_mut(&token) {
            frame.forget_reads(entity);
        }
    });
}

/// The rows of a find of `meta` in a frame with the transaction cache on, with the frame's held writes on them (and the
/// sim overlay, in a sim); `None` when the database answers alone, as it does with no such frame.
///
/// In a cache frame (hold or read-through, plans/archive/TX-Cache.md D2, D3) the rows come from what the frame read before when
/// they can: a find by the whole key from the row the frame keeps (or, for a row it created, its held writes alone); a
/// list find with no page from the list it kept under the key of `list`; a list find whose condition pins a type-one
/// relationship to a row the frame created from its held creates alone. Otherwise the database answers and the frame
/// keeps the rows. With a page (`limit`), the database reads only as far as the page can reach: each held write moves
/// at most one row into or out of the rows before it, and a held update of a row past that point starts from its
/// stored row. The caller takes the page. A `lock` (for update) always reads the database.
pub(super) fn overlay_rows(
    facade: &EntityFacade,
    meta: &ReadMeta,
    condition: Option<&Condition>,
    order: &[OrderBy],
    page: LimitOffset,
    lock: Option<bool>,
    list: Option<&FindLookup>,
) -> Result<Option<Vec<EntityValue>>, Error> {
    if sim_session_open() {
        let sim = sim_view(&meta.full_name);
        if sim.affects() {
            return sim_overlay_rows(facade, meta, condition, order, page, lock, sim);
        }
    }
    let Some((token, _)) = facade
        .transactions_for(&meta.full_name)
        .ok()
        .and_then(|tx| tx.cache_frame())
    else {
        return Ok(None);
    };
    let entity = meta.full_name.as_str();
    // A find the shared entity cache answers (an entity marked `cache`, the frame not having written it) is left to it:
    // it is memory too, and shared.
    if let Some(lookup) = list {
        if lock.is_none() && meta.caches(lookup) && !super::frame_wrote(facade, entity, meta) {
            return Ok(None);
        }
    }
    let pinned = if lock.is_none() {
        pinned_keys(condition, &meta.pk_fields)
    } else {
        None
    };
    if let Some((pk, keys)) = pinned {
        return pinned_row(facade, meta, token, &pk, &keys).map(Some);
    }
    if lock.is_none() && page.limit.is_none() && page.offset.is_none() {
        if let Some(rows) = known_in_rows(meta, token, condition, order)? {
            return Ok(Some(rows));
        }
    }
    let holds = TX_HOLD.with(|slot| {
        slot.borrow()
            .get(&token)
            .map(|frame| frame.holds(entity))
            .unwrap_or_default()
    });
    let cacheable = lock.is_none() && page.limit.is_none() && page.offset.is_none();
    let relevant = holds
        .iter()
        .any(|held| held.operation != Op::Create || may_match(condition, &held.plain));
    let list = list.filter(|_| cacheable);
    let new_rows_only = lock.is_none() && pins_created_parent(facade, meta, token, condition);
    if !relevant && list.is_none() && !new_rows_only && lock.is_none() {
        // Nothing the frame holds or kept can change the answer: the database gives it, with its limit and offset. A
        // locking find is read here all the same, so the frame keeps the rows it locked and knows them locked.
        return Ok(None);
    }
    // The kept list is looked up by the key written into a thread buffer; an owned key is made only to keep one.
    let kept = list.and_then(|lookup| {
        with_lookup_key(lookup, |key| {
            TX_HOLD.with(|slot| slot.borrow().get(&token).and_then(|frame| frame.kept_list(entity, key)))
        })
    });
    let mut memory = true;
    let loaded: Vec<Arc<FieldMap>> = if new_rows_only {
        Vec::new()
    } else if let Some(kept) = kept {
        kept.iter().map(Arc::clone).collect()
    } else {
        memory = false;
        let window = page.limit.map(|limit| {
            let held = i64::try_from(holds.len()).unwrap_or(i64::MAX);
            page.offset
                .unwrap_or(0)
                .max(0)
                .saturating_add(limit.max(0))
                .saturating_add(held)
        });
        let loaded: Vec<Arc<FieldMap>> = load_plain(facade, meta, condition, order, window, None, lock)?
            .into_iter()
            .map(Arc::new)
            .collect();
        TX_HOLD.with(|slot| {
            let mut map = slot.borrow_mut();
            let frame = map.entry(token).or_default();
            match list {
                Some(lookup) => {
                    let rows: KeptList = Rc::new(loaded.clone());
                    frame.keep_list(entity, &owned_lookup_key(lookup), rows, &meta.pk_fields);
                }
                None => {
                    let mut buffer = String::new();
                    for row in &loaded {
                        buffer.clear();
                        if write_pk(row, &meta.pk_fields, &mut buffer) {
                            frame.keep_stored(entity, &buffer, Some(Arc::clone(row)));
                            if lock.is_some() {
                                frame.note_locked(entity, &buffer);
                            }
                        }
                    }
                }
            }
        });
        loaded
    };
    let rows: Vec<MemRow> = loaded
        .into_iter()
        .map(|fields| MemRow { fields, trusted: true })
        .collect();
    let overlay = Overlay::new(rows, &holds, &meta.pk_fields);
    // An update of a row the condition did not load starts from the whole stored row, not the fields it set.
    let (stored, read) = stored_rows(facade, meta, Some(token), &overlay)?;
    stats::note_read_of(token, memory && !read);
    let mut rows = overlay.apply(&stored)?;
    // matched as the database would match them: text values of the condition at their fields' types
    let typed = super::typed_condition(facade, &meta.full_name, condition);
    let condition = typed.as_ref().or(condition);
    rows.retain(|row| row.trusted || row_matches(condition, &row.fields));
    if !order.is_empty() {
        sort_rows(&mut rows, order);
    }
    Ok(Some(values_of(meta, rows)))
}

fn values_of(meta: &ReadMeta, rows: Vec<MemRow>) -> Vec<EntityValue> {
    let name: Arc<str> = Arc::from(meta.full_name.as_str());
    rows.into_iter()
        .map(|row| EntityValue::from_shared(Arc::clone(&name), row.fields))
        .collect()
}

/// A find by an `in` of keys of a one-field primary key (and nothing else), with no page and no order of more than one
/// row, when the frame knows every one of the rows (it read it, or created it): the rows from memory, in the order of
/// the keys, with the frame's held writes on them. `None` when one is not known, for the database to answer.
fn known_in_rows(
    meta: &ReadMeta,
    token: u64,
    condition: Option<&Condition>,
    order: &[OrderBy],
) -> Result<Option<Vec<EntityValue>>, Error> {
    let [pk_field] = meta.pk_fields.as_slice() else {
        return Ok(None);
    };
    let Some(Condition::In(field, values)) = condition else {
        return Ok(None);
    };
    if field.entity_alias.is_some() || field.expr.is_some() || &field.name != pk_field || values.is_empty() {
        return Ok(None);
    }
    if !order.is_empty() && values.len() > 1 {
        return Ok(None);
    }
    let entity = meta.full_name.as_str();
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::with_capacity(values.len());
    let known = TX_HOLD.with(|slot| {
        let map = slot.borrow();
        let Some(frame) = map.get(&token) else {
            return false;
        };
        for value in values {
            let mut keys = FieldMap::new();
            keys.insert(Arc::from(pk_field.as_str()), value.clone());
            let Some(pk) = pk_of(&keys, &meta.pk_fields) else {
                return false;
            };
            if !seen.insert(pk.clone()) {
                continue;
            }
            let base = match (frame.stored(entity, &pk), frame.first_op(entity, &pk)) {
                (Some(row), _) => row,
                (None, Some(Op::Create)) => None,
                (None, _) => return false,
            };
            found.push((base, frame.holds_of(entity, &pk)));
        }
        true
    });
    if !known {
        return Ok(None);
    }
    stats::note_read_of(token, true);
    let mut rows = Vec::with_capacity(found.len());
    for (base, holds) in found {
        let stored: Vec<MemRow> = base
            .map(|row| MemRow {
                fields: row,
                trusted: true,
            })
            .into_iter()
            .collect();
        rows.extend(Overlay::new(stored, &holds, &meta.pk_fields).apply(&HashMap::new())?);
    }
    Ok(Some(values_of(meta, rows)))
}

/// A find by the whole key in a cache frame: the row the frame keeps, or for a row it created its held writes alone,
/// or else the row read by its key and kept (plans/archive/TX-Cache.md D2, G7). The rest of the condition is checked in
/// memory.
fn pinned_row(
    facade: &EntityFacade,
    meta: &ReadMeta,
    token: u64,
    pk: &str,
    keys: &FieldMap,
) -> Result<Vec<EntityValue>, Error> {
    let entity = meta.full_name.as_str();
    let (holds, known, first) = TX_HOLD.with(|slot| {
        let map = slot.borrow();
        match map.get(&token) {
            Some(frame) => (
                frame.holds_of(entity, pk),
                frame.stored(entity, pk),
                frame.first_op(entity, pk),
            ),
            None => (Vec::new(), None, None),
        }
    });
    let (base, memory) = match (known, first) {
        (Some(row), _) => (row, true),
        // A row the frame created is not on the database, unless the frame read it there (then the overlay fails it).
        (None, Some(Op::Create)) => (None, true),
        (None, _) => {
            let row = load_by_pk(facade, entity, keys)?.map(Arc::new);
            TX_HOLD.with(|slot| {
                slot.borrow_mut()
                    .entry(token)
                    .or_default()
                    .keep_stored(entity, pk, row.clone())
            });
            (row, false)
        }
    };
    stats::note_read_of(token, memory);
    // The condition is the key: every row the overlay gives, of that key alone, matches it.
    let rows: Vec<MemRow> = base
        .map(|row| MemRow {
            fields: row,
            trusted: true,
        })
        .into_iter()
        .collect();
    let overlay = Overlay::new(rows, &holds, &meta.pk_fields);
    let rows = overlay.apply(&HashMap::new())?;
    Ok(values_of(meta, rows))
}

/// Whether `condition` pins every key field of a type-one relationship of `meta` to a row the frame created, so no row
/// on the database refers to it and the frame's held creates are all the rows there are (Java's `listGet` shortcut,
/// plans/archive/TX-Cache.md D3).
fn pins_created_parent(facade: &EntityFacade, meta: &ReadMeta, token: u64, condition: Option<&Condition>) -> bool {
    if meta.one_keys.is_empty() {
        return false;
    }
    let Some(condition) = condition else {
        return false;
    };
    let pinned = equalities(condition);
    if pinned.is_empty() {
        return false;
    }
    TX_HOLD.with(|slot| {
        let map = slot.borrow();
        let Some(frame) = map.get(&token).filter(|frame| !frame.is_empty()) else {
            return false;
        };
        meta.one_keys.iter().any(|(related, pairs)| {
            let mut keys = FieldMap::new();
            for (field, related_field) in pairs {
                match pinned.get(field.as_str()) {
                    Some(value) => {
                        keys.insert(Arc::from(related_field.as_str()), value.clone());
                    }
                    None => return false,
                }
            }
            let Ok(related) = super::read_meta(facade, related) else {
                return false;
            };
            let Some(pk) = pk_of(&keys, &related.pk_fields) else {
                return false;
            };
            frame.first_op(&related.full_name, &pk) == Some(Op::Create)
                && frame.stored(&related.full_name, &pk).is_none()
        })
    })
}

/// The fields a condition compares by equality at the top level, with their values; few, so a vector.
fn equalities(condition: &Condition) -> Equalities<'_> {
    let parts: &[Condition] = match condition {
        Condition::And(list) => list,
        other => std::slice::from_ref(other),
    };
    Equalities(
        parts
            .iter()
            .filter_map(|part| match part {
                Condition::Equals(field, value) if field.entity_alias.is_none() && !value.is_null() => {
                    Some((field.name.as_str(), value))
                }
                _ => None,
            })
            .collect(),
    )
}

struct Equalities<'c>(Vec<(&'c str, &'c Value)>);

impl<'c> Equalities<'c> {
    fn get(&self, name: &str) -> Option<&'c Value> {
        self.0.iter().find(|(field, _)| *field == name).map(|(_, value)| *value)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The distinct fields compared.
    fn len(&self) -> usize {
        self.0
            .iter()
            .enumerate()
            .filter(|(at, (field, _))| !self.0[..*at].iter().any(|(earlier, _)| earlier == field))
            .count()
    }
}

/// The key text and key fields of a condition that is the whole key and nothing else: an equality of each key field at
/// the top level. A condition with more is not answered by key, since what more it asks is judged by the database (a
/// value of another type compares there as it would not here).
fn pinned_keys(condition: Option<&Condition>, pk_fields: &[String]) -> Option<(String, FieldMap)> {
    let condition = condition?;
    let parts = match condition {
        Condition::And(list) => list.len(),
        _ => 1,
    };
    let pinned = equalities(condition);
    if parts != pk_fields.len() || pinned.len() != pk_fields.len() {
        return None;
    }
    let mut keys = FieldMap::new();
    for name in pk_fields {
        keys.insert(Arc::from(name.as_str()), pinned.get(name.as_str())?.clone());
    }
    let pk = pk_of(&keys, pk_fields)?;
    Some((pk, keys))
}

/// [`overlay_rows`] with a sim open that changed the entity: the frame's held writes and the sim overlay on the rows of
/// the database; `None` when the database answers alone. With a page (`limit`) and no sim, the database reads only as far as the page can reach: each held
/// write moves at most one row into or out of the rows before it, and a held update of a row past that point starts
/// from its stored row. The caller takes the page.
fn sim_overlay_rows(
    facade: &EntityFacade,
    meta: &ReadMeta,
    condition: Option<&Condition>,
    order: &[OrderBy],
    page: LimitOffset,
    lock: Option<bool>,
    sim: SimView,
) -> Result<Option<Vec<EntityValue>>, Error> {
    let token = hold_token(facade, &meta.full_name);
    // A find by the whole primary key sees only the held rows of that key.
    let pinned = (!sim.affects())
        .then(|| pinned_pk(condition, &meta.pk_fields))
        .flatten();
    let holds = match token {
        Some(token) => TX_HOLD.with(|slot| {
            slot.borrow()
                .get(&token)
                .map(|frame| match &pinned {
                    Some(pk) => frame.holds_of(&meta.full_name, pk),
                    None => frame.holds(&meta.full_name),
                })
                .unwrap_or_default()
        }),
        None => Vec::new(),
    };
    if !sim.affects() {
        // With no held row that can be in the result, the database answers alone, with its limit and offset.
        if holds
            .iter()
            .all(|held| held.operation == Op::Create && !may_match(condition, &held.plain))
        {
            return Ok(None);
        }
    }
    let window = match page.limit {
        Some(limit) if !sim.affects() => {
            let held = i64::try_from(holds.len()).unwrap_or(i64::MAX);
            Some(
                page.offset
                    .unwrap_or(0)
                    .max(0)
                    .saturating_add(limit.max(0))
                    .saturating_add(held),
            )
        }
        _ => None,
    };
    let loaded = load_plain(facade, meta, condition, order, window, None, lock)?;
    if let Some(token) = token {
        stats::note_read_of(token, false);
    }
    let rows: Vec<MemRow> = loaded
        .into_iter()
        .map(|fields| MemRow {
            fields: Arc::new(fields),
            trusted: true,
        })
        .collect();
    let overlay = Overlay::new(rows, &holds, &meta.pk_fields);
    // An update of a row the condition did not load starts from the whole stored row, not the fields it set.
    let (stored, _) = stored_rows(facade, meta, token, &overlay)?;
    let mut rows = overlay.apply(&stored)?;
    // matched as the database would match them: text values of the condition at their fields' types
    let typed = super::typed_condition(facade, &meta.full_name, condition);
    let condition = typed.as_ref().or(condition);
    rows.retain(|row| row.trusted || row_matches(condition, &row.fields));
    if sim.affects() {
        rows.retain(|row| match pk_of(&row.fields, &meta.pk_fields) {
            Some(pk) => !sim.hide.contains(&row_key(&meta.full_name, &pk)),
            None => true,
        });
        if sim.table {
            let extra = load_plain(facade, meta, condition, order, None, Some(&sim.schema), None)?;
            for fields in extra {
                if let Some(pk) = pk_of(&fields, &meta.pk_fields) {
                    rows.retain(|row| pk_of(&row.fields, &meta.pk_fields).as_deref() != Some(pk.as_str()));
                }
                rows.push(MemRow {
                    fields: Arc::new(fields),
                    trusted: true,
                });
            }
        }
    }
    if !order.is_empty() {
        sort_rows(&mut rows, order);
    }
    let name: Arc<str> = Arc::from(meta.full_name.as_str());
    let values = rows
        .into_iter()
        .map(|row| EntityValue::from_shared(Arc::clone(&name), row.fields))
        .collect();
    Ok(Some(values))
}

/// The stored row of each held key an overlay needs; `None` where the database has none.
type StoredRows<'h> = HashMap<&'h str, Option<Arc<FieldMap>>>;

/// Held keys read in one query of [`stored_rows`].
const STORED_BATCH: usize = 100;

/// The stored row of each of the overlay's unloaded held updates. The frame keeps each stored row it reads, and the
/// loaded rows under its held updates, so a later find reads none of them again; the rest are read in one query per
/// [`STORED_BATCH`] keys.
pub(super) fn stored_rows<'h>(
    facade: &EntityFacade,
    meta: &ReadMeta,
    token: Option<u64>,
    overlay: &Overlay<'h>,
) -> Result<(StoredRows<'h>, bool), Error> {
    let needed = overlay.unloaded_updates();
    let mut stored = HashMap::with_capacity(needed.len());
    let mut missing = Vec::new();
    TX_HOLD.with(|slot| {
        let mut map = slot.borrow_mut();
        let Some(frame) = token.and_then(|token| map.get_mut(&token)) else {
            missing.extend(needed.iter().copied());
            return;
        };
        for (pk, fields) in overlay.loaded_updates() {
            if frame.stored(&meta.full_name, pk).is_none() {
                frame.keep_stored(&meta.full_name, pk, Some(Arc::clone(fields)));
            }
        }
        for held in &needed {
            match frame.stored(&meta.full_name, &held.pk) {
                Some(row) => {
                    stored.insert(held.pk.as_str(), row);
                }
                None => missing.push(*held),
            }
        }
    });
    let read = !missing.is_empty();
    for batch in missing.chunks(STORED_BATCH) {
        let keys: Vec<&FieldMap> = batch.iter().map(|held| &held.plain).collect();
        let mut found: HashMap<String, Arc<FieldMap>> = HashMap::with_capacity(batch.len());
        for row in load_by_pks(facade, &meta.full_name, &keys)? {
            if let Some(pk) = pk_of(&row, &meta.pk_fields) {
                found.insert(pk, Arc::new(row));
            }
        }
        TX_HOLD.with(|slot| {
            let mut map = slot.borrow_mut();
            let mut frame = token.and_then(|token| map.get_mut(&token));
            for held in batch {
                let row = found.get(&held.pk).cloned();
                if let Some(frame) = frame.as_deref_mut() {
                    frame.keep_stored(&meta.full_name, &held.pk, row.clone());
                }
                stored.insert(held.pk.as_str(), row);
            }
        });
    }
    Ok((stored, read))
}

/// The primary key text a condition pins: every key field compared by equality at the top level.
pub(super) fn pinned_pk(condition: Option<&Condition>, pk_fields: &[String]) -> Option<String> {
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

/// The members of `view` the frame of `token` holds rows of that may show in a read with `condition`: each base
/// entity's full name, once.
fn showing_members(
    facade: &EntityFacade,
    view: &str,
    token: u64,
    condition: Option<&Condition>,
) -> Result<Vec<String>, Error> {
    // Each member: its full name, and the view's aliases of its fields (alias, field); a member that is a view itself
    // stands for the entities under it, whose rows are judged with no condition (`None`): any held row of them shows.
    let members: Vec<ViewMember> = facade.with_entity(view, |entity, _, model| {
        Ok(entity
            .member_entities
            .iter()
            .flat_map(|member| {
                let nested = model.entity(&member.entity_name).ok().filter(|member| member.is_view);
                if let Some(nested) = nested {
                    return base_entities(model, nested, 0)
                        .into_iter()
                        .map(|name| (name, None))
                        .collect::<Vec<_>>();
                }
                let full_name = model
                    .entity(&member.entity_name)
                    .map(|entity| entity.full_name.clone())
                    .unwrap_or_else(|_| member.entity_name.clone());
                let aliases = entity
                    .aliases
                    .iter()
                    .filter(|alias| alias.entity_alias.as_deref() == Some(member.entity_alias.as_str()))
                    .filter(|alias| alias.function.is_none())
                    .map(|alias| {
                        (
                            alias.name.clone(),
                            alias.field.clone().unwrap_or_else(|| alias.name.clone()),
                        )
                    })
                    .collect();
                vec![(full_name, Some(aliases))]
            })
            .collect())
    })?;
    Ok(TX_HOLD.with(|slot| {
        let map = slot.borrow();
        let Some(frame) = map.get(&token) else {
            return Vec::new();
        };
        let mut showing: Vec<String> = Vec::new();
        for (entity, aliases) in &members {
            if showing.contains(entity) {
                continue;
            }
            let shows = frame.holds(entity).iter().any(|held| match aliases {
                Some(aliases) => held_may_show(frame, held, aliases, condition),
                None => true,
            });
            if shows {
                showing.push(entity.clone());
            }
        }
        showing
    }))
}

/// Held rows of a view's members past which a view read writes them first instead of shadowing them.
const SHADOW_ROWS_MAX: usize = 10_000;

/// Before a find or count of a view entity in a frame that holds rows of a member that may be in its result: the
/// shadows of those members' tables (`TableShadow`), so the read sees the held rows with none of them written. Where
/// a shadow cannot be made (a database other than Postgres, a member with a full-text field or an encrypted key, more
/// than [`SHADOW_ROWS_MAX`] held rows) every held row is written now instead (an early flush, in order, so a child row
/// follows its parent), as Java's bypass of the transaction cache for a view. A held row may be in the result unless
/// the view's `condition`, mapped to the member's fields through the view's aliases, surely does not match it, before
/// or after the write. Empty when nothing held may show.
fn view_shadows(facade: &EntityFacade, view: &str, condition: Option<&Condition>) -> Result<Vec<TableShadow>, Error> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return Ok(Vec::new());
    }
    let tx = facade.transactions_for(view)?;
    let Some(token) = tx.frame_token() else {
        return Ok(Vec::new());
    };
    let showing = showing_members(facade, view, token, condition)?;
    if showing.is_empty() {
        return Ok(Vec::new());
    }
    match shadows_of(facade, view, token, &showing)? {
        Some(shadows) => {
            stats::note_shadowed(token);
            Ok(shadows)
        }
        None => {
            flush_early(facade, &tx, token, FlushCause::View, view)?;
            Ok(Vec::new())
        }
    }
}

/// The shadows a find or count of `entity_name` reads through: a view's ([`view_shadows`]), or with `aggregate` (a
/// `distinct` or `having` read) a plain entity's ([`entity_shadows`]). Empty, with no lookup, when this thread holds
/// no row.
pub(crate) fn read_shadows(
    facade: &EntityFacade,
    entity_name: &str,
    condition: Option<&Condition>,
    aggregate: bool,
) -> Result<Vec<TableShadow>, Error> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return Ok(Vec::new());
    }
    if super::read_meta(facade, entity_name)?.is_view {
        view_shadows(facade, entity_name, condition)
    } else if aggregate {
        entity_shadows(facade, entity_name)
    } else {
        Ok(Vec::new())
    }
}

/// Before a `distinct` or `having` find or count of `entity`, which the frame cannot answer from its rows: when the
/// frame holds rows of the entity, the shadow of its table, so the read sees them with none written. Where a shadow
/// cannot be made (as for [`view_shadows`]) the held writes up to the entity's last are written first (an early flush,
/// cause "selective find"). Empty when the frame holds no row of it.
fn entity_shadows(facade: &EntityFacade, entity: &str) -> Result<Vec<TableShadow>, Error> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return Ok(Vec::new());
    }
    let tx = facade.transactions_for(entity)?;
    let Some(token) = tx.frame_token() else {
        return Ok(Vec::new());
    };
    let full_name = super::read_meta(facade, entity)?.full_name.clone();
    let holds = TX_HOLD.with(|slot| {
        slot.borrow()
            .get(&token)
            .is_some_and(|frame| frame.holds_entity(&full_name))
    });
    if !holds {
        return Ok(Vec::new());
    }
    match shadows_of(facade, entity, token, std::slice::from_ref(&full_name))? {
        Some(shadows) => {
            stats::note_shadowed(token);
            Ok(shadows)
        }
        None => {
            flush_through(facade, &full_name)?;
            Ok(Vec::new())
        }
    }
}

/// The shadow of each of `entities` from the frame's held rows, or `None` where one cannot be made. `view` is the
/// entity read, whose database decides.
fn shadows_of(
    facade: &EntityFacade,
    view: &str,
    token: u64,
    entities: &[String],
) -> Result<Option<Vec<TableShadow>>, Error> {
    let shadowable = facade.with_entity(view, |_, dialect, model| {
        if !dialect.name.starts_with("postgres") {
            return Ok(false);
        }
        Ok(entities.iter().all(|name| {
            model.entity(name).is_ok_and(|entity| {
                !entity.is_view
                    && entity
                        .fields
                        .iter()
                        .all(|field| !(field.encrypt && field.is_pk) && field.field_type != "text-fts")
            })
        }))
    })?;
    let held_count: usize = TX_HOLD.with(|slot| {
        slot.borrow()
            .get(&token)
            .map_or(0, |frame| entities.iter().map(|name| frame.holds(name).len()).sum())
    });
    if !shadowable || held_count > SHADOW_ROWS_MAX {
        return Ok(None);
    }
    let mut shadows = Vec::with_capacity(entities.len());
    for name in entities {
        let meta = super::read_meta(facade, name)?;
        let holds: Vec<Rc<Held>> = TX_HOLD.with(|slot| {
            slot.borrow()
                .get(&token)
                .map(|frame| frame.holds(name))
                .unwrap_or_default()
        });
        let overlay = Overlay::new(Vec::new(), &holds, &meta.pk_fields);
        let (stored, _) = stored_rows(facade, &meta, Some(token), &overlay)?;
        let rows = overlay.apply(&stored)?;
        let (fields, types) = facade.with_entity(name, |entity, dialect, model| {
            let mut fields = Vec::with_capacity(entity.fields.len());
            let mut types = Vec::with_capacity(entity.fields.len());
            for field in &entity.fields {
                fields.push(field.name.clone());
                types.push(moqui_db::sql::field_sql_type(model, dialect, field)?);
            }
            Ok((fields, types))
        })?;
        let mut seen = HashSet::new();
        let hidden: Vec<Vec<Value>> = holds
            .iter()
            .filter(|held| seen.insert(held.pk.as_str()))
            .map(|held| {
                meta.pk_fields
                    .iter()
                    .map(|field| held.plain.get(field.as_str()).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
        // The rows as stored: an encrypted field's value encrypted, as the table has it.
        let encrypted = facade.with_entity(name, |entity, _, _| Ok(entity.fields.iter().any(|field| field.encrypt)))?;
        let mut stored_rows = Vec::with_capacity(rows.len());
        for row in &rows {
            let sealed = if encrypted {
                let plain = FieldMap::clone(&row.fields);
                std::borrow::Cow::Owned(facade.with_entity(name, |entity, _, _| {
                    super::super::field_rw::seal_stored(facade, entity, plain)
                })?)
            } else {
                std::borrow::Cow::Borrowed(row.fields.as_ref())
            };
            stored_rows.push(
                fields
                    .iter()
                    .map(|field| sealed.get(field.as_str()).cloned().unwrap_or(Value::Null))
                    .collect(),
            );
        }
        let rows = stored_rows;
        shadows.push(TableShadow {
            entity_name: meta.full_name.clone(),
            hidden,
            rows,
            types,
        });
    }
    Ok(Some(shadows))
}

/// Before a read of a view built in code (a data document, a REST query): the shadows of its `members` the frame holds
/// rows of, as [`view_shadows`] gives a view entity's, with no condition to judge by (every member with a held row is
/// shadowed). Where a shadow cannot be made every held row is written now instead. Empty when none is held.
pub(crate) fn member_shadows(facade: &EntityFacade, members: &[String], view: &str) -> Result<Vec<TableShadow>, Error> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return Ok(Vec::new());
    }
    let Some(first) = members.first() else {
        return Ok(Vec::new());
    };
    let tx = facade.transactions_for(first)?;
    let Some(token) = tx.frame_token() else {
        return Ok(Vec::new());
    };
    // a member that is a view stands for the entities under it
    let full_names: Vec<String> = facade
        .with_entity(first, |_, _, model| {
            Ok(members
                .iter()
                .flat_map(|name| match model.entity(name) {
                    Ok(entity) if entity.is_view => base_entities(model, entity, 0),
                    Ok(entity) => vec![entity.full_name.clone()],
                    Err(_) => vec![name.clone()],
                })
                .collect())
        })
        .unwrap_or_else(|_| members.to_vec());
    let mut held: Vec<String> = Vec::new();
    TX_HOLD.with(|slot| {
        if let Some(frame) = slot.borrow().get(&token) {
            for name in &full_names {
                if frame.holds_entity(name) && !held.contains(name) {
                    held.push(name.clone());
                }
            }
        }
    });
    if held.is_empty() {
        return Ok(Vec::new());
    }
    match shadows_of(facade, first, token, &held)? {
        Some(shadows) => {
            stats::note_shadowed(token);
            Ok(shadows)
        }
        None => {
            flush_early(facade, &tx, token, FlushCause::View, view)?;
            Ok(Vec::new())
        }
    }
}

/// A member entity of a view read, by full name, with the view's aliases of its fields (alias, field); `None` for an
/// entity under a member view, whose held rows are judged with no condition.
type ViewMember = (String, Option<Vec<(String, String)>>);

/// The entities under a view, through the views among its members, by full name; past a few levels (a view of
/// views of views) what is left is not looked into.
fn base_entities(
    model: &moqui_db::model::EntityModel,
    view: &moqui_db::model::EntityDefinition,
    depth: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    for member in &view.member_entities {
        match model.entity(&member.entity_name) {
            Ok(entity) if entity.is_view && depth < 8 => out.extend(base_entities(model, entity, depth + 1)),
            Ok(entity) if !entity.is_view => out.push(entity.full_name.clone()),
            Ok(_) => {}
            Err(_) => out.push(member.entity_name.clone()),
        }
    }
    out
}

/// Whether a held write of a view member may change a view read with `condition`: the row before or after it may
/// match, judged on the view's aliases of the member's fields.
fn held_may_show(frame: &FrameHold, held: &Held, aliases: &[(String, String)], condition: Option<&Condition>) -> bool {
    let Some(condition) = condition else {
        return true;
    };
    let stored = frame.stored(&held.entity_name, &held.pk);
    let states: Vec<FieldMap> = match (held.operation, stored) {
        (Op::Create, _) => vec![held.plain.clone()],
        (Op::Update | Op::Upsert, Some(Some(row))) => {
            let mut after = FieldMap::clone(&row);
            after.extend(held.plain.iter().map(|(name, value)| (name.clone(), value.clone())));
            vec![FieldMap::clone(&row), after]
        }
        (Op::Update | Op::Upsert, Some(None)) => vec![held.plain.clone()],
        (Op::Delete, Some(Some(row))) => vec![FieldMap::clone(&row)],
        (Op::Delete, Some(None)) => return false,
        (_, None) => return true,
    };
    states.iter().any(|fields| {
        let mapped: FieldMap = aliases
            .iter()
            .filter_map(|(alias, field)| Some((Arc::from(alias.as_str()), fields.get(field.as_str())?.clone())))
            .collect();
        may_match(Some(condition), &mapped)
    })
}

/// The stored rows of `keys` (each a held write's values, which hold the whole primary key), in one query.
pub(super) fn load_by_pks(
    facade: &EntityFacade,
    entity_name: &str,
    keys: &[&FieldMap],
) -> Result<Vec<FieldMap>, Error> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let owner = facade.clone();
    let plan = facade.with_group(entity_name, |entity, dialect, model, tx| {
        let mut any = Vec::with_capacity(keys.len());
        for values in keys {
            any.push(pk_condition(entity, values)?);
        }
        let condition = if any.len() == 1 {
            any.remove(0)
        } else {
            Condition::Or(any)
        };
        let fields: Vec<FieldRef> = entity.fields.iter().map(|field| FieldRef::new(&field.name)).collect();
        let statement = select(entity, dialect, &fields, Some(&condition), &[], LimitOffset::default())?;
        Ok(SelectPlan {
            sql: statement.sql,
            params: statement.params,
            columns: entity_columns(entity, model).into(),
            tx: tx.clone(),
        })
    })?;
    let db_rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
    rows_plain(&owner, entity_name, &plan.columns, db_rows)
}

/// The token of the active frame of `entity` when this thread holds rows of any frame. A suspended frame's rows are
/// not visible.
pub(super) fn hold_token(facade: &EntityFacade, entity: &str) -> Option<u64> {
    if TX_HOLD.with(|slot| slot.borrow().is_empty()) {
        return None;
    }
    facade.transactions_for(entity).ok().and_then(|tx| tx.frame_token())
}

#[cfg(test)]
#[path = "tx_tests.rs"]
mod tests;
