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

//! Per-factory request workers. Drop stops and joins only this pool's threads.

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use moqui_lib::node::MNode;
use moqui_lib::sync::{lock, wait};

use super::pool::WorkCleanup;
use crate::context::{Error, ErrorKind, ExecutionContextFactory};

const DEFAULT_REQUEST_WORKERS: u32 = 8;
const DEFAULT_REQUEST_QUEUE: u32 = 64;
const DEFAULT_SERVICE_WORKERS: u32 = 25;
const DEFAULT_TRANSFER_WORKERS: u32 = 4;

/// Factory-owned request workers. [`RequestPool::idle`] starts no threads.
///
/// Drop sets the stop flag, wakes this pool's workers, fails queued waiters, and joins the threads.
#[must_use = "dropping the pool stops and joins its threads"]
pub struct RequestPool {
    inner: Mutex<Option<Arc<Shared>>>,
}

struct Shared {
    state: Mutex<State>,
    workers: usize,
    queue_cap: usize,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct State {
    stop: bool,
    queue: VecDeque<Work>,
    /// The workers waiting for work, woken one at a time (plans/Profiling3.md W13).
    idle: super::idle::Idle,
}

type JobOutput = Box<dyn Any + Send>;
type JobRun = Box<dyn FnOnce() -> Result<JobOutput, Error> + Send>;
type JobFinish = Box<dyn FnOnce(Result<JobOutput, Error>) + Send>;

struct Work {
    factory: Arc<ExecutionContextFactory>,
    run: JobRun,
    finish: JobFinish,
}

impl RequestPool {
    /// No workers. Boot and `test_context` store this.
    pub fn idle() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Spawn `workers` threads (at least 1) and a queue of `queue` jobs.
    ///
    /// The same sizes, while this pool is running, leave it in place. Any other call stops the
    /// previous pool before the new one is visible to [`Self::try_submit`].
    pub fn start(&self, workers: usize, queue: usize) -> Result<(), Error> {
        let workers = workers.max(1);
        let previous = {
            let mut slot = lock(&self.inner);
            if let Some(existing) = slot.as_ref() {
                if existing.workers == workers && existing.queue_cap == queue && !lock(&existing.state).stop {
                    return Ok(());
                }
            }
            let shared = make_shared(workers, queue)?;
            slot.replace(shared)
        };
        if let Some(previous) = previous {
            shutdown(&previous);
        }
        Ok(())
    }

    pub(crate) fn start_from_conf(&self, conf_text: &str) -> Result<(), Error> {
        let (workers, queue) = request_sizes(conf_text)?;
        self.start(workers, queue)
    }

    /// Queue `job` and return a receiver. A full or stopped pool returns an error and does not run it.
    pub fn try_submit<T>(
        &self,
        factory: Arc<ExecutionContextFactory>,
        job: impl FnOnce() -> Result<T, Error> + Send + 'static,
    ) -> Result<std::sync::mpsc::Receiver<Result<T, Error>>, Error>
    where
        T: Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        self.try_submit_then(factory, job, move |result| {
            let _ = tx.send(result);
        })?;
        Ok(rx)
    }

    /// Queue `job` and hand its result to `done`, which runs on the worker once the job's context and cleanup have
    /// ended: the completion hook an async caller finishes its own future from, so no thread waits on the reply
    /// (`plans/High-Level-Review.md` HL-006). A full or stopped pool returns an error and calls neither; a job still
    /// queued when the pool stops gets `done` with an error.
    pub fn try_submit_then<T>(
        &self,
        factory: Arc<ExecutionContextFactory>,
        job: impl FnOnce() -> Result<T, Error> + Send + 'static,
        done: impl FnOnce(Result<T, Error>) + Send + 'static,
    ) -> Result<(), Error>
    where
        T: Send + 'static,
    {
        let shared = self.running()?;
        {
            let mut state = lock(&shared.state);
            if state.stop {
                return Err(Error::of_kind(ErrorKind::Unavailable, "request pool is not running"));
            }
            if state.queue.len() >= shared.queue_cap {
                return Err(Error::of_kind(ErrorKind::Unavailable, "request queue is full"));
            }
            state.queue.push_back(pack(factory, job, done));
            state.idle.wake_one();
        }
        Ok(())
    }

