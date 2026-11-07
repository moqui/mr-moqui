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

//! QuickJS evaluation for server expressions and XML actions.
//!
//! One runtime per thread. Bare names resolve through a proxy on `globalThis`
//! that reads the thread-local context stack. `ec` is an own property of
//! `globalThis` and hosts the calls XML actions emit. The bare name `context` is
//! the stack itself, as Java's `ContextStack` is: a proxy whose reads and writes
//! are those of the bare names (see [`CONTEXT_PROXY`]).

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::context_stack::{ContextStack, Lookup};
use moqui_lib::object::{HostObject, Value};
use rquickjs::allocator::{Allocator, RustAllocator};
use rquickjs::class::Trace;
use rquickjs::function::{Opt, Rest, This};
use rquickjs::object::Property;
use rquickjs::Error as JsError;
use rquickjs::{
    CatchResultExt, Class, Context, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Null, Object, Persistent,
    Runtime, Undefined, Value as JsValue,
};

mod bind;
mod compiled;
mod convert;
mod decimal;
mod ec_extra;
mod entities;
mod facades;
#[cfg(test)]
mod framework_gaps_tests;
mod helpers;
#[cfg(test)]
mod metadata_tests;
mod native_tools;
#[cfg(test)]
mod realm_tests;
mod screen_host;
mod upload;
mod values;
mod web_host;

pub(crate) use crate::script::text::ScriptText;
use compiled::{eval_compiled, eval_compiled_text, MAX_COMPILED_SCRIPTS, MAX_COMPILED_SCRIPT_BYTES};
use convert::{
    collection_to_js, enter_nested, host_ref_to_js, js_bytes, js_caught, js_i64, js_matches_host, js_string,
    js_truthy, js_value_to_host, object_map, text_or_empty,
};
pub use convert::{host_to_js, js_to_host};
use decimal::HostDecimal;
pub use ec_extra::expand_localized;
pub(crate) use ec_extra::{code128_options, encode_html_js_safe, flatten_nested_map, js_safe_map, localized, url_safe_chars};
pub(crate) use helpers::{java_type_of, now_timestamp, timestamp_of, HELPERS_JS};
pub(crate) use facades::{module_cache_on, read_module};
use entities::{
    atom_name, entity_to_js, install_entity_prototypes, keep_assigned, prototype_has, prototype_member, wrap_find,
    HostEntity, HostList, ScriptList,
};
pub(crate) use convert::{converts_with_script, temporal_text, MAX_NESTING, SAFE_INTEGER, SET_KEY, TOO_DEEP, WRAP_KEY};
pub(crate) use crate::script::host::rounding_strategy_named as rounding_named;
pub(crate) use decimal::{arith, decimal_of, decimals_compare, decimals_equal, Arith};
pub(crate) use entities::{
    entity_field_names, entity_is_field, entity_set_field, EntityCell, EntityField,
};
pub use facades::forget_scripts;
pub use facades::run_script_location;
use facades::{resource_expand, resource_script, system_get_property};
use helpers::install_helpers;
use native_tools::{tool_to_js, HostTool};
pub(crate) use screen_host::with_screen_runner;
use upload::StreamBytes;
pub use values::script_list_values;
pub(crate) use values::{context_combined, pop_context_frame, put_context_all, value_entity_name, value_field};
pub use values::{value_items, value_map};
pub(crate) use web_host::invoke as invoke_web_handler;
pub(crate) use web_host::INVOKE_JS as WEB_INVOKE_JS;

use crate::context::{Error, ErrorKind, ExecutionContext};
use crate::script::engine::EntryKind;
pub(crate) use crate::script::run::{mark_request_thread, script_time_remaining};
pub use crate::script::run::DeadlineGuard;
#[cfg(test)]
use crate::script::run::{script_limit_for, NESTED_STACK, REQUEST_SCRIPT_STACK, SCRIPT_LIMIT};
pub(crate) use crate::script::run::STACK;
use crate::script::run::{stack_budget, EngineTag, ScriptLimit, DEADLINE, DEPTH, MAX_SCRIPT_DEPTH};
use moqui_db::sql::Condition;

use crate::entity::{EntityFacade, EntityFind, EntityList, EntityValue};

const MAX_CACHED_TEMPLATES: usize = 4096;
/// The source text the template and screen unit caches of an engine may hold. Their bytecode is about the size of the
/// source and lives in the engine's [`MEMORY_LIMIT`] with everything the scripts make.
const MAX_CACHED_TEMPLATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CACHED_UNIT_BYTES: usize = 8 * 1024 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
// Leave room for a new render's transient values and compilation. Source-byte cache limits cannot predict bytecode
// size, and a screen may catch an allocation error before with_context sees it and releases the caches.
const CACHE_HEAP_PRESSURE: i64 = (MEMORY_LIMIT * 3 / 4) as i64;
pub(crate) const VALUE_OF_MESSAGE: &str =
    "decimal valueOf is not supported; use add, subtract, multiply, divide, eq, compare, isZero, or toNumber";

thread_local! {
    /// While true, an expression of a widget runs with the `ec` of the server-static browser host: the compatibility report's full check.
    static WIDGET_WRAP: Cell<bool> = const { Cell::new(false) };
    /// This thread's engines: the main one ([`EngineKind::Main`]) and, for the test harness, the spec one.
    static ENGINES: RefCell<Vec<(EngineKind, Rc<Engine>)>> = const { RefCell::new(Vec::new()) };
    /// The QuickJS engine of each running QuickJS script, innermost last. [`crate::script::run`] has the engine of
    /// every running script, of both engines.
    static RUNNING: RefCell<Vec<Rc<Engine>>> = const { RefCell::new(Vec::new()) };
}

/// Whether widget expressions run with the wrapped `ec` now (the server-static report's full check).
pub(crate) fn widget_wrap_on() -> bool {
    WIDGET_WRAP.with(Cell::get)
}

/// Write the innermost QuickJS script's tracked values back to the stack: the boundary an oxide script crosses when it
/// starts inside a running QuickJS script, so it reads by bare name what the QuickJS script changed in place
/// ([`crate::script::engine::cross_into_oxide`]). Kept tracked (`Flush::Keep`) for a read-only oxide script; released
/// (`Flush::Release`) for one that may write, so the QuickJS script reads what it wrote.
pub(crate) fn flush_innermost(release: bool) -> Result<(), Error> {
    let Some(engine) = current_engine() else {
        return Ok(());
    };
    // SAFETY: `engine` is running on this thread (it is the innermost of `RUNNING`), so its outermost
    // `Context::with` holds the runtime's lock for as long as this call lasts, and the `Ctx` made here does not
    // outlive it.
    let ctx = unsafe { Ctx::from_raw(engine.context.as_raw()) };
    flush_tracked(&ctx, &engine, if release { Flush::Release } else { Flush::Keep })
}

/// Turn the wrapped `ec` for widget expressions on or off. Returns the setting it replaces.
pub fn set_widget_wrap(on: bool) -> bool {
    WIDGET_WRAP.with(|cell| cell.replace(on))
}

/// The widget check set for a scope. Dropped, also by an error or a panic, it puts back the setting it replaced.
pub(crate) struct WidgetWrap {
    previous: bool,
}

impl WidgetWrap {
    pub(crate) fn set(on: bool) -> Self {
        Self {
            previous: set_widget_wrap(on),
        }
    }
}

impl Drop for WidgetWrap {
    fn drop(&mut self) {
        set_widget_wrap(self.previous);
    }
}

/// Run `body`, which evaluates expressions for the framework itself (an entity filter's apply condition and map), with
/// the real `ec` while the static report's full check is on: they are not the screen's widget expressions. An entity
/// find in a render (a drop-down's options) evaluates its filters on this thread with the check on, and with the
/// wrapper a filter's `ec.user.context` was recorded against the screen and closed the find. A script a widget
/// expression calls into runs on an engine of its own, with the real `ec`, already.
pub(crate) fn with_real_ec<T>(body: impl FnOnce() -> T) -> T {
    let _unchecked = WidgetWrap::set(false);
    body()
}

/// The real `ec` put back on `globalThis` when dropped, after a widget expression ran with the wrapper.
struct RealEc<'js> {
    ctx: Ctx<'js>,
    real: JsValue<'js>,
}

impl Drop for RealEc<'_> {
    fn drop(&mut self) {
        let _ = set_ec(&self.ctx, self.real.clone());
    }
}

/// Replace the global `ec`. It is read-only to scripts (see [`freeze_host_globals`]), so it is defined again.
fn set_ec<'js>(ctx: &Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<()> {
    ctx.globals()
        .prop("ec", Property::from(value).enumerable().configurable())
}

/// The `ec` of the browser host as a wrapper over the real one: the members `plans/archive/QvtServerStatic.md` lists are the real members,
/// and any other area, or any other member of an allowed area (`ec.l10n.getAvailableLocales`), is reported through `deny` and
/// throws, so the check records it rather than the expression failing on an undefined member. Symbol keys (a coercion's
/// `Symbol.toPrimitive`) and the probes of a plain object (`toString`, `valueOf` and the other `Object.prototype`
/// members a template's coercion or a check reads; `then` and `toJSON`, undefined there) are as on a plain object, not
/// violations.
const WRAPPED_EC: &str = r#"(function (real, deny) {
    const has = (target, key) => Object.prototype.hasOwnProperty.call(target, key);
    const guard = (target, prefix) => new Proxy(target, {
        get(target, key) {
            if (typeof key === "symbol") return undefined;
            if (has(target, key)) return target[key];
            if (key in Object.prototype) return Object.prototype[key];
            if (key === "then" || key === "toJSON") return undefined;
            deny(prefix + String(key));
            throw new Error("ec." + prefix + String(key) + " is not available in a server-static qvt screen");
        }
    });
    const area = (name, names) => {
        const source = real[name], out = {};
        names.forEach((member) => { const value = source[member]; out[member] = typeof value === "function" ? value.bind(source) : value; });
        return guard(out, name + ".");
    };
    return guard({
        l10n: area("l10n", ["localize", "format", "formatCurrency", "formatCurrencyNoSymbol"]),
        user: area("user", ["userId", "username", "locale", "timeZone", "userAccount", "getPreference"]),
        resource: area("resource", ["expand", "expandNoL10n"])
    }, "");
})"#;

