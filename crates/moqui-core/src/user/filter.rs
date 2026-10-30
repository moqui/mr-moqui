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

//! Entity find filters from `ArtifactAuthzFilter` and `EntityFilter`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use moqui_db::model::EntityDefinition;
use moqui_db::sql::Condition;
use moqui_lib::object::{to_plain_string, Value};

use super::authz::{authz_enabled_for, context_entered, generation};
use super::login::entity_facade;
use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityFacade, EntityValue};

/// Add `EntityFilter` terms when a context is entered and entity authz is on.
///
/// `moqui.*` finds are unchanged. No matching filter returns `condition` as given.
/// A missing view alias with `allowMissingAlias` not `Y` fails closed (`1 = 0`).
pub fn apply_entity_filters(entity_name: &str, condition: Option<Condition>) -> Option<Condition> {
    apply_filters(&Target::Named(entity_name), condition)
}

/// [`apply_entity_filters`] for a view that is not in the entity model, such as a dynamic view. The view must be
/// complete, with its aliases, so a filter on a member entity finds the alias of its field.
pub(crate) fn apply_view_filters(view: &EntityDefinition, condition: Option<Condition>) -> Option<Condition> {
    apply_filters(&Target::Defined(view), condition)
}

/// The entity a find reads: one in the model by name, or a definition the caller holds.
enum Target<'a> {
    Named(&'a str),
    Defined(&'a EntityDefinition),
}

impl Target<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Named(name) => name,
            Self::Defined(entity) => &entity.full_name,
        }
    }

    fn with<T>(
        &self,
        facade: &EntityFacade,
        body: impl FnOnce(&EntityDefinition) -> Result<T, moqui_lib::error::Error>,
    ) -> Result<T, Error> {
        match self {
            Self::Named(name) => facade
                .with_entity(name, |entity, _, _| body(entity))
                .map_err(Error::from),
            Self::Defined(entity) => body(entity).map_err(Error::from),
        }
    }
}

fn apply_filters(target: &Target<'_>, condition: Option<Condition>) -> Option<Condition> {
    if !context_entered() || target.name().starts_with("moqui.") {
        return condition;
    }
    if ExecutionContext::with_current(|ctx| ctx.authz_disabled()).unwrap_or(true) {
        return condition;
    }
    if !authz_enabled_for("AT_ENTITY") {
        return condition;
    }
    match with_user_context(|| extra_conditions(target)) {
        Ok(extra) => combine(condition, extra),
        Err(err) => {
            // the entity and the reason in the message, which the console log shows without the fields
            tracing::error!(entity = %target.name(), error = %err, "entity filter failed closed on {}: {err}", target.name());
            combine(condition, vec![false_constraint()])
        }
    }
}

/// Run `body` with the `ec.user.context` values on top of the context, as Java pushes the user context to evaluate
/// apply conditions and filter maps.
fn with_user_context<T>(body: impl FnOnce() -> T) -> T {
    let entries = ExecutionContext::with_current(|ec| ec.user_context().clone()).unwrap_or_default();
    let _frame = crate::script::ContextFrame::push();
    for (name, value) in entries {
        if let Err(err) = crate::script::set_context_name(&name, value) {
            tracing::warn!(error = %err, name = %name, "could not add a user context value for the entity filters");
        }
    }
    body()
}

fn extra_conditions(target: &Target<'_>) -> Result<Vec<Condition>, Error> {
    let facade = entity_facade()?;
    let mut extra = Vec::new();
    for (allow_missing, filter) in applicable_filters()? {
        if let Some(condition) = one_filter(&facade, target, &filter, allow_missing)? {
            extra.push(condition);
        }
    }
    Ok(extra)
}

/// `ec.artifactExecution.getFindFiltersForUser(entityName)`: the `EntityFilter` records that apply to a find on
/// `entity_name` now, each with its `entityFilterSetId`. Empty when authz is off, nothing authorized the current
/// artifact by a record, or the entity is a framework (`moqui.*`) entity.
pub fn find_filters_for_user(entity_name: &str) -> Result<Vec<EntityValue>, Error> {
    if entity_name.starts_with("moqui.") || ExecutionContext::with_current(|ctx| ctx.authz_disabled()).unwrap_or(true) {
        return Ok(Vec::new());
    }
    let facade = entity_facade()?;
    let mut found = Vec::new();
    for (_, filter) in with_user_context(applicable_filters)? {
        let filter_entity = text(&filter, "entityName");
        let applies = facade
            .with_entity(entity_name, |entity, _, _| Ok(applies_to(entity, &filter_entity)))
            .map_err(Error::from)?;
        if applies {
            found.push(filter);
        }
    }
    Ok(found)
}

