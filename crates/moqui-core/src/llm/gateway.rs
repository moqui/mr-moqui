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

//! The OpenAI-compatible gateway (plans/LLM-A2A-Assist.md 4.17, D10; docs/Gateway.md), off by default (`llm-facade`
//! `gateway enabled`): internal tools and scripts call the configured models at `/llm/v1` with their own keys, under
//! the central keys, quotas and usage log.
//!
//! - `GET /models` lists the aliases the caller may use, `GET /models/{alias}` one of them;
//!   `POST /chat/completions` (streaming or not), `POST /responses` and `POST /embeddings` pass a request through to
//!   the alias's model. An alias (`gateway/alias` in the conf, an `LlmModelAlias` row over it) names a model or a
//!   feature whose binding it follows; the request's `model` must be one, else 404 `model_not_found`.
//! - Chat Completions goes only to a `chat-completions` endpoint and Responses only to a `responses` one; embeddings to
//!   either (their `/v1/embeddings`). The upstream body is the request's allowlisted fields ([`Kind::fields`]; any other
//!   is 400 `unsupported_parameter`, so nothing the caller sends reaches another API or a base URL), with the model's
//!   patches over them ([`provider::apply_model_patches`]), `n` only 1, the output limits clamped to the model's
//!   `max-output`, `model` set to the provider's name, and the endpoint's key and headers; the caller's headers are never
//!   sent on. A stream gets `stream_options.include_usage` (Chat Completions) so its usage is known; the usage chunk is
//!   passed on only when the caller asked for it. Responses is sent with `store: false`.
//! - No tool runs here: function tool definitions pass through to the model and its tool calls back to the caller; any
//!   other tool type (hosted search, code interpreter, MCP) is 400.
//! - Auth is `Authorization: Bearer <login key or app password>` (what OpenAI clients send) or HTTP Basic with an app
//!   password, over TLS, a believed proxy's https or loopback; the user needs `LlmGatewayUse`. An app password (made
//!   for DAV, SFTP or git) opens the gateway only when an `LlmApiClient` with the `gateway` scope names it, whatever
//!   `require-client` says; with `require-client` (the default) a login key needs one too. The client's
//!   `allowedAliases` and limits then apply.
//! - Before a call: the user's quota for the `gateway` scope and the client's daily tokens and cost, each with the
//!   most the call may use (its prompt estimated and its output limit), 429 `insufficient_quota` with `Retry-After`
//!   until the day or month ends; then the host's room for one more open call ([`Open`]); then the requests per minute
//!   of the user's `LlmQuota` and of the client ([`usage::take_request`], per node) and the AT_LLM tarpit
//!   (`gateway:<alias>`), each 429. A refusal of the host's takes no count.
//! - A call runs on a thread of the host's own ([`Answer::Later`]), not a request worker; the endpoint's slots limit it
//!   as an agent's call. Each upstream request is one `LlmCall` of source `gateway` with the user and the client; when
//!   the model's usage did not come (a stream cut short), it is estimated and marked `usageEstimated`, and counts toward
//!   the quotas as any usage. A stream is read through the pump and written on as it comes, each write within the idle
//!   limit and the call's deadline; when the caller goes away or stops reading, the upstream request is dropped and the
//!   call is logged as canceled, so a stalled caller holds no endpoint slot. A stream that fails upstream ends with an
//!   error frame, not `[DONE]`. The model server's errors reach the caller as generic messages; the detail is in the log
//!   and in `LlmCall`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use rust_decimal::Decimal;
use serde_json::{json, Map, Value as Json};

pub use super::a2a::{HttpRequest, Reply};
use super::conf::{GatewayConf, ProtocolId, ReasoningLevel};
use super::provider::{self, pump, sse, CancelToken, ErrorClass, ProviderError, ProviderEvent};
use super::registry::{Registry, Target};
use super::usage::{self, CallRecord, CallSource, Reserve, Usage};
use super::{decimal_of, flag_of, int_of, text_of};
use crate::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::service::rust_services::{rust_service, RustServiceFn};

const LLM_MODEL_ALIAS: &str = "moqui.llm.LlmModelAlias";
const LLM_API_CLIENT: &str = "moqui.llm.LlmApiClient";

/// The content log keeps at most this much of a stream.
const MAX_LOGGED_STREAM: usize = 1024 * 1024;

/// The output limit fields of the APIs, clamped to the model's `max-output`.
const OUTPUT_LIMITS: [&str; 3] = ["max_tokens", "max_completion_tokens", "max_output_tokens"];

/// How the host answers a call: a [`Reply`] now, or a reply or a stream from a thread of its own.
pub trait Respond {
    /// Answer with a whole reply.
    fn reply(&mut self, reply: Reply);
    /// Start a `200 text/event-stream`; false when the caller is gone.
    fn start(&mut self) -> bool;
    /// Write one piece of the stream by `deadline`; false when the caller is gone or did not take it in time.
    fn send(&mut self, text: &str, deadline: Instant) -> bool;
    /// Set by the host when the caller goes away.
    fn gone(&self) -> CancelToken;
}

/// What the host holds for an open call (its count of open calls), dropped when the call ends.
pub type Held = Box<dyn Send>;

/// The host's room for one more open call of a user: what it holds, or its refusal (503 or 429).
pub type Open<'a> = &'a dyn Fn(&str) -> Result<Held, Reply>;

/// A call's work on the host's thread.
pub type Work = Box<dyn FnOnce(&mut dyn Respond) + Send>;

/// What the host does with a request.
pub enum Answer {
    Reply(Reply),
    /// `work` answers through its [`Respond`], on a thread of the host's own, which keeps `held` until it ends.
    Later { held: Held, work: Work },
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Answer::Reply(reply) => f.debug_tuple("Reply").field(reply).finish(),
            Answer::Later { .. } => f.write_str("Later"),
        }
    }
}

/// An error in the OpenAI shape: `{"error": {"message", "type", "param", "code"}}`.
pub fn error(status: u16, kind: &str, code: Option<&str>, param: Option<&str>, message: &str) -> Reply {
    Reply::json(
        status,
        &json!({"error": {"message": message, "type": kind, "param": param, "code": code}}),
    )
}

fn invalid(param: Option<&str>, code: &str, message: &str) -> Reply {
    error(400, "invalid_request_error", Some(code), param, message)
}

fn too_many(code: &str, message: &str, seconds: u64) -> Reply {
    error(429, if code == "insufficient_quota" { code } else { "requests" }, Some(code), None, message)
        .with_header("retry-after", seconds.max(1).to_string())
}

/// Whether the gateway is on: the `llm-facade` and its `gateway` are both enabled.
pub fn enabled(factory: &ExecutionContextFactory) -> bool {
    let llm = super::facade(factory);
    llm.enabled() && llm.conf().gateway.enabled
}

/// The APIs the gateway passes through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Chat,
    Responses,
    Embeddings,
}

impl Kind {
    /// The request fields passed on; any other is refused.
    pub fn fields(self) -> &'static [&'static str] {
        match self {
            Kind::Chat => &[
                "model",
                "messages",
                "stream",
                "stream_options",
                "temperature",
                "top_p",
                "n",
                "stop",
                "max_tokens",
                "max_completion_tokens",
                "presence_penalty",
                "frequency_penalty",
                "logit_bias",
                "logprobs",
                "top_logprobs",
                "seed",
                "response_format",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "reasoning_effort",
                "user",
            ],
            Kind::Responses => &[
                "model",
                "input",
                "instructions",
                "stream",
                "temperature",
                "top_p",
                "max_output_tokens",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "text",
                "reasoning",
                "truncation",
                "include",
                "store",
                "user",
            ],
            Kind::Embeddings => &["model", "input", "encoding_format", "dimensions", "user"],
        }
    }

    fn path(self) -> &'static str {
        match self {
            Kind::Chat => "/chat/completions",
            Kind::Responses => "/responses",
            Kind::Embeddings => "/embeddings",
        }
    }

    /// Whether an endpoint of `protocol` takes this API.
    fn takes(self, protocol: ProtocolId) -> bool {
        match self {
            Kind::Chat => protocol == ProtocolId::ChatCompletions,
            Kind::Responses => protocol == ProtocolId::Responses,
            Kind::Embeddings => matches!(protocol, ProtocolId::ChatCompletions | ProtocolId::Responses),
        }
    }

    /// The fields whose text the model reads: the prompt, for the estimate of its tokens.
    fn prompt_fields(self) -> &'static [&'static str] {
        match self {
            Kind::Chat => &["messages", "tools"],
            Kind::Responses => &["input", "instructions", "tools"],
            Kind::Embeddings => &["input"],
        }
    }
}

