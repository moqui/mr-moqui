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

use moqui_lib::object::Value;

use super::{install, run};
use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;

#[test]
fn basic_enum_types_returns_fixture_rows_and_a_service_runs() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(dir.join("service/moqui.rest.xml"), REST_XML).unwrap();
    let service_path = dir.join("service/echo.xml");
    std::fs::write(&service_path, SERVICE_XML).unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    install(ctx.factory(), &[dir.join("service/moqui.rest.xml")]);
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    insert_type(&facade, "DayOfWeek", "Day of Week");
    insert_type(&facade, "GeoType", "Geo Type");

    let listed = run(&["moqui", "basic", "enumTypes"], "get", &BTreeMap::new()).unwrap();
    assert_eq!(row_count(&listed), 2, "{listed:?}");
    assert_eq!(field(&listed, "DayOfWeek"), Some("Day of Week"));
    assert_eq!(field(&listed, "GeoType"), Some("Geo Type"));

    let one = run(&["moqui", "basic", "enumTypes", "GeoType"], "GET", &BTreeMap::new()).unwrap();
    let Value::Map(one_map) = one else {
        panic!("one result {one:?}");
    };
    assert_eq!(one_map.get("enumTypeId").and_then(Value::as_str), Some("GeoType"));
    assert_eq!(one_map.get("description").and_then(Value::as_str), Some("Geo Type"));

    let mut params = BTreeMap::new();
    params.insert("note".to_string(), Value::string("hello"));
    let echoed = run(&["moqui", "basic", "echo"], "post", &params).unwrap();
    let Value::Map(echoed_map) = echoed else {
        panic!("service result {echoed:?}");
    };
    assert_eq!(echoed_map.get("echoed").and_then(Value::as_str), Some("hello"));

    // The method comes from the request, so a parameter named _method changes nothing.
    let mut sneaky = BTreeMap::new();
    sneaky.insert("_method".to_string(), Value::string("post"));
    let err = run(&["moqui", "basic", "echo"], "get", &sneaky).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::MethodNotAllowed,
        "{err}"
    );
    let err = run(&["moqui", "basic", "nothing"], "get", &BTreeMap::new()).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::NotFound,
        "{err}"
    );
    let err = run(&["nope"], "get", &BTreeMap::new()).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::NotFound,
        "{err}"
    );
}

