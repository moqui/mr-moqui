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

//! The edge node's web front (plans/Edge-Node.md §5.3): a request to a terminate host goes down its local node's link
//! as an HTTP/2 request, after the edge node's own web protection, with the bodies streamed both ways. The client's
//! address goes in [`CLIENT_HEADER`]; any forwarding header the client sent is removed, and so are the hop-by-hop
//! headers. A WebSocket upgrade becomes an extended CONNECT (RFC 8441) down the link, answered 101 here once the local
//! node takes it. While the node's link is down the host answers with the offline page.
//!
//! A passthrough host's request on the plain HTTP listener is sent to its https URL; one that came over this
//! listener's TLS under another server name is misdirected (421), since its TLS belongs to the local node.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::response::Response;
use http::header::{self, HeaderMap, HeaderValue};
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::TokioIo;
use sha1::{Digest as _, Sha1};

use super::accept::EdgeServer;
use super::hosts::Mode;
use super::{CLIENT_HEADER, PROTO_HEADER, TUNNEL_HEADER};
use crate::http::{plain, Peer, TlsConnection};

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
    /// Down the node's link.
    Forward {
        node: String,
        send: SendRequest<tonic::body::Body>,
    },
    /// The node's link is down.
    Offline,
    /// A passthrough host over plain HTTP: to its https URL.
    Https(String),
    /// A passthrough host's request that came over this listener's TLS.
    Misdirected,
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
    /// Where `request` goes, when it names one of the paired nodes' hosts; `None` for the edge node's own.
    pub(crate) fn target<B>(&self, request: &http::Request<B>) -> Option<Target> {
        let host = request_host(request)?;
        let route = self.server.hosts.route(&host)?;
        let tls = request.extensions().get::<TlsConnection>().is_some();
        Some(match route.mode {
            Mode::Passthrough if tls => Target::Misdirected,
            Mode::Passthrough => {
                let path = request.uri().path_and_query().map_or("/", |path| path.as_str());
                let name = super::hosts::normalize(&host).unwrap_or(host);
                Target::Https(format!("https://{name}{path}"))
            }
            Mode::Terminate => match self.server.down_sender(&route.node) {
                Some(send) => Target::Forward { node: route.node, send },
                None => Target::Offline,
            },
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
            Target::Forward { node, send } => {
                let server = Arc::clone(&self.server);
                let client = std::net::SocketAddr::new(peer.client, peer.remote.port());
                let https = peer.https;
                Box::pin(async move { forward(server, node, send, request, client, https).await })
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

/// Send `request` down `node`'s link as the edge node's request for `client`, and its answer back.
async fn forward<B>(
    server: Arc<EdgeServer>,
    node: String,
    mut send: SendRequest<tonic::body::Body>,
    request: http::Request<B>,
    client: std::net::SocketAddr,
    https: bool,
) -> Response
where
    B: hyper::body::Body<Data = bytes::Bytes> + Send + 'static,
    B::Error: Into<axum::BoxError>,
{
    // only a WebSocket upgrade becomes a CONNECT down the link: a client's own CONNECT is never forwarded
    if request.method() == http::Method::CONNECT {
        return plain(405, "CONNECT is not served here");
    }
    let websocket = is_websocket(&request);
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
    let mut down = http::Request::new(if websocket {
        tonic::body::Body::empty()
    } else {
        tonic::body::Body::new(body)
    });
    *down.uri_mut() = uri;
    *down.version_mut() = http::Version::HTTP_2;
    *down.headers_mut() = headers;
    if websocket {
        *down.method_mut() = http::Method::CONNECT;
        down.extensions_mut()
            .insert(hyper::ext::Protocol::from_static("websocket"));
    } else {
        *down.method_mut() = parts.method;
    }
    // a WebSocket holds a stream of the link as a tunnel does
    let active = if websocket {
        match server.opened(&node, "websocket") {
            Some(active) => Some(active),
            None => return plain(503, "too many connections to this site's server; try again"),
        }
    } else {
        None
    };
    let sent = async {
        send.ready().await?;
        send.send_request(down).await
    };
    let response = match tokio::time::timeout(server.config().open_timeout, sent).await {
        Ok(Ok(response)) => response,
        Ok(Err(err)) => {
            tracing::debug!("Edge forward to {node} failed: {err}");
            return if send.is_closed() {
                offline(&server.config().offline_page)
            } else {
                plain(502, "the site's server did not answer")
            };
        }
        Err(_) => return plain(504, "the site's server did not answer in time"),
    };
    if !websocket {
        server.count("moqui.edgelink.forwards", "terminate");
        return response.map(axum::body::Body::new);
    }
    let (Some(on_upgrade), Some(key)) = (on_upgrade, key) else {
        return plain(400, "a WebSocket upgrade without its key");
    };
    if !response.status().is_success() {
        return response.map(axum::body::Body::new);
    }
    let mut answer = Response::new(axum::body::Body::empty());
    *answer.status_mut() = http::StatusCode::SWITCHING_PROTOCOLS;
    let accept = websocket_accept(key.as_bytes());
    {
        let headers = answer.headers_mut();
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
        if let Ok(accept) = HeaderValue::from_str(&accept) {
            headers.insert(header::SEC_WEBSOCKET_ACCEPT, accept);
        }
        for name in [header::SEC_WEBSOCKET_PROTOCOL, header::SEC_WEBSOCKET_EXTENSIONS] {
            if let Some(value) = response.headers().get(&name) {
                headers.insert(name, value.clone());
            }
        }
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