/// Run `body`, a widget expression, with the wrapped `ec` when a widget check is on. A violation is recorded when the expression
/// touches an area outside the host, and the error is the caller's: `fallback` turns it into a value so a render goes on.
fn with_widget_ec<'js, R>(
    ctx: &Ctx<'js>,
    expression: &str,
    body: impl FnOnce() -> Result<R, Error>,
) -> Result<R, Error> {
    if !WIDGET_WRAP.with(Cell::get) {
        return body();
    }
    crate::screen::statics::set_expression(expression);
    let globals = ctx.globals();
    let real: JsValue = globals.get("ec").map_err(js_error)?;
    let deny = Function::new(ctx.clone(), |ctx: Ctx<'_>, area: String| {
        host_guard(&ctx, || {
            crate::screen::statics::record_wrapped(&area);
            Ok(())
        })
    })
    .map_err(js_error)?;
    let make: Function = eval_js(ctx, WRAPPED_EC)?;
    let wrapper: JsValue = make
        .call((real.clone(), deny))
        .catch(ctx)
        .map_err(|caught| Error::new(caught.to_string()))?;
    set_ec(ctx, wrapper).map_err(js_error)?;
    let _real = RealEc { ctx: ctx.clone(), real };
    // The expression may call back into the host (expand inside an expression): that is not checked twice.
    let _unchecked = WidgetWrap::set(false);
    body()
}


/// An inert module's value kept for the run, with the text it was made from ([`kept_module`]).
type KeptModule = (Arc<str>, Persistent<JsValue<'static>>);

/// Persistents drop before the context, and the context drops before the runtime.
struct Engine {
    /// The live bytes of this engine's heap, kept by its [`CountingAllocator`].
    heap_bytes: Rc<Cell<usize>>,
    /// Lists, maps and decimals a running script read by bare name. The script mutates the lists and maps, so
    /// they are written back to the stack at the end of the run and before a host call, when changed.
    tracked: RefCell<HashMap<String, Persistent<JsValue<'static>>>>,
    /// Tracked values a host call took off `tracked`, with the value written for each. The script may still hold
    /// the object, so it is tracked again while the stack holds what was written. See [`flush_tracked`].
    released: RefCell<HashMap<String, Released>>,
    templates: RefCell<super::bounded::BoundedMap<String, Persistent<Function<'static>>>>,
    /// Action scripts, conditions, and expressions compiled to bytecode, by the hash of their text.
    scripts: RefCell<compiled::CompiledScripts>,
    /// Compiled screen units by unit id.
    units: RefCell<super::bounded::BoundedMap<u64, Persistent<Function<'static>>>>,
    /// The values of inert modules (`ec.resource.script`) given in the outermost run on this thread, by location, with
    /// the text each was made from; emptied when that run ends. See [`kept_module`].
    modules: RefCell<HashMap<String, KeptModule>>,
    /// Objects made once per engine, such as the XML actions macros, by name. See [`engine_object`].
    objects: RefCell<HashMap<&'static str, Persistent<Object<'static>>>>,
    /// Ids of the mode runtimes this engine has already run.
    runtimes: RefCell<std::collections::HashSet<u64>>,
    /// How many screen units are running, one inside another (a section in a form-list cell runs inside the list's unit).
    /// A unit gets the depth it starts at, so the outermost one starts its mode's render state afresh.
    unit_depth: Cell<u32>,
    /// The object the bare name `context` reads as: the stack itself, made once (see [`CONTEXT_PROXY`]).
    context_proxy: Persistent<JsValue<'static>>,
    /// Whether [`flush_tracked`] is writing this engine's tracked values back. A `context` converted meanwhile
    /// (a tracked map that holds it) reads the stack as it is, instead of flushing again inside the flush.
    flushing: Cell<bool>,
    object_utilities: Persistent<Object<'static>>,
    collection_utilities: Persistent<Object<'static>>,
    string_utilities: Persistent<Object<'static>>,
    web_utilities: Persistent<Object<'static>>,
    user_security_checks: Persistent<Object<'static>>,
    context: Context,
    // The context and its persistents borrow this runtime. Drop order frees them first.
    runtime: Runtime,
}

/// Process-wide counts of the script engines, for [`engine_stats`].
mod counts {
    use std::sync::atomic::AtomicUsize;
    pub(super) static STARTED: AtomicUsize = AtomicUsize::new(0);
    /// Engines started at depth 0: the threads that ran a script.
    pub(super) static THREADS: AtomicUsize = AtomicUsize::new(0);
    pub(super) static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub(super) static LIVE_MAX: AtomicUsize = AtomicUsize::new(0);
    pub(super) static HEAP: AtomicUsize = AtomicUsize::new(0);
    pub(super) static HEAP_MAX: AtomicUsize = AtomicUsize::new(0);
}

/// What the process's script engines have done since it started: engines started and the most alive at once, the
/// deepest nesting, the QuickJS heaps' live bytes (all engines) now and at their highest, and the scripts compiled and
/// read from the shared bytecode instead.
#[derive(Clone, Debug, Default)]
pub struct EngineStats {
    pub engines_started: usize,
    pub threads: usize,
    pub engines_live: usize,
    pub engines_live_max: usize,
    pub depth_max: usize,
    pub heap_bytes: usize,
    pub heap_bytes_max: usize,
    pub compiled: u64,
    pub shared_reads: u64,
    /// The quickjs-oxide runtimes (one per thread that ran an oxide script): started, alive now and at most.
    pub oxide_engines_started: usize,
    pub oxide_engines_live: usize,
    pub oxide_engines_live_max: usize,
    /// Their reserved heap bytes as last sampled, now and at the highest.
    pub oxide_heap_bytes: usize,
    pub oxide_heap_bytes_max: usize,
    /// The peak reserved heap bytes of each thread's oxide runtime (sampled every 32 runs), by thread.
    pub oxide_heap_peaks: Vec<(String, usize)>,
    /// Texts the oxide runtimes compiled.
    pub oxide_compiled: u64,
    /// Oxide runtimes dropped after a host function panicked.
    pub oxide_poisoned: u64,
    /// The process's oxide code images: made, loaded by a runtime instead of compiled (each a parse avoided), failed,
    /// and the bytes kept.
    pub oxide_images_made: u64,
    pub oxide_image_loads: u64,
    pub oxide_image_failures: u64,
    pub oxide_image_bytes: usize,
    /// Oxide runs that met something the bindings do not have before any host call, and ran on QuickJS instead.
    pub oxide_fallbacks: u64,
    /// Differences between the engines the difference check (`script_engine_check`) found.
    pub oxide_differences: u64,
    /// Texts the static check parsed.
    pub check_parses: u64,
    /// Scripts run by entry kind: (kind, on QuickJS, on quickjs-oxide).
    pub scripts_by_kind: Vec<(&'static str, u64, u64)>,
}

/// [`EngineStats`] of the process.
pub fn engine_stats() -> EngineStats {
    use std::sync::atomic::Ordering::Relaxed;
    let oxide = crate::script::oxide::stats();
    EngineStats {
        engines_started: counts::STARTED.load(Relaxed),
        threads: counts::THREADS.load(Relaxed),
        engines_live: counts::LIVE.load(Relaxed),
        engines_live_max: counts::LIVE_MAX.load(Relaxed),
        depth_max: crate::script::run::DEPTH_MAX.load(Relaxed),
        heap_bytes: counts::HEAP.load(Relaxed),
        heap_bytes_max: counts::HEAP_MAX.load(Relaxed),
        compiled: compiled::compiled_count(),
        shared_reads: compiled::shared_bytecode_reads(),
        oxide_engines_started: oxide.engines_started,
        oxide_engines_live: oxide.engines_live,
        oxide_engines_live_max: oxide.engines_live_max,
        oxide_heap_bytes: oxide.heap_bytes,
        oxide_heap_bytes_max: oxide.heap_bytes_max,
        oxide_heap_peaks: crate::script::oxide::thread_heap_peaks(),
        oxide_compiled: oxide.compiled,
        oxide_poisoned: oxide.poisoned,
        oxide_images_made: oxide.images.made,
        oxide_image_loads: oxide.images.loads,
        oxide_image_failures: oxide.images.failed,
        oxide_image_bytes: oxide.images.bytes,
        oxide_fallbacks: crate::script::engine::oxide_fallbacks(),
        oxide_differences: crate::script::engine::oxide_differences(),
        check_parses: crate::script::check::parses(),
        scripts_by_kind: crate::script::engine::scripts_by_kind(),
    }
}

fn add_heap(bytes: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    let now = counts::HEAP.fetch_add(bytes, Relaxed) + bytes;
    counts::HEAP_MAX.fetch_max(now, Relaxed);
}

fn sub_heap(bytes: usize) {
    counts::HEAP.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// QuickJS's heap on Rust's allocator ([`RustAllocator`]), its live bytes counted, so the engine knows its heap size
/// without `JS_ComputeMemoryUsage`, which walks every object (4% of a Mantle service call's time when it ran every 16th
/// script).
struct CountingAllocator(Rc<Cell<usize>>);

// SAFETY: every call goes to `RustAllocator`, which meets the trait's contract; the count only reads the sizes it gives.
unsafe impl Allocator for CountingAllocator {
    fn alloc(&mut self, size: usize) -> *mut u8 {
        let ptr = RustAllocator.alloc(size);
        if !ptr.is_null() {
            // SAFETY: `ptr` was just allocated by `RustAllocator`.
            let size = unsafe { RustAllocator::usable_size(ptr) };
            self.0.set(self.0.get() + size);
            add_heap(size);
        }
        ptr
    }

    fn calloc(&mut self, count: usize, size: usize) -> *mut u8 {
        let ptr = RustAllocator.calloc(count, size);
        if !ptr.is_null() {
            // SAFETY: `ptr` was just allocated by `RustAllocator`.
            let size = unsafe { RustAllocator::usable_size(ptr) };
            self.0.set(self.0.get() + size);
            add_heap(size);
        }
        ptr
    }

    unsafe fn dealloc(&mut self, ptr: *mut u8) {
        // SAFETY: the caller hands back a pointer this allocator gave, which `RustAllocator` made.
        let size = unsafe { RustAllocator::usable_size(ptr) };
        self.0.set(self.0.get().saturating_sub(size));
        sub_heap(size);
        // SAFETY: as above.
        unsafe { RustAllocator.dealloc(ptr) };
    }

    unsafe fn realloc(&mut self, ptr: *mut u8, new_size: usize) -> *mut u8 {
        // SAFETY: the caller hands back a pointer this allocator gave, which `RustAllocator` made.
        let old = unsafe { RustAllocator::usable_size(ptr) };
        // SAFETY: as above.
        let moved = unsafe { RustAllocator.realloc(ptr, new_size) };
        if !moved.is_null() {
            // SAFETY: `moved` was just allocated by `RustAllocator`.
            let new = unsafe { RustAllocator::usable_size(moved) };
            self.0.set((self.0.get() + new).saturating_sub(old));
            add_heap(new);
            sub_heap(old);
        }
        moved
    }

    unsafe fn usable_size(ptr: *mut u8) -> usize {
        // SAFETY: the caller hands a pointer this allocator gave, which `RustAllocator` made.
        unsafe { RustAllocator::usable_size(ptr) }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        counts::LIVE.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Engine {
    /// Reclaim cached code before the hard heap limit interrupts a valid script. Live user values remain rooted.
    fn reclaim_cache_pressure(&self) -> bool {
        let used = || i64::try_from(self.heap_bytes.get()).unwrap_or(i64::MAX);
        if used() < CACHE_HEAP_PRESSURE {
            return false;
        }
        self.runtime.run_gc();
        if used() < CACHE_HEAP_PRESSURE {
            return false;
        }
        self.clear_caches();
        self.runtime.run_gc();
        true
    }

    /// Drop the compiled code and the objects kept in this engine, to free its heap.
    fn clear_caches(&self) {
        self.modules.borrow_mut().clear();
        self.templates.borrow_mut().clear();
        self.scripts.borrow_mut().clear();
        self.units.borrow_mut().clear();
        self.objects.borrow_mut().clear();
    }
}

/// The object kept in the innermost engine under `name`, made by `make` the first time.
pub(crate) fn engine_object<'js>(
    ctx: &Ctx<'js>,
    name: &'static str,
    make: impl FnOnce(&Ctx<'js>) -> Result<Object<'js>, Error>,
) -> Result<Object<'js>, Error> {
    let engine = current_engine();
    let saved = engine
        .as_ref()
        .and_then(|engine| engine.objects.borrow().get(name).cloned());
    if let Some(saved) = saved {
        return saved.restore(ctx).map_err(js_error);
    }
    let made = make(ctx)?;
    if let Some(engine) = engine {
        engine
            .objects
            .borrow_mut()
            .insert(name, Persistent::save(ctx, made.clone()));
    }
    Ok(made)
}

/// Whether the error is QuickJS running out of the engine's memory.
fn is_out_of_memory(err: &Error) -> bool {
    err.message().contains("out of memory")
}

/// A list or map written back before a host call, kept so the script's object survives the call.
struct Released {
    value: Persistent<JsValue<'static>>,
    written: Value,
}

/// What a flush does with the tracked values after writing them back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flush {
    /// Keep tracking them, for a host call that only reads the stack.
    Keep,
    /// Release them, for a host call that may change the stack.
    Release,
    /// Forget them, at the end of a script or a screen unit.
    End,
}

#[derive(Clone, Copy)]
enum HelperKind {
    Object,
    Collection,
    String,
    Web,
    Security,
}

/// Push an empty frame on the thread-local context stack.
pub fn push_context_frame() {
    STACK.with(|stack| stack.borrow_mut().push());
}

/// A context-stack frame that pops when dropped.
pub struct ContextFrame;

impl ContextFrame {
    pub fn push() -> Self {
        push_context_frame();
        Self
    }

    /// Push a frame holding `entries`, without copying them in one by one. A `context` entry is left out, as a put of it
    /// is refused.
    pub fn push_entries(mut entries: indexmap::IndexMap<String, Value>) -> Self {
        entries.shift_remove("context");
        STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if stack.push_entries(entries).is_err() {
                stack.push();
            }
        });
        Self
    }

    /// Pop the frame and return what it holds.
    pub fn take(self) -> indexmap::IndexMap<String, Value> {
        std::mem::forget(self);
        pop_context_frame()
    }

    /// Leave the frame on the stack, for a host call that pushes it and a later one (`sri.popContext`) that pops it.
    pub fn keep(self) {
        std::mem::forget(self);
    }
}

impl Drop for ContextFrame {
    fn drop(&mut self) {
        STACK.with(|stack| {
            let _ = stack.borrow_mut().pop();
        });
    }
}

/// The context stack's depth when made. Dropped without [`StackMark::keep`], it pops the frames pushed since: a script
/// that threw between its `sri.pushContext()` and `sri.popContext()` leaves the stack as it found it.
pub(crate) struct StackMark {
    depth: Option<(usize, usize)>,
}

impl StackMark {
    pub(crate) fn new() -> Self {
        Self {
            depth: Some(STACK.with(|stack| stack.borrow().depth())),
        }
    }

    /// Leave the frames pushed since as they are.
    pub(crate) fn keep(mut self) {
        self.depth = None;
    }
}

impl Drop for StackMark {
    fn drop(&mut self) {
        if let Some(depth) = self.depth {
            STACK.with(|stack| {
                if let Ok(mut stack) = stack.try_borrow_mut() {
                    stack.truncate(depth);
                }
            });
        }
    }
}

/// An isolated context that pops when dropped, as Java's `ContextStack.pushContext`: the names below it are hidden, so
/// a service sees its own parameters and not its caller's.
pub struct IsolatedContext;

impl IsolatedContext {
    pub fn push() -> Self {
        STACK.with(|stack| stack.borrow_mut().push_context());
        Self
    }
}

impl Drop for IsolatedContext {
    fn drop(&mut self) {
        STACK.with(|stack| {
            let _ = stack.borrow_mut().pop_context();
        });
    }
}

/// Put `value` in the top frame.
/// The names set in the top context frame, as an entity ECA rule's `set-results` reads them.
pub fn top_context_entries() -> indexmap::IndexMap<String, Value> {
    STACK.with(|stack| stack.borrow().top_entries())
}

pub fn set_context_name(name: &str, value: Value) -> Result<(), Error> {
    STACK.with(|stack| stack.borrow_mut().put(name, value).map(|_| ()).map_err(Error::from))
}

/// Replace the thread-local stack with an empty one. The runtime stays.
pub fn clear_context_stack() {
    STACK.with(|stack| *stack.borrow_mut() = ContextStack::new());
}

/// What the script engines of this thread hold, summed over them. QuickJS allocates with the C library's `malloc`, so a
/// Rust global allocator does not see it; the `js_speed` test reads this instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineMemory {
    pub engines: usize,
    /// Bytes and blocks QuickJS has from `malloc` and not freed.
    pub malloc_size: i64,
    pub malloc_count: i64,
    /// QuickJS's own estimate of what its objects, strings, shapes and functions take.
    pub memory_used_size: i64,
    pub obj_count: i64,
    pub str_count: i64,
    pub str_size: i64,
    pub shape_count: i64,
    pub js_func_count: i64,
    pub js_func_code_size: i64,
    /// Entries and source bytes of the compiled script, screen unit and template caches.
    pub compiled_scripts: usize,
    pub compiled_script_bytes: usize,
    pub units: usize,
    pub unit_bytes: usize,
    pub templates: usize,
    /// This thread's quickjs-oxide runtime (0 or 1): the bytes its heap reserves and the live bytes its last
    /// collection left, its compiled units and their instruction, position and source bytes, and its
    /// compiled-expression cache's texts and their source bytes.
    pub oxide_engines: usize,
    pub oxide_reserved_bytes: usize,
    pub oxide_live_bytes: usize,
    pub oxide_code_units: usize,
    pub oxide_code_bytes: usize,
    pub oxide_compiled_scripts: usize,
    pub oxide_compiled_script_bytes: usize,
}

