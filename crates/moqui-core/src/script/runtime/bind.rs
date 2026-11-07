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

//! The QuickJS binder of the host table (`script/host`): every [`HostClass`] is one prototype of functions over the
//! generic class [`HostNeutral`], made once per engine ([`engine_object`]). A call converts its arguments by their
//! [`ArgKind`]s with QuickJS's own conversions (so a wrong type throws the `TypeError` a typed function threw), runs
//! the method's flush, and throws a [`HostError`] as the facades did. See plans/Script-Engine-Oxide.md section 5.

use rquickjs::function::{Params, RustFunction};
use rquickjs::object::Accessor;

use super::*;
use crate::script::host::{
    self, Arg, ArgKind, Args, Bound, Call, EngineOps, FlushBefore, HostClass, HostError, Method, Ret,
};

/// An object of a host class: the state its methods read. Its prototype is its class.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostNeutral {
    #[qjs(skip_trace)]
    pub(super) obj: HostObject,
}

/// A new object of `class` over `obj`, its prototype the running engine's.
pub(super) fn running_instance<'js>(
    ctx: &Ctx<'js>,
    obj: HostObject,
    class: &'static HostClass,
) -> rquickjs::Result<Object<'js>> {
    instance(ctx, obj, class, Prototypes::Running)
}

/// The state a method of the host table reads from its receiver: an object of a host class, or an entity list or value
/// (exotic classes whose prototypes are the table's, `runtime/entities.rs`).
fn receiver_state<'js>(ctx: &Ctx<'js>, this: &JsValue<'js>) -> Option<HostObject> {
    if let Ok(class) = Class::<HostNeutral>::from_js(ctx, this.clone()) {
        return class.try_borrow().ok().map(|held| held.obj.clone());
    }
    let object = this.as_object()?;
    if let Some(class) = object.as_class::<super::HostEntity>() {
        return class.try_borrow().ok().map(|held| HostObject(held.inner.clone()));
    }
    if let Some(class) = object.as_class::<super::HostList>() {
        return class.try_borrow().ok().map(|held| held.host_state());
    }
    None
}

/// Install every path of the host table under `ec`, as the oxide binder's `install_ec` does, so a registered path is
/// bound on both engines. Runs while the engine starts: the prototypes made here are kept for this install only, not
/// in an engine's cache, since the engine being started is not the running one.
pub(super) fn install_ec<'js>(ctx: &Ctx<'js>, ec: &Object<'js>) -> Result<(), Error> {
    install_paths(ctx, ec, host::all_paths().iter().copied())
}

/// [`install_ec`] of the given paths.
fn install_paths<'js, 'p>(
    ctx: &Ctx<'js>,
    ec: &Object<'js>,
    paths: impl IntoIterator<Item = &'p host::Path>,
) -> Result<(), Error> {
    let made = RefCell::new(HashMap::new());
    for entry in paths {
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
            target = match target.get::<_, Option<Object>>(name) {
                Ok(Some(found)) => found,
                _ => {
                    let object = caught(ctx, entry.path, Object::new(ctx.clone()))?;
                    caught(ctx, entry.path, target.set(name, object.clone()))?;
                    object
                }
            };
        }
        match entry.bound {
            Bound::Object(class) => {
                let object = caught(
                    ctx,
                    entry.path,
                    instance(ctx, HostObject::new(()), class, Prototypes::Install(&made)),
                )?;
                caught(ctx, entry.path, target.set(last, object))?;
            }
            Bound::Function(method) => caught(ctx, entry.path, target.set(last, function(ctx, method)?))?,
            Bound::Shim(_) => {}
        }
    }
    Ok(())
}

/// Run every shim of the host table ([`host::Shim`]), once `ec` is a global and the helper objects are filled: each is
/// given the global object, its hidden functions and the helper objects. The shims are fixed framework scripts, so one
/// that fails fails the engine's start, naming it.
pub(super) fn install_shims<'js>(
    ctx: &Ctx<'js>,
    string_utilities: &Object<'js>,
    collection_utilities: &Object<'js>,
    web_utilities: &Object<'js>,
) -> Result<(), Error> {
    let helpers = caught(ctx, "helpers", Object::new(ctx.clone()))?;
    caught(ctx, "helpers", helpers.set("CollectionUtilities", collection_utilities.clone()))?;
    caught(ctx, "helpers", helpers.set("StringUtilities", string_utilities.clone()))?;
    caught(ctx, "helpers", helpers.set("WebUtilities", web_utilities.clone()))?;
    for entry in host::all_paths() {
        let Bound::Shim(shim) = entry.bound else {
            continue;
        };
        let installed = (|| {
            let hidden = caught(ctx, entry.path, Object::new(ctx.clone()))?;
            for method in shim.hidden.iter().flat_map(|group| group.iter()) {
                caught(ctx, method.path, hidden.set(method.name, function(ctx, method)?))?;
            }
            let install: Function = eval_js(ctx, shim.js)?;
            caught(
                ctx,
                entry.path,
                install.call::<_, ()>((ctx.globals(), hidden, helpers.clone())),
            )
        })();
        if let Err(err) = installed {
            return Err(Error::new(format!(
                "The host shim {} was not installed: {}",
                entry.path,
                err.message()
            )));
        }
    }
    Ok(())
}

