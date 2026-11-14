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

//! XMPP for local users (plans/archive/Comms-Framework.md §4.3, C3a; docs/interfaces/XMPP.md): the host's side of
//! [`moqui_xmpp`]. Off while `xmpp_domain` is empty.
//!
//! - **Listeners.** `xmpp_port` (STARTTLS) on `xmpp_host` and `xmpp_tls_port` (direct TLS, ALPN `xmpp-client`) on
//!   `xmpp_tls_host`, each off while empty, on a small Tokio runtime of their own; the certificates from `runtime/cert`
//!   by the name the client asks for ([`crate::tls::CertStore`]: the domain, `rooms.` and `upload.`, else `default`).
//!   At most [`PER_SOURCE`] connections from one source at once (an IPv6 /64 is one, an IPv4-mapped address is its IPv4
//!   one, [`moqui_lib::source_guard::source_key`]) and [`MAX_CONNECTIONS`] in all, WebSockets included.
//!   `/xmpp-websocket` (RFC 7395) on the HTTP listener
//!   when `xmpp_websocket_enabled` ([`ws`]), and `/xmpp-upload/` for files ([`upload`]).
//! - **Settings.** External services (XEP-0215) hand out TURN credentials only with `xmpp_turn_enabled`; the SCRAM salts
//!   of unknown users derive from `xmpp_scram_decoy_key` (generated when unset), so they stay the same across restarts.
//! - **The backend.** Each service the session names runs on a request worker ([`crate::worker`]): as the signed-in
//!   user, logged in by username (`internal_login_user`, which refuses a disabled account), or as internal work.
//! - **Pushes.** A subscription to the notification bus's `notification-message` topic, as `/notws` has, hands every
//!   chat push to the session registry, which keeps the ones for its users.
//! - **Behind an edge node** (plans/archive/Edge-Node.md §5.4, E9). An edge node starts no XMPP server of its own; its tunnel
//!   ports (`edge_xmpp_port`, `edge_xmpp_tls_port`) pipe each client down the link, and [`serve_tunneled`] serves the
//!   stream here as a connection from the client the edge node names, with this node's certificates and TLS, so
//!   channel binding keeps its exporter. This node needs no listener of its own for that.
//! - **On an all-in-one node** (plans/Edge-Node2.md §4.1, E16) its own client ports serve its pairings' domains too:
//!   a connection that names a paired node's XMPP domain (the TLS server name, the stream header's `to`) goes down
//!   that node's link (`edge_link::tunnel::xmpp_to_pairing`), and the rest are served here, so one pair of ports
//!   takes every domain.

pub(crate) mod upload;
pub(crate) mod ws;

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::{Error as CoreError, ExecutionContext, ExecutionContextFactory};
use moqui_core::notify::NOTIFICATION_MESSAGE;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use moqui_xmpp::backend::{Backend, BoxFuture, CallError, Caller};
use moqui_xmpp::{Config, Server};
use serde_json::Value as Json;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Connections from one source (an IPv4 address, an IPv6 /64) at once.
const PER_SOURCE: usize = 20;
/// Connections at once in all (TCP and WebSocket).
const MAX_CONNECTIONS: usize = 10_000;

/// Open connections by source, against [`PER_SOURCE`] and [`MAX_CONNECTIONS`].
type Sources = moqui_lib::source_guard::SourceSlots;

/// The running server of this process, for the HTTP routes and the tests.
pub struct XmppSlot {
    pub server: Mutex<Option<Arc<Server>>>,
    /// The STARTTLS and direct TLS listeners' addresses.
    pub addresses: Mutex<(Option<SocketAddr>, Option<SocketAddr>)>,
    /// The open connections by source, TCP, WebSocket and tunneled.
    sources: Sources,
    /// What a stream tunneled from an edge node is served with, while the server runs.
    intake: Mutex<Option<Intake>>,
}

impl Default for XmppSlot {
    fn default() -> Self {
        XmppSlot {
            server: Mutex::default(),
            addresses: Mutex::default(),
            sources: Sources::new(MAX_CONNECTIONS, PER_SOURCE),
            intake: Mutex::default(),
        }
    }
}

