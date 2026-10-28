//! Every monitoring/report screen must render through HostLink and expose its native interactions.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]
mod common;
use example as _;
use moqui_core::{
    entity::document::Document,
    native::{Parameters, ServiceInfo},
    report, search,
    transaction::TransactionAttribute,
};
use moqui_host::tui::HostLink;
use moqui_lib::object::Value;
use moqui_tui::{AppView, Key, KeyPress, Node, ScreenModel, Session};
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;

const TRACE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn document(value: Json) -> Document {
    let Value::Map(map) = moqui_lib::json::from_json(&value) else {
        panic!("fixture object")
    };
    map
}
fn service() -> ServiceInfo<'static> {
    ServiceInfo {
        name: "org.moqui.report.ReportServices.test#MonitorTui",
        service_type: "rust",
        location: "test",
        method: "",
    }
}
fn error(model: &ScreenModel) -> Option<String> {
    let mut error = None;
    moqui_tui::walk(&model.c, &mut |node| {
        if let Node::Error { text, .. } = node {
            error.get_or_insert_with(|| text.clone());
        }
    });
    error
}
fn open(link: &HostLink, path: &str) -> AppView<HostLink> {
    let mut view = AppView::with_session(Session::start(path), link.clone());
    view.run_requests();
    assert_eq!(
        view.session().path(),
        path.split('?').next().unwrap(),
        "{path}: {}",
        view.session().message()
    );
    assert!(
        error(view.session().model()).is_none(),
        "{path}: {:?}",
        error(view.session().model())
    );
    assert!(
        !view.session().message_is_error(),
        "{path}: {}",
        view.session().message()
    );
    let text = view.screen_text(180, 80);
    for failure in ["Error rendering", "Exception in render", "Internal Server Error"] {
        assert!(!text.contains(failure), "{path}: {text}");
    }
    view
}
fn press(view: &mut AppView<HostLink>, key: Key) {
    view.on_key(&KeyPress::plain(key));
}
fn focus(view: &mut AppView<HostLink>, id: &str) {
    for _ in 0..300 {
        if view.session().focus_id() == id {
            return;
        }
        press(view, Key::Tab);
    }
    panic!("cannot focus {id}; {:?}", view.session().focus_order());
}
fn component(view: &AppView<HostLink>, name: &str) -> String {
    view.session()
        .index()
        .components
        .iter()
        .find(|(_, info)| info.name == name)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| panic!("no {name} component on {}", view.session().path()))
}
fn link_id(view: &AppView<HostLink>, fragment: &str) -> String {
    let mut id = None;
    moqui_tui::walk(&view.session().model().c, &mut |node| {
        if let Node::Link { id: node_id, url, .. } = node {
            if url.contains(fragment) {
                id.get_or_insert_with(|| node_id.clone());
            }
        }
    });
    id.unwrap_or_else(|| panic!("no {fragment} link on {}", view.session().path()))
}

