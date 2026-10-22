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

//! The MCP server (docs/MCP.md; plans/LLM-A2A-Assist.md 4.16) over HTTP on a seeded server on embedded Postgres: the
//! public documents, the transport's refusals (GET, a foreign Origin, a missing version header), `server/discover`
//! without credentials, auth and permissions, `tools/list` sorted and private with its TTL, `screen` going on in its
//! session, a read, and a write that answers `input_required` with its link: the owner opens the link's conversation
//! in the drawer of their browser session and approves the card there (a header login cannot), and the client's retry
//! with the `requestState` runs it once, then replays it. With `MCP_SDK_PYTHON` naming a python that has the `mcp`
//! SDK (2.3.0), `tests/fixtures/mcp/smoke_mcp_sdk.py` runs the official client against the same server.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "llm"))]

use std::net::SocketAddr;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use serde_json::{json, Value as Json};

use crate::common::{
    exchange, http_request, merge_cookies, session_token, sign_in, start_with_conf_files, start_with_conf_properties,
};

const CONF: &str = r#"<llm-facade><mcp server-enabled="true" approval-wait="2" tools-ttl-ms="60000"/></llm-facade>
    <screen-facade><screen location="component://webroot/screen/webroot/apps.xml">
        <subscreens-item name="mcpscreens" menu-title="MCP Screens" location="component://mcpscreens/screen/McpScreens.xml"/>
    </screen></screen-facade>"#;

/// A component with one form that saves an `EnumerationType`, for `screen` writes through MCP.
const FILES: [(&str, &str); 3] = [
    (
        "component/mcpscreens/component.xml",
        r#"<component name="mcpscreens"/>"#,
    ),
    (
        "component/mcpscreens/screen/McpScreens.xml",
        r#"<screen require-authentication="true" default-menu-title="MCP Screens">
    <subscreens default-item="Edit"/><widgets><subscreens-active/></widgets></screen>"#,
    ),
    (
        "component/mcpscreens/screen/McpScreens/Edit.xml",
        r#"<screen require-authentication="true" default-menu-title="Edit">
    <transition name="updateEnumType"><service-call name="update#moqui.basic.EnumerationType"/>
        <default-response url="."/></transition>
    <actions><entity-find-one entity-name="moqui.basic.EnumerationType" value-field="enumType">
        <field-map field-name="enumTypeId" value="MCP_SCREEN"/></entity-find-one></actions>
    <widgets><form-single name="EditEnumType" transition="updateEnumType" map="enumType">
        <field name="enumTypeId"><default-field><hidden/></default-field></field>
        <field name="description"><default-field title="Description"><text-line/></default-field></field>
        <field name="submitButton"><default-field title="Save"><submit/></default-field></field>
    </form-single></widgets></screen>"#,
    ),
];

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.basic.EnumerationType enumTypeId="MCP_TYPE" description="Before"/>
    <moqui.basic.EnumerationType enumTypeId="MCP_SDK_TYPE" description="Before"/>
    <moqui.basic.EnumerationType enumTypeId="MCP_SCREEN" description="Before"/>
    <moqui.agent.AgentToolExposure toolName="update_enum_type" serviceName="update#moqui.basic.EnumerationType"
        title="Change an enumeration type" description="Changes the description of an enumeration type"
        exposeMcp="Y" exposeAssistant="N" lockVersion="2"/>
    <moqui.agent.AgentToolExposure toolName="find_enums" serviceName="org.moqui.impl.BasicServices.find#Enumeration"
        title="Find enumerations" exposeMcp="Y" lockVersion="1"/>
    <moqui.security.ArtifactGroup artifactGroupId="MCP_TEST" description="MCP test"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="MCP_TEST" artifactName="component://mcpscreens/.*"
        nameIsPattern="Y" artifactTypeEnumId="AT_XML_SCREEN" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="MCP_TEST" artifactName="component://mcpscreens/.*"
        nameIsPattern="Y" artifactTypeEnumId="AT_XML_SCREEN_TRANS" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="MCP_TEST" artifactName="component://webroot/screen/webroot/apps.xml"
        artifactTypeEnumId="AT_XML_SCREEN" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="MCP_TEST" artifactName=".*Enumeration.*"
        nameIsPattern="Y" artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="MCP_TEST" artifactName="moqui\.basic\.Enumeration.*"
        nameIsPattern="Y" artifactTypeEnumId="AT_ENTITY"/>
    <moqui.security.UserAccount userId="MCP_ANN" username="ann.mcp" userFullName="Ann MCP"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA" locale="en_US" timeZone="US/Central"/>
    <moqui.security.UserAccount userId="MCP_BOB" username="bob.mcp" userFullName="Bob MCP"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserAccount userId="MCP_CARL" username="carl.mcp" userFullName="Carl MCP"
        currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA"/>
    <moqui.security.UserGroup userGroupId="MCP_CALLERS" description="MCP callers"/>
    <moqui.security.UserGroupMember userGroupId="MCP_CALLERS" userId="MCP_ANN" fromDate="1265184000000"/>
    <moqui.security.UserGroupMember userGroupId="MCP_CALLERS" userId="MCP_BOB" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="MCP_CALLERS" userPermissionId="AgentUse" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="MCP_CALLERS" userPermissionId="McpServerUse" fromDate="1265184000000"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="MCP_TEST_CALLERS" userGroupId="MCP_CALLERS" artifactGroupId="MCP_TEST"
        authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    <!-- carl uses the agents in the drawer but may not call the MCP server -->
    <moqui.security.UserGroup userGroupId="MCP_DRAWER" description="Drawer only"/>
    <moqui.security.UserGroupMember userGroupId="MCP_DRAWER" userId="MCP_CARL" fromDate="1265184000000"/>
    <moqui.security.UserGroupPermission userGroupId="MCP_DRAWER" userPermissionId="AgentUse" fromDate="1265184000000"/>
