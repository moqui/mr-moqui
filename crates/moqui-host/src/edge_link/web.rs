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

//! The edge node's web front (plans/archive/Edge-Node.md §5.3): a request to a terminate host goes down its local node's link
//! as an HTTP/2 request, after the edge node's own web protection, with the bodies streamed both ways. The client's
//! address goes in [`CLIENT_HEADER`]; any forwarding header the client sent is removed, and so are the hop-by-hop
//! headers. A WebSocket upgrade becomes an extended CONNECT (RFC 8441) down the link, answered 101 here once the local
//! node takes it. While the node's link is down the host answers with the offline page.
//!
//! A passthrough host's request on the plain HTTP listener is sent to its https URL; one that came over this
//! listener's TLS under another server name is misdirected (421), since its TLS belongs to the local node.
//!
//! **Pools** (plans/Edge-Node2.md §4.4). Each request goes to one member of the node's pool ([`super::balance`]):
//! sticky by the host's cookie (`EdgeHost.stickyCookie`, else the conf's `sticky-cookie`, the session cookie by
//! default), round robin without it; a WebSocket as its opening request. A request that fails before any byte of an
//! answer goes once more to another member when it is a GET or HEAD with no body, or when it was not sent at all
//! ([`send_once_more`]); nothing else is sent twice.
//!
//! **Waits** ([`Waits`]). A request's answer may come only once its whole body is stored (an LFS object, a CI artifact
//! or result, a large upload), so the edge node bounds the member's connection taking the request and each gap in the
//! request's body as it goes down, and then the wait for the answer's head from when the body is all sent, which is
//! long by default; never the whole of it by one short timeout. A request with no body waits only a short time for its
//! answer's head (`forward-first-byte-seconds`), so a GET or HEAD a hung member holds goes once more to another member
//! and the hung one is set aside, well within the client's patience.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::response::Response;
use http::header::{self, HeaderMap, HeaderValue};
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::TokioIo;
use sha1::{Digest as _, Sha1};

use super::accept::{EdgeServer, Picked};
use super::balance::Affinity;
use super::hosts::Mode;
use super::{CLIENT_HEADER, PROTO_HEADER, TUNNEL_HEADER};
use crate::http::{plain, ExtendedConnect, Peer, TlsConnection};

/// The offline page of a terminate host whose node's link is down, unless `edge_offline_page` names another.
pub(crate) const DEFAULT_OFFLINE_PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Offline</title>
<style>body{font-family:system-ui,sans-serif;margin:0;display:flex;min-height:100vh;align-items:center;justify-content:center;background:#f6f6f6;color:#222}
main{max-width:32rem;padding:2rem;text-align:center}h1{font-weight:500}</style></head>
<body><main><h1>Offline for now</h1><p>This site's server cannot be reached at the moment. Please try again in a few minutes.</p></main></body></html>
"#;

/// How long a client waits before trying again, by the offline page's `Retry-After`.
const RETRY_AFTER: &str = "30";

/// The hop-by-hop headers (RFC 9110 §7.6.1), which a proxy does not pass on.
const HOP_BY_HOP: [&str; 9] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// The headers that name a client, a protocol or a host for a proxy: never passed on from a client, and never believed
/// on the down connection except the edge node's own.
const FORWARDING: [&str; 9] = [
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "x-real-ip",
    CLIENT_HEADER,
    PROTO_HEADER,
    TUNNEL_HEADER,
];

/// Remove the hop-by-hop headers, with the ones `Connection` names.
pub(crate) fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<String> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|name| name.trim().to_ascii_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    for name in &named {
        headers.remove(name.as_str());
    }
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
}

/// Remove the forwarding headers, and `client_ip_header`, the one the web protection reads a proxy's client from.
pub(crate) fn strip_forwarding(headers: &mut HeaderMap, client_ip_header: &str) {
    for name in FORWARDING {
        headers.remove(name);
    }
    if !client_ip_header.is_empty() {
        headers.remove(client_ip_header);
    }
}

/// The edge node's front for the terminate and passthrough hosts, on both web listeners.
pub(crate) struct WebFront {
    server: Arc<EdgeServer>,
}

