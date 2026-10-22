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

//! The read fence (plans/LLM-A2A-Assist.md 4.6) on service calls: a service labelled READ that writes fails, async,
//! remote, mail and print calls, tx-commit rules, script HTTP and SQL that may write are refused, and the allowlisted
//! entities are written.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::pool::ServicePools;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::fence::{self, Effect, Violation, FENCE_ERROR};
use crate::entity::EntityFacade;

const SERVICES: &str = r#"<services>
    <service verb="read" noun="ButWrite" authenticate="false" read-only="true">
        <in-parameters><parameter name="id"/></in-parameters>
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", id); made.set("testMedium", "written");</script>
            <entity-create value-field="made"/>
        </actions>
    </service>
    <service verb="read" noun="Only" authenticate="false" read-only="true">
        <in-parameters><parameter name="id"/></in-parameters>
        <out-parameters><parameter name="found"/></out-parameters>
        <actions><script>found = ec.entity.find("moqui.test.TestEntity").condition("testId", id).one() != null ? "Y" : "N"</script></actions>
    </service>
    <service verb="ping" noun="Remote" type="remote-rest" authenticate="false" location="http://127.0.0.1:1/no-such-remote"/>
    <service verb="read" noun="WithCommitRule" authenticate="false" read-only="true">
        <in-parameters><parameter name="id"/></in-parameters>
    </service>
</services>"#;

/// A tx-commit rule of `read#WithCommitRule` that writes a row: it would run on the service pool, outside the fence.
const SECAS: &str = r#"<secas>
    <seca service="read#WithCommitRule" when="tx-commit">
        <actions>
            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
            <script>made.set("testId", parameters.id); made.set("testMedium", "from-commit-rule")</script>
            <entity-create value-field="made"/>
        </actions>
    </seca>
</secas>"#;

