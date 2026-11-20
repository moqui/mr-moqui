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

//! The instance's end of the manager link (plans/Vault-Instances.md §4.4): pairing ([`pair`]) and the [`Dialer`],
//! which keeps one link to its manager open, redialing with a backoff. It opens no listener, so it runs while the
//! instance is sealed (Sealed-Secrets §3.3): a server's gate starts it before it waits ([`start_for_gate`]), and keeps
//! the bootstrap bundle it takes on a first start until the seal opens ([`after_unseal`]).
//!
//! On each link: Hello with the instance's status; on a first start the bootstrap bundle; then, while sealed, its
//! status every so often, and once unsealed an event and the Serve stream, down which the manager's commands come
//! ([`super::commands`]). A seal closes the Serve stream; the link goes on with status alone.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;
use tokio::io::AsyncWriteExt as _;
use tokio::net::TcpStream;
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::bundle::Values;
use super::commands::{LinkContext, Managed, ServerManaged};
use super::key::{KeyStore, LinkKey};
use super::proto::{BootstrapRequest, EventRequest, HelloRequest, PairRequest};
use super::state::{set_from_proto, LinkState};
use super::{
    check_versions, manager_client, parse_url, LinkError, ManagerClient, ALPN, LINK_VERSION, ROLE_PAIR, ROLE_UP,
};
use crate::link::{jitter, read_answer, Answer, Backoff};
use crate::tls_pin::{parse_hex, Pin, PinVerifier};

/// The name in the link key's certificate.
const CERT_NAME: &str = "moqui-manage-link";

/// What the instance's end needs: the manager, its pinned key, this instance's link key, where the pairing is kept, a
/// pairing secret when it is not paired yet, and the timings.
#[derive(Clone)]
pub struct ManageConfig {
    /// `https://host[:port]`.
    pub url: String,
    /// SHA-256 of the manager's link key, hex.
    pub pin: String,
    pub key: Arc<LinkKey>,
    /// Where the pairing is kept; none keeps it only for the dialer's life.
    pub state_file: Option<PathBuf>,
    /// The pairing token or code, while not paired.
    pub secret: Option<Arc<Zeroizing<String>>>,
    pub backoff: Backoff,
    /// The waits after the manager refused this instance.
    pub refused_backoff: Backoff,
    /// HTTP/2 PING interval, and how long an unanswered one waits.
    pub keepalive: Duration,
    /// How often the instance's status goes up.
    pub status_every: Duration,
    /// TCP connect and TLS handshake, and each call of the link's own.
    pub connect_timeout: Duration,
}

impl std::fmt::Debug for ManageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManageConfig")
            .field("url", &self.url)
            .field("pin", &self.pin)
            .field("key", &self.key)
            .field("state_file", &self.state_file)
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl ManageConfig {
    /// A link to `url`, pinning `pin`, with the default timings.
    #[must_use]
    pub fn new(url: &str, pin: &str, key: Arc<LinkKey>) -> Self {
        Self {
            url: url.to_string(),
            pin: pin.to_string(),
            key,
            state_file: None,
            secret: None,
            backoff: Backoff::default(),
            refused_backoff: Backoff::refused(),
            keepalive: Duration::from_secs(15),
            status_every: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(10),
        }
    }

    /// The server's: what it pinned (`runtime/db/manage_link.json`), else `manage@url` and `manage@pin` with the
    /// pairing token in `manage@token-file`; none when neither names a manager (it is not managed).
    pub fn from_settings(runtime_dir: &Path, setting: super::Settings<'_>) -> Result<Option<Self>, String> {
        let db = runtime_dir.join("db");
        let state_file = db.join(super::STATE_FILE);
        let state = LinkState::load(&state_file).unwrap_or_default();
        let url = Some(state.url.clone())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| setting("manage@url"));
        if url.is_empty() {
            return Ok(None);
        }
        let pin = Some(state.pin.clone())
            .filter(|pin| !pin.is_empty())
            .unwrap_or_else(|| setting("manage@pin"));
        let key = LinkKey::load_or_create(&db.join(super::KEY_FILE), &KeyStore::from_settings(setting)?, CERT_NAME)?;
        let mut config = Self::new(&url, &pin, Arc::new(key));
        config.state_file = Some(state_file);
        if state.app_instance_id.is_empty() {
            let token_file = setting("manage@token-file");
            if !token_file.is_empty() {
                if let Ok(token) = std::fs::read_to_string(&token_file) {
                    config.secret = Some(Arc::new(Zeroizing::new(token.trim().to_string())));
                }
            }
        }
        Ok(Some(config))
    }
}

