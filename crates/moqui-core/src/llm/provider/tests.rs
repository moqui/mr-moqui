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

//! The providers: the fixtures of `testdata/` played through an installed `HttpSender` against their goldens (set
//! `UPDATE_GOLDEN=1` to rewrite them, then read the diff), the request each protocol encodes, the reasoning patches and
//! the replay rule, tool arguments, and the call driver's retries, re-ask, nudge, compaction, cancel and idle limit on
//! the scripted fake; `ec.llm.complete` with a schema on the in-memory database.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};

use super::anthropic::AnthropicMessages;
use super::chat_completions::ChatCompletions;
use super::fake::{self, Step};
use super::responses::Responses;
use super::*;
use crate::llm::conf::{EndpointConf, LlmConf, ModelConf, ProtocolId, ReasoningLevel, ReasoningReplay};
use crate::llm::registry::Target;
use crate::llm::usage::CallSource;
use crate::outbound::{self, HttpSender, Request, Response};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/llm/provider/testdata")
}

fn target(id: &str, protocol: ProtocolId, url: &str) -> Target {
    let mut endpoint = EndpointConf::new(id, protocol, url);
    endpoint.api_key = Some(crate::llm::conf::Secret("sk-test-0123456789abcdefghij".to_string()));
    let mut model = ModelConf::new(format!("{id}-model"), id, "model-1");
    model.behavior.reasoning_replay = ReasoningReplay::All;
    Target {
        endpoint,
        model,
        reasoning: ReasoningLevel::None,
        feature: None,
        fell_back_from: None,
    }
}

const FIXTURE_HOST: &str = "https://fixture.llm.test/";

/// Answers the fixture host with the named `.sse` file and passes any other request on, as a test elsewhere in the
/// crate may make one meanwhile.
struct FixtureServer {
    seen: Mutex<Vec<Request>>,
}

impl HttpSender for FixtureServer {
    fn send(&self, request: Request) -> Result<Response, moqui_lib::error::Error> {
        let Some(rest) = request.url.strip_prefix(FIXTURE_HOST) else {
            return outbound::send_default(request);
        };
        let name = rest.split('/').next().unwrap_or("").to_string();
        self.seen.lock().unwrap().push(request);
        let body = std::fs::read_to_string(testdata().join(format!("{name}.sse")))
            .unwrap_or_else(|error| panic!("fixture {name}: {error}"));
        Ok(Response {
            status: 200,
            body,
            bytes: Vec::new(),
            headers: vec![("content-type".to_string(), "text/event-stream".to_string())],
        })
    }
}

fn simple_request() -> ModelRequest {
    ModelRequest {
        system: vec!["You help.".to_string()],
        tools: vec![ToolSpec::new("search", "Search.", Json::Null)],
        history: vec![Message::user("Find the open orders")],
        ..ModelRequest::default()
    }
}

fn reply_summary(reply: &Reply) -> Json {
    json!({
        "model": reply.model,
        "response_id": reply.response_id,
        "text": reply.text,
        "reasoning": reply.reasoning,
        "tool_calls": reply.tool_calls,
        "bad_tool_calls": reply.bad_tool_calls.iter().map(|call| json!({"id": call.id, "name": call.name, "raw": call.raw_arguments, "error": call.error})).collect::<Vec<_>>(),
        "cut_tool_calls": reply.cut_tool_calls,
        "replay": reply.replay.as_ref().map(|bag| json!(bag.items)),
        "usage": {"input": reply.usage.input, "cached_input": reply.usage.cached_input, "cache_write": reply.usage.cache_write,
            "output": reply.usage.output, "reasoning": reply.usage.reasoning},
        "finish": ProviderEvent::Finish(reply.finish.clone()).to_json(),
        "text_deltas": reply.text_deltas,
    })
}

fn check_golden(name: &str, actual: &Json) {
    let path = testdata().join(format!("{name}.events.json"));
    let text = serde_json::to_string_pretty(actual).unwrap() + "\n";
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(text, expected, "{name} differs from {}", path.display());
}

const FIXTURES: [(&str, ProtocolId); 12] = [
    ("lmstudio-qwen-tool", ProtocolId::ChatCompletions),
    ("llamacpp-reasoning", ProtocolId::ChatCompletions),
    ("ollama-parallel-index0", ProtocolId::ChatCompletions),
    ("vllm-reasoning-tools", ProtocolId::ChatCompletions),
    ("deepseek-reasoning-content", ProtocolId::ChatCompletions),
    ("gemini-thought-signature", ProtocolId::ChatCompletions),
    ("chat-truncated", ProtocolId::ChatCompletions),
    ("openai-responses-reasoning-tool", ProtocolId::Responses),
    ("xai-responses-text", ProtocolId::Responses),
    ("responses-incomplete", ProtocolId::Responses),
    ("anthropic-thinking-tool", ProtocolId::AnthropicMessages),
    ("anthropic-overloaded", ProtocolId::AnthropicMessages),
];