/// The TLS settings of both kinds of client connection and the server's runtime, so a tunneled stream is served as a
/// listener's connection would be. Each kind's TLS is made when first needed, by a listener at start or by the first
/// tunnel, so a node that serves only the WebSocket never reads `runtime/cert` for it.
#[derive(Clone)]
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
struct Intake {
    runtime_dir: Arc<std::path::Path>,
    starttls: Arc<std::sync::OnceLock<Option<TlsAcceptor>>>,
    direct: Arc<std::sync::OnceLock<Option<TlsAcceptor>>>,
    runtime: tokio::runtime::Handle,
}

impl Intake {
    /// The TLS of `direct` or STARTTLS connections, made the first time; None, logged once, when it cannot be.
    fn acceptor(&self, direct: bool) -> Option<TlsAcceptor> {
        let cell = if direct { &self.direct } else { &self.starttls };
        cell.get_or_init(|| match tls_acceptor(&self.runtime_dir, direct) {
            Ok(acceptor) => Some(acceptor),
            Err(err) => {
                let kind = if direct { "direct TLS" } else { "STARTTLS" };
                tracing::error!(
                    "XMPP: {kind} connections are off, on this node's port and through an edge node: {err}"
                );
                None
            }
        })
        .clone()
    }
}

/// A connection counted against its source and the total; dropping it gives the place back.
pub(crate) type Admitted = moqui_lib::source_guard::SourceSlot;

/// A place for a connection from `ip`; None when its source (an IPv6 /64) has [`PER_SOURCE`] open or the server
/// [`MAX_CONNECTIONS`].
fn admit(sources: &Sources, ip: IpAddr) -> Option<Admitted> {
    sources.take(ip).ok()
}

/// A place for a WebSocket connection from `ip`, counted with the TCP ones.
pub(crate) fn admit_websocket(factory: &ExecutionContextFactory, ip: IpAddr) -> Option<Admitted> {
    admit(&factory.extension(|_| XmppSlot::default()).sources, ip)
}

/// The XMPP server of this node; dropping it stops the listeners and the subscription.
pub(crate) struct XmppNode {
    _runtime: tokio::runtime::Runtime,
    _subscription: moqui_core::notify::Subscription,
    factory: Weak<ExecutionContextFactory>,
}

impl Drop for XmppNode {
    fn drop(&mut self) {
        if let Some(factory) = self.factory.upgrade() {
            let slot = factory.extension(|_| XmppSlot::default());
            *lock(&slot.server) = None;
            *lock(&slot.intake) = None;
        }
    }
}

fn property(name: &str) -> String {
    moqui_lib::env::get_prop_or_env(name).trim().to_string()
}

fn number<T: std::str::FromStr>(name: &str, default: T) -> T {
    let text = property(name);
    if text.is_empty() {
        return default;
    }
    text.parse().unwrap_or_else(|_| {
        tracing::error!("XMPP: {name} {text} is not a number, its default is used");
        default
    })
}

/// The server's settings from the properties.
pub(crate) fn config() -> Option<Config> {
    let domain = property("xmpp_domain");
    if domain.is_empty() {
        return None;
    }
    let mut config = Config::for_domain(&domain);
    let rooms = property("xmpp_rooms_domain");
    if !rooms.is_empty() {
        config.rooms_domain = rooms.to_lowercase();
    }
    let upload = property("xmpp_upload_domain");
    if !upload.is_empty() {
        config.upload_domain = upload.to_lowercase();
    }
    let base = property("xmpp_upload_url");
    config.upload_base_url = if base.is_empty() {
        match property("webapp_https_port").as_str() {
            "" | "443" => format!("https://{}", config.domain),
            port => format!("https://{}:{port}", config.domain),
        }
    } else {
        base.trim_end_matches('/').to_string()
    };
    config.upload_max_bytes = number("xmpp_upload_max_bytes", config.upload_max_bytes);
    config.resume_timeout = Duration::from_secs(number("xmpp_resume_seconds", 300_u64).clamp(0, 3600));
    config.fast_token_lifetime = Duration::from_secs(number("xmpp_fast_token_days", 14_u64).clamp(1, 365) * 86_400);
    config.software_version = env!("CARGO_PKG_VERSION").to_string();
    config.turn_enabled = property("xmpp_turn_enabled") == "true";
    let decoy = property("xmpp_scram_decoy_key");
    if decoy.is_empty() || decoy.ends_with("CHANGEME") {
        tracing::warn!("XMPP: xmpp_scram_decoy_key is not set; unknown users' SCRAM salts change with each start");
    } else {
        config.decoy_key = decoy.into_bytes();
    }
    Some(config)
}

