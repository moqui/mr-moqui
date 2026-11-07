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

//! The macro walker on quickjs-oxide under `script_engine=oxide` (plans/Script-Engine-Oxide.md, Phase 3): XML actions
//! (`TemplateSupport.js`, `XmlActions.js`) and screen macro modules generate their JavaScript here instead of on the
//! thread's QuickJS engine (`script/walker.rs`).
//!
//! Generation is a function of the text alone and calls nothing of the host, so it runs in a runtime of its own on
//! each thread, apart from the runtime scripts run on: the macro chains are loaded once per thread and kept. The tree is
//! built in Rust as `walker::tree_json` gives it (an element's `resolve` applied there, before any macro runs), and
//! [`WALK_JS`] makes the node objects and calls the macros in post-order as `walker::walk` does.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use quickjs_oxide::engine::api::core::{
    CallableRef, CompileOptions, Context, FunctionBytecodeRef, JsString, ObjectRef, Runtime, RuntimeError, Value,
};

use super::convert::key;
use super::errors::runtime_error;
use crate::context::Error;
use crate::script::engine::{script_engine, ScriptEngine};

/// Whether generation runs on quickjs-oxide: under `script_engine=oxide` only.
pub(crate) fn on_oxide() -> bool {
    script_engine() == ScriptEngine::Oxide
}

/// `walker::walk` in JavaScript, over the tree's JSON text: the node objects (`name`, `text`, `attrs`, `src`, `inert`,
/// `parent`, `children`) first, then the macros in post-order, each node's `source` set as its macro returns. An
/// element with no macro is `{missing}` when there is no fallback; the elements that took the fallback are `logged`.
const WALK_JS: &str = r#"(function (macros, treeText, missing, fallback, stamped) {
    const logged = [];
    const build = (n, parent) => {
        const o = { name: n.name, text: n.text, attrs: n.attrs };
        if (n.src != null) o.src = n.src;
        o.inert = n.inert;
        if (parent) o.parent = parent;
        o.children = [];
        for (const c of n.children) o.children.push(build(c, o));
        return o;
    };
    const lacking = {};
    const run = (o) => {
        if (o.inert) { o.source = ""; return ""; }
        let combined = "";
        for (const c of o.children) combined += run(c);
        const name = o.name;
        let macro = name;
        if (!(macro in macros)) {
            if (fallback == null) { lacking.message = "no " + missing + " for " + name; throw lacking; }
            logged.push(name);
            if (!(fallback in macros)) { o.source = ""; return ""; }
            combined = "";
            macro = fallback;
        }
        let source = macros[macro](o, combined);
        if (typeof source !== "string") {
            throw new TypeError("Error converting from js '" + (source === null ? "null" : typeof source) + "' into type 'string'");
        }
        if (typeof o.src === "string" && stamped.includes(name) && source !== "") {
            source = "sri.stamp(" + JSON.stringify(o.src) + ");\n" + source + "sri.stamp(null);\n";
        }
        o.source = source;
        return source;
    };
    try {
        return { source: run(build(JSON.parse(treeText), null)), logged };
    } catch (e) {
        if (e === lacking) return { missing: lacking.message, logged };
        throw e;
    }
})"#;

/// Sets a support object's `options.debug` and `options.mode`, as `walker::load_chain_for` does.
const OPTIONS_JS: &str =
    "(function (support, debug, mode) { support.options.debug = debug; support.options.mode = mode; })";

/// The thread's generator runtime: the walker, and the macro chains loaded on it by their key.
struct Generator {
    walk: CallableRef,
    options: CallableRef,
    chains: RefCell<HashMap<String, Rc<Vec<ObjectRef>>>>,
    /// The macro sources compiled on the thread, by their hash.
    compiled: RefCell<HashMap<u64, Compiled>>,
    context: Context,
    // the context is the runtime's; kept so the runtime lives as long as the handles
    _runtime: Runtime,
}

