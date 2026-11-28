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

//! The SSH server's life while the server runs (plans/Runtime-Config.md §3.4, §8.4): what serves its connections for
//! as long as it is the factory's ([`Intake`]: the shared state, the counts and the settings applied in place), each
//! bind's accept loop and stop ([`serve`], [`SshListener`]), the russh settings of each access mode made at each bind
//! with the host key read again ([`Configs`]), and the edge node's tunnel streams, taken while a bind runs and ended
//! when SSH is off ([`now_running`], [`now_off`], [`serve_stream`]). The listener registry's SSH entry drives it
//! (`crate::listeners`); one connection's serving, its logins and its channels are [`super::server`]'s.
//!
//! A bind made again keeps the intake, so its counts and settings go on; its old socket's sessions end after its
//! stop's grace, or at once for the process stop or "end connections now". Sessions still at the sign-in dialog end
//! when the dialog is no longer offered (`access` leaves `open`, or the terminal client is turned off).

// Without a server boot there is no listener registry to start the SSH server (an `ssh` build with no web server or
// database): its life is unused there, and with it what it serves.
#![cfg_attr(
    not(all(feature = "axum", any(feature = "postgres", feature = "mysql"))),
    allow(dead_code)
)]

use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use arc_swap::{ArcSwap, ArcSwapOption};
use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use russh::keys::{HashAlg, PrivateKey};
use russh::MethodSet;
use tokio::net::TcpListener;
use tokio::sync::watch;

use super::live::{Access, Method, SshLive};
use super::server::{host_key, serve_connection, Counts, IDLE};

/// The address the server listens on, kept on the factory for `Server::ssh_address`.
#[derive(Default)]
pub(crate) struct SshAddress(pub(crate) std::sync::Mutex<Option<SocketAddr>>);

/// How a bind's sessions end when its stop begins (plans/Runtime-Config.md §3.1): at once (the process stop, as before
/// the registry, and "end connections now"), or after a grace (a live stop or a bind made again).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ending {
    Now,
    After(Duration),
}

/// One bind of the server: its accept loop, and the sessions it took, which end with its stop. Dropped, it stops at
/// once.
pub(crate) struct SshListener {
    address: SocketAddr,
    intake: Arc<Intake>,
    stop: watch::Sender<Option<Ending>>,
    bind: Arc<BindState>,
    task: tokio::task::AbortHandle,
}

impl SshListener {
    #[must_use]
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    /// Take new settings in place: new connections, logins and channels read them.
    pub(crate) fn apply(&self, live: &SshLive) {
        self.intake.apply(live);
    }

    /// Stop taking connections; the sessions taken end as `how` says. A stop already begun keeps its grace, unless
    /// `how` ends them now. The edge tunnels' sessions end the same way if SSH is then off ([`now_off`]).
    pub(crate) fn begin_stop(&self, how: Ending) {
        upgrade(&self.stop, how);
        lock(&self.intake.tunnels).last_stop = Some(how);
    }

    /// What ends this bind's sessions now, also once its stop has begun with a grace, and the tunnels' that a stop
    /// with a grace left closing.
    #[must_use]
    pub(crate) fn end_now_handle(&self) -> Arc<dyn Fn() + Send + Sync> {
        let (stop, intake) = (self.stop.clone(), Arc::clone(&self.intake));
        Arc::new(move || {
            upgrade(&stop, Ending::Now);
            intake.end_tunnels_now();
        })
    }

    /// Wait up to `wait`, its stop begun, until its port is let go, which comes before its sessions' grace.
    #[must_use]
    pub(crate) fn released(&self, wait: Duration) -> bool {
        self.bind.wait(wait, |counts| counts.freed)
    }

    /// Wait up to `wait`, its stop begun, until its loop and every session it took have ended.
    #[must_use]
    pub(crate) fn finished(&self, wait: Duration) -> bool {
        self.bind.wait(wait, |counts| counts.held == 0)
    }

    /// The sessions it holds now, connections still logging in included.
    #[must_use]
    pub(crate) fn sessions(&self) -> usize {
        lock(&self.bind.counts).sessions
    }
}

/// A bind dropped without a stop that finished (a boot that failed part way, a stop past its wait) ends its sessions
/// at once, and its loop with them.
impl Drop for SshListener {
    fn drop(&mut self) {
        upgrade(&self.stop, Ending::Now);
        self.task.abort();
    }
}

