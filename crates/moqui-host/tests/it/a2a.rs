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

//! The A2A server (docs/A2A.md; plans/LLM-A2A-Assist.md 4.15) over HTTP on a seeded server on embedded Postgres, with
//! the scripted fake provider. Ported from the Java wire tests (`A2ACoreTests`, `A2AJsonRpcTests`) and the
//! `interop_a2a_sdk.py` cases, against both bindings: the card and its URL against forwarded headers, auth, the
//! envelope and version errors, SendMessage with its idempotency per user, GetTask, ListTasks paging, ownership
//! (another user's task is not found through any operation), streaming frames of each binding, SubscribeToTask with
//! CancelTask racing it, an approval as AUTH_REQUIRED with its link, decided by the owner in a browser session and
//! resumed on its own, push notifications to a loopback receiver allowed by the test conf (token and authentication
//! headers, credentials never returned), and the parts kept in history. With `A2A_SDK_PYTHON` naming a python that has
//! `a2a-sdk`, the ported `tests/fixtures/a2a/interop_a2a_sdk.py` runs against the same server over both bindings.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "llm"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm::provider::fake::{self, Step};
use serde_json::{json, Value as Json};

use crate::common::{exchange, http_request, merge_cookies, session_token, sign_in, start_with_conf_properties};

const ENDPOINT: &str = "a2a-fake";

const CONF: &str = r#"<llm-facade><a2a enabled="true" blocking-wait="60">
    <push allow-http="false" allow-hosts="127.0.0.1" timeout="5" max-attempts="3"/></a2a></llm-facade>"#;

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="a2a-fake" protocol="fake" description="A2A fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="a2a-model" endpointId="a2a-fake" providerModel="fake-a2a" tier="balanced"
        contextWindow="32768"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="a2a-model" reasoningLevel="none"/>
    <moqui.agent.AgentDefinition agentId="assist" surfaces="drawer,chat,tui,a2a" exposeA2a="Y"
        a2aSkillsJson='[{"id":"answer-from-data","name":"Answer from data","description":"Finds records and answers","tags":["data"]}]'/>
    <moqui.agent.AgentDefinition agentId="a2a_hidden" agentName="Hidden" surfaces="a2a" featureId="assist"/>
    <moqui.basic.EnumerationType enumTypeId="A2A_TYPE" description="Before"/>
    <moqui.agent.AgentToolExposure toolName="update_enum_type" serviceName="update#moqui.basic.EnumerationType"
        title="Change an enumeration type" lockVersion="1"/>
    <moqui.security.ArtifactGroup artifactGroupId="A2A_TEST" description="A2A test"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="A2A_TEST" artifactName=".*EnumerationType"
        nameIsPattern="Y" artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="A2A_TEST" artifactName="moqui.basic.EnumerationType"
        artifactTypeEnumId="AT_ENTITY"/>
    <moqui.security.UserAccount userId="A2A_ANN" username="ann.a2a" userFullName="Ann A2A"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA" locale="en_US" timeZone="US/Central"/>
    <moqui.security.UserAccount userId="A2A_BOB" username="bob.a2a" userFullName="Bob A2A"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="A2A_CARL" username="carl.a2a" userFullName="Carl A2A"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="A2A_CALLERS" description="A2A callers"/>
    <moqui.security.UserGroupMember userGroupId="A2A_CALLERS" userId="A2A_ANN" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="A2A_CALLERS" userId="A2A_BOB" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="A2A_CALLERS" userPermissionId="AgentUse" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="A2A_CALLERS" userPermissionId="A2aServerUse" fromDate="1265184000000"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="A2A_TEST_CALLERS" userGroupId="A2A_CALLERS" artifactGroupId="A2A_TEST"
        authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    <!-- ann's drive, with a file her messages may name; bob may not read it -->
    <moqui.resource.ResourceDrive driveId="A2AD" driveName="A2A drive" driveKindEnumId="RdkShared"
        rootLocation="dbresource://a2adrive" ownerUserId="A2A_ANN"/>
    <moqui.resource.DbResource resourceId="A2AR1" filename="a2adrive" isFile="N"/>
    <moqui.resource.DbResource resourceId="A2AR2" parentResourceId="A2AR1" filename="brief.txt" isFile="Y"/>
    <!-- carl uses the agents in the drawer but may not call them over A2A -->
    <moqui.security.UserGroup userGroupId="A2A_DRAWER" description="Drawer only"/>
    <moqui.security.UserGroupMember userGroupId="A2A_DRAWER" userId="A2A_CARL" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="A2A_DRAWER" userPermissionId="AgentUse" fromDate="1265184000000"/>
</entity-facade-xml>"#;

/// A caller with a login key, over both bindings.
struct Caller {
    address: SocketAddr,
    key: String,
}