/// The urls of the links in a list's rows that contain `fragment`.
fn row_urls(view: &AppView<HostLink>, list: &str, fragment: &str) -> Vec<String> {
    fn walk(value: &Json, fragment: &str, found: &mut Vec<String>) {
        match value {
            Json::Object(map) => {
                if let Some(url) = map.get("url").and_then(Json::as_str) {
                    if url.contains(fragment) {
                        found.push(url.to_string());
                    }
                }
                map.values().for_each(|value| walk(value, fragment, found));
            }
            Json::Array(items) => items.iter().for_each(|value| walk(value, fragment, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for row in view.session().list_page(list) {
        walk(&serde_json::to_value(&row).unwrap(), fragment, &mut found);
    }
    found
}
fn props(view: &AppView<HostLink>, name: &str) -> Json {
    let id = component(view, name);
    view.session().index().components[&id].props.clone()
}
fn set_field(view: &mut AppView<HostLink>, id: &str, value: &str) {
    focus(view, id);
    view.session_mut().set_buffer(id, value.to_string());
}

#[test]
fn every_monitoring_screen_has_a_working_native_view_and_interactions() {
    moqui_host::logging::install();
    let gate = common::start_with_data(
        "monitor-tui",
        r#"<entity-facade-xml type="seed">
      <moqui.security.UserAccount userId="MONITOR_ONLY" username="monitor.only" disabled="N"/>
      <moqui.security.UserGroup userGroupId="MONITOR_ONLY" description="Monitor app only"/>
      <moqui.security.UserGroupMember userGroupId="MONITOR_ONLY" userId="MONITOR_ONLY" fromDate="0"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="MONITOR_ONLY_APP" artifactGroupId="MONITOR_APP" userGroupId="MONITOR_ONLY" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
    </entity-facade-xml>"#,
    );
    let factory = gate.server.factory();
    let now = jiff::Timestamp::now().as_millisecond();
    let stamp = jiff::Timestamp::from_millisecond(now - 5000).unwrap().to_string();
    let facade = search::installed(factory).unwrap();
    facade.index_entries("otel_spans",vec![document(json!({"_id":"tui-root","trace_id":TRACE,"span_id":"1111111111111111","name":"TUI monitored request","kind":"SERVER","start":now-5000,"start_nano":(now-5000)*1_000_000,"duration_nano":10000000,"status_code":"ERROR","service.name":"monitor-tui","service.instance.id":"tui-instance","source.id":"tui-source","source.name":"TUI fixture","attributes":{"http.route":"/native-monitor"},"events":[{"name":"exception","attributes":{"exception.message":"tui exception"}}]}))]).unwrap();
    facade.index_entries("otel_logs",(0..3).map(|at|document(json!({"_id":format!("tui-log-{at}"),"timestamp":stamp,"severity_text":"ERROR","severity_number":17,"body":format!("tui-marker line {at}"),"trace_id":TRACE,"span_id":"1111111111111111","service.name":"monitor-tui","service.instance.id":"tui-instance","source.id":"tui-source","source.name":"TUI fixture","thread.name":"tui-worker","user.id":"EX_JOHN_DOE","exception.type":"TuiError","exception.stacktrace":"monitor_tui.rs:42"}))).collect()).unwrap();
    // An agent run: its AT_LLM frame and one LLM call with gen_ai.* attributes, for the AI screen.
    facade.index_entries("otel_spans",vec![
        document(json!({"_id":"tui-agent-frame","trace_id":"cccccccccccccccccccccccccccccccc","span_id":"3333333333333333","name":"agent:tui-helper","kind":"INTERNAL","start":now-4000,"start_nano":(now-4000)*1_000_000,"duration_nano":30000000,"status_code":"UNSET","moqui.artifact.type":"AT_LLM","service.name":"monitor-tui"})),
        document(json!({"_id":"tui-agent-call","trace_id":"cccccccccccccccccccccccccccccccc","span_id":"4444444444444444","parent_span_id":"3333333333333333","name":"tui-endpoint:tui-model","kind":"CLIENT","start":now-3990,"start_nano":(now-3990)*1_000_000,"duration_nano":20000000,"status_code":"UNSET","moqui.artifact.type":"AT_LLM","gen_ai.operation.name":"chat","gen_ai.provider.name":"tui-endpoint","gen_ai.request.model":"tui-model","gen_ai.usage.input_tokens":120,"gen_ai.usage.output_tokens":30,"service.name":"monitor-tui"})),
    ]).unwrap();
    facade.index_entries("otel_errors",vec![document(json!({"_id":"tui-error","fingerprint":"tui-error","message":"TUI failure group","count":3,"first_seen":now-5000,"last_seen":now-5000,"stacktrace":"monitor_tui.rs:42","service.name":"monitor-tui","sample_trace_ids":[TRACE]}))]).unwrap();
    facade.index_entries("otel_metrics",vec![document(json!({"_id":"tui-metric","name":"moqui.artifact.duration","bucket_start":now-60000,"bucket_end":now,"granularity":"minute","count":3,"sum":0.03,"min":0.01,"max":0.01,"moqui.artifact.type":"AT_SERVICE","moqui.artifact.name":"monitor.tui","service.name":"monitor-tui","service.instance.id":"tui-instance"}))]).unwrap();
    // Request metrics for two versions, one with server errors, for the Services drill-down, the Metrics picker and Compare.
    let bounds = json!([0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0]);
    facade.index_entries("otel_metrics",[("1.0","200",4,"tui-instance"),("2.0","200",3,"tui-canary"),("2.0","500",2,"tui-canary")].into_iter().map(|(version,status,count,instance)|document(json!({"_id":format!("tui-http-{version}-{status}"),"name":"http.server.request.duration","aggregation":"histogram","unit":"s","bucket_start":now-1_200_000,"bucket_end":now-1_140_000,"granularity":"minute","count":count,"sum":0.02*count as f64,"min":0.02,"max":0.02,"bounds":bounds,"buckets":[0,0,count,0,0,0,0,0,0,0,0,0,0,0,0],"http.route":"/native-monitor","http.request.method":"GET","http.response.status_code":status.parse::<i64>().unwrap(),"service.name":"monitor-tui","service.instance.id":instance,"service.version":version}))).collect()).unwrap();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let tx = factory
            .entity()
            .unwrap()
            .transactions_for("moqui.report.Definition")
            .unwrap();
        let aggregate = json!({"datasetId":"tui-dataset","aggregations":[{"id":"severity_total","type":"sum","role":"metric","params":{"field":"severity_number"}}]});
        for definition in [
            json!({"id":"tui-dataset","type":"dataset","title":"TUI scoped logs","body":{"documentType":"otel_logs","timeField":"timestamp","rowFilter":{"field":{"name":"service.name","op":"eq","value":"monitor-tui"}}}}),
            json!({"id":"tui-panel","type":"panel","title":"TUI library logs","body":{"title":"TUI library logs","visualization":"table","queries":[{"datasetId":"tui-dataset","mode":"documents","queryString":"body:tui-marker","pageSize":50}]}}),
            json!({"id":"tui-dashboard","type":"dashboard","title":"TUI acceptance dashboard","body":{"variables":[{"name":"service","kind":"custom","label":"Service","datasetId":"tui-dataset","field":"service.name","options":["monitor-tui"],"default":"monitor-tui"}],"panels":{"a-total":{"title":"TUI severity total","visualization":"stat","queries":[aggregate.clone()],"transformations":[{"type":"organize","order":["severity_total"]}]},"b-library":{"panelRef":"tui-panel"},"c-notes":{"title":"TUI notes","visualization":"text","text":"TUI offline instructions"}},"layout":{"type":"autoGrid"}}}),
            json!({"id":"tui-dashboard-second","type":"dashboard","title":"TUI second dashboard","body":{"panels":{"notes":{"title":"Second dashboard notes","visualization":"text","text":"Rotation reached this dashboard"}},"layout":{"type":"autoGrid"}}}),
            json!({"id":"tui-search","type":"search","title":"TUI saved search","body":{"datasetId":"tui-dataset","queryString":"body:tui-marker","from":"now-1h","thru":"now"}}),
            json!({"id":"tui-contact","type":"contactPoint","title":"TUI shared contact","body":{"type":"notification","topic":"ReportAlert","userId":"EX_JOHN_DOE"}}),
            json!({"id":"tui-alert","type":"alert","title":"TUI paused alert","body":{"query":aggregate,"field":"severity_total","reduce":"last","condition":{"type":"above","threshold":0},"intervalSeconds":60,"paused":true,"contactPointRefs":["tui-contact"]}}),
            json!({"id":"tui-report","type":"report","title":"TUI report","body":{"dashboards":[{"definitionId":"tui-dashboard"}]}}),
            json!({"id":"tui-playlist","type":"report","title":"TUI playlist","body":{"kind":"playlist","seconds":5,"playlist":[{"definitionId":"tui-dashboard"},{"definitionId":"tui-dashboard-second"}]}}),
        ] {
            let parameters: Parameters = document(json!({"definition":definition})).into_iter().collect();
            tx.run(TransactionAttribute::UseOrBegin, None, || {
                report::save_definition(&context, &service(), &parameters)
            })
            .unwrap();
        }
    }
    let link = HostLink::as_user(factory, "john.doe");
    let cases = [
        ("Monitor/dashboard", "/apps/monitor/dashboard?from=now-1h&thru=now"),
        ("Monitor/Services", "/apps/monitor/Services?from=now-1h&thru=now"),
        ("Monitor/Artifacts", "/apps/monitor/Artifacts?from=now-1h&thru=now"),
        (
            "Monitor/Traces",
            "/apps/monitor/Traces?from=now-1h&thru=now&serviceName=monitor-tui&spanName=TUI*&statusCode=ERROR&minDuration=1&maxDuration=500",
        ),
        (
            "Monitor/TraceDetail",
            "/apps/monitor/TraceDetail?traceId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&from=now-1h&thru=now",
        ),
        ("Monitor/Logs", "/apps/monitor/Logs?from=now-1h&thru=now"),
        ("Monitor/AgentTraces", "/apps/monitor/AgentTraces?from=now-1h&thru=now"),
        ("Monitor/Checks", "/apps/monitor/Checks"),
        (
            "Monitor/Metrics",
            "/apps/monitor/Metrics?from=now-1h&thru=now&metricName=http.server.request.duration&metricStat=p95&groupBy=service.version",
        ),
        ("Monitor/Errors", "/apps/monitor/Errors?from=now-1h&thru=now"),
        (
            "Monitor/ErrorDetail",
            "/apps/monitor/ErrorDetail?fingerprint=tui-error&from=now-1h&thru=now",
        ),
        ("Monitor/Sources", "/apps/monitor/Sources"),
        (
            "Monitor/LogContext",
            "/apps/monitor/LogContext?datasetId=otel_logs&documentType=otel_logs&documentId=tui-log-1&from=now-1h&thru=now",
        ),
        (
            "Monitor/Split",
            "/apps/monitor/Split?leftVersion=1.0&rightVersion=2.0&from=now-1h&thru=now",
        ),
        ("Dashboards/Home", "/apps/dashboards/Home"),
        (
            "Dashboards/Dashboard",
            "/apps/dashboards/Dashboard?definitionId=tui-dashboard&from=now-1h&thru=now",
        ),
        (
            "Dashboards/Definition",
            "/apps/dashboards/Definition?definitionId=tui-dashboard&leftVersion=0&rightVersion=1",
        ),
        ("Dashboards/Panels", "/apps/dashboards/Panels"),
        ("Dashboards/GitSync", "/apps/dashboards/GitSync"),
        ("Dashboards/Datasets", "/apps/dashboards/Datasets"),
        (
            "Dashboards/Explore",
            "/apps/dashboards/Explore?savedSearchId=tui-search",
        ),
        (
            "Dashboards/LogContext",
            "/apps/dashboards/LogContext?datasetId=otel_logs&documentType=otel_logs&documentId=tui-log-1&from=now-1h&thru=now",
        ),
        (
            "Dashboards/Split",
            "/apps/dashboards/Split?datasetId=otel_logs&queryString=body%3Atui-marker&from=now-1h&thru=now",
        ),
        (
            "Dashboards/Annotations",
            "/apps/dashboards/Annotations?from=now-1h&thru=now",
        ),
        ("Dashboards/Alerts", "/apps/dashboards/Alerts"),
        (
            "Dashboards/AlertRule",
            "/apps/dashboards/AlertRule?definitionId=tui-alert",
        ),
        (
            "Dashboards/AlertShared",
            "/apps/dashboards/AlertShared?definitionId=tui-contact",
        ),
        ("Dashboards/Reports", "/apps/dashboards/Reports"),
        (
            "Dashboards/Playlist",
            "/apps/dashboards/Playlist?definitionId=tui-playlist",
        ),
    ];
    // A new screen without an acceptance case is a failed gate, rather than silently missing coverage.
    let covered: BTreeSet<_> = cases.iter().map(|(name, _)| name.to_string()).collect();
    let mut actual = BTreeSet::new();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/tools/screen");
    for directory in ["Monitor", "Dashboards"] {
        for file in std::fs::read_dir(root.join(directory)).unwrap() {
            let file = file.unwrap().path();
            if file.extension().is_some_and(|extension| extension == "xml") {
                actual.insert(format!("{directory}/{}", file.file_stem().unwrap().to_string_lossy()));
            }
        }
    }
    assert_eq!(covered, actual, "every new screen needs a fixture and native traversal");
    let mut seen = BTreeSet::new();
    let mut failures = Vec::new();
    for (name, path) in cases {
        // Independent views are checked together; one failure does not hide the rest.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut view = open(&link, path);
            for (id, info) in &view.session().index().components {
                assert!(
                    moqui_tui::find_component(&info.name).is_some(),
                    "unregistered {}",
                    info.name
                );
                seen.insert(info.name.clone());
                if let Some(state) = view.session().component_state(id) {
                    assert!(state.error.is_empty(), "{}: {}", info.name, state.error);
                }
            }
            let _ = view.screen_text(100, 40);
            let _ = view.screen_text(180, 80);
        }));
        if let Err(error) = result {
            let message = error
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| error.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_else(|| "render panic".into());
            failures.push(format!("{name}: {message}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    for name in [
        "report-dashboard",
        "report-panel",
        "trace-waterfall",
        "log-tail",
        "report-definition-editor",
        "report-query-editor",
        "report-playlist",
    ] {
        assert!(seen.contains(name), "specialized twin {name} was not reachable");
    }
    // The AI screen groups LLM calls by trace and names each by its agent frame.
    let ai = open(&link, "/apps/monitor/AgentTraces?from=now-1h&thru=now").screen_text(220, 80);
    assert!(
        ai.contains("agent:tui-helper") && ai.contains("tui-model") && ai.contains("120"),
        "{ai}"
    );
    // Log patterns group the fixture's three lines into one template.
    let patterns = open(
        &link,
        "/apps/monitor/Logs?from=now-1h&thru=now&showPatterns=Y&queryString=body%3Atui-marker",
    )
    .screen_text(220, 120);
    assert!(patterns.contains("tui-marker line <*>"), "{patterns}");
    let mut dashboard = open(
        &link,
        "/apps/dashboards/Dashboard?definitionId=tui-dashboard&from=now-1h&thru=now",
    );
    let id = component(&dashboard, "report-dashboard");
    focus(&mut dashboard, &id);
    press(&mut dashboard, Key::Char('i'));
    assert!(
        dashboard.screen_text(180, 100).contains("severity_total"),
        "selected panel inspector"
    );
    let inspector = &dashboard.session().component_state(&id).unwrap().local["inspection"];
    assert_eq!(
        inspector["dashboard"]["panels"]["a-total"]["inspection"][0]["type"], "organize",
        "native inspector must load authorized per-step snapshots: {inspector}"
    );
    assert!(inspector["dashboard"]["panels"]["a-total"]["inspection"][0]["input"]["frames"].is_array());
    press(&mut dashboard, Key::Char('c'));
    let csv = dashboard.session_mut().take_downloads();
    assert_eq!(csv.len(), 1, "{}", dashboard.session().message());
    assert!(csv[0].content_type.starts_with("text/csv"));
    let csv = String::from_utf8(csv[0].bytes.clone()).unwrap();
    assert!(csv.contains("severity_total") && csv.contains("51"), "{csv}");
    press(&mut dashboard, Key::Char('x'));
    let xlsx = dashboard.session_mut().take_downloads();
    assert_eq!(xlsx.len(), 1, "{}", dashboard.session().message());
    assert!(xlsx[0].bytes.starts_with(b"PK"));
    // d k: kiosk mode draws the dashboard alone, without the app chrome; Esc brings the chrome back.
    let chrome = dashboard.screen_text(180, 80);
    let title = chrome.lines().next().unwrap_or("").to_string();
    assert!(title.contains("Dashboard"), "{chrome}");
    press(&mut dashboard, Key::Char('d'));
    press(&mut dashboard, Key::Char('k'));
    let kiosk = dashboard.screen_text(180, 80);
    assert!(kiosk.contains("(kiosk; Esc or d k leaves)"), "{kiosk}");
    assert!(
        kiosk.contains("TUI severity total") && !kiosk.contains(&title),
        "{kiosk}"
    );
    // Tab moves the focus, but the hidden screen cannot keep the keys from the kiosk.
    press(&mut dashboard, Key::Tab);
    press(&mut dashboard, Key::Escape);
    assert!(dashboard.screen_text(180, 80).contains(&title));
    assert_eq!(dashboard.session().focus_id(), id);
    press(&mut dashboard, Key::Char('v'));
    assert_eq!(
        dashboard.session().parameters().get("panelId").map(String::as_str),
        Some("a-total")
    );
    let mut editor = open(&link, "/apps/dashboards/Definition?definitionId=tui-dashboard");
    let id = component(&editor, "report-definition-editor");
    focus(&mut editor, &id);
    press(&mut editor, Key::Char('a'));
    press(&mut editor, Key::Enter);
    let field = editor
        .session()
        .index()
        .fields
        .values()
        .find(|field| field.name == "definitionJson")
        .unwrap()
        .id
        .clone();
    let body: Json = serde_json::from_str(editor.session().buffer(&field)).unwrap();
    assert_eq!(
        body["refreshSeconds"], 0,
        "structured editing changes the real definition form buffer"
    );
    assert!(editor.session().is_dirty(&field));
    let mut playlist = open(&link, "/apps/dashboards/Playlist?definitionId=tui-playlist");
    let id = component(&playlist, "report-playlist");
    focus(&mut playlist, &id);
    let deadline = playlist
        .session()
        .timer(&moqui_tui::Timer::Component(id.clone()))
        .expect("automatic playlist rotation");
    playlist.session_mut().on_time(deadline);
    playlist.run_requests();
    assert_eq!(
        playlist.session().component_state(&id).unwrap().data.as_ref().unwrap()["dashboard"]["definitionId"],
        "tui-dashboard-second"
    );
    assert!(playlist
        .screen_text(180, 80)
        .contains("Rotation reached this dashboard"));
    press(&mut playlist, Key::Char('p'));
    assert!(playlist.session().timer(&moqui_tui::Timer::Component(id)).is_none());
    let mut trace = open(
        &link,
        "/apps/monitor/TraceDetail?traceId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&from=now-1h&thru=now",
    );
    let id = component(&trace, "trace-waterfall");
    focus(&mut trace, &id);
    press(&mut trace, Key::Char('i'));
    assert!(trace.screen_text(180, 100).contains("http.route"));
    let logs = link_id(&trace, "Logs");
    focus(&mut trace, &logs);
    press(&mut trace, Key::Enter);
    assert!(trace.session().path().ends_with("/Logs"));
    let tail = component(&trace, "log-tail");
    focus(&mut trace, &tail);
    press(&mut trace, Key::Char('i'));
    // The focused tail is scrolled into view at the foot of the screen; its lines follow the Explore histogram, whose
    // row count depends on where the minute boundaries fall, so draw tall enough to hold them.
    let text = trace.screen_text(180, 160);
    assert!(text.contains("tui-marker"), "{text}");
    press(&mut trace, Key::Char('t'));
    assert!(trace.session().path().ends_with("/TraceDetail"));
    let mut context = open(
        &link,
        "/apps/dashboards/LogContext?datasetId=otel_logs&documentType=otel_logs&documentId=tui-log-1&from=now-1h&thru=now",
    );
    let row = context
        .session()
        .list_page("LogContext")
        .iter()
        .position(|row| {
            serde_json::to_string(row)
                .unwrap()
                .contains(&format!("traceId={TRACE}"))
        })
        .expect("a correlated log row");
    focus(&mut context, "LogContext");
    for _ in 0..row {
        press(&mut context, Key::Down);
    }
    assert_eq!(context.session().row_at("LogContext"), row);
    press(&mut context, Key::Right);
    assert!(context.session().path().ends_with("/TraceDetail"));
    assert_eq!(
        context.session().parameters().get("traceId").map(String::as_str),
        Some(TRACE)
    );

    // Monitor Logs includes Explore: its row links, tail and forms resolve under the Monitor screen.
    let mut logs = open(
        &link,
        "/apps/monitor/Logs?from=now-1h&thru=now&queryString=body%3Atui-marker",
    );
    let tail = props(&logs, "log-tail");
    assert!(
        tail["source"]
            .as_str()
            .is_some_and(|url| url.starts_with("/apps/monitor/Logs/tail")),
        "{tail}"
    );
    let traces = row_urls(&logs, "ExploreDocuments", "TraceDetail");
    assert!(
        !traces.is_empty() && traces.iter().all(|url| url.starts_with("/apps/monitor/TraceDetail?")),
        "{traces:?}"
    );
    let contexts = row_urls(&logs, "ExploreDocuments", "LogContext");
    assert!(
        !contexts.is_empty() && contexts.iter().all(|url| url.starts_with("/apps/monitor/LogContext?")),
        "{contexts:?}"
    );
    logs.session_mut().navigate(&traces[0], true);
    logs.run_requests();
    assert!(
        logs.session().path().ends_with("/monitor/TraceDetail"),
        "{}: {}",
        logs.session().path(),
        logs.session().message()
    );
    assert_eq!(
        logs.session().parameters().get("traceId").map(String::as_str),
        Some(TRACE)
    );
    logs.session_mut().navigate(&contexts[0], true);
    logs.run_requests();
    assert!(
        logs.session().path().ends_with("/LogContext") && !logs.session().message_is_error(),
        "{}: {}",
        logs.session().path(),
        logs.session().message()
    );

    // A Monitor dashboard inspects and exports through its own transitions; Esc leaves the single-panel view.
    let mut overview = open(&link, "/apps/monitor/dashboard?panelId=errors&from=now-1h&thru=now");
    assert_eq!(
        props(&overview, "report-dashboard")["dashboard"]["screenUrl"],
        "/apps/monitor/dashboard"
    );
    let id = component(&overview, "report-dashboard");
    focus(&mut overview, &id);
    press(&mut overview, Key::Char('i'));
    let inspected = overview.session().component_state(&id).unwrap().local["inspection"].clone();
    assert!(
        inspected["dashboard"]["panels"]["errors"].is_object(),
        "Monitor inspection: {inspected}"
    );
    press(&mut overview, Key::Char('c'));
    let csv = overview.session_mut().take_downloads();
    assert_eq!(csv.len(), 1, "{}", overview.session().message());
    let csv = String::from_utf8(csv[0].bytes.clone()).unwrap();
    assert!(csv.contains("TUI failure group"), "{csv}");
    press(&mut overview, Key::Escape);
    assert!(
        overview
            .session()
            .component_state(&id)
            .is_some_and(|state| !state.closed.contains("__report_inspector")),
        "Esc closes the inspector first"
    );
    press(&mut overview, Key::Escape);
    overview.run_requests();
    assert_eq!(overview.session().path(), "/apps/monitor/dashboard");
    assert!(
        !overview.session().parameters().contains_key("panelId"),
        "{:?}",
        overview.session().parameters()
    );

    // The time form keeps the dashboard variables and the single-panel view.
    let mut timed = open(
        &link,
        "/apps/dashboards/Dashboard?definitionId=tui-dashboard&from=now-1h&thru=now&panelId=c-notes&var-service=tui-other",
    );
    set_field(&mut timed, "DashboardTime.from", "now-2h");
    press(&mut timed, Key::Enter);
    timed.run_requests();
    assert_eq!(
        timed.session().parameters().get("from").map(String::as_str),
        Some("now-2h"),
        "{}",
        timed.session().message()
    );
    let dashboard = props(&timed, "report-dashboard")["dashboard"].clone();
    assert_eq!(dashboard["variableValues"]["service"], "tui-other", "{dashboard}");
    assert_eq!(dashboard["panelId"], "c-notes", "{dashboard}");

    // A new ingest key shows its token once, in the transition's message.
    let mut sources = open(&link, "/apps/monitor/Sources");
    set_field(&mut sources, "CreateIngestKey.source", "tui-token-check");
    press(&mut sources, Key::Enter);
    sources.run_requests();
    let message = sources.session().message().to_string();
    assert!(
        message.contains("for tui-token-check") && message.contains("token: mqi_"),
        "{message}"
    );
    assert!(sources.session().path().ends_with("/monitor/Sources"));

    // The Metrics picker charts the picked metric's percentile split by the chosen attribute.
    let metrics = open(
        &link,
        "/apps/monitor/Metrics?from=now-1h&thru=now&metricName=http.server.request.duration&metricStat=p95&groupBy=service.version",
    );
    let chart = all_props(&metrics, "report-panel")
        .into_iter()
        .find(|props| {
            props["panel"]["title"]
                .as_str()
                .is_some_and(|title| title.starts_with("http.server.request.duration"))
        })
        .expect("the metric chart");
    assert_eq!(
        chart["panel"]["title"], "http.server.request.duration · p95 by service.version",
        "{chart}"
    );
    let names = field_names(&chart["frame"]);
    assert!(
        names.iter().any(|name| name == "latency.p95") && names.iter().any(|name| name == "group"),
        "{chart}"
    );
    assert!(!names.iter().any(|name| name == "latency.p50"), "{chart}");
    assert!(metrics
        .session()
        .index()
        .fields
        .values()
        .any(|field| field.name == "groupBy"));

    // The structured trace search narrows by service, name, status and duration.
    let traces = open(
        &link,
        "/apps/monitor/Traces?from=now-1h&thru=now&serviceName=monitor-tui&spanName=TUI*&statusCode=ERROR&minDuration=1&maxDuration=500",
    );
    assert_eq!(traces.session().list_page("TraceList").len(), 1);
    for narrowed in ["statusCode=OK", "minDuration=20", "serviceName=other-service"] {
        let view = open(&link, &format!("/apps/monitor/Traces?from=now-1h&thru=now&{narrowed}"));
        assert!(view.session().list_page("TraceList").is_empty(), "{narrowed}");
    }

    // Errors charts the failed request spans over time.
    let errors = open(&link, "/apps/monitor/Errors?from=now-1h&thru=now");
    let chart = all_props(&errors, "report-panel")
        .into_iter()
        .find(|props| props["panel"]["title"] == "Failed requests and jobs over time")
        .expect("the errors chart");
    assert_eq!(field_sum(&chart["frame"], "count"), 1.0, "{chart}");

    // Services drills from a service to its endpoints and instances by data links; an endpoint leaves the artifact panels whole.
    let services = open(
        &link,
        "/apps/monitor/Services?from=now-1h&thru=now&var-service=monitor-tui",
    );
    let dashboard = props(&services, "report-dashboard")["dashboard"].clone();
    let panels = &dashboard["panels"];
    for id in [
        "summary",
        "endpoints",
        "instances",
        "rate",
        "errors",
        "duration",
        "artifacts",
    ] {
        assert!(
            panels[id]["notices"]
                .as_array()
                .is_some_and(|notices| notices.iter().all(|notice| notice["level"] != "error")),
            "{id}: {}",
            panels[id]
        );
    }
    let summary = &panels["summary"]["frames"][0];
    assert_eq!(field_sum(summary, "requests"), 9.0, "{summary}");
    assert_eq!(field_sum(summary, "errors"), 2.0, "{summary}");
    let instances = &panels["instances"]["frames"][0];
    assert_eq!(
        field_values(instances, "instance"),
        vec![json!("tui-canary"), json!("tui-instance")],
        "{instances}"
    );
    assert!(
        field_sum(&panels["duration"]["frames"][0], "latency.p95") > 0.0,
        "{}",
        panels["duration"]
    );
    let links = serde_json::to_string(&panels["endpoints"]).unwrap();
    assert!(
        links.contains("/apps/monitor/Services?var-service=${service}&var-endpoint=${endpoint}"),
        "{links}"
    );
    let endpoint = open(
        &link,
        "/apps/monitor/Services?from=now-1h&thru=now&var-service=monitor-tui&var-endpoint=%2Fnative-monitor&var-instance=tui-canary",
    );
    let panels = props(&endpoint, "report-dashboard")["dashboard"]["panels"].clone();
    assert_eq!(
        field_sum(&panels["summary"]["frames"][0], "requests"),
        5.0,
        "{}",
        panels["summary"]
    );
    let other = open(
        &link,
        "/apps/monitor/Services?from=now-1h&thru=now&var-endpoint=%2Fno-such-endpoint",
    );
    let panels = props(&other, "report-dashboard")["dashboard"]["panels"].clone();
    // The server's own artifact metrics join the fixture's, so this is at least the fixture's hits.
    assert!(
        field_sum(&panels["artifacts"]["frames"][0], "hits") >= 3.0,
        "{}",
        panels["artifacts"]
    );

    // Compare draws rate, errors and duration for each version side by side.
    let split = open(
        &link,
        "/apps/monitor/Split?leftVersion=1.0&rightVersion=2.0&from=now-1h&thru=now",
    );
    let charts = all_props(&split, "report-panel");
    let chart = |title: &str| {
        charts
            .iter()
            .find(|props| props["panel"]["title"] == title)
            .unwrap_or_else(|| panic!("no {title} chart: {charts:?}"))
            .clone()
    };
    assert_eq!(field_sum(&chart("Requests · 1.0")["frame"], "requests"), 4.0);
    assert_eq!(field_sum(&chart("Requests · 2.0")["frame"], "requests"), 5.0);
    assert_eq!(field_sum(&chart("Server errors (5xx) · 2.0")["frame"], "errors"), 2.0);
    assert_eq!(field_sum(&chart("Server errors (5xx) · 1.0")["frame"], "errors"), 0.0);
    assert!(field_sum(&chart("Duration percentiles · 2.0")["frame"], "latency.p50") > 0.0);

    // With only MONITOR_APP, Logs, its surrounding entries and Compare work through the Monitor mounts.
    let monitor_only = HostLink::as_user(factory, "monitor.only");
    let mut logs = open(
        &monitor_only,
        "/apps/monitor/Logs?from=now-1h&thru=now&queryString=body%3Atui-marker",
    );
    let contexts = row_urls(&logs, "ExploreDocuments", "LogContext");
    assert!(
        !contexts.is_empty() && contexts.iter().all(|url| url.starts_with("/apps/monitor/LogContext?")),
        "{contexts:?}"
    );
    logs.session_mut().navigate(&contexts[0], true);
    logs.run_requests();
    assert!(
        logs.session().path().ends_with("/monitor/LogContext") && !logs.session().message_is_error(),
        "{}: {}",
        logs.session().path(),
        logs.session().message()
    );
    let text = logs.screen_text(180, 80);
    assert!(text.contains("tui-marker line"), "{text}");
    let split = open(
        &monitor_only,
        "/apps/monitor/Split?leftVersion=1.0&rightVersion=2.0&from=now-1h&thru=now",
    );
    let errors = all_props(&split, "report-panel")
        .into_iter()
        .find(|props| props["panel"]["title"] == "Server errors (5xx) · 2.0")
        .expect("the monitor-only version comparison");
    assert_eq!(field_sum(&errors["frame"], "errors"), 2.0, "{errors}");
    let mut denied = AppView::with_session(
        Session::start("/apps/dashboards/LogContext?datasetId=otel_logs&documentType=otel_logs&documentId=tui-log-1"),
        monitor_only.clone(),
    );
    denied.run_requests();
    let text = denied.screen_text(180, 80);
    assert!(
        !text.contains("tui-marker line"),
        "the Dashboards mount still needs DASHBOARDS_APP: {text}"
    );
}

fn all_props(view: &AppView<HostLink>, name: &str) -> Vec<Json> {
    view.session()
        .index()
        .components
        .values()
        .filter(|info| info.name == name)
        .map(|info| info.props.clone())
        .collect()
}
fn field_values(frame: &Json, name: &str) -> Vec<Json> {
    frame["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|field| field["name"] == name)
        .and_then(|field| field["values"].as_array().cloned())
        .unwrap_or_default()
}
fn field_names(frame: &Json) -> Vec<String> {
    frame["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|field| field["name"].as_str().map(str::to_owned))
        .collect()
}
fn field_sum(frame: &Json, name: &str) -> f64 {
    field_values(frame, name).iter().filter_map(Json::as_f64).sum()
}
