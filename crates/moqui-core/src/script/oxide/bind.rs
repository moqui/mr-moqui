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

//! The quickjs-oxide binder of the host table (`script/host`), the counterpart of `script/runtime/bind.rs`
//! (plans/archive/Script-Engine-Oxide.md section 5): each [`HostClass`] is one prototype of host functions, made on first use and
//! kept for the runtime, and each object of it a host object holding an [`OxideHost`].
//!
//! The engine calls [`install_ec`] once, after it made `ec`:
//!
//! ```ignore
//! bind::install_ec(&mut context, &ec, Rc::new(MyHooks))?;
//! ```
//!
//! [`OxideHooks`] is what the binder needs of the engine: value conversion (host values and oxide values, as
//! `oxide::convert` makes them, `oxide::engine::EngineHooks`) and the flush of the engine's tracked values. The tests
//! have a minimal one for a bare runtime.
//!
//! A call behaves as on QuickJS: too few arguments throw `TypeError: Error calling function with N argument(s) while M
//! where expected`; a wrong argument type throws `TypeError: Error converting from js 'int' into type 'string'`; a
//! [`HostError`] throws the same `Error` with the same `message`, `stack` detail, `moquiErrorKind` and
//! `moquiRetryAfter`; a panic is caught and thrown as an `Error` of its message, so it does not poison the runtime.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use moqui_lib::object::{HostObject, Value};
use quickjs_oxide::engine::api::core::{
    AccessorValue, CallableRef, DescriptorField, FunctionOptions, Js, JsResult, JsString, NativeErrorKind, ObjectRef,
    OrdinaryPropertyDescriptor, PropertyKey, RuntimeError, Scope, Value as JsValue,
};

use crate::script::host::{
    self, Arg, ArgKind, Args, Bound, Call, EngineOps, FlushBefore, HostClass, HostError, Method, Path, Ret,
};

