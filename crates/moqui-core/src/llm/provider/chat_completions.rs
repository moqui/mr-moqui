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

//! OpenAI Chat Completions, the protocol of LM Studio, llama.cpp, Ollama, vLLM, Gemini's compatible endpoint, DeepSeek
//! and GLM (from Java's `OpenAiCompatProtocol`).
//!
//! The system blocks are one system message; the tail follows the newest user message's text. A stream asks for its
//! usage (`stream_options.include_usage`), which comes after the finish reason, so the finish is emitted at the end of
//! the stream; a stream that ends with output and no finish reason is cut short, a retryable failure. Tool calls are
//! assembled by index, and split on the id when a server sends every parallel call at index 0 (Ollama). Reasoning comes
//! as `reasoning_content` (DeepSeek, llama.cpp, LM Studio), `reasoning` (vLLM, newer LM Studio) or `thinking`;
//! `reasoning_content` is kept to be echoed, even empty, since DeepSeek's thinking mode refuses a tool turn without it.
//! A tool call's fields beyond id, type and function (Gemini's `extra_content` with its thought signature) and a
//! message's `extra_content` are replayed too.

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::classify::{chat_finish, classify_error, Finish, ProviderError};
use super::sse::Frame;
use super::{
    apply_patches, count, http_headers, text, Decoder, HttpCall, ModelRequest, Protocol, ProviderEvent, ReplayItem,
    Role,
};
use crate::llm::registry::Target;
use crate::llm::usage::Usage;

pub struct ChatCompletions;

impl Protocol for ChatCompletions {
    fn id(&self) -> &'static str {
        "chat-completions"
    }

    fn encode(&self, request: &ModelRequest, target: &Target) -> Result<HttpCall, Error> {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(target.model.name));
        body.insert("messages".to_string(), Json::Array(messages(request, target)));
        if let Some(max) = request.max_output(target) {
            body.insert("max_tokens".to_string(), json!(max));
        }
        if let Some(temperature) = target.model.behavior.temperature {
            body.insert("temperature".to_string(), json!(temperature));
        }
        body.insert("stream".to_string(), json!(true));
        body.insert("stream_options".to_string(), json!({"include_usage": true}));
        let tools = request.sorted_tools();
        if !tools.is_empty() {
            let tools: Vec<Json> = tools
                .iter()
                .map(|tool| {
                    json!({"type": "function", "function": {
                        "name": tool.name, "description": tool.description, "parameters": tool.schema()}})
                })
                .collect();
            body.insert("tools".to_string(), Json::Array(tools));
            if let Some(parallel) = target.model.behavior.parallel_tool_calls {
                body.insert("parallel_tool_calls".to_string(), json!(parallel));
            }
        }
        if let Some(schema) = &request.response_schema {
            body.insert(
                "response_format".to_string(),
                json!({"type": "json_schema", "json_schema": {"name": schema.name, "schema": schema.schema, "strict": schema.strict}}),
            );
        }
        let mut body = Json::Object(body);
        apply_patches(&mut body, target, request.level(target));
        Ok(HttpCall {
            url: target.endpoint.request_url(),
            headers: http_headers(target),
            body,
        })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(ChatDecoder::default())
    }
}

