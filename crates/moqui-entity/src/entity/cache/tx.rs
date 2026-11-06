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

//! The transaction cache (plans/TX-Cache.md): a `cache` or `force-cache` frame holds its writes in memory
//! ([`super::hold`]) and writes them at its commit; while it holds them, its finds see them through the overlay.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use moqui_db::sql::{select, Condition, FieldRef, LimitOffset, OrderBy};
use moqui_lib::error::Error;

use super::batch::{flush_writes, Flushed};
use super::hold::{FrameHold, Held, Overlay};
use super::stats::{self, FlushCause};
use super::{
    entity_columns, load_plain, may_match, pk_condition, pk_of, query_tx, row_key, row_matches, rows_plain,
    sim_session_open, sim_view, sort_rows, EntityFacade, EntityValue, FieldMap, MemRow, Op, ReadMeta, SelectPlan,
    SimView,
};
use crate::driver::DbConnection;
use crate::transaction::{CacheMode, TransactionManager};

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
        let flushed = flush_held(&facade, token, conn);
        match &flushed {
            Ok(batch) => stats::note_batch(
                token,
                batch.statements,
                batch.rows,
                crate::driver::thread_round_trips() - round_trips,
            ),
            Err(err) => stats::note_flush_failed(&owner, err),
        }
        stats::end_frame(token);
        flushed.map(|_| ())
    })?;
    tx.on_rollback(move || {
        TX_HOLD.with(|slot| slot.borrow_mut().remove(&token));
        stats::end_frame(token);
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
    tx.with_connection(|conn| flush_held(facade, token, conn)).map(|_| ())
}

pub(super) fn flush_held(facade: &EntityFacade, token: u64, conn: &mut dyn DbConnection) -> Result<Flushed, Error> {
    let held_rows = TX_HOLD
        .with(|slot| slot.borrow_mut().remove(&token))
        .unwrap_or_default();
    flush_writes(facade, held_rows.into_order(), conn)
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
/// early flush of a selective find (plans/TX-Cache.md D4).
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
    tx.with_connection(|conn| flush_writes(facade, prefix.into_iter(), conn))
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
    // (plans/TX-Cache.md D6).
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
        _ => Ok(()),
    }
}

/// The rows of a find of `meta` with this frame's held writes and the sim overlay on them; `None` when the database
/// answers alone. With a page (`limit`) and no sim, the database reads only as far as the page can reach: each held
/// write moves at most one row into or out of the rows before it, and a held update of a row past that point starts
/// from its stored row. The caller takes the page.
pub(super) fn overlay_rows(
    facade: &EntityFacade,
    meta: &ReadMeta,
    condition: Option<&Condition>,
    order: &[OrderBy],
    page: LimitOffset,
    lock: Option<bool>,
) -> Result<Option<Vec<EntityValue>>, Error> {
    let sim = if sim_session_open() {
        sim_view(&meta.full_name)
    } else {
        SimView::empty()
    };
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
        .map(|fields| MemRow { fields, trusted: true })
        .collect();
    let overlay = Overlay::new(rows, &holds, &meta.pk_fields);
    // An update of a row the condition did not load starts from the whole stored row, not the fields it set.
    let stored = stored_rows(facade, meta, token, &overlay)?;
    let mut rows = overlay.apply(&stored)?;
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
) -> Result<HashMap<&'h str, Option<Rc<FieldMap>>>, Error> {
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
                frame.keep_stored(&meta.full_name, pk, Some(Rc::new(fields.clone())));
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
    for batch in missing.chunks(STORED_BATCH) {
        let keys: Vec<&FieldMap> = batch.iter().map(|held| &held.plain).collect();
        let mut found: HashMap<String, Rc<FieldMap>> = HashMap::with_capacity(batch.len());
        for row in load_by_pks(facade, &meta.full_name, &keys)? {
            if let Some(pk) = pk_of(&row, &meta.pk_fields) {
                found.insert(pk, Rc::new(row));
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
    Ok(stored)
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

/// Before a view read: when the frame holds rows of one of the view's members, write every held row now (in order,
/// so a child row follows its parent), as Java's bypass of the transaction cache for a view.
pub(super) fn flush_for_view(facade: &EntityFacade, view: &str) -> Result<(), Error> {
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
    let held = TX_HOLD.with(|slot| slot.borrow().get(&token).is_some_and(|frame| frame.holds_any(&members)));
    if held {
        flush_early(facade, &tx, token, FlushCause::View, view)?;
    }
    Ok(())
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
            columns: entity_columns(entity, model),
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
