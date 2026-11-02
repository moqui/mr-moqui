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

//! `ec.service.passAuthority(endpoint)`: a PASS frontend's calls to its authority over the host's gRPC, beside
//! `ec.service.rest()` (plans/Edge-Node.md §5.5, `crate::pass_authority`). The endpoint is `grpc://host:port`,
//! `grpcs://host:port` or `edge:`.
//!
//! ```text
//! client.call({operation, siteId, input, delegation, visitorAddress, operationId, sessionHandle})
//!     -> {result} or {error: {code, message, fieldErrors, correlationId, status}}, with sessionHandle
//! client.upload(call, file)   file: an uploaded file (ec.web.multiPartParameters.file); the same answer
//! client.download(call)       -> a response for ec.web.response.forward: statusCode, headerPairs and the body left on
//!                                the server, or the error envelope as a JSON body with its status; .error is set then
//! ```
//!
//! With no gRPC in the host (Lite, Dev) `passAuthority` throws "gRPC not built into this host", which mr-pass's
//! automatic transport reads as REST. A transport failure (the authority or the link down) is an `Unavailable` error
//! envelope, so a caller handles it as the REST adapter's 503.

use crate::pass_authority::{self, Call, PassError, UploadFile};

use super::*;

const PASS_JS: &str = r#"(function (ec, host) {
    function PassAuthority(endpoint) { this.endpoint = String(endpoint); }
    PassAuthority.prototype.call = function (call) { return host.passCall(this.endpoint, call || {}); };
    PassAuthority.prototype.upload = function (call, file) { return host.passUpload(this.endpoint, call || {}, file); };
    PassAuthority.prototype.download = function (call) {
        const opened = host.passOpen(this.endpoint, call || {});
        if (opened.error) {
            const body = JSON.stringify({ code: opened.error.code, message: opened.error.message,
                fieldErrors: opened.error.fieldErrors || [], correlationId: opened.error.correlationId });
            return { error: opened.error, statusCode: opened.error.status, bodyId: null,
                headerPairs: [["Content-Type", "application/json"], ["Cache-Control", "no-store"]],
                bytes: function () { return body; }, text: function () { return body; } };
        }
        return { error: null, statusCode: 200, headerPairs: opened.headers, bodyId: opened.bodyId,
            contentLength: opened.length,
            bytes: function () {
                if (this.bodyId == null) return null;
                const id = this.bodyId; this.bodyId = null;
                return host.restReadBody(id).bytes;
            } };
    };
    ec.service.passAuthority = function (endpoint) {
        if (!host.passBuilt()) throw new Error("gRPC not built into this host");
        return new PassAuthority(endpoint);
    };
})"#;

pub(super) fn install<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> Result<(), Error> {
    let define: Function = eval_js(ctx, PASS_JS)?;
    let ec: Object = caught(ctx, "ec", ctx.globals().get("ec"))?;
    caught(ctx, "passAuthority", define.call::<_, ()>((ec, host.clone())))
}

pub(super) fn methods<'js>(ctx: &Ctx<'js>, host: &Object<'js>) -> Result<(), Error> {
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = caught(ctx, $name, Function::new(ctx.clone(), $function))?;
            caught(ctx, $name, host.set($name, function))?;
        };
    }
    method!("passBuilt", pass_built);
    method!("passCall", pass_call);
    method!("passOpen", pass_open);
    method!("passUpload", pass_upload);
    Ok(())
}

fn pass_built(ctx: Ctx<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(pass_authority::sender().is_ok()))
}

/// The call object a script passes, checked: the operation and site are required.
fn call_of(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<Call> {
    let Value::Map(map) = js_to_host(ctx, value)? else {
        return Err(Exception::throw_message(ctx, "a PASS authority call is not an object"));
    };
    let text = |name: &str| match map.get(name) {
        Some(Value::Null) | None => String::new(),
        Some(value) => moqui_lib::object::to_plain_string(value),
    };
    let call = Call {
        operation: text("operation"),
        site_id: text("siteId"),
        input: match map.get("input") {
            Some(input @ Value::Map(_)) => moqui_lib::json::to_json(input),
            _ => serde_json::json!({}),
        },
        delegation: text("delegation"),
        visitor_address: text("visitorAddress"),
        operation_id: text("operationId"),
        session_handle: text("sessionHandle"),
        client_id: String::new(),
    };
    if call.operation.is_empty() {
        return Err(Exception::throw_message(
            ctx,
            "a PASS authority call needs an operation",
        ));
    }
    Ok(call)
}

/// Refuse a call sim mode or the effect fence stops, as `ec.service.rest()` does.
fn allowed(ctx: &Ctx<'_>, endpoint: &str, call: &Call) -> rquickjs::Result<()> {
    let what = format!("PASS authority {} at {endpoint}", call.operation);
    let sim = with_ec(ctx, |ec| ec.factory().entity())?.is_some_and(|facade| facade.sim_active());
    if sim {
        return Err(Exception::throw_message(
            ctx,
            &format!("{what} denied because sim mode is active"),
        ));
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, &what)
        .map_err(|refused| Exception::throw_message(ctx, refused.message()))
}

fn sender(ctx: &Ctx<'_>) -> rquickjs::Result<Arc<dyn pass_authority::PassSender>> {
    pass_authority::sender().map_err(|err| Exception::throw_message(ctx, err.message()))
}

/// A transport failure as the REST adapter's 503, logged with the reason the client does not see.
fn unavailable(endpoint: &str, call: &Call, err: &moqui_lib::error::Error) -> PassError {
    tracing::warn!(
        "PASS authority {} at {endpoint} could not be reached: {}",
        call.operation,
        err.message()
    );
    PassError::unavailable(&if err.message().contains(crate::edge::OFFLINE) {
        crate::edge::OFFLINE.to_string()
    } else {
        "Authority temporarily unavailable".to_string()
    })
}

fn reply_js<'js>(ctx: &Ctx<'js>, reply: &pass_authority::Reply) -> rquickjs::Result<JsValue<'js>> {
    host_to_js(ctx, moqui_lib::json::from_json(&pass_authority::reply_json(reply)))
}

