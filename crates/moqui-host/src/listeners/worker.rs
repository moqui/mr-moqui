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

//! The `moqui-listeners` thread (plans/Runtime-Config.md §8.1): the passes asked for after the start. A conf hook may
//! fire on a thread that holds a transaction, and a service runs with an ExecutionContext entered, so hooks, signals
//! and services only enqueue a request here; this thread merges what waits into one pass (the causes merged, the
//! listeners to end at once joined, a deferred request held until it is due) and answers each request that asked.
//! Started on the first request, it holds the registry weakly and ends when the registry does or
//! [`super::Registry::join_worker`] stops it.

use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_core::listeners::{Cause, Outcome, ReconcileRequest};
use moqui_lib::sync::lock;

use super::{panic_text, Inner, Scope};

/// What a request is answered with.
type Answer = Result<Vec<Outcome>, String>;

/// The worker of one registry.
pub(super) enum State {
    /// Not started: no request yet.
    Idle,
    Running(Worker),
    /// Stopped: a request is answered with a disconnect.
    Ended,
}

pub(super) struct Worker {
    jobs: Sender<Job>,
    thread: JoinHandle<()>,
}

/// One request waiting for its pass.
struct Job {
    request: ReconcileRequest,
    due: Instant,
    reply: SyncSender<Answer>,
}

/// Queue `request` for a pass after `delay`, starting the worker on the first request. A boot or shutdown cause is
/// answered with an error at once: the staged boot passes and the stops are the server's own, never the worker's.
pub(super) fn submit(inner: &Arc<Inner>, request: ReconcileRequest, delay: Duration) -> Receiver<Answer> {
    let (reply, answer) = mpsc::sync_channel(1);
    if matches!(request.cause, Cause::Boot | Cause::Shutdown) {
        let refused = format!("a {} pass is the server's own, not one to ask for", request.cause);
        tracing::warn!("A listener reconcile was refused: {refused}");
        let _ = reply.send(Err(refused));
        return answer;
    }
    let job = Job {
        request,
        due: super::deadline_after(delay),
        reply,
    };
    let mut state = lock(&inner.worker);
    if matches!(*state, State::Idle) {
        let (jobs, queue) = mpsc::channel();
        let registry = Arc::downgrade(inner);
        let spawned = std::thread::Builder::new()
            .name("moqui-listeners".to_string())
            .spawn(moqui_core::context::framework_thread(move || work(&registry, &queue)));
        match spawned {
            Ok(thread) => *state = State::Running(Worker { jobs, thread }),
            Err(err) => {
                tracing::error!("The listener worker thread did not start, listener changes wait for a restart: {err}");
                *state = State::Ended;
            }
        }
    }
    // with no worker the job, and its reply sender, is dropped here: the receiver reads a disconnect
    if let State::Running(worker) = &*state {
        let _ = worker.jobs.send(job);
    }
    answer
}

/// Stop the worker and wait for a pass it is running; a later request is answered with a disconnect.
pub(super) fn join(inner: &Inner) {
    let state = std::mem::replace(&mut *lock(&inner.worker), State::Ended);
    let State::Running(Worker { jobs, thread }) = state else {
        return;
    };
    drop(jobs);
    // a listener's stop that ends the registry from within a pass must not wait on itself
    if thread.thread().id() == std::thread::current().id() {
        return;
    }
    if thread.join().is_err() {
        tracing::error!("The listener worker thread panicked");
    }
}

/// The worker's thread, swapped for one already done, so a test can wait for the worker to end without holding the
/// registry.
#[cfg(test)]
pub(super) fn take_thread(inner: &Inner) -> Option<JoinHandle<()>> {
    match &mut *lock(&inner.worker) {
        State::Running(worker) => Some(std::mem::replace(&mut worker.thread, std::thread::spawn(|| {}))),
        State::Idle | State::Ended => None,
    }
}

fn work(registry: &Weak<Inner>, queue: &Receiver<Job>) {
    let mut waiting: Vec<Job> = Vec::new();
    loop {
        let next = match waiting.iter().map(|job| job.due).min() {
            None => queue.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(due) => queue.recv_timeout(due.saturating_duration_since(Instant::now())),
        };
        match next {
            Ok(job) => waiting.push(job),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        waiting.extend(queue.try_iter());
        if !waiting.iter().any(|job| job.due <= Instant::now()) {
            continue;
        }
        let Some(inner) = registry.upgrade() else {
            return;
        };
        let mut entries = lock(&inner.entries);
        // the requests that came while this pass waited for the lock (another pass, the boot) join it
        waiting.extend(queue.try_iter());
        let now = Instant::now();
        let (due, later): (Vec<Job>, Vec<Job>) = waiting.drain(..).partition(|job| job.due <= now);
        waiting = later;
        let (causes, end_now) = merge(&due);
        let answer = catch_unwind(AssertUnwindSafe(|| {
            inner.pass(&mut entries, Scope::All, &causes, &end_now, false)
        }))
        .unwrap_or_else(|panic| {
            Err(moqui_lib::error::Error::new(format!(
                "the pass panicked: {}",
                panic_text(panic.as_ref())
            )))
        })
        .map_err(|err| err.message().to_string());
        drop(entries);
        if let Err(err) = &answer {
            tracing::error!("A listener reconcile failed: {err}");
        }
        for job in due {
            let _ = job.reply.send(answer.clone());
        }
    }
}

/// The causes of `jobs`, each once in the order they came, and the listeners any of them ends at once.
fn merge(jobs: &[Job]) -> (Vec<Cause>, Vec<String>) {
    let mut causes: Vec<Cause> = Vec::new();
    let mut end_now: Vec<String> = Vec::new();
    for job in jobs {
        if !causes.contains(&job.request.cause) {
            causes.push(job.request.cause.clone());
        }
        for name in &job.request.end_now {
            if !end_now.contains(name) {
                end_now.push(name.clone());
            }
        }
    }
    (causes, end_now)
}

/// Run `future` on `handle` and wait up to `limit` for it on a plain channel, as `crate::edge::block_on` does: an
/// entry's async bind from the pass's plain thread, which holds the registry's lock, so a bind that never resolves is
/// aborted at the limit rather than holding the lock. Never `Handle::block_on`, which panics on a runtime's worker
/// thread.
///
/// # Errors
///
/// When the future did not finish within `limit`, panicked, or the runtime dropped it.
pub(crate) fn on_runtime<F>(handle: &tokio::runtime::Handle, limit: Duration, future: F) -> Result<F::Output, String>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    let task = handle.spawn(async move {
        let _ = tx.send(future.await);
    });
    match rx.recv_timeout(limit) {
        Ok(output) => Ok(output),
        Err(RecvTimeoutError::Timeout) => {
            task.abort();
            Err(format!(
                "the listener's task did not finish within {:.1} s",
                limit.as_secs_f64()
            ))
        }
        Err(RecvTimeoutError::Disconnected) => {
            Err("the listener's task panicked or its runtime dropped it before it finished".to_string())
        }
    }
}
