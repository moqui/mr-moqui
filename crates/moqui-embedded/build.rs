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

//! Packs the runtime files src/lib.rs embeds into `OUT_DIR` (`embedded.bin` and `embedded.idx`), at the level the
//! features `production` and `all` select; src/select.rs has what each level takes and the format. Those two pack the
//! whole webroot, so they need the vault's browser module built first (`scripts/vault-wasm.sh`, which `./moq build`
//! runs before cargo; it is not kept in git): without it the build fails naming it, rather than embed no module.
//!
//! Under the features `opensearch` and `chromium` it also names the program archives src/programs.rs embeds, from
//! `MOQUI_OPENSEARCH_ARCHIVE` and `MOQUI_CHROMIUM_ARCHIVE` or the cache that `./moq opensearch-download` and `./moq
//! chromium-download` fill ([`cache_dir`]); they are embedded as they are, with no copy into `OUT_DIR`.
//!
//! Cargo watches each packed top-level entry of a component, not the component as a whole, so an edit to a component's
//! tests (or its `.git`) does not pack again; a new top-level directory in a component, or a new component, is packed
//! once anything else changes, or when `MOQUI_EMBED_COMPONENTS` (which `./moq build` sets to the runtime's component
//! directories) changes. The runtime is the repository's, or the one `MOQUI_EMBED_RUNTIME` names.

#[allow(dead_code)]
#[path = "src/select.rs"]
mod select;

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    // `./moq build --embed production|all` names the runtime it uses (MOQUI_RUNTIME), as `./moq archive` would pack it
    println!("cargo:rerun-if-env-changed=MOQUI_EMBED_RUNTIME");
    let runtime = std::env::var_os("MOQUI_EMBED_RUNTIME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| manifest.join("../../runtime"), PathBuf::from);
    let level = select::Level::from_features(
        std::env::var_os("CARGO_FEATURE_PRODUCTION").is_some(),
        std::env::var_os("CARGO_FEATURE_ALL").is_some(),
    );
    println!("cargo:rerun-if-env-changed=MOQUI_EMBED_COMPONENTS");
    println!(
        "cargo:rerun-if-changed={}",
        manifest.join("archive-excludes.txt").display()
    );

    let mut watch = |path: &Path| println!("cargo:rerun-if-changed={}", path.display());
    if level.whole_webroot() {
        check_browser_libs(&runtime);
    }
    let files = select::gather(&runtime, &manifest, level, &mut watch).unwrap_or_else(|err| panic!("{err}"));
    let (blob, index) = select::pack(&files).unwrap_or_else(|err| panic!("{err}"));

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    write_out(&out.join("embedded.bin"), &blob);
    write_out(&out.join("embedded.idx"), index.as_bytes());

    stage_programs();
}

/// The browser libraries of each component this build packs that has a `build/js-build.json` (`./moq build-js`
/// makes them; git keeps only their `MANIFEST.json`): the base components always, the others when
/// `MOQUI_EMBED_COMPONENTS` names them. A file its manifest lists and the build has not made fails the build.
fn check_browser_libs(runtime: &Path) {
    let components = runtime.join("components");
    let embedded = std::env::var("MOQUI_EMBED_COMPONENTS").unwrap_or_default();
    let Ok(sets) = std::fs::read_dir(&components) else {
        return;
    };
    let mut roots: Vec<PathBuf> = sets
        .flatten()
        .filter(|set| set.path().is_dir())
        .flat_map(|set| std::fs::read_dir(set.path()).into_iter().flatten().flatten())
        .map(|component| component.path())
        .filter(|root| root.join("build/js-build.json").is_file())
        .filter(|root| {
            let relative = root
                .strip_prefix(&components)
                .unwrap_or(root)
                .to_string_lossy()
                .replace('\\', "/");
            relative.starts_with("base/")
                || embedded
                    .split_whitespace()
                    .any(|listed| listed.trim_end_matches('/') == relative)
        })
        .collect();
    roots.sort();
    for root in roots {
        check_component_libs(&root);
    }
}

