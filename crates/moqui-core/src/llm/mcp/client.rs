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

//! The MCP client (plans/LLM-A2A-Assist.md 4.16 "Client"; docs/MCP.md "Calling remote MCP servers"): what the MCP
//! client tools ([`crate::llm::tools::mcp_client`]) call a remote server with.
//!
//! A server is an `AgentMcpServer` row: its URL (an admin's, checked as [`crate::llm::remote`] checks every address),
//! its credential (or each user's own, `perUserAuth`, from `AgentMcpUserToken`) and its timeout. It is learned with
//! `server/discover` at protocol 2026-07-28, stateless: every request carries the version and the client's
//! capabilities in `_meta` and the `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers. A server that does not
//! discover (an error, 404, no 2026-07-28 among its versions) is spoken to with the legacy protocol instead:
//! `initialize` at 2025-11-25, `notifications/initialized`, and its `Mcp-Session-Id` on the requests after, made
//! again once when the server forgets the session. An answer may be JSON or an event stream; the response with the
//! request's id is taken, and a request of the server's own in a stream (legacy elicitation) is not answered, so such
//! a call fails.
//!
//! `tools/list` is read page by page and kept for its `ttlMs` (else the conf's `tools-ttl-ms`) per server, address and
//! credential; a server that fails is not asked again for a minute. `tools/call` answers complete, or
//! `input_required` (MRTR): a URL elicitation the user opens, and a `requestState` the call is retried with, with the
//! elicitation accepted, once they come back. A form elicitation is not filled. A body past the conf's
//! `max-result-bytes` is refused.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use moqui_lib::http::Method;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::context::ExecutionContext;
use crate::entity::{EntityFacade, EntityValue};
use crate::llm::provider::sse::SseParser;
use crate::llm::remote::{self, Address, Auth, Call};
use crate::llm::{millis_of, text_of};

/// The legacy protocol version a server that does not discover is initialized with.
pub const LEGACY_VERSION: &str = "2025-11-25";
/// The most tools read of a server, and pages.
const MAX_TOOLS: usize = 1000;
const MAX_PAGES: usize = 10;
/// How long a server that failed is not asked again.
const BREAKER: Duration = Duration::from_secs(60);
/// How long the tool list and the session of a server may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// The largest tool list, all its pages together (smaller in the tests, so one can pass it).
const LIST_CAP: u64 = if cfg!(test) { 64 * 1024 } else { 4 * 1024 * 1024 };
/// The shortest time a tool list is kept, whatever the server says (a `ttlMs` of 0 included), so a catalog built for
/// each `find_tools` does not list the server each time.
const MIN_TTL: Duration = Duration::from_secs(10);

/// A remote MCP server as its row has it, with the credential of the user calling.
#[derive(Clone, Debug)]
pub struct Server {
    pub server_id: String,
    pub name: String,
    pub description: Option<String>,
    pub address: Address,
    pub auth: Auth,
    pub timeout: Duration,
    pub allowed_agents: Option<String>,
    pub disabled: bool,
    pub per_user: bool,
}

