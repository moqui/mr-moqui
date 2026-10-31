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

//! Another Moqui project on this machine (plans/design-dev-aide.md 13.6): a Java Moqui checkout, an MR Moqui clone or
//! an MR Moqui runtime, named by its directory, whose artifacts the dev tools read without it being the runtime of
//! the process. [`detect`] tells the layout from marker paths; [`ModelDirs`] are the directories a model-only boot of
//! it reads; [`boot`] boots it inside a running server without touching the process; [`cache`] keeps the index of a
//! few such projects in memory, fresh with their files, for the artifact tool and `DevToolsServices`.

pub mod cache;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_core::context::{BootOptions, ExecutionContextFactory};
use moqui_lib::error::Error;
use serde::Serialize;

/// Which Moqui a project is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// Java Moqui: `framework/` (Java and Groovy sources, no conf of MR Moqui's) beside `runtime/`.
    Java,
    /// MR Moqui: a runtime with `base-component/framework`.
    Mr,
}

impl Layout {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Java => "java",
            Self::Mr => "mr",
        }
    }
}

/// A Moqui project found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    /// The canonical directory named: the Java checkout, the MR Moqui clone, or the MR Moqui runtime.
    pub root: PathBuf,
    pub layout: Layout,
    /// Its runtime directory (canonical).
    pub runtime: PathBuf,
}

/// Where a model-only boot reads: the runtime, the framework directory (its conf and XSDs), and the framework
/// component's directory when it is another (a Java Moqui checkout's `framework`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDirs {
    pub runtime: PathBuf,
    pub framework: PathBuf,
    pub framework_component: Option<PathBuf>,
}

impl Project {
    /// The directories of a model-only boot of the project. An MR Moqui project reads its own framework; a Java
    /// checkout's artifacts are read with MR Moqui's framework conf, which `mr_framework` answers (the server's own, or
    /// the one beside the executable).
    ///
    /// # Errors
    ///
    /// Returns an error for a Java checkout when `mr_framework` answers none.
    pub fn model_dirs(&self, mr_framework: impl FnOnce() -> Option<PathBuf>) -> Result<ModelDirs, Error> {
        match self.layout {
            Layout::Mr => Ok(ModelDirs {
                runtime: self.runtime.clone(),
                framework: self.runtime.join("base-component/framework"),
                framework_component: None,
            }),
            Layout::Java => {
                let framework = mr_framework().ok_or_else(|| {
                    Error::new(format!(
                        "{} is a Java Moqui checkout; its artifacts are read with MR Moqui's framework conf, which was \
                         not found: set MOQUI_FRAMEWORK to an MR Moqui runtime/base-component/framework",
                        self.root.display()
                    ))
                })?;
                Ok(ModelDirs {
                    runtime: self.runtime.clone(),
                    framework,
                    framework_component: Some(self.root.join("framework")),
                })
            }
        }
    }
}

/// The project at `dir`, its layout told from marker paths (no walk of the tree, but a short one of `framework/src`):
///
/// - Java Moqui: `framework/src/main/java` or `framework/src/main/groovy`, or `framework/build.gradle` with a `.java`
///   or `.groovy` file under `framework/src`; it needs `runtime/` beside `framework/`. `dir` may also be such a
///   checkout's `runtime`.
/// - MR Moqui: `runtime/base-component/framework/component.xml` (a clone), or `base-component/framework/component.xml`
///   (a runtime directory).
///
/// # Errors
///
/// Returns an error naming what was looked for when `dir` is neither, or is a Java framework with no runtime, or an MR
/// Moqui clone (`crates/`) with no runtime framework.
pub fn detect(dir: &Path) -> Result<Project, Error> {
    let dir = canonical_dir(dir)?;
    if is_java_framework(&dir.join("framework")) {
        return java_project(&dir);
    }
    if let Some(parent) = dir.parent().filter(|parent| {
        dir.file_name().is_some_and(|name| name == "runtime") && is_java_framework(&parent.join("framework"))
    }) {
        return java_project(parent);
    }
    if let Some(runtime) = mr_runtime(&dir) {
        return Ok(Project {
            root: dir,
            layout: Layout::Mr,
            runtime,
        });
    }
    if dir.join("crates").is_dir() {
        return Err(Error::new(format!(
            "{} looks like an MR Moqui clone (crates/) but has no runtime/base-component/framework/component.xml",
            dir.display()
        )));
    }
    Err(Error::new(format!(
        "{} is not a Moqui project: a Java Moqui checkout has framework/src/main/java or framework/src/main/groovy (or \
         framework/build.gradle with Java or Groovy sources) beside runtime/, and an MR Moqui clone or runtime has \
         runtime/base-component/framework or base-component/framework",
        dir.display()
    )))
}

