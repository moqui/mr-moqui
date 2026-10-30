//! Real PostgreSQL acceptance tests for the authorized report boundary and telemetry store.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]
use crate::common;
use moqui_core::{
    entity::{document::Document, EntityFacade},
    native::{Parameters, ServiceInfo},
    search,
    transaction::TransactionAttribute,
};
use moqui_lib::object::Value;
use moqui_report as report;
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;

fn document(value: Json) -> Document {
    let Value::Map(map) = moqui_lib::json::from_json(&value) else {
        panic!("document")
    };
    map
}
fn parameters(value: Json) -> Parameters {
    document(value).into_iter().collect()
}
fn result(value: Parameters) -> Json {
    moqui_lib::json::to_json(&Value::Map(value.into_iter().collect()))
}
fn service() -> ServiceInfo<'static> {
    ServiceInfo {
        name: "org.moqui.report.ReportServices.test#Fixture",
        service_type: "rust",
        location: "test",
        method: "",
    }
}
fn row(entity: &EntityFacade, name: &str, fields: Json) {
    let mut row = entity.make_value(name).unwrap();
    for (k, v) in fields.as_object().unwrap() {
        row.set(k.as_str(), moqui_lib::json::from_json(v));
    }
    row.create_or_update(entity).unwrap();
}