/// A request's checked fields, before its alias is resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub kind: Kind,
    pub alias: String,
    pub stream: bool,
    /// The caller asked for the usage chunk of a Chat Completions stream.
    pub include_usage: bool,
    /// The reasoning level the request asks for (`reasoning_effort`, `reasoning.effort`).
    pub level: Option<ReasoningLevel>,
    /// The fields passed on, without `model`, `stream` and `stream_options`.
    pub fields: Map<String, Json>,
    /// The characters of the prompt's text (the strings of its messages or input, its instructions, and its tools).
    pub prompt_chars: usize,
}

impl Prepared {
    /// The output limit the request asks for, at most the model's `max-output`; else the model's `max-output`, else 0.
    pub fn max_output(&self, target: &Target) -> u64 {
        let asked = OUTPUT_LIMITS
            .iter()
            .filter_map(|key| self.fields.get(*key).and_then(Json::as_u64))
            .max();
        let model = target.model.max_output.map(u64::from);
        match (asked, model) {
            (Some(asked), Some(model)) => asked.min(model),
            (asked, model) => asked.or(model).unwrap_or(0),
        }
    }

    /// The prompt's tokens, estimated as the model's calibration counts characters.
    pub fn prompt_tokens(&self, target: &Target) -> u64 {
        super::tokens::Estimator::for_model(&target.model).tokens_of_chars(self.prompt_chars)
    }

    /// The most the call may use: its prompt and its output limit, with their cost at the model's prices.
    pub fn reserve(&self, target: &Target) -> Reserve {
        let usage = Usage {
            input: self.prompt_tokens(target),
            output: if self.kind == Kind::Embeddings { 0 } else { self.max_output(target) },
            ..Usage::default()
        };
        Reserve {
            tokens: usage.total(),
            cost: usage::cost(&target.model.prices, &usage).unwrap_or_default(),
        }
    }
}

/// The characters of the strings in `value`.
fn string_chars(value: &Json) -> usize {
    match value {
        Json::String(text) => text.chars().count(),
        Json::Array(items) => items.iter().map(string_chars).sum(),
        Json::Object(map) => map.values().map(string_chars).sum(),
        _ => 0,
    }
}

/// An OpenAI effort as a level: `minimal` is low, `xhigh` max.
fn level_of(effort: Option<&Json>) -> Option<ReasoningLevel> {
    match effort?.as_str()? {
        "minimal" => Some(ReasoningLevel::Low),
        "xhigh" => Some(ReasoningLevel::Max),
        other => ReasoningLevel::parse(other),
    }
}

/// Function tools only, each with a name: no tool runs on the server.
fn check_tools(kind: Kind, tools: &Json) -> Result<(), Reply> {
    let tools = tools
        .as_array()
        .ok_or_else(|| invalid(Some("tools"), "invalid_type", "tools must be a list"))?;
    for (at, tool) in tools.iter().enumerate() {
        let kind_of = tool.get("type").and_then(Json::as_str).unwrap_or_default();
        if kind_of != "function" {
            return Err(invalid(
                Some("tools"),
                "unsupported_tool",
                &format!(
                    "tools[{at}] is of type {kind_of:?}: the gateway passes only function tools to the model, and runs no tool itself"
                ),
            ));
        }
        let name = match kind {
            Kind::Chat => tool.pointer("/function/name"),
            _ => tool.get("name"),
        };
        if !name.and_then(Json::as_str).is_some_and(|name| !name.is_empty()) {
            return Err(invalid(Some("tools"), "invalid_value", &format!("tools[{at}] has no function name")));
        }
    }
    Ok(())
}

/// Check a request body for `kind`: a JSON object of allowlisted fields with a `model` and the content the API needs,
/// one choice (`n` 1), and output limits that are whole numbers.
///
/// # Errors
///
/// Returns the 400 reply naming what is wrong.
pub fn prepare(kind: Kind, body: &[u8]) -> Result<Prepared, Reply> {
    let body: Json = serde_json::from_slice(body)
        .map_err(|error| invalid(None, "invalid_json", &format!("the request body is not JSON: {error}")))?;
    let Json::Object(mut fields) = body else {
        return Err(invalid(None, "invalid_json", "the request body must be a JSON object"));
    };
    if let Some(field) = fields.keys().find(|key| !kind.fields().contains(&key.as_str())) {
        let why = match field.as_str() {
            "previous_response_id" | "conversation" | "background" | "prompt" => {
                " (the gateway sends store: false, so there is no stored response or conversation to refer to)"
            }
            _ => "",
        };
        return Err(invalid(
            Some(field),
            "unsupported_parameter",
            &format!("{field} is not passed through by the gateway{why}"),
        ));
    }
    let alias = match fields.remove("model") {
        Some(Json::String(alias)) if !alias.trim().is_empty() => alias.trim().to_string(),
        _ => return Err(invalid(Some("model"), "missing_required_parameter", "model must name a model")),
    };
    let stream = match fields.remove("stream") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(stream)) => stream,
        Some(_) => return Err(invalid(Some("stream"), "invalid_type", "stream must be true or false")),
    };
    if stream && kind == Kind::Embeddings {
        return Err(invalid(Some("stream"), "unsupported_parameter", "embeddings do not stream"));
    }
    let mut include_usage = false;
    match fields.remove("stream_options") {
        None | Some(Json::Null) => {}
        Some(Json::Object(options)) => {
            if let Some(key) = options.keys().find(|key| *key != "include_usage") {
                return Err(invalid(
                    Some("stream_options"),
                    "unsupported_parameter",
                    &format!("stream_options.{key} is not passed through by the gateway"),
                ));
            }
            include_usage = options.get("include_usage").and_then(Json::as_bool).unwrap_or(false);
        }
        Some(_) => return Err(invalid(Some("stream_options"), "invalid_type", "stream_options must be an object")),
    }
    if fields.get("n").is_some_and(|n| !n.is_null() && n.as_u64() != Some(1)) {
        return Err(invalid(
            Some("n"),
            "unsupported_value",
            "the gateway makes one choice a call: n must be 1 (make more calls for more choices)",
        ));
    }
    for key in OUTPUT_LIMITS {
        if fields.get(key).is_some_and(|limit| !limit.is_null() && !limit.as_u64().is_some_and(|limit| limit > 0)) {
            return Err(invalid(Some(key), "invalid_value", &format!("{key} must be a whole number above 0")));
        }
    }
    match kind {
        Kind::Chat => {
            let messages = fields.get("messages").and_then(Json::as_array);
            if !messages.is_some_and(|messages| !messages.is_empty() && messages.iter().all(Json::is_object)) {
                return Err(invalid(
                    Some("messages"),
                    "invalid_value",
                    "messages must be a list of message objects with at least one",
                ));
            }
        }
        Kind::Responses => {
            if !fields.get("input").is_some_and(|input| input.is_string() || input.is_array()) {
                return Err(invalid(Some("input"), "invalid_value", "input must be text or a list of items"));
            }
            if fields.get("store").is_some_and(|store| store != &Json::Bool(false)) {
                return Err(invalid(Some("store"), "invalid_value", "the gateway sends store: false only"));
            }
            if let Some(include) = fields.get("include") {
                let allowed = include.as_array().is_some_and(|items| {
                    items
                        .iter()
                        .all(|item| item.as_str() == Some("reasoning.encrypted_content"))
                });
                if !allowed {
                    return Err(invalid(
                        Some("include"),
                        "invalid_value",
                        "include may only list reasoning.encrypted_content",
                    ));
                }
            }
        }
        Kind::Embeddings => {
            if !fields.get("input").is_some_and(|input| input.is_string() || input.is_array()) {
                return Err(invalid(Some("input"), "invalid_value", "input must be text or a list"));
            }
        }
    }
    if let Some(tools) = fields.get("tools") {
        check_tools(kind, tools)?;
    }
    let level = match kind {
        Kind::Chat => level_of(fields.get("reasoning_effort")),
        Kind::Responses => level_of(fields.get("reasoning").and_then(|reasoning| reasoning.get("effort"))),
        Kind::Embeddings => None,
    };
    let prompt_chars = kind
        .prompt_fields()
        .iter()
        .filter_map(|key| Some((*key, fields.get(*key)?)))
        .map(|(key, value)| if key == "tools" { value.to_string().len() } else { string_chars(value) })
        .sum();
    Ok(Prepared {
        kind,
        alias,
        stream,
        include_usage,
        level,
        fields,
        prompt_chars,
    })
}

