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

use std::net::SocketAddr;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::accept::EdgeServer;
use super::{CLIENT_HEADER, TUNNEL_HEADER, TUNNEL_HTTPS, TUNNEL_PROTOCOL, TUNNEL_SSH};

/// SSH connections piped at once.
const MAX_SSH_TUNNELS: usize = 64;

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

/// The node the SSH tunnel goes to: `edge_ssh_tunnel_node`, else the one node linked now; none when several are and
/// none is named.
fn ssh_node(server: &EdgeServer) -> Result<String, String> {
    let named = &server.config().ssh_tunnel_node;
    if !named.is_empty() {
        return Ok(named.clone());
    }
    let linked = server.linked_nodes();
    match linked.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("no local node is linked".to_string()),
        _ => Err("several local nodes are linked; set edge_ssh_tunnel_node to the one SSH goes to".to_string()),
    }
}

/// Listen on `bind` and pipe each connection to a linked node's SSH server, until the listeners stop. `guard`, the web
/// protection, admits each connection first (its deny and exempt rules, bans and per-client limit). A bind that fails is
/// logged, and the rest of the edge node goes on.
pub(crate) fn listen_ssh(server: &Arc<EdgeServer>, bind: SocketAddr, guard: Arc<crate::guard::Guard>) {
    let listener = match std::net::TcpListener::bind(bind)
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
        .and_then(tokio::net::TcpListener::from_std)
    {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("The edge SSH tunnel could not bind {bind}: {err}");
            return;
        }
    };
    let address = listener.local_addr().unwrap_or(bind);
    tracing::info!("Edge node: the SSH tunnel to a linked local node listens on {address}");
    let server = Arc::clone(server);
    let permits = Arc::new(tokio::sync::Semaphore::new(MAX_SSH_TUNNELS));
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
                    tracing::debug!("Edge SSH tunnel: accept failed: {err}");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    continue;
                }
            };
            let Some(admitted) = guard.admit(remote.ip()) else {
                tracing::debug!("Edge SSH tunnel: web protection refused {remote}");
                continue;
            };
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                tracing::warn!("Edge SSH tunnel: {MAX_SSH_TUNNELS} open already; {remote} is closed");
                continue;
            };
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                let node = match ssh_node(&server) {
                    Ok(node) => node,
                    Err(err) => {
                        server.count("moqui.edgelink.refused", TUNNEL_SSH);
                        tracing::info!("Edge SSH tunnel from {remote} refused: {err}");
                        return;
                    }
                };
                let Some(_active) = server.opened(&node, TUNNEL_SSH) else {
                    return;
                };
                let mut tunnel = match open(&server, &node, TUNNEL_SSH, remote).await {
                    Ok(tunnel) => tunnel,
                    Err(err) => {
                        server.count("moqui.edgelink.refused", TUNNEL_SSH);
                        tracing::info!("Edge SSH tunnel from {remote}: {err}");
                        return;
                    }
                };
                let _ = stream.set_nodelay(true);
                pipe(server.shutdown(), &mut stream, &mut tunnel).await;
                drop((permit, admitted));
            });
        }
    });
}

#[cfg(test)]
#[path = "tunnel_tests.rs"]
mod tests;