/// TLS settings of a connection to the manager: its key pinned, this instance's presented, ALPN `moqui-manage/1`.
fn client_config(config: &ManageConfig) -> Result<Arc<rustls::ClientConfig>, LinkError> {
    let pin = parse_hex(&config.pin).ok_or_else(|| {
        LinkError::Refused("manage@pin is not the SHA-256 fingerprint of the manager's link key".to_string())
    })?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let identity = config.key.identity();
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|err| LinkError::Failed(format!("manager link TLS: {err}")))?
        .dangerous()
        .with_custom_certificate_verifier(PinVerifier::new(Pin::PublicKey(pin), true, &provider))
        .with_client_auth_cert(vec![identity.cert()], identity.key_der())
        .map_err(|err| LinkError::Failed(format!("manager link TLS client certificate: {err}")))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(tls))
}

/// A connection to the manager with `role` sent and answered: its `Manager` client, and the task that drives it.
async fn connect(config: &ManageConfig, role: u8) -> Result<(ManagerClient, tokio::task::JoinHandle<()>), LinkError> {
    let (host, port) = parse_url(&config.url).map_err(LinkError::Refused)?;
    let tls = client_config(config)?;
    let place = format!("{host}:{port}");
    let name = rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|_| LinkError::Refused(format!("{host} is not a host name")))?;
    let connect = async {
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|err| LinkError::Failed(format!("could not connect to the manager at {place}: {err}")))?;
        let _ = tcp.set_nodelay(true);
        tokio_rustls::TlsConnector::from(tls)
            .connect(name, tcp)
            .await
            .map_err(|err| {
                let pinned = err
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<rustls::Error>())
                    .is_some_and(|tls| matches!(tls, rustls::Error::InvalidCertificate(_)));
                if pinned {
                    LinkError::Refused(format!(
                        "the server at {place} presented a link key other than the manager's pinned one"
                    ))
                } else {
                    LinkError::Failed(format!("TLS with the manager at {place} failed: {err}"))
                }
            })
    };
    let mut stream = tokio::time::timeout(config.connect_timeout, connect)
        .await
        .map_err(|_| LinkError::Failed(format!("the manager at {place} did not answer in time")))??;
    if stream.get_ref().1.alpn_protocol() != Some(ALPN) {
        return Err(LinkError::Failed(format!(
            "the server at {place} does not take the manager link (ALPN moqui-manage/1)"
        )));
    }
    let roled = async {
        stream.write_all(&[role]).await?;
        stream.flush().await?;
        read_answer(&mut stream).await
    };
    let (answer, message) = tokio::time::timeout(config.connect_timeout, roled)
        .await
        .map_err(|_| LinkError::Failed("the manager did not answer the link".to_string()))?
        .map_err(|err| LinkError::Failed(format!("the link connection failed: {err}")))?;
    match answer {
        Answer::Ok => {}
        Answer::Refused => return Err(LinkError::Refused(message)),
        _ => return Err(LinkError::Failed(message)),
    }
    let (send, connection) = hyper::client::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
        .timer(hyper_util::rt::TokioTimer::new())
        .keep_alive_interval(config.keepalive)
        .keep_alive_timeout(config.keepalive)
        .keep_alive_while_idle(true)
        .handshake(hyper_util::rt::TokioIo::new(stream))
        .await
        .map_err(|err| LinkError::Failed(format!("HTTP/2 with the manager failed: {err}")))?;
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok((manager_client(send), driver))
}

/// A refusal from the manager's `Status`: refused for a pairing or a key, failed otherwise.
fn call_error(call: &str, status: &tonic::Status) -> LinkError {
    match status.code() {
        tonic::Code::PermissionDenied | tonic::Code::Unauthenticated | tonic::Code::FailedPrecondition => {
            LinkError::Refused(format!("{call}: {}", status.message()))
        }
        _ => LinkError::Failed(format!("{call}: {}", status.message())),
    }
}

