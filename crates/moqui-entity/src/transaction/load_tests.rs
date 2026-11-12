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

//! Connection admission near pool capacity (`plans/archive/High-Level-Review.md` HL-022), opt-in: streamed database downloads
//! (each window a permit of the transfer limit and one autocommit read), request transactions and agent steps on one
//! pool, with a reserve probe. Statements sleep to stand for database time. Prints, per scenario, the ordinary
//! high-water mark, pool waits, wakeups per returned connection, checkout wait percentiles, how many waiters a later
//! checkout overtook, request latency and reserve availability.
//!
//! `cargo test -p moqui-entity --lib transaction::load_tests -- --ignored --nocapture`

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::{PoolConfig, TransactionManager};
use crate::driver::{DbCancel, DbConnection, DbDriver, DbRow};

const RUN: Duration = Duration::from_millis(2500);
/// A checkout slower than this waited for a returned connection.
const WAITED: Duration = Duration::from_micros(300);

struct Scenario {
    name: &'static str,
    /// Ordinary connections: `max - reserve`.
    ordinary: u32,
    reserve: u32,
    /// The transfer admission limit (`transfer-workers`, or what the capacity check leaves).
    transfer_limit: usize,
    downloads: usize,
    request_workers: usize,
    agent_workers: usize,
}

#[test]
#[ignore = "measurement, about 8 s: cargo test -p moqui-entity --lib transaction::load_tests -- --ignored --nocapture"]
fn admission_near_pool_capacity() {
    // 6 request workers and 2 agent workers hold at most one connection each here (no force-new), so 8 ordinary
    // connections serve them; 16 transfer reads need 16 more.
    let scenarios = [
        Scenario {
            name: "counted: 8 + 16 transfers fit",
            ordinary: 24,
            reserve: 4,
            transfer_limit: 16,
            downloads: 32,
            request_workers: 6,
            agent_workers: 2,
        },
        Scenario {
            name: "uncounted: 16 transfers over 8 + 2 spare",
            ordinary: 10,
            reserve: 4,
            transfer_limit: 16,
            downloads: 32,
            request_workers: 6,
            agent_workers: 2,
        },
        Scenario {
            name: "clamped: 2 transfers in 8 + 2 spare",
            ordinary: 10,
            reserve: 4,
            transfer_limit: 2,
            downloads: 32,
            request_workers: 6,
            agent_workers: 2,
        },
    ];
    for scenario in &scenarios {
        let report = run(scenario);
        println!("\n== {} ==\n{report}", scenario.name);
    }
}

#[derive(Default)]
struct Samples {
    /// (asked at, got at) for every checkout.
    checkouts: Vec<(Instant, Instant)>,
    request_latency: Vec<Duration>,
    agent_latency: Vec<Duration>,
    reserve_waits: Vec<Duration>,
    reserve_failures: u64,
    failures: u64,
}

