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

//! Certificates by ACME, from System › Certificates and never on their own (plans/Edge-Node2.md §4.8, decision 12).
//!
//! A name an operator set up there (`moqui.server.AcmeName`) is ordered from its authority ([`order`]) with the
//! TLS-ALPN-01 challenge, which needs no port 80: the authority opens a TLS connection to the name on port 443 offering
//! ALPN `acme-tls/1`, and this node answers with a certificate made for the challenge ([`challenge`]). The certificate
//! the order gets is a row (`AcmeCertificate`, its key encrypted) that every member of a cluster writes as
//! `runtime/cert/<name>/fullchain.pem` and `privkey.pem` ([`store::sync_files`]), certbot's layout, which the HTTPS
//! listener's [`crate::tls::CertStore`] reads. The renewal job renews only names set up there.
//!
//! Where the challenge is answered:
//! - the HTTPS listener ([`accept`], from `crate::http`), before any other settings are picked: a handshake that offers
//!   `acme-tls/1` gets the challenge's certificate when the name it asks for has a challenge open, and is refused
//!   otherwise;
//! - on an edge node ([`answer`], from `edge_link::accept`), after a passthrough host's check: a local node's
//!   passthrough name goes down its tunnel untouched, and the local node answers it on its own;
//! - on a local node, a passthrough tunnel's TLS ([`accept`], from `edge_link::down`).
//!
//! **The challenge-only listener.** A node with no certificate at all does not start its HTTPS listener (a name with no
//! certificate gets no HTTPS). Once a name is set up here, the listener starts ([`Acme::want`], `crate::http`'s late
//! bind): it answers `acme-tls/1` for a name with a challenge open and refuses every other handshake until a certificate
//! is there.

mod challenge;
pub(crate) mod check;
pub(crate) mod order;
pub(crate) mod services;
pub(crate) mod store;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _};

pub(crate) use challenge::Challenge;

/// The ALPN protocol of the TLS-ALPN-01 challenge (RFC 8737).
pub(crate) const ALPN: &[u8] = b"acme-tls/1";
/// The open challenges are read from their rows again when the last read is this old, and at most this often, so a
/// handshake that offers `acme-tls/1` costs at most one read in this time, whoever sends it.
const READ_EVERY: Duration = Duration::from_secs(1);
/// How long a challenge just written waits before the authority, or the check, is told to look: past [`READ_EVERY`], so
/// every member's next `acme-tls/1` handshake reads the rows again and sees it.
pub(crate) const SEEN_BY_EVERY_MEMBER: Duration = Duration::from_millis(1200);
/// How often each member writes the certificates' files from their rows and sees a name set up on another member.
const SYNC_EVERY: Duration = Duration::from_secs(60);

/// This process's part of ACME: the open challenges as last read, whether the HTTPS listener of a node that started
/// with no certificate is wanted, and the names it is ordering now. A factory extension ([`Acme::of`]).
pub(crate) struct Acme {
    challenges: RwLock<HashMap<String, Arc<Challenge>>>,
    /// When the challenge rows were last read.
    read: Mutex<Option<Instant>>,
    /// Why the listener is wanted: a name is set up (on any member), a certificate file is there, or set-up checks
    /// running now. The first two never go back; a check's ends with it.
    reasons: Mutex<Wanted>,
    /// Whether the listener of a node with no certificate is bound now ([`Acme::wanted`]).
    wanted: tokio::sync::watch::Sender<bool>,
    /// The HTTPS listener's address while it is bound, for the screen.
    listening: Mutex<Option<SocketAddr>>,
    /// Whether this process has an HTTPS listener, bound or standing by; with none a challenge cannot be answered.
    https: std::sync::atomic::AtomicBool,
    /// The names this process is ordering now.
    ordering: Mutex<HashSet<String>>,
}

#[derive(Default)]
struct Wanted {
    set_up: bool,
    files: bool,
    checks: usize,
}

impl Default for Acme {
    fn default() -> Self {
        Self {
            challenges: RwLock::default(),
            read: Mutex::default(),
            reasons: Mutex::default(),
            wanted: tokio::sync::watch::channel(false).0,
            listening: Mutex::default(),
            https: std::sync::atomic::AtomicBool::new(false),
            ordering: Mutex::default(),
        }
    }
}

/// A set-up check that wants the listener while it runs ([`Acme::check_started`]).
pub(crate) struct Checking(Arc<Acme>);

impl Drop for Checking {
    fn drop(&mut self) {
        self.0.change(|wanted| wanted.checks = wanted.checks.saturating_sub(1));
    }
}

