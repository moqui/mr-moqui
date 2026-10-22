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

//! The server on the default Postgres search backend: log events shipped to `moqui_logs` through the
//! SearchFacade and read through the log viewer (with its time-range query string), HTTP requests
//! shipped to `moqui_http_log`, and the `/search` proxy staying closed. This binary sets process
//! state through the logger, so it has one test.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

mod common;

// the example component loads with the repository runtime; link its tool factory as the moqui binary does
use example as _;

use std::time::{Duration, Instant};

use common::{basic, exchange, http_request, start_with_data};

#[test]
fn postgres_backend_ships_logs_through_the_search_facade() {
    moqui_host::logging::install();
    let marker = format!("pglogmarker{}", std::process::id());
    // Before the server exists: the queue holds it until search attaches.
    tracing::warn!("early boot line {marker}");
    let gate = start_with_data("search-logs-postgres", "<entity-facade-xml type=\"seed\"/>");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let facade = moqui_core::search::installed(gate.server.factory()).expect("search is installed");
    assert_eq!(facade.backend().name(), "postgres");
    assert!(facade.backend().keeps_log_indexes());

    tracing::warn!("postgres log shipping {marker}");
    moqui_host::logging::flush(&facade);
    let deadline = Instant::now() + Duration::from_secs(20);
    let page = loop {
        let path = format!("/System/LogViewer.html?queryString={marker}");
        let (status, _, page) = exchange(address, &http_request("GET", &path, &[("Authorization", &john)], ""));
        assert_eq!(status, 200);
        if page.contains(&format!("postgres log shipping {marker}")) || Instant::now() > deadline {
            break page;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(page.contains(&format!("postgres log shipping {marker}")), "{page}");
    assert!(page.contains(&format!("early boot line {marker}")), "{page}");
    assert!(!page.contains("has no log index"), "{page}");

    // The HTTP request log.
    let deadline = Instant::now() + Duration::from_secs(20);
    let total = loop {
        moqui_host::logging::flush(&facade);
        let page = facade
            .search(
                &moqui_core::search::Target::Index("moqui_http_log".to_string()),
                &moqui_core::search::SearchRequest::new(moqui_core::search::query::Query::MatchAll),
            )
            .expect("http log search");
        if page.total > 0 || Instant::now() > deadline {
            break page.total;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(total > 0, "no moqui_http_log rows");

    // Retention, as the clean_ElasticSearchLogMessages jobs run it: delete entries older than a bound on @timestamp.
    let target = moqui_core::search::Target::Index("moqui_logs".to_string());
    let cutoff = moqui_lib::object::Value::I64(moqui_host::logging::now_millis() + 60_000);
    let removed = facade
        .delete_matching(
            &target,
            &moqui_core::search::query::Query::range("@timestamp", None, Some(cutoff)),
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
