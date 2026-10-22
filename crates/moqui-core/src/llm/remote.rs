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

//! The outbound side of the A2A and MCP clients (plans/LLM-A2A-Assist.md 4.15, 4.16, 4.19; phase 13.12): the addresses
//! they may call, their credentials, and the one HTTP path both take, with its SSRF limits.
//!
//! - **Addresses** come only from admin rows (`AgentRemoteA2a.cardUrl`, `AgentMcpServer.url`), never from a model or a
//!   remote: an absolute http(s) URL with no user info or fragment, https unless the row allows a private network, and
//!   no internal address unless it does ([`Address::of`]). A call goes only to its row's origin: an interface URL a
//!   card names on another origin is refused ([`exchange`]).
//! - **At connect time** the client of a row that does not allow a private network refuses every internal address, a
//!   name's included: the check is in the resolver the connection uses, so a name that resolves to a public address
//!   for a check and an internal one for the connect (DNS rebinding) cannot get past it; an address written as a
//!   decimal, hex or IPv4-mapped IPv6 number is read as the URL parser reads it. No redirect is followed (a 3xx is an
//!   error), a connection has 5 s to open, the whole exchange has the row's timeout, and a body past the caller's cap
//!   is refused.
//! - **Sim and fence**: no call goes out in a sim, and none under the read fence unless a remote permit for that
//!   origin is above it ([`crate::entity::fence::enter_remote`]: a remote tool an admin marked READ, or the tool list
//!   the catalog reads); a call checks both before it connects ([`gate`]).
//! - **Credentials** are kept encrypted in the rows' `authJson` and never shown or sent anywhere but their own origin
//!   ([`Auth`]).

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::http::{Method, OpenRequest, RestClient};
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::{json, Value as Json};

use crate::context::ExecutionContext;
use crate::entity::{fence, EntityFacade, EntityValue};
use crate::llm::provider::sse::{Frame, SseParser};
use crate::llm::text_of;
use crate::service::rust_services::{rust_service, RustServiceFn};

/// The longest URL a row may have.
pub const MAX_URL: usize = 2000;
/// The most any answer's body is read to; a caller's cap is at most this.
const BODY_CAP: u64 = 8 * 1024 * 1024;
/// How long a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) const REMOTE_A2A: &str = "moqui.agent.AgentRemoteA2a";
pub(crate) const MCP_SERVER: &str = "moqui.agent.AgentMcpServer";
pub(crate) const MCP_SERVER_TOOL: &str = "moqui.agent.AgentMcpServerTool";
pub(crate) const MCP_USER_TOKEN: &str = "moqui.agent.AgentMcpUserToken";
pub(crate) const REMOTE_TASK: &str = "moqui.agent.AgentRemoteTask";

/// How a client signs in to a remote: `authJson` as `{"bearer": key}`, `{"basic": {username, password}}` or
/// `{"header": {name, value}}`.
#[derive(Clone, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    None,
    Bearer(String),
    Basic(String, String),
    Header(String, String),
}

impl std::fmt::Debug for Auth {
    // never the secret
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind())
    }
}

impl Auth {
    /// The credential of an `authJson`: none for none.
    ///
    /// # Errors
    ///
    /// What is wrong with it (not JSON, an unknown kind, a part missing); never its value.
    pub fn parse(text: Option<&str>) -> Result<Self, String> {
        let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
            return Ok(Auth::None);
        };
        let value: Json = serde_json::from_str(text).map_err(|_| "its authJson is not JSON".to_string())?;
        let string = |value: Option<&Json>| {
            value
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        if let Some(key) = string(value.get("bearer")) {
            return Ok(Auth::Bearer(key));
        }
        if let Some(basic) = value.get("basic") {
            return match (string(basic.get("username")), basic.get("password").and_then(Json::as_str)) {
                (Some(username), Some(password)) => Ok(Auth::Basic(username, password.to_string())),
                _ => Err("its basic authJson needs username and password".to_string()),
            };
        }
        if let Some(header) = value.get("header") {
            return match (string(header.get("name")), string(header.get("value"))) {
                (Some(name), Some(value))
                    if name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
                        && !matches!(name.to_ascii_lowercase().as_str(), "host" | "content-length" | "cookie") =>
                {
                    Ok(Auth::Header(name, value))
                }
                _ => Err("its header authJson needs a plain header name and a value".to_string()),
            };
        }
        if value.as_object().is_some_and(serde_json::Map::is_empty) {
            return Ok(Auth::None);
        }
        Err("its authJson is not bearer, basic or header".to_string())
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Auth::None => "none",
            Auth::Bearer(_) => "bearer",
            Auth::Basic(..) => "basic",
            Auth::Header(..) => "header",
        }
    }

    /// The headers that carry it.
    pub fn headers(&self) -> Vec<(String, String)> {
        use base64::Engine;
        match self {
            Auth::None => Vec::new(),
            Auth::Bearer(key) => vec![("Authorization".to_string(), format!("Bearer {key}"))],
            Auth::Basic(username, password) => vec![(
                "Authorization".to_string(),
                format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
                ),
            )],
            Auth::Header(name, value) => vec![(name.clone(), value.clone())],
        }
    }

    /// The `authJson` of it.
    pub fn to_json(&self) -> Json {
        match self {
            Auth::None => json!({}),
            Auth::Bearer(key) => json!({"bearer": key}),
            Auth::Basic(username, password) => json!({"basic": {"username": username, "password": password}}),
            Auth::Header(name, value) => json!({"header": {"name": name, "value": value}}),
        }
    }

    /// A short hash of it, for a cache key: two credentials never share a cached answer.
    pub fn fingerprint(&self) -> String {
        moqui_lib::json::canonical_sha256(&self.to_json())
            .chars()
            .take(16)
            .collect()
    }
}

