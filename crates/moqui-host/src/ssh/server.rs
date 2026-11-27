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

//! The built-in SSH server (`plans/archive/Ssh.md`): off unless `ssh-server@port` is set. It listens on `ssh-server@host` (default
//! `127.0.0.1`) with the host key `ssh-server@host-key`, a secret property that is the base64 seed of an Ed25519 key
//! (generated into `runtime/.env` like the other `CHANGEME` secrets) or an OpenSSH private key.
//!
//! Any user may connect, as any user may log in over HTTP; every screen and service is still checked as the user.
//! Login is the user facade: a password through `loginUser` (failures count toward the lockout, and an account that
//! needs a second factor cannot log in here), or a public key whose fingerprint is an active `UserSshKey` of that
//! user. A client with neither (keyboard-interactive, which OpenSSH tries after its keys) is let in with no user and no
//! questions: it gets the terminal client's login dialog, as `moqui tui` shows it, and nothing else
//! (`ssh-server` `login-dialog`, on by default).
//!
//! `apps`, the terminal client (`crate::term`), needs a pty. It is the whole session of a user without `SSH_SHELL`
//! and of one who logged in through the dialog, the `exec` command `apps` (`ssh -t host apps`), and a command at the
//! shell's prompt. A user who logged in by key or password with `SSH_SHELL` gets the admin shell ([`super::shell`]),
//! for `shell` and `exec`. The `sftp` subsystem needs `SSH_SFTP` and serves `ssh-server` `sftp-root`. Git needs `SSH_GIT` (or, with `ssh-server` `git-any-account`, any account logged in by key), and
//! a key unless `ssh-server` `git-password-auth` is true. `agent-apps` (`super::agent`) is the terminal client for a program,
//! on an `exec` channel with no pty, one command line at a time, for a user logged in by key or password.
//!
//! At most `ssh-server` `max-sessions` logged-in sessions are open at once; one with no user counts too. Of those, one
//! user logged in by key or password holds at most `max-sessions-per-user` and one address `max-sessions-per-source`
//! (a cluster peer is counted by neither), so one account or one host does not hold them all. Those with no user, let
//! in for the login dialog, hold at most `max-dialog-sessions`, `max-dialog-sessions-per-source` of them from one
//! address, so connections that need no credentials never fill the sessions a key or password login needs
//! (SEC-FIL-87). A connection that has not logged in has [`PREAUTH_TIMEOUT`] to do so, and at most [`MAX_PREAUTH`]
//! such connections wait at once, at most [`PREAUTH_PER_SOURCE`] of them from one source (an IPv6 /64 is one; an edge
//! tunnel's connection counts as the client the edge node names), so one host cannot keep SFTP, Git, edge tunnels and
//! the SSH cluster transport out. One let in with no user is closed when it has opened no `apps` within [`PREAUTH_TIMEOUT`] of its connection, or its dialog
//! has not logged in within `crate::term::LOGIN_WAIT`.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::source_guard::{SlotRefused, SourceSlot, SourceSlots};
use russh::keys::{HashAlg, PrivateKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId, ChannelOpenFailure};
use russh::{MethodKind, MethodSet};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::sftp_fs::{Backend, Held, OpenChannel, SftpFs};
use crate::command::{interactive, run_apps, shell, with_newlines, AppsAccess};
use crate::worker::on_pool;

const DEFAULT_MAX_SESSIONS: usize = 8;
/// `ssh-server` `max-sessions-per-user`: the sessions one user logged in by key or password holds at once, so one
/// account (any account, with `git-any-account`) does not hold every session.
const DEFAULT_MAX_PER_USER: usize = 4;
/// `ssh-server` `max-sessions-per-source`: the sessions one address (an IPv6 /64) holds at once, under the total so
/// one address does not hold them all.
const DEFAULT_MAX_PER_SOURCE: usize = 4;
/// `ssh-server` `max-dialog-sessions`: the sessions with no user, let in for the login dialog, at once.
const DEFAULT_MAX_DIALOGS: usize = 4;
/// `ssh-server` `max-dialog-sessions-per-source`: of those, the ones one address holds.
const DEFAULT_MAX_DIALOGS_PER_SOURCE: usize = 2;
const DEFAULT_MAX_FILE: usize = 256 * 1024 * 1024;
/// Session channels of one connection waiting for their request or running `sftp`; one more is refused (SEC-FIL-61).
const MAX_SESSION_CHANNELS: usize = 8;
/// The longest `TERM` of a pty request and value of an `env` request a channel keeps (SEC-FIL-88).
const MAX_CHANNEL_TEXT: usize = 256;
/// The connection's inactivity timeout; `apps` ends after as long without input (`crate::term::IDLE`).
pub(super) const IDLE: Duration = Duration::from_secs(600);
/// How long a connection has from accept to a successful login.
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(30);
/// The most characters of a client's user name a log line keeps.
const MAX_LOGGED_NAME: usize = 64;
/// The most characters of a failed login's reason a log line keeps.
const MAX_LOGGED_MESSAGE: usize = 512;

/// Text a client sent (a user name, or a reason that names it) for a log line: cut to `max` characters and quoted
/// with its control characters escaped, so a terminal tailing the log never gets the client's bytes as they came
/// (SEC-FIL-92).
fn for_log(text: &str, max: usize) -> String {
    let cut: String = text.chars().take(max).collect();
    format!("{cut:?}")
}
/// Connections that have not logged in yet, at once; one more is closed. They do not count toward `ssh-server` `max-sessions`.
const MAX_PREAUTH: usize = 16;
/// Of [`MAX_PREAUTH`], those one source may hold (plans/archive/High-Level-Review.md HL-032).
const PREAUTH_PER_SOURCE: usize = 4;

/// The address the server listens on, kept on the factory for `Server::ssh_address`.
#[derive(Default)]
pub struct SshAddress(pub std::sync::Mutex<Option<SocketAddr>>);

/// A running server. Drop stops the listener and disconnects every session.
pub struct SshServer {
    address: SocketAddr,
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl SshServer {
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for SshServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

struct Shared {
    factory: std::sync::Weak<ExecutionContextFactory>,
    root: String,
    max_file: usize,
    /// `dav-server` `sftp-drives`: SFTP to the drives for a user without `SSH_SFTP`.
    sftp_drives: bool,
    /// The live terminal sessions of the process, against `terminal` `max-sessions`.
    #[cfg(feature = "tui")]
    apps: Arc<crate::term::Sessions>,
    /// The sessions `agent-apps open` started, kept between its commands.
    #[cfg(feature = "tui")]
    agents: Arc<super::agent::AgentSessions>,
    /// The cluster topics received, shared with the HTTPS cluster listener.
    cluster_posts: Arc<std::sync::Mutex<Vec<String>>>,
    /// The host key's SHA-256 fingerprint, which a cluster channel's proof is bound to (SEC-CLU-04).
    host_fingerprint: String,
    /// Keyboard-interactive lets a client in with no user, for the login dialog (`ssh-server` `login-dialog`).
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    dialog: bool,
    /// Git over SSH on a connection that logged in by password (`ssh-server` `git-password-auth`).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    git_password_auth: bool,
    /// Git over SSH for any account that logged in by key, with no `SSH_GIT` (`ssh-server` `git-any-account`): each
    /// repository's own check decides what it reads and writes.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    git_any_account: bool,
}

/// The host key from `ssh-server` `host-key`: an OpenSSH private key, or the base64 of a 32-byte Ed25519 seed.
///
/// # Errors
///
/// Returns an error when the attribute is empty, still the placeholder, or does not decode.
pub fn host_key(text: &str) -> Result<PrivateKey, Error> {
    let text = text.trim();
    if text.is_empty() || text.ends_with("CHANGEME") {
        return Err(Error::new(
            "ssh-server@host-key is not set; the SSH server needs a host key",
        ));
    }
    if text.starts_with("-----BEGIN") {
        return russh::keys::decode_secret_key(text, None)
            .map_err(|err| Error::new(format!("ssh-server@host-key does not decode: {err}")));
    }
    use base64::Engine;
    let seed = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| Error::new(format!("ssh-server@host-key is not base64: {err}")))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| Error::new("ssh-server@host-key must be the base64 of 32 bytes, or an OpenSSH private key"))?;
    let keypair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    Ok(PrivateKey::from(keypair))
}

