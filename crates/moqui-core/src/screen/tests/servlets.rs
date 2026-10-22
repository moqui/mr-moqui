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

//! The webapp's script servlets, filters and listeners and its lifecycle actions, through a whole request
//! (`web/servlet.rs`, `script/runtime/web_host.rs`, docs/Web-Scripts.md).

use std::path::{Path, PathBuf};

use super::{basic, header_value, request, webroot, World};
use crate::screen::serve_at;

/// A directory of scripts for one test, removed when it drops.
struct Scripts(PathBuf);

impl Scripts {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-servlets-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn write(&self, name: &str, text: &str) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scripts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn conf(webapp: &str) -> String {
    format!(
        r#"<moqui-conf><webapp-list><webapp name="webroot">
            <servlet name="MoquiServlet" class="org.moqui.impl.webapp.MoquiServlet"><url-pattern>/*</url-pattern></servlet>
            {webapp}</webapp></webapp-list></moqui-conf>"#
    )
}

fn cached(world: &World, name: &str) -> String {
    let context = world.factory.new_context();
    let _entered = context.enter();
    crate::script::eval_value(&format!("ec.cache.getCache('test.servlets').get('{name}') ?? ''"))
        .map(|value| moqui_lib::object::to_plain_string(&value))
        .unwrap()
}

#[test]
fn a_script_servlet_answers_its_paths_through_its_filters() {
    let scripts = Scripts::new("servlet");
    let servlet = scripts.write(
        "Hello.js",
        r#"({
            doGet(request, response, config) {
                response.setStatus(201)
                response.setHeader("X-Greeting", config.getInitParameter("greeting"))
                response.setContentType("text/plain")
                response.write([request.servletPath, request.pathInfo, request.getParameter("q"), request.getHeader("X-Test"),
                    request.getAttribute("seen"), request.getRequestURL()].join("|"))
            },
            doPut(request, response) { response.write("put " + request.bodyText()) },
        })"#,
    );
    let filter = scripts.write(
        "Mark.js",
        r#"({ doFilter(request, response, chain) {
            request.setAttribute("seen", "marked")
            response.addHeader("X-Order", "mark")
            chain.doFilter(request, response)
            response.setHeader("X-After", String(response.getStatus()))
        } })"#,
    );
    let stop = scripts.write(
        "Stop.js",
        r#"({ doFilter(request, response) { response.setStatus(403); response.write("stopped") } })"#,
    );
    let world = World::open_in_framework(&conf(&format!(
        r#"<filter name="Mark" location="{filter}"><url-pattern>/hello/*</url-pattern></filter>
        <filter name="Stop" location="{stop}" priority="1"><url-pattern>/hello/stop</url-pattern></filter>
        <servlet name="Hello" location="{servlet}"><init-param name="greeting" value="hi"/>
            <url-pattern>/hello/*</url-pattern></servlet>"#
    )));
    let root = webroot().to_string_lossy().into_owned();

    let mut get = request("GET", "/hello/there");
    get.query.push(("q".to_string(), "one".to_string()));
    get.raw_query = Some("q=one".to_string());
    get.set_header("x-test", "yes");
    get.set_header("host", "erp.example:8080");
    let response = serve_at(&world.factory, get, &root);
    assert_eq!(response.status, 201, "{}", response.body_text());
    assert_eq!(
        response.body_text(),
        "/hello|/there|one|yes|marked|http://erp.example:8080/hello/there"
    );
    assert_eq!(header_value(&response, "X-Greeting").as_deref(), Some("hi"));
    assert_eq!(header_value(&response, "X-Order").as_deref(), Some("mark"));
    assert_eq!(header_value(&response, "X-After").as_deref(), Some("201"));
    // the conf's screen-render headers still go on every response
    assert_eq!(
        header_value(&response, "X-Content-Type-Options").as_deref(),
        Some("nosniff")
    );

    let mut put = request("PUT", "/hello");
    put.set_header("content-type", "text/plain");
    put.body = b"some text".to_vec();
    let response = serve_at(&world.factory, put, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(response.body_text(), "put some text");

    let response = serve_at(&world.factory, request("POST", "/hello/x"), &root);
    assert_eq!(response.status, 405, "{}", response.body_text());

    let response = serve_at(&world.factory, request("GET", "/hello/stop"), &root);
    assert_eq!(response.status, 403);
    assert_eq!(response.body_text(), "stopped", "the servlet never ran");
    assert_eq!(header_value(&response, "X-Order"), None, "a later filter never ran");

    // the screens are the MoquiServlet's still: an anonymous visit to the root goes to Login
    let response = serve_at(&world.factory, request("GET", "/"), &root);
    assert_eq!(response.status, 302, "{}", response.body_text());
}

#[test]
fn a_filter_wraps_the_screens_and_a_servlet_error_is_its_requests_only() {
    let scripts = Scripts::new("wrap");
    let filter = scripts.write(
        "Wrap.js",
        r#"({ doFilter(request, response, chain) { chain.doFilter(); response.setHeader("X-Wrapped", "yes") } })"#,
    );
    let broken = scripts.write("Broken.js", r#"({ doGet() { throw new Error("broken servlet") } })"#);
    let not_object = scripts.write("Plain.js", r#"const x = 1"#);
    let world = World::open_in_framework(&conf(&format!(
        r#"<filter name="Wrap" location="{filter}"><url-pattern>/*</url-pattern></filter>
        <servlet name="Broken" location="{broken}"><url-pattern>/broken</url-pattern></servlet>
        <servlet name="Plain" location="{not_object}"><url-pattern>/plain</url-pattern></servlet>
        <servlet name="Java" class="com.example.NotHere"><url-pattern>/java/*</url-pattern></servlet>"#
    )));
    let root = webroot().to_string_lossy().into_owned();
    let response = serve_at(&world.factory, request("GET", "/"), &root);
    assert_eq!(response.status, 302, "{}", response.body_text());
    assert_eq!(header_value(&response, "Location").as_deref(), Some("/Login"));
    assert_eq!(header_value(&response, "X-Wrapped").as_deref(), Some("yes"));

    let response = serve_at(&world.factory, request("GET", "/broken"), &root);
    assert_eq!(response.status, 500);
    assert!(
        response.body_text().contains("broken servlet"),
        "{}",
        response.body_text()
    );
    let response = serve_at(&world.factory, request("GET", "/plain"), &root);
    assert_eq!(response.status, 500);
    assert!(
        response.body_text().contains("handler object"),
        "{}",
        response.body_text()
    );
}

#[test]
fn the_lifecycle_actions_and_listeners_run_at_their_points() {
    let scripts = Scripts::new("lifecycle");
    let listener = scripts.write(
        "Listen.js",
        r#"const cache = () => ec.cache.getCache('test.servlets')
        ;({
            requestInitialized(event) { cache().put('initialized', (cache().get('initialized') ?? 0) + 1) },
            requestDestroyed(event) { cache().put('destroyed', event.type) },
            sessionCreated(event) { cache().put('session', event.sessionId ? 'created' : 'no id') },
        })"#,
    );
    let ping = scripts.write(
        "Ping.js",
        r#"({ doGet(request, response) {
            if (request.getParameter("out")) ec.user.logoutUser()
            response.write("pong")
        } })"#,
    );
    let world = World::open_in_framework(&conf(&format!(
        r#"<listener location="{listener}"/>
        <servlet name="Ping" location="{ping}"><url-pattern>/ping</url-pattern></servlet>
        <before-request><actions><script>
            if (ec.web.requestParameters.fail === 'before') throw new Error('refused before the request')
            ec.web.response.setHeader('X-Before', ec.web.request.getRequestURI())
        </script></actions></before-request>
        <after-request><actions><script>
            ec.cache.getCache('test.servlets').put('after', ec.web.request.getRequestURI())
            if (ec.web.requestParameters.fail === 'after') throw new Error('after the request')
        </script></actions></after-request>
        <after-login><actions><script>ec.cache.getCache('test.servlets').put('login', ec.user.username)</script></actions></after-login>
        <before-logout><actions><script>ec.cache.getCache('test.servlets').put('logout', ec.user.username)</script></actions></before-logout>
        <first-hit-in-visit><actions><script>ec.cache.getCache('test.servlets').put('visit', ec.user.visitId ?? 'none')</script></actions></first-hit-in-visit>"#
    )));
    world.user("ann", None);
    let root = webroot().to_string_lossy().into_owned();

    let response = serve_at(&world.factory, request("GET", "/"), &root);
    assert_eq!(response.status, 302, "{}", response.body_text());
    assert_eq!(header_value(&response, "X-Before").as_deref(), Some("/"));
    assert_eq!(cached(&world, "after"), "/");
    assert_eq!(cached(&world, "initialized"), "1");
    assert_eq!(cached(&world, "destroyed"), "requestDestroyed");
    assert_eq!(cached(&world, "session"), "created");

    let mut failing = request("GET", "/Login");
    failing.query.push(("fail".to_string(), "before".to_string()));
    let response = serve_at(&world.factory, failing, &root);
    assert_eq!(response.status, 500, "a before-request failure fails the request");
    assert_eq!(
        cached(&world, "after"),
        "/Login",
        "after-request runs however the request ended"
    );

    let mut after = request("GET", "/");
    after.query.push(("fail".to_string(), "after".to_string()));
    let response = serve_at(&world.factory, after, &root);
    assert_eq!(response.status, 302, "an after-request failure is only logged");

    let mut login = request("GET", "/ping");
    login.set_header("authorization", basic("ann"));
    let response = serve_at(&world.factory, login, &root);
    assert_eq!(response.body_text(), "pong");
    assert_eq!(cached(&world, "login"), "ann");
    // the sign in starts the visit a request with no cookie put off
    assert!(!cached(&world, "visit").is_empty(), "first-hit-in-visit ran");

    let mut logout = request("GET", "/ping");
    logout.query.push(("out".to_string(), "Y".to_string()));
    logout.set_header("authorization", basic("ann"));
    let response = serve_at(&world.factory, logout, &root);
    assert_eq!(response.body_text(), "pong");
    assert_eq!(
        cached(&world, "logout"),
        "ann",
        "before-logout runs while the user is there"
    );
    drop(scripts);
}

#[test]
fn the_shipped_auth_filter_wants_a_user_with_the_permission() {
    let scripts = Scripts::new("auth");
    let servlet = scripts.write("Ok.js", r#"({ service(request, response) { response.write("in") } })"#);
    let world = World::open_in_framework(&conf(&format!(
        r#"<filter name="Auth" class="org.moqui.impl.webapp.MoquiAuthFilter">
            <init-param name="permission" value="SearchRemote"/><url-pattern>/search/*</url-pattern></filter>
        <servlet name="Ok" location="{servlet}"><url-pattern>/search/*</url-pattern></servlet>"#
    )));
    world.user("ann", None);
    world.user("root", Some("ADMIN"));
    let mut permission = world.facade.make_value("moqui.security.UserPermission").unwrap();
    permission.set("userPermissionId", moqui_lib::object::Value::string("SearchRemote"));
    permission.create(&world.facade).unwrap();
    let mut grant = world.facade.make_value("moqui.security.UserGroupPermission").unwrap();
    grant.set("userGroupId", moqui_lib::object::Value::string("ADMIN"));
    grant.set("userPermissionId", moqui_lib::object::Value::string("SearchRemote"));
    grant.set("fromDate", super::stamp());
    grant.create(&world.facade).unwrap();
    let root = webroot().to_string_lossy().into_owned();

    let response = serve_at(&world.factory, request("GET", "/search/x"), &root);
    assert_eq!(response.status, 401, "{}", response.body_text());
    assert!(header_value(&response, "WWW-Authenticate").is_some_and(|value| value.starts_with("Basic")));
    let mut ann = request("GET", "/search/x");
    ann.set_header("authorization", basic("ann"));
    assert_eq!(serve_at(&world.factory, ann, &root).status, 403);
    let mut admin = request("GET", "/search/x");
    admin.set_header("authorization", basic("root"));
    let response = serve_at(&world.factory, admin, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(response.body_text(), "in");
    assert!(scripts.path().exists());
}