/// [`EngineMemory`] of this thread's engines, after a collection when `collect`. Not while a script runs on the thread.
pub fn engine_memory(collect: bool) -> EngineMemory {
    let engines: Vec<Rc<Engine>> =
        ENGINES.with(|engines| engines.borrow().iter().map(|(_, engine)| Rc::clone(engine)).collect());
    let mut memory = EngineMemory {
        engines: engines.len(),
        ..EngineMemory::default()
    };
    for engine in engines {
        if collect {
            engine.runtime.run_gc();
        }
        let usage = engine.runtime.memory_usage();
        memory.malloc_size += usage.malloc_size;
        memory.malloc_count += usage.malloc_count;
        memory.memory_used_size += usage.memory_used_size;
        memory.obj_count += usage.obj_count;
        memory.str_count += usage.str_count;
        memory.str_size += usage.str_size;
        memory.shape_count += usage.shape_count;
        memory.js_func_count += usage.js_func_count;
        memory.js_func_code_size += usage.js_func_code_size;
        let scripts = engine.scripts.borrow();
        memory.compiled_scripts += scripts.len();
        memory.compiled_script_bytes += scripts.bytes();
        let units = engine.units.borrow();
        memory.units += units.len();
        memory.unit_bytes += units.bytes();
        memory.templates += engine.templates.borrow().len();
    }
    let oxide = crate::script::oxide::engine_memory(collect);
    memory.oxide_engines = oxide.engines;
    memory.oxide_reserved_bytes = oxide.reserved_bytes;
    memory.oxide_live_bytes = oxide.live_bytes;
    memory.oxide_code_units = oxide.code_units;
    memory.oxide_code_bytes = oxide.code_bytes;
    memory.oxide_compiled_scripts = oxide.compiled_scripts;
    memory.oxide_compiled_script_bytes = oxide.compiled_script_bytes;
    memory
}

/// Run one script statement in the server runtime. The script is not strict.
pub fn run_script(source: &str) -> Result<(), Error> {
    if source.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    if crate::script::engine::script_engine() == crate::script::engine::ScriptEngine::Oxide {
        let ran = crate::script::engine::try_oxide(EntryKind::Script, source, |source, hash, _| {
            crate::script::oxide::run_script(source, hash)
        })?;
        if ran.is_some() {
            return Ok(());
        }
    }
    with_kind(EntryKind::Script, |ctx| eval_compiled::<JsValue>(ctx, source, false).map(|_| ()))
}

/// A source that starts with `{` is an object literal in an expression, not a block.
pub(crate) fn as_expression(source: &str) -> std::borrow::Cow<'_, str> {
    if source.trim_start().starts_with('{') {
        std::borrow::Cow::Owned(format!("({source}\n)"))
    } else {
        std::borrow::Cow::Borrowed(source)
    }
}

/// Evaluate a strict expression. Null, false, and a zero number are false. A decimal is true.
///
/// This is JavaScript truth, as an `<if>` in the actions has: an empty list or map is true, where Java's
/// `XmlAction.checkCondition` (`as Boolean`) makes it false. A rule condition and an `<if>` agree, and converted Groovy
/// tests a list's length instead (`groovy_convert`; the linter flags the rest, `lint_hazards`).
pub fn eval_condition(expression: &str) -> Result<bool, Error> {
    if expression.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let source = as_expression(expression);
    let quickjs = || {
        with_kind(EntryKind::Condition, |ctx| {
            with_widget_ec(ctx, expression, || {
                let value = eval_compiled::<JsValue>(ctx, &source, true)?;
                Ok(js_truthy(&value))
            })
        })
    };
    crate::script::engine::small_script(EntryKind::Condition, &source, crate::script::oxide::eval_condition, quickjs)
}

