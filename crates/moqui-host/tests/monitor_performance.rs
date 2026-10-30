//! Opt-in, serial real-store/transport measurements. No timing assertions or user database.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
mod common;
mod speed;
use example as _;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use moqui_core::{entity::document::Document, search::SearchFacade, telemetry::Exporter};
use moqui_lib::{error::Error, object::Value};
use serde_json::{json, Value as Json};

#[derive(Default)]
struct StoreStats {
    documents: BTreeMap<String, usize>,
    milliseconds: Vec<f64>,
    http_roots: usize,
}
struct Store {
    facade: Arc<SearchFacade>,
    stats: Mutex<StoreStats>,
}
impl Exporter for Store {
    fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
        let started = Instant::now();
        let http_roots = documents
            .iter()
            .filter(|document| {
                document.get("kind").and_then(Value::as_str) == Some("SERVER")
                    && document.get("url.path").and_then(Value::as_str) == Some("/apps/dashboards/Dashboard.html")
            })
            .count();
        let count = self.facade.index_entries_deferred_refresh(index, documents)?;
        let mut stats = self.stats.lock().unwrap();
        *stats.documents.entry(index.to_owned()).or_default() += count;
        stats.milliseconds.push(started.elapsed().as_secs_f64() * 1000.0);
        stats.http_roots += http_roots;
        Ok(count)
    }
    fn upsert_error(&self, document: Document) -> Result<(), Error> {
        let fingerprint = document
            .get("fingerprint")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::new("missing error fingerprint"))?;
        self.facade
            .upsert_counter("otel_errors", fingerprint, &document, "count")
    }
}
fn distribution(mut values: Vec<f64>) -> Json {
    if values.is_empty() {
        return json!({"count":0});
    }
    values.sort_by(f64::total_cmp);
    let percentile = |p: f64| values[((values.len() - 1) as f64 * p).ceil() as usize];
    json!({"count":values.len(),"median_ms":percentile(0.5),"p95_ms":percentile(0.95),"p99_ms":percentile(0.99),
        "min_ms":values[0],"max_ms":values[values.len()-1]})
}

