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

//! [`EntityFacade::create_values`]: the values of one entity inserted several rows to a statement, each prepared as
//! a create prepares it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use moqui_db::model::EntityDefinition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::super::exec::{after_statement, execute_deferred_tx, execute_tx, pipelining};
use super::super::facade::EntityFacade;
use super::super::{cache, field_rw, fk, load_feed};
use super::{bind_columns, has_field_defaults, pk_values, EntityArtifact, EntityValue, FieldMap, WriteNote, STAMP};

/// Rows one statement of [`EntityFacade::create_values`] inserts at most.
pub const CREATE_ROWS: usize = 100;
/// Values one such statement binds at most, under SQL Server's 2100 parameters.
pub const CREATE_BINDS: usize = 2000;
/// The rows to a statement [`EntityFacade::create_values`] sends, largest first: a few statement texts an entity.
const CREATE_SIZES: [usize; 5] = [CREATE_ROWS, 30, 10, 3, 1];

impl EntityFacade {
    /// Insert `rows`, each a value of `entity_name`, as [`Self::create_value`] inserts each, in as few statements as
    /// the bind limits allow: `INSERT ... VALUES (...), (...)` of up to [`CREATE_ROWS`] rows and [`CREATE_BINDS`]
    /// values, one row to a statement where the database takes no more ([`moqui_db::sql::supports_insert_rows`]). The
    /// entity is named by its full name from the start, as a create names it, for its ECA rules, its authorization,
    /// the data feed and the caches.
    ///
    /// The rows are taken from `rows` a statement's worth at a time. Each is checked first, as a create checks its row
    /// before its write: its field defaults filled, its fields converted to their types, its primary key required and
    /// its foreign keys checked. Then, in the entity's `create` frame, which authorizes the write and counts it once a
    /// statement, each is sealed (its encrypted fields) and audited, the statement sent, and the rows noted for the
    /// data feed and cleared from the caches, one cluster invalidation carrying their keys. So no audit row is written
    /// for a row of a statement that fails its checks. A statement binds every column a row of it or of a statement
    /// before it sets, null where a row sets none, as the transaction cache's batch writes a created row. In a
    /// pipelined transaction ([`super::super::exec::PipelineWrites`]), where a statement goes out without waiting for its
    /// result, the database inserts one statement's rows while the next are made and checked.
    ///
    /// Where a row needs a write of its own, each row is created by [`Self::create_value`]: an entity ECA rule on
    /// create runs, dummy foreign keys are on (a row's missing related row, its own parent among them, is made first),
    /// or a sim or a transaction cache would hold the writes.
    ///
    /// # Errors
    ///
    /// When a row is of another entity, or fails as its create would; rows written before it stay in the transaction.
    pub fn create_values(&self, entity_name: &str, rows: impl IntoIterator<Item = EntityValue>) -> Result<(), Error> {
        let mut rows = rows.into_iter().peekable();
        if rows.peek().is_none() {
            return Ok(());
        }
        let full_name = self.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()))?;
        let entity_name = full_name.as_str();
        let of_entity = |row: EntityValue| {
            if *row.entity_name == *full_name {
                Ok(row)
            } else {
                Err(Error::new(format!(
                    "a value of {} is not a value of {full_name}",
                    row.entity_name
                )))
            }
        };
        self.reject_view(entity_name, "create")?;
        let rules = load_feed::match_rules_with(self, entity_name, "create");
        if (crate::hooks::entered() && !rules.is_empty())
            || fk::dummy_fks(self)
            || !cache::writes_direct(self, entity_name, "create")?
        {
            for row in rows {
                self.create_value(&of_entity(row)?)?;
            }
            return Ok(());
        }
        // Declared first, so it drops last: the authorization cache is invalidated after the writes.
        let _note = WriteNote(self, entity_name);
        let tx = self.transactions_for(entity_name)?;
        let (defaults, fields, per_statement, dialect_name) =
            self.with_group(entity_name, |entity, dialect, _, _| {
                let fields: Vec<String> = entity.fields.iter().map(|field| field.name.clone()).collect();
                // as many rows as fit when every field is bound
                let per_statement = if moqui_db::sql::supports_insert_rows(dialect) {
                    (CREATE_BINDS / fields.len().max(1)).clamp(1, CREATE_ROWS)
                } else {
                    1
                };
                Ok((has_field_defaults(entity), fields, per_statement, dialect.name.clone()))
            })?;
        self.check_table_runtime(entity_name);
        let deferred = pipelining(&tx);
        let mut stack = HashSet::from([full_name.clone()]);
        // the fields a row so far set, which every statement from then on binds
        let mut bound_fields = vec![false; fields.len()];
        // the stamp a create sets, the same for every row of a transaction
        let stamp = self.write_stamp(entity_name);
        loop {
            // the statement's rows checked, under one look-up of the entity
            let mut checked: Vec<FieldMap> = Vec::with_capacity(per_statement);
            self.with_entity(entity_name, |entity, _, _| {
                for row in rows.by_ref().take(per_statement) {
                    let fields = of_entity(row)?.into_fields();
                    let owned = if defaults {
                        self.apply_field_defaults(entity_name, &fields)?
                    } else {
                        fields
                    };
                    let values = convert_fields(entity, owned, Some(|| stamp.clone()));
                    pk_values(entity, &values)?;
                    fk::enforce(self, entity_name, &values, &mut stack)?;
                    checked.push(values);
                }
                Ok(())
            })?;
            if checked.is_empty() {
                break;
            }
            let _artifact = EntityArtifact::push(self, entity_name, "create", "create")?;
            // a row's fields as sealed where sealing changed them, else none: the row binds its plain fields
            let mut sealed: Vec<Option<FieldMap>> = Vec::with_capacity(checked.len());
            let mut plain: Vec<Arc<FieldMap>> = Vec::with_capacity(checked.len());
            self.with_entity(entity_name, |entity, _, _| {
                for values in checked {
                    sealed.push(
                        match field_rw::before_write_shared(self, entity, "create", Cow::Borrowed(&values), None)? {
                            Cow::Owned(changed) => Some(changed),
                            Cow::Borrowed(_) => None,
                        },
                    );
                    plain.push(Arc::new(values));
                }
                Ok(())
            })?;
            let bound = |at: usize| sealed[at].as_ref().unwrap_or(&plain[at]);
            for (name, set) in fields.iter().zip(bound_fields.iter_mut()) {
                *set = *set
                    || (0..sealed.len()).any(|at| bound(at).get(name.as_str()).is_some_and(|value| !value.is_null()));
            }
            let names: Vec<&str> = fields
                .iter()
                .zip(&bound_fields)
                .filter(|(_, set)| **set)
                .map(|(name, _)| name.as_str())
                .collect();
            let key = format!("{full_name}|{dialect_name}|create-rows|{}|", names.join(","));
            let mut done = 0;
            while done < sealed.len() {
                // The largest size that fits, of a few, so the statement cache keeps a few texts of the entity.
                let size = CREATE_SIZES
                    .iter()
                    .copied()
                    .find(|size| *size <= sealed.len() - done)
                    .unwrap_or(1);
                let sql = super::super::sql_cache::cached(self, &format!("{key}{size}"), || {
                    self.with_group(entity_name, |entity, dialect, _, _| {
                        moqui_db::sql::insert_rows(entity, dialect, &names, size)
                    })
                })?;
                let mut params: Vec<Value> = Vec::with_capacity(size * names.len());
                for at in done..done + size {
                    params.extend(bind_columns(&names, bound(at)));
                }
                done += size;
                if deferred {
                    execute_deferred_tx(&tx, &sql, params, None)?;
                } else {
                    execute_tx(&tx, &sql, &params)?;
                }
            }
            if tx.lock_track_enabled() {
                for values in &plain {
                    if let Ok(keys) = self.with_entity(entity_name, |entity, _, _| pk_values(entity, values)) {
                        let pk = keys
                            .iter()
                            .map(|(name, value)| format!("{name}={}", moqui_lib::object::to_plain_string(value)))
                            .collect::<Vec<_>>()
                            .join(",");
                        tx.note_entity_lock(entity_name, &pk);
                    }
                }
            }
            load_feed::note_creates(self, entity_name, &plain);
            // As Java's `EntityCache`, a cache clear or cluster publish that fails is logged; the writes already ran.
            if let Err(err) = cache::on_create_rows(self, entity_name, &plain) {
                tracing::error!(error = %err, "Could not clear or publish the cache for the creates of {entity_name}");
            }
            after_statement(self, &tx, true);
        }
        Ok(())
    }
}

/// [`EntityFacade::normalize_values`] of `entity`: each field converted to its type, a field the entity does not have
/// left out, and with `stamp` (a create) a missing `lastUpdatedStamp` set to what it gives.
pub(super) fn convert_fields(
    entity: &EntityDefinition,
    mut values: FieldMap,
    stamp: Option<impl FnOnce() -> Value>,
) -> FieldMap {
    // Converted in place: the map is usually all fields of the entity, so it is not built again.
    let mut known = 0;
    for field in &entity.fields {
        let Some(value) = values.get_mut(field.name.as_str()) else {
            continue;
        };
        known += 1;
        let java = field.java_type.as_str();
        // Text is a script's or a caller's, so a timestamp without an offset is in the session time zone.
        if !moqui_lib::object::is_java_type(value, java) {
            *value = crate::hooks::basic_convert(value, java).value;
        }
    }
    if known != values.len() {
        values.retain(|name, _| entity.field(name).is_some());
    }
    if let Some(stamp) = stamp {
        if entity.field(STAMP).is_some() && values.get(STAMP).is_none_or(Value::is_null) {
            values.insert(Arc::from(STAMP), stamp());
        }
    }
    values
}

#[cfg(test)]
#[path = "create_values_tests.rs"]
mod tests;
