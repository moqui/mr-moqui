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

//! Recording a load's samples (plans/LoadRunner.md section 5): a user's thread keeps its samples in its own
//! [`Recorder`], with no lock, and merges them into the load's [`Shared`] state at most once a second and when it ends.
//! The shared state has, per name, a histogram, counts by outcome and an error count, and the run's bins.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::histogram::Histogram;
use moqui_lib::sync::lock;
use serde_json::{json, Value};

use super::bins::{Bins, Series};
use super::pace::Pause;
use super::{Kind, Load, ERROR_SAMPLES};

/// How often a user's samples merge into the load, at most.
pub(crate) const MERGE_EVERY: Duration = Duration::from_secs(1);

/// One name's statistics.
pub(crate) struct NameStats {
    pub(crate) kind: Kind,
    pub(crate) histogram: Histogram,
    pub(crate) errors: u64,
    /// Counts by outcome label: an HTTP status, `ok` or `error`.
    pub(crate) outcomes: BTreeMap<String, u64>,
    /// For HTTP: counts by status, 0 for a transport error.
    pub(crate) statuses: BTreeMap<u16, u64>,
    pub(crate) series: Series,
}

/// One scenario's users, and what they did.
#[derive(Clone, Debug, Default)]
pub(crate) struct ScenarioCounts {
    pub(crate) name: String,
    pub(crate) users: u64,
    pub(crate) iterations: u64,
    pub(crate) errors: u64,
    /// A rate scenario, whose result has `dropped`.
    pub(crate) rate: bool,
    /// Its scheduled starts that no user took in time ([`super::schedule`]).
    pub(crate) dropped: u64,
}

/// What a load's users merged so far.
#[derive(Default)]
pub(crate) struct Shared {
    /// The names in the order first merged.
    pub(crate) order: Vec<String>,
    pub(crate) names: HashMap<String, NameStats>,
    /// Every sample but a step's, which would count its requests twice.
    pub(crate) all: Histogram,
    pub(crate) iterations: u64,
    /// Every error: of a request, a throw in the scenario, a user whose setup or thread failed. Not a step's, whose
    /// error is the request's or the throw's inside it.
    pub(crate) errors: u64,
    /// The errors that are requests' (not a step's): the others are throws and users that failed.
    pub(crate) request_errors: u64,
    pub(crate) error_samples: Vec<Value>,
    /// The first user's start of its timed iterations and the last user's end of them.
    pub(crate) first_timed: Option<Instant>,
    pub(crate) last_timed: Option<Instant>,
    pub(crate) bins: Bins,
    pub(crate) scenarios: Vec<ScenarioCounts>,
    /// The users' timed time, and the part of it spent on a request or a pause: the rest is the script's.
    pub(crate) user_time: Duration,
    pub(crate) wait_time: Duration,
}

impl Shared {
    pub(crate) fn sample(&mut self, sample: Value) {
        if self.error_samples.len() < ERROR_SAMPLES {
            self.error_samples.push(sample);
        }
    }

    /// An error outside any request, such as a user whose setup threw.
    pub(crate) fn user_error(&mut self, scenario: usize, user: u64, message: &str) {
        self.errors += 1;
        if let Some(counts) = self.scenarios.get_mut(scenario) {
            counts.errors += 1;
        }
        let name = self.scenarios.get(scenario).map(|counts| counts.name.clone());
        self.sample(json!({ "name": null, "status": null, "scenario": name, "user": user, "message": message }));
    }

    fn add(&mut self, pending: Pending) {
        let Pending {
            name,
            kind,
            outcome,
            status,
            micros,
            second,
            error,
        } = pending;
        if kind != Kind::Step {
            self.all.record(micros);
            self.bins.add(second, micros, error);
            self.request_errors += u64::from(error);
        }
        let stats = match self.names.get_mut(&name) {
            Some(stats) => stats,
            None => {
                self.order.push(name.clone());
                self.names.entry(name).or_insert_with(|| NameStats {
                    kind,
                    histogram: Histogram::new(),
                    errors: 0,
                    outcomes: BTreeMap::new(),
                    statuses: BTreeMap::new(),
                    series: Series::default(),
                })
            }
        };
        stats.histogram.record(micros);
        stats.errors += u64::from(error);
        *stats.outcomes.entry(outcome).or_default() += 1;
        if let Some(status) = status {
            *stats.statuses.entry(status).or_default() += 1;
        }
        stats.series.add(second, micros, error);
    }
}

/// A sample kept on its user's thread until it merges.
pub(crate) struct Pending {
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) outcome: String,
    pub(crate) status: Option<u16>,
    pub(crate) micros: u64,
    /// Seconds from the load's start.
    pub(crate) second: u64,
    pub(crate) error: bool,
}

