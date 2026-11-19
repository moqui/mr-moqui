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
//! releases they replace, yrs without SEC-TLS-95's depth limit among them (plans/Security-Review.md SEC-CLI-126). The
//! generated crate of `./moq build` copies the root's table as it writes itself (`root_patch_table`,
//! scripts/moq/build.sh; checked in scripts/moq-test.sh). The apps are outside the workspace, so their manifests are
//! read here by path, and `./moq test-framework` checks them on every push.

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
