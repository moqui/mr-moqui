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

//! The JSON-RPC 2.0 binding of A2A 1.0: `POST /a2a` with the PascalCase methods. An error is HTTP 200 with the
//! JSON-RPC `error` (its code, a message, and a `google.rpc.ErrorInfo` with the A2A reason in `data`); a batch is
//! `-32600`, a body that is not JSON `-32700`, a request without `A2A-Version: 1.0` `-32009`. The streaming methods
//! (`SendStreamingMessage`, `SubscribeToTask`) answer `text/event-stream`, each `data:` line a JSON-RPC response whose
//! `result` is a `StreamResponse`; an error before the stream is the plain JSON-RPC error.

use std::sync::Arc;

use serde_json::{json, Value as Json};

use super::types::{self, A2aError};
use super::{card, ops, push, Answer, Caller, Failure, HttpRequest, Reply};
use crate::context::ExecutionContextFactory;

/// A JSON-RPC response with `result`.
pub fn success(id: &Json, result: Json) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// A JSON-RPC response with `error`.
pub fn failure(id: &Json, error: &A2aError) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "error": error.to_rpc()})
}

fn reply(id: &Json, result: Result<Json, Failure>) -> Reply {
    match result {
        Ok(result) => Reply::json(200, &success(id, result)),
        Err(Failure::A2a(error)) => Reply::json(200, &failure(id, &error)),
        Err(Failure::Reply(reply)) => reply,
    }
}

/// The envelope of a request: its id (a string, a number or null), method and params (an object; absent is `{}`).
///
/// # Errors
///
/// Returns the id read so far and the error: ParseError for a body that is not JSON, InvalidRequest for a batch or a
/// malformed envelope, InvalidParams for params that are not an object.
pub fn envelope(body: &[u8]) -> Result<(Json, String, Json), (Json, A2aError)> {
    let parsed: Json = serde_json::from_slice(body)
        .map_err(|error| (Json::Null, A2aError::parse(format!("the body is not JSON: {error}"))))?;
    if parsed.is_array() {
        return Err((Json::Null, A2aError::invalid_request("batch requests are not served")));
    }
    let Json::Object(call) = parsed else {
        return Err((
            Json::Null,
            A2aError::invalid_request("the request must be a JSON object"),
        ));
    };
    let id = match call.get("id") {
        None => Json::Null,
        Some(id @ (Json::String(_) | Json::Number(_) | Json::Null)) => id.clone(),
        Some(_) => {
            return Err((
                Json::Null,
                A2aError::invalid_request("id must be a string, a number or null"),
            ))
        }
    };
    if call.get("jsonrpc").and_then(Json::as_str) != Some("2.0") {
        return Err((id, A2aError::invalid_request("jsonrpc must be \"2.0\"")));
    }
    let Some(method) = call
        .get("method")
        .and_then(Json::as_str)
        .filter(|method| !method.is_empty())
    else {
        return Err((id, A2aError::invalid_request("method is required")));
    };
    let params = match call.get("params") {
        None | Some(Json::Null) => json!({}),
        Some(params @ Json::Object(_)) => params.clone(),
        Some(_) => return Err((id, A2aError::invalid_params("params must be an object"))),
    };
    Ok((id, method.to_string(), params))
}

/// A stream frame: one JSON-RPC response with a `StreamResponse`.
fn frame(id: Json) -> impl Fn(&Json) -> String + Send + 'static {
    move |response: &Json| format!("data: {}\n\n", success(&id, response.clone()))
}

/// Serve a JSON-RPC request of an authenticated caller.
pub(crate) fn serve(factory: &Arc<ExecutionContextFactory>, caller: &Caller, request: &HttpRequest) -> Answer {
    if !request.method.eq_ignore_ascii_case("POST") {
        return Answer::Reply(
            Reply::json(
                405,
                &json!({"error": {"code": 405, "status": "UNIMPLEMENTED", "message": "JSON-RPC takes POST"}}),
            )
            .with_header("allow", "POST"),
        );
    }
    let (id, method, params) = match envelope(&request.body) {
        Ok(envelope) => envelope,
        Err((id, error)) => return Answer::Reply(Reply::json(200, &failure(&id, &error))),
    };
    if let Err(error) = types::check_version(request.header(types::VERSION_HEADER)) {
        return Answer::Reply(Reply::json(200, &failure(&id, &error)));
    }
    dispatch(factory, caller, &id, &method, &params)
}

/// Run one method.
pub(crate) fn dispatch(
    factory: &Arc<ExecutionContextFactory>,
    caller: &Caller,
    id: &Json,
    method: &str,
    params: &Json,
) -> Answer {
    let ctx = match ops::Ctx::new(factory, caller) {
        Ok(ctx) => ctx,
        Err(error) => return Answer::Reply(Reply::json(200, &failure(id, &error))),
    };
    let unary = |result: Result<Json, Failure>| Answer::Reply(reply(id, result));
    match method {
        "SendMessage" => {
            let id = id.clone();
            super::send_answer(factory, caller, params, Box::new(move |result| reply(&id, result)))
        }
        "SendStreamingMessage" => {
            let streamed = ops::send(&ctx, params).and_then(|sent| {
                let history = sent.config.history_length;
                super::stream_answer(factory, caller, sent.task, history, true, frame(id.clone()))
            });
            streamed.unwrap_or_else(|failure| unary(Err(failure)))
        }
        "SubscribeToTask" => {
            let streamed = ops::subscribe(&ctx, params).and_then(|task| {
                let close = super::closes_on_pause(&task);
                super::stream_answer(factory, caller, task, Some(0), close, frame(id.clone()))
            });
            streamed.unwrap_or_else(|failure| unary(Err(failure)))
        }
        "GetTask" => unary(ops::get(&ctx, params)),
        "ListTasks" => unary(ops::list(&ctx, params)),
        "CancelTask" => {
            let id = id.clone();
            super::cancel_answer(factory, caller, params, Box::new(move |result| reply(&id, result)))
        }
        "CreateTaskPushNotificationConfig" => unary(push::create_op(&ctx, params)),
        "GetTaskPushNotificationConfig" => unary(push::get_op(&ctx, params)),
        "ListTaskPushNotificationConfigs" => unary(push::list_op(&ctx, params)),
        "DeleteTaskPushNotificationConfig" => unary(push::delete_op(&ctx, params)),
        "GetExtendedAgentCard" => unary(
            card::extended(factory, &ctx.facade, caller).map_err(|error| Failure::of(&error, "GetExtendedAgentCard")),
        ),
        other => unary(Err(A2aError::method_not_found(other).into())),
    }
}
