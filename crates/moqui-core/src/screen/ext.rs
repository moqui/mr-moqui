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

//! Declared actions and panels (plans/archive/Comms-Framework.md §3.3, layers 2 and 3): `<ext-action>` and `<ext-panel>`, direct
//! children of a `screen` or a `screen-extend`, which another app adds to a screen it does not own, and which a `.qvue`
//! screen renders in its menus (`m-ext-actions`) and panes (`m-ext-panel`) and the terminal client lists.
//!
//! Each has a provider-qualified `name` (its key: a later extension replaces the whole descriptor), a `slot` of the
//! screen's (`message-actions`, `room-header`, ...), a `sequence`, a `label` and `icon`, and one target: a `screen` (a
//! path from the declaring screen; a dialog unless `target="page"`), or for an action a `transition` of the declaring
//! screen. `parameter` children map an item's fields to the target's parameters (`name`, `from`), or give one a fixed
//! `value` (the kind of record to make); nothing else of the item is passed. `condition` is a display test on the item's fields (`field`, `!field`, `field == 'v'`,
//! `field != 'v'`), never an authorization rule: the server leaves out a target the user may not view, and an item whose
//! `hook` (a provider hook it needs, such as `create-from`) no app the user may open answers; the target checks access
//! again when it is called.

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::definition::ScreenDef;
use super::url::ScreenUrlInfo;
use crate::context::Error;

type Item = moqui_lib::object::ValueMap;

/// One `ext-action` or `ext-panel`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtItem {
    pub name: String,
    pub slot: String,
    pub sequence: i64,
    pub label: String,
    pub icon: String,
    /// A path from the declaring screen (`Dialog`, `../Other/Page`), or empty.
    pub screen: String,
    /// A transition of the declaring screen, or empty. Actions only.
    pub transition: String,
    /// `dialog` (the default for a screen) or `page`.
    pub target: String,
    pub condition: Option<Condition>,
    /// `(name, from)`: the target's parameter and the item field it takes.
    pub parameters: Vec<(String, String)>,
    /// `(name, value)`: a target parameter with a fixed value.
    pub values: Vec<(String, String)>,
    /// The text of a confirmation asked before the action runs.
    pub confirm: String,
    /// A provider hook (`create-from`) the item needs: it is left out for a user no app answers that hook for.
    pub hook: String,
}

/// The display test of an item, parsed from `condition`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Condition {
    pub field: String,
    /// `truthy`, `falsy`, `equals` or `not-equals`.
    pub op: &'static str,
    pub value: String,
}

/// The declared items of a screen, each list in `sequence` order (ties keep the order written).
#[derive(Clone, Debug, Default)]
pub struct ExtItems {
    pub actions: Vec<ExtItem>,
    pub panels: Vec<ExtItem>,
}

impl ExtItems {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.panels.is_empty()
    }
}

