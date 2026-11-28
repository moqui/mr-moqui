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

//! The manager's end of the manager link (plans/archive/Vault-Instances.md §4.4), with the `instances` feature: the HTTPS
//! listener's hand-off of ALPN `moqui-manage/1` ([`Acceptor`]), and the registry of linked instances by `AppInstance`,
//! with each one's generation, last status and Serve stream, through which [`ManagerServer::call`] runs a command on
//! it. The rules on the rows are `moqui_instance::link`'s.
//!
//! A ClientHello that offers `moqui-manage/1` gets the certificate of the manager's link key and must present a
//! client certificate; any other gets the listener's own settings (an ACME challenge its answer). On an edge node the
//! edge link reads the ClientHello and takes no manager link: a manager is an inner-zone server, reached through its
//! own edge node's passthrough host name when it is behind a firewall (docs/Run-Deploy.md "Managed instances").
//!
//! A link's calls: Pair on a pairing connection alone, a few times an hour from one address; on a link connection the
//! instance's key must be pinned on a row (looked up again at its Hello, so a revoke in between holds), Hello comes
//! first, and while its last Hello or Status said sealed only Hello, Status, Bootstrap, Unseal and SealedShares are
//! answered.
//!
//! **Unsealed, as the manager takes it** (M6). The instance's own claim of unsealed is not enough: a command that
//! needs an unsealed instance (all but status and seal) runs only on a link generation whose unseal this manager made
//! (by transit, under the environment's policy: `moqui_instance::transit::unseal`) or on which the instance proved,
//! with the key derived from its root key, that it holds that key (`moqui_instance::transit::check_proof`). A claim with
//! neither is taken as sealed ([`ManagerServer::call`]): its status reports `sealState` `unverified`, which a deploy and a
//! rotation read as not up, and Monitor's rows keep it sealed. Once a link is taken as unsealed and serves commands, the
//! instance's recovery bundle is deposited when it changed (`moqui_instance::shares::deposit_seal_bundle`).
//!
//! **A link from a new address.** A new link of an instance from another address than its live link's, or than its
//! last link's within `manage@address-change-wait-seconds` of that link going down, is refused and told to the
//! administrators, until an operator confirms the move (`confirm#InstanceLinkAddress`) or the wait passes; a pairing
//! confirms it too ([`admit_address`]). With M6 a holder of a stolen link key gets no command from a claim of unsealed;
//! under the automatic policy it still gets the root key from transit until the instance is revoked (plans/
//! Vault-Instances.md §4.4, the limit of that key), and that unseal from a new address raises Monitor's alert.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin as BoxPin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures_util::Stream;
use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_lib::sync::lock;
use tokio::io::AsyncReadExt as _;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_rustls::server::TlsStream;
use tonic::{Request, Response, Status, Streaming};

use super::key::{KeyStore, LinkKey};
use super::proto::manager_server::{Manager, ManagerServer as ManagerGrpc};
use super::proto::{
    BootstrapReply, BootstrapRequest, Command, CommandReply, EventReply, EventRequest, HelloReply, HelloRequest,
    HoldersReply, HoldersRequest, InstanceStatus, PairReply, PairRequest, SealedShare, SealedSharesReply,
    SealedSharesRequest, ShareConfirmation, ShareConfirmationsReply, ShareConfirmationsRequest, SharesReply,
    SharesRequest, StatusReply, UnsealReply, UnsealRequest,
};
use super::state::set_to_proto;
use super::{check_versions, Call, CommandKind, ALPN, LINK_VERSION, MAX_MESSAGE, ROLE_PAIR, ROLE_UP};
use crate::link::{read_hello, write_answer, Answer};
use crate::tls_pin::{hex, public_key_sha256, Pin, PinVerifier};

/// The name in the manager's link key certificate.
const CERT_NAME: &str = "moqui-manager";

/// What a link whose key is not pinned is told.
const NOT_PAIRED: &str = "this instance's link key is not pinned on the manager, or was revoked: pair again";

/// The most of an event's message that is logged.
const MAX_EVENT_TEXT: usize = 200;

/// The manager's timings and limits.
#[derive(Clone, Debug)]
pub struct AcceptConfig {
    /// HTTP/2 PING interval, and how long an unanswered one waits.
    pub keepalive: Duration,
    /// From the handshake to the role byte, and from a link connection's start to its Hello.
    pub handshake_timeout: Duration,
    /// The longest a pairing connection stays open.
    pub pairing_lifetime: Duration,
    /// The Pair calls one address may make in `pair_window`.
    pub pair_attempts: usize,
    pub pair_window: Duration,
    /// How long after an instance's link went down a link from another address is refused unless confirmed
    /// (`manage@address-change-wait-seconds`).
    pub address_wait: Duration,
}

impl Default for AcceptConfig {
    fn default() -> Self {
        Self {
            keepalive: Duration::from_secs(15),
            handshake_timeout: Duration::from_secs(10),
            pairing_lifetime: Duration::from_secs(30),
            pair_attempts: 10,
            pair_window: Duration::from_secs(600),
            address_wait: Duration::from_secs(300),
        }
    }
}

/// A command waiting for its answer.
type Waiting = std::sync::mpsc::SyncSender<CommandReply>;

/// A linked instance.
struct Linked {
    generation: u64,
    status: InstanceStatus,
    /// The manager takes this link's "unsealed" (M6): it unsealed it by transit, or the instance proved it.
    verified: bool,
    remote: SocketAddr,
    /// Its Serve stream, while open.
    commands: Option<tokio::sync::mpsc::UnboundedSender<Command>>,
    waiting: Arc<Mutex<HashMap<u64, Waiting>>>,
    /// Closes its connection when it drops (a newer generation, a revoke).
    _close: watch::Sender<bool>,
}

/// What the manager's screens show of an instance's link.
#[derive(Clone, Debug, Default)]
pub struct LinkView {
    pub linked: bool,
    pub generation: u64,
    pub status: InstanceStatus,
    pub serving: bool,
    pub remote: String,
    /// The manager takes the link's "unsealed" (M6).
    pub verified: bool,
}

/// The manager's end: its link key, the linked instances, and how to reach its database.
pub struct ManagerServer {
    factory: Weak<ExecutionContextFactory>,
    config: AcceptConfig,
    manager_id: String,
    fingerprint: String,
    linked: Mutex<HashMap<String, Linked>>,
    generation: AtomicU64,
    command_ids: AtomicU64,
    pair_calls: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    /// Each instance's last link address, and since when it is down (none while it is linked).
    seen: Mutex<HashMap<String, (IpAddr, Option<Instant>)>>,
    /// Instances whose next link may come from a new address: an operator confirmed it, or it paired.
    confirmed: Mutex<std::collections::HashSet<String>>,
    /// When each instance's refused move was last told to the administrators.
    alerted: Mutex<HashMap<String, Instant>>,
    shutdown: watch::Receiver<bool>,
}

/// How often a refused move of one instance is told to the administrators.
const ALERT_EVERY: Duration = Duration::from_secs(3600);

