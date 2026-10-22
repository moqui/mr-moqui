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

//! Static files under a screen: the response headers, conditional requests, HEAD, and the paths that must
//! never be served.

use std::path::PathBuf;

use super::{request, World};
use crate::screen::serve_at;

struct Site {
    dir: PathBuf,
    root: String,
}

impl Site {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-static-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Site/css")).unwrap();
        std::fs::create_dir_all(dir.join("Site/js")).unwrap();
        std::fs::create_dir_all(dir.join("Site/.git")).unwrap();
        std::fs::write(
            dir.join("Site.xml"),
            r#"<screen require-authentication="false"><widgets><label text="the screen"/></widgets></screen>"#,
        )
        .unwrap();
        std::fs::write(dir.join("Site/css/app.css"), "body { color: red }").unwrap();
        std::fs::write(dir.join("Site/js/app.min.js"), "var a = 1;").unwrap();
        std::fs::write(dir.join("Site/js/app.js"), "var b = 2;").unwrap();
        std::fs::write(dir.join("Site/.secret"), "hidden").unwrap();
        std::fs::write(dir.join("Site/.git/config"), "hidden").unwrap();
        std::fs::write(dir.join("Site/Other.xml"), "<screen/>").unwrap();
        std::fs::write(dir.join("Site/Note.xml"), "<not-a-screen/>").unwrap();
        std::fs::write(dir.join("Site/page.ftl"), "<#if x>y</#if>").unwrap();
        std::fs::write(dir.join("outside.txt"), "outside").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("outside.txt"), dir.join("Site/link.txt")).unwrap();
        let root = dir.join("Site.xml").to_string_lossy().into_owned();
        Self { dir, root }
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_file_under_the_screen_is_served_with_a_type_a_validator_and_a_cache_rule() {
    let site = Site::new("serve");
    let world = World::open("");
    let response = serve_at(&world.factory, request("GET", "/css/app.css"), &site.root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(response.body_text(), "body { color: red }");
    assert_eq!(response.header("Content-Type"), Some("text/css; charset=UTF-8"));
    assert_eq!(
        response.header("Cache-Control"),
        Some("max-age=86400, must-revalidate, public"),
        "the conf's screen-resource-text header"
    );
    assert_eq!(response.header("Content-Length"), Some("19"));
    assert!(response.header("ETag").is_some_and(|tag| tag.starts_with("W/\"")));
    assert_eq!(response.header("X-Content-Type-Options"), Some("nosniff"));

    // A minified file and a request with a version parameter are cached for a year.
    let minified = serve_at(&world.factory, request("GET", "/js/app.min.js"), &site.root);
    assert_eq!(minified.header("Content-Type"), Some("text/javascript; charset=UTF-8"));
    assert_eq!(
        minified.header("Cache-Control"),
        Some("public, max-age=31536000, immutable")
    );
    let mut versioned = request("GET", "/js/app.js");
    versioned.query = vec![("v".to_string(), "abc".to_string())];
    let versioned = serve_at(&world.factory, versioned, &site.root);
    assert_eq!(
        versioned.header("Cache-Control"),
        Some("public, max-age=31536000, immutable")
    );
}

#[test]
fn if_none_match_gets_a_304_and_head_gets_no_body() {
    let site = Site::new("conditional");
    let world = World::open("");
    let first = serve_at(&world.factory, request("GET", "/css/app.css"), &site.root);
    let tag = first.header("ETag").unwrap().to_string();

    let mut again = request("GET", "/css/app.css");
    again.set_header("if-none-match", tag.clone());
    let response = serve_at(&world.factory, again, &site.root);
    assert_eq!(response.status, 304);
    assert!(response.body.is_empty());
    assert_eq!(response.header("ETag"), Some(tag.as_str()));
    assert!(response.header("Cache-Control").is_some());

    let mut other = request("GET", "/css/app.css");
    other.set_header("if-none-match", "W/\"different\"");
    assert_eq!(serve_at(&world.factory, other, &site.root).status, 200);

    let head = serve_at(&world.factory, request("HEAD", "/css/app.css"), &site.root);
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert_eq!(head.header("Content-Length"), Some("19"));
    assert_eq!(head.header("ETag"), Some(tag.as_str()));

    let post = serve_at(&world.factory, request("POST", "/css/app.css"), &site.root);
    assert_eq!(post.status, 405);
    assert_eq!(post.header("Allow"), Some("GET, HEAD"));
}

#[test]
fn traversal_hidden_sources_and_links_out_of_the_directory_are_not_found() {
    let site = Site::new("traversal");
    let world = World::open("");
    for path in [
        "/css/../../outside.txt",
        "/../outside.txt",
        "/css/%2e%2e/%2e%2e/outside.txt",
        "/css/..%2f..%2foutside.txt",
        "/css/%2E%2E%2F%2E%2E%2Foutside.txt",
        "/css/..%5c..%5coutside.txt",
        "/.secret",
        "/.git/config",
        "/%2esecret",
        "/css//../../outside.txt",
        "/Other.xml",
        "/Note.xml",
        "/Site.xml",
        "/page.ftl",
        "/css/missing.css",
        "/link.txt",
        "/css",
    ] {
        let response = serve_at(&world.factory, request("GET", path), &site.root);
        // Other.xml is a screen of the site, so it redirects to the login. Nothing else is found at all.
        assert!(
            response.status == 404 || (path == "/Other.xml" && response.status == 302),
            "{path}: {} {}",
            response.status,
            response.body_text()
        );
        assert_ne!(response.body_text(), "hidden", "{path}");
        assert_ne!(response.body_text(), "outside", "{path}");
        assert!(!response.body_text().contains("<#if"), "{path}");
    }
}

#[test]
fn a_screen_with_a_render_mode_extension_still_beats_a_missing_file() {
    let site = Site::new("screen-wins");
    let world = World::open("");
    let response = serve_at(&world.factory, request("GET", "/.text"), &site.root);
    assert_ne!(response.status, 500, "{}", response.body_text());
}

/// True when `node` is installed and has `node --test`, which came in Node 18; otherwise says why `tests` are skipped.
fn node_has_test_runner(tests: &str) -> bool {
    let output = match std::process::Command::new("node").arg("--version").output() {
        Ok(output) => output,
        Err(err) => {
            eprintln!("skipping {tests}, no node: {err}");
            return false;
        }
    };
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let major = version
        .trim_start_matches('v')
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok());
    match major {
        Some(major) if major >= 18 => true,
        _ => {
            eprintln!("skipping {tests}: node {version} has no `node --test` (Node 18 or later)");
            false
        }
    }
}

