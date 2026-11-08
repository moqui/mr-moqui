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

//! Screen render code: the macro modules for a render mode and the generated function for a section.
//!
//! A section's widgets (or fail-widgets) are walked once per render mode. The result is a
//! JavaScript function that runs for each request on the server runtime, calls `sri` and `ec`, and
//! writes through `sri.write`. It is kept with the screen definition, so it lives until the screen
//! file changes. Set `MOQUI_ACTIONS_DEBUG` to put a comment naming the element before each statement.
//!
//! The macro modules are embedded in the binary. A `macro-template-location` that is not the
//! built-in path and names a file that exists replaces the embedded copy of that mode, which is
//! how a theme overrides a mode. `ScreenSupport.js` is never replaced.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use moqui_lib::node::MNode;
use moqui_lib::sync::lock;

use super::cache::ScreenFacade;
use super::definition::ScreenDef;
use super::form::{self, FormInstance};
use super::statics::{Eligibility, StaticTemplate};
use crate::context::{Error, ExecutionContextFactory};
use crate::script::walker::{self, Resolve};

const TEMPLATE_SUPPORT: &str = include_str!("../../../../runtime/base-component/framework/template/TemplateSupport.js");
const SCREEN_SUPPORT: &str =
    include_str!("../../../../runtime/base-component/framework/template/screen/ScreenSupport.js");

/// The mode modules built into the binary, by file name.
const MODE_MODULES: [(&str, &str); 10] = [
    (
        "ScreenText.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenText.js"),
    ),
    (
        "ScreenCsv.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenCsv.js"),
    ),
    (
        "ScreenXml.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenXml.js"),
    ),
    (
        "ScreenHtmlPrint.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenHtmlPrint.js"),
    ),
    (
        "ScreenHtml.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenHtml.js"),
    ),
    (
        "ScreenQvt.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenQvt.js"),
    ),
    (
        "ScreenQvtStatic.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenQvtStatic.js"),
    ),
    (
        "ScreenPlain.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenPlain.js"),
    ),
    (
        "ScreenXlsx.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenXlsx.js"),
    ),
    (
        "ScreenTui.js",
        include_str!("../../../../runtime/base-component/framework/template/screen/ScreenTui.js"),
    ),
];

const BUILT_IN_DIR: &str = "framework/template/screen/";
const MAX_STATIC_TEMPLATES: usize = 64;
/// Units kept for dynamic and user-column forms of one definition, one for each different assembled form.
const MAX_DYNAMIC_UNITS: usize = 128;
const DEBUG_ENV: &str = "MOQUI_ACTIONS_DEBUG";

static NEXT_UNIT: AtomicU64 = AtomicU64::new(1);

/// The generated function for one section in one render mode.
#[derive(Debug)]
pub struct Unit {
    /// Distinguishes this generation from any other, so a runtime can keep the compiled function.
    pub id: u64,
    /// A function expression: the mode's runtime prelude and the body.
    pub source: String,
    /// Code the mode shares between its units, run once for each engine before the first unit that names it.
    pub runtime: Arc<str>,
    pub runtime_id: u64,
}

/// What a definition keeps for rendering: generated units and assembled forms.
pub struct Generated {
    /// By section and mode, so bounded by the definition.
    units: Mutex<HashMap<String, Option<Arc<Unit>>>>,
    /// By the text of an assembled dynamic or user-column form, which varies with the context and user.
    dynamic_units: Mutex<crate::script::BoundedMap<String, Arc<Unit>>>,
    forms: Mutex<HashMap<String, Arc<FormInstance>>>,
    /// The server-static templates by request path, and whether the screen can be one, by macro module.
    statics: Mutex<crate::script::BoundedMap<String, Arc<StaticTemplate>>>,
    eligibility: Mutex<HashMap<String, Arc<Eligibility>>>,
    /// Files read while rendering, such as a form another screen's form extends. A change to one makes the definition stale.
    dependencies: Mutex<Vec<super::definition::Source>>,
    /// Why a server-static template could not be made, by request path, so it is not tried again until the screen changes.
    static_failures: Mutex<HashMap<String, String>>,
    /// Held while a server-static template is made, so concurrent first requests make it once.
    static_build: Mutex<()>,
}

impl Default for Generated {
    fn default() -> Self {
        Self {
            units: Mutex::default(),
            dynamic_units: Mutex::new(crate::script::BoundedMap::new(MAX_DYNAMIC_UNITS)),
            forms: Mutex::default(),
            statics: Mutex::new(crate::script::BoundedMap::new(MAX_STATIC_TEMPLATES)),
            eligibility: Mutex::default(),
            dependencies: Mutex::default(),
            static_failures: Mutex::default(),
            static_build: Mutex::default(),
        }
    }
}

impl std::fmt::Debug for Generated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Generated")
    }
}

impl Generated {
    pub(super) fn dependencies(&self) -> Vec<super::definition::Source> {
        lock(&self.dependencies).clone()
    }