/// The seal state a status reports for a claim of unsealed the manager does not take (M6).
pub const UNVERIFIED: &str = "unverified";

/// The notification topic of a link refused from a new address.
pub const ADDRESS_TOPIC: &str = "InstanceLinkAddress";

/// Whether a new link of an instance from `new` is admitted (see the module doc): `live` is the address of its live
/// link, `last` that of its last link and how long ago it went down. Refused while another address's link is live, or
/// went down less than `wait` ago, unless `confirmed`.
pub fn admit_address(
    live: Option<IpAddr>,
    last: Option<(IpAddr, Duration)>,
    new: IpAddr,
    wait: Duration,
    confirmed: bool,
) -> Result<(), String> {
    if confirmed {
        return Ok(());
    }
    if let Some(live) = live.filter(|live| *live != new) {
        return Err(format!(
            "a link of this instance is live from {live}, and this one comes from {new}: an operator confirms a move \
             (System › Instances)"
        ));
    }
    if let Some((last, since)) = last.filter(|(last, since)| *last != new && *since < wait) {
        return Err(format!(
            "this instance's link from {last} went down {} s ago, and this one comes from {new}: an operator \
             confirms a move (System › Instances), or it is taken after {} s",
            since.as_secs(),
            wait.as_secs()
        ));
    }
    Ok(())
}

impl std::fmt::Debug for ManagerServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagerServer")
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

/// The manager's server on a factory, while its listener runs.
#[derive(Default)]
struct Kept(Mutex<Option<Arc<ManagerServer>>>);

/// The manager's acceptor on a factory, for a link that reaches it other than on the HTTPS listener: a passthrough
/// tunnel its edge node sends down the edge link ([`acceptor_of`]).
#[derive(Default)]
struct KeptAcceptor(Mutex<Option<Arc<Acceptor>>>);

/// The manager's acceptor of `factory`, when its HTTPS listener takes manager links: for a manager behind a firewall,
/// which its instances reach at a passthrough host name of its edge node (plans/archive/Vault-Instances.md §4.4).
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn acceptor_of(factory: &ExecutionContextFactory) -> Option<Arc<Acceptor>> {
    lock(&factory.extension(|_| KeptAcceptor::default()).0).clone()
}

/// The bytes under a link connection's TLS: a TCP connection to the HTTPS listener, or a passthrough tunnel the
/// manager's edge node sent down its edge link.
pub(crate) trait ManageStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> ManageStream for S {}

/// The manager's server of `factory`, when its HTTPS listener takes manager links.
pub(crate) fn server_of(factory: &ExecutionContextFactory) -> Option<Arc<ManagerServer>> {
    lock(&factory.extension(|_| Kept::default()).0).clone()
}

/// The HTTPS listener's hand-off of `moqui-manage/1`.
pub(crate) struct Acceptor {
    tls: Arc<rustls::ServerConfig>,
    server: Arc<ManagerServer>,
}

/// A connection after its handshake: the listener's own, or the link's.
pub(crate) enum Secured {
    Http(TlsStream<TcpStream>),
    Link(TlsStream<TcpStream>),
}

/// The manager's acceptor, with its link key from `runtime/db/manage_link_key`; none on an edge node, whose edge link
/// reads the ClientHello, or when the key cannot be had.
pub(crate) fn start(
    factory: &Arc<ExecutionContextFactory>,
    shutdown: watch::Receiver<bool>,
    edge_node: bool,
) -> Option<Arc<Acceptor>> {
    if edge_node {
        tracing::info!("Manager link: this edge node takes no manager links on its HTTPS listener");
        return None;
    }
    let file = factory.runtime_dir().join("db").join(super::KEY_FILE);
    let key = KeyStore::from_settings(&super::server_setting)
        .and_then(|store| LinkKey::load_or_create(&file, &store, CERT_NAME));
    let key = match key {
        Ok(key) => key,
        Err(err) => {
            tracing::error!("Manager link: managed instances cannot link here: {err}");
            return None;
        }
    };
    let link_url = super::server_setting("manage@link-url");
    moqui_instance::link::set_manager(&link_url, key.fingerprint());
    let instance_id = moqui_core::cluster::instance_id(factory).unwrap_or_default();
    let mut config = AcceptConfig::default();
    if let Ok(seconds) = super::server_setting("manage@address-change-wait-seconds").parse::<u64>() {
        config.address_wait = Duration::from_secs(seconds);
    }
    let acceptor = acceptor(factory, &key, &instance_id, config, shutdown);
    if acceptor.is_some() {
        tracing::info!(
            "Manager link: taking managed instances' links on the HTTPS listener (link key {}{})",
            key.fingerprint(),
            if link_url.is_empty() {
                String::new()
            } else {
                format!(", dialed at {link_url}")
            }
        );
    }
    acceptor
}

/// The acceptor of `key`, kept on `factory` for its services.
pub(crate) fn acceptor(
    factory: &Arc<ExecutionContextFactory>,
    key: &LinkKey,
    manager_id: &str,
    config: AcceptConfig,
    shutdown: watch::Receiver<bool>,
) -> Option<Arc<Acceptor>> {
    let identity = key.identity();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    // any key passes the handshake, which proves its possession; it is looked up among the pinned ones after it
    let verifier = PinVerifier::new(Pin::Deferred, true, &provider);
    let tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|err| err.to_string())
        .map(|builder| builder.with_client_cert_verifier(verifier))
        .and_then(|builder| {
            builder
                .with_single_cert(vec![identity.cert()], identity.key_der())
                .map_err(|err| err.to_string())
        });
    let mut tls = match tls {
        Ok(tls) => tls,
        Err(err) => {
            tracing::error!("Manager link: its TLS settings: {err}");
            return None;
        }
    };
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let server = Arc::new(ManagerServer {
        factory: Arc::downgrade(factory),
        config,
        manager_id: manager_id.to_string(),
        fingerprint: key.fingerprint().to_string(),
        linked: Mutex::new(HashMap::new()),
        generation: AtomicU64::new(0),
        command_ids: AtomicU64::new(0),
        pair_calls: Mutex::new(HashMap::new()),
        seen: Mutex::new(HashMap::new()),
        confirmed: Mutex::new(std::collections::HashSet::new()),
        alerted: Mutex::new(HashMap::new()),
        shutdown,
    });
    *lock(&factory.extension(|_| Kept::default()).0) = Some(Arc::clone(&server));
    let acceptor = Arc::new(Acceptor {
        tls: Arc::new(tls),
        server,
    });
    *lock(&factory.extension(|_| KeptAcceptor::default()).0) = Some(Arc::clone(&acceptor));
    Some(acceptor)
}

