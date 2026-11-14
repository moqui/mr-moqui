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

//! Byte tunnels down an edge link, on the edge node (plans/archive/Edge-Node.md §5.2, §5.3). Each is an extended CONNECT
//! (RFC 8441, `:protocol` `moqui-tunnel`) on the node's down connection, with its kind in a header; the local node
//! picks the listener it goes to, so the edge node never names an address and is no open proxy.
//!
//! - **Passthrough** ([`super::TUNNEL_HTTPS`]): a passthrough host's connection, read up to its ClientHello by the
//!   HTTPS listener ([`super::accept`]). A PROXY v2 header with the client's address goes first, then the ClientHello's
//!   bytes, then both ways as they come. The local node ends the TLS with its own certificate.
//! - **SSH** ([`super::TUNNEL_SSH`]): each connection to `edge_ssh_tunnel_port`, to the SSH server of the one active
//!   pairing (refused with several, and while it is away).
//! - **XMPP** ([`super::TUNNEL_XMPP`], [`super::TUNNEL_XMPPS`]): each connection to `edge_xmpp_port` (STARTTLS) or
//!   `edge_xmpp_tls_port` (direct TLS), to the XMPP server of the local node whose XMPP domain the connection names
//!   (plans/Edge-Node2.md §4.1, E16: the TLS server name on the direct TLS port, the stream header's `to` on the
//!   STARTTLS port, [`super::xmpp_domain`]), which ends the TLS (§5.4). The edge node reads only that, sees no stanza,
//!   and channel binding works as on the local node's own ports. An all-in-one node's own XMPP ports do the same for
//!   its pairings' domains ([`xmpp_to_pairing`]), so one pair of ports serves every domain.
//! - **A port per pairing** ([`watch_rows`], E16): each `moqui.edge.EdgePort` row, an SSH or XMPP port whose
//!   connections go to its pairing only, opened and closed as the System app sets the rows.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Weak};

use hyper_util::rt::TokioIo;
use moqui_core::context::ExecutionContextFactory;
use moqui_core::edge::ports::{EdgePort, PortState};
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

/// Pipe `a` and `b` both ways until either closes or `stop` ends.
async fn pipe<A, B>(stop: impl std::future::Future<Output = ()>, a: &mut A, b: &mut B)
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
        () = stop => {}
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
    pipe(stopped(shutdown, None), &mut stream, &mut tunnel).await;
}

/// Where a tunnel port's connections go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PortRoute {
    /// To the one active pairing; refused while none or several are paired, and while that one is away (the conf's
    /// SSH port). Which pairings are linked never decides it, so one away never sends its traffic to another.
    OnlyPairing,
    /// To the pairing whose XMPP domain the connection names ([`xmpp_route`]): the conf's XMPP ports.
    ByDomain,
    /// To this pairing: an `EdgePort` row (E16).
    Pairing(Arc<str>),
}

/// The node a tunnel port's connection goes to: the one active pairing (the host table's, as
/// `EdgeServer::only_pairing_link` counts them); none when none or several are paired. A pairing that is away is
/// refused when its tunnel is opened.
fn only_pairing(server: &EdgeServer) -> Result<String, String> {
    match server.hosts.active().as_slice() {
        [only] => Ok(only.clone()),
        [] => Err("no local node is paired".to_string()),
        _ => Err("several local nodes are paired; give each its own port (System, Edge Clients, Ports)".to_string()),
    }
}

/// Where an XMPP client's connection goes on a port of every domain (plans/Edge-Node2.md §4.1, E16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum XmppRoute {
    /// Down this pairing's link.
    Pairing(String),
    /// To this node's own XMPP server (an all-in-one node's own domain).
    Own,
    /// Refused, with why (for the log).
    Refused(String),
}

