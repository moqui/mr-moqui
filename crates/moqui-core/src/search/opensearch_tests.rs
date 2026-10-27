/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

use super::*;
use crate::search::mapping::FieldMapping;
use crate::search::parse::parse_query_string;

#[test]
fn telemetry_payloads_do_not_create_dynamic_mappings() {
    let mapping = crate::search::telemetry::telemetry_mapping("otel_logs").unwrap();
    let compiled = mapping_json(&mapping);
    assert_eq!(compiled["dynamic"], false);
    for field in ["attributes", "resource", "events"] {
        assert_eq!(compiled["properties"][field]["enabled"], false);
    }
    assert_eq!(compiled["properties"]["source.id"]["type"], "keyword");
    assert_eq!(compiled["properties"]["timestamp"]["type"], "date");
    assert!(mapping_json(&sales()).get("dynamic").is_none());
}

#[test]
#[ignore = "requires MONITOR_OPENSEARCH_URL pointing at a disposable test backend"]
fn telemetry_deferred_refresh_keeps_stable_ids_and_ordinary_write_visibility() {
    let mut backend = OpenSearchBackend::new(&ClusterConf {
        url: std::env::var("MONITOR_OPENSEARCH_URL").expect("MONITOR_OPENSEARCH_URL"),
        user: std::env::var("MONITOR_OPENSEARCH_USER").ok(),
        password: std::env::var("MONITOR_OPENSEARCH_PASSWORD").ok(),
        index_prefix: Some(format!("monitor-deferred-{}-{}-",std::process::id(),Timestamp::now().as_microsecond())),
    });
    // A regression that waits for refresh after it is disabled must fail promptly.
    backend.client = RestClient::new().timeout(std::time::Duration::from_secs(5));
    let mapping = crate::search::telemetry::telemetry_mapping("otel_logs").unwrap();
    backend.ensure_index(&mapping).unwrap();
    let index = backend.index_name(&mapping);
    let make = |body: &str| IndexMap::from_iter([
        ("timestamp".into(),Value::string("2026-01-01T00:00:00Z")),
        ("body".into(),Value::string(body)),
    ]);
    backend.index_documents(&mapping,&[("ordinary".into(),make("searchable immediately"))]).unwrap();
    let mappings = [Arc::new(mapping.clone())];
    assert_eq!(backend.search(&mappings,&SearchRequest::new(Query::MatchAll)).unwrap().total,1,
        "ordinary index_documents must still wait until searchable");
    backend.checked("PUT",&format!("{index}/_settings"),Some(Body::Json(json!({"index":{"refresh_interval":"-1"}})))).unwrap();
    assert_eq!(backend.index_telemetry_documents(&mapping,&[("retry-stable".into(),make("first attempt"))]).unwrap(),1);
    assert_eq!(backend.index_telemetry_documents(&mapping,&[("retry-stable".into(),make("retried replacement"))]).unwrap(),1);
    assert_eq!(backend.search(&mappings,&SearchRequest::new(Query::MatchAll)).unwrap().total,1,
        "deferred writes acknowledge storage without forcing a refresh");
    assert_eq!(backend.get_document(&mapping,"retry-stable").unwrap().unwrap()["body"],Value::string("retried replacement"));
    backend.checked("POST",&format!("{index}/_refresh"),None).unwrap();
    let page = backend.search(&mappings,&SearchRequest::new(Query::MatchAll)).unwrap();
    assert_eq!(page.total,2,"retry must replace one stable id, not duplicate it");
    assert_eq!(page.hits.iter().filter(|hit|hit.id=="retry-stable").count(),1);
    assert!(backend.index_telemetry_documents(&sales(),&[]).is_err(),"business indexes may not opt into telemetry deferral");
    backend.checked("DELETE",&index,None).unwrap();
}