/// Begin a stop `how`: the first stop gives the grace, and a later one only ends the sessions now.
pub(super) fn upgrade(stop: &watch::Sender<Option<Ending>>, how: Ending) {
    stop.send_if_modified(|current| match (*current, how) {
        (None, _) | (Some(Ending::After(_)), Ending::Now) => {
            *current = Some(how);
            true
        }
        _ => false,
    });
}

/// Resolves when a session of a bind (or of the edge tunnels) whose stop `stop` carries is to end: at once when the
/// stop ends them now or its sender is gone, else after its grace, cut short by a later "end now".
pub(super) async fn ended(mut stop: watch::Receiver<Option<Ending>>) {
    let first = match stop.wait_for(Option::is_some).await {
        Ok(how) => *how,
        Err(_) => return,
    };
    if let Some(Ending::After(grace)) = first {
        tokio::select! {
            () = tokio::time::sleep(grace) => {}
            _ = stop.wait_for(|how| *how == Some(Ending::Now)) => {}
        }
    }
}

/// A bind's loop and sessions as its stop waits for them, on a plain thread.
#[derive(Default)]
pub(super) struct BindState {
    counts: Mutex<BindCounts>,
    changed: Condvar,
}

#[derive(Default)]
struct BindCounts {
    /// Its port is let go: its loop has ended.
    freed: bool,
    /// Its loop and its sessions, each until it ends.
    held: usize,
    /// Its sessions.
    sessions: usize,
}

impl BindState {
    /// Count the loop (`session` false) or a session until the returned guard drops; the loop's end lets the port go.
    pub(super) fn hold(self: &Arc<Self>, session: bool) -> BindHold {
        let mut counts = lock(&self.counts);
        counts.held += 1;
        if session {
            counts.sessions += 1;
        }
        BindHold {
            state: Arc::clone(self),
            session,
        }
    }

    /// Wait up to `wait` until `done` holds of the counts; false when it did not by then.
    fn wait(&self, wait: Duration, done: impl Fn(&BindCounts) -> bool) -> bool {
        let deadline = std::time::Instant::now().checked_add(wait);
        let mut counts = lock(&self.counts);
        while !done(&counts) {
            let left = deadline.map_or(wait, |deadline| {
                deadline.saturating_duration_since(std::time::Instant::now())
            });
            if left.is_zero() {
                return false;
            }
            counts = moqui_lib::sync::wait_timeout(&self.changed, counts, left).0;
        }
        true
    }
}

/// What [`BindState::hold`] counts, given back as it drops.
pub(super) struct BindHold {
    state: Arc<BindState>,
    session: bool,
}

impl Drop for BindHold {
    fn drop(&mut self) {
        let mut counts = lock(&self.state.counts);
        counts.held = counts.held.saturating_sub(1);
        if self.session {
            counts.sessions = counts.sessions.saturating_sub(1);
        } else {
            counts.freed = true;
        }
        drop(counts);
        self.state.changed.notify_all();
    }
}

/// What every connection of the server shares, made at its first start.
pub(super) struct Shared {
    pub(super) factory: std::sync::Weak<ExecutionContextFactory>,
    /// `ssh-server` `sftp-root`, read at the first start.
    pub(super) root: String,
    /// The live terminal sessions of the process, against `terminal` `max-sessions`.
    #[cfg(feature = "tui")]
    pub(super) apps: Arc<crate::term::Sessions>,
    /// The sessions `agent-apps open` started, kept between its commands.
    #[cfg(feature = "tui")]
    pub(super) agents: Arc<super::agent::AgentSessions>,
    /// The cluster topics received, shared with the HTTPS cluster listener.
    pub(super) cluster_posts: Arc<std::sync::Mutex<Vec<String>>>,
}

/// The russh settings one bind serves with, one for each access mode, all with the host key read at that bind
/// (plans/Runtime-Config.md §8.4): a connection takes the one for the mode it sees at accept.
pub(super) struct Configs {
    open: Arc<russh::server::Config>,
    login: Arc<russh::server::Config>,
    key: Arc<russh::server::Config>,
    /// The host key's SHA-256 fingerprint, which a cluster channel's proof is bound to (SEC-CLU-04).
    pub(super) fingerprint: String,
}