/// `dir` read as a Java Moqui checkout whatever its sources (`--java`): `framework/` beside `runtime/`, and not an
/// MR Moqui clone. `dir` may also be such a checkout's `runtime`.
///
/// # Errors
///
/// Returns an error when `dir` has no `framework/` and `runtime/`, or is an MR Moqui clone.
pub fn detect_java(dir: &Path) -> Result<Project, Error> {
    let dir = canonical_dir(dir)?;
    let beside_runtime = dir
        .parent()
        .filter(|parent| dir.file_name().is_some_and(|name| name == "runtime") && parent.join("framework").is_dir());
    let checkout = if dir.join("framework").is_dir() {
        dir.clone()
    } else if let Some(parent) = beside_runtime {
        parent.to_path_buf()
    } else {
        return Err(Error::new(format!(
            "{} is not a Java Moqui checkout: it needs framework/ beside runtime/",
            dir.display()
        )));
    };
    if checkout
        .join("runtime/base-component/framework/component.xml")
        .is_file()
    {
        return Err(Error::new(format!(
            "{} is an MR Moqui clone (runtime/base-component/framework), not a Java Moqui checkout",
            checkout.display()
        )));
    }
    java_project(&checkout)
}

fn java_project(checkout: &Path) -> Result<Project, Error> {
    let runtime = checkout.join("runtime");
    if !runtime.is_dir() {
        return Err(Error::new(format!(
            "{} is a Java Moqui framework with no runtime/ beside it",
            checkout.display()
        )));
    }
    Ok(Project {
        root: checkout.to_path_buf(),
        layout: Layout::Java,
        runtime: canonical_dir(&runtime)?,
    })
}

/// Whether `framework` is a Java Moqui framework directory: Java or Groovy sources in the Gradle layout, or a
/// `build.gradle` with such sources somewhere under `src`.
fn is_java_framework(framework: &Path) -> bool {
    let src = framework.join("src");
    src.join("main/java").is_dir()
        || src.join("main/groovy").is_dir()
        || (framework.join("build.gradle").is_file() && has_sources(&src, 6, &mut 2000))
}

/// Whether there is a `.java` or `.groovy` file under `dir`, at most `depth` levels down and `budget` entries read.
fn has_sources(dir: &Path, depth: u32, budget: &mut u32) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        if *budget == 0 {
            return false;
        }
        *budget -= 1;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "java" || ext == "groovy") {
            return true;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            dirs.push(path);
        }
    }
    depth > 0 && dirs.iter().any(|sub| has_sources(sub, depth - 1, budget))
}

/// The MR Moqui runtime of `dir`: `dir/runtime` of a clone, or `dir` itself.
fn mr_runtime(dir: &Path) -> Option<PathBuf> {
    [dir.join("runtime"), dir.to_path_buf()]
        .into_iter()
        .find(|runtime| runtime.join("base-component/framework/component.xml").is_file())
}

fn canonical_dir(dir: &Path) -> Result<PathBuf, Error> {
    let canonical = std::fs::canonicalize(dir).map_err(|err| Error::new(format!("{}: {err}", dir.display())))?;
    if !canonical.is_dir() {
        return Err(Error::new(format!("{} is not a directory", dir.display())));
    }
    Ok(canonical)
}

/// A project directory a request names: an absolute path with no `.` or `..` part, of a directory that exists,
/// answered canonical (links resolved). A request names a path on the server's machine; nothing relative to the
/// server's working directory is taken.
///
/// # Errors
///
/// Returns an error when the path is relative, has a `.` or `..` part, or is no directory.
pub fn requested_dir(text: &str) -> Result<PathBuf, Error> {
    let text = text.trim();
    let path = Path::new(text);
    if text.is_empty() || !path.is_absolute() {
        return Err(Error::new(format!("project {text}: an absolute directory path")));
    }
    // Split the text, not the path: a path's components leave out an inner `.`.
    if text.split(['/', '\\']).any(|part| part == "." || part == "..") {
        return Err(Error::new(format!("project {text}: no . or .. parts")));
    }
    canonical_dir(path)
}

/// A no-database driver: the model loads and no data is read.
struct NoDatabase;

impl moqui_core::driver::DbDriver for NoDatabase {
    fn connect(&self) -> Result<Box<dyn moqui_core::driver::DbConnection>, Error> {
        Err(Error::new(
            "this factory reads another project's artifacts and has no database",
        ))
    }
}

/// A factory of `dirs` booted inside this process for its artifacts only ([`BootOptions::artifacts_only`]): the
/// process's properties, threads and hooks are left as they are, and its entity facade is model-only over no
/// database. A definition that does not resolve is logged and left out, as on any boot.
///
/// # Errors
///
/// Returns an error when the conf or the entity model does not load.
pub fn boot(dirs: &ModelDirs) -> Result<Arc<ExecutionContextFactory>, Error> {
    let options = BootOptions {
        framework_component: dirs.framework_component.clone(),
        artifacts_only: true,
        ..BootOptions::default()
    };
    let factory = ExecutionContextFactory::boot_with_options(&dirs.runtime, &dirs.framework, options)
        .map_err(|err| Error::new(err.to_string()))?;
    let pool = moqui_core::transaction::PoolConfig {
        min: 0,
        max: 1,
        reserve: 0,
        acquire_timeout: std::time::Duration::from_millis(10),
        ..Default::default()
    };
    let transactions = moqui_core::transaction::TransactionManager::new(Arc::new(NoDatabase), pool)?;
    let groups = vec![moqui_core::entity::EntityGroup::new(
        "transactional",
        "postgres",
        transactions,
    )];
    let facade = moqui_core::entity::open_for_factory(&factory, "transactional", groups)?;
    facade.set_model_only();
    factory.set_entity(facade);
    Ok(factory)
}

#[cfg(test)]
mod tests;
