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

//! The scripted fake protocol, for the tests and for trying the agents with no model. It answers itself: the call goes
//! through the same pump, SSE parser, decoder, retries and call log as a real one, with a body it makes up.
//!
//! A test queues replies per endpoint with [`script`]: text and reasoning in pieces, tool calls with their arguments in
//! pieces, replay tokens, usage, a finish, an error event, an HTTP error status, pauses ([`Step::Delay`], a
//! [`Step::Gate`] the test opens, and [`Step::Hang`] for the idle limit) and a stream cut short ([`Step::Cut`]). With
//! nothing queued the fake follows the request: with a response schema it answers a sample of the schema; with tools it
//! calls each tool the newest user message names; with a reasoning level other than none it reasons and gives a replay
//! token; otherwise it answers "Fake reply to: …". Its text comes in pieces of 8 characters. Its usage counts 4
//! characters a token, and a request with the cache key of an earlier one reports as cached what a prefix-caching
//! server would: the longest common prefix of its prompt (the system text, the tools, then the messages, as serialized)
//! with that earlier one's; with [`single_slot`] it models a one-slot local server, where any request, a side call too,
//! replaces the cached prompt.
//! [`requests`] gives the bodies it was sent.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::sse::Frame;
use super::{apply_patches, count, text, Decoder, HttpCall, ModelRequest, Protocol, ProviderEvent, ReplayItem};
use crate::llm::conf::ReasoningLevel;
use crate::llm::registry::Target;
use crate::llm::usage::Usage;
use crate::outbound::OpenResponse;

pub struct Fake;

/// One step of a scripted reply.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// A pause before the next step, in milliseconds.
    Delay(u64),
    /// A pause until [`open_gate`] opens the gate of this name (at most a minute): a test that must act while the
    /// reply is in flight opens it when it is done, so the reply never ends first on a busy machine.
    Gate(String),
    Text(String),
    Reasoning(String),
    /// A replay token, sent back with the next request to the same model.
    Replay(String),
    /// A tool call, its arguments in `pieces` deltas.
    Tool {
        id: String,
        name: String,
        arguments: String,
        pieces: usize,
    },
    Usage(Usage),
    /// `stop`, `tool_calls`, `length`, `content_filter`, `refusal` or `context_overflow`.
    Finish(String),
    /// An error event in the stream: an error type (`overloaded_error`, `rate_limit_error`, `api_error`) and a message.
    Error {
        kind: String,
        message: String,
    },
    /// A read that blocks this long, in milliseconds, then the stream ends.
    Hang(u64),
    /// The stream ends here, with no finish.
    Cut,
}

/// One scripted reply: an HTTP status (an error status sends `error_body` and no stream), headers and the steps.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub error_body: String,
    pub steps: Vec<Step>,
}

impl Reply {
    pub fn steps(steps: Vec<Step>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            error_body: String::new(),
            steps,
        }
    }

    /// Text and a stop.
    pub fn text(text: &str) -> Self {
        Self::steps(vec![Step::Text(text.to_string()), Step::Finish("stop".to_string())])
    }

    /// An HTTP error.
    pub fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            error_body: body.to_string(),
            steps: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// The scripts and the bodies seen, by endpoint id.
