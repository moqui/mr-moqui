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
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use moqui_lib::context_stack::{ContextStack, Lookup};
use moqui_lib::object::{HostObject, Value};
use rquickjs::class::Trace;
use rquickjs::context::EvalOptions;
use rquickjs::function::{Opt, Rest, This};
use rquickjs::object::Property;
use rquickjs::Error as JsError;
use rquickjs::{
    CatchResultExt, Class, Context, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Null, Object, Persistent,
    Runtime, Undefined, Value as JsValue,
};

mod archive_host;
mod compiled;
mod conditions;
mod convert;
mod decimal;
mod ec_extra;
mod ec_screen;
mod edi;
mod entities;
mod error_info;
mod facades;
mod files_host;
#[cfg(test)]
mod framework_gaps_tests;
mod helpers;
#[cfg(feature = "llm")]
mod llm;
#[cfg(test)]
mod metadata_tests;
mod native_tools;
mod notification;
mod screen_host;
mod search;
mod services;
mod sftp;
mod tools_host;
mod upload;
mod urls;
mod user_host;
mod values;
mod web_host;

pub(crate) use compiled::ScriptText;
use compiled::{eval_compiled, eval_compiled_text, MAX_COMPILED_SCRIPTS, MAX_COMPILED_SCRIPT_BYTES};
use conditions::*;
use convert::*;
use decimal::*;
pub use ec_extra::expand_localized;
use entities::*;
use error_info::*;
pub(crate) use facades::forget_scripts;
pub use facades::run_script_location;
use facades::*;
use helpers::*;
use native_tools::*;
pub(crate) use screen_host::with_screen_runner;
use search::*;
use services::*;
use upload::*;
pub use values::script_list_values;
pub(crate) use values::{
    context_combined, pop_context_frame, put_context_all, value_entity_name, value_field, value_items, value_map,
};
pub(crate) use web_host::invoke as invoke_web_handler;

use crate::context::{Error, ErrorKind, ExecutionContext};
use moqui_db::sql::{CompareOp, Condition};

use crate::entity::{EntityFacade, EntityFind, EntityList, EntityValue};
use crate::service::call::CallOptions;

const MAX_CACHED_TEMPLATES: usize = 4096;
/// The source text the template and screen unit caches of an engine may hold. Their bytecode is about the size of the
/// source and lives in the engine's [`MEMORY_LIMIT`] with everything the scripts make.
const MAX_CACHED_TEMPLATE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CACHED_UNIT_BYTES: usize = 8 * 1024 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
/// The stack the outermost script on a thread may use, below the 2 MiB of a service worker. QuickJS counts an engine's
/// stack from where it is entered. Nested scripts share this budget plus [`NESTED_STACK`], capped at [`NESTED_STACK`]
/// per engine except on larger request workers. Less than [`MIN_NESTED_STACK`] left fails before entering an engine.
const SCRIPT_STACK: usize = 1024 * 1024;
/// Full app renders nest widget templates, includes and services on the request worker's 8 MiB stack.
const REQUEST_SCRIPT_STACK: usize = 4 * 1024 * 1024;
const NESTED_STACK: usize = 256 * 1024;
const MIN_NESTED_STACK: usize = 32 * 1024;
const VALUE_OF_MESSAGE: &str =
    "decimal valueOf is not supported; use add, subtract, multiply, divide, eq, compare, isZero, or toNumber";

