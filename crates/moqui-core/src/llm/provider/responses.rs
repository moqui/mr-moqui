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

//! The OpenAI Responses API, also xAI's (from Java's `OpenAiResponsesProtocol`). Stateless: `store` is always false
//! and `include` always asks for `reasoning.encrypted_content`, so a reasoning item can be replayed; each reasoning item
//! of a reply is in its replay bag and goes back, before the message's own items, to the same endpoint and model only
//! (encrypted reasoning made by one deployment is refused by another). The conversation id is the `prompt_cache_key`;
//! a server that does not take it can drop it with `{"prompt_cache_key":null}` in the model's api-config. The
//! temperature is left out when a reasoning effort other than none is set, since those models refuse it.

use std::collections::BTreeMap;

use moqui_lib::error::Error;
use serde_json::{json, Map, Value as Json};

use super::classify::{classify_error, ErrorClass, Finish, ProviderError};
use super::sse::Frame;
use super::{
    apply_patches, count, http_headers, text, Decoder, HttpCall, ModelRequest, Protocol, ProviderEvent, ReplayItem,
    Role,
};
use crate::llm::registry::Target;
use crate::llm::usage::Usage;

pub struct Responses;

const INCLUDE_REASONING: &str = "reasoning.encrypted_content";

impl Protocol for Responses {
    fn id(&self) -> &'static str {
        "responses"
    }

    fn encode(&self, request: &ModelRequest, target: &Target) -> Result<HttpCall, Error> {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(target.model.name));
        let instructions = request.system_text();
        if !instructions.is_empty() {
            body.insert("instructions".to_string(), json!(instructions));
        }
        body.insert("input".to_string(), Json::Array(input(request, target)));
        if let Some(max) = request.max_output(target) {
            body.insert("max_output_tokens".to_string(), json!(max));
        }
        if let Some(temperature) = target.model.behavior.temperature {
            body.insert("temperature".to_string(), json!(temperature));
        }
        body.insert("stream".to_string(), json!(true));
        let tools = request.sorted_tools();
        if !tools.is_empty() {
            let tools: Vec<Json> = tools
                .iter()
                .map(|tool| json!({"type": "function", "name": tool.name, "description": tool.description, "parameters": tool.schema()}))
                .collect();
            body.insert("tools".to_string(), Json::Array(tools));
            if let Some(parallel) = target.model.behavior.parallel_tool_calls {
                body.insert("parallel_tool_calls".to_string(), json!(parallel));
            }
        }
        if let Some(schema) = &request.response_schema {
            body.insert(
                "text".to_string(),
                json!({"format": {"type": "json_schema", "name": schema.name, "schema": schema.schema, "strict": schema.strict}}),
            );
        }
        if let Some(key) = &request.cache_key {
            body.insert("prompt_cache_key".to_string(), json!(key));
        }
        let mut body = Json::Object(body);
        apply_patches(&mut body, target, request.level(target));
        if let Json::Object(map) = &mut body {
            map.insert("store".to_string(), json!(false));
            let mut include: Vec<Json> = match map.get("include") {
                Some(Json::Array(items)) => items.clone(),
                Some(Json::String(item)) => vec![json!(item)],
                _ => Vec::new(),
            };
            if !include.iter().any(|item| item.as_str() == Some(INCLUDE_REASONING)) {
                include.push(json!(INCLUDE_REASONING));
            }
            map.insert("include".to_string(), Json::Array(include));
            let effort = map
                .get("reasoning")
                .and_then(|reasoning| reasoning.get("effort"))
                .or_else(|| map.get("reasoning_effort"))
                .and_then(Json::as_str)
                .map(str::to_string);
            if effort.is_some_and(|effort| !effort.is_empty() && effort != "none") {
                map.remove("temperature");
            }
        }
        Ok(HttpCall {
            url: target.endpoint.request_url(),
            headers: http_headers(target),
            body,
        })
    }

    fn decoder(&self) -> Box<dyn Decoder> {
        Box::new(ResponsesDecoder::default())
    }
}

