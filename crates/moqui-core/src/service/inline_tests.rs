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

//! The inline executor: async work drains on the calling thread, `callFuture()` runs at once.

use std::collections::BTreeMap;
use std::path::PathBuf;

use moqui_lib::object::Value;

use super::pool::ServicePools;
use crate::context::{BootOptions, ExecutionContext, ExecutionContextFactory, Executor};
use crate::entity::EntityFacade;

const SERVICES: &str = r#"<services>
    <service verb="inline" noun="Mark" authenticate="false">
        <in-parameters><parameter name="id"/><parameter name="after"/></in-parameters>
        <actions>
            <entity-find-one entity-name="moqui.test.TestEntity" value-field="prior">
                <field-map field-name="testId" from="after"/>
            </entity-find-one>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", id); made.set("testMedium", prior ? "seen" : "missing");</script>
            <entity-create value-field="made"/>
        </actions>
    </service>
    <service verb="inline" noun="Echo" authenticate="false" transaction="ignore">
        <in-parameters><parameter name="note"/></in-parameters>
        <out-parameters><parameter name="note"/></out-parameters>
    </service>
    <service verb="inline" noun="MarkOrFail" authenticate="false">
        <in-parameters><parameter name="id"/></in-parameters>
        <actions>
            <if condition="id == 'BAD'"><return error="true" message="bad row"/></if>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", id); made.set("testMedium", "made");</script>
            <entity-create value-field="made"/>
        </actions>
    </service>
    <service verb="inline" noun="IgnoreFail" authenticate="false" transaction="ignore">
        <actions><return error="true" message="ignore failed"/></actions>
    </service>
    <service verb="inline" noun="NoCache" authenticate="false" no-tx-cache="true">
        <in-parameters><parameter name="id"/></in-parameters>
        <out-parameters><parameter name="found"/></out-parameters>
        <actions>
            <script>found = ec.entity.find("moqui.test.TestEntity").condition("testId", id).useCache(false).one() != null ? "Y" : "N"</script>
        </actions>
    </service>
</services>"#;

fn inline_context() -> (EntityFacade, ExecutionContext) {
    // A file of its own for each test: tests run at once, and one writing the file another reads is a parse error.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("moqui-svc-inline-executor-{}-{next}.xml", std::process::id()));
    std::fs::write(&path, SERVICES).unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context_with("", ServicePools::inline());
    ctx.factory().set_entity(facade.clone());
    {
        let _entered = ctx.enter();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    (facade, ctx)
}

fn mark(id: &str, after: &str) -> BTreeMap<String, Value> {
    let mut params = BTreeMap::new();
    params.insert("id".into(), Value::string(id));
    params.insert("after".into(), Value::string(after));
    params
}

fn medium(facade: &EntityFacade, id: &str) -> Option<String> {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(id))
        .one()
        .unwrap()
        .and_then(|row| row.get("testMedium").and_then(Value::as_str).map(str::to_string))
}

#[test]
fn async_calls_wait_for_run_pending_and_run_in_order() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    super::async_call("inline#Mark", &mark("IA", "none")).unwrap();
    super::async_call("inline#Mark", &mark("IB", "IA")).unwrap();
    assert_eq!(ctx.factory().service_pools().pending(), 2);
    assert_eq!(medium(&facade, "IA"), None, "queued work ran early");
    assert_eq!(ctx.factory().run_pending(), 2);
    assert_eq!(medium(&facade, "IA").as_deref(), Some("missing"));
    assert_eq!(medium(&facade, "IB").as_deref(), Some("seen"));
    assert_eq!(ctx.factory().run_pending(), 0);
}

#[test]
fn leaving_the_outermost_context_drains() {
    let (facade, ctx) = inline_context();
    {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        super::async_call("inline#Mark", &mark("IC", "none")).unwrap();
        {
            let _inner = ctx.enter();
        }
        assert_eq!(ctx.factory().service_pools().pending(), 1, "an inner guard drained");
    }
    assert_eq!(ctx.factory().service_pools().pending(), 0);
    assert_eq!(medium(&facade, "IC").as_deref(), Some("missing"));
}

