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

//! The render and operation profiles (plans/Profiling2.md, Round 53; plans/Profiling3.md M1): where one screen
//! render's or one service call's time goes, by phase, with counts of what it did.
//!
//! Each is off unless its variable names a file: `MOQUI_RENDER_PROFILE` for each outermost screen render job, and
//! `MOQUI_OP_PROFILE` for each outermost service call that is not inside a render (a job, a REST call, a service a
//! test or the command line calls). Each appends one JSON line to its file: the path (the screen, or the service), the
//! render mode, the total, the thread's CPU time over the same span, its waited round trips, counts (bare-name reads
//! and writes, entity field reads, artifacts pushed by type and action), and for each phase its count and its
//! inclusive and exclusive microseconds. Phases nest on a thread-local stack, so a phase's exclusive time leaves out the
//! phases opened inside it. A unit's exclusive time is the interpreter running the macros, less the host calls it made,
//! and a host call's is its body, less the argument and result conversions and any nested unit.
//!
//! When both are off, each [`span`] and [`count`] is one load of a lazily set static. When one is on, each span costs
//! two clock reads, which adds a few percent to a render with many host calls.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use indexmap::IndexMap;

fn open_file(variable: &str) -> Option<Mutex<File>> {
    let path = std::env::var_os(variable)?;
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(Mutex::new(file)),
        Err(err) => {
            tracing::error!("Could not open the profile file {}: {err}", path.to_string_lossy());
            None
        }
    }
}

static RENDER_FILE: LazyLock<Option<Mutex<File>>> = LazyLock::new(|| open_file("MOQUI_RENDER_PROFILE"));
static OP_FILE: LazyLock<Option<Mutex<File>>> = LazyLock::new(|| open_file("MOQUI_OP_PROFILE"));
static ENABLED: LazyLock<bool> = LazyLock::new(|| RENDER_FILE.is_some() || OP_FILE.is_some());

/// Whether a profile is on.
#[inline]
pub(crate) fn enabled() -> bool {
    *ENABLED
}

/// The thread's CPU time in nanoseconds (`CLOCK_THREAD_CPUTIME_ID`). None where there is no such clock.
#[cfg(unix)]
fn thread_cpu_ns() -> Option<u64> {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    let seconds = u64::try_from(time.tv_sec).ok()?;
    let nanos = u64::try_from(time.tv_nsec).ok()?;
    Some(seconds * 1_000_000_000 + nanos)
}

#[cfg(not(unix))]
fn thread_cpu_ns() -> Option<u64> {
    None
}

/// A phase's key: its kind (`phase`, `host`) and its name.
type Key = (&'static str, &'static str);

#[derive(Default)]
struct Totals {
    count: u64,
    inclusive_ns: u64,
    exclusive_ns: u64,
}

struct Open {
    key: Key,
    started: Instant,
    child_ns: u64,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Kind {
    #[default]
    Render,
    Operation,
}

#[derive(Default)]
struct Profile {
    depth: u32,
    kind: Kind,
    path: String,
    mode: String,
    started: Option<Instant>,
    cpu_started: Option<u64>,
    round_trips_started: u64,
    stack: Vec<Open>,
    totals: IndexMap<Key, Totals, foldhash::fast::RandomState>,
    counts: IndexMap<&'static str, u64, foldhash::fast::RandomState>,
    artifacts: IndexMap<String, u64, foldhash::fast::RandomState>,
}

impl Profile {
    fn open(&mut self, key: Key) {
        self.stack.push(Open {
            key,
            started: Instant::now(),
            child_ns: 0,
        });
    }

    fn close(&mut self) {
        let Some(open) = self.stack.pop() else { return };
        let elapsed = u64::try_from(open.started.elapsed().as_nanos()).unwrap_or(u64::MAX);
        let totals = self.totals.entry(open.key).or_default();
        totals.count += 1;
        totals.inclusive_ns += elapsed;
        totals.exclusive_ns += elapsed.saturating_sub(open.child_ns);
        if let Some(parent) = self.stack.last_mut() {
            parent.child_ns += elapsed;
        }
    }

    fn begin(&mut self, kind: Kind, path: &str) {
        self.kind = kind;
        self.path = path.to_string();
        self.mode.clear();
        self.stack.clear();
        self.totals.clear();
        self.counts.clear();
        self.artifacts.clear();
        self.started = Some(Instant::now());
        self.cpu_started = thread_cpu_ns();
        self.round_trips_started = crate::driver::thread_round_trips();
    }

