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

//! The A2A server's routes (`moqui_core::llm::a2a`, docs/A2A.md): `/a2a` (JSON-RPC), `/a2a/{*rest}` (HTTP+JSON) and
//! the public card at `/.well-known/agent-card.json`. The body is read whole (at most 4 MiB) and the request is served
//! on a request worker, which authenticates it and answers, or hands back what takes longer: a blocking send's wait
//! and a stream each run on a thread of their own (at most 256 at once on a node; one more is 503), so neither holds a
//! request worker; one user has at most 16 open (one more is 429). A stream is written into the response body as the
//! drawer's is. Credentials count as taken over TLS, a believed proxy's https or loopback; a refused login counts for
//! web protection.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use hyper::body::Frame;
use moqui_core::llm::a2a::{self, Answer, HttpRequest, Origin, Reply};
use moqui_lib::sync::lock;
use tokio::sync::mpsc;

use crate::guard::Event;
use crate::http::{forwarded_https, plain, read_body, ClientAddr, HttpState, TlsConnection};

/// The largest request body.
const MAX_BODY: usize = 4 * 1024 * 1024;
/// Waits and streams open at once on this node, and of one user.
const MAX_OPEN: usize = 256;
pub(crate) const MAX_OPEN_PER_USER: usize = 16;

static OPEN: AtomicUsize = AtomicUsize::new(0);
static OPEN_BY_USER: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

/// One open wait or stream, counted for the node and its user until its thread ends (the gateway's calls count too).
pub(crate) struct Open(String);

/// Why no wait or stream could be opened.
pub(crate) enum Full {
    Node,
    User,
}

impl Open {
    pub(crate) fn take(user_id: &str) -> Result<Self, Full> {
        let mut by_user = lock(&OPEN_BY_USER);
        let by_user = by_user.get_or_insert_with(HashMap::new);
        let mine = by_user.get(user_id).copied().unwrap_or(0);
        if mine >= MAX_OPEN_PER_USER {
            return Err(Full::User);
        }
        if OPEN.fetch_add(1, Ordering::SeqCst) >= MAX_OPEN {
            OPEN.fetch_sub(1, Ordering::SeqCst);
            return Err(Full::Node);
        }
        by_user.insert(user_id.to_string(), mine + 1);
        Ok(Open(user_id.to_string()))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
        let mut by_user = lock(&OPEN_BY_USER);
        if let Some(by_user) = by_user.as_mut() {
            if let Some(count) = by_user.get_mut(&self.0) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    by_user.remove(&self.0);
                }
            }
        }
    }
}

type Chunk = std::io::Result<Bytes>;

/// The response body of a stream: chunks from its thread until it drops its sender.
struct ChannelBody(mpsc::Receiver<Chunk>);

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        self.0
            .poll_recv(cx)
            .map(|chunk| chunk.map(|chunk| chunk.map(Frame::data)))
    }
}

/// 503 when the node has no room, 429 when the user has [`MAX_OPEN_PER_USER`] open.
pub(crate) fn busy(full: &Full) -> Response {
    let (status, text) = match full {
        Full::Node => (503, "too many A2A streams and waits on this server; try again"),
        Full::User => (
            429,
            "too many A2A streams and waits of yours are open; close one and try again",
        ),
    };
    Response::builder()
        .status(status)
        .header("retry-after", "5")
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from(text))
        .unwrap_or_else(|_| plain(status, text))
}

pub(crate) fn response_of(reply: Reply) -> Response {
    let mut response = Response::builder().status(reply.status);
    for (name, value) in reply.headers {
        response = response.header(name, value);
    }
    response
        .body(axum::body::Body::from(reply.body))
        .unwrap_or_else(|_| plain(500, "an A2A response header was not valid"))
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

pub(crate) fn origin_of(state: &HttpState, headers: &HeaderMap, over_tls: bool, peer: SocketAddr) -> Origin {
    Origin {
        https: over_tls || forwarded_https(headers, peer.ip(), state.guard.conf.addressing()),
        believed: state.guard.conf.addressing().believes(peer.ip()),
        host: header(headers, "host"),
        forwarded_proto: header(headers, "x-forwarded-proto"),
        forwarded_host: header(headers, "x-forwarded-host"),
    }
}

/// `GET /.well-known/agent-card.json`: the public card, 404 while the server is off.
pub(crate) async fn card(
    State(state): State<Arc<HttpState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let over_tls = request.extensions().get::<TlsConnection>().is_some();
    let origin = origin_of(&state, request.headers(), over_tls, peer);
    response_of(a2a::card::public(&state.factory, &origin))
}

/// `/a2a` and `/a2a/{*rest}`.
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
    let query = axum::extract::Query::<Vec<(String, String)>>::try_from_uri(&parts.uri)
        .map(|query| query.0)
        .unwrap_or_default();
    let origin = origin_of(&state, &parts.headers, over_tls, peer);
    let a2a_request = HttpRequest {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().strip_prefix("/a2a").unwrap_or_default().to_string(),
        query,
        headers,
        body: body.to_vec(),
        secure: a2a::secure(origin.https, peer.ip()),
        client_ip: client.to_string(),
        origin,
    };
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let guard = Arc::clone(&state.guard);
    let (done, answer) = tokio::sync::oneshot::channel::<Answer>();
    let submitted = factory.request_pool().try_submit(Arc::clone(&factory), move || {
        let answer = a2a::serve(&job_factory, &a2a_request);
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
        return plain(500, "the A2A request ended without a response");
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
                .name("moqui-a2a-wait".to_string())
                .spawn(move || {
                    let _open = open;
                    let _ = done.send(finish());
                });
            if let Err(error) = spawned {
                return crate::http::internal_error(&format!("the A2A wait did not start: {error}"));
            }
            match reply.await {
                Ok(reply) => response_of(reply),
                Err(_) => plain(500, "the A2A wait ended without a response"),
            }
        }
        Answer::Stream { user_id, write } => {
            let open = match Open::take(&user_id) {
                Ok(open) => open,
                Err(full) => return busy(&full),
            };
            let (tx, rx) = mpsc::channel::<Chunk>(64);
            let shutdown = state.shutdown.clone();
            let spawned = std::thread::Builder::new()
                .name("moqui-a2a-sse".to_string())
                .spawn(move || {
                    let _open = open;
                    let mut sink = |frame: Option<&str>| -> bool {
                        if *shutdown.borrow() || tx.is_closed() {
                            return false;
                        }
                        let text = frame.map_or_else(|| ": ping\n\n".to_string(), str::to_string);
                        tx.blocking_send(Ok(Bytes::from(text))).is_ok()
                    };
                    write(&mut sink);
                });
            if let Err(error) = spawned {
                return crate::http::internal_error(&format!("the A2A stream did not start: {error}"));
            }
            Response::builder()
                .status(200)
                .header("content-type", "text/event-stream; charset=utf-8")
                .header("cache-control", "no-cache, no-store")
                .header("x-accel-buffering", "no")
                .body(axum::body::Body::new(ChannelBody(rx)))
                .unwrap_or_else(|_| plain(500, "the A2A stream header was not valid"))
        }
    }
}
