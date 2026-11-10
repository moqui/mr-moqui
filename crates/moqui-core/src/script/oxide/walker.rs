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

//! The macro walker on quickjs-oxide (plans/archive/Script-Engine-Oxide.md, Phase 3): XML actions
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
    CallableRef, CompileOptions, Context, FunctionBytecodeRef, HeapLimits, Host, JsString, ObjectRef, Runtime,
    RuntimeError, RuntimeOptions, Value,
};

use super::convert::key;
use super::errors::{runtime_error, rquickjs_type_name};
use crate::context::Error;
use crate::script::bounded::BoundedMap;
use crate::script::run::{self, ScriptLimit, DEADLINE};

/// `walker::walk` in JavaScript, over the tree's JSON text: the node objects (`name`, `text`, `attrs`, `src`, `inert`,
/// `parent`, `children`) first, then the macros in post-order, each node's `source` set as its macro returns. The
/// result is `{source}`, or how the walk ended: `{missing}` for an element with no macro when there is no fallback,
/// `{converting}` for a macro entry that is not a function or a macro that did not return a string (the `value`, the
/// type it is `to` be, and whether it is a `proxy`, for the rquickjs conversion error Rust words), `{thrown}` for a
/// macro that threw (its stack without the walker's frames, as on QuickJS where the walker is Rust). `logged` is the
/// elements that took the fallback, however the walk ended.
///
/// The outer function runs once per runtime: it takes the stamped element names (`screen::stamp::STAMPED`), puts a
/// `Proxy` on the global that notes each proxy it makes (rquickjs names a proxy's type, which the bindings cannot
/// tell), and gives the walk.
const WALK_JS: &str = r#"(function (stampedNames) {
  const stamped = new Set(stampedNames);
  const proxies = new WeakSet();
  const NativeProxy = Proxy;
  const nativeRevocable = NativeProxy.revocable;
  const revocable = function revocable(target, handler) {
      const made = nativeRevocable(target, handler);
      proxies.add(made.proxy);
      return made;
  };
  globalThis.Proxy = new NativeProxy(NativeProxy, {
      construct(target, args, newTarget) {
          const made = Reflect.construct(target, args, newTarget);
          proxies.add(made);
          return made;
      },
      get(target, key, receiver) {
          return key === "revocable" ? revocable : Reflect.get(target, key, receiver);
      },
  });
  return function (macros, treeText, missing, fallback) {
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
    // the stack of an error a macro threw, down to the macro: the walker's frames and the native call into it go
    const trim = (e) => {
        if (e === null || typeof e !== "object") return;
        try {
            if (typeof e.stack !== "string") return;
            const lines = e.stack.split("\n");
            let end = lines.findIndex((line) => line.includes("(moqui-walker:"));
            if (end < 0) return;
            if (end > 0 && lines[end - 1] === "    at call (native)") end--;
            e.stack = lines.slice(0, end).join("\n") + "\n";
        } catch (ignored) {}
    };
    const lacking = {};
    const converting = {};
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
        const fn = macros[macro];
        if (typeof fn !== "function") { converting.value = fn; converting.to = "function"; throw converting; }
        let source;
        try {
            // `this` undefined, as the QuickJS walker calls a macro
            source = fn.call(undefined, o, combined);
        } catch (e) {
            trim(e);
            throw e;
        }
        if (typeof source !== "string") { converting.value = source; converting.to = "string"; throw converting; }
        if (typeof o.src === "string" && stamped.has(name) && source !== "") {
            source = "sri.stamp(" + JSON.stringify(o.src) + ");\n" + source + "sri.stamp(null);\n";
        }
        o.source = source;
        return source;
    };
    try {
        return { source: run(build(JSON.parse(treeText), null)), logged };
    } catch (e) {
        if (e === lacking) return { missing: lacking.message, logged };
        if (e === converting) {
            const value = converting.value;
            converting.value = undefined;
            return { converting: true, value, to: converting.to, proxy: proxies.has(value), logged };
        }
        return { thrown: e, threw: true, logged };
    }
  };
})"#;

/// The file name of [`WALK_JS`] in a stack, which `trim` cuts at.
const WALK_FILE: &str = "moqui-walker";

/// The file name of a macro source in a stack: QuickJS's for the scripts it evaluates (`compiled::eval_shared`).
const MACROS_FILE: &str = "eval_script";

/// Sets a support object's `options.debug` and `options.mode`, as `walker::load_chain_for` does.
const OPTIONS_JS: &str =
    "(function (support, debug, mode) { support.options.debug = debug; support.options.mode = mode; })";

/// The generator runtime's heap cap, as the QuickJS engine's memory limit the walker runs in on QuickJS.
const HEAP_CAP: usize = 64 * 1024 * 1024;