/// `scheme://host:port` of an absolute http(s) URL, lower case, the port written out; `None` for anything else.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, host) = crate::llm::a2a::push::scheme_host(url)?;
    let rest = url.trim().split_once("://")?.1;
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let port = if authority.starts_with('[') {
        authority
            .rsplit_once("]:")
            .map(|(_, port)| port.to_string())
    } else {
        authority.rsplit_once(':').map(|(_, port)| port.to_string())
    };
    let port = match port.filter(|port| !port.is_empty()) {
        Some(port) => port.parse::<u16>().ok()?,
        None if scheme == "https" => 443,
        None => 80,
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host
    };
    Some(format!("{scheme}://{host}:{port}"))
}

/// Check a URL an admin row gives: absolute http(s) with no user info or fragment, at most [`MAX_URL`] long; unless
/// `allow_private`, https and not an internal address written out (a decimal, hex or IPv4-mapped one included) or
/// `localhost`. A row that allows a private network may name an internal host, and only such a host over http (an
/// internal address written out, `localhost`, or a name that resolves only to internal addresses), so a credential never
/// crosses the internet in cleartext. A name is checked again when it is resolved for the connect.
///
/// # Errors
///
/// What does not fit, naming no secret.
pub fn check_url(url: &str, allow_private: bool) -> Result<(), String> {
    let url = url.trim();
    if url.len() > MAX_URL {
        return Err(format!("the URL is longer than {MAX_URL} characters"));
    }
    let Some((scheme, host)) = crate::llm::a2a::push::scheme_host(url) else {
        return Err("the URL must be an absolute http or https URL with no user info or fragment".to_string());
    };
    if origin_of(url).is_none() {
        return Err("the URL's port is not a number".to_string());
    }
    if allow_private {
        if scheme != "https" && !internal_host(url, &host) {
            return Err(format!(
                "{} is not on a private network, so it must be https",
                moqui_lib::http::redacted(url)
            ));
        }
        return Ok(());
    }
    if scheme != "https" {
        return Err("the URL must be https; http is only for a private network the row allows".to_string());
    }
    if named_internal(url, &host) {
        return Err(format!(
            "{} is an internal address, which the row does not allow (allowPrivateNetwork)",
            moqui_lib::http::redacted(url)
        ));
    }
    Ok(())
}

/// Whether the host of `url` is internal as written: an internal address, `localhost` or a `*.localhost` name.
fn named_internal(url: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.');
    host == "localhost"
        || host.ends_with(".localhost")
        || moqui_lib::http::host_address(url).is_some_and(moqui_lib::http::internal_address)
}

/// Whether the host of `url` is internal: as written, or a name every address of which is internal.
fn internal_host(url: &str, host: &str) -> bool {
    if named_internal(url, host) {
        return true;
    }
    if moqui_lib::http::host_address(url).is_some() {
        return false;
    }
    use std::net::ToSocketAddrs;
    let addresses: Vec<std::net::SocketAddr> = (host, 0).to_socket_addrs().map(Iterator::collect).unwrap_or_default();
    !addresses.is_empty() && addresses.iter().all(|address| moqui_lib::http::internal_address(address.ip()))
}

/// Whether a link a remote gives may be shown to the user, by the remote's row: https, or http only on a row that
/// allows a private network and to an internal host.
pub fn link_allowed(link: &str, address: &Address) -> bool {
    let Some((scheme, host)) = crate::llm::a2a::push::scheme_host(link) else {
        return false;
    };
    link.len() <= MAX_URL && (scheme == "https" || (address.allow_private && internal_host(link, &host)))
}

/// The host (and port, when it is written) of a link, as the user is shown it.
pub fn link_host(link: &str) -> String {
    link.trim()
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Where a row lets a client call: its URL, the URL's origin and whether a private network is allowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub url: String,
    pub origin: String,
    pub allow_private: bool,
}

impl Address {
    /// The address of a row's URL, checked ([`check_url`]).
    ///
    /// # Errors
    ///
    /// Why the URL may not be called.
    pub fn of(url: &str, allow_private: bool) -> Result<Self, String> {
        check_url(url, allow_private)?;
        let url = url.trim().to_string();
        let origin = origin_of(&url).ok_or("the URL has no origin")?;
        Ok(Self {
            url,
            origin,
            allow_private,
        })
    }

    /// Whether `url` is on this address's origin.
    pub fn same_origin(&self, url: &str) -> bool {
        origin_of(url).is_some_and(|origin| origin == self.origin)
    }
}