#[derive(Default)]
struct State {
    scripts: HashMap<String, VecDeque<Reply>>,
    requests: HashMap<String, Vec<Json>>,
    /// The prompt of the last request with a cache key, by endpoint and key.
    cached: HashMap<String, String>,
    /// The endpoints that model a single-slot server (llama.cpp or LM Studio with one slot).
    single_slot: std::collections::HashSet<String>,
    /// The prompt in such an endpoint's one slot: the last request's, whatever its cache key.
    slot: HashMap<String, String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The bodies kept per endpoint.
const KEPT_REQUESTS: usize = 64;

fn with_state<T>(work: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    work(state.get_or_insert_with(State::default))
}

/// Queue replies for the endpoint `endpoint_id`, after any already queued.
pub fn script(endpoint_id: &str, replies: Vec<Reply>) {
    with_state(|state| {
        state
            .scripts
            .entry(endpoint_id.to_string())
            .or_default()
            .extend(replies)
    });
}

/// Drop the endpoint's queued replies, the bodies it was sent and the prompt in its slot.
pub fn clear(endpoint_id: &str) {
    with_state(|state| {
        state.scripts.remove(endpoint_id);
        state.requests.remove(endpoint_id);
        state.slot.remove(endpoint_id);
    });
}

/// Model the endpoint as a local server with one slot (on) or as a cache per key (off, the default). With one slot,
/// every request, a side call with no cache key too, replaces the cached prompt, and a request reads from the cache only
/// its common prefix with the request just before it on the endpoint: a side call between two calls of a conversation
/// evicts that conversation's prefix.
pub fn single_slot(endpoint_id: &str, on: bool) {
    with_state(|state| {
        state.slot.remove(endpoint_id);
        if on {
            state.single_slot.insert(endpoint_id.to_string());
        } else {
            state.single_slot.remove(endpoint_id);
        }
    });
}

/// The gates [`open_gate`] opened, until a reply passes them.
static GATES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static GATE_OPENED: Condvar = Condvar::new();

/// The longest a [`Step::Gate`] waits for its gate.
const GATE_WAIT: Duration = Duration::from_secs(60);

/// Open the gate `name`: the reply waiting at its [`Step::Gate`] (or the next one to reach it) goes on.
pub fn open_gate(name: &str) {
    GATES.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(name.to_string());
    GATE_OPENED.notify_all();
}

fn wait_for_gate(name: &str) {
    let deadline = std::time::Instant::now() + GATE_WAIT;
    let mut gates = GATES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if let Some(at) = gates.iter().position(|gate| gate == name) {
            gates.remove(at);
            return;
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            tracing::error!("the fake's gate {name} was not opened within {} s", GATE_WAIT.as_secs());
            return;
        }
        gates = GATE_OPENED
            .wait_timeout(gates, left)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
    }
}

/// The bodies the endpoint was sent, oldest first (the last 64).
pub fn requests(endpoint_id: &str) -> Vec<Json> {
    with_state(|state| state.requests.get(endpoint_id).cloned().unwrap_or_default())
}

impl Protocol for Fake {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn encode(&self, request: &ModelRequest, target: &Target) -> Result<HttpCall, Error> {
        let replays = request.replays(target);
        let messages: Vec<Json> = request
            .history
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let mut item = json!({"role": message.role, "text": request.text_of(index)});
                if !message.tool_calls.is_empty() {
                    item["tool_calls"] = json!(message.tool_calls);
                }
                if let Some(id) = &message.tool_call_id {
                    item["tool_call_id"] = json!(id);
                }
                if let Some(bag) = replays[index] {
                    item["replay"] = json!(bag.items);
                }
                item
            })
            .collect();
        let tools: Vec<Json> = request
            .sorted_tools()
            .iter()
            .map(|tool| json!({"name": tool.name, "parameters": tool.schema()}))
            .collect();
        let mut body = json!({
            "model": target.model.name,
            "system": request.system_text(),
            "tools": tools,
            "messages": messages,
            "reasoning": request.level(target).as_str(),
            "max_output": request.max_output(target),
            "schema": request.response_schema.as_ref().map(|schema| &schema.schema),
            "cache_key": request.cache_key,
            "stream": true,
        });
        apply_patches(&mut body, target, request.level(target));
        Ok(HttpCall {
            url: format!("fake://{}", target.endpoint.id),
            headers: Vec::new(),
            body,
        })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(FakeDecoder::default())
    }

    fn open(&self, call: &HttpCall, target: &Target, _timeout: Duration) -> Result<OpenResponse, Error> {
        let endpoint = target.endpoint.id.clone();
        let scripted = with_state(|state| {
            let seen = state.requests.entry(endpoint.clone()).or_default();
            seen.push(call.body.clone());
            if seen.len() > KEPT_REQUESTS {
                seen.remove(0);
            }
            state.scripts.get_mut(&endpoint).and_then(VecDeque::pop_front)
        });
        let reply = scripted.unwrap_or_else(|| follow(&call.body));
        if !(200..300).contains(&reply.status) {
            let mut headers = reply.headers.clone();
            headers.push(("content-type".to_string(), "application/json".to_string()));
            return Ok(OpenResponse {
                status: reply.status,
                headers,
                length: Some(reply.error_body.len() as u64),
                reader: Box::new(std::io::Cursor::new(reply.error_body.into_bytes())),
            });
        }
        let input = input_tokens(&call.body);
        // the cache is the endpoint's, as a server's is its own: one slot, or a prompt per cache key
        let slot = with_state(|state| {
            state.single_slot.contains(&endpoint).then(|| {
                let prompt = prompt_text(&call.body);
                state.slot.insert(endpoint.clone(), prompt.clone()).map(|earlier| cached_tokens(&earlier, &prompt))
            })
        });
        let cached = if let Some(slot) = slot {
            slot.unwrap_or(0).min(input)
        } else {
            text(call.body.get("cache_key")).map_or(0, |key| {
            let key = format!("{endpoint}\n{key}");
            let prompt = prompt_text(&call.body);
            let earlier = with_state(|state| state.cached.insert(key, prompt.clone())).unwrap_or_default();
            cached_tokens(&earlier, &prompt).min(input)
            })
        };
        let mut headers = reply.headers.clone();
        headers.push(("content-type".to_string(), "text/event-stream".to_string()));
        Ok(OpenResponse {
            status: reply.status,
            headers,
            length: None,
            reader: Box::new(FakeReader::new(
                &reply.steps,
                text(call.body.get("model")).unwrap_or_default(),
                input,
                cached,
            )),
        })
    }
}

