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

//! The OpenAI-compatible gateway's routes, `/llm/v1` and under it (`moqui_core::llm::gateway`, docs/Gateway.md). The
//! body is read whole (at most 16 MiB) and the request is served on a request worker, which authenticates and checks
//! it; the model call runs on a thread of its own, counted with the A2A server's waits and streams (at most 256 on a
//! node, 16 of one user), and answers with a whole reply or a `text/event-stream` written into the response body as it
//! comes. A caller that goes away ends the call: a whole call's handler is dropped, which cancels it, and a stream's
//! next write fails, or waits no longer than the idle limit when the caller stops reading; either way the call drops its
//! upstream request and its endpoint slot. `/llm/v1` is exempt from the request deadline: the gateway's own `timeout`
//! bounds a call and answers 504 in the OpenAI shape.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::response::Response;
use bytes::Bytes;
use hyper::body::Frame;
use moqui_core::llm::a2a::{self, HttpRequest, Origin, Reply};
use moqui_core::llm::gateway::{self, Answer, Respond};
use moqui_core::llm::provider::CancelToken;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

use super::a2a::{response_of, Full, Open};
use crate::guard::Event;
use crate::http::{forwarded_https, read_body, ClientAddr, HttpState, TlsConnection};

/// The largest request body (embeddings of many texts).
const MAX_BODY: usize = 16 * 1024 * 1024;

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

/// How a call's thread answers: a whole reply, or the start of a stream.
enum Head {
    Reply(Reply),
    Stream(mpsc::Receiver<Chunk>),
}

/// The host's side of [`Respond`]: the first answer goes to the waiting handler, a stream's pieces into its body.
struct HostRespond {
    head: Option<oneshot::Sender<Head>>,
    body: Option<mpsc::Sender<Chunk>>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    gone: CancelToken,
}

impl Respond for HostRespond {
    fn reply(&mut self, reply: Reply) {
        if let Some(head) = self.head.take() {
            let _ = head.send(Head::Reply(reply));
        }
    }

    fn start(&mut self) -> bool {
        let Some(head) = self.head.take() else {
            return false;
        };
        let (tx, rx) = mpsc::channel::<Chunk>(64);
        self.body = Some(tx);
        head.send(Head::Stream(rx)).is_ok()
    }

    /// The piece goes into the body's channel by `deadline`: a caller whose connection does not take what is written
    /// (it stopped reading) is gone once the channel stays full that long.
    fn send(&mut self, text: &str, deadline: Instant) -> bool {
        let Some(body) = &self.body else {
            return false;
        };
        let mut chunk = Ok(Bytes::from(text.to_string()));
        loop {
            if *self.shutdown.borrow() {
                self.gone.cancel();
                return false;
            }
            match body.try_send(chunk) {
                Ok(()) => return true,
                Err(TrySendError::Closed(_)) => {
                    self.gone.cancel();
                    return false;
                }
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        tracing::warn!("a gateway caller took no stream data for its idle limit; the call is dropped");
                        self.gone.cancel();
                        return false;
                    }
                    chunk = back;
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    fn gone(&self) -> CancelToken {
        self.gone.clone()
    }
}

/// Cancels a call when the handler waiting for its answer is dropped: the caller went away before it.
struct CancelOnDrop(Option<CancelToken>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}

/// 503 when the node has no room, 429 when the user has too many calls open.
fn busy(full: &Full) -> Reply {
    let (status, text) = match full {
        Full::Node => (503, "too many open model calls on this server; try again"),
        Full::User => (429, "too many of your model calls are open; wait for one to end and try again"),
    };
    gateway::error(status, "requests", Some("too_many_open_calls"), None, text).with_header("retry-after", "5")
}

fn refused(status: u16, message: &str) -> Response {
    let kind = if status >= 500 { "server_error" } else { "invalid_request_error" };
    response_of(gateway::error(status, kind, None, None, message))
}

/// `/llm/v1` and `/llm/v1/{*rest}`.
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
        return refused(413, &format!("the request is over {MAX_BODY} bytes"));
    }
    let body = match read_body(&state, body, MAX_BODY).await {
        Ok(body) => body,
        Err(refusal) => return refusal.response(),
    };
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(name, value)| Some((name.as_str().to_ascii_lowercase(), value.to_str().ok()?.to_string())))
        .collect();
    let https = over_tls || forwarded_https(&parts.headers, peer.ip(), state.guard.conf.addressing());
    let gateway_request = HttpRequest {
        method: parts.method.as_str().to_string(),
        path: parts.uri.path().strip_prefix("/llm/v1").unwrap_or_default().to_string(),
        query: Vec::new(),
        headers,
        body: body.to_vec(),
        secure: a2a::secure(https, peer.ip()),
        client_ip: client.to_string(),
        origin: Origin::default(),
    };
    let factory = Arc::clone(&state.factory);
    let job_factory = Arc::clone(&factory);
    let guard = Arc::clone(&state.guard);
    let (done, answer) = oneshot::channel::<Answer>();
    let submitted = factory.request_pool().try_submit(Arc::clone(&factory), move || {
        let open = |user_id: &str| -> Result<gateway::Held, Reply> {
            Open::take(user_id)
                .map(|open| Box::new(open) as gateway::Held)
                .map_err(|full| busy(&full))
        };
        let answer = gateway::serve(&job_factory, &gateway_request, &open);
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
        return refused(500, "the gateway request ended without a response");
    };
    let (held, work) = match answer {
        Answer::Reply(reply) => return response_of(reply),
        Answer::Later { held, work } => (held, work),
    };
    let gone = CancelToken::new();
    let mut cancel_on_drop = CancelOnDrop(Some(gone.clone()));
    let (head, first) = oneshot::channel::<Head>();
    let shutdown = state.shutdown.clone();
    let spawned = std::thread::Builder::new()
        .name("moqui-llm-gateway".to_string())
        .spawn(move || {
            let _held = held;
            let mut respond = HostRespond {
                head: Some(head),
                body: None,
                shutdown,
                gone,
            };
            work(&mut respond);
        });
    if let Err(error) = spawned {
        return refused(500, &format!("the gateway call did not start: {error}"));
    }
    match first.await {
        Ok(Head::Reply(reply)) => response_of(reply),
        Ok(Head::Stream(rx)) => {
            // the stream's body notices a caller that goes away from here on
            cancel_on_drop.0 = None;
            Response::builder()
                .status(200)
                .header("content-type", "text/event-stream; charset=utf-8")
                .header("cache-control", "no-cache, no-store")
                .header("x-accel-buffering", "no")
                .body(axum::body::Body::new(ChannelBody(rx)))
                .unwrap_or_else(|_| refused(500, "the gateway stream header was not valid"))
        }
        Err(_) => refused(500, "the gateway call ended without a response"),
    }
}