/// Whether a call to `address` may go out now: never in a sim, and under the read fence only with a remote permit for
/// its origin above it.
///
/// # Errors
///
/// Why not, as the model and the log read it.
pub(crate) fn gate(address: &Address, what: &str) -> Result<(), String> {
    let sim = ExecutionContext::with_current(|ec| ec.factory().entity().is_some_and(|facade| facade.sim_active()))
        .unwrap_or(false);
    if sim {
        return Err(format!("{what} is not called in a sim, which reaches nothing outside"));
    }
    fence::check_remote(&address.origin, what).map_err(|error| error.message().to_string())
}

/// The clients of the calls: one that refuses internal addresses at connect time, one for rows that allow a private
/// network. Neither follows a redirect.
pub(crate) fn client(allow_private: bool) -> &'static RestClient {
    static PUBLIC: OnceLock<RestClient> = OnceLock::new();
    static PRIVATE: OnceLock<RestClient> = OnceLock::new();
    let slot = if allow_private { &PRIVATE } else { &PUBLIC };
    slot.get_or_init(|| {
        RestClient::new()
            .deny_internal_addresses(!allow_private)
            .redirects(0, false)
            .connect_timeout(CONNECT_TIMEOUT)
            .max_response_bytes(BODY_CAP)
    })
}

/// An answer: its status, headers (names as sent) and body text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn is_events(&self) -> bool {
        self.header("content-type")
            .is_some_and(|kind| kind.to_ascii_lowercase().starts_with("text/event-stream"))
    }
}

/// What one exchange is: the method, the URL (on the address's origin), the headers and body, its timeout and the
/// most body read.
pub(crate) struct Call<'a> {
    pub method: Method,
    pub url: &'a str,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Duration,
    pub cap: u64,
    /// What it is for, in errors: "the MCP server files".
    pub what: &'a str,
}

/// Check a call before it connects: its URL on the address's origin and allowed, the sim and the fence.
fn check_call(address: &Address, call: &Call<'_>) -> Result<(), String> {
    if !address.same_origin(call.url) {
        return Err(format!(
            "{} named {}, which is not on its configured origin {}, so it is not called",
            call.what,
            moqui_lib::http::redacted(call.url),
            address.origin
        ));
    }
    check_url(call.url, address.allow_private)?;
    gate(address, call.what)
}

fn transport_error(what: &str, error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    format!("{what} could not be reached: {}", crate::llm::usage::redact(&text, &[]))
}

fn not_redirected(what: &str, status: u16) -> Result<(), String> {
    if (300..400).contains(&status) {
        return Err(format!(
            "{what} answered with a redirect ({status}), which is not followed; an admin must give its own URL"
        ));
    }
    Ok(())
}

/// One request and its whole answer.
///
/// # Errors
///
/// The call may not go out, the transport failed, a redirect, or a body past the cap.
pub(crate) fn exchange(address: &Address, call: Call<'_>) -> Result<Answer, String> {
    check_call(address, &call)?;
    let mut request = client(address.allow_private)
        .request(call.method, call.url)
        .timeout(call.timeout);
    let mut content_type = None;
    for (name, value) in &call.headers {
        if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.clone());
        } else {
            request = request.header(name.clone(), value.clone());
        }
    }
    if let Some(body) = call.body {
        request = request.text(content_type.as_deref().unwrap_or("application/json"), body);
    }
    let response = request.send(false).map_err(|error| {
        if error.to_string().contains("larger than the limit") {
            format!("{}'s answer is larger than {} bytes", call.what, call.cap.min(BODY_CAP))
        } else {
            transport_error(call.what, &error)
        }
    })?;
    not_redirected(call.what, response.status())?;
    if u64::try_from(response.bytes().len()).unwrap_or(u64::MAX) > call.cap.min(BODY_CAP) {
        return Err(format!("{}'s answer is larger than {} bytes", call.what, call.cap));
    }
    Ok(Answer {
        status: response.status(),
        headers: response.response_headers().to_vec(),
        body: response.body().to_string(),
    })
}

/// The events of a stream as they come: the `data` of each, read within the call's timeout and cap.
pub(crate) struct Events {
    reader: Box<dyn Read + Send>,
    parser: SseParser,
    pending: VecDeque<Frame>,
    read: u64,
    cap: u64,
    deadline: Instant,
    done: bool,
    what: String,
}

impl Events {
    /// The next event's data, `None` at the end of the stream.
    ///
    /// # Errors
    ///
    /// The time or the cap ran out, or the stream broke.
    pub fn next_data(&mut self) -> Result<Option<String>, String> {
        let mut buffer = [0_u8; 8192];
        loop {
            if let Some(frame) = self.pending.pop_front() {
                return Ok(Some(frame.data));
            }
            if self.done {
                return Ok(None);
            }
            if Instant::now() >= self.deadline {
                return Err(format!("{} did not finish in time", self.what));
            }
            let count = self
                .reader
                .read(&mut buffer)
                .map_err(|error| format!("{}'s stream broke: {error}", self.what))?;
            if count == 0 {
                self.done = true;
                if let Some(frame) = self.parser.finish() {
                    self.pending.push_back(frame);
                }
                continue;
            }
            self.read += count as u64;
            if self.read > self.cap {
                return Err(format!("{}'s stream is larger than {} bytes", self.what, self.cap));
            }
            let frames = self
                .parser
                .push(&buffer[..count])
                .map_err(|error| format!("{}'s stream does not read: {error}", self.what))?;
            self.pending.extend(frames);
        }
    }
}

