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
//! The client settings are `client-conf.xml`, or the file `MOQUI_CLIENT_CONF` names (an absolute path, or one relative to
//! this crate), so a build of one app (its own `start-path`) needs no change to a tracked file: `src/lib.rs` embeds the
//! file `MOQUI_CLIENT_CONF_FILE` names.
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
    "scheme_fetch",
    "client_show_shell",
    "open_in_shell",
    "notify",
    "notify_clear",
    "chat_room_shown",
    "push_check",
    "push_check_fix",
    "push_test",
    "open_external",
    "call_state",
    "call_take_answer",
    "call_audio",
    "call_audio_route",
    "secure_screen",
    "screen_start",
    "screen_offer",
    "screen_key",
    "screen_answer",
    "screen_remote_offer",
    "screen_stop",
    "screen_status",
    "calendar_insert",
    "call_dial_account",
    "call_dial_choice",
    "call_dial_from",
    "contacts_search",
    "contact_names",
    "location_allow",
    "location_share_start",
    "location_share_stop",
    "location_share_status",
];

fn main() {
    let staged = Path::new("../../target/mobile-shell");
    println!("cargo:rerun-if-changed=../../target/mobile-shell");
    println!("cargo:rerun-if-env-changed=MOQUI_CLIENT_CONF");
    let conf = std::env::var("MOQUI_CLIENT_CONF")
        .ok()
        .filter(|path| !path.trim().is_empty());
    let conf = Path::new(env!("CARGO_MANIFEST_DIR")).join(conf.as_deref().unwrap_or("client-conf.xml").trim());
    assert!(conf.is_file(), "MOQUI_CLIENT_CONF: no file {}", conf.display());
    println!("cargo:rerun-if-changed={}", conf.display());
    println!("cargo:rustc-env=MOQUI_CLIENT_CONF_FILE={}", conf.display());
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