/// A REST path's frames are not artifact hits, but the service it calls is: its span is a child of the request's,
/// with no span for the path frames between them.
#[cfg(feature = "telemetry")]
#[test]
fn a_rest_service_is_a_span_of_the_request() {
    use crate::entity::document::Document;
    use crate::telemetry::{Exporter, SpanKind};
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Memory(Mutex<Vec<Document>>);
    impl Exporter for Memory {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, moqui_lib::error::Error> {
            let count = documents.len();
            if index == "otel_spans" {
                self.0.lock().unwrap().extend(documents);
            }
            Ok(count)
        }
    }
    let dir = std::env::temp_dir().join(format!("moqui-rest-telemetry-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(dir.join("service/moqui.rest.xml"), REST_XML).unwrap();
    std::fs::write(dir.join("service/echo.xml"), SERVICE_XML).unwrap();
    let facade = crate::testing::open();
    let ctx =
        ExecutionContextFactory::test_context_with_conf("<moqui-conf><telemetry sample-ratio=\"1\"/></moqui-conf>");
    install(ctx.factory(), &[dir.join("service/moqui.rest.xml")]);
    ctx.factory().set_entity(facade.clone());
    let telemetry = crate::telemetry::for_factory(ctx.factory());
    let memory = Arc::new(Memory::default());
    telemetry.set_exporter(memory.clone());
    let root_id;
    {
        let _entered = ctx.enter();
        // Registers the service file, as the first test does.
        let failures = crate::service::lint(&[dir.join("service/echo.xml")]).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let root = telemetry.start_span("POST", SpanKind::Server, None);
        root_id = root.context().unwrap().span_id;
        {
            let _trace = crate::telemetry::attach(root.context());
            let mut params = BTreeMap::new();
            params.insert("note".to_string(), Value::string("traced"));
            run(&["moqui", "basic", "echo"], "post", &params).unwrap();
        }
        root.finish();
    }
    telemetry.flush();
    let spans = memory.0.lock().unwrap();
    let named = |name: &str| {
        spans
            .iter()
            .find(|span| span.get("name").and_then(Value::as_str) == Some(name))
            .map(|span| {
                span.get("parent_span_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
    };
    assert_eq!(named("echo.ping#Rest"), Some(root_id), "{spans:?}");
    for path in ["/moqui", "/moqui/basic", "/moqui/basic/echo"] {
        assert_eq!(named(path), None, "{path}: {spans:?}");
    }
    drop(spans);
    telemetry.shutdown();
}

/// As Java's `RestApi`: identity-admin entities are refused, a `UserAccount` write loses the password fields and,
/// for a caller who is not ADMIN, the identity fields, secret-config writes need ADMIN, and results never carry
/// the password fields.
#[test]
fn entity_resources_keep_sensitive_entities_and_fields_out() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-sensitive-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(
        dir.join("service/sec.rest.xml"),
        r#"<resource name="sec" require-authentication="false">
                <resource name="keys"><method type="get"><entity name="moqui.security.UserLoginKey" operation="list"/></method></resource>
                <resource name="users">
                    <method type="get"><entity name="moqui.security.UserAccount" operation="list"/></method>
                    <method type="post"><entity name="moqui.security.UserAccount" operation="create"/></method>
                </resource>
                <resource name="servers"><method type="post"><entity name="moqui.basic.email.EmailServer" operation="create"/></method></resource>
            </resource>"#,
    )
    .unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    install(ctx.factory(), &[dir.join("service/sec.rest.xml")]);
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    create(
        &facade,
        "moqui.security.UserAccount",
        &[("userId", "U1"), ("username", "u1"), ("currentPassword", "hash")],
    );
    let unauthorized =
        |err: &crate::context::Error| crate::context::ErrorKind::of(err) == crate::context::ErrorKind::Unauthorized;

    let err = run(&["sec", "keys"], "get", &BTreeMap::new()).unwrap_err();
    assert!(unauthorized(&err), "{err}");

    let listed = run(&["sec", "users"], "get", &BTreeMap::new()).unwrap();
    let Value::List(rows) = &listed else {
        panic!("list {listed:?}")
    };
    let Value::Map(row) = &rows[0] else {
        panic!("row {listed:?}")
    };
    assert_eq!(row.get("userId").and_then(Value::as_str), Some("U1"));
    assert!(!row.contains_key("currentPassword"), "{row:?}");

    let mut params = BTreeMap::new();
    params.insert("userId".to_string(), Value::string("U2"));
    params.insert("username".to_string(), Value::string("u2"));
    params.insert("currentPassword".to_string(), Value::string("chosen"));
    run(&["sec", "users"], "post", &params).unwrap();
    let stored = facade
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("userId", Value::string("U2"))
        .one()
        .unwrap()
        .expect("created");
    assert!(stored.get("currentPassword").is_none_or(Value::is_null));
    assert!(
        stored.get("username").is_none_or(Value::is_null),
        "identity fields need ADMIN"
    );

    let mut server = BTreeMap::new();
    server.insert("emailServerId".to_string(), Value::string("E1"));
    let err = run(&["sec", "servers"], "post", &server).unwrap_err();
    assert!(unauthorized(&err), "{err}");
}

#[test]
fn service_metadata_resolves_named_resources_ids_and_extra_paths_without_executing() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-metadata-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("api.rest.xml");
    std::fs::write(&path, r#"<resource name="api"><resource name="items">
            <resource name="named"><method type="get"><service name="literal#Named"/></method></resource>
            <id name="itemId"><resource name="detail"><method type="post"><service name="literal#Detail"/></method></resource></id>
            <method type="get"><entity name="moqui.basic.Enumeration" operation="list"/></method>
            </resource><resource name="browse"><id name="path" allow-extra-path="true">
            <method type="get"><service name="literal#Browse"/></method></id></resource></resource>"#).unwrap();
    let context = ExecutionContextFactory::test_context();
    install(context.factory(), &[path]);
    let _entered = context.enter();
    for (path, method, expected, parameters) in [
        ("api/items/named", "GET", "literal#Named", vec![]),
        (
            "/api/items/123/detail/",
            "post",
            "literal#Detail",
            vec![Value::string("itemId")],
        ),
        ("api/browse", "get", "literal#Browse", vec![Value::string("path")]),
        ("api/browse/a/b", "get", "literal#Browse", vec![Value::string("path")]),
    ] {
        let Value::Map(result) = super::resolve_service(path, method).unwrap() else {
            panic!("map");
        };
        assert_eq!(result["serviceName"].as_str(), Some(expected));
        assert_eq!(result["pathParameters"], Value::List(parameters));
        assert_eq!(result["method"], Value::string(method.to_ascii_lowercase()));
        assert_eq!(
            result["path"],
            Value::string(format!("/rest/s1/{}", path.trim_matches('/')))
        );
    }
    for path in ["", "missing", "api/items", "api/items/123", "api/items/named/missing"] {
        assert!(super::resolve_service(path, "get").is_err(), "{path}");
    }
    assert!(super::resolve_service("api/items/named", "post").is_err());
    assert!(crate::script::eval_condition(
        r#"ec.service.restApi.resolveService("api/items/123/detail", "post").pathParameters[0] === "itemId""#,
    )
    .unwrap());
}

#[test]
fn a_login_is_required_unless_the_resource_waives_it() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-auth-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(
        dir.join("service/secure.rest.xml"),
        r#"<resource name="secure"><resource name="items">
                <method type="get"><entity name="moqui.basic.EnumerationType" operation="list"/></method>
            </resource></resource>"#,
    )
    .unwrap();
    std::fs::write(dir.join("service/broken.rest.xml"), "<resource name=").unwrap();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(crate::testing::open());
    // A broken file beside it does not stop the good one from being served.
    let mut loaded = BTreeMap::new();
    super::load_dir(&mut loaded, &dir.join("service"));
    assert_eq!(loaded.keys().collect::<Vec<_>>(), ["secure"]);
    install(ctx.factory(), &[dir.join("service/secure.rest.xml")]);
    let _entered = ctx.enter();
    let err = run(&["secure", "items"], "get", &BTreeMap::new()).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::AuthenticationRequired,
        "{err}"
    );
}

/// The `AT_REST_PATH` frames: an ALWAYS grant on the root path flows to the service, a user without one is
/// refused at the path, and a service that needs no login is still behind the path's authorization.
#[test]
fn rest_path_authz_gates_and_inherits() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-path-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(
        dir.join("service/moqui.rest.xml"),
        r#"<resource name="moqui"><resource name="basic">
                <resource name="echo"><method type="post"><service name="gate.ping#Rest"/></method></resource>
                <resource name="secured"><method type="post"><service name="gate.secure#Rest"/></method></resource>
            </resource></resource>"#,
    )
    .unwrap();
    let service_path = dir.join("service/gate.xml");
    std::fs::write(
        &service_path,
        r#"<services>
                <service verb="ping" noun="Rest" authenticate="false">
                    <out-parameters><parameter name="echoed"/></out-parameters>
                    <actions><script>echoed = "anonymous";</script></actions>
                </service>
                <service verb="secure" noun="Rest">
                    <out-parameters><parameter name="echoed"/></out-parameters>
                    <actions><script>echoed = "secured";</script></actions>
                </service>
            </services>"#,
    )
    .unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><artifact-execution-facade>
                <artifact-execution type="AT_REST_PATH" authz-enabled="true"/>
                <artifact-execution type="AT_SERVICE" authz-enabled="true"/>
                <artifact-execution type="AT_ENTITY" authz-enabled="true"/>
            </artifact-execution-facade></moqui-conf>"#,
    );
    install(ctx.factory(), &[dir.join("service/moqui.rest.xml")]);
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    {
        let _authz = ctx.disable_authz();
        let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        for (user, group) in [("pathadmin", Some("ADMIN")), ("pathplain", None)] {
            create(
                &facade,
                "moqui.security.UserAccount",
                &[("userId", user), ("username", user)],
            );
            if let Some(group) = group {
                let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
                member.set("userId", Value::string(user));
                member.set("userGroupId", Value::string(group));
                member.set(
                    "fromDate",
                    moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value,
                );
                member.create(&facade).unwrap();
            }
        }
        create(
            &facade,
            "moqui.security.ArtifactGroup",
            &[("artifactGroupId", "MOQUI_API")],
        );
        create(
            &facade,
            "moqui.security.ArtifactGroupMember",
            &[
                ("artifactGroupId", "MOQUI_API"),
                ("artifactName", "/moqui"),
                ("artifactTypeEnumId", "AT_REST_PATH"),
                ("nameIsPattern", "N"),
                ("inheritAuthz", "Y"),
            ],
        );
        create(
            &facade,
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "MOQUI_API_ADMIN"),
                ("userGroupId", "ADMIN"),
                ("artifactGroupId", "MOQUI_API"),
                ("authzTypeEnumId", "AUTHZT_ALWAYS"),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
    }
    let echoed = |value: Value| match value {
        Value::Map(map) => map.get("echoed").and_then(Value::as_str).unwrap_or("").to_string(),
        other => panic!("service result {other:?}"),
    };
    let kind = |err: &crate::context::Error| crate::context::ErrorKind::of(err);
    let none = BTreeMap::new();

    // No one logged in: the path asks for a login, also for the service that needs none.
    let err = run(&["moqui", "basic", "echo"], "post", &none).unwrap_err();
    assert_eq!(kind(&err), crate::context::ErrorKind::AuthenticationRequired, "{err}");

    // ADMIN has no grant on the service itself; the ALWAYS grant on /moqui is inherited.
    crate::user::internal_login_user("pathadmin").unwrap();
    let result = run(&["moqui", "basic", "secured"], "post", &none).unwrap();
    assert_eq!(echoed(result), "secured");
    let result = run(&["moqui", "basic", "echo"], "post", &none).unwrap();
    assert_eq!(echoed(result), "anonymous");
    crate::user::logout().unwrap();

    // A user outside ADMIN is refused at the path, before the service.
    crate::user::internal_login_user("pathplain").unwrap();
    for leaf in ["secured", "echo"] {
        let err = run(&["moqui", "basic", leaf], "post", &none).unwrap_err();
        assert_eq!(kind(&err), crate::context::ErrorKind::Unauthorized, "{leaf}: {err}");
        assert!(err.message().contains("AT_REST_PATH"), "{leaf}: {err}");
    }
    crate::user::logout().unwrap();
}