/// Where an XMPP connection naming `domain` goes: to the one pairing whose Configure named that XMPP domain
/// (`pairings`, each pairing and its domain); else to this node's own server when it has one (`own`, an all-in-one
/// node's `xmpp_domain`) and the connection names it, a subdomain of it, or none; else, while exactly one pairing is
/// active (`active`), to it, as before a node named its domain, linked or not (an away one is refused when its tunnel
/// opens, never sent to another). A domain two pairings name is refused.
pub(crate) fn xmpp_route(
    domain: Option<&str>,
    own: Option<&str>,
    pairings: &[(String, String)],
    active: &[String],
) -> XmppRoute {
    if let Some(domain) = domain {
        let named: Vec<&str> = pairings
            .iter()
            .filter(|(_, held)| !held.is_empty() && held.eq_ignore_ascii_case(domain))
            .map(|(node, _)| node.as_str())
            .collect();
        match named.as_slice() {
            [node] => return XmppRoute::Pairing((*node).to_string()),
            [] => {}
            several => {
                return XmppRoute::Refused(format!("{domain} is named by the local nodes {}", several.join(", ")))
            }
        }
    }
    if let Some(own) = own.filter(|own| !own.is_empty()) {
        let ours = domain.is_none_or(|domain| domain == own || domain.ends_with(&format!(".{own}")));
        if ours {
            return XmppRoute::Own;
        }
    }
    match active {
        [only] => XmppRoute::Pairing(only.clone()),
        _ => XmppRoute::Refused(match domain {
            Some(domain) => format!("no local node set up XMPP for {domain}"),
            None => "the connection names no XMPP domain and several local nodes are paired".to_string(),
        }),
    }
}

/// Each active pairing and the XMPP domain of its last Configure (`xmppDomain`), lower case, from the rows: for the
/// offer. A connection reads the host table's copy (`Hosts::xmpp_domains`), which Configure keeps current, and
/// Configure refuses a domain another pairing named first.
pub(crate) fn xmpp_domains(
    facade: &moqui_core::entity::EntityFacade,
) -> Result<Vec<(String, String)>, moqui_lib::error::Error> {
    let rows = facade
        .find("moqui.edge.EdgeClient")?
        .eq("statusId", moqui_lib::object::Value::string("EdgeClientActive"))
        .select_field("pairedNodeId")
        .select_field("setupJson")
        .disable_authz()
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| {
            let text = |field: &str| {
                row.get(field)
                    .map(moqui_lib::object::to_plain_string)
                    .unwrap_or_default()
            };
            let setup: serde_json::Value = serde_json::from_str(&text("setupJson")).unwrap_or_default();
            let domain = setup["xmppDomain"]
                .as_str()
                .and_then(super::xmpp_domain::host_name)
                .unwrap_or_default();
            (text("pairedNodeId"), domain)
        })
        .collect())
}

/// This node's own XMPP domain while its own XMPP server runs (an all-in-one node), lower case.
fn own_xmpp_domain(factory: &ExecutionContextFactory, direct: bool) -> Option<String> {
    let domain = moqui_lib::env::get_prop_or_env("xmpp_domain");
    (crate::xmpp::running(factory, direct))
        .then(|| super::xmpp_domain::host_name(&domain))
        .flatten()
}

/// Where the XMPP connection naming `domain` goes on this node ([`xmpp_route`]), from the host table (no database
/// read per connection). `own_ports` is a connection to this node's own XMPP port, which never falls to the one
/// pairing: what names none of a pairing's is the server's own.
pub(crate) fn route_xmpp(
    server: &EdgeServer,
    factory: &Weak<ExecutionContextFactory>,
    domain: Option<&str>,
    direct: bool,
    own_ports: bool,
) -> XmppRoute {
    let own = factory.upgrade().and_then(|factory| own_xmpp_domain(&factory, direct));
    let active = if own_ports { Vec::new() } else { server.hosts.active() };
    xmpp_route(domain, own.as_deref(), &server.hosts.xmpp_domains(), &active)
}

/// A port whose connections each go down a link as a byte tunnel of one kind.
pub(crate) struct TunnelPort {
    pub(crate) kind: &'static str,
    pub(crate) bind: SocketAddr,
    pub(crate) route: PortRoute,
    /// Connections piped at once, shared by the ports of one service.
    pub(crate) permits: Arc<tokio::sync::Semaphore>,
}

/// The edge node's tunnel ports from its settings: SSH to the one active pairing (at most [`MAX_SSH_TUNNELS`] at once),
/// and XMPP's STARTTLS and direct TLS ports, each connection by the XMPP domain it names (at most [`MAX_XMPP_TUNNELS`]
/// at once, the two together). All of them also count toward the link's own limit of tunnels
/// (`accept::MAX_TUNNELS_PER_NODE`). The ports of one pairing each are `EdgePort` rows ([`watch_rows`]).
pub(crate) fn ports(config: &super::EdgeConfig) -> Vec<TunnelPort> {
    let mut ports = Vec::new();
    if let Some(bind) = config.ssh_tunnel {
        ports.push(TunnelPort {
            kind: TUNNEL_SSH,
            bind,
            route: PortRoute::OnlyPairing,
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
                route: PortRoute::ByDomain,
                permits: Arc::clone(&xmpp_permits),
            });
        }
    }
    ports
}

