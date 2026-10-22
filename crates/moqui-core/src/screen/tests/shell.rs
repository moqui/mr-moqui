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

//! The suite shell's data (`shellData`, `sri.getShellData`), the palette search (`shellSearch`, `sri.getShellSearch`)
//! and `org.moqui.impl.ShellServices`: the apps the user may open with their menus and `<shell-providers>`, an app root
//! that fails logged and left out, the palette sources of those apps only, a failing source logged and left out, the
//! theme values of the user's qapps theme, and the counts sent on the `shellCounts` topic, coalesced per user.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::object::Value;
use serde_json::json;

use super::{basic, request, World};
use crate::screen::serve_at;

fn screen_authz_conf() -> &'static str {
    "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_XML_SCREEN\" authz-enabled=\"true\"/></artifact-execution-facade></moqui-conf>"
}

/// A root screen with the `shellData` and `shellSearch` transitions of webroot.xml, and its apps root.
const ROOT: &str = r#"<screen require-authentication="false">
    <transition name="shellData" read-only="true" begin-transaction="false"><actions><script><![CDATA[
        const shellData = sri.getShellData()
        if (shellData != null) ec.web.sendJsonResponse(shellData)
    ]]></script></actions><default-response type="none"/></transition>
    <transition name="shellProviders" read-only="true" begin-transaction="false"><actions><script><![CDATA[
        const kind = ec.web.parameters.kind
        // twice in one request: the second is the kept one
        const first = sri.getShellProviders(kind)
        const again = sri.getShellProviders(kind)
        ec.web.sendJsonResponse({first: first, same: JSON.stringify(first) === JSON.stringify(again)})
    ]]></script></actions><default-response type="none"/></transition>
    <transition name="savePreference"><actions><script><![CDATA[
        ec.user.setPreference('SHELL_DENSITY', 'compact')
        ec.user.setPreference('KEYMAP', '{"global": {"palette": "Mod+É"}}')
        ec.web.sendJsonResponse({saved: true})
    ]]></script></actions><default-response type="none"/></transition>
    <transition name="shellSearch" read-only="true" begin-transaction="false"><actions><script><![CDATA[
        const params = ec.web.parameters
        const found = sri.getShellSearch(params.text, params.groups, params.limit)
        if (found != null) ec.web.sendJsonResponse(found)
    ]]></script></actions><default-response type="none"/></transition>
    <subscreens default-item="apps"/>
    <widgets><subscreens-active/></widgets>
</screen>"#;

/// The palette sources, the today source and the counts services the providers and themes name.
const SERVICES: &str = r#"<services>
    <service verb="get" noun="Counts" authenticate="false">
        <in-parameters><parameter name="userId"/></in-parameters>
        <out-parameters><parameter name="counts" type="Map"/></out-parameters>
        <actions><script>counts = {today: 3, chat: 1, user: userId}</script></actions>
    </service>
    <service verb="get" noun="OtherCounts" authenticate="false">
        <in-parameters><parameter name="userId"/></in-parameters>
        <out-parameters><parameter name="counts" type="Map"/></out-parameters>
        <actions><script>counts = {other: true}</script></actions>
    </service>
    <service verb="get" noun="ZonedCounts">
        <description>authenticate and authorization as a Mantle service has them: no app's authorization covers it</description>
        <in-parameters><parameter name="userId"/></in-parameters>
        <out-parameters><parameter name="counts" type="Map"/></out-parameters>
        <actions><script>counts = {zone: ec.user.timeZone, locale: ec.user.locale, user: userId}</script></actions>
    </service>
    <service verb="get" noun="IdiomCounts" authenticate="false">
        <description>the Java idiom of Mantle's get#ShellCounts: off for the reads, on again after only if it was on</description>
        <in-parameters><parameter name="userId"/></in-parameters>
        <out-parameters><parameter name="counts" type="Map"/></out-parameters>
        <actions><script><![CDATA[
            const wasDisabled = ec.artifactExecution.disableAuthz()
            try { counts = {idiom: true, wasDisabled: wasDisabled} } finally { if (!wasDisabled) ec.artifactExecution.enableAuthz() }
        ]]></script></actions>
    </service>
    <service verb="get" noun="BadCounts" authenticate="false">
        <in-parameters><parameter name="userId"/></in-parameters>
        <out-parameters><parameter name="counts" type="Map"/></out-parameters>
        <actions><script>throw new Error('the counts are down')</script></actions>
    </service>
    <service verb="search" noun="Records" authenticate="false">
        <in-parameters><parameter name="text"/><parameter name="limit" type="Integer"/><parameter name="docTypes" type="List"/>
            <parameter name="idMatch" type="Boolean"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/><parameter name="moreUrl"/></out-parameters>
        <actions><script><![CDATA[
            rows = []
            for (let i = 0; i < 10; i++) rows.push({title: text + ' ' + i, subtitle: (docTypes || []).join(',') + (idMatch ? ' id' : ''),
                icon: 'sym_o_label', url: '/qapps/open/List?i=' + i})
            moreUrl = '/qapps/open/List?text=' + text
        ]]></script></actions>
    </service>
    <service verb="search" noun="Stack" authenticate="false">
        <in-parameters><parameter name="text"/><parameter name="limit" type="Integer"/><parameter name="docTypes" type="List"/>
            <parameter name="idMatch" type="Boolean"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/></out-parameters>
        <actions><script>rows = [{title: 'stack', subtitle: ec.artifactExecution.getStackNameString()}]</script></actions>
    </service>
    <service verb="search" noun="Ids" authenticate="false">
        <in-parameters><parameter name="text"/><parameter name="limit" type="Integer"/><parameter name="docTypes" type="List"/>
            <parameter name="idMatch" type="Boolean"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/></out-parameters>
        <actions><script>rows = [{title: 'Order ' + text, subtitle: idMatch ? 'id' : 'text', url: '/qapps/open/List?id=' + text}]</script></actions>
    </service>
    <service verb="search" noun="Fail" authenticate="false">
        <in-parameters><parameter name="text"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/></out-parameters>
        <actions><script>throw new Error('the source is down')</script></actions>
    </service>
    <service verb="search" noun="Empty" authenticate="false">
        <in-parameters><parameter name="text"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/></out-parameters>
        <actions><script>rows = []</script></actions>
    </service>
    <service verb="search" noun="Secret" authenticate="false">
        <in-parameters><parameter name="text"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/></out-parameters>
        <actions><script>rows = [{title: 'secret ' + text, url: '/qapps/secret'}]</script></actions>
    </service>
</services>"#;

