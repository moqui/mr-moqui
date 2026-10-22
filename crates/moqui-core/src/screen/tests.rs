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

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, MutexGuard};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use moqui_lib::object::Value;

use super::{locate, serve_at};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::web::WebRequest;

fn webroot() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/webroot/screen/webroot.xml")
}

fn tools_rest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/tools/service/moqui.rest.xml")
}

fn test_cookie_key() -> String {
    STANDARD.encode([0x09_u8; 32])
}

struct Env {
    _lock: MutexGuard<'static, ()>,
    names: Vec<String>,
}

impl Env {
    fn new() -> Self {
        let lock = crate::user::session::cookie_property_lock();
        moqui_lib::env::set_property("session_cookie_key", test_cookie_key());
        Self {
            _lock: lock,
            names: vec!["session_cookie_key".to_string()],
        }
    }

    fn set(&mut self, name: &str, value: &str) {
        moqui_lib::env::set_property(name, value);
        if !self.names.iter().any(|existing| existing == name) {
            self.names.push(name.to_string());
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for name in &self.names {
            moqui_lib::env::clear_property(name);
        }
    }
}

struct World {
    _env: Env,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
}

impl World {
    fn open(conf: &str) -> Self {
        Self::open_with(ExecutionContextFactory::test_context_with_conf(conf))
    }

    /// [`World::open`] with the framework directory, so `component://` locations of the base components resolve.
    fn open_in_framework(conf: &str) -> Self {
        Self::open_with(ExecutionContextFactory::test_context_in(
            conf,
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        ))
    }

    fn open_with(context: crate::context::ExecutionContext) -> Self {
        let env = Env::new();
        let facade = crate::testing::open();
        context.factory().set_entity(facade.clone());
        let entered = context.enter();
        let _authz = context.disable_authz();
        let _ = (&entered, &_authz);
        Self {
            _env: env,
            factory: Arc::clone(context.factory()),
            facade,
        }
    }

    fn load_service_rest(&self) {
        crate::service::install(&self.factory, &[tools_rest()]);
    }

    fn user(&self, username: &str, group: Option<&str>) {
        let stored = crate::user::hash_password("secret").unwrap();
        let mut account = self.facade.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", Value::string(username));
        account.set("username", Value::string(username));
        account.set("currentPassword", Value::string(stored));
        account.set("passwordHashType", Value::string("argon2"));
        account.create(&self.facade).unwrap();
        if let Some(group) = group {
            let mut member = self.facade.make_value("moqui.security.UserGroupMember").unwrap();
            member.set("userId", Value::string(username));
            member.set("userGroupId", Value::string(group));
            member.set("fromDate", stamp());
            member.create(&self.facade).unwrap();
        }
    }

    /// The two `REST_SCHEMA` rows in `MoquiSetupData.xml`: the permission, and the `ADMIN` grant.
    fn grant_rest_schema(&self) {
        let mut permission = self.facade.make_value("moqui.security.UserPermission").unwrap();
        permission.set("userPermissionId", Value::string("REST_SCHEMA"));
        permission.set("description", Value::string("REST schema dumps"));
        permission.create(&self.facade).unwrap();
        let mut grant = self.facade.make_value("moqui.security.UserGroupPermission").unwrap();
        grant.set("userGroupId", Value::string("ADMIN"));
        grant.set("userPermissionId", Value::string("REST_SCHEMA"));
        grant.set("fromDate", stamp());
        grant.create(&self.facade).unwrap();
    }

    fn allow_entity(&self, group: &str) {
        let mut member = self.facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
        member.set("artifactGroupId", Value::string("ENUM_TYPE"));
        member.set("artifactName", Value::string("moqui.basic.EnumerationType"));
        member.set("artifactTypeEnumId", Value::string("AT_ENTITY"));
        member.set("nameIsPattern", Value::string("N"));
        member.set("inheritAuthz", Value::string("Y"));
        member.create(&self.facade).unwrap();
        let mut authz = self.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
        authz.set("artifactAuthzId", Value::string("ENUM_TYPE"));
        authz.set("userGroupId", Value::string(group));
        authz.set("artifactGroupId", Value::string("ENUM_TYPE"));
        authz.set("authzTypeEnumId", Value::string("AUTHZT_ALWAYS"));
        authz.set("authzActionEnumId", Value::string("AUTHZA_ALL"));
        authz.create(&self.facade).unwrap();
    }

