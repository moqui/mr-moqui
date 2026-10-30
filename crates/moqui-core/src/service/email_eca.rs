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

//! Email ECA. Rules come from the factory's `*.emecas.xml` files. Actions run through the same
//! XML action host as service ECA.

use std::collections::BTreeMap;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::eca_rules;
use crate::context::Error;

#[derive(Clone, Debug)]
pub(super) struct EmailRule {
    condition: String,
    actions: String,
}

pub(super) fn email_rule_from(node: &MNode) -> EmailRule {
    EmailRule {
        condition: super::rule_actions::first_condition(node),
        actions: node
            .first_named("actions")
            .map(|child| child.write_string())
            .unwrap_or_default(),
    }
}

/// How many email ECA rules are installed, for a caller that runs each on its own ([`run_email_rule`]).
pub fn email_rule_count() -> usize {
    eca_rules::current().map_or(0, |rules| rules.email.len())
}

/// Run the email ECA rule at `index` (in load order) when its condition passes. An index past the rules (they were
/// reloaded meanwhile) runs nothing.
///
/// # Errors
///
/// What the rule's condition or actions fail with.
pub fn run_email_rule(index: usize, fields: &BTreeMap<String, Value>) -> Result<(), Error> {
    let Some(rules) = eca_rules::current() else {
        return Ok(());
    };
    match rules.email.get(index) {
        Some(rule) => run_rule(rule, fields),
        None => Ok(()),
    }
}

fn run_rule(rule: &EmailRule, fields: &BTreeMap<String, Value>) -> Result<(), Error> {
    let _frame = super::rule_actions::publish_names(fields)?;
    if super::rule_actions::condition_allows(&rule.condition)? {
        super::rule_actions::run_stored_actions(&rule.actions)?;
    }
    Ok(())
}

/// Run email ECA actions whose condition passes. `fields` are placed on the script context. A rule that fails is
/// logged and the others still run; the first failure is returned, so the poll leaves that message on the server.
pub fn run_email_rules(fields: &BTreeMap<String, Value>) -> Result<(), Error> {
    let Some(rules) = eca_rules::current() else {
        return Ok(());
    };
    let mut first = None;
    for rule in &rules.email {
        if let Err(err) = run_rule(rule, fields) {
            tracing::error!("email ECA rule failed: {err}");
            first.get_or_insert(err);
        }
    }
    first.map_or(Ok(()), Err)
}

/// Install SECA and EmECA rules from XML on a factory, for tests (moqui-mail's too).
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn install_rules(
    factory: &std::sync::Arc<crate::context::ExecutionContextFactory>,
    seca_xml: &str,
    email_xml: &str,
) {
    super::eca_rules::install(factory, super::eca_rules::from_xml(seca_xml, email_xml));
}

#[cfg(test)]
#[path = "email_eca_tests.rs"]
mod tests;
