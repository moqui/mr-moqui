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

//! The edge node's end of the link (plans/archive/Edge-Node.md §5.1, §5.7): the TLS hand-off on the HTTPS listener and the
//! registry of live links by paired node, with each one's generation and its down connection's sender for
//! edge-to-local calls. The rows are in [`super::store`].
//!
//! The listener reads each ClientHello first ([`Acceptor::accept`]). One that offers `moqui-edge-pair/1` gets the web
//! certificate (checking and pairing, which the local node checks by WebPKI); one that offers `moqui-edge/1` gets the
//! certificate of this node's link key (the up and down connections, which the local node pinned). Both ask for a
//! client certificate without requiring it, and the presented key is looked up among the paired nodes after the
//! handshake. Any other client gets the listener's own settings, so a browser is never asked for a certificate.
//!
//! A ClientHello whose server name is a passthrough host of a linked node gets no handshake here: the connection and
//! the bytes read so far go down that node's link ([`super::tunnel`], plans/archive/Edge-Node.md §5.3). The server name of a
//! passthrough host whose node is not linked closes the connection.
//!
//! **Pools** (plans/Edge-Node2.md §4.4, §4.5). A pairing is one local node or one cluster of them: every member links
//! with the pairing's key and names its own instance id in its Hello. The registry keeps the live links by pairing and
//! member; the pairing's one generation counter numbers every member's connections, a new member's link sits beside
//! the others, and a member that dials again replaces only its own older one. A member not linked is admitted
//! ([`moqui_core::edge::pool::admission`]) when a linked member's member list, asked for at that moment, has it as a
//! live member, else refused "join the cluster first"; with none linked the key admits it, and a new node taking the
//! pairing over is logged, noted on the pairing's row and told to the administrators, or held for approval. Each
//! member's state (starting, live, draining, stopping) and its cluster's member list come with its Hello and every
//! Ping, and are kept in memory only. [`EdgeServer::link_of`] picks a member that takes work as [`super::balance`]
//! says for the kind of work (plans/Edge-Node2.md §4.4), and counts it in flight until it is answered.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::cluster_state::{Elsewhere, HeldLinks, LinkOp};
use moqui_core::edge::pool::{self, Admission, LinkedMember, RefusedMember};
use moqui_core::edge::{
    ClusterMemberInfo, DelegateAnswer, DelegateCall, EdgeClientInfo, MemberState, PairingCode, PoolMemberInfo,
};
use moqui_core::telemetry::Telemetry;
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::server::TlsStream;
use tonic::{Request, Response, Status};

use super::balance::{self, Affinity, Flight, Health, Rules};
use super::hosts::{self, Hosts, Route};
use super::pair::Identity;
use super::proto::link_server::{Link, LinkServer};
use super::proto::{
    ConfigureReply, ConfigureRequest, ContentChunk, DelegateRequest, DeliverReply, DeliverRequest, DrainRequest, Hello,
    HelloReply, HelloRequest, LocalHelloRequest, MemberStatus, MembersRequest, Offer, OfferRequest, PairReply,
    PairRequest, PingReply, PingRequest, ProbeReply, ProbeRequest, PutContentReply, ReplicateReply, ReplicateRequest,
    SignalMessage, SignalReply, UnpairReply, UnpairRequest,
};
use super::store::{self, Paired};
use super::{
    check_versions, hello, valid_node_id, write_answer, Answer, EdgeConfig, ALPN, ALPN_PAIR, LINK_VERSION, ROLE_CHECK,
    ROLE_DOWN, ROLE_PAIR, ROLE_UP,
};
use crate::tls_pin::{hex, public_key_sha256, Pin, PinVerifier};

/// The HTTPS listener's hand-off to the link on an edge node.
pub(crate) struct Acceptor {
    /// `moqui-edge/1`: the link key's certificate.
    link: Arc<rustls::ServerConfig>,
    /// `moqui-edge-pair/1`: the web certificate.
    pairing: Arc<rustls::ServerConfig>,
    server: Arc<EdgeServer>,
}

/// TLS settings for one of the link's ALPN protocols: a client certificate asked for but not required, any key accepted
/// by the handshake and checked against the paired nodes after it.
fn server_config(
    alpn: &[u8],
    certs: impl FnOnce(
        rustls::ConfigBuilder<rustls::ServerConfig, rustls::server::WantsServerCert>,
    ) -> Result<rustls::ServerConfig, rustls::Error>,
) -> Result<rustls::ServerConfig, String> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = PinVerifier::new(Pin::Deferred, false, &provider);
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| format!("edge link TLS: {err}"))?
        .with_client_cert_verifier(verifier);
    let mut config = certs(builder).map_err(|err| format!("edge link TLS certificate: {err}"))?;
    config.alpn_protocols = vec![alpn.to_vec()];
    Ok(config)
}

/// The listener's hand-off to `server`: pairing on the listener's certificates (`resolver`), links on `identity`, this
/// node's link key.
pub(crate) fn acceptor(
    server: &Arc<EdgeServer>,
    resolver: Arc<dyn rustls::server::ResolvesServerCert>,
    identity: &Identity,
) -> Option<Arc<Acceptor>> {
    let link = server_config(ALPN, |builder| {
        builder.with_single_cert(vec![identity.cert()], identity.key_der())
    });
    let pairing = server_config(ALPN_PAIR, |builder| Ok(builder.with_cert_resolver(resolver)));
    let (link, pairing) = match (link, pairing) {
        (Ok(link), Ok(pairing)) => (Arc::new(link), Arc::new(pairing)),
        (Err(err), _) | (_, Err(err)) => {
            tracing::error!("The edge link is not taken on the HTTPS listener: {err}");
            return None;
        }
    };
    tracing::info!(
        "Edge node: taking edge links on the HTTPS listener (link key {})",
        identity.fingerprint
    );
    Some(Arc::new(Acceptor {
        link,
        pairing,
        server: Arc::clone(server),
    }))
}

/// The most a ClientHello may take. It is read whole before any TLS settings are picked, and a passthrough tunnel
/// replays it whole.
const MAX_HELLO: usize = 64 * 1024;

/// A connection the link takes over from the listener. It is moved once, from the accept to its serve, so the
/// stream is not boxed; [`Secured`] then has two variants of a size.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Handoff {
    /// An up, down, check or pairing connection, after its handshake.
    Link(TlsStream<TcpStream>),
    /// A passthrough host's connection before any handshake, with the bytes read from it so far (the ClientHello),
    /// for its node's link.
    Passthrough {
        stream: TcpStream,
        hello: Vec<u8>,
        node: String,
    },
}

/// A secured connection to be served as HTTP, or one the link takes.
pub(crate) enum Secured {
    Http(TlsStream<TcpStream>),
    Link(Handoff),
}

/// Read the ClientHello from `stream`: rustls's view of it, and every byte read, which a passthrough tunnel replays.
async fn read_hello<S>(stream: &mut S) -> std::io::Result<(rustls::server::Accepted, Vec<u8>)>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut acceptor = rustls::server::Acceptor::default();
    let mut seen = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let fresh = &chunk[..read];
        seen.extend_from_slice(fresh);
        if seen.len() > MAX_HELLO {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "the ClientHello is too long",
            ));
        }
        let mut rest = fresh;
        while !rest.is_empty() {
            if acceptor.read_tls(&mut rest)? == 0 {
                break;
            }
        }
        match acceptor.accept() {
            Ok(Some(accepted)) => return Ok((accepted, seen)),
            Ok(None) => {}
            Err((err, mut alert)) => {
                let mut out = Vec::new();
                if alert.write_all(&mut out).is_ok() {
                    let _ = stream.write_all(&out).await;
                }
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, err));
            }
        }
    }
}

impl Acceptor {
    /// The start of a connection to the HTTPS listener: the link's settings when the client offers one of the link's
    /// ALPN protocols, a passthrough host's link when its server name is one, `http` otherwise.
    pub(crate) async fn accept(
        &self,
        mut stream: TcpStream,
        http: &Arc<rustls::ServerConfig>,
    ) -> std::io::Result<Secured> {
        let (accepted, hello) = read_hello(&mut stream).await?;
        let (link, pairing, acme, server_name) = {
            let client_hello = accepted.client_hello();
            let offered = |wanted: &[u8]| {
                client_hello
                    .alpn()
                    .is_some_and(|mut offered| offered.any(|protocol| protocol == wanted))
            };
            (
                offered(ALPN),
                offered(ALPN_PAIR),
                offered(crate::acme::ALPN),
                client_hello.server_name().map(str::to_string),
            )
        };
        let config = if link {
            Some(&self.link)
        } else if pairing {
            Some(&self.pairing)
        } else {
            None
        };
        if let Some(config) = config {
            let start = tokio_rustls::StartHandshake::from_parts(accepted, stream);
            let stream = start.into_stream(Arc::clone(config)).await?;
            return Ok(Secured::Link(Handoff::Link(stream)));
        }
        // the front set's (plans/Edge-Node2.md §4.3): an edge node behind a front passes no host through
        let passthrough = server_name
            .as_deref()
            .filter(|_| self.server.config.front)
            .and_then(|name| self.server.hosts.route(name))
            .filter(|route| route.mode == hosts::Mode::Passthrough);
        if let Some(Route { node, .. }) = passthrough {
            if !self.server.takes_work(&node) {
                self.server.count("moqui.edgelink.refused", super::TUNNEL_HTTPS);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    format!("a passthrough host of {node}, whose link is down"),
                ));
            }
            return Ok(Secured::Link(Handoff::Passthrough { stream, hello, node }));
        }
        // an ACME challenge for a name this node ends TLS for itself; a passthrough name's went down its tunnel above
        if acme {
            return Err(crate::acme::answer(accepted, stream, &self.server.factory, server_name.as_deref()).await);
        }
        let start = tokio_rustls::StartHandshake::from_parts(accepted, stream);
        Ok(Secured::Http(start.into_stream(Arc::clone(http)).await?))
    }

    /// Serve a connection the link took until it closes.
    pub(crate) async fn serve(&self, handoff: Handoff, remote: SocketAddr) {
        match handoff {
            Handoff::Link(stream) => Arc::clone(&self.server).serve(stream, remote).await,
            Handoff::Passthrough { stream, hello, node } => {
                super::tunnel::passthrough(Arc::clone(&self.server), stream, hello, &node, remote).await;
            }
        }
    }
}

/// A member's live link: its generation, its key, and the down connection once it is open. Dropping it closes its
/// connections.
struct Linked {
    generation: u64,
    fingerprint: String,
    remote: SocketAddr,
    /// The up connection closes when this is dropped or sends.
    _close: watch::Sender<bool>,
    hello: Option<Hello>,
    down: Option<Down>,
    /// As its last Hello or Ping said (plans/Edge-Node2.md §4.4).
    state: MemberState,
    /// The same, for its down connection's queue delivery, which runs only while it is live.
    state_watch: watch::Sender<MemberState>,
    /// When its last Hello or Ping came (epoch milliseconds).
    seen: i64,
    /// The token its down connection must name after its generation (plans/Edge-Node2.md §4.11).
    down_token: [u8; super::DOWN_TOKEN],
    /// When it was registered.
    linked_at: Instant,
    /// The fingerprint of the member's own key, as admitted: a link whose pin was forgotten or changed since, on any
    /// edge member, is closed by the periodic check (plans/Edge-Node2.md §4.6).
    member_key: String,
}

struct Down {
    send: hyper::client::conn::http2::SendRequest<tonic::body::Body>,
    _close: watch::Sender<bool>,
}

/// One pairing's members linked now, by instance id, with the latest member list one of them sent and the nodes with
/// its key refused or held lately (plans/Edge-Node2.md §4.4). In memory only: a restart loses it, and the first link
/// brings it back.
#[derive(Default)]
struct Pool {
    members: HashMap<String, Linked>,
    /// The latest member list, kept after the members go: with none linked, a member of it comes back, not takes over.
    cluster: Option<Vec<ClusterMemberInfo>>,
    /// When a member last sent a list that was taken (not an empty or refused one).
    cluster_at: Option<Instant>,
    /// Newest last, at most [`MAX_REFUSED`].
    refused: Vec<RefusedMember>,
    /// The round robin's next turn ([`EdgeServer::link_of`]).
    turn: usize,
    /// Each member's traffic and health for balancing, kept across its links (at most [`MAX_HEALTH`] once pruned).
    health: HashMap<String, Arc<Health>>,
}

/// The members' health a pool keeps before those neither linked nor busy are let go.
const MAX_HEALTH: usize = 128;

/// The refusals kept per pairing for Clients.
const MAX_REFUSED: usize = 8;
/// How long admission waits for the linked members' member lists, all asked at once.
const ASK_MEMBERS: Duration = Duration::from_secs(3);
/// The oldest list a member sent with its Ping that admission takes when no member answers in time: two Pings.
const LIST_FRESH: Duration = Duration::from_secs(30);
/// How long a member's first link has to pin its key before another link of its id, with none pinned, may replace it.
const YOUNG_LINK: Duration = Duration::from_secs(5);
/// How long a member whose link is up has to answer the Ping that tells whether a reconnect of its id replaces it.
const STILL_LIVE: Duration = Duration::from_secs(1);
/// The takeover and hold notices one pairing sends in [`NOTICE_WINDOW`]; more are only logged.
const NOTICES_PER_WINDOW: usize = 4;
const NOTICE_WINDOW: Duration = Duration::from_secs(3600);

