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
//! so a lookup labels only the rows the frame's ids name: its key is the entity's single primary-key field, its
//! conditions are plain equalities, it never reads a restricted entity or a view over one, and it never keys, shows or
//! conditions on a password or encrypted field, nor on an identity field of `UserAccount` (its username, email address
//! and the rest), so a dataset does not list accounts' emails (plans/Security-Review.md SEC-TLS-82: a lookup on
//! `UserAccount` keyed by `disabled` answered password hashes).

use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex};

use moqui_core::context::ExecutionContext;
use moqui_core::entity::sensitive;
use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::{fail, text};

/// The most distinct keys one lookup resolves; a field with more keeps its raw values.
const MAX_KEYS: usize = 1000;

/// The dataset fields whose refused lookup was warned about, so a dashboard's queries warn once per field.
static WARNED: LazyLock<Mutex<BTreeSet<(String, String)>>> = LazyLock::new(Mutex::default);

/// How many dataset fields [`WARNED`] remembers; past that it starts again.
const MAX_WARNED: usize = 10_000;

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

/// Why `lookup` is not resolved, or None when it may be: its entity is defined and is not restricted to generic access
/// (nor a view over one), its `keyField` is the entity's single primary-key field, so it reads only the rows the frame's
/// ids name and not every row that shares a value, its `conditions` are a map of plain values (equalities), and none of
/// its key, display and condition fields is a password or encrypted field, nor an identity field of `UserAccount` (its
/// own, or a view's alias that reads one, by any name and through views of views).
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
    let Ok((full_name, pk_fields)) = facade.with_entity(entity_name, |entity, _, _| {
        let pk_fields: Vec<String> = entity.pk_field_names().into_iter().map(str::to_string).collect();
        Ok((entity.full_name.clone(), pk_fields))
    }) else {
        return Some(format!("entity {entity_name} is not defined"));
    };
    let access = sensitive::generic_access(&facade, &full_name);
    if !access.read {
        return Some(format!("entity {full_name} is not available to a lookup"));
    }
    if pk_fields != [key] {
        return Some(format!(
            "keyField {key} is not the single primary-key field of {full_name}, so it could read rows the frame does not name"
        ));
    }
    let conditions = match lookup.get("conditions") {
        None | Some(Json::Null) => None,
        Some(Json::Object(conditions)) => Some(conditions),
        Some(_) => return Some("lookup conditions must be a map of field names to values".into()),
    };
    if let Some((field, _)) = conditions
        .into_iter()
        .flatten()
        .find(|(_, value)| value.is_object() || value.is_array())
    {
        return Some(format!(
            "lookup condition {field} must be a plain value: a lookup's conditions are equalities"
        ));
    }
    let condition_fields = conditions.into_iter().flat_map(|map| map.keys().map(String::as_str));
    let fields: Vec<&str> = [key, display].into_iter().chain(condition_fields).collect();
    if let Some(field) = fields.iter().find(|field| access.hidden(field)) {
        return Some(format!(
            "field {field} of {full_name} is a password or encrypted field, which a lookup does not read"
        ));
    }
    // by origin: a view's alias of a UserAccount identity field under any name, through views of views
    let identity = sensitive::identity_fields(&facade, &full_name);
    if let Some(field) = fields.iter().find(|field| identity.iter().any(|name| name == **field)) {
        return Some(format!(
            "field {field} of {full_name} is an account's identity field, which a lookup does not read"
        ));
    }
    None
}

/// The refusal of the first `fieldDefaults.<field>.lookup` of a dataset body that may not be resolved ([`refusal`]):
/// what `save#Definition` refuses. The `statusTypeId` and `enumTypeId` forms always pass.
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
        .limit(MAX_KEYS as i64);
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
        let mut warned = moqui_lib::sync::lock(&WARNED);
        if warned.len() >= MAX_WARNED {
            warned.clear();
        }
        if warned.insert((dataset_id.to_string(), field_name.to_string())) {
            tracing::warn!(dataset=%dataset_id,field=%field_name,%reason,"report field lookup refused; keeping raw values");
        }
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