#[test]
fn run_pending_waits_while_a_transaction_is_in_place() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    assert!(tx.begin(None).unwrap());
    super::async_call("inline#Mark", &mark("ID", "none")).unwrap();
    assert_eq!(ctx.factory().run_pending(), 0);
    tx.commit().unwrap();
    assert_eq!(ctx.factory().run_pending(), 1);
    assert_eq!(medium(&facade, "ID").as_deref(), Some("missing"));
}

#[test]
fn a_failed_queued_call_is_logged_and_the_rest_run() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    // As Java's `validateCall`, an unknown service is an error before anything is queued.
    assert!(super::async_call("inline#NoSuchService", &BTreeMap::new()).is_err());
    assert_eq!(ctx.factory().service_pools().pending(), 0);
    let mut bad = BTreeMap::new();
    bad.insert("id".into(), Value::string("BAD"));
    super::async_call("inline#MarkOrFail", &bad).unwrap();
    super::async_call("inline#Mark", &mark("IE", "none")).unwrap();
    assert_eq!(ctx.factory().run_pending(), 2);
    assert_eq!(medium(&facade, "IE").as_deref(), Some("missing"));
}

#[test]
fn call_future_and_start_future_run_at_once() {
    let (_facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let mut params = BTreeMap::new();
    params.insert("note".into(), Value::string("hello"));
    let out = super::call_future("inline#Echo", &params).unwrap();
    assert_eq!(out.get("note").and_then(Value::as_str), Some("hello"));
    let mut future = super::start_future("inline#Echo", &params).unwrap();
    let out = future.get(None).unwrap().expect("settled");
    assert_eq!(out.get("note").and_then(Value::as_str), Some("hello"));
    assert!(super::call_future("inline#NoSuchService", &params).is_err());
    assert_eq!(ctx.factory().service_pools().pending(), 0);
}

#[test]
fn an_inline_boot_starts_no_pool_and_no_deadline_timer() {
    let runtime = std::env::temp_dir().join(format!("moqui-inline-boot-{}", std::process::id()));
    std::fs::create_dir_all(&runtime).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = {
        let _lock = crate::user::session::cookie_property_lock();
        ExecutionContextFactory::boot_with_options(
            &runtime,
            &framework,
            BootOptions {
                executor: Executor::Inline,
            },
        )
        .expect("boot")
    };
    assert_eq!(factory.executor(), Executor::Inline);
    assert!(factory.service_pools().is_inline());
    assert_eq!(factory.service_pools().sizes(), (0, 0));
    factory.start_request_pool().unwrap();
    assert!(factory
        .request_pool()
        .try_submit(std::sync::Arc::clone(&factory), || Ok(()))
        .is_err());
    assert!(!crate::transaction::single_threaded() || cfg!(target_os = "emscripten"));
    let ctx = factory.new_context();
    {
        let _entered = ctx.enter();
        assert!(crate::transaction::single_threaded(), "the deadline timer would start");
    }
    drop(ctx);
    drop(factory);
    let _ = std::fs::remove_dir_all(&runtime);
}

#[test]
fn an_ignore_service_runs_in_the_callers_transaction_and_its_error_marks_it() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    assert!(tx.begin(None).unwrap());
    super::sync("inline#IgnoreFail", &BTreeMap::new()).unwrap();
    assert!(tx.in_transaction(), "ignore left the caller's transaction in place");
    let err = tx.commit().expect_err("the caller's transaction is rollback-only");
    assert!(err.to_string().contains("inline#IgnoreFail"), "{err}");
    ctx.message().clear();
}

#[test]
fn a_no_tx_cache_service_sees_the_rows_the_caller_held() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let tx = facade.transactions_for("moqui.test.TestEntity").unwrap();
    tx.run(crate::transaction::TransactionAttribute::Cache, None, || {
        super::sync("inline#Mark", &mark("NC", "none"))?;
        assert!(tx.transaction_cache_active());
        let mut id = BTreeMap::new();
        id.insert("id".into(), Value::string("NC"));
        let out = super::sync("inline#NoCache", &id)?;
        assert_eq!(out.get("found"), Some(&Value::string("Y")));
        assert!(!tx.transaction_cache_active(), "no-tx-cache stops holding");
        Ok(())
    })
    .unwrap();
    assert_eq!(medium(&facade, "NC").as_deref(), Some("missing"));
}