/// The host and port the SSH server will bind by the merged conf, for the port check before the framework is built:
/// read as [`start`] reads them, so None while `ssh-server` `enabled` is false or the port is empty.
pub(crate) fn listen_target(conf: &MNode) -> Result<Option<(String, u16)>, Error> {
    let node = conf.first_named("ssh-server");
    configured_target(&|name: &str| {
        node.as_ref()
            .and_then(|node| node.attribute(name))
            .map(|raw| raw.trim().to_string())
            .unwrap_or_default()
    })
}

/// The host and port of `port_text` and `host_text` (default `127.0.0.1`); None when `port_text` is empty.
fn listen_target_of(port_text: &str, host_text: &str) -> Result<Option<(String, u16)>, Error> {
    let port_text = port_text.trim();
    if port_text.is_empty() {
        return Ok(None);
    }
    let port: u16 = port_text
        .parse()
        .map_err(|_| Error::new(format!("ssh-server port {port_text} is not a port number")))?;
    let host = match host_text.trim() {
        "" => "127.0.0.1".to_string(),
        other => other.to_string(),
    };
    Ok(Some((host, port)))
}

/// [`listen_target_of`], or None when the conf's `ssh-server` `enabled` is false, whatever the port.
fn conf_listen_target(enabled: bool, port_text: &str, host_text: &str) -> Result<Option<(String, u16)>, Error> {
    if !enabled {
        return Ok(None);
    }
    listen_target_of(port_text, host_text)
}

/// [`conf_listen_target`] by the conf's `ssh-server` attributes (`attr`, expanded) `enabled`, `port` and `host`;
/// `enabled` is on unless it is `false`, as `conf_flag` reads it.
fn configured_target(attr: &dyn Fn(&str) -> String) -> Result<Option<(String, u16)>, Error> {
    conf_listen_target(attr("enabled") != "false", &attr("port"), &attr("host"))
}

/// Start the server when the conf's `ssh-server` `port` is set (`0` binds an ephemeral port) and its
/// `enabled` is not false.
///
/// # Errors
///
/// Returns an error when the host key, the address, or the bind fails.
pub fn start(
    factory: &Arc<ExecutionContextFactory>,
    runtime_dir: &std::path::Path,
    cluster_posts: Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<Option<SshServer>, Error> {
    moqui_lib::env::set_property(moqui_core::cluster::SSH_FINGERPRINT_PROPERTY, "");
    moqui_lib::env::set_property(moqui_core::cluster::SSH_ADDRESS_PROPERTY, "");
    let attr = |name: &str| factory.conf_setting("ssh-server", name);
    let Some((host, port)) = configured_target(&attr)? else {
        return Ok(None);
    };
    // by its handle: the conf's, else seal's store, where it is generated (plans/Sealed-Secrets.md S4)
    let key = host_key(
        &factory
            .secret("ssh-server@host-key")
            .map_err(|err| Error::new(err.to_string()))?
            .unwrap_or_default(),
    )?;
    let root = match factory.conf_setting("ssh-server", "sftp-root").as_str() {
        "" => "runtime://sftp".to_string(),
        other => other.to_string(),
    };
    if root.starts_with("runtime:") {
        let relative = root.trim_start_matches("runtime://").trim_start_matches("runtime:");
        let _ = std::fs::create_dir_all(runtime_dir.join(relative.trim_start_matches('/')));
    }
    let max_sessions = positive(
        factory.conf_number("ssh-server", "max-sessions", DEFAULT_MAX_SESSIONS),
        DEFAULT_MAX_SESSIONS,
    );
    // 0 is no share: the total alone bounds a user or a source
    let max_per_user = factory.conf_number("ssh-server", "max-sessions-per-user", DEFAULT_MAX_PER_USER);
    let max_per_source = factory.conf_number("ssh-server", "max-sessions-per-source", DEFAULT_MAX_PER_SOURCE);
    let max_dialogs = factory.conf_number("ssh-server", "max-dialog-sessions", DEFAULT_MAX_DIALOGS);
    let max_dialogs_per_source = factory.conf_number(
        "ssh-server",
        "max-dialog-sessions-per-source",
        DEFAULT_MAX_DIALOGS_PER_SOURCE,
    );
    let max_file = positive(
        factory.conf_number("ssh-server", "sftp-max-file", DEFAULT_MAX_FILE),
        DEFAULT_MAX_FILE,
    );
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    // "none" is not offered, so a client tries its keys first; keyboard-interactive, which OpenSSH tries next, is the
    // login dialog, and only with the terminal client.
    let dialog = cfg!(feature = "tui") && factory.conf_flag("ssh-server", "login-dialog", true);
    let git_password_auth = factory.conf_flag("ssh-server", "git-password-auth", false);
    let git_any_account = factory.conf_flag("ssh-server", "git-any-account", false);
    let methods: &[MethodKind] = if dialog {
        &[
            MethodKind::PublicKey,
            MethodKind::KeyboardInteractive,
            MethodKind::Password,
        ]
    } else {
        &[MethodKind::PublicKey, MethodKind::Password]
    };
    let config = Arc::new(russh::server::Config {
        keys: vec![key],
        methods: MethodSet::from(methods),
        inactivity_timeout: Some(IDLE),
        auth_rejection_time: Duration::from_millis(500),
        auth_rejection_time_initial: Some(Duration::from_millis(0)),
        max_auth_attempts: 6,
        nodelay: true,
        ..russh::server::Config::default()
    });
    let bind = format!("{host}:{port}");
    let listener = crate::edge::block_on(async move { TcpListener::bind(bind).await })?
        .map_err(|err| Error::new(format!("could not bind the SSH server to {host}:{port}: {err}")))?;
    let address = listener
        .local_addr()
        .map_err(|err| Error::new("the SSH listener has no address").with_source(err))?;
    let (shutdown, watch) = tokio::sync::watch::channel(false);
    let shared = Arc::new(Shared {
        factory: Arc::downgrade(factory),
        root,
        max_file,
        sftp_drives: factory.conf_flag("dav-server", "sftp-drives", true),
        #[cfg(feature = "tui")]
        apps: crate::term::Sessions::of(factory),
        #[cfg(feature = "tui")]
        agents: Arc::default(),
        cluster_posts,
        host_fingerprint: fingerprint.clone(),
        dialog,
        git_password_auth,
        git_any_account,
    });
    let intake = Arc::new(Intake {
        config,
        shared,
        counts: Arc::new(
            Counts::new(max_sessions, max_per_user, max_per_source).with_dialogs(max_dialogs, max_dialogs_per_source),
        ),
        shutdown: watch,
    });
    *moqui_lib::sync::lock(&factory.extension(|_| IntakeSlot::default()).0) = Some(Arc::clone(&intake));
    crate::edge::handle().spawn(accept_loop(listener, intake));
    *moqui_lib::sync::lock(&factory.extension(|_| SshAddress::default()).0) = Some(address);
    // For the cluster member row: peers pin this key, and the ssh:// base URL defaults to this address.
    moqui_lib::env::set_property(moqui_core::cluster::SSH_FINGERPRINT_PROPERTY, fingerprint);
    moqui_lib::env::set_property(moqui_core::cluster::SSH_ADDRESS_PROPERTY, address.to_string());
    Ok(Some(SshServer { address, shutdown }))
}

/// `value`, or `default` when it is 0.
fn positive(value: usize, default: usize) -> usize {
    if value == 0 {
        default
    } else {
        value
    }
}

/// The connections counted against the two limits.
struct Counts {
    preauth: SourceSlots,
    sessions: AtomicUsize,
    max_sessions: usize,
    /// The sessions of each source, against `ssh-server` `max-sessions-per-source`.
    by_source: SourceSlots,
    /// The sessions of each user logged in by key or password, against `ssh-server` `max-sessions-per-user`.
    by_user: std::sync::Mutex<HashMap<String, usize>>,
    max_per_user: usize,
    /// The sessions with no user, against `ssh-server` `max-dialog-sessions` and `max-dialog-sessions-per-source`.
    dialogs: SourceSlots,
}

impl Counts {
    fn new(max_sessions: usize, max_per_user: usize, max_per_source: usize) -> Self {
        Self {
            preauth: SourceSlots::new(MAX_PREAUTH, PREAUTH_PER_SOURCE),
            sessions: AtomicUsize::new(0),
            max_sessions,
            // the total is `max_sessions`' to keep: this table only keeps each source's share
            by_source: SourceSlots::new(usize::MAX, max_per_source),
            by_user: std::sync::Mutex::default(),
            max_per_user,
            dialogs: SourceSlots::new(usize::MAX, 0),
        }
    }

    /// With the share of the sessions with no user: at most `total` (0 for no share but `max-sessions`), and
    /// `per_source` of them from one source (0 for no share).
    fn with_dialogs(mut self, total: usize, per_source: usize) -> Self {
        let total = if total == 0 { usize::MAX } else { total };
        self.dialogs = SourceSlots::new(total, per_source);
        self
    }
}

/// Who a session is for, which says what it is counted against.
#[derive(Clone, Copy, Debug)]
enum Who<'a> {
    /// A user logged in by key or password: the user's share and the source's.
    User(&'a str),
    /// Let in with no user, for the login dialog: the source's share.
    Dialog,
    /// A cluster peer with the cluster key: `max-sessions` only, as the members of a cluster share addresses.
    Cluster,
}

/// Why a login got no session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionRefused {
    /// `ssh-server` `max-sessions` are open.
    Total,
    /// The user has `max-sessions-per-user` open.
    User,
    /// The source has `max-sessions-per-source` open.
    Source,
    /// `max-dialog-sessions` with no user are open, or the source has `max-dialog-sessions-per-source` of them.
    Dialog,
}