#[test]
fn every_fixture_decodes_to_its_golden_events_through_an_installed_sender() {
    let _turn = crate::user::session::cookie_property_lock();
    let server = Arc::new(FixtureServer {
        seen: Mutex::new(Vec::new()),
    });
    outbound::install_http_sender(server.clone());
    // every fixture is listed
    let mut files: Vec<String> = std::fs::read_dir(testdata())
        .unwrap()
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_str()?
                .strip_suffix(".sse")
                .map(str::to_string)
        })
        .collect();
    files.sort();
    let mut listed: Vec<String> = FIXTURES.iter().map(|(name, _)| (*name).to_string()).collect();
    listed.sort();
    assert_eq!(files, listed);
    for (name, protocol) in FIXTURES {
        let mut target = target("fixture", protocol, &format!("{FIXTURE_HOST}{name}"));
        target.endpoint.path = Some("/stream".to_string());
        let mut events = Vec::new();
        let reply = {
            let mut sink = |signal: Signal<'_>| {
                if let Signal::Event(event) = signal {
                    events.push(event.to_json());
                }
            };
            let mut options = CallOptions::new(CallSource::Test);
            options.max_attempts = 1;
            options.length_retry = false;
            options.sink = Some(&mut sink);
            call(&target, &simple_request(), &mut options)
        };
        assert_eq!(reply.attempts, 1, "{name}");
        check_golden(name, &json!({"events": events, "reply": reply_summary(&reply)}));
    }
    let seen = server.seen.lock().unwrap();
    assert_eq!(seen.len(), FIXTURES.len());
    let header = |request: &Request, name: &str| {
        request
            .headers
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    };
    let chat = &seen[0];
    assert_eq!(chat.url, format!("{FIXTURE_HOST}lmstudio-qwen-tool/stream"));
    assert_eq!(
        header(chat, "authorization").as_deref(),
        Some("Bearer sk-test-0123456789abcdefghij")
    );
    assert_eq!(header(chat, "accept").as_deref(), Some("text/event-stream"));
    let anthropic = seen.last().unwrap();
    assert_eq!(
        header(anthropic, "x-api-key").as_deref(),
        Some("sk-test-0123456789abcdefghij")
    );
    assert_eq!(header(anthropic, "anthropic-version").as_deref(), Some("2023-06-01"));
    assert!(header(anthropic, "authorization").is_none());
}

/// A request with every part: system blocks, unsorted tools, an earlier assistant turn with another model's replay,
/// a tool turn of this turn with this model's replay, and the tail.
fn full_request(target: &Target, items: Vec<ReplayItem>) -> ModelRequest {
    let other = ReplayBag {
        endpoint_id: "elsewhere".to_string(),
        model_id: "other".to_string(),
        items: vec![ReplayItem::ReasoningContent {
            text: "not mine".to_string(),
        }],
    };
    let mine = ReplayBag {
        endpoint_id: target.endpoint.id.clone(),
        model_id: target.model.id.clone(),
        items,
    };
    ModelRequest {
        system: vec!["Core.".to_string(), "Agent.".to_string()],
        tools: vec![
            ToolSpec::new(
                "screen",
                "Open a screen.",
                json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            ),
            ToolSpec::new("data", "Read rows.", Json::Null),
        ],
        history: vec![
            Message::user("Hello"),
            Message {
                replay: Some(other),
                ..Message::assistant("Hi.")
            },
            Message::user("Find the open orders"),
            Message {
                tool_calls: vec![ToolCall {
                    id: "call_1".to_string(),
                    name: "data".to_string(),
                    arguments: json!({"entity": "Order"}),
                }],
                replay: Some(mine),
                ..Message::assistant("")
            },
            Message::tool_result("call_1", "data", "3 rows"),
        ],
        tail: Some("<context>now Mon</context>".to_string()),
        cache_key: Some("CONV1".to_string()),
        ..ModelRequest::default()
    }
}

#[test]
fn chat_completions_encodes_the_history_the_replay_and_the_patches() {
    let mut target = target("chat", ProtocolId::ChatCompletions, "http://127.0.0.1:1234");
    target.model.api_config = Some(json!({"chat_template_kwargs": {"preserve_thinking": true}, "top_k": 20}));
    target.model.reasoning.insert(
        ReasoningLevel::Low,
        json!({"reasoning_effort": "low", "chat_template_kwargs": {"enable_thinking": false}, "top_k": null,
            "messages": "replaced?", "stream_options": {"include_usage": false}}),
    );
    target.reasoning = ReasoningLevel::Low;
    target.model.behavior.temperature = Some(0.3);
    let mut extra = Map::new();
    extra.insert(
        "extra_content".to_string(),
        json!({"google": {"thought_signature": "SIG"}}),
    );
    let request = full_request(
        &target,
        vec![
            ReplayItem::ReasoningContent {
                text: "I look it up.".to_string(),
            },
            ReplayItem::ToolCallExtra {
                call_id: "call_1".to_string(),
                fields: extra,
            },
        ],
    );
    let call = ChatCompletions.encode(&request, &target).unwrap();
    assert_eq!(call.url, "http://127.0.0.1:1234/v1/chat/completions");
    let body = call.body;
    assert_eq!(body["model"], "model-1");
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0], json!({"role": "system", "content": "Core.\n\nAgent."}));
    // another model's replay is not sent; this one's is, with the call's extra fields
    assert_eq!(messages[2], json!({"role": "assistant", "content": "Hi."}));
    assert_eq!(messages[4]["reasoning_content"], "I look it up.");
    assert_eq!(messages[4]["content"], Json::Null);
    assert_eq!(
        messages[4]["tool_calls"],
        json!([{"id": "call_1", "type": "function", "function": {"name": "data", "arguments": "{\"entity\":\"Order\"}"},
            "extra_content": {"google": {"thought_signature": "SIG"}}}])
    );
    assert_eq!(
        messages[5],
        json!({"role": "tool", "tool_call_id": "call_1", "content": "3 rows"})
    );
    // the tail goes on the newest user message only
    assert_eq!(
        messages[3]["content"],
        "Find the open orders\n\n<context>now Mon</context>"
    );
    assert_eq!(messages[1]["content"], "Hello");
    let names: Vec<&str> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["data", "screen"]);
    assert_eq!(
        body["tools"][0]["function"]["parameters"],
        json!({"type": "object", "properties": {}})
    );
    // the patches: nested objects combine, null removes, the messages stay, other keys replace
    assert_eq!(
        body["chat_template_kwargs"],
        json!({"preserve_thinking": true, "enable_thinking": false})
    );
    assert_eq!(body["reasoning_effort"], "low");
    assert!(body.get("top_k").is_none());
    assert!(body["messages"].is_array());
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": false}));
    assert_eq!(body["temperature"], 0.3);

    // reasoning-replay none sends no replay, last only the turn in progress's
    target.model.behavior.reasoning_replay = ReasoningReplay::None;
    let body = ChatCompletions.encode(&request, &target).unwrap().body;
    assert!(body["messages"][4].get("reasoning_content").is_none());
    target.model.behavior.reasoning_replay = ReasoningReplay::Last;
    assert!(request.replays(&target)[3].is_some());
    let mut later = request.clone();
    later.history.push(Message::user("And the closed ones?"));
    assert!(later.replays(&target)[3].is_none());
}

