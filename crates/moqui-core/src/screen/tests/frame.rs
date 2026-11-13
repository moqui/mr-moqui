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

//! The frame of a parent level (`screen/frame.rs`): a user whose non-inheritable grants open an area below an app root,
//! and none the root, gets the root's level as the frame of a page they may open, and nothing more.

use moqui_lib::object::Value;

use super::{basic, request, World};
use crate::screen::serve_at;

const ROOT: &str = r#"<screen require-authentication="false"><subscreens default-item="App"/><widgets><subscreens-active/></widgets></screen>"#;

/// An app root with always-actions that fail when `frameProbe` is set, so a frame that ran them would fail, and a label
/// of its own; an area with its own leaf, and a screen the area's users may not open.
fn tree() -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("moqui-frame-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/App/Time")).unwrap();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text).unwrap();
    write("Root.xml", ROOT);
    write(
        "Root/App.xml",
        r#"<screen default-menu-title="App"><subscreens default-item="Home"/>
            <always-actions><if condition="frameProbe"><then><script>throw new Error('App actions ran')</script></then></if></always-actions>
            <widgets><label text="App menu of its own"/><subscreens-panel id="app-panel" type="popup"/></widgets></screen>"#,
    );
    write(
        "Root/App/Home.xml",
        r#"<screen default-menu-title="Home"><widgets><label text="App home"/></widgets></screen>"#,
    );
    write(
        "Root/App/Secret.xml",
        r#"<screen default-menu-title="Secret"><widgets><label text="Secret"/></widgets></screen>"#,
    );
    write(
        "Root/App/Time.xml",
        r#"<screen default-menu-title="Time"><subscreens default-item="Leave"/>
            <widgets><label text="Time frame"/><subscreens-panel id="time-panel"/></widgets></screen>"#,
    );
    write(
        "Root/App/Time/Leave.xml",
        r#"<screen default-menu-title="Leave"><widgets><label text="Leave requests"/></widgets></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    (dir, root)
}

/// A view grant of `group` on the screen at `location`, inheritable by the screens below it or not.
fn grant(world: &World, id: &str, group: &str, location: &str, inherit: bool) {
    let mut member = world.facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
    member.set("artifactGroupId", Value::string(id));
    member.set("artifactName", Value::string(location));
    member.set("artifactTypeEnumId", Value::string("AT_XML_SCREEN"));
    member.set("nameIsPattern", Value::string("N"));
    member.set("inheritAuthz", Value::string(if inherit { "Y" } else { "N" }));
    member.create(&world.facade).unwrap();
    let mut authz = world.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
    authz.set("artifactAuthzId", Value::string(id));
    authz.set("userGroupId", Value::string(group));
    authz.set("artifactGroupId", Value::string(id));
    authz.set("authzTypeEnumId", Value::string("AUTHZT_ALLOW"));
    authz.set("authzActionEnumId", Value::string("AUTHZA_VIEW"));
    authz.create(&world.facade).unwrap();
}

