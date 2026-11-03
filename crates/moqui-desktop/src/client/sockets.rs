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

//! The server's sockets of a page, held by the host (plans/Mobile-Desktop.md §4 "One transport seam"): a page on an app
//! scheme cannot open `/notws` or `/callws` to its server, and could not send the key if it did. `bridge.js` gives the
//! page a socket-like object (`moquiDesktop.socket`, which `moqui.openSocket` uses); its `socket_open` names a path only,
//! and the host opens it on the server of the page's account (`moqui_client::socket`). Frames come back to the page as
//! `window.__moquiSocketEvent(id, event)`, evaluated in that webview.
//!
//! A socket is the page's: only its webview may send on it or close it, and a page that loads again loses its sockets.
//! When the server closes one, the host lets it go and the page opens another, as it would a browser socket; a close
//! for authentication has the host check the key.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use moqui_client::socket::{Event, Socket};
use tauri::{AppHandle, Manager, Webview};

use super::Client;

/// The longest text frame a page may send.
const FRAME_MAX: usize = 1024 * 1024;

/// The sockets of the pages: by id, with the webview each is for.
#[derive(Default)]
pub(crate) struct Sockets {
    open: Mutex<HashMap<u64, (String, Socket)>>,
    next: AtomicU64,
}

impl Sockets {
    /// Close the sockets of a webview: its page is loading again, or it closed.
    pub(crate) fn close_for(&self, label: &str) {
        moqui_lib::sync::lock(&self.open).retain(|_, (owner, _)| owner != label);
    }

    /// Close every socket: the account changed.
    pub(crate) fn close_all(&self) {
        moqui_lib::sync::lock(&self.open).clear();
    }

    /// Have the pages of `account` open their sockets again, with its new key: each hears its socket close, as after a
    /// drop, and opens another. Answers how many.
    pub(crate) fn reopen_account(&self, app: &AppHandle, client: &Client, account: &str) -> usize {
        let ids: Vec<(u64, String)> = moqui_lib::sync::lock(&self.open)
            .iter()
            .map(|(id, (label, _))| (*id, label.clone()))
            .collect();
        let mut count = 0;
        for (id, label) in ids {
            if client.account_of(&label).is_none_or(|(shown, _)| shown.id != account) {
                continue;
            }
            self.remove(id);
            if let Some(window) = app.get_webview_window(&label) {
                let closed = event_json(&Event::Closed);
                let _ = window.eval(format!(
                    "window.__moquiSocketEvent && window.__moquiSocketEvent({id}, {closed})"
                ));
            }
            count += 1;
        }
        count
    }

    fn remove(&self, id: u64) {
        moqui_lib::sync::lock(&self.open).remove(&id);
    }
}

/// What the page's `__moquiSocketEvent` takes for an event: `{kind}` and the text of a frame.
pub(crate) fn event_json(event: &Event) -> serde_json::Value {
    match event {
        Event::Open => serde_json::json!({ "kind": "open" }),
        Event::Text(text) => serde_json::json!({ "kind": "text", "text": text }),
        Event::Closed => serde_json::json!({ "kind": "closed" }),
        Event::Revoked => serde_json::json!({ "kind": "revoked" }),
    }
}

/// Open `path` (`/notws` or `/callws`, with a query) on the server of the page's account; answers the socket's id. Off
/// the main thread (`async`): it reads the key store.
#[tauri::command(async)]
pub(crate) fn socket_open(app: AppHandle, webview: Webview, path: String) -> Result<u64, String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let label = webview.label().to_string();
    let (account, key) = client.account_of(&label).ok_or("No account is signed in")?;
    let key = key.ok_or("The account is signed out")?;
    let id = client.sockets.next.fetch_add(1, Ordering::Relaxed) + 1;
    let sink_app = app.clone();
    let sink_label = label.clone();
    let sink_account = account.clone();
    let sink = Box::new(move |event: Event| {
        let script = format!(
            "window.__moquiSocketEvent && window.__moquiSocketEvent({id}, {})",
            event_json(&event)
        );
        if let Some(window) = sink_app.get_webview_window(&sink_label) {
            let _ = window.eval(script);
        }
        match event {
            // the page opens another, as it would after a browser socket closed
            Event::Closed => {
                if let Some(client) = sink_app.try_state::<Client>() {
                    client.sockets.remove(id);
                }
            }
            Event::Revoked => {
                if let Some(client) = sink_app.try_state::<Client>() {
                    client.sockets.remove(id);
                }
                super::scheme::check_key(&sink_app, &sink_account);
            }
            Event::Open | Event::Text(_) => {}
        }
    });
    // held while the socket starts: a connection that fails at once removes it only after it is in (open only starts
    // its thread), so none is left reconnecting with nothing to drop it
    let mut open = moqui_lib::sync::lock(&client.sockets.open);
    let socket = moqui_client::socket::open(&account.server, &key, &path, sink).map_err(|err| err.to_string())?;
    open.insert(id, (label, socket));
    Ok(id)
}

/// Send a text frame on one of the page's sockets.
#[tauri::command]
pub(crate) fn socket_send(app: AppHandle, webview: Webview, id: u64, text: String) -> Result<(), String> {
    if text.len() > FRAME_MAX {
        return Err("The frame is too long".to_string());
    }
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let open = moqui_lib::sync::lock(&client.sockets.open);
    match open.get(&id) {
        Some((owner, socket)) if owner == webview.label() => {
            socket.send(text);
            Ok(())
        }
        _ => Err("No such socket".to_string()),
    }
}

/// Close one of the page's sockets.
#[tauri::command]
pub(crate) fn socket_close(app: AppHandle, webview: Webview, id: u64) -> Result<(), String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let mut open = moqui_lib::sync::lock(&client.sockets.open);
    if open.get(&id).is_some_and(|(owner, _)| owner == webview.label()) {
        open.remove(&id);
    }
    Ok(())
}
