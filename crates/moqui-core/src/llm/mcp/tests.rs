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

//! The MCP server (plans/LLM-A2A-Assist.md 4.16, 9 "Approvals") on the in-memory database: the `requestState`'s
//! tamper, replay, TTL and principal checks; the transport's header checks; `server/discover`; the order, TTL and
//! cache scope of `tools/list` with the exposures it offers and the lazy trio past `max-direct-tools`; a read call
//! fenced; a write that answers `input_required` with its link, decided by the owner, then run once by the retry and
//! replayed after; a rejection; and a client without URL elicitations. Over HTTP on embedded Postgres, with the
//! owner deciding in a browser session and the MCP SDK's client, is `crates/moqui-host/tests/it/mcp.rs`.

use std::sync::Arc;

use base64::Engine;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::server::{self, Body};
use super::state::{self, Claims, Expect, Refused};
use super::{Answer, HttpRequest};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::llm::approval::{self, ApprovalStatus, Channel, Decider, Decision};
use crate::llm::run::{self, RunState};

const USER: &str = "mcp.tester";
const OTHER: &str = "mcp.other";
const KEY: &str = "bWNwLXRlc3Qta2V5LW9mLWF0LWxlYXN0LTMyLWJ5dGVzLWxvbmc=";

struct Bed {
    ctx: ExecutionContext,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
}

fn bed(extra: &str) -> Bed {
    bed_with(extra, "")
}

/// [`bed`] with more conf, the children of `moqui-conf`.
fn bed_with(extra: &str, conf: &str) -> Bed {
    let conf = format!(
        r#"<moqui-conf><llm-facade><mcp server-enabled="true" request-state-key="{KEY}" approval-wait="1"
            tools-ttl-ms="120000" {extra}/></llm-facade>{conf}</moqui-conf>"#
    );
    let ctx = ExecutionContextFactory::test_context_in(
        &conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    for id in [USER, OTHER] {
        row(
            &facade,
            "moqui.security.UserAccount",
            &[("userId", id), ("username", id), ("userFullName", id)],
        );
    }
    row(
        &facade,
        "moqui.agent.AgentDefinition",
        &[
            ("agentId", "mcp"),
            ("agentName", "MCP"),
            ("surfaces", "mcp"),
            ("exposeMcp", "Y"),
        ],
    );
    row(
        &facade,
        "moqui.basic.EnumerationType",
        &[("enumTypeId", "McpType"), ("description", "MCP")],
    );
    row(
        &facade,
        "moqui.basic.Enumeration",
        &[("enumId", "MCP_E1"), ("enumTypeId", "McpType"), ("description", "Old")],
    );
    for (tool, service, class, mcp, assistant) in [
        (
            "enum_find",
            "org.moqui.impl.BasicServices.find#Enumeration",
            "",
            "Y",
            "Y",
        ),
        ("enum_update", "update#moqui.basic.Enumeration", "", "Y", "N"),
        // a write labeled a read by mistake: the fence stops it
        ("enum_sneaky", "create#moqui.basic.Enumeration", "AgclRead", "Y", "N"),
        // the assistant's only: not offered over MCP
        ("enum_delete", "delete#moqui.basic.Enumeration", "", "N", "Y"),
        // a name MCP does not take as it is
        (
            "enum#type",
            "org.moqui.impl.BasicServices.find#Enumeration",
            "",
            "Y",
            "N",
        ),
    ] {
        let title = format!("Title of {tool}");
        let mut fields = vec![
            ("toolName", tool),
            ("serviceName", service),
            ("title", title.as_str()),
            ("exposeMcp", mcp),
            ("exposeAssistant", assistant),
            ("lockVersion", "3"),
        ];
        if !class.is_empty() {
            fields.push(("classificationEnumId", class));
        }
        row(&facade, crate::llm::tools::catalog::EXPOSURE, &fields);
    }
    Bed {
        factory: Arc::clone(ctx.factory()),
        ctx,
        facade,
    }
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (field, text) in fields {
        value.set(*field, Value::string(*text));
    }
    value.create(facade).unwrap();
}

/// A login key of `username`, made in the context entered.
fn login_key(username: &str) -> String {
    crate::user::internal_login_user(username).unwrap();
    let key = crate::user::get_login_key(None).unwrap();
    crate::user::logout().unwrap();
    key
}

fn meta(caps: Json) -> Json {
    json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": caps,
        "io.modelcontextprotocol/clientInfo": {"name": "test-client", "version": "1"}})
}

fn url_caps() -> Json {
    json!({"elicitation": {"url": {}}})
}

