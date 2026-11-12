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

//! What the run page's server panel shows for a run on this server (plans/archive/LoadRunner.md §8, L5): the machine's load,
//! the process's CPU share and memory, sampled every few seconds over the run from the status map
//! (`moqui_core::context::status::status_map`, what the old Service Load Runner screen polled), and the Monitor
//! annotations at the run's start and end.
//!
//! For a run in the server the generator is this process too, so the process CPU share is the server's work and the
//! load's together; the result says so (`generator.note`). The server's own request-side numbers are not split out.

use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use serde_json::{json, Value};

/// The samples a run keeps; past it every other one is dropped, so a long run keeps its whole span.
const MAX_SAMPLES: usize = 720;

/// The note on an in-target run's generator block.
pub(crate) const IN_TARGET_NOTE: &str =
    "in-target: the generator is the server's own process, so its CPU share includes the server's work";

/// A run's server samples so far.
pub(crate) struct Samples {
    started: Instant,
    last: Option<(Instant, Duration)>,
    samples: Vec<Value>,
}

impl Samples {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            last: moqui_core::context::resources::process_cpu_time().map(|cpu| (Instant::now(), cpu)),
            samples: Vec::new(),
        }
    }

    /// Take one sample now: seconds from the start, the machine's load and its percent of the cores, the process's
    /// CPU share of the machine since the last sample, its memory in MiB and its threads.
    pub(crate) fn sample(&mut self, factory: &ExecutionContextFactory) {
        let status = moqui_lib::json::to_json(&moqui_lib::object::Value::Map(moqui_core::context::status::status_map(
            factory, false,
        )));
        let number = |group: &str, key: &str| status.get(group).and_then(|group| group.get(key)).cloned();
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let now = Instant::now();
        let cpu = moqui_core::context::resources::process_cpu_time();
        // over too short an interval the clock ticks of /proc/self/stat are too coarse to give a share
        let share = match (self.last, cpu) {
            (Some(before), Some(after)) => moqui_core::context::resources::cpu_share(before, (now, after), cores)
                .map(|share| (share * 1000.0).round() / 1000.0),
            _ => None,
        };
        if let Some(cpu) = cpu {
            self.last = Some((now, cpu));
        }
        self.samples.push(json!({
            "at": (now.duration_since(self.started).as_secs_f64() * 10.0).round() / 10.0,
            "loadPercent": number("Utilization", "LoadPercent"),
            "load": number("System", "Load"),
            "cpu": share,
            "memoryMb": number("Process", "MemoryUsed"),
            "threads": number("Process", "Threads"),
        }));
        if self.samples.len() > MAX_SAMPLES {
            let kept: Vec<Value> = self.samples.iter().step_by(2).cloned().collect();
            self.samples = kept;
        }
    }

    /// `{ samples, cores }` for the result and the snapshot.
    pub(crate) fn to_json(&self) -> Value {
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        json!({ "samples": self.samples, "cores": cores })
    }
}

/// A Monitor annotation (`monitor_annotations`, source `user`) of a load run on this server, as the runtime settings
/// write theirs (moqui-report `settings.rs`): without search (logged at debug), or when it cannot be written (logged
/// as an error), the run goes on.
pub(crate) fn annotate(factory: &ExecutionContextFactory, run_id: &str, text: &str, created_by: &str) {
    let Some(search) = moqui_core::search::installed(factory) else {
        tracing::debug!(run_id, "No search index: a load run's annotation is not written");
        return;
    };
    let resource = moqui_core::telemetry::for_factory(factory).resource();
    let annotation = json!({
        "_id": format!("load-run-{run_id}-{}", jiff::Timestamp::now().as_millisecond()),
        "time": jiff::Timestamp::now().to_string(),
        "source": "user",
        "scope": "service",
        "service.name": resource.service_name,
        "text": moqui_core::telemetry::redact_text(text),
        "tags": ["load-run", run_id],
        "created_by": created_by,
    });
    let moqui_lib::object::Value::Map(document) = moqui_lib::json::from_json(&annotation) else {
        return;
    };
    match search.index_entries("monitor_annotations", vec![document]) {
        Ok(1) => {}
        Ok(_) => tracing::error!(run_id, "A load run's annotation was not indexed"),
        Err(error) => tracing::error!(run_id, %error, "A load run's annotation was not written"),
    }
}

/// The annotation's text: `Load run <label or script>: <what>`.
pub(crate) fn annotation_text(label: &str, script: &str, what: &str) -> String {
    let named = if label.trim().is_empty() { script } else { label.trim() };
    format!("Load run {named}: {what}")
}
