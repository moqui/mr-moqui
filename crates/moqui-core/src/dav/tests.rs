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

//! The DAV server on the real services: app passwords (made, taken, revoked, never the web password unless the conf says
//! so, never another user's), discovery from the root to the principal, and WebDAV over a user's drives (folders,
//! uploads and their versions, conditional writes, ranges, move, copy, trash, the desktops' litter, locks, and another
//! user's tree not found). CalDAV and CardDAV are tested with Mantle's adapters (mr-mantle-usl `test/Dav.spec.js`).

use std::collections::BTreeMap;

use base64::Engine;
use moqui_lib::object::{to_plain_string, Value};

use super::{serve, DavRequest, DavResponse};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

fn setup() -> (ExecutionContext, EntityFacade) {
    let ctx = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    ctx.factory().resource().install_git().unwrap();
    (ctx, facade)
}

fn user(facade: &EntityFacade, user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut row = facade.make_value("moqui.security.UserAccount").unwrap();
    row.set("userId", Value::string(user_id));
    row.set("username", Value::string(user_id));
    row.set("userFullName", Value::string(format!("{user_id} Example")));
    row.set("emailAddress", Value::string(format!("{user_id}@example.com")));
    row.set(
        "currentPassword",
        Value::string(crate::user::hash_password("web-Password-1").unwrap()),
    );
    row.set("passwordHashType", Value::string("argon2id"));
    row.create(facade).unwrap();
}

fn login(user_id: &str) {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    crate::user::internal_login_user(user_id).unwrap();
}

fn service(name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let out = crate::service::sync(name, &parameters).unwrap();
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    assert!(errors.is_empty(), "{name}: {errors}");
    out
}

fn app_password(user_id: &str, name: &str) -> (String, String) {
    login(user_id);
    let out = service(
        "org.moqui.impl.UserServices.create#UserAppPassword",
        &[("name", Value::string(name))],
    );
    crate::user::logout().unwrap();
    (
        out.get("appPasswordId").map(to_plain_string).unwrap(),
        out.get("password").map(to_plain_string).unwrap(),
    )
}

struct Client<'a> {
    factory: &'a ExecutionContextFactory,
    authorization: Option<String>,
}

impl Client<'_> {
    fn send(&self, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> DavResponse {
        let mut request = DavRequest {
            method: method.to_string(),
            path: path.to_string(),
            headers: headers
                .iter()
                .map(|(key, value)| (key.to_ascii_lowercase(), (*value).to_string()))
                .collect(),
            body: body.as_bytes().to_vec(),
            secure: true,
            client_ip: "127.0.0.1".to_string(),
        };
        if let Some(authorization) = &self.authorization {
            request.headers.push(("authorization".into(), authorization.clone()));
        }
        let response = serve(self.factory, &request);
        let _ = crate::user::logout();
        response
    }
}

fn basic(username: &str, password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
    )
}

fn body(response: &DavResponse) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

fn header<'a>(response: &'a DavResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

const PROPFIND_ALL: &str = r#"<?xml version="1.0"?><propfind xmlns="DAV:"><allprop/></propfind>"#;

#[test]
fn an_app_password_logs_a_device_in_and_the_web_password_does_not() {
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "alice");
    user(&facade, "bob");
    let (id, password) = app_password("alice", "Phone");
    assert_eq!(password.len(), 24, "{password}");
    assert!(crate::user::looks_like_app_password(&password));
    let factory = context.factory();
    let anonymous = Client {
        factory,
        authorization: None,
    };
    let answer = anonymous.send("PROPFIND", "/dav/", &[("Depth", "0")], "");
    assert_eq!(answer.status, 401);
    assert!(header(&answer, "WWW-Authenticate").is_some_and(|value| value.starts_with("Basic")));
    // OPTIONS answers without a login, as clients probe first
    assert_eq!(anonymous.send("OPTIONS", "/dav/", &[], "").status, 200);
    let web = Client {
        factory,
        authorization: Some(basic("alice", "web-Password-1")),
    };
    assert_eq!(
        web.send("PROPFIND", "/dav/", &[("Depth", "0")], "").status,
        401,
        "the web password is not taken"
    );
    let other = Client {
        factory,
        authorization: Some(basic("bob", &password)),
    };
    assert_eq!(
        other.send("PROPFIND", "/dav/", &[("Depth", "0")], "").status,
        401,
        "not another user's"
    );
    // the dashes and case do not matter
    let typed = password.replace('-', "").to_uppercase();
    let alice = Client {
        factory,
        authorization: Some(basic("alice", &typed)),
    };
    let root = alice.send("PROPFIND", "/dav/", &[("Depth", "0")], "");
    assert_eq!(root.status, 207, "{}", body(&root));
    assert!(body(&root).contains("/dav/principals/alice/"), "{}", body(&root));
    // over plain HTTP the credentials are refused
    let mut request = DavRequest {
        method: "PROPFIND".into(),
        path: "/dav/".into(),
        ..DavRequest::default()
    };
    request
        .headers
        .push(("authorization".into(), basic("alice", &password)));
    assert_eq!(serve(factory, &request).status, 403);
    let _ = crate::user::logout();

    login("alice");
    let listed = service("org.moqui.impl.UserServices.get#UserAppPasswords", &[]);
    let Some(Value::List(list)) = listed.get("appPasswordList") else {
        panic!("{listed:?}")
    };
    assert_eq!(list.len(), 1);
    let Value::Map(first) = &list[0] else { panic!() };
    assert!(
        first
            .get("lastUsedDate")
            .is_some_and(|value| !matches!(value, Value::Null)),
        "the use is written"
    );
    assert!(!first.contains_key("passwordHash"));
    service(
        "org.moqui.impl.UserServices.revoke#UserAppPassword",
        &[("appPasswordId", Value::string(&id))],
    );
    crate::user::logout().unwrap();
    assert_eq!(
        alice.send("PROPFIND", "/dav/", &[("Depth", "0")], "").status,
        401,
        "a revoked password is refused"
    );
}

