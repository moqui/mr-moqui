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

//! The Assist drawer's JSON calls (docs/Assist.md; plans/LLM-A2A-Assist.md 4.5, 4.11) over HTTP on a seeded server on
//! embedded Postgres, with the scripted fake provider: each call's happy path and its refusal for another user; a turn
//! sent twice with one client message id; a turn while a run is active queued as input and not counted by the AT_LLM
//! tarpit, which counts runs; the client's tier; a decision refused for a header login and taken from the owner's
//! browser session with its token; a run that streams to its approval, is decided over HTTP and resumes; and the
//! admin screens, where another user's content needs AgentAudit and each such read is logged.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "llm"))]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm::provider::fake::{self, Step};
use moqui_core::llm::run::state::RunState;
use moqui_lib::object::{Value, to_plain_string};
use serde_json::{Value as Json, json};

use crate::common::{basic, exchange, http_request, merge_cookies, session_token, sign_in, start_with_data};

const ENDPOINT: &str = "assist-fake";
const WAIT: Duration = Duration::from_secs(60);

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="assist-fake" protocol="fake" description="Assist fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="assist-fast" endpointId="assist-fake" providerModel="fake-fast" tier="fast"
        contextWindow="32768"/>
    <moqui.llm.LlmModel modelId="assist-thorough" endpointId="assist-fake" providerModel="fake-thorough" tier="thorough"
        contextWindow="32768"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="assist-fast" reasoningLevel="none"/>
    <moqui.basic.EnumerationType enumTypeId="AST_TYPE" description="Before"/>
    <moqui.agent.AgentToolExposure toolName="update_enum_type" serviceName="update#moqui.basic.EnumerationType"
        title="Change an enumeration type" lockVersion="1"/>
    <moqui.security.ArtifactGroup artifactGroupId="ASSIST_TEST" description="Assist drawer test"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="ASSIST_TEST" artifactName=".*EnumerationType"
        nameIsPattern="Y" artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="ASSIST_TEST" artifactName="moqui.basic.EnumerationType"
        artifactTypeEnumId="AT_ENTITY"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="ASSIST_TEST_ADMIN" userGroupId="ADMIN" artifactGroupId="ASSIST_TEST"
        authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    <!-- ann uses the agents and nothing else; bob may not use them; both sign in with the password moqui -->
    <moqui.security.UserAccount userId="AST_ANN" username="ann.assist" userFullName="Ann Assist"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA" locale="en_US" timeZone="US/Central"/>
    <moqui.security.UserAccount userId="AST_BOB" username="bob.assist" userFullName="Bob Assist"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="AST_AGENT_USERS" description="Assist users"/>
    <moqui.security.UserGroupMember userGroupId="AST_AGENT_USERS" userId="AST_ANN" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="AST_AGENT_USERS" userPermissionId="AgentUse" fromDate="1265184000000"/>
    <!-- ann's runs are limited to 2 a minute: the third locks the fourth out -->
    <moqui.security.ArtifactTarpit artifactTarpitId="AST_ANN_RUNS" userGroupId="AST_AGENT_USERS" artifactGroupId="AgentRuns"
        maxHitsCount="2" maxHitsDuration="60" tarpitDuration="60"/>
    <!-- ann's dictations and text actions: one a minute (the AT_LLM tarpit of assist:*; the second locks the third out) -->
    <moqui.security.ArtifactTarpit artifactTarpitId="AST_ANN_ASSIST" userGroupId="AST_AGENT_USERS" artifactGroupId="AssistModelCalls"
        maxHitsCount="1" maxHitsDuration="60" tarpitDuration="60"/>
    <moqui.agent.AgentSuggestedPrompt promptId="AST_P1" label="Find users" promptText="Find the user accounts"
        sendMode="send" screenPathPattern="/apps/system.*" sequenceNum="1"/>
</entity-facade-xml>"#;

/// A browser session of `username`: its cookie and the session token of a page.
fn browser(address: SocketAddr, username: &str) -> (String, String) {
    let cookie = sign_in(address, username, "moqui");
    let (status, headers, html) = exchange(
        address,
        &http_request("GET", "/qapps", &[("Cookie", &cookie), ("Accept", "text/html")], ""),
    );
    let token = session_token(&html)
        .or_else(|| crate::common::header(&headers, "moquiSessionToken"))
        .unwrap_or_else(|| {
            panic!(
                "no session token for {username}: {status} {}",
                html.chars().take(500).collect::<String>()
            )
        });
    (merge_cookies(&cookie, &headers), token)
}

/// A signed-in user with the token of their session.
struct Client {
    address: SocketAddr,
    cookie: String,
    token: String,
}

impl Client {
    fn sign_in(address: SocketAddr, username: &str) -> Self {
        let (cookie, token) = browser(address, username);
        Self { address, cookie, token }
    }

    fn get(&self, call: &str, query: &str) -> (u16, Json) {
        let path = if query.is_empty() {
            format!("/assist/{call}")
        } else {
            format!("/assist/{call}?{query}")
        };
        let (status, _, body) = exchange(
            self.address,
            &http_request(
                "GET",
                &path,
                &[("Cookie", &self.cookie), ("Accept", "application/json")],
                "",
            ),
        );
        (status, parse(&body))
    }

    fn post(&self, call: &str, body: &Json) -> (u16, Json) {
        let (status, _, text) = self.post_raw(call, body, true);
        (status, parse(&text))
    }

    /// A form post, as `moqui.request` sends a plain object: an object parameter is JSON text.
    fn post_form(&self, call: &str, form: &[(&str, &str)]) -> (u16, Json) {
        let body: Vec<String> = form
            .iter()
            .map(|(name, value)| format!("{name}={}", urlencoding(value)))
            .collect();
        let headers = [
            ("Cookie", self.cookie.as_str()),
            ("Accept", "application/json"),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("X-CSRF-Token", self.token.as_str()),
        ];
        let (status, _, text) = exchange(
            self.address,
            &http_request("POST", &format!("/assist/{call}"), &headers, &body.join("&")),
        );
        (status, parse(&text))
    }

    fn post_raw(&self, call: &str, body: &Json, with_token: bool) -> (u16, Vec<(String, String)>, String) {
        let text = body.to_string();
        let mut headers = vec![
            ("Cookie", self.cookie.as_str()),
            ("Accept", "application/json"),
            ("Content-Type", "application/json"),
        ];
        if with_token {
            headers.push(("X-CSRF-Token", self.token.as_str()));
        }
        exchange(
            self.address,
            &http_request("POST", &format!("/assist/{call}"), &headers, &text),
        )
    }
}

