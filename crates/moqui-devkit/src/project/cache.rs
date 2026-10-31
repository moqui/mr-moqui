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

//! The indexes of other projects a running dev server answers about, kept in memory (plans/design-dev-aide.md 13.6).
//!
//! [`open`] gives a project's [`ProjectIndex`]: booted ([`super::boot`]) and read into a [`MemoryIndex`] on first use,
//! then kept for the next requests. At most [`capacity`] projects are kept (`moqui_devtools_projects`, 3), the least
//! recently used dropped first. The first build of one project holds only that project's slot, so requests about the
//! server's own runtime or another project go on meanwhile.
//!
//! Freshness without a restart: at each use, when more than [`RECHECK`] passed since the last look, the project's
//! component files are listed and their modification times and sizes compared with the ones read. Changed, added and
//! removed files are read again into the index (the names resolved again); a changed `component.xml`, `MoquiConf.xml`,
//! runtime conf or set of components boots the project again. A changed entity or service file also marks the booted
//! model old, so the checks and the service parameters, which read the model and not the index, boot it again first.
//! A file that does not parse is logged and left out; a project that does not boot is an error of that request only.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;
use moqui_lib::sync::{lock, read, write};
use serde::Serialize;

use super::{ModelDirs, Project};
use crate::index::{self, ArtifactRow, Direction, Extension, Located, MemoryIndex, RefEntry};
use crate::{ArtifactKind, Problem};

/// How long a look at a project's files holds before the next request looks again.
pub const RECHECK: Duration = Duration::from_millis(500);

/// A file's modification time and size, as last read.
type Stamp = (Option<SystemTime>, u64);

fn stamp(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path)
        .ok()
        .map(|meta| (meta.modified().ok(), meta.len()))
}

/// One project's booted model and index.
pub struct ProjectIndex {
    pub project: Project,
    pub dirs: ModelDirs,
    pub factory: Arc<ExecutionContextFactory>,
    index: RwLock<MemoryIndex>,
    seen: Mutex<Seen>,
    /// An entity or service file changed since the boot: the model is behind the index.
    model_old: AtomicBool,
    /// How long the boot and the first read took.
    pub build_millis: u64,
}

struct Seen {
    files: BTreeMap<PathBuf, Stamp>,
    /// What a change of boots the project again: the components found and their conf files, the runtime conf.
    structure: BTreeMap<PathBuf, Option<Stamp>>,
    looked: Instant,
}

/// What a look at the files did.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refresh {
    /// Files read again (changed or added) and left out (removed).
    pub read: usize,
    pub removed: usize,
    /// Booted again: the components or the conf changed.
    pub rebooted: bool,
    pub millis: u64,
}

/// The state of a project's index, as the services answer it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub project: String,
    pub layout: &'static str,
    pub runtime: String,
    pub components: Vec<String>,
    pub files: usize,
    pub artifacts: usize,
    pub refs: usize,
    /// Files that did not read or parse, with why.
    pub problems: Vec<(String, String)>,
    /// What the entity model left out (a view or relationship that does not resolve).
    pub model_problems: Vec<String>,
    pub build_millis: u64,
}

impl ProjectIndex {
    /// Boot `project` on `dirs` and read its files.
    ///
    /// # Errors
    ///
    /// Returns why when the project does not boot.
    pub fn build(project: Project, dirs: ModelDirs) -> Result<Self, Error> {
        let started = Instant::now();
        let factory = super::boot(&dirs)?;
        let context = factory.new_context();
        let _entered = context.enter();
        let files = index::component_files(&factory);
        let memory = MemoryIndex::of_files(&factory, &files);
        let build_millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::info!(
            "Project {} ({}): {} components, {} files, {} artifacts read in {build_millis}ms",
            project.root.display(),
            project.layout.as_str(),
            factory.components().len(),
            files.len(),
            memory.artifacts.len()
        );
        let seen = Seen {
            files: files
                .iter()
                .filter_map(|file| stamp(&file.path).map(|found| (file.path.clone(), found)))
                .collect(),
            structure: structure(&project, &factory),
            looked: Instant::now(),
        };
        Ok(Self {
            project,
            dirs,
            factory,
            index: RwLock::new(memory),
            seen: Mutex::new(seen),
            model_old: AtomicBool::new(false),
            build_millis,
        })
    }

