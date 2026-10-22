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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};

use super::{PoolConfig, TransactionAttribute, TransactionManager};

struct Shared {
    connects: AtomicUsize,
    statements: AtomicUsize,
    fail_at: AtomicUsize,
    fail_test: AtomicBool,
    log: Mutex<Vec<String>>,
    next_id: AtomicU64,
    cancels: Mutex<Vec<Arc<AtomicBool>>>,
    cancellers: AtomicUsize,
}

struct FakeDriver {
    shared: Arc<Shared>,
}

struct FakeConn {
    id: u64,
    shared: Arc<Shared>,
    cancelled: Arc<AtomicBool>,
    streaming: bool,
}

struct FlagCancel(Arc<AtomicBool>);

impl FakeDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::new(Shared {
                connects: AtomicUsize::new(0),
                statements: AtomicUsize::new(0),
                fail_at: AtomicUsize::new(0),
                fail_test: AtomicBool::new(false),
                log: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                cancels: Mutex::new(Vec::new()),
                cancellers: AtomicUsize::new(0),
            }),
        })
    }

    fn connects(&self) -> usize {
        self.shared.connects.load(Ordering::SeqCst)
    }

    fn events(&self) -> Vec<String> {
        self.shared.log.lock().expect("log").clone()
    }

    fn event_count(&self, prefix: &str) -> usize {
        self.events().iter().filter(|event| event.starts_with(prefix)).count()
    }

    fn any_cancelled(&self) -> bool {
        self.shared
            .cancels
            .lock()
            .expect("cancels")
            .iter()
            .any(|flag| flag.load(Ordering::SeqCst))
    }
}

impl FakeConn {
    fn touch(&mut self, event: &str) -> Result<(), Error> {
        let n = self.shared.statements.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.log.lock().expect("log").push(format!("sql:{event}"));
        let fail_at = self.shared.fail_at.load(Ordering::SeqCst);
        let fail_test = event == "SELECT 1" && self.shared.fail_test.load(Ordering::SeqCst);
        if fail_test || (fail_at > 0 && n == fail_at) {
            return Err(Error::new(format!("statement failed: {event}")));
        }
        Ok(())
    }

    fn mark(&self, event: &str) {
        self.shared
            .log
            .lock()
            .expect("log")
            .push(format!("{event}:{}", self.id));
    }
}

impl DbCancel for FlagCancel {
    fn cancel(&self) -> Result<(), Error> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl DbConnection for FakeConn {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<u64, Error> {
        self.touch(sql)?;
        Ok(1)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }

    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.touch(sql)?;
        Ok(vec![DbRow {
            columns: vec![("id".to_string(), Value::I64(i64::try_from(self.id).unwrap_or(i64::MAX)))],
        }])
    }

    fn open_stream(&mut self, sql: &str, _params: &[Value]) -> Result<(), Error> {
        self.touch(sql)?;
        self.streaming = true;
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(None)
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        self.streaming = false;
        Ok(())
    }

    fn stream_open(&self) -> bool {
        self.streaming
    }

    fn begin(&mut self) -> Result<(), Error> {
        self.mark("begin");
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        self.mark("commit");
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        self.mark("rollback");
        Ok(())
    }

    fn savepoint(&mut self, name: &str) -> Result<(), Error> {
        self.mark(&format!("savepoint:{name}"));
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, name: &str) -> Result<(), Error> {
        self.mark(&format!("rollback_to:{name}"));
        Ok(())
    }

    fn release_savepoint(&mut self, name: &str) -> Result<(), Error> {
        self.mark(&format!("release:{name}"));
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        self.shared.cancellers.fetch_add(1, Ordering::SeqCst);
        Box::new(FlagCancel(Arc::clone(&self.cancelled)))
    }

    fn set_statement_timeout(&mut self, remaining: Option<Duration>) -> Result<(), Error> {
        let ms = remaining.map(|duration| duration.as_millis()).unwrap_or(0);
        self.shared.log.lock().expect("log").push(format!("timeout:{ms}"));
        Ok(())
    }
}

impl DbDriver for FakeDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        self.shared.connects.fetch_add(1, Ordering::SeqCst);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.shared
            .cancels
            .lock()
            .expect("cancels")
            .push(Arc::clone(&cancelled));
        self.shared.log.lock().expect("log").push(format!("connect:{id}"));
        Ok(Box::new(FakeConn {
            id,
            shared: Arc::clone(&self.shared),
            cancelled,
            streaming: false,
        }))
    }
}

struct Clean<'a>(&'a TransactionManager);

impl Drop for Clean<'_> {
    fn drop(&mut self) {
        let _ = self.0.destroy_all_in_thread();
    }
}

