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

//! Service and email ECA rules, loaded once per factory from every component's `service/` directory
//! (`*.secas.xml` and `*.emecas.xml`), as `ServiceFacadeImpl` does.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use moqui_lib::node::MNode;

use super::email_eca::{email_rule_from, EmailRule};
use super::seca::{seca_rule_from, SecaIndex, SecaRule};
use crate::context::{ExecutionContext, ExecutionContextFactory};

#[derive(Default)]
pub(super) struct EcaRules {
    /// Rules in priority order, a later rule with the same id having replaced an earlier one.
    pub(super) seca: Vec<Arc<SecaRule>>,
    /// `seca` by `when` and service.
    pub(super) seca_index: SecaIndex,
    pub(super) email: Vec<Arc<EmailRule>>,
}

impl EcaRules {
    fn new(seca: Vec<Arc<SecaRule>>, email: Vec<Arc<EmailRule>>) -> Self {
        Self {
            seca_index: SecaIndex::new(&seca),
            seca,
            email,
        }
    }
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

/// Replace a factory's rules, for tests that bring their own XML.
#[cfg(test)]
pub(super) fn install(factory: &Arc<ExecutionContextFactory>, rules: EcaRules) {
    let shared = factory.extension::<Shared>(|_| RwLock::new(Arc::new(EcaRules::default())));
    *shared.write().unwrap_or_else(|err| err.into_inner()) = Arc::new(rules);
}

fn load(factory: &ExecutionContextFactory) -> EcaRules {
    // Rules with an id go in a java.util.HashMap in Java: a later rule with the same id replaces the
    // earlier one but keeps its place, and the rules come out in that map's order.
    let mut seca_by_id: HashMap<String, (u64, Arc<SecaRule>)> = HashMap::new();
    let mut seca: Vec<Arc<SecaRule>> = Vec::new();
    let mut email = Vec::new();
    let (mut seca_files, mut email_files, mut no_id) = (0_usize, 0_usize, 0_usize);
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
            if !is_seca && !name.ends_with(".emecas.xml") {
                continue;
            }
            let root = match MNode::parse_file(&path) {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!(file = %path.display(), error = %err, "could not read ECA rules");
                    continue;
                }
            };
            if is_seca {
                seca_files += 1;
                for node in root.children_named("seca") {
                    let rule = Arc::new(seca_rule_from(&node));
                    match node.attribute("id").filter(|id| !id.is_empty()) {
                        Some(id) => {
                            let order = seca_by_id.len() as u64;
                            seca_by_id
                                .entry(id)
                                .and_modify(|entry| entry.1 = rule.clone())
                                .or_insert((order, rule));
                        }
                        None => {
                            no_id += 1;
                            seca.push(rule);
                        }
                    }
                }
            } else {
                email_files += 1;
                for node in root.children_named("emeca") {
                    email.push(Arc::new(email_rule_from(&node)));
                }
            }
        }
    }
    seca.extend(seca_rules_in_java_order(seca_by_id));
    seca.sort_by_key(|rule| rule.priority);
    tracing::info!(
        "Loaded {} Service ECA rules from {seca_files} .secas.xml files, {no_id} rules have no id, {} SECA rules active in {}ms",
        seca.len(),
        seca.len(),
        started.elapsed().as_millis()
    );
    tracing::info!(
        "Loaded [{}] Email ECA rules from {email_files} .emecas.xml files",
        email.len()
    );
    EcaRules::new(seca, email)
}

/// The rules with an id in the order Java's `ruleByIdMap.values()` gives them.
fn seca_rules_in_java_order(mut by_id: HashMap<String, (u64, Arc<SecaRule>)>) -> Vec<Arc<SecaRule>> {
    let order = crate::context::hits::java_hash_map_order(by_id.iter().map(|(id, (order, _))| (id.as_str(), *order)));
    order
        .iter()
        .filter_map(|id| by_id.remove(id).map(|(_, rule)| rule))
        .collect()
}

/// Parse rules from XML text, for tests.
#[cfg(test)]
pub(super) fn from_xml(seca_xml: &str, email_xml: &str) -> EcaRules {
    let mut seca: Vec<Arc<SecaRule>> = Vec::new();
    let mut email = Vec::new();
    if !seca_xml.is_empty() {
        let root = MNode::parse_text("secas", seca_xml).expect("secas xml");
        seca = root
            .children_named("seca")
            .iter()
            .map(|node| Arc::new(seca_rule_from(node)))
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
    EcaRules::new(seca, email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_with_an_id_come_in_java_hash_map_order() {
        let rule = |id: &str, service: &str| {
            let xml = format!(r#"<secas><seca id="{id}" service="{id}.{service}" when="post-service"/></secas>"#);
            let root = MNode::parse_text("secas", &xml).expect("secas xml");
            Arc::new(seca_rule_from(&root.children_named("seca")[0]))
        };
        let mut by_id = HashMap::new();
        // As loaded from AccountingPayment, OrderReturn and ProductAsset .secas.xml, then 57 more, so 128 bins.
        let mut ids: Vec<String> = [
            "OrderPlacedAuthorizePayments",
            "OrderPlacedAutoApprove",
            "OrderPlacedReserveAssets",
        ]
        .iter()
        .map(|id| id.to_string())
        .collect();
        ids.extend((0..57).map(|index| format!("Other{index:02}")));
        for (order, id) in ids.iter().enumerate() {
            by_id.insert(id.clone(), (order as u64, rule(id, "first")));
        }
        // A later rule with the same id replaces the first and keeps its place.
        by_id.get_mut("OrderPlacedAutoApprove").expect("rule").1 = rule("OrderPlacedAutoApprove", "second");
        let placed: Vec<String> = seca_rules_in_java_order(by_id)
            .iter()
            .map(|rule| rule.describe())
            .filter(|text| text.contains("OrderPlaced"))
            .collect();
        let expected = [
            ("OrderPlacedAutoApprove", "second"),
            ("OrderPlacedAuthorizePayments", "first"),
            ("OrderPlacedReserveAssets", "first"),
        ]
        .map(|(id, which)| rule(id, which).describe());
        assert_eq!(placed, expected);
    }
}