/// As Java's `RestResult` and `nestedRemoveNullsFromResultMap`: a service's out-parameters lose the `UserAccount`
/// password fields at any depth, the null out-parameters, and the null fields one map down.
#[test]
fn service_results_lose_user_account_secrets_and_nulls() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-secrets-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(
        dir.join("service/acct.rest.xml"),
        r#"<resource name="acct" require-authentication="false">
                <resource name="accounts"><method type="get"><service name="acct.get#Accounts"/></method></resource>
            </resource>"#,
    )
    .unwrap();
    let service_path = dir.join("service/acct.xml");
    std::fs::write(
        &service_path,
        r#"<services>
                <service verb="get" noun="Accounts" authenticate="false">
                    <out-parameters>
                        <parameter name="account" type="Map"/>
                        <parameter name="accounts" type="List"/>
                        <parameter name="nested" type="Map"/>
                        <parameter name="nothing"/>
                    </out-parameters>
                    <actions><script>
                        account = {userId: "U1", currentPassword: "hash", passwordSalt: "salt", note: null};
                        accounts = [{userId: "U2", resetPassword: "reset", passwordHashType: "SHA", note: null}];
                        nested = {party: {users: [{userId: "U3", passwordBase64: "b64"}], kept: null}, gone: null};
                        nothing = null;
                    </script></actions>
                </service>
            </services>"#,
    )
    .unwrap();
    let ctx = ExecutionContextFactory::test_context();
    install(ctx.factory(), &[dir.join("service/acct.rest.xml")]);
    ctx.factory().set_entity(crate::testing::open());
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");

    let result = run(&["acct", "accounts"], "get", &BTreeMap::new()).unwrap();
    let Value::Map(result) = &result else {
        panic!("service result {result:?}")
    };
    assert!(!result.contains_key("nothing"), "{result:?}");
    let Some(Value::Map(account)) = result.get("account") else {
        panic!("account {result:?}")
    };
    assert_eq!(account.keys().collect::<Vec<_>>(), ["userId"], "{account:?}");
    let Some(Value::List(accounts)) = result.get("accounts") else {
        panic!("accounts {result:?}")
    };
    let Value::Map(listed) = &accounts[0] else {
        panic!("listed {accounts:?}")
    };
    assert_eq!(listed.keys().collect::<Vec<_>>(), ["userId"], "{listed:?}");
    let Some(Value::Map(nested)) = result.get("nested") else {
        panic!("nested {result:?}")
    };
    assert!(!nested.contains_key("gone"), "{nested:?}");
    let Some(Value::Map(party)) = nested.get("party") else {
        panic!("party {nested:?}")
    };
    // Only one map down, as Java: a null deeper in stays.
    assert_eq!(party.get("kept"), Some(&Value::Null), "{party:?}");
    let Some(Value::List(users)) = party.get("users") else {
        panic!("users {party:?}")
    };
    let Value::Map(user) = &users[0] else {
        panic!("user {users:?}")
    };
    assert_eq!(user.keys().collect::<Vec<_>>(), ["userId"], "{user:?}");
}