/// Expand a template. `${expression}` runs in this runtime. No `$`, or a trailing `$`, is unchanged.
pub fn expand(template: &str) -> Result<String, Error> {
    if !template.contains('$') || template.ends_with('$') {
        return Ok(template.to_string());
    }
    if template.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    if crate::script::engine::may_route() {
        // on quickjs-oxide the template literal itself, run as an expression: the same text QuickJS's function returns
        let literal = template_literal(template);
        let quickjs = || with_kind(EntryKind::Expand, |ctx| expand_in(ctx, template));
        return crate::script::engine::small_script(EntryKind::Expand, &literal, crate::script::oxide::eval_text, quickjs);
    }
    with_kind(EntryKind::Expand, |ctx| expand_in(ctx, template))
}

/// See [`upload::text_upload_to_js`].
pub use upload::text_upload_to_js;

pub fn with_ctx<R>(body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    with_kind(EntryKind::Host, body)
}

/// [`with_ctx`] in the thread's spec engine, where the test harness's code and globals are
/// ([`eval_named_value`]).
pub fn with_spec_ctx<R>(body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    crate::script::engine::require_quickjs(EntryKind::Spec)?;
    with_engine(EngineKind::Spec, body)
}

pub(super) fn eval_source<'js, T>(ctx: &Ctx<'js>, source: &str) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    eval_js(ctx, source)
}

/// Run one generated action script whose text and cache key were made once.
///
/// The script is not strict. A strict function would reject bare assignment, and
/// that assignment is how a plain value reaches the context stack.
pub(super) fn eval_script_text(script: &ScriptText) -> Result<(), Error> {
    if crate::script::engine::may_route() {
        let ran = crate::script::engine::try_oxide(EntryKind::Actions, script.text(), |source, hash, _| {
            crate::script::oxide::run_actions(source, hash, script.file_name())
        })?;
        if ran.is_some() {
            return Ok(());
        }
    }
    with_kind(EntryKind::Actions, |ctx| eval_compiled_text::<JsValue>(ctx, script, false).map(|_| ()))
}

/// Same prefix rules as [`expand`], on the callback's context.
///
/// [`expand`] enters `Context::with` again, and that lock is already held here.
fn expand_here(ctx: &Ctx<'_>, template: &str) -> Result<String, Error> {
    if !template.contains('$') || template.ends_with('$') {
        return Ok(template.to_string());
    }
    if template.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    expand_in(ctx, template)
}

fn with_context<R>(body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    with_kind(EntryKind::Host, body)
}

/// A script of `kind` in the thread's main QuickJS engine. Under `script_engine=oxide` a kind the oxide bindings do not
/// cover is an error naming it ([`crate::script::engine::require_quickjs`]).
fn with_kind<R>(kind: EntryKind, body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    crate::script::engine::require_quickjs(kind)?;
    with_engine(EngineKind::Main, body)
}

/// Which of a thread's engines a script runs in. Every service, screen and action script runs in the main one, nested
/// ones included ([`with_nested_context`]). The test harness's spec code runs in an engine of its own, so the globals
/// it defines (`call`, `login`, ...) are not in front of the context stack for the services a spec calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineKind {
    Main,
    Spec,
}

/// The thread's engine of `kind`, started the first time.
fn engine_of(kind: EngineKind) -> Result<Rc<Engine>, Error> {
    if let Some(engine) = ENGINES.with(|engines| {
        engines
            .borrow()
            .iter()
            .find(|(of, _)| *of == kind)
            .map(|(_, engine)| Rc::clone(engine))
    }) {
        return Ok(engine);
    }
    let started = Rc::new(Engine::start()?);
    if kind == EngineKind::Main {
        counts::THREADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    ENGINES.with(|engines| engines.borrow_mut().push((kind, Rc::clone(&started))));
    Ok(started)
}

/// The engine of the innermost running script given back when it ends, however it ends.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        RUNNING.with(|running| {
            running.borrow_mut().pop();
        });
    }
}

fn with_engine<R>(kind: EngineKind, body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    let depth = DEPTH.with(Cell::get);
    if depth >= MAX_SCRIPT_DEPTH {
        return Err(Error::new("scripts are nested too deeply"));
    }
    let stack = stack_budget(depth)?;
    crate::script::engine::cross_into_quickjs()?;
    let engine = engine_of(kind)?;
    let entered = RUNNING.with(|running| running.borrow().iter().any(|held| Rc::ptr_eq(held, &engine)));
    RUNNING.with(|running| running.borrow_mut().push(Rc::clone(&engine)));
    let _running = Running;
    let _tag = EngineTag::QuickJs.enter();
    if entered {
        return with_nested_context(&engine, depth, body);
    }
    // The heap's live bytes are counted as it allocates ([`CountingAllocator`]), so every entry checks them. The hard
    // limit and its error recovery still apply.
    engine.reclaim_cache_pressure();
    // QuickJS counts the stack from where the runtime is entered, here: the nested scripts this one runs share it.
    engine.runtime.set_max_stack_size(stack);
    let _limit = (depth == 0).then(ScriptLimit::arm);
    let _guard = crate::script::run::enter_depth(depth);
    engine.context.with(|ctx| {
        let _run = RunValues {
            engine: &engine,
            outer: None,
        };
        let result = body(&ctx);
        // The caches live in the engine's heap: when it is full, they go, so the next script on the thread can run.
        if result.as_ref().is_err_and(is_out_of_memory) {
            engine.clear_caches();
            ctx.run_gc();
        }
        let flushed = flush_tracked(&ctx, &engine, Flush::End);
        // the modules kept for this run go with it: the next run makes them again
        engine.modules.borrow_mut().clear();
        // what the outermost script opened with `send()` and did not read or forward goes with it
        if depth == 0 {
            crate::web::upstream::clear();
        }
        match (result, flushed) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(err), _) | (Ok(_), Err(err)) => Err(err),
        }
    })
}

/// A script run by a host call of a running script (a service it calls runs another script): in the thread's one
/// engine and context, entered again through the lock the outermost script's `Context::with` holds
/// (`Ctx::from_raw`), not in a runtime of its own. Its compiled scripts, objects and heap are the outer one's; its run
/// state is its own ([`OuterRun`]), what it read written back to its own context frame at its end. A script file's own
/// declarations are in its function ([`crate::script::parse::file_function`]), so they do not meet the caller's; a
/// screen mode's render state is given back by its outermost unit's epilogue.
fn with_nested_context<R>(
    engine: &Rc<Engine>,
    depth: usize,
    body: impl FnOnce(&Ctx) -> Result<R, Error>,
) -> Result<R, Error> {
    // SAFETY: `engine` is running on this thread (it is in `RUNNING` below this call), so its outermost
    // `Context::with` holds the runtime's lock for as long as this call lasts, and the `Ctx` made here does not
    // outlive it.
    let ctx = unsafe { Ctx::from_raw(engine.context.as_raw()) };
    // The caller's run state is set aside as it is, as a runtime of its own would leave it, and given back after: its
    // tracked values (written to its own context frame at its next host call or its end), its released ones, its
    // screen unit depth (a render this run makes starts afresh) and its flush in progress.
    let outer = OuterRun {
        engine,
        tracked: engine.tracked.take(),
        released: engine.released.take(),
        unit_depth: engine.unit_depth.replace(0),
        flushing: engine.flushing.replace(false),
    };
    let guard = crate::script::run::enter_depth(depth);
    let result = body(&ctx);
    let flushed = flush_tracked(&ctx, engine, Flush::End);
    drop(guard);
    drop(outer);
    match (result, flushed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(err), _) | (Ok(_), Err(err)) => Err(err),
    }
}

/// The run state of a script a nested run ([`with_nested_context`]) set aside, given back when it ends, also by an
/// error or a panic; what the nested run left is dropped.
struct OuterRun<'a> {
    engine: &'a Engine,
    tracked: HashMap<String, Persistent<JsValue<'static>>>,
    released: HashMap<String, Released>,
    unit_depth: u32,
    flushing: bool,
}

impl Drop for OuterRun<'_> {
    fn drop(&mut self) {
        if let Ok(mut tracked) = self.engine.tracked.try_borrow_mut() {
            *tracked = std::mem::take(&mut self.tracked);
        }
        if let Ok(mut released) = self.engine.released.try_borrow_mut() {
            *released = std::mem::take(&mut self.released);
        }
        self.engine.unit_depth.set(self.unit_depth);
        self.engine.flushing.set(self.flushing);
    }
}

/// Write each tracked list and map back to the context stack.
///
/// A host call that may change the stack releases them: the next bare read must see what the call left. The
/// script can still hold a released object, as `list.push(call())` evaluates `list` first, so a released value is
/// tracked again, the same JS object, at its next read by name or at the end of the script while the stack still
/// holds the value written. A host call that put a different value there wins and the held object is dropped,
/// even when the script changed it after the call too: there is one value per name, and the host's is the later
/// assignment, as when a Java callee replaces a list the script holds.
///
/// Released values are not checked at each flush: a list held across a loop of calls would be compared and
/// converted at every call. So a change through a held object after a call, before its name is read again, is
/// not on the stack for host code that reads the stack directly until the end of the script.
fn flush_tracked(ctx: &Ctx<'_>, engine: &Engine, mode: Flush) -> Result<(), Error> {
    if engine.flushing.replace(true) {
        // Inside this engine's own flush (see `context_combined_map`): the values are being written already.
        return Ok(());
    }
    let _flushing = Flushing(&engine.flushing);
    if mode == Flush::End {
        readopt_all(engine);
    }
    let entries: Vec<(String, Persistent<JsValue<'static>>)> = if mode != Flush::Keep {
        engine.tracked.borrow_mut().drain().collect()
    } else {
        engine
            .tracked
            .borrow()
            .iter()
            .map(|(name, saved)| (name.clone(), saved.clone()))
            .collect()
    };
    let mut first = None;
    for (name, saved) in entries {
        let restored = saved.clone().restore(ctx).map_err(|err| js_caught(ctx, err));
        // What the script did not change is on the stack already: no conversion, no write. A release still notes
        // what the stack holds, for `readopt`.
        if let Ok(value) = &restored {
            let unchanged = STACK.with(|stack| {
                let stack = stack.borrow();
                stack.with_value(&name, |held| {
                    js_matches_host(value, held).then(|| (mode == Flush::Release).then(|| held.clone()))
                })
            });
            if let Some(Some(written)) = unchanged {
                if let Some(written) = written {
                    engine
                        .released
                        .borrow_mut()
                        .insert(name.clone(), Released { value: saved, written });
                }
                continue;
            }
        }
        let outcome = restored.and_then(|value| js_value_to_host(ctx, &value));
        match outcome {
            Ok(host) => {
                if mode == Flush::Release {
                    let written = host.clone();
                    engine
                        .released
                        .borrow_mut()
                        .insert(name.clone(), Released { value: saved, written });
                }
                STACK.with(|stack| {
                    let _ = stack.borrow_mut().put_nearest(&name, host);
                });
            }
            Err(err) => {
                first.get_or_insert(err);
            }
        }
    }
    match first {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Clears an engine's `flushing` flag when the flush ends, also by an error or a panic.
struct Flushing<'a>(&'a Cell<bool>);

impl Drop for Flushing<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// Track a released value again when the stack still holds what was written for it. Returns its object.
fn readopt(engine: &Engine, name: &str) -> Option<Persistent<JsValue<'static>>> {
    let released = engine.released.borrow_mut().remove(name)?;
    if !STACK.with(|stack| stack.borrow().holds(name, &released.written)) {
        return None;
    }
    engine
        .tracked
        .borrow_mut()
        .insert(name.to_string(), released.value.clone());
    Some(released.value)
}

