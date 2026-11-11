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

//! Per-factory service workers. Drop stops and joins only this guard's threads.
//!
//! [`ServicePools::inline`] is the executor with no threads (`plans/archive/Wasm.md`). Async calls and
//! `run_job_now` queue on the guard and run on the calling thread at a drain point:
//! [`ExecutionContextFactory::run_pending`], or when the outermost execution context on the thread
//! exits with no transaction in place. `callFuture()` runs at once, in a fresh context, under a new
//! transaction. That new transaction suspends the caller's, so it needs a second connection: on a
//! single-writer database (SQLite) a callee that writes while the caller holds a write
//! transaction fails instead of waiting. The queue has no cap, so nothing spills to a one-shot job.
//! There is no scheduler.

use std::any::Any;
use std::collections::{BTreeMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::{lock, wait, wait_timeout};

use super::call::AsyncCall;
use crate::context::{Error, ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::transaction::TransactionManager;

const DEFAULT_WORKERS: usize = 22;
/// Java's `makeWorkerPool` default; `MoquiDefaultConf.xml` sets 65535.
const DEFAULT_QUEUE: usize = 65_536;
const DEFAULT_CHECK_SECONDS: u64 = 60;
/// Java's `workerPool.awaitTermination(30, TimeUnit.SECONDS)` at shutdown.
const DRAIN_WAIT: Duration = Duration::from_secs(30);
/// Java's `job-pool-core` and `job-pool-max` defaults.
const DEFAULT_JOB_POOL_CORE: usize = 2;
const DEFAULT_JOB_POOL_MAX: usize = 8;
/// How long a stop waits for job runs in progress before it leaves their threads (HL-026).
const JOB_JOIN_WAIT: Duration = Duration::from_secs(30);
/// The job worker threads' name prefix.
const JOB_THREAD: &str = "moqui-job-";

type TickHook = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    static IN_WORKER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn in_worker() -> bool {
    IN_WORKER.with(std::cell::Cell::get)
}

struct WorkerFlag;

impl WorkerFlag {
    fn enter() -> Self {
        IN_WORKER.with(|flag| flag.set(true));
        Self
    }
}

impl Drop for WorkerFlag {
    fn drop(&mut self) {
        IN_WORKER.with(|flag| flag.set(false));
    }
}

/// Ends what one unit of work left on a pool thread when it drops: the script context stack, any transaction frame
/// still open on the factory's managers, which is rolled back, and the entity layer's thread state. Request and
/// service workers, reserved threads and the scheduler pass hold one for each item, as Java runs `destroyAllInThread`
/// in `finally`. It goes outside the item's `catch_unwind`, so it never runs during an unwind.
pub struct WorkCleanup {
    factory: Option<Arc<ExecutionContextFactory>>,
    /// False for an inline item run under the caller's context, whose script stack and sim session stay.
    outermost: bool,
}

impl WorkCleanup {
    pub fn new(factory: Option<Arc<ExecutionContextFactory>>) -> Self {
        Self {
            factory,
            outermost: true,
        }
    }
}

impl Drop for WorkCleanup {
    fn drop(&mut self) {
        let outermost = self.outermost;
        let factory = self.factory.take();
        cleanup_step("work cleanup", || {
            if outermost {
                crate::script::clear_context_stack();
                crate::web::take_auth_failure();
            }
            let Some(facade) = factory.as_ref().and_then(|factory| factory.entity()) else {
                return;
            };
            rollback_frames(&facade);
            crate::entity::clear_thread_state(&facade, outermost);
        });
    }
}

/// Run a cleanup guard's work. During an unwind a second panic would abort the process, so then a panic in `work` is
/// caught and logged; otherwise `work` runs as it is.
pub(crate) fn cleanup_step(what: &str, work: impl FnOnce()) {
    if !thread::panicking() {
        work();
        return;
    }
    if let Err(payload) = catch_unwind(AssertUnwindSafe(work)) {
        tracing::error!(cleanup = what, panic = %panic_text(payload.as_ref()), "cleanup panicked during an unwind");
    }
}

/// Roll back every transaction frame left on this thread for the facade's managers, the sim overlay's included.
///
/// A frame still open after a unit of work was never ended by the code that began it, so nothing decided to keep
/// its writes; Java's `destroyAllInThread` commits it, and this rolls it back.
pub(crate) fn rollback_frames(facade: &EntityFacade) {
    for (_, _, transactions) in facade.groups() {
        rollback_manager(transactions);
    }
    if let Some(sim) = facade.sim_transactions() {
        rollback_manager(&sim);
    }
}

fn rollback_manager(manager: &TransactionManager) {
    if let Err(err) = manager.rollback_all_in_thread() {
        tracing::warn!(error = %err, "transaction cleanup after a unit of work failed");
    }
}

/// The message of a caught panic, for the log.
pub fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

struct ReservedSlot(Arc<Shared>);

impl ReservedSlot {
    fn take(shared: &Arc<Shared>, limit: usize) -> Result<Self, Error> {
        let before = shared.reserved.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if before >= limit {
            shared.reserved.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            return Err(Error::new("no reserved service worker is free for a nested call"));
        }
        Ok(Self(Arc::clone(shared)))
    }
}

impl Drop for ReservedSlot {
    fn drop(&mut self) {
        self.0.reserved.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Factory-owned pool guard. [`ServicePools::idle`] starts no threads.
///
/// Drop sets the stop flag, wakes this guard's workers, and joins them. Another factory's guard
/// is a different value and keeps running.
#[must_use = "dropping the guard stops and joins this pool's threads"]
pub struct ServicePools {
    pub(crate) shared: Option<Arc<Shared>>,
    inline: Option<Inline>,
}

/// Work waiting for a drain point on the inline executor.
#[derive(Default)]
struct Inline {
    queue: Mutex<VecDeque<Work>>,
    draining: std::sync::atomic::AtomicBool,
}

pub(crate) struct Shared {
    pub(crate) state: Mutex<State>,
    pub(crate) wake: Condvar,
    pub(crate) tick_hook: Mutex<Vec<TickHook>>,
    pub(crate) workers: usize,
    pub(crate) queue_cap: usize,
    /// Jobs allowed to wait beyond the workers, from `service-facade/@job-queue-max`.
    pub(crate) job_queue_max: usize,
    /// Threads that run scheduled jobs, apart from the service workers: `service-facade/@job-pool-core`.
    pub(crate) job_workers: usize,
    /// `service-facade/@job-pool-max` (at least twice the processors, as Java): with `job_queue_max`, how many jobs
    /// may be running or waiting.
    pub(crate) job_pool_max: usize,
    /// Wakes the job workers.
    pub(crate) job_wake: Condvar,
    /// How often the scheduler thread runs a pass, from `service-facade/@scheduled-job-check-time`; the default when
    /// that is 0, for the tick hooks.
    pub(crate) check_interval: Duration,
    /// Whether the scheduler runs job passes: Java starts its `ScheduledJobRunner` only when
    /// `scheduled-job-check-time` is more than 0.
    pub(crate) job_runner: bool,
    /// The factory the pool serves, set once it is built. Weak, so the pool never keeps it alive.
    pub(crate) factory: Mutex<Weak<ExecutionContextFactory>>,
    /// Threads running a nested `call_future` for a worker. Bounded by the worker count.
    pub(crate) reserved: std::sync::atomic::AtomicUsize,
    /// What the scheduler passes did, for the tools job list.
    pub(crate) runner: Mutex<RunnerStats>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

/// The `ScheduledJobRunner` counters the tools service job list shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunnerStats {
    /// When the last pass ran, in epoch milliseconds. 0 before the first.
    pub last_execute_millis: i64,
    pub execute_count: u64,
    pub total_jobs_run: u64,
    pub last_jobs_active: usize,
    pub last_jobs_paused: usize,
}

/// The worker pool as the tools dashboard shows a thread pool executor: threads and queue.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStatus {
    pub workers: usize,
    pub active: usize,
    pub queued: usize,
    pub queue_cap: usize,
}

impl Shared {
    pub(crate) fn note_pass(&self, report: &super::jobs::PassReport) {
        let mut stats = lock(&self.runner);
        stats.last_execute_millis = jiff::Timestamp::now().as_millisecond();
        stats.execute_count += 1;
        stats.total_jobs_run += u64::try_from(report.jobs_run).unwrap_or(0);
        stats.last_jobs_active = report.jobs_active;
        stats.last_jobs_paused = report.jobs_paused;
    }
}

#[derive(Default)]
pub(crate) struct State {
    /// Shutdown began: no new work is taken, and the scheduler and job workers end. Service workers run what is queued.
    pub(crate) stop: bool,
    /// Service workers end even with work queued: the queue drained, or the wait for it ran out.
    pub(crate) halt: bool,
    pub(crate) queue: VecDeque<Work>,
    /// Workers running something now.
    pub(crate) active: usize,
    /// Workers waiting for work that may still hold a script runtime (they have not let it go in this wait).
    pub(crate) warm_idle: usize,
    /// The workers waiting for work, woken one at a time, the one idle most recently first (plans/Profiling3.md W13).
    pub(crate) idle: super::idle::Idle,
    /// Scheduled job runs waiting for a job worker, as Java's separate `jobWorkerPool`.
    pub(crate) jobs: VecDeque<Box<super::jobs::JobRun>>,
    /// Job workers running a job now.
    pub(crate) jobs_active: usize,
}

pub(crate) enum Work {
    Call(CallJob),
    Job(Box<super::jobs::JobRun>),
    Closure(ClosureJob),
    Rules(RulesJob),
    #[cfg(test)]
    Task(Box<dyn FnOnce() + Send>),
}

pub(crate) struct ClosureJob {
    run: ClosureFn,
    reply: Sender<ClosureOut>,
    trace_context: Option<crate::telemetry::TraceContext>,
    /// What a fire-and-forget [`ServicePools::spawn`] runs, named in the log when it is dropped at shutdown.
    what: Option<&'static str>,
}

/// The tx-commit or tx-rollback rules of a service call, run after its transaction ends: the service, the phase
/// (`when`), the call's parameters and results, and the authorization the call had when it registered them, which the
/// rules run under.
#[derive(Clone, Debug)]
pub(crate) struct TxRules {
    pub(crate) service: String,
    pub(crate) when: String,
    pub(crate) parameters: BTreeMap<String, Value>,
    pub(crate) results: Option<BTreeMap<String, Value>>,
    pub(crate) authz: crate::user::RegisteredAuthz,
}

/// [`TxRules`] waiting for a service worker, with the caller they run as.
pub(crate) struct RulesJob {
    rules: TxRules,
    factory: Arc<ExecutionContextFactory>,
    user_name: Option<String>,
    origin: Option<crate::context::AgentOrigin>,
    trace_context: Option<crate::telemetry::TraceContext>,
}

type ClosureOut = Result<Box<dyn Any + Send>, Error>;
type ClosureFn = Box<dyn FnOnce() -> ClosureOut + Send>;

pub(crate) struct CallJob {
    name: String,
    parameters: BTreeMap<String, Value>,
    factory: Arc<ExecutionContextFactory>,
    /// The user who made the call; the worker acts as them.
    user_name: Option<String>,
    /// The agent run the call was made in, which the worker works for too (the agent triggers' loop guard).
    origin: Option<crate::context::AgentOrigin>,
    trace_context: Option<crate::telemetry::TraceContext>,
    reply: Option<Sender<Result<BTreeMap<String, Value>, Error>>>,
}

/// The agent run the current context works for, carried with async work so its writes keep their origin.
pub(crate) fn current_origin() -> Option<crate::context::AgentOrigin> {
    ExecutionContext::with_current(|ec| ec.agent_origin()).flatten()
}

/// A fire-and-forget call already checked as its caller (`call::prepare_async`), with the caller it runs as.
pub(crate) struct CheckedCall {
    pub(crate) name: String,
    parameters: BTreeMap<String, Value>,
    user_name: Option<String>,
    origin: Option<crate::context::AgentOrigin>,
}

impl CheckedCall {
    /// A call checked just now, by the current user and agent run.
    pub(crate) fn new(name: String, parameters: BTreeMap<String, Value>) -> Self {
        Self {
            name,
            parameters,
            user_name: crate::user::current_username().ok().flatten(),
            origin: current_origin(),
        }
    }
}

impl Work {
    fn service(call: CheckedCall, factory: Arc<ExecutionContextFactory>) -> Self {
        Work::Call(CallJob {
            name: call.name,
            parameters: call.parameters,
            factory,
            user_name: call.user_name,
            origin: call.origin,
            trace_context: crate::telemetry::capture(),
            reply: None,
        })
    }

    fn replied(
        name: String,
        parameters: BTreeMap<String, Value>,
        factory: Arc<ExecutionContextFactory>,
        reply: Sender<Result<BTreeMap<String, Value>, Error>>,
    ) -> Self {
        Work::Call(CallJob {
            name,
            parameters,
            factory,
            user_name: crate::user::current_username().ok().flatten(),
            origin: current_origin(),
            trace_context: crate::telemetry::capture(),
            reply: Some(reply),
        })
    }
}

impl ServicePools {
    /// No workers. `test_context` stores this.
    pub fn idle() -> Self {
        Self {
            shared: None,
            inline: None,
        }
    }

    /// No threads. Async work waits for a drain point on the calling thread. See the module docs.
    pub fn inline() -> Self {
        Self {
            shared: None,
            inline: Some(Inline::default()),
        }
    }

    /// True for [`Self::inline`].
    pub fn is_inline(&self) -> bool {
        self.inline.is_some()
    }

    /// True when the pool has threads and a scheduler, so its tick hooks run.
    pub(crate) fn is_threaded(&self) -> bool {
        self.shared.is_some()
    }

    /// Queued inline work not yet run.
    pub fn pending(&self) -> usize {
        self.inline.as_ref().map_or(0, |inline| lock(&inline.queue).len())
    }

    /// Run queued inline work on this thread, oldest first, until the queue is empty. Work queued
    /// while draining runs in the same pass. Returns how many ran.
    ///
    /// Runs nothing while a transaction is in place on this thread, or when this thread is already
    /// draining. A failure is logged, as on a worker, and does not stop the rest.
    pub fn run_pending(&self) -> usize {
        let Some(inline) = &self.inline else {
            return 0;
        };
        if crate::transaction::thread_in_transaction() {
            return 0;
        }
        if inline.draining.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return 0;
        }
        let _done = DrainFlag(&inline.draining);
        let mut ran = 0;
        loop {
            let Some(work) = lock(&inline.queue).pop_front() else {
                break;
            };
            run_inline(work);
            ran += 1;
        }
        ran
    }

    /// Called from the scheduler after the state lock is released. Idle pools ignore it.
    pub fn add_tick_hook(&self, hook: impl Fn() + Send + Sync + 'static) {
        let Some(shared) = &self.shared else {
            return;
        };
        lock(&shared.tick_hook).push(Arc::new(hook));
    }

    /// Scheduler pass interval. Idle pools report [`DEFAULT_CHECK_SECONDS`].
    pub(crate) fn check_interval(&self) -> Duration {
        self.shared
            .as_ref()
            .map(|shared| shared.check_interval)
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_CHECK_SECONDS))
    }

    /// Test pool. `workers` is at least 1. Same Drop as a booted pool.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn bounded(workers: usize, queue: usize) -> Self {
        start(Settings::test(workers.max(1), queue)).expect("service worker pool")
    }

    /// Threads busy and work waiting now. Idle is all zero.
    pub fn status(&self) -> PoolStatus {
        let Some(shared) = self.shared.as_ref() else {
            return PoolStatus::default();
        };
        let state = lock(&shared.state);
        PoolStatus {
            workers: shared.workers,
            active: state.active,
            queued: state.queue.len(),
            queue_cap: shared.queue_cap,
        }
    }

    /// The scheduler pass counters, or `None` when the pool is idle or runs no job passes.
    pub fn runner_stats(&self) -> Option<RunnerStats> {
        self.shared
            .as_ref()
            .filter(|shared| shared.job_runner)
            .map(|shared| lock(&shared.runner).clone())
    }

    /// Worker count and queue capacity. Idle is `(0, 0)`.
    pub fn sizes(&self) -> (usize, usize) {
        self.shared
            .as_ref()
            .map(|shared| (shared.workers, shared.queue_cap))
            .unwrap_or((0, 0))
    }

    /// Submit and wait. A full queue returns immediately. An in-worker caller uses a fresh thread.
    pub fn call_future(
        &self,
        name: &str,
        parameters: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, Error> {
        if self.inline.is_some() {
            return call_now(name, parameters);
        }
        self.running()?.call_future(name, parameters)
    }

    /// Run `job` on a service worker and wait for it.
    ///
    /// An idle pool runs `job` on this thread and does not enter a context. A caller that is
    /// already a worker uses a reserved thread, the same rule as [`Self::call_future`]. A worker
    /// enters a fresh execution context before `job`.
    ///
    /// # Errors
    ///
    /// Returns an error when the pool is stopped, the queue is full, no reserved thread is free,
    /// the worker panics, or `job` returns an error.
    pub fn call<T, F>(&self, job: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, Error> + Send + 'static,
    {
        let Some(shared) = &self.shared else {
            if self.inline.is_some() {
                if let Ok(factory) = capture_factory() {
                    let ctx = factory.new_context();
                    let _entered = ctx.enter();
                    return job();
                }
            }
            return job();
        };
        let shared = Arc::clone(shared);
        let run: ClosureFn = Box::new(move || job().map(|value| Box::new(value) as Box<dyn Any + Send>));
        let rx = if in_worker() {
            reserve_closure(&shared, run)?
        } else {
            queue_closure(&shared, run, None)?
        };
        let boxed = rx
            .recv()
            .unwrap_or_else(|_| Err(Error::new("service worker pool is not running")))?;
        boxed
            .downcast::<T>()
            .map(|value| *value)
            .map_err(|_| Error::new("service worker returned the wrong type"))
    }

    /// Run `job` on a service worker in an execution context of its own, without waiting for it, as Java's
    /// `ThreadPoolRunnable`. A failure is logged with `what`. With no worker threads it runs now.
    pub fn spawn(&self, what: &'static str, job: impl FnOnce() -> Result<(), Error> + Send + 'static) {
        let Some(shared) = &self.shared else {
            let result = match capture_factory() {
                Ok(factory) => {
                    let ctx = factory.new_context();
                    let _entered = ctx.enter();
                    job()
                }
                Err(_) => job(),
            };
            if let Err(err) = result {
                tracing::error!(error = %err, "{what} failed");
            }
            return;
        };
        let run: ClosureFn = Box::new(move || {
            if let Err(err) = job() {
                tracing::error!(error = %err, "{what} failed");
            }
            Ok(Box::new(()) as Box<dyn Any + Send>)
        });
        if let Err(err) = queue_closure(shared, run, Some(what)) {
            tracing::error!(error = %err, "{what} was not run");
        }
    }

    /// Run the tx rules of a service call on a service worker, as the current user and agent run, without waiting.
    /// When the queue is full or the pool is stopping they are stored as a queued job, which the scheduler runs later
    /// (here or on another member); a failure to store them is logged with the service and phase. With no worker
    /// threads they run now.
    pub(crate) fn spawn_rules(&self, factory: &Arc<ExecutionContextFactory>, rules: TxRules) {
        let job = RulesJob {
            rules,
            factory: Arc::clone(factory),
            user_name: crate::user::current_username().ok().flatten(),
            origin: current_origin(),
            trace_context: crate::telemetry::capture(),
        };
        let Some(shared) = &self.shared else {
            run_rules(job, false);
            return;
        };
        let job = {
            let mut state = lock(&shared.state);
            if !state.stop && state.queue.len() < shared.queue_cap {
                state.queue.push_back(Work::Rules(job));
                state.idle.wake_one();
                return;
            }
            job
        };
        spill_rules(job, "the service worker queue is full or stopping");
    }

    /// Start the call and return its pending result. See [`Shared::submit_future`].
    pub(crate) fn submit_future(
        &self,
        name: &str,
        parameters: &BTreeMap<String, Value>,
    ) -> Result<CallReceiver, Error> {
        if self.inline.is_some() {
            return Ok(settled(call_now(name, parameters)));
        }
        self.running()?.submit_future(name, parameters)
    }

    /// Tell the pool which factory it serves. Boot calls this once the factory is built.
    pub fn attach(&self, factory: &Arc<ExecutionContextFactory>) {
        if let Some(shared) = &self.shared {
            *lock(&shared.factory) = Arc::downgrade(factory);
            crate::startup::integration(
                factory,
                "Scheduled jobs",
                if shared.job_runner {
                    crate::startup::State::on(format!(
                        "every {} seconds, {} job workers",
                        shared.check_interval.as_secs(),
                        shared.job_workers
                    ))
                } else {
                    crate::startup::State::off("scheduled-job-check-time is 0")
                },
            );
        }
    }

    /// Fire-and-forget. A full or stopping queue stores the call as a one-shot service job, which the scheduler
    /// pass submits when a job worker is free. With no database to store it in, that is an error.
    ///
    /// The call is checked first, as Java's `validateCall` (see `call::prepare_async`): parameters that fail
    /// validation leave their errors in the message facade and queue nothing, and nothing is queued in a sim session.
    pub fn submit(&self, name: &str, parameters: &BTreeMap<String, Value>) -> Result<(), Error> {
        if self.inline.is_none() {
            self.running()?;
        }
        let factory = capture_factory()?;
        let AsyncCall::Run(parameters) = super::call::prepare_async(name, parameters)? else {
            return Ok(());
        };
        self.submit_checked(&factory, CheckedCall::new(name.to_string(), parameters))
    }

    /// Queue a call [`Self::submit`] checked, now or later (a call registered for the end of a transaction), with no
    /// second check. A full or stopping queue stores it as a one-shot service job, as `submit` does.
    pub(crate) fn submit_checked(
        &self,
        factory: &Arc<ExecutionContextFactory>,
        call: CheckedCall,
    ) -> Result<(), Error> {
        super::call::note_pending(&call.name);
        if let Some(inline) = &self.inline {
            lock(&inline.queue).push_back(Work::service(call, Arc::clone(factory)));
            return Ok(());
        }
        let shared = self.running()?;
        let call = {
            let mut state = lock(&shared.state);
            // A stopping pool stores the call too: the database is still up while the queue drains (HL-025).
            if !state.stop && state.queue.len() < shared.queue_cap {
                state.queue.push_back(Work::service(call, Arc::clone(factory)));
                state.idle.wake_one();
                return Ok(());
            }
            call
        };
        // stored with its caller, as a queued call carries them
        let name = call.name;
        super::jobs::persist_queued(
            factory,
            &super::jobs::QueuedCall {
                service: name.clone(),
                parameters: call.parameters,
                user_name: call.user_name,
                origin: call.origin,
                rules: None,
            },
        )
        .map_err(|err| Error::new(format!("async call of {name} was not queued: {err}")))
    }

    /// One scheduler pass now, as the scheduler thread runs it on its interval. With no job runner only the hooks run.
    pub fn tick(&self) -> super::jobs::PassReport {
        match &self.shared {
            Some(shared) => {
                let report = if shared.job_runner {
                    super::jobs::run_scheduled(shared)
                } else {
                    super::jobs::PassReport::default()
                };
                run_tick_hook(shared);
                report
            }
            None => super::jobs::PassReport::default(),
        }
    }

    /// Queue a job run on the inline executor. Returns it when this guard is not inline.
    pub(crate) fn submit_inline_job(&self, run: Box<super::jobs::JobRun>) -> Result<(), Box<super::jobs::JobRun>> {
        match &self.inline {
            Some(inline) => {
                lock(&inline.queue).push_back(Work::Job(run));
                Ok(())
            }
            None => Err(run),
        }
    }

    #[cfg(test)]
    pub(crate) fn submit_task(&self, task: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        let shared = self.running()?;
        {
            let mut state = lock(&shared.state);
            if state.stop {
                return Err(Error::new("service worker pool is not running"));
            }
            if state.queue.len() >= shared.queue_cap {
                return Err(Error::new("service worker queue is full"));
            }
            state.queue.push_back(Work::Task(Box::new(task)));
            state.idle.wake_one();
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn shared_handle(&self) -> Arc<Shared> {
        Arc::clone(self.shared.as_ref().expect("service worker pool"))
    }

    /// Stop threaded workers and the scheduler before shutting down their tools. As Java's `workerPool.shutdown()` and
    /// `awaitTermination(30s)`, the async calls already queued still run, for up to 30 seconds; what is left then
    /// fails. Queued job runs are abandoned at once, as Java never shuts down its `jobWorkerPool`.
    pub(crate) fn shutdown(&self) {
        self.shutdown_within(DRAIN_WAIT);
    }

    /// Stop with no wait for the queue, as a shutdown whose drain time has run out.
    #[cfg(test)]
    pub(crate) fn shutdown_now(&self) {
        self.shutdown_within(Duration::ZERO);
    }

    fn shutdown_within(&self, limit: Duration) {
        let Some(shared) = &self.shared else {
            return;
        };
        // On one of this pool's own workers, no other may be left to run the queue.
        let own = lock(&shared.handles)
            .iter()
            .any(|handle| handle.thread().id() == thread::current().id());
        drain(shared, if own { Duration::ZERO } else { limit });
        fail_pending(shared);
        let handles = std::mem::take(&mut *lock(&shared.handles));
        join_all(handles);
    }

    fn running(&self) -> Result<&Arc<Shared>, Error> {
        self.shared
            .as_ref()
            .ok_or_else(|| Error::new("service worker pool is not running"))
    }
}

impl Drop for ServicePools {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Run each scheduler hook after a pass. Hooks are not called under the state lock.
/// One panic does not skip the rest.
pub(crate) fn run_tick_hook(shared: &Shared) {
    let hooks = lock(&shared.tick_hook).clone();
    for hook in hooks {
        if catch_unwind(AssertUnwindSafe(|| hook())).is_err() {
            tracing::warn!("scheduler tick hook panicked");
        }
    }
}

impl Shared {
    pub(crate) fn call_future(
        self: &Arc<Self>,
        name: &str,
        parameters: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, Error> {
        receive(&self.submit_future(name, parameters)?)
    }

    /// Start the call and return the channel its result arrives on. A full queue is an error at
    /// once. A caller that is itself a worker runs the call on a reserved thread.
    pub(crate) fn submit_future(
        self: &Arc<Self>,
        name: &str,
        parameters: &BTreeMap<String, Value>,
    ) -> Result<CallReceiver, Error> {
        let factory = capture_factory()?;
        let parameters = match super::call::prepare_async(name, parameters)? {
            AsyncCall::Run(parameters) => parameters,
            AsyncCall::Skip(result) => return Ok(settled(Ok(result))),
        };
        let parameters = &parameters;
        if in_worker() {
            return reserved(self, factory, name, parameters);
        }
        let (tx, rx) = mpsc::channel();
        {
            let mut state = lock(&self.state);
            if state.stop {
                return Err(Error::new("service worker pool is not running"));
            }
            if state.queue.len() >= self.queue_cap {
                return Err(Error::new("service worker queue is full"));
            }
            state
                .queue
                .push_back(Work::replied(name.to_string(), parameters.clone(), factory, tx));
            state.idle.wake_one();
        }
        Ok(rx)
    }
}

/// The receiving end of a started service call.
pub(crate) type CallReceiver = mpsc::Receiver<Result<BTreeMap<String, Value>, Error>>;

/// A receiver that already holds `outcome`.
fn settled(outcome: Result<BTreeMap<String, Value>, Error>) -> CallReceiver {
    let (tx, rx) = mpsc::channel();
    let _ = tx.send(outcome);
    rx
}

pub(crate) fn receive(rx: &CallReceiver) -> Result<BTreeMap<String, Value>, Error> {
    rx.recv()
        .unwrap_or_else(|_| Err(Error::new("service worker pool is not running")))
}

/// Read `tools.@worker-pool-max` and `tools.@worker-queue`, and from `service-facade` the
/// `job-queue-max` and `scheduled-job-check-time`. Missing or non-integer values are the defaults.
///
/// # Errors
///
/// Returns an error when the conf does not parse, or the pool would have no workers.
pub fn start_pools(conf_text: &str) -> Result<ServicePools, Error> {
    start(settings(conf_text)?)
}

struct Settings {
    workers: usize,
    queue: usize,
    job_queue_max: usize,
    job_pool_core: usize,
    job_pool_max: usize,
    check_interval: Duration,
    job_runner: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl Settings {
    fn test(workers: usize, queue: usize) -> Self {
        Self {
            workers,
            queue,
            job_queue_max: 0,
            job_pool_core: DEFAULT_JOB_POOL_CORE,
            job_pool_max: DEFAULT_JOB_POOL_MAX,
            check_interval: Duration::from_secs(DEFAULT_CHECK_SECONDS),
            job_runner: true,
        }
    }
}

pub(crate) fn capture_factory() -> Result<Arc<ExecutionContextFactory>, Error> {
    ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory()))
        .ok_or_else(|| Error::new("no execution context is entered"))
}

fn settings(conf_text: &str) -> Result<Settings, Error> {
    let root = MNode::parse_shared("moqui-conf", conf_text)?;
    let tools = root.first_named("tools");
    let facade = root.first_named("service-facade");
    let tool = |name: &str, default: usize| tools.as_ref().map_or(default, |node| usize_attr(node, name, default));
    let service = |name: &str, default: usize| facade.as_ref().map_or(default, |node| usize_attr(node, name, default));
    let seconds = service(
        "scheduled-job-check-time",
        usize::try_from(DEFAULT_CHECK_SECONDS).unwrap_or(60),
    );
    // As Java, 0 starts no job runner. The tick hooks still run, at the default interval.
    let job_runner = seconds > 0;
    if !job_runner {
        tracing::warn!("Not starting Scheduled Service Job Runner (config:{seconds})");
    }
    let seconds = if job_runner {
        u64::try_from(seconds).unwrap_or(DEFAULT_CHECK_SECONDS)
    } else {
        DEFAULT_CHECK_SECONDS
    };
    Ok(Settings {
        workers: tool("worker-pool-max", DEFAULT_WORKERS),
        queue: tool("worker-queue", DEFAULT_QUEUE),
        job_queue_max: service("job-queue-max", 0),
        job_pool_core: service("job-pool-core", DEFAULT_JOB_POOL_CORE).max(1),
        job_pool_max: service("job-pool-max", DEFAULT_JOB_POOL_MAX)
            .max(thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get) * 2),
        check_interval: Duration::from_secs(seconds),
        job_runner,
    })
}

/// An attribute as a number, `${}` expanded; `default` when it is missing, empty or not a number.
fn usize_attr(node: &MNode, name: &str, default: usize) -> usize {
    node.attribute(name)
        .map(|raw| moqui_lib::env::expand(&raw).unwrap_or(raw))
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(default)
}

fn start(settings: Settings) -> Result<ServicePools, Error> {
    if settings.workers == 0 {
        return Err(Error::new("the service worker pool needs at least one worker"));
    }
    // The job workers come out of `worker-pool-max`, so the threads that hold database connections stay within the
    // count the transaction pool is sized for (see `check_serve_caps`). A pool too small to spare them keeps all its
    // workers and adds the job workers.
    let mut settings = settings;
    if settings.workers > settings.job_pool_core {
        settings.workers -= settings.job_pool_core;
    }
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        wake: Condvar::new(),
        tick_hook: Mutex::new(Vec::new()),
        workers: settings.workers,
        queue_cap: settings.queue,
        job_queue_max: settings.job_queue_max,
        job_workers: settings.job_pool_core,
        job_pool_max: settings.job_pool_max,
        job_wake: Condvar::new(),
        check_interval: settings.check_interval,
        job_runner: settings.job_runner,
        factory: Mutex::new(Weak::new()),
        reserved: std::sync::atomic::AtomicUsize::new(0),
        runner: Mutex::new(RunnerStats::default()),
        handles: Mutex::new(Vec::new()),
    });
    let handles = spawn_all(&shared, settings.workers)?;
    *lock(&shared.handles) = handles;
    Ok(ServicePools {
        shared: Some(shared),
        inline: None,
    })
}

fn spawn_all(shared: &Arc<Shared>, workers: usize) -> Result<Vec<JoinHandle<()>>, Error> {
    let mut handles = Vec::with_capacity(workers.saturating_add(1));
    for index in 0..workers {
        if let Err(err) = push_handle(&mut handles, spawn_worker(shared, index)) {
            stop_joined(shared, handles);
            return Err(err);
        }
    }
    for index in 0..shared.job_workers {
        if let Err(err) = push_handle(&mut handles, spawn_job_worker(shared, index)) {
            stop_joined(shared, handles);
            return Err(err);
        }
    }
    if let Err(err) = push_handle(&mut handles, spawn_scheduler(shared)) {
        stop_joined(shared, handles);
        return Err(err);
    }
    Ok(handles)
}

fn push_handle(handles: &mut Vec<JoinHandle<()>>, spawned: Result<JoinHandle<()>, Error>) -> Result<(), Error> {
    handles.push(spawned?);
    Ok(())
}

fn spawn_worker(shared: &Arc<Shared>, index: usize) -> Result<JoinHandle<()>, Error> {
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name(format!("moqui-worker-{index}"))
        .spawn(move || worker_loop(shared))
        .map_err(|err| Error::new(format!("service worker did not start: {err}")))
}

fn spawn_job_worker(shared: &Arc<Shared>, index: usize) -> Result<JoinHandle<()>, Error> {
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name(format!("{JOB_THREAD}{index}"))
        .spawn(move || job_loop(shared))
        .map_err(|err| Error::new(format!("service job worker did not start: {err}")))
}

/// A job worker: scheduled job runs only, so a long job never holds a service worker, as Java's `jobWorkerPool`.
fn job_loop(shared: Arc<Shared>) {
    loop {
        let run = {
            let mut state = lock(&shared.state);
            loop {
                if state.stop {
                    return;
                }
                if let Some(run) = state.jobs.pop_front() {
                    state.jobs_active += 1;
                    break run;
                }
                state = wait(&shared.job_wake, state);
            }
        };
        run_job_work(run);
        lock(&shared.state).jobs_active -= 1;
    }
}

fn run_job_work(run: Box<super::jobs::JobRun>) {
    let _cleanup = WorkCleanup::new(Some(Arc::clone(run.factory())));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _flag = WorkerFlag::enter();
        super::jobs::run_job(*run);
    }));
    if let Err(payload) = outcome {
        tracing::error!(panic = %panic_text(payload.as_ref()), "service job panicked");
    }
}

