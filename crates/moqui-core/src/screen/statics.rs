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

//! Server-static `qvt` (`plans/archive/QvtServerStatic.md`): which screens it applies to, the cached template, and the `actions`
//! transition that answers the data of a screen and of each of its form-lists.
//!
//! A server-static screen is sent as a script. The `ScreenQvtStatic.js` units run once, with no actions and no request data,
//! and the template they write is compiled by the Vue compiler into a render function, so the page that loads it needs no
//! `unsafe-eval`. The script puts the render function in `moqui.qvtStatic[path]`, the client loads it with a script
//! element and fills it from `GET {screen}/actions` and `GET {screen}/actions/{formName}`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use moqui_lib::codec::{fnv1a, fnv1a_extend};
use moqui_lib::node::MNode;
use moqui_lib::object::{is_empty, Value};

use super::definition::{Condition, ScreenDef, SectionDef, SectionKind};
use super::form;
use super::generate::UnitRequest;
use super::render::{check_condition, Frames, RenderState};
use super::sri::{self, NodeLit, Runner};
use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::script::{self, ContextFrame};
use crate::web::{self, WebRequest};

/// The unit mode the static macros generate for, and the module that writes it.
pub(crate) const UNIT_MODE: &str = "qvt-static";
pub(crate) const STATIC_MODULE: &str = "framework/template/screen/ScreenQvtStatic.js";
/// The module of server-merged qvt. A screen with another macro module for qvt is not transcribed.
pub(crate) const MERGED_MODULE: &str = "framework/template/screen/ScreenQvt.js";
/// The conf's `Cache-Control` of a server-static response, `response-header` `screen-server-static`.
pub fn server_static_cache_control() -> String {
    web::conf_header("screen-server-static", "Cache-Control").unwrap_or_else(|| "private, no-store".to_string())
}
const COMPILER_LOCATION: &str = "component://webroot/screen/webroot/libs/vue/compiler-dom.cjs.js";
const STATIC_MODULE_SOURCE: &str =
    include_str!("../../../../runtime/base-component/framework/template/screen/ScreenQvtStatic.js");
/// The keys the renderer sets for a paginated list, after the list name.
pub(crate) const PAGE_SUFFIXES: [&str; 6] = [
    "Count",
    "PageIndex",
    "PageSize",
    "PageMaxIndex",
    "PageRangeLow",
    "PageRangeHigh",
];

// ---- eligibility ----

/// Whether qvt is sent to the browser as a static template for a screen, and when it is not, why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Eligibility {
    /// `server-static` names qvt (or the automatic subscreens case).
    pub marked: bool,
    /// Marked, or checked as if marked, and nothing in the screen needs request data in its template.
    pub eligible: bool,
    pub reasons: Vec<String>,
}

fn names_of(text: &str) -> Vec<String> {
    text.split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn includes_qvt(list: &[String]) -> bool {
    list.iter().any(|name| name == "all" || name == "qvt")
}

fn descendants_named(node: &MNode, names: &[&str], out: &mut Vec<MNode>) {
    for child in node.children() {
        if names.contains(&child.name().as_str()) {
            out.push(child.clone());
        }
        descendants_named(&child, names, out);
    }
}

/// The reasons the widget tree of `def` cannot be one template. `force` skips the check that the screen is marked.
fn structure_reasons(
    facade: &super::ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &ScreenDef,
    macro_location: &str,
) -> Vec<String> {
    let mut reasons = Vec::new();
    if macro_location != MERGED_MODULE && macro_location != STATIC_MODULE {
        reasons.push(format!(
            "the qvt macro module is {macro_location}, not the built-in one"
        ));
    }
    if !def.allows_render_mode("qvt") {
        reasons.push("render-modes does not include qvt".to_string());
    }
    if def.allow_extra_path {
        reasons.push(
            "allow-extra-path: the extra path would be lost, since the data request is a path of its own".to_string(),
        );
    }
    for (name, form) in &def.forms {
        if form.dynamic {
            reasons.push(format!(
                "form {name} is dynamic=\"true\", so its widgets depend on request data"
            ));
        }
        let Ok(base) = form.node.node() else {
            continue;
        };
        let mut nodes = vec![base];
        nodes.extend(form.extensions.iter().filter_map(|ext| ext.node().ok()));
        let expression = |value: Option<String>| value.is_some_and(|text| text.contains("${"));
        for node in &nodes {
            if expression(node.attribute("extends")) {
                reasons.push(format!("form {name} extends an expression"));
            }
            for child in node.children() {
                match child.name().as_str() {
                    "auto-fields-entity" | "auto-widget-entity" if expression(child.attribute("entity-name")) => {
                        reasons.push(format!(
                            "form {name} takes its fields from an entity named by an expression"
                        ));
                    }
                    "auto-fields-service" | "auto-widget-service" if expression(child.attribute("service-name")) => {
                        reasons.push(format!(
                            "form {name} takes its fields from a service named by an expression"
                        ));
                    }
                    _ => {}
                }
            }
        }
        // The form inherits the screen's value and an extension may override it, so the last one that says anything wins.
        let own = nodes
            .iter()
            .rev()
            .find_map(|node| node.attribute("server-static").filter(|text| !text.is_empty()));
        if let Some(text) = own {
            if !includes_qvt(&names_of(&text)) {
                reasons.push(format!(
                    "form {name} says server-static=\"{text}\", which leaves out qvt"
                ));
            }
        }
    }
    for (name, section) in &def.sections {
        if matches!(section.kind, SectionKind::Include { .. }) {
            reasons.push(format!("section-include {name} names its section with an expression"));
        }
    }
    if let Ok(source) = def.screen_node() {
        let mut includes = Vec::new();
        descendants_named(&source, &["include-screen"], &mut includes);
        for include in includes {
            let location = include.attribute("location").unwrap_or_default();
            if location.contains("${") {
                reasons.push("include-screen names its screen with an expression".to_string());
            } else if let Ok(other) = facade.screen_def(factory, &location) {
                if !other.forms.is_empty() {
                    reasons.push(format!(
                        "include-screen of {location}, which has forms (the form requests are named by the including screen)"
                    ));
                }
            }
        }
    }
    reasons
}

/// The eligibility of `def` for static qvt. `force` answers as if the screen were marked, which the compatibility report asks.
pub(crate) fn eligibility_of(
    facade: &super::ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &Arc<ScreenDef>,
    macro_location: &str,
    force: bool,
) -> Arc<Eligibility> {
    let key = format!("{macro_location}|{force}");
    if let Some(found) = def.generated.eligibility(&key) {
        return found;
    }
    let marked = includes_qvt(&def.server_static);
    let mut reasons = Vec::new();
    if !marked && !force {
        reasons.push("server-static does not name qvt".to_string());
    }
    reasons.extend(structure_reasons(facade, factory, def, macro_location));
    let result = Arc::new(Eligibility {
        marked,
        eligible: (marked || force) && reasons.is_empty(),
        reasons,
    });
    def.generated.put_eligibility(key, Arc::clone(&result));
    result
}

// ---- the cached template ----

/// A static template, compiled. `html` is the Vue template and `script` the body that is sent.
#[derive(Debug)]
pub(crate) struct StaticTemplate {
    pub html: String,
    pub script: String,
    pub etag: String,
}

/// A number for the resolved definition of a screen: its merged XML (`screen-extend` included), each form and its
/// extensions, the files it was read from, and the macro module. A change to any of them changes the validator.
fn identity(def: &ScreenDef) -> u64 {
    let mut hash = fnv1a(def.source.text().as_bytes());
    for (name, form) in &def.forms {
        hash = fnv1a_extend(hash, name.as_bytes());
        hash = fnv1a_extend(hash, form.node.text().as_bytes());
        for extension in &form.extensions {
            hash = fnv1a_extend(hash, extension.text().as_bytes());
        }
    }
    for source in &def.sources {
        hash = fnv1a_extend(hash, source.location.as_bytes());
        hash = fnv1a_extend(hash, &source.modified.unwrap_or(0).to_le_bytes());
    }
    fnv1a_extend(hash, STATIC_MODULE_SOURCE.as_bytes())
}

/// The compiler's text, read once for the factory.
struct CompilerText(Mutex<Option<Arc<str>>>);

pub(crate) fn compiler_source(factory: &ExecutionContextFactory) -> Result<Arc<str>, Error> {
    let kept = factory.extension(|_| CompilerText(Mutex::new(None)));
    if let Some(text) = kept.0.lock().unwrap_or_else(|err| err.into_inner()).as_ref() {
        return Ok(Arc::clone(text));
    }
    let text: Arc<str> = read_compiler_source(factory)?.into();
    *kept.0.lock().unwrap_or_else(|err| err.into_inner()) = Some(Arc::clone(&text));
    Ok(text)
}

fn read_compiler_source(factory: &ExecutionContextFactory) -> Result<String, Error> {
    let found = factory
        .resource()
        .get(COMPILER_LOCATION)
        .and_then(|resource| resource.read_text());
    // A test context has no webroot component.
    #[cfg(test)]
    if found.is_err() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/base-component/webroot/screen/webroot/libs/vue/compiler-dom.cjs.js");
        return std::fs::read_to_string(path).map_err(|err| Error::new(err.to_string()));
    }
    found.map_err(|err| {
        Error::new(format!(
            "the Vue compiler is not available at {COMPILER_LOCATION}: {err}"
        ))
    })
}