    /// Look at the files when [`RECHECK`] passed: read the changed ones again. Answers `None` when the project must
    /// boot again (its components or conf changed).
    fn refresh(&self) -> Option<Refresh> {
        let mut seen = lock(&self.seen);
        if seen.looked.elapsed() < RECHECK {
            return Some(Refresh::default());
        }
        let started = Instant::now();
        if structure(&self.project, &self.factory) != seen.structure {
            return None;
        }
        let files = index::component_files(&self.factory);
        let mut now: BTreeMap<PathBuf, Stamp> = BTreeMap::new();
        let mut changed: Vec<Located> = Vec::new();
        for file in files {
            let Some(found) = stamp(&file.path) else {
                continue;
            };
            if seen.files.get(&file.path) != Some(&found) {
                changed.push(file.clone());
            }
            now.insert(file.path, found);
        }
        let removed: Vec<PathBuf> = seen
            .files
            .keys()
            .filter(|path| !now.contains_key(*path))
            .cloned()
            .collect();
        let mut report = Refresh {
            read: changed.len(),
            removed: removed.len(),
            ..Refresh::default()
        };
        if !changed.is_empty() || !removed.is_empty() {
            let model = |kind: ArtifactKind| matches!(kind, ArtifactKind::Entity | ArtifactKind::Service);
            let removed_model = removed.iter().any(|path| {
                index::locate(&self.factory, path).is_some_and(|located| model(located.kind))
                    || path
                        .components()
                        .any(|part| part.as_os_str() == "entity" || part.as_os_str() == "service")
            });
            if removed_model || changed.iter().any(|file| model(file.kind)) {
                self.model_old.store(true, Ordering::Release);
            }
            let paths: Vec<String> = changed
                .iter()
                .map(|file| file.path.clone())
                .chain(removed)
                .map(|path| path.display().to_string())
                .collect();
            let context = self.factory.new_context();
            let _entered = context.enter();
            write(&self.index).replace_files(&self.factory, &paths, &changed);
            tracing::info!(
                "Project {}: {} files read again, {} removed",
                self.project.root.display(),
                report.read,
                report.removed
            );
        }
        seen.files = now;
        seen.looked = Instant::now();
        report.millis = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        Some(report)
    }

    /// Run `body` in a context of the project's factory (its services and entities, not the server's), authorization
    /// off: it only reads definitions.
    pub fn entered<T>(&self, body: impl FnOnce(&Arc<ExecutionContextFactory>) -> T) -> T {
        let context = self.factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        body(&self.factory)
    }

    /// Read the index.
    pub fn with_index<T>(&self, body: impl FnOnce(&MemoryIndex) -> T) -> T {
        body(&read(&self.index))
    }

    #[must_use]
    pub fn find(&self, text: &str, kinds: &[String], components: &[String], limit: usize) -> Vec<ArtifactRow> {
        self.with_index(|index| index.find_in(text, kinds, components, limit))
    }

    #[must_use]
    pub fn describe(&self, id: &str) -> Option<ArtifactRow> {
        self.with_index(|index| index.describe(id))
    }

    /// An artifact by its id, by a `component://` screen location, or the best match of its name.
    #[must_use]
    pub fn named(&self, name: &str) -> Option<ArtifactRow> {
        self.with_index(|index| {
            name.strip_prefix("component://")
                .and_then(|_| index.describe(&format!("screen:{name}")))
                .or_else(|| index.named(name))
        })
    }

    #[must_use]
    pub fn children(&self, id: &str) -> Vec<ArtifactRow> {
        self.with_index(|index| index.children(id))
    }

    #[must_use]
    pub fn refs(&self, id: &str, direction: Direction, kinds: &[String]) -> Vec<RefEntry> {
        self.with_index(|index| index.refs_of_kinds(id, direction, kinds))
    }

    #[must_use]
    pub fn at(&self, path: &str, line: u32, word: &str) -> Option<index::At> {
        self.with_index(|index| index.at(path, line, word))
    }

    #[must_use]
    pub fn extensions(&self, components: &[String]) -> Vec<Extension> {
        self.with_index(|index| index.extensions(components))
    }