fn harness(config: PoolConfig) -> (TransactionManager, Arc<FakeDriver>) {
    let driver = FakeDriver::new();
    let mgr = TransactionManager::new(Arc::clone(&driver) as Arc<dyn DbDriver>, config).expect("pool");
    (mgr, driver)
}

fn small() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 8,
        reserve: 2,
        acquire_timeout: Duration::from_secs(2),
        ..Default::default()
    }
}

fn conn_id(conn: &mut dyn DbConnection) -> Result<i64, Error> {
    let rows = conn.query_once("id", &[])?;
    match rows[0].get("id") {
        Some(Value::I64(id)) => Ok(*id),
        _ => Err(Error::new("id column missing")),
    }
}

fn position(events: &[String], needle: &str) -> usize {
    events.iter().position(|event| event.contains(needle)).expect(needle)
}

#[test]
fn manager_is_send_sync() {
    fn assert_bounds<T: Clone + Send + Sync>() {}
    assert_bounds::<TransactionManager>();
}

#[test]
fn begin_does_not_connect_and_commit_reuses_idle() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    assert_eq!(driver.connects(), 0);
    assert_eq!(mgr.stats().checkouts, 0);

    mgr.with_connection(|conn| conn.execute("one", &[])).expect("first");
    assert_eq!(driver.connects(), 1);
    assert_eq!(mgr.stats().checkouts, 1);

    mgr.with_connection(|conn| conn.execute("two", &[])).expect("second");
    assert_eq!(driver.connects(), 1);
    assert_eq!(mgr.stats().checkouts, 1);

    mgr.commit().expect("commit");
    assert_eq!(mgr.stats().checked_out, 0);
    assert_eq!(mgr.stats().ordinary_idle, 1);
    assert!(!mgr.in_transaction());

    assert!(mgr.begin(None).expect("second begin"));
    mgr.with_connection(|_| Ok(())).expect("reuse");
    assert_eq!(driver.connects(), 1);
    assert_eq!(mgr.stats().checkouts, 2);
    mgr.commit().expect("second commit");
}

#[test]
fn joined_begin_commit_does_not_commit_the_client() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let ran = Arc::new(AtomicUsize::new(0));
    assert!(mgr.begin(None).expect("outer"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    let ran_hook = Arc::clone(&ran);
    mgr.on_commit(move || {
        ran_hook.fetch_add(1, Ordering::SeqCst);
    })
    .expect("hook");
    assert!(!mgr.begin(None).expect("join"));
    mgr.commit().expect("inner commit");
    assert_eq!(driver.event_count("commit:"), 0);
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    assert!(mgr.in_transaction());
    mgr.commit().expect("outer commit");
    assert_eq!(driver.event_count("commit:"), 1);
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    assert!(!mgr.in_transaction());
}

#[test]
fn force_new_parks_the_client_and_nests_to_the_thread_budget() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("outer"));
    let outer_id = mgr.with_connection(conn_id).expect("outer conn");
    assert!(mgr.force_new(None).expect("force"));
    assert_eq!(mgr.stats().parked, 1);
    assert_eq!(mgr.stats().ordinary_idle, 0);
    let inner_id = mgr.with_connection(conn_id).expect("inner conn");
    assert_ne!(outer_id, inner_id);
    assert_eq!(driver.connects(), 2);
    mgr.commit().expect("inner commit");
    assert_eq!(mgr.stats().parked, 0);
    assert_eq!(mgr.stats().checked_out, 1);
    let resumed = mgr.with_connection(conn_id).expect("resumed");
    assert_eq!(resumed, outer_id);
    assert_eq!(driver.connects(), 2);
    mgr.commit().expect("outer commit");

    // max 8 less reserve 2 is 6 ordinary connections: 5 parked and the innermost one.
    assert_eq!(mgr.park_budget(), 5);
    assert!(mgr.begin(None).expect("again"));
    for level in 1..=5 {
        mgr.with_connection(|_| Ok(())).expect("hold");
        assert!(mgr.force_new(None).expect("nested park"));
        assert_eq!(mgr.stats().parked, level);
    }
    // A frame that has not checked out parks nothing.
    assert!(mgr.force_new(None).expect("empty frame"));
    assert_eq!(mgr.stats().parked, 5);
    mgr.with_connection(|_| Ok(())).expect("innermost");
    let err = mgr.force_new(None).expect_err("over budget");
    assert!(err.to_string().contains("park budget"), "{err}");
    for _ in 0..7 {
        mgr.commit().expect("unwind");
    }
    assert!(!mgr.in_transaction());
    assert_eq!(mgr.stats().parked, 0);
}