/// 4 characters a token, at least 1.
fn tokens(characters: usize) -> u64 {
    (characters as u64).div_ceil(4).max(1)
}

fn input_tokens(body: &Json) -> u64 {
    let system = body.get("system").and_then(Json::as_str).map_or(0, str::len);
    let messages = body.get("messages").map_or(0, |messages| messages.to_string().len());
    let tools = body.get("tools").map_or(0, |tools| tools.to_string().len());
    tokens(system + messages + tools)
}

/// The prompt of a body as a server caches it: the system text, the tools and the messages, in that order.
fn prompt_text(body: &Json) -> String {
    let system = body.get("system").and_then(Json::as_str).unwrap_or_default();
    let tools = body.get("tools").map_or_else(String::new, Json::to_string);
    let messages = body.get("messages").map_or_else(String::new, Json::to_string);
    format!("{system}\n{tools}\n{messages}")
}

/// The tokens of `prompt` a server that cached `earlier` reads from its cache: their longest common prefix, in whole
/// tokens of 4 characters (all of it for the same prompt).
pub fn cached_tokens(earlier: &str, prompt: &str) -> u64 {
    if !earlier.is_empty() && earlier == prompt {
        return u64::MAX;
    }
    let common = earlier
        .bytes()
        .zip(prompt.bytes())
        .take_while(|(left, right)| left == right)
        .count();
    common as u64 / 4
}

/// The reply the fake makes up for a body when nothing is scripted.
fn follow(body: &Json) -> Reply {
    let messages = body
        .get("messages")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    let last = messages.last();
    let last_role = last
        .and_then(|message| message.get("role"))
        .and_then(Json::as_str)
        .unwrap_or("");
    let user_text = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Json::as_str) == Some("user"))
        .and_then(|message| message.get("text"))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    let mut steps = Vec::new();
    if body
        .get("reasoning")
        .and_then(Json::as_str)
        .is_some_and(|level| level != ReasoningLevel::None.as_str())
    {
        steps.push(Step::Reasoning("Thinking it over.".to_string()));
        steps.push(Step::Replay(format!("fake-{}", messages.len())));
    }
    if let Some(schema) = body.get("schema").filter(|schema| !schema.is_null()) {
        steps.push(Step::Text(sample(schema).to_string()));
        steps.push(Step::Finish("stop".to_string()));
        return Reply::steps(steps);
    }
    let tools = body.get("tools").and_then(Json::as_array).cloned().unwrap_or_default();
    if last_role == "user" && !tools.is_empty() {
        let named: Vec<&Json> = tools
            .iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(Json::as_str)
                    .is_some_and(|name| user_text.contains(name))
            })
            .collect();
        if !named.is_empty() {
            for (index, tool) in named.iter().enumerate() {
                steps.push(Step::Tool {
                    id: format!("fake_call_{}_{index}", messages.len()),
                    name: text(tool.get("name")).unwrap_or_default(),
                    arguments: tool
                        .get("parameters")
                        .map_or_else(|| "{}".to_string(), |schema| sample(schema).to_string()),
                    pieces: 2,
                });
            }
            steps.push(Step::Finish("tool_calls".to_string()));
            return Reply::steps(steps);
        }
    }
    let answer = if last_role == "tool" {
        format!(
            "Done: {}",
            last.and_then(|message| message.get("text"))
                .and_then(Json::as_str)
                .unwrap_or("")
        )
    } else {
        format!("Fake reply to: {}", user_text.chars().take(60).collect::<String>())
    };
    steps.push(Step::Text(answer));
    steps.push(Step::Finish("stop".to_string()));
    Reply::steps(steps)
}