impl Configs {
    pub(super) fn new(key: &PrivateKey) -> Self {
        let config = |access: Access| {
            Arc::new(russh::server::Config {
                keys: vec![key.clone()],
                methods: MethodSet::from(super::live::offered(access, cfg!(feature = "tui"))),
                inactivity_timeout: Some(IDLE),
                auth_rejection_time: Duration::from_millis(500),
                auth_rejection_time_initial: Some(Duration::from_millis(0)),
                max_auth_attempts: 6,
                nodelay: true,
                ..russh::server::Config::default()
            })
        };
        Self {
            open: config(Access::Open),
            login: config(Access::Login),
            key: config(Access::Key),
            fingerprint: key.public_key().fingerprint(HashAlg::Sha256).to_string(),
        }
    }

    /// The settings a connection is served with under `access` ([`SshLive::offered_as`]).
    pub(super) fn of(&self, access: Access) -> Arc<russh::server::Config> {
        Arc::clone(match access {
            Access::Open => &self.open,
            Access::Login => &self.login,
            Access::Key => &self.key,
        })
    }
}

/// Serve `listener`, bound to the conf's `ssh-server` host and port, as a bind of this factory's SSH server with the
/// settings `live`, the host key read again from `ssh-server@host-key`: the listener registry's start
/// (plans/Runtime-Config.md §8.4). The server's [`Intake`] is made at the first start (with `runtime_dir`'s SFTP root
/// and `cluster_posts`) and kept for the next. Sets the address `Server::ssh_address` answers and the properties this
/// member's cluster row carries; the caller rewrites the row.
///
/// # Errors
///
/// Returns an error when the host key is not set or does not decode, or the listener has no address.
pub(crate) fn serve(
    factory: &Arc<ExecutionContextFactory>,
    runtime_dir: &std::path::Path,
    cluster_posts: Arc<std::sync::Mutex<Vec<String>>>,
    listener: TcpListener,
    live: &SshLive,
) -> Result<SshListener, Error> {
    // by its handle: the conf's, else seal's store, where it is generated (plans/Sealed-Secrets.md S4)
    let key = host_key(
        &factory
            .secret("ssh-server@host-key")
            .map_err(|err| Error::new(err.to_string()))?
            .unwrap_or_default(),
    )?;
    let address = listener
        .local_addr()
        .map_err(|err| Error::new("the SSH listener has no address").with_source(err))?;
    let configs = Arc::new(Configs::new(&key));
    let fingerprint = configs.fingerprint.clone();
    let intake = intake(factory, runtime_dir, cluster_posts, live);
    intake.apply(live);
    // the newest bind's, which the edge tunnels' streams are served with
    intake.configs.store(Some(Arc::clone(&configs)));
    let (stop, stopped) = watch::channel(None);
    let bind = Arc::new(BindState::default());
    let task = crate::edge::handle()
        .spawn(accept_loop(
            listener,
            Arc::clone(&intake),
            configs,
            stopped,
            Arc::clone(&bind),
        ))
        .abort_handle();
    *lock(&factory.extension(|_| SshAddress::default()).0) = Some(address);
    // For the cluster member row: peers pin this key, and the ssh:// base URL defaults to this address.
    moqui_lib::env::set_property(moqui_core::cluster::SSH_FINGERPRINT_PROPERTY, fingerprint);
    moqui_lib::env::set_property(moqui_core::cluster::SSH_ADDRESS_PROPERTY, address.to_string());
    Ok(SshListener {
        address,
        intake,
        stop,
        bind,
        task,
    })
}

/// Whether the listener registry last left SSH running, off, or neither yet, on the factory: what [`now_off`] acts on
/// only when it changes.
#[derive(Default)]
struct Published(Mutex<Option<bool>>);

/// SSH runs (the listener registry's last action left a bind running): the edge node's tunnel streams are taken
/// ([`serve_stream`]).
pub(crate) fn now_running(factory: &ExecutionContextFactory) {
    *lock(&factory.extension(|_| Published::default()).0) = Some(true);
    if let Some(intake) = intake_if_made(factory) {
        lock(&intake.tunnels).open.get_or_insert_with(|| watch::channel(None).0);
    }
}