impl WebFront {
    pub(crate) fn new(server: &Arc<EdgeServer>) -> Self {
        Self {
            server: Arc::clone(server),
        }
    }
}

/// Where a request to one of the paired nodes' hosts goes.
#[derive(Clone)]
pub(crate) enum Target {
    /// Down a link of the node's pool, to the member chosen as it is sent: by `sticky`, the value of the host's sticky
    /// cookie, else in turn.
    Forward { node: String, sticky: Option<String> },
    /// The node's link is down.
    Offline,
    /// A passthrough host over plain HTTP: to its https URL.
    Https(String),
    /// A passthrough host's request that came over this listener's TLS.
    Misdirected,
}

impl Target {
    /// The paired node a forward goes to.
    pub(crate) fn node(&self) -> Option<&str> {
        match self {
            Self::Forward { node, .. } => Some(node),
            _ => None,
        }
    }
}

/// The host a request names: `Host`, else the URI's authority.
fn request_host<B>(request: &http::Request<B>) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str().to_string())
        })
}

/// The value of the cookie `name` in `headers` (each `Cookie` header, as HTTP/2 splits them), when it is not empty.
fn sticky_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies: Vec<&str> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect();
    if cookies.is_empty() {
        return None;
    }
    moqui_core::web::cookie_value(&cookies.join("; "), name).filter(|value| !value.is_empty())
}

/// Whether a request may go once more to another member after it was sent and failed before any answer: a GET or HEAD
/// with no body, not a WebSocket (plans/Edge-Node2.md §4.4).
fn may_send_again(method: &http::Method, body_ended: bool, websocket: bool) -> bool {
    !websocket && body_ended && (method == http::Method::GET || method == http::Method::HEAD)
}

