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

//! Where a runtime keeps its components (plans/archive/Components-Dir.md), in one place: the component sets, the
//! framework component, what marks a directory as a runtime, and the clone-relative paths that tests and dev builds
//! join to `CARGO_MANIFEST_DIR`. Code that has a factory asks it for the loaded components instead; this is for code
//! that runs without a boot (the test runners, the dev tools, the runtime lookup).

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
    /// Test-only add-on components, loaded on a dev or test instance, never on production.
    Test,
}

impl Set {
    /// Every set, in load order.
    pub const ALL: [Set; 4] = [Set::Base, Set::Addon, Set::Dev, Set::Test];

    /// The set's directory, relative to the runtime.
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            Set::Base => "components/base",
            Set::Addon => "components/addon",
            Set::Dev => "components/dev",
            Set::Test => "components/test",
        }
    }

    /// Whether the set loads only on some instance purposes, so a run of its specs boots on the dev purpose.
    #[must_use]
    pub fn is_restricted(self) -> bool {
        matches!(self, Set::Dev | Set::Test)
    }
}

/// The framework component of the clone, relative to the repository root.
pub const REPO_FRAMEWORK: &str = "runtime/components/base/framework";
/// The webroot component of the clone, relative to the repository root.
pub const REPO_WEBROOT: &str = "runtime/components/base/webroot";
/// The webroot's static files (`webroot/screen/webroot`) of the clone, relative to the repository root.
pub const REPO_WEBROOT_FILES: &str = "runtime/components/base/webroot/screen/webroot";
/// The directory of the component sets in a runtime, with their manifest `components.xml`.
pub const COMPONENTS_DIR: &str = "components";

/// The sets' manifest of `runtime`, `components/components.xml`, which lists the sets that load.
#[must_use]
pub fn manifest(runtime: &Path) -> PathBuf {
    runtime.join(COMPONENTS_DIR).join("components.xml")
}

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

/// The component directories of the layout before `components/` (plans/archive/Components-Dir.md), each with where its
/// components go now. Nothing moves them; a boot that finds one says so ([`warn_old_dirs`]).
pub const OLD_DIRS: [(&str, &str); 5] = [
    ("base-component", "components/base"),
    ("component", "components/addon"),
    ("dev-component", "components/dev"),
    ("ecomponent", "components/addon"),
    ("deployed-component", "components/addon"),
];

/// The component directories of a Java Moqui checkout's runtime, in its load order, which a model-only boot that reads
/// the checkout's artifacts (`BootOptions::framework_component`) discovers its components in.
pub const JAVA_DIRS: [&str; 4] = ["base-component", "mantle", "component", "ecomponent"];

/// The directories in `runtime`'s component sets and in the component directories of a Java Moqui checkout's runtime
/// ([`JAVA_DIRS`]), for the dev tools that read either kind of project; a runtime has the one or the other.
#[must_use]
pub fn component_dirs_any_layout(runtime: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = component_dirs(runtime, &Set::ALL)
        .into_iter()
        .map(|(_, dir)| dir)
        .collect();
    for name in JAVA_DIRS {
        let Ok(entries) = std::fs::read_dir(runtime.join(name)) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .collect();
        found.sort();
        dirs.extend(found);
    }
    dirs
}

/// Whether `framework` is a Java Moqui framework component (its `build.gradle`), whose runtime has [`JAVA_DIRS`].
#[must_use]
pub fn is_java_framework(framework: &Path) -> bool {
    framework.join("build.gradle").is_file()
}

/// A WARN for each directory of [`OLD_DIRS`] in `runtime` that has something in it and that no `component-dir` of the
/// conf names (`listed`, its locations): its components no longer load, and the WARN says where they go.
pub fn warn_old_dirs(runtime: &Path, listed: &[String]) {
    for (old, new) in OLD_DIRS {
        let dir = runtime.join(old);
        let in_use = std::fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_some());
        if in_use && !listed.iter().any(|location| location.trim_end_matches('/') == old) {
            tracing::warn!(
                "{} is the old place of components and is not loaded: move its components to {} \
                 (plans/archive/Components-Dir.md)",
                dir.display(),
                runtime.join(new).display()
            );
        }
    }
}

/// Link the base set `base` (another runtime's `components/base`) into `runtime` as its base set, with a copy of the
/// sets' manifest beside it, so a scratch runtime loads the framework's components as the real one does. For tests
/// and tools that build a runtime from another.
///
/// # Errors
///
/// Returns the error of creating `components/`, linking or copying.
#[cfg(unix)]
pub fn link_base_set(base: &Path, runtime: &Path) -> std::io::Result<()> {
    let sets = runtime.join(COMPONENTS_DIR);
    std::fs::create_dir_all(&sets)?;
    std::os::unix::fs::symlink(base, set_dir(runtime, Set::Base))?;
    if let Some(manifest) = base
        .parent()
        .map(|parent| parent.join("components.xml"))
        .filter(|path| path.is_file())
    {
        std::fs::copy(manifest, sets.join("components.xml"))?;
    }
    Ok(())
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

    /// The old component directories are named nowhere in the repository but this module and the archived plans, the one
    /// that moved them among them (plans/archive/Components-Dir.md §5), so a stale path or a new hand-built one shows up
    /// here rather than at the next move. Skipped outside a git checkout.
    #[test]
    fn the_old_component_directories_are_named_nowhere() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let Ok(out) = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["ls-files", "-z"])
            .output()
        else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let allowed =
            |path: &str| path.starts_with("plans/archive/") || path == "crates/moqui-core/src/context/layout.rs";
        let old = [
            "base-component",
            "dev-component",
            "ecomponent",
            "deployed-component",
            "runtime/component/",
        ];
        let mut found = Vec::new();
        for path in String::from_utf8_lossy(&out.stdout)
            .split('\0')
            .filter(|path| !path.is_empty() && !allowed(path))
        {
            let Ok(text) = std::fs::read_to_string(repo.join(path)) else {
                continue;
            };
            for (number, line) in text.lines().enumerate() {
                let named = old.iter().any(|name| {
                    line.match_indices(name)
                        .any(|(at, _)| !line[..at].ends_with(|ch: char| ch.is_ascii_alphanumeric()))
                });
                if named {
                    found.push(format!("{path}:{}", number + 1));
                }
            }
        }
        assert!(
            found.is_empty(),
            "the old component directories are named in:\n{}",
            found.join("\n")
        );
    }
}