    /// Allow `group` to view the screen at `location`.
    fn allow_screen(&self, group: &str, location: &str) {
        self.grant_screen("SCREEN_VIEW", group, location, "AUTHZT_ALWAYS");
    }

    /// An inheritable view record of `authz_type` for `group` on the screen at `location`, with `id` as both the
    /// artifact group and the authorization id.
    fn grant_screen(&self, id: &str, group: &str, location: &str, authz_type: &str) {
        let mut member = self.facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
        member.set("artifactGroupId", Value::string(id));
        member.set("artifactName", Value::string(location));
        member.set("artifactTypeEnumId", Value::string("AT_XML_SCREEN"));
        member.set("nameIsPattern", Value::string("N"));
        member.set("inheritAuthz", Value::string("Y"));
        member.create(&self.facade).unwrap();
        let mut authz = self.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
        authz.set("artifactAuthzId", Value::string(id));
        authz.set("userGroupId", Value::string(group));
        authz.set("artifactGroupId", Value::string(id));
        authz.set("authzTypeEnumId", Value::string(authz_type));
        authz.set("authzActionEnumId", Value::string("AUTHZA_VIEW"));
        authz.create(&self.facade).unwrap();
    }

    fn enumeration_type(&self) {
        let mut row = self.facade.make_value("moqui.basic.EnumerationType").unwrap();
        row.set("enumTypeId", Value::string("TestType"));
        row.set("description", Value::string("Test"));
        row.create(&self.facade).unwrap();
    }
}

fn stamp() -> Value {
    moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value
}

fn authz_conf() -> &'static str {
    "<moqui-conf><artifact-execution-facade><artifact-execution type=\"AT_ENTITY\" authz-enabled=\"true\"/></artifact-execution-facade></moqui-conf>"
}

fn basic(username: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{username}:secret")))
}

fn request(method: &str, path: &str) -> WebRequest {
    let mut request = WebRequest::new(method, path);
    request.peer_ip = "127.0.0.1".to_string();
    request
}

fn header_value(response: &crate::web::WebResponse, name: &str) -> Option<String> {
    response.header(name).map(str::to_string)
}

#[test]
fn rest_service_path_lands_on_s1() {
    let context = ExecutionContextFactory::test_context();
    let hit = locate(
        context.factory(),
        &webroot().to_string_lossy(),
        "/rest/s1/moqui/basic/enumTypes",
    )
    .unwrap();
    assert_eq!(hit.transition.as_deref(), Some("s1"));
    assert_eq!(hit.extra, vec!["moqui", "basic", "enumTypes"]);
    assert_eq!(hit.screen, "webroot/rest");
}

#[test]
fn login_method_parameter_and_csrf_misses() {
    let world = World::open("");
    let root = webroot().to_string_lossy().into_owned();
    let response = serve_at(&world.factory, request("GET", "/rest/login"), &root);
    assert_eq!(response.status, 405, "{}", response.body_text());
    assert_eq!(header_value(&response, "Allow").as_deref(), Some("POST"));

    let mut missing = request("POST", "/rest/login");
    missing.set_header("content-type", "application/x-www-form-urlencoded");
    missing.body = b"password=secret".to_vec();
    let response = serve_at(&world.factory, missing, &root);
    assert_eq!(response.status, 400, "{}", response.body_text());
    assert!(response.body_text().contains("username"), "{}", response.body_text());

    // A session that came with the request has a token, and a post without it is refused. (A session made by the
    // request itself has not sent its token yet, so Java does not check one then.)
    let first = serve_at(&world.factory, request("GET", "/rest/login"), &root);
    let cookie = header_value(&first, "Set-Cookie").expect("session cookie");
    let session = cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_string();
    let mut otp = request("POST", "/rest/sendOtp");
    otp.set_header("content-type", "application/x-www-form-urlencoded");
    otp.body = b"factorId=EMAIL".to_vec();
    otp.session_cookie = Some(session);
    let response = serve_at(&world.factory, otp, &root);
    assert_eq!(response.status, 401, "{}", response.body_text());
}

