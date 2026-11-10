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

//! Byte tunnels down an edge link, on the edge node (plans/Edge-Node.md §5.2, §5.3). Each is an extended CONNECT
//! (RFC 8441, `:protocol` `moqui-tunnel`) on the node's down connection, with its kind in a header; the local node
//! picks the listener it goes to, so the edge node never names an address and is no open proxy.
//!
//! - **Passthrough** ([`super::TUNNEL_HTTPS`]): a passthrough host's connection, read up to its ClientHello by the
//!   HTTPS listener ([`super::accept`]). A PROXY v2 header with the client's address goes first, then the ClientHello's
//!   bytes, then both ways as they come. The local node ends the TLS with its own certificate.
//! - **SSH** ([`super::TUNNEL_SSH`]): each connection to `edge_ssh_tunnel_port`, to the local node's SSH server.
//! - **XMPP** ([`super::TUNNEL_XMPP`], [`super::TUNNEL_XMPPS`]): each connection to `edge_xmpp_port` (STARTTLS) or
//!   `edge_xmpp_tls_port` (direct TLS), to the local node's XMPP server, which ends the TLS (§5.4). The edge node sees
//!   no stanza, and channel binding works as on the local node's own ports.

use std::net::SocketAddr;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::accept::EdgeServer;
use super::{CLIENT_HEADER, TUNNEL_HEADER, TUNNEL_HTTPS, TUNNEL_PROTOCOL, TUNNEL_SSH, TUNNEL_XMPP, TUNNEL_XMPPS};

/// SSH connections piped at once.
const MAX_SSH_TUNNELS: usize = 64;
/// XMPP client connections piped at once, STARTTLS and direct TLS together. Clients stay connected, so this is a good
/// part of what a link may hold (768 tunnels and WebSockets); the local node's own limits apply behind it.
const MAX_XMPP_TUNNELS: usize = 256;

/// Open a tunnel of `kind` down `node`'s link for `client`.
async fn open(
    server: &EdgeServer,
    node: &str,
    kind: &'static str,
    client: SocketAddr,
) -> Result<TokioIo<hyper::upgrade::Upgraded>, String> {
    let mut send = server
        .down_sender(node)
        .ok_or_else(|| format!("{node} is not linked"))?;
    let request = http::Request::builder()
        .method(http::Method::CONNECT)
        .version(http::Version::HTTP_2)
        .uri(super::origin())
        .header(TUNNEL_HEADER, kind)
        .header(CLIENT_HEADER, client.to_string())
        .extension(hyper::ext::Protocol::from_static(TUNNEL_PROTOCOL))
        .body(tonic::body::Body::empty())
        .map_err(|err| format!("the tunnel request: {err}"))?;
    let opened = async {
        send.ready().await.map_err(|err| format!("{node}'s link: {err}"))?;
        let response = send
            .send_request(request)
            .await
            .map_err(|err| format!("{node}'s link: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("{node} refused the {kind} tunnel: {}", response.status()));
        }
        hyper::upgrade::on(response)
            .await
            .map_err(|err| format!("{node}'s {kind} tunnel: {err}"))
    };
    match tokio::time::timeout(server.config().open_timeout, opened).await {
        Ok(Ok(upgraded)) => Ok(TokioIo::new(upgraded)),
        Ok(Err(err)) => Err(err),
        Err(_) => Err(format!("{node} did not open the {kind} tunnel in time")),
    }
}

/// Pipe `a` and `b` both ways until either closes or `shutdown` turns true.
async fn pipe<A, B>(mut shutdown: tokio::sync::watch::Receiver<bool>, a: &mut A, b: &mut B)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    tokio::select! {
        piped = tokio::io::copy_bidirectional(a, b) => {
            if let Err(err) = piped {
                tracing::trace!("edge tunnel ended: {err}");
            }
        }
        _ = shutdown.wait_for(|stopped| *stopped) => {}
    }
}

