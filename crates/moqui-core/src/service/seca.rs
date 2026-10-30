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

//! Service ECA. Rules come from the factory's `*.secas.xml` files (see `eca_rules`).

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::{read, write};

use super::eca_rules;
use super::util::has_error;
use crate::context::Error;

#[derive(Clone, Debug)]
pub(super) struct SecaRule {
    id: String,
    /// The `service` attribute as written, with its `#`.
    service_name: String,
    service_plain: String,
    when_name: String,
    name_is_pattern: bool,
    /// The service name pattern, parsed when the rules load.
    pattern: Option<moqui_lib::object::CompiledRegex>,
    run_on_error: bool,
    pub(super) priority: i32,
    /// The `<condition>` element as written, for the tools screens.
    condition: String,
    /// What the rule checks: as Java, the first child of `<condition>` only, made into the condition group once.
    condition_check: Option<String>,
    actions: String,
}

/// Run SECA actions whose service and `when` match. A false condition skips the rule.
///
/// Each rule runs in its own context frame holding the parameters, `parameters`, and, for the
/// rules that run after the service, the results and `results`. As Java's, `parameters` and `results` are the call's
/// own maps: what a rule sets in them is in `parameters` and `results` when it ends, so a pre-* rule changes the
/// service's input and a post-* rule its results.
pub fn run_seca_rules(
    service: &str,
    when: &str,
    parameters: &mut BTreeMap<String, Value>,
    mut results: Option<&mut BTreeMap<String, Value>>,
) -> Result<(), Error> {
    let Some(rules) = eca_rules::current() else {
        return Ok(());
    };
    for &position in rules.seca_index.matching(&rules.seca, service, when).iter() {
        let rule = &rules.seca[position];
        if !rule.run_on_error && has_error() {
            continue;
        }
        let _frame = super::rule_actions::publish_names(parameters)?;
        crate::script::set_context_name("parameters", Value::Map(to_index_map(parameters)))?;
        if let Some(results) = results.as_deref() {
            for (name, value) in results {
                if name == "context" {
                    continue;
                }
                crate::script::set_context_name(name, value.clone())?;
            }
            crate::script::set_context_name("results", Value::Map(to_index_map(results)))?;
        }
        let in_rule = |err: Error| {
            Error::new(format!(
                "service ECA {} on {when} of {service}: {}",
                rule.id,
                err.message()
            ))
        };
        let passed = match &rule.condition_check {
            Some(check) => crate::script::eval_condition_group(check).map_err(in_rule)?,
            None => true,
        };
        if passed {
            super::rule_actions::run_stored_actions(&rule.actions).map_err(in_rule)?;
        }
        read_back("parameters", parameters);
        if let Some(results) = results.as_deref_mut() {
            read_back("results", results);
        }
    }
    Ok(())
}

/// The map a rule left under `name` in its frame, in place of `map`. Anything but a map leaves `map` as it was.
fn read_back(name: &str, map: &mut BTreeMap<String, Value>) {
    if let Some(Value::Map(values)) = crate::script::context_value(name) {
        *map = values.into_iter().collect();
    }
}

/// Whether any rule runs for `service` at `when`, so a transaction hook is registered only when one does.
pub(super) fn has_rules(service: &str, when: &str) -> bool {
    eca_rules::current().is_some_and(|rules| !rules.seca_index.matching(&rules.seca, service, when).is_empty())
}

/// The positions of the pattern rules that matched, by `when`, then service name. Nested, so a lookup makes no key.
type PatternHits = HashMap<String, HashMap<String, Arc<[usize]>>>;

/// The SECA rules by `when` and service, made once when the rules load, so a call does not scan every rule at each of
/// its phases. Each list holds positions in the priority order.
#[derive(Default)]
pub(super) struct SecaIndex {
    /// By `when`, then by the service name without `#`.
    exact: HashMap<String, HashMap<String, Vec<usize>>>,
    /// The rules whose name is a pattern, by `when`. A pattern is matched against the name a call has, the first time a
    /// service is called (`pattern_hits`); Java expands it at load to the known services, so there it never runs for a
    /// service it did not know then, as an implicit entity-auto one (an accepted difference, P7a-08).
    patterns: HashMap<String, Vec<usize>>,
    /// The pattern rules that matched, by `when` and service name, so each pattern runs once for a service.
    pattern_hits: std::sync::RwLock<PatternHits>,
}

impl SecaIndex {
    pub(super) fn new(rules: &[Arc<SecaRule>]) -> Self {
        let mut index = Self::default();
        for (position, rule) in rules.iter().enumerate() {
            let when = rule.when_name.clone();
            if rule.name_is_pattern {
                index.patterns.entry(when).or_default().push(position);
            } else {
                index
                    .exact
                    .entry(when)
                    .or_default()
                    .entry(rule.service_plain.clone())
                    .or_default()
                    .push(position);
            }
        }
        index
    }

