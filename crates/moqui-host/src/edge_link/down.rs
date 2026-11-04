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

//! The local node's side of the down connection (plans/Edge-Node.md §5.1, §5.3): one HTTP/2 server with extended
//! CONNECT (RFC 8441) that serves the edge node
//! - the link's own gRPC calls (`content-type: application/grpc` on `/moqui.edge.v1.LocalLink/`, with no client named),
//!   and the PASS authority's on `/moqui.pass.v1.PassAuthority/` the same way (§5.5), for which the edge node is the
//!   client;
//! - byte tunnels (`:protocol` `moqui-tunnel`): a passthrough connection, whose PROXY v2 header names the client and
//!   whose TLS this node ends with the HTTPS listener's own settings, served as a connection to that listener; or a
//!   connection to this node's SSH server, or an XMPP client's (STARTTLS or direct TLS) to its XMPP server. The kind
//!   picks this node's own listener, never an address the edge node names. A PROXY header is read here only, never on
//!   an open socket. An SSH or XMPP tunnel is served in process as a connection from the client the edge node names;
//! - every request that names a client in [`CLIENT_HEADER`], forwarded from a terminate host, whatever its path or
//!   content type: this node's web app, as if the client had come straight
//!   here. The client's address is the one the edge node sends in [`CLIENT_HEADER`], believed only because the request
//!   came over this authenticated connection; that header and every other forwarding header are removed before the
//!   app sees the request, so web protection and the logs see the client, never a header a client wrote.

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::response::Response;
use http::header::{self, HeaderValue};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync::lock;
use tower::ServiceExt as _;

use super::{
    CLIENT_HEADER, PROTO_HEADER, TUNNEL_HEADER, TUNNEL_HTTPS, TUNNEL_PROTOCOL, TUNNEL_SSH, TUNNEL_XMPP, TUNNEL_XMPPS,
};
use crate::http::{plain, DownServe, ExtendedConnect, Peer};

/// How long a passthrough tunnel's PROXY header may take, and its TLS handshake when the listener has no header
/// timeout.
const TUNNEL_HANDSHAKE: Duration = Duration::from_secs(10);
/// HTTP/2 streams at once on the down connection: forwarded requests, WebSockets and tunnels together.
const MAX_STREAMS: u32 = 1024;

/// What a local node's down connection serves besides the link's own calls: its web app and, for tunnels, its HTTPS
/// listener's TLS settings and its SSH server. From [`local_web`] of a running server.
#[derive(Clone)]
pub struct LocalWeb {
    serve: DownServe,
    factory: Weak<ExecutionContextFactory>,
}

impl LocalWeb {
    /// The server's factory, while it runs: what the link's queue, delegated calls and replication work on.
    pub(crate) fn factory(&self) -> Weak<ExecutionContextFactory> {
        Weak::clone(&self.factory)
    }
}

/// A server's [`LocalWeb`], while its listeners run.
#[derive(Default)]
struct Slot(Mutex<Option<LocalWeb>>);

/// Keep `serve` as `factory`'s [`LocalWeb`] until [`clear`].
pub(crate) fn install(factory: &Arc<ExecutionContextFactory>, serve: DownServe) {
    let web = LocalWeb {
        serve,
        factory: Arc::downgrade(factory),
    };
    *lock(&factory.extension(|_| Slot::default()).0) = Some(web);
}

/// Forget `factory`'s [`LocalWeb`]: its app holds the factory, so it goes when the listeners stop.
pub(crate) fn clear(factory: &ExecutionContextFactory) {
    lock(&factory.extension(|_| Slot::default()).0).take();
}

/// The web app and listeners of the server on `factory` for a down connection, while its listeners run.
pub fn local_web(factory: &ExecutionContextFactory) -> Option<LocalWeb> {
    lock(&factory.extension(|_| Slot::default()).0).clone()
}

/// The HTTP/2 settings of the down connection's server.
pub(crate) fn builder(
    keepalive: Duration,
    web: Option<&LocalWeb>,
) -> hyper::server::conn::http2::Builder<hyper_util::rt::TokioExecutor> {
    let mut builder = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new());
    builder
        .timer(hyper_util::rt::TokioTimer::new())
        .keep_alive_interval(keepalive)
        .keep_alive_timeout(keepalive)
        .enable_connect_protocol()
        .adaptive_window(true)
        .max_concurrent_streams(MAX_STREAMS);
    if let Some(web) = web {
        let header_bytes = u32::try_from(web.serve.app.limits.max_header_bytes).unwrap_or(u32::MAX);
        builder.max_header_list_size(header_bytes);
    }
    builder
}

type Answer = Pin<Box<dyn Future<Output = Result<Response, Infallible>> + Send>>;

