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
//!
//! The app manifest names the page bridge's commands (src/bridge.rs). With it every app command is checked against
//! the ACL, also from the app's own pages, and gets an `allow-<command>` permission (`_` as `-`); only the capability
//! the bridge adds at run time grants them. `restart_node` is the splash page's: `capabilities/splash.json` grants it
//! to the app's own pages, never to the node's origin. The `client_` and `socket_` commands are the client's
//! (src/client/): its capabilities grant the shell's origin both and an app window's origin the sockets only. The
//! `outbox_`, `draft_` and `file_save` commands are the client's outbox and drafts, granted to both, each for the
//! calling window's account.

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(&[
        "notify",
        "set_counts",
        "set_presence",
        "open_external",
        "set_comms_root",
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
        "outbox_retry",
        "outbox_discard",
        "outbox_unsent",
        "draft_get",
        "draft_list",
        "draft_put",
        "draft_remove",
        "file_save",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("the Tauri build steps failed");
}