#[test]
fn guarantee_mode_flushes_the_session_of_a_failed_request() {
    let world = World::open("");
    world
        .factory
        .sessions
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .mode = crate::user::session::FlushMode::Guarantee;
    let root = webroot().to_string_lossy().into_owned();
    let response = serve_at(&world.factory, request("GET", "/no/such/screen"), &root);
    assert_eq!(response.status, 404, "{}", response.body_text());
    let cookie = header_value(&response, "Set-Cookie").expect("session cookie");
    let value = cookie.split(';').next().unwrap().split_once('=').unwrap().1;
    let session_id = crate::user::verify_session_cookie(value).unwrap().expect("session id");
    let row = world
        .facade
        .find("moqui.server.HttpSession")
        .unwrap()
        .eq("sessionId", Value::string(&session_id))
        .one()
        .unwrap();
    assert!(row.is_some(), "the failed request's session was not written");
}

#[test]
fn login_sets_a_cookie_and_the_next_request_is_that_user() {
    let world = World::open("");
    world.user("admin", None);
    world.enumeration_type();
    let root = webroot().to_string_lossy().into_owned();
    let mut login = request("POST", "/rest/login");
    login.set_header("content-type", "application/x-www-form-urlencoded");
    login.body = b"username=admin&password=secret".to_vec();
    let response = serve_at(&world.factory, login, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(
        response.body_text().contains("\"loggedIn\" : true"),
        "{}",
        response.body_text()
    );
    let set_cookie = header_value(&response, "Set-Cookie").expect("session cookie");
    assert!(set_cookie.starts_with("moqui.session="), "{set_cookie}");
    let value = set_cookie
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_string();

    let mut again = request("GET", "/rest/e1/moqui.basic.EnumerationType");
    again.session_cookie = Some(value);
    let response = serve_at(&world.factory, again, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("TestType"), "{}", response.body_text());
    assert!(
        header_value(&response, "X-Total-Count").is_some(),
        "{:?}",
        response.headers
    );
    assert_eq!(header_value(&response, "Authorization"), None);
}

#[test]
fn anonymous_entity_rest_is_401_and_a_user_without_an_allow_is_403() {
    let world = World::open(authz_conf());
    world.load_service_rest();
    world.user("admin", Some("ADMIN"));
    world.user("plain", None);
    world.allow_entity("ADMIN");
    world.enumeration_type();
    let root = webroot().to_string_lossy().into_owned();

    let response = serve_at(
        &world.factory,
        request("GET", "/rest/e1/moqui.basic.EnumerationType"),
        &root,
    );
    assert_eq!(response.status, 401, "{}", response.body_text());

    let mut allowed = request("GET", "/rest/s1/moqui/basic/enumTypes");
    allowed.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, allowed, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("TestType"), "{}", response.body_text());

    let mut denied = request("GET", "/rest/e1/moqui.basic.EnumerationType");
    denied.set_header("authorization", basic("plain"));
    let response = serve_at(&world.factory, denied, &root);
    assert_eq!(response.status, 403, "{}", response.body_text());
}

/// As Java's `effectiveRequestMethod`, `X-HTTP-Method-Override` on a POST is the method for `/rest/s1` too, and the
/// REST path is authorized for that method: a view-only user reads with an override `GET` and is refused a `PUT`.
#[test]
fn service_rest_takes_the_method_override_and_authorizes_it() {
    let world = World::open(
        "<moqui-conf><artifact-execution-facade>\
            <artifact-execution type=\"AT_REST_PATH\" authz-enabled=\"true\"/>\
            <artifact-execution type=\"AT_ENTITY\" authz-enabled=\"true\"/>\
        </artifact-execution-facade></moqui-conf>",
    );
    world.load_service_rest();
    world.user("viewer", Some("VIEWERS"));
    world.enumeration_type();
    let mut member = world.facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
    member.set("artifactGroupId", Value::string("REST_VIEW"));
    member.set("artifactName", Value::string("/moqui"));
    member.set("artifactTypeEnumId", Value::string("AT_REST_PATH"));
    member.set("nameIsPattern", Value::string("N"));
    member.set("inheritAuthz", Value::string("Y"));
    member.create(&world.facade).unwrap();
    let mut authz = world.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
    authz.set("artifactAuthzId", Value::string("REST_VIEW"));
    authz.set("userGroupId", Value::string("VIEWERS"));
    authz.set("artifactGroupId", Value::string("REST_VIEW"));
    authz.set("authzTypeEnumId", Value::string("AUTHZT_ALWAYS"));
    authz.set("authzActionEnumId", Value::string("AUTHZA_VIEW"));
    authz.create(&world.facade).unwrap();
    let root = webroot().to_string_lossy().into_owned();
    let send = |method: &str, over: Option<&str>| {
        let mut call = request(method, "/rest/s1/moqui/basic/enumTypes");
        call.set_header("authorization", basic("viewer"));
        if let Some(over) = over {
            call.set_header("x-http-method-override", over);
        }
        serve_at(&world.factory, call, &root)
    };

    let response = send("GET", None);
    assert_eq!(response.status, 200, "{}", response.body_text());
    let response = send("POST", Some("GET"));
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("TestType"), "{}", response.body_text());
    // Authorized as an update, which the user may not do, before the method is looked for.
    let response = send("POST", Some("PUT"));
    assert_eq!(response.status, 403, "{}", response.body_text());
    let response = send("POST", None);
    assert_eq!(response.status, 403, "{}", response.body_text());
}