impl Acme {
    pub(crate) fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|_| Self::default())
    }

    fn change(&self, change: impl FnOnce(&mut Wanted)) {
        let mut reasons = sync::lock(&self.reasons);
        change(&mut reasons);
        let wanted = reasons.set_up || reasons.files || reasons.checks > 0;
        self.wanted
            .send_if_modified(|now| std::mem::replace(now, wanted) != wanted);
    }

    /// A name is set up with ACME, here or on another member: the HTTPS listener of a node with no certificate starts,
    /// to answer its challenges, and stays.
    pub(crate) fn want(&self) {
        self.change(|wanted| wanted.set_up = true);
    }

    /// A certificate file is in `runtime/cert`: the listener of a node that started with none starts, and stays.
    pub(crate) fn files_found(&self) {
        self.change(|wanted| wanted.files = true);
    }

    /// A set-up check runs: the listener is wanted until the returned guard drops, and then only when something else
    /// wants it.
    pub(crate) fn check_started(self: &Arc<Self>) -> Checking {
        self.change(|wanted| wanted.checks += 1);
        Checking(Arc::clone(self))
    }

    /// Whether the listener of a node that started with no certificate is wanted now; it binds when this turns true
    /// and closes when it turns false (`crate::http`).
    pub(crate) fn wanted(&self) -> tokio::sync::watch::Receiver<bool> {
        self.wanted.subscribe()
    }

    pub(crate) fn set_listening(&self, address: Option<SocketAddr>) {
        *sync::lock(&self.listening) = address;
    }

    /// This process has an HTTPS listener, bound or standing by.
    pub(crate) fn set_https(&self) {
        self.https.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn has_https(&self) -> bool {
        self.https.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn listening(&self) -> Option<SocketAddr> {
        *sync::lock(&self.listening)
    }

    /// The certificate settings for `name`'s open challenge, as last read.
    fn config(&self, name: &str) -> Option<Arc<rustls::ServerConfig>> {
        let challenge = sync::read(&self.challenges).get(name).cloned()?;
        challenge.config(name)
    }

    /// Whether this caller should read the rows again: true at most once in [`READ_EVERY`].
    fn claim_read(&self) -> bool {
        let mut read = sync::lock(&self.read);
        if read.is_some_and(|at| at.elapsed() < READ_EVERY) {
            return false;
        }
        *read = Some(Instant::now());
        true
    }

    /// Read the rows again at the next handshake that wants one: a challenge was just written.
    pub(crate) fn forget_read(&self) {
        *sync::lock(&self.read) = None;
    }

    /// Forget `name`'s challenge when its digest is `digest`: it was closed.
    pub(crate) fn forget(&self, name: &str, digest: &[u8; 32]) {
        let mut held = sync::write(&self.challenges);
        if held.get(name).is_some_and(|challenge| challenge.digest() == digest) {
            held.remove(name);
        }
    }

    fn replace_challenges(&self, challenges: HashMap<String, Arc<Challenge>>) {
        let mut held = sync::write(&self.challenges);
        // a challenge already loaded keeps its certificate
        let kept = challenges
            .into_iter()
            .map(|(name, fresh)| match held.get(&name) {
                Some(old) if old.same_as(&fresh) => (name, Arc::clone(old)),
                _ => (name, fresh),
            })
            .collect();
        *held = kept;
    }

    /// Put a challenge in place in this process: a test's, or one whose row is written too.
    #[cfg(test)]
    pub(crate) fn insert(&self, name: &str, challenge: Challenge) {
        sync::write(&self.challenges).insert(name.to_string(), Arc::new(challenge));
    }

    /// Mark `name` as being ordered by this process; `None` when it already is.
    pub(crate) fn begin_order(self: &Arc<Self>, name: &str) -> Option<Ordering> {
        sync::lock(&self.ordering).insert(name.to_string()).then(|| Ordering {
            acme: Arc::clone(self),
            name: name.to_string(),
        })
    }

    pub(crate) fn is_ordering(&self, name: &str) -> bool {
        sync::lock(&self.ordering).contains(name)
    }
}

/// A name this process is ordering, until dropped.
pub(crate) struct Ordering {
    acme: Arc<Acme>,
    name: String,
}

impl Drop for Ordering {
    fn drop(&mut self) {
        sync::lock(&self.acme.ordering).remove(&self.name);
    }
}

/// A host name as the challenge rows hold it: lower case, no trailing dot.
pub(crate) fn normal_name(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Whether a ClientHello offers the challenge's protocol.
pub(crate) fn offers_challenge(hello: &rustls::server::ClientHello<'_>) -> bool {
    hello
        .alpn()
        .is_some_and(|mut offered| offered.any(|protocol| protocol == ALPN))
}

/// The certificate settings for an open challenge of `name`. The rows are read again first when the last read is
/// [`READ_EVERY`] old, so a challenge another member wrote or closed is seen within that time (an order waits
/// [`SEEN_BY_EVERY_MEMBER`] before the authority looks); else the challenges as last read. None when `name` has no
/// open challenge.
pub(crate) async fn challenge_config(
    factory: &Weak<ExecutionContextFactory>,
    acme: &Arc<Acme>,
    name: Option<&str>,
) -> Option<Arc<rustls::ServerConfig>> {
    let name = normal_name(name?);
    if acme.claim_read() {
        if let Some(factory) = factory.upgrade() {
            let reader = Arc::clone(acme);
            let read = tokio::task::spawn_blocking(move || store::read_challenges(&factory, &reader)).await;
            if let Ok(Err(err)) = read {
                tracing::error!("The open ACME challenges could not be read: {err}");
            }
        }
    }
    acme.config(&name)
}

/// The refusal of an `acme-tls/1` handshake for a name with no open challenge.
fn refused() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "an ACME challenge handshake for a name with no challenge open",
    )
}

