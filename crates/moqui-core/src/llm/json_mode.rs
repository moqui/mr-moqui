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

//! JSON tool mode and leaked tool calls (plans/LLM-A2A-Assist.md 7.9).
//!
//! **JSON tool mode** (`tool-mode="json"`, after Dolibarr's JSON-only protocol) is for models with no tool calling or a
//! broken chat template. The tools are compact signatures in the system text ([`tools_section`], in the frozen prefix,
//! every tool every time: no per-category subsets, which would break the cache), and the model answers one object:
//! `{"tool": name, "args": {...}}`, `{"reply": "…"}` or `{"ask": {...}}` (the hidden system tools: a reply and a
//! question are not in the tool list). [`parse_reply`] takes the object only when it is the whole reply (in one code
//! fence at most); the OpenAI `name`/`arguments` form and Qwen/Hermes `<tool_call>` blocks are mapped, and plain text
//! is a reply. The request ([`to_json_mode`]) has
//! no native tools: an assistant's calls go back as the text the model wrote (kept as the message's `raw`), and each
//! result as a user message `<result tool="screen">…</result>`.
//!
//! **Leaked calls** ([`leaked_calls`]): a model in native mode whose reply is only calls written as text (`<tool_call>`
//! blocks, or one bare call object) gets them taken as calls, when each names a tool it was offered. A call inside prose,
//! a code fence or quotes is one the model quoted (a record's text, say) and never runs; in JSON mode such a reply is not
//! valid and is asked again.

use serde_json::{json, Map, Value as Json};

use super::provider::{Message, ModelRequest, Role, ToolSpec};

/// What a JSON-mode reply is.
#[derive(Clone, Debug, PartialEq)]
pub enum Parsed {
    /// A tool call (an `ask` object is a call of `ask`).
    Call { name: String, args: Json },
    /// An answer to the user.
    Reply(String),
    /// It meant to be an object and is not one the protocol knows: why, for the one retry.
    Invalid(String),
}

/// The note of the one retry of an invalid JSON-mode reply.
pub const RETRY_NOTE: &str = "Your last reply was not one JSON object of the protocol. Answer with exactly one object: \
{\"tool\": name, \"args\": {...}}, {\"reply\": \"...\"} or {\"ask\": {...}}.";

/// The tools section of a JSON-mode prompt: the protocol, then one signature a tool, sorted by name.
pub fn tools_section(protocol: &str, tools: &[ToolSpec]) -> String {
    let mut sorted: Vec<&ToolSpec> = tools.iter().collect();
    sorted.sort_by(|left, right| left.name.cmp(&right.name));
    let mut text = protocol.trim().to_string();
    for tool in sorted {
        text.push('\n');
        text.push_str(&signature(tool));
    }
    text
}

/// A tool as one line: `name(arg: type, arg?: type): description`, `?` marking what may be left out.
pub fn signature(tool: &ToolSpec) -> String {
    let schema = tool.schema();
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Json::as_array)
        .map(|names| names.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default();
    let mut args = Vec::new();
    if let Some(properties) = schema.get("properties").and_then(Json::as_object) {
        for (name, property) in properties {
            let optional = if required.contains(&name.as_str()) { "" } else { "?" };
            args.push(format!("{name}{optional}: {}", type_of(property)));
        }
    }
    format!("- {}({}): {}", tool.name, args.join(", "), tool.description.trim())
}

/// A property's type in a signature: an enum's values, `string[]`, or the JSON type.
fn type_of(property: &Json) -> String {
    if let Some(values) = property.get("enum").and_then(Json::as_array) {
        let shown: Vec<String> = values.iter().take(8).map(Json::to_string).collect();
        return shown.join("|");
    }
    let kind = match property.get("type") {
        Some(Json::String(kind)) => kind.clone(),
        Some(Json::Array(kinds)) => kinds
            .iter()
            .filter_map(Json::as_str)
            .filter(|kind| *kind != "null")
            .collect::<Vec<_>>()
            .join("|"),
        _ if property.get("properties").is_some() => "object".to_string(),
        _ => "any".to_string(),
    };
    if kind == "array" {
        let item = property.get("items").map_or_else(|| "any".to_string(), type_of);
        return format!("{item}[]");
    }
    kind
}