fn attr(node: &MNode, name: &str) -> String {
    node.attribute(name)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn is_field_name(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

/// Parse a `condition`: `field`, `!field`, `field == 'value'` or `field != 'value'` (single or double quotes, or a bare
/// word). Anything else is an error, so no expression reaches the browser.
pub(crate) fn parse_condition(text: &str) -> Result<Option<Condition>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    for (token, op) in [("!=", "not-equals"), ("==", "equals")] {
        if let Some((field, value)) = text.split_once(token) {
            let field = field.trim();
            let value = value.trim();
            let unquoted = value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
                .or_else(|| value.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')))
                .unwrap_or(value);
            if !is_field_name(field) || unquoted.contains(['\'', '"']) {
                return Err(format!("condition {text} is not field == 'value'"));
            }
            return Ok(Some(Condition {
                field: field.to_string(),
                op,
                value: unquoted.to_string(),
            }));
        }
    }
    let (field, op) = match text.strip_prefix('!') {
        Some(rest) => (rest.trim(), "falsy"),
        None => (text, "truthy"),
    };
    if !is_field_name(field) {
        return Err(format!("condition {text} is not a field name"));
    }
    Ok(Some(Condition {
        field: field.to_string(),
        op,
        value: String::new(),
    }))
}

fn item_of(node: &MNode, kind: &str, location: &str) -> Option<ExtItem> {
    let name = attr(node, "name");
    let slot = attr(node, "slot");
    let screen = attr(node, "screen");
    let transition = if kind == "ext-action" {
        attr(node, "transition")
    } else {
        String::new()
    };
    if name.is_empty() || slot.is_empty() {
        tracing::error!("{location}: an {kind} needs a name and a slot; skipped");
        return None;
    }
    if screen.is_empty() == transition.is_empty() {
        let need = if kind == "ext-action" {
            "exactly one of screen and transition"
        } else {
            "a screen"
        };
        tracing::error!("{location}: {kind} {name} needs {need}; skipped");
        return None;
    }
    if screen.contains("://") || screen.starts_with('/') {
        tracing::error!("{location}: {kind} {name} names a screen by location or absolute path; it must be a path from the screen; skipped");
        return None;
    }
    let condition = match parse_condition(&attr(node, "condition")) {
        Ok(condition) => condition,
        Err(err) => {
            tracing::error!("{location}: {kind} {name}: {err}; skipped");
            return None;
        }
    };
    let mut parameters = Vec::new();
    let mut values = Vec::new();
    for parameter in node.children_named("parameter") {
        let target = attr(&parameter, "name");
        if let Some(value) = parameter.attribute("value") {
            if !is_field_name(&target) {
                tracing::error!("{location}: {kind} {name} has a parameter that is not a field name; skipped");
                return None;
            }
            values.push((target, value));
            continue;
        }
        let from = Some(attr(&parameter, "from"))
            .filter(|from| !from.is_empty())
            .unwrap_or_else(|| target.clone());
        if !is_field_name(&target) || !is_field_name(&from) {
            tracing::error!("{location}: {kind} {name} has a parameter that is not name and from field names; skipped");
            return None;
        }
        parameters.push((target, from));
    }
    let target = attr(node, "target");
    Some(ExtItem {
        name,
        slot,
        sequence: attr(node, "sequence").parse().unwrap_or(0),
        label: attr(node, "label"),
        icon: attr(node, "icon"),
        screen,
        transition,
        target: if target == "page" {
            "page".to_string()
        } else {
            "dialog".to_string()
        },
        condition,
        parameters,
        values,
        confirm: attr(node, "confirm"),
        hook: attr(node, "hook"),
    })
}

/// The `ext-action` and `ext-panel` children of the merged screen. One that is not valid is logged with the screen's
/// location and left out.
pub(super) fn of(screen: &MNode, location: &str) -> ExtItems {
    let read = |kind: &str| {
        let mut items: Vec<ExtItem> = screen
            .children_named(kind)
            .iter()
            .filter_map(|node| item_of(node, kind, location))
            .collect();
        items.sort_by_key(|item| item.sequence);
        items
    };
    ExtItems {
        actions: read("ext-action"),
        panels: read("ext-panel"),
    }
}

/// Merge the `kind` children of a screen-extend into the screen by `name`: one with a name the screen has replaces the
/// whole descriptor where it was; a new one is added.
pub(super) fn merge_extend(screen: &MNode, extend: &MNode, kind: &str) {
    for child in extend.children_named(kind) {
        let name = attr(&child, "name");
        let index = screen
            .children()
            .into_iter()
            .find(|candidate| candidate.name() == kind && !name.is_empty() && attr(candidate, "name") == name)
            .and_then(|candidate| screen.index_of(&candidate));
        match index {
            Some(index) => {
                let _ = screen.replace_child(index, child.deep_copy());
            }
            None => screen.append_node(child.deep_copy()),
        }
    }
}

/// The screen path names of a path from the screen at `base` (its path names): `..` goes up, `.` stays.
fn relative_names(base: &[String], path: &str) -> Vec<String> {
    let mut names = base.to_vec();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                names.pop();
            }
            name => names.push(name.to_string()),
        }
    }
    names
}

fn encoded(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("/{}", moqui_lib::string::url_encode_if_needed(name)))
        .collect()
}

fn text_or_null(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        Value::string(text)
    }
}