#[test]
fn redirect_json_list_security_headers_and_cors() {
    let mut world = World::open("");
    let path = std::env::temp_dir().join(format!("moqui-screen-redirect-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<screen require-authentication="false">
            <transition name="go"><default-response type="url" url="/next"/></transition>
        </screen>"#,
    )
    .unwrap();
    let root = path.to_string_lossy().into_owned();
    let response = serve_at(&world.factory, request("GET", "/go"), &root);
    assert_eq!(response.status, 302, "{}", response.body_text());
    assert_eq!(header_value(&response, "Location").as_deref(), Some("/next"));
    let _ = std::fs::remove_file(&path);

    let web = webroot().to_string_lossy().into_owned();
    // A JSON array that is not a list of objects is `_requestBodyJsonList`, as in Java, not a refused body: the
    // request goes on, here to the REST screen's own check that there is a user.
    let mut bad_json = request("POST", "/rest/e1/moqui.basic.EnumerationType");
    bad_json.set_header("content-type", "application/json");
    bad_json.body = b"[1]".to_vec();
    let response = serve_at(&world.factory, bad_json, &web);
    assert_eq!(response.status, 401, "{}", response.body_text());

    let mut https = request("GET", "/");
    https.https = true;
    let response = serve_at(&world.factory, https, &web);
    // The root's default screen redirects an anonymous user to Login, which a render sends as 302.
    assert_eq!(response.status, 302, "{}", response.body_text());
    assert_eq!(header_value(&response, "Location").as_deref(), Some("/Login"));
    assert!(header_value(&response, "Cache-Control").unwrap().contains("no-store"));
    assert!(header_value(&response, "Content-Security-Policy")
        .unwrap()
        .contains("frame-ancestors"));
    assert_eq!(
        header_value(&response, "X-Frame-Options").as_deref(),
        Some("sameorigin")
    );
    assert_eq!(
        header_value(&response, "X-Content-Type-Options").as_deref(),
        Some("nosniff")
    );
    assert_eq!(
        header_value(&response, "X-XSS-Protection").as_deref(),
        Some("1; mode=block")
    );
    assert!(header_value(&response, "Strict-Transport-Security").is_some());

    world._env.set("webapp_allow_origins", "https://app.example");
    let mut cors = request("GET", "/");
    cors.set_header("origin", "https://app.example");
    let response = serve_at(&world.factory, cors, &web);
    assert_eq!(
        header_value(&response, "Access-Control-Allow-Origin").as_deref(),
        Some("https://app.example")
    );
    assert_eq!(
        header_value(&response, "Access-Control-Allow-Credentials").as_deref(),
        Some("true")
    );

    let mut options = request("OPTIONS", "/rest/s1/moqui/basic/enumTypes");
    options.set_header("origin", "https://app.example");
    let response = serve_at(&world.factory, options, &web);
    assert_eq!(response.status, 204, "{}", response.body_text());
    assert!(header_value(&response, "Access-Control-Allow-Headers")
        .unwrap()
        .contains("api_key"));
}

/// The webapp's `response-header` entries and its `handle-cors` and `allow-origins` attributes come from the conf, as
/// Java's `WebappInfo` reads them; the properties are only the fallback.
#[test]
fn the_webapp_conf_sets_the_response_headers_and_cors() {
    let web = webroot().to_string_lossy().into_owned();
    let world = World::open(
        r#"<moqui-conf><webapp-list><webapp name="webroot" allow-origins="https://conf.example">
            <response-header type="screen-render" name="Content-Security-Policy" value="default-src 'self'"/>
            <response-header type="screen-render" name="X-Extra" value="yes"/>
        </webapp></webapp-list></moqui-conf>"#,
    );
    let mut get = request("GET", "/");
    get.set_header("origin", "https://conf.example");
    let response = serve_at(&world.factory, get, &web);
    assert_eq!(
        header_value(&response, "Content-Security-Policy").as_deref(),
        Some("default-src 'self'")
    );
    assert_eq!(header_value(&response, "X-Extra").as_deref(), Some("yes"));
    assert_eq!(
        header_value(&response, "Access-Control-Allow-Origin").as_deref(),
        Some("https://conf.example"),
        "allow-origins from the webapp element"
    );
    drop(world);

    let mut world = World::open(
        r#"<moqui-conf><webapp-list><webapp name="webroot" handle-cors="false"/></webapp-list></moqui-conf>"#,
    );
    world._env.set("webapp_allow_origins", "https://app.example");
    let mut get = request("GET", "/");
    get.set_header("origin", "https://app.example");
    let response = serve_at(&world.factory, get, &web);
    assert_eq!(
        header_value(&response, "Access-Control-Allow-Origin"),
        None,
        "handle-cors=false"
    );
    // A webapp that declares no response-header gets the shipped ones.
    assert_eq!(
        header_value(&response, "X-Frame-Options").as_deref(),
        Some("sameorigin")
    );
}

/// A login form posted with no username or password fails as a wrong password does, back to the Login screen with the
/// message, not a 500 from `loginUser(null, null)`; and it tried no password, so web protection does not count it.
#[test]
fn a_blank_login_is_a_failed_login_not_an_error() {
    let world = World::open_in_framework("");
    let root = webroot().to_string_lossy().into_owned();
    let _ = crate::web::take_auth_failure();
    let response = serve_at(&world.factory, request("POST", "/Login/login"), &root);
    assert_ne!(response.status, 500, "{}", response.body_text());
    assert!(!crate::web::take_auth_failure(), "a blank form is not a failed guess");
    // over JSON the answer is the failed transition's, the same as for a wrong password
    let json = |body: &[u8]| {
        let mut call = request("POST", "/Login/login");
        call.set_header("accept", "application/json");
        call.set_header("content-type", "application/json");
        call.body = body.to_vec();
        serve_at(&world.factory, call, &root)
    };
    let blank = json(br#"{"username": null}"#);
    let wrong = json(br#"{"username": "nobody", "password": "wrong"}"#);
    assert_eq!(blank.status, wrong.status, "{}", blank.body_text());
    assert!(
        blank
            .body_text()
            .contains("\"errors\" : \"The username or password is not valid\""),
        "{}",
        blank.body_text()
    );
}

/// As `MoquiServlet.handleCors`: a request from an origin CORS does not allow is answered 401 before it runs; an
/// allowed origin, the request's own host and no origin at all go on.
#[test]
fn a_disallowed_origin_is_refused_before_the_request_runs() {
    let mut world = World::open("");
    let path = std::env::temp_dir().join(format!("moqui-screen-origin-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<screen require-authentication="false">
            <transition name="go"><default-response type="url" url="/next"/></transition>
        </screen>"#,
    )
    .unwrap();
    let root = path.to_string_lossy().into_owned();
    world._env.set("webapp_allow_origins", "https://app.example");
    let send = |method: &str, origin: Option<&str>| {
        let mut call = request(method, "/go");
        call.set_header("host", "erp.example:8080");
        if let Some(origin) = origin {
            call.set_header("origin", origin);
        }
        serve_at(&world.factory, call, &root)
    };
    for method in ["GET", "POST", "OPTIONS"] {
        let response = send(method, Some("https://evil.example"));
        assert_eq!(response.status, 401, "{method} {}", response.body_text());
        assert!(
            response.body_text().contains("Origin not allowed"),
            "{}",
            response.body_text()
        );
        assert_eq!(header_value(&response, "Access-Control-Allow-Origin"), None);
    }
    assert_eq!(send("GET", Some("https://app.example")).status, 302);
    assert_eq!(send("GET", Some("http://erp.example:8080")).status, 302);
    assert_eq!(send("GET", None).status, 302);
    assert_eq!(send("OPTIONS", Some("https://app.example")).status, 204);

    drop(world);

    // behind a proxy that rewrites Host: the forwarded host of a believed proxy, or the conf webapp's host
    let world = World::open(
        r#"<moqui-conf><webapp-list><webapp name="webroot" client-ip-header="x-forwarded-for"
            trusted-proxies="127.0.0.1" http-host="erp.example"/></webapp-list></moqui-conf>"#,
    );
    let mut proxied = request("POST", "/go");
    proxied.set_header("host", "127.0.0.1:8080");
    proxied.set_header("x-forwarded-host", "shop.example");
    proxied.set_header("origin", "https://shop.example");
    assert_eq!(serve_at(&world.factory, proxied.clone(), &root).status, 302);
    proxied.peer_ip = "10.1.1.1".to_string();
    assert_eq!(
        serve_at(&world.factory, proxied, &root).status,
        401,
        "not from a trusted proxy"
    );
    let mut configured = request("POST", "/go");
    configured.set_header("host", "127.0.0.1:8080");
    configured.set_header("origin", "https://erp.example");
    assert_eq!(serve_at(&world.factory, configured, &root).status, 302);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn script_send_json_response_writes_the_web_slot() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let _web = crate::web::install(BTreeMap::new());
    crate::script::run_script("ec.web.sendJsonResponse({loggedIn:true})").unwrap();
    let (status, _headers, body, sent) = crate::web::take_output();
    assert!(sent);
    assert_eq!(status, 200);
    let text = String::from_utf8(body).unwrap();
    assert!(text.contains("loggedIn"), "{text}");
}

/// As Java's `sendTextResponseInternal`: a text response sent while the message facade has an error is the errors,
/// with a 500.
#[test]
fn script_send_text_response_with_an_error_sends_the_errors() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let _web = crate::web::install(BTreeMap::new());
    crate::script::run_script("ec.message.addError('No such order'); ec.web.sendTextResponse('all good')").unwrap();
    let (status, _headers, body, sent) = crate::web::take_output();
    assert!(sent);
    assert_eq!(status, 500);
    assert_eq!(String::from_utf8(body).unwrap(), "No such order\n");
}

/// P8-08: in a request a path-only link is empty, and a full URL is the conf webapp's, never the request's `Host`.
#[test]
fn webapp_root_url_is_the_conf_webapp_and_never_the_host_header() {
    let context = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><webapp-list><webapp name="webroot" http-host="erp.example" http-port="8080"
            https-host="erp.example" https-port="8443"/></webapp-list></moqui-conf>"#,
    );
    let _entered = context.enter();
    let _web = crate::web::install(BTreeMap::new());
    let mut request = WebRequest::new("GET", "/apps/ship");
    request.set_header("host", "attacker.example");
    crate::web::set_request_info(&request);
    let conf = context.factory().webapp_root_url("webroot");
    assert_eq!(conf, "http://erp.example:8080");
    let value = |script: &str| {
        crate::script::eval_value(script)
            .map(|value| moqui_lib::object::to_plain_string(&value))
            .unwrap()
    };
    assert_eq!(value("ec.web.getWebappRootUrl(false, null)"), "");
    assert_eq!(value("ec.web.getWebappRootUrl()"), "");
    assert_eq!(value("ec.web.getWebappRootUrl(true, null)"), conf);
    assert_eq!(value("ec.web.getWebappRootUrl('webroot', null, true, false)"), conf);
    // An https link asked for from an http request needs the full URL; with no https-enabled it stays http.
    assert_eq!(value("ec.web.getWebappRootUrl(false, true)"), conf);
    let mut secure = WebRequest::new("GET", "/apps/ship");
    secure.https = true;
    crate::web::set_request_info(&secure);
    assert_eq!(value("ec.web.getWebappRootUrl(false, null)"), "");
    assert_eq!(value("ec.web.getWebappRootUrl(false, false)"), conf);
}

#[test]
fn script_send_error_follows_accept() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    let _web = crate::web::install(BTreeMap::new());
    let mut request = WebRequest::new("GET", "/apps/ship");
    request.set_header("accept", "application/json");
    crate::web::set_request_info(&request);
    crate::script::run_script("ec.web.sendError(404, 'No labelUrl', null)").unwrap();
    let (status, headers, body, sent) = crate::web::take_output();
    assert!(sent);
    assert_eq!(status, 404);
    assert_eq!(
        headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str()),
        Some("application/json;charset=UTF-8")
    );
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "{'message':'No labelUrl','errorName':'Not Found','error':404,'path':'/apps/ship'}"
    );
}

