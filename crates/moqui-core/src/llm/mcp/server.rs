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

//! The Streamable HTTP transport of the MCP server (MCP 2026-07-28 "Streamable HTTP"), its checks, `server/discover`
//! and the public documents: the Server Card, the AI Catalog and the setup prompt.

use std::sync::Arc;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Map, Value as Json};

use super::{code, Answer, Caller, HttpRequest, Reply};
use crate::context::ExecutionContextFactory;
use crate::llm::a2a::Login;

/// The methods whose `Mcp-Name` header must name `params.name` (or `params.uri`).
const NAMED: [&str; 3] = ["tools/call", "resources/read", "prompts/get"];

/// The Server Card's schema.
const CARD_SCHEMA: &str = "https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json";

/// A JSON-RPC error with the HTTP status it goes with.
#[derive(Clone, Debug, PartialEq)]
pub struct McpError {
    pub status: u16,
    pub code: i64,
    pub message: String,
    pub data: Option<Json>,
    pub headers: Vec<(String, String)>,
}

impl McpError {
    pub fn new(status: u16, code: i64, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            data: None,
            headers: Vec::new(),
        }
    }

    #[must_use]
    pub fn data(mut self, data: Json) -> Self {
        self.data = Some(data);
        self
    }

    #[must_use]
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    /// An error of the call's parameters, as a JSON-RPC error of a request that was read (HTTP 200).
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(200, code::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(200, code::INTERNAL, message)
    }

    fn mismatch(message: impl Into<String>) -> Self {
        Self::new(400, code::HEADER_MISMATCH, message)
    }

    /// The response, with the request's `id` (none when it could not be read).
    pub fn reply(&self, id: Option<&Json>) -> Reply {
        let mut error = json!({"code": self.code, "message": self.message});
        if let Some(data) = &self.data {
            error["data"] = data.clone();
        }
        let mut body = json!({"jsonrpc": "2.0", "error": error});
        if let Some(id) = id {
            body["id"] = id.clone();
        }
        let mut reply = Reply::json(self.status, &body);
        for (name, value) in &self.headers {
            reply = reply.with_header(name, value.clone());
        }
        reply
    }
}

/// A result's response: the result with `_meta.serverInfo`.
pub fn result_reply(id: &Json, mut result: Json) -> Reply {
    if let Some(object) = result.as_object_mut() {
        let meta = object.entry("_meta").or_insert_with(|| json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.insert(super::META_SERVER_INFO.to_string(), super::server_info());
        }
    }
    Reply::json(200, &json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// What a request's `_meta` says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub version: String,
    pub client_caps: Json,
    pub client_info: Option<Json>,
}

impl Meta {
    /// Whether the client takes URL elicitations: `elicitation.url` in its capabilities (an empty `elicitation` is
    /// form mode only).
    pub fn url_elicitation(&self) -> bool {
        self.client_caps
            .get("elicitation")
            .and_then(|elicitation| elicitation.get("url"))
            .is_some_and(|url| !url.is_null())
    }

    /// The client's name, for the conversation a write is shown in.
    pub fn client_name(&self) -> String {
        self.client_info
            .as_ref()
            .and_then(|info| info.get("title").or_else(|| info.get("name")))
            .and_then(Json::as_str)
            .map(|name| crate::llm::prompt::one_line(name, 60))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "An MCP client".to_string())
    }
}

/// One JSON-RPC request, read and checked.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: Json,
    pub method: String,
    pub params: Json,
    pub meta: Meta,
}

/// A header value as the client sent it, the Base64 sentinel (`=?base64?…?=`) decoded.
///
/// # Errors
///
/// When the sentinel does not decode to UTF-8.
pub fn header_value(raw: &str) -> Result<String, String> {
    match raw.strip_prefix("=?base64?").and_then(|rest| rest.strip_suffix("?=")) {
        Some(encoded) => STANDARD
            .decode(encoded.as_bytes())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| "a Base64 header value does not decode".to_string()),
        None => Ok(raw.to_string()),
    }
}

/// Whether a media type list (`Accept`) takes `wanted` (`type/subtype`), by itself or a wildcard.
fn accepts(header: &str, wanted: &str) -> bool {
    let (kind, _) = wanted.split_once('/').unwrap_or((wanted, ""));
    header.split(',').any(|item| {
        let media = item.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        media == wanted || media == "*/*" || media == format!("{kind}/*")
    })
}

