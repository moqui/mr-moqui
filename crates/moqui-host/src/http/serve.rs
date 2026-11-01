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

//! Serving one connection and one request (plans/Edge-Node.md §5.3): what a request is served with ([`WebApp`]), where
//! it came from ([`Peer`]), the per-request checks and the router or an edge node's forward ([`serve_request`]), shared
//! by the listeners, the edge link's down connection and its passthrough tunnels ([`serve_tunneled`]).

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::response::Response;
use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use tower::ServiceExt as _;

use super::{forwarded_https, plain, ClientAddr, HttpsRequest, ListenLimits, TlsConnection};
use crate::guard::Guard;

/// What a request is served with: the router, and the limits and web protection in front of it. The listeners and the
/// edge link's down connection share it (plans/Edge-Node.md §5.3).
#[derive(Clone)]
pub(crate) struct WebApp {
    pub router: Router,
    pub limits: ListenLimits,
    pub guard: Arc<Guard>,
    /// On an edge node: the terminate host names, whose requests go down a local node's link.
    pub edge: Option<Arc<crate::edge_link::WebFront>>,
}

impl WebApp {
    pub(crate) fn new(router: Router, limits: ListenLimits, guard: Arc<Guard>) -> Self {
        Self {
            router,
            limits,
            guard,
            edge: None,
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
/// listener's TLS settings, with the listeners' stop for their connections (plans/Edge-Node.md §5.3).
#[derive(Clone)]
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) struct DownServe {
    pub app: WebApp,
    pub tls: Option<Arc<rustls::ServerConfig>>,
    pub shutdown: tokio::sync::watch::Receiver<bool>,
}

/// The HTTP/1.1 connection settings of the listeners, from `limits`.
pub(super) fn http1_builder(limits: &ListenLimits) -> hyper::server::conn::http1::Builder {
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header)
        .max_buf_size(limits.max_header_bytes)
        .max_headers(limits.max_headers);
    builder
}

/// Serve an HTTPS connection that came another way than the listener as the listener serves its own: the edge link's
/// passthrough tunnel, whose client `remote` its PROXY header named (plans/Edge-Node.md §5.3). The caller has had the
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
    let serving = Serving {
        remote,
        builder: http1_builder(&web.limits),
        web,
        closing,
        exporter,
    };
    run_connection(stream, true, serving).await;
}

/// What one connection is served with.
pub(super) struct Serving {
    pub remote: SocketAddr,
    pub web: WebApp,
    pub builder: hyper::server::conn::http1::Builder,
    pub closing: tokio::sync::watch::Receiver<bool>,
    /// The TLS session's exporter (RFC 9266), when the xmpp feature takes it at the handshake.
    pub exporter: Option<Arc<Vec<u8>>>,
}

pub(super) async fn run_connection<S>(stream: S, secure: bool, serving: Serving)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Serving {
        remote,
        web,
        builder,
        mut closing,
        exporter,
    } = serving;
    let service = hyper::service::service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
        let client = web.guard.client_of(remote.ip(), request.headers());
        let https = secure || forwarded_https(request.headers(), remote.ip(), web.guard.conf.addressing());
        let peer = Peer {
            remote,
            client,
            secure,
            https,
            exporter: exporter.clone(),
        };
        serve_request(&web, request, &peer)
    });
    let connection = builder
        .serve_connection(TokioIo::new(Lingering::new(stream)), service)
        .with_upgrades();
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

/// Serve one request with `web`, from `peer`: the URI length and web protection checks, then on an edge node the
/// forward of a terminate host down its link, else the router, either under the request deadline. The listeners'
/// connections and the edge link's down connection call it (plans/Edge-Node.md §5.3).
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
    if peer.secure {
        request.extensions_mut().insert(TlsConnection);
    }
    if peer.https {
        request.extensions_mut().insert(HttpsRequest);
    }
    let limits = &web.limits;
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
            .check(remote.ip(), client, method, request.uri().path(), request.headers())
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
    if refused.is_none() {
        let target = web
            .edge
            .as_ref()
            .and_then(|front| Some((front, front.target(&request)?)));
        if let Some((front, target)) = target {
            request.extensions_mut().insert(Forward {
                front: Arc::clone(front),
                target,
                peer: peer.clone(),
            });
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
