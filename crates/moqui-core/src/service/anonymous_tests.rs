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

//! Anonymous artifact frames of services (`authenticate="anonymous-*"`) and REST paths (`require-authentication=
//! "anonymous-*"`): the inheritable grant on the frame, the anonymous login it starts when nobody is logged in, and its
//! end when the frame goes, also after an error, under a nested anonymous frame and with a user logged in.

use std::collections::BTreeMap;
use std::path::PathBuf;

use moqui_lib::object::Value;

use super::pool::ServicePools;
use crate::context::{ErrorKind, ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;

const SERVICES: &str = r#"<services>
    <service verb="probe" noun="Anon">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", id); made.set("testMedium", ec.user.username ?? "anonymous");</script>
            <entity-create value-field="made"/>
        </actions>
    </service>
    <service verb="find" noun="Anon">
        <out-parameters><parameter name="count"/></out-parameters>
        <actions><entity-find-count entity-name="moqui.test.TestEntity" count-field="count"/></actions>
    </service>
    <service verb="anon" noun="All" authenticate="anonymous-all">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <actions><service-call name="probe#Anon"><field-map field-name="id" from="id"/></service-call></actions>
    </service>
    <service verb="anon" noun="View" authenticate="anonymous-view">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <out-parameters><parameter name="count"/></out-parameters>
        <actions>
            <service-call name="find#Anon" out-map="found"/>
            <set field="count" from="found.count"/>
            <service-call name="probe#Anon"><field-map field-name="id" from="id"/></service-call>
        </actions>
    </service>
    <service verb="anon" noun="Nested" authenticate="anonymous-all">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <actions>
            <service-call name="anon#All"><field-map field-name="id" value="${id}-inner"/></service-call>
            <service-call name="probe#Anon"><field-map field-name="id" from="id"/></service-call>
        </actions>
    </service>
    <service verb="anon" noun="Fail" authenticate="anonymous-all">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <actions>
            <service-call name="probe#Anon"><field-map field-name="id" from="id"/></service-call>
            <script>throw new Error("anonymous body failed")</script>
        </actions>
    </service>
    <service verb="anon" noun="Register" authenticate="anonymous-all">
        <in-parameters><parameter name="id" required="true"/></in-parameters>
        <actions><script>ec.service.special().name("probe#Anon").parameters({id: id}).registerOnCommit()</script></actions>
    </service>
</services>"#;

const REST: &str = r#"<resource name="anon" require-authentication="anonymous-all">
    <resource name="probe"><method type="post"><service name="probe#Anon"/></method></resource>
    <resource name="view" require-authentication="anonymous-view">
        <method type="post"><service name="probe#Anon"/></method>
    </resource>
</resource>"#;

const REST_VIEW: &str = r#"<resource name="peek" require-authentication="anonymous-view">
    <method type="get"><service name="find#Anon"/></method>
    <method type="post"><service name="probe#Anon"/></method>
</resource>"#;

/// Service, entity and REST path authorization on, an inline executor, and the user `anonplain` in no group: nobody is
/// allowed any service, entity or path, so whatever runs runs on an anonymous frame's grant.
fn anonymous_context() -> (EntityFacade, ExecutionContext, Dir) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = Dir(std::env::temp_dir().join(format!("moqui-svc-anonymous-{}-{next}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let services = dir.0.join("anon.xml");
    std::fs::write(&services, SERVICES).unwrap();
    let rest = dir.0.join("anon.rest.xml");
    std::fs::write(&rest, REST).unwrap();
    let rest_view = dir.0.join("peek.rest.xml");
    std::fs::write(&rest_view, REST_VIEW).unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context_with(
        r#"<moqui-conf><artifact-execution-facade>
            <artifact-execution type="AT_REST_PATH" authz-enabled="true"/>
            <artifact-execution type="AT_SERVICE" authz-enabled="true"/>
            <artifact-execution type="AT_ENTITY" authz-enabled="true"/>
        </artifact-execution-facade></moqui-conf>"#,
        ServicePools::inline(),
    );
    ctx.factory().set_entity(facade.clone());
    super::install(ctx.factory(), &[rest, rest_view]);
    {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let failures = crate::service::lint(&[services]).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let mut account = facade.make_value("moqui.security.UserAccount").unwrap();
        account.set("userId", Value::string("anonplain"));
        account.set("username", Value::string("anonplain"));
        account.create(&facade).unwrap();
    }
    (facade, ctx, dir)
}

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn id(id: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([("id".to_string(), Value::string(id))])
}

fn medium(ctx: &ExecutionContext, facade: &EntityFacade, id: &str) -> Option<String> {
    let _authz = ctx.disable_authz();
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(id))
        .one()
        .unwrap()
        .and_then(|row| row.get("testMedium").and_then(Value::as_str).map(str::to_string))
}

/// Nothing of an anonymous frame is left: no frame on the stack, the session not anonymous, and the user as before.
fn assert_clean(ctx: &ExecutionContext, user: Option<&str>) {
    assert_eq!(ctx.artifact_stack_names(), "");
    assert!(
        !crate::user::is_anonymous().unwrap(),
        "the anonymous login outlived its frame"
    );
    assert_eq!(crate::user::current_username().unwrap().as_deref(), user);
    // A call that needs a login is refused again: no anonymous login and no grant are left.
    let err = super::sync("probe#Anon", &id("AFTER")).unwrap_err();
    let expected = if user.is_some() {
        ErrorKind::Unauthorized
    } else {
        ErrorKind::AuthenticationRequired
    };
    assert_eq!(ErrorKind::of(&err), expected, "{err}");
    ctx.message().clear();
}

#[test]
fn an_anonymous_all_service_logs_in_grants_all_and_ends_both() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    // Without the frame nobody may call probe#Anon.
    assert_clean(&ctx, None);
    super::sync("anon#All", &id("ALL")).unwrap();
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    // probe#Anon needs a login: the anonymous one counts; the grant reached it and its entity write.
    assert_eq!(medium(&ctx, &facade, "ALL").as_deref(), Some("anonymous"));
    assert_clean(&ctx, None);
}

#[test]
fn an_anonymous_view_service_grants_view_only() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    let err = super::sync("anon#View", &id("VIEW")).unwrap_err();
    assert_eq!(ErrorKind::of(&err), ErrorKind::Unauthorized, "{err}");
    assert!(err.message().contains("probe#Anon"), "{err}");
    ctx.message().clear();
    assert_eq!(medium(&ctx, &facade, "VIEW"), None);
    assert_clean(&ctx, None);
}