#[test]
fn force_new_on_two_threads_at_once() {
    let (mgr, _) = harness(small());
    let parked = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let _clean = Clean(&mgr);
                assert!(mgr.begin(None).expect("outer"));
                mgr.with_connection(|_| Ok(())).expect("outer conn");
                assert!(mgr.force_new(None).expect("force"));
                // Both threads have a client parked before either begins a second level.
                parked.wait();
                mgr.with_connection(|_| Ok(())).expect("inner conn");
                assert!(mgr
                    .force_new(None)
                    .expect("second level while the other thread is parked"));
                mgr.with_connection(|_| Ok(())).expect("innermost conn");
                parked.wait();
                for _ in 0..3 {
                    mgr.commit().expect("commit");
                }
            });
        }
    });
    assert_eq!(mgr.stats().parked, 0);
    assert_eq!(mgr.stats().checked_out, 0);
}

#[test]
fn run_force_new_nests_three_levels_and_resumes_each() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let ids = mgr
        .run(TransactionAttribute::UseOrBegin, None, || {
            let outer = mgr.with_connection(conn_id)?;
            let (middle, inner) = mgr.run(TransactionAttribute::ForceNew, None, || {
                let middle = mgr.with_connection(conn_id)?;
                let inner = mgr.run(TransactionAttribute::ForceNew, None, || {
                    assert_eq!(mgr.stats().parked, 2);
                    mgr.with_connection(conn_id)
                })?;
                assert_eq!(mgr.with_connection(conn_id)?, middle);
                Ok((middle, inner))
            })?;
            assert_eq!(mgr.with_connection(conn_id)?, outer);
            Ok([outer, middle, inner])
        })
        .expect("nested");
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_eq!(driver.event_count("commit:"), 3);
    assert_eq!(mgr.stats().parked, 0);
}

#[test]
fn ordinary_checkout_fails_when_only_reserve_is_idle() {
    let (mgr, driver) = harness(PoolConfig {
        min: 0,
        max: 2,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    });
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("ordinary");
    mgr.with_reserve(|_| Ok(())).expect("reserve");
    assert_eq!(mgr.stats().reserve_idle, 1);
    assert_eq!(mgr.stats().ordinary_idle, 0);
    assert!(mgr.force_new(None).expect("park"));
    assert_eq!(mgr.stats().parked, 1);
    let started = Instant::now();
    let err = mgr.with_connection(|_| Ok(())).expect_err("ordinary must fail");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "waited {:?}",
        started.elapsed()
    );
    assert!(err.to_string().contains("reserve"), "{err}");
    assert_eq!(mgr.stats().reserve_idle, 1);
    assert_eq!(driver.connects(), 2);
}

#[test]
fn check_caps_allows_40_and_rejects_80() {
    let (mgr, _) = harness(PoolConfig::default());
    assert!(mgr.check_caps(10, 10).is_ok());
    let err = mgr.check_caps(20, 20).expect_err("80 > 42");
    assert!(err.to_string().contains("80"), "{err}");
    assert!(err.to_string().contains("42"), "{err}");
}

#[test]
fn statement_error_marks_rollback_only_and_commit_rolls_back() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    driver.shared.fail_at.store(1, Ordering::SeqCst);
    assert!(mgr.begin(None).expect("begin"));
    let err = mgr.with_connection(|conn| conn.execute("boom", &[])).expect_err("boom");
    assert!(err.to_string().contains("boom"), "{err}");
    let next = mgr.with_connection(|_| Ok(())).expect_err("blocked");
    assert!(next.to_string().contains("rollback-only"), "{next}");
    let commit = mgr.commit().expect_err("rollback commit");
    assert!(commit.to_string().contains("rollback-only"), "{commit}");
    assert!(commit.to_string().contains("boom"), "{commit}");
    assert_eq!(driver.event_count("rollback:"), 1);
    assert_eq!(driver.event_count("commit:"), 0);
    assert!(!mgr.in_transaction());
}

#[test]
fn callbacks_run_cache_then_feed_then_commit_then_after() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let idle_at_after = Arc::new(AtomicBool::new(false));
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    mgr.on_cache_flush(|conn| conn.execute("cache-flush", &[]).map(|_| ()))
        .expect("cache");
    mgr.on_feed_insert(|conn| conn.execute("feed-insert", &[]).map(|_| ()))
        .expect("feed");
    let mgr2 = mgr.clone();
    let flag = Arc::clone(&idle_at_after);
    let log = Arc::clone(&driver.shared);
    mgr.on_commit(move || {
        let stats = mgr2.stats();
        flag.store(stats.checked_out == 0 && stats.ordinary_idle >= 1, Ordering::SeqCst);
        log.log.lock().expect("log").push("after-commit".to_string());
    })
    .expect("after");
    mgr.commit().expect("commit");
    let events = driver.events();
    let cache = position(&events, "sql:cache-flush");
    let feed = position(&events, "sql:feed-insert");
    let commit = position(&events, "commit:");
    let after = position(&events, "after-commit");
    assert!(cache < feed && feed < commit && commit < after, "{events:?}");
    assert!(idle_at_after.load(Ordering::SeqCst));
}

