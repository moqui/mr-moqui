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

//! The MCP server (plans/LLM-A2A-Assist.md 4.16, D10; docs/MCP.md), protocol version 2026-07-28, off by default
//! (`llm-facade` `mcp server-enabled`). MCP is stateless at this version: no `initialize` and no session; every request
//! carries its protocol version and client capabilities in `_meta`, and clients learn the server with
//! `server/discover`.
//!
//! The transport ([`server`]) is Streamable HTTP: `POST /mcp`, one JSON-RPC request each, answered with one JSON
//! object (GET and DELETE are 405). Before the body is used: the `Origin` (403 when present and not this machine,
//! the configured host or an allowed origin: never by the request's own `Host`, against DNS rebinding), the
//! content types, the `MCP-Protocol-Version`, `Mcp-Method` and `Mcp-Name` headers against the body (400 with
//! `HeaderMismatch` when one is missing or differs), the `_meta` fields (400 with `-32602`) and the version (400
//! with `UnsupportedProtocolVersion`). The public documents are the Server Card (`/mcp/server-card`, also at the
//! plan's `/.well-known/mcp.json`), the AI Catalog that names it (`/.well-known/ai-catalog.json`) and a setup prompt
//! for coding agents (`/mcp/setup`).
//!
//! The tools ([`tools`]) are the read tools `screen`, `search`, `data` and `skill`, and the services an
//! `AgentToolExposure` opts in with `exposeMcp`, by a sanitized name (past `max-direct-tools` of them the lazy
//! `find_tools`, `describe_tools` and `run_tool` instead), bounded by the caller's own permissions and by the MCP
//! agent (the conf's `agent`, `mcp` in the seed: its tool policy is the server's allowlist, its role group the MCP
//! role group), sorted by name with `ttlMs` and `cacheScope: "private"`. They run on the one tool registry as the
//! caller: a read under the read fence with no run; a write, or a `screen` call that may hold one, as a run of the
//! `mcp` surface with one tool call and no model, so it has a conversation, events and approvals as any run has.
//!
//! A write waits for its human owner (MRTR): the call answers `input_required` with a URL elicitation (the drawer
//! opened on the call's conversation, which shows the approval card) and a `requestState` ([`state`]) bound to the
//! principal, the tool, a digest of the arguments and the approval's expiry. The owner decides in Moqui; the MCP
//! client never can. The retry with that state checks it, then the approval: still pending, it waits up to
//! `approval-wait` and answers `input_required` again; decided, the call goes on once (Approved moves to Executing
//! only once) and its result is stored on the run, so a later retry replays it.
//!
//! Auth is a Bearer login key, HTTP Basic with an app password (or a login key as the password) or the
//! `api_key`/`login_key` header, over TLS or loopback ([`crate::llm::a2a::log_in`]), and `McpServerUse` and
//! `AgentUse`. OAuth comes later (answered question 10: Moqui as the authorization server): [`www_authenticate`] is
//! where a 401 will name its protected resource metadata.

pub mod client;
pub mod server;
pub mod state;
pub mod tools;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use serde_json::{json, Value as Json};

use crate::context::ExecutionContextFactory;
pub use crate::llm::a2a::{Answer, HttpRequest, Origin, Reply};

/// The protocol version this server speaks.
pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// `_meta` keys of the protocol.
pub const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub const META_CLIENT_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
pub const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// The JSON-RPC and MCP error codes this server answers with. Codes of its own are outside JSON-RPC's reserved range,
/// as the MCP error code policy asks.
pub mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL: i64 = -32603;
    pub const HEADER_MISMATCH: i64 = -32020;
    pub const MISSING_CLIENT_CAPABILITY: i64 = -32021;
    pub const UNSUPPORTED_VERSION: i64 = -32022;
    /// No valid credentials (HTTP 401).
    pub const UNAUTHENTICATED: i64 = -31401;
    /// Credentials over plain HTTP, or a user without the permissions (HTTP 403).
    pub const FORBIDDEN: i64 = -31403;
    /// The tarpit (HTTP 429, with `Retry-After`).
    pub const TOO_MANY: i64 = -31429;
}

/// Whether the server is on: the `llm-facade` and its `mcp server-enabled` are both.
pub fn enabled(factory: &ExecutionContextFactory) -> bool {
    let llm = crate::llm::facade(factory);
    llm.enabled() && llm.conf().mcp.server_enabled
}

/// The protocol versions this server takes: the conf's `protocol-version`, which is [`PROTOCOL_VERSION`] unless an
/// admin pins another.
pub fn supported_versions(factory: &ExecutionContextFactory) -> Vec<String> {
    vec![crate::llm::facade(factory).conf().mcp.protocol_version.clone()]
}

/// The server's `Implementation`, in every result's `_meta`.
pub fn server_info() -> Json {
    json!({"name": "moqui", "title": "Moqui", "version": env!("CARGO_PKG_VERSION")})
}

/// The base URL the server is reached at: the conf's `public-url`, else the request's (a believed proxy's forwarded
/// headers only with `trust-forwarded-headers`), as the A2A card's is made.
pub fn base_url(factory: &ExecutionContextFactory, origin: &Origin) -> String {
    let llm = crate::llm::facade(factory);
    let mut conf = llm.conf().a2a.clone();
    conf.public_url.clone_from(&llm.conf().mcp.public_url);
    conf.trust_forwarded_headers = llm.conf().mcp.trust_forwarded_headers;
    crate::llm::a2a::card::base_url_of(&conf, origin, || {
        factory.webapp_root_url_for("webroot", Some(origin.https))
    })
}

/// The `WWW-Authenticate` of a 401. The OAuth 2.1 resource server (Moqui as the authorization server, answered
/// question 10) adds `resource_metadata` here.
pub fn www_authenticate() -> &'static str {
    "Bearer realm=\"mcp\", Basic realm=\"mcp\""
}

/// A request's user, authenticated, and the base URL the server is reached at.
#[derive(Clone, Debug)]
pub struct Caller {
    pub user_id: String,
    pub username: String,
    pub base_url: String,
}

/// Serve a request to `/mcp` on a request worker ([`server::serve`]).
pub fn serve(factory: &Arc<ExecutionContextFactory>, request: &HttpRequest) -> Answer {
    server::serve(factory, request)
}
