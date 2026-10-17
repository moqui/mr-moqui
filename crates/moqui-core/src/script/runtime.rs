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
//! `globalThis` and hosts the calls XML actions emit.

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
use rquickjs::proxy::{Proxy, ProxyHandler, ProxyProperty, ProxyReceiver, ProxyTarget};
use rquickjs::Error as JsError;
use rquickjs::{
    CatchResultExt, Class, Context, Ctx, Exception, FromJs, Function, IntoJs, JsLifetime, Null, Object, Persistent,
    Runtime, Undefined, Value as JsValue,
};

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
#[cfg(test)]
mod framework_gaps_tests;
mod helpers;
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

pub(crate) use compiled::ScriptText;
use compiled::{eval_compiled, eval_compiled_text, MAX_COMPILED_SCRIPTS, MAX_COMPILED_SCRIPT_BYTES};
use conditions::*;
use convert::*;
use decimal::*;
pub use ec_extra::expand_localized;
use entities::*;
use error_info::*;
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
/// timeout unless `script_timeout` says otherwise.
pub(crate) fn mark_request_thread() {
    REQUEST_THREAD.with(|flag| flag.set(true));
    SCRIPT_LIMIT.with(|cell| cell.set(None));
}

/// The seconds a script may run when no transaction limits it, read once per thread. `script_timeout` when set, for
/// every thread; else `webapp_request_timeout` on a request worker, and no limit on a service, job, or scheduler
/// thread, which run batch work outside a transaction as Java does. 0 is no limit.
fn default_script_limit() -> Option<Duration> {
    if let Some(limit) = SCRIPT_LIMIT.with(Cell::get) {
        return limit;
    }
    let limit = script_limit_for(
        &moqui_lib::env::get_prop_or_env("script_timeout"),
        REQUEST_THREAD.with(Cell::get),
        &moqui_lib::env::get_prop_or_env("webapp_request_timeout"),
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
    /// Lists and maps a running script read by bare name. The script mutates these objects, so
    /// they are written back to the stack at the end of the run and before a host call.
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
        let outcome = saved
            .clone()
            .restore(ctx)
            .map_err(|err| js_caught(ctx, err))
            .and_then(|value| js_value_to_host(ctx, &value));
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
    Ok(Helpers {
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

fn install_proxy(ctx: &Ctx<'_>) -> Result<(), Error> {
    let target = Object::new(ctx.clone()).map_err(js_error)?;
    let handler = ProxyHandler::new(ctx.clone())
        .map_err(js_error)?
        .with_has(proxy_has)
        .map_err(js_error)?
        .with_getter(proxy_get)
        .map_err(js_error)?
        .with_setter(proxy_set)
        .map_err(js_error)?;
    let proxy = Proxy::new(ctx.clone(), target, handler).map_err(js_error)?;
    ctx.globals().set_prototype(Some(&proxy)).map_err(js_error)?;
    Ok(())
}

fn proxy_has(target: ProxyTarget<'_>, property: ProxyProperty<'_>) -> rquickjs::Result<bool> {
    let ctx = target.0.ctx().clone();
    host_guard(&ctx, || proxy_has_inner(&target, &property))
}

fn proxy_has_inner(target: &ProxyTarget<'_>, property: &ProxyProperty<'_>) -> rquickjs::Result<bool> {
    if property.is_symbol() {
        return Ok(false);
    }
    let name = property.to_string()?;
    let ctx = target.0.ctx().clone();
    Ok(!global_has_own(&ctx, &name)?)
}

fn proxy_get<'js>(
    target: ProxyTarget<'js>,
    property: ProxyProperty<'js>,
    _receiver: ProxyReceiver<'js>,
) -> rquickjs::Result<JsValue<'js>> {
    let ctx = target.0.ctx().clone();
    host_guard(&ctx, || proxy_get_inner(&ctx, &property))
}

fn proxy_get_inner<'js>(ctx: &Ctx<'js>, property: &ProxyProperty<'js>) -> rquickjs::Result<JsValue<'js>> {
    let ctx = ctx.clone();
    if property.is_symbol() {
        return Undefined.into_js(&ctx);
    }
    let name = property.to_string()?;
    if let Some(engine) = current_engine() {
        let saved = engine.tracked.borrow().get(&name).cloned();
        if let Some(saved) = saved.or_else(|| readopt(&engine, &name)) {
            return saved.restore(&ctx);
        }
    }
    match stack_get(&name) {
        Lookup::Value(value @ (Value::List(_) | Value::Map(_))) => {
            let js = host_to_js(&ctx, value)?;
            if let Some(engine) = current_engine() {
                engine
                    .tracked
                    .borrow_mut()
                    .insert(name, Persistent::save(&ctx, js.clone()));
            }
            Ok(js)
        }
        Lookup::Value(value) => host_to_js(&ctx, value),
        Lookup::Context => context_snapshot(&ctx),
        Lookup::Missing => match helper_object(&ctx, &name)? {
            Some(helper) => helper.into_js(&ctx),
            None => Null.into_js(&ctx),
        },
    }
}

fn proxy_set<'js>(
    target: ProxyTarget<'js>,
    property: ProxyProperty<'js>,
    value: JsValue<'js>,
    _receiver: ProxyReceiver<'js>,
) -> rquickjs::Result<bool> {
    let ctx = target.0.ctx().clone();
    host_guard(&ctx, || proxy_set_inner(&ctx, &property, &value))
}

fn proxy_set_inner<'js>(ctx: &Ctx<'js>, property: &ProxyProperty<'js>, value: &JsValue<'js>) -> rquickjs::Result<bool> {
    let ctx = ctx.clone();
    if property.is_symbol() {
        return Ok(true);
    }
    let name = property.to_string()?;
    if let Some(engine) = current_engine() {
        engine.tracked.borrow_mut().remove(&name);
        engine.released.borrow_mut().remove(&name);
    }
    let host = js_to_host(&ctx, value)?;
    // A list or map put under a name stays the script's object, so `entry = list[i]; entry.field = x` changes the
    // element of the list, as it does in Java where both are the one Map. It is written to the stack at the next flush.
    let keep = matches!(host, Value::List(_) | Value::Map(_)) && is_plain_collection(value);
    STACK.with(|stack| {
        stack
            .borrow_mut()
            .put(name.clone(), host)
            .map(|_| ())
            .map_err(|err| Exception::throw_message(&ctx, err.message()))
    })?;
    if keep {
        if let Some(engine) = current_engine() {
            engine
                .tracked
                .borrow_mut()
                .insert(name, Persistent::save(&ctx, value.clone()));
        }
    }
    Ok(true)
}

