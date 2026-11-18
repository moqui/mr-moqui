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

//! A dataset field's lookup (`fieldDefaults.<field>.lookup`, or its `statusTypeId` and `enumTypeId` forms): the labels
//! of the ids already in an authorized frame, read with authorization off and answered as the field's `mappings`
//! (docs/tools/Monitoring.md, Dashboards). The entity, key field, display field and conditions are the dataset author's,
//! so a lookup reads only a label entity ([`LABEL_ENTITIES`] and the conf's `report@lookup-entities`), never a
//! restricted entity or a view over one, and never keys, shows or conditions on a password or encrypted field
//! (plans/Security-Review.md SEC-TLS-82: a lookup on `UserAccount` answered password hashes).

use std::collections::BTreeSet;

use moqui_core::context::ExecutionContext;
use moqui_core::entity::sensitive;
use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::{fail, text};

/// The label tables every lookup may read; the conf's `report@lookup-entities` adds more.
const LABEL_ENTITIES: &[&str] = &["moqui.basic.Enumeration", "moqui.basic.StatusItem", "moqui.basic.Uom"];

/// The most distinct keys one lookup resolves; a field with more keeps its raw values.
const MAX_KEYS: usize = 1000;
/// [`MAX_KEYS`] as a find limit.
const MAX_ROWS: i64 = 1000;

/// The lookup a field's config asks for: its `lookup`, else a `statusTypeId` (a `StatusItem` description) or an
/// `enumTypeId` (an `Enumeration` description).
fn of_config(config: &Map<String, Json>) -> Option<Json> {
    config
        .get("lookup")
        .cloned()
        .or_else(|| {
            config.get("statusTypeId").map(|id| {
                json!({"entityName":"moqui.basic.StatusItem","keyField":"statusId","displayField":"description",
                    "conditions":{"statusTypeId":id}})
            })
        })
        .or_else(|| {
            config.get("enumTypeId").map(|id| {
                json!({"entityName":"moqui.basic.Enumeration","keyField":"enumId","displayField":"description",
                    "conditions":{"enumTypeId":id}})
            })
        })
}

/// Whether `full_name` is a label entity: one of [`LABEL_ENTITIES`] or of the conf's `report@lookup-entities` (full
/// entity names, separated by commas or white space).
fn is_label_entity(ec: &ExecutionContext, full_name: &str) -> bool {
    LABEL_ENTITIES.contains(&full_name)
        || ec
            .factory()
            .conf_setting("report", "lookup-entities")
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .any(|name| name == full_name)
}

/// Why `lookup` is not resolved, or None when it may be: it names a label entity ([`is_label_entity`]) that is not
/// restricted to generic access (nor a view over one), and a key field, display field and condition fields none of
/// which is a password or encrypted field.
pub(crate) fn refusal(ec: &ExecutionContext, lookup: &Json) -> Option<String> {
    if !lookup.is_object() {
        return Some("a lookup must be a map".into());
    }
    let entity_name = text(lookup, "entityName", "");
    let key = text(lookup, "keyField", "");
    let display = text(lookup, "displayField", "description");
    if entity_name.is_empty() || key.is_empty() {
        return Some("a lookup needs entityName and keyField".into());
    }
    let Some(facade) = ec.factory().entity() else {
        return Some("the entity facade is unavailable".into());
    };
    let Ok(full_name) = facade.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone())) else {
        return Some(format!("entity {entity_name} is not defined"));
    };
    if !is_label_entity(ec, &full_name) {
        return Some(format!(
            "entity {full_name} is not a label entity a lookup may read (moqui.basic.Enumeration, StatusItem, Uom, or the conf's report lookup-entities)"
        ));
    }
    let access = sensitive::generic_access(&facade, &full_name);
    if !access.read {
        return Some(format!("entity {full_name} is not available to a lookup"));
    }
    let conditions = match lookup.get("conditions") {
        None | Some(Json::Null) => None,
        Some(Json::Object(conditions)) => Some(conditions),
        Some(_) => return Some("lookup conditions must be a field-name map".into()),
    };
    let condition_fields = conditions.into_iter().flat_map(|map| map.keys().map(String::as_str));
    let fields: Vec<&str> = [key, display].into_iter().chain(condition_fields).collect();
    if let Some(field) = fields.into_iter().find(|field| access.hidden(field)) {
        return Some(format!(
            "field {field} of {full_name} is a password or encrypted field, which a lookup does not read"
        ));
    }
    None
}