/// What an answered challenge returns in place of a connection to serve: the validator closes it after the handshake
/// (RFC 8737 §3).
fn answered() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "an ACME challenge answered")
}

/// The HTTPS listener's handshake on a node with no edge link, and a local node's passthrough tunnel: `http` for every
/// client but one that offers `acme-tls/1`, which gets the challenge's certificate when the name it asks for has one
/// open and is refused otherwise. An answered challenge is an error too: there is no connection to serve.
pub(crate) async fn accept<S>(
    stream: S,
    http: Arc<rustls::ServerConfig>,
    factory: &Weak<ExecutionContextFactory>,
) -> std::io::Result<tokio_rustls::server::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let acme = factory.upgrade().map(|factory| Acme::of(&factory));
    accept_with(stream, http, factory, acme).await
}

/// [`accept`] with this process's [`Acme`], when there is one.
async fn accept_with<S>(
    stream: S,
    http: Arc<rustls::ServerConfig>,
    factory: &Weak<ExecutionContextFactory>,
    acme: Option<Arc<Acme>>,
) -> std::io::Result<tokio_rustls::server::TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let start = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await?;
    let (challenge, name) = {
        let hello = start.client_hello();
        (offers_challenge(&hello), hello.server_name().map(str::to_string))
    };
    if !challenge {
        return start.into_stream(http).await;
    }
    let Some(acme) = acme else {
        return Err(refused());
    };
    let Some(config) = challenge_config(factory, &acme, name.as_deref()).await else {
        tracing::debug!(
            "Refused an ACME challenge handshake for {}: no challenge open",
            name.as_deref().unwrap_or("(no server name)")
        );
        return Err(refused());
    };
    finish(start.into_stream(config).await?).await;
    Err(answered())
}

/// On an edge node, a ClientHello already read that offers `acme-tls/1` and is not a passthrough host's: answered as
/// [`accept`] answers it. Always an error to return from the accept: there is no connection to serve.
pub(crate) async fn answer<S>(
    accepted: rustls::server::Accepted,
    stream: S,
    factory: &Weak<ExecutionContextFactory>,
    name: Option<&str>,
) -> std::io::Error
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(acme) = factory.upgrade().map(|factory| Acme::of(&factory)) else {
        return refused();
    };
    let Some(config) = challenge_config(factory, &acme, name).await else {
        return refused();
    };
    match tokio_rustls::StartHandshake::from_parts(accepted, stream)
        .into_stream(config)
        .await
    {
        Ok(stream) => {
            finish(stream).await;
            answered()
        }
        Err(err) => err,
    }
}

async fn finish<S>(mut stream: tokio_rustls::server::TlsStream<S>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tracing::info!(
        "Answered an ACME challenge for {}",
        stream.get_ref().1.server_name().unwrap_or("(no server name)")
    );
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.shutdown()).await;
}

/// Start this process's part: each member writes the certificates' files from their rows now and every
/// [`SYNC_EVERY`], and wants the HTTPS listener once a name is set up (on any member), until `shutdown`.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let factory = Arc::downgrade(factory);
    tokio::spawn(async move {
        loop {
            let Some(strong) = factory.upgrade() else {
                return;
            };
            let synced = tokio::task::spawn_blocking(move || {
                let dir = store::cert_dir(&strong);
                if !crate::tls::pairs_in(&dir).is_empty() {
                    Acme::of(&strong).files_found();
                }
                store::sync_files(&strong)
            })
            .await;
            if let Ok(Err(err)) = synced {
                tracing::error!("The ACME certificates' files could not be brought up to date: {err}");
            }
            tokio::select! {
                _ = shutdown.wait_for(|stopped| *stopped) => return,
                () = tokio::time::sleep(SYNC_EVERY) => {}
            }
        }
    });
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