#[test]
fn a_builder_changes_in_place_and_an_in_map_of_the_context_passes_it() {
    let (_facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::script::run_actions(
        r#"<actions>
            <set field="note" value="from context"/>
            <service-call name="inline#Echo" in-map="context" out-map="viaContext"/>
            <service-call name="inline#Echo" in-map="true" out-map="viaTrue">
                <field-map field-name="note" value="from field map"/>
            </service-call>
            <script>(function () {
                const builder = ec.service.sync().name("inline#Echo");
                sameBuilder = builder.parameter("note", "chained") === builder;
                chained = builder.call().note;
                fromObject = ec.service.sync().name("inline#Echo").parameters({note: "from object"}).call().note;
            })()</script>
        </actions>"#,
    )
    .unwrap();
    for check in [
        "viaContext.note === 'from context'",
        "viaTrue.note === 'from field map'",
        "sameBuilder === true",
        "chained === 'chained'",
        "fromObject === 'from object'",
    ] {
        assert!(crate::script::eval_condition(check).unwrap(), "{check}");
    }
    crate::script::clear_context_stack();
}

#[test]
fn a_multi_call_runs_each_row_in_one_transaction() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::script::run_script(
        r#"ec.service.sync().name("inline#Mark").parameters({id_0: "MU0", after_0: "none", id_1: "MU1", after_1: "MU0",
            id_2: "MU2", after_2: "none", _useRowSubmit_2: "true"}).multi(true).call()"#,
    )
    .unwrap();
    assert_eq!(medium(&facade, "MU0").as_deref(), Some("missing"));
    assert_eq!(
        medium(&facade, "MU1").as_deref(),
        Some("seen"),
        "row 1 sees row 0 in the same transaction"
    );
    assert_eq!(
        medium(&facade, "MU2"),
        None,
        "a row that uses row submit without _rowSubmit_2 is skipped"
    );
    // An error in a row stops the rows and rolls back the ones before it.
    crate::script::run_script(
        r#"ec.service.sync().name("inline#MarkOrFail").parameters({id_0: "MV0", id_1: "BAD", id_2: "MV2"}).multi(true).call()"#,
    )
    .unwrap();
    assert!(
        ctx.message().errors().iter().any(|error| error.contains("bad row")),
        "{:?}",
        ctx.message().errors()
    );
    assert_eq!(medium(&facade, "MV0"), None, "row 0 rolled back with the failed row 1");
    assert_eq!(medium(&facade, "MV2"), None, "the rows after the error do not run");
    ctx.message().clear();
}

const AUTHZ_SERVICES: &str = r#"<services>
    <service verb="authz" noun="Whoami">
        <in-parameters>
            <parameter name="id" required="true"/>
            <parameter name="count" type="Integer"/>
            <parameter name="authUsername"/>
        </in-parameters>
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", id); made.set("testMedium", ec.user.username + "/" + authUsername);</script>
            <entity-create value-field="made"/>
        </actions>
    </service>
    <service verb="call" noun="Whoami" authenticate="false">
        <actions>
            <service-call name="authz#Whoami"><field-map field-name="id" value="NESTED"/></service-call>
        </actions>
    </service>
    <service verb="create" noun="moqui.test.TestEntity" type="entity-auto" authenticate="false">
        <in-parameters><auto-parameters include="all"/></in-parameters>
    </service>
</services>"#;

