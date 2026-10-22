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

//! Path walks: default items, subscreens, path parameters, render-mode extensions, conditional
//! defaults, standalone screens, and the missing `/vapps`.

use std::path::PathBuf;

use moqui_lib::object::Value;

use super::{fixture_root, webroot};
use crate::context::{Error, ErrorKind, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::screen::cache::OutputKind;
use crate::screen::{ScreenFacade, ScreenUrlInfo};

fn walk(factory: &ExecutionContextFactory, path: &str) -> Result<std::sync::Arc<ScreenUrlInfo>, Error> {
    ScreenUrlInfo::parse(factory, &fixture_root(), path, 0)
}

fn names(info: &ScreenUrlInfo) -> Vec<&str> {
    info.path_defs.iter().map(|def| def.screen_name.as_str()).collect()
}

#[test]
fn an_empty_path_follows_default_items_to_the_end() {
    let context = ExecutionContextFactory::test_context();
    let info = walk(context.factory(), "/").unwrap();
    assert_eq!(names(&info), ["Root", "Home"]);
    assert_eq!(info.full_path, ["Home"]);
    assert!(info.minimal_path.is_empty(), "{:?}", info.minimal_path);
    assert_eq!(info.render_path_difference, 0);
    assert_eq!(info.target_screen.screen_name, "Home");

    let deep = walk(context.factory(), "/Sub/Deep").unwrap();
    assert_eq!(names(&deep), ["Root", "Sub", "Deep", "Leaf"]);
    assert_eq!(deep.full_path, ["Sub", "Deep", "Leaf"]);
    assert_eq!(deep.minimal_path, ["Sub", "Deep"]);
    assert_eq!(deep.pre_transition_path, ["Sub", "Deep", "Leaf"]);
    assert_eq!(deep.minimal_path_url(), "/Sub/Deep");
    assert_eq!(deep.full_path_url(), "/Sub/Deep/Leaf");
}

#[test]
fn subscreens_come_from_the_directory_and_from_subscreens_items() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    let root = facade.screen_def(context.factory(), &fixture_root()).unwrap();
    let by_menu: Vec<&str> = root.subscreens.sorted().iter().map(|item| item.name.as_str()).collect();
    // No index first by title, then by index: Linked is index 1 and Home is index 2.
    assert_eq!(by_menu, ["Picker", "Secure", "Standalone", "Sub", "Linked", "Home"]);
    let linked = root.subscreens.get("Linked").unwrap();
    assert_eq!(linked.menu_title, "Linked Items");
    assert_eq!(linked.menu_index, Some(1));
    assert!(linked.menu_include);
    assert_eq!(root.subscreens.get("Home").unwrap().menu_title, "Start");

    let info = walk(context.factory(), "/Linked").unwrap();
    assert_eq!(names(&info), ["Root", "Other"]);
    // A subscreens-item without a location is the file named like it under the screen directory.
    let sub = facade
        .screen_def(
            context.factory(),
            &format!("{}/src/screen/testdata/Root/Sub.xml", env!("CARGO_MANIFEST_DIR")),
        )
        .unwrap();
    assert!(sub
        .subscreens
        .get("Deep")
        .unwrap()
        .location
        .ends_with("Root/Sub/Deep.xml"));
}

#[test]
fn a_transition_ends_the_walk_and_the_rest_are_extra_path_names() {
    let context = ExecutionContextFactory::test_context();
    let info = walk(context.factory(), "/byId/42/more").unwrap();
    assert_eq!(info.target_transition_name.as_deref(), Some("byId"));
    assert_eq!(info.extra_path, ["42", "more"]);
    let transition = info.transition(Some("get")).unwrap();
    assert_eq!(transition.path_parameters, ["itemId"]);
    // A method that does not match a method-specific transition finds none.
    assert!(walk(context.factory(), "/postOnly")
        .unwrap()
        .transition(Some("get"))
        .is_none());
    assert!(walk(context.factory(), "/postOnly")
        .unwrap()
        .transition(Some("post"))
        .is_some());

    // allow-extra-path lets names that match nothing follow a screen.
    let extra = walk(context.factory(), "/Sub/anything/at/all").unwrap();
    assert_eq!(extra.target_screen.screen_name, "Sub");
    assert_eq!(extra.extra_path, ["anything", "at", "all"]);

    let missing = walk(context.factory(), "/NoSuchScreen").unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound, "{missing}");
    let under_deep = walk(context.factory(), "/Sub/Deep/Leaf/Nothing").unwrap_err();
    assert_eq!(under_deep.kind(), ErrorKind::NotFound);
}

