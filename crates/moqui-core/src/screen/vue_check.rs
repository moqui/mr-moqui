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

//! Whether Vue accepts a qvt template. The vendored compiler bundle (`webroot/libs/vue/compiler-dom.cjs.js`)
//! runs in a QuickJS runtime of its own and compiles each template as the browser build does, as the template of a
//! component. This is the check that "the qvt response is a component the Vue 3 compiler can compile" (Gate 3) runs
//! for the fixture screens, and the compiler the server-static work compiles with.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use rquickjs::{CatchResultExt, Context, Runtime};

use crate::context::Error;

const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
const STACK_LIMIT: usize = 4 * 1024 * 1024;

/// Compile each template, wrapped in a `div` as the client wraps a screen, and return the compiler errors as
/// `"<index>: <code> <message> at <line>:<column>"`. An empty list means every template compiled.
///
/// # Errors
///
/// Returns an error when the compiler bundle does not load or the compile script itself fails.
pub fn compile_errors(compiler_source: &str, templates: &[String]) -> Result<Vec<String>, Error> {
    let count = u32::try_from(templates.len().max(1)).unwrap_or(u32::MAX);
    compile_errors_within(compiler_source, templates, COMPILE_TIME_LIMIT.saturating_mul(count))
}

/// [`compile_errors`], with the compile script stopped once `limit` has passed.
fn compile_errors_within(compiler_source: &str, templates: &[String], limit: Duration) -> Result<Vec<String>, Error> {
    let runtime = Runtime::new().map_err(|err| Error::new(err.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    // A template that sends the compiler into a loop stops at the deadline instead of holding the worker.
    runtime.set_interrupt_handler(Some(Box::new(|| {
        COMPILE_DEADLINE.with(Cell::get).is_some_and(|at| Instant::now() >= at)
    })));
    let context = Context::full(&runtime).map_err(|err| Error::new(err.to_string()))?;
    let list = serde_json::to_string(templates).map_err(|err| Error::new(err.to_string()))?;
    context.with(|ctx| {
        ctx.eval::<(), _>(compiler_source)
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the Vue compiler did not load: {err}")))?;
        let script = format!(
            "(function () {{ var errors = []; {list}.forEach(function (template, index) {{
                VueCompilerDOM.compile('<div>' + template + '</div>', {{ mode: 'function', hoistStatic: false,
                    onError: function (err) {{ errors.push(index + ': ' + err.code + ' ' + err.message + (err.loc ? ' at ' + err.loc.start.line + ':' + err.loc.start.column : '')); }} }});
            }}); return JSON.stringify(errors); }})()"
        );
        COMPILE_DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + limit)));
        let json: Result<String, Error> = ctx
            .eval(script)
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the Vue compiler failed: {err}")));
        COMPILE_DEADLINE.with(|deadline| deadline.set(None));
        serde_json::from_str(&json?).map_err(|err| Error::new(err.to_string()))
    })
}

/// A QuickJS runtime that has the compiler loaded, kept for the thread: loading the bundle costs more than a compile.
struct Compiler {
    context: Context,
    // The context borrows this runtime. Drop order frees the context first.
    #[allow(dead_code)]
    runtime: Runtime,
}