/// The running server, when there is one.
pub(crate) fn server_of(factory: &ExecutionContextFactory) -> Option<Arc<Server>> {
    lock(&factory.extension(|_| XmppSlot::default()).server).clone()
}

/// The HTTP routes while the server runs: the upload URLs, and `/xmpp-websocket` with `xmpp_websocket_enabled`.
pub(crate) fn routes(
    app: axum::Router<Arc<crate::http::HttpState>>,
    factory: &ExecutionContextFactory,
) -> axum::Router<Arc<crate::http::HttpState>> {
    if server_of(factory).is_none() {
        return app;
    }
    // a PUT's body is read only after its slot is checked, and never past the slot's size (upload.rs)
    let app = app.route(
        &format!("{}{{token}}/{{name}}", upload::PREFIX),
        axum::routing::any(upload::handle),
    );
    if property("xmpp_websocket_enabled") == "true" {
        tracing::info!("XMPP over WebSocket at {}", ws::PATH);
        app.route(ws::PATH, axum::routing::get(ws::upgrade).connect(ws::upgrade))
    } else {
        app
    }
}

/// Start the listeners and the subscription when `xmpp_domain` is set. Never on a dedicated edge node, whose client
/// ports tunnel to its local node (checked for this factory, as a test runs both ends in one process). An all-in-one
/// node starts its own, on its own ports, beside the tunnel ports of its paired node's domain (plans/archive/Edge-Node.md §10).
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) -> Option<XmppNode> {
    let node = start_node(factory);
    // a start that recorded nothing returned early on an error it logged
    moqui_core::startup::report(factory).record_default(
        moqui_core::startup::Section::Listener,
        LISTENER,
        moqui_core::startup::State::failed("see the XMPP error above"),
    );
    node
}

/// The startup report's name for the XMPP client listeners.
pub(crate) const LISTENER: &str = "XMPP";

/// The client ports: the port property, the host property, and whether the port is direct TLS (else STARTTLS).
const CLIENT_PORTS: [(&str, &str, bool); 2] = [
    ("xmpp_port", "xmpp_host", false),
    ("xmpp_tls_port", "xmpp_tls_host", true),
];

/// Where the client port `port_name` listens: None while it is empty, else its address or why that listener is off.
fn client_address(port_name: &str, host_name: &str) -> Option<Result<SocketAddr, String>> {
    let port = property(port_name);
    if port.is_empty() {
        return None;
    }
    let Ok(port) = port.parse::<u16>() else {
        return Some(Err(format!("{port_name} {port} is not a port; that listener is off")));
    };
    let host = Some(property(host_name))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| property("xmpp_host"));
    let host = if host.is_empty() { "0.0.0.0".to_string() } else { host };
    Some(
        match (host.as_str(), port).to_socket_addrs().map(|mut found| found.next()) {
            Ok(Some(bind)) => Ok(bind),
            _ => Err(format!("{host_name} {host} does not resolve; {port_name} is off")),
        },
    )
}

/// The client ports XMPP will bind, for the port check before the framework is built: none while `xmpp_domain` is
/// empty or on a dedicated edge node (`front`, `front_edge` or `edge`), whose client ports tunnel to its local node.
pub(crate) fn listen_targets() -> Vec<crate::port_check::Target> {
    if property("xmpp_domain").is_empty() || moqui_core::edge::node_role_kind().edge_node() {
        return Vec::new();
    }
    CLIENT_PORTS
        .iter()
        .filter_map(|(port_name, host_name, _)| {
            let bind = client_address(port_name, host_name)?.ok()?;
            Some(crate::port_check::Target::optional(
                LISTENER,
                port_name,
                bind.ip(),
                bind.port(),
                crate::port_check::Proto::Tcp,
            ))
        })
        .collect()
}

