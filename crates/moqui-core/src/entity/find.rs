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

//! Entity find. The condition tree is `moqui_db`. A view uses `select_view`.

use moqui_db::sql::{
    count_view, count_with, select_view_with, select_with, Condition, ConditionValues, FieldRef, LimitOffset, OrderBy,
    SelectOptions,
};
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};

use super::cache::{self, FindLookup};
use super::exec::{after_statement, query_tx};
use super::facade::EntityFacade;
use super::field_rw;
use super::params::{entity_columns, map_row, view_columns};
use super::value::{EntityArtifact, EntityList, EntityValue};
use crate::driver::DbRow;
use crate::transaction::TransactionManager;

/// A find builder. Methods consume `self` so conditions chain. A clone is the same find, for a count beside a list.
#[derive(Clone)]
pub struct EntityFind {
    pub(super) facade: EntityFacade,
    entity_name: String,
    pub(super) conditions: Vec<Condition>,
    order: Vec<OrderBy>,
    pub(super) limit: Option<i64>,
    pub(super) offset: Option<i64>,
    distinct: bool,
    for_update: bool,
    pub(super) having: Vec<Condition>,
    select_fields: Vec<String>,
    /// Set by `select_form_fields`: order only by a selected field, as a form-list's find does.
    form_order_only: bool,
    /// Set by `require_search_form_parameters`: a find with no search parameter returns nothing.
    pub(super) require_search_parameters: bool,
    /// Set by `search_form_map` when the input made at least one condition.
    pub(super) has_search_parameters: bool,
    /// `disableAuthz`: this find is not checked against the user's authorizations.
    disable_authz: bool,
    /// `useCache`: `None` follows the entity's `cache` attribute.
    use_cache: Option<bool>,
}

impl EntityFind {
    pub(crate) fn new(facade: EntityFacade, entity_name: String) -> Self {
        Self {
            facade,
            entity_name,
            conditions: Vec::new(),
            order: Vec::new(),
            limit: None,
            offset: None,
            distinct: false,
            for_update: false,
            having: Vec::new(),
            select_fields: Vec::new(),
            form_order_only: false,
            require_search_parameters: false,
            has_search_parameters: false,
            disable_authz: false,
            use_cache: None,
        }
    }

    /// `useCache(flag)`: true reads through the entity cache even for an entity whose `cache` is not `true` (but not one
    /// marked `never`), false reads the database, as Java's `EntityFind.useCache`.
    #[must_use]
    pub fn use_cache(mut self, flag: bool) -> Self {
        self.use_cache = Some(flag);
        self
    }

    /// Run this find without the authorization check, as Java's `EntityFind.disableAuthz()`.
    #[must_use]
    pub fn disable_authz(mut self) -> Self {
        self.disable_authz = true;
        self
    }

    fn authz_guard(&self) -> Option<crate::context::AuthzGuard> {
        self.disable_authz
            .then(|| crate::context::ExecutionContext::with_current(|ec| ec.disable_authz()))
            .flatten()
    }

    pub fn entity_name(&self) -> &str {
        &self.entity_name
    }

    pub fn eq(mut self, field: &str, value: Value) -> Self {
        self.conditions.push(Condition::equals(field, value));
        self
    }

    pub fn matches(mut self, field: &str, value: Value) -> Self {
        self.conditions.push(Condition::matches(field, value));
        self
    }

