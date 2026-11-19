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

use std::collections::HashMap;

use base64::Engine;
use tauri::http::{Response, StatusCode};
use tauri::Url;

use moqui_client_tauri::scheme::SchemeHooks;

use super::{
    fetch_answer, fetch_request, page_site, redirect_page, refuse_fetch, FetchHooks, MobileHooks, MAIN_FRAME_HEADER,
    OWN_HEADER,
};
use moqui_client_tauri::APP_ORIGIN;
use tauri::http::HeaderMap;

fn redirect(location: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", location)
        .body(Vec::new())
        .unwrap()
}

#[test]
fn a_documents_redirect_is_a_page_that_goes_there_and_another_is_refused() {
    let page = redirect_page(true, redirect("/qapps/AppList?a=1&b=\"2\""));
    assert_eq!(page.status(), 200);
    let text = String::from_utf8(page.body().clone()).unwrap();
    assert!(
        text.contains("content=\"0;url=/qapps/AppList?a=1&amp;b=&quot;2&quot;\""),
        "{text}"
    );
    assert_eq!(redirect_page(false, redirect("/x")).status(), 502);
    let ok = Response::builder().status(200).body(b"x".to_vec()).unwrap();
    assert_eq!(redirect_page(true, ok).body(), b"x");
}

#[test]
fn a_pages_fetch_through_the_host_stays_on_its_origin() {
    let shell = Url::parse("http://moqui-shell.localhost/").unwrap();
    let headers = HashMap::from([("Content-Type".to_string(), "application/json".to_string())]);
    let body = base64::engine::general_purpose::STANDARD.encode(br#"{"a":1}"#);
    let request = fetch_request(&shell, "post", "/qapps/BasicComms/Chat/send", &headers, Some(&body)).unwrap();
    assert_eq!(request.method(), "POST");
    assert_eq!(request.uri().path(), "/qapps/BasicComms/Chat/send");
    assert_eq!(request.body(), br#"{"a":1}"#);
    assert_eq!(request.headers()["content-type"], "application/json");
    // another origin, a page that is not the app's, a body not base64
    assert!(fetch_request(&shell, "POST", "http://moqui-app.localhost/x", &headers, None).is_err());
    assert!(fetch_request(&shell, "POST", "https://evil.example/x", &headers, None).is_err());
    let web = Url::parse("https://evil.example/").unwrap();
    assert!(fetch_request(&web, "POST", "/x", &headers, None).is_err());
    assert!(fetch_request(&shell, "POST", "/x", &headers, Some("not base64!")).is_err());
    assert!(fetch_request(&shell, "NOT A METHOD", "/x", &headers, None).is_err());
    // a body over the limit is refused before it is decoded
    let huge = "A".repeat(super::BODY_MAX.div_ceil(3) * 4 + 4);
    let refused = fetch_request(&shell, "POST", "/x", &headers, Some(&huge)).unwrap_err();
    assert!(refused.contains("too large"), "{refused}");
}

#[test]
fn the_hosts_answer_to_a_fetch_carries_its_status_headers_and_body() {
    let response = Response::builder()
        .status(201)
        .header("X-Moqui-Offline", "1")
        .body(b"done".to_vec())
        .unwrap();
    let answer = fetch_answer(&response);
    assert_eq!(answer["status"], 201);
    assert_eq!(answer["headers"]["x-moqui-offline"], "1");
    assert_eq!(
        answer["body"],
        base64::engine::general_purpose::STANDARD.encode(b"done")
    );
}

/// SEC-CLI-06: where a page's request through the host comes from is the host's to say: the caller's `Origin`,
/// `Referer`, `Sec-Fetch-*` and `Service-Worker` are replaced by the page's own origin and a fetch's destination, and the
/// request is `same-origin` (the IPC takes the main frame only); the rest of its headers go on.
#[test]
fn a_pages_fetch_through_the_host_has_the_hosts_origin_and_fetch_headers_not_its_own() {
    let app = Url::parse("http://moqui-app.localhost/qapps/Orders").unwrap();
    let hostile = HashMap::from([
        ("Origin".to_string(), "https://evil.example".to_string()),
        ("referer".to_string(), "https://evil.example/page".to_string()),
        ("Sec-Fetch-Site".to_string(), "none".to_string()),
        ("sec-fetch-dest".to_string(), "document".to_string()),
        ("Sec-Fetch-Mode".to_string(), "navigate".to_string()),
        ("Service-Worker".to_string(), "script".to_string()),
        (
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        ),
    ]);
    let request = fetch_request(&app, "POST", "/apps/Orders/approve", &hostile, None).unwrap();
    let headers = request.headers();
    assert_eq!(
        headers.get_all("origin").iter().collect::<Vec<_>>(),
        ["http://moqui-app.localhost"]
    );
    assert_eq!(headers.get_all("sec-fetch-dest").iter().collect::<Vec<_>>(), ["empty"]);
    for gone in ["referer", "sec-fetch-site", "sec-fetch-mode", "service-worker"] {
        assert!(!headers.contains_key(gone), "{gone}: {:?}", headers.get(gone));
    }
    assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
    // the shell's own origin for the shell's request
    let shell = Url::parse("http://moqui-shell.localhost/").unwrap();
    let request = fetch_request(&shell, "POST", "/ChatKeys/storeKey", &hostile, None).unwrap();
    assert_eq!(request.headers()["origin"], "http://moqui-shell.localhost");
    assert_eq!(
        FetchHooks.fetch_site(request.headers(), "http://moqui-shell.localhost"),
        Some("same-origin")
    );
}

/// SEC-CLI-06: a fetch through the host on the shell's origin is answered only to the shell's current load (its page
/// and nonce): a server page still running as the host shows the shell has the shell's URL, not its nonce. A server
/// page's own fetch is its account's, as the scheme answers it.
#[test]
fn a_fetch_on_the_shells_origin_without_the_shells_nonce_is_refused() {
    assert!(refuse_fetch("http://moqui-shell.localhost", false).is_err());
    assert!(refuse_fetch("http://moqui-shell.localhost", true).is_ok());
    assert!(refuse_fetch("http://moqui-app.localhost", false).is_ok());
}

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, value.parse().unwrap());
    }
    map
}