/// The body sent upstream: the request's fields with the model's `api-config` and its patch for `level` merged over
/// them ([`provider::apply_model_patches`], which keeps the content fields as they are), the output limits clamped to
/// the model's `max-output`, `model` the provider's name, and the stream settings.
pub fn upstream_body(prepared: &Prepared, target: &Target, level: Option<ReasoningLevel>) -> Json {
    let mut body = Json::Object(prepared.fields.clone());
    provider::apply_model_patches(&mut body, target, level);
    let Json::Object(mut body) = body else {
        return Json::Null;
    };
    if let Some(max) = target.model.max_output.map(u64::from) {
        for key in OUTPUT_LIMITS {
            if body.get(key).and_then(Json::as_u64).is_some_and(|limit| limit > max) {
                body.insert(key.to_string(), json!(max));
            }
        }
    }
    body.insert("model".to_string(), Json::String(target.model.name.clone()));
    match prepared.kind {
        Kind::Chat => {
            body.insert("stream".to_string(), Json::Bool(prepared.stream));
            // the gateway's own, whatever a patch says: the usage of a stream must come
            if prepared.stream {
                body.insert("stream_options".to_string(), json!({"include_usage": true}));
            } else {
                body.remove("stream_options");
            }
        }
        Kind::Responses => {
            body.insert("stream".to_string(), Json::Bool(prepared.stream));
            body.insert("store".to_string(), Json::Bool(false));
        }
        Kind::Embeddings => {}
    }
    Json::Object(body)
}

/// A model name the gateway answers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub name: String,
    pub model: Option<String>,
    pub feature: Option<String>,
    pub description: Option<String>,
}

/// The aliases in effect: the conf's, with the `LlmModelAlias` rows over them by name (a disabled row turns one off). A
/// row with neither a model nor a feature is logged and left out.
pub fn aliases(conf: &GatewayConf, facade: &EntityFacade) -> Vec<Alias> {
    let mut out: Vec<Alias> = conf
        .aliases
        .iter()
        .map(|alias| Alias {
            name: alias.name.clone(),
            model: alias.model.clone(),
            feature: alias.feature.clone(),
            description: alias.description.clone(),
        })
        .collect();
    let rows = facade
        .find(LLM_MODEL_ALIAS)
        .and_then(|find| find.use_cache(true).disable_authz().list());
    match rows {
        Ok(rows) => {
            for row in rows.iter() {
                let Some(name) = text_of(row, "alias") else {
                    continue;
                };
                out.retain(|alias| alias.name != name);
                if flag_of(row, "disabled") == Some(true) {
                    continue;
                }
                let (model, feature) = (text_of(row, "modelId"), text_of(row, "featureId"));
                if model.is_none() && feature.is_none() {
                    tracing::error!("LlmModelAlias {name} names neither a model nor a feature; it is left out");
                    continue;
                }
                out.push(Alias {
                    name,
                    model,
                    feature,
                    description: text_of(row, "description"),
                });
            }
        }
        Err(error) => tracing::error!("the LlmModelAlias rows could not be read, so only the conf's aliases apply: {error}"),
    }
    out.sort_by(|left, right| left.name.cmp(&right.name));
    out
}

impl Alias {
    /// Where a call to this alias goes, and the reasoning level of its feature's binding.
    ///
    /// # Errors
    ///
    /// Returns why the model or the feature cannot take a call.
    pub fn target(&self, registry: &Registry) -> Result<(Target, Option<ReasoningLevel>), Error> {
        match (&self.model, &self.feature) {
            (Some(model), _) => Ok((registry.resolve_model(model, ReasoningLevel::None)?, None)),
            (None, Some(feature)) => {
                let target = registry.resolve_feature(feature)?;
                let level = target.reasoning;
                Ok((target, Some(level)))
            }
            (None, None) => Err(Error::new(format!("alias {} names no model", self.name))),
        }
    }
}

/// An `LlmApiClient` a call came through.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Client {
    pub client_id: String,
    /// Empty is every alias the user may use.
    pub aliases: Vec<String>,
    pub daily_tokens: Option<u64>,
    pub daily_cost: Option<Decimal>,
    pub requests_per_minute: Option<u32>,
}

impl Client {
    fn allows(&self, alias: &str) -> bool {
        self.aliases.is_empty() || self.aliases.iter().any(|allowed| allowed == alias)
    }
}

/// A request's user, authenticated, and its client.
#[derive(Clone, Debug)]
pub struct Caller {
    pub user_id: String,
    pub client: Option<Client>,
}

impl Caller {
    fn allows(&self, alias: &str) -> bool {
        self.client.as_ref().is_none_or(|client| client.allows(alias))
    }
}

/// The credential a request logged in with, as an `LlmApiClient` names it.
enum Credential {
    /// The hash of a login key, as `UserLoginKey.loginKey` stores it.
    LoginKey(String),
    AppPassword(String),
}

/// The id and the username of the owner of an app password, by its hash.
fn app_password_owner(facade: &EntityFacade, password: &str) -> Option<(String, String)> {
    let hashed = crate::user::hash_login_key(&crate::user::app_password_key(password)).ok()?;
    let row = facade
        .find("moqui.security.UserAppPassword")
        .ok()?
        .eq("passwordHash", Value::string(&hashed))
        .disable_authz()
        .one()
        .ok()??;
    let user = facade
        .find("moqui.security.UserAccount")
        .ok()?
        .eq("userId", Value::string(text_of(&row, "userId")?))
        .disable_authz()
        .one()
        .ok()??;
    Some((text_of(&row, "appPasswordId")?, text_of(&user, "username")?))
}

/// Log in with `password` as an app password; its id when that worked.
fn login_app_password(facade: &EntityFacade, username: Option<&str>, password: &str) -> Option<Credential> {
    match app_password_owner(facade, password) {
        Some((id, owner)) if username.is_none_or(|username| username.eq_ignore_ascii_case(&owner)) => {
            crate::user::login_app_password(&owner, password)
                .ok()
                .map(|()| Credential::AppPassword(id))
        }
        _ => {
            crate::web::note_auth_failure();
            None
        }
    }
}

fn login_key(key: &str) -> Option<Credential> {
    crate::user::login_user_key(key).ok()?;
    crate::user::hash_login_key(key).ok().map(Credential::LoginKey)
}

/// Log the request's user in from its headers, and find its client.
///
/// # Errors
///
/// Returns the reply to send: 401 (with `WWW-Authenticate`) without valid credentials, 403 over plain HTTP, without
/// `LlmGatewayUse`, for an app password no `LlmApiClient` of the gateway names, or (with `require-client`) a login key
/// none names.
pub fn authenticate(factory: &ExecutionContextFactory, facade: &EntityFacade, request: &HttpRequest) -> Result<Caller, Reply> {
    let conf = super::facade(factory).conf().gateway.clone();
    let unauthorized = |message: &str| {
        error(401, "invalid_request_error", Some("invalid_api_key"), None, message)
            .with_header("www-authenticate", "Bearer realm=\"llm\", Basic realm=\"llm\"")
    };
    let Some(authorization) = request
        .header("authorization")
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Err(unauthorized(
            "an API key is required: Authorization: Bearer <login key or app password>",
        ));
    };
    if !request.secure {
        return Err(error(
            403,
            "permission_error",
            Some("insecure_transport"),
            None,
            "credentials over plain HTTP are refused; use HTTPS",
        ));
    }
    let credential = crate::user::with_client_ip(&request.client_ip, || {
        if let Some(token) = authorization
            .strip_prefix("Bearer ")
            .or_else(|| authorization.strip_prefix("bearer "))
            .map(str::trim)
        {
            return if crate::user::looks_like_app_password(token) {
                login_app_password(facade, None, token)
            } else {
                login_key(token)
            };
        }
        let (username, password) = crate::web::basic_credentials(authorization)?;
        if crate::user::looks_like_app_password(&password) {
            return login_app_password(facade, Some(&username), &password);
        }
        // a login key as the password, as git hosts take personal access tokens; never the account password
        if password.len() == 40 {
            let credential = login_key(&password)?;
            let user = crate::user::current_username().ok().flatten().unwrap_or_default();
            if username.is_empty() || user.eq_ignore_ascii_case(&username) {
                return Some(credential);
            }
            let _ = crate::user::logout();
        }
        None
    });
    let user_id = crate::user::current_user_id().ok().flatten().filter(|id| !id.is_empty());
    let (Some(credential), Some(user_id)) = (credential, user_id) else {
        return Err(unauthorized("the API key is not valid"));
    };
    if !crate::user::has_permission("LlmGatewayUse").unwrap_or(false) {
        return Err(error(
            403,
            "permission_error",
            Some("permission_denied"),
            None,
            "this needs the LlmGatewayUse permission",
        ));
    }
    let client = client_of(facade, &credential, &user_id).map_err(|message| {
        error(403, "permission_error", Some("client_not_allowed"), None, &message)
    })?;
    if client.is_none() {
        if matches!(credential, Credential::AppPassword(_)) {
            return Err(error(
                403,
                "permission_error",
                Some("client_required"),
                None,
                "an app password opens the gateway only when an LlmApiClient with the gateway scope names it",
            ));
        }
        if conf.require_client {
            return Err(error(
                403,
                "permission_error",
                Some("client_required"),
                None,
                "this key is not one an LlmApiClient with the gateway scope names (the gateway's require-client is on)",
            ));
        }
    }
    Ok(Caller { user_id, client })
}