/// Where a prototype is kept: for one install, or in the running engine.
#[derive(Clone, Copy)]
enum Prototypes<'a, 'js> {
    Install(&'a RefCell<HashMap<&'static str, Object<'js>>>),
    Running,
}

/// A new object of `class` over `obj`.
fn instance<'js>(
    ctx: &Ctx<'js>,
    obj: HostObject,
    class: &'static HostClass,
    kept: Prototypes<'_, 'js>,
) -> rquickjs::Result<Object<'js>> {
    let proto = prototype(ctx, class, kept)?;
    Ok(Class::instance_proto(HostNeutral { obj }, proto)?.into_inner())
}

/// The prototype of `class`, made the first time.
fn prototype<'js>(
    ctx: &Ctx<'js>,
    class: &'static HostClass,
    kept: Prototypes<'_, 'js>,
) -> rquickjs::Result<Object<'js>> {
    let made = match kept {
        Prototypes::Install(made) => {
            if let Some(found) = made.borrow().get(class.name) {
                return Ok(found.clone());
            }
            let proto = make_prototype(ctx, class);
            if let Ok(proto) = &proto {
                made.borrow_mut().insert(class.name, proto.clone());
            }
            proto
        }
        // A host call runs in the innermost engine, which is the one running it.
        Prototypes::Running => engine_object(ctx, class.name, |ctx| make_prototype(ctx, class)),
    };
    made.map_err(|err| Exception::throw_message(ctx, err.message()))
}

fn make_prototype<'js>(ctx: &Ctx<'js>, class: &'static HostClass) -> Result<Object<'js>, Error> {
    let proto = caught(ctx, class.name, Object::new(ctx.clone()))?;
    fill_prototype(ctx, &proto, class)?;
    Ok(proto)
}

/// Put `class`'s methods, getters and what its shim adds on `proto`: a new prototype, or the prototype of an exotic
/// QuickJS class whose methods are the table's (an entity list or value).
pub(super) fn fill_prototype<'js>(ctx: &Ctx<'js>, proto: &Object<'js>, class: &'static HostClass) -> Result<(), Error> {
    for method in class.methods {
        // not enumerable, as a class's methods are, so `for...in` over an entity value sees only its fields
        let made = rquickjs::object::Property::from(function(ctx, method)?).writable().configurable();
        caught(ctx, method.path, proto.prop(method.name, made))?;
    }
    for getter in class.getters {
        let get = move |ctx: Ctx<'js>, this: This<JsValue<'js>>| trampoline(&ctx, getter, this.0, 0, |_| None);
        caught(
            ctx,
            getter.path,
            proto.prop(getter.name, Accessor::from(get).configurable()),
        )?;
    }
    if let Some(shim) = class.shim_js {
        let hidden = caught(ctx, class.name, Object::new(ctx.clone()))?;
        for method in class.hidden {
            caught(ctx, method.path, hidden.set(method.name, function(ctx, method)?))?;
        }
        let install: Function = eval_js(ctx, shim)?;
        caught(ctx, class.name, install.call::<_, ()>((proto.clone(), hidden)))?;
    }
    Ok(())
}

/// The function of a method, with the `name` and `length` a typed function of its arguments had. It reads its
/// arguments from the call's parameters in place, so a call makes no vector of them.
fn function<'js>(ctx: &Ctx<'js>, method: &'static Method) -> Result<Function<'js>, Error> {
    let call = move |params: Params<'_, 'js>| -> rquickjs::Result<JsValue<'js>> {
        let ctx = params.ctx().clone();
        trampoline(&ctx, method, params.this(), params.len(), |index| params.arg(index))
    };
    let made = caught(
        ctx,
        method.path,
        Class::instance(ctx.clone(), RustFunction(Box::new(call))),
    )?;
    let made = caught(ctx, method.path, Function::from_js(ctx, made.into_value()))?;
    let made = caught(ctx, method.path, made.with_name(method.name))?;
    caught(ctx, method.path, made.with_length(method.required()))
}