    /// The positions of the rules of `rules`, the list this index was made from, that run for `service` at `when`,
    /// in priority order.
    fn matching<'a>(&'a self, rules: &[Arc<SecaRule>], service: &str, when: &str) -> Cow<'a, [usize]> {
        let patterns = self.patterns.get(when).map_or(&[][..], Vec::as_slice);
        let exact = self.exact.get(when);
        if patterns.is_empty() && exact.is_none() {
            return Cow::Borrowed(&[]);
        }
        let plain: Cow<'_, str> = if service.contains('#') {
            Cow::Owned(service.replace('#', ""))
        } else {
            Cow::Borrowed(service)
        };
        let named = exact
            .and_then(|by_service| by_service.get(plain.as_ref()))
            .map_or(&[][..], Vec::as_slice);
        if patterns.is_empty() {
            return Cow::Borrowed(named);
        }
        let cached = read(&self.pattern_hits)
            .get(when)
            .and_then(|by_service| by_service.get(plain.as_ref()))
            .cloned();
        let hits = cached.unwrap_or_else(|| {
            let hits: Arc<[usize]> = patterns
                .iter()
                .copied()
                .filter(|position| pattern_matches(&rules[*position], &plain))
                .collect();
            write(&self.pattern_hits)
                .entry(when.to_string())
                .or_default()
                .insert(plain.into_owned(), Arc::clone(&hits));
            hits
        });
        if hits.is_empty() {
            return Cow::Borrowed(named);
        }
        let mut positions: Vec<usize> = hits.to_vec();
        positions.extend_from_slice(named);
        positions.sort_unstable();
        Cow::Owned(positions)
    }
}

fn to_index_map(values: &BTreeMap<String, Value>) -> indexmap::IndexMap<String, Value> {
    values
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// A rule from its `<seca>` element. A name pattern that does not parse is an error, so the loader skips the rule.
pub(super) fn seca_rule_from(node: &MNode) -> Result<SecaRule, Error> {
    let service = node.attribute("service").unwrap_or_default();
    let service_plain = service.replace('#', "");
    let name_is_pattern = node.attribute_is("name-is-pattern", "true");
    let pattern = if name_is_pattern {
        Some(moqui_lib::object::CompiledRegex::new(&service_plain).map_err(|err| {
            Error::new(format!(
                "service name pattern {service_plain} is not valid: {}",
                err.message()
            ))
        })?)
    } else {
        None
    };
    let condition_node = node.first_named("condition");
    let condition_check = condition_node
        .as_ref()
        .and_then(|condition| condition.children().into_iter().next())
        .map(|first| format!("<and>{}</and>", first.write_string()));
    Ok(SecaRule {
        id: node.attribute("id").unwrap_or_default(),
        service_name: service.clone(),
        service_plain,
        when_name: node.attribute("when").unwrap_or_default(),
        name_is_pattern,
        pattern,
        run_on_error: node.attribute_is("run-on-error", "true"),
        priority: node
            .attribute("priority")
            .and_then(|text| text.parse().ok())
            .unwrap_or(5),
        condition: condition_node.map(|child| child.write_string()).unwrap_or_default(),
        condition_check,
        actions: node
            .first_named("actions")
            .map(|child| child.write_string())
            .unwrap_or_default(),
    })
}

impl SecaRule {
    /// Whether the rule runs for `service`, at any `when`.
    pub(super) fn applies_to(&self, service: &str) -> bool {
        service_matches(self, service)
    }

    /// The rule as XML, for the tools screens that list rules.
    pub(super) fn describe(&self) -> String {
        let mut text = format!(
            "<seca service=\"{}\" when=\"{}\" priority=\"{}\"{}{}>",
            self.service_plain,
            self.when_name,
            self.priority,
            if self.name_is_pattern {
                " name-is-pattern=\"true\""
            } else {
                ""
            },
            if self.run_on_error {
                " run-on-error=\"true\""
            } else {
                ""
            },
        );
        text.push_str(&self.condition);
        text.push_str(&self.actions);
        text.push_str("</seca>");
        text
    }
}

fn service_matches(rule: &SecaRule, service: &str) -> bool {
    let plain = service.replace('#', "");
    if !rule.name_is_pattern {
        return rule.service_plain == plain;
    }
    pattern_matches(rule, &plain)
}

/// Whether the rule's name pattern matches `plain`, a service name without `#`.
fn pattern_matches(rule: &SecaRule, plain: &str) -> bool {
    rule.pattern
        .as_ref()
        .is_some_and(|pattern| pattern.is_full_match(plain))
}

impl SecaRule {
    /// Java's load check: a named service that is defined, or a pattern that matches a known service. `known` is
    /// filled with the known service names when a pattern first needs them, so a load reads the service files once.
    pub(super) fn service_defined(
        &self,
        facade: Option<&crate::entity::EntityFacade>,
        known: &mut Option<Vec<String>>,
    ) -> bool {
        if !self.name_is_pattern {
            return super::call::is_service_defined(&self.service_name, facade)
                || super::define::registered()
                    .unwrap_or_default()
                    .iter()
                    .any(|def| def.name.replace('#', "") == self.service_plain);
        }
        self.service_count(known) > 0
    }

    /// How many services the rule runs for, as Java's `getSecaRuleCount` counts it: one for a named service, and for a
    /// pattern each known service it matches. `known` is filled as for [`Self::service_defined`].
    pub(super) fn service_count(&self, known: &mut Option<Vec<String>>) -> usize {
        if !self.name_is_pattern {
            return 1;
        }
        known
            .get_or_insert_with(super::info::known_service_names)
            .iter()
            .filter(|name| pattern_matches(self, &name.replace('#', "")) || pattern_matches(self, name))
            .count()
    }
}

#[cfg(test)]
#[path = "seca_tests.rs"]
mod tests;