/// Track a released value of the innermost script again, for host code that reads that name from the stack.
fn readopt_name(name: &str) {
    if let Some(engine) = current_engine() {
        readopt(&engine, name);
    }
}

/// Track again each released value the stack still holds.
fn readopt_all(engine: &Engine) {
    let names: Vec<String> = engine.released.borrow().keys().cloned().collect();
    for name in names {
        readopt(engine, &name);
    }
}

/// Flush the innermost script's tracked values, for a host call that reads the stack. `clear` releases them, for
/// a host call that may also change it.
fn flush_current(ctx: &Ctx<'_>, clear: bool) -> rquickjs::Result<()> {
    let mode = if clear { Flush::Release } else { Flush::Keep };
    match current_engine() {
        Some(engine) => flush_tracked(ctx, &engine, mode).map_err(|err| Exception::throw_message(ctx, err.message())),
        None => Ok(()),
    }
}

/// Run a screen unit in the innermost script. The caller's values are released before and set aside, and what
/// the unit read is written back and forgotten after, as at the end of a script: the unit's own variables are
/// gone, and the next unit may run under a different context frame.
fn in_unit_scope<T>(ctx: &Ctx<'_>, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let engine = current_engine().ok_or_else(|| Error::new("no script engine is running"))?;
    flush_tracked(ctx, &engine, Flush::Release)?;
    let scope = RunValues {
        engine: &engine,
        outer: Some(engine.released.take()),
    };
    let result = body();
    let flushed = flush_tracked(ctx, &engine, Flush::End);
    drop(scope);
    result.and_then(|value| flushed.map(|()| value))
}

/// The tracked and released values of a script run or a screen unit, forgotten when it ends, also by an error or a
/// panic: a value left behind would be what the next script on this thread's engine reads by that name. A unit puts
/// back the released values of the script around it.
struct RunValues<'a> {
    engine: &'a Engine,
    outer: Option<HashMap<String, Released>>,
}

impl Drop for RunValues<'_> {
    fn drop(&mut self) {
        if let Ok(mut tracked) = self.engine.tracked.try_borrow_mut() {
            tracked.clear();
        }
        if let Ok(mut released) = self.engine.released.try_borrow_mut() {
            *released = self.outer.take().unwrap_or_default();
        }
    }
}

/// The value an inert module at `location` gave earlier in this outermost run, when made from the same text.
fn kept_module<'js>(ctx: &Ctx<'js>, location: &str, text: &Arc<str>) -> Option<JsValue<'js>> {
    let engine = current_engine()?;
    let modules = engine.modules.borrow();
    let (kept_text, value) = modules.get(location)?;
    if !Arc::ptr_eq(kept_text, text) && **kept_text != **text {
        return None;
    }
    value.clone().restore(ctx).ok()
}

/// Keep an inert module's value for the rest of the outermost run ([`kept_module`]).
fn keep_module<'js>(ctx: &Ctx<'js>, location: &str, text: Arc<str>, value: &JsValue<'js>) {
    if let Some(engine) = current_engine() {
        engine
            .modules
            .borrow_mut()
            .insert(location.to_string(), (text, Persistent::save(ctx, value.clone())));
    }
}

/// The engine of the innermost running script.
fn current_engine() -> Option<Rc<Engine>> {
    RUNNING.with(|running| running.borrow().last().cloned())
}

impl Engine {
    fn start() -> Result<Self, Error> {
        let heap_bytes = Rc::new(Cell::new(0));
        counts::STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let live = counts::LIVE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        counts::LIVE_MAX.fetch_max(live, std::sync::atomic::Ordering::Relaxed);
        let runtime = Runtime::new_with_alloc(CountingAllocator(Rc::clone(&heap_bytes))).map_err(js_error)?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        runtime.set_interrupt_handler(Some(Box::new(|| {
            DEADLINE
                .with(Cell::get)
                .is_some_and(|deadline| Instant::now() >= deadline)
        })));
        let context = Context::full(&runtime).map_err(js_error)?;
        let helpers = context.with(|ctx| install_context(&ctx))?;
        Ok(Self {
            heap_bytes,
            tracked: RefCell::new(HashMap::new()),
            released: RefCell::new(HashMap::new()),
            templates: RefCell::new(super::bounded::BoundedMap::with_bytes(
                MAX_CACHED_TEMPLATES,
                MAX_CACHED_TEMPLATE_BYTES,
            )),
            scripts: RefCell::new(super::bounded::BoundedMap::with_bytes(
                MAX_COMPILED_SCRIPTS,
                MAX_COMPILED_SCRIPT_BYTES,
            )),
            units: RefCell::new(super::bounded::BoundedMap::with_bytes(
                MAX_CACHED_TEMPLATES,
                MAX_CACHED_UNIT_BYTES,
            )),
            modules: RefCell::new(HashMap::new()),
            objects: RefCell::new(HashMap::new()),
            runtimes: RefCell::new(std::collections::HashSet::new()),
            unit_depth: Cell::new(0),
            context_proxy: helpers.context_proxy,
            flushing: Cell::new(false),
            object_utilities: helpers.object_utilities,
            collection_utilities: helpers.collection_utilities,
            string_utilities: helpers.string_utilities,
            web_utilities: helpers.web_utilities,
            user_security_checks: helpers.user_security_checks,
            context,
            runtime,
        })
    }
}

struct Helpers {
    context_proxy: Persistent<JsValue<'static>>,
    object_utilities: Persistent<Object<'static>>,
    collection_utilities: Persistent<Object<'static>>,
    string_utilities: Persistent<Object<'static>>,
    web_utilities: Persistent<Object<'static>>,
    user_security_checks: Persistent<Object<'static>>,
}

fn install_context(ctx: &Ctx<'_>) -> Result<Helpers, Error> {
    let object_utilities = Object::new(ctx.clone()).map_err(js_error)?;
    let collection_utilities = Object::new(ctx.clone()).map_err(js_error)?;
    let string_utilities = Object::new(ctx.clone()).map_err(js_error)?;
    let web_utilities = Object::new(ctx.clone()).map_err(js_error)?;
    let user_security_checks = Object::new(ctx.clone()).map_err(js_error)?;
    let builtins = eval_js::<JsValue>(ctx, "Object.getOwnPropertyNames(globalThis)")?;
    ec_extra::fill_helpers(ctx, &string_utilities, &collection_utilities, &web_utilities)?;
    ec_extra::fill_security_checks(ctx, &user_security_checks)?;
    install_ec(ctx)?;
    install_helpers(ctx)?;
    bind::install_shims(ctx, &string_utilities, &collection_utilities, &web_utilities)?;
    install_entity_prototypes(ctx)?;
    freeze_host_globals(ctx, builtins)?;
    install_proxy(ctx)?;
    let context_proxy = make_context_proxy(ctx)?;
    Ok(Helpers {
        context_proxy: Persistent::save(ctx, context_proxy),
        object_utilities: Persistent::save(ctx, object_utilities),
        collection_utilities: Persistent::save(ctx, collection_utilities),
        string_utilities: Persistent::save(ctx, string_utilities),
        web_utilities: Persistent::save(ctx, web_utilities),
        user_security_checks: Persistent::save(ctx, user_security_checks),
    })
}

/// Make the globals the host installed (`ec`, `sri`, `compare`, `toDecimal` and the rest) read-only, so an assignment
/// in one script does not change them for the later scripts on the thread. An assignment throws, in sloppy code too:
/// a script that used such a name for a context value would otherwise lose the value without an error. `ec` and `sri`,
/// read on every statement of actions and screens, are plain read-only values instead, as a getter call costs about half
/// again on each read; `ec` is configurable for the wrapper a widget check puts in its place (see [`set_ec`]).
const FREEZE_HOST_GLOBALS: &str = r#"(function (builtins) {
    const known = new Set(builtins);
    for (const name of Object.getOwnPropertyNames(globalThis)) {
        if (known.has(name)) continue;
        if (name === "ec" || name === "sri") {
            Object.defineProperty(globalThis, name, { writable: false, configurable: name === "ec" });
            continue;
        }
        const value = globalThis[name];
        Object.defineProperty(globalThis, name, {
            get() { return value; },
            set() { throw new TypeError(name + " is a host global and cannot be assigned"); },
            enumerable: true,
            configurable: false
        });
    }
})"#;

fn freeze_host_globals<'js>(ctx: &Ctx<'js>, builtins: JsValue<'js>) -> Result<(), Error> {
    let freeze: Function = eval_js(ctx, FREEZE_HOST_GLOBALS)?;
    freeze
        .call::<_, ()>((builtins,))
        .catch(ctx)
        .map_err(|caught| Error::new(format!("host globals: {caught}")))
}

/// Bare names resolve through the prototype of `globalThis`, a [`HostGlobalNames`]: a name the global object does not
/// have reaches it, and it reads and writes the context stack.
fn install_proxy(ctx: &Ctx<'_>) -> Result<(), Error> {
    let names = Class::instance(ctx.clone(), HostGlobalNames {}).map_err(js_error)?;
    ctx.globals().set_prototype(Some(&names)).map_err(js_error)?;
    Ok(())
}

/// The prototype of `globalThis` that resolves bare names. An exotic object, so QuickJS calls its hooks directly with
/// the property's atom, the same hooks a Proxy's traps run behind; a Proxy's trap was also a JavaScript call into Rust
/// with its arguments converted, and that was most of the cost of a bare name. Its get, set and has are
/// [`read_name`], [`write_name`] and the global object's own properties, as the traps were. A symbol is not a name.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(exotic)]
pub(super) struct HostGlobalNames {}