impl Pool {
    fn note_refused(&mut self, refused: RefusedMember) {
        self.refused.retain(|old| old.instance_id != refused.instance_id);
        self.refused.push(refused);
        if self.refused.len() > MAX_REFUSED {
            self.refused.remove(0);
        }
    }

    /// Its members linked now, as admission and the report see them.
    fn linked(&self) -> Vec<LinkedMember> {
        self.members
            .iter()
            .map(|(member, link)| LinkedMember {
                instance_id: member.clone(),
                generation: link.generation,
                down: link.down.as_ref().is_some_and(|down| !down.send.is_closed()),
                remote_address: link.remote.to_string(),
                state: link.state,
                binary_version: link
                    .hello
                    .as_ref()
                    .map(|hello| hello.binary_version.clone())
                    .unwrap_or_default(),
                last_seen: Some(link.seen),
            })
            .collect()
    }
}

/// The live links by pairing, and the pairings revoked since this process started: a link whose key was read as active
/// just before a revoke is refused here, so a revoke always leaves no live link.
#[derive(Default)]
struct Registry {
    pools: HashMap<String, Pool>,
    revoked: HashSet<String>,
}

impl Registry {
    fn link(&self, node: &str, member: &str) -> Option<&Linked> {
        self.pools.get(node)?.members.get(member)
    }

    fn link_mut(&mut self, node: &str, member: &str) -> Option<&mut Linked> {
        self.pools.get_mut(node)?.members.get_mut(member)
    }
}

/// A member's up connection to register, once admitted ([`EdgeServer::register`]).
struct Registration {
    generation: u64,
    fingerprint: String,
    remote: SocketAddr,
    close: watch::Sender<bool>,
    hello: Hello,
    state: MemberState,
    /// The pairing's members linked when the admission was decided.
    linked: Vec<String>,
    /// The fingerprint of the member's own key.
    member_key: String,
}

/// Why [`EdgeServer::register`] refused.
#[derive(Debug, PartialEq, Eq)]
enum Unregistered {
    Revoked,
    /// A newer generation of the member is already live.
    Stale(u64),
    /// The pairing's linked members changed after the admission was decided: dial again.
    Changed,
}

/// The member of a pairing a piece of work goes to ([`EdgeServer::link_of`]): its down connection's sender and the
/// link's generation, with the work counted in flight until this drops.
pub(crate) struct Picked {
    pub node: String,
    pub member: String,
    pub generation: u64,
    pub send: hyper::client::conn::http2::SendRequest<tonic::body::Body>,
    pub flight: Flight,
}

/// The edge node's links and paired nodes.
pub(crate) struct EdgeServer {
    factory: Weak<ExecutionContextFactory>,
    config: EdgeConfig,
    /// The fingerprint of this node's link key, which pairing tells.
    link_fingerprint: String,
    registry: Mutex<Registry>,
    /// Pair calls by source.
    pair_attempts: Mutex<PairAttempts>,
    shutdown: watch::Receiver<bool>,
    /// The paired nodes' public host names (§5.3).
    pub(crate) hosts: Hosts,
    telemetry: Arc<Telemetry>,
    /// The header the web protection reads a trusted proxy's client from, which a forwarded request never carries.
    client_ip_header: String,
    /// Tunnels and forwarded WebSockets open now.
    active: Arc<AtomicI64>,
    /// The places for tunnels and forwarded WebSockets, by node.
    tunnel_slots: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
    /// The takeover and hold notices sent lately, by pairing ([`EdgeServer::notice_allowed`]).
    notices: Mutex<HashMap<String, Vec<Instant>>>,
    /// Balancing's settings, from the `<edge>` conf (plans/Edge-Node2.md §4.4).
    balance: Rules,
    /// The edge cluster's live members as Hello and Ping answers list them, and when they were read
    /// (plans/Edge-Node2.md §4.6).
    edge_members: Mutex<Option<(Instant, Vec<super::members::EdgeMember>)>>,
    /// Wakes the writer of this edge member's `EdgeMemberLinks` row when a link comes or goes
    /// ([`EdgeServer::keep_member_links`]).
    member_links: tokio::sync::Notify,
    /// The links being admitted now, by pairing and member: in this edge member's row from the admission's own write
    /// until they register or are refused.
    admitting: Mutex<HashSet<(String, String)>>,
    /// Held while this edge member's row is made and written, so a write never takes an older view than one before it.
    links_write: Mutex<()>,
}

/// How long the edge cluster's list is answered before its member rows are read again: every link's Ping asks it.
const EDGE_MEMBERS_FOR: Duration = Duration::from_secs(2);

/// The sources whose Pair calls are counted at once; a new source past it is refused until a sweep frees room.
const MAX_PAIR_SOURCES: usize = 10_000;

/// Pair calls by source (an IPv6 /64 is one, an IPv4-mapped address its IPv4 one): when its window began, and how
/// many since. Bounded at [`MAX_PAIR_SOURCES`]; the sources whose window ended are swept at most once a window, not
/// on every call (plans/archive/High-Level-Review.md HL-032).
#[derive(Default)]
pub(super) struct PairAttempts {
    by_source: HashMap<IpAddr, (Instant, u32)>,
    next_sweep: Option<Instant>,
}

impl PairAttempts {
    /// Count a call from `ip` at `now`: false when its source made more than `limit` in `window`, or when the table
    /// is full of sources still in their window.
    pub(super) fn allowed(&mut self, ip: IpAddr, now: Instant, window: Duration, limit: u32) -> bool {
        let key = moqui_lib::source_guard::source_key(ip);
        if self.next_sweep.is_none_or(|at| now >= at) {
            self.by_source
                .retain(|_, (started, _)| now.duration_since(*started) < window);
            self.next_sweep = Some(now + window);
        }
        if self.by_source.len() >= MAX_PAIR_SOURCES && !self.by_source.contains_key(&key) {
            return false;
        }
        let (started, count) = self.by_source.entry(key).or_insert((now, 0));
        if now.duration_since(*started) >= window {
            (*started, *count) = (now, 0);
        }
        *count += 1;
        *count <= limit
    }

    #[cfg(test)]
    pub(super) fn sources(&self) -> usize {
        self.by_source.len()
    }
}

/// Tunnels and forwarded WebSockets open at once down one node's link: well under the down connection's stream limit
/// (`down::MAX_STREAMS`), so the link's own calls and plain forwarded requests keep room.
const MAX_TUNNELS_PER_NODE: usize = 768;

/// A tunnel or forwarded WebSocket counted as open, holding its place among its node's, until dropped.
pub(crate) struct Active {
    active: Arc<AtomicI64>,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl EdgeServer {
    pub(crate) fn new(
        factory: &Arc<ExecutionContextFactory>,
        config: EdgeConfig,
        link_fingerprint: &str,
        shutdown: watch::Receiver<bool>,
    ) -> Arc<Self> {
        let telemetry = moqui_core::telemetry::for_factory(factory);
        let active = Arc::new(AtomicI64::new(0));
        let server = Arc::new(Self {
            factory: Arc::downgrade(factory),
            config,
            link_fingerprint: link_fingerprint.to_string(),
            registry: Mutex::new(Registry::default()),
            pair_attempts: Mutex::new(PairAttempts::default()),
            shutdown,
            hosts: Hosts::default(),
            telemetry: Arc::clone(&telemetry),
            client_ip_header: factory
                .extension(moqui_core::web::address::ClientAddressing::of_factory)
                .header()
                .to_string(),
            active: Arc::clone(&active),
            tunnel_slots: Mutex::new(HashMap::new()),
            notices: Mutex::new(HashMap::new()),
            balance: Rules::of(&moqui_core::edge::settings(factory)),
            edge_members: Mutex::new(None),
            member_links: tokio::sync::Notify::new(),
            admitting: Mutex::new(HashSet::new()),
            links_write: Mutex::new(()),
        });
        telemetry.observe_gauge("moqui.edgelink.tunnels.active", move || {
            Some(active.load(Ordering::Acquire) as f64)
        });
        let weak = Arc::downgrade(&server);
        telemetry.observe_gauge("moqui.edgelink.up", move || {
            let server = weak.upgrade()?;
            let linked = lock(&server.registry)
                .pools
                .values()
                .flat_map(|pool| pool.members.values())
                .filter(|link| link.down.is_some())
                .count();
            Some(linked as f64)
        });
        server
    }

    /// The fingerprint of this node's link key: public, what local nodes pin.
    pub(crate) fn link_fingerprint(&self) -> &str {
        &self.link_fingerprint
    }

    pub(crate) fn config(&self) -> &EdgeConfig {
        &self.config
    }

    /// The header the web protection reads a trusted proxy's client from.
    pub(crate) fn client_ip_header(&self) -> &str {
        &self.client_ip_header
    }

    /// The listeners' stop.
    pub(crate) fn shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.clone()
    }

    /// Balancing's settings.
    pub(crate) fn balance(&self) -> &Rules {
        &self.balance
    }

    /// Whether a member of pairing `node` takes new work now: linked with its down connection open, and live.
    pub(crate) fn takes_work(&self, node: &str) -> bool {
        lock(&self.registry).pools.get(node).is_some_and(|pool| {
            pool.members
                .values()
                .any(|link| link.state.takes_work() && link.down.as_ref().is_some_and(|down| !down.send.is_closed()))
        })
    }

    /// The pairings with a member linked, down connection and all.
    pub(crate) fn linked_nodes(&self) -> Vec<String> {
        lock(&self.registry)
            .pools
            .iter()
            .filter(|(_, pool)| pool.members.values().any(|link| link.down.is_some()))
            .map(|(node, _)| node.clone())
            .collect()
    }

    /// The members of each pairing (of `node` when given) for Clients (plans/Edge-Node2.md §4.4): the links, the
    /// latest member lists and the nodes refused or held lately.
    pub(crate) fn members(&self, node: Option<&str>) -> Result<Vec<PoolMemberInfo>, Error> {
        let registry = lock(&self.registry);
        let mut nodes: Vec<&String> = registry
            .pools
            .keys()
            .filter(|key| node.is_none_or(|node| node == key.as_str()))
            .collect();
        nodes.sort();
        Ok(nodes
            .into_iter()
            .flat_map(|key| {
                let pool = &registry.pools[key];
                pool::report(key, &pool.linked(), pool.cluster.as_deref(), &pool.refused)
            })
            .collect())
    }

    /// Add one to the counter `name`, of a tunnel or forward `kind`.
    pub(crate) fn count(&self, name: &str, kind: &str) {
        let mut attributes = moqui_core::entity::document::Document::default();
        attributes.insert(
            "moqui.edgelink.kind".to_string(),
            moqui_lib::object::Value::string(kind),
        );
        self.telemetry.add_counter(name, 1, attributes);
    }

    /// Add one to the counter `name`, of a forward, tunnel or call `kind` to member `member` of pairing `node`
    /// (plans/Edge-Node2.md §4.10, metrics).
    pub(crate) fn count_member(&self, name: &str, kind: &str, node: &str, member: &str) {
        let mut attributes = moqui_core::entity::document::Document::default();
        for (attribute, value) in [
            ("moqui.edgelink.kind", kind),
            ("moqui.edgelink.pairing", node),
            ("moqui.edgelink.member", member),
        ] {
            attributes.insert(attribute.to_string(), moqui_lib::object::Value::string(value));
        }
        self.telemetry.add_counter(name, 1, attributes);
    }

    /// The work of `picked` of `kind` was answered: its member's failures in a row start again.
    pub(crate) fn succeeded(&self, picked: &Picked) {
        picked.flight.succeeded();
    }

    /// The work of `picked` of `kind` failed or timed out before an answer: counted toward setting its member aside
    /// (plans/Edge-Node2.md §4.4, health), logged and counted when it does.
    pub(crate) fn failed(&self, picked: &Picked, kind: &str) {
        if picked.flight.failed() {
            self.count_member("moqui.edgelink.set_asides", kind, &picked.node, &picked.member);
            tracing::warn!(
                "Edge link of {} member {}: {} failures in a row; set aside for {:?}",
                picked.node,
                picked.member,
                self.balance.failures,
                self.balance.aside_for
            );
        }
    }

    /// Balancing's defaults and each member's traffic, of pairing `node` when given (plans/Edge-Node2.md §4.10
    /// Balancing): every member linked or with health kept, by pairing and id.
    pub(crate) fn balancing(&self, node: Option<&str>) -> moqui_core::edge::BalancingInfo {
        let now = Instant::now();
        let wall = moqui_core::user::now_millis();
        let registry = lock(&self.registry);
        let mut members = Vec::new();
        let mut nodes: Vec<&String> = registry
            .pools
            .keys()
            .filter(|key| node.is_none_or(|node| node == key.as_str()))
            .collect();
        nodes.sort();
        for key in nodes {
            let pool = &registry.pools[key];
            let mut ids: Vec<&String> = pool.members.keys().chain(pool.health.keys()).collect();
            ids.sort();
            ids.dedup();
            for id in ids {
                let link = pool.members.get(id);
                let health = pool.health.get(id).map(|health| health.now(now)).unwrap_or_default();
                let linked = link.is_some_and(|link| link.down.as_ref().is_some_and(|down| !down.send.is_closed()));
                let live = link.is_some_and(|link| link.state.takes_work());
                let aside_until = health.aside_until.map(|until| {
                    let left = i64::try_from(until.saturating_duration_since(now).as_millis()).unwrap_or(i64::MAX);
                    wall.saturating_add(left)
                });
                members.push(moqui_core::edge::BalanceMemberInfo {
                    paired_node_id: key.clone(),
                    instance_id: id.clone(),
                    linked,
                    state: link.map(|link| link.state.as_str().to_string()).unwrap_or_default(),
                    takes_work: linked && live && aside_until.is_none(),
                    in_flight: health.in_flight,
                    requests: health.requests,
                    failures_in_row: health.failures_in_row,
                    set_asides: health.set_asides,
                    set_aside_until: aside_until,
                });
            }
        }
        moqui_core::edge::BalancingInfo {
            sticky_cookie: self.balance.sticky_cookie.to_string(),
            set_aside_failures: self.balance.failures,
            set_aside_seconds: self.balance.aside_for.as_secs(),
            members,
        }
    }

