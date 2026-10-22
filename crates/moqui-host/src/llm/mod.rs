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

//! The host routes of the agents (plans/LLM-A2A-Assist.md 4.5), under the `llm` feature: the event stream of a run
//! and its catch-up ([`sse`]), the frame document of a Vue SFC canvas ([`sandbox`]), and the A2A server ([`a2a`]). The
//! drawer's JSON calls are transitions of the webroot `assist` subscreen (`moqui_core::llm::assist`, docs/Assist.md);
//! the MCP server ([`mcp`]) and the OpenAI-compatible gateway ([`gateway`]) are here too.

pub(crate) mod a2a;
pub(crate) mod gateway;
pub(crate) mod mcp;
#[cfg(feature = "tui")]
pub(crate) mod screens;
pub(crate) mod sandbox;
pub(crate) mod sse;

use std::sync::Arc;

use axum::routing::{any, get};
use axum::Router;

use crate::http::HttpState;

/// Add `GET /assist/runs/{runId}/events` (SSE), `GET /assist/runs/{runId}/events.json` and `GET /assist/sandbox` (the
/// Vue SFC frame document, [`sandbox`]), the A2A server's `/a2a`, `/a2a/{*rest}` and `/.well-known/agent-card.json`
/// ([`a2a`]; each 404 while it is off), the MCP server's `/mcp` and its public documents ([`mcp`], 404 while it is
/// off), and the OpenAI-compatible gateway's `/llm/v1` and `/llm/v1/{*rest}`
/// ([`gateway`], 404 while it is off) to `app`.
pub(crate) fn routes(app: Router<Arc<HttpState>>) -> Router<Arc<HttpState>> {
    app.route("/assist/runs/{run_id}/events", get(sse::events))
        .route("/assist/runs/{run_id}/events.json", get(sse::events_json))
        .route("/assist/sandbox", get(sandbox::frame))
        .route("/a2a", any(a2a::handle))
        .route("/a2a/", any(a2a::handle))
        .route("/a2a/{*rest}", any(a2a::handle))
        .route("/.well-known/agent-card.json", get(a2a::card))
        .route("/mcp", any(mcp::handle))
        .route("/mcp/server-card", get(mcp::document))
        .route("/mcp/setup", get(mcp::document))
        .route("/.well-known/mcp.json", get(mcp::document))
        .route("/.well-known/ai-catalog.json", get(mcp::document))
        .route("/llm/v1", any(gateway::handle))
        .route("/llm/v1/{*rest}", any(gateway::handle))
}