/// SSH is off, or its start failed (plans/Runtime-Config.md §3.4, off means closed): the edge tunnels' sessions end as
/// the last bind's stop ended its own and no tunnel stream is taken; on the change to off (the first pass that finds it
/// off included), the address and the process-wide properties of the cluster row go too, which a pass that finds it
/// still off leaves alone. The row keeps what it had, as nothing can stand for an SSH server that is not there.
pub(crate) fn now_off(factory: &ExecutionContextFactory) {
    if let Some(intake) = intake_if_made(factory) {
        intake.close_tunnels();
    }
    let was = lock(&factory.extension(|_| Published::default()).0).replace(false);
    if was == Some(false) {
        return;
    }
    if let Some(address) = factory.extension_if_present::<SshAddress>() {
        *lock(&address.0) = None;
    }
    moqui_lib::env::set_property(moqui_core::cluster::SSH_FINGERPRINT_PROPERTY, "");
    moqui_lib::env::set_property(moqui_core::cluster::SSH_ADDRESS_PROPERTY, "");
}

/// The rewrites of this member's cluster row, one at a time ([`rewrite_member_row`]).
#[derive(Default)]
struct MemberRewrites(Mutex<()>);

/// Rewrite this member's cluster row, which names the SSH server's address and host key, after a start or a new bind
/// (`moqui_core::cluster::refresh_member`, plans/Runtime-Config.md §3.4): on a thread of its own, so a database that
/// is slow or down holds no listener change behind the registry's lock. The rewrites run one at a time and each reads
/// the properties as it runs, so the last one writes what is current. Before the cluster starts it writes nothing.
pub(crate) fn rewrite_member_row(factory: &Arc<ExecutionContextFactory>) {
    let factory = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-ssh-member".to_string())
        .spawn(move || {
            let Some(factory) = factory.upgrade() else {
                return;
            };
            let rewrites = factory.extension(|_| MemberRewrites::default());
            let _one = lock(&rewrites.0);
            moqui_core::cluster::refresh_member(&factory);
        });
    if let Err(err) = spawned {
        tracing::error!("This member's cluster row keeps its old SSH address: no thread to rewrite it: {err}");
    }
}

/// What serves one connection for as long as the SSH server is the factory's: its settings, its shared state and
/// counts. Each bind's accept loop hands it each TCP connection with that bind's configs and stop, and an edge link's
/// SSH tunnel hands it a tunnel stream with the client's address ([`serve_stream`], plans/archive/Edge-Node.md §5.3),
/// which ends with the tunnels' own stop.
pub(super) struct Intake {
    pub(super) shared: Arc<Shared>,
    pub(super) counts: Arc<Counts>,
    /// The settings applied in place, which each new connection, login and channel reads when it asks.
    pub(super) live: Arc<ArcSwap<SshLive>>,
    /// The newest bind's configs, which a tunnel stream is served with; none before the first bind.
    configs: ArcSwapOption<Configs>,
    /// True while the sign-in dialog is not offered (`access` is not `open`, or the terminal client is off): the
    /// sessions still at the dialog end (`super::server::dialog_closed`).
    pub(super) dialog_off: watch::Sender<bool>,
    /// The edge tunnels' sessions' stop, while SSH runs.
    tunnels: Mutex<Tunnels>,
}

/// The edge tunnels' sessions: taken while SSH runs, ended when it is off.
#[derive(Default)]
struct Tunnels {
    /// Their stop while SSH runs; none while it is off, when a tunnel stream is refused.
    open: Option<watch::Sender<Option<Ending>>>,
    /// The stop of those the last close is ending, kept for "end connections now" while they have their grace.
    closing: Option<watch::Sender<Option<Ending>>>,
    /// How the last bind's stop began, which the tunnels' follows when SSH is then off.
    last_stop: Option<Ending>,
}

impl Intake {
    /// Take `live` from now on ([`SshListener::apply`]): the limits at once, the access mode for the next connection
    /// and login, and the sessions at the sign-in dialog end when it is no longer offered.
    fn apply(&self, live: &SshLive) {
        self.counts.set(&live.limits);
        self.live.store(Arc::new(live.clone()));
        let off = !live.takes(Method::Dialog);
        let changed = self
            .dialog_off
            .send_if_modified(|was| std::mem::replace(was, off) != off);
        if changed && off {
            tracing::info!(
                "SSH's sign-in dialog is no longer offered (access {}, terminal client {}): the sessions at it end",
                live.access.word(),
                if live.tui { "on" } else { "off" }
            );
        }
    }

