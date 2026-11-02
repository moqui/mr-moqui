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

//! A message for one node over the edge link (plans/Edge-Node.md §5.4, "Calls (WebRTC)", E7). An edge node and its
//! local node share no database, so neither is a cluster member the other can post to. While a link is up, its end
//! adds a route here for the node at the other end (the local node's dialer for its edge node, the edge node for each
//! linked local node), and [`crate::cluster::send_to`] hands a message for that node to the route instead of the
//! cluster's sender: the rtc-signal frames and replies of calls whose media is on the edge node, the call services'
//! media controls, and the SIP engine's controls (E8). The other end hands what it receives to [`receive`], which
//! publishes it on this process's bus for the topics it takes.
//!
//! A node that has had a route in this process is remembered: a message for it while its link is down is refused
//! (`the edge link to .. is down`), never published here or posted to the cluster, which it is no member of. The
//! registry is per process (as the bus is): two servers of one process (a test's) share it, keyed by instance id.
//! On a clustered local node only the member that holds the link has its edge node's route; a call's socket on
//! another member is refused the same way (plans/Edge-Node.md §5.4, "As built (E7)").
//!
//! As the cluster's direct send, a route is fire and forget, from a bounded queue: a full one is refused at once
//! (`overloaded`), and a message lost with the link is not sent again (the client renegotiates).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use moqui_lib::error::Error;
use moqui_lib::sync::lock;

use crate::notify::{TopicMessage, RTC_SIGNAL};

/// The topics a message over the link may be on: what [`receive`] publishes. Anything else is refused there.
pub const TOPICS: [&str; 1] = [RTC_SIGNAL];

/// Hands a message on a topic to the link, without waiting for it to be sent: Err when the link's queue is full
/// (`overloaded`) or the link has ended.
pub type Sender = Arc<dyn Fn(&str, &TopicMessage) -> Result<(), String> + Send + Sync>;

struct Route {
    token: u64,
    sender: Sender,
}

/// The nodes that have had a route in this process.
fn known() -> &'static Mutex<std::collections::HashSet<String>> {
    static KNOWN: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    KNOWN.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn routes() -> &'static Mutex<HashMap<String, Route>> {
    static ROUTES: std::sync::OnceLock<Mutex<HashMap<String, Route>>> = std::sync::OnceLock::new();
    ROUTES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many routes there are, so a send with none does not take the lock.
static COUNT: AtomicUsize = AtomicUsize::new(0);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// A route while it is held: dropping it removes the route, unless a newer one for the node replaced it.
#[must_use = "dropping the guard removes the route"]
pub struct RouteGuard {
    instance_id: String,
    token: u64,
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        let mut all = lock(routes());
        if all
            .get(&self.instance_id)
            .is_some_and(|route| route.token == self.token)
        {
            all.remove(&self.instance_id);
            COUNT.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Send messages for `instance_id` through `sender` until the guard drops. A route added later for the same node
/// (the next link) replaces this one, and this guard's drop then leaves it.
pub fn add_route(instance_id: &str, sender: Sender) -> RouteGuard {
    let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
    lock(known()).insert(instance_id.to_string());
    let replaced = lock(routes()).insert(instance_id.to_string(), Route { token, sender });
    if replaced.is_none() {
        COUNT.fetch_add(1, Ordering::AcqRel);
    }
    RouteGuard {
        instance_id: instance_id.to_string(),
        token,
    }
}

/// The route for `instance_id` while its link is up.
pub fn route(instance_id: &str) -> Option<Sender> {
    if COUNT.load(Ordering::Acquire) == 0 || instance_id.is_empty() {
        return None;
    }
    lock(routes()).get(instance_id).map(|route| Arc::clone(&route.sender))
}

/// Send `message` on `topic` to `instance_id` over its link, when a link to it is up; refused when it is a link's
/// node whose link is down now. `None` for any other node, which the caller sends to the usual way.
pub fn send(instance_id: &str, topic: &str, message: &TopicMessage) -> Option<Result<(), Error>> {
    if let Some(sender) = route(instance_id) {
        return Some(sender(topic, message).map_err(Error::new));
    }
    if instance_id.is_empty() || !lock(known()).contains(instance_id) {
        return None;
    }
    Some(Err(Error::new(format!("the edge link to {instance_id} is down"))))
}

/// A message the link's other end sent: published on this process's bus, on a topic of [`TOPICS`] only. The
/// listener (`notws/rtc.rs` in the host) checks that it is for this node, as it does a cluster message's.
pub fn receive(topic: &str, message: &TopicMessage) -> Result<(), Error> {
    if !TOPICS.contains(&topic) {
        return Err(Error::new(format!(
            "the edge link takes no messages on the topic {topic}"
        )));
    }
    crate::notify::publish(topic, message).map_err(|err| Error::new(err.message().to_string()))
}

#[cfg(test)]
#[path = "signal_tests.rs"]
mod tests;
