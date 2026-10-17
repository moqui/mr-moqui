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
        let env = Env::new();
        let context = ExecutionContextFactory::test_context_with_conf(conf);
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
        let mut member = self.facade.make_value("moqui.security.ArtifactGroupMember").unwrap();
        member.set("artifactGroupId", Value::string("SCREEN_VIEW"));
        member.set("artifactName", Value::string(location));
        member.set("artifactTypeEnumId", Value::string("AT_XML_SCREEN"));
        member.set("nameIsPattern", Value::string("N"));
        member.set("inheritAuthz", Value::string("Y"));
        member.create(&self.facade).unwrap();
        let mut authz = self.facade.make_value("moqui.security.ArtifactAuthz").unwrap();
        authz.set("artifactAuthzId", Value::string("SCREEN_VIEW"));
        authz.set("userGroupId", Value::string(group));
        authz.set("artifactGroupId", Value::string("SCREEN_VIEW"));
        authz.set("authzTypeEnumId", Value::string("AUTHZT_ALWAYS"));
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

#[test]
fn entity_json_has_the_primary_key_and_the_service_document_lists_enum_types() {
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
    assert_eq!(response.status, 200, "{}", response.body_text());
    assert!(response.body_text().contains("enumTypeId"), "{}", response.body_text());

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

mod authc;
mod files;
mod forms;
mod golden;
mod html;
mod markdown;
mod menu;
mod modules;
mod print;
mod qvt;
mod render;
mod saved_finds;
mod statics;
mod walk;
mod widgets;

/// The fixture screen tree under `testdata`, as a root screen location.
fn fixture_root() -> String {
    format!("{}/src/screen/testdata/Root.xml", env!("CARGO_MANIFEST_DIR"))
}
