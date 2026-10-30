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

//! SearchFacade on embedded Postgres.
//!
//! The first test drives the Postgres backend with mappings shaped like a sales order item and a party
//! document (`TestSalesOrderItem`, `TestParty`), and the aggregation and query trees a sales report and a
//! party search build. The second runs the data feed end
//! to end: entity writes, `DataFeedPending`, a service worker, `index#DataDocuments`, and
//! `search#DataDocuments`.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use indexmap::IndexMap;
use moqui_core::driver::DbDriver;
use moqui_core::entity::document::Document;
use moqui_core::search::agg::Agg;
use moqui_core::search::mapping::{DisplayFields, DocumentMapping, FieldKind, FieldMapping, ObjectMapping};
use moqui_core::search::parse::parse_query_string;
use moqui_core::search::postgres::PostgresBackend;
use moqui_core::search::{Capability, Query, Rank, SearchBackend, SearchRequest, SortField};
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_host::db::embedded::EmbeddedCluster;
use moqui_lib::object::{to_plain_string, Value};

#[test]
fn postgres_backend_sales_and_party_parity() {
    let _shared = crate::common::shared();
    let cluster_dir = temp_dir("moqui-pg-search");
    let _guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let tx = TransactionManager::new(Arc::clone(&driver), pool()).unwrap();
    let tx_for_lock = tx.clone();
    let backend = PostgresBackend::open(tx, None).unwrap();

    sales_parity(&backend);
    party_parity(&backend);
    child_dates_as_text_parity(&backend);
    child_dates_indexed_as_millis(&tx_for_lock, &backend);
    phrase_parity(&backend);
    extensions_parity(&backend);
    universal_parity(&backend);
    hybrid_parity(&backend);
    read_order_parity(&backend);
    ddl_under_own_lock(&tx_for_lock, &backend);
    accent(
        PostgresBackend::open(
            TransactionManager::new(Arc::clone(&driver), pool()).unwrap(),
            Some("search_accent"),
        )
        .unwrap()
        .with_analysis(None, true)
        .unwrap(),
    );

    drop(backend);
    drop(cluster);
}

/// The same parity checks against a live OpenSearch (or ElasticSearch 7+). Ignored unless asked for.
#[test]
#[ignore = "needs OpenSearch: OPENSEARCH_URL=http://127.0.0.1:9200 cargo test -p moqui-host --test it -- search --ignored"]
fn opensearch_backend_sales_and_party_parity() {
    let _shared = crate::common::shared();
    let url = std::env::var("OPENSEARCH_URL").expect("set OPENSEARCH_URL to run this test");
    let conf = moqui_core::search::conf::ClusterConf {
        url,
        user: std::env::var("OPENSEARCH_USER").ok(),
        password: std::env::var("OPENSEARCH_PASSWORD").ok(),
        index_prefix: Some(format!("moqui_search_test_{}_", std::process::id())),
    };
    let backend = moqui_core::search::opensearch::OpenSearchBackend::new(&conf);
    sales_parity(&backend);
    party_parity(&backend);
    child_dates_as_text_parity(&backend);
    phrase_parity(&backend);
    extensions_parity(&backend);
    universal_parity(&backend);
    hybrid_parity(&backend);
    read_order_parity(&backend);
}

/// Two deliveries of one record that finish out of order: the document read later stays, whichever is written last.
fn read_order_parity(backend: &dyn SearchBackend) {
    let mut mapping = catalog_mapping();
    mapping.document_id = "TestReadOrder".into();
    mapping.physical_name = "test_read_order".into();
    backend.ensure_index(&mapping).unwrap();
    let earlier: jiff::Timestamp = "2026-10-01T00:00:00Z".parse().unwrap();
    let later: jiff::Timestamp = "2026-10-01T00:00:01Z".parse().unwrap();
    backend
        .index_documents_at(&mapping, &[catalog_item("ITEM", "Newer title", "")], Some(later))
        .unwrap();
    backend
        .index_documents_at(&mapping, &[catalog_item("ITEM", "Older title", "")], Some(earlier))
        .unwrap();
    let stored = backend.get_document(&mapping, "ITEM").unwrap().expect("the document");
    assert_eq!(
        stored.get("title").map(to_plain_string).as_deref(),
        Some("Newer title"),
        "{}",
        backend.name()
    );
    // A read at the same time or later replaces it.
    backend
        .index_documents_at(&mapping, &[catalog_item("ITEM", "Same time", "")], Some(later))
        .unwrap();
    let stored = backend.get_document(&mapping, "ITEM").unwrap().expect("the document");
    assert_eq!(
        stored.get("title").map(to_plain_string).as_deref(),
        Some("Same time"),
        "{}",
        backend.name()
    );
    backend.drop_index(&mapping).unwrap();
}

fn field(kind: FieldKind) -> FieldMapping {
    FieldMapping { kind, sortable: false }
}

fn object(fields: &[(&str, FieldKind)]) -> ObjectMapping {
    let mut object = ObjectMapping::default();
    for (name, kind) in fields {
        object.fields.insert((*name).to_string(), field(*kind));
    }
    object
}

fn doc(pairs: Vec<(&str, Value)>) -> Document {
    pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect()
}

fn dec(text: &str) -> Value {
    moqui_lib::object::basic_convert(&Value::string(text), "BigDecimal").value
}

fn ts(text: &str) -> Value {
    Value::Timestamp(text.parse().unwrap())
}

fn list(items: Vec<Document>) -> Value {
    Value::List(items.into_iter().map(Value::Map).collect())
}

fn tree(text: &str) -> IndexMap<String, Agg> {
    Agg::map_from_value(&moqui_lib::json::from_json(&serde_json::from_str(text).unwrap())).unwrap()
}

fn query(text: &str) -> Query {
    Query::from_value(&moqui_lib::json::from_json(&serde_json::from_str(text).unwrap())).unwrap()
}

fn map_at<'a>(value: &'a Value, key: &str) -> &'a Value {
    match value {
        Value::Map(map) => map.get(key).unwrap_or_else(|| panic!("no {key} in {map:?}")),
        other => panic!("not a map at {key}: {other:?}"),
    }
}

fn buckets(value: &Value) -> Vec<Value> {
    match map_at(value, "buckets") {
        Value::List(items) => items.clone(),
        other => panic!("buckets: {other:?}"),
    }
}

fn number(value: &Value) -> String {
    match value {
        Value::Decimal(decimal) => decimal.normalize().to_string(),
        other => to_plain_string(other),
    }
}

