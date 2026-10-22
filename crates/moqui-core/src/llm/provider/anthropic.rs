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

//! Anthropic Messages, native (`/v1/messages`). `cache_control` marks the end of the tools, of the system blocks and of
//! the last history message, so the prefix is cached up to the newest turn. Tool results are `tool_result` blocks in a
//! user message, consecutive ones together. A `thinking` block (its text may be empty when the thinking is omitted; the
//! `signature` carries it) and a `redacted_thinking` block are kept in the reply's replay bag and sent back unchanged,
//! first in their assistant message, to the same endpoint and model. Thinking and effort come from the model's
//! reasoning patches (`thinking`, `output_config.effort`); a response schema is `output_config.format`. Usage adds the
//! cache reads and writes into the input, which Anthropic reports beside it. The temperature is not sent: current
//! models refuse it, and the api-config can add it for one that takes it.

use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::classify::{anthropic_finish, classify_error, ErrorClass, Finish, ProviderError};
use super::sse::Frame;
use super::{
    apply_patches, count, http_headers, text, Decoder, HttpCall, ModelRequest, Protocol, ProviderEvent, ReplayItem,
    Role,
};
use crate::llm::registry::Target;
use crate::llm::usage::Usage;

pub struct AnthropicMessages;

/// The output limit when neither the request nor the model gives one: the API requires one.
const DEFAULT_MAX_TOKENS: u32 = 16_000;

const API_VERSION: &str = "2023-06-01";

impl Protocol for AnthropicMessages {
    fn id(&self) -> &'static str {
        "anthropic-messages"
    }

    fn encode(&self, request: &ModelRequest, target: &Target) -> Result<HttpCall, Error> {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(target.model.name));
        body.insert(
            "max_tokens".to_string(),
            json!(request.max_output(target).unwrap_or(DEFAULT_MAX_TOKENS)),
        );
        let mut system: Vec<Json> = request
            .system
            .iter()
            .filter(|block| !block.is_empty())
            .map(|block| json!({"type": "text", "text": block}))
            .collect();
        if let Some(Json::Object(last)) = system.last_mut() {
            last.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
        }
        if !system.is_empty() {
            body.insert("system".to_string(), Json::Array(system));
        }
        let mut tools: Vec<Json> = request
            .sorted_tools()
            .iter()
            .map(|tool| json!({"name": tool.name, "description": tool.description, "input_schema": tool.schema()}))
            .collect();
        if let Some(Json::Object(last)) = tools.last_mut() {
            last.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
        }
        if !tools.is_empty() {
            body.insert("tools".to_string(), Json::Array(tools));
        }
        body.insert("messages".to_string(), Json::Array(messages(request, target)));
        body.insert("stream".to_string(), json!(true));
        if let Some(schema) = &request.response_schema {
            body.insert(
                "output_config".to_string(),
                json!({"format": {"type": "json_schema", "schema": schema.schema}}),
            );
        }
        let mut body = Json::Object(body);
        apply_patches(&mut body, target, request.level(target));
        let mut headers = http_headers(target);
        if !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("anthropic-version"))
        {
            headers.push(("anthropic-version".to_string(), API_VERSION.to_string()));
        }
        Ok(HttpCall {
            url: target.endpoint.request_url(),
            headers,
            body,
        })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(AnthropicDecoder::default())
    }
}