</entity-facade-xml>"#;

/// A caller with a login key.
struct Caller {
    address: SocketAddr,
    key: String,
}

fn meta(caps: Json) -> Json {
    json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": caps,
        "io.modelcontextprotocol/clientInfo": {"name": "it-client", "version": "1"}})
}

impl Caller {
    /// One request with the headers a client sends, `extra` over them (an empty value drops one): the status, the
    /// headers and the body.
    fn post(&self, body: &Json, extra: &[(&str, &str)]) -> (u16, Vec<(String, String)>, Json) {
        let method = body["method"].as_str().unwrap_or_default().to_string();
        let auth = format!("Bearer {}", self.key);
        let mut headers: Vec<(String, String)> = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json, text/event-stream".to_string()),
            ("MCP-Protocol-Version".to_string(), "2026-07-28".to_string()),
            ("Mcp-Method".to_string(), method),
            ("Authorization".to_string(), auth),
        ];
        if let Some(name) = body["params"]["name"].as_str() {
            headers.push(("Mcp-Name".to_string(), name.to_string()));
        }
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
        let (status, headers, text) = exchange(self.address, &http_request("POST", "/mcp", &refs, &body.to_string()));
        let body = if text.is_empty() {
            Json::Null
        } else {
            serde_json::from_str(&text).unwrap_or_else(|_| json!({"unparsed": text}))
        };
        (status, headers, body)
    }

    fn rpc(&self, method: &str, params: Json) -> (u16, Json) {
        let (status, _, body) = self.post(
            &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
            &[],
        );
        (status, body)
    }

    /// The result of a `tools/call` that must be answered with one.
    fn call(&self, name: &str, arguments: Json, extra: Json) -> Json {
        let mut params =
            json!({"name": name, "arguments": arguments, "_meta": meta(json!({"elicitation": {"url": {}}}))});
        if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            params.extend(extra.clone());
        }
        let (status, response) = self.rpc("tools/call", params);
        assert!(
            status == 200 && response.get("result").is_some(),
            "{name}: {status} {response}"
        );
        response["result"].clone()
    }
}

