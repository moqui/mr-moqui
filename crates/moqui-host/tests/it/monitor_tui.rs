//! Every monitoring/report screen must render through HostLink and expose its native interactions.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]
use crate::common;
use moqui_core::{
    entity::document::Document,
    native::{Parameters, ServiceInfo},
    search,
    transaction::TransactionAttribute,
};
use moqui_host::tui::HostLink;
use moqui_lib::object::Value;
use moqui_report as report;
use moqui_tui::{AppView, Key, KeyPress, Node, ScreenModel, Session};
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;

const TRACE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The Evals fixture, run as john.doe: the rows the screens' own transitions make (entity-auto creates) and the eval
/// services for the rest.
const EVAL_FIXTURE: &str = r#"
    const S = 'org.moqui.agent.AgentEvalServices.'
    const call = (name, parameters) => ec.service.sync().name(name).parameters(parameters).call() || {}
    // an entity-auto create without authorization, as the screens' transitions make them
    const make = (entity, parameters) => ec.service.sync().name('create#' + entity).parameters(parameters).disableAuthz().call() || {}
    make('moqui.agent.AgentDefinition', {agentId: 'TUI_EVAL_AGENT', agentName: 'TUI eval agent', surfaces: 'job'})
    make('moqui.agent.AgentScorer', {scorerId: 'TUI_SCORER', scorerName: 'TUI says ships', scorerTypeEnumId: 'AgstService',
        serviceName: S + 'score#Contains', configJson: '{"terms": ["ships"]}', passThreshold: 0.5, createdDate: ec.user.nowTimestamp})
    make('moqui.agent.AgentDataset', {datasetId: 'TUI_EVAL_DS', datasetName: 'TUI eval cases', description: 'TUI dataset',
        agentId: 'TUI_EVAL_AGENT', versionNum: 0, createdDate: ec.user.nowTimestamp})
    call(S + 'add#DatasetItem', {datasetId: 'TUI_EVAL_DS', inputText: 'TUI first case', groundTruth: 'No expected output'})
    call(S + 'add#DatasetItem', {datasetId: 'TUI_EVAL_DS', inputText: 'TUI second case', groundTruth: 'No phrases'})
    for (const scorerId of ['CONTAINS', 'EXACT_MATCH'])
        make('moqui.agent.AgentScorerAttach', {scorerId, datasetId: 'TUI_EVAL_DS', fromDate: '2000-01-01 00:00:00'})
    // score#ExactMatch answers 'No expected output' and score#Contains 'No phrases to look for': two fixed outputs
    call(S + 'start#Experiment', {datasetId: 'TUI_EVAL_DS', serviceName: S + 'score#ExactMatch', experimentName: 'TUI baseline'})
    call(S + 'start#Experiment', {datasetId: 'TUI_EVAL_DS', serviceName: S + 'score#Contains', experimentName: 'TUI candidate',
        notes: 'TUI candidate notes'})
    make('moqui.agent.AgentRun', {runId: 'TUI_EVAL_RUN', agentId: 'TUI_EVAL_AGENT', userId: ec.user.userId, statusId: 'AgrCompleted',
        submittedDate: ec.user.nowTimestamp, endedDate: ec.user.nowTimestamp, resultJson: JSON.stringify({text: 'The TUI order ships Tuesday.'})})
    const message = make('moqui.agent.AgentMessage', {conversationId: 'TUI_EVAL_CONV', messageSeq: 1, runId: 'TUI_EVAL_RUN',
        role: 'user', createdDate: ec.user.nowTimestamp})
    make('moqui.agent.AgentMessagePart', {messageId: message.messageId, partSeq: 1, partKindEnumId: 'AgpkText',
        textValue: 'When does the TUI order ship?'})
    call(S + 'score#Run', {runId: 'TUI_EVAL_RUN', scorerId: 'TUI_SCORER'})
    make('moqui.agent.AgentScorerAttach', {scorerId: 'TUI_SCORER', agentId: 'TUI_EVAL_AGENT', samplingRate: 1,
        fromDate: '2000-01-01 00:00:00', lastScoredDate: ec.user.nowTimestamp})
    if (ec.message.hasError()) throw new Error('Evals fixture: ' + ec.message.getErrorsString())