/// Pair with the manager by the token or code in `config` (§4.4): it pins this instance's link key and bundle key,
/// and this instance pins its approvers. The pairing is kept in `config.state_file`.
pub async fn pair(config: &ManageConfig) -> Result<LinkState, LinkError> {
    let secret = config
        .secret
        .as_ref()
        .ok_or_else(|| LinkError::Refused("no pairing token or code".to_string()))?;
    let bundle_key = config.key.bundle_public().map_err(LinkError::Failed)?;
    let (mut client, driver) = connect(config, ROLE_PAIR).await?;
    let request = PairRequest {
        link_version: LINK_VERSION,
        secret: secret.as_str().to_string(),
        bundle_key: bundle_key.to_vec(),
        binary_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let answer = tokio::time::timeout(config.connect_timeout, client.pair(request)).await;
    driver.abort();
    let reply = answer
        .map_err(|_| LinkError::Failed("the manager did not answer Pair".to_string()))?
        .map_err(|status| call_error("Pair", &status))?
        .into_inner();
    let mut state = LinkState {
        url: config.url.clone(),
        pin: config.pin.clone(),
        app_instance_id: reply.app_instance_id,
        environment_id: reply.environment_id,
        manager_id: reply.manager_id,
        ..LinkState::default()
    };
    if let Some(approvers) = &reply.approvers {
        state.pin_approvers(&set_from_proto(approvers));
    }
    if let Some(file) = &config.state_file {
        state.save(file).map_err(LinkError::Failed)?;
    }
    tracing::info!(
        "Manager link: paired with {} as AppInstance {} ({} approvers pinned)",
        config.url,
        state.app_instance_id,
        state.approvers.len()
    );
    Ok(state)
}

/// Where the dialer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkPhase {
    /// Not paired, and no token or code to pair with.
    Unpaired,
    Pairing,
    Linking,
    Linked,
    /// Lost; dialing again.
    Down,
    /// The manager refused this instance, or pairing failed for good; waiting longer.
    Refused,
}

impl LinkPhase {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unpaired => "unpaired",
            Self::Pairing => "pairing",
            Self::Linking => "linking",
            Self::Linked => "linked",
            Self::Down => "down",
            Self::Refused => "refused",
        }
    }
}

/// The dialer's state, for a screen and a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStatus {
    pub phase: LinkPhase,
    pub generation: u64,
    pub app_instance_id: String,
    pub message: String,
    /// The Serve stream is open: the manager's commands come down it.
    pub serving: bool,
}

/// The dialer's state shared with its task.
struct Shared {
    status: Mutex<LinkStatus>,
    link: Arc<LinkContext>,
    /// A bootstrap bundle taken while sealed, kept in memory until the seal opens.
    pending: Mutex<Option<Values>>,
}

impl Shared {
    fn set(&self, change: impl FnOnce(&mut LinkStatus)) {
        change(&mut lock(&self.status));
    }
}

/// The instance's dialer: one link to its manager, dialed again when lost.
pub struct Dialer {
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
}

impl Dialer {
    /// Start dialing on `handle` until `shutdown` turns true or [`Dialer::stop`].
    #[must_use]
    pub fn start(
        config: ManageConfig,
        managed: Arc<dyn Managed>,
        handle: &tokio::runtime::Handle,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let state = config
            .state_file
            .as_deref()
            .and_then(LinkState::load)
            .unwrap_or_default();
        let shared = Arc::new(Shared {
            status: Mutex::new(LinkStatus {
                phase: LinkPhase::Linking,
                generation: 0,
                app_instance_id: state.app_instance_id.clone(),
                message: String::new(),
                serving: false,
            }),
            link: LinkContext::new(state, config.state_file.clone(), config.key.fingerprint()),
            pending: Mutex::new(None),
        });
        let (stop, stopped) = watch::channel(false);
        handle.spawn(run(config, managed, Arc::clone(&shared), stopped, shutdown));
        Self { shared, stop }
    }

    #[must_use]
    pub fn status(&self) -> LinkStatus {
        lock(&self.shared.status).clone()
    }

