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

//! The A2A wire rules that need no database (ported from the Java `A2ACoreTests` and `A2AJsonRpcTests`): the
//! envelope, the version header, the strict one-of of parts with their caps, the configuration, the cursor, the error
//! shapes of both bindings, the advertised URL against forwarded headers, and the push URL checks. The tasks, streams,
//! idempotency, ownership, paging and push delivery over HTTP are `crates/moqui-host/tests/it/a2a.rs`.

use serde_json::json;

use super::card::{base_url_of, valid_base_url};
use super::jsonrpc::envelope;
use super::types::{self, check_version, parse_config, parse_message, parse_push, A2aError, Cursor, PartLimits};
use super::Origin;
use crate::llm::conf::LlmConf;
use crate::llm::run::RunState;

const LIMITS: PartLimits = PartLimits {
    max_parts: 3,
    max_part_bytes: 16,
};

fn user(parts: serde_json::Value) -> serde_json::Value {
    json!({"messageId": "m1", "role": "ROLE_USER", "parts": parts})
}

#[test]
fn the_envelope_keeps_its_id_and_takes_no_batch() {
    let (id, method, params) = envelope(br#"{"jsonrpc":"2.0","id":7,"method":"GetTask"}"#).unwrap();
    assert_eq!((id, method.as_str(), params), (json!(7), "GetTask", json!({})));
    let (id, _, _) = envelope(br#"{"jsonrpc":"2.0","id":"a-1","method":"GetTask","params":{"id":"x"}}"#).unwrap();
    assert_eq!(id, json!("a-1"));
    let code = |body: &[u8]| envelope(body).unwrap_err().1.code;
    assert_eq!(code(br#"{"jsonrpc":"#), -32700);
    assert_eq!(code(br#"[{"jsonrpc":"2.0","id":4,"method":"GetTask"}]"#), -32600);
    assert_eq!(code(br#""text""#), -32600);
    assert_eq!(code(br#"{"jsonrpc":"1.0","id":1,"method":"GetTask"}"#), -32600);
    assert_eq!(code(br#"{"jsonrpc":"2.0","id":1}"#), -32600);
    assert_eq!(code(br#"{"jsonrpc":"2.0","id":{},"method":"GetTask"}"#), -32600);
    let (id, error) = envelope(br#"{"jsonrpc":"2.0","id":3,"method":"GetTask","params":[1]}"#).unwrap_err();
    assert_eq!(
        (id, error.code),
        (json!(3), -32602),
        "the id of a request read so far is kept"
    );
}

#[test]
fn the_version_header_is_strict() {
    assert!(check_version(Some("1.0")).is_ok());
    assert!(check_version(Some(" 1.0.2 ")).is_ok());
    for refused in [None, Some(""), Some("0.3"), Some("1.1"), Some("2.0"), Some("1.0.x")] {
        assert_eq!(check_version(refused).unwrap_err().code, -32009, "{refused:?}");
    }
}

#[test]
fn a_part_is_exactly_one_of_text_raw_url_and_data() {
    let message = parse_message(
        Some(&user(json!([{"text": "hi"}, {"data": {"a": 1}}, {"data": null}]))),
        LIMITS,
    )
    .unwrap();
    assert_eq!(message.text, "hi\n{\"a\":1}\nnull", "a null data part is JSON null");
    assert_eq!(message.data, Some(json!({"a": 1})));
    let refused = |parts: serde_json::Value| parse_message(Some(&user(parts)), LIMITS).unwrap_err();
    assert_eq!(refused(json!([{"text": "a", "data": 1}])).code, -32602);
    assert_eq!(refused(json!([{}])).code, -32602);
    assert_eq!(refused(json!([])).code, -32602);
    assert_eq!(refused(json!([{"text": 1}])).code, -32602);
    assert_eq!(
        refused(json!([{"text": "a"}, {"text": "b"}, {"text": "c"}, {"text": "d"}])).code,
        -32602,
        "max parts"
    );
    assert_eq!(
        refused(json!([{"text": "seventeen bytes!!"}])).code,
        -32602,
        "max part bytes"
    );
    assert_eq!(refused(json!([{"raw": "not base64!"}])).code, -32602);
    assert_eq!(
        refused(json!([{"url": "https://example.com/x"}])).code,
        -32602,
        "only this server's content"
    );
    assert_eq!(refused(json!([{"text": "a", "mediaType": 3}])).code, -32602);
    let raw = parse_message(Some(&user(json!([{"raw": "aGVsbG8=", "filename": "a.txt"}]))), LIMITS).unwrap();
    assert_eq!(raw.text, "[Attached file a.txt, 5 bytes, not kept]");
    let url = parse_message(Some(&user(json!([{"url": "dbresource://a/b.txt"}]))), LIMITS).unwrap();
    assert_eq!(url.locations, ["dbresource://a/b.txt"]);
    // a message has an id and is the user's
    let no_id = json!({"role": "ROLE_USER", "parts": [{"text": "a"}]});
    assert_eq!(parse_message(Some(&no_id), LIMITS).unwrap_err().code, -32602);
    let agent = json!({"messageId": "m", "role": "ROLE_AGENT", "parts": [{"text": "a"}]});
    assert_eq!(parse_message(Some(&agent), LIMITS).unwrap_err().code, -32602);
    let blank = json!({"messageId": "m", "role": "ROLE_USER", "parts": [{"text": "a"}], "taskId": " "});
    assert_eq!(parse_message(Some(&blank), LIMITS).unwrap_err().code, -32602);
}

#[test]
fn the_configuration_is_checked_before_a_task_is_made() {
    assert_eq!(
        parse_config(Some(&json!({"acceptedOutputModes": ["image/png"]})))
            .unwrap_err()
            .code,
        -32005
    );
    let config = parse_config(Some(&json!({"acceptedOutputModes": ["text/plain"], "historyLength": 2,
        "returnImmediately": true, "taskPushNotificationConfig": {"url": "https://hooks.example.com/a", "token": "t"}})))
    .unwrap();
    assert_eq!(config.history_length, Some(2));
    assert!(config.return_immediately);
    assert_eq!(config.push.unwrap().token.as_deref(), Some("t"));
    assert_eq!(
        parse_config(Some(&json!({"historyLength": -1}))).unwrap_err().code,
        -32602
    );
    let push = parse_push(&json!({"taskId": "t1", "url": "https://h.example.com",
        "authentication": {"scheme": "Bearer", "credentials": "s3cret"}}))
    .unwrap();
    assert_eq!(
        push.authentication,
        Some(("Bearer".to_string(), Some("s3cret".to_string())))
    );
    // a 1.0 draft's nested config
    let nested =
        parse_push(&json!({"taskId": "t1", "pushNotificationConfig": {"url": "https://h.example.com"}})).unwrap();
    assert_eq!(nested.url, "https://h.example.com");
    assert_eq!(parse_push(&json!({"taskId": "t1"})).unwrap_err().code, -32602);
}

#[test]
fn the_list_cursor_round_trips_and_refuses_what_it_did_not_make() {
    let cursor = Cursor {
        submitted: 1_792_500_000_123,
        run_id: "100042".to_string(),
    };
    assert_eq!(Cursor::decode(Some(&cursor.encode())).unwrap(), Some(cursor));
    assert_eq!(Cursor::decode(Some("")).unwrap(), None);
    assert_eq!(Cursor::decode(Some("not-a-token")).unwrap_err().code, -32602);
    let foreign = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, r#"{"offset":5}"#);
    assert_eq!(Cursor::decode(Some(&foreign)).unwrap_err().code, -32602);
    assert_eq!(types::page_size(Some(&json!(0))).unwrap_err().code, -32602);
    assert_eq!(types::page_size(Some(&json!(101))).unwrap_err().code, -32602);
    assert_eq!(types::page_size(Some(&json!("7"))).unwrap(), 7, "a query parameter");
    assert_eq!(types::page_size(None).unwrap(), 50);
    assert_eq!(
        types::state_filter(Some(&json!("TASK_STATE_WORKING"))).unwrap(),
        Some(RunState::Working)
    );
    assert_eq!(types::state_filter(Some(&json!("working"))).unwrap_err().code, -32602);
    assert_eq!(
        types::timestamp_millis(Some(&json!("2026-10-22T10:00:00Z")), "t").unwrap(),
        Some(1_792_663_200_000)
    );
    assert_eq!(types::rfc3339(1_792_663_200_000), "2026-10-22T10:00:00Z");
}

#[test]
fn errors_have_the_shape_of_each_binding_and_never_the_cause() {
    let error = A2aError::task_not_found("t9");
    assert_eq!(
        error.to_rpc(),
        json!({"code": -32001, "message": "Task t9 was not found", "data": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": "TASK_NOT_FOUND", "domain": "a2a-protocol.org", "metadata": {}}]})
    );
    assert_eq!(error.to_rest()["error"]["code"], 404);
    assert_eq!(error.to_rest()["error"]["status"], "NOT_FOUND");
    assert_eq!(
        A2aError::not_cancelable("TASK_STATE_COMPLETED").http_status(),
        (400, "FAILED_PRECONDITION")
    );
    assert_eq!(A2aError::version("v").reason(), "VERSION_NOT_SUPPORTED");
    let internal = A2aError::internal();
    assert_eq!((internal.code, internal.message.as_str()), (-32603, "Internal error"));
    assert_eq!(internal.http_status().0, 500);
}

#[test]
fn the_advertised_url_takes_forwarded_headers_only_when_trusted() {
    let conf = |extra: &str| {
        LlmConf::from_conf_text(&format!(
            r#"<moqui-conf><llm-facade enabled="true"><a2a enabled="true" {extra}/></llm-facade></moqui-conf>"#
        ))
        .unwrap()
        .a2a
    };
    let forwarded = Origin {
        https: false,
        believed: true,
        host: Some("internal:8080".to_string()),
        forwarded_proto: Some("https".to_string()),
        forwarded_host: Some("agents.example.com, proxy.local".to_string()),
    };
    let root = || "http://configured:9000".to_string();
    assert_eq!(
        base_url_of(&conf(""), &forwarded, root),
        "http://internal:8080",
        "not trusted"
    );
    assert_eq!(
        base_url_of(&conf(r#"trust-forwarded-headers="true""#), &forwarded, root),
        "https://agents.example.com"
    );
    // the headers of a peer the client-ip-header conf does not believe are not read, whatever the a2a conf says
    let unbelieved = Origin {
        believed: false,
        ..forwarded.clone()
    };
    assert_eq!(
        base_url_of(&conf(r#"trust-forwarded-headers="true""#), &unbelieved, root),
        "http://internal:8080"
    );
    assert_eq!(
        base_url_of(&conf(r#"public-url="https://a2a.example.com/""#), &forwarded, root),
        "https://a2a.example.com"
    );
    // a host header that is not a plain host is never advertised
    let injected = Origin {
        host: Some("evil.com/\r\nx".to_string()),
        ..Origin::default()
    };
    assert_eq!(base_url_of(&conf(""), &injected, root), "http://configured:9000");
    let bad_forward = Origin {
        believed: true,
        forwarded_host: Some("a b".to_string()),
        host: Some("[::1]:8080".to_string()),
        ..Origin::default()
    };
    assert_eq!(
        base_url_of(&conf(r#"trust-forwarded-headers="true""#), &bad_forward, root),
        "http://[::1]:8080"
    );
    for bad in [
        "ftp://x",
        "https://u@x",
        "https://x?q",
        "https://x#f",
        "https://x//y",
        "https:// x",
        "",
    ] {
        assert_eq!(valid_base_url(bad), None, "{bad}");
    }
    assert_eq!(
        valid_base_url("https://x.example.com/base/").as_deref(),
        Some("https://x.example.com/base")
    );
}

#[test]
fn push_urls_name_a_host_and_no_user() {
    use super::push::scheme_host;
    assert_eq!(
        scheme_host("https://Hooks.Example.com:8443/a?b=1"),
        Some(("https".to_string(), "hooks.example.com".to_string()))
    );
    assert_eq!(
        scheme_host("http://[::1]:9/x").map(|(_, host)| host).as_deref(),
        Some("::1")
    );
    for bad in [
        "ftp://x/",
        "https://user@x/",
        "https://x/#frag",
        "mailto:x",
        "https:///x",
        "https://x y/",
    ] {
        assert_eq!(scheme_host(bad), None, "{bad}");
    }
}

#[test]
fn credentials_need_tls_a_believed_proxy_or_loopback() {
    use std::net::IpAddr;
    let public: IpAddr = "203.0.113.7".parse().unwrap();
    assert!(
        !super::secure(false, public),
        "plain HTTP from a peer that is not loopback"
    );
    assert!(super::secure(true, public));
    assert!(super::secure(false, "127.0.0.1".parse().unwrap()));
    assert!(super::secure(false, "::1".parse().unwrap()));
    // a request with credentials over plain HTTP is refused before they are looked at
    crate::script::clear_context_stack();
    let context = crate::context::ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><llm-facade enabled="true"><a2a enabled="true"/></llm-facade></moqui-conf>"#,
    );
    let _entered = context.enter();
    let request = super::HttpRequest {
        method: "POST".to_string(),
        headers: vec![("authorization".to_string(), "Basic YW5uOm1vcXVp".to_string())],
        secure: super::secure(false, public),
        client_ip: public.to_string(),
        ..super::HttpRequest::default()
    };
    let refused = super::authenticate(context.factory(), &request).unwrap_err();
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert!(refused.body.contains("plain HTTP"), "{}", refused.body);
    let anonymous = super::HttpRequest {
        secure: true,
        ..super::HttpRequest::default()
    };
    let challenged = super::authenticate(context.factory(), &anonymous).unwrap_err();
    assert_eq!(challenged.status, 401);
    assert!(challenged.headers.iter().any(|(name, _)| name == "www-authenticate"));
}

#[test]
fn the_card_lists_no_openid_connect_while_jwts_are_not_accepted() {
    crate::script::clear_context_stack();
    let context = crate::context::ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><llm-facade enabled="true"><a2a enabled="true"><security oidc-issuer="https://id.example.com"/></a2a></llm-facade></moqui-conf>"#,
    );
    let _entered = context.enter();
    let reply = super::card::public(context.factory(), &Origin::default());
    let card: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(reply.status, 200, "{card}");
    assert!(card["securitySchemes"].get("oidc").is_none(), "{card}");
    assert!(!reply.body.contains("openIdConnect"), "{card}");
    assert!(card["securitySchemes"]["bearer"].is_object());
}

#[test]
fn the_head_of_each_config_goes_first_and_only_when_due() {
    use super::push::{due_heads, Waiting};
    let row = |config: &str, delivery: &str, next: i64| Waiting {
        config_id: config.to_string(),
        delivery_id: delivery.to_string(),
        next,
    };
    // a: its head waits for a retry, so nothing after it goes; b: its head is due; c: due
    let waiting = [
        row("a", "1", 500),
        row("a", "2", 0),
        row("b", "3", 0),
        row("b", "4", 0),
        row("c", "5", 90),
    ];
    let heads: Vec<String> = due_heads(&waiting, 100)
        .into_iter()
        .map(|head| head.delivery_id)
        .collect();
    assert_eq!(heads, ["3", "5"]);
    assert!(due_heads(&waiting, 50).iter().all(|head| head.delivery_id == "3"));
}

#[test]
fn a_restarted_step_replaces_the_streamed_text() {
    use super::ops::{Follow, Projector};
    let facade = crate::testing::open();
    let mut projector = Projector::new(Follow {
        run_id: "r1".to_string(),
        context_id: "c1".to_string(),
        base_url: "http://localhost".to_string(),
        after: 0,
        close_on_pause: true,
        text: true,
    });
    let mut project = |event: &str, payload: serde_json::Value| projector.project(&facade, event, &payload).unwrap();
    let (first, ends) = project(
        "text.chunk",
        json!({"messageId": "m", "part": 0, "offset": 0, "text": "Wrong "}),
    );
    assert!(!ends);
    assert_eq!(
        first[0]["artifactUpdate"]["append"], false,
        "a message's first chunk replaces"
    );
    let (more, _) = project(
        "text.chunk",
        json!({"messageId": "m", "part": 0, "offset": 6, "text": "answer"}),
    );
    assert_eq!(more[0]["artifactUpdate"]["append"], true);
    let (restart, ends) = project(
        "step.restart",
        json!({"messageId": "m", "attempt": 2, "reason": "retry"}),
    );
    assert!(!ends);
    assert_eq!(
        restart,
        [
            json!({"artifactUpdate": {"taskId": "r1", "contextId": "c1", "append": false, "lastChunk": false,
            "artifact": {"artifactId": "response", "name": "response", "parts": [{"text": ""}]}}})
        ],
        "the restarted call's text is void"
    );
    let (again, _) = project(
        "text.chunk",
        json!({"messageId": "m", "part": 0, "offset": 0, "text": "Right."}),
    );
    assert_eq!(again[0]["artifactUpdate"]["append"], false);
    assert!(project("run.done", json!({})).1);
}
