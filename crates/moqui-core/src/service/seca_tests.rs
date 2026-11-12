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

use moqui_lib::object::Value;

use super::run_seca_rules;
use crate::service::eca_rules;

/// As Java's `ServiceEcaRule`: only the first child of `<condition>` is checked, and a name pattern that does not
/// parse is an error, so the loader skips that rule.
#[test]
fn only_the_first_condition_child_counts_and_a_bad_pattern_is_an_error() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="test.example#First" when="pre-service">
                    <condition><expression>true</expression><expression>false</expression></condition>
                    <actions><script>ec.message.addMessage("first");</script></actions>
                </seca>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    run_seca_rules("test.example#First", "pre-service", &mut BTreeMap::new(), None).unwrap();
    assert_eq!(context.message().messages(), ["first"]);
    let node = moqui_lib::node::MNode::parse_text(
        "seca",
        r#"<seca service="test.(unclosed" name-is-pattern="true" when="pre-service"/>"#,
    )
    .unwrap();
    assert!(super::seca_rule_from(&node).is_err());
}

/// Java's SECA load check: an entity-auto name of a defined entity counts as a defined service; a name that is
/// neither registered nor entity-auto does not.
#[test]
fn a_rule_for_an_entity_auto_service_is_kept_and_one_for_no_service_is_not() {
    let facade = crate::testing::open();
    let rule = |service: &str| {
        let node =
            moqui_lib::node::MNode::parse_text("seca", &format!(r#"<seca service="{service}" when="post-service"/>"#))
                .unwrap();
        super::seca_rule_from(&node).unwrap()
    };
    let known = &mut super::KnownServices::default();
    assert!(rule("update#moqui.basic.Enumeration").service_defined(Some(&facade), known));
    assert!(!rule("update#moqui.basic.NoSuchEntity").service_defined(Some(&facade), known));
    assert!(!rule("no.such.Services.do#Nothing").service_defined(Some(&facade), known));
}

#[test]
fn seca_for_a_test_service_runs_its_action() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="test.example#Run" when="pre-service" priority="8">
                    <actions><script>ec.message.addMessage("late");</script></actions>
                </seca>
                <seca service="test.example#Run" when="pre-service" priority="1">
                    <condition><expression>false</expression></condition>
                    <actions><script>ec.message.addMessage("skipped");</script></actions>
                </seca>
                <seca service="test.example#Run" when="pre-service" priority="2">
                    <actions><script>ec.message.addMessage("early");</script></actions>
                </seca>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    let mut parameters = BTreeMap::new();
    parameters.insert("note".to_string(), Value::string("go"));
    run_seca_rules("test.example#Run", "pre-service", &mut parameters, None).unwrap();
    // Priority order, and the false condition skips its rule.
    assert_eq!(context.message().messages(), ["early", "late"]);
    run_seca_rules("test.example#Run", "post-service", &mut parameters, None).unwrap();
    assert_eq!(context.message().messages(), ["early", "late"]);
}

/// A parameter or result named `context` (a commit status's) cannot be a name of the rule's frame, which `context`
/// names: the rule still runs and finds it in `parameters` and `results`.
#[test]
fn a_parameter_named_context_stays_in_parameters_and_results() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="test.example#Status" when="post-service">
                    <condition><expression>state == 'failure'</expression></condition>
                    <actions><script>ec.message.addMessage(parameters.context + " " + results.context);</script></actions>
                </seca>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    let mut parameters = BTreeMap::new();
    parameters.insert("context".to_string(), Value::string("ci/build"));
    parameters.insert("state".to_string(), Value::string("failure"));
    let mut results = BTreeMap::new();
    results.insert("context".to_string(), Value::string("ci/test"));
    run_seca_rules(
        "test.example#Status",
        "post-service",
        &mut parameters,
        Some(&mut results),
    )
    .unwrap();
    assert_eq!(context.message().messages(), ["ci/build ci/test"]);
}

/// Java writes each entity audit row with `create#moqui.entity.EntityAuditLog`, so that service's SECA rules run.
#[test]
fn an_entity_audit_row_is_created_through_its_service() {
    let facade = crate::testing::open();
    facade.set_disable_audit_log(false);
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas><seca service="create#moqui.entity.EntityAuditLog" when="pre-service">
                    <actions><script>ec.message.addMessage("audited " + changedFieldName);</script></actions>
                </seca></secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
    user.set("userId", Value::string("AUDIT_SECA"));
    user.set("username", Value::string("bob"));
    user.create(&facade).unwrap();
    user.set("username", Value::string("bob2"));
    user.update(&facade).unwrap();
    let audits = facade
        .find("moqui.entity.EntityAuditLog")
        .unwrap()
        .eq("changedFieldName", Value::string("username"))
        .list()
        .unwrap();
    assert_eq!(audits.len(), 2);
    assert_eq!(context.message().messages(), ["audited username", "audited username"]);
}