    pub fn condition(mut self, condition: Condition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// A find that returns nothing unless `search_form_map` made a condition from the input. Java's
    /// `requireSearchFormParameters`: a list screen with no search yet shows no rows.
    pub fn require_search_form_parameters(mut self, require: bool) -> Self {
        self.require_search_parameters = require;
        self
    }

    pub fn order_by(mut self, field: &str, descending: bool) -> Self {
        let entry = if descending {
            OrderBy::desc(field)
        } else {
            OrderBy::asc(field)
        };
        self.order.push(entry);
        self
    }

    /// An order-by entry as Java writes it: `-field`, `^field` for upper case, `field NULLS FIRST`.
    pub fn order_by_text(mut self, text: &str) -> Self {
        self.order.extend(OrderBy::parse(text));
        self
    }

    pub fn order_by_entry(mut self, entry: OrderBy) -> Self {
        self.order.push(entry);
        self
    }

    /// `SELECT DISTINCT`. Not cached and not merged with held or sim rows.
    pub fn distinct(mut self, distinct: bool) -> Self {
        self.distinct = distinct;
        self
    }

    /// Lock the rows read until the transaction ends. The find fails outside a transaction. It
    /// reads from the database, not from the entity cache or the rows a transaction cache holds.
    pub fn for_update(mut self, lock: bool) -> Self {
        self.for_update = lock;
        self
    }

    /// A condition on an aggregate of a view-entity (`function` aliases). It goes in `HAVING`.
    pub fn having(mut self, condition: Condition) -> Self {
        self.having.push(condition);
        self
    }

    /// Read only these fields. A field the entity does not have is an error. The values returned
    /// have just these fields. On a view-entity only the members those fields read are joined,
    /// and with an aggregate alias the selected fields are the `GROUP BY`. As in Java, `field` may
    /// be a comma-separated list, and a field already selected is not added again.
    pub fn select_field(mut self, field: &str) -> Self {
        for name in field.split(',').map(str::trim).filter(|name| !name.is_empty()) {
            if !self.select_fields.iter().any(|known| known == name) {
                self.select_fields.push(name.to_string());
            }
        }
        self
    }

    /// The fields a form-list shows and hides, for the find of its `entity-find`, as Java's
    /// `FormListRenderInfo.getListObject`: each name that is a field of the entity is selected, and an
    /// order-by field that is not selected is dropped, so a `DISTINCT` or an aggregate view groups by
    /// what the form shows.
    pub fn select_form_fields(mut self, fields: &str) -> Self {
        let names: Vec<String> = self
            .facade
            .with_entity(&self.entity_name, |entity, _, _| {
                Ok(fields
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty() && entity.has_field(name))
                    .map(str::to_string)
                    .collect())
            })
            .unwrap_or_default();
        for name in names {
            if !self.select_fields.contains(&name) {
                self.select_fields.push(name);
            }
        }
        self.form_order_only = true;
        self
    }

    /// The fields [`Self::select_field`] added, in order.
    pub fn selected_fields(&self) -> &[String] {
        &self.select_fields
    }

    /// True when the result depends on more than the conditions, order, and paging, so the entity
    /// cache and the held-row overlay, which key on those alone, must not answer.
    fn bypasses_cache(&self) -> bool {
        self.distinct || self.for_update || !self.having.is_empty() || !self.select_fields.is_empty()
    }

    pub fn limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: i64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// The limit set by `limit` or a paginated `search_form_map`.
    pub fn limit_value(&self) -> Option<i64> {
        self.limit
    }

    /// The offset set by `offset` or a paginated `search_form_map`.
    pub fn offset_value(&self) -> Option<i64> {
        self.offset
    }

    pub fn list(self) -> Result<EntityList, Error> {
        self.list_internal("list")
    }

