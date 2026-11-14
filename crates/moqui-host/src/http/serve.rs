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

//! Serving one connection and one request (plans/archive/Edge-Node.md §5.3): what a request is served with ([`WebApp`]), where
//! it came from ([`Peer`]), the per-request checks and the router or an edge node's forward ([`serve_request`]), shared
//! by the listeners, the edge link's down connection and its passthrough tunnels ([`serve_tunneled`]).
//!
//! A connection is HTTP/1.1, or HTTP/2 when its TLS handshake chose `h2` ([`Http`], [`run_connection`]). An HTTP/2
//! request is put in the form the app reads from HTTP/1.1 ([`h2_request_form`]); a gRPC call of a service the HTTPS
//! listener carries goes to it before the router ([`WebApp::grpc`]); a connection with no request for the header
//! timeout is closed with a GOAWAY, and so is one whose answer asks to close it (web protection's refusals).

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use axum::extract::ConnectInfo;
use axum::response::Response;
use axum::Router;
use http::header::{self, HeaderValue};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use moqui_lib::sync::lock;
use tower::ServiceExt as _;

use super::{forwarded_https, plain, ClientAddr, HttpsRequest, ListenLimits, TlsConnection};
use crate::guard::Guard;

/// Whether a `Host` header names a public-site host.
#[cfg(feature = "git")]
pub(crate) type SiteHost = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Whether a `Host` header names a host whose root screen refuses the framework's routes
/// (`moqui_core::screen::info::host_refuses_framework_routes`).
#[cfg(feature = "grpc")]
pub(crate) type RefusingHost = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What a request is served with: the router, and the limits and web protection in front of it. The listeners and the
/// edge link's down connection share it (plans/archive/Edge-Node.md §5.3).
#[derive(Clone)]
pub(crate) struct WebApp {
    pub router: Router,
    pub limits: ListenLimits,
    pub guard: Arc<Guard>,
    /// On an edge node: the terminate host names, whose requests go down a local node's link.
    pub edge: Option<Arc<crate::edge_link::WebFront>>,
    /// Whether a `Host` is a public-site host, where git requests at a page's URL go to the git handler
    /// ([`crate::git_http::page_request`]); `None` serves git only under `/git/`.
    #[cfg(feature = "git")]
    pub site_host: Option<SiteHost>,
    /// The gRPC services the HTTPS listener carries over HTTP/2 (the OTLP receiver's and the PASS authority's, those
    /// that are on), which a gRPC call goes to instead of the router.
    #[cfg(feature = "grpc")]
    pub grpc: Option<tonic::service::Routes>,
    /// Whether a host refuses the framework's routes, the gRPC services among them: a call to such a host goes to the
    /// router, which answers Not Found (plans/Edge-Node2.md §4.2).
    #[cfg(feature = "grpc")]
    pub refusing_host: Option<RefusingHost>,
}

impl WebApp {
    pub(crate) fn new(router: Router, limits: ListenLimits, guard: Arc<Guard>) -> Self {
        Self {
            router,
            limits,
            guard,
            edge: None,
            #[cfg(feature = "git")]
            site_host: None,
            #[cfg(feature = "grpc")]
            grpc: None,
            #[cfg(feature = "grpc")]
            refusing_host: None,
        }
    }
}

/// Which HTTP a connection speaks: HTTP/2 when its TLS handshake chose `h2`, else HTTP/1.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Http {
    One,
    Two,
}

impl Http {
    /// The protocol `session`'s handshake chose.
    pub(crate) fn of(session: &rustls::ServerConnection) -> Self {
        if session.alpn_protocol() == Some(b"h2".as_slice()) {
            Self::Two
        } else {
            Self::One
        }
    }
}

/// Where a request came from: the connection's peer, the client (the peer, or who a trusted proxy names), and whether
/// it came over TLS.
#[derive(Clone)]
pub(crate) struct Peer {
    pub remote: SocketAddr,
    pub client: IpAddr,
    /// Over this process's own TLS, or a link that carried a request the edge node took over TLS.
    pub secure: bool,
    /// The request is HTTPS (`secure`, or a trusted proxy's `X-Forwarded-Proto`).
    pub https: bool,
    /// The TLS session's exporter (RFC 9266), when the xmpp feature takes it at the handshake.
    pub exporter: Option<Arc<Vec<u8>>>,
    /// How the request reached this server (plans/Terminal-Fancy.md §8.2).
    pub via: moqui_core::web::RequestVia,
}