fn spawn_scheduler(shared: &Arc<Shared>) -> Result<JoinHandle<()>, Error> {
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("moqui-scheduler".to_string())
        .spawn(move || super::schedule::scheduler_loop(shared))
        .map_err(|err| Error::new(format!("service scheduler did not start: {err}")))
}

fn worker_loop(shared: Arc<Shared>) {
    let waiter = Arc::new(std::sync::Condvar::new());
    while let Some(work) = next_work(&shared, &waiter) {
        run_work(&shared, work);
    }
}

/// The next work item, counted active under the lock that takes it off the queue, so the pool never looks empty
/// (nothing queued, nothing active) between the two: `waitWorkerPoolEmpty` would return before the work ran.
/// Waiting for work is on `waiter`, the worker's own ([`super::idle`]); the pool's `wake` is the drain's.
fn next_work(shared: &Shared, waiter: &Arc<std::sync::Condvar>) -> Option<Work> {
    let mut state = lock(&shared.state);
    // whether this wait already let the thread's script runtime go
    let mut released = false;
    loop {
        if state.halt {
            return None;
        }
        if let Some(work) = state.queue.pop_front() {
            state.active += 1;
            if state.stop && state.queue.is_empty() {
                // Tell the shutdown waiting in `drain` that the queue is empty.
                shared.wake.notify_all();
            }
            return Some(work);
        }
        if state.stop {
            return None;
        }
        if released {
            state.idle.enter(waiter);
            state = wait(waiter, state);
            state.idle.leave(waiter);
            continue;
        }
        state.warm_idle += 1;
        state.idle.enter(waiter);
        let (guard, waited) = wait_timeout(waiter, state, IDLE_RUNTIME);
        state = guard;
        state.idle.leave(waiter);
        state.warm_idle -= 1;
        // a worker idle while [`KEEP_WARM`] others are too gives back its script runtime's heap; its next script
        // starts one again. A busy pool's workers are seldom idle that long, and a quiet one keeps a few runtimes warm.
        if waited.timed_out() && state.queue.is_empty() && !state.stop && !state.halt && state.warm_idle >= KEEP_WARM {
            drop(state);
            crate::script::release_idle_runtime();
            released = true;
            state = lock(&shared.state);
        }
    }
}