/// Live acceptance is opt-in; the ordinary suite keeps exercising serialization with mocks.
#[test]
#[ignore = "requires MONITOR_OPENSEARCH_URL (and optional USER/PASSWORD) pointing at a disposable test backend"]
fn monitoring_aggregations_and_cursor_on_live_opensearch() {
    let backend = OpenSearchBackend::new(&ClusterConf {
        url: std::env::var("MONITOR_OPENSEARCH_URL").expect("MONITOR_OPENSEARCH_URL"),
        user: std::env::var("MONITOR_OPENSEARCH_USER").ok(),
        password: std::env::var("MONITOR_OPENSEARCH_PASSWORD").ok(),
        index_prefix: Some(format!("monitor-test-{}-", std::process::id())),
    });
    let mapping = crate::search::telemetry::telemetry_mapping("otel_metrics").unwrap();
    backend.ensure_index(&mapping).unwrap();
    let at = "2026-01-01T00:00:00Z";
    let documents:Vec<_>=[("a",6,Some("A")),("b",12,Some("A")),("c",30,None)].into_iter().map(|(id,sum,kind)|{
        let mut body=match moqui_lib::json::from_json(&json!({"name":"monitor-fixture","bucket_start":at,"sum":sum,"count":3,"bounds":[1,2],"buckets":[1,1,1]})){Value::Map(m)=>m,_=>unreachable!()};
        if let Some(kind)=kind{body.insert("moqui.artifact.sub_type".into(),Value::string(kind));}(id.to_owned(),body)
    }).collect();
    backend.index_documents(&mapping, &documents).unwrap();
    let aggs = Agg::map_from_value(&moqui_lib::json::from_json(&json!({
        "average":{"avg":{"field":"sum"}},"stats":{"stats":{"field":"sum"}},"median":{"median":{"field":"sum"}},
        "values":{"value_count":{"field":"sum"}},"percentile":{"percentiles":{"field":"sum","percents":[50,95]}},
        "time":{"date_histogram":{"field":"bucket_start","fixed_interval":"1h","min_doc_count":0,"extended_bounds":{"min":"2025-12-31T23:00:00Z","max":"2026-01-01T01:00:00Z"}}},
        "selected":{"filters":{"filters":{"small":{"range":{"sum":{"lt":10}}},"all":{"match_all":{}}}}},
        "kind":{"terms":{"field":"moqui.artifact.sub_type","size":1,"missing":"Missing","other_bucket":true}},
        "bands":{"range":{"field":"sum","ranges":[{"key":"small","to":10},{"key":"large","from":10}]}},
        "numeric":{"histogram":{"field":"sum","interval":10}},
        "sample":{"top_hits":{"size":1,"sort":["sum"]}},
        "latency":{"histogram_merge":{"bounds":[1,2]}}
    })))
    .unwrap();
    let output = backend
        .aggregate(&mapping, &Query::eq("name", Value::string("monitor-fixture")), &aggs)
        .unwrap();
    let output = moqui_lib::json::to_json(&Value::Map(output));
    assert_eq!(output["average"].as_f64(), Some(16.0));
    assert_eq!(output["stats"]["count"], 3);
    assert!((output["median"].as_f64().unwrap() - 12.0).abs() < 1.0);
    assert_eq!(output["kind"]["buckets"][0]["doc_count"], 2);
    assert_eq!(output["kind"]["buckets"][1]["doc_count"], 1);
    assert_eq!(output["sample"][0]["id"], "a");
    assert_eq!(output["latency"]["buckets"], json!([3, 3, 3]));
    assert_eq!(output["values"], 3);
    assert_eq!(output["time"]["buckets"].as_array().unwrap().len(), 3);
    let incompatible = Agg::map_from_value(&moqui_lib::json::from_json(
        &json!({"latency":{"histogram_merge":{"bounds":[1,3]}}}),
    ))
    .unwrap();
    assert!(backend.aggregate(&mapping, &Query::MatchAll, &incompatible).is_err());
    let mut request = SearchRequest::new(Query::MatchAll);
    request.sort = vec![
        SortField::parse("bucket_start").unwrap(),
        SortField::parse("__cursor").unwrap(),
    ];
    request.limit = 2;
    let first = backend
        .search(&[std::sync::Arc::new(mapping.clone())], &request)
        .unwrap();
    assert_eq!(first.hits.len(), 2);
    request.search_after = Some(vec![
        Value::string(at),
        Value::string(format!("{}:{}", mapping.document_id, first.hits.last().unwrap().id)),
    ]);
    let next = backend
        .search(&[std::sync::Arc::new(mapping.clone())], &request)
        .unwrap();
    assert_eq!(next.hits.len(), 1);
    assert_eq!(next.hits[0].id, "c");
    // Equal timestamps must traverse beyond OpenSearch's 10,000 offset window without repeats.
    let cursor_documents: Vec<_> = (0..10_025)
        .map(|at_index| {
            (
                format!("cursor-{at_index:05}"),
                IndexMap::from_iter([
                    ("name".into(), Value::string("cursor-fixture")),
                    ("bucket_start".into(), Value::string(at)),
                ]),
            )
        })
        .collect();
    assert_eq!(backend.index_documents(&mapping, &cursor_documents).unwrap(), 10_025);
    request.query = Query::eq("name", Value::string("cursor-fixture"));
    request.limit = 997;
    request.search_after = None;
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let page = backend
            .search(&[std::sync::Arc::new(mapping.clone())], &request)
            .unwrap();
        assert_eq!(page.total, 10_025);
        for hit in &page.hits {
            assert!(seen.insert(hit.id.clone()), "repeated {}", hit.id);
        }
        let Some(last) = page.hits.last() else {
            break;
        };
        request.search_after = Some(vec![
            Value::string(at),
            Value::string(format!("{}:{}", mapping.document_id, last.id)),
        ]);
    }
    assert_eq!(seen.len(), 10_025);
    let errors = crate::search::telemetry::telemetry_mapping("otel_errors").unwrap();
    backend.ensure_index(&errors).unwrap();
    let counter = IndexMap::from_iter([
        ("fingerprint".into(), Value::string("concurrent-fixture")),
        ("count".into(), Value::I64(1)),
    ]);
    backend
        .upsert_counter(&errors, "concurrent-fixture", &counter, "count")
        .unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..25 {
                    backend
                        .upsert_counter(&errors, "concurrent-fixture", &counter, "count")
                        .unwrap();
                }
            });
        }
    });
    assert_eq!(
        backend.get_document(&errors, "concurrent-fixture").unwrap().unwrap()["count"],
        Value::I64(101)
    );
    backend.drop_index(&errors).unwrap();
    let mut changed = mapping.clone();
    changed.root.fields.insert(
        "future_column".into(),
        FieldMapping {
            kind: FieldKind::Keyword,
            sortable: true,
        },
    );
    assert!(backend
        .ensure_index(&changed)
        .unwrap_err()
        .message()
        .contains("history is preserved"));
    assert!(backend.get_document(&mapping, "a").unwrap().is_some());
    backend.drop_index(&mapping).unwrap();
}