    /// Jobs waiting for a worker. A job a worker has already taken is not included.
    pub fn queue_depth(&self) -> usize {
        let shared = lock(&self.inner).as_ref().cloned();
        let Some(shared) = shared else {
            return 0;
        };
        let state = lock(&shared.state);
        state.queue.len()
    }

    pub(crate) fn shutdown(&self) {
        let shared = lock(&self.inner).take();
        if let Some(shared) = shared {
            shutdown(&shared);
        }
    }

    fn running(&self) -> Result<Arc<Shared>, Error> {
        lock(&self.inner)
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::of_kind(ErrorKind::Unavailable, "request pool is not running"))
    }
}

impl Drop for RequestPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `(request-pool-max, worker-pool-max)`. Missing or non-integer attributes are 8 and 25.
pub fn caps(conf_text: &str) -> (u32, u32) {
    let Ok(Some(tools)) = tools_of(conf_text) else {
        return (DEFAULT_REQUEST_WORKERS, DEFAULT_SERVICE_WORKERS);
    };
    (
        fit_u32(
            usize_attr(&tools, "request-pool-max", widen(DEFAULT_REQUEST_WORKERS)),
            DEFAULT_REQUEST_WORKERS,
        ),
        fit_u32(
            usize_attr(&tools, "worker-pool-max", widen(DEFAULT_SERVICE_WORKERS)),
            DEFAULT_SERVICE_WORKERS,
        ),
    )
}

/// Errors when `(request cap + service cap) * 2` does not fit the default group's pool, or the pool of a group linked
/// to it (a database of its own that each transaction spans, such as `content`), with room for one agent worker more
/// in a binary that links the agent runtime (crate::llm_hooks): the agent pool takes as many of its
/// `max-concurrent-runs` as the rest of the pool leaves ([`spare_connections`]), at least one.
///
/// The pool the `content` group reads from also holds a connection for each streamed database download reading a
/// window ([`transfer_readers`]). When the webapp's `transfer-workers` do not fit beside the workers and the agent
/// worker, this warns and the downloads read fewer at once, at least one (`plans/High-Level-Review.md` HL-022).
pub fn check_serve_caps(factory: &ExecutionContextFactory) -> Result<(), Error> {
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let (request_cap, service_cap) = caps(&factory.conf_text());
    let agent_workers = agent_workers();
    for (group, manager) in linked_managers(&facade)? {
        manager
            .check_caps_with(request_cap, service_cap, agent_workers)
            .map_err(|err| Error::new(format!("datasource {group}: {err}")))?;
    }
    let asked = transfer_workers(factory);
    let fits = transfer_readers(factory);
    if fits < asked {
        let (group, manager) = content_manager(&facade)?;
        let left = manager.spare_connections(request_cap, service_cap);
        tracing::warn!(
            "datasource {group}: the {asked} transfer-workers of webapp webroot do not fit: max - reserve less \
             ({request_cap} + {service_cap}) * 2 leaves {left}, {agent_workers} of them for an agent worker; streamed \
             database downloads read {fits} at a time (raise pool-maxsize to fit them)"
        );
    }
    Ok(())
}

/// One agent worker in a binary that links the agent runtime, else none.
fn agent_workers() -> u32 {
    u32::from(crate::llm_hooks::linked())
}

/// The `webroot` webapp's `transfer-workers`: how many reads of database sources streamed responses may run at once,
/// each holding a connection of the `content` group's pool for one window. 4 when missing or not a positive number.
pub fn transfer_workers(factory: &ExecutionContextFactory) -> u32 {
    factory
        .webapp_setting("webroot", "transfer-workers")
        .parse::<u32>()
        .ok()
        .filter(|workers| *workers > 0)
        .unwrap_or(DEFAULT_TRANSFER_WORKERS)
}

