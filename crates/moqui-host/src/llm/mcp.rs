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

//! The MCP server's routes (`moqui_core::llm::mcp`, docs/MCP.md): `/mcp` (Streamable HTTP, POST only) and the public
//! documents, `/mcp/server-card`, `/mcp/setup`, `/.well-known/mcp.json` and `/.well-known/ai-catalog.json`; each is
//! 404 while the server is off. The body is read whole (at most 4 MiB) and served on a request worker, which
//! authenticates and answers; a retry that waits for its owner's decision waits on a thread of its own, counted with
//! the A2A waits and streams of its user (16 at most, one more is 429). A refused login counts for web protection.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use moqui_core::llm::mcp::{self, Answer, HttpRequest, Reply};

use super::a2a::{busy, origin_of, response_of, Open};
use crate::guard::Event;
use crate::http::{plain, read_body, ClientAddr, HttpState, TlsConnection};

/// The largest request body.
const MAX_BODY: usize = 4 * 1024 * 1024;

/// A public document of the server: the path after the last `/` names it.
pub(crate) async fn document(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let origin = origin_of(&state, request.headers(), over_tls, peer);
    let which = request.uri().path().rsplit('/').next().unwrap_or_default().to_string();
    response_of(mcp::server::document(&state.factory, &which, &origin))
}

/// `/mcp`.
pub(crate) async fn handle(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let client = request
        .extensions()
        .get::<ClientAddr>()
        .map_or(peer.ip(), |client| client.0);
    let (parts, body) = request.into_parts();
    let declared = parts
        .headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_BODY) {
        return plain(413, &format!("the request is over {MAX_BODY} bytes"));
    }
    let body = match read_body(&state, body, MAX_BODY).await {
        Ok(body) => body,
        Err(refused) => return refused.response(),
    };
    let mut headers: Vec<(String, String)> = Vec::new();
    for (name, value) in &parts.headers {
        if let Ok(value) = value.to_str() {
            headers.push((name.as_str().to_ascii_lowercase(), value.to_string()));
        }
    }
    let origin = origin_of(&state, &parts.headers, over_tls, peer);
    let mcp_request = HttpRequest {
        method: parts.method.as_str().to_string(),
        path: String::new(),
        query: Vec::new(),
        headers,
        body: body.to_vec(),
        secure: moqui_core::llm::a2a::secure(origin.https, peer.ip()),
        client_ip: client.to_string(),
        origin,
    };
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let guard = Arc::clone(&state.guard);
    let (done, answer) = tokio::sync::oneshot::channel::<Answer>();
    let submitted = factory.request_pool().try_submit(Arc::clone(&factory), move || {
        let answer = mcp::serve(&job_factory, &mcp_request);
        if moqui_core::web::take_auth_failure() {
            guard.record(client, Event::AuthFailure);
        }
        let _ = done.send(answer);
        Ok(())
    });
    if submitted.is_err() {
        return state.guard.unavailable("the server is busy; try again");
    }
    let Ok(answer) = answer.await else {
        return plain(500, "the MCP request ended without a response");
    };
    match answer {
        Answer::Reply(reply) => response_of(reply),
        Answer::Later { user_id, finish } => {
            let open = match Open::take(&user_id) {
                Ok(open) => open,
                Err(full) => return busy(&full),
            };
            let (done, reply) = tokio::sync::oneshot::channel::<Reply>();
            let spawned = std::thread::Builder::new()
                .name("moqui-mcp-wait".to_string())
                .spawn(move || {
                    let _open = open;
                    let _ = done.send(finish());
                });
            if let Err(error) = spawned {
                return crate::http::internal_error(&format!("the MCP wait did not start: {error}"));
            }
            match reply.await {
                Ok(reply) => response_of(reply),
                Err(_) => plain(500, "the MCP wait ended without a response"),
            }
        }
        // the server answers no request with a stream
        Answer::Stream { .. } => plain(500, "the MCP server does not stream"),
    }
}
