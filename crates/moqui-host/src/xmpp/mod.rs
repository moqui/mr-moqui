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

//! XMPP for local users (plans/Comms-Framework.md §4.3, C3a; docs/interfaces/XMPP.md): the host's side of
//! [`moqui_xmpp`]. Off while `xmpp_domain` is empty.
//!
//! - **Listeners.** `xmpp_port` (STARTTLS) on `xmpp_host` and `xmpp_tls_port` (direct TLS, ALPN `xmpp-client`) on
//!   `xmpp_tls_host`, each off while empty, on a small Tokio runtime of their own; the certificates from `runtime/cert`
//!   by the name the client asks for ([`crate::tls::CertStore`]: the domain, `rooms.` and `upload.`, else `default`).
//!   At most [`PER_SOURCE`] connections from one address at once and [`MAX_CONNECTIONS`] in all, WebSockets included.
//!   `/xmpp-websocket` (RFC 7395) on the HTTP listener
//!   when `xmpp_websocket_enabled` ([`ws`]), and `/xmpp-upload/` for files ([`upload`]).
//! - **Settings.** External services (XEP-0215) hand out TURN credentials only with `xmpp_turn_enabled`; the SCRAM salts
//!   of unknown users derive from `xmpp_scram_decoy_key` (generated when unset), so they stay the same across restarts.
//! - **The backend.** Each service the session names runs on a request worker ([`crate::worker`]): as the signed-in
//!   user, logged in by username (`internal_login_user`, which refuses a disabled account), or as internal work.
//! - **Pushes.** A subscription to the notification bus's `notification-message` topic, as `/notws` has, hands every
//!   chat push to the session registry, which keeps the ones for its users.

pub(crate) mod upload;
pub(crate) mod ws;

use std::collections::{BTreeMap, HashMap};
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

/// Connections from one address at once.
const PER_SOURCE: usize = 20;
/// Connections at once in all (TCP and WebSocket).
const MAX_CONNECTIONS: usize = 10_000;

/// Open connections by source address.
type Sources = Arc<Mutex<HashMap<IpAddr, usize>>>;

/// The running server of this process, for the HTTP routes and the tests.
#[derive(Default)]
pub struct XmppSlot {
    pub server: Mutex<Option<Arc<Server>>>,
    /// The STARTTLS and direct TLS listeners' addresses.
    pub addresses: Mutex<(Option<SocketAddr>, Option<SocketAddr>)>,
    /// The open connections by source, TCP and WebSocket.
    sources: Sources,
}

/// A connection counted against its source and the total; dropping it gives the place back.
pub(crate) struct Admitted {
    sources: Sources,
    ip: IpAddr,
}

impl Drop for Admitted {
    fn drop(&mut self) {
        let mut open = lock(&self.sources);
        if let Some(count) = open.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                open.remove(&self.ip);
            }
        }
    }
}

/// A place for a connection from `ip`; None when that source has [`PER_SOURCE`] open or the server
/// [`MAX_CONNECTIONS`].
fn admit(sources: &Sources, ip: IpAddr) -> Option<Admitted> {
    let mut open = lock(sources);
    let total: usize = open.values().sum();
    let count = open.entry(ip).or_insert(0);
    if *count >= PER_SOURCE || total >= MAX_CONNECTIONS {
        if *count == 0 {
            open.remove(&ip);
        }
        return None;
    }
    *count += 1;
    Some(Admitted {
        sources: Arc::clone(sources),
        ip,
    })
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
            *lock(&factory.extension(|_| XmppSlot::default()).server) = None;
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
        app.route(ws::PATH, axum::routing::get(ws::upgrade))
    } else {
        app
    }
}

/// Start the listeners and the subscription when `xmpp_domain` is set.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) -> Option<XmppNode> {
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
    let sources = Arc::clone(&slot.sources);
    let mut addresses = (None, None);
    for (port_name, host_name, direct) in [
        ("xmpp_port", "xmpp_host", false),
        ("xmpp_tls_port", "xmpp_tls_host", true),
    ] {
        let port = property(port_name);
        if port.is_empty() {
            continue;
        }
        let Ok(port) = port.parse::<u16>() else {
            tracing::error!("XMPP: {port_name} {port} is not a port; that listener is off");
            continue;
        };
        let host = Some(property(host_name))
            .filter(|host| !host.is_empty())
            .unwrap_or_else(|| property("xmpp_host"));
        let host = if host.is_empty() { "0.0.0.0".to_string() } else { host };
        let acceptor = match tls_acceptor(&runtime_dir, direct) {
            Ok(acceptor) => acceptor,
            Err(err) => {
                tracing::error!("XMPP: {port_name} is off: {err}");
                continue;
            }
        };
        let bind = match (host.as_str(), port).to_socket_addrs().map(|mut found| found.next()) {
            Ok(Some(bind)) => bind,
            _ => {
                tracing::error!("XMPP: {host_name} {host} does not resolve; {port_name} is off");
                continue;
            }
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
        tracing::info!(
            "XMPP {} listening on {}",
            if direct { "direct TLS" } else { "STARTTLS" },
            bind
        );
        runtime.spawn(accept_loop(
            listener,
            Arc::clone(&server),
            acceptor,
            direct,
            Arc::clone(&sources),
        ));
    }
    *lock(&slot.addresses) = addresses;
    tracing::info!("XMPP for {} started", server.config.domain);
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

async fn accept_loop(
    listener: TcpListener,
    server: Arc<Server>,
    acceptor: TlsAcceptor,
    direct: bool,
    sources: Sources,
) {
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
        tokio::spawn(async move {
            moqui_xmpp::conn::serve_tcp(server, stream, peer, acceptor, direct).await;
            drop(admitted);
        });
    }
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
