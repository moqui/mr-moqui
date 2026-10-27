//! HTTP boundary and scheduled report acceptance on a real seeded PostgreSQL server.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "telemetry"))]
mod common;
use example as _;
use moqui_core::{native::Parameters, transaction::TransactionAttribute};
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};
use std::io::{Read, Write};
use std::time::Duration;
fn params(value: Json) -> Parameters {
    let Value::Map(map) = moqui_lib::json::from_json(&value) else {
        panic!("map")
    };
    map.into_iter().collect()
}
fn invoke(name: &str, input: Json) -> Json {
    let result = moqui_core::service::sync(name, &params(input)).unwrap_or_else(|error| panic!("{name}: {error}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|context| context.message().errors().to_vec())
        .unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    moqui_lib::json::to_json(&Value::Map(result.into_iter().collect()))
}
fn post(
    address: std::net::SocketAddr,
    path: &str,
    key: &str,
    kind: &str,
    encoding: Option<&str>,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut header = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\napi_key: {key}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(encoding) = encoding {
        header.push_str(&format!("Content-Encoding: {encoding}\r\n"));
    }
    header.push_str("\r\n");
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
    let status = String::from_utf8_lossy(&response[..split])
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, response[split + 4..].to_vec())
}
#[test]
fn otlp_keys_partial_success_compression_and_request_context() {
    let gate = common::start_with_conf_properties(
        "monitor-edge",
        Some(
            r#"<entity-facade-xml type="seed"><moqui.security.UserAccount userId="OTHER_MONITOR" username="other.monitor" disabled="N"/><moqui.security.UserGroupMember userGroupId="ADMIN" userId="OTHER_MONITOR" fromDate="0"/></entity-facade-xml>"#,
        ),
        "<telemetry sample-ratio=\"0\"><receiver body-limit=\"4096\" requests-per-minute=\"20\"/></telemetry>",
        &[],
    );
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let key = invoke(
        "org.moqui.monitor.MonitorServices.create#IngestKey",
        json!({"source":"edge-fixture"}),
    );
    let token = key["accessToken"].as_str().unwrap();
    {
        let login_context = factory.new_context();
        let _login = login_context.enter();
        assert!(
            moqui_core::user::login_user_key(token).is_err(),
            "collector credentials must not authenticate through the general login-key path"
        );
    }
    {
        let other = factory.new_context();
        let _other_entered = other.enter();
        moqui_core::user::internal_login_user("other.monitor").unwrap();
        let result = moqui_core::service::sync(
            "org.moqui.monitor.MonitorServices.revoke#IngestKey",
            &params(json!({"keyId":key["keyId"]})),
        );
        assert!(
            result.is_err() || !other.message().errors().is_empty(),
            "even another administrator cannot revoke someone else's source key"
        );
    }
    {
        let _metadata = context.disable_authz();
        let row = factory
            .entity()
            .unwrap()
            .find("moqui.monitor.IngestKey")
            .unwrap()
            .eq("keyId", Value::string(key["keyId"].as_str().unwrap()))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(row.get("revoked").and_then(Value::as_str), Some("N"));
    }
    let address = gate.server.address();
    let now = jiff::Timestamp::now().as_millisecond();
    let span = json!({"traceId":"fefefefefefefefefefefefefefefefe","spanId":"1212121212121212","name":"edge-fixture","kind":2,"startTimeUnixNano":((now-10)*1_000_000).to_string(),"endTimeUnixNano":(now*1_000_000).to_string(),"attributes":[{"key":"password","value":{"stringValue":"must-never-index"}}]});
    let body=json!({"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"outside-fixture"}}]},"scopeSpans":[{"spans":[span,{"traceId":"invalid","spanId":"invalid"}]}]}]}).to_string();
    assert_eq!(
        post(
            address,
            "/v1/traces",
            "not-a-key",
            "application/json",
            None,
            body.as_bytes()
        )
        .0,
        401
    );
    let (status, response) = post(address, "/v1/traces", token, "application/json", None, body.as_bytes());
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&response));
    let response: Json = serde_json::from_slice(&response).unwrap();
    assert_eq!(response["partialSuccess"]["rejectedSpans"], "1");
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(body.as_bytes()).unwrap();
    assert_eq!(
        post(
            address,
            "/v1/traces",
            token,
            "application/json",
            Some("gzip"),
            &gzip.finish().unwrap()
        )
        .0,
        200
    );
    assert_eq!(post(address, "/v1/traces", token, "text/plain", None, b"bad").0, 415);
    assert_eq!(
        post(
            address,
            "/v1/traces",
            token,
            "application/json",
            None,
            &vec![b' '; 4097]
        )
        .0,
        413
    );
    assert_eq!(
        post(address, "/v1/traces", token, "application/json", None, b"{").0,
        400
    );
    let annotation =
        json!({"source":"deploy","text":"edge deployment","serviceName":"outside-fixture","time":now}).to_string();
    assert_eq!(
        post(
            address,
            "/v1/annotations",
            token,
            "application/json",
            None,
            annotation.as_bytes()
        )
        .0,
        200
    );
    let telemetry = moqui_core::telemetry::for_factory(factory);
    telemetry.flush();
    let search = moqui_core::search::installed(factory).unwrap();
    let mut query =
        moqui_core::search::SearchRequest::new(moqui_core::search::Query::eq("name", Value::string("edge-fixture")));
    query.limit = 10;
    let spans = search
        .search(&moqui_core::search::Target::Index("otel_spans".into()), &query)
        .unwrap();
    assert!(!spans.hits.is_empty());
    assert!(!spans
        .hits
        .iter()
        .any(|hit| format!("{:?}", hit.body).contains("must-never-index")));
    assert!(telemetry.counters().dropped >= 2);
    // The HTTP root survives zero child sampling and receives the worker's authenticated Visit/user context.
    let auth = common::basic("john.doe", "moqui");
    let parent = "00-34343434343434343434343434343434-5656565656565656-01";
    let (status, _, body) = common::exchange(
        address,
        &common::http_request(
            "GET",
            "/apps/monitor/Traces.html",
            &[("Authorization", &auth), ("traceparent", parent)],
            "",
        ),
    );
    assert_eq!(status, 200, "{body}");
    telemetry.flush();
    let roots = search
        .search(
            &moqui_core::search::Target::Index("otel_spans".into()),
            &moqui_core::search::SearchRequest::new(moqui_core::search::Query::all(vec![
                moqui_core::search::Query::eq("trace_id", Value::string("34343434343434343434343434343434")),
                moqui_core::search::Query::eq("kind", Value::string("SERVER")),
            ])),
        )
        .unwrap();
    let root = roots
        .hits
        .iter()
        .find(|hit| hit.body.get("kind").and_then(Value::as_str) == Some("SERVER"))
        .expect("retained HTTP root");
    assert_eq!(root.body.get("user.id").and_then(Value::as_str), Some("EX_JOHN_DOE"));
    assert!(root.body.get("moqui.visit.id").is_some());
    assert!(root.body.get("moqui.artifact.name").is_some());
    invoke(
        "org.moqui.monitor.MonitorServices.revoke#IngestKey",
        json!({"keyId":key["keyId"]}),
    );
    assert_eq!(
        post(address, "/v1/traces", token, "application/json", None, b"{}").0,
        401
    );
    let expiring = invoke(
        "org.moqui.monitor.MonitorServices.create#IngestKey",
        json!({"source":"expiry-fixture"}),
    );
    let expiring_token = expiring["accessToken"].as_str().unwrap();
    let entity = factory.entity().unwrap();
    {
        let _authz = context.disable_authz();
        entity
            .find("moqui.monitor.IngestKey")
            .unwrap()
            .eq("keyId", Value::string(expiring["keyId"].as_str().unwrap()))
            .update_all([("expiresDate".into(), Value::I64(now - 1))])
            .unwrap();
    }
    assert_eq!(
        post(address, "/v1/logs", expiring_token, "application/json", None, b"{}").0,
        401,
        "expired keys fail closed"
    );
    {
        let _authz = context.disable_authz();
        entity
            .find("moqui.monitor.IngestKey")
            .unwrap()
            .eq("keyId", Value::string(expiring["keyId"].as_str().unwrap()))
            .update_all([("expiresDate".into(), Value::I64(now + 86_400_000))])
            .unwrap();
        entity
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .update_all([("disabled".into(), Value::string("Y"))])
            .unwrap();
    }
    assert_eq!(
        post(address, "/v1/logs", expiring_token, "application/json", None, b"{}").0,
        401,
        "disabled key owners fail closed"
    );
    {
        let _authz = context.disable_authz();
        entity
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .update_all([("disabled".into(), Value::string("N"))])
            .unwrap();
        entity
            .find("moqui.security.UserGroupMember")
            .unwrap()
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .eq("userGroupId", Value::string("ADMIN"))
            .delete_all()
            .unwrap();
    }
    assert_eq!(
        post(address, "/v1/logs", expiring_token, "application/json", None, b"{}").0,
        401,
        "current ingest permission is required after key creation"
    );
}
#[test]
fn scheduled_reports_recheck_owner_and_export_the_panel_frame() {
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    let properties = if std::path::Path::new(chrome).is_file() {
        vec![("pdf_chromium_path", chrome)]
    } else {
        Vec::new()
    };
    let gate = common::start_with_properties(
        "monitor-reports",
        Some(
            r#"<entity-facade-xml type="seed"><moqui.security.UserAccount userId="OTHER_RECIPIENT" username="report.recipient" disabled="N" emailAddress="recipient@example.test"/>
        <moqui.security.UserGroupMember userGroupId="ADMIN" userId="OTHER_RECIPIENT" fromDate="0"/>
        <moqui.basic.email.EmailServer emailServerId="MONITOR_CAPTURE" smtpHost="127.0.0.1" smtpPort="25"/>
        <moqui.basic.email.EmailTemplate emailTemplateId="DASHBOARD_REPORT" emailServerId="MONITOR_CAPTURE" fromAddress="reports@example.test" bodyScreenLocation="component://framework/screen/DashboardReportEmail.xml" webappName="webroot" subject="${title}"/>
        </entity-facade-xml>"#,
        ),
        &properties,
    );
    let factory = gate.server.factory();
    let ec = factory.new_context();
    let _entered = ec.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let search = moqui_core::search::installed(factory).unwrap();
    let stamp = jiff::Timestamp::now().as_millisecond() - 1000;
    let Value::Map(document) = moqui_lib::json::from_json(
        &json!({"_id":"report-edge","timestamp":stamp,"body":"scheduled-fixture","severity_number":17}),
    ) else {
        unreachable!()
    };
    search.index_entries("otel_logs", vec![document]).unwrap();
    invoke(
        "org.moqui.report.ReportServices.save#Definition",
        json!({"definition":{"id":"scheduled-dashboard","type":"dashboard","title":"Scheduled fixture","body":{"panels":{"data":{"title":"Panel","visualization":"table","queries":[{"datasetId":"otel_logs","queryString":"body:scheduled-fixture","aggs":{"total":{"sum":{"field":"severity_number"}}}}]}}}}}),
    );
    let preview = invoke(
        "org.moqui.report.ReportServices.preview#Report",
        json!({"definitionId":"scheduled-dashboard","renderMode":"csv"}),
    );
    use base64::Engine;
    let csv = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(preview["contentBase64"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert!(csv.contains("17"), "{csv}");
    // The histogram panels have data in the current, partial bucket. Dropping it is a warning,
    // not missing data, so the report renders with the default drop_partial.
    invoke(
        "org.moqui.report.ReportServices.save#Definition",
        json!({"definition":{"id":"chart-preview-dashboard","type":"dashboard","title":"Offline chart preview","body":{"panels":{"plot":{"title":"Report chart","visualization":"timeSeries","queries":[{"datasetId":"otel_logs","queryString":"body:scheduled-fixture","aggregations":[{"id":"time","type":"date_histogram","role":"x-axis","params":{"field":"timestamp","fixed_interval":"1m"}},{"id":"value","type":"sum","role":"metric","params":{"field":"severity_number"}}]}]}}}}}),
    );
    let Value::Map(metric) = moqui_lib::json::from_json(
        &json!({"_id":"report-edge-metric","name":"http.server.request.duration","bucket_start":stamp,"bucket_end":stamp+60_000,"granularity":"minute","count":3,"sum":0.6,"max":0.4,"service.name":"report-edge"}),
    ) else {
        unreachable!()
    };
    search.index_entries("otel_metrics", vec![metric]).unwrap();
    for definition in ["chart-preview-dashboard", "monitor-overview"] {
        for mode in ["csv", "html"] {
            let preview = invoke(
                "org.moqui.report.ReportServices.preview#Report",
                json!({"definitionId":definition,"renderMode":mode}),
            );
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(preview["contentBase64"].as_str().unwrap())
                .unwrap();
            if mode == "html" {
                assert!(bytes.starts_with(b"<!doctype"), "{definition} {mode}");
            }
        }
    }
    let chart_renderer = moqui_core::screen::PdfRenderer::available();
    if chart_renderer {
        for (mode, signature) in [
            ("png", b"\x89PNG\r\n\x1a\n".as_slice()),
            ("pdf", b"%PDF".as_slice()),
            ("html", b"<!doctype".as_slice()),
        ] {
            let preview = invoke(
                "org.moqui.report.ReportServices.preview#Report",
                json!({"definitionId":"chart-preview-dashboard","renderMode":mode}),
            );
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(preview["contentBase64"].as_str().unwrap())
                .unwrap();
            assert!(bytes.starts_with(signature), "actual {mode} chart preview");
            let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/monitoring")
                .join(format!("preview-chart.{mode}"));
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            std::fs::write(output, bytes).unwrap();
        }
    }
    let capture = moqui_core::service::mail_capture::MailCapture::of(factory);
    capture.set_enabled(Some(true));
    capture.clear();
    let location = gate.runtime.join("scheduled-fixture.csv");
    let schedule = invoke(
        "org.moqui.report.ReportServices.create#Schedule",
        json!({"definitionId":"scheduled-dashboard","cronExpression":"0 * * * * ?","saveToLocation":location.to_string_lossy(),"renderMode":"csv","userId":"OTHER_RECIPIENT","emailTemplateId":if chart_renderer {Some("DASHBOARD_REPORT")} else {None}}),
    );
    let id = schedule["screenScheduledId"].as_str().unwrap();
    invoke(
        "org.moqui.report.ReportServices.send#Report",
        json!({"screenScheduledId":id}),
    );
    assert_eq!(std::fs::read_to_string(&location).unwrap(), csv);
    if chart_renderer {
        let messages = capture.messages();
        assert_eq!(messages.len(), 1);
        let message = &messages[0];
        assert_eq!(message.to, ["recipient@example.test"]);
        assert!(message.body.contains("cid:report-image@moqui"), "{}", message.body);
        assert!(!message.body.contains("<script"));
        assert!(message.attachment_names.contains(&"report.csv".to_string()));
        assert!(!message.message_id.is_empty());
    }

    for verb in ["pause", "send"] {
        let other = factory.new_context();
        let _other_entered = other.enter();
        moqui_core::user::internal_login_user("report.recipient").unwrap();
        let result = moqui_core::service::sync(
            &format!(
                "org.moqui.report.ReportServices.{verb}#{}",
                if verb == "pause" { "Schedule" } else { "Report" }
            ),
            &params(json!({"screenScheduledId":id,"paused":true})),
        );
        assert!(
            result.is_err() || !other.message().errors().is_empty(),
            "{verb}: schedule ownership cannot be bypassed with a full primary key"
        );
    }
    let facade = factory.entity().unwrap();
    {
        let _metadata = ec.disable_authz();
        let schedule = facade
            .find("moqui.screen.ScreenScheduled")
            .unwrap()
            .eq("screenScheduledId", Value::string(id))
            .one()
            .unwrap()
            .unwrap();
        assert_eq!(schedule.get("paused").and_then(Value::as_str), Some("N"));
    }
    let tx = facade.transactions_for("moqui.security.UserAccount").unwrap();
    tx.run(TransactionAttribute::ForceNew, None, || {
        let _authz = ec.disable_authz();
        facade
            .find("moqui.security.UserAccount")?
            .eq("userId", Value::string("EX_JOHN_DOE"))
            .update_all([("disabled".into(), Value::string("Y"))])
    })
    .unwrap();
    assert!(
        moqui_core::service::sync(
            "org.moqui.report.ReportServices.send#Report",
            &params(json!({"screenScheduledId":id}))
        )
        .is_err()
            || moqui_core::context::ExecutionContext::with_current(|context| !context.message().errors().is_empty())
                .unwrap_or(false),
        "a disabled owner must not render or deliver a report"
    );
}
#[test]
fn chromium_report_pdf_png_wait_for_ready_and_block_network() {
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    let properties = if std::path::Path::new(chrome).is_file() {
        vec![("pdf_chromium_path", chrome)]
    } else {
        Vec::new()
    };
    let _gate = common::start_with_properties("monitor-print", None, &properties);
    if !moqui_core::screen::PdfRenderer::available() {
        eprintln!("Chromium unavailable; report print integration is not verified");
        return;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/must-not-load", listener.local_addr().unwrap());
    let html = format!(
        "<!doctype html><meta name=\"moqui-report\" content=\"1\"><style>body{{font:14px sans-serif}}</style><h1>Telemetry report</h1><img src=\"{url}\"><script>window.moquiReportRendered=false;setTimeout(()=>{{document.body.append('chart ready');window.moquiReportRendered=true}},50)</script>"
    );
    let pdf = moqui_core::screen::PdfRenderer::render(&html).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let png = moqui_core::screen::PdfRenderer::render_png(&html).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(matches!(listener.accept(),Err(error)if error.kind()==std::io::ErrorKind::WouldBlock));
    let broken = "<!doctype html><meta name=\"moqui-report\" content=\"1\"><script>window.moquiReportError='fixture failure';</script>";
    assert!(moqui_core::screen::PdfRenderer::render(broken).is_err());
}

#[test]
#[ignore = "needs a live Collector: MONITOR_COLLECTOR_URL=http://127.0.0.1:14318"]
fn exports_all_signals_to_a_live_collector() {
    use moqui_core::telemetry::{otlp::OtlpExporter, Exporter};
    let endpoint = std::env::var("MONITOR_COLLECTOR_URL").expect("set MONITOR_COLLECTOR_URL");
    let exporter = OtlpExporter::new(&endpoint, Vec::new()).unwrap();
    let now = jiff::Timestamp::now().as_millisecond();
    let docs = [
        (
            "otel_spans",
            json!({"trace_id":"78787878787878787878787878787878","span_id":"9090909090909090","name":"collector-export-fixture","kind":"SERVER","start_nano":(now-10)*1_000_000,"duration_nano":10_000_000,"status_code":"OK","service.name":"moqui-collector-fixture"}),
        ),
        (
            "otel_logs",
            json!({"timestamp":now,"severity_number":9,"severity_text":"INFO","body":"collector-export-fixture","service.name":"moqui-collector-fixture"}),
        ),
        (
            "otel_metrics",
            json!({"name":"fixture.duration","granularity":"minute","aggregation":"histogram","bucket_start":now-60_000,"bucket_end":now,"count":2,"sum":0.3,"min":0.1,"max":0.2,"bounds":[0.1,0.2],"buckets":[1,1,0],"service.name":"moqui-collector-fixture"}),
        ),
    ];
    for (index, doc) in docs {
        let Value::Map(doc) = moqui_lib::json::from_json(&doc) else {
            unreachable!()
        };
        assert_eq!(exporter.export(index, vec![doc]).unwrap(), 1, "{index}");
    }
}

#[test]
fn startup_retires_legacy_jobs_without_deleting_historical_runs() {
    let fixture = r#"<entity-facade-xml type="seed">
      <moqui.service.job.ServiceJob jobName="clean_ArtifactData_daily" serviceName="retired" cronExpression="0 0 * * * ?" paused="N"/>
      <moqui.service.job.ServiceJobParameter jobName="clean_ArtifactData_daily" parameterName="days" parameterValue="7"/>
      <moqui.service.job.ServiceJobRun jobRunId="LEGACY_MONITOR_RUN" jobName="clean_ArtifactData_daily"/>
      <moqui.service.job.ServiceJob jobName="clean_ElasticSearchLogMessages_daily" serviceName="retired" cronExpression="0 0 * * * ?"/>
      <moqui.service.job.ServiceJobParameter jobName="clean_ElasticSearchLogMessages_daily" parameterName="days" parameterValue="7"/>
      <moqui.service.job.ServiceJob jobName="clean_ElasticSearchHttpLogMessages_daily" serviceName="retired" cronExpression="0 0 * * * ?"/>
    </entity-facade-xml>"#;
    let gate = common::start_with_data("monitor-retirement", fixture);
    let entity = gate.server.factory().entity().unwrap();
    let context = gate.server.factory().new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let job = entity
        .find("moqui.service.job.ServiceJob")
        .unwrap()
        .eq("jobName", Value::string("clean_ArtifactData_daily"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(job.get("paused").and_then(Value::as_str), Some("Y"));
    assert!(job
        .get("cronExpression")
        .is_none_or(|value| matches!(value, Value::Null)));
    assert!(entity
        .find("moqui.service.job.ServiceJobRun")
        .unwrap()
        .eq("jobRunId", Value::string("LEGACY_MONITOR_RUN"))
        .one()
        .unwrap()
        .is_some());
    for name in [
        "clean_ArtifactData_daily",
        "clean_ElasticSearchLogMessages_daily",
        "clean_ElasticSearchHttpLogMessages_daily",
    ] {
        assert_eq!(
            entity
                .find("moqui.service.job.ServiceJobParameter")
                .unwrap()
                .eq("jobName", Value::string(name))
                .count()
                .unwrap(),
            0
        );
    }
    assert!(entity
        .find("moqui.service.job.ServiceJob")
        .unwrap()
        .eq("jobName", Value::string("clean_ElasticSearchLogMessages_daily"))
        .one()
        .unwrap()
        .is_none());
}

/// Real Collector decoding/re-encoding, including its default protobuf+gzip HTTP export to Moqui.
#[test]
#[ignore = "needs Docker and a local MONITOR_COLLECTOR_IMAGE (default otel/opentelemetry-collector-contrib:0.130.0)"]
fn collector_forwards_json_and_raw_protobuf_for_every_signal() {
    use std::process::{Child, Command, Stdio};
    struct Collector {
        name: String,
        child: Child,
        directory: std::path::PathBuf,
    }
    impl Drop for Collector {
        fn drop(&mut self) {
            let _ = Command::new("docker")
                .args(["rm", "--force", &self.name])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = self.child.wait();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
    fn varint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        while value >= 128 {
            bytes.push((value as u8) | 128);
            value >>= 7;
        }
        bytes.push(value as u8);
        bytes
    }
    fn message(field: u64, payload: impl AsRef<[u8]>) -> Vec<u8> {
        let payload = payload.as_ref();
        [varint(field * 8 + 2), varint(payload.len() as u64), payload.to_vec()].concat()
    }
    fn fixed(field: u64, value: u64) -> Vec<u8> {
        [varint(field * 8 + 1), value.to_le_bytes().to_vec()].concat()
    }
    fn resource(service: &str) -> Vec<u8> {
        message(
            1,
            [message(1, "service.name"), message(2, message(1, service))].concat(),
        )
    }
    let gate = common::start_with_conf(
        "monitor-collector-forward",
        "<telemetry sample-ratio=\"0\"><receiver body-limit=\"65536\"/></telemetry>",
        &[],
    );
    let factory = gate.server.factory();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let key = invoke(
        "org.moqui.monitor.MonitorServices.create#IngestKey",
        json!({"source":"real-collector-fixture","expireDays":1}),
    );
    let token = key["accessToken"].as_str().unwrap();
    let source_id = key["keyId"].as_str().unwrap();
    let id = format!(
        "{}-{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let name = format!("moqui-collector-{id}");
    let directory = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&directory).unwrap();
    let configuration = directory.join("collector.yaml");
    std::fs::write(
        &configuration,
        r#"receivers:
  otlp:
    protocols:
      http:
        endpoint: 0.0.0.0:4318
exporters:
  otlphttp/moqui:
    endpoint: ${env:MOQUI_OTLP_ENDPOINT}
    headers:
      Authorization: "Bearer ${env:MOQUI_INGEST_KEY}"
    compression: gzip
service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [otlphttp/moqui]
    logs:
      receivers: [otlp]
      exporters: [otlphttp/moqui]
    metrics:
      receivers: [otlp]
      exporters: [otlphttp/moqui]
"#,
    )
    .unwrap();
    let image = std::env::var("MONITOR_COLLECTOR_IMAGE")
        .unwrap_or_else(|_| "otel/opentelemetry-collector-contrib:0.130.0".into());
    let output_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/monitoring")
        .join(format!("{name}.log"));
    std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
    let output = std::fs::File::create(&output_path).unwrap();
    let child = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--pull=never",
            "--name",
            &name,
            "--add-host=host.docker.internal:host-gateway",
            "-p",
            "127.0.0.1::4318",
            "-e",
            &format!(
                "MOQUI_OTLP_ENDPOINT=http://host.docker.internal:{}",
                gate.server.address().port()
            ),
            "-e",
            &format!("MOQUI_INGEST_KEY={token}"),
            "-v",
            &format!("{}:/etc/otelcol/config.yaml:ro", configuration.display()),
            &image,
            "--config=/etc/otelcol/config.yaml",
        ])
        .stdout(Stdio::from(output.try_clone().unwrap()))
        .stderr(Stdio::from(output))
        .spawn()
        .expect("start disposable Docker Collector");
    let mut collector = Collector { name, child, directory };
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let address = loop {
        if let Some(status) = collector.child.try_wait().unwrap() {
            panic!(
                "Collector exited {status}: {}",
                std::fs::read_to_string(&output_path).unwrap_or_default()
            );
        }
        let port = Command::new("docker")
            .args(["port", &collector.name, "4318/tcp"])
            .output()
            .unwrap();
        if port.status.success() {
            if let Ok(address) = String::from_utf8_lossy(&port.stdout)
                .trim()
                .parse::<std::net::SocketAddr>()
            {
                if std::net::TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                    break address;
                }
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Collector did not listen; {}",
            output_path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let now = jiff::Timestamp::now().as_millisecond();
    let nanos = (now as u64) * 1_000_000;
    let service = format!("collector-{id}");
    let trace = vec![0x71; 16];
    let span = vec![0x72; 8];
    let trace_message = [
        message(1, &trace),
        message(2, &span),
        message(5, "collector-protobuf"),
        vec![48, 2],
        fixed(7, nanos - 10_000_000),
        fixed(8, nanos),
    ]
    .concat();
    let log_message = [
        fixed(1, nanos),
        vec![16, 9],
        message(3, "INFO"),
        message(5, message(1, "collector-protobuf")),
    ]
    .concat();
    let number_point = [fixed(3, nanos), fixed(4, 7.0f64.to_bits())].concat();
    let metric_message = [
        message(1, "fixture.collector.protobuf"),
        message(5, message(1, number_point)),
    ]
    .concat();
    for (path, body) in [
        (
            "/v1/traces",
            message(
                1,
                [message(1, resource(&service)), message(2, message(2, trace_message))].concat(),
            ),
        ),
        (
            "/v1/logs",
            message(
                1,
                [message(1, resource(&service)), message(2, message(2, log_message))].concat(),
            ),
        ),
        (
            "/v1/metrics",
            message(
                1,
                [message(1, resource(&service)), message(2, message(2, metric_message))].concat(),
            ),
        ),
    ] {
        let (status, response) = post(address, path, "", "application/x-protobuf", None, &body);
        assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&response));
    }
    let attrs = json!({"attributes":[{"key":"service.name","value":{"stringValue":service}},{"key":"source.id","value":{"stringValue":"forged-source"}}]});
    let requests = [
        (
            "/v1/traces",
            json!({"resourceSpans":[{"resource":attrs,"scopeSpans":[{"spans":[{"traceId":"73737373737373737373737373737373","spanId":"7474747474747474","name":"collector-json","kind":2,"startTimeUnixNano":(nanos-10_000_000).to_string(),"endTimeUnixNano":nanos.to_string()}]}]}]}),
        ),
        (
            "/v1/logs",
            json!({"resourceLogs":[{"resource":attrs,"scopeLogs":[{"logRecords":[{"timeUnixNano":nanos.to_string(),"severityNumber":9,"body":{"stringValue":"collector-json"}}]}]}]}),
        ),
        (
            "/v1/metrics",
            json!({"resourceMetrics":[{"resource":attrs,"scopeMetrics":[{"metrics":[{"name":"fixture.collector.json","gauge":{"dataPoints":[{"timeUnixNano":nanos.to_string(),"asDouble":11.0}]}}]}]}]}),
        ),
    ];
    for (path, body) in requests {
        let (status, response) = post(address, path, "", "application/json", None, body.to_string().as_bytes());
        assert_eq!(status, 200, "{path}: {}", String::from_utf8_lossy(&response));
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        moqui_core::telemetry::for_factory(factory).flush();
        let mut complete = true;
        for dataset in ["otel_spans", "otel_logs", "otel_metrics"] {
            let page=moqui_core::report::documents(&context,&json!({"datasetId":dataset,"from":"now-1h","thru":"now+1h","query":{"field":{"name":"source.id","op":"eq","value":source_id}},"pageSize":20})).unwrap();
            complete &= page["total"].as_u64().unwrap_or(0) == 2;
            for hit in page["hits"].as_array().unwrap() {
                assert_eq!(hit["body"]["source.id"], source_id);
                assert_eq!(hit["body"]["service.name"], service);
            }
        }
        if complete {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Collector records not persisted; {}",
            std::fs::read_to_string(&output_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(collector);
}