thread_local! {
    static COMPILER: RefCell<Option<(u64, Compiler)>> = const { RefCell::new(None) };
    static COMPILE_DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// How long one template compile may run.
const COMPILE_TIME_LIMIT: Duration = Duration::from_secs(20);

fn compiler_for(source: &str) -> Result<Compiler, Error> {
    let runtime = Runtime::new().map_err(|err| Error::new(err.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    // `compile_render` sets `COMPILE_DEADLINE`: without this a template that loops the compiler never stops.
    runtime.set_interrupt_handler(Some(Box::new(|| {
        COMPILE_DEADLINE.with(Cell::get).is_some_and(|at| Instant::now() >= at)
    })));
    let context = Context::full(&runtime).map_err(|err| Error::new(err.to_string()))?;
    context.with(|ctx| {
        ctx.eval::<(), _>(source)
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the Vue compiler did not load: {err}")))
    })?;
    Ok(Compiler { context, runtime })
}

/// The whole compiler text, so an edited bundle of the same length starts a new runtime. Compiles are rare: once per
/// screen definition that is server-static.
fn source_key(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// Compile a template to a render function, as the server-static qvt does when it caches a screen: `prefixIdentifiers`
/// output, so the code reads the scope as `_ctx.name` and has no `with`. The result is the body of a function of `Vue`
/// that returns `render(_ctx, _cache)`.
///
/// # Errors
///
/// Returns an error when the compiler does not load or reports any error for the template.
pub fn compile_render(compiler_source: &str, template: &str) -> Result<String, Error> {
    compile_render_within(compiler_source, template, COMPILE_TIME_LIMIT)
}

/// [`compile_render`], with the compile stopped once `limit` has passed.
fn compile_render_within(compiler_source: &str, template: &str, limit: Duration) -> Result<String, Error> {
    let key = source_key(compiler_source);
    COMPILER.with(|slot| {
        if slot.borrow().as_ref().is_none_or(|(held, _)| *held != key) {
            *slot.borrow_mut() = Some((key, compiler_for(compiler_source)?));
        }
        let guard = slot.borrow();
        let (_, compiler) = guard.as_ref().ok_or_else(|| Error::new("the Vue compiler did not start"))?;
        let input = serde_json::to_string(template).map_err(|err| Error::new(err.to_string()))?;
        let script = format!(
            "(function () {{ var errors = []; var result = VueCompilerDOM.compile({input}, {{ prefixIdentifiers: true, mode: 'function',
                onError: function (err) {{ errors.push(err.code + ' ' + err.message + (err.loc ? ' at ' + err.loc.start.line + ':' + err.loc.start.column : '')); }} }});
                return JSON.stringify({{ code: result.code, errors: errors }}); }})()"
        );
        COMPILE_DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + limit)));
        let json: Result<String, Error> = compiler.context.with(|ctx| {
            ctx.eval(script)
                .catch(&ctx)
                .map_err(|err| Error::new(format!("the Vue compiler failed: {err}")))
        });
        COMPILE_DEADLINE.with(|deadline| deadline.set(None));
        let json = json?;
        let parsed: serde_json::Value = serde_json::from_str(&json).map_err(|err| Error::new(err.to_string()))?;
        let errors: Vec<String> = parsed["errors"]
            .as_array()
            .map(|list| list.iter().filter_map(|item| item.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if !errors.is_empty() {
            return Err(Error::new(format!("the Vue compiler rejected the template: {}", errors.join("; "))));
        }
        parsed["code"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| Error::new("the Vue compiler gave no code"))
    })
}

/// The string templates of the client components: each script is run in a QuickJS context of its own (the client loads
/// `MoquiLib.js` and then `WebrootVue.qvt.js`), and each entry of `moqui.qvt.components` that has a template is
/// answered as its name, the template, and the hash `qvt.templateHash` gives. A definition registered under two names
/// is answered once.
///
/// # Errors
///
/// Returns an error when a script does not run.
pub fn component_templates(scripts: &[&str]) -> Result<Vec<(String, String, String)>, Error> {
    let runtime = Runtime::new().map_err(|err| Error::new(err.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    let context = Context::full(&runtime).map_err(|err| Error::new(err.to_string()))?;
    context.with(|ctx| {
        for script in scripts {
            ctx.eval::<(), _>(*script)
                .catch(&ctx)
                .map_err(|err| Error::new(format!("a client script did not run: {err}")))?;
        }
        let json: String = ctx
            .eval(
                "(function () { var qvt = globalThis.moqui.qvt; var seen = []; var out = [];
                    Object.keys(qvt.components).sort().forEach(function (name) {
                        var comp = qvt.components[name];
                        if (typeof comp.template !== 'string' || seen.indexOf(comp) >= 0) return;
                        seen.push(comp);
                        out.push([name, comp.template, qvt.templateHash(comp.template)]);
                    });
                    return JSON.stringify(out); })()",
            )
            .catch(&ctx)
            .map_err(|err| Error::new(format!("the client components could not be read: {err}")))?;
        serde_json::from_str(&json).map_err(|err| Error::new(err.to_string()))
    })
}