fn messages(request: &ModelRequest, target: &Target) -> Vec<Json> {
    let replays = request.replays(target);
    // (role, blocks), consecutive user-side entries merged
    let mut turns: Vec<(&str, Vec<Json>)> = Vec::new();
    let mut push = |role: &'static str, blocks: Vec<Json>| match turns.last_mut() {
        Some((last, known)) if *last == role => known.extend(blocks),
        _ => turns.push((role, blocks)),
    };
    for (index, message) in request.history.iter().enumerate() {
        match message.role {
            Role::User => push("user", vec![json!({"type": "text", "text": request.text_of(index)})]),
            Role::Tool => {
                let mut block = json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
                    "content": message.text,
                });
                if message.is_error {
                    block["is_error"] = json!(true);
                }
                push("user", vec![block]);
            }
            Role::Assistant => {
                let mut blocks = Vec::new();
                if let Some(bag) = replays[index] {
                    for item in &bag.items {
                        match item {
                            ReplayItem::Thinking { thinking, signature } => {
                                blocks.push(json!({"type": "thinking", "thinking": thinking, "signature": signature}));
                            }
                            ReplayItem::RedactedThinking { data } => {
                                blocks.push(json!({"type": "redacted_thinking", "data": data}));
                            }
                            _ => {}
                        }
                    }
                }
                if !message.text.is_empty() {
                    blocks.push(json!({"type": "text", "text": message.text}));
                }
                for call in &message.tool_calls {
                    blocks.push(json!({"type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments}));
                }
                if blocks.is_empty() {
                    blocks.push(json!({"type": "text", "text": ""}));
                }
                push("assistant", blocks);
            }
        }
    }
    let count = turns.len();
    turns
        .into_iter()
        .enumerate()
        .map(|(index, (role, mut blocks))| {
            if index + 1 == count {
                if let Some(Json::Object(last)) = blocks.last_mut() {
                    // a thinking block cannot carry cache_control
                    if !matches!(
                        last.get("type").and_then(Json::as_str),
                        Some("thinking" | "redacted_thinking")
                    ) {
                        last.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
                    }
                }
            }
            json!({"role": role, "content": blocks})
        })
        .collect()
}

#[derive(Debug, Default)]
enum Block {
    #[default]
    Other,
    Text,
    Thinking {
        thinking: String,
        signature: String,
    },
    Redacted(String),
    Tool,
}

#[derive(Debug, Default)]
pub struct AnthropicDecoder {
    blocks: std::collections::BTreeMap<u32, Block>,
    usage: Usage,
    stop_reason: Option<String>,
    has_output: bool,
    done: bool,
}

impl AnthropicDecoder {
    fn note_usage(&mut self, usage: &Json) {
        let cache_read = usage
            .get("cache_read_input_tokens")
            .map(|_| count(usage.get("cache_read_input_tokens")));
        let cache_write = usage
            .get("cache_creation_input_tokens")
            .map(|_| count(usage.get("cache_creation_input_tokens")));
        if let Some(read) = cache_read {
            self.usage.cached_input = read;
        }
        if let Some(write) = cache_write {
            self.usage.cache_write = write;
        }
        if usage.get("input_tokens").is_some() {
            self.usage.input = count(usage.get("input_tokens")) + self.usage.cached_input + self.usage.cache_write;
        } else if cache_read.is_some() || cache_write.is_some() {
            self.usage.input = self.usage.input.max(self.usage.cached_input + self.usage.cache_write);
        }
        if usage.get("output_tokens").is_some() {
            self.usage.output = count(usage.get("output_tokens"));
        }
    }