/// The script that registers a compiled template: `code` is the body of a function of `Vue` that returns the render function.
fn wrap_script(key: &str, code: &str) -> String {
    let key = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "(function(){{var m=globalThis.moqui=globalThis.moqui||{{}};(m.qvtStatic=m.qvtStatic||{{}})[{key}]=function(Vue){{\n{code}\n}};}})();\n"
    )
}

/// The template for the request path `key`: made once for each definition, then kept with it. A failure is kept too, so
/// a screen that cannot be a static template is not built again on every request until its definition changes.
fn template_for(state: &RenderState, runner: &dyn Runner, key: &str) -> Result<Arc<StaticTemplate>, Error> {
    let def = Arc::clone(&state.url_info.target_screen);
    if let Some(found) = def.generated.static_template(key) {
        return Ok(found);
    }
    if let Some(reason) = def.generated.static_failure(key) {
        return Err(Error::new(reason));
    }
    let _building = def.generated.static_build_lock();
    if let Some(found) = def.generated.static_template(key) {
        return Ok(found);
    }
    match build_template(state, runner, &def, key) {
        Ok(made) => {
            def.generated.put_static_template(key.to_string(), Arc::clone(&made));
            Ok(made)
        }
        Err(err) if is_security(&err) => Err(err),
        Err(err) => {
            def.generated
                .put_static_failure(key.to_string(), err.message().to_string());
            Err(err)
        }
    }
}

fn is_security(err: &Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::AuthenticationRequired | ErrorKind::Unauthorized | ErrorKind::Tarpit
    )
}

fn build_template(
    state: &RenderState,
    runner: &dyn Runner,
    def: &Arc<ScreenDef>,
    key: &str,
) -> Result<Arc<StaticTemplate>, Error> {
    state
        .screen_path_index
        .set(state.url_info.path_defs.len().saturating_sub(1));
    let request = UnitRequest {
        factory: &state.factory,
        facade: &state.facade,
        def,
        section: "",
        fail: false,
        mode: UNIT_MODE,
        macro_location: STATIC_MODULE,
    };
    // The template is written into the web slot, which holds this request's response: set that aside, and put it back
    // whether the template is made or not, so a failure leaves nothing behind for the merged render that follows.
    let stash = web::stash_output();
    let written = (|| -> Result<String, Error> {
        match runner.generate(&request)? {
            Some(unit) => {
                runner.run(&unit)?;
                let (_, _, body, _) = web::take_output();
                String::from_utf8(body).map_err(|err| Error::new(err.to_string()))
            }
            None => Ok(String::new()),
        }
    })();
    web::restore_output(stash);
    let html = written?;
    let code = super::vue_check::compile_render(&compiler_source(&state.factory)?, &html)
        .map_err(|err| Error::new(format!("{}: {}", def.location, err.message())))?;
    let script = wrap_script(key, &code);
    let etag = format!("W/\"{:x}\"", fnv1a_extend(identity(def), script.as_bytes()));
    Ok(Arc::new(StaticTemplate { html, script, etag }))
}

