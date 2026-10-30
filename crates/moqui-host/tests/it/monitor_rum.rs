//! Browser RUM and session replay on a real seeded PostgreSQL server: the qvt shell loads the RUM script only for a
//! signed-in user, /rum/traces and /rum/logs take a session's OTLP/JSON under its CSRF token with the session's
//! identity forced on, and /rum/replay holds a replay to its size cap, readable only with MONITOR_REPLAY
//! (plans/Monitoring.md, Later: Browser RUM).
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
use crate::common::{self, exchange, http_request, merge_cookies, session_token, sign_in};
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};
use std::net::SocketAddr;

fn browser(address: SocketAddr, username: &str) -> (String, String, String) {
    let cookie = sign_in(address, username, "moqui");
    let (status, headers, html) = exchange(
        address,
        &http_request("GET", "/qapps", &[("Cookie", &cookie), ("Accept", "text/html")], ""),
    );
    let token = session_token(&html)
        .or_else(|| common::header(&headers, "moquiSessionToken"))
        .unwrap_or_else(|| panic!("no session token: {status}"));
    (merge_cookies(&cookie, &headers), token, html)
}

fn post(address: SocketAddr, path: &str, cookie: &str, token: Option<&str>, body: &Json) -> (u16, String) {
    let mut headers = vec![("Accept", "application/json"), ("Content-Type", "application/json")];
    if !cookie.is_empty() {
        headers.push(("Cookie", cookie));
    }
    if let Some(token) = token {
        headers.push(("X-CSRF-Token", token));
    }
    let (status, _, text) = exchange(address, &http_request("POST", path, &headers, &body.to_string()));
    (status, text)
}

fn spans(trace: &str, now: i64) -> Json {
    let at = |ms: i64| (ms * 1_000_000).to_string();
    json!({"resourceSpans":[{"resource":{"attributes":[
    {"key":"service.name","value":{"stringValue":"pretend-server"}},
    {"key":"session.id","value":{"stringValue":"0123456789abcdef0123456789abcdef"}}]},
    "scopeSpans":[{"spans":[
        {"traceId":trace,"spanId":"7a7a7a7a7a7a7a7a","name":"documentLoad","kind":1,"startTimeUnixNano":at(now-500),"endTimeUnixNano":at(now),
         "attributes":[{"key":"user.id","value":{"stringValue":"ADMIN"}},{"key":"browser.web_vital.lcp","value":{"intValue":"420"}}]},
        {"traceId":trace,"spanId":"7b7b7b7b7b7b7b7b","name":"posing","kind":2,"startTimeUnixNano":at(now-500),"endTimeUnixNano":at(now)}
    ]}]}]})
}