fn messages(request: &ModelRequest, target: &Target) -> Vec<Json> {
    let mut out = Vec::new();
    let system = request.system_text();
    if !system.is_empty() {
        out.push(json!({"role": "system", "content": system}));
    }
    let replays = request.replays(target);
    for (index, message) in request.history.iter().enumerate() {
        match message.role {
            Role::User => out.push(json!({"role": "user", "content": request.text_of(index)})),
            Role::Tool => out.push(json!({
                "role": "tool",
                "tool_call_id": message.tool_call_id.clone().unwrap_or_default(),
                "content": message.text,
            })),
            Role::Assistant => {
                let mut item = Map::new();
                item.insert("role".to_string(), json!("assistant"));
                item.insert(
                    "content".to_string(),
                    if message.text.is_empty() && !message.tool_calls.is_empty() {
                        Json::Null
                    } else {
                        json!(message.text)
                    },
                );
                let replay = replays[index].map(|bag| bag.items.as_slice()).unwrap_or_default();
                if !message.tool_calls.is_empty() {
                    let calls: Vec<Json> = message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            let mut entry = Map::new();
                            entry.insert("id".to_string(), json!(call.id));
                            entry.insert("type".to_string(), json!("function"));
                            entry.insert(
                                "function".to_string(),
                                json!({"name": call.name, "arguments": call.arguments.to_string()}),
                            );
                            for item in replay {
                                if let ReplayItem::ToolCallExtra { call_id, fields } = item {
                                    if *call_id == call.id {
                                        for (key, value) in fields {
                                            entry.entry(key.clone()).or_insert_with(|| value.clone());
                                        }
                                    }
                                }
                            }
                            Json::Object(entry)
                        })
                        .collect();
                    item.insert("tool_calls".to_string(), Json::Array(calls));
                }
                for replayed in replay {
                    match replayed {
                        ReplayItem::ReasoningContent { text } => {
                            item.insert("reasoning_content".to_string(), json!(text));
                        }
                        ReplayItem::MessageExtra { fields } => {
                            for (key, value) in fields {
                                if !matches!(key.as_str(), "role" | "content" | "tool_calls" | "reasoning_content") {
                                    item.insert(key.clone(), value.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                out.push(Json::Object(item));
            }
        }
    }
    out
}

/// One tool call being assembled.
#[derive(Debug, Default)]
struct Call {
    id: String,
    name: String,
    /// Whether its ToolStart went out.
    started: bool,
    /// Arguments that came before the name.
    pending: String,
    extra: Map<String, Json>,
}

#[derive(Debug, Default)]
pub struct ChatDecoder {
    started: bool,
    finish: Option<String>,
    refusal: String,
    has_output: bool,
    saw_reasoning_content: bool,
    reasoning_content: String,
    message_extra: Map<String, Json>,
    /// By our index.
    calls: BTreeMap<u32, Call>,
    /// The server's index of each of our calls' latest chunk, and our index for it.
    by_server_index: BTreeMap<u64, u32>,
    by_id: BTreeMap<String, u32>,
    usage: Option<Usage>,
    error: Option<ProviderError>,
    done: bool,
}

impl ChatDecoder {
    fn chunk(&mut self, chunk: &Json, out: &mut Vec<ProviderEvent>) {
        if !self.started {
            self.started = true;
            out.push(ProviderEvent::Started {
                model: text(chunk.get("model")),
                id: text(chunk.get("id")),
            });
        }
        if let Some((code, kind, message)) = super::classify::error_fields(chunk) {
            let mut error = ProviderError::new(
                classify_error(None, code.as_deref(), kind.as_deref(), &message),
                message,
            );
            error.code = code.or(kind);
            self.error = Some(error);
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            self.usage = Some(usage_of(usage, chunk.get("timings")));
        } else if let Some(timings) = chunk.get("timings").filter(|timings| timings.is_object()) {
            // llama.cpp without include_usage
            if self.usage.is_none() {
                self.usage = Some(usage_of(&json!({}), Some(timings)));
            }
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Json::as_array)
            .and_then(|choices| choices.first())
        else {
            return;
        };
        let choice_index = u32::try_from(count(choice.get("index"))).unwrap_or(0);
        if let Some(reason) = text(choice.get("finish_reason")) {
            self.finish = Some(reason);
        }
        // a whole message (a server that did not stream) reads as one delta
        let Some(delta) = choice
            .get("delta")
            .or_else(|| choice.get("message"))
            .filter(|delta| delta.is_object())
        else {
            return;
        };
        if let Some(content) = delta
            .get("content")
            .and_then(Json::as_str)
            .filter(|content| !content.is_empty())
        {
            self.has_output = true;
            out.push(ProviderEvent::Text {
                index: choice_index,
                delta: content.to_string(),
            });
        }
        if let Some(refusal) = delta.get("refusal").and_then(Json::as_str) {
            self.refusal.push_str(refusal);
        }
        if let Some(reasoning) = delta.get("reasoning_content") {
            self.saw_reasoning_content = true;
            if let Some(reasoning) = reasoning.as_str().filter(|reasoning| !reasoning.is_empty()) {
                self.reasoning_content.push_str(reasoning);
                out.push(ProviderEvent::Reasoning {
                    delta: reasoning.to_string(),
                });
            }
        } else if let Some(reasoning) = text(delta.get("reasoning")).or_else(|| text(delta.get("thinking"))) {
            out.push(ProviderEvent::Reasoning { delta: reasoning });
        }
        if let Some(Json::Object(extra)) = delta.get("extra_content") {
            self.message_extra
                .insert("extra_content".to_string(), Json::Object(extra.clone()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Json::as_array) {
            for call in calls {
                self.tool_call(call, out);
            }
        }
    }

    fn tool_call(&mut self, call: &Json, out: &mut Vec<ProviderEvent>) {
        self.has_output = true;
        let id = text(call.get("id"));
        let server_index = call.get("index").and_then(Json::as_u64);
        let next = u32::try_from(self.calls.len()).unwrap_or(u32::MAX);
        let ours = match (&id, server_index) {
            (Some(id), _) if self.by_id.contains_key(id) => self.by_id[id],
            (Some(id), Some(server)) => match self.by_server_index.get(&server) {
                // the same index with a new id: a server that sends every call at index 0
                Some(known)
                    if self
                        .calls
                        .get(known)
                        .is_some_and(|call| !call.id.is_empty() && call.id != *id) =>
                {
                    next
                }
                Some(known) => *known,
                None => next,
            },
            (Some(_), None) => next,
            (None, Some(server)) => self.by_server_index.get(&server).copied().unwrap_or(next),
            (None, None) => next.saturating_sub(1).min(next),
        };
        if let Some(server) = server_index {
            self.by_server_index.insert(server, ours);
        }
        let entry = self.calls.entry(ours).or_default();
        if let Some(id) = &id {
            if entry.id.is_empty() {
                entry.id.clone_from(id);
            }
            self.by_id.insert(id.clone(), ours);
        }
        let function = call.get("function");
        if let Some(name) = text(function.and_then(|function| function.get("name"))) {
            if entry.name.is_empty() {
                entry.name = name;
            }
        }
        if let Json::Object(fields) = call {
            for (key, value) in fields {
                if !matches!(key.as_str(), "id" | "type" | "function" | "index") && !value.is_null() {
                    entry.extra.insert(key.clone(), value.clone());
                }
            }
        }
        let arguments = match function.and_then(|function| function.get("arguments")) {
            Some(Json::String(arguments)) => arguments.clone(),
            Some(Json::Null) | None => String::new(),
            // an object instead of its text (some local servers)
            Some(other) => other.to_string(),
        };
        if !entry.started && !entry.name.is_empty() {
            entry.started = true;
            out.push(ProviderEvent::ToolStart {
                index: ours,
                call_id: entry.id.clone(),
                name: entry.name.clone(),
            });
            if !entry.pending.is_empty() {
                out.push(ProviderEvent::ToolArgs {
                    index: ours,
                    delta: std::mem::take(&mut entry.pending),
                });
            }
        }
        if !arguments.is_empty() {
            if entry.started {
                out.push(ProviderEvent::ToolArgs {
                    index: ours,
                    delta: arguments,
                });
            } else {
                entry.pending.push_str(&arguments);
            }
        }
    }
}

impl Decoder for ChatDecoder {
    fn frame(&mut self, frame: &Frame, out: &mut Vec<ProviderEvent>) {
        match serde_json::from_str::<Json>(&frame.data) {
            Ok(chunk) => self.chunk(&chunk, out),
            Err(error) => tracing::debug!("a Chat Completions stream frame is not JSON, skipped: {error}"),
        }
    }

    fn end(&mut self, out: &mut Vec<ProviderEvent>) {
        if self.done {
            return;
        }
        self.done = true;
        for (index, call) in &mut self.calls {
            if !call.started {
                // a call that never got a name still goes out, to be reported as bad
                call.started = true;
                out.push(ProviderEvent::ToolStart {
                    index: *index,
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                });
                if !call.pending.is_empty() {
                    out.push(ProviderEvent::ToolArgs {
                        index: *index,
                        delta: std::mem::take(&mut call.pending),
                    });
                }
            }
            out.push(ProviderEvent::ToolEnd { index: *index });
            if !call.extra.is_empty() {
                out.push(ProviderEvent::Replay(ReplayItem::ToolCallExtra {
                    call_id: call.id.clone(),
                    fields: call.extra.clone(),
                }));
            }
        }
        if self.saw_reasoning_content {
            out.push(ProviderEvent::Replay(ReplayItem::ReasoningContent {
                text: std::mem::take(&mut self.reasoning_content),
            }));
        }
        if !self.message_extra.is_empty() {
            out.push(ProviderEvent::Replay(ReplayItem::MessageExtra {
                fields: std::mem::take(&mut self.message_extra),
            }));
        }
        if let Some(usage) = self.usage {
            out.push(ProviderEvent::Usage(usage));
        }
        let finish = if let Some(error) = self.error.take() {
            error.into_finish()
        } else if let Some(reason) = &self.finish {
            let finish = chat_finish(reason);
            if finish == Finish::Stop && !self.has_output && !self.refusal.is_empty() {
                Finish::Refusal
            } else {
                finish
            }
        } else if !self.refusal.is_empty() && !self.has_output {
            Finish::Refusal
        } else {
            Finish::Error(ProviderError::new(
                super::ErrorClass::Network,
                if self.has_output {
                    "the stream ended without a finish reason"
                } else {
                    "the stream ended with no reply"
                },
            ))
        };
        out.push(ProviderEvent::Finish(finish));
    }

    fn body(&mut self, body: &Json, out: &mut Vec<ProviderEvent>) {
        self.chunk(body, out);
        self.end(out);
    }
}

/// Usage from a Chat Completions `usage` object: OpenAI's and vLLM's `prompt_tokens_details.cached_tokens`, DeepSeek's
/// `prompt_cache_hit_tokens`, `completion_tokens_details.reasoning_tokens`, and llama.cpp's `timings` (`cache_n` cached
/// of `cache_n + prompt_n`) when the usage has no counts. A server whose total adds the reasoning beside the completion
/// tokens (Gemini) gets it added into the output, which counts it everywhere else.
pub fn usage_of(usage: &Json, timings: Option<&Json>) -> Usage {
    let mut out = Usage {
        input: count(usage.get("prompt_tokens")),
        output: count(usage.get("completion_tokens")),
        cached_input: count(usage.pointer("/prompt_tokens_details/cached_tokens"))
            .max(count(usage.get("prompt_cache_hit_tokens"))),
        cache_write: 0,
        reasoning: count(usage.pointer("/completion_tokens_details/reasoning_tokens")),
    };
    let total = count(usage.get("total_tokens"));
    if out.reasoning > 0 && total > 0 && total == out.input + out.output + out.reasoning {
        out.output += out.reasoning;
    }
    if let Some(timings) = timings {
        let cached = count(timings.get("cache_n"));
        if out.cached_input == 0 {
            out.cached_input = cached;
        }
        if out.input == 0 {
            out.input = cached + count(timings.get("prompt_n"));
        }
        if out.output == 0 {
            out.output = count(timings.get("predicted_n"));
        }
    }
    out
}