#[test]
fn entity_json_has_the_primary_key_and_the_service_document_lists_enum_types() {
    let world = World::open("");
    world.load_service_rest();
    world.user("admin", Some("ADMIN"));
    world.grant_rest_schema();
    let root = webroot().to_string_lossy().into_owned();

    let anonymous = serve_at(
        &world.factory,
        request("GET", "/rest/entity.json/moqui.basic.EnumerationType"),
        &root,
    );
    assert_eq!(anonymous.status, 401, "{}", anonymous.body_text());

    let mut entity = request("GET", "/rest/entity.json/moqui.basic.EnumerationType");
    entity.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, entity, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("enumTypeId"), "{}", response.body_text());
    // As Java's RestSchemaUtil sends it: the schema type, and the document alone with no messages merged in.
    assert_eq!(
        header_value(&response, "Content-Type").as_deref(),
        Some("application/schema+json;charset=UTF-8")
    );
    let document: serde_json::Value = serde_json::from_str(&response.body_text()).unwrap();
    assert!(document.get("messages").is_none(), "{document}");

    let mut all = request("GET", "/rest/entity.json");
    all.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, all, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(
        response.body_text().contains("moqui.basic.EnumerationType"),
        "catalog missing EnumerationType"
    );
    assert!(
        response.body_text().contains("moqui.security.UserAccount"),
        "catalog missing UserAccount"
    );

    let mut master = request("GET", "/rest/master.json/moqui.basic.Enumeration");
    master.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, master, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("enumId"), "{}", response.body_text());

    let mut service = request("GET", "/rest/service.swagger/moqui.json");
    service.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, service, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(
        header_value(&response, "Content-Type").as_deref(),
        Some("application/json;charset=UTF-8")
    );
    assert!(
        response.body_text().contains("\"openapi\" : \"3.1.0\""),
        "{}",
        response.body_text()
    );
    assert!(
        response.body_text().contains("\"/moqui/basic/enumTypes\""),
        "{}",
        response.body_text()
    );

    let mut yaml = request("GET", "/rest/service.swagger/moqui.yaml");
    yaml.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, yaml, &root);
    assert_eq!(response.status, 404, "{}", response.body_text());
}