thread_local! {
    /// While true, an expression of a widget runs with the `ec` of the server-static browser host: the compatibility report's full check.
    static WIDGET_WRAP: Cell<bool> = const { Cell::new(false) };
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
    /// Whether `DEADLINE` is the default limit of a script that runs outside a transaction (see [`ScriptLimit`]).
    static DEADLINE_IS_DEFAULT: Cell<bool> = const { Cell::new(false) };
    /// The default limit of a script outside a transaction, read at the first script on the thread.
    static SCRIPT_LIMIT: Cell<Option<Option<Duration>>> = const { Cell::new(None) };
    /// Whether this thread is a request worker, whose scripts outside a transaction are limited by default.
    static REQUEST_THREAD: Cell<bool> = const { Cell::new(false) };
    /// Where the stack was when the outermost script on the thread was entered.
    static STACK_BASE: Cell<usize> = const { Cell::new(0) };
    static STACK: RefCell<ContextStack> = RefCell::new(ContextStack::new());
    static ENGINES: RefCell<Vec<Rc<Engine>>> = const { RefCell::new(Vec::new()) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
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
/// and any other area is reported through `deny` and throws.
const WRAPPED_EC: &str = r#"(function (real, deny) {
    const pick = (source, names) => {
        const out = {};
        names.forEach((name) => { const value = source[name]; out[name] = typeof value === "function" ? value.bind(source) : value; });
        return out;
    };
    const allowed = {
        l10n: pick(real.l10n, ["localize", "format", "formatCurrency", "formatCurrencyNoSymbol"]),
        user: pick(real.user, ["userId", "username", "locale", "timeZone", "userAccount", "getPreference"]),
        resource: pick(real.resource, ["expand", "expandNoL10n"])
    };
    return new Proxy(allowed, {
        get(target, key) {
            if (typeof key === "symbol") return undefined;
            if (Object.prototype.hasOwnProperty.call(target, key)) return target[key];
            deny(String(key));
            throw new Error("ec." + String(key) + " is not available in a server-static qvt screen");
        }
    });
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

/// Bounds script time on this thread to a transaction's remaining time.
///
/// The QuickJS interrupt handler reads the deadline, so a loop that never returns
/// ends with an error instead of holding the worker. Nested guards keep the earlier deadline. A transaction's limit
/// replaces the default limit of a script that runs outside one, and the time inside does not count against that default.
pub struct DeadlineGuard {
    previous: Option<Instant>,
    previous_default: bool,
    /// When the guard replaced a default limit, so the default is moved on by the time spent inside.
    replaced_default: Option<Instant>,
}

impl DeadlineGuard {
    pub fn new(remaining: Option<Duration>) -> Self {
        let previous = DEADLINE.with(Cell::get);
        let previous_default = DEADLINE_IS_DEFAULT.with(Cell::get);
        let mut replaced_default = None;
        if let Some(remaining) = remaining {
            let now = Instant::now();
            let next = now + remaining;
            let chosen = match previous {
                Some(existing) if !previous_default => existing.min(next),
                Some(_) => {
                    replaced_default = Some(now);
                    next
                }
                None => next,
            };
            DEADLINE.with(|deadline| deadline.set(Some(chosen)));
            DEADLINE_IS_DEFAULT.with(|flag| flag.set(false));
        }
        Self {
            previous,
            previous_default,
            replaced_default,
        }
    }
}

impl Drop for DeadlineGuard {
    fn drop(&mut self) {
        let previous = match (self.previous, self.replaced_default) {
            (Some(at), Some(entered)) => Some(at + entered.elapsed()),
            (previous, _) => previous,
        };
        DEADLINE.with(|deadline| deadline.set(previous));
        DEADLINE_IS_DEFAULT.with(|flag| flag.set(self.previous_default));
    }
}

/// The time left before this thread's script deadline, when it has one.
pub(crate) fn script_time_remaining() -> Option<Duration> {
    DEADLINE
        .with(Cell::get)
        .map(|deadline| deadline.saturating_duration_since(Instant::now()))
}

/// The default of `webapp_request_timeout`, the host's limit on a whole request, read here by the same name.
const DEFAULT_REQUEST_TIMEOUT: u64 = 300;

/// Mark this thread a request worker: a script on it that runs outside a transaction is limited to the request
/// timeout unless `resource-facade` `script-timeout` says otherwise.
pub(crate) fn mark_request_thread() {
    REQUEST_THREAD.with(|flag| flag.set(true));
    SCRIPT_LIMIT.with(|cell| cell.set(None));
}

/// The seconds a script may run when no transaction limits it, read once per thread. The conf's `resource-facade`
/// `script-timeout` when set, for every thread; else the webapp's `requests` `request-timeout`
/// (`webapp_request_timeout`) on a request worker, and no limit on a service, job, or scheduler thread, which run batch
/// work outside a transaction as Java does. 0 is no limit.
fn default_script_limit() -> Option<Duration> {
    if let Some(limit) = SCRIPT_LIMIT.with(Cell::get) {
        return limit;
    }
    let factory = crate::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()));
    let limit = script_limit_for(
        &crate::resource::setting("script-timeout"),
        REQUEST_THREAD.with(Cell::get),
        &crate::web::protection_setting(
            factory.as_deref(),
            "requests",
            "request-timeout",
            "webapp_request_timeout",
        ),
    );
    SCRIPT_LIMIT.with(|cell| cell.set(Some(limit)));
    limit
}

