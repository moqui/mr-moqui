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

//! The scheduler thread and the Quartz cron parser. The thread only sleeps and starts passes; a
//! pass is in `jobs`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use moqui_lib::sync::{lock, wait_timeout};

use croner::parser::{CronParser, Seconds};
use croner::Cron;

use super::pool::Shared;
use crate::context::Error;

/// How often the thread looks at the stop flag. A pass runs every `check_interval`.
const SLICE: Duration = Duration::from_millis(250);

/// Java's wait before the first job pass, "to make sure all is loaded and we're past an initial activity burst".
const INITIAL_DELAY: Duration = Duration::from_secs(120);

pub(crate) fn scheduler_loop(shared: Arc<Shared>) {
    let mut clock = Clock::new(Instant::now(), shared.job_runner, shared.check_interval);
    loop {
        {
            let state = lock(&shared.state);
            if state.stop {
                return;
            }
            let (state, _) = wait_timeout(&shared.wake, state, SLICE);
            if state.stop {
                return;
            }
        }
        let (jobs, hooks) = clock.due(Instant::now());
        if !jobs && !hooks {
            continue;
        }
        let _cleanup = super::pool::WorkCleanup::new(shared.factory());
        if jobs {
            super::jobs::run_scheduled(&shared);
        }
        if hooks {
            super::pool::run_tick_hook(&shared);
        }
    }
}

/// When the scheduler runs a job pass and the tick hooks. Job passes are Java's `scheduleAtFixedRate(jobRunner, 120,
/// rate)`: the first after [`INITIAL_DELAY`], then every interval, and none without a job runner. The tick hooks (the
/// cluster heartbeat) are this port's own, so they run every interval from the start.
struct Clock {
    next_jobs: Option<Instant>,
    last_hooks: Instant,
    interval: Duration,
}

impl Clock {
    fn new(start: Instant, job_runner: bool, interval: Duration) -> Self {
        Self {
            next_jobs: job_runner.then(|| start + INITIAL_DELAY),
            last_hooks: start,
            interval,
        }
    }

    /// Whether a job pass and the hooks are due at `now`; each that is is counted as run.
    fn due(&mut self, now: Instant) -> (bool, bool) {
        let jobs = self.next_jobs.is_some_and(|at| now >= at);
        if jobs {
            self.next_jobs = Some(now + self.interval);
        }
        let hooks = now.duration_since(self.last_hooks) >= self.interval;
        if hooks {
            self.last_hooks = now;
        }
        (jobs, hooks)
    }
}

/// Parsed patterns, as Java's `ScheduledJobRunner.cronByExpression`, so a pass does not parse each job's pattern again.
static CRONS: OnceLock<Mutex<HashMap<String, Arc<Cron>>>> = OnceLock::new();

/// More distinct patterns than this clears the map, which only bounds memory if patterns are generated.
const CRON_CACHE_MAX: usize = 1000;

/// [`parse_cron`], parsed once for each pattern. A pattern that does not parse is not kept.
pub(crate) fn cached_cron(pattern: &str) -> Result<Arc<Cron>, Error> {
    let crons = CRONS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(cron) = lock(crons).get(pattern) {
        return Ok(Arc::clone(cron));
    }
    let cron = Arc::new(parse_cron(pattern)?);
    let mut crons = lock(crons);
    if crons.len() >= CRON_CACHE_MAX {
        crons.clear();
    }
    crons.insert(pattern.to_string(), Arc::clone(&cron));
    Ok(cron)
}

/// Quartz syntax: optional seconds, Sunday is day 1, `?` `L` `W` `#` allowed.
pub(crate) fn parse_cron(pattern: &str) -> Result<Cron, Error> {
    CronParser::builder()
        .seconds(Seconds::Optional)
        .alternative_weekdays(true)
        .sloppy_ranges(true)
        .build()
        .parse(pattern)
        .map_err(|err| Error::new(format!("cron pattern: {err}")))
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
