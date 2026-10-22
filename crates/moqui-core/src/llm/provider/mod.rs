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

//! The model providers (plan 4.2): a provider-neutral [`ModelRequest`], the [`Protocol`] trait with one implementation
//! per wire protocol ([`chat_completions`], [`responses`], [`anthropic`] and the scripted [`fake`]), the normalized
//! [`ProviderEvent`]s every protocol decodes to, and [`call`], which sends a request and assembles the [`Reply`].
//!
//! A call goes through [`crate::outbound::open`] (so a test's installed `HttpSender` answers it), and its body is read
//! by the reader-thread [`pump`] and the [`sse`] parser. A retryable error is tried again up to
//! [`classify::MAX_ATTEMPTS`] times; a reply cut at the output limit with no complete tool call is asked again once
//! with twice the output; an empty reply gets one nudge when the caller gives one; a context overflow goes to the
//! caller's compaction hook once. Each attempt is an `LlmCall` row ([`super::usage`]). Provider calls are the one
//! outbound path a sim allows: nothing here checks the sim.
//!
//! The reasoning level of a request is a JSON patch per model, deep-merged onto the body after the model's api-config
//! (nested objects combine, scalars and arrays replace, null removes a key), as Java's `LlmApiConfig`; a patch cannot
//! replace the messages, the input, the instructions, the system prompt, the tools or `stream`. What a model gives back
//! to be replayed (Anthropic thinking signatures, Responses reasoning items, Gemini thought signatures, DeepSeek
//! `reasoning_content`) is the [`ReplayBag`] of its assistant message, sent back only to the same endpoint and model,
//! and only as the model's `reasoning-replay` says.

pub mod anthropic;
pub mod chat_completions;
pub mod classify;
pub mod fake;
pub mod pump;
pub mod responses;
pub mod sse;

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value as Json};

use super::conf::{ProtocolId, ReasoningLevel, ReasoningReplay};
use super::registry::Target;
use super::usage::{self, CallRecord, CallSource, Usage};
use super::LlmFacade;
use crate::entity::EntityFacade;
use crate::outbound::OpenResponse;

pub use classify::{ErrorClass, Finish, ProviderError};
pub use pump::CancelToken;

/// The largest error body read.
pub(crate) const MAX_ERROR_BODY: usize = 1024 * 1024;

/// The largest whole JSON body read, when a server answers without streaming.
pub(crate) const MAX_JSON_BODY: usize = 16 * 1024 * 1024;

/// The limit of a call that has no deadline: the pump's idle limit ends a silent one long before.
const DEFAULT_CALL_LIMIT: Duration = Duration::from_secs(30 * 60);

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// A JSON schema object; `null` is an object with no properties.
    pub parameters: Json,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Json) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }

    /// The parameters schema, `{"type":"object","properties":{}}` when none was given.
    pub fn schema(&self) -> Json {
        match &self.parameters {
            Json::Null => json!({"type": "object", "properties": {}}),
            schema => schema.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    /// The result of a tool call.
    Tool,
}

/// A complete tool call: its arguments are a JSON object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Json,
}

/// One message of the history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// An assistant message's tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// A tool result's call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// A tool result that is an error.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    /// What an assistant message gives back to its model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<ReplayBag>,
    /// The model's own text of a JSON tool mode reply (plan 7.9), sent in place of the text and calls to a model in
    /// that mode, so its next request starts with the bytes it wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl Message {
    fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            is_error: false,
            replay: None,
            raw: None,
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, text)
    }

    pub fn tool_result(call_id: impl Into<String>, name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            tool_name: Some(name.into()),
            ..Self::new(Role::Tool, text)
        }
    }
}

/// What a model gave back with an assistant message to be sent back on later requests, tied to the endpoint and model
/// that made it: signatures and encrypted reasoning are rejected (or worse, misread) anywhere else.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplayBag {
    pub endpoint_id: String,
    pub model_id: String,
    pub items: Vec<ReplayItem>,
}

impl ReplayBag {
    /// Whether this bag goes back to `target`.
    pub fn is_for(&self, target: &Target) -> bool {
        self.endpoint_id == target.endpoint.id && self.model_id == target.model.id
    }

    /// The JSON text an assistant message stores.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// # Errors
    ///
    /// Returns an error when the text is not a stored bag.
    pub fn from_json(text: &str) -> Result<Self, Error> {
        serde_json::from_str(text).map_err(|error| Error::new(format!("a stored replay bag does not read: {error}")))
    }
}

/// One thing to replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ReplayItem {
    /// An Anthropic `thinking` block; its text may be empty (thinking omitted), the signature carries it.
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    /// A Responses `reasoning` output item with its `encrypted_content`.
    ResponsesReasoning {
        item: Json,
    },
    /// Chat Completions `reasoning_content`, echoed on the next request (DeepSeek's thinking mode requires it on tool
    /// turns, even empty).
    ReasoningContent {
        text: String,
    },
    /// The fields of a Chat Completions tool call beyond id, type and function: Gemini's
    /// `extra_content.google.thought_signature`.
    ToolCallExtra {
        call_id: String,
        fields: Map<String, Json>,
    },
    /// The fields of a Chat Completions assistant message beyond its content and calls (`extra_content`).
    MessageExtra {
        fields: Map<String, Json>,
    },
    /// The fake protocol's.
    Fake {
        token: String,
    },
}

/// A JSON schema the reply must follow (structured output).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResponseSchema {
    pub name: String,
    pub schema: Json,
    pub strict: bool,
}

/// A provider-neutral request: the stable system blocks, the tools (sent sorted by name), the append-only history, the
/// volatile tail of the newest user message, a reasoning level (else the target's), an output limit (else the
/// model's), an optional response schema and a cache key (the conversation id).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelRequest {
    pub system: Vec<String>,
    pub tools: Vec<ToolSpec>,
    pub history: Vec<Message>,
    /// Sent after the text of the newest user message. The caller keeps the same tail for the later steps of a turn, so
    /// the message's bytes do not change (append-only).
    pub tail: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub max_output: Option<u32>,
    pub response_schema: Option<ResponseSchema>,
    pub cache_key: Option<String>,
}