    /// Record the sources of another definition this one's generated code was made from.
    pub(super) fn add_dependencies(&self, sources: &[super::definition::Source]) {
        let mut dependencies = lock(&self.dependencies);
        for source in sources {
            if !dependencies.iter().any(|known| known.location == source.location) {
                dependencies.push(source.clone());
            }
        }
    }

    pub(super) fn form(&self, name: &str) -> Option<Arc<FormInstance>> {
        lock(&self.forms).get(name).cloned()
    }

    pub(super) fn put_form(&self, name: &str, form: Arc<FormInstance>) {
        lock(&self.forms).insert(name.to_string(), form);
    }

    pub(super) fn static_template(&self, key: &str) -> Option<Arc<StaticTemplate>> {
        lock(&self.statics).get(key)
    }

    pub(super) fn put_static_template(&self, key: String, template: Arc<StaticTemplate>) {
        // One for each path the screen was requested by: many odd paths for one screen do not grow without end.
        lock(&self.statics).insert(key, template);
    }

    pub(super) fn static_failure(&self, key: &str) -> Option<String> {
        lock(&self.static_failures).get(key).cloned()
    }

    pub(super) fn put_static_failure(&self, key: String, reason: String) {
        let mut failures = lock(&self.static_failures);
        if failures.len() < MAX_STATIC_TEMPLATES {
            failures.insert(key, reason);
        }
    }

    pub(super) fn static_build_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        lock(&self.static_build)
    }

    pub(super) fn eligibility(&self, key: &str) -> Option<Arc<Eligibility>> {
        lock(&self.eligibility).get(key).cloned()
    }

    pub(super) fn put_eligibility(&self, key: String, value: Arc<Eligibility>) {
        lock(&self.eligibility).insert(key, value);
    }

    fn unit(&self, key: &str) -> Option<Option<Arc<Unit>>> {
        lock(&self.units).get(key).cloned()
    }

    fn put_unit(&self, key: String, unit: Option<Arc<Unit>>) {
        lock(&self.units).insert(key, unit);
    }

    fn dynamic_unit(&self, key: &str) -> Option<Arc<Unit>> {
        lock(&self.dynamic_units).get(key)
    }

    fn put_dynamic_unit(&self, key: String, unit: Arc<Unit>) {
        lock(&self.dynamic_units).insert(key, unit);
    }

    /// The number of units generated, for tests.
    #[cfg(test)]
    pub fn unit_count(&self) -> usize {
        lock(&self.units).len() + self.dynamic_unit_count()
    }

    /// The number of dynamic form units kept, for tests.
    #[cfg(test)]
    pub fn dynamic_unit_count(&self) -> usize {
        lock(&self.dynamic_units).len()
    }
}

/// Which section's code to make.
pub struct UnitRequest<'a> {
    pub factory: &'a ExecutionContextFactory,
    pub facade: &'a ScreenFacade,
    pub def: &'a Arc<ScreenDef>,
    /// The section name. Empty is the screen's own root section.
    pub section: &'a str,
    pub fail: bool,
    pub mode: &'a str,
    pub macro_location: &'a str,
}

/// The unit for a section, from the definition's cache when it was made before. None when the
/// section has no widgets (or no fail-widgets).
///
/// # Errors
///
/// Returns an error when the module cannot be loaded, an element has no macro, a macro throws, or
/// the generated code does not parse.
///
/// Generation runs on the thread's quickjs-oxide generator (`script::oxide::walker`).
pub fn unit_for(request: &UnitRequest<'_>) -> Result<Option<Arc<Unit>>, Error> {
    let key = format!(
        "{}|{}|{}|{}",
        request.mode,
        request.macro_location,
        request.section,
        if request.fail { "fail" } else { "widgets" }
    );
    if let Some(found) = request.def.generated.unit(&key) {
        return Ok(found);
    }
    let made = match unit_root(request.def, request.section, request.fail)? {
        Some(root) => match mode_source(request.factory, request.macro_location) {
            Ok(source) => {
                let loaded = load_module(&source, request.mode, std::env::var_os(DEBUG_ENV).is_some())?;
                let resolve = |node: &MNode| -> Result<Option<MNode>, Error> {
                    let name = node.name();
                    if request.mode == super::statics::UNIT_MODE
                        && (name == "section-include" || name == "include-screen")
                    {
                        return super::statics::inline_include(request, node).map(Some);
                    }
                    if name != "form-single" && name != "form-list" {
                        return Ok(None);
                    }
                    if node.has_attribute("mq-assembled") {
                        return Ok(None);
                    }
                    let form_name = node.attribute("name").unwrap_or_default();
                    if request.def.forms.get(&form_name).is_some_and(|form| form.dynamic)
                        || form::has_db_extensions(&request.def.location, &form_name)
                        || form::extends_db_form(request.def, &form_name)
                    {
                        // Its fields depend on the context or on the user's DbForms, so the call that renders it
                        // assembles it then.
                        let call = MNode::new("render-dynamic-form");
                        call.set_attribute("name", form_name);
                        return Ok(Some(call));
                    }
                    let assembled = form::assemble(request.facade, request.factory, request.def, &form_name)?;
                    let node = assembled.node()?;
                    // The user may have chosen this form-list's columns: the render asks, and runs this code only when not.
                    if name == "form-list"
                        && request.mode != super::statics::UNIT_MODE
                        && (node.attribute_is("select-columns", "true") || node.attribute_is("saved-finds", "true"))
                    {
                        node.set_attribute("mq-assembled", "true");
                        let wrapper = MNode::new("form-list-user-columns");
                        wrapper.set_attribute("name", form_name);
                        wrapper.append_node(node);
                        return Ok(Some(wrapper));
                    }
                    Ok(Some(node))
                };
                let body = generate_unit_body(&loaded, &root, Some(&resolve), request.mode).map_err(|err| {
                    Error::new(format!(
                        "{} {}: {}",
                        request.def.location,
                        describe(request),
                        err.message()
                    ))
                })?;
                Some(Arc::new(make_unit(&loaded, body)?))
            }
            // A mode with no module yet renders nothing for widgets with nothing in them.
            Err(_) if is_empty_root(&root) => None,
            Err(err) => {
                return Err(Error::new(format!(
                    "{} {}: {}",
                    request.def.location,
                    describe(request),
                    err.message()
                )))
            }
        },
        None => None,
    };
    request.def.generated.put_unit(key, made.clone());
    Ok(made)
}

