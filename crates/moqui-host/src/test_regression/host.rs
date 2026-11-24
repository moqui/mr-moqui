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

//! The spec host of `moqui test-regression`: the client host of `moqui test-client` (sessions, HTTP, the browser),
//! with `app` on top (prelude.js), and a recording of what each case did. After each case the host takes the
//! database's snapshot, keeps the rows that changed, and moves the data clock on.

use std::sync::{Arc, Mutex};

use moqui_devkit::testkit::{CaseExtras, CaseResult, Outcome, SpecHost};
use moqui_lib::sync::lock;
use serde_json::{json, Value};

use super::engine::Clock;
use super::record::{CaseRecord, Recorder, Recording};
use super::CLOCK_STEP_MS;

const PRELUDE: &str = include_str!("prelude.js");

/// The name of the record of what a spec changed after its last case (its `cleanupSpec`).
pub(crate) const AFTER_CASES: &str = "(after the cases)";

pub(crate) struct RegressionHost {
    client: Arc<dyn SpecHost>,
    prelude: String,
    recorder: Mutex<Recorder>,
    clock: Clock,
    spec: Mutex<String>,
    /// The calls of the case running, kept until its end.
    calls: Mutex<Vec<Value>>,
}

impl RegressionHost {
    pub(crate) fn new(client: Arc<dyn SpecHost>, recorder: Recorder, clock: Clock) -> Self {
        let prelude = format!("{}\n{PRELUDE}", client.prelude());
        Self {
            client,
            prelude,
            recorder: Mutex::new(recorder),
            clock,
            spec: Mutex::new(String::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn begin_spec(&self, spec: &str) {
        *lock(&self.spec) = spec.to_string();
    }

    /// The outcome of a case, once the runner has it. A case that did not run has no record yet: it gets one with no
    /// calls and no changes.
    pub(crate) fn case_finished(&self, case: &CaseResult) {
        let outcome = match &case.outcome {
            Outcome::Passed => "passed".to_string(),
            Outcome::Failed(failure) => format!("failed: {}", failure.message.lines().next().unwrap_or("")),
            Outcome::Ignored(reason) => format!("ignored: {reason}"),
            Outcome::Skipped(reason) => format!("skipped: {reason}"),
        };
        let spec = lock(&self.spec).clone();
        let mut recorder = lock(&self.recorder);
        let cases = &mut recorder.recording.cases;
        match cases.iter_mut().rev().find(|record| record.spec == spec && record.case == case.name) {
            Some(record) => record.outcome = outcome,
            None => cases.push(CaseRecord {
                spec,
                case: case.name.clone(),
                outcome,
                clock_ms: self.clock.now(),
                ..CaseRecord::default()
            }),
        }
    }

    pub(crate) fn spec_failed(&self, spec: &str, message: &str) {
        let line = message.lines().next().unwrap_or("");
        lock(&self.recorder).recording.problems.push(format!("{spec}: setup failed: {line}"));
    }

    /// Close the case running: its calls, the rows changed since the last case, and the clock moved on. The step after
    /// the last case of a spec is kept even when it changed nothing, so both targets have the same steps and clocks:
    /// a row the engine writes in the background can land in it on one target only.
    fn close_case(&self, spec: &str, case: &str, outcome: &str) {
        let calls = std::mem::take(&mut *lock(&self.calls));
        let mut recorder = lock(&self.recorder);
        let changes = match recorder.changes() {
            Ok(changes) => changes,
            Err(err) => {
                recorder.recording.problems.push(format!("{spec} / {case}: the snapshot failed: {err}"));
                Vec::new()
            }
        };
        recorder.recording.cases.push(CaseRecord {
            spec: spec.to_string(),
            case: case.to_string(),
            outcome: outcome.to_string(),
            calls,
            changes,
            clock_ms: self.clock.now(),
        });
        drop(recorder);
        if let Err(err) = self.clock.advance(CLOCK_STEP_MS) {
            lock(&self.recorder).recording.problems.push(err.to_string());
        }
    }

    pub(crate) fn finish(&self) -> Recording {
        std::mem::take(&mut lock(&self.recorder).recording)
    }

    fn own_call(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "regression.config" => Ok(json!({
                "user": std::env::var("MOQUI_TEST_REGRESSION_USER").unwrap_or_else(|_| "john.doe".to_string()),
                "password": std::env::var("MOQUI_TEST_REGRESSION_PASSWORD").unwrap_or_else(|_| "moqui".to_string()),
                "clockMs": self.clock.now(),
            })),
            "regression.record" => {
                lock(&self.calls).push(args.clone());
                Ok(Value::Null)
            }
            other => Err(format!("{other} is not a call of the regression host")),
        }
    }
}

impl SpecHost for RegressionHost {
    fn prelude(&self) -> &str {
        &self.prelude
    }

    fn call(&self, name: &str, args: &str) -> Result<String, String> {
        if name.starts_with("regression.") {
            let args: Value =
                serde_json::from_str(args).map_err(|err| format!("{name}: the arguments did not read: {err}"))?;
            return self.own_call(name, &args).map(|value| value.to_string());
        }
        self.client.call(name, args)
    }

    fn after_case(&self, case: &str, failed: bool, allow_problems: bool) -> CaseExtras {
        let extras = self.client.after_case(case, failed, allow_problems);
        let spec = lock(&self.spec).clone();
        self.close_case(&spec, case, if failed { "failed" } else { "passed" });
        extras
    }

    fn after_spec(&self) {
        let spec = lock(&self.spec).clone();
        self.close_case(&spec, AFTER_CASES, "passed");
        self.client.after_spec();
    }
}