impl Server {
    /// The server of a row, for `user_id`: a `perUserAuth` server takes the user's own token, which must be there.
    ///
    /// # Errors
    ///
    /// Why it cannot be called: its URL, its credential, no token of the user's.
    pub fn of(facade: &EntityFacade, row: &EntityValue, user_id: &str) -> Result<Self, String> {
        let server_id = text_of(row, "mcpServerId").unwrap_or_default();
        let url = text_of(row, "url").ok_or("it has no url")?;
        let address = Address::of(&url, text_of(row, "allowPrivateNetwork").as_deref() == Some("Y"))?;
        let per_user = text_of(row, "perUserAuth").as_deref() == Some("Y");
        let name = text_of(row, "serverName").unwrap_or_else(|| server_id.clone());
        let auth = if per_user {
            let token = crate::llm::run::quietly(|| {
                facade
                    .find(remote::MCP_USER_TOKEN)?
                    .eq("mcpServerId", Value::string(&server_id))
                    .eq("userId", Value::string(user_id))
                    .disable_authz()
                    .one()
            })
            .map_err(|error| format!("the user's token for {name} could not be read: {error}"))?
            .filter(|token| {
                token
                    .get("expireDate")
                    .and_then(millis_of)
                    .is_none_or(|at| at > crate::llm::run::now_millis())
            });
            match token {
                Some(token) => Auth::parse(text_of(&token, "tokenJson").as_deref())?,
                None => {
                    return Err(format!(
                        "{name} needs the user's own token, and none is set for them (or it expired)"
                    ))
                }
            }
        } else {
            Auth::parse(text_of(row, "authJson").as_deref())?
        };
        let conf_timeout = ExecutionContext::with_current(|ec| crate::llm::facade(ec.factory()).conf().mcp.call_timeout)
            .unwrap_or(60);
        Ok(Self {
            name,
            description: text_of(row, "description"),
            address,
            auth,
            timeout: Duration::from_secs(
                crate::llm::int_of(row, "timeoutSeconds")
                    .filter(|seconds| *seconds > 0)
                    .map_or(conf_timeout, i64::unsigned_abs),
            ),
            allowed_agents: text_of(row, "allowedAgents"),
            disabled: text_of(row, "disabled").as_deref() == Some("Y"),
            per_user,
            server_id,
        })
    }

    fn what(&self) -> String {
        format!("the MCP server {}", self.name)
    }

    /// Text of the server's own (an error it gave) as the model reads it: wrapped as untrusted.
    pub fn untrusted(&self, text: &str) -> String {
        crate::llm::prompt::untrusted(&format!("mcp:{}", self.server_id), text)
    }

    /// The key of what is kept of it: the server, its address and the credential.
    fn key(&self) -> String {
        format!("{}|{}|{}", self.server_id, self.address.url, self.auth.fingerprint())
    }
}

/// A tool a server lists.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTool {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Json,
}

/// A server's tools as listed, and how it was spoken to.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// `2026-07-28`, or `legacy <version>`.
    pub protocol: String,
    pub instructions: Option<String>,
    pub tools: Vec<RemoteTool>,
}

/// How a server is spoken to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Session {
    Modern,
    Legacy { version: String, session_id: Option<String> },
}

struct Kept {
    until: Instant,
    listing: Arc<Listing>,
}

#[derive(Default)]
struct State {
    lists: HashMap<String, Kept>,
    sessions: HashMap<String, Session>,
    failed: HashMap<String, (Instant, String)>,
}

/// The lists, sessions and failures kept for a factory: an extension of it, so a factory never sees another's (tests
/// run many at once, and a dropped test server's port may come back as another one's).
#[derive(Default)]
struct FactoryState(Mutex<State>);

/// The state kept when no execution context is current.
static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(work: impl FnOnce(&mut State) -> T) -> T {
    match ExecutionContext::with_current(|ec| ec.factory().extension(|_| FactoryState::default())) {
        Some(kept) => work(&mut moqui_lib::sync::lock(&kept.0)),
        None => work(moqui_lib::sync::lock(&STATE).get_or_insert_with(State::default)),
    }
}

/// Forget every kept tool list, session and failure of the factory: an admin changed a server.
pub fn forget_lists() {
    with_state(|state| *state = State::default());
}

fn conf() -> crate::llm::conf::McpConf {
    ExecutionContext::with_current(|ec| crate::llm::facade(ec.factory()).conf().mcp.clone())
        .unwrap_or_else(|| crate::llm::conf::LlmConf::default().mcp)
}

/// A JSON-RPC error of the server, or why the call did not get an answer.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcError {
    pub status: u16,
    pub code: Option<i64>,
    pub message: String,
}

impl RpcError {
    fn of(message: String) -> Self {
        Self {
            status: 0,
            code: None,
            message,
        }
    }
}

/// A header value as the transport takes it: plain visible ASCII as it is, else the Base64 sentinel.
fn header_value(text: &str) -> String {
    if text.chars().all(|ch| ch.is_ascii_graphic() || ch == ' ') && !text.starts_with(' ') && !text.ends_with(' ') {
        text.to_string()
    } else {
        format!("=?base64?{}?=", base64::engine::general_purpose::STANDARD.encode(text))
    }
}