/// A value that fits `schema`: an object's properties, the first of an enum, a const, one item of an array, and
/// `"text"`, 1 or true for the scalars.
pub fn sample(schema: &Json) -> Json {
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(first) = schema
        .get("enum")
        .and_then(Json::as_array)
        .and_then(|values| values.first())
    {
        return first.clone();
    }
    let kind = match schema.get("type") {
        Some(Json::String(kind)) => kind.as_str(),
        Some(Json::Array(kinds)) => kinds
            .iter()
            .filter_map(Json::as_str)
            .find(|kind| *kind != "null")
            .unwrap_or("null"),
        _ if schema.get("properties").is_some() => "object",
        _ => "string",
    };
    match kind {
        "object" => {
            let mut out = Map::new();
            if let Some(Json::Object(properties)) = schema.get("properties") {
                for (name, property) in properties {
                    out.insert(name.clone(), sample(property));
                }
            }
            Json::Object(out)
        }
        "array" => Json::Array(vec![sample(schema.get("items").unwrap_or(&json!({"type": "string"})))]),
        "integer" | "number" => json!(1),
        "boolean" => json!(true),
        "null" => Json::Null,
        _ => json!("text"),
    }
}

enum Piece {
    Bytes(Vec<u8>),
    Sleep(Duration),
    Gate(String),
}

/// The fake's response body: its SSE events, with the pauses between them.
struct FakeReader {
    pieces: VecDeque<Piece>,
    current: std::io::Cursor<Vec<u8>>,
}

impl FakeReader {
    fn new(steps: &[Step], model: String, input: u64, cached: u64) -> Self {
        let mut pieces = VecDeque::new();
        let event = |value: Json| Piece::Bytes(format!("data: {value}\n\n").into_bytes());
        pieces.push_back(event(json!({"type": "started", "model": model, "id": "fake-response"})));
        let mut output = 0usize;
        let mut usage_sent = false;
        let mut tool_index = 0u32;
        for step in steps {
            match step {
                Step::Delay(ms) => pieces.push_back(Piece::Sleep(Duration::from_millis(*ms))),
                Step::Gate(name) => pieces.push_back(Piece::Gate(name.clone())),
                Step::Text(text) => {
                    output += text.len();
                    for chunk in chunks(text, 8) {
                        pieces.push_back(event(json!({"type": "text", "delta": chunk})));
                    }
                }
                Step::Reasoning(text) => {
                    output += text.len();
                    pieces.push_back(event(json!({"type": "reasoning", "delta": text})));
                }
                Step::Replay(token) => pieces.push_back(event(json!({"type": "replay", "token": token}))),
                Step::Tool {
                    id,
                    name,
                    arguments,
                    pieces: count,
                } => {
                    output += arguments.len() + name.len();
                    pieces.push_back(event(
                        json!({"type": "tool", "index": tool_index, "id": id, "name": name}),
                    ));
                    let size = arguments.chars().count().div_ceil((*count).max(1)).max(1);
                    for chunk in chunks(arguments, size) {
                        pieces.push_back(event(json!({"type": "args", "index": tool_index, "delta": chunk})));
                    }
                    pieces.push_back(event(json!({"type": "tool_end", "index": tool_index})));
                    tool_index += 1;
                }
                Step::Usage(usage) => {
                    usage_sent = true;
                    pieces.push_back(event(usage_json(usage)));
                }
                Step::Finish(reason) => {
                    if !usage_sent {
                        usage_sent = true;
                        let usage = Usage {
                            input,
                            cached_input: cached,
                            cache_write: 0,
                            output: tokens(output),
                            reasoning: 0,
                        };
                        pieces.push_back(event(usage_json(&usage)));
                    }
                    pieces.push_back(event(json!({"type": "finish", "reason": reason})));
                }
                Step::Error { kind, message } => {
                    pieces.push_back(event(
                        json!({"type": "error", "error": {"type": kind, "message": message}}),
                    ));
                }
                Step::Hang(ms) => pieces.push_back(Piece::Sleep(Duration::from_millis(*ms))),
                Step::Cut => break,
            }
        }
        Self {
            pieces,
            current: std::io::Cursor::new(Vec::new()),
        }
    }
}

