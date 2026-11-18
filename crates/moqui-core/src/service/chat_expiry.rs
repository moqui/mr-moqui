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

//! The chat expiry worker (plans/Comms-Messaging.md §18.3 phase 16): disappearing messages and view-once media are
//! deleted at their time, not by a daily job. The rules are in
//! `framework/service/org/moqui/chat/ChatExpiryServices.xml`; this is only the clock.
//!
//! - A thread per node ([`start`], beside the push worker). It sleeps until the earliest `ChatEvent.expireDate` (one
//!   row of the index `CHAT_EVENT_EXPIRE`), at most [`SWEEP`], so it also sees what another node wrote, then runs a
//!   round: `ChatExpiryServices.expire#DueMessages`, again at once while it answers `more`.
//! - `append#Event` calls `wake#ChatExpiry` with an event's due time: once the transaction commits, the worker wakes
//!   when that time is sooner than its planned wake.
//! - A round that fails is logged; the next starts within [`SWEEP`]. A round that deleted nothing while a room failed
//!   or was skipped (it failed lately: the service skips it a while on this node) waits by [`backoff`] instead of the
//!   next due time, which is that room's and past, so a failing room never spins the worker. The rooms of a round are each on their own (the
//!   service's rule). In a cluster two nodes may run the same round: the room row is locked first, so the second finds
//!   nothing left.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use moqui_db::sql::Condition;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::chat_bridge::{current_factory, millis_of, now_millis, param};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

/// The longest the worker sleeps without looking.
pub const SWEEP: Duration = Duration::from_secs(60);
/// The rounds run back to back while a round answers `more`.
const MAX_ROUNDS: usize = 20;
const ROUND_SERVICE: &str = "org.moqui.chat.ChatExpiryServices.expire#DueMessages";

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, wake_chat_expiry)?;
    Ok(())
}

/// This node's worker: whether it runs, the soonest due time it was told of, and its thread.
#[derive(Default)]
struct Worker {
    started: AtomicBool,
    /// Woken: Some(due millis), or Some(0) to look at once.
    wake: Mutex<Option<i64>>,
    signal: Condvar,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

fn worker(factory: &ExecutionContextFactory) -> Arc<Worker> {
    factory.extension(|_| Worker::default())
}

/// How long to sleep at `now` before the next due time `next` (none known: [`SWEEP`]); never past [`SWEEP`].
pub fn wait_for(now: i64, next: Option<i64>) -> Duration {
    let sweep = i64::try_from(SWEEP.as_millis()).unwrap_or(60_000);
    match next {
        None => SWEEP,
        Some(due) => Duration::from_millis(u64::try_from((due - now).clamp(0, sweep)).unwrap_or(0)),
    }
}

/// The soonest of two due times.
pub fn sooner(kept: Option<i64>, due: i64) -> i64 {
    kept.map_or(due, |kept| kept.min(due))
}

/// Wake this node's worker for a due time (0: look now).
pub fn wake(factory: &ExecutionContextFactory, due: i64) {
    let worker = worker(factory);
    let mut guard = lock(&worker.wake);
    *guard = Some(sooner(*guard, due));
    worker.signal.notify_all();
}

/// `wake#ChatExpiry` (`dueMillis`): wake the worker once the caller's transaction commits (at once outside one;
/// nothing on a rollback).
fn wake_chat_expiry(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let due = param(parameters, "dueMillis")
        .and_then(|text| text.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let Some(factory) = current_factory() else {
        return Ok(BTreeMap::new());
    };
    let weak = Arc::downgrade(&factory);
    let send = move || {
        if let Some(factory) = weak.upgrade() {
            wake(&factory, due);
        }
    };
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(send) {
                tracing::warn!("Could not wait for the commit to wake the chat expiry worker: {err}");
                wake(&factory, due);
            }
        }
        _ => send(),
    }
    Ok(BTreeMap::new())
}

/// The earliest due time of any event, from the index.
fn next_due(facade: &EntityFacade) -> Result<Option<i64>, LibError> {
    let rows = facade
        .find("moqui.chat.ChatEvent")?
        .condition(Condition::is_not_null("expireDate"))
        .order_by("expireDate", false)
        .limit(1)
        .disable_authz()
        .list()?;
    Ok(rows.values().first().and_then(|row| millis_of(row, "expireDate")))
}

/// What the rounds did: whether a room failed or was skipped (it failed lately) with nothing deleted, so the worker
/// waits before it looks again rather than at the next due time, which is that room's and past.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rounds {
    pub deleted: i64,
    pub failed: i64,
    pub skipped: i64,
}

