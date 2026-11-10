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
        std::fs::write(dir.join("Site/sw.js"), "self.addEventListener('push', () => {});").unwrap();
        std::fs::write(dir.join("Site/manifest.webmanifest"), "{\"name\": \"Moqui\"}").unwrap();
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

/// The PWA's service worker is never cached (a browser would keep an old one in charge) and may take the whole site as
/// its scope; the manifest has its own media type.
#[test]
fn the_service_worker_is_uncached_with_the_root_scope_and_the_manifest_is_typed() {
    let site = Site::new("pwa");
    let world = World::open("");
    let worker = serve_at(&world.factory, request("GET", "/sw.js"), &site.root);
    assert_eq!(worker.status, 200, "{}", worker.body_text());
    assert_eq!(worker.header("Cache-Control"), Some("no-cache"));
    assert_eq!(worker.header("Service-Worker-Allowed"), Some("/"));
    let mut versioned = request("GET", "/sw.js");
    versioned.query = vec![("v".to_string(), "abc".to_string())];
    let versioned = serve_at(&world.factory, versioned, &site.root);
    assert_eq!(versioned.header("Cache-Control"), Some("no-cache"));
    let manifest = serve_at(&world.factory, request("GET", "/manifest.webmanifest"), &site.root);
    assert_eq!(
        manifest.header("Content-Type"),
        Some("application/manifest+json; charset=UTF-8")
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

/// A static file under a screen starts no Visitor or Visit, where the screen itself does.
#[test]
fn a_file_under_the_screen_starts_no_visit() {
    let site = Site::new("visit");
    let world = World::open("");
    let visits = || {
        let ctx = world.factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let facade = world.factory.entity().unwrap();
        (
            facade.find("moqui.server.Visit").unwrap().count().unwrap(),
            facade.find("moqui.server.Visitor").unwrap().count().unwrap(),
        )
    };
    let before = visits();
    let mut file = request("GET", "/css/app.css");
    file.set_header("cookie", "moqui.visitor=V-NONE");
    let response = serve_at(&world.factory, file, &site.root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(visits(), before, "a file is just content");
    let mut screen = request("GET", "/");
    screen.set_header("cookie", "moqui.visitor=V-NONE");
    let response = serve_at(&world.factory, screen, &site.root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(visits(), (before.0 + 1, before.1 + 1), "the screen starts a visit");
}

/// `Panel.qvue` beside `Panel.xml`, which names qvue in its `render-modes`, is the screen in qvue mode (its actions and
/// template run), not the raw file. A `.qvue` beside a screen that does not name the mode (a public island's file), and
/// one with no screen beside it, are still static files.
#[test]
fn a_render_mode_file_beside_its_screen_renders_the_screen() {
    let site = Site::new("mode-source");
    let write = |path: &str, text: &str| std::fs::write(site.dir.join(path), text).unwrap();
    let panel = site.dir.join("Site/Panel.qvue").to_string_lossy().into_owned();
    write(
        "Site/Panel.xml",
        &format!(
            r#"<screen require-authentication="false" render-modes="html,qvue">
                <actions><set field="greeting" value="Hello from the screen"/></actions>
                <widgets><render-mode><text type="qvue" template="fancy" location="{panel}"/></render-mode></widgets>
            </screen>"#
        ),
    );
    write("Site/Panel.qvue", "<template><p>${greeting}</p></template>");
    let private = site.dir.join("Site/Private.qvue").to_string_lossy().into_owned();
    write(
        "Site/Private.xml",
        &format!(
            r#"<screen require-authentication="true" render-modes="qvue">
                <widgets><render-mode><text type="qvue" template="false" location="{private}"/></render-mode></widgets>
            </screen>"#
        ),
    );
    write("Site/Private.qvue", "<template><p>private</p></template>");
    write(
        "Site/Island.xml",
        r#"<screen require-authentication="false"><widgets><label text="island"/></widgets></screen>"#,
    );
    write("Site/Island.qvue", "<template><p>island ${raw}</p></template>");
    write("Site/Loose.qvue", "<template><p>loose ${raw}</p></template>");
    let world = World::open("");

    let response = serve_at(&world.factory, request("GET", "/Panel.qvue"), &site.root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    let body = response.body_text();
    assert!(body.contains("Hello from the screen"), "{body}");
    assert!(!body.contains("${greeting}"), "the raw file was sent: {body}");
    // the screen's authentication holds: no user, no file
    let response = serve_at(&world.factory, request("GET", "/Private.qvue"), &site.root);
    assert!(
        !response.body_text().contains("<p>private</p>"),
        "{}: {}",
        response.status,
        response.body_text()
    );

    for (path, text) in [
        ("/Island.qvue", "<template><p>island ${raw}</p></template>"),
        ("/Loose.qvue", "<template><p>loose ${raw}</p></template>"),
    ] {
        let response = serve_at(&world.factory, request("GET", path), &site.root);
        assert_eq!(response.status, 200, "{path}: {}", response.body_text());
        assert_eq!(response.body_text(), text, "{path}");
    }
    // the file is refused as a file too, wherever a walk asks for one
    let facade = crate::screen::ScreenFacade::of(&world.factory);
    let location = |name: &str| site.dir.join("Site").join(name).to_string_lossy().into_owned();
    assert!(crate::screen::static_files::is_render_mode_source(
        &world.factory,
        &facade,
        &location("Panel.qvue")
    ));
    for name in ["Island.qvue", "Loose.qvue"] {
        assert!(
            !crate::screen::static_files::is_render_mode_source(&world.factory, &facade, &location(name)),
            "{name}"
        );
    }
}
