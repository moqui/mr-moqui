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

//! Rendering: action order, authentication and authorization, `makeRender` with no socket, the
//! definition cache, and transition responses.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use moqui_lib::object::Value;

use super::{fixture_root, request, World};
use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::screen::{make_render, serve_at, RenderOutput, ScreenFacade, ScreenUrlInfo};

/// Render `path` in text mode unless the path names a mode. The screens render the screens below
/// them, and html has no macro module until a later slice.
fn render(factory: &std::sync::Arc<ExecutionContextFactory>, path: &str) -> Result<RenderOutput, Error> {
    let named = path.rsplit('/').next().unwrap_or("").contains('.');
    let render = make_render(factory).root_screen(fixture_root()).screen_path(path);
    if named {
        render.render()
    } else {
        render.render_mode("text").render()
    }
}

fn trace_of(output: &RenderOutput) -> Vec<String> {
    let text = output.body_text();
    match crate::service::parse_json(&text).unwrap_or_else(|err| panic!("{text}: {err}")) {
        Value::List(items) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        other => panic!("not a list: {other:?}"),
    }
}

fn location_of(output: &RenderOutput) -> Option<&str> {
    output.header("Location")
}
fn parameters(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect()
}

#[test]
fn always_actions_run_before_pre_actions_before_actions_down_the_path() {
    let context = ExecutionContextFactory::test_context();
    let output = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/Sub/Deep")
        .render_mode("text")
        .parameters(parameters(&[("needed", "yes")]))
        .render()
        .unwrap();
    assert_eq!(output.status, 200);
    assert_eq!(
        trace_of(&output),
        [
            "Root.always",
            "Sub.always",
            "Deep.always",
            "Leaf.always",
            "Root.pre",
            "Sub.pre",
            "Deep.pre",
            "Leaf.pre",
            "Root.actions",
            "Sub.actions",
            "Deep.actions",
            "Leaf.actions",
        ]
    );
}

#[test]
fn a_screen_with_a_missing_required_parameter_is_skipped_and_reports_an_error() {
    let context = ExecutionContextFactory::test_context();
    // Sub needs `needed`. Without it Sub does not render, and the error is shown where the widgets
    // would be, as Java does for any section that finds an error.
    let output = render(context.factory(), "/Sub/Deep").unwrap();
    assert_eq!(output.body_text(), "Required parameter missing (needed)");
    // With the parameter the whole path renders: Deep's actions run.
    let with = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/Sub/Deep")
        .render_mode("text")
        .parameters(parameters(&[("needed", "yes")]))
        .render()
        .unwrap();
    assert!(trace_of(&with).contains(&"Deep.actions".to_string()));
}

/// The first screen rendered (a level loaded alone) with a required parameter missing runs none of its actions: the
/// message alone shows, not also what its actions would say with no value (Tools AutoEdit with no aen).
#[test]
fn a_stopped_first_screen_runs_none_of_its_actions() {
    let dir = std::env::temp_dir().join(format!("moqui-render-stopped-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Needs.xml"),
        r#"<screen require-authentication="false"><parameter name="needId" required="true"/>
            <actions><script>ec.message.addError('Needs actions ran')</script></actions>
            <widgets><label text="needs page"/></widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    let response = serve_at(
        &world.factory,
        with_query(request("GET", "/Needs.text"), "lastStandalone", "true"),
        &root,
    );
    let body = response.body_text();
    assert!(body.contains("Required parameter missing (needId)"), "{body}");
    assert!(
        !body.contains("Needs actions ran") && !body.contains("needs page"),
        "{body}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_always_standalone_mode_starts_pre_actions_at_the_third_screen() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/Sub/Deep/Leaf.csv").unwrap();
    assert_eq!(output.status, 200);
    // Pre-actions skip Root and Sub, and only the target's own actions run.
    assert_eq!(
        trace_of(&output),
        [
            "Root.always",
            "Sub.always",
            "Deep.always",
            "Leaf.always",
            "Deep.pre",
            "Leaf.pre",
            "Leaf.actions",
        ]
    );
    // The same path as text runs every screen's pre-actions.
    let text = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/Sub/Deep/Leaf.text")
        .parameters(parameters(&[("needed", "yes")]))
        .render()
        .unwrap();
    assert!(trace_of(&text).contains(&"Root.pre".to_string()));
}

#[test]
fn a_transition_runs_always_actions_but_not_pre_actions_or_screen_actions() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/byId/7").unwrap();
    assert_eq!(output.status, 200);
    assert!(output.body_text().contains("\"id\" : \"7\""), "{}", output.body_text());
    assert!(output.body_text().contains("Root.always"), "{}", output.body_text());
    assert!(!output.body_text().contains("Root.pre"), "{}", output.body_text());
    assert!(!output.body_text().contains("Root.actions"), "{}", output.body_text());
}

#[test]
fn make_render_without_a_socket_sends_status_content_type_and_an_empty_body() {
    let context = ExecutionContextFactory::test_context();
    // A screen with empty widgets has no output of its own. The modes with a macro module write their
    // wrapper, if they have one, and the others write nothing.
    let picker = format!(
        "{}/src/screen/testdata/Elsewhere/PickerA.xml",
        env!("CARGO_MANIFEST_DIR")
    );
    let as_mode = |mode: &str| {
        make_render(context.factory())
            .root_screen(picker.clone())
            .render_mode(mode)
            .render()
            .unwrap()
    };
    let html = as_mode("html");
    assert_eq!(html.status, 200);
    assert_eq!(html.content_type(), Some("text/html;charset=UTF-8"));
    assert!(html.body.is_empty(), "{}", html.body_text());

    for (mode, content_type, body) in [
        ("csv", "text/csv;charset=UTF-8", ""),
        ("text", "text/plain;charset=UTF-8", ""),
        ("xml", "text/xml;charset=UTF-8", "<PickerA>\n</PickerA>\n"),
        ("qjs", "application/javascript;charset=UTF-8", ""),
    ] {
        let output = as_mode(mode);
        assert_eq!(output.status, 200, "{mode}");
        assert_eq!(output.content_type(), Some(content_type), "{mode}");
        assert_eq!(output.body_text(), body, "{mode}");
    }
    // A screen with no widgets is server-static for qvt, so qvt is the script of an empty template, with the header that says so.
    let qvt = as_mode("qvt");
    assert_eq!(qvt.status, 200);
    assert_eq!(qvt.content_type(), Some("text/javascript;charset=UTF-8"));
    assert_eq!(qvt.header("X-Moqui-Qvt"), Some("static-template"));
    assert!(qvt.body_text().contains("m.qvtStatic"), "{}", qvt.body_text());
    // The print mode writes an empty document, and xsl-fo is the same mode.
    let print = as_mode("html-print");
    assert_eq!(print.content_type(), Some("text/html;charset=UTF-8"));
    assert!(
        print.body_text().starts_with("<!DOCTYPE html>"),
        "{}",
        print.body_text()
    );
    assert!(print
        .body_text()
        .ends_with("<body class=\"letter-portrait\">\n</body></html>\n"));
    assert_eq!(as_mode("xsl-fo").body_text(), print.body_text());
    // The render mode can come from the builder or the renderMode parameter instead of the path.
    let by_builder = make_render(context.factory())
        .root_screen(picker.clone())
        .render_mode("xsl-fo")
        .render()
        .unwrap();
    assert_eq!(by_builder.content_type(), Some("text/html;charset=UTF-8"));
    let by_parameter = make_render(context.factory())
        .root_screen(picker.clone())
        .parameters(parameters(&[("renderMode", "csv")]))
        .render()
        .unwrap();
    assert_eq!(by_parameter.content_type(), Some("text/csv;charset=UTF-8"));
    // A mode nobody configured is a 404.
    let unknown = make_render(context.factory())
        .root_screen(picker)
        .render_mode("vuet")
        .render()
        .unwrap_err();
    assert_eq!(unknown.kind(), ErrorKind::NotFound);
}

#[test]
fn make_render_fills_parameters_session_and_base_url_from_maps() {
    // A webapp that names a host, so a redirect keeps the absolute URL, as Java's sendHttpRedirect does.
    let context = ExecutionContextFactory::test_context_with_conf(
        "<moqui-conf><webapp-list><webapp name=\"webroot\" http-host=\"app.example\"/></webapp-list></moqui-conf>",
    );
    let output = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/echo")
        .parameters(parameters(&[("p", "from-map")]))
        .session(parameters(&[("who", "session-value")]))
        .render()
        .unwrap();
    let text = output.body_text();
    assert!(text.contains("\"who\" : \"session-value\""), "{text}");
    assert!(text.contains("\"p\" : \"from-map\""), "{text}");
    assert!(text.contains("\"pFromContext\" : \"from-map\""), "{text}");

    let redirect = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/goUrl")
        .base_url("https://app.example/")
        .render()
        .unwrap();
    assert_eq!(redirect.status, 302);
    assert_eq!(location_of(&redirect), Some("https://app.example/Sub"));
}

/// Java's `sendHttpRedirect`: a redirect stays on this site. An `http(s)` target is its path and query
/// when the webapp names no `http-host`, and a target that is not a path of this site goes to the root.
#[test]
fn a_redirect_location_stays_on_this_site() {
    let context = ExecutionContextFactory::test_context();
    let location = |target: &str| super::super::render::http_redirect_location(context.factory(), target);

    // No http-host: an absolute target is its path, so the browser stays here.
    assert_eq!(location("https://evil.example/steal"), "/steal");
    assert_eq!(location("https://evil.example"), "/");
    assert_eq!(location("https://evil.example/steal?x=1"), "/steal?x=1");
    assert_eq!(location("http://evil.example/steal"), "/steal");
    // A path of this site is as it is.
    assert_eq!(location("/Sub/Deep"), "/Sub/Deep");
    assert_eq!(location("Sub/Deep"), "Sub/Deep");
    // A reduced path that is not a single slash, or that holds a backslash, is the root. A URL
    // Java's URI.create rejects is the root too.
    assert_eq!(location("https://evil.example//phish"), "/");
    assert_eq!(location("https://evil.example/\\evil.com"), "/");
    assert_eq!(location("https://evil.example/a b"), "/");
    // A target that is not a path of this site goes to the root, where Java sends it as it is.
    assert_eq!(location("//evil.example/steal"), "/");
    assert_eq!(location("javascript:alert(1)"), "/");
    assert_eq!(location("mailto:someone@example.com"), "/");
    // A line break cannot split the header.
    assert_eq!(location("/Sub\r\nX-Evil: 1"), "/SubX-Evil: 1");
    assert_eq!(location("/Sub\0"), "/Sub");
    // An empty location is the root.
    assert_eq!(location("  "), "/");

    // With an http-host the absolute target is sent, as Java sends it.
    let named = ExecutionContextFactory::test_context_with_conf(
        "<moqui-conf><webapp-list><webapp name=\"webroot\" http-host=\"app.example\"/></webapp-list></moqui-conf>",
    );
    assert_eq!(
        super::super::render::http_redirect_location(named.factory(), "https://evil.example/steal"),
        "https://evil.example/steal"
    );
}

/// A screen whose response is an absolute URL redirects to its path, as Java's `sendHttpRedirect` does
/// when the webapp names no `http-host`.
#[test]
fn a_screen_that_answers_with_an_absolute_url_redirects_to_its_path() {
    let context = ExecutionContextFactory::test_context();
    let redirect = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/goExternal")
        .render()
        .unwrap();
    assert_eq!(redirect.status, 302);
    assert_eq!(location_of(&redirect), Some("/steal"));

    // The same screen path, for a URL whose path is protocol-relative after the host is dropped.
    let phish = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/goPhish")
        .render()
        .unwrap();
    assert_eq!(phish.status, 302);
    assert_eq!(location_of(&phish), Some("/"));
}

/// A qvt screen is always standalone, so `sri.sendRedirectAndStopRender` answers 205. The
/// `X-Redirect-To` value is held to the same rule as a `Location`.
#[test]
fn a_standalone_redirect_stays_on_this_site() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-redirect-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Hop.xml"),
        r#"<screen require-authentication="false">
            <always-actions><script>sri.sendRedirectAndStopRender('https://evil.example//phish')</script></always-actions>
            <widgets><label text="no"/></widgets>
        </screen>"#,
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    let response = serve_at(&world.factory, request("GET", "/Hop.qvt"), &root);
    assert_eq!(response.status, 205, "{}", response.body_text());
    assert_eq!(super::header_value(&response, "X-Redirect-To").as_deref(), Some("/"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shop P3 F-SP3-1: a status the screen's actions set before the output begins is the response's, as in Java, with
/// the widgets rendered under it and the headers kept; with none set it is 200. A transition's holds too.
#[test]
fn a_status_the_actions_set_before_output_is_the_responses() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-status-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let write = |name: &str, xml: &str| std::fs::write(dir.join(name), xml).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/Gone.xml",
        r#"<screen require-authentication="false">
            <always-actions><script>ec.web.response.setStatus(404)</script></always-actions>
            <widgets><label text="No such product"/></widgets>
        </screen>"#,
    );
    write(
        "Root/Down.xml",
        r#"<screen require-authentication="false">
            <transition name="hold"><actions><script>ec.web.response.setStatus(202)</script></actions>
                <default-response type="none"/></transition>
            <actions><script>ec.web.response.setStatus(503); ec.web.response.setHeader('Retry-After', '120')</script></actions>
            <widgets><label text="Back soon"/></widgets>
        </screen>"#,
    );
    write(
        "Root/Fine.xml",
        r#"<screen require-authentication="false"><widgets><label text="All here"/></widgets></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");

    let gone = serve_at(&world.factory, request("GET", "/Gone.text"), &root);
    assert_eq!(gone.status, 404, "{}", gone.body_text());
    assert!(gone.body_text().contains("No such product"), "{}", gone.body_text());
    // qvt: the status holds and the per-user render is still not cached.
    let gone_qvt = serve_at(&world.factory, request("GET", "/Gone.qvt"), &root);
    assert_eq!(gone_qvt.status, 404, "{}", gone_qvt.body_text());
    assert!(
        gone_qvt.body_text().contains("No such product"),
        "{}",
        gone_qvt.body_text()
    );
    assert_eq!(
        super::header_value(&gone_qvt, "Cache-Control").as_deref(),
        Some("private, no-store")
    );

    let down = serve_at(&world.factory, request("GET", "/Down.text"), &root);
    assert_eq!(down.status, 503, "{}", down.body_text());
    assert_eq!(super::header_value(&down, "Retry-After").as_deref(), Some("120"));
    assert!(down.body_text().contains("Back soon"), "{}", down.body_text());

    let held = serve_at(&world.factory, request("POST", "/Down/hold"), &root);
    assert_eq!(held.status, 202, "{}", held.body_text());

    let fine = serve_at(&world.factory, request("GET", "/Fine.text"), &root);
    assert_eq!(fine.status, 200, "{}", fine.body_text());
    assert!(fine.body_text().contains("All here"), "{}", fine.body_text());
    let _ = std::fs::remove_dir_all(&dir);
}

/// No execution context: the location is still sanitized and an off-site `http(s)` URL still collapses.
#[test]
fn a_redirect_with_no_context_is_still_held_to_this_site() {
    assert!(
        !crate::context::ExecutionContext::is_entered(),
        "this thread already has a context, so the no-factory path is not what runs"
    );
    let held = |url: &str| {
        let _web = crate::web::install(BTreeMap::new());
        crate::screen::send_redirect_and_stop_render(url);
        let (status, headers, _, sent) = crate::web::take_output();
        assert!(sent, "{url}");
        assert_eq!(status, 302, "{url}");
        headers
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("Location"))
            .map(|(_, value)| value)
            .unwrap_or_default()
    };
    assert_eq!(held("https://evil.example//phish\r\n\0"), "/");
    assert_eq!(held("https://evil.example/steal?x=1"), "/steal?x=1");
    assert_eq!(held("https://evil.example/a b"), "/");
}