#[test]
fn destroy_all_in_thread_commits_open_and_parked_frames() {
    let (mgr, driver) = harness(small());
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("work");
    mgr.destroy_all_in_thread().expect("destroy");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 1);
    assert_eq!(mgr.stats().checked_out, 0);
    assert_eq!(mgr.stats().parked, 0);
    mgr.destroy_all_in_thread().expect("empty");

    assert!(mgr.begin(None).expect("outer"));
    mgr.with_connection(|_| Ok(())).expect("outer work");
    assert!(mgr.force_new(None).expect("inner"));
    mgr.with_connection(|_| Ok(())).expect("inner work");
    mgr.destroy_all_in_thread().expect("both");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 3);
    assert_eq!(mgr.stats().parked, 0);
    assert_eq!(mgr.stats().checked_out, 0);
}

#[test]
fn rollback_all_in_thread_rolls_back_open_joined_and_parked_frames() {
    let (mgr, driver) = harness(small());
    assert!(mgr.begin(None).expect("outer"));
    mgr.with_connection(|_| Ok(())).expect("outer work");
    assert!(!mgr.begin(None).expect("joined"));
    assert!(mgr.force_new(None).expect("inner"));
    mgr.with_connection(|_| Ok(())).expect("inner work");
    mgr.rollback_all_in_thread().expect("rollback all");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 0);
    assert_eq!(driver.event_count("rollback:"), 2);
    assert_eq!(mgr.stats().parked, 0);
    assert_eq!(mgr.stats().checked_out, 0);
    mgr.rollback_all_in_thread().expect("empty");
}

#[test]
fn managers_do_not_share_a_stack() {
    let (first, _) = harness(small());
    let (second, _) = harness(small());
    let _clean_first = Clean(&first);
    let _clean_second = Clean(&second);
    assert!(first.begin(None).expect("first"));
    assert!(second.begin(None).expect("second"));
    first.commit().expect("first commit");
    assert!(second.in_transaction());
    second.commit().expect("second commit");
}

#[test]
fn reserve_work_does_not_take_the_enlisted_connection() {
    let (mgr, _) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    let enlisted = mgr.with_connection(conn_id).expect("enlist");
    let reserve = mgr
        .with_reserve(|conn| {
            let id = conn_id(conn)?;
            conn.begin()?;
            conn.execute("reserve-work", &[])?;
            conn.commit()?;
            Ok(id)
        })
        .expect("reserve");
    assert_ne!(enlisted, reserve);
    assert_eq!(mgr.stats().reserve_idle, 1);
    assert_eq!(mgr.with_connection(conn_id).expect("still enlisted"), enlisted);
}

#[test]
fn fresh_connections_run_application_sql_before_any_checkout_test() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    mgr.with_connection(|conn| conn.query_once("metadata-probe", &[]))
        .expect("ordinary connect");
    mgr.with_reserve(|conn| conn.query_once("reserve-metadata-probe", &[]))
        .expect("reserve connect");
    assert_eq!(driver.connects(), 2);
    assert_eq!(driver.event_count("sql:SELECT 1"), 0);
    assert_eq!(driver.event_count("sql:metadata-probe"), 1);
    assert_eq!(driver.event_count("sql:reserve-metadata-probe"), 1);
    assert_eq!(mgr.stats().ordinary_idle, 1);
    assert_eq!(mgr.stats().reserve_idle, 1);
    assert_eq!(mgr.stats().checked_out, 0);

    mgr.with_connection(|_| Ok(())).expect("ordinary reuse");
    mgr.with_reserve(|_| Ok(())).expect("reserve reuse");
    assert_eq!(driver.connects(), 2);
    assert_eq!(driver.event_count("sql:SELECT 1"), 2);
}