/// `WebrootVue.qvt.render.js`: the render function of each client component, compiled as a server-static template is
/// (`prefixIdentifiers`, no `with`), with the hash of the template it came from.
///
/// # Errors
///
/// Returns an error when a script does not run or the compiler rejects a template.
pub fn precompiled_components(compiler_source: &str, scripts: &[&str]) -> Result<String, Error> {
    let mut out = String::from(
        "/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */\n\n\
         // Generated from the component templates of WebrootVue.qvt.js; do not edit. Make it again with\n\
         //   UPDATE_GOLDEN=1 cargo test -p moqui-core --lib client_components_are_precompiled\n\
         // qvt.applyRenders uses an entry only while its hash matches the template (plans/archive/QvtServerStatic.md, Page build).\n\
         (function (moqui) {\n    \"use strict\";\n    moqui.qvtRenders = {\n",
    );
    let templates = component_templates(scripts)?;
    for (index, (name, template, hash)) in templates.iter().enumerate() {
        let code =
            compile_render(compiler_source, template).map_err(|err| Error::new(format!("component {name}: {err}")))?;
        let body = indented(&code, "            ");
        let name_lit = serde_json::to_string(name).map_err(|err| Error::new(err.to_string()))?;
        out.push_str(&format!(
            "        {name_lit}: {{ hash: \"{hash}\", make: function (Vue) {{\n{body}\n        }} }}{}\n",
            if index + 1 < templates.len() { "," } else { "" }
        ));
    }
    out.push_str("    };\n})(globalThis.moqui);\n");
    Ok(out)
}

/// A screen of a `.qvue` file, bundled with a client that has no template compiler and no `unsafe-eval` (the desktop
/// client's shell, plans/Mobile-Desktop.md §4 "Bundled shell and host contracts"): its name, and its file's text.
pub struct BundledQvue<'a> {
    pub name: &'a str,
    pub source: &'a str,
}

/// The parts of a `.qvue` file as `moqui.parseVue` (MoquiLib.js) finds them: the template, the script with its
/// attributes, and the style with whether it is scoped.
struct QvueParts<'a> {
    template: Option<&'a str>,
    script: Option<(&'a str, &'a str)>,
    style: Option<(&'a str, bool)>,
}

/// As `parseVue`: the first `<tag` followed by a space or `>`, to its `</tag>` (lower case matched), the template to
/// the last `</template>`, as a template may hold templates.
fn qvue_parts(text: &str) -> QvueParts<'_> {
    let lower = text.to_ascii_lowercase();
    let part = |tag: &str| -> Option<(&str, &str)> {
        let opening = format!("<{tag}");
        let mut from = 0;
        let open_at = loop {
            let at = from + lower.get(from..)?.find(&opening)?;
            let next = text.get(at + opening.len()..)?.chars().next();
            if next.is_some_and(|c| c == '>' || c.is_whitespace()) {
                break at;
            }
            from = at + 1;
        };
        let start = open_at + lower.get(open_at..)?.find('>')? + 1;
        let attrs = text.get(open_at + opening.len()..start - 1).unwrap_or("");
        let close = if tag == "template" {
            text.rfind("</template>")?
        } else {
            start + lower.get(start..)?.find(&format!("</{tag}>"))?
        };
        (close >= start).then(|| (attrs, &text[start..close]))
    };
    QvueParts {
        template: part("template").map(|(_, body)| body),
        script: part("script").filter(|(_, body)| !body.trim().is_empty()),
        style: part("style")
            .filter(|(_, body)| !body.trim().is_empty())
            .map(|(attrs, body)| (body, attrs.contains("scoped"))),
    }
}

