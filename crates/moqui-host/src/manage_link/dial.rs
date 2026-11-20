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
//!
//! The managed sealed start (plans/Vault-Instances.md §4.5, M6) on the link:
//! - while sealed after an automatic unseal failed (the manager down, or holding it for an approval), the unseal is
//!   tried again on each status, through the manager's transit ([`super::unseal`]);
//! - while sealed and opened by recovery shares, the instance offers a key it made for this unseal, signed with its
//!   link key, and enters the shares holders sealed to it (`moqui vault seal-share`), which the manager relays;
//! - once unsealed, each status carries the instance's proof that it holds its root key (the key derived from it,
//!   over this link's generation and the Hello's challenge); when the manager does not take it and the seal is the
//!   manager's transit, the instance has the manager unwrap its root key once on this link, and checks the answer;
//! - and the recovery shares go to their holders ([`super::shares`]), whose confirmations it checks.

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
use super::proto::{
    BootstrapRequest, EventRequest, HelloRequest, HoldersRequest, PairRequest, SealedSharesRequest,
    ShareConfirmationsRequest, UnsealReply, UnsealRequest,
};
use super::state::{set_from_proto, LinkState};
use super::{
    check_versions, manager_client, parse_url, LinkError, ManagerClient, ALPN, LINK_VERSION, ROLE_PAIR, ROLE_UP,
};
use crate::link::{jitter, read_answer, Answer, Backoff};
use crate::tls_pin::{parse_hex, Pin, PinVerifier};

/// The name in the link key's certificate.
const CERT_NAME: &str = "moqui-manage-link";

/// Every how many status ticks of a served link the recovery shares' step runs again.
const SHARES_EVERY: u32 = 20;

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

/// The link now, while it is up: its client and what its Hello said (M6).
#[derive(Clone)]
struct LinkNow {
    client: ManagerClient,
    generation: u64,
    transit_public: Vec<u8>,
    challenge: Vec<u8>,
}

/// The dialer's state shared with its task.
struct Shared {
    status: Mutex<LinkStatus>,
    link: Arc<LinkContext>,
    /// A bootstrap bundle taken while sealed, kept in memory until the seal opens.
    pending: Mutex<Option<Values>>,
    now: Mutex<Option<LinkNow>>,
    /// The runtime the link runs on: an unseal from the gate's thread is a call spawned there.
    handle: tokio::runtime::Handle,
    /// The key this sealed instance offers for an unseal by recovery shares, and when it stops being taken; made again
    /// once it expires, and dropped once unsealed.
    unseal_key: Mutex<Option<(Arc<moqui_vault::hpke::SecretKey>, i64)>>,
    /// A new set of recovery shares sent to replace the old one, until the manager takes or refuses its deliveries:
    /// in memory only, as the seal keeps the new split only then.
    replacement: Mutex<Option<(super::proto::SharesRequest, super::state::ShareSet)>>,
}

/// Clears the link now when a link ends.
struct LinkEnds(Arc<Shared>);

impl Drop for LinkEnds {
    fn drop(&mut self) {
        *lock(&self.0.now) = None;
    }
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
            now: Mutex::new(None),
            handle: handle.clone(),
            unseal_key: Mutex::new(None),
            replacement: Mutex::new(None),
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