/// What the binder needs of the oxide engine.
pub(crate) trait OxideHooks {
    /// A script value as a host value; a value it cannot convert throws.
    fn to_host(&self, scope: &mut Scope<'_>, value: &JsValue) -> JsResult<Value>;
    /// A host value as a script value.
    fn to_js(&self, scope: &mut Scope<'_>, value: Value) -> JsResult<JsValue>;
    /// Write the engine's tracked values back to the context stack before a method that reads it. None are tracked in
    /// Phase 1.
    fn flush(&self, _scope: &mut Scope<'_>, _mode: FlushBefore) -> JsResult<()> {
        Ok(())
    }
    /// The whole context as a script sees it, the tracked values written back first.
    fn whole_context(&self, scope: &mut Scope<'_>) -> JsResult<indexmap::IndexMap<String, Value>> {
        self.flush(scope, FlushBefore::Keep)?;
        Ok(crate::script::context_combined())
    }
}

/// The state of a host object on this engine: the table's state and its class. The state of a class that holds a
/// resource ([`host::released_at_run_end`]) is let go when the outermost script on the thread ends
/// ([`release_run_objects`]), so it does not wait for a collection; a method of the object then throws.
pub(crate) struct OxideHost {
    obj: RefCell<Option<HostObject>>,
    class: &'static HostClass,
}

impl OxideHost {
    /// The table's state, or none once let go.
    pub(crate) fn obj(&self) -> Option<HostObject> {
        self.obj.borrow().clone()
    }

    /// The table's state, or the error that its object was let go when its script ended.
    pub(crate) fn held(&self, js: &mut impl Js) -> JsResult<HostObject> {
        self.obj().ok_or_else(|| {
            let name = self.class.name.strip_prefix("host:").unwrap_or(self.class.name);
            js.throw_error(
                NativeErrorKind::Type,
                &format!("this {name} was let go when the script that made it ended"),
            )
        })
    }
}

thread_local! {
    /// The objects of this thread's runs whose state goes when the outermost script ends.
    static RELEASED_AT_END: RefCell<Vec<std::rc::Weak<OxideHost>>> = const { RefCell::new(Vec::new()) };
    /// The length of [`RELEASED_AT_END`] past which the entries of objects already collected are dropped, doubled each
    /// time, so a long run that opens a stream per record does not keep an entry for each.
    static PRUNE_AT: Cell<usize> = const { Cell::new(MIN_PRUNE_AT) };
}

const MIN_PRUNE_AT: usize = 64;

/// Let go the state of every object of a resource class ([`host::released_at_run_end`]) made since the outermost script
/// on the thread started, the objects themselves still reachable or not: called when it ends
/// ([`crate::script::run::enter_depth`]). An upload's view of its spool file, a stream's buffer, an SFTP client (closed
/// with an error in the log, as Java's `finalize` does) then go now, as QuickJS's count lets them go, not at a
/// collection that may be far off.
pub(crate) fn release_run_objects() {
    let held = RELEASED_AT_END.with(|held| held.try_borrow_mut().map(|mut held| std::mem::take(&mut *held)));
    let Ok(held) = held else {
        return;
    };
    PRUNE_AT.with(|at| at.set(MIN_PRUNE_AT));
    for object in held {
        if let Some(object) = object.upgrade() {
            // taken out before it drops, so a drop that reaches the binder finds the cell free
            let state = object.obj.borrow_mut().take();
            drop(state);
        }
    }
}

/// The number of objects whose state goes when the outermost script ends, for the tests.
#[cfg(test)]
pub(crate) fn release_pending_for_tests() -> usize {
    RELEASED_AT_END.with(|held| held.borrow().len())
}

/// The binder of one runtime: the hooks and the prototypes made so far, by class name.
struct Binder {
    hooks: Rc<dyn OxideHooks>,
    prototypes: RefCell<HashMap<&'static str, ObjectRef>>,
}

/// The global property that marks a runtime the host table is installed in.
const INSTALLED: &str = "__moquiHostTable";

/// Install every path of the host table under `ec` (`ec.l10n` as `ec`'s `l10n`), each an object of its class.
pub(crate) fn install_ec(js: &mut impl Js, ec: &ObjectRef, hooks: Rc<dyn OxideHooks>) -> JsResult<()> {
    install_paths(js, ec, host::all_paths(), hooks)
}

/// [`install_ec`] of the given paths.
pub(crate) fn install_paths(
    js: &mut impl Js,
    ec: &ObjectRef,
    paths: &[&'static Path],
    hooks: Rc<dyn OxideHooks>,
) -> JsResult<()> {
    // One binder per runtime: a second install would make a second set of prototypes and objects.
    let global = js.global_object()?;
    let marker = key(INSTALLED)?;
    if js.get_own_property(&global, &marker)?.is_some() {
        return Err(js.throw_error(
            NativeErrorKind::Type,
            "the host table is already installed in this runtime",
        ));
    }
    let binder = Rc::new(Binder {
        hooks,
        prototypes: RefCell::new(HashMap::new()),
    });
    // the marker holds the binder, so a conversion can make an object of a class the table returns (an entity value
    // or list's prototype, a find kept in the context) with this runtime's prototypes
    let handle = js.new_host_object(Installed(Rc::clone(&binder)), None)?;
    js.define_own_property(
        &global,
        &marker,
        &OrdinaryPropertyDescriptor::data(JsValue::Object(handle), false, false, false),
    )?;
    for entry in paths.iter() {
        if let Bound::Shim(_) = entry.bound {
            continue;
        }
        let Some(rest) = entry.path.strip_prefix("ec.") else {
            tracing::error!(
                "The host table path {} is not under ec; it is not installed",
                entry.path
            );
            continue;
        };
        let mut target = ec.clone();
        let mut names: Vec<&str> = rest.split('.').collect();
        let Some(last) = names.pop() else {
            continue;
        };
        for name in names {
            let key = key(name)?;
            target = match js.get_property(&target, &key)? {
                JsValue::Object(found) => found,
                _ => {
                    let made = js.new_object()?;
                    js.set_property(&target, &key, JsValue::Object(made.clone()))?;
                    made
                }
            };
        }
        let value = match entry.bound {
            Bound::Object(class) => instance(js, &binder, HostObject::new(()), class)?,
            Bound::Function(method) => function(js, &binder, method)?.into_object(),
            Bound::Shim(_) => continue,
        };
        js.set_property(&target, &key(last)?, JsValue::Object(value))?;
    }
    Ok(())
}

/// Run every shim of the host table ([`host::Shim`]) once `ec` and the helper objects are made: each is called with the
/// global object, its hidden functions by name and `helpers` (an object of `CollectionUtilities`, `StringUtilities` and
/// `WebUtilities`). The shims are fixed framework scripts, so one that fails fails the runtime's start; every shim is
/// tried, each failure logged naming its shim, and the error returned names them all. Its prototypes are kept apart
/// from [`install_ec`]'s.
pub(crate) fn install_shims(js: &mut impl Js, helpers: &ObjectRef, hooks: Rc<dyn OxideHooks>) -> JsResult<()> {
    let binder = Rc::new(Binder {
        hooks,
        prototypes: RefCell::new(HashMap::new()),
    });
    let global = js.global_object()?;
    let mut failed = Vec::new();
    for entry in host::all_paths() {
        let Bound::Shim(shim) = entry.bound else {
            continue;
        };
        if let Err(err) = install_shim(js, &binder, &global, helpers, shim) {
            let detail = match err {
                RuntimeError::Exception(thrown) => match thrown_text(js, &thrown) {
                    Ok(Some(text)) => text,
                    _ => String::from("(exception)"),
                },
                other => other.to_string(),
            };
            tracing::error!("The host shim {} was not installed: {detail}", entry.path);
            failed.push(format!("{}: {detail}", entry.path));
        }
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(RuntimeError::Internal(format!(
        "host shims not installed: {}",
        failed.join("; ")
    )))
}

/// One shim outside the table's paths ([`host::globals::SHIM`]), with prototypes of its own.
pub(crate) fn install_one_shim(
    js: &mut impl Js,
    helpers: &ObjectRef,
    shim: &'static host::Shim,
    hooks: Rc<dyn OxideHooks>,
) -> JsResult<()> {
    let binder = Rc::new(Binder {
        hooks,
        prototypes: RefCell::new(HashMap::new()),
    });
    let global = js.global_object()?;
    install_shim(js, &binder, &global, helpers, shim)
}

fn install_shim(
    js: &mut impl Js,
    binder: &Rc<Binder>,
    global: &ObjectRef,
    helpers: &ObjectRef,
    shim: &'static host::Shim,
) -> JsResult<()> {
    let hidden = js.new_object()?;
    for method in shim.hidden.iter().flat_map(|group| group.iter()) {
        let made = function(js, binder, method)?;
        js.set_property(&hidden, &key(method.name)?, JsValue::Object(made.into_object()))?;
    }
    let install = super::compiled::eval_start(js, shim.js)?;
    let install = match &install {
        JsValue::Object(object) => js.as_callable(object)?,
        _ => None,
    };
    let Some(install) = install else {
        return Err(js.throw_error(NativeErrorKind::Type, "a host shim is not a function"));
    };
    js.call(
        &install,
        JsValue::Undefined,
        &[
            JsValue::Object(global.clone()),
            JsValue::Object(hidden),
            JsValue::Object(helpers.clone()),
        ],
    )?;
    Ok(())
}

/// The fallback reason of a host call made after the run fell back (the first reason is the one reported).
pub(super) const FELL_BACK: &str = "a run that fell back";

/// The binder [`install_ec`] installed, held by the runtime's marker.
struct Installed(Rc<Binder>);

/// The binder of the runtime `js` is in; an error before [`install_ec`] ran.
fn installed(js: &mut impl Js) -> JsResult<Rc<Binder>> {
    let global = js.global_object()?;
    if let JsValue::Object(marker) = js.get_property(&global, &super::convert::static_key(INSTALLED)?)? {
        if let Some(held) = js.host_data::<Installed>(&marker)? {
            return Ok(Rc::clone(&held.0));
        }
    }
    Err(js.throw_error(NativeErrorKind::Type, "the host table is not installed in this runtime"))
}

/// The prototype of `class` in the runtime `js` is in, made on first use: for an exotic object whose methods are the
/// table's (an entity value or list).
pub(crate) fn class_prototype(js: &mut impl Js, class: &'static HostClass) -> JsResult<ObjectRef> {
    let binder = installed(js)?;
    prototype(js, &binder, class)
}

/// A new object of `class` over `obj` in the runtime `js` is in (a find kept in the context, read back).
pub(crate) fn class_instance(js: &mut impl Js, obj: HostObject, class: &'static HostClass) -> JsResult<ObjectRef> {
    let binder = installed(js)?;
    instance(js, &binder, obj, class)
}

fn key(name: &str) -> JsResult<PropertyKey> {
    Ok(PropertyKey::from_js_string(JsString::try_from_utf8(name)?))
}

fn text(value: &str) -> JsResult<JsValue> {
    Ok(JsValue::String(JsString::try_from_utf8(value)?))
}

/// A new object of `class` over `obj`.
fn instance(js: &mut impl Js, binder: &Rc<Binder>, obj: HostObject, class: &'static HostClass) -> JsResult<ObjectRef> {
    let proto = prototype(js, binder, class)?;
    let made = js.new_host_object(
        OxideHost {
            obj: RefCell::new(Some(obj)),
            class,
        },
        Some(&proto),
    )?;
    if host::released_at_run_end(class) {
        if let Some(held) = js.host_data::<OxideHost>(&made)? {
            RELEASED_AT_END.with(|list| {
                let mut list = list.borrow_mut();
                if list.len() >= PRUNE_AT.with(Cell::get) {
                    list.retain(|object| object.strong_count() > 0);
                    PRUNE_AT.with(|at| at.set((list.len() * 2).max(MIN_PRUNE_AT)));
                }
                list.push(Rc::downgrade(&held));
            });
        }
    }
    Ok(made)
}

fn prototype(js: &mut impl Js, binder: &Rc<Binder>, class: &'static HostClass) -> JsResult<ObjectRef> {
    if let Some(found) = binder.prototypes.borrow().get(class.name) {
        return Ok(found.clone());
    }
    let proto = js.new_object()?;
    for method in class.methods {
        let made = function(js, binder, method)?;
        // not enumerable, as a class's methods are, so `for...in` over an entity value sees only its fields
        js.define_own_property(
            &proto,
            &key(method.name)?,
            &OrdinaryPropertyDescriptor::data(JsValue::Object(made.into_object()), true, false, true),
        )?;
    }
    for getter in class.getters {
        let made = function(js, binder, getter)?;
        let descriptor = OrdinaryPropertyDescriptor {
            get: DescriptorField::Present(AccessorValue::Callable(made)),
            set: DescriptorField::Present(AccessorValue::Undefined),
            enumerable: DescriptorField::Present(false),
            configurable: DescriptorField::Present(true),
            ..OrdinaryPropertyDescriptor::default()
        };
        js.define_own_property(&proto, &key(getter.name)?, &descriptor)?;
    }
    if let Some(shim) = class.shim_js {
        let hidden = js.new_object()?;
        for method in class.hidden {
            let made = function(js, binder, method)?;
            js.set_property(&hidden, &key(method.name)?, JsValue::Object(made.into_object()))?;
        }
        let install = super::compiled::eval_start(js, shim)?;
        let install = match &install {
            JsValue::Object(object) => js.as_callable(object)?,
            _ => None,
        };
        let Some(install) = install else {
            return Err(js.throw_error(NativeErrorKind::Type, "a host class shim is not a function"));
        };
        js.call(
            &install,
            JsValue::Undefined,
            &[JsValue::Object(proto.clone()), JsValue::Object(hidden)],
        )?;
    }
    binder.prototypes.borrow_mut().insert(class.name, proto.clone());
    Ok(proto)
}

/// The host function of a method, named and with the `length` of its required arguments.
fn function(js: &mut impl Js, binder: &Rc<Binder>, method: &'static Method) -> JsResult<CallableRef> {
    let binder = Rc::clone(binder);
    let length = u32::try_from(method.required()).unwrap_or(u32::MAX);
    js.new_function(
        FunctionOptions::new(method.name).length(length),
        move |scope: &mut Scope<'_>| trampoline(&binder, method, scope),
    )
}

/// One call: the arguments by kind, the flush, the body, with a panic caught and a failure thrown as on QuickJS.
fn trampoline(binder: &Rc<Binder>, method: &'static Method, scope: &mut Scope<'_>) -> JsResult<JsValue> {
    // a run that fell back makes no more side effects: a `catch` or `finally` of the throw does not reach the host
    if super::engine::fallback_pending() {
        return Err(super::engine::host_error(scope, super::convert::Conv::Unsupported(FELL_BACK)));
    }
    let required = method.required();
    let given = scope.argc();
    if given < required {
        let message = format!("Error calling function with {given} argument(s) while {required} where expected");
        return Err(scope.throw_error(NativeErrorKind::Type, &message));
    }
    // a panic is not in flight when a call starts, so a mark an earlier one left at the top is stale
    UNWOUND.with(|unwound| unwound.set(false));
    match caught_panic(|| call_method(binder, method, scope)) {
        Ok(result) => result,
        Err(message) => Err(plain_error(scope, &message)),
    }
}

/// A host function of the engine's own outside the table (`oxide::resource`, `oxide::web`), through the table's
/// trampoline: a panic in its own code is thrown as an `Error`, one that unwound through the engine (a nested run in
/// a lend) goes on and poisons the runtime, as for a table method; a [`Conv`](super::convert::Conv) failure throws as
/// [`super::engine::host_error`] makes it.
pub(super) fn host_body<T>(
    scope: &mut Scope<'_>,
    body: impl FnOnce(&mut Scope<'_>) -> Result<T, super::convert::Conv>,
) -> JsResult<T> {
    UNWOUND.with(|unwound| unwound.set(false));
    match caught_panic(|| body(&mut *scope).map_err(|conv| super::engine::host_error(&mut *scope, conv))) {
        Ok(result) => result,
        Err(message) => Err(plain_error(scope, &message)),
    }
}

/// Before a lend outside the table's methods (a tool's call, an entity value's relationship or computed field, the
/// engine's own host functions): the running script's tracked values written back and released, while its own frame is
/// the stack's top, as `engine::release_for_quickjs` does for a QuickJS script nested there. A script nested in the lend
/// then reads what the outer one changed, whichever engine it runs on, and a value it assigns is not written over at
/// the outer run's end.
pub(super) fn release_before_lend<J: Js>(js: &mut J) -> JsResult<()> {
    super::tracked::flush(js, super::tracked::Flush::Release).map_err(|conv| super::engine::host_error(js, conv))
}

/// Lend the engine to `body`, as a non-leaf table method's lend does ([`OxideOps::lend`]): a panic in the body's own
/// code is thrown as an `Error`, one through a nested run goes on. The running script's tracked values are released
/// first ([`release_before_lend`]).
pub(super) fn lend<R>(scope: &mut Scope<'_>, body: impl FnOnce() -> JsResult<R>) -> JsResult<R> {
    release_before_lend(scope)?;
    let lent = {
        let _engine = EngineCall;
        scope.lend(|| caught_panic(body))
    };
    match lent {
        Ok(result) => result,
        Err(message) => Err(plain_error(scope, &message)),
    }
}

/// A mark over an engine operation a body makes itself (an `execute` in a lend): a panic that unwinds through it came
/// through the engine, so [`caught_panic`] passes it on rather than making it an error.
pub(super) fn engine_call() -> impl Drop {
    EngineCall
}

/// `body`, with a panic in its own code caught as its message, as `host_guard` catches one on QuickJS. A panic that
/// unwound through the engine (a nested call, a conversion, a nested script in a lent body) left it inconsistent and
/// poisoned: it goes on to the embedder's call, as quickjs-oxide passes a host function's panic, rather than becoming
/// an error the script could catch and go on from. It goes on marked, so each trampoline and lend it passes on the way
/// up passes it on too.
pub(super) fn caught_panic<R>(body: impl FnOnce() -> R) -> Result<R, String> {
    match std::panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => Ok(result),
        Err(payload) if UNWOUND.with(|unwound| unwound.replace(false)) => {
            let _engine = EngineCall;
            std::panic::resume_unwind(payload)
        }
        Err(payload) => Err(host::panic_message(payload.as_ref())),
    }
}

thread_local! {
    /// Set when a panic unwinds out of an engine operation a trampoline made ([`EngineCall`]).
    static UNWOUND: Cell<bool> = const { Cell::new(false) };
}

/// Marks an engine operation a trampoline makes outside the body, and a lend: a panic that unwinds through it came
/// through the engine, so the runtime cannot go on. A panic in the body's own code does not pass one; one in a lent
/// body's own code is caught inside the lend ([`caught_panic`]), so it does not pass the lend's either.
struct EngineCall;

impl Drop for EngineCall {
    fn drop(&mut self) {
        if std::thread::panicking() {
            UNWOUND.with(|unwound| unwound.set(true));
        }
    }
}

/// What a body's call ended with: its result, a failure to throw, or an engine error passed on.
enum Outcome {
    Done(Ret),
    Failed(HostError, Option<RuntimeError>),
}

fn call_method(binder: &Rc<Binder>, method: &'static Method, scope: &mut Scope<'_>) -> JsResult<JsValue> {
    let _profile = crate::screen::profile::span("host", method.path);
    let mut callbacks = Vec::new();
    let engine = EngineCall;
    let args_span = crate::screen::profile::span("phase", "host.args");
    let args = convert_args(binder, scope, method.args, &mut callbacks)?;
    drop(args_span);
    // the receiver's datum read from the call, with no handle; the receiver itself lifted only when returned
    let held = scope.this_host_data::<OxideHost>()?;
    let state = match held {
        Some(held) => Some(held.held(scope)?),
        // an entity value or list, whose prototype is the table's; else no state (an `ec` object's method)
        None => entities::receiver_state(scope)?,
    };
    drop(engine);
    let outcome = {
        let mut ops = OxideOps {
            scope: &mut *scope,
            binder,
            callbacks,
            thrown: None,
            // the innermost runtime entered is the one whose script called this function
            engine: super::engine::current(),
        };
        let flushed = match method.flush {
            FlushBefore::None => Ok(()),
            mode => {
                let _flush = crate::screen::profile::span("phase", "flush.before");
                ops.flush(mode)
            }
        };
        let this_state = state.as_ref();
        let result = flushed.and_then(|()| {
            if !method.leaf {
                // The body may run a nested script: the engine is lent to it, so the script runs on this runtime.
                // The scope is the engine's until the body returns, so the body's engine operations are the ones
                // made before the lend: the flush above, and the whole context an argument passes.
                let passes_context =
                    (0..method.args.len()).any(|index| matches!(args.get(index), Some(Arg::WholeContext)));
                let whole = if passes_context { Some(ops.whole_context()?) } else { None };
                let mut lent = LentOps { whole };
                // A panic in the body's own code is caught inside the lend and thrown as an error. One that unwinds
                // out of the lend came through the engine's frames (a nested script, whose trampoline resumed it
                // marked): the runtime is poisoned, so it goes on to the embedder ([`EngineCall`]).
                let _engine = EngineCall;
                return ops.scope.lend(|| {
                    let mut call = Call {
                        this: this_state,
                        args,
                        ops: &mut lent,
                    };
                    caught_panic(|| (method.f)(&mut call)).unwrap_or_else(|message| Err(HostError::Message(message)))
                });
            }
            let mut call = Call {
                this: this_state,
                args,
                ops: &mut ops,
            };
            (method.f)(&mut call)
        });
        match result {
            Ok(ret) => Outcome::Done(ret),
            Err(err) => Outcome::Failed(err, ops.thrown.take()),
        }
    };
    let _engine = EngineCall;
    let _ret = crate::screen::profile::span("phase", "host.ret");
    match outcome {
        Outcome::Done(Ret::Value(value)) => binder.hooks.to_js(scope, value),
        Outcome::Done(Ret::This) => Ok(scope.this()),
        Outcome::Done(Ret::Undefined) => Ok(JsValue::Undefined),
        Outcome::Done(Ret::Object(obj, class)) => Ok(JsValue::Object(instance(scope, binder, obj, class)?)),
        Outcome::Failed(err, thrown) => Err(throw(scope, err, thrown)),
    }
}

/// The type name QuickJS gives `value` in a conversion error.
fn type_name(scope: &mut Scope<'_>, value: &JsValue) -> JsResult<&'static str> {
    Ok(match value {
        JsValue::Undefined => "undefined",
        JsValue::Null => "null",
        JsValue::Bool(_) => "bool",
        JsValue::Int(_) => "int",
        JsValue::Float(_) => "float",
        JsValue::BigInt(_) => "big_int",
        JsValue::String(_) => "string",
        JsValue::Symbol(_) => "symbol",
        JsValue::Object(object) => {
            if scope.is_array_object(object)? {
                "array"
            } else if scope.is_constructor(object)? {
                "constructor"
            } else if scope.as_callable(object)?.is_some() {
                "function"
            } else if scope.is_error_object(object)? {
                "exception"
            } else {
                "object"
            }
        }
    })
}

/// A `TypeError` for an argument of the wrong type, worded as QuickJS's.
fn wrong_type(scope: &mut Scope<'_>, value: &JsValue, kind: ArgKind) -> RuntimeError {
    let from = match type_name(scope, value) {
        Ok(from) => from,
        Err(err) => return err,
    };
    let message = format!("Error converting from js '{from}' into type '{}'", kind.type_name());
    scope.throw_error(NativeErrorKind::Type, &message)
}

fn convert_args(
    binder: &Binder,
    scope: &mut Scope<'_>,
    kinds: &[ArgKind],
    callbacks: &mut Vec<CallableRef>,
) -> JsResult<Args> {
    let given = scope.argc();
    let mut converted = Args::new();
    for (index, kind) in kinds.iter().copied().enumerate() {
        if index >= given {
            converted.push(Arg::Missing);
            continue;
        }
        // text read from the heap with no handle and one copy
        if matches!(kind, ArgKind::Text | ArgKind::OptText | ArgKind::NullableText | ArgKind::OptNullableText) {
            if let Some(text) = scope.arg_text(index) {
                converted.push(Arg::Text(text));
                continue;
            }
        }
        // text for a value read the same way: as a string, it converts to its text and nothing else
        if matches!(kind, ArgKind::Value | ArgKind::OptValue) {
            if let Some(text) = scope.arg_text(index) {
                converted.push(Arg::Value(Value::String(text)));
                continue;
            }
        }
        // an element literal holding its conversion, read with no handle
        if matches!(kind, ArgKind::Node | ArgKind::OptNode) {
            if let Some(node) = node_lits::held_lit_arg(scope, index)? {
                converted.push(Arg::Node(node));
                continue;
            }
        }
        let value = scope.arg(index);
        let void = matches!(value, JsValue::Undefined | JsValue::Null);
        converted.push(match (kind, &value) {
            (ArgKind::Value, _) => Arg::Value(binder.hooks.to_host(scope, &value)?),
            (ArgKind::OptValue, JsValue::Undefined) => Arg::Undefined,
            (ArgKind::OptValue, _) => Arg::Value(binder.hooks.to_host(scope, &value)?),
            (ArgKind::NullableText | ArgKind::OptNullableText, _) if void => Arg::Undefined,
            (
                ArgKind::Text | ArgKind::OptText | ArgKind::NullableText | ArgKind::OptNullableText,
                JsValue::String(text),
            ) => Arg::Text(text.to_utf8_lossy()),
            (ArgKind::Bool | ArgKind::OptBool, JsValue::Bool(flag)) => Arg::Bool(*flag),
            (ArgKind::Int | ArgKind::OptInt, JsValue::Int(number)) => Arg::Int(*number),
            #[allow(clippy::cast_possible_truncation)] // a fraction truncated, as QuickJS's `i32` conversion
            (ArgKind::Int | ArgKind::OptInt, JsValue::Float(number)) => Arg::Int(*number as i32),
            (ArgKind::OptNumber | ArgKind::Number, JsValue::Int(number)) => Arg::Number(f64::from(*number)),
            (ArgKind::OptNumber | ArgKind::Number, JsValue::Float(number)) => Arg::Number(*number),
            (ArgKind::Callback, JsValue::Object(object)) => match scope.as_callable(object)? {
                Some(callable) => {
                    callbacks.push(callable);
                    Arg::Callback(callbacks.len() - 1)
                }
                None => return Err(wrong_type(scope, &value, kind)),
            },
            (ArgKind::Texts | ArgKind::OptTexts, JsValue::Object(object)) if scope.is_array_object(object)? => {
                let array = binder.hooks.to_host(scope, &value)?;
                texts_arg(scope, array)?
            }
            (ArgKind::Context, JsValue::Object(_)) => {
                Arg::Context(crate::script::value_map(&binder.hooks.to_host(scope, &value)?))
            }
            (ArgKind::Context, _) => Arg::Context(None),
            (ArgKind::OptWhole, _) if void => Arg::Undefined,
            (ArgKind::Whole | ArgKind::OptWhole, JsValue::Int(number)) => Arg::Int(*number),
            (ArgKind::Whole | ArgKind::OptWhole, JsValue::Float(_)) => {
                return Err(plain_error(scope, "limit is not an integer"))
            }
            (ArgKind::Whole | ArgKind::OptWhole, _) => return Err(plain_error(scope, "limit is not a number")),
            (ArgKind::Parameters, _) if is_ec_context(scope, &value)? => Arg::WholeContext,
            (ArgKind::Parameters, _) => Arg::Value(binder.hooks.to_host(scope, &value)?),
            (ArgKind::Thrown, _) => thrown_text(scope, &value)?.map_or(Arg::Missing, Arg::Text),
            (ArgKind::Host, JsValue::Object(object)) => match scope.host_data::<OxideHost>(object)? {
                Some(held) => Arg::Host(held.held(scope)?),
                None => return Err(wrong_type(scope, &value, kind)),
            },
            (ArgKind::Node, _) | (ArgKind::OptNode, JsValue::Object(_)) => Arg::Node(node_lits::lit_of(scope, &value)?),
            (ArgKind::OptNode, _) => Arg::Undefined,
            _ => return Err(wrong_type(scope, &value, kind)),
        });
    }
    Ok(converted)
}

/// The strings of an array argument, as QuickJS's `Vec<String>` converts it: an item that is not a string throws.
fn texts_arg(scope: &mut Scope<'_>, array: Value) -> JsResult<Arg> {
    let Value::List(items) = array else {
        return Ok(Arg::Texts(Vec::new()));
    };
    let mut texts = Vec::with_capacity(items.len());
    for item in items {
        let from = match item {
            Value::String(text) => {
                texts.push(text);
                continue;
            }
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::I32(_) | Value::I64(_) => "int",
            Value::F64(_) => "float",
            Value::List(_) => "array",
            _ => "object",
        };
        let message = format!("Error converting from js '{from}' into type 'string'");
        return Err(scope.throw_error(NativeErrorKind::Type, &message));
    }
    Ok(Arg::Texts(texts))
}

/// Whether `value` is the global `ec`'s `context`.
fn is_ec_context(scope: &mut Scope<'_>, value: &JsValue) -> JsResult<bool> {
    let JsValue::Object(_) = value else {
        return Ok(false);
    };
    let global = scope.global_object()?;
    let JsValue::Object(ec) = scope.get_property(&global, &key("ec")?)? else {
        return Ok(false);
    };
    let context = scope.get_property(&ec, &key("context")?)?;
    Ok(context.strict_equal(value))
}

/// The text a log call gives its exception: `name: message` of an error object, its `message` alone, or a string.
fn thrown_text(scope: &mut impl Js, value: &JsValue) -> JsResult<Option<String>> {
    Ok(match value {
        JsValue::Undefined | JsValue::Null => None,
        JsValue::Object(object) => {
            let name = scope.get_property(object, &key("name")?).ok();
            let detail = scope.get_property(object, &key("message")?).ok();
            let as_text = |value: Option<JsValue>| match value {
                Some(JsValue::String(text)) => Some(text.to_utf8_lossy()),
                _ => None,
            };
            Some(match (as_text(name), as_text(detail)) {
                (Some(name), Some(detail)) => format!("{name}: {detail}"),
                (_, Some(detail)) => detail,
                _ => String::from("(exception)"),
            })
        }
        JsValue::String(text) => Some(text.to_utf8_lossy()),
        _ => Some(String::new()),
    })
}

/// The text of a thrown error, as QuickJS's caught error gives it: `name: message` of an error object, else the value as
/// text.
fn error_text(scope: &mut Scope<'_>, err: RuntimeError) -> String {
    match err {
        RuntimeError::Exception(value) => match thrown_text(scope, &value) {
            Ok(Some(text)) => text,
            _ => String::from("(exception)"),
        },
        other => other.to_string(),
    }
}

/// `new Error(message)`, as QuickJS's `Exception::throw_message` makes it, thrown.
fn plain_error(scope: &mut impl Js, message: &str) -> RuntimeError {
    match new_error(scope, message) {
        Ok(error) => RuntimeError::Exception(JsValue::Object(error)),
        Err(err) => err,
    }
}

fn new_error(scope: &mut impl Js, message: &str) -> JsResult<ObjectRef> {
    let global = scope.global_object()?;
    let constructor = match scope.get_property(&global, &key("Error")?)? {
        JsValue::Object(object) => scope.as_callable(&object)?,
        _ => None,
    };
    let Some(constructor) = constructor else {
        return Err(scope.throw_error(NativeErrorKind::Type, "Error is not a constructor"));
    };
    match scope.construct(&constructor, &[text(message)?])? {
        JsValue::Object(error) => Ok(error),
        _ => Err(scope.throw_error(NativeErrorKind::Type, "Error made no object")),
    }
}

/// Throw `err` as the QuickJS binder throws it.
pub(super) fn throw(scope: &mut impl Js, err: HostError, thrown: Option<RuntimeError>) -> RuntimeError {
    if let Some(marked) = err.marked() {
        return match marked_error(scope, &marked) {
            Ok(error) => RuntimeError::Exception(JsValue::Object(error)),
            Err(err) => err,
        };
    }
    match err {
        HostError::Message(message) => plain_error(scope, &message),
        HostError::Type(message) => scope.throw_error(NativeErrorKind::Type, &message),
        HostError::Thrown => thrown.unwrap_or_else(|| plain_error(scope, "a host call failed")),
        HostError::Lib(_) | HostError::Context(_) => plain_error(scope, "a host call failed"),
    }
}

/// An `Error` of the message with the diagnostic after its stack and the kind and retry seconds marked, as runtime.rs's
/// `throw_diagnostic`.
fn marked_error(scope: &mut impl Js, marked: &host::Thrown<'_>) -> JsResult<ObjectRef> {
    let error = new_error(scope, marked.message)?;
    let detail = marked
        .diagnostic
        .strip_prefix(marked.message)
        .unwrap_or(&marked.diagnostic);
    if !detail.is_empty() {
        let stack_key = key("stack")?;
        let stack = match scope.get_property(&error, &stack_key)? {
            JsValue::String(stack) => stack.to_utf8_lossy(),
            _ => String::new(),
        };
        scope.set_property(&error, &stack_key, text(&format!("{stack}{detail}\n"))?)?;
    }
    if let Some(kind) = marked.kind {
        scope.set_property(&error, &key(host::KIND_PROPERTY)?, text(kind)?)?;
    }
    if let Some(seconds) = marked.retry_after {
        #[allow(clippy::cast_precision_loss)] // retry seconds are small
        let seconds = JsValue::number(seconds as f64);
        scope.set_property(&error, &key(host::RETRY_AFTER_PROPERTY)?, seconds)?;
    }
    Ok(error)
}

/// [`EngineOps`] of a body the engine is lent to (a method that is not [`Method::leaf`]): only the whole context the
/// binder read before the lend.
struct LentOps {
    whole: Option<indexmap::IndexMap<String, Value>>,
}

impl LentOps {
    fn refused() -> HostError {
        HostError::message("a host call that may run nested scripts cannot call back into its script (it is not a leaf)")
    }
}

impl EngineOps for LentOps {
    fn evaluate(&mut self, _source: &str, _name: &str) -> Result<Value, HostError> {
        Err(Self::refused())
    }

    fn call_back(&mut self, _index: usize, _args: Vec<Value>) -> Result<Value, HostError> {
        Err(Self::refused())
    }

    fn flush(&mut self, _mode: FlushBefore) -> Result<(), HostError> {
        Err(Self::refused())
    }

    fn whole_context(&mut self) -> Result<indexmap::IndexMap<String, Value>, HostError> {
        self.whole.take().ok_or_else(Self::refused)
    }

    /// The units a render runs from the lent body run on this runtime, in the script that made the call.
    fn screen_runner(&mut self) -> Result<&dyn crate::screen::sri::Runner, HostError> {
        Ok(&super::units::RUNNER)
    }
}

/// [`EngineOps`] of one call on quickjs-oxide.
struct OxideOps<'s, 'a> {
    scope: &'s mut Scope<'a>,
    binder: &'s Binder,
    callbacks: Vec<CallableRef>,
    thrown: Option<RuntimeError>,
    /// The runtime the call is in.
    engine: Option<Rc<super::engine::OxideEngine>>,
}

impl OxideOps<'_, '_> {
    fn failed(&mut self, err: RuntimeError) -> HostError {
        self.thrown = Some(err);
        HostError::Thrown
    }
}

impl EngineOps for OxideOps<'_, '_> {
    fn lend(&mut self, body: &mut dyn FnMut()) -> Result<(), HostError> {
        // a panic in the body's own code is an error; one through the engine goes on ([`caught_panic`])
        let _engine = EngineCall;
        self.scope.lend(|| caught_panic(body)).map_err(HostError::Message)
    }

    fn screen_runner(&mut self) -> Result<&dyn crate::screen::sri::Runner, HostError> {
        Ok(&super::units::RUNNER)
    }

    fn evaluate(&mut self, source: &str, name: &str) -> Result<Value, HostError> {
        let hooks = Rc::clone(&self.binder.hooks);
        let _engine = EngineCall;
        let evaluated = match self.scope.eval_with_filename(source, name) {
            Ok(evaluated) => evaluated,
            Err(err) => return Err(HostError::message(error_text(self.scope, err))),
        };
        hooks.to_host(self.scope, &evaluated).map_err(|err| self.failed(err))
    }

    fn call_back(&mut self, index: usize, args: Vec<Value>) -> Result<Value, HostError> {
        let Some(function) = self.callbacks.get(index).cloned() else {
            return Err(HostError::message("no such callback"));
        };
        let hooks = Rc::clone(&self.binder.hooks);
        let _engine = EngineCall;
        let called = (|| {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(hooks.to_js(self.scope, arg)?);
            }
            let result = self.scope.call(&function, JsValue::Undefined, &values)?;
            hooks.to_host(self.scope, &result)
        })();
        called.map_err(|err| self.failed(err))
    }

    fn run_pending_job(&mut self) -> Result<bool, HostError> {
        let Some(engine) = self.engine.clone() else {
            return Err(HostError::message("no oxide engine"));
        };
        // the job queue is the runtime's: it runs inside a lend, above this call's frames
        let _engine = EngineCall;
        let ran = self.scope.lend(|| {
            debug_assert!(engine.runtime().is_lent(), "a job run on a runtime this call did not lend");
            engine.runtime().run_pending_job()
        });
        match ran {
            Ok(outcome) => Ok(outcome.executed()),
            Err(err) => Err(self.failed(err)),
        }
    }

    fn whole_context(&mut self) -> Result<indexmap::IndexMap<String, Value>, HostError> {
        let hooks = Rc::clone(&self.binder.hooks);
        let _engine = EngineCall;
        hooks.whole_context(self.scope).map_err(|err| self.failed(err))
    }

    fn flush(&mut self, mode: FlushBefore) -> Result<(), HostError> {
        if mode == FlushBefore::None {
            return Ok(());
        }
        let hooks = Rc::clone(&self.binder.hooks);
        let _engine = EngineCall;
        hooks.flush(self.scope, mode).map_err(|err| self.failed(err))
    }
}

/// Entity values and lists as host exotic objects whose methods are the table's.
#[path = "entities.rs"]
pub(crate) mod entities;

/// Element literals ([`ArgKind::Node`]).
#[path = "node_lits.rs"]
mod node_lits;

/// Native tools as host exotic objects.
#[allow(dead_code)] // the engine makes tool objects once its conversion of host values reaches them
#[path = "tool.rs"]
pub(crate) mod tool;

#[cfg(test)]
#[path = "bind_tests.rs"]
mod tests;
