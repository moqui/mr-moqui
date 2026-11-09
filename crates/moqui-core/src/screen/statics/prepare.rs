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

//! Per-request preparation of server-static screens and their section scopes.

use moqui_lib::node::MNode;
use moqui_lib::object::{is_empty, Value};

use super::{disable_key, disabled, editable_options, lit_of, MAX_WALK_DEPTH};

use crate::context::Error;
use crate::screen::definition::{Condition, ScreenDef, SectionDef, SectionKind};
use crate::screen::render::{check_condition, RenderState};
use crate::screen::sri;
use crate::script::{self, ContextFrame};

/// The request-specific names a section-iterate entry keeps in its own scope: control availability (disable_key and its
/// plain twin), options (option_key, editable_options) and nested section-iterate scopes.
const ENTRY_SCOPE_PREFIXES: [&str; 4] = ["mqDisable_", "mqOpt_", "mqPlain_", "mqSection_"];

/// Run the data preparation of a screen as the render does, without writing anything: the root actions, then the actions of
/// the sections the render reaches. A section-iterate runs its actions for each entry, and the list is kept as those actions left it.
pub(super) fn prepare_screen(state: &RenderState, def: &ScreenDef) -> Result<(), Error> {
    if let Some(actions) = &def.root.actions {
        script::run_actions(&actions.xml)?;
    }
    if let Some(widgets) = &def.root.widgets {
        walk_widgets(state, def, &widgets.shared_node()?, 0)?;
    }
    Ok(())
}

fn walk_widgets(state: &RenderState, owner: &ScreenDef, node: &MNode, depth: usize) -> Result<(), Error> {
    if depth > MAX_WALK_DEPTH {
        return Err(Error::new(format!(
            "sections of {} are nested too deeply",
            owner.location
        )));
    }
    for child in node.child_iter() {
        let name = child.name();
        match name.as_str() {
            "link" => {
                let key = disable_key(&lit_of(&child, true));
                script::set_context_name(&key, Value::Bool(disabled(state, &child)))?;
            }
            "section" | "section-iterate" => {
                let section_name = child.attribute("name").unwrap_or_default();
                match owner.sections.get(&section_name) {
                    Some(section) => prepare_section(state, owner, section, depth)?,
                    None => walk_widgets(state, owner, &child, depth)?,
                }
            }
            "section-include" => {
                let given = child.attribute("name").unwrap_or_default();
                let location = child.attribute("location").unwrap_or_default();
                let section_name = if given.is_empty() {
                    location
                        .split_once('#')
                        .map_or(String::new(), |(_, section)| section.to_string())
                } else {
                    given
                };
                if let Some(section) = owner.sections.get(&section_name) {
                    prepare_section(state, owner, section, depth)?;
                }
            }
            "include-screen" => {
                let location = child.attribute("location").unwrap_or_default();
                if location.contains("${") {
                    continue;
                }
                let other = state.facade.screen_def(&state.factory, &location)?;
                if let Some(actions) = &other.root.actions {
                    script::run_actions(&actions.xml)?;
                }
                if let Some(widgets) = &other.root.widgets {
                    walk_widgets(state, &other, &widgets.shared_node()?, depth + 1)?;
                }
            }
            "editable" => {
                if let Some((key, options)) = editable_options(state, &child) {
                    script::set_context_name(&key, moqui_lib::json::from_json(&options))?;
                }
            }
            // A form reads its own scope, and the rest of these hold no widgets with actions.
            "form-single" | "form-list" | "tree" | "actions" | "condition" | "row-actions" | "entity-find" => {}
            _ => walk_widgets(state, owner, &child, depth)?,
        }
    }
    Ok(())
}

fn prepare_section(state: &RenderState, owner: &ScreenDef, section: &SectionDef, depth: usize) -> Result<(), Error> {
    if let SectionKind::Iterate {
        list,
        entry,
        key,
        paginate,
    } = &section.kind
    {
        let value = script::eval_value(list)?;
        if is_empty(&value) {
            return Ok(());
        }
        let mut items = sri::iteration_entries(&value);
        // paginate: the page keys of the list go in the package, and the list is the page, as ScreenSection renders it.
        if *paginate && is_name(list) {
            items = sri::paginate_items(list, items)?;
        }
        let mut kept = Vec::with_capacity(items.len());
        let mut scopes = Vec::with_capacity(items.len());
        let count = items.len();
        for (index, (item_key, item)) in items.into_iter().enumerate() {
            let frame = ContextFrame::push();
            let outcome = (|| -> Result<(Value, Value), Error> {
                script::set_context_name(entry, item.clone())?;
                if let (Some(key_name), Some(item_key)) = (key, &item_key) {
                    script::set_context_name(key_name, Value::string(item_key))?;
                }
                let index_value = Value::I64(i64::try_from(index).unwrap_or(0));
                script::set_context_name("sectionEntryIndex", index_value.clone())?;
                script::set_context_name(&format!("{entry}_index"), index_value)?;
                script::set_context_name(&format!("{entry}_has_next"), Value::Bool(index + 1 < count))?;
                if let Some(actions) = &section.actions {
                    script::run_actions(&actions.xml)?;
                }
                if let Some(widgets) = &section.widgets {
                    walk_widgets(state, owner, &widgets.shared_node()?, depth + 1)?;
                }
                // These request-specific values belong to this iteration, not the cached template or another entry.
                let extras = script::top_context_entries()
                    .into_iter()
                    .filter(|(name, _)| ENTRY_SCOPE_PREFIXES.iter().any(|prefix| name.starts_with(prefix)))
                    .collect();
                Ok((script::context_value(entry).unwrap_or(item), Value::Map(extras)))
            })();
            drop(frame);
            let (item, extras) = outcome?;
            kept.push(item);
            scopes.push(extras);
        }
        // The list is what the per-entry actions left, when the list is a name the package holds.
        if is_name(list) {
            script::set_context_name(list, Value::List(kept))?;
        }
        if scopes.iter().any(|scope| !is_empty(scope)) {
            script::set_context_name(&format!("mqSection_{}", section.name), Value::List(scopes))?;
        }
        return Ok(());
    }
    let holds = match &section.condition {
        Some(Condition::Expression(expression)) => script::eval_condition(expression)?,
        Some(condition) => check_condition(condition)?,
        None => true,
    };
    let fragment = if holds {
        if let Some(actions) = &section.actions {
            script::run_actions(&actions.xml)?;
        }
        section.widgets.as_ref()
    } else {
        section.fail_widgets.as_ref()
    };
    if let Some(fragment) = fragment {
        walk_widgets(state, owner, &fragment.shared_node()?, depth + 1)?;
    }
    Ok(())
}

fn is_name(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        && !text.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}