fn sales_parity(backend: &dyn SearchBackend) {
    let mut root = object(&[
        ("orderId", FieldKind::Keyword),
        ("productId", FieldKind::Keyword),
        ("itemTypeEnumId", FieldKind::Keyword),
        ("placedDate", FieldKind::Date),
        ("itemSaleTotal", FieldKind::Double),
        ("orderQuantity", FieldKind::Double),
        ("partStatusId", FieldKind::Keyword),
        ("statusId", FieldKind::Keyword),
        ("isNewCustomer", FieldKind::Keyword),
        ("customerClassId", FieldKind::Keyword),
    ]);
    root.children.insert(
        "issuances".into(),
        object(&[
            ("issuedSaleTotal", FieldKind::Double),
            ("issuedCostTotal", FieldKind::Double),
        ]),
    );
    let mapping = DocumentMapping {
        document_id: "TestSalesOrderItem".into(),
        index_name: Some("test_sales".into()),
        physical_name: "test_sales_order_item".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    };
    backend.ensure_index(&mapping).unwrap();
    // Ensure again with the same shape is a no-op; the data stays.
    backend.ensure_index(&mapping).unwrap();
    assert!(backend.index_exists(&mapping).unwrap());

    let issuance = |sale: &str, cost: &str| doc(vec![("issuedSaleTotal", dec(sale)), ("issuedCostTotal", dec(cost))]);
    let item = |order: &str,
                product: Option<&str>,
                kind: &str,
                placed: &str,
                total: &str,
                quantity: &str,
                status: &str,
                new_customer: &str,
                class: Option<&str>,
                issued: Vec<Document>| {
        let mut body = doc(vec![
            ("orderId", Value::string(order)),
            ("itemTypeEnumId", Value::string(kind)),
            ("placedDate", ts(placed)),
            ("itemSaleTotal", dec(total)),
            ("orderQuantity", dec(quantity)),
            ("partStatusId", Value::string(status)),
            ("statusId", Value::string(status)),
            ("isNewCustomer", Value::string(new_customer)),
        ]);
        if let Some(product) = product {
            body.insert("productId".into(), Value::string(product));
        }
        if let Some(class) = class {
            body.insert("customerClassId".into(), Value::string(class));
        }
        if !issued.is_empty() {
            body.insert("issuances".into(), list(issued));
        }
        body
    };
    let docs = vec![
        (
            "O1::01".to_string(),
            item(
                "O1",
                Some("P1"),
                "ItemProduct",
                "2026-01-05T10:00:00Z",
                "100.00",
                "2",
                "OrderPlaced",
                "Y",
                Some("CustVip"),
                vec![issuance("60", "30"), issuance("40", "25")],
            ),
        ),
        (
            "O1::02".to_string(),
            item(
                "O1",
                None,
                "ItemDiscount",
                "2026-01-05T10:00:00Z",
                "-10.00",
                "1",
                "OrderPlaced",
                "Y",
                Some("CustVip"),
                vec![],
            ),
        ),
        (
            "O2::01".to_string(),
            item(
                "O2",
                Some("P2"),
                "ItemProduct",
                "2026-01-06T09:00:00Z",
                "50.00",
                "1",
                "OrderApproved",
                "N",
                None,
                vec![],
            ),
        ),
        (
            "O3::01".to_string(),
            item(
                "O3",
                Some("P1"),
                "ItemProduct",
                "2026-01-20T12:00:00Z",
                "30.00",
                "3",
                "OrderCompleted",
                "Y",
                Some("CustRetail"),
                vec![issuance("30", "12")],
            ),
        ),
        (
            "O4::01".to_string(),
            item(
                "O4",
                Some("P3"),
                "ItemProduct",
                "2026-01-07T12:00:00Z",
                "999.00",
                "1",
                "OrderCancelled",
                "N",
                Some("CustVip"),
                vec![],
            ),
        ),
    ];
    assert_eq!(backend.index_documents(&mapping, &docs).unwrap(), 5);
    // Indexing the same id again replaces it.
    assert_eq!(backend.index_documents(&mapping, &docs[..1]).unwrap(), 1);

    // get#SalesOrderSummary: bool.filter of a date range and status terms; weeks start Sunday (-1d).
    let statuses = r#"["OrderPlaced","OrderProcessing","OrderApproved","OrderSent","OrderCompleted","OrderHold"]"#;
    let filter = query(&format!(
        r#"{{"bool":{{"all":[
            {{"field":{{"name":"placedDate","op":"range","gte":1767225600000,"lt":1769904000000}}}},
            {{"field":{{"name":"partStatusId","op":"in","values":{statuses}}}}},
            {{"field":{{"name":"statusId","op":"in","values":{statuses}}}}}]}}}}"#
    ));
    let aggs = tree(
        r#"{"DateOuter":{"date_histogram":{"field":"placedDate","calendar_interval":"1w","time_zone":"UTC",
              "format":"dd MMM yyyy","order":{"_key":"desc"},"offset":"-1d"},
            "aggs":{
              "orderCount":{"cardinality":{"field":"orderId","precision_threshold":40000}},
              "totalSales":{"sum":{"field":"itemSaleTotal"}},
              "ItemProduct":{"filter":{"term":{"itemTypeEnumId":"ItemProduct"}},"aggs":{
                  "productSaleTotal":{"sum":{"field":"itemSaleTotal"}},
                  "productIssuedTotal":{"sum":{"field":"issuances.issuedSaleTotal"}},
                  "productCostTotal":{"sum":{"field":"issuances.issuedCostTotal"}},
                  "productQuantityTotal":{"sum":{"field":"orderQuantity"}}}},
              "ItemDiscount":{"filter":{"terms":{"itemTypeEnumId":["ItemDiscount","ItemPromoDiscount"]}},
                  "aggs":{"discountTotal":{"sum":{"field":"itemSaleTotal"}}}},
              "NewCustomer":{"filter":{"term":{"isNewCustomer":"Y"}},
                  "aggs":{"newCustomerOrderCount":{"cardinality":{"field":"orderId"}}}}}}}"#,
    );
    let out = backend.aggregate(&mapping, &filter, &aggs).unwrap();
    let weeks = buckets(&out["DateOuter"]);
    let shown: Vec<String> = weeks
        .iter()
        .map(|bucket| to_plain_string(map_at(bucket, "key_display")))
        .collect();
    assert_eq!(
        shown,
        vec!["18 Jan 2026", "11 Jan 2026", "04 Jan 2026"],
        "newest first, with the empty week kept"
    );
    let row = |bucket: &Value| -> Vec<String> {
        vec![
            number(map_at(bucket, "doc_count")),
            number(map_at(bucket, "orderCount")),
            number(map_at(bucket, "totalSales")),
            number(map_at(map_at(bucket, "ItemProduct"), "productSaleTotal")),
            number(map_at(map_at(bucket, "ItemProduct"), "productIssuedTotal")),
            number(map_at(map_at(bucket, "ItemProduct"), "productCostTotal")),
            number(map_at(map_at(bucket, "ItemProduct"), "productQuantityTotal")),
            number(map_at(map_at(bucket, "ItemDiscount"), "discountTotal")),
            number(map_at(map_at(bucket, "NewCustomer"), "newCustomerOrderCount")),
            number(map_at(map_at(bucket, "NewCustomer"), "doc_count")),
        ]
    };
    assert_eq!(
        row(&weeks[0]),
        vec!["1", "1", "30", "30", "30", "12", "3", "0", "1", "1"]
    );
    assert_eq!(row(&weeks[1]), vec!["0", "0", "0", "0", "0", "0", "0", "0", "0", "0"]);
    assert_eq!(
        row(&weeks[2]),
        vec!["3", "2", "140", "150", "100", "55", "3", "-10", "1", "2"]
    );
    assert_eq!(
        to_plain_string(map_at(&weeks[2], "key")),
        to_plain_string(&ts("2026-01-04T00:00:00Z"))
    );

    // The invoice histogram sends min_doc_count 1, which drops the empty week.
    let sparse = tree(
        r#"{"DateOuter":{"date_histogram":{"field":"placedDate","unit":"week","time_zone":"UTC","offset":"-1d",
            "format":"dd MMM yyyy","min_doc_count":1}}}"#,
    );
    let out = backend.aggregate(&mapping, &filter, &sparse).unwrap();
    assert_eq!(buckets(&out["DateOuter"]).len(), 2);

    // A quarter bucket and a month bucket in a zone west of UTC.
    let quarter = tree(
        r#"{"Q":{"date_histogram":{"field":"placedDate","unit":"1q","time_zone":"America/Denver","format":"yyyy-MM-dd"}}}"#,
    );
    let out = backend.aggregate(&mapping, &filter, &quarter).unwrap();
    let quarters = buckets(&out["Q"]);
    assert_eq!(quarters.len(), 1);
    assert_eq!(to_plain_string(map_at(&quarters[0], "key_display")), "2026-01-01");
    assert_eq!(number(map_at(&quarters[0], "doc_count")), "4");

    // A fixed ISO offset east of UTC: 2026-01-05T10:00Z is the 5th at 15:00 local, in the day that starts at
    // 2026-01-04T19:00Z. Postgres reads a text '+05:00' as POSIX (west), which would make it 2026-01-05T05:00Z.
    let fixed = tree(
        r#"{"D":{"date_histogram":{"field":"placedDate","unit":"day","time_zone":"+05:00","format":"yyyy-MM-dd",
            "min_doc_count":1}}}"#,
    );
    let out = backend.aggregate(&mapping, &filter, &fixed).unwrap();
    let days = buckets(&out["D"]);
    assert_eq!(
        to_plain_string(map_at(&days[0], "key")),
        to_plain_string(&ts("2026-01-04T19:00:00Z"))
    );
    assert_eq!(to_plain_string(map_at(&days[0], "key_display")), "2026-01-05");
    assert_eq!(number(map_at(&days[0], "doc_count")), "2");

    // get#SalesTopProduct: terms by product, ordered by a metric, null product left out.
    let top = tree(
        r#"{"TopTerms":{"terms":{"field":"productId","size":15,"order":{"totalSales":"desc"}},"aggs":{
            "totalQuantity":{"sum":{"field":"orderQuantity"}},"totalSales":{"sum":{"field":"itemSaleTotal"}},
            "productIssuedTotal":{"sum":{"field":"issuances.issuedSaleTotal"}}}}}"#,
    );
    let out = backend.aggregate(&mapping, &filter, &top).unwrap();
    let products: Vec<(String, String, String)> = buckets(&out["TopTerms"])
        .iter()
        .map(|bucket| {
            (
                to_plain_string(map_at(bucket, "key")),
                number(map_at(bucket, "totalSales")),
                number(map_at(bucket, "totalQuantity")),
            )
        })
        .collect();
    assert_eq!(
        products,
        vec![
            ("P1".into(), "130".into(), "5".into()),
            ("P2".into(), "50".into(), "1".into())
        ]
    );

    // get#InvoicesByCustomerClass: terms with no order is doc_count descending, then key.
    let classes = tree(r#"{"TopTerms":{"terms":{"field":"customerClassId","size":100}}}"#);
    let out = backend.aggregate(&mapping, &filter, &classes).unwrap();
    let keys: Vec<(String, String)> = buckets(&out["TopTerms"])
        .iter()
        .map(|bucket| {
            (
                to_plain_string(map_at(bucket, "key")),
                number(map_at(bucket, "doc_count")),
            )
        })
        .collect();
    assert_eq!(
        keys,
        vec![("CustVip".into(), "2".into()), ("CustRetail".into(), "1".into())]
    );

    // Top-level metrics, and nothing matched: zero, not null.
    let none = backend
        .aggregate(
            &mapping,
            &Query::eq("orderId", Value::string("NOPE")),
            &tree(r#"{"total":{"sum":{"field":"itemSaleTotal"}},"orders":{"cardinality":{"field":"orderId"}}}"#),
        )
        .unwrap();
    assert_eq!(number(&none["total"]), "0");
    assert_eq!(number(&none["orders"]), "0");

    // Retention delete by index time, then everything older than now.
    let removed = backend
        .delete_matching(
            &mapping,
            &Query::range_full("@timestamp", None, None, None, Some(Value::string("2000-01-01"))),
        )
        .unwrap();
    assert_eq!(removed, 0);
    assert!(backend.get_document(&mapping, "O3::01").unwrap().is_some());
    assert!(backend.delete_document(&mapping, "O3::01").unwrap());
    assert!(backend.get_document(&mapping, "O3::01").unwrap().is_none());
    backend.drop_index(&mapping).unwrap();
    assert!(!backend.index_exists(&mapping).unwrap());
}

fn party_mapping() -> DocumentMapping {
    let mut root = object(&[
        ("partyId", FieldKind::Keyword),
        ("pseudoId", FieldKind::Keyword),
        ("partyTypeEnumId", FieldKind::Keyword),
        ("ownerPartyId", FieldKind::Keyword),
    ]);
    root.fields.insert(
        "combinedName".into(),
        FieldMapping {
            kind: FieldKind::Text,
            sortable: true,
        },
    );
    root.children
        .insert("roles".into(), object(&[("roleTypeId", FieldKind::Keyword)]));
    root.children.insert(
        "classifications".into(),
        object(&[
            ("partyClassificationId", FieldKind::Keyword),
            ("classFromDate", FieldKind::Date),
            ("classThruDate", FieldKind::Date),
        ]),
    );
    let mut accounts = ObjectMapping::default();
    accounts.fields.insert(
        "username".into(),
        FieldMapping {
            kind: FieldKind::Text,
            sortable: true,
        },
    );
    root.children.insert("userAccounts".into(), accounts);
    root.children.insert(
        "contactMechs".into(),
        object(&[("city", FieldKind::Text), ("postalCode", FieldKind::Text)]),
    );
    DocumentMapping {
        document_id: "TestParty".into(),
        index_name: Some("test".into()),
        physical_name: "test_party".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    }
}

fn party_parity(backend: &dyn SearchBackend) {
    let mapping = party_mapping();
    backend.ensure_index(&mapping).unwrap();
    let role = |id: &str| doc(vec![("roleTypeId", Value::string(id))]);
    let class = |id: &str, from: &str, thru: Option<&str>| {
        let mut body = doc(vec![
            ("partyClassificationId", Value::string(id)),
            ("classFromDate", ts(from)),
        ]);
        if let Some(thru) = thru {
            body.insert("classThruDate".into(), ts(thru));
        }
        body
    };
    let account = |name: &str| doc(vec![("username", Value::string(name))]);
    let city = |name: &str| doc(vec![("city", Value::string(name))]);
    let docs = vec![
        (
            "P100".to_string(),
            doc(vec![
                ("partyId", Value::string("P100")),
                ("pseudoId", Value::string("AB100")),
                ("combinedName", Value::string("John Smith")),
                ("ownerPartyId", Value::string("ORG_A")),
                ("roles", list(vec![role("Customer")])),
                (
                    "classifications",
                    list(vec![class("CustVip", "2025-01-01T00:00:00Z", None)]),
                ),
                ("userAccounts", list(vec![account("john.smith")])),
                ("contactMechs", list(vec![city("Springfield")])),
            ]),
        ),
        (
            "P101".to_string(),
            doc(vec![
                ("partyId", Value::string("P101")),
                ("pseudoId", Value::string("CD101")),
                ("combinedName", Value::string("Jane Johnson")),
                ("ownerPartyId", Value::string("ORG_B")),
                ("roles", list(vec![role("Customer"), role("Supplier")])),
                (
                    "classifications",
                    list(vec![
                        class("CustVip", "2025-01-01T00:00:00Z", Some("2025-06-01T00:00:00Z")),
                        class("CustRetail", "2025-06-01T00:00:00Z", None),
                    ]),
                ),
                ("userAccounts", list(vec![account("jjohnson")])),
            ]),
        ),
        (
            "P102".to_string(),
            doc(vec![
                ("partyId", Value::string("P102")),
                ("pseudoId", Value::string("EF102")),
                ("combinedName", Value::string("Acme Corp")),
                ("roles", list(vec![role("Supplier")])),
                ("contactMechs", list(vec![city("Johnstown")])),
            ]),
        ),
    ];
    backend.index_documents(&mapping, &docs).unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let ids = |request: &SearchRequest| -> (Vec<String>, u64) {
        let page = backend.search(&mappings, request).unwrap();
        let mut ids: Vec<String> = page.hits.iter().map(|hit| hit.id.clone()).collect();
        if request.sort.is_empty() {
            ids.sort();
        }
        (ids, page.total)
    };
    let request = |text: &str| SearchRequest::new(parse_query_string(text).unwrap());

    // search#Party: auto-wildcard text over every field, AND a dotted child field.
    assert_eq!(
        ids(&request("(*:jo*) AND roles.roleTypeId:Customer")).0,
        vec!["P100", "P101"]
    );
    assert_eq!(
        ids(&request("*:jo*")).0,
        vec!["P100", "P101", "P102"],
        "Johnstown is a contact city"
    );
    assert_eq!(ids(&request("combinedName:*ohn*")).0, vec!["P100", "P101"]);
    assert_eq!(ids(&request("pseudoId:AB*")).0, vec!["P100"]);
    assert_eq!(
        ids(&request("pseudoId:ab*")).0,
        Vec::<String>::new(),
        "keyword prefix is case-sensitive"
    );
    assert_eq!(ids(&request("ownerPartyId:(ORG_A OR _NA_)")).0, vec!["P100"]);
    assert_eq!(ids(&request("_exists_:ownerPartyId")).0, vec!["P100", "P101"]);
    assert_eq!(ids(&request("NOT _exists_:ownerPartyId")).0, vec!["P102"]);
    assert_eq!(ids(&request("*:* -roles.roleTypeId:Customer")).0, vec!["P102"]);
    assert_eq!(ids(&request("combinedName:\"jane johnson\"")).0, vec!["P101"]);

    // Any element: P101 had CustVip once. One element, now: only P100's classification is current.
    assert_eq!(
        ids(&request("classifications.partyClassificationId:CustVip")).0,
        vec!["P100", "P101"]
    );
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let nested = moqui_core::search::es_json::from_es(&Value::string(format!(
        r#"{{"bool":{{"filter":[
            {{"term":{{"classifications.partyClassificationId":"CustVip"}}}},
            {{"range":{{"classifications.classFromDate":{{"lte":{now}}}}}}}
        ],"should":[
            {{"range":{{"classifications.classThruDate":{{"gte":{now}}}}}}},
            {{"bool":{{"must_not":[{{"exists":{{"field":"classifications.classThruDate"}}}}]}}}}
        ],"minimum_should_match":1}}}}"#
    )))
    .unwrap();
    let child = SearchRequest::new(Query::all(vec![
        parse_query_string("*:*").unwrap(),
        Query::child("classifications", nested),
    ]));
    assert_eq!(ids(&child).0, vec!["P100"]);

    // Sort on a sortable text field and on a child field (max, descending, missing last).
    let mut sorted = request("*:*");
    sorted.sort = vec![SortField::parse("combinedName.keyword").unwrap()];
    assert_eq!(ids(&sorted).0, vec!["P102", "P101", "P100"]);
    sorted.sort = vec![SortField::parse("-userAccounts.username.keyword").unwrap()];
    assert_eq!(ids(&sorted).0, vec!["P100", "P101", "P102"]);
    // Paging keeps the exact total.
    sorted.limit = 1;
    sorted.offset = 1;
    assert_eq!(ids(&sorted), (vec!["P101".to_string()], 3));

    // Highlights on the requested fields, with <em> markers.
    let mut highlighted = request("(*:jo*) AND roles.roleTypeId:Customer");
    highlighted.highlight = vec!["combinedName".into(), "contactMechs.city".into()];
    let page = backend.search(&mappings, &highlighted).unwrap();
    let john = page.hits.iter().find(|hit| hit.id == "P100").unwrap();
    assert_eq!(john.highlights["combinedName"], vec!["<em>John</em> Smith".to_string()]);
    assert!(!john.highlights.contains_key("contactMechs.city"));
    assert!(john.score > 0.0, "a text clause scores");

    // An index with two document types searches both.
    let mut product_root = object(&[("productId", FieldKind::Keyword)]);
    product_root.fields.insert("productName".into(), field(FieldKind::Text));
    let product = DocumentMapping {
        document_id: "TestProduct".into(),
        index_name: Some("test".into()),
        physical_name: "test_product".into(),
        root: product_root,
        manual_mapping_service: None,
        display: Default::default(),
    };
    backend.ensure_index(&product).unwrap();
    backend
        .index_documents(
            &product,
            &[(
                "WIDGET".to_string(),
                doc(vec![
                    ("productId", Value::string("WIDGET")),
                    ("productName", Value::string("Johnny Widget")),
                ]),
            )],
        )
        .unwrap();
    let both = vec![Arc::new(mapping.clone()), Arc::new(product.clone())];
    let page = backend.search(&both, &request("*:johnn*")).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].document_id, "TestProduct");
    assert_eq!(page.hits[0].index, "test_product");
    let page = backend.search(&both, &request("*:jo*")).unwrap();
    assert_eq!(page.total, 4);

    backend.drop_index(&product).unwrap();
    backend.drop_index(&mapping).unwrap();
}