/// The unit for a dynamic form as it was just assembled. The code is kept under the text of the
/// assembled form, so a form that comes out the same each time is generated once.
///
/// # Errors
///
/// Returns an error when the module cannot be loaded or the form's code cannot be generated.
pub fn unit_for_dynamic_form(
    request: &UnitRequest<'_>,
    form: &FormInstance,
) -> Result<Arc<Unit>, Error> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    form.xml.hash(&mut hasher);
    let key = format!(
        "dynamic|{}|{}|{}|{:x}",
        request.mode,
        request.macro_location,
        form.name,
        hasher.finish()
    );
    if let Some(found) = request.def.generated.dynamic_unit(&key) {
        return Ok(found);
    }
    let source = mode_source(request.factory, request.macro_location)?;
    let loaded = load_module(&source, request.mode, std::env::var_os(DEBUG_ENV).is_some())?;
    let root = MNode::new("dynamic-form");
    root.append_node(form.node()?);
    let body = generate_unit_body(&loaded, &root, None, request.mode).map_err(|err| {
        Error::new(format!(
            "{} dynamic form {}: {}",
            request.def.location,
            form.name,
            err.message()
        ))
    })?;
    let unit = Arc::new(make_unit(&loaded, body)?);
    request.def.generated.put_dynamic_unit(key, Arc::clone(&unit));
    Ok(unit)
}

fn is_empty_root(root: &MNode) -> bool {
    match root.first_named("widgets") {
        Some(widgets) => widgets.children().is_empty(),
        None => root.children().is_empty(),
    }
}

fn describe(request: &UnitRequest<'_>) -> String {
    let which = if request.fail { "fail-widgets" } else { "widgets" };
    if request.section.is_empty() {
        format!("{which} ({})", request.mode)
    } else {
        format!("section {} {which} ({})", request.section, request.mode)
    }
}

/// A unit's code and the element literals it refers to as `$lits[i]`.
struct UnitBody {
    code: String,
    lits: Vec<String>,
}

fn make_unit(module: &ModeModule, body: UnitBody) -> Result<Unit, Error> {
    let UnitBody { code: body, lits } = body;
    let function = format!(
        "function ($depth) {{\n\"use strict\";\n{}{body}{}}}",
        module.prelude, module.epilogue
    );
    // The literals are made, converted and frozen once, when an engine compiles the unit.
    let source = if lits.is_empty() {
        format!("({function})")
    } else {
        format!(
            "(function () {{\n\"use strict\";\nconst $lits = sri.holdNodeLits([\n{}\n]);\nreturn {function};\n}})()",
            lits.join(",\n")
        )
    };
    crate::script::check_program(&source).map_err(|err| Error::new(format!("{}\n{source}", err.message())))?;
    Ok(Unit {
        id: NEXT_UNIT.fetch_add(1, Ordering::Relaxed),
        source,
        runtime: Arc::clone(&module.runtime),
        runtime_id: module.runtime_id,
    })
}

