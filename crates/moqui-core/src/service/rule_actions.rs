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

//! Running action XML for rules: the frame that holds a rule's parameters, its condition, and its
//! actions. SECA, email ECA, and entity ECA share it.

use std::collections::BTreeMap;

use moqui_lib::object::Value;

/// Push a script frame holding `values`. The frame pops when the guard drops.
pub(crate) fn publish_names(
    values: &BTreeMap<String, Value>,
) -> Result<crate::script::ContextFrame, crate::context::Error> {
    let frame = crate::script::ContextFrame::push();
    for (name, value) in values {
        // `context` names the context itself; a parameter of that name (a commit status's) is in `parameters`
        if name == "context" {
            continue;
        }
        crate::script::set_context_name(name, value.clone())?;
    }
    Ok(frame)
}

pub(crate) use crate::entity::load_feed::first_condition;

/// The scope a service call holds, for a rule's inline condition or actions (an entity, service, email or git ECA rule):
/// a `disableAuthz()`, `disableEntityEca()` or `disableTarpit()` they leave on ends with them (SEC-RTE-92).
fn keep_authz() -> (Option<crate::context::AuthzScope>, crate::script::preview::PreviewCode) {
    (
        crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::keep_authz),
        // the rule is the framework's or a component's, not a preview's text (SEC-AI-102)
        crate::script::preview::lift(),
    )
}

/// An empty condition passes. A [`first_condition`] group is evaluated as `<if>` evaluates its
/// condition macros (`expression`, `compare`, `and`, `or`, `not`); any other child is an error, so
/// a rule never runs because its condition was not understood. Plain text is an expression.
pub(crate) fn condition_allows(condition: &str) -> Result<bool, crate::context::Error> {
    let trimmed = condition.trim();
    if trimmed.is_empty() {
        return Ok(true);
    }
    let _authz = keep_authz();
    if trimmed.starts_with('<') {
        return crate::script::eval_condition_group(trimmed);
    }
    crate::script::eval_condition(trimmed)
}

/// Run action XML on the shared action runner. A bare element is wrapped in `<actions>`.
pub(crate) fn run_stored_actions(actions: &str) -> Result<(), crate::context::Error> {
    let trimmed = actions.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let _authz = keep_authz();
    if trimmed.starts_with("<actions") {
        return crate::script::run_actions(trimmed);
    }
    crate::script::run_actions(&format!("<actions>{trimmed}</actions>"))
}

/// [`run_stored_actions`] with the rule's prepared script kept in `prepared` (plans/Profiling3.md W1).
pub(crate) fn run_prepared_actions(
    actions: &str,
    prepared: &crate::script::PreparedActions,
) -> Result<(), crate::context::Error> {
    let trimmed = actions.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let _authz = keep_authz();
    prepared.run_with(
        || {
            if trimmed.starts_with("<actions") {
                std::borrow::Cow::Borrowed(trimmed)
            } else {
                std::borrow::Cow::Owned(format!("<actions>{trimmed}</actions>"))
            }
        },
        None,
    )
}

#[cfg(test)]
mod tests {
    use moqui_lib::node::MNode;

    use super::first_condition;

    #[test]
    fn a_rule_condition_is_its_first_child_as_java_reads_it() {
        let rule = MNode::parse_text(
            "eeca",
            "<eeca><condition><expression>true</expression><expression>false</expression></condition></eeca>",
        )
        .unwrap();
        let condition = first_condition(&rule);
        assert!(condition.starts_with("<and><expression>"), "{condition}");
        assert!(
            condition.contains("true") && !condition.contains("false"),
            "{condition}"
        );
        let empty = MNode::parse_text("eeca", "<eeca><condition/></eeca>").unwrap();
        assert_eq!(first_condition(&empty), "");
        let none = MNode::parse_text("emeca", "<emeca/>").unwrap();
        assert_eq!(first_condition(&none), "");
    }

    /// A rule's inline condition and actions hold the scope a service call holds (SEC-RTE-92): what they disable, by a
    /// throw or by never enabling it, ends with them.
    #[test]
    fn a_rules_condition_and_actions_end_what_they_disable() {
        crate::script::clear_context_stack();
        let context = crate::context::ExecutionContextFactory::test_context();
        let _entered = context.enter();
        assert!(super::condition_allows("ec.artifactExecution.disableAuthz() === false").unwrap());
        assert!(
            !context.authz_disabled(),
            "a rule's condition left authorization disabled"
        );
        super::run_stored_actions(
            "<script>ec.artifactExecution.disableAuthz(); ec.artifactExecution.disableEntityEca()</script>",
        )
        .unwrap();
        assert!(
            !context.authz_disabled(),
            "a rule's actions left authorization disabled"
        );
        assert!(
            !context.entity_eca_disabled(),
            "a rule's actions left entity ECA rules disabled"
        );
        let prepared = crate::script::PreparedActions::default();
        let failed = super::run_prepared_actions(
            "<script>ec.artifactExecution.disableTarpit(); throw new Error('down')</script>",
            &prepared,
        );
        assert!(failed.is_err());
        assert!(
            !context.tarpit_disabled(),
            "a rule's failing actions left tarpit counting disabled"
        );
    }
}