/// The schema documents need the `REST_SCHEMA` permission, as Java's `RestSchemaUtil.requireSchemaAccess`
/// does: a user without it is refused with 403, and nobody with 401.
#[test]
fn the_rest_schema_documents_need_the_rest_schema_permission() {
    let world = World::open("");
    world.load_service_rest();
    world.user("admin", None);
    let root = webroot().to_string_lossy().into_owned();

    let anonymous = serve_at(
        &world.factory,
        request("GET", "/rest/entity.json/moqui.basic.EnumerationType"),
        &root,
    );
    assert_eq!(anonymous.status, 401, "{}", anonymous.body_text());

    let mut entity = request("GET", "/rest/entity.json/moqui.basic.EnumerationType");
    entity.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, entity, &root);
    assert_eq!(response.status, 403, "{}", response.body_text());
    assert!(
        response.body_text().contains("Not authorized for REST schema"),
        "{}",
        response.body_text()
    );

    for path in [
        "/rest/master.json/moqui.basic.Enumeration",
        "/rest/service.swagger/moqui.json",
    ] {
        let mut document = request("GET", path);
        document.set_header("authorization", basic("admin"));
        let response = serve_at(&world.factory, document, &root);
        assert_eq!(response.status, 403, "{path}: {}", response.body_text());
    }

    // admin in ADMIN, and the REST_SCHEMA grant: the document is served.
    let mut member = world.facade.make_value("moqui.security.UserGroupMember").unwrap();
    member.set("userId", Value::string("admin"));
    member.set("userGroupId", Value::string("ADMIN"));
    member.set("fromDate", stamp());
    member.create(&world.facade).unwrap();
    world.grant_rest_schema();
    let mut allowed = request("GET", "/rest/entity.json/moqui.basic.EnumerationType");
    allowed.set_header("authorization", basic("admin"));
    let response = serve_at(&world.factory, allowed, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("enumTypeId"), "{}", response.body_text());
}