/// A passthrough host's connection from `remote`, whose first bytes, `hello`, were read already: down `node`'s link
/// behind a PROXY v2 header.
pub(crate) async fn passthrough(
    server: Arc<EdgeServer>,
    stream: TcpStream,
    hello: Vec<u8>,
    node: &str,
    remote: SocketAddr,
) {
    let local = stream
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
    let Some(_active) = server.opened(node, TUNNEL_HTTPS) else {
        return;
    };
    let tunnel = match open(&server, node, TUNNEL_HTTPS, remote).await {
        Ok(tunnel) => tunnel,
        Err(err) => {
            server.count("moqui.edgelink.refused", TUNNEL_HTTPS);
            tracing::debug!("Edge passthrough from {remote}: {err}");
            return;
        }
    };
    relay(server.shutdown(), stream, tunnel, &hello, remote, local).await;
}

/// Write a PROXY v2 header naming `client` (to `local`, the edge node's address) and the bytes read already on
/// `tunnel`, then pipe `stream` and `tunnel` both ways.
async fn relay<S, T>(
    shutdown: tokio::sync::watch::Receiver<bool>,
    mut stream: S,
    mut tunnel: T,
    hello: &[u8],
    client: SocketAddr,
    local: SocketAddr,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut head = crate::proxy_header::v2(client, local);
    head.extend_from_slice(hello);
    if tunnel.write_all(&head).await.is_err() || tunnel.flush().await.is_err() {
        return;
    }
    pipe(shutdown, &mut stream, &mut tunnel).await;
}

/// The node a tunnel port's connections go to: the one its property names, else the one node linked now; none when
/// several are and none is named.
fn tunnel_node(server: &EdgeServer, named: &str, property: &str) -> Result<String, String> {
    if !named.is_empty() {
        return Ok(named.to_string());
    }
    let linked = server.linked_nodes();
    match linked.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("no local node is linked".to_string()),
        _ => Err(format!(
            "several local nodes are linked; set {property} to the one it goes to"
        )),
    }
}

/// A port whose connections each go down a link as a byte tunnel of one kind.
pub(crate) struct TunnelPort {
    pub(crate) kind: &'static str,
    pub(crate) bind: SocketAddr,
    /// The paired node named for it, and the property that names it; an empty name is the one node linked now.
    pub(crate) node: String,
    pub(crate) node_property: &'static str,
    /// Connections piped at once, shared by the ports of one service.
    pub(crate) permits: Arc<tokio::sync::Semaphore>,
}

/// The edge node's tunnel ports from its settings: SSH (at most [`MAX_SSH_TUNNELS`] at once), and XMPP's STARTTLS and
/// direct TLS ports (at most [`MAX_XMPP_TUNNELS`] at once, the two together). All of them also count toward the
/// link's own limit of tunnels (`accept::MAX_TUNNELS_PER_NODE`).
pub(crate) fn ports(config: &super::EdgeConfig) -> Vec<TunnelPort> {
    let mut ports = Vec::new();
    if let Some(bind) = config.ssh_tunnel {
        ports.push(TunnelPort {
            kind: TUNNEL_SSH,
            bind,
            node: config.ssh_tunnel_node.clone(),
            node_property: "edge_ssh_tunnel_node",
            permits: Arc::new(tokio::sync::Semaphore::new(MAX_SSH_TUNNELS)),
        });
    }
    let xmpp_permits = Arc::new(tokio::sync::Semaphore::new(MAX_XMPP_TUNNELS));
    for (kind, bind) in [
        (TUNNEL_XMPP, config.xmpp_tunnel),
        (TUNNEL_XMPPS, config.xmpp_tls_tunnel),
    ] {
        if let Some(bind) = bind {
            ports.push(TunnelPort {
                kind,
                bind,
                node: config.xmpp_tunnel_node.clone(),
                node_property: "edge_xmpp_node",
                permits: Arc::clone(&xmpp_permits),
            });
        }
    }
    ports
}