fn urlencoding(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(byte).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn parse(body: &str) -> Json {
    serde_json::from_str(body).unwrap_or_else(|_| json!({"unparsed": body}))
}

fn read_stream(address: SocketAddr, request: &str, limit: Duration) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(limit)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => out.extend_from_slice(&buffer[..count]),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn rows(
    factory: &Arc<ExecutionContextFactory>,
    entity: &str,
    field: &str,
    value: &str,
) -> Vec<moqui_core::entity::EntityValue> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory
        .entity()
        .unwrap()
        .find(entity)
        .unwrap()
        .eq(field, Value::string(value))
        .list()
        .unwrap()
        .values()
        .to_vec()
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn wait(factory: &Arc<ExecutionContextFactory>, run_id: &str) -> RunState {
    moqui_core::llm::run::wait_for(factory, run_id, WAIT).unwrap()
}

fn description(factory: &Arc<ExecutionContextFactory>) -> String {
    field(
        &rows(factory, "moqui.basic.EnumerationType", "enumTypeId", "AST_TYPE")[0],
        "description",
    )
}

fn str_of(value: &Json) -> String {
    value.as_str().unwrap_or_default().to_string()
}

#[test]
fn the_drawer_calls_over_http_on_postgres() {
    let gate = start_with_data("assist", DATA);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let context = factory.new_context();
    let _entered = context.enter();
    fake::clear(ENDPOINT);

    let john = Client::sign_in(address, "john.doe");
    let ann = Client::sign_in(address, "ann.assist");
    let bob = Client::sign_in(address, "bob.assist");

    // bootstrap: the agents, tiers and features; no user is 401, a user without AgentUse 403
    let (status, boot) = john.get("bootstrap", "");
    assert_eq!(status, 200, "{boot}");
    assert_eq!(boot["defaultAgentId"], "assist", "{boot}");
    let assist = boot["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["agentId"] == "assist")
        .unwrap();
    assert_eq!(assist["tiers"], json!(["fast", "thorough"]), "{boot}");
    assert_eq!(assist["tierFixed"], false);
    assert!(
        boot["secretFields"]["contains"]
            .as_array()
            .unwrap()
            .contains(&json!("password"))
    );
    let (status, _, _) = exchange(
        address,
        &http_request("GET", "/assist/bootstrap", &[("Accept", "application/json")], ""),
    );
    assert_eq!(status, 401);
    let (status, refused) = bob.get("bootstrap", "");
    assert_eq!((status, refused["code"].as_str()), (403, Some("refused")), "{refused}");

    // a turn starts a run in a new conversation, at the tier asked for, from the page the user is on
    fake::script(ENDPOINT, vec![fake::Reply::text("Hello from Assist.")]);
    let turn = json!({"text": "Say hello\nplease", "clientMessageId": "john-m1", "clientId": "tab-a", "tier": "thorough",
        "contextVersion": 1, "state": {"path": "/qapps/system/Llm", "title": "LLM and Agents",
            "forms": [{"domId": "Find", "values": {"name": "x", "password": "hunter2"}, "dirty": ["name", "password"]}]}});
    let (status, _, text) = john.post_raw("turn", &turn, false);
    assert_eq!(status, 401, "a POST without the session token: {text}");
    let (status, started) = john.post("turn", &turn);
    assert_eq!(status, 200, "{started}");
    assert_eq!(
        (started["queued"].as_bool(), started["existing"].as_bool()),
        (Some(false), Some(false)),
        "{started}"
    );
    let conversation_id = str_of(&started["conversationId"]);
    let run_id = str_of(&started["runId"]);
    assert_eq!(started["follow"]["clientId"], "tab-a");
    assert_eq!(wait(&factory, &run_id), RunState::Completed);
    let sent = fake::requests(ENDPOINT);
    assert_eq!(
        sent.last().unwrap()["model"],
        "fake-thorough",
        "the client's tier chose the model"
    );
    let run_row = &rows(&factory, "moqui.agent.AgentRun", "runId", &run_id)[0];
    assert_eq!(field(run_row, "tier"), "thorough");
    let message = &rows(&factory, "moqui.agent.AgentMessage", "clientMessageId", "john-m1")[0];
    let metadata = field(message, "metadataJson");
    assert!(
        metadata.contains("page /apps/system/Llm") && !metadata.contains("hunter2"),
        "{metadata}"
    );
    let conversation = &rows(
        &factory,
        "moqui.agent.AgentConversation",
        "conversationId",
        &conversation_id,
    )[0];
    assert_eq!(field(conversation, "title"), "Say hello");
    assert!(field(conversation, "screenStateJson").contains("/apps/system/Llm"));
    assert!(!field(conversation, "screenStateJson").contains("hunter2"));

    // the same message again is the same run
    let (status, again) = john.post("turn", &turn);
    assert_eq!(
        (status, again["existing"].as_bool(), str_of(&again["runId"])),
        (200, Some(true), run_id.clone()),
        "{again}"
    );
    assert_eq!(
        rows(&factory, "moqui.agent.AgentRun", "conversationId", &conversation_id).len(),
        1
    );

    // the conversation: its items, runs and follow owner; another user's is not found
    let (status, read) = john.get(
        "conversation",
        &format!("conversationId={conversation_id}&clientId=tab-a"),
    );
    assert_eq!(status, 200, "{read}");
    let items = read["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{read}");
    assert_eq!(
        (items[0]["role"].as_str(), items[0]["parts"][0]["text"].as_str()),
        (Some("user"), Some("Say hello\nplease"))
    );
    assert_eq!(items[1]["parts"][0]["text"], "Hello from Assist.");
    assert_eq!(read["follow"]["following"], true);
    assert!(read["activeRun"].is_null());
    let assistant_message = str_of(&items[1]["messageId"]);
    let (status, _) = ann.get("conversation", &format!("conversationId={conversation_id}"));
    assert_eq!(status, 404);

    // the list and its search; another user's list does not show it
    let (status, list) = john.get("conversations", "search=hello");
    assert_eq!(status, 200, "{list}");
    assert_eq!(
        list["conversations"][0]["conversationId"],
        conversation_id.as_str(),
        "{list}"
    );
    let (_, list) = john.get("conversations", "search=nothing-like-it");
    assert_eq!(list["conversations"].as_array().unwrap().len(), 0);
    let (_, list) = ann.get("conversations", "");
    assert!(!list.to_string().contains(&conversation_id), "{list}");

    // feedback on the assistant's message; not on another user's
    let (status, kept) = john.post(
        "feedback",
        &json!({"messageId": assistant_message, "rating": "up", "note": "good"}),
    );
    assert_eq!(
        (status, kept["feedback"]["rating"].as_str()),
        (200, Some("up")),
        "{kept}"
    );
    let (_, read) = john.get("conversation", &format!("conversationId={conversation_id}"));
    assert_eq!(read["items"][1]["feedback"]["note"], "good");
    let (status, _) = ann.post("feedback", &json!({"messageId": assistant_message, "rating": "down"}));
    assert_eq!(status, 404);

    // follow: another tab claims it; a navigation in a tab that does not follow changes nothing
    let (status, follow) = john.post(
        "follow",
        &json!({"conversationId": conversation_id, "clientId": "tab-b"}),
    );
    assert_eq!((status, follow["following"].as_bool()), (200, Some(true)), "{follow}");
    let (status, ignored) = john.post(
        "context",
        &json!({"conversationId": conversation_id, "clientId": "tab-a",
        "contextVersion": 9, "state": {"path": "/qapps/tools"}}),
    );
    assert_eq!((status, ignored["applied"].as_bool()), (200, Some(false)), "{ignored}");
    let (status, applied) = john.post(
        "context",
        &json!({"conversationId": conversation_id, "clientId": "tab-b",
        "contextVersion": 3, "state": {"path": "/qapps/system/Llm/Runs"}}),
    );
    assert_eq!((status, applied["applied"].as_bool()), (200, Some(true)), "{applied}");
    assert_eq!(applied["follow"], json!({"clientId": "tab-b", "contextVersion": 3}));
    let conversation = &rows(
        &factory,
        "moqui.agent.AgentConversation",
        "conversationId",
        &conversation_id,
    )[0];
    assert!(
        field(conversation, "screenStateJson").contains("/apps/system/Llm/Runs"),
        "the page synced in"
    );
    // the same as a form, whose page state is JSON text
    let (status, applied) = john.post_form(
        "context",
        &[
            ("conversationId", conversation_id.as_str()),
            ("clientId", "tab-b"),
            ("contextVersion", "4"),
            ("state", r#"{"path": "/qapps/system/Llm/Approvals"}"#),
        ],
    );
    assert_eq!(
        (status, applied["follow"]["contextVersion"].as_i64()),
        (200, Some(4)),
        "{applied}"
    );
    let conversation = &rows(
        &factory,
        "moqui.agent.AgentConversation",
        "conversationId",
        &conversation_id,
    )[0];
    assert!(field(conversation, "screenStateJson").contains("/apps/system/Llm/Approvals"));
    let (status, _) = ann.post(
        "follow",
        &json!({"conversationId": conversation_id, "clientId": "tab-x"}),
    );
    assert_eq!(status, 404);
    let (status, _) = ann.post(
        "context",
        &json!({"conversationId": conversation_id, "clientId": "tab-x", "contextVersion": 1}),
    );
    assert_eq!(status, 404);

    // retry runs the last message again; another user cannot
    fake::script(ENDPOINT, vec![fake::Reply::text("Hello again.")]);
    let (status, retried) = john.post(
        "retry",
        &json!({"conversationId": conversation_id, "clientId": "tab-b",
        "clientMessageId": "john-retry"}),
    );
    assert_eq!(status, 200, "{retried}");
    let retry_run = str_of(&retried["runId"]);
    assert_ne!(retry_run, run_id);
    assert_eq!(wait(&factory, &retry_run), RunState::Completed);
    let (status, _) = ann.post(
        "retry",
        &json!({"conversationId": conversation_id, "clientId": "tab-x",
        "clientMessageId": "ann-retry"}),
    );
    assert_eq!(status, 404);

    // a message while the run works is queued for it (an AgentInput), once for one client message id
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::steps(vec![
                Step::Delay(2500),
                Step::Text("Working slowly.".to_string()),
                Step::Finish("stop".to_string()),
            ]),
            fake::Reply::text("And the second message, folded in."),
        ],
    );
    let (status, slow) = john.post(
        "turn",
        &json!({"conversationId": conversation_id, "text": "Take your time",
        "clientMessageId": "john-m2", "clientId": "tab-b"}),
    );
    assert_eq!(status, 200, "{slow}");
    let slow_run = str_of(&slow["runId"]);
    let queued_turn = json!({"conversationId": conversation_id, "text": "Also this", "clientMessageId": "john-m3",
        "clientId": "tab-b"});
    let (status, queued) = john.post("turn", &queued_turn);
    assert_eq!(
        (status, queued["queued"].as_bool(), str_of(&queued["runId"])),
        (200, Some(true), slow_run.clone()),
        "{queued}"
    );
    let (_, queued_again) = john.post("turn", &queued_turn);
    assert_eq!(
        (queued_again["existing"].as_bool(), queued_again["queued"].as_bool()),
        (Some(true), Some(true)),
        "{queued_again}"
    );
    assert_eq!(
        rows(&factory, "moqui.agent.AgentInput", "clientMessageId", "john-m3").len(),
        1
    );
    let (_, read) = john.get("conversation", &format!("conversationId={conversation_id}"));
    assert_eq!(read["activeRun"]["runId"], slow_run.as_str(), "{read}");
    let (status, _) = ann.post("cancel", &json!({"runId": slow_run}));
    assert_eq!(status, 404, "another user's run is not found");
    let (status, refused) = john.post(
        "retry",
        &json!({"conversationId": conversation_id, "clientId": "tab-b",
        "clientMessageId": "john-retry-2"}),
    );
    assert_eq!(status, 409, "no retry while a run is active: {refused}");
    wait(&factory, &slow_run);

    // cancel stops a run of the user's
    fake::script(
        ENDPOINT,
        vec![fake::Reply::steps(vec![
            Step::Delay(5000),
            Step::Text("Too late.".to_string()),
            Step::Finish("stop".to_string()),
        ])],
    );
    let (_, slow) = john.post(
        "turn",
        &json!({"text": "Something long", "clientMessageId": "john-m4", "clientId": "tab-c"}),
    );
    let cancel_run = str_of(&slow["runId"]);
    let (status, canceled) = john.post("cancel", &json!({"runId": cancel_run}));
    assert_eq!(status, 200, "{canceled}");
    assert_eq!(wait(&factory, &cancel_run), RunState::Canceled);
    fake::clear(ENDPOINT);

    // the tarpit counts ann's runs, not her queued messages: two runs, a queued message, a third run, then refused
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::text("One."),
            fake::Reply::steps(vec![
                Step::Delay(2000),
                Step::Text("Two.".to_string()),
                Step::Finish("stop".to_string()),
            ]),
            fake::Reply::text("Two, with the message."),
            fake::Reply::text("Three."),
        ],
    );
    let (status, first) = ann.post(
        "turn",
        &json!({"text": "One", "clientMessageId": "ann-1", "clientId": "ann-tab"}),
    );
    assert_eq!(status, 200, "{first}");
    let ann_conversation = str_of(&first["conversationId"]);
    let ann_run = str_of(&first["runId"]);
    wait(&factory, &ann_run);
    let (_, second) = ann.post(
        "turn",
        &json!({"text": "Two", "clientMessageId": "ann-2", "clientId": "ann-tab",
        "conversationId": ann_conversation}),
    );
    let (status, queued) = ann.post(
        "turn",
        &json!({"text": "Two more", "clientMessageId": "ann-2b", "clientId": "ann-tab",
        "conversationId": ann_conversation}),
    );
    assert_eq!((status, queued["queued"].as_bool()), (200, Some(true)), "{queued}");
    wait(&factory, &str_of(&second["runId"]));
    let (status, third) = ann.post(
        "turn",
        &json!({"text": "Three", "clientMessageId": "ann-3", "clientId": "ann-tab"}),
    );
    assert_eq!(
        status, 200,
        "the third run is still let through, and locks the next: {third}"
    );
    wait(&factory, &str_of(&third["runId"]));
    let (status, headers, text) = ann.post_raw(
        "turn",
        &json!({"text": "Four", "clientMessageId": "ann-4",
        "clientId": "ann-tab"}),
        true,
    );
    assert_eq!(status, 429, "{text}");
    assert!(crate::common::header(&headers, "Retry-After").is_some(), "{headers:?}");
    assert_eq!(parse(&text)["code"], "tarpit");
    // a refused turn into a conversation moves neither its follow owner nor its page
    let before = &rows(
        &factory,
        "moqui.agent.AgentConversation",
        "conversationId",
        &ann_conversation,
    )[0];
    let (owner, page) = (field(before, "clientId"), field(before, "screenStateJson"));
    let (status, _, text) = ann.post_raw(
        "turn",
        &json!({"text": "Five", "clientMessageId": "ann-5", "clientId": "ann-other-tab", "conversationId": ann_conversation,
            "contextVersion": 40, "state": {"path": "/apps/system/Security/UserGroup"}}),
        true,
    );
    assert_eq!(status, 429, "{text}");
    let after = &rows(
        &factory,
        "moqui.agent.AgentConversation",
        "conversationId",
        &ann_conversation,
    )[0];
    assert_eq!(
        (field(after, "clientId"), field(after, "screenStateJson")),
        (owner, page)
    );
    assert_ne!(field(after, "contextVersion"), "40");

    // a run that asks for approval: it streams to the pause, the decision is refused for a header login and for
    // another user, taken from the owner's browser session, and the run resumes and writes
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::steps(vec![
                Step::Tool {
                    id: "u1".to_string(),
                    name: "run_tool".to_string(),
                    arguments: json!({"name": "update_enum_type",
                        "parameters": {"enumTypeId": "AST_TYPE", "description": "After"}})
                    .to_string(),
                    pieces: 1,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("Changed it."),
        ],
    );
    let (status, started) = john.post(
        "turn",
        &json!({"text": "Change AST_TYPE to After", "clientMessageId": "john-m5",
        "clientId": "tab-a"}),
    );
    assert_eq!(status, 200, "{started}");
    let approval_run = str_of(&started["runId"]);
    let approval_conversation = str_of(&started["conversationId"]);
    let events_path = str_of(&started["eventsPath"]);
    let body = read_stream(
        address,
        &http_request("GET", &events_path, &[("Cookie", &john.cookie)], ""),
        WAIT,
    );
    assert!(
        body.contains("event: approval.requested") && body.contains("TASK_STATE_INPUT_REQUIRED"),
        "{body}"
    );
    assert_eq!(wait(&factory, &approval_run), RunState::InputRequired);
    let last_seq: i64 = body
        .lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .filter_map(|id| id.trim().parse().ok())
        .max()
        .unwrap();
    assert_eq!(description(&factory), "Before");
    let (_, read) = john.get("conversation", &format!("conversationId={approval_conversation}"));
    let card = &read["pending"]["approvals"][0];
    assert_eq!(
        (card["targetName"].as_str(), card["tool"].as_str()),
        (Some("update#moqui.basic.EnumerationType"), Some("run_tool")),
        "{read}"
    );
    let approval_id = str_of(&card["approvalId"]);
    assert_eq!(read["activeRun"]["pauseKind"], "approval");
    // the tasks pane has it waiting; another user's does not
    let (status, tasks) = john.get("tasks", "");
    assert_eq!(status, 200, "{tasks}");
    assert!(
        tasks["waiting"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["approvalId"] == approval_id.as_str()),
        "{tasks}"
    );
    let (_, tasks) = ann.get("tasks", "");
    assert!(!tasks.to_string().contains(&approval_id), "{tasks}");
    // the preview: the owner's (when the sim overlay is there), never another user's
    let (status, previewed) = john.post("preview", &json!({"approvalId": approval_id}));
    if boot["features"]["preview"] == true {
        assert_eq!(status, 200, "{previewed}");
    } else {
        assert_ne!(status, 404, "{previewed}");
    }
    let (status, _) = ann.post("preview", &json!({"approvalId": approval_id}));
    assert_eq!(status, 404);
    // a header login may not decide (Basic authentication carries no session token, and no person decided)
    let decision = json!({"approvalId": approval_id, "decision": "approve"});
    let (status, _, text) = exchange(
        address,
        &http_request(
            "POST",
            "/assist/decide",
            &[
                ("Authorization", &basic("john.doe", "moqui")),
                ("Accept", "application/json"),
                ("Content-Type", "application/json"),
            ],
            &decision.to_string(),
        ),
    );
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("decided only by its user"), "{text}");
    let (status, _) = ann.post("decide", &decision);
    assert_eq!(status, 404);
    let (status, _, text) = john.post_raw("decide", &decision, false);
    assert_eq!(status, 401, "the browser session without its token: {text}");
    assert_eq!(description(&factory), "Before");
    let (status, decided) = john.post("decide", &decision);
    assert_eq!(
        (status, decided["status"].as_str()),
        (200, Some("approved")),
        "{decided}"
    );
    assert_eq!(wait(&factory, &approval_run), RunState::Completed);
    assert_eq!(description(&factory), "After", "the approved write ran");
    let (status, decided_again) = john.post("decide", &decision);
    assert_eq!(status, 409, "a second decision: {decided_again}");
    // the resumed run streams from where the client was
    let body = read_stream(
        address,
        &http_request(
            "GET",
            &events_path,
            &[("Cookie", &john.cookie), ("Last-Event-ID", &last_seq.to_string())],
            "",
        ),
        WAIT,
    );
    assert!(
        body.contains("event: approval.decided") && body.contains("Changed it.") && body.contains("event: run.done"),
        "{body}"
    );

    // a question: answered by its run's owner, refused for another user and for a question it does not wait on
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::steps(vec![
                Step::Tool {
                    id: "q1".to_string(),
                    name: "ask".to_string(),
                    arguments: json!({"question": "Which one?", "options": ["Red", "Blue"]}).to_string(),
                    pieces: 1,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("Blue it is."),
        ],
    );
    let (_, asked) = john.post(
        "turn",
        &json!({"text": "Pick a color", "clientMessageId": "john-m6", "clientId": "tab-a"}),
    );
    let ask_run = str_of(&asked["runId"]);
    assert_eq!(wait(&factory, &ask_run), RunState::InputRequired);
    let (_, read) = john.get(
        "conversation",
        &format!("conversationId={}", str_of(&asked["conversationId"])),
    );
    let question = &read["pending"]["questions"][0];
    assert_eq!(
        question["options"][0],
        json!({"label": "Red", "recommended": true}),
        "{read}"
    );
    let question_id = str_of(&question["questionId"]);
    let (status, _) = ann.post(
        "answer",
        &json!({"runId": ask_run, "questionId": question_id, "choices": ["Blue"]}),
    );
    assert_eq!(status, 404);
    let (status, _) = john.post(
        "answer",
        &json!({"runId": ask_run, "questionId": "NOT_IT", "choices": ["Blue"]}),
    );
    assert_eq!(status, 409);
    let (status, answered) = john.post(
        "answer",
        &json!({"runId": ask_run, "questionId": question_id, "choices": ["Blue"]}),
    );
    assert_eq!(status, 200, "{answered}");
    assert_eq!(wait(&factory, &ask_run), RunState::Completed);

    // a hand-off's outcome is a signal: the watch's status as the server has it; another user's is not found
    {
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut watch = facade.make_value("moqui.agent.AgentHandoff").unwrap();
        for (name, value) in [
            ("handoffId", "AST_WATCH"),
            ("runId", approval_run.as_str()),
            ("userId", "EX_JOHN_DOE"),
            ("statusId", "AghWatching"),
        ] {
            watch.set(name, Value::string(value));
        }
        watch.create(&facade).unwrap();
    }
    let (status, signal) = john.post(
        "submitted",
        &json!({"handoffId": "AST_WATCH", "ok": false,
        "errors": ["Description is required"]}),
    );
    assert_eq!((status, signal["status"].as_str()), (200, Some("watching")), "{signal}");
    let (status, _) = ann.post("submitted", &json!({"handoffId": "AST_WATCH", "ok": true}));
    assert_eq!(status, 404);

    // suggested prompts for the page, the @ pickers, and the calls of later phases
    let (status, prompts) = john.get("prompts", "path=/qapps/system/Llm");
    assert_eq!(status, 200, "{prompts}");
    assert_eq!(prompts["prompts"][0]["label"], "Find users", "{prompts}");
    assert_eq!(prompts["prompts"][0]["send"], true);
    let (_, prompts) = john.get("prompts", "path=/qapps/tools");
    assert_eq!(prompts["prompts"].as_array().unwrap().len(), 0, "{prompts}");
    let (status, mentions) = john.get("mentions", "kind=agents&q=assi");
    assert_eq!(
        (status, mentions["items"][0]["agentId"].as_str()),
        (200, Some("assist")),
        "{mentions}"
    );
    let (status, mentions) = john.get("mentions", "kind=skills");
    assert_eq!((status, mentions["kind"].as_str()), (200, Some("skills")), "{mentions}");
    let (_, mentions) = john.get("mentions", "kind=skills&q=dates");
    assert_eq!(mentions["items"][0]["name"].as_str(), Some("dates"), "{mentions}");
    let (status, mentions) = john.get("mentions", "kind=records&q=john");
    assert_eq!(
        (status, mentions["kind"].as_str()),
        (200, Some("records")),
        "{mentions}"
    );
    assert!(mentions["groups"].is_array(), "{mentions}");
    let (status, _) = bob.get("mentions", "kind=records&q=john");
    assert_eq!(status, 403);
    // the calls of 13.14 (the_drawer_extras_over_http_on_postgres): a file and an action are required, and dictation
    // waits for a model bound to transcribe
    for (call, code) in [("attachments", (400, "invalid")), ("transcribe", (501, "not_available")), ("textAction", (400, "invalid"))] {
        let (status, answer) = john.post(call, &json!({}));
        assert_eq!((status, answer["code"].as_str()), (code.0, Some(code.1)), "{call}: {answer}");
    }
    // the canvas calls need a canvas of the user's (artifactId), the read proxy a path
    for call in ["canvasState", "canvasAction", "canvasAdjust", "canvasProblems"] {
        let (status, answer) = john.post(call, &json!({}));
        assert_eq!((status, answer["code"].as_str()), (400, Some("invalid")), "{call}: {answer}");
        let (status, answer) = john.post(call, &json!({"artifactId": "NO_SUCH_CANVAS", "text": "x"}));
        assert_eq!((status, answer["code"].as_str()), (404, Some("not_found")), "{call}: {answer}");
    }
    let (status, _) = john.get("read", "");
    assert_eq!(status, 400);

    // the admin screens: lists for an admin; another user's content needs AgentAudit, and each such read is logged
    let admin = basic("john.doe", "moqui");
    let screen = |path: &str| {
        let (base, query) = path.split_once('?').unwrap_or((path, ""));
        let target = if query.is_empty() {
            format!("{base}.html")
        } else {
            format!("{base}.html?{query}")
        };
        let (status, _, page) = exchange(address, &http_request("GET", &target, &[("Authorization", &admin)], ""));
        assert!(
            status == 200 && !page.contains("Error rendering"),
            "{path}: {status}: {}",
            page.chars().take(3000).collect::<String>()
        );
        page
    };
    assert!(screen("/System/Llm/Conversations/ConversationList").contains(&ann_conversation));
    assert!(screen("/System/Llm/Runs/RunList").contains(&ann_run));
    assert!(screen("/System/Llm/Approvals/ApprovalList").contains(&approval_id));
    assert!(
        screen(&format!(
            "/System/Llm/Approvals/ApprovalDetail?approvalId={approval_id}"
        ))
        .contains("After")
    );
    assert!(screen(&format!("/System/Llm/Runs/RunDetail?runId={approval_run}")).contains("approval.requested"));
    // the skills: the synced files, one with its sections, and what retrieval makes of a query
    assert!(screen("/System/Llm/Skills/SkillList").contains("screen-use"));
    let dates = rows(&factory, "moqui.agent.AgentSkill", "skillName", "dates");
    let dates_id = field(&dates[0], "skillId");
    assert!(screen(&format!("/System/Llm/Skills/SkillDetail?skillId={dates_id}")).contains("dates#phrases"));
    let tested = screen("/System/Llm/Skills/RetrievalTest?query=add+a+login+for+Jane");
    assert!(
        tested.contains("create-user-account") && tested.contains("skills that may fit: create-user-account"),
        "{}",
        tested.chars().take(3000).collect::<String>()
    );
    assert!(screen("/System/Llm/PromptPreview").contains("- screen-use: Use when"));
    let audit_rows = || {
        rows(
            &factory,
            "moqui.agent.AgentAuditLog",
            "conversationId",
            &ann_conversation,
        )
        .len()
    };
    let page = screen(&format!(
        "/System/Llm/Conversations/ConversationDetail?conversationId={ann_conversation}"
    ));
    assert!(
        !page.contains("Two more") && page.contains("needs the AgentAudit permission"),
        "{page}"
    );
    let page = screen(&format!("/System/Llm/Runs/RunDetail?runId={ann_run}"));
    assert!(
        !page.contains("One.") && page.contains("needs the AgentAudit permission"),
        "{page}"
    );
    assert_eq!(audit_rows(), 0);
    // the service itself: for an admin without AgentAudit the runs are metadata only, no answer, reason or error text
    {
        let viewer = factory.new_context();
        let _entered = viewer.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        // as the screen calls it: the artifact check is the screen's, the content check the service's own
        let _authz = viewer.disable_authz();
        let parameters = [("conversationId".to_string(), Value::string(&ann_conversation))]
            .into_iter()
            .collect();
        let out =
            moqui_core::service::sync("org.moqui.agent.AgentAdminServices.get#AgentAuditView", &parameters).unwrap();
        assert_eq!(out.get("shown"), Some(&Value::Bool(false)), "{out:?}");
        let Some(Value::List(runs)) = out.get("runList") else {
            panic!("no runList: {out:?}")
        };
        assert!(!runs.is_empty());
        for run in runs {
            let Value::Map(run) = run else { panic!("{run:?}") };
            for field in ["resultJson", "statusReason", "errorText"] {
                assert!(!run.contains_key(field), "{field} of another user's run: {run:?}");
            }
            assert!(run.contains_key("statusId"));
        }
        assert!(!format!("{out:?}").contains("One."), "no answer text: {out:?}");
    }
    {
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        for (entity, fields) in [
            (
                "moqui.security.UserGroup",
                vec![("userGroupId", "AST_AUDIT"), ("description", "Assist auditors")],
            ),
            (
                "moqui.security.UserGroupMember",
                vec![
                    ("userGroupId", "AST_AUDIT"),
                    ("userId", "EX_JOHN_DOE"),
                    ("fromDate", "2020-01-01 00:00:00"),
                ],
            ),
            (
                "moqui.security.UserGroupPermission",
                vec![
                    ("userGroupId", "AST_AUDIT"),
                    ("userPermissionId", "AgentAudit"),
                    ("fromDate", "2020-01-01 00:00:00"),
                ],
            ),
        ] {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(name, Value::string(value));
            }
            row.create(&facade).unwrap();
        }
    }
    let page = screen(&format!(
        "/System/Llm/Conversations/ConversationDetail?conversationId={ann_conversation}"
    ));
    assert!(page.contains("Two more") && page.contains("is logged"), "{page}");
    assert_eq!(audit_rows(), 1);
    let log = &rows(
        &factory,
        "moqui.agent.AgentAuditLog",
        "conversationId",
        &ann_conversation,
    )[0];
    assert_eq!(
        (field(log, "auditorUserId"), field(log, "targetUserId")),
        ("EX_JOHN_DOE".to_string(), "AST_ANN".to_string())
    );
    screen(&format!("/System/Llm/Runs/RunDetail?runId={ann_run}"));
    assert_eq!(audit_rows(), 2, "every read is logged");
    // reading one's own content is not an audit
    screen(&format!(
        "/System/Llm/Conversations/ConversationDetail?conversationId={conversation_id}"
    ));
    assert_eq!(
        rows(
            &factory,
            "moqui.agent.AgentAuditLog",
            "conversationId",
            &conversation_id
        )
        .len(),
        0
    );
}

