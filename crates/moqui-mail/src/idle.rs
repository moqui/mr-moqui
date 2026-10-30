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

//! IMAP IDLE on the Inbox of each active account, so new mail shows at once instead of at the next sync job.
//!
//! A worker thread per account, at most `email-client` `idle-max` (8) of them in a server; the others rely on the job. The host
//! starts the pool after boot ([`start`]); the scheduler's tick (every `scheduled_job_check_time`) starts a worker
//! for an account that became active and stops one whose account was paused, removed or changed. A worker signs in,
//! selects INBOX and waits in IDLE, renewed every ten minutes; when the server reports EXISTS, EXPUNGE or a flag change
//! it runs an Inbox sync (`sync#EmailAccount` with `inboxOnly`), as it does when the Inbox, selected again after each
//! IDLE and each sync, holds a different count than when the last sync started (mail no report covered). A connection
//! that fails is made again after 30 seconds, then twice as long each time up to 15 minutes; a refused sign-in or a
//! server without IDLE ends the worker.
//! `email-client idle-enabled="false"` in the conf turns it all off, for tests and small deployments that rely on the job. The factory's
//! shutdown stops the workers: each connection is shut down, which ends its wait, and the threads are joined.

use std::collections::HashMap;
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::sync::{account_info, connect_imap, run_sync, select, ImapConn, SyncFailure, SyncOptions, ACCOUNT};
use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::{Condition, EntityFacade};
use moqui_core::service::util::field_text;

/// How long one IDLE waits before it is renewed (RFC 2177 says under 29 minutes).
const IDLE_RENEW: Duration = Duration::from_secs(10 * 60);
const FIRST_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// How long a stop waits for the workers.
const STOP_WAIT: Duration = Duration::from_secs(10);

struct Worker {
    fingerprint: String,
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    fn signal(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(socket) = lock(&self.socket).as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }

    fn finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

#[derive(Default)]
struct IdlePool {
    workers: Mutex<HashMap<String, Worker>>,
    installed: AtomicBool,
    stopped: AtomicBool,
}

fn pool(factory: &ExecutionContextFactory) -> Arc<IdlePool> {
    factory.extension(|_| IdlePool::default())
}

/// The conf's `email-client` `idle-enabled`, true unless it is false.
fn enabled(factory: &ExecutionContextFactory) -> bool {
    factory.conf_flag("email-client", "idle-enabled", true)
}

/// The conf's `email-client` `idle-max`, 8 when unset.
fn max_workers() -> usize {
    super::client_setting("idle-max").parse::<usize>().unwrap_or(8)
}

/// Start the IDLE workers on the scheduler's tick. Called once by the host after boot.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>) {
    let pool = pool(factory);
    if pool.installed.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak = Arc::downgrade(factory);
    factory.service_pools().add_tick_hook(move || {
        if let Some(factory) = weak.upgrade() {
            reconcile(&factory);
        }
    });
}

