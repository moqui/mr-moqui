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

//! The thread's oxide runtime and a run on it: started on the thread's first oxide script with [`MoquiHost`] and a
//! 64 MiB heap cap, counted in the thread's shared depth, deadline and default limit ([`crate::script::run`]),
//! dropped and started again after a host-function panic poisons it, and reported in the engine stats.
//!
//! A run is not nested in another oxide run on the thread (the router sends such a script to QuickJS until the fork's
//! re-entry handle F3a lands), so every Context call here is the runtime's one entry.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{
    CallableRef, Context, HeapLimits, JsResult, ObjectRef, OrdinaryPropertyDescriptor, RuntimeError, RuntimeOptions,
    Scope, Value,
};

use super::compiled::Compiled;
use super::convert::{key, to_host, truthy, Conv};
use super::errors::{panic_message, runtime_error, throw_plain};
use super::host::MoquiHost;
use crate::context::Error;
use crate::script::run::{self, EngineTag, ScriptLimit, MAX_SCRIPT_DEPTH};
use crate::script::runtime::EntityCell;

/// The runtime's heap cap, as QuickJS's memory limit.
const HEAP_CAP: usize = 64 * 1024 * 1024;
/// The collection floor: below it the runtime does not collect. Lower than the fork's 1 MiB default, since every
/// thread that runs a small script holds a runtime (plans/Script-Engine-Oxide.md section 4).
const MIN_HEAP: usize = 512 * 1024;
/// Runs between samples of the runtime's reserved bytes for the stats.
const HEAP_SAMPLE_RUNS: u64 = 32;

/// What the runtime cuts when it starts, behind the static check (`script::check`): a computed key it cannot read
/// (`row[name]`) may still name `constructor` at run time, so no function's `constructor` reaches `Function` (or its
/// generator and async kinds), and the legacy accessor definers, which would leave a function on a shared prototype,
/// are gone. Expressions use neither.
const HARDEN_JS: &str = r#"(function () {
    const refuse = function () { throw new TypeError("Function is not available to an expression"); };
    const kinds = [function () {}];
    try { kinds.push(eval("(function* () {})"), eval("(async function () {})"), eval("(async function* () {})")); } catch (e) {}
    for (const kind of kinds) {
        Object.defineProperty(Object.getPrototypeOf(kind), "constructor", { value: refuse, writable: false, configurable: false });
    }
    for (const name of ["__defineGetter__", "__defineSetter__", "__lookupGetter__", "__lookupSetter__"]) {
        delete Object.prototype[name];
    }
})()"#;