const CANVAS_DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="assist-fake" protocol="fake" description="Assist fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="assist-fast" endpointId="assist-fake" providerModel="fake-fast" tier="fast"
        contextWindow="32768"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="assist-fast" reasoningLevel="none"/>
    <moqui.security.UserGroup userGroupId="AST_CANVAS" description="Canvas before"/>
    <moqui.security.UserAccount userId="AST_ANN" username="ann.assist" userFullName="Ann Assist"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA" locale="en_US" timeZone="US/Central"/>
    <moqui.security.UserGroup userGroupId="AST_AGENT_USERS" description="Assist users"/>
    <moqui.security.UserGroupMember userGroupId="AST_AGENT_USERS" userId="AST_ANN" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="AST_AGENT_USERS" userPermissionId="AgentUse" fromDate="1265184000000"/>
    <!-- an agent whose role group permits nothing: its canvases may not read or write -->
    <moqui.security.UserGroup userGroupId="AST_CANVAS_ROLE" description="A role that permits nothing"/>
    <moqui.agent.AgentDefinition agentId="canvas-limited" agentName="Limited" featureId="assist" roleGroupId="AST_CANVAS_ROLE"
        surfaces="drawer"/>
</entity-facade-xml>"#;

/// Make a canvas of `program` in `conversation_id` directly, as write_ui stores one.
fn make_canvas(factory: &Arc<ExecutionContextFactory>, conversation_id: &str, run_id: &str, program: &str, mutations: &[&str]) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let prepared = moqui_core::llm::ui::Prepared {
        program: program.to_string(),
        statements: vec!["root".to_string()],
        mutations: mutations.iter().map(|name| (*name).to_string()).collect(),
        reads: Vec::new(),
    };
    facade
        .transactions_for("moqui.agent.AgentArtifact")
        .unwrap()
        .run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
            moqui_core::llm::ui::Canvas::write_version(&facade, None, conversation_id, run_id, "openui", "Limited", &prepared)
        })
        .unwrap()
        .artifact_id
}