/// The comma list of a field.
fn list_of(row: &crate::entity::EntityValue, field: &str) -> Vec<String> {
    text_of(row, field)
        .map(|text| {
            text.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn has_gateway_scope(row: &crate::entity::EntityValue) -> bool {
    list_of(row, "scopes").iter().any(|scope| scope == "gateway")
}

/// The `LlmApiClient` of `credential` with the `gateway` scope, when one names it; its use is noted at most once a
/// minute. Rows of the credential for other scopes (A2A, MCP) are not the gateway's.
///
/// # Errors
///
/// Returns why the client may not use the gateway: disabled, another user's, rows of the credential with no gateway
/// scope, or two with it (ambiguous, which `store#LlmApiClient` refuses to make).
fn client_of(facade: &EntityFacade, credential: &Credential, user_id: &str) -> Result<Option<Client>, String> {
    let (field, value) = match credential {
        Credential::LoginKey(hash) => ("loginKey", hash),
        Credential::AppPassword(id) => ("appPasswordId", id),
    };
    let rows = facade
        .find(LLM_API_CLIENT)
        .and_then(|find| find.eq(field, Value::string(value)).disable_authz().list())
        .map_err(|error| {
            tracing::error!("the LlmApiClient of a gateway call could not be read: {error}");
            "the client could not be read".to_string()
        })?;
    let gateway: Vec<&crate::entity::EntityValue> = rows.iter().filter(|row| has_gateway_scope(row)).collect();
    let row = match gateway.as_slice() {
        [] if rows.iter().next().is_some() => {
            return Err("the LlmApiClient of this key does not have the gateway scope".to_string())
        }
        [] => return Ok(None),
        [row] => (*row).clone(),
        _ => {
            let ids: Vec<String> = gateway.iter().filter_map(|row| text_of(row, "clientId")).collect();
            tracing::error!("LlmApiClient rows {} name the same credential with the gateway scope; the call is refused", ids.join(", "));
            return Err("more than one LlmApiClient with the gateway scope names this key".to_string());
        }
    };
    let client_id = text_of(&row, "clientId").unwrap_or_default();
    if flag_of(&row, "disabled") == Some(true) {
        return Err(format!("client {client_id} is disabled"));
    }
    if text_of(&row, "userId").is_some_and(|owner| owner != user_id) {
        return Err(format!("client {client_id} is another user's"));
    }
    let now = jiff::Timestamp::now().as_millisecond();
    let used = row
        .get("lastUsedDate")
        .and_then(super::millis_of)
        .is_some_and(|used| now - used < 60_000);
    if !used {
        let noted = facade.make_value(LLM_API_CLIENT).and_then(|mut update| {
            update.set("clientId", Value::string(&client_id));
            update.set("lastUsedDate", crate::user::stamp_value(now));
            facade
                .transactions_for(LLM_API_CLIENT)?
                .run(crate::transaction::TransactionAttribute::ForceNew, None, || update.update(facade))
        });
        if let Err(error) = noted {
            tracing::warn!("client {client_id}: last use not written: {error}");
        }
    }
    Ok(Some(Client {
        aliases: list_of(&row, "allowedAliases"),
        daily_tokens: int_of(&row, "dailyTokens").and_then(|n| u64::try_from(n).ok()).filter(|n| *n > 0),
        daily_cost: decimal_of(&row, "dailyCost").filter(|cost| !cost.is_zero()),
        requests_per_minute: int_of(&row, "requestsPerMinute")
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0),
        client_id,
    }))
}

/// The routes under `/llm/v1`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Route {
    Models,
    Model(String),
    Call(Kind),
}

fn route(method: &str, path: &str) -> Result<Route, Reply> {
    let path = path.trim_end_matches('/');
    let (route, allowed) = match path {
        "/models" => (Route::Models, "GET"),
        "/chat/completions" => (Route::Call(Kind::Chat), "POST"),
        "/responses" => (Route::Call(Kind::Responses), "POST"),
        "/embeddings" => (Route::Call(Kind::Embeddings), "POST"),
        _ => match path.strip_prefix("/models/").filter(|id| !id.is_empty() && !id.contains('/')) {
            Some(id) => (Route::Model(id.to_string()), "GET"),
            None => {
                return Err(error(
                    404,
                    "invalid_request_error",
                    Some("unknown_url"),
                    None,
                    &format!("the gateway has no {path:?}; it serves /models, /chat/completions, /responses and /embeddings"),
                ))
            }
        },
    };
    if !method.eq_ignore_ascii_case(allowed) {
        return Err(error(405, "invalid_request_error", Some("method_not_allowed"), None, &format!("use {allowed}"))
            .with_header("allow", allowed));
    }
    Ok(route)
}

/// Serve a request under `/llm/v1` (`request.path` is the rest) on a request worker: authenticate it, check it, take
/// the host's room for it (`open`), and hand the model call to the host. 404 while the gateway is off.
pub fn serve(factory: &Arc<ExecutionContextFactory>, request: &HttpRequest, open: Open<'_>) -> Answer {
    if !enabled(factory) {
        return Answer::Reply(error(404, "invalid_request_error", Some("gateway_off"), None, "the LLM gateway is off"));
    }
    let route = match route(&request.method, &request.path) {
        Ok(route) => route,
        Err(reply) => return Answer::Reply(reply),
    };
    let facade = match super::run::facade_of(factory) {
        Ok(facade) => facade,
        Err(error) => return Answer::Reply(internal(&error, "a request")),
    };
    let caller = match authenticate(factory, &facade, request) {
        Ok(caller) => caller,
        Err(reply) => return Answer::Reply(reply),
    };
    let llm = super::facade(factory);
    let registry = llm.registry(&facade);
    let usable = |alias: &Alias| caller.allows(&alias.name) && alias.target(&registry).is_ok();
    match route {
        Route::Models => {
            let data: Vec<Json> = aliases(&llm.conf().gateway, &facade)
                .iter()
                .filter(|alias| usable(alias))
                .map(model_json)
                .collect();
            Answer::Reply(Reply::json(200, &json!({"object": "list", "data": data})))
        }
        Route::Model(id) => match aliases(&llm.conf().gateway, &facade)
            .iter()
            .find(|alias| alias.name == id && usable(alias))
        {
            Some(alias) => Answer::Reply(Reply::json(200, &model_json(alias))),
            None => Answer::Reply(not_found(&id)),
        },
        Route::Call(kind) => match call(factory, &facade, &registry, &caller, kind, &request.body, open) {
            Ok(answer) | Err(answer) => answer,
        },
    }
}

fn model_json(alias: &Alias) -> Json {
    json!({"id": alias.name, "object": "model", "created": 0, "owned_by": "moqui"})
}

fn not_found(alias: &str) -> Reply {
    error(
        404,
        "invalid_request_error",
        Some("model_not_found"),
        Some("model"),
        &format!("The model `{alias}` does not exist or you do not have access to it."),
    )
}

fn internal(error: &Error, what: &str) -> Reply {
    tracing::error!("the LLM gateway's {what} failed: {error}");
    self::error(500, "server_error", None, None, "Internal error")
}

/// Check a call and admit it: the alias, the API its endpoint takes, the quotas with the call's reserve, the host's
/// room, the request rates and the tarpit.
fn call(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    registry: &Registry,
    caller: &Caller,
    kind: Kind,
    body: &[u8],
    open: Open<'_>,
) -> Result<Answer, Answer> {
    let prepared = prepare(kind, body).map_err(Answer::Reply)?;
    let llm = super::facade(factory);
    let alias = aliases(&llm.conf().gateway, facade)
        .into_iter()
        .find(|alias| alias.name == prepared.alias && caller.allows(&alias.name))
        .ok_or_else(|| Answer::Reply(not_found(&prepared.alias)))?;
    let (target, bound_level) = alias.target(registry).map_err(|problem| {
        tracing::warn!("gateway alias {} cannot take a call: {problem}", alias.name);
        Answer::Reply(not_found(&prepared.alias))
    })?;
    if !kind.takes(target.endpoint.protocol) {
        return Err(Answer::Reply(invalid(
            Some("model"),
            "unsupported_model",
            &format!(
                "model {} is on a {} endpoint, which does not take {}",
                alias.name,
                target.endpoint.protocol.as_str(),
                kind.path()
            ),
        )));
    }
    let held = admit(facade, &llm.conf().limits, caller, &alias.name, &prepared.reserve(&target), open)
        .map_err(Answer::Reply)?;
    let level = prepared.level.or(bound_level);
    let factory = Arc::clone(factory);
    let caller = caller.clone();
    let timeout = Duration::from_secs(llm.conf().gateway.timeout.max(1));
    Ok(Answer::Later {
        held,
        work: Box::new(move |respond: &mut dyn Respond| {
            let context = factory.new_context();
            let _entered = context.enter();
            let _authz = context.disable_authz();
            match super::run::facade_of(&factory) {
                Ok(facade) => {
                    let llm = super::facade(&factory);
                    let upstream = Upstream {
                        llm: &llm,
                        facade: &facade,
                        caller: &caller,
                        target: &target,
                        prepared: &prepared,
                        level,
                        timeout,
                    };
                    upstream.run(respond);
                }
                Err(error) => respond.reply(internal(&error, "call")),
            }
        }),
    })
}

/// The quotas of the user and the client with what the call may use at most (`reserve`), then the host's room
/// (`open`), then the requests per minute of both and the tarpit, which count only a call the host has room for.
fn admit(
    facade: &EntityFacade,
    limits: &super::conf::LimitsConf,
    caller: &Caller,
    alias: &str,
    reserve: &Reserve,
    open: Open<'_>,
) -> Result<Held, Reply> {
    let now = jiff::Timestamp::now().as_millisecond();
    let failed = |error: &Error| internal(error, "quota check");
    let status = usage::check_quota_reserving(facade, limits, &caller.user_id, "gateway", now, reserve)
        .map_err(|error| failed(&error))?;
    if let Some(reason) = &status.exceeded {
        let seconds = usage::seconds_until_reset(&status.limit, now).unwrap_or(3600);
        return Err(too_many("insufficient_quota", &format!("the LLM quota does not allow the call: {reason}"), seconds));
    }
    if let Some(client) = &caller.client {
        if client.daily_tokens.is_some() || client.daily_cost.is_some() {
            let (start, _) = usage::day_of(now)
                .and_then(usage::day_bounds)
                .map_err(|error| failed(&error))?;
            let used = usage::client_calls_since(facade, &client.client_id, start).map_err(|error| failed(&error))?;
            if let Some(reason) = reserve.exceeds(&used, client.daily_tokens, client.daily_cost) {
                let day = usage::QuotaLimit {
                    period: "day".to_string(),
                    ..usage::QuotaLimit::default()
                };
                let seconds = usage::seconds_until_reset(&day, now).unwrap_or(3600);
                return Err(too_many(
                    "insufficient_quota",
                    &format!("the client's daily limit does not allow the call: {reason}"),
                    seconds,
                ));
            }
        }
    }
    let held = open(&caller.user_id)?;
    let user_rate = usage::quota_limits(facade, limits, &caller.user_id, "gateway")
        .map(|limits| usage::requests_per_minute(&limits))
        .map_err(|error| failed(&error))?;
    let rates = [
        user_rate.map(|rate| (format!("user:{}", caller.user_id), rate, "your")),
        caller
            .client
            .as_ref()
            .and_then(|client| client.requests_per_minute.map(|rate| (format!("client:{}", client.client_id), rate, "this client's"))),
    ];
    for (key, rate, whose) in rates.into_iter().flatten() {
        if let Err(seconds) = usage::take_request(&key, rate, now) {
            return Err(too_many(
                "rate_limit_exceeded",
                &format!("{whose} limit of {rate} requests per minute is reached; try again in {seconds} s"),
                seconds,
            ));
        }
    }
    count_gateway_hit(alias).map_err(|error| match ErrorKind::of(&error) {
        ErrorKind::Tarpit => too_many(
            "rate_limit_exceeded",
            "too many gateway requests; try again later",
            super::a2a::retry_after_of(&error).unwrap_or(60),
        ),
        _ => internal(&error, "tarpit check"),
    })?;
    Ok(held)
}

/// Count a gateway request of `alias` against the user's AT_LLM tarpit (`gateway:<alias>`).
///
/// # Errors
///
/// Returns the tarpit's refusal ([`ErrorKind::Tarpit`], with `retry_after`).
pub fn count_gateway_hit(alias: &str) -> Result<(), Error> {
    let name = format!("gateway:{alias}");
    ExecutionContext::with_current(|ec| -> Result<(), Error> {
        ec.push_checked("AT_LLM", &name, "view", false, true, None)?;
        let _ = ec.pop();
        Ok(())
    })
    .unwrap_or(Ok(()))
}

/// A model server's error as the caller gets it, with a generic message (the detail, which may name the internal
/// endpoint, is logged here and kept in `LlmCall`): its status for a request it refused, 429 for its rate limits, 502
/// or 504 for what failed on the way (its refusal of the central key included, which the caller cannot fix).
fn upstream_error(error: &ProviderError, model_id: &str) -> Reply {
    tracing::warn!(
        "the gateway call to model {model_id} failed: {}",
        usage::redact(&error.to_string(), &[])
    );
    let (status, kind, code, message): (u16, &str, &str, String) = match error.class {
        ErrorClass::RateLimited | ErrorClass::Quota => (
            429,
            "requests",
            "rate_limit_exceeded",
            "the model server is rate limited; try again later".to_string(),
        ),
        ErrorClass::Overloaded => (503, "server_error", "overloaded", "the model server is overloaded; try again later".to_string()),
        ErrorClass::Auth => (502, "server_error", "upstream_auth", "the model server refused the gateway's credentials".to_string()),
        ErrorClass::Timeout => (504, "server_error", "upstream_timeout", "the model server did not answer in time".to_string()),
        ErrorClass::Network | ErrorClass::Server => (
            502,
            "server_error",
            "upstream_error",
            "the model server could not be reached or failed".to_string(),
        ),
        ErrorClass::ContextOverflow => (
            400,
            "invalid_request_error",
            "context_length_exceeded",
            "the prompt is over the model's context window".to_string(),
        ),
        ErrorClass::ContentFilter => (
            400,
            "invalid_request_error",
            "content_filter",
            "the model server's content filter refused the request".to_string(),
        ),
        ErrorClass::BadRequest => {
            let status = error.status.filter(|status| matches!(status, 400 | 409 | 413 | 422)).unwrap_or(400);
            let message = match error.status {
                Some(upstream) => format!("the model server refused the request (HTTP {upstream})"),
                None => "the request could not be sent to the model server".to_string(),
            };
            (status, "invalid_request_error", "invalid_request", message)
        }
    };
    let reply = self::error(status, kind, Some(code), None, &message);
    match error.retry_after {
        Some(wait) if status == 429 || status == 503 => reply.with_header("retry-after", wait.as_secs().max(1).to_string()),
        _ => reply,
    }
}

/// One admitted call on the host's thread.
struct Upstream<'a> {
    llm: &'a super::LlmFacade,
    facade: &'a EntityFacade,
    caller: &'a Caller,
    target: &'a Target,
    prepared: &'a Prepared,
    level: Option<ReasoningLevel>,
    timeout: Duration,
}