fn sales() -> DocumentMapping {
    let field = |kind| FieldMapping { kind, sortable: false };
    let mut root = ObjectMapping::default();
    for (name, kind) in [
        ("orderId", FieldKind::Keyword),
        ("productId", FieldKind::Keyword),
        ("itemTypeEnumId", FieldKind::Keyword),
        ("placedDate", FieldKind::Date),
        ("itemSaleTotal", FieldKind::Double),
        ("statusId", FieldKind::Keyword),
    ] {
        root.fields.insert(name.into(), field(kind));
    }
    root.fields.insert(
        "combinedName".into(),
        FieldMapping {
            kind: FieldKind::Text,
            sortable: true,
        },
    );
    let mut issuances = ObjectMapping::default();
    issuances
        .fields
        .insert("issuedSaleTotal".into(), field(FieldKind::Double));
    root.children.insert("issuances".into(), issuances);
    DocumentMapping {
        document_id: "MantleSalesOrderItem".into(),
        index_name: Some("mantle_sales".into()),
        physical_name: "mantle_sales_order_item".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    }
}

#[test]
fn sales_order_summary_body_matches_the_java_service() {
    let mapping = sales();
    let aggs = Agg::map_from_value(&moqui_lib::json::from_json(
        &serde_json::from_str(
            r#"{"DateOuter":{"date_histogram":{"field":"placedDate","calendar_interval":"1w","time_zone":"UTC",
                    "format":"dd MMM yyyy","order":{"_key":"desc"},"offset":"-1d"},
                  "aggs":{"orderCount":{"cardinality":{"field":"orderId","precision_threshold":40000}},
                          "ItemProduct":{"filter":{"term":{"itemTypeEnumId":"ItemProduct"}},
                             "aggs":{"productIssuedTotal":{"sum":{"field":"issuances.issuedSaleTotal"}}}}}}}"#,
        )
        .unwrap(),
    ))
    .unwrap();
    let json = aggs_json(&mapping, &aggs).unwrap();
    assert_eq!(
        json,
        json!({"DateOuter": {
        "date_histogram": {"field": "placedDate", "calendar_interval": "1w", "time_zone": "UTC",
            "format": "dd MMM yyyy", "offset": "-1d", "order": {"_key": "desc"}, "min_doc_count": 0},
        "aggs": {
            "ItemProduct": {"filter": {"term": {"itemTypeEnumId": "ItemProduct"}},
                "aggs": {"productIssuedTotal": {"sum": {"field": "issuances.issuedSaleTotal"}}}},
            "orderCount": {"cardinality": {"field": "orderId", "precision_threshold": 40000}}
        }}})
    );
    let range = Query::range(
        "placedDate",
        Some(Value::string("2026-01-01")),
        Some(Value::I64(1_769_904_000_000)),
    );
    assert_eq!(
        query_json(&mapping, &range),
        json!({"range": {"placedDate": {"gte": 1_767_225_600_000_i64, "lt": 1_769_904_000_000_i64}}})
    );
}

