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

//! The OpenAI-compatible gateway (docs/Gateway.md; plans/LLM-A2A-Assist.md 4.17) over HTTP on a seeded server on
//! embedded Postgres, against a fake upstream on loopback that speaks Chat Completions (SSE and whole), Responses and
//! embeddings: the models list per client, the client row with the gateway scope (and store#LlmApiClient refusing a
//! second one, two refused as ambiguous), streaming with and without the usage chunk, tool definitions passed through
//! and tool calls back, a cut stream ending on an error frame with its usage estimated, whole replies with the output
//! limit clamped, Responses, embeddings, the usage rows with the client, the central key sent upstream and never the
//! caller's, the allowlist of body fields and `n`, auth (401, 403 without the permission or a client, a Bearer app
//! password), an unknown or disallowed alias (404), the user's token quota with the call's reserve and the requests per
//! minute (429 with Retry-After), a call the host has no room for taking no request of the minute, upstream errors
//! without their detail, a stream and a whole call the caller drops (the upstream request is dropped, the call logged
//! as canceled), and a caller that stops reading freeing the endpoint's slot. The LLM embedder is the search facade's
//! (`semantic embedder="llm"`) and the skill retrieval's semantic branch over the fake embeddings, keeps the batches
//! before a failed one, skips a server that does not answer, and refuses once its binding moves to another model. A
//! second server checks the gateway's own timeout (504 in the OpenAI shape, past the webapp request deadline) and that
//! an app password needs a client without `require-client`. With `OPENAI_PYTHON` naming a python that has the `openai`
//! package, `tests/fixtures/gateway/openai_client.py` runs the official client against the first server.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "llm"))]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::search::semantic::Embedder;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::common::{exchange, http_request, start_with_conf_properties};

const CENTRAL_KEY: &str = "sk-central-0123456789abcdefghij";

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="GW_ANN" username="ann.gw" userFullName="Ann Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_BOB" username="bob.gw" userFullName="Bob Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_CARL" username="carl.gw" userFullName="Carl Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_DAN" username="dan.gw" userFullName="Dan Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_EVE" username="eve.gw" userFullName="Eve Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_FAY" username="fay.gw" userFullName="Fay Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_GUS" username="gus.gw" userFullName="Gus Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_HAL" username="hal.gw" userFullName="Hal Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="GW_IVY" username="ivy.gw" userFullName="Ivy Gateway"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="GW_CALLERS" description="Gateway callers"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_ANN" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_CARL" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_DAN" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_EVE" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_FAY" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_GUS" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_HAL" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="GW_CALLERS" userId="GW_IVY" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="GW_CALLERS" userPermissionId="LlmGatewayUse" fromDate="1265184000000"/>
    <moqui.security.UserGroup userGroupId="GW_ADMINS" description="Gateway admins"/>
    <moqui.security.UserGroupMember userGroupId="GW_ADMINS" userId="GW_ANN" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="GW_ADMINS" userPermissionId="AgentAdmin" fromDate="1265184000000"/>
    <moqui.llm.LlmQuota quotaId="GW_DAN_RATE" userId="GW_DAN" scope="gateway" requestsPerMinute="2"/>
    <moqui.llm.LlmQuota quotaId="GW_EVE_TOKENS" userId="GW_EVE" scope="gateway" maxTokens="10"/>
    <moqui.llm.LlmQuota quotaId="GW_FAY_TOKENS" userId="GW_FAY" scope="gateway" maxTokens="1000"/>
    <moqui.llm.LlmQuota quotaId="GW_IVY_RATE" userId="GW_IVY" scope="gateway" requestsPerMinute="17"/>
    <moqui.llm.LlmModelAlias alias="row-fast" modelId="gw-chat" description="A row's alias"/>
    <moqui.llm.LlmModelAlias alias="secret" modelId="gw-chat" disabled="Y"/>
</entity-facade-xml>"#;

/// One request the fake upstream got: its path, headers (lower case) and JSON body.
type Seen = (String, Vec<(String, String)>, Json);

/// A loopback model server: Chat Completions, Responses and embeddings, as scripted by the request's last text.
struct Upstream {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// The scripts (`slow`, `flood`, `slow-whole`) whose caller went away: the gateway dropped the upstream request.
    dropped: Arc<Mutex<Vec<String>>>,
}

fn read_request(stream: &mut TcpStream) -> Option<Seen> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..count]);
        let text = String::from_utf8_lossy(&buffer).into_owned();
        let Some((head, rest)) = text.split_once("\r\n\r\n") else {
            continue;
        };
        let mut lines = head.lines();
        let path = lines.next()?.split_whitespace().nth(1)?.to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
            .collect();
        let length: usize = headers
            .iter()
            .find(|(name, _)| name == "content-length")
            .and_then(|(_, value)| value.parse().ok())
            .unwrap_or(0);
        if rest.len() >= length {
            return Some((path, headers, serde_json::from_str(&rest[..length]).unwrap_or(Json::Null)));
        }
    }
}