    /// What the instance pinned.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.shared.link.snapshot()
    }

    /// Stop dialing and close the link.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Keep a bootstrap bundle that waits for the seal to open, with `managed`; true when one was kept.
    fn store_pending(&self, managed: &dyn Managed) -> bool {
        store_pending(&self.shared, managed)
    }
}

impl Drop for Dialer {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

fn store_pending(shared: &Shared, managed: &dyn Managed) -> bool {
    let mut pending = lock(&shared.pending);
    let Some(values) = pending.as_ref() else {
        return false;
    };
    match managed.store_bundle(values) {
        Ok(()) => {
            *pending = None;
            drop(pending);
            if let Err(err) = shared.link.update(|state| state.bootstrap_done = true) {
                tracing::error!("Manager link: the pairing state was not written: {err}");
            }
            tracing::info!("Manager link: the bootstrap bundle is kept in the seal");
            true
        }
        Err(err) if managed.sealed() => {
            tracing::debug!("Manager link: the bootstrap bundle waits for the unseal: {err}");
            false
        }
        Err(err) => {
            // a refusal does not change by waiting: the bundle is dropped and the first start is over, so a boot does
            // not wait for it again; its secrets are then put by the manager or `moqui operator secret put`
            tracing::error!("Manager link: the bootstrap bundle was not kept: {err}");
            *pending = None;
            drop(pending);
            if let Err(err) = shared.link.update(|state| state.bootstrap_done = true) {
                tracing::error!("Manager link: the pairing state was not written: {err}");
            }
            false
        }
    }
}

/// Until `stop` turns true (or its sender is gone). Its guard is dropped here, so a select! that waits on this holds
/// none across the awaits of its branches.
async fn until(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stop| *stop).await;
}

/// Wait `wait`, or until a stop: false when stopped.
async fn pause(wait: Duration, stopped: &mut watch::Receiver<bool>, shutdown: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(wait) => true,
        () = until(stopped) => false,
        () = until(shutdown) => false,
    }
}

async fn run(
    config: ManageConfig,
    managed: Arc<dyn Managed>,
    shared: Arc<Shared>,
    mut stopped: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut attempt: u32 = 0;
    let mut refusals: u32 = 0;
    loop {
        if *stopped.borrow() || *shutdown.borrow() {
            return;
        }
        if shared.link.snapshot().app_instance_id.is_empty() {
            if config.secret.is_none() {
                shared.set(|status| {
                    status.phase = LinkPhase::Unpaired;
                    status.message = "not paired: no pairing token (manage@token-file) or code".to_string();
                });
                return;
            }
            shared.set(|status| status.phase = LinkPhase::Pairing);
            match pair(&config).await {
                Ok(state) => {
                    let id = state.app_instance_id.clone();
                    let _ = shared.link.update(|kept| *kept = state);
                    shared.set(|status| {
                        status.app_instance_id = id;
                        status.message.clear();
                    });
                }
                Err(LinkError::Refused(message)) => {
                    // a token or code works once: dialing again with it cannot help
                    tracing::error!("Manager link: pairing refused: {message}");
                    shared.set(|status| {
                        status.phase = LinkPhase::Refused;
                        status.message = message;
                    });
                    return;
                }
                Err(LinkError::Failed(message)) => {
                    tracing::warn!("Manager link: pairing failed: {message}");
                    shared.set(|status| {
                        status.phase = LinkPhase::Down;
                        status.message = message;
                    });
                    attempt = attempt.saturating_add(1);
                    if !pause(config.backoff.delay(attempt - 1, jitter()), &mut stopped, &mut shutdown).await {
                        return;
                    }
                    continue;
                }
            }
        }
        shared.set(|status| status.phase = LinkPhase::Linking);
        let linked = link_once(&config, &managed, &shared, &mut stopped, &mut shutdown).await;
        shared.set(|status| status.serving = false);
        let wait = match linked {
            Ok(true) => {
                attempt = 0;
                refusals = 0;
                shared.set(|status| status.phase = LinkPhase::Down);
                config.backoff.delay(0, jitter())
            }
            Ok(false) => return,
            Err(LinkError::Refused(message)) => {
                tracing::warn!("Manager link: refused: {message}");
                shared.set(|status| {
                    status.phase = LinkPhase::Refused;
                    status.message = message;
                });
                refusals = refusals.saturating_add(1);
                config.refused_backoff.delay(refusals - 1, jitter())
            }
            Err(LinkError::Failed(message)) => {
                tracing::debug!("Manager link: {message}");
                shared.set(|status| {
                    status.phase = LinkPhase::Down;
                    status.message = message;
                });
                attempt = attempt.saturating_add(1);
                config.backoff.delay(attempt - 1, jitter())
            }
        };
        if !pause(wait, &mut stopped, &mut shutdown).await {
            return;
        }
    }
}