/// How long a worker waits for work before it lets its script runtime go, when [`KEEP_WARM`] others are idle too.
const IDLE_RUNTIME: Duration = Duration::from_millis(500);
/// The idle workers that keep their script runtime, so a quiet pool's next call does not start one.
const KEEP_WARM: usize = 2;

/// Run work `next_work` took, which counted it active.
fn run_work(shared: &Shared, work: Work) {
    match work {
        Work::Job(run) => run_job_work(run),
        Work::Closure(job) => run_closure(shared, job),
        Work::Rules(job) => run_rules(job, true),
        other => {
            if let Some(job) = call_job(other) {
                run_call(job);
            }
        }
    }
    lock(&shared.state).active -= 1;
}

fn call_job(work: Work) -> Option<CallJob> {
    match work {
        Work::Call(job) => Some(job),
        Work::Job(_) | Work::Closure(_) | Work::Rules(_) => None,
        #[cfg(test)]
        Work::Task(task) => {
            run_task(task);
            None
        }
    }
}

#[cfg(test)]
fn run_task(task: Box<dyn FnOnce() + Send>) {
    let _cleanup = WorkCleanup::new(None);
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _flag = WorkerFlag::enter();
        task();
    }));
}

fn run_call(job: CallJob) {
    let _trace = crate::telemetry::attach(job.trace_context.clone());
    // The cleanup ends before the reply, so the caller hears only after it.
    let outcome = {
        let _cleanup = WorkCleanup::new(Some(Arc::clone(&job.factory)));
        unwind_result(catch_unwind(AssertUnwindSafe(|| {
            let _flag = WorkerFlag::enter();
            invoke(
                &job.factory,
                &job.name,
                &job.parameters,
                job.user_name.as_deref(),
                job.origin.as_ref(),
            )
        })))
    };
    match job.reply {
        Some(reply) => {
            let _ = reply.send(outcome);
        }
        None => {
            // Nobody waits on a fire-and-forget call, so a failure is only visible here.
            if let Err(err) = outcome {
                tracing::error!(service = %job.name, error = %err, "async service call failed");
            }
        }
    }
}