#[test]
fn min_and_max_aggregations_round_trip() {
    let mapping = sales();
    let aggs = Agg::map_from_value(&moqui_lib::json::from_json(
        &serde_json::from_str(r#"{"lo":{"min":{"field":"itemSaleTotal"}},"hi":{"max":{"field":"itemSaleTotal"}}}"#)
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(
        aggs_json(&mapping, &aggs).unwrap(),
        json!({"lo": {"min": {"field": "itemSaleTotal"}}, "hi": {"max": {"field": "itemSaleTotal"}}})
    );
    let out = read_aggs(&aggs, &json!({"lo": {"value": 2.5}, "hi": {"value": null}}));
    assert!(matches!(out["lo"], Value::Decimal(_)));
    assert_eq!(out["hi"], Value::Null);
}

#[test]
fn query_strings_compile_to_query_string_and_nested() {
    let mapping = sales();
    let query = Query::all(vec![
        parse_query_string("*:jo* AND combinedName:\"jo smith\"").unwrap(),
        Query::child("issuances", Query::range("issuedSaleTotal", Some(Value::I64(1)), None)),
    ]);
    let json = query_json(&mapping, &query);
    let text = json.to_string();
    assert!(text.contains(r#""query":"jo*""#), "{text}");
    assert!(text.contains(r#""query":"\"jo smith\"""#), "{text}");
    assert!(
        text.contains(r#""nested":{"path":"issuances","query":{"range":{"issuances.issuedSaleTotal":{"gte":1}}}"#),
        "{text}"
    );
    let mut request = SearchRequest::new(Query::MatchAll);
    request.sort = vec![crate::search::SortField::parse("-combinedName").unwrap()];
    let body = search_json(&mapping, &request);
    assert_eq!(
        body["sort"],
        json!([{"combinedName.keyword": {"order": "desc", "missing": "_last", "mode": "max", "unmapped_type": "keyword"}}])
    );
    assert_eq!(body["track_total_hits"], json!(true));
}

#[test]
fn reads_bucket_responses() {
    let mapping = sales();
    let aggs = Agg::map_from_value(&moqui_lib::json::from_json(
        &serde_json::from_str(
            r#"{"T":{"terms":{"field":"productId"},"aggs":{"s":{"sum":{"field":"itemSaleTotal"}},
                    "F":{"filter":{"term":{"itemTypeEnumId":"ItemProduct"}}}}}}"#,
        )
        .unwrap(),
    ))
    .unwrap();
    let _ = aggs_json(&mapping, &aggs).unwrap();
    let response =
        json!({"T": {"buckets": [{"key": "P1", "doc_count": 2, "s": {"value": 130.5}, "F": {"doc_count": 1}}]}});
    let out = read_aggs(&aggs, &response);
    let Value::Map(terms) = &out["T"] else { panic!() };
    let Value::List(buckets) = &terms["buckets"] else {
        panic!()
    };
    let Value::Map(bucket) = &buckets[0] else { panic!() };
    assert_eq!(to_plain_string(&bucket["key"]), "P1");
    assert_eq!(bucket["doc_count"], Value::I64(2));
    assert_eq!(to_plain_string(&bucket["s"]), "130.5");
    let Value::Map(filter) = &bucket["F"] else { panic!() };
    assert_eq!(filter["doc_count"], Value::I64(1));
}

#[test]
fn a_sort_resolves_against_every_type_and_names_an_unmapped_type() {
    let sales = sales();
    let mut party = sales.clone();
    party.document_id = "MantleParty".into();
    party.physical_name = "mantle_party".into();
    party.root.fields.clear();
    party.root.fields.insert(
        "pseudoId".into(),
        FieldMapping {
            kind: FieldKind::Keyword,
            sortable: false,
        },
    );
    let mut request = SearchRequest::new(Query::MatchAll);
    request.sort = ["combinedName", "-placedDate", "pseudoId", "_score"]
        .iter()
        .map(|text| SortField::parse(text).unwrap())
        .collect();
    // The party type is first and has none of the sales fields.
    let body = search_body(&[&party, &sales], &request);
    assert_eq!(
        body["sort"],
        json!([
            {"combinedName.keyword": {"order": "asc", "missing": "_last", "mode": "min", "unmapped_type": "keyword"}},
            {"placedDate": {"order": "desc", "missing": "_last", "mode": "max", "unmapped_type": "date"}},
            {"pseudoId": {"order": "asc", "missing": "_last", "mode": "min", "unmapped_type": "keyword"}},
            {"_score": {"order": "asc"}}
        ])
    );
}

/// A one-request-per-connection HTTP server that answers `replies` in order, then stops.
/// Joining it gives the request lines it saw.
fn serve(replies: Vec<(u16, Json)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                if let Some(split) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..split]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= split + 4 + length {
                        break;
                    }
                }
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => buf.extend_from_slice(&chunk[..size]),
                }
            }
            let text = String::from_utf8_lossy(&buf);
            seen.push(text.lines().next().unwrap_or_default().to_string());
            let body = body.to_string();
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
        seen
    });
    (url, handle)
}