/// One link, from its dial to its loss: Ok(true) when it was up and is lost, Ok(false) when stopped.
async fn link_once(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Arc<Shared>,
    stopped: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<bool, LinkError> {
    let (mut client, mut driver) = connect(config, ROLE_UP).await?;
    let first_start = !shared.link.snapshot().bootstrap_done;
    let mut status_now = status_of(managed).await;
    status_now.first_start = first_start;
    let hello = tokio::time::timeout(
        config.connect_timeout,
        client.hello(HelloRequest {
            status: Some(status_now),
        }),
    )
    .await
    .map_err(|_| LinkError::Failed("the manager did not answer Hello".to_string()))?
    .map_err(|status| call_error("Hello", &status))?
    .into_inner();
    check_versions(LINK_VERSION, hello.link_version, "manager").map_err(LinkError::Refused)?;
    shared.set(|status| {
        status.phase = LinkPhase::Linked;
        status.generation = hello.generation;
        status.message.clear();
    });
    tracing::info!(
        "Manager link: linked to {} (generation {})",
        config.url,
        hello.generation
    );
    if first_start && lock(&shared.pending).is_none() {
        take_bundle(config, &mut client, shared).await?;
    }
    // each time the link finds the instance unsealed it tells the manager: started, or unsealed after a seal it saw
    let mut seen_sealed = false;
    let mut ticker = tokio::time::interval(config.status_every);
    ticker.tick().await;
    loop {
        if managed.sealed() {
            seen_sealed = true;
            tokio::select! {
                _ = ticker.tick() => {
                    let status = status_of(managed).await;
                    client.status(status).await.map_err(|status| call_error("Status", &status))?;
                }
                _ = &mut driver => return Ok(true),
                () = until(stopped) => return Ok(false),
                () = until(shutdown) => return Ok(false),
            }
            continue;
        }
        // unsealed: a bundle kept while sealed goes into the seal now, the manager hears of it, and Serve opens
        let stored = {
            let managed = Arc::clone(managed);
            let shared = Arc::clone(shared);
            tokio::task::spawn_blocking(move || store_pending(&shared, managed.as_ref()))
                .await
                .unwrap_or(false)
        };
        let status = status_of(managed).await;
        client
            .status(status)
            .await
            .map_err(|status| call_error("Status", &status))?;
        {
            let kind = if seen_sealed { "unsealed" } else { "started" };
            client
                .event(EventRequest {
                    kind: kind.to_string(),
                    message: if stored {
                        "the bootstrap bundle is kept in the seal".to_string()
                    } else {
                        String::new()
                    },
                })
                .await
                .map_err(|status| call_error("Event", &status))?;
        }
        match serve(config, managed, shared, &mut client, &mut driver, stopped, shutdown).await? {
            Served::Lost => return Ok(true),
            Served::Stopped => return Ok(false),
            Served::Sealed => {
                let status = status_of(managed).await;
                client
                    .status(status)
                    .await
                    .map_err(|status| call_error("Status", &status))?;
            }
        }
    }
}

/// The bootstrap bundle on a first start: opened with this instance's bundle key and kept, or held until the seal opens.
async fn take_bundle(config: &ManageConfig, client: &mut ManagerClient, shared: &Arc<Shared>) -> Result<(), LinkError> {
    let reply = tokio::time::timeout(config.connect_timeout, client.bootstrap(BootstrapRequest {}))
        .await
        .map_err(|_| LinkError::Failed("the manager did not answer Bootstrap".to_string()))?
        .map_err(|status| call_error("Bootstrap", &status))?
        .into_inner();
    if reply.sealed.is_empty() {
        // the manager has none for this instance (one it did not make, or a provision whose 15 minutes passed)
        let _ = shared.link.update(|state| state.bootstrap_done = true);
        return Ok(());
    }
    let instance = shared.link.snapshot().app_instance_id;
    let values =
        super::bundle::open(config.key.bundle_secret(), &instance, &reply.sealed).map_err(LinkError::Refused)?;
    tracing::info!("Manager link: took the bootstrap bundle ({} secrets)", values.len());
    *lock(&shared.pending) = Some(values);
    Ok(())
}

async fn status_of(managed: &Arc<dyn Managed>) -> super::proto::InstanceStatus {
    let managed = Arc::clone(managed);
    tokio::task::spawn_blocking(move || managed.status())
        .await
        .unwrap_or_default()
}

/// How a Serve stream ended.
enum Served {
    /// The connection is gone.
    Lost,
    Stopped,
    /// The instance was sealed: the stream closed, the link stays.
    Sealed,
}

/// The Serve stream: each command run (off the runtime, as it may block) and answered, the status sent every so
/// often, until the link is lost or the instance sealed.
async fn serve(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Arc<Shared>,
    client: &mut ManagerClient,
    driver: &mut tokio::task::JoinHandle<()>,
    stopped: &mut watch::Receiver<bool>,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<Served, LinkError> {
    let (replies, outgoing) = tokio::sync::mpsc::channel::<super::proto::CommandReply>(16);
    let stream = futures_util::stream::unfold(outgoing, |mut outgoing| async move {
        outgoing.recv().await.map(|reply| (reply, outgoing))
    });
    let mut commands = client
        .clone()
        .serve(stream)
        .await
        .map_err(|status| call_error("Serve", &status))?
        .into_inner();
    shared.set(|status| status.serving = true);
    let mut ticker = tokio::time::interval(config.status_every);
    ticker.tick().await;
    let served = loop {
        tokio::select! {
            command = commands.message() => match command {
                Ok(Some(command)) => {
                    let (runner, link) = (Arc::clone(managed), Arc::clone(&shared.link));
                    let reply = tokio::task::spawn_blocking(move || super::commands::run(runner.as_ref(), &link, command))
                        .await
                        .map_err(|err| LinkError::Failed(format!("a command failed: {err}")))?;
                    // a seal's answer goes up whole: the manager ends the stream when it reads a sealed status
                    if replies.send(reply).await.is_err() {
                        break Served::Lost;
                    }
                }
                Ok(None) if managed.sealed() => break Served::Sealed,
                Ok(None) => break Served::Lost,
                Err(status) => return Err(call_error("Serve", &status)),
            },
            _ = ticker.tick() => {
                if managed.sealed() {
                    break Served::Sealed;
                }
                let status = status_of(managed).await;
                client.status(status).await.map_err(|status| call_error("Status", &status))?;
            }
            _ = &mut *driver => break Served::Lost,
            () = until(stopped) => break Served::Stopped,
            () = until(shutdown) => break Served::Stopped,
        }
    };
    shared.set(|status| status.serving = false);
    Ok(served)
}

/// The dialers of this process, by runtime directory: a server's gate starts its own before the HTTP server, which then
/// finds it here.
static DIALERS: LazyLock<Mutex<HashMap<PathBuf, Arc<Dialer>>>> = LazyLock::new(Mutex::default);

/// The dialer of `runtime_dir`, when one runs.
pub(crate) fn dialer_of(runtime_dir: &Path) -> Option<Arc<Dialer>> {
    lock(&DIALERS).get(runtime_dir).cloned()
}

fn keep(runtime_dir: &Path, dialer: Dialer) -> Arc<Dialer> {
    let dialer = Arc::new(dialer);
    lock(&DIALERS).insert(runtime_dir.to_path_buf(), Arc::clone(&dialer));
    dialer
}

/// A server's gate (Sealed-Secrets §3.3): when this server is managed, start its dialer on a runtime of its own before
/// the gate waits, so a sealed instance links (dialing out, no listener) and takes its bootstrap bundle.
pub(crate) fn start_for_gate(runtime_dir: &Path, conf: &moqui_lib::node::MNode) {
    start_thread(runtime_dir, &super::gate_settings(conf));
}

/// The dialer on a thread and a runtime of its own, for the life of the process.
fn start_thread(runtime_dir: &Path, setting: super::Settings<'_>) {
    if dialer_of(runtime_dir).is_some() {
        return;
    }
    let config = match ManageConfig::from_settings(runtime_dir, setting) {
        Ok(Some(config)) => config,
        Ok(None) => return,
        Err(err) => {
            tracing::error!("Manager link: not started: {err}");
            return;
        }
    };
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let runtime_dir = runtime_dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("manage-link".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::error!("Manager link: no runtime for the link: {err}");
                    let _ = ready_tx.send(());
                    return;
                }
            };
            // the link lives as long as the process: its shutdown never comes
            let (_never, shutdown) = watch::channel(false);
            let managed: Arc<dyn Managed> = Arc::new(ServerManaged::new(&runtime_dir));
            let dialer = keep(&runtime_dir, Dialer::start(config, managed, runtime.handle(), shutdown));
            let _ = ready_tx.send(());
            runtime.block_on(async move {
                let mut stopped = dialer.stop.subscribe();
                let _ = stopped.wait_for(|stop| *stop).await;
            });
        });
    match spawned {
        Ok(_) => {
            let _ = ready_rx.recv_timeout(Duration::from_secs(5));
        }
        Err(err) => tracing::error!("Manager link: its thread did not start: {err}"),
    }
}

