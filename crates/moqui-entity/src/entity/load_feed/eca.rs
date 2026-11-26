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
        .map(|rule| rule.id.clone())
        .collect();
    note_touch(facade, entity_name, operation, None, None);
    ids
}

/// The ECA rules for a write, which the write hands to `service::run_matched_eca`: nothing is kept on the
/// facade, so a nested or concurrent write cannot change the rules of this one. A load's switch on this thread
/// ([`crate::entity::WriteSwitches`]) leaves the security checks ([`is_security_check`]); the facade's own flag and the
/// execution context's `disableEntityEca` leave none.
pub(crate) fn match_rules_with(facade: &EntityFacade, entity_name: &str, operation: &str) -> Vec<Arc<EcaRule>> {
    if facade.shared_flags().disable_entity_eca || (crate::hooks::get().entity_eca_disabled)() {
        return Vec::new();
    }
    let checks_only = crate::entity::WriteSwitches::current().disable_entity_eca;
    eca_index(facade)
        .of_entity(entity_name)
        .filter(|rule| rule_matches(rule, entity_name, operation))
        .filter(|rule| !checks_only || is_security_check(&rule.actions))
        .cloned()
        .collect()
}

/// Whether a rule's actions call a `UserSecurityChecks` assert (`Security.eecas.xml` and the components': privileged
/// membership, sealed grants, stored scripts, templates, expressions, document conditions, translations, and any added
/// later): checks, not business rules, so they run even when the message facade already has errors, and a load's switch
/// does not turn them off.
#[must_use]
pub fn is_security_check(actions: &str) -> bool {
    actions.contains("UserSecurityChecks.assert")
}

/// Rules stored for `entity_name`. This lookup does not run them; `service::run_matched_eca` does.
pub fn eca_rules(facade: &EntityFacade, entity_name: &str) -> Vec<Arc<EcaRule>> {
    eca_index(facade).of_entity(entity_name).cloned().collect()
}

/// How many entity ECA rules are loaded, for the tools artifact stats.
pub fn eca_rule_count(facade: &EntityFacade) -> usize {
    eca_index(facade).rules.len()
}

/// Add the `<eeca>` rules of an `<eecas>` document after the stored ones, for a test that needs its own rules.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn add_eca_rules(facade: &EntityFacade, eecas: &str) -> Result<(), moqui_lib::error::Error> {
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
    /// Shared, so a write that matches a rule holds it without copying its texts (plans/Profiling3.md W9).
    rules: Vec<Arc<EcaRule>>,
    by_entity: HashMap<String, Vec<usize>>,
}

impl EcaIndex {
    /// The rules of `entity_name`: those with no id, then those with one, each in file order.
    fn of_entity<'a>(&'a self, entity_name: &str) -> impl Iterator<Item = &'a Arc<EcaRule>> + 'a {
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
            if moqui_lib::vfs::is_file(dir) {
                if dir.to_string_lossy().ends_with(".eecas.xml") {
                    files += 1;
                    match MNode::parse_file(dir) {
                        Ok(root) => index.ingest(facade, &root, &dir.display().to_string()),
                        Err(err) => tracing::warn!(error = %err, path = %dir.display(), "could not parse eeca file"),
                    }
                }
                continue;
            }
            let read = match moqui_lib::vfs::read_dir(dir) {
                Ok(read) => read,
                Err(err) => {
                    tracing::warn!(error = %err, dir = %dir.display(), "could not read eeca directory");
                    continue;
                }
            };
            let mut paths: Vec<PathBuf> = read
                .iter()
                .map(moqui_lib::vfs::DirEntry::path)
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
        for node in root.named_iter("eeca") {
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
                condition: first_condition(&node),
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
                self.rules[pos] = Arc::new(rule);
            } else {
                self.rules.push(Arc::new(rule));
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

/// Read the facade's EECA rules again from the entity directories and swap them in (plans/design-dev-aide.md 6.1, F4);
/// a write in flight keeps the rules it has. Answers how many rules there are now.
pub fn reload_eca_rules(facade: &EntityFacade) -> usize {
    reload_eca_rules_with(facade, &[])
}

/// [`reload_eca_rules`] reading `component_dirs` too, those the model is not read from yet: a site's first rules file
/// makes its component's `entity` directory, which the model reads only at its next relink (a component's first
/// entity file).
pub fn reload_eca_rules_with(facade: &EntityFacade, component_dirs: &[PathBuf]) -> usize {
    let mut paths = facade.model_paths();
    for dir in component_dirs {
        if !paths.contains(dir) {
            paths.push(dir.clone());
        }
    }
    let index = Arc::new(EcaIndex::load(facade, &paths));
    let count = index.rules.len();
    *write(&facade.inner.eca_rules) = Some(index);
    count
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

/// The `<condition>` of a rule element as Java's `EntityEcaRule` and `EmailEcaRule` read it: only its first child, "the
/// script is effectively the first child of the condition element". Empty when there is none, so the rule always runs.
/// It is kept as the condition group `<and>` holding that child, as a SECA keeps its check, so a check runs the
/// generated script without parsing the XML again.
pub fn first_condition(rule: &moqui_lib::node::MNode) -> String {
    rule.first_named("condition")
        .and_then(|condition| condition.first_child())
        .map(|first| format!("<and>{}</and>", first.write_string()))
        .unwrap_or_default()
}
