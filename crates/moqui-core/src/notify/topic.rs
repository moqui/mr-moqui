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

//! In-process topic bus. Listeners run on the publishing thread. This process only.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

use crate::context::Error;

/// Cache invalidation topic. Cluster fan-out is [`crate::cluster::publish`], not a subscriber of this bus.
pub const ENTITY_CACHE_INVALIDATE: &str = "entity-cache-invalidate";

/// Notification topic. A repeated `message_id` is delivered once until it ages out of the recent set.
pub const NOTIFICATION_MESSAGE: &str = "notification-message";

/// A co-edited document's update or presence from another server (`crate::collab`), for that server's editors.
pub const COLLAB_UPDATE: &str = "collab-update";

const RECENT_IDS: usize = 1024;

/// A published message. `body` is the topic payload. `origin_id` is this process.
#[derive(Clone, Debug)]
pub struct TopicMessage {
    pub origin_id: String,
    pub message_id: String,
    pub body: serde_json::Value,
}

/// Called on the publishing thread.
pub type Listener = Arc<dyn Fn(&TopicMessage) + Send + Sync>;

/// Removes the listener when dropped.
#[must_use = "dropping the subscription removes the listener"]
pub struct Subscription {
    topic: String,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut bus = lock();
        if let Some(slots) = bus.topics.get_mut(&self.topic) {
            slots.retain(|slot| slot.id != self.id);
        }
    }
}

#[derive(Clone)]
struct Slot {
    id: u64,
    listener: Listener,
}

struct Bus {
    topics: HashMap<String, Vec<Slot>>,
    recent: VecDeque<String>,
    seen: HashSet<String>,
    next_id: u64,
}

impl Bus {
    fn new() -> Self {
        let mut bus = Self {
            topics: HashMap::new(),
            recent: VecDeque::new(),
            seen: HashSet::new(),
            next_id: 1,
        };
        bus.ensure();
        bus
    }

    fn ensure(&mut self) {
        self.topics.entry(ENTITY_CACHE_INVALIDATE.to_string()).or_default();
        self.topics.entry(NOTIFICATION_MESSAGE.to_string()).or_default();
        self.topics.entry(COLLAB_UPDATE.to_string()).or_default();
    }

    /// True the first time `id` is seen. A later publish of the same id is dropped until it ages out.
    fn remember(&mut self, id: &str) -> bool {
        if !self.seen.insert(id.to_string()) {
            return false;
        }
        self.recent.push_back(id.to_string());
        while self.recent.len() > RECENT_IDS {
            if let Some(old) = self.recent.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }
}

fn lock() -> std::sync::MutexGuard<'static, Bus> {
    static BUS: OnceLock<Mutex<Bus>> = OnceLock::new();
    BUS.get_or_init(|| Mutex::new(Bus::new()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

/// This process's origin id, stable for the life of the process.
pub fn origin_id() -> &'static str {
    static ORIGIN: OnceLock<String> = OnceLock::new();
    ORIGIN
        .get_or_init(|| crate::entity::uuid_v4().unwrap_or_else(|_| "local".to_string()))
        .as_str()
}

/// Create the two topic maps. Safe to call more than once.
pub fn install() {
    lock().ensure();
}

/// Subscribe to `name`. Only [`ENTITY_CACHE_INVALIDATE`], [`NOTIFICATION_MESSAGE`] and [`COLLAB_UPDATE`] exist.
///
/// # Errors
///
/// Returns an error when `name` is not one of those topics.
pub fn subscribe(name: &str, listener: Listener) -> Result<Subscription, Error> {
    let mut bus = lock();
    bus.ensure();
    let id = bus.next_id;
    bus.next_id = bus.next_id.saturating_add(1);
    let slots = bus
        .topics
        .get_mut(name)
        .ok_or_else(|| Error::new(format!("unknown topic {name}")))?;
    slots.push(Slot { id, listener });
    Ok(Subscription {
        topic: name.to_string(),
        id,
    })
}

/// Whether anything listens to `name` in this process: the host's notification socket subscribes to
/// [`NOTIFICATION_MESSAGE`] when it starts, so a command such as `moqui load` has no listener.
pub fn has_listeners(name: &str) -> bool {
    lock().topics.get(name).is_some_and(|slots| !slots.is_empty())
}

/// Deliver `message` to the topic's listeners on this thread, then return.
///
/// A second publish of the same `message_id` on [`NOTIFICATION_MESSAGE`] does not call listeners.
///
/// # Errors
///
/// Returns an error when `name` is not a known topic.
pub fn publish(name: &str, message: &TopicMessage) -> Result<(), Error> {
    let listeners = {
        let mut bus = lock();
        bus.ensure();
        if !bus.topics.contains_key(name) {
            return Err(Error::new(format!("unknown topic {name}")));
        }
        if name == NOTIFICATION_MESSAGE && !message.message_id.is_empty() && !bus.remember(&message.message_id) {
            return Ok(());
        }
        bus.topics.get(name).cloned().unwrap_or_default()
    };
    for slot in listeners {
        (slot.listener)(message);
    }
    Ok(())
}
