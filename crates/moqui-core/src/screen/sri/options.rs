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

//! The options of a drop-down, radio or check, and the label a transition gives for a value.

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use super::fields::node_parameters_checked;
use super::NodeLit;
use crate::context::Error;
use crate::screen::form;
use crate::screen::render::RenderState;
use crate::script::ContextFrame;
use crate::{l10n, script};

/// `sri.getFieldOptions(widget)`: the options of a drop-down, radio or check in order.
///
/// # Errors
///
/// Returns an error when an `entity-find` or `list-options` expression fails.
pub fn get_field_options(state: &RenderState, widget: &NodeLit) -> Result<Vec<(String, String)>, Error> {
    let field_name = widget.field().map_or("", |field| field.attr("name"));
    // A key given again keeps the place it was first given and takes the last label (plans/High-Level-Review.md
    // HL-005): what `IndexMap::insert` does, without a scan of the options so far.
    let mut options: IndexMap<String, String> = IndexMap::default();
    let mut put = |key: String, text: String| {
        options.insert(key, text);
    };
    for child in &widget.children {
        match child.name.as_str() {
            "entity-options" => {
                let Some(find) = child.child("entity-find") else {
                    continue;
                };
                for item in find_list(find)? {
                    if let Some((key, text)) = field_option(field_name, child, &item)? {
                        put(key, text);
                    }
                }
            }
            "list-options" => {
                let list = script::eval_value(child.attr("list"))?;
                let has_key_or_text = !child.attr("key").is_empty() || !child.attr("text").is_empty();
                for item in script::value_items(&list).unwrap_or_default() {
                    if script::value_map(&item).is_some() {
                        if let Some((key, text)) = field_option(field_name, child, &item)? {
                            put(key, text);
                        }
                    } else if has_key_or_text {
                        let wrapped = Value::Map(IndexMap::from_iter([("entry".to_string(), item)]));
                        if let Some((key, text)) = field_option(field_name, child, &wrapped)? {
                            put(key, text);
                        }
                    } else {
                        let key = to_plain_string(&item);
                        let text = l10n::localize(&key)?;
                        put(key, text);
                    }
                }
            }
            "option" => {
                let mut key = child.attr("key").to_string();
                if key.contains("${") {
                    key = script::expand(&key)?;
                }
                let text = if child.attrs.contains_key("text") {
                    script::expand_localized(child.attr("text"))?
                } else {
                    l10n::localize(&key)?
                };
                put(key, text);
            }
            _ => {}
        }
    }
    if options.is_empty() {
        if let Some(dynamic) = widget.child("dynamic-options") {
            return dynamic_options(state, dynamic);
        }
    }
    Ok(options.into_iter().collect())
}

/// Render `transition` as a test render from the root screen of `state`, with `parameters`, as the widget's browser
/// would ask for it. The caller checks its dependencies first and reads the response its own way.
fn render_transition(
    state: &RenderState,
    transition: &str,
    parameters: moqui_lib::object::ValueMap,
) -> Result<crate::screen::screen_test::TestRender, Error> {
    let url = crate::screen::urls::build_url(state, transition)?;
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.clone())
        .ok_or_else(|| Error::new("no root screen"))?;
    let parameters: std::collections::BTreeMap<String, Value> = parameters.into_iter().collect();
    crate::screen::screen_test::ScreenTest::new(&state.factory)
        .root_screen(root)
        .render(&url.path_with_params(), parameters)
}

