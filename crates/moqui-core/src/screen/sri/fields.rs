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

//! Field values, the values of `parameter` and `set` elements, and the validation hints of a field.

use indexmap::IndexMap;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use super::NodeLit;
use crate::context::{Error, ExecutionContext};
use crate::screen::form;
use crate::screen::render::RenderState;
use crate::script::ContextFrame;
use crate::{l10n, script, web};

/// `sri.getFieldValue(field, default)`: the form's error parameters, the field's `from`, the form's
/// map, then the context, then the expanded default.
///
/// # Errors
///
/// Returns an error when an expression in the form does not evaluate.
pub fn get_field_value(field: &NodeLit, default_value: &str) -> Result<Value, Error> {
    let field_name = field.attr("name");
    let mut value = Value::Null;
    if let Some(form) = field.parent.as_deref() {
        if form.name == "form-single" {
            let errors = web::saved_for_page().error_parameters;
            if errors.get("moquiFormName").map(String::as_str) == Some(form.attr("name")) {
                if let Some(found) = errors.get(field_name).filter(|text| !text.is_empty()) {
                    return Ok(Value::string(found));
                }
            }
            let from = if field.attr("from").is_empty() {
                field.attr("entry-name")
            } else {
                field.attr("from")
            };
            if !from.is_empty() {
                return script::eval_value(from);
            }
            let map_name = if form.attr("map").is_empty() {
                "fieldValues"
            } else {
                form.attr("map")
            };
            let map = script::eval_value(map_name).unwrap_or(Value::Null);
            if let Some(found) = script::value_field(&map, field_name) {
                value = found;
            }
        }
    }
    if is_empty(&value) {
        let from_context = script::context_value(field_name);
        if let Some(found) = from_context.filter(|found| !is_empty(found)) {
            return Ok(found);
        }
    } else {
        return Ok(value);
    }
    let default_text = script::expand(default_value)?;
    if !default_text.is_empty() {
        return Ok(Value::String(default_text));
    }
    Ok(value)
}

fn format_for_mode<'a>(state: &RenderState, widget: &'a NodeLit) -> Option<&'a str> {
    let mut format = widget.attr("format");
    if state.render_mode == "text" || state.render_mode == "csv" {
        let text_format = widget.attr("text-format");
        if !text_format.is_empty() {
            format = text_format;
        }
    }
    Some(format).filter(|text| !text.is_empty())
}

/// `sri.getFieldValueString(widget)`.
///
/// # Errors
///
/// Returns an error when the value or its format cannot be made.
pub fn get_field_value_string(state: &RenderState, widget: &NodeLit) -> Result<String, Error> {
    let field = widget.field().ok_or_else(|| Error::new("widget has no field"))?;
    let value = get_field_value(field, widget.attr("default-value"))?;
    value_string(&value, format_for_mode(state, widget))
}

/// `sri.getFieldValueString(field, default, format, type)`. A date-time widget names its `type`, so a
/// value that reached the script as text or as milliseconds is formatted as the date, time or time stamp it is.
///
/// # Errors
///
/// Returns an error when the value or its format cannot be made.
pub fn get_field_value_string_for(
    field: &NodeLit,
    default_value: &str,
    format: &str,
    kind: &str,
) -> Result<String, Error> {
    let value = get_field_value(field, default_value)?;
    let format = Some(format).filter(|text| !text.is_empty());
    if let Value::String(text) = &value {
        if !text.is_empty() && !kind.is_empty() {
            let zone = l10n::time_zone();
            let parsed = match kind {
                "date" => moqui_lib::object::parse_date(text).map(Value::Date),
                "time" => moqui_lib::object::parse_time(text).map(Value::Time),
                _ => moqui_lib::object::parse_timestamp(text, &zone).map(Value::Timestamp),
            };
            if let Some(parsed) = parsed {
                return l10n::format_value(&parsed, format);
            }
        }
    }
    value_string(&value, format)
}

fn value_string(value: &Value, format: Option<&str>) -> Result<String, Error> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(text) => Ok(text.clone()),
        other => l10n::format_value(other, format),
    }
}

/// `sri.getFieldValuePlainString(field, default)`.
///
/// # Errors
///
/// Returns an error when the value cannot be made.
pub fn get_field_value_plain_string(field: &NodeLit, default_value: &str) -> Result<String, Error> {
    let value = get_field_value(field, "")?;
    if is_empty(&value) && !default_value.is_empty() {
        return script::expand(default_value);
    }
    Ok(to_plain_string(&value))
}

/// `sri.getFieldValueClass(field)`.
///
/// # Errors
///
/// Returns an error when the value cannot be made.
pub fn get_field_value_class(field: &NodeLit) -> Result<String, Error> {
    Ok(match get_field_value(field, "")? {
        Value::String(_) | Value::Null => "String",
        Value::Bool(_) => "Boolean",
        Value::I32(_) => "Integer",
        Value::I64(_) => "Long",
        Value::F64(_) => "Double",
        Value::Decimal(_) => "BigDecimal",
        Value::Timestamp(_) => "Timestamp",
        Value::Date(_) => "Date",
        Value::Time(_) => "Time",
        Value::Bytes(_) => "byte[]",
        Value::List(_) => "ArrayList",
        Value::Map(_) => "LinkedHashMap",
        Value::Object(_) => "EntityValue",
    }
    .to_string())
}