/// `request` as a JSON-mode model gets it: no native tools; the tail in the text of the newest user message; each
/// assistant message as the text the model wrote (its calls as protocol objects when there is no such text); each tool
/// result (consecutive ones together) as a user message of `<result>` blocks.
pub fn to_json_mode(request: &ModelRequest) -> ModelRequest {
    let mut out = ModelRequest {
        tools: Vec::new(),
        history: Vec::with_capacity(request.history.len()),
        tail: None,
        ..request.clone()
    };
    for (index, message) in request.history.iter().enumerate() {
        match message.role {
            Role::User => out.history.push(Message::user(request.text_of(index).into_owned())),
            Role::Assistant => {
                let text = match &message.raw {
                    Some(raw) => raw.clone(),
                    None => assistant_text(message),
                };
                out.history.push(Message {
                    text,
                    tool_calls: Vec::new(),
                    raw: None,
                    ..message.clone()
                });
            }
            Role::Tool => {
                let block = result_block(message);
                match out.history.last_mut() {
                    Some(last) if last.role == Role::User && index > 0 && request.history[index - 1].role == Role::Tool => {
                        last.text.push('\n');
                        last.text.push_str(&block);
                    }
                    _ => out.history.push(Message::user(block)),
                }
            }
        }
    }
    out
}

/// An assistant message as protocol text: its text, then one object a call.
fn assistant_text(message: &Message) -> String {
    let mut lines = Vec::new();
    if !message.text.trim().is_empty() {
        lines.push(if message.tool_calls.is_empty() {
            json!({"reply": message.text}).to_string()
        } else {
            message.text.clone()
        });
    }
    for call in &message.tool_calls {
        lines.push(json!({"tool": call.name, "args": call.arguments}).to_string());
    }
    lines.join("\n")
}

/// A tool result as `<result tool="name">…</result>`, its own closing markers elided.
pub fn result_block(message: &Message) -> String {
    let name: String = message
        .tool_name
        .as_deref()
        .unwrap_or("tool")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '#'))
        .collect();
    let error = if message.is_error { " error=\"true\"" } else { "" };
    let body = super::prompt::elide_markers(&message.text);
    format!("<result tool=\"{name}\"{error}>\n{body}\n</result>")
}

/// The protocol object of a JSON-mode reply: a call, a reply, or why it is not valid. The reply must be the object
/// alone (in one code fence at most), or only `<tool_call>` blocks; an object inside prose is never taken as a call (the
/// model may be quoting one), so a reply with one is not valid and is asked again. Plain text with no object is a reply.
/// The OpenAI `name`/`arguments` form is a call too.
pub fn parse_reply(text: &str) -> Parsed {
    let trimmed = strip_fences(text.trim());
    if trimmed.is_empty() {
        return Parsed::Invalid("the reply is empty".to_string());
    }
    if let Some(blocks) = only_tool_call_blocks(trimmed) {
        return match blocks.first().and_then(object_call) {
            Some((name, args)) => Parsed::Call { name, args },
            None => Parsed::Invalid("the <tool_call> block is not a call object".to_string()),
        };
    }
    let Some((start, end)) = first_object(trimmed) else {
        if trimmed.starts_with('{') {
            return Parsed::Invalid("the object is not closed".to_string());
        }
        return Parsed::Reply(trimmed.to_string());
    };
    let whole = start == 0 && end == trimmed.len();
    if !whole {
        // prose around an object: a protocol object only alone; prose that holds braces is the answer
        return if looks_like_protocol(&trimmed[start..end]) {
            Parsed::Invalid("the object is not alone in the reply".to_string())
        } else {
            Parsed::Reply(trimmed.to_string())
        };
    }
    let value = match super::provider::parse_arguments(trimmed) {
        Ok(value) => value,
        Err(error) => return Parsed::Invalid(error),
    };
    let Json::Object(object) = value else {
        return Parsed::Reply(trimmed.to_string());
    };
    if let Some(reply) = object.get("reply") {
        return Parsed::Reply(match reply {
            Json::String(text) => text.clone(),
            other => other.to_string(),
        });
    }
    if let Some(ask) = object.get("ask") {
        let args = match ask {
            Json::Object(_) => ask.clone(),
            Json::String(question) => json!({"question": question}),
            other => json!({"question": other.to_string()}),
        };
        return Parsed::Call {
            name: "ask".to_string(),
            args,
        };
    }
    match object_call(&Json::Object(object)) {
        Some((name, args)) => Parsed::Call { name, args },
        None => Parsed::Invalid("the object has no tool, reply or ask".to_string()),
    }
}