/// A compiled macro source and its text.
type Compiled = (Rc<str>, FunctionBytecodeRef);

thread_local! {
    static GENERATOR: RefCell<Option<Rc<Generator>>> = const { RefCell::new(None) };
}

fn callable(context: &Context, source: &str) -> Result<CallableRef, RuntimeError> {
    let Value::Object(object) = context.eval(source)? else {
        return Err(RuntimeError::Invariant("a walker function"));
    };
    context
        .as_callable(&object)?
        .ok_or(RuntimeError::Invariant("a walker function"))
}

fn generator() -> Result<Rc<Generator>, Error> {
    if let Some(found) = GENERATOR.with(|cell| cell.borrow().clone()) {
        return Ok(found);
    }
    let runtime = Runtime::new();
    let context = runtime.context();
    let started = (|| -> Result<Generator, RuntimeError> {
        Ok(Generator {
            walk: callable(&context, WALK_JS)?,
            options: callable(&context, OPTIONS_JS)?,
            chains: RefCell::new(HashMap::new()),
            compiled: RefCell::new(HashMap::new()),
            context: context.clone(),
            _runtime: runtime,
        })
    })()
    .map_err(|err| Error::new(format!("the quickjs-oxide generator did not start: {err}")))?;
    let started = Rc::new(started);
    GENERATOR.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&started)));
    Ok(started)
}

fn failed(context: &Context, err: RuntimeError) -> Error {
    let mut context = context.clone();
    runtime_error(&mut context, err)
}

fn js_text(text: &str) -> Result<Value, Error> {
    JsString::try_from_utf8(text)
        .map(Value::String)
        .map_err(|_| Error::new("text is not UTF-8"))
}

fn property(context: &Context, object: &ObjectRef, name: &str) -> Result<Value, Error> {
    let key = key(name).map_err(|_| Error::new("a property key"))?;
    context.get_property(object, &key).map_err(|err| failed(context, err))
}

fn text_property(context: &Context, object: &ObjectRef, name: &str) -> Result<Option<String>, Error> {
    Ok(match property(context, object, name)? {
        Value::String(text) => Some(text.to_utf8_lossy()),
        _ => None,
    })
}

fn object_property(context: &Context, object: &ObjectRef, name: &str) -> Result<Option<ObjectRef>, Error> {
    Ok(match property(context, object, name)? {
        Value::Object(found) => Some(found),
        _ => None,
    })
}

/// The strings of an array value.
fn strings(context: &Context, value: &Value) -> Result<Vec<String>, Error> {
    let Value::Object(array) = value else {
        return Err(Error::new("not an array of names"));
    };
    let length = match property(context, array, "length")? {
        Value::Int(length) => usize::try_from(length).unwrap_or(0),
        Value::Float(length) if length >= 0.0 => length as usize,
        _ => 0,
    };
    let mut out = Vec::with_capacity(length);
    for index in 0..length {
        match property(context, array, &index.to_string())? {
            Value::String(text) => out.push(text.to_utf8_lossy()),
            _ => return Err(Error::new("not an array of names")),
        }
    }
    Ok(out)
}

/// The value of `source`, compiled once on the thread and evaluated anew each call.
fn evaluate(generator: &Generator, source: &str) -> Result<Value, Error> {
    let context = &generator.context;
    let hash = crate::script::text::hash_text(source);
    let kept = generator
        .compiled
        .borrow()
        .get(&hash)
        .filter(|(text, _)| **text == *source)
        .map(|(_, compiled)| compiled.clone());
    let compiled = match kept {
        Some(compiled) => compiled,
        None => {
            let compiled = context
                .compile_with_options(source, &CompileOptions::new("macros"))
                .map_err(|err| failed(context, err))?;
            generator
                .compiled
                .borrow_mut()
                .insert(hash, (Rc::from(source), compiled.clone()));
            compiled
        }
    };
    context.execute(&compiled).map_err(|err| failed(context, err))
}

