//! Mixed-index reporting uses authorized document types and immutable row constraints before
//! retrieval, totals, paging and document expansion. Framework rows keep this fixture small.
#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]
mod common;
use example as _;
use moqui_core::{
    context::{ExecutionContext, ExecutionContextFactory},
    native::{Parameters, ServiceInfo},
    report, search,
    transaction::TransactionAttribute,
};
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};
use std::{collections::BTreeSet, sync::Arc};
fn parameters(value: Json) -> Parameters {
    match moqui_lib::json::from_json(&value) {
        Value::Map(map) => map.into_iter().collect(),
        _ => panic!("object"),
    }
}
fn service() -> ServiceInfo<'static> {
    ServiceInfo {
        name: "org.moqui.report.ReportServices.test#MixedDocuments",
        service_type: "rust",
        location: "test",
        method: "",
    }
}
fn viewer<T>(factory: &Arc<ExecutionContextFactory>, name: &str, body: impl FnOnce(&ExecutionContext) -> T) -> T {
    let ec = factory.new_context();
    let _entered = ec.enter();
    moqui_core::user::internal_login_user(name).unwrap();
    body(&ec)
}
fn options() -> Json {
    json!({"datasetId":"scoped-mixed","from":"now-1h","thru":"now+1h","pageSize":100,"sort":["amount"]})
}

