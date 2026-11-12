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

//! Opt-in measurement of the browser SQLite stream (plans/archive/High-Level-Review.md HL-003): peak heap and time to the first
//! row of an `open_stream` over growing tables, beside a `LIMIT 1` query for the cost of one row. It runs natively, on
//! the same driver code the wasm build links; the browser's own worker memory adds the emscripten heap around it.
//! Its own test binary, so the counting allocator sees only this. Run:
//! `cargo test -p moqui-browser --test sqlite_stream_measure -- --ignored --nocapture`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use moqui_browser::sqlite::SqliteDriver;
use moqui_core::driver::DbConnection;
use moqui_lib::object::Value;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: each method forwards to `System` with the caller's layout and pointer unchanged; the counters only add and
// subtract the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract for `alloc` is passed on to `System`.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `alloc` above with this layout.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// The heap growth over `LIVE` at the start of `body`, at its peak, and what `body` returns.
fn peak_during<T>(body: impl FnOnce() -> T) -> (usize, T) {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let out = body();
    (PEAK.load(Ordering::Relaxed).saturating_sub(base), out)
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn browser_sqlite_stream_peak_memory_and_time_to_first_row() {
    let payload = "x".repeat(200);
    println!("rows      first-row(ms)  stream-peak(KiB)  full-read(ms)  limit-1-peak(KiB)");
    for rows in [1_000_usize, 10_000, 100_000, 300_000] {
        let driver = SqliteDriver::memory().unwrap();
        let mut conn = driver.connect_sqlite();
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, body TEXT)", &[])
            .unwrap();
        conn.begin().unwrap();
        for id in 0..rows {
            conn.execute(
                "INSERT INTO t (id, body) VALUES (?, ?)",
                &[Value::I64(id as i64), Value::string(payload.as_str())],
            )
            .unwrap();
        }
        conn.commit().unwrap();
        let started = Instant::now();
        let (stream_peak, first) = peak_during(|| {
            conn.open_stream("SELECT id, body FROM t ORDER BY id", &[]).unwrap();
            let first = conn.next_row().unwrap();
            (started.elapsed(), first.is_some())
        });
        assert!(first.1);
        let mut read = 1;
        while conn.next_row().unwrap().is_some() {
            read += 1;
        }
        let full = started.elapsed();
        conn.close_stream().unwrap();
        assert_eq!(read, rows);
        let (limit_peak, _) = peak_during(|| conn.query("SELECT id, body FROM t ORDER BY id LIMIT 1", &[]).unwrap());
        println!(
            "{rows:<9} {:>13.2}  {:>16}  {:>13.2}  {:>17}",
            first.0.as_secs_f64() * 1000.0,
            stream_peak / 1024,
            full.as_secs_f64() * 1000.0,
            limit_peak / 1024
        );
    }
}