#[test]
fn a_query_tail_becomes_inline_parameters() {
    let context = ExecutionContextFactory::test_context();
    let info = walk(context.factory(), "/Sub?a=1&b=x%20y").unwrap();
    assert_eq!(info.inline_parameters.get("a").map(String::as_str), Some("1"));
    assert_eq!(info.inline_parameters.get("b").map(String::as_str), Some("x y"));
    assert_eq!(info.target_screen.screen_name, "Sub");
    // The parameters are on this result only: the cached walk of the same path is shared and stays without them.
    let plain = walk(context.factory(), "/Sub").unwrap();
    assert!(plain.inline_parameters.is_empty());
    let again = walk(context.factory(), "/Sub").unwrap();
    assert!(std::sync::Arc::ptr_eq(&plain, &again));
}

#[test]
fn render_mode_extensions_select_the_mode_and_xsl_fo_is_html_print() {
    let context = ExecutionContextFactory::test_context();
    for (path, mode, standalone) in [
        ("/Sub.html", "html", 0),
        ("/Sub.text", "text", 0),
        ("/Sub.xml", "xml", 0),
        ("/Sub.csv", "csv", 1),
        ("/Sub.html-print", "html-print", 1),
        ("/Sub.xsl-fo", "html-print", 1),
        ("/Sub.qvt", "qvt", 1),
        ("/Sub.qjs", "qjs", 1),
        ("/Sub.qvue", "qvue", 1),
    ] {
        let info = walk(context.factory(), path).unwrap();
        assert_eq!(info.target_render_mode.as_deref(), Some(mode), "{path}");
        assert_eq!(info.last_standalone, standalone, "{path}");
        assert_eq!(info.target_screen.screen_name, "Sub", "{path}");
        // An explicit mode renders that screen and does not follow default items below it.
        assert_eq!(info.full_path, ["Sub"], "{path}");
    }
    // The old Vue modes and anything else are not render modes, so no screen has that name.
    for path in ["/Sub.vuet", "/Sub.js", "/Sub.vue", "/Sub.vapps", "/Sub.nonsense"] {
        let err = walk(context.factory(), path).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound, "{path}: {err}");
    }
    // Always-standalone modes render only the target: the screens above only run actions.
    let csv = walk(context.factory(), "/Sub/Deep/Leaf.csv").unwrap();
    assert_eq!(names(&csv), ["Root", "Sub", "Deep", "Leaf"]);
    assert_eq!(csv.render_path_difference, 3);
    assert_eq!(csv.render_defs().len(), 1);
    let html = walk(context.factory(), "/Sub/Deep/Leaf.html").unwrap();
    assert_eq!(html.render_path_difference, 0);
}

#[test]
fn a_transition_takes_a_render_mode_extension_too() {
    let context = ExecutionContextFactory::test_context();
    let info = walk(context.factory(), "/goUrl.json").err();
    // json is not a render mode, so the name stays whole and matches nothing.
    assert_eq!(info.map(|err| err.kind()), Some(ErrorKind::NotFound));
    let info = walk(context.factory(), "/goUrl.text").unwrap();
    assert_eq!(info.target_transition_name.as_deref(), Some("goUrl"));
    assert_eq!(info.target_transition_extension.as_deref(), Some("text"));
}

#[test]
fn a_standalone_screen_drops_the_screens_above_it_from_the_render() {
    let context = ExecutionContextFactory::test_context();
    let info = walk(context.factory(), "/Standalone/Child").unwrap();
    assert_eq!(names(&info), ["Root", "Standalone", "Child"]);
    assert_eq!(info.render_path_difference, 1);
    // A negative lastStandalone drops that many from the top.
    let info = ScreenUrlInfo::parse(context.factory(), &fixture_root(), "/Sub/Deep/Leaf", -2).unwrap();
    assert_eq!(info.render_path_difference, 2);
}