#[test]
fn webdav_serves_the_users_drives_through_the_file_services() {
    let (context, facade) = setup();
    let _entered = context.enter();
    user(&facade, "alice");
    user(&facade, "bob");
    let (_, password) = app_password("alice", "Laptop");
    let (_, bob_password) = app_password("bob", "Laptop");
    let factory = context.factory();
    let alice = Client {
        factory,
        authorization: Some(basic("alice", &password)),
    };
    let bob = Client {
        factory,
        authorization: Some(basic("bob", &bob_password)),
    };

    let principal = alice.send("PROPFIND", "/dav/principals/alice/", &[("Depth", "0")], PROPFIND_ALL);
    assert_eq!(principal.status, 207);
    assert!(body(&principal).contains("alice Example"), "{}", body(&principal));

    let home = alice.send("PROPFIND", "/dav/files/alice/", &[("Depth", "1")], "");
    assert_eq!(home.status, 207, "{}", body(&home));
    assert!(body(&home).contains("/dav/files/alice/My%20files/"), "{}", body(&home));
    assert_eq!(
        alice.send("PROPFIND", "/dav/files/bob/", &[("Depth", "1")], "").status,
        404,
        "another user's tree"
    );

    let base = "/dav/files/alice/My%20files";
    assert_eq!(alice.send("MKCOL", &format!("{base}/Docs"), &[], "").status, 201);
    assert_eq!(alice.send("MKCOL", &format!("{base}/Docs"), &[], "").status, 405);
    assert_eq!(alice.send("MKCOL", &format!("{base}/No/Deeper"), &[], "").status, 409);
    let put = alice.send(
        "PUT",
        &format!("{base}/Docs/a.txt"),
        &[("Content-Type", "text/plain")],
        "hello there",
    );
    assert_eq!(put.status, 201, "{}", body(&put));
    let first_tag = header(&put, "ETag").unwrap().to_string();
    let got = alice.send("GET", &format!("{base}/Docs/a.txt"), &[], "");
    assert_eq!((got.status, body(&got).as_str()), (200, "hello there"));
    assert_eq!(header(&got, "ETag"), Some(first_tag.as_str()));
    assert_eq!(
        alice
            .send(
                "GET",
                &format!("{base}/Docs/a.txt"),
                &[("If-None-Match", &first_tag)],
                ""
            )
            .status,
        304
    );
    let part = alice.send("GET", &format!("{base}/Docs/a.txt"), &[("Range", "bytes=0-4")], "");
    assert_eq!((part.status, body(&part).as_str()), (206, "hello"));
    assert_eq!(
        alice
            .send("PUT", &format!("{base}/Docs/a.txt"), &[("If-None-Match", "*")], "again")
            .status,
        412
    );
    assert_eq!(
        alice
            .send(
                "PUT",
                &format!("{base}/Docs/a.txt"),
                &[("If-Match", "\"stale\"")],
                "again"
            )
            .status,
        412
    );
    let replaced = alice.send(
        "PUT",
        &format!("{base}/Docs/a.txt"),
        &[("If-Match", &first_tag)],
        "hello again",
    );
    assert_eq!(replaced.status, 204, "{}", body(&replaced));
    assert_ne!(
        header(&replaced, "ETag"),
        Some(first_tag.as_str()),
        "a new version, a new tag"
    );
    login("alice");
    let drive_id = to_plain_string(
        service("org.moqui.impl.FileServices.get#UserDrives", &[])
            .get("personalDriveId")
            .unwrap(),
    );
    let versions = service(
        "org.moqui.impl.FileServices.get#Versions",
        &[
            ("driveId", Value::string(&drive_id)),
            ("path", Value::string("Docs/a.txt")),
        ],
    );
    let Some(Value::List(versions)) = versions.get("versionList") else {
        panic!("{versions:?}")
    };
    assert!(versions.len() >= 2, "the replaced text is a version: {versions:?}");
    crate::user::logout().unwrap();

    // desktop litter is taken and not kept
    assert_eq!(
        alice.send("PUT", &format!("{base}/Docs/.DS_Store"), &[], "x").status,
        201
    );
    assert_eq!(alice.send("PUT", &format!("{base}/Docs/._a.txt"), &[], "x").status, 201);
    let listing = alice.send("PROPFIND", &format!("{base}/Docs/"), &[("Depth", "1")], "");
    assert_eq!(listing.status, 207);
    let text = body(&listing);
    assert!(
        text.contains("Docs/a.txt") && text.contains("<D:getcontentlength>11</D:getcontentlength>"),
        "{text}"
    );
    assert!(!text.contains("DS_Store") && !text.contains("._a.txt"), "{text}");

    // move, copy, trash
    let destination = format!("https://example.com{base}/Docs/b.txt");
    assert_eq!(
        alice
            .send(
                "MOVE",
                &format!("{base}/Docs/a.txt"),
                &[("Destination", &destination)],
                ""
            )
            .status,
        201
    );
    assert_eq!(alice.send("GET", &format!("{base}/Docs/a.txt"), &[], "").status, 404);
    assert_eq!(
        body(&alice.send("GET", &format!("{base}/Docs/b.txt"), &[], "")),
        "hello again"
    );
    assert_eq!(alice.send("MKCOL", &format!("{base}/Archive"), &[], "").status, 201);
    let copied = alice.send(
        "COPY",
        &format!("{base}/Docs/b.txt"),
        &[("Destination", &format!("{base}/Archive/c.txt"))],
        "",
    );
    assert_eq!(copied.status, 201, "{}", body(&copied));
    assert_eq!(
        body(&alice.send("GET", &format!("{base}/Archive/c.txt"), &[], "")),
        "hello again"
    );
    assert_eq!(
        alice
            .send(
                "COPY",
                &format!("{base}/Docs/b.txt"),
                &[("Destination", &format!("{base}/Archive/c.txt")), ("Overwrite", "F")],
                ""
            )
            .status,
        412
    );
    assert_eq!(alice.send("DELETE", &format!("{base}/Docs/b.txt"), &[], "").status, 204);
    assert_eq!(alice.send("GET", &format!("{base}/Docs/b.txt"), &[], "").status, 404);
    login("alice");
    let trash = service(
        "org.moqui.impl.FileServices.list#Trash",
        &[("driveId", Value::string(&drive_id))],
    );
    assert!(
        format!("{trash:?}").contains("b.txt"),
        "a delete goes to the trash: {trash:?}"
    );
    crate::user::logout().unwrap();

    // a lock keeps another user out and not its owner
    let locked = alice.send(
        "LOCK",
        &format!("{base}/Archive/c.txt"),
        &[("Timeout", "Second-60")],
        r#"<?xml version="1.0"?><lockinfo xmlns="DAV:"><lockscope><exclusive/></lockscope><locktype><write/></locktype><owner>alice</owner></lockinfo>"#,
    );
    assert_eq!(locked.status, 200, "{}", body(&locked));
    let token = header(&locked, "Lock-Token").unwrap().to_string();
    assert!(body(&locked).contains("Second-60"));
    assert_eq!(
        alice.send("PUT", &format!("{base}/Archive/c.txt"), &[], "mine").status,
        204
    );
    assert_eq!(
        alice
            .send(
                "UNLOCK",
                &format!("{base}/Archive/c.txt"),
                &[("Lock-Token", &token)],
                ""
            )
            .status,
        204
    );
    // bob reaches only his own drive
    assert_eq!(bob.send("GET", &format!("{base}/Archive/c.txt"), &[], "").status, 404);
    let bob_home = bob.send("PROPFIND", "/dav/files/bob/", &[("Depth", "1")], "");
    assert!(!body(&bob_home).contains("alice"), "{}", body(&bob_home));
    // PROPPATCH is answered, protected properties refused
    let patched = alice.send(
        "PROPPATCH",
        &format!("{base}/Archive/c.txt"),
        &[],
        r#"<?xml version="1.0"?><propertyupdate xmlns="DAV:" xmlns:z="urn:x"><set><prop><z:Win32LastModifiedTime>x</z:Win32LastModifiedTime></prop></set></propertyupdate>"#,
    );
    assert_eq!(patched.status, 207);
    assert!(body(&patched).contains("200 OK"), "{}", body(&patched));
}