/// Child-list dates as `index#DataDocuments` in JavaScript sends them: epoch-millis text (a script sees a timestamp as
/// its millis). A range and an exists test inside the child list compare them as times; on Postgres the text once went
/// into the body as it was and the `timestamptz` cast of a query failed ("date/time field value out of range").
fn child_dates_as_text_parity(backend: &dyn SearchBackend) {
    let mut mapping = party_mapping();
    mapping.document_id = "TestChildDates".into();
    mapping.physical_name = "test_child_dates".into();
    backend.ensure_index(&mapping).unwrap();
    let millis = |text: &str| text.parse::<jiff::Timestamp>().unwrap().as_millisecond().to_string();
    let class = |id: &str, from: &str, thru: Option<&str>| {
        let mut body = doc(vec![
            ("partyClassificationId", Value::string(id)),
            ("classFromDate", Value::string(millis(from))),
        ]);
        if let Some(thru) = thru {
            body.insert("classThruDate".into(), Value::string(millis(thru)));
        }
        body
    };
    let party = |id: &str, classes: Vec<Document>| {
        (
            id.to_string(),
            doc(vec![("partyId", Value::string(id)), ("classifications", list(classes))]),
        )
    };
    backend
        .index_documents(
            &mapping,
            &[
                party("C1", vec![class("CustVip", "2025-01-01T00:00:00Z", None)]),
                party(
                    "C2",
                    vec![class("CustVip", "2025-01-01T00:00:00Z", Some("2025-06-01T00:00:00Z"))],
                ),
                party("C3", vec![class("CustRetail", "2026-03-01T00:00:00Z", None)]),
            ],
        )
        .unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let ids = |query: Query| -> Vec<String> {
        let page = backend.search(&mappings, &SearchRequest::new(query)).unwrap();
        let mut ids: Vec<String> = page.hits.iter().map(|hit| hit.id.clone()).collect();
        ids.sort();
        ids
    };
    let text = |text: &str| parse_query_string(text).unwrap();
    let nested = |json: &str| {
        Query::child(
            "classifications",
            moqui_core::search::es_json::from_es(&Value::string(json)).unwrap(),
        )
    };
    let name = backend.name();
    // A range inside the child list, by a date and by epoch millis.
    assert_eq!(
        ids(text("classifications.classFromDate:[2024-12-31 TO 2025-01-02]")),
        vec!["C1", "C2"],
        "{name}"
    );
    let june = millis("2025-06-01T00:00:00Z");
    assert_eq!(
        ids(nested(&format!(
            r#"{{"range":{{"classifications.classThruDate":{{"lte":{june}}}}}}}"#
        ))),
        vec!["C2"],
        "{name}"
    );
    // Exists and not exists of a child date.
    assert_eq!(
        ids(text("_exists_:classifications.classThruDate")),
        vec!["C2"],
        "{name}"
    );
    assert_eq!(
        ids(nested(
            r#"{"bool":{"must_not":[{"exists":{"field":"classifications.classThruDate"}}]}}"#
        )),
        vec!["C1", "C3"],
        "{name}"
    );
    // The current classification of one element: from on or before now, thru after now or none.
    let now = jiff::Timestamp::now().as_millisecond();
    assert_eq!(
        ids(nested(&format!(
            r#"{{"bool":{{"filter":[{{"range":{{"classifications.classFromDate":{{"lte":{now}}}}}}}],"should":[
                {{"range":{{"classifications.classThruDate":{{"gte":{now}}}}}}},
                {{"bool":{{"must_not":[{{"exists":{{"field":"classifications.classThruDate"}}}}]}}}}
            ],"minimum_should_match":1}}}}"#
        ))),
        vec!["C1", "C3"],
        "{name}"
    );
    backend.drop_index(&mapping).unwrap();
}

