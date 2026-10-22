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
use rquickjs::{Array, Ctx, Object};

use super::cache::ScreenFacade;
use super::definition::ScreenDef;
use super::form::{self, FormInstance};
use super::statics::{Eligibility, StaticTemplate};
use crate::context::{Error, ExecutionContextFactory};
use crate::script::walker::{self, Resolve, Walk};

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
pub fn unit_for(ctx: &Ctx<'_>, request: &UnitRequest<'_>) -> Result<Option<Arc<Unit>>, Error> {
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
                let loaded = load_mode_source(ctx, &source, request.mode, std::env::var_os(DEBUG_ENV).is_some())?;
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
                    if node.attribute("mq-assembled").is_some() {
                        return Ok(None);
                    }
                    let form_name = node.attribute("name").unwrap_or_default();
                    if request.def.forms.get(&form_name).is_some_and(|form| form.dynamic)
                        || form::has_db_extensions(&request.def.location, &form_name)
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
                        && (node.attribute("select-columns").as_deref() == Some("true")
                            || node.attribute("saved-finds").as_deref() == Some("true"))
                    {
                        node.set_attribute("mq-assembled", "true");
                        let wrapper = MNode::new("form-list-user-columns");
                        wrapper.set_attribute("name", form_name);
                        wrapper.append_node(node);
                        return Ok(Some(wrapper));
                    }
                    Ok(Some(node))
                };
                let body = generate_body(ctx, &loaded, &root, Some(&resolve), request.mode).map_err(|err| {
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
    ctx: &Ctx<'_>,
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
    let loaded = load_mode_source(ctx, &source, request.mode, std::env::var_os(DEBUG_ENV).is_some())?;
    let root = MNode::new("dynamic-form");
    root.append_node(form.node()?);
    let body = generate_body(ctx, &loaded, &root, None, request.mode).map_err(|err| {
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

fn make_unit(module: &ModeModule<'_>, body: String) -> Result<Unit, Error> {
    let source = format!(
        "(function ($depth) {{\n\"use strict\";\n{}{body}{}}})",
        module.prelude, module.epilogue
    );
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
pub struct ModeModule<'js> {
    macros: Object<'js>,
    opaque: HashSet<String>,
    prelude: String,
    epilogue: String,
    runtime: Arc<str>,
    runtime_id: u64,
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

fn load_mode_source<'js>(ctx: &Ctx<'js>, source: &str, mode: &str, debug: bool) -> Result<ModeModule<'js>, Error> {
    let chain = walker::load_chain_for(ctx, TEMPLATE_SUPPORT, &[SCREEN_SUPPORT, source], debug, mode)?;
    let module = chain
        .into_iter()
        .next_back()
        .ok_or_else(|| Error::new(format!("no macro module for {mode}")))?;
    let macros: Object = module
        .get("macros")
        .map_err(|_| Error::new(format!("macro module for {mode} has no macros")))?;
    let opaque: Array = module
        .get("opaque")
        .map_err(|_| Error::new(format!("macro module for {mode} has no opaque list")))?;
    let prelude: String = module.get("prelude").unwrap_or_default();
    let epilogue: String = module.get("epilogue").unwrap_or_default();
    let runtime: String = module.get("runtime").unwrap_or_default();
    let runtime_id = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        runtime.hash(&mut hasher);
        hasher.finish()
    };
    Ok(ModeModule {
        macros,
        opaque: walker::names_of(&opaque)?,
        prelude,
        epilogue,
        runtime: Arc::from(runtime),
        runtime_id,
    })
}

fn generate_body<'js>(
    ctx: &Ctx<'js>,
    module: &ModeModule<'js>,
    root: &MNode,
    resolve: Option<Resolve<'_>>,
    mode: &str,
) -> Result<String, Error> {
    let missing = format!("screen macro in {mode}, element");
    let walk = Walk {
        macros: &module.macros,
        opaque: &module.opaque,
        resolve,
        missing: &missing,
        fallback: Some("@element"),
    };
    let (source, _) = walker::walk(ctx, &walk, root, None)?;
    Ok(source.replace(['\u{1}', '\u{2}'], ""))
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
    crate::script::walker_context(|ctx| {
        let module = load_mode_source(ctx, source, "html", false)?;
        let names = module
            .macros
            .keys::<String>()
            .collect::<Result<HashSet<String>, _>>()
            .map_err(|err| Error::new(err.to_string()))?;
        Ok((names, module.opaque.clone()))
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
    crate::script::walker_context(|ctx| {
        let module = load_mode_source(ctx, source, mode, debug_comments)?;
        generate_body(ctx, &module, &root, None, mode)
    })
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
    crate::script::walker_context(|ctx| {
        let module = load_mode_source(ctx, source, mode, false)?;
        let body = generate_body(ctx, &module, &root, None, mode)?;
        Ok(make_unit(&module, body)?.source)
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
}