fn start_node(factory: &Arc<ExecutionContextFactory>) -> Option<XmppNode> {
    use moqui_core::startup::{listener, State};
    if property("xmpp_domain").is_empty() {
        listener(factory, LISTENER, State::off("xmpp_domain is not set"));
        return None;
    }
    if moqui_core::edge::settings(factory).edge_node() {
        tracing::warn!(
            "XMPP: xmpp_domain is set on a dedicated edge node (node_role {}), which starts no XMPP server: \
             edge_xmpp_port and edge_xmpp_tls_port tunnel clients to the local node",
            moqui_core::edge::settings(factory).role.as_str()
        );
        listener(factory, LISTENER, State::off("an edge node starts no XMPP server"));
        return None;
    }
    if let Some(busy) = crate::port_check::busy(LISTENER) {
        listener(factory, LISTENER, State::Skipped(busy));
        return None;
    }
    let config = config()?;
    // the services read these: the bookmarks' room JIDs, the URLs of files sent to XMPP clients
    moqui_lib::env::set_property("xmpp_rooms_domain", config.rooms_domain.clone());
    moqui_lib::env::set_property("xmpp_upload_url", config.upload_base_url.clone());
    moqui_lib::env::set_property("xmpp_upload_max_bytes", config.upload_max_bytes.to_string());
    let backend: Arc<dyn Backend> = Arc::new(HostBackend {
        factory: Arc::downgrade(factory),
    });
    let server = Server::new(config, backend);
    let pushed = Arc::downgrade(&server);
    let subscription = match moqui_core::notify::subscribe(
        NOTIFICATION_MESSAGE,
        Arc::new(move |message: &moqui_core::notify::TopicMessage| {
            if let Some(server) = pushed.upgrade() {
                server.registry.deliver_notice(&message.body);
            }
        }),
    ) {
        Ok(subscription) => subscription,
        Err(err) => {
            tracing::error!("XMPP not started: the notification bus refused the subscription: {err}");
            return None;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("moqui-xmpp")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!("XMPP not started: its runtime did not start: {err}");
            return None;
        }
    };
    let slot = factory.extension(|_| XmppSlot::default());
    *lock(&slot.server) = Some(Arc::clone(&server));
    let runtime_dir = factory.runtime_dir().to_path_buf();
    let sources = slot.sources.clone();
    // both kinds' TLS, made for a listener here or for the first stream an edge node tunnels here
    let intake = Intake {
        runtime_dir: Arc::from(runtime_dir.as_path()),
        starttls: Arc::default(),
        direct: Arc::default(),
        runtime: runtime.handle().clone(),
    };
    let mut addresses = (None, None);
    let mut opened = Vec::new();
    for (port_name, host_name, direct) in CLIENT_PORTS {
        let bind = match client_address(port_name, host_name) {
            None => continue,
            Some(Ok(bind)) => bind,
            Some(Err(reason)) => {
                tracing::error!("XMPP: {reason}");
                continue;
            }
        };
        let Some(acceptor) = intake.acceptor(direct) else {
            tracing::error!("XMPP: {port_name} is off: it has no TLS settings");
            continue;
        };
        let listener = match runtime.block_on(TcpListener::bind(bind)) {
            Ok(listener) => listener,
            Err(err) => {
                tracing::error!("XMPP: {port_name} could not bind {bind}: {err}");
                continue;
            }
        };
        let local = listener.local_addr().ok();
        if direct {
            addresses.1 = local;
        } else {
            addresses.0 = local;
        }
        opened.push(format!(
            "tcp {} {}",
            local.unwrap_or(bind),
            if direct { "direct TLS" } else { "STARTTLS" }
        ));
        runtime.spawn(accept_loop(
            listener,
            Arc::clone(&server),
            acceptor,
            Listening {
                direct,
                sources: sources.clone(),
                factory: Arc::downgrade(factory),
                routes: moqui_core::edge::settings(factory).all_in_one(),
            },
        ));
    }
    *lock(&slot.addresses) = addresses;
    *lock(&slot.intake) = Some(intake);
    listener(
        factory,
        LISTENER,
        State::On(if opened.is_empty() {
            format!("for {}, no client port", server.config.domain)
        } else {
            format!("{}, for {}", opened.join(", "), server.config.domain)
        }),
    );
    Some(XmppNode {
        _runtime: runtime,
        _subscription: subscription,
        factory: Arc::downgrade(factory),
    })
}