/// A terminate host's request on an edge node, taken from the request by the router's edge-forward layer, inside the
/// request log, and sent down its node's link.
#[derive(Clone)]
pub(crate) struct Forward {
    front: Arc<crate::edge_link::WebFront>,
    target: crate::edge_link::Target,
    peer: Peer,
}

impl Forward {
    pub(crate) async fn send(self, request: axum::extract::Request) -> Response {
        self.front.forward(self.target, request, &self.peer).await
    }
}

/// Marks a WebSocket opened by HTTP/2 extended CONNECT (RFC 8441) on the edge link's down connection: web protection
/// checks it as the GET of an HTTP/1.1 upgrade.
#[derive(Clone, Copy)]
pub(crate) struct ExtendedConnect;

/// What the edge link's down connection serves on a local node: the app, and for passthrough tunnels the HTTPS
/// listener's TLS settings, with the listeners' stop for their connections (plans/archive/Edge-Node.md §5.3).
#[derive(Clone)]
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) struct DownServe {
    pub app: WebApp,
    pub tls: Option<Arc<rustls::ServerConfig>>,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
}

/// The HTTP/1.1 connection settings of the listeners, from `limits`.
pub(crate) fn http1_builder(limits: &ListenLimits) -> hyper::server::conn::http1::Builder {
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header)
        .max_buf_size(limits.max_header_bytes)
        .max_headers(limits.max_headers);
    builder
}

/// HTTP/2 PINGs this often find a client that went away; one not answered in [`H2_PING_TIMEOUT`] closes the
/// connection.
const H2_PING_EVERY: Duration = Duration::from_secs(30);
const H2_PING_TIMEOUT: Duration = Duration::from_secs(20);
/// Streams a client may reset before the server has taken them, the rapid-reset bound (h2's default, set here).
const H2_PENDING_RESETS: usize = 20;
/// Streams the server may reset for a client's protocol errors before the connection is closed (hyper's default).
const H2_ERROR_RESETS: usize = 1024;
/// How often, at most, an HTTP/2 connection checks whether it has been idle for the header timeout.
const H2_IDLE_CHECK: Duration = Duration::from_secs(5);

/// The HTTP/2 connection settings of the HTTPS listener, from `limits`: WebSockets by extended CONNECT (RFC 8441),
/// `http2-max-streams` streams at once, and the request head bounded by `max-header-bytes`.
pub(crate) fn http2_builder(limits: &ListenLimits) -> hyper::server::conn::http2::Builder<TokioExecutor> {
    let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
    builder
        .timer(TokioTimer::new())
        .keep_alive_interval(H2_PING_EVERY)
        .keep_alive_timeout(H2_PING_TIMEOUT)
        .enable_connect_protocol()
        .max_concurrent_streams(limits.http2_max_streams)
        .max_header_list_size(u32::try_from(limits.max_header_bytes).unwrap_or(u32::MAX))
        .max_pending_accept_reset_streams(H2_PENDING_RESETS)
        .max_local_error_reset_streams(H2_ERROR_RESETS);
    builder
}

/// An HTTP/2 request in the form the app reads from HTTP/1.1: `Host` from `:authority` when it has none, the URI in
/// origin form, and a WebSocket's extended CONNECT (RFC 8441) marked [`ExtendedConnect`].
pub(crate) fn h2_request_form<B>(request: &mut http::Request<B>) {
    if !request.headers().contains_key(header::HOST) {
        if let Some(host) = request
            .uri()
            .authority()
            .and_then(|authority| HeaderValue::from_str(authority.as_str()).ok())
        {
            request.headers_mut().insert(header::HOST, host);
        }
    }
    let path = request.uri().path_and_query().map_or("/", |path| path.as_str());
    if let Ok(origin_form) = path.parse::<http::Uri>() {
        *request.uri_mut() = origin_form;
    }
    let websocket = request.method() == http::Method::CONNECT
        && request
            .extensions()
            .get::<hyper::ext::Protocol>()
            .is_some_and(|protocol| protocol.as_str() == "websocket");
    if websocket {
        request.extensions_mut().insert(ExtendedConnect);
    }
}

/// Requests in flight on one connection, and when the last one ended, to close a connection that sits idle.
pub(crate) struct Activity {
    in_flight: AtomicUsize,
    last: Mutex<Instant>,
}

