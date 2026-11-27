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

//! The repository's crate patches reach the apps that build the framework in a workspace of their own. Cargo takes
//! `[patch.crates-io]` only from the root manifest of the workspace it builds, so the desktop app (a workspace of its
//! own, whose node in the app links the framework) names the root's patches again; without them it built the crates.io
//! releases they replace, yrs without SEC-TLS-95's depth limit among them (plans/archive/Security-Review.md SEC-CLI-126). The
//! generated crate of `./moq build` copies the root's table as it writes itself (`root_patch_table`,
//! scripts/moq/build.sh; checked in scripts/moq-test.sh). The apps are outside the workspace, so their manifests are
//! read here by path, and `./moq test-framework` checks them on every push. The mobile app's and the Tauri glue's
//! `Cargo.lock` are committed, and must hold the repository's versions of the crates they share with it
//! (`./moq update-app-locks`; SEC-CLI-104).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The repository's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// The lines of the table `name` (`[name]` to the next table), comments and blank lines left out.
fn table<'a>(manifest: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("[{name}]");
    let mut lines = Vec::new();
    let mut inside = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == header;
        } else if inside && !line.is_empty() && !line.starts_with('#') {
            lines.push(line);
        }
    }
    lines
}

/// The quoted value of `key` in an inline table (`{ key = "value", ... }`).
fn inline_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let after = line.split(&format!("{key} = \"")).nth(1)?;
    after.split('"').next()
}

/// A manifest's patches of crates.io crates, each by where it comes from: a git repository at a revision, or a
/// directory, made absolute against the manifest's own.
fn patches(manifest: &Path) -> BTreeMap<String, String> {
    let dir = manifest.parent().unwrap();
    table(&read(manifest), "patch.crates-io")
        .into_iter()
        .map(|line| {
            let (name, spec) = line.split_once('=').unwrap_or_else(|| panic!("{line}"));
            let source = if let Some(path) = inline_value(spec, "path") {
                let full = dir.join(path);
                let full = full.canonicalize().unwrap_or_else(|err| panic!("{}: {err}", full.display()));
                format!("path {}", full.display())
            } else {
                let git = inline_value(spec, "git").unwrap_or_else(|| panic!("no git or path: {line}"));
                let rev = inline_value(spec, "rev").unwrap_or_else(|| panic!("a git patch with no rev: {line}"));
                format!("git {git} {rev}")
            };
            (name.trim().to_string(), source)
        })
        .collect()
}

/// The manifests under `crates/` of a workspace of their own that build the framework (a path dependency on
/// moqui-core or moqui-host).
fn app_workspaces() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root().join("crates"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("Cargo.toml"))
        .filter(|manifest| manifest.is_file())
        .filter(|manifest| {
            let text = read(manifest);
            let own_workspace = text.lines().any(|line| line.trim() == "[workspace]");
            let framework = text.lines().map(str::trim).any(|line| {
                (line.starts_with("moqui-core =") || line.starts_with("moqui-host =")) && line.contains("path = ")
            });
            own_workspace && framework
        })
        .collect();
    found.sort();
    found
}

#[test]
fn an_app_that_builds_the_framework_in_its_own_workspace_takes_every_patch_of_the_repository() {
    let wanted = patches(&root().join("Cargo.toml"));
    assert!(wanted.contains_key("yrs"), "the repository patches yrs (SEC-TLS-95): {wanted:?}");
    let apps = app_workspaces();
    assert!(
        apps.iter().any(|manifest| manifest.ends_with("moqui-desktop/Cargo.toml")),
        "the desktop app builds the framework in a workspace of its own: {apps:?}"
    );
    for manifest in apps {
        let has = patches(&manifest);
        for (name, source) in &wanted {
            assert_eq!(
                has.get(name),
                Some(source),
                "{} does not patch {name} as the repository's Cargo.toml does ({source}); cargo takes patches only \
                 from the root of the workspace it builds",
                manifest.display()
            );
        }
    }
}

/// A lock file's crates.io packages, as name and version.
fn registry_packages(lock: &Path) -> Vec<(String, String)> {
    let mut packages = Vec::new();
    let (mut name, mut version, mut registry) = (String::new(), String::new(), false);
    let mut flush = |name: &mut String, version: &mut String, registry: &mut bool| {
        if !name.is_empty() && *registry {
            packages.push((std::mem::take(name), std::mem::take(version)));
        }
        name.clear();
        version.clear();
        *registry = false;
    };
    for line in read(lock).lines().map(str::trim) {
        if line == "[[package]]" {
            flush(&mut name, &mut version, &mut registry);
        } else if let Some(value) = line.strip_prefix("name = ") {
            name = value.trim_matches('"').to_string();
        } else if let Some(value) = line.strip_prefix("version = ") {
            version = value.trim_matches('"').to_string();
        } else if let Some(value) = line.strip_prefix("source = ") {
            registry = value.trim_matches('"').starts_with("registry+");
        }
    }
    flush(&mut name, &mut version, &mut registry);
    packages
}

/// The semver range a version is compatible within: its major, else `0.<minor>`, else `0.0.<patch>`.
fn range(version: &str) -> String {
    let parts: Vec<&str> = version.split(['.', '+', '-']).collect();
    let part = |index: usize| parts.get(index).copied().unwrap_or("0");
    if part(0) != "0" {
        part(0).to_string()
    } else if part(1) != "0" {
        format!("0.{}", part(1))
    } else {
        format!("0.0.{}", part(2))
    }
}

#[test]
fn the_mobile_app_and_the_glue_commit_locks_with_the_repositorys_versions_of_the_crates_they_share() {
    let ignored = read(&root().join(".gitignore"));
    let repository: BTreeMap<(String, String), String> = registry_packages(&root().join("Cargo.lock"))
        .into_iter()
        .map(|(name, version)| ((name, range(&version)), version))
        .collect();
    for app in ["crates/moqui-client-tauri", "crates/moqui-mobile"] {
        assert!(
            !ignored.lines().any(|line| line.trim() == format!("/{app}/Cargo.lock")),
            "{app}/Cargo.lock is committed (SEC-CLI-104), not ignored"
        );
        let drift: Vec<String> = registry_packages(&root().join(app).join("Cargo.lock"))
            .into_iter()
            .filter_map(|(name, version)| {
                let theirs = repository.get(&(name.clone(), range(&version)))?;
                (theirs != &version).then(|| format!("{name} {version}, the repository's {theirs}"))
            })
            .collect();
        assert!(
            drift.is_empty(),
            "{app}/Cargo.lock has other versions than the repository's of crates they share: {drift:?}; run \
             ./moq update-app-locks and commit it"
        );
    }
}

#[test]
fn a_semver_range_is_the_first_part_that_is_not_zero() {
    assert_eq!(range("1.0.229"), "1");
    assert_eq!(range("0.12.4"), "0.12");
    assert_eq!(range("0.0.3"), "0.0.3");
    assert_eq!(range("3.0.0-alpha.7"), "3");
}