/// The macro chain `walker::load_chain_for` loads: the first source evaluates to the support object, each later one to
/// a function of the results before it. With `cache_key`, the chain is kept on the thread under it, as the QuickJS
/// engine keeps the XML actions macros (`engine_object`); without, it is loaded anew, as a screen mode is on QuickJS,
/// since a mode's macros keep counters across one walk that must start from nothing at the next.
fn chain(
    generator: &Generator,
    cache_key: Option<&str>,
    support: &str,
    modules: &[&str],
    debug: bool,
    mode: &str,
) -> Result<Rc<Vec<ObjectRef>>, Error> {
    if let Some(found) = cache_key.and_then(|key| generator.chains.borrow().get(key).cloned()) {
        return Ok(found);
    }
    let context = &generator.context;
    let ctx = context.clone();
    let Value::Object(support) = evaluate(generator, support)? else {
        return Err(Error::new("the macro support source is not an object"));
    };
    ctx.call(
        &generator.options,
        Value::Undefined,
        &[Value::Object(support.clone()), Value::Bool(debug), js_text(mode)?],
    )
    .map_err(|err| failed(context, err))?;
    let mut results = vec![support];
    for module in modules {
        let Value::Object(function) = evaluate(generator, module)? else {
            return Err(Error::new("a macro module is not a function"));
        };
        let function = context
            .as_callable(&function)
            .map_err(|err| failed(context, err))?
            .ok_or_else(|| Error::new("a macro module is not a function"))?;
        let args: Vec<Value> = results.iter().cloned().map(Value::Object).collect();
        let Value::Object(made) = ctx
            .call(&function, Value::Undefined, &args)
            .map_err(|err| failed(context, err))?
        else {
            return Err(Error::new("a macro module did not give an object"));
        };
        results.push(made);
    }
    let made = Rc::new(results);
    if let Some(cache_key) = cache_key {
        generator
            .chains
            .borrow_mut()
            .insert(cache_key.to_string(), Rc::clone(&made));
    }
    Ok(made)
}

/// Walk the tree `tree` (`walker::tree_json`) with `macros`, as `walker::walk` does.
fn walk(
    generator: &Generator,
    macros: &ObjectRef,
    tree: &str,
    missing: &str,
    fallback: Option<&str>,
) -> Result<String, Error> {
    let context = &generator.context;
    let ctx = context.clone();
    let stamped = ctx
        .new_array_from_values(
            crate::screen::stamp::STAMPED
                .iter()
                .map(|name| js_text(name))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|err| failed(context, err))?;
    let fallback = match fallback {
        Some(fallback) => js_text(fallback)?,
        None => Value::Null,
    };
    let args = [
        Value::Object(macros.clone()),
        js_text(tree)?,
        js_text(missing)?,
        fallback,
        Value::Object(stamped),
    ];
    let Value::Object(result) = ctx
        .call(&generator.walk, Value::Undefined, &args)
        .map_err(|err| failed(context, err))?
    else {
        return Err(Error::new("the walker gave no result"));
    };
    for name in strings(context, &property(context, &result, "logged")?)? {
        tracing::error!("No {missing} for {name}; it renders as an unknown element");
    }
    if let Some(missing) = text_property(context, &result, "missing")? {
        return Err(Error::new(missing));
    }
    text_property(context, &result, "source")?.ok_or_else(|| Error::new("the walker gave no source"))
}

/// The script XML actions generate for `root`, on quickjs-oxide: `actions::generate_with`'s walk.
///
/// # Errors
///
/// An element with no macro, or a macro that throws.
pub(crate) fn generate_actions(
    root: &moqui_lib::node::MNode,
    support: &str,
    actions: &str,
    debug: bool,
) -> Result<String, Error> {
    let generator = generator()?;
    let name = if debug { "xml-actions-debug" } else { "xml-actions" };
    let loaded = chain(&generator, Some(name), support, &[actions], debug, "")?;
    let macros = loaded.last().ok_or_else(|| Error::new("no XML actions macros"))?;
    let tree = crate::script::walker::tree_json(root, &HashSet::new(), None)?;
    walk(&generator, macros, &tree, "XML action macro", None)
}