impl SessionRefused {
    /// The conf attribute that refused, for the log.
    fn limit(self) -> &'static str {
        match self {
            Self::Total => "max-sessions",
            Self::User => "max-sessions-per-user",
            Self::Source => "max-sessions-per-source",
            Self::Dialog => "max-dialog-sessions",
        }
    }
}

/// What a session holds beside its place in `max-sessions`, each given back as it drops.
#[derive(Default)]
struct Shares {
    /// Its source's share (`max-sessions-per-source`), held for its drop, which gives it back.
    #[allow(dead_code)]
    source: Option<SourceSlot>,
    /// Its user's share (`max-sessions-per-user`): a key or password login's, or a dialog's once it logged in.
    user: Option<String>,
    /// With no user, its place among the dialogs (`max-dialog-sessions`), until the dialog logs in.
    dialog: Option<SourceSlot>,
}

/// One connection's place in [`Counts`]: a pre-auth slot from accept, traded for a session at login. Drop gives back
/// whichever it holds.
struct Slot {
    counts: Arc<Counts>,
    /// The pre-auth slot, until the login trades it for a session.
    preauth: std::sync::Mutex<Option<SourceSlot>>,
    session: AtomicBool,
    source: IpAddr,
    /// What the session holds beside its place in `max-sessions`.
    shares: std::sync::Mutex<Shares>,
    /// Let in with no user: the login dialog must start within [`PREAUTH_TIMEOUT`] and log in within
    /// `crate::term::LOGIN_WAIT`.
    dialog: AtomicBool,
    /// The connection let in with no user has started its `apps`, the login dialog.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    dialog_started: AtomicBool,
    /// The dialog logged in ([`DialogLogin::done`](crate::term::DialogLogin::done)).
    dialog_done: AtomicBool,
}

impl Slot {
    /// A pre-auth slot for a connection from `source`, or why there is none: [`MAX_PREAUTH`] connections are already
    /// waiting to log in, or [`PREAUTH_PER_SOURCE`] from that source.
    fn preauth(counts: &Arc<Counts>, source: IpAddr) -> Result<Self, SlotRefused> {
        let preauth = counts.preauth.take(source)?;
        Ok(Self {
            counts: Arc::clone(counts),
            preauth: std::sync::Mutex::new(Some(preauth)),
            session: AtomicBool::new(false),
            source,
            shares: std::sync::Mutex::default(),
            dialog: AtomicBool::new(false),
            dialog_started: AtomicBool::new(false),
            dialog_done: AtomicBool::new(false),
        })
    }

    /// Trade the pre-auth slot for a session.
    ///
    /// # Errors
    ///
    /// [`SessionRefused`] when `ssh-server` `max-sessions` are open, or the user or the source has its share, or
    /// with no user the dialogs have theirs.
    fn log_in(&self, who: Who<'_>) -> Result<(), SessionRefused> {
        if self.session.load(Ordering::Acquire) {
            return Ok(());
        }
        let counts = &self.counts;
        let dialog = match who {
            Who::Dialog => Some(counts.dialogs.take(self.source).map_err(|_| SessionRefused::Dialog)?),
            Who::User(_) | Who::Cluster => None,
        };
        let source = match who {
            Who::Cluster => None,
            Who::User(_) | Who::Dialog => Some(counts.by_source.take(self.source).map_err(|_| SessionRefused::Source)?),
        };
        let user = match who {
            Who::User(name) => {
                if !self.take_user(name) {
                    return Err(SessionRefused::User);
                }
                Some(name.to_string())
            }
            Who::Dialog | Who::Cluster => None,
        };
        if counts.sessions.fetch_add(1, Ordering::AcqRel) >= counts.max_sessions {
            counts.sessions.fetch_sub(1, Ordering::AcqRel);
            self.release_user(user.as_deref());
            return Err(SessionRefused::Total);
        }
        *moqui_lib::sync::lock(&self.shares) = Shares { source, user, dialog };
        self.session.store(true, Ordering::Release);
        moqui_lib::sync::lock(&self.preauth).take();
        Ok(())
    }

    /// Take `user`'s share of the sessions, as a key or password login does: false when the user has
    /// `max-sessions-per-user` open.
    fn take_user(&self, user: &str) -> bool {
        let counts = &self.counts;
        let mut by_user = moqui_lib::sync::lock(&counts.by_user);
        let held = by_user.entry(user.to_string()).or_insert(0);
        if counts.max_per_user > 0 && *held >= counts.max_per_user {
            if *held == 0 {
                by_user.remove(user);
            }
            return false;
        }
        *held += 1;
        true
    }

    /// Give a user's share back.
    fn release_user(&self, user: Option<&str>) {
        let Some(user) = user else {
            return;
        };
        let mut by_user = moqui_lib::sync::lock(&self.counts.by_user);
        if let Some(held) = by_user.get_mut(user) {
            *held = held.saturating_sub(1);
            if *held == 0 {
                by_user.remove(user);
            }
        }
    }

    fn logged_in(&self) -> bool {
        self.session.load(Ordering::Acquire)
    }
}