fn trampoline<'js>(
    ctx: &Ctx<'js>,
    method: &'static Method,
    this: JsValue<'js>,
    given: usize,
    arg: impl Fn(usize) -> Option<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(ctx, || {
        host::note_path(method.path);
        let required = method.required();
        if given < required {
            return Err(JsError::MissingArgs {
                expected: required,
                given,
            });
        }
        let mut ops = QuickOps {
            ctx,
            runner: super::screen_host::CtxRunner { ctx },
            callbacks: Vec::new(),
            thrown: None,
        };
        let converted = convert_args(ctx, method.args, arg, &mut ops.callbacks)?;
        let state = receiver_state(ctx, &this);
        let flushed = match method.flush {
            FlushBefore::None => Ok(()),
            mode => ops.flush(mode),
        };
        let result = flushed.and_then(|()| {
            let mut call = Call {
                this: state.as_ref(),
                args: converted,
                ops: &mut ops,
            };
            (method.f)(&mut call)
        });
        match result {
            Ok(ret) => ret_to_js(ctx, ret, this),
            Err(err) => Err(throw(ctx, err, ops.thrown.take())),
        }
    })
}

/// The arguments as their kinds, by QuickJS's conversions (`String`, `Option<String>`, `bool`, `i32`, `f64`).
fn convert_args<'js>(
    ctx: &Ctx<'js>,
    kinds: &[ArgKind],
    arg: impl Fn(usize) -> Option<JsValue<'js>>,
    callbacks: &mut Vec<Function<'js>>,
) -> rquickjs::Result<Args> {
    let mut converted = Args::new();
    for (index, kind) in kinds.iter().enumerate() {
        let Some(value) = arg(index) else {
            converted.push(Arg::Missing);
            continue;
        };
        let void = value.is_null() || value.is_undefined();
        converted.push(match kind {
            ArgKind::Value => Arg::Value(js_to_host(ctx, &value)?),
            ArgKind::OptValue if value.is_undefined() => Arg::Undefined,
            ArgKind::OptValue => Arg::Value(js_to_host(ctx, &value)?),
            ArgKind::Text | ArgKind::OptText => Arg::Text(String::from_js(ctx, value)?),
            ArgKind::NullableText | ArgKind::OptNullableText if void => Arg::Undefined,
            ArgKind::NullableText | ArgKind::OptNullableText => Arg::Text(String::from_js(ctx, value)?),
            ArgKind::Bool | ArgKind::OptBool => Arg::Bool(bool::from_js(ctx, value)?),
            ArgKind::Int | ArgKind::OptInt => Arg::Int(i32::from_js(ctx, value)?),
            ArgKind::OptNumber | ArgKind::Number => Arg::Number(f64::from_js(ctx, value)?),
            ArgKind::Texts | ArgKind::OptTexts => Arg::Texts(Vec::<String>::from_js(ctx, value)?),
            ArgKind::Callback => {
                callbacks.push(Function::from_js(ctx, value)?);
                Arg::Callback(callbacks.len() - 1)
            }
            ArgKind::Context if value.is_object() => Arg::Context(value_map(&js_to_host(ctx, &value)?)),
            ArgKind::Context => Arg::Context(None),
            ArgKind::Thrown => thrown_text(&value).map_or(Arg::Missing, Arg::Text),
            ArgKind::OptWhole if void => Arg::Undefined,
            ArgKind::Whole | ArgKind::OptWhole => Arg::Int(whole(ctx, &value)?),
            ArgKind::Parameters if is_ec_context(ctx, &value)? => Arg::WholeContext,
            ArgKind::Parameters => Arg::Value(js_to_host(ctx, &value)?),
            ArgKind::Host => Arg::Host(Class::<HostNeutral>::from_js(ctx, value)?.try_borrow()?.obj.clone()),
            ArgKind::Node => Arg::Node(super::screen_host::lit_of(ctx, &value)?),
            ArgKind::OptNode if value.is_object() => Arg::Node(super::screen_host::lit_of(ctx, &value)?),
            ArgKind::OptNode => Arg::Undefined,
        });
    }
    Ok(converted)
}