/// An array or an object made by `{}` or `new Object()`, the values that convert to a host list or map and back
/// as the same shape: not a `Set`, a `Map`, a class instance or an exotic host object.
fn is_plain_collection(value: &JsValue<'_>) -> bool {
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

fn global_has_own(ctx: &Ctx<'_>, name: &str) -> rquickjs::Result<bool> {
    let globals = ctx.globals();
    let object: Object = globals.get("Object")?;
    let prototype: Object = object.get("prototype")?;
    let has_own: Function = prototype.get("hasOwnProperty")?;
    has_own.call((This(globals), name))
}

/// `context` reads as a plain object of every visible name. A write to it does not reach the stack.
fn context_snapshot<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    if let Some(engine) = current_engine() {
        readopt_all(&engine);
    }
    flush_current(ctx, false)?;
    let mut visible = STACK.with(|stack| stack.borrow().combined_map());
    visible.retain(|_, value| !matches!(value, Value::Bytes(_)));
    collection_to_js(ctx, Value::Map(visible))
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
        .map_err(|caught| Error::new(caught.to_string()))
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
    throw_diagnostic(ctx, err.message(), &err.diagnostic(), ErrorKind::of(err))
}

fn throw_context(ctx: &Ctx<'_>, err: &Error) -> JsError {
    throw_diagnostic(ctx, err.message(), &err.diagnostic(), err.kind())
}