fn backend(url: &str) -> OpenSearchBackend {
    OpenSearchBackend::new(&ClusterConf {
        url: url.to_string(),
        user: None,
        password: None,
        index_prefix: None,
    })
}

#[test]
fn index_exists_is_false_only_for_a_404() {
    let (url, server) = serve(vec![
        (404, json!({"error": {"type": "index_not_found_exception"}})),
        (503, json!({"error": {"reason": "cluster_block_exception"}})),
        (401, json!({"error": {"reason": "unauthorized"}})),
        (200, json!({"mantle_sales_order_item": {"mappings": {}}})),
    ]);
    let backend = backend(&url);
    let mapping = sales();
    assert!(!backend.index_exists(&mapping).unwrap());
    let err = backend.index_exists(&mapping).unwrap_err();
    assert!(err.message().contains("503"), "{err}");
    assert!(backend.index_exists(&mapping).is_err());
    assert!(backend.index_exists(&mapping).unwrap());
    assert_eq!(server.join().unwrap().len(), 4);
}

#[test]
fn shard_failures_and_partial_deletes_are_errors() {
    let (url, server) = serve(vec![
        (
            200,
            json!({"_shards": {"total": 2, "successful": 1, "failed": 1,
                "failures": [{"reason": {"reason": "No mapping found for [x] in order to sort on"}}]},
                "hits": {"total": {"value": 1}, "hits": []}}),
        ),
        (
            200,
            json!({"_shards": {"total": 1, "successful": 0, "failed": 1}, "aggregations": {}}),
        ),
        (
            200,
            json!({"deleted": 3, "timed_out": false,
                "failures": [{"cause": {"reason": "version conflict"}}]}),
        ),
        (
            200,
            json!({"_shards": {"total": 2, "successful": 2, "failed": 0},
                "hits": {"total": {"value": 0}, "hits": []}}),
        ),
    ]);
    let backend = backend(&url);
    let mapping = Arc::new(sales());
    let err = backend
        .search(&[Arc::clone(&mapping)], &SearchRequest::new(Query::MatchAll))
        .unwrap_err();
    assert!(err.message().contains("No mapping found"), "{err}");
    let err = backend
        .aggregate(&mapping, &Query::MatchAll, &IndexMap::new())
        .unwrap_err();
    assert!(err.message().contains("1 failed"), "{err}");
    let err = backend.delete_matching(&mapping, &Query::MatchAll).unwrap_err();
    assert!(err.message().contains("deleted 3"), "{err}");
    assert!(err.message().contains("version conflict"), "{err}");
    let page = backend
        .search(&[Arc::clone(&mapping)], &SearchRequest::new(Query::MatchAll))
        .unwrap();
    assert_eq!(page.total, 0);
    server.join().unwrap();
}

#[test]
fn a_failing_manual_mapping_service_fails_the_ensure_and_is_tried_again() {
    let ctx = crate::context::ExecutionContextFactory::test_context();
    ctx.factory().set_entity(crate::testing::open());
    let _entered = ctx.enter();
    let missing = || (404, json!({"error": {"type": "index_not_found_exception"}}));
    let (url, server) = serve(vec![missing(), missing()]);
    let backend = backend(&url);
    let mut mapping = sales();
    mapping.manual_mapping_service = Some("moqui.test.NoSuchServices.make#Mapping".into());
    let err = backend.ensure_index(&mapping).unwrap_err();
    assert!(err.message().contains("manual mapping service"), "{err}");
    assert!(lock(&backend.known).is_empty());
    assert!(backend.ensure_index(&mapping).is_err());
    // Each ensure looked again and neither made the index.
    let seen = server.join().unwrap();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(
        seen.iter()
            .all(|line| line.starts_with("GET /mantle_sales_order_item/_mapping")),
        "{seen:?}"
    );
    let errors = ctx.message().errors().to_vec();
    assert!(errors.is_empty(), "{errors:?}");
}
