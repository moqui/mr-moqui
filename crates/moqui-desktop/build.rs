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

//! Runs the Tauri build steps: the config, the capabilities and the icons, for tauri::generate_context!.
//! It also copies the CEF runtime beside the executable when Cargo's build directory leaves it in `build/`.
//!
//! The app manifest names the page bridge's commands (src/bridge.rs). With it every app command is checked against
//! the ACL, also from the app's own pages, and gets an `allow-<command>` permission (`_` as `-`); only the capability
//! the bridge adds at run time grants them. `restart_node` is the splash page's: `capabilities/splash.json` grants it
//! to the app's own pages, never to the node's origin. The `client_` and `socket_` commands are the client's
//! (src/client/): its capabilities grant the shell's origin both and an app window's origin the sockets only. The
//! `outbox_`, `draft_` and `file_save` commands are the client's outbox and drafts, granted to both, each for the
//! calling window's account. The `vault_` commands are the vault module the app runs natively for the app windows' pages
//! (plans/archive/Vault-Instances.md V5), granted to the app windows only.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "notify",
        "notify_clear",
        "push_check",
        "push_test",
        "set_counts",
        "set_presence",
        "open_external",
        "node_setup_secret",
        "secure_screen",
        "capture_sources",
        "capture_choose",
        "capture_overlay",
        "capture_ink",
        "capture_overlay_close",
        "restart_node",
        "client_state",
        "client_sign_in",
        "client_send_code",
        "client_sign_out",
        "client_switch_account",
        "client_add_account",
        "client_remove_account",
        "client_open_remote",
        "client_take_open",
        "open_in_shell",
        "client_sso_providers",
        "client_sso_start",
        "client_sso_cancel",
        "socket_open",
        "socket_send",
        "socket_close",
        "outbox_add",
        "outbox_list",
        "outbox_kinds",
        "outbox_retry",
        "outbox_discard",
        "outbox_unsent",
        "draft_get",
        "draft_list",
        "draft_put",
        "draft_remove",
        "file_save",
        "vault_call",
        "vault_memory",
        "packages_list",
        "packages_download",
        "packages_cancel",
        "packages_delete",
        "packages_status",
        "packages_addresses",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("the Tauri build steps failed");
    place_cef_runtime();
}

/// Copies the CEF runtime beside the executable when Cargo's build directory layout leaves it in `build/`.
///
/// cef-dll-sys 154.5.0 copies the runtime files of the CEF distribution to the third parent of its `OUT_DIR`. Under
/// Cargo 1.100's layout, `target/<profile>/build/<package>/<hash>/out`, that is `build/`, where the Windows loader never
/// sees `libcef.dll`; under the older `target/<profile>/build/<package>-<hash>/out` it is beside the executable and
/// nothing is left to do. The files come from the distribution itself, which the patched tauri-runtime-cef passes on
/// as `DEP_TAURI_RUNTIME_CEF_CEF_DIR` (patches/tauri-runtime-cef/build.rs): its loose files, less the two that only
/// build the wrapper, and `locales/`. This script watches those sources, which only a new distribution changes, so a
/// build with nothing changed does not run it again. The copies cef-dll-sys left in `build/` are deleted. Without the
/// variable, under the newer layout, the build warns that the runtime was not placed.
fn place_cef_runtime() {
    // On macOS the bundler places the framework; cef-dll-sys copies nothing there.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" && target_os != "linux" {
        return;
    }
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script"));
    let Some(build_dir) = layout_build_dir(&out_dir) else {
        return;
    };
    let Some(profile_dir) = build_dir.parent() else {
        return;
    };
    let Some(cef_dir) = std::env::var_os("DEP_TAURI_RUNTIME_CEF_CEF_DIR").map(PathBuf::from) else {
        println!(
            "cargo::warning=the CEF runtime was not placed beside the executable: tauri-runtime-cef passed no \
             DEP_TAURI_RUNTIME_CEF_CEF_DIR (is patches/tauri-runtime-cef in [patch.crates-io]?)"
        );
        return;
    };

    for name in file_names(&cef_dir) {
        remove_stray_copy(&build_dir.join(&name));
        if BUILD_ONLY_FILES.iter().any(|build_only| name == *build_only) {
            continue;
        }
        let from = cef_dir.join(&name);
        println!("cargo::rerun-if-changed={}", from.display());
        copy_if_changed(&from, &profile_dir.join(&name));
    }
    let locales = cef_dir.join("locales");
    println!("cargo::rerun-if-changed={}", locales.display());
    let names = file_names(&locales);
    if !names.is_empty() {
        let to_locales = profile_dir.join("locales");
        std::fs::create_dir_all(&to_locales)
            .unwrap_or_else(|error| panic!("creating {}: {error}", to_locales.display()));
        for name in &names {
            copy_if_changed(&locales.join(name), &to_locales.join(name));
            remove_stray_copy(&build_dir.join("locales").join(name));
        }
        // Empty once its CEF files are gone; a crate's build directory of the same name keeps its contents.
        let _ = std::fs::remove_dir(build_dir.join("locales"));
    }
}

