//! Streaming/idempotent maintenance acceptance against the actual search stores.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
mod common;
use example as _;
use moqui_core::{entity::document::Document, search, telemetry::maintenance};
use moqui_lib::object::Value;

#[test]
fn postgres_rollups_stream_over_a_page_and_retry_without_doubling() {
    if !crate::common::own_process() {
        return;
    }
    verify(None);
}

#[test]
#[ignore = "requires MONITOR_READER_OPENSEARCH_URL"]
fn opensearch_rollups_stream_over_a_page_and_retry_without_doubling() {
    if !crate::common::own_process() {
        return;
    }
    let url = std::env::var("MONITOR_READER_OPENSEARCH_URL").expect("set MONITOR_READER_OPENSEARCH_URL");
    verify(Some(&url));
}

fn verify(url: Option<&str>) {
    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 24 * HOUR;
    let prefix = format!("monitor_rollup_{}_", std::process::id());
    let gate = if let Some(url) = url {
        common::start_with_conf_properties(
            "telemetry-rollup-os",
            None,
            "<search-facade backend=\"opensearch\"/>",
            &[
                ("search_backend", "opensearch"),
                ("elasticsearch_url", url),
                ("elasticsearch_index_prefix", &prefix),
                ("opensearch_run", "false"),
            ],
        )
    } else {
        common::start_with_data("telemetry-rollup-pg", "<entity-facade-xml type=\"seed\"/>")
    };
    let factory = gate.server.factory();
    let facade = search::installed(factory).unwrap();
    let day = jiff::Timestamp::now().as_millisecond().div_euclid(DAY) * DAY - DAY;
    let hour = day + HOUR;
    let documents = (0..1005)
        .map(|at| {
            Document::from_iter([
                ("_id".into(), Value::String(format!("rollup-{at:05}"))),
                ("name".into(), Value::string("rollup-page-fixture")),
                ("series_fingerprint".into(), Value::string("rollup-page-series")),
                ("bucket_start".into(), Value::I64(hour)),
                ("bucket_end".into(), Value::I64(hour + 60_000)),
                ("granularity".into(), Value::string("minute")),
                ("aggregation".into(), Value::string("histogram")),
                ("count".into(), Value::I64(1)),
                ("sum".into(), Value::F64(2.0)),
                ("min".into(), Value::F64(2.0)),
                ("max".into(), Value::F64(2.0)),
                ("bounds".into(), Value::List(vec![Value::F64(1.0), Value::F64(3.0)])),
                (
                    "buckets".into(),
                    Value::List(vec![Value::I64(0), Value::I64(1), Value::I64(0)]),
                ),
            ])
        })
        .collect();
    assert_eq!(facade.index_entries("otel_metrics", documents).unwrap(), 1005);
    // Real normalized OTLP populations exercise the same exponential hour/day
    // persistence path on PostgreSQL and OpenSearch, across differing scales.
    let decoder = moqui_core::telemetry::ingest::Decoder::default();
    for (at, scale, count, zero, buckets, sum) in [(1, 1, 4, 1, vec![1, 2], 3.0), (2, 0, 8, 2, vec![6], 8.0)] {
        let body = serde_json::json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"rollup.exponential",
        "exponentialHistogram":{"aggregationTemporality":1,"dataPoints":[{
            "startTimeUnixNano":(hour+(at-1)*60_000)*1_000_000,"timeUnixNano":(hour+at*60_000)*1_000_000,
            "scale":scale,"count":count,"zeroCount":zero,"sum":sum,"positive":{"offset":0,"bucketCounts":buckets}
        }]}}]}]}]});
        let decoded = decoder
            .decode(
                moqui_core::telemetry::ingest::Signal::Metrics,
                "application/json",
                body.to_string().as_bytes(),
                "rollup-fixture",
            )
            .unwrap();
        assert_eq!(decoded.rejected, 0);
        assert_eq!(facade.index_entries("otel_metrics", decoded.documents).unwrap(), 1);
    }
    for _ in 0..2 {
        let status = maintenance::rollup_metrics(factory).unwrap();
        assert_eq!(status["failed"], Value::I64(0), "{status:?}");
        for (granularity, bucket) in [("hour", hour), ("day", day)] {
            let id = format!("rollup-page-series:{granularity}:{bucket}");
            let stored = facade
                .get_document("otel_metrics", &id)
                .unwrap()
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(stored["count"], Value::I64(1005));
            assert_eq!(
                moqui_lib::object::to_plain_string(&stored["sum"])
                    .parse::<f64>()
                    .unwrap(),
                2010.0
            );
            assert_eq!(
                stored["buckets"],
                Value::List(vec![Value::I64(0), Value::I64(1005), Value::I64(0)])
            );
            let mut request = search::SearchRequest::new(search::Query::all(vec![
                search::Query::eq("name", Value::string("rollup.exponential")),
                search::Query::eq("granularity", Value::string(granularity)),
            ]));
            request.limit = 2;
            let page = facade
                .search(&search::Target::Index("otel_metrics".into()), &request)
                .unwrap();
            assert_eq!(page.total, 1, "one deterministic exponential rollup per period");
            let raw = moqui_lib::json::to_json(&page.hits[0].body["exponential_histogram"]);
            assert_eq!(raw["scale"], 0);
            assert_eq!(raw["count"], 12);
            assert_eq!(raw["zeroCount"], 3);
            assert_eq!(raw["positive"]["bucketCounts"], serde_json::json!([9]));
            assert_eq!(page.hits[0].body["count"], Value::I64(12));
        }
    }
    let now = jiff::Timestamp::now().as_millisecond();
    let cases = [
        ("otel_spans", "start", None, 7),
        ("otel_logs", "timestamp", None, 7),
        ("otel_errors", "last_seen", None, 90),
        ("monitor_annotations", "time", None, 730),
        ("otel_metrics", "bucket_start", Some("minute"), 7),
        ("otel_metrics", "bucket_start", Some("hour"), 90),
        ("otel_metrics", "bucket_start", Some("day"), 730),
    ];
    for (index, field, granularity, days) in cases {
        let tag = granularity.unwrap_or("signal");
        let documents = [("old", now - (days + 1) * DAY), ("fresh", now - 1000)]
            .into_iter()
            .map(|(age, stamp)| {
                let mut document = Document::from_iter([
                    ("_id".into(), Value::String(format!("retention-{tag}-{age}"))),
                    (field.into(), Value::I64(stamp)),
                ]);
                if let Some(granularity) = granularity {
                    document.insert("granularity".into(), Value::string(granularity));
                }
                document
            })
            .collect();
        assert_eq!(facade.index_entries(index, documents).unwrap(), 2, "{index}/{tag}");
    }
    let status = maintenance::retain(factory).unwrap();
    assert_eq!(status["failed"], Value::I64(0), "{status:?}");
    for (index, _, granularity, _) in cases {
        let tag = granularity.unwrap_or("signal");
        assert!(
            facade
                .get_document(index, &format!("retention-{tag}-old"))
                .unwrap()
                .is_none(),
            "old {index}/{tag} survived retention"
        );
        assert!(
            facade
                .get_document(index, &format!("retention-{tag}-fresh"))
                .unwrap()
                .is_some(),
            "fresh {index}/{tag} was deleted"
        );
    }
}
