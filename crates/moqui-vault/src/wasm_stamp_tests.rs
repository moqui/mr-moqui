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

//! The checked-in browser module against its source (scripts/vault-wasm.sh writes both the module and the stamp).

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::text::hex;

const VAULT_JS: &str = "runtime/components/base/webroot/screen/webroot/js/vault";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// `Cargo.toml` and every `.rs` under `src/` but the tests, relative to the crate, in byte order.
fn source_files(crate_dir: &Path) -> Vec<String> {
    let mut files = vec!["Cargo.toml".to_string()];
    let mut dirs = vec![PathBuf::from("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(crate_dir.join(&dir)).unwrap() {
            let entry = entry.unwrap();
            let relative = dir.join(entry.file_name());
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type().unwrap().is_dir() {
                dirs.push(relative);
            } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
                files.push(relative.to_string_lossy().to_string());
            }
        }
    }
    files.sort();
    files
}

/// The checksum Cargo.lock records for `name` at `version`.
fn lock_checksum(lock: &str, name: &str, version: &str) -> Option<String> {
    fn value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
        line.strip_prefix(key)?.strip_prefix(" = \"")?.strip_suffix('"')
    }
    let mut current: (Option<&str>, Option<&str>) = (None, None);
    for line in lock.lines() {
        if line == "[[package]]" {
            current = (None, None);
        } else if let Some(found) = value(line, "name") {
            current.0 = Some(found);
        } else if let Some(found) = value(line, "version") {
            current.1 = Some(found);
        } else if let Some(found) = value(line, "checksum") {
            if current == (Some(name), Some(version)) {
                return Some(found.to_string());
            }
        }
    }
    None
}

/// The names of the dependencies the crate's Cargo.toml takes from the workspace (`x.workspace = true` or
/// `x = { workspace = true, ... }`), sorted.
fn workspace_dependency_names(manifest: &str) -> Vec<String> {
    let mut names: Vec<String> = manifest
        .lines()
        .filter_map(|line| {
            let name: String = line
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            let rest = line[name.len()..].replace(' ', "");
            let from_workspace = rest.starts_with(".workspace=true") || rest.starts_with("={workspace=true");
            (!name.is_empty() && from_workspace).then_some(name)
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The lines of the TOML section `header` (up to the next header), as the script's awk reads them.
fn section<'a>(text: &'a str, header: &str) -> Vec<&'a str> {
    let mut on = false;
    let mut out = Vec::new();
    for line in text.lines() {
        if line == header {
            on = true;
        } else if line.starts_with('[') {
            on = false;
        } else if on {
            out.push(line);
        }
    }
    out
}

#[test]
fn the_checked_in_wasm_is_what_the_source_builds() {
    let root = root();
    if std::env::var_os("MOQUI_REGENERATE_WASM").is_some() {
        let status = std::process::Command::new("bash")
            .arg(root.join("scripts/vault-wasm.sh"))
            .status()
            .unwrap();
        assert!(status.success(), "scripts/vault-wasm.sh failed");
    }
    let stamp_text = std::fs::read_to_string(root.join(VAULT_JS).join("moqui_vault.wasm.json"))
        .expect("the stamp: run scripts/vault-wasm.sh");
    let stamp: serde_json::Value = serde_json::from_str(&stamp_text).unwrap();
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();

    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut lines = String::new();
    for path in source_files(crate_dir) {
        let bytes = std::fs::read(crate_dir.join(&path)).unwrap();
        lines.push_str(&format!("{path} {}\n", sha(&bytes)));
    }
    let manifest = std::fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
    let workspace = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let workspace_dependencies = section(&workspace, "[workspace.dependencies]");
    for name in workspace_dependency_names(&manifest) {
        let prefix = format!("{name} = ");
        for line in workspace_dependencies.iter().filter(|line| line.starts_with(&prefix)) {
            lines.push_str(&format!("workspace {}\n", line.trim()));
        }
    }
    let config = std::fs::read_to_string(root.join(".cargo/config.toml")).unwrap();
    for line in section(&config, "[profile.release]") {
        if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            lines.push_str(&format!("profile {}\n", line.trim()));
        }
    }
    for entry in stamp["lock"].as_array().unwrap() {
        let entry = entry.as_str().unwrap();
        let mut fields = entry.split(' ');
        let (Some(name), Some(version), Some(checksum)) = (fields.next(), fields.next(), fields.next()) else {
            panic!("a stamp lock entry is name, version and checksum: {entry}");
        };
        assert_eq!(
            lock_checksum(&lock, name, version).as_deref(),
            Some(checksum),
            "Cargo.lock changed {name} {version}: rebuild the module with scripts/vault-wasm.sh"
        );
        lines.push_str(&format!("lock {entry}\n"));
    }
    assert_eq!(
        stamp["source_sha256"].as_str().unwrap(),
        sha(lines.as_bytes()),
        "crates/moqui-vault changed since the module was built: run scripts/vault-wasm.sh \
         (or MOQUI_REGENERATE_WASM=1 cargo test -p moqui-vault the_checked_in_wasm) and commit both files"
    );
    match std::process::Command::new("rustc").arg("-V").output() {
        Ok(output) if output.status.success() => {
            let rustc = String::from_utf8_lossy(&output.stdout).trim().to_string();
            assert_eq!(
                stamp["rustc"].as_str().unwrap(),
                rustc,
                "the module was built by another rustc: rebuild it with scripts/vault-wasm.sh and commit both files"
            );
        }
        _ => eprintln!("rustc -V did not run: the module's rustc version is not compared"),
    }
    let wasm = std::fs::read(root.join(VAULT_JS).join("moqui_vault.wasm")).unwrap();
    assert_eq!(
        stamp["wasm_sha256"].as_str().unwrap(),
        sha(&wasm),
        "the module is not the one its stamp names"
    );
}