    /// The finished profile's line, and the profile drained for the next.
    fn line(&mut self) -> serde_json::Value {
        // phases a failure left open are closed as they unwind; any left are dropped
        self.stack.clear();
        let total_us = self
            .started
            .take()
            .map_or(0.0, |started| started.elapsed().as_secs_f64() * 1e6);
        let cpu_us = match (self.cpu_started.take(), thread_cpu_ns()) {
            (Some(start), Some(end)) => serde_json::json!((end.saturating_sub(start) as f64 / 1e3).round()),
            _ => serde_json::Value::Null,
        };
        let phases: serde_json::Map<String, serde_json::Value> = self
            .totals
            .drain(..)
            .map(|((kind, name), totals)| {
                (
                    format!("{kind}:{name}"),
                    serde_json::json!({
                        "n": totals.count,
                        "incl_us": (totals.inclusive_ns as f64 / 1e3).round(),
                        "excl_us": (totals.exclusive_ns as f64 / 1e3).round(),
                    }),
                )
            })
            .collect();
        let counts: serde_json::Map<String, serde_json::Value> = self
            .counts
            .drain(..)
            .map(|(name, n)| (name.to_string(), n.into()))
            .collect();
        let artifacts: serde_json::Map<String, serde_json::Value> =
            self.artifacts.drain(..).map(|(name, n)| (name, n.into())).collect();
        serde_json::json!({
            "path": self.path,
            "mode": self.mode,
            "total_us": total_us.round(),
            "cpu_us": cpu_us,
            "round_trips": crate::driver::thread_round_trips().saturating_sub(self.round_trips_started),
            "counts": counts,
            "artifacts": artifacts,
            "phases": phases,
        })
    }
}

thread_local! {
    static PROFILE: RefCell<Profile> = RefCell::new(Profile::default());
}

/// An open phase, closed when dropped.
pub(crate) struct Span(bool);

impl Drop for Span {
    fn drop(&mut self) {
        if self.0 {
            PROFILE.with_borrow_mut(Profile::close);
        }
    }
}

/// Open a phase of the render or operation this thread is in. Outside one, or with the profiles off, it records
/// nothing.
#[inline]
pub(crate) fn span(kind: &'static str, name: &'static str) -> Span {
    if !enabled() {
        return Span(false);
    }
    Span(PROFILE.with_borrow_mut(|profile| {
        if profile.depth == 0 {
            return false;
        }
        profile.open((kind, name));
        true
    }))
}

/// Count one of something in the render or operation this thread is in.
#[inline]
pub(crate) fn count(name: &'static str) {
    if !enabled() {
        return;
    }
    PROFILE.with_borrow_mut(|profile| {
        if profile.depth > 0 {
            *profile.counts.entry(name).or_default() += 1;
        }
    });
}

/// Count an artifact pushed on the artifact stack, by its type and action.
#[inline]
pub(crate) fn count_artifact(artifact_type: &str, action: &str) {
    if !enabled() {
        return;
    }
    PROFILE.with_borrow_mut(|profile| {
        if profile.depth == 0 {
            return;
        }
        let key = format!("{artifact_type}:{action}");
        *profile.artifacts.entry(key).or_default() += 1;
    });
}

/// The render job or operation a thread is in, written out when the outermost one ends.
pub(crate) struct Request(bool);

/// Begin a render job's profile: a nested job (an include) only counts as a phase of the outer one. A render inside an
/// operation is a phase of the operation.
pub(crate) fn request(path: &str) -> Request {
    if !enabled() {
        return Request(false);
    }
    PROFILE.with_borrow_mut(|profile| {
        if profile.depth == 0 {
            if RENDER_FILE.is_none() {
                return Request(false);
            }
            profile.begin(Kind::Render, path);
        }
        profile.depth += 1;
        profile.open(("phase", if profile.depth == 1 { "job" } else { "job.nested" }));
        Request(true)
    })
}

/// Begin a service call's profile: the outermost call outside a render is an operation of its own, and a call inside
/// one is a phase of it.
pub(crate) fn operation(service: &str) -> Request {
    if !enabled() {
        return Request(false);
    }
    PROFILE.with_borrow_mut(|profile| {
        if profile.depth == 0 {
            if OP_FILE.is_none() {
                return Request(false);
            }
            profile.begin(Kind::Operation, service);
        }
        profile.depth += 1;
        profile.open((
            "phase",
            if profile.depth == 1 {
                "service"
            } else {
                "service.nested"
            },
        ));
        Request(true)
    })
}

/// The render mode of the job this thread is in, once it is known.
pub(crate) fn set_mode(mode: &str) {
    if !enabled() {
        return;
    }
    PROFILE.with_borrow_mut(|profile| {
        if profile.depth == 1 && profile.mode.is_empty() {
            profile.mode = mode.to_string();
        }
    });
}

impl Drop for Request {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let finished = PROFILE.with_borrow_mut(|profile| {
            profile.close();
            profile.depth = profile.depth.saturating_sub(1);
            if profile.depth > 0 {
                return None;
            }
            Some((profile.kind, profile.line()))
        });
        let Some((kind, line)) = finished else { return };
        let file = match kind {
            Kind::Render => RENDER_FILE.as_ref(),
            Kind::Operation => OP_FILE.as_ref(),
        };
        if let Some(file) = file {
            let mut text = line.to_string();
            text.push('\n');
            let mut file = moqui_lib::sync::lock(file);
            if let Err(err) = file.write_all(text.as_bytes()) {
                tracing::error!("Could not write the profile: {err}");
            }
        }
    }
}