fn run(scenario: &Scenario) -> String {
    let manager = TransactionManager::new(
        Arc::new(SleepDriver),
        PoolConfig {
            min: 0,
            max: scenario.ordinary + scenario.reserve,
            reserve: scenario.reserve,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: None,
            test_query: None,
            test_after_idle: Duration::from_secs(3600),
        },
    )
    .unwrap();
    let samples = Arc::new(Mutex::new(Samples::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let windows = Arc::new(AtomicU64::new(0));
    let permits = Arc::new(Permits::new(scenario.transfer_limit));
    let mut threads = Vec::new();

    // downloads: a permit, then one autocommit read of a window (4 ms), then the client takes it (3 ms)
    for _ in 0..scenario.downloads {
        let (manager, samples, stop, permits, windows) = (
            manager.clone(),
            Arc::clone(&samples),
            Arc::clone(&stop),
            Arc::clone(&permits),
            Arc::clone(&windows),
        );
        threads.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                permits.acquire();
                let asked = Instant::now();
                let read = manager.with_connection(|conn| {
                    let got = Instant::now();
                    conn.query("window 4", &[])?;
                    Ok(got)
                });
                permits.release();
                record(&samples, asked, read);
                windows.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(3));
            }
        }));
    }
    // request workers: a transaction of three 1 ms statements, then 2 ms until the next request
    for _ in 0..scenario.request_workers {
        let (manager, samples, stop) = (manager.clone(), Arc::clone(&samples), Arc::clone(&stop));
        threads.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let began = Instant::now();
                if let Some(latency) = transaction(&manager, &samples, 3, began) {
                    samples.lock().unwrap().request_latency.push(latency);
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }));
    }
    // agent steps: one short transaction, then 5 ms (a model call holds no connection)
    for _ in 0..scenario.agent_workers {
        let (manager, samples, stop) = (manager.clone(), Arc::clone(&samples), Arc::clone(&stop));
        threads.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let began = Instant::now();
                if let Some(latency) = transaction(&manager, &samples, 1, began) {
                    samples.lock().unwrap().agent_latency.push(latency);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }));
    }
    // the reserve: a probe every 20 ms
    {
        let (manager, samples, stop) = (manager.clone(), Arc::clone(&samples), Arc::clone(&stop));
        threads.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let asked = Instant::now();
                let probe = manager.with_reserve(|conn| conn.query("probe 0", &[]).map(|_| ()));
                let mut samples = samples.lock().unwrap();
                match probe {
                    Ok(()) => samples.reserve_waits.push(asked.elapsed()),
                    Err(_) => samples.reserve_failures += 1,
                }
                drop(samples);
                std::thread::sleep(Duration::from_millis(20));
            }
        }));
    }
    std::thread::sleep(RUN);
    stop.store(true, Ordering::Relaxed);
    for thread in threads {
        thread.join().unwrap();
    }
    let counters = manager.pool_counters();
    let samples = samples.lock().unwrap();
    let waits: Vec<Duration> = samples.checkouts.iter().map(|(asked, got)| *got - *asked).collect();
    let waited: Vec<Duration> = waits.iter().copied().filter(|wait| *wait >= WAITED).collect();
    let (overtaken, overtakes, most) = overtaking(&samples.checkouts);
    let seconds = RUN.as_secs_f64();
    format!(
        "ordinary high-water {} of {} | reserve high-water {} of {}\n\
         checkouts {} | waited {} ({:.1}%) | pool waits {} | wakeups sent {} | waiters woken {} \
         ({:.2} per returned connection, {:.2} per waited checkout)\n\
         checkout wait all: {}\n\
         checkout wait waited: {}\n\
         overtaken waiters {} of {} ({:.1}%), overtakes {}, most past one waiter {}\n\
         request latency: {} | {:.0}/s\n\
         agent step latency: {} | {:.0}/s\n\
         download windows {:.0}/s | reserve probes {} ok, {} failed, {}\n\
         failures {}",
        counters.ordinary_high,
        scenario.ordinary,
        counters.reserve_high,
        scenario.reserve,
        waits.len(),
        waited.len(),
        percent(waited.len(), waits.len()),
        counters.waits,
        counters.notifies,
        counters.woken,
        ratio(counters.woken, counters.notifies),
        ratio(counters.woken, counters.waited_checkouts),
        percentiles(&waits),
        percentiles(&waited),
        overtaken,
        waited.len(),
        percent(overtaken, waited.len()),
        overtakes,
        most,
        percentiles(&samples.request_latency),
        samples.request_latency.len() as f64 / seconds,
        percentiles(&samples.agent_latency),
        samples.agent_latency.len() as f64 / seconds,
        windows.load(Ordering::Relaxed) as f64 / seconds,
        samples.reserve_waits.len(),
        samples.reserve_failures,
        percentiles(&samples.reserve_waits),
        samples.failures,
    )
}

/// A transaction of `statements` 1 ms statements; its latency, or `None` when it failed.
fn transaction(
    manager: &TransactionManager,
    samples: &Mutex<Samples>,
    statements: usize,
    began: Instant,
) -> Option<Duration> {
    manager.begin(None).unwrap();
    let asked = Instant::now();
    let first = manager.with_connection(|conn| {
        let got = Instant::now();
        conn.execute("statement 1", &[])?;
        Ok(got)
    });
    let ok = first.is_ok();
    record(samples, asked, first);
    if !ok {
        let _ = manager.rollback();
        return None;
    }
    for _ in 1..statements {
        manager
            .with_connection(|conn| conn.execute("statement 1", &[]))
            .unwrap();
    }
    manager.commit().unwrap();
    Some(began.elapsed())
}