#[test]
fn pattern_and_named_rules_run_together_in_priority_order() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="test.example.Thing.run#Run" when="pre-service" priority="3">
                    <actions><script>ec.message.addMessage("named 3");</script></actions>
                </seca>
                <seca service="test\.example\..*" name-is-pattern="true" when="pre-service" priority="2">
                    <actions><script>ec.message.addMessage("pattern 2");</script></actions>
                </seca>
                <seca service="test.example.Thing.run#Run" when="pre-service" priority="1">
                    <actions><script>ec.message.addMessage("named 1");</script></actions>
                </seca>
                <seca service="test.example.Thing.run#Other" when="pre-service" priority="1">
                    <actions><script>ec.message.addMessage("other");</script></actions>
                </seca>
                <seca service="test\.example\..*" name-is-pattern="true" when="post-service" priority="1">
                    <actions><script>ec.message.addMessage("post");</script></actions>
                </seca>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    let run = |service: &str, when: &str| {
        context.message().clear();
        run_seca_rules(service, when, &mut BTreeMap::new(), None).unwrap();
        context.message().messages().to_vec()
    };
    assert_eq!(
        run("test.example.Thing.run#Run", "pre-service"),
        ["named 1", "pattern 2", "named 3"]
    );
    assert_eq!(
        run("test.example.Thing.run#Other", "pre-service"),
        ["other", "pattern 2"]
    );
    assert_eq!(run("test.other.Thing.run#Run", "pre-service"), Vec::<String>::new());
    assert_eq!(run("test.example.Thing.run#Run", "post-service"), ["post"]);
    assert!(run("test.example.Thing.run#Run", "pre-validate").is_empty());
}

#[test]
fn compare_and_combined_conditions_decide_whether_a_rule_runs() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="test.example#Cond" when="pre-service" priority="1">
                    <condition><compare field="note" operator="equals" value="go"/></condition>
                    <actions><script>ec.message.addMessage("compare");</script></actions>
                </seca>
                <seca service="test.example#Cond" when="pre-service" priority="2">
                    <condition><and>
                        <compare field="note" operator="equals" value="go"/>
                        <expression>count &gt; 2</expression>
                    </and></condition>
                    <actions><script>ec.message.addMessage("and");</script></actions>
                </seca>
                <seca service="test.example#Cond" when="pre-service" priority="3">
                    <condition><or>
                        <compare field="note" operator="equals" value="stop"/>
                        <not><expression>count &gt; 2</expression></not>
                    </or></condition>
                    <actions><script>ec.message.addMessage("or");</script></actions>
                </seca>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    let run = |note: &str, count: i64| {
        context.message().clear();
        let mut parameters = BTreeMap::new();
        parameters.insert("note".to_string(), Value::string(note));
        parameters.insert("count".to_string(), Value::I64(count));
        run_seca_rules("test.example#Cond", "pre-service", &mut parameters, None).unwrap();
        context.message().messages().to_vec()
    };
    assert_eq!(run("go", 5), ["compare", "and"]);
    assert_eq!(run("go", 1), ["compare", "or"]);
    assert_eq!(run("stop", 5), ["or"]);
    assert!(run("other", 5).is_empty());
}

#[test]
fn a_condition_element_that_is_not_a_condition_fails_the_rule() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="test.example#Odd" when="pre-service">
                    <condition><unknown-check field="note"/></condition>
                    <actions><script>ec.message.addMessage("ran");</script></actions>
                </seca>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    assert!(run_seca_rules("test.example#Odd", "pre-service", &mut BTreeMap::new(), None).is_err());
    assert!(context.message().messages().is_empty(), "the rule did not run");
}

/// Java's `getSecaRuleCount` counts a pattern rule once for each known service it matches; the rules loaded are
/// counted once each.
#[test]
fn the_rule_count_counts_a_pattern_once_per_matching_service() {
    let context = crate::context::ExecutionContextFactory::test_context();
    eca_rules::install(
        context.factory(),
        eca_rules::from_xml(
            r#"<secas>
                <seca service="count#One" when="pre-service"/>
                <seca service="count.*" name-is-pattern="true" when="post-service"/>
            </secas>"#,
            "",
        ),
    );
    let _entered = context.enter();
    let path = std::env::temp_dir().join(format!("moqui-svc-seca-count-{}.xml", std::process::id()));
    std::fs::write(
        &path,
        r#"<services><service verb="count" noun="One"/><service verb="count" noun="Two"/><service verb="other" noun="Three"/></services>"#,
    )
    .unwrap();
    crate::service::lint(&[path]).unwrap();
    assert_eq!(crate::service::info::seca_rules_active(), 2);
    assert_eq!(crate::service::info::seca_rule_count(), 3);
}