/// A dialog's login (SEC-FIL-87): the user's share of the sessions is taken as a key or password login takes it, and
/// once logged in the connection gives back its place among the dialogs, so logged-in dialogs do not fill
/// `max-dialog-sessions` and a user's dialog sessions count toward `max-sessions-per-user`. The `apps` session holds
/// it by a weak reference: the connection's place goes when the connection does, whatever the session does after.
#[derive(Debug)]
struct SlotLogin(std::sync::Weak<Slot>);

impl crate::term::DialogLogin for SlotLogin {
    fn take(&self, username: &str) -> Result<(), String> {
        self.0.upgrade().map_or(Ok(()), |slot| slot.take_dialog_user(username))
    }

    fn give_back(&self) {
        if let Some(slot) = self.0.upgrade() {
            slot.give_back_dialog_user();
        }
    }

    fn done(&self) {
        if let Some(slot) = self.0.upgrade() {
            slot.dialog_logged_in();
        }
    }
}

impl Slot {
    /// A dialog's login: take `username`'s share, the reason it is refused.
    fn take_dialog_user(&self, username: &str) -> Result<(), String> {
        let mut shares = moqui_lib::sync::lock(&self.shares);
        if shares.user.is_some() {
            return Ok(());
        }
        if !self.take_user(username) {
            return Err(format!(
                "{username} has ssh-server max-sessions-per-user sessions open; close one and log in again"
            ));
        }
        shares.user = Some(username.to_string());
        Ok(())
    }

    /// A dialog's login failed: give its user's share back.
    fn give_back_dialog_user(&self) {
        let user = moqui_lib::sync::lock(&self.shares).user.take();
        self.release_user(user.as_deref());
    }

    /// A dialog logged in: its place among the dialogs goes back.
    fn dialog_logged_in(&self) {
        self.dialog_done.store(true, Ordering::Release);
        moqui_lib::sync::lock(&self.shares).dialog = None;
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.session.swap(false, Ordering::AcqRel) {
            self.counts.sessions.fetch_sub(1, Ordering::AcqRel);
            // the source's share goes back as its slot drops
            let shares = std::mem::take(&mut *moqui_lib::sync::lock(&self.shares));
            self.release_user(shares.user.as_deref());
        }
    }
}

/// Resolves [`PREAUTH_TIMEOUT`] after `since` when the connection has not logged in by then, and never otherwise.
async fn preauth_expired(slot: &Slot, since: tokio::time::Instant) {
    tokio::time::sleep_until(since + PREAUTH_TIMEOUT).await;
    if slot.logged_in() {
        std::future::pending::<()>().await;
    }
}

/// Resolves when the connection was let in with no user and has started no `apps` within [`PREAUTH_TIMEOUT`] of
/// `since` (SEC-FIL-87), or its dialog has not logged in within the login wait, so a client that opens no session, or
/// leaves the dialog, does not hold a session slot; never otherwise.
async fn dialog_expired(slot: &Slot, since: tokio::time::Instant) {
    #[cfg(feature = "tui")]
    dialog_expired_within(slot, since, PREAUTH_TIMEOUT, crate::term::LOGIN_WAIT).await;
    #[cfg(not(feature = "tui"))]
    let _ = (slot, since);
    std::future::pending::<()>().await;
}

/// [`dialog_expired`] with its two waits: `start` for the dialog's `apps`, `login` for its login (a test's own).
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
async fn dialog_expired_within(slot: &Slot, since: tokio::time::Instant, start: Duration, login: Duration) {
    tokio::time::sleep_until(since + start).await;
    if slot.dialog.load(Ordering::Acquire) && !slot.dialog_started.load(Ordering::Acquire) {
        return;
    }
    tokio::time::sleep_until(since + login).await;
    if slot.dialog.load(Ordering::Acquire) && !slot.dialog_done.load(Ordering::Acquire) {
        return;
    }
    std::future::pending::<()>().await;
}

/// What serves one connection: the server's settings, its shared state and counts, and its stop. The listener hands
/// it each TCP connection, and an edge link's SSH tunnel hands it a tunnel stream with the client's address
/// ([`serve_stream`], plans/archive/Edge-Node.md §5.3).
struct Intake {
    config: Arc<russh::server::Config>,
    shared: Arc<Shared>,
    counts: Arc<Counts>,
    shutdown: tokio::sync::watch::Receiver<bool>,
}

/// The server's [`Intake`], kept on the factory while it runs.
#[derive(Default)]
struct IntakeSlot(std::sync::Mutex<Option<Arc<Intake>>>);

/// Serve `stream` as a connection from `peer` to this factory's SSH server, as one the listener accepted: the edge
/// link's SSH tunnel. False when no SSH server runs.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn serve_stream<S>(factory: &ExecutionContextFactory, stream: S, peer: SocketAddr) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let intake = moqui_lib::sync::lock(&factory.extension(|_| IntakeSlot::default()).0).clone();
    match intake {
        Some(intake) if !*intake.shutdown.borrow() => {
            intake.serve(stream, peer, true);
            true
        }
        _ => false,
    }
}

/// Whether this factory's SSH server runs, to take [`serve_stream`].
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn running(factory: &ExecutionContextFactory) -> bool {
    moqui_lib::sync::lock(&factory.extension(|_| IntakeSlot::default()).0)
        .as_ref()
        .is_some_and(|intake| !*intake.shutdown.borrow())
}

async fn accept_loop(listener: TcpListener, intake: Arc<Intake>) {
    let mut shutdown = intake.shutdown.clone();
    loop {
        let accepted = tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, peer)) = accepted else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        intake.serve(stream, peer, false);
    }
}

impl Intake {
    /// Serve one connection from `peer` in a task of its own, or close it when too many wait to log in. `via_edge`: a
    /// tunnel stream an edge link handed over, `peer` the client the edge node names.
    fn serve<S>(&self, stream: S, peer: SocketAddr, via_edge: bool)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let slot = match Slot::preauth(&self.counts, peer.ip()) {
            Ok(slot) => slot,
            Err(SlotRefused::Full) => {
                tracing::warn!("SSH connection from {peer} refused: {MAX_PREAUTH} connections are waiting to log in");
                return;
            }
            Err(SlotRefused::Source) => {
                tracing::warn!(
                    "SSH connection from {peer} refused: {PREAUTH_PER_SOURCE} from there are waiting to log in"
                );
                return;
            }
        };
        let slot = Arc::new(slot);
        let since = tokio::time::Instant::now();
        let handler = Connection {
            shared: Arc::clone(&self.shared),
            slot: Arc::clone(&slot),
            peer,
            via_edge,
            username: None,
            logged_in_at: None,
            login_name: None,
            dialog_used: false,
            by_password: false,
            by_app_password: false,
            cluster: false,
            can_sftp: false,
            can_shell: false,
            can_git: false,
            channels: HashMap::new(),
            sftp_open: Arc::new(AtomicUsize::new(0)),
            ptys: HashMap::new(),
            terminal_env: HashMap::new(),
            git_protocol: HashMap::new(),
        };
        let config = Arc::clone(&self.config);
        let mut stop = self.shutdown.clone();
        tokio::spawn(async move {
            // The version exchange waits on the client too, so the login deadline covers it.
            let deadline = since + PREAUTH_TIMEOUT;
            match tokio::time::timeout_at(deadline, russh::server::run_stream(config, stream, handler)).await {
                Ok(Ok(session)) => {
                    let handle = session.handle();
                    tokio::select! {
                        _ = stop.changed() => {
                            let _ = handle.disconnect(russh::Disconnect::ByApplication, "server stopping".into(), String::new()).await;
                        }
                        () = preauth_expired(&slot, since) => {
                            tracing::debug!("SSH connection from {peer} did not log in within {PREAUTH_TIMEOUT:?}");
                            let _ = handle.disconnect(russh::Disconnect::ByApplication, "login timed out".into(), String::new()).await;
                        }
                        () = dialog_expired(&slot, since) => {
                            tracing::info!("SSH connection from {peer} did not log in through the dialog in time");
                            let _ = handle.disconnect(russh::Disconnect::ByApplication, "login timed out".into(), String::new()).await;
                        }
                        result = session => {
                            if let Err(err) = result {
                                tracing::debug!("SSH session from {peer} ended: {err:?}");
                            }
                        }
                    }
                }
                Ok(Err(err)) => tracing::debug!("SSH connection from {peer} failed: {err:?}"),
                Err(_) => tracing::debug!("SSH connection from {peer} sent no version within {PREAUTH_TIMEOUT:?}"),
            }
            drop(slot);
        });
    }
}