/// An inline executor with service and entity authorization on, `authz#Whoami` allowed to the group `ASYNC_CALLERS`, and
/// two users: `asyncok` in that group and `asyncplain` in none. Nobody is allowed any entity, not even through
/// `create#moqui.test.TestEntity`, which needs no authentication; `call#Whoami`, which needs none either, calls
/// `authz#Whoami`.
fn authz_context() -> (EntityFacade, ExecutionContext) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("moqui-svc-inline-authz-{}-{next}.xml", std::process::id()));
    std::fs::write(&path, AUTHZ_SERVICES).unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context_with(
        r#"<moqui-conf><artifact-execution-facade>
            <artifact-execution type="AT_SERVICE" authz-enabled="true"/>
            <artifact-execution type="AT_ENTITY" authz-enabled="true"/>
        </artifact-execution-facade></moqui-conf>"#,
        ServicePools::inline(),
    );
    ctx.factory().set_entity(facade.clone());
    {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
        let create = |entity: &str, pairs: &[(&str, &str)]| {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in pairs {
                row.set(*name, Value::string(*value));
            }
            row.create(&facade).unwrap();
        };
        for user in ["asyncok", "asyncplain"] {
            create("moqui.security.UserAccount", &[("userId", user), ("username", user)]);
        }
        let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userId", Value::string("asyncok"));
        member.set("userGroupId", Value::string("ASYNC_CALLERS"));
        member.set(
            "fromDate",
            moqui_lib::object::basic_convert(&Value::I64(1_000), "java.sql.Timestamp").value,
        );
        member.create(&facade).unwrap();
        create("moqui.security.ArtifactGroup", &[("artifactGroupId", "ASYNC_SERVICES")]);
        create(
            "moqui.security.ArtifactGroupMember",
            &[
                ("artifactGroupId", "ASYNC_SERVICES"),
                ("artifactName", "authz#Whoami"),
                ("artifactTypeEnumId", "AT_SERVICE"),
                ("nameIsPattern", "N"),
                ("inheritAuthz", "N"),
            ],
        );
        create(
            "moqui.security.ArtifactAuthz",
            &[
                ("artifactAuthzId", "ASYNC_ALLOW"),
                ("userGroupId", "ASYNC_CALLERS"),
                ("artifactGroupId", "ASYNC_SERVICES"),
                ("authzTypeEnumId", "AUTHZT_ALLOW"),
                ("authzActionEnumId", "AUTHZA_ALL"),
            ],
        );
    }
    (facade, ctx)
}

fn whoami(id: &str) -> BTreeMap<String, Value> {
    let mut params = BTreeMap::new();
    params.insert("id".into(), Value::string(id));
    params
}

/// As Java's `validateCall`, a caller who may not run the service cannot queue it.
#[test]
fn an_async_call_the_caller_may_not_run_is_refused_before_it_is_queued() {
    let (facade, ctx) = authz_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("asyncplain").unwrap();
    let err = super::async_call("authz#Whoami", &whoami("NO1")).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::Unauthorized,
        "{err}"
    );
    assert!(super::call_future("authz#Whoami", &whoami("NO2")).is_err());
    assert_eq!(ctx.factory().service_pools().pending(), 0);
    crate::user::logout().unwrap();
    assert_eq!(ctx.factory().run_pending(), 0);
    let _authz = ctx.disable_authz();
    assert_eq!(medium(&facade, "NO1"), None);
    assert_eq!(medium(&facade, "NO2"), None);
}

/// The queue-time check passed, so the run has authorization disabled, as Java's `AsyncServiceInfo`: the service's
/// entity write, which no one is allowed, goes through. It runs as the user who queued it, with `authUsername` added.
#[test]
fn an_authorized_async_call_runs_as_its_caller_with_authz_disabled() {
    let (facade, ctx) = authz_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("asyncok").unwrap();
    super::async_call("authz#Whoami", &whoami("OK1")).unwrap();
    assert_eq!(ctx.factory().service_pools().pending(), 1);
    crate::user::logout().unwrap();
    assert_eq!(ctx.factory().run_pending(), 1);
    {
        let _authz = ctx.disable_authz();
        assert_eq!(medium(&facade, "OK1").as_deref(), Some("asyncok/asyncok"));
    }
    // Run now, as the inline executor runs a future.
    crate::user::internal_login_user("asyncok").unwrap();
    super::call_future("authz#Whoami", &whoami("OK2")).unwrap();
    let _authz = ctx.disable_authz();
    assert_eq!(medium(&facade, "OK2").as_deref(), Some("asyncok/asyncok"));
}