/// The JSON of `item` for the clients, with its `url`; None when the user may not view its target screen or the
/// declaring screen has no such transition.
fn item_json(
    factory: &std::sync::Arc<crate::context::ExecutionContextFactory>,
    root: &str,
    screen_names: &[String],
    def: &ScreenDef,
    item: &ExtItem,
) -> Result<Option<Value>, Error> {
    if !item.hook.is_empty() {
        let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::require_authz);
        if super::shell::declared_providers(factory, &item.hook).is_empty() {
            return Ok(None);
        }
    }
    let url = if item.transition.is_empty() {
        let names = relative_names(screen_names, &item.screen);
        let info = match ScreenUrlInfo::resolve(factory, root, &names, 0) {
            Ok(info) => info,
            Err(err) => {
                tracing::warn!(
                    "{}: {} {} has no screen at {}: {err}",
                    def.location,
                    item.slot,
                    item.name,
                    item.screen
                );
                return Ok(None);
            }
        };
        if !info.extra_path.is_empty()
            || !super::menu::permitted_under(
                &info.path_defs[..info.path_defs.len().saturating_sub(1)],
                &info.target_screen,
            )
        {
            return Ok(None);
        }
        encoded(&names)
    } else {
        if !def
            .transitions
            .iter()
            .any(|transition| transition.name == item.transition)
        {
            tracing::warn!(
                "{}: {} {} names no transition {}",
                def.location,
                item.slot,
                item.name,
                item.transition
            );
            return Ok(None);
        }
        format!(
            "{}/{}",
            encoded(screen_names),
            moqui_lib::string::url_encode_if_needed(&item.transition)
        )
    };
    let mut out = Item::default();
    out.insert("name".into(), Value::string(&item.name));
    out.insert("slot".into(), Value::string(&item.slot));
    out.insert("sequence".into(), Value::I64(item.sequence));
    out.insert("label".into(), Value::string(crate::l10n::localize(&item.label)?));
    out.insert("icon".into(), text_or_null(&item.icon));
    out.insert(
        "kind".into(),
        Value::string(if item.transition.is_empty() {
            "screen"
        } else {
            "transition"
        }),
    );
    out.insert("target".into(), Value::string(&item.target));
    out.insert("url".into(), Value::string(url));
    out.insert("confirm".into(), text_or_null(&crate::l10n::localize(&item.confirm)?));
    out.insert(
        "condition".into(),
        item.condition.as_ref().map_or(Value::Null, |condition| {
            let mut test = Item::default();
            test.insert("field".into(), Value::string(&condition.field));
            test.insert("op".into(), Value::string(condition.op));
            test.insert("value".into(), Value::string(&condition.value));
            Value::Map(test)
        }),
    );
    out.insert(
        "parameters".into(),
        Value::List(
            item.parameters
                .iter()
                .map(|(name, from)| {
                    let mut parameter = Item::default();
                    parameter.insert("name".into(), Value::string(name));
                    parameter.insert("from".into(), Value::string(from));
                    Value::Map(parameter)
                })
                .chain(item.values.iter().map(|(name, value)| {
                    let mut parameter = Item::default();
                    parameter.insert("name".into(), Value::string(name));
                    parameter.insert("value".into(), Value::string(value));
                    Value::Map(parameter)
                }))
                .collect(),
        ),
    );
    Ok(Some(Value::Map(out)))
}

/// `{actions, panels}` of the screen `def` at `screen_names` from `root`, for the user: the targets they may not view
/// are left out.
///
/// # Errors
///
/// Returns an error when a label cannot be localized.
pub(crate) fn items_json(
    factory: &std::sync::Arc<crate::context::ExecutionContextFactory>,
    root: &str,
    screen_names: &[String],
    def: &ScreenDef,
) -> Result<(Value, Value), Error> {
    let mut lists = [Vec::new(), Vec::new()];
    for (index, items) in [&def.ext_items.actions, &def.ext_items.panels].into_iter().enumerate() {
        for item in items {
            if let Some(json) = item_json(factory, root, screen_names, def, item)? {
                lists[index].push(json);
            }
        }
    }
    let [actions, panels] = lists;
    Ok((Value::List(actions), Value::List(panels)))
}

/// `sri.getExtItems(pathNames)`: `{actions, panels}` of the screen at the path, as `items_json`, for the `extItems`
/// transition. None when the user may not view the screen (401 without a user, 403 or 404 sent).
///
/// # Errors
///
/// Returns an error when the root screen of the request is not known.
pub fn ext_items(state: &super::RenderState, path_names: &[String]) -> Result<Option<Value>, Error> {
    if crate::user::current_username()?.is_none() {
        crate::web::send_json_error(401, "Authentication required");
        return Ok(None);
    }
    let root = state
        .url_info
        .path_defs
        .first()
        .map(|def| def.location.clone())
        .ok_or_else(|| Error::new("no root screen"))?;
    let info = match ScreenUrlInfo::resolve(&state.factory, &root, path_names, 0) {
        Ok(info) if info.extra_path.is_empty() => info,
        Ok(_) => {
            crate::web::send_json_error(404, &format!("Screen not found for path {path_names:?}"));
            return Ok(None);
        }
        Err(err) if err.kind() == crate::context::ErrorKind::NotFound => {
            crate::web::send_json_error(404, &format!("Screen not found for path {path_names:?}"));
            return Ok(None);
        }
        Err(err) => return Err(err),
    };
    if !super::menu::permitted_under(
        &info.path_defs[..info.path_defs.len().saturating_sub(1)],
        &info.target_screen,
    ) {
        crate::web::send_json_error(403, &format!("View not permitted for path {path_names:?}"));
        return Ok(None);
    }
    let (actions, panels) = items_json(&state.factory, &root, &info.full_path, &info.target_screen)?;
    let mut out = Item::default();
    out.insert("actions".into(), actions);
    out.insert("panels".into(), panels);
    Ok(Some(Value::Map(out)))
}

#[cfg(test)]
#[path = "ext_tests.rs"]
mod tests;
