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

//! The roots of the Workspace (plans/design-dev-aide.md 4.1): the git repositories that hold the loaded components (found
//! by looking up from each component directory for `.git`), each loaded component in no repository, `runtime/conf` when
//! no repository holds it, and the roots the conf and Settings add. A root whose components the site does not own is
//! upstream: shown, searched and read, written only by the full tier with "Allow editing upstream components" on (D17).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use serde::Serialize;

use crate::settings::Settings;

pub const ROOT_ENTITY: &str = "moqui.dev.DevAideRoot";

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RootKind {
    Git,
    Plain,
}

/// A component in a root, at a directory relative to the root.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootComponent {
    pub name: String,
    pub path: String,
    pub site: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    /// Stable within a server: the directory's name, made unique.
    pub id: String,
    pub label: String,
    #[serde(serialize_with = "path_string")]
    pub path: PathBuf,
    pub kind: RootKind,
    pub components: Vec<RootComponent>,
    /// No component in it is the site's.
    pub upstream: bool,
    /// Settings or the conf made it read only.
    pub read_only: bool,
    /// The publish branch of Settings (question 15), as a name or pattern.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_branch: Option<String>,
    pub publish_branch_required: bool,
}

fn path_string<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.display().to_string())
}

impl Root {
    /// The component a path relative to this root is in, the deepest one.
    #[must_use]
    pub fn component_of(&self, relative: &str) -> Option<&RootComponent> {
        self.components
            .iter()
            .filter(|component| {
                component.path.is_empty()
                    || relative == component.path
                    || relative.starts_with(&format!("{}/", component.path))
            })
            .max_by_key(|component| component.path.len())
    }

    /// Whether a path relative to this root is in a site component.
    #[must_use]
    pub fn is_site_path(&self, relative: &str) -> bool {
        self.component_of(relative).is_some_and(|component| component.site)
    }
}

/// The directory holding `.git` (a directory, or a file of a worktree) at or above `dir`.
#[must_use]
pub fn git_top(dir: &Path) -> Option<PathBuf> {
    let mut current = Some(dir);
    while let Some(at) = current {
        if at.join(".git").exists() {
            return Some(at.to_path_buf());
        }
        current = at.parent();
    }
    None
}

/// A root found: its directory, its kind, and its components with their directories.
struct Found {
    path: PathBuf,
    kind: RootKind,
    components: Vec<(String, PathBuf)>,
}

/// Every root of `factory`, repositories first in the order their first component loads.
#[must_use]
pub fn roots(factory: &ExecutionContextFactory, settings: &Settings, facade: Option<&EntityFacade>) -> Vec<Root> {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    // (root path, kind) -> its components, in load order
    let mut found: Vec<Found> = Vec::new();
    fn add(found: &mut Vec<Found>, path: PathBuf, kind: RootKind, component: Option<(String, PathBuf)>) {
        match found.iter_mut().find(|have| have.path == path) {
            Some(root) => root.components.extend(component),
            None => found.push(Found {
                path,
                kind,
                components: component.into_iter().collect(),
            }),
        }
    }
    for component in factory.components() {
        let dir = canonical(&component.location);
        match git_top(&dir) {
            Some(top) => add(&mut found, top, RootKind::Git, Some((component.name.clone(), dir))),
            None => add(
                &mut found,
                dir.clone(),
                RootKind::Plain,
                Some((component.name.clone(), dir)),
            ),
        }
    }
    let conf = canonical(&factory.runtime_dir().join("conf"));
    if conf.is_dir() && !found.iter().any(|root| conf.starts_with(&root.path)) {
        add(&mut found, conf, RootKind::Plain, None);
    }
    let mut labels: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut read_only: BTreeMap<PathBuf, bool> = BTreeMap::new();
    let mut publish: BTreeMap<PathBuf, (Option<String>, bool)> = BTreeMap::new();
    for (location, label) in &settings.roots {
        let path = canonical(location);
        if path.is_dir() {
            let kind = if path.join(".git").exists() {
                RootKind::Git
            } else {
                RootKind::Plain
            };
            add(&mut found, path.clone(), kind, None);
            if !label.is_empty() {
                labels.insert(path, label.clone());
            }
        }
    }
    if let Some(facade) = facade {
        if let Ok(rows) = facade.find(ROOT_ENTITY).and_then(|find| find.list()) {
            for row in rows.iter() {
                let text = |field: &str| {
                    row.get(field)
                        .map(moqui_lib::object::to_plain_string)
                        .unwrap_or_default()
                };
                let path = canonical(Path::new(&text("location")));
                if !path.is_dir() {
                    tracing::warn!("Dev AIDE root {} is not a directory; left out", path.display());
                    continue;
                }
                let kind = if path.join(".git").exists() {
                    RootKind::Git
                } else {
                    RootKind::Plain
                };
                add(&mut found, path.clone(), kind, None);
                if !text("label").is_empty() {
                    labels.insert(path.clone(), text("label"));
                }
                read_only.insert(path.clone(), text("readOnly") == "Y");
                let branch = Some(text("publishBranch")).filter(|branch| !branch.is_empty());
                publish.insert(path, (branch, text("publishBranchRequired") == "Y"));
            }
        }
    }
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    found
        .into_iter()
        .map(|Found { path, kind, components }| {
            let base = path
                .file_name()
                .map_or_else(|| "root".to_string(), |name| name.to_string_lossy().into_owned());
            let count = ids.entry(base.clone()).or_insert(0);
            *count += 1;
            let id = if *count == 1 {
                base.clone()
            } else {
                format!("{base}-{count}")
            };
            let components: Vec<RootComponent> = components
                .into_iter()
                .map(|(name, dir)| RootComponent {
                    site: settings.site_components.contains(&name),
                    path: dir
                        .strip_prefix(&path)
                        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default(),
                    name,
                })
                .collect();
            let (publish_branch, publish_branch_required) = publish.remove(&path).unwrap_or((None, false));
            Root {
                label: labels.remove(&path).unwrap_or_else(|| base.clone()),
                id,
                upstream: !components.iter().any(|component| component.site),
                components,
                read_only: read_only.remove(&path).unwrap_or(false),
                publish_branch,
                publish_branch_required,
                kind,
                path,
            }
        })
        .collect()
}

/// The root of id `id`.
#[must_use]
pub fn find<'a>(roots: &'a [Root], id: &str) -> Option<&'a Root> {
    roots.iter().find(|root| root.id == id)
}

/// The root and relative path of a full path, when a root holds it (the deepest root).
#[must_use]
pub fn locate<'a>(roots: &'a [Root], path: &Path) -> Option<(&'a Root, String)> {
    roots
        .iter()
        .filter(|root| crate::confine::inside(path, &root.path))
        .max_by_key(|root| root.path.as_os_str().len())
        .map(|root| {
            let relative = path
                .strip_prefix(&root.path)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            (root, relative)
        })
}

/// The `DevAideRoot` row of a root's location, when Settings has one.
pub fn root_row(facade: &EntityFacade, location: &Path) -> Option<moqui_core::entity::EntityValue> {
    facade
        .find(ROOT_ENTITY)
        .ok()?
        .eq("location", Value::string(location.display().to_string()))
        .one()
        .ok()?
}