#[test]
fn responses_encodes_stateless_with_reasoning_items_and_no_temperature_with_effort() {
    let mut target = target("resp", ProtocolId::Responses, "https://api.openai.com");
    target.model.behavior.temperature = Some(0.2);
    target.model.reasoning.insert(
        ReasoningLevel::Medium,
        json!({"reasoning": {"effort": "medium"}, "store": true}),
    );
    let reasoning = json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "ENC"});
    let mut request = full_request(
        &target,
        vec![ReplayItem::ResponsesReasoning {
            item: reasoning.clone(),
        }],
    );
    request.reasoning = Some(ReasoningLevel::Medium);
    request.response_schema = Some(ResponseSchema {
        name: "answer".to_string(),
        schema: json!({"type": "object"}),
        strict: true,
    });
    let body = Responses.encode(&request, &target).unwrap().body;
    assert_eq!(body["store"], false);
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
    assert_eq!(body["instructions"], "Core.\n\nAgent.");
    assert_eq!(body["prompt_cache_key"], "CONV1");
    assert_eq!(body["reasoning"], json!({"effort": "medium"}));
    assert!(body.get("temperature").is_none());
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    let input = body["input"].as_array().unwrap();
    let kinds: Vec<&str> = input.iter().map(|item| item["type"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        [
            "message",
            "message",
            "message",
            "reasoning",
            "function_call",
            "function_call_output"
        ]
    );
    assert_eq!(input[3], reasoning);
    assert_eq!(input[4]["call_id"], "call_1");
    assert_eq!(
        input[2]["content"],
        "Find the open orders\n\n<context>now Mon</context>"
    );
    // to another model the reasoning item is not sent
    let mut elsewhere = target.clone();
    elsewhere.model.id = "resp-other".to_string();
    let body = Responses.encode(&request, &elsewhere).unwrap().body;
    assert!(!body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "reasoning"));
}

#[test]
fn anthropic_encodes_cache_marks_thinking_first_and_merged_tool_results() {
    let mut target = target("claude", ProtocolId::AnthropicMessages, "https://api.anthropic.com");
    target
        .model
        .reasoning
        .insert(ReasoningLevel::High, json!({"output_config": {"effort": "high"}}));
    target.reasoning = ReasoningLevel::High;
    let mut request = full_request(
        &target,
        vec![
            ReplayItem::Thinking {
                thinking: String::new(),
                signature: "SIG".to_string(),
            },
            ReplayItem::RedactedThinking {
                data: "RED".to_string(),
            },
        ],
    );
    request.history.push(Message::user("Thanks"));
    request.response_schema = Some(ResponseSchema {
        name: "answer".to_string(),
        schema: json!({"type": "object", "properties": {"n": {"type": "integer"}}}),
        strict: false,
    });
    let call = AnthropicMessages.encode(&request, &target).unwrap();
    assert_eq!(call.url, "https://api.anthropic.com/v1/messages");
    assert!(call
        .headers
        .contains(&("x-api-key".to_string(), "sk-test-0123456789abcdefghij".to_string())));
    let body = call.body;
    assert_eq!(body["max_tokens"], 16000);
    assert_eq!(
        body["system"],
        json!([{"type": "text", "text": "Core."}, {"type": "text", "text": "Agent.", "cache_control": {"type": "ephemeral"}}])
    );
    assert_eq!(body["tools"][0]["name"], "data");
    assert!(body["tools"][0].get("cache_control").is_none());
    assert_eq!(body["tools"][1]["cache_control"], json!({"type": "ephemeral"}));
    // effort and the schema share output_config
    assert_eq!(
        body["output_config"],
        json!({"format": {"type": "json_schema", "schema": {"type": "object", "properties": {"n": {"type": "integer"}}}}, "effort": "high"})
    );
    let messages = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant", "user"]);
    assert_eq!(messages[1]["content"], json!([{"type": "text", "text": "Hi."}]));
    assert_eq!(
        messages[3]["content"],
        json!([
            {"type": "thinking", "thinking": "", "signature": "SIG"},
            {"type": "redacted_thinking", "data": "RED"},
            {"type": "tool_use", "id": "call_1", "name": "data", "input": {"entity": "Order"}}
        ])
    );
    // the tool result and the next user message are one user turn, the last block marked for the cache; the tail goes
    // on the newest user message
    assert_eq!(
        messages[4]["content"],
        json!([
            {"type": "tool_result", "tool_use_id": "call_1", "content": "3 rows"},
            {"type": "text", "text": "Thanks\n\n<context>now Mon</context>", "cache_control": {"type": "ephemeral"}}
        ])
    );
    assert_eq!(messages[2]["content"][0]["text"], "Find the open orders");
}