/// The tree to walk for a section: the section's widgets, or for the root section a `screen`
/// element around them, which the xml mode reads to name its root element.
fn unit_root(def: &ScreenDef, section: &str, fail: bool) -> Result<Option<MNode>, Error> {
    let fragment = if section.is_empty() {
        if fail {
            def.root.fail_widgets.as_ref()
        } else {
            def.root.widgets.as_ref()
        }
    } else {
        let found = def
            .sections
            .get(section)
            .ok_or_else(|| Error::new(format!("No section with name [{section}] in screen [{}]", def.location)))?;
        if fail {
            found.fail_widgets.as_ref()
        } else {
            found.widgets.as_ref()
        }
    };
    let Some(fragment) = fragment else {
        return Ok(None);
    };
    let widgets = fragment.node()?;
    if section.is_empty() && !fail {
        let screen = MNode::new("screen");
        screen.append_node(widgets);
        Ok(Some(screen))
    } else {
        Ok(Some(widgets))
    }
}

/// A loaded mode: the macro table, the elements it treats as opaque, and its runtime prelude and epilogue (the code
/// every unit starts and ends with). A unit is a function of `$depth`, how many units it runs inside (0 for the
/// outermost); a mode keeps the render state of the unit around it in its prelude and gives it back in the epilogue, as
/// a `try` and its `finally`, so a section rendered inside a form-list cell leaves the list's state as it found it.
/// Loaded on the thread's quickjs-oxide generator (`script::oxide::walker`).
pub struct ModeModule {
    module: crate::script::oxide::walker::ScreenModule,
    prelude: String,
    epilogue: String,
    runtime: Arc<str>,
    runtime_id: u64,
}

impl ModeModule {
    fn opaque(&self) -> &HashSet<String> {
        &self.module.opaque
    }
}

fn runtime_id(runtime: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    runtime.hash(&mut hasher);
    hasher.finish()
}

/// The module `source` for `mode` on the thread's quickjs-oxide generator.
fn load_module(source: &str, mode: &str, debug: bool) -> Result<ModeModule, Error> {
    let module = crate::script::oxide::walker::screen_module(TEMPLATE_SUPPORT, SCREEN_SUPPORT, source, mode, debug)?;
    Ok(ModeModule {
        prelude: module.prelude.clone(),
        epilogue: module.epilogue.clone(),
        runtime_id: runtime_id(&module.runtime),
        runtime: Arc::from(module.runtime.as_str()),
        module,
    })
}

/// `body` with the module `source` for `mode` loaded.
fn with_module<R>(
    source: &str,
    mode: &str,
    debug: bool,
    body: impl FnOnce(&ModeModule) -> Result<R, Error>,
) -> Result<R, Error> {
    body(&load_module(source, mode, debug)?)
}

fn mode_source(factory: &ExecutionContextFactory, location: &str) -> Result<String, Error> {
    let file = location.rsplit('/').next().unwrap_or(location);
    let built_in = format!("{BUILT_IN_DIR}{file}");
    if location != built_in {
        if let Ok(resource) = factory.resource().get(location) {
            if resource.exists() && resource.is_file() {
                return resource.read_text();
            }
        }
    }
    MODE_MODULES
        .iter()
        .find(|(name, _)| *name == file)
        .map(|(_, source)| (*source).to_string())
        .ok_or_else(|| Error::new(format!("there is no macro module {file} at {location}")))
}

/// [`generate_body`] for a unit: the element literals are collected for [`make_unit`], so each is made once.
fn generate_unit_body(
    module: &ModeModule,
    root: &MNode,
    resolve: Option<Resolve<'_>>,
    mode: &str,
) -> Result<UnitBody, Error> {
    // the tree is built (forms resolved) before the collection starts
    let tree = walker::tree_json(root, module.opaque(), resolve)?;
    let (code, lits) = module.module.generate(&tree, mode, true)?;
    Ok(UnitBody { code, lits })
}

#[cfg(test)]
fn generate_body(module: &ModeModule, root: &MNode, resolve: Option<Resolve<'_>>, mode: &str) -> Result<String, Error> {
    let tree = walker::tree_json(root, module.opaque(), resolve)?;
    Ok(module.module.generate(&tree, mode, false)?.0)
}

/// The elements the built-in html module names, and those whose subtree it reads for itself, for `moqui lint
/// --screens`: an element of a screen's widgets outside the first set renders as an unknown element.
///
/// # Errors
///
/// Returns an error when the module does not load.
pub fn widget_vocabulary() -> Result<(HashSet<String>, HashSet<String>), Error> {
    let source = MODE_MODULES
        .iter()
        .find(|(name, _)| *name == "ScreenHtml.js")
        .map(|(_, source)| *source)
        .ok_or_else(|| Error::new("missing built-in module"))?;
    with_module(source, "html", false, |module| {
        let names = module.module.macro_names()?;
        Ok((names, module.opaque().clone()))
    })
}

