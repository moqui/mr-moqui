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

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_core::edge::{EdgeClientInfo, PairingCode};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use tokio::io::AsyncReadExt as _;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::server::TlsStream;
use tonic::{Request, Response, Status};

use super::pair::Identity;
use super::proto::link_server::{Link, LinkServer};
use super::proto::local_link_client::LocalLinkClient;
use super::proto::{
    Hello, HelloReply, HelloRequest, LocalHelloRequest, PairReply, PairRequest, PingReply, PingRequest, ProbeReply,
    ProbeRequest, UnpairReply, UnpairRequest,
};
use super::store::{self, Paired};
use super::{
    check_versions, hello, valid_node_id, write_answer, Answer, EdgeConfig, H2Channel, ALPN, ALPN_PAIR, LINK_VERSION,
    ROLE_CHECK, ROLE_DOWN, ROLE_PAIR, ROLE_UP,
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

/// A secured connection: to be served as HTTP, or a link.
pub(crate) enum Secured {
    Http(TlsStream<TcpStream>),
    Link(TlsStream<TcpStream>),
}

impl Acceptor {
    /// The TLS handshake of a connection to the HTTPS listener: the link's settings when the client offers one of the
    /// link's ALPN protocols, `http` otherwise.
    pub(crate) async fn accept(&self, stream: TcpStream, http: &Arc<rustls::ServerConfig>) -> std::io::Result<Secured> {
        let start = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await?;
        let offered = |wanted: &[u8]| {
            start
                .client_hello()
                .alpn()
                .is_some_and(|mut offered| offered.any(|protocol| protocol == wanted))
        };
        let config = if offered(ALPN) {
            Some(&self.link)
        } else if offered(ALPN_PAIR) {
            Some(&self.pairing)
        } else {
            None
        };
        match config {
            Some(config) => Ok(Secured::Link(start.into_stream(Arc::clone(config)).await?)),
            None => Ok(Secured::Http(start.into_stream(Arc::clone(http)).await?)),
        }
    }

    /// Serve a link connection until it closes.
    pub(crate) async fn serve(&self, stream: TlsStream<TcpStream>, remote: SocketAddr) {
        Arc::clone(&self.server).serve(stream, remote).await;
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
    /// Pair calls by address: when the window began, and how many since.
    pair_attempts: Mutex<HashMap<IpAddr, (Instant, u32)>>,
    shutdown: watch::Receiver<bool>,
}

impl EdgeServer {
    pub(crate) fn new(
        factory: &Arc<ExecutionContextFactory>,
        config: EdgeConfig,
        link_fingerprint: &str,
        shutdown: watch::Receiver<bool>,
    ) -> Arc<Self> {
        let server = Arc::new(Self {
            factory: Arc::downgrade(factory),
            config,
            link_fingerprint: link_fingerprint.to_string(),
            registry: Mutex::new(Registry::default()),
            pair_attempts: Mutex::new(HashMap::new()),
            shutdown,
        });
        let weak = Arc::downgrade(&server);
        moqui_core::telemetry::for_factory(factory).observe_gauge("moqui.edgelink.up", move || {
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
        hello(
            &self.config.instance_id,
            &self.config.name,
            &self.config.capabilities,
            &self.config.public_names,
        )
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
        let now = Instant::now();
        let mut attempts = lock(&self.pair_attempts);
        attempts.retain(|_, (started, _)| now.duration_since(*started) < self.config.pair_window);
        let (_, count) = attempts.entry(ip).or_insert((now, 0));
        *count += 1;
        *count <= self.config.pair_attempts
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
        self.serve_h2(stream, LinkServer::new(service), None, limits).await;
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
        self.serve_h2(stream, LinkServer::new(service), Some(closed), limits)
            .await;
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
        let handshake = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
            .timer(hyper_util::rt::TokioTimer::new())
            .keep_alive_interval(self.config.keepalive)
            .keep_alive_timeout(self.config.keepalive)
            .keep_alive_while_idle(true)
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
        let mut client = LocalLinkClient::with_origin(H2Channel(send.clone()), super::origin());
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
                    link.down = Some(Down { send, _close: close });
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
        let _ = driver.await;
        let mut registry = lock(&self.registry);
        if let Some(link) = registry
            .links
            .get_mut(&node)
            .filter(|link| link.generation == generation)
        {
            link.down = None;
        }
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
        let mut client = LocalLinkClient::with_origin(H2Channel(send), super::origin());
        let started = Instant::now();
        match tokio::time::timeout(self.config.handshake_timeout, client.ping(PingRequest { generation })).await {
            Ok(Ok(_)) => Ok(started.elapsed()),
            Ok(Err(status)) => Err(format!("Ping: {}", status.message())),
            Err(_) => Err(format!("{node} did not answer Ping in time")),
        }
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
        }))
    }

    async fn ping(&self, _request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let (_, generation) = self.up()?;
        if !self.helloed.load(Ordering::Acquire) {
            return Err(Status::failed_precondition("Hello first"));
        }
        Ok(Response::new(PingReply { generation }))
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