fn chunk(delta: &Json, finish: Option<&str>) -> String {
    format!(
        "data: {}\n\n",
        json!({"id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "upstream-chat",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
    )
}

/// Words of one meaning share a dimension: what a real embedding model does, for three meanings.
fn meaning_vector(text: &str) -> Vec<f64> {
    const GROUPS: [&[&str]; 3] = [
        &["password", "passphrase", "credential", "login", "forgot", "secret"],
        &["ship", "shipping", "shipment", "parcel", "order", "deliver"],
        &["job", "schedule", "cron", "timer", "nightly"],
    ];
    let mut vector = vec![0.0, 0.0, 0.0, 0.05];
    for word in text.split(|ch: char| !ch.is_alphanumeric()).map(str::to_lowercase) {
        for (at, group) in GROUPS.iter().enumerate() {
            if group.contains(&word.as_str()) {
                vector[at] += 1.0;
            }
        }
    }
    vector
}

fn answer(stream: &mut TcpStream, path: &str, body: &Json, dropped: &Mutex<Vec<String>>) {
    let text = body
        .get("messages")
        .and_then(Json::as_array)
        .and_then(|messages| messages.last())
        .and_then(|message| message.get("content"))
        .or_else(|| body.get("input"))
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let streaming = body.get("stream").and_then(Json::as_bool).unwrap_or(false);
    let sse_head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
    let gone = |mode: &str| dropped.lock().unwrap().push(mode.to_string());
    let json_reply = |stream: &mut TcpStream, status: &str, extra: &str, json: &Json| {
        let body = json.to_string();
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        );
    };
    if text == "upstream-429" {
        json_reply(
            stream,
            "429 Too Many Requests",
            "Retry-After: 7\r\n",
            &json!({"error": {"message": "Rate limit reached for requests at http://127.0.0.1 internal", "type": "requests", "code": "rate_limit_exceeded"}}),
        );
        return;
    }
    if text == "slow-whole" {
        // answers after 20 s unless the gateway drops the request first
        let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
        for _ in 0..200 {
            let mut probe = [0_u8; 1];
            if matches!(stream.read(&mut probe), Ok(0)) {
                gone("slow-whole");
                return;
            }
        }
    }
    match path {
        "/v1/chat/completions" if streaming => {
            let mut out = String::from(sse_head);
            out.push_str(&chunk(&json!({"role": "assistant", "content": ""}), None));
            match text.as_str() {
                "slow" => {
                    if stream.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                    for n in 0..200 {
                        std::thread::sleep(Duration::from_millis(100));
                        if stream.write_all(chunk(&json!({"content": format!(" {n}")}), None).as_bytes()).is_err() {
                            gone("slow");
                            return;
                        }
                    }
                    return;
                }
                "flood" => {
                    // as fast as the gateway takes it, until it stops reading
                    let big = chunk(&json!({"content": "x".repeat(60_000)}), None);
                    if stream.write_all(out.as_bytes()).is_err() {
                        return;
                    }
                    for _ in 0..20_000 {
                        if stream.write_all(big.as_bytes()).is_err() {
                            gone("flood");
                            return;
                        }
                    }
                    return;
                }
                "cut" => {
                    // two pieces, then the connection ends with no finish and no [DONE]
                    out.push_str(&chunk(&json!({"content": "Half"}), None));
                    out.push_str(&chunk(&json!({"content": " an answer"}), None));
                    let _ = stream.write_all(out.as_bytes());
                    return;
                }
                "use-tool" => {
                    out.push_str(&chunk(
                        &json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                            "function": {"name": "lookup", "arguments": "{\"q\":"}}]}),
                        None,
                    ));
                    out.push_str(&chunk(&json!({"tool_calls": [{"index": 0, "function": {"arguments": "1}"}}]}), None));
                    out.push_str(&chunk(&json!({}), Some("tool_calls")));
                }
                _ => {
                    out.push_str(&chunk(&json!({"content": "Hello"}), None));
                    out.push_str(&chunk(&json!({"content": " there"}), None));
                    out.push_str(&chunk(&json!({}), Some("stop")));
                }
            }
            if body.pointer("/stream_options/include_usage") == Some(&Json::Bool(true)) {
                out.push_str(&format!(
                    "data: {}\n\n",
                    json!({"id": "c1", "object": "chat.completion.chunk", "created": 1, "model": "upstream-chat",
                        "choices": [], "usage": {"prompt_tokens": 11, "completion_tokens": 4, "total_tokens": 15}})
                ));
            }
            out.push_str("data: [DONE]\n\n");
            let _ = stream.write_all(out.as_bytes());
        }
        "/v1/chat/completions" => json_reply(
            stream,
            "200 OK",
            "",
            &json!({"id": "c2", "object": "chat.completion", "created": 1, "model": "upstream-chat",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello whole"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12}}),
        ),
        "/v1/responses" => {
            let completed = json!({"id": "r1", "object": "response", "created_at": 1, "status": "completed",
                "model": "upstream-resp", "output": [{"id": "m1", "type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "Hi there", "annotations": []}]}],
                "usage": {"input_tokens": 5, "output_tokens": 2, "total_tokens": 7}});
            if streaming {
                let mut out = String::from(sse_head);
                for (event, data) in [
                    ("response.created", json!({"type": "response.created", "sequence_number": 0,
                        "response": {"id": "r1", "object": "response", "created_at": 1, "status": "in_progress", "model": "upstream-resp", "output": []}})),
                    ("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 1,
                        "item_id": "m1", "output_index": 0, "content_index": 0, "delta": "Hi"})),
                    ("response.output_text.delta", json!({"type": "response.output_text.delta", "sequence_number": 2,
                        "item_id": "m1", "output_index": 0, "content_index": 0, "delta": " there"})),
                    ("response.completed", json!({"type": "response.completed", "sequence_number": 3, "response": completed})),
                ] {
                    out.push_str(&format!("event: {event}\ndata: {data}\n\n"));
                }
                let _ = stream.write_all(out.as_bytes());
            } else {
                json_reply(stream, "200 OK", "", &completed);
            }
        }
        "/v1/embeddings" => {
            let inputs: Vec<String> = match body.get("input") {
                Some(Json::Array(items)) => items.iter().map(|item| item.as_str().unwrap_or_default().to_string()).collect(),
                Some(Json::String(one)) => vec![one.clone()],
                _ => Vec::new(),
            };
            if inputs.iter().any(|input| input.contains("boom")) {
                json_reply(stream, "400 Bad Request", "", &json!({"error": {"message": "this input cannot be embedded"}}));
                return;
            }
            let data: Vec<Json> = inputs
                .iter()
                .enumerate()
                .map(|(index, text)| json!({"object": "embedding", "index": index, "embedding": meaning_vector(text)}))
                .rev()
                .collect();
            let tokens: usize = inputs.iter().map(|text| text.split_whitespace().count()).sum();
            json_reply(
                stream,
                "200 OK",
                "",
                &json!({"object": "list", "data": data, "model": "upstream-embed",
                    "usage": {"prompt_tokens": tokens, "total_tokens": tokens}}),
            );
        }
        _ => json_reply(stream, "404 Not Found", "", &json!({"error": {"message": "no such path"}})),
    }
}