/// The tunnel kind of a row's `tunnelKind`.
fn row_kind(kind: &str) -> Option<&'static str> {
    [TUNNEL_SSH, TUNNEL_XMPP, TUNNEL_XMPPS]
        .into_iter()
        .find(|known| *known == kind)
}

/// Where a row's port listens: on `edge_ssh_tunnel_host` for SSH and `edge_xmpp_host` for XMPP, every address while
/// it is empty.
fn row_bind(kind: &str, port: u16) -> Result<SocketAddr, String> {
    let host_name = if kind == TUNNEL_SSH {
        "edge_ssh_tunnel_host"
    } else {
        "edge_xmpp_host"
    };
    let host = moqui_lib::env::get_prop_or_env(host_name);
    let ip = match host.trim() {
        "" => std::net::IpAddr::from([0, 0, 0, 0]),
        host => host
            .parse()
            .map_err(|_| format!("{host_name} {host} is not an IP address"))?,
    };
    Ok(SocketAddr::new(ip, port))
}

/// How often the rows are read again with no change said on this node (another member of a cluster, a row written
/// by hand).
const ROWS_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// The `EdgePort` rows' listeners (plans/Edge-Node2.md §4.10 Ports, E16), until the listeners stop: read at start,
/// again within a second of a change on this node (`moqui_core::edge::ports::changed`, and once more two seconds
/// after, for the change's commit), when the host table's active pairings change, and every [`ROWS_EVERY`]. A row's
/// port listens while the row is there and its pairing is active, and goes down that pairing's link only; a row
/// removed or changed, or its pairing revoked, closes its listener and its connections (the row stays, for a re-pair).
/// A port that does not bind is logged and tried again at the next read; what each does is reported for
/// `get#EdgePorts`.
pub(crate) fn watch_rows(
    server: &Arc<EdgeServer>,
    guard: Arc<crate::guard::Guard>,
    factory: Weak<ExecutionContextFactory>,
) {
    let server = Arc::clone(server);
    tokio::spawn(async move {
        let mut running: HashMap<u16, (EdgePort, tokio::sync::watch::Sender<bool>)> = HashMap::new();
        let mut states: BTreeMap<u16, PortState> = BTreeMap::new();
        let mut seen = None;
        let mut last_active: Vec<String> = Vec::new();
        let mut again: Option<std::time::Instant> = None;
        let mut read_at: Option<std::time::Instant> = None;
        let mut failing = false;
        let mut shutdown = server.shutdown();
        loop {
            let Some(strong) = factory.upgrade() else { return };
            let generation = moqui_core::edge::ports::generation(&strong);
            drop(strong);
            let now = std::time::Instant::now();
            // the host table's active pairings, which pairing, revoking and unpairing load again
            let active = server.hosts.active();
            let due = seen != Some(generation)
                || active != last_active
                || again.is_some_and(|at| now >= at)
                || read_at.is_none_or(|at| now.duration_since(at) >= ROWS_EVERY);
            if due {
                again = (seen.is_some() && seen != Some(generation)).then(|| now + std::time::Duration::from_secs(2));
                seen = Some(generation);
                read_at = Some(now);
                last_active.clone_from(&active);
                let read = crate::worker::on_pool_without_authz(&factory, |factory| {
                    let facade = factory
                        .entity()
                        .ok_or_else(|| moqui_core::context::Error::new("no entity facade"))?;
                    Ok(moqui_core::edge::ports::list(&facade, None)?)
                })
                .await;
                match read {
                    Ok(rows) => {
                        failing = false;
                        reconcile(&server, &guard, &factory, rows, &active, &mut running, &mut states);
                        if let Some(factory) = factory.upgrade() {
                            moqui_core::edge::ports::set_states(&factory, states.clone());
                        }
                    }
                    Err(err) if !failing => {
                        failing = true;
                        tracing::error!(
                            "Edge ports: the rows were not read, the listeners stay: {}",
                            err.message()
                        );
                    }
                    Err(_) => {}
                }
            }
            tokio::select! {
                () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                _ = shutdown.wait_for(|stopped| *stopped) => return,
            }
        }
    });
}