#[test]
fn tool_arguments_parse_leniently_then_strictly() {
    let ok: &[(&str, Json)] = &[
        ("", json!({})),
        ("  {\"a\": 1} ", json!({"a": 1})),
        ("\"{\\\"a\\\": 2}\"", json!({"a": 2})),
        ("```json\n{\"a\": [1, 2,],}\n```", json!({"a": [1, 2]})),
        (
            "{'city': 'Paris', 'now': True, 'zone': None}",
            json!({"city": "Paris", "now": true, "zone": null}),
        ),
        ("{\"text\": \"True, None,}\"}", json!({"text": "True, None,}"})),
    ];
    for (raw, expected) in ok {
        assert_eq!(&parse_arguments(raw).unwrap(), expected, "{raw}");
    }
    for raw in ["{\"a\": 1", "[1, 2]", "42", "not json"] {
        assert!(parse_arguments(raw).is_err(), "{raw}");
    }
}

#[test]
fn deep_merge_combines_objects_replaces_the_rest_and_null_removes() {
    let mut dest = json!({"a": {"b": 1, "c": [1]}, "d": 1, "e": "x"});
    let patch = json!({"a": {"c": [2], "f": {"g": true}}, "d": null, "e": {"h": 1}});
    if let (Json::Object(dest), Json::Object(patch)) = (&mut dest, &patch) {
        deep_merge(dest, patch);
    }
    assert_eq!(dest, json!({"a": {"b": 1, "c": [2], "f": {"g": true}}, "e": {"h": 1}}));
}

#[test]
fn schema_checks_and_reply_json() {
    let schema = json!({"type": "object", "required": ["name", "tags"], "additionalProperties": false,
        "properties": {"name": {"type": "string"}, "tags": {"type": "array", "items": {"enum": ["a", "b"]}},
            "count": {"type": ["integer", "null"]}}});
    assert!(check_schema(&json!({"name": "x", "tags": ["a"], "count": null}), &schema).is_ok());
    let fails = [
        (json!({"name": "x"}), "$.tags is required"),
        (json!({"name": 1, "tags": []}), "$.name must be string"),
        (json!({"name": "x", "tags": ["c"]}), "$.tags[0] must be one of"),
        (json!({"name": "x", "tags": [], "more": 1}), "$.more is not allowed"),
        (
            json!({"name": "x", "tags": [], "count": 1.5}),
            "$.count must be one of the types",
        ),
    ];
    for (value, expected) in fails {
        let error = check_schema(&value, &schema).unwrap_err();
        assert!(error.starts_with(expected), "{value}: {error}");
    }
    assert_eq!(reply_json("```json\n{\"a\": 1}\n```").unwrap(), json!({"a": 1}));
    assert_eq!(reply_json("Here it is: {\"a\": 1}. Done").unwrap(), json!({"a": 1}));
    assert!(reply_json("no json here").is_err());
}

fn fake_target(id: &str) -> Target {
    fake::clear(id);
    target(id, ProtocolId::Fake, "")
}

fn fast<'a>(source: CallSource) -> CallOptions<'a> {
    let mut options = CallOptions::new(source);
    options.idle = Some(Duration::from_millis(300));
    options
}

#[test]
fn the_fake_follows_the_request_and_streams_in_pieces() {
    let target = fake_target("fake-follow");
    let mut options = fast(CallSource::Test);
    let reply = call(&target, &simple_request(), &mut options);
    assert_eq!(reply.finish, Finish::Stop);
    assert_eq!(reply.text, "Fake reply to: Find the open orders");
    assert!(reply.text_deltas > 3);
    // a named tool is called with a sample of its schema
    let mut request = simple_request();
    request.history = vec![Message::user("Use search to find them")];
    let reply = call(&target, &request, &mut options);
    assert_eq!(reply.finish, Finish::ToolCalls);
    assert_eq!(reply.tool_calls[0].name, "search");
    assert_eq!(reply.tool_calls[0].arguments, json!({}));
    // with a reasoning level it reasons and gives a replay, which goes back to the same model only
    request.reasoning = Some(ReasoningLevel::Low);
    let reply = call(&target, &request, &mut options);
    let bag = reply.replay.clone().unwrap();
    assert_eq!(
        (bag.endpoint_id.as_str(), bag.model_id.as_str()),
        ("fake-follow", "fake-follow-model")
    );
    assert_eq!(reply.reasoning, "Thinking it over.");
    request.history.push(reply.assistant_message());
    request
        .history
        .push(Message::tool_result(&reply.tool_calls[0].id, "search", "3 orders"));
    let after = call(&target, &request, &mut options);
    assert_eq!(after.text, "Done: 3 orders");
    let sent = fake::requests("fake-follow");
    assert_eq!(sent.last().unwrap()["messages"][1]["replay"], json!(bag.items));
    // a schema is answered with a sample; the cache key reports the earlier input as cached
    let schema_request = ModelRequest {
        history: vec![Message::user("Classify")],
        response_schema: Some(ResponseSchema {
            name: "label".to_string(),
            schema: json!({"type": "object", "properties": {"label": {"enum": ["spam", "ham"]}, "score": {"type": "number"}}}),
            strict: true,
        }),
        cache_key: Some("fake-follow-cache".to_string()),
        ..ModelRequest::default()
    };
    let first = call(&target, &schema_request, &mut options);
    assert_eq!(reply_json_text(&first), json!({"label": "spam", "score": 1}));
    assert_eq!(first.usage.cached_input, 0);
    let second = call(&target, &schema_request, &mut options);
    assert_eq!(second.usage.cached_input, second.usage.input);
}

