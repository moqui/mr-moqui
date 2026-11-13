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

//! Where a runtime keeps its components (plans/Components-Dir.md), in one place: the component sets, the framework
//! component, what marks a directory as a runtime, and the clone-relative paths that tests and dev builds join to
//! `CARGO_MANIFEST_DIR`. Code that has a factory asks it for the loaded components instead; this is for code that
//! runs without a boot (the test runners, the dev tools, the runtime lookup).

use std::path::{Path, PathBuf};

/// A set of components in a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Set {
    /// The framework's components: framework, tools, webroot and the rest.
    Base,
    /// Add-on components, loaded on every instance purpose (or the purposes their `component.xml` names).
    Addon,
    /// Dev-only components, loaded on a dev instance.
    Dev,
}

impl Set {
    /// Every set, in load order.
    pub const ALL: [Set; 3] = [Set::Base, Set::Addon, Set::Dev];

    /// The set's directory, relative to the runtime.
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            Set::Base => "base-component",
            Set::Addon => "component",
            Set::Dev => "dev-component",
        }
    }

    /// Whether the set loads only on some instance purposes, so a run of its specs boots on the dev purpose.
    #[must_use]
    pub fn is_restricted(self) -> bool {
        matches!(self, Set::Dev)
    }
}

/// The framework component of the clone, relative to the repository root.
pub const REPO_FRAMEWORK: &str = "runtime/base-component/framework";
/// The webroot component of the clone, relative to the repository root.
pub const REPO_WEBROOT: &str = "runtime/base-component/webroot";
/// The webroot's static files (`webroot/screen/webroot`) of the clone, relative to the repository root.
pub const REPO_WEBROOT_FILES: &str = "runtime/base-component/webroot/screen/webroot";

/// The directory of `set` in `runtime`.
#[must_use]
pub fn set_dir(runtime: &Path, set: Set) -> PathBuf {
    runtime.join(set.dir_name())
}

/// The framework component of `runtime`, when `MOQUI_FRAMEWORK` is not set.
#[must_use]
pub fn framework_dir(runtime: &Path) -> PathBuf {
    set_dir(runtime, Set::Base).join("framework")
}

/// Whether `dir` is a runtime: it has the base set's directory. A directory test, not the framework's `component.xml`,
/// so a runtime whose framework is missing is still found and the built-in files refill it.
#[must_use]
pub fn is_runtime(dir: &Path) -> bool {
    set_dir(dir, Set::Base).is_dir()
}

/// The set of `runtime` that `dir` is in, if any.
#[must_use]
pub fn set_of(runtime: &Path, dir: &Path) -> Option<Set> {
    Set::ALL.into_iter().find(|set| dir.starts_with(set_dir(runtime, *set)))
}

/// The directories in each of `sets` of `runtime`, sorted by name within a set, with the set of each. A set that is
/// not there has none.
#[must_use]
pub fn component_dirs(runtime: &Path, sets: &[Set]) -> Vec<(Set, PathBuf)> {
    let mut found = Vec::new();
    for set in sets {
        let Ok(entries) = std::fs::read_dir(set_dir(runtime, *set)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .collect();
        dirs.sort();
        found.extend(dirs.into_iter().map(|dir| (*set, dir)));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_dirs_are_by_set_then_name() {
        let root = std::env::temp_dir().join(format!("moqui-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["b", "a"] {
            std::fs::create_dir_all(set_dir(&root, Set::Addon).join(dir)).unwrap();
        }
        std::fs::create_dir_all(framework_dir(&root)).unwrap();
        std::fs::write(set_dir(&root, Set::Addon).join("README.md"), "").unwrap();
        let found = component_dirs(&root, &Set::ALL);
        let names: Vec<(Set, String)> = found
            .iter()
            .map(|(set, dir)| (*set, dir.file_name().unwrap().to_string_lossy().into_owned()))
            .collect();
        assert_eq!(
            names,
            [
                (Set::Base, "framework".into()),
                (Set::Addon, "a".into()),
                (Set::Addon, "b".into())
            ]
        );
        assert!(is_runtime(&root));
        assert_eq!(set_of(&root, &found[1].1.join("screen")), Some(Set::Addon));
        assert_eq!(set_of(&root, &root.join("conf")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