impl Activity {
    pub(crate) fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            last: Mutex::new(Instant::now()),
        }
    }

    pub(crate) fn started(&self) {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn ended(&self) {
        *lock(&self.last) = Instant::now();
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
    }

    /// How long the connection has had no request in flight; zero while one is.
    pub(crate) fn idle_for(&self) -> Duration {
        if self.in_flight.load(Ordering::SeqCst) > 0 {
            return Duration::ZERO;
        }
        lock(&self.last).elapsed()
    }
}

/// Serve an HTTPS connection that came another way than the listener as the listener serves its own: the edge link's
/// passthrough tunnel, whose client `remote` its PROXY header named (plans/archive/Edge-Node.md §5.3). The caller has had the
/// guard admit the client, before the TLS handshake; `closing` turning true shuts the connection down gracefully.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) async fn serve_tunneled<S>(
    stream: tokio_rustls::server::TlsStream<S>,
    remote: SocketAddr,
    web: WebApp,
    closing: tokio::sync::watch::Receiver<bool>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    #[cfg(feature = "xmpp")]
    let exporter = moqui_xmpp::conn::exporter(stream.get_ref().1).map(Arc::new);
    #[cfg(not(feature = "xmpp"))]
    let exporter = None;
    let http = Http::of(stream.get_ref().1);
    let serving = Serving {
        remote,
        builder: http1_builder(&web.limits),
        web,
        closing,
        exporter,
        hold: None,
        via: moqui_core::web::RequestVia::Edge,
    };
    run_connection(stream, true, http, serving).await;
}

/// What one connection is served with.
pub(crate) struct Serving {
    pub remote: SocketAddr,
    pub web: WebApp,
    pub builder: hyper::server::conn::http1::Builder,
    pub closing: tokio::sync::watch::Receiver<bool>,
    /// The TLS session's exporter (RFC 9266), when the xmpp feature takes it at the handshake.
    pub exporter: Option<Arc<Vec<u8>>>,
    /// The listener's slots the connection holds, given back when its socket closes ([`WriteStall`]).
    pub hold: Option<ConnectionHold>,
    /// How the connection reached this server.
    pub via: moqui_core::web::RequestVia,
}

/// Serve one connection as `http` until it closes or `serving.closing` turns true, then shut it down gracefully.
pub(crate) async fn run_connection<S>(stream: S, secure: bool, http: Http, serving: Serving)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    match http {
        Http::One => run_http1(stream, secure, serving).await,
        Http::Two => run_http2(stream, serving).await,
    }
}

async fn run_http1<S>(stream: S, secure: bool, serving: Serving)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Serving {
        remote,
        web,
        builder,
        mut closing,
        exporter,
        hold,
        via,
    } = serving;
    let io = WriteStall::new(Lingering::new(stream), web.limits.write, hold);
    let service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
        let client = web.guard.client_of(remote.ip(), request.headers());
        let https = secure || forwarded_https(request.headers(), remote.ip(), web.guard.conf().addressing());
        let peer = Peer {
            remote,
            client,
            secure,
            https,
            exporter: exporter.clone(),
            via,
        };
        serve_request(&web, request, &peer)
    });
    let connection = builder.serve_connection(TokioIo::new(io), service).with_upgrades();
    let mut connection = std::pin::pin!(connection);
    tokio::select! {
        result = connection.as_mut() => {
            if let Err(err) = result {
                tracing::trace!("http connection from {remote} ended: {err}");
            }
        }
        _ = closing.changed() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

type Answer = std::pin::Pin<Box<dyn Future<Output = Result<Response, std::convert::Infallible>> + Send>>;

/// An HTTP/2 connection, which is always over this process's TLS.
async fn run_http2<S>(stream: S, serving: Serving)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Serving {
        remote,
        web,
        mut closing,
        exporter,
        hold,
        via,
        ..
    } = serving;
    let io = WriteStall::new(Lingering::new(stream), web.limits.write, hold);
    let idle_limit = web.limits.header;
    let builder = http2_builder(&web.limits);
    let activity = Arc::new(Activity::new());
    // an answer that asks to close its connection (a web protection refusal) turns this true
    let (close, mut close_asked) = tokio::sync::watch::channel(false);
    let service = {
        let activity = Arc::clone(&activity);
        hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| -> Answer {
            activity.started();
            h2_request_form(&mut request);
            let answer = h2_answer(&web, request, remote, exporter.clone(), via);
            let (activity, close) = (Arc::clone(&activity), close.clone());
            Box::pin(async move {
                let mut response = answer.await?;
                activity.ended();
                // HTTP/2 has no Connection header: the connection is shut down with a GOAWAY instead
                let closes = response
                    .headers()
                    .get(header::CONNECTION)
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.eq_ignore_ascii_case("close"));
                if closes {
                    response.headers_mut().remove(header::CONNECTION);
                    let _ = close.send(true);
                }
                Ok(response)
            })
        })
    };
    let connection = builder.serve_connection(TokioIo::new(io), service);
    let mut connection = std::pin::pin!(connection);
    let mut check = tokio::time::interval(idle_limit.map_or(H2_IDLE_CHECK, |limit| limit.min(H2_IDLE_CHECK)));
    let mut shutting = false;
    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(err) = result {
                    tracing::trace!("http/2 connection from {remote} ended: {err}");
                }
                return;
            }
            _ = closing.changed(), if !shutting => {
                shutting = true;
                connection.as_mut().graceful_shutdown();
            }
            _ = close_asked.changed(), if !shutting => {
                shutting = true;
                connection.as_mut().graceful_shutdown();
            }
            _ = check.tick(), if !shutting && idle_limit.is_some() => {
                if idle_limit.is_some_and(|limit| activity.idle_for() >= limit) {
                    shutting = true;
                    connection.as_mut().graceful_shutdown();
                }
            }
        }
    }
}

