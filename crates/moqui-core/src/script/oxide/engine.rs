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
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{
    CallableRef, Context, HeapLimits, Hook, HostClass, Js, JsResult, ObjectRef, OrdinaryPropertyDescriptor,
    RuntimeError, RuntimeOptions, Scope, Value,
};

use super::compiled::Compiled;
use super::convert::{key, to_host, truthy, Conv};
use super::errors::{panic_message, runtime_error, throw_plain};
use super::host::MoquiHost;
use super::tracked::Flush;
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

/// `toNumber(x)`, as the QuickJS host's (`runtime/helpers.rs`): a JavaScript number from a decimal, a number or
/// numeric text, and null from null or empty text. Read-only, as the QuickJS host's globals are.
const TO_NUMBER_JS: &str = r#"Object.defineProperty(globalThis, "toNumber", {
    value: function toNumber(value) {
        if (value == null || value === "") return null;
        if (typeof value === "number") return value;
        return toDecimal(value).toNumber();
    },
    writable: false, enumerable: true, configurable: false
})"#;

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
    pub(super) entities: HostClass<super::names::EntityValues>,
    /// Native tools (`ec.getTool`), the host table's exotic class (`bind::tool`).
    pub(super) tools: HostClass<super::bind::tool::ToolClass>,
    /// Host objects the bindings pass through but do not bind yet (an upload, a stream).
    pub(super) opaque: HostClass<super::names::OpaqueValues>,
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
    /// The values this run read or assigned by name ([`super::tracked`]). Emptied when the run ends.
    run: RefCell<super::tracked::RunValues>,
    /// The entity values this run made, with their cells, so one converts back to its cell.
    entities: RefCell<Vec<(ObjectRef, Arc<EntityCell>)>>,
    /// The reserved bytes last sampled, counted in [`counts::HEAP`], and the most sampled.
    heap: Cell<usize>,
    heap_peak: Cell<usize>,
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

/// The most reserved heap bytes each thread's oxide runtime reached, by thread, with the thread's name (or id), for the
/// peak heap of a run (`MOQUI_SCRIPT_STATS_FILE`). An entry goes with its runtime.
type ThreadPeaks = std::collections::HashMap<std::thread::ThreadId, (String, usize)>;

static THREAD_PEAKS: std::sync::LazyLock<std::sync::Mutex<ThreadPeaks>> = std::sync::LazyLock::new(Default::default);

fn note_thread_peak(bytes: usize) {
    let thread = std::thread::current();
    let mut peaks = moqui_lib::sync::lock(&THREAD_PEAKS);
    let peak = peaks.entry(thread.id()).or_insert_with(|| {
        let name = thread
            .name()
            .map_or_else(|| format!("{:?}", thread.id()), str::to_string);
        (name, 0)
    });
    peak.1 = peak.1.max(bytes);
}

/// The peak reserved heap bytes of each live thread's oxide runtime, as sampled, by thread name.
pub(crate) fn thread_heap_peaks() -> Vec<(String, usize)> {
    let mut peaks: Vec<(String, usize)> = moqui_lib::sync::lock(&THREAD_PEAKS).values().cloned().collect();
    peaks.sort();
    peaks
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
            entities: super::names::register_entities(&context)?,
            tools: super::bind::tool::register(&mut context.clone(), std::rc::Rc::new(EngineHooks))?,
            opaque: context.register_exotic_class(super::names::OpaqueValues)?,
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
        context.eval(TO_NUMBER_JS)?;
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
            run: RefCell::default(),
            entities: RefCell::new(Vec::new()),
            heap: Cell::new(0),
            heap_peak: Cell::new(0),
            runs: Cell::new(0),
        })
    }

    /// The values of the running script, by name.
    pub(super) fn run_values(&self) -> &RefCell<super::tracked::RunValues> {
        &self.run
    }

    /// The entity value object this run made for a cell.
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
            if now > self.heap_peak.get() {
                self.heap_peak.set(now);
                note_thread_peak(now);
            }
        }
    }
}

impl Drop for OxideEngine {
    fn drop(&mut self) {
        counts::LIVE.fetch_sub(1, Relaxed);
        counts::HEAP.fetch_sub(self.heap.get(), Relaxed);
        // the runtime is dropped on its own thread: its thread's peak goes with it
        moqui_lib::sync::lock(&THREAD_PEAKS).remove(&std::thread::current().id());
    }
}