/// A JSON answer carries the URL the client follows, so it is held to the same rule as the `Location`
/// of a redirect: a target that is not a path of this site goes to the root.
#[test]
fn a_json_redirect_holds_the_url_to_this_site_too() {
    let context = ExecutionContextFactory::test_context();
    let mut request = request("GET", "/goExternal");
    request.set_header("accept", "application/json");
    let response = serve_at(context.factory(), request, &fixture_root());
    let text = response.body_text();
    assert!(text.contains("\"redirectUrl\" : \"/steal\""), "{text}");
}

#[test]
fn a_screen_that_requires_authentication_is_401_for_nobody_and_200_for_a_user() {
    let world = World::open("");
    let anonymous = render(&world.factory, "/Secure").unwrap_err();
    assert_eq!(anonymous.kind(), ErrorKind::AuthenticationRequired);

    // A page in a browser goes to the login, as Java's checkWebappSettings; a fragment the client fetches gets the 401.
    let response = serve_at(&world.factory, request("GET", "/Secure.text"), &fixture_root());
    assert_eq!(response.status, 302, "{}", response.body_text());
    assert_eq!(location_of_response(&response), Some("/Login".to_string()));
    let fragment = serve_at(
        &world.factory,
        with_query(request("GET", "/Secure.text"), "lastStandalone", "true"),
        &fixture_root(),
    );
    assert_eq!(fragment.status, 401, "{}", fragment.body_text());

    world.user("admin", None);
    let ec = world.factory.new_context();
    let _entered = ec.enter();
    crate::user::login_user("admin", "secret").unwrap();
    let output = render(&world.factory, "/Secure").unwrap();
    assert_eq!(output.status, 200);
    assert!(output.body_text().contains("secure"), "{}", output.body_text());
}

/// SEC-RTE-09: a screen that needs no user, under a parent that needs one, needs one too: a `lastStandalone` fragment
/// of it and its transition are 401 for nobody, as its full page is; a user gets the fragment.
#[test]
fn a_screen_under_a_parent_that_needs_a_user_needs_one_as_a_fragment() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-parent-authc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/Locked")).unwrap();
    let write = |name: &str, xml: &str| std::fs::write(dir.join(name), xml).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/Locked.xml",
        r#"<screen><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/Locked/Open.xml",
        r#"<screen require-authentication="false">
            <transition name="poke"><actions><script>ec.web.response.setStatus(202)</script></actions>
                <default-response type="none"/></transition>
            <widgets><label text="inside the lock"/></widgets>
        </screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");

    let page = serve_at(&world.factory, request("GET", "/Locked/Open.text"), &root);
    assert_eq!(page.status, 302, "{}", page.body_text());
    let fragment = serve_at(
        &world.factory,
        with_query(request("GET", "/Locked/Open.text"), "lastStandalone", "true"),
        &root,
    );
    assert_eq!(fragment.status, 401, "{}", fragment.body_text());
    assert!(
        !fragment.body_text().contains("inside the lock"),
        "{}",
        fragment.body_text()
    );
    let poked = serve_at(&world.factory, request("POST", "/Locked/Open/poke"), &root);
    assert_eq!(poked.status, 401, "{}", poked.body_text());
    let anonymous = make_render(&world.factory)
        .root_screen(root.clone())
        .screen_path("/Locked/Open")
        .render_mode("text")
        .parameters([("lastStandalone".to_string(), Value::string("true"))].into())
        .render()
        .unwrap_err();
    assert_eq!(anonymous.kind(), ErrorKind::AuthenticationRequired);

    world.user("admin", None);
    let ec = world.factory.new_context();
    let _entered = ec.enter();
    crate::user::login_user("admin", "secret").unwrap();
    let output = make_render(&world.factory)
        .root_screen(root)
        .screen_path("/Locked/Open")
        .render_mode("text")
        .parameters([("lastStandalone".to_string(), Value::string("true"))].into())
        .render()
        .unwrap();
    assert!(output.body_text().contains("inside the lock"), "{}", output.body_text());
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-RTE-09 follow-up: a parent's `allow-web-request="false"` refuses a web request for a screen below it, as a
/// `lastStandalone` fragment too, as for its full page.
#[test]
fn a_parent_that_allows_no_web_request_refuses_a_fragment_below_it() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-parent-web-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/Internal")).unwrap();
    let write = |name: &str, xml: &str| std::fs::write(dir.join(name), xml).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/Internal.xml",
        r#"<screen require-authentication="false"><web-settings allow-web-request="false"/>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/Internal/Below.xml",
        r#"<screen require-authentication="false"><widgets><label text="not for the web"/></widgets></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    let page = serve_at(&world.factory, request("GET", "/Internal/Below.text"), &root);
    assert_eq!(page.status, 403, "{}", page.body_text());
    let fragment = serve_at(
        &world.factory,
        with_query(request("GET", "/Internal/Below.text"), "lastStandalone", "true"),
        &root,
    );
    assert_eq!(fragment.status, 403, "{}", fragment.body_text());
    assert!(
        !fragment.body_text().contains("not for the web"),
        "{}",
        fragment.body_text()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-RTE-13: a path with no screen is a plain 404 for nobody, naming neither the path nor the last screen found; a
/// signed-in user gets the name and the location to fix a link.
#[test]
fn a_missing_screen_is_a_plain_not_found_for_nobody() {
    let world = World::open("");
    let anonymous = render(&world.factory, "/NoSuchScreen").unwrap_err();
    assert_eq!(anonymous.kind(), ErrorKind::NotFound);
    assert_eq!(anonymous.message(), "screen path not found");
    let response = serve_at(&world.factory, request("GET", "/NoSuchScreen"), &fixture_root());
    assert_eq!(response.status, 404);
    assert!(!response.body_text().contains("testdata"), "{}", response.body_text());

    world.user("admin", None);
    let ec = world.factory.new_context();
    let _entered = ec.enter();
    crate::user::login_user("admin", "secret").unwrap();
    let signed_in = render(&world.factory, "/NoSuchScreen").unwrap_err();
    assert!(signed_in.message().contains("NoSuchScreen under "), "{signed_in}");
}

/// The HTTP log reads the bound session after the screen runs: session id, visitor, and username.
#[test]
fn a_screen_request_records_the_session_the_visitor_and_the_user() {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;

    struct ClearKey;
    impl Drop for ClearKey {
        fn drop(&mut self) {
            moqui_lib::env::clear_property("moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key");
        }
    }

    let _lock = crate::user::session::cookie_property_lock();
    moqui_lib::env::set_property(
        "moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key",
        STANDARD.encode([0x09_u8; 32]),
    );
    let _clear = ClearKey;
    let context = ExecutionContextFactory::test_context();
    let entered = context.enter();
    let guard = crate::user::bind(context.factory(), None, None, false).expect("bind");
    crate::user::session::set_user("user-1", "ada");
    crate::user::session::set_visit("visit-1", "visitor-1");
    let session_id = guard.session_id();
    let header = crate::user::format_session_cookie(&session_id, false).expect("cookie");
    let cookie = header.split(';').next().expect("cookie pair").to_string();
    drop(guard);
    drop(entered);
    let mut request = request("GET", "/");
    request.set_header("cookie", &cookie);
    let response = serve_at(context.factory(), request, &fixture_root());
    assert_eq!(response.http_log.session.as_deref(), Some(session_id.as_str()));
    assert_eq!(response.http_log.visitor_id.as_deref(), Some("visitor-1"));
    assert_eq!(response.http_log.remote_user.as_deref(), Some("ada"));
}

#[test]
fn an_unknown_path_is_404_over_http_and_a_not_found_error_from_make_render() {
    let context = ExecutionContextFactory::test_context();
    let err = render(context.factory(), "/Nothing/Here").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
    let world = World::open("");
    let response = serve_at(&world.factory, request("GET", "/Nothing/Here"), &fixture_root());
    assert_eq!(response.status, 404, "{}", response.body_text());
    let response = serve_at(&world.factory, request("GET", "/Sub.vuet"), &fixture_root());
    assert_eq!(response.status, 404, "{}", response.body_text());
}

fn screen_authz_conf() -> &'static str {
    "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_XML_SCREEN\" authz-enabled=\"true\"/></artifact-execution-facade></moqui-conf>"
}

#[test]
fn a_signed_in_user_without_an_allow_record_is_403_on_a_screen() {
    let world = World::open(screen_authz_conf());
    world.user("admin", Some("ADMIN"));
    world.user("plain", None);
    let secure = format!("{}/src/screen/testdata/Root/Secure.xml", env!("CARGO_MANIFEST_DIR"));
    world.allow_screen("ADMIN", &secure);
    let root = fixture_root();

    let mut denied = request("GET", "/Secure.text");
    denied.set_header("authorization", super::basic("plain"));
    let response = serve_at(&world.factory, denied, &root);
    assert_eq!(response.status, 403, "{}", response.body_text());

    let mut allowed = request("GET", "/Secure.text");
    allowed.set_header("authorization", super::basic("admin"));
    let response = serve_at(&world.factory, allowed, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("secure"), "{}", response.body_text());

    let mut nobody = with_query(request("GET", "/Secure.text"), "lastStandalone", "-1");
    nobody.peer_ip = "127.0.0.1".to_string();
    assert_eq!(serve_at(&world.factory, nobody, &root).status, 401);
}

#[test]
fn a_page_asked_for_before_login_is_where_screen_last_goes_back_to() {
    let world = World::open("");
    let root = fixture_root();
    let response = serve_at(
        &world.factory,
        with_query(request("GET", "/Secure"), "tab", "two"),
        &root,
    );
    assert_eq!(response.status, 302, "{}", response.body_text());
    let cookie = super::header_value(&response, "Set-Cookie").expect("session cookie");
    let session = cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_string();

    let mut back = request("POST", "/goLast");
    back.session_cookie = Some(session.clone());
    let response = serve_at(&world.factory, back, &root);
    assert_eq!(response.status, 302, "{}", response.body_text());
    assert_eq!(location_of_response(&response), Some("/Secure?tab=two".to_string()));

    // The saved path is used once; after it the newest history entry, else the root.
    let mut again = request("POST", "/goLast");
    again.session_cookie = Some(session);
    let response = serve_at(&world.factory, again, &root);
    assert_eq!(location_of_response(&response), Some("/".to_string()));
}

/// `save-current-screen` stores the transition path and the request parameters for the next
/// `screen-last`. It does not add a screen-history line.
#[test]
fn a_save_current_screen_is_where_screen_last_goes() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let dir = std::env::temp_dir().join(format!("moqui-screen-last-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Last")).unwrap();
    std::fs::write(
        dir.join("Last.xml"),
        r#"<screen require-authentication="false">
            <transition name="saveHere">
                <default-response url="Kept" save-current-screen="true"/>
            </transition>
            <transition name="goLast"><default-response type="screen-last"/></transition>
            <subscreens default-item="Kept"/>
            <widgets/>
        </screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Last/Kept.xml"),
        r#"<screen require-authentication="false"><widgets/></screen>"#,
    )
    .unwrap();
    let root = dir.join("Last.xml").to_string_lossy().into_owned();
    let saved = make_render(context.factory())
        .root_screen(root.clone())
        .screen_path("/saveHere")
        .parameters(parameters(&[("tab", "two"), ("password", "secret")]))
        .render()
        .unwrap();
    assert_eq!(location_of(&saved), Some("/Kept"));
    assert!(
        crate::user::screen_history().unwrap().is_empty(),
        "{:?}",
        crate::user::screen_history()
    );
    let back = make_render(context.factory())
        .root_screen(root)
        .screen_path("/goLast")
        .render()
        .unwrap();
    let location = location_of(&back).unwrap_or("");
    assert_eq!(location, "/saveHere?tab=two", "{location}");
    assert!(!location.contains("secret"), "{location}");
    let _ = std::fs::remove_dir_all(&dir);
}

fn with_query(mut request: crate::web::WebRequest, name: &str, value: &str) -> crate::web::WebRequest {
    request.query.push((name.to_string(), value.to_string()));
    request
}

fn location_of_response(response: &crate::web::WebResponse) -> Option<String> {
    super::header_value(response, "Location")
}