/// A click's nonces (`canvasAction` `kind=arm`).
fn arm(client: &Client, artifact_id: &str, version: i64) -> Vec<String> {
    let (status, armed) = client.post("canvasAction", &json!({"artifactId": artifact_id, "version": version, "kind": "arm", "count": 1, "activation": true}));
    assert_eq!(status, 200, "{armed}");
    armed["nonces"].as_array().unwrap().iter().map(str_of).collect()
}

/// A scripted reply that calls one tool.
fn tool_reply(id: &str, name: &str, arguments: &Json) -> fake::Reply {
    fake::Reply::steps(vec![
        Step::Tool {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
            pieces: 3,
        },
        Step::Finish("tool_calls".to_string()),
    ])
}

fn group_description(factory: &Arc<ExecutionContextFactory>) -> String {
    field(&rows(factory, "moqui.security.UserGroup", "userGroupId", "AST_CANVAS")[0], "description")
}

/// The tool results of a run, in order.
fn tool_results(factory: &Arc<ExecutionContextFactory>, conversation_id: &str) -> Vec<String> {
    let messages = rows(factory, "moqui.agent.AgentMessage", "conversationId", conversation_id);
    let mut out = Vec::new();
    let mut ordered: Vec<_> = messages.iter().filter(|row| field(row, "role") == "tool").collect();
    ordered.sort_by_key(|row| field(row, "messageSeq").parse::<i64>().unwrap_or(0));
    for message in ordered {
        for part in rows(factory, "moqui.agent.AgentMessagePart", "messageId", &field(message, "messageId")) {
            out.push(field(&part, "textValue"));
        }
    }
    out
}