/// The conversions the host table's binder makes on this engine ([`super::bind::OxideHooks`]): those of
/// [`super::convert`], so a decimal is the decimal host object and an entity value the entity exotic object. A value the bindings
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

    /// The tracked values written back before a method that reads the stack (kept) or may change it (released).
    fn flush(&self, scope: &mut Scope<'_>, mode: crate::script::host::FlushBefore) -> JsResult<()> {
        use crate::script::host::FlushBefore;
        let mode = match mode {
            FlushBefore::None => return Ok(()),
            FlushBefore::Keep => Flush::Keep,
            FlushBefore::Release => Flush::Release,
        };
        super::tracked::flush(scope, mode).map_err(|conv| host_error(scope, conv))
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
pub(super) fn guarded<J: Js, T>(js: &mut J, body: impl FnOnce(&mut J) -> Result<T, Conv>) -> JsResult<T> {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| body(&mut *js)));
    match result {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(conv)) => Err(host_error(js, conv)),
        Err(payload) => {
            let message = panic_message(payload);
            Err(plain(js, &message))
        }
    }
}

/// [`guarded`] for an exotic hook, whose answer is handled.
pub(super) fn guarded_hook<J: Js, T>(js: &mut J, body: impl FnOnce(&mut J) -> Result<T, Conv>) -> JsResult<Hook<T>> {
    guarded(js, body).map(Hook::Handled)
}

pub(super) fn host_error<J: Js>(scope: &mut J, conv: Conv) -> RuntimeError {
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

fn plain<J: Js>(scope: &mut J, message: &str) -> RuntimeError {
    match current() {
        Some(engine) => throw_plain(scope, &engine.protos.error, message),
        None => scope.throw_error(quickjs_oxide::engine::api::core::NativeErrorKind::Type, message),
    }
}

/// Forgets what one run made (the names' objects, the entity values, a fallback mark) when it ends, however it ends.
/// A nested run (inside a host call of a running oxide script that lent the engine) sets the outer run's values aside
/// and gives them back, as QuickJS's `OuterRun` does.
struct RunState<'a> {
    engine: &'a OxideEngine,
    outer: Option<Outer>,
}

/// What a nested run set aside of the run around it.
struct Outer {
    values: super::tracked::RunValues,
    entities: Vec<(ObjectRef, Arc<EntityCell>)>,
    fallback: Option<&'static str>,
}

impl<'a> RunState<'a> {
    fn enter(engine: &'a OxideEngine, nested: bool) -> Self {
        let outer = nested.then(|| Outer {
            values: engine.run.take(),
            entities: engine.entities.take(),
            fallback: FALLBACK.with(Cell::take),
        });
        FALLBACK.with(|fallback| fallback.set(None));
        Self { engine, outer }
    }
}

impl Drop for RunState<'_> {
    fn drop(&mut self) {
        match self.outer.take() {
            Some(outer) => {
                if let Ok(mut values) = self.engine.run.try_borrow_mut() {
                    *values = outer.values;
                }
                if let Ok(mut entities) = self.engine.entities.try_borrow_mut() {
                    *entities = outer.entities;
                }
                FALLBACK.with(|fallback| fallback.set(outer.fallback));
            }
            None => {
                if let Ok(mut values) = self.engine.run.try_borrow_mut() {
                    values.clear();
                }
                if let Ok(mut entities) = self.engine.entities.try_borrow_mut() {
                    entities.clear();
                }
                FALLBACK.with(|fallback| fallback.set(None));
            }
        }
    }
}

/// Before a QuickJS script nested in the running oxide one ([`crate::script::engine::cross_into_quickjs`]): the
/// tracked values released. Only while the runtime is lent; a host function that does not lend had the binder flush
/// before its call.
pub(crate) fn release_for_quickjs() -> Result<(), Error> {
    let Some(engine) = current() else {
        return Ok(());
    };
    if !engine.runtime.is_lent() {
        return Ok(());
    }
    let mut context = engine.context.clone();
    super::tracked::flush(&mut context, Flush::Release).map_err(|conv| match conv {
        Conv::Js(err) => runtime_error(&mut context, err),
        Conv::Error(message) => Error::new(message),
        Conv::Unsupported(reason) => Error::new(format!("quickjs-oxide could not write back {reason}")),
    })
}

/// This thread's oxide runtime's realm, started if need be, for the tests that define their own host functions.
#[cfg(test)]
pub(super) fn context_for_tests() -> Context {
    engine().expect("the oxide runtime starts").context.clone()
}

/// Whether this thread's oxide runtime is lent by a running host function (`Scope::lend`, the fork's F3a): a script
/// may then run on it, nested in the running one.
pub(crate) fn is_lent() -> bool {
    current().is_some_and(|engine| engine.runtime.is_lent())
}