    /// The artifacts the file `path` (in a component of the project) defines at its top.
    #[must_use]
    pub fn in_file(&self, path: &Path) -> Vec<ArtifactRow> {
        self.with_index(|index| index.in_file(&path.display().to_string()))
    }

    /// The file of a `component://<component>/<path>` location, or an absolute path, when it is in a component of the
    /// project (a path out of every component is not the project's to read).
    #[must_use]
    pub fn file(&self, named: &str) -> Option<PathBuf> {
        let named = named.trim();
        let path = if let Some(location) = named.strip_prefix("component://") {
            let (name, rest) = location.split_once('/')?;
            let component = self
                .factory
                .components()
                .iter()
                .find(|component| component.name == name)?;
            component.location.join(rest)
        } else {
            PathBuf::from(named)
        };
        if !path.is_absolute() || path.components().any(|part| part == std::path::Component::ParentDir) {
            return None;
        }
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        self.location_of(&path).map(|_| path)
    }

    /// `component://<component>/<path>` of a file in a component of the project.
    #[must_use]
    pub fn location_of(&self, path: &Path) -> Option<String> {
        self.factory.components().iter().find_map(|component| {
            let dir = std::fs::canonicalize(&component.location).unwrap_or_else(|_| component.location.clone());
            path.strip_prefix(&dir).ok().map(|relative| {
                format!(
                    "component://{}/{}",
                    component.name,
                    relative.to_string_lossy().replace('\\', "/")
                )
            })
        })
    }

    /// The checks of `text` as the file at `path`, on the project's model.
    #[must_use]
    pub fn check(&self, path: &Path, text: &str) -> Vec<Problem> {
        if path.extension().is_none_or(|ext| ext != "xml") {
            return Vec::new();
        }
        self.entered(|factory| crate::check::check_text(factory, path, text))
    }

    /// A service's in and out parameters (name, type, required), from the project's service definitions.
    #[must_use]
    pub fn service_parameters(&self, name: &str) -> Option<serde_json::Value> {
        self.entered(|_| moqui_core::service::define::find(name).map(|def| parameters_of(&def)))
    }

    #[must_use]
    pub fn summary(&self) -> Summary {
        let model_problems = self
            .factory
            .entity()
            .map(|facade| facade.with_model(|model| model.problems().to_vec()))
            .unwrap_or_default();
        self.with_index(|index| Summary {
            model_problems,
            project: self.project.root.display().to_string(),
            layout: self.project.layout.as_str(),
            runtime: self.project.runtime.display().to_string(),
            components: self
                .factory
                .components()
                .iter()
                .map(|component| component.name.clone())
                .collect(),
            files: lock(&self.seen).files.len(),
            artifacts: index.artifacts.len(),
            refs: index.refs.len(),
            problems: index.problems.clone(),
            build_millis: self.build_millis,
        })
    }
}

/// A service definition's parameters as the artifact tool answers them: `{in: [...], out: [...]}`, each
/// `{name, type, required}`.
#[must_use]
pub fn parameters_of(def: &moqui_core::service::define::ServiceDef) -> serde_json::Value {
    let list = |params: &[moqui_core::service::define::ParamDef]| -> Vec<serde_json::Value> {
        params
            .iter()
            .map(|param| {
                serde_json::json!({"name": param.name, "type": param.java_type,
                    "required": param.required == moqui_core::service::define::Required::True})
            })
            .collect()
    };
    serde_json::json!({"in": list(&def.in_params), "out": list(&def.out_params)})
}

/// The paths whose change boots the project again, with their stamps: each component directory found (with its
/// `component.xml` and `MoquiConf.xml`), the runtime's conf files and the framework component of a Java checkout.
fn structure(project: &Project, factory: &ExecutionContextFactory) -> BTreeMap<PathBuf, Option<Stamp>> {
    let mut found = BTreeMap::new();
    let mut dirs: Vec<PathBuf> = ["base-component", "component", "dev-component"]
        .iter()
        .flat_map(|kind| {
            std::fs::read_dir(project.runtime.join(kind))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
        })
        .collect();
    dirs.extend(factory.components().iter().map(|component| component.location.clone()));
    let known: HashSet<PathBuf> = dirs.drain(..).collect();
    for dir in known {
        for file in ["component.xml", "MoquiConf.xml"] {
            let path = dir.join(file);
            found.insert(path.clone(), stamp(&path));
        }
    }
    for entry in std::fs::read_dir(project.runtime.join("conf"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "xml") {
            found.insert(path.clone(), stamp(&path));
        }
    }
    found
}