/// The streamed database reads that may run at once: [`transfer_workers`], or as many as the `content` group's pool
/// leaves beside the request and service workers and an agent worker when that is fewer, at least one. Reserve
/// connections are never counted. All of them with no entity facade.
pub fn transfer_readers(factory: &ExecutionContextFactory) -> u32 {
    let asked = transfer_workers(factory);
    let Some(facade) = factory.entity() else {
        return asked;
    };
    let Ok((_, manager)) = content_manager(&facade) else {
        return asked;
    };
    let (request_cap, service_cap) = caps(&factory.conf_text());
    let left = manager
        .spare_connections(request_cap, service_cap)
        .saturating_sub(agent_workers());
    asked.min(left.max(1))
}

/// The manager the `content` group reads from, with the name of a group it serves: the group's own, or the default
/// group's when the content shares the transactional database.
fn content_manager(
    facade: &crate::entity::EntityFacade,
) -> Result<(String, crate::transaction::TransactionManager), Error> {
    let manager = match crate::service::content::content_transactions(facade) {
        Some(manager) => manager,
        None => default_manager(facade)?,
    };
    let group = facade
        .groups()
        .find(|(_, _, transactions)| transactions.shares_stack(&manager))
        .map_or_else(String::new, |(name, _, _)| name.to_string());
    Ok((group, manager))
}

/// The default group's manager and the managers linked to it, each with the name of a group it serves.
fn linked_managers(
    facade: &crate::entity::EntityFacade,
) -> Result<Vec<(String, crate::transaction::TransactionManager)>, Error> {
    let default = default_manager(facade)?;
    let mut out = vec![(facade.default_group().to_string(), default.clone())];
    for manager in default.linked() {
        let group = facade
            .groups()
            .find(|(_, _, transactions)| transactions.shares_stack(&manager))
            .map_or_else(String::new, |(name, _, _)| name.to_string());
        out.push((group, manager));
    }
    Ok(out)
}

fn default_manager(facade: &crate::entity::EntityFacade) -> Result<crate::transaction::TransactionManager, Error> {
    let group = facade.default_group();
    facade
        .groups()
        .find(|(name, _, _)| *name == group)
        .map(|(_, _, transactions)| transactions.clone())
        .ok_or_else(|| Error::new(format!("no transaction manager for entity group {group}")))
}

/// The connections of the default group's pool that the request and service workers leave (`max - reserve` less
/// `(request cap + service cap) * 2`), and of the `content` group's pool the streamed downloads' reads leave too
/// ([`transfer_readers`]): the fewest of it and the pools linked to it; `None` with no entity facade.
pub fn spare_connections(factory: &ExecutionContextFactory) -> Option<u32> {
    let facade = factory.entity()?;
    let (request_cap, service_cap) = caps(&factory.conf_text());
    let content = content_manager(&facade).ok().map(|(_, manager)| manager);
    let transfers = transfer_readers(factory);
    linked_managers(&facade).ok().and_then(|managers| {
        managers
            .iter()
            .map(|(_, manager)| {
                let spare = manager.spare_connections(request_cap, service_cap);
                if content.as_ref().is_some_and(|content| content.shares_stack(manager)) {
                    spare.saturating_sub(transfers)
                } else {
                    spare
                }
            })
            .min()
    })
}

fn pack<T, F, D>(factory: Arc<ExecutionContextFactory>, job: F, done: D) -> Work
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Error> + Send + 'static,
    D: FnOnce(Result<T, Error>) + Send + 'static,
{
    Work {
        factory,
        run: Box::new(move || job().map(|value| Box::new(value) as JobOutput)),
        finish: Box::new(move |result| done(map_job_result(result))),
    }
}

fn map_job_result<T: 'static>(result: Result<JobOutput, Error>) -> Result<T, Error> {
    match result {
        Ok(value) => value
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| Error::new("request worker result type mismatch")),
        Err(err) => Err(err),
    }
}

