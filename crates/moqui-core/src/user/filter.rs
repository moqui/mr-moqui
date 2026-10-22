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
use std::sync::{Arc, Mutex, PoisonError};

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
    sets: HashMap<String, Arc<Vec<FilterSet>>>,
}

/// The filter sets of `authz_id`, from the cache of this factory. Sets read while the generation moved are returned
/// but not kept.
fn filter_sets(authz_id: &str) -> Result<Arc<Vec<FilterSet>>, Error> {
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| Error::new("no execution context"))?;
    let cache = factory.extension(|_| FilterSetCache::default());
    let loaded_at = generation();
    {
        let guard = cache.by_authz.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.generation == loaded_at {
            if let Some(found) = guard.sets.get(authz_id) {
                return Ok(Arc::clone(found));
            }
        }
    }
    let sets = Arc::new(read_filter_sets(authz_id)?);
    let mut guard = cache.by_authz.lock().unwrap_or_else(PoisonError::into_inner);
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
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use moqui_db::sql::Condition;
    use moqui_lib::object::Value;

    use super::super::session;
    use crate::context::ExecutionContextFactory;
    use crate::entity::EntityFacade;
    use crate::script::{clear_context_stack, set_context_name};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    const VIEW: &str = "mantle.product.ProductOrg";
    const PRODUCT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<entities>
    <entity entity-name="Product" package="mantle.product">
        <field name="productId" type="id" is-pk="true"/>
        <field name="ownerPartyId" type="id"/>
    </entity>
    <view-entity entity-name="ProductOrg" package="mantle.product">
        <member-entity entity-alias="PROD" entity-name="mantle.product.Product"/>
        <alias name="productId" entity-alias="PROD" field="productId"/>
        <alias name="ownerPartyId" entity-alias="PROD" field="ownerPartyId"/>
    </view-entity>