fn is_websocket<B>(request: &http::Request<B>) -> bool {
    let has = |name: header::HeaderName, token: &str| {
        request
            .headers()
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|part| part.trim().eq_ignore_ascii_case(token))
    };
    request.method() == http::Method::GET && has(header::CONNECTION, "upgrade") && has(header::UPGRADE, "websocket")
}

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key` (RFC 6455 §4.2.2).
fn websocket_accept(key: &[u8]) -> String {
    use base64::Engine as _;
    let mut sha = Sha1::new();
    sha.update(key);
    sha.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(sha.finalize())
}

impl WebFront {
    /// Where `request` goes, when it names one of the paired nodes' hosts; `None` for the edge node's own, and for a
    /// chat webhook or file of a bridge the host's node routes through this node (plans/Edge-Node2.md §4.1), which this
    /// node answers by the bridge id within the host's name.
    pub(crate) fn target<B>(&self, request: &http::Request<B>) -> Option<Target> {
        let host = request_host(request)?;
        let route = self.server.hosts.route(&host)?;
        if route.mode == Mode::Terminate && self.server.hosts.answers_here(&route.node, request.uri().path()) {
            return None;
        }
        let tls = request.extensions().get::<TlsConnection>().is_some();
        Some(match route.mode {
            Mode::Passthrough if tls => Target::Misdirected,
            Mode::Passthrough => {
                let path = request.uri().path_and_query().map_or("/", |path| path.as_str());
                let name = super::hosts::normalize(&host).unwrap_or(host);
                Target::Https(format!("https://{name}{path}"))
            }
            Mode::Terminate if self.server.takes_work(&route.node) => {
                let cookie = route
                    .sticky_cookie
                    .unwrap_or_else(|| Arc::clone(&self.server.balance().sticky_cookie));
                Target::Forward {
                    sticky: sticky_value(request.headers(), &cookie),
                    node: route.node,
                }
            }
            Mode::Terminate => Target::Offline,
        })
    }

    /// Answer `request` for `target`, from `peer`.
    pub(crate) fn forward<B>(
        &self,
        target: Target,
        request: http::Request<B>,
        peer: &Peer,
    ) -> Pin<Box<dyn Future<Output = Response> + Send>>
    where
        B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
        B::Error: Into<axum::BoxError>,
    {
        match target {
            Target::Forward { node, sticky } => {
                let server = Arc::clone(&self.server);
                let client = std::net::SocketAddr::new(peer.client, peer.remote.port());
                let https = peer.https;
                Box::pin(async move { forward(server, node, sticky, request, client, https).await })
            }
            Target::Offline => {
                self.server.count("moqui.edgelink.offline", "terminate");
                let page = Arc::clone(&self.server.config().offline_page);
                Box::pin(async move { offline(&page) })
            }
            Target::Https(location) => Box::pin(async move { redirect(&location) }),
            Target::Misdirected => {
                Box::pin(async { plain(421, "this host's TLS goes to its own server; ask again with its name") })
            }
        }
    }
}

fn offline(page: &str) -> Response {
    let mut response = Response::new(axum::body::Body::from(page.to_string()));
    *response.status_mut() = http::StatusCode::SERVICE_UNAVAILABLE;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::RETRY_AFTER, HeaderValue::from_static(RETRY_AFTER));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn redirect(location: &str) -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = http::StatusCode::MOVED_PERMANENTLY;
    if let Ok(location) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

/// An answer's body, its request counted in flight on its member until it ends or drops.
struct Counted {
    body: hyper::body::Incoming,
    _flight: super::balance::Flight,
}

impl hyper::body::Body for Counted {
    type Data = bytes::Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

/// Why a request down a member's link had no answer.
#[derive(Debug)]
pub(crate) enum Failed {
    /// The connection failed it; whether the connection is closed now.
    Error(String, bool),
    TimedOut,
}

/// The bounds of a forwarded request's wait for its answer's head. A request whose answer comes only after its whole
/// body is stored (an LFS or CI upload, a slow multipart post) may take as long as its body takes to send, so only
/// the parts that say nothing is happening are short: `open` for the member's connection to take the request, `idle`
/// for a gap in the request's body as it goes down, and `head`, from when the body is all sent, for the answer's
/// head (the `<edge>` conf's `forward-idle-seconds` and `forward-head-seconds`). A request with no body waits
/// `first_byte` for its head instead (`forward-first-byte-seconds`, at most `head`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Waits {
    pub(crate) open: Duration,
    pub(crate) idle: Duration,
    pub(crate) head: Duration,
    pub(crate) first_byte: Duration,
}

/// How far a forwarded request's body has gone down its member's connection: when a frame of it was last taken, and
/// when it ended, in nanoseconds after `start`. A request that carries none in its extensions has no body.
pub(crate) struct Progress {
    start: tokio::time::Instant,
    /// The request has no body: its head is waited for `first_byte` from when it is sent.
    bodyless: bool,
    last: AtomicU64,
    /// [`NOT_ENDED`] while the body is still going.
    ended: AtomicU64,
}

const NOT_ENDED: u64 = u64::MAX;

impl Progress {
    fn new(ended: bool) -> Self {
        Self {
            start: tokio::time::Instant::now(),
            bodyless: ended,
            last: AtomicU64::new(0),
            ended: AtomicU64::new(if ended { 0 } else { NOT_ENDED }),
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_nanos()).unwrap_or(NOT_ENDED - 1)
    }

    /// The request is handed to a connection now: its body has not been read by it yet, so the clocks start again.
    fn sending(&self) {
        let now = self.now();
        self.last.store(now, Ordering::Release);
        if self.ended.load(Ordering::Acquire) != NOT_ENDED {
            self.ended.store(now, Ordering::Release);
        }
    }

    fn moved(&self) {
        self.last.store(self.now(), Ordering::Release);
    }

    fn end(&self) {
        let now = self.now();
        let _ = self
            .ended
            .compare_exchange(NOT_ENDED, now, Ordering::AcqRel, Ordering::Acquire);
    }

    /// When the wait for the answer's head runs out as things stand: for a request with no body, `first_byte` after it
    /// was sent; else `idle` after the body last moved while it goes, and after it ended the later of that and `head`
    /// after the end, so the end of the body never shortens the wait.
    fn deadline(&self, waits: &Waits) -> tokio::time::Instant {
        let at = |nanos: u64, wait: Duration| self.start + Duration::from_nanos(nanos) + wait;
        let idle = at(self.last.load(Ordering::Acquire), waits.idle);
        match self.ended.load(Ordering::Acquire) {
            // `sending` set both clocks to when it was sent
            ended if self.bodyless => at(ended, waits.first_byte.min(waits.head)),
            NOT_ENDED => idle,
            ended => idle.max(at(ended, waits.head)),
        }
    }
}

/// A request body that tells its [`Progress`] as each frame is taken and when it ends.
struct Watched {
    body: tonic::body::Body,
    progress: Arc<Progress>,
}

impl hyper::body::Body for Watched {
    type Data = bytes::Bytes;
    type Error = <tonic::body::Body as hyper::body::Body>::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let polled = Pin::new(&mut self.body).poll_frame(cx);
        match &polled {
            std::task::Poll::Ready(Some(Ok(_))) => {
                self.progress.moved();
                // hyper asks is_end_stream after a frame and may not poll again
                if self.body.is_end_stream() {
                    self.progress.end();
                }
            }
            std::task::Poll::Ready(_) => self.progress.end(),
            std::task::Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        let ended = self.body.is_end_stream();
        if ended {
            self.progress.end();
        }
        ended
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

/// `body` as a forwarded request's, with the [`Progress`] to put in the request's extensions.
pub(crate) fn watched(body: tonic::body::Body) -> (tonic::body::Body, Arc<Progress>) {
    use hyper::body::Body as _;
    let progress = Arc::new(Progress::new(body.is_end_stream()));
    let body = tonic::body::Body::new(Watched {
        body,
        progress: Arc::clone(&progress),
    });
    (body, progress)
}

/// One try of `request` down `send`, within `waits`: the answer's head, the request back when it was not sent at all
/// (the connection was not ready), or why there was no answer.
async fn try_send(
    mut send: SendRequest<tonic::body::Body>,
    request: http::Request<tonic::body::Body>,
    waits: Waits,
) -> Result<http::Response<hyper::body::Incoming>, (Failed, Option<Box<http::Request<tonic::body::Body>>>)> {
    match tokio::time::timeout(waits.open, send.ready()).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return Err((
                Failed::Error(err.to_string(), send.is_closed()),
                Some(Box::new(request)),
            ))
        }
        Err(_) => return Err((Failed::TimedOut, Some(Box::new(request)))),
    }
    let progress = request
        .extensions()
        .get::<Arc<Progress>>()
        .cloned()
        .unwrap_or_else(|| Arc::new(Progress::new(true)));
    progress.sending();
    let answer = send.try_send_request(request);
    tokio::pin!(answer);
    loop {
        tokio::select! {
            answered = &mut answer => {
                return match answered {
                    Ok(response) => Ok(response),
                    Err(mut err) => {
                        let unsent = err.take_message().map(Box::new);
                        Err((Failed::Error(err.into_error().to_string(), send.is_closed()), unsent))
                    }
                };
            }
            // the deadline moves while the body goes, so it is read again when it comes
            () = tokio::time::sleep_until(progress.deadline(&waits)) => {
                if progress.deadline(&waits) <= tokio::time::Instant::now() {
                    return Err((Failed::TimedOut, None));
                }
            }
        }
    }
}

/// Send `request` down the link of `first`, and when it fails before any byte of an answer, once more down the link of
/// the member `other` gives in place of the failed one (plans/Edge-Node2.md §4.4): a request not sent at all, whatever
/// it is, or one `again` makes anew (a GET or HEAD with no body). Each try's outcome goes to `outcome`, true when it
/// was answered. The answer, or the last failure, with the member it came from.
pub(crate) async fn send_once_more(
    first: Picked,
    request: http::Request<tonic::body::Body>,
    again: Option<impl FnOnce() -> http::Request<tonic::body::Body>>,
    other: impl FnOnce(&str) -> Option<Picked>,
    waits: Waits,
    outcome: impl Fn(&Picked, bool),
) -> (Result<http::Response<hyper::body::Incoming>, Failed>, Picked) {
    let (failed, unsent) = match try_send(first.send.clone(), request, waits).await {
        Ok(response) => {
            outcome(&first, true);
            return (Ok(response), first);
        }
        Err(failed) => failed,
    };
    outcome(&first, false);
    let Some(request) = unsent.map(|request| *request).or_else(|| again.map(|again| again())) else {
        return (Err(failed), first);
    };
    let Some(second) = other(&first.member) else {
        return (Err(failed), first);
    };
    drop(first);
    match try_send(second.send.clone(), request, waits).await {
        Ok(response) => {
            outcome(&second, true);
            (Ok(response), second)
        }
        Err((failed, _)) => {
            outcome(&second, false);
            (Err(failed), second)
        }
    }
}

/// Send `request` down a link of `node`'s pool as the edge node's request for `client`, and its answer back.
async fn forward<B>(
    server: Arc<EdgeServer>,
    node: String,
    sticky: Option<String>,
    request: http::Request<B>,
    client: std::net::SocketAddr,
    https: bool,
) -> Response
where
    B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<axum::BoxError>,
{
    // only a WebSocket becomes a CONNECT down the link, by HTTP/1.1 upgrade or HTTP/2 extended CONNECT (RFC 8441): a
    // client's own CONNECT is never forwarded
    let h2_websocket = request.extensions().get::<ExtendedConnect>().is_some();
    if request.method() == http::Method::CONNECT && !h2_websocket {
        return plain(405, "CONNECT is not served here");
    }
    let websocket = h2_websocket || is_websocket(&request);
    let again = may_send_again(request.method(), request.body().is_end_stream(), websocket);
    let (mut parts, body) = request.into_parts();
    let Some(host) = parts
        .headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| parts.uri.authority().map(|authority| authority.as_str().to_string()))
    else {
        return plain(400, "no host");
    };
    let path = parts.uri.path_and_query().map_or("/", |path| path.as_str());
    let scheme = if https { "https" } else { "http" };
    let Ok(uri) = format!("{scheme}://{host}{path}").parse::<http::Uri>() else {
        return plain(400, "the request's host and path do not make a URL");
    };
    let mut headers = std::mem::take(&mut parts.headers);
    let key = headers.remove(header::SEC_WEBSOCKET_KEY);
    strip_hop_by_hop(&mut headers);
    strip_forwarding(&mut headers, server.client_ip_header());
    headers.remove(header::HOST);
    if let Ok(value) = HeaderValue::from_str(&client.to_string()) {
        headers.insert(CLIENT_HEADER, value);
    }
    headers.insert(PROTO_HEADER, HeaderValue::from_static(scheme));
    let on_upgrade = websocket
        .then(|| parts.extensions.remove::<hyper::upgrade::OnUpgrade>())
        .flatten();
    let method = if websocket { http::Method::CONNECT } else { parts.method };
    let make = move |body: tonic::body::Body, method: http::Method, uri: http::Uri, headers: HeaderMap| {
        let (body, progress) = watched(body);
        let mut down = http::Request::new(body);
        down.extensions_mut().insert(progress);
        *down.uri_mut() = uri;
        *down.version_mut() = http::Version::HTTP_2;
        *down.headers_mut() = headers;
        *down.method_mut() = method;
        if websocket {
            down.extensions_mut()
                .insert(hyper::ext::Protocol::from_static("websocket"));
        }
        down
    };
    // a GET or HEAD with no body may go once more, made anew from a copy of its head
    let again = again.then(|| {
        let (method, uri, headers) = (method.clone(), uri.clone(), headers.clone());
        move || make(tonic::body::Body::empty(), method, uri, headers)
    });
    let down = make(
        if websocket {
            tonic::body::Body::empty()
        } else {
            tonic::body::Body::new(body)
        },
        method,
        uri,
        headers,
    );
    // a WebSocket holds a stream of the link as a tunnel does
    let active = if websocket {
        match server.opened(&node, "websocket") {
            Some(active) => Some(active),
            None => return plain(503, "too many connections to this site's server; try again"),
        }
    } else {
        None
    };
    let affinity = sticky.as_deref().map_or(Affinity::Any, Affinity::Key);
    let Ok(picked) = server.link_of(&node, affinity) else {
        server.count("moqui.edgelink.offline", "terminate");
        return offline(&server.config().offline_page);
    };
    // each member a request is sent to counts it, as a tunnel's open and a call do
    server.count_member("moqui.edgelink.requests", "terminate", &node, &picked.member);
    let (answered, picked) = send_once_more(
        picked,
        down,
        again,
        |failed| {
            let other = server.link_other(&node, affinity, failed)?;
            server.count_member("moqui.edgelink.retries", "terminate", &node, &other.member);
            server.count_member("moqui.edgelink.requests", "terminate", &node, &other.member);
            Some(other)
        },
        Waits {
            open: server.config().open_timeout,
            idle: server.config().forward_idle,
            // a WebSocket is answered as it opens, with no body to wait for
            head: if websocket {
                server.config().open_timeout
            } else {
                server.config().forward_head
            },
            first_byte: if websocket {
                server.config().open_timeout
            } else {
                server.config().forward_first_byte
            },
        },
        |picked, ok| {
            if ok {
                server.succeeded(picked);
            } else {
                server.failed(picked, "terminate");
            }
        },
    )
    .await;
    let response = match answered {
        Ok(response) => response,
        Err(Failed::Error(err, closed)) => {
            tracing::debug!("Edge forward to {node} member {} failed: {err}", picked.member);
            return if closed {
                offline(&server.config().offline_page)
            } else {
                plain(502, "the site's server did not answer")
            };
        }
        Err(Failed::TimedOut) => return plain(504, "the site's server did not answer in time"),
    };
    if !websocket {
        server.count("moqui.edgelink.forwards", "terminate");
        // the request counts in flight on its member until its answer's body ends
        let flight = picked.flight;
        return response.map(move |body| axum::body::Body::new(Counted { body, _flight: flight }));
    }
    drop(picked);
    let Some(on_upgrade) = on_upgrade else {
        return plain(400, "a WebSocket upgrade the connection cannot take");
    };
    // an HTTP/1.1 upgrade carries its key; an HTTP/2 extended CONNECT has none, and is answered 200 (RFC 8441 §5)
    let key = match (h2_websocket, key) {
        (true, _) => None,
        (false, Some(key)) => Some(key),
        (false, None) => return plain(400, "a WebSocket upgrade without its key"),
    };
    if !response.status().is_success() {
        return response.map(axum::body::Body::new);
    }
    let mut answer = Response::new(axum::body::Body::empty());
    {
        let headers = answer.headers_mut();
        if let Some(key) = key {
            headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
            headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
            if let Ok(accept) = HeaderValue::from_str(&websocket_accept(key.as_bytes())) {
                headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
            }
        }
        for name in [header::SEC_WEBSOCKET_PROTOCOL, header::SEC_WEBSOCKET_EXTENSIONS] {
            if let Some(value) = response.headers().get(&name) {
                headers.insert(name, value.clone());
            }
        }
    }
    if !h2_websocket {
        *answer.status_mut() = http::StatusCode::SWITCHING_PROTOCOLS;
    }
    let local = hyper::upgrade::on(response);
    tokio::spawn(async move {
        let (client, local) = match tokio::join!(on_upgrade, local) {
            (Ok(client), Ok(local)) => (client, local),
            (Err(err), _) | (_, Err(err)) => {
                tracing::debug!("Edge WebSocket to {node}: the upgrade failed: {err}");
                return;
            }
        };
        let (mut client, mut local) = (TokioIo::new(client), TokioIo::new(local));
        let mut shutdown = server.shutdown();
        tokio::select! {
            piped = tokio::io::copy_bidirectional(&mut client, &mut local) => {
                if let Err(err) = piped {
                    tracing::trace!("Edge WebSocket to {node} ended: {err}");
                }
            }
            _ = shutdown.wait_for(|stopped| *stopped) => {}
        }
        drop(active);
    });
    answer
}

#[cfg(test)]
#[path = "web_tests.rs"]
mod tests;