#[test]
fn a_cached_definition_is_reused_and_reloaded_when_its_file_changes() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    facade.set_check_interval(Some(Duration::ZERO));
    let dir = std::env::temp_dir().join(format!("moqui-screen-cache-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Cached.xml");
    std::fs::write(
        &path,
        "<screen require-authentication=\"false\" default-menu-title=\"First\"><widgets/></screen>",
    )
    .unwrap();
    let location = path.to_string_lossy().into_owned();

    let first = facade.screen_def(context.factory(), &location).unwrap();
    let hits = facade.cache_stats().hits;
    let again = facade.screen_def(context.factory(), &location).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert_eq!(facade.cache_stats().hits, hits + 1);
    assert_eq!(again.default_menu_title, "First");

    std::fs::write(
        &path,
        "<screen require-authentication=\"false\" default-menu-title=\"Second\"><widgets/></screen>",
    )
    .unwrap();
    // The write can land in the same clock tick, so move the modified time forward to be sure.
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(30)).unwrap();
    drop(file);
    let changed = facade.screen_def(context.factory(), &location).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &changed));
    assert_eq!(changed.default_menu_title, "Second");
    let stable = facade.screen_def(context.factory(), &location).unwrap();
    assert!(std::sync::Arc::ptr_eq(&changed, &stable));

    // A screen that does not compile fails to load, and the cache keeps nothing for it.
    std::fs::write(
        &path,
        "<screen><always-actions><set field=\"x\" from=\"def y = 1\"/></always-actions></screen>",
    )
    .unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(60)).unwrap();
    drop(file);
    assert!(facade.screen_def(context.factory(), &location).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn screen_permission_checks_fail_closed_without_an_execution_context() {
    let context = ExecutionContextFactory::test_context();
    let def = ScreenFacade::of(context.factory())
        .screen_def(context.factory(), &fixture_root())
        .unwrap();
    std::thread::spawn(move || {
        assert!(!crate::screen::sri::screen_permitted(&def));
        let err = crate::screen::render::Frames::default()
            .push_screen(&def, false)
            .unwrap_err();
        assert!(err.to_string().contains("no execution context"), "{err}");
    })
    .join()
    .unwrap();
}

#[test]
fn files_are_checked_once_per_interval_and_a_new_subscreen_file_is_seen() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let facade = ScreenFacade::of(factory);
    facade.set_check_interval(Some(Duration::from_secs(3600)));
    let dir = std::env::temp_dir().join(format!("moqui-screen-interval-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Top")).unwrap();
    let top = dir.join("Top.xml");
    std::fs::write(
        &top,
        r#"<screen require-authentication="false" default-menu-title="One"><widgets/></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Top/A.xml"),
        r#"<screen require-authentication="false"><widgets/></screen>"#,
    )
    .unwrap();
    let location = top.to_string_lossy().into_owned();
    let bump = |path: &std::path::Path, seconds: u64| {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(seconds))
            .unwrap();
    };

    let first = facade.screen_def(factory, &location).unwrap();
    assert!(first.subscreens.get("A").is_some() && first.subscreens.get("B").is_none());
    // Inside the interval the edit is not looked for, as a Java screen.location hit.
    std::fs::write(
        &top,
        r#"<screen require-authentication="false" default-menu-title="Two"><widgets/></screen>"#,
    )
    .unwrap();
    bump(&top, 30);
    assert!(std::sync::Arc::ptr_eq(
        &first,
        &facade.screen_def(factory, &location).unwrap()
    ));
    let path = ScreenUrlInfo::resolve(factory, &location, &["A".to_string()], 0).unwrap();
    let again = ScreenUrlInfo::resolve(factory, &location, &["A".to_string()], 0).unwrap();
    assert!(std::sync::Arc::ptr_eq(&path.target_screen, &again.target_screen));

    // Past the interval the edit and a file added to the subscreen directory are both seen, and the resolved path follows.
    facade.set_check_interval(Some(Duration::ZERO));
    std::fs::write(
        dir.join("Top/B.xml"),
        r#"<screen require-authentication="false"><widgets/></screen>"#,
    )
    .unwrap();
    let directory = std::fs::File::open(dir.join("Top")).unwrap();
    directory
        .set_modified(SystemTime::now() + Duration::from_secs(40))
        .unwrap();
    let changed = facade.screen_def(factory, &location).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &changed));
    assert_eq!(changed.default_menu_title, "Two");
    assert!(changed.subscreens.get("B").is_some());
    let fresh = ScreenUrlInfo::resolve(factory, &location, &["B".to_string()], 0).unwrap();
    assert!(fresh.target_screen.location.ends_with("Top/B.xml"));
    assert!(std::sync::Arc::ptr_eq(&fresh.path_defs[0], &changed));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transition_responses_resolve_to_a_redirect_or_nothing() {
    let context = ExecutionContextFactory::test_context();
    let go = |path: &str| render(context.factory(), path).unwrap();

    // A relative url-type screen-path starts from the screens the transition sits under.
    let relative = go("/goUrl");
    assert_eq!((relative.status, location_of(&relative)), (302, Some("/Sub")));
    let absolute = go("/goAbsolute");
    assert_eq!((absolute.status, location_of(&absolute)), (302, Some("/Sub/Deep")));
    let up = go("/Sub/up");
    assert_eq!(location_of(&up), Some("/Home"));
    let here = go("/Sub/here");
    assert_eq!(location_of(&here), Some("/Sub"));

    let plain = go("/goPlain");
    assert_eq!((plain.status, location_of(&plain)), (302, Some("/plain/target")));
    let with_parameters = go("/goParams");
    assert_eq!(location_of(&with_parameters), Some("/Sub?who=a%20b&x=1"));

    let none = go("/goNone");
    assert_eq!(none.status, 200);
    assert!(none.header("Location").is_none());
    assert!(none.body.is_empty());
}

#[test]
fn a_conditional_response_wins_over_the_default_when_its_condition_holds() {
    let context = ExecutionContextFactory::test_context();
    let go = |path: &str, flag: Option<&str>| {
        let mut render = make_render(context.factory())
            .root_screen(fixture_root())
            .screen_path(path);
        if let Some(flag) = flag {
            render = render.parameters(parameters(&[("flag", flag)]));
        }
        render.render().unwrap()
    };
    assert_eq!(location_of(&go("/goCond", Some("y"))), Some("/Home"));
    assert_eq!(location_of(&go("/goCond", Some("n"))), Some("/Sub"));
    assert_eq!(location_of(&go("/goCond", None)), Some("/Sub"));
    // A condition that is not a bare expression runs as actions.
    assert_eq!(location_of(&go("/goCompare", Some("y"))), Some("/Home"));
    assert_eq!(location_of(&go("/goCompare", Some("n"))), Some("/Sub"));
}

#[test]
fn the_error_response_is_used_when_the_actions_leave_errors() {
    let context = ExecutionContextFactory::test_context();
    let failed = render(context.factory(), "/failTx").unwrap();
    assert_eq!(failed.status, 302);
    assert_eq!(location_of(&failed), Some("/Home"));
    let fine = render(context.factory(), "/okTx").unwrap();
    assert_eq!(location_of(&fine), Some("/Sub"));
}

/// As Java: a refused JSON submit keeps its inputs for the next request, and that request takes them whatever it is (a
/// menu, a notification poll), so a page loaded later shows current fields such as a concurrency token.
#[test]
fn failed_json_submits_keep_their_inputs_for_the_next_request_only() {
    let world = World::open("");
    let submit = |path: &str| {
        let mut submit = request("POST", path);
        submit.set_header("content-type", "application/x-www-form-urlencoded");
        submit.body = b"moquiFormName=EditRecord&expectedVersion=old-version".to_vec();
        submit.set_header("accept", "application/json");
        let response = serve_at(&world.factory, submit, &fixture_root());
        let cookie = super::header_value(&response, "Set-Cookie").expect("session cookie");
        cookie.split(';').next().unwrap().to_string()
    };
    let saved_errors = |cookie: &str| {
        let context = world.factory.new_context();
        let _entered = context.enter();
        let _session = crate::user::bind(&world.factory, Some(cookie.split_once('=').unwrap().1), None, false).unwrap();
        crate::user::session::take_error_parameters()
    };
    for path in ["/failTx", "/failNone"] {
        let cookie = submit(path);
        assert_eq!(saved_errors(&cookie).get("expectedVersion").map(String::as_str), Some("old-version"), "{path}");

        let cookie = submit(path);
        let mut next = request("GET", "/goNone");
        next.set_header("cookie", &cookie);
        next.set_header("accept", "application/json");
        assert_eq!(serve_at(&world.factory, next, &fixture_root()).status, 200);
        assert!(saved_errors(&cookie).is_empty(), "{path}: the next request takes the refused inputs");
    }
}

#[test]
fn failed_redirect_submits_keep_form_inputs_for_the_following_page() {
    let world = World::open("");
    let mut submit = request("POST", "/failTx");
    submit.set_header("content-type", "application/x-www-form-urlencoded");
    submit.body = b"moquiFormName=EditRecord&description=retained+draft&password=private".to_vec();
    let response = serve_at(&world.factory, submit, &fixture_root());
    assert_eq!(response.status, 302);
    let cookie = super::header_value(&response, "Set-Cookie").expect("session cookie");
    let value = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
    let context = world.factory.new_context();
    let _entered = context.enter();
    let _session = crate::user::bind(&world.factory, Some(value), None, false).unwrap();
    let errors = crate::user::session::take_saved_for_page().error_parameters;
    assert_eq!(errors.get("description").map(String::as_str), Some("retained draft"));
    assert!(!errors.contains_key("password"));
}