fn pass_call<'js>(ctx: Ctx<'js>, endpoint: String, call: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let call = call_of(&ctx, &call)?;
        allowed(&ctx, &endpoint, &call)?;
        let sender = sender(&ctx)?;
        let reply = sender
            .call(&endpoint, call.clone())
            .unwrap_or_else(|err| pass_authority::Reply::error(unavailable(&endpoint, &call, &err)));
        reply_js(&ctx, &reply)
    })
}

fn pass_upload<'js>(
    ctx: Ctx<'js>,
    endpoint: String,
    call: JsValue<'js>,
    file: JsValue<'js>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let call = call_of(&ctx, &call)?;
        allowed(&ctx, &endpoint, &call)?;
        let upload = match js_to_host(&ctx, &file)? {
            Value::Object(object) => object.downcast::<crate::web::multipart::UploadedFile>(),
            _ => None,
        };
        let Some(upload) = upload else {
            return Err(Exception::throw_message(
                &ctx,
                "a PASS authority upload needs an uploaded file",
            ));
        };
        if upload.size() > pass_authority::MAX_UPLOAD_BYTES {
            let error = PassError {
                code: "Unprocessable".to_string(),
                message: "Upload exceeds the size limit".to_string(),
                status: 422,
                ..PassError::default()
            };
            return reply_js(&ctx, &pass_authority::Reply::error(error));
        }
        let bytes = upload
            .read_bytes()
            .map_err(|err| Exception::throw_message(&ctx, err.message()))?;
        let file = UploadFile {
            filename: upload.file_name.clone(),
            content_type: upload.content_type.clone(),
            bytes,
        };
        let sender = sender(&ctx)?;
        let reply = sender
            .upload(&endpoint, call.clone(), file)
            .unwrap_or_else(|err| pass_authority::Reply::error(unavailable(&endpoint, &call, &err)));
        reply_js(&ctx, &reply)
    })
}

/// A download: `{headers, bodyId, length}` with the body left on the server, or `{error}`.
fn pass_open<'js>(ctx: Ctx<'js>, endpoint: String, call: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let call = call_of(&ctx, &call)?;
        allowed(&ctx, &endpoint, &call)?;
        let sender = sender(&ctx)?;
        let opened = match sender.download(&endpoint, call.clone()) {
            Ok(opened) => opened,
            Err(err) => Err(unavailable(&endpoint, &call, &err)),
        };
        let mut map = indexmap::IndexMap::new();
        match opened {
            Ok(download) => {
                let meta = &download.meta;
                let filename = meta.filename.replace(['"', '\\', '\r', '\n'], "_");
                let content_type = if meta.content_type.contains(['\r', '\n']) {
                    "application/octet-stream".to_string()
                } else {
                    meta.content_type.clone()
                };
                let headers = [
                    ("Content-Type".to_string(), content_type),
                    ("Cache-Control".to_string(), "no-store".to_string()),
                    ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                    (
                        "Content-Disposition".to_string(),
                        format!("{}; filename=\"{filename}\"", meta.disposition),
                    ),
                ];
                map.insert(
                    "headers".to_string(),
                    Value::List(
                        headers
                            .into_iter()
                            .map(|(name, value)| Value::List(vec![Value::String(name), Value::String(value)]))
                            .collect(),
                    ),
                );
                map.insert(
                    "length".to_string(),
                    meta.length
                        .and_then(|length| i64::try_from(length).ok())
                        .map_or(Value::Null, Value::I64),
                );
                let id = crate::web::upstream::register(download.reader, meta.length);
                map.insert("bodyId".to_string(), Value::I64(i64::from(id)));
            }
            Err(error) => {
                map.insert("error".to_string(), moqui_lib::json::from_json(&error.to_json()));
            }
        }
        host_to_js(&ctx, Value::Map(map))
    })
}
