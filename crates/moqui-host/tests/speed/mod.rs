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

//! What the speed tests share: an allocator that counts this thread's allocations, the timed rows, and the table.
//!
//! The counts are of the thread that runs the operation, so a tokio worker or a cache evictor does not show in them.
//! They do not change from run to run the way times do, which makes them the steadier measure of a change.
//!
//! - **allocs/call**: allocations and reallocations. The allocator's work, and what fragments the heap.
//! - **bytes/call**: bytes those asked for, freed or not. The memory traffic ("pressure").
//! - **kept/call**: bytes allocated and not freed on this thread by the end of the batch, a cache or a log that grows.
//! - **sql/call**: statements the entity layer ran (`statement_stats`), on any thread. Begin and commit are not counted.
//! - **js kept/call** (rows with `js` set): bytes this thread's QuickJS engines hold from `malloc` at the end of the
//!   batch less at its start, with no collection between. QuickJS allocates with the C library's `malloc`, so the
//!   columns above do not see it.

#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Counting;

/// Opt-in recording sink for comparable Profiling runs. It measures producer instrumentation,
/// normalization and queue cost; storage latency is covered by backend acceptance tests.
pub struct TelemetryMeasurement {
    telemetry: std::sync::Arc<moqui_core::telemetry::Telemetry>,
    sink: std::sync::Arc<TelemetryVolume>,
    live_before: i64,
}

#[derive(Default)]
struct TelemetryVolume(std::sync::Mutex<std::collections::BTreeMap<String, (usize, usize)>>);

impl moqui_core::telemetry::Exporter for TelemetryVolume {
    fn export(
        &self,
        index: &str,
        documents: Vec<moqui_core::entity::document::Document>,
    ) -> Result<usize, moqui_lib::error::Error> {
        let count = documents.len();
        let bytes = documents
            .into_iter()
            .map(|doc| {
                moqui_lib::json::to_json(&moqui_lib::object::Value::Map(doc))
                    .to_string()
                    .len()
            })
            .sum::<usize>();
        let mut volume = self.0.lock().unwrap();
        let entry = volume.entry(index.to_owned()).or_default();
        entry.0 += count;
        entry.1 += bytes;
        Ok(count)
    }
}

pub fn telemetry_measurement(factory: &moqui_core::context::ExecutionContextFactory) -> Option<TelemetryMeasurement> {
    if std::env::var("MONITOR_SPEED_EXPORT").as_deref() != Ok("true") {
        return None;
    }
    let live_before = process_live();
    let telemetry = moqui_core::telemetry::for_factory(factory);
    let sink = std::sync::Arc::new(TelemetryVolume::default());
    telemetry.set_exporter(sink.clone());
    Some(TelemetryMeasurement {
        telemetry,
        sink,
        live_before,
    })
}

impl Drop for TelemetryMeasurement {
    fn drop(&mut self) {
        self.telemetry.flush();
        let live_after = process_live();
        println!(
            "process Rust live bytes after telemetry flush: before {}, after {}, growth {}",
            self.live_before,
            live_after,
            live_after - self.live_before
        );
        println!(
            "telemetry: {:?}; normalized document volume: {:?}",
            self.telemetry.counters(),
            self.sink.0.lock().unwrap()
        );
        self.telemetry.shutdown();
    }
}

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
    static LIVE: Cell<i64> = const { Cell::new(0) };
    static LIVE_SHARD: Cell<usize> = const { Cell::new(usize::MAX) };
}

