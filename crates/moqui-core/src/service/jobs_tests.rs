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

//! Unit tests of the job run's lock and end (jobs.rs); the scheduler pass's tests are in schedule_tests.rs.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::object::{to_plain_string, Value};

use super::{abandon, timestamp_value, JobRun, RunEnd, JOB_LOCK, JOB_RUN};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};

const PREVIOUS: i64 = 1_700_000_000_000;

/// A run whose lock row is claimed, as the pass leaves it before a worker takes the run.
fn claimed(ctx: &ExecutionContext, facade: &EntityFacade, job: &str, run_id: &str) -> JobRun {
    let mut run = facade.make_value(JOB_RUN).unwrap();
    run.set("jobRunId", Value::string(run_id));
    run.set("jobName", Value::string(job));
    run.create(facade).unwrap();
    let mut lock = facade.make_value(JOB_LOCK).unwrap();
    lock.set("jobName", Value::string(job));
    lock.set("jobRunId", Value::string(run_id));
    lock.set("lastRunTime", super::now_stamp());
    lock.create(facade).unwrap();
    JobRun {
        job_name: job.to_string(),
        service_name: "never#Called".to_string(),
        parameters: BTreeMap::new(),
        timeout: Duration::from_secs(60),
        script_limit: None,
        job_run_id: run_id.to_string(),
        last_run_time: timestamp_value(PREVIOUS),
        delete_when_done: false,
        user_name: None,
        origin: None,
        topic: None,
        description: None,
        error_history_only: false,
        retries: false,
        rules: None,
        claim: None,
        factory: Arc::clone(ctx.factory()),
    }
}

/// A run that waited for a worker while another instance took its lock over does not call its service (an unknown
/// one here, whose call would end the run row with an error): its row goes and the other run keeps the lock.
#[test]
fn a_parked_run_whose_lock_was_taken_over_does_not_call_its_service() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut run = claimed(&ctx, &facade, "ParkedJob", "R-PARKED");
    run.claim = Some(super::LocalClaim::new(ctx.factory(), "R-PARKED"));
    let mut lock = one(&facade, JOB_LOCK, "jobName", "ParkedJob");
    lock.set("jobRunId", Value::string("ELSEWHERE"));
    lock.update(&facade).unwrap();
    super::run_job(run);
    assert_eq!(
        text(&one(&facade, JOB_LOCK, "jobName", "ParkedJob"), "jobRunId"),
        "ELSEWHERE"
    );
    let left = facade
        .find(JOB_RUN)
        .unwrap()
        .eq("jobRunId", Value::string("R-PARKED"))
        .one()
        .unwrap();
    assert!(left.is_none(), "the run did not start");
}

fn one(facade: &EntityFacade, entity: &str, field: &str, id: &str) -> EntityValue {
    facade
        .find(entity)
        .unwrap()
        .eq(field, Value::string(id))
        .one()
        .unwrap()
        .unwrap_or_else(|| panic!("no {entity} {id}"))
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// The lock is free with the run time from before, and the run ended with an error.
fn assert_released_and_failed(facade: &EntityFacade, job: &str, run_id: &str) {
    let lock = one(facade, JOB_LOCK, "jobName", job);
    assert_eq!(text(&lock, "jobRunId"), "");
    assert_eq!(
        super::millis_of(lock.get("lastRunTime").unwrap_or(&Value::Null)),
        Some(PREVIOUS)
    );
    let run = one(facade, JOB_RUN, "jobRunId", run_id);
    assert_eq!(text(&run, "hasError"), "Y");
    assert!(!text(&run, "endTime").is_empty());
}

#[test]
fn a_run_that_unwinds_releases_its_lock_and_records_an_error() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let run = claimed(&ctx, &facade, "PanicJob", "R1");
    let telemetry = crate::telemetry::for_factory(ctx.factory());
    #[cfg(feature = "telemetry")]
    let runs = {
        #[derive(Default)]
        struct Runs(std::sync::Mutex<Vec<String>>);
        impl crate::telemetry::Exporter for Runs {
            fn export(
                &self,
                index: &str,
                documents: Vec<crate::entity::document::Document>,
            ) -> Result<usize, moqui_lib::error::Error> {
                let count = documents.len();
                for document in documents.iter().filter(|_| index == "otel_metrics") {
                    if document.get("name").and_then(Value::as_str) == Some("moqui.job.runs") {
                        self.0.lock().unwrap().push(text_of(document.get("outcome")));
                    }
                }
                Ok(count)
            }
        }
        fn text_of(value: Option<&Value>) -> String {
            value.map(to_plain_string).unwrap_or_default()
        }
        let runs = std::sync::Arc::new(Runs::default());
        telemetry.set_exporter(runs.clone());
        runs
    };
    let unwound = catch_unwind(AssertUnwindSafe(|| {
        let _end = RunEnd {
            facade: &facade,
            job: &run,
            telemetry: &telemetry,
            done: false,
        };
        // A frame the service began and never ended: the records must not join it.
        assert!(facade.begin("moqui.test.TestEntity", None).unwrap());
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string("from-panicked-job"));
        row.create(&facade).unwrap();
        panic!("stand-in service panic");
    }));
    assert!(unwound.is_err());
    assert!(!crate::transaction::thread_in_transaction());
    assert_released_and_failed(&facade, "PanicJob", "R1");
    assert!(text(&one(&facade, JOB_RUN, "jobRunId", "R1"), "errors").contains("panicked"));
    let kept = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("from-panicked-job"))
        .one()
        .unwrap();
    assert!(kept.is_none(), "the panicked job's frame was committed");
    #[cfg(feature = "telemetry")]
    {
        telemetry.flush();
        assert_eq!(
            *runs.0.lock().unwrap(),
            ["panic"],
            "a panicked run counts in moqui.job.runs"
        );
        telemetry.shutdown();
    }
}