/// The exotic methods `#[rquickjs::class(exotic)]` calls for [`HostGlobalNames`]. The receiver of a set is ignored,
/// as the Proxy's set trap ignored it.
#[allow(non_snake_case)]
mod __impl_exotic_HostGlobalNames__ {
    use super::*;
    use rquickjs::class::{JsCell, PropertyDescriptor, PropertyName};
    use rquickjs::Atom;

    pub(crate) struct ExoticImpl;

    /// The name an atom is, or `None` for a symbol.
    fn name_of<'js>(atom: &Atom<'js>) -> rquickjs::Result<Option<(super::AtomText<'js>, JsValue<'js>)>> {
        let key = atom.to_value()?;
        Ok(super::AtomText::of(&key)?.map(|text| (text, key)))
    }

    impl ExoticImpl {
        pub fn exotic_get_property<'js>(
            _this: &JsCell<'js, HostGlobalNames>,
            ctx: &Ctx<'js>,
            atom: Atom<'js>,
            _receiver: JsValue<'js>,
        ) -> rquickjs::Result<JsValue<'js>> {
            host_guard(ctx, || match name_of(&atom)? {
                Some((name, _)) => read_name(ctx, &name),
                None => Undefined.into_js(ctx),
            })
        }

        pub fn exotic_set_property<'js>(
            _this: &JsCell<'js, HostGlobalNames>,
            ctx: &Ctx<'js>,
            atom: Atom<'js>,
            _receiver: JsValue<'js>,
            value: JsValue<'js>,
        ) -> rquickjs::Result<bool> {
            let written = host_guard(ctx, || {
                if let Some((name, _)) = name_of(&atom)? {
                    write_name(ctx, &name, &value)?;
                }
                Ok(true)
            });
            entities::keep_assigned(value);
            written
        }

        pub fn exotic_delete_property<'js>(
            _this: &JsCell<'js, HostGlobalNames>,
            _ctx: &Ctx<'js>,
            _atom: Atom<'js>,
        ) -> rquickjs::Result<bool> {
            // as the Proxy's empty target answered
            Ok(true)
        }

        pub fn exotic_has_property<'js>(
            _this: &JsCell<'js, HostGlobalNames>,
            ctx: &Ctx<'js>,
            atom: Atom<'js>,
        ) -> rquickjs::Result<bool> {
            host_guard(ctx, || match name_of(&atom)? {
                Some((_, key)) => Ok(!global_has_own_key(ctx, &key)?),
                None => Ok(false),
            })
        }

        pub fn exotic_get_own_property<'js>(
            _this: &JsCell<'js, HostGlobalNames>,
            _ctx: &Ctx<'js>,
            _atom: Atom<'js>,
        ) -> rquickjs::Result<Option<PropertyDescriptor<'js>>> {
            Ok(None)
        }

        pub fn exotic_get_own_property_names<'js>(
            _this: &JsCell<'js, HostGlobalNames>,
            _ctx: &Ctx<'js>,
        ) -> rquickjs::Result<Vec<PropertyName<'js>>> {
            Ok(Vec::new())
        }
    }
}

/// The text of a property name, read in place: QuickJS hands out an ASCII string's own bytes, so a name read on each
/// bare-name or field access makes no copy (a name with other characters is converted on QuickJS's heap).
pub(crate) struct AtomText<'js>(rquickjs::CString<'js>);

impl<'js> AtomText<'js> {
    /// The text of a property key, `None` for a symbol.
    pub(crate) fn of(key: &JsValue<'js>) -> rquickjs::Result<Option<Self>> {
        if key.is_symbol() {
            return Ok(None);
        }
        let text = match key.as_string() {
            Some(text) => text.clone(),
            // a number key (an index) as its text
            None => rquickjs::convert::Coerced::<rquickjs::String<'js>>::from_js(key.ctx(), key.clone())?.0,
        };
        Ok(Some(Self(text.to_cstring()?)))
    }
}

impl std::ops::Deref for AtomText<'_> {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl PartialEq<&str> for AtomText<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_str() == *other
    }
}

/// A bare `name` read: the script's own object of a list, map or decimal it already read, else the stack's value,
/// tracked when it is one of those. A decimal is tracked so each read is not a new host object; it does not change, so
/// the flush finds it as it was and writes nothing. `context` is the stack itself. A missing name is a helper object by that name, else null.
fn read_name<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<JsValue<'js>> {
    let ctx = ctx.clone();
    if let Some(engine) = current_engine() {
        let saved = engine.tracked.borrow().get(name).cloned();
        if let Some(saved) = saved.or_else(|| readopt(&engine, name)) {
            return saved.restore(&ctx);
        }
    }
    // In place, without the copy `stack_get` makes, unless the conversion runs script code that could reach the stack.
    let found = STACK.with(|stack| {
        stack.borrow().with_value(name, |value| {
            let track = matches!(value, Value::List(_) | Value::Map(_) | Value::Decimal(_));
            (!converts_with_script(value)).then(|| host_ref_to_js(&ctx, value).map(|js| (js, track)))
        })
    });
    if let Some(Some(converted)) = found {
        let (js, track) = converted?;
        if track {
            if let Some(engine) = current_engine() {
                engine
                    .tracked
                    .borrow_mut()
                    .insert(name.to_string(), Persistent::save(&ctx, js.clone()));
            }
        }
        return Ok(js);
    }
    match stack_get(name) {
        Lookup::Value(value @ (Value::List(_) | Value::Map(_) | Value::Decimal(_))) => {
            let js = host_to_js(&ctx, value)?;
            if let Some(engine) = current_engine() {
                engine
                    .tracked
                    .borrow_mut()
                    .insert(name.to_string(), Persistent::save(&ctx, js.clone()));
            }
            Ok(js)
        }
        Lookup::Value(value) => host_to_js(&ctx, value),
        Lookup::Context => context_proxy(&ctx),
        Lookup::Missing => match helper_object(&ctx, name)? {
            Some(helper) => helper.into_js(&ctx),
            None => Null.into_js(&ctx),
        },
    }
}

/// A bare `name = value`: the value goes on the stack's top frame, and a plain list or map stays the script's object.
fn write_name<'js>(ctx: &Ctx<'js>, name: &str, value: &JsValue<'js>) -> rquickjs::Result<()> {
    let ctx = ctx.clone();
    if let Some(engine) = current_engine() {
        engine.tracked.borrow_mut().remove(name);
        engine.released.borrow_mut().remove(name);
    }
    let host = js_to_host(&ctx, value)?;
    // A list or map put under a name stays the script's object, so `entry = list[i]; entry.field = x` changes the
    // element of the list, as it does in Java where both are the one Map. It is written to the stack at the next flush.
    let keep = matches!(host, Value::List(_) | Value::Map(_)) && is_plain_collection(value);
    STACK.with(|stack| {
        stack
            .borrow_mut()
            .put(name, host)
            .map(|_| ())
            .map_err(|err| Exception::throw_message(&ctx, err.message()))
    })?;
    if keep {
        if let Some(engine) = current_engine() {
            engine
                .tracked
                .borrow_mut()
                .insert(name.to_string(), Persistent::save(&ctx, value.clone()));
        }
    }
    Ok(())
}

/// An array or an object made by `{}` or `new Object()`, the values that convert to a host list or map and back
/// as the same shape: not a `Set`, a `Map`, a class instance, an exotic host object or a proxy (`context`, an entity).
fn is_plain_collection(value: &JsValue<'_>) -> bool {
    if value.as_proxy().is_some() {
        return false;
    }
    if value.is_array() {
        return true;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    let ctx = object.ctx();
    let plain: Option<Object> = ctx
        .globals()
        .get("Object")
        .ok()
        .and_then(|object: Object| object.get("prototype").ok());
    match (object.get_prototype(), plain) {
        (Some(prototype), Some(plain)) => prototype == plain,
        _ => false,
    }
}

/// Whether `globalThis` has the property `key` (a string) as its own, asked of QuickJS directly: the `has` trap runs
/// for every bare name a script reads or writes.
fn global_has_own_key(ctx: &Ctx<'_>, key: &JsValue<'_>) -> rquickjs::Result<bool> {
    let globals = ctx.globals();
    // SAFETY: the context, the global object and the key are alive for the calls; the atom made is freed here.
    let found = unsafe {
        let atom = rquickjs::qjs::JS_ValueToAtom(ctx.as_raw().as_ptr(), key.as_raw());
        if atom == rquickjs::qjs::JS_ATOM_NULL {
            // the conversion threw; the exception is pending
            return Err(rquickjs::Error::Exception);
        }
        let found =
            rquickjs::qjs::JS_GetOwnProperty(ctx.as_raw().as_ptr(), std::ptr::null_mut(), globals.as_raw(), atom);
        rquickjs::qjs::JS_FreeAtom(ctx.as_raw().as_ptr(), atom);
        found
    };
    if found < 0 {
        return Err(rquickjs::Error::Exception);
    }
    Ok(found > 0)
}

/// The target of the `context` proxy. It marks the proxy for [`js_value_to_host`], which converts it to the combined
/// map of the visible names, as Java passes the stack itself to a call that takes a `Map`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostContextStack {}

/// The bare name `context`: the stack itself, as Java's `ContextStack`. Each trap is the bare-name proxy's: `context.x`
/// reads as a bare `x` (`read`), `context.x = v` is a bare `x = v` (`write`), `'x' in context` holds for a visible name
/// (`has`), `Object.keys(context)` and a spread list the visible names (`names`) and `delete context.x` removes the name
/// from the top frame (`remove`). The `Map` methods of Java's stack that ported scripts call (`context.get(name)`,
/// `context.put(name, value)`) are there for a name no frame holds: a context value of that name wins, as a bare one.
const CONTEXT_PROXY: &str = r#"(function (target, read, write, has, names, remove) {
    const methods = {
        get: (name) => read(String(name)),
        put: (name, value) => { const old = read(String(name)); write(String(name), value); return old; },
        putAll: (map) => { for (const [key, value] of Object.entries(map || {})) write(key, value); },
        remove: (name) => { const old = read(String(name)); remove(String(name)); return old; },
        containsKey: (name) => has(String(name)),
        keySet: () => names(),
        getCombinedMap: () => { const out = {}; for (const key of names()) out[key] = read(key); return out; }
    };
    return new Proxy(target, {
        get(target, key) {
            if (typeof key !== "string") return undefined;
            if (!has(key) && Object.prototype.hasOwnProperty.call(methods, key)) return methods[key];
            return read(key);
        },
        set(target, key, value) { if (typeof key === "string") write(key, value); return true; },
        deleteProperty(target, key) { if (typeof key === "string") remove(key); return true; },
        has(target, key) { return typeof key === "string" && (key === "context" || has(key)); },
        ownKeys() { return names(); },
        getOwnPropertyDescriptor(target, key) {
            return typeof key === "string" && has(key)
                ? { value: read(key), writable: true, enumerable: true, configurable: true } : undefined;
        }
    });
})"#;

