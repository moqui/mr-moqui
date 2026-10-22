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

//! The server on the OpenSearch search backend: the `/search` proxy behind its login and
//! permission, log events shipped to `moqui_logs` and read through the log viewer, and HTTP
//! requests shipped to `moqui_http_log`. Ignored unless `OPENSEARCH_URL` names a running node, such
//! as the install in `runtime/opensearch` (`bin/opensearch -d -p pid`). This binary sets process
//! properties, so it has one test.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::time::{Duration, Instant};

use crate::common::{basic, exchange, http_request, start_with_data};

#[test]
#[ignore = "needs OpenSearch: OPENSEARCH_URL=http://127.0.0.1:9200 cargo test -p moqui-host --test it -- search_opensearch --ignored"]
fn opensearch_backend_proxies_and_ships_logs() {
    let url = std::env::var("OPENSEARCH_URL").expect("set OPENSEARCH_URL to run this test");
    let prefix = format!("moqui_server_test_{}_", std::process::id());
    for (name, value) in [
        ("search_backend", "opensearch"),
        ("elasticsearch_url", url.as_str()),
        ("elasticsearch_index_prefix", prefix.as_str()),
        ("opensearch_run", "false"),
    ] {
        moqui_lib::env::set_property(name, value.to_string());
    }
    moqui_host::logging::install();
    let gate = start_with_data("search-opensearch", "<entity-facade-xml type=\"seed\"/>");
    let address = gate.server.address();
    let john = basic("john.doe", "moqui");
    let facade = moqui_core::search::installed(gate.server.factory()).expect("search is installed");
    assert_eq!(facade.backend().name(), "opensearch");

    // The proxy: a login and SearchRemote (AuthFilter.js), then ProxyServlet.js, then the cluster's own answer.
    let (status, headers, _) = exchange(address, &http_request("GET", "/search/", &[], ""));
    assert_eq!(status, 401);
    assert!(crate::common::header(&headers, "www-authenticate").is_some_and(|value| value.starts_with("Basic")));
    let (status, _, body) = exchange(
        address,
        &http_request("GET", "/search/", &[("Authorization", &john)], ""),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("cluster_name"), "{body}");
    // kibana_host defaults to this host; with no Dashboards there the proxy answers 502.
    let (status, _, _) = exchange(address, &http_request("GET", "/dashboard/", &[], ""));
    assert_eq!(status, 401, "the Dashboards proxy needs a login too");

    // A log event and the requests above reach their indexes; the log viewer reads moqui_logs.
    let marker = format!("searchlogmarker{}", std::process::id());
    tracing::warn!("opensearch log shipping {marker}");
    moqui_host::logging::flush(&facade);
    let deadline = Instant::now() + Duration::from_secs(20);
    let page = loop {
        let path = format!("/System/LogViewer.html?queryString={marker}");
        let (status, _, page) = exchange(address, &http_request("GET", &path, &[("Authorization", &john)], ""));
        assert_eq!(status, 200);
        if page.contains(&format!("opensearch log shipping {marker}")) || Instant::now() > deadline {
            break page;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(page.contains(&format!("opensearch log shipping {marker}")), "{page}");
    assert!(!page.contains("has no log index"), "{page}");

    moqui_host::logging::flush(&facade);
    let search = format!("/search/{prefix}moqui_http_log/_search?q=request_path:LogViewer.html");
    let deadline = Instant::now() + Duration::from_secs(20);
    let body = loop {
        let (_, _, body) = exchange(address, &http_request("GET", &search, &[("Authorization", &john)], ""));
        if body.contains("\"request_path\"") || Instant::now() > deadline {
            break body;
        }
        std::thread::sleep(Duration::from_millis(500));
    };
    assert!(body.contains("\"request_path\""), "{body}");

    let cleanup = format!("/search/{prefix}*");
    exchange(
        address,
        &http_request("DELETE", &cleanup, &[("Authorization", &john)], ""),
    );
    drop(gate);
}