#[test]
fn failed_checkout_test_discards_the_idle_connection() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    mgr.with_connection(|_| Ok(())).expect("warm");
    assert_eq!(mgr.stats().ordinary_idle, 1);
    driver.shared.fail_test.store(true, Ordering::SeqCst);
    let err = mgr.with_connection(|_| Ok(())).expect_err("bad test");
    assert!(err.to_string().contains("connection test failed"), "{err}");
    assert_eq!(driver.connects(), 1);
    assert_eq!(mgr.stats().ordinary_idle, 0);
    assert_eq!(mgr.stats().checked_out, 0);
}

#[test]
fn idle_connections_past_the_idle_time_close_down_to_min() {
    let config = PoolConfig {
        idle_timeout: Some(Duration::from_millis(30)),
        ..small()
    };
    let (mgr, driver) = harness(config);
    let _clean = Clean(&mgr);
    mgr.with_connection(|_| Ok(())).expect("warm");
    std::thread::sleep(Duration::from_millis(60));
    mgr.with_connection(|_| Ok(())).expect("after idle");
    assert_eq!(driver.connects(), 2, "the stale client was closed and a new one opened");
    assert_eq!(mgr.stats().ordinary_idle, 1);

    let kept = PoolConfig {
        min: 1,
        idle_timeout: Some(Duration::from_millis(30)),
        ..small()
    };
    let (mgr, driver) = harness(kept);
    let _clean = Clean(&mgr);
    mgr.with_connection(|_| Ok(())).expect("warm");
    std::thread::sleep(Duration::from_millis(60));
    mgr.with_connection(|_| Ok(())).expect("after idle");
    assert_eq!(driver.connects(), 1, "min keeps the one client");
}

#[test]
fn idle_checkout_runs_the_configured_test_query() {
    let config = PoolConfig {
        test_query: Some("SELECT 42".to_string()),
        ..small()
    };
    let (mgr, driver) = harness(config);
    let _clean = Clean(&mgr);
    mgr.with_connection(|_| Ok(())).expect("connect");
    assert_eq!(driver.event_count("sql:SELECT 42"), 0);
    mgr.with_connection(|_| Ok(())).expect("reuse");
    assert_eq!(driver.event_count("sql:SELECT 42"), 1);
    assert_eq!(driver.event_count("sql:SELECT 1"), 0);
}

#[test]
fn poll_deadline_marks_rollback_only_and_cancels() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(Some(Duration::ZERO)).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    mgr.poll_deadline().expect("poll");
    assert!(driver.any_cancelled());
    let blocked = mgr.with_connection(|_| Ok(())).expect_err("blocked");
    assert!(blocked.to_string().contains("rollback-only"), "{blocked}");
    let err = mgr.commit().expect_err("deadline commit");
    assert!(err.to_string().contains("deadline"), "{err}");
    assert_eq!(driver.event_count("rollback:"), 1);
    assert_eq!(driver.event_count("commit:"), 0);
}

#[test]
fn savepoint_forwards_and_errors_without_a_connection() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let missing = mgr.savepoint("s").expect_err("no conn");
    assert!(missing.to_string().contains("no connection enlisted"), "{missing}");
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    mgr.savepoint("s").expect("savepoint");
    mgr.rollback_to_savepoint("s").expect("rollback to");
    mgr.release_savepoint("s").expect("release");
    let events = driver.events().join(" ");
    assert!(events.contains("savepoint:s"), "{events}");
    assert!(events.contains("rollback_to:s"), "{events}");
    assert!(events.contains("release:s"), "{events}");
}

#[test]
fn open_stream_reuses_the_enlisted_connection() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|conn| conn.open_stream("stream", &[]))
        .expect("open");
    assert_eq!(driver.connects(), 1);
    mgr.with_connection(|conn| {
        assert!(conn.stream_open());
        Ok(())
    })
    .expect("reuse");
    assert_eq!(driver.connects(), 1);
    assert_eq!(mgr.stats().checkouts, 1);
    let err = mgr.force_new(None).expect_err("stream");
    assert!(err.to_string().contains("open stream"), "{err}");
}

#[test]
fn rollback_info_is_logged_on_commit() {
    let (mgr, _) = harness(small());
    let _clean = Clean(&mgr);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let subscriber = LogSubscriber(Arc::clone(&lines));
    tracing::subscriber::with_default(subscriber, || {
        assert!(mgr.begin(None).expect("begin"));
        mgr.set_rollback_only("boom-cause").expect("mark");
        let err = mgr.commit().expect_err("rollback commit");
        assert!(err.to_string().contains("rollback-only"), "{err}");
        assert!(err.to_string().contains("boom-cause"), "{err}");
    });
    let text = lines.lock().expect("lines").join("\n");
    assert!(text.contains("boom-cause"), "{text}");
    assert!(text.contains("callsite="), "{text}");
    assert!(text.contains("error="), "{text}");
}