    /// SSH is off: no tunnel stream is taken, and the tunnels' sessions end as the last bind's stop ended its own.
    fn close_tunnels(&self) {
        let mut tunnels = lock(&self.tunnels);
        let Some(open) = tunnels.open.take() else {
            return;
        };
        upgrade(&open, tunnels.last_stop.unwrap_or(Ending::Now));
        // the ones an earlier close left in their grace end now, with their sender
        tunnels.closing = Some(open);
    }

    /// "End connections now": the tunnels' sessions a close left in their grace end now.
    fn end_tunnels_now(&self) {
        if let Some(closing) = &lock(&self.tunnels).closing {
            upgrade(closing, Ending::Now);
        }
    }
}

/// The server's [`Intake`], kept on the factory from its first start.
#[derive(Default)]
struct IntakeSlot(std::sync::Mutex<Option<Arc<Intake>>>);

/// This factory's intake, once its server has started.
fn intake_if_made(factory: &ExecutionContextFactory) -> Option<Arc<Intake>> {
    lock(&factory.extension_if_present::<IntakeSlot>()?.0).clone()
}

/// This factory's intake, made at its server's first start with the settings `live`, the SFTP root of the conf (made
/// in `runtime_dir` when it is a `runtime:` location) and `cluster_posts`.
fn intake(
    factory: &Arc<ExecutionContextFactory>,
    runtime_dir: &std::path::Path,
    cluster_posts: Arc<std::sync::Mutex<Vec<String>>>,
    live: &SshLive,
) -> Arc<Intake> {
    let slot = factory.extension(|_| IntakeSlot::default());
    let mut made = lock(&slot.0);
    if let Some(intake) = made.as_ref() {
        return Arc::clone(intake);
    }
    let root = match factory.conf_setting("ssh-server", "sftp-root").as_str() {
        "" => "runtime://sftp".to_string(),
        other => other.to_string(),
    };
    if root.starts_with("runtime:") {
        let relative = root.trim_start_matches("runtime://").trim_start_matches("runtime:");
        let _ = std::fs::create_dir_all(runtime_dir.join(relative.trim_start_matches('/')));
    }
    let shared = Arc::new(Shared {
        factory: Arc::downgrade(factory),
        root,
        #[cfg(feature = "tui")]
        apps: crate::term::Sessions::of(factory),
        #[cfg(feature = "tui")]
        agents: Arc::default(),
        cluster_posts,
    });
    let intake = Arc::new(Intake {
        shared,
        counts: Arc::new(Counts::of(&live.limits)),
        live: Arc::new(ArcSwap::from_pointee(live.clone())),
        configs: ArcSwapOption::empty(),
        dialog_off: watch::channel(!live.takes(Method::Dialog)).0,
        tunnels: Mutex::default(),
    });
    *made = Some(Arc::clone(&intake));
    intake
}

/// Serve `stream` as a connection from `peer` to this factory's SSH server, as one a bind accepted: the edge link's
/// SSH tunnel. False when SSH is not running.
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn serve_stream<S>(factory: &ExecutionContextFactory, stream: S, peer: SocketAddr) -> bool
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Some(intake) = intake_if_made(factory) else {
        return false;
    };
    let Some(stop) = lock(&intake.tunnels).open.as_ref().map(watch::Sender::subscribe) else {
        return false;
    };
    let Some(configs) = intake.configs.load_full() else {
        return false;
    };
    serve_connection(&intake, stream, peer, true, &configs, stop, None);
    true
}

/// Whether this factory's SSH server runs, to take [`serve_stream`].
#[cfg_attr(not(feature = "edge"), allow(dead_code))]
pub(crate) fn running(factory: &ExecutionContextFactory) -> bool {
    intake_if_made(factory).is_some_and(|intake| lock(&intake.tunnels).open.is_some())
}

/// One bind's accept loop: each connection is served with the bind's `configs` and ends with its stop, which ends the
/// loop too and lets the port go.
async fn accept_loop(
    listener: TcpListener,
    intake: Arc<Intake>,
    configs: Arc<Configs>,
    mut stop: watch::Receiver<Option<Ending>>,
    bind: Arc<BindState>,
) {
    let _loop = bind.hold(false);
    // a local declared after the guard, so the port is let go before the guard says so, however the loop ends
    let listener = listener;
    loop {
        let accepted = tokio::select! {
            _ = stop.wait_for(Option::is_some) => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, peer)) = accepted else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        serve_connection(&intake, stream, peer, false, &configs, stop.clone(), Some(&bind));
    }
}

#[cfg(test)]
#[path = "intake_tests.rs"]
mod tests;