#[test]
fn a_conditional_default_replaces_the_default_item() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let a = walk(context.factory(), "/Picker").unwrap();
    assert_eq!(a.full_path, ["Picker", "A"]);
    assert!(!a.reusable);
    let _frame = crate::script::ContextFrame::push();
    crate::script::set_context_name("mode", Value::string("b")).unwrap();
    let b = walk(context.factory(), "/Picker").unwrap();
    assert_eq!(b.full_path, ["Picker", "B"]);
}

#[test]
fn vapps_is_not_a_screen_under_webroot() {
    let context = ExecutionContextFactory::test_context();
    let root = webroot().to_string_lossy().into_owned();
    for path in ["/vapps", "/vapps/ScreenTree", "/vapps.qvt"] {
        let err = ScreenUrlInfo::parse(context.factory(), &root, path, 0).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound, "{path}: {err}");
    }
    // The apps that stay are there.
    for path in ["/qapps", "/apps", "/rest/s1/x", "/Login"] {
        ScreenUrlInfo::parse(context.factory(), &root, path, 0).unwrap_or_else(|err| panic!("{path}: {err}"));
    }
    let facade = ScreenFacade::of(context.factory());
    let webroot_def = facade.screen_def(context.factory(), &root).unwrap();
    assert!(webroot_def.subscreens.get("vapps").is_none());
}

#[test]
fn text_output_conf_has_the_modes_and_the_print_alias() {
    let context = ExecutionContextFactory::test_context();
    let facade = ScreenFacade::of(context.factory());
    assert_eq!(facade.mime_type("csv"), Some("text/csv"));
    assert_eq!(facade.mime_type("xsl-fo"), Some("text/html"));
    assert!(facade.is_always_standalone("xsl-fo"));
    assert!(facade.is_always_standalone("html-print"));
    assert!(!facade.is_always_standalone("html"));
    assert!(!facade.is_render_mode_valid("vuet"));
    assert!(facade
        .text_output("qvt")
        .unwrap()
        .macro_template_location
        .ends_with("ScreenQvt.js"));
    assert_eq!(facade.mime_type("tui"), Some("application/json"));
    assert!(facade.is_always_standalone("tui"));
    assert_eq!(facade.output_kind("tui"), OutputKind::Json);
    assert_eq!(facade.output_kind("qvt"), OutputKind::Vue);
    assert_eq!(facade.output_kind("qvue"), OutputKind::Text);
}

#[test]
fn the_default_conf_parses_into_the_factory_conf() {
    let conf = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/base-component/framework/conf/MoquiDefaultConf.xml"
    ))
    .unwrap();
    let parsed = super::super::cache::ScreenConf::parse(&conf);
    let mut modes: Vec<&str> = parsed.text_output.keys().map(String::as_str).collect();
    modes.sort_unstable();
    assert_eq!(
        modes,
        [
            "csv",
            "html",
            "html-print",
            "qjs",
            "qvt",
            "qvue",
            "text",
            "tui",
            "xlsx",
            "xml"
        ]
    );
    // xlsx is the one screen-output (binary) mode
    let xlsx = &parsed.text_output["xlsx"];
    assert!(xlsx.binary && xlsx.always_standalone);
    assert_eq!(xlsx.macro_template_location, "framework/template/screen/ScreenXlsx.js");
    assert!(!parsed.text_output["csv"].binary);
    let print = &parsed.text_output["html-print"];
    assert_eq!(print.mime_type, "text/html");
    assert!(print.always_standalone);
    assert_eq!(
        print.macro_template_location,
        "framework/template/screen/ScreenHtmlPrint.js"
    );
    assert!(!parsed.text_output["html"].always_standalone);
    let tui = &parsed.text_output["tui"];
    assert!(!tui.binary && tui.always_standalone);
    assert_eq!(tui.mime_type, "application/json");
    assert_eq!(tui.kind, OutputKind::Json);
    assert_eq!(tui.macro_template_location, "framework/template/screen/ScreenTui.js");
    assert_eq!(parsed.text_output["qvt"].kind, OutputKind::Vue);
    assert_eq!(parsed.text_output["html"].kind, OutputKind::Html);
    assert_eq!(parsed.text_output["qvue"].kind, OutputKind::Text);
    assert_eq!(parsed.default_paginate_rows, 20);
    for dropped in ["vuet", "js", "vue"] {
        assert!(!parsed.text_output.contains_key(dropped), "{dropped}");
    }
}

