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
//! The script entries the framework calls (an expression, a condition, a template, actions, a script file), each run on
//! the thread's quickjs-oxide runtime (`script::oxide`), and the thread-local context stack bare names read and write.

use std::cell::Cell;

use moqui_lib::context_stack::{ContextStack, Lookup};
use moqui_lib::object::Value;

mod convert;
mod decimal;
mod encode;
#[cfg(test)]
mod ec_extra_tests;
#[cfg(test)]
mod entities_tests;
mod facades;
#[cfg(test)]
mod framework_gaps_tests;
mod helpers;
mod localize;
#[cfg(test)]
mod metadata_tests;
mod values;

pub(crate) use crate::script::text::ScriptText;
pub(crate) use convert::{converts_with_script, has_special_key, temporal_text, MAX_NESTING, SAFE_INTEGER, SET_KEY, TOO_DEEP, WRAP_KEY};
pub(crate) use crate::script::host::rounding_strategy_named as rounding_named;
pub(crate) use decimal::{arith, decimal_of, decimals_compare, decimals_equal, Arith};
pub(crate) use encode::{code128_options, encode_html_js_safe, flatten_nested_map, js_safe_map, url_safe_chars};
pub(crate) use crate::script::host::entities::{
    entity_field_names, entity_is_field, entity_set_field, EntityCell, EntityField,
};
pub use facades::{forget_scripts, run_script_location};
pub(crate) use facades::{module_cache_on, read_module};
pub(crate) use helpers::{java_type_of, now_timestamp, timestamp_of, HELPERS_JS};
pub use localize::expand_localized;
pub(crate) use localize::localized;
pub use values::script_list_values;
pub(crate) use values::{context_combined, pop_context_frame, put_context_all, value_entity_name, value_field};
pub use values::{value_items, value_map};

use crate::context::{Error, ErrorKind};
use crate::script::engine::EntryKind;
pub use crate::script::run::{interrupt_thread_on, limit_thread_scripts, thread_interrupted, DeadlineGuard};
pub(crate) use crate::script::run::STACK;
#[cfg(test)]
use crate::script::run::{script_limit_for, stack_budget, NESTED_STACK, REQUEST_SCRIPT_STACK, SCRIPT_LIMIT};
pub(crate) use crate::script::run::{mark_request_thread, script_time_remaining};

pub(crate) const VALUE_OF_MESSAGE: &str =
    "decimal valueOf is not supported; use add, subtract, multiply, divide, eq, compare, isZero, or toNumber";

thread_local! {
    /// While true, an expression of a widget runs with the `ec` of the server-static browser host: the compatibility
    /// report's full check.
    static WIDGET_WRAP: Cell<bool> = const { Cell::new(false) };
}

