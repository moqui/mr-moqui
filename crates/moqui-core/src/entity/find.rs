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

use std::sync::Arc;

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
use super::sim_view;
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
    pub(super) for_update: bool,
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
    pub(super) use_cache: Option<bool>,
    /// Java's scroll-insensitive result set type: the iterator can move back. See [`EntityListIterator`].
    scrollable: bool,
    /// Java's `useClone`: read from a clone datasource of the entity's group when one is open.
    use_clone: bool,
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
            scrollable: false,
            use_clone: false,
        }
    }

    /// Java's `useClone(true)`: read from a clone of the entity's group (`{group}#clone1` and so on) when one is open,
    /// such as a read replica for reports. A find for update stays on the group itself.
    #[must_use]
    pub fn use_clone(mut self, on: bool) -> Self {
        self.use_clone = on;
        self
    }

    /// An iterator that can move back and jump, as Java's `resultSetType(TYPE_SCROLL_INSENSITIVE)`. It keeps the rows
    /// it reads.
    #[must_use]
    pub fn scrollable(mut self, on: bool) -> Self {
        self.scrollable = on;
        self
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

    /// Java's `condition(field, value)`: a second value for the same field replaces the first.
    pub fn eq(mut self, field: &str, value: Value) -> Self {
        add_condition(&mut self.conditions, Condition::equals(field, value));
        self
    }

    pub fn matches(mut self, field: &str, value: Value) -> Self {
        self.conditions.push(Condition::matches(field, value));
        self
    }

    /// As Java's `condition(EntityCondition)`: an equality (or `IS NULL`) on a plain field replaces the one the find
    /// already has for that field, and an `AND` is unrolled so its equalities do the same.
    pub fn condition(mut self, condition: Condition) -> Self {
        add_condition(&mut self.conditions, condition);
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

    /// Read only these fields. A field the entity does not have is skipped with a warning when the
    /// find runs, as Java's `selectField`. The values returned have just these fields. On a
    /// view-entity only the members those fields read are joined, and with an aggregate alias the
    /// selected fields are the `GROUP BY`. As in Java, `field` may be a comma-separated list, and a
    /// field already selected is not added again.
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
    /// cache and the held-row overlay, which key on those alone, must not answer. Java caches a distinct or
    /// select-field find of a cached entity by reading every field, so it answers with whole, non-distinct rows; this
    /// reads the database and returns what was asked for.
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
        cache::sim_check_find(&self.facade, &self.entity_name)?;
        let condition = self.combined();
        let lookup = self.lookup(&condition);
        if let Some(schema) = self.sim_view_schema(false)? {
            let plan = self.plan(false)?;
            let rows = cache::query_sim(&self.facade, &schema, &plan.sql, &plan.params)?;
            let values = self.rows_to_values(&plan, &rows)?;
            return Ok(self.attach_l10n(EntityList::new(values)));
        }
        let bypass = self.bypasses_cache();
        if !bypass {
            if let Some(cached) = cache::before_find(&self.facade, &lookup) {
                return Ok(self.attach_l10n(cached));
            }
        }
        // As Java: a find on a table that does not exist finds nothing.
        if !self.facade.table_ready(&self.entity_name) {
            return Ok(EntityList::new(Vec::new()));
        }
        self.require_transaction_for_lock()?;
        let plan = self.plan(false)?;
        let rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
        after_statement(&self.facade, &plan.tx, false);
        let values = self.rows_to_values(&plan, &rows)?;
        let list = EntityList::new(values);
        if !bypass {
            // The rows were read; a cache that cannot hold them only costs the next read.
            if let Err(err) = cache::after_find(&self.facade, &lookup, &list) {
                tracing::warn!(error = %err, "Could not cache the find of {}", self.entity_name);
            }
        }
        Ok(self.attach_l10n(list))
    }

    /// Java `updateAll`: set `fields` on each row found and update the rows that change, so each update runs its
    /// entity ECA rules. Returns how many rows were updated. Not for a `create-only` entity. The find skips the cache.
    pub fn update_all(self, fields: impl IntoIterator<Item = (String, Value)>) -> Result<i64, Error> {
        let fields: Vec<(String, Value)> = fields.into_iter().collect();
        let _authz = self.authz_guard();
        let facade = self.facade.clone();
        self.refuse_create_only("updated")?;
        let list = self.use_cache(false).list()?;
        let mut updated = 0_i64;
        for value in list.values() {
            let mut changed = value.clone();
            let mut modified = false;
            for (name, new_value) in &fields {
                let same = value.get(name).is_some_and(|old| {
                    old == new_value
                        || moqui_lib::object::to_plain_string(old) == moqui_lib::object::to_plain_string(new_value)
                });
                modified |= !same;
                changed.set(name.clone(), new_value.clone());
            }
            if modified {
                changed.update(&facade)?;
                updated += 1;
            }
        }
        Ok(updated)
    }

    /// Java `deleteAll`: delete each row found through the value path, so each delete runs its entity ECA rules.
    /// Returns how many rows were deleted. Not for a `create-only` entity. The find skips the cache.
    pub fn delete_all(self) -> Result<i64, Error> {
        let _authz = self.authz_guard();
        let facade = self.facade.clone();
        self.refuse_create_only("deleted")?;
        let list = self.use_cache(false).list()?;
        for value in list.values() {
            value.delete(&facade)?;
        }
        Ok(i64::try_from(list.len()).unwrap_or(i64::MAX))
    }

    fn refuse_create_only(&self, verb: &str) -> Result<(), Error> {
        self.facade.with_entity(&self.entity_name, |entity, _, _| {
            if entity.create_only {
                return Err(Error::new(format!(
                    "Entity {} is create-only (immutable), cannot be {verb}.",
                    entity.full_name
                )));
            }
            Ok(())
        })
    }

    pub fn one(self) -> Result<Option<EntityValue>, Error> {
        self.one_internal("one")
    }

    pub(crate) fn one_internal(mut self, operation: &str) -> Result<Option<EntityValue>, Error> {
        // Java `oneInternal`: a find one with no condition, or with a primary key field set to null or empty, is null
        // without a query.
        if self.conditions.is_empty() || self.has_empty_pk() {
            // Still a `view` of the entity, authorized and counted as a hit.
            let _authz = self.authz_guard();
            let _artifact = EntityArtifact::push(&self.facade, &self.entity_name, "view", operation)?;
            return Ok(None);
        }
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

    /// A primary key field compared to null or an empty value, as Java's `hasEmptyPk`.
    fn has_empty_pk(&self) -> bool {
        let Ok(pk_names) = self.facade.with_entity(&self.entity_name, |entity, _, _| {
            Ok(entity
                .pk_field_names()
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>())
        }) else {
            return false;
        };
        self.conditions.iter().any(|condition| match condition {
            Condition::Equals(field, value) => {
                field.entity_alias.is_none() && pk_names.contains(&field.name) && moqui_lib::object::is_empty(value)
            }
            Condition::IsNull(field) => field.entity_alias.is_none() && pk_names.contains(&field.name),
            _ => false,
        })
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
        cache::sim_check_find(&self.facade, &self.entity_name)?;
        if let Some(schema) = self.sim_view_schema(true)? {
            let plan = self.plan(true)?;
            let rows = cache::query_sim(&self.facade, &schema, &plan.sql, &plan.params)?;
            return count_of(&rows);
        }
        let condition = self.combined();
        let lookup = self.lookup(&condition);
        let bypass = self.bypasses_cache();
        if let Some(count) = cache::before_count(&self.facade, &lookup).filter(|_| !bypass) {
            return Ok(count);
        }
        if !self.facade.table_ready(&self.entity_name) {
            return Ok(0);
        }
        let plan = self.plan(true)?;
        let rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
        after_statement(&self.facade, &plan.tx, false);
        let count = count_of(&rows)?;
        if !bypass {
            if let Err(err) = cache::after_count(&self.facade, &lookup, count) {
                tracing::warn!(error = %err, "Could not cache the count of {}", self.entity_name);
            }
        }
        Ok(count)
    }

    /// A view the sim changed is read from the sim schema, once this copies the member rows the read needs there; see
    /// [`super::sim_view`]. `None` when production answers. The copy starts from production's answer to this find
    /// with every alias, its page widened to hold the rows the sim deleted, as Java's `copyWorkingSet`.
    fn sim_view_schema(&self, for_count: bool) -> Result<Option<String>, Error> {
        let Some(plan) = sim_view::plan_for(&self.facade, &self.entity_name)? else {
            return Ok(None);
        };
        let schema = plan.prepare(&self.facade, || {
            let mut production = self.clone();
            production.select_fields.clear();
            production.form_order_only = false;
            production.use_cache = Some(false);
            let slack = i64::try_from(cache::sim_deleted_count())
                .unwrap_or(i64::MAX)
                .saturating_add(1);
            production.limit = if for_count {
                None
            } else {
                self.limit
                    .map(|limit| limit.saturating_add(self.offset.unwrap_or(0)).saturating_add(slack))
            };
            production.offset = None;
            Ok(production.list()?.into_values())
        })?;
        Ok(Some(schema))
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
        cache::sim_check_find(&self.facade, &self.entity_name)?;
        // A view the sim changed is read from the sim schema, into memory.
        if sim_view::plan_for(&self.facade, &self.entity_name)?.is_some() {
            let facade = self.facade.clone();
            let entity_name = self.entity_name.clone();
            return Ok(EntityListIterator::memory(facade, entity_name, self.list()?));
        }
        let _authz = self.authz_guard();
        let _artifact = EntityArtifact::push(&self.facade, &self.entity_name, "view", "iterator")?;
        let condition = self.combined();
        let lookup = self.lookup(&condition);
        if !self.bypasses_cache() {
            if let Some(cached) = cache::before_find(&self.facade, &lookup) {
                return Ok(EntityListIterator::memory(self.facade, self.entity_name, cached));
            }
        }
        if !self.facade.table_ready(&self.entity_name) {
            return Ok(EntityListIterator::memory(
                self.facade,
                self.entity_name,
                EntityList::new(Vec::new()),
            ));
        }
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
            self.scrollable,
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
            filtered: *condition != self.unfiltered(),
            entity_name: self.entity_name.clone(),
            use_cache: self.use_cache,
            condition: condition.clone(),
            order: self.order.clone(),
            limit: self.limit,
            offset: self.offset,
        }
    }

    /// The find's own conditions, before the user's authorization filters.
    fn unfiltered(&self) -> Option<Condition> {
        match self.conditions.len() {
            0 => None,
            1 => Some(self.conditions[0].clone()),
            _ => Some(Condition::and(self.conditions.clone())),
        }
    }

    fn combined(&self) -> Option<Condition> {
        let condition = self.unfiltered();
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
        let use_clone = self.use_clone && !self.for_update;
        self.facade
            .with_read_group(&self.entity_name, use_clone, |entity, dialect, model, tx| {
                let condition = field_rw::encrypt_condition(&self.facade, entity, condition.clone())?;
                // Java's `selectField` skips a name that is not a field of the entity.
                let select_fields: Vec<String> = self
                    .select_fields
                    .iter()
                    .filter(|name| {
                        let known = entity.has_field(name);
                        if !known {
                            tracing::warn!("{} has no field {name}, not selecting it", entity.full_name);
                        }
                        known
                    })
                    .cloned()
                    .collect();
                let options = SelectOptions {
                    distinct: self.distinct,
                    for_update: self.for_update,
                    having: having.clone(),
                    fields: select_fields.clone(),
                };
                let wanted = |name: &str| select_fields.is_empty() || select_fields.iter().any(|field| field == name);
                // Java's `orderBy` skips a name that is not a field of the entity, such as a default
                // order a screen names for a column its entity does not have.
                let order: Vec<OrderBy> = self
                    .order
                    .iter()
                    .filter(|entry| entry.field.entity_alias.is_some() || entity.has_field(&entry.field.name))
                    .filter(|entry| !self.form_order_only || select_fields.contains(&entry.field.name))
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

    /// Each value reads its localized fields in the locale of the time it is read. The list cache keeps the stored row.
    fn attach_l10n(&self, list: EntityList) -> EntityList {
        let Some(targets) = find_l10n(&self.facade, &self.entity_name) else {
            return list;
        };
        EntityList::new(
            list.into_values()
                .into_iter()
                .map(|value| value.with_l10n(Some(&targets)))
                .collect(),
        )
    }
}

/// Java keeps one value per field in `simpleAndMap`: `condition(field, value)`, and an `EQUALS` condition routed to it,
/// replace the field's earlier value in place. Other conditions are ANDed in order; an `AND` list is unrolled.
pub(super) fn add_condition(conditions: &mut Vec<Condition>, condition: Condition) {
    let simple_field = |condition: &Condition| match condition {
        Condition::Equals(field, _) | Condition::IsNull(field)
            if field.entity_alias.is_none() && field.expr.is_none() =>
        {
            Some(field.name.clone())
        }
        _ => None,
    };
    if let Condition::And(list) = condition {
        for item in list {
            add_condition(conditions, item);
        }
        return;
    }
    if let Some(name) = simple_field(&condition) {
        if let Some(slot) = conditions
            .iter_mut()
            .find(|known| simple_field(known).as_deref() == Some(name.as_str()))
        {
            *slot = condition;
            return;
        }
    }
    conditions.push(condition);
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

/// The localized fields of `entity_name`, for the values a find of it returns.
fn find_l10n(facade: &EntityFacade, entity_name: &str) -> Option<Arc<field_rw::L10nTargets>> {
    facade
        .with_entity(entity_name, |entity, _, _| Ok(field_rw::l10n_targets(facade, entity)))
        .ok()
        .flatten()
}

/// The number a count query returned.
fn count_of(rows: &[DbRow]) -> Result<i64, Error> {
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

/// Java's `EntityListIterator`: a cursor over a find's rows, read from the database as it moves. `next_value` moves
/// forward. With [`EntityFind::scrollable`] (Java's scroll-insensitive result set) it keeps the rows read, so
/// `previous_value`, `absolute`, `relative` and `get_partial_list` can go anywhere; otherwise it only moves forward and
/// holds no more than two rows, so a large export streams. Rows from the cache or the overlay are in memory and scroll.
pub struct EntityListIterator {
    facade: EntityFacade,
    entity_name: String,
    columns: Vec<(String, String)>,
    tx: Option<TransactionManager>,
    open: bool,
    /// In memory: every row.
    memory: Option<Vec<EntityValue>>,
    /// The localized fields each row returned reads.
    l10n: Option<Arc<field_rw::L10nTargets>>,
    scrollable: bool,
    /// A scrollable stream's rows read so far, by index.
    seen: Vec<EntityValue>,
    /// A forward-only stream's last rows read, by index.
    window: std::collections::VecDeque<(usize, EntityValue)>,
    /// Rows read from the stream so far.
    consumed: usize,
    /// The current row's index: -1 before the first; at or past the end after the last.
    current: i64,
    fetch_size: Option<usize>,
}

impl EntityListIterator {
    fn stream(
        facade: EntityFacade,
        entity_name: String,
        columns: Vec<(String, String)>,
        tx: TransactionManager,
        scrollable: bool,
    ) -> Self {
        let l10n = find_l10n(&facade, &entity_name);
        Self {
            facade,
            entity_name,
            columns,
            tx: Some(tx),
            open: true,
            memory: None,
            l10n,
            scrollable,
            seen: Vec::new(),
            window: std::collections::VecDeque::new(),
            consumed: 0,
            current: -1,
            fetch_size: None,
        }
    }

    fn memory(facade: EntityFacade, entity_name: String, list: EntityList) -> Self {
        let l10n = find_l10n(&facade, &entity_name);
        Self {
            facade,
            entity_name,
            columns: Vec::new(),
            tx: None,
            open: false,
            memory: Some(list.into_values()),
            l10n,
            scrollable: true,
            seen: Vec::new(),
            window: std::collections::VecDeque::new(),
            consumed: 0,
            current: -1,
            fetch_size: None,
        }
    }

    /// Move to the next row and return it; `None` past the last.
    pub fn next_value(&mut self) -> Result<Option<EntityValue>, Error> {
        let index = usize::try_from(self.current + 1).unwrap_or(0);
        let row = self.row_at(index)?;
        self.current = i64::try_from(index).unwrap_or(i64::MAX);
        row.map(|row| self.emit(row)).transpose()
    }

    /// Java's `hasNext`: whether a row follows the current one. A forward-only stream reads it ahead.
    pub fn has_next(&mut self) -> Result<bool, Error> {
        let index = usize::try_from(self.current + 1).unwrap_or(0);
        Ok(self.row_at(index)?.is_some())
    }

    /// Java's `previous`: move to the row before the current one and return it; `None` before the first.
    ///
    /// # Errors
    ///
    /// A forward-only stream cannot move back.
    pub fn previous_value(&mut self) -> Result<Option<EntityValue>, Error> {
        if self.current <= 0 {
            self.current = -1;
            return Ok(None);
        }
        let index = usize::try_from(self.current - 1).unwrap_or(0);
        let row = self.row_at(index)?;
        self.current = i64::try_from(index).unwrap_or(0);
        row.map(|row| self.emit(row)).transpose()
    }

    /// Java's `absolute`, 1-based as JDBC: row `n` from the start, or from the end when negative; 0 is before the first.
    /// True when the cursor is on a row.
    pub fn absolute(&mut self, row: i64) -> Result<bool, Error> {
        if row == 0 {
            self.current = -1;
            return Ok(false);
        }
        let index = if row > 0 {
            row - 1
        } else {
            let total = i64::try_from(self.total()?).unwrap_or(i64::MAX);
            total + row
        };
        if index < 0 {
            self.current = -1;
            return Ok(false);
        }
        let found = self.row_at(usize::try_from(index).unwrap_or(usize::MAX))?.is_some();
        self.current = index;
        Ok(found)
    }

    /// Java's `relative`: move `rows` from the current row. True when the cursor is on a row.
    pub fn relative(&mut self, rows: i64) -> Result<bool, Error> {
        let target = self.current.saturating_add(rows);
        if target < 0 {
            self.current = -1;
            return Ok(false);
        }
        self.absolute(target + 1)
    }

    /// Java's `currentIndex`: the current row, 1-based; 0 before the first.
    pub fn current_index(&self) -> i64 {
        self.current + 1
    }

    /// Java's `setFetchSize`: a hint for how many rows a read brings. Kept; the stream reads by its own batches.
    pub fn set_fetch_size(&mut self, rows: usize) {
        self.fetch_size = Some(rows);
    }

    /// Java's `getPartialList`: up to `limit` rows from row `offset` (1-based; 0 is 1).
    pub fn get_partial_list(&mut self, offset: i64, limit: usize) -> Result<EntityList, Error> {
        let mut out = Vec::new();
        if limit == 0 || !self.absolute(offset.max(1))? {
            return Ok(EntityList::new(out));
        }
        let index = usize::try_from(self.current).unwrap_or(0);
        if let Some(row) = self.row_at(index)? {
            out.push(self.emit(row)?);
        }
        while out.len() < limit {
            match self.next_value()? {
                Some(row) => out.push(row),
                None => break,
            }
        }
        Ok(EntityList::new(out))
    }

    /// Java's `getCompleteList`: every row, from the first when the cursor can go back, else from here on.
    pub fn get_complete_list(&mut self) -> Result<EntityList, Error> {
        if self.memory.is_some() || self.scrollable {
            self.current = -1;
        }
        let mut out = Vec::new();
        while let Some(row) = self.next_value()? {
            out.push(row);
        }
        Ok(EntityList::new(out))
    }

    /// Every row, reading the stream to its end. Only for a cursor that keeps its rows.
    fn total(&mut self) -> Result<usize, Error> {
        if let Some(rows) = &self.memory {
            return Ok(rows.len());
        }
        if !self.scrollable {
            return Err(self.forward_only());
        }
        while self.open {
            if let Some(row) = self.read_raw()? {
                self.seen.push(row);
            }
        }
        Ok(self.seen.len())
    }

    /// The row at `index`, reading the stream as far as it needs.
    fn row_at(&mut self, index: usize) -> Result<Option<EntityValue>, Error> {
        if let Some(rows) = &self.memory {
            return Ok(rows.get(index).cloned());
        }
        if self.scrollable {
            while self.seen.len() <= index && self.open {
                if let Some(row) = self.read_raw()? {
                    self.seen.push(row);
                }
            }
            return Ok(self.seen.get(index).cloned());
        }
        if let Some((_, row)) = self.window.iter().find(|(at, _)| *at == index) {
            return Ok(Some(row.clone()));
        }
        if index < self.consumed {
            return Err(self.forward_only());
        }
        while self.consumed <= index {
            let at = self.consumed;
            match self.read_raw()? {
                Some(row) => {
                    self.window.push_back((at, row));
                    if self.window.len() > 2 {
                        self.window.pop_front();
                    }
                }
                None => return Ok(None),
            }
        }
        Ok(self.window.back().map(|(_, row)| row.clone()))
    }

    fn forward_only(&self) -> Error {
        Error::new(format!(
            "the iterator of {} reads forward only; make the find scrollable to move back",
            self.entity_name
        ))
    }

    /// The next database row, decrypted; `None` at the end, which closes the stream.
    fn read_raw(&mut self) -> Result<Option<EntityValue>, Error> {
        if !self.open {
            return Ok(None);
        }
        let Some(tx) = self.tx.clone() else {
            return Ok(None);
        };
        let row = tx.with_connection(|conn| conn.next_row())?;
        match row {
            Some(row) => {
                self.consumed += 1;
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

    /// A row as the caller gets it, reading its localized fields when they are read.
    fn emit(&self, row: EntityValue) -> Result<EntityValue, Error> {
        Ok(row.with_l10n(self.l10n.as_ref()))
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

    /// Java's `EntityListIterator`: a scrollable cursor moves back and jumps; a forward-only one refuses to move back.
    #[test]
    fn a_scrollable_iterator_moves_back_and_jumps_and_a_forward_one_does_not() {
        let (facade, _tape) = seeded();
        let id = |row: Option<crate::entity::EntityValue>| {
            row.and_then(|row| row.get("testId").and_then(Value::as_str).map(str::to_string))
        };
        let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
        tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
            let mut scroll = facade
                .find("moqui.test.TestEntity")?
                .order_by("testId", false)
                .scrollable(true)
                .iterator()?;
            assert!(scroll.has_next()?);
            assert_eq!(id(scroll.next_value()?).as_deref(), Some("A"));
            assert_eq!(id(scroll.next_value()?).as_deref(), Some("B"));
            assert_eq!(id(scroll.previous_value()?).as_deref(), Some("A"));
            assert!(scroll.absolute(-1)?);
            assert_eq!(scroll.current_index(), 2);
            assert!(!scroll.relative(1)?);
            let page = scroll.get_partial_list(2, 5)?;
            assert_eq!(page.len(), 1);
            assert_eq!(scroll.get_complete_list()?.len(), 2);
            drop(scroll);
            let mut third = facade.make_value("moqui.test.TestEntity")?;
            third.set("testId", Value::string("C"));
            third.create(&facade)?;
            let mut forward = facade
                .find("moqui.test.TestEntity")?
                .order_by("testId", false)
                .iterator()?;
            assert_eq!(id(forward.next_value()?).as_deref(), Some("A"));
            assert!(forward.has_next()?);
            assert_eq!(id(forward.next_value()?).as_deref(), Some("B"));
            assert_eq!(id(forward.next_value()?).as_deref(), Some("C"));
            // The last two rows are kept, so one step back works; the first row is gone.
            assert_eq!(id(forward.previous_value()?).as_deref(), Some("B"));
            assert!(forward.absolute(1).is_err());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn select_field_reads_only_those_columns_and_ignores_a_missing_one() {
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
        // As Java's `selectField`, a name that is not a field is skipped; with none left every field is read.
        let rows = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .select_field("nope")
            .eq("testId", Value::string("A"))
            .list()
            .unwrap();
        assert!(rows.values()[0].get("testMedium").is_some(), "every field is read");
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

    /// Java's `condition(field, value)` keeps one value per field: the second replaces the first, also inside an `AND`.
    #[test]
    fn a_second_equality_on_a_field_replaces_the_first() {
        let (facade, _tape) = seeded();
        let found = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string("B"))
            .eq("testId", Value::string("A"))
            .list()
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found.values()[0].get("testId").and_then(Value::as_str), Some("A"));
        let find = facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testMedium", Value::string("beta"))
            .condition(Condition::and(vec![
                Condition::equals("testMedium", Value::string("alpha")),
                Condition::not_equals("testId", Value::string("B")),
            ]))
            .condition(Condition::not_equals("testId", Value::string("C")));
        assert_eq!(
            find.conditions,
            [
                Condition::equals("testMedium", Value::string("alpha")),
                Condition::not_equals("testId", Value::string("B")),
                Condition::not_equals("testId", Value::string("C")),
            ]
        );
        assert_eq!(find.list().unwrap().len(), 1);
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