/// Open and close the row listeners to match `rows`, keeping what each does in `states`.
fn reconcile(
    server: &Arc<EdgeServer>,
    guard: &Arc<crate::guard::Guard>,
    factory: &Weak<ExecutionContextFactory>,
    rows: Vec<EdgePort>,
    active: &[String],
    running: &mut HashMap<u16, (EdgePort, tokio::sync::watch::Sender<bool>)>,
    states: &mut BTreeMap<u16, PortState>,
) {
    // a revoked pairing's rows stay, for a re-pair, but take no connection
    let (rows, idle): (Vec<EdgePort>, Vec<EdgePort>) =
        rows.into_iter().partition(|row| active.contains(&row.paired_node_id));
    running.retain(|port, (row, _)| {
        let kept = rows.contains(row);
        if !kept {
            tracing::info!(
                "Edge port {port} ({}, local node {}) is closed",
                row.tunnel_kind,
                row.paired_node_id
            );
        }
        kept
    });
    states.retain(|port, _| rows.iter().any(|row| row.port == *port));
    for row in idle {
        let error = format!("local node {} is not an active pairing", row.paired_node_id);
        states.insert(
            row.port,
            PortState {
                listening: false,
                error,
            },
        );
    }
    for row in rows {
        if running.contains_key(&row.port) {
            continue;
        }
        let started = row_kind(&row.tunnel_kind)
            .ok_or_else(|| format!("{} is not a tunnel kind", row.tunnel_kind))
            .and_then(|kind| Ok((kind, row_bind(kind, row.port)?)))
            .and_then(|(kind, bind)| {
                let max = if kind == TUNNEL_SSH {
                    MAX_SSH_TUNNELS
                } else {
                    MAX_XMPP_TUNNELS
                };
                let (stop, stopped) = tokio::sync::watch::channel(false);
                let port = TunnelPort {
                    kind,
                    bind,
                    route: PortRoute::Pairing(Arc::from(row.paired_node_id.as_str())),
                    permits: Arc::new(tokio::sync::Semaphore::new(max)),
                };
                let address = listen(server, port, Arc::clone(guard), factory.clone(), Some(stopped))?;
                Ok((address, stop))
            });
        match started {
            Ok((address, stop)) => {
                tracing::info!(
                    "Edge port {address} ({}) goes to local node {}",
                    row.tunnel_kind,
                    row.paired_node_id
                );
                states.insert(
                    row.port,
                    PortState {
                        listening: true,
                        error: String::new(),
                    },
                );
                running.insert(row.port, (row, stop));
            }
            Err(error) => {
                if states.get(&row.port).is_none_or(|state| state.error != error) {
                    tracing::error!("Edge port {} does not listen: {error}", row.port);
                }
                states.insert(
                    row.port,
                    PortState {
                        listening: false,
                        error,
                    },
                );
            }
        }
    }
}

/// On an all-in-one node with its own XMPP server (`xmpp_domain` set), the property of the server's own XMPP listener
/// that `bind` would take: `xmpp_port` or `xmpp_tls_port` on the same port and an overlapping host. The paired node's
/// XMPP domain gets tunnel ports of its own (plans/archive/Edge-Node.md §10), so such a tunnel port is skipped, logged.
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

/// Waits until the listeners stop or, for a row's port, its row goes (`stop` turns true or its sender drops).
async fn stopped(mut shutdown: tokio::sync::watch::Receiver<bool>, stop: Option<tokio::sync::watch::Receiver<bool>>) {
    match stop {
        Some(mut stop) => {
            tokio::select! {
                _ = shutdown.wait_for(|stopped| *stopped) => {}
                _ = stop.wait_for(|stopped| *stopped) => {}
            }
        }
        None => {
            let _ = shutdown.wait_for(|stopped| *stopped).await;
        }
    }
}

/// Listen on `port`'s address and pipe each connection down a linked node's tunnel of its kind, until the listeners
/// stop (or, with `stop`, the port's row goes). `guard`, the web protection, admits each connection first (its deny
/// and exempt rules, bans and per-client limit). The node is the port's route's: the one active pairing, the pairing
/// whose XMPP domain the connection names (what was read for it goes first, then both ways; an all-in-one node's own
/// domain is served by its own XMPP server), or the row's pairing. The bound address, or why it did not bind.
pub(crate) fn listen(
    server: &Arc<EdgeServer>,
    port: TunnelPort,
    guard: Arc<crate::guard::Guard>,
    factory: Weak<ExecutionContextFactory>,
    stop: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<SocketAddr, String> {
    let TunnelPort {
        kind,
        bind,
        route,
        permits,
    } = port;
    let listener = std::net::TcpListener::bind(bind)
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
        .and_then(tokio::net::TcpListener::from_std)
        .map_err(|err| format!("the edge {kind} tunnel could not bind {bind}: {err}"))?;
    let address = listener.local_addr().unwrap_or(bind);
    let server = Arc::clone(server);
    tokio::spawn(async move {
        let ended = stopped(server.shutdown(), stop.clone());
        tokio::pin!(ended);
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted,
                () = &mut ended => return,
            };
            let (stream, remote) = match accepted {
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
            let (server, route, factory, stop) = (Arc::clone(&server), route.clone(), factory.clone(), stop.clone());
            tokio::spawn(async move {
                connection(&server, kind, &route, &factory, stream, remote, stop).await;
                drop((permit, admitted));
            });
        }
    });
    Ok(address)
}

