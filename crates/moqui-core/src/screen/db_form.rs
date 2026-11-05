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

//! Forms made of `DbForm` records (Java's `getDbFormNode`), read as of a date (docs/ui/Db-Forms.md).
//!
//! A `DbForm` with `isDated` has effective-dated fields, options and dependencies: the form is read as of a date, now by
//! default, and a row with no `fromDate` is a draft, left out unless drafts are asked for. A `DbFormFieldDependency` in
//! effect becomes `visible-when` (effect show) or `require-when` (effect require) on its target field. The rows are read
//! through the entity cache by form and filtered here, so the cache holds one list for every date.

use std::collections::{HashMap, HashSet};

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::context::{Error, ExecutionContext};
use crate::entity::{EntityList, EntityValue};

/// The attributes of a `form-single` or `form-list` that set the date its `DbForm` is read as of: an expression for a
/// time stamp (or text or epoch millis), and one for whether drafts are included (true, "true" or "Y"), for a preview.
/// They are on the form, not in the context, so a request parameter cannot move the date or show the drafts.
pub const AS_OF_ATTRIBUTE: &str = "db-form-as-of";
pub const DRAFTS_ATTRIBUTE: &str = "db-form-include-drafts";

/// The date a `DbForm` is read as of, and whether its drafts are included.
#[derive(Clone, Copy, Debug)]
pub struct AsOf {
    pub millis: i64,
    pub drafts: bool,
}

impl AsOf {
    /// From a date value and a drafts flag; a missing or unreadable date is now.
    pub fn of(date: Option<&Value>, drafts: Option<&Value>) -> Self {
        let millis = date
            .and_then(millis_of)
            .unwrap_or_else(crate::user::now_effective_millis);
        Self {
            millis,
            drafts: drafts.is_some_and(is_true),
        }
    }

    /// Whether `form` has either attribute ([`AS_OF_ATTRIBUTE`], [`DRAFTS_ATTRIBUTE`]).
    pub fn declared_on(form: &MNode) -> bool {
        [AS_OF_ATTRIBUTE, DRAFTS_ATTRIBUTE]
            .iter()
            .any(|name| form.attribute(name).is_some_and(|text| !text.trim().is_empty()))
    }

    /// From the attributes of `form`, each expression evaluated in the render's context; no form, or no attribute, is
    /// now without drafts.
    ///
    /// # Errors
    ///
    /// Returns an error when an expression does not evaluate.
    pub fn of_form(form: Option<&MNode>) -> Result<Self, Error> {
        let eval = |name: &str| -> Result<Option<Value>, Error> {
            match form.and_then(|form| form.attribute(name)).filter(|text| !text.trim().is_empty()) {
                Some(expression) => crate::script::eval_value(&expression).map(Some),
                None => Ok(None),
            }
        };
        let date = eval(AS_OF_ATTRIBUTE)?;
        let drafts = eval(DRAFTS_ATTRIBUTE)?;
        Ok(Self::of(date.as_ref(), drafts.as_ref()))
    }
}

fn is_true(value: &Value) -> bool {
    matches!(value, Value::Bool(true)) || matches!(moqui_lib::object::to_plain_string(value).as_str(), "true" | "Y")
}