/// One HTTP/2 request: a gRPC call of a service the listener carries, else [`serve_request`].
fn h2_answer(
    web: &WebApp,
    request: hyper::Request<hyper::body::Incoming>,
    remote: SocketAddr,
    exporter: Option<Arc<Vec<u8>>>,
    via: moqui_core::web::RequestVia,
) -> Answer {
    #[cfg(feature = "grpc")]
    if let Some(grpc) = grpc_routes(web, &request) {
        let mut request = request;
        // the PASS authority reads the client's address from this (`crate::pass_authority`)
        request.extensions_mut().insert(remote);
        let call = grpc.oneshot(request);
        return Box::pin(async move {
            match call.await {
                Ok(response) => Ok(response.map(axum::body::Body::new)),
                Err(never) => match never {},
            }
        });
    }
    let client = web.guard.client_of(remote.ip(), request.headers());
    let peer = Peer {
        remote,
        client,
        secure: true,
        https: true,
        exporter,
        via,
    };
    Box::pin(serve_request(web, request, &peer))
}

/// The listener's gRPC services for `request`, when it is a gRPC call (not gRPC-Web) and they are on. A request for an
/// edge node's terminate host is not one: it goes down its node's link as any other.
#[cfg(feature = "grpc")]
fn grpc_routes<B>(web: &WebApp, request: &http::Request<B>) -> Option<tonic::service::Routes> {
    let routes = web.grpc.as_ref()?;
    let grpc = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value == "application/grpc"
                || value.starts_with("application/grpc+")
                || value.starts_with("application/grpc;")
        });
    if !grpc {
        return None;
    }
    if web.edge.as_ref().is_some_and(|front| front.target(request).is_some()) {
        return None;
    }
    if let Some(refusing) = &web.refusing_host {
        let host = request
            .uri()
            .host()
            .or_else(|| {
                request
                    .headers()
                    .get(header::HOST)
                    .and_then(|value| value.to_str().ok())
            })
            .unwrap_or("");
        if refusing(host) {
            return None;
        }
    }
    Some(routes.clone())
}

/// The URI under `/git/` of a request that is git's at a page's URL ([`crate::git_http::page_request`]), with its
/// query.
#[cfg(feature = "git")]
fn git_page_uri<B>(web: &WebApp, request: &hyper::Request<B>) -> Option<http::Uri> {
    let is_site_host = web.site_host.as_ref()?;
    let uri = request.uri();
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.host())
        .unwrap_or("");
    let path = crate::git_http::page_request(
        request.method().as_str(),
        uri.path(),
        uri.query(),
        host,
        is_site_host.as_ref(),
    )?;
    let mut parts = uri.clone().into_parts();
    let path_and_query = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    parts.path_and_query = Some(path_and_query.parse().ok()?);
    http::Uri::from_parts(parts).ok()
}

