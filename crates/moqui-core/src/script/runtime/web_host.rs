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

//! The handler objects of the conf's script servlets, filters and listeners ([`invoke`]), over the request and
//! response the tools shim makes (`script/host/web.rs`). Guide: `docs/interfaces/Web-Scripts.md`.

use super::*;

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

/// Run the handler object at `location` for `kind` (`servlet`, `filter` or `listener`), in a context frame of its own.
/// `data` carries `name`, `location`, `initParams` and, for a listener, `event` and `eventData`.
///
/// # Errors
///
/// Returns an error when the file cannot be read, does not evaluate to an object, or the handler throws.
pub(crate) fn invoke(location: &str, kind: &str, data: Value) -> Result<(), Error> {
    let source = super::facades::read_script(location)?;
    let _frame = crate::script::ContextFrame::push();
    with_context(|ctx| {
        let script = ScriptText::named(source, location);
        let handler = eval_compiled_text::<JsValue>(ctx, &script, false)?;
        let invoker = engine_object(ctx, "web-invoke", |ctx| {
            let make: Function = eval_js(ctx, INVOKE_JS)?;
            let ec: Object = caught(ctx, "ec", ctx.globals().get("ec"))?;
            let next = caught(ctx, "chainNext", Function::new(ctx.clone(), chain_next))?;
            caught(ctx, "web-invoke", make.call::<_, Object>((ec, next)))
        })?;
        let call: Function = caught(ctx, "invoke", invoker.get("invoke"))?;
        let data = host_to_js(ctx, data).map_err(js_error)?;
        call.call::<_, ()>((handler, kind, data))
            .catch(ctx)
            .map_err(|caught| caught_error(Error::script(caught.to_string()), &caught))
    })
}

/// `chain.doFilter()`: the next filter, or the servlet at the end of the chain.
fn chain_next(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        crate::web::servlet::chain_next().map_err(|err| throw_context(&ctx, &err))
    })
}