fn login_key(factory: &Arc<ExecutionContextFactory>, username: &str) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(username).unwrap();
    moqui_core::user::get_login_key(None).unwrap()
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

fn description(factory: &Arc<ExecutionContextFactory>, enum_type_id: &str) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory
        .entity()
        .unwrap()
        .find("moqui.basic.EnumerationType")
        .unwrap()
        .eq("enumTypeId", moqui_lib::object::Value::string(enum_type_id))
        .one()
        .unwrap()
        .and_then(|row| row.get("description").map(moqui_lib::object::to_plain_string))
        .unwrap_or_default()
}

fn text(result: &Json) -> String {
    result["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

#[test]
fn the_mcp_server_over_http_on_postgres() {
    let gate = start_with_conf_files("mcp", Some(DATA), CONF, &[], &FILES);
    let factory = Arc::clone(gate.server.factory());
    let address = gate.server.address();
    let ann = Caller {
        address,
        key: login_key(&factory, "ann.mcp"),
    };
    let bob = Caller {
        address,
        key: login_key(&factory, "bob.mcp"),
    };
    let carl = Caller {
        address,
        key: login_key(&factory, "carl.mcp"),
    };

    // the public documents
    let (status, headers, card) = exchange(address, &http_request("GET", "/mcp/server-card", &[], ""));
    assert_eq!(status, 200, "{card}");
    assert_eq!(
        crate::common::header(&headers, "content-type").as_deref(),
        Some("application/mcp-server-card+json")
    );
    assert_eq!(
        crate::common::header(&headers, "access-control-allow-origin").as_deref(),
        Some("*")
    );
    let card: Json = serde_json::from_str(&card).unwrap();
    assert_eq!(card["remotes"][0]["url"], "http://localhost/mcp");
    assert_eq!(card["remotes"][0]["supportedProtocolVersions"], json!(["2026-07-28"]));
    let (status, _, same) = exchange(address, &http_request("GET", "/.well-known/mcp.json", &[], ""));
    assert_eq!(
        (status, serde_json::from_str::<Json>(&same).unwrap()),
        (200, card.clone())
    );
    let (status, _, catalog) = exchange(address, &http_request("GET", "/.well-known/ai-catalog.json", &[], ""));
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Json>(&catalog).unwrap()["entries"][0]["url"],
        "http://localhost/mcp/server-card"
    );
    let (status, _, setup) = exchange(address, &http_request("GET", "/mcp/setup", &[], ""));
    assert!(status == 200 && setup.contains("http://localhost/mcp"), "{setup}");

    // the transport refuses before it reads
    let (status, headers, _) = exchange(address, &http_request("GET", "/mcp", &[], ""));
    assert_eq!(
        (status, crate::common::header(&headers, "allow").as_deref()),
        (405, Some("POST"))
    );
    let list = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta(json!({}))}});
    assert_eq!(ann.post(&list, &[("Origin", "https://evil.example.com")]).0, 403);
    assert_eq!(
        ann.post(&list, &[("Origin", "http://localhost")]).0,
        200,
        "the server's own origin"
    );
    let (status, _, missing) = ann.post(&list, &[("MCP-Protocol-Version", "")]);
    assert_eq!((status, missing["error"]["code"].as_i64()), (400, Some(-32020)));
    let (status, _, mismatch) = ann.post(&list, &[("Mcp-Method", "tools/call")]);
    assert_eq!((status, mismatch["error"]["code"].as_i64()), (400, Some(-32020)));

    // discover is public; the rest needs credentials and the permissions
    let nobody = Caller {
        address,
        key: "not-a-key".to_string(),
    };
    let (status, discovered) = nobody.rpc("server/discover", json!({"_meta": meta(json!({}))}));
    assert_eq!(status, 200, "{discovered}");
    assert_eq!(discovered["result"]["supportedVersions"], json!(["2026-07-28"]));
    assert!(discovered["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("requestState"));
    let (status, _, refused) = nobody.post(&list, &[]);
    assert_eq!((status, refused["error"]["code"].as_i64()), (401, Some(-31401)));
    assert_eq!(
        carl.rpc("tools/list", json!({"_meta": meta(json!({}))})).0,
        403,
        "AgentUse alone is not enough"
    );

    let (status, listed) = ann.rpc("tools/list", json!({"_meta": meta(json!({}))}));
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["result"]["cacheScope"], "private");
    assert_eq!(listed["result"]["ttlMs"], 60_000);
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "sorted by name");
    for name in ["data", "find_enums", "search", "skill", "update_enum_type"] {
        assert!(names.iter().any(|known| known == name), "{name} in {names:?}");
    }
    assert!(!names.iter().any(|name| name == "run_tool"), "{names:?}");

    // screen goes on in its session, which only its user may name
    if names.iter().any(|name| name == "screen") {
        let first = ann.call("screen", json!({"go": "/apps"}), json!({}));
        let session = first["_meta"]["org.moqui/session"].as_str().unwrap().to_string();
        assert!(text(&first).contains(&format!("session: {session}")), "{first}");
        let again = ann.call("screen", json!({"session": session}), json!({}));
        assert_eq!(again["_meta"]["org.moqui/session"], session.as_str(), "{again}");
        let foreign = bob.call("screen", json!({"session": session}), json!({}));
        assert_eq!(foreign["isError"], true, "{foreign}");
    } else {
        eprintln!("SKIPPED: the screen checks of the MCP test: this build has no screen sessions (the tui feature)");
    }

    // a read
    let found = ann.call("find_enums", json!({"enumTypeId": "MCP_TYPE"}), json!({}));
    assert_eq!(found["isError"], false, "{found}");

    // a write: the link, the owner's decision in Moqui, the retry runs it once
    let arguments = json!({"enumTypeId": "MCP_TYPE", "description": "After"});
    let asked = ann.call("update_enum_type", arguments.clone(), json!({}));
    assert_eq!(asked["resultType"], "input_required", "{asked}");
    let url = asked["inputRequests"]["approval"]["params"]["url"]
        .as_str()
        .unwrap()
        .to_string();
    let conversation = url
        .strip_prefix("http://localhost/qapps?assist=")
        .expect("the drawer's link")
        .to_string();
    let request_state = asked["requestState"].as_str().unwrap().to_string();
    assert_eq!(description(&factory, "MCP_TYPE"), "Before");
    // another user's retry with the state is refused
    let stolen = bob.rpc(
        "tools/call",
        json!({"name": "update_enum_type", "arguments": arguments, "requestState": request_state,
            "_meta": meta(json!({"elicitation": {"url": {}}}))}),
    );
    assert_eq!(stolen.1["error"]["code"].as_i64(), Some(-32602), "{stolen:?}");
    // the owner opens the link: the drawer shows the call's conversation with its card
    let (cookie, session) = browser(address, "ann.mcp");
    let (status, _, shown) = exchange(
        address,
        &http_request(
            "GET",
            &format!("/assist/conversation?conversationId={conversation}"),
            &[
                ("Cookie", &cookie),
                ("Accept", "application/json"),
                ("X-CSRF-Token", &session),
            ],
            "",
        ),
    );
    assert_eq!(status, 200, "{shown}");
    let shown: Json = serde_json::from_str(&shown).unwrap();
    let card = &shown["pending"]["approvals"][0];
    assert_eq!(card["targetName"], "update#moqui.basic.EnumerationType", "{shown}");
    let approval_id = card["approvalId"].as_str().unwrap().to_string();
    assert_eq!(shown["activeRun"]["state"], "TASK_STATE_AUTH_REQUIRED", "{shown}");
    // the MCP client, a header login, cannot decide
    let decide = json!({"approvalId": approval_id, "decision": "approve"}).to_string();
    let bearer = format!("Bearer {}", ann.key);
    let (status, _, _) = exchange(
        address,
        &http_request(
            "POST",
            "/assist/decide",
            &[
                ("Authorization", &bearer),
                ("Accept", "application/json"),
                ("Content-Type", "application/json"),
            ],
            &decide,
        ),
    );
    assert!(
        status == 401 || status == 403,
        "a header login decides nothing: {status}"
    );
    // still pending: the retry that said it opened the link waits, then asks again with the same state
    let waited = ann.call(
        "update_enum_type",
        arguments.clone(),
        json!({"requestState": request_state, "inputResponses": {"approval": {"action": "accept"}}}),
    );
    assert_eq!(waited["resultType"], "input_required", "{waited}");
    assert_eq!(waited["requestState"], request_state.as_str());
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
            &decide,
        ),
    );
    assert_eq!(status, 200, "{decided}");
    assert_eq!(
        description(&factory, "MCP_TYPE"),
        "Before",
        "the decision does not run it; the retry does"
    );
    let ran = ann.call(
        "update_enum_type",
        arguments.clone(),
        json!({"requestState": request_state, "inputResponses": {"approval": {"action": "accept"}}}),
    );
    assert_eq!(
        (ran["resultType"].as_str(), ran["isError"].as_bool()),
        (Some("complete"), Some(false)),
        "{ran}"
    );
    assert_eq!(description(&factory, "MCP_TYPE"), "After");
    let replayed = ann.call(
        "update_enum_type",
        arguments.clone(),
        json!({"requestState": request_state}),
    );
    assert_eq!(replayed["content"], ran["content"], "a replay of the stored answer");

    if names.iter().any(|name| name == "screen") {
        screen_writes(&factory, &ann, address);
    }

    // the official SDK client, when a python with mcp 2.3.0 is named; CI without one skips it, loudly
    let python = std::env::var("MCP_SDK_PYTHON").unwrap_or_default();
    if python.is_empty() {
        eprintln!(
            "SKIPPED: the MCP SDK smoke check (tests/fixtures/mcp/smoke_mcp_sdk.py) did not run: MCP_SDK_PYTHON is \
             not set; docs/MCP.md says how to make the venv"
        );
        return;
    }
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp/smoke_mcp_sdk.py");
    let output = std::process::Command::new(&python)
        .arg("-I")
        .arg(&script)
        .arg(format!("http://{address}"))
        .env("MCP_TOKEN", &ann.key)
        .env("MCP_OWNER_COOKIE", &cookie)
        .env("MCP_OWNER_CSRF", &session)
        .env("MCP_READ_TOOL", "find_enums")
        .env("MCP_READ_ARGS", json!({"enumTypeId": "MCP_TYPE"}).to_string())
        .env("MCP_WRITE_TOOL", "update_enum_type")
        .env(
            "MCP_WRITE_ARGS",
            json!({"enumTypeId": "MCP_SDK_TYPE", "description": "By the SDK"}).to_string(),
        )
        .output()
        .expect("the python of MCP_SDK_PYTHON runs");
    let printed = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!("{printed}");
    assert!(output.status.success(), "the MCP SDK smoke script failed:\n{printed}");
    assert_eq!(description(&factory, "MCP_SDK_TYPE"), "By the SDK");
}

