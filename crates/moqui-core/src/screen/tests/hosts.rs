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

//! Root screens by host name (plans/Edge-Node2.md §4.2): a wrapper root of a host with the default root's shared
//! static files and error screens, `framework-routes` include and refuse, the root list each root once, the hosts a link
//! may use, and a public site whose root is also its host's root screen.

use super::walk::Tree;
use super::{request, Env};
use crate::context::ExecutionContextFactory;
use crate::screen::info;

/// The default root, `Default.xml` with a transition, a home page, an error screen and a shared script, and a wrapper
/// root, `Wrapper.xml` with a home page and a script of its own; the conf gives the wrapper to `own.test` (no
/// framework-routes), `inc.test` (include), `ref.test` (refuse) and `*.sub.test`.
fn sites(tree: &Tree) -> String {
    let default = tree.path("Default.xml");
    let wrapper = tree.path("Wrapper.xml");
    tree.write(
        "Default.xml",
        r#"<screen require-authentication="false">
            <transition name="ping"><actions><script>ec.web.sendTextResponse('default ping')</script></actions>
                <default-response type="none"/></transition>
            <subscreens default-item="Home"/>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    tree.write(
        "Default/Home.xml",
        r#"<screen require-authentication="false"><widgets><label text="default home"/></widgets></screen>"#,
    );
    tree.write(
        "Default/error.xml",
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    );
    tree.write(
        "Default/error/NotFound.xml",
        r#"<screen require-authentication="false"><widgets><label text="default not found"/></widgets></screen>"#,
    );
    tree.write("Default/js/shared.js", "shared script");
    tree.write(
        "Wrapper.xml",
        r#"<screen require-authentication="false"><subscreens default-item="Home"/>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    tree.write(
        "Wrapper/Home.xml",
        r#"<screen require-authentication="false"><widgets><label text="wrapper home"/></widgets></screen>"#,
    );
    tree.write("Wrapper/js/own.js", "own script");
    format!(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <root-screen host=".*" location="{default}"/>
            <root-screen host="own\.test" location="{wrapper}"/>
            <root-screen host="inc\.test" location="{wrapper}" framework-routes="include"/>
            <root-screen host="ref\.test" location="{wrapper}" framework-routes="refuse"/>
            <root-screen host=".*\.sub\.test" location="{wrapper}"/>
        </webapp></webapp-list></moqui-conf>"#
    )
}

fn get(factory: &std::sync::Arc<ExecutionContextFactory>, host: &str, path: &str) -> (u16, String) {
    let (path, query) = path
        .split_once('?')
        .map_or((path, None), |(path, query)| (path, Some(query)));
    let mut request = request("GET", path);
    request.raw_query = query.map(str::to_string);
    request.set_header("Host", host);
    request.set_header("Accept", "text/html");
    let response = crate::screen::serve(factory, request);
    (response.status, response.body_text())
}

#[test]
fn a_wrapper_root_has_the_default_roots_static_files_and_error_screens_and_its_framework_routes_setting() {
    let tree = Tree::new("hosts");
    let context = ExecutionContextFactory::test_context_with_conf(&sites(&tree));
    let _env = Env::new();
    let factory = context.factory();
    let has = |(status, body): (u16, String), text: &str| {
        assert_eq!(status, 200, "{body}");
        assert!(body.contains(text), "{text}: {body}");
    };
    has(get(factory, "own.test", "/"), "wrapper home");
    has(get(factory, "inc.test:8080", "/"), "wrapper home");
    has(get(factory, "other.test", "/"), "default home");
    // the shared static files come from the default root on every host, a wrapper's own first
    for host in ["own.test", "inc.test", "ref.test"] {
        has(get(factory, host, "/js/shared.js"), "shared script");
        has(get(factory, host, "/js/own.js"), "own script");
    }
    // the default root's transition: only with include
    has(get(factory, "inc.test", "/ping"), "default ping");
    for host in ["own.test", "ref.test"] {
        let (status, body) = get(factory, host, "/ping");
        assert_eq!(status, 404, "{host}: {body}");
        assert!(!body.contains("default ping"), "{body}");
    }
    // a page the wrapper does not have shows the default root's error screen
    let (status, body) = get(factory, "own.test", "/Nope");
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("default not found"), "{body}");
    // `//js/...` is a sparse search under the root, not the default root's static files
    let (status, body) = get(factory, "own.test", "//js/shared.js");
    assert!(!body.contains("shared script"), "{status}: {body}");
    // `..` does not take a static route out of the static files
    let (status, _) = get(factory, "own.test", "/js/../ping");
    assert_eq!(status, 404);

    // each root once; the hosts a link may use
    assert_eq!(
        info::all_root_screen_locations(factory),
        [tree.path("Default.xml"), tree.path("Wrapper.xml")]
    );
    let link = |host: &str| info::link_host(factory, host, "webroot");
    assert_eq!(link("own.test:8443").as_deref(), Some("own.test"));
    assert_eq!(link("Shop.Sub.Test.").as_deref(), Some("shop.sub.test"));
    for host in [
        "other.test",
        "evil.test/x.sub.test",
        "own.test:@evil.test",
        "own.test:x",
        "own.test:",
        "",
        // a host that refuses the framework's routes makes no links: they would often be to the apps
        "ref.test",
    ] {
        assert_eq!(link(host), None, "{host}");
    }
    assert!(info::host_refuses_framework_routes(factory, "REF.test:443"));
    assert!(!info::host_refuses_framework_routes(factory, "inc.test"));
    assert_eq!(
        info::root_for_host(factory, "inc.test", "webroot")
            .unwrap()
            .framework_routes,
        info::FrameworkRoutes::Include
    );
}