/// A load user's samples, kept on its own thread so that no request takes a lock to be counted.
pub(crate) struct Recorder {
    pub(crate) load: Arc<Load>,
    pub(crate) scenario: usize,
    pub(crate) user: u64,
    pub(crate) iteration: u64,
    /// The scenario's `pause` before each request.
    pub(crate) pause: Option<Pause>,
    /// The scenario is running: setup and teardown are not timed.
    pub(crate) timed: bool,
    /// The message of the last error counted and thrown (a request's, or a stopped load's refusal), so that the
    /// scenario's throw that carries it is not counted again; a different throw still is.
    pub(crate) counted: Option<String>,
    /// The steps open, innermost last: each one's start, and [`Recorder::paused`] then.
    pub(crate) steps: Vec<(Instant, Duration)>,
    /// Every pause this user waited, timed or not: a step's time leaves them out.
    pub(crate) paused: Duration,
    /// The user waits for the schedule (above a closed scenario's target, or for a rate's next start): not active.
    parked: bool,
    /// In a rate scenario, the current iteration's scheduled start and [`Recorder::paused`] then: the first step it
    /// begins is timed from there, and so is its `<scenario> iteration` sample.
    pub(crate) scheduled: Option<(Instant, Duration)>,
    pub(crate) iteration_start: Option<(Instant, Duration)>,
    /// The current iteration had an error.
    pub(crate) failed: bool,
    /// The second of the oldest sample not merged yet, `u64::MAX` for none: the load keeps that second open.
    hold: Arc<AtomicU64>,
    pending: Vec<Pending>,
    iterations: u64,
    errors: u64,
    started: Option<Instant>,
    ended: Option<Instant>,
    last_merge: Instant,
    /// The time of the timed part spent on requests and pauses.
    pub(crate) waited: Duration,
}

impl Recorder {
    pub(crate) fn new(load: Arc<Load>, scenario: usize, user: u64, pause: Option<Pause>) -> Recorder {
        let hold = Arc::new(AtomicU64::new(u64::MAX));
        lock(&load.holds).push(Arc::clone(&hold));
        Recorder {
            load,
            scenario,
            user,
            iteration: 0,
            pause,
            timed: false,
            counted: None,
            steps: Vec::new(),
            paused: Duration::ZERO,
            parked: false,
            scheduled: None,
            iteration_start: None,
            failed: false,
            hold,
            pending: Vec::new(),
            iterations: 0,
            errors: 0,
            started: None,
            ended: None,
            last_merge: Instant::now(),
            waited: Duration::ZERO,
        }
    }

    /// The scenario's iterations start (`on`) or end.
    pub(crate) fn set_timed(&mut self, on: bool) {
        if on == self.timed {
            return;
        }
        self.timed = on;
        if on {
            self.started.get_or_insert_with(Instant::now);
            self.load.active.fetch_add(1, Ordering::Relaxed);
        } else {
            self.ended = Some(Instant::now());
            if !std::mem::take(&mut self.parked) {
                self.load.active.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// The user waits for its schedule, and is not counted active meanwhile.
    pub(crate) fn park(&mut self) {
        if self.timed && !self.parked {
            self.parked = true;
            self.load.active.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// The user starts an iteration, scheduled at `scheduled` in a rate scenario.
    pub(crate) fn begin_iteration(&mut self, scheduled: Option<Instant>) {
        if std::mem::take(&mut self.parked) {
            self.load.active.fetch_add(1, Ordering::Relaxed);
        }
        self.failed = false;
        let started = scheduled.map(|at| (at.min(Instant::now()), self.paused));
        self.scheduled = started;
        self.iteration_start = started;
    }

    pub(crate) fn push(&mut self, pending: Pending) {
        if self.pending.is_empty() {
            self.hold.store(pending.second, Ordering::Relaxed);
        }
        self.pending.push(pending);
        self.merge_when_due(Duration::ZERO);
    }

    pub(crate) fn count_error(&mut self) {
        self.errors += 1;
        self.failed = true;
    }

    pub(crate) fn count_iteration(&mut self) {
        self.iterations += 1;
        self.merge_when_due(Duration::ZERO);
    }

    /// Merge when the oldest sample kept is of a second gone by, or the last merge is a second old or will be after
    /// `ahead` (a pause about to start, so that its samples do not wait out the pause). Until then the load keeps
    /// the oldest sample's second open ([`Load::tick`]), so a request that takes longer than the bins' lag does not
    /// leave its user's earlier samples out of their bin's percentiles.
    pub(crate) fn merge_when_due(&mut self, ahead: Duration) {
        if self.pending.is_empty() && self.iterations == 0 {
            return;
        }
        let behind = self
            .pending
            .first()
            .is_some_and(|oldest| oldest.second < self.load.started.elapsed().as_secs());
        if behind || self.last_merge.elapsed() + ahead >= MERGE_EVERY {
            self.merge();
        }
    }

    /// Merge into the load what this user has kept.
    pub(crate) fn merge(&mut self) {
        let load = Arc::clone(&self.load);
        {
            let mut shared = lock(&load.shared);
            for pending in self.pending.drain(..) {
                shared.add(pending);
            }
            let (iterations, errors) = (std::mem::take(&mut self.iterations), std::mem::take(&mut self.errors));
            shared.iterations += iterations;
            shared.errors += errors;
            if let Some(counts) = shared.scenarios.get_mut(self.scenario) {
                counts.iterations += iterations;
                counts.errors += errors;
            }
            if let Some(started) = self.started {
                shared.first_timed = Some(shared.first_timed.map_or(started, |first| first.min(started)));
                let ended = self.ended.unwrap_or_else(Instant::now);
                shared.last_timed = Some(shared.last_timed.map_or(ended, |last| last.max(ended)));
            }
            self.hold.store(u64::MAX, Ordering::Relaxed);
        }
        self.last_merge = Instant::now();
        load.tick();
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.set_timed(false);
        self.merge();
        if let (Some(started), Some(ended)) = (self.started, self.ended) {
            let mut shared = lock(&self.load.shared);
            shared.user_time += ended.saturating_duration_since(started);
            shared.wait_time += self.waited;
        }
    }
}