impl Acceptor {
    /// The start of a connection to the HTTPS listener: the link's settings for a client that offers
    /// `moqui-manage/1`, an ACME challenge's answer, else the listener's own.
    pub(crate) async fn accept(
        &self,
        mut stream: TcpStream,
        http: &Arc<rustls::ServerConfig>,
        factory: &Weak<ExecutionContextFactory>,
    ) -> std::io::Result<Secured> {
        let (accepted, _) = read_hello(&mut stream).await?;
        let (manage, acme, name) = {
            let hello = accepted.client_hello();
            let offered = |wanted: &[u8]| hello.alpn().is_some_and(|mut alpn| alpn.any(|offer| offer == wanted));
            (
                offered(ALPN),
                offered(crate::acme::ALPN),
                hello.server_name().map(str::to_string),
            )
        };
        if manage {
            let start = tokio_rustls::StartHandshake::from_parts(accepted, stream);
            return Ok(Secured::Link(start.into_stream(Arc::clone(&self.tls)).await?));
        }
        if acme {
            return Err(crate::acme::answer(accepted, stream, factory, name.as_deref()).await);
        }
        let start = tokio_rustls::StartHandshake::from_parts(accepted, stream);
        Ok(Secured::Http(start.into_stream(Arc::clone(http)).await?))
    }

    /// Serve a link connection until it closes, the listeners stop, or `closing` turns true: the closing of the
    /// listener it came over, which ends it with that listener alone (plans/Runtime-Config.md §8.3).
    pub(crate) async fn serve<S: ManageStream>(
        &self,
        stream: TlsStream<S>,
        remote: SocketAddr,
        closing: watch::Receiver<bool>,
    ) {
        Arc::clone(&self.server).serve(stream, remote, closing).await;
    }

    /// A link connection that came down a passthrough tunnel, its ClientHello read (`accepted`, which offered
    /// `moqui-manage/1`): the link's handshake, then served as on the listener, from `remote`, the instance's address
    /// the tunnel's PROXY header named. `limit` bounds the handshake.
    #[cfg_attr(not(feature = "edge"), allow(dead_code))]
    pub(crate) async fn serve_passthrough<S: ManageStream>(
        &self,
        accepted: rustls::server::Accepted,
        stream: S,
        remote: SocketAddr,
        limit: Duration,
    ) {
        let start = tokio_rustls::StartHandshake::from_parts(accepted, stream);
        match tokio::time::timeout(limit, start.into_stream(Arc::clone(&self.tls))).await {
            // down a link this node dialed, on no listener of its own: it ends with the listeners' stop
            Ok(Ok(stream)) => self.serve(stream, remote, self.server.shutdown.clone()).await,
            Ok(Err(err)) => tracing::debug!("manager link through the edge node from {remote}: TLS failed: {err}"),
            Err(_) => tracing::debug!("manager link through the edge node from {remote}: TLS timed out"),
        }
    }
}

/// A connection's state on the manager.
struct Conn {
    helloed: AtomicBool,
    /// The instance's last Hello or Status said sealed; so until its Hello.
    sealed: AtomicBool,
    instance: String,
    bundle_key: Vec<u8>,
    generation: u64,
    remote: SocketAddr,
    /// The key the connection presented: a link's pinned one, a pairing connection's new one.
    fingerprint: String,
    /// The pairing connection's new key.
    pairing_key: Option<String>,
    close: Mutex<Option<watch::Sender<bool>>>,
    /// What the instance's unsealed proof on this link signs (M6), random for each link.
    challenge: Vec<u8>,
    /// The manager takes this link's "unsealed" (M6).
    verified: AtomicBool,
    /// The seal state last written to the instance's row for Monitor: a claim of unsealed the manager does not take is
    /// written as `unverified`, so the instance stays sealed there.
    noted: Mutex<String>,
}

#[derive(Clone)]
struct Service {
    server: Arc<ManagerServer>,
    conn: Arc<Conn>,
}

/// Why a call is refused before its work.
fn refuse(call: Call, conn: &Conn) -> Result<(), Status> {
    if conn.pairing_key.is_some() {
        return if call == Call::Pair {
            Ok(())
        } else {
            Err(Status::permission_denied("a pairing connection serves Pair alone"))
        };
    }
    if call == Call::Pair {
        return Err(Status::permission_denied(
            "this instance is paired: a link connection serves no Pair",
        ));
    }
    if call != Call::Hello && !conn.helloed.load(Ordering::Acquire) {
        return Err(Status::failed_precondition("Hello comes first"));
    }
    if !call.sealed_ok() && conn.sealed.load(Ordering::Acquire) {
        return Err(Status::failed_precondition(format!(
            "sealed: {} waits until the instance is unsealed (only Hello, Status, Bootstrap and Unseal while sealed)",
            call.name()
        )));
    }
    Ok(())
}

impl ManagerServer {
    /// SHA-256 of the manager's link key, hex: what its instances pin.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    async fn on_pool<T, F>(&self, job: F) -> Result<T, Status>
    where
        T: Send + 'static,
        F: FnOnce(&moqui_core::entity::EntityFacade) -> Result<T, moqui_lib::error::Error> + Send + 'static,
    {
        crate::worker::on_pool_without_authz(&self.factory, move |factory| {
            let facade = factory
                .entity()
                .ok_or_else(|| CoreError::new("the manager's database is not open"))?;
            job(&facade).map_err(CoreError::from)
        })
        .await
        .map_err(|err| Status::permission_denied(err.message().to_string()))
    }

    /// Count a Pair call from `ip`; false when it is over the limit for the window.
    fn pair_allowed(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut calls = lock(&self.pair_calls);
        let window = self.config.pair_window;
        calls.retain(|_, at| {
            at.retain(|at| now.duration_since(*at) < window);
            !at.is_empty()
        });
        let at = calls.entry(ip).or_default();
        if at.len() >= self.config.pair_attempts {
            return false;
        }
        at.push(now);
        true
    }

    async fn serve<S: ManageStream>(
        self: Arc<Self>,
        mut stream: TlsStream<S>,
        remote: SocketAddr,
        listener: watch::Receiver<bool>,
    ) {
        let fingerprint = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .and_then(public_key_sha256)
            .map(|digest| hex(&digest));
        let mut role = [0_u8; 1];
        let read = tokio::time::timeout(self.config.handshake_timeout, stream.read_exact(&mut role)).await;
        if !matches!(read, Ok(Ok(_))) {
            return;
        }
        let Some(fingerprint) = fingerprint else {
            let _ = write_answer(
                &mut stream,
                Answer::Refused,
                "the manager link needs the instance's key",
            )
            .await;
            return;
        };
        match role[0] {
            ROLE_PAIR => {
                if write_answer(&mut stream, Answer::Ok, "").await.is_err() {
                    return;
                }
                let conn = Arc::new(Conn {
                    helloed: AtomicBool::new(false),
                    sealed: AtomicBool::new(true),
                    instance: String::new(),
                    bundle_key: Vec::new(),
                    generation: 0,
                    remote,
                    fingerprint: fingerprint.clone(),
                    pairing_key: Some(fingerprint),
                    close: Mutex::new(None),
                    challenge: Vec::new(),
                    verified: AtomicBool::new(false),
                    noted: Mutex::new(String::new()),
                });
                let lifetime = self.config.pairing_lifetime;
                self.serve_h2(stream, conn, None, Some(lifetime), listener).await;
            }
            ROLE_UP => self.serve_up(stream, remote, fingerprint, listener).await,
            _ => {
                let _ = write_answer(&mut stream, Answer::Failed, "unknown role").await;
            }
        }
    }