fn script_limit_for(script_timeout: &str, request_thread: bool, request_timeout: &str) -> Option<Duration> {
    let seconds = match script_timeout.trim().parse::<u64>() {
        Ok(seconds) => seconds,
        Err(_) if request_thread => request_timeout.trim().parse().unwrap_or(DEFAULT_REQUEST_TIMEOUT),
        Err(_) => 0,
    };
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// The default deadline of the outermost script on a thread when nothing set one (see [`default_script_limit`]), so a
/// loop in a script that runs outside a transaction (a widget condition, a `transaction="ignore"` service a request
/// calls) cannot hold a request worker for good. Cleared when the script ends.
struct ScriptLimit {
    armed: bool,
}

impl ScriptLimit {
    fn arm() -> Self {
        let armed = DEADLINE.with(Cell::get).is_none()
            && default_script_limit().is_some_and(|limit| {
                DEADLINE.with(|deadline| deadline.set(Some(Instant::now() + limit)));
                DEADLINE_IS_DEFAULT.with(|flag| flag.set(true));
                true
            });
        Self { armed }
    }
}

impl Drop for ScriptLimit {
    fn drop(&mut self) {
        if self.armed {
            DEADLINE.with(|deadline| deadline.set(None));
            DEADLINE_IS_DEFAULT.with(|flag| flag.set(false));
        }
    }
}

/// An address on the current stack.
fn stack_address() -> usize {
    let marker = 0_u8;
    std::hint::black_box(std::ptr::from_ref(&marker)) as usize
}

/// The stack budget of the engine entered at `depth`, or an error when a nested script has too little stack left.
fn stack_budget(depth: usize) -> Result<usize, Error> {
    let here = stack_address();
    let (budget, ceiling) = if REQUEST_THREAD.with(Cell::get) {
        (REQUEST_SCRIPT_STACK, REQUEST_SCRIPT_STACK)
    } else {
        (SCRIPT_STACK, NESTED_STACK)
    };
    if depth == 0 {
        STACK_BASE.with(|base| base.set(here));
        return Ok(budget);
    }
    let used = STACK_BASE.with(Cell::get).abs_diff(here);
    let left = (budget + NESTED_STACK).saturating_sub(used).min(ceiling);
    if left < MIN_NESTED_STACK {
        return Err(Error::new("scripts are nested too deeply for the thread's stack"));
    }
    Ok(left)
}

/// Persistents drop before the context, and the context drops before the runtime.
struct Engine {
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

impl Engine {
    /// Drop the compiled code and the objects kept in this engine, to free its heap.
    fn clear_caches(&self) {
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
struct StackMark {
    depth: Option<(usize, usize)>,
}

impl StackMark {
    fn new() -> Self {
        Self {
            depth: Some(STACK.with(|stack| stack.borrow().depth())),
        }
    }

    /// Leave the frames pushed since as they are.
    fn keep(mut self) {
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
}

/// [`EngineMemory`] of this thread's engines, after a collection when `collect`. Not while a script runs on the thread.
pub fn engine_memory(collect: bool) -> EngineMemory {
    let engines = ENGINES.with(|engines| engines.borrow().clone());
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
    memory
}

/// Run one script statement in the server runtime. The script is not strict.
pub fn run_script(source: &str) -> Result<(), Error> {
    if source.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    eval_script(source)
}

/// A source that starts with `{` is an object literal in an expression, not a block.
fn as_expression(source: &str) -> std::borrow::Cow<'_, str> {
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
    with_context(|ctx| {
        with_widget_ec(ctx, expression, || {
            let value = eval_compiled::<JsValue>(ctx, &as_expression(expression), true)?;
            Ok(js_truthy(&value))
        })
    })
}

/// Expand a template. `${expression}` runs in this runtime. No `$`, or a trailing `$`, is unchanged.
pub fn expand(template: &str) -> Result<String, Error> {
    if !template.contains('$') || template.ends_with('$') {
        return Ok(template.to_string());
    }
    if template.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    with_context(|ctx| expand_in(ctx, template))
}

/// See [`upload::text_upload_to_js`].
pub(crate) use upload::text_upload_to_js;

pub(crate) fn with_ctx<R>(body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    with_context(body)
}

pub(super) fn eval_source<'js, T>(ctx: &Ctx<'js>, source: &str) -> Result<T, Error>
where
    T: FromJs<'js>,
{
    eval_js(ctx, source)
}

/// Run one generated action script.
///
/// The script is not strict. A strict function would reject bare assignment, and
/// that assignment is how a plain value reaches the context stack.
pub(super) fn eval_script(source: &str) -> Result<(), Error> {
    with_context(|ctx| eval_compiled::<JsValue>(ctx, source, false).map(|_| ()))
}

/// [`eval_script`] for a script whose text and cache key were made once.
pub(super) fn eval_script_text(script: &ScriptText) -> Result<(), Error> {
    with_context(|ctx| eval_compiled_text::<JsValue>(ctx, script, false).map(|_| ()))
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

/// A script that runs a service that runs a script needs its own runtime: a `Context::with`
/// cannot be entered again while it is held. Each nesting level gets one, kept for reuse. A chain of Mantle services
/// from a shipment delivery to a ledger posting nests about 20 deep; the stack budget ([`stack_budget`]) is what stops
/// runaway recursion, so this only bounds the number of engines.
const MAX_SCRIPT_DEPTH: usize = 64;

struct DepthGuard;

impl Drop for DepthGuard {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn with_context<R>(body: impl FnOnce(&Ctx) -> Result<R, Error>) -> Result<R, Error> {
    let depth = DEPTH.with(Cell::get);
    if depth >= MAX_SCRIPT_DEPTH {
        return Err(Error::new("scripts are nested too deeply"));
    }
    let stack = stack_budget(depth)?;
    let existing = ENGINES.with(|engines| engines.borrow().get(depth).cloned());
    let engine = match existing {
        Some(engine) => engine,
        None => {
            let started = Rc::new(Engine::start()?);
            ENGINES.with(|engines| engines.borrow_mut().push(Rc::clone(&started)));
            started
        }
    };
    engine.runtime.set_max_stack_size(stack);
    let _limit = (depth == 0).then(ScriptLimit::arm);
    DEPTH.with(|current| current.set(depth + 1));
    let _guard = DepthGuard;
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
                    let mut stack = stack.borrow_mut();
                    if !stack.put_existing(&name, host.clone()) {
                        let _ = stack.put(name.clone(), host);
                    }
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

/// Whether a script is running on this thread.
pub(crate) fn in_script() -> bool {
    DEPTH.with(Cell::get) > 0
}

/// The engine of the innermost running script.
fn current_engine() -> Option<Rc<Engine>> {
    let depth = DEPTH.with(Cell::get);
    ENGINES.with(|engines| engines.borrow().get(depth.checked_sub(1)?).cloned())
}

impl Engine {
    fn start() -> Result<Self, Error> {
        let runtime = Runtime::new().map_err(js_error)?;
        runtime.set_memory_limit(MEMORY_LIMIT);
        runtime.set_interrupt_handler(Some(Box::new(|| {
            DEADLINE
                .with(Cell::get)
                .is_some_and(|deadline| Instant::now() >= deadline)
        })));
        let context = Context::full(&runtime).map_err(js_error)?;
        let helpers = context.with(|ctx| install_context(&ctx))?;
        Ok(Self {
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
    tools_host::install(ctx, &string_utilities, &collection_utilities, &web_utilities)?;
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
    fn name_of<'js>(atom: &Atom<'js>) -> rquickjs::Result<Option<(std::string::String, JsValue<'js>)>> {
        let key = atom.to_value()?;
        if key.is_symbol() {
            return Ok(None);
        }
        Ok(Some((atom.to_string()?, key)))
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

/// Set a name on the stack from host code, forgetting any list or map the script already read
/// under that name so its next read sees the new value.
fn put_name(ctx: &Ctx<'_>, name: &str, value: Value) -> rquickjs::Result<()> {
    if let Some(engine) = current_engine() {
        engine.tracked.borrow_mut().remove(name);
        engine.released.borrow_mut().remove(name);
    }
    STACK.with(|stack| {
        stack
            .borrow_mut()
            .put(name, value)
            .map(|_| ())
            .map_err(|err| Exception::throw_message(ctx, err.message()))
    })
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
    let mut options = EvalOptions::default();
    options.strict = true;
    options.global = true;
    ctx.eval_with_options::<T, _>(source, options)
        .catch(ctx)
        .map_err(|caught| Error::new(caught.to_string()))
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
    with_context(|ctx| run_template_function_here(ctx, source))
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
    let mut source = String::from("(function(){return `");
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
    source.push_str("`;})");
    source
}

fn host_guard<'js, T>(ctx: &Ctx<'js>, body: impl FnOnce() -> rquickjs::Result<T>) -> rquickjs::Result<T> {
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => Err(Exception::throw_message(ctx, &panic_message(payload))),
    }
}

/// A function that returns `text`, for the Java getters of an object made for a script.
fn text_getter<'js>(ctx: &Ctx<'js>, text: String) -> rquickjs::Result<Function<'js>> {
    Function::new(ctx.clone(), move |ctx: Ctx<'_>| host_guard(&ctx, || Ok(text.clone())))
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

fn throw_lib(ctx: &Ctx<'_>, err: &moqui_lib::error::Error) -> JsError {
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
const KIND_PROPERTY: &str = "moquiErrorKind";

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
const RETRY_AFTER_PROPERTY: &str = "moquiRetryAfter";

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
    let message = caught(ctx, "message", Class::instance(ctx.clone(), HostMessage {}))?;
    ec.set("message", message).map_err(js_error)?;
    ec.set("logger", logger_object(ctx)?).map_err(js_error)?;
    ec.set("service", service_object(ctx)?).map_err(js_error)?;
    let user = caught(ctx, "user", Class::instance(ctx.clone(), HostUser {}))?;
    user_host::install_user_extras(ctx, &user)?;
    ec.set("user", user).map_err(js_error)?;
    let web = caught(ctx, "web", Class::instance(ctx.clone(), HostWeb {}))?;
    ec.set("web", web).map_err(js_error)?;
    ec.set("cache", cache_object(ctx)?).map_err(js_error)?;
    ec.set("search", search_object(ctx)?).map_err(js_error)?;
    #[cfg(feature = "llm")]
    ec.set("llm", llm::llm_object(ctx)?).map_err(js_error)?;
    ec.set("transaction", transaction_object(ctx)?).map_err(js_error)?;
    ec_extra::install_ec_extras(ctx, &ec)?;
    let make_notification = Function::new(ctx.clone(), notification::make_notification_message).map_err(js_error)?;
    ec.set("makeNotificationMessage", make_notification).map_err(js_error)?;
    caught(ctx, "globals", ctx.globals().set("ec", ec))?;
    ec_screen::install_builder(ctx)?;
    caught(ctx, "globals", ctx.globals().set("sri", screen_host::sri_object(ctx)?))?;
    let system = Object::new(ctx.clone()).map_err(js_error)?;
    let property = Function::new(ctx.clone(), system_get_property).map_err(js_error)?;
    system.set("getProperty", property).map_err(js_error)?;
    caught(ctx, "globals", ctx.globals().set("System", system))?;
    edi::install(ctx)?;
    sftp::install(ctx)?;
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
    let group = facade.inner.default_group.clone();
    facade
        .inner
        .groups
        .get(&group)
        .map(|runtime| runtime.transactions.clone())
        .ok_or_else(|| Exception::throw_message(ctx, &format!("no transaction manager for entity group {group}")))
}

fn entity_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let entity = Object::new(ctx.clone()).map_err(js_error)?;
    let find = caught(ctx, "entity.find", Function::new(ctx.clone(), entity_find))?;
    entity.set("find", find).map_err(js_error)?;
    let make = caught(ctx, "entity.makeValue", Function::new(ctx.clone(), entity_make_value))?;
    entity.set("makeValue", make).map_err(js_error)?;
    let start_sim = caught(
        ctx,
        "entity.startTxCacheDb",
        Function::new(ctx.clone(), entities::entity_start_tx_cache_db),
    )?;
    entity.set("startTxCacheDb", start_sim).map_err(js_error)?;
    let stop_sim = caught(
        ctx,
        "entity.stopTxCache",
        Function::new(ctx.clone(), entities::entity_stop_tx_cache),
    )?;
    entity.set("stopTxCache", stop_sim).map_err(js_error)?;
    let sim_active = caught(
        ctx,
        "entity.isTxCacheActive",
        Function::new(ctx.clone(), entities::entity_is_tx_cache_active),
    )?;
    entity.set("isTxCacheActive", sim_active).map_err(js_error)?;
    let documents = caught(
        ctx,
        "entity.getDataDocuments",
        Function::new(ctx.clone(), entity_get_data_documents),
    )?;
    entity.set("getDataDocuments", documents).map_err(js_error)?;
    let sequenced = caught(
        ctx,
        "entity.sequencedIdPrimary",
        Function::new(ctx.clone(), entities::entity_sequenced_id_primary),
    )?;
    entity.set("sequencedIdPrimary", sequenced).map_err(js_error)?;
    let conf_name = caught(
        ctx,
        "entity.getDatabaseConfName",
        Function::new(ctx.clone(), entities::entity_get_database_conf_name),
    )?;
    entity.set("getDatabaseConfName", conf_name).map_err(js_error)?;
    let temp_set = caught(
        ctx,
        "entity.tempSetSequencedIdPrimary",
        Function::new(ctx.clone(), entities::entity_temp_set_sequenced_id_primary),
    )?;
    entity.set("tempSetSequencedIdPrimary", temp_set).map_err(js_error)?;
    let temp_reset = caught(
        ctx,
        "entity.tempResetSequencedIdPrimary",
        Function::new(ctx.clone(), entities::entity_temp_reset_sequenced_id_primary),
    )?;
    entity
        .set("tempResetSequencedIdPrimary", temp_reset)
        .map_err(js_error)?;
    let factory = Object::new(ctx.clone()).map_err(js_error)?;
    let make_condition = Function::new(ctx.clone(), make_condition).map_err(js_error)?;
    factory.set("makeCondition", make_condition).map_err(js_error)?;
    let make_where = Function::new(ctx.clone(), make_condition_where).map_err(js_error)?;
    factory.set("makeConditionWhere", make_where).map_err(js_error)?;
    entity.set("databaseTimeZone", "UTC").map_err(js_error)?;
    let make_date = Function::new(ctx.clone(), make_condition_date).map_err(js_error)?;
    factory.set("makeConditionDate", make_date).map_err(js_error)?;
    let make_direct = Function::new(ctx.clone(), make_action_condition).map_err(js_error)?;
    factory
        .set("makeActionConditionDirect", make_direct)
        .map_err(js_error)?;
    entity.set("conditionFactory", factory).map_err(js_error)?;
    Ok(entity)
}

fn cache_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let cache = Object::new(ctx.clone()).map_err(js_error)?;
    let get_cache = Function::new(ctx.clone(), cache_get_cache).map_err(js_error)?;
    cache.set("getCache", get_cache).map_err(js_error)?;
    Ok(cache)
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

fn logger_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let logger = Object::new(ctx.clone()).map_err(js_error)?;
    let log = Function::new(ctx.clone(), logger_log).map_err(js_error)?;
    logger.set("log", log).map_err(js_error)?;
    let warn = Function::new(ctx.clone(), logger_warn).map_err(js_error)?;
    logger.set("warn", warn).map_err(js_error)?;
    let info = Function::new(ctx.clone(), facades::logger_info).map_err(js_error)?;
    logger.set("info", info).map_err(js_error)?;
    let error = Function::new(ctx.clone(), facades::logger_error).map_err(js_error)?;
    logger.set("error", error).map_err(js_error)?;
    let debug = Function::new(ctx.clone(), facades::logger_debug).map_err(js_error)?;
    logger.set("debug", debug).map_err(js_error)?;
    let trace = Function::new(ctx.clone(), facades::logger_trace).map_err(js_error)?;
    logger.set("trace", trace).map_err(js_error)?;
    Ok(logger)
}

fn service_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let service = Object::new(ctx.clone()).map_err(js_error)?;
    let sync = Function::new(ctx.clone(), service_sync).map_err(js_error)?;
    service.set("sync", sync).map_err(js_error)?;
    let async_call = Function::new(ctx.clone(), service_async).map_err(js_error)?;
    service.set("async", async_call).map_err(js_error)?;
    let job = Function::new(ctx.clone(), service_job).map_err(js_error)?;
    service.set("job", job).map_err(js_error)?;
    Ok(service)
}

fn facade_of(ctx: &Ctx<'_>) -> rquickjs::Result<EntityFacade> {
    let installed = with_ec(ctx, |ec| ec.factory().entity())?;
    match installed {
        Some(facade) => Ok(facade),
        None => Err(Exception::throw_message(ctx, "no entity facade is installed")),
    }
}

fn with_ec<T>(ctx: &Ctx<'_>, body: impl FnOnce(&ExecutionContext) -> T) -> rquickjs::Result<T> {
    ExecutionContext::with_current(body).ok_or_else(|| Exception::throw_message(ctx, "no execution context is entered"))
}

pub(super) fn js_error(err: impl ToString) -> Error {
    Error::new(err.to_string())
}

/// Read one name from the thread-local stack. `None` when the name is missing.
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
    if expression.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    with_context(|ctx| {
        with_widget_ec(ctx, expression, || {
            let evaluated = eval_compiled::<JsValue>(ctx, &as_expression(expression), true)?;
            js_value_to_host(ctx, &evaluated)
        })
    })
}

/// Evaluate a field's `from` expression to a host value. A timestamp, date, or time reaches a script as epoch
/// millis text, so when the expression is a plain path such as `segment.actualStartDate` and the value at that path
/// is a temporal value, the temporal value is returned, which a display formats as a date and time.
///
/// # Errors
///
/// Returns an error when the expression does not parse, throws, or gives a value that cannot be held.
pub fn eval_field_value(expression: &str) -> Result<Value, Error> {
    let value = eval_value(expression)?;
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
pub(crate) fn eval_named_value(source: &str, file_name: &str) -> Result<Value, Error> {
    if source.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    with_context(|ctx| {
        let evaluated = compiled::eval_named(ctx, source, file_name)?;
        js_value_to_host(ctx, &evaluated)
    })
}

/// Evaluate a JavaScript object expression into a map. A non-object is an error.
pub fn eval_map(expression: &str) -> Result<std::collections::BTreeMap<String, Value>, Error> {
    if expression.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    with_context(|ctx| {
        with_widget_ec(ctx, expression, || {
            let evaluated = eval_compiled::<JsValue>(ctx, &as_expression(expression), true)?;
            object_map(ctx, &evaluated)
        })
    })
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