/// The key of a template: the request path, as the client names it, without the render mode extension or the query.
pub(crate) fn template_key(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path.strip_suffix(".qvt").unwrap_or(path).to_string()
}

/// Send the static template for the screen being rendered when it is eligible. Returns false when the screen stays a server merge.
///
/// # Errors
///
/// Returns a security error. A template that cannot be made is logged and the screen stays a server merge.
pub(crate) fn serve_template(state: &RenderState, request: &WebRequest, path: &str) -> Result<bool, Error> {
    let info = &state.url_info;
    if state.render_mode != "qvt" || info.render_path_difference + 1 != info.path_defs.len() {
        return Ok(false);
    }
    let force = report_forces_static();
    let eligibility = eligibility_of(
        &state.facade,
        &state.factory,
        &info.target_screen,
        &state.macro_location,
        force,
    );
    if !eligibility.eligible {
        report_ineligible(&info.target_screen, &eligibility);
        // A report has what it needs, and the merged render would only run the screen's actions again.
        return Ok(report_active());
    }
    let mut frames = Frames::untracked();
    let last = info.path_defs.len() - 1;
    for (index, def) in info.path_defs.iter().enumerate() {
        frames.enter_screen(def, index == last)?;
    }
    let key = template_key(path);
    let made = script::with_screen_runner(|runner| template_for(state, runner, &key));
    let template = match made {
        Ok(template) => template,
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::AuthenticationRequired | ErrorKind::Unauthorized | ErrorKind::Tarpit
            ) =>
        {
            return Err(err);
        }
        Err(err) => {
            tracing::warn!(
                "{} is not sent as a static template: {err}",
                info.target_screen.location
            );
            report_failure(&info.target_screen, &err);
            return Ok(report_active());
        }
    };
    report_template(&info.target_screen, &template);
    if !report_active() {
        super::static_files::respond_bytes(
            request,
            "text/javascript;charset=UTF-8",
            &server_static_cache_control(),
            &[("X-Moqui-Qvt".to_string(), "static-template".to_string())],
            &template.etag,
            template.script.as_bytes().to_vec(),
        );
    }
    Ok(true)
}

// ---- the actions transition ----

fn lit_of(node: &MNode, with_children: bool) -> NodeLit {
    NodeLit {
        name: node.name(),
        attrs: node.attributes().into_iter().collect(),
        text: node.text().unwrap_or_default(),
        children: if with_children {
            node.children().iter().map(lit_down).collect()
        } else {
            Vec::new()
        },
        parent: node.parent().map(|parent| Box::new(lit_of(&parent, false))),
    }
}

fn lit_down(node: &MNode) -> NodeLit {
    NodeLit {
        name: node.name(),
        attrs: node.attributes().into_iter().collect(),
        text: node.text().unwrap_or_default(),
        children: node.children().iter().map(lit_down).collect(),
        parent: None,
    }
}

fn json_error(status: u16, message: &str) {
    web::send_json_error(status, message);
}

fn send_json(body: &serde_json::Value, extra: Vec<(String, String)>) {
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Cache-Control".to_string(), "private, no-store".to_string()),
    ];
    headers.extend(extra);
    web::send_bytes_with_headers(
        200,
        headers,
        serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec()),
    );
}

/// The context as plain JSON: an entity value is its map, a decimal a string, and what has no plain form (a find, a file) is left out.
fn unwrap_context() -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for (key, value) in script::context_combined() {
        if matches!(value, Value::Object(_))
            && script::value_map(&value).is_none()
            && script::value_items(&value).is_none()
        {
            continue;
        }
        out.insert(key, script::template::unwrap_json(&value));
    }
    out
}

fn unwrap_map(map: &IndexMap<String, Value>) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (key, value) in map {
        if matches!(value, Value::Object(_))
            && script::value_map(value).is_none()
            && script::value_items(value).is_none()
        {
            continue;
        }
        out.insert(key.clone(), script::template::unwrap_json(value));
    }
    serde_json::Value::Object(out)
}

/// The names of the forms that are lists, with the list each shows.
fn list_forms(state: &RenderState, def: &Arc<ScreenDef>) -> Vec<(String, Arc<form::FormInstance>)> {
    let mut out = Vec::new();
    for (name, definition) in &def.forms {
        if !definition.list || definition.dynamic {
            continue;
        }
        match form::assemble(&state.facade, &state.factory, def, name) {
            Ok(instance) => out.push((name.clone(), instance)),
            Err(err) => tracing::warn!("form {name} of {} could not be assembled: {err}", def.location),
        }
    }
    out
}

/// Run the data preparation of a screen as the render does, without writing anything: the root actions, then the actions of
/// the sections the render reaches. A section-iterate runs its actions for each entry, and the list is kept as those actions left it.
fn prepare_screen(state: &RenderState, def: &Arc<ScreenDef>) -> Result<(), Error> {
    if let Some(actions) = &def.root.actions {
        script::run_actions(&actions.xml)?;
    }
    if let Some(widgets) = &def.root.widgets {
        walk_widgets(state, def, &widgets.node()?, 0)?;
    }
    Ok(())
}

const MAX_WALK_DEPTH: usize = 12;