#[test]
fn the_mcp_server_is_off_by_default() {
    let gate = start_with_conf_properties("mcp-off", None, "", &[]);
    let address = gate.server.address();
    for path in [
        "/mcp/server-card",
        "/.well-known/mcp.json",
        "/.well-known/ai-catalog.json",
        "/mcp/setup",
    ] {
        let (status, _, _) = exchange(address, &http_request("GET", path, &[], ""));
        assert_eq!(status, 404, "{path}");
    }
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": meta(json!({}))}})
        .to_string();
    let (status, _, _) = exchange(
        address,
        &http_request(
            "POST",
            "/mcp",
            &[
                ("Content-Type", "application/json"),
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "server/discover"),
            ],
            &body,
        ),
    );
    assert_eq!(status, 404);
}

/// A loopback proxy that adds `Authorization` to each request it forwards to `target`, one request a connection: the
/// published conformance suite sends no credentials. Out: its address.
fn auth_proxy(target: SocketAddr, authorization: String) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let authorization = authorization.clone();
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader, Read, Write};
                let mut reader = BufReader::new(client.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0_usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(value) = lower.strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    if !lower.starts_with("connection:") && !lower.starts_with("authorization:") {
                        head.push_str(&line);
                    }
                }
                let mut body = vec![0_u8; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                head.push_str(&format!("Authorization: {authorization}\r\nConnection: close\r\n\r\n"));
                let Ok(mut server) = std::net::TcpStream::connect(target) else {
                    return;
                };
                let _ = server.write_all(head.as_bytes());
                let _ = server.write_all(&body);
                let mut answer = Vec::new();
                let _ = server.read_to_end(&mut answer);
                let mut client = client;
                let _ = client.write_all(&answer);
            });
        }
    });
    address
}