/// What a request that takes a stream got: the stream, or a whole answer (a JSON body, an error).
pub(crate) enum Opened {
    Events(Events),
    Whole(Answer),
}

/// One request whose answer may be a stream of events (`text/event-stream`), read as it comes; any other answer is read
/// whole, within the cap.
///
/// # Errors
///
/// The call may not go out, the transport failed before the headers, a redirect, or a body past the cap.
pub(crate) fn open(address: &Address, call: Call<'_>) -> Result<Opened, String> {
    check_call(address, &call)?;
    let mut headers = call.headers.clone();
    if call.body.is_some() && !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let opened = client(address.allow_private)
        .open(OpenRequest {
            method: call.method,
            url: call.url.to_string(),
            headers,
            body: call.body.map(bytes::Bytes::from),
            timeout: Some(call.timeout),
        })
        .map_err(|error| transport_error(call.what, &error))?;
    not_redirected(call.what, opened.status)?;
    let events = opened.headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type") && value.to_ascii_lowercase().starts_with("text/event-stream")
    });
    let cap = call.cap.min(BODY_CAP);
    if events {
        return Ok(Opened::Events(Events {
            reader: opened.reader,
            parser: SseParser::new(),
            pending: VecDeque::new(),
            read: 0,
            cap,
            deadline: Instant::now() + call.timeout,
            done: false,
            what: call.what.to_string(),
        }));
    }
    let mut bytes = Vec::new();
    opened
        .reader
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}'s answer broke: {error}", call.what))?;
    if bytes.len() as u64 > cap {
        return Err(format!("{}'s answer is larger than {cap} bytes", call.what));
    }
    Ok(Opened::Whole(Answer {
        status: opened.status,
        headers: opened.headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }))
}

/// Whether a comma list of agent ids (`allowedAgents`) lets `agent_id` use a remote: `*` is every agent, empty none.
pub fn agent_allowed(list: Option<&str>, agent_id: &str) -> bool {
    list.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .any(|item| item == "*" || (!item.is_empty() && item == agent_id))
}

/// A name part of a remote tool name: letters, digits, `-` and `_`, no `__` (the separator), never empty.
pub fn name_part(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().chars() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '-' {
            ch
        } else {
            '_'
        };
        if ch == '_' && out.ends_with('_') {
            continue;
        }
        out.push(ch);
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "x".to_string()
    } else {
        out
    }
}

/// The longest tool name a provider takes.
pub const MAX_TOOL_NAME: usize = 64;

/// `prefix` and the name parts joined by `__`, collision safe: a name a part had to be changed for (not a plain name),
/// one longer than [`MAX_TOOL_NAME`], or one `taken` already, ends in a hash of the parts as they were.
pub fn tool_name(prefix: &str, parts: &[&str], taken: &dyn Fn(&str) -> bool) -> String {
    let plain: Vec<String> = parts.iter().map(|part| name_part(part)).collect();
    let name = format!("{prefix}{}", plain.join("__"));
    let changed = parts.iter().zip(&plain).any(|(part, plain)| part.trim() != plain);
    if !changed && name.len() <= MAX_TOOL_NAME && !taken(&name) {
        return name;
    }
    // a hashed name that is taken too (a part that hashes alike, or a remote named as a hash) gets another salt
    for salt in 0_u32.. {
        let hash: String = moqui_lib::json::canonical_sha256(&json!({"parts": parts, "salt": salt}))
            .chars()
            .take(8)
            .collect();
        let room = MAX_TOOL_NAME - hash.len() - 1;
        let mut head: String = name.chars().take(room).collect();
        while head.ends_with('_') {
            head.pop();
        }
        let hashed = format!("{head}_{hash}");
        if !taken(&hashed) {
            return hashed;
        }
    }
    unreachable!("a salt is always found")
}

/// The run bookkeeping of a remote call that waits on its remote (a question asked, a link opened): the row of a run's
/// tool call, if there is one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Waiting {
    pub remote_task_id: String,
    pub remote_task_ref: Option<String>,
    pub remote_context_id: Option<String>,
    pub remote_state: Option<String>,
    pub request_state: Option<String>,
    pub approval_id: Option<String>,
}

impl Waiting {
    fn of(row: &EntityValue) -> Self {
        Self {
            remote_task_id: text_of(row, "remoteTaskId").unwrap_or_default(),
            remote_task_ref: text_of(row, "remoteTaskRef"),
            remote_context_id: text_of(row, "remoteContextId"),
            remote_state: text_of(row, "remoteState"),
            request_state: text_of(row, "requestState"),
            approval_id: text_of(row, "approvalId"),
        }
    }
}

/// The newest `AgentRemoteTask` of a run's tool call.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn waiting_of(facade: &EntityFacade, run_id: &str, tool_call_id: &str) -> Result<Option<Waiting>, Error> {
    Ok(facade
        .find(REMOTE_TASK)?
        .eq("runId", Value::string(run_id))
        .eq("toolCallId", Value::string(tool_call_id))
        .order_by("lastUpdatedDate", true)
        .limit(1)
        .disable_authz()
        .list()?
        .values()
        .first()
        .map(Waiting::of))
}