/// `target/<profile>/build/` when `OUT_DIR` is `build/<package>/<hash>/out` under it, Cargo 1.100's layout, where
/// cef-dll-sys's copies land. None under the older `build/<package>-<hash>/out`, also with a profile named `build`.
fn layout_build_dir(out_dir: &Path) -> Option<&Path> {
    let package = std::env::var("CARGO_PKG_NAME").ok()?;
    let mut ancestors = out_dir.ancestors();
    let is_named = |dir: Option<&Path>, name: &str| dir.and_then(Path::file_name).is_some_and(|dir| dir == name);
    if !is_named(ancestors.next(), "out") {
        return None;
    }
    ancestors.next()?;
    if !is_named(ancestors.next(), &package) {
        return None;
    }
    ancestors.next().filter(|dir| is_named(Some(dir), "build"))
}

/// The loose files of a CEF distribution, among those cef-dll-sys copies, that only build its C++ wrapper (and record
/// the archive): not copied beside the executable.
const BUILD_ONLY_FILES: [&str; 2] = ["CMakeLists.txt", "archive.json"];

/// The names of the files directly in a directory, none when it cannot be read.
fn file_names(dir: &Path) -> Vec<OsString> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    names
}

/// Copies a CEF file over the one beside the executable unless that one has the same size and modification time,
/// which the copy takes from its source. The copy is written beside it and renamed over it: on Linux a running app
/// keeps the old file mapped, which truncating it in place would crash. On Windows a running app keeps its CEF files
/// open, so a rebuild that changes nothing in CEF leaves them alone, and a CEF upgrade that has to replace one says to
/// close the app.
fn copy_if_changed(from: &Path, to: &Path) {
    let from_meta = from
        .metadata()
        .unwrap_or_else(|error| panic!("reading {}: {error}", from.display()));
    let modified = from_meta.modified().ok();
    if let Ok(to_meta) = to.metadata() {
        if to_meta.is_file()
            && to_meta.len() == from_meta.len()
            && modified.is_some()
            && to_meta.modified().ok() == modified
        {
            return;
        }
    }
    if to.is_dir() {
        std::fs::remove_dir_all(to)
            .unwrap_or_else(|error| panic!("removing {} before copying the CEF runtime: {error}", to.display()));
    }
    let mut part_name = to.file_name().expect("a CEF file has a name").to_os_string();
    part_name.push(".part");
    let part = to.with_file_name(part_name);
    std::fs::copy(from, &part)
        .unwrap_or_else(|error| panic!("copying {} to {}: {error}", from.display(), part.display()));
    if let Some(modified) = modified {
        std::fs::File::options()
            .write(true)
            .open(&part)
            .and_then(|file| file.set_modified(modified))
            .unwrap_or_else(|error| panic!("setting the modification time of {}: {error}", part.display()));
    }
    if let Err(error) = std::fs::rename(&part, to) {
        let _ = std::fs::remove_file(&part);
        panic!(
            "replacing {}: {error}. If moqui-desktop is running, close it and rebuild.",
            to.display()
        );
    }
}

/// Deletes a copy cef-dll-sys left in `build/`; one that cannot be deleted only takes disk space.
fn remove_stray_copy(path: &Path) {
    if path.is_file() {
        if let Err(error) = std::fs::remove_file(path) {
            println!(
                "cargo::warning=could not delete the CEF file {}: {error}",
                path.display()
            );
        }
    }
}