/// Whether an object's text names a protocol key (`tool`, `reply`, `ask`, `name` with `arguments`).
fn looks_like_protocol(object: &str) -> bool {
    match super::provider::parse_arguments(object) {
        Ok(Json::Object(map)) => {
            ["tool", "reply", "ask", "function"].iter().any(|key| map.contains_key(*key))
                || (map.contains_key("name") && (map.contains_key("arguments") || map.contains_key("parameters")))
        }
        _ => false,
    }
}

/// A call object's tool and arguments: `{"tool", "args"}`, `{"name", "arguments"|"parameters"}`, or
/// `{"function": {"name", "arguments"}}`; arguments written as a JSON string are read.
fn object_call(value: &Json) -> Option<(String, Json)> {
    let object = value.as_object()?;
    if let Some(function) = object.get("function").and_then(Json::as_object) {
        return object_call(&Json::Object(function.clone()));
    }
    let name = ["tool", "name", "tool_name"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Json::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    let args = ["args", "arguments", "parameters", "input"]
        .iter()
        .find_map(|key| object.get(*key))
        .cloned()
        .unwrap_or_else(|| Json::Object(Map::new()));
    let args = match args {
        Json::String(text) => super::provider::parse_arguments(&text).ok()?,
        Json::Null => Json::Object(Map::new()),
        other @ Json::Object(_) => other,
        _ => return None,
    };
    Some((name.to_string(), args))
}

/// The tool calls a native-mode reply wrote as text, when it made none: a reply that is only `<tool_call>` blocks (with
/// whitespace between), or only one bare call object, each naming a tool in `tools`. A call quoted in prose, in a code
/// fence or in quotes is text the model wrote about, never run. Out: the calls and the text left (none).
pub fn leaked_calls(text: &str, tools: &[&str]) -> Option<(Vec<(String, Json)>, String)> {
    let trimmed = text.trim();
    let calls: Vec<(String, Json)> = match only_tool_call_blocks(trimmed) {
        Some(blocks) => blocks.iter().filter_map(object_call).collect(),
        None => {
            if !trimmed.starts_with('{') {
                return None;
            }
            let (start, end) = first_object(trimmed)?;
            if start != 0 || end != trimmed.len() {
                return None;
            }
            vec![object_call(&super::provider::parse_arguments(trimmed).ok()?)?]
        }
    };
    if calls.is_empty() || !calls.iter().all(|(name, _)| tools.contains(&name.as_str())) {
        return None;
    }
    Some((calls, String::new()))
}

/// The objects of a text that is only `<tool_call>…</tool_call>` blocks (whitespace between, each block one object),
/// else `None`.
fn only_tool_call_blocks(text: &str) -> Option<Vec<Json>> {
    let mut rest = text.trim();
    if !rest.starts_with("<tool_call>") {
        return None;
    }
    let mut out = Vec::new();
    while !rest.is_empty() {
        let after = rest.strip_prefix("<tool_call>")?;
        let close = after.find("</tool_call>")?;
        let inner = after[..close].trim();
        let (start, end) = first_object(inner)?;
        if start != 0 || end != inner.len() {
            return None;
        }
        out.push(super::provider::parse_arguments(inner).ok()?);
        rest = after[close + "</tool_call>".len()..].trim_start();
    }
    Some(out)
}

/// Text inside a Markdown code fence, the fence and its language left out.
fn strip_fences(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    let rest = rest.trim_start_matches(|ch: char| ch.is_ascii_alphanumeric());
    rest.trim().strip_suffix("```").unwrap_or(rest).trim()
}

/// The byte range of the first balanced `{…}` of `text`, strings and their escapes respected.
pub fn first_object(text: &str) -> Option<(usize, usize)> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, start + offset + 1));
                }
            }
            _ => {}
        }
    }
    None
}

/// The intent words of the optional pre-call (`intent-prefilter="hint"`, after Nextcloud's prototype trick).
pub const INTENTS: [&str; 5] = ["answer", "screen", "data", "service", "ui"];

/// The intent a pre-call answered: the first of [`INTENTS`] its text names.
pub fn parse_intent(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    lower
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .find_map(|word| INTENTS.iter().find(|intent| **intent == word).copied())
}

/// `tail` with a `likely: <intent>` line before its closing marker.
pub fn tail_with_intent(tail: &str, intent: &str) -> String {
    match tail.rfind("</context>") {
        Some(at) => format!("{}likely: {intent}\n{}", &tail[..at], &tail[at..]),
        None => format!("{tail}\nlikely: {intent}"),
    }
}

#[cfg(test)]
#[path = "json_mode_tests.rs"]
mod tests;