/// Child-list dates of a document indexed before 2026-10-19, which kept the epoch-millis text a script sent in the body:
/// a range and the exists test still compare them as times, beside a document with ISO times and one with a bare date.
fn child_dates_indexed_as_millis(tx: &TransactionManager, backend: &PostgresBackend) {
    let mut mapping = party_mapping();
    mapping.document_id = "TestChildDatesMillis".into();
    mapping.physical_name = "test_child_dates_millis".into();
    backend.ensure_index(&mapping).unwrap();
    let class = |from: &str, thru: Option<&str>| {
        let mut body = doc(vec![
            ("partyClassificationId", Value::string("CustVip")),
            ("classFromDate", Value::string(from)),
        ]);
        if let Some(thru) = thru {
            body.insert("classThruDate".into(), Value::string(thru));
        }
        body
    };
    let party = |id: &str, from: &str, thru: Option<&str>| {
        (
            id.to_string(),
            doc(vec![
                ("partyId", Value::string(id)),
                ("classifications", list(vec![class(from, thru)])),
            ]),
        )
    };
    backend
        .index_documents(
            &mapping,
            &[
                party("OLD", "2025-01-01T00:00:00Z", Some("2025-06-01T00:00:00Z")),
                party("ISO", "2025-01-01T00:00:00Z", None),
                party("DAY", "2026-03-01", None),
            ],
        )
        .unwrap();
    // Put back the body an old index wrote: the millis as text.
    let millis = |text: &str| text.parse::<jiff::Timestamp>().unwrap().as_millisecond();
    let table = format!("{}.test_child_dates_millis", backend.schema());
    tx.with_connection(|conn| {
        conn.execute(
            &format!(
                "UPDATE {table} SET body = jsonb_set(body, '{{classifications}}', \
                 jsonb_build_array(jsonb_build_object('partyClassificationId', 'CustVip', \
                 'classFromDate', ?::text, 'classThruDate', ?::text))) WHERE doc_id = 'OLD'"
            ),
            &[
                Value::string(millis("2025-01-01T00:00:00Z").to_string()),
                Value::string(millis("2025-06-01T00:00:00Z").to_string()),
            ],
        )
    })
    .unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let ids = |text: &str| -> Vec<String> {
        let page = backend
            .search(&mappings, &SearchRequest::new(parse_query_string(text).unwrap()))
            .unwrap();
        let mut ids: Vec<String> = page.hits.iter().map(|hit| hit.id.clone()).collect();
        ids.sort();
        ids
    };
    assert_eq!(
        ids("classifications.classFromDate:[2024-12-31 TO 2025-01-02]"),
        vec!["ISO", "OLD"]
    );
    assert_eq!(
        ids("classifications.classThruDate:[2025-05-31 TO 2025-06-02]"),
        vec!["OLD"]
    );
    assert_eq!(
        ids("classifications.classFromDate:[2026-02-28 TO 2026-03-02]"),
        vec!["DAY"]
    );
    assert_eq!(ids("_exists_:classifications.classThruDate"), vec!["OLD"]);
    backend.drop_index(&mapping).unwrap();
}