/// An integer argument as runtime/convert.rs's `js_i64` reads it, with its errors.
fn whole(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<i32> {
    if let Some(number) = value.as_int() {
        return Ok(number);
    }
    if value.as_float().is_some() {
        return Err(Exception::throw_message(ctx, "limit is not an integer"));
    }
    Err(Exception::throw_message(ctx, "limit is not a number"))
}

/// Whether `value` is `ec.context`, the whole context: `in-map="context"` passes it.
fn is_ec_context<'js>(ctx: &Ctx<'js>, value: &JsValue<'js>) -> rquickjs::Result<bool> {
    if !value.is_object() {
        return Ok(false);
    }
    let ec: JsValue = ctx.globals().get("ec")?;
    let Some(ec) = ec.as_object() else {
        return Ok(false);
    };
    let context: JsValue = ec.get("context")?;
    Ok(context == *value)
}

/// The text a log call gives its exception: `name: message` of an error object, its `message` alone, or a string.
fn thrown_text(value: &JsValue<'_>) -> Option<String> {
    if value.is_null() || value.is_undefined() {
        return None;
    }
    Some(match value.as_object() {
        Some(object) => {
            let name: Option<String> = object.get("name").ok();
            let detail: Option<String> = object.get("message").ok();
            match (name, detail) {
                (Some(name), Some(detail)) => format!("{name}: {detail}"),
                (_, Some(detail)) => detail,
                _ => String::from("(exception)"),
            }
        }
        None => value
            .as_string()
            .and_then(|text| text.to_string().ok())
            .unwrap_or_default(),
    })
}

fn ret_to_js<'js>(ctx: &Ctx<'js>, ret: Ret, this: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    match ret {
        Ret::Value(value) => host_to_js(ctx, value),
        Ret::This => Ok(this),
        Ret::Undefined => Ok(JsValue::new_undefined(ctx.clone())),
        Ret::Object(obj, class) => instance(ctx, obj, class, Prototypes::Running).map(Object::into_value),
    }
}

/// Throw `err` as the facades throw it.
fn throw(ctx: &Ctx<'_>, err: HostError, thrown: Option<JsError>) -> JsError {
    match err {
        HostError::Message(message) => Exception::throw_message(ctx, &message),
        HostError::Type(message) => Exception::throw_type(ctx, &message),
        HostError::Lib(err) => throw_lib(ctx, &err),
        HostError::Context(err) => throw_context(ctx, &err),
        HostError::Thrown => thrown.unwrap_or_else(|| Exception::throw_message(ctx, "a host call failed")),
    }
}

/// [`EngineOps`] of one call on QuickJS: the call's functions, and the exception a callback or a flush left.
struct QuickOps<'a, 'js> {
    ctx: &'a Ctx<'js>,
    /// The screen runner of the call ([`EngineOps::screen_runner`]): units run in this context, as the call does.
    runner: super::screen_host::CtxRunner<'a, 'js>,
    callbacks: Vec<Function<'js>>,
    thrown: Option<JsError>,
}

impl EngineOps for QuickOps<'_, '_> {
    fn call_back(&mut self, index: usize, args: Vec<Value>) -> Result<Value, HostError> {
        let ctx = self.ctx;
        let function = self
            .callbacks
            .get(index)
            .cloned()
            .ok_or_else(|| HostError::message("no such callback"))?;
        let called = (|| {
            let args = args
                .into_iter()
                .map(|arg| host_to_js(ctx, arg))
                .collect::<rquickjs::Result<Vec<_>>>()?;
            let result: JsValue = function.call((Rest(args),))?;
            js_to_host(ctx, &result)
        })();
        called.map_err(|err| {
            self.thrown = Some(err);
            HostError::Thrown
        })
    }

    fn evaluate(&mut self, source: &str, _name: &str) -> Result<Value, HostError> {
        let ctx = self.ctx;
        let evaluated: JsValue = eval_source(ctx, source).map_err(|err| HostError::message(err.message()))?;
        js_to_host(ctx, &evaluated).map_err(|err| {
            self.thrown = Some(err);
            HostError::Thrown
        })
    }

    fn whole_context(&mut self) -> Result<indexmap::IndexMap<String, Value>, HostError> {
        if let Some(engine) = current_engine() {
            readopt_all(&engine);
        }
        self.flush(FlushBefore::Keep)?;
        Ok(context_combined())
    }

    fn screen_runner(&mut self) -> Result<&dyn crate::screen::sri::Runner, HostError> {
        Ok(&self.runner)
    }

    fn flush(&mut self, mode: FlushBefore) -> Result<(), HostError> {
        let clear = match mode {
            FlushBefore::None => return Ok(()),
            FlushBefore::Keep => false,
            FlushBefore::Release => true,
        };
        flush_current(self.ctx, clear).map_err(|err| {
            self.thrown = Some(err);
            HostError::Thrown
        })
    }
}

#[cfg(test)]
#[path = "bind_tests.rs"]
mod tests;