/// Generate the body for a tree of widgets with the built-in module for `mode`, forms as given. For
/// tests: it reads no screen file and caches nothing.
///
/// # Errors
///
/// Returns an error when the XML does not parse or generate.
#[cfg(test)]
pub fn generate_xml(mode: &str, xml: &str, debug_comments: bool) -> Result<String, Error> {
    let file = match mode {
        "text" => "ScreenText.js",
        "csv" => "ScreenCsv.js",
        "xlsx" => "ScreenXlsx.js",
        "xml" => "ScreenXml.js",
        "html-print" => "ScreenHtmlPrint.js",
        "html" => "ScreenHtml.js",
        "qvt" => "ScreenQvt.js",
        "qvt-static" => "ScreenQvtStatic.js",
        "qjs" | "qvue" => "ScreenPlain.js",
        "tui" => "ScreenTui.js",
        other => return Err(Error::new(format!("no built-in macro module for render mode {other}"))),
    };
    let source = MODE_MODULES
        .iter()
        .find(|(name, _)| *name == file)
        .map(|(_, source)| *source)
        .ok_or_else(|| Error::new("missing built-in module"))?;
    let root = MNode::parse_text("fixture", xml)?;
    with_module(source, mode, debug_comments, |module| generate_body(module, &root, None, mode))
}

