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
    let context = ExecutionContextFactory::test_context();
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

    // The service-call shortcut becomes the first action with the defaults the Java screen sets.
    let service = root.transition("service", None).unwrap();
    assert_eq!(service.single_service.as_deref(), Some("moqui.Missing.noop"));
    let xml = &service.actions.as_ref().unwrap().xml;
    assert!(
        xml.contains("in-map=\"true\"") && xml.contains("multi=\"parameter\""),
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
fn a_transition_runs_its_path_in_one_transaction_and_keeps_the_last_response_that_holds() {
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
                <conditional-response url="First"><condition><expression>true</expression></condition></conditional-response>
                <conditional-response url="Second"><condition><expression>ran == 'y'</expression></condition></conditional-response>
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
    // Both conditional responses hold; Java keeps the last one.
    assert_eq!(location_of(&fine), Some("/Second"));

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
        r#"<screen require-authentication="false"><widgets><label text="mounted app"/></widgets></screen>"#,
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

#[test]
fn a_transition_that_writes_refuses_parameters_in_the_url() {
    let world = World::open("");
    let mut posted = request("POST", "/okTx");
    posted.query.push(("x".to_string(), "1".to_string()));
    let response = serve_at(&world.factory, posted, &fixture_root());
    assert_eq!(response.status, 400, "{}", response.body_text());
    assert!(
        response.body_text().contains("URL parameters"),
        "{}",
        response.body_text()
    );
    // A read-only transition takes them.
    let mut read = request("GET", "/goUrl");
    read.query.push(("x".to_string(), "1".to_string()));
    assert_eq!(serve_at(&world.factory, read, &fixture_root()).status, 302);
}