fn reply_json_text(reply: &Reply) -> Json {
    serde_json::from_str(&reply.text).unwrap()
}

#[test]
fn a_retryable_error_is_tried_again_and_the_attempts_end_at_three() {
    let target = fake_target("fake-retry");
    fake::script(
        "fake-retry",
        vec![
            fake::Reply::status(429, r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#)
                .header("retry-after", "0"),
            fake::Reply::steps(vec![
                Step::Text("Hal".to_string()),
                Step::Error {
                    kind: "overloaded_error".to_string(),
                    message: "Overloaded".to_string(),
                },
            ]),
            fake::Reply::text("Hello"),
        ],
    );
    let mut restarts = Vec::new();
    let reply = {
        let mut sink = |signal: Signal<'_>| {
            if let Signal::Restart { attempt, reason } = signal {
                restarts.push((attempt, reason.to_string()));
            }
        };
        let mut options = fast(CallSource::Test);
        options.sink = Some(&mut sink);
        call(&target, &simple_request(), &mut options)
    };
    assert_eq!(
        (reply.finish.clone(), reply.text.as_str(), reply.attempts),
        (Finish::Stop, "Hello", 3)
    );
    assert_eq!(
        restarts,
        [
            (2, "retry after rate_limited".to_string()),
            (3, "retry after overloaded".to_string())
        ]
    );

    // three server errors in a row end the call; a bad request is not tried again
    fake::script(
        "fake-retry",
        vec![
            fake::Reply::status(500, "").header("retry-after", "0"),
            fake::Reply::status(502, "").header("retry-after", "0"),
            fake::Reply::status(503, "").header("retry-after", "0"),
            fake::Reply::text("never"),
        ],
    );
    let reply = call(&target, &simple_request(), &mut fast(CallSource::Test));
    assert_eq!(reply.attempts, 3);
    assert_eq!(
        reply.finish.error().map(|error| (error.class, error.status)),
        Some((ErrorClass::Overloaded, Some(503)))
    );
    fake::clear("fake-retry");
    fake::script(
        "fake-retry",
        vec![
            fake::Reply::status(401, r#"{"error":{"message":"bad key"}}"#),
            fake::Reply::text("x"),
        ],
    );
    let reply = call(&target, &simple_request(), &mut fast(CallSource::Test));
    assert_eq!(
        (reply.attempts, reply.finish.error().map(|error| error.class)),
        (1, Some(ErrorClass::Auth))
    );
    // a stream cut short is tried again
    fake::clear("fake-retry");
    fake::script(
        "fake-retry",
        vec![
            fake::Reply::steps(vec![Step::Text("partial".to_string()), Step::Cut]),
            fake::Reply::text("whole"),
        ],
    );
    let started = Instant::now();
    let reply = call(&target, &simple_request(), &mut fast(CallSource::Test));
    assert_eq!((reply.text.as_str(), reply.attempts), ("whole", 2));
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn length_keeps_complete_calls_and_asks_again_once_when_nothing_is_usable() {
    let target = fake_target("fake-length");
    let tool = |id: &str, arguments: &str| Step::Tool {
        id: id.to_string(),
        name: "search".to_string(),
        arguments: arguments.to_string(),
        pieces: 3,
    };
    fake::script(
        "fake-length",
        vec![fake::Reply::steps(vec![
            tool("c1", "{\"query\":\"orders\"}"),
            tool("c2", "{\"query\":\"cust"),
            Step::Finish("length".to_string()),
        ])],
    );
    let reply = call(&target, &simple_request(), &mut fast(CallSource::Test));
    assert_eq!(reply.finish, Finish::Length);
    assert_eq!(
        (reply.tool_calls.len(), reply.cut_tool_calls, reply.attempts),
        (1, 1, 1)
    );
    assert_eq!(reply.tool_calls[0].arguments, json!({"query": "orders"}));

    // a cut with no complete call is asked again with twice the output
    fake::script(
        "fake-length",
        vec![
            fake::Reply::steps(vec![
                Step::Text("The long".to_string()),
                Step::Finish("length".to_string()),
            ]),
            fake::Reply::text("Short."),
        ],
    );
    let mut request = simple_request();
    request.max_output = Some(100);
    let reply = call(&target, &request, &mut fast(CallSource::Test));
    assert_eq!((reply.text.as_str(), reply.attempts), ("Short.", 2));
    let sent = fake::requests("fake-length");
    assert_eq!(sent[sent.len() - 1]["max_output"], 200);
    assert_eq!(sent[sent.len() - 2]["max_output"], 100);

    // only once
    fake::script(
        "fake-length",
        vec![
            fake::Reply::steps(vec![Step::Text("a".to_string()), Step::Finish("length".to_string())]),
            fake::Reply::steps(vec![Step::Text("b".to_string()), Step::Finish("length".to_string())]),
        ],
    );
    let reply = call(&target, &request, &mut fast(CallSource::Test));
    assert_eq!((reply.finish.clone(), reply.attempts), (Finish::Length, 2));
}

#[test]
fn an_empty_reply_gets_one_nudge_and_an_overflow_one_compaction() {
    let target = fake_target("fake-nudge");
    let empty = || fake::Reply::steps(vec![Step::Finish("stop".to_string())]);
    fake::script("fake-nudge", vec![empty(), fake::Reply::text("Here.")]);
    let mut options = fast(CallSource::Test);
    options.nudge = Some(EMPTY_NUDGE.to_string());
    let reply = call(&target, &simple_request(), &mut options);
    assert_eq!((reply.text.as_str(), reply.nudged, reply.attempts), ("Here.", true, 2));
    let sent = fake::requests("fake-nudge");
    assert_eq!(sent.last().unwrap()["messages"][1]["text"], EMPTY_NUDGE);
    fake::script("fake-nudge", vec![empty(), empty()]);
    let reply = call(&target, &simple_request(), &mut options);
    assert_eq!((reply.finish.clone(), reply.attempts), (Finish::Empty, 2));
    // without a nudge an empty reply is the answer
    fake::script("fake-nudge", vec![empty()]);
    assert_eq!(
        call(&target, &simple_request(), &mut fast(CallSource::Test)).attempts,
        1
    );

    fake::script(
        "fake-nudge",
        vec![
            fake::Reply::status(
                400,
                r#"{"error":{"code":"context_length_exceeded","message":"too long"}}"#,
            ),
            fake::Reply::text("Fits now."),
        ],
    );
    let mut compactions = 0;
    let reply = {
        let mut compact = |request: &ModelRequest| {
            compactions += 1;
            let mut smaller = request.clone();
            smaller.system = vec!["Short.".to_string()];
            Some(smaller)
        };
        let mut options = fast(CallSource::Test);
        options.compact = Some(&mut compact);
        call(&target, &simple_request(), &mut options)
    };
    assert_eq!((reply.text.as_str(), compactions), ("Fits now.", 1));
    assert_eq!(fake::requests("fake-nudge").last().unwrap()["system"], "Short.");
    // with no hook the overflow is the answer
    fake::script("fake-nudge", vec![fake::Reply::status(413, "prompt is too long")]);
    assert_eq!(
        call(&target, &simple_request(), &mut fast(CallSource::Test)).finish,
        Finish::ContextOverflow
    );
}

#[test]
fn a_cancel_or_a_silent_model_ends_the_call_without_waiting_for_it() {
    let target = fake_target("fake-cancel");
    fake::script(
        "fake-cancel",
        vec![fake::Reply::steps(vec![
            Step::Text("Start".to_string()),
            Step::Hang(5000),
            Step::Text("late".to_string()),
        ])],
    );
    let mut options = fast(CallSource::Test);
    options.idle = Some(Duration::from_secs(30));
    let cancel = options.cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        cancel.cancel();
    });
    let started = Instant::now();
    let reply = call(&target, &simple_request(), &mut options);
    assert_eq!(
        (reply.finish.clone(), reply.text.as_str(), reply.attempts),
        (Finish::Canceled, "Start", 1)
    );
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());

    // no bytes within the idle limit is a retryable timeout; the deadline ends the retries
    fake::script(
        "fake-cancel",
        vec![
            fake::Reply::steps(vec![Step::Hang(3000), Step::Finish("stop".to_string())]),
            fake::Reply::steps(vec![Step::Hang(3000), Step::Finish("stop".to_string())]),
        ],
    );
    let mut options = fast(CallSource::Test);
    options.idle = Some(Duration::from_millis(200));
    options.deadline = Some(Instant::now() + Duration::from_millis(900));
    let started = Instant::now();
    let reply = call(&target, &simple_request(), &mut options);
    assert_eq!(reply.finish.error().map(|error| error.class), Some(ErrorClass::Timeout));
    assert!(
        reply.attempts <= 2 && started.elapsed() < Duration::from_secs(2),
        "{} {:?}",
        reply.attempts,
        started.elapsed()
    );
}

