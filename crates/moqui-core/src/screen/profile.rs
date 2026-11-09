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

//! The render profile (plans/Profiling2.md, Round 53): where one screen render's time goes, by phase.
//!
//! It is off unless `MOQUI_RENDER_PROFILE` names a file. When it is on, each outermost screen render job appends one
//! JSON line to that file: the screen path, the render mode, the total, and for each phase its count and its inclusive
//! and exclusive microseconds. Phases nest on a thread-local stack, so a phase's exclusive time leaves out the phases
//! opened inside it. A unit's exclusive time is the interpreter running the macros, less the host calls it made, and a
//! host call's is its body, less the argument and result conversions and any nested unit.
//!
//! When it is off, each [`span`] is one load of a lazily set static. When it is on, each span costs two clock reads,
//! which adds a few percent to a render with many host calls.

use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use indexmap::IndexMap;

static FILE: LazyLock<Option<Mutex<File>>> = LazyLock::new(|| {
    let path = std::env::var_os("MOQUI_RENDER_PROFILE")?;
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => Some(Mutex::new(file)),
        Err(err) => {
            tracing::error!("Could not open the render profile file {}: {err}", path.to_string_lossy());
            None
        }
    }
});

/// Whether the render profile is on.
#[inline]
pub(crate) fn enabled() -> bool {
    FILE.is_some()
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

#[derive(Default)]
struct Profile {
    depth: u32,
    path: String,
    mode: String,
    started: Option<Instant>,
    stack: Vec<Open>,
    totals: IndexMap<Key, Totals, foldhash::fast::RandomState>,
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

/// Open a phase of the render this thread is in. Outside a render, or with the profile off, it records nothing.
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

/// The render job a thread is in, written out when the outermost one ends.
pub(crate) struct Request(bool);

/// Begin a render job's profile: a nested job (an include) only counts as a phase of the outer one.
pub(crate) fn request(path: &str) -> Request {
    if !enabled() {
        return Request(false);
    }
    PROFILE.with_borrow_mut(|profile| {
        profile.depth += 1;
        if profile.depth == 1 {
            profile.path = path.to_string();
            profile.mode.clear();
            profile.stack.clear();
            profile.totals.clear();
            profile.started = Some(Instant::now());
        }
        profile.open(("phase", if profile.depth == 1 { "job" } else { "job.nested" }));
    });
    Request(true)
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
        let line = PROFILE.with_borrow_mut(|profile| {
            profile.close();
            profile.depth = profile.depth.saturating_sub(1);
            if profile.depth > 0 {
                return None;
            }
            // phases a failure left open are closed as they unwind; any left are dropped
            profile.stack.clear();
            let total_us = profile.started.take().map_or(0.0, |started| started.elapsed().as_secs_f64() * 1e6);
            let phases: serde_json::Map<String, serde_json::Value> = profile
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
            Some(serde_json::json!({
                "path": profile.path,
                "mode": profile.mode,
                "total_us": total_us.round(),
                "phases": phases,
            }))
        });
        if let (Some(line), Some(file)) = (line, FILE.as_ref()) {
            let mut text = line.to_string();
            text.push('\n');
            if let Ok(mut file) = file.lock() {
                if let Err(err) = file.write_all(text.as_bytes()) {
                    tracing::error!("Could not write the render profile: {err}");
                }
            }
        }
    }
}
