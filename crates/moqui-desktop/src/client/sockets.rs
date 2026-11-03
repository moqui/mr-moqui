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
//!
//! A client's `/notws` frames are also written into the account's store (`push.rs`, MD4). A drop of any of the
//! account's `/notws` marks the timelines' heads stale, as pushes may be missed while it is down, and has the mirror
//! (`mirror.rs`) read them again when the shell's opens next; the shell's opening starts the mirror. An opening also
//! wakes the outbox's sender (`flush.rs`), and my own message pushed back takes its operation off (`outbox::echoed`).
//!
//! The node in process (`crate::node::scheme`, no port) has its pages' sockets here too: `/notws`, `/callws` and
//! `/termws`, opened in process (`moqui_host::in_process`) with the window's cookies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use base64::Engine;
use moqui_client::socket::{Event, Socket};
use moqui_host::in_process::{LocalSocket, SocketEvent};
use tauri::{AppHandle, Manager, Webview};

use super::Client;
use crate::node::scheme::Local;

/// The paths a page of the node in process may open a socket to.
const LOCAL_PATHS: [&str; 3] = ["/notws", "/callws", "/termws"];

/// A socket held for a page: on a client's server, or on the node in process.
enum Held {
    Remote(Socket),
    Local(LocalSocket),
}

impl Held {
    fn send(&self, text: String) {
        match self {
            Self::Remote(socket) => socket.send(text),
            Self::Local(socket) => socket.send(text),
        }
    }

    /// A binary frame: on the node in process only (the terminal's); a client's sockets carry text.
    fn send_binary(&self, bytes: Vec<u8>) -> Result<(), String> {
        match self {
            Self::Remote(_) => Err("This socket carries text only".to_string()),
            Self::Local(socket) => {
                socket.send_binary(bytes);
                Ok(())
            }
        }
    }
}

/// The longest text frame a page may send.
const FRAME_MAX: usize = 1024 * 1024;

/// The sockets of the pages: by id, with the webview each is for.
#[derive(Default)]
pub(crate) struct Sockets {
    open: Mutex<HashMap<u64, (String, Held)>>,
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
    if let Some(local) = app.try_state::<Local>() {
        return open_local(&app, &local, &webview, &path);
    }
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let label = webview.label().to_string();
    let (account, key) = client.account_of(&label).ok_or("No account is signed in")?;
    let key = key.ok_or("The account is signed out")?;
    let id = client.sockets.next.fetch_add(1, Ordering::Relaxed) + 1;
    let sink_app = app.clone();
    let sink_label = label.clone();
    let sink_account = account.clone();
    let notifications = path.split_once('?').map_or(path.as_str(), |(bare, _)| bare) == "/notws";
    let sink = Box::new(move |event: Event| {
        let script = format!(
            "window.__moquiSocketEvent && window.__moquiSocketEvent({id}, {})",
            event_json(&event)
        );
        if let Some(window) = sink_app.get_webview_window(&sink_label) {
            let _ = window.eval(script);
        }
        if notifications {
            notified(&sink_app, &sink_label, &sink_account, &event);
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
    open.insert(id, (label, Held::Remote(socket)));
    Ok(id)
}

/// An event of a client's `/notws` for the store: a frame kept (`push.rs`), the heads marked stale when the socket
/// drops (noted for the mirror), and the mirror started when the shell's opens.
fn notified(app: &AppHandle, label: &str, account: &moqui_client::account::Account, event: &Event) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    match event {
        Event::Text(text) => {
            client.with_store(&account.id, |store| {
                super::push::keep_frame(store, &account.user_id, text)
            });
            // my message came back: its outbox operation is done
            if let Some(client_ref) = super::push::echo_ref(&account.user_id, text) {
                let removed = client.with_store(&account.id, |store| super::outbox::echoed(store, &client_ref));
                if removed == Some(true) {
                    super::outbox::notify_pages(app, &account.id, &serde_json::json!({ "kind": "changed" }));
                }
            }
        }
        Event::Closed | Event::Revoked => {
            // pushes may be missed until it is back: newer events are a gap until the mirror reads them again
            client.mirror.dropped(&account.id);
            mark_stale(&client, &account.id);
        }
        Event::Open => {
            // the network is back: what waits in the outbox goes
            client.flusher.wake();
            if label == "main" {
                super::mirror::start(app, &account.id);
            }
        }
    }
}

fn mark_stale(client: &Client, account: &str) {
    if let Some(Err(err)) = client.with_store(account, moqui_client::store::Store::mark_chat_heads_stale) {
        tracing::warn!("The chat heads of {account} were not marked stale: {err}");
    }
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
    let id = sockets.next.fetch_add(1, Ordering::Relaxed) + 1;
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
    let mut open = moqui_lib::sync::lock(&sockets.open);
    let socket = node.open_socket(path, &headers, sink).map_err(|err| err.to_string())?;
    open.insert(id, ("main".to_string(), Held::Local(socket)));
    Ok(id)
}

/// `work` on the sockets of this app's pages: a client's, or the node's in process (managed on the app).
fn with_sockets<T>(app: &AppHandle, work: impl FnOnce(&Sockets) -> T) -> Option<T> {
    if let Some(client) = app.try_state::<Client>() {
        return Some(work(&client.sockets));
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
    with_sockets(&app, |sockets| {
        let open = moqui_lib::sync::lock(&sockets.open);
        match open.get(&id) {
            Some((owner, socket)) if owner == webview.label() => match bytes {
                Some(bytes) => socket.send_binary(bytes),
                None => {
                    socket.send(text);
                    Ok(())
                }
            },
            _ => Err("No such socket".to_string()),
        }
    })
    .unwrap_or_else(|| Err("No sockets in this app".to_string()))
}

/// Close one of the page's sockets.
#[tauri::command]
pub(crate) fn socket_close(app: AppHandle, webview: Webview, id: u64) -> Result<(), String> {
    with_sockets(&app, |sockets| {
        let mut open = moqui_lib::sync::lock(&sockets.open);
        if open.get(&id).is_some_and(|(owner, _)| owner == webview.label()) {
            open.remove(&id);
        }
    });
    Ok(())
}