fn invoke(
    factory: &Arc<ExecutionContextFactory>,
    name: &str,
    parameters: &BTreeMap<String, Value>,
    user_name: Option<&str>,
    origin: Option<&crate::context::AgentOrigin>,
) -> Result<BTreeMap<String, Value>, Error> {
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    if let Some(user_name) = user_name {
        crate::user::internal_login_queued_user(user_name)?;
    }
    ctx.set_agent_origin(origin.cloned());
    // As Java's `AsyncServiceInfo`: authorization was checked as the caller before the call was queued.
    let options = super::call::CallOptions {
        disable_authz: true,
        ..super::call::CallOptions::default()
    };
    super::call::sync_with(name, parameters, &options)
}

/// `callFuture()` on the inline executor: run now in a fresh context, as the caller, under a new
/// transaction so the call does not join the caller's.
fn call_now(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    let factory = capture_factory()?;
    let parameters = match super::call::prepare_async(name, parameters)? {
        AsyncCall::Run(parameters) => parameters,
        AsyncCall::Skip(result) => return Ok(result),
    };
    let parameters = &parameters;
    let user_name = crate::user::current_username().ok().flatten();
    let origin = current_origin();
    // Checked as the caller above, so the call runs with authorization disabled, as on a worker.
    let options = super::call::CallOptions {
        require_new_transaction: Some(true),
        disable_authz: true,
        ..super::call::CallOptions::default()
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        if let Some(user_name) = user_name.as_deref() {
            crate::user::internal_login_queued_user(user_name)?;
        }
        ctx.set_agent_origin(origin);
        super::call::sync_with(name, parameters, &options)
    }));
    unwind_result(result)
}