#[test]
fn output_kind_comes_from_the_attribute_or_the_mime_type() {
    let parsed = super::super::cache::ScreenConf::parse(
        r#"<moqui-conf><screen-facade>
        <screen-text-output type="tui" mime-type="text/plain" always-standalone="true" output-kind="json"
            macro-template-location="framework/template/screen/ScreenTui.js"/>
        <screen-text-output type="extra" mime-type="application/json"
            macro-template-location="framework/template/screen/ScreenPlain.js"/>
        <screen-text-output type="page" mime-type="text/html"
            macro-template-location="framework/template/screen/ScreenHtml.js"/>
        <screen-text-output type="template" mime-type="text/html" output-kind="vue"
            macro-template-location="framework/template/screen/ScreenQvt.js"/>
        <screen-text-output type="odd" mime-type="text/html" output-kind="pdf"
            macro-template-location="framework/template/screen/ScreenHtml.js"/>
    </screen-facade></moqui-conf>"#,
    );
    assert_eq!(parsed.text_output["tui"].kind, OutputKind::Json);
    assert_eq!(parsed.text_output["tui"].mime_type, "text/plain");
    assert_eq!(parsed.text_output["extra"].kind, OutputKind::Json);
    assert_eq!(parsed.text_output["page"].kind, OutputKind::Html);
    assert_eq!(parsed.text_output["template"].kind, OutputKind::Vue);
    // an unknown output-kind follows the mime type
    assert_eq!(parsed.text_output["odd"].kind, OutputKind::Html);
    // no mode is built in: one the conf does not name is not there
    for absent in ["qvt", "qvue", "html", "xlsx"] {
        assert!(!parsed.text_output.contains_key(absent), "{absent}");
    }
}

#[test]
fn dots_and_tilde_parameters_are_cleaned_before_the_walk() {
    let context = ExecutionContextFactory::test_context();
    // Linked is the path name of the screen whose own name is Other.
    let linked = walk(context.factory(), "/Sub/../Linked").unwrap();
    assert_eq!(linked.full_path, ["Linked"]);
    assert_eq!(linked.target_screen.screen_name, "Other");
    let deep = walk(context.factory(), "/Sub/./Deep").unwrap();
    assert_eq!(deep.minimal_path, ["Sub", "Deep"]);
    let home = walk(context.factory(), "/Sub/..").unwrap();
    assert_eq!(home.full_path, ["Home"]);

    let tilde = walk(context.factory(), "/Home/~id=I1").unwrap();
    assert_eq!(tilde.full_path, ["Home"]);
    assert_eq!(tilde.inline_parameters.get("id").map(String::as_str), Some("I1"));
    let decoded = walk(context.factory(), "/Home/~id=a%20b").unwrap();
    assert_eq!(decoded.inline_parameters.get("id").map(String::as_str), Some("a b"));
    let plus = walk(context.factory(), "/Home/~id=a+b").unwrap();
    assert_eq!(plus.inline_parameters.get("id").map(String::as_str), Some("a b"));
    // Java's split("=") drops a trailing empty and keeps a pair only when there are two parts.
    let empty = walk(context.factory(), "/Home/~id=").unwrap();
    assert!(empty.inline_parameters.is_empty());
    assert_eq!(empty.full_path, ["Home"]);
    let extra = walk(context.factory(), "/Home/~id=a=b").unwrap();
    assert!(extra.inline_parameters.is_empty());
    assert_eq!(extra.full_path, ["Home"]);
}

fn at(ms: i64) -> Value {
    moqui_lib::object::basic_convert(&Value::I64(ms), "java.sql.Timestamp").value
}