/// The scenarios of the published conformance suite (`@modelcontextprotocol/conformance` 0.2.0-alpha.12, required for
/// 2026-07-28) that hold for any server. The rest need its reference server's own tools, resources and prompts.
const CONFORMANCE: [&str; 4] = ["server-stateless", "tools-list", "dns-rebinding-protection", "caching"];

/// The checks of those scenarios that do not apply here, so their failure is expected: the diagnostic tools of the
/// suite's reference server, and the caching hints of resources and prompts, which this server does not serve.
const NOT_APPLICABLE: [&str; 7] = [
    "sep-2575-server-rejects-undeclared-capability",
    "sep-2575-missing-capability-http-400",
    "sep-2575-http-server-no-independent-requests-on-stream",
    "sep-2575-server-no-log-without-loglevel",
    "sep-2549-prompts-list-caching-hints",
    "sep-2549-resources-list-caching-hints",
    "sep-2549-resources-templates-list-caching-hints",
];

/// The `[check-id] STATUS` lines of the suite's output, its colors left out.
fn conformance_checks(printed: &str) -> Vec<(String, String)> {
    let mut plain = String::new();
    let mut chars = printed.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once('[')?;
            let (id, rest) = rest.split_once(']')?;
            let status = rest.split_whitespace().next()?;
            matches!(status, "SUCCESS" | "FAILURE" | "SKIPPED" | "WARNING" | "INFO")
                .then(|| (id.trim().to_string(), status.to_string()))
        })
        .collect()
}