/// `sri.getNamedValuePlain(fieldName, formNode)`.
///
/// # Errors
///
/// Returns an error when the form's map expression does not evaluate.
pub fn get_named_value_plain(field_name: &str, form: &NodeLit) -> Result<String, Error> {
    let mut value = None;
    if form.name == "form-single" {
        let map_name = if form.attr("map").is_empty() {
            "fieldValues"
        } else {
            form.attr("map")
        };
        let map = script::eval_value(map_name)?;
        value = script::value_field(&map, field_name);
    }
    let value = value.or_else(|| script::context_value(field_name));
    Ok(value.map(|found| to_plain_string(&found)).unwrap_or_default())
}

fn default_text(widget: &NodeLit) -> Result<String, Error> {
    let text = widget.attr("default-text");
    if text.is_empty() {
        Ok(String::new())
    } else {
        script::expand_localized(text)
    }
}

/// `sri.getFieldEntityValue(widget)`: the text of the entity value the field's value points at.
///
/// # Errors
///
/// Returns an error when the entity cannot be found or read.
pub fn get_field_entity_value(widget: &NodeLit) -> Result<String, Error> {
    let field = widget.field().ok_or_else(|| Error::new("widget has no field"))?;
    let field_value = get_field_value(field, "")?;
    if field_value.is_null() {
        return default_text(widget);
    }
    let entity_name = widget.attr("entity-name");
    let shape = form::entity_shape(entity_name)?;
    let mut key_field = widget.attr("key-field-name");
    if key_field.is_empty() {
        key_field = widget.attr("entity-key-name");
    }
    let key_field = if key_field.is_empty() {
        shape.pk_fields.first().cloned().unwrap_or_default()
    } else {
        key_field.to_string()
    };
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("no entity facade is installed"))?;
    // As Java's getFieldEntityValue: through the entity cache unless the widget says `use-cache="false"`, so a form-list
    // does not query once per row for the same few values.
    let use_cache = [widget.attr("use-cache"), widget.attr("entity-use-cache")]
        .into_iter()
        .find(|text| !text.is_empty())
        .is_none_or(|text| text == "true");
    let found = facade
        .find(entity_name)?
        .eq(&key_field, field_value)
        .use_cache(use_cache)
        .one()?;
    let Some(found) = found else {
        return default_text(widget);
    };
    let text = widget.attr("text");
    if !text.is_empty() {
        let fields: IndexMap<String, Value> = found
            .localized_fields(&facade)
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect();
        let _frame = ContextFrame::push();
        script::put_context_all(&fields);
        return script::expand_localized(text);
    }
    if shape.description_field.is_empty() {
        return Ok(String::new());
    }
    Ok(found
        .get_localized(&facade, &shape.description_field)
        .as_ref()
        .map(to_plain_string)
        .unwrap_or_default())
}

/// `sri.addNodeParameters`: the `parameter-map`, `parameter` children and `depends-on` values of an element.
///
/// # Errors
///
/// Returns an error when an expression fails.
pub fn node_parameters(parent: &NodeLit) -> Result<IndexMap<String, Value>, Error> {
    node_parameters_checked(parent).map(|(parameters, _)| parameters)
}

/// [`node_parameters`] and whether every `depends-on` field has a value in the context or the form map.
/// With `depends-optional` true an empty field only leaves its parameter out.
///
/// # Errors
///
/// Returns an error when an expression fails.
pub fn node_parameters_checked(parent: &NodeLit) -> Result<(IndexMap<String, Value>, bool), Error> {
    let mut parameters = IndexMap::new();
    let map_expr = parent.attr("parameter-map");
    if !map_expr.is_empty() {
        if let Some(map) = script::value_map(&script::eval_value(map_expr)?) {
            parameters.extend(map);
        }
    }
    for parameter in parent.children.iter().filter(|child| child.name == "parameter") {
        let name = parameter.attr("name");
        let from = if parameter.attr("from").is_empty() {
            name
        } else {
            parameter.attr("from")
        };
        parameters.insert(name.to_string(), context_value(from, parameter.attr("value"))?);
    }
    let optional = parent.attr("depends-optional") == "true";
    let mut has_all = true;
    for depends in parent.children.iter().filter(|child| child.name == "depends-on") {
        let field = depends.attr("field");
        let parameter = if depends.attr("parameter").is_empty() {
            field
        } else {
            depends.attr("parameter")
        };
        let mut value = script::context_value(field).unwrap_or(Value::Null);
        if is_empty(&value) {
            if let Some(found) = script::context_value("_formMap").and_then(|map| script::value_field(&map, field)) {
                value = found;
            }
        }
        if is_empty(&value) {
            has_all = false;
        } else {
            parameters.insert(parameter.to_string(), value);
        }
    }
    Ok((parameters, has_all || optional))
}

