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

//! The commit batch of a transaction-cache frame (plans/archive/TX-Cache.md D6): the held writes of each row merged into one,
//! sent without waiting (creates and upserts as rows the driver may join into multi-row statements), and one wait for
//! them all. A driver that cannot pipeline (MySQL, SQLite) runs each statement as it is sent, as before.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use moqui_lib::error::Error;

use super::hold::Held;
use std::sync::Arc;

use moqui_db::dialect::Dialect;
use moqui_db::model::EntityDefinition;
use moqui_db::sql::Lock;
use moqui_lib::object::Value;

use super::super::sql_cache;
use super::{bind_columns, pk_condition, present_columns, EntityFacade, FieldMap, Op};
use crate::driver::DbConnection;

/// The statements and rows one flush wrote: `statements` merged writes sent, `rows` held writes they stand for.
pub(super) struct Flushed {
    pub(super) statements: u64,
    pub(super) rows: u64,
}

/// One row's held writes as one ([`merge`]): a create and its later updates are one insert of the final values, and
/// updates are one update of the union of their columns. ECA rules and audit rows ran for each write when it was held.
struct Merged {
    /// The first write of the row, for its entity and key and what an error says.
    first: Rc<Held>,
    operation: Op,
    sealed: FieldMap,
    writes: u64,
    /// The first write is a quiet create and nothing merged into it ([`Held::if_missing`]).
    if_missing: bool,
}

/// Merge each row's writes into one where moving a write cannot change what the database checks, keeping the held
/// order. A write joins its row's previous merged write when the two are adjacent, and an update joins the row's earlier
/// create, upsert or update across other writes when it changes no field of a relationship key (so no foreign key it
/// sets or clears moves past a row that depends on it). A delete is never merged: the update before it may clear a
/// reference that another row's delete after it needs gone (a thread's replies before their parent).
fn merge(facade: &EntityFacade, writes: impl Iterator<Item = Rc<Held>>) -> Vec<Merged> {
    let mut out: Vec<Merged> = Vec::new();
    // The open merged write of each row, by entity and key; a delete closes it.
    let mut open: HashMap<(String, String), usize> = HashMap::new();
    // The relationship key fields of each entity met.
    let mut key_fields: HashMap<String, HashSet<String>> = HashMap::new();
    for held in writes {
        let key = (held.entity_name.clone(), held.pk.clone());
        let target = open.get(&key).copied().filter(|&at| {
            let merged = &out[at];
            let joins = matches!(
                (merged.operation, held.operation),
                (Op::Create | Op::Upsert | Op::Update, Op::Update) | (Op::Create | Op::Upsert | Op::Update, Op::Upsert)
            );
            if !joins {
                return false;
            }
            if at + 1 == out.len() {
                return true;
            }
            held.operation == Op::Update && !sets_key_field(facade, &mut key_fields, &held)
        });
        match target {
            Some(at) => {
                let merged = &mut out[at];
                merged
                    .sealed
                    .extend(held.sealed.iter().map(|(name, value)| (name.clone(), value.clone())));
                merged.writes += 1;
                // a write merged into a quiet create changes it: it goes out as a create
                merged.if_missing = false;
                if merged.operation == Op::Update && held.operation == Op::Upsert {
                    merged.operation = Op::Upsert;
                }
            }
            None => {
                if held.operation == Op::Delete {
                    open.remove(&key);
                } else {
                    open.insert(key, out.len());
                }
                out.push(Merged {
                    operation: held.operation,
                    sealed: held.sealed.clone(),
                    if_missing: held.if_missing,
                    first: held,
                    writes: 1,
                });
            }
        }
    }
    out
}

/// Whether an update sets a field of one of its entity's type-one relationship keys (a foreign key).
fn sets_key_field(facade: &EntityFacade, key_fields: &mut HashMap<String, HashSet<String>>, held: &Held) -> bool {
    let fields = key_fields.entry(held.entity_name.clone()).or_insert_with(|| {
        super::read_meta(facade, &held.entity_name)
            .map(|meta| {
                meta.one_keys
                    .iter()
                    .flat_map(|(_, pairs)| pairs.iter().map(|(field, _)| field.clone()))
                    .filter(|field| !meta.pk_fields.contains(field))
                    .collect()
            })
            .unwrap_or_default()
    });
    held.sealed.keys().any(|name| fields.contains(&**name))
}