#[test]
fn the_deadline_canceller_is_installed_once_per_frame() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    for _ in 0..5 {
        mgr.with_connection(|_| Ok(())).expect("statement");
    }
    assert_eq!(driver.shared.cancellers.load(Ordering::SeqCst), 1);
    mgr.commit().expect("commit");
    assert!(mgr.begin(None).expect("begin again"));
    mgr.with_connection(|_| Ok(())).expect("enlist again");
    mgr.with_connection(|_| Ok(())).expect("statement");
    assert_eq!(driver.shared.cancellers.load(Ordering::SeqCst), 2);
    mgr.commit().expect("commit again");
}

#[test]
fn deadline_timer_cancels_without_poll() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(Some(Duration::from_millis(80))).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    std::thread::sleep(Duration::from_millis(400));
    let blocked = mgr.with_connection(|_| Ok(())).expect_err("blocked");
    assert!(blocked.to_string().contains("rollback-only"), "{blocked}");
    assert!(blocked.to_string().contains("deadline"), "{blocked}");
    assert!(driver.any_cancelled());
    let err = mgr.commit().expect_err("deadline commit");
    assert!(err.to_string().contains("deadline"), "{err}");
}

#[test]
fn run_ignore_joins_ignore_suspend_suspends_and_cache_follows_the_attribute() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    // Nothing in place: ignore begins nothing.
    mgr.run(TransactionAttribute::Ignore, None, || {
        assert!(!mgr.in_transaction());
        Ok(())
    })
    .expect("ignore alone");
    assert!(mgr.begin(None).expect("begin"));
    let enlisted = mgr.with_connection(conn_id).expect("enlist");
    // In place: ignore runs inside it, on the same connection, as Java.
    mgr.run(TransactionAttribute::Ignore, None, || {
        assert!(mgr.in_transaction());
        assert_eq!(mgr.with_connection(conn_id)?, enlisted);
        Ok(())
    })
    .expect("ignore joins");
    mgr.run(TransactionAttribute::IgnoreSuspend, None, || {
        assert!(!mgr.in_transaction());
        assert!(!mgr.transaction_cache_active());
        Ok(())
    })
    .expect("ignore suspend");
    assert_eq!(mgr.with_connection(conn_id).expect("resumed"), enlisted);
    mgr.commit().expect("commit");

    // An error under ignore marks the transaction in place rollback-only.
    assert!(mgr.begin(None).expect("begin"));
    let failed = mgr.run(TransactionAttribute::Ignore, None, || -> Result<(), Error> {
        Err(Error::new("body failed"))
    });
    assert!(failed.is_err());
    let err = mgr.commit().expect_err("rollback-only commit");
    assert!(err.to_string().contains("body failed"), "{err}");

    mgr.run(TransactionAttribute::UseOrBegin, None, || {
        assert!(!mgr.transaction_cache_active());
        Ok(())
    })
    .expect("plain");
    mgr.run(TransactionAttribute::Cache, None, || {
        assert!(mgr.transaction_cache_active());
        Ok(())
    })
    .expect("cache");
    mgr.run(TransactionAttribute::ForceCache, None, || {
        assert!(mgr.transaction_cache_active());
        Ok(())
    })
    .expect("force cache");

    // use-transaction-cache="false" keeps it off even for the cache attribute.
    mgr.set_use_transaction_cache(false);
    mgr.run(TransactionAttribute::Cache, None, || {
        assert!(!mgr.transaction_cache_active());
        Ok(())
    })
    .expect("cache off");
    assert!(!mgr.in_transaction());
    assert!(driver.event_count("commit:") >= 1);
}

#[test]
fn run_rolls_back_a_begun_frame_when_the_body_panics() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = mgr.run(TransactionAttribute::UseOrBegin, None, || -> Result<(), Error> {
            mgr.with_connection(|_| Ok(()))?;
            panic!("service body failed");
        });
    }));
    assert!(outcome.is_err());
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 0);
    assert_eq!(driver.event_count("rollback:"), 1);
}

#[test]
fn cache_attribute_on_a_joined_frame_keeps_the_cache_for_that_frame() {
    let (mgr, _) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    assert!(!mgr.transaction_cache_active());
    mgr.run(TransactionAttribute::Cache, None, || {
        assert!(mgr.transaction_cache_active());
        // A force-new frame has its own setting.
        mgr.run(TransactionAttribute::ForceNew, None, || {
            assert!(!mgr.transaction_cache_active());
            Ok(())
        })?;
        assert!(mgr.transaction_cache_active());
        Ok(())
    })
    .expect("cache");
    // As Java initTransactionCache: the cache stays on until this frame ends.
    assert!(mgr.transaction_cache_active());
    mgr.commit().expect("commit");
    assert!(mgr.begin(None).expect("next"));
    assert!(!mgr.transaction_cache_active());
    mgr.commit().expect("next commit");
}

