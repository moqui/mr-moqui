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
//! scheme cannot open `/notws` or `/callws` to its server, and could not send the key if it did. The host's page bridge
//! gives the page a socket-like object (`moquiDesktop.socket`, which `moqui.openSocket` uses) that names a path only,
//! and [`open_remote`] opens it on the server of the page's account ([`crate::socket`]). Frames go back to the page
//! through [`ClientHost::socket_event`].
//!
//! A socket is the page's: only its page may send on it or close it, and a page that loads again loses its sockets.
//! When the server closes one, the host lets it go and the page opens another, as it would a browser socket; a close
//! for authentication has the host check the key.
//!
//! A client's `/notws` frames are also written into the account's store (`sync::push`, MD4). A drop of any of the
//! account's `/notws` marks the timelines' heads stale, as pushes may be missed while it is down, and has the mirror
//! (`sync::mirror`) read them again when the shell's opens next; the shell's opening starts the mirror. An opening also
//! wakes the outbox's sender (`sync::flush`), and my own message pushed back takes its operation off
//! (`sync::outbox::echoed`). With a server at `clientApi` 4 the changes feeds (`sync::feeds`) are read when a `/notws`
//! opens (in the mirror's run, or in one of their own when the mirror does not start) and when a push's number shows a
//! gap.
//!
//! [`Sockets`] holds any [`PageSocket`], so a host may keep other sockets of its pages there too (the desktop app's
//! node in process).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{json, Value};

use crate::account::Account;
use crate::app::{Client, ClientHost};
use crate::socket::{Event, Socket};
use crate::sync::{feeds, mirror, outbox, push};

/// A socket held for a page.
pub trait PageSocket: Send {
    /// Send a text frame.
    fn send(&self, text: String);

    /// Send a binary frame; a client's sockets carry text only.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket carries text only.
    fn send_binary(&self, _bytes: Vec<u8>) -> Result<(), String> {
        Err("This socket carries text only".to_string())
    }
}

impl PageSocket for Socket {
    fn send(&self, text: String) {
        Socket::send(self, text);
    }
}

/// A socket held, with the page it is for.
type Held = (String, Box<dyn PageSocket>);

/// The sockets of the pages: by id, with the page each is for.
#[derive(Default)]
pub struct Sockets {
    open: Mutex<HashMap<u64, Held>>,
    next: AtomicU64,
}

impl Sockets {
    /// Open a socket for `page` with `start`, which is given its id; answers the id. The set is held while it starts:
    /// a connection that fails at once removes it only after it is in (`start` only starts its thread), so none is left
    /// reconnecting with nothing to drop it.
    ///
    /// # Errors
    ///
    /// Returns `start`'s error.
    pub fn open(
        &self,
        page: &str,
        start: impl FnOnce(u64) -> Result<Box<dyn PageSocket>, String>,
    ) -> Result<u64, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let mut open = moqui_lib::sync::lock(&self.open);
        let socket = start(id)?;
        open.insert(id, (page.to_string(), socket));
        Ok(id)
    }

    /// Send a frame on one of `page`'s sockets: `text`, or `binary`'s bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the page holds no such socket, or it does not carry the frame.
    pub fn send(&self, page: &str, id: u64, text: String, binary: Option<Vec<u8>>) -> Result<(), String> {
        let open = moqui_lib::sync::lock(&self.open);
        match open.get(&id) {
            Some((owner, socket)) if owner == page => match binary {
                Some(bytes) => socket.send_binary(bytes),
                None => {
                    socket.send(text);
                    Ok(())
                }
            },
            _ => Err("No such socket".to_string()),
        }
    }

    /// Close one of `page`'s sockets.
    pub fn close(&self, page: &str, id: u64) {
        let mut open = moqui_lib::sync::lock(&self.open);
        if open.get(&id).is_some_and(|(owner, _)| owner == page) {
            open.remove(&id);
        }
    }

    /// Close the sockets of a page: it is loading again, or it closed.
    pub fn close_for(&self, page: &str) {
        moqui_lib::sync::lock(&self.open).retain(|_, (owner, _)| owner != page);
    }

    /// Close every socket: the account changed.
    pub fn close_all(&self) {
        moqui_lib::sync::lock(&self.open).clear();
    }

    /// Let a socket go: it closed. For the host's own sockets' events only: it has no page check, so it is never
    /// reached from a page's command (a page closes its own with [`Self::close`]).
    pub fn remove(&self, id: u64) {
        moqui_lib::sync::lock(&self.open).remove(&id);
    }

    /// Have the pages of `account` open their sockets again, with its new key: each hears its socket close, as after a
    /// drop, and opens another. Answers how many.
    pub fn reopen_account(&self, client: &Client, host: &dyn ClientHost, account: &str) -> usize {
        let ids: Vec<(u64, String)> = moqui_lib::sync::lock(&self.open)
            .iter()
            .map(|(id, (page, _))| (*id, page.clone()))
            .collect();
        let mut count = 0;
        for (id, page) in ids {
            if client
                .account_of(host, &page)
                .is_none_or(|(shown, _)| shown.id != account)
            {
                continue;
            }
            self.remove(id);
            host.socket_event(&page, id, &event_json(&Event::Closed));
            count += 1;
        }
        count
    }
}

