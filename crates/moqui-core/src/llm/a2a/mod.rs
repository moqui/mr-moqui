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

//! The A2A 1.0 server (plans/LLM-A2A-Assist.md 4.15, D9, D10; docs/A2A.md), off by default (`llm-facade` `a2a
//! enabled`). It is an adapter over the one run model: a context is a conversation, a task is a run of the `a2a`
//! surface, and the task states are the run states, whose wire values are A2A's.
//!
//! Both bindings: JSON-RPC 2.0 at `POST /a2a` ([`jsonrpc`]) and HTTP+JSON under `/a2a/` ([`rest`]), over the same
//! operations ([`ops`]): SendMessage (blocking, or `returnImmediately`), SendStreamingMessage, GetTask, ListTasks
//! (an owner-scoped keyset cursor, the page's history and artifacts loaded in a few queries), CancelTask,
//! SubscribeToTask, the push notification configs ([`push`], delivered from an outbox, token-authenticated as the
//! 1.0 schema has it) and the extended agent card ([`card`]). A stream projects the run's stored, sequenced events:
//! the task first, then `statusUpdate` for each state and `artifactUpdate` for the text of the `response` artifact,
//! closing on a terminal or interrupted state.
//!
//! A request is authenticated here, on the request pool: `Authorization: Bearer <login key>`, HTTP Basic with an
//! app password (or a login key as the password), or the `api_key`/`login_key` header; only over TLS, a believed
//! proxy's https or loopback. The caller needs `A2aServerUse` and `AgentUse`, and reaches only agents whose
//! `AgentDefinition` has `exposeA2a` and serves `a2a`. Every task lookup is owner-scoped: another user's task is not
//! found. A write the agent wants waits for its human owner: the task goes to `TASK_STATE_AUTH_REQUIRED` with a link
//! the owner opens in Moqui to decide, and resumes on its own afterwards; a remote caller can never approve.
//! A blocking send's wait, a cancel's wait and a stream run on a thread of their own ([`Answer`]), not on a request
//! worker; the host caps them per node and per user.

pub mod card;
pub mod client;
pub mod jsonrpc;
pub mod ops;
pub mod push;
pub mod rest;
pub mod types;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use serde_json::{json, Value as Json};

use crate::context::{ErrorKind, ExecutionContextFactory};
pub use types::A2aError;

/// An HTTP request to `/a2a` as the host hands it over.
#[derive(Clone, Debug, Default)]
pub struct HttpRequest {
    pub method: String,
    /// The path after `/a2a`: empty or `/` for JSON-RPC, else an HTTP+JSON path such as `/message:send`.
    pub path: String,
    pub query: Vec<(String, String)>,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Credentials may be taken: TLS, a believed proxy's https, or loopback.
    pub secure: bool,
    pub client_ip: String,
    pub origin: Origin,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn query_value(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Where the request came in, for the URLs the server advertises ([`card::base_url`]).
#[derive(Clone, Debug, Default)]
pub struct Origin {
    /// Over TLS or a believed proxy's https.
    pub https: bool,
    /// The peer is a proxy whose forwarding headers are believed (`client-ip-header` and its trusted proxies): only
    /// then are `X-Forwarded-Proto` and `X-Forwarded-Host` read, and only with `trust-forwarded-headers`.
    pub believed: bool,
    pub host: Option<String>,
    pub forwarded_proto: Option<String>,
    pub forwarded_host: Option<String>,
}

/// Whether credentials may be taken from a request: over TLS or a believed proxy's https (`https`), or from loopback.
/// Plain HTTP from anyone else is refused before a password or key is looked at.
pub fn secure(https: bool, peer: std::net::IpAddr) -> bool {
    https || peer.is_loopback()
}

/// A whole response.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn json(status: u16, body: &Json) -> Self {
        Self {
            status,
            headers: vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("cache-control".to_string(), "no-store".to_string()),
            ],
            body: body.to_string(),
        }
    }

    #[must_use]
    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }
}

/// A frame sink of a stream: `Some(text)` is one SSE frame, `None` a keep-alive comment; false when the client is
/// gone.
pub type Sink<'a> = &'a mut dyn FnMut(Option<&str>) -> bool;