impl ModelRequest {
    /// The system blocks as one text.
    pub fn system_text(&self) -> String {
        self.system
            .iter()
            .map(String::as_str)
            .filter(|block| !block.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The tools sorted by name.
    pub fn sorted_tools(&self) -> Vec<&ToolSpec> {
        let mut tools: Vec<&ToolSpec> = self.tools.iter().collect();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        tools
    }

    pub fn level(&self, target: &Target) -> ReasoningLevel {
        self.reasoning.unwrap_or(target.reasoning)
    }

    /// The output limit: the request's, else the model's.
    pub fn max_output(&self, target: &Target) -> Option<u32> {
        self.max_output.or(target.model.max_output)
    }

    /// The text of message `index`, with the tail when it is the newest user message.
    pub fn text_of(&self, index: usize) -> Cow<'_, str> {
        let message = &self.history[index];
        match &self.tail {
            Some(tail) if message.role == Role::User && Some(index) == self.newest_user() && !tail.is_empty() => {
                Cow::Owned(if message.text.is_empty() {
                    tail.clone()
                } else {
                    format!("{}\n\n{tail}", message.text)
                })
            }
            _ => Cow::Borrowed(&message.text),
        }
    }

    fn newest_user(&self) -> Option<usize> {
        self.history.iter().rposition(|message| message.role == Role::User)
    }

    /// The replay bag each message sends to `target`: only an assistant message's, only one made by the same endpoint
    /// and model, and per the model's `reasoning-replay`: none, those of the turn in progress (after the newest user
    /// message), or all.
    pub fn replays<'a>(&'a self, target: &Target) -> Vec<Option<&'a ReplayBag>> {
        let from = match target.model.behavior.reasoning_replay {
            ReasoningReplay::None => usize::MAX,
            ReasoningReplay::Last => self.newest_user().map_or(0, |index| index + 1),
            ReasoningReplay::All => 0,
        };
        self.history
            .iter()
            .enumerate()
            .map(|(index, message)| {
                message
                    .replay
                    .as_ref()
                    .filter(|bag| index >= from && message.role == Role::Assistant && bag.is_for(target))
            })
            .collect()
    }
}

/// A normalized event of a model's stream. Tool events carry the call's index within the reply.
#[derive(Clone, Debug, PartialEq)]
pub enum ProviderEvent {
    Started { model: Option<String>, id: Option<String> },
    Text { index: u32, delta: String },
    Reasoning { delta: String },
    Replay(ReplayItem),
    ToolStart { index: u32, call_id: String, name: String },
    ToolArgs { index: u32, delta: String },
    ToolEnd { index: u32 },
    Usage(Usage),
    Finish(Finish),
}

impl ProviderEvent {
    /// The event as JSON, for the golden files and the content log.
    pub fn to_json(&self) -> Json {
        match self {
            ProviderEvent::Started { model, id } => json!({"type": "started", "model": model, "id": id}),
            ProviderEvent::Text { index, delta } => json!({"type": "text", "index": index, "delta": delta}),
            ProviderEvent::Reasoning { delta } => json!({"type": "reasoning", "delta": delta}),
            ProviderEvent::Replay(item) => json!({"type": "replay", "item": item}),
            ProviderEvent::ToolStart { index, call_id, name } => {
                json!({"type": "tool_start", "index": index, "call_id": call_id, "name": name})
            }
            ProviderEvent::ToolArgs { index, delta } => json!({"type": "tool_args", "index": index, "delta": delta}),
            ProviderEvent::ToolEnd { index } => json!({"type": "tool_end", "index": index}),
            ProviderEvent::Usage(usage) => json!({
                "type": "usage",
                "input": usage.input,
                "cached_input": usage.cached_input,
                "cache_write": usage.cache_write,
                "output": usage.output,
                "reasoning": usage.reasoning,
            }),
            ProviderEvent::Finish(finish) => match finish.error() {
                Some(error) => json!({
                    "type": "finish",
                    "finish": finish.as_str(),
                    "class": error.class.as_str(),
                    "status": error.status,
                    "code": error.code,
                    "message": error.message,
                }),
                None => json!({"type": "finish", "finish": finish.as_str()}),
            },
        }
    }
}

/// An encoded request: where it goes, its headers and its JSON body.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpCall {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Json,
}

/// A wire protocol.
pub trait Protocol: Send + Sync {
    /// `chat-completions`, `responses`, `anthropic-messages` or `fake`.
    fn id(&self) -> &'static str;

    /// The request for `target`.
    ///
    /// # Errors
    ///
    /// Returns an error when the request cannot be sent to this protocol.
    fn encode(&self, request: &ModelRequest, target: &Target) -> Result<HttpCall, Error>;

    /// A decoder for one call, stateful over its stream.
    fn decoder(&self) -> Box<dyn Decoder>;

    /// The error of a response that did not succeed.
    fn classify(&self, status: u16, headers: &[(String, String)], body: &str) -> ProviderError {
        classify::classify_http(status, headers, body)
    }

    /// Send the call and return once the response headers are in. The fake answers itself.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails before the response headers.
    fn open(&self, call: &HttpCall, _target: &Target, timeout: Duration) -> Result<OpenResponse, Error> {
        open_http(call, timeout)
    }
}

/// Turns the frames of one stream into events.
pub trait Decoder: Send {
    fn frame(&mut self, frame: &sse::Frame, out: &mut Vec<ProviderEvent>);

    /// The stream ended (`[DONE]` or the end of the body): the closing events, and a finish when the stream sent none.
    fn end(&mut self, out: &mut Vec<ProviderEvent>);

    /// A whole JSON body, from a server that answered without streaming.
    fn body(&mut self, body: &Json, out: &mut Vec<ProviderEvent>);
}

/// The implementation of a protocol.
pub fn protocol(id: ProtocolId) -> &'static dyn Protocol {
    match id {
        ProtocolId::ChatCompletions => &chat_completions::ChatCompletions,
        ProtocolId::Responses => &responses::Responses,
        ProtocolId::AnthropicMessages => &anthropic::AnthropicMessages,
        ProtocolId::Fake => &fake::Fake,
    }
}