    fn list_internal(self, operation: &str) -> Result<EntityList, Error> {
        if self.require_search_parameters && !self.has_search_parameters {
            return Ok(EntityList::new(Vec::new()));
        }
        // The view frame is what authorizes the read, and the filters come from its record.
        let _authz = self.authz_guard();
        let _artifact = EntityArtifact::push(&self.facade, &self.entity_name, "view", operation)?;
        if crate::deterministic::enabled() {
            let tx = self.facade.transactions_for(&self.entity_name)?;
            return tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
                self.list_in_transaction()
            });
        }
        self.list_in_transaction()
    }

    fn list_in_transaction(self) -> Result<EntityList, Error> {
        // Java's listExtended returns an empty list when the physical table is absent.
        // Keep the existence probe and the read in the same facade transaction.
        if crate::deterministic::enabled()
            && !self.facade.with_entity(&self.entity_name, |entity, _, _| Ok(entity.is_view))?
            && !self.facade.table_exists(&self.entity_name)?
        {
            return Ok(EntityList::new(Vec::new()));
        }
        let condition = self.combined();
        let lookup = self.lookup(&condition);
        let bypass = self.bypasses_cache();
        if !bypass {
            if let Some(cached) = cache::before_find(&self.facade, &lookup) {
                crate::deterministic::trace("cache", "hit", &self.entity_name, "succeeded")?;
                return self.localize_list(cached);
            }
        }
        crate::deterministic::trace(
            "cache",
            if bypass { "bypass" } else { "miss" },
            &self.entity_name,
            "succeeded",
        )?;
        self.require_transaction_for_lock()?;
        let plan = self.plan(false)?;
        let rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
        after_statement(&self.facade, &plan.tx, false);
        let values = self.rows_to_values(&plan, &rows)?;
        let list = EntityList::new(values);
        if !bypass {
            cache::after_find(&self.facade, &lookup, &list)?;
        }
        self.localize_list(list)
    }

    pub fn one(self) -> Result<Option<EntityValue>, Error> {
        self.one_internal("one")
    }

    pub(crate) fn one_internal(mut self, operation: &str) -> Result<Option<EntityValue>, Error> {
        self.narrow_to_full_pk();
        let entity_name = self.entity_name.clone();
        let list = self.list_internal(operation)?;
        // As Java `oneExtended`: the first row, more than one only traced. A find one with a partial key, such as
        // Mantle's `entity-find-one` of an OrderPart with only the customer in the context, takes the first match.
        if list.len() > 1 {
            tracing::trace!(entity = %entity_name, rows = list.len(), "found more than one result for find one");
        }
        Ok(list.values().first().cloned())
    }

    /// Java `oneInternal`: a find one whose field equalities hold the full primary key, with more besides (a
    /// `condition(context)` puts every field of the context in, nulls as `IS NULL`), finds by the primary key alone.
    /// Other kinds of condition stay.
    fn narrow_to_full_pk(&mut self) {
        let pk_names: Vec<String> = match self.facade.with_entity(&self.entity_name, |entity, _, _| {
            Ok(entity.pk_field_names().iter().map(|name| (*name).to_string()).collect())
        }) {
            Ok(names) => names,
            Err(_) => return,
        };
        let equals_on = |condition: &Condition, name: &str| match condition {
            Condition::Equals(field, value) => {
                field.entity_alias.is_none() && field.name == name && !moqui_lib::object::is_empty(value)
            }
            _ => false,
        };
        let has_full_pk = !pk_names.is_empty()
            && pk_names
                .iter()
                .all(|name| self.conditions.iter().any(|condition| equals_on(condition, name)));
        // A null in the map is an `IS NULL` here; Java keeps it in the same simple map as the equalities.
        let equalities = self
            .conditions
            .iter()
            .filter(|condition| matches!(condition, Condition::Equals(..) | Condition::IsNull(..)))
            .count();
        if !has_full_pk || equalities <= pk_names.len() {
            return;
        }
        let mut kept: Vec<Condition> = Vec::new();
        for condition in std::mem::take(&mut self.conditions) {
            if let Condition::IsNull(field) = &condition {
                if field.entity_alias.is_none() {
                    continue;
                }
            }
            let Condition::Equals(field, _) = &condition else {
                kept.push(condition);
                continue;
            };
            let first_for_pk = pk_names.contains(&field.name)
                && !kept
                    .iter()
                    .any(|known| matches!(known, Condition::Equals(other, _) if other.name == field.name));
            if first_for_pk && equals_on(&condition, &field.name) {
                kept.push(condition);
            }
        }
        self.conditions = kept;
    }

    pub fn count(self) -> Result<i64, Error> {
        if self.require_search_parameters && !self.has_search_parameters {
            return Ok(0);
        }
        let _authz = self.authz_guard();
        let _artifact = EntityArtifact::push(&self.facade, &self.entity_name, "view", "count")?;
        let condition = self.combined();
        if let Some(count) = cache::before_count(&self.facade, &self.entity_name, condition.as_ref()) {
            crate::deterministic::trace("cache", "hit", &self.entity_name, "succeeded")?;
            return Ok(count);
        }
        crate::deterministic::trace("cache", "miss", &self.entity_name, "succeeded")?;
        let plan = self.plan(true)?;
        let rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
        after_statement(&self.facade, &plan.tx, false);
        let value = rows
            .first()
            .and_then(|row| row.columns.first())
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Null);
        match basic_convert(&value, "Long").value {
            Value::I64(number) => Ok(number),
            other => Err(Error::new(format!("count returned {other:?}"))),
        }
    }

    /// Open a cursor when this find has no transaction hold and no sim schema.
    ///
    /// A hold or a sim schema is merged in memory. The cursor stays closed, so an open stream is
    /// not asked to mix in rows that are not in the database yet. The cursor path needs an active
    /// transaction: the open stream is that transaction's enlisted connection. Drop closes it.
    pub fn iterator(self) -> Result<EntityListIterator, Error> {
        if self.require_search_parameters && !self.has_search_parameters {
            return Ok(EntityListIterator::memory(
                self.facade,
                self.entity_name,
                EntityList::new(Vec::new()),
            ));
        }
        let _authz = self.authz_guard();
        let _artifact = EntityArtifact::push(&self.facade, &self.entity_name, "view", "iterator")?;
        let condition = self.combined();
        let lookup = self.lookup(&condition);
        if !self.bypasses_cache() {
            if let Some(cached) = cache::before_find(&self.facade, &lookup) {
                crate::deterministic::trace("cache", "hit", &self.entity_name, "succeeded")?;
                return Ok(EntityListIterator::memory(self.facade, self.entity_name, cached));
            }
        }
        crate::deterministic::trace(
            "cache",
            if self.bypasses_cache() { "bypass" } else { "miss" },
            &self.entity_name,
            "succeeded",
        )?;
        self.require_transaction_for_lock()?;
        let plan = self.plan(false)?;
        plan.tx
            .with_connection(|conn| conn.open_stream(&plan.sql, &plan.params))?;
        after_statement(&self.facade, &plan.tx, false);
        Ok(EntityListIterator::stream(
            self.facade,
            self.entity_name,
            plan.columns,
            plan.tx,
        ))
    }

    fn require_transaction_for_lock(&self) -> Result<(), Error> {
        if self.for_update && !self.facade.transactions_for(&self.entity_name)?.in_transaction() {
            return Err(Error::new(format!(
                "find of {} for update needs a transaction in place",
                self.entity_name
            )));
        }
        Ok(())
    }

    fn lookup(&self, condition: &Option<Condition>) -> FindLookup {
        FindLookup {
            entity_name: self.entity_name.clone(),
            use_cache: self.use_cache,
            condition: condition.clone(),
            order: self.order.clone(),
            limit: self.limit,
            offset: self.offset,
        }
    }

    fn combined(&self) -> Option<Condition> {
        let condition = match self.conditions.len() {
            0 => None,
            1 => Some(self.conditions[0].clone()),
            _ => Some(Condition::and(self.conditions.clone())),
        };
        if !authorization_filters() {
            return condition;
        }
        crate::user::apply_entity_filters(&self.entity_name, condition)
    }

    fn plan(&self, count_only: bool) -> Result<ReadPlan, Error> {
        let condition = self.combined();
        let page = LimitOffset {
            limit: self.limit,
            offset: self.offset,
        };
        let having = match self.having.len() {
            0 => None,
            1 => Some(self.having[0].clone()),
            _ => Some(Condition::and(self.having.clone())),
        };
        let options = SelectOptions {
            distinct: self.distinct,
            for_update: self.for_update,
            having,
            fields: self.select_fields.clone(),
        };
        self.facade.with_group(&self.entity_name, |entity, dialect, model, tx| {
            let condition = field_rw::encrypt_condition(&self.facade, entity, condition.clone())?;
            for name in &self.select_fields {
                if !entity.has_field(name) {
                    return Err(Error::new(format!("{} has no field {name}", entity.full_name)));
                }
            }
            let wanted =
                |name: &str| self.select_fields.is_empty() || self.select_fields.iter().any(|field| field == name);
            // Java's `orderBy` skips a name that is not a field of the entity, such as a default
            // order a screen names for a column its entity does not have.
            let order: Vec<OrderBy> = self
                .order
                .iter()
                .filter(|entry| entry.field.entity_alias.is_some() || entity.has_field(&entry.field.name))
                .filter(|entry| !self.form_order_only || self.select_fields.contains(&entry.field.name))
                .cloned()
                .collect();
            let (statement, mut columns) = if entity.is_view {
                let statement = if count_only {
                    count_view(entity, model, dialect, &FindValues, condition.as_ref(), &options)?
                } else {
                    select_view_with(
                        entity,
                        model,
                        dialect,
                        &FindValues,
                        condition.as_ref(),
                        &order,
                        page,
                        &options,
                    )?
                };
                (statement, view_columns(entity))
            } else if count_only {
                (count_with(entity, dialect, condition.as_ref(), &options)?, Vec::new())
            } else {
                let fields: Vec<FieldRef> = entity
                    .fields
                    .iter()
                    .filter(|field| wanted(&field.name))
                    .map(|field| FieldRef::new(&field.name))
                    .collect();
                let statement = select_with(entity, dialect, &fields, condition.as_ref(), &order, page, &options)?;
                (statement, entity_columns(entity, model))
            };
            if count_only {
                columns.clear();
            } else {
                columns.retain(|(name, _)| wanted(name));
            }
            Ok(ReadPlan {
                sql: statement.sql,
                params: statement.params,
                columns,
                tx: tx.clone(),
            })
        })
    }

    fn rows_to_values(&self, plan: &ReadPlan, rows: &[DbRow]) -> Result<Vec<EntityValue>, Error> {
        let facade = self.facade.clone();
        self.facade.with_entity(&self.entity_name, |entity, _, _| {
            let mut values = Vec::with_capacity(rows.len());
            for row in rows {
                let mapped = map_row(&plan.columns, row);
                let adjusted = field_rw::decrypt_stored(&facade, entity, mapped)?;
                values.push(EntityValue::from_fields(entity.full_name.clone(), adjusted));
            }
            Ok(values)
        })
    }

    /// Localization uses the locale at read time. The list cache keeps the decrypted row.
    fn localize_list(&self, list: EntityList) -> Result<EntityList, Error> {
        let facade = self.facade.clone();
        self.facade.with_entity(&self.entity_name, |entity, _, _| {
            let mut values = Vec::with_capacity(list.len());
            for row in list.values() {
                let adjusted = field_rw::localize_stored(&facade, entity, row.fields().clone())?;
                values.push(EntityValue::from_fields(row.entity_name().to_string(), adjusted));
            }
            Ok(EntityList::new(values))
        })
    }
}

