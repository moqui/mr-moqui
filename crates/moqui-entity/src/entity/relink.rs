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

//! Relink the entity model after entity files changed or a component was added (plans/design-dev-aide.md F5, 6.1;
//! plans/archive/Component-Deploy.md): every entity file read again with the configuration the model was loaded with, the
//! tables of the entities the changed files define or extend checked against the new model, creating what is missing,
//! then the new model swapped in (calls in flight keep the one they took) and the caches read from the old one dropped.
//! The check runs before the swap, so no other thread reads a column before it exists. A model that does not load, or
//! a check with a failed step, keeps the old model and the old component directories in place.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// What a relink would change, from [`EntityFacade::preview_relink`].
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkPreview {
    /// The entities and views the new model has that the current one does not, by full name.
    pub added: Vec<String>,
    /// The entities whose fields change (an `extend-entity` adds some).
    pub changed: Vec<String>,
    /// The changes the table check cannot make: a primary key field added or removed, a field's type changed.
    pub incompatible: Vec<String>,
}

impl EntityFacade {
    /// What relinking with the components' `entity` directories `component_dirs` would change, changing nothing: the
    /// entities added and changed, and the changes the additive table check cannot make, which a component loaded on
    /// a running server must not bring (plans/archive/Component-Deploy.md).
    ///
    /// # Errors
    ///
    /// When a file does not parse or the model does not link, with problems the current model does not have.
    pub fn preview_relink(&self, component_dirs: &[PathBuf]) -> Result<RelinkPreview, Error> {
        let model = self.load_relinked(Some(component_dirs))?;
        let old = self.inner.model.model();
        let mut preview = RelinkPreview::default();
        for entity in model.entities().iter() {
            let Ok(before) = old.entity(&entity.full_name) else {
                preview.added.push(entity.full_name.clone());
                continue;
            };
            if entity.is_view {
                continue;
            }
            let mut changed = before.fields.len() != entity.fields.len();
            for field in &entity.fields {
                match before.fields.iter().find(|have| have.name == field.name) {
                    Some(have) if have.is_pk != field.is_pk => preview
                        .incompatible
                        .push(format!("{}.{}: the primary key changes", entity.full_name, field.name)),
                    Some(have) if have.field_type != field.field_type => preview.incompatible.push(format!(
                        "{}.{}: its type changes from {} to {}",
                        entity.full_name, field.name, have.field_type, field.field_type
                    )),
                    Some(_) => {}
                    None if field.is_pk => preview.incompatible.push(format!(
                        "{}.{}: a primary key field is added",
                        entity.full_name, field.name
                    )),
                    None => changed = true,
                }
            }
            if changed {
                preview.changed.push(entity.full_name.clone());
            }
        }
        preview.added.sort();
        preview.changed.sort();
        Ok(preview)
    }

    /// The model the files make, with `component_dirs` in place of the components' `entity` directories when given;
    /// an error with the problems the current model does not have.
    fn load_relinked(&self, component_dirs: Option<&[PathBuf]>) -> Result<EntityModel, Error> {
        let conf = MNode::parse_text("conf", &self.inner.model.conf_text())?;
        let paths = match component_dirs {
            Some(dirs) => {
                let mut paths = vec![self.inner.framework_dir.join("entity")];
                paths.extend(self.inner.entity_files.iter().cloned());
                paths.extend(dirs.iter().cloned());
                paths
            }
            None => self.model_paths(),
        };
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
        Ok(model)
    }

    /// Load the entity model again from its files and swap it in; `changed_files` are the entity files that changed,
    /// whose entities' tables are checked before the swap.
    ///
    /// # Errors
    ///
    /// Returns an error, with the old model still in place, when a file does not parse, the model does not link, or a
    /// step of the table check failed.
    pub fn relink(&self, changed_files: &[PathBuf]) -> Result<RelinkReport, Error> {
        self.relink_components(None, changed_files)
    }

    /// [`Self::relink`] reading the components' `entity` directories `component_dirs`, when given, in place of the
    /// current ones. They are set only once the new model is in place, so a model that does not load leaves the
    /// directories as they were: a component added to a running server (plans/archive/Component-Deploy.md), or a site's first
    /// entity file.
    ///
    /// # Errors
    ///
    /// As [`Self::relink`].
    pub fn relink_components(
        &self,
        component_dirs: Option<Vec<PathBuf>>,
        changed_files: &[PathBuf],
    ) -> Result<RelinkReport, Error> {
        let started = std::time::Instant::now();
        let model = self.load_relinked(component_dirs.as_deref())?;
        let old = self.inner.model.model();
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
        let model = Arc::new(model);
        let names: Vec<&str> = check.iter().map(String::as_str).collect();
        if !names.is_empty() {
            report.ddl = self.check_staged(&model, &names)?;
        }
        self.inner.model.swap(model);
        if let Some(dirs) = component_dirs {
            self.set_component_entity_dirs(dirs);
        }
        self.forget_model_caches();
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

    /// Check the tables of `names` against `model` before it is swapped in, once more after a wait when a step failed
    /// (a lock held by a transaction that has since ended), and answer the DDL that ran.
    fn check_staged(&self, model: &Arc<EntityModel>, names: &[&str]) -> Result<Vec<String>, Error> {
        let mut ddl = Vec::new();
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            let check = self.inner.model.with_staged(model, || self.check_tables_ddl(names));
            let check = check.map_err(|err| {
                Error::new(format!(
                    "the entity model was not relinked, the table check failed: {err}"
                ))
            })?;
            ddl.extend(check.ddl);
            if check.errors == 0 {
                return Ok(ddl);
            }
            tracing::warn!(
                "The table check for the new entity model had {} failed steps (attempt {})",
                check.errors,
                attempt + 1
            );
        }
        Err(Error::new(
            "the entity model was not relinked: steps of its table check failed, see the log; the tables of the \
             entities it changes keep what was added",
        ))
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
