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

/// The completion hook gets the result on the worker, after the job's context has ended; a job still queued when the
/// pool stops gets an error.
#[test]
fn the_completion_hook_gets_the_result_and_a_stop_fails_a_queued_job() {
    let pool = RequestPool::idle();
    pool.start(1, 4).unwrap();
    let factory = test_factory();
    let (tx, rx) = std::sync::mpsc::channel();
    let done_tx = tx.clone();
    pool.try_submit_then(
        Arc::clone(&factory),
        || Ok::<_, Error>(7),
        move |result| {
            let entered = crate::context::ExecutionContext::is_entered();
            let _ = done_tx.send((result.map_err(|err| err.to_string()), entered));
        },
    )
    .unwrap();
    let (result, entered) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(result, Ok(7));
    assert!(!entered, "the hook runs after the job's context ended");

    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _release = NotifyOnDrop(Arc::clone(&gate));
    let running = Arc::new((Mutex::new(false), Condvar::new()));
    let (gate_job, running_job) = (Arc::clone(&gate), Arc::clone(&running));
    pool.try_submit_then(
        Arc::clone(&factory),
        move || {
            set_flag(&running_job);
            wait_flag(&gate_job);
            Ok::<_, Error>(0)
        },
        |_| {},
    )
    .unwrap();
    wait_flag(&running);
    let queued_tx = tx.clone();
    pool.try_submit_then(
        Arc::clone(&factory),
        || Ok::<_, Error>(1),
        move |result| {
            let _ = queued_tx.send((result.map_err(|err| err.to_string()), false));
        },
    )
    .unwrap();
    let stopping = std::thread::spawn(move || pool.shutdown());
    let (result, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(result.unwrap_err().contains("not running"));
    set_flag(&gate);
    stopping.join().unwrap();
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
    assert_eq!(caps("<moqui-conf/>"), (16, 22));
    assert_eq!(super::request_sizes("<moqui-conf/>").unwrap(), (16, 64));
    let junk =
        r#"<moqui-conf><tools request-pool-max="nope" request-queue="nope" worker-pool-max="nope"/></moqui-conf>"#;
    assert_eq!(caps(junk), (16, 22));
    assert_eq!(super::request_sizes(junk).unwrap(), (16, 64));
    let conf = std::fs::read_to_string(framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap();
    assert_eq!(caps(&conf), (16, 22));
    let root = MNode::parse_text("moqui-conf", &conf).unwrap();
    let tools = root.first_named("tools").unwrap();
    assert_eq!(tools.attribute("request-pool-max").as_deref(), Some("16"));
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
fn serve_caps_reject_a_small_pool_and_accept_16_and_22() {
    let ctx = ExecutionContextFactory::test_context();
    let factory = ctx.factory();
    let missing = check_serve_caps(factory).unwrap_err();
    assert!(missing.to_string().contains("no entity facade"), "{missing}");
    assert_eq!(caps(&factory.conf_text()), (16, 22));

    factory.set_entity(facade(PoolConfig {
        min: 0,
        max: 4,
        reserve: 0,
        acquire_timeout: Duration::from_secs(1),
        ..Default::default()
    }));
    let err = check_serve_caps(factory).unwrap_err();
    assert!(err.to_string().contains("(16 + 22) * 2"), "{err}");

    // 76 for the request and service workers, one agent worker with the llm feature, and the 4 transfer reads (the
    // content shares this pool, as with an external transactional database) within 90 less 8 reserve
    factory.set_entity(facade(sized(90)));
    check_serve_caps(factory).unwrap();
    assert_eq!(super::transfer_readers(factory), 4);
    assert_eq!(super::spare_connections(factory), Some(2));
}

/// Streamed database downloads hold a connection of the content group's pool for each window they read: the startup
/// check counts them there, and a pool that cannot hold them beside the workers warns and admits fewer, never a reserve
/// connection (plans/archive/High-Level-Review.md HL-022).
#[test]
fn transfer_reads_count_in_the_content_pool() {
    let ctx = ExecutionContextFactory::test_context();
    let factory = ctx.factory();
    assert_eq!(super::transfer_workers(factory), 4);
    let agent = u32::from(crate::llm_hooks::linked());

    // the shipped pools, read as boot reads them: the embedded transactional database takes the postgres database
    // conf's size, the same an external Postgres gets, and the embedded content database its own
    let text = std::fs::read_to_string(framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap();
    let conf = MNode::parse_text("moqui-conf", &text).unwrap();
    let transactional = crate::entity::datasource::datasource_conf(&conf, "transactional").unwrap();
    let content = crate::entity::datasource::datasource_conf(&conf, "content").unwrap();
    assert_eq!((transactional.pool.max, transactional.pool.reserve), (90, 8));
    assert_eq!((content.pool.max, content.pool.reserve), (90, 8));
    assert_eq!(caps(&text), (16, 22));
    // (16 + 22) * 2 + 1 + 4 = 81 of 82: a content database of its own fits all 4 reads
    factory.set_entity(two_groups(transactional.pool.clone(), content.pool.clone()));
    check_serve_caps(factory).unwrap();
    assert_eq!(super::transfer_readers(factory), 4);
    assert_eq!(super::spare_connections(factory), Some(2));
    // and so does content sharing the transactional pool
    factory.set_entity(facade(transactional.pool));
    check_serve_caps(factory).unwrap();
    assert_eq!(super::transfer_readers(factory), 4);

    // a content pool with 2 left beside the workers reads fewer at once
    factory.set_entity(two_groups(sized(90), sized(86)));
    check_serve_caps(factory).unwrap();
    let readers = super::transfer_readers(factory);
    assert_eq!(readers, (2 - agent).max(1));
    assert_eq!(super::spare_connections(factory), Some(2u32.saturating_sub(readers)));

    // the transactional pool is not the content's: transfers leave it alone
    factory.set_entity(two_groups(sized(76 + agent + 8), sized(90)));
    check_serve_caps(factory).unwrap();
    assert_eq!(super::transfer_readers(factory), 4);

    // a content pool that has no room past the workers and the agent worker still reads one at a time
    factory.set_entity(two_groups(sized(90), sized(84 + agent)));
    check_serve_caps(factory).unwrap();
    assert_eq!(super::transfer_readers(factory), 1);
}

fn sized(max: u32) -> PoolConfig {
    PoolConfig {
        min: 0,
        max,
        reserve: 8,
        acquire_timeout: Duration::from_secs(1),
        ..Default::default()
    }
}

/// `transactional` and a `content` group with a pool of its own, the content group's entities in it.
fn two_groups(transactional: PoolConfig, content: PoolConfig) -> EntityFacade {
    EntityFacade::open(
        &framework_dir(),
        "transactional",
        vec![
            EntityGroup::new(
                "transactional",
                "postgres",
                TransactionManager::new(Arc::new(StubDriver), transactional).unwrap(),
            ),
            EntityGroup::new(
                "content",
                "postgres",
                TransactionManager::new(Arc::new(StubDriver), content).unwrap(),
            ),
        ],
    )
    .unwrap()
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
            // the deadline an outermost script arms as it starts (script/run.rs)
            let _limit = crate::script::run::ScriptLimit::arm();
            Ok::<_, crate::context::Error>(crate::script::script_time_remaining())
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/components/base/framework")
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
