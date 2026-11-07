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
//!                                the server, or the error envelope as a JSON body with its status; .error is set then.
//!                                A file that takes byte ranges (the call's input asked one) answers 200, 206 or 416
//!                                with Accept-Ranges and Content-Range; a public image may carry the authority's
//!                                `public, max-age=60` in place of no-store, and a theme asset by digest
//!                                (`getShopThemeAsset`) `public, max-age=31536000, immutable`
//! ```
//!
//! With no gRPC in the host (Lite, Dev) `passAuthority` throws "gRPC not built into this host", which mr-pass's
//! automatic transport reads as REST. A transport failure (the authority or the link down) is an `Unavailable` error
//! envelope, so a caller handles it as the REST adapter's 503. The functions are hidden functions of the tools shim;
//! `pass_js!` puts `passAuthority` on `ec.service`.

use moqui_lib::object::Value;

use super::{method, with_ec, Call, HostError, Method, Ret};
use crate::pass_authority::{self, Call as PassCall, PassError, UploadFile};

/// `(function (ec, host) { ... })`: `ec.service.passAuthority` over the hidden functions.
macro_rules! pass_js {
    () => {
        r#"(function (ec, host) {
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
        return { error: null, statusCode: opened.status, headerPairs: opened.headers, bodyId: opened.bodyId,
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
})"#
    };
}
pub(super) use pass_js;

/// The PASS functions of the tools shim.
pub(super) static METHODS: &[Method] = &[
    method!("tools", "passBuilt", [], None, leaf, pass_built),
    method!("tools", "passCall", [Text, Value], pass_call),
    method!("tools", "passOpen", [Text, Value], pass_open),
    method!("tools", "passUpload", [Text, Value, Value], pass_upload),
];

fn pass_built(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(pass_authority::sender().is_ok().into())
}

/// The call object a script passes, checked: the operation and site are required.
fn call_of(value: Option<&Value>) -> Result<PassCall, HostError> {
    let Some(Value::Map(map)) = value else {
        return Err(HostError::message("a PASS authority call is not an object"));
    };
    let text = |name: &str| match map.get(name) {
        Some(Value::Null) | None => String::new(),
        Some(value) => moqui_lib::object::to_plain_string(value),
    };
    let call = PassCall {
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
        return Err(HostError::message("a PASS authority call needs an operation"));
    }
    Ok(call)
}

/// Refuse a call sim mode or the effect fence stops, as `ec.service.rest()` does.
fn allowed(endpoint: &str, call: &PassCall) -> Result<(), HostError> {
    let what = format!("PASS authority {} at {endpoint}", call.operation);
    let sim = with_ec(|ec| ec.factory().entity())?.is_some_and(|facade| facade.sim_active());
    if sim {
        return Err(HostError::message(format!("{what} denied because sim mode is active")));
    }
    crate::entity::fence::check_effect(crate::entity::fence::Effect::RemoteCall, &what)
        .map_err(|refused| HostError::message(refused.message()))
}

fn sender() -> Result<std::sync::Arc<dyn pass_authority::PassSender>, HostError> {
    pass_authority::sender().map_err(|err| HostError::message(err.message()))
}

/// A transport failure as the REST adapter's 503, logged with the reason the client does not see.
fn unavailable(endpoint: &str, call: &PassCall, err: &moqui_lib::error::Error) -> PassError {
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

fn reply_value(reply: &pass_authority::Reply) -> Result<Ret, HostError> {
    Ok(moqui_lib::json::from_json(&pass_authority::reply_json(reply)).into())
}

fn pass_call(host_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let endpoint = host_call.text_or_empty(0);
    let call = call_of(host_call.value(1))?;
    allowed(&endpoint, &call)?;
    let sender = sender()?;
    let reply = sender
        .call(&endpoint, call.clone())
        .unwrap_or_else(|err| pass_authority::Reply::error(unavailable(&endpoint, &call, &err)));
    reply_value(&reply)
}

fn pass_upload(host_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let endpoint = host_call.text_or_empty(0);
    let call = call_of(host_call.value(1))?;
    allowed(&endpoint, &call)?;
    let upload = match host_call.value(2) {
        Some(Value::Object(object)) => object.downcast::<crate::web::multipart::UploadedFile>(),
        _ => None,
    };
    let Some(upload) = upload else {
        return Err(HostError::message("a PASS authority upload needs an uploaded file"));
    };
    if upload.size() > pass_authority::MAX_UPLOAD_BYTES {
        let error = PassError {
            code: "Unprocessable".to_string(),
            message: "Upload exceeds the size limit".to_string(),
            status: 422,
            ..PassError::default()
        };
        return reply_value(&pass_authority::Reply::error(error));
    }
    let bytes = upload.read_bytes().map_err(|err| HostError::message(err.message()))?;
    let file = UploadFile {
        filename: upload.file_name.clone(),
        content_type: upload.content_type.clone(),
        bytes,
    };
    let sender = sender()?;
    let reply = sender
        .upload(&endpoint, call.clone(), file)
        .unwrap_or_else(|err| pass_authority::Reply::error(unavailable(&endpoint, &call, &err)));
    reply_value(&reply)
}

/// A download: `{status, headers, bodyId, length}` with the body left on the server, or `{error}`.
fn pass_open(host_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let endpoint = host_call.text_or_empty(0);
    let call = call_of(host_call.value(1))?;
    allowed(&endpoint, &call)?;
    let sender = sender()?;
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
            // only a public product image (Shop F118) or a theme asset by digest (F55) may be kept by a shared cache;
            // the sender passes no other value
            let cache_control = meta
                .cache_control
                .as_deref()
                .and_then(|value| pass_authority::download_cache_control(&call.operation, value))
                .unwrap_or("no-store");
            let mut headers = vec![
                ("Content-Type".to_string(), content_type),
                ("Cache-Control".to_string(), cache_control.to_string()),
                ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                (
                    "Content-Disposition".to_string(),
                    format!("{}; filename=\"{filename}\"", meta.disposition),
                ),
            ];
            // a file that takes byte ranges answers its range as a static file does (Shop F88)
            if let Some(ranged) = meta.ranged {
                headers.push(("Accept-Ranges".to_string(), "bytes".to_string()));
                if let Some(range) = ranged.content_range() {
                    headers.push(("Content-Range".to_string(), range));
                }
            }
            map.insert(
                "status".to_string(),
                Value::I64(i64::from(meta.ranged.map_or(200, pass_authority::Ranged::status))),
            );
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
    Ok(Value::Map(map).into())
}