/// One queued item on the inline executor. Mirrors [`run_work`] without the worker flag: a nested
/// `callFuture()` already runs inline. The drain runs only with no transaction on the thread, so a frame
/// left after the item is the item's and is rolled back. The script stack and sim session are cleared only
/// when no context was entered, since otherwise they belong to the caller.
fn run_inline(work: Work) {
    let factory = match &work {
        Work::Job(run) => Some(Arc::clone(run.factory())),
        Work::Call(job) => Some(Arc::clone(&job.factory)),
        _ => None,
    };
    let mut cleanup = WorkCleanup::new(factory);
    cleanup.outermost = !ExecutionContext::is_entered();
    match work {
        Work::Job(run) => {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| super::jobs::run_job(*run))) {
                tracing::error!(panic = %panic_text(payload.as_ref()), "inline service job panicked");
            }
        }
        Work::Call(job) => {
            let _trace = crate::telemetry::attach(job.trace_context.clone());
            let result = catch_unwind(AssertUnwindSafe(|| {
                invoke(
                    &job.factory,
                    &job.name,
                    &job.parameters,
                    job.user_name.as_deref(),
                    job.origin.as_ref(),
                )
            }));
            let outcome = unwind_result(result);
            drop(cleanup);
            match job.reply {
                Some(reply) => {
                    let _ = reply.send(outcome);
                }
                None => {
                    if let Err(err) = outcome {
                        tracing::error!(service = %job.name, error = %err, "async service call failed");
                    }
                }
            }
        }
        Work::Closure(job) => {
            let ClosureJob {
                run,
                reply,
                trace_context,
                what: _,
            } = job;
            let _trace = crate::telemetry::attach(trace_context);
            let _ = reply.send(closure_outcome(catch_unwind(AssertUnwindSafe(run))));
        }
        Work::Rules(job) => run_rules(job, false),
        #[cfg(test)]
        Work::Task(task) => {
            let _ = catch_unwind(AssertUnwindSafe(task));
        }
    }
}