fn record(samples: &Mutex<Samples>, asked: Instant, result: Result<Instant, Error>) {
    let mut samples = samples.lock().unwrap();
    match result {
        Ok(got) => samples.checkouts.push((asked, got)),
        Err(_) => samples.failures += 1,
    }
}

/// Waited checkouts that a checkout asking later got ahead of: (how many were, overtakes in all, most past one).
fn overtaking(checkouts: &[(Instant, Instant)]) -> (usize, usize, usize) {
    let mut by_ask: Vec<(Instant, Instant)> = checkouts.to_vec();
    by_ask.sort_by_key(|(asked, _)| *asked);
    let (mut overtaken, mut overtakes, mut most) = (0, 0, 0);
    for (index, (asked, got)) in by_ask.iter().enumerate() {
        if *got - *asked < WAITED {
            continue;
        }
        let past = by_ask[index + 1..]
            .iter()
            .take_while(|(later, _)| later < got)
            .filter(|(_, later_got)| later_got < got)
            .count();
        if past > 0 {
            overtaken += 1;
            overtakes += past;
            most = most.max(past);
        }
    }
    (overtaken, overtakes, most)
}

fn percentiles(values: &[Duration]) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    let at = |q: f64| {
        let index = ((sorted.len() - 1) as f64 * q).round() as usize;
        millis(sorted[index])
    };
    format!(
        "p50 {} p90 {} p99 {} max {} ms",
        at(0.5),
        at(0.9),
        at(0.99),
        millis(sorted[sorted.len() - 1])
    )
}

fn millis(value: Duration) -> String {
    format!("{:.2}", value.as_secs_f64() * 1000.0)
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// A counting semaphore: the transfer admission limit.
struct Permits {
    free: Mutex<usize>,
    available: Condvar,
}

impl Permits {
    fn new(count: usize) -> Self {
        Self {
            free: Mutex::new(count),
            available: Condvar::new(),
        }
    }

    fn acquire(&self) {
        let mut free = self.free.lock().unwrap();
        while *free == 0 {
            free = self.available.wait(free).unwrap();
        }
        *free -= 1;
    }

    fn release(&self) {
        *self.free.lock().unwrap() += 1;
        self.available.notify_one();
    }
}

/// Connections whose statements sleep for the milliseconds after the statement's first space.
struct SleepDriver;
struct SleepConn;
struct NoCancel;

impl DbCancel for NoCancel {
    fn cancel(&self) -> Result<(), Error> {
        Ok(())
    }
}

impl DbDriver for SleepDriver {
    fn connect(&self) -> Result<Box<dyn DbConnection>, Error> {
        Ok(Box::new(SleepConn))
    }
}

fn pause(sql: &str) {
    let millis = sql
        .split_once(' ')
        .and_then(|(_, millis)| millis.parse::<u64>().ok())
        .unwrap_or(0);
    if millis > 0 {
        std::thread::sleep(Duration::from_millis(millis));
    }
}

impl DbConnection for SleepConn {
    fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<u64, Error> {
        pause(sql);
        Ok(1)
    }

    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<DbRow>, Error> {
        self.query_once(sql, params)
    }

    fn query_once(&mut self, sql: &str, _params: &[Value]) -> Result<Vec<DbRow>, Error> {
        pause(sql);
        Ok(Vec::new())
    }

    fn open_stream(&mut self, _sql: &str, _params: &[Value]) -> Result<(), Error> {
        Ok(())
    }

    fn next_row(&mut self) -> Result<Option<DbRow>, Error> {
        Ok(None)
    }

    fn close_stream(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn stream_open(&self) -> bool {
        false
    }

    fn begin(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn commit(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn rollback_to_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn release_savepoint(&mut self, _name: &str) -> Result<(), Error> {
        Ok(())
    }

    fn canceller(&self) -> Box<dyn DbCancel> {
        Box::new(NoCancel)
    }
}