impl Upstream {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let dropped = Arc::new(Mutex::new(Vec::new()));
        let (kept, flag) = (Arc::clone(&seen), Arc::clone(&dropped));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (kept, flag) = (Arc::clone(&kept), Arc::clone(&flag));
                std::thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                    let Some((path, headers, body)) = read_request(&mut stream) else {
                        return;
                    };
                    kept.lock().unwrap().push((path.clone(), headers, body.clone()));
                    answer(&mut stream, &path, &body, &flag);
                });
            }
        });
        Self { port, seen, dropped }
    }

    fn last(&self) -> Seen {
        self.seen.lock().unwrap().last().cloned().expect("a request reached the upstream")
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn dropped(&self, mode: &str) -> bool {
        self.dropped.lock().unwrap().iter().any(|known| known == mode)
    }
}

/// A server that takes connections and never answers.
fn hanging() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming().flatten() {
            held.push(stream);
        }
    });
    port
}

fn conf(port: u16, hang: u16) -> String {
    format!(
        r#"<llm-facade>
    <endpoint id="gw-up" protocol="chat-completions" url="http://127.0.0.1:{port}" api-key="{CENTRAL_KEY}"/>
    <endpoint id="gw-resp" protocol="responses" url="http://127.0.0.1:{port}" api-key="{CENTRAL_KEY}"/>
    <endpoint id="gw-tight" protocol="chat-completions" url="http://127.0.0.1:{port}" max-concurrent="1" idle-timeout="2"/>
    <endpoint id="gw-down" protocol="chat-completions" url="http://127.0.0.1:1"/>
    <endpoint id="gw-hang" protocol="chat-completions" url="http://127.0.0.1:{hang}"/>
    <model id="gw-chat" endpoint="gw-up" name="upstream-chat" price-input="1" price-output="2" max-output="2000"/>
    <model id="gw-chat2" endpoint="gw-up" name="upstream-chat"/>
    <model id="gw-tight" endpoint="gw-tight" name="upstream-chat"/>
    <model id="gw-down" endpoint="gw-down" name="upstream-chat"/>
    <model id="gw-embed" endpoint="gw-up" name="upstream-embed"/>
    <model id="gw-embed2" endpoint="gw-up" name="upstream-embed-2"/>
    <model id="gw-embed-hang" endpoint="gw-hang" name="upstream-embed"/>
    <model id="gw-r" endpoint="gw-resp" name="upstream-resp"/>
    <bind feature="embed" model="gw-embed" reasoning="none"/>
    <gateway enabled="true" require-client="true" timeout="60">
        <alias name="fast" model="gw-chat"/>
        <alias name="nolimit" model="gw-chat2"/>
        <alias name="tight" model="gw-tight"/>
        <alias name="down" model="gw-down"/>
        <alias name="resp" model="gw-r"/>
        <alias name="embed" feature="embed"/>
        <alias name="other" model="gw-chat"/>
        <alias name="secret" model="gw-chat"/>
    </gateway>
</llm-facade>
<search-facade><semantic enabled="true" dimensions="4" embedder="llm"/></search-facade>"#
    )
}

/// A login key of `username`, and with `client`, an `LlmApiClient` row for it with the gateway scope.
fn key_of(factory: &Arc<ExecutionContextFactory>, username: &str, client: Option<(&str, &str)>) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username).unwrap();
    let key = moqui_core::user::get_login_key(None).unwrap();
    if let Some((client_id, aliases)) = client {
        let user_id = moqui_core::user::current_user_id().unwrap().unwrap();
        let _authz = context.disable_authz();
        create(
            factory,
            "moqui.llm.LlmApiClient",
            &[
                ("clientId", client_id),
                ("userId", &user_id),
                ("loginKey", &moqui_core::user::hash_login_key(&key).unwrap()),
                ("scopes", "gateway"),
                ("allowedAliases", aliases),
            ],
        );
    }
    key
}

fn create(factory: &Arc<ExecutionContextFactory>, entity: &str, fields: &[(&str, &str)]) {
    let facade = factory.entity().unwrap();
    let mut row = facade.make_value(entity).unwrap();
    for (field, text) in fields {
        if !text.is_empty() {
            row.set(*field, Value::string(*text));
        }
    }
    row.create(&facade).unwrap();
}

/// The `LlmCall` rows of `user_id`, oldest first.
fn calls(factory: &Arc<ExecutionContextFactory>, user_id: &str) -> Vec<moqui_core::entity::EntityValue> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find("moqui.llm.LlmCall")
        .unwrap()
        .eq("userId", Value::string(user_id))
        .order_by("startDate", false)
        .order_by("callId", false)
        .list()
        .unwrap()
        .values()
        .to_vec()
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(moqui_lib::object::to_plain_string).unwrap_or_default()
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// One call with `authorization`: its status, headers and body as JSON.
fn send(address: SocketAddr, authorization: &str, method: &str, path: &str, body: &Json) -> (u16, Vec<(String, String)>, Json) {
    let text = if body.is_null() { String::new() } else { body.to_string() };
    let mut headers = vec![("Content-Type", "application/json")];
    if !authorization.is_empty() {
        headers.push(("Authorization", authorization));
    }
    let (status, headers, response) = exchange(address, &http_request(method, &format!("/llm/v1{path}"), &headers, &text));
    (status, headers, serde_json::from_str(&response).unwrap_or_else(|_| json!({"unparsed": response})))
}

