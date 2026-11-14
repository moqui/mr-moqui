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

//! Root screens by host name on a served instance (plans/Edge-Node2.md §4.2, E14): a wrapper root, `SitesRoot.xml`,
//! for `site.localhost` with `framework-routes="include"` (the shell, sign-in and the apps beside its own page) and for
//! `public.localhost` with `framework-routes="refuse"` (its own page and the shared static files only), beside
//! `webroot.xml` on every other host.

use std::net::SocketAddr;

use crate::common::{cookies_of, exchange, header, merge_cookies, start_with_conf_files};

/// The wrapper root: a home page of its own, a transition that answers the link a request makes, and a file beside it.
const ROOT: &str = r#"<screen require-authentication="false">
    <transition name="rootUrl" read-only="true"><actions><script>ec.web.sendTextResponse(ec.web.getWebappRootUrl(true, null))</script></actions>
        <default-response type="none"/></transition>
    <subscreens default-item="Home"/>
    <widgets><subscreens-active/></widgets>
</screen>"#;
const HOME: &str =
    r#"<screen require-authentication="false"><widgets><label text="Sites home page" type="h1"/></widgets></screen>"#;

const CONF: &str = r#"<webapp-list><webapp name="webroot">
    <root-screen host="site\.localhost" location="component://sites-test/screen/SitesRoot.xml" framework-routes="include"/>
    <root-screen host="public\.localhost|git\.localhost" location="component://sites-test/screen/SitesRoot.xml" framework-routes="refuse"/>
    <public-site host="git.localhost" site-id="SITES_GIT" screen-location="component://sites-test/screen/SitesRoot.xml">
        <mount path="/pages" screen="Home"/></public-site>
    <token-route name="stk" screen-location="component://sites-test/screen/SitesRoot.xml"
        token-service="org.moqui.resource.FileServices.check#PublicShareToken"/>
</webapp></webapp-list>"#;

fn files() -> [(&'static str, &'static str); 4] {
    [
        (
            "components/addon/sites-test/component.xml",
            r#"<component name="sites-test"/>"#,
        ),
        ("components/addon/sites-test/screen/SitesRoot.xml", ROOT),
        ("components/addon/sites-test/screen/SitesRoot/Home.xml", HOME),
        (
            "components/addon/sites-test/screen/SitesRoot/site.css",
            "h1 { color: teal; }",
        ),
    ]
}