/// As Java's `filterByDate` and its paused check: both date ends are inclusive, and a `paused` that is not null or
/// `N`, an empty one too, pauses the job.
#[test]
fn job_dates_are_inclusive_and_an_empty_paused_pauses() {
    let facade = crate::testing::open();
    let mut job = facade.make_value(super::SERVICE_JOB).unwrap();
    assert!(super::paused_ok(&job));
    job.set("paused", Value::string("N"));
    assert!(super::paused_ok(&job));
    for paused in ["", "Y", "T"] {
        job.set("paused", Value::string(paused));
        assert!(!super::paused_ok(&job), "paused {paused:?}");
    }
    job.set("fromDate", timestamp_value(PREVIOUS));
    job.set("thruDate", timestamp_value(PREVIOUS + 1_000));
    assert!(super::within_dates(&job, PREVIOUS));
    assert!(super::within_dates(&job, PREVIOUS + 1_000));
    assert!(!super::within_dates(&job, PREVIOUS - 1));
    assert!(!super::within_dates(&job, PREVIOUS + 1_001));
}

/// A run started by hand holds no lock until `hold#BackupLock` gives it one, which can land between the runner's
/// release and its write of the run's end (a rule on that write stands in for it here): the lock is released again
/// once the end is written, so it does not name an ended run.
#[test]
fn a_lock_taken_while_a_run_ends_is_released_after_its_end() {
    let facade = crate::testing::open_with(
        &crate::testing::framework_dir(),
        crate::testing::Options {
            entity_eca: true,
            data_feed: false,
            tape: None,
        },
    );
    crate::entity::load_feed::add_eca_rules(
        &facade,
        r#"<eecas>
                <eeca id="TestHoldWhileEnding" entity="moqui.service.job.ServiceJobRun" on-update="true" run-before="true">
                    <actions>
                        <entity-make-value entity-name="moqui.service.job.ServiceJobRunLock" value-field="held"/>
                        <script>held.set("jobName", "HeldJob"); held.set("jobRunId", "R-HELD")</script>
                        <entity-update value-field="held"/>
                    </actions>
                </eeca>
            </eecas>"#,
    )
    .unwrap();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let run = claimed(&ctx, &facade, "HeldJob", "R-HELD");
    // the lock is free when the run ends, as for a run the request has not yet given it
    let mut lock = one(&facade, JOB_LOCK, "jobName", "HeldJob");
    lock.set("jobRunId", Value::Null);
    lock.update(&facade).unwrap();
    let outcome = super::RunOutcome {
        has_error: false,
        results: None,
        result_values: Default::default(),
        messages: String::new(),
        errors: String::new(),
    };
    super::finish_run(&facade, &run, &outcome);
    assert!(!text(&one(&facade, JOB_RUN, "jobRunId", "R-HELD"), "endTime").is_empty());
    assert_eq!(text(&one(&facade, JOB_LOCK, "jobName", "HeldJob"), "jobRunId"), "");
}

#[test]
fn a_run_dropped_at_shutdown_releases_its_lock() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let run = {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        claimed(&ctx, &facade, "DroppedJob", "R2")
    };
    abandon(run);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    assert_released_and_failed(&facade, "DroppedJob", "R2");
}
