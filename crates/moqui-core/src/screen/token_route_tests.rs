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

fn route(name: &str) -> TokenRoute {
    TokenRoute {
        name: name.to_string(),
        screen_location: format!("component://test/{name}.xml"),
        token_service: "test.check#Token".to_string(),
        title: None,
        post_transition: None,
    }
}

fn redaction_of<const N: usize>(routes: [TokenRoute; N]) -> Redaction {
    Redaction {
        routes: routes.to_vec(),
        paths: Vec::new(),
    }
}

/// SEC-PSF-03: a component's token path (PASS's held mail, held report and preview links) loses its token wherever the
/// server records a path or URL, at the root or under a mount as the pattern says; a shorter segment (an operation's
/// name, `held-mail/claim`) and a path the pattern does not name are kept.
#[test]
fn a_token_paths_token_is_redacted_from_a_recorded_path_or_url() {
    let patterns = ["/preview/{token}", "/**/held-mail/{token}", "/**/-/held-report/{token}"];
    let redaction = Redaction {
        routes: vec![route("i")],
        paths: patterns
            .iter()
            .map(|pattern| TokenPath::parse(pattern).unwrap())
            .collect(),
    };
    let url = |text: &str| redact_url(&redaction, text).into_owned();
    assert_eq!(url("/preview/Prev.iew~Token_01-x"), "/preview/:token");
    assert_eq!(
        url("/preview/PreviewToken01/asset/a.css?v=1"),
        "/preview/:token/asset/a.css?v=1"
    );
    assert_eq!(
        url("/shop/preview/PreviewToken01"),
        "/shop/preview/PreviewToken01",
        "/preview is at the root only"
    );
    assert_eq!(url("/held-mail/HeldMailToken01"), "/held-mail/:token");
    assert_eq!(
        url("/forum/held-mail/HeldMailToken01/claim"),
        "/forum/held-mail/:token/claim"
    );
    assert_eq!(
        url("https://pass.example.com/acme/-/forum/held-mail/HeldMailToken01#x"),
        "https://pass.example.com/acme/-/forum/held-mail/:token#x"
    );
    assert_eq!(
        url("/acme/tools/-/held-report/HeldReportToken01"),
        "/acme/tools/-/held-report/:token"
    );
    assert_eq!(
        url("/acme/held-report/HeldReportToken01"),
        "/acme/held-report/HeldReportToken01"
    );
    assert_eq!(url("/rest/s1/pass/held-mail/claim"), "/rest/s1/pass/held-mail/claim");
    assert_eq!(
        url("/i/abcDEF12345/held-mail/HeldMailToken01"),
        "/i/:token/held-mail/:token"
    );
    assert_eq!(url("/forum/held-mail"), "/forum/held-mail");
    let text = redact_text(
        &redaction,
        "GET /forum/held-mail/HeldMailToken01 from /preview/PreviewToken01",
    );
    assert_eq!(text, "GET /forum/held-mail/:token from /preview/:token");
    for bad in [
        "/preview/*",
        "preview/{token}",
        "/{token}",
        "/**/{token}",
        "/a/**/b/{token}",
        "/a//{token}",
    ] {
        assert_eq!(TokenPath::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_path_under_a_route_is_the_shell_the_screen_in_a_mode_or_a_path_below() {
    let routes = [route("s"), route("u")];
    let at = |path: &str| match_path(&routes, path).map(|found| (found.route.name, found.token, found.target));
    assert_eq!(
        at("/s/abcDEF12_-"),
        Some(("s".into(), "abcDEF12_-".into(), RouteTarget::Shell))
    );
    assert_eq!(
        at("/s/abcDEF12/"),
        Some(("s".into(), "abcDEF12".into(), RouteTarget::Shell))
    );
    assert_eq!(
        at("/s/abcDEF12.qvue"),
        Some(("s".into(), "abcDEF12".into(), RouteTarget::Screen("qvue".into())))
    );
    assert_eq!(
        at("/u/abcDEF12/list/rows"),
        Some(("u".into(), "abcDEF12".into(), RouteTarget::Below("/list/rows".into())))
    );
    assert_eq!(at("/s/short"), None, "a token is at least 8 characters");
    assert_eq!(at("/s/bad*token!"), None);
    assert_eq!(at("/s/abcDEF12.html"), None, "only the qvt render modes");
    assert_eq!(at("/call/abcDEF12"), None, "not a configured route");
    assert_eq!(at("/s"), None);
    assert_eq!(at("/qapps/s/abcDEF12"), None);
}

#[test]
fn a_routes_token_is_redacted_from_a_recorded_path_or_url() {
    let routes = redaction_of([route("i"), route("u"), route("call")]);
    let path = |text: &str| redact_path(&routes, text);
    assert_eq!(path("/i/abcDEF12_-xyz").as_deref(), Some("/i/:token"));
    assert_eq!(path("/i/abcDEF12_-xyz/").as_deref(), Some("/i/:token/"));
    assert_eq!(path("/u/abcDEF12345/oneClick").as_deref(), Some("/u/:token/oneClick"));
    assert_eq!(path("/call/abcDEF12345.qvt").as_deref(), Some("/call/:token.qvt"));
    assert_eq!(path("/i/short"), None, "no token under 8 characters");
    assert_eq!(path("/i/abcDEF12*x"), None);
    assert_eq!(path("/s/abcDEF12345"), None, "not a configured route");
    assert_eq!(path("/apps/i/abcDEF12345"), None, "a route is at the root only");
    assert_eq!(path("/i"), None);
    let url = |text: &str| redact_url(&routes, text).into_owned();
    assert_eq!(url("/i/abcDEF12345/accept?x=1"), "/i/:token/accept?x=1");
    assert_eq!(
        url("https://staff.example.com/i/abcDEF12345/accept"),
        "https://staff.example.com/i/:token/accept"
    );
    assert_eq!(
        url("http://localhost:8080/call/abcDEF12345"),
        "http://localhost:8080/call/:token"
    );
    assert_eq!(
        url("https://example.com/apps/System"),
        "https://example.com/apps/System"
    );
    assert_eq!(url("https://example.com"), "https://example.com");
    assert_eq!(url("//staff.example.com/i/abcDEF12345"), "//staff.example.com/i/:token");
    assert_eq!(url("plain text"), "plain text");
    assert_eq!(redact_url(&Redaction::default(), "/i/abcDEF12345"), "/i/abcDEF12345");
}

#[test]
fn a_routes_token_is_redacted_within_text_and_json() {
    let routes = redaction_of([route("i"), route("tk")]);
    let text = |text: &str| redact_text(&routes, text).into_owned();
    assert_eq!(text("GET /i/abcDEF12345/accept"), "GET /i/:token/accept");
    assert_eq!(
        text("from (https://h.example.com/tk/abcDEF12345?x=1) and [/i/abcDEF12345]"),
        "from (https://h.example.com/tk/:token?x=1) and [/i/:token]"
    );
    assert_eq!(
        text("GET /apps/i/abcDEF12345"),
        "GET /apps/i/abcDEF12345",
        "not under a route"
    );
    assert_eq!(text("caf\u{e9} /i/abcDEF12345\n"), "caf\u{e9} /i/:token\n");
    assert!(matches!(
        redact_text(&routes, "no path here"),
        std::borrow::Cow::Borrowed(_)
    ));
    let mut json = serde_json::json!({
        "name": "GET /tk/abcDEF12345.qvt",
        "attributes": [{"key": "url.full", "value": {"stringValue": "https://h/i/abcDEF12345/factor"}}],
        "events": [{"type": 2, "data": {"attributes": {"href": "/tk/abcDEF12345/files"}}}, 4, null]
    });
    redact_json(&routes, &mut json);
    let shown = json.to_string();
    assert!(!shown.contains("abcDEF12345"), "{shown}");
    assert_eq!(
        json["attributes"][0]["value"]["stringValue"],
        "https://h/i/:token/factor"
    );
    assert_eq!(json["events"][0]["data"]["attributes"]["href"], "/tk/:token/files");
}