/// A quoted phrase: those words, in that order, next to each other, after the `simple` analysis
/// (lower case, no stemming, stop words kept). It does not run from one stored value into the next.
fn phrase_parity(backend: &dyn SearchBackend) {
    let mut root = object(&[("noteId", FieldKind::Keyword)]);
    root.fields.insert("title".into(), field(FieldKind::Text));
    root.fields.insert("noteText".into(), field(FieldKind::Text));
    root.children
        .insert("lines".into(), object(&[("lineText", FieldKind::Text)]));
    let mapping = DocumentMapping {
        document_id: "TestPhraseNote".into(),
        index_name: None,
        physical_name: "test_phrase_note".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    };
    backend.ensure_index(&mapping).unwrap();
    let note = |id: &str, title: &str, text: &str, lines: &[&str]| {
        (
            id.to_string(),
            doc(vec![
                ("noteId", Value::string(id)),
                ("title", Value::string(title)),
                ("noteText", Value::string(text)),
                (
                    "lines",
                    list(
                        lines
                            .iter()
                            .map(|line| doc(vec![("lineText", Value::string(*line))]))
                            .collect(),
                    ),
                ),
            ]),
        )
    };
    let docs = vec![
        note("ADJACENT", "Pump Notes", "The Water Pump is industrial", &[]),
        note("REVERSED", "Reversed", "pump water", &[]),
        note("SEPARATED", "Separated", "water for the pump", &[]),
        // "water" ends one stored value and "pump" starts the next.
        note("SPLIT", "pump station", "fresh water", &["clean water", "pump house"]),
    ];
    backend.index_documents(&mapping, &docs).unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let ids = |text: &str| -> Vec<String> {
        let page = backend
            .search(&mappings, &SearchRequest::new(parse_query_string(text).unwrap()))
            .unwrap();
        let mut ids: Vec<String> = page.hits.iter().map(|hit| hit.id.clone()).collect();
        ids.sort();
        ids
    };

    assert_eq!(
        ids("noteText:\"water pump\""),
        vec!["ADJACENT"],
        "adjacent, in order, any case"
    );
    assert_eq!(ids("noteText:\"pump water\""), vec!["REVERSED"], "order matters");
    assert_eq!(
        ids("noteText:\"for the pump\""),
        vec!["SEPARATED"],
        "stop words are kept"
    );
    assert_eq!(ids("noteText:\"the water pump\""), vec!["ADJACENT"]);
    assert_eq!(
        ids("\"water pump\""),
        vec!["ADJACENT"],
        "every field: not across stored values"
    );
    assert_eq!(ids("\"pump house\""), vec!["SPLIT"], "a child value");
    assert_eq!(
        ids("lines.lineText:\"water pump\""),
        Vec::<String>::new(),
        "not across child elements"
    );
    assert_eq!(
        ids("noteText:water AND noteText:pump"),
        vec!["ADJACENT", "REVERSED", "SEPARATED"]
    );

    // Highlight marks the phrase where its words are next to each other, not each word alone.
    let mut highlighted = SearchRequest::new(parse_query_string("\"water pump\"").unwrap());
    highlighted.highlight = vec!["noteText".into(), "title".into()];
    let page = backend.search(&mappings, &highlighted).unwrap();
    assert_eq!(page.hits.len(), 1);
    assert_eq!(
        page.hits[0].highlights["noteText"],
        vec!["The <em>Water</em> <em>Pump</em> is industrial".to_string()]
    );
    assert!(
        !page.hits[0].highlights.contains_key("title"),
        "Pump Notes has no water before pump"
    );

    backend.drop_index(&mapping).unwrap();
}

fn catalog_mapping() -> DocumentMapping {
    let mut root = object(&[("itemId", FieldKind::Keyword)]);
    for name in ["title", "subtitle", "description"] {
        root.fields.insert(name.into(), field(FieldKind::Text));
    }
    root.children
        .insert("notes".into(), object(&[("noteText", FieldKind::Text)]));
    let mut display = DisplayFields::default();
    display.roles.insert("title".into(), "title".into());
    display.weights.insert("title".into(), 'A');
    display.weights.insert("subtitle".into(), 'B');
    DocumentMapping {
        document_id: "TestCatalogItem".into(),
        index_name: None,
        physical_name: "test_catalog_item".into(),
        root,
        manual_mapping_service: None,
        display,
    }
}

fn catalog_item(id: &str, title: &str, description: &str) -> (String, Document) {
    (
        id.to_string(),
        doc(vec![
            ("itemId", Value::string(id)),
            ("title", Value::string(title)),
            ("description", Value::string(description)),
        ]),
    )
}