/// What a call left for its `LlmCall` row.
#[derive(Default)]
struct Outcome {
    status: Option<u16>,
    first_byte_ms: Option<i64>,
    usage: Usage,
    /// The characters of the output passed on (text, reasoning, tool names and arguments), for the estimate.
    output_chars: usize,
    finish: Option<String>,
    error: Option<ProviderError>,
    response: Option<String>,
    /// The whole reply, sent once the call is logged, so a caller that reads its usage finds it.
    reply: Option<Reply>,
}

impl Upstream<'_> {
    fn run(&self, respond: &mut dyn Respond) {
        let body = upstream_body(self.prepared, self.target, self.level);
        let url = match self.prepared.kind {
            Kind::Embeddings => super::embed::embeddings_url(&self.target.endpoint),
            Kind::Chat | Kind::Responses => self.target.endpoint.request_url(),
        };
        let mut record = CallRecord::new(CallSource::Gateway, self.target);
        record.user_id = Some(self.caller.user_id.clone());
        record.client_id = self.caller.client.as_ref().map(|client| client.client_id.clone());
        let log_content = self.llm.conf().content_log.enabled;
        if log_content {
            record.request_json = Some(body.to_string());
        }
        let started = Instant::now();
        let cancel = respond.gone();
        let mut outcome = if self.prepared.stream {
            self.stream(&url, &body, respond, started, &cancel)
        } else {
            self.whole(&url, &body, started, &cancel)
        };
        record.duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
        record.http_status = outcome.status;
        record.first_token_ms = outcome.first_byte_ms;
        record.usage = outcome.usage;
        // the model read the prompt and wrote what was passed on even when no usage came (a cut stream): estimate it,
        // so the quotas count it
        if record.usage.is_zero() && outcome.status.is_some_and(|status| (200..300).contains(&status)) {
            let estimator = super::tokens::Estimator::for_model(&self.target.model);
            record.usage = Usage {
                input: self.prepared.prompt_tokens(self.target),
                output: estimator.tokens_of_chars(outcome.output_chars),
                ..Usage::default()
            };
            record.usage_estimated = !record.usage.is_zero();
        }
        record.finish = outcome.finish.take().or_else(|| outcome.error.as_ref().map(|_| "error".to_string()));
        if let Some(error) = &outcome.error {
            record.error_class = Some(error.class.as_str().to_string());
            record.error_text = Some(error.to_string());
        }
        if log_content {
            record.response_json = outcome.response.take();
        }
        if let Err(error) = self.llm.record_call(self.facade, self.target, &record) {
            tracing::error!("the gateway call to model {} was not logged: {error}", self.target.model.id);
        }
        if let Some(reply) = outcome.reply.take() {
            respond.reply(reply);
        }
    }

    /// The usage and finish of a whole response, as the protocol's decoder reads it.
    fn read_body(&self, json: &Json, outcome: &mut Outcome) {
        if self.prepared.kind == Kind::Embeddings {
            outcome.usage = super::embed::usage_of(json);
            outcome.finish = Some("stop".to_string());
            return;
        }
        let mut decoder = provider::protocol(self.target.endpoint.protocol).decoder();
        let mut events = Vec::new();
        decoder.body(json, &mut events);
        if !events.iter().any(|event| matches!(event, ProviderEvent::Finish(_))) {
            decoder.end(&mut events);
        }
        note_events(&events, outcome);
    }

    fn fail(&self, outcome: &mut Outcome, error: ProviderError) {
        outcome.reply = Some(upstream_error(&error, &self.target.model.id));
        outcome.error = Some(error);
    }

    /// A whole call, dropped when the caller goes away (`cancel`).
    fn whole(&self, url: &str, body: &Json, started: Instant, cancel: &CancelToken) -> Outcome {
        let mut outcome = Outcome::default();
        let limits = pump::Limits::new(self.timeout, Some(started + self.timeout));
        match super::embed::post(self.target, url, body, &limits, cancel, provider::MAX_JSON_BODY) {
            Ok(Some(posted)) => {
                outcome.status = Some(posted.status);
                outcome.first_byte_ms = posted.first_byte_ms;
                outcome.response = Some(posted.body.clone());
                if (200..300).contains(&posted.status) {
                    match serde_json::from_str::<Json>(&posted.body) {
                        Ok(json) => {
                            self.read_body(&json, &mut outcome);
                            outcome.reply = Some(Reply::json(200, &json));
                        }
                        Err(problem) => self.fail(
                            &mut outcome,
                            ProviderError::new(ErrorClass::Server, format!("the response is not JSON: {problem}")),
                        ),
                    }
                } else {
                    let mut error = provider::classify::classify_http(posted.status, &posted.headers, &posted.body);
                    error.status = Some(posted.status);
                    self.fail(&mut outcome, error);
                }
            }
            Ok(None) => outcome.finish = Some("canceled".to_string()),
            Err(error) => self.fail(&mut outcome, error),
        }
        outcome
    }

    fn stream(&self, url: &str, body: &Json, respond: &mut dyn Respond, started: Instant, cancel: &CancelToken) -> Outcome {
        let mut outcome = Outcome::default();
        let idle = Duration::from_secs(self.target.endpoint.idle_timeout.max(1));
        let call_deadline = started + self.timeout;
        let limits = pump::Limits::new(idle, Some(call_deadline));
        let (status, headers, stream) = match provider::open_json(self.target, url, body, "text/event-stream", &limits, cancel) {
            Ok(Some(opened)) => opened,
            Ok(None) => {
                outcome.finish = Some("canceled".to_string());
                return outcome;
            }
            Err(error) => {
                self.fail(&mut outcome, error);
                return outcome;
            }
        };
        outcome.status = Some(status);
        if !(200..300).contains(&status) {
            let (text, _) = provider::read_capped(stream, &limits, cancel, provider::MAX_ERROR_BODY);
            let mut error = provider::classify::classify_http(status, &headers, &text);
            error.status = Some(status);
            outcome.response = Some(text);
            self.fail(&mut outcome, error);
            return outcome;
        }
        let json_body = headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("content-type") && value.to_ascii_lowercase().contains("application/json")
        });
        if json_body {
            // a server that does not stream answers whole; the caller gets that
            let (text, _) = provider::read_capped(stream, &limits, cancel, provider::MAX_JSON_BODY);
            outcome.response = Some(text.clone());
            match serde_json::from_str::<Json>(&text) {
                Ok(json) => {
                    self.read_body(&json, &mut outcome);
                    outcome.reply = Some(Reply::json(200, &json));
                }
                Err(problem) => self.fail(
                    &mut outcome,
                    ProviderError::new(ErrorClass::Server, format!("the response is not JSON: {problem}")),
                ),
            }
            return outcome;
        }
        if !respond.start() {
            outcome.finish = Some("canceled".to_string());
            return outcome;
        }
        // each write must be taken within the idle limit and the call's deadline: a caller that stops reading is gone
        let send_by = || (Instant::now() + idle).min(call_deadline);
        let mut parser = sse::SseParser::new();
        let mut decoder = provider::protocol(self.target.endpoint.protocol).decoder();
        let mut events: Vec<ProviderEvent> = Vec::new();
        let mut logged = String::new();
        let mut first_byte_ms = None;
        let (mut gone, mut done, mut failure) = (false, false, None::<String>);
        let mut forward = |frame: &sse::Frame, events: &mut Vec<ProviderEvent>, respond: &mut dyn Respond| -> Step {
            if first_byte_ms.is_none() {
                first_byte_ms = Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
            }
            if logged.len() < MAX_LOGGED_STREAM {
                logged.push_str(&frame.data);
                logged.push('\n');
            }
            if frame.is_done() {
                return if respond.send("data: [DONE]\n\n", send_by()) { Step::Done } else { Step::Gone };
            }
            decoder.frame(frame, events);
            if self.prepared.kind == Kind::Chat && !self.prepared.include_usage && is_usage_chunk(&frame.data) {
                return Step::Sent;
            }
            let data = frame.data.replace('\n', "\ndata: ");
            let text = match &frame.event {
                Some(event) => format!("event: {event}\ndata: {data}\n\n"),
                None => format!("data: {data}\n\n"),
            };
            if respond.send(&text, send_by()) {
                Step::Sent
            } else {
                Step::Gone
            }
        };
        let end = stream.pump(&limits, cancel, |bytes| {
            let frames = match parser.push(bytes) {
                Ok(frames) => frames,
                Err(problem) => {
                    failure = Some(problem);
                    return false;
                }
            };
            for frame in frames {
                match forward(&frame, &mut events, respond) {
                    Step::Sent => {}
                    Step::Done => {
                        done = true;
                        return false;
                    }
                    Step::Gone => {
                        gone = true;
                        return false;
                    }
                }
            }
            true
        });
        if end == pump::End::Done && !done && !gone {
            if let Some(frame) = parser.finish() {
                match forward(&frame, &mut events, respond) {
                    Step::Done => done = true,
                    Step::Gone => gone = true,
                    Step::Sent => {}
                }
            }
        }
        // the decoder's own end: what it says when the stream ended without its finish
        let finished = done || events.iter().any(|event| matches!(event, ProviderEvent::Finish(_)));
        if !events.iter().any(|event| matches!(event, ProviderEvent::Finish(_))) {
            decoder.end(&mut events);
        }
        note_events(&events, &mut outcome);
        outcome.first_byte_ms = first_byte_ms;
        outcome.response = Some(logged);
        let ended_badly = match (&end, failure) {
            (_, Some(problem)) => Some(ProviderError::new(ErrorClass::Server, problem)),
            (pump::End::Failed(message), None) => Some(ProviderError::new(ErrorClass::Network, message.clone())),
            (pump::End::Idle, None) => Some(ProviderError::new(
                ErrorClass::Timeout,
                format!("no bytes from the model for {} s", idle.as_secs()),
            )),
            (pump::End::Deadline, None) => Some(ProviderError::new(ErrorClass::Timeout, "the call passed the gateway's timeout")),
            _ if !finished => Some(ProviderError::new(ErrorClass::Server, "the stream ended before its end")),
            _ => None,
        };
        if gone || cancel.is_canceled() {
            outcome.finish = Some("canceled".to_string());
            outcome.error = None;
        } else if let Some(error) = ended_badly {
            // not [DONE], as if it had ended well: an error frame the caller's client raises
            let frame = upstream_error(&error, &self.target.model.id).body;
            let _ = respond.send(&format!("data: {frame}\n\n"), send_by());
            outcome.finish = None;
            outcome.error = Some(error);
        }
        outcome
    }
}