fn input(request: &ModelRequest, target: &Target) -> Vec<Json> {
    let mut out = Vec::new();
    let replays = request.replays(target);
    for (index, message) in request.history.iter().enumerate() {
        match message.role {
            Role::User => out.push(json!({"type": "message", "role": "user", "content": request.text_of(index)})),
            Role::Tool => out.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.clone().unwrap_or_default(),
                "output": message.text,
            })),
            Role::Assistant => {
                if let Some(bag) = replays[index] {
                    for item in &bag.items {
                        if let ReplayItem::ResponsesReasoning { item } = item {
                            out.push(item.clone());
                        }
                    }
                }
                if !message.text.is_empty() {
                    out.push(json!({"type": "message", "role": "assistant", "content": message.text}));
                }
                for call in &message.tool_calls {
                    out.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.name,
                        "arguments": call.arguments.to_string(),
                    }));
                }
            }
        }
    }
    out
}

#[derive(Debug, Default)]
struct Call {
    started: bool,
    /// Whether arguments came as deltas, so the done item does not add them again.
    had_deltas: bool,
}

#[derive(Debug, Default)]
pub struct ResponsesDecoder {
    started: bool,
    /// By output index.
    calls: BTreeMap<u32, Call>,
    item_index: BTreeMap<String, u32>,
    has_text: bool,
    done: bool,
}

impl ResponsesDecoder {
    fn index_of(&mut self, event: &Json) -> u32 {
        if let Some(index) = event.get("output_index").and_then(Json::as_u64) {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            if let Some(item) = text(event.get("item_id")).or_else(|| text(event.pointer("/item/id"))) {
                self.item_index.insert(item, index);
            }
            return index;
        }
        text(event.get("item_id"))
            .and_then(|item| self.item_index.get(&item).copied())
            .unwrap_or(0)
    }

    fn start(&mut self, response: Option<&Json>, out: &mut Vec<ProviderEvent>) {
        if !self.started {
            self.started = true;
            out.push(ProviderEvent::Started {
                model: text(response.and_then(|response| response.get("model"))),
                id: text(response.and_then(|response| response.get("id"))),
            });
        }
    }