/// The pure functions of the html client run under Node. Nothing runs when `node` is not installed.
#[test]
fn the_html_client_passes_its_node_tests() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/base-component/webroot/screen/webroot/js/moqui-html.test.js");
    if !node_has_test_runner("the client tests") {
        return;
    }
    let output = match std::process::Command::new("node").arg("--test").arg(&file).output() {
        Ok(output) => output,
        Err(err) => {
            eprintln!("skipping the client tests, no node: {err}");
            return;
        }
    };
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The qvt client: MoquiLib.js, the formatter MoquiL10n.js (checked against the vectors the server formatter uses),
/// the components of WebrootVue.qvt.js (every template compiles), its suite shell (palette, key map, screen context) and
/// its record components (the HTML of m-rich-text, the record panels, avatars and record cards), its layout panels,
/// the cleaning of m-html-view, the form-list views (board columns, calendar times and lanes), m-markdown's rendering, and
/// the chat store (WebrootChat.js: presence, unread counts, typing, a pushed event
/// merged into a timeline, the status menu), Assist (WebrootAssist.js: the reducer, follow.locate, safePath, chips) and its canvas
/// (WebrootAssistCanvas.js: the library spec golden, lang-core parses, state reconciliation, and the Vue SFC frame's
/// message validator, watchdog and submit gate).
/// Nothing runs when `node` is not installed.
#[test]
fn the_qvt_client_passes_its_node_tests() {
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/webroot/screen/webroot/js");
    if !node_has_test_runner("the qvt client tests") {
        return;
    }
    let mut command = std::process::Command::new("node");
    command.arg("--test");
    for name in [
        "MoquiLib.test.js",
        "MoquiL10n.test.js",
        "WebrootVue.qvt.test.js",
        "WebrootVue.qvt.static.test.js",
        "WebrootVue.qvt.shell.test.js",
        "WebrootVue.qvt.records.test.js",
        "WebrootVue.qvt.layout.test.js",
        "WebrootVue.qvt.htmlview.test.js",
        "WebrootVue.qvt.views.test.js",
        "WebrootVue.qvt.markdown.test.js",
        "WebrootVue.qvt.code.test.js",
        "WebrootVue.qvt.coedit.test.js",
        "WebrootPublic.qvt.test.js",
        "WebrootChat.test.js",
        "WebrootAssist.test.js",
        "WebrootAssistCanvas.test.js",
    ] {
        command.arg(dir.join(name));
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            eprintln!("skipping the qvt client tests, no node: {err}");
            return;
        }
    };
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