fn fence_context(name: &str) -> (EntityFacade, ExecutionContext) {
    let path = std::env::temp_dir().join(format!("moqui-svc-fence-{name}-{}.xml", std::process::id()));
    std::fs::write(&path, SERVICES).unwrap();
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context_in("", ServicePools::inline(), crate::context::framework_dir());
    ctx.factory().set_entity(facade.clone());
    super::eca_rules::install(ctx.factory(), super::eca_rules::from_xml(SECAS, ""));
    {
        let _entered = ctx.enter();
        let framework = crate::context::framework_dir().join("service/org/moqui");
        let paths = vec![
            path,
            framework.join("EmailServices.xml"),
            framework.join("impl/EmailServices.xml"),
            framework.join("impl/PrintServices.xml"),
        ];
        let failures = crate::service::lint(&paths).unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    (facade, ctx)
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

fn id(value: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([("id".to_string(), Value::string(value))])
}

/// The errors the call left, cleared for the next one.
fn take_errors(ctx: &ExecutionContext) -> String {
    let text = ctx.message().errors().join("\n");
    ctx.message().clear_errors();
    text
}

#[test]
fn a_read_service_that_writes_fails_under_the_fence_and_writes_without_it() {
    let (facade, ctx) = fence_context("read");
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    {
        let guard = fence::enter("tool find_things");
        let _ = super::sync("read#ButWrite", &id("FENCED"));
        let errors = take_errors(&ctx);
        assert!(errors.contains(FENCE_ERROR) && errors.contains("moqui.test.TestEntity"), "{errors}");
        assert_eq!(
            guard.violations(),
            vec![Violation::Write {
                entity: "moqui.test.TestEntity".to_string(),
                operation: "create".to_string()
            }]
        );
        // a read runs as it does unfenced
        let out = super::sync("read#Only", &id("FENCED")).unwrap();
        assert_eq!(out.get("found").and_then(Value::as_str), Some("N"));
        assert!(take_errors(&ctx).is_empty());
    }
    assert_eq!(medium(&facade, "FENCED"), None);
    super::sync("read#ButWrite", &id("UNFENCED")).unwrap();
    assert!(take_errors(&ctx).is_empty());
    assert_eq!(medium(&facade, "UNFENCED").as_deref(), Some("written"));
}

#[test]
fn the_allowlisted_entities_are_written_under_the_fence() {
    let (facade, ctx) = fence_context("allowed");
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let guard = fence::enter("render /apps/x");
    // the run bookkeeping, a sequenced id (its bank), a visit
    let mut call = facade.make_value("moqui.agent.AgentToolCall").unwrap();
    call.set_sequenced_id_primary(&facade).unwrap();
    call.set("toolName", Value::string("fenced"));
    call.create(&facade).unwrap();
    let mut visit = facade.make_value("moqui.server.Visit").unwrap();
    visit.set_sequenced_id_primary(&facade).unwrap();
    visit.create(&facade).unwrap();
    assert!(guard.violations().is_empty(), "{:?}", guard.violations());
    // and the rest is not: neither other data nor the agent's configuration and decisions
    let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
    row.set("enumId", Value::string("FENCED_ENUM"));
    let refused = row.create(&facade).unwrap_err();
    assert!(refused.to_string().contains(FENCE_ERROR), "{refused}");
    let mut log = facade.make_value("moqui.agent.AgentAuditLog").unwrap();
    log.set_sequenced_id_primary(&facade).unwrap();
    let refused = log.create(&facade).unwrap_err();
    assert!(refused.to_string().contains(FENCE_ERROR), "{refused}");
    drop(guard);
    let found = facade.find("moqui.agent.AgentToolCall").unwrap().eq("toolName", Value::string("fenced")).one().unwrap();
    assert!(found.is_some());
}

#[test]
fn async_remote_mail_and_print_calls_are_refused_under_the_fence() {
    let (facade, ctx) = fence_context("effects");
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let guard = fence::enter("tool run_tool");

    // async: refused, nothing queued
    let refused = super::async_call("read#ButWrite", &id("ASYNC")).unwrap_err();
    assert!(refused.to_string().contains(FENCE_ERROR), "{refused}");
    assert_eq!(ctx.factory().service_pools().pending(), 0);

    // remote REST: refused before a socket
    let _ = super::sync("ping#Remote", &BTreeMap::new());
    let errors = take_errors(&ctx);
    assert!(errors.contains(FENCE_ERROR) && errors.contains("a remote call"), "{errors}");

    // mail
    #[cfg(feature = "smtp")]
    {
        let message = BTreeMap::from([("emailMessageId".to_string(), Value::string("NO_SUCH_MESSAGE"))]);
        let _ = super::sync("org.moqui.impl.EmailServices.send#EmailMessage", &message);
        let errors = take_errors(&ctx);
        assert!(errors.contains(FENCE_ERROR) && errors.contains("mail (send#EmailMessage)"), "{errors}");
    }

    // printing
    let print_job: indexmap::IndexMap<String, Value> =
        [("printJobId".to_string(), Value::string("FENCED_JOB"))].into_iter().collect();
    let job = BTreeMap::from([("printJob".to_string(), Value::Map(print_job))]);
    let _ = super::sync("org.moqui.impl.PrintServices.send#PrintJobInternal", &job);
    let errors = take_errors(&ctx);
    assert!(errors.contains(FENCE_ERROR) && errors.contains("printing"), "{errors}");

    let effects: Vec<Effect> = guard
        .violations()
        .into_iter()
        .filter_map(|violation| match violation {
            Violation::Effect { effect, .. } => Some(effect),
            Violation::Write { .. } => None,
        })
        .collect();
    assert!(effects.contains(&Effect::AsyncCall) && effects.contains(&Effect::RemoteCall) && effects.contains(&Effect::Print));
    drop(guard);
    assert_eq!(ctx.factory().run_pending(), 0);
    assert_eq!(medium(&facade, "ASYNC"), None);
}

#[test]
fn a_read_with_tx_commit_rules_is_refused_and_its_rule_never_runs() {
    let (facade, ctx) = fence_context("seca");
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    {
        let guard = fence::enter("tool find_things");
        let _ = super::sync("read#WithCommitRule", &id("SECA_FENCED"));
        let errors = take_errors(&ctx);
        assert!(errors.contains(FENCE_ERROR) && errors.contains("tx-commit"), "{errors}");
        assert!(guard
            .violations()
            .iter()
            .any(|violation| matches!(violation, Violation::Effect { effect: Effect::AsyncCall, .. })));
    }
    assert_eq!(ctx.factory().run_pending(), 0);
    assert_eq!(medium(&facade, "SECA_FENCED"), None);
    // unfenced, the rule runs after the commit
    super::sync("read#WithCommitRule", &id("SECA_OPEN")).unwrap();
    ctx.factory().run_pending();
    assert_eq!(medium(&facade, "SECA_OPEN").as_deref(), Some("from-commit-rule"));
}

#[test]
fn script_http_and_sql_that_may_write_are_refused_under_the_fence() {
    let (facade, ctx) = fence_context("script");
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let guard = fence::enter("tool run_tool");
    for (verb, call) in [("call", "call()"), ("send", "send()")] {
        let ran = crate::script::run_actions(&format!(
            r#"<actions><script><![CDATA[
                const rc = ec.service.rest().method("GET")
                rc.uri().host("127.0.0.1").port(1).path("fenced").build()
                rc.{call}
            ]]></script></actions>"#
        ));
        let text = ran.err().map(|error| error.to_string()).unwrap_or_default();
        assert!(text.contains(FENCE_ERROR) && text.contains("HTTP GET"), "{verb}: {text}");
    }
    // the outbound backstop refuses any other caller too
    let sent = crate::outbound::send(crate::outbound::Request {
        method: crate::outbound::Method::Get,
        url: "http://127.0.0.1:1/fenced".to_string(),
        headers: Vec::new(),
        body: None,
        timeout: None,
    });
    assert!(sent.is_err_and(|error| error.message().contains(FENCE_ERROR)));
    // raw SQL: a query runs, a write does not
    crate::entity::info::run_sql(&facade, "transactional", "SELECT 1", 10).unwrap();
    let refused = crate::entity::info::run_sql(&facade, "transactional", "DELETE FROM test_entity", 10).unwrap_err();
    assert!(refused.to_string().contains(FENCE_ERROR), "{refused}");
    let effects: Vec<Effect> = guard
        .violations()
        .into_iter()
        .filter_map(|violation| match violation {
            Violation::Effect { effect, .. } => Some(effect),
            Violation::Write { .. } => None,
        })
        .collect();
    assert_eq!(effects, [Effect::RemoteCall, Effect::RemoteCall, Effect::RemoteCall, Effect::Sql]);
}