#[test]
fn bad_tool_arguments_are_reported_and_a_cut_stream_with_calls_is_a_failure() {
    let target = fake_target("fake-bad");
    fake::script(
        "fake-bad",
        vec![fake::Reply::steps(vec![
            Step::Tool {
                id: "c1".to_string(),
                name: "search".to_string(),
                arguments: "{'q': 'x',}".to_string(),
                pieces: 2,
            },
            Step::Tool {
                id: "c2".to_string(),
                name: "search".to_string(),
                arguments: "{oops".to_string(),
                pieces: 1,
            },
            Step::Finish("tool_calls".to_string()),
        ])],
    );
    let reply = call(&target, &simple_request(), &mut fast(CallSource::Test));
    assert_eq!(reply.finish, Finish::ToolCalls);
    assert_eq!(reply.tool_calls[0].arguments, json!({"q": "x"}));
    assert_eq!(reply.bad_tool_calls.len(), 1);
    assert!(
        reply.bad_tool_calls[0].error.starts_with("the arguments are not JSON"),
        "{}",
        reply.bad_tool_calls[0].error
    );
}

#[test]
fn a_private_address_needs_allow_private_network_and_loopback_is_allowed() {
    let mut target = target("lan", ProtocolId::ChatCompletions, "http://192.168.1.20:1234");
    let reply = call(&target, &simple_request(), &mut fast(CallSource::Test));
    let error = reply.finish.error().unwrap();
    assert_eq!(error.class, ErrorClass::BadRequest);
    assert!(error.message.contains("allow-private-network"), "{error}");
    target.endpoint.allow_private_network = true;
    assert!(check_address(&target).is_ok());
    for url in [
        "http://127.0.0.1:1234",
        "http://[::1]:8080",
        "http://[::ffff:127.0.0.1]:1234",
        "http://2130706433:1234",
        "http://localhost:1234",
        "https://api.openai.com",
    ] {
        assert!(
            check_address(&self::target("ok", ProtocolId::ChatCompletions, url)).is_ok(),
            "{url}"
        );
    }
    for url in [
        "http://10.0.0.5",
        "http://[fd00::1]:80",
        "http://169.254.169.254/latest",
        // a numeric host and IPv4 written in hex are the address they stand for
        "http://2852039166/",
        "http://0xa9.0xfe.0xa9.0xfe/",
        // an IPv4-mapped IPv6 address is the IPv4 one
        "http://[::ffff:10.0.0.1]:8080",
    ] {
        assert!(
            check_address(&self::target("no", ProtocolId::ChatCompletions, url)).is_err(),
            "{url}"
        );
    }
}

