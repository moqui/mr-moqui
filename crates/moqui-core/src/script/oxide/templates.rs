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

//! Fancy templates on quickjs-oxide under `script_engine=oxide` (plans/Script-Engine-Oxide.md section 5.4, Phase 3).
//! Under `rquickjs` and `small-oxide` they run on QuickJS (`script/template.rs`).
//!
//! - **The live form** (the trusted renderer) is a run on the thread's oxide runtime: the compiled function called at
//!   once, its bare names on the context stack through the runtime's global names, as the other kinds run there.
//! - **The snapshot form** (the locked-down renderer) runs in a sandbox runtime of its own on each thread, as on
//!   QuickJS: no `ec`, no host object, only `format`, `localize`, `t` and the frozen built-ins
//!   (`template::SANDBOX_LOCKDOWN`), a heap cap and a deadline polled through `Host::interrupt`. quickjs-oxide sets
//!   its heap cap when a runtime starts, so a theme render's lower memory limit runs in a sandbox of that limit
//!   rounded down to a power of two ([`bucket`]), kept with the default one for the thread's next render at that cap;
//!   a limit under 1 MiB renders nothing ([`LEAST_HEAP`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use quickjs_oxide::engine::api::core::{
    CallableRef, CompileOptions, Context, FunctionOptions, HeapLimits, Host, JsResult, JsString, NativeErrorKind,
    OrdinaryPropertyDescriptor, Runtime, RuntimeError, RuntimeOptions, Scope, Stream, Value,
};

use super::errors::{runtime_error, throw_plain};
use crate::context::Error;
use crate::script::engine::{script_engine, EntryKind, Route, ScriptEngine};
use crate::script::run::{self, EngineTag};
use crate::script::template;
use crate::script::BoundedMap;

/// Whether templates run on quickjs-oxide: under `script_engine=oxide` only.
pub(crate) fn on_oxide() -> bool {
    script_engine() == ScriptEngine::Oxide
}

// ---- the live form ----

/// The text that calls a compiled template function, and its hash.
type Call = (Rc<str>, u64);

thread_local! {
    /// The [`Call`] of each compiled template function, by its text: built once per template.
    static CALLS: RefCell<BoundedMap<Rc<str>, Call>> = RefCell::new(BoundedMap::new(MAX_FUNCTIONS));
}

/// Run a compiled live-form template function (`(function(){...})`) on the thread's oxide runtime and give its text.
/// Its bare names read and write the context stack, and what it writes there is written back at its end. A run inside
/// a running QuickJS script writes that script's tracked values back first (`engine::cross_into_oxide`).
///
/// # Errors
///
/// The template's own error, or an error naming what the oxide bindings do not have yet.
pub(crate) fn run_live(function: &str) -> Result<String, Error> {
    if run::is_running(EngineTag::Oxide) && !super::is_lent() {
        return Err(Error::new(
            "script_engine=oxide: a template nested in a running quickjs-oxide script needs the re-entry handle (F3a)",
        ));
    }
    let (call, hash) = CALLS.with(|calls| {
        if let Some(found) = calls.borrow_mut().get(function) {
            return found;
        }
        let call: Rc<str> = Rc::from(format!("{function}()"));
        let hash = crate::script::text::hash_text(&call);
        calls.borrow_mut().insert(Rc::from(function), (Rc::clone(&call), hash));
        (call, hash)
    });
    let ran = crate::script::engine::cross_into_oxide(true, || super::eval_text(&call, hash, true))?;
    crate::script::engine::count(EntryKind::Template, Route::Oxide);
    ran.map_err(|outcome| match outcome {
        super::Outcome::Error(err) => err,
        super::Outcome::Fallback(reason) => Error::new(format!(
            "script_engine=oxide: this template needs {reason}, which the quickjs-oxide bindings do not have yet"
        )),
    })
}

// ---- the locked-down sandbox ----