struct DrainFlag<'a>(&'a std::sync::atomic::AtomicBool);

impl Drop for DrainFlag<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

fn reserved(
    shared: &Arc<Shared>,
    factory: Arc<ExecutionContextFactory>,
    name: &str,
    parameters: &BTreeMap<String, Value>,
) -> Result<CallReceiver, Error> {
    // A nested wait must not take a thread the pool cannot spare: at most one per worker.
    let slot = ReservedSlot::take(shared, shared.workers)?;
    let (tx, rx) = mpsc::channel();
    let name = name.to_string();
    let parameters = parameters.clone();
    let user_name = crate::user::current_username().ok().flatten();
    let origin = current_origin();
    let trace_context = crate::telemetry::capture();
    // The thread ends when the call does; its slot is held until then.
    thread::Builder::new()
        .name("moqui-reserved".to_string())
        .spawn(move || {
            let _trace = crate::telemetry::attach(trace_context);
            let _slot = slot;
            let outcome = {
                let _cleanup = WorkCleanup::new(Some(Arc::clone(&factory)));
                unwind_result(catch_unwind(AssertUnwindSafe(|| {
                    let _flag = WorkerFlag::enter();
                    invoke(&factory, &name, &parameters, user_name.as_deref(), origin.as_ref())
                })))
            };
            let _ = tx.send(outcome);
        })
        .map_err(|err| Error::new(format!("reserved worker did not start: {err}")))?;
    Ok(rx)
}

fn unwind_result(
    result: Result<Result<BTreeMap<String, Value>, Error>, Box<dyn std::any::Any + Send>>,
) -> Result<BTreeMap<String, Value>, Error> {
    match result {
        Ok(inner) => inner,
        Err(_) => Err(Error::new("service worker panicked")),
    }
}

/// Stop taking work and wait up to `limit` for the service workers to take what is queued.
fn drain(shared: &Shared, limit: Duration) {
    let deadline = std::time::Instant::now() + limit;
    let mut state = lock(&shared.state);
    state.stop = true;
    state.idle.wake_all();
    shared.wake.notify_all();
    shared.job_wake.notify_all();
    while !state.queue.is_empty() {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            tracing::warn!(
                queued = state.queue.len(),
                "Worker pool NOT YET terminated, waited {} seconds",
                limit.as_secs()
            );
            return;
        }
        state = wait_timeout(&shared.wake, state, left).0;
    }
}

fn fail_pending(shared: &Shared) {
    let (pending, jobs) = {
        let mut state = lock(&shared.state);
        state.stop = true;
        state.halt = true;
        state.idle.wake_all();
        (std::mem::take(&mut state.queue), std::mem::take(&mut state.jobs))
    };
    shared.wake.notify_all();
    shared.job_wake.notify_all();
    for work in pending {
        fail_one(work);
    }
    for run in jobs {
        super::jobs::abandon(*run);
    }
}

/// Work still queued when the pool stops. Fire-and-forget calls and tx rules are stored as queued jobs, which run
/// on the next start or on another member (HL-025); a caller waiting for a result gets an error. What is not kept is
/// logged by name.
fn fail_one(work: Work) {
    let stopped = Error::new("service worker pool is not running");
    match work {
        Work::Call(job) => match job.reply {
            Some(reply) => {
                tracing::warn!(service = %job.name, "the service worker pool stopped before a waited-for call ran");
                let _ = reply.send(Err(stopped));
            }
            None => spill_call(job),
        },
        Work::Closure(job) => {
            if let Some(what) = job.what {
                tracing::error!("{what} was not run: the service worker pool stopped first");
            }
            let _ = job.reply.send(Err(stopped));
        }
        Work::Rules(job) => spill_rules(job, "the service worker pool stopped before they ran"),
        Work::Job(run) => super::jobs::abandon(*run),
        #[cfg(test)]
        Work::Task(_) => {}
    }
}

fn queue_closure(
    shared: &Shared,
    run: ClosureFn,
    what: Option<&'static str>,
) -> Result<mpsc::Receiver<ClosureOut>, Error> {
    let (tx, rx) = mpsc::channel();
    {
        let mut state = lock(&shared.state);
        if state.stop {
            return Err(Error::new("service worker pool is not running"));
        }
        if state.queue.len() >= shared.queue_cap {
            return Err(Error::new("service worker queue is full"));
        }
        state.queue.push_back(Work::Closure(ClosureJob {
            run,
            reply: tx,
            trace_context: crate::telemetry::capture(),
            what,
        }));
        state.idle.wake_one();
    }
    Ok(rx)
}