/// The headers of an HTTP protocol: JSON in, a stream out, the auth header and the endpoint's own headers.
pub(crate) fn http_headers(target: &Target) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "text/event-stream".to_string()),
    ];
    if let Some(auth) = target.endpoint.auth() {
        headers.push(auth);
    }
    for (name, value) in &target.endpoint.headers {
        headers.retain(|(known, _)| !known.eq_ignore_ascii_case(name));
        headers.push((name.clone(), value.clone()));
    }
    headers
}

fn open_http(call: &HttpCall, timeout: Duration) -> Result<OpenResponse, Error> {
    let body = serde_json::to_vec(&call.body).map_err(|error| Error::new(format!("the request body: {error}")))?;
    crate::outbound::open(crate::outbound::OpenRequest {
        method: crate::outbound::Method::Post,
        url: call.url.clone(),
        headers: call.headers.clone(),
        body: Some(bytes::Bytes::from(body)),
        timeout: Some(timeout),
        follow_redirects: false,
    })
}

/// The keys of a body a patch cannot change.
const PROTECTED_KEYS: [&str; 6] = ["messages", "input", "instructions", "system", "tools", "stream"];

/// Merge `patch` onto `dest`: nested objects combine, other values replace, null removes the key.
pub fn deep_merge(dest: &mut Map<String, Json>, patch: &Map<String, Json>) {
    for (key, value) in patch {
        match value {
            Json::Null => {
                dest.remove(key);
            }
            Json::Object(nested) => {
                let slot = dest.entry(key.clone()).or_insert_with(|| Json::Object(Map::new()));
                if !slot.is_object() {
                    *slot = Json::Object(Map::new());
                }
                if let Json::Object(slot) = slot {
                    deep_merge(slot, nested);
                }
            }
            other => {
                dest.insert(key.clone(), other.clone());
            }
        }
    }
}

/// The model's api-config and then its patch for `level`, merged onto `body`, leaving the protected keys as the
/// protocol wrote them.
pub fn apply_patches(body: &mut Json, target: &Target, level: ReasoningLevel) {
    apply_model_patches(body, target, Some(level));
}

/// [`apply_patches`] with the level's patch only when there is a level (the gateway applies one only when the request
/// or the alias's feature asks for it).
pub fn apply_model_patches(body: &mut Json, target: &Target, level: Option<ReasoningLevel>) {
    let Json::Object(map) = body else {
        return;
    };
    let kept: Vec<(&str, Option<Json>)> = PROTECTED_KEYS
        .iter()
        .map(|key| (*key, map.get(*key).cloned()))
        .collect();
    for patch in [target.model.api_config.as_ref(), level.and_then(|level| target.model.reasoning.get(&level))]
        .into_iter()
        .flatten()
    {
        if let Json::Object(patch) = patch {
            deep_merge(map, patch);
        }
    }
    for (key, value) in kept {
        match value {
            Some(value) => {
                map.insert(key.to_string(), value);
            }
            None => {
                map.remove(key);
            }
        }
    }
}

/// Tool arguments as a JSON object, parsed leniently and then strictly: empty is `{}`; code fences, trailing commas and
/// Python's `True`, `False` and `None` are repaired, a string holding the object is unwrapped, single quotes become
/// double when there are no double quotes; what is left must parse as an object. A cut argument string is not
/// completed.
///
/// # Errors
///
/// Returns why the arguments are not an object.
pub fn parse_arguments(raw: &str) -> Result<Json, String> {
    let text = raw.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        return Ok(Json::Object(Map::new()));
    }
    let object = |value: Json| -> Result<Json, String> {
        match value {
            Json::Object(_) => Ok(value),
            Json::String(inner) => match serde_json::from_str::<Json>(&inner) {
                Ok(value @ Json::Object(_)) => Ok(value),
                _ => Err("the arguments are a string, not a JSON object".to_string()),
            },
            other => Err(format!("the arguments are {}, not a JSON object", kind_of(&other))),
        }
    };
    if let Ok(value) = serde_json::from_str::<Json>(text) {
        return object(value);
    }
    let mut repaired = text.to_string();
    if repaired.starts_with("```") {
        repaired = repaired.trim_start_matches('`').to_string();
        if let Some(rest) = repaired.strip_prefix("json") {
            repaired = rest.to_string();
        }
        repaired = repaired.trim_end().trim_end_matches('`').trim().to_string();
    }
    if !repaired.contains('"') && repaired.contains('\'') {
        repaired = repaired.replace('\'', "\"");
    }
    repaired = repair_outside_strings(&repaired);
    match serde_json::from_str::<Json>(&repaired) {
        Ok(value) => object(value),
        Err(error) => Err(format!("the arguments are not JSON: {error}")),
    }
}

fn kind_of(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Number(_) => "a number",
        Json::String(_) => "a string",
        Json::Array(_) => "an array",
        Json::Object(_) => "an object",
    }
}

/// Drop trailing commas and turn Python literals into JSON ones, outside string literals.
fn repair_outside_strings(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if in_string {
            out.push(ch);
            if ch == '\\' && index + 1 < chars.len() {
                out.push(chars[index + 1]);
                index += 1;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            ',' => {
                let next = chars[index + 1..].iter().find(|next| !next.is_whitespace());
                if !matches!(next, Some('}' | ']')) {
                    out.push(ch);
                }
            }
            _ if ch.is_ascii_alphabetic() && (index == 0 || !chars[index - 1].is_ascii_alphanumeric()) => {
                let word: String = chars[index..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == '_')
                    .collect();
                out.push_str(match word.as_str() {
                    "True" => "true",
                    "False" => "false",
                    "None" => "null",
                    other => other,
                });
                index += word.chars().count();
                continue;
            }
            _ => out.push(ch),
        }
        index += 1;
    }
    out
}

/// A tool call whose arguments did not parse: the run tells the model what was wrong.
#[derive(Clone, Debug, PartialEq)]
pub struct BadToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
    pub error: String,
}

/// What a call produced.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub model: Option<String>,
    pub response_id: Option<String>,
    pub text: String,
    pub reasoning: String,
    /// The complete calls, in order.
    pub tool_calls: Vec<ToolCall>,
    pub bad_tool_calls: Vec<BadToolCall>,
    /// The calls a cut at the output limit left incomplete, dropped.
    pub cut_tool_calls: usize,
    pub replay: Option<ReplayBag>,
    /// All attempts together.
    pub usage: Usage,
    /// The cost of all attempts, when the model has prices.
    pub cost: Option<Decimal>,
    pub finish: Finish,
    /// HTTP attempts made, retries, nudges and re-asks included.
    pub attempts: u32,
    pub http_status: Option<u16>,
    /// The `LlmCall` rows, one per attempt.
    pub call_ids: Vec<String>,
    /// The text deltas of the last attempt.
    pub text_deltas: u32,
    pub first_token_ms: Option<i64>,
    pub nudged: bool,
}