/// The options of a `dynamic-options` element, from its transition called as a test render with the
/// `depends-on` values as parameters. It gives no options while a `depends-on` field is empty, and
/// none with `server-search`, where the browser asks as the user types. The transition's JSON is a
/// list of maps, or a map with an `options` list, each with the value and label fields.
fn dynamic_options(state: &RenderState, node: &NodeLit) -> Result<Vec<(String, String)>, Error> {
    if node.attr("server-search") == "true" {
        return Ok(Vec::new());
    }
    let field = |name: &str, default: &'static str| {
        let given = node.attr(name);
        if given.is_empty() {
            default.to_string()
        } else {
            given.to_string()
        }
    };
    let (label_field, value_field) = (field("label-field", "label"), field("value-field", "value"));
    let (parameters, has_all_depends) = node_parameters_checked(node)?;
    if !has_all_depends {
        return Ok(Vec::new());
    }
    let rendered = render_transition(state, node.attr("transition"), parameters)?;
    let json = rendered.json_value()?;
    let list = match &json {
        Value::List(items) => items.clone(),
        Value::Map(entries) => match entries.get("options") {
            Some(Value::List(items)) => items.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let mut options: IndexMap<String, String> = IndexMap::default();
    for entry in &list {
        let Value::Map(entry) = entry else {
            continue;
        };
        let value = entry.get(&value_field).map(to_plain_string).unwrap_or_default();
        let label = entry.get(&label_field).map(to_plain_string).unwrap_or_default();
        if !value.is_empty() && !label.is_empty() {
            options.insert(value, label);
        }
    }
    Ok(options.into_iter().collect())
}

/// Run an `entity-find` element as actions and return the rows.
fn find_list(find: &NodeLit) -> Result<Vec<Value>, Error> {
    const LIST: &str = "fieldOptionList__";
    let mut find = find.clone();
    find.attrs.insert("list".to_string(), LIST.to_string());
    let xml = format!("<actions>{}</actions>", find.to_xml());
    let (ran, list) = {
        let _frame = ContextFrame::push();
        (script::run_actions(&xml), script::context_value(LIST))
    };
    ran?;
    Ok(list.and_then(|value| script::value_items(&value)).unwrap_or_default())
}

/// `ScreenForm.addFieldOption`: the key and text of one option, or None when it has no key.
fn field_option(field_name: &str, option: &NodeLit, item: &Value) -> Result<Option<(String, String)>, Error> {
    let _frame = ContextFrame::push();
    field_option_in(field_name, option, item)
}

fn field_option_in(field_name: &str, option: &NodeLit, item: &Value) -> Result<Option<(String, String)>, Error> {
    let fields = script::value_map(item).unwrap_or_default();
    script::put_context_all(&fields);
    let entity_name = script::value_entity_name(item);
    let mut key = None;
    let key_attr = option.attr("key");
    if key_attr.is_empty() {
        if let Some(entity_name) = &entity_name {
            let shape = form::entity_shape(entity_name)?;
            if let Some(first) = shape.pk_fields.first() {
                key = fields.get(first).map(to_plain_string);
            }
        }
    } else {
        let expanded = script::expand(key_attr)?;
        key = Some(expanded).filter(|text| text != "null");
    }
    if key.is_none() {
        key = script::context_value(field_name).map(|value| to_plain_string(&value));
    }
    let Some(key) = key else {
        return Ok(None);
    };
    let text_attr = option.attr("text");
    let text = if text_attr.is_empty() {
        let has_description = match &entity_name {
            Some(name) => form::entity_shape(name)?
                .fields
                .iter()
                .any(|(field, _, _)| field == "description"),
            None => true,
        };
        match fields.get("description").filter(|_| has_description) {
            Some(description) if !description.is_null() => to_plain_string(description),
            _ => l10n::localize(&key)?,
        }
    } else {
        let expanded = script::expand_localized(text_attr)?;
        if expanded == "null" {
            l10n::localize(&key)?
        } else {
            expanded
        }
    };
    Ok(Some((key, text)))
}

/// `sri.getFieldTransitionValue(transition, node, term, labelField, alwaysGet)`: the label a transition gives for a
/// value, as the widget's options would show it, by a test render of the transition with `term` and the node's
/// parameters. None with no term (unless `alwaysGet`) or while a `depends-on` field is empty.
///
/// # Errors
///
/// Returns an error when the transition cannot be rendered.
pub fn field_transition_value(
    state: &RenderState,
    transition: &str,
    node: &NodeLit,
    term: &str,
    label_field: &str,
    always_get: bool,
) -> Result<Option<String>, Error> {
    if !always_get && term.is_empty() {
        return Ok(None);
    }
    let label_field = if label_field.is_empty() { "label" } else { label_field };
    let (mut parameters, has_all_depends) = node_parameters_checked(node)?;
    if !has_all_depends {
        return Ok(None);
    }
    parameters.insert("term".to_string(), Value::string(term));
    let rendered = render_transition(state, transition, parameters)?;
    let first_label = |item: &Value| match item {
        Value::Map(entry) => entry.get(label_field).map(to_plain_string),
        other => Some(to_plain_string(other)),
    };
    Ok(match rendered.json_value() {
        Ok(Value::List(items)) => items.first().and_then(first_label),
        Ok(Value::Map(entries)) => match entries.get("options") {
            Some(Value::List(items)) if !items.is_empty() => items.first().and_then(first_label),
            _ => entries.get(label_field).map(to_plain_string),
        },
        Ok(Value::Null) => None,
        Ok(other) => Some(to_plain_string(&other)),
        Err(_) => Some(rendered.text()),
    })
}