/// What a view's `entity-condition` and SQL expressions read: the time now, `${}` expansion in
/// the script runtime, and plain conversion of definition text.
pub(crate) struct FindValues;

impl ConditionValues for FindValues {
    /// Java's `ec.user.nowTimestamp`, so a date filter follows the effective time.
    fn now(&self) -> Value {
        basic_convert(&Value::I64(crate::user::now_effective_millis()), "java.sql.Timestamp").value
    }

    fn expand(&self, text: &str) -> Result<String, Error> {
        crate::script::expand(text).map_err(|err| Error::new(err.message()))
    }

    fn convert(&self, text: &str, java_type: &str) -> Result<Value, Error> {
        if text.is_empty() || text == "null" {
            return Ok(Value::Null);
        }
        Ok(basic_convert(&Value::string(text), java_type).value)
    }
}

fn authorization_filters() -> bool {
    if !crate::user::context_entered() {
        return false;
    }
    crate::context::ExecutionContext::with_current(|ctx| !ctx.authz_disabled()).unwrap_or(false)
}

struct ReadPlan {
    sql: String,
    params: Vec<Value>,
    columns: Vec<(String, String)>,
    tx: TransactionManager,
}

/// A streaming find. Drop calls `close_stream` and releases the pinned connection.
pub struct EntityListIterator {
    facade: EntityFacade,
    entity_name: String,
    columns: Vec<(String, String)>,
    tx: Option<TransactionManager>,
    open: bool,
    memory: Vec<EntityValue>,
    index: usize,
}