impl Reply {
    fn empty() -> Self {
        Self {
            model: None,
            response_id: None,
            text: String::new(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
            bad_tool_calls: Vec::new(),
            cut_tool_calls: 0,
            replay: None,
            usage: Usage::default(),
            cost: None,
            finish: Finish::Empty,
            attempts: 0,
            http_status: None,
            call_ids: Vec::new(),
            text_deltas: 0,
            first_token_ms: None,
            nudged: false,
        }
    }

    /// The assistant message to append to the history.
    pub fn assistant_message(&self) -> Message {
        Message {
            tool_calls: self.tool_calls.clone(),
            replay: self.replay.clone(),
            ..Message::assistant(self.text.clone())
        }
    }
}

/// Builds a [`Reply`] from the events of one attempt.
#[derive(Debug, Default)]
pub struct Assembler {
    model: Option<String>,
    response_id: Option<String>,
    text: String,
    reasoning: String,
    tools: BTreeMap<u32, (String, String, String)>,
    replay: Vec<ReplayItem>,
    usage: Usage,
    finish: Option<Finish>,
    text_deltas: u32,
}

impl Assembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, event: &ProviderEvent) {
        match event {
            ProviderEvent::Started { model, id } => {
                if model.is_some() {
                    self.model.clone_from(model);
                }
                if id.is_some() {
                    self.response_id.clone_from(id);
                }
            }
            ProviderEvent::Text { delta, .. } => {
                self.text.push_str(delta);
                self.text_deltas += 1;
            }
            ProviderEvent::Reasoning { delta } => self.reasoning.push_str(delta),
            ProviderEvent::Replay(item) => self.replay.push(item.clone()),
            ProviderEvent::ToolStart { index, call_id, name } => {
                let entry = self.tools.entry(*index).or_default();
                if !call_id.is_empty() {
                    entry.0.clone_from(call_id);
                }
                if !name.is_empty() {
                    entry.1.clone_from(name);
                }
            }
            ProviderEvent::ToolArgs { index, delta } => self.tools.entry(*index).or_default().2.push_str(delta),
            ProviderEvent::ToolEnd { .. } => {}
            ProviderEvent::Usage(usage) => self.usage = *usage,
            ProviderEvent::Finish(finish) => self.finish = Some(finish.clone()),
        }
    }

    pub fn has_finish(&self) -> bool {
        self.finish.is_some()
    }

    /// The reply, its tool arguments parsed: on a cut at the output limit only the calls whose arguments parse as they
    /// are stay, the rest are dropped; otherwise a call whose arguments do not parse even leniently is a bad call.
    pub fn finish(self, target: &Target) -> Reply {
        let finish = self.finish.unwrap_or_else(|| {
            Finish::Error(ProviderError::new(
                ErrorClass::Network,
                "the stream ended before the model finished",
            ))
        });
        let cut = finish == Finish::Length;
        let mut reply = Reply::empty();
        for (index, (id, name, raw)) in self.tools {
            let id = if id.is_empty() { format!("call_{index}") } else { id };
            let parsed = if cut {
                match serde_json::from_str::<Json>(raw.trim()) {
                    Ok(value @ Json::Object(_)) => Ok(value),
                    _ => {
                        reply.cut_tool_calls += 1;
                        continue;
                    }
                }
            } else {
                parse_arguments(&raw)
            };
            match parsed {
                Ok(arguments) if !name.is_empty() => reply.tool_calls.push(ToolCall { id, name, arguments }),
                Ok(_) => reply.bad_tool_calls.push(BadToolCall {
                    id,
                    name,
                    raw_arguments: raw,
                    error: "the call has no tool name".to_string(),
                }),
                Err(error) => reply.bad_tool_calls.push(BadToolCall {
                    id,
                    name,
                    raw_arguments: raw,
                    error,
                }),
            }
        }
        let complete = reply.tool_calls.len() + reply.bad_tool_calls.len();
        reply.finish = classify::settle(finish, !self.text.trim().is_empty(), complete);
        reply.model = self.model;
        reply.response_id = self.response_id;
        reply.text = self.text;
        reply.reasoning = self.reasoning;
        reply.usage = self.usage;
        reply.text_deltas = self.text_deltas;
        if !self.replay.is_empty() {
            reply.replay = Some(ReplayBag {
                endpoint_id: target.endpoint.id.clone(),
                model_id: target.model.id.clone(),
                items: self.replay,
            });
        }
        reply
    }
}

/// What a caller hears while a call streams.
#[derive(Debug)]
pub enum Signal<'a> {
    Event(&'a ProviderEvent),
    /// The call starts again (a retry, a nudge or a re-ask): text streamed so far is void.
    Restart {
        attempt: u32,
        reason: &'a str,
    },
}

/// Where the `LlmCall` rows go.
pub struct Recorder<'a> {
    pub llm: &'a LlmFacade,
    pub facade: &'a EntityFacade,
}

/// The caller's compaction of a request over the context window: a smaller request, or `None` to give up.
pub type CompactHook<'a> = &'a mut dyn FnMut(&ModelRequest) -> Option<ModelRequest>;

/// How a call is made, besides the request.
pub struct CallOptions<'a> {
    pub source: CallSource,
    pub user_id: Option<String>,
    pub run_id: Option<String>,
    pub step_seq: Option<i64>,
    pub cancel: CancelToken,
    /// When the whole call, retries included, must end.
    pub deadline: Option<Instant>,
    pub sink: Option<&'a mut dyn FnMut(Signal<'_>)>,
    /// An empty reply is asked again once with this user message after it.
    pub nudge: Option<String>,
    /// A reply cut at the output limit with no complete tool call is asked again once with twice the output.
    pub length_retry: bool,
    /// A context overflow goes here once; a request it returns is sent instead.
    pub compact: Option<CompactHook<'a>>,
    pub recorder: Option<Recorder<'a>>,
    /// The idle limit, else the endpoint's `idle-timeout`.
    pub idle: Option<Duration>,
    /// Attempts for a retryable error, the first included.
    pub max_attempts: u32,
}