/// The down connection's service: `grpc` for the edge node's calls (the link's and the PASS authority's), `web` for the
/// rest.
pub(crate) fn service<G>(
    grpc: G,
    web: Option<LocalWeb>,
) -> impl hyper::service::Service<http::Request<Incoming>, Response = Response, Error = Infallible, Future = Answer>
where
    G: tower::Service<http::Request<Incoming>, Response = http::Response<tonic::body::Body>, Error = Infallible>
        + Clone
        + Send
        + 'static,
    G::Future: Send,
{
    hyper::service::service_fn(move |request: http::Request<Incoming>| -> Answer {
        // only the edge node opens a tunnel: a client's CONNECT is refused there unless it is a WebSocket
        if is_tunnel(&request) {
            let answer = tunnel(request, web.as_ref());
            return Box::pin(async move { Ok(answer) });
        }
        // a forwarded request goes to the app whatever it looks like, so a client cannot reach the link's calls
        if request.headers().contains_key(CLIENT_HEADER) {
            return match &web {
                Some(web) => Box::pin(forwarded(request, web)),
                None => Box::pin(async { Ok(plain(503, "this node serves no web app on its edge link")) }),
            };
        }
        if is_link_call(&request) {
            let call = grpc.clone().oneshot(request);
            return Box::pin(async move {
                let response = match call.await {
                    Ok(response) => response,
                    Err(never) => match never {},
                };
                Ok(response.map(axum::body::Body::new))
            });
        }
        Box::pin(async { Ok(plain(404, "not a call of the edge link")) })
    })
}

/// The path prefixes of the calls the edge node makes on the down connection: the link's own, and the PASS authority's
/// (plans/Edge-Node.md §5.5, `crate::pass_authority`), whose client is the edge node.
const LINK_CALLS: [&str; 2] = ["/moqui.edge.v1.LocalLink/", crate::pass_authority::PATH_PREFIX];

/// A call of the edge node's own: gRPC on one of [`LINK_CALLS`], with no client named (checked before).
fn is_link_call<B>(request: &http::Request<B>) -> bool {
    let grpc = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/grpc"));
    let path = request.uri().path();
    grpc && LINK_CALLS.iter().any(|prefix| path.starts_with(prefix))
}

fn protocol<B>(request: &http::Request<B>) -> Option<&str> {
    (request.method() == http::Method::CONNECT)
        .then(|| request.extensions().get::<hyper::ext::Protocol>())
        .flatten()
        .map(hyper::ext::Protocol::as_str)
}

fn is_tunnel<B>(request: &http::Request<B>) -> bool {
    protocol(request) == Some(TUNNEL_PROTOCOL)
}

/// A request from a terminate host, as a request to this node's app from the client the edge node names.
fn forwarded(
    mut request: http::Request<Incoming>,
    web: &LocalWeb,
) -> impl Future<Output = Result<Response, Infallible>> + Send + 'static {
    let headers = request.headers();
    let client: Option<SocketAddr> = headers
        .get(CLIENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let https = headers
        .get(PROTO_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "https");
    let ready = match client {
        Some(client) => {
            let app = &web.serve.app;
            let headers = request.headers_mut();
            super::web::strip_forwarding(headers, app.guard.conf().addressing().header());
            // HTTP/2 names the host in :authority; the app reads Host, as from an HTTP/1.1 client
            if !headers.contains_key(header::HOST) {
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
            if protocol(&request) == Some("websocket") {
                request.extensions_mut().insert(ExtendedConnect);
            }
            let peer = Peer {
                remote: client,
                client: client.ip(),
                secure: https,
                https,
                exporter: None,
            };
            Ok(crate::http::serve_request(app, request, &peer))
        }
        None => Err(plain(400, "the client address the edge node named does not parse")),
    };
    async move {
        match ready {
            Ok(call) => call.await,
            Err(refused) => Ok(refused),
        }
    }
}

/// Whether this node runs an SSH server that takes a tunnel stream in process.
fn ssh_running(factory: &ExecutionContextFactory) -> bool {
    #[cfg(feature = "ssh")]
    {
        crate::ssh::server::running(factory)
    }
    #[cfg(not(feature = "ssh"))]
    {
        let _ = factory;
        false
    }
}

/// Serve a tunnel stream as an SSH connection from `client`, in process.
fn ssh<S>(factory: &ExecutionContextFactory, stream: S, client: SocketAddr)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    #[cfg(feature = "ssh")]
    {
        tracing::debug!("edge SSH tunnel from {client}");
        if !crate::ssh::server::serve_stream(factory, stream, client) {
            tracing::debug!("edge SSH tunnel from {client}: the SSH server has stopped");
        }
    }
    #[cfg(not(feature = "ssh"))]
    {
        let _ = (factory, stream, client);
    }
}