fn reserve_closure(shared: &Arc<Shared>, run: ClosureFn) -> Result<mpsc::Receiver<ClosureOut>, Error> {
    let slot = ReservedSlot::take(shared, shared.workers)?;
    let factory = caller_factory(shared)?;
    let (tx, rx) = mpsc::channel();
    let trace_context = crate::telemetry::capture();
    thread::Builder::new()
        .name("moqui-reserved".to_string())
        .spawn(move || {
            let _trace = crate::telemetry::attach(trace_context);
            let _slot = slot;
            let outcome = {
                let _cleanup = WorkCleanup::new(Some(Arc::clone(&factory)));
                closure_outcome(catch_unwind(AssertUnwindSafe(|| {
                    let _flag = WorkerFlag::enter();
                    let ctx = factory.new_context();
                    let _entered = ctx.enter();
                    run()
                })))
            };
            let _ = tx.send(outcome);
        })
        .map_err(|err| Error::new(format!("reserved worker did not start: {err}")))?;
    Ok(rx)
}

fn run_closure(shared: &Shared, job: ClosureJob) {
    let ClosureJob {
        run,
        reply,
        trace_context,
        what: _,
    } = job;
    let _trace = crate::telemetry::attach(trace_context);
    let outcome = {
        let _cleanup = WorkCleanup::new(attached_factory(shared).ok());
        closure_outcome(catch_unwind(AssertUnwindSafe(|| enter_and_run(shared, run))))
    };
    let _ = reply.send(outcome);
}

fn enter_and_run(shared: &Shared, run: ClosureFn) -> ClosureOut {
    let _flag = WorkerFlag::enter();
    let factory = attached_factory(shared)?;
    let ctx = factory.new_context();
    let _entered = ctx.enter();
    run()
}

fn closure_outcome(result: Result<ClosureOut, Box<dyn Any + Send>>) -> ClosureOut {
    match result {
        Ok(inner) => inner,
        Err(_) => Err(Error::new("service worker panicked")),
    }
}

fn attached_factory(shared: &Shared) -> Result<Arc<ExecutionContextFactory>, Error> {
    lock(&shared.factory)
        .upgrade()
        .ok_or_else(|| Error::new("service worker pool has no factory"))
}

fn caller_factory(shared: &Shared) -> Result<Arc<ExecutionContextFactory>, Error> {
    if let Some(factory) = ExecutionContext::with_current(|ctx| Arc::clone(ctx.factory())) {
        return Ok(factory);
    }
    attached_factory(shared)
}

/// Run tx rules in a context of their own, as the caller that committed. `own_cleanup` is false on the inline path,
/// whose caller ends the work. A failure is logged with the service and phase.
fn run_rules(job: RulesJob, own_cleanup: bool) {
    let RulesJob {
        mut rules,
        factory,
        user_name,
        origin,
        trace_context,
    } = job;
    let _trace = crate::telemetry::attach(trace_context);
    let _cleanup = own_cleanup.then(|| WorkCleanup::new(Some(Arc::clone(&factory))));
    let outcome = unwind_result(catch_unwind(AssertUnwindSafe(|| {
        let _flag = own_cleanup.then(WorkerFlag::enter);
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        // As a job's user: a user that cannot be logged in is logged, and the rules still run, as before they carried
        // the caller.
        if let Some(user_name) = user_name.as_deref() {
            if let Err(err) = crate::user::internal_login_queued_user(user_name) {
                tracing::warn!(service = %rules.service, when = %rules.when, error = %err, "could not log in the user of tx rules");
            }
        }
        ctx.set_agent_origin(origin);
        super::call::run_tx_rules(&factory, &mut rules).map(|()| BTreeMap::new())
    })));
    if let Err(err) = outcome {
        tracing::error!(service = %rules.service, when = %rules.when, error = %err, "service tx rules failed");
    }
}

/// Store tx rules the queue could not take as a queued job. A failure loses them, so it is an error naming them.
fn spill_rules(job: RulesJob, why: &str) {
    let RulesJob {
        rules,
        factory,
        user_name,
        origin,
        trace_context: _,
    } = job;
    let (service, when) = (rules.service.clone(), rules.when.clone());
    let call = super::jobs::QueuedCall {
        service: rules.service,
        parameters: rules.parameters,
        user_name,
        origin,
        rules: Some(super::jobs::QueuedRules {
            when: rules.when,
            results: rules.results,
            authz: rules.authz,
        }),
    };
    match super::jobs::persist_queued(&factory, &call) {
        Ok(()) => tracing::warn!(service = %service, when = %when, "{why}: the service tx rules are queued as a job"),
        Err(err) => tracing::error!(
            service = %service,
            when = %when,
            error = %err,
            "{why}, and the service tx rules could not be queued as a job: they are not run"
        ),
    }
}

/// Store a fire-and-forget call the stopping pool did not run as a queued job.
fn spill_call(job: CallJob) {
    let CallJob {
        name,
        parameters,
        factory,
        user_name,
        origin,
        ..
    } = job;
    let call = super::jobs::QueuedCall {
        service: name.clone(),
        parameters,
        user_name,
        origin,
        rules: None,
    };
    match super::jobs::persist_queued(&factory, &call) {
        Ok(()) => {
            tracing::warn!(service = %name, "the service worker pool stopped before an async call ran; it is queued as a job")
        }
        Err(err) => tracing::error!(
            service = %name,
            error = %err,
            "the service worker pool stopped before an async call ran, and it could not be queued as a job: it is not run"
        ),
    }
}

fn stop_joined(shared: &Shared, handles: Vec<JoinHandle<()>>) {
    fail_pending(shared);
    join_all(handles);
}

fn join_all(handles: Vec<JoinHandle<()>>) {
    let deadline = std::time::Instant::now() + JOB_JOIN_WAIT;
    for handle in handles {
        // A job can run for hours: a stop waits for it only so long, so an orchestrator's kill does not find the
        // locks of every running job still held. One left running keeps its lock, which its next start takes over.
        if handle.thread().name().is_some_and(|name| name.starts_with(JOB_THREAD)) {
            while !handle.is_finished() && std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if !handle.is_finished() {
                tracing::warn!(
                    "service job thread {} still running after {} seconds of shutdown; not waiting for it",
                    name_of(&handle),
                    JOB_JOIN_WAIT.as_secs()
                );
                continue;
            }
        }
        join_one(handle);
    }
}

fn name_of(handle: &JoinHandle<()>) -> String {
    handle.thread().name().unwrap_or("(unnamed)").to_string()
}