fn make_context_proxy<'js>(ctx: &Ctx<'js>) -> Result<JsValue<'js>, Error> {
    let target = caught(ctx, "context", Class::instance(ctx.clone(), HostContextStack {}))?;
    let read = caught(ctx, "context", Function::new(ctx.clone(), context_read))?;
    let write = caught(ctx, "context", Function::new(ctx.clone(), context_write))?;
    let has = caught(ctx, "context", Function::new(ctx.clone(), context_has))?;
    let names = caught(ctx, "context", Function::new(ctx.clone(), context_names))?;
    let remove = caught(ctx, "context", Function::new(ctx.clone(), context_remove))?;
    let make: Function = eval_js(ctx, CONTEXT_PROXY)?;
    caught(ctx, "context", make.call((target, read, write, has, names, remove)))
}

/// The `context` proxy of the running engine, made once with it.
fn context_proxy<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    match current_engine() {
        Some(engine) => engine.context_proxy.clone().restore(ctx),
        None => make_context_proxy(ctx).map_err(|err| Exception::throw_message(ctx, err.message())),
    }
}

fn context_read<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || read_name(&ctx, &name))
}

fn context_write<'js>(ctx: Ctx<'js>, name: String, value: JsValue<'js>) -> rquickjs::Result<()> {
    host_guard(&ctx, || write_name(&ctx, &name, &value))
}

fn context_has(ctx: Ctx<'_>, name: String) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(STACK.with(|stack| stack.borrow().contains_key(&name))))
}

fn context_names<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let names = STACK.with(|stack| stack.borrow().combined_keys());
        host_to_js(&ctx, Value::List(names.into_iter().map(Value::String).collect()))
    })
}

/// `delete context.x`: the name goes from the top frame, as Java's `ContextStack.remove`, and the script's object of it
/// is forgotten.
fn context_remove(ctx: Ctx<'_>, name: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        if let Some(engine) = current_engine() {
            engine.tracked.borrow_mut().remove(&name);
            engine.released.borrow_mut().remove(&name);
        }
        STACK.with(|stack| {
            stack.borrow_mut().remove(&name);
        });
        Ok(())
    })
}

/// The combined map of the visible names, for `context` passed to a host call that takes a map: as Java passes the
/// stack itself. The script's tracked lists and maps are written first, so the map has what the script changed.
pub(super) fn context_combined_map(ctx: &Ctx<'_>) -> Result<Value, Error> {
    if let Some(engine) = current_engine() {
        if !engine.flushing.get() {
            readopt_all(&engine);
            flush_tracked(ctx, &engine, Flush::Keep)?;
        }
    }
    Ok(Value::Map(STACK.with(|stack| stack.borrow().combined_map())))
}

fn stack_get(name: &str) -> Lookup {
    STACK.with(|stack| stack.borrow().get(name))
}

fn helper_kind(name: &str) -> Option<HelperKind> {
    match name {
        "ObjectUtilities" => Some(HelperKind::Object),
        "CollectionUtilities" => Some(HelperKind::Collection),
        "StringUtilities" => Some(HelperKind::String),
        "WebUtilities" => Some(HelperKind::Web),
        "UserSecurityChecks" => Some(HelperKind::Security),
        _ => None,
    }
}

fn helper_object<'js>(ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<Option<Object<'js>>> {
    let Some(kind) = helper_kind(name) else {
        return Ok(None);
    };
    let persistent = current_engine().map(|engine| match kind {
        HelperKind::Object => engine.object_utilities.clone(),
        HelperKind::Collection => engine.collection_utilities.clone(),
        HelperKind::String => engine.string_utilities.clone(),
        HelperKind::Web => engine.web_utilities.clone(),
        HelperKind::Security => engine.user_security_checks.clone(),
    });
    let Some(persistent) = persistent else {
        return Ok(None);
    };
    persistent.restore(ctx).map(Some)
}

fn eval_js<'js, T>(ctx: &Ctx<'js>, source: &str) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    // Compiled once for the process: each engine that starts installs the same host scripts.
    compiled::eval_shared::<T>(ctx, source).map_err(|err| Error::new(err.message().to_string()))
}

fn expand_in(ctx: &Ctx<'_>, template: &str) -> Result<String, Error> {
    if let Some(function) = restore_template(ctx, template)? {
        return with_widget_ec(ctx, template, || call_template(ctx, function));
    }
    let function = eval_js::<Function>(ctx, &template_source(template))?;
    save_template(ctx, template, &function);
    with_widget_ec(ctx, template, || call_template(ctx, function))
}

/// Run a compiled fancy template function and return its text. The function is kept per engine.
pub(crate) fn run_template_function(source: &str) -> Result<String, Error> {
    with_kind(EntryKind::Template, |ctx| run_template_function_here(ctx, source))
}

/// [`run_template_function`] on the callback's context, for a host call made from a running script.
pub(crate) fn run_template_function_here(ctx: &Ctx<'_>, source: &str) -> Result<String, Error> {
    let function = match restore_template(ctx, source)? {
        Some(function) => function,
        None => {
            let function = eval_js::<Function>(ctx, source)?;
            save_template(ctx, source, &function);
            function
        }
    };
    call_template(ctx, function)
}

fn restore_template<'js>(ctx: &Ctx<'js>, template: &str) -> Result<Option<Function<'js>>, Error> {
    let persistent = current_engine().and_then(|engine| engine.templates.borrow_mut().get(template));
    let Some(persistent) = persistent else {
        return Ok(None);
    };
    persistent.restore(ctx).map(Some).map_err(js_error)
}

fn save_template<'js>(ctx: &Ctx<'js>, template: &str, function: &Function<'js>) {
    let saved = Persistent::save(ctx, function.clone());
    if let Some(engine) = current_engine() {
        engine
            .templates
            .borrow_mut()
            .insert_sized(template.to_string(), saved, template.len());
    }
}

fn call_template(ctx: &Ctx<'_>, function: Function<'_>) -> Result<String, Error> {
    function
        .call(())
        .catch(ctx)
        .map_err(|caught| caught_error(Error::new(caught.to_string()), &caught))
}

fn template_source(template: &str) -> String {
    format!("(function(){{return {};}})", template_literal(template))
}

/// A `${}` template as a JavaScript template literal: a backslash before `${` keeps it literal, any other backslash is
/// itself, and a backtick is escaped.
pub(crate) fn template_literal(template: &str) -> String {
    let mut source = String::with_capacity(template.len() + 2);
    source.push('`');
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if bytes.get(index + 1) == Some(&b'$') && bytes.get(index + 2) == Some(&b'{') {
                source.push_str("\\${");
                index += 3;
            } else {
                source.push_str("\\\\");
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'`' {
            source.push_str("\\`");
            index += 1;
            continue;
        }
        let ch = template[index..].chars().next().unwrap_or('\u{FFFD}');
        source.push(ch);
        index += ch.len_utf8();
    }
    source.push('`');
    source
}

/// Run a host callback's `body`, turning a panic into a JS exception.
pub fn host_guard<'js, T>(ctx: &Ctx<'js>, body: impl FnOnce() -> rquickjs::Result<T>) -> rquickjs::Result<T> {
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => Err(Exception::throw_message(ctx, &panic_message(payload))),
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "host callback panicked".to_string()
}

/// Throw `err` as a JS exception, with its diagnostic, kind and retry delay.
pub fn throw_lib(ctx: &Ctx<'_>, err: &moqui_lib::error::Error) -> JsError {
    throw_diagnostic(
        ctx,
        err.message(),
        &err.diagnostic(),
        ErrorKind::of(err),
        Error::retry_after_of(err),
    )
}

fn throw_context(ctx: &Ctx<'_>, err: &Error) -> JsError {
    throw_diagnostic(ctx, err.message(), &err.diagnostic(), err.kind(), err.retry_after())
}

/// Keep `error.message` unchanged for scripts that catch or classify errors. Source diagnostics
/// belong to the stack, alongside the JavaScript call site, not in the exception's message. A security kind goes on
/// the error, so it is still one when the script ends with it.
fn throw_diagnostic(
    ctx: &Ctx<'_>,
    message: &str,
    diagnostic: &str,
    kind: ErrorKind,
    retry_after: Option<u64>,
) -> JsError {
    match Exception::from_message(ctx.clone(), message) {
        Ok(exception) => {
            let detail = diagnostic.strip_prefix(message).unwrap_or(diagnostic);
            if !detail.is_empty() {
                let stack = exception.stack().unwrap_or_default();
                if let Err(err) = exception.as_object().set("stack", format!("{stack}{detail}\n")) {
                    return err;
                }
            }
            if let Err(err) = mark_kind(&exception, kind).and_then(|()| mark_retry_after(&exception, retry_after)) {
                return err;
            }
            exception.throw()
        }
        Err(err) => err,
    }
}

/// The property of a thrown error that names its security kind, so a security error is still one after it has
/// crossed the script and come back: an authorization failure in a service's script is still one for the call, as
/// Java's `ArtifactAuthorizationException` passes through Groovy. It goes with that one error: one a script catches
/// and goes on from does not mark a later failure, and a nested unit does not change the outer one's.
pub(crate) const KIND_PROPERTY: &str = "moquiErrorKind";

/// The kinds [`mark_kind`] carries on the error, by name.
const CARRIED_KINDS: [(ErrorKind, &str); 8] = [
    (ErrorKind::AuthenticationRequired, "AuthenticationRequired"),
    (ErrorKind::Unauthorized, "Unauthorized"),
    (ErrorKind::Tarpit, "Tarpit"),
    (ErrorKind::NotFound, "NotFound"),
    (ErrorKind::Conflict, "Conflict"),
    (ErrorKind::Unprocessable, "Unprocessable"),
    (ErrorKind::Unavailable, "Unavailable"),
    (ErrorKind::MethodNotAllowed, "MethodNotAllowed"),
];

/// Put `kind` on an error about to be thrown, when it is one of the [`CARRIED_KINDS`].
fn mark_kind(exception: &Exception<'_>, kind: ErrorKind) -> rquickjs::Result<()> {
    match CARRIED_KINDS.iter().find(|(carried, _)| *carried == kind) {
        Some((_, name)) => exception.as_object().set(KIND_PROPERTY, *name),
        None => Ok(()),
    }
}

