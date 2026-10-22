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
//! it runs an Inbox sync (`sync#EmailAccount` with `inboxOnly`). A connection that fails is made again after 30
//! seconds, then twice as long each time up to 15 minutes; a refused sign-in or a server without IDLE ends the worker.
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

use super::sync::{account_info, connect_imap, run_sync, select, SyncFailure, SyncOptions, ACCOUNT};
use crate::context::ExecutionContextFactory;
use crate::entity::{Condition, EntityFacade};
use crate::service::util::field_text;

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
                crate::service::pool::panic_text(payload.as_ref())
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
    if let Err(err) = select(&mut conn, "INBOX", true) {
        return Ended::Retry(format!("SELECT INBOX: {err}"));
    }
    if resync {
        sync_inbox(factory, account_id, stop);
    }
    while !stop.load(Ordering::SeqCst) {
        // what came before the IDLE (a NOOP gathers it) is a change too, and a server may not send it during the IDLE
        if let Err(err) = conn.session.noop() {
            return ended_by(stop, format!("NOOP: {err}"));
        }
        let mut changed = conn.session.take_all_unsolicited().any(|response| changes(&response));
        if !changed {
            let waited = {
                let mut idle = conn.session.idle();
                idle.timeout(IDLE_RENEW).keepalive(false);
                idle.wait_while(|response| !changes(&response))
            };
            // the IDLE leaves the connection with no read timeout; the commands after it keep the usual minute
            if let Some(socket) = &conn.socket {
                let _ = socket.set_read_timeout(Some(Duration::from_secs(60)));
            }
            match waited {
                Ok(imap::extensions::idle::WaitOutcome::MailboxChanged) => changed = true,
                Ok(imap::extensions::idle::WaitOutcome::TimedOut) => {}
                Err(err) => return ended_by(stop, format!("IDLE: {err}")),
            }
        }
        if changed && !stop.load(Ordering::SeqCst) {
            sync_inbox(factory, account_id, stop);
        }
    }
    let _ = conn.session.logout();
    Ended::Stopped
}

fn ended_by(stop: &AtomicBool, text: String) -> Ended {
    if stop.load(Ordering::SeqCst) {
        Ended::Stopped
    } else {
        Ended::Retry(text)
    }
}

fn changes(response: &imap::types::UnsolicitedResponse) -> bool {
    use imap::types::UnsolicitedResponse;
    matches!(
        response,
        UnsolicitedResponse::Exists(_)
            | UnsolicitedResponse::Expunge(_)
            | UnsolicitedResponse::Vanished { .. }
            | UnsolicitedResponse::Fetch { .. }
            | UnsolicitedResponse::Recent(_)
    )
}

/// An Inbox sync in a context of its own; one already running is waited for a little, so this change is not missed.
fn sync_inbox(factory: &Arc<ExecutionContextFactory>, account_id: &str, stop: &AtomicBool) {
    let _cleanup = crate::service::pool::WorkCleanup::new(Some(Arc::clone(factory)));
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