/// What the page's `__moquiSocketEvent` takes for an event: `{kind}` and the text of a frame.
pub fn event_json(event: &Event) -> Value {
    match event {
        Event::Open => json!({ "kind": "open" }),
        Event::Text(text) => json!({ "kind": "text", "text": text }),
        Event::Closed => json!({ "kind": "closed" }),
        Event::Revoked => json!({ "kind": "revoked" }),
    }
}

/// Open `path` (`/notws` or `/callws`, with a query) on the server of `page`'s account; answers the socket's id. It
/// reads the key store, so off the host's main thread.
///
/// # Errors
///
/// Returns an error when the page shows no account, the account is signed out, or the socket does not start.
pub fn open_remote(client: &Arc<Client>, host: &Arc<dyn ClientHost>, page: &str, path: &str) -> Result<u64, String> {
    if client.is_stopped() {
        return Err("The client is stopped".to_string());
    }
    let (account, key) = client
        .account_of(host.as_ref(), page)
        .ok_or("No account is signed in")?;
    let key = key.ok_or("The account is signed out")?;
    let notifications = path.split_once('?').map_or(path, |(bare, _)| bare) == "/notws";
    client.sockets.open(page, |id| {
        // weakly: the socket is in the client's own set, and must not keep the client after a shutdown
        let sink_client: Weak<Client> = Arc::downgrade(client);
        let sink_host = Arc::clone(host);
        let sink_page = page.to_string();
        let sink_account = account.clone();
        let sink = Box::new(move |event: Event| {
            let Some(sink_client) = sink_client.upgrade() else {
                return;
            };
            sink_host.socket_event(&sink_page, id, &event_json(&event));
            if notifications {
                notified(&sink_client, &sink_host, &sink_page, &sink_account, &event);
            }
            match event {
                // the page opens another, as it would after a browser socket closed
                Event::Closed => sink_client.sockets.remove(id),
                Event::Revoked => {
                    sink_client.sockets.remove(id);
                    sink_host.check_key(&sink_account);
                }
                Event::Open | Event::Text(_) => {}
            }
        });
        let socket = crate::socket::open(&account.server, &key, path, sink).map_err(|err| err.to_string())?;
        Ok(Box::new(socket) as Box<dyn PageSocket>)
    })
}

/// An event of a client's `/notws` for the store: a frame kept (`push`), the heads marked stale when the socket drops
/// (noted for the mirror), and the mirror started when the shell's opens.
fn notified(client: &Arc<Client>, host: &Arc<dyn ClientHost>, page: &str, account: &Account, event: &Event) {
    match event {
        Event::Text(text) => {
            let gap = client.with_store(&account.id, |store| push::keep_frame(store, &account.user_id, text));
            if gap == Some(true) {
                feeds::start(client, host, &account.id);
            }
            // my message came back: its outbox operation is done
            if let Some(client_ref) = push::echo_ref(&account.user_id, text) {
                let removed = client.with_store(&account.id, |store| outbox::echoed(store, &client_ref));
                if removed == Some(true) {
                    host.notify_pages(&account.id, &json!({ "kind": "changed" }));
                }
            }
        }
        Event::Closed | Event::Revoked => {
            // pushes may be missed until it is back: newer events are a gap until the mirror reads them again
            client.mirror.dropped(&account.id);
            mark_stale(client, &account.id);
        }
        Event::Open => {
            // the network is back: what waits in the outbox goes
            client.flusher.wake();
            // pushes may have been missed while it was down: the feeds catch up, in the mirror's run when one starts
            if !(host.is_shell(page) && mirror::start(client, host, &account.id)) {
                feeds::start(client, host, &account.id);
            }
        }
    }
}

fn mark_stale(client: &Client, account: &str) {
    if let Some(Err(err)) = client.with_store(account, crate::store::Store::mark_chat_heads_stale) {
        tracing::warn!("The chat heads of {account} were not marked stale: {err}");
    }
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