/// The canvas over HTTP on embedded Postgres with the scripted fake (plans/LLM-A2A-Assist.md 13.6): write_ui answers
/// skill_required until openui-lang is loaded, a program that does not parse names its section, one that checks is
/// the conversation's canvas; the read proxy answers a screen list's rows by field name; a click's Mutation runs as the
/// approval with no card, one without the user's activation is refused; the agent's own run of a Mutation is a card.
#[test]
fn the_canvas_over_http_on_postgres() {
    let gate = start_with_data("assist-canvas", CANVAS_DATA);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let context = factory.new_context();
    let _entered = context.enter();
    fake::clear(ENDPOINT);
    let john = Client::sign_in(address, "john.doe");
    let (_, boot) = john.get("bootstrap", "");
    assert_eq!(boot["features"]["canvas"], true, "{boot}");
    let assist = boot["agents"].as_array().unwrap().iter().find(|agent| agent["agentId"] == "assist").unwrap();
    assert_eq!(assist["dialects"][0]["name"], "openui", "{boot}");

    let list = "/apps/system/Security/UserGroup/UserGroupList";
    let save = "/apps/system/Security/UserGroup/UserGroupDetail/updateUserGroup";
    let good = format!(
        "root = Stack([CardHeader(\"Groups\"), table, FormControl(\"Description\", Input(\"description\", $description)), saveButton])\n\
         groups = Query(\"request\", {{path: \"{list}\", query: {{userGroupId: \"AST_CANVAS\"}}}}, {{rows: []}})\n\
         table = Table([Col(\"Group\", groups.rows.userGroupId), Col(\"Description\", groups.rows.description)])\n\
         save = Mutation(\"request\", {{method: \"POST\", path: \"{save}\", body: {{userGroupId: \"AST_CANVAS\", description: $description}}}})\n\
         saveButton = Button(\"Save\", Action([@Run(save), @Run(groups)]))"
    );
    let broken = format!(
        "root = Stack([table])\ngroups = Query(\"request\", {{path: \"{list}\", query: {{userGroupId: $id}}, {{rows: []}})\ntable = Table([Col(\"Group\", groups.rows.userGroupId)])"
    );
    fake::script(
        ENDPOINT,
        vec![
            tool_reply("w0", "write_ui", &json!({"title": "Groups", "program": good})),
            tool_reply("s1", "skill", &json!({"name": "openui-lang"})),
            tool_reply("w1", "write_ui", &json!({"title": "Groups", "program": broken})),
            tool_reply("w2", "write_ui", &json!({"title": "Groups", "program": good})),
            fake::Reply::text("The canvas shows the group."),
        ],
    );
    let (status, started) = john.post("turn", &json!({"text": "Show the canvas group in a canvas", "clientMessageId": "john-c1", "clientId": "tab-a"}));
    assert_eq!(status, 200, "{started}");
    let run_id = str_of(&started["runId"]);
    let conversation_id = str_of(&started["conversationId"]);
    assert_eq!(wait(&factory, &run_id), RunState::Completed);
    let results = tool_results(&factory, &conversation_id);
    assert_eq!(results.len(), 4, "{results:?}");
    assert!(results[0].contains("\"skill_required\"") && results[0].contains("openui-lang"), "{}", results[0]);
    assert!(
        results[2].contains("\"openui_parse\"") && results[2].contains("\"skill\":\"openui-lang#query\""),
        "the parse error names its section: {}",
        results[2]
    );
    assert!(results[3].starts_with("canvas v1 shown: \"Groups\""), "{}", results[3]);
    assert!(!results[3].contains("Query("), "the result does not repeat the program: {}", results[3]);
    let events = rows(&factory, "moqui.agent.AgentRunEvent", "runId", &run_id);
    assert!(events.iter().any(|event| field(event, "eventType") == "canvas.open"), "no canvas.open event");

    // the conversation has the canvas; the read proxy answers its Query's rows by field name
    let (_, read) = john.get("conversation", &format!("conversationId={conversation_id}"));
    let canvas = &read["canvas"];
    assert_eq!((canvas["version"].as_i64(), canvas["kind"].as_str()), (Some(1), Some("openui")), "{read}");
    assert_eq!(canvas["mutations"], json!(["save"]));
    let artifact_id = str_of(&canvas["artifactId"]);
    let query = urlencoding(&json!({"userGroupId": "AST_CANVAS"}).to_string());
    let (status, rows_read) = john.get("read", &format!("artifactId={artifact_id}&path={list}&query={query}"));
    assert_eq!(status, 200, "{rows_read}");
    assert_eq!(rows_read["rows"][0]["userGroupId"], "AST_CANVAS", "{rows_read}");
    assert_eq!(rows_read["rows"][0]["description"], "Canvas before", "{rows_read}");
    // a transition that writes is never read
    let (status, refused) = john.get("read", &format!("artifactId={artifact_id}&path={save}"));
    assert_eq!(status, 403, "{refused}");
    // another user's canvas is not found
    let ann = Client::sign_in(address, "ann.assist");
    let (status, _) = ann.get("read", &format!("artifactId={artifact_id}&path={list}"));
    assert_eq!(status, 404);
    let (status, _) = ann.post("canvasAction", &json!({"artifactId": artifact_id, "kind": "mutation", "statement": "save", "activation": true}));
    assert_eq!(status, 404);

    // a click runs the program's Mutation with the posted state: the click is the approval, recorded with no card
    let click = json!({"artifactId": artifact_id, "version": 1, "kind": "mutation", "statement": "save",
        "state": {"description": "Canvas renamed"}, "activation": false});
    let (status, refused) = john.post("canvasAction", &click);
    assert_eq!(status, 403, "a write with no user activation: {refused}");
    assert_eq!(group_description(&factory), "Canvas before");
    let mut clicked = click.clone();
    clicked["activation"] = json!(true);
    let (status, _, text) = john.post_raw("canvasAction", &clicked, false);
    assert_eq!(status, 401, "without the session token: {text}");
    // a click runs a Mutation only with a nonce of its own (kind=arm), once
    let (status, refused) = john.post("canvasAction", &clicked);
    assert_eq!(status, 403, "no nonce: {refused}");
    let (status, refused) = john.post("canvasAction", &json!({"artifactId": artifact_id, "version": 1, "kind": "arm", "activation": false}));
    assert_eq!(status, 403, "arming with no activation: {refused}");
    let nonce = arm(&john, &artifact_id, 1).remove(0);
    clicked["nonce"] = json!(nonce);
    let (status, done) = john.post("canvasAction", &clicked);
    assert_eq!((status, done["ok"].as_bool()), (200, Some(true)), "{done}");
    assert_eq!(group_description(&factory), "Canvas renamed");
    let approval = &rows(&factory, "moqui.agent.AgentApproval", "approvalId", &str_of(&done["approvalId"]))[0];
    assert_eq!((field(approval, "decisionChannel"), field(approval, "statusId")), ("canvas".to_string(), "AgaExecuted".to_string()));
    let (_, read) = john.get("conversation", &format!("conversationId={conversation_id}"));
    assert_eq!(read["pending"]["approvals"], json!([]), "a click asks no card");
    let (status, again) = john.post("canvasAction", &clicked);
    assert_eq!(status, 403, "a nonce runs once: {again}");
    // an old version is a conflict, and the version is required
    let mut stale = clicked.clone();
    stale["version"] = json!(0);
    let (status, _) = john.post("canvasAction", &stale);
    assert_eq!(status, 409);
    let (status, _) = john.post("canvasState", &json!({"artifactId": artifact_id, "state": {"a": "b"}}));
    assert_eq!(status, 400, "no version");
    // a state that does not fit is refused, never cut
    let long = "x".repeat(4001);
    let (status, refused) = john.post("canvasState", &json!({"artifactId": artifact_id, "version": 1, "state": {"description": long}}));
    assert_eq!(status, 400, "{refused}");
    let nonce = arm(&john, &artifact_id, 1).remove(0);
    let (status, refused) = john.post("canvasAction", &json!({"artifactId": artifact_id, "version": 1, "kind": "mutation", "statement": "save",
        "nonce": nonce, "activation": true, "state": {"description": "x".repeat(4001)}}));
    assert_eq!((status, refused["ok"].as_bool()), (200, Some(false)), "{refused}");
    assert_eq!(group_description(&factory), "Canvas renamed");
    // the read proxy reads only /apps
    let (status, _) = john.get("read", &format!("artifactId={artifact_id}&path=/assist/bootstrap"));
    assert_eq!(status, 400);

    // the page's problems: mapped to their sections, kept once
    let problems = json!({"artifactId": artifact_id, "version": 1, "revalidate": false,
        "problems": [{"source": "query", "code": "tool-error", "message": "the Query groups failed", "statementId": "groups"},
                     {"source": "query", "code": "tool-error", "message": "the Query groups failed", "statementId": "groups"}]});
    let (status, mapped) = john.post("canvasProblems", &problems);
    assert_eq!(status, 200, "{mapped}");
    assert_eq!(mapped["problems"].as_array().unwrap().len(), 1, "{mapped}");
    assert_eq!(mapped["problems"][0]["skill"], "openui-lang#query");
    let (_, again) = john.post("canvasProblems", &problems);
    assert_eq!(again["new"], json!([]), "a problem reported again is not new: {again}");

    // the agent proposes the Mutation without a click: that is an approval card; approved, it runs with the canvas's state
    let (status, _) = john.post("canvasState", &json!({"artifactId": artifact_id, "version": 1, "state": {"description": "Agent run"}}));
    assert_eq!(status, 200);
    fake::script(
        ENDPOINT,
        vec![tool_reply("r1", "write_ui", &json!({"run": "save"})), fake::Reply::text("Saved it.")],
    );
    let (status, started) = john.post("turn", &json!({"text": "Save it for me", "clientMessageId": "john-c2", "clientId": "tab-a",
        "conversationId": conversation_id}));
    assert_eq!(status, 200, "{started}");
    let second = str_of(&started["runId"]);
    assert_eq!(wait(&factory, &second), RunState::InputRequired);
    assert_eq!(group_description(&factory), "Canvas renamed");
    let (_, read) = john.get("conversation", &format!("conversationId={conversation_id}"));
    let card = &read["pending"]["approvals"][0];
    assert_eq!(card["tool"], "write_ui", "{read}");
    assert!(card["changes"].to_string().contains("Agent run"), "{card}");
    let (status, decided) = john.post("decide", &json!({"approvalId": card["approvalId"], "decision": "approve"}));
    assert_eq!(status, 200, "{decided}");
    assert_eq!(wait(&factory, &second), RunState::Completed);
    assert_eq!(group_description(&factory), "Agent run");

    // a canvas of an agent whose role group permits nothing reads and writes nothing, whatever the user may do
    fake::script(ENDPOINT, vec![fake::Reply::text("Hello.")]);
    let (status, limited) = john.post("turn", &json!({"text": "Hi", "clientMessageId": "john-c3", "clientId": "tab-a", "agentId": "canvas-limited"}));
    assert_eq!(status, 200, "{limited}");
    assert_eq!(wait(&factory, &str_of(&limited["runId"])), RunState::Completed);
    let limited_canvas = make_canvas(&factory, &str_of(&limited["conversationId"]), &str_of(&limited["runId"]), &good, &["save"]);
    let (status, refused) = john.get("read", &format!("artifactId={limited_canvas}&path={list}"));
    assert_eq!(status, 403, "the agent's role group: {refused}");
    let nonce = arm(&john, &limited_canvas, 1).remove(0);
    let (status, refused) = john.post("canvasAction", &json!({"artifactId": limited_canvas, "version": 1, "kind": "mutation", "statement": "save",
        "nonce": nonce, "activation": true, "state": {"description": "Limited"}}));
    assert_eq!((status, refused["ok"].as_bool()), (200, Some(false)), "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("role does not permit"), "{refused}");
    assert_eq!(group_description(&factory), "Agent run");
    // a Mutation that posts a secret is refused: a canvas never posts one
    let secret = format!("root = Stack([b])\nsave = Mutation(\"request\", {{method: \"POST\", path: \"{save}\", body: {{userGroupId: \"AST_CANVAS\", password: $password}}}})\nb = Button(\"Go\", Action([@Run(save)]))");
    let secret_canvas = make_canvas(&factory, &conversation_id, &second, &secret, &["save"]);
    let nonce = arm(&john, &secret_canvas, 1).remove(0);
    let (status, refused) = john.post("canvasAction", &json!({"artifactId": secret_canvas, "version": 1, "kind": "mutation", "statement": "save",
        "nonce": nonce, "activation": true, "state": {"password": "hunter2"}}));
    assert_eq!((status, refused["ok"].as_bool()), (200, Some(false)), "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("secret"), "{refused}");

    // closing it: the tail no longer names it
    let (status, closed) = john.post("canvasState", &json!({"artifactId": artifact_id, "version": 1, "closed": true}));
    assert_eq!((status, closed["closed"].as_bool()), (200, Some(true)), "{closed}");
}

impl Client {
    /// A multipart post of one file field.
    fn post_file(&self, call: &str, field: &str, name: &str, content_type: &str, content: &str, extra: &[(&str, &str)]) -> (u16, Json) {
        let boundary = "assist-test-boundary";
        let mut body = String::new();
        for (key, value) in extra {
            body.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"));
        }
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n{content}\r\n--{boundary}--\r\n"
        ));
        let content_type = format!("multipart/form-data; boundary={boundary}");
        let headers = [
            ("Cookie", self.cookie.as_str()),
            ("Accept", "application/json"),
            ("Content-Type", content_type.as_str()),
            ("X-CSRF-Token", self.token.as_str()),
        ];
        let (status, _, text) = exchange(self.address, &http_request("POST", &format!("/assist/{call}"), &headers, &body));
        (status, parse(&text))
    }
}