/// What the host does with a request.
pub enum Answer {
    Reply(Reply),
    /// The response comes from `finish`, on a thread of the host's own (a blocking send's or a cancel's wait). The
    /// host counts it against `user_id`'s waits and streams.
    Later {
        user_id: String,
        finish: Box<dyn FnOnce() -> Reply + Send>,
    },
    /// A `text/event-stream` written by `write` on a thread of the host's own, until it returns; counted as a wait.
    Stream {
        user_id: String,
        write: Box<dyn FnOnce(Sink<'_>) + Send>,
    },
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Answer::Reply(reply) => f.debug_tuple("Reply").field(reply).finish(),
            Answer::Later { .. } => f.write_str("Later"),
            Answer::Stream { .. } => f.write_str("Stream"),
        }
    }
}

/// Whether the server is on: the `llm-facade` and its `a2a` are both enabled.
pub fn enabled(factory: &ExecutionContextFactory) -> bool {
    let llm = crate::llm::facade(factory);
    llm.enabled() && llm.conf().a2a.enabled
}

/// A request's user, authenticated, and the base URL the server is reached at.
#[derive(Clone, Debug)]
pub struct Caller {
    pub user_id: String,
    pub base_url: String,
}

/// The plain error of a request refused before either binding reads it, in the HTTP+JSON shape.
fn refusal(status: u16, name: &str, message: &str) -> Reply {
    Reply::json(
        status,
        &json!({"error": {"code": status, "status": name, "message": message}}),
    )
}

/// What the credentials of a request came to ([`log_in`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Login {
    /// No `Authorization`, `api_key` or `login_key` header.
    None,
    /// Credentials over plain HTTP from anyone but loopback: refused before they are looked at.
    Insecure,
    /// They are not valid.
    Invalid,
    /// The user they logged in, in the context in place.
    User(String),
}

/// Log the request's user in from its headers, as the A2A and MCP servers take them: `Authorization: Bearer <login
/// key>` (when `bearer`), HTTP Basic with an app password or a login key as the password (never the account
/// password), or the `api_key`/`login_key` header (when `api_key`); only over TLS, a believed proxy's https or
/// loopback.
pub(crate) fn log_in(request: &HttpRequest, bearer: bool, api_key: bool) -> Login {
    let authorization = request
        .header("authorization")
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let key = request
        .header("api_key")
        .or_else(|| request.header("login_key"))
        .map(str::trim)
        .filter(|key| !key.is_empty());
    if authorization.is_none() && key.is_none() {
        return Login::None;
    }
    if !request.secure {
        return Login::Insecure;
    }
    let logged_in = crate::user::with_client_ip(&request.client_ip, || -> bool {
        if let Some(header) = authorization {
            if let Some(token) = header
                .strip_prefix("Bearer ")
                .or_else(|| header.strip_prefix("bearer "))
            {
                return bearer && crate::user::login_user_key(token.trim()).is_ok();
            }
            if let Some((username, password)) = crate::web::basic_credentials(header) {
                if crate::user::looks_like_app_password(&password)
                    && crate::user::login_app_password(&username, &password).is_ok()
                {
                    return true;
                }
                // a login key as the password, as git hosts take personal access tokens; never the account password
                if password.len() == 40 && crate::user::login_user_key(&password).is_ok() {
                    let user = crate::user::current_username().ok().flatten().unwrap_or_default();
                    if username.is_empty() || user.eq_ignore_ascii_case(&username) {
                        return true;
                    }
                    let _ = crate::user::logout();
                }
                return false;
            }
            return false;
        }
        key.is_some_and(|key| api_key && crate::user::login_user_key(key).is_ok())
    });
    let user_id = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty());
    match user_id.filter(|_| logged_in) {
        Some(user_id) => Login::User(user_id),
        None => Login::Invalid,
    }
}

