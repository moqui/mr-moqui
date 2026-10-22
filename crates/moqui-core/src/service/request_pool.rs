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
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use moqui_lib::node::MNode;
use moqui_lib::sync::lock;

use super::pool::WorkCleanup;
use crate::context::{Error, ErrorKind, ExecutionContextFactory};

const DEFAULT_REQUEST_WORKERS: u32 = 8;
const DEFAULT_REQUEST_QUEUE: u32 = 64;
const DEFAULT_SERVICE_WORKERS: u32 = 32;

/// Factory-owned request workers. [`RequestPool::idle`] starts no threads.
///
/// Drop sets the stop flag, wakes this pool's workers, fails queued waiters, and joins the threads.
#[must_use = "dropping the pool stops and joins its threads"]
pub struct RequestPool {
    inner: Mutex<Option<Arc<Shared>>>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    workers: usize,
    queue_cap: usize,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct State {
    stop: bool,
    queue: VecDeque<Work>,
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
        let shared = self.running()?;
        let (tx, rx) = mpsc::channel();
        {
            let mut state = lock(&shared.state);
            if state.stop {
                return Err(Error::of_kind(ErrorKind::Unavailable, "request pool is not running"));
            }
            if state.queue.len() >= shared.queue_cap {
                return Err(Error::of_kind(ErrorKind::Unavailable, "request queue is full"));
            }
            state.queue.push_back(pack(factory, job, tx));
        }
        shared.wake.notify_all();
        Ok(rx)
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

/// `(request-pool-max, worker-pool-max)`. Missing or non-integer attributes are 8 and 32.
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

/// Errors when `(request cap + service cap) * 2` does not fit the default group's pool, with room for one agent
/// worker more in a build with the `llm` feature: the agent pool takes as many of its `max-concurrent-runs` as the
/// rest of the pool leaves ([`spare_connections`]), at least one.
pub fn check_serve_caps(factory: &ExecutionContextFactory) -> Result<(), Error> {
    let facade = factory.entity().ok_or_else(|| Error::new("no entity facade"))?;
    let (request_cap, service_cap) = caps(factory.conf_text());
    let manager = default_manager(&facade)?;
    let agent_workers = if cfg!(feature = "llm") { 1 } else { 0 };
    manager.check_caps_with(request_cap, service_cap, agent_workers)?;
    Ok(())
}

fn default_manager(facade: &crate::entity::EntityFacade) -> Result<crate::transaction::TransactionManager, Error> {
    let group = facade.inner.default_group.clone();
    facade
        .inner
        .groups
        .get(&group)
        .map(|runtime| runtime.transactions.clone())
        .ok_or_else(|| Error::new(format!("no transaction manager for entity group {group}")))
}

/// The connections of the default group's pool that the request and service workers leave (`max - reserve` less
/// `(request cap + service cap) * 2`); `None` with no entity facade.
pub fn spare_connections(factory: &ExecutionContextFactory) -> Option<u32> {
    let facade = factory.entity()?;
    let (request_cap, service_cap) = caps(factory.conf_text());
    default_manager(&facade)
        .ok()
        .map(|manager| manager.spare_connections(request_cap, service_cap))
}

fn pack<T, F>(factory: Arc<ExecutionContextFactory>, job: F, tx: Sender<Result<T, Error>>) -> Work
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, Error> + Send + 'static,
{
    Work {
        factory,
        run: Box::new(move || job().map(|value| Box::new(value) as JobOutput)),
        finish: Box::new(move |result| {
            let _ = tx.send(map_job_result(result));
        }),
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
    let root = MNode::parse_text("moqui-conf", conf_text)?;
    if root.name() == "tools" {
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
        wake: Condvar::new(),
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
    while let Some(work) = next_work(&shared) {
        run_work(work);
    }
}

fn next_work(shared: &Shared) -> Option<Work> {
    let mut state = lock(&shared.state);
    loop {
        if state.stop {
            return None;
        }
        if let Some(work) = state.queue.pop_front() {
            return Some(work);
        }
        state = shared.wake.wait(state).unwrap_or_else(|err| err.into_inner());
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
        std::mem::take(&mut state.queue)
    };
    shared.wake.notify_all();
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
    }
    shared.wake.notify_all();
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
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use moqui_lib::error::Error as LibError;
    use moqui_lib::node::MNode;
    use moqui_lib::object::Value;

    use super::{caps, check_serve_caps, RequestPool};
    use crate::context::{Error, ExecutionContextFactory};
    use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};
    use crate::entity::{EntityFacade, EntityGroup};
    use crate::transaction::{PoolConfig, TransactionManager};

    #[test]
    fn idle_rejects_and_runs_nothing() {
        let pool = RequestPool::idle();
        assert_eq!(pool.queue_depth(), 0);
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_job = Arc::clone(&ran);
        let err = pool
            .try_submit(test_factory(), move || {
                ran_job.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Error>(())
            })
            .unwrap_err();
        assert!(err.to_string().contains("not running"), "{err}");
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn full_queue_rejects_without_running_the_job() {
        let pool = RequestPool::idle();
        pool.start(1, 1).unwrap();
        let factory = test_factory();
        let running = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let _release = NotifyOnDrop(Arc::clone(&gate));
        let running_job = Arc::clone(&running);
        let gate_job = Arc::clone(&gate);
        let first = pool
            .try_submit(Arc::clone(&factory), move || {
                set_flag(&running_job);
                wait_flag(&gate_job);
                Ok::<(), Error>(())
            })
            .unwrap();
        assert!(
            wait_flag_timeout(&running, Duration::from_secs(5)),
            "blocking job did not start"
        );
        let second = pool.try_submit(Arc::clone(&factory), || Ok::<(), Error>(())).unwrap();
        assert_eq!(pool.queue_depth(), 1);
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_job = Arc::clone(&ran);
        let err = pool
            .try_submit(factory, move || {
                ran_job.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Error>(())
            })
            .unwrap_err();
        assert!(err.to_string().contains("request queue is full"), "{err}");
        assert_eq!(ran.load(Ordering::SeqCst), 0);
        set_flag(&gate);
        first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        second.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn caps_read_request_and_service_max() {
        let text = r#"<tools request-pool-max="3" request-queue="5" worker-pool-max="9"/>"#;
        assert_eq!(caps(text), (3, 9));
        assert_eq!(super::request_sizes(text).unwrap(), (3, 5));
        assert_eq!(caps("<moqui-conf/>"), (8, 32));
        assert_eq!(super::request_sizes("<moqui-conf/>").unwrap(), (8, 64));
        let junk =
            r#"<moqui-conf><tools request-pool-max="nope" request-queue="nope" worker-pool-max="nope"/></moqui-conf>"#;
        assert_eq!(caps(junk), (8, 32));
        assert_eq!(super::request_sizes(junk).unwrap(), (8, 64));
        let conf = std::fs::read_to_string(framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap();
        assert_eq!(caps(&conf), (8, 32));
        let root = MNode::parse_text("moqui-conf", &conf).unwrap();
        let tools = root.first_named("tools").unwrap();
        assert_eq!(tools.attribute("request-pool-max").as_deref(), Some("8"));
        assert_eq!(tools.attribute("request-queue").as_deref(), Some("64"));
    }

    #[test]
    fn panic_is_an_error_and_the_worker_continues() {
        let pool = RequestPool::idle();
        pool.start(1, 4).unwrap();
        let factory = test_factory();
        let first = pool
            .try_submit(Arc::clone(&factory), || -> Result<(), Error> {
                panic!("request boom");
            })
            .unwrap();
        let err = first.recv_timeout(Duration::from_secs(5)).unwrap().unwrap_err();
        assert!(err.to_string().contains("panicked"), "{err}");
        let second = pool.try_submit(factory, || Ok::<u32, Error>(7)).unwrap();
        assert_eq!(second.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), 7);
    }

    #[test]
    fn serve_caps_reject_a_small_pool_and_accept_8_and_32() {
        let ctx = ExecutionContextFactory::test_context();
        let factory = ctx.factory();
        let missing = check_serve_caps(factory).unwrap_err();
        assert!(missing.to_string().contains("no entity facade"), "{missing}");
        assert_eq!(caps(factory.conf_text()), (8, 32));

        factory.set_entity(facade(PoolConfig {
            min: 0,
            max: 4,
            reserve: 0,
            acquire_timeout: Duration::from_secs(1),
            ..Default::default()
        }));
        let err = check_serve_caps(factory).unwrap_err();
        assert!(err.to_string().contains("(8 + 32) * 2"), "{err}");

        // 80 for the request and service workers, and one agent worker with the llm feature, within 90 less 8 reserve
        factory.set_entity(facade(PoolConfig {
            min: 0,
            max: 90,
            reserve: 8,
            acquire_timeout: Duration::from_secs(1),
            ..Default::default()
        }));
        check_serve_caps(factory).unwrap();
        assert_eq!(super::spare_connections(factory), Some(2));
    }

    #[test]
    fn worker_destroys_every_thread_transaction() {
        let pool = RequestPool::idle();
        pool.start(1, 2).unwrap();
        let ctx = ExecutionContextFactory::test_context();
        let factory = Arc::clone(ctx.factory());
        let primary = memory_manager();
        let other = memory_manager();
        let sim = memory_manager();
        let facade = EntityFacade::open(
            &framework_dir(),
            "transactional",
            vec![
                EntityGroup::new("transactional", "postgres", primary.clone()),
                EntityGroup::new("other", "postgres", other.clone()),
            ],
        )
        .unwrap();
        facade.set_sim_transactions(Some(sim.clone()));
        factory.set_entity(facade);
        let primary_job = primary.clone();
        let other_job = other.clone();
        let sim_job = sim.clone();
        let done = pool
            .try_submit(factory, move || {
                enlist(&primary_job)?;
                enlist(&other_job)?;
                enlist(&sim_job)?;
                Ok::<(), Error>(())
            })
            .unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        assert_returned(&primary);
        assert_returned(&other);
        assert_returned(&sim);
    }

    #[test]
    fn the_sim_flag_ends_with_its_request() {
        let pool = RequestPool::idle();
        pool.start(1, 2).unwrap();
        let factory = test_factory();
        let facade = crate::testing::open();
        factory.set_entity(facade.clone());
        let first = facade.clone();
        let done = pool
            .try_submit(Arc::clone(&factory), move || {
                first.set_sim_active(true);
                Ok::<(), Error>(())
            })
            .unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
        let next = pool
            .try_submit(factory, move || Ok::<bool, Error>(facade.sim_active()))
            .unwrap();
        assert!(!next.recv_timeout(Duration::from_secs(5)).unwrap().unwrap());
    }

    #[test]
    fn a_script_on_a_request_worker_is_limited_to_the_request_timeout() {
        let pool = RequestPool::idle();
        pool.start(1, 1).unwrap();
        let left = pool
            .try_submit(test_factory(), || {
                crate::script::walker_context(|_| Ok(crate::script::script_time_remaining()))
            })
            .unwrap()
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        let expected = moqui_lib::env::get_prop_or_env("webapp_request_timeout")
            .parse::<u64>()
            .unwrap_or(300);
        if expected > 0 {
            assert!(
                left.is_some_and(|left| left <= Duration::from_secs(expected) && left > Duration::ZERO),
                "{left:?}"
            );
        }
    }

    fn test_factory() -> Arc<ExecutionContextFactory> {
        Arc::clone(ExecutionContextFactory::test_context().factory())
    }

    /// Leaves a frame open on purpose, with no scope, for the pool's end-of-job cleanup to end.
    fn enlist(manager: &TransactionManager) -> Result<(), Error> {
        manager.begin(None)?;
        manager.with_connection(|_| Ok(()))?;
        Ok(())
    }

    fn assert_returned(manager: &TransactionManager) {
        let stats = manager.stats();
        assert_eq!(stats.checked_out, 0, "{stats:?}");
        assert!(stats.ordinary_idle >= 1, "{stats:?}");
    }

    fn memory_manager() -> TransactionManager {
        TransactionManager::new(
            Arc::new(StubDriver),
            PoolConfig {
                min: 0,
                max: 4,
                reserve: 0,
                acquire_timeout: Duration::from_secs(1),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn facade(config: PoolConfig) -> EntityFacade {
        EntityFacade::open(
            &framework_dir(),
            "transactional",
            vec![EntityGroup::new(
                "transactional",
                "postgres",
                TransactionManager::new(Arc::new(StubDriver), config).unwrap(),
            )],
        )
        .unwrap()
    }

    fn framework_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
    }

    fn set_flag(flag: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, cv) = flag.as_ref();
        let mut value = lock.lock().unwrap();
        *value = true;
        drop(value);
        cv.notify_all();
    }

    fn wait_flag(flag: &Arc<(Mutex<bool>, Condvar)>) {
        let (lock, cv) = flag.as_ref();
        let mut value = lock.lock().unwrap();
        while !*value {
            value = cv.wait(value).unwrap();
        }
    }

    fn wait_flag_timeout(flag: &Arc<(Mutex<bool>, Condvar)>, timeout: Duration) -> bool {
        let (lock, cv) = flag.as_ref();
        let mut value = lock.lock().unwrap();
        let deadline = Instant::now() + timeout;
        while !*value {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            let (next, wait) = cv.wait_timeout(value, left).unwrap();
            value = next;
            if wait.timed_out() && !*value {
                return false;
            }
        }
        true
    }

    struct NotifyOnDrop(Arc<(Mutex<bool>, Condvar)>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            let (flag, cv) = self.0.as_ref();
            *moqui_lib::sync::lock(flag) = true;
            cv.notify_all();
        }
    }

    struct StubDriver;
    struct StubConn;
    struct StubCancel;

    impl DbCancel for StubCancel {
        fn cancel(&self) -> Result<(), LibError> {
            Ok(())
        }
    }

    impl DbDriver for StubDriver {
        fn connect(&self) -> Result<Box<dyn DbConnection>, LibError> {
            Ok(Box::new(StubConn))
        }
    }

    impl DbConnection for StubConn {
        fn execute(&mut self, _sql: &str, _params: &[Value]) -> Result<u64, LibError> {
            Ok(0)
        }

        fn query(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, LibError> {
            Ok(Vec::new())
        }

        fn query_once(&mut self, _sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, LibError> {
            Ok(Vec::new())
        }

        fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), LibError> {
            Ok(())
        }

        fn next_row(&mut self) -> Result<Option<DbRow>, LibError> {
            Ok(None)
        }

        fn close_stream(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn stream_open(&self) -> bool {
            false
        }

        fn begin(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn commit(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn rollback(&mut self) -> Result<(), LibError> {
            Ok(())
        }

        fn savepoint(&mut self, _name: &str) -> Result<(), LibError> {
            Ok(())
        }

        fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), LibError> {
            Ok(())
        }

        fn release_savepoint(&mut self, _name: &str) -> Result<(), LibError> {
            Ok(())
        }

        fn canceller(&self) -> Box<dyn DbCancel> {
            Box::new(StubCancel)
        }
    }
}