/// The calls of phase 13.14 over HTTP on embedded Postgres with the scripted fake (docs/Assist.md): bootstrap's new
/// features and limits; a file attached, refused when it is a program or of a type not allowed, sent with a turn, read
/// by the agent with data, shown as a file part and downloaded by its user only; memory added, gated for one
/// conversation and forgotten, the user's own; Save as skill drafted from a run that used a screen, saved, listed and
/// shared for review; an inline text action proposed with its scaffold; a queued message canceled and not edited
/// after; the events through a screen transition; the response actions of prompts.
#[test]
fn the_drawer_extras_over_http_on_postgres() {
    let gate = start_with_data("assist-extras", DATA);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let context = factory.new_context();
    let _entered = context.enter();
    fake::clear(ENDPOINT);
    let john = Client::sign_in(address, "john.doe");
    let ann = Client::sign_in(address, "ann.assist");

    let (status, boot) = john.get("bootstrap", "");
    assert_eq!(status, 200, "{boot}");
    let features = &boot["features"];
    assert_eq!(
        (features["attachments"].as_bool(), features["transcribe"].as_bool(), features["textActions"].as_bool(), features["saveSkill"].as_bool()),
        (Some(true), Some(false), Some(true), Some(true)),
        "{boot}"
    );
    assert_eq!(boot["limits"]["maxAttachments"], 5, "{boot}");
    assert_eq!(boot["memory"]["preference"], true, "{boot}");
    let (_, ann_boot) = ann.get("bootstrap", "");
    assert_eq!(ann_boot["features"]["saveSkill"], false, "ann has no AgentSkillAuthor");

    // a file: checked, stored, sent with a turn and read by the agent
    // a program is refused by the web layer before the call (415), and by the call itself when it gets one
    let (status, refused) = john.post_file("attachments", "file", "tool.exe", "text/plain", "MZ\u{90}\u{0}rest", &[]);
    assert_eq!(status, 415, "{refused}");
    let (status, refused) = john.post_file("attachments", "file", "a.zip", "application/zip", "PK..", &[]);
    assert!(status == 400 && str_of(&refused["errors"]).contains("cannot be attached"), "{refused}");
    let (status, file) = john.post_file("attachments", "file", "../stock.csv", "text/csv", "product,qty\nseed packets,40\n", &[]);
    assert_eq!(status, 200, "{file}");
    assert_eq!((file["fileName"].as_str(), file["readable"].as_bool()), (Some("stock.csv"), Some(true)), "{file}");
    let attachment_id = str_of(&file["attachmentId"]);
    let reference = str_of(&file["ref"]);
    fake::script(
        ENDPOINT,
        vec![tool_reply("d1", "data", &json!({"artifact": reference})), fake::Reply::text("You have 40 seed packets.")],
    );
    let (status, started) = john.post(
        "turn",
        &json!({"text": "How many seed packets?", "clientMessageId": "x-m1", "clientId": "tab-x", "attachments": [attachment_id]}),
    );
    assert_eq!(status, 200, "{started}");
    let conversation_id = str_of(&started["conversationId"]);
    assert_eq!(wait(&factory, &str_of(&started["runId"])), RunState::Completed);
    let sent = fake::requests(ENDPOINT);
    let first_user = sent[sent.len() - 2]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "user")
        .cloned()
        .unwrap();
    assert!(
        str_of(&first_user["text"]).contains(&format!("[attached {reference}: stock.csv (text/csv, 28 bytes); read it with data artifact={reference}]")),
        "{first_user}"
    );
    let results = tool_results(&factory, &conversation_id);
    assert!(results[0].contains("seed packets,40"), "{results:?}");
    assert!(results[0].starts_with("<untrusted from=\"attachment\">"), "a file's text is data: {results:?}");
    let (_, read) = john.get("conversation", &format!("conversationId={conversation_id}"));
    let file_part = read["items"][0]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|part| part["kind"] == "file")
        .cloned()
        .unwrap_or(Json::Null);
    assert_eq!((file_part["attachmentId"].as_str(), file_part["size"].as_i64()), (Some(attachment_id.as_str()), Some(28)), "{read}");
    assert!(read["context"]["percent"].is_number(), "the context meter: {read}");
    assert_eq!(read["memory"]["effective"], true, "{read}");
    let (status, headers, body) = exchange(
        address,
        &http_request("GET", &format!("/assist/attachment?attachmentId={attachment_id}"), &[("Cookie", &john.cookie)], ""),
    );
    assert_eq!((status, body.as_str()), (200, "product,qty\nseed packets,40\n"));
    assert!(crate::common::header(&headers, "Content-Disposition").unwrap_or_default().contains("stock.csv"), "{headers:?}");
    let (status, _) = ann.get("attachment", &format!("attachmentId={attachment_id}"));
    assert_eq!(status, 404, "another user's file is not found");
    let (status, refused) = ann.post(
        "turn",
        &json!({"text": "Mine?", "clientMessageId": "ann-x1", "clientId": "tab-y", "attachments": [attachment_id]}),
    );
    assert_eq!((status, refused["code"].as_str()), (404, Some("not_found")), "{refused}");

    // memory: added, gated for one conversation, forgotten; the user's own
    let (status, refused) = john.post("memory", &json!({"op": "add", "text": "Prefers metric units", "conversationId": "NOT_HIS"}));
    assert_eq!(status, 404, "a conversation that is not the user's: {refused}");
    let (status, added) = john.post("memory", &json!({"op": "add", "text": "Prefers metric units", "days": 30}));
    assert_eq!(status, 200, "{added}");
    assert_eq!(added["memory"]["temporary"], true, "{added}");
    let memory_id = str_of(&added["memory"]["memoryId"]);
    let (_, listed) = john.get("memories", &format!("conversationId={conversation_id}"));
    assert_eq!(listed["memories"][0]["text"], "Prefers metric units", "{listed}");
    let (status, gated) = john.post(
        "memory",
        &json!({"op": "setting", "scope": "conversation", "conversationId": conversation_id, "create": false}),
    );
    assert_eq!(status, 200, "{gated}");
    assert_eq!(gated["memory"], json!({"preference": true, "conversation": false, "effective": false}), "{gated}");
    let (_, theirs) = ann.get("memories", "");
    assert_eq!(theirs["memories"], json!([]), "{theirs}");
    let (status, _) = ann.post("memory", &json!({"op": "forget", "memoryId": memory_id}));
    assert_eq!(status, 404);
    let (status, forgot) = john.post("memory", &json!({"op": "forget", "memoryId": memory_id}));
    assert_eq!((status, forgot["status"].as_str()), (200, Some("forgotten")), "{forgot}");

    // Save as skill: a run that used a screen, drafted (the model words it), saved, listed, shared
    fake::script(
        ENDPOINT,
        vec![
            tool_reply("s1", "screen", &json!({"go": "/apps/system/Security/UserAccount/UserAccountList"})),
            fake::Reply::text("That is the list of users."),
        ],
    );
    let (_, started) = john.post("turn", &json!({"text": "Show the user accounts", "clientMessageId": "x-m2", "clientId": "tab-x"}));
    let run_id = str_of(&started["runId"]);
    assert_eq!(wait(&factory, &run_id), RunState::Completed);
    fake::script(
        ENDPOINT,
        vec![fake::Reply::text(
            &json!({"name": "list-users", "title": "List the users", "description": "Use when the user asks to see the user accounts.",
                "steps": ["Open /apps/system/Security/UserAccount/UserAccountList."]})
            .to_string(),
        )],
    );
    let (status, draft) = john.post("skill", &json!({"op": "draft", "runId": run_id}));
    assert_eq!(status, 200, "{draft}");
    let markdown = str_of(&draft["markdown"]);
    assert!(markdown.contains("screens: [/apps/system/Security/UserAccount/UserAccountList]"), "{markdown}");
    let (status, refused) = ann.post("skill", &json!({"op": "draft", "runId": run_id}));
    assert_eq!(status, 403, "{refused}");
    let markdown = markdown.replacen("name: ", "name: x-", 1);
    // the client does not say it is learned (or says it is not): the server knows its draft, edited as it is
    let (status, saved) = john.post("skill", &json!({"op": "save", "markdown": markdown, "learned": false}));
    assert_eq!(status, 200, "{saved}");
    let skill_id = str_of(&saved["skillId"]);
    let (_, skills) = john.get("skills", "");
    assert_eq!(skills["skills"][0]["provenance"], "learned", "{skills}");
    // a skill the user wrote is theirs, not learned
    let own = "---\nname: z-my-own\ndescription: Use when I want my own way.\nkind: procedure\n---\nDo it my way.";
    let (status, mine) = john.post("skill", &json!({"op": "save", "markdown": own}));
    assert_eq!(status, 200, "{mine}");
    let (_, skills) = john.get("skills", "");
    let provenance: Vec<(String, String)> = skills["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| (str_of(&skill["name"]), str_of(&skill["provenance"])))
        .collect();
    assert!(provenance.contains(&("z-my-own".to_string(), "user".to_string())), "{provenance:?}");
    let (_, one) = john.get("skills", &format!("skillId={skill_id}"));
    assert!(str_of(&one["skill"]["markdown"]).starts_with("---\nname: x-"), "{one}");
    let (status, shared) = john.post("skill", &json!({"op": "share", "skillId": skill_id}));
    assert_eq!((status, shared["status"].as_str()), (200, Some("draft")), "{shared}");
    let (_, theirs) = ann.get("skills", "");
    assert_eq!(theirs["skills"], json!([]), "{theirs}");

    // an inline text action: the scaffold and task, the text wrapped; the answer is a proposal
    fake::script(ENDPOINT, vec![fake::Reply::text("The text is fixed.")]);
    let (status, proposed) = john.post("textAction", &json!({"action": "grammar", "text": "Teh text is fixd.", "field": "Note"}));
    assert_eq!(status, 200, "{proposed}");
    assert_eq!((proposed["text"].as_str(), proposed["original"].as_str()), (Some("The text is fixed."), Some("Teh text is fixd.")));
    let call = fake::requests(ENDPOINT).last().cloned().unwrap();
    assert!(str_of(&call["system"]).ends_with("Task: Fix the spelling, grammar and punctuation only; change nothing else."), "{call}");
    assert!(call.to_string().contains("<untrusted from=\\\"field Note\\\">Teh text is fixd.</untrusted>"), "{call}");
    let (status, _) = john.post("textAction", &json!({"action": "summarize", "text": "x"}));
    assert_eq!(status, 400);

    // a queued message canceled is not edited after; the events read through the screen transition
    fake::script(
        ENDPOINT,
        vec![fake::Reply::steps(vec![Step::Delay(1500), Step::Text("Slow.".to_string()), Step::Finish("stop".to_string())])],
    );
    let (_, started) = john.post("turn", &json!({"text": "Go slow", "clientMessageId": "x-m3", "clientId": "tab-x"}));
    let run_id = str_of(&started["runId"]);
    let (_, queued) = john.post(
        "turn",
        &json!({"text": "Never mind", "clientMessageId": "x-m4", "clientId": "tab-x", "conversationId": str_of(&started["conversationId"])}),
    );
    assert_eq!(queued["queued"], true, "{queued}");
    let input = rows(&factory, "moqui.agent.AgentInput", "clientMessageId", "x-m4");
    let input_id = field(&input[0], "inputId");
    let (status, canceled) = john.post("queued", &json!({"inputId": input_id, "op": "cancel"}));
    assert_eq!((status, canceled["status"].as_str()), (200, Some("canceled")), "{canceled}");
    let (status, late) = john.post("queued", &json!({"inputId": input_id, "op": "edit", "text": "Too late"}));
    assert_eq!((status, late["code"].as_str()), (409, Some("conflict")), "{late}");
    let (status, _) = ann.post("queued", &json!({"inputId": input_id, "op": "cancel"}));
    assert_eq!(status, 404);
    assert_eq!(wait(&factory, &run_id), RunState::Completed);
    let (status, events) = john.get("events", &format!("runId={run_id}&after=0"));
    assert_eq!(status, 200, "{events}");
    let types: Vec<String> = events["events"].as_array().unwrap().iter().map(|event| str_of(&event["type"])).collect();
    assert!(types.contains(&"run.done".to_string()) && !types.contains(&"input.folded".to_string()), "{types:?}");
    assert_eq!(events["state"], "TASK_STATE_COMPLETED");
    let (status, _) = ann.get("events", &format!("runId={run_id}"));
    assert_eq!(status, 404);

    // dictation: the model bound to transcribe (a loopback endpoint here) answers; the api quota is checked, the call is an
    // LlmCall (its use estimated) and each one counts in the AT_LLM tarpit (ann's allows one a minute)
    let (port, heard) = transcription_server();
    {
        let facade = factory.entity().unwrap();
        let _authz = context.disable_authz();
        for (entity, fields) in [
            ("moqui.llm.LlmEndpoint", vec![("endpointId", "assist-audio".to_string()), ("protocol", "chat-completions".to_string()),
                ("baseUrl", format!("http://127.0.0.1:{port}")), ("isLocal", "Y".to_string())]),
            ("moqui.llm.LlmModel", vec![("modelId", "assist-whisper".to_string()), ("endpointId", "assist-audio".to_string()),
                ("providerModel", "whisper-test".to_string()), ("tier", "fast".to_string())]),
            ("moqui.llm.LlmFeatureBinding", vec![("featureId", "transcribe".to_string()), ("modelId", "assist-whisper".to_string()),
                ("reasoningLevel", "none".to_string())]),
        ] {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(name, Value::string(value));
            }
            row.create(&facade).unwrap();
        }
    }
    let (_, boot) = john.get("bootstrap", "");
    assert_eq!(boot["features"]["transcribe"], true, "{boot}");
    let (status, heard_text) = john.post_file("transcribe", "audio", "dictation.webm", "audio/webm", "RIFFfakeaudio", &[("language", "en")]);
    assert_eq!((status, heard_text["text"].as_str()), (200, Some("Hello from the audio")), "{heard_text}");
    let request = heard.lock().unwrap().join("\n");
    assert!(request.contains("POST /v1/audio/transcriptions") && request.contains("whisper-test") && request.contains("RIFFfakeaudio"), "{request}");
    let calls = rows(&factory, "moqui.llm.LlmCall", "modelId", "assist-whisper");
    assert_eq!(calls.len(), 1);
    assert_eq!((field(&calls[0], "usageEstimated"), field(&calls[0], "userId")), ("Y".to_string(), "EX_JOHN_DOE".to_string()));
    // over the one a minute, the second locks the third out
    for take in ["first", "second"] {
        let (status, _) = ann.post_file("transcribe", "audio", "a.webm", "audio/webm", take, &[]);
        assert_eq!(status, 200);
    }
    let (status, limited) = ann.post_file("transcribe", "audio", "c.webm", "audio/webm", "third", &[]);
    assert_eq!((status, limited["code"].as_str()), (429, Some("tarpit")), "{limited}");
    let (status, refused) = john.post_file("transcribe", "audio", "x.txt", "text/plain", "not audio", &[]);
    assert_eq!(status, 400, "{refused}");
    // over the user's api quota, nothing is sent (the quota is checked before the tarpit)
    {
        let facade = factory.entity().unwrap();
        let _authz = context.disable_authz();
        let mut quota = facade.make_value("moqui.llm.LlmQuota").unwrap();
        for (name, value) in [("quotaId", "AST_ANN_API"), ("userId", "AST_ANN"), ("scope", "api"), ("maxTokens", "1")] {
            quota.set(name, Value::string(value));
        }
        quota.create(&facade).unwrap();
    }
    let sent_before = heard.lock().unwrap().len();
    let (status, over) = ann.post_file("transcribe", "audio", "d.webm", "audio/webm", "fourth", &[]);
    assert_eq!((status, over["code"].as_str()), (429, Some("quota")), "{over}");
    assert_eq!(heard.lock().unwrap().len(), sent_before, "nothing was sent");

    // the response actions: the framework's own download, and the apps' (none here)
    let (status, prompts) = john.get("prompts", "path=/qapps/system/Security");
    assert_eq!(status, 200, "{prompts}");
    assert_eq!(prompts["responseActions"][0]["key"], "download", "{prompts}");
}

/// A loopback transcription endpoint: each request is kept, each answered `{"text": "Hello from the audio"}`.
fn transcription_server() -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = Arc::clone(&heard);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 8192];
            // the headers, then the body by its length
            let body_len = loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break None,
                    Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                }
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|n| n.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    break Some(end + 4 + length);
                }
            };
            if let Some(total) = body_len {
                while bytes.len() < total {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                    }
                }
            }
            kept.lock().unwrap().push(String::from_utf8_lossy(&bytes).into_owned());
            let body = r#"{"text": "Hello from the audio"}"#;
            let _ = stream.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                    .as_bytes(),
            );
        }
    });
    (port, heard)
}