"#;

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
    open_at(link, path, path.split('?').next().unwrap())
}
/// [`open`] for a screen that lands on `landing`, as a tab panel on its default tab.
fn open_at(link: &HostLink, path: &str, landing: &str) -> AppView<HostLink> {
    let mut view = AppView::with_session(Session::start(path), link.clone());
    view.run_requests();
    assert_eq!(view.session().path(), landing, "{path}: {}", view.session().message());
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
    if !crate::common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start_with_data(
        "monitor-tui",
        r#"<entity-facade-xml type="seed">
      <moqui.security.UserAccount userId="MONITOR_ONLY" username="monitor.only" disabled="N"/>
      <moqui.security.UserGroup userGroupId="MONITOR_ONLY" description="Monitor app only"/>
      <moqui.security.UserGroupMember userGroupId="MONITOR_ONLY" userId="MONITOR_ONLY" fromDate="0"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="MONITOR_ONLY_APP" artifactGroupId="SYSTEM_APP" userGroupId="MONITOR_ONLY" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
      <moqui.monitor.MonitorIncident incidentId="TUI_INCIDENT" title="TUI incident" severity="major" statusId="Open" source="user" serviceName="monitor-tui" version="2.0" causeStatus="hypothesis"/>
      <moqui.monitor.MonitorIncidentCause incidentId="TUI_INCIDENT" causeSeqId="01" causeRank="1" kind="release" refId="monitor-tui:2.0" title="Release 2.0 (tui)" score="0.9" supportingJson='["every instance of 2.0 regressed"]' contradictingJson="[]" statusId="hypothesis"/>
      <moqui.monitor.MonitorIncidentEvidence incidentId="TUI_INCIDENT" evidenceSeqId="01" evidenceDate="2026-10-30 12:00:00" kind="trace" refId="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" summary="TUI monitored request failed"/>
      <moqui.monitor.RecoveryDecision decisionId="TUI_DECISION" incidentId="TUI_INCIDENT" policyId="DEFAULT" action="isolate-instance" target="tui-canary" adapterId="IN_PROCESS" scopeKey="recovery:monitor-tui" statusId="AwaitingApproval" blastRadius="1 of 2 instances" decidedBy="system" decidedDate="2026-10-30 12:01:00"/>
      <moqui.monitor.RecoveryAction actionId="TUI_ACTION" decisionId="TUI_DECISION" incidentId="TUI_INCIDENT" action="escalate" adapterId="IN_PROCESS" scopeKey="recovery:monitor-tui" statusId="Succeeded" createdDate="2026-10-30 12:00:30" resultingState="escalated"/>
      <moqui.entity.document.DataDocument dataDocumentId="TuiBusinessDoc" documentName="TUI business document" primaryEntityName="moqui.basic.Enumeration" indexName="tui_business"/>
      <moqui.entity.document.DataDocumentField dataDocumentId="TuiBusinessDoc" fieldSeqId="01" fieldPath="enumId"/>
      <moqui.entity.document.DataDocumentField dataDocumentId="TuiBusinessDoc" fieldSeqId="02" fieldPath="enumTypeId"/>
      <moqui.entity.feed.DataFeed dataFeedId="TuiBusinessFeed" dataFeedTypeEnumId="DTFDTP_MAN_PULL" feedName="TUI business feed"/>
      <moqui.entity.feed.DataFeedDocument dataFeedId="TuiBusinessFeed" dataDocumentId="TuiBusinessDoc"/>
      <moqui.screen.ScreenScheduled screenScheduledId="TUI_SCHEDULE" screenPath="/apps/dashboards/Reports" reportDefinitionId="tui-dashboard" reportOwnerId="EX_JOHN_DOE" userId="EX_JOHN_DOE" cronExpression="0 0 6 ? * MON" renderMode="csv" deliveryMode="personal" emailSubject="TUI weekly schedule" paused="Y" noResultsAbort="Y" versionNumber="1"/>
      <moqui.screen.ScreenScheduledLock screenScheduledId="TUI_SCHEDULE"/>
      <moqui.report.ReportOccurrence occurrenceId="TUI_OCCURRENCE" screenScheduledId="TUI_SCHEDULE" occurrenceKey="TUI_OCCURRENCE" occurrenceKind="manual" occurrenceDate="2026-10-30 12:00:00" ownerUserId="EX_JOHN_DOE" scheduleRevision="1" definitionId="tui-dashboard" definitionVersion="1" renderMode="csv" deliveryMode="personal" statusId="RptOccComplete" recipientCount="1" sentCount="1" skippedCount="0" failedCount="0" uncertainCount="0" renderCount="1" versionNumber="1"/>
      <moqui.report.ReportRecipientDelivery deliveryId="TUI_DELIVERY" occurrenceId="TUI_OCCURRENCE" userId="EX_JOHN_DOE" channel="email" statusId="RptDlvSent" attemptCount="1" attemptDate="2026-10-30 12:00:01"/>
      <moqui.monitor.MonitorRegression regressionId="TUI_REGRESSION" serviceName="monitor-tui" metric="error_rate" baselineVersion="1.0" candidateVersion="2.0" baselineValue="0" candidateValue="0.4" magnitude="9" confidence="0.999" severity="critical" dataQuality="good" locality="unknown" statusId="Open" incidentId="TUI_INCIDENT"/>
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
    facade.index_entries("monitor_annotations",["1.0","2.0"].into_iter().map(|version|document(json!({"_id":format!("release-tui-{version}"),"time":now-3_600_000,"source":"release","scope":"service","service.name":"monitor-tui","service.version":version,"service.instance.id":"tui-instance","vcs.ref.head.revision":"tui","text":format!("monitor-tui {version}"),"tags":["release"]}))).collect()).unwrap();
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
            json!({"id":"tui-business","type":"dataset","title":"TUI business dataset","body":{"documentType":"TuiBusinessDoc","timeField":"_timestamp","signal":"business","rowFilterService":"org.moqui.report.ReportServices.get#UserGroupRowFilter","rowFilterParameters":{"groupField":"enumTypeId"}}}),
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
        ("Monitor/Overview", "/apps/system/Monitor/Overview?from=now-1h&thru=now"),
        ("Monitor/Services", "/apps/system/Monitor/Services?from=now-1h&thru=now"),
        ("Monitor/Operations", "/apps/system/Monitor/Operations?from=now-1h&thru=now"),
        ("Monitor/Releases", "/apps/system/Monitor/Releases"),
        ("Monitor/ReleaseDetail", "/apps/system/Monitor/ReleaseDetail?serviceName=monitor-tui&version=1.0"),
        ("Monitor/Incidents", "/apps/system/Monitor/Incidents"),
        ("Monitor/IncidentDetail", "/apps/system/Monitor/IncidentDetail?incidentId=TUI_INCIDENT"),
        ("Monitor/Recovery", "/apps/system/Monitor/Recovery"),
        // its subscreens are walked by evals_screens_work_in_the_terminal
        ("Llm/Evals", "/apps/system/Llm/Evals"),
        ("Monitor/Settings", "/apps/system/Monitor/Settings"),
        ("Monitor/Artifacts", "/apps/system/Monitor/Artifacts?from=now-1h&thru=now"),
        (
            "Monitor/Traces",
            "/apps/system/Monitor/Traces?from=now-1h&thru=now&serviceName=monitor-tui&spanName=TUI*&statusCode=ERROR&minDuration=1&maxDuration=500",
        ),
        (
            "Monitor/TraceDetail",
            "/apps/system/Monitor/TraceDetail?traceId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&from=now-1h&thru=now",
        ),
        ("Monitor/Logs", "/apps/system/Monitor/Logs?from=now-1h&thru=now"),
        ("Monitor/AgentTraces", "/apps/system/Monitor/AgentTraces?from=now-1h&thru=now"),
        ("Monitor/Checks", "/apps/system/Monitor/Checks"),
        (
            "Monitor/Metrics",
            "/apps/system/Monitor/Metrics?from=now-1h&thru=now&metricName=http.server.request.duration&metricStat=p95&groupBy=service.version",
        ),
        ("Monitor/Errors", "/apps/system/Monitor/Errors?from=now-1h&thru=now"),
        (
            "Monitor/ErrorDetail",
            "/apps/system/Monitor/ErrorDetail?fingerprint=tui-error&from=now-1h&thru=now",
        ),
        ("Monitor/Sources", "/apps/system/Monitor/Sources"),
        ("Monitor/Browser", "/apps/system/Monitor/Browser?from=now-1h&thru=now"),
        ("Monitor/Replays", "/apps/system/Monitor/Replays"),
        (
            "Monitor/ReplayDetail",
            "/apps/system/Monitor/ReplayDetail?replayId=0123abcd-tui-replay-fixture",
        ),
        (
            "Monitor/LogContext",
            "/apps/system/Monitor/LogContext?datasetId=otel_logs&documentType=otel_logs&documentId=tui-log-1&from=now-1h&thru=now",
        ),
        (
            "Monitor/Split",
            "/apps/system/Monitor/Split?leftVersion=1.0&rightVersion=2.0&from=now-1h&thru=now",
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
            "Dashboards/DatasetDetail",
            "/apps/dashboards/DatasetDetail?datasetId=tui-business",
        ),
        (
            "Dashboards/DatasetFields",
            "/apps/dashboards/DatasetFields?datasetId=tui-business",
        ),
        (
            "Dashboards/DatasetView",
            "/apps/dashboards/DatasetView?datasetId=tui-business",
        ),
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
        ("Dashboards/Schedules", "/apps/dashboards/Schedules"),
        (
            "Dashboards/ScheduledReportDetail",
            "/apps/dashboards/ScheduledReportDetail?screenScheduledId=TUI_SCHEDULE",
        ),
        (
            "Dashboards/ReportDelivery",
            "/apps/dashboards/ReportDelivery?occurrenceId=TUI_OCCURRENCE",
        ),
        (
            "Dashboards/Playlist",
            "/apps/dashboards/Playlist?definitionId=tui-playlist",
        ),
        ("Dashboards/Playlists", "/apps/dashboards/Playlists"),
        (
            "Dashboards/WallDisplay",
            "/apps/dashboards/WallDisplay?definitionId=tui-playlist",
        ),
        (
            "Dashboards/Analysis",
            "/apps/dashboards/Analysis?datasetId=otel_logs&run=Run&from=now-1h&thru=now",
        ),
        (
            "Dashboards/Drill",
            "/apps/dashboards/Drill?definitionId=tui-dashboard&panelId=a-total&from=now-1h&thru=now",
        ),
    ];
    // A new screen without an acceptance case is a failed gate, rather than silently missing coverage.
    let covered: BTreeSet<_> = cases.iter().map(|(name, _)| name.to_string()).collect();
    let mut actual = BTreeSet::new();
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/tools/screen");
    for (directory, folder) in [("Monitor", "System/Monitor"), ("Dashboards", "Dashboards")] {
        for file in std::fs::read_dir(root.join(folder)).unwrap() {
            let file = file.unwrap().path();
            if file.extension().is_some_and(|extension| extension == "xml") {
                actual.insert(format!("{directory}/{}", file.file_stem().unwrap().to_string_lossy()));
            }
        }
    }
    // Evals is under LLM and Agents; its screens have their own test below.
    actual.insert("Llm/Evals".to_string());
    assert_eq!(covered, actual, "every new screen needs a fixture and native traversal");
    let mut seen = BTreeSet::new();
    let mut failures = Vec::new();
    for (name, path) in cases {
        // Independent views are checked together; one failure does not hide the rest.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // a tab panel opens on its default tab
            let mut view = match name {
                "Llm/Evals" => open_at(&link, path, "/apps/system/Llm/Evals/Experiments"),
                _ => open(&link, path),
            };
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
    // The dataset screens: the business dataset with its state above Tools' other datasets, and its steward's detail.
    let datasets = open(&link, "/apps/dashboards/Datasets").screen_text(220, 120);
    assert!(
        datasets.contains("TUI business dataset")
            && datasets.contains("Needs review")
            && datasets.contains("Other datasets"),
        "{datasets}"
    );
    let detail = open(&link, "/apps/dashboards/DatasetDetail?datasetId=tui-business").screen_text(220, 120);
    assert!(
        detail.contains("Admission checks") && detail.contains("user groups on enumTypeId"),
        "{detail}"
    );
    // The delivery screens: the viewer's schedule on the list, its recipients and runs, and a run's outcomes.
    let schedules = open(&link, "/apps/dashboards/Schedules").screen_text(220, 120);
    assert!(
        schedules.contains("TUI weekly schedule") && schedules.contains("New schedule") && schedules.contains("Paused"),
        "{schedules}"
    );
    let schedule = open(
        &link,
        "/apps/dashboards/ScheduledReportDetail?screenScheduledId=TUI_SCHEDULE",
    )
    .screen_text(220, 160);
    assert!(
        schedule.contains("TUI acceptance dashboard")
            && schedule.contains("Sent to you alone.")
            && schedule.contains("1 sent"),
        "{schedule}"
    );
    let run = open(&link, "/apps/dashboards/ReportDelivery?occurrenceId=TUI_OCCURRENCE").screen_text(220, 120);
    assert!(
        run.contains("Schedule revision 1, report version 1") && run.contains("Your delivery") && run.contains("Sent"),
        "{run}"
    );
    // The alert screens: the viewer's rule with its kind and whom it tells beside the shared contact point, and in the
    // editor its reading, its state and the shared item it names.
    let alerts = open(&link, "/apps/dashboards/Alerts").screen_text(220, 200);
    assert!(
        alerts.contains("TUI paused alert")
            && alerts.contains("Operational")
            && alerts.contains("1 contact point")
            && alerts.contains("TUI shared contact"),
        "{alerts}"
    );
    let rule = open(&link, "/apps/dashboards/AlertRule?definitionId=tui-alert").screen_text(220, 240);
    assert!(
        rule.contains("Reading and condition")
            && rule.contains("Contact points")
            && rule.contains("TUI shared contact")
            && rule.contains("Paused"),
        "{rule}"
    );
    // The AI screen groups LLM calls by trace and names each by its agent frame.
    let ai = open(&link, "/apps/system/Monitor/AgentTraces?from=now-1h&thru=now").screen_text(220, 80);
    assert!(
        ai.contains("agent:tui-helper") && ai.contains("tui-model") && ai.contains("120"),
        "{ai}"
    );
    // Log patterns group the fixture's three lines into one template.
    let patterns = open(
        &link,
        "/apps/system/Monitor/Logs?from=now-1h&thru=now&showPatterns=Y&queryString=body%3Atui-marker",
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
        "/apps/system/Monitor/TraceDetail?traceId=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&from=now-1h&thru=now",
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
        "/apps/system/Monitor/Logs?from=now-1h&thru=now&queryString=body%3Atui-marker",
    );
    let tail = props(&logs, "log-tail");
    assert!(
        tail["source"]
            .as_str()
            .is_some_and(|url| url.starts_with("/apps/system/Monitor/Logs/tail")),
        "{tail}"
    );
    let traces = row_urls(&logs, "ExploreDocuments", "TraceDetail");
    assert!(
        !traces.is_empty() && traces.iter().all(|url| url.starts_with("/apps/system/Monitor/TraceDetail?")),
        "{traces:?}"
    );
    let contexts = row_urls(&logs, "ExploreDocuments", "LogContext");
    assert!(
        !contexts.is_empty() && contexts.iter().all(|url| url.starts_with("/apps/system/Monitor/LogContext?")),
        "{contexts:?}"
    );
    logs.session_mut().navigate(&traces[0], true);
    logs.run_requests();
    assert!(
        logs.session().path().ends_with("/Monitor/TraceDetail"),
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
    let mut overview = open(&link, "/apps/system/Monitor/Overview?panelId=errors&from=now-1h&thru=now");
    assert_eq!(
        props(&overview, "report-dashboard")["dashboard"]["screenUrl"],
        "/apps/system/Monitor/Overview"
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
    assert_eq!(overview.session().path(), "/apps/system/Monitor/Overview");
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
    let mut sources = open(&link, "/apps/system/Monitor/Sources");
    set_field(&mut sources, "CreateIngestKey.source", "tui-token-check");
    press(&mut sources, Key::Enter);
    sources.run_requests();
    let message = sources.session().message().to_string();
    assert!(
        message.contains("for tui-token-check") && message.contains("token: mqi_"),
        "{message}"
    );
    assert!(sources.session().path().ends_with("/Monitor/Sources"));

    // The Metrics picker charts the picked metric's percentile split by the chosen attribute.
    let metrics = open(
        &link,
        "/apps/system/Monitor/Metrics?from=now-1h&thru=now&metricName=http.server.request.duration&metricStat=p95&groupBy=service.version",
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
        "/apps/system/Monitor/Traces?from=now-1h&thru=now&serviceName=monitor-tui&spanName=TUI*&statusCode=ERROR&minDuration=1&maxDuration=500",
    );
    assert_eq!(traces.session().list_page("TraceList").len(), 1);
    for narrowed in ["statusCode=OK", "minDuration=20", "serviceName=other-service"] {
        let view = open(&link, &format!("/apps/system/Monitor/Traces?from=now-1h&thru=now&{narrowed}"));
        assert!(view.session().list_page("TraceList").is_empty(), "{narrowed}");
    }

    // Errors charts the failed request spans over time.
    let errors = open(&link, "/apps/system/Monitor/Errors?from=now-1h&thru=now");
    let chart = all_props(&errors, "report-panel")
        .into_iter()
        .find(|props| props["panel"]["title"] == "Failed requests and jobs over time")
        .expect("the errors chart");
    assert_eq!(field_sum(&chart["frame"], "count"), 1.0, "{chart}");

    // Services drills from a service to its endpoints and instances by data links; an endpoint leaves the artifact panels whole.
    let services = open(
        &link,
        "/apps/system/Monitor/Services?from=now-1h&thru=now&var-service=monitor-tui",
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
        links.contains("/apps/system/Monitor/Services?var-service=${service}&var-endpoint=${endpoint}"),
        "{links}"
    );
    let endpoint = open(
        &link,
        "/apps/system/Monitor/Services?from=now-1h&thru=now&var-service=monitor-tui&var-endpoint=%2Fnative-monitor&var-instance=tui-canary",
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
        "/apps/system/Monitor/Services?from=now-1h&thru=now&var-endpoint=%2Fno-such-endpoint",
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
        "/apps/system/Monitor/Split?leftVersion=1.0&rightVersion=2.0&from=now-1h&thru=now",
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

    // With only SYSTEM_APP (no DASHBOARDS_APP), Logs, its surrounding entries and Compare work through the Monitor mounts.
    let monitor_only = HostLink::as_user(factory, "monitor.only");
    let mut logs = open(
        &monitor_only,
        "/apps/system/Monitor/Logs?from=now-1h&thru=now&queryString=body%3Atui-marker",
    );
    let contexts = row_urls(&logs, "ExploreDocuments", "LogContext");
    assert!(
        !contexts.is_empty() && contexts.iter().all(|url| url.starts_with("/apps/system/Monitor/LogContext?")),
        "{contexts:?}"
    );
    logs.session_mut().navigate(&contexts[0], true);
    logs.run_requests();
    assert!(
        logs.session().path().ends_with("/Monitor/LogContext") && !logs.session().message_is_error(),
        "{}: {}",
        logs.session().path(),
        logs.session().message()
    );
    let text = logs.screen_text(180, 80);
    assert!(text.contains("tui-marker line"), "{text}");
    let split = open(
        &monitor_only,
        "/apps/system/Monitor/Split?leftVersion=1.0&rightVersion=2.0&from=now-1h&thru=now",
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

/// Press the submit button `id` of a form and wait for the answer, which must not be an error.
fn submit(view: &mut AppView<HostLink>, id: &str) {
    focus(view, id);
    press(view, Key::Enter);
    view.run_requests();
    assert!(!view.session().message_is_error(), "{id}: {}", view.session().message());
}
/// Follow the link of the current screen whose url holds `fragment`.
fn follow(view: &mut AppView<HostLink>, fragment: &str) {
    let id = link_id(view, fragment);
    focus(view, &id);
    press(view, Key::Enter);
    view.run_requests();
}
/// F10 on the list `list` at its current row, then the action whose label starts with `label`.
fn row_action(view: &mut AppView<HostLink>, list: &str, label: &str) {
    focus(view, list);
    press(view, Key::F(10));
    let Some(moqui_tui::Overlay::Menu(menu)) = view.session().top_overlay() else {
        panic!("no actions on {list}: {:?}", view.session().overlays());
    };
    let at = menu
        .items
        .iter()
        .position(|item| item.label.starts_with(label))
        .unwrap_or_else(|| panic!("no {label} on {list}: {:?}", menu.items));
    for _ in 0..at {
        press(view, Key::Down);
    }
    press(view, Key::Enter);
    view.run_requests();
}
/// Whether `text` holds epoch millis: a run of 13 digits.
fn has_millis(text: &str) -> bool {
    text.split(|ch: char| !ch.is_ascii_digit()).any(|run| run.len() == 13)
}
fn rows_text(view: &AppView<HostLink>, list: &str) -> String {
    serde_json::to_string(&view.session().list_page(list)).unwrap()
}
fn at(view: &AppView<HostLink>, path: &str) {
    assert_eq!(view.session().path(), path, "{}", view.session().message());
}

/// Whether `text` holds a date and time as `yyyy-MM-dd HH:mm` draws it.
fn has_date(text: &str) -> bool {
    text.as_bytes().windows(16).any(|window| {
        window.iter().enumerate().all(|(at, byte)| match at {
            4 | 7 => *byte == b'-',
            10 => *byte == b' ',
            13 => *byte == b':',
            _ => byte.is_ascii_digit(),
        })
    })
}
/// The url of the link of the current screen whose url holds `fragment`.
fn link_url(view: &AppView<HostLink>, fragment: &str) -> String {
    let mut found = None;
    moqui_tui::walk(&view.session().model().c, &mut |node| {
        if let Node::Link { url, .. } = node {
            if url.contains(fragment) {
                found.get_or_insert_with(|| url.clone());
            }
        }
    });
    found.unwrap_or_else(|| panic!("no {fragment} link on {}", view.session().path()))
}

/// LLM and Agents › Evals in the terminal; the gate above covers only its tab panel. Each screen is drawn once, but where a
/// form's answer comes back to it: the tabs, the row links to the detail screens, and every form (start an experiment,
/// a review, compare, create a dataset, add and remove an item, remove and attach a scorer, create a scorer, live
/// scoring, find, score and save a run).
#[test]
fn evals_screens_work_in_the_terminal() {
    if !crate::common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start("monitor-tui-evals");
    let factory = gate.server.factory();
    {
        let context = factory.new_context();
        let _entered = context.enter();
        // john.doe holds MONITOR_EVAL and MONITOR_EVAL_RUN through ADMIN (AgentEvalData.xml). A scorer, a dataset with
        // two items and the scorers attached, two service-target experiments of it (which complete at once) for Compare,
        // and a scored agent run with live scoring for Runs and Scorers.
        moqui_core::user::internal_login_user("john.doe").unwrap();
        moqui_core::script::run_script(EVAL_FIXTURE).unwrap();
    }
    // by name: TUI baseline, TUI candidate
    let experiments: Vec<String> = {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let mut rows = factory
            .entity()
            .unwrap()
            .find("moqui.agent.AgentExperiment")
            .unwrap()
            .eq("datasetId", Value::string("TUI_EVAL_DS"))
            .list()
            .unwrap()
            .values()
            .iter()
            .map(|row| {
                (
                    moqui_lib::object::to_plain_string(row.get("experimentName").unwrap()),
                    moqui_lib::object::to_plain_string(row.get("experimentId").unwrap()),
                )
            })
            .collect::<Vec<_>>();
        rows.sort();
        rows.into_iter().map(|(_, id)| id).collect()
    };
    assert_eq!(experiments.len(), 2, "{experiments:?}");
    let link = HostLink::as_user(factory, "john.doe");
    const E: &str = "/apps/system/Llm/Evals/";

    // Experiments, the tab panel's default: its rows link to each experiment and to Compare from it.
    let mut view = open_at(&link, "/apps/system/Llm/Evals", &format!("{E}Experiments"));
    let text = view.screen_text(100, 40);
    assert!(
        text.contains("1:Experiments") && text.contains("4:Runs") && text.contains("TUI baseline"),
        "{text}"
    );
    let details = row_urls(&view, "Experiments", "/ExperimentDetail?");
    let compares = row_urls(&view, "Experiments", "/Compare?");
    for id in &experiments {
        assert!(
            details
                .iter()
                .any(|url| url.starts_with(&format!("{E}ExperimentDetail?"))
                    && url.contains(&format!("experimentId={id}"))),
            "{details:?}"
        );
        assert!(
            compares
                .iter()
                .any(|url| url.starts_with(&format!("{E}Compare?")) && url.contains(&format!("baseId={id}"))),
            "{compares:?}"
        );
    }
    // Start an experiment: a service target completes at once, on its own screen.
    set_field(&mut view, "StartExperiment.datasetId", "TUI_EVAL_DS");
    set_field(
        &mut view,
        "StartExperiment.serviceName",
        "org.moqui.agent.AgentEvalServices.score#ExactMatch",
    );
    set_field(&mut view, "StartExperiment.experimentName", "TUI started here");
    submit(&mut view, "StartExperiment.submitButton");
    at(&view, &format!("{E}ExperimentDetail"));
    let started = view.session().parameters()["experimentId"].clone();
    // its summary, and each result's scores, a line per scorer
    let text = view.screen_text(240, 60);
    assert!(
        text.contains("TUI started here") && text.contains("Completed · 2/2 done"),
        "{text}"
    );
    assert!(
        text.contains("Contains: ") && text.contains("Matches expected: 0.00"),
        "{text}"
    );
    assert!(link_url(&view, "/DatasetDetail?").contains("datasetId=TUI_EVAL_DS"));
    // A person's review is a score of the result.
    let result = view.session().index().choices("ReviewResult.resultId")[0].value.clone();
    set_field(&mut view, "ReviewResult.resultId", &result);
    set_field(&mut view, "ReviewResult.scorerId", "HUMAN_REVIEW");
    set_field(&mut view, "ReviewResult.score", "1");
    set_field(&mut view, "ReviewResult.reason", "TUI reviewed");
    submit(&mut view, "ReviewResult.submitButton");
    at(&view, &format!("{E}ExperimentDetail"));
    assert!(rows_text(&view, "ScorerSummary").contains("Human review"));
    // Compare with another, then pick the baseline.
    follow(&mut view, "/Compare?");
    at(&view, &format!("{E}Compare"));
    assert_eq!(view.session().parameters()["baseId"], started);
    set_field(&mut view, "Pick.otherId", &experiments[0]);
    submit(&mut view, "Pick.submitButton");
    at(&view, &format!("{E}Compare"));
    assert_eq!(view.session().parameters()["otherId"], experiments[0]);
    // the change's word column is titled "", as in the browser
    let titles: Vec<String> = view
        .session()
        .list("Compare")
        .unwrap()
        .columns
        .iter()
        .map(|column| column.title.clone())
        .collect();
    assert_eq!(titles, ["Item", "Before", "After", "Change", ""]);
    let compare = rows_text(&view, "Compare");
    assert!(
        compare.contains("TUI first case") && compare.contains("TUI second case"),
        "{compare}"
    );
    let text = view.screen_text(240, 40);
    assert!(
        text.contains("TUI started here: mean") && text.contains("TUI baseline: mean"),
        "{text}"
    );

    // Datasets (tab 2): its row links to the dataset; a new one opens on its own screen.
    view.on_key(&KeyPress::with_alt(Key::Char('2')));
    view.run_requests();
    at(&view, &format!("{E}Datasets"));
    let datasets = row_urls(&view, "Datasets", "/DatasetDetail?");
    assert!(
        datasets
            .iter()
            .any(|url| url.starts_with(&format!("{E}DatasetDetail?")) && url.contains("datasetId=TUI_EVAL_DS")),
        "{datasets:?}"
    );
    set_field(&mut view, "CreateDataset.datasetId", "TUI_EVAL_NEW");
    set_field(&mut view, "CreateDataset.datasetName", "TUI new dataset");
    submit(&mut view, "CreateDataset.submitButton");
    at(&view, &format!("{E}DatasetDetail"));
    assert_eq!(view.session().parameters()["datasetId"], "TUI_EVAL_NEW");
    assert!(view
        .screen_text(100, 40)
        .contains("TUI new dataset · version 0 · 0 items"));

    // Scorers (tab 3): a new one, and live scoring of an agent's runs started and stopped.
    view.on_key(&KeyPress::with_alt(Key::Char('3')));
    view.run_requests();
    at(&view, &format!("{E}Scorers"));
    // a time stamp a script put in a row (epoch millis text) shows in its display's date format
    let live = rows_text(&view, "LiveScoring");
    assert!(has_date(&live) && !has_millis(&live), "{live}");
    set_field(&mut view, "CreateScorer.scorerId", "TUI_NEW_SCORER");
    set_field(&mut view, "CreateScorer.scorerName", "TUI new scorer");
    set_field(&mut view, "CreateScorer.scorerTypeEnumId", "AgstHuman");
    submit(&mut view, "CreateScorer.submitButton");
    at(&view, &format!("{E}Scorers"));
    assert!(rows_text(&view, "Scorers").contains("TUI new scorer"));
    set_field(&mut view, "AttachAgent.scorerId", "EXACT_MATCH");
    set_field(&mut view, "AttachAgent.agentId", "TUI_EVAL_AGENT");
    set_field(&mut view, "AttachAgent.samplingRate", "0.5");
    submit(&mut view, "AttachAgent.submitButton");
    assert_eq!(view.session().list_page("LiveScoring").len(), 2);
    let stop = view
        .session()
        .list_page("LiveScoring")
        .iter()
        .position(|row| serde_json::to_string(row).unwrap().contains("Matches expected"))
        .unwrap_or_else(|| panic!("{}", rows_text(&view, "LiveScoring")));
    focus(&mut view, "LiveScoring");
    for _ in 0..stop {
        press(&mut view, Key::Down);
    }
    row_action(&mut view, "LiveScoring", "Stop");
    assert_eq!(
        view.session().list_page("LiveScoring").len(),
        1,
        "{}",
        view.session().message()
    );
    // The live row's Scores opens Runs for its agent.
    focus(&mut view, "LiveScoring");
    press(&mut view, Key::Enter);
    view.run_requests();
    at(&view, &format!("{E}Runs"));
    assert_eq!(view.session().parameters()["agentId"], "TUI_EVAL_AGENT");

    // Runs: find a run, score it, and save it as an item of the dataset.
    set_field(&mut view, "RunFilter.runId", "TUI_EVAL_RUN");
    submit(&mut view, "RunFilter.submitButton");
    assert_eq!(view.session().parameters()["runId"], "TUI_EVAL_RUN");
    let runs = rows_text(&view, "Runs");
    assert!(
        runs.contains("When does the TUI order ship?") && runs.contains("TUI says ships"),
        "{runs}"
    );
    set_field(&mut view, "ScoreRun.runId", "TUI_EVAL_RUN");
    set_field(&mut view, "ScoreRun.scorerId", "EXACT_MATCH");
    submit(&mut view, "ScoreRun.submitButton");
    at(&view, &format!("{E}Runs"));
    assert!(rows_text(&view, "Runs").contains("Matches expected"));
    set_field(&mut view, "SaveRun.runId", "TUI_EVAL_RUN");
    set_field(&mut view, "SaveRun.datasetId", "TUI_EVAL_DS");
    set_field(&mut view, "SaveRun.useOutput", "Y");
    submit(&mut view, "SaveRun.submitButton");

    // The dataset: the saved run is its third item, a new version.
    at(&view, &format!("{E}DatasetDetail"));
    assert_eq!(view.session().parameters()["datasetId"], "TUI_EVAL_DS");
    assert!(view
        .screen_text(100, 40)
        .contains("TUI eval cases · version 3 · 3 items"));
    let items = rows_text(&view, "Items");
    assert!(
        items.contains("When does the TUI order ship?") && items.contains("The TUI order ships Tuesday."),
        "{items}"
    );
    let experiment_rows = rows_text(&view, "DatasetExperiments");
    assert!(
        has_date(&experiment_rows) && !has_millis(&experiment_rows),
        "{experiment_rows}"
    );
    assert_eq!(row_urls(&view, "DatasetExperiments", "/ExperimentDetail?").len(), 3);
    assert!(link_url(&view, "/Experiments?").contains("datasetId=TUI_EVAL_DS"));
    // An item added is a new version; Remove (a row action) asks first, and leaves it out of the next.
    set_field(&mut view, "AddItem.inputText", "TUI fourth case");
    set_field(&mut view, "AddItem.groundTruth", "ships");
    submit(&mut view, "AddItem.submitButton");
    assert!(view.screen_text(100, 40).contains("version 4 · 4 items"));
    focus(&mut view, "Items");
    for _ in 0..3 {
        press(&mut view, Key::Down);
    }
    row_action(&mut view, "Items", "Remove");
    assert!(
        matches!(view.session().top_overlay(), Some(moqui_tui::Overlay::Confirm(_))),
        "{:?}",
        view.session().overlays()
    );
    press(&mut view, Key::Right);
    press(&mut view, Key::Enter);
    view.run_requests();
    assert!(!rows_text(&view, "Items").contains("TUI fourth case"));
    assert!(view.screen_text(100, 40).contains("version 5 · 3 items"));
    // A scorer removed (a row action) and attached again.
    assert_eq!(view.session().list_page("Attached").len(), 2);
    row_action(&mut view, "Attached", "Remove");
    assert_eq!(
        view.session().list_page("Attached").len(),
        1,
        "{}",
        view.session().message()
    );
    let again = view
        .session()
        .index()
        .choices("AttachScorer.scorerId")
        .iter()
        .find(|choice| choice.value == "CONTAINS" || choice.value == "EXACT_MATCH")
        .expect("the removed scorer is offered again")
        .value
        .clone();
    set_field(&mut view, "AttachScorer.scorerId", &again);
    submit(&mut view, "AttachScorer.submitButton");
    assert_eq!(view.session().list_page("Attached").len(), 2);
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