#[test]
fn a_nested_anonymous_service_leaves_the_outer_login_in_place() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    super::sync("anon#Nested", &id("NEST")).unwrap();
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    // The inner anon#All found the session anonymous already, so its end did not end the outer login: the outer body's
    // own probe#Anon, after the inner call, still ran.
    assert_eq!(medium(&ctx, &facade, "NEST-inner").as_deref(), Some("anonymous"));
    assert_eq!(medium(&ctx, &facade, "NEST").as_deref(), Some("anonymous"));
    assert_clean(&ctx, None);
}

#[test]
fn an_anonymous_service_keeps_the_logged_in_user() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("anonplain").unwrap();
    assert_clean(&ctx, Some("anonplain"));
    super::sync("anon#All", &id("USER")).unwrap();
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    // No anonymous login: the user runs the call, on the frame's grant.
    assert_eq!(medium(&ctx, &facade, "USER").as_deref(), Some("anonplain"));
    assert_clean(&ctx, Some("anonplain"));
    crate::user::logout().unwrap();
}

#[test]
fn an_anonymous_service_that_fails_ends_its_login() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    // A failed body is a recorded failure of the call: the message errors hold it and the write rolls back.
    let _ = super::sync("anon#Fail", &id("FAIL"));
    assert!(
        ctx.message()
            .errors()
            .iter()
            .any(|error| error.contains("anonymous body failed")),
        "{:?}",
        ctx.message().errors()
    );
    ctx.message().clear();
    assert_eq!(medium(&ctx, &facade, "FAIL"), None);
    assert_clean(&ctx, None);
    // Input that fails before the frame is entered leaves nothing either.
    let err = super::sync("anon#All", &BTreeMap::new());
    assert!(err.is_err() || !ctx.message().errors().is_empty());
    ctx.message().clear();
    assert_clean(&ctx, None);
}

/// A call registered on commit under an anonymous-all frame is checked against that frame's grant when registered, and
/// runs after the frame is gone.
#[test]
fn a_call_registered_under_an_anonymous_frame_runs_after_it() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    super::sync("anon#Register", &id("REG")).unwrap();
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    ctx.factory().run_pending();
    assert_eq!(medium(&ctx, &facade, "REG").as_deref(), Some("anonymous"));
    assert_clean(&ctx, None);
}

#[test]
fn an_anonymous_rest_path_logs_in_grants_and_ends_both() {
    let (facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    super::run(&["anon", "probe"], "post", &id("REST")).unwrap();
    assert_eq!(medium(&ctx, &facade, "REST").as_deref(), Some("anonymous"));
    assert_clean(&ctx, None);
    // anonymous-view below anonymous-all inherits all actions, as Java's `setAnonymousAuthorizedView` keeps an ALL.
    super::run(&["anon", "view"], "post", &id("RESTNESTED")).unwrap();
    assert_eq!(medium(&ctx, &facade, "RESTNESTED").as_deref(), Some("anonymous"));
    assert_clean(&ctx, None);
    // anonymous-view on its own: the view method runs, the create method is refused.
    super::run(&["peek"], "get", &BTreeMap::new()).unwrap();
    let err = super::run(&["peek"], "post", &id("RESTVIEW")).unwrap_err();
    assert_eq!(ErrorKind::of(&err), ErrorKind::Unauthorized, "{err}");
    ctx.message().clear();
    assert_eq!(medium(&ctx, &facade, "RESTVIEW"), None);
    assert_clean(&ctx, None);
    // A user logged in stays logged in, and runs the call on the path's grant.
    crate::user::internal_login_user("anonplain").unwrap();
    super::run(&["anon", "probe"], "post", &id("RESTUSER")).unwrap();
    assert_eq!(medium(&ctx, &facade, "RESTUSER").as_deref(), Some("anonplain"));
    assert_clean(&ctx, Some("anonplain"));
    crate::user::logout().unwrap();
}

/// An error past the anonymous path frame, a resource that is not there, still ends the login and pops the frames.
#[test]
fn an_anonymous_rest_path_that_fails_on_entry_ends_its_login() {
    let (_facade, ctx, _dir) = anonymous_context();
    let _entered = ctx.enter();
    let err = super::run(&["anon", "nothing"], "get", &BTreeMap::new()).unwrap_err();
    assert_eq!(ErrorKind::of(&err), ErrorKind::NotFound, "{err}");
    assert_clean(&ctx, None);
    let err = super::run(&["anon", "probe", "deeper"], "post", &id("DEEP")).unwrap_err();
    assert_eq!(ErrorKind::of(&err), ErrorKind::NotFound, "{err}");
    assert_clean(&ctx, None);
}