/// Serve one request with `web`, from `peer`: the URI length and web protection checks, then on an edge node the
/// forward of a terminate host down its link, else the router, either under the request deadline. The listeners'
/// connections and the edge link's down connection call it (plans/archive/Edge-Node.md §5.3).
pub(crate) fn serve_request<B>(
    web: &WebApp,
    mut request: hyper::Request<B>,
    peer: &Peer,
) -> impl Future<Output = Result<Response, std::convert::Infallible>> + Send + 'static
where
    B: hyper::body::Body<Data = axum::body::Bytes> + Send + 'static,
    B::Error: Into<axum::BoxError>,
{
    let (remote, client) = (peer.remote, peer.client);
    #[cfg(feature = "xmpp")]
    if let Some(exporter) = &peer.exporter {
        request
            .extensions_mut()
            .insert(crate::xmpp::ws::TlsExporter(Arc::clone(exporter)));
    }
    #[cfg(not(feature = "xmpp"))]
    let _ = &peer.exporter;
    request.extensions_mut().insert(ConnectInfo(remote));
    request.extensions_mut().insert(ClientAddr(client));
    request.extensions_mut().insert(peer.via);
    if peer.secure {
        request.extensions_mut().insert(TlsConnection);
    }
    if peer.https {
        request.extensions_mut().insert(HttpsRequest);
    }
    let limits = &web.limits;
    // the host a request is for, for the web protection rules of that host (plans/Edge-Node2.md §4.3)
    let host = crate::guard::request_host(
        request
            .headers()
            .get(axum::http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .or_else(|| request.uri().authority().map(axum::http::uri::Authority::as_str))
            .unwrap_or_default(),
    );
    let uri_length = request.uri().path_and_query().map_or(0, |whole| whole.as_str().len());
    let refused = if uri_length > limits.max_uri {
        Some(plain(414, "the request URI is too long"))
    } else {
        // a WebSocket over HTTP/2 extended CONNECT is checked as the GET of an HTTP/1.1 upgrade
        let method = if request.extensions().get::<ExtendedConnect>().is_some() {
            "GET"
        } else {
            request.method().as_str()
        };
        web.guard
            .check(
                remote.ip(),
                client,
                &host,
                method,
                request.uri().path(),
                request.headers(),
            )
            .err()
            .map(|refused| *refused.response)
    };
    // a websocket, a git clone or push (which has its own idle timeout), an upload (whose screen has the
    // deadline once it has arrived), and a model call of the LLM gateway (whose own timeout bounds it, answered in
    // the OpenAI shape) run as long as they need
    let upload = request
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_start().to_ascii_lowercase().starts_with("multipart/"));
    let socket = matches!(
        request.uri().path(),
        "/notws" | "/termws" | "/xmpp-websocket" | "/callws"
    );
    // git at a page's URL on a public-site host is served as the same request under /git/, with its exemption from the
    // deadline (docs/interfaces/Git-Hosting.md, Clone URLs); web protection has checked it by the path it came with,
    // and its path rules are asked again for the path it is served at
    #[cfg(feature = "git")]
    let refused = match refused {
        Some(refused) => Some(refused),
        None => match git_page_uri(web, &request) {
            // a path rule that denies the path it is served at stops it as it stops a request under /git/
            Some(uri) => match web.guard.check_served_path(client, &host, uri.path()) {
                Ok(()) => {
                    *request.uri_mut() = uri;
                    request.extensions_mut().insert(crate::git_http::GitPageRequest);
                    None
                }
                Err(refused) => Some(*refused.response),
            },
            None => None,
        },
    };
    let path = request.uri().path();
    let deadline = limits.request.filter(|_| {
        !socket
            && !path.starts_with("/xmpp-upload/")
            && !path.starts_with("/git/")
            && !path.starts_with("/dav/")
            && !path.starts_with("/llm/v1")
            && !upload
    });
    // on an edge node, a terminate host's request goes through the router's request log and telemetry like any other,
    // and down its node's link at the router's edge-forward layer
    let mut refused = refused;
    if refused.is_none() {
        let target = web
            .edge
            .as_ref()
            .and_then(|front| Some((front, front.target(&request)?)));
        match target {
            // a ban the target's own node told this edge node holds for its hosts (plans/Edge-Node2.md §4.3)
            Some((_, target)) if target.node().is_some_and(|node| web.guard.banned_for(node, client)) => {
                refused = Some(plain(403, "forbidden"));
            }
            Some((front, target)) => {
                request.extensions_mut().insert(Forward {
                    front: Arc::clone(front),
                    target,
                    peer: peer.clone(),
                });
            }
            None => {}
        }
    }
    let call = refused.is_none().then(|| {
        let routed = web.router.clone().oneshot(request);
        async move {
            match routed.await {
                Ok(response) => response,
                Err(never) => match never {},
            }
        }
    });
    async move {
        let Some(call) = call else {
            return Ok(refused.unwrap_or_else(|| plain(500, "refused")));
        };
        Ok(match deadline {
            Some(deadline) => match tokio::time::timeout(deadline, call).await {
                Ok(response) => response,
                Err(_) => plain(504, "the request took too long"),
            },
            None => call.await,
        })
    }
}