/// As Java's `PathNode`: `description` is an attribute, `childPaths` counts the resources and ids under a node, an
/// empty `require-authentication` inherits, a method is keyed by `type` as written, and a method that does not load
/// is left out of a file that is otherwise served.
#[test]
fn rest_files_read_as_java_reads_them() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-read-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(
        dir.join("service/read.rest.xml"),
        r#"<resource name="read" displayName="Read API" description="Reads things" require-authentication="false">
                <resource name="types" require-authentication="">
                    <method type="get"><entity name="moqui.basic.EnumerationType" operation="list"/></method>
                    <method type="post"><other name="nothing"/></method>
                    <method><entity name="moqui.basic.EnumerationType" operation="list"/></method>
                    <id name="enumTypeId"><resource name="detail"/></id>
                </resource>
                <resource name="shout"><method type="GET"><entity name="moqui.basic.EnumerationType" operation="list"/></method></resource>
            </resource>"#,
    )
    .unwrap();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(crate::testing::open());
    let mut loaded = BTreeMap::new();
    super::load_dir(&mut loaded, &dir.join("service"));
    let Value::Map(info) = super::resource_info(&loaded["read"], true) else {
        panic!("info")
    };
    assert_eq!(info["description"], Value::string("Reads things"));
    assert_eq!(info["displayName"], Value::string("Read API"));
    assert_eq!(info["childPaths"], Value::I64(4), "types, its id and detail, and shout");
    assert_eq!(info["childMethods"], Value::I64(2), "{info:?}");
    install(ctx.factory(), &[dir.join("service/read.rest.xml")]);
    let _entered = ctx.enter();
    let kind = |err: &crate::context::Error| crate::context::ErrorKind::of(err);
    // No login: the empty value took the root's `false`.
    assert!(run(&["read", "types"], "get", &BTreeMap::new()).is_ok());
    let err = run(&["read", "types"], "post", &BTreeMap::new()).unwrap_err();
    assert_eq!(kind(&err), crate::context::ErrorKind::MethodNotAllowed, "{err}");
    // The request's method is lower case, so `type="GET"` is never matched.
    let err = run(&["read", "shout"], "GET", &BTreeMap::new()).unwrap_err();
    assert_eq!(kind(&err), crate::context::ErrorKind::MethodNotAllowed, "{err}");
}