/// Log the request's user in from its headers ([`log_in`]).
///
/// # Errors
///
/// Returns the reply to send: 401 (with `WWW-Authenticate`) without valid credentials, 403 over plain HTTP or
/// without the permissions.
pub fn authenticate(factory: &ExecutionContextFactory, request: &HttpRequest) -> Result<Caller, Reply> {
    let conf = crate::llm::facade(factory).conf().a2a.clone();
    let unauthorized = |message: &str| {
        refusal(401, "UNAUTHENTICATED", message)
            .with_header("www-authenticate", "Bearer realm=\"a2a\", Basic realm=\"a2a\"")
    };
    let user_id = match log_in(request, conf.security.bearer, conf.security.api_key) {
        Login::User(user_id) => user_id,
        Login::None => {
            return Err(unauthorized(
                "credentials are required: a Bearer login key, or Basic with an app password",
            ))
        }
        Login::Insecure => {
            return Err(refusal(
                403,
                "PERMISSION_DENIED",
                "credentials over plain HTTP are refused; use HTTPS",
            ))
        }
        Login::Invalid => return Err(unauthorized("the credentials are not valid")),
    };
    let permitted = |permission: &str| crate::user::has_permission(permission).unwrap_or(false);
    if !permitted("A2aServerUse") || !permitted("AgentUse") {
        return Err(refusal(
            403,
            "PERMISSION_DENIED",
            "this needs the A2aServerUse and AgentUse permissions",
        ));
    }
    Ok(Caller {
        user_id,
        base_url: card::base_url(factory, &request.origin),
    })
}

/// Serve a request to `/a2a` or under it, on a request worker: JSON-RPC at the root, HTTP+JSON below it. 404 while
/// the server is off.
pub fn serve(factory: &Arc<ExecutionContextFactory>, request: &HttpRequest) -> Answer {
    if !enabled(factory) {
        return Answer::Reply(refusal(404, "NOT_FOUND", "the A2A server is off"));
    }
    let caller = match authenticate(factory, request) {
        Ok(caller) => caller,
        Err(reply) => return Answer::Reply(reply),
    };
    if request.path.is_empty() || request.path == "/" {
        jsonrpc::serve(factory, &caller, request)
    } else {
        rest::serve(factory, &caller, request)
    }
}

/// An error of an operation as the caller may read it: a tarpit is 429 with its seconds (the plain shape, before
/// either binding's), anything that is not an A2A error is logged and answered as an internal error.
pub(crate) enum Failure {
    A2a(A2aError),
    Reply(Reply),
}

impl From<A2aError> for Failure {
    fn from(error: A2aError) -> Self {
        Failure::A2a(error)
    }
}

impl Failure {
    /// A failure of the framework: a tarpit or an unavailable pool is said; anything else is logged with `what` and
    /// is "Internal error" to the caller.
    pub(crate) fn of(error: &moqui_lib::error::Error, what: &str) -> Self {
        match ErrorKind::of(error) {
            ErrorKind::Tarpit => {
                let seconds = retry_after_of(error).unwrap_or(60);
                Failure::Reply(
                    refusal(429, "RESOURCE_EXHAUSTED", "too many agent runs; try again later")
                        .with_header("retry-after", seconds.to_string()),
                )
            }
            ErrorKind::Unavailable => Failure::Reply(
                refusal(503, "UNAVAILABLE", "the server is busy; try again").with_header("retry-after", "5"),
            ),
            _ => {
                tracing::error!("A2A {what} failed: {error}");
                Failure::A2a(A2aError::internal())
            }
        }
    }
}

/// How a binding writes a unary result or error.
pub(crate) type Wrap = Box<dyn Fn(Result<Json, Failure>) -> Reply + Send>;

/// The answer to a non-streaming send: `{task}` now when the caller asked for it at once or the task waits for no one
/// here, else after the task ends or pauses, up to `a2a blocking-wait`, waited for on a thread of its own.
pub(crate) fn send_answer(
    factory: &Arc<ExecutionContextFactory>,
    caller: &Caller,
    params: &Json,
    wrap: Wrap,
) -> Answer {
    let ctx = match ops::Ctx::new(factory, caller) {
        Ok(ctx) => ctx,
        Err(error) => return Answer::Reply(wrap(Err(error.into()))),
    };
    let sent = match ops::send(&ctx, params) {
        Ok(sent) => sent,
        Err(failure) => return Answer::Reply(wrap(Err(failure))),
    };
    let history = sent.config.history_length;
    if ops::answers_now(&ctx, &sent) {
        let task = ops::task_json(&ctx.facade, &caller.base_url, &sent.task, history)
            .map(|task| json!({"task": task}))
            .map_err(|error| Failure::of(&error, "SendMessage"));
        return Answer::Reply(wrap(task));
    }
    let factory = Arc::clone(factory);
    let base_url = caller.base_url.clone();
    let wait = std::time::Duration::from_secs(crate::llm::facade(&factory).conf().a2a.blocking_wait.max(1));
    let user_id = caller.user_id.clone();
    Answer::Later {
        user_id,
        finish: Box::new(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            let task = ops::wait_settled(&factory, &sent.task, &base_url, wait, &|| false).and_then(|task| {
                let facade = crate::llm::run::facade_of(&factory)?;
                match task {
                    Some(task) => ops::task_json(&facade, &base_url, &task, history),
                    None => Ok(Json::Null),
                }
            });
            wrap(
                task.map(|task| json!({"task": task}))
                    .map_err(|error| Failure::of(&error, "SendMessage")),
            )
        }),
    }
}