/// The opt-in lexical extensions both backends have: proximity, fuzzy, autocomplete, and weighted
/// ranking.
fn extensions_parity(backend: &dyn SearchBackend) {
    for need in [
        Capability::Proximity,
        Capability::Fuzzy,
        Capability::Autocomplete,
        Capability::WeightedRank,
    ] {
        assert!(
            backend.capabilities().contains(&need),
            "{} lacks {need:?}",
            backend.name()
        );
    }
    let mapping = catalog_mapping();
    backend.ensure_index(&mapping).unwrap();
    let docs = vec![
        catalog_item("PUMP1", "Industrial Water Pump", "heavy duty pump for water"),
        catalog_item("PUMP2", "Water Heater", "the pump is sold separately"),
        catalog_item("FILTER", "Pump Station Filter", "filter for water pumping"),
        // "water" ends the title and "pump" starts the description.
        catalog_item("SPLIT", "Fresh water", "pump house"),
        catalog_item("WINDOW", "Garden Hose", "water one two three four pump"),
    ];
    backend.index_documents(&mapping, &docs).unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let run = |query: Query, rank: Rank| -> Vec<String> {
        let mut request = SearchRequest::new(query);
        request.rank = rank;
        backend
            .search(&mappings, &request)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.id)
            .collect()
    };
    let sorted = |mut ids: Vec<String>| {
        ids.sort();
        ids
    };
    let near = |terms: &str, fields: &str, distance: u32| {
        query(&format!(
            r#"{{"near": {{"terms": "{terms}", "fields": {fields}, "distance": {distance}}}}}"#
        ))
    };

    // Near: in order, at most `distance` other words between, inside one stored value.
    assert_eq!(
        sorted(run(near("water pump", "\"all\"", 0), Rank::Default)),
        vec!["PUMP1"]
    );
    assert_eq!(
        sorted(run(near("water pump", "\"all\"", 3), Rank::Default)),
        vec!["PUMP1"],
        "not from one stored value into the next"
    );
    assert_eq!(
        sorted(run(near("water pump", "[\"description\"]", 4), Rank::Default)),
        vec!["WINDOW"]
    );
    assert!(run(near("water pump", "[\"description\"]", 3), Rank::Default).is_empty());
    // Three words: the other words between all of them count against the one distance.
    assert_eq!(
        run(near("water two pump", "[\"description\"]", 3), Rank::Default),
        vec!["WINDOW"]
    );
    assert!(run(near("water two pump", "[\"description\"]", 2), Rank::Default).is_empty());
    assert!(run(near("two water pump", "[\"description\"]", 9), Rank::Default).is_empty());
    assert!(
        run(near("pump water", "[\"title\"]", 5), Rank::Default).is_empty(),
        "order matters"
    );

    // Fuzzy: a misspelled title still finds it; other titles do not.
    assert_eq!(
        run(query(r#"{"fuzzy": {"terms": "industral water pump"}}"#), Rank::Default),
        vec!["PUMP1"]
    );

    // Suggest: a title start, then word starts in titles.
    let suggested = run(query(r#"{"suggest": {"text": "wat"}}"#), Rank::Default);
    assert_eq!(sorted(suggested.clone()), vec!["PUMP1", "PUMP2", "SPLIT"]);
    assert_eq!(
        suggested[0], "PUMP2",
        "the title that starts with it is first: {suggested:?}"
    );
    assert_eq!(
        run(query(r#"{"suggest": {"text": "pump sta"}}"#), Rank::Default),
        vec!["FILTER"]
    );

    // Weighted ranking: a title match (weight A) outranks a description match.
    let ranked = run(parse_query_string("water").unwrap(), Rank::CoverDensity);
    assert_eq!(ranked.len(), 5, "{ranked:?}");
    assert_eq!(ranked.last().map(String::as_str), Some("WINDOW"), "{ranked:?}");
    assert!(
        ranked[..3]
            .iter()
            .all(|id| ["PUMP1", "PUMP2", "SPLIT"].contains(&id.as_str())),
        "{ranked:?}"
    );

    backend.drop_index(&mapping).unwrap();
}

/// What universal search needs from the facade, across two document types: a trusted per-type
/// filter that applies before totals, paging, and highlights, and a document-type facet that
/// counts only what the filter lets through.
fn universal_parity(backend: &dyn SearchBackend) {
    let mut first = catalog_mapping();
    first.document_id = "TestUniA".into();
    first.physical_name = "test_uni_a".into();
    let mut second = catalog_mapping();
    second.document_id = "TestUniB".into();
    second.physical_name = "test_uni_b".into();
    for mapping in [&first, &second] {
        backend.ensure_index(mapping).unwrap();
    }
    backend
        .index_documents(
            &first,
            &[
                catalog_item("A1", "Water Pump", "allowed"),
                catalog_item("A2", "Water Tank", "allowed"),
                catalog_item("A3", "Water Secret", "not allowed"),
            ],
        )
        .unwrap();
    backend
        .index_documents(
            &second,
            &[
                catalog_item("B1", "Water Hose", "allowed"),
                catalog_item("B2", "Water Valve", "not allowed"),
            ],
        )
        .unwrap();
    let both = vec![Arc::new(first.clone()), Arc::new(second.clone())];
    // A per-type predicate, as a service builds from the user's authorization.
    let allowed = query(
        r#"{"bool": {"any": [
            {"bool": {"all": [{"field": {"name": "_document", "value": "TestUniA"}},
                              {"field": {"name": "itemId", "op": "in", "values": ["A1", "A2"]}}]}},
            {"bool": {"all": [{"field": {"name": "_document", "value": "TestUniB"}},
                              {"field": {"name": "itemId", "value": "B1"}}]}}
        ]}}"#,
    );
    let mut request = SearchRequest::new(parse_query_string("water").unwrap());
    request.filter = Some(allowed.clone());
    request.highlight = vec!["title".into(), "description".into()];
    let page = backend.search(&both, &request).unwrap();
    let mut ids: Vec<String> = page.hits.iter().map(|hit| hit.id.clone()).collect();
    ids.sort();
    assert_eq!(ids, vec!["A1", "A2", "B1"]);
    assert_eq!(page.total, 3, "the total counts only what the filter allows");
    let a1 = page.hits.iter().find(|hit| hit.id == "A1").unwrap();
    assert_eq!(a1.highlights["title"], vec!["<em>Water</em> Pump".to_string()]);
    assert!(
        !a1.highlights.contains_key("description"),
        "the filter does not highlight"
    );
    request.limit = 1;
    request.offset = 2;
    assert_eq!(
        backend.search(&both, &request).unwrap().total,
        3,
        "paging keeps the filtered total"
    );

    // What the document-type facet counts per type (the facade test is in the feed scenario).
    let filtered = Query::all(vec![parse_query_string("water").unwrap(), allowed]);
    for mapping in [&first, &second] {
        let request = {
            let mut request = SearchRequest::new(filtered.clone());
            request.limit = 0;
            request
        };
        let total = backend.search(&[Arc::new((*mapping).clone())], &request).unwrap().total;
        let expected = if mapping.document_id == "TestUniA" { 2 } else { 1 };
        assert_eq!(total, expected, "{}", mapping.document_id);
    }

    for mapping in [&first, &second] {
        backend.drop_index(mapping).unwrap();
    }
}

/// Hybrid retrieval without an embedder: the lexical, title-prefix, and fuzzy branches fused by
/// reciprocal rank, with the trusted filter on every branch and in the total.
fn hybrid_parity(backend: &dyn SearchBackend) {
    use moqui_core::search::hybrid::{self, HybridOptions};
    use moqui_core::search::TotalRelation;

    assert!(backend.capabilities().contains(&Capability::Hybrid));
    let mapping = catalog_mapping();
    backend.ensure_index(&mapping).unwrap();
    backend
        .index_documents(
            &mapping,
            &[
                catalog_item("PUMP1", "Industrial Water Pump", "heavy duty pump for water"),
                catalog_item("PUMP2", "Water Heater", "the pump is sold separately"),
                catalog_item("FILTER", "Pump Station Filter", "filter for water pumping"),
                catalog_item("SPLIT", "Fresh water", "pump house"),
            ],
        )
        .unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let run = |text: &str, fuzzy_below: u64, filter: Option<Query>| {
        let mut request = SearchRequest::new(Query::MatchAll);
        request.filter = filter;
        let options = HybridOptions {
            text: text.into(),
            fuzzy_below,
            ..HybridOptions::default()
        };
        hybrid::search(backend, &mappings, &request, &options, None).unwrap()
    };
    let ids = |page: &moqui_core::search::SearchPage| page.hits.iter().map(|hit| hit.id.clone()).collect::<Vec<_>>();

    // A typo: no lexical match, so the fuzzy branch runs and finds it.
    let page = run("industral water pump", 5, None);
    assert_eq!(ids(&page), vec!["PUMP1"]);
    assert_eq!((page.total, page.total_relation), (1, TotalRelation::Eq));

    // A prefix as someone types: the title that starts with it first, then title words.
    let page = run("wat", 0, None);
    assert_eq!(ids(&page)[0], "PUMP2", "{:?}", ids(&page));
    let mut found = ids(&page);
    found.sort();
    assert_eq!(found, vec!["PUMP1", "PUMP2", "SPLIT"]);
    assert_eq!(page.total, 3);

    // The trusted filter applies to every branch and to the total.
    let page = run(
        "wat",
        0,
        Some(query(
            r#"{"field": {"name": "itemId", "value": "PUMP2", "negate": true}}"#,
        )),
    );
    assert!(!ids(&page).contains(&"PUMP2".to_string()), "{:?}", ids(&page));
    assert_eq!(page.total, 2);

    backend.drop_index(&mapping).unwrap();
}

/// Accent folding on Postgres: unaccented searches find accented text, in the text vector, the
/// fuzzy title, and the autocomplete title.
/// Dropping a table the caller's own open transaction has written to: the DDL runs on another
/// connection, so it gives up after its lock timeout with an error instead of waiting forever.
fn ddl_under_own_lock(tx: &TransactionManager, backend: &PostgresBackend) {
    let mapping = catalog_mapping();
    backend.ensure_index(&mapping).unwrap();
    tx.begin(None).unwrap();
    backend
        .index_documents(&mapping, &[catalog_item("HELD", "Held", "held by the caller")])
        .unwrap();
    let started = Instant::now();
    let err = backend.drop_index(&mapping).unwrap_err();
    assert!(err.to_string().contains("stayed locked"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(30));
    tx.rollback().unwrap();
    backend.drop_index(&mapping).unwrap();
}

fn accent(backend: PostgresBackend) {
    assert!(backend.capabilities().contains(&Capability::Accent));
    let mapping = catalog_mapping();
    backend.ensure_index(&mapping).unwrap();
    backend
        .index_documents(
            &mapping,
            &[catalog_item("CAFE", "Café Crème Brûlée", "dessert à la française")],
        )
        .unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let ids = |query: Query| -> Vec<String> {
        backend
            .search(&mappings, &SearchRequest::new(query))
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.id)
            .collect()
    };
    assert_eq!(ids(parse_query_string("creme").unwrap()), vec!["CAFE"]);
    assert_eq!(ids(parse_query_string("francaise").unwrap()), vec!["CAFE"]);
    assert_eq!(ids(query(r#"{"suggest": {"text": "cafe cr"}}"#)), vec!["CAFE"]);
    assert_eq!(ids(query(r#"{"fuzzy": {"terms": "cafe creme brulee"}}"#)), vec!["CAFE"]);
    backend.drop_index(&mapping).unwrap();
}

/// The data feed end to end, on a database loaded with framework seed.
#[test]
fn feed_indexes_and_deletes_through_search() {
    let _shared = crate::common::shared();
    use moqui_core::context::ExecutionContextFactory;

    let runtime = temp_dir("moqui-rt-search-feed");
    let cluster_dir = temp_dir("moqui-pg-search-feed");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    moqui_host::load_data(&runtime, &framework, &cluster_dir, &moqui_host::LoadFlags::default()).unwrap();

    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 30).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).unwrap();
    let facade = moqui_host::db::entity_facade::open_entity_facade(&framework, Arc::clone(&driver), pool()).unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());
    let store = moqui_host::search::attach_search(&factory, &facade, true).unwrap();
    assert!(!store.has_own_cluster());
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let services: Vec<PathBuf> = ["EntityServices.xml", "search/SearchServices.xml"]
            .iter()
            .map(|name| framework.join("service/org/moqui").join(name))
            .collect();
        let skipped = moqui_core::service::lint(&services).unwrap();
        assert!(skipped.is_empty(), "service scripts did not parse: {skipped:?}");
        feed_scenario(&facade);
    }
    drop(store);
    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

/// With an entity database that is not Postgres, search starts its own cluster under the runtime.
#[test]
fn a_non_postgres_entity_database_gets_a_dedicated_search_cluster() {
    let _shared = crate::common::shared();
    use moqui_core::context::ExecutionContextFactory;

    let runtime = temp_dir("moqui-rt-search-own");
    let cluster_dir = temp_dir("moqui-pg-search-own");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    std::fs::create_dir_all(&runtime).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 30).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).unwrap();
    let facade = moqui_host::db::entity_facade::open_entity_facade(&framework, Arc::clone(&driver), pool()).unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());
    // The entity side stands in for MySQL: false is what attach_search gets for one.
    let store = moqui_host::search::attach_search(&factory, &facade, false).unwrap();
    assert!(store.has_own_cluster());
    assert!(runtime.join("db/search/data/PG_VERSION").is_file());
    drop(store);
    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

fn create(facade: &moqui_core::entity::EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (name, text) in pairs {
        value.set(*name, Value::string(*text));
    }
    value.create(facade).unwrap();
}

fn pending(facade: &moqui_core::entity::EntityFacade) -> usize {
    facade
        .find("moqui.entity.feed.DataFeedPending")
        .unwrap()
        .count()
        .unwrap() as usize
}

fn wait_drained(facade: &moqui_core::entity::EntityFacade) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while pending(facade) > 0 {
        assert!(Instant::now() < deadline, "DataFeedPending did not drain");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn call(name: &str, params: &[(&str, Value)]) -> std::collections::BTreeMap<String, Value> {
    let params = params
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    moqui_core::service::sync(name, &params).unwrap_or_else(|err| panic!("{name}: {err}"))
}

fn search_ids(index: &str, query_string: &str) -> Vec<String> {
    let out = call(
        "org.moqui.search.SearchServices.search#DataDocuments",
        &[
            ("indexName", Value::string(index)),
            ("queryString", Value::string(query_string)),
        ],
    );
    let mut ids: Vec<String> = match out.get("documentList") {
        Some(Value::List(items)) => items.iter().map(|item| to_plain_string(map_at(item, "_id"))).collect(),
        other => panic!("documentList: {other:?}"),
    };
    ids.sort();
    ids
}

fn feed_scenario(facade: &moqui_core::entity::EntityFacade) {
    // Two documents on one feed: an enumeration type with its enumerations as a child list, and an
    // enumeration with its type's description flattened in.
    create(
        facade,
        "moqui.entity.document.DataDocument",
        &[
            ("dataDocumentId", "TestEnumType"),
            ("indexName", "test_enums"),
            ("primaryEntityName", "moqui.basic.EnumerationType"),
        ],
    );
    for (seq, path, alias, kind) in [
        ("01", "enumTypeId", "", ""),
        ("02", "description", "", ""),
        ("03", "enums:enumId", "", ""),
        ("04", "enums:description", "enumDescription", ""),
        ("05", "(enums ? enums.length : 0)", "enumCount", "long"),
    ] {
        let mut pairs = vec![
            ("dataDocumentId", "TestEnumType"),
            ("fieldSeqId", seq),
            ("fieldPath", path),
        ];
        if !alias.is_empty() {
            pairs.push(("fieldNameAlias", alias));
        }
        if !kind.is_empty() {
            pairs.push(("fieldType", kind));
        }
        create(facade, "moqui.entity.document.DataDocumentField", &pairs);
    }
    create(
        facade,
        "moqui.entity.document.DataDocument",
        &[
            ("dataDocumentId", "TestEnum"),
            ("indexName", "test_enums"),
            ("primaryEntityName", "moqui.basic.Enumeration"),
        ],
    );
    for (seq, path, alias) in [
        ("01", "enumId", ""),
        ("02", "enumTypeId", ""),
        ("03", "description", ""),
        ("04", "type:description", "typeDescription"),
    ] {
        let mut pairs = vec![("dataDocumentId", "TestEnum"), ("fieldSeqId", seq), ("fieldPath", path)];
        if !alias.is_empty() {
            pairs.push(("fieldNameAlias", alias));
        }
        create(facade, "moqui.entity.document.DataDocumentField", &pairs);
    }
    create(
        facade,
        "moqui.entity.feed.DataFeed",
        &[
            ("dataFeedId", "TestEnumFeed"),
            ("dataFeedTypeEnumId", "DTFDTP_RT_PUSH"),
            ("feedName", "Test Enums"),
        ],
    );
    create(
        facade,
        "moqui.entity.feed.DataFeedDocument",
        &[("dataFeedId", "TestEnumFeed"), ("dataDocumentId", "TestEnumType")],
    );
    create(
        facade,
        "moqui.entity.feed.DataFeedDocument",
        &[("dataFeedId", "TestEnumFeed"), ("dataDocumentId", "TestEnum")],
    );

    // One transaction: a type and two enumerations. The commit inserts pending rows; a worker
    // delivers them and deletes the rows.
    let entity = "moqui.basic.EnumerationType";
    facade.begin(entity, None).unwrap();
    create(
        facade,
        entity,
        &[("enumTypeId", "TestColor"), ("description", "Paint Colors")],
    );
    create(
        facade,
        "moqui.basic.Enumeration",
        &[
            ("enumId", "TcRed"),
            ("enumTypeId", "TestColor"),
            ("description", "Crimson Red"),
        ],
    );
    create(
        facade,
        "moqui.basic.Enumeration",
        &[
            ("enumId", "TcBlue"),
            ("enumTypeId", "TestColor"),
            ("description", "Navy Blue"),
        ],
    );
    facade.commit(entity).unwrap();
    wait_drained(facade);

    assert_eq!(search_ids("test_enums", "*:crim*"), vec!["TcRed", "TestColor"]);
    assert_eq!(search_ids("test_enums", "enums.enumId:TcBlue"), vec!["TestColor"]);
    let search = moqui_core::search::current().unwrap();
    let body = search.get_document("TestEnumType", "TestColor").unwrap().unwrap();
    assert_eq!(number(&body["enumCount"]), "2", "the expression field ran: {body:?}");
    assert_eq!(to_plain_string(&body["description"]), "Paint Colors");

    // A rolled-back write leaves no pending row.
    facade.begin(entity, None).unwrap();
    create(
        facade,
        "moqui.basic.Enumeration",
        &[("enumId", "TcGhost"), ("enumTypeId", "TestColor")],
    );
    facade.rollback(entity).unwrap();
    assert_eq!(pending(facade), 0);

    // Updating the type reindexes each enumeration through the type-one relationship.
    facade.begin(entity, None).unwrap();
    let mut color = facade.make_value(entity).unwrap();
    color.set("enumTypeId", Value::string("TestColor"));
    color.set("description", Value::string("Wall Paint"));
    color.update(facade).unwrap();
    facade.commit(entity).unwrap();
    wait_drained(facade);
    let red = search.get_document("TestEnum", "TcRed").unwrap().unwrap();
    assert_eq!(to_plain_string(&red["typeDescription"]), "Wall Paint");
    assert_eq!(
        search_ids("test_enums", "typeDescription:wall"),
        vec!["TcBlue", "TcRed"]
    );

    // Deleting an enumeration deletes its document and reindexes the type, whose child list shrinks.
    facade.begin(entity, None).unwrap();
    let mut blue = facade.make_value("moqui.basic.Enumeration").unwrap();
    blue.set("enumId", Value::string("TcBlue"));
    blue.set("enumTypeId", Value::string("TestColor"));
    blue.delete(facade).unwrap();
    facade.commit(entity).unwrap();
    wait_drained(facade);
    assert!(search.get_document("TestEnum", "TcBlue").unwrap().is_none());
    let body = search.get_document("TestEnumType", "TestColor").unwrap().unwrap();
    assert_eq!(number(&body["enumCount"]), "1");

    // search#DataDocuments page fields, highlights, and an invalid query string.
    let out = call(
        "org.moqui.search.SearchServices.search#DataDocuments",
        &[
            ("indexName", Value::string("test_enums")),
            ("documentType", Value::string("TestEnum")),
            ("queryString", Value::string("*:crim*")),
            ("highlightFields", Value::List(vec![Value::string("description")])),
            ("pageSize", Value::I32(10)),
        ],
    );
    assert_eq!(number(&out["documentListCount"]), "1");
    assert_eq!(number(&out["documentListPageRangeHigh"]), "1");
    let Value::List(documents) = &out["documentList"] else {
        panic!()
    };
    assert_eq!(to_plain_string(map_at(&documents[0], "_type")), "TestEnum");
    assert_eq!(to_plain_string(map_at(&documents[0], "_index")), "test_enum");
    let highlights = map_at(&documents[0], "highlights");
    assert_eq!(
        to_plain_string(map_at(highlights, "description")),
        "<em>Crimson</em> Red"
    );
    // A bare fuzzy term is a query (I-18); numbered fuzziness is still a query error, which finds nothing.
    assert_eq!(search_ids("test_enums", "crim~"), vec!["TcRed", "TestColor"]);
    let bad = call(
        "org.moqui.search.SearchServices.search#DataDocuments",
        &[
            ("indexName", Value::string("test_enums")),
            ("queryString", Value::string("crim~1")),
        ],
    );
    assert_eq!(number(&bad["documentListCount"]), "0");

    // Rebuilding the feed sends every document again.
    search.drop_index("TestEnum").unwrap();
    let out = call(
        "org.moqui.search.SearchServices.index#DataFeedDocuments",
        &[
            ("dataFeedId", Value::string("TestEnumFeed")),
            ("dataDocumentId", Value::string("TestEnum")),
        ],
    );
    // TestEnum's primary entity is every Enumeration, seed included.
    let enumerations = facade.find("moqui.basic.Enumeration").unwrap().count().unwrap();
    assert_eq!(number(&out["documentsIndexed"]), enumerations.to_string());
    assert!(search.get_document("TestEnum", "TcRed").unwrap().is_some());

    // Read-your-writes: a fresh search right after the commit waits for this context's pending
    // rows to be delivered, with no wait_drained.
    facade.begin(entity, None).unwrap();
    create(
        facade,
        "moqui.basic.Enumeration",
        &[
            ("enumId", "TcGreen"),
            ("enumTypeId", "TestColor"),
            ("description", "Emerald Green"),
        ],
    );
    facade.commit(entity).unwrap();
    let mut fresh = SearchRequest::new(parse_query_string("*:emerald*").unwrap());
    fresh.fresh_by = Some(Instant::now() + Duration::from_secs(10));
    let page = search
        .search(&moqui_core::search::Target::Index("test_enums".into()), &fresh)
        .unwrap();
    let mut ids: Vec<String> = page.hits.into_iter().map(|hit| hit.id).collect();
    ids.sort();
    assert_eq!(ids, vec!["TcGreen", "TestColor"]);

    // The document-type facet over an index with two types, with a trusted filter.
    let aggs = tree(r#"{"types": {"terms": {"field": "_document", "size": 10}}}"#);
    let filter = Query::all(vec![
        parse_query_string("*:emerald* OR *:crim*").unwrap(),
        query(
            r#"{"bool": {"none": [{"bool": {"all": [{"field": {"name": "_document", "value": "TestEnum"}}, {"field": {"name": "enumId", "value": "TcRed"}}]}}]}}"#,
        ),
    ]);
    let facets = search
        .aggregate(&moqui_core::search::Target::Index("test_enums".into()), &filter, &aggs)
        .unwrap();
    let counts: Vec<(String, String)> = buckets(&facets["types"])
        .iter()
        .map(|bucket| {
            (
                to_plain_string(map_at(bucket, "key")),
                number(map_at(bucket, "doc_count")),
            )
        })
        .collect();
    assert_eq!(
        counts,
        vec![
            ("TestEnum".to_string(), "1".to_string()),
            ("TestEnumType".to_string(), "1".to_string())
        ],
        "TcGreen and the type; TcRed is filtered out"
    );
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 8,
        reserve: 2,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
