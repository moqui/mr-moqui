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

//! The built-in SSH server (`plans/archive/Ssh.md`): off unless `ssh_port` is set. It listens on `ssh_host` (default
//! `127.0.0.1`) with the host key `ssh_host_key`, a secret property that is the base64 seed of an Ed25519 key
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
//! for `shell` and `exec`. The `sftp` subsystem needs `SSH_SFTP` and serves `ssh-server` `sftp-root`. Git needs `SSH_GIT`, and
//! a key unless `ssh-server` `git-password-auth` is true. `agent-apps` (`super::agent`) is the terminal client for a program,
//! on an `exec` channel with no pty, one command line at a time, for a user logged in by key or password.
//!
//! At most `ssh-server` `max-sessions` logged-in sessions are open at once; one with no user counts too. A connection that has
//! not logged in has [`PREAUTH_TIMEOUT`] to do so, and at most [`MAX_PREAUTH`] such connections wait at once. One let in
//! with no user is closed when its dialog has not logged in within `crate::term::LOGIN_WAIT`.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{Error as CoreError, ExecutionContextFactory};
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use russh::keys::{HashAlg, PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelId, ChannelOpenFailure};
use russh::{MethodKind, MethodSet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{
    on_pool,
    sftp_fs::{Backend, SftpFs},
    shell,
};

const DEFAULT_MAX_SESSIONS: usize = 8;
const DEFAULT_MAX_FILE: usize = 256 * 1024 * 1024;
/// The connection's inactivity timeout; `apps` ends after as long without input (`crate::term::IDLE`).
pub(super) const IDLE: Duration = Duration::from_secs(600);
/// How long a connection has from accept to a successful login.
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(30);
/// Connections that have not logged in yet, at once; one more is closed. They do not count toward `ssh-server` `max-sessions`.
const MAX_PREAUTH: usize = 16;
/// The longest admin shell line, and the longest `js` block.
const MAX_LINE: usize = 1024 * 1024;
const PROMPT: &str = "moqui> ";

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
    /// Keyboard-interactive lets a client in with no user, for the login dialog (`ssh-server` `login-dialog`).
    #[cfg_attr(not(feature = "tui"), allow(dead_code))]
    dialog: bool,
    /// Git over SSH on a connection that logged in by password (`ssh-server` `git-password-auth`).
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    git_password_auth: bool,
}

/// The host key from `ssh_host_key`: an OpenSSH private key, or the base64 of a 32-byte Ed25519 seed.
///
/// # Errors
///
/// Returns an error when the property is empty, still the placeholder, or does not decode.
pub fn host_key(text: &str) -> Result<PrivateKey, Error> {
    let text = text.trim();
    if text.is_empty() || text.ends_with("CHANGEME") {
        return Err(Error::new("ssh_host_key is not set; the SSH server needs a host key"));
    }
    if text.starts_with("-----BEGIN") {
        return russh::keys::decode_secret_key(text, None)
            .map_err(|err| Error::new(format!("ssh_host_key does not decode: {err}")));
    }
    use base64::Engine;
    let seed = base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|err| Error::new(format!("ssh_host_key is not base64: {err}")))?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| Error::new("ssh_host_key must be the base64 of 32 bytes, or an OpenSSH private key"))?;
    let keypair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    Ok(PrivateKey::from(keypair))
}