// Cross-thread transfers make the row's thread-local `kept` unsuitable as total
// retained memory. Separate cache lines avoid adding producer/exporter contention
// to the very workload this allocation-free counter measures.
#[repr(align(128))]
struct LiveShard(
    std::sync::atomic::AtomicI64,
    std::sync::atomic::AtomicU64,
    std::sync::atomic::AtomicU64,
);
static LIVE_SHARDS: [LiveShard; 128] = [const {
    LiveShard(
        std::sync::atomic::AtomicI64::new(0),
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    )
}; 128];
static PROCESS_ALLOCS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Count cross-thread allocations only in a separate allocation pass. Timed passes
/// retain the existing live-byte accounting without the additional counters.
pub fn record_process_allocations(enabled: bool) {
    PROCESS_ALLOCS.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub fn process_counts() -> Counts {
    let mut counts = Counts::default();
    for shard in &LIVE_SHARDS {
        counts.live += shard.0.load(std::sync::atomic::Ordering::Relaxed);
        counts.allocs += shard.1.load(std::sync::atomic::Ordering::Relaxed);
        counts.bytes += shard.2.load(std::sync::atomic::Ordering::Relaxed);
    }
    counts
}
static NEXT_LIVE_SHARD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn note_process_live(delta: i64, allocation: bool) {
    let _ = LIVE_SHARD.try_with(|slot| {
        let mut index = slot.get();
        if index == usize::MAX {
            index = NEXT_LIVE_SHARD
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                .min(LIVE_SHARDS.len() - 1);
            slot.set(index);
        }
        LIVE_SHARDS[index]
            .0
            .fetch_add(delta, std::sync::atomic::Ordering::Relaxed);
        if allocation && PROCESS_ALLOCS.load(std::sync::atomic::Ordering::Relaxed) {
            LIVE_SHARDS[index].1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            LIVE_SHARDS[index]
                .2
                .fetch_add(delta.unsigned_abs(), std::sync::atomic::Ordering::Relaxed);
        }
    });
}

fn process_live() -> i64 {
    LIVE_SHARDS
        .iter()
        .map(|shard| shard.0.load(std::sync::atomic::Ordering::Relaxed))
        .sum()
}

fn note_alloc(size: usize) {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    note_process_live(size, true);
    let _ = ALLOCS.try_with(|count| count.set(count.get() + 1));
    let _ = BYTES.try_with(|bytes| bytes.set(bytes.get() + size.unsigned_abs()));
    let _ = LIVE.try_with(|live| live.set(live.get() + size));
}

fn note_free(size: usize) {
    let size = i64::try_from(size).unwrap_or(i64::MAX);
    note_process_live(-size, false);
    let _ = LIVE.try_with(|live| live.set(live.get() - size));
}

// SAFETY: every call goes to `System` with the caller's arguments. The counters are const-initialized thread locals
// without destructors, so touching them does not allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note_free(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_free(layout.size());
        note_alloc(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[derive(Clone, Copy, Default)]
pub struct Counts {
    pub allocs: u64,
    pub bytes: u64,
    pub live: i64,
}

pub fn counts() -> Counts {
    Counts {
        allocs: ALLOCS.with(Cell::get),
        bytes: BYTES.with(Cell::get),
        live: LIVE.with(Cell::get),
    }
}

pub struct Measured {
    pub elapsed: Duration,
    pub allocs: u64,
    pub bytes: u64,
    pub kept: i64,
    pub statements: u64,
    pub js_kept: i64,
}

/// Time `body` and count what this thread allocated in it.
pub fn measure<T>(body: impl FnOnce() -> T) -> (Measured, T) {
    let (statements_before, _) = moqui_core::entity::statement_stats();
    let js_before = moqui_core::script::engine_memory(false).malloc_size;
    let before = counts();
    let started = Instant::now();
    let out = body();
    let elapsed = started.elapsed();
    let after = counts();
    let js_after = moqui_core::script::engine_memory(false).malloc_size;
    let (statements_after, _) = moqui_core::entity::statement_stats();
    let measured = Measured {
        elapsed,
        allocs: after.allocs - before.allocs,
        bytes: after.bytes - before.bytes,
        kept: after.live - before.live,
        statements: statements_after - statements_before,
        js_kept: js_after - js_before,
    };
    (measured, out)
}

pub struct Row {
    pub operation: String,
    pub calls: u64,
    pub batches: Vec<Measured>,
}

/// The rows of one mode. A warmup batch is run with `record` off, so its first-call costs are not kept.
#[derive(Default)]
pub struct Rows {
    pub record: bool,
    /// Print the js kept column.
    pub js: bool,
    pub rows: Vec<Row>,
}

impl Rows {
    /// Run `body`, which makes `calls` calls, and keep its measure under `operation`.
    pub fn run<T>(&mut self, operation: &str, calls: usize, body: impl FnOnce() -> T) -> T {
        let (measured, out) = measure(body);
        self.note(operation, calls, measured);
        out
    }

    /// Run `body` on `threads` threads at once, each making `calls` calls (`body` is given the thread's index), and
    /// keep their measure under `operation` as `threads * calls` calls. The time is the slowest thread's, from a
    /// common start; the allocation columns are the threads' sums, each counted on its own thread; the statements are
    /// those of the whole run.
    pub fn run_threads(&mut self, operation: &str, threads: usize, calls: usize, body: impl Fn(usize) + Sync) {
        let barrier = std::sync::Barrier::new(threads);
        let (statements_before, _) = moqui_core::entity::statement_stats();
        let each: Vec<Measured> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|thread| {
                    let barrier = &barrier;
                    let body = &body;
                    scope.spawn(move || {
                        barrier.wait();
                        measure(|| body(thread)).0
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("speed thread"))
                .collect()
        });
        let (statements_after, _) = moqui_core::entity::statement_stats();
        let measured = Measured {
            elapsed: each.iter().map(|one| one.elapsed).max().unwrap_or_default(),
            allocs: each.iter().map(|one| one.allocs).sum(),
            bytes: each.iter().map(|one| one.bytes).sum(),
            kept: each.iter().map(|one| one.kept).sum(),
            statements: statements_after - statements_before,
            js_kept: each.iter().map(|one| one.js_kept).sum(),
        };
        self.note(operation, threads * calls, measured);
    }

    pub fn note(&mut self, operation: &str, calls: usize, measured: Measured) {
        if !self.record {
            return;
        }
        if let Some(row) = self.rows.iter_mut().find(|row| row.operation == operation) {
            row.batches.push(measured);
            return;
        }
        self.rows.push(Row {
            operation: operation.to_string(),
            calls: u64::try_from(calls).unwrap_or(u64::MAX),
            batches: vec![measured],
        });
    }

    /// Per call, from the upper median batch by time. The allocation columns are of that batch too.
    pub fn print(&self) {
        let js = if self.js {
            format!(" {:>8}", "js kept")
        } else {
            String::new()
        };
        println!(
            "  {:<46} {:>7} {:>10} {:>9} {:>10} {:>9} {:>10} {:>9} {:>6}{js} {:>17}",
            "operation", "calls", "median ms", "us/call", "calls/s", "allocs", "bytes", "kept", "sql", "min-max ms"
        );
        for row in &self.rows {
            let mut order: Vec<usize> = (0..row.batches.len()).collect();
            order.sort_by_key(|index| row.batches[*index].elapsed);
            let median = &row.batches[order[order.len() / 2]];
            let calls = row.calls.max(1) as f64;
            let seconds = median.elapsed.as_secs_f64();
            let per_second = if seconds == 0.0 { 0.0 } else { calls / seconds };
            let fastest = row.batches[order[0]].elapsed.as_secs_f64() * 1.0e3;
            let slowest = row.batches[order[order.len() - 1]].elapsed.as_secs_f64() * 1.0e3;
            let js = if self.js {
                format!(" {:>8.0}", median.js_kept as f64 / calls)
            } else {
                String::new()
            };
            println!(
                "  {:<46} {:>7} {:>10.3} {:>9.2} {:>10.0} {:>9.1} {:>10.0} {:>9.0} {:>6.2}{js} {:>8.3}-{:<8.3}",
                row.operation,
                row.calls,
                seconds * 1.0e3,
                seconds * 1.0e6 / calls,
                per_second,
                median.allocs as f64 / calls,
                median.bytes as f64 / calls,
                median.kept as f64 / calls,
                median.statements as f64 / calls,
                fastest,
                slowest,
            );
        }
    }
}

pub fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|text| text.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// Removes the database directory when the test ends, also on a panic.
pub struct DirGuard(pub PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Under `/tmp`, not `TMPDIR`: the Postgres socket path has to fit a short limit.
pub fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    PathBuf::from("/tmp").join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

pub fn warn_if_debug(name: &str) {
    if cfg!(debug_assertions) {
        eprintln!(
            "{name}: this binary has debug assertions, so it is not a release profile. \
             The numbers are not comparable to ./moq build."
        );
    }
}

/// The database a test runs on: the embedded cluster in `dir`, or, when `SPEED_PG_HOST` is set, a new database on that
/// server, dropped when this is. `SPEED_PG_PORT` (5432), `SPEED_PG_USER` and `SPEED_PG_PASSWORD` (`moqui`),
/// `SPEED_PG_DATABASE` (`postgres`, the one connected to for `CREATE DATABASE`) and `SPEED_PG_SSLMODE` (an `sslmode`;
/// unset or `disable` is no TLS) with `SPEED_PG_SSLROOTCERT` name the rest. The role needs `CREATEDB`.
#[cfg(all(unix, feature = "embedded-postgres"))]
pub enum SpeedDb {
    Embedded(moqui_host::db::embedded::EmbeddedCluster),
    External {
        admin: moqui_host::db::postgres::PostgresDriver,
        name: String,
    },
}

#[cfg(all(unix, feature = "embedded-postgres"))]
impl SpeedDb {
    pub fn start(dir: &std::path::Path, max_connections: u32) -> (Self, moqui_host::db::postgres::PostgresDriver) {
        use moqui_core::driver::DbDriver;
        use moqui_host::db::entity_db::postgres_server_driver;

        let Some(host) = std::env::var("SPEED_PG_HOST").ok().filter(|host| !host.is_empty()) else {
            let (cluster, driver) = moqui_host::db::embedded::EmbeddedCluster::start(dir, max_connections)
                .expect("start embedded Postgres");
            println!("database: embedded Postgres, Unix socket");
            return (Self::Embedded(cluster), driver);
        };
        let var = |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_string());
        let port: u16 = var("SPEED_PG_PORT", "5432").parse().expect("SPEED_PG_PORT");
        let user = var("SPEED_PG_USER", "moqui");
        let password = var("SPEED_PG_PASSWORD", "moqui");
        let mode = var("SPEED_PG_SSLMODE", "disable");
        let tls = moqui_core::entity::datasource::tls_wanted(&mode).then(|| {
            moqui_host::db::pg_tls::TlsOptions::new(
                moqui_core::entity::datasource::tls_verify(&mode),
                &var("SPEED_PG_SSLROOTCERT", ""),
            )
        });
        let driver = |database: &str| {
            postgres_server_driver(&user, Some(&password), database, &host, port, tls.as_ref())
                .expect("postgres driver")
        };
        let admin = driver(&var("SPEED_PG_DATABASE", "postgres"));
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0);
        let name = format!("speed_{}_{stamp}", std::process::id());
        admin
            .connect()
            .and_then(|mut conn| conn.execute(&format!("CREATE DATABASE {name}"), &[]))
            .expect("create speed database");
        let shown = if tls.is_some() {
            format!(" sslmode {mode}")
        } else {
            String::new()
        };
        println!("database: external Postgres {host}:{port}{shown}, database {name}");
        let target = driver(&name);
        (Self::External { admin, name }, target)
    }
}

#[cfg(all(unix, feature = "embedded-postgres"))]
impl Drop for SpeedDb {
    fn drop(&mut self) {
        use moqui_core::driver::DbDriver;

        if let Self::External { admin, name } = self {
            let dropped = admin
                .connect()
                .and_then(|mut conn| conn.execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"), &[]));
            if let Err(err) = dropped {
                eprintln!("drop speed database {name}: {err}");
            }
        }
    }
}