/// Why an oxide run gave no result.
pub(crate) enum Outcome {
    /// The script's own error, as the QuickJS path would give it.
    Error(Error),
    /// The run met something the oxide bindings do not have yet; when it reached no host call, the router runs the
    /// script on QuickJS.
    Fallback(&'static str),
}

/// Objects of the runtime the bindings use, made once when it starts.
pub(super) struct Protos {
    pub(super) decimal: ObjectRef,
    pub(super) make_entity: CallableRef,
    pub(super) set_prototype: ObjectRef,
    pub(super) object_prototype: ObjectRef,
    pub(super) date_prototype: ObjectRef,
    pub(super) date_get_time: CallableRef,
    pub(super) array_from: CallableRef,
    pub(super) error: CallableRef,
}

pub(super) struct OxideEngine {
    runtime: quickjs_oxide::engine::api::core::Runtime,
    context: Context,
    pub(super) protos: Protos,
    compiled: RefCell<Compiled>,
    /// The objects this run made for bare names (lists, maps, decimals, entity values), so a name read twice is the
    /// same object. Emptied when the run ends.
    names: RefCell<HashMap<String, Value>>,
    /// The entity values this run made, with their cells, so one converts back to its cell.
    entities: RefCell<Vec<(ObjectRef, Arc<EntityCell>)>>,
    /// The reserved bytes last sampled, counted in [`counts::HEAP`].
    heap: Cell<usize>,
    runs: Cell<u64>,
}

/// Process-wide counts of the oxide runtimes, for [`stats`].
mod counts {
    use super::{AtomicU64, AtomicUsize};
    pub(super) static STARTED: AtomicUsize = AtomicUsize::new(0);
    pub(super) static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub(super) static LIVE_MAX: AtomicUsize = AtomicUsize::new(0);
    pub(super) static HEAP: AtomicUsize = AtomicUsize::new(0);
    pub(super) static HEAP_MAX: AtomicUsize = AtomicUsize::new(0);
    pub(super) static COMPILED: AtomicU64 = AtomicU64::new(0);
    pub(super) static POISONED: AtomicU64 = AtomicU64::new(0);
}

/// What the process's oxide runtimes have done since it started.
#[derive(Clone, Debug, Default)]
pub(crate) struct OxideStats {
    pub(crate) engines_started: usize,
    pub(crate) engines_live: usize,
    pub(crate) engines_live_max: usize,
    /// The runtimes' reserved heap bytes as last sampled, now and at their highest.
    pub(crate) heap_bytes: usize,
    pub(crate) heap_bytes_max: usize,
    /// Texts compiled.
    pub(crate) compiled: u64,
    /// Runtimes dropped after a host function panicked.
    pub(crate) poisoned: u64,
}

pub(crate) fn stats() -> OxideStats {
    OxideStats {
        engines_started: counts::STARTED.load(Relaxed),
        engines_live: counts::LIVE.load(Relaxed),
        engines_live_max: counts::LIVE_MAX.load(Relaxed),
        heap_bytes: counts::HEAP.load(Relaxed),
        heap_bytes_max: counts::HEAP_MAX.load(Relaxed),
        compiled: counts::COMPILED.load(Relaxed),
        poisoned: counts::POISONED.load(Relaxed),
    }
}

/// What this thread's oxide runtime holds.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OxideMemory {
    pub(crate) engines: usize,
    /// Bytes the heap's arenas and tables reserve, and the live bytes the last collection left.
    pub(crate) reserved_bytes: usize,
    pub(crate) live_bytes: usize,
    /// Compiled units and the bytes of their instructions, positions and source.
    pub(crate) code_units: usize,
    pub(crate) code_bytes: usize,
    /// Texts in the compiled-expression cache, and their source bytes.
    pub(crate) compiled_scripts: usize,
    pub(crate) compiled_script_bytes: usize,
}

/// [`OxideMemory`] of this thread's runtime, after a collection when `collect`. Not while a script runs on it.
pub(crate) fn engine_memory(collect: bool) -> OxideMemory {
    let Some(engine) = current() else {
        return OxideMemory::default();
    };
    if run::is_running(EngineTag::Oxide) {
        return OxideMemory::default();
    }
    if collect {
        let _ = engine.runtime.run_gc();
    }
    let mut memory = OxideMemory {
        engines: 1,
        ..OxideMemory::default()
    };
    if let Ok(report) = engine.runtime.memory_report() {
        memory.reserved_bytes = report.reserved_bytes();
        memory.live_bytes = report.gc.live_bytes;
        memory.code_units = report.code.units;
        memory.code_bytes = report.code.instruction_bytes + report.code.position_bytes + report.code.source_bytes;
    }
    let compiled = engine.compiled.borrow();
    memory.compiled_scripts = compiled.len();
    memory.compiled_script_bytes = compiled.bytes();
    memory
}

