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
    let gate = common::start_with_data("monitor-tui", "<entity-facade-xml type=\"seed\"/>");
    let factory = gate.server.factory();
    let now = jiff::Timestamp::now().as_millisecond();
    let stamp = jiff::Timestamp::from_millisecond(now - 5000).unwrap().to_string();
    let facade = search::installed(factory).unwrap();
    facade.index_entries("otel_spans",vec![document(json!({"_id":"tui-root","trace_id":TRACE,"span_id":"1111111111111111","name":"TUI monitored request","kind":"SERVER","start":now-5000,"start_nano":(now-5000)*1_000_000,"duration_nano":10000000,"status_code":"ERROR","service.name":"monitor-tui","service.instance.id":"tui-instance","source.id":"tui-source","source.name":"TUI fixture","attributes":{"http.route":"/native-monitor"},"events":[{"name":"exception","attributes":{"exception.message":"tui exception"}}]}))]).unwrap();
    facade.index_entries("otel_logs",(0..3).map(|at|document(json!({"_id":format!("tui-log-{at}"),"timestamp":stamp,"severity_text":"ERROR","severity_number":17,"body":format!("tui-marker line {at}"),"trace_id":TRACE,"span_id":"1111111111111111","service.name":"monitor-tui","service.instance.id":"tui-instance","source.id":"tui-source","source.name":"TUI fixture","thread.name":"tui-worker","user.id":"EX_JOHN_DOE","exception.type":"TuiError","exception.stacktrace":"monitor_tui.rs:42"}))).collect()).unwrap();
    facade.index_entries("otel_errors",vec![document(json!({"_id":"tui-error","fingerprint":"tui-error","message":"TUI failure group","count":3,"first_seen":now-5000,"last_seen":now-5000,"stacktrace":"monitor_tui.rs:42","service.name":"monitor-tui","sample_trace_ids":[TRACE]}))]).unwrap();
    facade.index_entries("otel_metrics",vec![document(json!({"_id":"tui-metric","name":"moqui.artifact.duration","bucket_start":now-60000,"bucket_end":now,"granularity":"minute","count":3,"sum":0.03,"min":0.01,"max":0.01,"moqui.artifact.type":"AT_SERVICE","moqui.artifact.name":"monitor.tui","service.name":"monitor-tui","service.instance.id":"tui-instance"}))]).unwrap();
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
            json!({"id":"tui-alert","type":"alert","title":"TUI paused alert","body":{"query":aggregate,"field":"severity_total","reduce":"last","condition":{"type":"above","threshold":0},"intervalSeconds":60,"paused":true}}),
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
        ("Monitor/Traces", "/apps/monitor/Traces?from=now-1h&thru=now"),
        (
            "Monitor/TraceDetail",
            "/apps/monitor/TraceDetail?traceId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&from=now-1h&thru=now",
        ),
        ("Monitor/Logs", "/apps/monitor/Logs?from=now-1h&thru=now"),
        ("Monitor/Metrics", "/apps/monitor/Metrics?from=now-1h&thru=now"),
        ("Monitor/Errors", "/apps/monitor/Errors?from=now-1h&thru=now"),
        (
            "Monitor/ErrorDetail",
            "/apps/monitor/ErrorDetail?fingerprint=tui-error&from=now-1h&thru=now",
        ),
        ("Monitor/Sources", "/apps/monitor/Sources"),
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
        !contexts.is_empty()
            && contexts
                .iter()
                .all(|url| url.starts_with("/apps/dashboards/LogContext?")),
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
}