fn screen_db() -> (crate::context::ExecutionContext, EntityFacade) {
    let context = ExecutionContextFactory::test_context();
    let facade = crate::testing::open();
    context.factory().set_entity(facade.clone());
    (context, facade)
}

fn alias_row(facade: &EntityFacade, path: &str, screen: &str, from_ms: i64, thru_ms: Option<i64>) {
    let mut row = facade.make_value("moqui.screen.ScreenPathAlias").unwrap();
    row.set("aliasPath", Value::string(path));
    row.set("fromDate", at(from_ms));
    row.set("screenPath", Value::string(screen));
    if let Some(thru) = thru_ms {
        row.set("thruDate", at(thru));
    }
    row.create(facade).unwrap();
}

#[test]
fn a_screen_path_alias_rewrites_the_request_and_a_link() {
    let (context, facade) = screen_db();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    alias_row(&facade, "/pcontact", "/Home", 1_000, None);
    // Two open rows: the newer fromDate wins. An expired row and a future row do not apply.
    alias_row(&facade, "/pick", "/Home", 1_000_000_000_000, None);
    alias_row(&facade, "/pick", "/Sub", 1_700_000_000_000, None);
    alias_row(&facade, "/old", "/Home", 1_000, Some(2_000));
    alias_row(&facade, "/later", "/Home", 9_000_000_000_000, None);
    alias_row(&facade, "/Sub/go", "/Home", 1_000, None);

    let home = walk(context.factory(), "/pcontact").unwrap();
    assert_eq!(home.full_path, ["Home"]);
    // The lookup key includes the raw query, so the row for `/pcontact` does not match.
    assert_eq!(
        walk(context.factory(), "/pcontact?x=1").unwrap_err().kind(),
        ErrorKind::NotFound
    );
    let newer = walk(context.factory(), "/pick").unwrap();
    assert_eq!(newer.full_path[0], "Sub");
    assert_eq!(walk(context.factory(), "/old").unwrap_err().kind(), ErrorKind::NotFound);
    assert_eq!(
        walk(context.factory(), "/later").unwrap_err().kind(),
        ErrorKind::NotFound
    );

    let linked = crate::screen::resolve_screen_path(context.factory(), &fixture_root(), &["Sub".into()], "go").unwrap();
    assert_eq!(linked.names, ["Home"]);
}