    fn event(&mut self, kind: &str, event: &Json, out: &mut Vec<ProviderEvent>) {
        if self.done {
            return;
        }
        let index = u32::try_from(count(event.get("index"))).unwrap_or(0);
        match kind {
            "message_start" => {
                let message = event.get("message").unwrap_or(&Json::Null);
                out.push(ProviderEvent::Started {
                    model: text(message.get("model")),
                    id: text(message.get("id")),
                });
                if let Some(usage) = message.get("usage") {
                    self.note_usage(usage);
                }
            }
            "content_block_start" => {
                let block = event.get("content_block").unwrap_or(&Json::Null);
                let started = match block.get("type").and_then(Json::as_str) {
                    Some("text") => {
                        if let Some(delta) = text(block.get("text")) {
                            self.has_output = true;
                            out.push(ProviderEvent::Text { index, delta });
                        }
                        Block::Text
                    }
                    Some("thinking") => {
                        let thinking = text(block.get("thinking")).unwrap_or_default();
                        if !thinking.is_empty() {
                            out.push(ProviderEvent::Reasoning {
                                delta: thinking.clone(),
                            });
                        }
                        Block::Thinking {
                            thinking,
                            signature: text(block.get("signature")).unwrap_or_default(),
                        }
                    }
                    Some("redacted_thinking") => Block::Redacted(text(block.get("data")).unwrap_or_default()),
                    Some("tool_use") => {
                        self.has_output = true;
                        out.push(ProviderEvent::ToolStart {
                            index,
                            call_id: text(block.get("id")).unwrap_or_default(),
                            name: text(block.get("name")).unwrap_or_default(),
                        });
                        if let Some(input) = block
                            .get("input")
                            .filter(|input| input.as_object().is_some_and(|map| !map.is_empty()))
                        {
                            out.push(ProviderEvent::ToolArgs {
                                index,
                                delta: input.to_string(),
                            });
                        }
                        Block::Tool
                    }
                    _ => Block::Other,
                };
                self.blocks.insert(index, started);
            }
            "content_block_delta" => {
                let delta = event.get("delta").unwrap_or(&Json::Null);
                match delta.get("type").and_then(Json::as_str) {
                    Some("text_delta") => {
                        if let Some(delta) = text(delta.get("text")) {
                            self.has_output = true;
                            out.push(ProviderEvent::Text { index, delta });
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(piece) = text(delta.get("thinking")) {
                            if let Some(Block::Thinking { thinking, .. }) = self.blocks.get_mut(&index) {
                                thinking.push_str(&piece);
                            }
                            out.push(ProviderEvent::Reasoning { delta: piece });
                        }
                    }
                    Some("signature_delta") => {
                        if let (Some(piece), Some(Block::Thinking { signature, .. })) =
                            (text(delta.get("signature")), self.blocks.get_mut(&index))
                        {
                            signature.push_str(&piece);
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(piece) = text(delta.get("partial_json")) {
                            out.push(ProviderEvent::ToolArgs { index, delta: piece });
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => match self.blocks.remove(&index) {
                Some(Block::Thinking { thinking, signature }) => {
                    out.push(ProviderEvent::Replay(ReplayItem::Thinking { thinking, signature }));
                }
                Some(Block::Redacted(data)) => out.push(ProviderEvent::Replay(ReplayItem::RedactedThinking { data })),
                Some(Block::Tool) => out.push(ProviderEvent::ToolEnd { index }),
                _ => {}
            },
            "message_delta" => {
                if let Some(reason) = text(event.pointer("/delta/stop_reason")) {
                    self.stop_reason = Some(reason);
                }
                if let Some(usage) = event.get("usage") {
                    self.note_usage(usage);
                }
            }
            "message_stop" => {
                self.done = true;
                out.push(ProviderEvent::Usage(self.usage));
                let finish = match &self.stop_reason {
                    Some(reason) => anthropic_finish(reason),
                    None => Finish::Stop,
                };
                out.push(ProviderEvent::Finish(finish));
            }
            "error" => {
                self.done = true;
                let error = event.get("error").unwrap_or(event);
                let kind = text(error.get("type"));
                let message = text(error.get("message")).unwrap_or_default();
                let mut failure = ProviderError::new(classify_error(None, None, kind.as_deref(), &message), message);
                failure.code = kind;
                if !self.usage.is_zero() {
                    out.push(ProviderEvent::Usage(self.usage));
                }
                out.push(ProviderEvent::Finish(failure.into_finish()));
            }
            _ => {}
        }
    }
}

impl Decoder for AnthropicDecoder {
    fn frame(&mut self, frame: &Frame, out: &mut Vec<ProviderEvent>) {
        let event = match serde_json::from_str::<Json>(&frame.data) {
            Ok(event) => event,
            Err(error) => {
                tracing::debug!("an Anthropic stream frame is not JSON, skipped: {error}");
                return;
            }
        };
        let kind = frame
            .event
            .clone()
            .or_else(|| text(event.get("type")))
            .unwrap_or_default();
        self.event(&kind, &event, out);
    }

    fn end(&mut self, out: &mut Vec<ProviderEvent>) {
        if self.done {
            return;
        }
        self.done = true;
        if !self.usage.is_zero() {
            out.push(ProviderEvent::Usage(self.usage));
        }
        out.push(ProviderEvent::Finish(Finish::Error(ProviderError::new(
            ErrorClass::Network,
            if self.has_output {
                "the stream ended without message_stop"
            } else {
                "the stream ended with no reply"
            },
        ))));
    }

    fn body(&mut self, body: &Json, out: &mut Vec<ProviderEvent>) {
        self.event("message_start", &json!({"message": body}), out);
        if let Some(blocks) = body.get("content").and_then(Json::as_array) {
            for (index, block) in blocks.iter().enumerate() {
                self.event(
                    "content_block_start",
                    &json!({"index": index, "content_block": block}),
                    out,
                );
                self.event("content_block_stop", &json!({"index": index}), out);
            }
        }
        if let Some(reason) = text(body.get("stop_reason")) {
            self.stop_reason = Some(reason);
        }
        self.event("message_stop", &json!({}), out);
    }
}