fn join_one(handle: JoinHandle<()>) {
    if handle.thread().id() == thread::current().id() {
        std::mem::forget(handle);
        return;
    }
    let _ = handle.join();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use super::{ServicePools, DEFAULT_CHECK_SECONDS, IDLE_RUNTIME};
    use moqui_lib::object::Value;

    #[test]
    fn trace_context_crosses_workers_and_does_not_leak() {
        let ctx = crate::context::ExecutionContextFactory::test_context_with("", ServicePools::bounded(1, 4));
        let context = crate::telemetry::TraceContext::extract(
            "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01",
            "vendor=state",
        )
        .unwrap();
        {
            let _trace = crate::telemetry::attach(Some(context.clone()));
            assert_eq!(
                ctx.factory()
                    .service_pools()
                    .call(|| Ok(crate::telemetry::capture()))
                    .unwrap(),
                Some(context)
            );
        }
        assert_eq!(
            ctx.factory()
                .service_pools()
                .call(|| Ok(crate::telemetry::capture()))
                .unwrap(),
            None
        );
    }

    /// A closure a service worker waits on runs on a reserved thread, in the caller's trace.
    #[test]
    fn a_reserved_closure_joins_the_callers_trace() {
        let ctx = crate::context::ExecutionContextFactory::test_context_with("", ServicePools::bounded(1, 4));
        let context =
            crate::telemetry::TraceContext::extract("00-0123456789abcdef0123456789abcdef-0123456789abcdef-01", "")
                .unwrap();
        let shared = ctx.factory().service_pools().shared_handle();
        let run: super::ClosureFn =
            Box::new(|| Ok(Box::new(crate::telemetry::capture()) as Box<dyn std::any::Any + Send>));
        let reply = {
            let _trace = crate::telemetry::attach(Some(context.clone()));
            super::reserve_closure(&shared, run).unwrap()
        };
        let captured = reply.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(
            *captured.downcast::<Option<crate::telemetry::TraceContext>>().unwrap(),
            Some(context)
        );
    }

    /// Light work that arrives one at a time stays on the worker idle most recently (plans/Profiling3.md W13): the
    /// others are not woken, so they wait out their idle time and let their script runtimes go.
    #[test]
    fn work_one_at_a_time_runs_on_the_worker_idle_most_recently() {
        let pools = ServicePools::bounded(4, 16);
        // every worker started and waiting
        std::thread::sleep(Duration::from_millis(100));
        let mut threads = std::collections::BTreeSet::new();
        for _ in 0..10 {
            let (tx, rx) = std::sync::mpsc::channel();
            pools
                .submit_task(move || {
                    let _ = tx.send(format!("{:?}", std::thread::current().id()));
                })
                .unwrap();
            threads.insert(rx.recv_timeout(Duration::from_secs(5)).unwrap());
            // the worker is waiting again before the next arrives
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(threads.len(), 1, "{threads:?}");
    }

    /// Work taken off the queue is active at once: there is no moment with nothing queued and nothing active while it
    /// waits to run (`waitWorkerPoolEmpty` returned before an after-commit send ran).
    #[test]
    fn work_taken_off_the_queue_is_counted_active_under_the_same_lock() {
        let pools = ServicePools::bounded(1, 4);
        let shared = pools.shared_handle();
        // The one worker runs a task that waits, so the next is taken here, not by it.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        pools
            .submit_task(move || {
                let _ = started_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(10));
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        super::lock(&shared.state)
            .queue
            .push_back(super::Work::Task(Box::new(|| {})));
        let work = super::next_work(&shared, &Arc::new(std::sync::Condvar::new())).expect("the queued task");
        let status = pools.status();
        assert_eq!((status.queued, status.active), (0, 2));
        super::run_work(&shared, work);
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while pools.status().active > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(pools.status().active, 0);
    }

    /// Three workers that ran a script and then wait more than [`IDLE_RUNTIME`]: one lets its runtime go, and two
    /// ([`KEEP_WARM`]) keep theirs.
    #[test]
    fn an_idle_worker_beyond_the_warm_ones_lets_its_runtime_go() {
        // three call workers: the job workers come out of the count
        let pools = ServicePools::bounded(3 + super::DEFAULT_JOB_POOL_CORE, 3);
        let ctx = crate::context::ExecutionContextFactory::test_context_with("", pools);
        let factory = Arc::clone(ctx.factory());
        let together = Arc::new(std::sync::Barrier::new(3));
        let released = crate::script::engine_stats().oxide_released_idle;
        let calls: Vec<_> = (0..3)
            .map(|_| {
                let factory = Arc::clone(&factory);
                let together = Arc::clone(&together);
                std::thread::spawn(move || {
                    factory.service_pools().call(move || {
                        // each on a worker of its own: none returns before all three ran a script
                        let value = crate::script::eval_value("1 + 1");
                        together.wait();
                        value
                    })
                })
            })
            .collect();
        for call in calls {
            assert_eq!(call.join().unwrap().unwrap(), Value::I32(2));
        }
        std::thread::sleep(IDLE_RUNTIME + Duration::from_millis(600));
        assert!(crate::script::engine_stats().oxide_released_idle > released, "one let its runtime go");
    }

    #[test]
    fn a_script_on_a_service_worker_has_no_default_limit() {
        let ctx = crate::context::ExecutionContextFactory::test_context_with("", ServicePools::bounded(1, 1));
        let left = ctx
            .factory()
            .service_pools()
            .call(|| {
                // the deadline an outermost script arms as it starts (script/run.rs)
                let _limit = crate::script::run::ScriptLimit::arm();
                Ok::<_, crate::context::Error>(crate::script::script_time_remaining())
            })
            .unwrap();
        assert_eq!(left, None, "batch work outside a transaction runs as long as it takes");
    }

    #[test]
    fn a_panicking_tick_hook_does_not_skip_the_next() {
        let pools = ServicePools::bounded(1, 1);
        let hit = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&hit);
        pools.add_tick_hook(|| panic!("stand-in tick hook"));
        pools.add_tick_hook(move || flag.store(true, Ordering::SeqCst));
        pools.tick();
        assert!(hit.load(Ordering::SeqCst));
        assert_eq!(
            ServicePools::idle().check_interval(),
            Duration::from_secs(DEFAULT_CHECK_SECONDS)
        );
    }

    #[test]
    fn a_frame_and_sim_flag_left_by_one_job_do_not_reach_the_next() {
        use crate::context::{ExecutionContext, ExecutionContextFactory};
        use moqui_lib::object::Value;

        let ctx = ExecutionContextFactory::test_context_with("", ServicePools::bounded(1, 4));
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let pools = ctx.factory().service_pools();
        let left = facade.clone();
        pools
            .call(move || {
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                assert!(left.begin("moqui.test.TestEntity", None)?);
                let mut row = left.make_value("moqui.test.TestEntity")?;
                row.set("testId", Value::string("left-open"));
                row.create(&left)?;
                left.set_sim_active(true);
                Ok(())
            })
            .unwrap();
        let next = facade.clone();
        let (in_transaction, sim, found) = pools
            .call(move || {
                let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
                let found = next
                    .find("moqui.test.TestEntity")?
                    .eq("testId", Value::string("left-open"))
                    .one()?
                    .is_some();
                Ok((crate::transaction::thread_in_transaction(), next.sim_active(), found))
            })
            .unwrap();
        assert!(!in_transaction, "the next job joined the frame the last one left");
        assert!(!sim, "the sim flag outlived its job");
        assert!(!found, "the frame left open was committed");
    }

    /// As Java's `workerPool.shutdown()`, the calls queued when the pool shuts down still run.
    #[test]
    fn shutdown_runs_the_queued_work_first() {
        let pools = ServicePools::bounded(1, 8);
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        pools
            .submit_task(move || {
                started_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(100));
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for _ in 0..3 {
            let ran = Arc::clone(&ran);
            pools
                .submit_task(move || {
                    ran.fetch_add(1, Ordering::SeqCst);
                })
                .unwrap();
        }
        pools.shutdown();
        assert_eq!(ran.load(Ordering::SeqCst), 3, "a queued call was dropped");
        assert!(pools.submit_task(|| {}).is_err(), "a stopped pool takes no work");
    }

    /// The wait for the queue is bounded, as Java's `awaitTermination`: what is still queued then fails.
    #[test]
    fn shutdown_waits_for_the_queue_only_so_long() {
        let pools = ServicePools::bounded(1, 8);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        pools
            .submit_task(move || {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(10));
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let shared = pools.shared_handle();
        let run: super::ClosureFn = Box::new(|| Ok(Box::new(()) as Box<dyn std::any::Any + Send>));
        let queued = super::queue_closure(&shared, run, None).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _ = release_tx.send(());
        });
        let started = std::time::Instant::now();
        pools.shutdown_within(Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(5));
        let outcome = queued.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(outcome.is_err(), "the work left after the wait fails");
        releaser.join().unwrap();
    }

    #[test]
    fn an_idle_pool_runs_a_closure_inline() {
        let pools = ServicePools::idle();
        assert_eq!(pools.call(|| Ok::<_, crate::context::Error>(4)).unwrap(), 4);
        assert!(pools.call(|| Err::<(), _>(crate::context::Error::new("nope"))).is_err());
    }
}
