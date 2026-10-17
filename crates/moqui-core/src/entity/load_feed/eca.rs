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

//! Entity ECA rules: read from the `*.eecas.xml` files of the entity directories, indexed by entity, and matched
//! for a write. `service::run_matched_eca` runs them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use moqui_lib::node::MNode;
use moqui_lib::sync::{read, write};

use super::feed::note_touch;
use super::loader::canonical_name;
use crate::context::ExecutionContext;
use crate::entity::facade::EntityFacade;

/// One ECA rule. `actions` is the stored action body. It is not executed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcaRule {
    pub id: String,
    pub entity_name: String,
    pub on_create: bool,
    pub on_update: bool,
    pub on_delete: bool,
    pub run_before: bool,
    /// Fill fields the write does not set from the database row, for update and delete.
    pub get_entire_entity: bool,
    /// Put the database row before the write in `originalValue`, for update and delete.
    pub get_original_value: bool,
    /// Copy context names the actions set that are non-primary-key fields onto the value.
    pub set_results: bool,
    /// Run even when the message facade has errors.
    pub run_on_error: bool,
    pub condition: String,
    pub actions: String,
}

/// Match ECA rules for a write, and note the write for the data feed. Returns the ids of the matched rules.
///
/// `operation` is `create`, `update`, or `delete`. Do not call this from inside `with_slot`.
pub fn match_rules(facade: &EntityFacade, entity_name: &str, operation: &str) -> Vec<String> {
    let ids = match_rules_with(facade, entity_name, operation)
        .into_iter()
        .map(|rule| rule.id)
        .collect();
    note_touch(facade, entity_name, operation, None, None);
    ids
}

/// The ECA rules for a write, which the write hands to `service::run_matched_eca`: nothing is kept on the
/// facade, so a nested or concurrent write cannot change the rules of this one.
pub(crate) fn match_rules_with(facade: &EntityFacade, entity_name: &str, operation: &str) -> Vec<EcaRule> {
    if facade.flags().disable_entity_eca || ExecutionContext::with_current(|ec| ec.entity_eca_disabled()) == Some(true)
    {
        return Vec::new();
    }
    eca_index(facade)
        .of_entity(entity_name)
        .filter(|rule| rule_matches(rule, entity_name, operation))
        .cloned()
        .collect()
}

/// Rules stored for `entity_name`. This lookup does not run them; `service::run_matched_eca` does.
pub fn eca_rules(facade: &EntityFacade, entity_name: &str) -> Vec<EcaRule> {
    eca_index(facade).of_entity(entity_name).cloned().collect()
}

/// How many entity ECA rules are loaded, for the tools artifact stats.
pub fn eca_rule_count(facade: &EntityFacade) -> usize {
    eca_index(facade).rules.len()
}

/// Add the `<eeca>` rules of an `<eecas>` document after the stored ones, for a test that needs its own rules.
#[cfg(test)]
pub(crate) fn add_eca_rules(facade: &EntityFacade, eecas: &str) -> Result<(), moqui_lib::error::Error> {
    let root = MNode::parse_text("eecas", eecas)?;
    let mut index = (*eca_index(facade)).clone();
    index.ingest(facade, &root, "eecas");
    *write(&facade.inner.eca_rules) = Some(Arc::new(index));
    Ok(())
}

fn rule_matches(rule: &EcaRule, entity_name: &str, operation: &str) -> bool {
    if rule.entity_name != entity_name {
        return false;
    }
    match operation {
        "create" => rule.on_create,
        "update" => rule.on_update,
        "delete" => rule.on_delete,
        _ => false,
    }
}

/// Entity ECA rules in file order, with the positions of each entity's rules, so matching a write is a map
/// lookup. Built once per facade; a reader holds the `Arc` and takes no lock while matching.
#[derive(Clone, Default)]
pub(crate) struct EcaIndex {
    rules: Vec<EcaRule>,
    by_entity: HashMap<String, Vec<usize>>,
}