/// The client's `_meta` of a 2026-07-28 request: the version, its capabilities (URL elicitations) and who it is.
fn meta() -> Json {
    json!({
        super::META_VERSION: super::PROTOCOL_VERSION,
        super::META_CLIENT_CAPS: {"elicitation": {"url": {}}},
        super::META_CLIENT_INFO: {"name": "moqui", "title": "Moqui", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// One request and its response's `result`, as the session speaks.
fn request(
    server: &Server,
    session: &Session,
    method: &str,
    params: Json,
    timeout: Duration,
    cap: u64,
) -> Result<(Json, remote::Answer), RpcError> {
    let what = server.what();
    let mut params = params;
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json, text/event-stream".to_string()),
    ];
    match session {
        Session::Modern => {
            params["_meta"] = meta();
            headers.push(("MCP-Protocol-Version".to_string(), super::PROTOCOL_VERSION.to_string()));
            headers.push(("Mcp-Method".to_string(), method.to_string()));
            if method == "tools/call" {
                let name = params.get("name").and_then(Json::as_str).unwrap_or_default();
                headers.push(("Mcp-Name".to_string(), header_value(name)));
            }
        }
        Session::Legacy { version, session_id } => {
            headers.push(("MCP-Protocol-Version".to_string(), version.clone()));
            if let Some(id) = session_id {
                headers.push(("Mcp-Session-Id".to_string(), id.clone()));
            }
        }
    }
    headers.extend(server.auth.headers());
    let id = next_id();
    let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let answer = remote::exchange(
        &server.address,
        Call {
            method: Method::Post,
            url: &server.address.url,
            headers,
            body: Some(body),
            timeout,
            cap,
            what: &what,
        },
    )
    .map_err(RpcError::of)?;
    let response = response_of(&answer, id).map_err(|message| RpcError {
        status: answer.status,
        code: None,
        message: format!("{what}: {message}"),
    })?;
    if let Some(error) = response.get("error") {
        return Err(RpcError {
            status: answer.status,
            code: error.get("code").and_then(Json::as_i64),
            message: format!(
                "{what} refused {method}: {}",
                server.untrusted(&crate::llm::prompt::one_line(
                    error.get("message").and_then(Json::as_str).unwrap_or("an error"),
                    300
                ))
            ),
        });
    }
    if !(200..300).contains(&answer.status) {
        return Err(RpcError {
            status: answer.status,
            code: None,
            message: format!("{what} answered {method} with {}", answer.status),
        });
    }
    let result = response.get("result").cloned().ok_or_else(|| RpcError {
        status: answer.status,
        code: None,
        message: format!("{what} answered {method} with no result"),
    })?;
    Ok((result, answer))
}

fn next_id() -> i64 {
    static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The JSON-RPC response of request `id` in an answer: its JSON body, or the event of a stream that carries it.
fn response_of(answer: &remote::Answer, id: i64) -> Result<Json, String> {
    if !answer.is_events() {
        let body: Json = serde_json::from_str(&answer.body)
            .map_err(|_| format!("the answer ({}) is not JSON", answer.status))?;
        return Ok(body);
    }
    let mut parser = SseParser::new();
    let mut frames = parser
        .push(answer.body.as_bytes())
        .map_err(|error| format!("its event stream does not read: {error}"))?;
    frames.extend(parser.finish());
    for frame in frames {
        let Ok(message) = serde_json::from_str::<Json>(&frame.data) else {
            continue;
        };
        if message.get("id").and_then(Json::as_i64) == Some(id)
            && (message.get("result").is_some() || message.get("error").is_some())
        {
            return Ok(message);
        }
        if message.get("method").is_some() && message.get("id").is_some() {
            // the server's own name for it, made plain
            let method = crate::llm::remote::name_part(&crate::llm::prompt::one_line(
                message.get("method").and_then(Json::as_str).unwrap_or_default(),
                60,
            ));
            return Err(format!(
                "it asked this client for {method} in its stream, which this client does not answer"
            ));
        }
    }
    Err("its event stream has no response to the request".to_string())
}

/// How to speak to the server: discovered at 2026-07-28, else initialized with the legacy protocol; kept per server.
fn session(server: &Server) -> Result<Session, String> {
    let key = server.key();
    if let Some(session) = with_state(|state| state.sessions.get(&key).cloned()) {
        return Ok(session);
    }
    let session = discover(server)?;
    with_state(|state| state.sessions.insert(key, session.clone()));
    Ok(session)
}

fn discover(server: &Server) -> Result<Session, String> {
    match request(server, &Session::Modern, "server/discover", json!({}), LIST_TIMEOUT, LIST_CAP) {
        Ok((result, _)) => {
            let speaks = result
                .get("supportedVersions")
                .and_then(Json::as_array)
                .is_some_and(|versions| versions.iter().any(|version| version.as_str() == Some(super::PROTOCOL_VERSION)));
            if speaks {
                return Ok(Session::Modern);
            }
            initialize(server)
        }
        // refused for its credential: the legacy protocol would not do better
        Err(error) if matches!(error.status, 401 | 403) => Err(error.message),
        // a transport failure is not a protocol answer
        Err(error) if error.status == 0 => Err(error.message),
        Err(_) => initialize(server),
    }
}

fn initialize(server: &Server) -> Result<Session, String> {
    let bare = Session::Legacy {
        version: LEGACY_VERSION.to_string(),
        session_id: None,
    };
    let params = json!({"protocolVersion": LEGACY_VERSION, "capabilities": {},
        "clientInfo": {"name": "moqui", "title": "Moqui", "version": env!("CARGO_PKG_VERSION")}});
    let (result, answer) =
        request(server, &bare, "initialize", params, LIST_TIMEOUT, LIST_CAP).map_err(|error| error.message)?;
    let version = result
        .get("protocolVersion")
        .and_then(Json::as_str)
        .unwrap_or(LEGACY_VERSION)
        .to_string();
    let session = Session::Legacy {
        version: version.clone(),
        session_id: answer.header("mcp-session-id").map(str::to_string),
    };
    // the notification is answered 202 with no body
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json, text/event-stream".to_string()),
        ("MCP-Protocol-Version".to_string(), version),
    ];
    if let Session::Legacy {
        session_id: Some(id), ..
    } = &session
    {
        headers.push(("Mcp-Session-Id".to_string(), id.clone()));
    }
    headers.extend(server.auth.headers());
    let what = server.what();
    let notified = remote::exchange(
        &server.address,
        Call {
            method: Method::Post,
            url: &server.address.url,
            headers,
            body: Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string()),
            timeout: LIST_TIMEOUT,
            cap: 64 * 1024,
            what: &what,
        },
    )?;
    if !(200..300).contains(&notified.status) {
        return Err(format!("{what} answered its initialized notification with {}", notified.status));
    }
    Ok(session)
}

/// A request in the server's session, made again once with a new session when a legacy server forgot it (404).
fn in_session(
    server: &Server,
    method: &str,
    params: &Json,
    timeout: Duration,
    cap: u64,
) -> Result<(Json, Session), String> {
    let first = session(server)?;
    match request(server, &first, method, params.clone(), timeout, cap) {
        Ok((result, _)) => Ok((result, first)),
        Err(error) if error.status == 404 && matches!(first, Session::Legacy { .. }) => {
            let key = server.key();
            with_state(|state| state.sessions.remove(&key));
            let again = session(server)?;
            request(server, &again, method, params.clone(), timeout, cap)
                .map(|(result, _)| (result, again))
                .map_err(|error| error.message)
        }
        Err(error) => Err(error.message),
    }
}

/// The server's tools: the list kept while it is fresh, else read now (every page); a server that failed is not
/// asked again for a minute.
///
/// # Errors
///
/// Why the tools could not be listed.
pub fn listing(server: &Server) -> Result<Arc<Listing>, String> {
    let key = server.key();
    let now = Instant::now();
    let kept = with_state(|state| {
        if let Some((at, why)) = state.failed.get(&key) {
            if now.duration_since(*at) < BREAKER {
                return Some(Err(why.clone()));
            }
        }
        state
            .lists
            .get(&key)
            .filter(|kept| kept.until > now)
            .map(|kept| Ok(Arc::clone(&kept.listing)))
    });
    if let Some(kept) = kept {
        return kept;
    }
    match list_now(server) {
        Ok((listing, ttl)) => {
            let listing = Arc::new(listing);
            with_state(|state| {
                state.failed.remove(&key);
                state.lists.insert(
                    key,
                    Kept {
                        until: now + ttl,
                        listing: Arc::clone(&listing),
                    },
                );
            });
            Ok(listing)
        }
        Err(why) => {
            // a refusal of the fence or a sim is no failure of the server's
            if !why.contains(crate::entity::fence::FENCE_ERROR) && !why.contains("in a sim") {
                with_state(|state| {
                    state.sessions.remove(&key);
                    state.failed.insert(key, (now, why.clone()));
                });
            }
            Err(why)
        }
    }
}

fn list_now(server: &Server) -> Result<(Listing, Duration), String> {
    let conf = conf();
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    let mut ttl = Duration::from_millis(conf.tools_ttl_ms);
    let mut protocol = String::new();
    let mut total: u64 = 0;
    for _ in 0..MAX_PAGES {
        let params = match &cursor {
            Some(cursor) => json!({"cursor": cursor}),
            None => json!({}),
        };
        let (result, session) =
            in_session(server, "tools/list", &params, LIST_TIMEOUT, LIST_CAP.saturating_sub(total))?;
        total += result.to_string().len() as u64;
        if total > LIST_CAP {
            return Err(format!("{}'s tool list is larger than {LIST_CAP} bytes", server.what()));
        }
        protocol = match &session {
            Session::Modern => super::PROTOCOL_VERSION.to_string(),
            Session::Legacy { version, .. } => format!("legacy {version}"),
        };
        if let Some(ms) = result.get("ttlMs").and_then(Json::as_u64) {
            ttl = Duration::from_millis(ms.min(3_600_000));
        }
        for tool in result.get("tools").and_then(Json::as_array).into_iter().flatten() {
            let Some(name) = tool.get("name").and_then(Json::as_str).filter(|name| !name.trim().is_empty()) else {
                continue;
            };
            if tools.len() >= MAX_TOOLS {
                break;
            }
            let text = |key: &str| tool.get(key).and_then(Json::as_str).unwrap_or_default().trim().to_string();
            tools.push(RemoteTool {
                name: name.to_string(),
                title: text("title"),
                description: text("description"),
                input_schema: tool.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"})),
            });
        }
        cursor = result
            .get("nextCursor")
            .and_then(Json::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string);
        if cursor.is_none() || tools.len() >= MAX_TOOLS {
            break;
        }
    }
    let instructions = None;
    let ttl = ttl.max(MIN_TTL);
    Ok((
        Listing {
            protocol,
            instructions,
            tools,
        },
        ttl,
    ))
}

/// What a call of a remote tool came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Its result: the content as text, its structured content, and whether the server says it is an error.
    Complete {
        text: String,
        structured: Option<Json>,
        is_error: bool,
    },
    /// The server needs the user first (MRTR): the link to open and its message, and the state the retry echoes.
    InputRequired {
        request_state: Option<String>,
        link: Option<String>,
        message: String,
        /// The key of the elicitation, which the retry answers.
        key: Option<String>,
    },
}

/// The text of a `CallToolResult`'s content: text as it is; an image, audio, a resource or a link named, never
/// fetched.
fn content_text(result: &Json) -> String {
    let mut out = Vec::new();
    for item in result.get("content").and_then(Json::as_array).into_iter().flatten() {
        let text = |key: &str| item.get(key).and_then(Json::as_str).unwrap_or_default().to_string();
        match item.get("type").and_then(Json::as_str).unwrap_or_default() {
            "text" => out.push(text("text")),
            "image" | "audio" => out.push(format!(
                "[{} {}, not shown]",
                item.get("type").and_then(Json::as_str).unwrap_or_default(),
                text("mimeType")
            )),
            "resource" => {
                let resource = item.get("resource").cloned().unwrap_or(Json::Null);
                match resource.get("text").and_then(Json::as_str) {
                    Some(body) => out.push(body.to_string()),
                    None => out.push(format!(
                        "[resource {}, not shown]",
                        resource.get("uri").and_then(Json::as_str).unwrap_or_default()
                    )),
                }
            }
            "resource_link" => out.push(format!("[link {} {}]", text("uri"), text("name")).trim_end().to_string()),
            _ => {}
        }
    }
    out.join("\n")
}

/// Call `tool` with `arguments`; a retry with the `request_state` an earlier `input_required` gave answers its
/// elicitation `key` as accepted.
///
/// # Errors
///
/// The call failed, or was refused, or asked for what this client does not give (a form).
pub fn call_tool(
    server: &Server,
    tool: &str,
    arguments: &Json,
    retry: Option<(&str, Option<&str>)>,
) -> Result<Outcome, String> {
    let conf = conf();
    let mut params = json!({"name": tool, "arguments": arguments});
    if let Some((state, key)) = retry {
        params["requestState"] = json!(state);
        if let Some(key) = key {
            params["inputResponses"] = json!({key: {"action": "accept"}});
        }
    }
    let (result, _) = in_session(server, "tools/call", &params, server.timeout, conf.max_result_bytes)?;
    if result.get("resultType").and_then(Json::as_str) == Some("input_required") {
        let request_state = result
            .get("requestState")
            .and_then(Json::as_str)
            .map(str::to_string);
        let requests = result.get("inputRequests").and_then(Json::as_object);
        let Some(requests) = requests.filter(|requests| !requests.is_empty()) else {
            // the same state again with no new request: still waiting on the one asked before
            return Ok(Outcome::InputRequired {
                request_state,
                link: None,
                message: String::new(),
                key: retry.and_then(|(_, key)| key).map(str::to_string),
            });
        };
        let (key, request) = requests.iter().next().expect("not empty");
        let params = request.get("params").cloned().unwrap_or(Json::Null);
        let is_url = request.get("method").and_then(Json::as_str) == Some("elicitation/create")
            && params.get("mode").and_then(Json::as_str) == Some("url");
        if !is_url || requests.len() > 1 {
            return Err(format!(
                "{} asked for input this client does not give (only one link to open is taken)",
                server.what()
            ));
        }
        let link = params
            .get("url")
            .and_then(Json::as_str)
            // https, or http only to an internal host of a row that allows a private network
            .filter(|url| remote::link_allowed(url, &server.address))
            .map(str::to_string);
        return Ok(Outcome::InputRequired {
            request_state,
            link,
            message: params
                .get("message")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            key: Some(key.clone()),
        });
    }
    Ok(Outcome::Complete {
        text: content_text(&result),
        structured: result.get("structuredContent").cloned().filter(|value| !value.is_null()),
        is_error: result.get("isError").and_then(Json::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_answer_gives_the_response_of_its_request_and_refuses_a_request_of_the_server() {
        let answer = |body: &str| remote::Answer {
            status: 200,
            headers: vec![("Content-Type".to_string(), "text/event-stream".to_string())],
            body: body.to_string(),
        };
        let found = response_of(
            &answer("event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"ok\":1}}\n\n"),
            7,
        )
        .unwrap();
        assert_eq!(found["result"]["ok"], 1);
        let asked = response_of(
            &answer("data: {\"jsonrpc\":\"2.0\",\"id\":\"s1\",\"method\":\"elicitation/create\",\"params\":{}}\n\n"),
            7,
        )
        .unwrap_err();
        assert!(asked.contains("elicitation_create"), "the server's method name, made plain: {asked}");
        assert_eq!(header_value("read_file"), "read_file");
        assert_eq!(header_value("lire_fichier_é"), "=?base64?bGlyZV9maWNoaWVyX8Op?=");
    }

    #[test]
    fn content_is_text_and_named_media_never_fetched() {
        let result = json!({"content": [{"type": "text", "text": "two files"},
            {"type": "image", "mimeType": "image/png", "data": "AAAA"},
            {"type": "resource", "resource": {"uri": "file:///a.txt", "text": "alpha"}},
            {"type": "resource_link", "uri": "file:///b.txt", "name": "b"}]});
        assert_eq!(
            content_text(&result),
            "two files\n[image image/png, not shown]\nalpha\n[link file:///b.txt b]"
        );
    }
}