/// A `.qvue` script as the body of a function that returns its component, as `moqui.parseVue` runs it: a leading
/// `export default` returns the object, and a leading `define(` its result.
fn script_body(script: &str) -> String {
    let source = script.trim();
    let after_default = source
        .strip_prefix("export")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix("default"))
        .filter(|rest| rest.starts_with(char::is_whitespace));
    if let Some(rest) = after_default {
        format!("return {}", rest.trim_start())
    } else if source
        .strip_prefix("define")
        .is_some_and(|rest| rest.trim_start().starts_with('('))
    {
        format!("return {source}")
    } else {
        source.to_string()
    }
}

/// The templates a `.qvue` script's component needs render functions for: its own string `template` (path `[]`,
/// which takes the place of the file's, as in `parseVue`) and those of the components under `components`, each object
/// once (a component used twice, or one that lists itself, is found once). The script runs in a QuickJS context of
/// its own, with a stand-in `moqui`, under the compile deadline; one that does not run there is an error, since the
/// bundle would not run either.
fn script_templates(script: &str, limit: Duration) -> Result<Vec<(Vec<String>, String)>, Error> {
    let runtime = Runtime::new().map_err(|err| Error::new(err.to_string()))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    runtime.set_interrupt_handler(Some(Box::new(|| {
        COMPILE_DEADLINE.with(Cell::get).is_some_and(|at| Instant::now() >= at)
    })));
    let context = Context::full(&runtime).map_err(|err| Error::new(err.to_string()))?;
    let source = format!(
        "(function () {{ var moqui = {{ define: function (a, b, c) {{ var cb = typeof a === 'string' ? (c || b) : (b === undefined ? a : b);
            return typeof cb === 'function' ? cb() : cb; }} }};
            var module = {{ exports: {{}} }}; var define = moqui.define;
            var returned = (function (exports) {{\n{}\n}})(module.exports);
            var comp = returned !== undefined ? returned : module.exports; var out = []; var seen = [];
            (function walk(c, path) {{ if (!c || typeof c !== 'object' || seen.indexOf(c) >= 0) return; seen.push(c);
                if (typeof c.template === 'string') out.push([path, c.template]);
                var cs = c.components; if (!cs || typeof cs !== 'object') return;
                Object.keys(cs).sort().forEach(function (name) {{ walk(cs[name], path.concat([name])); }}); }})(comp, []);
            return JSON.stringify(out); }})()",
        script_body(script)
    );
    COMPILE_DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + limit)));
    let found: Result<String, Error> = context.with(|ctx| {
        ctx.eval(source)
            .catch(&ctx)
            .map_err(|err| Error::new(format!("its script did not run: {err}")))
    });
    COMPILE_DEADLINE.with(|deadline| deadline.set(None));
    serde_json::from_str(&found?).map_err(|err| Error::new(err.to_string()))
}