/// An error already on the list, or a validation error, skips the service-call and still runs `<actions>`.
/// An error the service itself adds does not skip the actions. With neither, the service runs and then the actions.
#[test]
fn a_transition_runs_its_actions_when_there_is_already_an_error() {
    let world = World::open("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-transition-split-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Split")).unwrap();
    let services = dir.join("SplitServices.xml");
    std::fs::write(
        &services,
        r#"<services>
            <service verb="mark" noun="Ran" authenticate="false">
                <out-parameters><parameter name="y"/></out-parameters>
                <actions><set field="y" value="svc"/></actions>
            </service>
            <service verb="fail" noun="Marked" authenticate="false">
                <out-parameters><parameter name="y"/></out-parameters>
                <actions><set field="y" value="svc"/><message error="true">boom</message></actions>
            </service>
        </services>"#,
    )
    .unwrap();
    let actions = r#"<actions>
            <set field="order" value="actions"/>
            <if condition="y == 'svc'"><then><set field="order" value="service-then-actions"/></then></if>
        </actions>
        <default-response url="Ok"><parameter name="order" from="order"/></default-response>
        <error-response url="Err"><parameter name="order" from="order"/></error-response>"#;
    std::fs::write(
        dir.join("Split.xml"),
        format!(
            r#"<screen require-authentication="false">
            <always-actions>
                <if condition="priorError"><then><script>ec.message.addError('prior')</script></then></if>
                <if condition="priorValidation"><then><script>ec.message.addValidationError('Form', 'field', 'svc', 'bad field')</script></then></if>
            </always-actions>
            <transition name="go"><service-call name="mark#Ran"/>{actions}</transition>
            <transition name="failGo"><service-call name="fail#Marked"/>{actions}</transition>
            <widgets/>
        </screen>"#
        ),
    )
    .unwrap();
    for name in ["Ok", "Err"] {
        std::fs::write(
            dir.join(format!("Split/{name}.xml")),
            r#"<screen require-authentication="false"><widgets/></screen>"#,
        )
        .unwrap();
    }
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&services)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let root = dir.join("Split.xml").to_string_lossy().into_owned();
    let facade = ScreenFacade::of(&world.factory);
    let def = facade.screen_def(&world.factory, &root).unwrap();
    let go = def.transition("go", None).unwrap();
    let service_xml = go.service_actions.as_ref().unwrap().xml.to_string();
    let action_xml = go.actions.as_ref().unwrap().xml.to_string();
    assert!(
        service_xml.contains("service-call") && !action_xml.contains("service-call"),
        "{service_xml}\n{action_xml}"
    );

    let run = |path: &str, prior_error: bool, prior_validation: bool| {
        let mut parameters = BTreeMap::new();
        if prior_error {
            parameters.insert("priorError".to_string(), Value::string("true"));
        }
        if prior_validation {
            parameters.insert("priorValidation".to_string(), Value::string("true"));
        }
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .parameters(parameters)
            .render()
            .unwrap()
    };
    // An error already on the list: the service does not run, the actions do, and the error response is chosen.
    assert_eq!(location_of(&run("/go", true, false)), Some("/Err?order=actions"));
    // A validation error already on the facade does the same.
    assert_eq!(location_of(&run("/go", false, true)), Some("/Err?order=actions"));
    // The service itself adds an error: its result is visible, and the actions still run.
    assert_eq!(
        location_of(&run("/failGo", false, false)),
        Some("/Err?order=service-then-actions")
    );
    // No error: the service runs, then the actions, and the default response is the one chosen.
    assert_eq!(
        location_of(&run("/go", false, false)),
        Some("/Ok?order=service-then-actions")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A field `<set>` follows the XSD: `set-if-empty="false"` skips a null or empty new value, and any other
/// value is assigned, including an empty one. `value` is not localized.
#[test]
fn a_field_set_follows_the_xsd_rule() {
    let world = World::open("");
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let set = |attrs: &[(&str, &str)]| {
        let mut node = crate::screen::sri::NodeLit {
            name: "set".to_string(),
            ..Default::default()
        };
        for (key, value) in attrs {
            node.attrs.insert((*key).to_string(), (*value).to_string());
        }
        crate::screen::sri::set_in_context(&node).unwrap();
    };
    let text = |name: &str| crate::script::context_value(name).map(|value| moqui_lib::object::to_plain_string(&value));

    crate::script::set_context_name("status", Value::string("old")).unwrap();
    set(&[("field", "status"), ("value", ""), ("set-if-empty", "false")]);
    assert_eq!(text("status").as_deref(), Some("old"));

    set(&[("field", "status"), ("value", "new"), ("set-if-empty", "false")]);
    assert_eq!(text("status").as_deref(), Some("new"));

    set(&[("field", "status"), ("value", "")]);
    assert_eq!(text("status").as_deref(), Some(""));

    set(&[("field", "label"), ("value", "Status##keep")]);
    assert_eq!(text("label").as_deref(), Some("Status##keep"));
}

#[test]
fn a_transition_rejects_the_wrong_method_and_a_missing_required_parameter() {
    let context = ExecutionContextFactory::test_context();
    let output = render(context.factory(), "/postOnly").unwrap();
    assert_eq!(output.status, 405);
    assert_eq!(output.header("Allow"), Some("POST"));
    let posted = make_render(context.factory())
        .root_screen(fixture_root())
        .screen_path("/postOnly")
        .method("POST")
        .render()
        .unwrap();
    assert_eq!(posted.status, 200);
}

#[test]
fn the_definition_model_holds_what_the_xml_declares() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let root = facade.screen_def(context.factory(), &fixture_root()).unwrap();
    assert_eq!(root.screen_name, "Root");
    assert_eq!(root.authentication, crate::screen::Authentication::Optional);
    assert!(root.always_actions.is_some() && root.pre_actions.is_some() && root.root.actions.is_some());
    assert_eq!(root.subscreens.default_item.as_deref(), Some("Home"));

    // The service-call shortcut is its own script, with the in-map and out-map defaults the Java screen sets; no
    // multi form posts to it, so it is not a multi call (SEC-RTE-81).
    let service = root.transition("service", None).unwrap();
    assert_eq!(service.single_service.as_deref(), Some("moqui.Missing.noop"));
    let xml = &service.service_actions.as_ref().unwrap().xml;
    assert!(service.actions.is_none(), "{xml}");
    assert!(
        xml.contains("in-map=\"true\"") && !xml.contains("multi=") && !service.multi_parameter,
        "{xml}"
    );
    assert!(!service.read_only);
    assert_eq!(service.authz_action, "update");

    let by_id = root.transition("byId", Some("get")).unwrap();
    assert_eq!(by_id.path_parameters, ["itemId"]);
    let cond = root.transition("goCond", None).unwrap();
    assert_eq!(cond.conditional_responses.len(), 1);
    assert_eq!(cond.conditional_responses[0].url, "Home");
    assert!(matches!(
        cond.conditional_responses[0].condition,
        Some(crate::screen::Condition::Expression(_))
    ));
    let compare = root.transition("goCompare", None).unwrap();
    assert!(matches!(
        compare.conditional_responses[0].condition,
        Some(crate::screen::Condition::Wrapped(_))
    ));
    let fail = root.transition("failTx", None).unwrap();
    assert_eq!(fail.error_response.as_ref().unwrap().url, "/Home");
    let none = root.transition("goNone", None).unwrap();
    assert_eq!(none.default_response.kind, crate::screen::ResponseKind::None);
    assert!(none.read_only);

    let sub = facade
        .screen_def(
            context.factory(),
            &format!("{}/src/screen/testdata/Root/Sub.xml", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    assert!(sub.allow_extra_path);
    assert!(sub.parameter("needed").unwrap().required);
}

#[test]
fn a_transition_that_does_not_compile_fails_when_it_runs_not_when_the_screen_loads() {
    let context = ExecutionContextFactory::test_context();
    let dir = std::env::temp_dir().join(format!("moqui-screen-broken-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Broken.xml");
    std::fs::write(
        &path,
        "<screen require-authentication=\"false\"><transition name=\"bad\"><actions><set field=\"x\" from=\"def y = 1\"/></actions><default-response type=\"none\"/></transition></screen>",
    )
    .unwrap();
    let err = make_render(context.factory())
        .root_screen(path.to_string_lossy())
        .screen_path("/bad")
        .render()
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::General);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Timing for a form-list page: `cargo test -p moqui-core --release render_bench -- --ignored --nocapture`.
#[test]
#[ignore = "timing, run by hand"]
fn render_bench_form_list_page() {
    let context = ExecutionContextFactory::test_context();
    let factory = context.factory();
    let dir = std::env::temp_dir().join(format!("moqui-render-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Bench/Apps")).unwrap();
    std::fs::write(
        dir.join("Bench.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Bench/Apps.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-menu/><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    for name in ["One", "Two", "Three"] {
        std::fs::write(
            dir.join(format!("Bench/Apps/{name}.xml")),
            r#"<screen require-authentication="false"><widgets><label text="x"/></widgets></screen>"#,
        )
        .unwrap();
    }
    std::fs::write(
        dir.join("Bench/Apps/List.xml"),
        r##"<screen require-authentication="false">
            <transition name="edit"><default-response url="."/></transition>
            <actions><script>rows = []; for (let i = 0; i &lt; 200; i++) rows.push({id: 'R' + i, name: 'Name ' + i, amount: i * 1.5})</script></actions>
            <widgets><form-list name="Rows" list="rows">
                <row-actions><set field="label" from="name + '!'"/></row-actions>
                <field name="id"><default-field><link url="edit" text="${id}" parameter-map="({id: id})"/></default-field></field>
                <field name="name"><default-field><display text="${label}"/></default-field></field>
                <field name="amount"><default-field><display format="#,##0.00"/></default-field></field>
            </form-list></widgets></screen>"##,
    )
    .unwrap();
    let root = dir.join("Bench.xml").to_string_lossy().into_owned();
    let render = || {
        make_render(factory)
            .root_screen(root.clone())
            .screen_path("/Apps/List")
            .render()
            .unwrap()
    };
    let warm = render();
    assert!(warm.body_text().contains("Name 199!"), "{}", warm.body_text());
    let runs = 30;
    let started = std::time::Instant::now();
    for _ in 0..runs {
        render();
    }
    let each = started.elapsed() / runs;
    println!("render_bench: {runs} renders of a 200-row form-list, {each:?} each");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_transition_runs_its_path_in_one_transaction_and_takes_the_first_response_that_holds() {
    let world = World::open("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-transition-tx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Tx")).unwrap();
    std::fs::write(
        dir.join("Tx.xml"),
        r#"<screen require-authentication="false">
            <always-actions><if condition="mark"><then>
                <script>const made = ec.entity.makeValue('moqui.basic.EnumerationType'); made.set('enumTypeId', mark);
                    made.set('description', 'from always-actions'); made.create()</script>
            </then></if></always-actions>
            <transition name="fail"><actions><message error="true">no</message></actions><default-response url="."/></transition>
            <transition name="fine"><actions><set field="ran" value="y"/></actions>
                <conditional-response url="Never" condition="ran == 'n'"/>
                <conditional-response url="Never"><condition><expression>ran == 'n'</expression></condition></conditional-response>
                <conditional-response url="First" condition="ran == 'y'"/>
                <conditional-response url="Second"><condition><expression>true</expression></condition></conditional-response>
                <default-response url="Default"/></transition>
            <transition name="byPath"><path-parameter name="itemId"/><parameter name="itemId" required="true"/>
                <actions><set field="seen" from="itemId"/></actions><default-response url="Default"/></transition>
            <widgets/></screen>"#,
    )
    .unwrap();
    for name in ["First", "Second", "Default"] {
        std::fs::write(
            dir.join(format!("Tx/{name}.xml")),
            r#"<screen require-authentication="false"><widgets/></screen>"#,
        )
        .unwrap();
    }
    let root = dir.join("Tx.xml").to_string_lossy().into_owned();
    let post = |path: &str, mark: &str| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .method("POST")
            .parameters(BTreeMap::from([("mark".to_string(), Value::string(mark))]))
            .render()
            .unwrap()
    };
    let exists = |id: &str| {
        world
            .facade
            .find("moqui.basic.EnumerationType")
            .unwrap()
            .eq("enumTypeId", Value::string(id))
            .one()
            .unwrap()
            .is_some()
    };

    // The always-action's write rolls back with the transition that left an error, and stays with one that did not.
    post("/fail", "RolledBack");
    assert!(!exists("RolledBack"));
    let fine = post("/fine", "Kept");
    assert!(exists("Kept"));
    // of the two responses that hold, the first in the file: the ones before them do not hold and are passed over, a
    // condition attribute as a condition element
    assert_eq!(location_of(&fine), Some("/First"));

    // A required parameter that the path supplies is not missing.
    let by_path = post("/byPath/A1", "");
    assert_eq!(location_of(&by_path), Some("/Default"), "{}", by_path.body_text());
    let missing = post("/byPath", "");
    assert_eq!(missing.status, 400, "{}", missing.body_text());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_auto_display_field_shows_the_related_entity_description() {
    let world = World::open("");
    world.enumeration_type();
    let mut row = world.facade.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string("TestOne"));
    row.set("enumTypeId", Value::string("TestType"));
    row.set("description", Value::string("One"));
    row.create(&world.facade).unwrap();
    let dir = std::env::temp_dir().join(format!("moqui-screen-auto-display-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Auto.xml"),
        r#"<screen require-authentication="false"><widgets>
            <form-list name="Enums" list="enums">
                <entity-find entity-name="moqui.basic.Enumeration" list="enums"><econdition field-name="enumId" value="TestOne"/></entity-find>
                <auto-fields-entity entity-name="moqui.basic.Enumeration" field-type="display" include="all"/>
            </form-list></widgets></screen>"#,
    )
    .unwrap();
    let output = make_render(&world.factory)
        .root_screen(dir.join("Auto.xml").to_string_lossy().into_owned())
        .screen_path("/")
        .render_mode("text")
        .render()
        .unwrap();
    let text = output.body_text();
    // The type's own description with its key, from the related EnumerationType, not the Enumeration's description.
    assert!(text.contains("Test [TestType]"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// As Java's `addEntityFields`, which reads `getFieldNames`: the auto fields of a view-entity are its aliases.
#[test]
fn auto_fields_of_a_view_entity_are_its_aliases() {
    let world = World::open("");
    world.enumeration_type();
    let mut row = world.facade.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string("TestOne"));
    row.set("enumTypeId", Value::string("TestType"));
    row.set("enumCode", Value::string("CodeOne"));
    row.create(&world.facade).unwrap();
    let dir = std::env::temp_dir().join(format!("moqui-screen-auto-view-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Auto.xml"),
        r#"<screen require-authentication="false"><widgets>
            <form-list name="Enums" list="enums">
                <entity-find entity-name="moqui.basic.EnumerationAndType" list="enums"/>
                <auto-fields-entity entity-name="moqui.basic.EnumerationAndType" field-type="display" include="all"/>
            </form-list></widgets></screen>"#,
    )
    .unwrap();
    let output = make_render(&world.factory)
        .root_screen(dir.join("Auto.xml").to_string_lossy().into_owned())
        .screen_path("/")
        .render_mode("text")
        .render()
        .unwrap();
    let text = output.body_text();
    assert!(text.contains("CodeOne") && text.contains("TestOne"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn subscreens_item_rows_mount_screens_and_a_user_group_limits_them() {
    let world = World::open("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-subscreens-rows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Base")).unwrap();
    std::fs::write(
        dir.join("Base.xml"),
        r#"<screen require-authentication="false"><subscreens default-item="Own"/><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Base/Own.xml"),
        r#"<screen require-authentication="false"><widgets><label text="own"/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("App.xml"),
        r#"<screen require-authentication="false">
            <transition name="peek" read-only="true"><actions><script>ec.web.sendJsonResponse({peeked: true})</script></actions>
                <default-response type="none"/></transition>
            <widgets><label text="mounted app"/></widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Base.xml").to_string_lossy().into_owned();
    let render_path = |path: &str| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .render_mode("text")
            .render()
    };
    // F133: the first screen of a lastStandalone render, which no renderSubscreen decides, and a transition below it
    let render_standalone = |path: &str| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path(path)
            .render_mode("text")
            .parameters(
                [("lastStandalone".to_string(), Value::string("true"))]
                    .into_iter()
                    .collect(),
            )
            .render()
    };
    // Loaded before the rows exist.
    assert!(render_path("/").unwrap().body_text().contains("own"));
    let row = |name: &str, group: &str, default: &str| {
        let mut row = world.facade.make_value("moqui.screen.SubscreensItem").unwrap();
        row.set("screenLocation", Value::string(&root));
        row.set("subscreenName", Value::string(name));
        row.set("userGroupId", Value::string(group));
        row.set(
            "subscreenLocation",
            Value::string(dir.join("App.xml").to_string_lossy()),
        );
        row.set("menuInclude", Value::string("Y"));
        row.set("makeDefault", Value::string(default));
        row.create(&world.facade).unwrap();
    };
    row("Mounted", "ALL_USERS", "Y");
    row("Admins", "ADMIN", "N");

    // The write makes the cached definition stale at once: the row's item is there and is now the default.
    assert!(render_path("/Mounted").unwrap().body_text().contains("mounted app"));
    assert!(render_path("/").unwrap().body_text().contains("mounted app"));
    // A user who is not in the row's group does not get the screen.
    let denied = render_path("/Admins").unwrap_err();
    assert_eq!(denied.kind(), ErrorKind::Unauthorized, "{denied}");
    // Nor alone (lastStandalone), nor its transitions; the ALL_USERS item still renders alone and runs its transition.
    assert!(render_standalone("/Mounted")
        .unwrap()
        .body_text()
        .contains("mounted app"));
    let denied = render_standalone("/Admins").unwrap_err();
    assert_eq!(denied.kind(), ErrorKind::Unauthorized, "{denied}");
    assert!(render_path("/Mounted/peek").unwrap().body_text().contains("peeked"));
    let denied = render_path("/Admins/peek").unwrap_err();
    assert_eq!(denied.kind(), ErrorKind::Unauthorized, "{denied}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_subscreens_read_is_not_cached_as_no_rows() {
    let world = World::open("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-subscreens-failed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Base")).unwrap();
    std::fs::write(
        dir.join("Base.xml"),
        r#"<screen require-authentication="false"><subscreens default-item="Own"/><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Base/Own.xml"),
        r#"<screen require-authentication="false"><widgets><label text="own"/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("App.xml"),
        r#"<screen require-authentication="false"><widgets><label text="mounted app"/></widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Base.xml").to_string_lossy().into_owned();
    let mut row = world.facade.make_value("moqui.screen.SubscreensItem").unwrap();
    row.set("screenLocation", Value::string(&root));
    row.set("subscreenName", Value::string("Mounted"));
    row.set("userGroupId", Value::string("ALL_USERS"));
    row.set(
        "subscreenLocation",
        Value::string(dir.join("App.xml").to_string_lossy()),
    );
    row.set("makeDefault", Value::string("Y"));
    row.create(&world.facade).unwrap();

    // A transaction marked rollback-only fails every read, as a database that is down would.
    let tx = world.facade.transactions_for("moqui.screen.SubscreensItem").unwrap();
    let mut resolved = None;
    let ended = tx.run(crate::transaction::TransactionAttribute::ForceNew, None, || {
        tx.set_rollback_only("the database is away")?;
        resolved = Some(ScreenUrlInfo::resolve(&world.factory, &root, &[], 0));
        Ok(())
    });
    assert_eq!(ErrorKind::of(&ended.unwrap_err()), ErrorKind::RollbackOnly);
    let failed = resolved.unwrap().unwrap();
    assert!(
        !failed.reusable,
        "a path resolved without its SubscreensDefault rows is not cached"
    );
    assert_eq!(
        failed.path_defs[0].config_generation,
        crate::screen::definition::STALE_GENERATION
    );
    assert_eq!(failed.full_path, vec!["Own"]);

    // The next request reads the rows and mounts the app as the default.
    let output = make_render(&world.factory)
        .root_screen(root.clone())
        .screen_path("/")
        .render_mode("text")
        .render()
        .unwrap();
    assert!(output.body_text().contains("mounted app"), "{}", output.body_text());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_production_500_and_a_failed_section_show_an_error_id_not_the_detail() {
    let world = World::open("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-error-id-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Fails.xml"),
        r#"<screen require-authentication="false"><actions><script>throw new Error('SELECT * FROM SECRET_TABLE')</script></actions>
            <widgets/></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Section.xml"),
        r#"<screen require-authentication="false"><widgets><label text="before"/>
            <section name="Broken"><actions><script>throw new Error('SELECT * FROM SECRET_TABLE')</script></actions>
                <widgets><label text="never"/></widgets></section></widgets></screen>"#,
    )
    .unwrap();
    let serve = |name: &str, production: bool| {
        crate::web::TEST_PRODUCTION.with(|flag| flag.set(Some(production)));
        let root = dir.join(name).to_string_lossy().into_owned();
        let response = serve_at(&world.factory, request("GET", "/"), &root);
        crate::web::TEST_PRODUCTION.with(|flag| flag.set(None));
        response
    };

    let dev = serve("Fails.xml", false);
    assert_eq!(dev.status, 500);
    assert!(dev.body_text().contains("SECRET_TABLE"), "{}", dev.body_text());
    let production = serve("Fails.xml", true);
    assert_eq!(production.status, 500);
    assert!(
        !production.body_text().contains("SECRET_TABLE"),
        "{}",
        production.body_text()
    );
    assert!(
        production.body_text().contains("internal error, id "),
        "{}",
        production.body_text()
    );

    let dev = serve("Section.xml", false);
    assert!(dev.body_text().contains("SECRET_TABLE"), "{}", dev.body_text());
    let production = serve("Section.xml", true);
    let body = production.body_text();
    assert!(body.contains("before"), "{body}");
    assert!(
        body.contains("Error rendering") && body.contains("internal error, id "),
        "{body}"
    );
    assert!(!body.contains("SECRET_TABLE"), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_transition_link_carries_its_service_keys_and_a_failed_condition_disables_it() {
    let world = World::open("");
    world.enumeration_type();
    let dir = std::env::temp_dir().join(format!("moqui-screen-link-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Links.xml"),
        r#"<screen require-authentication="false">
            <transition name="deleteType"><service-call name="delete#moqui.basic.EnumerationType"/><default-response url="."/></transition>
            <transition name="never"><condition><expression>false</expression></condition><actions><set field="x" value="1"/></actions>
                <default-response url="."/></transition>
            <widgets>
                <form-list name="Types" list="types">
                    <entity-find entity-name="moqui.basic.EnumerationType" list="types"/>
                    <field name="enumTypeId"><default-field><display/></default-field></field>
                    <field name="remove"><default-field><link url="deleteType" text="Remove" link-type="anchor"/></default-field></field>
                </form-list>
                <link url="never" text="Never" link-type="anchor"/>
            </widgets></screen>"#,
    )
    .unwrap();
    let output = make_render(&world.factory)
        .root_screen(dir.join("Links.xml").to_string_lossy().into_owned())
        .screen_path("/")
        .render()
        .unwrap();
    let html = output.body_text();
    assert!(html.contains("deleteType?enumTypeId=TestType"), "{html}");
    // A disabled link is written without its target.
    assert!(!html.contains("/never"), "{html}");
    assert!(html.contains("Never"), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A render decides a link path's permission once, but a transition's condition on each row it is asked in.
#[test]
fn a_transition_condition_disables_its_link_row_by_row() {
    let world = World::open("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-link-rows-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Rows.xml"),
        r#"<screen require-authentication="false">
            <transition name="maybe"><condition><expression>ok</expression></condition><actions><set field="x" value="1"/></actions>
                <default-response url="."/></transition>
            <actions><script>rows = [{rowId:'ra', ok:true}, {rowId:'rb', ok:false}, {rowId:'rc', ok:true}]</script></actions>
            <widgets>
                <form-list name="Rows" list="rows">
                    <field name="rowId"><default-field><display/></default-field></field>
                    <field name="go"><default-field><link url="maybe" text="Go" link-type="anchor" parameter-map="{rowId:rowId}"/></default-field></field>
                </form-list>
            </widgets></screen>"#,
    )
    .unwrap();
    let output = make_render(&world.factory)
        .root_screen(dir.join("Rows.xml").to_string_lossy().into_owned())
        .screen_path("/")
        .render()
        .unwrap();
    let html = output.body_text();
    assert!(html.contains("maybe?rowId=ra"), "{html}");
    assert!(!html.contains("rowId=rb"), "{html}");
    assert!(html.contains("maybe?rowId=rc"), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Allow `group` the `action` on one artifact, with the authorization inherited by what runs under it.
fn grant(world: &World, id: &str, group: &str, artifact_type: &str, name: &str, action: &str) {
    let mut member = world.facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
    member.set("artifactGroupId", Value::string(id));
    member.set("artifactName", Value::string(name));
    member.set("artifactTypeEnumId", Value::string(artifact_type));
    member.set("nameIsPattern", Value::string("N"));
    member.set("inheritAuthz", Value::string("Y"));
    member.create(&world.facade).unwrap();
    let mut authz = world.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
    authz.set("artifactAuthzId", Value::string(id));
    authz.set("userGroupId", Value::string(group));
    authz.set("artifactGroupId", Value::string(id));
    authz.set("authzTypeEnumId", Value::string("AUTHZT_ALWAYS"));
    authz.set("authzActionEnumId", Value::string(action));
    authz.create(&world.facade).unwrap();
}

#[test]
fn a_link_to_a_transition_whose_service_the_user_may_not_call_is_disabled_and_counts_no_hit() {
    let world = World::open(
        "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_XML_SCREEN\" authz-enabled=\"true\"/>\
         <artifact-execution type=\"AT_SERVICE\" authz-enabled=\"true\"/></artifact-execution-facade>\
         <server-stats><artifact-stats type=\"AT_XML_SCREEN\" persist-bin=\"true\"/>\
         <artifact-stats type=\"AT_XML_SCREEN_TRANS\" persist-bin=\"true\"/></server-stats></moqui-conf>",
    );
    world.enumeration_type();
    world.user("plain", Some("VIEWERS"));
    world.user("admin", Some("ADMIN"));
    let dir = std::env::temp_dir().join(format!("moqui-screen-link-service-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let screen = |authentication: &str| {
        format!(
            r#"<screen require-authentication="{authentication}">
            <transition name="deleteType"><service-call name="delete#moqui.basic.EnumerationType"/><default-response url="."/></transition>
            <widgets><link url="deleteType" text="Remove" link-type="anchor" parameter-map="{{enumTypeId: 'TestType'}}"/></widgets></screen>"#
        )
    };
    std::fs::write(dir.join("Links.xml"), screen("true")).unwrap();
    std::fs::write(dir.join("Open.xml"), screen("anonymous-all")).unwrap();
    let links = dir.join("Links.xml").to_string_lossy().into_owned();
    for group in ["VIEWERS", "ADMIN"] {
        grant(
            &world,
            &format!("LINKS_{group}"),
            group,
            "AT_XML_SCREEN",
            &links,
            "AUTHZA_VIEW",
        );
    }
    grant(
        &world,
        "DELETE_TYPE",
        "ADMIN",
        "AT_SERVICE",
        "delete#moqui.basic.EnumerationType",
        "AUTHZA_ALL",
    );
    let page = |root: &str, username: &str| {
        let mut get = request("GET", "/");
        get.set_header("authorization", super::basic(username));
        let response = serve_at(&world.factory, get, root);
        assert_eq!(response.status, 200, "{}", response.body_text());
        response.body_text()
    };

    // Java greys the link when the user may view the screen but not call its service.
    let html = page(&links, "plain");
    assert!(html.contains("Remove") && !html.contains("deleteType"), "{html}");
    let html = page(&links, "admin");
    assert!(html.contains("deleteType?enumTypeId=TestType"), "{html}");
    // An anonymous-all screen needs no allow record for the service either.
    let html = page(&dir.join("Open.xml").to_string_lossy(), "plain");
    assert!(html.contains("deleteType?enumTypeId=TestType"), "{html}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_transition_that_writes_refuses_parameters_in_the_url() {
    let world = World::open("");
    let root = fixture_root();
    let mut posted = request("POST", "/okTx");
    posted.query.push(("x".to_string(), "1".to_string()));
    let response = serve_at(&world.factory, posted, &root);
    assert_eq!(response.status, 400, "{}", response.body_text());
    assert!(
        response.body_text().contains("URL parameters"),
        "{}",
        response.body_text()
    );
    // A `~name=value` segment is a URL parameter too. The action does not run, so this is not the redirect to Sub.
    for method in ["GET", "POST"] {
        let response = serve_at(&world.factory, request(method, "/okTx/~orderId=1"), &root);
        assert_eq!(response.status, 400, "{method} {}", response.body_text());
        assert!(
            response.body_text().contains("URL parameters"),
            "{method} {}",
            response.body_text()
        );
        assert!(response.header("Location").is_none(), "{method}");
    }
    // A path-parameter extra path is not a `~name=value` parameter.
    let by_id = serve_at(&world.factory, request("GET", "/byId/7"), &root);
    assert_eq!(by_id.status, 200, "{}", by_id.body_text());
    assert!(by_id.body_text().contains("\"id\" : \"7\""), "{}", by_id.body_text());
    // A read-only transition with actions still takes a tilde parameter.
    let read_tilde = serve_at(&world.factory, request("GET", "/readTx/~orderId=1"), &root);
    assert_eq!(read_tilde.status, 302, "{}", read_tilde.body_text());
    assert_eq!(location_of_response(&read_tilde).as_deref(), Some("/Sub"));
    // A read-only transition takes a query parameter.
    let mut read = request("GET", "/goUrl");
    read.query.push(("x".to_string(), "1".to_string()));
    assert_eq!(serve_at(&world.factory, read, &root).status, 302);
    // CSV, XLS, print, and text-export links put these two names on the query. Any other name is still refused.
    let mut allowed = request("GET", "/okTx");
    allowed.query.push(("lastStandalone".to_string(), "1".to_string()));
    allowed.query.push(("renderMode".to_string(), "csv".to_string()));
    let allowed = serve_at(&world.factory, allowed, &root);
    assert_eq!(allowed.status, 302, "{}", allowed.body_text());
    assert_eq!(location_of_response(&allowed).as_deref(), Some("/Sub"));
    let mut mixed = request("GET", "/okTx");
    mixed.query.push(("lastStandalone".to_string(), "1".to_string()));
    mixed.query.push(("x".to_string(), "1".to_string()));
    let mixed = serve_at(&world.factory, mixed, &root);
    assert_eq!(mixed.status, 400, "{}", mixed.body_text());
}

#[test]
fn a_timestamp_from_a_path_or_formatted_without_a_pattern_shows_formatted() {
    let world = World::open("");
    world.enumeration_type();
    let mut row = world.facade.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string("TestOne"));
    row.set("enumTypeId", Value::string("TestType"));
    row.create(&world.facade).unwrap();
    let dir = std::env::temp_dir().join(format!("moqui-screen-timestamp-display-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("Stamp.xml"),
        r#"<screen require-authentication="false"><actions>
            <entity-find-one entity-name="moqui.basic.Enumeration" value-field="e"><field-map field-name="enumId" value="TestOne"/></entity-find-one>
            <set field="holder" from="({seg: e})"/>
        </actions><widgets>
            <label text="A[${ec.l10n.format(e.lastUpdatedStamp, null)}]"/>
            <label text="N[${ec.l10n.format('1234567890', null)}]"/>
            <form-single name="Stamp">
                <field name="d1" from="e.lastUpdatedStamp"><default-field><display/></default-field></field>
                <field name="d2" from="holder?.seg.lastUpdatedStamp"><default-field><display/></default-field></field>
            </form-single></widgets></screen>"#,
    )
    .unwrap();
    let output = make_render(&world.factory)
        .root_screen(dir.join("Stamp.xml").to_string_lossy().into_owned())
        .screen_path("/")
        .render_mode("text")
        .render()
        .unwrap();
    let text = output.body_text();
    // The default timestamp format, yyyy-MM-dd HH:mm, not the epoch millis a script holds.
    let shown = |prefix: &str| {
        let start = text.find(prefix).unwrap_or_else(|| panic!("{prefix} in {text}")) + prefix.len();
        text[start..].chars().take(16).collect::<String>()
    };
    let is_default = |shown: &str| {
        shown.len() == 16
            && shown.char_indices().all(|(index, c)| match index {
                4 | 7 => c == '-',
                10 => c == ' ',
                13 => c == ':',
                _ => c.is_ascii_digit(),
            })
    };
    assert!(is_default(&shown("A[")), "{text}");
    assert_eq!(text.matches(&shown("A[")).count(), 3, "{text}");
    // Other digits are not a timestamp.
    assert!(text.contains("N[1234567890]"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Commerce F72: a screen's actions and widget expressions cannot reach `runAsSystem`, even when a service renders the
/// screen (the render began after the service's frame, so what runs is the screen's, not the service's own code).
#[test]
fn a_screen_rendered_by_a_service_cannot_run_as_system() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-system-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let attempt = "(function () { try { return ec.artifactExecution.runAsSystem(() => 'allowed') } \
                   catch (e) { return 'refused' } })()";
    std::fs::write(
        dir.join("Root.xml"),
        format!(
            r#"<screen require-authentication="false">
                <actions><script>fromActions = {attempt}</script></actions>
                <widgets><label text="actions:${{fromActions}} widget:${{{attempt}}}"/></widgets>
            </screen>"#
        ),
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    context.push("AT_SERVICE", "test.Services.render#Page", "view");
    let output = make_render(context.factory())
        .root_screen(&root)
        .render_mode("text")
        .render()
        .unwrap();
    context.pop().unwrap();
    let text = output.body_text();
    assert!(text.contains("actions:refused widget:refused"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shop F72: the `.qvue` of a public island (a screen `server-static` for qvue, or one that declines its session) is a
/// cacheable asset for a cookieless visitor, so it sends no cookie and keeps no session. A request with a session of its
/// own, a signed-in user, and a screen that is neither all get `private, no-store`, and their sessions are kept.
#[test]
fn a_public_islands_component_makes_no_session() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-island-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    );
    let island = |attributes: &str, actions: &str| {
        format!(
            r#"<screen require-authentication="false" render-modes="html,qvue" {attributes}>{actions}
                <widgets><render-mode><text type="qvue" template="false"><![CDATA[<template><p>island</p></template>
<script>module.exports = {{}}</script>]]></text></render-mode></widgets></screen>"#
        )
    };
    write("Root/Static.xml", &island(r#"server-static="qvue""#, ""));
    write(
        "Root/Declined.xml",
        &island("", "<actions><script>ec.web.declineSession()</script></actions>"),
    );
    write("Root/Plain.xml", &island("", ""));
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    world.user("ann", None);
    let sessions = |world: &World| {
        crate::user::session::flush_dirty(&world.factory).unwrap();
        world
            .facade
            .find("moqui.server.HttpSession")
            .unwrap()
            .list()
            .unwrap()
            .len()
    };
    let cache_control = |response: &crate::web::WebResponse| {
        response
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("cache-control"))
            .map(|(_, value)| value.clone())
            .collect::<Vec<_>>()
    };
    let cookie = |response: &crate::web::WebResponse| super::header_value(response, "Set-Cookie");
    let before = sessions(&world);
    for path in ["/Static.qvue", "/Declined.qvue"] {
        let response = serve_at(&world.factory, request("GET", path), &root);
        assert_eq!(response.status, 200, "{path}: {}", response.body_text());
        assert!(response.body_text().contains("<p>island</p>"), "{path}");
        assert_eq!(cookie(&response), None, "{path}");
        assert_eq!(super::header_value(&response, "moquiSessionToken"), None, "{path}");
        let cache = cache_control(&response);
        assert!(cache.len() == 1 && cache[0].contains("public"), "{path}: {cache:?}");
    }
    assert_eq!(sessions(&world), before, "no session row for a public island");

    // a screen that is neither: private, and a session starts
    let response = serve_at(&world.factory, request("GET", "/Plain.qvue"), &root);
    assert_eq!(cache_control(&response), ["private, no-store"]);
    let session = cookie(&response).expect("a session cookie");
    let session = session.split(';').next().unwrap().to_string();
    // a request with its own session: private, and the session is kept
    let mut with_session = request("GET", "/Static.qvue");
    with_session.set_header("cookie", &session);
    let response = serve_at(&world.factory, with_session, &root);
    assert_eq!(response.status, 200);
    assert_eq!(cache_control(&response), ["private, no-store"]);
    assert_eq!(sessions(&world), before + 1);
    // a signed cookie naming a session that is gone: public, and the new session's cookie is not sent with it (a cache
    // would hand it to everyone)
    let gone = crate::user::session::format_session_cookie("0123456789abcdef0123456789abcdef01234567", false).unwrap();
    let mut stale = request("GET", "/Static.qvue");
    stale.set_header("cookie", gone.split(';').next().unwrap());
    let response = serve_at(&world.factory, stale, &root);
    assert_eq!(response.status, 200);
    let cache = cache_control(&response);
    assert!(cache.len() == 1 && cache[0].contains("public"), "{cache:?}");
    assert_eq!(cookie(&response), None);
    assert_eq!(sessions(&world), before + 1);
    // a signed-in user: private
    let mut signed_in = request("GET", "/Static.qvue");
    signed_in.set_header("authorization", super::basic("ann"));
    let response = serve_at(&world.factory, signed_in, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(cache_control(&response), ["private, no-store"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Commerce F87: a `<public-site>` with a `host-check-service` that answers `restricted` for its host keeps the framework
/// routes off that host (404), but not the static ones a page there loads; another host keeps them, and a check that
/// fails refuses the route.
#[test]
fn a_restricted_host_refuses_the_framework_routes_but_not_the_static_ones() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-preview-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let write = |name: &str, text: &str| std::fs::write(dir.join(name), text).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    );
    for name in ["apps", "libs"] {
        write(
            &format!("Root/{name}.xml"),
            &format!(
                r#"<screen require-authentication="false"><widgets><label text="{name} here"/></widgets></screen>"#
            ),
        );
    }
    write(
        "HostServices.xml",
        r#"<services><service verb="check" noun="PreviewHost" authenticate="anonymous-all">
            <in-parameters><parameter name="siteId"/><parameter name="hostname"/><parameter name="path"/></in-parameters>
            <out-parameters><parameter name="restricted" type="Boolean"/></out-parameters>
            <actions><script>if (hostname === 'fail.test') throw new Error('the check is down')
                restricted = siteId === 'SHOP' &amp;&amp; hostname === 'preview.test'</script></actions>
        </service></services>"#,
    );
    let site = |host: &str| {
        format!(
            r#"<public-site host="{host}" site-id="SHOP" screen-location="component://none/Shop.xml"
                host-check-service="check#PreviewHost"><mount path="/" screen="Shop"/></public-site>"#
        )
    };
    let world = World::open(&format!(
        r#"<moqui-conf><webapp-list><webapp name="webroot">{}{}{}</webapp></webapp-list></moqui-conf>"#,
        site("preview.test"),
        site("shop.test"),
        site("fail.test")
    ));
    {
        let context = world.factory.new_context();
        let _entered = context.enter();
        let failures = crate::service::lint(&[dir.join("HostServices.xml")]).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let get = |host: &str, path: &str| {
        let mut on = request("GET", path);
        on.set_header("host", host);
        serve_at(&world.factory, on, &root)
    };
    let preview = get("preview.test", "/apps");
    assert_eq!(preview.status, 404, "{}", preview.body_text());
    assert!(!preview.body_text().contains("apps here"));
    let static_file = get("preview.test", "/libs");
    assert_eq!(static_file.status, 200, "{}", static_file.body_text());
    assert!(static_file.body_text().contains("libs here"));
    let live = get("shop.test:8443", "/apps");
    assert_eq!(live.status, 200, "{}", live.body_text());
    assert!(live.body_text().contains("apps here"));
    assert_eq!(get("fail.test", "/apps").status, 404, "a failed check refuses");
    // a static first name counts only while the screen path stays under it; another servlet's path is refused
    for path in ["/libs/../apps", "/libs/x/../../apps", "/libs/..", "/fop/apps"] {
        let response = get("preview.test", path);
        assert_eq!(response.status, 404, "{path}: {}", response.body_text());
        assert_eq!(super::header_value(&response, "Set-Cookie"), None, "{path}: the refusal makes no session");
    }
    assert_eq!(get("preview.test", "/libs/x/../.").status, 200, "still the libs");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-82: a screen that stands at a preview's proposed text (an overlay) writes a placeholder for its render-mode
/// text, so a script the model wrote there does not run in the application's origin; the same text from the file
/// renders as written.
#[test]
fn a_previewed_screen_writes_a_placeholder_for_its_client_text() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Preview.xml");
    let screen = |marker: &str| {
        format!(
            r#"<screen require-authentication="false"><widgets><label text="shown {marker}"/>
            <render-mode><text type="html"><![CDATA[<script>window.{marker} = 1</script>]]></text></render-mode>
            </widgets></screen>"#
        )
    };
    std::fs::write(&location, screen("FROM_FILE")).unwrap();
    let root = location.to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode("html")
            .render()
            .unwrap()
            .body_text()
    };
    let plain = render();
    assert!(plain.contains("window.FROM_FILE = 1"), "{plain}");
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&root, None, &screen("PROPOSED"));
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(previewed.contains("shown PROPOSED"), "{previewed}");
    assert!(!previewed.contains("window.PROPOSED"), "{previewed}");
    assert!(previewed.contains("not shown in a preview"), "{previewed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: the actions of a screen that stands at a preview's proposed text run with authorization required, and a
/// script's `disableAuthz()` there is refused, of `ec.artifactExecution`, a find and a service call alike, and in a file
/// they run; the same text from the file disables it as written.
#[test]
fn a_previewed_screens_actions_may_not_disable_authorization() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-authz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Authz.xml");
    let screen = r#"<screen require-authentication="false"><actions>
        <script><![CDATA[
            const refused = []
            const tries = [
                ['artifact', () => { const was = ec.artifactExecution.disableAuthz(); if (!was) ec.artifactExecution.enableAuthz() }],
                ['find', () => ec.entity.find('moqui.basic.Enumeration').disableAuthz()],
                ['service', () => ec.service.sync().name('org.moqui.impl.BasicServices.noop#Service').disableAuthz()],
            ]
            for (const [name, run] of tries) { try { run() } catch (e) { refused.push(name) } }
            refusedText = refused.join(',') || 'none'
        ]]></script>
        <script>try { ec.resource.script(libraryAt) ; libraryText = 'ran' } catch (e) { libraryText = 'refused' }</script>
        </actions><widgets>
            <label text="REFUSED[${refusedText}] LIBRARY[${libraryText}]"/>
        </widgets></screen>"#;
    // a file of a component the proposed actions run: it may disable authorization only where they may
    let library = dir.join("Library.js");
    std::fs::write(
        &library,
        "ec.artifactExecution.disableAuthz(); ec.artifactExecution.enableAuthz(); 1",
    )
    .unwrap();
    std::fs::write(&location, screen).unwrap();
    let root = location.to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode("text")
            .parameters(BTreeMap::from([(
                "libraryAt".to_string(),
                Value::string(library.to_string_lossy()),
            )]))
            .render()
            .unwrap()
            .body_text()
    };
    let plain = render();
    assert!(plain.contains("REFUSED[none] LIBRARY[ran]"), "{plain}");
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&root, None, screen);
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(
        previewed.contains("REFUSED[artifact,find,service] LIBRARY[refused]"),
        "{previewed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: the whole of a proposed screen runs under the lock, not only its `<actions>`: a form-list's row-actions and
/// its `entity-find`, a drop-down's `entity-options`, a tree node, a label's `${}` and a section's `condition` may not
/// disable authorization, and its actions may not sign in as another user; the same text from the file may.
#[test]
fn a_previewed_screens_widgets_forms_trees_and_expressions_may_not_disable_authorization_or_switch_user() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-widgets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // `PROBE` disables authorization on a find and answers whether that was refused
    let probe = "(() => { try { ec.entity.find('moqui.basic.Enumeration').disableAuthz(); return 'ALLOWED' } \
        catch (e) { return String(e.message).includes('not available in a preview') ? 'REFUSED' : 'OTHER ' + e.message } })()";
    let refused = "not available in a preview";
    let enums = r#"<entity-find entity-name="moqui.basic.Enumeration" list="enumList" disable-authz="true"><econdition field-name="enumId" value="NONE_SUCH"/></entity-find>"#;
    let cases: Vec<(&str, String)> = vec![
        (
            "actions",
            format!(
                r#"<actions><script>out = {probe}</script></actions><widgets><label text="MARK ${{out}}"/></widgets>"#
            ),
        ),
        (
            "login",
            r#"<actions><script>try { ec.user.internalLoginUser('nobody.here'); out = 'ALLOWED' }
                catch (e) { out = String(e.message).includes('not available in a preview') ? 'REFUSED' : 'ALLOWED' }
                ec.message.clearErrors()</script></actions>
                <widgets><label text="MARK ${out}"/></widgets>"#
                .to_string(),
        ),
        (
            "label",
            format!(r#"<widgets><label text="MARK ${{{probe}}}"/></widgets>"#),
        ),
        (
            "condition",
            format!(
                r#"<widgets><section name="S" condition="{probe} == 'ALLOWED'"><widgets><label text="MARK ALLOWED"/></widgets>
                <fail-widgets><label text="MARK REFUSED"/></fail-widgets></section></widgets>"#
            ),
        ),
        (
            "row-actions",
            format!(
                r#"<actions><script>rows = [{{v: 1}}]</script></actions><widgets><form-list name="F" list="rows" skip-form="true">
                <row-actions><script>out = 'MARK ' + {probe}</script></row-actions>
                <field name="out"><default-field><display/></default-field></field></form-list></widgets>"#
            ),
        ),
        (
            "form-find",
            format!(
                r#"<widgets><label text="MARK ALLOWED"/><form-list name="F" list="enumList" skip-form="true">{enums}
                <field name="enumId"><default-field><display/></default-field></field></form-list></widgets>"#
            ),
        ),
        (
            "options",
            format!(
                r#"<widgets><label text="MARK ALLOWED"/><form-single name="F"><field name="enumId"><default-field><drop-down>
                <entity-options key="${{enumId}}" text="${{description}}">{}</entity-options></drop-down></default-field></field>
                </form-single></widgets>"#,
                enums.replace(r#" list="enumList""#, "")
            ),
        ),
    ];
    for (name, body) in &cases {
        let location = dir.join(format!("Probe{}.xml", name.replace('-', "")));
        let screen = format!(r#"<screen require-authentication="false">{body}</screen>"#);
        std::fs::write(&location, &screen).unwrap();
        let root = location.to_string_lossy().into_owned();
        let render = || {
            make_render(&world.factory)
                .root_screen(root.clone())
                .screen_path("")
                .render_mode("html")
                .render()
                .map(|output| output.body_text())
                .unwrap_or_else(|err| err.message().to_string())
        };
        let plain = render();
        assert!(
            plain.contains("MARK ALLOWED") && !plain.contains(refused),
            "{name} from the file: {plain}"
        );
        let mut overlay = crate::screen::overlay::Overlay::new();
        overlay.insert(&root, None, &screen);
        let previewed = crate::screen::overlay::with(overlay, render);
        assert!(
            previewed.contains("MARK REFUSED") || previewed.contains(refused),
            "{name} in a preview: {previewed}"
        );
        assert!(
            !previewed.contains("MARK ALLOWED") || previewed.contains(refused),
            "{name} in a preview: {previewed}"
        );
    }
    // a tree's node data, asked by its own request
    let location = dir.join("ProbeTree.xml");
    let screen = format!(
        r#"<screen require-authentication="false"><widgets><tree name="T">
        <tree-node name="N"><label text="${{nodeList_entry.name}}"/></tree-node>
        <tree-sub-node node-name="N" list="nodes"><actions><script>nodes = [{{name: 'MARK ' + {probe}}}]</script></actions></tree-sub-node>
        </tree></widgets></screen>"#
    );
    std::fs::write(&location, &screen).unwrap();
    let root = location.to_string_lossy().into_owned();
    let fetch = || {
        let mut get = request("GET", "/actions/T");
        get.query = vec![("treeNodeId".to_string(), "#".to_string())];
        serve_at(&world.factory, get, &root).body_text()
    };
    let plain = fetch();
    assert!(plain.contains("MARK ALLOWED"), "tree from the file: {plain}");
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&root, None, &screen);
    let previewed = crate::screen::overlay::with(overlay, fetch);
    assert!(previewed.contains("MARK REFUSED"), "tree in a preview: {previewed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: a proposed screen's code runs on an allow-list host. Raw SQL, a resource read, the caches, the factory,
/// the effective time, an artifact frame, `runAsUser`, and every `ec.web` member (a redirect, a header such as the
/// response's policy, a cookie, raw output) are not available to it; the same text from the file reaches them (raw SQL
/// then meets its own gate).
#[test]
fn a_previewed_screen_reaches_no_sql_file_cache_clock_or_response() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-reach-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, "the file").unwrap();
    // `probe(text)` runs the text and answers REFUSED when the preview's host refused it, ALLOWED for anything else
    let probe = |text: &str| {
        format!(
            "(() => {{ try {{ {text}; return 'ALLOWED' }} catch (e) {{ \
             return String(e.message).includes('not available in a preview') ? 'REFUSED' : 'ALLOWED' }} }})()"
        )
    };
    let file = format!("ec.resource.getLocationText('file://{}')", secret.to_string_lossy());
    let cases = [
        (
            "sql",
            probe("ec.entity.runSqlStatement('transactional', 'select user_id from user_account', 5)"),
        ),
        ("file", probe(&file)),
        (
            "component",
            probe("ec.resource.getLocationText('component://webroot/screen/webroot.xml')"),
        ),
        ("cache", probe("ec.cache.getCache('test.preview').get('x')")),
        ("factory", probe("ec.factory.getConfValue('webapp@name')")),
        ("disabled", probe("ec.artifactExecution.authzDisabled()")),
        ("clock", probe("ec.user.setEffectiveTime(1)")),
        (
            "frame",
            probe(
                "ec.artifactExecution.push('component://webroot/screen/webroot.xml', 'AT_XML_SCREEN', 'view', false)",
            ),
        ),
        (
            "runas",
            probe("ec.artifactExecution.runAsUser('component://webroot/screen/webroot.xml', () => 1)"),
        ),
        (
            "redirect",
            probe("ec.web.response.sendRedirect('https://collector.example.net/')"),
        ),
        (
            "policy",
            probe("ec.web.response.setHeader('Content-Security-Policy', 'default-src *')"),
        ),
        ("cookie", probe("ec.web.response.addHeader('Set-Cookie', 'taken=1')")),
        ("write", probe("ec.web.response.write('<script>steal()</script>')")),
        ("params", probe("ec.web.parameters")),
    ];
    let refused = "not available in a preview";
    for (name, expression) in &cases {
        let location = dir.join(format!("Reach{name}.xml"));
        let screen = format!(
            r#"<screen require-authentication="false"><actions><script>out = {expression}; ec.message.clearErrors()</script></actions>
            <widgets><label text="MARK ${{out}}"/></widgets></screen>"#
        );
        std::fs::write(&location, &screen).unwrap();
        let root = location.to_string_lossy().into_owned();
        let render = || {
            make_render(&world.factory)
                .root_screen(root.clone())
                .screen_path("")
                .render_mode("text")
                .render()
                .map(|output| output.body_text())
                .unwrap_or_else(|err| err.message().to_string())
        };
        let plain = render();
        assert!(
            plain.contains("MARK ALLOWED") && !plain.contains(refused),
            "{name} from the file: {plain}"
        );
        let mut overlay = crate::screen::overlay::Overlay::new();
        overlay.insert(&root, None, &screen);
        let previewed = crate::screen::overlay::with(overlay, render);
        assert!(previewed.contains("MARK REFUSED"), "{name} in a preview: {previewed}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: the allow-list host still renders a preview's data: one call of each allowed group (an entity find, a
/// service call, the messages, the localizations, the logger, the context, the user's reads, the helper objects, a
/// decimal, `sri`'s reads and URLs, an expand, the lock's own question) gives its value.
#[test]
fn a_previewed_screen_still_renders_with_the_allowed_members() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-allowed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Allowed.xml");
    let screen = r#"<screen require-authentication="false"><actions><script><![CDATA[
            const parts = []
            const tries = {
                find: () => ec.entity.find('moqui.basic.Enumeration').condition('enumId', 'NONE_SUCH').list().size() === 0,
                service: () => { ec.service.sync().name('org.moqui.impl.BasicServices.noop#Service').call(); return true },
                message: () => { ec.message.addMessage('a preview message'); return ec.message.messages.length > 0 },
                l10n: () => ec.l10n.localize('Hello') === 'Hello',
                logger: () => { ec.logger.info('a preview log'); return true },
                context: () => { ec.context.put('previewValue', 7); return ec.context.get('previewValue') == 7 },
                user: () => ec.user.nowTimestamp != null && ec.user.hasPermission('NONE_SUCH') === false,
                helpers: () => StringUtilities.encodeForXmlAttribute('<a>') !== '<a>',
                decimal: () => String(toDecimal('1.5').add(toDecimal('1'))) === '2.5',
                sri: () => typeof sri.getRenderMode() === 'string' && sri.buildUrl('.').url != null,
                expand: () => ec.resource.expand('${1 + 1}', '') === '2',
                locked: () => ec.artifactExecution.authzLocked() === true,
            }
            for (const [name, run] of Object.entries(tries)) {
                let ok
                try { ok = run() } catch (e) { ok = 'error ' + e.message }
                parts.push(name + '=' + ok)
            }
            out = parts.join(' ')
            ec.message.clearErrors()
        ]]></script></actions><widgets><label text="MARK ${out}"/></widgets></screen>"#;
    std::fs::write(&location, screen).unwrap();
    let root = location.to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode("html")
            .render()
            .map(|output| output.body_text())
            .unwrap_or_else(|err| err.message().to_string())
    };
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&root, None, screen);
    let previewed = crate::screen::overlay::with(overlay, render);
    for name in [
        "find", "service", "message", "l10n", "logger", "context", "user", "helpers", "decimal", "sri", "expand",
        "locked",
    ] {
        assert!(previewed.contains(&format!("{name}=true")), "{name}: {previewed}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: a preview renders on a script runtime of its own whose built-in objects are frozen, so the proposed text
/// changes no prototype the framework's render code or a later request calls; the thread's own runtime is untouched.
#[test]
fn a_previewed_screen_changes_no_built_in_object() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-frozen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Frozen.xml");
    let screen = r#"<screen require-authentication="false"><actions><script><![CDATA[
            // a sloppy assignment to a frozen property does nothing; each is read back
            const kept = []
            for (const [name, change, holds] of [
                ['join', () => { Array.prototype.join = function () { return 'taken' } }, () => [1, 2].join('-') === '1-2'],
                ['toString', () => { Object.prototype.toString = function () { return 'taken' } },
                    () => ({}).toString() === '[object Object]'],
                ['string', () => { String.prototype.trim = function () { return 'taken' } }, () => ' a '.trim() === 'a'],
                ['iterator', () => { Object.getPrototypeOf([][Symbol.iterator]()).next = null }, () => [...[1]].length === 1],
            ]) {
                try { change() } catch (e) {}
                kept.push(name + '=' + (holds() ? 'frozen' : 'changed'))
            }
            out = kept.join(' ')
        ]]></script></actions><widgets><label text="MARK ${out} ${[1, 2].join('-')}"/></widgets></screen>"#;
    std::fs::write(&location, screen).unwrap();
    let root = location.to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode("text")
            .render()
            .map(|output| output.body_text())
            .unwrap_or_else(|err| err.message().to_string())
    };
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&root, None, screen);
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(
        previewed.contains("MARK join=frozen toString=frozen string=frozen iterator=frozen 1-2"),
        "{previewed}"
    );
    // the thread's own runtime after it
    let context = world.factory.new_context();
    let _entered = context.enter();
    assert_eq!(
        crate::script::eval_value("[1, 2].join('-')")
            .map(|value| moqui_lib::object::to_plain_string(&value))
            .unwrap(),
        "1-2"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: in a preview `ec`, each facade under it, `sri` and the host classes' prototypes are frozen once the
/// preview's runtime has made them, so the proposed text replaces no function the framework's render code calls after it
/// (`ec.resource.expand`, `ec.l10n.localize`, an entity value's `get`), nor `ec` itself.
#[test]
fn a_previewed_screen_poisons_no_host_object() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-poison-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Poison.xml");
    let screen = r#"<screen require-authentication="false"><actions><script><![CDATA[
            const taken = function () { return 'POISONED' }
            const value = ec.entity.find('moqui.basic.Enumeration').condition('enumId', 'NONE_SUCH').one()
                ?? ec.entity.makeValue('moqui.basic.Enumeration')
            value.set ? value.set('enumId', 'E1') : (value.enumId = 'E1')
            const kept = []
            for (const [name, change, holds] of [
                ['expand', () => { ec.resource.expand = taken }, () => ec.resource.expand('a${1 + 1}', '') === 'a2'],
                ['define', () => { Object.defineProperty(ec.resource, 'expand', {value: taken}) },
                    () => ec.resource.expand('b', '') === 'b'],
                ['localize', () => { ec.l10n.localize = taken }, () => ec.l10n.localize('Name') !== 'POISONED'],
                ['entity', () => { Object.getPrototypeOf(value).get = taken }, () => value.get('enumId') === 'E1'],
                ['facade', () => { ec.resource = {expand: taken} }, () => ec.resource.expand('c', '') === 'c'],
                ['ec', () => { globalThis.ec = {resource: {expand: taken}} }, () => ec.resource.expand('d', '') === 'd'],
                ['sri', () => { sri.renderSection = taken }, () => sri.renderSection !== taken],
            ]) {
                try { change() } catch (e) {}
                kept.push(name + '=' + (holds() ? 'kept' : 'changed'))
            }
            out = kept.join(' ')
        ]]></script></actions><widgets><label text="MARK ${out} ${ec.l10n.localize('Name')}"/></widgets></screen>"#;
    std::fs::write(&location, screen).unwrap();
    let root = location.to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode("html")
            .render()
            .map(|output| output.body_text())
            .unwrap_or_else(|err| err.message().to_string())
    };
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&root, None, screen);
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(
        previewed.contains("MARK expand=kept define=kept localize=kept entity=kept facade=kept ec=kept sri=kept"),
        "{previewed}"
    );
    assert!(!previewed.contains("POISONED"), "{previewed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: the expressions a render unit writes into its own code (a field's `hide`, a `conditional-field`'s
/// condition, a form-single's `map`, a `text-map`, a `currency-unit-field`, a report-embed's `variables`) run in a
/// preview as a region of the allow-list host, so the proposed text in them may not write to the response; the same text
/// from the file runs as it is.
#[test]
fn a_previewed_screens_inline_expressions_run_on_the_allow_list_host() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-inline-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let refused = "not available in a preview";
    // a response write, refused anywhere in a preview's code; a JSON response in place of the page, which a render unit
    // may send but the proposed text's own code may not
    for write in [
        "ec.web.response.write('PWNED')",
        "ec.web.sendJsonResponse({x: 'PWNED'})",
    ] {
        let rows = r#"<actions><script>rows = [{v: 1}]; v = 1</script></actions>"#;
        let cases: Vec<(&str, String)> = vec![
            (
                "hide",
                format!(
                    r#"{rows}<widgets><form-list name="F" list="rows" skip-form="true"><field name="v" hide="({write}, false)">
                <default-field><display/></default-field></field></form-list></widgets>"#
                ),
            ),
            (
                "conditional-field",
                format!(
                    r#"{rows}<widgets><form-single name="F"><field name="v"><conditional-field condition="({write}, true)">
                <display text="x"/></conditional-field><default-field><display text="y"/></default-field></field>
                </form-single></widgets>"#
                ),
            ),
            (
                "map",
                format!(
                    r#"<widgets><form-single name="F" map="({write}, {{v: 1}})"><field name="v"><default-field><display/>
                </default-field></field></form-single></widgets>"#
                ),
            ),
            (
                "text-map",
                format!(r#"<widgets><label text="L${{a}}" text-map="({write}, {{a: 1}})"/></widgets>"#),
            ),
            (
                "currency",
                format!(
                    r#"{rows}<widgets><form-single name="F"><field name="v"><default-field>
                <display currency-unit-field="({write}, 'USD')"/></default-field></field></form-single></widgets>"#
                ),
            ),
            (
                "report-embed",
                format!(r#"<widgets><report-embed definition-id="NONE_SUCH" variables="({write}, {{}})"/></widgets>"#),
            ),
        ];
        for (name, body) in &cases {
            let location = dir.join(format!("Inline{}.xml", name.replace('-', "")));
            let screen = format!(r#"<screen require-authentication="false">{body}</screen>"#);
            std::fs::write(&location, &screen).unwrap();
            let root = location.to_string_lossy().into_owned();
            let render = || {
                make_render(&world.factory)
                    .root_screen(root.clone())
                    .screen_path("")
                    .render_mode("html")
                    .as_request(true)
                    .render()
                    .map(|output| output.body_text())
                    .unwrap_or_else(|err| err.message().to_string())
            };
            let plain = render();
            assert!(!plain.contains(refused), "{name} from the file: {plain}");
            let mut overlay = crate::screen::overlay::Overlay::new();
            overlay.insert(&root, None, &screen);
            let previewed = crate::screen::overlay::with(overlay, render);
            assert!(previewed.contains(refused), "{name} in a preview: {previewed}");
            assert!(!previewed.contains("PWNED"), "{name} in a preview: {previewed}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: an inline expression of proposed text is never spliced into a render unit's code. Text that would close
/// the code around it (`0)) : 0) || EVIL || ((`, with or without a backtick) is refused when the screen loads, as it is
/// not one expression; text that is one expression and calls past the allow-list runs as a region and is refused there.
/// Each for a field's `hide`, a widget's `condition` and a form-single's `map`; nothing is written.
#[test]
fn a_previewed_screens_inline_text_is_not_spliced_into_code() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-splice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let evil = "sri.write('&lt;img src=x class=PWNED&gt;')";
    let rows = r#"<actions><script>rows = [{v: 1}]</script></actions>"#;
    let site = |name: &str, text: &str| -> String {
        match name {
            "hide" => format!(
                r#"{rows}<widgets><form-list name="F" list="rows" skip-form="true"><field name="v" hide="{text}">
                <default-field><display/></default-field></field></form-list><label text="MARK"/></widgets>"#
            ),
            "condition" => {
                format!(r#"<widgets><label text="MARK"/><label text="shown" condition="{text}"/></widgets>"#)
            }
            _ => format!(
                r#"<widgets><label text="MARK"/><form-single name="F" map="{text}"><field name="v"><default-field>
                <display/></default-field></field></form-single></widgets>"#
            ),
        }
    };
    for name in ["hide", "condition", "map"] {
        for (kind, text) in [
            ("breakout", format!("0)) : 0) || {evil} || ((")),
            ("backtick", format!("0)) : 0) || {evil} || ((`")),
            ("statements", format!("0); {evil}; (0")),
            ("unbalanced", format!("0), ({evil}, 0")),
            ("one-expression", format!("0 || {evil}")),
        ] {
            let location = dir.join(format!("Splice{name}{}.xml", kind.replace('-', "")));
            let screen = format!(
                r#"<screen require-authentication="false">{}</screen>"#,
                site(name, &text)
            );
            std::fs::write(&location, &screen).unwrap();
            let root = location.to_string_lossy().into_owned();
            let render = || {
                make_render(&world.factory)
                    .root_screen(root.clone())
                    .screen_path("")
                    .render_mode("html")
                    .render()
                    .map(|output| output.body_text())
                    .unwrap_or_else(|err| err.message().to_string())
            };
            let mut overlay = crate::screen::overlay::Overlay::new();
            overlay.insert(&root, None, &screen);
            let previewed = crate::screen::overlay::with(overlay, render);
            assert!(!previewed.contains("PWNED"), "{name} {kind} in a preview: {previewed}");
            if kind == "one-expression" {
                assert!(
                    previewed.contains("is not available in a preview"),
                    "{name} {kind} in a preview: {previewed}"
                );
            } else {
                assert!(
                    previewed.contains("is not one JavaScript expression"),
                    "{name} {kind} in a preview: {previewed}"
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: while a preview holds the runtime, a definition's actions snippet (a drop-down's `entity-options` find)
/// and every expansion, the trusted root screen's own among them, run on the region's allow-list, whoever's frame is
/// on top: a `sri.write` of raw markup from either is refused, and from the files alone it writes.
#[test]
fn a_previewed_render_runs_definition_finds_and_expansions_as_regions() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-entries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let root_text = r#"<screen require-authentication="false"><widgets>
        <label text="R${sri.write('&lt;i&gt;PWNEDROOT&lt;/i&gt;') || ''}"/><subscreens-active/></widgets></screen>"#;
    std::fs::write(dir.join("Root.xml"), root_text).unwrap();
    let child = dir.join("Root/Child.xml");
    let child_text = r#"<screen require-authentication="false"><widgets><form-single name="F"><field name="pick">
        <default-field><drop-down><entity-options key="${enumId}" text="${description}">
            <entity-find entity-name="moqui.basic.Enumeration">
                <econdition field-name="enumId" from="sri.write('&lt;i&gt;PWNEDOPT&lt;/i&gt;') || 'X'"/></entity-find>
        </entity-options></drop-down></default-field></field></form-single><label text="CHILD"/></widgets></screen>"#;
    std::fs::write(&child, child_text).unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("Child")
            .render_mode("html")
            .render()
            .map(|output| output.body_text())
            .unwrap_or_else(|err| err.message().to_string())
    };
    let plain = render();
    assert!(
        plain.contains("<i>PWNEDROOT</i>") && plain.contains("<i>PWNEDOPT</i>"),
        "from the files: {plain}"
    );
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&child.to_string_lossy(), None, child_text);
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(!previewed.contains("<i>PWNEDROOT"), "{previewed}");
    assert!(!previewed.contains("<i>PWNEDOPT"), "{previewed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: a preview's proposed text adds nothing to the page's script and stylesheet lists the shell writes as
/// `<script src>` and `<link>` (`footer_scripts`, `html_scripts`): by name, through `context` or `ec.context`, nor
/// sets one; the trusted root screen's own entries stay.
#[test]
fn a_previewed_screen_adds_no_script_to_the_page() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-scripts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    let root_text = r#"<screen require-authentication="false">
        <always-actions><script>footer_scripts = new Set(['/js/Own.js']); html_scripts = new Set()</script></always-actions>
        <widgets><subscreens-active/>
            <label text="SCRIPTS ${Array.from(footer_scripts).join(',')}|${Array.from(html_scripts).join(',')}"/></widgets>
    </screen>"#;
    std::fs::write(dir.join("Root.xml"), root_text).unwrap();
    let child = dir.join("Root/Child.xml");
    let child_text = r#"<screen require-authentication="false"><actions><script><![CDATA[
            const tried = []
            for (const [name, change] of [
                ['bare', () => footer_scripts.add('/evil/bare.js')],
                ['context', () => context.footer_scripts.add('/evil/context.js')],
                ['get', () => ec.context.get('footer_scripts').add('/evil/get.js')],
                ['html', () => html_scripts.add('/evil/html.js')],
                ['set', () => { footer_scripts = new Set(['/evil/set.js']) }],
                ['put', () => ec.context.put('html_scripts', new Set(['/evil/put.js']))],
            ]) {
                try { change(); tried.push(name) } catch (e) { tried.push(name + '!') }
            }
            out = tried.join(' ')
        ]]></script></actions><widgets><label text="CHILD ${out}"/></widgets></screen>"#;
    std::fs::write(&child, child_text).unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("Child")
            .render_mode("html")
            .render()
            .map(|output| output.body_text())
            .unwrap_or_else(|err| err.message().to_string())
    };
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&child.to_string_lossy(), None, child_text);
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(previewed.contains("CHILD "), "{previewed}");
    assert!(previewed.contains("SCRIPTS /js/Own.js|"), "{previewed}");
    assert!(!previewed.contains("/evil/"), "{previewed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: a name or a size a render unit writes into the page as it is (a `field-ref`'s name in its error note, a
/// panel's width) holds no markup of the proposed text in a preview, so the page gains no tag of its own (a
/// `<meta http-equiv="refresh">` that would take the viewer to another site).
#[test]
fn a_previewed_screen_writes_no_markup_of_a_name_or_size() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-asis-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let meta = "&lt;meta http-equiv=&quot;refresh&quot; content=&quot;0;url=https://evil.example/&quot;&gt;";
    let cases = [
        (
            "field-ref",
            format!(
                r#"<widgets><form-single name="F"><field name="v"><default-field><display text="x"/></default-field></field>
                <field-layout><field-ref name="{meta}"/></field-layout></form-single></widgets>"#
            ),
        ),
        (
            "panel",
            format!(
                r#"<widgets><container-panel id="P"><panel-left size="1&quot;&gt;{meta}"><label text="left"/></panel-left>
                <panel-center><label text="center"/></panel-center></container-panel></widgets>"#
            ),
        ),
    ];
    for (name, body) in &cases {
        let location = dir.join(format!("AsIs{}.xml", name.replace('-', "")));
        let screen = format!(r#"<screen require-authentication="false">{body}</screen>"#);
        std::fs::write(&location, &screen).unwrap();
        let root = location.to_string_lossy().into_owned();
        let render = || {
            make_render(&world.factory)
                .root_screen(root.clone())
                .screen_path("")
                .render_mode("html")
                .render()
                .map(|output| output.body_text())
                .unwrap_or_else(|err| err.message().to_string())
        };
        let mut overlay = crate::screen::overlay::Overlay::new();
        overlay.insert(&root, None, &screen);
        let previewed = crate::screen::overlay::with(overlay, render);
        assert!(!previewed.contains("<meta"), "{name} in a preview: {previewed}");
        assert!(
            previewed.contains("meta http-equiv"),
            "{name} in a preview renders: {previewed}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: a trusted screen whose form extends a proposed screen's form takes on its text: the form's row-actions
/// run on the preview's host there too.
#[test]
fn a_form_that_extends_a_proposed_form_runs_its_actions_locked() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-extends-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("Base.xml");
    let base_text = r#"<screen require-authentication="false"><widgets><form-list name="F" list="rows" skip-form="true">
        <row-actions><script>try { ec.cache.getCache('test.preview'); out = 'MARK ALLOWED' }
            catch (e) { out = String(e.message).includes('not available in a preview') ? 'MARK REFUSED' : 'MARK ALLOWED' }</script></row-actions>
        <field name="out"><default-field><display/></default-field></field></form-list></widgets></screen>"#;
    std::fs::write(&base, base_text).unwrap();
    let user = dir.join("User.xml");
    std::fs::write(
        &user,
        format!(
            r#"<screen require-authentication="false"><actions><script>rows = [{{v: 1}}]</script></actions>
            <widgets><form-list name="Mine" extends="{}#F" list="rows" skip-form="true"/></widgets></screen>"#,
            base.to_string_lossy()
        ),
    )
    .unwrap();
    let root = user.to_string_lossy().into_owned();
    let render = || {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode("text")
            .render()
            .map(|output| output.body_text())
            .unwrap_or_else(|err| err.message().to_string())
    };
    let plain = render();
    assert!(plain.contains("MARK ALLOWED"), "from the files: {plain}");
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&base.to_string_lossy(), None, base_text);
    let previewed = crate::screen::overlay::with(overlay, render);
    assert!(
        previewed.contains("MARK REFUSED"),
        "the extended form proposed: {previewed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: the menu data of a proposed path runs its screens' pre-actions under the lock, as a render does: the
/// screen is told by its frame on the artifact stack, not by the render's path index.
#[test]
fn the_menu_data_of_a_proposed_path_runs_its_actions_locked() {
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-menu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/App")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false">
    <transition name="menuData" read-only="true"><actions><script><![CDATA[
        const menuDataList = sri.getMenuData(sri.getScreenUrlInfo().extraPathNameList)
        if (menuDataList != null) ec.web.sendJsonResponse(menuDataList)
    ]]></script></actions><default-response type="none"/></transition>
    <subscreens default-item="App"/>
    <widgets><subscreens-active/></widgets>
</screen>"#,
    );
    write(
        "Root/App.xml",
        r#"<screen require-authentication="false" default-menu-title="App">
            <subscreens default-item="Prop"><subscreens-item name="Prop" location="{dir}/Root/App/Prop.xml"/></subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    // the pre-actions' outcome comes back as the menu title they set
    let proposed = r#"<screen require-authentication="false" default-menu-title="M ${probeOut}"><pre-actions><script>
        probeOut = 'ALLOWED'
        try { ec.user.internalLoginUser('nobody.here') } catch (e) { if (String(e.message).includes('not available in a preview')) probeOut = 'REFUSED' }
        ec.message.clearErrors()
        </script></pre-actions><widgets><label text="prop"/></widgets></screen>"#;
    write("Root/App/Prop.xml", proposed);
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    world.user("ada", None);
    let probe = || {
        let mut get = request("GET", "/menuData/App/Prop");
        get.set_header("authorization", super::basic("ada"));
        let body = serve_at(&world.factory, get, &root).body_text();
        let menu: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|err| panic!("{err}: {body}"));
        menu[0]["subscreens"][0]["title"]
            .as_str()
            .unwrap_or(&body)
            .trim_start_matches("M ")
            .to_string()
    };
    assert_eq!(probe(), "ALLOWED", "from the file");
    let mut overlay = crate::screen::overlay::Overlay::new();
    overlay.insert(&dir.join("Root/App/Prop.xml").to_string_lossy(), None, proposed);
    assert_eq!(crate::screen::overlay::with(overlay, probe), "REFUSED", "in a preview");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: raw SQL runs only in a service's own code, for a holder of SQL_RUNNER_WEB, or with authorization disabled
/// outside a render: a screen's actions and widgets reach no row past entity authorization by it.
#[test]
fn raw_sql_in_a_screen_needs_the_sql_runners_permission() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-raw-sql-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Sql.xml");
    std::fs::write(
        &location,
        r#"<screen require-authentication="false"><actions><script>
            try { ec.entity.runSqlStatement('transactional', 'select 1', 1); out = 'RAN' } catch (e) { out = 'REFUSED ' + e.message }
            ec.message.clearErrors()
        </script></actions><widgets><label text="MARK ${out}"/></widgets></screen>"#,
    )
    .unwrap();
    let out = make_render(&world.factory)
        .root_screen(location.to_string_lossy().into_owned())
        .screen_path("")
        .render_mode("text")
        .render()
        .unwrap()
        .body_text();
    assert!(out.contains("MARK REFUSED Raw SQL runs only"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-RTE-92: in an ordinary render a widget expression's `disableAuthz()` ends with the expression, so the widgets
/// after it are checked.
#[test]
fn a_widget_expressions_disable_ends_with_it() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-widget-authz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Widgets.xml");
    std::fs::write(
        &location,
        r#"<screen require-authentication="false"><widgets>
            <label text="FIRST ${ec.artifactExecution.disableAuthz()}"/>
            <section name="After" condition="!ec.artifactExecution.authzDisabled()"><widgets><label text="CHECKED"/></widgets>
                <fail-widgets><label text="UNCHECKED"/></fail-widgets></section>
        </widgets></screen>"#,
    )
    .unwrap();
    let out = make_render(&world.factory)
        .root_screen(location.to_string_lossy().into_owned())
        .screen_path("")
        .render_mode("text")
        .render()
        .unwrap()
        .body_text();
    assert!(
        out.contains("FIRST false") && out.contains("CHECKED") && !out.contains("UNCHECKED"),
        "{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-AI-102: a preview's response carries no `Refresh` header (a timed redirect to anywhere) and no Location to another
/// site, whatever its render set.
#[test]
fn a_preview_response_drops_a_refresh_and_a_redirect_to_another_site() {
    let _web = crate::web::install_with(Default::default(), "http://localhost:8080".to_string());
    crate::web::set_header("Refresh", Some("0; url=https://evil.example/phish"));
    crate::web::set_header("Location", Some("https://evil.example/steal"));
    crate::screen::render::preview_response();
    assert!(crate::web::response_header_values("Refresh").is_empty());
    assert!(crate::web::response_header_values("Location").is_empty());
    assert!(crate::web::response_header_values("Content-Security-Policy")
        .iter()
        .any(|policy| policy == crate::screen::render::PREVIEW_POLICY));
}

/// SEC-AI-102: a preview's response policy loads nothing from another site: images, styles' `url()`, fonts, fetches and
/// sockets are its own origin's (or a `data:` image or font), and only the app frames it.
#[test]
fn the_preview_policy_loads_nothing_from_another_site() {
    let policy = crate::screen::render::PREVIEW_POLICY;
    let directives: Vec<&str> = policy
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    for wanted in [
        "default-src 'self'",
        "img-src 'self' data:",
        "font-src 'self' data:",
        "frame-ancestors 'self'",
        "form-action 'self'",
    ] {
        assert!(directives.contains(&wanted), "{wanted}: {policy}");
    }
    for directive in &directives {
        for source in directive.split_whitespace().skip(1) {
            assert!(
                source.starts_with('\'') || source == "data:",
                "{directive} names a source of another site: {policy}"
            );
        }
    }
}

/// SEC-AI-82: while a screen stands at a preview's proposed text, its `encode="false"` labels, displays, links,
/// editables and button-menus are encoded in html and qvt (a qvt `{{ }}` as well), and a label's `type` is one of the
/// XSD's tags, overlay or not, so it is never markup of its own.
#[test]
fn a_previewed_screen_writes_its_unencoded_text_encoded_and_a_label_type_is_a_tag() {
    let world = World::open_in_framework("");
    let dir = std::env::temp_dir().join(format!("moqui-screen-overlay-raw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let location = dir.join("Raw.xml");
    let screen = r#"<screen require-authentication="false">
        <transition name="go"><default-response url="."/></transition>
        <actions><script>rows = [{v: 1}]</script></actions>
        <widgets>
            <label type="img src=x @error=TYPEMARK" text="typed"/>
            <label encode="false" text="L&lt;b class=&quot;RAWMARK&quot;&gt;x&lt;/b&gt;{{ 7*191 }}"/>
            <link url="go" text="K&lt;b class=&quot;RAWMARK&quot;&gt;x&lt;/b&gt;{{ 7*191 }}" encode="false"/>
            <editable text="E&lt;b class=&quot;RAWMARK&quot;&gt;x&lt;/b&gt;{{ 7*191 }}" transition="go" encode="false"/>
            <button-menu text="M&lt;b class=&quot;RAWMARK&quot;&gt;x&lt;/b&gt;{{ 7*191 }}" encode="false"><link url="go" text="item"/></button-menu>
            <form-list name="Rows" list="rows" skip-form="true"><field name="v"><default-field>
                <display text="D&lt;b class=&quot;RAWMARK&quot;&gt;x&lt;/b&gt;{{ 7*191 }}" encode="false"/></default-field></field></form-list>
        </widgets></screen>"#;
    std::fs::write(&location, screen).unwrap();
    let root = location.to_string_lossy().into_owned();
    let render = |mode: &str| {
        make_render(&world.factory)
            .root_screen(root.clone())
            .screen_path("")
            .render_mode(mode)
            .render()
            .unwrap()
            .body_text()
    };
    for mode in ["html", "qvt"] {
        let plain = render(mode);
        let previewed = {
            let mut overlay = crate::screen::overlay::Overlay::new();
            overlay.insert(&root, None, screen);
            crate::screen::overlay::with(overlay, || render(mode))
        };
        for out in [&plain, &previewed] {
            assert!(
                !out.contains("TYPEMARK") && !out.contains("<img src=x"),
                "{mode}: {out}"
            );
            assert!(out.contains(">typed</span>"), "{mode}: {out}");
        }
        // from the file, encode="false" is the screen's own markup
        for prefix in ["L", "K", "D", "M"] {
            assert!(
                plain.contains(&format!(r#"{prefix}<b class="RAWMARK">"#)),
                "{mode} {prefix}: {plain}"
            );
        }
        assert!(!previewed.contains(r#"<b class="RAWMARK">"#), "{mode}: {previewed}");
        // in qvt the text's braces are character references too, so Vue reads no interpolation in it
        if mode == "qvt" {
            for prefix in ["L", "K", "D", "M"] {
                assert!(
                    plain.contains(&format!("{prefix}<b class=\"RAWMARK\">x</b>{{{{ 7*191")),
                    "{prefix}: {plain}"
                );
                let encoded = format!("{prefix}&lt;b class=&quot;RAWMARK&quot;&gt;x&lt;/b&gt;&#123;&#123; 7*191");
                assert!(previewed.contains(&encoded), "{prefix}: {previewed}");
            }
        }
        for prefix in ["L", "K", "D", "M"] {
            assert!(
                previewed.contains(&format!("{prefix}&lt;b class=")),
                "{mode} {prefix}: {previewed}"
            );
        }
        // an editable's text: written in html, an attribute m-editable shows as markup in qvt
        let editable = if mode == "html" {
            r#"E<b class="RAWMARK">"#
        } else {
            "E&lt;b class=&quot;RAWMARK"
        };
        assert!(plain.contains(editable), "{mode}: {plain}");
        assert!(!previewed.contains(editable), "{mode}: {previewed}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