impl Rounds {
    /// Nothing went and something is held back (failed now or lately): the next due time cannot be trusted.
    pub fn stuck(&self) -> bool {
        self.deleted == 0 && (self.failed > 0 || self.skipped > 0)
    }
}

/// The wait after `rounds` that were stuck `times` times in a row: 5 s, doubled, at most [`SWEEP`].
pub fn backoff(times: u32) -> Duration {
    let millis = 5_000_u64.saturating_mul(1_u64 << times.min(10));
    Duration::from_millis(millis).min(SWEEP)
}

fn count(out: &BTreeMap<String, Value>, key: &str) -> i64 {
    out.get(key)
        .map(moqui_lib::object::to_plain_string)
        .and_then(|text| text.parse().ok())
        .unwrap_or(0)
}

/// One round and its follow-ups while there is more (none after a failure); the errors are the round's, logged by the
/// caller.
fn rounds() -> Result<Rounds, LibError> {
    let mut done = Rounds::default();
    for _ in 0..MAX_ROUNDS {
        let out = super::call::sync_checked(ROUND_SERVICE, &BTreeMap::new(), &super::call::CallOptions::default())
            .map_err(|err| LibError::new(err.to_string()))?;
        done.deleted += count(&out, "deletedCount");
        done.failed += count(&out, "failedCount");
        done.skipped += count(&out, "skippedCount");
        let more = out.get("more").is_some_and(|more| matches!(more, Value::Bool(true)));
        if !more {
            break;
        }
    }
    Ok(done)
}

/// Start this node's worker thread, once (the host calls it after the cluster starts). Its first round runs at once:
/// what came due while the server was down.
pub fn start(factory: &Arc<ExecutionContextFactory>) {
    if factory.entity().is_none() {
        return;
    }
    let state = worker(factory);
    if state.started.swap(true, Ordering::SeqCst) {
        return;
    }
    let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
    let spawned = std::thread::Builder::new()
        .name("moqui-chat-expiry".to_string())
        .spawn(move || {
            let mut planned: Option<i64> = Some(0);
            let mut stuck_times: u32 = 0;
            loop {
                let worker = {
                    let Some(factory) = weak.upgrade() else {
                        return;
                    };
                    if factory.is_shutting_down() {
                        return;
                    }
                    worker(&factory)
                };
                // sleep until the planned time, or a sooner wake
                {
                    let mut guard = lock(&worker.wake);
                    loop {
                        if let Some(due) = guard.take() {
                            planned = Some(sooner(planned, due));
                        }
                        let wait = wait_for(now_millis(), planned);
                        if wait.is_zero() {
                            break;
                        }
                        let (next, timed_out) = worker
                            .signal
                            .wait_timeout(guard, wait)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        guard = next;
                        if timed_out.timed_out() && guard.is_none() {
                            break;
                        }
                    }
                }
                let Some(factory) = weak.upgrade() else {
                    return;
                };
                if factory.is_shutting_down() {
                    return;
                }
                let Some(facade) = factory.entity() else {
                    return;
                };
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(rounds));
                let stuck = match outcome {
                    Ok(Ok(done)) => done.stuck(),
                    Ok(Err(error)) => {
                        tracing::error!("the chat expiry round failed: {error}");
                        true
                    }
                    Err(_) => {
                        tracing::error!("the chat expiry round panicked");
                        true
                    }
                };
                planned = if stuck {
                    // a failing room's due time is past: waiting for it would spin; wait more each time instead
                    let wait = backoff(stuck_times);
                    stuck_times = stuck_times.saturating_add(1);
                    Some(now_millis() + i64::try_from(wait.as_millis()).unwrap_or(60_000))
                } else {
                    stuck_times = 0;
                    match next_due(&facade) {
                        Ok(next) => next,
                        Err(error) => {
                            tracing::error!("the chat expiry worker could not read the next due time: {error}");
                            None
                        }
                    }
                };
            }
        });
    match spawned {
        Ok(handle) => *lock(&state.thread) = Some(handle),
        Err(error) => tracing::error!("the chat expiry worker did not start: {error}"),
    }
}

/// Wake this node's worker to end, and wait for it. The factory calls it as it shuts down.
pub(crate) fn shutdown(factory: &ExecutionContextFactory) {
    let worker = worker(factory);
    let Some(handle) = lock(&worker.thread).take() else {
        return;
    };
    *lock(&worker.wake) = Some(0);
    worker.signal.notify_all();
    if handle.thread().id() != std::thread::current().id() && handle.join().is_err() {
        tracing::error!("the chat expiry worker panicked");
    }
}

#[cfg(test)]
#[path = "chat_expiry_tests.rs"]
mod tests;