/// Send `writes` as the commit batch on `conn` and wait for them once (without `wait`, the next statement that waits
/// does: the commit). A failure found when sending names its write;
/// one found by the wait names the batch, and the database's message names the row. An update that changed no row
/// fails the batch, as a direct update does (plans/archive/TX-Cache.md G4), except on MySQL, which counts an update that leaves
/// the row as it was as none, and except for a row in `locked`, which a locking find of the frame read.
pub(super) fn flush_writes(
    facade: &EntityFacade,
    writes: impl Iterator<Item = Rc<Held>>,
    locked: &HashMap<(String, String), Lock>,
    conn: &mut dyn DbConnection,
    wait: bool,
) -> Result<Flushed, Error> {
    let merged = merge(facade, writes);
    let mut flushed = Flushed { statements: 0, rows: 0 };
    for write in &merged {
        let held = &write.first;
        let named = |err: Error| Error::new(format!("Could not write the {}", held.describe())).with_source(err);
        let statement = facade
            .with_group(&held.entity_name, |entity, dialect, _, _| {
                let mysql = moqui_db::sql::is_mysql(dialect);
                if write.if_missing && moqui_db::sql::supports_insert_if_missing(dialect) {
                    return Ok(Some((if_missing_statement(facade, entity, dialect, &write.sealed)?, mysql, true)));
                }
                Ok(
                    cached_statement(facade, entity, dialect, write.operation, &write.sealed)?
                        .map(|statement| (statement, mysql, false)),
                )
            })
            .map_err(named)?;
        let Some(((sql, params), mysql, if_missing)) = statement else {
            continue;
        };
        match write.operation {
            // one statement of its own, which writes nothing when the row is there by now
            Op::Create if if_missing => conn.execute_deferred(&sql, params, None),
            Op::Create | Op::Upsert => conn.execute_deferred_row(&sql, params, row_key(held)),
            // A row the frame locked is there until the commit and needs no check. Another must change a row: the
            // driver has the server check that where it can, so the COMMIT need not wait for it.
            Op::Update if !mysql && !locked.contains_key(&(held.entity_name.clone(), held.pk.clone())) => {
                let missing = format!("Could not write the {}: the record was not found", held.describe());
                conn.execute_deferred_update(&sql, params, missing)
            }
            Op::Update => conn.execute_deferred(&sql, params, None),
            Op::Delete => conn.execute_deferred(&sql, params, None),
        }
        .map_err(named)?;
        flushed.statements += 1;
        flushed.rows += write.writes;
    }
    if !wait {
        return Ok(flushed);
    }
    conn.finish_deferred().map_err(|err| {
        Error::new(format!(
            "Could not write the transaction cache's {} held writes",
            flushed.rows
        ))
        .with_source(err)
    })?;
    Ok(flushed)
}

/// A statement's SQL text, shared with `sql_cache`, and its parameters.
type Statement = (Arc<str>, Vec<Value>);

/// The statement of one merged write, its SQL rendered once per entity, operation and columns (`sql_cache`). A create
/// writes every column, null where unset, so the creates of an entity share one statement and the driver can join
/// them (no column has a database default); an update writes the columns it holds; `None` is an update of none.
/// A held quiet create's statement: every field, as a held create writes them, in an insert that writes nothing when
/// the key is there ([`moqui_db::sql::insert_if_missing`]).
fn if_missing_statement(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    dialect: &Dialect,
    values: &FieldMap,
) -> Result<Statement, Error> {
    let columns: Vec<&str> = entity.fields.iter().map(|field| field.name.as_str()).collect();
    let key = format!("{}|{}|txc-insert-if-missing", entity.full_name, dialect.name);
    let sql = sql_cache::cached(facade, &key, || moqui_db::sql::insert_if_missing(entity, dialect, &columns))?;
    Ok((sql, bind_columns(&columns, values)))
}

fn cached_statement(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    dialect: &Dialect,
    operation: Op,
    values: &FieldMap,
) -> Result<Option<Statement>, Error> {
    let pk_values = || -> Result<Vec<Value>, Error> {
        entity
            .pk_field_names()
            .into_iter()
            .map(|name| {
                values
                    .get(name)
                    .filter(|value| !value.is_null())
                    .map(|value| moqui_db::sql::typed_value(entity.java_type_of(name), value.clone()))
                    .ok_or_else(|| Error::new(format!("primary key {name} is required on {}", entity.full_name)))
            })
            .collect()
    };
    let mut key = format!("{}|{}|txc-", entity.full_name, dialect.name);
    match operation {
        Op::Create => {
            let columns: Vec<&str> = entity.fields.iter().map(|field| field.name.as_str()).collect();
            key.push_str("insert");
            let sql = sql_cache::cached(facade, &key, || moqui_db::sql::insert(entity, dialect, &columns))?;
            Ok(Some((sql, bind_columns(&columns, values))))
        }
        Op::Delete => {
            let params = pk_values()?;
            key.push_str("delete");
            let sql = sql_cache::cached(facade, &key, || {
                Ok(moqui_db::sql::delete(entity, dialect, Some(&pk_condition(entity, values)?))?.sql)
            })?;
            Ok(Some((sql, params)))
        }
        Op::Update | Op::Upsert => {
            let update = operation == Op::Update;
            let columns = present_columns(entity, values, update, update);
            if columns.is_empty() {
                if update {
                    return Ok(None);
                }
                return Err(Error::new(format!("upsert has no columns for {}", entity.full_name)));
            }
            key.push_str(if update { "update|" } else { "upsert|" });
            for column in &columns {
                key.push_str(column);
                key.push(',');
            }
            let mut params = bind_columns(&columns, values);
            let sql = if update {
                params.extend(pk_values()?);
                sql_cache::cached(facade, &key, || {
                    let condition = pk_condition(entity, values)?;
                    Ok(moqui_db::sql::update(entity, dialect, &columns, Some(&condition))?.sql)
                })?
            } else {
                sql_cache::cached(facade, &key, || {
                    moqui_db::sql::insert_or_update(entity, dialect, &columns)
                })?
            };
            Ok(Some((sql, params)))
        }
    }
}

/// The key of a created row for [`DbConnection::execute_deferred_row`]: a row whose key is among those the driver holds
/// is sent after them.
fn row_key(held: &Held) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    held.entity_name.hash(&mut hasher);
    held.pk.hash(&mut hasher);
    hasher.finish()
}
