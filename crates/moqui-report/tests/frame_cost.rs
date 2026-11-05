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

//! The cost of the JSON round trip between composed report queries (plans/High-Level-Review.md HL-016), opt-in:
//! `cargo test -p moqui-report --test frame_cost -- --ignored --nocapture`.
//!
//! A composed dashboard panel of 4 queries, each compared with the previous period, gives its parent 8 child results.
//! Each child turns its frames into JSON (`execute_query_inner`'s result) and the parent turns them back into frames
//! before it compares and transforms them, then the top turns the result into JSON once. Three ways are measured on
//! the same frames, at growing sizes: the child frames copied before they are read back (as before), taken (as now),
//! and kept as frames with no round trip (an owned internal result, not built). Each line gives the time, the bytes
//! allocated and the peak bytes live above the start, for the whole panel: the round trip, `compare_previous` and the
//! final serialization.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use moqui_report::frame::{compare_previous, Frame};
use serde_json::{json, Value as Json};

struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is passed to the system allocator unchanged; the counters only observe it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        // SAFETY: the caller's layout is passed through as it came.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the pointer came from `alloc` above with this layout.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

const QUERIES: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Way {
    Copied,
    Taken,
    Owned,
}

fn frames(rows: usize, offset: i64) -> Vec<Frame> {
    let rows: Vec<BTreeMap<String, Json>> = (0..rows)
        .map(|n| {
            let n = i64::try_from(n).unwrap_or(i64::MAX);
            BTreeMap::from([
                ("time".to_string(), json!(1_700_000_000_000_i64 + n * 60_000 + offset)),
                ("service".to_string(), json!(format!("service-{}", n % 16))),
                ("count".to_string(), json!(n % 97)),
                ("sum".to_string(), json!(n as f64 * 1.25)),
                ("p95".to_string(), json!(n as f64 * 0.01)),
            ])
        })
        .collect();
    vec![Frame::from_rows(&rows)]
}

/// One child result as the parent gets it: its frames in JSON, and what else a result carries.
fn child(frames: Vec<Frame>) -> Json {
    json!({"frames":frames,"notices":[],"inspection":[],"range":{"from":"a","thru":"b"},"sourceCount":1})
}

fn read_back(result: &mut Json, way: Way) -> Vec<Frame> {
    match way {
        Way::Copied => result["frames"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|frame| serde_json::from_value::<Frame>(frame.clone()).unwrap())
            .collect(),
        _ => match result.get_mut("frames").map(Json::take) {
            Some(Json::Array(frames)) => frames
                .into_iter()
                .map(|frame| serde_json::from_value::<Frame>(frame).unwrap())
                .collect(),
            _ => Vec::new(),
        },
    }
}

/// The panel: per query the current and previous branches, compared; then all of it as JSON.
fn panel(rows: usize, way: Way) -> (Duration, usize, usize) {
    let inputs: Vec<(Vec<Frame>, Vec<Frame>)> = (0..QUERIES)
        .map(|_| (frames(rows, 0), frames(rows, -3_600_000)))
        .collect();
    let keys = ["time".to_string(), "service".to_string()];
    let (allocated, live) = (ALLOCATED.load(Ordering::Relaxed), LIVE.load(Ordering::Relaxed));
    PEAK.store(live, Ordering::Relaxed);
    let started = Instant::now();
    let mut compared = Vec::new();
    for (current, previous) in inputs {
        let (current, previous) = if way == Way::Owned {
            (current, previous)
        } else {
            let (mut current, mut previous) = (child(current), child(previous));
            (read_back(&mut current, way), read_back(&mut previous, way))
        };
        compared.extend(compare_previous(current, previous, 3_600_000, Some(&keys)).unwrap());
    }
    let out = json!({"frames":compared});
    let elapsed = started.elapsed();
    let peak = PEAK.load(Ordering::Relaxed).saturating_sub(live);
    let allocated = ALLOCATED.load(Ordering::Relaxed) - allocated;
    drop(out);
    (elapsed, allocated, peak)
}

#[test]
#[ignore = "a measurement: run it with --ignored --nocapture"]
fn the_json_round_trip_of_composed_report_frames() {
    println!("rows per frame | way | ms | MB allocated | MB peak");
    // 10,000 rows is the service-source cap; a larger join is refused by compare_previous
    for rows in [100, 1_000, 10_000] {
        for (name, way) in [("copied", Way::Copied), ("taken", Way::Taken), ("owned", Way::Owned)] {
            // the best of three, so a cold first run does not count
            let best = (0..3)
                .map(|_| panel(rows, way))
                .min_by_key(|(elapsed, _, _)| *elapsed)
                .unwrap_or_default();
            let mb = |bytes: usize| bytes as f64 / 1_048_576.0;
            println!(
                "{rows:>6} | {name:<6} | {:>8.1} | {:>8.1} | {:>7.1}",
                best.0.as_secs_f64() * 1000.0,
                mb(best.1),
                mb(best.2)
            );
        }
    }
}
