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

//! Runs the Tauri build steps: the config, the icons and the embedded assets for tauri::generate_context!, the Android
//! package name tauri::mobile_entry_point binds to, and the app manifest of the commands.
//!
//! The assets are the client's shell as `./moq android` stages it (`target/mobile-shell`, `frontendDist`); a build
//! without it (`cargo test` on the desktop) gets a placeholder page, so the crate still compiles, and says so.
//!
//! The app manifest names the app's commands, so each is checked against the ACL; the one capability the app has
//! (`capabilities/main.json`) grants them to its own pages in the one webview. Which page may call the shell's is the
//! host's check (src/host.rs).

use std::path::Path;

const COMMANDS: &[&str] = &[
    "client_state",
    "client_sign_in",
    "client_send_code",
    "client_sign_out",
    "client_switch_account",
    "client_add_account",
    "client_remove_account",
    "client_open_remote",
    "client_take_open",
    "client_sso_providers",
    "client_sso_start",
    "client_sso_cancel",
    "socket_open",
    "socket_send",
    "socket_close",
    "outbox_add",
    "outbox_list",
    "outbox_retry",
    "outbox_discard",
    "outbox_unsent",
    "draft_get",
    "draft_list",
    "draft_put",
    "draft_remove",
    "file_save",
    "scheme_fetch",
    "client_show_shell",
    "open_in_shell",
    "notify",
    "open_external",
];

fn main() {
    let staged = Path::new("../../target/mobile-shell");
    println!("cargo:rerun-if-changed=../../target/mobile-shell");
    println!("cargo:rerun-if-changed=client-conf.xml");
    if !staged.join("shell/index.html").exists() {
        println!(
            "cargo:warning=the client's shell is not staged (./moq android stages it): a placeholder page instead"
        );
        let shell = staged.join("shell");
        std::fs::create_dir_all(&shell).expect("the placeholder shell's directory");
        std::fs::write(
            shell.join("index.html"),
            "<!DOCTYPE html><title>MR Moqui</title><p>The shell was not staged: build with ./moq android.</p>",
        )
        .expect("the placeholder shell's page");
    }
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("the Tauri build steps failed");
}