/// A request to `host` (with the server's port) with `headers`.
fn on(address: SocketAddr, host: &str, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Answer {
    let mut out = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{}\r\nConnection: close\r\n",
        address.port()
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() || method == "POST" {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    out.push_str(body);
    exchange(address, &out)
}

type Answer = (u16, Vec<(String, String)>, String);

fn get(address: SocketAddr, host: &str, path: &str) -> Answer {
    on(address, host, "GET", path, &[("Accept", "text/html")], "")
}

/// Sign in through `host`'s login form, as a browser on that host does; the session cookie.
fn sign_in_on(address: SocketAddr, host: &str) -> String {
    let (_, headers, _) = get(address, host, "/Login");
    let cookie = cookies_of(&headers);
    let (status, headers, text) = on(
        address,
        host,
        "POST",
        "/Login/login",
        &[
            ("Cookie", &cookie),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        "username=john.doe&password=moqui",
    );
    assert_eq!(status, 302, "{host}: {text}");
    for (name, value) in &headers {
        if name.eq_ignore_ascii_case("set-cookie") {
            assert!(
                !value.to_ascii_lowercase().contains("domain="),
                "a host-only cookie: {value}"
            );
        }
    }
    merge_cookies(&cookie, &headers)
}

/// The shared static files, the framework routes of each mode, the error screens, links and origins per host, and the
/// session cookie tied to its host (checks 2 to 7 of §4.2).
#[test]
fn a_wrapper_root_serves_its_host_by_its_framework_routes() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_conf_files("root-hosts", None, CONF, &[("webapp_handle_cors", "true")], &files());
    let address = gate.server.address();
    let port = address.port();

    // its own page at its root, and its own file
    for host in ["site.localhost", "public.localhost"] {
        let (status, _, body) = get(address, host, "/");
        assert_eq!(status, 200, "{host}: {body}");
        assert!(body.contains("Sites home page"), "{host}: {body}");
        let (status, _, body) = get(address, host, "/site.css");
        assert_eq!((status, body.as_str()), (200, "h1 { color: teal; }"), "{host}");
        // the shared static files
        let (status, _, body) = get(address, host, "/js/MoquiLib.js");
        assert_eq!(status, 200, "{host}: {body}");
    }

    // include: the framework's screens beside the wrapper's; refuse: none of the framework's routes
    let (status, _, body) = get(address, "site.localhost", "/Login");
    assert_eq!(status, 200, "{body}");
    for path in [
        "/Login",
        "/qapps",
        "/apps/system",
        "/rest/s1/moqui/basic/geos",
        "/status",
        "/sw.js",
        "/manifest.webmanifest",
        "/stk/AnyToken0123456789",
        "/health",
        "/notws",
        "/dav/",
    ] {
        let (status, _, body) = get(address, "public.localhost", path);
        assert_eq!(status, 404, "public.localhost{path}: {body}");
        assert!(!body.contains("expired"), "{path}: {body}");
    }
    let (status, _, body) = get(address, "localhost", "/stk/AnyToken0123456789");
    assert!(
        body.contains("expired"),
        "the token route answers on another host: {status} {body}"
    );
    let (status, headers, _) = get(address, "site.localhost", "/sw.js");
    assert_eq!(status, 200);
    assert_eq!(header(&headers, "Service-Worker-Allowed").as_deref(), Some("/"));
    let (status, _, body) = get(address, "site.localhost", "/manifest.webmanifest");
    assert!(status == 200 && body.contains("start_url"), "{status} {body}");

    // a refusing host that is a public site's: git at a page's URL reaches the git server (401 with no credentials),
    // a request to /git/ itself does not
    #[cfg(feature = "git")]
    {
        let refs = "/moqui/mr-moqui/info/refs?service=git-upload-pack";
        let (status, _, body) = get(address, "git.localhost", refs);
        assert_eq!(status, 401, "git at a page's URL: {body}");
        let (status, _, body) = get(address, "git.localhost", &format!("/git{refs}"));
        assert_eq!(status, 404, "/git/ on a refusing host: {body}");
        let (status, _, body) = get(address, "localhost", &format!("/git{refs}"));
        assert_eq!(status, 401, "/git/ on another host: {body}");
    }

    // a page the wrapper has not: the default root's error screen, on both
    for host in ["site.localhost", "public.localhost"] {
        let (status, headers, body) = get(address, host, "/NoSuchPage");
        assert_eq!(status, 404, "{host}: {body}");
        assert!(
            header(&headers, "Content-Type").is_some_and(|kind| kind.starts_with("text/html")),
            "{host}"
        );
        assert!(body.contains("Page Not Found"), "{host}: {body}");
    }

    // a full link made in a request keeps its host, a configured one; a refusing host's links are the conf's, where the
    // apps are
    let (status, _, body) = get(address, "site.localhost", "/rootUrl");
    assert_eq!(status, 200, "{body}");
    assert!(body.starts_with("http://site.localhost"), "{body}");
    let (status, _, body) = get(address, "public.localhost", "/rootUrl");
    assert_eq!(status, 200, "{body}");
    assert!(!body.contains("public.localhost"), "{body}");

    // each origin is its own: a page of site.localhost may post to it, one of public.localhost may not
    let post = |origin: &str| {
        on(
            address,
            "site.localhost",
            "POST",
            "/rootUrl",
            &[
                ("Origin", origin),
                ("Content-Type", "application/x-www-form-urlencoded"),
            ],
            "",
        )
        .0
    };
    assert_ne!(post(&format!("http://site.localhost:{port}")), 401);
    assert_eq!(post(&format!("http://public.localhost:{port}")), 401);

    // sign-in on the include host: the session cookie is the host's alone, and the shell is the default root's
    let cookie = sign_in_on(address, "site.localhost");
    let (status, _, body) = on(
        address,
        "site.localhost",
        "GET",
        "/shellData",
        &[("Cookie", &cookie), ("Accept", "application/json")],
        "",
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"apps\""), "{body}");
}

/// §5 E14's acceptance: two host names, two root screens. On each the shell, the static files, sign-in, the error
/// screens and the links are right, and each host has its own session.
#[test]
#[ignore = "acceptance: two hosts with two root screens, each with its shell, files, sign-in, errors, links and session"]
fn two_hosts_with_two_root_screens() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start_with_conf_files("root-hosts-accept", None, CONF, &[], &files());
    let address = gate.server.address();
    let mut sessions = Vec::new();
    for (host, home) in [("localhost", None), ("site.localhost", Some("Sites home page"))] {
        // the root page: the wrapper's own, or webroot.xml's apps
        let (status, _, body) = get(address, host, "/");
        assert!(status == 200 || status == 302, "{host}: {status} {body}");
        if let Some(home) = home {
            assert!(body.contains(home), "{host}: {body}");
        }
        // the static files
        for path in ["/js/MoquiLib.js", "/css/moqui-html.css", "/favicon.ico"] {
            let (status, _, _) = get(address, host, path);
            assert_eq!(status, 200, "{host}{path}");
        }
        // sign-in, on the host's own session
        let cookie = sign_in_on(address, host);
        let (status, _, body) = on(
            address,
            host,
            "GET",
            "/qapps",
            &[("Cookie", &cookie), ("Accept", "text/html")],
            "",
        );
        assert_eq!(status, 200, "{host}: {body}");
        let (status, _, body) = on(
            address,
            host,
            "GET",
            "/shellData",
            &[("Cookie", &cookie), ("Accept", "application/json")],
            "",
        );
        assert_eq!(status, 200, "{host}: {body}");
        assert!(body.contains("\"apps\""), "{host}: {body}");
        // an error page
        let (status, _, body) = on(
            address,
            host,
            "GET",
            "/NoSuchPage",
            &[("Cookie", &cookie), ("Accept", "text/html")],
            "",
        );
        assert_eq!(status, 404, "{host}");
        assert!(body.contains("Page Not Found"), "{host}: {body}");
        sessions.push(cookie);
    }
    // a link made on the wrapper's host is its own
    let (_, _, body) = get(address, "site.localhost", "/rootUrl");
    assert!(body.starts_with("http://site.localhost"), "{body}");
    // two sign-ins, two sessions
    let session = |cookie: &str| {
        cookie
            .split("; ")
            .find(|pair| pair.starts_with("moqui.session="))
            .map(str::to_string)
    };
    assert!(session(&sessions[0]).is_some());
    assert_ne!(session(&sessions[0]), session(&sessions[1]));
}