/// A request to `/mcp` with the headers a client sends, `headers` over them (an empty value drops one).
fn request(key: &str, body: &Json, headers: &[(&str, &str)]) -> HttpRequest {
    let method = body["method"].as_str().unwrap_or_default().to_string();
    let mut list: Vec<(String, String)> = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("accept".to_string(), "application/json, text/event-stream".to_string()),
        ("mcp-protocol-version".to_string(), "2026-07-28".to_string()),
        ("mcp-method".to_string(), method),
        ("authorization".to_string(), format!("Bearer {key}")),
    ];
    if let Some(name) = body["params"]["name"].as_str() {
        list.push(("mcp-name".to_string(), name.to_string()));
    }
    for (name, value) in headers {
        list.retain(|(known, _)| !known.eq_ignore_ascii_case(name));
        if !value.is_empty() {
            list.push(((*name).to_ascii_lowercase(), (*value).to_string()));
        }
    }
    HttpRequest {
        method: "POST".to_string(),
        headers: list,
        body: body.to_string().into_bytes(),
        secure: true,
        client_ip: "127.0.0.1".to_string(),
        ..HttpRequest::default()
    }
}

/// Serve `request`, a wait run to its end: the status and the body.
fn serve(bed: &Bed, request: &HttpRequest) -> (u16, Json) {
    let reply = match super::serve(&bed.factory, request) {
        Answer::Reply(reply) => reply,
        Answer::Later { finish, .. } => finish(),
        Answer::Stream { .. } => panic!("the MCP server does not stream"),
    };
    let body = if reply.body.is_empty() {
        Json::Null
    } else {
        serde_json::from_str(&reply.body).unwrap()
    };
    (reply.status, body)
}

fn rpc(bed: &Bed, key: &str, method: &str, params: Json) -> (u16, Json) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    serve(bed, &request(key, &body, &[]))
}

fn call(bed: &Bed, key: &str, name: &str, arguments: Json, extra: Json) -> Json {
    let mut params = json!({"name": name, "arguments": arguments, "_meta": meta(url_caps())});
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    let (status, response) = rpc(bed, key, "tools/call", params);
    assert_eq!(status, 200, "{response}");
    response
}

fn text_of(result: &Json) -> String {
    result["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

fn description(bed: &Bed, enum_id: &str) -> Option<String> {
    bed.facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string(enum_id))
        .disable_authz()
        .one()
        .unwrap()
        .and_then(|row| crate::llm::text_of(&row, "description"))
}

#[test]
fn a_request_state_is_refused_on_tamper_expiry_another_principal_or_another_request() {
    let key = b"0123456789abcdef0123456789abcdef".to_vec();
    let digest = state::digest("enum_update", &json!({"enumId": "E1", "description": "New"}));
    assert_eq!(
        digest,
        state::digest("enum_update", &json!({"description": "New", "enumId": "E1"})),
        "canonical"
    );
    assert_eq!(state::digest("t", &Json::Null), state::digest("t", &json!({})));
    let claims = Claims {
        v: state::VERSION,
        aid: "a1".to_string(),
        sub: USER.to_string(),
        cid: "r1".to_string(),
        tool: "enum_update".to_string(),
        dig: digest.clone(),
        exp: 2_000,
        n: state::nonce().unwrap(),
    };
    let sealed = state::seal(&key, &claims);
    let expect = Expect {
        user_id: USER,
        tool: "enum_update",
        digest: &digest,
        now: 1_000,
    };
    assert_eq!(state::open(&key, &sealed, &expect), Ok(claims.clone()));
    // the same state again opens the same: single use is the approval's, server side
    assert_eq!(state::open(&key, &sealed, &expect), Ok(claims.clone()));
    // a changed payload, a changed tag, another key
    let (payload, tag) = sealed.split_once('.').unwrap();
    let mut other = claims.clone();
    other.sub = OTHER.to_string();
    let forged = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&other).unwrap());
    assert_eq!(
        state::open(&key, &format!("{forged}.{tag}"), &expect),
        Err(Refused::Tampered)
    );
    let flipped = if tag.starts_with('A') { 'B' } else { 'A' };
    assert_eq!(
        state::open(&key, &format!("{payload}.{flipped}{}", &tag[1..]), &expect),
        Err(Refused::Tampered)
    );
    assert_eq!(
        state::open(b"another key, also 32 bytes long.", &sealed, &expect),
        Err(Refused::Tampered)
    );
    for malformed in ["", "no-dot", "a.b.c", "!!.??"] {
        assert!(
            matches!(
                state::open(&key, malformed, &expect),
                Err(Refused::Malformed | Refused::Tampered)
            ),
            "{malformed}"
        );
    }
    assert_eq!(state::open(&key, &"a".repeat(5000), &expect), Err(Refused::Malformed));
    // the TTL, the principal and the request
    assert_eq!(
        state::open(&key, &sealed, &Expect { now: 2_000, ..expect }),
        Err(Refused::Expired)
    );
    assert_eq!(
        state::open(
            &key,
            &sealed,
            &Expect {
                user_id: OTHER,
                ..expect
            }
        ),
        Err(Refused::OtherPrincipal)
    );
    assert_eq!(
        state::open(
            &key,
            &sealed,
            &Expect {
                tool: "enum_find",
                ..expect
            }
        ),
        Err(Refused::OtherRequest)
    );
    let changed = state::digest("enum_update", &json!({"enumId": "E1", "description": "Other"}));
    assert_eq!(
        state::open(
            &key,
            &sealed,
            &Expect {
                digest: &changed,
                ..expect
            }
        ),
        Err(Refused::OtherRequest)
    );
    let mut old = claims;
    old.v = 9;
    assert_eq!(
        state::open(&key, &state::seal(&key, &old), &expect),
        Err(Refused::Version)
    );
}