    async fn serve_up<S: ManageStream>(
        self: Arc<Self>,
        mut stream: TlsStream<S>,
        remote: SocketAddr,
        fingerprint: String,
        listener: watch::Receiver<bool>,
    ) {
        let lookup = fingerprint.clone();
        let linked = self
            .on_pool(move |facade| moqui_instance::link::linked(facade, &lookup))
            .await;
        let linked = match linked {
            Ok(Some(linked)) => linked,
            Ok(None) => {
                tracing::info!("Manager link from {remote} refused: key {fingerprint} is not pinned");
                let _ = write_answer(&mut stream, Answer::Refused, NOT_PAIRED).await;
                return;
            }
            Err(err) => {
                tracing::error!("Manager link from {remote}: {}", err.message());
                let _ = write_answer(&mut stream, Answer::Failed, "the manager could not take the link").await;
                return;
            }
        };
        if write_answer(&mut stream, Answer::Ok, "").await.is_err() {
            return;
        }
        let (close, closed) = watch::channel(false);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let conn = Arc::new(Conn {
            helloed: AtomicBool::new(false),
            sealed: AtomicBool::new(true),
            instance: linked.app_instance_id.clone(),
            bundle_key: linked.bundle_key,
            generation,
            remote,
            fingerprint,
            pairing_key: None,
            close: Mutex::new(Some(close)),
            challenge: {
                let mut challenge = vec![0_u8; 32];
                moqui_vault::rng::Rng::fill(&mut moqui_vault::rng::OsRng, &mut challenge);
                challenge
            },
            verified: AtomicBool::new(false),
            noted: Mutex::new(String::new()),
        });
        let deadline = self.config.handshake_timeout;
        Arc::clone(&self)
            .serve_h2(stream, Arc::clone(&conn), Some((closed, deadline)), None, listener)
            .await;
        let gone = {
            let mut linked = lock(&self.linked);
            let current = linked
                .get(&conn.instance)
                .is_some_and(|link| link.generation == generation);
            if current {
                linked.remove(&conn.instance);
            }
            current
        };
        if gone {
            if let Some(seen) = lock(&self.seen).get_mut(&conn.instance) {
                seen.1 = Some(Instant::now());
            }
            let instance = conn.instance.clone();
            let _ = self
                .on_pool(move |facade| {
                    moqui_instance::link::note_state(facade, &instance, "down", None, "");
                    // what holders sealed to its unseal goes with the link (M6)
                    moqui_instance::shares::drop_relay(facade, &instance);
                    Ok(())
                })
                .await;
            tracing::info!(
                "Manager link of AppInstance {} (generation {generation}) closed",
                conn.instance
            );
        }
    }

    /// Serve the `Manager` service on a connection until it ends, `closed` signals (with the time its Hello must come
    /// in), the listeners stop, `listener` (the closing of the listener it came over) turns true, or a pairing
    /// connection's time is up.
    async fn serve_h2<S: ManageStream>(
        self: Arc<Self>,
        stream: TlsStream<S>,
        conn: Arc<Conn>,
        closed: Option<(watch::Receiver<bool>, Duration)>,
        lifetime: Option<Duration>,
        mut listener: watch::Receiver<bool>,
    ) {
        let service = ManagerGrpc::new(Service {
            server: Arc::clone(&self),
            conn: Arc::clone(&conn),
        })
        .max_decoding_message_size(MAX_MESSAGE)
        .max_encoding_message_size(MAX_MESSAGE);
        let mut builder = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new());
        builder
            .timer(hyper_util::rt::TokioTimer::new())
            .keep_alive_interval(self.config.keepalive)
            .keep_alive_timeout(self.config.keepalive);
        if lifetime.is_some() {
            builder.max_concurrent_streams(2);
        }
        let connection = builder.serve_connection(
            hyper_util::rt::TokioIo::new(stream),
            hyper_util::service::TowerToHyperService::new(service),
        );
        let mut shutdown = self.shutdown.clone();
        let (closing, silent) = match closed {
            Some((mut closed, deadline)) => {
                let helloed = Arc::clone(&conn);
                (
                    Box::pin(async move {
                        let _ = closed.changed().await;
                    }) as BoxPin<Box<dyn std::future::Future<Output = ()> + Send>>,
                    Box::pin(async move {
                        tokio::time::sleep(deadline).await;
                        if helloed.helloed.load(Ordering::Acquire) {
                            std::future::pending::<()>().await;
                        }
                    }) as BoxPin<Box<dyn std::future::Future<Output = ()> + Send>>,
                )
            }
            None => (
                Box::pin(std::future::pending::<()>()) as BoxPin<Box<dyn std::future::Future<Output = ()> + Send>>,
                Box::pin(std::future::pending::<()>()) as BoxPin<Box<dyn std::future::Future<Output = ()> + Send>>,
            ),
        };
        let expired = async move {
            match lifetime {
                Some(lifetime) => tokio::time::sleep(lifetime).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            served = connection => {
                if let Err(err) = served {
                    tracing::debug!("manager link connection ended: {err}");
                }
            }
            () = closing => {}
            () = silent => tracing::debug!("manager link: a link connection sent no Hello in time"),
            () = expired => tracing::debug!("manager link: a pairing connection reached its time limit"),
            _ = shutdown.wait_for(|stopped| *stopped) => {}
            _ = listener.wait_for(|closing| *closing) => {}
        }
    }

    /// Register a link that said Hello: it replaces the instance's older one, whose connection closes. A link older
    /// than the one registered is refused.
    fn register(&self, conn: &Conn, status: InstanceStatus) -> Result<(), Status> {
        let mut linked = lock(&self.linked);
        if let Some(live) = linked
            .get(&conn.instance)
            .filter(|live| live.generation > conn.generation)
        {
            return Err(Status::failed_precondition(format!(
                "a newer link of this instance is up (generation {})",
                live.generation
            )));
        }
        let Some(close) = lock(&conn.close).take() else {
            return Err(Status::failed_precondition("this link said Hello already"));
        };
        let replaced = linked.insert(
            conn.instance.clone(),
            Linked {
                generation: conn.generation,
                status,
                verified: false,
                remote: conn.remote,
                commands: None,
                waiting: Arc::default(),
                _close: close,
            },
        );
        drop(linked);
        lock(&self.seen).insert(conn.instance.clone(), (conn.remote.ip(), None));
        if let Some(old) = replaced {
            tracing::info!(
                "Manager link of AppInstance {}: generation {} from {} replaces generation {}",
                conn.instance,
                conn.generation,
                conn.remote,
                old.generation
            );
        }
        Ok(())
    }