/// The OpenAPI of a service method: its path parameter, its other in-parameters as the query of a `get` or the
/// JSON body of a `post`, its out-parameters as the response, and the `enum` of a parameter on an `Enumeration`
/// field, as Java's Swagger.
#[test]
fn openapi_lists_service_parameters_with_their_descriptions_and_enumeration_ids() {
    let dir = std::env::temp_dir().join(format!("moqui-rest-openapi-{}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    std::fs::create_dir_all(dir.join("service")).unwrap();
    std::fs::write(
        dir.join("service/feeds.rest.xml"),
        r#"<resource name="feeds"><id name="dataFeedId">
                <method type="get"><service name="feeds.get#Feed"/></method>
                <method type="post"><service name="feeds.get#Feed"/></method>
            </id></resource>"#,
    )
    .unwrap();
    let service_path = dir.join("service/feeds.xml");
    std::fs::write(
        &service_path,
        r#"<services>
                <service verb="get" noun="Feed" authenticate="false">
                    <in-parameters>
                        <parameter name="dataFeedId" required="true"><description> The feed </description></parameter>
                        <parameter name="dataFeedTypeEnumId" entity-name="moqui.entity.feed.DataFeed" field-name="dataFeedTypeEnumId"/>
                        <parameter name="count" type="Integer" default-value="10"><description>How many</description></parameter>
                        <parameter name="options" type="Map"/>
                    </in-parameters>
                    <out-parameters><parameter name="feed" type="Map"/></out-parameters>
                    <actions><script>feed = {};</script></actions>
                </service>
            </services>"#,
    )
    .unwrap();
    let facade = crate::testing::open();
    for (id, sequence) in [("DTFDTP_MAN_PULL", 2), ("DTFDTP_RT_PUSH", 1)] {
        let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
        row.set("enumId", Value::string(id));
        row.set("enumTypeId", Value::string("DataFeedType"));
        row.set("sequenceNum", Value::I64(sequence));
        row.create(&facade).unwrap();
    }
    let ctx = ExecutionContextFactory::test_context();
    install(ctx.factory(), &[dir.join("service/feeds.rest.xml")]);
    ctx.factory().set_entity(facade);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&service_path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");

    let doc = super::openapi_document(&["feeds".to_string()]).unwrap();
    let at = |value: &Value, path: &[&str]| -> Value {
        path.iter().fold(value.clone(), |current, key| match current {
            Value::Map(map) => map.get(*key).cloned().unwrap_or_else(|| panic!("no {key} in {map:?}")),
            other => panic!("not a map at {key}: {other:?}"),
        })
    };
    let ids = Value::List(vec![Value::string("DTFDTP_RT_PUSH"), Value::string("DTFDTP_MAN_PULL")]);
    let get = at(&doc, &["paths", "/feeds/{dataFeedId}", "get"]);
    let Value::List(parameters) = at(&get, &["parameters"]) else {
        panic!("parameters {get:?}")
    };
    let named = |name: &str| {
        parameters
            .iter()
            .find(|item| matches!(item, Value::Map(map) if map["name"] == Value::string(name)))
            .cloned()
    };
    let path = named("dataFeedId").expect("path parameter");
    assert_eq!(at(&path, &["in"]), Value::string("path"));
    assert_eq!(at(&path, &["required"]), Value::Bool(true));
    assert_eq!(at(&path, &["schema", "description"]), Value::string("The feed"));
    let feed_type = named("dataFeedTypeEnumId").expect("query parameter");
    assert_eq!(at(&feed_type, &["in"]), Value::string("query"));
    assert_eq!(at(&feed_type, &["schema", "enum"]), ids);
    let count = named("count").expect("count");
    assert_eq!(at(&count, &["schema", "type"]), Value::string("integer"));
    assert_eq!(at(&count, &["schema", "format"]), Value::string("int32"));
    assert_eq!(at(&count, &["schema", "default"]), Value::string("10"));
    assert_eq!(at(&count, &["schema", "description"]), Value::string("How many"));
    assert!(
        !matches!(at(&feed_type, &["schema"]), Value::Map(map) if map.contains_key("description")),
        "no description, none in the schema"
    );
    assert!(named("options").is_none(), "an object is no query parameter");
    assert_eq!(
        at(
            &get,
            &[
                "responses",
                "200",
                "content",
                "application/json",
                "schema",
                "properties",
                "feed",
                "type"
            ]
        ),
        Value::string("object")
    );

    let post = at(&doc, &["paths", "/feeds/{dataFeedId}", "post"]);
    let body = at(&post, &["requestBody", "content", "application/json", "schema"]);
    assert_eq!(at(&body, &["properties", "dataFeedTypeEnumId", "enum"]), ids);
    assert_eq!(
        at(&body, &["properties", "count", "description"]),
        Value::string("How many")
    );
    assert_eq!(at(&body, &["required"]), Value::List(vec![Value::string("dataFeedId")]));
}

fn create(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
    let mut row = facade.make_value(entity).unwrap();
    for (name, value) in pairs {
        row.set(*name, Value::string(*value));
    }
    row.create(facade).unwrap();
}

fn insert_type(facade: &EntityFacade, id: &str, description: &str) {
    let mut row = facade.make_value("moqui.basic.EnumerationType").unwrap();
    row.set("enumTypeId", Value::string(id));
    row.set("description", Value::string(description));
    row.create(facade).unwrap();
}

fn row_count(value: &Value) -> usize {
    let Value::List(items) = value else {
        panic!("list result {value:?}");
    };
    items.len()
}

fn field<'a>(value: &'a Value, id: &str) -> Option<&'a str> {
    let Value::List(items) = value else {
        return None;
    };
    items.iter().find_map(|item| {
        let Value::Map(row) = item else {
            return None;
        };
        let found = row.get("enumTypeId").and_then(Value::as_str)?;
        if found == id {
            row.get("description").and_then(Value::as_str)
        } else {
            None
        }
    })
}

struct DirGuard(PathBuf);
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const REST_XML: &str = r#"<resource name="moqui" require-authentication="false">
        <resource name="basic">
            <resource name="enumTypes">
                <method type="get"><entity name="moqui.basic.EnumerationType" operation="list"/></method>
                <id name="enumTypeId">
                    <method type="get"><entity name="moqui.basic.EnumerationType" operation="one"/></method>
                </id>
            </resource>
            <resource name="echo">
                <method type="post"><service name="echo.ping#Rest"/></method>
            </resource>
        </resource>
    </resource>"#;

const SERVICE_XML: &str = r#"<services>
        <service verb="ping" noun="Rest" authenticate="false">
            <in-parameters><parameter name="note"/></in-parameters>
            <out-parameters><parameter name="echoed"/></out-parameters>
            <actions><script>echoed = note;</script></actions>
        </service>
    </services>"#;