impl EcaIndex {
    /// The rules of `entity_name`: those with no id, then those with one, each in file order.
    fn of_entity<'a>(&'a self, entity_name: &str) -> impl Iterator<Item = &'a EcaRule> + 'a {
        self.by_entity
            .get(entity_name)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|at| &self.rules[*at])
    }

    /// Read every `*.eecas.xml` of `dirs`: the framework's rules, then each component's, as Java loads them from
    /// every entity directory.
    fn load(facade: &EntityFacade, dirs: &[PathBuf]) -> Self {
        let mut index = Self::default();
        let mut files = 0;
        for dir in dirs {
            // A conf `load-entity` file outside the directories.
            if dir.is_file() {
                if dir.to_string_lossy().ends_with(".eecas.xml") {
                    files += 1;
                    match MNode::parse_file(dir) {
                        Ok(root) => index.ingest(facade, &root, &dir.display().to_string()),
                        Err(err) => tracing::warn!(error = %err, path = %dir.display(), "could not parse eeca file"),
                    }
                }
                continue;
            }
            let read = match std::fs::read_dir(dir) {
                Ok(read) => read,
                Err(err) => {
                    tracing::warn!(error = %err, dir = %dir.display(), "could not read eeca directory");
                    continue;
                }
            };
            let mut paths: Vec<PathBuf> = read
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(".eecas.xml"))
                })
                .collect();
            paths.sort();
            files += paths.len();
            for path in paths {
                match MNode::parse_file(&path) {
                    Ok(root) => index.ingest(facade, &root, &path.display().to_string()),
                    Err(err) => tracing::warn!(error = %err, path = %path.display(), "could not parse eeca file"),
                }
            }
        }
        tracing::info!(
            "Loaded {} Entity ECA rules from {files} .eecas.xml files, {} rules have no id",
            index.rules.len(),
            index.rules.iter().filter(|rule| rule.id.is_empty()).count()
        );
        index
    }

    /// Add the `<eeca>` rules of `root`, read from `location`. A rule with the id of a stored one replaces it in place.
    /// As Java, a rule's entity is kept by its full name, and a rule for an entity that is not defined is skipped.
    fn ingest(&mut self, facade: &EntityFacade, root: &MNode, location: &str) {
        for node in root.children_named("eeca") {
            let entity = node.attribute("entity").unwrap_or_default();
            let Ok(entity_name) = canonical_name(facade, &entity) else {
                tracing::warn!("Invalid entity name {entity} found in EECA file {location}, skipping");
                continue;
            };
            let rule = EcaRule {
                id: node.attribute("id").unwrap_or_default(),
                entity_name,
                on_create: attr_true(&node, "on-create"),
                on_update: attr_true(&node, "on-update"),
                on_delete: attr_true(&node, "on-delete"),
                run_before: attr_true(&node, "run-before"),
                get_entire_entity: attr_true(&node, "get-entire-entity"),
                get_original_value: attr_true(&node, "get-original-value"),
                set_results: attr_true(&node, "set-results"),
                run_on_error: attr_true(&node, "run-on-error"),
                condition: crate::service::rule_actions::first_condition(&node),
                actions: node
                    .first_named("actions")
                    .map(|child| child.write_string())
                    .unwrap_or_default(),
            };
            if let Some(pos) = self
                .rules
                .iter()
                .position(|existing| existing.id == rule.id && !rule.id.is_empty())
            {
                self.rules[pos] = rule;
            } else {
                self.rules.push(rule);
            }
        }
        // As Java, an entity's rules with no id come first, then those with an id; each in file order (Java's id'd
        // rules are in `HashMap` order, which is not defined).
        self.by_entity.clear();
        let (no_id, with_id): (Vec<usize>, Vec<usize>) =
            (0..self.rules.len()).partition(|at| self.rules[*at].id.is_empty());
        for at in no_id.into_iter().chain(with_id) {
            self.by_entity
                .entry(self.rules[at].entity_name.clone())
                .or_default()
                .push(at);
        }
    }
}

/// The facade's ECA rules, read from the entity directories on first use.
fn eca_index(facade: &EntityFacade) -> Arc<EcaIndex> {
    let rules = &facade.inner.eca_rules;
    if let Some(index) = read(rules).as_ref() {
        return Arc::clone(index);
    }
    let mut slot = write(rules);
    Arc::clone(slot.get_or_insert_with(|| Arc::new(EcaIndex::load(facade, &facade.model_paths()))))
}

fn attr_true(node: &MNode, name: &str) -> bool {
    node.attribute(name).is_some_and(|value| value == "true")
}
