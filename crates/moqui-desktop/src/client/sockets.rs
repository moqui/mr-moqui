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

//! The commands of the server's sockets of a page, held by the host (plans/Mobile-Desktop.md §4 "One transport seam"):
//! `bridge.js` gives the page a socket-like object (`moquiDesktop.socket`, which `moqui.openSocket` uses); its
//! `socket_open` names a path only. A client's are opened on the server of the page's account and bridged by
//! `moqui_client::bridge`, which keeps their `/notws` frames in the store; frames come back to the page as
//! `window.__moquiSocketEvent(id, event)`, evaluated in that webview (`DesktopHost::socket_event`).
//!
//! The node in process (`crate::node::scheme`, no port) has its pages' sockets here too: `/notws`, `/callws` and
//! `/termws`, opened in process (`moqui_host::in_process`) with the window's cookies, held in a
//! `moqui_client::bridge::Sockets` managed on the app.

use base64::Engine;
use moqui_client::bridge::{event_json, PageSocket, Sockets};
use moqui_client::socket::Event;
use moqui_host::in_process::{LocalSocket, SocketEvent};
use tauri::{AppHandle, Manager, Webview};

use super::Client;
use crate::node::scheme::Local;

/// The paths a page of the node in process may open a socket to.
const LOCAL_PATHS: [&str; 3] = ["/notws", "/callws", "/termws"];

/// The longest text frame a page may send.
const FRAME_MAX: usize = 1024 * 1024;

/// A socket of the node in process: text, and the terminal's binary frames.
struct LocalPageSocket(LocalSocket);

impl PageSocket for LocalPageSocket {
    fn send(&self, text: String) {
        self.0.send(text);
    }

    fn send_binary(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.0.send_binary(bytes);
        Ok(())
    }
}

/// Open `path` (`/notws` or `/callws`, with a query) on the server of the page's account; answers the socket's id. Off
/// the main thread (`async`): it reads the key store.
#[tauri::command(async)]
pub(crate) fn socket_open(app: AppHandle, webview: Webview, path: String) -> Result<u64, String> {
    if let Some(local) = app.try_state::<Local>() {
        return open_local(&app, &local, &webview, &path);
    }
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    moqui_client::bridge::open_remote(&client.core, &client.host, webview.label(), &path)
}

/// Whether a page of the node may open a socket at `path` (with its query).
pub(crate) fn local_path_allowed(path: &str) -> bool {
    let bare = path.split_once('?').map_or(path, |(bare, _)| bare);
    LOCAL_PATHS.contains(&bare) && !path.chars().any(|c| c.is_control() || c == '#')
}

/// [`socket_open`] for a page of the node in process: the socket in process, with the window's cookies.
fn open_local(app: &AppHandle, local: &Local, webview: &Webview, path: &str) -> Result<u64, String> {
    if webview.label() != "main" {
        return Err("Not the node's window".to_string());
    }
    if !local_path_allowed(path) {
        return Err(format!("Not a socket a page may open: {path}"));
    }
    let node = local.node().ok_or("The node is starting")?;
    let sockets = app.try_state::<Sockets>().ok_or("No sockets")?;
    let mut headers = vec![
        (
            "Host".to_string(),
            crate::node::scheme::NODE_ORIGIN
                .trim_start_matches("http://")
                .to_string(),
        ),
        ("Origin".to_string(), crate::node::scheme::NODE_ORIGIN.to_string()),
    ];
    if let Some(cookie) = local.jar.header() {
        headers.push(("Cookie".to_string(), cookie));
    }
    sockets.open("main", |id| {
        let sink_app = app.clone();
        let sink = Box::new(move |event: SocketEvent| {
            let (event, json) = match event {
                // the terminal's bytes, in base64 for the page
                SocketEvent::Binary(bytes) => {
                    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                    (
                        Event::Text(String::new()),
                        serde_json::json!({ "kind": "binary", "data": data }),
                    )
                }
                SocketEvent::Open => (Event::Open, event_json(&Event::Open)),
                SocketEvent::Text(text) => {
                    let event = Event::Text(text);
                    let json = event_json(&event);
                    (event, json)
                }
                SocketEvent::Closed => (Event::Closed, event_json(&Event::Closed)),
                SocketEvent::Unauthorized => (Event::Revoked, event_json(&Event::Revoked)),
            };
            let script = format!("window.__moquiSocketEvent && window.__moquiSocketEvent({id}, {json})");
            if let Some(window) = sink_app.get_webview_window("main") {
                let _ = window.eval(script);
            }
            if matches!(event, Event::Closed | Event::Revoked) {
                if let Some(sockets) = sink_app.try_state::<Sockets>() {
                    sockets.remove(id);
                }
            }
        });
        let socket = node.open_socket(path, &headers, sink).map_err(|err| err.to_string())?;
        Ok(Box::new(LocalPageSocket(socket)) as Box<dyn PageSocket>)
    })
}

/// `work` on the sockets of this app's pages: a client's, or the node's in process (managed on the app).
fn with_sockets<T>(app: &AppHandle, work: impl FnOnce(&Sockets) -> T) -> Option<T> {
    if let Some(client) = app.try_state::<Client>() {
        return Some(work(client.core.sockets()));
    }
    app.try_state::<Sockets>().map(|sockets| work(&sockets))
}

/// Send a frame on one of the page's sockets: `text`, or with `binary` its bytes in base64.
#[tauri::command]
pub(crate) fn socket_send(
    app: AppHandle,
    webview: Webview,
    id: u64,
    text: String,
    binary: Option<bool>,
) -> Result<(), String> {
    if text.len() > FRAME_MAX {
        return Err("The frame is too long".to_string());
    }
    let bytes = if binary == Some(true) {
        let decoded = base64::engine::general_purpose::STANDARD.decode(text.as_bytes());
        Some(decoded.map_err(|_| "The frame is not base64".to_string())?)
    } else {
        None
    };
    with_sockets(&app, |sockets| sockets.send(webview.label(), id, text, bytes))
        .unwrap_or_else(|| Err("No sockets in this app".to_string()))
}

/// Close one of the page's sockets.
#[tauri::command]
pub(crate) fn socket_close(app: AppHandle, webview: Webview, id: u64) -> Result<(), String> {
    with_sockets(&app, |sockets| sockets.close(webview.label(), id));
    Ok(())
}