</entities>
"#;

    #[test]
    fn apply_entity_filters_returns_the_same_condition() {
        let condition = Condition::equals("testId", Value::string("T1"));
        let filtered = super::apply_entity_filters("moqui.test.TestEntity", Some(condition.clone()));
        assert_eq!(filtered, Some(condition));
        assert!(super::apply_entity_filters("moqui.test.TestEntity", None).is_none());
    }

    #[test]
    fn a_filter_map_joins_a_list_of_maps_with_their_own_comparisons() {
        let (facade, _model_dir) = product_facade();
        let map = |pairs: Vec<(&str, Value)>| -> std::collections::BTreeMap<String, Value> {
            pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
        };
        let as_map = |pairs: Vec<(&str, Value)>| {
            Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
        };
        // [_join:'or', _list:[[ownerPartyId:'ORG1'], [productId:'P%', _comp:'like']]]
        let filter = map(vec![
            ("_join", Value::string("or")),
            (
                "_list",
                Value::List(vec![
                    as_map(vec![("ownerPartyId", Value::string("ORG1"))]),
                    as_map(vec![
                        ("productId", Value::string("P%")),
                        ("_comp", Value::string("like")),
                    ]),
                ]),
            ),
        ]);
        let condition = facade
            .with_entity("mantle.product.Product", |entity, _, _| {
                super::map_conditions(
                    entity,
                    "mantle.product.Product",
                    super::Relation::Direct,
                    &filter,
                    (false, ""),
                    false,
                )
            })
            .unwrap()
            .expect("a condition");
        let text = format!("{condition:?}");
        assert!(text.starts_with("Or"), "{text}");
        assert!(text.contains("ORG1") && text.contains("Like"), "{text}");
        assert!(!text.contains("_join") && !text.contains("_list"), "{text}");
        assert_eq!(super::comparison_enum("!=").unwrap(), "ENTCO_NOT_EQUALS");
        assert!(super::comparison_enum("sounds-like").is_err());
    }

    #[test]
    fn view_find_applies_user_org_and_skips_active_org_until_set() {
        session::reset_session();
        clear_context_stack();
        let runtime = temp_dir("filter-runtime");
        let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
        let factory = {
            let _lock = crate::user::session::cookie_property_lock();
            ExecutionContextFactory::boot_with_framework(runtime.path(), &framework).expect("boot")
        };
        let (facade, _model_dir) = product_facade();
        factory.set_entity(facade.clone());
        let context = factory.new_context();
        let _entered = context.enter();
        session::set_user("org-user", "org-user");
        {
            let _guard = context.disable_authz();
            seed(&facade);
        }
        set_context_name(
            "filterOrgIds",
            Value::List(vec![Value::string("ORG1"), Value::string("ORG2")]),
        )
        .unwrap();

        // The filters come from the record that allowed this frame.
        context
            .push_checked("AT_ENTITY", VIEW, "view", true, false, None)
            .expect("the ALL_USERS allow covers the view");
        let incoming = Condition::equals("productId", Value::string("P1"));
        let filtered = super::apply_entity_filters(VIEW, Some(incoming)).expect("user org filter");
        assert!(has_equals(&filtered, "productId", "P1"), "{filtered:?}");
        assert!(has_in(&filtered, "ownerPartyId", &["ORG1", "ORG2"]), "{filtered:?}");
        assert!(!has_equals(&filtered, "ownerPartyId", "ORG9"), "{filtered:?}");

        set_context_name("activeOrgId", Value::string("ORG9")).unwrap();
        let both = super::apply_entity_filters(VIEW, None).expect("both filters");
        assert!(has_in(&both, "ownerPartyId", &["ORG1", "ORG2"]), "{both:?}");
        assert!(has_equals(&both, "ownerPartyId", "ORG9"), "{both:?}");

        // A view not in the model, as a dynamic view is, gets the same filters on its members.
        let mut dynamic = facade.with_entity(VIEW, |entity, _, _| Ok(entity.clone())).unwrap();
        dynamic.full_name = "dynamic.ProductFilterView".to_string();
        let on_dynamic = super::apply_view_filters(&dynamic, None).expect("filters on the dynamic view");
        assert!(has_in(&on_dynamic, "ownerPartyId", &["ORG1", "ORG2"]), "{on_dynamic:?}");
        assert!(has_equals(&on_dynamic, "ownerPartyId", "ORG9"), "{on_dynamic:?}");

        // `getFindFiltersForUser` lists the filters behind those terms, by set
        let sets: Vec<String> = super::find_filters_for_user(VIEW)
            .unwrap()
            .iter()
            .map(|filter| super::text(filter, "entityFilterSetId"))
            .collect();
        assert!(sets.contains(&"MANTLE_USER_ORG".to_string()), "{sets:?}");
        assert!(sets.contains(&"MANTLE_ACTIVE_ORG".to_string()), "{sets:?}");
        assert!(super::find_filters_for_user("moqui.security.UserAccount")
            .unwrap()
            .is_empty());

        // the orgs may come from ec.user.context, as setup#UserOrganizationInfo puts them, instead of the context
        clear_context_stack();
        assert!(crate::script::eval_condition(
            "(ec.user.context.filterOrgIds = ['ORG7']) && ec.user.context.filterOrgIds[0] === 'ORG7' \
                && Object.keys(ec.user.context).includes('filterOrgIds') && ec.user.context.missing === undefined"
        )
        .unwrap());
        let from_user = super::apply_entity_filters(VIEW, None).expect("user org filter from the user context");
        assert!(has_in(&from_user, "ownerPartyId", &["ORG7"]), "{from_user:?}");
        context.user_context().clear();

        context.pop().unwrap();

        // The same filters on a real find through the view: ORG9 is out until the active org is set.
        {
            let _guard = context.disable_authz();
            for (id, owner) in [("P1", "ORG1"), ("P2", "ORG9"), ("P3", "ORG2")] {
                create(
                    &facade,
                    "mantle.product.Product",
                    &[("productId", id), ("ownerPartyId", owner)],
                );
            }
        }
        clear_context_stack();
        set_context_name(
            "filterOrgIds",
            Value::List(vec![Value::string("ORG1"), Value::string("ORG2")]),
        )
        .unwrap();
        let ids = |facade: &EntityFacade| -> Vec<String> {
            let mut ids: Vec<String> = facade
                .find(VIEW)
                .unwrap()
                .list()
                .unwrap()
                .values()
                .iter()
                .filter_map(|row| row.get("productId").and_then(Value::as_str).map(str::to_string))
                .collect();
            ids.sort();
            ids
        };
        // applyCond on the active org set skips it while activeOrgId is unset.
        assert_eq!(ids(&facade), ["P1", "P3"]);
        set_context_name("activeOrgId", Value::string("ORG1")).unwrap();
        assert_eq!(ids(&facade), ["P1"]);

        // The filter sets are kept for an authorization generation, and a write to a filter table moves it.
        {
            let _guard = context.disable_authz();
            facade
                .find("moqui.security.ArtifactAuthzFilter")
                .unwrap()
                .eq("artifactAuthzId", Value::string("MANTLE_ORG_AUTHZ"))
                .eq("entityFilterSetId", Value::string("MANTLE_ACTIVE_ORG"))
                .one()
                .unwrap()
                .expect("the active org link")
                .delete(&facade)
                .unwrap();
        }
        assert_eq!(ids(&facade), ["P1", "P3"]);

        clear_context_stack();
        session::reset_session();
    }

    fn seed(facade: &EntityFacade) {
        create(
            facade,
            "moqui.security.ArtifactGroupMember",
            &[
                ("artifactGroupId", "MANTLE_ORG"),
                ("artifactName", VIEW),
                ("artifactTypeEnumId", "AT_ENTITY"),
                ("nameIsPattern", "N"),
            ],
        );
        create(
            facade,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "MANTLE_ORG_AUTHZ"),
                ("userGroupId", "ALL_USERS"),
                ("artifactGroupId", "MANTLE_ORG"),
                ("authzTypeEnumId", "AUTHZT_ALLOW"),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
        create(
            facade,
            "moqui.security.EntityFilterSet",
            &[
                ("entityFilterSetId", "MANTLE_USER_ORG"),
                ("description", "User Organization Filters"),
            ],
        );
        create(
            facade,
            "moqui.security.EntityFilter",
            &[
                ("entityFilterId", "MANTLE_USER_ORG_A"),
                ("entityFilterSetId", "MANTLE_USER_ORG"),
                ("entityName", "mantle.product.Product"),
                ("filterMap", "({ ownerPartyId: filterOrgIds })"),
            ],
        );
        // a filter on another entity whose map does not evaluate; finds on the product view do not run it
        create(
            facade,
            "moqui.security.EntityFilter",
            &[
                ("entityFilterId", "MANTLE_USER_ORG_BROKEN"),
                ("entityFilterSetId", "MANTLE_USER_ORG"),
                ("entityName", "mantle.party.Party"),
                ("filterMap", "[ownerPartyId:(['_NA_']+"),
            ],
        );
        create(
            facade,
            "moqui.security.EntityFilterSet",
            &[
                ("entityFilterSetId", "MANTLE_ACTIVE_ORG"),
                ("description", "Active Organization Filters"),
                ("applyCond", "activeOrgId"),
                ("allowMissingAlias", "Y"),
            ],
        );
        create(
            facade,
            "moqui.security.EntityFilter",
            &[
                ("entityFilterId", "MANTLE_ACTIVE_ORG_A"),
                ("entityFilterSetId", "MANTLE_ACTIVE_ORG"),
                ("entityName", "mantle.product.Product"),
                ("filterMap", "({ ownerPartyId: activeOrgId })"),
            ],
        );
        create(
            facade,
            "moqui.security.ArtifactAuthzFilter",
            &[
                ("artifactAuthzId", "MANTLE_ORG_AUTHZ"),
                ("entityFilterSetId", "MANTLE_USER_ORG"),
            ],
        );
        create(
            facade,
            "moqui.security.ArtifactAuthzFilter",
            &[
                ("artifactAuthzId", "MANTLE_ORG_AUTHZ"),
                ("entityFilterSetId", "MANTLE_ACTIVE_ORG"),
            ],
        );
    }

    fn create(facade: &EntityFacade, name: &str, fields: &[(&str, &str)]) {
        let mut value = facade.make_value(name).unwrap();
        for (field, text) in fields {
            value.set(*field, Value::string(*text));
        }
        value.create(facade).unwrap();
    }

    fn has_equals(condition: &Condition, field: &str, expected: &str) -> bool {
        match condition {
            Condition::Equals(field_ref, value) => field_ref.name == field && value.as_str() == Some(expected),
            Condition::And(parts) | Condition::Or(parts) => parts.iter().any(|part| has_equals(part, field, expected)),
            Condition::Not(inner) => has_equals(inner, field, expected),
            _ => false,
        }
    }

    fn has_in(condition: &Condition, field: &str, expected: &[&str]) -> bool {
        match condition {
            Condition::In(field_ref, values) => {
                field_ref.name == field
                    && values.len() == expected.len()
                    && values
                        .iter()
                        .zip(expected)
                        .all(|(value, want)| value.as_str() == Some(*want))
            }
            Condition::And(parts) | Condition::Or(parts) => parts.iter().any(|part| has_in(part, field, expected)),
            Condition::Not(inner) => has_in(inner, field, expected),
            _ => false,
        }
    }

    fn product_facade() -> (EntityFacade, TempDir) {
        let root = temp_dir("product-fw");
        let entity_dir = root.path().join("entity");
        let conf_dir = root.path().join("conf");
        std::fs::create_dir_all(&entity_dir).unwrap();
        std::fs::create_dir_all(&conf_dir).unwrap();
        let real = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
        std::os::unix::fs::symlink(
            real.join("conf/MoquiDefaultConf.xml"),
            conf_dir.join("MoquiDefaultConf.xml"),
        )
        .unwrap();
        for entry in std::fs::read_dir(real.join("entity")).unwrap() {
            let entry = entry.unwrap();
            if !entry.file_type().unwrap().is_file() {
                continue;
            }
            std::os::unix::fs::symlink(entry.path(), entity_dir.join(entry.file_name())).unwrap();
        }
        std::fs::write(entity_dir.join("MantleProductAuthz.xml"), PRODUCT_XML).unwrap();
        (open_mem(root.path()), root)
    }

    fn open_mem(framework_dir: &Path) -> EntityFacade {
        crate::testing::open_at(framework_dir)
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(label: &str) -> TempDir {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("moqui-{label}-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
}