/// A host function of the sandbox, given the `Error` constructor it throws with.
type SandboxFunction = fn(&mut Scope<'_>, &CallableRef) -> JsResult<Value>;

/// Compiled template functions a sandbox keeps.
const MAX_FUNCTIONS: usize = 4096;
/// Templates compiled to static code in a sandbox, and their source bytes; later ones are transient, so text that
/// changes on every render cannot grow the runtime's code past its cap.
const MAX_STATIC: usize = 1024;
const MAX_STATIC_BYTES: usize = 4 * 1024 * 1024;
/// Sandboxes kept per thread: the default cap's and the lower caps theme renders asked for most recently.
const MAX_SANDBOXES: usize = 3;
/// The heap a sandbox runtime needs to start and run a small template. quickjs-oxide fixes a runtime's heap cap when
/// it starts and has no call that lowers it for one render, so a limit below this renders nothing on oxide (an
/// `out of memory` error before the template runs), where QuickJS lowers its limit for the render and fails at the
/// first allocation past it.
const LEAST_HEAP: usize = 1024 * 1024;

/// The cap of the sandbox a render with a memory limit of `memory` runs in: the limit rounded down to a power of two,
/// at most the renderer's own, so a render never gets more than its limit and the thread holds few sandboxes (1, 2,
/// 4, ... MiB). `None` below [`LEAST_HEAP`].
fn bucket(memory: usize) -> Option<usize> {
    let limit = memory.min(template::LOCKED_BUDGET.1);
    if limit < LEAST_HEAP {
        return None;
    }
    if limit == template::LOCKED_BUDGET.1 {
        return Some(limit);
    }
    Some(1usize << (usize::BITS - 1 - limit.leading_zeros()))
}

thread_local! {
    static SANDBOXES: RefCell<Vec<Rc<Sandbox>>> = const { RefCell::new(Vec::new()) };
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// What a sandbox runtime asks of Moqui: the clock, the local time zone, nothing printed, and the render's deadline
/// as the interrupt.
struct SandboxHost;

impl Host for SandboxHost {
    fn write(&mut self, _: Stream, _: &[u8]) {}

    fn now_ms(&mut self) -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
    }

    fn local_tz_offset_minutes(&mut self, utc_ms: f64) -> i32 {
        super::host::local_offset_minutes(utc_ms)
    }

    fn interrupt(&mut self) -> bool {
        DEADLINE.with(Cell::get).is_some_and(|at| Instant::now() >= at)
    }
}

struct Sandbox {
    cap: usize,
    functions: RefCell<BoundedMap<String, CallableRef>>,
    fixed: Cell<(usize, usize)>,
    context: Context,
    runtime: Runtime,
}

impl Sandbox {
    fn start(cap: usize) -> Result<Self, RuntimeError> {
        let options = RuntimeOptions {
            heap: HeapLimits { max_bytes: cap },
            ..RuntimeOptions::default()
        };
        let runtime = Runtime::with_host(SandboxHost, options);
        let context = runtime.context();
        let Value::Object(error) = context.eval("Error")? else {
            return Err(RuntimeError::Invariant("the Error constructor"));
        };
        let error = context
            .as_callable(&error)?
            .ok_or(RuntimeError::Invariant("the Error constructor"))?;
        let global = context.global_object()?;
        let functions: [(&str, u32, SandboxFunction); 3] =
            [("format", 2, format), ("localize", 1, localize), ("t", 1, text)];
        for (name, length, body) in functions {
            let error = error.clone();
            let function = context.new_function(
                FunctionOptions::new(name).length(length),
                move |scope: &mut Scope<'_>| -> JsResult<Value> { body(scope, &error) },
            )?;
            context.define_own_property(
                &global,
                &super::convert::key(name).map_err(|_| RuntimeError::Invariant("a key"))?,
                &OrdinaryPropertyDescriptor::data(Value::from(function), true, false, true),
            )?;
        }
        context.eval(template::SANDBOX_PRELUDE)?;
        // As on QuickJS: every render on the thread shares the runtime, so nothing a template does may last.
        context.eval(template::SANDBOX_LOCKDOWN)?;
        Ok(Self {
            cap,
            functions: RefCell::new(BoundedMap::new(MAX_FUNCTIONS)),
            fixed: Cell::new((0, 0)),
            context,
            runtime,
        })
    }

    /// The compiled template function for `compiled`, kept or compiled now.
    fn function(&self, compiled: &str) -> JsResult<CallableRef> {
        if let Some(function) = self.functions.borrow_mut().get(compiled) {
            return Ok(function);
        }
        let (units, bytes) = self.fixed.get();
        let fixed = units < MAX_STATIC && bytes + compiled.len() <= MAX_STATIC_BYTES;
        let options = CompileOptions::new("template");
        let options = if fixed { options } else { options.transient() };
        let context = self.context.clone();
        let script = context.compile_with_options(compiled, &options)?;
        let Value::Object(object) = context.execute(&script)? else {
            return Err(RuntimeError::Invariant("a template function"));
        };
        let function = context
            .as_callable(&object)?
            .ok_or(RuntimeError::Invariant("a template function"))?;
        if fixed {
            self.fixed.set((units + 1, bytes + compiled.len()));
        }
        self.functions
            .borrow_mut()
            .insert(compiled.to_string(), function.clone());
        Ok(function)
    }
}

/// `format(value, pattern)`: the snapshot's text, number or flag formatted as the QuickJS sandbox's `format`.
fn format(scope: &mut Scope<'_>, error: &CallableRef) -> JsResult<Value> {
    let value = match scope.arg(0) {
        Value::String(text) => moqui_lib::object::Value::String(text.to_utf8_lossy()),
        Value::Int(number) => moqui_lib::object::Value::I64(i64::from(number)),
        Value::Float(number) => moqui_lib::object::Value::F64(number),
        Value::Bool(flag) => moqui_lib::object::Value::Bool(flag),
        _ => moqui_lib::object::Value::Null,
    };
    // as rquickjs's `Opt<String>`: no pattern when it is not passed, else it must be a string
    let pattern = if scope.argc() < 2 {
        None
    } else {
        match scope.arg(1) {
            Value::String(text) => Some(text.to_utf8_lossy()),
            other => {
                let kind = match other {
                    Value::Undefined => "undefined",
                    Value::Null => "null",
                    Value::Bool(_) => "bool",
                    Value::Int(_) => "int",
                    Value::Float(_) => "float",
                    Value::Object(_) => "object",
                    _ => "value",
                };
                let message = format!("Error converting from js '{kind}' into type 'string'");
                return Err(throw_plain(scope, error, &message));
            }
        }
    };
    match template::format_snapshot_value(value, pattern.as_deref()) {
        Ok(text) => js_text(&text),
        Err(err) => Err(throw_plain(scope, error, err.message())),
    }
}

/// `localize(text)`, as the QuickJS sandbox's.
fn localize(scope: &mut Scope<'_>, error: &CallableRef) -> JsResult<Value> {
    let Value::String(text) = scope.arg(0) else {
        return Err(scope.throw_error(NativeErrorKind::Type, "localize takes a string"));
    };
    match crate::l10n::localize(&text.to_utf8_lossy()) {
        Ok(text) => js_text(&text),
        Err(err) => Err(throw_plain(scope, error, err.message())),
    }
}

/// `t(key)`: the render's text for `key`, as the QuickJS sandbox's.
fn text(scope: &mut Scope<'_>, _: &CallableRef) -> JsResult<Value> {
    let key = match scope.arg(0) {
        Value::String(key) => Some(key.to_utf8_lossy()),
        _ => None,
    };
    js_text(&template::snapshot_text(key))
}

fn js_text(text: &str) -> JsResult<Value> {
    JsString::try_from_utf8(text)
        .map(Value::String)
        .map_err(|_| RuntimeError::OutOfMemory)
}

/// The thread's sandbox of heap cap `cap`, started if need be.
fn sandbox(cap: usize) -> Result<Rc<Sandbox>, Error> {
    SANDBOXES.with(|cell| {
        let mut sandboxes = cell.borrow_mut();
        if let Some(found) = sandboxes.iter().find(|sandbox| sandbox.cap == cap) {
            return Ok(Rc::clone(found));
        }
        let started = Rc::new(Sandbox::start(cap).map_err(|err| match err {
            RuntimeError::OutOfMemory => Error::script("Error: out of memory"),
            other => Error::new(format!("the quickjs-oxide template sandbox did not start: {other}")),
        })?);
        if sandboxes.len() >= MAX_SANDBOXES {
            // the oldest lower cap goes; the default cap's sandbox stays
            if let Some(at) = sandboxes
                .iter()
                .position(|sandbox| sandbox.cap != template::LOCKED_BUDGET.1)
            {
                sandboxes.remove(at);
            }
        }
        sandboxes.push(Rc::clone(&started));
        Ok(started)
    })
}

/// Drop the thread's sandbox of `cap`, for one a host function's panic poisoned.
fn forget(cap: usize) {
    SANDBOXES.with(|cell| cell.borrow_mut().retain(|sandbox| sandbox.cap != cap));
}

/// The deadline of one locked render, put back as it was however the render ends.
struct DeadlineSet(Option<Instant>);

impl DeadlineSet {
    fn new(deadline: Instant) -> Self {
        Self(DEADLINE.with(|cell| cell.replace(Some(deadline))))
    }
}

impl Drop for DeadlineSet {
    fn drop(&mut self) {
        DEADLINE.with(|cell| cell.set(self.0));
    }
}

/// Render the compiled snapshot form `compiled` with `json` as its snapshot in the thread's sandbox of at most `memory`
/// heap bytes, stopped at `deadline`: `template::render_locked_until` on quickjs-oxide.
///
/// # Errors
///
/// The template's own error, `interrupted` past the deadline, or `out of memory` past the cap.
pub(crate) fn render_locked(compiled: &str, json: &str, deadline: Instant, memory: usize) -> Result<String, Error> {
    let Some(cap) = bucket(memory) else {
        return Err(Error::script("Error: out of memory"));
    };
    let sandbox = sandbox(cap)?;
    let _deadline = DeadlineSet::new(deadline);
    let mut context = sandbox.context.clone();
    let ran = sandbox.function(compiled).and_then(|function| {
        let data = JsString::try_from_utf8(json).map_err(|_| RuntimeError::OutOfMemory)?;
        context.call(&function, Value::Undefined, &[Value::String(data)])
    });
    match ran {
        Ok(Value::String(text)) => Ok(text.to_utf8_lossy()),
        Ok(other) => Ok(context
            .to_js_string(&other)
            .map_err(|err| runtime_error(&mut context, err))?
            .to_utf8_lossy()),
        Err(err) => {
            match &err {
                RuntimeError::Poisoned => forget(sandbox.cap),
                RuntimeError::OutOfMemory => {
                    sandbox.functions.borrow_mut().clear();
                    let _ = sandbox.runtime.run_gc();
                }
                _ => {}
            }
            Err(runtime_error(&mut context, err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lowered limit runs in a sandbox of at most that limit, and nearby limits share one.
    #[test]
    fn memory_limits_share_a_few_sandboxes_and_never_get_more_than_asked() {
        let mib = 1024 * 1024;
        assert_eq!(bucket(0), None);
        assert_eq!(bucket(mib - 1), None);
        assert_eq!(bucket(mib), Some(mib));
        assert_eq!(bucket(2_000_000), Some(mib));
        assert_eq!(bucket(3 * mib), Some(2 * mib));
        assert_eq!(bucket(template::LOCKED_BUDGET.1 - 1), Some(16 * mib));
        assert_eq!(bucket(usize::MAX), Some(template::LOCKED_BUDGET.1));
        let deadline = Instant::now() + template::LOCKED_BUDGET.0;
        let limits = [
            template::LOCKED_BUDGET.1,
            2_000_000,
            1_500_000,
            3_000_000,
            5_000_000,
            9_000_000,
            2_500_000,
        ];
        for memory in limits {
            let out = render_locked(
                "(function(){ return function(){ return 'x'; }; })()",
                "{}",
                deadline,
                memory,
            );
            assert_eq!(out.unwrap(), "x");
            let kept = SANDBOXES.with(|cell| cell.borrow().len());
            assert!(kept <= MAX_SANDBOXES, "{kept}");
        }
        let default_kept = SANDBOXES.with(|cell| {
            cell.borrow()
                .iter()
                .any(|sandbox| sandbox.cap == template::LOCKED_BUDGET.1)
        });
        assert!(default_kept);
    }
}