/// SEC-CLI-102: the WebView tells the scheme nothing of where a request comes from, so the scheme takes as the page's
/// own only the main frame's document (wry's mark) and the bridge's marked fetches, or an `Origin` of its own; the rest,
/// what a frame of another origin in the page starts among it, is `cross-site`, and the server refuses a writing GET.
#[test]
fn only_the_main_frames_document_and_the_pages_own_marked_fetches_are_same_origin() {
    assert_eq!(
        page_site(&headers(&[(MAIN_FRAME_HEADER, "1")]), APP_ORIGIN),
        "same-origin"
    );
    assert_eq!(page_site(&headers(&[(OWN_HEADER, "1")]), APP_ORIGIN), "same-origin");
    assert_eq!(
        page_site(&headers(&[("origin", APP_ORIGIN)]), APP_ORIGIN),
        "same-origin"
    );
    // a nested frame or an image a frame of another origin starts: no mark, no Origin
    assert_eq!(
        page_site(&headers(&[("accept", "text/html")]), APP_ORIGIN),
        "cross-site"
    );
    assert_eq!(page_site(&HeaderMap::new(), APP_ORIGIN), "cross-site");
    // a CORS request of another origin says so, even with a mark it could only have sent after a preflight
    assert_eq!(
        page_site(
            &headers(&[("origin", "https://elsewhere.example"), (OWN_HEADER, "1")]),
            APP_ORIGIN
        ),
        "cross-site"
    );
    assert_eq!(page_site(&headers(&[("origin", "null")]), APP_ORIGIN), "cross-site");
    assert_eq!(
        MobileHooks.fetch_site(&headers(&[(MAIN_FRAME_HEADER, "1")]), APP_ORIGIN),
        Some("same-origin")
    );
    assert_eq!(
        MobileHooks.fetch_site(&HeaderMap::new(), APP_ORIGIN),
        Some("cross-site")
    );
}

/// The marks are set where the WebView or the page can tell, under the names the scheme reads: wry's binding sets
/// [`MAIN_FRAME_HEADER`] from `isForMainFrame` and drops a copy a request came with, and the bridge adds [`OWN_HEADER`]
/// to its own-origin GET and HEAD fetches and XHRs.
#[test]
fn the_main_frame_and_the_bridge_mark_the_pages_own_requests_under_the_names_the_scheme_reads() {
    let binding = include_str!("../patches/wry/src/android/binding.rs");
    assert!(
        binding.contains(&format!("const MAIN_FRAME_HEADER: &str = \"{MAIN_FRAME_HEADER}\";")),
        "wry's binding names the main frame's mark otherwise"
    );
    assert!(
        binding.contains("\"isForMainFrame\", \"()Z\""),
        "wry's binding does not ask isForMainFrame"
    );
    assert!(
        binding.contains("if header.as_str() == MAIN_FRAME_HEADER {\n        continue;"),
        "wry's binding keeps a main-frame mark the request came with"
    );
    assert!(
        binding.contains("if main_frame {\n    request_builder = request_builder.header(MAIN_FRAME_HEADER, \"1\");"),
        "wry's binding does not mark the main frame's document"
    );
    let bridge = include_str!("bridge.js");
    assert!(
        bridge.contains(&format!("const ownHeader = \"{OWN_HEADER}\";")),
        "the bridge names its mark otherwise"
    );
    assert!(
        bridge.contains("XMLHttpRequest.prototype.send"),
        "the bridge does not mark the page's XHRs"
    );
}
