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

    let report = process_pending(None).unwrap();
    assert_eq!(report.rows_done, 1, "{report:?}");
    assert_eq!(report.documents_deleted, 1, "{report:?}");
    assert_eq!(report.rows_left, bad + 1, "{report:?}");
    let left = facade.find(PENDING).unwrap().count().unwrap();
    assert_eq!(left, i64::try_from(bad + 1).unwrap());
    let good = facade
        .find(PENDING)
        .unwrap()
        .eq("dataFeedPendingId", Value::string("Q00000"))
        .one()
        .unwrap();
    assert!(good.is_none());
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
    deliver_rows(&facade, &rows, &realtime_feeds(&facade).unwrap(), false, &mut report);
    assert_eq!(report, FeedReport::default(), "{report:?}");

    // A row still there is claimed and delivered once; the second pass finds it gone.
    pending(&facade, "R00002", "GOOD");
    let rows = facade.find(PENDING).unwrap().list().unwrap();
    let mut first = FeedReport::default();
    deliver_rows(&facade, &rows, &realtime_feeds(&facade).unwrap(), false, &mut first);
    assert_eq!((first.rows_done, first.documents_deleted), (1, 1), "{first:?}");
    let mut second = FeedReport::default();
    deliver_rows(&facade, &rows, &realtime_feeds(&facade).unwrap(), false, &mut second);
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
    let report = process_pending(Some(&["R00004".to_string()])).unwrap();
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