impl EntityListIterator {
    fn stream(
        facade: EntityFacade,
        entity_name: String,
        columns: Vec<(String, String)>,
        tx: TransactionManager,
    ) -> Self {
        Self {
            facade,
            entity_name,
            columns,
            tx: Some(tx),
            open: true,
            memory: Vec::new(),
            index: 0,
        }
    }

    fn memory(facade: EntityFacade, entity_name: String, list: EntityList) -> Self {
        Self {
            facade,
            entity_name,
            columns: Vec::new(),
            tx: None,
            open: false,
            memory: list.values().to_vec(),
            index: 0,
        }
    }

    pub fn next_value(&mut self) -> Result<Option<EntityValue>, Error> {
        if !self.memory.is_empty() || !self.open {
            if self.index >= self.memory.len() {
                return Ok(None);
            }
            let value = self.memory[self.index].clone();
            self.index += 1;
            let facade = self.facade.clone();
            let adjusted = self.facade.with_entity(&self.entity_name, |entity, _, _| {
                field_rw::localize_stored(&facade, entity, value.fields().clone())
            })?;
            return Ok(Some(EntityValue::from_fields(
                value.entity_name().to_string(),
                adjusted,
            )));
        }
        let Some(tx) = self.tx.clone() else {
            return Ok(None);
        };
        let row = tx.with_connection(|conn| conn.next_row())?;
        match row {
            Some(row) => {
                let mapped = map_row(&self.columns, &row);
                let facade = self.facade.clone();
                let hook = facade.clone();
                let adjusted = facade.with_entity(&self.entity_name, |entity, _, _| {
                    field_rw::after_read(&hook, entity, mapped)
                })?;
                Ok(Some(EntityValue::from_fields(self.entity_name.clone(), adjusted)))
            }
            None => {
                self.close()?;
                Ok(None)
            }
        }
    }

