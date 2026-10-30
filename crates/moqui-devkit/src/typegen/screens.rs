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

//! `screens.d.ts`: a partial context type for each screen, and for a server-static qvt screen the package its template
//! receives. Action scripts are dynamic, so a context interface lists only the keys the definition proves: parameters,
//! form maps and form-list lists, `section-iterate` lists, and the names `<set>`, the finds and `service-call` out-maps
//! write. `MoquiContext` is the index signature for the rest.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use moqui_db::model::EntityModel;
use moqui_lib::node::MNode;

use super::ts::{doc, ident, is_name, quote, server_type};
use moqui_core::screen::statics::{is_lookup, option_key, sub_nodes, HEADER_SUFFIXES, PAGE_SUFFIXES};
use moqui_core::screen::{ScreenDef, SectionKind};

/// One form of a screen: the assembled form element, or the form's own element when assembly failed.
pub(super) struct FormSource {
    pub name: String,
    pub list: bool,
    pub dynamic: bool,
    pub node: MNode,
}

/// One screen as the generator reads it.
pub(super) struct ScreenSource<'a> {
    pub location: String,
    pub def: &'a ScreenDef,
    pub forms: Vec<FormSource>,
    /// qvt is sent as a static template for this screen: marked (or the subscreens-only case) and eligible.
    pub server_static: bool,
}

/// Proven keys by type. The actions win over the widgets, and the widgets over the parameters: a form's `map` that an
/// `entity-find-one` fills is that entity value, not a map of the form's fields.
#[derive(Default)]
struct Keys {
    tiers: [BTreeMap<String, BTreeSet<String>>; 3],
}

const ACTIONS: usize = 0;
const WIDGETS: usize = 1;
const PARAMETERS: usize = 2;

impl Keys {
    fn add(&mut self, tier: usize, name: &str, ts_type: impl Into<String>) {
        if is_name(name) {
            self.tiers[tier]
                .entry(name.to_string())
                .or_default()
                .insert(ts_type.into());
        }
    }

    fn resolved(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for tier in &self.tiers {
            for (name, types) in tier {
                out.entry(name.clone())
                    .or_insert_with(|| types.iter().cloned().collect::<Vec<_>>().join(" | "));
            }
        }
        out
    }
}

/// The full name of an entity named in a definition, as a literal type argument. `None` for a name built at runtime.
fn entity_arg(name: &str, model: Option<&EntityModel>) -> Option<String> {
    if name.is_empty() || name.contains("${") {
        return None;
    }
    let full = model
        .and_then(|model| model.entity(name).ok())
        .map_or_else(|| name.to_string(), |entity| entity.full_name.clone());
    Some(quote(&full))
}

fn walk_actions(xml: &str, model: Option<&EntityModel>, keys: &mut Keys) {
    if let Ok(node) = MNode::parse_text("typegen-actions", xml) {
        visit_action(&node, model, keys);
    }
}

fn visit_action(node: &MNode, model: Option<&EntityModel>, keys: &mut Keys) {
    let attr = |name: &str| node.attribute(name).unwrap_or_default();
    match &*node.name_rc() {
        "set" => {
            let ts_type = match node.attribute("type").as_deref() {
                Some("NewList") => "unknown[]".to_string(),
                Some("NewMap") => "MoquiMap".to_string(),
                Some(java) if !java.is_empty() => server_type(java).to_string(),
                _ if !node.has_attribute("from") && node.has_attribute("value") => "string".to_string(),
                _ => "unknown".to_string(),
            };
            keys.add(ACTIONS, &attr("field"), ts_type);
        }
        "entity-find" => {
            if let Some(entity) = entity_arg(&attr("entity-name"), model) {
                keys.add(ACTIONS, &attr("list"), format!("EntityList<{entity}>"));
            }
        }
        "entity-find-one" => {
            if let Some(entity) = entity_arg(&attr("entity-name"), model) {
                keys.add(ACTIONS, &attr("value-field"), format!("EntityValue<{entity}> | null"));
            }
        }
        "entity-make-value" => {
            if let Some(entity) = entity_arg(&attr("entity-name"), model) {
                keys.add(ACTIONS, &attr("value-field"), format!("EntityValue<{entity}>"));
            }
        }
        "entity-find-count" => keys.add(ACTIONS, &attr("count-field"), "number"),
        "entity-find-related-one" => keys.add(ACTIONS, &attr("value-field"), "EntityValue | null"),
        "entity-find-related" => keys.add(ACTIONS, &attr("list-name"), "EntityList"),
        "service-call" => {
            let name = attr("name");
            let out_map = attr("out-map");
            if !name.is_empty() && !name.contains("${") && out_map != "context" {
                keys.add(ACTIONS, &out_map, format!("MoquiOut<{}>", quote(&name)));
            }
        }
        // A script is dynamic. The text of a `<script>` is not read.
        "script" => return,
        _ => {}
    }
    for child in node.child_iter() {
        visit_action(&child, model, keys);
    }
}

fn field_names(form: &MNode) -> Vec<String> {
    form.children_named("field")
        .iter()
        .filter_map(|field| field.attribute("name"))
        .filter(|name| !name.is_empty() && !name.contains("${"))
        .collect()
}