/// Run off/roots/full in separate processes with one compiler/profile and no
/// competing work. A fresh TCP connection per request includes actual auth,
/// screen rendering, body consumption and local persistence / optional OTLP tee.
#[test]
#[ignore = "real-store timings: MONITOR_PERF_MODE=off|roots|full; optional MONITOR_PERF_OPENSEARCH_URL and MONITOR_PERF_COLLECTOR_URL"]
fn authorized_http_with_real_store_and_optional_collector() {
    if !crate::common::own_process() {
        return;
    }
    let mode = std::env::var("MONITOR_PERF_MODE").expect("set MONITOR_PERF_MODE=off|roots|full");
    assert!(matches!(mode.as_str(), "off" | "roots" | "full"));
    let enabled = mode != "off";
    let ratio = if mode == "full" { "1" } else { "0" };
    let os = std::env::var("MONITOR_PERF_OPENSEARCH_URL").ok();
    let collector = std::env::var("MONITOR_PERF_COLLECTOR_URL").unwrap_or_default();
    let prefix = format!("monitor_perf_{}_", std::process::id());
    let mut properties = vec![("scheduled_job_check_time", "0")];
    if let Some(url) = &os {
        properties.extend([
            ("search_backend", "opensearch"),
            ("elasticsearch_url", url.as_str()),
            ("elasticsearch_index_prefix", prefix.as_str()),
            ("opensearch_run", "false"),
        ]);
    }
    let escape = |value: &str| value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
    let conf = format!(
        "<telemetry enabled=\"{enabled}\" sample-ratio=\"{ratio}\" otlp-endpoint=\"{}\"/>{}",
        escape(&collector),
        if os.is_some() {
            "<search-facade backend=\"opensearch\"/>"
        } else {
            ""
        }
    );
    let gate = common::start_with_conf_properties("monitor-perf", None, &conf, &properties);
    let factory = gate.server.factory();
    // This workload exercises the report executor's server-owned row policy, not
    // only a screen-level ADMIN grant. Disallowed records outnumber allowed ones.
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let entity = factory.entity().unwrap();
        // Keep authorization/row policy enabled during every HTTP call, while
        // allowing a short load fixture through the demo 120/min screen quota.
        let mut limit = entity.make_value("moqui.security.ArtifactTarpit").unwrap();
        limit.set("userGroupId", Value::string("ALL_USERS"));
        limit.set("artifactGroupId", Value::string("ALL_SCREENS"));
        limit.set("maxHitsCount", Value::I64(100_000));
        limit.update(&entity).unwrap();
        for document_id in ["MonitorPerfA", "MonitorPerfB"] {
            for (name, fields) in [
                (
                    "moqui.entity.document.DataDocument",
                    json!({"dataDocumentId":document_id,"indexName":"monitor_perf","primaryEntityName":"moqui.basic.Enumeration"}),
                ),
                (
                    "moqui.entity.document.DataDocumentField",
                    json!({"dataDocumentId":document_id,"fieldSeqId":"01","fieldPath":"enumId","fieldNameAlias":"id"}),
                ),
                (
                    "moqui.entity.document.DataDocumentField",
                    json!({"dataDocumentId":document_id,"fieldSeqId":"02","fieldPath":"enumTypeId","fieldNameAlias":"scope"}),
                ),
                (
                    "moqui.entity.document.DataDocumentField",
                    json!({"dataDocumentId":document_id,"fieldSeqId":"03","fieldPath":"description","fieldNameAlias":"name"}),
                ),
                (
                    "moqui.entity.document.DataDocumentUserGroup",
                    json!({"dataDocumentId":document_id,"userGroupId":"ADMIN"}),
                ),
            ] {
                let mut row = entity.make_value(name).unwrap();
                for (field, value) in fields.as_object().unwrap() {
                    row.set(field, moqui_lib::json::from_json(value));
                }
                row.create(&entity).unwrap();
            }
        }
    }
    let search = moqui_core::search::installed(factory).unwrap();
    let indexed_at = jiff::Timestamp::now().to_string();
    let documents: Vec<Document> = (0..128)
        .map(|at| {
            let Value::Map(document) = moqui_lib::json::from_json(
                &json!({"_id":format!("perf-{at}"),"_type":if at%2==0 {"MonitorPerfA"} else {"MonitorPerfB"},"_timestamp":indexed_at,
            "id":format!("perf-{at}"),"scope":if at<32 {"ALLOWED"} else {"DENIED"},"name":format!("Record {at}")}),
            ) else {
                unreachable!()
            };
            document
        })
        .collect();
    assert_eq!(search.index_documents(&documents).unwrap(), 128);
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        for definition in [
            json!({"id":"monitor-perf-rows","type":"dataset","title":"Performance rows","body":{"indexName":"monitor_perf",
                "rowFilter":{"field":{"name":"scope","op":"eq","value":"ALLOWED"}}}}),
            json!({"id":"monitor-perf-dashboard","type":"dashboard","title":"Authorized performance report","body":{"panels":{"allowed":{
                "title":"Allowed population","visualization":"stat","queries":[{"datasetId":"monitor-perf-rows",
                    "aggregations":[{"id":"documents","type":"value_count","role":"metric","params":{"field":"id"}}]}]}}}}),
        ] {
            let params =
                moqui_core::native::Parameters::from([("definition".into(), moqui_lib::json::from_json(&definition))]);
            moqui_core::service::sync("org.moqui.report.ReportServices.save#Definition", &params).unwrap();
        }
        let params = moqui_core::native::Parameters::from([(
            "query".into(),
            moqui_lib::json::from_json(&json!({"datasetId":"monitor-perf-rows",
            "aggregations":[{"id":"documents","type":"value_count","role":"metric","params":{"field":"id"}}]})),
        )]);
        let result = moqui_core::service::sync("org.moqui.report.ReportServices.run#Query", &params).unwrap();
        assert_eq!(
            result.get("sourceCount"),
            Some(&Value::I64(32)),
            "row policy must exclude 96 disallowed records: result={result:?}, errors={:?}",
            context.message().errors()
        );
    }
    let telemetry = moqui_core::telemetry::for_factory(factory);
    assert!(
        telemetry.flush_complete(),
        "bounded telemetry flush barrier must complete before recording measurements; counters={:?}",
        telemetry.counters()
    );
    let store = Arc::new(Store {
        facade: moqui_core::search::installed(factory).unwrap(),
        stats: Mutex::new(StoreStats::default()),
    });
    telemetry.set_exporter(moqui_core::telemetry::otlp::configured(factory, store.clone()));
    let address = gate.server.address();
    let cookie = common::sign_in(address, "john.doe", "moqui");
    let path = "/apps/dashboards/Dashboard.html?definitionId=monitor-perf-dashboard&from=now-1h&thru=now";
    let request = common::http_request("GET", path, &[("Cookie", &cookie)], "");
    let read = || {
        let started = Instant::now();
        let (status, _, body) = common::exchange(address, &request);
        assert_eq!(status, 200, "{body}");
        assert!(
            !body.contains("Error rendering") && !body.contains("Exception in render"),
            "{body}"
        );
        assert!(body.contains("Authorized performance report"), "{body}");
        started.elapsed().as_secs_f64() * 1000.0
    };
    for _ in 0..8 {
        read();
    }
    assert!(
        factory.wait_worker_pool_empty(100),
        "warmup background work must finish before timing"
    );
    assert!(
        telemetry.flush_complete(),
        "bounded telemetry flush barrier must complete before recording measurements; counters={:?}",
        telemetry.counters()
    );
    *store.stats.lock().unwrap() = StoreStats::default();
    let before = telemetry.counters();
    let retained_before = speed::process_counts().live;
    let calls = std::env::var("MONITOR_PERF_CALLS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(80)
        .clamp(8, 2000);
    let mut scenarios = Vec::new();
    for workers in [1, 4] {
        let started = Instant::now();
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| scope.spawn(|| (0..calls.div_ceil(workers)).map(|_| read()).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let count = results.len();
        scenarios.push(json!({"workers":workers,"latency":distribution(results),"requests_per_second":count as f64/started.elapsed().as_secs_f64()}));
    }
    let drain = Instant::now();
    assert!(
        telemetry.flush_complete(),
        "bounded telemetry flush barrier must complete before recording measurements; counters={:?}",
        telemetry.counters()
    );
    let flush_ms = drain.elapsed().as_secs_f64() * 1000.0;
    let retained_after = speed::process_counts().live;
    // Allocation pass is separate: extra global allocation counters do not
    // contaminate the timed latency distribution.
    speed::record_process_allocations(true);
    let allocation_before = speed::process_counts();
    for _ in 0..16 {
        read();
    }
    assert!(
        telemetry.flush_complete(),
        "bounded telemetry flush barrier must complete before recording measurements; counters={:?}",
        telemetry.counters()
    );
    let allocation_after = speed::process_counts();
    speed::record_process_allocations(false);
    let after = telemetry.counters();
    let stats = store.stats.lock().unwrap();
    println!(
        "MONITOR_PERF_RESULT {}",
        json!({"mode":mode,"backend":if os.is_some(){"opensearch"}else{"postgres"},
        "collector":!collector.is_empty(),"index_prefix":prefix,"path":path,"calls_per_scenario":calls,"scenarios":scenarios,
        "flush_ms":flush_ms,"rust_retained_growth_bytes":retained_after-retained_before,
        "allocations_per_request":(allocation_after.allocs-allocation_before.allocs) as f64/16.0,
        "allocated_bytes_per_request":(allocation_after.bytes-allocation_before.bytes) as f64/16.0,
        "queue":{"queued":after.queued-before.queued,"exported":after.exported-before.exported,
            "dropped":after.dropped-before.dropped,"failed":after.failed-before.failed,
            "remote_exported":after.remote_exported-before.remote_exported,"remote_dropped":after.remote_dropped-before.remote_dropped,
            "remote_failed":after.remote_failed-before.remote_failed},
        "stored_documents":stats.documents,"http_roots":stats.http_roots,"local_export_batch_latency":distribution(stats.milliseconds.clone())})
    );
    let http_roots = stats.http_roots;
    drop(stats);
    if enabled {
        assert_eq!(
            http_roots,
            calls * 2 + 16,
            "every measured HTTP request must retain its root span"
        );
        assert!(
            after.exported > before.exported,
            "the real store must receive telemetry"
        );
    }
}

#[test]
#[ignore = "live outbound OTLP timings: MONITOR_PERF_COLLECTOR_URL=http://127.0.0.1:14318"]
fn collector_transport_batch_latency() {
    let url = std::env::var("MONITOR_PERF_COLLECTOR_URL").expect("set MONITOR_PERF_COLLECTOR_URL");
    let exporter = moqui_core::telemetry::otlp::OtlpExporter::new(&url, Vec::new()).unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    for (index, value) in [
        (
            "otel_spans",
            json!({"trace_id":"11111111111111111111111111111111","span_id":"2222222222222222","name":"transport-perf","kind":"SERVER","start_nano":now*1_000_000,"duration_nano":10_000_000,"status_code":"OK","service.name":"monitor-perf"}),
        ),
        (
            "otel_logs",
            json!({"timestamp":now,"severity_number":9,"severity_text":"INFO","body":"transport-perf","service.name":"monitor-perf"}),
        ),
        (
            "otel_metrics",
            json!({"name":"transport.perf","granularity":"minute","aggregation":"histogram","bucket_start":now-60_000,"bucket_end":now,"count":2,"sum":0.3,"bounds":[0.1,0.2],"buckets":[1,1,0],"service.name":"monitor-perf"}),
        ),
    ] {
        let Value::Map(document) = moqui_lib::json::from_json(&value) else {
            unreachable!()
        };
        for size in [1, 64, 512] {
            exporter.export(index, vec![document.clone(); size]).unwrap();
            let mut timings = Vec::new();
            for _ in 0..10 {
                let documents = vec![document.clone(); size];
                let started = Instant::now();
                assert_eq!(exporter.export(index, documents).unwrap(), size);
                timings.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            println!(
                "MONITOR_TRANSPORT_RESULT {}",
                json!({"index":index,"batch_size":size,"latency":distribution(timings)})
            );
        }
    }
}