    fn event(&mut self, kind: &str, event: &Json, out: &mut Vec<ProviderEvent>) {
        if self.done {
            return;
        }
        match kind {
            "response.created" | "response.in_progress" => self.start(event.get("response"), out),
            "response.output_text.delta" | "response.refusal.delta" => {
                self.start(None, out);
                if let Some(delta) = text(event.get("delta")) {
                    self.has_text = true;
                    let index = self.index_of(event);
                    out.push(ProviderEvent::Text { index, delta });
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = text(event.get("delta")) {
                    out.push(ProviderEvent::Reasoning { delta });
                }
            }
            "response.output_item.added" => {
                let index = self.index_of(event);
                let item = event.get("item");
                if item.and_then(|item| item.get("type")).and_then(Json::as_str) == Some("function_call") {
                    let item = item.unwrap_or(&Json::Null);
                    let call = self.calls.entry(index).or_default();
                    if !call.started {
                        call.started = true;
                        out.push(ProviderEvent::ToolStart {
                            index,
                            call_id: text(item.get("call_id"))
                                .or_else(|| text(item.get("id")))
                                .unwrap_or_default(),
                            name: text(item.get("name")).unwrap_or_default(),
                        });
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                let index = self.index_of(event);
                if let Some(delta) = text(event.get("delta")) {
                    self.calls.entry(index).or_default().had_deltas = true;
                    out.push(ProviderEvent::ToolArgs { index, delta });
                }
            }
            "response.output_item.done" => {
                let index = self.index_of(event);
                let Some(item) = event.get("item") else {
                    return;
                };
                match item.get("type").and_then(Json::as_str) {
                    Some("function_call") => {
                        let call = self.calls.entry(index).or_default();
                        if !call.started {
                            call.started = true;
                            out.push(ProviderEvent::ToolStart {
                                index,
                                call_id: text(item.get("call_id"))
                                    .or_else(|| text(item.get("id")))
                                    .unwrap_or_default(),
                                name: text(item.get("name")).unwrap_or_default(),
                            });
                        }
                        if !call.had_deltas {
                            if let Some(arguments) = text(item.get("arguments")) {
                                out.push(ProviderEvent::ToolArgs {
                                    index,
                                    delta: arguments,
                                });
                            }
                        }
                        out.push(ProviderEvent::ToolEnd { index });
                    }
                    Some("reasoning") => out.push(ProviderEvent::Replay(ReplayItem::ResponsesReasoning {
                        item: replayable(item),
                    })),
                    Some("message") if !self.has_text => {
                        // text that came only in the done item
                        let text = message_text(item);
                        if !text.is_empty() {
                            self.has_text = true;
                            out.push(ProviderEvent::Text { index, delta: text });
                        }
                    }
                    _ => {}
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                let response = event.get("response").unwrap_or(event);
                self.start(Some(response), out);
                self.done = true;
                if let Some(usage) = response.get("usage").filter(|usage| usage.is_object()) {
                    out.push(ProviderEvent::Usage(usage_of(usage)));
                }
                out.push(ProviderEvent::Finish(finish_of(kind, response)));
            }
            "error" => {
                self.done = true;
                let error = event.get("error").unwrap_or(event);
                let code = text(error.get("code"));
                let kind = text(error.get("type"));
                let message = text(error.get("message")).unwrap_or_default();
                let mut failure = ProviderError::new(
                    classify_error(None, code.as_deref(), kind.as_deref(), &message),
                    message,
                );
                failure.code = code.or(kind);
                out.push(ProviderEvent::Finish(failure.into_finish()));
            }
            _ => {}
        }
    }
}

/// A reasoning item as it goes back: its summary emptied when the encrypted content carries it.
fn replayable(item: &Json) -> Json {
    let mut item = item.clone();
    if let Json::Object(map) = &mut item {
        if map.get("encrypted_content").is_some_and(|content| !content.is_null()) {
            map.insert("summary".to_string(), json!([]));
        }
        map.remove("status");
    }
    item
}

fn message_text(item: &Json) -> String {
    item.get("content")
        .and_then(Json::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| matches!(part.get("type").and_then(Json::as_str), Some("output_text" | "text")))
                .filter_map(|part| part.get("text").and_then(Json::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn finish_of(kind: &str, response: &Json) -> Finish {
    let status = text(response.get("status")).unwrap_or_default();
    let reason = text(response.pointer("/incomplete_details/reason")).unwrap_or_default();
    if kind == "response.failed" || status == "failed" || status == "cancelled" {
        let error = response.get("error").unwrap_or(&Json::Null);
        let code = text(error.get("code"));
        let message = text(error.get("message")).unwrap_or_else(|| format!("the response {status}"));
        let mut failure = ProviderError::new(classify_error(None, code.as_deref(), None, &message), message);
        failure.code = code;
        return failure.into_finish();
    }
    if kind == "response.incomplete" || status == "incomplete" {
        return if reason.contains("content_filter") {
            Finish::ContentFilter
        } else {
            Finish::Length
        };
    }
    // tool calls or text decide it in settle
    Finish::Stop
}

/// Usage from a Responses `usage` object.
pub fn usage_of(usage: &Json) -> Usage {
    Usage {
        input: count(usage.get("input_tokens")),
        cached_input: count(usage.pointer("/input_tokens_details/cached_tokens")),
        cache_write: 0,
        output: count(usage.get("output_tokens")),
        reasoning: count(usage.pointer("/output_tokens_details/reasoning_tokens")),
    }
}

impl Decoder for ResponsesDecoder {
    fn frame(&mut self, frame: &Frame, out: &mut Vec<ProviderEvent>) {
        let event = match serde_json::from_str::<Json>(&frame.data) {
            Ok(event) => event,
            Err(error) => {
                tracing::debug!("a Responses stream frame is not JSON, skipped: {error}");
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
        out.push(ProviderEvent::Finish(Finish::Error(ProviderError::new(
            ErrorClass::Network,
            if self.has_text || !self.calls.is_empty() {
                "the stream ended without response.completed"
            } else {
                "the stream ended with no reply"
            },
        ))));
    }

    fn body(&mut self, body: &Json, out: &mut Vec<ProviderEvent>) {
        self.start(Some(body), out);
        if let Some(items) = body.get("output").and_then(Json::as_array) {
            for (index, item) in items.iter().enumerate() {
                let event = json!({"output_index": index, "item": item});
                self.event("response.output_item.done", &event, out);
            }
        }
        let status = text(body.get("status")).unwrap_or_default();
        let kind = match status.as_str() {
            "incomplete" => "response.incomplete",
            "failed" | "cancelled" => "response.failed",
            _ => "response.completed",
        };
        self.event(kind, &json!({"response": body}), out);
    }
}
