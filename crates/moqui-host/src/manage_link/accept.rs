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

//! The manager's end of the manager link (plans/Vault-Instances.md §4.4), with the `instances` feature: the HTTPS
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
//! first, and while its last Hello or Status said sealed only Hello, Status, Bootstrap and Unseal are answered (Unseal
//! "not built" until M6).
//!
//! **A link from a new address.** Until M6 ties "unsealed" to the transit unseal, the manager's view of the instance's
//! seal is the instance's own claim, so whoever holds its link key can claim unsealed and be sent a put-secret's value
//! or a deploy's attestation. So a new link of an instance from another address than its live link's, or than its last
//! link's within `manage@address-change-wait-seconds` of that link going down, is refused and told to the
//! administrators, until an operator confirms the move (`confirm#InstanceLinkAddress`) or the wait passes; a pairing
//! confirms it too. A thief at the same address, or after the wait, is not stopped by this ([`admit_address`]).

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
    InstanceStatus, PairReply, PairRequest, StatusReply, UnsealReply, UnsealRequest,
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
    Some(Arc::new(Acceptor {
        tls: Arc::new(tls),
        server,
    }))
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

    /// Serve a link connection until it closes.
    pub(crate) async fn serve(&self, stream: TlsStream<TcpStream>, remote: SocketAddr) {
        Arc::clone(&self.server).serve(stream, remote).await;
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

    async fn serve(self: Arc<Self>, mut stream: TlsStream<TcpStream>, remote: SocketAddr) {
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
                });
                let lifetime = self.config.pairing_lifetime;
                self.serve_h2(stream, conn, None, Some(lifetime)).await;
            }
            ROLE_UP => self.serve_up(stream, remote, fingerprint).await,
            _ => {
                let _ = write_answer(&mut stream, Answer::Failed, "unknown role").await;
            }
        }
    }

    async fn serve_up(self: Arc<Self>, mut stream: TlsStream<TcpStream>, remote: SocketAddr, fingerprint: String) {
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
        });
        let deadline = self.config.handshake_timeout;
        Arc::clone(&self)
            .serve_h2(stream, Arc::clone(&conn), Some((closed, deadline)), None)
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
    /// in), the listener stops, or a pairing connection's time is up.
    async fn serve_h2(
        self: Arc<Self>,
        stream: TlsStream<TcpStream>,
        conn: Arc<Conn>,
        closed: Option<(watch::Receiver<bool>, Duration)>,
        lifetime: Option<Duration>,
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
            })
            .unwrap_or_default()
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
    /// is sealed only status is answered, from what it last said.
    pub fn call(&self, instance: &str, body: super::proto::command::Body) -> Result<CommandReply, String> {
        let command = Command {
            id: 0,
            body: Some(body),
        };
        let kind = CommandKind::of(&command).ok_or("not a command of the manager link")?;
        let (sender, waiting, status) = {
            let linked = lock(&self.linked);
            let link = linked
                .get(instance)
                .ok_or_else(|| format!("AppInstance {instance} is not linked"))?;
            (link.commands.clone(), Arc::clone(&link.waiting), link.status.clone())
        };
        let sealed = status.seal_state != "unsealed";
        if kind == CommandKind::Status && (sealed || sender.is_none()) {
            return Ok(CommandReply {
                id: 0,
                ok: true,
                error: String::new(),
                status: Some(status),
                text: String::new(),
            });
        }
        if sealed && !kind.sealed_ok() {
            return Err(format!(
                "AppInstance {instance} is sealed: {} waits until it is unsealed",
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
        reply.map_err(|_| format!("AppInstance {instance} did not answer {} in time", kind.name()))
    }
}

type CommandStream = BoxPin<Box<dyn Stream<Item = Result<Command, Status>> + Send>>;

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
        let (instance, remote) = (self.conn.instance.clone(), self.conn.remote.ip().to_string());
        let (seal_state, members) = (status.seal_state.clone(), status.cluster_members.clone());
        self.server
            .on_pool(move |facade| {
                moqui_instance::link::note_state(facade, &instance, "linked", Some(&seal_state), &remote);
                // its cluster's live members, which Monitor compares (plans/Vault-Instances.md §4.8)
                moqui_instance::monitor::note_members(facade, &instance, &members);
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
        Ok(Response::new(HelloReply {
            link_version: LINK_VERSION,
            manager_id: self.server.manager_id.clone(),
            app_instance_id: self.conn.instance.clone(),
            generation: self.conn.generation,
            bootstrap_ready: moqui_instance::link::bundle_ready(&self.conn.instance),
        }))
    }

    async fn status(&self, request: Request<InstanceStatus>) -> Result<Response<StatusReply>, Status> {
        refuse(Call::Status, &self.conn)?;
        let status = request.into_inner();
        let sealed = status.seal_state != "unsealed";
        let was = self.conn.sealed.swap(sealed, Ordering::AcqRel);
        let members_changed = self
            .server
            .note_status(&self.conn.instance, self.conn.generation, &status);
        if members_changed {
            let (instance, members) = (self.conn.instance.clone(), status.cluster_members.clone());
            self.server
                .on_pool(move |facade| {
                    moqui_instance::monitor::note_members(facade, &instance, &members);
                    Ok(())
                })
                .await?;
        }
        if was != sealed {
            let instance = self.conn.instance.clone();
            let seal_state = status.seal_state.clone();
            tracing::info!("Manager link: AppInstance {instance} is {seal_state}");
            self.server
                .on_pool(move |facade| {
                    moqui_instance::link::note_state(facade, &instance, "linked", Some(&seal_state), "");
                    Ok(())
                })
                .await?;
        }
        Ok(Response::new(StatusReply {}))
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
        let _ = request;
        Err(Status::unimplemented(
            "unseal by transit over the manager link is not built yet (M6)",
        ))
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
        let stream = futures_util::stream::unfold(commands, |mut commands| async move {
            commands.recv().await.map(|command| (Ok(command), commands))
        });
        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
#[path = "accept_tests.rs"]
mod tests;