fn walk_widgets(state: &RenderState, owner: &Arc<ScreenDef>, node: &MNode, depth: usize) -> Result<(), Error> {
    if depth > MAX_WALK_DEPTH {
        return Err(Error::new(format!(
            "sections of {} are nested too deeply",
            owner.location
        )));
    }
    for child in node.children() {
        let name = child.name();
        match name.as_str() {
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
                    walk_widgets(state, &other, &widgets.node()?, depth + 1)?;
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

fn prepare_section(
    state: &RenderState,
    owner: &Arc<ScreenDef>,
    section: &SectionDef,
    depth: usize,
) -> Result<(), Error> {
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
        let count = items.len();
        for (index, (item_key, item)) in items.into_iter().enumerate() {
            let frame = ContextFrame::push();
            let outcome = (|| -> Result<Value, Error> {
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
                    walk_widgets(state, owner, &widgets.node()?, depth + 1)?;
                }
                Ok(script::context_value(entry).unwrap_or(item))
            })();
            drop(frame);
            kept.push(outcome?);
        }
        // The list is what the per-entry actions left, when the list is a name the package holds.
        if is_name(list) {
            script::set_context_name(list, Value::List(kept))?;
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
        walk_widgets(state, owner, &fragment.node()?, depth + 1)?;
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

/// The key under which the options of a widget are kept, which `ScreenQvtStatic.js` reads: the form, the field, the place of the
/// field's sub-node among its children and the place of the widget in the sub-node.
pub(crate) fn option_key(form: &str, field: &str, sub_index: usize, widget_index: usize) -> String {
    format!("mqOpt_{form}_{field}_{sub_index}_{widget_index}")
}

/// The key of the options of an editable select, by its id, which `ScreenQvtStatic.js` reads.
pub(crate) fn editable_option_key(id: &str) -> String {
    format!("mqOpt_editable_{id}")
}

/// The options of an `editable` select from its `list-options` and `entity-options`, as a drop-down's, under
/// [`editable_option_key`]: the package (or a form's extras) holds them, and the template adds them to its `option` elements.
/// None for another editable, one with no plain id, or a lookup that fails (logged).
fn editable_options(state: &RenderState, widget: &MNode) -> Option<(String, serde_json::Value)> {
    if widget.attribute("widget-type").as_deref() != Some("select")
        || (widget.first_named("list-options").is_none() && widget.first_named("entity-options").is_none())
    {
        return None;
    }
    let id = widget.attribute("id").unwrap_or_default();
    if id.is_empty() || id.contains("${") {
        tracing::warn!(
            "an editable select with list-options or entity-options needs a plain id in a server-static screen"
        );
        return None;
    }
    match lookup_options(state, widget) {
        Ok(options) => Some((editable_option_key(&id), options)),
        Err(err) => {
            tracing::warn!("options of editable {id}: {err}");
            None
        }
    }
}

pub(crate) fn is_lookup(widget: &MNode) -> bool {
    let dynamic = widget
        .first_named("dynamic-options")
        .is_some_and(|node| node.attribute("server-search").as_deref() != Some("true"));
    widget.first_named("entity-options").is_some() || widget.first_named("list-options").is_some() || dynamic
}

/// The options a widget gets from a lookup, as `{value, label}`: the `option` elements of the widget are in the template.
fn lookup_options(state: &RenderState, widget: &MNode) -> Result<serde_json::Value, Error> {
    let mut lit = lit_of(widget, true);
    lit.children.retain(|child| child.name != "option");
    let options = sri::get_field_options(state, &lit)?;
    Ok(serde_json::Value::Array(
        options
            .into_iter()
            .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
            .collect(),
    ))
}

pub(crate) fn sub_nodes(field: &MNode) -> impl Iterator<Item = (usize, MNode)> {
    field
        .children()
        .into_iter()
        .enumerate()
        .filter(|(_, child)| child.name().ends_with("-field"))
}

/// What a form-single needs that the browser cannot look up: the text of its `display-entity` fields as `{name}_display`, the options of
/// its drop-downs from lookups, and the fields its `set` elements make. They are kept under `mqExtra_{form}`.
fn single_form_extras(
    state: &RenderState,
    def: &Arc<ScreenDef>,
    name: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, Error> {
    let instance = form::assemble(&state.facade, &state.factory, def, name)?;
    let node = instance.node()?;
    let map_expression = node
        .attribute("map")
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "fieldValues".to_string());
    let map = script::eval_value(&map_expression).unwrap_or(Value::Null);
    let _frame = sri::single_form_map_frame(&map);
    let mut extras = serde_json::Map::new();
    for field in node.children_named("field") {
        let field_name = field.attribute("name").unwrap_or_default();
        for (sub_index, sub) in sub_nodes(&field) {
            let _sub_frame = ContextFrame::push();
            for set in sub.children_named("set") {
                let target = set.attribute("field").unwrap_or_default();
                let outcome = sri::set_in_context(&lit_of(&set, false));
                match outcome {
                    Ok(()) => {
                        if let Some(value) = script::context_value(&target) {
                            extras.insert(target, script::template::unwrap_json(&value));
                        }
                    }
                    Err(err) => tracing::warn!("set {target} in field {field_name} of {name}: {err}"),
                }
            }
            for (widget_index, widget) in sub.children().into_iter().enumerate() {
                collect_widget(state, &widget, name, &field_name, sub_index, widget_index, &mut extras);
            }
        }
    }
    Ok(extras)
}

/// Resolve one widget's lookups into `out`. A failed lookup is logged and the widget keeps no options.
fn collect_widget(
    state: &RenderState,
    widget: &MNode,
    form_name: &str,
    field_name: &str,
    sub_index: usize,
    widget_index: usize,
    out: &mut serde_json::Map<String, serde_json::Value>,
) {
    match widget.name().as_str() {
        "display-entity" => match sri::get_field_entity_value(&lit_of(widget, true)) {
            Ok(text) => {
                out.insert(format!("{field_name}_display"), serde_json::Value::String(text));
            }
            Err(err) => tracing::warn!("display-entity {field_name} of {form_name}: {err}"),
        },
        "link" if widget.attribute("entity-name").is_some_and(|text| !text.is_empty()) => {
            match sri::get_field_entity_value(&lit_of(widget, true)) {
                Ok(text) => {
                    out.insert(format!("{field_name}_display"), serde_json::Value::String(text));
                }
                Err(err) => tracing::warn!("link {field_name} of {form_name}: {err}"),
            }
        }
        "drop-down" | "check" | "radio" if is_lookup(widget) => match lookup_options(state, widget) {
            Ok(options) => {
                out.insert(option_key(form_name, field_name, sub_index, widget_index), options);
            }
            Err(err) => tracing::warn!("options of {field_name} in {form_name}: {err}"),
        },
        "editable" => {
            if let Some((key, options)) = editable_options(state, widget) {
                out.insert(key, options);
            }
        }
        _ => {}
    }
}

/// `GET {screen}/actions`.
fn screen_package(state: &RenderState, def: &Arc<ScreenDef>) -> Result<(), Error> {
    prepare_screen(state, def)?;
    let mut package = unwrap_context();
    for (name, definition) in &def.forms {
        if definition.list || definition.dynamic {
            continue;
        }
        match single_form_extras(state, def, name) {
            Ok(extras) if !extras.is_empty() => {
                package.insert(format!("mqExtra_{name}"), serde_json::Value::Object(extras));
            }
            Ok(_) => {}
            Err(err) => tracing::warn!("form {name} of {} has no extras: {err}", def.location),
        }
    }
    // A form-list is a request of its own, so its list and its page keys are not in this package.
    for (_, instance) in list_forms(state, def) {
        package.remove(&instance.list_name);
        for suffix in PAGE_SUFFIXES {
            package.remove(&format!("{}{suffix}", instance.list_name));
        }
    }
    // Request data, so it stays in the package. The template reads it only where an element asks for it.
    if wants_pass_through(state, def) {
        match super::urls::current_url(state) {
            Ok(url) => {
                let mut map = serde_json::Map::new();
                for (name, value) in url.pass_through_parameters() {
                    map.insert(name, serde_json::Value::String(value));
                }
                package.insert("mqPassThrough".to_string(), serde_json::Value::Object(map));
            }
            Err(err) => tracing::warn!("pass-through of {}: {err}", def.location),
        }
    }
    let mut body = serde_json::Value::Object(package);
    web::strip_credential_parameters(&mut body);
    send_json(&body, Vec::new());
    Ok(())
}

/// The response headers that carry the page keys, in the order of [`PAGE_SUFFIXES`].
const PAGE_HEADERS: [&str; 6] = [
    "X-Total-Count",
    "X-Page-Index",
    "X-Page-Size",
    "X-Page-Max-Index",
    "X-Page-Range-Low",
    "X-Page-Range-High",
];

/// The names a header field of a form-list reads from the request: its value and the find parameters that go with it.
pub(crate) const HEADER_SUFFIXES: [&str; 8] = ["", "_op", "_not", "_ic", "_from", "_thru", "_poffset", "_period"];

/// Whether the template has a form or a link with `pass-through-parameters="true"`, including an inlined include-screen.
/// A screen that does not use the attribute does not get the package key. A failed lookup is logged and skipped.
fn wants_pass_through(state: &RenderState, def: &Arc<ScreenDef>) -> bool {
    let mut seen = BTreeSet::new();
    match screen_wants_pass_through(state, def, 0, &mut seen) {
        Ok(found) => found,
        Err(err) => {
            tracing::warn!("pass-through of {}: {err}", def.location);
            false
        }
    }
}

fn screen_wants_pass_through(
    state: &RenderState,
    def: &Arc<ScreenDef>,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Result<bool, Error> {
    if depth > MAX_WALK_DEPTH || !seen.insert(def.location.clone()) {
        return Ok(false);
    }
    for definition in def.forms.values() {
        // The form element and each screen-extend of it. A parse failure is logged and that fragment is skipped.
        let mut nodes = Vec::new();
        match definition.node.node() {
            Ok(node) => nodes.push(node),
            Err(err) => tracing::warn!("pass-through form {}: {err}", definition.name),
        }
        for extension in &definition.extensions {
            match extension.node() {
                Ok(node) => nodes.push(node),
                Err(err) => tracing::warn!("pass-through form {} extension: {err}", definition.name),
            }
        }
        for node in &nodes {
            if node_wants_pass_through(state, node, depth, seen)? {
                return Ok(true);
            }
        }
    }
    if let Some(widgets) = &def.root.widgets {
        if node_wants_pass_through(state, &widgets.node()?, depth, seen)? {
            return Ok(true);
        }
    }
    for section in def.sections.values() {
        for fragment in [&section.widgets, &section.fail_widgets].into_iter().flatten() {
            if node_wants_pass_through(state, &fragment.node()?, depth, seen)? {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn node_wants_pass_through(
    state: &RenderState,
    node: &MNode,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Result<bool, Error> {
    if depth > MAX_WALK_DEPTH {
        return Ok(false);
    }
    if node.attribute("pass-through-parameters").as_deref() == Some("true") {
        return Ok(true);
    }
    if node.name() == "include-screen" {
        let location = node.attribute("location").unwrap_or_default();
        if !location.is_empty() && !location.contains("${") {
            match state.facade.screen_def(&state.factory, &location) {
                Ok(other) => {
                    if screen_wants_pass_through(state, &other, depth + 1, seen)? {
                        return Ok(true);
                    }
                }
                Err(err) => tracing::warn!("include-screen {location}: {err}"),
            }
        }
    }
    for child in node.children() {
        if node_wants_pass_through(state, &child, depth, seen)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `GET {screen}/actions/{formName}`.
fn form_package(state: &RenderState, def: &Arc<ScreenDef>, name: &str) -> Result<(), Error> {
    let Some(definition) = def.forms.get(name) else {
        if let Some(tree) = def.trees.get(name) {
            let nodes = super::tree::sub_node_json(state, tree, &def.location)?;
            web::send_json_response(nodes);
        } else {
            json_error(
                404,
                &format!("Could not find form or tree named {name} in screen {}", def.screen_name),
            );
        }
        return Ok(());
    };
    if !definition.list {
        json_error(
            400,
            &format!("form {name} in screen {} is not a form-list", def.screen_name),
        );
        return Ok(());
    }
    let instance = form::assemble(&state.facade, &state.factory, def, name)?;
    // A form without a find of its own shows a list the screen's actions prepare.
    if instance.entity_find.is_none() {
        prepare_screen(state, def)?;
    }
    let node = instance.node()?;
    let (handle, _) = sri::open_form_list(state, name)?;
    let mut rows: Vec<IndexMap<String, Value>> = state
        .form_lists
        .borrow()
        .get(handle)
        .and_then(Option::as_ref)
        .map(|list| list.rows.clone())
        .unwrap_or_default();
    let prefix = if instance.list_entry.is_empty() {
        format!("{}_entry", instance.list_name)
    } else {
        instance.list_entry.clone()
    };
    let mut resolved = Ok(());
    let sub_fields: Vec<String> = node
        .children_named("field")
        .iter()
        .filter(|field| field.attribute("aggregate").as_deref() == Some("sub-list"))
        .filter_map(|field| field.attribute("name"))
        .collect();
    for (index, row) in rows.iter_mut().enumerate() {
        let row_frame = sri::form_list_row_frame(state, handle, index)?;
        let extras = row_extras(&node, name, &|_| true);
        // the sub-rows of an aggregated row get the lookups and sets of the sub-list fields, each over the row
        if extras.is_ok() && !sub_fields.is_empty() {
            if let Some(Value::List(entries)) = row.get_mut("aggregateSubList") {
                for (sub, entry) in entries.iter_mut().enumerate() {
                    let sub_extras = {
                        let _sub_frame = sri::form_list_sub_row_frame(state, handle, index, sub)?;
                        row_extras(&node, name, &|field| {
                            sub_fields.contains(&field.attribute("name").unwrap_or_default())
                        })
                    };
                    if let (Value::Map(map), Ok(sub_extras)) = (entry, sub_extras) {
                        map.extend(sub_extras);
                    }
                }
            }
        }
        drop(row_frame);
        match extras {
            Ok(extras) => row.extend(extras),
            Err(err) => {
                resolved = Err(err);
                break;
            }
        }
        row.shift_remove(&prefix);
        row.shift_remove("_formMap");
    }
    sri::close_form_list(state, handle);
    resolved?;
    // The show-total row is not a row of the list: it is answered as {list}Total, which the static form-list draws after the rows.
    let total = rows
        .iter()
        .position(|row| row.get("_moquiRowType").and_then(Value::as_str) == Some("total"))
        .map(|index| rows.remove(index));

    let mut body = serde_json::Map::new();
    body.insert(
        instance.list_name.clone(),
        serde_json::Value::Array(rows.iter().map(unwrap_map).collect()),
    );
    if let Some(total) = &total {
        body.insert(format!("{}Total", instance.list_name), unwrap_map(total));
    }
    // The first, second and last rows: the map-*-row map (an empty one without), with the lookups and sets of the *-row-field
    // widgets over it, as Java pushes the map for those rows.
    for (kind, suffix) in [("first", "First"), ("second", "Second"), ("last", "Last")] {
        let tag = format!("{kind}-row-field");
        let has = node
            .children_named("field")
            .iter()
            .any(|field| field.first_named(&tag).is_some_and(|sub| !sub.children().is_empty()));
        if !has {
            continue;
        }
        let expression = node.attribute(&format!("map-{kind}-row")).unwrap_or_default();
        let map = if expression.trim().is_empty() {
            Value::Map(IndexMap::new())
        } else {
            script::eval_value(&expression)?
        };
        let extras = {
            let _frame = sri::single_form_map_frame(&map);
            row_extras_of(&node, name, &|_| true, &[tag.as_str()])
        };
        let mut fields = script::value_map(&map).unwrap_or_default();
        fields.extend(extras?);
        body.insert(format!("{}{suffix}", instance.list_name), unwrap_map(&fields));
    }
    let mut headers = Vec::new();
    if let Some(pages) = page_keys(&node, rows.len())? {
        for (suffix, header, number) in pages.numbers() {
            body.insert(
                format!("{}{suffix}", instance.list_name),
                serde_json::Value::from(number),
            );
            headers.push((header.to_string(), number.to_string()));
        }
    } else if script::context_value(&format!("{}Count", instance.list_name)).is_some() {
        // A list the screen's actions prepared and paginated by themselves has the keys in the context.
        for (suffix, header) in PAGE_SUFFIXES.iter().zip(PAGE_HEADERS) {
            let key = format!("{}{suffix}", instance.list_name);
            if let Some(value) = script::context_value(&key) {
                body.insert(key, script::template::unwrap_json(&value));
                headers.push((header.to_string(), moqui_lib::object::to_plain_string(&value)));
            }
        }
    }
    // The values the header fields have from the request, so the find form starts with what the user searched for.
    for field in node.children_named("field") {
        let field_name = field.attribute("name").unwrap_or_default();
        if field.first_named("header-field").is_none() {
            continue;
        }
        for suffix in HEADER_SUFFIXES {
            let key = format!("{field_name}{suffix}");
            if let Some(value) = script::context_value(&key).filter(|value| !value.is_null()) {
                body.insert(key, script::template::unwrap_json(&value));
            }
        }
    }
    // The columns the user chose for the form, as field names, when the column picker is on; without them the template's own columns hold.
    if node.attribute("select-columns").as_deref() == Some("true") {
        let location = super::saved_finds::form_location(&def.location, true, name);
        let source = definition.node.node()?;
        let saved = super::saved_finds::saved_find_location(&location, definition.dynamic, &source);
        if let Some(columns) = super::saved_finds::user_columns(&location, &saved)? {
            body.insert("mqColumns".to_string(), serde_json::json!(columns));
        }
    }
    // The user's saved finds of the form and the screen's default one, for the find dialog.
    if node.attribute("saved-finds").as_deref() == Some("true") {
        let location = super::saved_finds::form_location(&def.location, true, name);
        let source = definition.node.node()?;
        let finds = super::saved_finds::user_form_list_finds(&super::saved_finds::saved_find_location(
            &location,
            definition.dynamic,
            &source,
        ))?;
        let listed: Vec<serde_json::Value> = finds.iter().map(saved_find_json).collect();
        // The schedule of the active find, when it is one of these: the dialog shows it, or offers to schedule the find.
        let active = script::context_value("formListFindId")
            .map(|value| moqui_lib::object::to_plain_string(&value))
            .filter(|id| listed.iter().any(|find| find["formListFindId"] == id.as_str()));
        if let Some(scheduled) = active
            .map(|id| super::saved_finds::form_list_find_screen_scheduled(&id))
            .transpose()?
            .flatten()
        {
            let field = |name: &str| match &scheduled {
                Value::Map(map) => map
                    .get(name)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let mode = match field("renderMode").as_str() {
                "xsl-fo" => "PDF".to_string(),
                "" => "CSV".to_string(),
                other => other.to_uppercase(),
            };
            body.insert(
                "formListFindScheduled".to_string(),
                serde_json::json!({ "mode": mode, "cronDescription": field("cronDescription") }),
            );
        }
        body.insert("formListFinds".to_string(), serde_json::Value::Array(listed));
        if let Some(id) = super::saved_finds::user_default_find_id(&def.location)? {
            body.insert("defaultFormListFindId".to_string(), serde_json::Value::String(id));
        }
    }
    for key in ["orderByField", "pageIndex", "pageSize", "formListFindId"] {
        if let Some(value) = script::context_value(key).filter(|value| !value.is_null()) {
            body.entry(key.to_string())
                .or_insert_with(|| script::template::unwrap_json(&value));
        }
    }
    // Lookups of the widgets in the columns, in the scope of the screen.
    for field in node.children_named("field") {
        let field_name = field.attribute("name").unwrap_or_default();
        for (sub_index, sub) in sub_nodes(&field) {
            for (widget_index, widget) in sub.children().into_iter().enumerate() {
                if matches!(widget.name().as_str(), "drop-down" | "check" | "radio") && is_lookup(&widget) {
                    collect_widget(state, &widget, name, &field_name, sub_index, widget_index, &mut body);
                }
            }
        }
    }
    let mut body = serde_json::Value::Object(body);
    web::strip_credential_parameters(&mut body);
    send_json(&body, headers);
    Ok(())
}

/// One saved find as the find dialog lists it: its id, description, and whether it is the user's own (which the user may
/// update and delete) rather than a group's.
fn saved_find_json(find: &Value) -> serde_json::Value {
    let read = |path: &[&str]| -> String {
        let mut current = Some(find.clone());
        for key in path {
            current = match current {
                Some(Value::Map(map)) => map.get(*key).cloned(),
                _ => None,
            };
        }
        current
            .map(|value| moqui_lib::object::to_plain_string(&value))
            .unwrap_or_default()
    };
    serde_json::json!({
        "formListFindId": read(&["formListFind", "formListFindId"]),
        "description": read(&["description"]),
        "isByUserId": read(&["isByUserId"]),
    })
}

/// The page keys the renderer sets for a list with `search-form-inputs` that paginates: as `ScreenForm` does for a form-list's own `entity-find`.
struct PageKeys {
    count: i64,
    index: i64,
    size: i64,
}

impl PageKeys {
    fn max_index(&self) -> i64 {
        if self.size > 0 && self.count > 0 {
            (self.count - 1) / self.size
        } else {
            0
        }
    }

    fn numbers(&self) -> [(&'static str, &'static str, i64); 6] {
        let low = self.index * self.size + 1;
        let high = (self.index * self.size + self.size).min(self.count);
        [
            ("Count", "X-Total-Count", self.count),
            ("PageIndex", "X-Page-Index", self.index),
            ("PageSize", "X-Page-Size", self.size),
            ("PageMaxIndex", "X-Page-Max-Index", self.max_index()),
            ("PageRangeLow", "X-Page-Range-Low", low),
            ("PageRangeHigh", "X-Page-Range-High", high),
        ]
    }
}

const DEFAULT_PAGE_SIZE: i64 = 20;

fn whole(name: &str) -> Option<i64> {
    script::context_value(name).and_then(|value| moqui_lib::object::to_plain_string(&value).trim().parse().ok())
}

/// `None` when the form's find does not paginate. The total is the rows of a full page and more, else the rows of this page and those before it;
/// only a full page asks the database for the count, with the conditions of the form's find.
fn page_keys(form: &MNode, rows: usize) -> Result<Option<PageKeys>, Error> {
    let Some(find) = form.first_named("entity-find") else {
        return Ok(None);
    };
    let paginate = find
        .first_named("search-form-inputs")
        .is_some_and(|inputs| inputs.attribute("paginate").as_deref() != Some("false"));
    if !paginate {
        return Ok(None);
    }
    let index = whole("pageIndex").unwrap_or(0).max(0);
    let size = whole("pageSize").filter(|size| *size > 0).unwrap_or(DEFAULT_PAGE_SIZE);
    let rows = i64::try_from(rows).unwrap_or(i64::MAX);
    let count = if rows < size {
        rows + index * size
    } else {
        let counter = find.deep_copy();
        counter.set_name("entity-find-count");
        counter.set_attribute("count-field", "mqFormCount");
        counter.remove_where(|child| {
            matches!(
                child.name().as_str(),
                "order-by" | "select-field" | "limit-range" | "limit-view" | "use-iterator"
            )
        });
        let actions = MNode::new("actions");
        actions.append_node(counter);
        let (ran, counted) = {
            let _frame = ContextFrame::push();
            (
                script::run_actions(&actions.write_string()),
                script::context_value("mqFormCount"),
            )
        };
        ran?;
        counted
            .and_then(|value| moqui_lib::object::to_plain_string(&value).parse().ok())
            .unwrap_or(rows)
    };
    Ok(Some(PageKeys { count, index, size }))
}

/// For one row, in the row's frame: the text of its `display-entity` fields and the fields its `set` elements make.
fn row_extras(
    node: &MNode,
    form_name: &str,
    include: &dyn Fn(&MNode) -> bool,
) -> Result<IndexMap<String, Value>, Error> {
    row_extras_of(node, form_name, include, &["default-field", "conditional-field"])
}

/// [`row_extras`] for the widgets of the named sub-nodes of each field.
fn row_extras_of(
    node: &MNode,
    form_name: &str,
    include: &dyn Fn(&MNode) -> bool,
    subs: &[&str],
) -> Result<IndexMap<String, Value>, Error> {
    let mut extras = IndexMap::new();
    for field in node.children_named("field").into_iter().filter(|field| include(field)) {
        let field_name = field.attribute("name").unwrap_or_default();
        for sub in field
            .children()
            .into_iter()
            .filter(|sub| subs.contains(&sub.name().as_str()))
        {
            for set in sub.children_named("set") {
                let target = set.attribute("field").unwrap_or_default();
                sri::set_in_context(&lit_of(&set, false))?;
                if let Some(value) = script::context_value(&target) {
                    extras.insert(target, value);
                }
            }
            for widget in sub.children() {
                let entity_text = widget.name() == "display-entity"
                    || (widget.name() == "link"
                        && widget.attribute("entity-name").is_some_and(|text| !text.is_empty()));
                if entity_text {
                    match sri::get_field_entity_value(&lit_of(&widget, true)) {
                        Ok(text) => {
                            extras.insert(format!("{field_name}_display"), Value::String(text));
                        }
                        Err(err) => tracing::warn!("{} {field_name} of {form_name}: {err}", widget.name()),
                    }
                }
            }
        }
    }
    Ok(extras)
}

/// The `actions` transition: the pre-actions of the path (the always-actions have run), then the data of the screen or of one form.
///
/// # Errors
///
/// Returns the error an action raised.
pub(crate) fn run_actions_transition(state: &RenderState) -> Result<(), Error> {
    let info = &state.url_info;
    let defs = &info.path_defs;
    let last = defs.len().saturating_sub(1);
    // The mode is always standalone, so the pre-actions of the screens from the third on run, as for a qvt render.
    let first_pre = if state.facade.is_always_standalone("qvt") && defs.len() > 2 {
        2
    } else {
        info.render_path_difference
    };
    // As a render does first: the user's default saved find, or the one the request names, and the saved page size.
    let target_parameters: Vec<String> = info
        .target_screen
        .parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    super::saved_finds::apply_on_render(&info.target_screen.location, &target_parameters, true)?;
    for (index, def) in defs.iter().enumerate() {
        if index >= first_pre {
            state.screen_path_index.set(index);
            if let Some(actions) = &def.pre_actions {
                script::run_actions(&actions.xml)?;
            }
        }
    }
    state.screen_path_index.set(last);
    let target = Arc::clone(&info.target_screen);
    for parameter in target.parameters.iter().filter(|parameter| parameter.required) {
        if super::render::is_empty(script::context_value(&parameter.name).as_ref()) {
            json_error(400, &format!("Required parameter missing ({})", parameter.name));
            return Ok(());
        }
    }
    match info.extra_path.first() {
        None => screen_package(state, &target),
        Some(name) => form_package(state, &target, name),
    }
}

// ---- includes ----

/// `section-include` and `include-screen` written into the template: the included widgets take the place of the element, with the forms of the
/// screen they came from assembled. The included screen's own actions run when the data is made (`walk_widgets`).
pub(crate) fn inline_include(request: &UnitRequest<'_>, node: &MNode) -> Result<MNode, Error> {
    expand_include(request.facade, request.factory, request.def, node, 0)
}

fn expand_include(
    facade: &super::ScreenFacade,
    factory: &ExecutionContextFactory,
    def: &Arc<ScreenDef>,
    node: &MNode,
    depth: usize,
) -> Result<MNode, Error> {
    if depth > MAX_WALK_DEPTH {
        return Err(Error::new(format!(
            "includes in {} are nested too deeply",
            def.location
        )));
    }
    let location = node.attribute("location").unwrap_or_default();
    let out;
    let owner;
    if node.name() == "include-screen" {
        if location.contains("${") {
            return Ok(node.deep_copy());
        }
        let other = facade.screen_def(factory, &location)?;
        out = MNode::new("section");
        out.set_attribute("name", format!("include:{location}"));
        if let Some(widgets) = &other.root.widgets {
            out.append_node(widgets.node()?);
        }
        owner = other;
    } else {
        let given = node.attribute("name").unwrap_or_default();
        if location.contains("${") || given.contains("${") {
            return Ok(node.deep_copy());
        }
        let section_name = if given.is_empty() {
            location
                .split_once('#')
                .map_or(String::new(), |(_, name)| name.to_string())
        } else {
            given
        };
        let section = def.sections.get(&section_name).ok_or_else(|| {
            Error::new(format!(
                "No section with name [{section_name}] in screen [{}]",
                def.location
            ))
        })?;
        out = section_node(section)?;
        owner = Arc::clone(def);
    }
    inline_children(facade, factory, &owner, &out, depth + 1)?;
    Ok(out)
}

/// A section as the element it was written as.
fn section_node(section: &SectionDef) -> Result<MNode, Error> {
    let out = match &section.kind {
        SectionKind::Iterate {
            list,
            entry,
            key,
            paginate,
        } => {
            let out = MNode::new("section-iterate");
            out.set_attribute("list", list.clone());
            out.set_attribute("entry", entry.clone());
            if let Some(key) = key {
                out.set_attribute("key", key.clone());
            }
            out.set_attribute("paginate", paginate.to_string());
            out
        }
        _ => MNode::new("section"),
    };
    out.set_attribute("name", section.name.clone());
    match &section.condition {
        Some(Condition::Expression(expression)) => out.set_attribute("condition", expression.clone()),
        // The XML condition is kept inside the actions it was wrapped in: <actions><if><condition>...</condition>.
        Some(Condition::Wrapped(actions)) => {
            let wrapped = MNode::parse_text("condition", &actions.xml)?;
            let condition = wrapped
                .first_named("if")
                .and_then(|found| found.first_named("condition"))
                .map(|found| found.deep_copy());
            match condition {
                Some(condition) => out.append_node(condition),
                None => {
                    out.append("condition").append("wrapped");
                }
            }
        }
        None => {}
    }
    if let Some(widgets) = &section.widgets {
        out.append_node(widgets.node()?);
    }
    if let Some(fail) = &section.fail_widgets {
        out.append_node(fail.node()?);
    }
    Ok(out)
}

fn inline_children(
    facade: &super::ScreenFacade,
    factory: &ExecutionContextFactory,
    owner: &Arc<ScreenDef>,
    node: &MNode,
    depth: usize,
) -> Result<(), Error> {
    for (index, child) in node.children().into_iter().enumerate() {
        match child.name().as_str() {
            "section-include" | "include-screen" => {
                let expanded = expand_include(facade, factory, owner, &child, depth)?;
                node.replace_child(index, expanded)?;
            }
            "form-single" | "form-list" => {
                let name = child.attribute("name").unwrap_or_default();
                if owner.forms.get(&name).is_some_and(|definition| !definition.dynamic) {
                    let assembled = form::assemble(facade, factory, owner, &name)?.node()?;
                    assembled.set_attribute("mq-assembled", "true");
                    node.replace_child(index, assembled)?;
                }
            }
            _ => inline_children(facade, factory, owner, &child, depth)?,
        }
    }
    Ok(())
}

// ---- the compatibility report ----

mod report;
pub(crate) use report::{
    record_ast, record_note, record_wrapped, report_active, report_failure, report_forces_static, report_ineligible,
    report_template, set_expression,
};
pub use report::{
    static_report, static_reports_for_files, FileReport, ReportGuard, StaticReport, Violation, WidgetCheck,
};