fn usage_json(usage: &Usage) -> Json {
    json!({"type": "usage", "input": usage.input, "cached_input": usage.cached_input, "cache_write": usage.cache_write,
        "output": usage.output, "reasoning": usage.reasoning})
}

fn chunks(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(size.max(1)).map(|chunk| chunk.iter().collect()).collect()
}

impl Read for FakeReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let read = self.current.read(buffer)?;
            if read > 0 {
                return Ok(read);
            }
            match self.pieces.pop_front() {
                None => return Ok(0),
                Some(Piece::Sleep(pause)) => std::thread::sleep(pause),
                Some(Piece::Gate(name)) => wait_for_gate(&name),
                Some(Piece::Bytes(bytes)) => self.current = std::io::Cursor::new(bytes),
            }
        }
    }
}

/// Decodes the fake's own events.
#[derive(Debug, Default)]
pub struct FakeDecoder {
    done: bool,
}

impl Decoder for FakeDecoder {
    fn frame(&mut self, frame: &Frame, out: &mut Vec<ProviderEvent>) {
        let Ok(event) = serde_json::from_str::<Json>(&frame.data) else {
            return;
        };
        let index = u32::try_from(count(event.get("index"))).unwrap_or(0);
        match event.get("type").and_then(Json::as_str).unwrap_or("") {
            "started" => out.push(ProviderEvent::Started {
                model: text(event.get("model")),
                id: text(event.get("id")),
            }),
            "text" => out.push(ProviderEvent::Text {
                index: 0,
                delta: text(event.get("delta")).unwrap_or_default(),
            }),
            "reasoning" => out.push(ProviderEvent::Reasoning {
                delta: text(event.get("delta")).unwrap_or_default(),
            }),
            "replay" => out.push(ProviderEvent::Replay(ReplayItem::Fake {
                token: text(event.get("token")).unwrap_or_default(),
            })),
            "tool" => out.push(ProviderEvent::ToolStart {
                index,
                call_id: text(event.get("id")).unwrap_or_default(),
                name: text(event.get("name")).unwrap_or_default(),
            }),
            "args" => out.push(ProviderEvent::ToolArgs {
                index,
                delta: text(event.get("delta")).unwrap_or_default(),
            }),
            "tool_end" => out.push(ProviderEvent::ToolEnd { index }),
            "usage" => out.push(ProviderEvent::Usage(Usage {
                input: count(event.get("input")),
                cached_input: count(event.get("cached_input")),
                cache_write: count(event.get("cache_write")),
                output: count(event.get("output")),
                reasoning: count(event.get("reasoning")),
            })),
            "finish" => {
                self.done = true;
                let reason = text(event.get("reason")).unwrap_or_default();
                let finish = match reason.as_str() {
                    "refusal" => super::Finish::Refusal,
                    "context_overflow" => super::Finish::ContextOverflow,
                    other => super::classify::chat_finish(other),
                };
                out.push(ProviderEvent::Finish(finish));
            }
            "error" => {
                self.done = true;
                let kind = event.pointer("/error/type").and_then(Json::as_str).map(str::to_string);
                let message = event
                    .pointer("/error/message")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut failure = super::ProviderError::new(
                    super::classify::classify_error(None, None, kind.as_deref(), &message),
                    message,
                );
                failure.code = kind;
                out.push(ProviderEvent::Finish(failure.into_finish()));
            }
            _ => {}
        }
    }

    fn end(&mut self, out: &mut Vec<ProviderEvent>) {
        if !self.done {
            self.done = true;
            out.push(ProviderEvent::Finish(super::Finish::Error(super::ProviderError::new(
                super::ErrorClass::Network,
                "the stream ended without a finish",
            ))));
        }
    }

    fn body(&mut self, _body: &Json, out: &mut Vec<ProviderEvent>) {
        self.end(out);
    }
}