/// The remote context of the newest task a run had with remote `remote_id`, so a second delegation goes on in it.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn context_of(facade: &EntityFacade, run_id: &str, remote_id: &str) -> Result<Option<String>, Error> {
    Ok(facade
        .find(REMOTE_TASK)?
        .eq("runId", Value::string(run_id))
        .eq("remoteId", Value::string(remote_id))
        .order_by("lastUpdatedDate", true)
        .limit(1)
        .disable_authz()
        .list()?
        .values()
        .first()
        .and_then(|row| text_of(row, "remoteContextId")))
}

/// What a remote call keeps while it waits.
#[derive(Clone, Debug, Default)]
pub struct Keep<'a> {
    pub run_id: &'a str,
    pub tool_call_id: &'a str,
    pub remote_id: Option<&'a str>,
    pub mcp_server_id: Option<&'a str>,
    pub approval_id: Option<&'a str>,
    pub task_ref: Option<&'a str>,
    pub context_id: Option<&'a str>,
    pub state: &'a str,
    pub request_state: Option<&'a str>,
}

/// Write (or update, by `remote_task_id`) the `AgentRemoteTask` of a remote call: run bookkeeping written under the
/// run's lease with the fence lifted, as the question of `ask` is. Out: its id.
///
/// # Errors
///
/// Returns an error when the row cannot be written or the run's lease is gone.
pub(crate) fn keep(
    facade: &EntityFacade,
    node: &str,
    remote_task_id: Option<&str>,
    keep: &Keep<'_>,
) -> Result<String, Error> {
    let _lifted = fence::lift();
    crate::llm::run::quietly(|| {
        crate::llm::run::in_tx(facade, || {
            crate::llm::run::held(facade, keep.run_id, node, crate::llm::run::Hold::Lease)?;
            let existing = match remote_task_id {
                Some(id) => facade
                    .find(REMOTE_TASK)?
                    .eq("remoteTaskId", Value::string(id))
                    .for_update(true)
                    .disable_authz()
                    .one()?,
                None => None,
            };
            let is_new = existing.is_none();
            let mut row = match existing {
                Some(row) => row,
                None => {
                    let mut row = facade.make_value(REMOTE_TASK)?;
                    row.set_sequenced_id_primary(facade)?;
                    row.set("runId", Value::string(keep.run_id));
                    row.set("toolCallId", Value::string(keep.tool_call_id));
                    row
                }
            };
            let set_opt = |row: &mut EntityValue, field: &str, value: Option<&str>| {
                if let Some(value) = value {
                    row.set(field, Value::string(value));
                }
            };
            set_opt(&mut row, "remoteId", keep.remote_id);
            set_opt(&mut row, "mcpServerId", keep.mcp_server_id);
            set_opt(&mut row, "approvalId", keep.approval_id);
            set_opt(&mut row, "remoteTaskRef", keep.task_ref);
            set_opt(&mut row, "remoteContextId", keep.context_id);
            row.set("remoteState", Value::string(keep.state));
            row.set("requestState", keep.request_state.map_or(Value::Null, Value::string));
            row.set("lastUpdatedDate", crate::llm::run::stamp(crate::llm::run::now_millis()));
            if is_new {
                row.create(facade)?;
            } else {
                row.update(facade)?;
            }
            let id = text_of(&row, "remoteTaskId").unwrap_or_default();
            Ok(id)
        })
    })
}

// ---- admin services (System > LLM > Remote) ----

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    rust_service!(map, store_remote_agent)?;
    rust_service!(map, store_mcp_server)?;
    rust_service!(map, store_mcp_server_tool)?;
    rust_service!(map, test_remote_agent)?;
    rust_service!(map, test_mcp_server)?;
    Ok(())
}

type Parameters = BTreeMap<String, Value>;
type Out = Result<BTreeMap<String, Value>, Error>;

fn add_error(text: impl Into<String>) {
    let text = text.into();
    ExecutionContext::with_current(|ctx| ctx.message().add_error(text));
}