    /// A place for a tunnel or forwarded WebSocket of `kind` down `node`'s link, counted as open until the answer
    /// drops; `None`, counted as refused, when the node has [`MAX_TUNNELS_PER_NODE`] open already.
    pub(crate) fn opened(&self, node: &str, kind: &str) -> Option<Active> {
        let slots = Arc::clone(
            lock(&self.tunnel_slots)
                .entry(node.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(MAX_TUNNELS_PER_NODE))),
        );
        let Ok(slot) = slots.try_acquire_owned() else {
            self.count("moqui.edgelink.refused", kind);
            tracing::warn!(
                "Edge link of {node}: {MAX_TUNNELS_PER_NODE} tunnels open already; a new {kind} one is refused"
            );
            return None;
        };
        self.count("moqui.edgelink.tunnels", kind);
        self.active.fetch_add(1, Ordering::AcqRel);
        Some(Active {
            active: Arc::clone(&self.active),
            _slot: slot,
        })
    }

    /// Load the host names, trying again with a backoff from 1 s to 30 s until a load works (the database may not be
    /// ready yet), or the listeners stop.
    pub(crate) async fn load_hosts(self: &Arc<Self>) {
        let mut wait = Duration::from_secs(1);
        let mut shutdown = self.shutdown.clone();
        loop {
            let server = Arc::clone(self);
            let loaded = self.on_pool(move |factory| server.hosts.load(factory)).await;
            match loaded {
                Ok(count) => {
                    tracing::info!("Edge node: answering for {count} host names of paired local nodes");
                    return;
                }
                Err(err) => {
                    tracing::error!("Edge node: the host names were not loaded ({err}); trying again in {wait:?}")
                }
            }
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                _ = shutdown.wait_for(|stopped| *stopped) => return,
            }
            wait = (wait * 2).min(Duration::from_secs(30));
        }
    }

    fn factory(&self) -> Result<Arc<ExecutionContextFactory>, Error> {
        self.factory
            .upgrade()
            .ok_or_else(|| Error::new("the server is stopping"))
    }

    /// Run a store function on a request worker, with authorization off.
    async fn on_pool<T, F>(&self, job: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce(&ExecutionContextFactory) -> Result<T, Error> + Send + 'static,
    {
        crate::worker::on_pool_without_authz(&self.factory, move |factory| job(factory).map_err(CoreError::from))
            .await
            .map_err(|err| Error::new(err.message().to_string()))
    }

    fn hello(&self) -> Hello {
        let mut hello = hello(
            &self.config.instance_id,
            &self.config.name,
            &self.config.capabilities,
            &self.config.public_names,
        );
        // a media node of its local nodes' calls (plans/archive/Edge-Node.md §5.4, E7): its media address and TURN URLs, and
        // its area (§5.7, E10 calls) as the media socket is now, not as it was when the areas were read at start
        hello
            .capabilities
            .retain(|area| area != super::offer::CALLS_AREA && area != super::offer::SIP_AREA);
        // the media node and SIP are the edge set's (plans/Edge-Node2.md §4.3): a front node offers neither
        let edge_set = self
            .factory
            .upgrade()
            .is_some_and(|factory| moqui_core::edge::settings(&factory).role.has_edge());
        if let Some(advert) = self.media().filter(|_| edge_set) {
            hello.media_address = advert.media_address;
            hello.turn_urls = advert.turn_urls;
            hello.media_started_ms = u64::try_from(advert.started_ms).unwrap_or(0);
            hello.capabilities.push(super::offer::CALLS_AREA.to_string());
        }
        // its SIP engine's addresses, from which the local node keeps its SipNode row (E8), and its area (E10 phones)
        // as the engine is now
        if let Some(sip) = self
            .factory
            .upgrade()
            .filter(|_| edge_set)
            .and_then(|factory| moqui_core::edge::media::sip_advert(&factory))
        {
            hello.sip_address = sip.sip_address;
            hello.sip_tls_address = sip.tls_address;
            hello.sip_domain = sip.domain;
            hello.sip_registrar = sip.registrar;
            hello.capabilities.push(super::offer::SIP_AREA.to_string());
        }
        hello
    }

    /// The live members of this node's cluster with their link addresses, itself first (plans/Edge-Node2.md §4.6): what
    /// its Hello and Ping answers list, read again at most every [`EDGE_MEMBERS_FOR`]. Empty when the member rows
    /// cannot be read (logged), which a local node takes as not told: it keeps the list it has.
    async fn edge_members(&self) -> Vec<super::proto::EdgeMember> {
        if let Some((read, list)) = lock(&self.edge_members).as_ref() {
            if read.elapsed() < EDGE_MEMBERS_FOR {
                return super::members::to_proto(list);
            }
        }
        let read = self
            .on_pool(|factory| {
                let facade = factory
                    .entity()
                    .ok_or_else(|| Error::new("the entity facade is not installed"))?;
                moqui_core::cluster::member_report(factory, &facade)
                    .map_err(|err| Error::new(err.message().to_string()))
            })
            .await;
        match read {
            Ok(rows) => {
                let list = super::members::of_cluster(&self.config.instance_id, &self.config.link_url, &rows);
                let answer = super::members::to_proto(&list);
                *lock(&self.edge_members) = Some((Instant::now(), list));
                answer
            }
            Err(err) => {
                tracing::warn!("Edge link: this edge cluster's members were not read for a local node: {err}");
                Vec::new()
            }
        }
    }

    /// The members of this node's cluster with their link addresses, live or not (plans/Edge-Node2.md §4.6), for
    /// `get#EdgeCluster`. In the current context.
    pub(crate) fn edge_cluster(&self) -> Result<Vec<moqui_core::edge::EdgeClusterMember>, Error> {
        let factory = self
            .factory
            .upgrade()
            .ok_or_else(|| Error::new("the server is stopping"))?;
        let facade = factory
            .entity()
            .ok_or_else(|| Error::new("the entity facade is not installed"))?;
        moqui_core::edge::cluster_list::members(&factory, &facade, &self.config.instance_id, &self.config.link_url)
            .map_err(|err| Error::new(err.message().to_string()))
    }

    /// What this node's media node advertises, while its media socket is up.
    fn media(&self) -> Option<moqui_core::edge::media::MediaAdvert> {
        let factory = self.factory.upgrade()?;
        moqui_core::edge::media::advert(&factory).filter(|advert| !advert.media_address.is_empty())
    }

    /// The secret this node's embedded TURN checks the credentials of local node `node` with, while it runs: for that
    /// node's Hello answer only, so it signs the credentials of this node's TURN URLs (E7). It is the pairing's own
    /// (plans/Edge-Node2.md §4.1, E16), derived from `turn_auth_secret`, the pairing's id and `salt` (its key's
    /// fingerprint and when it paired, `moqui_core::edge::media::pairing_turn_salt`), never the shared secret: the
    /// TURN server takes it only while the pairing is active and has that salt, so revoking a pairing ends its TURN
    /// access with no new `turn_auth_secret`, and pairing it again gives it a new secret. Empty without TURN.
    fn turn_secret(&self, node: &str, salt: &str) -> String {
        if self.media().is_none_or(|advert| advert.turn_urls.is_empty()) {
            return String::new();
        }
        let secret = moqui_lib::env::get_prop_or_env("turn_auth_secret");
        let secret = secret.trim();
        if secret.is_empty() || secret.ends_with("CHANGEME") || node.is_empty() {
            return String::new();
        }
        #[cfg(feature = "rtc")]
        {
            moqui_rtc::turn::pairing_secret(secret.as_bytes(), node, salt)
        }
        #[cfg(not(feature = "rtc"))]
        {
            let _ = salt;
            String::new()
        }
    }

    /// The generation of member `member`'s live link of pairing `node`, or 0.
    fn current(&self, node: &str, member: &str) -> u64 {
        lock(&self.registry)
            .link(node, member)
            .map_or(0, |link| link.generation)
    }

    /// Register member `member`'s new up connection of pairing `node`, closing only that member's link of an older
    /// generation, which it replaces (plans/Edge-Node2.md §4.4); its `close` closes this one when dropped. A revoked
    /// pairing, a generation older than the member's live one, and a pairing whose linked members are no longer the
    /// ones the admission was decided on (two new nodes at once, plans/Edge-Node2.md §4.11) are refused. The answer is
    /// the token the member's down connection must name.
    fn register(
        &self,
        node: &str,
        member: &str,
        registration: Registration,
    ) -> Result<[u8; super::DOWN_TOKEN], Unregistered> {
        let mut token = [0_u8; super::DOWN_TOKEN];
        rustls::crypto::aws_lc_rs::default_provider()
            .secure_random
            .fill(&mut token)
            .map_err(|_| Unregistered::Changed)?;
        let Registration {
            generation,
            fingerprint,
            remote,
            close,
            hello,
            state,
            linked,
            member_key,
        } = registration;
        let mut registry = lock(&self.registry);
        if registry.revoked.contains(node) {
            return Err(Unregistered::Revoked);
        }
        if let Some(live) = registry.link(node, member).filter(|live| live.generation >= generation) {
            return Err(Unregistered::Stale(live.generation));
        }
        let pool = registry.pools.entry(node.to_string()).or_default();
        let mut now: Vec<&String> = pool.members.keys().collect();
        now.sort();
        let mut then: Vec<&String> = linked.iter().collect();
        then.sort();
        if now != then {
            return Err(Unregistered::Changed);
        }
        pool.refused.retain(|refused| refused.instance_id != member);
        let (state_watch, _) = watch::channel(state);
        let replaced = pool.members.insert(
            member.to_string(),
            Linked {
                generation,
                fingerprint,
                remote,
                _close: close,
                hello: Some(hello),
                down: None,
                state,
                state_watch,
                seen: moqui_core::user::now_millis(),
                down_token: token,
                linked_at: Instant::now(),
                member_key,
            },
        );
        drop(registry);
        if let Some(old) = replaced {
            tracing::info!(
                "Edge link of {node} member {member}: generation {generation} from {remote} replaces generation {} \
                 from {}",
                old.generation,
                old.remote
            );
        }
        Ok(token)
    }

    /// Forget member `member`'s link of pairing `node` if it is still of `generation`.
    fn unregister(&self, node: &str, member: &str, generation: u64) {
        let removed = {
            let mut registry = lock(&self.registry);
            let Some(pool) = registry.pools.get_mut(node) else {
                return;
            };
            if pool
                .members
                .get(member)
                .is_some_and(|link| link.generation == generation)
            {
                pool.members.remove(member)
            } else {
                None
            }
        };
        if removed.is_some() {
            self.member_links.notify_one();
        }
        drop(removed);
    }

    /// Close every member's link of pairing `node`, whatever its generation, and forget what was kept of it.
    pub(crate) fn close(&self, node: &str) -> bool {
        let removed = lock(&self.registry).pools.remove(node);
        self.member_links.notify_one();
        removed.is_some_and(|pool| !pool.members.is_empty())
    }

    /// Do to the links held here what another member of an edge cluster did to its own (plans/Edge-Node2.md §4.6,
    /// `moqui_core::edge::cluster_state`): a pairing revoked or unpaired, paired again, or a member's key forgotten.
    pub(crate) fn apply_link_op(&self, op: &LinkOp) {
        match op {
            LinkOp::Revoked { node } => {
                self.revoked(node);
                tracing::info!("Edge node: pairing {node} revoked on another member; its links here are closed");
            }
            LinkOp::Paired { node, key_changed } => {
                lock(&self.registry).revoked.remove(node);
                if *key_changed && self.close(node) {
                    tracing::info!("Edge node: {node} paired again on another member; its old key's links here closed");
                }
            }
            LinkOp::MemberForgotten { node, member } => {
                if self.close_member(node, member) {
                    tracing::warn!(
                        "Edge node: the link of member {member} of {node} is closed, its key forgotten on another member"
                    );
                }
            }
        }
    }

    /// Mark `node` revoked and close its members' links, in one step under the registry's lock.
    fn revoked(&self, node: &str) {
        let removed = {
            let mut registry = lock(&self.registry);
            registry.revoked.insert(node.to_string());
            registry.pools.remove(node)
        };
        self.member_links.notify_one();
        drop(removed);
    }

    /// The links this edge member holds and is admitting now, for its `EdgeMemberLinks` row.
    fn held_links(&self) -> HeldLinks {
        let mut held = HeldLinks::new();
        {
            let registry = lock(&self.registry);
            for (node, pool) in &registry.pools {
                held.entry(node.clone())
                    .or_default()
                    .extend(pool.members.keys().cloned());
            }
        }
        for (node, member) in lock(&self.admitting).iter() {
            held.entry(node.clone()).or_default().insert(member.clone());
        }
        held.retain(|_, members| !members.is_empty());
        held
    }

    /// What the other edge members' rows say of pairing `node` ([`moqui_core::edge::cluster_state::linked_elsewhere`]).
    fn read_elsewhere(&self, factory: &ExecutionContextFactory, node: &str) -> Result<Elsewhere, Error> {
        let facade = store::facade(factory)?;
        moqui_core::edge::cluster_state::linked_elsewhere(factory, &facade, &Self::edge_member_id(factory), node)
            .map_err(|err| Error::new(err.message().to_string()))
    }

    /// [`EdgeServer::read_elsewhere`] on a request worker.
    async fn linked_elsewhere(self: &Arc<Self>, node: &str) -> Result<Elsewhere, Error> {
        let (server, key) = (Arc::clone(self), node.to_string());
        self.on_pool(move |factory| server.read_elsewhere(factory, &key)).await
    }

    /// This edge member's id in its cluster, as its `EdgeMemberLinks` row names it.
    fn edge_member_id(factory: &ExecutionContextFactory) -> String {
        moqui_core::cluster::instance_id(factory).unwrap_or_default()
    }

    /// Write this edge member's row from what it holds and admits now, under [`EdgeServer::links_write`], in the
    /// caller's transaction when there is one.
    fn write_links(&self, factory: &ExecutionContextFactory) -> Result<(), Error> {
        let facade = store::facade(factory)?;
        let _writing = lock(&self.links_write);
        let held = self.held_links();
        moqui_core::edge::cluster_state::write_member_links(&facade, &Self::edge_member_id(factory), &held)
            .map_err(|err| Error::new(err.message().to_string()))
    }

    /// The links this edge member holds whose pairing is no longer active, or whose member's pinned key is forgotten
    /// or another (plans/Edge-Node2.md §4.6): what another member did and this one may not have heard.
    fn stale_links(&self, factory: &ExecutionContextFactory) -> Result<(Vec<String>, Vec<(String, String)>), Error> {
        let held: Vec<(String, Vec<(String, String)>)> = {
            let registry = lock(&self.registry);
            registry
                .pools
                .iter()
                .map(|(node, pool)| {
                    let members = pool
                        .members
                        .iter()
                        .map(|(member, link)| (member.clone(), link.member_key.clone()))
                        .collect();
                    (node.clone(), members)
                })
                .collect()
        };
        let mut revoked = Vec::new();
        let mut forgotten = Vec::new();
        for (node, members) in held {
            let Some((active, pins)) = store::pairing_state(factory, &node)? else {
                revoked.push(node);
                continue;
            };
            if !active {
                revoked.push(node);
                continue;
            }
            for (member, key) in members {
                if pool::key_matches(&pins, &member, &key) != Some(true) {
                    forgotten.push((node.clone(), member));
                }
            }
        }
        Ok((revoked, forgotten))
    }

    /// Keep this edge member's `EdgeMemberLinks` row (plans/Edge-Node2.md §4.6, §4.11) and check its links against the
    /// shared rows, while another member of its cluster is live (a node alone writes and checks nothing; it reads its
    /// cluster's member rows each time unless the heartbeat already knows a peer): every
    /// [`moqui_core::edge::cluster_state::MEMBER_LINK_BEAT`] and at once when a link comes or goes, until the
    /// listeners stop, when its row is removed. A pairing revoked, or a member's key forgotten or pinned again, on a
    /// member whose notice this one missed has its links closed here too. A write or check that fails is logged, and
    /// the next one goes on.
    pub(crate) async fn keep_member_links(self: &Arc<Self>) {
        let mut shutdown = self.shutdown.clone();
        let mut beats: u64 = 0;
        loop {
            let server = Arc::clone(self);
            let prune = beats.is_multiple_of(60);
            beats = beats.wrapping_add(1);
            let done = self
                .on_pool(move |factory| {
                    // only while another member of the cluster is live: as the heartbeat read them, else by its rows
                    // (the heartbeat may not have run since one came up)
                    let peers = moqui_core::cluster::has_live_peers(factory) || {
                        let facade = store::facade(factory)?;
                        moqui_core::cluster::live_member_count(factory, &facade)
                            .map_err(|err| Error::new(err.message().to_string()))?
                            > 1
                    };
                    if !peers {
                        return Ok(None);
                    }
                    server.write_links(factory)?;
                    if prune {
                        let facade = store::facade(factory)?;
                        if let Err(err) = moqui_core::edge::cluster_state::remove_member_links(&facade, None) {
                            tracing::warn!("Edge node: the rows of edge members gone were not pruned: {err}");
                        }
                    }
                    server.stale_links(factory).map(Some)
                })
                .await;
            match done {
                Ok(Some((revoked, forgotten))) => {
                    for node in revoked {
                        // closed, not marked revoked: its key is checked as each new link opens, and a pairing made
                        // again is let in whether or not this member heard of it
                        if self.close(&node) {
                            tracing::warn!("Edge node: pairing {node} is not active; its links here are closed");
                        }
                    }
                    for (node, member) in forgotten {
                        if self.close_member(&node, &member) {
                            tracing::warn!(
                                "Edge node: member {member} of {node} has another or a forgotten key now; its link here \
                                 is closed"
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::warn!("Edge node: the links this member holds were not written or checked: {err}"),
            }
            tokio::select! {
                () = tokio::time::sleep(moqui_core::edge::cluster_state::MEMBER_LINK_BEAT) => {}
                () = self.member_links.notified() => {}
                _ = shutdown.wait_for(|stopped| *stopped) => break,
            }
        }
        // stopping: the others no longer count this member's links
        let removed = self
            .on_pool(|factory| {
                let facade = store::facade(factory)?;
                moqui_core::edge::cluster_state::remove_member_links(&facade, Some(&Self::edge_member_id(factory)))
                    .map_err(|err| Error::new(err.message().to_string()))
            })
            .await;
        if let Err(err) = removed {
            tracing::warn!("Edge node: this member's row of links was not removed as it stops: {err}");
        }
    }

    /// Take what a member's Hello or Ping said of its state and its cluster (plans/Edge-Node2.md §4.4): the state on
    /// its link, and the member list, checked as input, as the pairing's latest. A list that fails the check is
    /// logged and not taken.
    fn note_status(&self, node: &str, member: &str, generation: u64, status: Option<&MemberStatus>) {
        let Some(status) = status else {
            return;
        };
        let (state, list) = super::status_from_proto(status);
        let list = match pool::check_member_list(&list) {
            Ok(()) => Some(list),
            Err(why) => {
                tracing::warn!("Edge link of {node} member {member}: its member list is not taken: {why}");
                None
            }
        };
        let mut registry = lock(&self.registry);
        let Some(pool) = registry.pools.get_mut(node) else {
            return;
        };
        let Some(link) = pool
            .members
            .get_mut(member)
            .filter(|link| link.generation == generation)
        else {
            return;
        };
        match state {
            Some(state) => {
                if link.state != state {
                    tracing::info!("Edge link of {node} member {member}: {}", state.as_str());
                }
                link.state = state;
                link.state_watch.send_replace(state);
            }
            None => tracing::warn!(
                "Edge link of {node} member {member}: state {} is not one this node knows",
                status.state.chars().take(20).collect::<String>()
            ),
        }
        link.seen = moqui_core::user::now_millis();
        if let Some(list) = list.filter(|list| !list.is_empty()) {
            pool.cluster = Some(list);
            pool.cluster_at = Some(Instant::now());
        }
    }

    /// Ask the members of pairing `node` other than `asking`, with their down connections open, for their cluster's
    /// members now (plans/Edge-Node2.md §4.4, admission): all at once, within one [`ASK_MEMBERS`], the first good
    /// answer taken and kept as the pairing's latest list. `None` when none answered in time; each list is checked as
    /// input.
    async fn ask_members(&self, node: &str, asking: &str) -> Option<Vec<ClusterMemberInfo>> {
        use futures_util::StreamExt as _;
        let asked: Vec<(String, u64, hyper::client::conn::http2::SendRequest<tonic::body::Body>)> = {
            let registry = lock(&self.registry);
            let pool = registry.pools.get(node)?;
            pool.members
                .iter()
                .filter(|(member, _)| member.as_str() != asking)
                .filter_map(|(member, link)| {
                    let down = link.down.as_ref().filter(|down| !down.send.is_closed())?;
                    Some((member.clone(), link.generation, down.send.clone()))
                })
                .collect()
        };
        let mut asks: futures_util::stream::FuturesUnordered<_> = asked
            .into_iter()
            .map(|(member, generation, send)| async move {
                let mut client = super::local_link_client(send);
                let answer = client.members(MembersRequest { generation }).await;
                (member, generation, answer)
            })
            .collect();
        let budget = tokio::time::sleep(ASK_MEMBERS);
        tokio::pin!(budget);
        loop {
            tokio::select! {
                next = asks.next() => match next {
                    None => return None,
                    Some((member, generation, Ok(reply))) => {
                        let status = reply.into_inner();
                        let (_, list) = super::status_from_proto(&status);
                        match pool::check_member_list(&list) {
                            Ok(()) => {
                                self.note_status(node, &member, generation, Some(&status));
                                return Some(list);
                            }
                            Err(why) => tracing::warn!(
                                "Edge link of {node} member {member}: its member list is not taken: {why}"
                            ),
                        }
                    }
                    Some((member, _, Err(status))) => tracing::info!(
                        "Edge link of {node}: member {member} did not give its member list: {}",
                        status.message()
                    ),
                },
                () = &mut budget => {
                    tracing::info!("Edge link of {node}: no member gave its member list in time");
                    return None;
                }
            }
        }
    }

    /// The latest member list of pairing `node` while it is at most [`LIST_FRESH`] old: what admission takes when no
    /// member answered in time.
    fn fresh_list(&self, node: &str) -> Option<Vec<ClusterMemberInfo>> {
        let registry = lock(&self.registry);
        let pool = registry.pools.get(node)?;
        pool.cluster_at
            .filter(|at| at.elapsed() <= LIST_FRESH)
            .and(pool.cluster.clone())
    }

    /// How long member `member`'s link of pairing `node` has been registered, while it is.
    fn link_age(&self, node: &str, member: &str) -> Option<Duration> {
        lock(&self.registry)
            .link(node, member)
            .map(|link| link.linked_at.elapsed())
    }

    /// Close member `member`'s link of pairing `node` (its key was forgotten, plans/Edge-Node2.md §4.11): its next link
    /// is admitted afresh.
    pub(crate) fn close_member(&self, node: &str, member: &str) -> bool {
        let removed = lock(&self.registry)
            .pools
            .get_mut(node)
            .and_then(|pool| pool.members.remove(member));
        self.member_links.notify_one();
        removed.is_some()
    }

    /// Whether member `member`'s link of pairing `node` still answers a Ping on its down connection within
    /// [`STILL_LIVE`]: a reconnect of its id does not replace a link that does (plans/Edge-Node2.md §4.11).
    async fn still_answers(&self, node: &str, member: &str) -> bool {
        let (send, generation) = {
            let registry = lock(&self.registry);
            let Some(link) = registry.link(node, member) else {
                return false;
            };
            let Some(down) = link.down.as_ref().filter(|down| !down.send.is_closed()) else {
                return false;
            };
            (down.send.clone(), link.generation)
        };
        let mut client = super::local_link_client(send);
        let request = PingRequest {
            generation,
            status: None,
        };
        matches!(tokio::time::timeout(STILL_LIVE, client.ping(request)).await, Ok(Ok(_)))
    }

    /// Whether pairing `node` may send another takeover or hold notice now: at most [`NOTICES_PER_WINDOW`] in
    /// [`NOTICE_WINDOW`], so cycling ids does not flood the administrators.
    fn notice_allowed(&self, node: &str) -> bool {
        let mut notices = lock(&self.notices);
        let sent = notices.entry(node.to_string()).or_default();
        sent.retain(|at| at.elapsed() < NOTICE_WINDOW);
        if sent.len() >= NOTICES_PER_WINDOW {
            return false;
        }
        sent.push(Instant::now());
        true
    }

    /// Ask member `member` of pairing `node` down its link to drain, or to come back (plans/Edge-Node2.md §4.4): its
    /// state after, which its link then has.
    pub(crate) async fn drain_member(&self, node: &str, member: &str, drain: bool) -> Result<MemberState, String> {
        let (send, generation) = {
            let registry = lock(&self.registry);
            let link = registry
                .link(node, member)
                .ok_or_else(|| format!("member {member} of {node} is not linked"))?;
            let down = link
                .down
                .as_ref()
                .ok_or_else(|| format!("member {member} of {node} has no down connection"))?;
            (down.send.clone(), link.generation)
        };
        let mut client = super::local_link_client(send);
        let request = DrainRequest { generation, drain };
        let status = match tokio::time::timeout(self.config.handshake_timeout, client.drain(request)).await {
            Ok(Ok(reply)) => reply.into_inner(),
            Ok(Err(status)) => return Err(format!("member {member} of {node}: {}", status.message())),
            Err(_) => return Err(format!("member {member} of {node} did not answer in time")),
        };
        self.note_status(node, member, generation, Some(&status));
        tracing::warn!(
            "Edge node: member {member} of {node} {} at the operator's ask",
            if drain { "drains" } else { "resumes" }
        );
        Ok(MemberState::parse(&status.state).unwrap_or_default())
    }

    /// Count a Pair call from `ip`; false when it is over the limit for the window.
    fn pair_allowed(&self, ip: IpAddr) -> bool {
        lock(&self.pair_attempts).allowed(ip, Instant::now(), self.config.pair_window, self.config.pair_attempts)
    }

    async fn serve(self: Arc<Self>, mut stream: TlsStream<TcpStream>, remote: SocketAddr) {
        let (fingerprint, alpn) = {
            let tls = stream.get_ref().1;
            let fingerprint = tls
                .peer_certificates()
                .and_then(|chain| chain.first())
                .and_then(public_key_sha256)
                .map(|digest| hex(&digest));
            (fingerprint, tls.alpn_protocol().map(<[u8]>::to_vec))
        };
        let mut role = [0_u8; 1];
        let read = tokio::time::timeout(self.config.handshake_timeout, stream.read_exact(&mut role)).await;
        if !matches!(read, Ok(Ok(_))) {
            tracing::debug!("Edge link from {remote}: no role");
            return;
        }
        let pairing = alpn.as_deref() == Some(ALPN_PAIR);
        match (role[0], pairing) {
            (ROLE_CHECK, true) => self.serve_pairing(stream, remote, None).await,
            (ROLE_PAIR, true) => match fingerprint {
                Some(fingerprint) => self.serve_pairing(stream, remote, Some(fingerprint)).await,
                None => {
                    let message = "pairing needs the local node's client certificate";
                    let _ = write_answer(&mut stream, Answer::Refused, message).await;
                }
            },
            (ROLE_UP, false) => self.serve_up(stream, remote, fingerprint).await,
            (ROLE_DOWN, false) => self.serve_down(stream, remote, fingerprint).await,
            (other, _) => {
                tracing::debug!("Edge link from {remote}: role {other} on the wrong protocol, or unknown");
                let _ = write_answer(&mut stream, Answer::Failed, "unknown role for this protocol").await;
            }
        }
    }

    /// A check connection (no key) or a pairing connection (the new key's fingerprint): a few calls, for a short time,
    /// two streams at a time.
    async fn serve_pairing(self: Arc<Self>, mut stream: TlsStream<TcpStream>, remote: SocketAddr, key: Option<String>) {
        if write_answer(&mut stream, Answer::Ok, "").await.is_err() {
            return;
        }
        let service = LinkService {
            server: Arc::clone(&self),
            mode: Mode::Pairing { key },
            remote,
            helloed: Arc::new(AtomicBool::new(false)),
            calls: AtomicU32::new(0),
        };
        let limits = Limits {
            lifetime: Some(self.config.pairing_lifetime),
            streams: Some(2),
            hello: None,
        };
        self.serve_h2(stream, link_server(service), None, limits).await;
    }

    async fn serve_up(self: Arc<Self>, mut stream: TlsStream<TcpStream>, remote: SocketAddr, key: Option<String>) {
        const NOT_PAIRED: &str = "this node's key is not paired with this edge node, or was revoked; pair again";
        let Some(fingerprint) = key else {
            let message = "a link needs this node's client certificate";
            let _ = write_answer(&mut stream, Answer::Refused, message).await;
            return;
        };
        let lookup = fingerprint.clone();
        let linked = self
            .on_pool(move |factory| store::link_up(factory, &lookup, remote))
            .await;
        let (node, generation) = match linked {
            Ok(Some(linked)) => linked,
            Ok(None) => {
                tracing::info!("Edge link from {remote} refused: key {fingerprint} is not paired or was revoked");
                let _ = write_answer(&mut stream, Answer::Refused, NOT_PAIRED).await;
                return;
            }
            Err(err) => {
                tracing::error!("Edge link from {remote}: {err}");
                let _ = write_answer(&mut stream, Answer::Failed, "the edge node could not take the link").await;
                return;
            }
        };
        if lock(&self.registry).revoked.contains(&node) {
            tracing::info!("Edge link of {node} from {remote} refused: revoked");
            let _ = write_answer(&mut stream, Answer::Refused, NOT_PAIRED).await;
            return;
        }
        // a member's proof of its own key is bound to this connection (plans/Edge-Node2.md §4.11)
        let Some(binding) = super::member_key::binding(stream.get_ref().1) else {
            let _ = write_answer(&mut stream, Answer::Failed, "the link's TLS gave no exporter value").await;
            return;
        };
        if write_answer(&mut stream, Answer::Ok, "").await.is_err() {
            return;
        }
        // the member is named in its Hello, which registers the link once it is admitted (plans/Edge-Node2.md §4.4);
        // until then the connection closes when this sender goes, with the service
        let (close, closed) = watch::channel(false);
        let helloed = Arc::new(AtomicBool::new(false));
        let member = Arc::new(std::sync::OnceLock::new());
        let service = LinkService {
            server: Arc::clone(&self),
            mode: Mode::Up {
                node: node.clone(),
                generation,
                fingerprint,
                binding,
                member: Arc::clone(&member),
                close: Mutex::new(Some(close)),
            },
            remote,
            helloed: Arc::clone(&helloed),
            calls: AtomicU32::new(0),
        };
        let limits = Limits {
            lifetime: None,
            streams: None,
            hello: Some((self.config.handshake_timeout, helloed)),
        };
        self.serve_h2(stream, link_server(service), Some(closed), limits).await;
        if let Some(member) = member.get() {
            self.unregister(&node, member, generation);
            tracing::debug!("Edge link of {node} member {member} (generation {generation}) closed");
        }
    }

    /// Serve `service` on an HTTP/2 connection until it ends, `closed` signals, the listener stops, or a limit is hit.
    async fn serve_h2<S>(
        &self,
        stream: TlsStream<TcpStream>,
        service: S,
        closed: Option<watch::Receiver<bool>>,
        limits: Limits,
    ) where
        S: tower::Service<
                http::Request<hyper::body::Incoming>,
                Response = http::Response<tonic::body::Body>,
                Error = std::convert::Infallible,
            > + Clone
            + Send
            + 'static,
        S::Future: Send + 'static,
    {
        let mut builder = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new());
        builder
            .timer(hyper_util::rt::TokioTimer::new())
            .keep_alive_interval(self.config.keepalive)
            .keep_alive_timeout(self.config.keepalive);
        if let Some(streams) = limits.streams {
            builder.max_concurrent_streams(streams);
        }
        let connection = builder.serve_connection(
            hyper_util::rt::TokioIo::new(stream),
            hyper_util::service::TowerToHyperService::new(service),
        );
        let mut shutdown = self.shutdown.clone();
        let closing = async move {
            match closed {
                Some(mut closed) => {
                    let _ = closed.changed().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        let expired = async move {
            match limits.lifetime {
                Some(lifetime) => tokio::time::sleep(lifetime).await,
                None => std::future::pending::<()>().await,
            }
        };
        let silent = async move {
            match limits.hello {
                Some((deadline, helloed)) => {
                    tokio::time::sleep(deadline).await;
                    if helloed.load(Ordering::Acquire) {
                        std::future::pending::<()>().await;
                    }
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            served = connection => {
                if let Err(err) = served {
                    tracing::debug!("edge link connection ended: {err}");
                }
            }
            () = closing => {}
            () = expired => tracing::debug!("edge link: a pairing connection reached its time limit"),
            () = silent => tracing::debug!("edge link: an up connection sent no Hello in time"),
            _ = shutdown.wait_for(|stopped| *stopped) => {}
        }
    }

    async fn serve_down(self: Arc<Self>, mut stream: TlsStream<TcpStream>, remote: SocketAddr, key: Option<String>) {
        let mut raw = [0_u8; 8 + super::DOWN_TOKEN];
        let read = tokio::time::timeout(self.config.handshake_timeout, stream.read_exact(&mut raw)).await;
        if !matches!(read, Ok(Ok(_))) {
            return;
        }
        let (generation, token) = raw.split_at(8);
        let generation = u64::from_be_bytes(generation.try_into().unwrap_or_default());
        // the pairing's one counter numbers every member's connections, so a key and a generation name one member; the
        // token from its Hello answer proves it is that member's (plans/Edge-Node2.md §4.11), and a down connection
        // already open and alive is not replaced
        let found = key.as_deref().and_then(|fingerprint| {
            let registry = lock(&self.registry);
            registry.pools.iter().find_map(|(node, pool)| {
                pool.members
                    .iter()
                    .find(|(_, link)| {
                        link.fingerprint == fingerprint
                            && link.generation == generation
                            && same_token(&link.down_token, token)
                            && link.down.as_ref().is_none_or(|down| down.send.is_closed())
                    })
                    .map(|(member, _)| (node.clone(), member.clone()))
            })
        });
        let Some((node, member)) = found else {
            let message = format!("no up connection of generation {generation} for this node's key");
            let _ = write_answer(&mut stream, Answer::Stale, &message).await;
            return;
        };
        if write_answer(&mut stream, Answer::Ok, "").await.is_err() {
            return;
        }
        // forwarded requests and tunnels share it, so the windows follow the link's bandwidth and delay
        let handshake = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
            .timer(hyper_util::rt::TokioTimer::new())
            .keep_alive_interval(self.config.keepalive)
            .keep_alive_timeout(self.config.keepalive)
            .keep_alive_while_idle(true)
            .adaptive_window(true)
            .handshake(hyper_util::rt::TokioIo::new(stream))
            .await;
        let (send, connection) = match handshake {
            Ok(pair) => pair,
            Err(err) => {
                tracing::debug!("Edge link of {node}: the down connection's HTTP/2 failed: {err}");
                return;
            }
        };
        let (close, mut closed) = watch::channel(false);
        let mut shutdown = self.shutdown.clone();
        let driver = tokio::spawn(async move {
            tokio::select! {
                ended = connection => {
                    if let Err(err) = ended {
                        tracing::debug!("edge link down connection ended: {err}");
                    }
                }
                _ = closed.changed() => {}
                _ = shutdown.wait_for(|stopped| *stopped) => {}
            }
        });
        let mut client = super::local_link_client(send.clone());
        let request = LocalHelloRequest {
            hello: Some(self.hello()),
            generation,
        };
        let local = match tokio::time::timeout(self.config.handshake_timeout, client.hello(request)).await {
            Ok(Ok(reply)) => reply.into_inner().hello.unwrap_or_default(),
            Ok(Err(status)) => {
                tracing::info!(
                    "Edge link of {node}: the local node refused the down Hello: {}",
                    status.message()
                );
                driver.abort();
                return;
            }
            Err(_) => {
                tracing::info!("Edge link of {node}: the local node did not answer the down Hello");
                driver.abort();
                return;
            }
        };
        let attached = {
            let mut registry = lock(&self.registry);
            match registry
                .link_mut(&node, &member)
                .filter(|link| link.generation == generation)
            {
                Some(link) => {
                    link.down = Some(Down {
                        send: send.clone(),
                        _close: close,
                    });
                    if link.hello.is_none() {
                        link.hello = Some(local);
                    }
                    true
                }
                None => false,
            }
        };
        if !attached {
            driver.abort();
            return;
        }
        tracing::info!("Edge link of {node} member {member} is up (generation {generation}, from {remote})");
        // the messages of the node's calls whose media is here go down while this connection lasts (E7), the replies for
        // a leg to the member whose socket bound it, by its own route (plans/Edge-Node2.md §4.4)
        let (route, signals) = super::signal::member_route(&node, &member, generation);
        let signal_client = send.clone();
        let signaling = tokio::spawn(super::signal::forward(signals, move |messages| {
            let mut client = super::local_link_client(signal_client.clone());
            let call: super::signal::Call = Box::pin(async move { client.signal(messages).await.map(drop) });
            call
        }));
        // the node's queued items go down while this connection lasts (plans/archive/Edge-Node.md §5.2), each member's
        // while it is live: its delivery pauses while it drains, starts or stops (plans/Edge-Node2.md §4.4), so it
        // claims nothing then. Items are claimed by lease, and receipts in the pool's database apply each once
        let states = lock(&self.registry)
            .link(&node, &member)
            .map(|link| link.state_watch.subscribe());
        let delivery = tokio::spawn({
            let factory = Weak::clone(&self.factory);
            let (node, owner) = (
                node.clone(),
                format!("{}/{node}/{member}/{generation}", self.config.instance_id),
            );
            let telemetry = Arc::clone(&self.telemetry);
            async move {
                let Some(mut states) = states else { return };
                loop {
                    if states.wait_for(|state| state.takes_work()).await.is_err() {
                        return;
                    }
                    let run = super::queue::deliver(
                        Weak::clone(&factory),
                        node.clone(),
                        moqui_core::edge::queue::TO_LOCAL,
                        owner.clone(),
                        Some(Arc::clone(&telemetry)),
                        deliver_down(send.clone(), generation),
                    );
                    let ended = tokio::select! {
                        () = run => true,
                        paused = states.wait_for(|state| !state.takes_work()) => paused.is_err(),
                    };
                    if ended {
                        return;
                    }
                }
            }
        });
        let _ = driver.await;
        delivery.abort();
        signaling.abort();
        drop(route);
        let mut registry = lock(&self.registry);
        if let Some(link) = registry
            .link_mut(&node, &member)
            .filter(|link| link.generation == generation)
        {
            link.down = None;
        }
    }

    /// The link of the only active pairing, a member chosen by `affinity`: what a delegated call with no owner goes
    /// down, and a PASS authority call for a site no pairing names. Counted by the pairings, not the links up, so with
    /// one pairing away such a call is [`OFFLINE`] rather than sent to another (plans/Edge-Node2.md §4.1). [`OFFLINE`]
    /// at once while no node is paired or that node's link is down, and refused while several are paired: such a call
    /// needs its owner.
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) fn only_pairing_link(&self, affinity: Affinity<'_>) -> Result<Picked, Error> {
        match self.hosts.active().as_slice() {
            [only] => self.link_of(only, affinity),
            [] => Err(Error::new(format!(
                "{}: no local node is paired",
                moqui_core::edge::OFFLINE
            ))),
            _ => Err(Error::new(
                "several local nodes are paired; a call down the link needs its owner, the node of the item it is for \
                 (plans/Edge-Node2.md §4.1)",
            )),
        }
    }

    /// The link a PASS authority call for site `site` goes down (plans/Edge-Node2.md §4.1): the pairing whose Configure
    /// names the site (from the host table, which Configure, pairing and revoking load again); with none naming it,
    /// the only pairing's, as before E16 (a pairing that names no sites). Refused when no pairing names the site and
    /// several are paired, or when two name it. Of the pairing's members (§4.4): by the session handle while a sign-in
    /// is in progress (`session`), else the one with the fewest calls in flight.
    ///
    /// # Errors
    ///
    /// [`OFFLINE`] at once while that node's link is down; the refusal.
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) fn pass_link(&self, site: &str, session: &str) -> Result<Picked, Error> {
        let affinity = match session.trim() {
            "" => Affinity::Fewest,
            handle => Affinity::Key(handle),
        };
        let named = match site.trim() {
            "" => None,
            site => self.hosts.site_node(site)?,
        };
        match named {
            Some(node) => self.link_of(&node, affinity),
            None => self.only_pairing_link(affinity).map_err(|err| {
                if err.message().starts_with(moqui_core::edge::OFFLINE) {
                    err
                } else {
                    Error::new(format!(
                        "no paired node names PASS site {site} in its setup, and several are paired, so its authority \
                         cannot be told"
                    ))
                }
            }),
        }
    }

    /// The member of pairing `node` a piece of work goes to (plans/Edge-Node2.md §4.4): of the members linked with
    /// their down connection open and live, the one `affinity` chooses ([`balance::choose`]), passing over those set
    /// aside while another takes work. The work counts in flight until the answer drops. [`OFFLINE`] at once while no
    /// member is linked, or none takes new work.
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) fn link_of(&self, node: &str, affinity: Affinity<'_>) -> Result<Picked, Error> {
        self.pick(node, affinity, None)
            .ok_or_else(|| Error::new(format!("{}: {node}", moqui_core::edge::OFFLINE)))
    }

    /// Another member of pairing `node` than `not`, as [`Self::link_of`] chooses: where a request that failed before
    /// any answer goes once more.
    pub(crate) fn link_other(&self, node: &str, affinity: Affinity<'_>, not: &str) -> Option<Picked> {
        self.pick(node, affinity, Some(not))
    }

    fn pick(&self, node: &str, affinity: Affinity<'_>, not: Option<&str>) -> Option<Picked> {
        let mut registry = lock(&self.registry);
        let pool = registry.pools.get_mut(node)?;
        let mut ready: Vec<(&String, &Linked)> = pool
            .members
            .iter()
            .filter(|(member, link)| {
                Some(member.as_str()) != not
                    && link.state.takes_work()
                    && link.down.as_ref().is_some_and(|down| !down.send.is_closed())
            })
            .collect();
        if ready.is_empty() {
            return None;
        }
        ready.sort_by(|left, right| left.0.cmp(right.0));
        let ready: Vec<(String, u64, hyper::client::conn::http2::SendRequest<tonic::body::Body>)> = ready
            .into_iter()
            .filter_map(|(member, link)| Some((member.clone(), link.generation, link.down.as_ref()?.send.clone())))
            .collect();
        if pool.health.len() >= MAX_HEALTH {
            let members = &pool.members;
            pool.health
                .retain(|member, health| members.contains_key(member) || health.in_flight() > 0);
        }
        let healths: Vec<Arc<Health>> = ready
            .iter()
            .map(|(member, _, _)| Arc::clone(pool.health.entry(member.clone()).or_default()))
            .collect();
        let turn = pool.turn;
        pool.turn = turn.wrapping_add(1);
        drop(registry);
        let candidates: Vec<balance::Candidate<'_>> = ready
            .iter()
            .zip(&healths)
            .map(|((member, _, _), health)| balance::Candidate { member, health })
            .collect();
        let index = balance::choose(&candidates, affinity, turn, Instant::now())?;
        let (member, generation, send) = ready[index].clone();
        Some(Picked {
            node: node.to_string(),
            member,
            generation,
            send,
            flight: Flight::start(Arc::clone(&healths[index]), self.balance.clone()),
        })
    }

    /// A Ping down the down connection of a member of `node` ([`Self::link_of`]): its round trip.
    pub(crate) async fn ping_local(&self, node: &str) -> Result<Duration, String> {
        let picked = self
            .link_of(node, Affinity::Any)
            .map_err(|_| format!("{node} is not linked"))?;
        let mut client = super::local_link_client(picked.send.clone());
        let started = Instant::now();
        let request = PingRequest {
            generation: picked.generation,
            status: None,
        };
        match tokio::time::timeout(self.config.handshake_timeout, client.ping(request)).await {
            Ok(Ok(_)) => Ok(started.elapsed()),
            Ok(Err(status)) => Err(format!("Ping: {}", status.message())),
            Err(_) => Err(format!("{node} did not answer Ping in time")),
        }
    }

    /// A Ping down the down connection of member `member` of pairing `node`, whatever its state: its round trip
    /// (Clients' Ping on a member row, plans/Edge-Node2.md §4.10).
    pub(crate) async fn ping_member(&self, node: &str, member: &str) -> Result<Duration, String> {
        let (send, generation) = {
            let registry = lock(&self.registry);
            let link = registry
                .link(node, member)
                .ok_or_else(|| format!("member {member} of {node} is not linked"))?;
            let down = link
                .down
                .as_ref()
                .filter(|down| !down.send.is_closed())
                .ok_or_else(|| format!("member {member} of {node} has no down connection"))?;
            (down.send.clone(), link.generation)
        };
        let mut client = super::local_link_client(send);
        let started = Instant::now();
        let request = PingRequest {
            generation,
            status: None,
        };
        match tokio::time::timeout(self.config.handshake_timeout, client.ping(request)).await {
            Ok(Ok(_)) => Ok(started.elapsed()),
            Ok(Err(status)) => Err(format!("Ping: {}", status.message())),
            Err(_) => Err(format!("member {member} of {node} did not answer Ping in time")),
        }
    }

    /// Run a delegated call on its node (the call's, else the only pairing's): its answer, or [`OFFLINE`] at once while
    /// that node's link is down.
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) async fn delegate(&self, call: DelegateCall) -> Result<DelegateAnswer, Error> {
        // of the pool, by the call's conversation, so one call's events reach one member (plans/Edge-Node2.md §4.4)
        let conversation = balance::conversation(&call.parameters);
        let affinity = conversation.as_deref().map_or(Affinity::Fewest, Affinity::Key);
        let picked = match call.node.as_deref() {
            Some(node) => self.link_of(node, affinity)?,
            None => self.only_pairing_link(affinity)?,
        };
        let (node, generation) = (picked.node.clone(), picked.generation);
        let mut client = super::local_link_client(picked.send.clone());
        // a conf entry may wait less than the link does (E8: routing a call that rings); the local node is told, and
        // does no work past it
        let wait = call.timeout.map_or(super::queue::ANSWER_TIMEOUT, |wait| {
            wait.min(super::queue::ANSWER_TIMEOUT)
        });
        let request = DelegateRequest {
            generation,
            service: call.service.clone(),
            parameters_json: call.parameters.to_string(),
            caller_json: call.caller.to_string(),
            operation_id: call.operation_id.clone(),
            timeout_ms: call
                .timeout
                .map_or(0, |_| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
        };
        let started = Instant::now();
        let mut attributes = moqui_core::entity::document::Document::default();
        attributes.insert(
            "moqui.artifact.name".to_string(),
            moqui_lib::object::Value::string(&call.service),
        );
        self.count_member("moqui.edgelink.requests", "delegate", &node, &picked.member);
        let answered = tokio::time::timeout(wait, client.delegate(request)).await;
        match &answered {
            Ok(Ok(_)) => self.succeeded(&picked),
            Ok(Err(status)) if !lost(status) => self.succeeded(&picked),
            _ => self.failed(&picked, "delegate"),
        }
        drop(picked);
        self.telemetry.record_duration(
            "moqui.edgelink.delegate.duration",
            started.elapsed().as_secs_f64(),
            attributes.clone(),
        );
        let reply = match answered {
            Ok(Ok(reply)) => reply.into_inner(),
            Ok(Err(status)) => {
                self.telemetry
                    .add_counter("moqui.edgelink.delegate.errors", 1, attributes);
                tracing::warn!(
                    "Edge link of {node}: delegated call of {} (operation {}) failed: {}",
                    call.service,
                    call.operation_id,
                    status.message()
                );
                return Err(Error::new(format!(
                    "{}: {}",
                    moqui_core::edge::OFFLINE,
                    status.message()
                )));
            }
            Err(_) => {
                self.telemetry
                    .add_counter("moqui.edgelink.delegate.errors", 1, attributes);
                // named as offline too, so a caller with a fallback (the SIP engine, E8) takes it as the node away
                return Err(Error::new(format!(
                    "{}: the local node did not answer the call of {} in time",
                    moqui_core::edge::OFFLINE,
                    call.service
                )));
            }
        };
        if !reply.errors.is_empty() {
            self.telemetry
                .add_counter("moqui.edgelink.delegate.errors", 1, attributes);
        }
        let result = if reply.result_json.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&reply.result_json)
                .map_err(|err| Error::new(format!("the local node's answer is not JSON: {err}")))?
        };
        Ok(DelegateAnswer {
            result,
            errors: reply.errors,
        })
    }

    /// The paired nodes with their live links: linked while any member is, the newest member's link.
    pub(crate) fn clients(&self) -> Result<Vec<EdgeClientInfo>, Error> {
        let rows = store::list_clients(&*self.factory()?)?;
        let registry = lock(&self.registry);
        Ok(rows
            .into_iter()
            .map(|mut client| {
                let newest = registry
                    .pools
                    .get(&client.paired_node_id)
                    .and_then(|pool| pool.members.values().max_by_key(|link| link.generation));
                if let Some(link) = newest {
                    let pool = &registry.pools[&client.paired_node_id];
                    client.linked = true;
                    client.down = pool.members.values().any(|link| link.down.is_some());
                    client.generation = link.generation;
                    client.remote_address = link.remote.to_string();
                    client.members_linked = u32::try_from(pool.members.len()).unwrap_or(u32::MAX);
                    if let Some(hello) = &link.hello {
                        client.binary_version.clone_from(&hello.binary_version);
                        client.link_version = hello.link_version;
                    }
                }
                client
            })
            .collect())
    }

    /// Revoke `node`'s key and close its link.
    pub(crate) fn revoke(&self, node: &str) -> Result<bool, Error> {
        let factory = self.factory()?;
        let revoked = store::revoke_client(&factory, node)?;
        self.revoked(node);
        // the other members of an edge cluster close the links they hold too (plans/Edge-Node2.md §4.6)
        tell_links(&factory, LinkOp::Revoked { node: node.to_string() });
        // its setup no longer names sites or bridges, and the only-pairing rule counts it out
        if let Err(err) = self.hosts.changed(&factory) {
            tracing::error!("Edge node: the host table was not loaded again after revoking {node}: {err}");
        }
        Ok(revoked)
    }

    pub(crate) fn create_pairing_code(&self, valid_minutes: u32) -> Result<PairingCode, Error> {
        store::create_code(&*self.factory()?, valid_minutes)
    }

    /// Tell the administrators that `to` took pairing `node` over from `from`, or is held for their approval
    /// (plans/Edge-Node2.md §4.5); a failure is logged.
    async fn tell_takeover(&self, node: &str, from: &str, to: &str, remote: &str, held: bool) {
        if !self.notice_allowed(node) {
            tracing::warn!(
                "Edge node: pairing {node} has sent {NOTICES_PER_WINDOW} takeover notices within the hour; the one for \
                 {to} is only logged"
            );
            return;
        }
        let (node, from, to, remote) = (node.to_string(), from.to_string(), to.to_string(), remote.to_string());
        let told = self
            .on_pool(move |_| pool::notify_takeover(&node, &from, &to, &remote, held))
            .await;
        if let Err(err) = told {
            tracing::error!("Edge node: the administrators were not told of a takeover: {err}");
        }
    }
}

/// Whether a call's status says its member was not reached or did not answer, rather than answering with a refusal:
/// what counts toward setting a member aside.
pub(crate) fn lost(status: &Status) -> bool {
    matches!(
        status.code(),
        tonic::Code::Unavailable | tonic::Code::Cancelled | tonic::Code::Unknown | tonic::Code::DeadlineExceeded
    )
}

/// Whether `token` is `expected`, in time that does not depend on where they differ.
fn same_token(expected: &[u8], token: &[u8]) -> bool {
    expected.len() == token.len()
        && expected
            .iter()
            .zip(token)
            .fold(0_u8, |differ, (left, right)| differ | (left ^ right))
            == 0
}

/// The edge node's [`Link`] service for one connection, with the link's message limits.
fn link_server(service: LinkService) -> LinkServer<LinkService> {
    LinkServer::new(service)
        .max_decoding_message_size(super::MAX_MESSAGE)
        .max_encoding_message_size(super::MAX_MESSAGE)
}

/// The answer to one delivery down a node's down connection.
type DeliveryAnswer =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<moqui_core::edge::queue::Applied, String>> + Send>>;

/// Send a queued item, with its content first, down a member's down connection of `generation`.
fn deliver_down(
    send: hyper::client::conn::http2::SendRequest<tonic::body::Body>,
    generation: u64,
) -> impl FnMut(moqui_core::edge::queue::QueueItem, Option<Vec<u8>>) -> DeliveryAnswer + Send {
    move |mut item, content| {
        let mut client = super::local_link_client(send.clone());
        Box::pin(async move {
            item.content_location = match content {
                Some(content) => {
                    let chunks = super::queue::content_chunks(generation, &item.queue_item_id, &content);
                    match client.put_content(chunks).await {
                        Ok(reply) => reply.into_inner().content_location,
                        Err(status) => return super::queue::delivery_error(&status),
                    }
                }
                None => String::new(),
            };
            let request = DeliverRequest {
                generation,
                item: Some(super::queue::to_proto(&item)),
            };
            match client.deliver(request).await {
                Ok(reply) => Ok(super::queue::applied(&reply.into_inner())),
                Err(status) => super::queue::delivery_error(&status),
            }
        })
    }
}

/// The limits of one link connection.
struct Limits {
    /// The longest it stays open.
    lifetime: Option<Duration>,
    /// HTTP/2 streams at once.
    streams: Option<u32>,
    /// Closed when no Hello came in this time.
    hello: Option<(Duration, Arc<AtomicBool>)>,
}

/// What an up or pairing connection may do.
enum Mode {
    /// A check connection (no key: Probe only) or a pairing connection (the new key).
    Pairing { key: Option<String> },
    /// An up connection of pairing `node`; its member is named by its Hello, which registers it once admitted.
    Up {
        node: String,
        generation: u64,
        fingerprint: String,
        /// The TLS exporter value the member's Hello proves its own key over.
        binding: [u8; 32],
        member: Arc<std::sync::OnceLock<String>>,
        /// Closes the connection when dropped: moved into the registry by the Hello that admits the member.
        close: Mutex<Option<watch::Sender<bool>>>,
    },
}

/// The edge node's [`Link`] for one connection.
struct LinkService {
    server: Arc<EdgeServer>,
    mode: Mode,
    remote: SocketAddr,
    helloed: Arc<AtomicBool>,
    calls: AtomicU32,
}

impl LinkService {
    /// The pairing and generation of an up connection whose member is admitted and still current.
    fn up(&self) -> Result<(&str, u64), Status> {
        let (node, member, generation) = self.up_member()?;
        let _ = member;
        Ok((node, generation))
    }

    /// The pairing, member and generation of an up connection whose member is admitted and still current.
    fn up_member(&self) -> Result<(&str, &str, u64), Status> {
        let Mode::Up {
            node,
            generation,
            member,
            ..
        } = &self.mode
        else {
            return Err(Status::failed_precondition("not an up connection"));
        };
        let Some(member) = member.get() else {
            return Err(Status::failed_precondition("Hello first"));
        };
        if self.server.current(node, member) != *generation {
            return Err(Status::failed_precondition("stale link generation"));
        }
        Ok((node, member, *generation))
    }

    /// Admit member `member` of pairing `node` (plans/Edge-Node2.md §4.4, §4.5, §4.11) and register its link, naming it
    /// on this connection before anything else is awaited. Its id is its own only with the key pinned for it
    /// (`member_key`, its fingerprint). A member linked already reconnects, unless its link still answers a Ping. With
    /// others linked, they are asked for their cluster's members now. The node a takeover displaced is refused until
    /// its operator clears it. With none linked the key admits it, and a new node taking the pairing over is noted, told
    /// and logged, or held. The answer is the down connection's token.
    async fn admit(
        &self,
        node: &str,
        member: &str,
        member_key: &str,
        local: &Hello,
        state: MemberState,
    ) -> Result<[u8; super::DOWN_TOKEN], Status> {
        let Mode::Up {
            generation,
            fingerprint,
            close,
            member: named,
            ..
        } = &self.mode
        else {
            return Err(Status::failed_precondition("not an up connection"));
        };
        let generation = *generation;
        let key = node.to_string();
        let row = self
            .server
            .on_pool(move |factory| store::member_settings(factory, &key))
            .await
            .map_err(|err| {
                tracing::error!("Edge link of {node}: its pairing was not read for admission: {err}");
                Status::internal("the edge node could not take the link")
            })?;
        let pinned = pool::key_matches(&row.pins, member, member_key);
        let (linked, known) = {
            let registry = lock(&self.server.registry);
            let pool = registry.pools.get(node);
            let linked: Vec<String> = pool
                .map(|pool| pool.members.keys().cloned().collect())
                .unwrap_or_default();
            let known = pool.and_then(|pool| pool.cluster.clone()).unwrap_or_default();
            (linked, known)
        };
        let reconnect = linked.iter().any(|linked| linked == member);
        // in an edge cluster a member linked on another edge member is linked too (plans/Edge-Node2.md §4.6, §4.11):
        // its link here is another of its links, not a takeover, and a node with only the key is refused while one is.
        // Read at each admission, also on a member that does not know its peers yet (just started)
        let elsewhere = self.server.linked_elsewhere(node).await.map_err(|err| {
            tracing::error!("Edge link of {node}: the links on the other edge members were not read: {err}");
            Status::internal("the edge node could not take the link")
        })?;
        // an id linked only elsewhere is that member's only with the key pinned for it: one with another key, or one
        // whose pin was forgotten, is refused while that member is linked (forgetting its key closes its links)
        let impostor = !reconnect && elsewhere.members.iter().any(|other| other == member) && pinned != Some(true);
        let mut linked_any = linked.clone();
        for other in &elsewhere.members {
            if !linked_any.contains(other) {
                linked_any.push(other.clone());
            }
        }
        // a linked id with no key pinned yet is a first link racing another of the same id: the older one is given
        // [`YOUNG_LINK`] to pin its key before a link of that id can replace it (plans/Edge-Node2.md §4.11)
        if reconnect && pinned.is_none() && self.server.link_age(node, member).is_some_and(|age| age < YOUNG_LINK) {
            return Err(Status::unavailable(
                "this member's first link is being taken: a second link of the same member is not taken yet",
            ));
        }
        if reconnect && pinned != Some(false) && self.server.still_answers(node, member).await {
            return Err(Status::unavailable(
                "this member's link is up and answers: a second link of the same member is not taken",
            ));
        }
        let listed = if !linked_any.iter().any(|linked| linked == member) && !linked_any.is_empty() {
            // asked now, else the latest list a linked member sent with its Ping, while it is fresh
            match self.server.ask_members(node, member).await {
                Some(list) => Some(list),
                None => self.server.fresh_list(node),
            }
        } else {
            None
        };
        let decided = if impostor {
            Admission::Refuse(pool::JOIN_FIRST.to_string())
        } else {
            pool::admission(&pool::AdmissionFacts {
                member,
                linked: &linked_any,
                listed: listed.as_deref(),
                known: &known,
                last_member: &row.last_member,
                hold: row.hold,
                approved: &row.approved,
                pinned,
                displaced: &row.displaced,
                forgotten: pool::forgotten(&row.pins, member),
            })
        };
        // a live member of the edge cluster whose links are not known yet (it has written no row since another member
        // came up): no node takes the pairing over until they are, so it cannot displace a member linked there
        if elsewhere.unknown && matches!(decided, Admission::TakeOver { .. } | Admission::Hold { .. }) {
            tracing::info!(
                "Edge link of {node}: member {member} would take the pairing over, but another edge member's links are \
                 not known yet; it dials again"
            );
            return Err(Status::unavailable(
                "the links on the other edge members are not known yet; dial again",
            ));
        }
        let remote = self.remote.to_string();
        let refuse = |message: String, held: bool| {
            let mut registry = lock(&self.server.registry);
            registry
                .pools
                .entry(node.to_string())
                .or_default()
                .note_refused(RefusedMember {
                    instance_id: member.to_string(),
                    remote_address: remote.clone(),
                    at: moqui_core::user::now_millis(),
                    message: message.clone(),
                    held,
                });
            Status::permission_denied(message)
        };
        let takeover = match decided {
            Admission::Admit => None,
            Admission::TakeOver { from } => Some(from),
            Admission::Refuse(message) => {
                tracing::warn!("Edge link of {node}: member {member} from {remote} refused: {message}");
                return Err(refuse(message, false));
            }
            Admission::Hold { from } => {
                let first = !lock(&self.server.registry)
                    .pools
                    .get(node)
                    .is_some_and(|pool| pool.refused.iter().any(|old| old.instance_id == member && old.held));
                tracing::warn!(
                    "Edge link of {node}: new node {member} from {remote} would take over from {from}; held for \
                     approval"
                );
                let refused = refuse(pool::HELD_FOR_APPROVAL.to_string(), true);
                if first {
                    self.server.tell_takeover(node, &from, member, &remote, true).await;
                }
                return Err(refused);
            }
        };
        // the member's key, claimed for its id in one transaction before its link is registered: pinned when none is,
        // kept when it matches, pinned again on a decided takeover; else refused. The ids linked, the last and the kept
        // list's are never evicted past the cap
        let mut keep: Vec<String> = linked_any.clone();
        keep.push(row.last_member.clone());
        keep.extend(known.iter().map(|row| row.instance_id.clone()));
        // in this edge member's row from the claim's own write until it registers or is refused (the guard)
        let _admitting = Admitting::start(&self.server, node, member);
        let claim = {
            let (key, id, fingerprint, keep) = (node.to_string(), member.to_string(), member_key.to_string(), keep);
            let wanted_takeover = takeover.is_some();
            let seen = elsewhere.members.clone();
            let server = Arc::clone(&self.server);
            self.server
                .on_pool(move |factory| {
                    let recheck = || -> Result<Option<bool>, Error> {
                        if !wanted_takeover {
                            return Ok(Some(false));
                        }
                        // with the pairing's row locked: another edge member that admitted a link of this pairing
                        // since has written its row in the same lock
                        let now = server.read_elsewhere(factory, &key)?;
                        if now.members.iter().any(|other| *other == id) {
                            return Ok(Some(true));
                        }
                        Ok((!now.members.iter().any(|other| !seen.contains(other))).then_some(false))
                    };
                    let write_own = || server.write_links(factory);
                    store::claim_key(
                        factory,
                        &key,
                        &id,
                        &fingerprint,
                        wanted_takeover,
                        &keep,
                        recheck,
                        write_own,
                    )
                })
                .await
        };
        let takeover = match claim {
            Ok(store::Claimed::Pinned { takeover: true }) => takeover,
            Ok(store::Claimed::Pinned { takeover: false }) => None,
            Ok(store::Claimed::Refused) => {
                tracing::warn!("Edge link of {node}: member {member} from {remote} refused: another key is pinned");
                return Err(refuse(pool::JOIN_FIRST.to_string(), false));
            }
            Ok(store::Claimed::Changed) => {
                return Err(Status::unavailable(
                    "another member of this pairing linked to another edge member meanwhile; dial again",
                ));
            }
            Err(err) => {
                tracing::error!("Edge link of {node}: member {member}'s key was not pinned: {err}");
                return Err(Status::internal("the edge node could not take the link"));
            }
        };
        let Some(close) = lock(close).take() else {
            return Err(Status::failed_precondition("Hello was said already on this connection"));
        };
        let registration = Registration {
            generation,
            fingerprint: fingerprint.clone(),
            remote: self.remote,
            close,
            hello: local.clone(),
            state,
            linked,
            member_key: member_key.to_string(),
        };
        let token = match self.server.register(node, member, registration) {
            Ok(token) => token,
            Err(Unregistered::Revoked) => {
                return Err(Status::permission_denied(
                    "this node's key is not paired with this edge node, or was revoked; pair again",
                ))
            }
            Err(Unregistered::Stale(live)) => {
                return Err(Status::failed_precondition(format!(
                    "generation {generation} is older than the live generation {live}"
                )))
            }
            Err(Unregistered::Changed) => {
                return Err(Status::unavailable(
                    "the pairing's links changed while this one was admitted; dial again",
                ))
            }
        };
        // named now, before anything else is awaited: a call dropped from here on still leaves no link behind
        let _ = named.set(member.to_string());
        // the serial writer keeps this edge member's row from now on (plans/Edge-Node2.md §4.6)
        self.server.member_links.notify_one();
        if let Some(from) = takeover {
            tracing::warn!("Edge link of {node}: new node {member} from {remote} takes the pairing over from {from}");
            let server = Arc::clone(&self.server);
            let (key, to) = (node.to_string(), member.to_string());
            tokio::spawn(async move {
                let (noted_key, noted_to, noted_from) = (key.clone(), to.clone(), from.clone());
                let noted = server
                    .on_pool(move |factory| store::note_takeover(factory, &noted_key, &noted_from, &noted_to))
                    .await;
                if let Err(err) = noted {
                    tracing::error!("Edge link of {key}: the takeover by {to} was not noted on its row: {err}");
                }
                server.tell_takeover(&key, &from, &to, &remote, false).await;
            });
        }
        Ok(token)
    }

    /// The key of a check (`None`) or pairing connection, counting the call against the connection's limit.
    fn pairing(&self) -> Result<Option<&str>, Status> {
        let Mode::Pairing { key } = &self.mode else {
            return Err(Status::failed_precondition("not a pairing connection"));
        };
        if self.calls.fetch_add(1, Ordering::AcqRel) >= self.server.config.pairing_calls {
            return Err(Status::resource_exhausted("too many calls on one pairing connection"));
        }
        Ok(key.as_deref())
    }
}

#[tonic::async_trait]
impl Link for LinkService {
    async fn probe(&self, _request: Request<ProbeRequest>) -> Result<Response<ProbeReply>, Status> {
        self.pairing()?;
        Ok(Response::new(ProbeReply {
            hello: Some(self.server.hello()),
            link_fingerprint: self.server.link_fingerprint.clone(),
        }))
    }

    async fn pair(&self, request: Request<PairRequest>) -> Result<Response<PairReply>, Status> {
        let fingerprint = self
            .pairing()?
            .ok_or_else(|| Status::permission_denied("pairing needs the local node's client certificate"))?
            .to_string();
        let remote = self.remote;
        if !self.server.pair_allowed(remote.ip()) {
            tracing::warn!("Edge pairing from {remote} refused: too many attempts");
            return Err(Status::resource_exhausted(
                "too many pairing attempts; wait and try again",
            ));
        }
        let request = request.into_inner();
        let local = request.hello.unwrap_or_default();
        check_versions(LINK_VERSION, local.link_version, "local node").map_err(Status::failed_precondition)?;
        let node = local.instance_id.clone();
        if !valid_node_id(&node) {
            return Err(Status::invalid_argument(
                "the local node's instance id is empty, too long, or has spaces or control characters",
            ));
        }
        let code = request.code;
        let key = fingerprint.clone();
        let pairing_node = node.clone();
        let paired = self
            .server
            .on_pool({
                let server = Arc::clone(&self.server);
                move |factory| {
                    let paired = store::pair_client(factory, &code, &pairing_node, &key, &local)?;
                    // a new pairing counts for the only-pairing rule at once
                    if let Err(err) = server.hosts.changed(factory) {
                        tracing::error!("Edge node: the host table was not loaded again after a pairing: {err}");
                    }
                    Ok(paired)
                }
            })
            .await
            .map_err(|err| {
                tracing::error!("Edge pairing from {remote}: {err}");
                Status::internal("the edge node could not pair")
            })?;
        match paired {
            Paired::Pinned { key_changed } => {
                lock(&self.server.registry).revoked.remove(&node);
                if key_changed {
                    // the links of the old key end now that its row has the new one
                    self.server.close(&node);
                }
                if let Ok(factory) = self.server.factory() {
                    tell_links(
                        &factory,
                        LinkOp::Paired {
                            node: node.clone(),
                            key_changed,
                        },
                    );
                }
                tracing::info!("Edge node: paired local node {node} (key {fingerprint}) from {remote}");
                Ok(Response::new(PairReply {
                    hello: Some(self.server.hello()),
                    key_fingerprint: fingerprint,
                    link_fingerprint: self.server.link_fingerprint.clone(),
                }))
            }
            Paired::Refused(refused) => {
                tracing::info!("Edge pairing from {remote} refused: {refused}");
                Err(Status::permission_denied(refused))
            }
        }
    }

    async fn hello(&self, request: Request<HelloRequest>) -> Result<Response<HelloReply>, Status> {
        let Mode::Up {
            node,
            generation,
            member: named,
            ..
        } = &self.mode
        else {
            return Err(Status::failed_precondition("not an up connection"));
        };
        let (node, generation) = (node.as_str(), *generation);
        let request = request.into_inner();
        let local = request.hello.unwrap_or_default();
        check_versions(LINK_VERSION, local.link_version, "local node").map_err(Status::failed_precondition)?;
        // the member is its own instance id, the pairing the key's (plans/Edge-Node2.md §4.4)
        let member = local.instance_id.clone();
        if !pool::valid_member_id(&member) {
            return Err(Status::invalid_argument(
                "the local node's instance id is empty, too long, or has spaces, control characters or @",
            ));
        }
        if named.get().is_some() {
            return Err(Status::failed_precondition("Hello was said already on this connection"));
        }
        // the member's own key, proved over this connection (plans/Edge-Node2.md §4.11)
        let Mode::Up { binding, .. } = &self.mode else {
            return Err(Status::failed_precondition("not an up connection"));
        };
        if !super::member_key::verify(&request.member_key, binding, &request.member_proof) {
            tracing::warn!(
                "Edge link of {node}: member {member} from {} gave no proof of its own key",
                self.remote
            );
            return Err(Status::permission_denied(
                "this member does not prove a key of its own (plans/Edge-Node2.md §4.11)",
            ));
        }
        let member_key = super::member_key::fingerprint(&request.member_key);
        let state = request
            .status
            .as_ref()
            .and_then(|status| MemberState::parse(&status.state))
            .unwrap_or_default();
        let down_token = self.admit(node, &member, &member_key, &local, state).await?;
        self.server
            .note_status(node, &member, generation, request.status.as_ref());
        let key = node.to_string();
        let saved_member = member.clone();
        let saved = self
            .server
            .on_pool(move |factory| store::save_hello(factory, &key, &saved_member, &local))
            .await;
        if let Err(err) = saved {
            tracing::warn!("Edge link of {node}: its Hello was not saved: {err}");
        }
        self.helloed.store(true, Ordering::Release);
        let mut hello = self.server.hello();
        // E16: the realm of this pairing's phones, from its last Configure, and what its TURN secret is derived from
        let realm_node = node.to_string();
        let read = self
            .server
            .on_pool(move |factory| {
                let realm = moqui_core::edge::sip::pairing_realm(factory, &realm_node, false);
                let salt = match factory.entity() {
                    Some(facade) => moqui_core::edge::media::pairing_turn_salt(&facade, &realm_node)?,
                    None => None,
                };
                Ok((realm, salt))
            })
            .await;
        let (realm, salt) = read.unwrap_or_else(|err| {
            tracing::error!("Edge link of {node}: its SIP realm and TURN secret were not read: {err}");
            (None, None)
        });
        if let Some((domain, registrar)) = realm {
            hello.sip_domain = domain;
            hello.sip_registrar = registrar;
        }
        Ok(Response::new(HelloReply {
            hello: Some(hello),
            generation,
            turn_secret: salt
                .map(|salt| self.server.turn_secret(node, &salt))
                .unwrap_or_default(),
            pairing_id: node.to_string(),
            down_token: down_token.to_vec(),
            // the edge cluster's live members, each of which the local node dials (plans/Edge-Node2.md §4.6)
            edge_members: self.server.edge_members().await,
        }))
    }

    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let (node, member, generation) = self.up_member()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        // the member's state and its cluster's members, every Ping (plans/Edge-Node2.md §4.4)
        self.server
            .note_status(node, member, generation, request.into_inner().status.as_ref());
        Ok(Response::new(PingReply {
            generation,
            // so a member that joins or leaves the edge cluster is seen within a Ping (plans/Edge-Node2.md §4.6)
            edge_members: self.server.edge_members().await,
        }))
    }

    async fn deliver(&self, request: Request<DeliverRequest>) -> Result<Response<DeliverReply>, Status> {
        let (node, _) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        let item = super::queue::from_proto(request.into_inner().item.unwrap_or_default());
        let applied = super::queue::receive(&self.server.factory, node, item).await;
        Ok(Response::new(super::queue::reply(&applied)))
    }

    async fn put_content(
        &self,
        request: Request<tonic::Streaming<ContentChunk>>,
    ) -> Result<Response<PutContentReply>, Status> {
        let (node, _) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        // the up connection is the generation; the chunks' own is not checked again
        let reply = super::queue::receive_content(&self.server.factory, node, request.into_inner(), |_| Ok(())).await?;
        Ok(Response::new(reply))
    }

    async fn signal(&self, request: Request<tonic::Streaming<SignalMessage>>) -> Result<Response<SignalReply>, Status> {
        let (node, member, _) = self.up_member()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        // the up connection is the generation; each message's own is not checked again. Only what this node may send
        // is taken, its frames' replies addressed to itself (E7), each stamped with the node (E12)
        let take = super::signal::edge_takes(node, member);
        let reply = super::signal::receive(request.into_inner(), |_| Ok(()), take).await?;
        Ok(Response::new(reply))
    }

    async fn replicate(&self, request: Request<ReplicateRequest>) -> Result<Response<ReplicateReply>, Status> {
        let (node, _) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        let batch = super::replicate::batch_of(request.into_inner());
        let set = batch.set_name.clone();
        let key = node.to_string();
        let answer = self
            .server
            .on_pool(move |factory| {
                let facade = store::facade(factory)?;
                moqui_core::edge::replica::apply_batch(&facade, &key, &batch)
            })
            .await
            .map_err(|err| {
                tracing::error!("Edge link of {node}: a batch of set {set} was not applied: {err}");
                Status::internal("the edge node could not apply the batch")
            })?;
        Ok(Response::new(ReplicateReply {
            resync: answer == moqui_core::edge::replica::BatchAnswer::Resync,
        }))
    }

    async fn get_offer(&self, _request: Request<OfferRequest>) -> Result<Response<Offer>, Status> {
        let (node, _) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        let server = Arc::clone(&self.server);
        let key = node.to_string();
        let offer = self
            .server
            .on_pool(move |factory| super::offer::offer(factory, &server.config, &key, &server.linked_nodes()))
            .await
            .map_err(|err| {
                tracing::error!("Edge link of {node}: the offer was not made: {err}");
                Status::internal("the edge node could not make its offer")
            })?;
        Ok(Response::new(offer))
    }

    async fn configure(&self, request: Request<ConfigureRequest>) -> Result<Response<ConfigureReply>, Status> {
        let (node, _) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        let request = request.into_inner();
        // refused whole before any change when it is not input (moqui_core::edge::setup::check_setup)
        super::offer::request_check(&request).map_err(|err| Status::invalid_argument(err.message().to_string()))?;
        let server = Arc::clone(&self.server);
        let key = node.to_string();
        let reply = self
            .server
            .on_pool(move |factory| {
                let linked = server.linked_nodes();
                let reply = super::offer::configure(factory, &server.config, &key, request, &linked)?;
                server.hosts.changed(factory)?;
                Ok(reply)
            })
            .await
            .map_err(|err| {
                tracing::error!("Edge link of {node}: its setup was not applied: {err}");
                Status::internal("the edge node could not apply the setup; its log has why")
            })?;
        Ok(Response::new(reply))
    }

    async fn unpair(&self, _request: Request<UnpairRequest>) -> Result<Response<UnpairReply>, Status> {
        let (node, _) = self.up()?;
        let key = node.to_string();
        let server = Arc::clone(&self.server);
        self.server
            .on_pool(move |factory| {
                let revoked = store::revoke_client(factory, &key)?;
                if let Err(err) = server.hosts.changed(factory) {
                    tracing::error!("Edge node: the host table was not loaded again after {key} unpaired: {err}");
                }
                Ok(revoked)
            })
            .await
            .map_err(|err| {
                tracing::error!("Edge link of {node}: Unpair failed: {err}");
                Status::internal("the edge node could not unpair")
            })?;
        tracing::info!("Edge node: local node {node} unpaired itself");
        lock(&self.server.registry).revoked.insert(node.to_string());
        if let Ok(factory) = self.server.factory() {
            tell_links(&factory, LinkOp::Revoked { node: node.to_string() });
        }
        // the answer goes out first; then the link closes
        let server = Arc::clone(&self.server);
        let key = node.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            server.close(&key);
        });
        Ok(Response::new(UnpairReply {}))
    }
}

/// A link being admitted, in [`EdgeServer::admitting`] while it lives: dropped once the link registered or was refused,
/// when the serial writer is woken to write the edge member's row without it if it did not register.
struct Admitting {
    server: Arc<EdgeServer>,
    key: (String, String),
}

impl Admitting {
    fn start(server: &Arc<EdgeServer>, node: &str, member: &str) -> Self {
        let key = (node.to_string(), member.to_string());
        lock(&server.admitting).insert(key.clone());
        Self {
            server: Arc::clone(server),
            key,
        }
    }
}

impl Drop for Admitting {
    fn drop(&mut self) {
        lock(&self.server.admitting).remove(&self.key);
        self.server.member_links.notify_one();
    }
}

/// Tell the other members of an edge cluster what was done here to a pairing's links (plans/Edge-Node2.md §4.6).
pub(crate) fn tell_links(factory: &ExecutionContextFactory, op: LinkOp) {
    if let Ok(facade) = store::facade(factory) {
        moqui_core::edge::cluster_state::tell_links(&facade, &[op]);
    }
}

#[cfg(test)]
#[path = "accept_tests.rs"]
mod tests;