/// The macro sources kept compiled on a thread, and their text's bytes at most: the built-in modes and their support
/// files are about 750 KB, and a mode file a developer edits is compiled again for each text it has had.
const MAX_COMPILED: usize = 32;
const MAX_COMPILED_BYTES: usize = 4 * 1024 * 1024;

/// The generator runtime's host: the thread's script deadline stops a walk, as the QuickJS interrupt handler stops one
/// on QuickJS.
struct GeneratorHost;

impl Host for GeneratorHost {
    fn interrupt(&mut self) -> bool {
        crate::script::run::interrupted()
            || DEADLINE
            .with(std::cell::Cell::get)
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }
}

/// The thread's generator runtime: the walker, and the macro chains loaded on it by their key.
struct Generator {
    walk: CallableRef,
    options: CallableRef,
    chains: RefCell<HashMap<String, Rc<Vec<ObjectRef>>>>,
    /// The macro sources compiled on the thread, by their hash, the least recently used dropped past the bounds.
    compiled: RefCell<BoundedMap<u64, Compiled>>,
    context: Context,
    // the context is the runtime's; kept so the runtime lives as long as the handles
    _runtime: Runtime,
}

/// A compiled macro source and its text.
type Compiled = (Rc<str>, FunctionBytecodeRef);

thread_local! {
    static GENERATOR: RefCell<Option<Rc<Generator>>> = const { RefCell::new(None) };
    /// The generator runtimes started on the thread.
    static STARTS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn callable(context: &Context, source: &str) -> Result<CallableRef, RuntimeError> {
    let compiled = context.compile_with_options(source, &CompileOptions::new(WALK_FILE))?;
    let Value::Object(object) = context.execute(&compiled)? else {
        return Err(RuntimeError::Invariant("a walker function"));
    };
    context
        .as_callable(&object)?
        .ok_or(RuntimeError::Invariant("a walker function"))
}

/// The walk of [`WALK_JS`], over the stamped element names.
fn walk_function(context: &Context) -> Result<CallableRef, RuntimeError> {
    let make = callable(context, WALK_JS)?;
    let names = crate::screen::stamp::STAMPED
        .iter()
        .map(|name| JsString::try_from_utf8(name).map(Value::String))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RuntimeError::Invariant("a stamped element name"))?;
    let stamped = context.new_array_from_values(names)?;
    let Value::Object(walk) = context.call(&make, Value::Undefined, &[Value::Object(stamped)])? else {
        return Err(RuntimeError::Invariant("the walker function"));
    };
    context
        .as_callable(&walk)?
        .ok_or(RuntimeError::Invariant("the walker function"))
}

fn generator() -> Result<Rc<Generator>, Error> {
    if let Some(found) = GENERATOR.with(|cell| cell.borrow().clone()) {
        return Ok(found);
    }
    let options = RuntimeOptions {
        heap: HeapLimits { max_bytes: HEAP_CAP },
        ..RuntimeOptions::default()
    };
    let runtime = Runtime::with_host(GeneratorHost, options);
    let context = runtime.context();
    let started = (|| -> Result<Generator, RuntimeError> {
        Ok(Generator {
            walk: walk_function(&context)?,
            options: callable(&context, OPTIONS_JS)?,
            chains: RefCell::new(HashMap::new()),
            compiled: RefCell::new(BoundedMap::with_bytes(MAX_COMPILED, MAX_COMPILED_BYTES)),
            context: context.clone(),
            _runtime: runtime,
        })
    })()
    .map_err(|err| Error::new(format!("the quickjs-oxide generator did not start: {err}")))?;
    let started = Rc::new(started);
    STARTS.with(|starts| starts.set(starts.get() + 1));
    GENERATOR.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&started)));
    Ok(started)
}

/// `body` with the thread's default script limit armed when no script is running, as the QuickJS walker's entry arms
/// it ([`crate::script::walker_context`]); inside a script the script's deadline applies.
fn limited<R>(body: impl FnOnce() -> Result<R, Error>) -> Result<R, Error> {
    let _limit = (run::depth() == 0).then(ScriptLimit::arm);
    body()
}