/// One client connection.
struct Connection {
    shared: Arc<Shared>,
    /// This connection's place in the pre-auth and session counts.
    slot: Arc<Slot>,
    peer: SocketAddr,
    /// Through an edge node's SSH tunnel: `peer` is the client the edge node names, never this machine (SEC-CLU-01),
    /// and no cluster peer comes this way (SEC-CLU-03).
    via_edge: bool,
    /// The user logged in by key or password; `None` before, and for a connection let in for the login dialog.
    username: Option<String>,
    /// When [`Self::username`] logged in: the sign-in `sh` and `attach` hold to `terminal` `remote-reauth-minutes`
    /// (SEC-FIL-97).
    #[cfg_attr(not(all(feature = "devtools", feature = "tui", unix)), allow(dead_code))]
    logged_in_at: Option<std::time::Instant>,
    /// Let in with no user (keyboard-interactive): the name the client gave, which the login dialog starts with. Such
    /// a connection gets `apps` on a pty and nothing else.
    login_name: Option<String>,
    /// The connection let in for the login dialog has started its one `apps`, so its channels cannot each hold a
    /// session with no user.
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    dialog_used: bool,
    /// The user logged in by password, not a key: git is refused unless `ssh-server` `git-password-auth`.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    by_password: bool,
    /// The user logged in with an app password: a device's credential, which no second factor guards, so the login
    /// gets the drives over SFTP and nothing else (no admin shell, no `sftp-root`, no git, no `apps` or `agent-apps`;
    /// SEC-FIL-02).
    by_app_password: bool,
    /// A cluster peer logged in with the cluster key: it may open `cluster <topic>` channels and nothing else.
    cluster: bool,
    can_sftp: bool,
    can_shell: bool,
    /// `git-upload-pack` and `git-receive-pack` (`SSH_GIT`); each repository is still checked. Unread without the `git`
    /// feature, where git commands are not served.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    can_git: bool,
    channels: HashMap<ChannelId, Channel<Msg>>,
    /// The `sftp` subsystems running on the connection ([`MAX_SESSION_CHANNELS`]).
    sftp_open: Arc<AtomicUsize>,
    /// The channels that asked for a pty, with its terminal and window size.
    ptys: HashMap<ChannelId, Pty>,
    /// The `env` variables of each channel that say how a terminal draws (`crate::term::is_terminal_variable`).
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    terminal_env: HashMap<ChannelId, BTreeMap<String, String>>,
    /// `GIT_PROTOCOL` of each channel's `env` request.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    git_protocol: HashMap<ChannelId, String>,
}

/// A channel's pty: its `TERM`, and its window size, which `apps` follows.
struct Pty {
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    term: String,
    size: tokio::sync::watch::Sender<(u16, u16)>,
}

/// What `agent-apps` gets on a connection: the sessions and the user, or why not.
#[cfg(feature = "tui")]
type AgentAccess = Result<super::agent::AgentAccess, &'static str>;
#[cfg(not(feature = "tui"))]
type AgentAccess = Result<(), &'static str>;

/// The SSH permissions of the user now logged in. Without any of them the user still gets `apps`.
#[derive(Clone, Copy, Debug)]
struct Permissions {
    sftp: bool,
    shell: bool,
    git: bool,
}

/// How a user logged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoginBy {
    Key,
    Password,
    /// An app password ([`Connection::by_app_password`]).
    AppPassword,
}

/// What a login by app password is told when it asks for anything but SFTP to the drives.
const DRIVES_ONLY: &str = "an app password opens the drives over SFTP only; log in with a key or the account password";

fn ssh_permissions() -> Result<Permissions, CoreError> {
    Ok(Permissions {
        sftp: moqui_core::user::has_permission("SSH_SFTP")?,
        shell: moqui_core::user::has_permission("SSH_SHELL")?,
        git: moqui_core::user::has_permission("SSH_GIT")?,
    })
}

/// The SSH permissions of `username` now, read again at each channel request (SEC-FIL-11), so a permission taken away
/// refuses what an open connection asks for next; `None` for an account that is gone or may no longer sign in
/// (disabled, locked, terminated, a display account, a password change due: [`moqui_core::user::account_may_sign_in`],
/// which changes nothing).
fn permissions_of(factory: &ExecutionContextFactory, username: &str) -> Result<Option<Permissions>, CoreError> {
    // read only: an internal login would reset the account's failed-login count at every request (SEC-FIL-11)
    if !moqui_core::user::account_may_sign_in(username)? {
        return Ok(None);
    }
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let facade = factory
        .entity()
        .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
    let Some(account) = facade
        .find("moqui.security.UserAccount")?
        .eq("username", Value::string(username))
        .one()?
    else {
        return Ok(None);
    };
    let user_id = account
        .get("userId")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    let has = |permission: &str| moqui_core::user::user_has_permission(&user_id, permission);
    Ok(Some(Permissions {
        sftp: has("SSH_SFTP")?,
        shell: has("SSH_SHELL")?,
        git: has("SSH_GIT")?,
    }))
}

/// How a connection logged in, and what the conf says of git over SSH.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
#[derive(Clone, Copy, Debug)]
struct GitLogin {
    /// The user has `SSH_GIT`.
    has_permission: bool,
    by_password: bool,
    /// `ssh-server` `git-any-account`.
    any_account: bool,
    /// `ssh-server` `git-password-auth`.
    password_auth: bool,
}

/// Whether a connection is served git.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitAccess {
    Served,
    /// The user may, by key only: this login was by password.
    NeedsKey,
    Refused,
}

/// `SSH_GIT`, or with `git-any-account` any account that proved a key. A password login gains nothing from
/// `git-any-account`: it needs `SSH_GIT` and `git-password-auth` as before. Sftp and the shell are not asked here.
#[cfg_attr(not(feature = "git"), allow(dead_code))]
fn git_allowed(login: GitLogin) -> GitAccess {
    let permitted = login.has_permission || (login.any_account && !login.by_password);
    if !permitted {
        GitAccess::Refused
    } else if login.by_password && !login.password_auth {
        GitAccess::NeedsKey
    } else {
        GitAccess::Served
    }
}

/// What a connection let in for the login dialog is told when it asks for anything but `apps` on a pty.
const DIALOG_ONLY: &str = "log in with a key or a password for commands; ssh -t opens the login screen";

/// A window size from the SSH request's `u32`s; 0 (not given) is 80 by 24.
fn window(columns: u32, rows: u32) -> (u16, u16) {
    let fit = |value: u32, default: u16| match u16::try_from(value) {
        Ok(0) => default,
        Ok(value) => value,
        Err(_) => u16::MAX,
    };
    (fit(columns, 80), fit(rows, 24))
}