#[test]
fn the_transport_checks_the_method_origin_content_types_and_mirrored_headers() {
    let bed = bed("");
    let _entered = bed.ctx.enter();
    let body = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
        "params": {"name": "enum_find", "arguments": {}, "_meta": meta(json!({}))}});
    let checked = |headers: &[(&str, &str)]| {
        let request = request("k", &body, headers);
        server::check_transport(&bed.factory, &request)
            .and_then(|()| match server::read_body(&bed.factory, &request) {
                Ok(_) => Ok(()),
                Err(refused) => Err(refused.1),
            })
            .map_err(|error| (error.status, error.code))
    };
    assert_eq!(checked(&[]), Ok(()));
    let mut get = request("k", &body, &[]);
    get.method = "GET".to_string();
    let refused = server::check_transport(&bed.factory, &get).unwrap_err();
    assert_eq!(
        (refused.status, refused.headers.clone()),
        (405, vec![("allow".to_string(), "POST".to_string())])
    );
    assert_eq!(checked(&[("Origin", "https://evil.example.com")]).unwrap_err().0, 403);
    // a DNS rebinding page names itself in both Host and Origin: the request's Host is never what allows an Origin
    let mut rebound = request("k", &body, &[("Origin", "http://evil.example.com")]);
    rebound.origin.host = Some("evil.example.com".to_string());
    assert_eq!(server::check_transport(&bed.factory, &rebound).unwrap_err().status, 403);
    assert_eq!(checked(&[("Origin", "http://127.0.0.1:8080")]), Ok(()), "this machine");
    assert_eq!(checked(&[("Origin", "http://localhost")]), Ok(()));
    assert_eq!(checked(&[("Origin", "http://app.localhost:3000")]), Ok(()));
    assert_eq!(checked(&[("Origin", "http://[::1]:8080")]), Ok(()));
    // a name that only starts like a loopback address is not one
    for hostile in [
        "http://127.attacker.example",
        "http://127.0.0.1.attacker.example",
        "http://localhost.attacker.example",
    ] {
        assert_eq!(checked(&[("Origin", hostile)]).unwrap_err().0, 403, "{hostile}");
    }
    assert_eq!(checked(&[("Content-Type", "text/plain")]).unwrap_err().0, 415);
    assert_eq!(checked(&[("Content-Type", "application/json; charset=utf-8")]), Ok(()));
    assert_eq!(checked(&[("Accept", "text/html")]).unwrap_err().0, 406);
    assert_eq!(checked(&[("Accept", "*/*")]), Ok(()));
    // the mirrored headers: missing or different from the body is HeaderMismatch, 400
    for headers in [
        vec![("MCP-Protocol-Version", "")],
        vec![("MCP-Protocol-Version", "2025-11-25")],
        vec![("Mcp-Method", "")],
        vec![("Mcp-Method", "tools/list")],
        vec![("Mcp-Name", "")],
        vec![("Mcp-Name", "enum_update")],
        vec![("Mcp-Name", "=?base64?not base64?=")],
    ] {
        assert_eq!(
            checked(&headers),
            Err((400, super::code::HEADER_MISMATCH)),
            "{headers:?}"
        );
    }
    // the Base64 sentinel is decoded before it is compared
    assert_eq!(checked(&[("Mcp-Name", "=?base64?ZW51bV9maW5k?=")]), Ok(()));
    assert_eq!(
        server::header_value("=?base64?SGVsbG8sIOS4lueVjA==?=").unwrap(),
        "Hello, 世界"
    );
    // the body: one request, its _meta, a version this server takes
    let read = |body: Json, headers: &[(&str, &str)]| {
        let request = request("k", &body, headers);
        server::read_body(&bed.factory, &request).map_err(|refused| (refused.0, refused.1.status, refused.1.code))
    };
    let list = |meta: Json| json!({"jsonrpc": "2.0", "id": "a", "method": "tools/list", "params": {"_meta": meta}});
    assert!(matches!(read(list(meta(json!({}))), &[]), Ok(Body::Request(_))));
    assert_eq!(
        read(list(json!({"io.modelcontextprotocol/clientCapabilities": {}})), &[]).unwrap_err(),
        (Some(json!("a")), 400, super::code::INVALID_PARAMS)
    );
    assert_eq!(
        read(
            list(json!({"io.modelcontextprotocol/protocolVersion": "2026-07-28"})),
            &[]
        )
        .unwrap_err()
        .2,
        super::code::INVALID_PARAMS,
        "the client's capabilities are required"
    );
    let old = json!({"io.modelcontextprotocol/protocolVersion": "1900-01-01",
        "io.modelcontextprotocol/clientCapabilities": {}});
    assert_eq!(
        read(list(old.clone()), &[("MCP-Protocol-Version", "1900-01-01")])
            .unwrap_err()
            .2,
        super::code::UNSUPPORTED_VERSION
    );
    assert_eq!(
        read(list(old), &[]).unwrap_err().2,
        super::code::HEADER_MISMATCH,
        "header and body differ"
    );
    let rejected = |text: &str| {
        let mut request = request("k", &list(meta(json!({}))), &[]);
        request.body = text.as_bytes().to_vec();
        server::read_body(&bed.factory, &request)
            .map(|_| ())
            .map_err(|refused| (refused.1.status, refused.1.code))
    };
    assert_eq!(rejected("{").unwrap_err(), (400, super::code::PARSE));
    assert_eq!(
        rejected(r#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"}]"#)
            .unwrap_err()
            .1,
        super::code::INVALID_REQUEST
    );
    assert_eq!(
        rejected(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap_err().1,
        super::code::INVALID_REQUEST
    );
    assert_eq!(
        rejected(r#"{"jsonrpc":"2.0","id":null,"method":"tools/list"}"#)
            .unwrap_err()
            .1,
        super::code::INVALID_REQUEST
    );
    assert!(matches!(
        rejected(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
        Ok(())
    ));
    let notification = request("k", &json!({"jsonrpc": "2.0", "method": "notifications/x"}), &[]);
    assert!(matches!(
        server::read_body(&bed.factory, &notification),
        Ok(Body::Notification)
    ));
    assert_eq!(serve(&bed, &notification), (202, Json::Null));
}

#[test]
fn discover_is_public_and_tools_list_is_the_callers_sorted_with_its_ttl() {
    let bed = bed("");
    let _entered = bed.ctx.enter();
    let key = login_key(USER);
    let (status, discovered) = rpc(
        &bed,
        "no-credentials",
        "server/discover",
        json!({"_meta": meta(json!({}))}),
    );
    assert_eq!(status, 200, "{discovered}");
    let result = &discovered["result"];
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
    assert_eq!(result["capabilities"], json!({"tools": {}}));
    assert_eq!(
        (result["ttlMs"].as_u64(), result["cacheScope"].as_str()),
        (Some(120_000), Some("public"))
    );
    let instructions = result["instructions"].as_str().unwrap();
    assert!(
        instructions.contains("absent means null") && instructions.contains("requestState"),
        "{instructions}"
    );
    assert!(
        (800..2000).contains(&instructions.len()),
        "about 1.2k characters: {}",
        instructions.len()
    );
    assert_eq!(result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "moqui");

    // every other method needs credentials, and the permissions
    let (status, refused) = rpc(&bed, "no-such-key", "tools/list", json!({"_meta": meta(json!({}))}));
    assert_eq!(
        (status, refused["error"]["code"].as_i64()),
        (401, Some(super::code::UNAUTHENTICATED))
    );
    let (status, refused) = rpc(&bed, &key, "tools/list", json!({"_meta": meta(json!({}))}));
    assert_eq!(
        (status, refused["error"]["code"].as_i64()),
        (403, Some(super::code::FORBIDDEN)),
        "{refused}"
    );
    for user in [USER, OTHER] {
        for permission in ["McpServerUse", "AgentUse"] {
            grant(&bed, user, permission);
        }
    }
    let (status, unknown) = rpc(&bed, &key, "resources/list", json!({"_meta": meta(json!({}))}));
    assert_eq!(
        (status, unknown["error"]["code"].as_i64()),
        (404, Some(super::code::METHOD_NOT_FOUND))
    );

    let (status, listed) = rpc(&bed, &key, "tools/list", json!({"_meta": meta(json!({}))}));
    assert_eq!(status, 200, "{listed}");
    let result = &listed["result"];
    assert_eq!(
        (result["ttlMs"].as_u64(), result["cacheScope"].as_str()),
        (Some(120_000), Some("private"))
    );
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    // the read tools (no screen sessions on this server) and the MCP exposures, by name; never the assistant's own
    assert_eq!(
        names,
        [
            "data",
            "enum_find",
            "enum_sneaky",
            "enum_type",
            "enum_update",
            "search",
            "skill"
        ]
    );
    let again = rpc(&bed, &key, "tools/list", json!({"_meta": meta(json!({}))})).1;
    assert_eq!(again["result"]["tools"], result["tools"], "the same order every time");
    let tool = |name: &str| {
        result["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap()
            .clone()
    };
    let update = tool("enum_update");
    assert_eq!(update["title"], "Title of enum_update");
    assert!(update["description"].as_str().unwrap().contains("approval"), "{update}");
    assert_eq!(update["annotations"]["readOnlyHint"], false);
    assert_eq!(update["_meta"]["org.moqui/lockVersion"], 3);
    assert_eq!(update["inputSchema"]["type"], "object");
    assert!(update["inputSchema"]["properties"]["enumId"].is_object(), "{update}");
    assert_eq!(tool("enum_find")["annotations"]["readOnlyHint"], true);
    assert_eq!(tool("search")["annotations"]["readOnlyHint"], true);
    let (_, paged) = rpc(
        &bed,
        &key,
        "tools/list",
        json!({"cursor": "next", "_meta": meta(json!({}))}),
    );
    assert_eq!(paged["error"]["code"].as_i64(), Some(super::code::INVALID_PARAMS));

    // past max-direct-tools, the lazy trio stands for the services
    let lazy = self::bed("max-direct-tools=\"2\"");
    let _entered = lazy.ctx.enter();
    let key = login_key(USER);
    for permission in ["McpServerUse", "AgentUse"] {
        grant(&lazy, USER, permission);
    }
    let (_, listed) = rpc(&lazy, &key, "tools/list", json!({"_meta": meta(json!({}))}));
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["data", "describe_tools", "find_tools", "run_tool", "search", "skill"]
    );
}

fn grant(bed: &Bed, user: &str, permission: &str) {
    let group = format!("MCP_{user}_{permission}");
    row(
        &bed.facade,
        "moqui.security.UserGroup",
        &[("userGroupId", &group), ("description", &group)],
    );
    row(
        &bed.facade,
        "moqui.security.UserGroupMember",
        &[("userGroupId", &group), ("userId", user), ("fromDate", "1265184000000")],
    );
    row(
        &bed.facade,
        "moqui.security.UserGroupPermission",
        &[
            ("userGroupId", &group),
            ("userPermissionId", permission),
            ("fromDate", "1265184000000"),
        ],
    );
}

fn ready(bed: &Bed) -> (String, String) {
    let key = login_key(USER);
    let other = login_key(OTHER);
    for user in [USER, OTHER] {
        for permission in ["McpServerUse", "AgentUse"] {
            grant(bed, user, permission);
        }
    }
    (key, other)
}

#[test]
fn a_read_runs_fenced_with_no_run() {
    let bed = bed("");
    let _entered = bed.ctx.enter();
    let (key, _) = ready(&bed);
    let found = call(&bed, &key, "enum_find", json!({"enumTypeId": "McpType"}), json!({}));
    let result = &found["result"];
    assert_eq!(
        (result["resultType"].as_str(), result["isError"].as_bool()),
        (Some("complete"), Some(false))
    );
    assert!(text_of(result).contains("MCP_E1"), "{found}");
    let sneaky = call(
        &bed,
        &key,
        "enum_sneaky",
        json!({"enumId": "MCP_SNEAK", "enumTypeId": "McpType"}),
        json!({}),
    );
    assert_eq!(sneaky["result"]["isError"], true, "{sneaky}");
    assert!(text_of(&sneaky["result"]).contains("read_tool_wrote"), "{sneaky}");
    assert!(
        description(&bed, "MCP_SNEAK").is_none(),
        "the mislabeled read wrote nothing"
    );
    // the registry's read tools run with no run too
    for (tool, arguments) in [
        ("search", json!({"text": "enumeration"})),
        ("skill", json!({})),
        ("data", json!({})),
    ] {
        let answered = call(&bed, &key, tool, arguments, json!({}));
        assert_eq!(answered["result"]["resultType"], "complete", "{tool}: {answered}");
        assert!(!text_of(&answered["result"]).is_empty(), "{tool}: {answered}");
    }
    let runs = bed
        .facade
        .find("moqui.agent.AgentRun")
        .unwrap()
        .disable_authz()
        .list()
        .unwrap();
    assert!(runs.values().is_empty(), "a read makes no run");
    let (_, unknown) = rpc(
        &bed,
        &key,
        "tools/call",
        json!({"name": "enum_delete", "_meta": meta(url_caps())}),
    );
    assert_eq!(
        unknown["error"]["code"].as_i64(),
        Some(super::code::INVALID_PARAMS),
        "{unknown}"
    );
}

#[test]
fn a_write_waits_for_its_owner_and_the_retry_runs_it_once() {
    let bed = bed("");
    let _entered = bed.ctx.enter();
    let (key, other) = ready(&bed);
    let arguments = json!({"enumId": "MCP_E1", "description": "New"});
    // a client that cannot show the link cannot write
    let (status, refused) = rpc(
        &bed,
        &key,
        "tools/call",
        json!({"name": "enum_update", "arguments": arguments, "_meta": meta(json!({"elicitation": {}}))}),
    );
    assert_eq!(
        (status, refused["error"]["code"].as_i64()),
        (400, Some(super::code::MISSING_CLIENT_CAPABILITY))
    );
    assert_eq!(
        refused["error"]["data"]["requiredCapabilities"],
        json!({"elicitation": {"url": {}}})
    );

    let asked = call(&bed, &key, "enum_update", arguments.clone(), json!({}));
    let result = &asked["result"];
    assert_eq!(result["resultType"], "input_required", "{asked}");
    let elicitation = &result["inputRequests"]["approval"];
    assert_eq!(elicitation["method"], "elicitation/create");
    assert_eq!(elicitation["params"]["mode"], "url");
    let url = elicitation["params"]["url"].as_str().unwrap();
    assert!(url.contains("/qapps?assist="), "{url}");
    let approval_id = result["_meta"]["org.moqui/approval"]["approvalId"]
        .as_str()
        .unwrap()
        .to_string();
    let request_state = result["requestState"].as_str().unwrap().to_string();
    let view = approval::load(&bed.facade, &approval_id).unwrap().unwrap();
    assert_eq!(view.status, ApprovalStatus::Pending);
    let paused = run::load_run(&bed.facade, &view.run_id).unwrap().unwrap();
    assert_eq!(
        (paused.state, paused.surface),
        (RunState::AuthRequired, crate::llm::prompt::Surface::Mcp)
    );
    assert!(
        url.ends_with(&paused.conversation_id),
        "the link opens the call's conversation: {url}"
    );
    assert_eq!(
        description(&bed, "MCP_E1").as_deref(),
        Some("Old"),
        "nothing runs before the decision"
    );
    let retry = |key: &str, arguments: &Json, state: &str, responses: Json| {
        call(
            &bed,
            key,
            "enum_update",
            arguments.clone(),
            json!({"requestState": state, "inputResponses": responses}),
        )
    };

    // the state is checked first: tampered, another user's, other arguments
    let tampered = format!(
        "{}{}",
        if request_state.starts_with('e') { 'f' } else { 'e' },
        &request_state[1..]
    );
    let code = |response: &Json| response["error"]["code"].as_i64();
    assert_eq!(
        code(&retry(&key, &arguments, &tampered, json!({}))),
        Some(super::code::INVALID_PARAMS)
    );
    assert_eq!(
        code(&retry(&other, &arguments, &request_state, json!({}))),
        Some(super::code::INVALID_PARAMS)
    );
    let changed = json!({"enumId": "MCP_E1", "description": "Changed"});
    assert_eq!(
        code(&retry(&key, &changed, &request_state, json!({}))),
        Some(super::code::INVALID_PARAMS)
    );
    // an expired state, made with the server's key
    let expired = state::seal(
        &base64::engine::general_purpose::STANDARD.decode(KEY).unwrap(),
        &Claims {
            v: state::VERSION,
            aid: approval_id.clone(),
            sub: USER.to_string(),
            cid: view.run_id.clone(),
            tool: "enum_update".to_string(),
            dig: state::digest("enum_update", &arguments),
            exp: run::now_millis() - 1,
            n: state::nonce().unwrap(),
        },
    );
    let refused = retry(&key, &arguments, &expired, json!({}));
    assert!(
        refused["error"]["message"].as_str().unwrap().contains("expired"),
        "{refused}"
    );

    // not decided yet: the client said it opened the link, so the retry waits, then asks to retry (no new link)
    let waited = retry(
        &key,
        &arguments,
        &request_state,
        json!({"approval": {"action": "accept"}}),
    );
    assert_eq!(waited["result"]["resultType"], "input_required", "{waited}");
    assert!(waited["result"].get("inputRequests").is_none(), "{waited}");
    assert_eq!(waited["result"]["requestState"], request_state.as_str());
    let declined = retry(
        &key,
        &arguments,
        &request_state,
        json!({"approval": {"action": "decline"}}),
    );
    assert_eq!(declined["result"]["isError"], true, "{declined}");
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Pending
    );
    assert_eq!(description(&bed, "MCP_E1").as_deref(), Some("Old"));

    // the owner decides in Moqui; the decision does not run the call: the retry does, once
    approval::decide(
        &bed.factory,
        &approval_id,
        &Decider::scripted(USER, Channel::Link),
        &Decision::Approve { arguments: None },
    )
    .unwrap();
    assert_eq!(
        run::load_run(&bed.facade, &view.run_id).unwrap().unwrap().state,
        RunState::AuthRequired
    );
    assert_eq!(description(&bed, "MCP_E1").as_deref(), Some("Old"));
    let ran = retry(
        &key,
        &arguments,
        &request_state,
        json!({"approval": {"action": "accept"}}),
    );
    assert_eq!(
        (ran["result"]["resultType"].as_str(), ran["result"]["isError"].as_bool()),
        (Some("complete"), Some(false)),
        "{ran}"
    );
    assert!(text_of(&ran["result"]).contains("it ran"), "{ran}");
    assert_eq!(description(&bed, "MCP_E1").as_deref(), Some("New"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Executed
    );
    assert_eq!(
        run::load_run(&bed.facade, &view.run_id).unwrap().unwrap().state,
        RunState::Completed
    );
    // a replay: the stored answer, nothing run again
    bed.facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("MCP_E1"))
        .disable_authz()
        .update_all([("description".to_string(), Value::string("Moved on"))])
        .unwrap();
    let replayed = retry(&key, &arguments, &request_state, json!({}));
    assert_eq!(replayed["result"]["content"], ran["result"]["content"], "{replayed}");
    assert_eq!(
        description(&bed, "MCP_E1").as_deref(),
        Some("Moved on"),
        "not run again"
    );
    assert_eq!(
        code(&retry(&other, &arguments, &request_state, json!({}))),
        Some(super::code::INVALID_PARAMS)
    );
}

#[test]
fn a_rejected_write_is_an_error_with_the_feedback() {
    let bed = bed("");
    let _entered = bed.ctx.enter();
    let (key, _) = ready(&bed);
    let arguments = json!({"enumId": "MCP_E1", "description": "Rejected"});
    let asked = call(&bed, &key, "enum_update", arguments.clone(), json!({}));
    let approval_id = asked["result"]["_meta"]["org.moqui/approval"]["approvalId"]
        .as_str()
        .unwrap()
        .to_string();
    let request_state = asked["result"]["requestState"].as_str().unwrap().to_string();
    // the MCP client cannot decide: its request is header-authenticated, never interactive
    assert!(Decider::from_request(Some(Channel::Link)).is_err());
    approval::decide(
        &bed.factory,
        &approval_id,
        &Decider::scripted(USER, Channel::Link),
        &Decision::Reject {
            feedback: "Use the other enumeration".to_string(),
        },
    )
    .unwrap();
    let answered = call(
        &bed,
        &key,
        "enum_update",
        arguments,
        json!({"requestState": request_state}),
    );
    assert_eq!(answered["result"]["isError"], true, "{answered}");
    assert!(
        text_of(&answered["result"]).contains("Use the other enumeration"),
        "{answered}"
    );
    assert_eq!(description(&bed, "MCP_E1").as_deref(), Some("Old"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Rejected
    );
}

#[test]
fn the_documents_name_the_endpoint() {
    let card = server::server_card("https://erp.example.com", &["2026-07-28".to_string()]);
    assert_eq!(card["name"], "org.moqui/moqui");
    assert!(card["description"].as_str().unwrap().len() <= 100);
    assert_eq!(card["remotes"][0]["url"], "https://erp.example.com/mcp");
    assert_eq!(card["remotes"][0]["type"], "streamable-http");
    let catalog = server::ai_catalog("https://erp.example.com:8443");
    assert_eq!(catalog["entries"][0]["identifier"], "urn:air:erp.example.com:mcp:moqui");
    assert_eq!(
        catalog["entries"][0]["url"],
        "https://erp.example.com:8443/mcp/server-card"
    );
    let setup = server::setup_prompt("https://erp.example.com", "2026-07-28");
    assert!(
        setup.contains("https://erp.example.com/mcp") && setup.contains("Bearer"),
        "{setup}"
    );
    assert_eq!(super::tools::mcp_name("enum#type find/all"), "enum_type_find_all");
}

#[test]
fn a_lazy_run_tool_write_is_refused_up_front_without_url_elicitation() {
    let bed = bed("max-direct-tools=\"2\"");
    let _entered = bed.ctx.enter();
    let (key, _) = ready(&bed);
    let arguments = json!({"name": "enum_update", "parameters": {"enumId": "MCP_E1", "description": "New"}});
    let (status, refused) = rpc(
        &bed,
        &key,
        "tools/call",
        json!({"name": "run_tool", "arguments": arguments, "_meta": meta(json!({"elicitation": {}}))}),
    );
    assert_eq!(
        (status, refused["error"]["code"].as_i64()),
        (400, Some(super::code::MISSING_CLIENT_CAPABILITY)),
        "{refused}"
    );
    for entity in ["moqui.agent.AgentRun", "moqui.agent.AgentApproval"] {
        let rows = bed.facade.find(entity).unwrap().disable_authz().list().unwrap();
        assert!(
            rows.values().is_empty(),
            "nothing is made, so the owner sees no card: {entity}"
        );
    }
    // a read through run_tool still runs for such a client
    let read = json!({"name": "enum_find", "parameters": {"enumTypeId": "McpType"}});
    let (_, found) = rpc(
        &bed,
        &key,
        "tools/call",
        json!({"name": "run_tool", "arguments": read, "_meta": meta(json!({}))}),
    );
    assert_eq!(found["result"]["isError"], false, "{found}");
    // with the capability, the write asks
    let asked = call(&bed, &key, "run_tool", arguments, json!({}));
    assert_eq!(asked["result"]["resultType"], "input_required", "{asked}");
}

#[test]
fn the_tarpit_counts_every_request_before_anything_is_built() {
    let bed = bed_with(
        "",
        r#"<artifact-execution-facade><artifact-execution type="AT_LLM" authz-enabled="true" tarpit-enabled="true"/>
            </artifact-execution-facade>"#,
    );
    let _entered = bed.ctx.enter();
    let (key, _) = ready(&bed);
    row(
        &bed.facade,
        "moqui.security.ArtifactGroup",
        &[("artifactGroupId", "McpToolCalls")],
    );
    row(
        &bed.facade,
        "moqui.security.ArtifactGroupMember",
        &[
            ("artifactGroupId", "McpToolCalls"),
            ("artifactName", "mcp:.*"),
            ("nameIsPattern", "Y"),
            ("artifactTypeEnumId", "AT_LLM"),
            ("inheritAuthz", "N"),
        ],
    );
    row(
        &bed.facade,
        "moqui.security.ArtifactTarpit",
        &[
            ("userGroupId", "ALL_USERS"),
            ("artifactGroupId", "McpToolCalls"),
            ("maxHitsCount", "3"),
            ("maxHitsDuration", "60"),
            ("tarpitDuration", "60"),
        ],
    );
    // unknown tools and lists count as calls do: the hit past the limit sets the lock, the next is refused
    for _ in 0..4 {
        let (_, unknown) = rpc(
            &bed,
            &key,
            "tools/call",
            json!({"name": "no_such_tool", "_meta": meta(json!({}))}),
        );
        assert_eq!(
            unknown["error"]["code"].as_i64(),
            Some(super::code::INVALID_PARAMS),
            "{unknown}"
        );
    }
    let (status, refused) = rpc(
        &bed,
        &key,
        "tools/call",
        json!({"name": "no_such_tool", "_meta": meta(json!({}))}),
    );
    assert_eq!(
        (status, refused["error"]["code"].as_i64()),
        (429, Some(super::code::TOO_MANY)),
        "{refused}"
    );
    for _ in 0..4 {
        assert_eq!(rpc(&bed, &key, "tools/list", json!({"_meta": meta(json!({}))})).0, 200);
    }
    let (status, _) = rpc(&bed, &key, "tools/list", json!({"_meta": meta(json!({}))}));
    assert_eq!(status, 429, "tools/list counts too");
}