/// Run `source` on the thread's oxide runtime and give its completion value to `finish`. When the text `writes` (the
/// static check's answer, or any script under `script_engine=oxide`), its tracked values are written back at its end.
fn run<T>(
    source: &str,
    hash: u64,
    writes: bool,
    file_name: &str,
    finish: impl FnOnce(&mut Context, &OxideEngine, Value) -> Result<T, Conv>,
) -> Result<T, Outcome> {
    let depth = run::depth();
    if depth >= MAX_SCRIPT_DEPTH {
        return Err(Outcome::Error(Error::new("scripts are nested too deeply")));
    }
    let nested = run::is_running(EngineTag::Oxide);
    if nested && !is_lent() {
        return Err(Outcome::Fallback(
            "a run nested in a running oxide script that did not lend its engine",
        ));
    }
    run::stack_budget(depth).map_err(Outcome::Error)?;
    let engine = engine().map_err(Outcome::Error)?;
    let _tag = EngineTag::Oxide.enter();
    let _limit = (depth == 0).then(ScriptLimit::arm);
    let _depth = run::enter_depth(depth);
    let state = RunState::enter(&engine, nested);
    let compiled = {
        let mut cache = engine.compiled.borrow_mut();
        let before = cache.compiles();
        let compiled = cache.get_or_compile(&engine.context, source, hash, file_name);
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
    let mut outcome = outcome;
    if FALLBACK.with(Cell::get).is_none() && !poisoned {
        if writes {
            // as QuickJS does at a script's end, whether it ended well or not
            let flushed = super::tracked::flush(&mut context, Flush::End);
            if let (Ok(_), Err(conv)) = (&outcome, flushed) {
                outcome = Err(conv_outcome(&mut context, conv));
            }
        } else if cfg!(debug_assertions) {
            check_kept_names(&mut context, &engine, source);
        }
    }
    let fallback = FALLBACK.with(Cell::take);
    drop(state);
    // a poisoned runtime goes when the outermost oxide run ends, which sees the poison too
    if poisoned && !nested {
        counts::POISONED.fetch_add(1, Relaxed);
        tracing::error!(script = %source, "a host function panicked in quickjs-oxide; the runtime is dropped and restarts on the next script");
        ENGINE.with(|current| *current.borrow_mut() = None);
    } else if !poisoned {
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

/// The safety net behind the static check's answer that a text does not write, in debug and test builds, for a run
/// that skips the flush at its end: a list or map it read by name converts back to what the stack holds, or the run
/// changed one and the change is lost. A difference is logged with the script, and counted for the tests
/// ([`changed_copies`]).
fn check_kept_names(context: &mut Context, engine: &OxideEngine, source: &str) {
    let kept = engine.run.borrow().tracked();
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
pub(crate) fn eval_condition(source: &str, hash: u64, writes: bool) -> Result<bool, Outcome> {
    run(source, hash, writes, super::compiled::FILE_NAME, |_, _, value| {
        Ok(truthy(&value))
    })
}

/// [`crate::script::expand`] on quickjs-oxide: `source` is the template as a template literal, its value the text.
pub(crate) fn eval_text(source: &str, hash: u64, writes: bool) -> Result<String, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        |context, _, value| match value {
            Value::String(text) => Ok(text.to_utf8_lossy()),
            other => Ok(context.to_js_string(&other)?.to_utf8_lossy()),
        },
    )
}

/// [`crate::script::eval_map`] on quickjs-oxide: the value of `source`, an object, as a map. Anything else is the
/// QuickJS path's error.
pub(crate) fn eval_map(
    source: &str,
    hash: u64,
    writes: bool,
) -> Result<std::collections::BTreeMap<String, HostValue>, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        |context, engine, value| {
            let not_an_object = || Conv::Error("script value is not an object".to_string());
            let Value::Object(object) = &value else {
                return Err(not_an_object());
            };
            if context.is_array_object(object)? || context.as_callable(object)?.is_some() {
                return Err(not_an_object());
            }
            match to_host(context, engine, &value)? {
                HostValue::Map(map) => Ok(map.into_iter().collect()),
                _ => Err(not_an_object()),
            }
        },
    )
}

/// A generated actions script on quickjs-oxide, named `file_name` in stack traces as on QuickJS. Not strict, as there:
/// a bare assignment is how a value reaches the context stack. Kept by the hash of its text and name.
pub(crate) fn run_actions(source: &str, hash: u64, file_name: &str) -> Result<(), Outcome> {
    let key = crate::script::text::hash_of(hash, file_name);
    run(source, key, true, file_name, |_, _, _| Ok(()))
}

/// [`crate::script::eval_value`] on quickjs-oxide: the value of `source` as a host value.
pub(crate) fn eval_value(source: &str, hash: u64, writes: bool) -> Result<HostValue, Outcome> {
    run(
        source,
        hash,
        writes,
        super::compiled::FILE_NAME,
        |context, engine, value| to_host(context, engine, &value),
    )
}
