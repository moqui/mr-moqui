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
//! It also moves the CEF runtime beside the executable when Cargo's build directory leaves it in `build/`.
//!
//! The app manifest names the page bridge's commands (src/bridge.rs). With it every app command is checked against
//! the ACL, also from the app's own pages, and gets an `allow-<command>` permission (`_` as `-`); only the capability
//! the bridge adds at run time grants them. `restart_node` is the splash page's: `capabilities/splash.json` grants it
//! to the app's own pages, never to the node's origin. The `client_` and `socket_` commands are the client's
//! (src/client/): its capabilities grant the shell's origin both and an app window's origin the sockets only. The
//! `outbox_`, `draft_` and `file_save` commands are the client's outbox and drafts, granted to both, each for the
//! calling window's account. The `vault_` commands are the vault module the app runs natively for the app windows' pages
//! (plans/archive/Vault-Instances.md V5), granted to the app windows only.

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
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("the Tauri build steps failed");
    place_cef_runtime();
}

/// Moves the CEF runtime from `target/<profile>/build/` to beside the executable.
///
/// cef-dll-sys 154.5.0 copies it to the third parent of `OUT_DIR`. Cargo 1.100's layout is
/// `target/<profile>/build/<package>/<hash>/out`, so that parent is `build/` and the Windows loader
/// never sees `libcef.dll`. When `libcef.dll` or `libcef.so` is a file directly in that directory, the
/// loose files and a `locales/` directory of `.pak` files move up to `target/<profile>/`. The same
/// marker is absent under the older layout, which already copies them beside the executable.
fn place_cef_runtime() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR for a build script"));
    let Some(build_dir) = out_dir
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "build"))
        .map(Path::to_path_buf)
    else {
        return;
    };
    // A later cef-dll-sys rebuild drops the marker back here. Cargo reruns this script when it appears.
    println!("cargo::rerun-if-changed={}", build_dir.join("libcef.dll").display());
    println!("cargo::rerun-if-changed={}", build_dir.join("libcef.so").display());
    if !build_dir.join("libcef.dll").is_file() && !build_dir.join("libcef.so").is_file() {
        return;
    }

    let entries =
        std::fs::read_dir(&build_dir).unwrap_or_else(|error| panic!("reading {}: {error}", build_dir.display()));
    let mut pending = Vec::new();
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| panic!("reading {}: {error}", build_dir.display()));
        let path = entry.path();
        if path.is_file() || (entry.file_name() == "locales" && directory_has_pak(&path)) {
            pending.push(path);
        }
    }
    let Some(profile_dir) = build_dir.parent() else {
        return;
    };
    for path in pending {
        let Some(name) = path.file_name() else {
            continue;
        };
        move_replacing(&path, &profile_dir.join(name));
    }
}

/// A CEF `locales/` directory holds `en-US.pak` and the other locale packs directly. A crate directory
/// of the same name holds hash subdirectories.
fn directory_has_pak(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_file() && path.extension().is_some_and(|ext| ext == "pak")
    })
}

fn move_replacing(from: &Path, to: &Path) {
    if to.is_dir() {
        std::fs::remove_dir_all(to)
            .unwrap_or_else(|error| panic!("removing {} before moving the CEF runtime: {error}", to.display()));
    } else if to.exists() {
        std::fs::remove_file(to)
            .unwrap_or_else(|error| panic!("removing {} before moving the CEF runtime: {error}", to.display()));
    }
    std::fs::rename(from, to).unwrap_or_else(|error| panic!("moving {} to {}: {error}", from.display(), to.display()));
}
