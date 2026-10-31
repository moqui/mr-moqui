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

//! Relink the entity model after entity files changed (plans/design-dev-aide.md F5, 6.1): every entity file read again
//! with the configuration the model was loaded with, the new model swapped in (calls in flight keep the one they took),
//! the caches read from the old one dropped, and the tables of the entities the changed files define or extend checked,
//! creating what is missing. A model that does not load keeps the old one in place.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use moqui_db::model::EntityModel;
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use serde::Serialize;

use super::facade::EntityFacade;

/// What a relink did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkReport {
    /// The entities the changed files define or extend, and the views over them, by full name.
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// The DDL the table check ran.
    pub ddl: Vec<String>,
    /// What went wrong and was passed over: a table check that failed.
    pub problems: Vec<String>,
    pub millis: u64,
}

impl EntityFacade {
    /// Load the entity model again from its files and swap it in; `changed_files` are the entity files that changed,
    /// whose entities' tables are checked after.
    ///
    /// # Errors
    ///
    /// Returns the load error, with the old model still in place, when a file does not parse or the model does not
    /// link.
    pub fn relink(&self, changed_files: &[PathBuf]) -> Result<RelinkReport, Error> {
        let started = std::time::Instant::now();
        let conf = MNode::parse_text("conf", self.inner.model.conf_text())?;
        let paths = self.model_paths();
        let directories: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        let model = EntityModel::load_paths(&conf, &directories)?;
        let old = self.inner.model.model();
        // The load leaves out what does not parse or link, so a boot goes on; a relink keeps the model there was
        // instead, as a file being edited is often broken for a moment. What the old model already left out does
        // not stop it.
        let known: HashSet<&str> = old.problems().iter().map(String::as_str).collect();
        let new_problems: Vec<&str> = model
            .problems()
            .iter()
            .map(String::as_str)
            .filter(|problem| !known.contains(problem))
            .collect();
        if !new_problems.is_empty() {
            return Err(Error::new(format!(
                "the entity model was not relinked: {}",
                new_problems.join("; ")
            )));
        }
        let before: HashSet<&str> = old.entities().iter().map(|entity| entity.full_name.as_str()).collect();
        let after: HashSet<&str> = model
            .entities()
            .iter()
            .map(|entity| entity.full_name.as_str())
            .collect();
        let mut report = RelinkReport {
            added: sorted(after.difference(&before)),
            removed: sorted(before.difference(&after)),
            ..RelinkReport::default()
        };
        let mut changed: BTreeSet<String> = BTreeSet::new();
        for file in changed_files {
            for name in declared(file) {
                if let Ok(entity) = model.entity(&name) {
                    changed.insert(entity.full_name.clone());
                }
            }
        }
        // the views over a changed entity read its new fields too
        for entity in model.entities().iter().filter(|entity| entity.is_view) {
            let over_changed = entity.member_entities.iter().any(|member| {
                model
                    .entity(&member.entity_name)
                    .is_ok_and(|member| changed.contains(&member.full_name))
            });
            if over_changed {
                changed.insert(entity.full_name.clone());
            }
        }
        changed.retain(|name| before.contains(name.as_str()));
        report.changed = changed.into_iter().collect();
        let check: Vec<String> = report
            .changed
            .iter()
            .chain(&report.added)
            .filter(|name| model.entity(name).is_ok_and(|entity| !entity.is_view))
            .cloned()
            .collect();
        drop(old);
        self.inner.model.swap(model);
        self.forget_model_caches();
        let names: Vec<&str> = check.iter().map(String::as_str).collect();
        if !names.is_empty() {
            match self.check_tables_ddl(&names) {
                Ok(ddl) => report.ddl = ddl,
                Err(err) => {
                    tracing::error!(error = %err, "The table check after the entity model changed failed");
                    report.problems.push(format!("table check: {err}"));
                }
            }
        }
        report.millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            "Relinked the entity model in {} ms: {} changed, {} added, {} removed, {} DDL statements",
            report.millis,
            report.changed.len(),
            report.added.len(),
            report.removed.len(),
            report.ddl.len()
        );
        Ok(report)
    }

    /// Drop what was read from the model: definitions built at runtime and every cache kept by definition (their
    /// generation), the read shapes and their SQL, the views by member, the list and count caches, the data document
    /// definitions, what is known of the tables, and the entity ECA rules (their entity names resolve against it).
    fn forget_model_caches(&self) {
        super::derived::clear(self);
        super::sql_cache::forget_model(self);
        super::cache::forget_model(self);
        super::document::clear_document_cache(self);
        self.forget_tables();
        super::load_feed::reload_eca_rules(self);
    }
}

fn sorted(
    names: std::collections::hash_set::Difference<'_, &str, std::collections::hash_map::RandomState>,
) -> Vec<String> {
    let mut out: Vec<String> = names.map(|name| (*name).to_string()).collect();
    out.sort();
    out
}

/// The entities an entity file defines or extends, by full name when it has a package, else by name.
fn declared(file: &Path) -> Vec<String> {
    let Ok(root) = MNode::parse_file(file) else {
        return Vec::new();
    };
    root.children()
        .iter()
        .filter(|node| node.has_name("entity") || node.has_name("view-entity") || node.has_name("extend-entity"))
        .filter_map(|node| {
            let name = node.attribute("entity-name")?;
            Some(match node.attribute("package").filter(|package| !package.is_empty()) {
                Some(package) => format!("{package}.{name}"),
                None => name,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_names_come_from_definitions_and_extensions() {
        let dir = std::env::temp_dir().join(format!("moqui-relink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("SiteEntities.xml");
        std::fs::write(
            &file,
            "<entities><entity entity-name=\"A\" package=\"p\"/><extend-entity entity-name=\"Example\"/>\
             <view-entity entity-name=\"V\" package=\"q\"/><!-- c --></entities>",
        )
        .unwrap();
        assert_eq!(declared(&file), ["p.A", "Example", "q.V"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