#[test]
fn statement_timeout_is_sent_only_when_enabled() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(Some(Duration::from_secs(5))).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("plain");
    assert!(!driver.events().iter().any(|event| event.starts_with("timeout:")));
    mgr.commit().expect("commit");

    mgr.set_use_statement_timeout(true);
    assert!(mgr.begin(Some(Duration::from_secs(5))).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("limited");
    let timeouts: Vec<_> = driver
        .events()
        .into_iter()
        .filter(|event| event.starts_with("timeout:"))
        .collect();
    assert_eq!(timeouts.len(), 1, "{timeouts:?}");
    let ms: u128 = timeouts[0].trim_start_matches("timeout:").parse().expect("ms");
    assert!(ms > 1_000 && ms <= 5_000, "{ms}");
    mgr.with_connection(|_| Ok(())).expect("second statement");
    let armed = driver
        .events()
        .into_iter()
        .filter(|event| event.starts_with("timeout:"))
        .count();
    assert_eq!(armed, 1, "armed once per enlist");
    mgr.commit().expect("commit");
    let last = driver
        .events()
        .into_iter()
        .rfind(|event| event.starts_with("timeout:"))
        .expect("cleared");
    assert_eq!(last, "timeout:0", "returned connection has no timeout");
}

#[test]
fn lock_track_warns_only_for_overlapping_transactions() {
    let (mgr, _) = harness(small());
    let _clean = Clean(&mgr);
    mgr.set_use_lock_track(true);

    let sequential = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(LogSubscriber(Arc::clone(&sequential)), || {
        for _ in 0..2 {
            assert!(mgr.begin(None).expect("begin"));
            mgr.note_entity_lock("moqui.test.TestEntity", "T1");
            mgr.note_entity_lock("moqui.test.TestEntity", "T1");
            mgr.commit().expect("commit");
        }
    });
    let text = sequential.lock().expect("lines").join("\n");
    assert!(!text.contains("same entity primary key"), "{text}");

    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(LogSubscriber(Arc::clone(&lines)), || {
        assert!(mgr.begin(None).expect("begin"));
        mgr.note_entity_lock("moqui.test.TestEntity", "T1");
        let other = mgr.clone();
        let other_lines = Arc::clone(&lines);
        std::thread::spawn(move || {
            tracing::subscriber::with_default(LogSubscriber(other_lines), || {
                assert!(other.begin(None).expect("begin"));
                other.note_entity_lock("moqui.test.TestEntity", "T1");
                other.commit().expect("commit");
            });
        })
        .join()
        .expect("join");
        mgr.commit().expect("commit");
    });
    let text = lines.lock().expect("lines").join("\n");
    assert!(text.contains("same entity primary key"), "{text}");
}

struct LogSubscriber(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for LogSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visit = FieldVisit(String::new());
        event.record(&mut visit);
        self.0.lock().expect("lines").push(visit.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

struct FieldVisit(String);

impl tracing::field::Visit for FieldVisit {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, "{}={:?} ", field.name(), value);
    }
}

#[test]
fn joined_rollback_marks_the_frame_rollback_only_and_gives_the_join_back() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("outer"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    assert!(!mgr.begin(None).expect("join"));
    mgr.rollback().expect("joined rollback");
    assert!(mgr.in_transaction(), "the owner's frame is still in place");
    assert_eq!(driver.event_count("rollback:"), 0);
    let err = mgr.commit().expect_err("owner commit");
    assert!(err.to_string().contains("rollback-only"), "{err}");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("rollback:"), 1);
    assert_eq!(driver.event_count("commit:"), 0);
}

#[test]
fn a_dropped_scope_rolls_back_or_poisons_and_a_committed_one_ends_its_frame() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    {
        let scope = mgr.scope(None).expect("scope");
        assert!(scope.began());
        mgr.with_connection(|_| Ok(())).expect("enlist");
    }
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("rollback:"), 1);

    let outer = mgr.scope(None).expect("outer");
    mgr.with_connection(|_| Ok(())).expect("enlist");
    let joined = mgr.scope(None).expect("joined");
    assert!(!joined.began());
    joined.commit().expect("joined commit");
    assert!(mgr.in_transaction());
    {
        let _dropped = mgr.scope(None).expect("joined again");
    }
    assert!(mgr.in_transaction(), "a dropped joined scope only gives its join back");
    let err = outer.commit().expect_err("poisoned");
    assert!(err.to_string().contains("rollback-only"), "{err}");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 0);

    let scope = mgr.scope(None).expect("again");
    mgr.with_connection(|_| Ok(())).expect("enlist");
    scope.commit().expect("commit");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 1);
}