/// Stop every worker and wait for them a little. Called by the factory's shutdown.
pub(crate) fn stop_all(factory: &ExecutionContextFactory) {
    let pool = pool(factory);
    pool.stopped.store(true, Ordering::SeqCst);
    let workers: Vec<Worker> = lock(&pool.workers).drain().map(|(_, worker)| worker).collect();
    for worker in &workers {
        worker.signal();
    }
    let deadline = Instant::now() + STOP_WAIT;
    for mut worker in workers {
        let Some(handle) = worker.handle.take() else {
            continue;
        };
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

/// The accounts IDLE should watch: active, with an IMAP server, the first `email-client` `idle-max` by id.
fn wanted_accounts(facade: &EntityFacade) -> Result<Vec<(String, String)>, moqui_lib::error::Error> {
    let rows = facade
        .find(ACCOUNT)?
        .condition(Condition::is_not_null("emailServerId"))
        .condition(Condition::not_equals("accountKindEnumId", Value::string("EakInternal")))
        .condition(Condition::or(vec![
            Condition::is_null("statusId"),
            Condition::equals("statusId", Value::string("EasActive")),
        ]))
        .order_by("emailAccountId", false)
        .list()?;
    let mut out = Vec::new();
    for row in rows.values() {
        if out.len() >= max_workers() {
            break;
        }
        match account_info(facade, row) {
            Ok(account) => {
                if let Some(server) = account.server.filter(super::sync::ServerInfo::is_imap) {
                    out.push((account.id, server.fingerprint()));
                }
            }
            Err(err) => tracing::warn!("Email account {}: {err}", field_text(row, "emailAccountId")),
        }
    }
    Ok(out)
}

/// Start the workers that are wanted and missing, stop those no longer wanted or whose server changed.
pub(super) fn reconcile(factory: &Arc<ExecutionContextFactory>) {
    let pool = pool(factory);
    if pool.stopped.load(Ordering::SeqCst) {
        return;
    }
    let wanted = if enabled(factory) {
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let Some(facade) = factory.entity() else {
            return;
        };
        match wanted_accounts(&facade) {
            Ok(wanted) => wanted,
            Err(err) => {
                tracing::warn!("Could not read the email accounts to watch: {err}");
                return;
            }
        }
    } else {
        Vec::new()
    };
    let mut workers = lock(&pool.workers);
    // ended on their own (a refused sign-in, no IDLE) or no longer wanted
    let leaving: Vec<String> = workers
        .iter()
        .filter(|(id, worker)| {
            worker.finished()
                || !wanted
                    .iter()
                    .any(|(wanted_id, print)| wanted_id == *id && *print == worker.fingerprint)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in leaving {
        if let Some(worker) = workers.remove(&id) {
            worker.signal();
        }
    }
    for (id, fingerprint) in wanted {
        if workers.contains_key(&id) {
            continue;
        }
        if let Some(worker) = spawn_worker(factory, &id, fingerprint) {
            workers.insert(id, worker);
        }
    }
}

fn spawn_worker(factory: &Arc<ExecutionContextFactory>, account_id: &str, fingerprint: String) -> Option<Worker> {
    let stop = Arc::new(AtomicBool::new(false));
    let socket = Arc::new(Mutex::new(None));
    let weak = Arc::downgrade(factory);
    let thread_stop = Arc::clone(&stop);
    let thread_socket = Arc::clone(&socket);
    let thread_account = account_id.to_string();
    let spawned = std::thread::Builder::new()
        .name(format!("moqui-email-idle-{account_id}"))
        .spawn(move || run_worker(&weak, &thread_account, &thread_stop, &thread_socket));
    match spawned {
        Ok(handle) => Some(Worker {
            fingerprint,
            stop,
            socket,
            handle: Some(handle),
        }),
        Err(err) => {
            tracing::error!("Could not start the IDLE worker of email account {account_id}: {err}");
            None
        }
    }
}

/// Start a worker for one account now, for a test.
#[cfg(test)]
pub(super) fn start_worker_now(factory: &Arc<ExecutionContextFactory>, account_id: &str) {
    let pool = pool(factory);
    pool.stopped.store(false, Ordering::SeqCst);
    if let Some(worker) = spawn_worker(factory, account_id, String::new()) {
        lock(&pool.workers).insert(account_id.to_string(), worker);
    }
}

/// The accounts with a worker now, for a test.
#[cfg(test)]
pub(super) fn worker_accounts(factory: &ExecutionContextFactory) -> Vec<String> {
    let mut ids: Vec<String> = lock(&pool(factory).workers).keys().cloned().collect();
    ids.sort();
    ids
}

enum Ended {
    /// Asked to stop.
    Stopped,
    /// For good: a refused sign-in, a server without IDLE, an account no longer active.
    Done(String),
    /// Try again after the backoff.
    Retry(String),
}

fn run_worker(
    factory: &Weak<ExecutionContextFactory>,
    account_id: &str,
    stop: &AtomicBool,
    socket: &Mutex<Option<TcpStream>>,
) {
    let mut backoff = FIRST_BACKOFF;
    // after a connection that failed, what changed meanwhile is synced once the next one is up
    let mut resync = false;
    while !stop.load(Ordering::SeqCst) {
        let Some(strong) = factory.upgrade() else {
            return;
        };
        let started = Instant::now();
        let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            watch(&strong, account_id, stop, socket, resync)
        }))
        .unwrap_or_else(|payload| {
            Ended::Retry(format!(
                "the worker stopped: {}",
                moqui_core::service::pool::panic_text(payload.as_ref())
            ))
        });
        drop(strong);
        *lock(socket) = None;
        match ended {
            Ended::Stopped => return,
            Ended::Done(text) => {
                tracing::info!("IDLE for email account {account_id} ends: {text}");
                return;
            }
            Ended::Retry(text) => {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                resync = true;
                // a connection that lasted a while starts the backoff again
                if started.elapsed() > MAX_BACKOFF {
                    backoff = FIRST_BACKOFF;
                }
                tracing::warn!(
                    "IDLE for email account {account_id} failed, trying again in {}s: {text}",
                    backoff.as_secs()
                );
                let until = Instant::now() + backoff;
                while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(250));
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// One connection: sign in, select INBOX, IDLE until stopped or the connection fails, syncing the Inbox on a change
/// (and once at the start with `resync`, after a connection that failed).
fn watch(
    factory: &Arc<ExecutionContextFactory>,
    account_id: &str,
    stop: &AtomicBool,
    socket: &Mutex<Option<TcpStream>>,
    resync: bool,
) -> Ended {
    // the sign-in in a context of its own: an OAuth server's access token is read, or refreshed, there
    let mut conn = {
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let Some(facade) = factory.entity() else {
            return Ended::Retry("no entity facade".to_string());
        };
        let account = match super::sync::load_account(&facade, account_id) {
            Ok(Some(account)) => account,
            Ok(None) => return Ended::Done("the account is gone".to_string()),
            Err(err) => return Ended::Retry(err.to_string()),
        };
        if account.status != "EasActive" {
            return Ended::Done(format!("the account is {}", account.status));
        }
        let Some(server) = account.server.filter(super::sync::ServerInfo::is_imap) else {
            return Ended::Done("the account has no IMAP server".to_string());
        };
        match connect_imap(&facade, &server) {
            Ok(conn) => conn,
            Err(SyncFailure::Auth(text) | SyncFailure::SignIn(text)) => return Ended::Done(text),
            Err(SyncFailure::Connect(text)) => return Ended::Retry(text),
        }
    };
    *lock(socket) = conn.socket.as_ref().and_then(|socket| socket.try_clone().ok());
    if stop.load(Ordering::SeqCst) {
        let _ = conn.session.logout();
        return Ended::Stopped;
    }
    if !conn.caps.has("IDLE") {
        let _ = conn.session.logout();
        return Ended::Done("the server has no IDLE; the sync job reads the account".to_string());
    }
    // How many messages the Inbox held when the last sync started, from a SELECT. A server reports in an IDLE what
    // changes while it runs, and should report then too what came since this connection was last told, but mail that
    // comes while the sync runs (on a connection of its own), or between a SELECT and the IDLE, may be reported by no
    // server at all, and an EXISTS read with the IDLE's tagged answer (after DONE) is lost. So after each IDLE, and
    // after each sync, the Inbox is selected again and its count compared with this one: a difference is new mail (or
    // mail gone), synced at once instead of at the next change or renewal. A count that stayed the same while a message
    // came and another went is left to the IDLE's report of each.
    let mut known = match select(&mut conn, "INBOX", true) {
        Ok(selected) => selected.mailbox.exists,
        Err(err) => return Ended::Retry(format!("SELECT INBOX: {err}")),
    };
    let mut due = resync;
    while !stop.load(Ordering::SeqCst) {
        if due {
            sync_inbox(factory, account_id, stop);
            match select(&mut conn, "INBOX", true) {
                // mail that came while the sync ran: synced again (`due` stays)
                Ok(selected) if selected.mailbox.exists != known => {
                    known = selected.mailbox.exists;
                    continue;
                }
                Ok(_) => {}
                Err(err) => return ended_by(stop, format!("SELECT INBOX: {err}")),
            }
        }
        let changed = match idle_wait(&mut conn) {
            Ok(changed) => changed,
            Err(text) => return ended_by(stop, text),
        };
        match select(&mut conn, "INBOX", true) {
            Ok(selected) => {
                // the count before the sync, so mail that comes during it differs at the look after it
                due = changed || selected.mailbox.exists != known;
                known = selected.mailbox.exists;
            }
            Err(err) => return ended_by(stop, format!("SELECT INBOX: {err}")),
        }
        due &= !stop.load(Ordering::SeqCst);
    }
    let _ = conn.session.logout();
    Ended::Stopped
}

/// One line from the server through the imap client's buffer: its `read_greeting` reads a line, and reads one again
/// once `greeting_read` is cleared. The client has no other public way to read a line.
fn read_line(conn: &mut ImapConn) -> imap::error::Result<String> {
    conn.session.greeting_read = false;
    let line = conn.session.read_greeting();
    conn.session.greeting_read = true;
    line.map(|bytes| String::from_utf8_lossy(&bytes).trim_end().to_string())
}

/// One IDLE (RFC 2177) up to a change or [`IDLE_RENEW`]; true on a change. The imap client's own IDLE takes the line
/// after the command to be the `+` continuation, and stalls (or panics) on an `* n EXISTS` a server sends before it, so
/// the command and its lines are handled here: any EXISTS, EXPUNGE, RECENT, FETCH or VANISHED, before the `+` or after,
/// is a change; once the `+` came, a change or the renewal ends the IDLE with DONE. DONE goes through a fresh handle of
/// the client's IDLE dropped at once: one never started writes DONE when dropped and reads up to the tagged answer of
/// the IDLE sent here (the client's tag), so the session goes on in step. A stop shuts the socket, which ends the read.
fn idle_wait(conn: &mut ImapConn) -> Result<bool, String> {
    conn.session.run_command("IDLE").map_err(|err| format!("IDLE: {err}"))?;
    let deadline = Instant::now() + IDLE_RENEW;
    let mut idling = false;
    let mut changed = false;
    let waited = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if idling && (changed || left.is_zero()) {
            break Ok(changed);
        }
        if left.is_zero() {
            break Err("IDLE: the server sent no continuation".to_string());
        }
        if let Some(socket) = &conn.socket {
            let _ = socket.set_read_timeout(Some(left.max(Duration::from_millis(10))));
        }
        match read_line(conn) {
            Ok(line) if line.starts_with('+') => idling = true,
            Ok(line) if line.starts_with('*') => changed |= changes(&line),
            // a tagged answer: the server refused the IDLE, or ended it
            Ok(line) => break Err(format!("IDLE: {line}")),
            Err(imap::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(err) => break Err(format!("IDLE: {err}")),
        }
    };
    // the commands after the IDLE keep the usual minute
    if let Some(socket) = &conn.socket {
        let _ = socket.set_read_timeout(Some(Duration::from_secs(60)));
    }
    if waited.is_ok() {
        drop(conn.session.idle());
    }
    waited
}

fn ended_by(stop: &AtomicBool, text: String) -> Ended {
    if stop.load(Ordering::SeqCst) {
        Ended::Stopped
    } else {
        Ended::Retry(text)
    }
}

/// Whether an untagged line is a change of the mailbox: `* n EXISTS`, `* n EXPUNGE`, `* n RECENT`, `* n FETCH` or
/// `* VANISHED`; not `* OK Still here` and the like.
fn changes(line: &str) -> bool {
    let mut words = line.split_whitespace().skip(1);
    let first = words.next().unwrap_or_default().to_ascii_uppercase();
    if first == "VANISHED" {
        return true;
    }
    first.parse::<u32>().is_ok()
        && words.next().is_some_and(|word| {
            ["EXISTS", "EXPUNGE", "RECENT", "FETCH"]
                .iter()
                .any(|name| word.eq_ignore_ascii_case(name))
        })
}

/// An Inbox sync in a context of its own; one already running is waited for a little, so this change is not missed.
fn sync_inbox(factory: &Arc<ExecutionContextFactory>, account_id: &str, stop: &AtomicBool) {
    let _cleanup = moqui_core::service::pool::WorkCleanup::new(Some(Arc::clone(factory)));
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let Some(facade) = factory.entity() else {
        return;
    };
    let options = SyncOptions {
        inbox_only: true,
        ..SyncOptions::default()
    };
    for _ in 0..6 {
        let report = run_sync(&facade, account_id, &options);
        if report.outcome != "busy" || stop.load(Ordering::SeqCst) {
            return;
        }
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
