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

use super::*;
use crate::context::ExecutionContextFactory;

fn create_row(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn pending(facade: &EntityFacade, id: &str, document_id: &str) {
    create_row(
        facade,
        PENDING,
        &[
            ("dataFeedPendingId", id),
            ("dataDocumentId", document_id),
            ("primaryKey", "F1"),
            ("operation", "index"),
        ],
    );
}

#[test]
fn the_drain_pages_past_rows_that_stay() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    // GOOD assembles (its record is gone, so it is a delete with no feed to tell); BAD has no
    // definition, so its delivery fails; HELD goes to search, which is not configured.
    create_row(
        &facade,
        "moqui.entity.document.DataDocument",
        &[("dataDocumentId", "GOOD"), ("primaryEntityName", "moqui.test.Foo")],
    );
    create_row(
        &facade,
        "moqui.entity.feed.DataFeed",
        &[("dataFeedId", "SEARCH"), ("dataFeedTypeEnumId", REALTIME_PUSH)],
    );
    create_row(
        &facade,
        "moqui.entity.feed.DataFeedDocument",
        &[("dataFeedId", "SEARCH"), ("dataDocumentId", "HELD")],
    );
    pending(&facade, "A0000", "HELD");
    let bad = usize::try_from(DRAIN_BATCH).unwrap() + 1;
    for index in 0..bad {
        pending(&facade, &format!("P{index:05}"), "BAD");
    }
    pending(&facade, "Q00000", "GOOD");

    let report = process_pending(None, DEFAULT_STUCK_AFTER).unwrap();
    assert_eq!(report.rows_done, 1, "{report:?}");
    assert_eq!(report.documents_deleted, 1, "{report:?}");
    assert_eq!(report.rows_left, bad + 1, "{report:?}");
    assert_eq!(report.rows_held, 1, "{report:?}");
    assert_eq!(report.rows_pending, bad + 1, "{report:?}");
    assert!(report.oldest_pending_seconds.is_some(), "{report:?}");
    // The rows that fail are new: nothing is stuck yet.
    assert_eq!(report.rows_stuck, 0, "{report:?}");
    assert_eq!(report.stuck(DEFAULT_STUCK_AFTER), None);
    let left = facade.find(PENDING).unwrap().count().unwrap();
    assert_eq!(left, i64::try_from(bad + 1).unwrap());
    let good = facade
        .find(PENDING)
        .unwrap()
        .eq("dataFeedPendingId", Value::string("Q00000"))
        .one()
        .unwrap();
    assert!(good.is_none());

    // A failing row made two hours ago is past the hour: the drain still pages past it and keeps it, and reports it.
    pending_since(&facade, "P99999", "BAD", 2 * 60 * 60);
    let report = process_pending(None, DEFAULT_STUCK_AFTER).unwrap();
    assert_eq!(report.rows_left, bad + 2, "{report:?}");
    assert_eq!(report.rows_stuck, 1, "{report:?}");
    assert_eq!(report.stuck_documents, ["BAD"], "{report:?}");
    assert!(report.oldest_pending_seconds.unwrap() >= 2 * 60 * 60, "{report:?}");
    let error = report.stuck(DEFAULT_STUCK_AFTER).unwrap();
    assert!(error.contains("1 data feed rows of BAD"), "{error}");
    // A held row is not stuck however old: search being down is the gauge's to show, not a failed delivery.
    pending_since(&facade, "A0001", "HELD", 2 * 60 * 60);
    assert_eq!(process_pending(None, DEFAULT_STUCK_AFTER).unwrap().rows_stuck, 1);
}

/// A pending row of `document_id` made `seconds` ago.
fn pending_since(facade: &EntityFacade, id: &str, document_id: &str, seconds: i64) {
    let mut row = facade.make_value(PENDING).unwrap();
    row.set("dataFeedPendingId", Value::string(id));
    row.set("dataDocumentId", Value::string(document_id));
    row.set("primaryKey", Value::string("F1"));
    row.set("operation", Value::string("index"));
    row.set(
        "lastUpdatedStamp",
        Value::Timestamp(Timestamp::now() - jiff::SignedDuration::from_secs(seconds)),
    );
    row.create(facade).unwrap();
}

