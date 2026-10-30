//! Day partitions of the telemetry tables on the Postgres search backend (`partition-telemetry`): rows land in their
//! day or the default partition, retention drops whole expired days (per metric granularity) before deleting rows, and
//! a table made unpartitioned is moved into partitions in place (plans/Monitoring.md, Indexes: retention).
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
use crate::common;
use moqui_core::{entity::document::Document, search, telemetry::maintenance, transaction::TransactionManager};
use moqui_lib::object::{to_plain_string, Value};

const DAY: i64 = 86_400_000;

fn scalar(tx: &TransactionManager, sql: &str) -> Option<String> {
    tx.with_reserve(|conn| conn.query(sql, &[]))
        .unwrap()
        .first()
        .and_then(|row| row.get("v"))
        .filter(|value| !value.is_null())
        .map(to_plain_string)
}

fn exec(tx: &TransactionManager, sql: &str) {
    tx.with_reserve(|conn| conn.execute(sql, &[]))
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

fn day_of(millis: i64) -> jiff::civil::Date {
    jiff::Timestamp::from_millisecond(millis)
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
}

fn name(day: jiff::civil::Date) -> String {
    day.strftime("%Y%m%d").to_string()
}

/// A day partition made by hand, as one made ahead would have been on that day.
fn make_day(tx: &TransactionManager, parent: &str, day: jiff::civil::Date) {
    let next = day.tomorrow().unwrap();
    exec(
        tx,
        &format!(
            "CREATE TABLE search.{parent}_p{} PARTITION OF search.{parent} FOR VALUES FROM ('{} 00:00:00+00') TO ('{} 00:00:00+00')",
            name(day),
            day.strftime("%Y-%m-%d"),
            next.strftime("%Y-%m-%d")
        ),
    );
}

fn partition_of(tx: &TransactionManager, table: &str, id: &str) -> Option<String> {
    scalar(
        tx,
        &format!("SELECT tableoid::regclass::text AS v FROM search.{table} WHERE doc_id = '{id}'"),
    )
}

fn span(id: &str, at: i64) -> Document {
    Document::from_iter([
        ("_id".into(), Value::string(id)),
        ("name".into(), Value::string("partition-fixture")),
        ("trace_id".into(), Value::string("5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a")),
        ("start".into(), Value::I64(at)),
    ])
}

fn metric(id: &str, granularity: &str, at: i64) -> Document {
    Document::from_iter([
        ("_id".into(), Value::string(id)),
        ("name".into(), Value::string("partition.fixture")),
        ("granularity".into(), Value::string(granularity)),
        ("bucket_start".into(), Value::I64(at)),
        ("count".into(), Value::I64(1)),
    ])
}

fn annotation(id: &str, at: i64) -> Document {
    Document::from_iter([
        ("_id".into(), Value::string(id)),
        ("source".into(), Value::string("user")),
        ("text".into(), Value::string("partition fixture")),
        ("time".into(), Value::I64(at)),
    ])
}

#[test]
fn day_partitions_take_rows_drop_expired_days_and_move_an_unpartitioned_table() {
    if !common::own_process() {
        return;
    }
    let gate = common::start_with_conf_properties(
        "telemetry-partitions",
        None,
        r#"<search-facade><postgres partition-telemetry="true" partition-days-ahead="2"/></search-facade>"#,
        &[],
    );
    let factory = gate.server.factory();
    let facade = search::installed(factory).unwrap();
    let tx = factory
        .entity()
        .unwrap()
        .transactions_for("moqui.entity.document.DataDocument")
        .unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    let today = day_of(now);
    let old = now - 10 * DAY;
    let far = now - 400 * DAY;

    // A first write makes the table partitioned, with yesterday through two days ahead and a default.
    assert_eq!(
        facade
            .index_entries("otel_spans", vec![span("part-fresh", now)])
            .unwrap(),
        1
    );
    assert_eq!(
        scalar(
            &tx,
            "SELECT count(*)::text AS v FROM pg_partitioned_table WHERE partrelid = 'search.otel_spans'::regclass"
        )
        .as_deref(),
        Some("1")
    );
    for offset in -1..=2 {
        let day = today.checked_add(jiff::Span::new().days(offset)).unwrap();
        let table = format!("search.otel_spans_p{}", name(day));
        assert_eq!(
            scalar(&tx, &format!("SELECT to_regclass('{table}')::text AS v")).as_deref(),
            Some(table.as_str()),
            "day {offset}"
        );
    }
    make_day(&tx, "otel_spans", day_of(old));
    facade
        .index_entries("otel_spans", vec![span("part-old", old), span("part-far", far)])
        .unwrap();
    // The same id and time again is the same row.
    facade
        .index_entries("otel_spans", vec![span("part-fresh", now)])
        .unwrap();
    assert_eq!(
        scalar(
            &tx,
            "SELECT count(*)::text AS v FROM search.otel_spans WHERE doc_id = 'part-fresh'"
        )
        .as_deref(),
        Some("1")
    );
    assert_eq!(
        partition_of(&tx, "otel_spans", "part-fresh").unwrap(),
        format!("search.otel_spans_p{}", name(today))
    );
    assert_eq!(
        partition_of(&tx, "otel_spans", "part-old").unwrap(),
        format!("search.otel_spans_p{}", name(day_of(old)))
    );
    assert_eq!(
        partition_of(&tx, "otel_spans", "part-far").as_deref(),
        Some("search.otel_spans_default")
    );
    let page = facade
        .search(
            &search::Target::Index("otel_spans".into()),
            &search::SearchRequest::new(search::Query::eq("name", Value::string("partition-fixture"))),
        )
        .unwrap();
    assert_eq!(page.hits.len(), 3, "searches read every partition");

    // Metrics: each granularity is its own list partition, split by day. The seeded database may already have the
    // table, made unpartitioned before this server's conf; then it is moved first.
    facade
        .index_entries("otel_metrics", vec![metric("part-now", "minute", now)])
        .unwrap();
    let moved = maintenance::partition_tables(factory, Some("otel_metrics")).unwrap();
    assert_eq!(moved["failed"], Value::List(Vec::new()), "{moved:?}");
    assert!(facade.get_document("otel_metrics", "part-now").unwrap().is_some());
    make_day(&tx, "otel_metrics_minute", day_of(old));
    make_day(&tx, "otel_metrics_hour", day_of(old));
    facade
        .index_entries(
            "otel_metrics",
            vec![
                metric("part-minute-old", "minute", old),
                metric("part-hour-old", "hour", old),
            ],
        )
        .unwrap();
    assert_eq!(
        partition_of(&tx, "otel_metrics", "part-minute-old").unwrap(),
        format!("search.otel_metrics_minute_p{}", name(day_of(old)))
    );

    let status = maintenance::retain(factory).unwrap();
    assert_eq!(status["failed"], Value::I64(0), "{status:?}");
    assert!(
        matches!(status["droppedPartitions"], Value::I64(dropped) if dropped >= 2),
        "{status:?}"
    );
    let gone = |table: &str| scalar(&tx, &format!("SELECT to_regclass('search.{table}')::text AS v")).is_none();
    assert!(
        gone(&format!("otel_spans_p{}", name(day_of(old)))),
        "the expired span day is dropped"
    );
    assert!(gone(&format!("otel_metrics_minute_p{}", name(day_of(old)))));
    assert!(
        !gone(&format!("otel_metrics_hour_p{}", name(day_of(old)))),
        "hour metrics keep 90 days"
    );
    assert!(facade.get_document("otel_spans", "part-old").unwrap().is_none());
    assert!(
        facade.get_document("otel_spans", "part-far").unwrap().is_none(),
        "the default partition's rows are deleted"
    );
    assert!(facade.get_document("otel_spans", "part-fresh").unwrap().is_some());
    assert!(facade.get_document("otel_metrics", "part-hour-old").unwrap().is_some());
    assert!(facade
        .get_document("otel_metrics", "part-minute-old")
        .unwrap()
        .is_none());

    // A table made before the option was on: written unpartitioned, then moved in place.
    facade
        .index_entries("monitor_annotations", vec![annotation("before", now)])
        .unwrap();
    exec(&tx, "DROP TABLE search.monitor_annotations");
    exec(
        &tx,
        "DELETE FROM search.document_index WHERE document_id = 'monitor_annotations'",
    );
    let plain = search::postgres::PostgresBackend::open(tx.clone(), None).unwrap();
    let mapping = search::telemetry::telemetry_mapping("monitor_annotations").unwrap();
    search::SearchBackend::ensure_index(&plain, &mapping).unwrap();
    search::SearchBackend::index_documents(
        &plain,
        &mapping,
        &[
            ("ann-today".to_string(), annotation("ann-today", now)),
            ("ann-old".to_string(), annotation("ann-old", now - 5 * DAY)),
        ],
    )
    .unwrap();
    // The server's backend learns the table is not partitioned when its insert's key does not match.
    facade
        .index_entries("monitor_annotations", vec![annotation("ann-server", now)])
        .unwrap();
    assert_eq!(
        scalar(
            &tx,
            "SELECT count(*)::text AS v FROM pg_partitioned_table WHERE partrelid = 'search.monitor_annotations'::regclass"
        )
        .as_deref(),
        Some("0")
    );
    let moved = maintenance::partition_tables(factory, Some("monitor_annotations")).unwrap();
    assert_eq!(moved["moved"], Value::I64(1), "{moved:?}");
    assert_eq!(moved["rows"], Value::I64(3), "{moved:?}");
    assert_eq!(
        moved["kept"],
        Value::List(vec![Value::string("search.monitor_annotations_unpartitioned")])
    );
    assert_eq!(
        partition_of(&tx, "monitor_annotations", "ann-old").unwrap(),
        format!("search.monitor_annotations_p{}", name(day_of(now - 5 * DAY)))
    );
    for id in ["ann-today", "ann-old", "ann-server"] {
        assert!(
            facade.get_document("monitor_annotations", id).unwrap().is_some(),
            "{id}"
        );
    }
    facade
        .index_entries("monitor_annotations", vec![annotation("ann-after", now)])
        .unwrap();
    assert_eq!(
        partition_of(&tx, "monitor_annotations", "ann-after").unwrap(),
        format!("search.monitor_annotations_p{}", name(today))
    );
    // Moving again finds nothing to move.
    let again = maintenance::partition_tables(factory, None).unwrap();
    assert_eq!(again["moved"], Value::I64(0), "{again:?}");
    assert_eq!(again["failed"], Value::List(Vec::new()), "{again:?}");
}