fn millis_of(value: &Value) -> Option<i64> {
    if value.is_null() || moqui_lib::object::to_plain_string(value).is_empty() {
        return None;
    }
    match crate::l10n::basic_convert(value, "java.sql.Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

/// Where a row stands as of a date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    InEffect,
    Draft,
    Out,
}

fn standing(row: &EntityValue, dated: bool, as_of: AsOf) -> Standing {
    if row
        .get("thruDate")
        .and_then(millis_of)
        .is_some_and(|thru| thru <= as_of.millis)
    {
        return Standing::Out;
    }
    match row.get("fromDate").and_then(millis_of) {
        Some(from) if from <= as_of.millis => Standing::InEffect,
        Some(_) => Standing::Out,
        None if !dated => Standing::InEffect,
        None if as_of.drafts => Standing::Draft,
        None => Standing::Out,
    }
}

/// The rows of a list in effect as of a date, with its drafts when they are asked for. A draft that replaces a row
/// (`replaces` gives the key it replaces, and `key` each row's) puts that row out, so a preview shows the form as it
/// will be once the drafts start.
fn in_effect(
    rows: &EntityList,
    dated: bool,
    as_of: AsOf,
    key: impl Fn(&EntityValue) -> String,
    replaces: impl Fn(&EntityValue) -> String,
) -> Vec<&EntityValue> {
    let standings: Vec<(Standing, &EntityValue)> = rows.iter().map(|row| (standing(row, dated, as_of), row)).collect();
    let replaced: HashSet<String> = standings
        .iter()
        .filter(|(standing, _)| *standing == Standing::Draft)
        .map(|(_, row)| replaces(row))
        .filter(|name| !name.is_empty())
        .collect();
    standings
        .into_iter()
        .filter(|(standing, row)| match standing {
            Standing::InEffect => !replaced.contains(&key(row)),
            Standing::Draft => true,
            Standing::Out => false,
        })
        .map(|(_, row)| row)
        .collect()
}

pub(super) fn db_text(value: &EntityValue, field: &str) -> String {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn db_number(value: &EntityValue, field: &str) -> i64 {
    db_text(value, field).parse().unwrap_or(0)
}

/// The operators of a `DbFormFieldDependency`, and of `visible-when` and `require-when` (xml-form-3.xsd WhenAttributes).
const OPERATORS: &[&str] = &["equals", "not-equals", "empty", "not-empty"];
/// The effects of a `DbFormFieldDependency` and the element each becomes on its target field.
const EFFECTS: &[(&str, &str)] = &[("show", "visible-when"), ("require", "require-when")];

/// The `DbForm` ids that modify the form, each with the user groups it is for: `DbFormLookup`, read as its two entities.
pub(super) fn db_form_groups(location: &str, name: &str) -> Result<Vec<(String, Vec<String>)>, Error> {
    let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
        return Ok(Vec::new());
    };
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let forms = facade
        .find("moqui.screen.form.DbForm")?
        .eq("modifyXmlScreenForm", Value::string(format!("{location}#{name}")))
        .use_cache(true)
        .list()?;
    let mut out = Vec::new();
    for form in forms.iter() {
        let form_id = db_text(form, "formId");
        let groups = facade
            .find("moqui.screen.form.DbFormUserGroup")?
            .eq("formId", Value::string(form_id.clone()))
            .use_cache(true)
            .list()?;
        out.push((
            form_id,
            groups.iter().map(|group| db_text(group, "userGroupId")).collect(),
        ));
    }
    Ok(out)
}

/// The `DbForm` nodes that modify this form for the current user's groups, as `getDbFormNodeList`.
/// They are read as of the date the screen form's attributes give ([`AsOf::of_form`]).
pub(super) fn user_db_forms(location: &str, name: &str, form: Option<&MNode>) -> Result<Vec<MNode>, Error> {
    let forms = db_form_groups(location, name)?;
    if forms.is_empty() {
        return Ok(Vec::new());
    }
    let user_groups = crate::user::current_user_groups()?;
    let as_of = AsOf::of_form(form)?;
    forms
        .iter()
        .filter(|(_, groups)| groups.iter().any(|group| user_groups.contains(group)))
        .map(|(id, _)| {
            let node = db_form_node(id, as_of)?;
            // F10: a field a DbForm adds or changes names the DbForm, for the screen builder's canvas
            if crate::devtools::is_dev_instance() {
                for field in node.children_named("field") {
                    field.set_attribute(super::stamp::ATTRIBUTE, format!("dbform:{id}:0:0"));
                }
            }
            Ok(node)
        })
        .collect()
}

/// Field attributes and sub-field attributes of a `DbFormFieldAttribute`; any other name goes on the widget.
const DB_FIELD_ATTRIBUTES: &[&str] = &["name", "from", "entry-name", "hide"];
const DB_SUB_FIELD_ATTRIBUTES: &[&str] = &[
    "title",
    "tooltip",
    "red-when",
    "validate-service",
    "validate-parameter",
    "validate-entity",
    "validate-field",
];

/// The XML of the form a `DbForm` makes as of a date, for `ec.screen.getFormNode("DbForm#id", asOf, drafts)`.
///
/// # Errors
///
/// As [`db_form_node`].
pub fn db_form_xml(form_id: &str, as_of: AsOf) -> Result<String, Error> {
    Ok(db_form_node(form_id, as_of)?.write_string())
}

/// `getDbFormNode`: a form-single or form-list built from a `DbForm` and its `DbFormField`, attribute, option and
/// dependency records in effect as of a date.
pub(super) fn db_form_node(form_id: &str, as_of: AsOf) -> Result<MNode, Error> {
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let id = Value::string(form_id);
    let form = facade
        .find("moqui.screen.form.DbForm")?
        .eq("formId", id.clone())
        .use_cache(true)
        .one()?
        .ok_or_else(|| Error::new(format!("Could not find DbForm record with ID [{form_id}]")))?;
    let dated = db_text(&form, "isDated") == "Y";
    let node = MNode::new(if db_text(&form, "isListForm") == "Y" {
        "form-list"
    } else {
        "form-single"
    });
    let all_fields = facade
        .find("moqui.screen.form.DbFormField")?
        .eq("formId", id.clone())
        .order_by("layoutSequenceNum", false)
        .use_cache(true)
        .list()?;
    let fields = in_effect(
        &all_fields,
        dated,
        as_of,
        |row| db_text(row, "fieldName"),
        |row| db_text(row, "replacesFieldName"),
    );
    let field_names: HashSet<String> = fields.iter().map(|row| db_text(row, "fieldName")).collect();
    let dependencies = facade
        .find("moqui.screen.form.DbFormFieldDependency")?
        .eq("formId", id.clone())
        .order_by("dependencySeqId", false)
        .use_cache(true)
        .list()?;
    let conditions = dependency_conditions(form_id, &dependencies, dated, as_of, &field_names);
    for db_field in fields {
        let field_name = db_text(db_field, "fieldName");
        let field_type = db_text(db_field, "fieldTypeEnumId");
        let Some(widget_name) = field_type.get(6..).filter(|name| !name.is_empty()) else {
            // failure tolerance: the form, and the response services that read it, keep its other fields
            tracing::error!("DbFormField {form_id}:{field_name} has no fieldTypeEnumId; left out of the form");
            continue;
        };
        let field = MNode::new("field");
        field.set_attribute("name", field_name.clone());
        let entry = db_text(db_field, "entryName");
        if !entry.is_empty() {
            field.set_attribute("from", entry);
        }
        let condition = db_text(db_field, "condition");
        let sub = if condition.is_empty() {
            field.append("default-field")
        } else {
            let sub = field.append("conditional-field");
            sub.set_attribute("condition", condition);
            sub
        };
        for attribute in ["title", "tooltip"] {
            let value = db_text(db_field, attribute);
            if !value.is_empty() {
                sub.set_attribute(attribute, value);
            }
        }
        if db_text(db_field, "isRequired") == "Y" {
            sub.set_attribute("required", "true");
        }
        let widget = sub.append(widget_name);
        let of_field = |entity: &str| -> Result<EntityList, Error> {
            Ok(facade
                .find(entity)?
                .eq("formId", id.clone())
                .eq("fieldName", Value::string(field_name.clone()))
                .use_cache(true)
                .list()?)
        };
        for attribute in of_field("moqui.screen.form.DbFormFieldAttribute")?.iter() {
            let name = db_text(attribute, "attributeName");
            let value = db_text(attribute, "value");
            if DB_FIELD_ATTRIBUTES.contains(&name.as_str()) {
                field.set_attribute(name, value);
            } else if DB_SUB_FIELD_ATTRIBUTES.contains(&name.as_str()) {
                sub.set_attribute(name, value);
            } else {
                widget.set_attribute(name, value);
            }
        }
        let all_options = of_field("moqui.screen.form.DbFormFieldOption")?;
        // a draft option replaces the option of the same key
        let options = in_effect(
            &all_options,
            dated,
            as_of,
            |row| db_text(row, "keyValue"),
            |row| db_text(row, "keyValue"),
        );
        let entity_options = of_field("moqui.screen.form.DbFormFieldEntOpts")?;
        let mut combined: Vec<(i64, bool, &EntityValue)> = options
            .into_iter()
            .map(|value| (db_number(value, "sequenceNum"), false, value))
            .chain(
                entity_options
                    .iter()
                    .map(|value| (db_number(value, "sequenceNum"), true, value)),
            )
            .collect();
        combined.sort_by_key(|(sequence, _, _)| *sequence);
        for (sequence_num, from_entity, option) in combined {
            if !from_entity {
                let node = widget.append("option");
                node.set_attribute("key", db_text(option, "keyValue"));
                node.set_attribute("text", db_text(option, "text"));
                continue;
            }
            let options_node = widget.append("entity-options");
            let option_text = db_text(option, "text");
            options_node.set_attribute(
                "text",
                if option_text.is_empty() {
                    "${description}".to_string()
                } else {
                    option_text
                },
            );
            let find = options_node.append("entity-find");
            find.set_attribute("entity-name", db_text(option, "entityName"));
            let of_sequence = |entity: &str| -> Result<Vec<EntityValue>, Error> {
                Ok(of_field(entity)?
                    .into_values()
                    .into_iter()
                    .filter(|value| db_number(value, "sequenceNum") == sequence_num)
                    .collect())
            };
            for cond in of_sequence("moqui.screen.form.DbFormFieldEntOptsCond")? {
                let node = find.append("econdition");
                node.set_attribute("field-name", db_text(&cond, "entityFieldName"));
                node.set_attribute("value", db_text(&cond, "value"));
            }
            let mut orders = of_sequence("moqui.screen.form.DbFormFieldEntOptsOrder")?;
            orders.sort_by_key(|order| db_number(order, "orderSequenceNum"));
            for order in orders {
                find.append("order-by")
                    .set_attribute("field-name", db_text(&order, "entityFieldName"));
            }
        }
        if let Some(found) = conditions.get(&field_name) {
            for condition in found {
                sub.append_node(condition.deep_copy());
            }
        }
        let layout = db_text(db_field, "layoutSequenceNum");
        if !layout.is_empty() {
            field.set_attribute("layoutSequenceNum", layout);
        }
        super::form::merge_field_node(&node, &field, false);
    }
    Ok(node)
}

/// The `visible-when` and `require-when` elements of the dependencies in effect, by target field. A dependency whose
/// source or target is not in the form is left out (a field it names may have ended), and so is one with an operator or
/// effect not in the allowlist, with `${` in its value, or an equals or not-equals with no value, which
/// `store#DbFormFieldDependency` would have refused, each with an error naming it.
/// A field takes one of each: a second dependency with the same target and effect is left out with a warning.
fn dependency_conditions(
    form_id: &str,
    rows: &EntityList,
    dated: bool,
    as_of: AsOf,
    field_names: &HashSet<String>,
) -> HashMap<String, Vec<MNode>> {
    let mut out: HashMap<String, Vec<MNode>> = HashMap::new();
    let found = in_effect(
        rows,
        dated,
        as_of,
        |row| db_text(row, "dependencySeqId"),
        |_| String::new(),
    );
    for row in found {
        let seq = db_text(row, "dependencySeqId");
        let source = db_text(row, "sourceFieldName");
        let target = db_text(row, "targetFieldName");
        if !field_names.contains(&source) || !field_names.contains(&target) {
            continue;
        }
        let operator = db_text(row, "operator");
        let operator = if operator.is_empty() {
            "equals".to_string()
        } else {
            operator
        };
        let effect = db_text(row, "effect");
        let Some((_, element)) = EFFECTS.iter().find(|(name, _)| *name == effect) else {
            tracing::error!(
                "DbFormFieldDependency {form_id}:{seq} has effect [{effect}], not show or require; left out"
            );
            continue;
        };
        if !OPERATORS.contains(&operator.as_str()) {
            tracing::error!(
                "DbFormFieldDependency {form_id}:{seq} has operator [{operator}], not in the allowlist; left out"
            );
            continue;
        }
        // the renders expand ${} in a value, so a value is data only
        let value = db_text(row, "value");
        if value.contains("${") {
            tracing::error!("DbFormFieldDependency {form_id}:{seq} has an expression in its value; left out");
            continue;
        }
        // the render modes read a blank equals differently (qvt: always, tui: not-empty), so it is never written
        if value.trim().is_empty() && (operator == "equals" || operator == "not-equals") {
            tracing::error!("DbFormFieldDependency {form_id}:{seq} has {operator} with no value; left out");
            continue;
        }
        let list = out.entry(target.clone()).or_default();
        if list.iter().any(|node| node.has_name(element)) {
            tracing::warn!("DbFormFieldDependency {form_id}:{seq}: field {target} already has a {element}; left out");
            continue;
        }
        let node = MNode::new(*element);
        node.set_attribute("field", source);
        node.set_attribute("value", value);
        if operator != "equals" {
            node.set_attribute("operator", operator);
        }
        list.push(node);
    }
    out
}