/// The host and port the SSH server binds by the properties `ssh_host` (default `127.0.0.1`) and `ssh_port`, for the
/// port check before the conf is read. None while `ssh_port` is empty, which leaves the server off.
pub(crate) fn listen_target() -> Result<Option<(String, u16)>, Error> {
    listen_target_of(
        &moqui_lib::env::get_prop_or_env("ssh_port"),
        &moqui_lib::env::get_prop_or_env("ssh_host"),
    )
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

/// An attribute of the conf's `ssh-server`, or the property `property` when the conf leaves it out or it expands to
/// nothing.
fn setting(factory: &ExecutionContextFactory, name: &str, property: &str) -> String {
    let configured = factory.conf_setting("ssh-server", name);
    if configured.is_empty() {
        moqui_lib::env::get_prop_or_env(property).trim().to_string()
    } else {
        configured
    }
}

/// Start the server when the conf's `ssh-server` `port` (`ssh_port`) is set (`0` binds an ephemeral port).
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
    let Some((host, port)) = listen_target_of(
        &setting(factory, "port", "ssh_port"),
        &setting(factory, "host", "ssh_host"),
    )?
    else {
        return Ok(None);
    };
    let key = host_key(&setting(factory, "host-key", "ssh_host_key"))?;
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
    let max_file = positive(
        factory.conf_number("ssh-server", "sftp-max-file", DEFAULT_MAX_FILE),
        DEFAULT_MAX_FILE,
    );
    let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
    // "none" is not offered, so a client tries its keys first; keyboard-interactive, which OpenSSH tries next, is the
    // login dialog, and only with the terminal client.
    let dialog = cfg!(feature = "tui") && factory.conf_flag("ssh-server", "login-dialog", true);
    let git_password_auth = factory.conf_flag("ssh-server", "git-password-auth", false);
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
        dialog,
        git_password_auth,
    });
    crate::edge::handle().spawn(accept_loop(listener, config, shared, watch, max_sessions));
    *factory
        .extension(|_| SshAddress::default())
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(address);
    tracing::info!("SSH server listening on {address}");
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
    preauth: AtomicUsize,
    sessions: AtomicUsize,
    max_sessions: usize,
}

/// One connection's place in [`Counts`]: a pre-auth slot from accept, traded for a session at login. Drop gives back
/// whichever it holds.
struct Slot {
    counts: Arc<Counts>,
    preauth: AtomicBool,
    session: AtomicBool,
    /// Let in with no user: the login dialog must log in within `crate::term::LOGIN_WAIT`.
    dialog: AtomicBool,
    /// The dialog logged in, set by the session's login gate.
    dialog_done: Arc<AtomicBool>,
}

