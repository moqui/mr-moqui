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

const ORIGIN: &str = "http://moqui-app.localhost";

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn a_page_reaches_only_paths_on_its_server() {
    assert_eq!(
        upstream_url("https://erp.example.com", "/apps/x?y=1").unwrap(),
        "https://erp.example.com/apps/x?y=1"
    );
    assert_eq!(
        upstream_url("https://erp.example.com/moqui", "/qapps").unwrap(),
        "https://erp.example.com/moqui/qapps"
    );
    for path in [
        "//evil.example.com/x",
        "https://evil.example.com/",
        "apps",
        "/a\\b",
        "/a\nb",
        "/x#y",
    ] {
        assert!(upstream_url("https://erp.example.com", path).is_err(), "{path:?}");
    }
}

#[test]
fn the_page_cannot_send_credentials_cookies_or_an_origin() {
    let sent = request_headers(&pairs(&[
        ("Accept", "application/json"),
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Range", "bytes=0-99"),
        ("X-Moqui-Page-Trace", "t1"),
        ("X-Moqui-Page-Path", "/apps/People/Time"),
        ("X-Moqui-Edge-Client", "1.2.3.4"),
        ("X-Moqui-Tunnel", "1"),
        ("Cookie", "JSESSIONID=1"),
        ("login_key", "stolen"),
        ("api_key", "stolen"),
        ("Authorization", "Basic x"),
        ("Origin", ORIGIN),
        ("Referer", ORIGIN),
    ]));
    let names: Vec<&str> = sent.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Accept",
            "Content-Type",
            "Range",
            "X-Moqui-Page-Trace",
            "X-Moqui-Page-Path"
        ]
    );
}

#[test]
fn the_answer_has_no_cookies_and_redirects_come_back_to_the_app() {
    let kept = response_headers(
        &pairs(&[
            ("Content-Type", "text/html"),
            ("Set-Cookie", "JSESSIONID=1"),
            ("Content-Length", "10"),
            ("Content-Encoding", "gzip"),
            ("Location", "https://erp.example.com/qapps/Home"),
            ("Content-Range", "bytes 0-99/200"),
            ("WWW-Authenticate", "Basic realm=x"),
            ("Access-Control-Allow-Origin", "*"),
        ]),
        "https://erp.example.com",
        ORIGIN,
    );
    assert_eq!(
        kept,
        pairs(&[
            ("Content-Type", "text/html"),
            ("Location", "http://moqui-app.localhost/qapps/Home"),
            ("Content-Range", "bytes 0-99/200"),
        ])
    );
}

#[test]
fn a_location_elsewhere_stays_and_a_server_path_loses_its_prefix() {
    let base = "https://erp.example.com/moqui";
    let to = |location: &str| rewrite_location(location, base, ORIGIN);
    assert_eq!(to("/moqui/qapps"), "http://moqui-app.localhost/qapps");
    assert_eq!(to("https://erp.example.com/moqui"), "http://moqui-app.localhost/");
    assert_eq!(
        to("https://erp.example.com/moqui?x=1"),
        "http://moqui-app.localhost/?x=1"
    );
    assert_eq!(
        to("https://erp.example.com/moqui/a#b"),
        "http://moqui-app.localhost/a#b"
    );
    // the same server named another way: upper case, the default port, http behind a TLS proxy
    assert_eq!(
        to("https://ERP.example.com:443/moqui/a"),
        "http://moqui-app.localhost/a"
    );
    assert_eq!(to("http://erp.example.com/moqui/a"), "http://moqui-app.localhost/a");
    // relative to the request
    assert_eq!(to("qapps"), "qapps");
    // a path of the server outside its base is the server's own url
    assert_eq!(to("/other"), "https://erp.example.com/other");
    assert_eq!(to("/moquix"), "https://erp.example.com/moquix");
    assert_eq!(
        rewrite_location("/Login", "https://erp.example.com", ORIGIN),
        "http://moqui-app.localhost/Login"
    );
    // a host that only starts with the server's is another host, as is another port
    assert_eq!(
        rewrite_location("https://erp.example.com.evil.net/x", "https://erp.example.com", ORIGIN),
        "https://erp.example.com.evil.net/x"
    );
    assert_eq!(
        to("https://erp.example.com:8443/moqui/a"),
        "https://erp.example.com:8443/moqui/a"
    );
    assert_eq!(
        to("https://accounts.example.org/auth"),
        "https://accounts.example.org/auth"
    );
    assert_eq!(to("//other.example.org/x"), "//other.example.org/x");
    assert_eq!(to("mailto:a@example.com"), "mailto:a@example.com");
}

#[test]
fn a_request_another_site_started_is_refused() {
    assert!(!cross_site(&pairs(&[]), ORIGIN));
    assert!(!cross_site(
        &pairs(&[("Origin", ORIGIN), ("Sec-Fetch-Site", "same-origin")]),
        ORIGIN
    ));
    assert!(!cross_site(&pairs(&[("Sec-Fetch-Site", "none")]), ORIGIN));
    assert!(cross_site(&pairs(&[("Origin", "https://evil.example.com")]), ORIGIN));
    assert!(cross_site(&pairs(&[("Origin", "null")]), ORIGIN));
    assert!(cross_site(&pairs(&[("Sec-Fetch-Site", "cross-site")]), ORIGIN));
    assert!(cross_site(&pairs(&[("Sec-Fetch-Site", "same-site")]), ORIGIN));
}

#[test]
fn a_cross_site_request_or_odd_method_never_leaves_the_host() {
    let proxy = Proxy::new().unwrap();
    // an unroutable server: an error here comes from the checks, before any connection
    let base = "https://192.0.2.1";
    let request = |method: &str, headers: &[(&str, &str)]| Request {
        method: method.to_string(),
        path: "/apps/x".to_string(),
        headers: pairs(headers),
        body: Vec::new(),
    };
    let refused = |request| match proxy.send(base, Some("k"), ORIGIN, request) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("forwarded"),
    };
    assert!(refused(request("POST", &[("Origin", "https://evil.example.com")])).contains("another site"));
    assert!(refused(request("TRACE", &[])).contains("not a method"));
    assert!(refused(request("CONNECT", &[])).contains("not a method"));
}

#[test]
fn an_attachment_is_named_without_a_directory() {
    assert_eq!(
        attachment_name("attachment; filename=\"report.pdf\"").as_deref(),
        Some("report.pdf")
    );
    assert_eq!(
        attachment_name("attachment; filename=x.txt; filename*=UTF-8''r%C3%A9sum%C3%A9.txt").as_deref(),
        Some("résumé.txt")
    );
    assert_eq!(
        attachment_name("attachment; filename=\"../../etc/passwd\"").as_deref(),
        Some("passwd")
    );
    assert_eq!(
        attachment_name("attachment; filename=\".hidden\"").as_deref(),
        Some("hidden")
    );
    assert_eq!(attachment_name("attachment").as_deref(), Some("download"));
    assert_eq!(
        attachment_name("attachment; filename=\"a:b*?.txt. \"").as_deref(),
        Some("a_b__.txt")
    );
    assert_eq!(
        attachment_name("attachment; filename=CON.txt").as_deref(),
        Some("_CON.txt")
    );
    assert_eq!(attachment_name("attachment; filename=com1").as_deref(), Some("_com1"));
    assert_eq!(attachment_name("inline; filename=a.pdf"), None);
}