fn param(parameters: &Parameters, key: &str) -> Option<String> {
    parameters
        .get(key)
        .filter(|value| !is_empty(value))
        .map(|value| to_plain_string(value).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn admin() -> bool {
    if crate::user::has_permission("AgentAdmin").unwrap_or(false) {
        return true;
    }
    add_error("this needs the AgentAdmin permission");
    false
}

/// The credential a store sets: `authKind` (none, bearer, basic, header) with `authSecret` (the key, the password or the
/// header's value), `authUsername` and `authHeader`; `None` when the call sets none (the row keeps its own).
fn auth_of(parameters: &Parameters) -> Result<Option<Auth>, String> {
    let Some(kind) = param(parameters, "authKind") else {
        return Ok(None);
    };
    let secret = param(parameters, "authSecret");
    let auth = match (kind.as_str(), secret) {
        ("none", _) => Auth::None,
        ("bearer", Some(key)) => Auth::Bearer(key),
        ("basic", Some(password)) => Auth::Basic(
            param(parameters, "authUsername").ok_or("a basic credential needs authUsername")?,
            password,
        ),
        ("header", Some(value)) => {
            let name = param(parameters, "authHeader").ok_or("a header credential needs authHeader")?;
            let auth = Auth::Header(name, value);
            // checked as a stored one is
            Auth::parse(Some(&auth.to_json().to_string()))?
        }
        ("bearer" | "basic" | "header", None) => return Err(format!("a {kind} credential needs authSecret")),
        _ => return Err(format!("{kind} is not none, bearer, basic or header")),
    };
    Ok(Some(auth))
}

/// Store a row of `entity` keyed by `key_field`: the fields given (an empty one clears it), the URL checked, the
/// credential set only when given.
fn store_row(
    facade: &EntityFacade,
    parameters: &Parameters,
    entity: &str,
    key_field: &str,
    url_field: &str,
    fields: &[&str],
) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(id) = param(parameters, key_field) else {
        add_error(format!("{key_field} is required"));
        return Ok(BTreeMap::new());
    };
    let auth = match auth_of(parameters) {
        Ok(auth) => auth,
        Err(why) => {
            add_error(why);
            return Ok(BTreeMap::new());
        }
    };
    let existing = facade
        .find(entity)?
        .eq(key_field, Value::string(&id))
        .for_update(true)
        .one()?;
    let mut row = match existing.clone() {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(entity)?;
            row.set(key_field, Value::string(&id));
            row
        }
    };
    for field in fields {
        if parameters.contains_key(*field) {
            row.set(*field, param(parameters, field).map_or(Value::Null, Value::string));
        }
    }
    let url = text_of(&row, url_field);
    // a card read from the old URL is not the new one's
    if entity == REMOTE_A2A && existing.as_ref().and_then(|row| text_of(row, url_field)) != url {
        row.set("cardJson", Value::Null);
        row.set("cardReadDate", Value::Null);
    }
    let allow_private = text_of(&row, "allowPrivateNetwork").as_deref() == Some("Y");
    match &url {
        None => {
            add_error(format!("{url_field} is required"));
            return Ok(BTreeMap::new());
        }
        Some(url) => {
            if let Err(why) = check_url(url, allow_private) {
                add_error(format!("{url_field}: {why}"));
                return Ok(BTreeMap::new());
            }
        }
    }
    if let Some(auth) = auth {
        row.set(
            "authJson",
            match auth {
                Auth::None => Value::Null,
                auth => Value::string(auth.to_json().to_string()),
            },
        );
    }
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok([(key_field.to_string(), Value::string(&id))].into_iter().collect())
}

/// `store#AgentRemoteA2a` (AgentAdmin): a remote A2A agent. The card URL is checked as a call would check it; the
/// credential is set only when `authKind` is given and never returned.
fn store_remote_agent(parameters: &Parameters, facade: &EntityFacade) -> Out {
    store_row(
        facade,
        parameters,
        REMOTE_A2A,
        "remoteId",
        "cardUrl",
        &[
            "remoteName",
            "description",
            "cardUrl",
            "allowedAgents",
            "allowPrivateNetwork",
            "timeoutSeconds",
            "disabled",
        ],
    )
}

/// `store#AgentMcpServer` (AgentAdmin): a remote MCP server, as `store#AgentRemoteA2a` stores a remote agent.
fn store_mcp_server(parameters: &Parameters, facade: &EntityFacade) -> Out {
    let out = store_row(
        facade,
        parameters,
        MCP_SERVER,
        "mcpServerId",
        "url",
        &[
            "serverName",
            "description",
            "url",
            "perUserAuth",
            "allowedAgents",
            "allowPrivateNetwork",
            "timeoutSeconds",
            "disabled",
        ],
    )?;
    crate::llm::mcp::client::forget_lists();
    Ok(out)
}

/// `store#AgentMcpServerTool` (AgentAdmin): allow a tool of a remote MCP server, with its classification (none is
/// EXTERNAL) and a description that replaces the server's.
fn store_mcp_server_tool(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let (Some(server_id), Some(tool_name)) = (param(parameters, "mcpServerId"), param(parameters, "toolName")) else {
        add_error("mcpServerId and toolName are required");
        return Ok(BTreeMap::new());
    };
    if let Some(class) = param(parameters, "classificationEnumId") {
        if crate::llm::tools::Classification::parse(&class).is_none() {
            add_error(format!("{class} is not a classification"));
            return Ok(BTreeMap::new());
        }
    }
    if facade
        .find(MCP_SERVER)?
        .eq("mcpServerId", Value::string(&server_id))
        .one()?
        .is_none()
    {
        add_error(format!("there is no MCP server {server_id}"));
        return Ok(BTreeMap::new());
    }
    let existing = facade
        .find(MCP_SERVER_TOOL)?
        .eq("mcpServerId", Value::string(&server_id))
        .eq("toolName", Value::string(&tool_name))
        .for_update(true)
        .one()?;
    let mut row = match existing.clone() {
        Some(row) => row,
        None => {
            let mut row = facade.make_value(MCP_SERVER_TOOL)?;
            row.set("mcpServerId", Value::string(&server_id));
            row.set("toolName", Value::string(&tool_name));
            row
        }
    };
    for field in ["classificationEnumId", "description", "disabled"] {
        if parameters.contains_key(field) {
            row.set(field, param(parameters, field).map_or(Value::Null, Value::string));
        }
    }
    if existing.is_some() {
        row.update(facade)?;
    } else {
        row.create(facade)?;
    }
    Ok([
        ("mcpServerId".to_string(), Value::string(&server_id)),
        ("toolName".to_string(), Value::string(&tool_name)),
    ]
    .into_iter()
    .collect())
}