    pub fn close(&mut self) -> Result<(), Error> {
        if !self.open {
            return Ok(());
        }
        let Some(tx) = self.tx.clone() else {
            self.open = false;
            return Ok(());
        };
        tx.with_connection(|conn| conn.close_stream())?;
        self.open = false;
        Ok(())
    }
}

impl Drop for EntityListIterator {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use moqui_db::sql::{CompareOp, Condition};
    use moqui_lib::object::Value;

    fn seeded() -> (crate::entity::EntityFacade, Arc<Mutex<Vec<String>>>) {
        let tape = Arc::new(Mutex::new(Vec::new()));
        let facade = crate::testing::open_with_tape(Arc::clone(&tape));
        for (id, medium) in [("A", "alpha"), ("B", "beta")] {
            let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
            row.set("testId", Value::string(id));
            row.set("testMedium", Value::string(medium));
            row.create(&facade).unwrap();
        }
        (facade, tape)
    }

    #[test]
    fn select_field_reads_only_those_columns_and_rejects_a_missing_one() {
        let (facade, tape) = seeded();
        let rows = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .select_field("testId")
            .eq("testId", Value::string("A"))
            .list()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(
            rows.values()[0].get("testMedium").is_none(),
            "only the selected field is read"
        );
        let sql = tape.lock().unwrap().last().cloned().unwrap();
        assert!(sql.starts_with("SELECT \"test_id\" AS \"testId\" FROM"), "{sql}");
        let err = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .select_field("nope")
            .list()
            .unwrap_err();
        assert!(err.to_string().contains("no field nope"), "{err}");
    }