impl Caller {
    fn headers(&self) -> Vec<(String, String)> {
        vec![
            ("Authorization".to_string(), format!("Bearer {}", self.key)),
            ("A2A-Version".to_string(), "1.0".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ]
    }

    fn send(&self, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> (u16, Vec<(String, String)>, Json) {
        let mut headers = self.headers();
        for (name, value) in extra {
            headers.retain(|(known, _)| !known.eq_ignore_ascii_case(name));
            if !value.is_empty() {
                headers.push(((*name).to_string(), (*value).to_string()));
            }
        }
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let (status, headers, text) = exchange(self.address, &http_request(method, path, &refs, body));
        (
            status,
            headers,
            serde_json::from_str(&text).unwrap_or_else(|_| json!({"unparsed": text})),
        )
    }

    /// One JSON-RPC call: its HTTP status and response.
    fn rpc(&self, method: &str, params: Json) -> (u16, Json) {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let (status, _, response) = self.send("POST", "/a2a", &[], &body);
        (status, response)
    }

    /// The result of a JSON-RPC call that must succeed.
    fn ok(&self, method: &str, params: Json) -> Json {
        let (status, response) = self.rpc(method, params);
        assert!(
            status == 200 && response.get("result").is_some(),
            "{method}: {status} {response}"
        );
        response["result"].clone()
    }

    /// The error code of a JSON-RPC call that must fail.
    fn code(&self, method: &str, params: Json) -> i64 {
        let (status, response) = self.rpc(method, params);
        assert_eq!(status, 200, "{response}");
        response["error"]["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("{method} did not fail: {response}"))
    }

    fn rest(&self, method: &str, path: &str, body: &str) -> (u16, Json) {
        let (status, _, response) = self.send(method, &format!("/a2a{path}"), &[], body);
        (status, response)
    }

    /// The `data:` payloads of an event stream, read to its end (HTTP/1.0, so the body is not chunked).
    fn stream(&self, method: &str, path: &str, body: &str) -> Vec<Json> {
        let mut request = format!("{method} {path} HTTP/1.0\r\nHost: localhost\r\nAccept: text/event-stream\r\n");
        for (name, value) in self.headers() {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out);
        let text = String::from_utf8_lossy(&out).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        assert!(head.contains("text/event-stream"), "{text}");
        body.split("\n\n")
            .filter_map(|frame| frame.lines().find_map(|line| line.strip_prefix("data: ")))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect()
    }
}

fn message(id: &str, text: &str) -> Json {
    json!({"messageId": id, "role": "ROLE_USER", "parts": [{"text": text}]})
}

fn login_key(factory: &Arc<ExecutionContextFactory>, username: &str) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username).unwrap();
    moqui_core::user::get_login_key(None).unwrap()
}

fn slow(text: &str, ms: u64) -> fake::Reply {
    fake::Reply::steps(vec![
        Step::Delay(ms),
        Step::Text(text.to_string()),
        Step::Finish("stop".to_string()),
    ])
}

/// One push notification received: its headers (lower case) and JSON body.
type Post = (Vec<(String, String)>, Json);

/// A loopback receiver of push notifications.
struct Receiver {
    port: u16,
    posts: Arc<Mutex<Vec<Post>>>,
}

impl Receiver {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let posts = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&posts);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                let (headers, body) = loop {
                    let Ok(count) = stream.read(&mut chunk) else {
                        break (Vec::new(), Json::Null);
                    };
                    if count == 0 {
                        break (Vec::new(), Json::Null);
                    }
                    buffer.extend_from_slice(&chunk[..count]);
                    let text = String::from_utf8_lossy(&buffer).into_owned();
                    let Some((head, rest)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let headers: Vec<(String, String)> = head
                        .lines()
                        .skip(1)
                        .filter_map(|line| line.split_once(':'))
                        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
                        .collect();
                    let length: usize = headers
                        .iter()
                        .find(|(name, _)| name == "content-length")
                        .and_then(|(_, value)| value.parse().ok())
                        .unwrap_or(0);
                    if rest.len() >= length {
                        break (headers, serde_json::from_str(&rest[..length]).unwrap_or(Json::Null));
                    }
                };
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                kept.lock().unwrap().push((headers, body));
            }
        });
        Self { port, posts }
    }

    /// The posts for `task_id` once one carries `state`, waiting up to a minute.
    fn wait_for(&self, task_id: &str, state: &str) -> Vec<Post> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let posts: Vec<_> = self
                .posts
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, body)| {
                    body.pointer("/statusUpdate/taskId")
                        .or_else(|| body.pointer("/artifactUpdate/taskId"))
                        == Some(&json!(task_id))
                })
                .cloned()
                .collect();
            if posts
                .iter()
                .any(|(_, body)| body.pointer("/statusUpdate/status/state") == Some(&json!(state)))
            {
                return posts;
            }
            assert!(
                Instant::now() < deadline,
                "no push with {state} for {task_id}: {posts:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A browser session's cookie and token, for the owner's decision.
fn browser(address: SocketAddr, username: &str) -> (String, String) {
    let cookie = sign_in(address, username, "moqui");
    let (_, headers, html) = exchange(
        address,
        &http_request("GET", "/qapps", &[("Cookie", &cookie), ("Accept", "text/html")], ""),
    );
    let token = session_token(&html)
        .or_else(|| crate::common::header(&headers, "moquiSessionToken"))
        .expect("a session token");
    (merge_cookies(&cookie, &headers), token)
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
        .eq(field, moqui_lib::object::Value::string(value))
        .list()
        .unwrap()
        .values()
        .to_vec()
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}

fn states(frames: &[Json], pointer: &str) -> Vec<String> {
    frames
        .iter()
        .filter_map(|frame| frame.pointer(pointer).and_then(Json::as_str).map(str::to_string))
        .collect()
}

#[test]
fn the_a2a_server_over_both_bindings_on_postgres() {
    let gate = start_with_conf_properties("a2a", Some(DATA), CONF, &[]);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    fake::clear(ENDPOINT);
    let ann = Caller {
        address,
        key: login_key(&factory, "ann.a2a"),
    };
    let bob = Caller {
        address,
        key: login_key(&factory, "bob.a2a"),
    };
    let carl = Caller {
        address,
        key: login_key(&factory, "carl.a2a"),
    };

    // the public card: both interfaces at the request's origin, forwarded headers not believed
    let (status, headers, card) = exchange(
        address,
        &http_request(
            "GET",
            "/.well-known/agent-card.json",
            &[("X-Forwarded-Host", "evil.example.com")],
            "",
        ),
    );
    let card: Json = serde_json::from_str(&card).unwrap();
    assert_eq!(status, 200, "{card}");
    assert_eq!(
        crate::common::header(&headers, "cache-control").as_deref(),
        Some("public, max-age=300")
    );
    assert_eq!(
        card["supportedInterfaces"],
        json!([{"url": "http://localhost/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
            {"url": "http://localhost/a2a", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"}])
    );
    assert_eq!(
        card["capabilities"],
        json!({"streaming": true, "pushNotifications": true, "extendedAgentCard": true})
    );
    assert!(card["securitySchemes"]["bearer"].is_object() && card["securitySchemes"]["apiKey"].is_object());

    // auth: none is 401 with a challenge, a bad key 401, a user without A2aServerUse 403
    let rpc_body = json!({"jsonrpc": "2.0", "id": 1, "method": "GetTask", "params": {"id": "x"}}).to_string();
    let (status, headers, _) = exchange(
        address,
        &http_request("POST", "/a2a", &[("A2A-Version", "1.0")], &rpc_body),
    );
    assert_eq!(status, 401);
    assert!(crate::common::header(&headers, "www-authenticate").is_some_and(|value| value.contains("Bearer")));
    let (status, _, _) = ann.send("POST", "/a2a", &[("Authorization", "Bearer not-a-key")], &rpc_body);
    assert_eq!(status, 401);
    let (status, refused) = carl.rpc("GetTask", json!({"id": "x"}));
    assert_eq!(status, 403, "{refused}");
    // Basic takes an app password (or a login key), never the account password
    let app_password = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("ann.a2a").unwrap();
        let _authz = context.disable_authz();
        let parameters: std::collections::BTreeMap<String, moqui_lib::object::Value> =
            [("name".to_string(), moqui_lib::object::Value::string("A2A test"))]
                .into_iter()
                .collect();
        let out = moqui_core::service::sync("org.moqui.impl.UserServices.create#UserAppPassword", &parameters).unwrap();
        moqui_lib::object::to_plain_string(out.get("password").expect("an app password"))
    };
    let (status, _, by_app) = ann.send(
        "POST",
        "/a2a",
        &[("Authorization", &crate::common::basic("ann.a2a", &app_password))],
        &rpc_body,
    );
    assert_eq!(
        (status, by_app["error"]["code"].as_i64()),
        (200, Some(-32001)),
        "{by_app}"
    );
    let (status, _, _) = ann.send(
        "POST",
        "/a2a",
        &[("Authorization", &crate::common::basic("ann.a2a", "moqui"))],
        &rpc_body,
    );
    assert_eq!(status, 401, "the account password is refused");

    // the envelope and the version
    let (status, _, missing) = ann.send("POST", "/a2a", &[("A2A-Version", "")], &rpc_body);
    assert_eq!(
        (status, missing["error"]["code"].as_i64()),
        (200, Some(-32009)),
        "{missing}"
    );
    let (_, _, old) = ann.send("POST", "/a2a", &[("A2A-Version", "0.3")], &rpc_body);
    assert_eq!(old["error"]["code"], -32009);
    assert_eq!(old["error"]["data"][0]["reason"], "VERSION_NOT_SUPPORTED");
    let (_, _, batch) = ann.send("POST", "/a2a", &[], &format!("[{rpc_body}]"));
    assert_eq!(batch["error"]["code"], -32600);
    let (_, _, parse) = ann.send("POST", "/a2a", &[], r#"{"jsonrpc":"#);
    assert_eq!(parse["error"]["code"], -32700);
    assert_eq!(ann.code("message/send", json!({})), -32601, "a 0.3 method name");
    let (_, string_id) = {
        let body = json!({"jsonrpc": "2.0", "id": "req-7", "method": "GetTask", "params": {"id": "nope"}}).to_string();
        let (status, _, response) = ann.send("POST", "/a2a", &[], &body);
        (status, response)
    };
    assert_eq!(
        (string_id["id"].as_str(), string_id["error"]["code"].as_i64()),
        (Some("req-7"), Some(-32001))
    );

    // the extended card: the exposed agents' skills only
    let extended = ann.ok("GetExtendedAgentCard", json!({}));
    let skills: Vec<&str> = extended["skills"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|skill| skill["id"].as_str())
        .collect();
    assert_eq!(skills, ["answer-from-data"], "{extended}");

    // SendMessage, blocking: the task completes with its response artifact and history
    fake::script(
        ENDPOINT,
        vec![fake::Reply::text("Hello from A2A."), fake::Reply::text("Hello, Bob.")],
    );
    let sent = ann.ok(
        "SendMessage",
        json!({"message": message("ann-m1", "Hello, who are you?")}),
    );
    let task = &sent["task"];
    let task_id = task["id"].as_str().unwrap().to_string();
    let context_id = task["contextId"].as_str().unwrap().to_string();
    assert_eq!(task["status"]["state"], "TASK_STATE_COMPLETED", "{sent}");
    assert_eq!(
        task["artifacts"],
        json!([{"artifactId": "response", "name": "response", "parts": [{"text": "Hello from A2A."}]}])
    );
    let roles: Vec<&str> = task["history"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["role"].as_str())
        .collect();
    assert_eq!(roles, ["ROLE_USER", "ROLE_AGENT"], "{task}");
    assert_eq!(task["history"][0]["messageId"], "ann-m1");
    assert_eq!(task["history"][1]["contextId"].as_str(), Some(context_id.as_str()));
    // the same messageId again is the same task, and the model is not called again; another user's is their own
    let calls = fake::requests(ENDPOINT).len();
    let again = ann.ok(
        "SendMessage",
        json!({"message": message("ann-m1", "Hello, who are you?")}),
    );
    assert_eq!(again["task"]["id"].as_str(), Some(task_id.as_str()));
    assert_eq!(fake::requests(ENDPOINT).len(), calls);
    let bobs = bob.ok("SendMessage", json!({"message": message("ann-m1", "Hello")}));
    let bob_task = bobs["task"]["id"].as_str().unwrap().to_string();
    assert_ne!(bob_task, task_id);

    // GetTask, ownership, history length; CancelTask of an ended task
    let got = ann.ok("GetTask", json!({"id": task_id, "historyLength": 1}));
    assert_eq!(got["history"].as_array().map(Vec::len), Some(1), "{got}");
    assert_eq!(got["history"][0]["role"], "ROLE_AGENT");
    assert_eq!(
        ann.ok("GetTask", json!({"id": task_id, "historyLength": 0}))["history"],
        Json::Null
    );
    assert_eq!(bob.code("GetTask", json!({"id": task_id})), -32001);
    assert_eq!(bob.code("CancelTask", json!({"id": task_id})), -32001);
    assert_eq!(bob.code("SubscribeToTask", json!({"id": task_id})), -32001);
    assert_eq!(
        bob.code(
            "CreateTaskPushNotificationConfig",
            json!({"taskId": task_id, "url": "https://hooks.example.com/x"})
        ),
        -32001
    );
    assert_eq!(
        bob.code(
            "SendMessage",
            json!({"message": {"messageId": "bob-m2", "role": "ROLE_USER", "taskId": task_id,
            "parts": [{"text": "mine now"}]}})
        ),
        -32001
    );
    assert_eq!(ann.code("CancelTask", json!({"id": task_id})), -32002);
    assert_eq!(ann.code("SubscribeToTask", json!({"id": task_id})), -32004);
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": {"messageId": "ann-m1b", "role": "ROLE_USER", "taskId": task_id,
            "parts": [{"text": "more"}]}})
        ),
        -32004,
        "an ended task takes no message"
    );
    // checks before a task is made: parts, output modes, agents, url parts
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": {"messageId": "ann-bad", "role": "ROLE_USER",
            "parts": [{"text": "a", "data": {}}]}})
        ),
        -32602
    );
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": message("ann-bad2", "x"),
            "configuration": {"acceptedOutputModes": ["image/png"]}})
        ),
        -32005
    );
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": message("ann-bad3", "x"), "metadata": {"agentId": "a2a_hidden"}})
        ),
        -32602,
        "an agent not exposed over A2A"
    );
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": {"messageId": "ann-bad4", "role": "ROLE_USER",
            "parts": [{"url": "dbresource://a2a/none.txt"}]}})
        ),
        -32602
    );
    let url_error = |caller: &Caller, id: &str, location: &str| {
        let (_, response) = caller.rpc(
            "SendMessage",
            json!({"message": {"messageId": id, "role": "ROLE_USER", "parts": [{"url": location}]}}),
        );
        response["error"]["message"].as_str().unwrap_or_default().to_string()
    };
    let brief = "dbresource://a2adrive/brief.txt";
    assert_eq!(
        url_error(&bob, "bob-url", brief),
        format!("Part.url {brief} is not a location you can read"),
        "another user's file"
    );
    assert_eq!(
        url_error(&ann, "ann-url-missing", "dbresource://a2adrive/none.txt"),
        "Part.url dbresource://a2adrive/none.txt is not a location you can read",
        "a missing file is answered as an unreadable one"
    );

    // the HTTP+JSON binding: a task, an error's google.rpc shape and status, the version, a send
    let (status, rest_task) = ann.rest("GET", &format!("/tasks/{task_id}?historyLength=0"), "");
    assert_eq!(
        (status, rest_task["id"].as_str()),
        (200, Some(task_id.as_str())),
        "{rest_task}"
    );
    let (status, not_found) = bob.rest("GET", &format!("/tasks/{task_id}"), "");
    assert_eq!(status, 404, "{not_found}");
    assert_eq!(not_found["error"]["status"], "NOT_FOUND");
    assert_eq!(not_found["error"]["details"][0]["reason"], "TASK_NOT_FOUND");
    let (status, _, version) = ann.send("GET", &format!("/a2a/tasks/{task_id}"), &[("A2A-Version", "")], "");
    assert_eq!(
        (status, version["error"]["details"][0]["reason"].as_str()),
        (400, Some("VERSION_NOT_SUPPORTED"))
    );
    let (status, _) = ann.rest("POST", &format!("/tasks/{task_id}:cancel"), "");
    assert_eq!(status, 400, "an ended task is not cancelable");
    let (status, unknown) = ann.rest("GET", "/nothing/here", "");
    assert_eq!(
        (status, unknown["error"]["details"][0]["reason"].as_str()),
        (404, Some("METHOD_NOT_FOUND"))
    );
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::text("Over REST."),
            fake::Reply::text("And in the same context."),
        ],
    );
    let (status, rest_sent) = ann.rest(
        "POST",
        "/message:send",
        &json!({"message": {"messageId": "ann-r1", "role": "ROLE_USER",
            "parts": [{"text": "Over REST"}, {"data": {"orderId": "SO100"}, "mediaType": "application/json"},
                {"url": "dbresource://a2adrive/brief.txt", "filename": "brief.txt"}]}})
        .to_string(),
    );
    assert_eq!(status, 200, "{rest_sent}");
    assert_eq!(rest_sent["task"]["status"]["state"], "TASK_STATE_COMPLETED");
    assert_eq!(
        rest_sent["task"]["history"][0]["parts"],
        json!([{"text": "Over REST"}, {"data": {"orderId": "SO100"}, "mediaType": "application/json"},
            {"url": "dbresource://a2adrive/brief.txt", "filename": "brief.txt"}]),
        "the parts come back as they were sent; the owner may name a file of her drive"
    );
    let rest_context = rest_sent["task"]["contextId"].as_str().unwrap().to_string();
    let follow_up = ann.ok(
        "SendMessage",
        json!({"message": {"messageId": "ann-r2", "role": "ROLE_USER", "contextId": rest_context,
            "parts": [{"text": "And again"}]}}),
    );
    assert_eq!(
        follow_up["task"]["contextId"].as_str(),
        Some(rest_context.as_str()),
        "a new task in the context"
    );
    assert_ne!(follow_up["task"]["id"], rest_sent["task"]["id"]);
    assert_eq!(
        bob.code(
            "SendMessage",
            json!({"message": {"messageId": "bob-c", "role": "ROLE_USER", "contextId": rest_context,
            "parts": [{"text": "x"}]}})
        ),
        -32602,
        "another user's context"
    );

    // ListTasks: a stable keyset cursor, no duplicates, the caller's tasks only, filters
    let mut seen = Vec::new();
    let mut token = String::new();
    loop {
        let page = ann.ok(
            "ListTasks",
            json!({"pageSize": 1, "pageToken": token, "historyLength": 0}),
        );
        assert_eq!(page["totalSize"], 3, "{page}");
        for task in page["tasks"].as_array().unwrap() {
            seen.push(task["id"].as_str().unwrap().to_string());
        }
        token = page["nextPageToken"].as_str().unwrap_or_default().to_string();
        if token.is_empty() {
            break;
        }
    }
    let mut unique = seen.clone();
    unique.dedup();
    assert_eq!((seen.len(), unique.len()), (3, 3), "{seen:?}");
    assert!(!seen.contains(&bob_task));
    let (status, rest_page) = ann.rest(
        "GET",
        &format!("/tasks?contextId={rest_context}&status=TASK_STATE_COMPLETED"),
        "",
    );
    assert_eq!(
        (status, rest_page["tasks"].as_array().map(Vec::len)),
        (200, Some(2)),
        "{rest_page}"
    );
    assert_eq!(
        rest_page["tasks"][0].get("artifacts"),
        None,
        "artifacts only when asked for"
    );
    assert_eq!(ann.code("ListTasks", json!({"pageToken": "bm90LWEtY3Vyc29y"})), -32602);
    assert_eq!(ann.code("ListTasks", json!({"pageSize": 0})), -32602);

    // the same message sent four times at once is one task, and the model answers it once
    fake::script(ENDPOINT, vec![slow("Once.", 300)]);
    let calls = fake::requests(ENDPOINT).len();
    let senders: Vec<_> = (0..4)
        .map(|_| {
            let caller = Caller {
                address,
                key: ann.key.clone(),
            };
            std::thread::spawn(move || {
                caller.ok(
                    "SendMessage",
                    json!({"message": message("ann-dup", "Only once"), "configuration": {"returnImmediately": true}}),
                )["task"]["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        })
        .collect();
    let mut ids: Vec<String> = senders.into_iter().map(|sender| sender.join().unwrap()).collect();
    ids.dedup();
    assert_eq!(ids.len(), 1, "{ids:?}");
    let once = ann.ok("SendMessage", json!({"message": message("ann-dup", "Only once")}));
    assert_eq!(once["task"]["status"]["state"], "TASK_STATE_COMPLETED", "{once}");
    assert_eq!(fake::requests(ENDPOINT).len(), calls + 1, "one model call");

    // streaming, JSON-RPC: the task first, then its updates, each framed as a JSON-RPC response
    fake::script(
        ENDPOINT,
        vec![fake::Reply::steps(vec![
            Step::Text("Streamed ".to_string()),
            Step::Delay(400),
            Step::Text("answer.".to_string()),
            Step::Finish("stop".to_string()),
        ])],
    );
    let body = json!({"jsonrpc": "2.0", "id": "s1", "method": "SendStreamingMessage",
        "params": {"message": message("ann-s1", "Answer as a stream")}})
    .to_string();
    let frames = ann.stream("POST", "/a2a", &body);
    assert!(
        frames
            .iter()
            .all(|frame| frame["jsonrpc"] == "2.0" && frame["id"] == "s1"),
        "{frames:?}"
    );
    let results: Vec<Json> = frames.iter().map(|frame| frame["result"].clone()).collect();
    assert!(results[0].get("task").is_some(), "{results:?}");
    // the task as it stood when the stream began (submitted, or working already), then each state after it
    let mut seen_states = vec![results[0]["task"]["status"]["state"]
        .as_str()
        .unwrap_or_default()
        .to_string()];
    seen_states.extend(states(&results, "/statusUpdate/status/state"));
    assert!(seen_states.contains(&"TASK_STATE_WORKING".to_string()), "{results:?}");
    assert_eq!(
        seen_states.last().map(String::as_str),
        Some("TASK_STATE_COMPLETED"),
        "{results:?}"
    );
    let last_artifact = results
        .iter()
        .rev()
        .find_map(|result| result.get("artifactUpdate"))
        .unwrap();
    assert_eq!(last_artifact["lastChunk"], true);
    assert_eq!(last_artifact["artifact"]["parts"][0]["text"], "Streamed answer.");
    // the same over HTTP+JSON: bare StreamResponses
    fake::script(ENDPOINT, vec![fake::Reply::text("A REST stream.")]);
    let frames = ann.stream(
        "POST",
        "/a2a/message:stream",
        &json!({"message": message("ann-s2", "Stream over REST")}).to_string(),
    );
    assert!(
        frames[0].get("task").is_some() && frames.iter().all(|frame| frame.get("jsonrpc").is_none()),
        "{frames:?}"
    );
    assert_eq!(
        states(&frames, "/statusUpdate/status/state").last().map(String::as_str),
        Some("TASK_STATE_COMPLETED")
    );

    // returnImmediately, then SubscribeToTask while CancelTask races it: one terminal state, and the stream ends on it
    fake::script(ENDPOINT, vec![slow("Too late.", 5000)]);
    let started = ann.ok(
        "SendMessage",
        json!({"message": message("ann-c1", "Take your time"), "configuration": {"returnImmediately": true}}),
    );
    let slow_id = started["task"]["id"].as_str().unwrap().to_string();
    assert!(
        ["TASK_STATE_SUBMITTED", "TASK_STATE_WORKING"].contains(&started["task"]["status"]["state"].as_str().unwrap()),
        "{started}"
    );
    let subscriber = {
        let caller = Caller {
            address,
            key: ann.key.clone(),
        };
        let path = format!("/a2a/tasks/{slow_id}:subscribe");
        std::thread::spawn(move || caller.stream("POST", &path, ""))
    };
    std::thread::sleep(Duration::from_millis(700));
    let canceled = ann.ok("CancelTask", json!({"id": slow_id}));
    assert_eq!(canceled["status"]["state"], "TASK_STATE_CANCELED", "{canceled}");
    let frames = subscriber.join().unwrap();
    assert!(frames[0].get("task").is_some(), "{frames:?}");
    assert_eq!(
        states(&frames, "/statusUpdate/status/state").last().map(String::as_str),
        Some("TASK_STATE_CANCELED")
    );
    assert_eq!(
        ann.code("CancelTask", json!({"id": slow_id})),
        -32002,
        "a second cancel"
    );

    // a write: AUTH_REQUIRED with a link for the owner; no message moves it; the owner decides in Moqui and the task
    // goes on by itself; the push config gets each state with its token and authentication
    let receiver = Receiver::start();
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": message("ann-p0", "x"),
            "configuration": {"taskPushNotificationConfig": {"url": "http://10.1.2.3/hook"}}})
        ),
        -32602,
        "an internal address the conf does not allow"
    );
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": message("ann-p1", "x"),
            "configuration": {"taskPushNotificationConfig": {"url": "https://user@hooks.example.com/hook"}}})
        ),
        -32602
    );
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::steps(vec![
                Step::Tool {
                    id: "w1".to_string(),
                    name: "run_tool".to_string(),
                    arguments: json!({"name": "update_enum_type",
                        "parameters": {"enumTypeId": "A2A_TYPE", "description": "After"}})
                    .to_string(),
                    pieces: 1,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("Changed it."),
        ],
    );
    let hook = format!("http://127.0.0.1:{}/hook", receiver.port);
    let paused = ann.ok(
        "SendMessage",
        json!({"message": message("ann-w1", "Change A2A_TYPE to After"),
            "configuration": {"taskPushNotificationConfig": {"url": hook, "token": "tok-1", "id": "hook-1",
                "authentication": {"scheme": "Bearer", "credentials": "push-secret"}}}}),
    );
    let write_task = paused["task"]["id"].as_str().unwrap().to_string();
    let write_context = paused["task"]["contextId"].as_str().unwrap().to_string();
    assert_eq!(
        paused["task"]["status"]["state"], "TASK_STATE_AUTH_REQUIRED",
        "{paused}"
    );
    let parts = &paused["task"]["status"]["message"]["parts"];
    let approval = &parts[1]["data"];
    assert_eq!(approval["type"], "moqui.approval", "{paused}");
    assert_eq!(
        approval["url"],
        format!("http://localhost/qapps?assist={write_context}")
    );
    assert!(parts[0]["text"].as_str().unwrap().contains("approval"), "{paused}");
    let approval_id = approval["approvalId"].as_str().unwrap().to_string();
    assert_eq!(
        ann.code(
            "SendMessage",
            json!({"message": {"messageId": "ann-w2", "role": "ROLE_USER", "taskId": write_task,
            "parts": [{"data": {"confirmed": true}}]}})
        ),
        -32004,
        "a remote caller cannot approve"
    );
    let configs = ann.ok("ListTaskPushNotificationConfigs", json!({"taskId": write_task}));
    assert_eq!(
        configs["configs"],
        json!([{"id": "hook-1", "taskId": write_task, "url": hook, "authentication": {"scheme": "Bearer"}}]),
        "never the token or the credentials"
    );
    assert_eq!(
        bob.code(
            "GetTaskPushNotificationConfig",
            json!({"taskId": write_task, "id": "hook-1"})
        ),
        -32001
    );
    let (status, rest_config) = ann.rest(
        "GET",
        &format!("/tasks/{write_task}/pushNotificationConfigs/hook-1"),
        "",
    );
    assert_eq!((status, rest_config["url"].as_str()), (200, Some(hook.as_str())));
    // a second config whose receiver is gone: it fails and waits for its retry, and holds up no other config
    let dead_port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    ann.ok(
        "CreateTaskPushNotificationConfig",
        json!({"taskId": write_task, "id": "hook-dead", "url": format!("http://127.0.0.1:{dead_port}/hook")}),
    );
    // one user holds at most 16 streams and waits: subscribers of the task waiting for its owner stay open
    let subscribers: Vec<_> = (0..16)
        .map(|_| {
            let caller = Caller {
                address,
                key: ann.key.clone(),
            };
            let path = format!("/a2a/tasks/{write_task}:subscribe");
            std::thread::spawn(move || caller.stream("GET", &path, ""))
        })
        .collect();
    // (an accepted 17th would stream until the decision below, and the exchange would time out)
    std::thread::sleep(Duration::from_secs(2));
    let (refused, _) = ann.rest("POST", &format!("/tasks/{write_task}:subscribe"), "");
    assert_eq!(refused, 429, "the 17th stream of one user");
    assert_eq!(
        bob.rest("GET", &format!("/tasks/{bob_task}"), "").0,
        200,
        "another user is not held up"
    );
    let pushed = receiver.wait_for(&write_task, "TASK_STATE_AUTH_REQUIRED");
    let (push_headers, push_body) = pushed.last().unwrap();
    let header = |name: &str| {
        push_headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(header("x-a2a-notification-token"), Some("tok-1"));
    assert_eq!(header("authorization"), Some("Bearer push-secret"));
    assert_eq!(
        push_body
            .pointer("/statusUpdate/status/message/parts/1/data/approvalId")
            .and_then(Json::as_str),
        Some(approval_id.as_str()),
        "{push_body}"
    );
    // the owner decides in their browser session
    let (cookie, session) = browser(address, "ann.a2a");
    let (status, _, decided) = exchange(
        address,
        &http_request(
            "POST",
            "/assist/decide",
            &[
                ("Cookie", &cookie),
                ("Accept", "application/json"),
                ("Content-Type", "application/json"),
                ("X-CSRF-Token", &session),
            ],
            &json!({"approvalId": approval_id, "decision": "approve"}).to_string(),
        ),
    );
    assert_eq!(status, 200, "{decided}");
    let pushed = receiver.wait_for(&write_task, "TASK_STATE_COMPLETED");
    let pushed_states = states(
        &pushed.iter().map(|(_, body)| body.clone()).collect::<Vec<_>>(),
        "/statusUpdate/status/state",
    );
    assert!(
        pushed_states.ends_with(&["TASK_STATE_WORKING".to_string(), "TASK_STATE_COMPLETED".to_string()]),
        "{pushed_states:?}"
    );
    assert!(
        pushed
            .iter()
            .any(|(_, body)| body.pointer("/artifactUpdate/artifact/parts/0/text") == Some(&json!("Changed it."))),
        "{pushed:?}"
    );
    let done = ann.ok("GetTask", json!({"id": write_task}));
    assert_eq!(done["status"]["state"], "TASK_STATE_COMPLETED", "{done}");
    // each subscriber followed the task through its owner's decision to its end
    for subscriber in subscribers {
        let frames = subscriber.join().unwrap();
        assert_eq!(
            states(&frames, "/statusUpdate/status/state").last().map(String::as_str),
            Some("TASK_STATE_COMPLETED"),
            "{frames:?}"
        );
    }
    let dead = rows(&factory, "moqui.agent.AgentPushDelivery", "pushConfigId", "hook-dead");
    assert!(
        !dead.is_empty() && dead.iter().all(|row| field(row, "deliveredDate").is_empty()),
        "the dead receiver's notifications wait for their retries"
    );
    assert!(
        dead.iter().any(|row| !field(row, "lastError").is_empty()),
        "the dead receiver was tried"
    );
    assert_eq!(
        ann.ok(
            "DeleteTaskPushNotificationConfig",
            json!({"taskId": write_task, "id": "hook-1"})
        ),
        json!({})
    );
    assert_eq!(
        ann.code(
            "GetTaskPushNotificationConfig",
            json!({"taskId": write_task, "id": "hook-1"})
        ),
        -32001
    );

    // a caller has at most 50 push configs (with hook-dead, 49 more)
    let listed = ann.ok("ListTasks", json!({"pageSize": 100, "historyLength": 0}));
    let mut created = 0;
    let mut cap = None;
    'tasks: for task in listed["tasks"].as_array().unwrap() {
        for n in 0..10 {
            let (_, response) = ann.rpc(
                "CreateTaskPushNotificationConfig",
                json!({"taskId": task["id"], "url": format!("https://hooks.example.com/{n}")}),
            );
            if response.get("error").is_some() {
                let message = response["error"]["message"].as_str().unwrap_or_default().to_string();
                if message.contains("a caller has at most 50") {
                    cap = Some(message);
                    break 'tasks;
                }
                // a task's own 10
                continue 'tasks;
            }
            created += 1;
        }
    }
    assert_eq!((created, cap.is_some()), (49, true), "{cap:?}");
    // the push thread removes a config and its notifications after 7 days
    let (configs, _) = {
        let context = factory.new_context();
        let _entered = context.enter();
        let facade = factory.entity().unwrap();
        moqui_core::llm::a2a::push::clean(&facade, jiff::Timestamp::now().as_millisecond() + 8 * 86_400_000).unwrap()
    };
    assert!(configs >= 50, "{configs}");
    assert!(rows(&factory, "moqui.agent.AgentPushConfig", "userId", "A2A_ANN").is_empty());
    assert!(rows(&factory, "moqui.agent.AgentPushDelivery", "pushConfigId", "hook-dead").is_empty());

    // the official SDK client, over both bindings, when a python with a2a-sdk is named; CI without one skips it, loudly
    let python = std::env::var("A2A_SDK_PYTHON").unwrap_or_default();
    if python.is_empty() {
        eprintln!(
            "SKIPPED: the a2a-sdk interop check (tests/fixtures/a2a/interop_a2a_sdk.py) did not run: A2A_SDK_PYTHON \
             is not set; docs/A2A.md says how to make the venv"
        );
        return;
    }
    fake::script(
        ENDPOINT,
        (0..40)
            .map(|n| slow(&format!("Answer {n} from Moqui."), 1500))
            .collect(),
    );
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/a2a/interop_a2a_sdk.py");
    let output = std::process::Command::new(&python)
        .arg("-I")
        .arg(&script)
        .arg(format!("http://{address}"))
        .env("A2A_TOKEN", &ann.key)
        .output()
        .expect("the python of A2A_SDK_PYTHON runs");
    let printed = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{printed}");
    assert!(output.status.success(), "the a2a-sdk interop script failed:\n{printed}");
}

#[test]
fn the_a2a_server_is_off_by_default() {
    let gate = start_with_conf_properties("a2a-off", None, "", &[]);
    let address = gate.server.address();
    let (status, _, _) = exchange(address, &http_request("GET", "/.well-known/agent-card.json", &[], ""));
    assert_eq!(status, 404);
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "GetTask", "params": {"id": "x"}}).to_string();
    let (status, _, _) = exchange(
        address,
        &http_request(
            "POST",
            "/a2a",
            &[("A2A-Version", "1.0"), ("Authorization", "Bearer x")],
            &body,
        ),
    );
    assert_eq!(status, 404);
    let (status, _, _) = exchange(address, &http_request("GET", "/a2a/tasks", &[], ""));
    assert_eq!(status, 404);
}