#[test]
fn basic_auth_writes_skip_the_session_token_but_a_cookie_session_does_not() {
    let world = World::open("");
    world.user("admin", None);
    world.enumeration_type();
    let root = webroot().to_string_lossy().into_owned();

    let mut stateless = request("POST", "/rest/e1/moqui.basic.EnumerationType");
    stateless.set_header("authorization", basic("admin"));
    stateless.set_header("content-type", "application/json");
    stateless.body = br#"{"enumTypeId":"BasicPost","description":"Basic POST"}"#.to_vec();
    let response = serve_at(&world.factory, stateless, &root);
    assert_ne!(response.status, 401, "{}", response.body_text());

    let mut login = request("POST", "/rest/login");
    login.set_header("content-type", "application/x-www-form-urlencoded");
    login.body = b"username=admin&password=secret".to_vec();
    let response = serve_at(&world.factory, login, &root);
    let cookie = header_value(&response, "Set-Cookie").expect("cookie");
    let value = cookie.split(';').next().unwrap().split_once('=').unwrap().1.to_string();
    let mut ambient = request("POST", "/rest/e1/moqui.basic.EnumerationType");
    ambient.session_cookie = Some(value);
    ambient.set_header("content-type", "application/json");
    ambient.body = br#"{"enumTypeId":"CookiePost","description":"Cookie POST"}"#.to_vec();
    let response = serve_at(&world.factory, ambient, &root);
    assert_eq!(response.status, 401, "{}", response.body_text());
}