impl<'a> CallOptions<'a> {
    pub fn new(source: CallSource) -> Self {
        Self {
            source,
            user_id: None,
            run_id: None,
            step_seq: None,
            cancel: CancelToken::new(),
            deadline: None,
            sink: None,
            nudge: None,
            length_retry: true,
            compact: None,
            recorder: None,
            idle: None,
            max_attempts: classify::MAX_ATTEMPTS,
        }
    }

    fn signal(&mut self, signal: Signal<'_>) {
        if let Some(sink) = self.sink.as_mut() {
            sink(signal);
        }
    }
}

/// The default nudge of an empty reply.
pub const EMPTY_NUDGE: &str = "Your last reply was empty. Answer the request, or call a tool.";

/// Send `request` to `target` and assemble the reply, with the retries, the re-ask, the nudge and the compaction the
/// options allow. The reply's finish says how it ended; an error is never returned.
///
/// With a recorder, a transaction in place on the call log's group is suspended for the call and resumed after it:
/// no transaction stays open while a model answers, and the `LlmCall` rows are written in transactions of their own.
pub fn call(target: &Target, request: &ModelRequest, options: &mut CallOptions<'_>) -> Reply {
    let in_place = options
        .recorder
        .as_ref()
        .and_then(|recorder| recorder.facade.transactions_for(usage::LLM_CALL).ok())
        .filter(crate::transaction::TransactionManager::in_transaction);
    let Some(transactions) = in_place else {
        return call_unsuspended(target, request, options);
    };
    let mut reply = None;
    let suspended = transactions.run(crate::transaction::TransactionAttribute::IgnoreSuspend, None, || {
        reply = Some(call_unsuspended(target, request, options));
        Ok(())
    });
    match (reply, suspended) {
        (Some(reply), Ok(())) => reply,
        (Some(reply), Err(error)) => {
            tracing::error!(
                "the transaction suspended for the call to model {} did not resume: {error}",
                target.model.id
            );
            reply
        }
        (None, result) => {
            let message = result.err().map_or_else(String::new, |error| error.to_string());
            let mut reply = Reply::empty();
            reply.finish = Finish::Error(ProviderError::new(
                ErrorClass::BadRequest,
                format!("the transaction in place could not be suspended for the model call: {message}"),
            ));
            reply
        }
    }
}

fn call_unsuspended(target: &Target, request: &ModelRequest, options: &mut CallOptions<'_>) -> Reply {
    let protocol = protocol(target.endpoint.protocol);
    let mut request = Cow::Borrowed(request);
    let mut attempts = 0;
    let mut error_attempts = 0;
    let mut usage = Usage::default();
    let mut call_ids = Vec::new();
    let (mut length_retried, mut nudged, mut compacted) = (false, false, false);
    loop {
        attempts += 1;
        let mut reply = exchange(protocol, target, &request, options, attempts);
        usage.add(&reply.usage);
        call_ids.append(&mut reply.call_ids);
        let out_of_time = options.deadline.is_some_and(|deadline| Instant::now() >= deadline);
        let again: Option<String> = match &reply.finish {
            Finish::Error(error) if !out_of_time && !options.cancel.is_canceled() => {
                error_attempts += 1;
                let backoff = (error_attempts < options.max_attempts)
                    .then(|| classify::backoff(error, error_attempts))
                    .flatten();
                match backoff {
                    Some(wait) if options.deadline.is_none_or(|deadline| Instant::now() + wait < deadline) => {
                        tracing::warn!(
                            "the call to model {} failed ({}); attempt {} in {} ms",
                            target.model.id,
                            usage::redact(&error.to_string(), &[]),
                            error_attempts + 1,
                            wait.as_millis()
                        );
                        if sleep_unless_canceled(wait, &options.cancel) {
                            Some(format!("retry after {}", error.class.as_str()))
                        } else {
                            reply.finish = Finish::Canceled;
                            None
                        }
                    }
                    _ => None,
                }
            }
            Finish::Length
                if options.length_retry && !length_retried && reply.tool_calls.is_empty() && !out_of_time =>
            {
                length_retried = true;
                let more = request
                    .max_output(target)
                    .unwrap_or(4096)
                    .saturating_mul(2)
                    .min(131_072);
                request.to_mut().max_output = Some(more);
                Some(format!(
                    "the reply was cut at the output limit; asking again with {more}"
                ))
            }
            Finish::Empty if !nudged && !out_of_time => match options.nudge.clone() {
                Some(nudge) => {
                    nudged = true;
                    request.to_mut().history.push(Message::user(nudge));
                    Some("the reply was empty; nudging".to_string())
                }
                None => None,
            },
            Finish::ContextOverflow if !compacted && !out_of_time => {
                compacted = true;
                match options.compact.as_mut().and_then(|compact| compact(&request)) {
                    Some(smaller) => {
                        request = Cow::Owned(smaller);
                        Some("the prompt was over the context window; compacted".to_string())
                    }
                    None => None,
                }
            }
            _ => None,
        };
        match again {
            Some(reason) => options.signal(Signal::Restart {
                attempt: attempts + 1,
                reason: &reason,
            }),
            None => {
                reply.usage = usage;
                reply.cost = usage::cost(&target.model.prices, &usage);
                reply.attempts = attempts;
                reply.call_ids = call_ids;
                reply.nudged = nudged;
                return reply;
            }
        }
    }
}

/// Sleep `wait` in short steps; false when the token was canceled meanwhile.
fn sleep_unless_canceled(wait: Duration, cancel: &CancelToken) -> bool {
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if cancel.is_canceled() {
            return false;
        }
        std::thread::sleep(
            until
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100)),
        );
    }
    !cancel.is_canceled()
}