#[test]
fn the_published_conformance_suite_where_it_applies() {
    // the suite's CLI (its node_modules/.bin/conformance, with Node 24 on the PATH); CI without one skips it, loudly
    let cli = std::env::var("MCP_CONFORMANCE").unwrap_or_default();
    if cli.is_empty() {
        eprintln!(
            "SKIPPED: the MCP conformance scenarios did not run: MCP_CONFORMANCE is not set; docs/MCP.md says how to \
             install the suite"
        );
        return;
    }
    let gate = start_with_conf_properties("mcp-conformance", Some(DATA), CONF, &[]);
    let factory = Arc::clone(gate.server.factory());
    let proxy = auth_proxy(
        gate.server.address(),
        format!("Bearer {}", login_key(&factory, "ann.mcp")),
    );
    let results = std::env::temp_dir().join(format!("mcp-conformance-{}", std::process::id()));
    let mut failed = Vec::new();
    let mut passed = 0;
    for scenario in CONFORMANCE {
        let output = std::process::Command::new(&cli)
            .args([
                "server",
                "--url",
                &format!("http://{proxy}/mcp"),
                "--scenario",
                scenario,
            ])
            .args(["--spec-version", "2026-07-28"])
            .current_dir(std::env::temp_dir())
            .env("CONFORMANCE_RESULTS_DIR", &results)
            .output()
            .expect("the conformance CLI runs");
        let printed = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        eprintln!("--- {scenario}\n{printed}");
        let checks = conformance_checks(&printed);
        assert!(
            !checks.is_empty() || output.status.success(),
            "{scenario} ran no checks:\n{printed}"
        );
        for (id, status) in checks {
            match status.as_str() {
                "SUCCESS" => passed += 1,
                "FAILURE" if !NOT_APPLICABLE.contains(&id.as_str()) => failed.push(format!("{scenario}: {id}")),
                _ => {}
            }
        }
    }
    eprintln!("conformance: {passed} checks passed, {} failed", failed.len());
    assert!(passed > 0 && failed.is_empty(), "conformance checks failed: {failed:?}");
}

