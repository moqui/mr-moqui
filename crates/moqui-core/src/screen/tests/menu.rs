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

//! The menu data of the `menuData` transition (`sri.getMenuData`): the user-specific menu as JSON, with the
//! pre-actions of the screens on the path run for their titles, the subscreens the user may not view dropped, and a
//! subscreen that does not load logged and left out. And the menu images of the app list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{basic, request, World};
use crate::screen::serve_at;

fn screen_authz_conf() -> &'static str {
    "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_XML_SCREEN\" authz-enabled=\"true\"/></artifact-execution-facade></moqui-conf>"
}

/// A root screen with the `menuData` transition of webroot.xml.
const ROOT: &str = r#"<screen require-authentication="false">
    <transition name="menuData" read-only="true"><actions><script><![CDATA[
        const menuDataList = sri.getMenuData(sri.getScreenUrlInfo().extraPathNameList)
        if (menuDataList != null) ec.web.sendJsonResponse(menuDataList)
    ]]></script></actions><default-response type="none"/></transition>
    <subscreens default-item="App"/>
    <widgets><subscreens-active/></widgets>
</screen>"#;

/// [`ROOT`], an app with an open screen, a screen only the admin group may
/// view, and a screen that takes a required parameter. The open screen's pre-actions set the title its menu item shows.
fn tree(name: &str) -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("moqui-menu-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/App")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write("Root.xml", ROOT);
    write(
        "Root/App.xml",
        r#"<screen require-authentication="true" default-menu-title="The App">
            <subscreens default-item="Open">
                <subscreens-item name="Open" menu-title="${openTitle}" location="{dir}/Root/App/Open.xml"/>
                <subscreens-item name="Secret" menu-title="Secret" location="{dir}/Root/App/Secret.xml"/>
                <subscreens-item name="Needs" menu-title="Needs" location="{dir}/Root/App/Needs.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/App/Open.xml",
        r#"<screen require-authentication="true" default-menu-title="Open">
            <pre-actions><set field="openTitle" value="Open Sesame"/></pre-actions>
            <widgets><label text="open"/></widgets></screen>"#,
    );
    write(
        "Root/App/Secret.xml",
        r#"<screen require-authentication="true" default-menu-title="Secret"><widgets><label text="secret"/></widgets></screen>"#,
    );
    write(
        "Root/App/Needs.xml",
        r#"<screen require-authentication="true" default-menu-title="Needs"><parameter name="needId" required="true"/><widgets/></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    (dir, root)
}

/// Allow `group` to view the screen at `location`, in an artifact group of its own.
fn allow(world: &World, id: &str, group: &str, location: &str) {
    world.grant_screen(id, group, location, "AUTHZT_ALWAYS");
}

fn get_menu(world: &World, root: &str, user: Option<&str>, path: &str) -> (u16, serde_json::Value) {
    let mut get = request("GET", path);
    if let Some(user) = user {
        get.set_header("authorization", basic(user));
    }
    let response = serve_at(&world.factory, get, root);
    let json = serde_json::from_str(&response.body_text()).unwrap_or(serde_json::Value::Null);
    (response.status, json)
}