fn tls_acceptor(runtime_dir: &std::path::Path, direct: bool) -> Result<TlsAcceptor, String> {
    let dir = runtime_dir.join("cert");
    if moqui_lib::env::get_prop_or_env("instance_purpose") == "dev" {
        if let Err(err) = crate::tls::ensure_dev_cert(&dir) {
            tracing::warn!("Could not make a development certificate in {}: {err}", dir.display());
        }
    }
    let mut config = crate::tls::server_config(crate::tls::CertStore::new(dir)).map_err(|err| err.to_string())?;
    config.alpn_protocols = if direct {
        vec![b"xmpp-client".to_vec()]
    } else {
        Vec::new()
    };
    Ok(TlsAcceptor::from(Arc::new(config)))
}

/// What a client port's accept loop serves with.
struct Listening {
    /// Direct TLS, else STARTTLS.
    direct: bool,
    sources: Sources,
    #[cfg_attr(not(feature = "edge"), allow(dead_code))]
    factory: Weak<ExecutionContextFactory>,
    /// An all-in-one node's port: a paired node's XMPP domain goes down its link (E16).
    #[cfg_attr(not(feature = "edge"), allow(dead_code))]
    routes: bool,
}

/// The client ports this node's own XMPP server listens on now: STARTTLS and direct TLS, each `None` while off.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn client_ports(factory: &ExecutionContextFactory) -> (Option<u16>, Option<u16>) {
    let (starttls, direct) = *lock(&factory.extension(|_| XmppSlot::default()).addresses);
    (starttls.map(|a| a.port()), direct.map(|a| a.port()))
}

/// How long a client on an all-in-one node's port has to name its XMPP domain.
#[cfg(feature = "edge")]
const DOMAIN_WAIT: Duration = Duration::from_secs(10);

/// One client connection of an all-in-one node's port (E16): down a paired node's link when it names that node's
/// XMPP domain, else served here with what was read for the domain first.
#[cfg(feature = "edge")]
async fn serve_routed(
    server: Arc<Server>,
    stream: tokio::net::TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    listening: &Listening,
) {
    let direct = listening.direct;
    let mut stream = stream;
    let (domain, seen) =
        match tokio::time::timeout(DOMAIN_WAIT, crate::edge_link::xmpp_domain::sniff(&mut stream, direct)).await {
            Ok(Ok(read)) => read,
            Ok(Err(err)) => {
                tracing::debug!("XMPP connection from {peer}: its start did not read: {err}");
                return;
            }
            Err(_) => {
                tracing::debug!("XMPP connection from {peer} named no domain in time");
                return;
            }
        };
    let mut held = Some(stream);
    if crate::edge_link::tunnel::xmpp_to_pairing(&listening.factory, &mut held, peer, domain.as_deref(), &seen, direct)
        .await
    {
        return;
    }
    if let Some(stream) = held {
        let _ = stream.set_nodelay(true);
        let stream = crate::edge_link::xmpp_domain::Prefixed::new(seen, stream);
        moqui_xmpp::conn::serve_stream(server, stream, peer, acceptor, direct).await;
    }
}

async fn accept_loop(listener: TcpListener, server: Arc<Server>, acceptor: TlsAcceptor, listening: Listening) {
    let listening = Arc::new(listening);
    let (direct, sources) = (listening.direct, listening.sources.clone());
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(err) => {
                tracing::debug!("XMPP accept failed: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        if moqui_xmpp::conn::blocked(&server, peer.ip()) {
            continue;
        }
        let Some(admitted) = admit(&sources, peer.ip()) else {
            tracing::debug!("XMPP connection from {peer} refused: too many are open from there or in all");
            continue;
        };
        let (server, acceptor) = (Arc::clone(&server), acceptor.clone());
        let listening = Arc::clone(&listening);
        tokio::spawn(async move {
            #[cfg(feature = "edge")]
            if listening.routes {
                serve_routed(server, stream, peer, acceptor, &listening).await;
                drop(admitted);
                return;
            }
            let _ = &listening;
            moqui_xmpp::conn::serve_tcp(server, stream, peer, acceptor, direct).await;
            drop(admitted);
        });
    }
}