/// The refusal of the first `fieldDefaults.<field>.lookup` of a dataset body that may not be resolved ([`refusal`]):
/// what `save#Definition` refuses. The `statusTypeId` and `enumTypeId` forms always read a label entity.
pub(crate) fn check_field_defaults(ec: &ExecutionContext, body: &Json) -> Result<(), Error> {
    let Some(fields) = body["fieldDefaults"].as_object() else {
        return Ok(());
    };
    for (name, config) in fields {
        let Some(lookup) = config.get("lookup") else {
            continue;
        };
        if let Some(reason) = refusal(ec, lookup) {
            return Err(fail(format!(
                "dataset fieldDefaults.{name}.lookup is refused: {reason}"
            )));
        }
    }
    Ok(())
}

/// The rows of `lookup` whose key is one of `keys` (each JSON text), as `{value, text}` mappings. One cached query of
/// at most [`MAX_KEYS`] rows.
fn resolve(ec: &ExecutionContext, lookup: &Json, keys: &BTreeSet<String>) -> Result<Vec<Json>, Error> {
    let entity = ec
        .factory()
        .entity()
        .ok_or_else(|| fail("entity facade is unavailable"))?;
    let key = text(lookup, "keyField", "");
    let display = text(lookup, "displayField", "description");
    let values = keys
        .iter()
        .map(|key| {
            serde_json::from_str::<Json>(key)
                .map(|value| moqui_lib::json::from_json(&value))
                .map_err(fail)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut find = entity
        .find(text(lookup, "entityName", ""))?
        .condition(moqui_core::entity::Condition::is_in(key, values))
        .use_cache(true)
        .limit(MAX_ROWS);
    for (name, value) in lookup["conditions"].as_object().into_iter().flat_map(|map| map.iter()) {
        find = find.eq(name, moqui_lib::json::from_json(value));
    }
    let rows = find.list()?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get(key).zip(row.get(display)))
        .map(|(key, display)| {
            json!({"value":moqui_lib::json::to_json(key),"text":moqui_lib::object::to_plain_string(display)})
        })
        .collect())
}

/// The mappings a field's lookup gives for the distinct `values` of its column, or None when its config has no lookup,
/// the column has no key or more than [`MAX_KEYS`], the lookup is refused ([`refusal`], a warning) or its query fails
/// (an error): the field then keeps its raw values, and the query goes on.
pub(crate) fn mappings(
    ec: &ExecutionContext,
    dataset_id: &str,
    field_name: &str,
    config: &Map<String, Json>,
    values: &[Json],
) -> Option<Vec<Json>> {
    let lookup = of_config(config)?;
    let keys: BTreeSet<String> = values
        .iter()
        .filter(|value| !value.is_null() && !value.is_array() && !value.is_object())
        .map(Json::to_string)
        .collect();
    if keys.len() > MAX_KEYS {
        tracing::warn!(dataset=%dataset_id,field=%field_name,"report lookup exceeds 1000 distinct keys; keeping raw values");
        return None;
    }
    if keys.is_empty() {
        return None;
    }
    if let Some(reason) = refusal(ec, &lookup) {
        tracing::warn!(dataset=%dataset_id,field=%field_name,%reason,"report field lookup refused; keeping raw values");
        return None;
    }
    // The labels of ids already in the authorized frame: the dataset's read permission and row filters decided what the
    // viewer sees, so reading a label table needs no grant of its own; which tables and fields is [`refusal`]'s rule.
    let resolved = {
        let _authz = ec.disable_authz();
        resolve(ec, &lookup, &keys)
    };
    match resolved {
        Ok(mappings) => Some(mappings),
        Err(error) => {
            tracing::error!(dataset=%dataset_id,field=%field_name,%error,"report field lookup failed; keeping authorized raw values");
            None
        }
    }
}

#[cfg(test)]
#[path = "lookup_tests.rs"]
mod tests;