/// Where a tunnel goes on this node.
enum Destination {
    Https(LocalWeb),
    /// The SSH server, as a connection from the client the edge node names.
    Ssh(Arc<ExecutionContextFactory>, SocketAddr),
    /// The XMPP server, as a client connection from the client the edge node names: direct TLS (`xmpps`) or STARTTLS.
    Xmpp {
        factory: Arc<ExecutionContextFactory>,
        client: SocketAddr,
        direct: bool,
    },
}

/// Take a tunnel when its kind names a listener this node has, and serve it once the stream is open.
fn tunnel(mut request: http::Request<Incoming>, web: Option<&LocalWeb>) -> Response {
    let kind = request
        .headers()
        .get(TUNNEL_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let client: Option<SocketAddr> = request
        .headers()
        .get(CLIENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok());
    let destination = match (kind, web) {
        (_, None) => return plain(404, "this node serves no tunnels on its edge link"),
        (TUNNEL_HTTPS, Some(web)) if web.serve.tls.is_some() => Destination::Https(web.clone()),
        (TUNNEL_HTTPS, Some(_)) => return plain(404, "this node has no HTTPS listener"),
        (TUNNEL_SSH, Some(web)) => match web.factory.upgrade().filter(|factory| ssh_running(factory)) {
            None => return plain(404, "this node has no SSH server"),
            Some(factory) => match client {
                Some(client) => Destination::Ssh(factory, client),
                None => return plain(400, "the edge node named no client"),
            },
        },
        (TUNNEL_XMPP | TUNNEL_XMPPS, Some(web)) => {
            let direct = kind == TUNNEL_XMPPS;
            match web
                .factory
                .upgrade()
                .filter(|factory| crate::xmpp::running(factory, direct))
            {
                None => return plain(404, "this node has no XMPP server for this kind of connection"),
                Some(factory) => match client {
                    Some(client) => Destination::Xmpp {
                        factory,
                        client,
                        direct,
                    },
                    None => return plain(400, "the edge node named no client"),
                },
            }
        }
        _ => return plain(400, "unknown tunnel kind"),
    };
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        let stream = match upgrade.await {
            Ok(upgraded) => TokioIo::new(upgraded),
            Err(err) => {
                tracing::debug!("edge tunnel: the stream did not open: {err}");
                return;
            }
        };
        match destination {
            Destination::Https(web) => passthrough(stream, web).await,
            Destination::Ssh(factory, client) => ssh(&factory, stream, client),
            Destination::Xmpp {
                factory,
                client,
                direct,
            } => {
                tracing::debug!("edge XMPP tunnel from {client}");
                if !crate::xmpp::serve_tunneled(&factory, stream, client, direct) {
                    tracing::debug!(
                        "edge XMPP tunnel from {client}: not taken (the server stopped, or the source refused)"
                    );
                }
            }
        }
    });
    Response::new(axum::body::Body::empty())
}

/// A passthrough tunnel: its PROXY header, then this node's TLS, then HTTP as on the HTTPS listener.
async fn passthrough<S>(mut stream: S, web: LocalWeb)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let client = match tokio::time::timeout(TUNNEL_HANDSHAKE, crate::proxy_header::read(&mut stream)).await {
        Ok(Ok(Some(client))) => client,
        Ok(Ok(None)) => {
            tracing::debug!("edge passthrough: a PROXY header with no client");
            return;
        }
        Ok(Err(err)) => {
            tracing::debug!("edge passthrough: {err}");
            return;
        }
        Err(_) => {
            tracing::debug!("edge passthrough: no PROXY header in time");
            return;
        }
    };
    // web protection admits the client before any TLS, as the HTTPS listener does
    let Some(_slot) = web.serve.app.guard.admit(client.ip()) else {
        tracing::debug!("edge passthrough: web protection refused {client}");
        return;
    };
    let Some(tls) = web.serve.tls.clone() else {
        return;
    };
    let handshake = tokio_rustls::TlsAcceptor::from(tls).accept(stream);
    let limit = web.serve.app.limits.header.unwrap_or(TUNNEL_HANDSHAKE);
    let stream = match tokio::time::timeout(limit, handshake).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => {
            tracing::debug!("edge passthrough: TLS handshake with {client} failed: {err}");
            return;
        }
        Err(_) => {
            tracing::debug!("edge passthrough: TLS handshake with {client} timed out");
            return;
        }
    };
    crate::http::serve_tunneled(stream, client, web.serve.app, web.serve.shutdown).await;
}

#[cfg(test)]
#[path = "down_tests.rs"]
mod tests;