impl Slot {
    /// A pre-auth slot, or `None` when [`MAX_PREAUTH`] connections are already waiting to log in.
    fn preauth(counts: &Arc<Counts>) -> Option<Self> {
        if counts.preauth.fetch_add(1, Ordering::AcqRel) >= MAX_PREAUTH {
            counts.preauth.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(Self {
            counts: Arc::clone(counts),
            preauth: AtomicBool::new(true),
            session: AtomicBool::new(false),
            dialog: AtomicBool::new(false),
            dialog_done: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Trade the pre-auth slot for a session. False when `ssh-server` `max-sessions` are open.
    fn log_in(&self) -> bool {
        if self.session.load(Ordering::Acquire) {
            return true;
        }
        if self.counts.sessions.fetch_add(1, Ordering::AcqRel) >= self.counts.max_sessions {
            self.counts.sessions.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        self.session.store(true, Ordering::Release);
        if self.preauth.swap(false, Ordering::AcqRel) {
            self.counts.preauth.fetch_sub(1, Ordering::AcqRel);
        }
        true
    }

    fn logged_in(&self) -> bool {
        self.session.load(Ordering::Acquire)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.preauth.swap(false, Ordering::AcqRel) {
            self.counts.preauth.fetch_sub(1, Ordering::AcqRel);
        }
        if self.session.swap(false, Ordering::AcqRel) {
            self.counts.sessions.fetch_sub(1, Ordering::AcqRel);
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

/// Resolves the login wait after `since` when the connection was let in with no user and its dialog has not logged in
/// by then, so a client that opens no session, or leaves the dialog, does not hold a session slot; never otherwise.
async fn dialog_expired(slot: &Slot, since: tokio::time::Instant) {
    #[cfg(feature = "tui")]
    {
        tokio::time::sleep_until(since + crate::term::LOGIN_WAIT).await;
        if slot.dialog.load(Ordering::Acquire) && !slot.dialog_done.load(Ordering::Acquire) {
            return;
        }
    }
    #[cfg(not(feature = "tui"))]
    let _ = (slot, since);
    std::future::pending::<()>().await;
}

async fn accept_loop(
    listener: TcpListener,
    config: Arc<russh::server::Config>,
    shared: Arc<Shared>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    max_sessions: usize,
) {
    let counts = Arc::new(Counts {
        preauth: AtomicUsize::new(0),
        sessions: AtomicUsize::new(0),
        max_sessions,
    });
    loop {
        let accepted = tokio::select! {
            _ = shutdown.changed() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, peer)) = accepted else {
            continue;
        };
        let Some(slot) = Slot::preauth(&counts) else {
            tracing::warn!("SSH connection from {peer} refused: {MAX_PREAUTH} connections are waiting to log in");
            drop(stream);
            continue;
        };
        let slot = Arc::new(slot);
        let since = tokio::time::Instant::now();
        let handler = Connection {
            shared: Arc::clone(&shared),
            slot: Arc::clone(&slot),
            peer,
            username: None,
            login_name: None,
            dialog_used: false,
            by_password: false,
            cluster: false,
            can_sftp: false,
            can_shell: false,
            can_git: false,
            channels: HashMap::new(),
            ptys: HashMap::new(),
            terminal_env: HashMap::new(),
            git_protocol: HashMap::new(),
        };
        let config = Arc::clone(&config);
        let mut stop = shutdown.clone();
        tokio::spawn(async move {
            let _ = stream.set_nodelay(true);
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
    /// The user logged in by key or password; `None` before, and for a connection let in for the login dialog.
    username: Option<String>,
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
    /// A cluster peer logged in with the cluster key: it may open `cluster <topic>` channels and nothing else.
    cluster: bool,
    can_sftp: bool,
    can_shell: bool,
    /// `git-upload-pack` and `git-receive-pack` (`SSH_GIT`); each repository is still checked. Unread without the `git`
    /// feature, where git commands are not served.
    #[cfg_attr(not(feature = "git"), allow(dead_code))]
    can_git: bool,
    channels: HashMap<ChannelId, Channel<Msg>>,
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

/// What `apps` gets on a channel: the client, or why not.
#[cfg(feature = "tui")]
type AppsAccess = Result<crate::term::Start, &'static str>;
#[cfg(not(feature = "tui"))]
type AppsAccess = Result<(), &'static str>;

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

fn ssh_permissions() -> Result<Permissions, CoreError> {
    Ok(Permissions {
        sftp: moqui_core::user::has_permission("SSH_SFTP")?,
        shell: moqui_core::user::has_permission("SSH_SHELL")?,
        git: moqui_core::user::has_permission("SSH_GIT")?,
    })
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
    /// Let the user in, with what the permissions allow; every user gets `apps`.
    fn accept(&mut self, username: &str, permissions: Permissions, by_password: bool) -> Auth {
        let Permissions { sftp, shell, git } = permissions;
        if !self.slot.log_in() {
            tracing::warn!(
                "SSH login of {username} from {} refused: ssh-server max-sessions {} are open",
                self.peer,
                self.slot.counts.max_sessions
            );
            return Auth::reject();
        }
        tracing::info!("SSH login of {username} from {}", self.peer);
        self.username = Some(username.to_string());
        self.by_password = by_password;
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

    /// `apps` on `channel`: a pty, and a build with the client. It runs as the user logged in, or with the login
    /// dialog first, once per connection.
    fn apps_access(&mut self, channel: ChannelId) -> AppsAccess {
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
                    done: Some(Arc::clone(&self.slot.dialog_done)),
                },
                (None, None) => return Err(DIALOG_ONLY),
            };
            self.dialog_used |= self.username.is_none();
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

/// Run `apps` on the stream, or write why it cannot run. Returns the exit status.
async fn run_apps<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    access: AppsAccess,
    newline: &str,
) -> u32 {
    match access {
        #[cfg(feature = "tui")]
        Ok(mut start) => {
            let ended = crate::term::serve(stream, &mut start).await;
            if let Some(note) = ended.note {
                let _ = stream.write_all(format!("apps: {note}{newline}").as_bytes()).await;
            }
            ended.status
        }
        #[cfg(not(feature = "tui"))]
        Ok(()) => 1,
        Err(reason) => {
            let _ = stream.write_all(format!("{reason}{newline}").as_bytes()).await;
            1
        }
    }
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
                    moqui_core::user::login_app_password(&username, &secret)
                })
                .is_ok();
            if !by_app_password {
                moqui_core::user::with_client_ip(&client_ip, || moqui_core::user::login_user(&username, &secret))?;
            }
            if moqui_core::user::current_user_id()?.is_none_or(|id| id.is_empty()) {
                return Err(CoreError::new("a second factor is required, which SSH does not take"));
            }
            ssh_permissions()
        })
        .await;
        match checked {
            Ok(permissions) => Ok(self.accept(user, permissions, true)),
            Err(err) => {
                tracing::warn!(
                    "SSH password login of {user} from {} failed: {}",
                    self.peer,
                    err.message()
                );
                Ok(Auth::reject())
            }
        }
    }

    async fn auth_publickey(&mut self, user: &str, public_key: &PublicKey) -> Result<Auth, Self::Error> {
        if user == super::cluster::CLUSTER_USER {
            if super::cluster::is_cluster_key(public_key) {
                if !self.slot.log_in() {
                    tracing::warn!(
                        "cluster peer from {} refused: ssh-server max-sessions {} are open",
                        self.peer,
                        self.slot.counts.max_sessions
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
        let checked = on_pool(&self.shared.factory, move |factory| {
            if !key_belongs_to(factory, &fingerprint, &username)? {
                return Err(CoreError::new(format!(
                    "no active UserSshKey {fingerprint} for {username}"
                )));
            }
            moqui_core::user::internal_login_user(&username)?;
            ssh_permissions()
        })
        .await;
        match checked {
            Ok(permissions) => Ok(self.accept(user, permissions, false)),
            Err(err) => {
                tracing::warn!("SSH key login of {user} from {} failed: {}", self.peer, err.message());
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
        if !self.slot.log_in() {
            tracing::warn!(
                "SSH login dialog for {user} from {} refused: ssh-server max-sessions {} are open",
                self.peer,
                self.slot.counts.max_sessions
            );
            return Ok(Auth::reject());
        }
        self.slot.dialog.store(true, Ordering::Release);
        tracing::info!(
            "SSH connection from {} let in for the login dialog as {user:?}",
            self.peer
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
        );
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
            tokio::spawn(super::cluster::serve_channel(open.into_stream(), factory, posts, topic));
            return Ok(());
        }
        let line = String::from_utf8_lossy(data).into_owned();
        #[cfg(feature = "git")]
        if moqui_core::resource::git::server::parse_ssh_command(&line).is_some()
            || moqui_core::resource::git::server::lfs::parse_ssh_command(&line).is_some()
        {
            let (Some(username), true) = (self.username.clone(), self.can_git) else {
                return session.channel_failure(channel);
            };
            // Git hosting opens the server to every developer, and keys are not guessed.
            if self.by_password && !self.shared.git_password_auth {
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
        tokio::spawn(async move {
            let mut stream = open.into_stream();
            let outcome = shell::run(&factory, &username, &line).await;
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
        if variable_name == "GIT_PROTOCOL" && self.username.is_some() {
            self.git_protocol.insert(channel, variable_value.to_string());
            return session.channel_success(channel);
        }
        #[cfg(feature = "tui")]
        if crate::term::is_terminal_variable(variable_name) && self.let_in() && variable_value.len() < 256 {
            self.terminal_env
                .entry(channel)
                .or_default()
                .insert(variable_name.to_string(), variable_value.to_string());
            return session.channel_success(channel);
        }
        session.channel_failure(channel)
    }

    /// The admin shell for a user with `SSH_SHELL` who logged in by key or password; for everyone else, `apps` is the
    /// whole session.
    async fn shell_request(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        if !self.let_in() {
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
        tokio::spawn(async move {
            let status = interactive(open.into_stream(), &factory, &username, pty, access).await;
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

fn with_newlines(text: &str, newline: &str) -> String {
    if newline == "\n" {
        text.to_string()
    } else {
        text.replace('\n', newline)
    }
}

/// The interactive loop: a prompt, a line, its output. With a PTY the client does not echo, so this echoes and
/// handles backspace, ctrl-C (drop the line) and ctrl-D (end on an empty line). `apps` runs the terminal client on
/// the stream, and the prompt comes back when it ends. Returns the exit status.
async fn interactive<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut stream: S,
    factory: &std::sync::Weak<ExecutionContextFactory>,
    username: &str,
    pty: bool,
    apps: AppsAccess,
) -> u32 {
    let newline = if pty { "\r\n" } else { "\n" };
    let greeting = format!("MR-Moqui admin shell for {username}; type help{newline}{PROMPT}");
    if stream.write_all(greeting.as_bytes()).await.is_err() {
        return 1;
    }
    let mut line: Vec<u8> = Vec::new();
    let mut block: Option<Vec<String>> = None;
    // Bytes past MAX_LINE are not kept; the line is refused when it ends.
    let mut overflow = false;
    let mut block_len = 0_usize;
    let mut buffer = [0u8; 1024];
    loop {
        let read = match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => return 0,
            Ok(read) => read,
        };
        for &byte in &buffer[..read] {
            let mut echo: Vec<u8> = Vec::new();
            let mut finished = None;
            match byte {
                b'\r' | b'\n' => {
                    if byte == b'\n' && pty {
                        continue;
                    }
                    if pty {
                        echo.extend_from_slice(b"\r\n");
                    }
                    finished = Some(String::from_utf8_lossy(&line).into_owned());
                    line.clear();
                }
                0x7f | 0x08 => {
                    if line.pop().is_some() && pty {
                        echo.extend_from_slice(b"\x08 \x08");
                    }
                }
                0x03 => {
                    line.clear();
                    overflow = false;
                    block = None;
                    if pty {
                        echo.extend_from_slice(format!("^C{newline}{PROMPT}").as_bytes());
                    }
                }
                0x04 if line.is_empty() => return 0,
                byte if byte >= 0x20 || byte == b'\t' => {
                    if line.len() < MAX_LINE {
                        line.push(byte);
                        if pty {
                            echo.push(byte);
                        }
                    } else {
                        overflow = true;
                    }
                }
                _ => {}
            }
            if !echo.is_empty() && stream.write_all(&echo).await.is_err() {
                return 1;
            }
            let Some(text) = finished else {
                continue;
            };
            if std::mem::take(&mut overflow) {
                block = None;
                let message = format!("the line is longer than {MAX_LINE} bytes and was dropped");
                if refuse(&mut stream, &message, newline).await.is_err() {
                    return 1;
                }
                continue;
            }
            // A `js` block gathers lines until a line with only `.`.
            if let Some(lines) = block.as_mut() {
                block_len = block_len.saturating_add(text.len() + 1);
                if block_len > MAX_LINE {
                    block = None;
                    let message = format!("the js block is longer than {MAX_LINE} bytes and was dropped");
                    if refuse(&mut stream, &message, newline).await.is_err() {
                        return 1;
                    }
                    continue;
                }
                if text.trim() == "." {
                    let code = lines.join("\n");
                    block = None;
                    let outcome = shell::run(factory, username, &format!("js {code}")).await;
                    if write_outcome(&mut stream, &outcome.text, newline).await.is_err() {
                        return 1;
                    }
                } else {
                    lines.push(text);
                    if stream.write_all(b"... ").await.is_err() {
                        return 1;
                    }
                    continue;
                }
            } else if shell::starts_block(&text) {
                block = Some(Vec::new());
                block_len = 0;
                if stream
                    .write_all(format!("end with a line of .{newline}... ").as_bytes())
                    .await
                    .is_err()
                {
                    return 1;
                }
                continue;
            } else if text.trim() == "apps" {
                // `apps` takes the stream until it ends; a second `apps` gets a fresh window watch.
                #[cfg(feature = "tui")]
                let access = match &apps {
                    Ok(start) => Ok(start.again()),
                    Err(reason) => Err(*reason),
                };
                #[cfg(not(feature = "tui"))]
                let access = apps;
                run_apps(&mut stream, access, newline).await;
            } else {
                let outcome = shell::run(factory, username, &text).await;
                if outcome.exit {
                    let _ = stream.write_all(format!("bye{newline}").as_bytes()).await;
                    return 0;
                }
                if write_outcome(&mut stream, &outcome.text, newline).await.is_err() {
                    return 1;
                }
            }
            if stream.write_all(PROMPT.as_bytes()).await.is_err() {
                return 1;
            }
        }
    }
}

/// An input the shell refuses: the message, then a fresh prompt.
async fn refuse<W: AsyncWriteExt + Unpin>(stream: &mut W, message: &str, newline: &str) -> std::io::Result<()> {
    stream
        .write_all(format!("error: {message}{newline}{PROMPT}").as_bytes())
        .await
}

async fn write_outcome<W: AsyncWriteExt + Unpin>(stream: &mut W, text: &str, newline: &str) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    stream.write_all(with_newlines(text, newline).as_bytes()).await?;
    stream.write_all(newline.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::{interactive, Counts, Slot, MAX_LINE, MAX_PREAUTH};

    #[tokio::test]
    async fn an_overlong_shell_line_or_js_block_is_refused() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let shell =
            tokio::spawn(
                async move { interactive(server, &std::sync::Weak::new(), "tester", false, Err("no apps")).await },
            );
        let (mut reader, mut writer) = tokio::io::split(client);
        let send = tokio::spawn(async move {
            writer.write_all(&vec![b'a'; MAX_LINE + 10]).await.unwrap();
            writer.write_all(b"\njs\n").await.unwrap();
            let chunk = vec![b'b'; MAX_LINE / 2];
            for _ in 0..3 {
                writer.write_all(&chunk).await.unwrap();
                writer.write_all(b"\n").await.unwrap();
            }
            writer.write_all(b"exit\n").await.unwrap();
            writer
        });
        let mut seen = Vec::new();
        reader.read_to_end(&mut seen).await.unwrap();
        let seen = String::from_utf8_lossy(&seen);
        assert!(seen.contains("error: the line is longer than"), "{seen}");
        assert!(seen.contains("error: the js block is longer than"), "{seen}");
        assert!(seen.ends_with("bye\n"), "{seen}");
        drop(send.await.unwrap());
        assert_eq!(shell.await.unwrap(), 0);
    }

    #[test]
    fn a_login_trades_the_preauth_slot_for_a_session() {
        let counts = Arc::new(Counts {
            preauth: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
            max_sessions: 1,
        });
        let waiting: Vec<Slot> = (0..MAX_PREAUTH).map(|_| Slot::preauth(&counts).unwrap()).collect();
        assert!(Slot::preauth(&counts).is_none(), "past MAX_PREAUTH");
        assert!(waiting[0].log_in());
        // The login gave its pre-auth slot back, and took the one session.
        let next = Slot::preauth(&counts).unwrap();
        assert!(!next.log_in(), "past ssh-server max-sessions");
        drop(waiting);
        assert!(next.log_in());
        drop(next);
        assert_eq!(counts.preauth.load(Ordering::Acquire), 0);
        assert_eq!(counts.sessions.load(Ordering::Acquire), 0);
    }
}