/// `/fop/<path>` sends the screen at `<path>` as `html-print` made into a PDF, as Java's `MoquiFopServlet`.
#[test]
fn fop_sends_the_print_document_of_the_screen_as_pdf() {
    struct MarkPdf;
    impl crate::screen::PdfEngine for MarkPdf {
        fn render(&self, html: &str) -> Result<Vec<u8>, crate::context::Error> {
            let printed = html.starts_with("<!DOCTYPE html>") && html.contains("Statement body");
            Ok(format!("%PDF {printed}").into_bytes())
        }
    }
    let _pdf = crate::screen::pdf_test_lock();
    let dir = std::env::temp_dir().join(format!("moqui-screen-fop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Root")).unwrap();
    std::fs::write(
        dir.join("Root.xml"),
        r#"<screen require-authentication="false"><widgets><subscreens-active/></widgets></screen>"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("Root/Doc.xml"),
        r#"<screen require-authentication="false"><widgets><label text="Statement body"/></widgets></screen>"#,
    )
    .unwrap();
    let root = dir.join("Root.xml").to_string_lossy().into_owned();
    let world = World::open("");
    crate::screen::install_pdf_engine(Arc::new(MarkPdf));

    let mut named = request("GET", "/fop/Doc");
    named.query.push(("filename".into(), "Statement 1.pdf".into()));
    let response = serve_at(&world.factory, named, &root);
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert_eq!(response.header("content-type"), Some("application/pdf"));
    assert_eq!(response.body, b"%PDF true");
    assert_eq!(
        response.header("content-disposition"),
        Some("attachment; filename=\"Statement_1.pdf\"; filename*=utf-8''Statement%201.pdf")
    );
    // with no file name the browser shows it
    let response = serve_at(&world.factory, request("GET", "/fop/Doc"), &root);
    assert_eq!(response.header("content-disposition"), Some("inline"));
    // the rest of the path is a screen path, and the plain path is still the html screen
    let response = serve_at(&world.factory, request("GET", "/fop/Missing"), &root);
    assert_eq!(response.status, 404, "{}", response.body_text());
    let response = serve_at(&world.factory, request("GET", "/Doc"), &root);
    assert!(response.body_text().contains("Statement body"));
    assert_ne!(response.header("content-type"), Some("application/pdf"));
    // no engine: the reason is the error
    crate::screen::clear_pdf_engine();
    let response = serve_at(&world.factory, request("GET", "/fop/Doc"), &root);
    assert_eq!(response.status, 500, "{}", response.body_text());
    assert!(
        response.body_text().contains("PDF is not available"),
        "{}",
        response.body_text()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

mod authc;
mod decimal;
mod files;
mod forms;
mod gate;
mod golden;
mod html;
mod markdown;
mod menu;
mod modules;
mod print;
mod qvt;
mod render;
mod saved_finds;
mod servlets;
mod shell;
mod statics;
mod tui;
mod walk;
mod widgets;

/// The fixture screen tree under `testdata`, as a root screen location.
fn fixture_root() -> String {
    format!("{}/src/screen/testdata/Root.xml", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn a_render_mode_parameter_of_tui_renders_standalone_json() {
    let world = World::open("");
    let root = webroot().to_string_lossy().into_owned();
    let mut asked = request("GET", "/Login");
    asked.query.push(("renderMode".to_string(), "tui".to_string()));
    let response = serve_at(&world.factory, asked, &root);
    let text = response.body_text();
    assert_eq!(response.status, 200, "{text}");
    // Without standalone the webroot shell renders, and its html is not JSON.
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"));
    assert!(!text.contains("did not render as JSON"), "{doc}");
}