/// The answer to `CancelTask`: the run is asked to stop on the request worker, and the task comes back once it has
/// stopped (or after a short wait, as it then stands), waited for on a thread of its own.
pub(crate) fn cancel_answer(
    factory: &Arc<ExecutionContextFactory>,
    caller: &Caller,
    params: &Json,
    wrap: Wrap,
) -> Answer {
    let ctx = match ops::Ctx::new(factory, caller) {
        Ok(ctx) => ctx,
        Err(error) => return Answer::Reply(wrap(Err(error.into()))),
    };
    let task = match ops::cancel(&ctx, params) {
        Ok(task) => task,
        Err(failure) => return Answer::Reply(wrap(Err(failure))),
    };
    let factory = Arc::clone(factory);
    let base_url = caller.base_url.clone();
    Answer::Later {
        user_id: caller.user_id.clone(),
        finish: Box::new(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            let task = ops::wait_canceled(&factory, &task, &base_url);
            wrap(task.map_err(|error| Failure::of(&error, "CancelTask")))
        }),
    }
}

/// The answer of a stream of `task`: its snapshot (`task`) first, then its updates from where the snapshot stands,
/// each a frame `frame` makes of a `StreamResponse`. `close_on_pause` ends the stream when the task pauses; a task
/// that has ended, or pauses as the stream starts, gets its snapshot only.
pub(crate) fn stream_answer(
    factory: &Arc<ExecutionContextFactory>,
    caller: &Caller,
    task: ops::TaskRow,
    history: Option<usize>,
    close_on_pause: bool,
    frame: impl Fn(&Json) -> String + Send + 'static,
) -> Result<Answer, Failure> {
    let facade = crate::llm::run::facade_of(factory).map_err(|error| Failure::of(&error, "a stream"))?;
    let first =
        ops::task_json(&facade, &caller.base_url, &task, history).map_err(|error| Failure::of(&error, "a stream"))?;
    let factory = Arc::clone(factory);
    let base_url = caller.base_url.clone();
    Ok(Answer::Stream {
        user_id: caller.user_id.clone(),
        write: Box::new(move |sink: Sink<'_>| {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            if !sink(Some(&frame(&json!({"task": first})))) {
                return;
            }
            if task.run.state.is_terminal() || (task.run.state.is_paused() && close_on_pause) {
                return;
            }
            let follow = ops::Follow::of(&task, &base_url, close_on_pause, true);
            let mut emit = |response: Option<Json>| match response {
                Some(response) => sink(Some(&frame(&response))),
                None => sink(None),
            };
            if let Err(error) = ops::follow(&factory, &follow, None, &|| false, &mut emit) {
                tracing::error!("the A2A stream of task {} ended on an error: {error}", task.id());
            }
        }),
    })
}

/// Whether a stream of `task` ends when it pauses: a subscriber of a task that waits for its owner's approval or hand-off
/// follows it through the decision; every other stream ends at a pause, a remote's link (an Auth pause, which waits for
/// the caller's Done or Cancel) included.
pub(crate) fn closes_on_pause(task: &ops::TaskRow) -> bool {
    !(task.run.state == crate::llm::run::RunState::AuthRequired
        && matches!(
            task.run.pause_kind,
            Some(crate::llm::tools::PauseKind::Approval | crate::llm::tools::PauseKind::Handoff)
        ))
}

/// The seconds a tarpit's refusal says to wait.
pub(crate) fn retry_after_of(error: &moqui_lib::error::Error) -> Option<u64> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(found) = current {
        if let Some(seconds) = found
            .downcast_ref::<crate::context::Error>()
            .and_then(crate::context::Error::retry_after)
        {
            return Some(seconds);
        }
        current = found.source();
    }
    None
}