#[test]
fn an_endpoint_takes_no_more_calls_than_its_limit() {
    let mut target = fake_target("fake-slots");
    target.endpoint.max_concurrent = Some(1);
    let replies = (0..3)
        .map(|_| {
            fake::Reply::steps(vec![
                Step::Delay(150),
                Step::Text("x".to_string()),
                Step::Finish("stop".to_string()),
            ])
        })
        .collect();
    fake::script("fake-slots", replies);
    let started = Instant::now();
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let target = target.clone();
            std::thread::spawn(move || call(&target, &simple_request(), &mut fast(CallSource::Test)).finish)
        })
        .collect();
    for handle in handles {
        assert_eq!(handle.join().unwrap(), Finish::Stop);
    }
    // one at a time: three pauses in a row
    assert!(
        started.elapsed() >= Duration::from_millis(440),
        "{:?}",
        started.elapsed()
    );
}

const COMPLETE_CONF: &str = r#"<llm-facade>
    <endpoint id="complete-fake" protocol="fake"/>
    <model id="complete-model" endpoint="complete-fake" name="fake-1" price-input="2" price-output="8"/>
    <bind feature="classify" model="complete-model" reasoning="none"/>
    <content-log enabled="true"/>
</llm-facade>"#;

#[test]
fn complete_answers_with_a_structured_reply_and_logs_the_call() {
    let facade = crate::testing::open();
    let llm = crate::llm::LlmFacade::new(LlmConf::from_conf_text(COMPLETE_CONF).unwrap());
    fake::clear("complete-fake");
    let schema = json!({"type": "object", "required": ["label", "confidence"], "additionalProperties": false,
        "properties": {"label": {"enum": ["invoice", "receipt", "other"]}, "confidence": {"type": "number"}}});
    let ask = crate::llm::CompleteRequest {
        feature: Some("classify".to_string()),
        system: Some("Classify the document.".to_string()),
        prompt: Some("Invoice #12 from Acme, due in 30 days".to_string()),
        schema: Some(schema.clone()),
        ..crate::llm::CompleteRequest::default()
    };
    // the fake answers a sample of the schema it was sent
    let answer = llm.complete(&facade, &ask).unwrap();
    assert_eq!(answer.json, Some(json!({"label": "invoice", "confidence": 1})));
    assert_eq!(
        (answer.model_id.as_str(), answer.finish.as_str(), answer.call_ids.len()),
        ("complete-model", "stop", 1)
    );
    assert!(answer.cost.is_some_and(|cost| cost > rust_decimal::Decimal::ZERO));
    let sent = fake::requests("complete-fake");
    assert_eq!(sent[0]["schema"], schema);
    let row = facade
        .find("moqui.llm.LlmCall")
        .unwrap()
        .eq("callId", moqui_lib::object::Value::string(&answer.call_ids[0]))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(crate::llm::text_of(&row, "source").as_deref(), Some("api"));
    assert_eq!(crate::llm::text_of(&row, "finishReason").as_deref(), Some("stop"));
    assert!(crate::llm::text_of(&row, "responseJson").is_some_and(|events| events.contains("\"finish\"")));

    // a reply that does not fit is asked again once with what was wrong
    fake::script(
        "complete-fake",
        vec![
            fake::Reply::text("{\"label\": \"bill\", \"confidence\": 0.9}"),
            fake::Reply::text("```json\n{\"label\": \"invoice\", \"confidence\": 0.9}\n```"),
        ],
    );
    let answer = llm.complete(&facade, &ask).unwrap();
    assert_eq!(answer.json, Some(json!({"label": "invoice", "confidence": 0.9})));
    assert_eq!(answer.call_ids.len(), 2);
    let last = fake::requests("complete-fake").pop().unwrap();
    assert!(last["messages"][2]["text"]
        .as_str()
        .unwrap()
        .contains("$.label must be one of"));
    fake::script(
        "complete-fake",
        vec![fake::Reply::text("nope"), fake::Reply::text("still nope")],
    );
    let error = llm.complete(&facade, &ask).unwrap_err();
    assert!(error.to_string().contains("does not follow the schema"), "{error}");

    // a model that cannot take a schema gets it in its instructions
    crate::llm::registry::store_verdict(
        &facade,
        "complete-model",
        "json-schema",
        crate::llm::registry::Verdict::No,
        crate::llm::registry::VerdictSource::Admin,
        None,
    )
    .unwrap();
    fake::script(
        "complete-fake",
        vec![fake::Reply::text("{\"label\": \"receipt\", \"confidence\": 0.5}")],
    );
    let answer = llm.complete(&facade, &ask).unwrap();
    assert_eq!(answer.json.unwrap()["label"], "receipt");
    let last = fake::requests("complete-fake").pop().unwrap();
    assert!(last["schema"].is_null());
    assert!(last["system"].as_str().unwrap().contains("follows this JSON schema"));

    // an error is an error; plain text needs no schema
    fake::script(
        "complete-fake",
        vec![fake::Reply::status(400, r#"{"error":{"message":"bad"}}"#)],
    );
    assert!(llm
        .complete(&facade, &ask)
        .unwrap_err()
        .to_string()
        .contains("bad_request"));
    let plain = crate::llm::CompleteRequest {
        model: Some("complete-model".to_string()),
        prompt: Some("Say hi".to_string()),
        ..crate::llm::CompleteRequest::default()
    };
    assert_eq!(llm.complete(&facade, &plain).unwrap().text, "Fake reply to: Say hi");
    let empty = crate::llm::CompleteRequest::default();
    assert!(llm.complete(&facade, &empty).is_err());
}

#[test]
fn a_cancel_while_waiting_for_a_slot_is_canceled() {
    let mut target = fake_target("fake-slot-cancel");
    target.endpoint.max_concurrent = Some(1);
    fake::script(
        "fake-slot-cancel",
        vec![fake::Reply::steps(vec![
            Step::Delay(600),
            Step::Text("x".to_string()),
            Step::Finish("stop".to_string()),
        ])],
    );
    let busy = {
        let target = target.clone();
        std::thread::spawn(move || call(&target, &simple_request(), &mut fast(CallSource::Test)).finish)
    };
    std::thread::sleep(Duration::from_millis(100));
    let mut options = fast(CallSource::Test);
    let cancel = options.cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        cancel.cancel();
    });
    let reply = call(&target, &simple_request(), &mut options);
    assert_eq!((reply.finish.clone(), reply.attempts), (Finish::Canceled, 1));
    assert_eq!(busy.join().unwrap(), Finish::Stop);
}