    /// The link now, waiting up to `wait` for it.
    fn link_now(&self, wait: Duration) -> Result<LinkNow, String> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(now) = lock(&self.shared.now).clone() {
                return Ok(now);
            }
            if Instant::now() >= deadline {
                let status = self.status();
                return Err(format!(
                    "no link to the manager ({}{})",
                    status.phase.name(),
                    if status.message.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", status.message)
                    }
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// This instance's id and the public half of its transit key on the manager, from the link's Hello (M6), waiting
    /// up to `wait` for the link. From a thread that is not the link's runtime's.
    pub fn transit_key(&self, wait: Duration) -> Result<(String, Vec<u8>), String> {
        let now = self.link_now(wait)?;
        if now.transit_public.len() != 32 {
            return Err("the manager gave no transit key for this instance".to_string());
        }
        Ok((self.state().app_instance_id, now.transit_public))
    }

    /// An Unseal call on the link, waiting up to `wait` for the link and the answer. From a thread that is not the link's
    /// runtime's (the gate's, or a blocking task's): the call runs on that runtime.
    pub fn unseal_call(&self, request: UnsealRequest, wait: Duration) -> Result<UnsealReply, String> {
        let mut client = self.link_now(wait)?.client;
        let (answer, answered) = std::sync::mpsc::channel();
        self.shared.handle.spawn(async move {
            let reply = tokio::time::timeout(Duration::from_secs(30), client.unseal(request)).await;
            let _ = answer.send(reply);
        });
        match answered.recv_timeout(wait + Duration::from_secs(30)) {
            Ok(Ok(Ok(reply))) => Ok(reply.into_inner()),
            Ok(Ok(Err(status))) => Err(format!("the manager refused the unseal: {}", status.message())),
            Ok(Err(_)) | Err(_) => Err("the manager did not answer the unseal in time".to_string()),
        }
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
    let _ends = LinkEnds(Arc::clone(shared));
    *lock(&shared.now) = Some(LinkNow {
        client: client.clone(),
        generation: hello.generation,
        transit_public: hello.transit_public.clone(),
        challenge: hello.challenge.clone(),
    });
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
    let mut sealed_tried = false;
    let mut ticker = tokio::time::interval(config.status_every);
    ticker.tick().await;
    loop {
        if managed.sealed() {
            seen_sealed = true;
            if !sealed_tried {
                // at once, not a status interval after the link came up
                sealed_tried = true;
                sealed_step(config, managed, shared, &mut client).await;
                continue;
            }
            tokio::select! {
                _ = ticker.tick() => {
                    let status = status_of(managed).await;
                    client.status(status).await.map_err(|status| call_error("Status", &status))?;
                    sealed_step(config, managed, shared, &mut client).await;
                }
                _ = &mut driver => return Ok(true),
                () = until(stopped) => return Ok(false),
                () = until(shutdown) => return Ok(false),
            }
            continue;
        }
        sealed_tried = false;
        *lock(&shared.unseal_key) = None;
        // unsealed: a bundle kept while sealed goes into the seal now, the manager hears of it, and Serve opens
        let stored = {
            let managed = Arc::clone(managed);
            let shared = Arc::clone(shared);
            tokio::task::spawn_blocking(move || store_pending(&shared, managed.as_ref()))
                .await
                .unwrap_or(false)
        };
        prove_unsealed(config, managed, shared, &mut client).await?;
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
        shares_step(config, managed, shared, &mut client, true).await;
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

/// The instance's status with its unsealed proof on this link, when it is unsealed (M6).
fn with_proof(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    mut status: super::proto::InstanceStatus,
) -> super::proto::InstanceStatus {
    if status.seal_state != "unsealed" {
        return status;
    }
    let Some(now) = lock(&shared.now).clone() else {
        return status;
    };
    let Some(key) = managed.unsealed_proof_key() else {
        return status;
    };
    let instance_id = shared.link.snapshot().app_instance_id;
    let link = moqui_vault::managed::ProofLink {
        instance_id: &instance_id,
        link_fingerprint: config.key.fingerprint(),
        generation: now.generation,
        challenge: &now.challenge,
    };
    status.proof = moqui_vault::managed::sign_proof(&key, &link).to_vec();
    status.proof_public = key.verifying_key().as_bytes().to_vec();
    status
}

/// Send the status with its proof: whether the manager takes this link's "unsealed".
async fn send_status(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
) -> Result<bool, LinkError> {
    let status = with_proof(config, managed, shared, status_of(managed).await);
    let reply = client
        .status(status)
        .await
        .map_err(|status| call_error("Status", &status))?
        .into_inner();
    Ok(reply.unsealed_verified)
}

/// The status with its proof; when the manager does not take it: a rotate-root's new proof key signed by the old one,
/// or, on the manager's transit, its unwrap of the root key once on this link (the first pin), then the status again.
async fn prove_unsealed(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
) -> Result<(), LinkError> {
    if send_status(config, managed, shared, client).await? {
        return Ok(());
    }
    if managed.previous_proof_key().is_some() {
        if rotate_proof(config, managed, shared, client).await {
            send_status(config, managed, shared, client).await?;
        }
        return Ok(());
    }
    if verify_transit(config, managed, shared, client).await {
        send_status(config, managed, shared, client).await?;
    }
    Ok(())
}

/// After a rotate-root: the new proof key (and the new transit wrap), signed by the old root key's proof key, so the
/// manager pins it; true when it did.
async fn rotate_proof(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
) -> bool {
    let (Some(old), Some(new)) = (managed.previous_proof_key(), managed.unsealed_proof_key()) else {
        return false;
    };
    let instance_id = shared.link.snapshot().app_instance_id;
    let wrapped = managed.seal_method().1.unwrap_or_default();
    let new_public = *new.verifying_key().as_bytes();
    let signature =
        moqui_vault::managed::sign_rotation(&old, &instance_id, config.key.fingerprint(), &new_public, &wrapped);
    let asked = client
        .rotate_proof(super::proto::RotateProofRequest {
            new_public: new_public.to_vec(),
            wrapped_root: wrapped,
            signature: signature.to_vec(),
        })
        .await;
    match asked {
        Ok(_) => {
            managed.forget_previous_proof();
            tracing::info!("Manager link: the manager pinned this instance's new unsealed proof key (a rotate-root)");
            true
        }
        Err(status) => {
            tracing::error!(
                "Manager link: the manager did not take the new unsealed proof key: {}",
                status.message()
            );
            false
        }
    }
}

/// The manager did not take this link's proof and the seal is the manager's transit (the instance's first unseal was
/// its own init): the manager unwraps the root key once on this link, as an unseal does, and the instance checks it got
/// its own root key back; true when it did. Under the approval policy, or outside its first start, it waits for a
/// member; it is asked again on a later status.
async fn verify_transit(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
) -> bool {
    let Some(wrapped) = managed.seal_method().1 else {
        return false;
    };
    let instance_id = shared.link.snapshot().app_instance_id;
    let ephemeral = moqui_vault::hpke::SecretKey::random(&mut moqui_vault::rng::OsRng);
    let Ok(public) = ephemeral.public() else {
        return false;
    };
    let asked = tokio::time::timeout(
        config.connect_timeout,
        client.unseal(UnsealRequest {
            wrapped_root: wrapped,
            ephemeral_public: public.as_bytes().to_vec(),
        }),
    )
    .await;
    match asked {
        Ok(Ok(reply)) => {
            let reply = reply.into_inner();
            if !reply.waiting.is_empty() {
                tracing::warn!(
                    "Manager link: the manager has not taken this instance's unseal: {}",
                    reply.waiting
                );
                return false;
            }
            let opened = moqui_vault::managed::open_reply(&ephemeral, &instance_id, &reply.sealed_root);
            match opened {
                Ok(root) if managed.is_root(&root) => {
                    tracing::info!("Manager link: the manager's transit unwrapped this instance's root key");
                    true
                }
                _ => {
                    tracing::error!("Manager link: the manager's transit did not give this instance's root key back");
                    false
                }
            }
        }
        Ok(Err(status)) => {
            tracing::warn!(
                "Manager link: the manager did not unwrap this instance's root key: {}",
                status.message()
            );
            false
        }
        Err(_) => {
            tracing::warn!("Manager link: the manager did not answer the unwrap in time");
            false
        }
    }
}

/// While sealed (M6): the automatic unseal again when it failed, and the shares holders sealed to this unseal.
async fn sealed_step(config: &ManageConfig, managed: &Arc<dyn Managed>, shared: &Shared, client: &mut ManagerClient) {
    if managed.auto_unseal_failed() {
        let retried = {
            let managed = Arc::clone(managed);
            tokio::task::spawn_blocking(move || managed.retry_unseal()).await
        };
        match retried {
            Ok(Ok(())) if !managed.sealed() => return,
            Ok(Ok(())) => {}
            Ok(Err(err)) => tracing::info!("Manager link: still sealed: {err}"),
            Err(err) => tracing::error!("Manager link: the unseal failed: {err}"),
        }
    }
    if managed.takes_shares() {
        take_sealed_shares(config, managed, shared, client).await;
    }
}

/// How long a key for an unseal by shares is taken; a new one is made after.
const UNSEAL_KEY_MILLIS: i64 = 60 * 60_000;

/// Offer this unseal's key (signed with the link key, with the time it stops being taken) and enter each share a holder
/// sealed to it. A share that does not open with it (sealed for another unseal, or changed) is refused.
async fn take_sealed_shares(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
) {
    let instance_id = shared.link.snapshot().app_instance_id;
    let now = moqui_core::user::now_millis();
    let (secret, expires) = {
        let mut held = lock(&shared.unseal_key);
        if held.as_ref().is_none_or(|(_, expires)| *expires <= now + 60_000) {
            *held = Some((
                Arc::new(moqui_vault::hpke::SecretKey::random(&mut moqui_vault::rng::OsRng)),
                now + UNSEAL_KEY_MILLIS,
            ));
        }
        let (secret, expires) = held.as_ref().expect("made just above when missing or expiring");
        (Arc::clone(secret), *expires)
    };
    let Ok(public) = secret.public() else {
        return;
    };
    let link_key = config.key.signing_key();
    let key = moqui_vault::managed::UnsealKey::sign(&link_key, &instance_id, public.as_bytes(), expires);
    let asked = tokio::time::timeout(
        config.connect_timeout,
        client.sealed_shares(SealedSharesRequest {
            unseal_public: public.as_bytes().to_vec(),
            signature: key.signature,
            link_public: link_key.verifying_key().as_bytes().to_vec(),
            expires,
        }),
    )
    .await;
    let shares = match asked {
        Ok(Ok(reply)) => reply.into_inner().shares,
        Ok(Err(status)) => {
            tracing::warn!("Manager link: the manager took no unseal key: {}", status.message());
            return;
        }
        Err(_) => return,
    };
    for sealed in shares {
        let Ok(share) = moqui_vault::managed::open_share(&secret, &instance_id, &sealed.sealed) else {
            tracing::error!(
                "Manager link: a share from {} does not open with this unseal's key (sealed for another, or changed): \
                 refused",
                sealed.holder_user_id
            );
            continue;
        };
        let entered = {
            let managed = Arc::clone(managed);
            tokio::task::spawn_blocking(move || managed.enter_share(&share)).await
        };
        match entered {
            Ok(Ok(())) => tracing::info!(
                "Manager link: entered the recovery share {} sealed to this unseal",
                sealed.holder_user_id
            ),
            Ok(Err(err)) => tracing::error!(
                "Manager link: the share {} sealed was not taken: {err}",
                sealed.holder_user_id
            ),
            Err(err) => tracing::error!("Manager link: entering a share failed: {err}"),
        }
    }
}

/// How the manager answered a share set.
enum Sent {
    Taken,
    /// Not now (the link, or the manager not yet taking this link's unsealed): sent again later.
    Later,
    /// Refused for good (a holder or a key it no longer takes): a new set is made for the current holders.
    Refused(String),
}

/// Send a share set; whether the manager took it.
async fn send_shares(client: &mut ManagerClient, request: super::proto::SharesRequest) -> Sent {
    match client.shares(request).await {
        Ok(_) => Sent::Taken,
        Err(status)
            if matches!(
                status.code(),
                tonic::Code::PermissionDenied | tonic::Code::InvalidArgument | tonic::Code::AlreadyExists
            ) =>
        {
            Sent::Refused(status.message().to_string())
        }
        Err(status) => {
            tracing::warn!(
                "Manager link: the manager did not take the recovery shares now (sent again later): {}",
                status.message()
            );
            Sent::Later
        }
    }
}

/// The first share set, kept to send (its deliveries only, each sealed to one holder) until the manager takes it:
/// false when it waits still.
async fn send_first_set(shared: &Shared, client: &mut ManagerClient) -> bool {
    use prost::Message as _;
    let pending = shared.link.snapshot().pending_shares;
    if pending.is_empty() {
        return true;
    }
    let request = moqui_vault::text::unhex(&pending)
        .ok()
        .and_then(|bytes| super::proto::SharesRequest::decode(bytes.as_slice()).ok());
    let sent = match request {
        Some(request) => send_shares(client, request).await,
        None => Sent::Refused("the share set kept to send does not read".to_string()),
    };
    match sent {
        Sent::Taken => {
            let _ = shared.link.update(|state| state.pending_shares.clear());
            tracing::info!("Manager link: the recovery shares went to their holders, one each");
            true
        }
        Sent::Later => false,
        Sent::Refused(why) => {
            tracing::error!(
                "Manager link: the manager refused the recovery shares for good ({why}): a new set is made for the \
                 current holders"
            );
            let _ = shared.link.update(|state| {
                state.pending_shares.clear();
                if let Some(set) = state.share_set.as_mut() {
                    set.refused = true;
                }
            });
            true
        }
    }
}

/// A replacement set in flight: kept (the old shares stop opening the seal) only once the manager took its deliveries,
/// dropped when it refused them for good. False when it waits still.
async fn send_replacement(managed: &Arc<dyn Managed>, shared: &Shared, client: &mut ManagerClient) -> bool {
    let Some((request, set)) = lock(&shared.replacement).clone() else {
        return true;
    };
    match send_shares(client, request).await {
        Sent::Taken => {
            *lock(&shared.replacement) = None;
            let committed = {
                let managed = Arc::clone(managed);
                tokio::task::spawn_blocking(move || managed.commit_new_shares()).await
            };
            match committed {
                Ok(Ok(())) => {
                    if let Err(err) = shared.link.update(|state| state.share_set = Some(set)) {
                        tracing::error!("Manager link: the new share set was not kept in the pairing state: {err}");
                    }
                    tracing::warn!(
                        "Manager link: a new set of recovery shares went to the holders; the old one is void"
                    );
                }
                Ok(Err(err)) => tracing::error!(
                    "Manager link: the new recovery shares were sent and not kept ({err}): the old set still opens the \
                     seal, and a new one is made"
                ),
                Err(err) => tracing::error!("Manager link: the new recovery shares were not kept: {err}"),
            }
            true
        }
        Sent::Later => false,
        Sent::Refused(why) => {
            *lock(&shared.replacement) = None;
            managed.drop_new_shares();
            tracing::error!("Manager link: the manager refused the new recovery shares ({why}): they are dropped");
            true
        }
    }
}

/// Once unsealed (M6): the recovery shares to their holders, the holders' confirmations, and a new set when the first
/// was refused for good or a holder was replaced (`check_holders`, once a link). A failure is logged; the link goes on.
async fn shares_step(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
    check_holders: bool,
) {
    if !send_first_set(shared, client).await || !send_replacement(managed, shared, client).await {
        return;
    }
    let state = shared.link.snapshot();
    let held = managed.held_shares();
    let first = state.share_set.as_ref().is_none_or(|set| set.refused) && held.is_some();
    let maybe_replaced =
        state.share_set.as_ref().is_some_and(|set| set.refused) || (check_holders && state.share_set.is_some());
    if first || maybe_replaced {
        let holders = match client.holders(HoldersRequest {}).await {
            Ok(reply) => reply.into_inner(),
            Err(status) => {
                tracing::warn!("Manager link: no recovery holders: {}", status.message());
                return;
            }
        };
        let checked = super::shares::checked_holders(&holders, &state.approver_set());
        let (threshold, count) = managed.recovery_counts();
        let count = usize::try_from(count).unwrap_or(usize::MAX);
        let due = first
            || state
                .share_set
                .as_ref()
                .is_some_and(|set| set.refused || super::shares::holder_replaced(set, &checked));
        if due && checked.len() < count {
            tracing::error!(
                "Manager link: {} of the {count} recovery holders have a key that an approver this instance pinned \
                 checked: the shares are not sent{}",
                checked.len(),
                if first {
                    "; they wait for `moqui operator init --take-shares` on the operator socket"
                } else {
                    ", and the old set stays"
                }
            );
        } else if due {
            make_and_send(config, managed, shared, client, &checked, threshold, held).await;
        }
    }
    take_confirmations(managed, shared, client).await;
}

/// A new share set to `holders`: the held shares of an automatic init (`held`), kept to send until the manager takes
/// them, or a new split, kept in the seal only once the manager took its deliveries.
async fn make_and_send(
    config: &ManageConfig,
    managed: &Arc<dyn Managed>,
    shared: &Shared,
    client: &mut ManagerClient,
    holders: &[super::shares::Checked],
    threshold: u32,
    held: Option<Vec<zeroize::Zeroizing<String>>>,
) {
    use prost::Message as _;
    let instance_id = shared.link.snapshot().app_instance_id;
    let first = held.is_some();
    let shares = match held {
        Some(shares) => shares,
        None => {
            let prepared = {
                let managed = Arc::clone(managed);
                tokio::task::spawn_blocking(move || managed.prepare_new_shares()).await
            };
            match prepared {
                Ok(Ok(shares)) => shares,
                Ok(Err(err)) => {
                    tracing::error!("Manager link: no new recovery shares: {err}");
                    return;
                }
                Err(err) => {
                    tracing::error!("Manager link: the new recovery shares were not made: {err}");
                    return;
                }
            }
        }
    };
    let made = super::shares::make_set(
        &instance_id,
        &config.key.signing_key(),
        &shares,
        &holders[..shares.len().min(holders.len())],
        threshold,
    );
    drop(shares);
    let (request, set) = match made {
        Ok(made) => made,
        Err(err) => {
            tracing::error!("Manager link: the recovery shares were not sealed: {err}");
            if !first {
                managed.drop_new_shares();
            }
            return;
        }
    };
    if first {
        let encoded = moqui_vault::text::hex(&request.encode_to_vec());
        if let Err(err) = shared.link.update(|state| {
            state.share_set = Some(set);
            state.pending_shares = encoded;
        }) {
            tracing::error!("Manager link: the share set was not kept: {err}");
        }
        send_first_set(shared, client).await;
    } else {
        *lock(&shared.replacement) = Some((request, set));
        send_replacement(managed, shared, client).await;
    }
}

/// The holders' confirmations of the share set, each checked here; the unconfirmed ones reported. Once every share
/// is confirmed, the held shares of an automatic init are dropped from the operator socket.
async fn take_confirmations(managed: &Arc<dyn Managed>, shared: &Shared, client: &mut ManagerClient) {
    let instance_id = shared.link.snapshot().app_instance_id;
    let Some(mut set) = shared.link.snapshot().share_set else {
        return;
    };
    if set.refused || !shared.link.snapshot().pending_shares.is_empty() {
        return;
    }
    if set.complete() {
        managed.release_held_shares();
        return;
    }
    let reply = match client
        .share_confirmations(ShareConfirmationsRequest {
            set_id: set.set_id.clone(),
        })
        .await
    {
        Ok(reply) => reply.into_inner(),
        Err(status) => {
            tracing::warn!("Manager link: no share confirmations: {}", status.message());
            return;
        }
    };
    if super::shares::take_confirmations(&mut set, &instance_id, &reply) > 0 {
        if let Err(err) = shared.link.update(|state| state.share_set = Some(set.clone())) {
            tracing::error!("Manager link: the confirmations were not kept: {err}");
        }
        if set.complete() {
            managed.release_held_shares();
            tracing::info!("Manager link: every recovery share is confirmed by its holder: init is complete");
            return;
        }
    }
    let unconfirmed: Vec<String> = set
        .shares
        .iter()
        .filter(|share| !share.confirmed)
        .map(|share| format!("{} ({})", share.index, share.user_id))
        .collect();
    tracing::warn!(
        "Manager link: recovery shares not confirmed by their holders yet: {}",
        unconfirmed.join(", ")
    );
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
    let mut ticks: u32 = 0;
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
                // the status with its proof, and when the manager does not take it, its new proof key or the transit's
                // unwrap once
                prove_unsealed(config, managed, shared, client).await?;
                // now and then (M6): a share set the manager has not taken, the held shares of an automatic init (once
                // the manager takes this link's unsealed), and the holders' confirmations until every share is confirmed
                ticks = ticks.wrapping_add(1);
                if ticks.is_multiple_of(SHARES_EVERY) {
                    shares_step(config, managed, shared, client, false).await;
                }
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
pub fn dialer_of(runtime_dir: &Path) -> Option<Arc<Dialer>> {
    lock(&DIALERS).get(runtime_dir).cloned()
}

fn keep(runtime_dir: &Path, dialer: Dialer) -> Arc<Dialer> {
    let dialer = Arc::new(dialer);
    lock(&DIALERS).insert(runtime_dir.to_path_buf(), Arc::clone(&dialer));
    dialer
}

/// A server's gate (Sealed-Secrets §3.3): when this server is managed, start its dialer on a runtime of its own before
/// the gate waits, so a sealed instance links (dialing out, no listener) and takes its bootstrap bundle.
pub fn start_for_gate(runtime_dir: &Path, conf: &moqui_lib::node::MNode) {
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
    // a managed instance's transit is its manager's (M6), installed before the gate opens the seal
    super::unseal::register(runtime_dir);
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
            super::unseal::register(runtime_dir);
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