/// One HTTP attempt, recorded.
fn exchange(
    protocol: &'static dyn Protocol,
    target: &Target,
    request: &ModelRequest,
    options: &mut CallOptions<'_>,
    attempt: u32,
) -> Reply {
    let started = Instant::now();
    let mut record = CallRecord::new(options.source, target);
    record.attempt = attempt;
    record.user_id.clone_from(&options.user_id);
    record.run_id.clone_from(&options.run_id);
    record.step_seq = options.step_seq;
    let log_content = options
        .recorder
        .as_ref()
        .is_some_and(|recorder| recorder.llm.conf().content_log.enabled);
    let mut events_log: Vec<Json> = Vec::new();
    let mut first_token: Option<i64> = None;
    let mut http_status = None;

    let mut assembler = Assembler::new();
    let outcome: Result<(), ProviderError> = (|| {
        let call = protocol
            .encode(request, target)
            .map_err(|error| ProviderError::new(ErrorClass::BadRequest, error.to_string()))?;
        if log_content {
            record.request_json = Some(call.body.to_string());
        }
        check_address(target).map_err(|message| ProviderError::new(ErrorClass::BadRequest, message))?;
        let Some(slot) = Slot::acquire(target, &options.cancel, options.deadline)? else {
            assembler.add(&ProviderEvent::Finish(Finish::Canceled));
            return Ok(());
        };
        let limit = options.deadline.map_or(DEFAULT_CALL_LIMIT, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        debug_assert!(
            !options.recorder.as_ref().is_some_and(|recorder| recorder
                .facade
                .transactions_for(usage::LLM_CALL)
                .is_ok_and(|transactions| transactions.in_transaction())),
            "a transaction is open across the call to model {}",
            target.model.id
        );
        let idle = options
            .idle
            .unwrap_or_else(|| Duration::from_secs(target.endpoint.idle_timeout.max(1)));
        let limits = pump::Limits::new(idle, options.deadline);
        // the request goes out on the reader thread, which keeps the endpoint's slot until it ends
        let opener: pump::Opener = {
            let call = call.clone();
            let target = target.clone();
            Box::new(move || {
                protocol
                    .open(&call, &target, limit)
                    .map(|opened| (opened.status, opened.headers, opened.reader))
                    .map_err(|error| error.to_string())
            })
        };
        let (status, headers, stream) = match pump::open(opener, slot, &limits, &options.cancel) {
            pump::Opened::Head {
                status,
                headers,
                stream,
            } => (status, headers, stream),
            pump::Opened::Failed(message) => {
                let class = if message.contains("timed out") || message.contains("timeout") {
                    ErrorClass::Timeout
                } else {
                    ErrorClass::Network
                };
                return Err(ProviderError::new(class, message));
            }
            pump::Opened::Ended(pump::End::Canceled) => {
                assembler.add(&ProviderEvent::Finish(Finish::Canceled));
                return Ok(());
            }
            pump::Opened::Ended(pump::End::Idle) => {
                return Err(ProviderError::new(
                    ErrorClass::Timeout,
                    format!("no response headers from the model within {} s", idle.as_secs_f64()),
                ));
            }
            pump::Opened::Ended(end) => {
                return Err(ProviderError::new(
                    ErrorClass::Timeout,
                    format!("the call ended before the response headers ({end:?})"),
                ));
            }
        };
        http_status = Some(status);
        if !(200..300).contains(&status) {
            let (body, _) = read_capped(stream, &limits, &options.cancel, MAX_ERROR_BODY);
            let mut error = protocol.classify(status, &headers, &body);
            error.status = Some(status);
            if log_content {
                record.response_json = Some(body);
            }
            return Err(error);
        }
        let content_type = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.to_ascii_lowercase())
            .unwrap_or_default();
        let mut decoder = protocol.decoder();
        let mut events = Vec::new();
        if content_type.contains("application/json") {
            let (body, end) = read_capped(stream, &limits, &options.cancel, MAX_JSON_BODY);
            if end == pump::End::Canceled {
                assembler.add(&ProviderEvent::Finish(Finish::Canceled));
                return Ok(());
            }
            let json: Json = serde_json::from_str(&body).map_err(|error| {
                ProviderError::new(ErrorClass::Server, format!("the response is not JSON: {error}"))
            })?;
            if classify::error_fields(&json).is_some() && json.get("choices").is_none() && json.get("output").is_none()
            {
                return Err(protocol.classify(status, &headers, &body));
            }
            decoder.body(&json, &mut events);
            if !events.iter().any(|event| matches!(event, ProviderEvent::Finish(_))) {
                decoder.end(&mut events);
            }
        } else {
            let mut parser = sse::SseParser::new();
            let mut finished = false;
            let mut failure: Option<ProviderError> = None;
            let cancel = options.cancel.clone();
            let end = stream.pump(&limits, &cancel, |bytes| {
                let frames = match parser.push(bytes) {
                    Ok(frames) => frames,
                    Err(message) => {
                        failure = Some(ProviderError::new(ErrorClass::Server, message));
                        return false;
                    }
                };
                for frame in frames {
                    if frame.is_done() {
                        finished = true;
                        return false;
                    }
                    let mut out = Vec::new();
                    decoder.frame(&frame, &mut out);
                    for event in &out {
                        if matches!(event, ProviderEvent::Finish(_)) {
                            finished = true;
                        }
                    }
                    events.extend(out);
                    if finished {
                        return false;
                    }
                }
                deliver(
                    &mut events,
                    &mut assembler,
                    options,
                    &mut events_log,
                    log_content,
                    &mut first_token,
                    started,
                );
                true
            });
            if let Some(failure) = failure {
                deliver(
                    &mut events,
                    &mut assembler,
                    options,
                    &mut events_log,
                    log_content,
                    &mut first_token,
                    started,
                );
                return Err(failure);
            }
            match end {
                pump::End::Canceled => {
                    deliver(
                        &mut events,
                        &mut assembler,
                        options,
                        &mut events_log,
                        log_content,
                        &mut first_token,
                        started,
                    );
                    assembler.add(&ProviderEvent::Finish(Finish::Canceled));
                    return Ok(());
                }
                pump::End::Deadline => {
                    deliver(
                        &mut events,
                        &mut assembler,
                        options,
                        &mut events_log,
                        log_content,
                        &mut first_token,
                        started,
                    );
                    return Err(ProviderError::new(ErrorClass::Timeout, "the call passed its deadline"));
                }
                pump::End::Idle => {
                    deliver(
                        &mut events,
                        &mut assembler,
                        options,
                        &mut events_log,
                        log_content,
                        &mut first_token,
                        started,
                    );
                    return Err(ProviderError::new(
                        ErrorClass::Timeout,
                        format!("no bytes from the model for {} s", idle.as_secs_f64()),
                    ));
                }
                pump::End::Failed(message) => {
                    deliver(
                        &mut events,
                        &mut assembler,
                        options,
                        &mut events_log,
                        log_content,
                        &mut first_token,
                        started,
                    );
                    return Err(ProviderError::new(ErrorClass::Network, message));
                }
                pump::End::Done | pump::End::Stopped => {
                    if !finished {
                        if let Some(frame) = parser.finish() {
                            if !frame.is_done() {
                                decoder.frame(&frame, &mut events);
                            }
                        }
                    }
                    if !events.iter().any(|event| matches!(event, ProviderEvent::Finish(_))) && !assembler.has_finish()
                    {
                        decoder.end(&mut events);
                    }
                }
            }
        }
        deliver(
            &mut events,
            &mut assembler,
            options,
            &mut events_log,
            log_content,
            &mut first_token,
            started,
        );
        Ok(())
    })();
    if let Err(error) = outcome {
        let event = ProviderEvent::Finish(error.into_finish());
        let mut events = vec![event];
        deliver(
            &mut events,
            &mut assembler,
            options,
            &mut events_log,
            log_content,
            &mut first_token,
            started,
        );
    }
    let mut reply = assembler.finish(target);
    reply.http_status = http_status;
    reply.first_token_ms = first_token;
    record.duration_ms = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    record.first_token_ms = first_token;
    record.http_status = http_status;
    record.finish = Some(reply.finish.as_str().to_string());
    if let Some(error) = reply.finish.error() {
        record.error_class = Some(error.class.as_str().to_string());
        record.error_text = Some(error.to_string());
    }
    record.usage = reply.usage;
    if log_content && record.response_json.is_none() {
        record.response_json = Some(Json::Array(events_log).to_string());
    }
    if let Some(recorder) = &options.recorder {
        match recorder.llm.record_call(recorder.facade, target, &record) {
            Ok(call_id) => reply.call_ids.push(call_id),
            Err(error) => tracing::error!("the call to model {} was not logged: {error}", target.model.id),
        }
    }
    reply
}