#[test]
fn a_parent_level_is_the_frame_of_a_page_below_it_the_user_may_open() {
    let (dir, root) = tree();
    let world = World::open_in_framework(
        "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_XML_SCREEN\" authz-enabled=\"true\"/>\
         </artifact-execution-facade></moqui-conf>",
    );
    world.user("self", Some("SELF"));
    world.user("staff", Some("STAFF"));
    let at = |name: &str| dir.join(name).to_string_lossy().into_owned();
    grant(&world, "APP_STAFF", "STAFF", &at("Root/App.xml"), true);
    grant(&world, "TIME_SELF", "SELF", &at("Root/App/Time.xml"), false);
    grant(&world, "LEAVE_SELF", "SELF", &at("Root/App/Time/Leave.xml"), false);
    let get = |user: &str, path: &str, page: Option<&str>| {
        let mut get = request("GET", path);
        get.set_header("authorization", basic(user));
        if let Some(page) = page {
            get.set_header("X-Moqui-Page-Path", page);
        }
        serve_at(&world.factory, get, &root)
    };

    // the page itself: its parents are entered without their own check
    let leaf = get("self", "/App/Time/Leave.qvt", None);
    assert_eq!(leaf.status, 200, "{}", leaf.body_text());
    assert!(leaf.body_text().contains("Leave requests"), "{}", leaf.body_text());

    // the app root's level for that page: its placeholder alone, none of its actions run and nothing of its own written
    let frame = get("self", "/App.qvt?frameProbe=1", Some("/App/Time/Leave"));
    assert_eq!(frame.status, 200, "{}", frame.body_text());
    assert_eq!(
        frame.body_text(),
        r#"<div class="m-frame"><m-subscreens-active></m-subscreens-active></div>"#
    );
    assert_eq!(frame.header("X-Moqui-Qvt"), Some("merged-fragment"));
    assert_eq!(frame.header("Cache-Control"), Some("private, no-store"));
    // a level the user may open renders as itself, the header changing nothing
    let time = get("self", "/App/Time.qvt", Some("/App/Time/Leave"));
    assert_eq!(time.status, 200, "{}", time.body_text());
    assert!(
        time.body_text().contains("Time frame") && time.body_text().contains("<m-subscreens-tabs>"),
        "{}",
        time.body_text()
    );
    let staff = get("staff", "/App.qvt", Some("/App/Time/Leave"));
    assert_eq!(staff.status, 200, "{}", staff.body_text());
    assert!(
        staff.body_text().contains("App menu of its own"),
        "{}",
        staff.body_text()
    );

    // still refused: the root as a page of its own (no page, an html page, the root itself named), for a page the user
    // may not open, for a path that is not below it, and for one that does not resolve
    for (path, page) in [
        ("/App.qvt", None),
        ("/App", Some("/App/Time/Leave")),
        ("/App.qvt", Some("/App")),
        ("/App.qvt", Some("/App/Secret")),
        ("/App.qvt", Some("/App/Home")),
        ("/App/Secret.qvt", Some("/App/Time/Leave")),
        ("/App.qvt", Some("/App/Nowhere")),
        ("/App.qvt", Some("//Leave")),
    ] {
        let refused = get("self", path, page);
        assert_eq!(refused.status, 403, "{path} for {page:?}: {}", refused.body_text());
        assert!(!refused.body_text().contains("m-frame"), "{path} for {page:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_frame_keeps_the_tabs_of_a_tab_panel_and_never_the_steps_of_a_wizard() {
    let made = std::cell::Cell::new(0);
    let def = |widgets: &str| {
        made.set(made.get() + 1);
        let dir = std::env::temp_dir().join(format!("moqui-frame-text-{}-{}", std::process::id(), made.get()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("S.xml");
        std::fs::write(&file, format!("<screen><widgets>{widgets}</widgets></screen>")).unwrap();
        let context = crate::context::ExecutionContextFactory::test_context();
        let facade = crate::screen::ScreenFacade::of(context.factory());
        let def = facade.screen_def(context.factory(), &file.to_string_lossy()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        crate::screen::frame::frame_text(&def)
    };
    assert_eq!(
        def(r#"<container><label text="x"/><subscreens-panel id="p"/></container>"#),
        r#"<div class="m-frame"><m-subscreens-tabs></m-subscreens-tabs><m-subscreens-active></m-subscreens-active></div>"#
    );
    assert_eq!(
        def(r#"<subscreens-menu/><form-single name="F"/><subscreens-active/>"#),
        r#"<div class="m-frame"><m-subscreens-tabs></m-subscreens-tabs><m-subscreens-active></m-subscreens-active></div>"#
    );
    assert_eq!(
        def(r#"<subscreens-panel id="w" type="wizard" finish-text="Done"/>"#),
        r#"<div class="m-frame"><m-subscreens-active></m-subscreens-active></div>"#
    );
    assert_eq!(
        def(r#"<label text="no placeholder"/>"#),
        r#"<div class="m-frame"><m-subscreens-active></m-subscreens-active></div>"#
    );
}
