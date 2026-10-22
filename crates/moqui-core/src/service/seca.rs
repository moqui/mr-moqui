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
        let cached = self
            .pattern_hits
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(when)
            .and_then(|by_service| by_service.get(plain.as_ref()))
            .cloned();
        let hits = cached.unwrap_or_else(|| {
            let hits: Arc<[usize]> = patterns
                .iter()
                .copied()
                .filter(|position| pattern_matches(&rules[*position], &plain))
                .collect();
            self.pattern_hits
                .write()
                .unwrap_or_else(|err| err.into_inner())
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
    let name_is_pattern = node.attribute("name-is-pattern").as_deref() == Some("true");
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
        run_on_error: node.attribute("run-on-error").as_deref() == Some("true"),
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
mod tests {
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
            let node = moqui_lib::node::MNode::parse_text(
                "seca",
                &format!(r#"<seca service="{service}" when="post-service"/>"#),
            )
            .unwrap();
            super::seca_rule_from(&node).unwrap()
        };
        assert!(rule("update#moqui.basic.Enumeration").service_defined(Some(&facade), &mut None));
        assert!(!rule("update#moqui.basic.NoSuchEntity").service_defined(Some(&facade), &mut None));
        assert!(!rule("no.such.Services.do#Nothing").service_defined(Some(&facade), &mut None));
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
}