/// [`ROOT`] and an apps root with: `open`, which everyone may view, with a menu three deep and every kind of provider;
/// `secret`, for the ADMIN group, with a palette source; `broken`, which does not parse; `needs`, which takes a parameter;
/// and `hidden`, out of the menu. `open` mounts a `Chatter` panel (a `.qvue` one, as BOSS's apps mount `Shared/Chatter`)
/// and no `RecordFiles`. The services are under `service/test`.
fn tree(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-shell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for path in ["Root", "Apps/Open/Sub/Deep1", "service/test"] {
        std::fs::create_dir_all(dir.join(path)).unwrap();
    }
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write("Root.xml", ROOT);
    write(
        "Root/apps.xml",
        r#"<screen require-authentication="false" default-menu-title="Applications">
            <subscreens default-item="open">
                <subscreens-item name="open" menu-title="Open App" menu-index="1" location="{dir}/Apps/Open.xml"/>
                <subscreens-item name="secret" menu-title="Secret App" menu-index="2" location="{dir}/Apps/Secret.xml"/>
                <subscreens-item name="broken" menu-title="Broken App" menu-index="3" location="{dir}/Apps/Broken.xml"/>
                <subscreens-item name="needs" menu-title="Needs App" menu-index="4" location="{dir}/Apps/Needs.xml"/>
                <subscreens-item name="hidden" menu-title="Hidden App" menu-index="5" menu-include="false" location="{dir}/Apps/Hidden.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Apps/Open.xml",
        r#"<screen require-authentication="true" default-menu-title="Open App" menu-image="sym_o_hub" menu-image-type="icon">
            <subscreens default-item="List">
                <subscreens-item name="List" menu-title="List" menu-index="1" location="{dir}/Apps/Open/List.xml"/>
                <subscreens-item name="Detail" menu-title="Detail" menu-index="2" location="{dir}/Apps/Open/Detail.xml"/>
                <subscreens-item name="Sub" menu-title="Sub" menu-index="3" location="{dir}/Apps/Open/Sub.xml"/>
                <subscreens-item name="QuickNote" menu-include="false" location="{dir}/Apps/Open/QuickNote.xml"/>
                <subscreens-item name="QuickCreate" menu-include="false" location="{dir}/Apps/Open/QuickCreate.xml"/>
                <subscreens-item name="Chatter" menu-include="false" location="{dir}/Apps/Open/Chatter.xml"/>
                <subscreens-item name="Record" menu-include="false" location="{dir}/Apps/Open/Record.xml"/>
            </subscreens>
            <shell-providers>
                <quick-create kind="note" label="Quick note" icon="sym_o_note_add" screen="open/QuickNote" sequence="20">
                    <parameter name="from" value="the shell"/></quick-create>
                <quick-create kind="task" label="Task" sequence="10"/>
                <quick-create kind="secret" label="Secret thing" screen="secret"/>
                <quick-create kind="lost" label="Lost" screen="open/NoSuchScreen"/>
                <quick-create kind="activity" label="Activity" requires-record="true" sequence="30"/>
                <today-source service="test.Shell.get#TodayItems" label="Open items"/>
                <palette-action label="Open the list" icon="sym_o_list" screen="open/List"/>
                <palette-type doc-type="OpenDoc"/>
                <palette-source group="Fail" label="Failing" service="test.Shell.search#Fail" sequence="10"/>
                <palette-source group="Records" label="Records" icon="sym_o_search" service="test.Shell.search#Records" prefix="~" sequence="20"/>
                <palette-source group="Ids" label="Id lookup" service="test.Shell.search#Ids" id-pattern="[A-Z]{2}[0-9]+" sequence="5"/>
                <palette-source group="Stack" service="test.Shell.search#Stack" id-pattern="STACK" sequence="40"/>
                <palette-source group="Empty" service="test.Shell.search#Empty" sequence="30"/>
                <notification-topic topic="OpenAlert"/>
                <keymap scope="open">
                    <key keys="g l" label="Go to the list" screen="open/List"/>
                    <key keys="c" label="Create" action="quickCreate"/>
                    <key keys="g s" label="Secret" screen="secret"/>
                </keymap>
                <account-menu-item label="Preferences" icon="sym_o_tune" screen="open/List"/>
                <frobnicate/>
            </shell-providers>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    let plain = |title: &str| {
        format!(
            r#"<screen require-authentication="true" default-menu-title="{title}"><widgets><label text="{title}"/></widgets></screen>"#
        )
    };
    write("Apps/Open/List.xml", &plain("List"));
    write("Apps/Open/QuickNote.xml", &plain("Quick note"));
    write("Apps/Open/QuickCreate.xml", &plain("Quick create"));
    write(
        "Apps/Open/Record.xml",
        r#"<screen require-authentication="true" default-menu-title="Record"><parameter name="recordId"/><widgets>
            <label text="a record"/>
            <chatter record-entity="test.Record" record-id="${recordId}"/>
            <record-files record-entity="test.Record" record-id="${recordId}"/>
        </widgets></screen>"#,
    );
    write(
        "Apps/Open/Chatter.xml",
        r#"<screen require-authentication="true" default-menu-title="Chatter" render-modes="vue,qvue"><widgets/></screen>"#,
    );
    write(
        "Apps/Open/Detail.xml",
        r#"<screen require-authentication="true" default-menu-title="Detail"><parameter name="detailId" required="true"/><widgets/></screen>"#,
    );
    write(
        "Apps/Open/Sub.xml",
        r#"<screen require-authentication="true" default-menu-title="Sub">
            <subscreens default-item="Deep1"><subscreens-item name="Deep1" menu-title="Deep one" location="{dir}/Apps/Open/Sub/Deep1.xml"/></subscreens>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    write(
        "Apps/Open/Sub/Deep1.xml",
        r#"<screen require-authentication="true" default-menu-title="Deep one">
            <subscreens default-item="Deep2"><subscreens-item name="Deep2" menu-title="Deep two" location="{dir}/Apps/Open/Sub/Deep1/Deep2.xml"/></subscreens>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    std::fs::create_dir_all(dir.join("Apps/Open/Sub/Deep1")).unwrap();
    write("Apps/Open/Sub/Deep1/Deep2.xml", &plain("Deep two"));
    write(
        "Apps/Secret.xml",
        r#"<screen require-authentication="true" default-menu-title="Secret App">
            <shell-providers>
                <palette-type doc-type="SecretDoc"/>
                <palette-source group="Secret" service="test.Shell.search#Secret"/>
            </shell-providers>
            <widgets><label text="secret"/></widgets></screen>"#,
    );
    write(
        "Apps/Broken.xml",
        r#"<screen require-authentication="false"><widgets><label text="broken"></widgets></screen>"#,
    );
    write(
        "Apps/Needs.xml",
        r#"<screen require-authentication="false"><parameter name="needId"/><widgets/></screen>"#,
    );
    write("Apps/Hidden.xml", &plain("Hidden"));
    write("service/test/Shell.xml", SERVICES);
    dir
}

fn root_of(dir: &std::path::Path) -> String {
    dir.join("Root.xml").to_string_lossy().into_owned()
}

/// Allow `group` to view the screen at `location`, in an artifact group of its own.
fn allow(world: &World, id: &str, group: &str, location: &str) {
    world.grant_screen(id, group, location, "AUTHZT_ALWAYS");
}

fn row(world: &World, entity: &str, fields: &[(&str, Value)]) {
    let mut value = world.facade.make_value(entity).unwrap();
    for (name, field) in fields {
        value.set(*name, field.clone());
    }
    value.create(&world.facade).unwrap();
}

fn theme_value(world: &World, theme: &str, sequence: i64, kind: &str, value: &str) {
    row(
        world,
        "moqui.screen.ScreenThemeResource",
        &[
            ("screenThemeId", Value::string(theme)),
            ("sequenceNum", Value::I64(sequence)),
            ("resourceTypeEnumId", Value::string(kind)),
            ("resourceValue", Value::string(value)),
        ],
    );
}

/// The users (plain; admin in ADMIN; other with a theme of its own; grouped in a group with a theme), the screen
/// grants, the themes and the services of [`tree`].
fn world(dir: &std::path::Path) -> World {
    world_with(dir, screen_authz_conf())
}

/// [`world`] with authorization enabled for screens and services, as the default configuration has it.
fn service_authz_world(dir: &std::path::Path) -> World {
    world_with(
        dir,
        "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_XML_SCREEN\" authz-enabled=\"true\"/>\
         <artifact-execution type=\"AT_SERVICE\" authz-enabled=\"true\"/></artifact-execution-facade></moqui-conf>",
    )
}

fn world_with(dir: &std::path::Path, conf: &str) -> World {
    let world = World::open(conf);
    world.user("plain", None);
    world.user("admin", Some("ADMIN"));
    world.user("other", None);
    world.user("grouped", Some("COUNTERS"));
    allow(
        &world,
        "OPEN_VIEW",
        "ALL_USERS",
        &dir.join("Apps/Open.xml").to_string_lossy(),
    );
    allow(
        &world,
        "SECRET_VIEW",
        "ADMIN",
        &dir.join("Apps/Secret.xml").to_string_lossy(),
    );
    // No ScreenTheme of the Quasar type is in this database, so a user with no theme of their own gets DEFAULT_QUASAR.
    theme_value(&world, "DEFAULT_QUASAR", 1, "STRT_TRAY_TODAY", "open/List");
    theme_value(&world, "DEFAULT_QUASAR", 2, "STRT_TRAY_CHAT", "secret");
    theme_value(&world, "DEFAULT_QUASAR", 3, "STRT_QUICK_CREATE", "open/QuickCreate");
    theme_value(&world, "DEFAULT_QUASAR", 4, "STRT_CHATTER_SCREEN", "Chatter");
    theme_value(&world, "DEFAULT_QUASAR", 6, "STRT_TUI_BREAKPOINTS", "md = 110");
    theme_value(&world, "DEFAULT_QUASAR", 7, "STRT_TUI_BREAKPOINTS", "lg = wide");
    theme_value(&world, "DEFAULT_QUASAR", 8, "STRT_TUI_COLORS", "primary = #1976d2");
    theme_value(&world, "DEFAULT_QUASAR", 9, "STRT_TUI_KEYS", "back = f3 | alt+left");
    theme_value(&world, "DEFAULT_QUASAR", 10, "STRT_TUI_KEYS", "refresh = f5");
    theme_value(&world, "DEFAULT_QUASAR", 11, "STRT_TUI_KEYS", "no equals sign");
    // The browser's brand colors are the terminal's too, under its own colors.
    theme_value(
        &world,
        "DEFAULT_QUASAR",
        12,
        "STRT_QUASAR_CONFIG",
        r##"{"brand":{"primary":"#1565c0","negative":"#c10015","dark":"#1d1d1d"},"notify":{}}"##,
    );
    theme_value(
        &world,
        "DEFAULT_QUASAR",
        5,
        "STRT_SHELL_COUNTS_SERVICE",
        "test.Shell.get#Counts",
    );
    theme_value(
        &world,
        "OTHER_QUASAR",
        1,
        "STRT_SHELL_COUNTS_SERVICE",
        "test.Shell.get#OtherCounts",
    );
    theme_value(
        &world,
        "GROUP_QUASAR",
        1,
        "STRT_SHELL_COUNTS_SERVICE",
        "test.Shell.get#BadCounts",
    );
    theme_value(
        &world,
        "ZONED_QUASAR",
        1,
        "STRT_SHELL_COUNTS_SERVICE",
        "test.Shell.get#ZonedCounts",
    );
    theme_value(
        &world,
        "MISSING_QUASAR",
        1,
        "STRT_SHELL_COUNTS_SERVICE",
        "test.Shell.get#NoSuchCounts",
    );
    row(
        &world,
        "moqui.security.UserScreenTheme",
        &[
            ("userId", Value::string("other")),
            ("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR")),
            ("screenThemeId", Value::string("OTHER_QUASAR")),
        ],
    );
    row(
        &world,
        "moqui.security.UserGroupScreenTheme",
        &[
            ("userGroupId", Value::string("COUNTERS")),
            ("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR")),
            ("screenThemeId", Value::string("GROUP_QUASAR")),
            ("sequenceNum", Value::I64(1)),
        ],
    );
    row(
        &world,
        "moqui.basic.LocalizedMessage",
        &[
            ("original", Value::string("Quick note")),
            ("locale", Value::string("en")),
            ("localized", Value::string("Quick note (en)")),
        ],
    );
    let services = [
        dir.join("service/test/Shell.xml"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/base-component/framework/service/org/moqui/impl/ShellServices.xml"),
    ];
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let failures = crate::service::lint(&services).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    world
}

fn get(world: &World, root: &str, user: Option<&str>, path: &str, query: &[(&str, &str)]) -> (u16, serde_json::Value) {
    let mut get = request("GET", path);
    for (name, value) in query {
        get.query.push(((*name).to_string(), (*value).to_string()));
    }
    if let Some(user) = user {
        get.set_header("authorization", basic(user));
    }
    let response = serve_at(&world.factory, get, root);
    let json = serde_json::from_str(&response.body_text()).unwrap_or(serde_json::Value::Null);
    (response.status, json)
}

fn names(list: &serde_json::Value, key: &str) -> Vec<String> {
    list.as_array()
        .unwrap_or_else(|| panic!("not a list: {list}"))
        .iter()
        .map(|item| item[key].as_str().unwrap_or("").to_string())
        .collect()
}

fn logged(lines: &Arc<Mutex<Vec<String>>>) -> String {
    lines.lock().unwrap().join("\n")
}

#[test]
fn shell_data_lists_the_apps_the_user_may_open_with_their_menus_providers_theme_values_and_counts() {
    let dir = tree("data");
    let world = world(&dir);
    let root = root_of(&dir);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let (status, data) = tracing::subscriber::with_default(crate::testing::LogSubscriber(Arc::clone(&lines)), || {
        get(&world, &root, Some("plain"), "/shellData", &[])
    });
    assert_eq!(status, 200, "{data}");
    // the app list's set and order: secret needs ADMIN, broken does not parse, needs takes a parameter, hidden is not in the menu
    assert_eq!(names(&data["apps"], "name"), ["open"], "{data}");
    let open = &data["apps"][0];
    assert_eq!(open["title"], "Open App");
    assert_eq!(open["image"], "sym_o_hub");
    assert_eq!(open["imageType"], "icon");
    assert_eq!(open["path"], "/qapps/open");
    // the menu to depth two: a screen with a required parameter, one out of the menu and the third level are not in it
    let menu: Vec<(String, String, i64)> = open["menu"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["title"].as_str().unwrap().to_string(),
                item["path"].as_str().unwrap().to_string(),
                item["depth"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        menu,
        [
            ("List".to_string(), "/qapps/open/List".to_string(), 1),
            ("Sub".to_string(), "/qapps/open/Sub".to_string(), 1),
            ("Deep one".to_string(), "/qapps/open/Sub/Deep1".to_string(), 2),
        ],
        "{data}"
    );

    let providers = &open["providers"];
    // in sequence order; a kind with no screen opens the theme's quick create screen for it; one the user may not open
    // and one to no screen are left out
    assert_eq!(
        providers["quickCreate"],
        json!([
            {"kind": "task", "label": "Task", "icon": null, "url": "/qapps/open/QuickCreate?kind=task", "requiresRecord": false, "sequence": 10},
            {"kind": "note", "label": "Quick note (en)", "icon": "sym_o_note_add", "url": "/qapps/open/QuickNote?from=the+shell",
                "requiresRecord": false, "sequence": 20},
            {"kind": "activity", "label": "Activity", "icon": null, "url": "/qapps/open/QuickCreate?kind=activity", "requiresRecord": true,
                "sequence": 30}
        ]),
        "{data}"
    );
    assert_eq!(
        providers["todaySources"],
        json!([{"service": "test.Shell.get#TodayItems", "label": "Open items"}])
    );
    assert_eq!(
        providers["paletteActions"],
        json!([{"label": "Open the list", "icon": "sym_o_list", "url": "/qapps/open/List", "sequence": null}])
    );
    assert_eq!(providers["paletteTypes"], json!(["OpenDoc"]));
    assert_eq!(
        providers["paletteSources"],
        json!([
            {"group": "Ids", "label": "Id lookup", "icon": null, "prefix": null, "idPattern": "[A-Z]{2}[0-9]+", "sequence": 5},
            {"group": "Fail", "label": "Failing", "icon": null, "prefix": null, "idPattern": null, "sequence": 10},
            {"group": "Records", "label": "Records", "icon": "sym_o_search", "prefix": "~", "idPattern": null, "sequence": 20},
            {"group": "Empty", "label": "Empty", "icon": null, "prefix": null, "idPattern": null, "sequence": 30},
            {"group": "Stack", "label": "Stack", "icon": null, "prefix": null, "idPattern": "STACK", "sequence": 40}
        ])
    );
    // the palette sources' services stay on the server
    assert!(!data.to_string().contains("test.Shell.search"), "{data}");
    // the record panels the theme names, as this app mounts them: Chatter with its render modes, no RecordFiles subscreen
    assert_eq!(
        open["recordScreens"],
        json!({"chatter": {"url": "/qapps/open/Chatter", "renderModes": ["vue", "qvue"]}, "recordFiles": null}),
        "{data}"
    );
    assert_eq!(providers["notificationTopics"], json!(["OpenAlert"]));
    assert_eq!(
        providers["keymaps"],
        json!([{"scope": "open", "keys": [
            {"keys": "g l", "label": "Go to the list", "url": "/qapps/open/List", "action": null},
            {"keys": "c", "label": "Create", "url": null, "action": "quickCreate"}
        ]}])
    );
    assert_eq!(
        providers["accountMenuItems"],
        json!([{"label": "Preferences", "icon": "sym_o_tune", "url": "/qapps/open/List", "sequence": null}])
    );
    // the theme values of DEFAULT_QUASAR: a screen the user may not open is null, a subscreen name is as written
    assert_eq!(
        data["shell"],
        json!({"trayToday": "/qapps/open/List", "trayChat": null, "trayNotifications": null, "quickCreate": "/qapps/open/QuickCreate",
            "mailScreen": null, "messagesScreen": null, "chatterScreen": "Chatter", "recordFilesScreen": null})
    );
    assert_eq!(data["counts"], json!({"today": 3, "chat": 1, "user": "plain"}));
    // the tui values: every key row, and the rows that do not read are left out and logged
    assert_eq!(
        data["tui"],
        json!({"breakpoints": {"md": 110}, "colors": {"primary": "#1976d2", "negative": "#c10015"},
            "keys": {"back": "f3 | alt+left", "refresh": "f5"}, "drawerOpen": null, "mouse": null})
    );
    let text = logged(&lines);
    assert!(
        text.contains("TUI breakpoint lg = wide is not a column count"),
        "{text}"
    );
    assert!(text.contains("[no equals sign] is not name = value"), "{text}");
    let broken = dir.join("Apps/Broken.xml").to_string_lossy().into_owned();
    assert!(
        text.contains(&format!("Leaving app broken [{broken}] out of the shell")),
        "{text}"
    );
    assert!(
        text.contains("Leaving quick-create open/NoSuchScreen of app open out of the shell"),
        "{text}"
    );
    assert!(text.contains("shell-providers has no element frobnicate"), "{text}");

    // ADMIN may open the secret app, and the providers and theme values that open it
    let (_, admin) = get(&world, &root, Some("admin"), "/shellData", &[]);
    assert_eq!(names(&admin["apps"], "name"), ["open", "secret"], "{admin}");
    assert_eq!(
        names(&admin["apps"][0]["providers"]["quickCreate"], "kind"),
        ["secret", "task", "note", "activity"]
    );
    assert_eq!(
        admin["apps"][0]["providers"]["keymaps"][0]["keys"][2]["url"],
        "/qapps/secret"
    );
    assert_eq!(admin["shell"]["trayChat"], "/qapps/secret");
    assert_eq!(admin["apps"][1]["providers"]["paletteTypes"], json!(["SecretDoc"]));
    // an app that mounts no record panel has none, and that is not an error
    assert_eq!(
        admin["apps"][1]["recordScreens"],
        json!({"chatter": null, "recordFiles": null})
    );

    // a user's own theme: its counts service and none of the default's values
    let (_, other) = get(&world, &root, Some("other"), "/shellData", &[]);
    assert_eq!(other["counts"], json!({"other": true}), "{other}");
    assert_eq!(other["shell"]["trayToday"], serde_json::Value::Null);
    // and no Chatter, since that theme names none
    assert_eq!(other["apps"][0]["recordScreens"]["chatter"], serde_json::Value::Null);
    assert_eq!(
        other["apps"][0]["providers"]["quickCreate"][0]["kind"], "note",
        "no quick create screen in this theme"
    );
    // a group's theme whose counts service fails: no counts, and the rest is there
    let (status, grouped) = get(&world, &root, Some("grouped"), "/shellData", &[]);
    assert_eq!(status, 200);
    assert_eq!(grouped["counts"], serde_json::Value::Null);
    assert_eq!(names(&grouped["apps"], "name"), ["open"]);

    let (status, _) = get(&world, &root, None, "/shellData", &[]);
    assert_eq!(status, 401);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `sri.getShellProviders(kind)` answers the providers of `getShellData` without the menus, record screens and counts, all
/// kinds or the one asked for, for the apps the user may open; a kind that is not one is an error. It is kept per session,
/// user, locale and theme: the second call of a request is the kept answer, a login drops it, and a changed app root
/// screen file is read again.
#[test]
fn shell_providers_are_the_providers_of_the_shell_data_alone_and_kept_per_session() {
    let dir = tree("providers");
    let world = world(&dir);
    let root = root_of(&dir);
    let (status, data) = get(&world, &root, Some("plain"), "/shellData", &[]);
    assert_eq!(status, 200, "{data}");
    let (status, answer) = get(&world, &root, Some("plain"), "/shellProviders", &[]);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["same"], true, "{answer}");
    let providers = &answer["first"];
    assert_eq!(names(&providers["apps"], "name"), ["open"], "{providers}");
    let open = &providers["apps"][0];
    assert_eq!(
        open["providers"], data["apps"][0]["providers"],
        "the same providers as the shell data"
    );
    assert_eq!(open["title"], "Open App");
    assert_eq!(open["path"], "/qapps/open");
    for left_out in ["menu", "recordScreens"] {
        assert!(open.get(left_out).is_none(), "{left_out} in {open}");
    }
    assert!(
        providers.get("counts").is_none() && providers.get("shell").is_none(),
        "{providers}"
    );
    // one kind
    let (_, answer) = get(
        &world,
        &root,
        Some("plain"),
        "/shellProviders",
        &[("kind", "todaySources")],
    );
    assert_eq!(
        answer["first"]["apps"][0]["providers"],
        json!({"todaySources": [{"service": "test.Shell.get#TodayItems", "label": "Open items"}]}),
        "{answer}"
    );
    // a login drops the session's: ADMIN gets the secret app too
    let (_, admin) = get(
        &world,
        &root,
        Some("admin"),
        "/shellProviders",
        &[("kind", "paletteTypes")],
    );
    assert_eq!(names(&admin["first"]["apps"], "name"), ["open", "secret"], "{admin}");
    assert_eq!(
        admin["first"]["apps"][1]["providers"],
        json!({"paletteTypes": ["SecretDoc"]})
    );
    let (status, _) = get(&world, &root, Some("plain"), "/shellProviders", &[("kind", "menus")]);
    assert_ne!(status, 200, "not a provider kind");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The kept providers of a key are used while the screens they were read from are current and dropped by a session's
/// prefix (a login or logout in it); a changed screen file is a miss.
#[test]
fn kept_shell_providers_follow_their_screens_and_the_session() {
    let dir = tree("providers-kept");
    let world = world(&dir);
    let facade = crate::screen::ScreenFacade::of(&world.factory);
    facade.set_check_interval(Some(std::time::Duration::ZERO));
    let location = dir.join("Apps/Open.xml").to_string_lossy().into_owned();
    let def = facade.screen_def(&world.factory, &location).unwrap();
    facade.put_shell_providers(
        "v1\u{1}plain".to_string(),
        json_value(json!({"apps": []})),
        vec![Arc::clone(&def)],
    );
    facade.put_shell_providers(
        "v2\u{1}plain".to_string(),
        json_value(json!({"apps": [1]})),
        vec![Arc::clone(&def)],
    );
    assert!(facade.cached_shell_providers(&world.factory, "v1\u{1}plain").is_some());
    facade.forget_shell_providers("v1\u{1}");
    assert!(facade.cached_shell_providers(&world.factory, "v1\u{1}plain").is_none());
    assert!(facade.cached_shell_providers(&world.factory, "v2\u{1}plain").is_some());
    // the app root screen changes: the kept providers are not used
    std::thread::sleep(std::time::Duration::from_millis(20));
    let text = std::fs::read_to_string(&location).unwrap();
    std::fs::write(&location, text.replace("Open items", "Open items now")).unwrap();
    let modified = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    let _ = std::fs::File::options()
        .write(true)
        .open(&location)
        .and_then(|file| file.set_modified(modified));
    assert!(facade.cached_shell_providers(&world.factory, "v2\u{1}plain").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A request that sets user preferences tells the page in its response: `X-Moqui-Preferences`, the JSON map of what it set,
/// percent-encoded as `encodeURIComponent`; a request that sets none has no such header.
#[test]
fn a_response_carries_the_preferences_its_request_set() {
    let dir = tree("preferences");
    let world = world(&dir);
    let root = root_of(&dir);
    let mut post = request("POST", "/savePreference");
    post.set_header("authorization", basic("plain"));
    post.set_header("user-agent", "GateAgent/1.0");
    post.set_header("accept-language", "de-DE,de;q=0.9");
    post.set_header("referer", "https://example.com/from");
    let response = serve_at(&world.factory, post, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    // the visit the login started records the request, as Java's
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let facade = world.factory.entity().unwrap();
        let visits = facade
            .find("moqui.server.Visit")
            .unwrap()
            .eq("userId", Value::string("plain"))
            .list()
            .unwrap();
        let visit = visits.iter().next().expect("a visit of the login");
        let text = |field: &str| {
            visit
                .get(field)
                .map(moqui_lib::object::to_plain_string)
                .unwrap_or_default()
        };
        assert_eq!(text("initialUserAgent"), "GateAgent/1.0");
        assert_eq!(text("initialLocale"), "de_DE");
        assert_eq!(text("initialReferrer"), "https://example.com/from");
        assert!(
            text("initialRequest").ends_with("/savePreference"),
            "{}",
            text("initialRequest")
        );
        assert_eq!(text("webappName"), "webroot");
    }
    let header = response
        .headers
        .iter()
        .find(|(name, _)| name == "X-Moqui-Preferences")
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| panic!("no preferences header: {:?}", response.headers));
    assert!(header.is_ascii() && !header.contains(['"', '{', ' ']), "{header}");
    let decoded = String::from_utf8(
        (0..header.len())
            .scan(0, |skip, at| {
                if *skip > 0 {
                    *skip -= 1;
                    return Some(None);
                }
                if header.as_bytes()[at] == b'%' {
                    *skip = 2;
                    Some(Some(u8::from_str_radix(&header[at + 1..at + 3], 16).unwrap()))
                } else {
                    Some(Some(header.as_bytes()[at]))
                }
            })
            .flatten()
            .collect(),
    )
    .unwrap();
    let map: serde_json::Value = serde_json::from_str(&decoded).unwrap();
    assert_eq!(
        map,
        json!({"KEYMAP": "{\"global\": {\"palette\": \"Mod+É\"}}", "SHELL_DENSITY": "compact"})
    );
    let mut again = request("GET", "/shellData");
    again.set_header("authorization", basic("plain"));
    let response = serve_at(&world.factory, again, &root);
    assert!(!response.headers.iter().any(|(name, _)| name == "X-Moqui-Preferences"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn json_value(json: serde_json::Value) -> Value {
    moqui_lib::json::from_json(&json)
}

/// In the html mode the chatter and record-files elements are a plain link to the subscreen of the current app root that the
/// html theme names, with the record, and nothing when the theme names none (record-files here) or there is no record.
#[test]
fn an_html_record_panel_is_a_link_to_the_theme_screen_of_the_app() {
    let dir = tree("html-panel");
    let world = world(&dir);
    row(
        &world,
        "moqui.security.UserScreenTheme",
        &[
            ("userId", Value::string("plain")),
            ("screenThemeTypeEnumId", Value::string("STT_INTERNAL")),
            ("screenThemeId", Value::string("PLAIN_HTML")),
        ],
    );
    theme_value(&world, "PLAIN_HTML", 1, "STRT_CHATTER_SCREEN", "Chatter");
    let root = root_of(&dir);
    let page = |query: &[(&str, &str)]| {
        let mut get = request("GET", "/apps/open/Record");
        for (name, value) in query {
            get.query.push(((*name).to_string(), (*value).to_string()));
        }
        get.set_header("authorization", basic("plain"));
        let response = serve_at(&world.factory, get, &root);
        assert_eq!(response.status, 200, "{}", response.body_text());
        response.body_text()
    };
    let text = page(&[("recordId", "R 1")]);
    assert!(
        text.contains(r#"<a href="/apps/open/Chatter?recordEntity=test.Record&amp;recordId=R+1" class="record-panel-link">Chatter</a>"#),
        "{text}"
    );
    assert!(!text.contains("RecordFiles"), "{text}");
    let none = page(&[]);
    assert!(
        none.contains("a record") && !none.contains("record-panel-link"),
        "{none}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shell_search_runs_only_the_sources_of_the_apps_the_user_may_open() {
    let dir = tree("search");
    let world = world(&dir);
    let root = root_of(&dir);
    let search = |user: Option<&str>, query: &[(&str, &str)]| get(&world, &root, user, "/shellSearch", query);

    let lines = Arc::new(Mutex::new(Vec::new()));
    let (status, found) = tracing::subscriber::with_default(crate::testing::LogSubscriber(Arc::clone(&lines)), || {
        search(Some("plain"), &[("text", "abc"), ("limit", "3")])
    });
    assert_eq!(status, 200, "{found}");
    // Ids needs an id, Fail fails (logged), Empty answers nothing, and Secret is of an app plain may not open
    assert_eq!(names(&found["groups"], "group"), ["Records"], "{found}");
    let records = &found["groups"][0];
    assert_eq!(records["label"], "Records");
    assert_eq!(records["icon"], "sym_o_search");
    assert_eq!(records["prefix"], "~");
    assert_eq!(records["moreUrl"], "/qapps/open/List?text=abc");
    assert_eq!(records["rows"].as_array().unwrap().len(), 3, "{found}");
    assert_eq!(
        records["rows"][0],
        json!({"title": "abc 0", "subtitle": "OpenDoc", "icon": "sym_o_label", "url": "/qapps/open/List?i=0"}),
        "the document types of the apps the user may open, and no idMatch"
    );
    let text = logged(&lines);
    assert!(
        text.contains("Leaving palette group Fail (test.Shell.search#Fail) of app open out of the search"),
        "{text}"
    );
    assert!(text.contains("the source is down"), "{text}");

    // ADMIN runs the secret app's source too, first by sequence, and the document types of both apps go to each source
    let (_, admin) = search(Some("admin"), &[("text", "abc")]);
    assert_eq!(names(&admin["groups"], "group"), ["Secret", "Records"], "{admin}");
    assert_eq!(
        admin["groups"][1]["rows"].as_array().unwrap().len(),
        5,
        "five rows by default"
    );
    assert_eq!(admin["groups"][1]["rows"][0]["subtitle"], "OpenDoc,SecretDoc");
    assert_eq!(admin["groups"][0]["label"], "Secret", "the group name without a label");

    // an id runs the id-pattern source, with idMatch
    let (_, ids) = search(Some("plain"), &[("text", "SO123")]);
    assert_eq!(names(&ids["groups"], "group"), ["Ids", "Records"], "{ids}");
    assert_eq!(ids["groups"][0]["rows"][0]["subtitle"], "id");
    assert_eq!(ids["groups"][1]["rows"][0]["subtitle"], "OpenDoc");

    // a source runs as the app root that registered it, so that app's authorization record and its entity filters apply
    let (_, stack) = search(Some("plain"), &[("text", "STACK")]);
    assert_eq!(names(&stack["groups"], "group"), ["Records", "Stack"], "{stack}");
    let frames = stack["groups"][1]["rows"][0]["subtitle"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        frames.contains("Apps/Open.xml"),
        "the app root is on the stack under the source: {frames}"
    );

    // groups narrows the sources, and names only the whitelisted ones: never another app's or a service
    let (_, narrowed) = search(Some("plain"), &[("text", "SO123"), ("groups", "Records, Secret")]);
    assert_eq!(names(&narrowed["groups"], "group"), ["Records"], "{narrowed}");
    let (_, service) = search(
        Some("plain"),
        &[("text", "abc"), ("groups", "test.Shell.search#Secret")],
    );
    assert_eq!(service["groups"], json!([]), "{service}");
    let (_, empty) = search(Some("plain"), &[("text", "  ")]);
    assert_eq!(empty["groups"], json!([]));
    let (status, _) = search(None, &[("text", "abc")]);
    assert_eq!(status, 401);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn shell_counts_come_from_the_service_of_the_users_qapps_theme_and_a_failing_one_is_logged_once() {
    let dir = tree("counts");
    let world = world(&dir);
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let counts = |user: &str| crate::service::shell::shell_counts(user).map(Value::Map);
    // DEFAULT_QUASAR without a theme of the user's; a user's own; none without the theme value
    assert_eq!(
        counts("plain").map(|value| moqui_lib::json::to_json(&value)),
        Some(json!({"today": 3, "chat": 1, "user": "plain"}))
    );
    assert_eq!(
        counts("other").map(|value| moqui_lib::json::to_json(&value)),
        Some(json!({"other": true}))
    );
    {
        let _authz = ctx.disable_authz();
        let mut theme = world.facade.make_value("moqui.security.UserScreenTheme").unwrap();
        theme.set("userId", Value::string("admin"));
        theme.set("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR"));
        theme.set("screenThemeId", Value::string("EMPTY_QUASAR"));
        theme.create(&world.facade).unwrap();
    }
    let lines = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(crate::testing::LogSubscriber(Arc::clone(&lines)), || {
        assert_eq!(counts("admin"), None, "no counts service in the theme");
        // a failing service and a missing one: no counts, each logged once however often it is asked
        for _ in 0..3 {
            assert_eq!(counts("grouped"), None);
        }
        let mut theme = world.facade.make_value("moqui.security.UserScreenTheme").unwrap();
        theme.set("userId", Value::string("plain"));
        theme.set("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR"));
        theme.set("screenThemeId", Value::string("MISSING_QUASAR"));
        let _authz = ctx.disable_authz();
        theme.create(&world.facade).unwrap();
        assert_eq!(counts("plain"), None);
        assert_eq!(counts("plain"), None);
    });
    // (the service call logs the error of its body each time, as any failing service's)
    let text = logged(&lines);
    let shell_line = |service: &str| format!("The shell counts service {service}");
    assert_eq!(
        text.matches(&shell_line("test.Shell.get#BadCounts")).count(),
        1,
        "{text}"
    );
    assert!(text.contains("the counts are down"), "{text}");
    assert_eq!(
        text.matches(&shell_line("test.Shell.get#NoSuchCounts")).count(),
        1,
        "{text}"
    );
    assert!(
        ctx.message().errors().is_empty(),
        "the failures are not the caller's errors"
    );

    // the service: the current user by default, another user's counts only for internal work
    crate::user::internal_login_user("other").unwrap();
    let out = crate::service::sync("org.moqui.impl.ShellServices.get#ShellCounts", &BTreeMap::new()).unwrap();
    assert_eq!(
        out.get("counts").map(moqui_lib::json::to_json),
        Some(json!({"other": true}))
    );
    let theirs = BTreeMap::from([("userId".to_string(), Value::string("grouped"))]);
    let out = crate::service::sync("org.moqui.impl.ShellServices.get#ShellCounts", &theirs).unwrap();
    assert!(!out.contains_key("counts"), "{out:?}");
    assert!(
        ctx.message().errors().join("\n").contains("another user's counts"),
        "{:?}",
        ctx.message().errors()
    );
    ctx.message().clear_errors();
    let _authz = ctx.disable_authz();
    let out = crate::service::sync("org.moqui.impl.ShellServices.get#ShellCounts", &theirs).unwrap();
    assert_eq!(out.get("counts"), Some(&Value::Null), "the group's service fails");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `shellCounts` transition of webroot.xml (the shell's poll while the socket is down), as that file writes it, under
/// the test root: 401 without a user, else `{counts}` of the user's theme service, null without one.
#[test]
fn the_shell_counts_transition_answers_the_counts_of_the_current_user() {
    let dir = tree("counts-transition");
    let webroot = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/webroot/screen/webroot.xml"),
    )
    .unwrap();
    let start = webroot
        .find("<transition name=\"shellCounts\"")
        .expect("webroot.xml has shellCounts");
    let end = start + webroot[start..].find("</transition>").unwrap() + "</transition>".len();
    let root_text = ROOT.replacen(
        "<subscreens default-item",
        &format!("{}\n    <subscreens default-item", &webroot[start..end]),
        1,
    );
    std::fs::write(dir.join("Root.xml"), root_text).unwrap();
    let world = world(&dir);
    let root = root_of(&dir);
    let (status, _) = get(&world, &root, None, "/shellCounts", &[]);
    assert_eq!(status, 401);
    let (status, answer) = get(&world, &root, Some("plain"), "/shellCounts", &[]);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer, json!({"counts": {"today": 3, "chat": 1, "user": "plain"}}));
    let (_, answer) = get(&world, &root, Some("other"), "/shellCounts", &[]);
    assert_eq!(answer, json!({"counts": {"other": true}}));
    {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        row(
            &world,
            "moqui.security.UserScreenTheme",
            &[
                ("userId", Value::string("admin")),
                ("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR")),
                ("screenThemeId", Value::string("EMPTY_QUASAR")),
            ],
        );
    }
    let (status, answer) = get(&world, &root, Some("admin"), "/shellCounts", &[]);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer, json!({"counts": null}), "no counts service in the theme");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A user whose qapps theme names `test.Shell.get#ZonedCounts`, with a time zone and a locale of their own.
fn zoned_user(world: &World, user: &str) {
    world.user(user, None);
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut account = world.facade.make_value("moqui.security.UserAccount").unwrap();
    account.set("userId", Value::string(user));
    account.set("timeZone", Value::string("America/Chicago"));
    account.set("locale", Value::string("de"));
    account.update(&world.facade).unwrap();
    row(
        world,
        "moqui.security.UserScreenTheme",
        &[
            ("userId", Value::string(user)),
            ("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR")),
            ("screenThemeId", Value::string("ZONED_QUASAR")),
        ],
    );
}

/// The counts service a theme names is trusted data and reads only that user's rows, and no app's authorization covers
/// it (a Mantle service under the webroot transitions): it runs for the current user without service authorization, in
/// shellData and the shellCounts transition, and the user's own time zone and locale apply. Another user's counts stay
/// refused to a caller with authorization on.
#[test]
fn the_counts_service_runs_for_the_current_user_without_service_authorization_in_the_users_zone() {
    let dir = tree("counts-authz");
    let webroot = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/webroot/screen/webroot.xml"),
    )
    .unwrap();
    let start = webroot.find("<transition name=\"shellCounts\"").unwrap();
    let end = start + webroot[start..].find("</transition>").unwrap() + "</transition>".len();
    let root_text = ROOT.replacen(
        "<subscreens default-item",
        &format!("{}\n    <subscreens default-item", &webroot[start..end]),
        1,
    );
    std::fs::write(dir.join("Root.xml"), root_text).unwrap();
    let world = service_authz_world(&dir);
    zoned_user(&world, "zoned");
    let root = root_of(&dir);
    let expected = json!({"zone": "America/Chicago", "locale": "de", "user": "zoned"});
    let (status, answer) = get(&world, &root, Some("zoned"), "/shellCounts", &[]);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer, json!({"counts": expected}));
    let (status, data) = get(&world, &root, Some("zoned"), "/shellData", &[]);
    assert_eq!(status, 200, "{data}");
    assert_eq!(data["counts"], expected, "{data}");

    // the service itself is refused to the user outside the shell, and get#ShellCounts still refuses another user's counts
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("zoned").unwrap();
    let direct = BTreeMap::from([("userId".to_string(), Value::string("zoned"))]);
    let err = crate::service::sync("test.Shell.get#ZonedCounts", &direct).unwrap_err();
    assert!(err.to_string().contains("not authorized"), "{err}");
    let theirs = BTreeMap::from([("userId".to_string(), Value::string("plain"))]);
    let err = crate::service::sync("org.moqui.impl.ShellServices.get#ShellCounts", &theirs).unwrap_err();
    assert!(err.to_string().contains("not authorized"), "{err}");
    ctx.message().clear_errors();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The shellCounts messages for `user` the topic carried so far.
fn sent_counts(seen: &Arc<Mutex<Vec<serde_json::Value>>>, user: &str) -> Vec<serde_json::Value> {
    seen.lock()
        .unwrap()
        .iter()
        .filter(|body| {
            body["topic"] == "shellCounts" && body["userIds"].as_array().is_some_and(|ids| ids.contains(&json!(user)))
        })
        .cloned()
        .collect()
}

fn wait_for(seen: &Arc<Mutex<Vec<serde_json::Value>>>, user: &str, count: usize, limit: Duration) -> usize {
    let start = Instant::now();
    loop {
        let found = sent_counts(seen, user).len();
        if found >= count || start.elapsed() > limit {
            return found;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A factory that has begun to shut down sends no counts, though a socket listens: its database is closing.
#[test]
fn publish_sends_nothing_once_the_factory_shuts_down() {
    let dir = tree("publish-closing");
    let world = world(&dir);
    let user = format!("closing{}", std::process::id());
    world.user(&user, None);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    let _subscription = crate::notify::subscribe(
        crate::notify::NOTIFICATION_MESSAGE,
        Arc::new(move |message| keep.lock().unwrap().push(message.body.clone())),
    )
    .unwrap();
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    assert!(!world.factory.is_shutting_down());
    world.factory.shutdown();
    assert!(world.factory.is_shutting_down());
    let parameters = BTreeMap::from([("userIdList".to_string(), Value::string(&user))]);
    crate::service::sync("org.moqui.impl.ShellServices.publish#ShellCounts", &parameters).unwrap();
    assert_eq!(wait_for(&seen, &user, 1, Duration::from_millis(600)), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn publish_sends_the_counts_on_the_shell_counts_topic_once_per_window_and_after_the_commit() {
    let dir = tree("publish");
    let world = world(&dir);
    let user = format!("counted{}", std::process::id());
    world.user(&user, None);
    crate::service::shell::set_window(&world.factory, Duration::from_millis(400));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    let _subscription = crate::notify::subscribe(
        crate::notify::NOTIFICATION_MESSAGE,
        Arc::new(move |message| keep.lock().unwrap().push(message.body.clone())),
    )
    .unwrap();
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let publish = || {
        let parameters = BTreeMap::from([(
            "userIdList".to_string(),
            Value::List(vec![Value::string(&user), Value::string(&user)]),
        )]);
        let started = Instant::now();
        crate::service::sync("org.moqui.impl.ShellServices.publish#ShellCounts", &parameters).unwrap();
        // it does not wait for the counts
        assert!(started.elapsed() < Duration::from_millis(300));
    };

    publish();
    assert_eq!(wait_for(&seen, &user, 1, Duration::from_secs(5)), 1);
    let first = &sent_counts(&seen, &user)[0];
    assert_eq!(
        first["message"],
        json!({"today": 3, "chat": 1, "user": user}),
        "{first}"
    );
    assert_eq!(first["showAlert"], false, "{first}");
    assert_eq!(
        first["notificationMessageId"],
        serde_json::Value::Null,
        "not persisted: {first}"
    );
    // three more inside the window are one send at its end
    publish();
    publish();
    publish();
    assert_eq!(wait_for(&seen, &user, 2, Duration::from_secs(5)), 2);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(sent_counts(&seen, &user).len(), 2, "the folded requests are one send");

    // in a transaction it waits for the commit, and a rollback sends nothing
    let tx = world.facade.default_transactions().unwrap();
    assert!(tx.begin(None).unwrap());
    publish();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(sent_counts(&seen, &user).len(), 2, "nothing before the commit");
    tx.rollback().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(sent_counts(&seen, &user).len(), 2, "nothing after a rollback");
    assert!(tx.begin(None).unwrap());
    publish();
    tx.commit().unwrap();
    assert_eq!(wait_for(&seen, &user, 3, Duration::from_secs(5)), 3);

    // the sender has no session: the user's time zone and locale are set from the account, so "today" is the user's day
    let zoned = format!("zoned{}", std::process::id());
    zoned_user(&world, &zoned);
    let parameters = BTreeMap::from([("userIdList".to_string(), Value::string(&zoned))]);
    crate::service::sync("org.moqui.impl.ShellServices.publish#ShellCounts", &parameters).unwrap();
    assert_eq!(wait_for(&seen, &zoned, 1, Duration::from_secs(5)), 1);
    assert_eq!(
        sent_counts(&seen, &zoned)[0]["message"],
        json!({"zone": "America/Chicago", "locale": "de", "user": zoned})
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The terminal keeps F9's drawer choice as the `TUI_DRAWER_OPEN` preference through webroot's `/apps/setPreference`
/// (asked for as JSON, it answers 200 with an empty body and the `X-Moqui-Preferences` header), and the shell data gives
/// it back as `tui.drawerOpen`: null until it is set. Alt+P's mouse choice, `TUI_MOUSE`, is `tui.mouse` the same way.
#[test]
fn the_drawer_choice_is_a_preference_the_shell_data_gives_back() {
    let world = World::open("");
    world.user("plain", None);
    let root = super::webroot().to_string_lossy().into_owned();
    let (status, data) = get(&world, &root, Some("plain"), "/shellData", &[]);
    assert_eq!(status, 200, "{data}");
    assert_eq!(data["tui"]["drawerOpen"], serde_json::Value::Null, "{data}");
    assert_eq!(data["tui"]["mouse"], serde_json::Value::Null, "{data}");
    let set_key = |key: &str, value: &str| {
        let mut post = request("POST", "/apps/setPreference");
        post.set_header("authorization", basic("plain"));
        post.set_header("accept", "application/json");
        post.set_header("content-type", "application/x-www-form-urlencoded");
        post.body = format!("preferenceKey={key}&preferenceValue={value}").into_bytes();
        serve_at(&world.factory, post, &root)
    };
    assert_eq!(set_key("TUI_MOUSE", "false").status, 200);
    let (_, data) = get(&world, &root, Some("plain"), "/shellData", &[]);
    assert_eq!(data["tui"]["mouse"], json!(false), "{data}");
    let set = |user: Option<&str>, value: &str| {
        let mut post = request("POST", "/apps/setPreference");
        if let Some(user) = user {
            post.set_header("authorization", basic(user));
        }
        post.set_header("accept", "application/json");
        post.set_header("content-type", "application/x-www-form-urlencoded");
        post.body = format!("preferenceKey=TUI_DRAWER_OPEN&preferenceValue={value}").into_bytes();
        serve_at(&world.factory, post, &root)
    };
    for (value, expected) in [("true", json!(true)), ("false", json!(false))] {
        let response = set(Some("plain"), value);
        assert_eq!(response.status, 200, "{}", response.body_text());
        assert_eq!(response.body_text(), "", "type none answers no body");
        assert!(
            response
                .headers
                .iter()
                .any(|(name, header)| name == "X-Moqui-Preferences" && header.contains("TUI_DRAWER_OPEN")),
            "{:?}",
            response.headers
        );
        let (status, data) = get(&world, &root, Some("plain"), "/shellData", &[]);
        assert_eq!(status, 200, "{data}");
        assert_eq!(data["tui"]["drawerOpen"], expected, "{data}");
    }
    // no user: setPreference refuses with a 401, as Java's AuthenticationRequiredException gave.
    let response = set(None, "true");
    assert_eq!(response.status, 401, "{}", response.body_text());
    assert!(
        response.body_text().contains("Authentication required"),
        "{}",
        response.body_text()
    );
}

/// One execution context renders several screens, as a spec, a TUI session or an agent's renders do: the shell data
/// (whose counts service runs with authorization disabled by the framework and takes the Java idiom
/// `wasDisabled = disableAuthz(); ...; if (!wasDisabled) enableAuthz()`), a screen and a transition under an app whose
/// inheritable authorization is ALWAYS, a service called from that transition. None of them leaves authorization
/// disabled after it: the user is still refused the screen of an app they may not open.
#[test]
fn renders_under_always_and_the_shell_counts_leave_authorization_as_it_was() {
    let dir = tree("authz-scope");
    std::fs::write(
        dir.join("Apps/Open/List.xml"),
        r#"<screen require-authentication="true" default-menu-title="List">
            <transition name="touch"><actions><script><![CDATA[
                const shellData = sri.getShellData()
                const out = ec.service.sync().name('test.Shell.get#IdiomCounts').parameters({userId: ec.user.userId}).call()
                ec.web.sendJsonResponse({counts: shellData.counts, direct: out.counts, disabled: ec.artifactExecution.authzDisabled()})
            ]]></script></actions><default-response type="none"/></transition>
            <actions><script>const shellData = sri.getShellData()</script></actions>
            <widgets><label text="List"/></widgets></screen>"#,
    )
    .unwrap();
    let world = world(&dir);
    world.user("idiom", None);
    theme_value(
        &world,
        "IDIOM_QUASAR",
        1,
        "STRT_SHELL_COUNTS_SERVICE",
        "test.Shell.get#IdiomCounts",
    );
    row(
        &world,
        "moqui.security.UserScreenTheme",
        &[
            ("userId", Value::string("idiom")),
            ("screenThemeTypeEnumId", Value::string("STT_INTERNAL_QUASAR")),
            ("screenThemeId", Value::string("IDIOM_QUASAR")),
        ],
    );
    let root = root_of(&dir);
    let secret = dir.join("Apps/Secret.xml").to_string_lossy().into_owned();
    let ctx = world.factory.new_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("idiom").unwrap();
    let test = crate::screen::ScreenTest::new(&world.factory).root_screen(root.clone());
    let still_refused = |step: &str| {
        assert!(!ctx.authz_disabled(), "authorization is disabled after {step}");
        let err = crate::user::authorize("AT_XML_SCREEN", &secret, "view").unwrap_err();
        assert!(err.to_string().contains("not authorized"), "after {step}: {err}");
    };
    still_refused("login");

    // the shell data: its counts service ran with authorization disabled, by the framework
    let data = test.render("shellData", BTreeMap::new()).unwrap();
    assert_eq!(data.status(), 200, "{}", data.text());
    let data = data.json().unwrap();
    assert_eq!(data["counts"], json!({"idiom": true, "wasDisabled": true}), "{data}");
    still_refused("the shell data");

    // a screen under ALWAYS whose actions get the shell data
    let list = test.render("apps/open/List", BTreeMap::new()).unwrap();
    assert_eq!(list.status(), 200, "{}", list.text());
    assert!(list.text().contains("List"), "{}", list.text());
    still_refused("a screen under ALWAYS");

    // a transition under ALWAYS: the shell data, and the service called directly with authorization on
    let touch = test.render("apps/open/List/touch", BTreeMap::new()).unwrap();
    assert_eq!(touch.status(), 200, "{}", touch.text());
    let touch = touch.json().unwrap();
    assert_eq!(touch["direct"], json!({"idiom": true, "wasDisabled": false}), "{touch}");
    assert_eq!(touch["disabled"], json!(false), "{touch}");
    still_refused("a transition under ALWAYS");

    // the screen the user may not open is still refused by a render
    let refused = test.render("apps/secret", BTreeMap::new());
    assert!(
        refused.as_ref().map_or(true, |out| !out.text().contains("secret")),
        "{:?}",
        refused.map(|out| out.text())
    );
    still_refused("a refused render");
    let _ = std::fs::remove_dir_all(&dir);
}