#[test]
fn signed_in_pages_send_rum_under_their_own_identity_and_replays_keep_to_their_caps() {
    if !common::own_process() {
        return;
    }
    let gate = common::start_with_conf_properties(
        "monitor-rum",
        Some(
            r#"<entity-facade-xml type="seed"><moqui.security.UserAccount userId="RUM_VIEWER" username="rum.viewer" disabled="N"/></entity-facade-xml>"#,
        ),
        r#"<telemetry sample-ratio="0"><browser enabled="true" replay-enabled="true" replay-max-bytes="3000"/></telemetry>"#,
        &[],
    );
    let address = gate.server.address();
    let factory = gate.server.factory();
    let now = jiff::Timestamp::now().as_millisecond();
    let trace = "7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c";

    // No session, or a session without its token, is refused.
    assert_eq!(post(address, "/rum/traces", "", None, &spans(trace, now)).0, 401);
    let (cookie, token, html) = browser(address, "john.doe");
    assert!(
        html.contains("/js/WebrootRum.js"),
        "the shell loads RUM for a signed-in user"
    );
    assert!(
        post(address, "/rum/traces", &cookie, None, &spans(trace, now)).0 >= 400,
        "no CSRF token"
    );

    let (status, text) = post(address, "/rum/traces", &cookie, Some(&token), &spans(trace, now));
    assert_eq!(status, 200, "{text}");
    let answer: Json = serde_json::from_str(&text).unwrap();
    assert_eq!(
        answer["partialSuccess"]["rejectedSpans"], "1",
        "the SERVER span is refused: {text}"
    );
    let error = json!({"resourceLogs":[{"scopeLogs":[{"logRecords":[{"timeUnixNano":(now * 1_000_000).to_string(),
        "severityNumber":17,"body":{"stringValue":"rum fixture error"},"traceId":trace,
        "attributes":[{"key":"exception.type","value":{"stringValue":"TypeError"}}]}]}]}]});
    let (status, text) = post(address, "/rum/logs", &cookie, Some(&token), &error);
    assert_eq!(status, 200, "{text}");

    moqui_core::telemetry::for_factory(factory).flush();
    let search = moqui_core::search::installed(factory).unwrap();
    let found = search
        .search(
            &moqui_core::search::Target::Index("otel_spans".into()),
            &moqui_core::search::SearchRequest::new(moqui_core::search::Query::eq("trace_id", Value::string(trace))),
        )
        .unwrap();
    assert_eq!(found.hits.len(), 1, "{:?}", found.hits);
    let span = &found.hits[0].body;
    assert_eq!(span.get("name"), Some(&Value::string("documentLoad")));
    assert_eq!(span.get("user.id"), Some(&Value::string("EX_JOHN_DOE")));
    assert_ne!(span.get("service.name"), Some(&Value::string("pretend-server")));
    assert_eq!(span.get("source.id"), Some(&Value::string("browser")));
    assert!(span
        .get("moqui.visit.id")
        .is_some_and(|visit| !matches!(visit, Value::Null)));
    let logs = search
        .search(
            &moqui_core::search::Target::Index("otel_logs".into()),
            &moqui_core::search::SearchRequest::new(moqui_core::search::Query::eq("trace_id", Value::string(trace))),
        )
        .unwrap();
    assert_eq!(logs.hits.len(), 1);
    assert_eq!(logs.hits[0].body.get("user.id"), Some(&Value::string("EX_JOHN_DOE")));

    // Replay: chunks until the size cap, then refused.
    let replay = "0123abcd-replay-fixture-0001";
    let chunk = |seq: u64| {
        json!({"replayId":replay,"seq":seq,"start":now,"end":now + 1000,"url":"/qapps/system?secret=1","traceIds":[trace],
            "events":[{"type":4,"data":{"href":"/qapps/system","width":800,"height":600},"timestamp":now},
                      {"type":3,"data":{"source":0,"texts":[{"id":1,"value":"*".repeat(1000)}]},"timestamp":now + 10}]})
    };
    assert_eq!(post(address, "/rum/replay", &cookie, Some(&token), &chunk(0)).0, 200);
    assert_eq!(post(address, "/rum/replay", &cookie, Some(&token), &chunk(1)).0, 200);
    assert_eq!(
        post(address, "/rum/replay", &cookie, Some(&token), &chunk(2)).0,
        413,
        "past replay-max-bytes"
    );
    moqui_core::telemetry::for_factory(factory).flush();
    let context = factory.new_context();
    let _entered = context.enter();
    // Someone without MONITOR_REPLAY sees no replays.
    moqui_core::user::internal_login_user("rum.viewer").unwrap();
    let listed = moqui_core::service::sync(
        "org.moqui.monitor.MonitorServices.list#BrowserReplays",
        &Default::default(),
    );
    assert!(
        listed.is_err() || !context.message().errors().is_empty(),
        "rum.viewer has no MONITOR_REPLAY"
    );
    context.message().clear_errors();
    // john.doe is in ADMIN, which has it.
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let listed = moqui_core::service::sync(
        "org.moqui.monitor.MonitorServices.list#BrowserReplays",
        &Default::default(),
    )
    .unwrap();
    assert!(
        context.message().errors().is_empty(),
        "{:?}",
        context.message().errors()
    );
    let Some(Value::List(replays)) = listed.get("replays") else {
        panic!("{listed:?}")
    };
    assert_eq!(replays.len(), 1, "{replays:?}");
    let detail = moqui_core::service::sync(
        "org.moqui.monitor.MonitorServices.get#BrowserReplay",
        &[("replayId".to_string(), Value::string(replay))].into_iter().collect(),
    )
    .unwrap();
    assert_eq!(
        detail.get("url"),
        Some(&Value::string("/qapps/system")),
        "the query is dropped"
    );
    assert!(
        matches!(detail.get("events"), Some(Value::List(events)) if events.len() == 4),
        "{:?}",
        detail.get("events")
    );
    assert_eq!(detail.get("traceIds"), Some(&Value::List(vec![Value::string(trace)])));
    // Not through the general search either, even for an administrator.
    let searched = moqui_core::service::sync(
        "org.moqui.search.SearchServices.search#DataDocuments",
        &[
            ("indexName".to_string(), Value::string("browser_replays")),
            ("queryString".to_string(), Value::string("*:*")),
        ]
        .into_iter()
        .collect(),
    );
    let leaked = searched
        .as_ref()
        .ok()
        .and_then(|out| out.get("documentList"))
        .is_some_and(|list| matches!(list, Value::List(items) if !items.is_empty()));
    assert!(!leaked, "{searched:?}");
    context.message().clear_errors();
    // An anonymous page is not given RUM.
    let (_, _, anonymous) = exchange(address, &http_request("GET", "/Login", &[("Accept", "text/html")], ""));
    assert!(!anonymous.contains("/js/WebrootRum.js"));
}