/// Whether widget expressions run with the wrapped `ec` now (the server-static report's full check).
pub(crate) fn widget_wrap_on() -> bool {
    WIDGET_WRAP.with(Cell::get)
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

/// The `ec` of the browser host as a wrapper over the real one: the members `plans/archive/QvtServerStatic.md` lists are the real members,
/// and any other area, or any other member of an allowed area (`ec.l10n.getAvailableLocales`), is reported through `deny` and
/// throws, so the check records it rather than the expression failing on an undefined member. Symbol keys (a coercion's
/// `Symbol.toPrimitive`) and the probes of a plain object (`toString`, `valueOf` and the other `Object.prototype`
/// members a template's coercion or a check reads; `then` and `toJSON`, undefined there) are as on a plain object, not
/// violations.
pub(crate) const WRAPPED_EC: &str = r#"(function (real, deny) {
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

/// Note `expression` as the one a widget check records against, when the check is on: quickjs-oxide's wrapper
/// (`oxide::widget`) sees the text only as it runs (`as_expression`'s form, a template literal).
pub(crate) fn note_widget_expression(expression: &str) {
    if WIDGET_WRAP.with(Cell::get) {
        crate::screen::statics::set_expression(expression);
    }
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
    pub fn push_entries(mut entries: moqui_lib::object::ValueMap) -> Self {
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
    pub fn take(self) -> moqui_lib::object::ValueMap {
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

/// The names set in the top context frame, as an entity ECA rule's `set-results` reads them.
pub fn top_context_entries() -> moqui_lib::object::ValueMap {
    STACK.with(|stack| stack.borrow().top_entries())
}

pub fn set_context_name(name: &str, value: Value) -> Result<(), Error> {
    STACK.with(|stack| stack.borrow_mut().put(name, value).map(|_| ()).map_err(Error::from))
}

/// Replace the thread-local stack with an empty one. The runtime stays.
pub fn clear_context_stack() {
    STACK.with(|stack| *stack.borrow_mut() = ContextStack::new());
}


/// What the script engine of this thread holds: its quickjs-oxide runtime (0 or 1), the bytes its heap reserves and
/// the live bytes its last collection left, its compiled units and their instruction, position and source bytes, and
/// its compiled-expression cache's texts and their source bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineMemory {
    pub oxide_engines: usize,
    pub oxide_reserved_bytes: usize,
    pub oxide_live_bytes: usize,
    pub oxide_code_units: usize,
    pub oxide_code_bytes: usize,
    pub oxide_compiled_scripts: usize,
    pub oxide_compiled_script_bytes: usize,
}

/// One line of this thread's quickjs-oxide heaps after a collection (the main runtime's and the spec runtime's), for a
/// per-spec heap log.
pub fn oxide_heap_line() -> String {
    crate::script::oxide::heap_line()
}

/// [`EngineMemory`] of this thread's engine, after a collection when `collect`. Not while a script runs on the thread.
pub fn engine_memory(collect: bool) -> EngineMemory {
    let oxide = crate::script::oxide::engine_memory(collect);
    EngineMemory {
        oxide_engines: oxide.engines,
        oxide_reserved_bytes: oxide.reserved_bytes,
        oxide_live_bytes: oxide.live_bytes,
        oxide_code_units: oxide.code_units,
        oxide_code_bytes: oxide.code_bytes,
        oxide_compiled_scripts: oxide.compiled_scripts,
        oxide_compiled_script_bytes: oxide.compiled_script_bytes,
    }
}

/// This thread's script runtime's collector counters and, in an `ic-stats` build, its inline-cache report with the
/// `worst` sites (plans/Profiling3.md M6).
pub fn engine_counters(worst: usize) -> String {
    crate::script::oxide::engine_counters(worst)
}

/// What the process's script engines have done since it started.
#[derive(Clone, Debug, Default)]
pub struct EngineStats {
    /// The deepest nesting of scripts.
    pub depth_max: usize,
    /// The quickjs-oxide runtimes (one per thread that ran a script, and the test harness's): started, alive now and
    /// at most.
    pub oxide_engines_started: usize,
    pub oxide_engines_live: usize,
    pub oxide_engines_live_max: usize,
    /// Their reserved heap bytes as last sampled, now and at the highest.
    pub oxide_heap_bytes: usize,
    pub oxide_heap_bytes_max: usize,
    /// The peak reserved heap bytes of each thread's oxide runtime (sampled every 32 runs), by thread.
    pub oxide_heap_peaks: Vec<(String, usize)>,
    /// The highest total of the runtimes' reserved heap bytes (to a MiB), and each runtime's share of it by thread
    /// name, the largest first.
    pub oxide_heap_at_max: (usize, Vec<(String, usize)>),
    /// Texts the oxide runtimes compiled.
    pub oxide_compiled: u64,
    /// Oxide runtimes dropped after a host function panicked.
    pub oxide_poisoned: u64,
    /// Runtimes dropped and started again because their heap stayed near the cap with their caches gone.
    pub oxide_restarted: u64,
    /// Oxide runtimes dropped and started fresh because their reserved heap was far past their live bytes.
    pub oxide_shed_fragmented: u64,
    /// Runtimes let go by a thread that waited for work ([`release_idle_runtime`]).
    pub oxide_released_idle: u64,
    /// The process's oxide code images: made, loaded by a runtime instead of compiled (each a parse avoided), failed,
    /// and the bytes kept.
    pub oxide_images_made: u64,
    pub oxide_image_loads: u64,
    pub oxide_image_failures: u64,
    pub oxide_image_bytes: usize,
    /// Scripts run by entry kind: (kind, runs).
    pub scripts_by_kind: Vec<(&'static str, u64)>,
}

/// [`EngineStats`] of the process.
pub fn engine_stats() -> EngineStats {
    use std::sync::atomic::Ordering::Relaxed;
    let oxide = crate::script::oxide::stats();
    EngineStats {
        depth_max: crate::script::run::DEPTH_MAX.load(Relaxed),
        oxide_engines_started: oxide.engines_started,
        oxide_engines_live: oxide.engines_live,
        oxide_engines_live_max: oxide.engines_live_max,
        oxide_heap_bytes: oxide.heap_bytes,
        oxide_heap_bytes_max: oxide.heap_bytes_max,
        oxide_heap_peaks: crate::script::oxide::thread_heap_peaks(),
        oxide_heap_at_max: crate::script::oxide::heap_at_max(),
        oxide_compiled: oxide.compiled,
        oxide_poisoned: oxide.poisoned,
        oxide_restarted: oxide.restarted,
        oxide_shed_fragmented: oxide.shed_fragmented,
        oxide_released_idle: oxide.released_idle,
        oxide_images_made: oxide.images.made,
        oxide_image_loads: oxide.images.loads,
        oxide_image_failures: oxide.images.failed,
        oxide_image_bytes: oxide.images.bytes,
        scripts_by_kind: crate::script::engine::scripts_by_kind(),
    }
}

/// Let this thread's script runtime go when no script runs on it: a pool worker that waited for work a while gives back
/// the heap its runtime holds, and its next script starts one again. Whether one was let go.
pub fn release_idle_runtime() -> bool {
    crate::script::oxide::release_idle()
}

/// A script's text with a null byte in it is refused before it reaches the engine.
fn refuse_null(source: &str) -> Result<(), Error> {
    if source.as_bytes().contains(&0) {
        return Err(Error::new("script source contains a null byte"));
    }
    Ok(())
}

/// Run one script statement. The script is not strict.
pub fn run_script(source: &str) -> Result<(), Error> {
    refuse_null(source)?;
    crate::script::engine::run(EntryKind::Script, source, |source, hash, _| {
        crate::script::oxide::run_script(source, hash)
    })
}

/// Evaluate a strict expression. Null, false, and a zero number are false. A decimal is true.
///
/// This is JavaScript truth, as an `<if>` in the actions has: an empty list or map is true, where Java's
/// `XmlAction.checkCondition` (`as Boolean`) makes it false. A rule condition and an `<if>` agree, and converted Groovy
/// tests a list's length instead (`groovy_convert`; the linter flags the rest, `lint_hazards`).
pub fn eval_condition(expression: &str) -> Result<bool, Error> {
    refuse_null(expression)?;
    let source = as_expression(expression);
    note_widget_expression(expression);
    crate::script::engine::run(EntryKind::Condition, &source, crate::script::oxide::eval_condition)
}

/// Expand a template. `${expression}` runs in this runtime. No `$`, or a trailing `$`, is unchanged.
pub fn expand(template: &str) -> Result<String, Error> {
    if !template.contains('$') || template.ends_with('$') {
        return Ok(template.to_string());
    }
    refuse_null(template)?;
    // the template literal itself, run as an expression
    let literal = template_literal(template);
    note_widget_expression(template);
    crate::script::engine::run(EntryKind::Expand, &literal, crate::script::oxide::eval_text)
}

/// Run one generated action script whose text and cache key were made once.
///
/// The script is not strict. A strict function would reject bare assignment, and
/// that assignment is how a plain value reaches the context stack.
pub(super) fn eval_script_text(script: &ScriptText) -> Result<(), Error> {
    crate::script::engine::run_hashed(EntryKind::Actions, script.text(), script.hash(), |source, hash, _| {
        crate::script::oxide::run_actions(source, hash, script.file_name())
    })
}

/// Evaluate a JavaScript expression to a host value.
///
/// # Errors
///
/// Returns an error when the expression does not parse, throws, or gives a value that cannot be held.
pub fn eval_value(expression: &str) -> Result<Value, Error> {
    eval_value_as(EntryKind::Value, expression)
}

/// [`eval_value`] for an entry of `kind` (a value, a field's value).
fn eval_value_as(kind: EntryKind, expression: &str) -> Result<Value, Error> {
    refuse_null(expression)?;
    let source = as_expression(expression);
    note_widget_expression(expression);
    crate::script::engine::run(kind, &source, crate::script::oxide::eval_value)
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

/// Evaluate a JavaScript object expression into a map. A non-object is an error.
pub fn eval_map(expression: &str) -> Result<std::collections::BTreeMap<String, Value>, Error> {
    refuse_null(expression)?;
    let source = as_expression(expression);
    note_widget_expression(expression);
    crate::script::engine::run(EntryKind::Map, &source, crate::script::oxide::eval_map)
}

/// Run a script once, not kept, named `file_name` in stack traces, and return its value as a host value. The test
/// harness runs a spec file with it, so an error names the spec file and line.
///
/// For the test harnesses only (moqui-devkit's spec engine, moqui-llm's agent specs): it runs in the thread's spec
/// runtime, whose globals (`call`, `login`, ...) are the harness's, apart from the scripts of services and screens.
/// Application code runs scripts through [`run_script`] and the other entries.
pub fn eval_named_value(source: &str, file_name: &str) -> Result<Value, Error> {
    refuse_null(source)?;
    crate::script::oxide::spec::eval_named_value(source, file_name)
}

/// Define the global `name` in the thread's spec runtime as an object of the host table's `class`, writable and
/// configurable (the test harness's `__moquiTestHost`). A plain set of a global would go to the context stack. For
/// the test harnesses only, as [`eval_named_value`].
pub fn install_spec_global(name: &str, class: &'static crate::script::host::HostClass) -> Result<(), Error> {
    crate::script::oxide::spec::install_global(name, class)
}

/// Whether a host function's path may be called ([`restrict_host_calls`]).
type HostAllow = fn(&str) -> bool;

thread_local! {
    static HOST_ALLOWED: std::cell::Cell<Option<HostAllow>> = const { std::cell::Cell::new(None) };
}

/// From now on this thread's scripts may call only the host functions whose path (`System.getProperty`,
/// `ec.service.sync`, `__moquiTestHost.call`) `allow` passes; any other throws. For a thread that runs code the host
/// does not trust with the server's own functions: a load script's users in a server run (plans/LoadRunner.md §10),
/// which get only the client harness's globals. A [`HostCallsLifted`] lets a host call that runs the server's own code
/// (a service) call them again while it runs.
pub fn restrict_host_calls(allow: fn(&str) -> bool) {
    HOST_ALLOWED.with(|allowed| allowed.set(Some(allow)));
}

/// Whether a script on this thread may call the host function at `path` ([`restrict_host_calls`]).
pub(crate) fn host_call_allowed(path: &str) -> bool {
    HOST_ALLOWED.with(|allowed| allowed.get().is_none_or(|allow| allow(path)))
}

/// The restriction of [`restrict_host_calls`] lifted until this is dropped: what a restricted thread's host runs of
/// the server's own (a service call) is not restricted.
#[must_use]
pub struct HostCallsLifted(Option<HostAllow>);

impl HostCallsLifted {
    pub fn new() -> Self {
        Self(HOST_ALLOWED.with(std::cell::Cell::take))
    }
}

impl Default for HostCallsLifted {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HostCallsLifted {
    fn drop(&mut self) {
        let held = self.0.take();
        HOST_ALLOWED.with(|allowed| allowed.set(held));
    }
}

/// Run `body` with a [`crate::screen::sri::Runner`] for the render in progress, on the thread's runtime.
pub(crate) fn with_screen_runner<R>(
    body: impl FnOnce(&dyn crate::screen::sri::Runner) -> Result<R, Error>,
) -> Result<R, Error> {
    crate::script::oxide::units::with_screen_runner(body)
}

/// Run the web handler script at `location` for an event of `kind` with `data`.
///
/// # Errors
///
/// Returns an error when the file cannot be read, does not evaluate to an object, or the handler throws.
pub(crate) fn invoke_web_handler(location: &str, kind: &str, data: Value) -> Result<(), Error> {
    crate::script::oxide::web::invoke(location, kind, data)
}

/// A source that starts with `{` is an object literal in an expression, not a block.
pub(crate) fn as_expression(source: &str) -> std::borrow::Cow<'_, str> {
    if source.trim_start().starts_with('{') {
        std::borrow::Cow::Owned(format!("({source}\n)"))
    } else {
        std::borrow::Cow::Borrowed(source)
    }
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

/// The property of a thrown error that names its security kind, so a security error is still one after it has
/// crossed the script and come back: an authorization failure in a service's script is still one for the call, as
/// Java's `ArtifactAuthorizationException` passes through Groovy. It goes with that one error: one a script catches
/// and goes on from does not mark a later failure, and a nested unit does not change the outer one's.
pub(crate) const KIND_PROPERTY: &str = "moquiErrorKind";

/// The error kinds a thrown error carries across a script (as [`KIND_PROPERTY`]), by name.
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

/// The property of a thrown error that holds a tarpit's retry seconds, so the `Retry-After` of a tarpit refused under
/// a script (a screen's render) is still sent.
pub(crate) const RETRY_AFTER_PROPERTY: &str = "moquiRetryAfter";

/// The kind a thrown error names `name` ([`KIND_PROPERTY`]), for an error that crossed a script.
pub(crate) fn carried_kind(name: &str) -> Option<ErrorKind> {
    CARRIED_KINDS.iter().find(|(_, known)| *known == name).map(|(kind, _)| *kind)
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

fn stack_get(name: &str) -> Lookup {
    STACK.with(|stack| stack.borrow().get(name))
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