/// How long a client has to name its XMPP domain on a port of every domain.
const DOMAIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// One connection of a tunnel port, to its node by `route`, until either end closes.
async fn connection(
    server: &Arc<EdgeServer>,
    kind: &'static str,
    route: &PortRoute,
    factory: &Weak<ExecutionContextFactory>,
    mut stream: TcpStream,
    remote: SocketAddr,
    stop: Option<tokio::sync::watch::Receiver<bool>>,
) {
    let refused = |why: String| {
        server.count("moqui.edgelink.refused", kind);
        tracing::info!("Edge {kind} tunnel from {remote} refused: {why}");
    };
    let mut first = Vec::new();
    let node = match route {
        PortRoute::OnlyPairing => only_pairing(server),
        PortRoute::Pairing(node) => Ok(node.to_string()),
        PortRoute::ByDomain => {
            let direct = kind == TUNNEL_XMPPS;
            let domain = match tokio::time::timeout(DOMAIN_WAIT, super::xmpp_domain::sniff(&mut stream, direct)).await {
                Ok(Ok((domain, seen))) => {
                    first = seen;
                    domain
                }
                Ok(Err(err)) => return refused(format!("its start did not read: {err}")),
                Err(_) => return refused("it named no XMPP domain in time".to_string()),
            };
            match route_xmpp(server, factory, domain.as_deref(), direct, false) {
                XmppRoute::Pairing(node) => Ok(node),
                XmppRoute::Own => {
                    // an all-in-one node's own domain on its tunnel port: its own XMPP server takes it
                    let stream = super::xmpp_domain::Prefixed::new(first, stream);
                    let taken = factory
                        .upgrade()
                        .is_some_and(|factory| crate::xmpp::serve_tunneled(&factory, stream, remote, direct));
                    if !taken {
                        refused("this node's own XMPP server did not take it".to_string());
                    }
                    return;
                }
                XmppRoute::Refused(why) => Err(why),
            }
        }
    };
    let node = match node {
        Ok(node) => node,
        Err(why) => return refused(why),
    };
    let Some(_active) = server.opened(&node, kind) else {
        return;
    };
    let mut tunnel = match open(server, &node, kind, remote).await {
        Ok(tunnel) => tunnel,
        Err(err) => return refused(err),
    };
    if !first.is_empty() && (tunnel.write_all(&first).await.is_err() || tunnel.flush().await.is_err()) {
        return;
    }
    let _ = stream.set_nodelay(true);
    pipe(stopped(server.shutdown(), stop), &mut stream, &mut tunnel).await;
}

/// On an all-in-one node, a connection to its own XMPP port that names a pairing's XMPP domain (plans/Edge-Node2.md
/// §4.1, E16): down that pairing's link, with what was read for the domain first. One pair of ports then serves every
/// domain. False when no pairing has the domain (or the edge link does not run), so the server's own takes it.
pub(crate) async fn xmpp_to_pairing(
    factory: &Weak<ExecutionContextFactory>,
    stream: &mut Option<TcpStream>,
    remote: SocketAddr,
    domain: Option<&str>,
    first: &[u8],
    direct: bool,
) -> bool {
    let Some(server) = super::hook::edge_server() else {
        return false;
    };
    let kind = if direct { TUNNEL_XMPPS } else { TUNNEL_XMPP };
    let XmppRoute::Pairing(node) = route_xmpp(&server, factory, domain, direct, true) else {
        return false;
    };
    let Some(mut client) = stream.take() else {
        return false;
    };
    let Some(_active) = server.opened(&node, kind) else {
        return true;
    };
    let mut tunnel = match open(&server, &node, kind, remote).await {
        Ok(tunnel) => tunnel,
        Err(err) => {
            server.count("moqui.edgelink.refused", kind);
            tracing::info!("XMPP connection from {remote} for local node {node} refused: {err}");
            return true;
        }
    };
    if tunnel.write_all(first).await.is_err() || tunnel.flush().await.is_err() {
        return true;
    }
    let _ = client.set_nodelay(true);
    pipe(stopped(server.shutdown(), None), &mut client, &mut tunnel).await;
    true
}

#[cfg(test)]
#[path = "tunnel_tests.rs"]
mod tests;
