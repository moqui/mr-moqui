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

//! Service, email and git ECA rules, loaded once per factory from every component's `service/` directory
//! (`*.secas.xml`, `*.emecas.xml` and `*.gitecas.xml`), as `ServiceFacadeImpl` does for the first two.

use indexmap::IndexMap;
use std::sync::{Arc, RwLock};

use moqui_lib::node::MNode;

use super::email_eca::{email_rule_from, EmailRule};
use super::git_eca::{git_rule_from, GitRule};
use super::seca::{seca_rule_from, SecaIndex, SecaRule};
use crate::context::{ExecutionContext, ExecutionContextFactory};

#[derive(Default)]
pub(super) struct EcaRules {
    /// Rules in priority order, a later rule with the same id having replaced an earlier one.
    pub(super) seca: Vec<Arc<SecaRule>>,
    /// `seca` by `when` and service.
    pub(super) seca_index: SecaIndex,
    pub(super) email: Vec<Arc<EmailRule>>,
    /// gitECA rules in priority order, a later rule with the same id having replaced an earlier one. Read only by the
    /// git events (moqui-git).
    pub(super) git: Vec<Arc<GitRule>>,
}

impl EcaRules {
    fn new(seca: Vec<Arc<SecaRule>>, email: Vec<Arc<EmailRule>>, git: Vec<Arc<GitRule>>) -> Self {
        Self {
            seca_index: SecaIndex::new(&seca),
            seca,
            email,
            git,
        }
    }

    /// The same rules with gitECA rules from XML text, for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn with_git(mut self, git_xml: &str) -> Self {
        let root = MNode::parse_text("gitecas", git_xml).expect("gitecas xml");
        self.git = git_rules(root.children_named("giteca").iter(), &mut 0);
        self
    }
}

/// gitECA rules from their elements: one that cannot run is logged and skipped; a rule with an id replaces an earlier
/// one with the same id in place; then by priority.
fn git_rules<'a>(nodes: impl Iterator<Item = &'a MNode>, skipped: &mut usize) -> Vec<Arc<GitRule>> {
    let mut rules: Vec<Arc<GitRule>> = Vec::new();
    for node in nodes {
        match git_rule_from(node) {
            Ok(rule) => {
                let rule = Arc::new(rule);
                match rule
                    .id
                    .clone()
                    .and_then(|id| rules.iter().position(|old| old.id.as_deref() == Some(&id)))
                {
                    Some(at) => rules[at] = rule,
                    None => rules.push(rule),
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "Skipping gitECA rule");
                *skipped += 1;
            }
        }
    }
    rules.sort_by_key(|rule| rule.priority);
    rules
}

type Shared = RwLock<Arc<EcaRules>>;

/// The rules of the entered context's factory. `None` when no context is entered.
pub(super) fn current() -> Option<Arc<EcaRules>> {
    ExecutionContext::with_current(|ctx| rules_of(ctx.factory()))
}

fn rules_of(factory: &Arc<ExecutionContextFactory>) -> Arc<EcaRules> {
    let shared = factory.extension::<Shared>(|factory| RwLock::new(Arc::new(load(factory))));
    let guard = shared.read().unwrap_or_else(|err| err.into_inner());
    Arc::clone(&guard)
}

/// Read a factory's SECA, EmECA and gitECA rules again from their files and swap them in (plans/design-dev-aide.md
/// 6.1, F4); a call in flight keeps the rules it has. A file that does not parse is logged and skipped, as at a boot.
/// Answers how many SECA, EmECA and gitECA rules there are now.
pub fn reload(factory: &Arc<ExecutionContextFactory>) -> (usize, usize, usize) {
    let rules = load(factory);
    let counts = (rules.seca.len(), rules.email.len(), rules.git.len());
    install(factory, rules);
    counts
}

/// Replace a factory's rules: tests that bring their own XML, and [`reload`].
pub(super) fn install(factory: &Arc<ExecutionContextFactory>, rules: EcaRules) {
    let shared = factory.extension::<Shared>(|_| RwLock::new(Arc::new(EcaRules::default())));
    *shared.write().unwrap_or_else(|err| err.into_inner()) = Arc::new(rules);
}