#[test]
fn a_completion_keeps_its_usage_when_the_callers_transaction_rolls_back() {
    let facade = crate::testing::open();
    let llm = crate::llm::LlmFacade::new(
        LlmConf::from_conf_text(&COMPLETE_CONF.replace("complete-fake", "rollback-fake")).unwrap(),
    );
    fake::clear("rollback-fake");
    let transactions = facade.transactions_for("moqui.llm.LlmCall").unwrap();
    let mut call_ids = Vec::new();
    let result: Result<(), moqui_lib::error::Error> =
        transactions.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
            assert!(transactions.in_transaction());
            // the provider call runs with the transaction suspended (a debug build asserts it), then resumes it
            let answer = llm.complete(
                &facade,
                &crate::llm::CompleteRequest {
                    model: Some("complete-model".to_string()),
                    prompt: Some("Hello".to_string()),
                    ..crate::llm::CompleteRequest::default()
                },
            )?;
            assert!(transactions.in_transaction());
            call_ids = answer.call_ids;
            let mut written = facade.make_value("moqui.basic.EnumerationType")?;
            written.set("enumTypeId", moqui_lib::object::Value::string("LlmRollbackProbe"));
            written.create(&facade)?;
            Err(moqui_lib::error::Error::new("roll back"))
        });
    assert!(result.is_err());
    assert_eq!(call_ids.len(), 1);
    // the caller's row is gone, the call's is not
    let find = |entity: &str, field: &str, id: &str| {
        facade
            .find(entity)
            .unwrap()
            .eq(field, moqui_lib::object::Value::string(id))
            .one()
            .unwrap()
    };
    assert!(find("moqui.basic.EnumerationType", "enumTypeId", "LlmRollbackProbe").is_none());
    assert!(find("moqui.llm.LlmCall", "callId", &call_ids[0]).is_some());
}

#[test]
fn a_completion_ends_by_its_timeout() {
    let facade = crate::testing::open();
    let llm = crate::llm::LlmFacade::new(
        LlmConf::from_conf_text(&COMPLETE_CONF.replace("complete-fake", "timeout-fake")).unwrap(),
    );
    fake::clear("timeout-fake");
    fake::script(
        "timeout-fake",
        vec![fake::Reply::steps(vec![
            Step::Hang(3000),
            Step::Finish("stop".to_string()),
        ])],
    );
    let started = Instant::now();
    let error = llm
        .complete(
            &facade,
            &crate::llm::CompleteRequest {
                model: Some("complete-model".to_string()),
                prompt: Some("Hello".to_string()),
                timeout: Some(Duration::from_millis(300)),
                ..crate::llm::CompleteRequest::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("timeout"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
}
