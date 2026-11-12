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

//! The edge node's end of the link (plans/Edge-Node.md §5.1, §5.7): the TLS hand-off on the HTTPS listener and the
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
//! the bytes read so far go down that node's link ([`super::tunnel`], plans/Edge-Node.md §5.3). The server name of a
//! passthrough host whose node is not linked closes the connection.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::{DelegateAnswer, DelegateCall, EdgeClientInfo, PairingCode};
use moqui_core::telemetry::Telemetry;
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::server::TlsStream;
use tonic::{Request, Response, Status};

use super::hosts::{self, Hosts, Route};
use super::pair::Identity;
use super::proto::link_server::{Link, LinkServer};
use super::proto::{
    ConfigureReply, ConfigureRequest, ContentChunk, DelegateRequest, DeliverReply, DeliverRequest, Hello, HelloReply,
    HelloRequest, LocalHelloRequest, Offer, OfferRequest, PairReply, PairRequest, PingReply, PingRequest, ProbeReply,
    ProbeRequest, PutContentReply, ReplicateReply, ReplicateRequest, SignalMessage, SignalReply, UnpairReply,
    UnpairRequest,
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
        let (link, pairing, server_name) = {
            let client_hello = accepted.client_hello();
            let offered = |wanted: &[u8]| {
                client_hello
                    .alpn()
                    .is_some_and(|mut offered| offered.any(|protocol| protocol == wanted))
            };
            (
                offered(ALPN),
                offered(ALPN_PAIR),
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
        let passthrough = server_name
            .as_deref()
            .and_then(|name| self.server.hosts.route(name))
            .filter(|route| route.mode == hosts::Mode::Passthrough);
        if let Some(Route { node, .. }) = passthrough {
            if self.server.down_sender(&node).is_none() {
                self.server.count("moqui.edgelink.refused", super::TUNNEL_HTTPS);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    format!("a passthrough host of {node}, whose link is down"),
                ));
            }
            return Ok(Secured::Link(Handoff::Passthrough { stream, hello, node }));
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

/// A live link: its generation, its key, and the down connection once it is open. Dropping it closes its connections.
struct Linked {
    generation: u64,
    fingerprint: String,
    remote: SocketAddr,
    /// The up connection closes when this is dropped or sends.
    _close: watch::Sender<bool>,
    hello: Option<Hello>,
    down: Option<Down>,
}

struct Down {
    send: hyper::client::conn::http2::SendRequest<tonic::body::Body>,
    _close: watch::Sender<bool>,
}

/// The live links, and the nodes revoked since this process started: a link whose key was read as active just before
/// a revoke is refused here, so a revoke always leaves no live link.
#[derive(Default)]
struct Registry {
    links: HashMap<String, Linked>,
    revoked: HashSet<String>,
}

/// Why [`EdgeServer::register`] refused.
#[derive(Debug, PartialEq, Eq)]
enum Unregistered {
    Revoked,
    /// A newer generation is already live.
    Stale(u64),
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
}

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
        });
        telemetry.observe_gauge("moqui.edgelink.tunnels.active", move || {
            Some(active.load(Ordering::Acquire) as f64)
        });
        let weak = Arc::downgrade(&server);
        telemetry.observe_gauge("moqui.edgelink.up", move || {
            let server = weak.upgrade()?;
            let linked = lock(&server.registry)
                .links
                .values()
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

    /// The sender of `node`'s down connection, while its link is up.
    pub(crate) fn down_sender(&self, node: &str) -> Option<hyper::client::conn::http2::SendRequest<tonic::body::Body>> {
        let registry = lock(&self.registry);
        let down = registry.links.get(node)?.down.as_ref()?;
        (!down.send.is_closed()).then(|| down.send.clone())
    }

    /// The nodes whose links are up, down connection and all.
    pub(crate) fn linked_nodes(&self) -> Vec<String> {
        lock(&self.registry)
            .links
            .iter()
            .filter(|(_, link)| link.down.is_some())
            .map(|(node, _)| node.clone())
            .collect()
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
        // a media node of its local nodes' calls (plans/Edge-Node.md §5.4, E7): its media address and TURN URLs, and
        // its area (§5.7, E10 calls) as the media socket is now, not as it was when the areas were read at start
        hello
            .capabilities
            .retain(|area| area != super::offer::CALLS_AREA && area != super::offer::SIP_AREA);
        if let Some(advert) = self.media() {
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

    /// What this node's media node advertises, while its media socket is up.
    fn media(&self) -> Option<moqui_core::edge::media::MediaAdvert> {
        let factory = self.factory.upgrade()?;
        moqui_core::edge::media::advert(&factory).filter(|advert| !advert.media_address.is_empty())
    }

    /// The secret this node's embedded TURN checks credentials with, while it runs: for a local node's Hello answer
    /// only, so it signs the credentials of this node's TURN URLs (E7). Empty without TURN.
    fn turn_secret(&self) -> String {
        if self.media().is_none_or(|advert| advert.turn_urls.is_empty()) {
            return String::new();
        }
        let secret = moqui_lib::env::get_prop_or_env("turn_auth_secret");
        let secret = secret.trim();
        if secret.is_empty() || secret.ends_with("CHANGEME") {
            String::new()
        } else {
            secret.to_string()
        }
    }

    /// The generation of `node`'s live link, or 0.
    fn current(&self, node: &str) -> u64 {
        lock(&self.registry).links.get(node).map_or(0, |link| link.generation)
    }

    /// Register a new up connection of `node`, closing the one of an older generation it replaces; the receiver
    /// closes this one. A revoked node, or a generation older than the live one, is refused.
    fn register(
        &self,
        node: &str,
        generation: u64,
        fingerprint: &str,
        remote: SocketAddr,
    ) -> Result<watch::Receiver<bool>, Unregistered> {
        let mut registry = lock(&self.registry);
        if registry.revoked.contains(node) {
            return Err(Unregistered::Revoked);
        }
        if let Some(live) = registry.links.get(node).filter(|live| live.generation >= generation) {
            return Err(Unregistered::Stale(live.generation));
        }
        let (close, closed) = watch::channel(false);
        let replaced = registry.links.insert(
            node.to_string(),
            Linked {
                generation,
                fingerprint: fingerprint.to_string(),
                remote,
                _close: close,
                hello: None,
                down: None,
            },
        );
        drop(registry);
        if let Some(old) = replaced {
            tracing::info!(
                "Edge link of {node}: generation {generation} from {remote} replaces generation {} from {}",
                old.generation,
                old.remote
            );
        }
        Ok(closed)
    }

    /// Forget `node`'s link if it is still of `generation`.
    fn unregister(&self, node: &str, generation: u64) {
        let mut registry = lock(&self.registry);
        if registry
            .links
            .get(node)
            .is_some_and(|link| link.generation == generation)
        {
            registry.links.remove(node);
        }
    }

    /// Close `node`'s link, whatever its generation.
    pub(crate) fn close(&self, node: &str) -> bool {
        let removed = lock(&self.registry).links.remove(node);
        removed.is_some()
    }

    /// Mark `node` revoked and close its link, in one step under the registry's lock.
    fn revoked(&self, node: &str) {
        let removed = {
            let mut registry = lock(&self.registry);
            registry.revoked.insert(node.to_string());
            registry.links.remove(node)
        };
        drop(removed);
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
        let closed = match self.register(&node, generation, &fingerprint, remote) {
            Ok(closed) => closed,
            Err(Unregistered::Revoked) => {
                tracing::info!("Edge link of {node} from {remote} refused: revoked");
                let _ = write_answer(&mut stream, Answer::Refused, NOT_PAIRED).await;
                return;
            }
            Err(Unregistered::Stale(live)) => {
                let message = format!("generation {generation} is older than the live generation {live}");
                let _ = write_answer(&mut stream, Answer::Stale, &message).await;
                return;
            }
        };
        if write_answer(&mut stream, Answer::Ok, "").await.is_err() {
            self.unregister(&node, generation);
            return;
        }
        let helloed = Arc::new(AtomicBool::new(false));
        let service = LinkService {
            server: Arc::clone(&self),
            mode: Mode::Up {
                node: node.clone(),
                generation,
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
        self.unregister(&node, generation);
        tracing::debug!("Edge link of {node} (generation {generation}) closed");
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
        let mut raw = [0_u8; 8];
        let read = tokio::time::timeout(self.config.handshake_timeout, stream.read_exact(&mut raw)).await;
        if !matches!(read, Ok(Ok(_))) {
            return;
        }
        let generation = u64::from_be_bytes(raw);
        let node = key.as_deref().and_then(|fingerprint| {
            lock(&self.registry)
                .links
                .iter()
                .find(|(_, link)| link.fingerprint == fingerprint && link.generation == generation)
                .map(|(node, _)| node.clone())
        });
        let Some(node) = node else {
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
                .links
                .get_mut(&node)
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
        tracing::info!("Edge link of {node} is up (generation {generation}, from {remote})");
        // the messages of the node's calls whose media is here go down while this connection lasts (E7)
        let (route, signals) = super::signal::route(&node, generation);
        let signal_client = send.clone();
        let signaling = tokio::spawn(super::signal::forward(signals, move |messages| {
            let mut client = super::local_link_client(signal_client.clone());
            let call: super::signal::Call = Box::pin(async move { client.signal(messages).await.map(drop) });
            call
        }));
        // the node's queued items go down while this connection lasts (plans/Edge-Node.md §5.2)
        let delivery = tokio::spawn(super::queue::deliver(
            Weak::clone(&self.factory),
            node.clone(),
            moqui_core::edge::queue::TO_LOCAL,
            format!("{}/{node}/{generation}", self.config.instance_id),
            Some(Arc::clone(&self.telemetry)),
            deliver_down(send, generation),
        ));
        let _ = driver.await;
        delivery.abort();
        signaling.abort();
        drop(route);
        let mut registry = lock(&self.registry);
        if let Some(link) = registry
            .links
            .get_mut(&node)
            .filter(|link| link.generation == generation)
        {
            link.down = None;
        }
    }

    /// The only local node whose link is up, with its down connection's sender and the link's generation: what a
    /// delegated call or a PASS authority call goes down. [`OFFLINE`] at once while no link is up, and refused while
    /// several are (plans/Edge-Node.md decision 1).
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) fn only_link(
        &self,
    ) -> Result<(String, hyper::client::conn::http2::SendRequest<tonic::body::Body>, u64), Error> {
        let registry = lock(&self.registry);
        let mut up = registry.links.iter().filter_map(|(node, link)| {
            let down = link.down.as_ref().filter(|down| !down.send.is_closed())?;
            Some((node.clone(), down.send.clone(), link.generation))
        });
        let first = up.next().ok_or_else(|| Error::new(moqui_core::edge::OFFLINE))?;
        if up.next().is_some() {
            return Err(Error::new(
                "several local nodes are linked; a call down the link needs one (plans/Edge-Node.md decision 1)",
            ));
        }
        Ok(first)
    }

    /// The link of `node` while its down connection is up, as [`EdgeServer::only_link`] gives it: what an all-in-one
    /// node's delegated call of a paired node's item goes down (plans/Edge-Node.md §10). [`OFFLINE`] at once while that
    /// node's link is down.
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) fn link_of(
        &self,
        node: &str,
    ) -> Result<(String, hyper::client::conn::http2::SendRequest<tonic::body::Body>, u64), Error> {
        let registry = lock(&self.registry);
        registry
            .links
            .get(node)
            .and_then(|link| {
                let down = link.down.as_ref().filter(|down| !down.send.is_closed())?;
                Some((node.to_string(), down.send.clone(), link.generation))
            })
            .ok_or_else(|| Error::new(format!("{}: {node}", moqui_core::edge::OFFLINE)))
    }

    /// A Ping down `node`'s down connection: its round trip.
    pub(crate) async fn ping_local(&self, node: &str) -> Result<Duration, String> {
        let (send, generation) = {
            let registry = lock(&self.registry);
            let link = registry
                .links
                .get(node)
                .ok_or_else(|| format!("{node} is not linked"))?;
            let down = link
                .down
                .as_ref()
                .ok_or_else(|| format!("{node} has no down connection"))?;
            (down.send.clone(), link.generation)
        };
        let mut client = super::local_link_client(send);
        let started = Instant::now();
        match tokio::time::timeout(self.config.handshake_timeout, client.ping(PingRequest { generation })).await {
            Ok(Ok(_)) => Ok(started.elapsed()),
            Ok(Err(status)) => Err(format!("Ping: {}", status.message())),
            Err(_) => Err(format!("{node} did not answer Ping in time")),
        }
    }

    /// Run a delegated call on the local node (the only one whose link is up): its answer, or [`OFFLINE`] at once while
    /// no link is up.
    ///
    /// [`OFFLINE`]: moqui_core::edge::OFFLINE
    pub(crate) async fn delegate(&self, call: DelegateCall) -> Result<DelegateAnswer, Error> {
        let (node, send, generation) = match call.node.as_deref() {
            Some(node) => self.link_of(node)?,
            None => self.only_link()?,
        };
        let mut client = super::local_link_client(send);
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
        let answered = tokio::time::timeout(wait, client.delegate(request)).await;
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

    /// The paired nodes with their live links.
    pub(crate) fn clients(&self) -> Result<Vec<EdgeClientInfo>, Error> {
        let rows = store::list_clients(&*self.factory()?)?;
        let registry = lock(&self.registry);
        Ok(rows
            .into_iter()
            .map(|mut client| {
                if let Some(link) = registry.links.get(&client.paired_node_id) {
                    client.linked = true;
                    client.down = link.down.is_some();
                    client.generation = link.generation;
                    client.remote_address = link.remote.to_string();
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
        let revoked = store::revoke_client(&*self.factory()?, node)?;
        self.revoked(node);
        Ok(revoked)
    }

    pub(crate) fn create_pairing_code(&self, valid_minutes: u32) -> Result<PairingCode, Error> {
        store::create_code(&*self.factory()?, valid_minutes)
    }
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

/// Send a queued item, with its content first, down a node's down connection of `generation`.
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
    Pairing {
        key: Option<String>,
    },
    Up {
        node: String,
        generation: u64,
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
    /// The node and generation of an up connection that is still current.
    fn up(&self) -> Result<(&str, u64), Status> {
        let Mode::Up { node, generation } = &self.mode else {
            return Err(Status::failed_precondition("not an up connection"));
        };
        if self.server.current(node) != *generation {
            return Err(Status::failed_precondition("stale link generation"));
        }
        Ok((node, *generation))
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
            .on_pool(move |factory| store::pair_client(factory, &code, &pairing_node, &key, &local))
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
        let (node, generation) = self.up()?;
        let local = request.into_inner().hello.unwrap_or_default();
        check_versions(LINK_VERSION, local.link_version, "local node").map_err(Status::failed_precondition)?;
        if local.instance_id != node {
            return Err(Status::permission_denied(format!(
                "this key is paired for local node {node}, not {}",
                local.instance_id.chars().take(40).collect::<String>()
            )));
        }
        if let Some(link) = lock(&self.server.registry)
            .links
            .get_mut(node)
            .filter(|link| link.generation == generation)
        {
            link.hello = Some(local.clone());
        }
        let key = node.to_string();
        let saved = self
            .server
            .on_pool(move |factory| store::save_hello(factory, &key, &local))
            .await;
        if let Err(err) = saved {
            tracing::warn!("Edge link of {node}: its Hello was not saved: {err}");
        }
        self.helloed.store(true, Ordering::Release);
        Ok(Response::new(HelloReply {
            hello: Some(self.server.hello()),
            generation,
            turn_secret: self.server.turn_secret(),
        }))
    }

    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let (_, generation) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        Ok(Response::new(PingReply { generation }))
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
        let (node, _) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        // the up connection is the generation; each message's own is not checked again. Only what this node may send
        // is taken, its frames' replies addressed to itself (E7), each stamped with the node (E12)
        let take = super::signal::edge_takes(node);
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
                server.hosts.load(factory)?;
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
        self.server
            .on_pool(move |factory| store::revoke_client(factory, &key))
            .await
            .map_err(|err| {
                tracing::error!("Edge link of {node}: Unpair failed: {err}");
                Status::internal("the edge node could not unpair")
            })?;
        tracing::info!("Edge node: local node {node} unpaired itself");
        lock(&self.server.registry).revoked.insert(node.to_string());
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

#[cfg(test)]
#[path = "accept_tests.rs"]
mod tests;