/// Indent each line that has text.
fn indented(text: &str, by: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{by}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A render function's code as the bundle has it: compiled from `template`, with no `with` or eval in it.
fn bundled_render(compiler_source: &str, template: &str) -> Result<String, Error> {
    let code = compile_render(compiler_source, template)?;
    for banned in ["with (", "with(", "new Function", "eval("] {
        if code.contains(banned) {
            return Err(Error::new(format!("the compiled render has {banned}")));
        }
    }
    Ok(indented(&code, "                "))
}

/// The script of bundled `.qvue` screens: `moqui.qvtBundled[name] = { make(Vue) }`, where `make` runs the file's script
/// and answers `{ component, style, scoped }`, the component with render functions compiled from its templates (the
/// file's, and its components' string templates, set in place) as server-static templates are (`prefixIdentifiers`,
/// no `with`). The client evaluates no text, so it runs under a CSP without `unsafe-eval` on the runtime build of Vue.
/// `moqui.bundledComponent(name)` (WebrootVue.qvt.js) makes a screen the first time it is used, once Vue is there, so
/// one that fails does not take the others with it. Not strict, as `parseVue`'s script is not.
///
/// # Errors
///
/// Returns an error naming the screen when its script cannot be bundled (`<script setup>`, a module, one that does
/// not run) or the compiler rejects a template.
pub fn precompiled_qvue(compiler_source: &str, screens: &[BundledQvue<'_>]) -> Result<String, Error> {
    let mut out = String::from(
        "/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */\n\n\
         // Generated from the bundled .qvue screens of the desktop client's shell; do not edit. Make it again with\n\
         //   UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled\n\
         // Each is moqui.qvtBundled[name] = { make(Vue) }, its templates compiled to render functions\n\
         // (moqui_core::screen::vue_check::precompiled_qvue); moqui.bundledComponent(name) makes it when it is first used.\n\
         (function (moqui) {\n    moqui.qvtBundled = moqui.qvtBundled || {};\n",
    );
    for screen in screens {
        let named = |err: Error| Error::new(format!("bundled screen {}: {err}", screen.name));
        let parts = qvue_parts(screen.source);
        if let Some((attrs, _)) = parts.script {
            if attrs.contains("setup") || attrs.contains("module") {
                return Err(named(Error::new("a <script setup> or module script is not bundled")));
            }
        }
        let script = parts.script.map(|(_, body)| body);
        let mut templates = match script {
            Some(script) => script_templates(script, COMPILE_TIME_LIMIT).map_err(named)?,
            None => Vec::new(),
        };
        // the file's template, unless the script gives the component one of its own
        if !templates.iter().any(|(path, _)| path.is_empty()) {
            if let Some(template) = parts.template {
                templates.insert(0, (Vec::new(), template.to_string()));
            }
        }
        let name_lit = serde_json::to_string(screen.name).map_err(|err| Error::new(err.to_string()))?;
        let body = script.map(script_body).unwrap_or_default();
        out.push_str(&format!(
            "    moqui.qvtBundled[{name_lit}] = {{ make: function (Vue) {{\n\
             \x20       const module = {{ exports: {{}} }};\n\
             \x20       const define = moqui.define;\n\
             \x20       // the file's script as it is (not indented: a template string in it keeps its text)\n\
             \x20       const returned = (function (exports) {{\n{body}\n        }})(module.exports);\n\
             \x20       const component = Object.assign({{}}, returned !== undefined ? returned : module.exports);\n"
        ));
        for (path, template) in &templates {
            let code = bundled_render(compiler_source, template)
                .map_err(|err| named(Error::new(format!("component {}: {err}", path.join("/")))))?;
            let mut target = String::from("component");
            for name in path {
                let lit = serde_json::to_string(name).map_err(|err| Error::new(err.to_string()))?;
                target.push_str(&format!(".components[{lit}]"));
            }
            out.push_str(&format!(
                "        {{\n\
                 \x20           const target = {target};\n\
                 \x20           target.render = (function () {{\n{code}\n            }})();\n\
                 \x20           delete target.template;\n\
                 \x20       }}\n"
            ));
        }
        let (style, scoped) = parts.style.unwrap_or(("", false));
        let style_lit = if style.trim().is_empty() {
            "null".to_string()
        } else {
            serde_json::to_string(style).map_err(|err| Error::new(err.to_string()))?
        };
        out.push_str(&format!(
            "        return {{ component: component, style: {style_lit}, scoped: {scoped} }};\n    }} }};\n"
        ));
    }
    out.push_str("})(globalThis.moqui);\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{compile_errors_within, compile_render_within, script_templates};

    #[test]
    fn a_bundled_script_that_loops_stops_at_the_deadline() {
        let started = Instant::now();
        let err = script_templates("for (;;) {}", Duration::from_millis(100)).unwrap_err();
        assert!(err.to_string().contains("did not run"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_compile_that_loops_stops_at_the_deadline() {
        let looping = "var VueCompilerDOM = { compile: function () { for (;;) {} } };";
        let started = Instant::now();
        let err = compile_errors_within(looping, &["<p/>".to_string()], Duration::from_millis(100)).unwrap_err();
        assert!(err.to_string().contains("the Vue compiler failed"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_render_compile_that_loops_stops_at_the_deadline() {
        let looping = "var VueCompilerDOM = { compile: function () { for (;;) {} } };";
        let started = Instant::now();
        let err = compile_render_within(looping, "<p/>", Duration::from_millis(100)).unwrap_err();
        assert!(err.to_string().contains("the Vue compiler failed"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
