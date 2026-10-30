//! Replacement readers must retain request histories and log context without artifact-hit storage.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]
use crate::common;
use moqui_core::entity::document::Document;
use moqui_lib::object::Value;
use serde_json::json;

fn document(value: serde_json::Value) -> Document {
    let Value::Map(map) = moqui_lib::json::from_json(&value) else {
        panic!("document")
    };
    map
}

#[test]
fn monitoring_and_migrated_screens_have_valid_native_expressions() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let report = moqui_devkit::screen_lint::lint_screens(&[
        root.join("runtime/base-component/tools/screen"),
        root.join("runtime/dev-component/example/screen/Gallery"),
    ])
    .unwrap();
    assert!(report.failures().is_empty(), "{}", report.failures().join("\n"));
}

#[test]
fn visits_logs_context_and_gallery_read_authorized_telemetry() {
    if !crate::common::own_process() {
        return;
    }
    verify_readers(None);
}

#[test]
#[ignore = "requires MONITOR_READER_OPENSEARCH_URL, for example http://127.0.0.1:19200"]
fn visits_logs_context_and_gallery_read_authorized_telemetry_opensearch() {
    if !crate::common::own_process() {
        return;
    }
    let url = std::env::var("MONITOR_READER_OPENSEARCH_URL").expect("set MONITOR_READER_OPENSEARCH_URL");
    verify_readers(Some(&url));
}