/// `sri.getContextValue(from, value)`.
///
/// # Errors
///
/// Returns an error when the expression fails.
pub fn context_value(from: &str, value: &str) -> Result<Value, Error> {
    if !value.is_empty() {
        return Ok(Value::String(script::expand_localized(value)?));
    }
    if !from.is_empty() {
        return script::eval_value(from);
    }
    Ok(Value::string(""))
}

/// `sri.setInContext(setNode)`: a `<set>` under a field. `set-if-empty="false"` skips a null or empty
/// new value; otherwise the field is assigned, including an empty value. `value` and `default-value`
/// go through [`script::expand`], which does not localize. That is the XSD rule `XmlActions.js`
/// already uses. Java's `setInContext` tests the raw attribute string, so `"false"` is truthy there.
///
/// # Errors
///
/// Returns an error when the expression fails.
pub fn set_in_context(set: &NodeLit) -> Result<(), Error> {
    let field = set.attr("field");
    if field.is_empty() {
        return Ok(());
    }
    let mut value = if !set.attr("from").is_empty() {
        script::eval_value(set.attr("from"))?
    } else if let Some(text) = set.attrs.get("value") {
        Value::String(script::expand(text)?)
    } else {
        Value::Null
    };
    if null_or_empty_string(&value) && !set.attr("default-value").is_empty() {
        value = Value::String(script::expand(set.attr("default-value"))?);
    }
    if !set.attr("type").is_empty() {
        value = l10n::basic_convert(&value, set.attr("type")).value;
    }
    // After the default and the type conversion, so a default that fills an empty value is kept.
    if set.attr("set-if-empty") == "false" && null_or_empty_string(&value) {
        return Ok(());
    }
    script::set_context_name(field, value)
}

/// Null or `""`. A `0`, `false`, or empty list is a value, as `XmlActions.js` checks `!= null && !== ""`.
fn null_or_empty_string(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.is_empty(),
        _ => false,
    }
}

// ---- validation ----

/// What the html macros read of a field's validation: input classes and a pattern with its message.
#[derive(Clone, Debug, Default)]
pub struct FieldValidation {
    pub classes: Vec<String>,
    pub regexp: Option<(String, String)>,
}

/// `FormInstance.getFieldValidationClasses` and `getFieldValidationRegexpInfo` for a field sub-node (`default-field`
/// and the like): `validate-service` and `validate-parameter`, else `validate-entity` and `validate-field`.
///
/// # Errors
///
/// Returns an error when the named entity does not exist.
pub fn field_validation(sub: &NodeLit) -> Result<FieldValidation, Error> {
    let field_name = sub.parent.as_ref().map_or("", |field| field.attr("name"));
    let mut out = FieldValidation::default();
    let service = sub.attr("validate-service");
    let entity = sub.attr("validate-entity");
    if !service.is_empty() {
        let parameter = match sub.attr("validate-parameter") {
            "" => field_name,
            other => other,
        };
        if let Some(hints) = crate::service::input_hints(service, parameter) {
            out.classes = hints.classes.iter().map(|class| (*class).to_string()).collect();
            out.regexp = hints.regexp;
        }
    } else if !entity.is_empty() {
        let name = match sub.attr("validate-field") {
            "" => field_name,
            other => other,
        };
        let shape = form::entity_shape(entity)?;
        if let Some((_, kind, _)) = shape.fields.iter().find(|(field, _, _)| field == name) {
            if kind.starts_with("number-") || kind.starts_with("currency-") {
                out.classes.push("number".to_string());
            }
        }
    }
    Ok(out)
}

/// `getFieldValidationJsRules` for a field sub-node, as rules: the checks of its `validate-service` parameter, or for a
/// numeric `validate-entity` field, each with its localized message. qvt writes each with [`crate::service::ClientRule::js`].
///
/// # Errors
///
/// Returns an error when the named entity does not exist.
pub fn field_validation_rules(sub: &NodeLit) -> Result<Vec<(crate::service::ClientRule, String)>, Error> {
    let field_name = sub.parent.as_ref().map_or("", |field| field.attr("name"));
    let service = sub.attr("validate-service");
    let entity = sub.attr("validate-entity");
    let rules = if !service.is_empty() {
        let parameter = match sub.attr("validate-parameter") {
            "" => field_name,
            other => other,
        };
        crate::service::input_rules(service, parameter).unwrap_or_default()
    } else if !entity.is_empty() {
        let name = match sub.attr("validate-field") {
            "" => field_name,
            other => other,
        };
        let shape = form::entity_shape(entity)?;
        match shape.fields.iter().find(|(field, _, _)| field == name) {
            Some((_, kind, _)) if kind.starts_with("number-") || kind.starts_with("currency-") => {
                if kind.ends_with("integer") {
                    vec![(
                        crate::service::ClientRule::Integer,
                        "Please enter a valid whole number".to_string(),
                    )]
                } else {
                    vec![(
                        crate::service::ClientRule::Number,
                        "Please enter a valid number".to_string(),
                    )]
                }
            }
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    Ok(rules
        .into_iter()
        .map(|(rule, message)| (rule, l10n::localize(&message).unwrap_or(message)))
        .collect())
}