impl Connection {
    /// Remove `channel` from the per-channel maps: its open handle, pty, terminal variables and `GIT_PROTOCOL`.
    fn forget_channel(&mut self, channel: ChannelId) {
        self.channels.remove(&channel);
        self.ptys.remove(&channel);
        self.terminal_env.remove(&channel);
        self.git_protocol.remove(&channel);
    }

    /// Read the user's SSH permissions again ([`permissions_of`]) before a channel request is answered: false, with
    /// none kept, when the account may no longer sign in (an app password's login too) or the read fails; a login by
    /// app password keeps no permission either way.
    async fn refresh_permissions(&mut self) -> bool {
        let Some(username) = self.username.clone() else {
            return true;
        };
        let read = on_pool(&self.shared.factory, move |factory| permissions_of(factory, &username)).await;
        let read = read.unwrap_or_else(|err| {
            tracing::warn!(
                "SSH permissions of a connection from {} not read: {}",
                self.peer,
                err.message()
            );
            None
        });
        let signs_in = read.is_some();
        let none = Permissions {
            sftp: false,
            shell: false,
            git: false,
        };
        let Permissions { sftp, shell, git } = read.filter(|_| !self.by_app_password).unwrap_or(none);
        self.can_sftp = sftp;
        self.can_shell = shell;
        self.can_git = git;
        if !signs_in {
            tracing::info!(
                "SSH connection from {}: its account may no longer sign in; refused",
                self.peer
            );
        }
        signs_in
    }

    /// Let the user in, with what the permissions allow; every user gets `apps`, but for a login by app password, which
    /// gets the drives over SFTP only.
    fn accept(&mut self, username: &str, permissions: Permissions, by: LoginBy) -> Auth {
        let by_app_password = by == LoginBy::AppPassword;
        let Permissions { sftp, shell, git } = if by_app_password {
            Permissions {
                sftp: false,
                shell: false,
                git: false,
            }
        } else {
            permissions
        };
        if let Err(refused) = self.slot.log_in(Who::User(username)) {
            tracing::warn!(
                "SSH login of {username} from {} refused: ssh-server {} sessions are open",
                self.peer,
                refused.limit()
            );
            return Auth::reject();
        }
        tracing::info!("SSH login of {username} from {}", self.peer);
        self.username = Some(username.to_string());
        self.logged_in_at = Some(std::time::Instant::now());
        self.by_password = by != LoginBy::Key;
        self.by_app_password = by_app_password;
        self.can_sftp = sftp;
        self.can_shell = shell;
        self.can_git = git;
        Auth::Accept
    }

    /// Whether a channel may be opened: a user logged in, or a connection let in for the login dialog.
    fn let_in(&self) -> bool {
        self.username.is_some() || self.login_name.is_some()
    }

    /// `agent-apps` for this connection: a user logged in by key or password, and a build with the client. No pty is
    /// needed.
    fn agent_access(&self) -> AgentAccess {
        if self.username.is_none() {
            return Err(DIALOG_ONLY);
        }
        if self.by_app_password {
            return Err(DRIVES_ONLY);
        }
        #[cfg(feature = "tui")]
        {
            Ok(super::agent::AgentAccess {
                factory: std::sync::Weak::clone(&self.shared.factory),
                sessions: Arc::clone(&self.shared.apps),
                agents: Arc::clone(&self.shared.agents),
                username: self.username.clone().unwrap_or_default(),
            })
        }
        #[cfg(not(feature = "tui"))]
        {
            Err("this build has no terminal UI")
        }
    }

    /// `sh` on `channel` (plans/Terminal-Fancy.md §6): a pty, and a build with host sessions; who may and from where
    /// is asked when it is typed, by the peer's address.
    #[cfg(all(feature = "devtools", feature = "tui", unix))]
    fn host_access(&self, channel: ChannelId, username: &str) -> crate::command::HostCommand {
        let pty = self.ptys.get(&channel)?;
        Some(crate::command::host::HostAccess {
            factory: std::sync::Weak::clone(&self.shared.factory),
            username: username.to_string(),
            size: pty.size.subscribe(),
            facts: crate::term::access::Facts::over_ssh(self.peer.ip(), self.via_edge),
            session_id: format!("ssh-{}", self.peer),
            signed_in: self.logged_in_at,
        })
    }

    #[cfg(not(all(feature = "devtools", feature = "tui", unix)))]
    fn host_access(&self, _channel: ChannelId, _username: &str) -> crate::command::HostCommand {
        crate::command::HostCommand
    }

    /// `apps` on `channel`: a pty, and a build with the client. It runs as the user logged in, or with the login
    /// dialog first, once per connection.
    fn apps_access(&mut self, channel: ChannelId) -> AppsAccess {
        if self.by_app_password {
            return Err(DRIVES_ONLY);
        }
        let Some(pty) = self.ptys.get(&channel) else {
            return Err("apps needs a terminal: connect with ssh -t");
        };
        #[cfg(feature = "tui")]
        {
            let who = match (&self.username, &self.login_name) {
                (Some(username), _) => crate::term::Who::User(username.clone()),
                (None, Some(_)) if self.dialog_used => return Err("one login screen per connection"),
                (None, Some(name)) => crate::term::Who::Login {
                    name: Some(name.clone()).filter(|name| !name.is_empty()),
                    done: Some(Arc::new(SlotLogin(Arc::downgrade(&self.slot)))),
                },
                (None, None) => return Err(DIALOG_ONLY),
            };
            if self.username.is_none() {
                self.dialog_used = true;
                self.slot.dialog_started.store(true, Ordering::Release);
            }
            Ok(crate::term::Start {
                factory: std::sync::Weak::clone(&self.shared.factory),
                sessions: Arc::clone(&self.shared.apps),
                who,
                terminal: crate::term::Terminal {
                    term: pty.term.clone(),
                    env: self.terminal_env.get(&channel).cloned().unwrap_or_default(),
                },
                size: pty.size.subscribe(),
                label: format!("SSH apps from {}", self.peer),
                client_ip: self.peer.ip().to_string(),
                admission: crate::term::Admission::Take,
                redraw: None,
            })
        }
        #[cfg(not(feature = "tui"))]
        {
            let _ = pty;
            Err("this build has no terminal UI")
        }
    }
}

/// Run `agent-apps` with `args` on the stream (one command, or the line mode with none), or write why it cannot run.
/// Returns the exit status.
async fn run_agent<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    access: AgentAccess,
    args: &str,
) -> u32 {
    match access {
        #[cfg(feature = "tui")]
        Ok(access) => match super::agent::one_shot(&access, args).await {
            Some((text, status)) => {
                let _ = stream.write_all(format!("{text}\n").as_bytes()).await;
                status
            }
            None => super::agent::lines(stream, &access).await,
        },
        #[cfg(not(feature = "tui"))]
        Ok(()) => {
            let _ = args;
            2
        }
        Err(reason) => {
            let _ = stream.write_all(format!("--- error {reason}\n").as_bytes()).await;
            2
        }
    }
}

/// Accept `channel` only to write `reason` on it and end it with exit status 1.
fn refuse_channel(
    open: Option<Channel<Msg>>,
    channel: ChannelId,
    session: &mut Session,
    reason: &'static str,
) -> Result<(), russh::Error> {
    let Some(open) = open else {
        return session.channel_failure(channel);
    };
    session.channel_success(channel)?;
    let handle = session.handle();
    tokio::spawn(async move {
        let mut stream = open.into_stream();
        let _ = stream.write_all(format!("{reason}\n").as_bytes()).await;
        let _ = stream.flush().await;
        let _ = handle.exit_status_request(channel, 1).await;
        let _ = handle.eof(channel).await;
        let _ = handle.close(channel).await;
    });
    Ok(())
}