fn verify_readers(opensearch_url: Option<&str>) {
    moqui_host::logging::install();
    let now = jiff::Timestamp::now().as_millisecond();
    let start = jiff::Timestamp::from_millisecond(now - 60_000).unwrap();
    let fixture = format!(
        r#"<entity-facade-xml type="seed"><moqui.server.Visit visitId="MONITOR_READER_VISIT" userId="EX_JOHN_DOE" fromDate="{start}"/></entity-facade-xml>"#
    );
    let prefix = format!("monitor_readers_{}_", std::process::id());
    let gate = match opensearch_url {
        Some(url) => common::start_with_conf_properties(
            "monitor-readers-os",
            Some(&fixture),
            "<search-facade backend=\"opensearch\"/>",
            &[
                ("search_backend", "opensearch"),
                ("elasticsearch_url", url),
                ("elasticsearch_index_prefix", &prefix),
                ("opensearch_run", "false"),
            ],
        ),
        None => common::start_with_data("monitor-readers", &fixture),
    };
    let facade = moqui_core::search::installed(gate.server.factory()).unwrap();
    facade.index_entries("otel_spans", vec![
        document(json!({"_id":"reader-root-a","trace_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","span_id":"1111111111111111","kind":"SERVER","name":"Reader request A","start":now-5000,"start_nano":(now-5000)*1_000_000,"duration_nano":10000000,"status_code":"OK","sampled":false,"moqui.visit.id":"MONITOR_READER_VISIT","user.id":"EX_JOHN_DOE","moqui.artifact.type":"AT_XML_SCREEN","moqui.artifact.name":"Reader request A"})),
        document(json!({"_id":"reader-root-b","trace_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","span_id":"2222222222222222","kind":"SERVER","name":"Reader request B","start":now-4000,"start_nano":(now-4000)*1_000_000,"duration_nano":20000000,"status_code":"ERROR","sampled":true,"moqui.visit.id":"MONITOR_READER_VISIT","user.id":"EX_JOHN_DOE","moqui.artifact.type":"AT_XML_SCREEN_TRANS","moqui.artifact.name":"Reader request B"})),
        document(json!({"_id":"reader-child","trace_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","span_id":"3333333333333333","parent_span_id":"1111111111111111","kind":"INTERNAL","name":"Reader child excluded","start":now-4900,"duration_nano":1000000,"moqui.visit.id":"MONITOR_READER_VISIT","user.id":"EX_JOHN_DOE"})),
    ]).unwrap();
    let same = jiff::Timestamp::from_millisecond(now - 3000).unwrap().to_string();
    facade.index_entries("otel_logs", (0..3).map(|at| document(json!({"_id":format!("reader-context-{at}"),"timestamp":same,"severity_text":"ERROR","severity_number":17,"body":format!("readermarker line {at}"),"thread.name":"reader-worker","user.id":"EX_JOHN_DOE","exception.type":"ReaderError","exception.message":"reader exception","exception.stacktrace":"reader.rs:42"}))).collect()).unwrap();
    let minute = now - 86_400_000;
    facade.index_entries("otel_metrics", vec![
        document(json!({"_id":"reader-duration","name":"moqui.artifact.duration","bucket_start":minute,"bucket_end":minute+60000,"granularity":"minute","count":3,"sum":0.3,"min":0.05,"max":0.15,"moqui.artifact.type":"AT_SERVICE","moqui.artifact.sub_type":"inline","moqui.artifact.name":"reader.artifact"})),
        document(json!({"_id":"reader-slow","name":"moqui.artifact.slow","bucket_start":minute,"bucket_end":minute+60000,"granularity":"minute","count":1,"sum":2,"moqui.artifact.type":"AT_SERVICE","moqui.artifact.sub_type":"inline","moqui.artifact.name":"reader.artifact"})),
    ]).unwrap();
    let auth = common::basic("john.doe", "moqui");
    let read = |path: &str| {
        let (status, _, body) = common::exchange(
            gate.server.address(),
            &common::http_request("GET", path, &[("Authorization", &auth)], ""),
        );
        assert_eq!(status, 200, "{path}: {body}");
        assert!(
            !body.contains("Error rendering") && !body.contains("Exception in render"),
            "{path}: {body}"
        );
        body
    };
    let visit = read("/System/Visit/VisitDetail.html?visitId=MONITOR_READER_VISIT");
    assert!(
        visit.contains("Reader request A") && visit.contains("Reader request B"),
        "{visit}"
    );
    assert!(!visit.contains("Reader child excluded"), "{visit}");
    assert!(
        visit.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa") && visit.contains("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        "{visit}"
    );
    let list = read("/System/Visit/VisitList.html?visitId=MONITOR_READER_VISIT");
    assert!(
        list.contains("Retained requests") && list.contains("MONITOR_READER_VISIT"),
        "{list}"
    );
    let visit_row = list
        .split("</tr>")
        .find(|row| row.contains("MONITOR_READER_VISIT") && row.contains("<td>"))
        .expect("visit row");
    assert!(
        visit_row.contains(">2</span>"),
        "request roots must count both sampled and unsampled requests once: {visit_row}"
    );
    let logs =
        read("/apps/dashboards/Explore.html?datasetId=otel_logs&queryString=body%3Areadermarker&from=now-1h&thru=now");
    assert!(
        logs.contains("ReaderError") && logs.contains("reader.rs:42") && logs.contains("reader-worker"),
        "{logs}"
    );
    assert!(
        logs.contains("Security/UserAccount/UserAccountDetail") && logs.contains("Surrounding entries"),
        "{logs}"
    );
    let newest = read(
        "/apps/dashboards/Explore.html?datasetId=otel_logs&queryString=body%3Areadermarker&from=now-1h&thru=now&pageSize=2",
    );
    assert!(
        newest.find("readermarker line 2").unwrap() < newest.find("readermarker line 1").unwrap(),
        "newest tie order: {newest}"
    );
    assert!(!newest.contains("readermarker line 0"), "page size: {newest}");
    let oldest = read(
        "/apps/dashboards/Explore.html?datasetId=otel_logs&queryString=body%3Areadermarker&from=now-1h&thru=now&pageSize=2&order=oldest",
    );
    assert!(
        oldest.find("readermarker line 1").unwrap() < oldest.find("readermarker line 2").unwrap(),
        "oldest visible-page order: {oldest}"
    );
    let invalid = read("/apps/dashboards/Explore.html?datasetId=otel_logs&queryString=%28&from=now-1h&thru=now");
    assert!(
        invalid.contains("query") || invalid.contains("Query"),
        "query error should render: {invalid}"
    );
    let tail = read("/apps/dashboards/Explore/tail?datasetId=otel_logs&queryString=body%3Areadermarker&from=now-1h");
    let tail: serde_json::Value = serde_json::from_str(&tail).expect("tail is JSON");
    let hits = tail["hits"].as_array().expect("tail hits");
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0]["id"], "reader-context-0");
    assert_eq!(hits[2]["id"], "reader-context-2");
    let context = read(
        "/apps/dashboards/LogContext.html?datasetId=otel_logs&documentType=otel_logs&documentId=reader-context-1&queryString=body%3Areadermarker&from=now-1h&thru=now",
    );
    for at in 0..3 {
        assert!(context.contains(&format!("readermarker line {at}")), "{context}");
    }
    let report = read("/apps/gallery/Report.html?artifactName=reader.artifact");
    assert!(
        report.contains("reader.artifact") && report.contains("live artifact metric intervals"),
        "{report}"
    );
}