/// A slot of the cache: the project's index once built. Its lock is held while the project builds or its files are
/// looked at, so requests about the same project wait for one build, and the others do not.
#[derive(Default)]
struct Slot {
    built: Mutex<Option<Arc<ProjectIndex>>>,
}

/// The projects kept, most recently used first.
static PROJECTS: Mutex<Vec<(PathBuf, Arc<Slot>)>> = Mutex::new(Vec::new());

/// How many projects are kept: `moqui_devtools_projects`, 3 when not set (at least 1).
#[must_use]
pub fn capacity() -> usize {
    moqui_lib::env::get_prop_or_env("moqui_devtools_projects")
        .trim()
        .parse::<usize>()
        .unwrap_or(3)
        .max(1)
}

/// The slot of `root`, made the most recently used; the least recently used beyond [`capacity`] are dropped (a
/// request still using one keeps it until it is done).
fn slot(root: &Path) -> Arc<Slot> {
    let mut kept = lock(&PROJECTS);
    let slot = match kept.iter().position(|(path, _)| path == root) {
        Some(at) => kept.remove(at).1,
        None => Arc::new(Slot::default()),
    };
    kept.insert(0, (root.to_path_buf(), Arc::clone(&slot)));
    let keep = capacity().min(kept.len());
    for (dropped, _) in kept.drain(keep..) {
        tracing::info!("Project {}: index dropped from memory", dropped.display());
    }
    slot
}

/// The index of `project` read with `dirs`: kept from an earlier request and brought up to date with its files, or
/// built now. With `model`, a model behind its files (an entity or service file changed) is booted again first.
///
/// # Errors
///
/// Returns why the project does not boot. A panic in the build is caught and answered as an error.
pub fn open(project: Project, dirs: ModelDirs, model: bool) -> Result<Arc<ProjectIndex>, Error> {
    let slot = slot(&project.root);
    let mut built = lock(&slot.built);
    if let Some(current) = built.as_ref() {
        let fresh = current.refresh().is_some() && !(model && current.model_old.load(Ordering::Acquire));
        if fresh && current.dirs == dirs {
            return Ok(Arc::clone(current));
        }
        tracing::info!("Project {}: booting again", project.root.display());
    }
    *built = None;
    let root = project.root.clone();
    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ProjectIndex::build(project, dirs)))
        .unwrap_or_else(|_| Err(Error::new(format!("the build of {} panicked", root.display()))));
    match made {
        Ok(index) => {
            let index = Arc::new(index);
            *built = Some(Arc::clone(&index));
            Ok(index)
        }
        Err(err) => {
            tracing::error!("Project {}: not read: {err}", root.display());
            Err(err)
        }
    }
}

/// Drop the kept index of `root`, so the next request builds it again.
pub fn forget(root: &Path) {
    lock(&PROJECTS).retain(|(path, _)| path != root);
}

/// The roots of the projects kept, most recently used first.
#[must_use]
pub fn kept() -> Vec<PathBuf> {
    lock(&PROJECTS).iter().map(|(path, _)| path.clone()).collect()
}

/// The project a request names in `project` (an absolute directory on this machine, [`super::requested_dir`]), read
/// against the server `own`: `None` when the text is empty or names the server's own runtime, which its database
/// index answers. A Java Moqui checkout is read with the server's own framework conf. With `model`, see [`open`].
///
/// # Errors
///
/// Returns why the directory is not taken (relative, `..`, not a Moqui project) or the project does not boot.
pub fn requested(
    project: &str,
    own: &ExecutionContextFactory,
    model: bool,
) -> Result<Option<Arc<ProjectIndex>>, Error> {
    if project.trim().is_empty() {
        return Ok(None);
    }
    let found = super::detect(&super::requested_dir(project)?)?;
    let own_runtime = std::fs::canonicalize(own.runtime_dir()).unwrap_or_else(|_| own.runtime_dir().to_path_buf());
    if found.runtime == own_runtime {
        return Ok(None);
    }
    let dirs = found.model_dirs(|| Some(own.framework_dir().to_path_buf()))?;
    open(found, dirs, model).map(Some)
}