/// A `screen` press that saves, through MCP: held for its approval, approved in the owner's browser and run by the
/// retry; then handed off with Edit in screen, where the retry waits (not answering at once) until the user's own save.
fn screen_writes(factory: &Arc<ExecutionContextFactory>, ann: &Caller, address: SocketAddr) {
    let (cookie, session) = browser(address, "ann.mcp");
    let decide = |approval_id: &str, decision: &str| {
        let body = json!({"approvalId": approval_id, "decision": decision}).to_string();
        let (status, _, text) = exchange(
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
                &body,
            ),
        );
        assert_eq!(status, 200, "{decision}: {text}");
        serde_json::from_str::<Json>(&text).unwrap()
    };
    let save = |text: &str| json!({"go": "/apps/mcpscreens/Edit", "set": {"Description": text}, "do": "Save"});
    let accept = |state: &str| json!({"requestState": state, "inputResponses": {"approval": {"action": "accept"}}});

    // approved: the retry saves the exact held request
    let arguments = save("Saved through MCP");
    let asked = ann.call("screen", arguments.clone(), json!({}));
    assert_eq!(asked["resultType"], "input_required", "{asked}");
    let approval_id = asked["_meta"]["org.moqui/approval"]["approvalId"]
        .as_str()
        .unwrap()
        .to_string();
    let state = asked["requestState"].as_str().unwrap().to_string();
    assert_eq!(description(factory, "MCP_SCREEN"), "Before", "held, not saved");
    decide(&approval_id, "approve");
    let ran = ann.call("screen", arguments, accept(&state));
    assert_eq!(
        (ran["resultType"].as_str(), ran["isError"].as_bool()),
        (Some("complete"), Some(false)),
        "{ran}"
    );
    assert!(text(&ran).contains("session: "), "{ran}");
    assert_eq!(description(factory, "MCP_SCREEN"), "Saved through MCP");

    // Edit in screen: the retry waits for the user's own save instead of answering at once
    let arguments = save("Proposed through MCP");
    let asked = ann.call("screen", arguments.clone(), json!({}));
    assert_eq!(asked["resultType"], "input_required", "{asked}");
    let approval_id = asked["_meta"]["org.moqui/approval"]["approvalId"]
        .as_str()
        .unwrap()
        .to_string();
    let state = asked["requestState"].as_str().unwrap().to_string();
    let edited = decide(&approval_id, "edit");
    assert!(edited["handoffId"].is_string(), "{edited}");
    let started = std::time::Instant::now();
    let waited = ann.call("screen", arguments.clone(), accept(&state));
    assert_eq!(waited["resultType"], "input_required", "{waited}");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(1500),
        "a retry during a hand-off waits approval-wait: {:?}",
        started.elapsed()
    );
    let body = format!("enumTypeId=MCP_SCREEN&description=Saved+by+the+user&moquiSessionToken={session}");
    let (status, _, saved) = exchange(
        address,
        &http_request(
            "POST",
            "/apps/mcpscreens/Edit/updateEnumType",
            &[
                ("Cookie", &cookie),
                ("Content-Type", "application/x-www-form-urlencoded"),
                ("Accept", "application/json"),
            ],
            &body,
        ),
    );
    assert!(status < 400, "{status} {saved}");
    assert_eq!(description(factory, "MCP_SCREEN"), "Saved by the user");
    let resolved = ann.call("screen", arguments, accept(&state));
    assert_eq!(resolved["resultType"], "complete", "{resolved}");
    assert!(text(&resolved).contains("saved this write themselves"), "{resolved}");
}