/// As Java's `callSingle` rethrows `ArtifactAuthorizationException`, an authorization failure in a service's body is an
/// error of the call, not a recorded failure: the service needs no authorization, its entity write is refused.
#[test]
fn an_authorization_failure_in_the_body_is_still_an_error() {
    let (facade, ctx) = authz_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("asyncok").unwrap();
    let mut params = BTreeMap::new();
    params.insert("testId".to_string(), Value::string("DENIED"));
    let err = super::sync("create#moqui.test.TestEntity", &params).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::Unauthorized,
        "{err}"
    );
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    crate::user::logout().unwrap();
    let _authz = ctx.disable_authz();
    assert_eq!(medium(&facade, "DENIED"), None);
}

/// The same for an inline body: the authorization failure of its entity write crosses the script with its kind, as
/// Java's `ArtifactAuthorizationException` passes through Groovy, so the call is an error and records nothing.
#[test]
fn an_authorization_failure_in_an_inline_body_is_still_an_error() {
    let (facade, ctx) = authz_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("asyncok").unwrap();
    let err = super::sync("authz#Whoami", &whoami("INLINE")).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::Unauthorized,
        "{err}"
    );
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    crate::user::logout().unwrap();
    let _authz = ctx.disable_authz();
    assert_eq!(medium(&facade, "INLINE"), None);
}

/// A `service-call` the user may not make is thrown out of the calling body, as Java's call builder throws, so the
/// outer call is an authorization error too.
#[test]
fn a_service_call_the_user_may_not_make_fails_the_calling_service() {
    let (facade, ctx) = authz_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("asyncplain").unwrap();
    let err = super::sync("call#Whoami", &BTreeMap::new()).unwrap_err();
    assert_eq!(
        crate::context::ErrorKind::of(&err),
        crate::context::ErrorKind::Unauthorized,
        "{err}"
    );
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    crate::user::logout().unwrap();
    let _authz = ctx.disable_authz();
    assert_eq!(medium(&facade, "NESTED"), None);
}

/// As Java's `validateCall`, input that fails validation is an error in the caller's message facade, and nothing runs.
#[test]
fn an_async_call_with_invalid_input_reports_it_when_queued() {
    let (facade, ctx) = authz_context();
    let _entered = ctx.enter();
    crate::user::internal_login_user("asyncok").unwrap();
    let mut params = whoami("BADINPUT");
    params.insert("count".into(), Value::string("not a number"));
    super::async_call("authz#Whoami", &params).unwrap();
    assert!(!ctx.message().errors().is_empty(), "no validation error at queue time");
    assert_eq!(ctx.factory().service_pools().pending(), 0);
    ctx.message().clear();
    let mut future = super::start_future("authz#Whoami", &params).unwrap();
    assert_eq!(future.get(None).unwrap(), Some(BTreeMap::new()));
    assert!(!ctx.message().errors().is_empty(), "no validation error for the future");
    ctx.message().clear();
    crate::user::logout().unwrap();
    assert_eq!(ctx.factory().run_pending(), 0);
    let _authz = ctx.disable_authz();
    assert_eq!(medium(&facade, "BADINPUT"), None);
}

/// As Java's `ServiceCallAsyncImpl`, an async call in a sim session is skipped: a worker would not run in the sim.
#[test]
fn an_async_call_in_a_sim_session_is_skipped() {
    let (facade, ctx) = inline_context();
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    facade.set_sim_active(true);
    super::async_call("inline#Mark", &mark("SIM1", "none")).unwrap();
    assert_eq!(ctx.factory().service_pools().pending(), 0);
    let out = super::call_future("inline#Echo", &BTreeMap::new()).unwrap();
    assert_eq!(out.get("simSkipped"), Some(&Value::Bool(true)));
    assert_eq!(out.get("serviceName").and_then(Value::as_str), Some("inline#Echo"));
    facade.set_sim_active(false);
    assert_eq!(ctx.factory().run_pending(), 0);
    assert_eq!(medium(&facade, "SIM1"), None);
}