/// The complete function source for a tree of widgets, as a unit would run it. For tests.
///
/// # Errors
///
/// Returns an error when the XML does not parse or generate.
#[cfg(test)]
pub fn generate_unit_source(mode: &str, xml: &str) -> Result<String, Error> {
    let file = match mode {
        "text" => "ScreenText.js",
        "csv" => "ScreenCsv.js",
        "xlsx" => "ScreenXlsx.js",
        "html-print" => "ScreenHtmlPrint.js",
        "html" => "ScreenHtml.js",
        "qvt" => "ScreenQvt.js",
        "qvt-static" => "ScreenQvtStatic.js",
        "qjs" | "qvue" => "ScreenPlain.js",
        "tui" => "ScreenTui.js",
        _ => "ScreenXml.js",
    };
    let source = MODE_MODULES
        .iter()
        .find(|(name, _)| *name == file)
        .map(|(_, source)| *source)
        .ok_or_else(|| Error::new("missing built-in module"))?;
    let root = MNode::parse_text("fixture", xml)?;
    with_module(source, mode, false, |module| {
        let body = generate_unit_body(module, &root, None, mode)?;
        Ok(make_unit(module, body)?.source)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_distinct_dynamic_forms_keep_their_units_at_the_bound() {
        let generated = Generated::default();
        let unit = |id| {
            Arc::new(Unit {
                id,
                source: String::new(),
                runtime: Arc::from(""),
                runtime_id: 0,
            })
        };
        generated.put_unit("text|loc|S|widgets".to_string(), Some(unit(0)));
        for id in 1..2_000 {
            generated.put_dynamic_unit(format!("dynamic|text|loc|Form|{id:x}"), unit(id));
            assert!(generated.dynamic_unit_count() <= MAX_DYNAMIC_UNITS);
        }
        assert_eq!(
            generated.dynamic_unit("dynamic|text|loc|Form|7cf").map(|unit| unit.id),
            Some(1_999)
        );
        // Section units are not dropped to make room.
        assert!(generated.unit("text|loc|S|widgets").is_some());
    }

    /// The render modes with a built-in macro module, and the module's file.
    const MODES: [(&str, &str); 10] = [
        ("html", "ScreenHtml.js"),
        ("qvt", "ScreenQvt.js"),
        ("tui", "ScreenTui.js"),
        ("csv", "ScreenCsv.js"),
        ("xlsx", "ScreenXlsx.js"),
        ("xml", "ScreenXml.js"),
        ("text", "ScreenText.js"),
        ("qjs", "ScreenPlain.js"),
        ("html-print", "ScreenHtmlPrint.js"),
        ("qvt-static", "ScreenQvtStatic.js"),
    ];

    fn built_in(file: &str) -> &'static str {
        MODE_MODULES
            .iter()
            .find(|(name, _)| *name == file)
            .map(|(_, source)| *source)
            .unwrap()
    }

    /// The `widgets` and `fail-widgets` of the framework's screen files, each with its file's name.
    fn screen_widgets() -> Vec<(String, MNode)> {
        let mut out = Vec::new();
        for file in crate::script::walker::test_files::component_xml_files("screen") {
            let Ok(root) = MNode::parse_file(&file) else {
                continue;
            };
            let mut widgets = Vec::new();
            crate::script::walker::test_files::named(&root, "widgets", &mut widgets);
            crate::script::walker::test_files::named(&root, "fail-widgets", &mut widgets);
            let name = file.file_name().unwrap_or_default().to_string_lossy().into_owned();
            out.extend(widgets.into_iter().map(|node| (name.clone(), node)));
        }
        out
    }

    fn element_names(node: &MNode, out: &mut HashSet<String>) {
        out.insert(node.name());
        for child in node.child_iter() {
            element_names(&child, out);
        }
    }

    /// A copy of `node` with every element stamped (`mq-src`) as a dev instance stamps a screen's widgets.
    fn stamped(file: &str, node: &MNode) -> MNode {
        fn stamp(node: &MNode, file: &str, count: &mut usize) {
            *count += 1;
            node.set_attribute(crate::screen::stamp::ATTRIBUTE, format!("component://test/screen/{file}:{count}:1"));
            for child in node.child_iter() {
                stamp(&child, file, count);
            }
        }
        let copy = MNode::parse_text(file, &node.write_string()).unwrap();
        stamp(&copy, file, &mut 0);
        copy
    }

    /// A resolve for the comparison, as `unit_for`'s replaces elements: a form is marked assembled, a form-list that
    /// lets the user choose columns is wrapped in `form-list-user-columns`, and an include in the static mode becomes
    /// what it includes.
    fn test_resolve(mode: &str) -> impl Fn(&MNode) -> Result<Option<MNode>, Error> + '_ {
        move |node: &MNode| {
            let name = node.name();
            if mode == super::super::statics::UNIT_MODE && (name == "section-include" || name == "include-screen") {
                let included = MNode::new("container");
                let label = MNode::new("label");
                label.set_attribute("text", format!("included {}", node.attribute("name").unwrap_or_default()));
                included.append_node(label);
                return Ok(Some(included));
            }
            if (name != "form-single" && name != "form-list") || node.has_attribute("mq-assembled") {
                return Ok(None);
            }
            let assembled = MNode::parse_text("assembled", &node.write_string())?;
            assembled.set_attribute("mq-assembled", "true");
            if name == "form-list" && assembled.attribute_is("select-columns", "true") {
                let wrapper = MNode::new("form-list-user-columns");
                wrapper.set_attribute("name", node.attribute("name").unwrap_or_default());
                wrapper.append_node(assembled);
                return Ok(Some(wrapper));
            }
            Ok(Some(assembled))
        }
    }

    /// The unit each of `trees` generates in `mode`, the module loaded once and the trees walked in order; a failure
    /// as its text.
    fn units_of(mode: &str, file: &str, debug: bool, trees: &[MNode]) -> Vec<String> {
        with_module(built_in(file), mode, debug, |module| {
            let resolve = test_resolve(mode);
            Ok(trees
                .iter()
                .map(|root| {
                    match generate_unit_body(module, root, Some(&resolve), mode).and_then(|body| make_unit(module, body)) {
                        Ok(unit) => unit.source,
                        Err(err) => format!("ERROR {}", err.message()),
                    }
                })
                .collect())
        })
        .unwrap()
    }

    /// `trees` generate in every built-in mode, and in html with the macros' debug option on too: most of them (a
    /// fixture element out of its place may fail), html with its elements stamped.
    fn assert_generate(trees: &[(String, MNode)]) {
        let roots: Vec<MNode> = trees.iter().map(|(_, root)| root.clone()).collect();
        let runs = MODES.iter().map(|&(mode, file)| (mode, file, false)).chain([("html", "ScreenHtml.js", true)]);
        for (mode, file, debug) in runs {
            let units = units_of(mode, file, debug, &roots);
            let failed = units.iter().filter(|unit| unit.starts_with("ERROR ")).count();
            if mode == "html" {
                assert!(units.iter().any(|unit| unit.contains("sri.stamp(")), "no stamped element");
            }
            assert!(failed * 4 < units.len(), "{mode}: {failed} of {} failed", units.len());
        }
    }

    /// The macro names of every built-in mode.
    fn macro_names() -> HashSet<String> {
        let mut names = HashSet::new();
        for (mode, file) in MODES {
            let found = with_module(built_in(file), mode, false, |module| module.module.macro_names()).unwrap();
            names.extend(found);
        }
        names
    }

    /// Every element name, those of the framework's screens and those a built-in mode has a macro for, generates in
    /// every built-in mode, its elements stamped and its forms resolved. Each name is generated in the first `widgets`
    /// that has it, and a name no screen has is generated alone. The full sweep of every screen's widgets is
    /// `every_screens_widgets_generate`.
    #[test]
    fn every_element_name_generates() {
        let mut covered = HashSet::new();
        let mut trees = Vec::new();
        for (file, widgets) in screen_widgets() {
            let mut names = HashSet::new();
            element_names(&widgets, &mut names);
            if !names.is_subset(&covered) {
                covered.extend(names);
                trees.push((file.clone(), stamped(&file, &widgets)));
            }
        }
        let mut alone: Vec<String> = macro_names()
            .into_iter()
            .filter(|name| !name.starts_with('@') && !covered.contains(name))
            .collect();
        alone.sort();
        for name in &alone {
            let fixture = MNode::parse_text("alone", &format!("<widgets><{name}/></widgets>")).unwrap();
            trees.push((format!("<{name}/> alone"), stamped("alone.xml", &fixture)));
            covered.insert(name.clone());
        }
        assert!(covered.len() > 120, "{}", covered.len());
        assert_generate(&trees);
        let (names, opaque) = widget_vocabulary().unwrap();
        assert!(names.len() > 100 && !opaque.is_empty(), "{} names", names.len());
    }

    /// Every `widgets` of every framework screen generates in every built-in mode, stamped and with its forms
    /// resolved. Too long for the regression suite in a debug build: `./moq accept-all` runs it.
    #[test]
    #[ignore = "a sweep of every screen's widgets in every mode: acceptance (./moq accept-all)"]
    fn every_screens_widgets_generate() {
        let trees: Vec<(String, MNode)> = screen_widgets()
            .into_iter()
            .map(|(file, widgets)| {
                let stamped = stamped(&file, &widgets);
                (file, stamped)
            })
            .collect();
        assert!(trees.len() > 100, "{}", trees.len());
        assert_generate(&trees);
    }

    /// `xml` generated in html with the macro module `module` (a source as a mode file is).
    fn generated(module: &str, xml: &str) -> Result<String, Error> {
        let root = MNode::parse_text("fixture", xml)?;
        with_module(module, "html", false, |module| generate_body(module, &root, None, "html"))
    }

    /// A macro runs with `this` undefined, as a plain function call.
    #[test]
    fn a_macro_runs_with_this_undefined() {
        const MODULE: &str = r#"(function (T, S) {
            "use strict";
            const macros = {
                "widgets": (node, children) => children,
                "probe": function () { return "sri.write(" + JSON.stringify(typeof this) + ");\n"; },
            };
            return { macros, opaque: [] };
        })"#;
        assert_eq!(generated(MODULE, "<widgets><probe/></widgets>").unwrap(), "sri.write(\"undefined\");\n");
    }

    /// A macro module whose macros fail in the ways a mode file's may: they throw, or return what is not a string.
    const FAILING_MODULE: &str = r#"(function (T, S) {
        "use strict";
        function deeper() { throw new Error("boom"); }
        const macros = {
            "widgets": (node, children) => children,
            "@element": () => "",
            "boom": function boomMacro() { deeper(); },
            "int": () => 42, "float": () => 1.5, "null": () => null, "undef": () => undefined,
            "object": () => ({}), "array": () => [], "bool": () => true, "fn": () => function () {},
            "arrow": () => () => 1, "err": () => new Error("x"), "sym": () => Symbol("s"), "big": () => 10n,
            "promise": () => Promise.resolve(1), "cls": () => class {}, "negz": () => -0, "huge": () => 2 ** 40,
            "proxy": () => new Proxy({}, {}), "revocable": () => Proxy.revocable({}, {}).proxy,
            "fnproxy": () => new Proxy(() => 1, {}), "arrproxy": () => new Proxy([], {}),
            "tagged": () => ({ [Symbol.toStringTag]: "Error" }), "errlike": () => Object.create(Error.prototype),
            "textentry": "not a function", "numentry": 7,
            "throwtext": () => { throw "a text"; }, "thrownum": () => { throw 42; },
            "thrownull": () => { throw null; }, "throwobj": () => { throw { a: 1 }; },
        };
        return { macros, opaque: [] };
    })"#;

    /// A macro that throws fails the walk with the error and the stack down to the macro, in the file name an
    /// evaluated source has, none of the walker's own frames.
    #[test]
    fn a_macro_that_throws_fails_with_its_error_and_stack() {
        let oxide = generated(FAILING_MODULE, "<widgets><boom/></widgets>").unwrap_err();
        assert!(
            oxide
                .message()
                .starts_with("Error: boom\n    at deeper (eval_script:3:"),
            "{}",
            oxide.message()
        );
        assert!(!oxide.message().contains("(native)"), "{}", oxide.message());
    }

    /// A macro that returns what is not a string fails with the conversion error QuickJS's bindings gave, its type
    /// named as they named it.
    #[test]
    fn a_macro_that_returns_no_string_fails_naming_its_type() {
        for (name, type_name) in [
            ("int", "int"),
            ("float", "float"),
            ("null", "null"),
            ("undef", "undefined"),
            ("object", "object"),
            ("array", "array"),
            ("bool", "bool"),
            ("fn", "constructor"),
            ("arrow", "function"),
            ("err", "exception"),
            ("sym", "symbol"),
            ("big", "big_int"),
            ("promise", "promise"),
            ("cls", "constructor"),
            ("negz", "float"),
            ("huge", "float"),
            ("proxy", "proxy"),
            ("revocable", "proxy"),
            ("fnproxy", "function"),
            ("arrproxy", "proxy"),
            ("tagged", "object"),
            ("errlike", "object"),
        ] {
            let failed = generated(FAILING_MODULE, &format!("<widgets><{name}/></widgets>")).unwrap_err();
            let expected = format!("Error converting from js '{type_name}' into type 'string'");
            assert_eq!(failed.message(), expected, "{name}");
        }
    }

    /// A macro entry that is not a function fails with the conversion error QuickJS's bindings gave.
    #[test]
    fn a_macro_entry_that_is_no_function_fails_naming_its_type() {
        for (name, type_name) in [("textentry", "string"), ("numentry", "int")] {
            let failed = generated(FAILING_MODULE, &format!("<widgets><{name}/></widgets>")).unwrap_err();
            let expected = format!("Error converting from js '{type_name}' into type 'function'");
            assert_eq!(failed.message(), expected, "{name}");
        }
    }

    /// A macro that throws what is not an error fails with the thrown value, printed.
    #[test]
    fn a_macro_that_throws_no_error_fails_with_the_value() {
        for (name, shown) in [
            ("throwtext", "\"a text\""),
            ("thrownum", "42"),
            ("thrownull", "null"),
            ("throwobj", "{ a: 1 }"),
        ] {
            assert_eq!(
                generated(FAILING_MODULE, &format!("<widgets><{name}/></widgets>")).unwrap_err().message(),
                format!("Exception generated by quickjs: {shown}\n"),
                "{name}"
            );
        }
    }

    /// An element with no macro is logged as it takes the fallback, also when a later macro throws.
    #[test]
    fn the_fallback_is_logged_when_a_later_macro_throws() {
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let subscriber = crate::testing::LogSubscriber(std::sync::Arc::clone(&lines));
        let result = tracing::subscriber::with_default(subscriber, || {
            generated(FAILING_MODULE, "<widgets><unheard/><boom/></widgets>")
        });
        assert!(result.is_err());
        let logged = lines.lock().unwrap().join("\n");
        assert!(
            logged.contains("No screen macro in html, element for unheard; it renders as an unknown element"),
            "{logged}"
        );
    }

    /// A collection of element literals that fails is a "screen macros" error.
    #[test]
    fn a_failed_collection_is_a_screen_macros_error() {
        const MODULE: &str = r#"(function (T, S) {
            S.beginLits = () => { throw new Error("no collection"); };
            return { macros: { "widgets": (node, children) => children }, opaque: [] };
        })"#;
        let root = MNode::parse_text("fixture", "<widgets/>").unwrap();
        let failed = with_module(MODULE, "html", false, |module| {
            generate_unit_body(module, &root, None, "html").map(|body| body.code)
        })
        .unwrap_err();
        assert!(failed.message().starts_with("screen macros: "), "{}", failed.message());
    }

    /// A macro that loops for ever stops at the thread's script deadline, and one that fills the heap stops at the cap;
    /// generation on the thread goes on afterwards.
    #[test]
    fn a_runaway_macro_stops_at_the_deadline_and_the_heap_cap() {
        const MODULE: &str = r#"(function (T, S) {
            const macros = {
                "widgets": (node, children) => children,
                "spin": () => { for (;;) {} },
                "hungry": () => { const held = []; for (;;) held.push("x".repeat(1 << 20) + held.length); },
                "fine": () => "sri.write('fine');\n",
            };
            return { macros, opaque: [] };
        })"#;
        let started = std::time::Instant::now();
        let oxide = {
            let _deadline = crate::script::DeadlineGuard::new(Some(std::time::Duration::from_millis(200)));
            generated(MODULE, "<widgets><spin/></widgets>")
        };
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
        let oxide = oxide.unwrap_err();
        assert!(oxide.message().contains("interrupted"), "{}", oxide.message());
        let oxide = generated(MODULE, "<widgets><hungry/></widgets>").unwrap_err();
        assert!(oxide.message().starts_with("Error: out of memory"), "{}", oxide.message());
        // the generator that ran out is replaced: the next generation starts a new runtime
        let starts = crate::script::oxide::walker::generator_starts();
        let oxide = generated(MODULE, "<widgets><fine/></widgets>");
        assert_eq!(crate::script::oxide::walker::generator_starts(), starts + 1);
        assert_eq!(oxide.unwrap(), "sri.write('fine');\n");
        let starts = crate::script::oxide::walker::generator_starts();
        let oxide = generated(MODULE, "<widgets><fine/></widgets>");
        assert_eq!(crate::script::oxide::walker::generator_starts(), starts, "a runtime that did not run out is kept");
        assert_eq!(oxide.unwrap(), "sri.write('fine');\n");
    }

    /// The macro sources compiled on a thread's oxide generator are bounded: a mode file edited again and again does not
    /// grow them.
    #[test]
    fn the_compiled_macro_sources_are_bounded() {
        for edit in 0..80 {
            let module = format!(
                "(function (T, S) {{ /* edit {edit} */ return {{ macros: {{ \"widgets\": (n, c) => c }}, opaque: [] }}; }})"
            );
            assert_eq!(generated(&module, "<widgets/>").unwrap(), "");
            let kept = crate::script::oxide::walker::compiled_count();
            assert!(kept <= 32, "{kept} after {edit}");
        }
    }
}