/// `test#AgentRemoteA2a` (AgentAdmin): read the remote's card now, as a delegation would (the SSRF limits and the
/// interface checks included), and keep it. Out: `ok`, `message`, the card's `agentName`, `interfaceUrl`, `binding`
/// and `skills`.
fn test_remote_agent(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(id) = param(parameters, "remoteId") else {
        add_error("remoteId is required");
        return Ok(BTreeMap::new());
    };
    let Some(row) = facade
        .find(REMOTE_A2A)?
        .eq("remoteId", Value::string(&id))
        .one()?
    else {
        add_error(format!("there is no remote agent {id}"));
        return Ok(BTreeMap::new());
    };
    let remote = match crate::llm::a2a::client::Remote::of(&row, Duration::from_secs(20)) {
        Ok(remote) => remote,
        Err(why) => return Ok(test_out(false, &why, Vec::new())),
    };
    match crate::llm::a2a::client::fetch_card(&remote) {
        Ok((card, raw)) => {
            crate::llm::a2a::client::keep_card(facade, &id, &remote.address.url, &raw);
            let skills: Vec<String> = card.skills.iter().map(|(name, _)| name.clone()).collect();
            Ok(test_out(
                true,
                &format!("the card of {} reads", card.name),
                vec![
                    ("agentName", Value::string(&card.name)),
                    ("interfaceUrl", Value::string(&card.url)),
                    ("binding", Value::string(card.binding.as_str())),
                    ("skills", Value::string(skills.join(", "))),
                ],
            ))
        }
        Err(why) => Ok(test_out(false, &why, Vec::new())),
    }
}

/// `test#AgentMcpServer` (AgentAdmin): discover the server and list its tools now, as an agent's catalog would. Out:
/// `ok`, `message`, `protocol` and `toolList` (each `{toolName, title, description, allowed, classificationEnumId}`).
fn test_mcp_server(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !admin() {
        return Ok(BTreeMap::new());
    }
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let Some(id) = param(parameters, "mcpServerId") else {
        add_error("mcpServerId is required");
        return Ok(BTreeMap::new());
    };
    let Some(row) = facade
        .find(MCP_SERVER)?
        .eq("mcpServerId", Value::string(&id))
        .one()?
    else {
        add_error(format!("there is no MCP server {id}"));
        return Ok(BTreeMap::new());
    };
    let user_id = crate::user::current_user_id().ok().flatten().unwrap_or_default();
    let server = match crate::llm::mcp::client::Server::of(facade, &row, &user_id) {
        Ok(server) => server,
        Err(why) => return Ok(test_out(false, &why, Vec::new())),
    };
    crate::llm::mcp::client::forget_lists();
    let allowed = crate::llm::tools::mcp_client::allow_rows(facade, &id);
    match crate::llm::mcp::client::listing(&server) {
        Ok(listing) => {
            let tools: Vec<Value> = listing
                .tools
                .iter()
                .map(|tool| {
                    let row = allowed.iter().find(|row| text_of(row, "toolName").as_deref() == Some(tool.name.as_str()));
                    let mut map = indexmap::IndexMap::new();
                    map.insert("toolName".to_string(), Value::string(&tool.name));
                    map.insert("title".to_string(), Value::string(&tool.title));
                    map.insert(
                        "description".to_string(),
                        Value::string(crate::llm::tools::format::cut(&tool.description, 200)),
                    );
                    map.insert(
                        "allowed".to_string(),
                        Value::string(if row.is_some_and(|row| text_of(row, "disabled").as_deref() != Some("Y")) {
                            "Y"
                        } else {
                            "N"
                        }),
                    );
                    map.insert(
                        "classificationEnumId".to_string(),
                        row.and_then(|row| text_of(row, "classificationEnumId"))
                            .map_or(Value::Null, Value::string),
                    );
                    Value::Map(map)
                })
                .collect();
            Ok(test_out(
                true,
                &format!("{} tools listed", tools.len()),
                vec![
                    ("protocol", Value::string(&listing.protocol)),
                    ("toolList", Value::List(tools)),
                ],
            ))
        }
        Err(why) => Ok(test_out(false, &why, Vec::new())),
    }
}