/// On an all-in-one node with its own XMPP server (`xmpp_domain` set), the property of the server's own XMPP listener
/// that `bind` would take: `xmpp_port` or `xmpp_tls_port` on the same port and an overlapping host. The paired node's
/// XMPP domain gets tunnel ports of its own (plans/Edge-Node.md §10), so such a tunnel port is skipped, logged.
pub(crate) fn own_xmpp_clash(bind: SocketAddr) -> Option<&'static str> {
    let property = |name: &str| moqui_lib::env::get_prop_or_env(name).trim().to_string();
    if property("xmpp_domain").is_empty() {
        return None;
    }
    let shared_host = property("xmpp_host");
    [("xmpp_port", "xmpp_host"), ("xmpp_tls_port", "xmpp_tls_host")]
        .into_iter()
        .find(|(port_name, host_name)| {
            let Ok(port) = property(port_name).parse::<u16>() else {
                return false;
            };
            let host = Some(property(host_name))
                .filter(|host| !host.is_empty())
                .unwrap_or_else(|| shared_host.clone());
            same_listener(bind, port, &host)
        })
        .map(|(port_name, _)| port_name)
}

/// Whether a listener on `bind` and one on `port` of `host` (an IP, a name, or empty for every address) would take the
/// same port: the ports are equal, and the hosts are equal or one is every address. A host name is taken to overlap,
/// as it is not resolved here.
fn same_listener(bind: SocketAddr, port: u16, host: &str) -> bool {
    if bind.port() != port || port == 0 {
        return false;
    }
    match host.trim().parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_unspecified() || bind.ip().is_unspecified() || ip == bind.ip(),
        Err(_) => true,
    }
}

/// Listen on `port`'s address and pipe each connection down a linked node's tunnel of its kind, until the listeners
/// stop. `guard`, the web protection, admits each connection first (its deny and exempt rules, bans and per-client
/// limit). A bind that fails is logged, and the rest of the edge node goes on. The bound address, for the startup
/// report.
pub(crate) fn listen(
    server: &Arc<EdgeServer>,
    port: TunnelPort,
    guard: Arc<crate::guard::Guard>,
) -> Option<SocketAddr> {
    let TunnelPort {
        kind,
        bind,
        node,
        node_property,
        permits,
    } = port;
    let listener = match std::net::TcpListener::bind(bind)
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
        .and_then(tokio::net::TcpListener::from_std)
    {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("The edge {kind} tunnel could not bind {bind}: {err}");
            return None;
        }
    };
    let address = listener.local_addr().unwrap_or(bind);
    let server = Arc::clone(server);
    let node: Arc<str> = Arc::from(node);
    tokio::spawn(async move {
        let mut shutdown = server.shutdown();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                _ = shutdown.wait_for(|stopped| *stopped) => return,
            };
            let (mut stream, remote) = match accepted {
                Ok(pair) => pair,
                Err(err) => {
                    // as out of file descriptors: wait rather than spin on the error
                    tracing::debug!("Edge {kind} tunnel: accept failed: {err}");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                }
            };
            let Some(admitted) = guard.admit(remote.ip()) else {
                tracing::debug!("Edge {kind} tunnel: web protection refused {remote}");
                continue;
            };
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                tracing::warn!("Edge {kind} tunnel: its most at once are open already; {remote} is closed");
                continue;
            };
            let (server, node) = (Arc::clone(&server), Arc::clone(&node));
            tokio::spawn(async move {
                let node = match tunnel_node(&server, &node, node_property) {
                    Ok(node) => node,
                    Err(err) => {
                        server.count("moqui.edgelink.refused", kind);
                        tracing::info!("Edge {kind} tunnel from {remote} refused: {err}");
                        return;
                    }
                };
                let Some(_active) = server.opened(&node, kind) else {
                    return;
                };
                let mut tunnel = match open(&server, &node, kind, remote).await {
                    Ok(tunnel) => tunnel,
                    Err(err) => {
                        server.count("moqui.edgelink.refused", kind);
                        tracing::info!("Edge {kind} tunnel from {remote}: {err}");
                        return;
                    }
                };
                let _ = stream.set_nodelay(true);
                pipe(server.shutdown(), &mut stream, &mut tunnel).await;
                drop((permit, admitted));
            });
        }
    });
    Some(address)
}

#[cfg(test)]
#[path = "tunnel_tests.rs"]
mod tests;