thread_local! {
    static ENGINE: RefCell<Option<Rc<OxideEngine>>> = const { RefCell::new(None) };
    /// What the running oxide script met that the bindings do not have, set by a host function before it throws.
    static FALLBACK: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// This thread's oxide runtime, when it has started.
pub(super) fn current() -> Option<Rc<OxideEngine>> {
    ENGINE.with(|engine| engine.borrow().clone())
}

fn engine() -> Result<Rc<OxideEngine>, Error> {
    if let Some(engine) = current() {
        return Ok(engine);
    }
    let started = Rc::new(
        OxideEngine::start().map_err(|err| Error::new(format!("the quickjs-oxide runtime did not start: {err}")))?,
    );
    ENGINE.with(|engine| *engine.borrow_mut() = Some(Rc::clone(&started)));
    Ok(started)
}

impl OxideEngine {
    fn start() -> Result<Self, RuntimeError> {
        let options = RuntimeOptions {
            heap: HeapLimits { max_bytes: HEAP_CAP },
            min_heap: MIN_HEAP,
            ..RuntimeOptions::default()
        };
        let runtime = quickjs_oxide::engine::api::core::Runtime::with_host(MoquiHost, options);
        let context = runtime.context();
        let helpers = context.eval(
            "({setPrototype: Set.prototype, objectPrototype: Object.prototype, \
             datePrototype: Date.prototype, getTime: Date.prototype.getTime, arrayFrom: Array.from, error: Error})",
        )?;
        let Value::Object(helpers) = helpers else {
            return Err(RuntimeError::Invariant("the helper object"));
        };
        let object = |name: &str| -> Result<ObjectRef, RuntimeError> {
            match context.get_property(&helpers, &key(name).map_err(conv_runtime)?)? {
                Value::Object(object) => Ok(object),
                _ => Err(RuntimeError::Invariant("a helper")),
            }
        };
        let function = |name: &str| -> Result<CallableRef, RuntimeError> {
            context
                .as_callable(&object(name)?)?
                .ok_or(RuntimeError::Invariant("a helper function"))
        };
        let protos = Protos {
            decimal: super::decimal::make_prototype(&context)?,
            make_entity: super::names::entity_maker(&context)?,
            set_prototype: object("setPrototype")?,
            object_prototype: object("objectPrototype")?,
            date_prototype: object("datePrototype")?,
            date_get_time: function("getTime")?,
            array_from: function("arrayFrom")?,
            error: function("error")?,
        };
        let ec = context.new_object()?;
        let global = context.global_object()?;
        context.define_own_property(
            &global,
            &key("ec").map_err(conv_runtime)?,
            &OrdinaryPropertyDescriptor::data(Value::Object(ec.clone()), false, false, true),
        )?;
        super::bind::install_ec(&mut context.clone(), &ec, std::rc::Rc::new(EngineHooks))?;
        super::decimal::install_to_decimal(&context)?;
        context.eval(HARDEN_JS)?;
        super::names::install_names(&context)?;
        counts::STARTED.fetch_add(1, Relaxed);
        let live = counts::LIVE.fetch_add(1, Relaxed) + 1;
        counts::LIVE_MAX.fetch_max(live, Relaxed);
        Ok(Self {
            runtime,
            context,
            protos,
            compiled: RefCell::new(Compiled::new()),
            names: RefCell::new(HashMap::new()),
            entities: RefCell::new(Vec::new()),
            heap: Cell::new(0),
            runs: Cell::new(0),
        })
    }

    /// The object this run made for `name`.
    pub(super) fn kept_name(&self, name: &str) -> Option<Value> {
        self.names.borrow().get(name).cloned()
    }

    pub(super) fn keep_name(&self, name: &str, value: Value) {
        self.names.borrow_mut().insert(name.to_string(), value);
    }

    /// The Proxy this run made for an entity cell.
    pub(super) fn entity_proxy(&self, cell: &Arc<EntityCell>) -> Option<ObjectRef> {
        self.entities
            .borrow()
            .iter()
            .find(|(_, kept)| Arc::ptr_eq(kept, cell))
            .map(|(proxy, _)| proxy.clone())
    }

    pub(super) fn keep_entity(&self, proxy: ObjectRef, cell: Arc<EntityCell>) {
        self.entities.borrow_mut().push((proxy, cell));
    }

    /// The cell of an entity value this run made.
    pub(super) fn entity_of(&self, object: &ObjectRef) -> Option<Arc<EntityCell>> {
        self.entities
            .borrow()
            .iter()
            .find(|(proxy, _)| proxy == object)
            .map(|(_, cell)| Arc::clone(cell))
    }

    /// Sample the reserved bytes every [`HEAP_SAMPLE_RUNS`] runs, for the stats.
    fn sample_heap(&self) {
        let runs = self.runs.get() + 1;
        self.runs.set(runs);
        if runs % HEAP_SAMPLE_RUNS != 1 {
            return;
        }
        if let Ok(report) = self.runtime.memory_report() {
            let now = report.reserved_bytes();
            let before = self.heap.replace(now);
            let total = if now >= before {
                counts::HEAP.fetch_add(now - before, Relaxed) + (now - before)
            } else {
                counts::HEAP.fetch_sub(before - now, Relaxed) - (before - now)
            };
            counts::HEAP_MAX.fetch_max(total, Relaxed);
        }
    }
}

impl Drop for OxideEngine {
    fn drop(&mut self) {
        counts::LIVE.fetch_sub(1, Relaxed);
        counts::HEAP.fetch_sub(self.heap.get(), Relaxed);
    }
}

/// The conversions the host table's binder makes on this engine ([`super::bind::OxideHooks`]): those of
/// [`super::convert`], so a decimal is the decimal host object and an entity value the entity Proxy. A value the bindings
/// do not have yet marks the run for a fallback, as in a bare-name read.
struct EngineHooks;

impl super::bind::OxideHooks for EngineHooks {
    fn to_host(&self, scope: &mut Scope<'_>, value: &Value) -> JsResult<HostValue> {
        let Some(engine) = current() else {
            return Err(RuntimeError::Invariant("no oxide engine"));
        };
        to_host(scope, &engine, value).map_err(|conv| host_error(scope, conv))
    }

    fn to_js(&self, scope: &mut Scope<'_>, value: HostValue) -> JsResult<Value> {
        let Some(engine) = current() else {
            return Err(RuntimeError::Invariant("no oxide engine"));
        };
        super::convert::to_js(scope, &engine, &value).map_err(|conv| host_error(scope, conv))
    }
}

fn conv_runtime(conv: Conv) -> RuntimeError {
    match conv {
        Conv::Js(err) => err,
        Conv::Unsupported(what) => RuntimeError::Invariant(what),
        Conv::Error(message) => RuntimeError::Internal(message),
    }
}

/// Run a host function's `body`: a panic becomes a thrown `Error` (the runtime is not poisoned), a value the bindings
/// do not have marks the run for a fallback and throws, and a conversion error throws a plain `Error` with its message.
pub(super) fn guarded(
    scope: &mut Scope<'_>,
    body: impl FnOnce(&mut Scope<'_>) -> Result<Value, Conv>,
) -> JsResult<Value> {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| body(&mut *scope)));
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(conv)) => Err(host_error(scope, conv)),
        Err(payload) => {
            let message = panic_message(payload);
            Err(plain(scope, &message))
        }
    }
}