/// What forwarding one frame did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Sent,
    /// The `[DONE]` frame: the stream is over.
    Done,
    /// The caller went away or stopped reading.
    Gone,
}

/// A Chat Completions chunk that holds only the usage (`choices` empty), which the caller did not ask for.
fn is_usage_chunk(data: &str) -> bool {
    serde_json::from_str::<Json>(data).is_ok_and(|chunk| {
        chunk.get("choices").and_then(Json::as_array).is_some_and(Vec::is_empty)
            && chunk.get("usage").is_some_and(|usage| !usage.is_null())
    })
}

/// The last usage, the finish and the output's characters of decoded events.
fn note_events(events: &[ProviderEvent], outcome: &mut Outcome) {
    for event in events {
        match event {
            ProviderEvent::Usage(usage) => outcome.usage = *usage,
            ProviderEvent::Text { delta, .. } | ProviderEvent::Reasoning { delta } | ProviderEvent::ToolArgs { delta, .. } => {
                outcome.output_chars += delta.chars().count();
            }
            ProviderEvent::ToolStart { name, .. } => outcome.output_chars += name.chars().count(),
            ProviderEvent::Finish(finish) => {
                outcome.finish = Some(finish.as_str().to_string());
                if let Some(error) = finish.error() {
                    outcome.error = Some(error.clone());
                }
            }
            _ => {}
        }
    }
}

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, store_api_client)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, Error>;

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn param(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(|value| to_plain_string(value).trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The scopes a client may have.
const SCOPES: [&str; 3] = ["a2a", "mcp", "gateway"];

/// `store#LlmApiClient` (AgentAdmin): create or change a client. Its credential is an app password of its user
/// (`appPasswordId`) or a login key of its user (`loginKey`, the key itself, kept only as its hash and never shown);
/// a new client needs one. Refused: a user that does not exist, a credential that is not the user's, a scope that is
/// not a2a, mcp or gateway, and a second client with the gateway scope for the same credential (the gateway would not
/// know whose limits apply). A field left out keeps its value. Out: `clientId`.
fn store_api_client(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !crate::user::has_permission("AgentAdmin").unwrap_or(false) {
        add_error("this needs the AgentAdmin permission");
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let client_id = param(parameters, "clientId");
    let existing = match &client_id {
        Some(id) => facade
            .find(LLM_API_CLIENT)?
            .eq("clientId", Value::string(id))
            .for_update(true)
            .one()?,
        None => None,
    };
    let value_of = |field: &str| -> Option<String> {
        if parameters.contains_key(field) {
            param(parameters, field)
        } else {
            existing.as_ref().and_then(|row| text_of(row, field))
        }
    };
    let Some(user_id) = value_of("userId") else {
        add_error("userId is required: the user the client calls as");
        return Ok(BTreeMap::new());
    };
    if facade
        .find("moqui.security.UserAccount")?
        .eq("userId", Value::string(&user_id))
        .one()?
        .is_none()
    {
        add_error(format!("there is no user {user_id}"));
        return Ok(BTreeMap::new());
    }
    let login_key = match param(parameters, "loginKey") {
        Some(key) => {
            let hash = crate::user::hash_login_key(&key)?;
            let owner = facade
                .find("moqui.security.UserLoginKey")?
                .eq("loginKey", Value::string(&hash))
                .one()?
                .and_then(|row| text_of(&row, "userId"));
            if owner.as_deref() != Some(user_id.as_str()) {
                add_error(format!("the login key given is not one of user {user_id}"));
                return Ok(BTreeMap::new());
            }
            Some(hash)
        }
        None => existing.as_ref().and_then(|row| text_of(row, "loginKey")),
    };
    let app_password_id = value_of("appPasswordId");
    if let Some(id) = &app_password_id {
        let owner = facade
            .find("moqui.security.UserAppPassword")?
            .eq("appPasswordId", Value::string(id))
            .one()?
            .and_then(|row| text_of(&row, "userId"));
        if owner.as_deref() != Some(user_id.as_str()) {
            add_error(format!("app password {id} is not one of user {user_id}"));
            return Ok(BTreeMap::new());
        }
    }
    if login_key.is_none() && app_password_id.is_none() {
        add_error("a client needs a credential: an app password of its user, or a login key of its user");
        return Ok(BTreeMap::new());
    }
    let scopes: Vec<String> = value_of("scopes")
        .map(|text| text.split(',').map(str::trim).filter(|scope| !scope.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    if let Some(scope) = scopes.iter().find(|scope| !SCOPES.contains(&scope.as_str())) {
        add_error(format!("scope {scope} is not a2a, mcp or gateway"));
        return Ok(BTreeMap::new());
    }
    if scopes.iter().any(|scope| scope == "gateway") {
        for (field, value) in [("loginKey", &login_key), ("appPasswordId", &app_password_id)] {
            let Some(value) = value else { continue };
            let other = facade
                .find(LLM_API_CLIENT)?
                .eq(field, Value::string(value))
                .list()?
                .iter()
                .find(|row| has_gateway_scope(row) && text_of(row, "clientId") != client_id)
                .and_then(|row| text_of(row, "clientId"));
            if let Some(other) = other {
                add_error(format!(
                    "client {other} already has the gateway scope for this credential; one credential may have one gateway client"
                ));
                return Ok(BTreeMap::new());
            }
        }
    }
    let creating = existing.is_none();
    let mut row = match existing {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(LLM_API_CLIENT)?;
            match &client_id {
                Some(id) => row.set("clientId", Value::string(id)),
                None => row.set_sequenced_id_primary(facade)?,
            }
            row.set("createdDate", crate::user::now_value());
            row
        }
    };
    row.set("userId", Value::string(&user_id));
    row.set("loginKey", login_key.map_or(Value::Null, Value::string));
    row.set("appPasswordId", app_password_id.map_or(Value::Null, Value::string));
    row.set("scopes", Value::string(scopes.join(",")));
    for field in ["description", "allowedAgents", "allowedAliases", "dailyTokens", "dailyCost", "requestsPerMinute", "disabled"] {
        if parameters.contains_key(field) {
            row.set(field, param(parameters, field).map_or(Value::Null, Value::string));
        }
    }
    if creating {
        row.create(facade)?;
    } else {
        row.update(facade)?;
    }
    let id = row.get("clientId").map(to_plain_string).unwrap_or_default();
    Ok(BTreeMap::from([("clientId".to_string(), Value::string(id))]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(kind: Kind, body: Json) -> (u16, Json) {
        let reply = prepare(kind, body.to_string().as_bytes()).unwrap_err();
        (reply.status, serde_json::from_str(&reply.body).unwrap())
    }

    fn target() -> Target {
        Target {
            endpoint: crate::llm::conf::EndpointConf::new("e", ProtocolId::ChatCompletions, "http://127.0.0.1:9"),
            model: crate::llm::conf::ModelConf::new("m", "e", "provider-name"),
            reasoning: ReasoningLevel::None,
            feature: None,
            fell_back_from: None,
        }
    }

    #[test]
    fn only_the_allowlisted_fields_pass_and_tools_are_functions() {
        let ok = prepare(
            Kind::Chat,
            json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}], "stream": true,
                "stream_options": {"include_usage": false}, "temperature": 0.2, "reasoning_effort": "high", "n": 1,
                "tools": [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}]})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(ok.alias, "fast");
        assert!(ok.stream && !ok.include_usage);
        assert_eq!(ok.level, Some(ReasoningLevel::High));
        assert!(!ok.fields.contains_key("model") && !ok.fields.contains_key("stream_options"));
        assert!(ok.prompt_chars > "userhi".len(), "the tools count too: {}", ok.prompt_chars);

        let (status, body) = refused(Kind::Chat, json!({"model": "fast", "messages": [{"role": "user"}], "base_url": "http://x"}));
        assert_eq!((status, body["error"]["code"].as_str(), body["error"]["param"].as_str()), (400, Some("unsupported_parameter"), Some("base_url")));
        let (_, body) = refused(Kind::Chat, json!({"model": "fast", "messages": [{"role": "user"}], "web_search_options": {}}));
        assert_eq!(body["error"]["param"], "web_search_options");
        let (_, body) = refused(
            Kind::Chat,
            json!({"model": "fast", "messages": [{"role": "user"}], "tools": [{"type": "web_search"}]}),
        );
        assert_eq!(body["error"]["code"], "unsupported_tool");
        let (_, body) = refused(Kind::Chat, json!({"model": "fast", "messages": []}));
        assert_eq!(body["error"]["param"], "messages");
        let (_, body) = refused(Kind::Chat, json!({"messages": [{"role": "user"}]}));
        assert_eq!(body["error"]["param"], "model");
        let (_, body) = refused(
            Kind::Chat,
            json!({"model": "m", "messages": [{"role": "user"}], "stream_options": {"include_obfuscation": true}}),
        );
        assert_eq!(body["error"]["param"], "stream_options");
        // one choice a call, and output limits that are whole numbers
        let (_, body) = refused(Kind::Chat, json!({"model": "m", "messages": [{"role": "user"}], "n": 8}));
        assert_eq!((body["error"]["param"].as_str(), body["error"]["code"].as_str()), (Some("n"), Some("unsupported_value")));
        let (_, body) = refused(Kind::Chat, json!({"model": "m", "messages": [{"role": "user"}], "max_tokens": -1}));
        assert_eq!(body["error"]["param"], "max_tokens");
        let (_, body) = refused(Kind::Responses, json!({"model": "m", "input": "x", "previous_response_id": "r1"}));
        assert!(body["error"]["message"].as_str().unwrap().contains("store: false"));
        let (_, body) = refused(Kind::Responses, json!({"model": "m", "input": "x", "store": true}));
        assert_eq!(body["error"]["param"], "store");
        let (_, body) = refused(Kind::Responses, json!({"model": "m", "input": "x", "tools": [{"type": "mcp", "server_url": "x"}]}));
        assert_eq!(body["error"]["code"], "unsupported_tool");
        let (_, body) = refused(Kind::Embeddings, json!({"model": "m", "input": "x", "stream": true}));
        assert_eq!(body["error"]["param"], "stream");
        assert_eq!(refused(Kind::Chat, json!([1])).0, 400);
    }

    #[test]
    fn the_upstream_body_takes_the_provider_name_the_shared_patches_and_clamped_limits() {
        let mut target = target();
        target.model.api_config = Some(json!({"messages": "not this", "stream_options": {"x": 1}, "seed": 7}));
        target.model.max_output = Some(2000);
        target
            .model
            .reasoning
            .insert(ReasoningLevel::High, json!({"chat_template_kwargs": {"enable_thinking": true}}));
        let prepared = prepare(
            Kind::Chat,
            json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}], "stream": true,
                "reasoning_effort": "high", "max_tokens": 100000, "max_completion_tokens": 50})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        let body = upstream_body(&prepared, &target, prepared.level);
        assert_eq!(body["model"], "provider-name");
        assert_eq!(body["messages"][0]["content"], "hi", "a patch cannot change the messages");
        assert_eq!(body["seed"], 7);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(body["stream_options"], json!({"include_usage": true}), "the gateway's stream options win");
        assert_eq!((body["max_tokens"].as_u64(), body["max_completion_tokens"].as_u64()), (Some(2000), Some(50)));
        let whole = upstream_body(&prepared, &target, None);
        assert!(whole.get("chat_template_kwargs").is_none());
        let responses = prepare(Kind::Responses, json!({"model": "m", "input": "x"}).to_string().as_bytes()).unwrap();
        assert_eq!(upstream_body(&responses, &target, None)["store"], false);
        // the reserve: the prompt estimated and the clamped output limit, else the model's
        assert_eq!(prepared.max_output(&target), 2000);
        assert_eq!(responses.max_output(&target), 2000);
        assert!(prepared.reserve(&target).tokens > 2000);
        target.model.max_output = None;
        assert_eq!(responses.max_output(&target), 0);
    }

    #[test]
    fn upstream_errors_reach_the_caller_without_their_detail() {
        let mut error = ProviderError::new(
            ErrorClass::Network,
            "error sending request for url (http://10.1.2.3:8000/v1/chat/completions): connection refused",
        );
        let reply = upstream_error(&error, "m");
        assert_eq!(reply.status, 502);
        assert!(!reply.body.contains("10.1.2.3"), "{}", reply.body);
        error.class = ErrorClass::BadRequest;
        error.status = Some(422);
        error.message = "model x at http://10.1.2.3 is not loaded".to_string();
        let reply = upstream_error(&error, "m");
        assert_eq!(reply.status, 422);
        assert!(!reply.body.contains("10.1.2.3") && reply.body.contains("HTTP 422"), "{}", reply.body);
        error.class = ErrorClass::RateLimited;
        error.retry_after = Some(Duration::from_secs(7));
        let reply = upstream_error(&error, "m");
        assert_eq!(reply.status, 429);
        assert!(reply.headers.iter().any(|(name, value)| name == "retry-after" && value == "7"));
    }

    #[test]
    fn routes_and_methods() {
        assert_eq!(route("GET", "/models").unwrap(), Route::Models);
        assert_eq!(route("GET", "/models/fast").unwrap(), Route::Model("fast".to_string()));
        assert_eq!(route("POST", "/chat/completions").unwrap(), Route::Call(Kind::Chat));
        assert_eq!(route("GET", "/chat/completions").unwrap_err().status, 405);
        assert_eq!(route("POST", "/completions").unwrap_err().status, 404);
        assert_eq!(route("POST", "/files").unwrap_err().status, 404);
    }

    #[test]
    fn a_usage_chunk_is_known_and_output_is_counted() {
        assert!(is_usage_chunk(r#"{"choices":[],"usage":{"prompt_tokens":3}}"#));
        assert!(!is_usage_chunk(r#"{"choices":[{"delta":{}}],"usage":null}"#));
        assert!(!is_usage_chunk("[DONE]"));
        let mut outcome = Outcome::default();
        note_events(
            &[
                ProviderEvent::Text { index: 0, delta: "Hello".to_string() },
                ProviderEvent::ToolArgs { index: 0, delta: "{}".to_string() },
            ],
            &mut outcome,
        );
        assert_eq!(outcome.output_chars, 7);
        assert!(outcome.usage.is_zero());
    }
}
