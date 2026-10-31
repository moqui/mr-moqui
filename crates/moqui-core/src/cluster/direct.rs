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

//! A message to one member, not a broadcast (plans/Comms-Framework.md §6.3, "Cluster"): a call's signaling goes to the
//! node holding its media, and its replies come back to the node holding the socket. [`send_to`] posts once over the
//! same transport as [`super::publish`], from its own bounded queue and thread, so a burst of ICE candidates neither
//! waits behind cache invalidations nor is retried after it has gone stale. A full queue is refused at once, so the
//! caller can answer `overloaded`.
//!
//! The body names its target (`"target": instanceId`). The notify transport reaches every notify member with one
//! NOTIFY, so the receiver drops a message that is not for it; [`for_this_member`] is that check.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_lib::sync::{lock, wait_timeout};

use super::{is_live, linked_state, load_members, trim_base, window_millis, ClusterState, PostOutcome};
use crate::context::{Error, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::notify::{self, TopicMessage};

/// Messages waiting for the direct sender. Past this a send is refused.
pub const DIRECT_LIMIT: usize = 1_024;
/// A member's address is read again after this.
const MEMBER_URL_TTL: Duration = Duration::from_secs(10);
/// How long the direct sender waits for more before it ends.
const DIRECT_IDLE: Duration = Duration::from_secs(30);

/// The direct sends waiting, and whether the thread posting them is running.
#[derive(Default)]
pub(super) struct DirectQueue {
    messages: VecDeque<(String, &'static str, Vec<u8>)>,
    sending: bool,
    urls: HashMap<String, (String, Instant)>,
}

/// Sends `message` on `topic` to the member `instance_id` only: published on this process's bus when that is this
/// member (or the factory is in no cluster), else queued for one post to that member.
///
/// # Errors
///
/// When the member is not live, the message is too big, or the direct queue is full (`overloaded`).
pub fn send_to(
    facade: &EntityFacade,
    instance_id: &str,
    topic: &'static str,
    message: &TopicMessage,
) -> Result<(), Error> {
    if instance_id.is_empty() {
        return Err(Error::new("no member to send to"));
    }
    let Some(state) = linked_state(facade) else {
        return notify::publish(topic, message);
    };
    if state.instance_id() == instance_id {
        return notify::publish(topic, message);
    }
    let Some(factory) = lock(&state.factory).upgrade() else {
        return Err(Error::new("the cluster is stopped"));
    };
    let base = member_url(&state, &factory, facade, instance_id)?;
    let payload = super::payload_bytes(message)?;
    let mut queue = lock(&state.direct);
    if queue.messages.len() >= DIRECT_LIMIT {
        return Err(Error::new("overloaded"));
    }
    queue.messages.push_back((base, topic, payload));
    if queue.sending {
        state.direct_ready.notify_one();
        return Ok(());
    }
    let running = Arc::clone(&state);
    match std::thread::Builder::new()
        .name("moqui-cluster-direct".to_string())
        .spawn(move || direct_loop(&running))
    {
        Ok(_) => queue.sending = true,
        Err(err) => tracing::error!("cluster direct sender thread did not start: {err}"),
    }
    Ok(())
}

/// Whether a message sent with [`send_to`] is for this member: its body's `target` is this member's id. A factory in
/// no cluster takes every message.
pub fn for_this_member(factory: &ExecutionContextFactory, message: &TopicMessage) -> bool {
    let Some(target) = message.body.get("target").and_then(serde_json::Value::as_str) else {
        return false;
    };
    match super::instance_id(factory) {
        Some(own) => own == target,
        None => true,
    }
}

fn member_url(
    state: &ClusterState,
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    instance_id: &str,
) -> Result<String, Error> {
    if let Some((url, at)) = lock(&state.direct).urls.get(instance_id) {
        if at.elapsed() < MEMBER_URL_TTL {
            return Ok(url.clone());
        }
    }
    let now = super::db_now(facade);
    let window = window_millis(factory);
    let member = load_members(facade)?
        .into_iter()
        .find(|m| m.instance_id == instance_id && is_live(m.heartbeat, now, window))
        .ok_or_else(|| Error::new(format!("cluster member {instance_id} is not live")))?;
    let url = trim_base(&member.base_url);
    lock(&state.direct)
        .urls
        .insert(instance_id.to_string(), (url.clone(), Instant::now()));
    Ok(url)
}

fn direct_loop(state: &ClusterState) {
    loop {
        let batch: Vec<(String, &'static str, Vec<u8>)> = {
            let mut queue = lock(&state.direct);
            while queue.messages.is_empty() {
                let (next, waited) = wait_timeout(&state.direct_ready, queue, DIRECT_IDLE);
                queue = next;
                if waited.timed_out() && queue.messages.is_empty() {
                    queue.sending = false;
                    return;
                }
            }
            queue.messages.drain(..).collect()
        };
        let Some(sender) = state.sender() else {
            if !state.no_sender_logged.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!("cluster direct send has no sender");
            }
            continue;
        };
        for (base, topic, payload) in batch {
            // One try: a signaling frame that waited for a retry is stale, and the client renegotiates.
            let sent = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sender.post(&base, topic, &payload)));
            match sent {
                Ok(Ok(PostOutcome::Accepted)) => {}
                Ok(Ok(PostOutcome::Rejected(status))) => {
                    tracing::warn!(base_url = %base, topic, status, "cluster direct message rejected")
                }
                Ok(Err(err)) => tracing::warn!("cluster direct post to {base} failed for topic {topic}: {err}"),
                Err(_) => tracing::error!("cluster direct post to {base} panicked"),
            }
        }
    }
}