/// A response [`open_json`] opened: its status, its headers and its body.
pub(crate) type OpenedJson = (u16, Vec<(String, String)>, pump::Stream);

/// Open a POST of `body` to `url` on `target`'s endpoint for a caller that reads the response itself (the gateway, the
/// embedder): the endpoint's auth and headers with `accept`, its address check and its slot, sent on a reader thread that
/// keeps the slot until it ends, with the response headers waited for as [`exchange`] waits for them. `None` when
/// `cancel` was set first.
///
/// # Errors
///
/// Returns the error of a request that could not be sent or got no response headers.
pub(crate) fn open_json(
    target: &Target,
    url: &str,
    body: &Json,
    accept: &str,
    limits: &pump::Limits,
    cancel: &CancelToken,
) -> Result<Option<OpenedJson>, ProviderError> {
    if target.endpoint.protocol == ProtocolId::Fake {
        return Err(ProviderError::new(
            ErrorClass::BadRequest,
            format!("endpoint {} is the scripted fake, which has no HTTP API", target.endpoint.id),
        ));
    }
    check_address(target).map_err(|message| ProviderError::new(ErrorClass::BadRequest, message))?;
    let Some(slot) = Slot::acquire(target, cancel, limits.deadline)? else {
        return Ok(None);
    };
    let mut headers = http_headers(target);
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("accept"));
    headers.push(("Accept".to_string(), accept.to_string()));
    let call = HttpCall {
        url: url.to_string(),
        headers,
        body: body.clone(),
    };
    let limit = limits.deadline.map_or(DEFAULT_CALL_LIMIT, |deadline| deadline.saturating_duration_since(Instant::now()));
    let opener: pump::Opener = Box::new(move || {
        open_http(&call, limit)
            .map(|opened| (opened.status, opened.headers, opened.reader))
            .map_err(|error| error.to_string())
    });
    match pump::open(opener, slot, limits, cancel) {
        pump::Opened::Head { status, headers, stream } => Ok(Some((status, headers, stream))),
        pump::Opened::Failed(message) => {
            // the client's own timeout is the deadline, and its message may not say it timed out
            let at_deadline = limits
                .deadline
                .is_some_and(|deadline| Instant::now() + Duration::from_millis(100) >= deadline);
            let class = if at_deadline || message.contains("timed out") || message.contains("timeout") {
                ErrorClass::Timeout
            } else {
                ErrorClass::Network
            };
            Err(ProviderError::new(class, message))
        }
        pump::Opened::Ended(pump::End::Canceled) => Ok(None),
        pump::Opened::Ended(end) => Err(ProviderError::new(
            ErrorClass::Timeout,
            format!("no response headers from the model server ({end:?})"),
        )),
    }
}

/// Pass decoded events to the assembler and the caller.
fn deliver(
    events: &mut Vec<ProviderEvent>,
    assembler: &mut Assembler,
    options: &mut CallOptions<'_>,
    log: &mut Vec<Json>,
    log_content: bool,
    first_token: &mut Option<i64>,
    started: Instant,
) {
    for event in events.drain(..) {
        if first_token.is_none()
            && matches!(
                event,
                ProviderEvent::Text { .. } | ProviderEvent::Reasoning { .. } | ProviderEvent::ToolStart { .. }
            )
        {
            *first_token = Some(i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX));
        }
        if log_content {
            log.push(event.to_json());
        }
        assembler.add(&event);
        options.signal(Signal::Event(&event));
    }
}

/// Read a body to its end or `max` bytes, as text.
pub(crate) fn read_capped(stream: pump::Stream, limits: &pump::Limits, cancel: &CancelToken, max: usize) -> (String, pump::End) {
    let mut bytes = Vec::new();
    let end = stream.pump(limits, cancel, |chunk| {
        let room = max.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(room)]);
        bytes.len() < max
    });
    (String::from_utf8_lossy(&bytes).into_owned(), end)
}

