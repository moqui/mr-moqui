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

use std::sync::{Arc, Mutex};

use moqui_db::sql::{
    count_view, count_with, select_view_with, select_with, select_with_head, Condition, ConditionValues, FieldRef,
    LimitOffset, OrderBy, SelectOptions,
};
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};
use moqui_lib::sync::lock;

use super::cache::{self, FindLookup};
use super::exec::{after_statement, query_tx};
use super::facade::EntityFacade;
use super::field_rw;
use super::params::{entity_columns, map_row, view_columns};
use super::sim_view;
use super::value::{EntityArtifact, EntityList, EntityValue};
use crate::driver::DbRow;
use crate::transaction::TransactionManager;

mod bulk;

pub use bulk::{Purged, BULK_CHUNK};

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
    skip_locked: bool,
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
    /// SQL actually executed by this find; clones retain its diagnostic history.
    query_texts: Arc<Mutex<Vec<String>>>,
    /// Rows a bulk write reads in one query ([`Self::bulk_chunk`]); `None` is [`BULK_CHUNK`].
    bulk_chunk: Option<i64>,
    /// The fields whose equality (or `IS NULL`) came from a map ([`Self::condition_from_map`]), which a find one with
    /// the whole primary key leaves out.
    map_fields: Vec<String>,
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
            skip_locked: false,
            having: Vec::new(),
            select_fields: Vec::new(),
            form_order_only: false,
            require_search_parameters: false,
            has_search_parameters: false,
            disable_authz: false,
            use_cache: None,
            scrollable: false,
            use_clone: false,
            query_texts: Arc::new(Mutex::new(Vec::new())),
            bulk_chunk: None,
            map_fields: Vec::new(),
        }
    }

    pub fn query_texts(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.query_texts)
    }

    fn record_query(&self, sql: &str) {
        lock(&self.query_texts).push(sql.to_string());
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

    fn authz_guard(&self) -> Option<crate::hooks::Guard> {
        self.disable_authz
            .then(|| (crate::hooks::get().disable_authz)())
            .flatten()
    }

    /// The conditions added so far, for the tests in moqui-core.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    pub fn conditions(&self) -> &[Condition] {
        &self.conditions
    }

    pub fn entity_name(&self) -> &str {
        &self.entity_name
    }

    /// Java's `condition(field, value)`: a second value for the same field replaces the first.
    pub fn eq(mut self, field: &str, value: Value) -> Self {
        self.map_fields.retain(|name| name != field);
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
        if !self.map_fields.is_empty() {
            let named: Vec<&str> = match &condition {
                Condition::And(list) => list.iter().filter_map(plain_equality_field).collect(),
                other => plain_equality_field(other).into_iter().collect(),
            };
            self.map_fields.retain(|name| !named.contains(&name.as_str()));
        }
        add_condition(&mut self.conditions, condition);
        self
    }

    /// [`Self::condition`] for one entry of a map such as the context (`condition(context)`, an `entity-find-one` with
    /// `auto-field-map`), which carries whatever the context holds: a find one with the whole primary key leaves such
    /// an equality out, as Java's `oneInternal` leaves out every field of its simple map. An equality set on its own
    /// ([`Self::eq`], [`Self::condition`]) stays and filters the row (plans/High-Level-Review.md, lane C note on
    /// `one()`).
    pub fn condition_from_map(mut self, condition: Condition) -> Self {
        if let Some(name) = plain_equality_field(&condition) {
            if !self.map_fields.iter().any(|known| known == name) {
                self.map_fields.push(name.to_string());
            }
        }
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

    /// With [`Self::for_update`]: leave out the rows another transaction holds instead of waiting for them, where the
    /// database has `SKIP LOCKED` (Postgres, MySQL 8, Oracle). Elsewhere the lock waits. For a worker claiming rows.
    pub fn skip_locked(mut self, skip: bool) -> Self {
        self.skip_locked = skip;
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

    /// What [`Self::bypasses_cache`] looks at, for a find in a transaction-cache frame ([`cache::before_selective_find`]).
    fn selective(&self) -> cache::Selective {
        cache::Selective {
            lock: self.for_update.then_some(self.skip_locked),
            fields: self.select_fields.clone(),
            aggregate: self.distinct || !self.having.is_empty(),
        }
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
            (crate::hooks::get().note_no_search_params)();
            return Ok(EntityList::new(Vec::new()));
        }
        // The view frame is what authorizes the read, and the filters come from its record.
        let _authz = self.authz_guard();
        let _artifact = EntityArtifact::push(&self.facade, &self.entity_name, "view", operation)?;
        cache::sim_check_find(&self.facade, &self.entity_name)?;
        let condition = self.combined();
        let lookup = self.lookup(condition.as_ref());
        if let Some(schema) = self.sim_view_schema(false)? {
            let plan = self.plan(false, condition)?;
            let rows = cache::query_sim(&self.facade, &schema, &plan.sql, &plan.params)?;
            self.record_query(&plan.sql);
            let values = self.rows_to_values(&plan, rows)?;
            return Ok(self.attach_l10n(EntityList::new(values)));
        }
        let bypass = self.bypasses_cache();
        let cached = if bypass {
            self.require_transaction_for_lock()?;
            cache::before_selective_find(&self.facade, &lookup, &self.selective())?
        } else {
            cache::before_find(&self.facade, &lookup)?
        };
        if let Some(cached) = cached {
            return Ok(self.attach_l10n(cached));
        }
        // As Java: a find on a table that does not exist finds nothing.
        if !self.facade.table_ready(&self.entity_name) {
            return Ok(EntityList::new(Vec::new()));
        }
        self.require_transaction_for_lock()?;
        let fence = self.store_fence(bypass, &lookup);
        let plan = self.plan(false, condition)?;
        let rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
        self.record_query(&plan.sql);
        after_statement(&self.facade, &plan.tx, false);
        cache::note_db_read(&plan.tx);
        let values = self.rows_to_values(&plan, rows)?;
        let list = EntityList::new(values);
        if bypass {
            cache::after_selective_find(&self.facade, &lookup, &self.selective(), &list);
        }
        if let Some(fence) = fence {
            // The rows were read; a cache that cannot hold them only costs the next read.
            if let Err(err) = cache::after_find(&self.facade, &lookup, &fence, &list) {
                tracing::warn!(error = %err, "Could not cache the find of {}", self.entity_name);
            }
        }
        Ok(self.attach_l10n(list))
    }

    /// The store fence of this find, taken before its query (plans/High-Level-Review.md HL-023); `None` when its result
    /// is not stored in the shared cache: it bypasses the cache, reads a replica ([`Self::use_clone`]), or is not cached.
    fn store_fence(&self, bypass: bool, lookup: &FindLookup) -> Option<cache::StoreFence> {
        if bypass || (self.use_clone && self.facade.reads_clone(&self.entity_name)) {
            return None;
        }
        cache::store_fence(&self.facade, lookup)
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
        Ok(list.into_values().into_iter().next())
    }

    /// A primary key field compared to null or an empty value, as Java's `hasEmptyPk`.
    fn has_empty_pk(&self) -> bool {
        self.facade
            .with_entity(&self.entity_name, |entity, _, _| {
                let is_pk = |name: &str| pk_names(entity).any(|pk| pk == name);
                Ok(self.conditions.iter().any(|condition| match condition {
                    Condition::Equals(field, value) => {
                        field.entity_alias.is_none() && is_pk(&field.name) && moqui_lib::object::is_empty(value)
                    }
                    Condition::IsNull(field) => field.entity_alias.is_none() && is_pk(&field.name),
                    _ => false,
                }))
            })
            .unwrap_or(false)
    }

    /// Java `oneInternal`: a find one whose field equalities hold the full primary key, with more besides (a
    /// `condition(context)` puts every field of the context in, nulls as `IS NULL`), finds by the primary key and
    /// leaves out the equalities that came from a map ([`Self::condition_from_map`]). Unlike Java, an equality set on
    /// its own stays: a caller that names `reportPending = Y` or an owner beside the key means it. Other kinds of
    /// condition stay.
    fn narrow_to_full_pk(&mut self) {
        let equals_on = |condition: &Condition, name: &str| match condition {
            Condition::Equals(field, value) => {
                field.entity_alias.is_none() && field.name == name && !moqui_lib::object::is_empty(value)
            }
            _ => false,
        };
        // A null in the map is an `IS NULL` here; Java keeps it in the same simple map as the equalities.
        let equalities = self
            .conditions
            .iter()
            .filter(|condition| matches!(condition, Condition::Equals(..) | Condition::IsNull(..)))
            .count();
        // Most finds have no more equalities than key fields, and leave here without copying the key names.
        let narrows = self.facade.with_entity(&self.entity_name, |entity, _, _| {
            let count = pk_names(entity).count();
            let has_full_pk = count > 0
                && pk_names(entity).all(|name| self.conditions.iter().any(|condition| equals_on(condition, name)));
            Ok(has_full_pk && equalities > count)
        });
        if !narrows.unwrap_or(false) {
            return;
        }
        let pk_names: Vec<String> = match self.facade.with_entity(&self.entity_name, |entity, _, _| {
            Ok(pk_names(entity).map(str::to_string).collect())
        }) {
            Ok(names) => names,
            Err(_) => return,
        };
        let map_fields = std::mem::take(&mut self.map_fields);
        let from_map = |name: &str| map_fields.iter().any(|known| known == name);
        let mut kept: Vec<Condition> = Vec::new();
        for condition in std::mem::take(&mut self.conditions) {
            if let Condition::IsNull(field) = &condition {
                if field.entity_alias.is_none() && from_map(&field.name) {
                    continue;
                }
            }
            let Condition::Equals(field, _) = &condition else {
                kept.push(condition);
                continue;
            };
            let is_pk = pk_names.contains(&field.name);
            let first_for_pk = is_pk
                && !kept
                    .iter()
                    .any(|known| matches!(known, Condition::Equals(other, _) if other.name == field.name));
            if (first_for_pk && equals_on(&condition, &field.name)) || (!is_pk && !from_map(&field.name)) {
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
            let plan = self.plan(true, self.combined())?;
            let rows = cache::query_sim(&self.facade, &schema, &plan.sql, &plan.params)?;
            self.record_query(&plan.sql);
            return count_of(&rows);
        }
        let condition = self.combined();
        let lookup = self.lookup(condition.as_ref());
        let bypass = self.bypasses_cache();
        // A distinct or having count of rows the frame holds goes to the database once they are written there; any other
        // count, a for-update or select-field one too, counts the rows the overlay gives (plans/TX-Cache.md G1).
        let held_count = if self.distinct || !self.having.is_empty() {
            cache::before_selective_count(&self.facade, &self.entity_name)?;
            None
        } else {
            cache::before_count(&self.facade, &lookup)?
        };
        if let Some(count) = held_count {
            return Ok(count);
        }
        if !self.facade.table_ready(&self.entity_name) {
            return Ok(0);
        }
        let fence = self.store_fence(bypass, &lookup);
        let plan = self.plan(true, condition)?;
        let rows = query_tx(&plan.tx, &plan.sql, &plan.params)?;
        self.record_query(&plan.sql);
        after_statement(&self.facade, &plan.tx, false);
        cache::note_db_read(&plan.tx);
        let count = count_of(&rows)?;
        if let Some(fence) = fence {
            if let Err(err) = cache::after_count(&self.facade, &lookup, &fence, count) {
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
    /// not asked to mix in rows that are not in the database yet. The cursor lives on the active
    /// transaction's enlisted connection: with no transaction in place the iterator begins one and
    /// commits it when it closes (plans/High-Level-Review.md HL-003). Drop closes it. The iterator
    /// reads only in the transaction it opened in; once that one has ended, or while another is in
    /// place, it fails rather than read or close another transaction's cursor.
    ///
    /// Bounded memory depends on the driver ([`crate::driver::DbConnection::open_stream`]): Postgres
    /// fetches through a cursor and MySQL reads an active result, but the browser's SQLite reads the
    /// whole result when the stream opens, so there an early stop still pays for every row.
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
        let lookup = self.lookup(condition.as_ref());
        let cached = if self.bypasses_cache() {
            self.require_transaction_for_lock()?;
            cache::before_selective_find(&self.facade, &lookup, &self.selective())?
        } else {
            cache::before_find(&self.facade, &lookup)?
        };
        if let Some(cached) = cached {
            return Ok(EntityListIterator::memory(self.facade, self.entity_name, cached));
        }
        if !self.facade.table_ready(&self.entity_name) {
            return Ok(EntityListIterator::memory(
                self.facade,
                self.entity_name,
                EntityList::new(Vec::new()),
            ));
        }
        self.require_transaction_for_lock()?;
        let plan = self.plan(false, condition)?;
        // A stream opened on a connection borrowed for one call would go back to the pool open, and the next read
        // would borrow another: begin a transaction that holds the connection until the iterator closes.
        let scope = if plan.tx.in_transaction() {
            None
        } else {
            Some(plan.tx.scope(None)?)
        };
        plan.tx
            .with_connection(|conn| conn.open_stream(&plan.sql, &plan.params))?;
        self.record_query(&plan.sql);
        after_statement(&self.facade, &plan.tx, false);
        cache::note_db_read(&plan.tx);
        let token = plan.tx.frame_token();
        Ok(EntityListIterator::stream(
            self.facade,
            self.entity_name,
            plan.columns,
            StreamTx {
                tx: plan.tx,
                token,
                scope,
            },
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

    fn lookup(&self, condition: Option<&Condition>) -> FindLookup {
        FindLookup {
            filtered: !self.is_unfiltered(condition),
            entity_name: self.entity_name.clone(),
            use_cache: self.use_cache,
            condition: condition.cloned(),
            order: self.order.clone(),
            limit: self.limit,
            offset: self.offset,
        }
    }

    /// Whether `condition` is [`Self::unfiltered`], compared without making that.
    fn is_unfiltered(&self, condition: Option<&Condition>) -> bool {
        match (self.conditions.as_slice(), condition) {
            ([], None) => true,
            ([only], Some(condition)) => only == condition,
            (all, Some(Condition::And(list))) if all.len() > 1 => all == list.as_slice(),
            _ => false,
        }
    }

    /// The find's own conditions, before the user's authorization filters.
    fn unfiltered(&self) -> Option<Condition> {
        match self.conditions.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            all => Some(Condition::and(all.to_vec())),
        }
    }

    fn combined(&self) -> Option<Condition> {
        let condition = self.unfiltered();
        if !authorization_filters() {
            return condition;
        }
        (crate::hooks::get().entity_filters)(&self.entity_name, condition)
    }

    /// `condition` is [`Self::combined`], which the caller has made already.
    fn plan(&self, count_only: bool, condition: Option<Condition>) -> Result<ReadPlan, Error> {
        let page = LimitOffset {
            limit: self.limit,
            offset: self.offset,
        };
        let having = match self.having.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            all => Some(Condition::and(all.to_vec())),
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
                    skip_locked: self.skip_locked,
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
                } else if select_fields.is_empty() && !options.distinct {
                    // Every field: the columns and the select list are the entity's, rendered once.
                    let shape = super::sql_cache::read_shape(&self.facade, entity, dialect);
                    let statement = match lone_pk_equals(entity, condition.as_ref(), &order, page, &options) {
                        Some(pk) => moqui_db::sql::Statement {
                            sql: shape
                                .select_pk(|| {
                                    let head = &shape.head;
                                    Ok(select_with_head(
                                        head,
                                        entity,
                                        dialect,
                                        condition.as_ref(),
                                        &order,
                                        page,
                                        &options,
                                    )?
                                    .sql)
                                })?
                                .to_string(),
                            params: vec![pk.clone()],
                        },
                        None => {
                            select_with_head(&shape.head, entity, dialect, condition.as_ref(), &order, page, &options)?
                        }
                    };
                    return Ok(ReadPlan {
                        sql: statement.sql,
                        params: statement.params,
                        columns: Arc::clone(&shape.columns),
                        tx: tx.clone(),
                    });
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
                    columns: columns.into(),
                    tx: tx.clone(),
                })
            })
    }

    fn rows_to_values(&self, plan: &ReadPlan, rows: Vec<DbRow>) -> Result<Vec<EntityValue>, Error> {
        let facade = self.facade.clone();
        self.facade.with_entity(&self.entity_name, |entity, _, _| {
            let mut values = Vec::with_capacity(rows.len());
            let name: Arc<str> = Arc::from(entity.full_name.as_str());
            for row in rows {
                let mapped = map_row(&plan.columns, row);
                let adjusted = field_rw::decrypt_stored(&facade, entity, mapped)?;
                values.push(EntityValue::from_fields(Arc::clone(&name), adjusted));
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

/// The field of an equality or `IS NULL` on a plain field (no member alias or expression).
fn plain_equality_field(condition: &Condition) -> Option<&str> {
    match condition {
        Condition::Equals(field, _) | Condition::IsNull(field)
            if field.entity_alias.is_none() && field.expr.is_none() =>
        {
            Some(field.name.as_str())
        }
        _ => None,
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
/// the script runtime, and conversion of definition text, a timestamp in the session time zone as Java's
/// `convertFieldString` reads an `econdition` value.
pub(crate) struct FindValues;

impl ConditionValues for FindValues {
    /// Java's `ec.user.nowTimestamp`, so a date filter follows the effective time.
    fn now(&self) -> Value {
        basic_convert(&Value::I64(crate::hooks::now_effective_millis()), "java.sql.Timestamp").value
    }

    fn expand(&self, text: &str) -> Result<String, Error> {
        crate::hooks::expand(text).map_err(|err| Error::new(err.message()))
    }

    fn convert(&self, text: &str, java_type: &str) -> Result<Value, Error> {
        if text.is_empty() || text == "null" {
            return Ok(Value::Null);
        }
        Ok(crate::hooks::basic_convert(&Value::string(text), java_type).value)
    }
}

/// The localized fields of `entity_name`, built once per entity and then shared. A raw read sees the stored values
/// and does not fill the cache.
fn find_l10n(facade: &EntityFacade, entity_name: &str) -> Option<Arc<field_rw::L10nTargets>> {
    if field_rw::raw_read() {
        return None;
    }
    // Read before the definition, so a view built again meanwhile is not kept under the older count.
    let generation = super::derived::generation(facade);
    let known = facade.with_slot_read(L10nCache::default, |cache| cache.get(entity_name, generation));
    if let Some(known) = known {
        return known;
    }
    let built = facade
        .with_entity(entity_name, |entity, _, _| Ok(field_rw::l10n_targets(facade, entity)))
        .ok()
        .flatten();
    facade.with_slot(L10nCache::default, |cache| {
        cache.insert(entity_name, generation, built.clone())
    });
    built
}

/// The localized fields of each entity a find read. `None` means the entity was checked and has none.
type L10nCache = super::derived::ByDefinition<Option<Arc<field_rw::L10nTargets>>>;

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

/// The primary key field names, or for a view the aliases of member primary keys, as `pk_field_names` without the
/// list.
fn pk_names(entity: &moqui_db::model::EntityDefinition) -> impl Iterator<Item = &str> + '_ {
    let view = entity.is_view;
    let aliases = entity
        .aliases
        .iter()
        .filter(move |alias| view && alias.is_pk)
        .map(|alias| alias.name.as_str());
    let fields = entity
        .fields
        .iter()
        .filter(move |field| !view && field.is_pk)
        .map(|field| field.name.as_str());
    aliases.chain(fields)
}

/// A find of every column by the single primary key, with no order or page. Its SQL does not change with the key, which
/// is bound as given; text for a key that is not text is left to the full render, which converts it.
fn lone_pk_equals<'a>(
    entity: &moqui_db::model::EntityDefinition,
    condition: Option<&'a Condition>,
    order: &[OrderBy],
    page: LimitOffset,
    options: &SelectOptions,
) -> Option<&'a Value> {
    if !order.is_empty() || page.is_set() || options.distinct || options.for_update || options.having.is_some() {
        return None;
    }
    let mut keys = entity.fields.iter().filter(|field| field.is_pk);
    let (Some(key), None) = (keys.next(), keys.next()) else {
        return None;
    };
    match condition {
        Some(Condition::Equals(field, value))
            if field.entity_alias.is_none()
                && field.expr.is_none()
                && field.name == key.name
                && !value.is_null()
                && (!matches!(value, Value::String(_)) || key.is_text()) =>
        {
            Some(value)
        }
        _ => None,
    }
}

fn authorization_filters() -> bool {
    crate::hooks::entered() && !(crate::hooks::get().authz_disabled)()
}

struct ReadPlan {
    sql: String,
    params: Vec<Value>,
    /// Field name and Java type of each column, shared by the finds of an entity that read every field.
    columns: Arc<[(Arc<str>, String)]>,
    tx: TransactionManager,
}

/// Java's `EntityListIterator`: a cursor over a find's rows, read from the database as it moves. `next_value` moves
/// forward. With [`EntityFind::scrollable`] (Java's scroll-insensitive result set) it keeps the rows read, so
/// `previous_value`, `absolute`, `relative` and `get_partial_list` can go anywhere; otherwise it only moves forward and
/// holds no more than two rows, so a large export streams. Rows from the cache or the overlay are in memory and scroll.
/// The transaction a stream reads in: the frame it opened in, and the scope it began when none was in place.
struct StreamTx {
    tx: TransactionManager,
    token: Option<u64>,
    scope: Option<crate::transaction::TxScope>,
}

pub struct EntityListIterator {
    facade: EntityFacade,
    entity_name: String,
    columns: Arc<[(Arc<str>, String)]>,
    tx: Option<StreamTx>,
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
        columns: Arc<[(Arc<str>, String)]>,
        tx: StreamTx,
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
            columns: Arc::new([]),
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
        let Some(stream) = self.tx.as_ref() else {
            return Ok(None);
        };
        let tx = stream.tx.clone();
        if tx.frame_token() != stream.token {
            self.close()?;
            return Err(Error::new(format!(
                "the iterator of {} was opened in a transaction that has ended or is not the one in place",
                self.entity_name
            )));
        }
        let row = tx.with_connection(|conn| conn.next_row())?;
        match row {
            Some(row) => {
                self.consumed += 1;
                let mapped = map_row(&self.columns, row);
                let facade = self.facade.clone();
                let hook = facade.clone();
                let adjusted = facade.with_entity(&self.entity_name, |entity, _, _| {
                    field_rw::after_read(&hook, entity, mapped)
                })?;
                Ok(Some(EntityValue::from_fields(
                    Arc::from(self.entity_name.as_str()),
                    adjusted,
                )))
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

    /// Close the stream, and commit the transaction the iterator began. A stream whose transaction has ended or is not
    /// the one in place is not closed here: its cursor went with its connection, and the one in place belongs to another
    /// iterator.
    pub fn close(&mut self) -> Result<(), Error> {
        if !self.open {
            return Ok(());
        }
        self.open = false;
        let Some(stream) = self.tx.as_mut() else {
            return Ok(());
        };
        let closed = if stream.tx.frame_token() == stream.token {
            stream.tx.with_connection(|conn| conn.close_stream())
        } else {
            Ok(())
        };
        match stream.scope.take() {
            Some(scope) if closed.is_ok() => scope.commit(),
            // Dropped, the scope rolls back.
            _ => closed,
        }
    }
}

impl Drop for EntityListIterator {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // The scope's own drop rolls back the transaction the iterator began.
            self.open = false;
            return;
        }
        if let Err(err) = self.close() {
            tracing::warn!(error = %err, entity = %self.entity_name, "could not close an entity iterator");
        }
    }
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