#[test]
fn without_an_entity_facade_an_alias_path_is_unchanged() {
    let context = ExecutionContextFactory::test_context();
    assert_eq!(
        walk(context.factory(), "/pcontact").unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

#[test]
fn a_double_slash_finds_a_screen_by_name() {
    let context = ExecutionContextFactory::test_context();
    let leaf = walk(context.factory(), "//Leaf").unwrap();
    assert_eq!(leaf.full_path, ["Sub", "Deep", "Leaf"]);
    let deep = walk(context.factory(), "//Deep").unwrap();
    assert_eq!(deep.minimal_path, ["Sub", "Deep"]);
    assert_eq!(deep.full_path, ["Sub", "Deep", "Leaf"]);
    let full = walk(context.factory(), "//Sub/Deep").unwrap();
    assert_eq!(full.minimal_path, ["Sub", "Deep"]);
    assert_eq!(full.full_path, ["Sub", "Deep", "Leaf"]);
    assert_eq!(
        walk(context.factory(), "//NoSuchName").unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        walk(context.factory(), "///foo").unwrap_err().kind(),
        ErrorKind::NotFound
    );
    // The miss is cached. A second lookup is still not found.
    assert_eq!(
        walk(context.factory(), "//NoSuchName").unwrap_err().kind(),
        ErrorKind::NotFound
    );
}

struct Tree {
    dir: PathBuf,
}

impl Tree {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-screen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_subscreen_cycle_stops_at_the_depth_cap() {
    let tree = Tree::new("cycle");
    let cycle = tree.path("Cycle.xml");
    let loop_screen = tree.path("Loop.xml");
    tree.write(
        "Cycle.xml",
        &format!(
            r#"<screen require-authentication="false"><subscreens>
                <subscreens-item name="Loop" location="{loop_screen}"/>
            </subscreens></screen>"#
        ),
    );
    tree.write(
        "Loop.xml",
        &format!(
            r#"<screen require-authentication="false"><subscreens>
                <subscreens-item name="Back" location="{cycle}"/>
            </subscreens></screen>"#
        ),
    );
    let context = ExecutionContextFactory::test_context();
    let found = ScreenUrlInfo::parse(context.factory(), &cycle, "//Loop", 0).unwrap();
    assert_eq!(found.full_path, ["Loop"]);
    let back = ScreenUrlInfo::parse(context.factory(), &cycle, "//Back", 0).unwrap();
    assert_eq!(back.full_path, ["Loop", "Back"]);
    let missing = ScreenUrlInfo::parse(context.factory(), &cycle, "//Missing", 0).unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
}

#[test]
fn a_file_under_a_screen_ends_the_walk() {
    let tree = Tree::new("files");
    let wrap = tree.path("Wrap.xml");
    let host = tree.path("Host.xml");
    let page = tree.path("Page.xml");
    tree.write(
        "Wrap.xml",
        r#"<screen require-authentication="false"><widgets><label text="wrap"/></widgets></screen>"#,
    );
    tree.write("Wrap/note.txt", "a note");
    tree.write(
        "Host.xml",
        &format!(
            r#"<screen require-authentication="false"><subscreens>
                <subscreens-item name="Wrap" location="{wrap}" no-sub-path="true"/>
            </subscreens></screen>"#
        ),
    );
    tree.write(
        "Page.xml",
        r#"<screen require-authentication="false"><subscreens default-item="readme.txt"/></screen>"#,
    );
    tree.write("Page/readme.txt", "read me");
    let context = ExecutionContextFactory::test_context();

    let wrapped = ScreenUrlInfo::parse(context.factory(), &host, "/note.txt", 0).unwrap();
    assert_eq!(wrapped.full_path, ["Wrap", "note.txt"]);
    assert!(
        wrapped
            .file_resource
            .as_deref()
            .is_some_and(|path| path.ends_with("note.txt")),
        "{:?}",
        wrapped.file_resource
    );

    let by_default = ScreenUrlInfo::parse(context.factory(), &page, "/", 0).unwrap();
    assert_eq!(by_default.full_path, ["readme.txt"]);
    assert!(by_default
        .file_resource
        .as_deref()
        .is_some_and(|path| path.ends_with("readme.txt")));

    let named = ScreenUrlInfo::parse(context.factory(), &page, "/readme.txt", 0).unwrap();
    assert_eq!(named.full_path, ["readme.txt"]);
    assert!(named.file_resource.is_some());
}

#[test]
fn a_missing_screen_url_is_a_hash() {
    let missing = crate::screen::urls::UrlInstance {
        exists: false,
        ..crate::screen::urls::UrlInstance::default()
    };
    assert_eq!(missing.path(), "#");
    assert_eq!(missing.url(), "#");
    assert_eq!(missing.minimal_path_url(), "#");
    assert!(!missing.is_screen_url());
    let plain = crate::screen::urls::UrlInstance {
        plain: Some("https://example.com/x".to_string()),
        exists: true,
        ..crate::screen::urls::UrlInstance::default()
    };
    assert!(!plain.is_screen_url());
    let screen = crate::screen::urls::UrlInstance {
        exists: true,
        ..crate::screen::urls::UrlInstance::default()
    };
    assert!(screen.is_screen_url());
}

/// A plain URL is the text as written. `path()` is empty. The base is not prefixed.
#[test]
fn a_plain_url_is_not_prefixed_and_has_no_path() {
    let mailto = crate::screen::urls::UrlInstance {
        plain: Some("mailto:someone@example.com".to_string()),
        base: "http://host".to_string(),
        exists: true,
        ..crate::screen::urls::UrlInstance::default()
    };
    assert_eq!(mailto.url(), "mailto:someone@example.com");
    assert_eq!(mailto.path(), "");
    let path = crate::screen::urls::UrlInstance {
        plain: Some("/some/path".to_string()),
        base: "http://host/".to_string(),
        exists: true,
        ..crate::screen::urls::UrlInstance::default()
    };
    assert_eq!(path.url(), "/some/path");
    assert_eq!(path.path(), "");
    let screen = crate::screen::urls::UrlInstance {
        names: vec!["Sub".to_string()],
        base: "http://host".to_string(),
        exists: true,
        ..crate::screen::urls::UrlInstance::default()
    };
    assert_eq!(screen.url(), "http://host/Sub");
    assert_eq!(screen.path(), "/Sub");
}

#[test]
fn the_host_header_selects_the_root_screen() {
    let tree = Tree::new("host");
    let shop = tree.path("Shop.xml");
    let other = tree.path("Other.xml");
    tree.write(
        "Shop.xml",
        r#"<screen require-authentication="false"><widgets><label text="shop screen"/></widgets></screen>"#,
    );
    tree.write(
        "Other.xml",
        r#"<screen require-authentication="false"><widgets><label text="other screen"/></widgets></screen>"#,
    );
    // `.*` is listed first and is still only the fallback.
    let conf = format!(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <root-screen host=".*" location="{other}"/>
            <root-screen host="shop\.example" location="{shop}"/>
        </webapp></webapp-list></moqui-conf>"#
    );
    let context = ExecutionContextFactory::test_context_with_conf(&conf);
    // A screen response signs the session cookie, so the key has to be set for this process.
    let _env = super::Env::new();
    let shop_location =
        crate::screen::info::root_screen_for_host(context.factory(), "shop.example:8080", "webroot").unwrap();
    assert_eq!(shop_location, shop);
    let fallback = crate::screen::info::root_screen_for_host(context.factory(), "other.example", "webroot").unwrap();
    assert_eq!(fallback, other);

    let mut request = super::request("GET", "/");
    request.set_header("Host", "shop.example");
    let response = crate::screen::serve(context.factory(), request);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("shop screen"), "{}", response.body_text());
    let mut request = super::request("GET", "/");
    request.set_header("Host", "other.example");
    let response = crate::screen::serve(context.factory(), request);
    assert!(
        response.body_text().contains("other screen"),
        "{}",
        response.body_text()
    );

    let _entered = context.enter();
    let rendered = crate::script::eval_value(
        "ec.screen.makeRender().rootScreenFromHost('shop.example').screenPath('/').renderMode('text').render()",
    )
    .unwrap();
    let rendered = moqui_lib::object::to_plain_string(&rendered);
    assert!(rendered.contains("shop screen"), "{rendered}");
    let explicit = format!(
        "ec.screen.makeRender().rootScreen('{}').rootScreenFromHost('shop.example').screenPath('/').renderMode('text').render()",
        other.replace('\'', "\\'")
    );
    let explicit = moqui_lib::object::to_plain_string(&crate::script::eval_value(&explicit).unwrap());
    assert!(explicit.contains("other screen"), "{explicit}");
    assert!(!explicit.contains("shop screen"), "{explicit}");
}

#[test]
fn a_host_with_no_root_screen_is_an_error() {
    let context = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <root-screen host="shop\.example" location="/nope.xml"/>
        </webapp></webapp-list></moqui-conf>"#,
    );
    let missing = crate::screen::info::root_screen_for_host(context.factory(), "other.example", "webroot").unwrap_err();
    assert!(missing.message().contains("other.example"), "{missing}");
    assert!(crate::screen::info::root_screen_for_host(context.factory(), "shop.example", "webroot").is_ok());

    let bad = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <root-screen host="[" location="/bad.xml"/>
        </webapp></webapp-list></moqui-conf>"#,
    );
    let invalid = crate::screen::info::root_screen_for_host(bad.factory(), "x", "webroot").unwrap_err();
    assert!(invalid.message().contains("regex"), "{invalid}");

    let no_webapp = ExecutionContextFactory::test_context_with_conf("<moqui-conf/>");
    let err = crate::screen::info::root_screen_for_host(no_webapp.factory(), "localhost", "webroot").unwrap_err();
    assert!(err.message().contains("webroot"), "{err}");
}