/// After the gate's seal opened: on a first start, wait up to `manage@bootstrap-wait-seconds` for the bootstrap bundle
/// and keep it in the seal before the boot reads its datasource password.
pub(crate) fn after_unseal(runtime_dir: &Path, conf: &moqui_lib::node::MNode) {
    let Some(dialer) = dialer_of(runtime_dir) else {
        return;
    };
    let managed = ServerManaged::new(runtime_dir);
    let wait = super::gate_settings(conf)("manage@bootstrap-wait-seconds")
        .parse::<u64>()
        .unwrap_or(120);
    let deadline = Instant::now() + Duration::from_secs(wait);
    loop {
        if dialer.store_pending(&managed) || dialer.state().bootstrap_done {
            return;
        }
        let status = dialer.status();
        if matches!(status.phase, LinkPhase::Unpaired | LinkPhase::Refused) || Instant::now() >= deadline {
            tracing::warn!(
                "Manager link: the boot goes on without a bootstrap bundle ({}: {})",
                status.phase.name(),
                status.message
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// At the HTTP server's start: the dialer, unless the gate started it or this server is not managed.
pub(crate) fn start_for_server(runtime_dir: &Path, handle: tokio::runtime::Handle, shutdown: watch::Receiver<bool>) {
    if dialer_of(runtime_dir).is_some() {
        return;
    }
    match ManageConfig::from_settings(runtime_dir, &super::server_setting) {
        Ok(Some(config)) => {
            let managed: Arc<dyn Managed> = Arc::new(ServerManaged::new(runtime_dir));
            keep(runtime_dir, Dialer::start(config, managed, &handle, shutdown));
        }
        Ok(None) => {}
        Err(err) => tracing::error!("Manager link: not started: {err}"),
    }
}

/// Pair this server with a manager by a code made in System › Instances (an instance the manager did not make), then
/// dial it on the current runtime's handle or a thread of its own.
pub(crate) fn pair_by_code(runtime_dir: &Path, url: &str, pin: &str, code: &str) -> Result<LinkState, String> {
    if let Some(dialer) = dialer_of(runtime_dir) {
        if !dialer.state().app_instance_id.is_empty() {
            return Err("this server is paired with a manager already".to_string());
        }
        dialer.stop();
        lock(&DIALERS).remove(runtime_dir);
    }
    let db = runtime_dir.join("db");
    let key = LinkKey::load_or_create(
        &db.join(super::KEY_FILE),
        &KeyStore::from_settings(&super::server_setting)?,
        CERT_NAME,
    )?;
    let mut config = ManageConfig::new(url, pin, Arc::new(key));
    config.state_file = Some(db.join(super::STATE_FILE));
    config.secret = Some(Arc::new(Zeroizing::new(code.trim().to_string())));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    let state = runtime.block_on(pair(&config)).map_err(|err| err.to_string())?;
    drop(runtime);
    start_thread(runtime_dir, &super::server_setting);
    Ok(state)
}