/// The property of a thrown error that holds a tarpit's retry seconds, so the `Retry-After` of a tarpit refused under
/// a script (a screen's render) is still sent.
pub(crate) const RETRY_AFTER_PROPERTY: &str = "moquiRetryAfter";

/// Put the retry seconds of a refusal on an error about to be thrown.
fn mark_retry_after(exception: &Exception<'_>, retry_after: Option<u64>) -> rquickjs::Result<()> {
    match retry_after {
        Some(seconds) => exception.as_object().set(RETRY_AFTER_PROPERTY, seconds as f64),
        None => Ok(()),
    }
}

/// The error a script ended with: `err` (its message) with the kind and retry seconds the thrown error carries.
fn caught_error(err: Error, caught: &rquickjs::CaughtError<'_>) -> Error {
    let err = err.with_kind(caught_kind(caught));
    let object = match caught {
        rquickjs::CaughtError::Exception(exception) => Some(exception.as_object()),
        rquickjs::CaughtError::Value(value) => value.as_object(),
        rquickjs::CaughtError::Error(_) => None,
    };
    let seconds = object.and_then(|object| object.get::<_, Option<f64>>(RETRY_AFTER_PROPERTY).ok().flatten());
    match seconds {
        Some(seconds) if seconds >= 0.0 => err.with_retry_after(seconds as u64),
        _ => err,
    }
}

/// The kind [`mark_kind`] names `name`, for an error that crossed a script on either engine.
pub(crate) fn carried_kind(name: &str) -> Option<ErrorKind> {
    CARRIED_KINDS.iter().find(|(_, known)| *known == name).map(|(kind, _)| *kind)
}

/// The security kind [`mark_kind`] put on the error the script ended with, else general.
fn caught_kind(caught: &rquickjs::CaughtError<'_>) -> ErrorKind {
    let object = match caught {
        rquickjs::CaughtError::Exception(exception) => Some(exception.as_object()),
        rquickjs::CaughtError::Value(value) => value.as_object(),
        rquickjs::CaughtError::Error(_) => None,
    };
    object
        .and_then(|object| object.get::<_, Option<String>>(KIND_PROPERTY).ok().flatten())
        .and_then(|name| CARRIED_KINDS.iter().find(|(_, known)| *known == name))
        .map_or(ErrorKind::General, |(kind, _)| *kind)
}

/// `ec` is an own property of `globalThis`, so the bare-name proxy does not hide it.
fn install_ec(ctx: &Ctx<'_>) -> Result<(), Error> {
    let ec = caught(ctx, "ec", Object::new(ctx.clone()))?;
    ec.set("entity", entity_object(ctx)?).map_err(js_error)?;
    ec.set("resource", resource_object(ctx)?).map_err(js_error)?;
    ec.set("transaction", transaction_object(ctx)?).map_err(js_error)?;
    ec_extra::install_ec_extras(ctx, &ec)?;
    // every path of the host table (script/host), as the oxide engine installs them; last, so a path inside a member
    // made above (ec.resource.x) is added to it
    bind::install_ec(ctx, &ec)?;
    caught(ctx, "globals", ctx.globals().set("ec", ec))?;
    let system = Object::new(ctx.clone()).map_err(js_error)?;
    let property = Function::new(ctx.clone(), system_get_property).map_err(js_error)?;
    system.set("getProperty", property).map_err(js_error)?;
    caught(ctx, "globals", ctx.globals().set("System", system))?;
    Ok(())
}

fn caught<'js, T>(ctx: &Ctx<'js>, step: &str, result: rquickjs::Result<T>) -> Result<T, Error> {
    result.catch(ctx).map_err(|err| Error::new(format!("{step}: {err}")))
}

/// `ec.transaction`: the read-only queries of Java's `TransactionFacade`, on the default entity group's manager.
/// Transactions are begun, committed, and suspended by service and screen attributes, not by scripts.
fn transaction_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let transaction = Object::new(ctx.clone()).map_err(js_error)?;
    let in_place = caught(
        ctx,
        "transaction.isTransactionInPlace",
        Function::new(ctx.clone(), |ctx: Ctx<'_>| {
            host_guard(&ctx, || Ok(default_transactions(&ctx)?.in_transaction()))
        }),
    )?;
    transaction.set("isTransactionInPlace", in_place).map_err(js_error)?;
    let cache = caught(
        ctx,
        "transaction.isTransactionCacheActive",
        Function::new(ctx.clone(), |ctx: Ctx<'_>| {
            host_guard(&ctx, || Ok(default_transactions(&ctx)?.transaction_cache_active()))
        }),
    )?;
    transaction.set("isTransactionCacheActive", cache).map_err(js_error)?;
    Ok(transaction)
}

fn default_transactions(ctx: &Ctx<'_>) -> rquickjs::Result<crate::transaction::TransactionManager> {
    let facade = facade_of(ctx)?;
    let group = facade.default_group();
    let transactions = facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, _, transactions)| transactions.clone());
    transactions
        .ok_or_else(|| Exception::throw_message(ctx, &format!("no transaction manager for entity group {group}")))
}

fn entity_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let entity = Object::new(ctx.clone()).map_err(js_error)?;
    // find, makeValue and the sequence and transaction cache functions are the host table's (script/host/entities.rs)
    entity.set("databaseTimeZone", "UTC").map_err(js_error)?;
    // getDataDocuments and conditionFactory are the host table's too (script/host/conditions.rs)
    Ok(entity)
}

fn resource_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let resource = Object::new(ctx.clone()).map_err(js_error)?;
    let expand = Function::new(ctx.clone(), resource_expand).map_err(js_error)?;
    resource.set("expand", expand).map_err(js_error)?;
    let script = Function::new(ctx.clone(), resource_script).map_err(js_error)?;
    resource.set("script", script).map_err(js_error)?;
    ec_extra::add_resource_functions(ctx, &resource)?;
    Ok(resource)
}

/// The entity facade of the entered context; throws when there is none.
pub fn facade_of(ctx: &Ctx<'_>) -> rquickjs::Result<EntityFacade> {
    let installed = with_ec(ctx, |ec| ec.factory().entity())?;
    match installed {
        Some(facade) => Ok(facade),
        None => Err(Exception::throw_message(ctx, "no entity facade is installed")),
    }
}

/// `body` on the entered execution context; throws when none is entered.
pub fn with_ec<T>(ctx: &Ctx<'_>, body: impl FnOnce(&ExecutionContext) -> T) -> rquickjs::Result<T> {
    ExecutionContext::with_current(body).ok_or_else(|| Exception::throw_message(ctx, "no execution context is entered"))
}

/// A framework error from a JS engine error.
pub fn js_error(err: impl ToString) -> Error {
    Error::new(err.to_string())
}

/// Read one name from the thread-local stack. `None` when the name is missing.
/// [`context_value`], moved out of the top frame when only the stack holds it: for a frame about to be popped.
pub fn take_context_value(name: &str) -> Option<Value> {
    STACK.with(|stack| stack.borrow_mut().take(name))
}

pub fn context_value(name: &str) -> Option<Value> {
    match stack_get(name) {
        Lookup::Value(value) => Some(value),
        Lookup::Missing | Lookup::Context => None,
    }
}

/// Evaluate a JavaScript expression to a host value.
///
/// # Errors
///
/// Returns an error when the expression does not parse, throws, or gives a value that cannot be held.
pub fn eval_value(expression: &str) -> Result<Value, Error> {
    eval_value_as(EntryKind::Value, expression)
}

/// [`eval_value`] for an entry of `kind` (a value, a field's value), routed between the engines by its kind.
fn eval_value_as(kind: EntryKind, expression: &str) -> Result<Value, Error> {
    if expression.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let source = as_expression(expression);
    let quickjs = || {
        with_kind(kind, |ctx| {
            with_widget_ec(ctx, expression, || {
                let evaluated = eval_compiled::<JsValue>(ctx, &source, true)?;
                js_value_to_host(ctx, &evaluated)
            })
        })
    };
    crate::script::engine::small_script(kind, &source, crate::script::oxide::eval_value, quickjs)
}

/// Evaluate a field's `from` expression to a host value. A timestamp, date, or time reaches a script as epoch
/// millis text, so when the expression is a plain path such as `segment.actualStartDate` and the value at that path
/// is a temporal value, the temporal value is returned, which a display formats as a date and time.
///
/// # Errors
///
/// Returns an error when the expression does not parse, throws, or gives a value that cannot be held.
pub fn eval_field_value(expression: &str) -> Result<Value, Error> {
    let value = eval_value_as(EntryKind::FieldValue, expression)?;
    if !matches!(value, Value::String(_)) {
        return Ok(value);
    }
    Ok(temporal_at_path(expression).unwrap_or(value))
}

/// The timestamp, date, or time at a plain dotted path in the context (`a.b`, `a?.b`), read without converting it
/// for a script. None for any other expression or value.
fn temporal_at_path(expression: &str) -> Option<Value> {
    let mut names = expression
        .trim()
        .split('.')
        .map(|name| name.strip_suffix('?').unwrap_or(name));
    let is_name = |name: &str| {
        name.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    };
    let first = names.next().filter(|name| is_name(name))?;
    let mut value = context_value(first)?;
    for name in names {
        if !is_name(name) {
            return None;
        }
        value = value_field(&value, name)?;
    }
    matches!(value, Value::Timestamp(_) | Value::Date(_) | Value::Time(_)).then_some(value)
}

/// Run a script once, not kept, named `file_name` in stack traces, and return its value as a host value. The test
/// harness runs a spec file with it, so an error names the spec file and line.
pub fn eval_named_value(source: &str, file_name: &str) -> Result<Value, Error> {
    if source.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    crate::script::engine::require_quickjs(EntryKind::Spec)?;
    with_engine(EngineKind::Spec, |ctx| {
        let evaluated = compiled::eval_named(ctx, source, file_name)?;
        js_value_to_host(ctx, &evaluated)
    })
}

/// Evaluate a JavaScript object expression into a map. A non-object is an error.
pub fn eval_map(expression: &str) -> Result<std::collections::BTreeMap<String, Value>, Error> {
    if expression.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    let source = as_expression(expression);
    let quickjs = || {
        with_kind(EntryKind::Map, |ctx| {
            with_widget_ec(ctx, expression, || {
                let evaluated = eval_compiled::<JsValue>(ctx, &source, true)?;
                object_map(ctx, &evaluated)
            })
        })
    };
    crate::script::engine::small_script(EntryKind::Map, &source, crate::script::oxide::eval_map, quickjs)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
