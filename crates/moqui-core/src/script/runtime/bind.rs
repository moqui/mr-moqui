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
use crate::script::host::{self, Arg, ArgKind, Args, Call, EngineOps, FlushBefore, HostClass, HostError, Method, Ret};

/// An object of a host class: the state its methods read. Its prototype is its class.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostNeutral {
    #[qjs(skip_trace)]
    obj: HostObject,
}

/// Install every path of the host table under `ec`, as the oxide binder's `install_ec` does, so a registered path is
/// bound on both engines. Runs while the engine starts: the prototypes made here are kept for this install only, not
/// in an engine's cache, since the engine being started is not the running one.
pub(super) fn install_ec<'js>(ctx: &Ctx<'js>, ec: &Object<'js>) -> Result<(), Error> {
    let made = RefCell::new(HashMap::new());
    for entry in host::all_paths() {
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
        let object = caught(
            ctx,
            entry.path,
            instance(ctx, HostObject::new(()), entry.class, Prototypes::Install(&made)),
        )?;
        caught(ctx, entry.path, target.set(last, object))?;
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
    for method in class.methods {
        caught(ctx, method.path, proto.set(method.name, function(ctx, method)?))?;
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
    Ok(proto)
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
            callbacks: Vec::new(),
            thrown: None,
        };
        let converted = convert_args(ctx, method.args, arg, &mut ops.callbacks)?;
        let state = Class::<HostNeutral>::from_js(ctx, this.clone())
            .ok()
            .and_then(|class| class.try_borrow().ok().map(|held| held.obj.clone()));
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
            ArgKind::OptNumber => Arg::Number(f64::from_js(ctx, value)?),
            ArgKind::Callback => {
                callbacks.push(Function::from_js(ctx, value)?);
                Arg::Callback(callbacks.len() - 1)
            }
            ArgKind::Context if value.is_object() => Arg::Context(value_map(&js_to_host(ctx, &value)?)),
            ArgKind::Context => Arg::Context(None),
            ArgKind::Thrown => thrown_text(&value).map_or(Arg::Missing, Arg::Text),
        });
    }
    Ok(converted)
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
