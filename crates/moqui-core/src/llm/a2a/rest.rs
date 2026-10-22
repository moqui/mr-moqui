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

//! The HTTP+JSON binding of A2A 1.0 under `/a2a/`, its paths read by hand as `/git/` is (a segment like
//! `{id}:cancel` cannot be a route parameter):
//!
//! | Method | Path | Operation |
//! | --- | --- | --- |
//! | POST | `/message:send`, `/message:stream` | SendMessage, SendStreamingMessage |
//! | GET | `/tasks`, `/tasks/{id}` | ListTasks, GetTask (the parameters in the query) |
//! | POST | `/tasks/{id}:cancel` | CancelTask |
//! | GET, POST | `/tasks/{id}:subscribe` | SubscribeToTask |
//! | POST, GET | `/tasks/{id}/pushNotificationConfigs` | create, list |
//! | GET, DELETE | `/tasks/{id}/pushNotificationConfigs/{configId}` | get, delete |
//! | GET | `/extendedAgentCard` | GetExtendedAgentCard |
//!
//! An error is a `google.rpc.Status` body (`{error: {code, status, message, details}}`) with the HTTP status of its
//! A2A error (TaskNotFound 404, an invalid request 400, an internal error 500). A stream's frames are bare
//! `StreamResponse`s.

use std::sync::Arc;

use serde_json::{json, Map, Value as Json};

use super::types::{self, A2aError};
use super::{card, ops, push, Answer, Caller, Failure, HttpRequest, Reply};
use crate::context::ExecutionContextFactory;

fn reply(result: Result<Json, Failure>) -> Reply {
    match result {
        Ok(result) => Reply::json(200, &result),
        Err(Failure::A2a(error)) => Reply::json(error.http_status().0, &error.to_rest()),
        Err(Failure::Reply(reply)) => reply,
    }
}

fn frame(response: &Json) -> String {
    format!("data: {response}\n\n")
}

/// The request's query as a JSON object of strings.
fn query_params(request: &HttpRequest) -> Json {
    let mut params = Map::new();
    for (name, value) in &request.query {
        params.insert(name.clone(), Json::String(value.clone()));
    }
    Json::Object(params)
}

/// The body as a JSON object; empty is `{}`.
fn body_params(request: &HttpRequest) -> Result<Json, A2aError> {
    if request.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Json>(&request.body) {
        Ok(object @ Json::Object(_)) => Ok(object),
        Ok(_) => Err(A2aError::invalid_request("the body must be a JSON object")),
        Err(error) => Err(A2aError::parse(format!("the body is not JSON: {error}"))),
    }
}

fn with(mut params: Json, field: &str, value: &str) -> Json {
    params[field] = Json::String(value.to_string());
    params
}

/// Serve an HTTP+JSON request of an authenticated caller.
pub(crate) fn serve(factory: &Arc<ExecutionContextFactory>, caller: &Caller, request: &HttpRequest) -> Answer {
    if let Err(error) = types::check_version(request.header(types::VERSION_HEADER)) {
        return Answer::Reply(reply(Err(error.into())));
    }
    let ctx = match ops::Ctx::new(factory, caller) {
        Ok(ctx) => ctx,
        Err(error) => return Answer::Reply(reply(Err(error.into()))),
    };
    let method = request.method.to_ascii_uppercase();
    let path = request.path.trim_end_matches('/');
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let body = || body_params(request).map_err(Failure::from);
    let unary = |result: Result<Json, Failure>| Answer::Reply(reply(result));
    match (method.as_str(), segments.as_slice()) {
        ("POST", ["message:send"]) => match body_params(request) {
            Ok(params) => super::send_answer(factory, caller, &params, Box::new(reply)),
            Err(error) => unary(Err(error.into())),
        },
        ("POST", ["message:stream"]) => body()
            .and_then(|params| ops::send(&ctx, &params))
            .and_then(|sent| {
                let history = sent.config.history_length;
                super::stream_answer(factory, caller, sent.task, history, true, frame)
            })
            .unwrap_or_else(|failure| unary(Err(failure))),
        ("GET", ["tasks"]) => unary(ops::list(&ctx, &query_params(request))),
        ("GET", ["extendedAgentCard"]) => unary(
            card::extended(factory, &ctx.facade, caller).map_err(|error| Failure::of(&error, "GetExtendedAgentCard")),
        ),
        (_, ["tasks", task]) if task.ends_with(":cancel") || task.ends_with(":subscribe") => {
            let (task_id, action) = task.rsplit_once(':').unwrap_or((task, ""));
            match (method.as_str(), action) {
                ("POST", "cancel") => match body() {
                    Ok(params) => super::cancel_answer(factory, caller, &with(params, "id", task_id), Box::new(reply)),
                    Err(failure) => unary(Err(failure)),
                },
                ("GET" | "POST", "subscribe") => ops::subscribe(&ctx, &json!({"id": task_id}))
                    .and_then(|task| {
                        let close = super::closes_on_pause(&task);
                        super::stream_answer(factory, caller, task, Some(0), close, frame)
                    })
                    .unwrap_or_else(|failure| unary(Err(failure))),
                _ => unary(Err(A2aError::method_not_found(&format!("{method} {path}")).into())),
            }
        }
        ("GET", ["tasks", task_id]) => unary(ops::get(&ctx, &with(query_params(request), "id", task_id))),
        ("POST", ["tasks", task_id, "pushNotificationConfigs"]) => {
            unary(body().and_then(|params| push::create_op(&ctx, &with(params, "taskId", task_id))))
        }
        ("GET", ["tasks", task_id, "pushNotificationConfigs"]) => {
            unary(push::list_op(&ctx, &with(query_params(request), "taskId", task_id)))
        }
        ("GET", ["tasks", task_id, "pushNotificationConfigs", config_id]) => {
            unary(push::get_op(&ctx, &json!({"taskId": task_id, "id": config_id})))
        }
        ("DELETE", ["tasks", task_id, "pushNotificationConfigs", config_id]) => {
            unary(push::delete_op(&ctx, &json!({"taskId": task_id, "id": config_id})))
        }
        _ => unary(Err(A2aError::method_not_found(&format!("{method} {path}")).into())),
    }
}