/// An endpoint URL with a private address that is not loopback needs `allow-private-network` (or `local`); loopback
/// (LM Studio, llama.cpp and Ollama on the same machine) is always allowed. The host is read as the URL parser reads
/// it, so a numeric host (`http://2852039166/`) is the address it stands for, and an IPv4-mapped IPv6 address is the
/// IPv4 one. Only addresses written in the URL are checked, with no lookup: the URL is admin configuration.
pub(crate) fn check_address(target: &Target) -> Result<(), String> {
    let endpoint = &target.endpoint;
    if endpoint.protocol == ProtocolId::Fake || endpoint.allow_private_network || endpoint.local {
        return Ok(());
    }
    let Some(mut address) = moqui_lib::http::host_address(&endpoint.url) else {
        return Ok(());
    };
    if let std::net::IpAddr::V6(v6) = address {
        if let Some(v4) = v6.to_ipv4_mapped() {
            address = std::net::IpAddr::V4(v4);
        }
    }
    let private = match address {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b)) || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || v6.is_unspecified()
        }
    };
    if private && !address.is_loopback() {
        return Err(format!(
            "endpoint {} is at the private address {address}; set allow-private-network (allowPrivateNetwork) to call it",
            endpoint.id
        ));
    }
    Ok(())
}

/// The calls in flight per endpoint, against the endpoint's `max-concurrent` and the model's `max-concurrency`.
static SLOTS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
static SLOT_FREED: Condvar = Condvar::new();

pub(crate) struct Slot {
    key: Option<String>,
}

impl Slot {
    /// A slot of the endpoint, `None` when the cancel token was set while waiting for one.
    pub(crate) fn acquire(
        target: &Target,
        cancel: &CancelToken,
        deadline: Option<Instant>,
    ) -> Result<Option<Self>, ProviderError> {
        let limit = match (target.endpoint.max_concurrent, target.model.behavior.max_concurrency) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        let Some(limit) = limit.filter(|limit| *limit > 0) else {
            return Ok(Some(Self { key: None }));
        };
        let key = target.endpoint.id.clone();
        let mut slots = SLOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            let used = slots.get_or_insert_with(HashMap::new).entry(key.clone()).or_insert(0);
            if *used < limit {
                *used += 1;
                return Ok(Some(Self { key: Some(key) }));
            }
            if cancel.is_canceled() {
                return Ok(None);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(ProviderError::new(
                    ErrorClass::Timeout,
                    format!("endpoint {key} had no free slot before the deadline"),
                ));
            }
            slots = SLOT_FREED
                .wait_timeout(slots, Duration::from_millis(200))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if let Some(key) = &self.key {
            let mut slots = SLOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(used) = slots.get_or_insert_with(HashMap::new).get_mut(key) {
                *used = used.saturating_sub(1);
            }
            SLOT_FREED.notify_all();
        }
    }
}

/// A count from JSON, as a number or numeric text.
pub(crate) fn count(value: Option<&Json>) -> u64 {
    match value {
        Some(Json::Number(number)) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|n| n.max(0.0) as u64))
            .unwrap_or(0),
        Some(Json::String(text)) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Text from JSON; empty is `None`.
pub(crate) fn text(value: Option<&Json>) -> Option<String> {
    value
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Whether `value` fits `schema`, for the subset a structured reply is checked against: `type` (one or a list),
/// `required`, `properties`, `enum`, `const` and `items`. Out: the first mismatch, with its path.
///
/// # Errors
///
/// Returns where and why the value does not fit.
pub fn check_schema(value: &Json, schema: &Json) -> Result<(), String> {
    check_at(value, schema, "$")
}

fn check_at(value: &Json, schema: &Json, path: &str) -> Result<(), String> {
    if let Some(constant) = schema.get("const") {
        if constant != value {
            return Err(format!("{path} must be {constant}"));
        }
    }
    if let Some(values) = schema.get("enum").and_then(Json::as_array) {
        if !values.contains(value) {
            return Err(format!("{path} must be one of {}", Json::Array(values.clone())));
        }
    }
    let fits = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    };
    match schema.get("type") {
        Some(Json::String(kind)) if !fits(kind) => {
            return Err(format!("{path} must be {kind}, not {}", kind_of(value)));
        }
        Some(Json::Array(kinds)) if !kinds.iter().filter_map(Json::as_str).any(fits) => {
            return Err(format!(
                "{path} must be one of the types {}",
                Json::Array(kinds.clone())
            ));
        }
        _ => {}
    }
    if let Json::Object(map) = value {
        if let Some(required) = schema.get("required").and_then(Json::as_array) {
            for name in required.iter().filter_map(Json::as_str) {
                if !map.contains_key(name) {
                    return Err(format!("{path}.{name} is required"));
                }
            }
        }
        if let Some(Json::Object(properties)) = schema.get("properties") {
            for (name, property) in properties {
                if let Some(child) = map.get(name) {
                    check_at(child, property, &format!("{path}.{name}"))?;
                }
            }
            if schema.get("additionalProperties") == Some(&Json::Bool(false)) {
                if let Some(extra) = map.keys().find(|name| !properties.contains_key(*name)) {
                    return Err(format!("{path}.{extra} is not allowed"));
                }
            }
        }
    }
    if let (Json::Array(items), Some(item_schema)) = (value, schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            check_at(item, item_schema, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

/// The JSON of a structured reply: the whole text, else inside a code fence, else from its first `{` or `[` to the
/// matching last one.
///
/// # Errors
///
/// Returns an error when no JSON is found.
pub fn reply_json(text: &str) -> Result<Json, String> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str::<Json>(trimmed) {
        return Ok(value);
    }
    let unfenced = trimmed
        .strip_prefix("```")
        .map(|rest| rest.strip_prefix("json").unwrap_or(rest))
        .and_then(|rest| rest.rsplit_once("```").map(|(inner, _)| inner.trim()));
    if let Some(Ok(value)) = unfenced.map(serde_json::from_str::<Json>) {
        return Ok(value);
    }
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(start), Some(end)) = (trimmed.find(open), trimmed.rfind(close)) {
            if start < end {
                if let Ok(value) = serde_json::from_str::<Json>(&trimmed[start..=end]) {
                    return Ok(value);
                }
            }
        }
    }
    Err("the reply is not JSON".to_string())
}