/// A host whose root screen is its public site's root still has the site bound by the conf, and a request that no
/// site route takes has no bound site, whatever it sends.
#[test]
fn a_public_site_at_its_hosts_root_screen_is_routed_and_binds_only_the_confs_site() {
    let tree = Tree::new("site-host");
    let site = tree.path("Site.xml");
    tree.write(
        "Site.xml",
        r#"<screen require-authentication="false"><subscreens default-item="Page"/>
            <widgets><subscreens-active/></widgets></screen>"#,
    );
    tree.write(
        "Site/Page.xml",
        r#"<screen require-authentication="false"><widgets><label text="page of [${context.resolvedSiteId || ''}]"/></widgets></screen>"#,
    );
    let conf = format!(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <root-screen host="site\.test|plain\.test" location="{site}" framework-routes="refuse"/>
            <public-site host="site.test" site-id="SITE_A" screen-location="{site}"><mount path="/" screen="Page"/></public-site>
        </webapp></webapp-list></moqui-conf>"#
    );
    let context = ExecutionContextFactory::test_context_with_conf(&conf);
    let _env = Env::new();
    let factory = context.factory();
    let (status, body) = get(factory, "site.test", "/?resolvedSiteId=EVIL");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("page of [SITE_A]"), "{body}");
    let (status, body) = get(factory, "plain.test", "/Page?resolvedSiteId=EVIL");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("page of []"), "{body}");
}

/// A conf whose root screens cannot be read (a host pattern that is not a valid regex, before the one that would match)
/// refuses the framework's routes on every host: the check fails closed.
#[test]
fn an_unreadable_root_screen_conf_refuses_the_framework_routes() {
    let context = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <root-screen host="[" location="/bad.xml"/>
            <root-screen host="shop\.test" location="/shop.xml" framework-routes="refuse"/>
            <root-screen host=".*" location="/default.xml"/>
        </webapp></webapp-list></moqui-conf>"#,
    );
    assert!(info::host_refuses_framework_routes(context.factory(), "shop.test"));
    assert!(info::host_refuses_framework_routes(context.factory(), "other.test"));
    assert_eq!(info::link_host(context.factory(), "shop.test", "webroot"), None);
}