/// The host of an origin or URL: no scheme, path or port, lower case.
fn host_of(text: &str) -> String {
    let text = text.trim().to_ascii_lowercase();
    let rest = text.split_once("://").map_or(text.as_str(), |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    if let Some(bracketed) = authority.strip_prefix('[') {
        return format!("[{}]", bracketed.split(']').next().unwrap_or_default());
    }
    authority.split(':').next().unwrap_or_default().to_string()
}

/// Whether a host is this machine's own name: `localhost`, a `*.localhost` name, or a loopback address (an IP address
/// parsed as one, so `127.attacker.example` is not).
fn is_local(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let bare = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

/// The host the server is configured at: the conf's `public-url`, else the webroot's URL.
fn configured_host(factory: &ExecutionContextFactory, https: bool) -> String {
    let public = crate::llm::facade(factory).conf().mcp.public_url.clone();
    host_of(&public.unwrap_or_else(|| factory.webapp_root_url_for("webroot", Some(https))))
}

/// The transport's checks of a request that has not been parsed: the method, the `Origin`, the content types.
///
/// # Errors
///
/// The refusal to answer.
pub fn check_transport(factory: &ExecutionContextFactory, request: &HttpRequest) -> Result<(), McpError> {
    if !request.method.eq_ignore_ascii_case("POST") {
        return Err(
            McpError::new(405, code::INVALID_REQUEST, "the MCP endpoint takes POST only").header("allow", "POST"),
        );
    }
    if let Some(origin) = request
        .header("origin")
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
    {
        // never the request's own Host, which a DNS rebinding page sets to its own name: this machine, the server's
        // configured host (public-url, else the webroot's), a believed proxy's forwarded host, or an allowed origin
        // (the webroot's allow-origins)
        let configured = configured_host(factory, request.origin.https);
        let forwarded = request
            .origin
            .believed
            .then(|| request.origin.forwarded_host.clone())
            .flatten();
        let allowed = is_local(&host_of(origin))
            || crate::web::origin_allowed_by(Some(factory), Some(origin), Some(&configured))
            || forwarded
                .as_deref()
                .is_some_and(|host| crate::web::origin_allowed_by(Some(factory), Some(origin), Some(host)));
        if !allowed {
            return Err(McpError::new(
                403,
                code::INVALID_REQUEST,
                format!("the Origin {origin} is not allowed"),
            ));
        }
    }
    let content_type = request.header("content-type").unwrap_or_default();
    if !content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .eq_ignore_ascii_case("application/json")
    {
        return Err(McpError::new(
            415,
            code::INVALID_REQUEST,
            "the body must be application/json",
        ));
    }
    if let Some(accept) = request.header("accept").filter(|accept| !accept.trim().is_empty()) {
        if !accepts(accept, "application/json") {
            return Err(McpError::new(
                406,
                code::INVALID_REQUEST,
                "Accept must take application/json (and text/event-stream)",
            ));
        }
    }
    Ok(())
}

/// What the body is.
#[derive(Debug)]
pub enum Body {
    Request(Request),
    /// A notification: accepted, nothing answered (202).
    Notification,
}

/// Read the body and check it against the headers: one JSON-RPC request (no batch, no response), its
/// `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers matching it, its `_meta` with the version and the
/// client's capabilities, and a version this server takes.
///
/// # Errors
///
/// The refusal, with the request's id when it was read.
pub fn read_body(
    factory: &ExecutionContextFactory,
    request: &HttpRequest,
) -> Result<Body, Box<(Option<Json>, McpError)>> {
    let body: Json = serde_json::from_slice(&request.body).map_err(|error| {
        (
            None,
            McpError::new(400, code::PARSE, format!("the body is not JSON: {error}")),
        )
    })?;
    let Some(object) = body.as_object() else {
        let message = if body.is_array() {
            "a batch is not taken: send one JSON-RPC message per request"
        } else {
            "the body must be a JSON-RPC request object"
        };
        return Err(Box::new((None, McpError::new(400, code::INVALID_REQUEST, message))));
    };
    let id = object.get("id").cloned();
    let invalid = |message: &str| Box::new((id.clone(), McpError::new(400, code::INVALID_REQUEST, message)));
    if object.get("jsonrpc").and_then(Json::as_str) != Some("2.0") {
        return Err(invalid("jsonrpc must be \"2.0\""));
    }
    let Some(method) = object.get("method").and_then(Json::as_str) else {
        if object.contains_key("result") || object.contains_key("error") {
            return Err(invalid("a client sends no JSON-RPC responses"));
        }
        return Err(invalid("the request has no method"));
    };
    let Some(id) = id.clone() else {
        // a notification: this version defines none over HTTP; one is accepted and dropped
        return Ok(Body::Notification);
    };
    if !(id.is_string() || id.is_i64() || id.is_u64()) {
        return Err(Box::new((
            None,
            McpError::new(400, code::INVALID_REQUEST, "the id must be a string or an integer"),
        )));
    }
    let failed = |error: McpError| Box::new((Some(id.clone()), error));
    let params = match object.get("params") {
        None | Some(Json::Null) => json!({}),
        Some(params @ Json::Object(_)) => params.clone(),
        Some(_) => {
            return Err(failed(McpError::new(
                400,
                code::INVALID_PARAMS,
                "params must be an object",
            )))
        }
    };
    // the mirrored headers, against the body: the body is the truth, a difference is refused
    let header_version = request
        .header("mcp-protocol-version")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| failed(McpError::mismatch("the MCP-Protocol-Version header is required")))?;
    let header_method = request
        .header("mcp-method")
        .ok_or_else(|| failed(McpError::mismatch("the Mcp-Method header is required")))?;
    if header_method != method {
        return Err(failed(McpError::mismatch(format!(
            "Header mismatch: Mcp-Method header value '{header_method}' does not match body value '{method}'"
        ))));
    }
    if NAMED.contains(&method) {
        let body_name = params
            .get("name")
            .or_else(|| params.get("uri"))
            .and_then(Json::as_str)
            .unwrap_or_default();
        let raw = request
            .header("mcp-name")
            .ok_or_else(|| failed(McpError::mismatch("the Mcp-Name header is required for this method")))?;
        let header_name = header_value(raw).map_err(|why| failed(McpError::mismatch(why)))?;
        if header_name != body_name {
            return Err(failed(McpError::mismatch(format!(
                "Header mismatch: Mcp-Name header value '{header_name}' does not match body value '{body_name}'"
            ))));
        }
    }
    let meta = params.get("_meta").and_then(Json::as_object);
    let Some(version) = meta
        .and_then(|meta| meta.get(super::META_VERSION))
        .and_then(Json::as_str)
    else {
        return Err(failed(McpError::new(
            400,
            code::INVALID_PARAMS,
            format!("params._meta must name {}", super::META_VERSION),
        )));
    };
    let Some(client_caps) = meta
        .and_then(|meta| meta.get(super::META_CLIENT_CAPS))
        .filter(|caps| caps.is_object())
        .cloned()
    else {
        return Err(failed(McpError::new(
            400,
            code::INVALID_PARAMS,
            format!("params._meta must hold {}, an object", super::META_CLIENT_CAPS),
        )));
    };
    if header_version != version {
        return Err(failed(McpError::mismatch(format!(
            "Header mismatch: MCP-Protocol-Version header value '{header_version}' does not match body value \
             '{version}'"
        ))));
    }
    let client_info = meta.and_then(|meta| meta.get(super::META_CLIENT_INFO)).cloned();
    let supported = super::supported_versions(factory);
    if !supported.iter().any(|known| known == version) {
        return Err(failed(
            McpError::new(400, code::UNSUPPORTED_VERSION, "Unsupported protocol version")
                .data(json!({"supported": supported, "requested": version})),
        ));
    }
    let meta = Meta {
        version: version.to_string(),
        client_caps,
        client_info,
    };
    Ok(Body::Request(Request {
        id,
        method: method.to_string(),
        params,
        meta,
    }))
}

/// Log the request's user in.
///
/// # Errors
///
/// 401 (with `WWW-Authenticate`) without valid credentials, 403 over plain HTTP or without `McpServerUse` and
/// `AgentUse`.
pub fn authenticate(factory: &ExecutionContextFactory, request: &HttpRequest) -> Result<Caller, McpError> {
    let unauthenticated = |message: &str| {
        McpError::new(401, code::UNAUTHENTICATED, message).header("www-authenticate", super::www_authenticate())
    };
    let user_id = match crate::llm::a2a::log_in(request, true, true) {
        Login::User(user_id) => user_id,
        Login::None => {
            return Err(unauthenticated(
                "credentials are required: a Bearer login key, or Basic with an app password",
            ))
        }
        Login::Insecure => {
            return Err(McpError::new(
                403,
                code::FORBIDDEN,
                "credentials over plain HTTP are refused; use HTTPS",
            ))
        }
        Login::Invalid => return Err(unauthenticated("the credentials are not valid")),
    };
    let permitted = |permission: &str| crate::user::has_permission(permission).unwrap_or(false);
    if !permitted("McpServerUse") || !permitted("AgentUse") {
        return Err(McpError::new(
            403,
            code::FORBIDDEN,
            "this needs the McpServerUse and AgentUse permissions",
        ));
    }
    Ok(Caller {
        username: crate::user::current_username().ok().flatten().unwrap_or_default(),
        user_id,
        base_url: super::base_url(factory, &request.origin),
    })
}

/// Serve a request to `/mcp`, on a request worker. 404 while the server is off.
pub fn serve(factory: &Arc<ExecutionContextFactory>, request: &HttpRequest) -> Answer {
    if !super::enabled(factory) {
        return Answer::Reply(McpError::new(404, code::METHOD_NOT_FOUND, "the MCP server is off").reply(None));
    }
    if let Err(error) = check_transport(factory, request) {
        return Answer::Reply(error.reply(None));
    }
    let rpc = match read_body(factory, request) {
        Ok(Body::Request(rpc)) => rpc,
        Ok(Body::Notification) => {
            return Answer::Reply(Reply {
                status: 202,
                headers: Vec::new(),
                body: String::new(),
            })
        }
        Err(refused) => return Answer::Reply(refused.1.reply(refused.0.as_ref())),
    };
    // the server's identity and versions are public; everything else is the caller's
    if rpc.method == "server/discover" {
        return Answer::Reply(result_reply(&rpc.id, discover(factory)));
    }
    if !matches!(rpc.method.as_str(), "tools/list" | "tools/call") {
        return Answer::Reply(
            McpError::new(404, code::METHOD_NOT_FOUND, format!("Method not found: {}", rpc.method))
                .reply(Some(&rpc.id)),
        );
    }
    let caller = match authenticate(factory, request) {
        Ok(caller) => caller,
        Err(error) => return Answer::Reply(error.reply(Some(&rpc.id))),
    };
    match rpc.method.as_str() {
        "tools/list" => Answer::Reply(match super::tools::list(factory, &caller, &rpc.params) {
            Ok(result) => result_reply(&rpc.id, result),
            Err(error) => error.reply(Some(&rpc.id)),
        }),
        _ => {
            let id = rpc.id.clone();
            match super::tools::call(factory, &caller, &rpc.params, &rpc.meta) {
                Ok(super::tools::Called::Now(result)) => Answer::Reply(result_reply(&id, result)),
                Ok(super::tools::Called::Wait(wait)) => Answer::Later {
                    user_id: caller.user_id.clone(),
                    finish: Box::new(move || result_reply(&id, wait())),
                },
                Err(error) => Answer::Reply(error.reply(Some(&id))),
            }
        }
    }
}

/// The connect-time instructions (`framework/prompt/mcp.md`); empty when the file cannot be read, logged.
pub fn instructions(factory: &ExecutionContextFactory) -> String {
    let path = factory.framework_dir().join("prompt").join("mcp.md");
    match std::fs::read_to_string(&path) {
        Ok(text) => crate::llm::prompt::prompt_text(&text),
        Err(error) => {
            tracing::error!("the MCP instructions {} could not be read: {error}", path.display());
            String::new()
        }
    }
}

/// `server/discover`: the versions, the capabilities (tools only) and the instructions, the same for every caller.
pub fn discover(factory: &ExecutionContextFactory) -> Json {
    let mut result = Map::new();
    result.insert("resultType".to_string(), json!("complete"));
    result.insert(
        "supportedVersions".to_string(),
        json!(super::supported_versions(factory)),
    );
    result.insert("capabilities".to_string(), json!({"tools": {}}));
    let instructions = instructions(factory);
    if !instructions.is_empty() {
        result.insert("instructions".to_string(), json!(instructions));
    }
    result.insert(
        "ttlMs".to_string(),
        json!(crate::llm::facade(factory).conf().mcp.tools_ttl_ms),
    );
    result.insert("cacheScope".to_string(), json!("public"));
    Json::Object(result)
}

/// The Server Card (the Server Card extension's `schema.ts`): the server's identity and its one remote, public.
pub fn server_card(base_url: &str, versions: &[String]) -> Json {
    json!({
        "$schema": CARD_SCHEMA,
        "name": "org.moqui/moqui",
        "version": env!("CARGO_PKG_VERSION"),
        "title": "Moqui",
        "description": "The apps of a Moqui server as tools: screens, search, data and the services it exposes",
        "websiteUrl": "https://www.moqui.org",
        "remotes": [{
            "type": "streamable-http",
            "url": format!("{base_url}/mcp"),
            "headers": [{"name": "Authorization", "description":
                "Bearer and a Moqui login key, or Basic with your username and an app password", "isRequired": true,
                "isSecret": true}],
            "supportedProtocolVersions": versions,
        }],
        "_meta": {"org.moqui/setup": format!("{base_url}/mcp/setup")},
    })
}

/// The AI Catalog that names the Server Card, for domain-level discovery.
pub fn ai_catalog(base_url: &str) -> Json {
    let host = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split(['/', ':'])
        .next()
        .unwrap_or("localhost");
    json!({
        "specVersion": "1.0",
        "entries": [{"identifier": format!("urn:air:{host}:mcp:moqui"), "type": "application/mcp-server-card+json",
            "url": format!("{base_url}/mcp/server-card")}],
    })
}

/// The setup prompt for coding agents: how to connect to this server.
pub fn setup_prompt(base_url: &str, version: &str) -> String {
    format!(
        "# Connect to the Moqui MCP server\n\n\
         - Endpoint: `{base_url}/mcp`, Streamable HTTP, protocol version `{version}` (no initialize; call \
         `server/discover` first).\n\
         - Auth: `Authorization: Bearer <login key>` (a Moqui login key; make one under My Account), or HTTP Basic \
         with your username and an app password. HTTPS only, except from the same machine.\n\
         - Your account needs the McpServerUse and AgentUse permissions; you see only the tools your account may \
         use.\n\
         - Writes wait for your approval in Moqui: the call answers input_required with a link (a URL elicitation). \
         Open it while signed in to Moqui, decide there, then retry the call with the requestState it gave.\n\
         - Example client configuration (JSON):\n\n\
         ```json\n{{\"mcpServers\": {{\"moqui\": {{\"type\": \"http\", \"url\": \"{base_url}/mcp\",\n  \
         \"headers\": {{\"Authorization\": \"Bearer ${{MOQUI_LOGIN_KEY}}\"}}}}}}}}\n```\n"
    )
}

/// A public document: `body` with its media type, cacheable for an hour, readable from any origin.
pub fn public_reply(body: String, content_type: &str) -> Reply {
    let tag = moqui_lib::json::canonical_sha256(&json!(body));
    Reply {
        status: 200,
        headers: vec![
            ("content-type".to_string(), content_type.to_string()),
            ("cache-control".to_string(), "public, max-age=3600".to_string()),
            ("etag".to_string(), format!("\"{}\"", &tag[..tag.len().min(32)])),
            ("access-control-allow-origin".to_string(), "*".to_string()),
            ("access-control-allow-methods".to_string(), "GET".to_string()),
            (
                "access-control-allow-headers".to_string(),
                "Content-Type, If-None-Match".to_string(),
            ),
            ("access-control-expose-headers".to_string(), "ETag".to_string()),
        ],
        body,
    }
}

/// A public document of the server by its path (`server-card`, `mcp.json`, `ai-catalog.json`, `setup`); 404 while
/// the server is off or for another path.
pub fn document(factory: &ExecutionContextFactory, which: &str, origin: &super::Origin) -> Reply {
    if !super::enabled(factory) {
        return McpError::new(404, code::METHOD_NOT_FOUND, "the MCP server is off").reply(None);
    }
    let base = super::base_url(factory, origin);
    let versions = super::supported_versions(factory);
    match which {
        "server-card" => public_reply(
            server_card(&base, &versions).to_string(),
            "application/mcp-server-card+json",
        ),
        "mcp.json" => public_reply(server_card(&base, &versions).to_string(), "application/json"),
        "ai-catalog.json" => public_reply(ai_catalog(&base).to_string(), "application/ai-catalog+json"),
        "setup" => public_reply(
            setup_prompt(&base, versions.first().map_or(super::PROTOCOL_VERSION, String::as_str)),
            "text/markdown; charset=utf-8",
        ),
        _ => McpError::new(404, code::METHOD_NOT_FOUND, "no such document").reply(None),
    }
}