/// What a connection holds until its socket closes: its slot of the listeners' connections and its client's slot.
pub(crate) type ConnectionHold = Box<dyn Send + Sync>;

/// A stream whose write fails once it has made no progress for `limit`, so a client that stops reading cannot keep its
/// connection, or a push loop's write, past it (`plans/archive/High-Level-Review.md` HL-027). A write that moves any bytes starts
/// the time again: a slow download that is still progressing goes on, and so does backpressure. The stream carries the
/// connection's [`ConnectionHold`], so an upgraded socket (a WebSocket hyper handed off) keeps its connection slots
/// until it closes.
pub(crate) struct WriteStall<S> {
    inner: S,
    limit: Option<Duration>,
    stalled: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    _hold: Option<ConnectionHold>,
}

impl<S> WriteStall<S> {
    pub(crate) fn new(inner: S, limit: Option<Duration>, hold: Option<ConnectionHold>) -> Self {
        Self {
            inner,
            limit,
            stalled: None,
            _hold: hold,
        }
    }

    /// After a write or flush that could not go on: the error once it has waited `limit`, else wake at that time.
    fn stall(&mut self, cx: &mut std::task::Context<'_>) -> Option<std::io::Error> {
        let limit = self.limit?;
        let sleep = self.stalled.get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        sleep.as_mut().poll(cx).is_ready().then(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the client took nothing for the write timeout",
            )
        })
    }

    fn progress<T>(
        &mut self,
        polled: std::task::Poll<std::io::Result<T>>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<T>> {
        match polled {
            std::task::Poll::Pending => match self.stall(cx) {
                Some(err) => std::task::Poll::Ready(Err(err)),
                None => std::task::Poll::Pending,
            },
            ready => {
                self.stalled = None;
                ready
            }
        }
    }
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for WriteStall<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for WriteStall<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_write(cx, buf);
        self.progress(polled, cx)
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.progress(polled, cx)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_flush(cx);
        self.progress(polled, cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// How long a closed connection the client has not closed yet is read and dropped, after the write side is shut down.
const LINGER: Duration = Duration::from_secs(2);

/// A connection that closes as a lingering close does. A server that closes with request bytes still unread (a
/// request head over `max-header-bytes`, answered 431) sends a reset, and a macOS or BSD client then discards the
/// answer it has not read yet. So when the connection is dropped before the client has closed its side, the write
/// side is shut down first and whatever the client still sends is read and dropped, for up to `LINGER`.
struct Lingering<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> {
    inner: Option<S>,
    /// The client closed its side, or the connection failed: nothing is left to drain.
    ended: bool,
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> Lingering<S> {
    fn new(inner: S) -> Self {
        Self {
            inner: Some(inner),
            ended: false,
        }
    }

    fn inner(&mut self) -> std::pin::Pin<&mut S> {
        std::pin::Pin::new(self.inner.as_mut().expect("the stream is taken only on drop"))
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> tokio::io::AsyncRead for Lingering<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = self.inner().poll_read(cx, buf);
        match &polled {
            std::task::Poll::Ready(Ok(())) if buf.filled().len() == before && buf.remaining() > 0 => self.ended = true,
            std::task::Poll::Ready(Err(_)) => self.ended = true,
            _ => {}
        }
        polled
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> tokio::io::AsyncWrite for Lingering<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = self.inner().poll_write(cx, buf);
        if matches!(polled, std::task::Poll::Ready(Err(_))) {
            self.ended = true;
        }
        polled
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = self.inner().poll_write_vectored(cx, bufs);
        if matches!(polled, std::task::Poll::Ready(Err(_))) {
            self.ended = true;
        }
        polled
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| inner.is_write_vectored())
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner().poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.inner().poll_shutdown(cx)
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> Drop for Lingering<S> {
    fn drop(&mut self) {
        let Some(mut stream) = self.inner.take() else {
            return;
        };
        if self.ended {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        runtime.spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            let _ = tokio::time::timeout(LINGER, async {
                let _ = stream.shutdown().await;
                let mut sink = [0u8; 4096];
                while matches!(stream.read(&mut sink).await, Ok(read) if read > 0) {}
            })
            .await;
        });
    }
}

#[cfg(all(test, feature = "grpc"))]
#[path = "serve_tests.rs"]
mod tests;