    /// Whether a link of `instance` from `remote` is admitted, by [`admit_address`]; a confirmation is used up.
    fn admit(&self, instance: &str, remote: SocketAddr) -> Result<(), String> {
        let live = lock(&self.linked).get(instance).map(|link| link.remote.ip());
        let last = lock(&self.seen)
            .get(instance)
            .and_then(|(ip, down)| down.map(|down| (*ip, down.elapsed())));
        let confirmed = lock(&self.confirmed).contains(instance);
        admit_address(live, last, remote.ip(), self.config.address_wait, confirmed)?;
        lock(&self.confirmed).remove(instance);
        Ok(())
    }

    /// Let `instance`'s next link come from a new address: an operator confirmed the move.
    pub fn confirm_move(&self, instance: &str) {
        lock(&self.confirmed).insert(instance.to_string());
        tracing::warn!("Manager link: an operator let AppInstance {instance} link from a new address");
    }

    /// Forget where `instance` linked from, and let its next link come from anywhere: it paired again.
    fn repaired(&self, instance: &str) {
        self.close(instance);
        lock(&self.seen).remove(instance);
        lock(&self.confirmed).insert(instance.to_string());
    }

    /// Tell the administrators that a link of `instance` from `remote` was refused, at most once an hour each; a
    /// failure is logged.
    async fn tell_move(&self, instance: &str, remote: SocketAddr, why: &str) {
        {
            let mut alerted = lock(&self.alerted);
            if alerted.get(instance).is_some_and(|at| at.elapsed() < ALERT_EVERY) {
                return;
            }
            alerted.insert(instance.to_string(), Instant::now());
        }
        let (instance, remote, why) = (instance.to_string(), remote.ip().to_string(), why.to_string());
        let told = self
            .on_pool(move |_| {
                let mut message = moqui_lib::object::ValueMap::default();
                for (name, value) in [
                    ("appInstanceId", &instance),
                    ("remoteAddress", &remote),
                    ("reason", &why),
                ] {
                    message.insert(name.to_string(), moqui_lib::object::Value::string(value));
                }
                let clean = |text: &str| text.replace(['$', '{', '}'], "");
                moqui_core::notify::NotificationMessage::new()
                    .topic(ADDRESS_TOPIC)
                    .user_group_id("ADMIN")
                    .notification_type("warning")
                    .message_map(message)
                    .title(format!(
                        "Instance {}: a link from a new address {} was refused",
                        clean(&instance),
                        clean(&remote)
                    ))
                    .send()
                    .map_err(|err| moqui_lib::error::Error::new(err.message().to_string()))
            })
            .await;
        if let Err(err) = told {
            tracing::error!(
                "Manager link: the administrators were not told of a refused move: {}",
                err.message()
            );
        }
    }

    /// Keep an instance's status, if `generation` is still its link; true when its cluster's members changed.
    fn note_status(&self, instance: &str, generation: u64, status: &InstanceStatus) -> bool {
        if let Some(link) = lock(&self.linked)
            .get_mut(instance)
            .filter(|link| link.generation == generation)
        {
            let members_changed = link.status.cluster_members != status.cluster_members;
            link.status = status.clone();
            return members_changed;
        }
        false
    }

    /// What the screens show of `instance`'s link.
    #[must_use]
    pub fn view(&self, instance: &str) -> LinkView {
        lock(&self.linked)
            .get(instance)
            .map(|link| LinkView {
                linked: true,
                generation: link.generation,
                status: link.status.clone(),
                serving: link.commands.is_some(),
                remote: link.remote.to_string(),
                verified: link.verified,
            })
            .unwrap_or_default()
    }

    /// Whether the manager takes the "unsealed" of `instance`'s link `generation` (M6), kept for its commands.
    fn set_verified(&self, conn: &Conn, verified: bool) {
        let was = conn.verified.swap(verified, Ordering::AcqRel);
        if let Some(link) = lock(&self.linked)
            .get_mut(&conn.instance)
            .filter(|link| link.generation == conn.generation)
        {
            link.verified = verified;
        }
        if was != verified {
            tracing::info!(
                "Manager link: AppInstance {} (generation {}) is {} as unsealed",
                conn.instance,
                conn.generation,
                if verified { "taken" } else { "not taken" }
            );
        }
    }