#[test]
fn menu_data_lists_what_the_user_may_view_and_runs_the_pre_actions_for_titles() {
    let (dir, root) = tree("view");
    let world = World::open(screen_authz_conf());
    world.user("admin", Some("ADMIN"));
    world.user("plain", None);
    // a grant on a screen reaches the screens below it, so the app itself has none
    allow(
        &world,
        "OPEN_VIEW",
        "ALL_USERS",
        &dir.join("Root/App/Open.xml").to_string_lossy(),
    );
    allow(
        &world,
        "SECRET_VIEW",
        "ADMIN",
        &dir.join("Root/App/Secret.xml").to_string_lossy(),
    );
    // the app's subscreens, with the ones the files add
    let names = |menu: &serde_json::Value| -> Vec<String> {
        menu[0]["subscreens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect()
    };

    let (status, menu) = get_menu(&world, &root, Some("plain"), "/menuData/App/Open");
    assert_eq!(status, 200, "{menu}");
    let items = names(&menu);
    assert!(items.contains(&"Open".to_string()), "{items:?}");
    assert!(!items.contains(&"Secret".to_string()), "{items:?}");
    assert!(
        !items.contains(&"Needs".to_string()),
        "a missing required parameter drops the item: {items:?}"
    );
    // the title was set by the pre-actions of the screens on the path (the first two, the root and the app, are skipped), and the last item is the target
    let open = menu[0]["subscreens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["name"] == "Open")
        .unwrap();
    assert_eq!(open["title"], "Open Sesame", "{menu}");
    assert_eq!(open["active"], true);
    assert_eq!(menu[0]["name"], "App");
    assert_eq!(menu[0]["title"], "The App");
    assert_eq!(menu[1]["name"], "Open");
    assert_eq!(menu[1]["path"], "/App/Open");

    let (_, admin) = get_menu(&world, &root, Some("admin"), "/menuData/App/Open");
    assert!(names(&admin).contains(&"Secret".to_string()), "{admin}");

    // a screen the user may not view is 403, nobody is 401, and no such screen is 404
    let (status, _) = get_menu(&world, &root, Some("plain"), "/menuData/App/Secret");
    assert_eq!(status, 403);
    let (status, _) = get_menu(&world, &root, None, "/menuData/App/Open");
    assert_eq!(status, 401);
    let (status, _) = get_menu(&world, &root, Some("admin"), "/menuData/App/Nothing");
    assert_eq!(status, 404);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The titles of the subscreens (the qapps tabs and drawer) are in the user's locale, as those of the screens on the path.
#[test]
fn menu_data_titles_are_in_the_users_locale() {
    let (dir, root) = tree("l10n");
    let world = World::open(screen_authz_conf());
    world.user("admin", Some("ADMIN"));
    for (id, file) in [
        ("OPEN_VIEW", "Root/App/Open.xml"),
        ("SECRET_VIEW", "Root/App/Secret.xml"),
    ] {
        allow(&world, id, "ADMIN", &dir.join(file).to_string_lossy());
    }
    let mut account = world
        .facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("userId", moqui_lib::object::Value::string("admin"))
        .one()
        .unwrap()
        .unwrap();
    account.set("locale", moqui_lib::object::Value::string("es_MX"));
    account.update(&world.facade).unwrap();
    for (original, localized) in [("Secret", "Secreto"), ("The App", "La Aplicación")] {
        let mut row = world.facade.make_value("moqui.basic.LocalizedMessage").unwrap();
        row.set("original", moqui_lib::object::Value::string(original));
        row.set("locale", moqui_lib::object::Value::string("es"));
        row.set("localized", moqui_lib::object::Value::string(localized));
        row.create(&world.facade).unwrap();
    }
    let (status, menu) = get_menu(&world, &root, Some("admin"), "/menuData/App/Open");
    assert_eq!(status, 200, "{menu}");
    let title = |name: &str| {
        menu[0]["subscreens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .map(|item| item["title"].clone())
    };
    assert_eq!(title("Secret").unwrap(), "Secreto", "{menu}");
    assert_eq!(
        title("Open").unwrap(),
        "Open Sesame",
        "expanded after the lookup: {menu}"
    );
    assert_eq!(menu[0]["title"], "La Aplicación");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A subscreen that does not parse, and one whose actions are not JavaScript, are logged by location and left out of the
/// menu data and of the html menu, and their siblings and the parent still render (AGENTS.md failure tolerance).
#[test]
fn a_subscreen_that_does_not_load_is_logged_and_left_out_of_the_menu() {
    let dir = std::env::temp_dir().join(format!("moqui-menu-broken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/App")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write("Root.xml", ROOT);
    write(
        "Root/App.xml",
        r#"<screen require-authentication="false" default-menu-title="The App">
            <subscreens default-item="Open">
                <subscreens-item name="Open" menu-title="Open" location="{dir}/Root/App/Open.xml"/>
                <subscreens-item name="Broken" menu-title="Broken" location="{dir}/Root/App/Broken.xml"/>
                <subscreens-item name="Script" menu-title="Script" location="{dir}/Root/App/Script.xml"/>
                <subscreens-item name="Other" menu-title="Other" location="{dir}/Root/App/Other.xml"/>
            </subscreens>
            <widgets><subscreens-menu/><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/App/Open.xml",
        r#"<screen require-authentication="false"><widgets><label text="open"/></widgets></screen>"#,
    );
    write(
        "Root/App/Broken.xml",
        r#"<screen require-authentication="false"><widgets><label text="broken"></widgets></screen>"#,
    );
    write(
        "Root/App/Script.xml",
        r#"<screen require-authentication="false"><actions><script>const x = ;</script></actions><widgets/></screen>"#,
    );
    write(
        "Root/App/Other.xml",
        r#"<screen require-authentication="false"><widgets><label text="other"/></widgets></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    world.user("plain", None);

    let lines = Arc::new(Mutex::new(Vec::new()));
    let (menu, page) = tracing::subscriber::with_default(crate::testing::LogSubscriber(Arc::clone(&lines)), || {
        let menu = get_menu(&world, &root, Some("plain"), "/menuData/App/Open");
        let mut get = request("GET", "/App/Open");
        get.set_header("authorization", basic("plain"));
        (menu, serve_at(&world.factory, get, &root))
    });
    let (status, menu) = menu;
    assert_eq!(status, 200, "{menu}");
    let names: Vec<&str> = menu[0]["subscreens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Open", "Other"], "{menu}");
    assert_eq!(menu[1]["name"], "Open");
    // the html menu leaves them out the same way, and the page renders
    assert_eq!(page.status, 200, "{}", page.body_text());
    let body = page.body_text();
    assert!(body.contains("open") && body.contains("/App/Other"), "{body}");
    assert!(!body.contains("/App/Broken"), "{body}");
    let logged = lines.lock().unwrap().join("\n");
    for name in ["Broken", "Script"] {
        let location = dir.join(format!("Root/App/{name}.xml")).to_string_lossy().into_owned();
        assert!(
            logged.contains(&format!("Leaving subscreen {name} [{location}]")),
            "{name} in {logged}"
        );
    }
    assert!(logged.contains("does not match"), "{logged}");
    assert!(logged.contains("not JavaScript"), "{logged}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The app list (`apps/AppList.xml`) draws an app's icon menu image as a Quasar icon (`q-icon`) in qvt and in the Material
/// Symbols font in html, and keeps an `fa fa-*` image as the classes of an `i` element. With no suite the apps are one
/// untitled group.
#[test]
fn the_app_list_draws_a_quasar_icon_name_and_keeps_fa_classes() {
    let app_list = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot/apps/AppList.xml")
        .to_string_lossy()
        .into_owned();
    let dir = std::env::temp_dir().join(format!("moqui-menu-app-list-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/apps")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| {
        std::fs::write(
            dir.join(path),
            text.replace("{dir}", &base).replace("{appList}", &app_list),
        )
        .unwrap()
    };
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><subscreens default-item="apps">
            <subscreens-item name="apps" location="{dir}/Root/apps.xml"/></subscreens><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/apps.xml",
        r#"<screen require-authentication="false"><subscreens default-item="AppList">
            <subscreens-item name="AppList" location="{appList}" menu-include="false"/>
            <subscreens-item name="Hub" menu-title="Hub" location="{dir}/Root/apps/Hub.xml"/>
            <subscreens-item name="Fa" menu-title="Fa" location="{dir}/Root/apps/Fa.xml"/>
        </subscreens><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/apps/Hub.xml",
        r#"<screen require-authentication="false" menu-image="sym_o_hub" menu-image-type="icon"><widgets/></screen>"#,
    );
    write(
        "Root/apps/Fa.xml",
        r#"<screen require-authentication="false" menu-image="fa fa-home" menu-image-type="icon"><widgets/></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    world.user("plain", None);
    let body = |path: &str| {
        let mut get = request("GET", path);
        get.set_header("authorization", basic("plain"));
        let response = serve_at(&world.factory, get, &root);
        assert_eq!(response.status, 200, "{path}: {}", response.body_text());
        response.body_text()
    };
    let qvt = body("/apps/AppList.qvt");
    assert!(
        qvt.contains(r#"<q-icon name="sym_o_hub"></q-icon><div class="m-app-tile-label">Hub</div>"#),
        "{qvt}"
    );
    assert!(
        qvt.contains(r#"<i class="fa fa-home m-app-tile-icon"></i><div class="m-app-tile-label">Fa</div>"#),
        "{qvt}"
    );
    super::qvt::assert_compiles(&["AppList".to_string()], std::slice::from_ref(&qvt));
    let html = body("/apps/AppList");
    assert!(
        html.contains(r#"<span class="material-symbols-outlined">hub</span><span>Hub</span>"#),
        "{html}"
    );
    assert!(html.contains(r#"<i class="fa fa-home"></i><span>Fa</span>"#), "{html}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The app list (`apps/AppList.xml`) lists only the apps the user may view, and a link drawn in a screen is disabled
/// when the user may not open its target. The webroot's seed data grants AppList to all users with an inheritable
/// record; a check made while it renders is decided on a fresh stack, as Java's `ScreenUrlInfo.isPermitted`, so that
/// grant does not reach the sibling apps: `Open` has a grant of its own and shows, `Secret` has none and stays out.
#[test]
fn a_menu_or_link_check_does_not_inherit_from_the_screen_rendering_it() {
    let app_list = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot/apps/AppList.xml")
        .to_string_lossy()
        .into_owned();
    let dir = std::env::temp_dir().join(format!("moqui-menu-app-view-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/apps")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| {
        std::fs::write(
            dir.join(path),
            text.replace("{dir}", &base).replace("{appList}", &app_list),
        )
        .unwrap()
    };
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><subscreens default-item="apps">
            <subscreens-item name="apps" location="{dir}/Root/apps.xml"/></subscreens><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/apps.xml",
        r#"<screen require-authentication="false"><subscreens default-item="AppList">
            <subscreens-item name="AppList" location="{appList}" menu-include="false"/>
            <subscreens-item name="Links" location="{dir}/Root/apps/Links.xml" menu-include="false"/>
            <subscreens-item name="Open" menu-title="Open" location="{dir}/Root/apps/Open.xml"/>
            <subscreens-item name="Secret" menu-title="Secret" location="{dir}/Root/apps/Secret.xml"/>
        </subscreens><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/apps/Links.xml",
        r#"<screen require-authentication="true"><widgets><render-mode><text type="html"><![CDATA[
            open=${sri.buildUrl("/apps/Open").getDisableLink()} secret=${sri.buildUrl("/apps/Secret").getDisableLink()}
        ]]></text></render-mode></widgets></screen>"#,
    );
    for name in ["Open", "Secret"] {
        write(
            &format!("Root/apps/{name}.xml"),
            r#"<screen require-authentication="true"><widgets><label text="app"/></widgets></screen>"#,
        );
    }
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open(screen_authz_conf());
    world.user("plain", Some("VIEWERS"));
    // AppList and Links as the seed data grants AppList; the Open app as an app grants itself; nothing for Secret.
    world.grant_screen("APPLIST_VIEW", "VIEWERS", &app_list, "AUTHZT_ALLOW");
    let location = |name: &str| dir.join(format!("Root/apps/{name}.xml")).to_string_lossy().into_owned();
    world.grant_screen("LINKS_VIEW", "VIEWERS", &location("Links"), "AUTHZT_ALLOW");
    world.grant_screen("OPEN_VIEW", "VIEWERS", &location("Open"), "AUTHZT_ALWAYS");
    let body = |path: &str| {
        let mut get = request("GET", path);
        get.set_header("authorization", basic("plain"));
        let response = serve_at(&world.factory, get, &root);
        assert_eq!(response.status, 200, "{path}: {}", response.body_text());
        response.body_text()
    };
    let list = body("/apps/AppList");
    assert!(
        list.contains("/apps/Open") && list.contains("<span>Open</span></a>"),
        "{list}"
    );
    assert!(!list.contains("Secret"), "{list}");
    let links = body("/apps/Links");
    assert!(links.contains("open=false secret=true"), "{links}");
    // A check is not a use, Java's isAccess: only opening the Secret app records a failure.
    let failures = || {
        world
            .facade
            .find("moqui.security.ArtifactAuthzFailure")
            .unwrap()
            .list()
            .unwrap()
            .len()
    };
    assert_eq!(failures(), 0);
    let mut get = request("GET", "/apps/Secret");
    get.set_header("authorization", basic("plain"));
    assert_eq!(serve_at(&world.factory, get, &root).status, 403);
    assert_eq!(failures(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A component adds a suite to the app list (`apps/AppList.xml`) with a `screen-extend` of it. The suite's template for the
/// render mode lays out the apps of its component, which do not show again among the other applications; a mode with no
/// template shows the suite as a titled group; a suite the user may open no app of is left out.
#[test]
fn a_suite_from_a_screen_extend_lays_out_the_apps_of_its_component() {
    let app_list = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot/apps/AppList.xml")
        .to_string_lossy()
        .into_owned();
    let dir = std::env::temp_dir().join(format!("moqui-menu-app-suite-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text.replace("{dir}", &base).replace("{appList}", &app_list)).unwrap()
    };
    write(
        "Root.xml",
        r#"<screen require-authentication="false"><subscreens default-item="apps">
            <subscreens-item name="apps" location="{dir}/Root/apps.xml"/></subscreens><widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Root/apps.xml",
        r#"<screen require-authentication="false"><subscreens default-item="AppList">
            <subscreens-item name="AppList" location="{appList}" menu-include="false"/>
            <subscreens-item name="Alpha" menu-title="Alpha" menu-index="1" location="component://suitetest/screen/Alpha.xml"/>
            <subscreens-item name="Beta" menu-title="Beta" menu-index="2" location="component://suitetest/screen/Beta.xml"/>
            <subscreens-item name="Plain" menu-title="Plain" menu-index="3" location="{dir}/Root/apps/Plain.xml"/>
        </subscreens><widgets><subscreens-active/></widgets></screen>"#,
    );
    for path in [
        "suitetest/screen/Alpha.xml",
        "suitetest/screen/Beta.xml",
        "Root/apps/Plain.xml",
    ] {
        write(
            path,
            r#"<screen require-authentication="false" menu-image="sym_o_hub" menu-image-type="icon"><widgets/></screen>"#,
        );
    }
    // The AppList here is a file, so its screen-extend is found by the path after /screen/.
    write(
        "suitetest/screen-extend/webroot/apps/AppList.xml",
        r#"<screen-extend><actions-extend when="append"><script><![CDATA[
            appSuites.push({name: 'Suite', title: 'Test Suite', component: 'suitetest', order: 10,
                templates: {html: 'component://suitetest/template/AppList.html.fancy', qvt: 'component://suitetest/template/AppList.qvt.fancy'}})
            appSuites.push({name: 'Empty', title: 'Empty Suite', component: 'nothing', order: 5})
        ]]></script></actions-extend></screen-extend>"#,
    );
    write(
        "suitetest/template/AppList.qvt.fancy",
        r#"<div class="suite">${html(suite.title)}: ${suite.items.map((item) => item.name + '=' + item.url + '@' + item.component).join(',')}</div>"#,
    );
    write(
        "suitetest/template/AppList.html.fancy",
        r#"<p data-suite>${suite.title} ${suite.items.length}</p>"#,
    );
    let component = crate::context::ComponentInfo {
        name: "suitetest".to_string(),
        version: String::new(),
        location: dir.join("suitetest"),
        depends_on: Vec::new(),
        purposes: Vec::new(),
    };
    let world = World::open_with(crate::context::ExecutionContextFactory::test_context_with_components(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
        vec![component],
    ));
    world.user("plain", None);
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let body = |path: &str| {
        let mut get = request("GET", path);
        get.set_header("authorization", basic("plain"));
        let response = serve_at(&world.factory, get, &root);
        assert_eq!(response.status, 200, "{path}: {}", response.body_text());
        response.body_text()
    };
    let qvt = body("/apps/AppList.qvt");
    assert!(
        qvt.contains(r#"<div class="suite">Test Suite: Alpha=/apps/Alpha@suitetest,Beta=/apps/Beta@suitetest</div>"#),
        "{qvt}"
    );
    assert!(qvt.contains("Other Applications"), "{qvt}");
    assert!(qvt.contains(r#"<div class="m-app-tile-label">Plain</div>"#), "{qvt}");
    assert!(!qvt.contains(r#"<div class="m-app-tile-label">Alpha</div>"#), "{qvt}");
    assert!(!qvt.contains("Empty Suite"), "{qvt}");
    super::qvt::assert_compiles(&["AppList".to_string()], std::slice::from_ref(&qvt));
    let html = body("/apps/AppList");
    assert!(html.contains("<p data-suite>Test Suite 2</p>"), "{html}");
    assert!(html.contains("<span>Plain</span></a>"), "{html}");
    // No tui template: the suite is a titled group of links.
    let tui = body("/apps/AppList.tui");
    for id in [
        "AppListSectionSuite",
        "AppListAppAlpha",
        "AppListAppBeta",
        "AppListSectionOther",
        "AppListAppPlain",
    ] {
        assert!(tui.contains(id), "{id} in {tui}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An app's menu item shows the app's own image: the image of its default subscreen,
/// which the walk reaches but the path does not name, does not replace it.
#[test]
fn menu_items_take_the_image_of_the_named_screen_not_its_default_subscreen() {
    let dir = std::env::temp_dir().join(format!("moqui-menu-image-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/Apps/App")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write(
        "Root.xml",
        r#"<screen require-authentication="false">
            <subscreens default-item="Apps"/>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/Apps.xml",
        r#"<screen require-authentication="false">
            <subscreens default-item="List">
                <subscreens-item name="List" menu-include="false" location="{dir}/Root/Apps/List.xml"/>
                <subscreens-item name="App" menu-title="App" location="{dir}/Root/Apps/App.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/Apps/List.xml",
        r#"<screen require-authentication="false"><widgets><render-mode><text type="html"><![CDATA[<#list sri.getMenuItems("/Apps") as item>[${item.name}:${item.image}:${item.imageType}]</#list>]]></text></render-mode></widgets></screen>"#,
    );
    write(
        "Root/Apps/App.xml",
        r#"<screen require-authentication="false" menu-image="fa fa-cogs" menu-image-type="icon">
            <subscreens default-item="Dash">
                <subscreens-item name="Dash" location="{dir}/Root/Apps/App/Dash.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/Apps/App/Dash.xml",
        r#"<screen require-authentication="false" menu-image="fa fa-tachometer" menu-image-type="icon"><widgets/></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    let response = serve_at(&world.factory, request("GET", "/Apps/List"), &root);
    let body = response.body_text();
    assert_eq!(response.status, 200, "{body}");
    assert!(body.contains("[App:fa fa-cogs:icon]"), "{body}");
    // A standalone request renders only its last screen, but a built URL resolves with `lastStandalone` 0,
    // so the image is the same.
    let mut standalone = request("GET", "/Apps/List");
    standalone.query.push(("lastStandalone".to_string(), "1".to_string()));
    let response = serve_at(&world.factory, standalone, &root);
    let body = response.body_text();
    assert_eq!(response.status, 200, "{body}");
    assert!(body.contains("[App:fa fa-cogs:icon]"), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The menu data's last entry has the image of the screen it names: at an app's root the target is the app's default
/// subscreen, and its own image shows rather than the app's, which the request's walk takes for it because the path does
/// not name the default. A screen with no image of its own still takes the app's.
#[test]
fn menu_data_takes_the_image_of_a_default_subscreen_at_an_apps_root() {
    let dir = std::env::temp_dir().join(format!("moqui-menu-default-image-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root/App")).unwrap();
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write("Root.xml", ROOT);
    write(
        "Root/App.xml",
        r#"<screen require-authentication="false" menu-image="sym_o_hub" menu-image-type="icon">
            <subscreens default-item="Home">
                <subscreens-item name="Home" menu-title="Home" location="{dir}/Root/App/Home.xml"/>
                <subscreens-item name="Plain" menu-title="Plain" location="{dir}/Root/App/Plain.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Root/App/Home.xml",
        r#"<screen require-authentication="false" menu-image="sym_o_dashboard" menu-image-type="icon"><widgets/></screen>"#,
    );
    write(
        "Root/App/Plain.xml",
        r#"<screen require-authentication="false"><widgets/></screen>"#,
    );
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    world.user("plain", None);

    let (status, menu) = get_menu(&world, &root, Some("plain"), "/menuData/App");
    assert_eq!(status, 200, "{menu}");
    assert_eq!(menu[0]["name"], "App");
    assert_eq!(menu[0]["image"], "sym_o_hub", "{menu}");
    assert_eq!(menu[1]["name"], "Home");
    assert_eq!(menu[1]["image"], "sym_o_dashboard", "{menu}");
    assert_eq!(menu[1]["imageType"], "icon");

    let (status, menu) = get_menu(&world, &root, Some("plain"), "/menuData/App/Plain");
    assert_eq!(status, 200, "{menu}");
    assert_eq!(menu[1]["name"], "Plain");
    assert_eq!(menu[1]["image"], "sym_o_hub", "{menu}");
    let _ = std::fs::remove_dir_all(&dir);
}
