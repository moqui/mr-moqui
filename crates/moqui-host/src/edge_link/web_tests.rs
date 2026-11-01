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

#[test]
fn hop_by_hop_and_forwarding_headers_are_not_passed_on() {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("connection", "keep-alive, X-Custom-Hop"),
        ("keep-alive", "timeout=5"),
        ("x-custom-hop", "1"),
        ("transfer-encoding", "chunked"),
        ("upgrade", "websocket"),
        ("te", "trailers"),
        ("x-forwarded-for", "198.51.100.1"),
        ("forwarded", "for=198.51.100.1"),
        ("x-real-ip", "198.51.100.1"),
        ("x-moqui-edge-client", "198.51.100.1:1"),
        ("x-moqui-edge-proto", "https"),
        ("cf-connecting-ip", "198.51.100.1"),
        ("cookie", "a=b"),
        ("accept", "text/html"),
    ] {
        headers.append(
            http::header::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    strip_hop_by_hop(&mut headers);
    strip_forwarding(&mut headers, "CF-Connecting-IP");
    let mut left: Vec<&str> = headers.keys().map(http::header::HeaderName::as_str).collect();
    left.sort_unstable();
    assert_eq!(left, ["accept", "cookie"]);
}

#[test]
fn a_websocket_upgrade_is_seen_and_answered_by_its_key() {
    assert_eq!(
        websocket_accept(b"dGhlIHNhbXBsZSBub25jZQ=="),
        "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
    );
    let upgrade = http::Request::get("/notws")
        .header("connection", "keep-alive, Upgrade")
        .header("upgrade", "WebSocket")
        .body(())
        .unwrap();
    assert!(is_websocket(&upgrade));
    let plain_get = http::Request::get("/notws").body(()).unwrap();
    assert!(!is_websocket(&plain_get));
    let post = http::Request::post("/notws")
        .header("connection", "upgrade")
        .header("upgrade", "websocket")
        .body(())
        .unwrap();
    assert!(!is_websocket(&post));
}

#[test]
fn the_offline_page_says_when_to_try_again() {
    let response = offline(DEFAULT_OFFLINE_PAGE);
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], RETRY_AFTER);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
}
