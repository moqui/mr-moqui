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

//! The commit batch of a transaction-cache frame (plans/TX-Cache.md D6): the held writes of each row merged into one,
//! sent without waiting (creates and upserts as rows the driver may join into multi-row statements), and one wait for
//! them all. A driver that cannot pipeline (MySQL, SQLite) runs each statement as it is sent, as before.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use moqui_lib::error::Error;

use super::hold::Held;
use super::{bind_columns, statement_for, EntityFacade, FieldMap, Op};
use crate::driver::DbConnection;

/// The statements and rows one flush wrote: `statements` merged writes sent, `rows` held writes they stand for.
pub(super) struct Flushed {
    pub(super) statements: u64,
    pub(super) rows: u64,
}

/// One row's held writes as one: a create and its later updates are one insert of the final values, updates are one
/// update of the union of their columns, and updates followed by a delete are the delete. ECA rules and audit rows ran
/// for each write when it was held.
struct Merged {
    /// The first write of the row, for its entity and key and what an error says.
    first: Rc<Held>,
    operation: Op,
    sealed: FieldMap,
    writes: u64,
}

/// Merge each row's writes, keeping the held order: a create (or upsert) stays where the row's first write was, so a
/// child row created after it follows it, and an update or delete goes where the row's last write was, so it follows
/// the writes it may depend on (a row it now refers to, a child deleted before it).
fn merge(writes: impl Iterator<Item = Rc<Held>>) -> Vec<Merged> {
    let mut out: Vec<Option<Merged>> = Vec::new();
    // The open merged write of each row, by entity and key; a delete closes it.
    let mut open: HashMap<(String, String), usize> = HashMap::new();
    for held in writes {
        let key = (held.entity_name.clone(), held.pk.clone());
        let Some(at) = open.get(&key).copied() else {
            let at = out.len();
            if held.operation != Op::Delete {
                open.insert(key, at);
            }
            out.push(Some(Merged {
                operation: held.operation,
                sealed: held.sealed.clone(),
                first: held,
                writes: 1,
            }));
            continue;
        };
        let Some(mut merged) = out[at].take() else {
            continue;
        };
        merged.writes += 1;
        let moves = match (merged.operation, held.operation) {
            (Op::Create | Op::Upsert, Op::Update | Op::Upsert) => {
                merged
                    .sealed
                    .extend(held.sealed.iter().map(|(name, value)| (name.clone(), value.clone())));
                false
            }
            (Op::Update, Op::Update) => {
                merged
                    .sealed
                    .extend(held.sealed.iter().map(|(name, value)| (name.clone(), value.clone())));
                true
            }
            (Op::Update, Op::Upsert) => {
                merged
                    .sealed
                    .extend(held.sealed.iter().map(|(name, value)| (name.clone(), value.clone())));
                merged.operation = Op::Upsert;
                true
            }
            (_, Op::Delete) => {
                merged.operation = Op::Delete;
                merged.sealed = held.sealed.clone();
                open.remove(&key);
                true
            }
            // A create after a write of the same row other than a delete is refused when it is held, and a delete
            // closes its row: keep both as they are.
            _ => {
                out[at] = Some(merged);
                let at = out.len();
                open.insert(key, at);
                out.push(Some(Merged {
                    operation: held.operation,
                    sealed: held.sealed.clone(),
                    first: held,
                    writes: 1,
                }));
                continue;
            }
        };
        if moves {
            if merged.operation != Op::Delete {
                open.insert(key, out.len());
            }
            out.push(Some(merged));
        } else {
            out[at] = Some(merged);
        }
    }
    out.into_iter().flatten().collect()
}

/// Send `writes` as the commit batch on `conn` and wait for them once. A failure found when sending names its write;
/// one found by the wait names the batch, and the database's message names the row. An update that changed no row
/// fails the batch, as a direct update does (plans/TX-Cache.md G4), except on MySQL, which counts an update that leaves
/// the row as it was as none.
pub(super) fn flush_writes(
    facade: &EntityFacade,
    writes: impl Iterator<Item = Rc<Held>>,
    conn: &mut dyn DbConnection,
) -> Result<Flushed, Error> {
    let merged = merge(writes);
    let mut flushed = Flushed { statements: 0, rows: 0 };
    for write in &merged {
        let held = &write.first;
        let named = |err: Error| Error::new(format!("Could not write the {}", held.describe())).with_source(err);
        let statement = facade
            .with_group(&held.entity_name, |entity, dialect, _, _| {
                let mysql = moqui_db::sql::is_mysql(dialect);
                if write.operation == Op::Create {
                    // Every column, null where unset, so the creates of an entity share one statement and the driver
                    // can join them. No column has a database default.
                    let columns: Vec<&str> = entity.fields.iter().map(|field| field.name.as_str()).collect();
                    let sql = moqui_db::sql::insert(entity, dialect, &columns)?;
                    return Ok(Some(((sql, bind_columns(&columns, &write.sealed)), mysql)));
                }
                Ok(statement_for(entity, dialect, write.operation, &write.sealed)?.map(|statement| (statement, mysql)))
            })
            .map_err(named)?;
        let Some(((sql, params), mysql)) = statement else {
            continue;
        };
        match write.operation {
            Op::Create | Op::Upsert => conn.execute_deferred_row(&sql, params, row_key(held)),
            Op::Update => {
                let zero_rows =
                    (!mysql).then(|| format!("Could not write the {}: the record was not found", held.describe()));
                conn.execute_deferred(&sql, params, zero_rows)
            }
            Op::Delete => conn.execute_deferred(&sql, params, None),
        }
        .map_err(named)?;
        flushed.statements += 1;
        flushed.rows += write.writes;
    }
    conn.finish_deferred().map_err(|err| {
        Error::new(format!(
            "Could not write the transaction cache's {} held writes: {err}",
            flushed.rows
        ))
        .with_source(err)
    })?;
    Ok(flushed)
}

/// The key of a created row for [`DbConnection::execute_deferred_row`]: a row whose key is among those the driver holds
/// is sent after them.
fn row_key(held: &Held) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    held.entity_name.hash(&mut hasher);
    held.pk.hash(&mut hasher);
    hasher.finish()
}