fn test_out(ok: bool, message: &str, more: Vec<(&str, Value)>) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = more.into_iter().map(|(key, value)| (key.to_string(), value)).collect();
    out.insert("ok".to_string(), Value::string(if ok { "Y" } else { "N" }));
    out.insert("message".to_string(), Value::string(message));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_admin_url_is_checked_for_internal_addresses_however_written() {
        assert!(check_url("https://agent.example.com/.well-known/agent-card.json", false).is_ok());
        for (url, why) in [
            ("http://agent.example.com/", "must be https"),
            ("https://user:pw@agent.example.com/", "no user info"),
            ("https://agent.example.com/#x", "no user info or fragment"),
            ("ftp://agent.example.com/", "absolute http or https"),
            ("https://localhost/", "internal address"),
            ("https://api.localhost/", "internal address"),
            ("https://localhost./", "internal address"),
            ("https://127.0.0.1/", "internal address"),
            ("https://10.1.2.3/", "internal address"),
            ("https://169.254.169.254/latest/meta-data", "internal address"),
            // decimal, hex and short forms of loopback and the metadata address
            ("https://2130706433/", "internal address"),
            ("https://0x7f.1/", "internal address"),
            ("https://127.1/", "internal address"),
            ("https://2852039166/", "internal address"),
            // IPv6 loopback, IPv4-mapped and NAT64 forms
            ("https://[::1]/", "internal address"),
            ("https://[::ffff:127.0.0.1]/", "internal address"),
            ("https://[::ffff:a9fe:a9fe]/", "internal address"),
            ("https://[64:ff9b::a00:1]/", "internal address"),
            ("https://[fd00::1]/", "internal address"),
        ] {
            let refused = check_url(url, false).unwrap_err();
            assert!(refused.contains(why), "{url}: {refused}");
        }
        // a row that allows a private network may name one, over http too, but never a public host over http
        assert!(check_url("http://127.0.0.1:8080/a2a", true).is_ok());
        assert!(check_url("http://localhost:8080/a2a", true).is_ok());
        assert!(check_url("http://10.0.0.5/mcp", true).is_ok());
        assert!(check_url("http://8.8.8.8/mcp", true).unwrap_err().contains("must be https"));
        assert!(check_url("http://agent.example.com/a2a", true).unwrap_err().contains("must be https"));
        assert!(check_url("https://agent.example.com/a2a", true).is_ok());
        assert!(check_url("http://user:pw@127.0.0.1:8080/", true).is_err());
        assert_eq!(origin_of("HTTPS://Agent.Example.com/x").as_deref(), Some("https://agent.example.com:443"));
        assert_eq!(origin_of("http://127.0.0.1:8080/a").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(origin_of("http://[::1]:81/").as_deref(), Some("http://[::1]:81"));
        let address = Address::of("https://agent.example.com/a2a", false).unwrap();
        assert!(address.same_origin("https://agent.example.com:443/other"));
        assert!(!address.same_origin("https://evil.example.com/a2a"));
        assert!(!address.same_origin("http://agent.example.com/a2a"));
    }

    #[test]
    fn credentials_read_and_never_print() {
        assert_eq!(Auth::parse(None).unwrap(), Auth::None);
        assert_eq!(Auth::parse(Some(r#"{"bearer":"k1"}"#)).unwrap(), Auth::Bearer("k1".to_string()));
        let basic = Auth::parse(Some(r#"{"basic":{"username":"ann","password":"pw"}}"#)).unwrap();
        assert_eq!(basic.headers()[0].1, "Basic YW5uOnB3");
        assert_eq!(format!("{basic:?}"), "basic");
        assert!(Auth::parse(Some(r#"{"header":{"name":"Host","value":"x"}}"#)).is_err());
        assert!(Auth::parse(Some(r#"{"header":{"name":"X-Api-Key","value":"s"}}"#)).is_ok());
        assert!(Auth::parse(Some("not json")).unwrap_err().contains("not JSON"));
        assert_ne!(Auth::Bearer("a".into()).fingerprint(), Auth::Bearer("b".into()).fingerprint());
    }

    #[test]
    fn remote_tool_names_are_plain_or_hashed_never_colliding() {
        let none = |_: &str| false;
        assert_eq!(tool_name("mcp__", &["files", "read_file"], &none), "mcp__files__read_file");
        // a part changed to fit gets a hash, so get.user and get_user do not meet
        let dotted = tool_name("mcp__", &["files", "get.user"], &none);
        let plain = tool_name("mcp__", &["files", "get_user"], &none);
        assert!(dotted.starts_with("mcp__files__get_user_") && dotted != plain, "{dotted} {plain}");
        // a double underscore inside a part would read as a separator
        assert!(tool_name("mcp__", &["a__b", "c"], &none).starts_with("mcp__a_b__c_"));
        // too long, or taken
        let long = tool_name("mcp__", &["server", &"x".repeat(80)], &none);
        assert!(long.len() <= MAX_TOOL_NAME, "{long}");
        let taken = |name: &str| name == "a2a__assist";
        assert_ne!(tool_name("a2a__", &["assist"], &taken), "a2a__assist");
        // the hashed name is checked against what is taken too
        let first = tool_name("a2a__", &["assist"], &taken);
        let both = |name: &str| name == "a2a__assist" || name == first;
        let second = tool_name("a2a__", &["assist"], &both);
        assert!(second != first && second.starts_with("a2a__assist_"), "{first} {second}");
        // links shown to a user: https, or http to an internal host of a private row
        let public = Address::of("https://agent.example.com/a2a", false).unwrap();
        let private = Address::of("http://127.0.0.1:9/a2a", true).unwrap();
        assert!(link_allowed("https://idp.example.com/login", &public));
        assert!(!link_allowed("http://idp.example.com/login", &public));
        assert!(!link_allowed("http://127.0.0.1:9/qapps", &public));
        assert!(link_allowed("http://127.0.0.1:9/qapps?assist=C1", &private));
        assert!(!link_allowed("http://idp.example.com/login", &private));
        assert!(!link_allowed("javascript:alert(1)", &private));
        assert_eq!(link_host("https://IdP.example.com:8443/x?y"), "idp.example.com:8443");
        assert!(agent_allowed(Some("assist, other"), "other"));
        assert!(agent_allowed(Some("*"), "any"));
        assert!(!agent_allowed(Some(""), "assist"));
        assert!(!agent_allowed(None, "assist"));
    }
}