/// `{ "a"?: unknown; ... }` for the names, or `MoquiMap` when there are none.
fn unknown_map(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return "MoquiMap".to_string();
    }
    let body: Vec<String> = names.iter().map(|name| format!("{}?: unknown", quote(name))).collect();
    format!("{{ {} }}", body.join("; "))
}

fn object_type(entries: &BTreeMap<String, String>) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }
    let body: Vec<String> = entries
        .iter()
        .map(|(name, ts_type)| format!("{}?: {ts_type}", quote(name)))
        .collect();
    format!("{{ {} }}", body.join("; "))
}

fn form_entity(form: &MNode, model: Option<&EntityModel>) -> Option<String> {
    form.first_named("entity-find")
        .and_then(|find| entity_arg(&find.attribute("entity-name").unwrap_or_default(), model))
}

/// The keys the widgets prove: each form-single's map and each form-list's list and page keys.
fn widget_keys(source: &ScreenSource<'_>, model: Option<&EntityModel>, keys: &mut Keys) {
    // Form-singles that name the same map (most often the default, `fieldValues`) read one map with all their fields.
    let mut maps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for form in &source.forms {
        let fields: BTreeSet<String> = field_names(&form.node).into_iter().collect();
        if form.list {
            let list = form.node.attribute("list").unwrap_or_default();
            let ts_type = match form_entity(&form.node, model) {
                Some(entity) => format!("EntityList<{entity}>"),
                None => format!("{}[] | EntityList", unknown_map(&fields)),
            };
            keys.add(WIDGETS, &list, ts_type);
            if is_name(&list) {
                for suffix in PAGE_SUFFIXES {
                    keys.add(WIDGETS, &format!("{list}{suffix}"), "number");
                }
            }
        } else {
            let map = form
                .node
                .attribute("map")
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "fieldValues".to_string());
            maps.entry(map).or_default().extend(fields);
        }
    }
    for (map, fields) in &maps {
        keys.add(WIDGETS, map, unknown_map(fields));
    }
    for section in source.def.sections.values() {
        if let SectionKind::Iterate { list, .. } = &section.kind {
            keys.add(WIDGETS, list, "unknown[] | EntityList | MoquiMap");
        }
    }
}

/// What a form-single adds to the screen package under `mqExtra_{form}`, as `single_form_extras` builds it.
fn single_extras(form: &FormSource) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for field in form.node.children_named("field") {
        let field_name = field.attribute("name").unwrap_or_default();
        for (sub_index, sub) in sub_nodes(&field) {
            for set in sub.named_iter("set") {
                let target = set.attribute("field").unwrap_or_default();
                if !target.is_empty() {
                    out.insert(target, "unknown".to_string());
                }
            }
            for (widget_index, widget) in sub.children().into_iter().enumerate() {
                widget_extra(&form.name, &field_name, sub_index, widget_index, &widget, &mut out);
            }
        }
    }
    out
}

fn widget_extra(
    form_name: &str,
    field_name: &str,
    sub_index: usize,
    widget_index: usize,
    widget: &MNode,
    out: &mut BTreeMap<String, String>,
) {
    match &*widget.name_rc() {
        "display-entity" => {
            out.insert(format!("{field_name}_display"), "string".to_string());
        }
        "link" if widget.attribute("entity-name").is_some_and(|text| !text.is_empty()) => {
            out.insert(format!("{field_name}_display"), "string".to_string());
        }
        "drop-down" | "check" | "radio" if is_lookup(widget) => {
            out.insert(
                option_key(form_name, field_name, sub_index, widget_index),
                "MoquiOption[]".to_string(),
            );
        }
        _ => {}
    }
}

/// The row of a form-list package and the rest of its body, as `form_package` builds them.
fn form_list_package(form: &FormSource, model: Option<&EntityModel>) -> (String, String) {
    let mut row = BTreeMap::new();
    let mut body = BTreeMap::new();
    for field in form.node.children_named("field") {
        let field_name = field.attribute("name").unwrap_or_default();
        if field_name.is_empty() || field_name.contains("${") {
            continue;
        }
        row.insert(field_name.clone(), "unknown".to_string());
        for sub in field
            .children()
            .into_iter()
            .filter(|sub| matches!(&*sub.name_rc(), "default-field" | "conditional-field"))
        {
            for set in sub.named_iter("set") {
                let target = set.attribute("field").unwrap_or_default();
                if !target.is_empty() {
                    row.insert(target, "unknown".to_string());
                }
            }
            for widget in sub.child_iter() {
                let entity_text = widget.has_name("display-entity")
                    || (widget.has_name("link")
                        && widget.attribute("entity-name").is_some_and(|text| !text.is_empty()));
                if entity_text {
                    row.insert(format!("{field_name}_display"), "string".to_string());
                }
            }
        }
        if field.first_named("header-field").is_some() {
            for suffix in HEADER_SUFFIXES {
                body.insert(format!("{field_name}{suffix}"), "unknown".to_string());
            }
        }
        for (sub_index, sub) in sub_nodes(&field) {
            for (widget_index, widget) in sub.children().into_iter().enumerate() {
                if matches!(&*widget.name_rc(), "drop-down" | "check" | "radio") && is_lookup(&widget) {
                    body.insert(
                        option_key(&form.name, &field_name, sub_index, widget_index),
                        "MoquiOption[]".to_string(),
                    );
                }
            }
        }
    }
    let row_type = match form_entity(&form.node, model) {
        Some(entity) => format!("MoquiPlain<{entity}> & {}", object_type(&row)),
        None => object_type(&row),
    };
    (row_type, object_type(&body))
}