fn check_component_libs(root: &Path) {
    let config_path = root.join("build/js-build.json");
    println!("cargo:rerun-if-changed={}", config_path.display());
    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&config_path).unwrap_or_else(|err| panic!("{}: {err}", config_path.display())),
    )
    .unwrap_or_else(|err| panic!("{}: {err}", config_path.display()));
    let output = config["output"]
        .as_str()
        .unwrap_or_else(|| panic!("{}: no output", config_path.display()));
    let libs = root.join(output);
    let manifest = libs.join("MANIFEST.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&manifest).unwrap_or_else(|err| panic!("{}: {err}; run ./moq build-js", manifest.display())),
    )
    .unwrap_or_else(|err| panic!("{}: {err}", manifest.display()));
    let files: Vec<&serde_json::Value> = json["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|library| library["files"].as_array().into_iter().flatten())
        .collect();
    // Only a component with optional font packs (the webroot) records which are installed.
    let selected: Vec<String> = if files.iter().any(|file| file["pack"].is_string()) {
        let selection = libs.join("fonts-extra/installed.json");
        println!("cargo:rerun-if-changed={}", selection.display());
        serde_json::from_slice(
            &std::fs::read(&selection)
                .unwrap_or_else(|err| panic!("{}: {err}; run ./moq build-js", selection.display())),
        )
        .expect("installed font packs")
    } else {
        Vec::new()
    };
    for file in files {
        if file["pack"]
            .as_str()
            .is_some_and(|pack| !selected.iter().any(|name| name == pack))
        {
            continue;
        }
        let base = if file["target"] == "server" {
            root.join("script/libs")
        } else {
            libs.clone()
        };
        let path = base.join(file["path"].as_str().expect("asset path"));
        println!("cargo:rerun-if-changed={}", path.display());
        assert!(path.is_file(), "{} missing; run ./moq build-js", path.display());
    }
}

/// Name the OpenSearch and Chromium archives to embed, under their features, for src/programs.rs: their paths in
/// `MOQUI_OPENSEARCH_STAGED` and `MOQUI_CHROMIUM_STAGED`, and their version and revision.
fn stage_programs() {
    for name in [
        "MOQUI_OPENSEARCH_ARCHIVE",
        "MOQUI_OPENSEARCH_VERSION",
        "MOQUI_CHROMIUM_ARCHIVE",
        "MOQUI_EMBED_CACHE",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    if std::env::var_os("CARGO_FEATURE_OPENSEARCH").is_some() {
        // .cargo/config.toml forces it, as POSTGRESQL_VERSION
        let version = std::env::var("MOQUI_OPENSEARCH_VERSION").unwrap_or_default();
        assert!(
            !version.is_empty() && version.chars().all(|c| c.is_ascii_digit() || c == '.'),
            "MOQUI_OPENSEARCH_VERSION must be an exact version like 3.4.0 (.cargo/config.toml), not {version:?}"
        );
        let archive = archive_path(
            "MOQUI_OPENSEARCH_ARCHIVE",
            &format!("opensearch-{version}-{target}.tar.gz"),
            "./moq opensearch-download --cache-only",
        );
        println!("cargo:rustc-env=MOQUI_OPENSEARCH_STAGED={}", archive.display());
        println!("cargo:rustc-env=MOQUI_OPENSEARCH_EMBEDDED_VERSION={version}");
    }
    if std::env::var_os("CARGO_FEATURE_CHROMIUM").is_some() {
        let archive = archive_path(
            "MOQUI_CHROMIUM_ARCHIVE",
            &format!("chromium-{target}.tar.gz"),
            "./moq chromium-download --cache-only",
        );
        let sidecar = PathBuf::from(format!("{}.revision", archive.display()));
        println!("cargo:rerun-if-changed={}", sidecar.display());
        let revision = std::fs::read_to_string(&sidecar).unwrap_or_else(|err| {
            panic!(
                "embed-chromium needs the revision of {} in {} ({err}); ./moq chromium-download --cache-only writes both",
                archive.display(),
                sidecar.display()
            )
        });
        let revision = revision.split_whitespace().next().unwrap_or_default();
        assert!(
            !revision.is_empty() && revision.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'),
            "{} does not hold a Chromium revision",
            sidecar.display()
        );
        println!("cargo:rustc-env=MOQUI_CHROMIUM_STAGED={}", archive.display());
        println!("cargo:rustc-env=MOQUI_CHROMIUM_EMBEDDED_REVISION={revision}");
    }
}

/// The archive `env` names, else `name` in the [`cache_dir`]; it must be there.
fn archive_path(env: &str, name: &str, fill: &str) -> PathBuf {
    let archive = std::env::var_os(env).map_or_else(|| cache_dir().join(name), PathBuf::from);
    println!("cargo:rerun-if-changed={}", archive.display());
    assert!(
        archive.is_file(),
        "the archive to embed is not at {}: run {fill} to download it there, or set {env}",
        archive.display()
    );
    std::fs::canonicalize(&archive).unwrap_or(archive)
}

/// Where `./moq` keeps the program archives a build embeds: `MOQUI_EMBED_CACHE`, else `moqui/embed` in the user's cache
/// directory (`$XDG_CACHE_HOME` or `~/.cache`; `%LOCALAPPDATA%` on Windows).
fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MOQUI_EMBED_CACHE").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("moqui/embed");
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    cache.join("moqui/embed")
}

fn write_out(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|err| panic!("could not write {}: {err}", path.display()));
}
