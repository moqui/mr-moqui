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

//! This node's XMPP sessions by user (docs/interfaces/XMPP.md "Sessions").
//!
//! - **Chat pushes.** The host subscribes to the notification bus as `/notws` does and hands each chat push here
//!   ([`Registry::deliver_notice`]): the same `chatRoom:`, `chatUser:` and `userStatus` messages the web client gets,
//!   to every session of the users the push names. So web and XMPP see the same events, from one source.
//! - **Between sessions.** A stanza to another local user's full JID (Jingle, a PEP notification) goes to that session.
//! - **Resumption.** A session whose connection dropped with stream management on waits, detached, for
//!   [`crate::Config::resume_timeout`]; a new connection takes it over by its id ([`Registry::take_detached`]).
//!
//! Sessions are per node: in a cluster, chat reaches every node through the bus, but a stanza between two XMPP sessions
//! reaches only one on the same node.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use minidom::Element;
use moqui_lib::sync::lock;
use serde_json::Value as Json;
use tokio::sync::{mpsc, oneshot, Notify};

/// How many pushes may wait for one session before it is closed as too slow (the client resumes or reconnects).
pub const QUEUE: usize = 512;
/// How many detached sessions one user may have waiting for resumption; a session dropped past this ends.
pub const DETACHED_PER_USER: usize = 10;

/// What reaches a session from outside its connection.
#[derive(Debug)]
pub enum Inbound {
    /// A chat push from the notification bus: its topic and its `message`.
    Notice { topic: String, message: Json },
    /// A stanza from another session of this node, already addressed.
    Stanza(Element),
}

/// One session, as others see it.
#[derive(Clone)]
pub struct Entry {
    pub id: u64,
    pub user_id: String,
    pub username: String,
    pub resource: String,
    pub tx: mpsc::Sender<Inbound>,
    /// Set when the queue overflowed, so the session's loop closes it.
    pub overflow: Arc<Notify>,
    /// The PEP nodes it wants notifications of (`<node>+notify` in its capabilities, XEP-0163 §4).
    pub notify: Arc<Mutex<HashSet<String>>>,
    /// It sent available presence (a client that did not is not sent messages to its bare JID's resources).
    pub available: Arc<AtomicBool>,
    pub carbons: Arc<AtomicBool>,
}

/// A detached session's hand-over: send it a channel and it sends the session back.
pub type Handover = oneshot::Sender<oneshot::Sender<Box<crate::session::Session>>>;

/// The attached sessions: by id, and the ids of each user's, so a stanza or a push to a user looks at that user's
/// sessions only, not every session of the node (plans/archive/High-Level-Review.md HL-009). Both change together under the
/// one lock.
#[derive(Default)]
struct Sessions {
    by_id: HashMap<u64, Entry>,
    by_user: HashMap<String, Vec<u64>>,
}

impl Sessions {
    fn of_user<'a>(&'a self, user_id: &str) -> impl Iterator<Item = &'a Entry> + 'a {
        self.by_user
            .get(user_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.by_id.get(id))
    }
}

#[derive(Default)]
pub struct Registry {
    sessions: Mutex<Sessions>,
    detached: Mutex<HashMap<String, (String, Handover)>>,
    next: AtomicU64,
}

impl Registry {
    pub fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn register(&self, entry: Entry) {
        let mut sessions = lock(&self.sessions);
        // a session registered again (the same id) is indexed once
        if let Some(old) = sessions.by_id.remove(&entry.id) {
            if let Some(ids) = sessions.by_user.get_mut(&old.user_id) {
                ids.retain(|known| *known != old.id);
            }
        }
        sessions
            .by_user
            .entry(entry.user_id.clone())
            .or_default()
            .push(entry.id);
        sessions.by_id.insert(entry.id, entry);
    }

    pub fn unregister(&self, id: u64) {
        let mut sessions = lock(&self.sessions);
        let Some(entry) = sessions.by_id.remove(&id) else {
            return;
        };
        if let Some(ids) = sessions.by_user.get_mut(&entry.user_id) {
            ids.retain(|known| *known != id);
            if ids.is_empty() {
                sessions.by_user.remove(&entry.user_id);
            }
        }
    }

    pub fn sessions_of(&self, user_id: &str) -> Vec<Entry> {
        lock(&self.sessions).of_user(user_id).cloned().collect()
    }

    pub fn has_user(&self, user_id: &str) -> bool {
        lock(&self.sessions).by_user.contains_key(user_id)
    }

    /// The session of a user's full JID resource.
    pub fn session(&self, user_id: &str, resource: &str) -> Option<Entry> {
        lock(&self.sessions)
            .of_user(user_id)
            .find(|entry| entry.resource == resource)
            .cloned()
    }

    /// The sessions of the users the push names (`userIds`), each sent its topic and message.
    pub fn deliver_notice(&self, body: &Json) {
        let Some(topic) = body.get("topic").and_then(Json::as_str) else {
            return;
        };
        if !(topic.starts_with("chatRoom:") || topic.starts_with("chatUser:") || topic == "userStatus") {
            return;
        }
        let users: HashSet<&str> = body
            .get("userIds")
            .and_then(Json::as_array)
            .map(|users| users.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        if users.is_empty() {
            return;
        }
        let message = body.get("message").cloned().unwrap_or(Json::Null);
        let targets: Vec<Entry> = {
            let sessions = lock(&self.sessions);
            users
                .iter()
                .flat_map(|user_id| sessions.of_user(user_id).cloned())
                .collect()
        };
        for entry in targets {
            self.send(
                &entry,
                Inbound::Notice {
                    topic: topic.to_string(),
                    message: message.clone(),
                },
            );
        }
    }

    /// Send to one session; a session whose queue is full is told to close.
    pub fn send(&self, entry: &Entry, inbound: Inbound) -> bool {
        match entry.tx.try_send(inbound) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                entry.overflow.notify_one();
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Keep a detached session for its user under its stream management id; false when the user has
    /// [`DETACHED_PER_USER`] waiting already, and this one is not kept.
    pub fn detach(&self, sm_id: &str, user_id: &str, handover: Handover) -> bool {
        let mut detached = lock(&self.detached);
        if detached.values().filter(|(owner, _)| owner == user_id).count() >= DETACHED_PER_USER {
            return false;
        }
        detached.insert(sm_id.to_string(), (user_id.to_string(), handover));
        true
    }

    pub fn forget_detached(&self, sm_id: &str) {
        lock(&self.detached).remove(sm_id);
    }

    /// The detached session `sm_id` of `user_id`, taken over; None when there is none (it timed out, or is another's).
    pub async fn take_detached(&self, sm_id: &str, user_id: &str) -> Option<Box<crate::session::Session>> {
        let handover = {
            let mut detached = lock(&self.detached);
            match detached.get(sm_id) {
                Some((owner, _)) if owner == user_id => detached.remove(sm_id).map(|(_, handover)| handover),
                _ => None,
            }
        }?;
        let (tx, rx) = oneshot::channel();
        handover.send(tx).ok()?;
        rx.await.ok()
    }

    /// Whether any session, attached or detached, has this user (for presence when the last one goes).
    pub fn has_detached_for(&self, user_id: &str) -> bool {
        lock(&self.detached).values().any(|(owner, _)| owner == user_id)
    }
}

impl Entry {
    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    pub fn has_carbons(&self) -> bool {
        self.carbons.load(Ordering::Relaxed)
    }
}

/// New shared flags for an entry.
pub fn flags() -> (Arc<AtomicBool>, Arc<AtomicBool>) {
    (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)))
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