/// The `data:` payloads (and `event:` names) of a stream, read to its end (HTTP/1.0, so the body is not chunked).
fn stream(address: SocketAddr, key: &str, path: &str, body: &Json) -> (String, Vec<(Option<String>, String)>) {
    let body = body.to_string();
    let request = format!(
        "POST /llm/v1{path} HTTP/1.0\r\nHost: localhost\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut connection = TcpStream::connect(address).unwrap();
    connection.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    connection.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    let _ = connection.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let frames = body
        .split("\n\n")
        .filter(|frame| !frame.trim().is_empty())
        .map(|frame| {
            let event = frame.lines().find_map(|line| line.strip_prefix("event: ")).map(str::to_string);
            let data = frame.lines().filter_map(|line| line.strip_prefix("data: ")).collect::<Vec<_>>().join("\n");
            (event, data)
        })
        .collect();
    (head.to_string(), frames)
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A stream request on a connection of its own, its head read, the connection handed back unread beyond that.
fn open_stream(address: SocketAddr, key: &str, body: &Json) -> TcpStream {
    let body = body.to_string();
    let mut connection = TcpStream::connect(address).unwrap();
    connection.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    connection
        .write_all(
            format!(
                "POST /llm/v1/chat/completions HTTP/1.0\r\nHost: localhost\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    connection
}

/// `store#LlmApiClient` as `username` (an AgentAdmin): the client id it made, or None when it was refused.
fn store_client(factory: &Arc<ExecutionContextFactory>, username: &str, fields: &[(&str, &str)]) -> Option<String> {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username).unwrap();
    // the service's own AgentAdmin check decides; its artifact authorization is the screens'
    let _authz = context.disable_authz();
    let parameters: std::collections::BTreeMap<String, Value> = fields
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    let out = match moqui_core::service::sync("org.moqui.agent.AgentAdminServices.store#LlmApiClient", &parameters) {
        Ok(out) => out,
        Err(error) => {
            eprintln!("store#LlmApiClient refused: {error}");
            return None;
        }
    };
    out.get("clientId").map(moqui_lib::object::to_plain_string).filter(|id| !id.is_empty())
}

fn embed_rows(factory: &Arc<ExecutionContextFactory>) -> usize {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory.entity().unwrap().find("moqui.llm.LlmCall").unwrap().eq("source", Value::string("embed")).list().unwrap().len()
}

#[test]
fn the_gateway_passes_calls_through_with_keys_quotas_and_usage_on_postgres() {
    let upstream = Upstream::start();
    let gate = start_with_conf_properties("gateway", Some(DATA), &conf(upstream.port, hanging()), &[]);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let ann = key_of(&factory, "ann.gw", Some(("GW_CL_ANN", "fast,resp,embed,row-fast,secret,tight,down")));
    let bob = key_of(&factory, "bob.gw", Some(("GW_CL_BOB", "")));
    let carl = key_of(&factory, "carl.gw", None);
    let dan = key_of(&factory, "dan.gw", Some(("GW_CL_DAN", "")));
    let eve = key_of(&factory, "eve.gw", Some(("GW_CL_EVE", "")));
    let fay = key_of(&factory, "fay.gw", Some(("GW_CL_FAY", "")));
    let ivy = key_of(&factory, "ivy.gw", Some(("GW_CL_IVY", "")));
    let bearer = |key: &str| format!("Bearer {key}");
    let hello = json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}]});

    // auth: none and a bad key are 401 with a challenge; no LlmGatewayUse, or no client while one is required, 403
    let (status, headers, body) = send(address, "", "GET", "/models", &Json::Null);
    assert_eq!(status, 401, "{body}");
    assert!(header(&headers, "www-authenticate").is_some_and(|value| value.starts_with("Bearer")));
    assert_eq!(body["error"]["code"], "invalid_api_key");
    let (status, _, _) = send(address, "Bearer 0123456789012345678901234567890123456789", "GET", "/models", &Json::Null);
    assert_eq!(status, 401);
    let (status, _, body) = send(address, &bearer(&bob), "GET", "/models", &Json::Null);
    assert_eq!((status, body["error"]["code"].as_str()), (403, Some("permission_denied")), "{body}");
    let (status, _, body) = send(address, &bearer(&carl), "POST", "/chat/completions", &hello);
    assert_eq!((status, body["error"]["code"].as_str()), (403, Some("client_required")), "{body}");

    // clients: the gateway takes the row of the key with its scope, store#LlmApiClient refuses a second one, and two
    // rows with it (made around the service) are refused as ambiguous
    let gus = key_of(&factory, "gus.gw", None);
    let gus_hash = moqui_core::user::hash_login_key(&gus).unwrap();
    assert_eq!(
        store_client(&factory, "ann.gw", &[("clientId", "GW_CL_GUS_A2A"), ("userId", "GW_GUS"), ("loginKey", &gus), ("scopes", "a2a")]).as_deref(),
        Some("GW_CL_GUS_A2A")
    );
    let (status, _, body) = send(address, &bearer(&gus), "POST", "/chat/completions", &hello);
    assert_eq!((status, body["error"]["code"].as_str()), (403, Some("client_not_allowed")), "an a2a client only: {body}");
    assert_eq!(
        store_client(&factory, "ann.gw", &[("clientId", "GW_CL_GUS"), ("userId", "GW_GUS"), ("loginKey", &gus), ("scopes", "gateway")]).as_deref(),
        Some("GW_CL_GUS")
    );
    assert_eq!(
        store_client(&factory, "ann.gw", &[("clientId", "GW_CL_GUS2"), ("userId", "GW_GUS"), ("loginKey", &gus), ("scopes", "gateway,mcp")]),
        None,
        "a second gateway client of one key is refused"
    );
    assert_eq!(
        store_client(&factory, "ann.gw", &[("clientId", "GW_CL_X"), ("userId", "GW_HAL"), ("loginKey", &gus), ("scopes", "gateway")]),
        None,
        "another user's key is refused"
    );
    assert_eq!(store_client(&factory, "dan.gw", &[("userId", "GW_GUS"), ("loginKey", &gus), ("scopes", "a2a")]), None, "AgentAdmin only");
    let (status, _, _) = send(address, &bearer(&gus), "POST", "/chat/completions", &hello);
    assert_eq!(status, 200);
    assert_eq!(field(calls(&factory, "GW_GUS").last().unwrap(), "clientId"), "GW_CL_GUS");
    let hal = key_of(&factory, "hal.gw", None);
    let hal_hash = moqui_core::user::hash_login_key(&hal).unwrap();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        for id in ["GW_CL_HAL1", "GW_CL_HAL2"] {
            create(&factory, "moqui.llm.LlmApiClient", &[("clientId", id), ("userId", "GW_HAL"), ("loginKey", &hal_hash), ("scopes", "gateway")]);
        }
    }
    let (status, _, body) = send(address, &bearer(&hal), "POST", "/chat/completions", &hello);
    assert_eq!(status, 403, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("more than one"), "{body}");
    let _ = gus_hash;

    // the models: the conf's and the rows' aliases the client may use; a disabled row turns a conf alias off
    let (status, _, models) = send(address, &bearer(&ann), "GET", "/models", &Json::Null);
    assert_eq!(status, 200, "{models}");
    let ids: Vec<&str> = models["data"].as_array().unwrap().iter().filter_map(|model| model["id"].as_str()).collect();
    assert_eq!(ids, ["down", "embed", "fast", "resp", "row-fast", "tight"]);
    assert_eq!(send(address, &bearer(&ann), "GET", "/models/fast", &Json::Null).2["object"], "model");
    assert_eq!(send(address, &bearer(&dan), "GET", "/models", &Json::Null).2["data"].as_array().unwrap().len(), 8);

    // an unknown alias, one the client may not use and one a row turned off: 404 model_not_found, nothing sent on
    let before = upstream.count();
    for alias in ["nope", "other", "secret"] {
        let (status, _, body) = send(
            address,
            &bearer(&ann),
            "POST",
            "/chat/completions",
            &json!({"model": alias, "messages": [{"role": "user", "content": "hi"}]}),
        );
        assert_eq!((status, body["error"]["code"].as_str()), (404, Some("model_not_found")), "{alias}: {body}");
    }
    // the body allowlist: a field the gateway does not pass, a hosted tool, more than one choice, the wrong API
    for (body, param) in [
        (json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}], "base_url": "http://evil"}), "base_url"),
        (json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}], "tools": [{"type": "web_search"}]}), "tools"),
        (json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}], "extra_headers": {"x": "y"}}), "extra_headers"),
        (json!({"model": "fast", "messages": [{"role": "user", "content": "hi"}], "n": 4}), "n"),
    ] {
        let (status, _, refused) = send(address, &bearer(&ann), "POST", "/chat/completions", &body);
        assert_eq!((status, refused["error"]["param"].as_str()), (400, Some(param)), "{refused}");
    }
    let (status, _, refused) = send(address, &bearer(&ann), "POST", "/chat/completions", &json!({"model": "resp", "messages": [{"role": "user", "content": "x"}]}));
    assert_eq!((status, refused["error"]["code"].as_str()), (400, Some("unsupported_model")), "{refused}");
    assert_eq!(upstream.count(), before, "no refused request reached the upstream");

    // a whole reply: the provider's model name and the central key go upstream, never the caller's key; the output
    // limit is clamped to the model's max-output
    let (status, _, whole) = send(address, &bearer(&ann), "POST", "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "hi"}], "temperature": 0.2, "max_tokens": 100000}));
    assert_eq!(status, 200, "{whole}");
    assert_eq!(whole["choices"][0]["message"]["content"], "Hello whole");
    let (path, headers, sent) = upstream.last();
    assert_eq!(path, "/v1/chat/completions");
    assert_eq!(header(&headers, "authorization"), Some(format!("Bearer {CENTRAL_KEY}").as_str()));
    assert!(!headers.iter().any(|(_, value)| value.contains(&ann)), "the caller's key went upstream");
    assert_eq!((sent["model"].as_str(), sent["stream"].as_bool(), sent["temperature"].as_f64()), (Some("upstream-chat"), Some(false), Some(0.2)));
    assert_eq!(sent["max_tokens"], 2000);
    let rows = calls(&factory, "GW_ANN");
    let last = rows.last().unwrap();
    assert_eq!(
        [field(last, "source"), field(last, "clientId"), field(last, "modelId"), field(last, "inputTokens"), field(last, "outputTokens"), field(last, "httpStatus")],
        ["gateway", "GW_CL_ANN", "gw-chat", "9", "3", "200"]
    );
    assert_eq!(field(last, "usageEstimated"), "");
    // 9 in at 1 and 3 out at 2 per million, at the column's precision
    let cost: f64 = field(last, "cost").parse().unwrap();
    assert!((cost - 0.000015).abs() < 0.00001, "{cost}");

    // streaming with the usage chunk the caller asked for, then without it: include_usage is forced upstream either way
    let (head, frames) = stream(address, &ann, "/chat/completions", &json!({
        "model": "row-fast", "messages": [{"role": "user", "content": "hi"}], "stream": true,
        "stream_options": {"include_usage": true}}));
    assert!(head.contains("text/event-stream"), "{head}");
    assert_eq!(frames.last().unwrap().1, "[DONE]");
    let text: String = frames
        .iter()
        .filter_map(|(_, data)| serde_json::from_str::<Json>(data).ok())
        .filter_map(|chunk| chunk.pointer("/choices/0/delta/content").and_then(Json::as_str).map(str::to_string))
        .collect();
    assert_eq!(text, "Hello there");
    assert!(frames.iter().any(|(_, data)| data.contains("\"usage\":{")), "{frames:?}");
    assert_eq!(upstream.last().2["stream_options"], json!({"include_usage": true}));
    let (_, frames) = stream(address, &ann, "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "hi"}], "stream": true}));
    assert!(!frames.iter().any(|(_, data)| data.contains("\"usage\":{")), "the usage chunk was not asked for: {frames:?}");
    assert_eq!(upstream.last().2["stream_options"], json!({"include_usage": true}));
    let rows = calls(&factory, "GW_ANN");
    let last = rows.last().unwrap();
    assert_eq!([field(last, "inputTokens"), field(last, "outputTokens"), field(last, "finishReason")], ["11", "4", "stop"]);
    assert!(!field(last, "firstTokenMs").is_empty());

    // a stream the model server cuts ends with an error frame, not [DONE]; its usage is estimated
    let (_, frames) = stream(address, &ann, "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "cut"}], "stream": true}));
    let last_frame: Json = serde_json::from_str(&frames.last().unwrap().1).unwrap_or(Json::Null);
    assert!(last_frame["error"]["message"].is_string(), "{frames:?}");
    assert!(!frames.iter().any(|(_, data)| data == "[DONE]"), "{frames:?}");
    let last = calls(&factory, "GW_ANN");
    let last = last.last().unwrap();
    assert_eq!((field(last, "errorClass").as_str(), field(last, "usageEstimated").as_str()), ("server", "Y"));

    // tool definitions pass through to the model and its tool calls back; no tool runs here
    let tools = json!([{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object", "properties": {"q": {"type": "integer"}}}}}]);
    let (_, frames) = stream(address, &ann, "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "use-tool"}], "stream": true, "tools": tools}));
    assert_eq!(upstream.last().2["tools"], tools);
    let arguments: String = frames
        .iter()
        .filter_map(|(_, data)| serde_json::from_str::<Json>(data).ok())
        .filter_map(|chunk| chunk.pointer("/choices/0/delta/tool_calls/0/function/arguments").and_then(Json::as_str).map(str::to_string))
        .collect();
    assert_eq!(arguments, "{\"q\":1}");
    assert_eq!(field(calls(&factory, "GW_ANN").last().unwrap(), "finishReason"), "tool_calls");

    // Responses, whole and streamed (its event names kept), sent with store false
    let (status, _, response) = send(address, &bearer(&ann), "POST", "/responses", &json!({"model": "resp", "input": "hi"}));
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["output"][0]["content"][0]["text"], "Hi there");
    assert_eq!((upstream.last().0.as_str(), upstream.last().2["store"].as_bool()), ("/v1/responses", Some(false)));
    let (_, frames) = stream(address, &ann, "/responses", &json!({"model": "resp", "input": "hi", "stream": true}));
    assert_eq!(frames.first().unwrap().0.as_deref(), Some("response.created"));
    assert_eq!(frames.last().unwrap().0.as_deref(), Some("response.completed"));
    let last = calls(&factory, "GW_ANN");
    assert_eq!([field(last.last().unwrap(), "inputTokens"), field(last.last().unwrap(), "outputTokens")], ["5", "2"]);

    // embeddings through an alias of the embed feature
    let (status, _, embedded) = send(address, &bearer(&ann), "POST", "/embeddings", &json!({"model": "embed", "input": ["forgot password", "ship order"]}));
    assert_eq!(status, 200, "{embedded}");
    assert_eq!(embedded["data"].as_array().unwrap().len(), 2);
    assert_eq!((upstream.last().0.as_str(), upstream.last().2["model"].as_str()), ("/v1/embeddings", Some("upstream-embed")));
    assert_eq!(field(calls(&factory, "GW_ANN").last().unwrap(), "inputTokens"), "4");

    // the model server's errors reach the caller without its detail: an upstream 429 with its Retry-After, an endpoint
    // that cannot be reached 502; the detail is in LlmCall
    let (status, headers, body) = send(address, &bearer(&ann), "POST", "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "upstream-429"}]}));
    assert_eq!((status, header(&headers, "retry-after")), (429, Some("7")), "{body}");
    assert!(!body.to_string().contains("127.0.0.1"), "{body}");
    let (status, _, body) = send(address, &bearer(&ann), "POST", "/chat/completions", &json!({
        "model": "down", "messages": [{"role": "user", "content": "hi"}]}));
    assert_eq!(status, 502, "{body}");
    assert!(!body.to_string().contains("127.0.0.1"), "{body}");
    assert!(field(calls(&factory, "GW_ANN").last().unwrap(), "errorText").contains("127.0.0.1"));

    // a Bearer app password whose LlmApiClient names it
    let app_password = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("ann.gw").unwrap();
        let _authz = context.disable_authz();
        let parameters: std::collections::BTreeMap<String, Value> =
            [("name".to_string(), Value::string("Gateway test"))].into_iter().collect();
        let out = moqui_core::service::sync("org.moqui.impl.UserServices.create#UserAppPassword", &parameters).unwrap();
        let id = moqui_lib::object::to_plain_string(out.get("appPasswordId").unwrap());
        create(&factory, "moqui.llm.LlmApiClient", &[("clientId", "GW_CL_ANN_APP"), ("userId", "GW_ANN"), ("appPasswordId", &id), ("scopes", "gateway")]);
        moqui_lib::object::to_plain_string(out.get("password").unwrap())
    };
    let (status, _, whole) = send(address, &bearer(&app_password), "POST", "/chat/completions", &hello);
    assert_eq!(status, 200, "{whole}");
    assert_eq!(field(calls(&factory, "GW_ANN").last().unwrap(), "clientId"), "GW_CL_ANN_APP");
    let (status, _, _) = send(address, &bearer("abcd-efgh-jkmn-pqrs-tuvw"), "GET", "/models", &Json::Null);
    assert_eq!(status, 401);

    // the user's token quota for the gateway: the first call uses it up, the next is 429 until the day ends
    let small = json!({"model": "nolimit", "messages": [{"role": "user", "content": "hi"}]});
    let (status, _, body) = send(address, &bearer(&eve), "POST", "/chat/completions", &small);
    assert_eq!(status, 200, "{body}");
    let (status, headers, body) = send(address, &bearer(&eve), "POST", "/chat/completions", &small);
    assert_eq!((status, body["error"]["code"].as_str()), (429, Some("insufficient_quota")), "{body}");
    let wait: u64 = header(&headers, "retry-after").unwrap().parse().unwrap();
    assert!((1..=86_400 + 3600).contains(&wait), "{wait}");
    // the most a call may use counts before it is made: its output limit over what is left is refused
    let (status, _, body) = send(address, &bearer(&fay), "POST", "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "hi"}], "max_tokens": 5000}));
    assert_eq!((status, body["error"]["code"].as_str()), (429, Some("insufficient_quota")), "the clamped 2000 is over 1000: {body}");
    let (status, _, body) = send(address, &bearer(&fay), "POST", "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "hi"}], "max_tokens": 100}));
    assert_eq!(status, 200, "{body}");
    assert_eq!(upstream.last().2["max_tokens"], 100);

    // requests per minute: two pass, the third is 429 with the seconds until the oldest ages out
    for _ in 0..2 {
        assert_eq!(send(address, &bearer(&dan), "POST", "/chat/completions", &hello).0, 200);
    }
    let (status, headers, body) = send(address, &bearer(&dan), "POST", "/chat/completions", &hello);
    assert_eq!((status, body["error"]["code"].as_str()), (429, Some("rate_limit_exceeded")), "{body}");
    let wait: u64 = header(&headers, "retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&wait), "{wait}");
    assert_eq!(calls(&factory, "GW_DAN").len(), 2);

    // a call the host has no room for (16 open of one user) uses up no request of the minute: ivy (17 a minute) holds
    // 16 streams, is refused one more, and once a stream ends her 17th request still passes
    let slow = json!({"model": "fast", "messages": [{"role": "user", "content": "slow"}], "stream": true});
    let mut held: Vec<TcpStream> = (0..16).map(|_| open_stream(address, &ivy, &slow)).collect();
    for connection in &mut held {
        let mut first = [0_u8; 64];
        assert!(connection.read(&mut first).unwrap() > 0);
    }
    for _ in 0..3 {
        let (status, _, body) = send(address, &bearer(&ivy), "POST", "/chat/completions", &hello);
        assert_eq!((status, body["error"]["code"].as_str()), (429, Some("too_many_open_calls")), "{body}");
    }
    drop(held.pop());
    wait_until("one of ivy's streams to end", || {
        let (status, _, body) = send(address, &bearer(&ivy), "POST", "/chat/completions", &hello);
        assert_ne!(body["error"]["code"], "rate_limit_exceeded", "a refused call took a request of the minute");
        status == 200
    });
    drop(held);

    // a caller that drops a stream: the gateway drops the upstream request and logs the call as canceled, with its
    // usage estimated from the prompt (a large one) since no usage came
    let big = "word ".repeat(20_000);
    let mut connection = open_stream(address, &ann, &json!({"model": "fast", "stream": true,
        "messages": [{"role": "system", "content": big}, {"role": "user", "content": "slow"}]}));
    let mut first = [0_u8; 512];
    let read = connection.read(&mut first).unwrap();
    assert!(String::from_utf8_lossy(&first[..read]).contains("200"), "{}", String::from_utf8_lossy(&first[..read]));
    drop(connection);
    wait_until("the upstream to see its request dropped", || upstream.dropped("slow"));
    wait_until("the canceled call's row", || {
        calls(&factory, "GW_ANN").last().is_some_and(|row| field(row, "finishReason") == "canceled")
    });
    let canceled = calls(&factory, "GW_ANN");
    let canceled = canceled.last().unwrap();
    let input: u64 = field(canceled, "inputTokens").parse().unwrap();
    assert!(input > 10_000 && field(canceled, "usageEstimated") == "Y", "{input}");

    // a whole call the caller leaves is canceled too: the gateway stops waiting and logs it (the blocking upstream
    // request itself ends when the model server answers or the call's timeout passes)
    let canceled_rows = |factory: &Arc<ExecutionContextFactory>| {
        calls(factory, "GW_ANN").iter().filter(|row| field(row, "finishReason") == "canceled").count()
    };
    let before = canceled_rows(&factory);
    let body = json!({"model": "fast", "messages": [{"role": "user", "content": "slow-whole"}]}).to_string();
    let mut connection = TcpStream::connect(address).unwrap();
    connection
        .write_all(
            format!(
                "POST /llm/v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {ann}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    drop(connection);
    wait_until("the whole call to be canceled", || canceled_rows(&factory) > before);

    // a caller that stops reading frees the endpoint's one slot within its idle limit (2 s): the next call gets it
    let mut stalled = open_stream(address, &ann, &json!({"model": "tight", "stream": true,
        "messages": [{"role": "user", "content": "flood"}]}));
    let mut first = [0_u8; 64];
    assert!(stalled.read(&mut first).unwrap() > 0);
    let started = Instant::now();
    let (status, _, body) = send(address, &bearer(&ann), "POST", "/chat/completions", &json!({
        "model": "tight", "messages": [{"role": "user", "content": "hi"}]}));
    assert_eq!(status, 200, "{body}");
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
    wait_until("the flood to be dropped", || upstream.dropped("flood"));
    drop(stalled);

    // the LLM embedder: the search facade's (semantic embedder="llm"), and the skill retrieval's semantic branch
    let embedder = moqui_core::search::installed(&factory).and_then(|search| search.embedder()).expect("the search facade's embedder");
    assert_eq!(embedder.model(), "llm:gw-embed/upstream-embed");
    let skills = moqui_core::llm::skills::SkillIndex {
        skills: [
            "---\nname: reset-user-credential\ntitle: Resetting a credential\ndescription: Use when someone needs a new password for their login.\nkind: procedure\n---\nSteps.\n",
            "---\nname: ship-sales-order\ntitle: Shipping a sales order\ndescription: Use when an order is ready to go out as a parcel.\nkind: procedure\n---\nSteps.\n",
            "---\nname: schedule-service-job\ntitle: Scheduling a service job\ndescription: Use when something must run nightly on a cron.\nkind: procedure\n---\nSteps.\n",
        ]
        .iter()
        .map(|text| moqui_core::llm::skills::parse_skill(text).unwrap())
        .collect(),
        aliases: std::collections::BTreeMap::new(),
    };
    let before = embed_rows(&factory);
    let found = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::llm::skills::retrieval::rank_for(&factory, &skills, "I forgot my passphrase", None)
    };
    assert_eq!(found[0].name, "reset-user-credential", "{found:?}");
    assert!(found[0].lexical == 0 && found[0].semantic.is_some() && !found[0].strong, "{:?}", found[0]);
    assert!(embed_rows(&factory) > before, "the embeddings calls are logged with source embed");
    // without the search facade's embedder, the embed feature's binding is the semantic branch
    let bound = moqui_core::llm::embed::bound_embedder(&factory).expect("embed is bound");
    let found = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::llm::skills::retrieval::rank_hybrid(&skills, "a parcel to deliver", None, Some(bound.as_ref() as &dyn Embedder), None)
    };
    assert_eq!(found[0].name, "ship-sales-order", "{found:?}");

    // a batch that fails keeps the batches before it: 65 texts, the last one refused, then the first 64 again are not
    // sent; a refused input opens no breaker, and the failed batch is retried a text at a time
    let texts: Vec<String> = (0..65).map(|n| if n == 64 { "boom".to_string() } else { format!("text {n}") }).collect();
    let sent = upstream.count();
    assert!(bound.embed(&texts).is_err());
    assert_eq!(upstream.count(), sent + 2);
    assert_eq!(bound.embed(&texts[..64]).unwrap().len(), 64);
    assert_eq!(upstream.count(), sent + 2, "the first batch was kept");
    assert!(bound.retries_each(&moqui_lib::error::Error::new("x")));

    // a server that does not answer costs one short wait, then its breaker skips it: skills rank by words, at once
    let hung = moqui_core::llm::embed::LlmEmbedder::for_factory(&factory, moqui_core::llm::embed::EmbedWith::Model("gw-embed-hang".to_string()), None).unwrap();
    let rank = |query: &str| {
        let context = factory.new_context();
        let _entered = context.enter();
        let started = Instant::now();
        let found = moqui_core::llm::skills::retrieval::rank_hybrid(&skills, query, None, Some(&hung as &dyn Embedder), None);
        (found, started.elapsed())
    };
    let (found, first) = rank("ship a sales order");
    assert!(first < Duration::from_secs(15) && found[0].semantic.is_none(), "{first:?} {found:?}");
    let (_, again) = rank("ship a sales order");
    assert!(again < Duration::from_secs(1), "the breaker is open: {again:?}");
    assert!(!hung.retries_each(&moqui_lib::error::Error::new("x")), "a batch is not retried a text at a time while it is open");

    // the embedder's name is fixed: once its binding goes to another model, it refuses rather than mixing vectors
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        create(&factory, "moqui.llm.LlmFeatureBinding", &[("featureId", "embed"), ("modelId", "gw-embed2"), ("reasoningLevel", "none")]);
    }
    let refused = embedder.embed(&["forgot password".to_string()]).unwrap_err();
    assert!(refused.message().contains("llm:gw-embed2/upstream-embed-2"), "{refused}");

    // the official OpenAI Python client, when a python with it is named; CI without one skips it, loudly
    let python = std::env::var("OPENAI_PYTHON").unwrap_or_default();
    if python.is_empty() {
        eprintln!(
            "SKIPPED: the OpenAI Python client check (tests/fixtures/gateway/openai_client.py) did not run: OPENAI_PYTHON \
             is not set; docs/Gateway.md says how to make the venv"
        );
        return;
    }
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gateway/openai_client.py");
    let output = std::process::Command::new(&python)
        .arg("-I")
        .arg(&script)
        .arg(format!("http://{address}/llm/v1"))
        .env("GATEWAY_KEY", &ann)
        .env("GATEWAY_KEY_OVER_QUOTA", &eve)
        .output()
        .expect("the python of OPENAI_PYTHON runs");
    let printed = format!("{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    eprintln!("{printed}");
    assert!(output.status.success(), "the OpenAI client script failed:\n{printed}");
}

#[test]
fn the_gateway_is_off_by_default() {
    let gate = start_with_conf_properties("gateway-off", None, "", &[]);
    let address = gate.server.address();
    let (status, _, body) = send(address, "Bearer x", "GET", "/models", &Json::Null);
    assert_eq!(status, 404, "{body}");
    let (status, _, _) = send(address, "", "POST", "/chat/completions", &json!({"model": "x"}));
    assert_eq!(status, 404);
}

#[test]
fn the_gateway_bounds_a_call_by_its_own_timeout_and_app_passwords_need_a_client() {
    let upstream = Upstream::start();
    let conf = format!(
        r#"<llm-facade>
    <endpoint id="gw-up" protocol="chat-completions" url="http://127.0.0.1:{}"/>
    <model id="gw-chat" endpoint="gw-up" name="upstream-chat"/>
    <gateway enabled="true" require-client="false" timeout="3"><alias name="fast" model="gw-chat"/></gateway>
</llm-facade>"#,
        upstream.port
    );
    // the webapp's request deadline is shorter than the gateway's timeout: the gateway's own bounds the call
    let gate = start_with_conf_properties("gateway-timeout", Some(DATA), &conf, &[("webapp_request_timeout", "1")]);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let ann = key_of(&factory, "ann.gw", None);
    let started = Instant::now();
    let (status, headers, body) = send(address, &format!("Bearer {ann}"), "POST", "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "slow-whole"}]}));
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (504, Some("upstream_timeout")),
        "{body} {:?}",
        calls(&factory, "GW_ANN").last().map(|row| field(row, "errorText"))
    );
    assert!(header(&headers, "content-type").is_some_and(|value| value.contains("json")));
    assert!(started.elapsed() >= Duration::from_secs(3), "{:?}", started.elapsed());
    // without require-client a login key needs no client, but an app password (made for DAV, SFTP or git) does
    let (status, _, body) = send(address, &format!("Bearer {ann}"), "POST", "/chat/completions", &json!({
        "model": "fast", "messages": [{"role": "user", "content": "hi"}]}));
    assert_eq!(status, 200, "{body}");
    let app_password = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("ann.gw").unwrap();
        let _authz = context.disable_authz();
        let parameters: std::collections::BTreeMap<String, Value> =
            [("name".to_string(), Value::string("A DAV client"))].into_iter().collect();
        let out = moqui_core::service::sync("org.moqui.impl.UserServices.create#UserAppPassword", &parameters).unwrap();
        moqui_lib::object::to_plain_string(out.get("password").unwrap())
    };
    let (status, _, body) = send(address, &format!("Bearer {app_password}"), "GET", "/models", &Json::Null);
    assert_eq!((status, body["error"]["code"].as_str()), (403, Some("client_required")), "{body}");
}