struct VectorSchema {
    tx: moqui_core::transaction::TransactionManager,
    schema: String,
}
impl Drop for VectorSchema {
    fn drop(&mut self) {
        assert!(self.schema.starts_with("report_vector_") && self.schema.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        self.tx.with_reserve(|conn| conn.execute(&format!("DROP SCHEMA {} CASCADE",self.schema), &[])).unwrap();
    }
}

#[test]
fn mixed_index_documents_keep_type_row_link_and_dialog_boundaries() {
    verify_documents(None, false);
}

#[test]
#[ignore = "requires MONITOR_READER_OPENSEARCH_URL, for example http://127.0.0.1:19200"]
fn mixed_index_documents_keep_type_row_link_and_dialog_boundaries_opensearch() {
    let url = std::env::var("MONITOR_READER_OPENSEARCH_URL").expect("set MONITOR_READER_OPENSEARCH_URL");
    verify_documents(Some(&url), false);
}

#[test]
#[ignore = "requires an isolated pgvector server, configured with MONITOR_VECTOR_PG_HOST/PORT/USER/PASSWORD/DATABASE"]
fn semantic_documents_rank_only_the_authorized_population_on_pgvector() {
    verify_documents(None, true);
}

fn verify_documents(opensearch_url: Option<&str>, vector: bool) {
    moqui_host::logging::install();
    let mut fixture = String::from(
        r#"<entity-facade-xml type="seed">
      <moqui.security.UserAccount userId="DOC_BOTH" username="doc.both" currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA" disabled="N"/>
      <moqui.security.UserAccount userId="DOC_PARTY" username="doc.party" currentPassword="16ac58bbfa332c1c55bd98b53e60720bfa90d394" passwordHashType="SHA" disabled="N"/>
      <moqui.security.UserGroup userGroupId="DOC_SHARED" description="Scoped document readers"/>
      <moqui.security.UserGroup userGroupId="DOC_BOTH" description="Both document types"/>
      <moqui.security.UserGroupMember userGroupId="DOC_SHARED" userId="DOC_BOTH" fromDate="0"/>
      <moqui.security.UserGroupMember userGroupId="DOC_SHARED" userId="DOC_PARTY" fromDate="0"/>
      <moqui.security.UserGroupMember userGroupId="DOC_BOTH" userId="DOC_BOTH" fromDate="0"/>
      <moqui.security.ArtifactGroup artifactGroupId="DOC_READ" description="Authorized reporting fixture"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="DOC_READ" artifactName="org.moqui.report.dataset.scoped-mixed" artifactTypeEnumId="AT_SERVICE" inheritAuthz="N"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="DOC_READ" artifactName="org.moqui.report.ReportServices.*" artifactTypeEnumId="AT_SERVICE" nameIsPattern="Y" inheritAuthz="Y"/>
      <moqui.security.ArtifactGroupMember artifactGroupId="DOC_READ" artifactName=".*" artifactTypeEnumId="AT_XML_SCREEN" nameIsPattern="Y" inheritAuthz="Y"/>
      <moqui.security.ArtifactAuthz artifactAuthzId="DOC_READ_SHARED" artifactGroupId="DOC_READ" userGroupId="DOC_SHARED" authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_ALL"/>
      <moqui.basic.EnumerationType enumTypeId="DOC_BOTH" description="Both-reader rows"/>
      <moqui.basic.EnumerationType enumTypeId="DOC_PARTY" description="Party-reader rows"/>
      <moqui.basic.EnumerationType enumTypeId="DOC_OTHER" description="Another owner's rows"/>
      <moqui.basic.Enumeration enumId="PARTY_A" enumTypeId="DOC_BOTH" optionValue="3" enumCode="Visible party" description="Common business record" sequenceNum="3"/>
      <moqui.basic.Enumeration enumId="PARTY_NOLINK" enumTypeId="DOC_BOTH" optionValue="7" enumCode="NoLink" description="Common business record" sequenceNum="7"/>
      <moqui.basic.Enumeration enumId="PARTY_BLOCKED" enumTypeId="DOC_BOTH" enumCode="Blocked same owner" description="BLOCKED_SAME_OWNER" sequenceNum="500"/>
      <moqui.basic.Enumeration enumId="PARTY_OTHER" enumTypeId="DOC_OTHER" enumCode="Visible secret" description="SECRET_OTHER_OWNER" sequenceNum="13"/>
      <moqui.basic.Enumeration enumId="PARTY_LIMITED" enumTypeId="DOC_PARTY" enumCode="Limited party" description="Own allowed type" sequenceNum="11"/>
      <moqui.basic.Enumeration enumId="PRODUCT_A" enumTypeId="DOC_BOTH" optionValue="20" enumCode="Visible product" description="Common business record" sequenceNum="5"/>
      <moqui.basic.Enumeration enumId="PRODUCT_FORBIDDEN" enumTypeId="DOC_PARTY" enumCode="Forbidden product" description="SECRET_FORBIDDEN_TYPE" sequenceNum="50"/>
    "#,
    );
    for (id, title, group) in [
        ("ReportParty", "Party", "DOC_SHARED"),
        ("ReportProduct", "Product", "DOC_BOTH"),
    ] {
        fixture.push_str(&format!(r#"
          <moqui.entity.document.DataDocument dataDocumentId="{id}" indexName="report_mixed" primaryEntityName="moqui.basic.Enumeration" documentTitle="{title}: ${{enumCode}}"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="01" fieldPath="enumId" fieldType="keyword"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="02" fieldPath="enumCode" fieldType="text" searchRole="title" sortable="Y"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="03" fieldPath="enumTypeId" fieldType="keyword"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="04" fieldPath="description" fieldType="text"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="05" fieldPath="sequenceNum" fieldNameAlias="amount" fieldType="long"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="06" fieldPath="lastUpdatedStamp" fieldNameAlias="time" fieldType="date"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="07" fieldPath="optionValue" fieldNameAlias="measure" fieldType="double"/>
          <moqui.entity.document.DataDocumentField dataDocumentId="{id}" fieldSeqId="08" fieldPath="(enumCode === 'NoLink' ? null : (enumCode === 'Visible party' ? 'Other' : 'A'))" fieldNameAlias="category" fieldType="keyword"/>
          <moqui.entity.document.DataDocumentUserGroup dataDocumentId="{id}" userGroupId="ADMIN"/>
          <moqui.entity.document.DataDocumentUserGroup dataDocumentId="{id}" userGroupId="{group}"/>
          <moqui.entity.document.DataDocumentLink dataDocumentId="{id}" linkSeqId="01" label="Open {title}" linkUrl="/fixture/{id}/${{enumId}}" urlType="plain" linkCondition="enumCode !== 'NoLink'"/>
        "#));
    }
    fixture.push_str("</entity-facade-xml>");
    let prefix = format!("report_documents_{}_", std::process::id());
    let gate = match opensearch_url {
        Some(url) => common::start_with_conf_properties(
            "report-documents-os",
            Some(&fixture),
            "<search-facade backend=\"opensearch\"/>",
            &[
                ("search_backend", "opensearch"),
                ("elasticsearch_url", url),
                ("elasticsearch_index_prefix", &prefix),
                ("opensearch_run", "false"),
            ],
        ),
        None => common::start_with_data("report-documents", &fixture),
    };
    let factory = gate.server.factory();
    let _vector_schema = vector.then(|| {
        use moqui_core::{driver::DbDriver, search::SearchBackend, transaction::{PoolConfig, TransactionManager}};
        let variable = |name: &str| std::env::var(format!("MONITOR_VECTOR_PG_{name}")).unwrap_or_else(|_| panic!("set MONITOR_VECTOR_PG_{name}"));
        let driver: Arc<dyn DbDriver> = Arc::new(moqui_host::db::postgres::PostgresDriver::tcp_no_tls(
            variable("USER"), variable("DATABASE"), variable("HOST"), variable("PORT").parse().unwrap(),
        ).password(variable("PASSWORD")));
        let tx = TransactionManager::new(driver, PoolConfig { min:0,max:4,reserve:1,..Default::default() }).unwrap();
        let schema = format!("report_vector_{}_{}", std::process::id(), jiff::Timestamp::now().as_microsecond());
        let backend = search::postgres::PostgresBackend::open(tx.clone(),Some(&schema)).unwrap().with_semantic(&search::semantic::SemanticConf {
            enabled:true,dimensions:64,representation:"vector".into(),max_chars:4000,embedder:Some("hash".into())
        }).unwrap();
        assert!(backend.capabilities().contains(&search::Capability::Semantic), "configured pgvector capability must actually be enabled");
        let facade = search::SearchFacade::new(Arc::new(backend), factory.entity().unwrap());
        facade.set_embedder(Some(Arc::new(search::semantic::HashEmbedder::new(64))));
        search::install(factory,facade);
        VectorSchema { tx, schema }
    });
    let facade = search::installed(factory).unwrap();
    viewer(factory, "john.doe", |ec| {
        let entity = factory.entity().unwrap();
        {
            let _setup = ec.disable_authz();
            for (kind, ids) in [
                (
                    "ReportParty",
                    vec![
                        "PARTY_A",
                        "PARTY_NOLINK",
                        "PARTY_BLOCKED",
                        "PARTY_OTHER",
                        "PARTY_LIMITED",
                    ],
                ),
                ("ReportProduct", vec!["PRODUCT_A", "PRODUCT_FORBIDDEN"]),
            ] {
                let ids = ids.into_iter().map(str::to_owned).collect::<Vec<_>>();
                let mut docs = moqui_core::entity::document::documents_by_ids(&entity, kind, &ids).unwrap();
                for doc in &mut docs {
                    let id = doc.get("_id").and_then(Value::as_str).unwrap();
                    let counts = match id {
                        "PARTY_A" => vec![1, 0, 0],
                        "PARTY_NOLINK" => vec![0, 1, 0],
                        "PRODUCT_A" => vec![0, 0, 1],
                        _ => vec![100, 100, 100],
                    };
                    doc.insert("bounds".into(), Value::List(vec![Value::I64(1), Value::I64(2)]));
                    doc.insert(
                        "buckets".into(),
                        Value::List(counts.into_iter().map(Value::I64).collect()),
                    );
                }
                assert_eq!(docs.len(), ids.len());
                assert_eq!(facade.index_documents(&docs).unwrap(), ids.len());
            }
            if vector {
                assert_eq!(facade.embed_pending(&search::Target::Index("report_mixed".into()),100,4000).unwrap(), 7);
                let mut nearest = search::SearchRequest::new(search::Query::MatchAll);
                nearest.retrieval = search::Retrieval::Semantic(search::hybrid::HybridOptions { text:"SECRET_OTHER_OWNER".into(),..Default::default() });
                let raw = facade.search(&search::Target::Index("report_mixed".into()), &nearest).unwrap();
                assert_eq!(raw.hits.first().unwrap().id,"PARTY_OTHER", "the unfiltered nearest result must be denied by the viewer's row policy");
            }
        }
        let tx = entity.transactions_for("moqui.report.Definition").unwrap();
        tx.run(TransactionAttribute::UseOrBegin,None,||report::save_definition(ec,&service(),&parameters(json!({"definition":{"id":"scoped-mixed","type":"dataset","title":"Mixed scoped records","userGroupId":"DOC_SHARED","body":{"indexName":"report_mixed","timeField":"time","rowFilter":{"field":{"name":"amount","op":"range","lt":100}},"contextFilters":[{"field":"enumTypeId","contextKey":"userId"}]}}})))).unwrap();
        let raw = report::documents(
            ec,
            &json!({"datasetId":"index:report_mixed","from":"now-1h","thru":"now+1h","pageSize":100}),
        )
        .unwrap();
        assert_eq!(
            raw["total"], 7,
            "administrator's derived index covers both actual DataDocuments"
        );
    });
    viewer(factory, "doc.both", |ec| {
        ec.user_context().insert("userId".into(), Value::string("DOC_OTHER"));
        let page = report::documents(ec, &options()).unwrap();
        assert_eq!(page["total"], 3);
        let hits = page["hits"].as_array().unwrap();
        assert_eq!(
            hits.iter()
                .map(|hit| hit["documentId"].as_str().unwrap())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["ReportParty", "ReportProduct"])
        );
        let party = hits.iter().find(|hit| hit["id"] == "PARTY_A").unwrap();
        assert_eq!(party["documentTitle"], "Party: Visible party");
        assert_eq!(party["links"][0]["url"], "/fixture/ReportParty/PARTY_A");
        let product = hits.iter().find(|hit| hit["id"] == "PRODUCT_A").unwrap();
        assert_eq!(product["documentTitle"], "Product: Visible product");
        assert!(
            hits.iter().find(|hit| hit["id"] == "PARTY_NOLINK").unwrap()["links"]
                .as_array()
                .unwrap()
                .is_empty(),
            "link conditions are evaluated separately for each authorized document"
        );
        let mut query = options();
        query["pageSize"] = json!(1);
        let mut ids = BTreeSet::new();
        loop {
            let page = report::documents(ec, &query).unwrap();
            assert_eq!(page["total"], 3);
            let hits = page["hits"].as_array().unwrap();
            if hits.is_empty() {
                break;
            }
            assert!(ids.insert(hits[0]["id"].as_str().unwrap().to_owned()));
            query["cursor"] = page["nextCursor"].clone();
        }
        assert_eq!(ids.len(), 3, "pagination counts only authorized rows across both types");
        let aggregated=report::execute_query(ec,&json!({"datasetId":"scoped-mixed","from":"now-1h","thru":"now+1h","aggs":{"total":{"sum":{"field":"amount"}},"types":{"terms":{"field":"_document"},"aggs":{"amount":{"sum":{"field":"amount"}}}}}})).unwrap();
        let frame: report::frame::Frame = serde_json::from_value(aggregated["frames"][0].clone()).unwrap();
        let rows = frame.rows();
        assert_eq!(
            rows.len(),
            2,
            "both authorized document types must have a bucket alongside root metrics: {aggregated}"
        );
        let per_type = rows
            .iter()
            .map(|row| (row["types"].as_str().unwrap(), row["amount"].as_f64().unwrap()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(per_type["ReportParty"], 10.0, "{aggregated}");
        assert_eq!(per_type["ReportProduct"], 5.0, "{aggregated}");
        assert_eq!(per_type.values().sum::<f64>(), 15.0);
        assert!(
            rows.iter().all(|row| row["total"].as_f64() == Some(15.0)),
            "the ungrouped scalar total uses the same authorized population: {aggregated}"
        );
        assert_eq!(aggregated["sourceCount"], 3);
        let rich=report::execute_query(ec,&json!({"datasetId":"scoped-mixed","from":"now-1h","thru":"now+1h","aggs":{"average":{"avg":{"field":"measure"}},"stats":{"stats":{"field":"measure"}},"median":{"median":{"field":"measure"}},"percentiles":{"percentiles":{"field":"measure","percents":[50,100]}},"owners":{"cardinality":{"field":"enumTypeId"}},"samples":{"top_hits":{"size":1,"sort":["-measure"]}},"distribution":{"histogram_merge":{"bounds":[1,2]}}}})).unwrap();
        let fields = rich["frames"][0]["fields"].as_array().unwrap();
        let value = |name: &str| {
            fields
                .iter()
                .find(|field| field["name"] == name)
                .unwrap_or_else(|| panic!("missing {name}: {rich}"))["values"][0]
                .clone()
        };
        assert_eq!(
            value("average").as_f64(),
            Some(10.0),
            "average must weight all three observations, not average per-type averages"
        );
        for (name, expected) in [
            ("stats.count", 3.0),
            ("stats.sum", 30.0),
            ("stats.min", 3.0),
            ("stats.max", 20.0),
            ("stats.avg", 10.0),
            ("median", 7.0),
            ("percentiles.50", 7.0),
            ("percentiles.100", 20.0),
            ("owners", 1.0),
        ] {
            assert_eq!(value(name).as_f64(), Some(expected), "{name}: {rich}");
        }
        for index in 0..3 {
            assert_eq!(value(&format!("distribution.bucket.{index}")).as_u64(), Some(1));
        }
        let sample: Json = serde_json::from_str(value("samples").as_str().unwrap()).unwrap();
        assert_eq!(sample[0]["id"], "PRODUCT_A");
        assert_eq!(sample[0]["documentId"], "ReportProduct");
        // Samples per bucket: Postgres ranks them for every bucket in one statement.
        let bucketed=report::execute_query(ec,&json!({"datasetId":"scoped-mixed","from":"now-1h","thru":"now+1h","aggs":{"category":{"terms":{"field":"category","size":10},"aggs":{"samples":{"top_hits":{"size":2,"sort":["-measure"]}}}}}})).unwrap();
        let frame: report::frame::Frame = serde_json::from_value(bucketed["frames"][0].clone()).unwrap();
        let samples: std::collections::BTreeMap<String, Vec<String>> = frame
            .rows()
            .iter()
            .map(|row| {
                let list: Json = match &row["samples"] {
                    Json::String(text) => serde_json::from_str(text).unwrap(),
                    other => other.clone(),
                };
                (
                    row["category"].as_str().unwrap_or_default().to_owned(),
                    list.as_array()
                        .unwrap()
                        .iter()
                        .map(|hit| hit["id"].as_str().unwrap().to_owned())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(samples["A"], ["PRODUCT_A"], "{bucketed}");
        assert_eq!(samples["Other"], ["PARTY_A"], "{bucketed}");
        for (size, order, missing) in [(10, "asc", "A"), (2, "desc", "Missing")] {
            let grouped=report::execute_query(ec,&json!({"datasetId":"scoped-mixed","from":"now-1h","thru":"now+1h","aggs":{"category":{"terms":{"field":"category","size":size,"order":{"_key":order},"missing":missing,"other_bucket":"Other"},"aggs":{"average":{"avg":{"field":"measure"}}}}}})).unwrap();
            let frame: report::frame::Frame = serde_json::from_value(grouped["frames"][0].clone()).unwrap();
            let dimension = frame.fields.iter().find(|field| field.name == "category").unwrap();
            let rows = frame.rows();
            assert_eq!(
                dimension.config["bucket"]["filters"].as_array().unwrap().len(),
                rows.len()
            );
            for (index, row) in rows.iter().enumerate() {
                let mut selected = options();
                selected["query"] = dimension.config["bucket"]["filters"][index].clone();
                let page = report::documents(ec, &selected).unwrap();
                assert_eq!(page["total"].as_f64(),row["count"].as_f64(),"canonical Missing/Other query must count exactly its row, including display-label collisions: {grouped}");
            }
            if size == 10 {
                assert!(dimension.values.iter().any(Json::is_null));
                assert!(dimension.values.iter().any(|value| value == "A"));
                assert_eq!(
                    dimension.config["bucket"]["labels"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|label| *label == "A")
                        .count(),
                    2
                );
            }
        }
        for (id, total) in [("PARTY_A", 1), ("PARTY_OTHER", 0), ("PARTY_BLOCKED", 0)] {
            let mut one = options();
            one["query"] = json!({"field":{"name":"_id","op":"eq","value":id}});
            assert_eq!(
                report::documents(ec, &one).unwrap()["total"],
                total,
                "single-document expansion cannot bypass filters"
            );
        }
        let mut ranked = options();
        ranked["queryString"] = json!("Visible");
        ranked["retrievalMode"] = json!("hybrid");
        if facade.capabilities().contains(&search::Capability::Hybrid) {
            let page = report::documents(ec, &ranked).unwrap();
            assert_eq!(page["total"], 2);
            assert!(page["hits"]
                .as_array()
                .unwrap()
                .iter()
                .all(|hit| ids.contains(hit["id"].as_str().unwrap())));
        } else {
            assert!(report::documents(ec, &ranked).is_err());
        }
        ranked["retrievalMode"] = json!("semantic");
        if !facade.capabilities().contains(&search::Capability::Semantic) {
            assert!(
                report::documents(ec, &ranked).is_err(),
                "unsupported semantic mode must not silently become lexical"
            );
        } else {
            for mode in ["semantic", "hybrid"] {
                ranked["retrievalMode"] = json!(mode);
                ranked["queryString"] = json!("SECRET_OTHER_OWNER");
                ranked.as_object_mut().unwrap().remove("sort");
                let page = report::documents(ec,&ranked).unwrap();
                let hits = page["hits"].as_array().unwrap();
                assert!(!hits.is_empty(), "{mode} must retrieve authorized candidates despite a denied nearest neighbor: {page}");
                assert!(hits.iter().all(|hit| ids.contains(hit["id"].as_str().unwrap())), "{mode} row policy must constrain ranking, not only returned bodies: {page}");
                assert!(!page.to_string().contains("SECRET_OTHER_OWNER"));
            }
        }
        assert!(
            report::documents(
                ec,
                &json!({"datasetId":"index:report_mixed","from":"now-1h","thru":"now+1h"})
            )
            .is_err(),
            "derived index may not bypass the scoped alias"
        );
    });
    viewer(factory, "doc.party", |ec| {
        let page = report::documents(ec, &options()).unwrap();
        assert_eq!(page["total"], 1);
        assert_eq!(page["hits"][0]["id"], "PARTY_LIMITED");
        let total=report::execute_query(ec,&json!({"datasetId":"scoped-mixed","from":"now-1h","thru":"now+1h","aggs":{"amount":{"sum":{"field":"amount"}}}})).unwrap();
        assert_eq!(
            total["frames"][0]["fields"][0]["values"][0].as_f64(),
            Some(11.0),
            "forbidden Product type must be removed before aggregation"
        );
        let mut forced = options();
        forced["documentType"] = json!("ReportProduct");
        forced["query"] = json!({"field":{"name":"_id","op":"eq","value":"PRODUCT_FORBIDDEN"}});
        assert_eq!(report::documents(ec, &forced).unwrap()["total"], 0);
        assert!(report::documents(
            ec,
            &json!({"datasetId":"ReportProduct","from":"now-1h","thru":"now+1h"})
        )
        .is_err());
    });
    let auth = common::basic("doc.both", "moqui");
    let path =
        "/apps/dashboards/Explore.html?datasetId=scoped-mixed&from=now-1h&thru=now%2B1h&queryString=_id%3APARTY_A";
    let (status, _, page) = common::exchange(
        gate.server.address(),
        &common::http_request("GET", path, &[("Authorization", &auth)], ""),
    );
    assert_eq!(status, 200, "{page}");
    assert!(
        page.contains("Party: Visible party") && page.contains("View document") && page.contains("PARTY_A"),
        "{page}"
    );
    assert!(
        !page.contains("SECRET_OTHER_OWNER") && !page.contains("BLOCKED_SAME_OWNER"),
        "dialog data must come only from authorized result rows"
    );
}
