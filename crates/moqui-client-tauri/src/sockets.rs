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

//! The commands of a page's sockets, all of them the server's (plans/Mobile-Desktop.md §4 "One transport seam"): the
//! page's `moquiDesktop.socket` names a path only (`/notws` or `/callws`), which the host opens on the server of the
//! page's account with its key and bridges (`moqui_client::bridge`); frames come back as
//! `window.__moquiSocketEvent(id, event)` (`ClientHost::socket_event`). The mobile app's; the desktop app has its own,
//! which also holds the node's in process with its binary terminal frames.

use tauri::{AppHandle, Webview};

use crate::state;

/// The longest text frame a page may send.
const FRAME_MAX: usize = 1024 * 1024;

/// Open `path` (`/notws` or `/callws`, with a query) on the server of the page's account; answers the socket's id. Off
/// the main thread (`async`): it reads the key store.
#[tauri::command(async)]
pub fn socket_open(app: AppHandle, webview: Webview, path: String) -> Result<u64, String> {
    let state = state(&app)?;
    if !state.shell.network_ready() {
        return Err("This app cannot make secure connections on this device".to_string());
    }
    moqui_client::bridge::open_remote(&state.core, &state.host, webview.label(), &path)
}

/// Send a text frame on one of the page's sockets; the server's sockets take no binary frames from a page.
#[tauri::command]
pub fn socket_send(
    app: AppHandle,
    webview: Webview,
    id: u64,
    text: String,
    binary: Option<bool>,
) -> Result<(), String> {
    if text.len() > FRAME_MAX {
        return Err("The frame is too long".to_string());
    }
    if binary == Some(true) {
        return Err("A server's socket takes text frames only".to_string());
    }
    state(&app)?.core.sockets().send(webview.label(), id, text, None)
}

/// Close one of the page's sockets.
#[tauri::command]
pub fn socket_close(app: AppHandle, webview: Webview, id: u64) -> Result<(), String> {
    state(&app)?.core.sockets().close(webview.label(), id);
    Ok(())
}
