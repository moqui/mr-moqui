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
fn only_what_the_host_may_say_is_kept_from_a_page_s_headers() {
    for name in [
        "Accept",
        "Content-Type",
        "X-CSRF-Token",
        "Origin",
        "Range",
        "Sec-Fetch-Dest",
        "X-Moqui-Qvt",
    ] {
        assert!(passed_on(name), "{name}");
    }
    for name in [
        "Cookie",
        "Host",
        "Accept-Encoding",
        "X-Forwarded-For",
        "x-forwarded-proto",
        "Forwarded",
        "X-Moqui-Edge-Host",
        "X-Moqui-Tunnel",
        "login_key",
        "Content-Length",
    ] {
        assert!(!passed_on(name), "{name}");
    }
}

#[test]
fn a_page_gets_no_cookie_and_a_redirect_to_the_node_s_address_stays_on_the_scheme() {
    moqui_lib::env::set_property("webapp_http_port", "18432");
    let headers: Vec<(String, String)> = [
        ("Set-Cookie", "s=1; HttpOnly"),
        ("Content-Encoding", "gzip"),
        ("Location", "http://127.0.0.1:18432/qapps/system?x=1"),
        ("Content-Type", "text/html"),
    ]
    .iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect();
    let shown = page_headers(&headers);
    assert_eq!(header(&shown, "set-cookie"), None);
    assert_eq!(header(&shown, "content-encoding"), None);
    assert_eq!(
        header(&shown, "location"),
        Some("http://moqui-node.localhost/qapps/system?x=1")
    );
    assert_eq!(header(&shown, "content-type"), Some("text/html"));
    let elsewhere = vec![("Location".to_string(), "https://sso.example.com/auth".to_string())];
    assert_eq!(
        header(&page_headers(&elsewhere), "location"),
        Some("https://sso.example.com/auth")
    );
}

#[test]
fn only_a_get_navigation_may_come_from_another_site() {
    assert!(navigation("GET", true));
    assert!(navigation("HEAD", true));
    assert!(!navigation("POST", true), "a form from another site");
    assert!(!navigation("GET", false), "a fetch from another site");
}