/// Keep `error.message` unchanged for scripts that catch or classify errors. Source diagnostics
/// belong to the stack, alongside the JavaScript call site, not in the exception's message. A security kind goes on
/// the error, so it is still one when the script ends with it.
fn throw_diagnostic(ctx: &Ctx<'_>, message: &str, diagnostic: &str, kind: ErrorKind) -> JsError {
    match Exception::from_message(ctx.clone(), message) {
        Ok(exception) => {
            let detail = diagnostic.strip_prefix(message).unwrap_or(diagnostic);
            if !detail.is_empty() {
                let stack = exception.stack().unwrap_or_default();
                if let Err(err) = exception.as_object().set("stack", format!("{stack}{detail}\n")) {
                    return err;
                }
            }
            if let Err(err) = mark_kind(&exception, kind) {
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
const CARRIED_KINDS: [(ErrorKind, &str); 3] = [
    (ErrorKind::AuthenticationRequired, "AuthenticationRequired"),
    (ErrorKind::Unauthorized, "Unauthorized"),
    (ErrorKind::Tarpit, "Tarpit"),
];

/// Put `kind` on an error about to be thrown, when it is one of the [`CARRIED_KINDS`].
fn mark_kind(exception: &Exception<'_>, kind: ErrorKind) -> rquickjs::Result<()> {
    match CARRIED_KINDS.iter().find(|(carried, _)| *carried == kind) {
        Some((_, name)) => exception.as_object().set(KIND_PROPERTY, *name),
        None => Ok(()),
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
mod tests {
    use super::{
        clear_context_stack, context_value, eval_compiled, eval_condition, eval_source, eval_value, expand,
        flush_current, js_error, push_context_frame, run_script, run_script_location, set_context_name, with_ctx,
        with_screen_runner, MAX_NESTING, STACK, TOO_DEEP,
    };
    use crate::context::{Error, ExecutionContextFactory};
    use moqui_lib::object::{basic_convert, Value};
    use rquickjs::Ctx;

    const MAX_COEFFICIENT: &str = "79228162514264337593543950335";

    fn decimal(text: &str) -> Value {
        let converted = basic_convert(&Value::string(text), "BigDecimal");
        assert!(converted.changed, "expected a decimal from {text}");
        converted.value
    }

    #[test]
    fn unset_bare_name_is_null() -> Result<(), Error> {
        clear_context_stack();
        assert!(!eval_condition("unsetBareName")?);
        assert!(eval_condition("unsetBareName === null")?);
        assert!(eval_condition("typeof unsetBareName === \"object\"")?);
        assert!(!eval_condition("0")?);
        assert!(!eval_condition("false")?);
        Ok(())
    }

    #[test]
    fn stack_name_evaluates_to_its_value() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("orderId", Value::string("1000"))?;
        push_context_frame();
        set_context_name("orderId", Value::string("2000"))?;
        assert!(eval_condition("orderId === \"2000\"")?);
        assert!(eval_condition("(assignedName = \"x\") === \"x\"")?);
        set_context_name("assignedName", Value::string("y"))?;
        assert!(eval_condition("assignedName === \"y\"")?);
        set_context_name("Array", Value::string("hidden"))?;
        assert!(eval_condition("typeof Array === \"function\"")?);
        set_context_name("ObjectUtilities", Value::string("mine"))?;
        assert!(eval_condition("ObjectUtilities === \"mine\"")?);
        clear_context_stack();
        assert!(eval_condition(
            "typeof ObjectUtilities === \"object\" && ObjectUtilities !== null"
        )?);
        Ok(())
    }

    #[test]
    fn decimal_overflow_is_a_js_exception() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("big", decimal(MAX_COEFFICIENT))?;
        set_context_name("zero", decimal("0"))?;
        assert!(eval_condition("zero")?);
        assert!(eval_condition("zero.isZero()")?);
        assert!(!eval_condition("big.isZero()")?);
        assert!(!eval_condition("big == big")?);
        let overflow = eval_condition("big.add(big)").expect_err("overflow");
        assert!(overflow.to_string().contains("overflow"), "{overflow}");
        let value_of = eval_condition("big + 1").expect_err("valueOf");
        assert!(value_of.to_string().contains("valueOf"), "{value_of}");
        Ok(())
    }

    #[test]
    fn decimal_methods_take_numbers_and_text_and_to_number_is_explicit() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("price", decimal("10.25"))?;
        holds("price.add(1).eq('11.25')");
        holds("price.subtract(0.25).eq(10)");
        holds("price.multiply('2').toString() === '20.50'");
        holds("price.compare(70) === -1 && price.compare('10.250') === 0");
        holds("price.toNumber() === 10.25 && Number(price.toString()) === 10.25");
        holds("toDecimal(3).add(price).eq('13.25') && toDecimal('0.1').add(toDecimal('0.2')).eq('0.3')");
        let nan = eval_condition("price.add(NaN)").expect_err("not finite");
        assert!(nan.to_string().contains("a decimal method takes"), "{nan}");
        let text = eval_condition("price.add('abc')").expect_err("not numeric");
        assert!(text.to_string().contains("abc"), "{text}");
        let null = eval_condition("toDecimal(null)").expect_err("null");
        assert!(null.to_string().contains("toDecimal"), "{null}");
        let number = eval_condition("Number(price) > 1").expect_err("valueOf still throws");
        assert!(number.to_string().contains("toNumber"), "{number}");
        Ok(())
    }

    #[test]
    fn expand_substitutes_a_context_field() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("name", Value::string("Ada"))?;
        set_context_name("big", decimal(MAX_COEFFICIENT))?;
        assert_eq!(expand("Hello ${name}")?, "Hello Ada");
        assert_eq!(expand("Hello ${name}")?, "Hello Ada");
        assert_eq!(expand("no dollars")?, "no dollars");
        assert_eq!(expand("trailing$")?, "trailing$");
        assert_eq!(expand("${big}")?, MAX_COEFFICIENT);
        Ok(())
    }

    fn holds(expression: &str) {
        assert!(eval_condition(expression).unwrap(), "{expression}");
    }

    /// Evaluate `source` in the running script, as a statement between host calls.
    fn step(ctx: &Ctx<'_>, source: &str) -> Result<bool, Error> {
        eval_source::<bool>(ctx, source)
    }

    /// What a host call that may change the stack does first.
    fn host_call(ctx: &Ctx<'_>) -> Result<(), Error> {
        flush_current(ctx, true).map_err(js_error)
    }

    #[test]
    fn a_push_of_a_host_call_result_reaches_the_list() -> Result<(), Error> {
        clear_context_stack();
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        set_context_name("messages", Value::List(Vec::new()))?;
        set_context_name("results", Value::List(Vec::new()))?;
        set_context_name("rows", Value::List(Vec::new()))?;
        run_script(
            r#"messages.push(ec.resource.expand('Only ${n} rows.', '', {n: 3}));
            messages.push(ec.resource.expand('${messages.length} before', '', {a: 1}));
            sameScript = messages.length;
            results.push(ec.service.sync().name('no.such.Services.run#Nothing').call());
            {
                const l = rows;
                for (let i = 0; i < 3; i++) {
                    ec.service.sync().name('no.such.Services.run#Nothing').call();
                    l.push(i);
                }
            }"#,
        )?;
        holds("sameScript === 2 && messages[0] === 'Only 3 rows.' && messages[1] === '1 before'");
        holds("results.length === 1 && results[0] === null");
        holds("rows.length === 3 && rows[2] === 2");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_held_list_survives_a_host_call_that_leaves_it() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("keptList", Value::List(vec![Value::I32(1)]))?;
        with_ctx(|ctx| {
            assert!(step(ctx, "var heldKept = keptList; heldKept.push(2); true")?);
            host_call(ctx)?;
            // Not read again: the push after the call is written back at the end of the script.
            assert!(step(ctx, "heldKept.push(3); true")?);
            host_call(ctx)?;
            assert!(step(ctx, "ec.context.get('keptList').length === 3")?);
            host_call(ctx)?;
            assert!(step(ctx, "keptList === heldKept && keptList.length === 3")?);
            host_call(ctx)?;
            step(ctx, "heldKept.push(4); heldKept = null; true")
        })?;
        holds("keptList.length === 4 && keptList[3] === 4");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_host_call_that_changes_a_held_list_wins() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("replacedList", Value::List(vec![Value::I32(1)]))?;
        set_context_name("appendedList", Value::List(vec![Value::I32(1)]))?;
        with_ctx(|ctx| {
            assert!(step(
                ctx,
                "var heldReplaced = replacedList, heldAppended = appendedList; true"
            )?);
            host_call(ctx)?;
            set_context_name("replacedList", Value::List(vec![Value::I32(9)]))?;
            STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                if let moqui_lib::context_stack::Lookup::Value(Value::List(mut list)) = stack.get("appendedList") {
                    list.push(Value::I32(8));
                    stack.put_existing("appendedList", Value::List(list));
                }
            });
            assert!(step(ctx, "heldReplaced.push(2); heldAppended.push(3); true")?);
            assert!(step(
                ctx,
                "replacedList !== heldReplaced && replacedList.length === 1 && replacedList[0] === 9"
            )?);
            step(
                ctx,
                "heldReplaced = heldAppended = null; appendedList.length === 2 && appendedList[1] === 8",
            )
        })
        .map(|seen| assert!(seen))?;
        holds("replacedList.length === 1 && replacedList[0] === 9");
        holds("appendedList.length === 2 && appendedList[1] === 8");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_nested_script_that_changes_a_held_list_wins() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("nestedChanged", Value::List(vec![Value::I32(1)]))?;
        set_context_name("nestedRead", Value::List(vec![Value::I32(1)]))?;
        with_ctx(|ctx| {
            assert!(step(
                ctx,
                "var heldChanged = nestedChanged, heldRead = nestedRead; true"
            )?);
            host_call(ctx)?;
            run_script("nestedChanged.push(5); nestedReadLength = nestedRead.length")?;
            assert!(step(ctx, "heldChanged.push(7); heldRead.push(6); true")?);
            assert!(step(
                ctx,
                "nestedChanged !== heldChanged && nestedChanged.length === 2 && nestedChanged[1] === 5"
            )?);
            step(
                ctx,
                "var same = nestedRead === heldRead && nestedReadLength === 1; heldChanged = heldRead = null; same",
            )
        })
        .map(|same| assert!(same))?;
        holds("nestedChanged.length === 2 && nestedRead.length === 2 && nestedRead[1] === 6");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_compiled_script_runs_again_with_the_context_of_each_run() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("n", Value::I32(1))?;
        run_script("total = n * 10")?;
        assert_eq!(
            context_value("total").map(|v| moqui_lib::object::to_plain_string(&v)),
            Some("10".to_string())
        );
        set_context_name("n", Value::I32(4))?;
        run_script("total = n * 10")?;
        assert_eq!(
            context_value("total").map(|v| moqui_lib::object::to_plain_string(&v)),
            Some("40".to_string())
        );
        assert!(eval_condition("n > 3")?);
        set_context_name("n", Value::I32(2))?;
        assert!(!eval_condition("n > 3")?);
        let failed = run_script("this is not javascript").unwrap_err();
        assert!(failed.message().contains("expecting"), "{}", failed.message());
        let thrown = run_script("throw new Error('boom ' + n)").unwrap_err();
        assert!(thrown.message().contains("boom 2"), "{}", thrown.message());
        Ok(())
    }

    fn too_deep(outcome: Result<impl std::fmt::Debug, Error>) {
        let err = outcome.expect_err("cyclic or too deep");
        assert!(err.message().contains(TOO_DEEP), "{}", err.message());
    }

    #[test]
    fn a_cyclic_or_too_deep_value_is_an_error() -> Result<(), Error> {
        clear_context_stack();
        too_deep(run_script("var a = {}; a.self = a; result = a;"));
        too_deep(eval_value("(() => { var a = []; a.push(a); return a; })()"));
        too_deep(eval_value("(() => { var s = new Set(); s.add([s]); return s; })()"));
        too_deep(eval_value(
            "(() => { var a = []; for (var i = 0; i < 100000; i++) a = [a]; return a; })()",
        ));
        too_deep(run_script("items = []; items.push(items);"));
        Ok(())
    }

    #[test]
    fn a_shared_value_converts_once_for_each_place() -> Result<(), Error> {
        clear_context_stack();
        let value = eval_value("(() => { var s = {x: 1}; return {a: s, b: [s, s], c: new Set([s])}; })()")?;
        let Value::Map(map) = value else {
            panic!("expected a map, got {value:?}")
        };
        let Some(Value::List(both)) = map.get("b") else {
            panic!("expected a list, got {map:?}")
        };
        assert_eq!(map.get("a"), both.get(1));
        assert!(matches!(map.get("a"), Some(Value::Map(shared)) if shared.get("x") == Some(&Value::I32(1))));
        let deepest = format!("(() => {{ var a = []; for (var i = 1; i < {MAX_NESTING}; i++) a = [a]; return a; }})()");
        set_context_name("deep", eval_value(&deepest)?)?;
        assert!(eval_condition("deep.length === 1 && deep[0][0].length === 1")?);
        too_deep(run_script("wrapped = [deep];"));
        let mut host = Value::List(Vec::new());
        for _ in 0..1000 {
            host = Value::List(vec![host]);
        }
        set_context_name("hostDeep", host)?;
        too_deep(eval_condition("hostDeep.length === 1"));
        Ok(())
    }

    #[test]
    fn an_integer_past_the_safe_range_is_an_error() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("smallest", Value::I64(i64::MIN))?;
        let err = eval_condition("smallest < 0").expect_err("i64::MIN is not exact");
        assert!(err.message().contains("too large"), "{}", err.message());
        set_context_name("safe", Value::I64(-9_007_199_254_740_991))?;
        assert!(eval_condition("safe === -Number.MAX_SAFE_INTEGER")?);
        Ok(())
    }

    fn plain(name: &str) -> Option<String> {
        context_value(name).map(|value| moqui_lib::object::to_plain_string(&value))
    }

    #[test]
    fn a_script_file_runs_in_names_of_its_own() -> Result<(), Error> {
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        clear_context_stack();
        let dir = std::env::temp_dir().join(format!("moqui-file-scope-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(js_error)?;
        let write = |name: &str, text: &str| -> Result<String, Error> {
            let path = dir.join(name);
            std::fs::write(&path, text).map_err(js_error)?;
            Ok(path.to_string_lossy().into_owned())
        };
        let declares = write(
            "Declares.js",
            "const base = n * 2;\nlet more = base + 1;\nvar shadow = 'file';\nfunction twice(x) { return x * 2; }\nresult = twice(more);\n",
        )?;
        let stops = write("Stops.js", "if (stop) return;\nreached = true;\n")?;
        let throws = write("Throws.js", "thrown = 1;\n\nthrow new Error('on line three');\n")?;

        // The same declarations run again on this thread's engine, and the bare assignment reaches the context.
        set_context_name("n", Value::I32(1))?;
        run_script_location(&declares)?;
        assert_eq!(plain("result"), Some("6".to_string()));
        set_context_name("n", Value::I32(2))?;
        run_script_location(&declares)?;
        assert_eq!(plain("result"), Some("10".to_string()));
        // A `var` or `function` of the file is not a global that hides the context name in a later script.
        set_context_name("shadow", Value::string("context"))?;
        holds("shadow === 'context' && twice === null && base === null");

        set_context_name("stop", Value::Bool(true))?;
        run_script_location(&stops)?;
        assert_eq!(plain("reached"), None);
        set_context_name("stop", Value::Bool(false))?;
        run_script_location(&stops)?;
        assert_eq!(plain("reached"), Some("true".to_string()));

        let err = run_script_location(&throws).expect_err("throws");
        assert!(err.message().contains(&format!("{throws}:3:")), "{}", err.message());
        assert!(!err.diagnostic().contains("Rust:"), "{}", err.diagnostic());
        assert_eq!(plain("thrown"), Some("1".to_string()));

        // The host's globals are read-only: an assignment throws, in sloppy code too, and does not replace them for the
        // later scripts of the thread.
        let assigned = run_script("compare = 'a context value';").expect_err("compare is a host global");
        assert!(
            assigned
                .message()
                .contains("compare is a host global and cannot be assigned"),
            "{}",
            assigned.message()
        );
        assert!(run_script("toNumber = null;").is_err());
        run_script("ec = null; sri = null;")?;
        holds("typeof toNumber === 'function' && typeof compare === 'function' && ec !== null && ec.resource != null");
        holds("sri !== null && typeof sri.pushContext === 'function'");
        clear_context_stack();
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn an_error_or_a_panic_out_of_a_run_leaves_no_tracked_values() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("heldRows", Value::List(vec![Value::I32(1)]))?;
        let failed = with_ctx(|ctx| -> Result<(), Error> {
            assert!(step(ctx, "heldRows.push(2); true")?);
            Err(Error::new("stopped"))
        });
        assert!(failed.is_err());
        holds("heldRows.length === 2");
        let panicked = std::panic::catch_unwind(|| {
            with_ctx(|ctx| -> Result<(), Error> {
                assert!(step(ctx, "heldRows.length === 2")?);
                panic!("out of the body");
            })
        });
        assert!(panicked.is_err());
        set_context_name("heldRows", Value::List(Vec::new()))?;
        holds("heldRows.length === 0");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_callback_panic_in_a_compiled_script_is_that_scripts_error() -> Result<(), Error> {
        clear_context_stack();
        with_ctx(|ctx| {
            let unguarded =
                rquickjs::Function::new(ctx.clone(), || -> i32 { panic!("unguarded callback") }).map_err(js_error)?;
            // Defined, not set: a set on `globalThis` goes to the context stack.
            ctx.globals()
                .prop("unguardedPanic", rquickjs::object::Property::from(unguarded))
                .map_err(js_error)?;
            let err = eval_compiled::<rquickjs::Value>(ctx, "unguardedPanic()", false).expect_err("panics");
            assert!(err.message().contains("unguarded callback"), "{}", err.message());
            // A panic left in the runtime would be resumed at the next exception rquickjs sees.
            let later = eval_source::<rquickjs::Value>(ctx, "throw new Error('a later error')").expect_err("throws");
            assert!(later.message().contains("a later error"), "{}", later.message());
            Ok(())
        })
    }

    #[test]
    fn paginate_list_leaves_a_page_from_a_paginated_find_as_it_is() -> Result<(), Error> {
        clear_context_stack();
        // Page 2 of a paginated find: the list is the page, and the count and range are those of the whole result.
        run_script(
            "rows = [{a: 21}, {a: 22}]; rowsCount = 22; rowsPageRangeLow = 21; rowsPageRangeHigh = 22;
            CollectionUtilities.paginateList('rows', null, context)",
        )?;
        holds("rows.length === 2 && rows[0].a === 21");
        run_script(
            "all = [1, 2, 3]; pageSize = 2; pageIndex = 1; CollectionUtilities.paginateList('all', 'allPage', context)",
        )?;
        holds("allPage.length === 1 && allPage[0] === 3 && allPageCount === 3 && all.length === 3");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_unit_that_throws_after_a_push_leaves_the_stack_as_it_found_it() -> Result<(), Error> {
        clear_context_stack();
        set_context_name("level", Value::string("outer"))?;
        let unit = crate::screen::generate::Unit {
            id: u64::MAX - 1,
            source: "(function () { sri.pushContext(); level = 'inner'; throw new Error('in the unit'); })".to_string(),
            runtime: std::sync::Arc::from(""),
            runtime_id: 0,
        };
        let err = with_screen_runner(|runner| runner.run(&unit)).expect_err("throws");
        assert!(err.message().contains("in the unit"), "{}", err.message());
        holds("level === 'outer'");
        clear_context_stack();
        Ok(())
    }

    #[test]
    fn a_loop_outside_a_transaction_stops_at_the_default_limit() -> Result<(), Error> {
        use std::time::{Duration, Instant};
        clear_context_stack();
        // As a `script_timeout` of 200 ms would set it.
        super::SCRIPT_LIMIT.with(|limit| limit.set(Some(Some(Duration::from_millis(200)))));
        let started = Instant::now();
        let err = run_script("while (true) {}").expect_err("interrupted");
        assert!(err.message().contains("interrupted"), "{}", err.message());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            super::script_time_remaining(),
            None,
            "the default goes when the script ends"
        );
        // A transaction's own limit replaces the default inside it, and the time inside does not count against it.
        with_ctx(|_| {
            assert!(super::script_time_remaining().is_some_and(|left| left <= Duration::from_millis(200)));
            {
                let _tx = super::DeadlineGuard::new(Some(Duration::from_secs(30)));
                assert!(super::script_time_remaining().is_some_and(|left| left > Duration::from_secs(20)));
                std::thread::sleep(Duration::from_millis(300));
            }
            assert!(super::script_time_remaining().is_some_and(|left| left > Duration::ZERO));
            Ok(())
        })?;
        super::SCRIPT_LIMIT.with(|limit| limit.set(Some(None)));
        assert!(eval_condition("true")?);
        assert_eq!(super::script_time_remaining(), None);
        Ok(())
    }

    #[test]
    fn the_default_limit_is_script_timeout_else_the_request_timeout_on_a_request_worker() {
        use std::time::Duration;
        let limit = super::script_limit_for;
        assert_eq!(limit("", false, ""), None, "a service or job thread");
        assert_eq!(limit("", true, ""), Some(Duration::from_secs(300)));
        assert_eq!(limit("", true, "90"), Some(Duration::from_secs(90)));
        assert_eq!(limit("", true, "0"), None);
        assert_eq!(limit("20", false, ""), Some(Duration::from_secs(20)));
        assert_eq!(limit("20", true, "90"), Some(Duration::from_secs(20)));
        assert_eq!(limit("0", true, "90"), None);
        assert_eq!(limit("junk", false, ""), None);
    }

    #[test]
    fn deep_recursion_in_a_nested_script_is_a_script_error() {
        // Less stack than a worker thread's 2 MiB, so a nested engine with a whole 1 MiB of its own under the outer
        // script's 1 MiB, as before the nested engines had a budget, overflows the thread.
        let outcome = std::thread::Builder::new()
            .stack_size(1536 * 1024)
            .spawn(|| {
                with_ctx(|ctx| {
                    let most: i64 = eval_source(
                        ctx,
                        "(function () { let most = 0; function down(n) { most = n; down(n + 1); } \
                        try { down(0); } catch (e) {} return most; })()",
                    )?;
                    let nested = rquickjs::Function::new(ctx.clone(), || {
                        match run_script("function down(n) { return down(n + 1) + 1; } down(0);") {
                            Ok(()) => "ran".to_string(),
                            Err(err) => err.message().to_string(),
                        }
                    })
                    .map_err(js_error)?;
                    let deep: rquickjs::Function = eval_source(
                        ctx,
                        "(function (nested, depth) { function deep(n) { return n === 0 ? nested() : deep(n - 1); } \
                        return deep(depth); })",
                    )?;
                    deep.call::<_, String>((nested, most * 9 / 10)).map_err(js_error)
                })
            })
            .unwrap()
            .join()
            .expect("the thread did not overflow");
        let message = outcome.unwrap();
        assert!(message.contains("stack"), "{message}");
    }

    #[test]
    fn request_scripts_have_room_for_nested_app_templates() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                super::mark_request_thread();
                assert_eq!(super::stack_budget(0).unwrap(), super::REQUEST_SCRIPT_STACK);
                assert!(super::stack_budget(1).unwrap() > super::NESTED_STACK);
                let err = run_script("function down(n) { return down(n + 1) + 1; } down(0);").unwrap_err();
                assert!(err.message().contains("stack"), "{err}");
            })
            .unwrap()
            .join()
            .expect("request recursion must fail as a script error, not overflow the worker");
    }

    #[test]
    fn running_out_of_memory_frees_the_engine_caches() -> Result<(), Error> {
        clear_context_stack();
        assert!(eval_condition("1 + 1 === 2")?);
        let cached = || super::ENGINES.with(|engines| engines.borrow()[0].scripts.borrow().len());
        assert!(cached() > 0);
        let err = run_script("(function () { const hog = []; while (true) hog.push(new Array(100000).fill(1)); })()")
            .expect_err("out of memory");
        assert!(err.message().contains("out of memory"), "{}", err.message());
        assert_eq!(cached(), 0, "the caches went with the failed script");
        assert!(eval_condition("1 + 1 === 2")?, "the engine runs again");
        Ok(())
    }
}