fn request_sizes(conf_text: &str) -> Result<(usize, usize), Error> {
    let Some(tools) = tools_of(conf_text)? else {
        return Ok((widen(DEFAULT_REQUEST_WORKERS), widen(DEFAULT_REQUEST_QUEUE)));
    };
    Ok((
        usize_attr(&tools, "request-pool-max", widen(DEFAULT_REQUEST_WORKERS)),
        usize_attr(&tools, "request-queue", widen(DEFAULT_REQUEST_QUEUE)),
    ))
}

fn tools_of(conf_text: &str) -> Result<Option<MNode>, Error> {
    let root = MNode::parse_shared("moqui-conf", conf_text)?;
    if root.has_name("tools") {
        return Ok(Some(root));
    }
    Ok(root.first_named("tools"))
}

fn usize_attr(node: &MNode, name: &str, default: usize) -> usize {
    node.attribute(name)
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(default)
}

fn fit_u32(value: usize, default: u32) -> u32 {
    u32::try_from(value).unwrap_or(default)
}

fn widen(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn make_shared(workers: usize, queue: usize) -> Result<Arc<Shared>, Error> {
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        workers,
        queue_cap: queue,
        handles: Mutex::new(Vec::new()),
    });
    let handles = spawn_all(&shared, workers)?;
    *lock(&shared.handles) = handles;
    Ok(shared)
}

fn spawn_all(shared: &Arc<Shared>, workers: usize) -> Result<Vec<JoinHandle<()>>, Error> {
    let mut handles = Vec::with_capacity(workers);
    for index in 0..workers {
        match spawn_worker(shared, index) {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                stop_started(shared, handles);
                return Err(err);
            }
        }
    }
    Ok(handles)
}

fn spawn_worker(shared: &Arc<Shared>, index: usize) -> Result<JoinHandle<()>, Error> {
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name(format!("moqui-request-{index}"))
        .stack_size(8 * 1024 * 1024)
        .spawn(move || worker_loop(shared))
        .map_err(|err| Error::new(format!("request worker did not start: {err}")))
}

fn worker_loop(shared: Arc<Shared>) {
    crate::script::mark_request_thread();
    let waiter = Arc::new(Condvar::new());
    while let Some(work) = next_work(&shared, &waiter) {
        run_work(work);
    }
}

/// The next job, waiting on `waiter`, this worker's own, while there is none.
fn next_work(shared: &Shared, waiter: &Arc<Condvar>) -> Option<Work> {
    let mut state = lock(&shared.state);
    loop {
        if state.stop {
            return None;
        }
        if let Some(work) = state.queue.pop_front() {
            return Some(work);
        }
        state.idle.enter(waiter);
        state = wait(waiter, state);
        state.idle.leave(waiter);
    }
}

fn run_work(work: Work) {
    let Work { factory, run, finish } = work;
    // The cleanup ends before `finish`, so the waiter hears only after it.
    let result = {
        let _cleanup = WorkCleanup::new(Some(Arc::clone(&factory)));
        let outcome = catch_unwind(AssertUnwindSafe(move || {
            let ctx = factory.new_context();
            let _entered = ctx.enter();
            run()
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => Err(Error::new("request worker panicked")),
        }
    };
    finish(result);
}

fn shutdown(shared: &Shared) {
    let pending = {
        let mut state = lock(&shared.state);
        state.stop = true;
        state.idle.wake_all();
        std::mem::take(&mut state.queue)
    };
    for work in pending {
        (work.finish)(Err(Error::new("request pool is not running")));
    }
    let handles = {
        let mut guard = lock(&shared.handles);
        std::mem::take(&mut *guard)
    };
    for handle in handles {
        join_one(handle);
    }
}

fn stop_started(shared: &Shared, handles: Vec<JoinHandle<()>>) {
    {
        let mut state = lock(&shared.state);
        state.stop = true;
        state.idle.wake_all();
    }
    for handle in handles {
        join_one(handle);
    }
}

fn join_one(handle: JoinHandle<()>) {
    if handle.thread().id() == thread::current().id() {
        std::mem::forget(handle);
        return;
    }
    let _ = handle.join();
}

#[cfg(test)]
#[path = "request_pool_tests.rs"]
mod tests;