/// The interface name of each screen in `MoquiScreen`, by location.
fn screen_idents(screens: &[ScreenSource<'_>]) -> BTreeMap<String, String> {
    let mut used = BTreeSet::new();
    let mut out = BTreeMap::new();
    for screen in screens {
        let trimmed = screen
            .location
            .split_once("://")
            .map_or(screen.location.as_str(), |(_, rest)| rest);
        let trimmed = trimmed.strip_suffix(".xml").unwrap_or(trimmed);
        let base = ident(trimmed);
        let mut name = base.clone();
        let mut index = 2;
        while !used.insert(name.clone()) {
            name = format!("{base}_{index}");
            index += 1;
        }
        out.insert(screen.location.clone(), name);
    }
    out
}

/// The text of `screens.d.ts`. `failures` are the screens that did not load, with the reason.
pub(super) fn emit(screens: &[ScreenSource<'_>], model: Option<&EntityModel>, failures: &[(String, String)]) -> String {
    let idents = screen_idents(screens);
    let mut out = String::from(super::HEADER);
    out.push_str("// Screen contexts, partial: only the keys the definition proves. Action scripts may set others.\n");
    for (location, reason) in failures {
        let reason = reason.lines().next().unwrap_or_default();
        let _ = writeln!(out, "// not loaded: {location}: {reason}");
    }
    let mut namespace = String::new();
    let mut contexts = String::new();
    let mut packages = String::new();
    let mut form_lists = String::new();
    for screen in screens {
        let name = &idents[&screen.location];
        let mut keys = Keys::default();
        for actions in [
            &screen.def.always_actions,
            &screen.def.pre_actions,
            &screen.def.root.actions,
        ]
        .into_iter()
        .flatten()
        {
            walk_actions(&actions.xml, model, &mut keys);
        }
        for section in screen.def.sections.values() {
            if let Some(actions) = &section.actions {
                walk_actions(&actions.xml, model, &mut keys);
            }
        }
        widget_keys(screen, model, &mut keys);
        for parameter in &screen.def.parameters {
            keys.add(PARAMETERS, &parameter.name, "unknown");
        }
        let _ = writeln!(namespace, "    /** {} */", doc(&screen.location));
        let _ = writeln!(namespace, "    interface {name} {{");
        for (key, ts_type) in keys.resolved() {
            let _ = writeln!(namespace, "        {}?: {ts_type};", quote(&key));
        }
        namespace.push_str("    }\n");
        let _ = writeln!(
            contexts,
            "    {}: MoquiScreen.{name} & MoquiContext;",
            quote(&screen.location)
        );
        if !screen.server_static {
            continue;
        }
        let mut lists = BTreeSet::new();
        let mut extras = BTreeMap::new();
        for form in screen.forms.iter().filter(|form| !form.dynamic) {
            if form.list {
                let list = form.node.attribute("list").unwrap_or_default();
                if !list.is_empty() {
                    lists.insert(quote(&list));
                    let (row, body) = form_list_package(form, model);
                    let _ = writeln!(
                        form_lists,
                        "    {}: MoquiFormListPackage<{}, {row}, {body}>;",
                        quote(&format!("{}#{}", screen.location, form.name)),
                        quote(&list)
                    );
                }
            } else {
                let single = single_extras(form);
                if !single.is_empty() {
                    extras.insert(format!("mqExtra_{}", form.name), object_type(&single));
                }
            }
        }
        let lists = if lists.is_empty() {
            "never".to_string()
        } else {
            lists.into_iter().collect::<Vec<_>>().join(" | ")
        };
        let _ = writeln!(
            packages,
            "    {}: MoquiScreenPackage<MoquiScreen.{name}, {lists}, {}>;",
            quote(&screen.location),
            object_type(&extras)
        );
    }
    out.push_str("\ndeclare namespace MoquiScreen {\n");
    out.push_str(&namespace);
    out.push_str("}\n\n/** The server context of each screen by location: the proven keys, then any name. */\n");
    out.push_str("interface MoquiScreenContexts {\n");
    out.push_str(&contexts);
    out.push_str("}\n\n/** Server-static qvt screens: the `GET {screen}/actions` package, which the template receives as its props. */\n");
    out.push_str("interface MoquiScreenPackages {\n");
    out.push_str(&packages);
    out.push_str("}\n\n/** The form-lists of server-static qvt screens: `GET {screen}/actions/{formName}`, by `location#formName`. */\n");
    out.push_str("interface MoquiFormListPackages {\n");
    out.push_str(&form_lists);
    out.push_str("}\n");
    out
}
