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
    let routes = [route("i"), route("u"), route("call")];
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
    assert_eq!(redact_url(&[], "/i/abcDEF12345"), "/i/abcDEF12345");
}

#[test]
fn a_routes_token_is_redacted_within_text_and_json() {
    let routes = [route("i"), route("tk")];
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