    #[test]
    fn an_iterator_that_requires_search_parameters_finds_nothing_without_them() {
        let (facade, tape) = seeded();
        let before = tape.lock().unwrap().len();
        let mut found = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .require_search_form_parameters(true)
            .iterator()
            .unwrap();
        assert!(found.next_value().unwrap().is_none());
        assert_eq!(tape.lock().unwrap().len(), before, "no statement runs");
    }

    #[test]
    fn for_update_needs_a_transaction_and_is_written_inside_one() {
        let (facade, tape) = seeded();
        let err = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .for_update(true)
            .list()
            .unwrap_err();
        assert!(err.to_string().contains("transaction"), "{err}");
        facade.begin("moqui.test.TestEntity", None).unwrap();
        let outcome = facade.find("moqui.test.TestEntity").unwrap().for_update(true).list();
        facade.rollback("moqui.test.TestEntity").unwrap();
        // The in-memory driver does not read FOR UPDATE; the statement is what counts here.
        let _ = outcome;
        assert!(tape.lock().unwrap().iter().any(|sql| sql.ends_with(" FOR UPDATE")));
    }

    #[test]
    fn distinct_and_having_reach_the_statement() {
        let (facade, tape) = seeded();
        let _ = facade.find("moqui.test.TestEntity").unwrap().distinct(true).list();
        assert!(tape
            .lock()
            .unwrap()
            .iter()
            .any(|sql| sql.starts_with("SELECT DISTINCT ")));
        // A plain entity has no aggregate to apply a having condition to.
        let err = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .having(Condition::greater("testId", Value::I32(1)))
            .list()
            .unwrap_err();
        assert!(err.to_string().contains("no aggregate"), "{err}");
    }

    #[test]
    fn ignore_case_binds_upper_case_text_and_field_comparisons_reach_the_statement() {
        let (facade, tape) = seeded();
        // The in-memory driver cannot read UPPER(); the statement and its bound values are the point.
        let _ = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .condition(Condition::ignore_case(Condition::equals(
                "testMedium",
                Value::string("Alpha"),
            )))
            .list();
        let _ = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .condition(Condition::compare_fields("testId", CompareOp::LessEqual, "testMedium"))
            .list();
        let tape = tape.lock().unwrap();
        assert!(
            tape.iter()
                .any(|sql| sql.ends_with("WHERE UPPER(\"test_medium\") = UPPER(?)")),
            "{tape:?}"
        );
        assert!(
            tape.iter()
                .any(|sql| sql.ends_with("WHERE \"test_id\" <= \"test_medium\"")),
            "{tape:?}"
        );
        let dialect = moqui_db::dialect::Dialects::load(
            &moqui_lib::node::MNode::parse_text(
                "conf",
                r#"<database-list><database name="postgres"/></database-list>"#,
            )
            .unwrap(),
        )
        .unwrap();
        let rendered = moqui_db::sql::render_condition(
            &Condition::ignore_case(Condition::equals("x", Value::string("Alpha"))),
            dialect.get("postgres").unwrap(),
        )
        .unwrap();
        assert_eq!(rendered.params, vec![Value::string("ALPHA")]);
    }
}