impl russh::server::Handler for Connection {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let (username, secret) = (user.to_string(), password.to_string());
        let client_ip = self.peer.ip().to_string();
        let checked = on_pool(&self.shared.factory, move |_| {
            // an app password, the device password SFTP to the drives takes, before the account password
            let by_app_password = moqui_core::user::looks_like_app_password(&secret)
                && moqui_core::user::with_client_ip(&client_ip, || {
                    moqui_core::user::login_app_password(&username, &secret, "ssh")
                })
                .is_ok();
            if !by_app_password {
                moqui_core::user::with_client_ip(&client_ip, || moqui_core::user::login_user(&username, &secret))?;
            }
            if moqui_core::user::current_user_id()?.is_none_or(|id| id.is_empty()) {
                return Err(CoreError::new("a second factor is required, which SSH does not take"));
            }
            let by = if by_app_password {
                LoginBy::AppPassword
            } else {
                LoginBy::Password
            };
            Ok((ssh_permissions()?, by))
        })
        .await;
        match checked {
            Ok((permissions, by)) => Ok(self.accept(user, permissions, by)),
            Err(err) => {
                tracing::warn!(
                    "SSH password login of {} from {} failed: {}",
                    for_log(user, MAX_LOGGED_NAME),
                    self.peer,
                    for_log(err.message(), MAX_LOGGED_MESSAGE)
                );
                Ok(Auth::reject())
            }
        }
    }

    async fn auth_publickey(&mut self, user: &str, offer: VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let public_key = offer.public_key;
        if user == super::cluster::CLUSTER_USER {
            if super::cluster::is_cluster_key(public_key) {
                // only a member of a cluster on the ssh transport, directly (SEC-CLU-03)
                let factory = std::sync::Weak::clone(&self.shared.factory);
                let members = tokio::task::spawn_blocking(move || {
                    factory
                        .upgrade()
                        .map(|factory| crate::cluster_edge::member_addresses(&factory))
                })
                .await;
                let members = match members {
                    Ok(Some(Ok(members))) => members,
                    Ok(Some(Err(err))) => {
                        tracing::warn!("cluster login from {}: the members could not be read: {err}", self.peer);
                        return Ok(Auth::reject());
                    }
                    Ok(None) | Err(_) => return Ok(Auth::reject()),
                };
                let takes_ssh = super::cluster::takes_ssh_channel();
                if let Some(why) = super::cluster::login_refused(takes_ssh, self.via_edge, self.peer.ip(), &members) {
                    tracing::warn!("cluster login from {} refused: {why}", self.peer);
                    return Ok(Auth::reject());
                }
                if let Err(refused) = self.slot.log_in(Who::Cluster) {
                    tracing::warn!(
                        "cluster peer from {} refused: ssh-server {} sessions are open",
                        self.peer,
                        refused.limit()
                    );
                    return Ok(Auth::reject());
                }
                tracing::info!("cluster peer connected from {}", self.peer);
                self.cluster = true;
                self.username = Some(user.to_string());
                return Ok(Auth::Accept);
            }
            tracing::warn!(
                "cluster login from {} with a key that is not the cluster key",
                self.peer
            );
            return Ok(Auth::reject());
        }
        let fingerprint = public_key.fingerprint(HashAlg::Sha256).to_string();
        let username = user.to_string();
        let client_ip = self.peer.ip().to_string();
        let checked = on_pool(&self.shared.factory, move |factory| {
            if !key_belongs_to(factory, &fingerprint, &username)? {
                return Err(CoreError::new(format!(
                    "no active UserSshKey {fingerprint} for {username}"
                )));
            }
            // a person's sign-in, as a password's: it clears a sign-out elsewhere (hasLoggedOut), checks ipAllowed
            // against the peer and is recorded in the login history
            moqui_core::user::login_ssh_key_user(&username, &client_ip)?;
            ssh_permissions()
        })
        .await;
        match checked {
            Ok(permissions) => Ok(self.accept(user, permissions, LoginBy::Key)),
            Err(err) => {
                tracing::warn!(
                    "SSH key login of {} from {} failed: {}",
                    for_log(user, MAX_LOGGED_NAME),
                    self.peer,
                    for_log(err.message(), MAX_LOGGED_MESSAGE)
                );
                Ok(Auth::reject())
            }
        }
    }

    /// No questions: the client is let in with no user, and its `apps` session shows the login dialog. A client with a
    /// key has tried it by now, since "none" is not offered and OpenSSH tries keys before keyboard-interactive.
    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        _response: Option<russh::server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if !self.shared.dialog || user == super::cluster::CLUSTER_USER {
            return Ok(Auth::reject());
        }
        if let Err(refused) = self.slot.log_in(Who::Dialog) {
            tracing::warn!(
                "SSH login dialog for {} from {} refused: ssh-server {} sessions are open",
                for_log(user, MAX_LOGGED_NAME),
                self.peer,
                refused.limit()
            );
            return Ok(Auth::reject());
        }
        self.slot.dialog.store(true, Ordering::Release);
        tracing::info!(
            "SSH connection from {} let in for the login dialog as {}",
            self.peer,
            for_log(user, MAX_LOGGED_NAME)
        );
        self.login_name = Some(user.to_string());
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.let_in() {
            reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        }
        // channels not yet given a request and running `sftp` subsystems, which hold files open (SEC-FIL-61)
        if self.channels.len() + self.sftp_open.load(Ordering::Acquire) >= MAX_SESSION_CHANNELS {
            tracing::warn!(
                "SSH session channel from {} refused: {MAX_SESSION_CHANNELS} are open on the connection",
                self.peer
            );
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        }
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if term.len() > MAX_CHANNEL_TEXT {
            return session.channel_failure(channel);
        }
        let (size, _) = tokio::sync::watch::channel(window(col_width, row_height));
        self.ptys.insert(
            channel,
            Pty {
                term: term.to_string(),
                size,
            },
        );
        session.channel_success(channel)
    }

    /// A new window size, for an `apps` session on the channel (now or later).
    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(pty) = self.ptys.get(&channel) {
            pty.size.send_replace(window(col_width, row_height));
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(username) = self.username.clone().filter(|_| name == "sftp" && !self.cluster) else {
            return session.channel_failure(channel);
        };
        if !self.refresh_permissions().await {
            return session.channel_failure(channel);
        }
        let held = Held::new(&username);
        let backend = if self.can_sftp {
            Backend::Root(self.shared.root.clone())
        } else if self.shared.sftp_drives {
            Backend::Drives { username }
        } else {
            return session.channel_failure(channel);
        };
        let Some(open) = self.channels.remove(&channel) else {
            return session.channel_failure(channel);
        };
        session.channel_success(channel)?;
        let fs = SftpFs::new(
            std::sync::Weak::clone(&self.shared.factory),
            backend,
            self.shared.max_file,
            held,
        )
        .counted(OpenChannel::new(&self.sftp_open));
        russh_sftp::server::run(open.into_stream(), fs).await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.cluster {
            let command = String::from_utf8_lossy(data).into_owned();
            let topic = command
                .strip_prefix("cluster ")
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            let Some(open) = self.channels.remove(&channel).filter(|_| !topic.is_empty()) else {
                return session.channel_failure(channel);
            };
            session.channel_success(channel)?;
            let factory = std::sync::Weak::clone(&self.shared.factory);
            let posts = Arc::clone(&self.shared.cluster_posts);
            let side = super::cluster::ChannelSide {
                peer: self.peer,
                host: self.shared.host_fingerprint.clone(),
            };
            tokio::spawn(super::cluster::serve_channel(
                open.into_stream(),
                factory,
                posts,
                topic,
                side,
            ));
            return Ok(());
        }
        let line = String::from_utf8_lossy(data).into_owned();
        #[cfg(feature = "git")]
        if moqui_git::server::parse_ssh_command(&line).is_some()
            || moqui_git::server::lfs::parse_ssh_command(&line).is_some()
        {
            if !self.refresh_permissions().await {
                return session.channel_failure(channel);
            }
            let may_git = git_allowed(GitLogin {
                has_permission: self.can_git,
                by_password: self.by_password,
                any_account: self.shared.git_any_account,
                password_auth: self.shared.git_password_auth,
            });
            let (Some(username), true) = (self.username.clone(), may_git != GitAccess::Refused) else {
                return session.channel_failure(channel);
            };
            // Git hosting opens the server to every developer, and keys are not guessed.
            if may_git == GitAccess::NeedsKey {
                tracing::warn!(
                    "SSH git of {username} from {} refused: git over SSH takes a key (ssh-server git-password-auth)",
                    self.peer
                );
                return session.channel_failure(channel);
            }
            let Some(open) = self.channels.remove(&channel) else {
                return session.channel_failure(channel);
            };
            session.channel_success(channel)?;
            tokio::spawn(super::git::serve_channel(
                open,
                channel,
                session.handle(),
                std::sync::Weak::clone(&self.shared.factory),
                username,
                line,
                self.git_protocol.remove(&channel),
            ));
            return Ok(());
        }
        let trimmed = line.trim();
        if let Some(args) = trimmed
            .strip_prefix("agent-apps")
            .filter(|args| args.is_empty() || args.starts_with(char::is_whitespace))
            .filter(|_| self.username.is_some())
        {
            let Some(open) = self.channels.remove(&channel) else {
                return session.channel_failure(channel);
            };
            session.channel_success(channel)?;
            let access = self.agent_access();
            let args = args.trim().to_string();
            let handle = session.handle();
            tokio::spawn(async move {
                let mut stream = open.into_stream();
                let status = run_agent(&mut stream, access, &args).await;
                let _ = stream.flush().await;
                let _ = handle.exit_status_request(channel, status).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }
        if trimmed == "apps" && self.let_in() {
            let Some(open) = self.channels.remove(&channel) else {
                return session.channel_failure(channel);
            };
            session.channel_success(channel)?;
            let access = self.apps_access(channel);
            let newline = if self.ptys.contains_key(&channel) { "\r\n" } else { "\n" };
            let handle = session.handle();
            tokio::spawn(async move {
                let mut stream = open.into_stream();
                let status = run_apps(&mut stream, access, newline).await;
                let _ = stream.flush().await;
                let _ = handle.exit_status_request(channel, status).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }
        if self.username.is_none() && self.login_name.is_some() {
            return refuse_channel(self.channels.remove(&channel), channel, session, DIALOG_ONLY);
        }
        if !self.refresh_permissions().await {
            return session.channel_failure(channel);
        }
        let (Some(username), true) = (self.username.clone(), self.can_shell) else {
            return session.channel_failure(channel);
        };
        let Some(open) = self.channels.remove(&channel) else {
            return session.channel_failure(channel);
        };
        session.channel_success(channel)?;
        let factory = std::sync::Weak::clone(&self.shared.factory);
        let handle = session.handle();
        let newline = if self.ptys.contains_key(&channel) { "\r\n" } else { "\n" };
        let from = self.peer.ip().to_canonical().to_string();
        tokio::spawn(async move {
            let mut stream = open.into_stream();
            let outcome = shell::run(&factory, &username, &from, &line).await;
            let text = with_newlines(&outcome.text, newline);
            if !text.is_empty() {
                let _ = stream.write_all(text.as_bytes()).await;
                let _ = stream.write_all(newline.as_bytes()).await;
            }
            let _ = stream.flush().await;
            let _ = handle.exit_status_request(channel, outcome.status).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }

    /// `GIT_PROTOCOL` (protocol v2 for git) is kept for the channel's exec, and the variables that say how a terminal
    /// draws for `apps` (`COLORTERM`, `NO_COLOR`, the locale); other variables are refused, as OpenSSH's default
    /// `AcceptEnv` does.
    async fn env_request(
        &mut self,
        channel: ChannelId,
        variable_name: &str,
        variable_value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if variable_value.len() > MAX_CHANNEL_TEXT {
            return session.channel_failure(channel);
        }
        if variable_name == "GIT_PROTOCOL" && self.username.is_some() {
            self.git_protocol.insert(channel, variable_value.to_string());
            return session.channel_success(channel);
        }
        #[cfg(feature = "tui")]
        if crate::term::is_terminal_variable(variable_name) && self.let_in() {
            self.terminal_env
                .entry(channel)
                .or_default()
                .insert(variable_name.to_string(), variable_value.to_string());
            return session.channel_success(channel);
        }
        session.channel_failure(channel)
    }

    /// A closed channel's entries go from every per-channel map, so a connection that opens and closes channels does
    /// not keep each one's `TERM` and variables until it ends (SEC-FIL-88).
    async fn channel_close(&mut self, channel: ChannelId, _session: &mut Session) -> Result<(), Self::Error> {
        self.forget_channel(channel);
        Ok(())
    }

    /// The admin shell for a user with `SSH_SHELL` who logged in by key or password; for everyone else, `apps` is the
    /// whole session.
    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        if self.cluster || !self.let_in() {
            return session.channel_failure(channel);
        }
        let username = self.username.clone().unwrap_or_default();
        let Some(open) = self.channels.remove(&channel) else {
            return session.channel_failure(channel);
        };
        session.channel_success(channel)?;
        let factory = std::sync::Weak::clone(&self.shared.factory);
        let handle = session.handle();
        let pty = self.ptys.contains_key(&channel);
        // an account that may no longer sign in keeps no permission; its `apps` login meets the same gate
        let _ = self.refresh_permissions().await;
        let access = self.apps_access(channel);
        if !self.can_shell || self.username.is_none() {
            let newline = if pty { "\r\n" } else { "\n" };
            tokio::spawn(async move {
                let mut stream = open.into_stream();
                let status = run_apps(&mut stream, access, newline).await;
                let _ = stream.flush().await;
                let _ = handle.exit_status_request(channel, status).await;
                let _ = handle.eof(channel).await;
                let _ = handle.close(channel).await;
            });
            return Ok(());
        }
        let host = self.host_access(channel, &username);
        let from = self.peer.ip().to_canonical().to_string();
        tokio::spawn(async move {
            let status = interactive(open.into_stream(), &factory, &username, &from, pty, access, host).await;
            let _ = handle.exit_status_request(channel, status).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
        Ok(())
    }
}

/// Whether an active `UserSshKey` with this fingerprint belongs to the account named `username`.
fn key_belongs_to(factory: &ExecutionContextFactory, fingerprint: &str, username: &str) -> Result<bool, CoreError> {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    let facade = factory
        .entity()
        .ok_or_else(|| CoreError::new("the entity facade is not installed"))?;
    let Some(account) = facade
        .find("moqui.security.UserAccount")?
        .eq("username", Value::string(username))
        .one()?
    else {
        return Ok(false);
    };
    let user_id = account
        .get("userId")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX);
    let keys = facade
        .find("moqui.security.UserSshKey")?
        .eq("userId", Value::string(&user_id))
        .list()?;
    Ok(keys.values().iter().any(|key| {
        let stored = key
            .get("fingerprint")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        let from = key.get("fromDate").and_then(millis);
        let thru = key.get("thruDate").and_then(millis);
        super::client::same_fingerprint(&stored, fingerprint)
            && from.is_none_or(|from| from <= now)
            && thru.is_none_or(|thru| thru > now)
    }))
}

fn millis(value: &Value) -> Option<i64> {
    match moqui_lib::object::basic_convert(value, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