/// The drain job: a run with a row that keeps failing past `stuckMinutes` is an error, so its run row is kept although
/// the job keeps only error history; once that row is gone a run succeeds and leaves no row.
#[test]
fn a_drain_with_a_stuck_row_is_a_kept_job_error() {
    // The framework directory, so the drain service is found by name as a server finds it.
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::pool::ServicePools::inline(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    create_row(
        &facade,
        "moqui.service.job.ServiceJob",
        &[
            ("jobName", "DRAIN"),
            ("serviceName", "org.moqui.EntityServices.process#DataFeedPending"),
            ("errorHistoryOnly", "Y"),
        ],
    );
    pending_since(&facade, "OLD01", "BAD", 2 * 60 * 60);
    pending(&facade, "NEW01", "BAD");
    let parameters = BTreeMap::from([("stuckMinutes".to_string(), Value::string("60"))]);
    let run_id = crate::service::run_job_now("DRAIN", &parameters).unwrap();
    ctx.factory().run_pending();
    let run = facade
        .find("moqui.service.job.ServiceJobRun")
        .unwrap()
        .eq("jobRunId", Value::string(&run_id))
        .one()
        .unwrap()
        .expect("the failed drain keeps its run row");
    let text = |name: &str| run.get(name).map(to_plain_string).unwrap_or_default();
    assert_eq!(text("hasError"), "Y");
    assert!(text("errors").contains("1 data feed rows of BAD"), "{}", text("errors"));
    // Both rows stay pending: at-least-once delivery.
    assert_eq!(facade.find(PENDING).unwrap().count().unwrap(), 2);
    ctx.message().clear();

    let mut gone = facade.make_value(PENDING).unwrap();
    gone.set("dataFeedPendingId", Value::string("OLD01"));
    gone.delete(&facade).unwrap();
    let run_id = crate::service::run_job_now("DRAIN", &parameters).unwrap();
    ctx.factory().run_pending();
    let run = facade
        .find("moqui.service.job.ServiceJobRun")
        .unwrap()
        .eq("jobRunId", Value::string(&run_id))
        .one()
        .unwrap();
    assert!(run.is_none(), "a drain with only new failures is a success: {run:?}");
}

/// Held rows are logged when first held and then at most once an hour; a drain with none held says so once.
#[test]
fn held_rows_are_logged_without_flooding() {
    let gauges = FeedGauges::default();
    let start = Instant::now();
    gauges.log_held(3, start);
    let first = *moqui_lib::sync::lock(&gauges.held_logged);
    assert_eq!(first, Some(start));
    gauges.log_held(3, start + Duration::from_secs(60));
    assert_eq!(
        *moqui_lib::sync::lock(&gauges.held_logged),
        Some(start),
        "logged again within the hour"
    );
    let later = start + HELD_LOG_EVERY;
    gauges.log_held(3, later);
    assert_eq!(*moqui_lib::sync::lock(&gauges.held_logged), Some(later));
    gauges.log_held(0, later);
    assert_eq!(*moqui_lib::sync::lock(&gauges.held_logged), None);
}

#[test]
fn a_row_another_worker_took_is_not_delivered_again() {
    let ctx = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    create_row(
        &facade,
        "moqui.entity.document.DataDocument",
        &[("dataDocumentId", "GOOD"), ("primaryEntityName", "moqui.test.Foo")],
    );
    pending(&facade, "R00001", "GOOD");
    // This worker read the row; another delivered it and deleted it before this one got to it.
    let rows = facade.find(PENDING).unwrap().list().unwrap();
    assert_eq!(rows.len(), 1);
    let mut gone = facade.make_value(PENDING).unwrap();
    gone.set("dataFeedPendingId", Value::string("R00001"));
    gone.delete(&facade).unwrap();

    let mut report = FeedReport::default();
    deliver_rows(
        &facade,
        &rows,
        &realtime_feeds(&facade).unwrap(),
        false,
        None,
        &mut report,
    );
    assert_eq!(report, FeedReport::default(), "{report:?}");

    // A row still there is claimed and delivered once; the second pass finds it gone.
    pending(&facade, "R00002", "GOOD");
    let rows = facade.find(PENDING).unwrap().list().unwrap();
    let mut first = FeedReport::default();
    deliver_rows(
        &facade,
        &rows,
        &realtime_feeds(&facade).unwrap(),
        false,
        None,
        &mut first,
    );
    assert_eq!((first.rows_done, first.documents_deleted), (1, 1), "{first:?}");
    let mut second = FeedReport::default();
    deliver_rows(
        &facade,
        &rows,
        &realtime_feeds(&facade).unwrap(),
        false,
        None,
        &mut second,
    );
    assert_eq!(second, FeedReport::default(), "{second:?}");

    // A worker given the later row of a record takes the earlier one too (its own delivery waits on whoever has
    // it), and leaves another record's row alone.
    pending(&facade, "R00003", "GOOD");
    pending(&facade, "R00004", "GOOD");
    create_row(
        &facade,
        PENDING,
        &[
            ("dataFeedPendingId", "R00005"),
            ("dataDocumentId", "GOOD"),
            ("primaryKey", "F2"),
            ("operation", "index"),
        ],
    );
    let report = process_pending(Some(&["R00004".to_string()]), DEFAULT_STUCK_AFTER).unwrap();
    assert_eq!(report.rows_done, 2, "{report:?}");
    let left: Vec<String> = facade
        .find(PENDING)
        .unwrap()
        .list()
        .unwrap()
        .iter()
        .filter_map(|row| row.get("dataFeedPendingId").map(to_plain_string))
        .collect();
    assert_eq!(left, vec!["R00005".to_string()]);
}

#[test]
fn a_failed_call_takes_back_only_its_own_errors() {
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(crate::testing::open());
    let _entered = ctx.enter();
    ctx.message().add_message("kept");
    ctx.message().add_error("earlier");
    assert!(call_checked("moqui.test.NoSuchServices.receive#Documents", &BTreeMap::new()).is_err());
    assert_eq!(ctx.message().errors(), ["earlier"]);
    assert_eq!(ctx.message().messages(), ["kept"]);
}
