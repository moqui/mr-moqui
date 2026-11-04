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

use super::{fetch_answer, fetch_request, redirect_page};

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