/// Whether this node's XMPP server runs and has the TLS of `direct` (or STARTTLS) connections, to take
/// [`serve_tunneled`] of that kind.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn running(factory: &ExecutionContextFactory, direct: bool) -> bool {
    let intake = lock(&factory.extension(|_| XmppSlot::default()).intake).clone();
    intake.is_some_and(|intake| intake.acceptor(direct).is_some())
}

/// Serve a stream an edge node tunneled here (plans/archive/Edge-Node.md §5.4) as a client connection from `client`, on the
/// server's runtime: STARTTLS first, or TLS at once when `direct`. The client's address is the one the edge node
/// names, so a source blocked for failed sign-ins is refused and the connection is counted against it, as at
/// [`accept_loop`]. False when it was not taken (the server stopped, that kind has no TLS settings, the source is
/// blocked or has too many open); the stream is then dropped, which closes it.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn serve_tunneled<S>(factory: &ExecutionContextFactory, stream: S, client: SocketAddr, direct: bool) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let slot = factory.extension(|_| XmppSlot::default());
    let (Some(server), Some(intake)) = (lock(&slot.server).clone(), lock(&slot.intake).clone()) else {
        return false;
    };
    let Some(acceptor) = intake.acceptor(direct) else {
        return false;
    };
    // as an address on a dual-stack listener: an IPv4-mapped one counts as the IPv4 address
    let client = SocketAddr::new(client.ip().to_canonical(), client.port());
    if moqui_xmpp::conn::blocked(&server, client.ip()) {
        tracing::debug!("XMPP: a tunneled connection from {client} refused: blocked after failed sign-ins");
        return false;
    }
    let Some(admitted) = admit(&slot.sources, client.ip()) else {
        tracing::debug!("XMPP: a tunneled connection from {client} refused: too many are open from there or in all");
        return false;
    };
    intake.runtime.spawn(async move {
        moqui_xmpp::conn::serve_stream(server, stream, client, acceptor, direct).await;
        drop(admitted);
    });
    true
}

/// The session's service calls, on a request worker.
struct HostBackend {
    factory: Weak<ExecutionContextFactory>,
}

impl Backend for HostBackend {
    fn call(&self, caller: Caller, service: &str, parameters: Json) -> BoxFuture<'_, Result<Json, CallError>> {
        let service = service.to_string();
        Box::pin(async move {
            let result =
                crate::worker::on_pool(&self.factory, move |_| run_service(&caller, &service, &parameters)).await;
            match result {
                Ok(Ok(out)) => Ok(out),
                Ok(Err(message)) => Err(CallError(message)),
                Err(err) => Err(CallError(err.message().to_string())),
            }
        })
    }
}

/// Run a service as `caller` in this worker's context: its out-parameters as JSON, or its errors.
fn run_service(caller: &Caller, service: &str, parameters: &Json) -> Result<Result<Json, String>, CoreError> {
    let _authz = match caller {
        Caller::Internal => ExecutionContext::with_current(ExecutionContext::disable_authz),
        Caller::User(username) => {
            if let Err(err) = moqui_core::user::internal_login_user(username) {
                return Ok(Err(format!("{username} may not sign in: {}", err.message())));
            }
            None
        }
    };
    let input: BTreeMap<String, Value> = match moqui_lib::json::from_json(parameters) {
        Value::Map(map) => map
            .into_iter()
            .filter(|(_, value)| !matches!(value, Value::Null))
            .collect(),
        _ => BTreeMap::new(),
    };
    let called = moqui_core::service::sync(service, &input);
    let errors = ExecutionContext::with_current(|ec| {
        let errors = ec.message().errors().join("; ");
        ec.message().clear_errors();
        errors
    })
    .unwrap_or_default();
    let out = match called {
        Ok(out) => out,
        Err(err) => return Ok(Err(err.message().to_string())),
    };
    if !errors.is_empty() {
        return Ok(Err(errors));
    }
    let map: serde_json::Map<String, Json> = out
        .iter()
        .map(|(key, value)| (key.clone(), moqui_lib::json::to_json(value)))
        .collect();
    Ok(Ok(Json::Object(map)))
}

#[cfg(test)]
mod tests;