/// A screen macro module loaded on the thread's generator: `screen::generate`'s `ModeModule` on quickjs-oxide.
pub(crate) struct ScreenModule {
    support: ObjectRef,
    macros: ObjectRef,
    pub(crate) opaque: HashSet<String>,
    pub(crate) prelude: String,
    pub(crate) epilogue: String,
    pub(crate) runtime: String,
}

/// Load the screen macro chain (`support`, `screen_support`, `source`) for `mode`, anew from its compiled sources, as
/// QuickJS loads it for each unit.
///
/// # Errors
///
/// A source that does not evaluate, a module that throws, or a module without `macros` or `opaque`.
pub(crate) fn screen_module(
    support: &str,
    screen_support: &str,
    source: &str,
    mode: &str,
    debug: bool,
) -> Result<ScreenModule, Error> {
    let generator = generator()?;
    let loaded = chain(&generator, None, support, &[screen_support, source], debug, mode)?;
    let (Some(module), Some(support)) = (loaded.last(), loaded.get(loaded.len().wrapping_sub(2))) else {
        return Err(Error::new(format!("no macro module for {mode}")));
    };
    let context = &generator.context;
    let macros = object_property(context, module, "macros")?
        .ok_or_else(|| Error::new(format!("macro module for {mode} has no macros")))?;
    let opaque = strings(context, &property(context, module, "opaque")?)
        .map_err(|_| Error::new(format!("macro module for {mode} has no opaque list")))?;
    Ok(ScreenModule {
        support: support.clone(),
        macros,
        opaque: opaque.into_iter().collect(),
        prelude: text_property(context, module, "prelude")?.unwrap_or_default(),
        epilogue: text_property(context, module, "epilogue")?.unwrap_or_default(),
        runtime: text_property(context, module, "runtime")?.unwrap_or_default(),
    })
}

impl ScreenModule {
    /// The names of the module's macros.
    pub(crate) fn macro_names(&self) -> Result<HashSet<String>, Error> {
        let generator = generator()?;
        let context = &generator.context;
        let keys = context
            .own_property_keys(&self.macros)
            .map_err(|err| failed(context, err))?;
        let mut names = HashSet::new();
        for found in keys {
            if let Some(name) = found.to_js_string() {
                names.insert(name.to_utf8_lossy());
            }
        }
        Ok(names)
    }

    /// Walk `tree` (`walker::tree_json`) with the module's macros; with `lits`, the element literals the walk
    /// collected (`beginLits` and `endLits`) too.
    ///
    /// # Errors
    ///
    /// A macro that throws.
    pub(crate) fn generate(&self, tree: &str, mode: &str, lits: bool) -> Result<(String, Vec<String>), Error> {
        let generator = generator()?;
        let context = &generator.context;
        let mut ctx = context.clone();
        let call = |ctx: &mut Context, name: &str| -> Result<Value, Error> {
            let Some(function) = object_property(context, &self.support, name)? else {
                return Err(Error::new(format!("screen support has no {name}")));
            };
            let function = context
                .as_callable(&function)
                .map_err(|err| failed(context, err))?
                .ok_or_else(|| Error::new(format!("screen support's {name} is not a function")))?;
            ctx.call(&function, Value::Object(self.support.clone()), &[])
                .map_err(|err| failed(context, err))
        };
        if lits {
            call(&mut ctx, "beginLits")?;
        }
        let missing = format!("screen macro in {mode}, element");
        let walked = walk(&generator, &self.macros, tree, &missing, Some("@element"));
        let collected = if lits { Some(call(&mut ctx, "endLits")) } else { None };
        let source = walked?.replace(['\u{1}', '\u{2}'], "");
        let lits = match collected {
            Some(value) => strings(context, &value?)?,
            None => Vec::new(),
        };
        Ok((source, lits))
    }
}
