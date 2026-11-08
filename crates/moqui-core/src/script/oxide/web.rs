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

//! The conf's script servlets, filters and listeners on quickjs-oxide under `script_engine=oxide`, the oxide side of
//! `runtime/web_host.rs`: the handler file's value called through the same invoker (`web_host::INVOKE_JS`, made once
//! per runtime), in a run of its own, with `chain.doFilter()` a host function that lends the engine to the rest of the
//! chain.

use moqui_lib::object::Value as HostValue;
use quickjs_oxide::engine::api::core::{CallableRef, Context, FunctionOptions, JsString, RuntimeError, Scope, Value};

use super::convert::{key, to_js, Conv};
use super::engine::{with_run, Outcome, OxideEngine};
use super::errors::runtime_error;
use crate::context::Error;
use crate::script::text::ScriptText;

/// [`crate::script::invoke_web_handler`] on quickjs-oxide.
pub(crate) fn invoke(location: &str, kind: &str, data: HostValue) -> Result<(), Error> {
    let (script, _) = crate::script::runtime::read_module(location)?;
    let _frame = crate::script::runtime::ContextFrame::push();
    let ran = with_run(true, location, |engine, context, note| {
        let called = call_handler(engine, context, &script, kind, &data);
        if let Err(Conv::Js(err)) = &called {
            note(err);
        }
        called.map_err(|conv| match conv {
            Conv::Js(err) => Outcome::Error(runtime_error(context, err)),
            Conv::Error(message) => Outcome::Error(Error::new(message)),
            Conv::Unsupported(reason) => Outcome::Fallback(reason),
        })
    });
    ran.map_err(|outcome| match outcome {
        Outcome::Error(err) => err,
        Outcome::Fallback(reason) => Error::new(format!(
            "script_engine=oxide: this web handler needs {reason}, which the quickjs-oxide bindings do not have yet"
        )),
    })
}

/// The handler file's value, called by the invoker for `kind` with `data`.
fn call_handler(
    engine: &OxideEngine,
    context: &mut Context,
    script: &ScriptText,
    kind: &str,
    data: &HostValue,
) -> Result<(), Conv> {
    let hash = crate::script::text::hash_of(script.hash(), script.file_name());
    let compiled = engine.compile(script.shared_text(), hash, script.file_name())?;
    let handler = context.execute(&compiled)?;
    let invoke = invoker(engine, context)?;
    let data = to_js(context, engine, data)?;
    let kind = Value::String(JsString::try_from_utf8(kind).map_err(|_| Conv::Error("a handler kind".to_string()))?);
    context.call(&invoke, Value::Undefined, &[handler, kind, data])?;
    Ok(())
}

/// The invoker's `invoke`, made once for the runtime over its `ec` and a `chainNext` host function.
fn invoker(engine: &OxideEngine, context: &Context) -> Result<CallableRef, RuntimeError> {
    if let Some(made) = engine.web_invoker().get() {
        return Ok(made.clone());
    }
    let invariant = |what: &'static str| RuntimeError::Invariant(what);
    let key_of = |name: &str| key(name).map_err(|_| invariant("a key"));
    let make = match context.eval(INVOKE_JS)? {
        Value::Object(object) => context.as_callable(&object)?,
        _ => None,
    }
    .ok_or(invariant("the web invoker"))?;
    let global = context.global_object()?;
    let ec = context.get_property(&global, &key_of("ec")?)?;
    let next = context.new_function(FunctionOptions::new("chainNext"), chain_next)?;
    let Value::Object(made) = context.call(&make, Value::Undefined, &[ec, Value::from(next)])? else {
        return Err(invariant("the web invoker"));
    };
    let invoke = match context.get_property(&made, &key_of("invoke")?)? {
        Value::Object(call) => context.as_callable(&call)?,
        _ => None,
    }
    .ok_or(invariant("the web invoker's invoke"))?;
    let _ = engine.web_invoker().set(invoke.clone());
    Ok(invoke)
}

/// `chain.doFilter()`: the next filter, or the servlet at the end of the chain, with the engine lent to it.
fn chain_next(scope: &mut Scope<'_>) -> Result<Value, RuntimeError> {
    super::bind::host_body(scope, |scope| {
        match super::bind::lend(scope, || Ok(crate::web::servlet::chain_next()))? {
            Ok(()) => Ok(Value::Undefined),
            Err(err) => Err(Conv::Js(super::bind::throw(
                scope,
                crate::script::host::HostError::Context(err),
                None,
            ))),
        }
    })
}

/// Calls a handler object's method for its kind: a servlet's `service`, else its `doGet` (`doPost`, ...) by method, a
/// filter's `doFilter` with the chain, a listener's method named for the event. The handler's `config` is set first.
const INVOKE_JS: &str = r#"(function (ec, chainNext) {
    const methods = { GET: "doGet", POST: "doPost", PUT: "doPut", DELETE: "doDelete", PATCH: "doPatch", HEAD: "doHead",
        OPTIONS: "doOptions" };
    function configOf(data) {
        const params = data.initParams || {};
        return { name: data.name, location: data.location, initParams: params,
            getInitParameter: (name) => params[name] ?? null, getInitParameterNames: () => Object.keys(params),
            getServletName: () => data.name, getFilterName: () => data.name };
    }
    return { invoke(handler, kind, data) {
        if (handler == null || typeof handler !== "object") {
            throw new Error(`${data.location} must evaluate to a handler object, as ({ doGet(request, response) {...} })`);
        }
        const config = configOf(data);
        if (Object.isExtensible(handler)) handler.config = config;
        if (kind === "listener") {
            const method = handler[data.event];
            if (typeof method === "function") method.call(handler, Object.assign({ type: data.event }, data.eventData || {}), config);
            return;
        }
        const request = ec.web.request, response = ec.web.response;
        if (kind === "filter") {
            if (typeof handler.doFilter !== "function") throw new Error(`${data.location} has no doFilter(request, response, chain)`);
            handler.doFilter(request, response, { doFilter: () => chainNext() }, config);
            return;
        }
        if (typeof handler.service === "function") { handler.service(request, response, config); return; }
        let name = methods[request.method];
        if (name === "doHead" && typeof handler.doHead !== "function") name = "doGet";
        if (name && typeof handler[name] === "function") { handler[name](request, response, config); return; }
        response.sendError(405, `HTTP method ${request.method} is not supported by this URL`);
    } };
})"#;