#[test]
fn run_rolls_back_when_the_body_leaves_a_join_open() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let result = mgr.run(TransactionAttribute::UseOrBegin, None, || {
        mgr.with_connection(|_| Ok(()))?;
        // A joined begin the body never ends.
        assert!(!mgr.begin(None)?);
        Ok(())
    });
    let err = result.expect_err("out of order");
    assert!(err.to_string().contains("rollback-only"), "{err}");
    assert!(!mgr.in_transaction(), "no frame is left behind");
    assert_eq!(driver.event_count("commit:"), 0);
    assert_eq!(driver.event_count("rollback:"), 1);
}

#[test]
fn run_rolls_back_a_frame_the_body_began_and_left_open() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let result = mgr.run(TransactionAttribute::UseOrBegin, None, || {
        mgr.with_connection(|_| Ok(()))?;
        assert!(mgr.force_new(None)?);
        mgr.with_connection(|_| Ok(()))?;
        Ok(())
    });
    assert!(result.is_err());
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 0);
    assert_eq!(driver.event_count("rollback:"), 2);
    assert_eq!(mgr.stats().parked, 0);
    assert_eq!(mgr.stats().checked_out, 0);
}

#[test]
fn ignore_suspend_stays_suspended_after_a_nested_transaction() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("outer"));
    let outer = mgr.with_connection(conn_id).expect("outer conn");
    mgr.run(TransactionAttribute::IgnoreSuspend, None, || {
        mgr.run(TransactionAttribute::UseOrBegin, None, || {
            mgr.with_connection(|_| Ok(()))
        })?;
        assert!(!mgr.in_transaction(), "the outer transaction stays suspended");
        assert_eq!(mgr.stats().parked, 1);
        let direct = mgr.with_connection(|conn| {
            conn.execute("direct", &[])?;
            conn_id(conn)
        })?;
        assert_ne!(direct, outer, "the direct write does not join the outer transaction");
        mgr.run(TransactionAttribute::ForceNew, None, || mgr.with_connection(|_| Ok(())))?;
        assert!(!mgr.in_transaction());
        Ok(())
    })
    .expect("ignore suspend");
    assert!(mgr.in_transaction(), "resumed at the end");
    assert_eq!(mgr.stats().parked, 0);
    assert_eq!(mgr.with_connection(conn_id).expect("resumed"), outer);
    mgr.commit().expect("outer commit");
    assert_eq!(driver.event_count("commit:"), 3);
}

#[test]
fn a_panicking_commit_hook_rolls_back_and_ends_the_frame() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    let token = mgr.frame_token().expect("token");
    mgr.on_cache_flush(|_| panic!("flush hook failed")).expect("hook");
    let err = mgr.commit().expect_err("hook panic");
    assert!(err.to_string().contains("flush hook failed"), "{err}");
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("rollback:"), 1);
    assert_eq!(driver.event_count("commit:"), 0);
    assert_eq!(mgr.stats().checked_out, 0);
    assert!(!super::manager::deadline_registered(token));
}

#[test]
fn a_panicking_after_commit_hook_does_not_skip_the_rest() {
    let (mgr, driver) = harness(small());
    let _clean = Clean(&mgr);
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    assert!(mgr.begin(None).expect("begin"));
    mgr.with_connection(|_| Ok(())).expect("enlist");
    mgr.on_commit(|| panic!("after-commit hook failed")).expect("hook");
    let counter = ran.clone();
    mgr.on_commit(move || {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    })
    .expect("hook");
    mgr.commit().expect("the commit succeeded, whatever a hook after it does");
    assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!mgr.in_transaction());
    assert_eq!(driver.event_count("commit:"), 1);
}

#[test]
fn deadline_slots_leave_with_their_frames() {
    let (mgr, _) = harness(small());
    let _clean = Clean(&mgr);
    assert!(mgr.begin(None).expect("begin"));
    let token = mgr.frame_token().expect("token");
    assert!(super::manager::deadline_registered(token));
    mgr.commit().expect("commit");
    assert!(!super::manager::deadline_registered(token));

    // A thread that ends with its frame still on the stack.
    let other = mgr.clone();
    let token = std::thread::spawn(move || {
        assert!(other.begin(None).expect("begin"));
        other.frame_token().expect("token")
    })
    .join()
    .expect("join");
    assert!(!super::manager::deadline_registered(token));
}
