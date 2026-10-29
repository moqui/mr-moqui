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

use super::*;

fn refused(kind: Kind, body: Json) -> (u16, Json) {
    let reply = prepare(kind, body.to_string().as_bytes()).unwrap_err();
    (reply.status, serde_json::from_str(&reply.body).unwrap())
}

fn target() -> Target {
    Target {
        endpoint: crate::conf::EndpointConf::new("e", ProtocolId::ChatCompletions, "http://127.0.0.1:9"),
        model: crate::conf::ModelConf::new("m", "e", "provider-name"),
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
    assert!(
        ok.prompt_chars > "userhi".len(),
        "the tools count too: {}",
        ok.prompt_chars
    );

    let (status, body) = refused(
        Kind::Chat,
        json!({"model": "fast", "messages": [{"role": "user"}], "base_url": "http://x"}),
    );
    assert_eq!(
        (status, body["error"]["code"].as_str(), body["error"]["param"].as_str()),
        (400, Some("unsupported_parameter"), Some("base_url"))
    );
    let (_, body) = refused(
        Kind::Chat,
        json!({"model": "fast", "messages": [{"role": "user"}], "web_search_options": {}}),
    );
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
    let (_, body) = refused(
        Kind::Chat,
        json!({"model": "m", "messages": [{"role": "user"}], "n": 8}),
    );
    assert_eq!(
        (body["error"]["param"].as_str(), body["error"]["code"].as_str()),
        (Some("n"), Some("unsupported_value"))
    );
    let (_, body) = refused(
        Kind::Chat,
        json!({"model": "m", "messages": [{"role": "user"}], "max_tokens": -1}),
    );
    assert_eq!(body["error"]["param"], "max_tokens");
    let (_, body) = refused(
        Kind::Responses,
        json!({"model": "m", "input": "x", "previous_response_id": "r1"}),
    );
    assert!(body["error"]["message"].as_str().unwrap().contains("store: false"));
    let (_, body) = refused(Kind::Responses, json!({"model": "m", "input": "x", "store": true}));
    assert_eq!(body["error"]["param"], "store");
    let (_, body) = refused(
        Kind::Responses,
        json!({"model": "m", "input": "x", "tools": [{"type": "mcp", "server_url": "x"}]}),
    );
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
    target.model.reasoning.insert(
        ReasoningLevel::High,
        json!({"chat_template_kwargs": {"enable_thinking": true}}),
    );
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
    assert_eq!(
        body["messages"][0]["content"], "hi",
        "a patch cannot change the messages"
    );
    assert_eq!(body["seed"], 7);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], true);
    assert_eq!(
        body["stream_options"],
        json!({"include_usage": true}),
        "the gateway's stream options win"
    );
    assert_eq!(
        (body["max_tokens"].as_u64(), body["max_completion_tokens"].as_u64()),
        (Some(2000), Some(50))
    );
    let whole = upstream_body(&prepared, &target, None);
    assert!(whole.get("chat_template_kwargs").is_none());
    let responses = prepare(
        Kind::Responses,
        json!({"model": "m", "input": "x"}).to_string().as_bytes(),
    )
    .unwrap();
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
    assert!(
        !reply.body.contains("10.1.2.3") && reply.body.contains("HTTP 422"),
        "{}",
        reply.body
    );
    error.class = ErrorClass::RateLimited;
    error.retry_after = Some(Duration::from_secs(7));
    let reply = upstream_error(&error, "m");
    assert_eq!(reply.status, 429);
    assert!(reply
        .headers
        .iter()
        .any(|(name, value)| name == "retry-after" && value == "7"));
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
            ProviderEvent::Text {
                index: 0,
                delta: "Hello".to_string(),
            },
            ProviderEvent::ToolArgs {
                index: 0,
                delta: "{}".to_string(),
            },
        ],
        &mut outcome,
    );
    assert_eq!(outcome.output_chars, 7);
    assert!(outcome.usage.is_zero());
}