/// The `EntityFilter` records of every filter set the authorizing record names whose apply conditions pass, each with
/// its set's `allowMissingAlias`.
fn applicable_filters() -> Result<Vec<(bool, EntityValue)>, Error> {
    // The filters come from the record that authorized this find, the frame on top of the stack.
    // A find that was not authorized by a record, or was inherited from no record, has none.
    let Some(authz_id) = ExecutionContext::with_current(|ec| ec.top_authz_info())
        .flatten()
        .and_then(|info| info.aacv)
        .map(|check| check.artifact_authz_id.clone())
    else {
        return Ok(Vec::new());
    };
    let mut applicable = Vec::new();
    for set in filter_sets(&authz_id)?.iter() {
        if !condition_passes(&set.authz_apply_cond)? || !condition_passes(&set.set_apply_cond)? {
            continue;
        }
        for filter in &set.filters {
            applicable.push((set.allow_missing, filter.clone()));
        }
    }
    Ok(applicable)
}

/// A filter set an `ArtifactAuthzFilter` names: the apply conditions, which are evaluated for each find, and the set's
/// `EntityFilter` records.
struct FilterSet {
    authz_apply_cond: String,
    set_apply_cond: String,
    allow_missing: bool,
    filters: Vec<EntityValue>,
}

/// The filter sets of each `artifactAuthzId`, read once for an authorization generation. A write to a filter table
/// moves the generation, as a write to the other authorization tables does.
#[derive(Default)]
struct FilterSetCache {
    by_authz: Mutex<GenerationSets>,
}

#[derive(Default)]
struct GenerationSets {
    generation: u64,
    sets: HashMap<String, Arc<[FilterSet]>>,
}

/// The filter sets of `authz_id`, from the cache of this factory. Sets read while the generation moved are returned
/// but not kept.
fn filter_sets(authz_id: &str) -> Result<Arc<[FilterSet]>, Error> {
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| Error::new("no execution context"))?;
    let cache = factory.extension(|_| FilterSetCache::default());
    let loaded_at = generation();
    {
        let guard = moqui_lib::sync::lock(&cache.by_authz);
        if guard.generation == loaded_at {
            if let Some(found) = guard.sets.get(authz_id) {
                return Ok(Arc::clone(found));
            }
        }
    }
    let sets: Arc<[FilterSet]> = read_filter_sets(authz_id)?.into();
    let mut guard = moqui_lib::sync::lock(&cache.by_authz);
    if generation() == loaded_at {
        if guard.generation != loaded_at {
            *guard = GenerationSets {
                generation: loaded_at,
                sets: HashMap::new(),
            };
        }
        guard.sets.insert(authz_id.to_string(), Arc::clone(&sets));
    }
    Ok(sets)
}

fn read_filter_sets(authz_id: &str) -> Result<Vec<FilterSet>, Error> {
    let facade = entity_facade()?;
    let _guard =
        ExecutionContext::with_current(|ctx| ctx.disable_authz()).ok_or_else(|| Error::new("no execution context"))?;
    let authz_filters = facade
        .find("moqui.security.ArtifactAuthzFilter")?
        .eq("artifactAuthzId", Value::string(authz_id))
        .list()?;
    let mut out = Vec::new();
    for authz_filter in authz_filters.values() {
        let set_id = text(authz_filter, "entityFilterSetId");
        let Some(set) = facade
            .find("moqui.security.EntityFilterSet")?
            .eq("entityFilterSetId", Value::string(&set_id))
            .one()?
        else {
            continue;
        };
        let filters = facade
            .find("moqui.security.EntityFilter")?
            .eq("entityFilterSetId", Value::string(&set_id))
            .list()?;
        out.push(FilterSet {
            authz_apply_cond: text(authz_filter, "applyCond"),
            set_apply_cond: text(&set, "applyCond"),
            allow_missing: text(&set, "allowMissingAlias") == "Y",
            filters: filters.into_values(),
        });
    }
    Ok(out)
}

fn condition_passes(expression: &str) -> Result<bool, Error> {
    if expression.is_empty() {
        return Ok(true);
    }
    crate::script::eval_condition(expression)
}