fn host_error(scope: &mut Scope<'_>, conv: Conv) -> RuntimeError {
    match conv {
        Conv::Js(err) => err,
        Conv::Unsupported(reason) => {
            FALLBACK.with(|fallback| {
                if fallback.get().is_none() {
                    fallback.set(Some(reason));
                }
            });
            RuntimeError::Internal(format!("not on quickjs-oxide yet: {reason}"))
        }
        Conv::Error(message) => plain(scope, &message),
    }
}

fn plain(scope: &mut Scope<'_>, message: &str) -> RuntimeError {
    match current() {
        Some(engine) => throw_plain(scope, &engine.protos.error, message),
        None => scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, message),
    }
}

/// Forgets what one run made (the names' objects, the entity values, a fallback mark) when it ends, however it ends.
struct RunState<'a>(&'a OxideEngine);

impl Drop for RunState<'_> {
    fn drop(&mut self) {
        if let Ok(mut names) = self.0.names.try_borrow_mut() {
            names.clear();
        }
        if let Ok(mut entities) = self.0.entities.try_borrow_mut() {
            entities.clear();
        }
        FALLBACK.with(|fallback| fallback.set(None));
    }
}

/// Run `source` on the thread's oxide runtime and give its completion value to `finish`.
fn run<T>(
    source: &str,
    hash: u64,
    finish: impl FnOnce(&mut Context, &OxideEngine, Value) -> Result<T, Conv>,
) -> Result<T, Outcome> {
    let depth = run::depth();
    if depth >= MAX_SCRIPT_DEPTH {
        return Err(Outcome::Error(Error::new("scripts are nested too deeply")));
    }
    if run::is_running(EngineTag::Oxide) {
        return Err(Outcome::Fallback("a run nested in a running oxide script"));
    }
    run::stack_budget(depth).map_err(Outcome::Error)?;
    let engine = engine().map_err(Outcome::Error)?;
    let _tag = EngineTag::Oxide.enter();
    let _limit = (depth == 0).then(ScriptLimit::arm);
    let _depth = run::enter_depth(depth);
    let state = RunState(&engine);
    FALLBACK.with(|fallback| fallback.set(None));
    let compiled = {
        let mut cache = engine.compiled.borrow_mut();
        let before = cache.compiles();
        let compiled = cache.get_or_compile(&engine.context, source, hash);
        counts::COMPILED.fetch_add(cache.compiles() - before, Relaxed);
        compiled
    };
    let mut context = engine.context.clone();
    let mut poisoned = false;
    let mut out_of_memory = false;
    let mut note = |err: &RuntimeError| {
        poisoned |= matches!(err, RuntimeError::Poisoned);
        out_of_memory |= matches!(err, RuntimeError::OutOfMemory);
    };
    let outcome = match compiled.and_then(|compiled| context.execute(&compiled)) {
        Ok(value) => finish(&mut context, &engine, value).map_err(|conv| {
            if let Conv::Js(err) = &conv {
                note(err);
            }
            conv_outcome(&mut context, conv)
        }),
        Err(err) => {
            note(&err);
            Err(Outcome::Error(runtime_error(&mut context, err)))
        }
    };
    let fallback = FALLBACK.with(Cell::take);
    if cfg!(debug_assertions) && fallback.is_none() && !poisoned {
        check_kept_names(&mut context, &engine, source);
    }
    drop(state);
    if poisoned {
        counts::POISONED.fetch_add(1, Relaxed);
        tracing::error!(script = %source, "a host function panicked in quickjs-oxide; the runtime is dropped and restarts on the next script");
        ENGINE.with(|current| *current.borrow_mut() = None);
    } else {
        if out_of_memory {
            engine.compiled.borrow_mut().clear_transient();
            let _ = engine.runtime.run_gc();
        }
        engine.sample_heap();
    }
    match fallback {
        Some(reason) => Err(Outcome::Fallback(reason)),
        None => outcome,
    }
}