/// With `<report week-start="0">` week buckets and `now/w` both start on Sunday, and a rounded `thru` closes at the
/// start of the next unit.
#[test]
fn postgres_sunday_weeks_align_buckets_and_rounding() {
    if !crate::common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start_with_conf("report-week", r#"<report week-start="0"/>"#, &[]);
    let factory = gate.server.factory();
    let facade = search::installed(factory).unwrap();
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let at = |text: &str| text.parse::<jiff::Timestamp>().unwrap().as_millisecond();
    // Rounding is in the viewer's zone, so the expected times are its local times.
    let zone = moqui_core::l10n::time_zone();
    let local = |text: &str| {
        text.parse::<jiff::civil::DateTime>()
            .unwrap()
            .to_zoned(zone.clone())
            .unwrap()
            .timestamp()
    };
    let ms = |text: &str| local(text).as_millisecond();
    let iso = |text: &str| local(text).to_string();
    // 2026-01-07 is a Wednesday; its Sunday week starts 2026-01-04.
    assert_eq!(
        search::time::parse("now/w", ms("2026-01-07T15:00:00")).unwrap(),
        ms("2026-01-04T00:00:00")
    );
    assert_eq!(
        search::time::parse_thru("now/w", ms("2026-01-07T15:00:00")).unwrap(),
        ms("2026-01-11T00:00:00")
    );
    let point = |id: &str, time: &str| {
        document(json!({"_id":id,"timestamp":iso(time),"body":"sundayweekfixture","severity_number":9}))
    };
    facade
        .index_entries(
            "otel_logs",
            vec![
                point("sunday-1", "2026-01-04T01:00:00"),
                point("sunday-2", "2026-01-10T23:00:00"),
                point("sunday-3", "2026-01-11T00:30:00"),
            ],
        )
        .unwrap();
    // Aligned to Sunday weeks the range has no partial bucket; Monday weeks would cut both ends.
    let weekly = report::execute_query(&context,&json!({"datasetId":"otel_logs","from":iso("2026-01-04T00:00:00"),"thru":iso("2026-01-18T00:00:00"),"queryString":"body:sundayweekfixture","aggs":{"time":{"date_histogram":{"field":"timestamp","calendar_interval":"week"},"aggs":{"value":{"value_count":{"field":"body"}}}}}})).unwrap();
    let frame: report::frame::Frame = serde_json::from_value(weekly["frames"][0].clone()).unwrap();
    let rows = frame.rows();
    let keys: Vec<i64> = rows
        .iter()
        .map(|row| match &row["time"] {
            Json::String(text) => text.parse().unwrap_or_else(|_| at(text)),
            other => other.as_i64().unwrap(),
        })
        .collect();
    assert_eq!(keys, [ms("2026-01-04T00:00:00"), ms("2026-01-11T00:00:00")], "{weekly}");
    assert_eq!(rows[0]["value"].as_f64(), Some(2.0), "{weekly}");
    assert_eq!(rows[1]["value"].as_f64(), Some(1.0), "{weekly}");
    assert!(
        !weekly["notices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|notice| notice["code"] == "partial_buckets_excluded"),
        "{weekly}"
    );
    // A rounded `thru` is the end of its day, and math applies after rounding.
    let noon = iso("2026-01-10T12:00:00");
    let day = report::execute_query(&context,&json!({"datasetId":"otel_logs","from":format!("{noon}||/d"),"thru":format!("{noon}||/d"),"queryString":"body:sundayweekfixture","aggs":{"value":{"value_count":{"field":"body"}}}})).unwrap();
    assert_eq!(day["range"]["thru"], iso("2026-01-11T00:00:00"), "{day}");
    assert_eq!(day["frames"][0]["fields"][0]["values"][0].as_f64(), Some(1.0), "{day}");
    let shifted = report::execute_query(&context,&json!({"datasetId":"otel_logs","from":format!("{noon}||/d"),"thru":format!("{noon}||/d+1h"),"queryString":"body:sundayweekfixture","aggs":{"value":{"value_count":{"field":"body"}}}})).unwrap();
    assert_eq!(shifted["range"]["thru"], iso("2026-01-11T01:00:00"), "{shifted}");
    assert_eq!(
        shifted["frames"][0]["fields"][0]["values"][0].as_f64(),
        Some(2.0),
        "{shifted}"
    );
}

#[test]
fn postgres_report_authorization_cursor_counters_and_definitions() {
    if !crate::common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let fixture = r#"<entity-facade-xml type="seed">
      <moqui.entity.document.DataDocument dataDocumentId="ReportFixture" indexName="report_fixture" primaryEntityName="moqui.basic.Enumeration"/>
      <moqui.entity.document.DataDocumentField dataDocumentId="ReportFixture" fieldSeqId="01" fieldPath="enumId" fieldType="keyword"/>
      <moqui.entity.document.DataDocumentField dataDocumentId="ReportFixture" fieldSeqId="02" fieldPath="enumTypeId" fieldType="keyword"/>
      <moqui.entity.document.DataDocumentField dataDocumentId="ReportFixture" fieldSeqId="03" fieldPath="sequenceNum" fieldType="long"/>
      <moqui.security.UserAccount userId="REPORT_OTHER" username="report.other" disabled="N"/>
      <moqui.resource.GitRepository repositoryId="report-sync" repositoryKind="db" description="Synced report definitions"/>
      <moqui.report.DefinitionGitSync syncId="REPORT_SYNC" folder="Synced" repositoryId="report-sync" path="dash" userId="EX_JOHN_DOE" enabled="Y"/>
      <moqui.basic.EnumerationType enumTypeId="REPORT_DISPLAY" description="Report labels"/>
      <moqui.basic.Enumeration enumId="visible" enumTypeId="REPORT_DISPLAY" description="Visible description"/>
      <moqui.security.UserGroup userGroupId="REPORT_VIEW" description="Scoped report viewer"/>
      <moqui.security.UserGroupMember userGroupId="REPORT_VIEW" userId="REPORT_OTHER" fromDate="0"/>
      <moqui.security.ArtifactGroup artifactGroupId="REPORT_TEST" description="Scoped dataset fixture"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_TEST" artifactName="org.moqui.report.ReportServices.*" artifactTypeEnumId="AT_SERVICE" nameIsPattern="Y" inheritAuthz="Y"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_TEST" artifactName="org.moqui.report.dataset.report-fixture-dataset" artifactTypeEnumId="AT_SERVICE" inheritAuthz="N"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_TEST" artifactName="org.moqui.report.dataset.scoped-telemetry-fixture" artifactTypeEnumId="AT_SERVICE" inheritAuthz="N"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="REPORT_TEST_VIEW" artifactGroupId="REPORT_TEST" userGroupId="REPORT_VIEW" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
      <moqui.security.ArtifactGroup artifactGroupId="REPORT_LOOKUP" description="Viewer-authorized fixture descriptions"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_LOOKUP" artifactName="moqui.basic.Enumeration" artifactTypeEnumId="AT_ENTITY" inheritAuthz="N"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="REPORT_LOOKUP_ADMIN" artifactGroupId="REPORT_LOOKUP" userGroupId="ADMIN" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_VIEW"/>
      <moqui.security.ArtifactGroup artifactGroupId="REPORT_POLICY" description="Administrator-authored policy fixture services"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_POLICY" artifactName="report.fixture.PolicyServices.get#Policy" artifactTypeEnumId="AT_SERVICE" inheritAuthz="N"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="REPORT_POLICY" artifactName="report.fixture.PolicyServices.get#MissingPolicy" artifactTypeEnumId="AT_SERVICE" inheritAuthz="N"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="REPORT_POLICY_ADMIN" artifactGroupId="REPORT_POLICY" userGroupId="ADMIN" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
    </entity-facade-xml>"#;
    let policy_services = r#"<services>
      <service verb="get" noun="Policy">
        <in-parameters><parameter name="userId" required="true"/></in-parameters>
        <out-parameters><parameter name="filter" type="Map"/></out-parameters>
        <actions><set field="filter" from="{field:{name:'enumTypeId',op:'eq',value:userId}}"/></actions>
      </service>
      <service verb="get" noun="MissingPolicy">
        <in-parameters><parameter name="userId" required="true"/></in-parameters>
        <actions/>
      </service>
    </services>"#;
    let gate = common::start_with_files(
        "report-monitoring",
        Some(fixture),
        &[],
        &[
            (
                "component/report-fixture/component.xml",
                "<component name=\"report-fixture\"/>",
            ),
            (
                "component/report-fixture/service/report/fixture/PolicyServices.xml",
                policy_services,
            ),
        ],
    );
    let factory = gate.server.factory();
    let facade = search::installed(factory).unwrap();
    let entity = factory.entity().unwrap();
    let context = factory.new_context();
    let entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    let from = "2026-01-01T00:00:00Z";
    let thru = "2027-01-01T00:00:00Z";
    let same = "2026-02-01T00:00:00Z";
    let static_panels=report::dashboard_frames(&context,&json!({"id":"static-panel-fixture","body":{"panels":{"text":{"visualization":"text","text":"Offline instructions"},"broken":{"visualization":"table"}}}}),&json!({}),from,thru).unwrap();
    assert!(static_panels["panels"]["text"]["notices"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(static_panels["panels"]["broken"]["notices"][0]["code"], "missing_query");
    let points=(0..10_025).map(|i|document(json!({"_id":format!("fixture-{i:05}"),"timestamp":same,"severity_number":13,"body":"cursorfixture","service.name":"report-fixture"}))).collect();
    assert_eq!(facade.index_entries("otel_logs", points).unwrap(), 10_025);
    let mut cursor = Json::Null;
    let mut seen = BTreeSet::new();
    loop {
        let page=report::documents(&context,&json!({"datasetId":"otel_logs","queryString":"body:cursorfixture","from":from,"thru":thru,"pageSize":1000,"sort":["time"],"cursor":cursor})).unwrap();
        let hits = page["hits"].as_array().unwrap();
        if hits.is_empty() {
            break;
        }
        assert_eq!(page["total"], 10_025);
        for hit in hits {
            assert!(
                seen.insert(hit["id"].as_str().unwrap().to_owned()),
                "duplicate cursor hit"
            );
        }
        cursor = page["nextCursor"].clone();
    }
    assert_eq!(seen.len(), 10_025);
    let original = facade.mapping("otel_logs").unwrap();
    let mut changed = (*original).clone();
    changed.root.fields.insert(
        "future_column".into(),
        search::FieldMapping {
            kind: search::FieldKind::Keyword,
            sortable: true,
        },
    );
    assert!(facade
        .backend()
        .ensure_index(&changed)
        .unwrap_err()
        .message()
        .contains("history is preserved"));
    assert!(
        facade
            .backend()
            .get_document(&original, "fixture-00000")
            .unwrap()
            .is_some(),
        "a telemetry schema mismatch must preserve old history"
    );
    // Descending timestamps preserve the ascending unique tie-breaker too.
    let page=report::documents(&context,&json!({"datasetId":"otel_logs","queryString":"body:cursorfixture","from":from,"thru":thru,"pageSize":1000,"sort":["-time"]})).unwrap();
    assert_eq!(page["hits"][0]["id"], "fixture-00000");
    facade.index_entries("otel_logs",vec![document(json!({"_id":"partial-fixture","timestamp":"2026-02-01T00:30:00Z","body":"partialfixture","severity_number":17}))]).unwrap();
    let mut partial = json!({"datasetId":"otel_logs","from":"2026-02-01T00:15:00Z","thru":"2026-02-01T00:45:00Z","queryString":"body:partialfixture","aggs":{"time":{"date_histogram":{"field":"timestamp","fixed_interval":"1h"},"aggs":{"value":{"sum":{"field":"severity_number"}}}}}});
    let excluded = report::execute_query(&context, &partial).unwrap();
    assert!(
        excluded["frames"].as_array().unwrap().is_empty(),
        "only partial observations must not become a zero-valued healthy chart"
    );
    assert!(excluded["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|notice| notice["code"] == "partial_buckets_excluded"
            && notice["observations"] == 1
            && notice["level"] == "warning"
            && notice["complete"] != false));
    partial["aggs"]["time"]["date_histogram"]["drop_partial"] = json!(false);
    let retained = report::execute_query(&context, &partial).unwrap();
    let fields = retained["frames"][0]["fields"].as_array().unwrap();
    assert_eq!(
        fields.iter().find(|field| field["name"] == "time").unwrap()["type"],
        "time"
    );
    assert_eq!(
        fields.iter().find(|field| field["name"] == "value").unwrap()["values"][0].as_f64(),
        Some(17.0),
        "retained histogram totals must match the scalar export"
    );
    assert_eq!(
        fields.iter().find(|field| field["name"] == "partial").unwrap()["values"][0],
        true
    );

    verify_nested_partial_filter_scope(&context, &facade);

    let counter = document(
        json!({"fingerprint":"concurrent","first_seen":same,"last_seen":same,"count":1,"message":"fixture","service.name":"report-fixture"}),
    );
    facade
        .upsert_counter("otel_errors", "concurrent", &counter, "count")
        .unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let facade = facade.clone();
            let counter = counter.clone();
            scope.spawn(move || {
                for _ in 0..25 {
                    facade
                        .upsert_counter("otel_errors", "concurrent", &counter, "count")
                        .unwrap();
                }
            });
        }
    });
    assert_eq!(
        facade.get_document("otel_errors", "concurrent").unwrap().unwrap()["count"],
        Value::I64(101)
    );

    // Native percentiles and histogram bucket merging run on stored metric documents.
    facade.index_entries("otel_metrics",vec![document(json!({"_id":"metric-a","name":"report-fixture-duration","bucket_start":same,"granularity":"minute","count":3,"sum":6,"min":1,"max":3,"bounds":[1,2],"buckets":[1,1,1]})),document(json!({"_id":"metric-b","name":"report-fixture-duration","bucket_start":same,"granularity":"minute","count":3,"sum":12,"min":3,"max":5,"bounds":[1,2],"buckets":[0,1,2]}))]).unwrap();
    let query = parameters(
        json!({"query":{"datasetId":"otel_metrics","from":from,"thru":thru,"queryString":"name:report-fixture-duration","aggs":{"total":{"sum":{"field":"sum"}},"average":{"avg":{"field":"sum"}},"p":{"percentiles":{"field":"sum","percents":[50]}},"latency":{"histogram_merge":{"bounds":[1,2]}}}}}),
    );
    let frames = result(report::run_query(&context, &service(), &query).unwrap());
    let frame: &Json = &frames["frames"][0];
    let field = |name: &str| {
        frame["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap()["values"][0]
            .clone()
    };
    assert_eq!(field("total").as_f64(), Some(18.0));
    assert_eq!(field("average").as_f64(), Some(9.0));
    assert_eq!(field("p.50").as_f64(), Some(9.0));
    assert_eq!(field("latency.bucket.2"), 3);
    let bad = parameters(
        json!({"query":{"datasetId":"otel_metrics","from":from,"thru":thru,"queryString":"name:report-fixture-duration","aggs":{"latency":{"histogram_merge":{"bounds":[1,3]}}}}}),
    );
    assert!(report::run_query(&context, &service(), &bad).is_err());
    facade.index_entries("otel_metrics",vec![document(json!({"_id":"compare-before","name":"comparison-fixture","bucket_start":"2026-01-01T00:00:00Z","sum":10})),document(json!({"_id":"compare-after","name":"comparison-fixture","bucket_start":"2026-01-02T00:00:00Z","sum":15}))]).unwrap();
    let comparison=result(report::run_query(&context,&service(),&parameters(json!({"query":{"datasetId":"otel_metrics","from":"2026-01-02T00:00:00Z","thru":"2026-01-03T00:00:00Z","queryString":"name:comparison-fixture","comparePrevious":true,"inspect":true,"aggs":{"value":{"sum":{"field":"sum"}}},"transformations":[{"type":"calculate","left":"value","value":2,"operator":"*","name":"double"}]}}))).unwrap());
    let compared = comparison["frames"][0]["fields"].as_array().unwrap();
    assert_eq!(
        compared
            .iter()
            .find(|field| field["name"] == "double.previous")
            .unwrap()["values"][0]
            .as_f64(),
        Some(20.0)
    );
    assert!(!comparison["inspection"].as_array().unwrap().is_empty());
    assert_eq!(comparison["previousRange"]["from"], "2026-01-01T00:00:00Z");
    facade.index_entries("otel_metrics",vec![document(json!({"_id":"subtotal-a1","name":"subtotal-fixture","bucket_start":same,"service.name":"A","sum":1})),document(json!({"_id":"subtotal-a2","name":"subtotal-fixture","bucket_start":same,"service.name":"A","sum":3})),document(json!({"_id":"subtotal-b","name":"subtotal-fixture","bucket_start":same,"service.name":"B","sum":100}))]).unwrap();
    let subtotal=result(report::run_query(&context,&service(),&parameters(json!({"query":{"datasetId":"otel_metrics","from":from,"thru":thru,"queryString":"name:subtotal-fixture","subtotals":true,"aggs":{"service":{"terms":{"field":"service.name"},"aggs":{"average":{"avg":{"field":"sum"}}}}}}}))).unwrap());
    let subtotal_fields = subtotal["frames"][0]["fields"].as_array().unwrap();
    let average = subtotal_fields.iter().find(|field| field["name"] == "average").unwrap()["values"][0]
        .as_f64()
        .unwrap();
    assert!(
        (average - 104.0 / 3.0).abs() < 1e-9,
        "grand average must come from all observations, not group averages"
    );

    let tx = entity.transactions_for("moqui.report.Definition").unwrap();
    let dataset = parameters(
        json!({"definition":{"id":"report-fixture-dataset","type":"dataset","title":"Fixture","userGroupId":"REPORT_VIEW","body":{"documentType":"ReportFixture","timeField":"_timestamp","contextFilters":[{"field":"enumTypeId","contextKey":"userId"}],"fieldDefaults":{"enumId":{"lookup":{"entityName":"moqui.basic.Enumeration","keyField":"enumId","displayField":"description"}},"sequenceNum":{"unit":"count","decimals":0}}}}}),
    );
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        report::save_definition(&context, &service(), &dataset)
    })
    .unwrap();
    // ADMIN artifact rights alone do not grant access to a DataDocument.
    assert!(report::dataset(&context, "report-fixture-dataset").is_err());
    {
        let _authz = context.disable_authz();
        row(
            &entity,
            "moqui.entity.document.DataDocumentUserGroup",
            json!({"dataDocumentId":"ReportFixture","userGroupId":"ADMIN"}),
        );
        row(
            &entity,
            "moqui.entity.document.DataDocumentUserGroup",
            json!({"dataDocumentId":"ReportFixture","userGroupId":"REPORT_VIEW"}),
        );
    }
    context.user_context().insert("userId".into(), Value::string("HIDDEN"));
    {
        let _fixture = context.disable_authz();
        facade.index_documents(&[document(json!({"_type":"ReportFixture","_id":"visible","enumId":"visible","enumTypeId":"EX_JOHN_DOE","sequenceNum":7})),document(json!({"_type":"ReportFixture","_id":"other-visible","enumId":"other-visible","enumTypeId":"REPORT_OTHER","sequenceNum":11})),document(json!({"_type":"ReportFixture","_id":"hidden","enumId":"hidden","enumTypeId":"HIDDEN","sequenceNum":100}))]).unwrap();
    }
    let visible=report::documents(&context,&json!({"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h","userId":"HIDDEN","query":{"field":{"name":"enumTypeId","op":"in","values":["EX_JOHN_DOE","HIDDEN"]}}})).unwrap();
    assert_eq!(visible["total"], 1);
    assert_eq!(visible["hits"][0]["id"], "visible");
    let formatted=result(report::run_query(&context,&service(),&parameters(json!({"query":{"datasetId":"report-fixture-dataset","mode":"documents","from":"now-1h","thru":"now+1h"}}))).unwrap());
    let formatted_fields = formatted["frames"][0]["fields"].as_array().unwrap();
    let key = formatted_fields.iter().find(|field| field["name"] == "enumId").unwrap();
    assert_eq!(
        key["values"][0], "visible",
        "formatting must preserve the raw record key"
    );
    let lookup_record = entity
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("visible"))
        .one()
        .unwrap();
    assert_eq!(
        lookup_record
            .as_ref()
            .and_then(|row| row.get("description"))
            .and_then(Value::as_str),
        Some("Visible description"),
        "display lookup fixture must exist"
    );
    assert_eq!(
        key["config"]["mappings"][0]["text"], "Visible description",
        "resolved field: {key}"
    );
    assert_eq!(
        formatted_fields
            .iter()
            .find(|field| field["name"] == "sequenceNum")
            .unwrap()["unit"],
        "count"
    );
    assert_eq!(report::documents(&context,&json!({"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h","query":{"field":{"name":"enumTypeId","op":"eq","value":"HIDDEN"}}})).unwrap()["total"],0,"client filters cannot replace identity constraints");
    // Administrator-authored application policies receive the authenticated identity even
    // when both request parameters and the mutable user context claim a different user.
    let service_policy = parameters(
        json!({"definition":{"id":"service-policy-dataset","type":"dataset","title":"Policy service","body":{"documentType":"ReportFixture","rowFilterService":"report.fixture.PolicyServices.get#Policy"}}}),
    );
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        report::save_definition(&context, &service(), &service_policy)
    })
    .unwrap();
    let service_rows = report::documents(
        &context,
        &json!({"datasetId":"service-policy-dataset","from":"now-1h","thru":"now+1h","userId":"HIDDEN"}),
    )
    .unwrap();
    assert_eq!(service_rows["total"], 1);
    assert_eq!(service_rows["hits"][0]["id"], "visible");
    let missing_policy = parameters(
        json!({"definition":{"id":"missing-policy-dataset","type":"dataset","title":"Missing policy","body":{"documentType":"ReportFixture","rowFilterService":"report.fixture.PolicyServices.get#MissingPolicy"}}}),
    );
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        report::save_definition(&context, &service(), &missing_policy)
    })
    .unwrap();
    assert!(
        report::documents(
            &context,
            &json!({"datasetId":"missing-policy-dataset","from":"now-1h","thru":"now+1h"})
        )
        .is_err(),
        "missing server policy must fail closed"
    );
    tx.run(TransactionAttribute::UseOrBegin,None,||report::save_definition(&context,&service(),&parameters(json!({"definition":{"id":"scoped-telemetry-fixture","type":"dataset","title":"Scoped logs","userGroupId":"REPORT_VIEW","body":{"documentType":"otel_logs","rowFilter":{"field":{"name":"source.id","op":"eq","value":"trusted-fixture"}},"contextFilters":[{"field":"user.id","contextKey":"userId"}]}}})))).unwrap();
    facade.index_entries("otel_logs",vec![document(json!({"_id":"scope-mine","timestamp":same,"source.id":"trusted-fixture","user.id":"REPORT_OTHER","body":"scope fixture"})),document(json!({"_id":"scope-other-user","timestamp":same,"source.id":"trusted-fixture","user.id":"EX_JOHN_DOE","body":"scope fixture"})),document(json!({"_id":"scope-other-source","timestamp":same,"source.id":"other-source","user.id":"REPORT_OTHER","body":"scope fixture"}))]).unwrap();
    let sums=result(report::run_query(&context,&service(),&parameters(json!({"query":{"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h","aggs":{"amount":{"sum":{"field":"sequenceNum"}}}}}))).unwrap());
    assert_eq!(sums["frames"][0]["fields"][0]["values"][0], 7);
    verify_saved_runtime_pills_and_csv(&context, &facade, &entity);
    verify_git_sync(&context, &entity);

    // Restore creates a new version; imports remap named references and never steal ownership.
    let saved = parameters(
        json!({"definition":{"id":"fixture-panel","type":"panel","title":"Private panel","body":{"queries":[{"datasetId":"report-fixture-dataset"}]}}}),
    );
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        report::save_definition(&context, &service(), &saved)
    })
    .unwrap();
    let bundle = result(
        report::export_definitions(
            &context,
            &service(),
            &parameters(json!({"definitionId":"fixture-panel"})),
        )
        .unwrap(),
    );
    let imported = tx
        .run(TransactionAttribute::UseOrBegin, None, || {
            report::import_definitions(
                &context,
                &service(),
                &parameters(json!({"bundle":bundle["bundle"],"idPrefix":"roundtrip-"})),
            )
        })
        .unwrap();
    assert_eq!(result(imported)["definitions"].as_array().unwrap().len(), 2);
    let uses = result(
        report::find_definition_uses(
            &context,
            &service(),
            &parameters(json!({"definitionId":"report-fixture-dataset"})),
        )
        .unwrap(),
    );
    assert!(uses["usedIn"]
        .as_array()
        .unwrap()
        .iter()
        .any(|definition| definition["id"] == "fixture-panel"));
    let diff = result(
        report::get_definition_diff(
            &context,
            &service(),
            &parameters(json!({"definitionId":"fixture-panel","fromVersion":0,"toVersion":1})),
        )
        .unwrap(),
    );
    assert!(diff["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["path"] == "/title"));
    let copied = tx
        .run(TransactionAttribute::UseOrBegin, None, || {
            report::copy_definition(
                &context,
                &service(),
                &parameters(json!({"definitionId":"monitor-overview","newDefinitionId":"fixture-monitor-copy"})),
            )
        })
        .unwrap();
    assert_eq!(result(copied)["definition"]["readOnly"], false);
    for definition in [
        json!({"id":"cycle-a","type":"panel","body":{}}),
        json!({"id":"cycle-b","type":"panel","body":{"panelRef":"cycle-a"}}),
    ] {
        tx.run(TransactionAttribute::UseOrBegin, None, || {
            report::save_definition(&context, &service(), &parameters(json!({"definition":definition})))
        })
        .unwrap();
    }
    assert!(tx
        .run(TransactionAttribute::UseOrBegin, None, || report::save_definition(
            &context,
            &service(),
            &parameters(json!({"definition":{"id":"cycle-a","type":"panel","body":{"panelRef":"cycle-b"}}}))
        ))
        .is_err());
    assert!(tx.run(TransactionAttribute::UseOrBegin,None,||report::import_definitions(&context,&service(),&parameters(json!({"idPrefix":"atomic-","bundle":{"definitions":[{"id":"valid","type":"panel","body":{}},{"id":"invalid","type":"panel","body":{"panelRef":"does-not-exist"}}]}})))).is_err());
    assert!(
        report::get_definition(
            &context,
            &service(),
            &parameters(json!({"definitionId":"atomic-valid"}))
        )
        .is_err(),
        "failed import must roll back earlier objects"
    );
    assert!(tx
        .run(TransactionAttribute::UseOrBegin, None, || report::save_definition(
            &context,
            &service(),
            &parameters(json!({"definition":{"id":"monitor-overview","type":"dashboard","body":{}}}))
        ))
        .is_err());
    let marker = result(
        report::create_annotation(
            &context,
            &service(),
            &parameters(json!({"source":"deploy","time":same,"serviceName":"report-fixture","text":"deploy fixture"})),
        )
        .unwrap(),
    );
    assert!(facade
        .get_document("monitor_annotations", marker["annotationId"].as_str().unwrap())
        .unwrap()
        .is_some());
    assert!(report::create_annotation(
        &context,
        &service(),
        &parameters(json!({"source":"deploy","time":same,"timeEnd":"2025-01-01T00:00:00Z","text":"bad range"}))
    )
    .is_err());
    report::record_release(factory).unwrap();
    report::record_release(factory).unwrap();
    let release = facade
        .search(
            &search::Target::Index("monitor_annotations".into()),
            &search::SearchRequest::new(search::Query::eq("source", Value::string("release"))),
        )
        .unwrap();
    assert_eq!(release.total, 1, "one marker per immutable build");
    let mut alert = json!({"definition":{"id":"fixture-alert","type":"alert","title":"Fixture alert","body":{"query":{"datasetId":"otel_metrics","from":from,"thru":thru,"queryString":"name:report-fixture-duration","aggs":{"total":{"sum":{"field":"sum"}}}},"field":"total","condition":{"type":"above","threshold":17},"contactPoints":[]}}});
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        report::save_definition(&context, &service(), &parameters(alert.clone()))
    })
    .unwrap();
    let clock = jiff::Timestamp::now().as_millisecond();
    context.set_effective_time(Some(clock));
    report::evaluate_rules(
        &context,
        &service(),
        &parameters(json!({"definitionId":"fixture-alert"})),
    )
    .unwrap();
    let state = || {
        let _storage = context.disable_authz();
        let rows = entity
            .find("moqui.report.AlertInstance")
            .unwrap()
            .eq("definitionId", Value::string("fixture-alert"))
            .list()
            .unwrap();
        assert_eq!(rows.len(), 1);
        let state = rows
            .iter()
            .next()
            .unwrap()
            .get("state")
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        state
    };
    assert_eq!(state(), "Firing");
    alert["definition"]["body"]["query"]["queryString"] = json!("name:absent-fixture-metric");
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        report::save_definition(&context, &service(), &parameters(alert.clone()))
    })
    .unwrap();
    context.set_effective_time(Some(clock + 61_000));
    report::evaluate_rules(
        &context,
        &service(),
        &parameters(json!({"definitionId":"fixture-alert"})),
    )
    .unwrap();
    assert_eq!(state(), "NoData", "empty telemetry must never establish recovery");
    context.set_effective_time(None);
    let legacy_body = json!({"paused":true,"query":{"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h","aggs":{"total":{"sum":{"field":"sequenceNum"}}}},"field":"total","condition":{"type":"above","threshold":5},"contactPoints":[{"type":"webhook","url":"https://example.invalid/alerts","headers":{"Authorization":"Bearer legacy-fixture-secret"}}]});
    assert!(
        tx.run(TransactionAttribute::UseOrBegin, None, || report::save_definition(
            &context,
            &service(),
            &parameters(json!({"definition":{"id":"literal-header-alert","type":"alert","body":legacy_body}}))
        ))
        .is_err(),
        "literal credentials must not enter saved definitions"
    );
    {
        let _fixture = context.disable_authz();
        row(
            &entity,
            "moqui.report.Definition",
            json!({"definitionId":"legacy-shared-alert","definitionType":"alert","title":"Legacy shared alert","bodyJson":legacy_body.to_string(),"ownerId":"EX_JOHN_DOE","userGroupId":"REPORT_VIEW","versionNumber":1,"readOnly":"N"}),
        );
        row(
            &entity,
            "moqui.report.DefinitionVersion",
            json!({"definitionId":"legacy-shared-alert","versionNumber":1,"bodyJson":json!({"type":"alert","title":"Legacy shared alert","body":legacy_body}).to_string()}),
        );
    }
    drop(entered);
    let other = factory.new_context();
    let _entered = other.enter();
    moqui_core::user::internal_login_user("report.other").unwrap();
    other
        .push_checked(
            "AT_SERVICE",
            "org.moqui.report.ReportServices.query#Documents",
            "view",
            true,
            false,
            None,
        )
        .unwrap();
    other.user_context().insert("userId".into(), Value::string("HIDDEN"));
    let scoped = report::documents(
        &other,
        &json!({"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h"}),
    )
    .unwrap();
    assert_eq!(scoped["total"], 1);
    assert_eq!(
        scoped["hits"][0]["id"], "other-visible",
        "request userId must not change the authenticated scope"
    );
    let logs=report::documents(&other,&json!({"datasetId":"scoped-telemetry-fixture","from":from,"thru":thru,"userId":"EX_JOHN_DOE","rowFilter":{"match_all":true}})).unwrap();
    assert_eq!(logs["total"], 1);
    assert_eq!(logs["hits"][0]["id"], "scope-mine");
    assert!(report::documents(&other, &json!({"datasetId":"index:otel_logs","from":from,"thru":thru})).is_err());
    assert!(
        report::documents(
            &other,
            &json!({"datasetId":"index:report_fixture","from":"now-1h","thru":"now+1h"})
        )
        .is_err(),
        "inheritable report service must not grant an independent dataset"
    );
    let hybrid = report::documents(
        &other,
        &json!({"datasetId":"report-fixture-dataset","queryString":"visible","retrievalMode":"hybrid","from":"now-1h","thru":"now+1h"}),
    );
    if let Ok(page) = hybrid {
        assert!(page["hits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|hit| hit["id"] == "other-visible"));
    }
    assert!(report::documents(&other, &json!({"datasetId":"otel_logs","from":from,"thru":thru})).is_err());
    assert!(report::get_definition(&other, &service(), &parameters(json!({"definitionId":"fixture-panel"}))).is_err());
    assert!(
        report::evaluate_rules(&other, &service(), &parameters(json!({"definitionId":"fixture-alert"}))).is_err(),
        "a shared service grant must not run another owner's private rule"
    );
    assert!(
        report::evaluate_rules(&other, &service(), &parameters(json!({}))).is_err(),
        "evaluating every owner's rules needs its own artifact grant"
    );
    assert!(
        report::get_definition_diff(&other, &service(), &parameters(json!({"definitionId":"fixture-panel"}))).is_err()
    );
    let visible_uses = result(
        report::find_definition_uses(
            &other,
            &service(),
            &parameters(json!({"definitionId":"report-fixture-dataset"})),
        )
        .unwrap(),
    );
    let uses = visible_uses["usedIn"].as_array().unwrap();
    assert!(
        !uses
            .iter()
            .any(|definition| definition["id"] == "fixture-panel" || definition["id"] == "roundtrip-fixture-panel"),
        "used-in must not reveal private referring definitions: {uses:?}"
    );
    assert!(
        uses.iter().any(|definition| definition["id"] == "legacy-shared-alert"),
        "visible shared referring definitions should remain listed"
    );
    assert!(report::create_annotation(&other, &service(), &parameters(json!({"text":"unauthorized marker"}))).is_err());
    let legacy = result(
        report::get_definition(
            &other,
            &service(),
            &parameters(json!({"definitionId":"legacy-shared-alert"})),
        )
        .unwrap(),
    );
    assert!(!legacy.to_string().contains("legacy-fixture-secret"));
    let exported = result(
        report::export_definitions(
            &other,
            &service(),
            &parameters(json!({"definitionId":"legacy-shared-alert"})),
        )
        .unwrap(),
    );
    assert!(!exported.to_string().contains("legacy-fixture-secret"));
    let diff = result(
        report::get_definition_diff(
            &other,
            &service(),
            &parameters(json!({"definitionId":"legacy-shared-alert","fromVersion":0,"toVersion":1})),
        )
        .unwrap(),
    );
    assert!(!diff.to_string().contains("legacy-fixture-secret"));
    let copied = tx
        .run(TransactionAttribute::UseOrBegin, None, || {
            report::copy_definition(
                &other,
                &service(),
                &parameters(json!({"definitionId":"legacy-shared-alert","newDefinitionId":"safe-alert-copy"})),
            )
        })
        .unwrap();
    assert!(!result(copied).to_string().contains("legacy-fixture-secret"));
    other.pop().unwrap();
}

/// Partial observations in another Filter bucket do not make the selected parent
/// incomplete. Once A has partial observations, its exact population is reported.
fn verify_nested_partial_filter_scope(context: &moqui_core::context::ExecutionContext, facade: &search::SearchFacade) {
    let point = |id: &str, service: &str, time: &str| {
        document(
            json!({"_id":id,"timestamp":format!("2026-02-02T{time}:00Z"),"body":"nested-partial-scope-fixture","service.name":service,"severity_number":17}),
        )
    };
    facade
        .index_entries(
            "otel_logs",
            vec![
                point("nested-a-complete-1", "A", "00:06"),
                point("nested-a-complete-2", "A", "00:07"),
                point("nested-b-partial-1", "B", "00:03"),
                point("nested-b-partial-2", "B", "00:12"),
            ],
        )
        .unwrap();
    let query = json!({"datasetId":"otel_logs","from":"2026-02-02T00:02:00Z","thru":"2026-02-02T00:13:00Z","queryString":"body:nested-partial-scope-fixture","aggs":{"selected":{"filter":{"term":{"service.name":"A"}},"aggs":{"time":{"date_histogram":{"field":"timestamp","fixed_interval":"5m"},"aggs":{"observations":{"value_count":{"field":"body"}}}}}}}});
    let complete = report::execute_query(context, &query).unwrap();
    assert!(
        !complete["frames"].as_array().unwrap().is_empty(),
        "selected A has completed observations: {complete}"
    );
    assert!(
        !complete["notices"]
            .as_array()
            .unwrap()
            .iter()
            .any(|notice| notice["complete"] == false || notice["code"] == "partial_buckets_excluded"),
        "unrelated B partial observations must not contaminate selected A completeness: {complete}"
    );
    let fields = complete["frames"][0]["fields"].as_array().unwrap();
    let observations = fields.iter().find(|field| field["name"] == "observations").unwrap()["values"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Json::as_f64)
        .sum::<f64>();
    assert_eq!(observations, 2.0, "both complete A observations remain visible");
    facade
        .index_entries(
            "otel_logs",
            vec![
                point("nested-a-partial-1", "A", "00:03"),
                point("nested-a-partial-2", "A", "00:12"),
            ],
        )
        .unwrap();
    let partial = report::execute_query(context, &query).unwrap();
    let excluded = partial["notices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|notice| notice["code"] == "partial_buckets_excluded")
        .collect::<Vec<_>>();
    assert_eq!(excluded.len(), 1, "one nested partial histogram notice: {partial}");
    assert_eq!(
        excluded[0]["observations"], 2,
        "only A's two partial observations count, not B's two: {partial}"
    );
    assert_eq!(excluded[0]["level"], "warning");
    assert!(
        excluded[0]["complete"] != false,
        "a dropped partial bucket is not missing data: {partial}"
    );
    assert!(
        !partial["frames"].as_array().unwrap().is_empty(),
        "completed selected-parent buckets are retained while the excluded observations are noted"
    );
    let kept = partial["frames"][0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "observations")
        .unwrap()["values"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Json::as_f64)
        .sum::<f64>();
    assert_eq!(
        kept, 2.0,
        "partial A rows are excluded from the completed bucket's values"
    );
}

fn numeric_field(frame: &Json, name: &str) -> f64 {
    frame["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == name)
        .unwrap_or_else(|| panic!("missing {name} in {frame}"))["values"][0]
        .as_f64()
        .unwrap()
}
fn preview_csv(definition_id: &str, filters: &Json) -> Json {
    result(moqui_core::service::sync("org.moqui.report.ReportServices.preview#Report",&parameters(json!({"definitionId":definition_id,"panelId":"business","renderMode":"csv","from":"now-1h","thru":"now+1h","filters":filters}))).unwrap())
}
fn verify_saved_runtime_pills_and_csv(
    context: &moqui_core::context::ExecutionContext,
    facade: &search::SearchFacade,
    entity: &EntityFacade,
) {
    use base64::Engine as _;
    {
        let _fixture = context.disable_authz();
        facade.index_documents(&[
            document(json!({"_type":"ReportFixture","_id":"pill-A","enumId":"pill-A","enumTypeId":"EX_JOHN_DOE","sequenceNum":7})),
            document(json!({"_type":"ReportFixture","_id":"pill-B","enumId":"pill-B","enumTypeId":"EX_JOHN_DOE","sequenceNum":11})),
            document(json!({"_type":"ReportFixture","_id":"pill-Missing","enumId":"pill-Missing","enumTypeId":"EX_JOHN_DOE"})),
            document(json!({"_type":"ReportFixture","_id":"pill-hidden","enumId":"pill-hidden","enumTypeId":"HIDDEN","sequenceNum":1000})),
        ]).unwrap();
    }
    let saved_filter = json!({"datasetId":"report-fixture-dataset","query":{"field":{"name":"enumId","op":"prefix","value":"pill-"}},"label":"Saved constraint","pinned":true});
    let definition = json!({"id":"scoped-pills-dashboard","type":"dashboard","title":"Saved/runtime filter acceptance","userGroupId":"REPORT_VIEW","body":{"filters":[saved_filter],"panels":{
        "business":{"title":"Authorized business totals","visualization":"table","queries":[{"datasetId":"report-fixture-dataset","aggs":{"amount":{"sum":{"field":"sequenceNum"}},"records":{"value_count":{"field":"enumId"}}}}]},
        "logs":{"title":"Raw telemetry","visualization":"stat","time":{"from":"2026-01-01T00:00:00Z","thru":"2027-01-01T00:00:00Z"},"queries":[{"datasetId":"otel_logs","queryString":"body:\"scope fixture\"","query":{"field":{"name":"source.id","op":"in","values":["trusted-fixture","other-source"]}},"aggs":{"rows":{"value_count":{"field":"body"}}}}]},
        "alias":{"title":"Scoped telemetry alias","visualization":"stat","time":{"from":"2026-01-01T00:00:00Z","thru":"2027-01-01T00:00:00Z"},"queries":[{"datasetId":"scoped-telemetry-fixture","queryString":"body:\"scope fixture\"","aggs":{"rows":{"value_count":{"field":"body"}}}}]}
    },"layout":{"type":"autoGrid"}}});
    let saved = entity
        .transactions_for("moqui.report.Definition")
        .unwrap()
        .run(TransactionAttribute::UseOrBegin, None, || {
            report::save_definition(context, &service(), &parameters(json!({"definition":definition})))
        })
        .unwrap();
    let stored = result(saved)["definition"].clone();
    let scenarios = [
        ("saved only", json!([]), 18.0, 3.0, 3.0),
        (
            "Other",
            json!([{"datasetId":"report-fixture-dataset","query":{"field":{"name":"enumId","op":"in","values":["pill-A"]}},"negated":true,"label":"Other"}]),
            11.0,
            2.0,
            3.0,
        ),
        (
            "Missing",
            json!([{"datasetId":"report-fixture-dataset","query":{"field":{"name":"sequenceNum","op":"exists","negate":true}},"label":"Missing"}]),
            0.0,
            1.0,
            3.0,
        ),
        (
            "typed numeric",
            json!([{"datasetId":"report-fixture-dataset","query":{"field":{"name":"sequenceNum","op":"eq","value":11}},"pinned":true,"label":"Numeric eleven"}]),
            11.0,
            1.0,
            3.0,
        ),
        (
            "runtime cannot widen identity",
            json!([{"datasetId":"report-fixture-dataset","query":{"field":{"name":"enumTypeId","op":"in","values":["EX_JOHN_DOE","HIDDEN"]}},"label":"Attempted wider identity"}]),
            18.0,
            3.0,
            3.0,
        ),
        (
            "dataset scope excludes alias",
            json!([{"datasetId":"otel_logs","query":{"field":{"name":"user.id","op":"eq","value":"REPORT_OTHER"}},"label":"Raw logs only"}]),
            18.0,
            3.0,
            2.0,
        ),
        (
            "disabled filter",
            json!([{"datasetId":"report-fixture-dataset","query":{"field":{"name":"enumTypeId","op":"eq","value":"HIDDEN"}},"disabled":true}]),
            18.0,
            3.0,
            3.0,
        ),
    ];
    for (label, filters, amount, count, logs) in scenarios {
        let rendered =
            report::dashboard_frames_filtered(context, &stored, &json!({}), "now-1h", "now+1h", &filters).unwrap();
        assert_eq!(
            rendered["filters"], filters,
            "runtime pills remain typed and scoped in URL/inspector state: {label}"
        );
        for (panel, expected) in [("business", count), ("logs", logs), ("alias", 1.0)] {
            assert!(
                !rendered["panels"][panel]["notices"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|notice| notice["complete"] == false || notice["level"] == "error"),
                "{label}/{panel}: {rendered}"
            );
            let metric = if panel == "business" { "records" } else { "rows" };
            assert_eq!(
                numeric_field(&rendered["panels"][panel]["frames"][0], metric),
                expected,
                "dataset-scoped predicates and alias row policy: {label}/{panel}"
            );
        }
        let business = &rendered["panels"]["business"]["frames"][0];
        assert_eq!(
            numeric_field(business, "amount"),
            amount,
            "saved constraint AND runtime predicate AND authenticated row policy: {label}"
        );
        let served = result(
            moqui_core::service::sync(
                "org.moqui.report.ReportServices.render#Dashboard",
                &parameters(
                    json!({"definitionId":"scoped-pills-dashboard","from":"now-1h","thru":"now+1h","filters":filters}),
                ),
            )
            .unwrap(),
        );
        assert_eq!(
            served["dashboard"]["panels"]["business"]["frames"], rendered["panels"]["business"]["frames"],
            "service metadata must preserve filters unchanged: {label}"
        );
        let output = preview_csv("scoped-pills-dashboard", &filters);
        assert_eq!(output["contentType"], "text/csv");
        assert_eq!(output["empty"], false);
        let csv = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(output["contentBase64"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let lines = csv.lines().filter(|line| !line.is_empty()).collect::<Vec<_>>();
        assert_eq!(
            lines.len(),
            2,
            "CSV contains exactly the selected authorized aggregate frame: {label}: {csv}"
        );
        let columns = lines[0]
            .split(',')
            .map(|cell| cell.trim_matches('"'))
            .collect::<Vec<_>>();
        let values = lines[1]
            .split(',')
            .map(|cell| cell.trim_matches('"').parse::<f64>().unwrap())
            .collect::<Vec<_>>();
        for (name, expected) in [("amount", amount), ("records", count)] {
            let at = columns.iter().position(|column| *column == name).unwrap();
            assert_eq!(values[at], expected, "CSV matches panel data: {label}/{name}");
        }
    }
    let hidden = json!([{"datasetId":"report-fixture-dataset","query":{"field":{"name":"enumTypeId","op":"eq","value":"HIDDEN"}}}]);
    let denied = report::dashboard_frames_filtered(context, &stored, &json!({}), "now-1h", "now+1h", &hidden).unwrap();
    // A business scalar count over an empty authorized population remains a zero,
    // preserving the SearchFacade meaning; it must never expose HIDDEN's 1,000.
    let denied_frame = &denied["panels"]["business"]["frames"][0];
    assert_eq!(numeric_field(denied_frame, "amount"), 0.0);
    assert_eq!(
        numeric_field(denied_frame, "records"),
        0.0,
        "a filter cannot replace the server identity policy"
    );
    let output = preview_csv("scoped-pills-dashboard", &hidden);
    let csv = base64::engine::general_purpose::STANDARD
        .decode(output["contentBase64"].as_str().unwrap())
        .unwrap();
    let csv = String::from_utf8(csv).unwrap();
    let data = csv
        .lines()
        .filter(|line| !line.is_empty())
        .nth(1)
        .expect("authorized zero aggregate row");
    assert!(
        data.split(',')
            .all(|value| value.trim_matches('"').parse::<f64>() == Ok(0.0)),
        "unauthorized hidden rows cannot enter exports: {csv}"
    );

    // Provenance follows a sorted, filtered and renamed Other row, rather than
    // remaining attached to its old ordinal position in the aggregation result.
    let transformed=report::execute_query(context,&json!({"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h","query":{"field":{"name":"enumId","op":"prefix","value":"pill-"}},"aggs":{"category":{"terms":{"field":"enumId","size":1,"other_bucket":"Other"},"aggs":{"records":{"value_count":{"field":"enumId"}}}}},"transformations":[{"type":"sort","field":"records","descending":true},{"type":"filter","field":"records","operator":"gt","value":1},{"type":"organize","rename":{"category":"status"}}]})).unwrap();
    let status = transformed["frames"][0]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == "status")
        .unwrap();
    assert_eq!(status["values"], json!(["Other"]));
    assert_eq!(status["config"]["bucket"]["datasetId"], "report-fixture-dataset");
    let selected = &status["config"]["bucket"]["filters"][0];
    assert!(
        !selected.is_null(),
        "transforms retain the exact selected bucket predicate: {transformed}"
    );
    let replay = report::documents(
        context,
        &json!({"datasetId":"report-fixture-dataset","from":"now-1h","thru":"now+1h","query":{"field":{"name":"enumId","op":"prefix","value":"pill-"}},"filters":[{"query":selected}]}),
    )
    .unwrap();
    assert_eq!(replay["total"], 2);
    let ids = replay["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids, BTreeSet::from(["pill-B", "pill-Missing"]));
    verify_tolerant_panels_and_single_panel_view(context, &stored);
    verify_repeated_panels(context, &stored);
}

/// A save in a synced folder writes its file once the save commits; a file changed in the repository is pulled by its
/// id, a second pull and the pulled save change nothing, a bad file is named while the rest pull, and a definition whose
/// file is gone is listed, not deleted.
fn verify_git_sync(context: &moqui_core::context::ExecutionContext, entity: &EntityFacade) {
    let sync = json!({"syncId":"REPORT_SYNC","folder":"Synced","repositoryId":"report-sync","path":"dash","userId":"EX_JOHN_DOE"});
    let resource = context.factory().resource();
    let file = "git://report-sync/dash/synced-dash.json";
    let save = |definition: Json| {
        entity
            .transactions_for("moqui.report.Definition")
            .unwrap()
            .run(TransactionAttribute::UseOrBegin, None, || {
                report::save_definition(context, &service(), &parameters(json!({"definition":definition})))
            })
            .unwrap()
    };
    save(json!({"id":"synced-dash","type":"dashboard","title":"Synced","body":{"folder":"Synced","panels":{}}}));
    let written: Json = serde_json::from_slice(
        &resource
            .location_bytes(file)
            .unwrap()
            .expect("the save pushed its file"),
    )
    .unwrap();
    assert_eq!(
        written,
        json!({"id":"synced-dash","type":"dashboard","title":"Synced","body":{"folder":"Synced","panels":{}}})
    );
    let versions = |location: &str| resource.version_history(location).unwrap().len();
    assert_eq!(versions(file), 1);

    resource
        .write_bytes(
            file,
            br#"{"id":"synced-dash","type":"dashboard","title":"Synced from git","body":{"panels":{},"folder":"Synced"}}"#,
        )
        .unwrap();
    resource
        .write_bytes("git://report-sync/dash/broken.json", b"not json")
        .unwrap();
    let factory = context.factory().clone();
    let pulled = report::git_sync::pull(&factory, &sync).unwrap();
    assert_eq!(pulled["saved"], json!(["synced-dash"]), "{pulled}");
    assert_eq!(
        pulled["failed"][0]["file"], "git://report-sync/dash/broken.json",
        "{pulled}"
    );
    let definition = report::definitions::read(context, "synced-dash").unwrap();
    assert_eq!(definition["title"], "Synced from git");
    assert_eq!(versions(file), 2, "the pulled save wrote nothing back");
    let again = report::git_sync::pull(&factory, &sync).unwrap();
    assert_eq!(
        (again["saved"].clone(), again["unchanged"].clone()),
        (json!([]), json!(1)),
        "{again}"
    );
    assert_eq!(
        report::git_sync::push(&factory, &sync).unwrap(),
        json!({"written":0,"unchanged":1,"removed":[]})
    );

    // Deleting a synced definition removes its file once the delete commits.
    save(json!({"id":"synced-two","type":"dashboard","title":"Second","body":{"folder":"Synced","panels":{}}}));
    let second = "git://report-sync/dash/synced-two.json";
    assert!(resource.location_bytes(second).unwrap().is_some());
    entity
        .transactions_for("moqui.report.Definition")
        .unwrap()
        .run(TransactionAttribute::UseOrBegin, None, || {
            report::delete_definition(context, &service(), &parameters(json!({"definitionId":"synced-two"})))
        })
        .unwrap();
    assert!(
        resource.location_bytes(second).unwrap().is_none(),
        "the delete removed its file"
    );

    resource.delete(file).unwrap();
    // A definition this sync wrote is deleted when its file is removed in git; an untracked one is only listed.
    {
        let _storage = context.disable_authz();
        let mut untracked = entity.make_value("moqui.report.Definition").unwrap();
        for (name, value) in [
            ("definitionId", "untracked-dash"),
            ("definitionType", "dashboard"),
            ("title", "Untracked"),
            ("bodyJson", r#"{"folder":"Synced","panels":{}}"#),
            ("ownerId", "EX_JOHN_DOE"),
            ("readOnly", "N"),
        ] {
            untracked.set(name, moqui_lib::object::Value::string(value));
        }
        untracked.create(entity).unwrap();
    }
    let gone = report::git_sync::pull(&factory, &sync).unwrap();
    assert_eq!(gone["deleted"], json!(["synced-dash"]), "{gone}");
    assert_eq!(gone["missing"], json!(["untracked-dash"]), "{gone}");
    assert!(
        report::definitions::read_optional(context, "synced-dash")
            .unwrap()
            .is_none(),
        "the removed file deleted its tracked definition"
    );
    assert!(report::definitions::read(context, "untracked-dash").is_ok());
    let _status = context.disable_authz();
    let row = entity
        .find("moqui.report.DefinitionGitSync")
        .unwrap()
        .eq("syncId", moqui_lib::object::Value::string("REPORT_SYNC"))
        .one()
        .unwrap()
        .unwrap();
    assert!(row.get("lastPullDate").is_some_and(|date| !date.is_null()));
    assert!(row
        .get("lastError")
        .and_then(moqui_lib::object::Value::as_str)
        .is_some_and(|error| error.contains("broken.json")));
}

/// A panel repeated for each value of a query variable set to All: one copy per option, each narrowed to its value, the
/// first keeping the panel's id; a copy alone renders through the single-panel view.
fn verify_repeated_panels(context: &moqui_core::context::ExecutionContext, stored: &Json) {
    let mut definition = stored.clone();
    let body = &mut definition["body"];
    body["variables"] =
        json!([{"name":"enum","kind":"query","datasetId":"report-fixture-dataset","field":"enumId","includeAll":true}]);
    let mut repeated = body["panels"]["business"].clone();
    repeated["title"] = json!("Totals for $enum");
    repeated["repeat"] = json!({"variable":"enum","maxPerRow":2});
    body["panels"] = json!({"byEnum":repeated});
    body["layout"] = json!({"type":"grid","items":[{"panelId":"byEnum","x":0,"y":0,"w":24,"h":1}]});
    let rendered =
        report::dashboard_frames_filtered(context, &definition, &json!({}), "now-1h", "now+1h", &json!([])).unwrap();
    let panels = rendered["panels"].as_object().unwrap();
    assert!(panels.len() >= 3, "a copy for each option: {rendered}");
    let copy = |value: &str| {
        panels
            .iter()
            .find(|(_, panel)| panel["repeatValues"]["enum"] == value)
            .unwrap_or_else(|| panic!("no copy for {value}: {rendered}"))
    };
    for (value, amount) in [("pill-A", 7.0), ("pill-B", 11.0)] {
        let (id, panel) = copy(value);
        assert_eq!(panel["title"], json!(format!("Totals for {value}")), "{id}");
        assert_eq!(panel["repeatOf"], "byEnum");
        assert_eq!(
            numeric_field(&panel["frames"][0], "amount"),
            amount,
            "{id} is narrowed to {value}: {panel}"
        );
    }
    assert!(panels.contains_key("byEnum"), "the first copy keeps the panel id");
    let items = rendered["layout"]["items"].as_array().unwrap();
    assert_eq!(items.len(), panels.len(), "{rendered}");
    assert!(
        items.iter().all(|item| item["w"] == 12),
        "two copies to a row: {rendered}"
    );
    let (id, _) = copy("pill-B");
    let mut single = definition.clone();
    report::select_panel(&mut single, id).unwrap();
    let alone =
        report::dashboard_frames_filtered(context, &single, &json!({}), "now-1h", "now+1h", &json!([])).unwrap();
    assert_eq!(alone["panels"].as_object().unwrap().len(), 1, "{alone}");
    assert_eq!(
        numeric_field(&alone["panels"][id]["frames"][0], "amount"),
        11.0,
        "{alone}"
    );
    assert_eq!(alone["layout"], json!({"type":"autoGrid","items":[{"panelId":id}]}));
    assert!(report::select_panel(&mut definition.clone(), "nothing~1").is_err());
}

/// One bad panel, library reference or variable is a notice on that piece; the data panel beside it
/// still renders, and the single-panel view lays out only the chosen panel.
fn verify_tolerant_panels_and_single_panel_view(context: &moqui_core::context::ExecutionContext, stored: &Json) {
    let mut definition = stored.clone();
    let body = &mut definition["body"];
    body["variables"] = json!([{"name":"broken","kind":"query","datasetId":"no-such-dataset","field":"enumId"}]);
    body["panels"]["badTime"] =
        json!({"visualization":"table","time":{"shift":"sideways"},"queries":[{"datasetId":"report-fixture-dataset"}]});
    body["panels"]["badLibrary"] = json!({"panelRef":"no-such-library-panel"});
    body["panels"]["badShape"] = json!("not a panel");
    body["panels"]["badQuery"] = json!({"visualization":"table","queries":[1,{"datasetId":"report-fixture-dataset","aggs":{"records":{"value_count":{"field":"enumId"}}}}]});
    let rendered =
        report::dashboard_frames_filtered(context, &definition, &json!({}), "now-1h", "now+1h", &json!([])).unwrap();
    assert_eq!(
        numeric_field(&rendered["panels"]["business"]["frames"][0], "records"),
        3.0,
        "the data panel renders beside the broken ones: {rendered}"
    );
    for panel in ["badTime", "badLibrary", "badShape"] {
        assert_eq!(
            rendered["panels"][panel]["notices"][0]["level"], "error",
            "{panel}: {rendered}"
        );
        assert_eq!(rendered["panels"][panel]["notices"][0]["complete"], false, "{panel}");
    }
    let bad_query = &rendered["panels"]["badQuery"];
    assert_eq!(bad_query["notices"][0]["level"], "error", "{bad_query}");
    assert_eq!(
        numeric_field(&bad_query["frames"][0], "records"),
        3.0,
        "a malformed query leaves its sibling query in the panel: {bad_query}"
    );
    assert!(rendered["notices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|notice| notice["code"] == "variable_error" && notice["variable"] == "broken"));
    // A query names the variables it ignores, so a field one query has does not empty another's panel.
    let mut narrowed = stored.clone();
    let body = &mut narrowed["body"];
    body["variables"] = json!([{"name":"endpoint","kind":"custom","datasetId":"report-fixture-dataset","field":"enumId","options":["no-such-enum"]}]);
    let mut ignoring = body["panels"]["business"].clone();
    ignoring["queries"][0]["ignoreVariables"] = json!(["endpoint"]);
    body["panels"]["ignoring"] = ignoring;
    let rendered = report::dashboard_frames_filtered(
        context,
        &narrowed,
        &json!({"endpoint":"no-such-enum"}),
        "now-1h",
        "now+1h",
        &json!([]),
    )
    .unwrap();
    assert_eq!(
        numeric_field(&rendered["panels"]["ignoring"]["frames"][0], "records"),
        3.0,
        "{rendered}"
    );
    assert!(
        rendered["panels"]["business"]["frames"][0]["fields"]
            .as_array()
            .into_iter()
            .flatten()
            .all(|field| field["name"] != "records"
                || field["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|value| value.as_f64().unwrap_or(0.0) == 0.0)),
        "the variable still narrows the other panel: {rendered}"
    );
    let single = result(
        moqui_core::service::sync(
            "org.moqui.report.ReportServices.render#Dashboard",
            &parameters(
                json!({"definitionId":"scoped-pills-dashboard","panelId":"logs","from":"now-1h","thru":"now+1h"}),
            ),
        )
        .unwrap(),
    );
    let dashboard = &single["dashboard"];
    assert_eq!(dashboard["panels"].as_object().unwrap().len(), 1, "{dashboard}");
    assert_eq!(
        dashboard["layout"],
        json!({"type":"autoGrid","items":[{"panelId":"logs"}]}),
        "the single-panel layout names no other panel"
    );
}