fn one_filter(
    facade: &EntityFacade,
    target: &Target<'_>,
    filter: &EntityValue,
    allow_missing: bool,
) -> Result<Option<Condition>, Error> {
    let filter_entity = text(filter, "entityName");
    let expression = text(filter, "filterMap");
    if filter_entity.is_empty() || expression.is_empty() {
        return Ok(None);
    }
    // Only a filter on this entity, or on a member of this view, is evaluated, as in Java: the map of a filter on
    // another entity does not run, and an error in it does not close this find.
    let applies = target.with(facade, |entity| Ok(applies_to(entity, &filter_entity)))?;
    if !applies {
        return Ok(None);
    }
    // An error evaluating the map closes the find even with `allowMissingAlias`: as in Java, that setting only
    // excuses a missing alias in the conditions, never a broken filter, which would otherwise stop filtering.
    let map = crate::script::eval_map(&expression)?;
    let join_or = text(filter, "joinOr") == "Y";
    let comparison = text(filter, "comparisonEnumId");
    target.with(facade, |entity| {
        conditions_for(entity, &filter_entity, &map, (join_or, &comparison), allow_missing)
    })
}

/// A filter on `filter_entity` applies to a find on `entity`: the same entity, or a member of the view.
fn applies_to(entity: &EntityDefinition, filter_entity: &str) -> bool {
    entity.full_name == filter_entity
        || (entity.is_view
            && entity
                .member_entities
                .iter()
                .any(|member| member.entity_name == filter_entity))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Relation {
    Direct,
    Member,
    None,
}

fn conditions_for(
    entity: &EntityDefinition,
    filter_entity: &str,
    map: &BTreeMap<String, Value>,
    (join_or, comparison): (bool, &str),
    allow_missing: bool,
) -> Result<Option<Condition>, moqui_lib::error::Error> {
    let relation = if entity.full_name == filter_entity {
        Relation::Direct
    } else if entity.is_view
        && entity
            .member_entities
            .iter()
            .any(|member| member.entity_name == filter_entity)
    {
        Relation::Member
    } else {
        Relation::None
    };
    if relation == Relation::None {
        return Ok(None);
    }
    map_conditions(
        entity,
        filter_entity,
        relation,
        map,
        (join_or, comparison),
        allow_missing,
    )
}

/// The condition of one filter map, as Java's `makeCondition(Map)`: `_comp` is the comparison for this map's fields,
/// `_join` (`or` or `and`) joins them, and `_list` holds more maps, each its own condition in that join.
fn map_conditions(
    entity: &EntityDefinition,
    filter_entity: &str,
    relation: Relation,
    map: &BTreeMap<String, Value>,
    (join_or, comparison): (bool, &str),
    allow_missing: bool,
) -> Result<Option<Condition>, moqui_lib::error::Error> {
    let comparison = match map.get("_comp").map(to_plain_string) {
        Some(operator) => comparison_enum(&operator)?,
        None => comparison.to_string(),
    };
    let join_or = match map.get("_join").map(|join| to_plain_string(join).to_ascii_lowercase()) {
        Some(join) => join == "or",
        None => join_or,
    };
    let mut parts = Vec::new();
    if let Some(Value::List(items)) = map.get("_list") {
        for item in items {
            let Value::Map(entries) = item else {
                continue;
            };
            let inner: BTreeMap<String, Value> = entries
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            if let Some(condition) =
                map_conditions(entity, filter_entity, relation, &inner, (false, ""), allow_missing)?
            {
                parts.push(condition);
            }
        }
    }
    for (field, value) in map {
        if field.starts_with('_') {
            continue;
        }
        let names = match relation {
            Relation::Direct => vec![field.clone()],
            Relation::Member => member_aliases(entity, filter_entity, field),
            Relation::None => Vec::new(),
        };
        if names.is_empty() {
            if allow_missing {
                continue;
            }
            return Err(moqui_lib::error::Error::new(format!(
                "authorized data filter is missing an alias for {field} on {}",
                entity.full_name
            )));
        }
        for name in names {
            parts.push(value_condition(&name, value, &comparison)?);
        }
    }
    Ok(join_conditions(parts, join_or))
}

/// The `ENTCO_` id of a `_comp` operator, by symbol or name as Java's `ComparisonOperator` reads them.
fn comparison_enum(operator: &str) -> Result<String, moqui_lib::error::Error> {
    Ok(match operator.trim().to_ascii_lowercase().as_str() {
        "=" | "==" | "equals" => "ENTCO_EQUALS",
        "!=" | "<>" | "not-equals" | "not_equals" => "ENTCO_NOT_EQUALS",
        "<" | "less" | "less-than" => "ENTCO_LESS",
        ">" | "greater" | "greater-than" => "ENTCO_GREATER",
        "<=" | "less-equals" | "less-than-equal-to" => "ENTCO_LESS_EQ",
        ">=" | "greater-equals" | "greater-than-equal-to" => "ENTCO_GREATER_EQ",
        "in" => "ENTCO_IN",
        "not-in" | "not_in" => "ENTCO_NOT_IN",
        "between" => "ENTCO_BETWEEN",
        "not-between" => "ENTCO_NOT_BETWEEN",
        "like" => "ENTCO_LIKE",
        "not-like" | "not_like" => "ENTCO_NOT_LIKE",
        "is-null" => "ENTCO_IS_NULL",
        "is-not-null" => "ENTCO_IS_NOT_NULL",
        other => {
            return Err(moqui_lib::error::Error::new(format!(
                "unknown filter comparison _comp {other}"
            )));
        }
    }
    .to_string())
}

/// View aliases that read `field` of the member entity `member_name`. `aliases` already holds
/// what each `alias-all` expands to.
fn member_aliases(entity: &EntityDefinition, member_name: &str, field: &str) -> Vec<String> {
    let mut names = Vec::new();
    for alias in &entity.aliases {
        let Some(entity_alias) = alias.entity_alias.as_deref() else {
            continue;
        };
        let Some(member) = entity.member(entity_alias) else {
            continue;
        };
        if member.entity_name != member_name {
            continue;
        }
        let source = alias.field.as_deref().unwrap_or(alias.name.as_str());
        if source == field {
            names.push(alias.name.clone());
        }
    }
    names
}

/// One filter entry. With no comparison a list is `IN` and anything else is equality, as Java
/// does; an enum id such as `ENTCO_LESS` picks the operator.
fn value_condition(field: &str, value: &Value, comparison: &str) -> Result<Condition, moqui_lib::error::Error> {
    let list = |value: &Value| match value {
        Value::List(items) => items.clone(),
        Value::Null => Vec::new(),
        other => vec![other.clone()],
    };
    let condition = match comparison {
        "" if matches!(value, Value::List(_)) => Condition::is_in(field, list(value)),
        "" | "ENTCO_EQUALS" if value.is_null() => Condition::is_null(field),
        "" | "ENTCO_EQUALS" => Condition::equals(field, value.clone()),
        "ENTCO_NOT_EQUALS" if value.is_null() => Condition::is_not_null(field),
        "ENTCO_NOT_EQUALS" => Condition::not_equals(field, value.clone()),
        "ENTCO_LESS" => Condition::less(field, value.clone()),
        "ENTCO_GREATER" => Condition::greater(field, value.clone()),
        "ENTCO_LESS_EQ" => Condition::less_equal(field, value.clone()),
        "ENTCO_GREATER_EQ" => Condition::greater_equal(field, value.clone()),
        "ENTCO_IN" => Condition::is_in(field, list(value)),
        "ENTCO_NOT_IN" => Condition::not_in(field, list(value)),
        "ENTCO_BETWEEN" | "ENTCO_NOT_BETWEEN" => {
            let mut bounds = list(value).into_iter();
            let (Some(low), Some(high)) = (bounds.next(), bounds.next()) else {
                return Err(moqui_lib::error::Error::new(format!(
                    "filter on {field} needs a low and a high value for between"
                )));
            };
            let between = Condition::between(field, low, high);
            if comparison == "ENTCO_NOT_BETWEEN" {
                Condition::negate(between)
            } else {
                between
            }
        }
        "ENTCO_LIKE" => Condition::like(field, value.clone()),
        "ENTCO_NOT_LIKE" => Condition::not_like(field, value.clone()),
        "ENTCO_IS_NULL" => Condition::is_null(field),
        "ENTCO_IS_NOT_NULL" => Condition::is_not_null(field),
        other => {
            return Err(moqui_lib::error::Error::new(format!(
                "unknown filter comparison {other} on {field}"
            )));
        }
    };
    Ok(condition)
}

fn join_conditions(parts: Vec<Condition>, join_or: bool) -> Option<Condition> {
    match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ if join_or => Some(Condition::or(parts)),
        _ => Some(Condition::and(parts)),
    }
}

fn combine(incoming: Option<Condition>, extra: Vec<Condition>) -> Option<Condition> {
    if extra.is_empty() {
        return incoming;
    }
    let filters = single(extra);
    match incoming {
        None => Some(filters),
        Some(condition) => Some(Condition::and(vec![condition, filters])),
    }
}

fn single(mut parts: Vec<Condition>) -> Condition {
    if parts.len() == 1 {
        return parts.remove(0);
    }
    Condition::and(parts)
}

fn false_constraint() -> Condition {
    Condition::is_in("_entityFilter", Vec::new())
}

fn text(row: &EntityValue, field: &str) -> String {
    match row.get(field) {
        Some(Value::String(value)) => value.clone(),
        Some(other) if !other.is_null() => moqui_lib::object::to_plain_string(other),
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