fn failed(context: &Context, err: RuntimeError) -> Error {
    // the heap cap's own error (a compile's too), or the `InternalError: out of memory` a macro caught and threw on
    if super::errors::is_out_of_memory(&mut context.clone(), &err) {
        // a runtime that ran out of heap is not used again: the next generation on the thread starts a new one
        GENERATOR.with(|cell| cell.borrow_mut().take());
    }
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
        .borrow_mut()
        .get(&hash)
        .filter(|(text, _)| **text == *source)
        .map(|(_, compiled)| compiled.clone());
    let compiled = match kept {
        Some(compiled) => compiled,
        None => {
            let compiled = context
                .compile_with_options(source, &CompileOptions::new(MACROS_FILE))
                .map_err(|err| failed(context, err))?;
            generator
                .compiled
                .borrow_mut()
                .insert_sized(hash, (Rc::from(source), compiled.clone()), source.len());
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
    let fallback = match fallback {
        Some(fallback) => js_text(fallback)?,
        None => Value::Null,
    };
    let args = [
        Value::Object(macros.clone()),
        js_text(tree)?,
        js_text(missing)?,
        fallback,
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
    if matches!(property(context, &result, "converting")?, Value::Bool(true)) {
        let value = property(context, &result, "value")?;
        let to = text_property(context, &result, "to")?.unwrap_or_default();
        let proxy = matches!(property(context, &result, "proxy")?, Value::Bool(true));
        let from = rquickjs_type_name(&mut context.clone(), &value, proxy);
        // as rquickjs's `FromJs` error for the macro or its value reads
        return Err(Error::new(format!("Error converting from js '{from}' into type '{to}'")));
    }
    if matches!(property(context, &result, "threw")?, Value::Bool(true)) {
        let thrown = property(context, &result, "thrown")?;
        // the text QuickJS's walker gives a macro's exception (`caught.to_string()`), not a script error of its own
        return Err(Error::new(failed(context, RuntimeError::Exception(thrown)).message().to_string()));
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
    limited(|| actions_on_generator(root, support, actions, debug))
}

fn actions_on_generator(
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
    #[cfg(test)]
    moqui_lib::sync::lock(&GENERATED_ACTIONS).push(tree.clone());
    walk(&generator, macros, &tree, "XML action macro", None)
}

/// The generator runtimes started on this thread. For tests.
#[cfg(test)]
pub(crate) fn generator_starts() -> u64 {
    STARTS.with(std::cell::Cell::get)
}

/// The macro sources kept compiled on this thread's generator. For tests.
#[cfg(test)]
pub(crate) fn compiled_count() -> usize {
    GENERATOR.with(|cell| cell.borrow().as_ref().map_or(0, |generator| generator.compiled.borrow().len()))
}

/// The trees of the XML actions generated here, for a test that asks which engine generated a text.
#[cfg(test)]
pub(crate) static GENERATED_ACTIONS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether XML actions whose tree contains `marker` were generated on quickjs-oxide. For tests.
#[cfg(test)]
pub(crate) fn generated_actions_with(marker: &str) -> bool {
    moqui_lib::sync::lock(&GENERATED_ACTIONS)
        .iter()
        .any(|tree| tree.contains(marker))
}

/// The macro names of the XML actions chain (`support`, then `module`), sorted. For tests.
#[cfg(test)]
pub(crate) fn action_macro_names(support: &str, module: &str) -> Result<Vec<String>, Error> {
    let generator = generator()?;
    let loaded = chain(&generator, None, support, &[module], false, "")?;
    let macros = loaded.last().ok_or_else(|| Error::new("no XML actions macros"))?;
    let context = &generator.context;
    let keys = context.own_property_keys(macros).map_err(|err| failed(context, err))?;
    let mut names: Vec<String> = keys
        .into_iter()
        .filter_map(|found| found.to_js_string().map(|name| name.to_utf8_lossy()))
        .collect();
    names.sort();
    Ok(names)
}

/// A screen macro module loaded on the thread's generator: `screen::generate`'s `ModeModule` on quickjs-oxide.
pub(crate) struct ScreenModule {
    /// The runtime the module was loaded on, whose objects its are: a runtime that ran out of heap is replaced for the
    /// loads after, not under a module already loaded.
    generator: Rc<Generator>,
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
    limited(|| screen_module_on_generator(support, screen_support, source, mode, debug))
}

fn screen_module_on_generator(
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
        generator: Rc::clone(&generator),
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
        let generator = &self.generator;
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
        limited(|| self.generate_on_generator(tree, mode, lits))
    }

    fn generate_on_generator(&self, tree: &str, mode: &str, lits: bool) -> Result<(String, Vec<String>), Error> {
        let generator = &self.generator;
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
        // as `screen::generate`'s `js_error` names a failure of the collection on QuickJS
        let screen_macros = |err: Error| Error::new(format!("screen macros: {}", err.message()));
        if lits {
            call(&mut ctx, "beginLits").map_err(screen_macros)?;
        }
        let missing = format!("screen macro in {mode}, element");
        let walked = walk(generator, &self.macros, tree, &missing, Some("@element"));
        let collected = if lits {
            Some(call(&mut ctx, "endLits").map_err(screen_macros))
        } else {
            None
        };
        let source = walked?.replace(['\u{1}', '\u{2}'], "");
        let lits = match collected {
            Some(value) => strings(context, &value?).map_err(screen_macros)?,
            None => Vec::new(),
        };
        Ok((source, lits))
    }
}