fn load(factory: &ExecutionContextFactory) -> EcaRules {
    // each rule as loaded, with its id when it has one
    let mut loaded: Vec<(Option<String>, Arc<SecaRule>)> = Vec::new();
    let mut email = Vec::new();
    let mut git_nodes: Vec<MNode> = Vec::new();
    let (mut seca_files, mut email_files, mut git_files, mut no_id, mut skipped) =
        (0_usize, 0_usize, 0_usize, 0_usize, 0_usize);
    let facade = factory.entity();
    let mut known_services = None;
    let started = std::time::Instant::now();
    for component in factory.components() {
        let dir = component.location.join("service");
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<_> = read.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect();
        paths.sort();
        for path in paths {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            let is_seca = name.ends_with(".secas.xml");
            let is_git = name.ends_with(".gitecas.xml");
            if !is_seca && !is_git && !name.ends_with(".emecas.xml") {
                continue;
            }
            let root = match MNode::parse_file(&path) {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!(file = %path.display(), error = %err, "could not read ECA rules");
                    continue;
                }
            };
            if is_git {
                git_files += 1;
                git_nodes.extend(root.children_named("giteca"));
            } else if is_seca {
                seca_files += 1;
                for node in root.named_iter("seca") {
                    // As Java: a rule that cannot run is logged and skipped, and the others still load.
                    let rule = match seca_rule_from(&node) {
                        Ok(rule) => rule,
                        Err(err) => {
                            tracing::warn!(file = %path.display(), error = %err, "Skipping SECA rule");
                            continue;
                        }
                    };
                    if !rule.service_defined(facade.as_ref(), &mut known_services) {
                        tracing::warn!(
                            "Invalid service name {} found in SECA file {}, skipping",
                            node.attribute("service").unwrap_or_default(),
                            path.display()
                        );
                        skipped += 1;
                        continue;
                    }
                    let id = node.attribute("id").filter(|id| !id.is_empty());
                    if id.is_none() {
                        no_id += 1;
                    }
                    loaded.push((id, Arc::new(rule)));
                }
            } else {
                email_files += 1;
                for node in root.named_iter("emeca") {
                    email.push(Arc::new(email_rule_from(&node)));
                }
            }
        }
    }
    let seca = seca_in_load_order(loaded);
    tracing::info!(
        "Loaded {} Service ECA rules from {seca_files} .secas.xml files, {no_id} rules have no id, {} SECA rules active, {skipped} skipped for an undefined service, in {}ms",
        seca.len(),
        seca.len(),
        started.elapsed().as_millis()
    );
    tracing::info!(
        "Loaded [{}] Email ECA rules from {email_files} .emecas.xml files",
        email.len()
    );
    let mut git_skipped = 0;
    let git = git_rules(git_nodes.iter(), &mut git_skipped);
    tracing::info!(
        "Loaded [{}] git ECA rules from {git_files} .gitecas.xml files, {git_skipped} skipped",
        git.len()
    );
    EcaRules::new(seca, email, git)
}

/// The SECA rules to run, by priority and, within a priority, in the order they were loaded (components in order,
/// files by name), so they run in the same order on every start: the rules with no id, then those with an id, where
/// a later rule with an id an earlier one has replaces it in its place.
fn seca_in_load_order(loaded: Vec<(Option<String>, Arc<SecaRule>)>) -> Vec<Arc<SecaRule>> {
    let mut seca: Vec<Arc<SecaRule>> = Vec::new();
    let mut by_id: IndexMap<String, Arc<SecaRule>> = IndexMap::new();
    for (id, rule) in loaded {
        match id {
            // IndexMap::insert keeps an existing key's place
            Some(id) => {
                by_id.insert(id, rule);
            }
            None => seca.push(rule),
        }
    }
    seca.extend(by_id.into_values());
    // a stable sort, so the load order holds within a priority
    seca.sort_by_key(|rule| rule.priority);
    seca
}

/// Parse rules from XML text, for tests.
#[cfg(any(test, feature = "test-support"))]
pub(super) fn from_xml(seca_xml: &str, email_xml: &str) -> EcaRules {
    let mut seca: Vec<Arc<SecaRule>> = Vec::new();
    let mut email = Vec::new();
    if !seca_xml.is_empty() {
        let root = MNode::parse_text("secas", seca_xml).expect("secas xml");
        seca = root
            .children_named("seca")
            .iter()
            .map(|node| Arc::new(seca_rule_from(node).expect("a valid SECA rule")))
            .collect();
        seca.sort_by_key(|rule| rule.priority);
    }
    if !email_xml.is_empty() {
        let root = MNode::parse_text("emecas", email_xml).expect("emecas xml");
        email = root
            .children_named("emeca")
            .iter()
            .map(|node| Arc::new(email_rule_from(node)))
            .collect();
    }
    EcaRules::new(seca, email, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same-priority rules run in load order on every start; a later rule with the same id replaces the earlier one
    /// in its place, and a lower priority still runs first.
    #[test]
    fn same_priority_rules_keep_their_load_order() {
        let rule = |id: &str, service: &str, priority: &str| {
            let xml = format!(
                r#"<secas><seca id="{id}" service="{service}" when="post-service" priority="{priority}"/></secas>"#
            );
            let root = MNode::parse_text("secas", &xml).expect("secas xml");
            Arc::new(seca_rule_from(&root.children_named("seca")[0]).expect("a valid SECA rule"))
        };
        let ids = [
            "OrderPlacedAuthorizePayments",
            "OrderPlacedAutoApprove",
            "OrderPlacedReserveAssets",
            "Zeta",
            "Alpha",
        ];
        let mut loaded: Vec<(Option<String>, Arc<SecaRule>)> = ids
            .iter()
            .map(|id| (Some(id.to_string()), rule(id, &format!("first.{id}"), "5")))
            .collect();
        loaded.insert(1, (None, rule("", "noid.Rule", "5")));
        loaded.push((
            Some("OrderPlacedAutoApprove".to_string()),
            rule("OrderPlacedAutoApprove", "second.OrderPlacedAutoApprove", "5"),
        ));
        loaded.push((Some("Early".to_string()), rule("Early", "first.Early", "1")));
        let order: Vec<String> = seca_in_load_order(loaded).iter().map(|rule| rule.describe()).collect();
        let expected: Vec<String> = [
            ("Early", "first.Early", "1"),
            ("", "noid.Rule", "5"),
            (
                "OrderPlacedAuthorizePayments",
                "first.OrderPlacedAuthorizePayments",
                "5",
            ),
            ("OrderPlacedAutoApprove", "second.OrderPlacedAutoApprove", "5"),
            ("OrderPlacedReserveAssets", "first.OrderPlacedReserveAssets", "5"),
            ("Zeta", "first.Zeta", "5"),
            ("Alpha", "first.Alpha", "5"),
        ]
        .iter()
        .map(|(id, service, priority)| rule(id, service, priority).describe())
        .collect();
        assert_eq!(order, expected);
    }
}