/// The safety net behind the static check's refusal of writes, in debug and test builds: a list or map this run read
/// by name converts back to what the stack holds, or the run changed a copy QuickJS would have written back. A
/// difference is logged with the script, and counted for the tests ([`changed_copies`]).
fn check_kept_names(context: &mut Context, engine: &OxideEngine, source: &str) {
    let kept: Vec<(String, Value)> = engine
        .names
        .borrow()
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    for (name, value) in kept {
        let held = crate::script::run::STACK.with(|stack| stack.borrow().with_value(&name, HostValue::clone));
        let Some(held) = held.filter(|held| matches!(held, HostValue::List(_) | HostValue::Map(_))) else {
            continue;
        };
        let Ok(now) = to_host(context, engine, &value) else {
            continue;
        };
        if now != held {
            CHANGED_COPIES.with(|changed| changed.set(changed.get() + 1));
            tracing::error!(name = %name, script = %source, "a quickjs-oxide expression changed its copy of a context value; QuickJS would have written it back");
        }
    }
}

thread_local! {
    static CHANGED_COPIES: Cell<u64> = const { Cell::new(0) };
}

/// Runs on this thread whose copy of a context value changed ([`check_kept_names`]).
#[cfg(test)]
pub(super) fn changed_copies() -> u64 {
    CHANGED_COPIES.with(Cell::get)
}

fn conv_outcome(context: &mut Context, conv: Conv) -> Outcome {
    match conv {
        Conv::Js(err) => Outcome::Error(runtime_error(context, err)),
        Conv::Unsupported(reason) => Outcome::Fallback(reason),
        Conv::Error(message) => Outcome::Error(Error::new(message)),
    }
}

/// [`crate::script::eval_condition`] on quickjs-oxide: `source` (as it runs, after `as_expression`) is true by
/// JavaScript's truth.
pub(crate) fn eval_condition(source: &str, hash: u64) -> Result<bool, Outcome> {
    run(source, hash, |_, _, value| Ok(truthy(&value)))
}

/// [`crate::script::eval_value`] on quickjs-oxide: the value of `source` as a host value.
pub(crate) fn eval_value(source: &str, hash: u64) -> Result<HostValue, Outcome> {
    run(source, hash, |context, engine, value| to_host(context, engine, &value))
}