    /// The instance's recovery bundle over its link, deposited as an operational item when it is not the one deposited
    /// last (the first time, and after a rekey or rotate-root; plans/archive/Vault-Instances.md §4.5): in the background, once
    /// its Serve stream is up; a failure is logged.
    fn deposit_bundle(self: Arc<Self>, instance: &str) {
        let instance = instance.to_string();
        tokio::spawn(async move {
            let (server, asked) = (Arc::clone(&self), instance.clone());
            let exported = tokio::task::spawn_blocking(move || {
                // the Serve stream is up as its first status as unsealed reaches the manager: a few seconds for both
                for _ in 0..30 {
                    let view = server.view(&asked);
                    if view.serving && view.verified && view.status.seal_state == "unsealed" {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                server.call(&asked, super::proto::command::Body::ExportSeal(Default::default()))
            })
            .await;
            let bundle = match exported {
                Ok(Ok(reply)) if reply.ok && !reply.text.is_empty() => reply.text,
                Ok(Ok(reply)) => {
                    tracing::warn!(
                        "Manager link: AppInstance {instance} gave no recovery bundle: {}",
                        reply.error
                    );
                    return;
                }
                Ok(Err(err)) => {
                    tracing::warn!("Manager link: no recovery bundle from AppInstance {instance}: {err}");
                    return;
                }
                Err(err) => {
                    tracing::error!("Manager link: the recovery bundle of AppInstance {instance}: {err}");
                    return;
                }
            };
            let kept = self
                .on_pool(move |facade| moqui_instance::shares::deposit_seal_bundle(facade, &instance, &bundle))
                .await;
            if let Err(status) = kept {
                tracing::warn!(
                    "Manager link: the recovery bundle was not deposited: {}",
                    status.message()
                );
            }
        });
    }

    /// Close `instance`'s link (a revoke, a pairing again).
    pub fn close(&self, instance: &str) {
        lock(&self.linked).remove(instance);
    }

    /// Forget `instance`'s link and where it came from (a revoke).
    pub fn revoked(&self, instance: &str) {
        self.close(instance);
        lock(&self.seen).remove(instance);
        lock(&self.confirmed).remove(instance);
    }

    /// Run `body` on `instance` over its link and wait for the answer, up to the command's time. While the instance
    /// is sealed only status is answered, from what it last said; a command that needs it unsealed runs only on a link
    /// whose "unsealed" the manager took (M6: see the module doc), and is refused on one it did not.
    pub fn call(&self, instance: &str, body: super::proto::command::Body) -> Result<CommandReply, String> {
        let command = Command {
            id: 0,
            body: Some(body),
        };
        let kind = CommandKind::of(&command).ok_or("not a command of the manager link")?;
        let (sender, waiting, status, verified) = {
            let linked = lock(&self.linked);
            let link = linked
                .get(instance)
                .ok_or_else(|| format!("AppInstance {instance} is not linked"))?;
            (
                link.commands.clone(),
                Arc::clone(&link.waiting),
                link.status.clone(),
                link.verified,
            )
        };
        let sealed = status.seal_state != "unsealed";
        // a claim of unsealed the manager does not take: `unverified`, which a deploy and a rotation read as not up
        let taken = |mut status: InstanceStatus| {
            if status.seal_state == "unsealed" && !verified {
                status.seal_state = UNVERIFIED.to_string();
            }
            status
        };
        if kind == CommandKind::Status && (sealed || sender.is_none()) {
            return Ok(CommandReply {
                id: 0,
                ok: true,
                error: String::new(),
                status: Some(taken(status)),
                text: String::new(),
            });
        }
        if sealed && !kind.sealed_ok() {
            return Err(format!(
                "AppInstance {instance} is sealed: {} waits until it is unsealed",
                kind.name()
            ));
        }
        if kind.needs_verified_unseal() && !verified {
            return Err(format!(
                "AppInstance {instance} says it is unsealed, and this manager neither unsealed it on this link nor had it prove it holds its root key: {} waits, taken as sealed (plans/archive/Vault-Instances.md §4.4)",
                kind.name()
            ));
        }
        let sender = sender.ok_or_else(|| format!("AppInstance {instance} takes no commands yet"))?;
        let id = self.command_ids.fetch_add(1, Ordering::AcqRel) + 1;
        let (answer, answered) = std::sync::mpsc::sync_channel(1);
        lock(&waiting).insert(id, answer);
        if sender.send(Command { id, ..command }).is_err() {
            lock(&waiting).remove(&id);
            return Err(format!("the link of AppInstance {instance} closed"));
        }
        let reply = answered.recv_timeout(kind.timeout());
        lock(&waiting).remove(&id);
        let mut reply = reply.map_err(|_| format!("AppInstance {instance} did not answer {} in time", kind.name()))?;
        reply.status = reply.status.map(taken);
        Ok(reply)
    }
}

type CommandStream = BoxPin<Box<dyn Stream<Item = Result<Command, Status>> + Send>>;

impl Service {
    /// Write the instance's seal state for Monitor when it changed: a claim of unsealed the manager does not take is
    /// `unverified` (sealed there); once taken, `unsealed`, and the relay of its unseal by shares goes (M6).
    async fn note_seal(&self, claimed: &str) -> Result<(), Status> {
        let effective = if claimed == "unsealed" && !self.conn.verified.load(Ordering::Acquire) {
            UNVERIFIED.to_string()
        } else {
            claimed.to_string()
        };
        {
            let mut noted = lock(&self.conn.noted);
            if *noted == effective {
                return Ok(());
            }
            noted.clone_from(&effective);
        }
        let (instance, remote) = (self.conn.instance.clone(), self.conn.remote.ip().to_string());
        tracing::info!("Manager link: AppInstance {instance} is {effective}");
        self.server
            .on_pool(move |facade| {
                moqui_instance::link::note_state(facade, &instance, "linked", Some(&effective), &remote);
                if effective == "unsealed" {
                    moqui_instance::shares::drop_relay(facade, &instance);
                }
                Ok(())
            })
            .await
    }

    /// The recovery share calls run only on a link the manager takes as unsealed (M6): a holder of the link key alone
    /// sends no share set in the instance's name and reads no confirmation.
    fn require_verified(&self, call: Call) -> Result<(), Status> {
        if self.conn.verified.load(Ordering::Acquire) {
            return Ok(());
        }
        Err(Status::failed_precondition(format!(
            "{} waits until the manager takes this link's unsealed (its transit unseal, or the instance's proof)",
            call.name()
        )))
    }
}

#[tonic::async_trait]
impl Manager for Service {
    async fn pair(&self, request: Request<PairRequest>) -> Result<Response<PairReply>, Status> {
        refuse(Call::Pair, &self.conn)?;
        let request = request.into_inner();
        check_versions(LINK_VERSION, request.link_version, "instance").map_err(Status::failed_precondition)?;
        if !self.server.pair_allowed(self.conn.remote.ip()) {
            return Err(Status::resource_exhausted(
                "too many pairing attempts from this address; wait",
            ));
        }
        let fingerprint = self.conn.pairing_key.clone().unwrap_or_default();
        let secret = zeroize::Zeroizing::new(request.secret);
        let bundle_key = request.bundle_key;
        let paired = self
            .server
            .on_pool(move |facade| moqui_instance::link::pair(facade, &secret, &fingerprint, &bundle_key))
            .await?;
        // a link of the key it replaces, if one is up, ends now; the new key may link from wherever it is
        self.server.repaired(&paired.app_instance_id);
        Ok(Response::new(PairReply {
            app_instance_id: paired.app_instance_id,
            environment_id: paired.environment_id,
            approvers: Some(set_to_proto(&paired.approvers)),
            manager_id: self.server.manager_id.clone(),
        }))
    }

    async fn hello(&self, request: Request<HelloRequest>) -> Result<Response<HelloReply>, Status> {
        refuse(Call::Hello, &self.conn)?;
        let status = request.into_inner().status.unwrap_or_default();
        check_versions(LINK_VERSION, status.link_version, "instance").map_err(Status::failed_precondition)?;
        // the key again: a revoke or a pairing again between this connection's start and its Hello holds
        let (lookup, instance) = (self.conn.fingerprint.clone(), self.conn.instance.clone());
        let pinned = self
            .server
            .on_pool(move |facade| {
                Ok(moqui_instance::link::linked(facade, &lookup)?
                    .is_some_and(|linked| linked.app_instance_id == instance))
            })
            .await?;
        if !pinned {
            return Err(Status::permission_denied(NOT_PAIRED));
        }
        if let Err(why) = self.server.admit(&self.conn.instance, self.conn.remote) {
            tracing::error!(
                "Manager link of AppInstance {} from {} refused: {why}",
                self.conn.instance,
                self.conn.remote
            );
            self.server.tell_move(&self.conn.instance, self.conn.remote, &why).await;
            return Err(Status::permission_denied(why));
        }
        self.conn
            .sealed
            .store(status.seal_state != "unsealed", Ordering::Release);
        self.server.register(&self.conn, status.clone())?;
        self.conn.helloed.store(true, Ordering::Release);
        self.note_seal(&status.seal_state).await?;
        let instance = self.conn.instance.clone();
        let members = status.cluster_members.clone();
        let schema_hash = status.schema_hash.clone();
        self.server
            .on_pool(move |facade| {
                // its cluster's live members, which Monitor compares (plans/archive/Vault-Instances.md §4.8)
                moqui_instance::monitor::note_members(facade, &instance, &members);
                // the schema a deploy records before it and a rollback compares (M3)
                moqui_instance::link::note_schema(facade, &instance, &schema_hash);
                Ok(())
            })
            .await?;
        tracing::info!(
            "Manager link of AppInstance {} is up (generation {}, {}, from {})",
            self.conn.instance,
            self.conn.generation,
            status.seal_state,
            self.conn.remote
        );
        // the public half of its transit key, made at its first link (M6), when its environment's method is the
        // manager's transit: a manager that cannot keep one (no seal of its own) says so, and the instance's transit
        // init fails with that
        let instance = self.conn.instance.clone();
        let transit_public = match self
            .server
            .on_pool(move |facade| {
                let row = moqui_instance::link::instance(facade, &instance)?;
                if moqui_instance::transit::environment_method(facade, &row)? != "transit" {
                    return Ok(Vec::new());
                }
                moqui_instance::transit::transit_public(facade, &instance).map(|public| public.to_vec())
            })
            .await
        {
            Ok(public) => public,
            Err(status) => {
                tracing::error!(
                    "Manager link: no transit key for AppInstance {}: {}",
                    self.conn.instance,
                    status.message()
                );
                Vec::new()
            }
        };
        Ok(Response::new(HelloReply {
            link_version: LINK_VERSION,
            manager_id: self.server.manager_id.clone(),
            app_instance_id: self.conn.instance.clone(),
            generation: self.conn.generation,
            bootstrap_ready: moqui_instance::link::bundle_ready(&self.conn.instance),
            transit_public,
            challenge: self.conn.challenge.clone(),
        }))
    }

    async fn status(&self, request: Request<InstanceStatus>) -> Result<Response<StatusReply>, Status> {
        refuse(Call::Status, &self.conn)?;
        let status = request.into_inner();
        let sealed = status.seal_state != "unsealed";
        self.conn.sealed.store(sealed, Ordering::Release);
        // "unsealed" as the manager takes it (M6): a transit unseal on this link stands until a sealed status; else the
        // instance's proof on this link, checked against the key pinned for it
        if sealed {
            self.server.set_verified(&self.conn, false);
        } else if !self.conn.verified.load(Ordering::Acquire) && !status.proof.is_empty() {
            let (instance, fingerprint, generation, challenge) = (
                self.conn.instance.clone(),
                self.conn.fingerprint.clone(),
                self.conn.generation,
                self.conn.challenge.clone(),
            );
            let (proof_public, proof) = (status.proof_public.clone(), status.proof.clone());
            let proved = self
                .server
                .on_pool(move |facade| {
                    moqui_instance::transit::check_proof(
                        facade,
                        &moqui_vault::managed::ProofLink {
                            instance_id: &instance,
                            link_fingerprint: &fingerprint,
                            generation,
                            challenge: &challenge,
                        },
                        &proof_public,
                        &proof,
                    )
                })
                .await
                .unwrap_or(false);
            if proved {
                self.server.set_verified(&self.conn, true);
            }
        }
        let members_changed = self
            .server
            .note_status(&self.conn.instance, self.conn.generation, &status);
        if members_changed {
            // what Monitor shows of its cluster: a write that fails is logged, and the heartbeat goes on
            let (instance, members) = (self.conn.instance.clone(), status.cluster_members.clone());
            let written = self
                .server
                .on_pool(move |facade| {
                    moqui_instance::monitor::note_members(facade, &instance, &members);
                    Ok(())
                })
                .await;
            if let Err(status) = written {
                tracing::error!(
                    "Manager link: the cluster members of AppInstance {} were not written: {}",
                    self.conn.instance,
                    status.message()
                );
            }
        }
        if !status.schema_hash.is_empty() {
            // the schema a deploy records before it and a rollback compares (M3): a write that fails is logged
            let (instance, schema_hash) = (self.conn.instance.clone(), status.schema_hash.clone());
            let written = self
                .server
                .on_pool(move |facade| {
                    moqui_instance::link::note_schema(facade, &instance, &schema_hash);
                    Ok(())
                })
                .await;
            if let Err(status) = written {
                tracing::error!(
                    "Manager link: the schema hash of AppInstance {} was not written: {}",
                    self.conn.instance,
                    status.message()
                );
            }
        }
        self.note_seal(&status.seal_state).await?;
        Ok(Response::new(StatusReply {
            unsealed_verified: self.conn.verified.load(Ordering::Acquire),
        }))
    }

    async fn bootstrap(&self, request: Request<BootstrapRequest>) -> Result<Response<BootstrapReply>, Status> {
        refuse(Call::Bootstrap, &self.conn)?;
        let _ = request;
        let Some(values) = moqui_instance::link::take_bundle(&self.conn.instance) else {
            return Ok(Response::new(BootstrapReply { sealed: Vec::new() }));
        };
        let sealed =
            super::bundle::seal(&self.conn.bundle_key, &self.conn.instance, &values).map_err(Status::internal)?;
        let instance = self.conn.instance.clone();
        self.server
            .on_pool(move |facade| moqui_instance::link::note_bootstrap(facade, &instance))
            .await?;
        tracing::info!(
            "Manager link: AppInstance {} took its bootstrap bundle",
            self.conn.instance
        );
        Ok(Response::new(BootstrapReply { sealed }))
    }

    async fn unseal(&self, request: Request<UnsealRequest>) -> Result<Response<UnsealReply>, Status> {
        refuse(Call::Unseal, &self.conn)?;
        let request = request.into_inner();
        let (instance, fingerprint, remote, generation) = (
            self.conn.instance.clone(),
            self.conn.fingerprint.clone(),
            self.conn.remote.ip().to_string(),
            self.conn.generation,
        );
        let answer = self
            .server
            .on_pool(move |facade| {
                moqui_instance::transit::unseal(
                    facade,
                    &moqui_instance::transit::Ask {
                        instance_id: &instance,
                        link_fingerprint: &fingerprint,
                        remote: &remote,
                        generation,
                        wrapped_root: &request.wrapped_root,
                        ephemeral_public: &request.ephemeral_public,
                    },
                )
            })
            .await?;
        match answer {
            moqui_instance::transit::Answer::Sealed { sealed_root, .. } => {
                // this manager unsealed it on this link: its "unsealed" here is taken
                self.server.set_verified(&self.conn, true);
                // transit::unseal wrote the unseal to the row (Monitor's unseal event), and the relay went
                *lock(&self.conn.noted) = "unsealed".to_string();
                let instance = self.conn.instance.clone();
                self.server
                    .on_pool(move |facade| {
                        moqui_instance::shares::drop_relay(facade, &instance);
                        Ok(())
                    })
                    .await?;
                Ok(Response::new(UnsealReply {
                    sealed_root,
                    waiting: String::new(),
                }))
            }
            moqui_instance::transit::Answer::Waiting(why) => Ok(Response::new(UnsealReply {
                sealed_root: Vec::new(),
                waiting: why,
            })),
        }
    }

    async fn sealed_shares(
        &self,
        request: Request<SealedSharesRequest>,
    ) -> Result<Response<SealedSharesReply>, Status> {
        refuse(Call::SealedShares, &self.conn)?;
        let request = request.into_inner();
        let instance = self.conn.instance.clone();
        let shares = self
            .server
            .on_pool(move |facade| {
                let unseal_public: [u8; 32] = request
                    .unseal_public
                    .as_slice()
                    .try_into()
                    .map_err(|_| moqui_lib::error::Error::new("the unseal key is an X25519 key"))?;
                let key = moqui_vault::managed::UnsealKey {
                    instance_id: instance.clone(),
                    unseal_public,
                    expires: request.expires,
                    signature: request.signature,
                };
                moqui_instance::shares::offer_unseal_key(facade, &instance, &request.link_public, &key)
            })
            .await?;
        Ok(Response::new(SealedSharesReply {
            shares: shares
                .into_iter()
                .map(|(holder_user_id, sealed)| SealedShare { sealed, holder_user_id })
                .collect(),
        }))
    }

    async fn holders(&self, request: Request<HoldersRequest>) -> Result<Response<HoldersReply>, Status> {
        refuse(Call::Holders, &self.conn)?;
        self.require_verified(Call::Holders)?;
        let _ = request;
        let instance = self.conn.instance.clone();
        let holders = self
            .server
            .on_pool(move |facade| moqui_instance::shares::holders(facade, &instance))
            .await?;
        Ok(Response::new(HoldersReply {
            holders: holders
                .into_iter()
                .map(|holder| super::proto::Holder {
                    user_id: holder.user_id,
                    personal_key_id: holder.personal_key_id,
                    encrypt_public: holder.encrypt_public.to_vec(),
                    verify_public: holder.verify_public.to_vec(),
                    checks: holder
                        .checks
                        .into_iter()
                        .map(|check| super::proto::KeyCheck {
                            checker_user_id: check.checker_user_id,
                            checker_key_id: check.checker_key_id,
                            checked_key_id: check.checked_key_id,
                            checked_user_id: check.checked_user_id,
                            checked_encrypt: check.checked_encrypt,
                            checked_verify: check.checked_verify,
                            checked_at: check.checked_at,
                            signature: check.signature,
                        })
                        .collect(),
                })
                .collect(),
        }))
    }

    async fn shares(&self, request: Request<SharesRequest>) -> Result<Response<SharesReply>, Status> {
        refuse(Call::Shares, &self.conn)?;
        self.require_verified(Call::Shares)?;
        let request = request.into_inner();
        let instance = self.conn.instance.clone();
        let sent: Vec<moqui_instance::shares::SentShare> = request
            .deliveries
            .into_iter()
            .map(|delivery| moqui_instance::shares::SentShare {
                index: delivery.index,
                delivery: moqui_vault::delivery::Delivery {
                    delivery_id: delivery.delivery_id,
                    recipient_user_id: delivery.recipient_user_id,
                    recipient_key_id: delivery.recipient_key_id,
                    purpose: delivery.purpose,
                    sender_kind: "instance".to_string(),
                    sender_id: instance.clone(),
                    sealed: delivery.sealed,
                    signature: delivery.signature,
                },
            })
            .collect();
        let (set_id, threshold, link_public) = (request.set_id, request.threshold, request.link_public);
        self.server
            .on_pool(move |facade| {
                moqui_instance::shares::record_shares(facade, &instance, &link_public, &set_id, threshold, &sent)
            })
            .await?;
        Ok(Response::new(SharesReply {}))
    }

    async fn rotate_proof(
        &self,
        request: Request<super::proto::RotateProofRequest>,
    ) -> Result<Response<super::proto::RotateProofReply>, Status> {
        refuse(Call::RotateProof, &self.conn)?;
        let request = request.into_inner();
        let (instance, fingerprint) = (self.conn.instance.clone(), self.conn.fingerprint.clone());
        self.server
            .on_pool(move |facade| {
                moqui_instance::transit::rotate_proof(
                    facade,
                    &instance,
                    &fingerprint,
                    &request.new_public,
                    &request.wrapped_root,
                    &request.signature,
                )
            })
            .await?;
        Ok(Response::new(super::proto::RotateProofReply {}))
    }

    async fn share_confirmations(
        &self,
        request: Request<ShareConfirmationsRequest>,
    ) -> Result<Response<ShareConfirmationsReply>, Status> {
        refuse(Call::ShareConfirmations, &self.conn)?;
        self.require_verified(Call::ShareConfirmations)?;
        let set_id = request.into_inner().set_id;
        let instance = self.conn.instance.clone();
        let confirmations = self
            .server
            .on_pool(move |facade| moqui_instance::shares::confirmations(facade, &instance, &set_id))
            .await?;
        Ok(Response::new(ShareConfirmationsReply {
            confirmations: confirmations
                .into_iter()
                .map(|confirmation| ShareConfirmation {
                    index: confirmation.index,
                    holder_user_id: confirmation.holder_user_id,
                    signature: confirmation.signature,
                })
                .collect(),
        }))
    }

    async fn event(&self, request: Request<EventRequest>) -> Result<Response<EventReply>, Status> {
        refuse(Call::Event, &self.conn)?;
        let event = request.into_inner();
        if !matches!(event.kind.as_str(), "started" | "sealed" | "unsealed" | "failed") {
            return Err(Status::invalid_argument(
                "an event is started, sealed, unsealed or failed",
            ));
        }
        // the instance's own text: cut, and escaped so it adds no line or control character to the log
        let message: String = event.message.chars().take(MAX_EVENT_TEXT).collect();
        tracing::info!(
            "Manager link: AppInstance {} {}{}",
            self.conn.instance,
            event.kind,
            if message.is_empty() {
                String::new()
            } else {
                format!(": {}", message.escape_debug())
            }
        );
        Ok(Response::new(EventReply {}))
    }

    type ServeStream = CommandStream;

    async fn serve(&self, request: Request<Streaming<CommandReply>>) -> Result<Response<Self::ServeStream>, Status> {
        refuse(Call::Serve, &self.conn)?;
        let mut replies = request.into_inner();
        let (sender, commands) = tokio::sync::mpsc::unbounded_channel::<Command>();
        let waiting = {
            let mut linked = lock(&self.server.linked);
            let link = linked
                .get_mut(&self.conn.instance)
                .filter(|link| link.generation == self.conn.generation)
                .ok_or_else(|| Status::failed_precondition("a newer link of this instance is up"))?;
            link.commands = Some(sender);
            Arc::clone(&link.waiting)
        };
        let (server, instance, generation) = (
            Arc::clone(&self.server),
            self.conn.instance.clone(),
            self.conn.generation,
        );
        tokio::spawn(async move {
            while let Ok(Some(reply)) = replies.message().await {
                let sealed = reply
                    .status
                    .as_ref()
                    .is_some_and(|status| status.seal_state != "unsealed");
                if let Some(status) = &reply.status {
                    server.note_status(&instance, generation, status);
                }
                if let Some(waiter) = lock(&waiting).remove(&reply.id) {
                    let _ = waiter.try_send(reply);
                }
                // sealed (by this command or its operator): no other command runs, so the stream ends
                if sealed {
                    break;
                }
            }
            if let Some(link) = lock(&server.linked)
                .get_mut(&instance)
                .filter(|link| link.generation == generation)
            {
                link.commands = None;
            }
        });
        if self.conn.verified.load(Ordering::Acquire) {
            Arc::clone(&self.server).deposit_bundle(&self.conn.instance);
        }
        let stream = futures_util::stream::unfold(commands, |mut commands| async move {
            commands.recv().await.map(|command| (Ok(command), commands))
        });
        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
#[path = "accept_tests.rs"]
mod tests;
