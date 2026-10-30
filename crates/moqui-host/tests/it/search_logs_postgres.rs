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

//! The default Postgres backend: SDK log events read through authorized Explore, retained
//! HTTP root spans, telemetry retention, and the Elastic proxy staying closed. The test runs in a process of its own
//! (`common::own_process`), which owns the process logger.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use crate::common;

use std::time::{Duration, Instant};

use common::{basic, exchange, http_request, start_with_data};

#[test]
fn postgres_backend_exports_telemetry_through_the_search_facade() {
    if !crate::common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let marker = format!("pglogmarker{}", std::process::id());
    // Before the server exists: the queue holds it until search attaches.
    tracing::warn!("early boot line {marker}");
    let gate = start_with_data("search-logs-postgres", "<entity-facade-xml type=\"seed\"/>");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let facade = moqui_core::search::installed(gate.server.factory()).expect("search is installed");
    assert_eq!(facade.backend().name(), "postgres");

    tracing::warn!("postgres log shipping {marker}");
    moqui_core::telemetry::for_factory(gate.server.factory()).flush();
    let deadline = Instant::now() + Duration::from_secs(20);
    let page = loop {
        let path =
            format!("/apps/dashboards/Explore.html?datasetId=otel_logs&queryString={marker}&from=now-1h&thru=now");
        let (status, _, page) = exchange(address, &http_request("GET", &path, &[("Authorization", &john)], ""));
        assert_eq!(status, 200);
        if page.contains(&format!("postgres log shipping {marker}")) || Instant::now() > deadline {
            break page;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(page.contains(&format!("postgres log shipping {marker}")), "{page}");
    assert!(page.contains(&format!("early boot line {marker}")), "{page}");
    assert!(!page.contains("Error rendering"), "{page}");

    // Every HTTP request retains a SERVER root, including requests below the sampling threshold.
    let deadline = Instant::now() + Duration::from_secs(20);
    let total = loop {
        moqui_core::telemetry::for_factory(gate.server.factory()).flush();
        let page = facade
            .search(
                &moqui_core::search::Target::Index("otel_spans".to_string()),
                &moqui_core::search::SearchRequest::new(moqui_core::search::query::Query::eq(
                    "kind",
                    moqui_lib::object::Value::string("SERVER"),
                )),
            )
            .expect("request span search");
        if page.total > 0 || Instant::now() > deadline {
            break page.total;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(total > 0, "no retained HTTP root spans");

    // The same public query tree covers arbitrary attributes, native fuzzy log text, and weighted groups.
    use moqui_core::entity::document::Document;
    use moqui_core::search::{SearchRequest, Target};
    use moqui_lib::object::Value;
    let record = |id: &str, body: &str, attributes: serde_json::Value| {
        Document::from_iter([
            ("_id".into(), Value::string(id)),
            ("timestamp".into(), Value::I64(jiff::Timestamp::now().as_millisecond())),
            ("body".into(), Value::string(body)),
            ("attributes".into(), moqui_lib::json::from_json(&attributes)),
        ])
    };
    facade
        .index_entries(
            "otel_logs",
            vec![
                record(
                    "phase3-a",
                    "network timeout",
                    serde_json::json!({"tenant.id":"one", "tags":["blue","green"]}),
                ),
                record(
                    "phase3-b",
                    "storage failure",
                    serde_json::json!({"tenant.id":"two", "tags":["one","red"]}),
                ),
            ],
        )
        .unwrap();
    let query = |text: &str| {
        facade
            .search(
                &Target::Index("otel_logs".into()),
                &SearchRequest::new(moqui_core::search::parse::parse_query_string(text).unwrap()),
            )
            .unwrap()
    };
    let found = query("attributes.tenant.id:one AND attributes.tags:bl*");
    assert_eq!(found.total, 1);
    assert_eq!(found.hits[0].id, "phase3-a");
    assert!(!found.hits[0].body.contains_key("__attributes"));
    assert_eq!(
        query("attributes.tenant.id:red").total,
        0,
        "keys and values must correlate"
    );
    assert!(query("body:timeout~").hits.iter().any(|hit| hit.id == "phase3-a"));
    let ranked = query("attributes.tenant.id:* AND (body:timeout OR (body:failure)^10)");
    assert_eq!(ranked.hits[0].id, "phase3-b");
    let mut sorted = SearchRequest::new(
        moqui_core::search::parse::parse_query_string("attributes.tenant.id:* AND (body:timeout OR (body:failure)^10)")
            .unwrap(),
    );
    sorted.sort = vec![moqui_core::search::SortField::parse("body").unwrap()];
    let sorted = facade.search(&Target::Index("otel_logs".into()), &sorted).unwrap();
    assert_eq!(
        sorted.hits[0].id, "phase3-a",
        "explicit sorts take precedence over boosts"
    );

    let mut bad = record("phase3-poison", "bad counter", serde_json::json!({}));
    bad.insert("severity_number".into(), Value::string("999999999999999999999"));
    assert_eq!(
        facade
            .index_entries_deferred_refresh(
                "otel_logs",
                vec![
                    record("phase3-before", "independent before", serde_json::json!({})),
                    bad,
                    record("phase3-after", "independent after", serde_json::json!({})),
                ]
            )
            .unwrap(),
        2
    );
    assert!(facade.get_document("otel_logs", "phase3-before").unwrap().is_some());
    assert!(facade.get_document("otel_logs", "phase3-after").unwrap().is_some());
    assert!(facade.get_document("otel_logs", "phase3-poison").unwrap().is_none());

    // Retention deletes telemetry logs by their normalized timestamp.
    let target = moqui_core::search::Target::Index("otel_logs".to_string());
    let cutoff = moqui_lib::object::Value::I64(jiff::Timestamp::now().as_millisecond() + 60_000);
    let removed = facade
        .delete_matching(
            &target,
            &moqui_core::search::query::Query::range("timestamp", None, Some(cutoff)),
        )
        .expect("retention delete");
    assert!(removed > 0, "retention deleted nothing");

    // The proxy to an Elastic server stays closed: this backend has none.
    let (status, _, _) = exchange(
        address,
        &http_request("GET", "/search/", &[("Authorization", &john)], ""),
    );
    assert_eq!(status, 404);
    drop(gate);
}
