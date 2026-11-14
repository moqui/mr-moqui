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

//! Messages on cluster topics over the link (plans/archive/Edge-Node.md §5.4, "Calls (WebRTC)", E7): while a link is up, each
//! end routes the messages `moqui_core::cluster::send_to` has for the node at the other end ([`route`]) into one
//! client-streaming call, `Link.Signal` up and `LocalLink.Signal` down ([`forward`]), so they arrive in the order they
//! were sent (an offer before its candidates) without a round trip each. The receiver ([`receive`]) publishes each on
//! its bus through `moqui_core::edge::signal::receive`, one after another.
//!
//! As the cluster's direct send: a bounded queue that refuses at once when full (`overloaded`), and one try; a call
//! the link lost is opened again for the next message, and what it was carrying is not sent again (signaling is
//! renegotiated, a control's node ends its calls when its heartbeat stops).
//!
//! **Pools** (plans/Edge-Node2.md §4.4). On an edge node each member of a pairing has a route of its own while its
//! down connection lasts ([`member_route`]), named `<member>@<pairing>` ([`route_name`]; `@` is in no node id), so two
//! pairings' members of one id never meet. A frame a member sends names itself as its origin; the edge node takes it
//! as from that route ([`edge_takes`]), so its media driver's replies for the leg go to the member whose socket bound
//! it, and the route hands them down with the member's own id as their target.

use std::future::Future;
use std::pin::Pin;

use moqui_core::edge::signal::RouteGuard;
use moqui_core::notify::TopicMessage;
use tokio::sync::mpsc;
use tonic::Status;

use super::proto::{SignalMessage, SignalReply};

/// Messages waiting for the link, per link: as many as the cluster's direct sender holds.
const QUEUE: usize = 1_024;

/// The messages of one call, as the generated clients take them: what [`forward`] hands it, until it closes.
pub(crate) struct Messages(mpsc::Receiver<SignalMessage>);

impl futures_util::Stream for Messages {
    type Item = SignalMessage;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<SignalMessage>> {
        self.0.poll_recv(cx)
    }
}
/// A `Signal` call until it ends.
pub(crate) type Call = Pin<Box<dyn Future<Output = Result<(), Status>> + Send>>;

/// A route for the node `instance_id` at the other end of the link of `generation`, and the queue of what it was
/// handed, for [`forward`]. The route goes when the guard drops (with the link's tasks).
pub(crate) fn route(instance_id: &str, generation: u64) -> (RouteGuard, mpsc::Receiver<SignalMessage>) {
    let (tx, rx) = mpsc::channel(QUEUE);
    let sender: moqui_core::edge::signal::Sender = std::sync::Arc::new(move |topic: &str, message: &TopicMessage| {
        let message = SignalMessage {
            generation,
            topic: topic.to_string(),
            origin_id: message.origin_id.clone(),
            message_id: message.message_id.clone(),
            body_json: message.body.to_string(),
        };
        tx.try_send(message).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => "overloaded".to_string(),
            mpsc::error::TrySendError::Closed(_) => "the edge link is down".to_string(),
        })
    });
    (moqui_core::edge::signal::add_route(instance_id, sender), rx)
}

/// The route name of member `member` of pairing `node` on an edge node.
pub(crate) fn route_name(node: &str, member: &str) -> String {
    format!("{member}@{node}")
}

/// On an edge node, a route for member `member` of pairing `node` ([`route_name`]) over its down connection of
/// `generation`: each message handed to it goes down with its `target` the member's own id, which the member checks as
/// its own (`moqui_core::cluster::for_this_member`). The route goes when the guard drops, leaving the other members'.
pub(crate) fn member_route(node: &str, member: &str, generation: u64) -> (RouteGuard, mpsc::Receiver<SignalMessage>) {
    let (tx, rx) = mpsc::channel(QUEUE);
    let target = serde_json::Value::String(member.to_string());
    let sender: moqui_core::edge::signal::Sender = std::sync::Arc::new(move |topic: &str, message: &TopicMessage| {
        let mut body = message.body.clone();
        if let serde_json::Value::Object(fields) = &mut body {
            fields.insert("target".to_string(), target.clone());
        }
        let message = SignalMessage {
            generation,
            topic: topic.to_string(),
            origin_id: message.origin_id.clone(),
            message_id: message.message_id.clone(),
            body_json: body.to_string(),
        };
        tx.try_send(message).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => "overloaded".to_string(),
            mpsc::error::TrySendError::Closed(_) => "the edge link is down".to_string(),
        })
    });
    (
        moqui_core::edge::signal::add_route(&route_name(node, member), sender),
        rx,
    )
}

/// Send what the route queues, in order, on calls that `open` makes (the link's `Signal`), until the route's queue
/// closes. A call that ends is opened again for the next message.
pub(crate) async fn forward<F>(mut queue: mpsc::Receiver<SignalMessage>, mut open: F)
where
    F: FnMut(Messages) -> Call,
{
    loop {
        let Some(first) = queue.recv().await else { return };
        let (tx, rx) = mpsc::channel::<SignalMessage>(QUEUE);
        if tx.try_send(first).is_err() {
            continue;
        }
        let call = open(Messages(rx));
        tokio::pin!(call);
        loop {
            tokio::select! {
                next = queue.recv() => match next {
                    Some(message) => match tx.try_send(message) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            let dropped = DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            tracing::warn!(
                                "edge link signal: the link's call is {QUEUE} messages behind; a message is dropped \
                                 ({dropped} since start)"
                            );
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                    },
                    None => {
                        drop(tx);
                        let _ = call.await;
                        return;
                    }
                },
                ended = &mut call => {
                    if let Err(status) = ended {
                        tracing::debug!("edge link signal call ended: {}", status.message());
                    }
                    break;
                }
            }
        }
    }
}

/// Messages a link's call could not take, dropped after their route had answered Ok.
static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// What one end does with a message the other sent, on a blocking thread: an error is why it was not taken.
pub(crate) type Take = std::sync::Arc<dyn Fn(&str, &TopicMessage) -> Result<(), String> + Send + Sync>;

/// The kind of a message on a cluster topic (`kind` in its body).
fn kind(message: &TopicMessage) -> &str {
    message
        .body
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
}

/// On an edge node (dedicated or all-in-one), what member `member` of the local node `node` may send: its sockets'
/// frames (`frame`, whose replies go back to that member only, so `origin` must be `member`, taken as its route
/// [`route_name`]), the call services' media controls (`control`) and the SIP engine's (`sip`, E8). The rest (a reply,
/// a relay or room frame of a media node) a local node has no business sending. Each message taken is stamped with
/// `node` ([`moqui_core::edge::signal::LINK_NODE`]), so the media driver keys its call's room by node and call (E12) and
/// two databases' call ids never meet in one room; one that already names a node is refused. A web protection ban
/// (17C) is taken as from the node. A front node (`calls` false, plans/Edge-Node2.md §4.7) has no calls, so it takes a
/// ban and nothing else.
pub(crate) fn edge_takes(node: &str, member: &str, calls: bool) -> Take {
    let node = node.to_string();
    let (member, route) = (member.to_string(), route_name(&node, member));
    std::sync::Arc::new(move |topic: &str, message: &TopicMessage| {
        // a web protection ban the node made or lifted, which holds here for its hosts only (plans/Edge-Node2.md §4.3)
        if topic == moqui_core::notify::WEB_BAN {
            if !matches!(kind(message), "ban" | "lift") {
                return Err(format!(
                    "local node {node} sent a web-ban message of kind {:?}",
                    kind(message)
                ));
            }
            let message = stamped(&node, message)?;
            return moqui_core::edge::signal::receive(topic, &message).map_err(|err| err.message().to_string());
        }
        if topic != moqui_core::notify::RTC_SIGNAL || !calls {
            return Err(format!("local node {node} sent a message on {topic}"));
        }
        let routed;
        let message = match kind(message) {
            "frame" => {
                let origin = message.body.get("origin").and_then(serde_json::Value::as_str);
                if origin != Some(member.as_str()) {
                    return Err(format!(
                        "a frame from member {member} of local node {node} names another origin"
                    ));
                }
                routed = with_origin(message, &route);
                &routed
            }
            "control" | "sip" => message,
            other => return Err(format!("local node {node} sent a message of kind {other:?}")),
        };
        let message = stamped(&node, message)?;
        moqui_core::edge::signal::receive(topic, &message).map_err(|err| err.message().to_string())
    })
}

/// `message` with its `origin` (where the media driver sends a frame's replies) set to `origin`.
fn with_origin(message: &TopicMessage, origin: &str) -> TopicMessage {
    let mut body = message.body.clone();
    if let serde_json::Value::Object(fields) = &mut body {
        fields.insert("origin".to_string(), serde_json::Value::String(origin.to_string()));
    }
    TopicMessage {
        origin_id: message.origin_id.clone(),
        message_id: message.message_id.clone(),
        body,
    }
}

/// `message` with the sending node `node` stamped on its body; Err when the body already has the field.
fn stamped(node: &str, message: &TopicMessage) -> Result<TopicMessage, String> {
    let serde_json::Value::Object(body) = &message.body else {
        return Err(format!("local node {node} sent a message that is not an object"));
    };
    if body.contains_key(moqui_core::edge::signal::LINK_NODE) {
        return Err(format!(
            "local node {node} sent a message that already names a node ({})",
            moqui_core::edge::signal::LINK_NODE
        ));
    }
    let mut body = body.clone();
    body.insert(
        moqui_core::edge::signal::LINK_NODE.to_string(),
        serde_json::Value::String(node.to_string()),
    );
    Ok(TopicMessage {
        origin_id: message.origin_id.clone(),
        message_id: message.message_id.clone(),
        body: serde_json::Value::Object(body),
    })
}

/// On a local node, what its edge node `edge` may send: the replies of its media driver for this node's sockets, each
/// checked against the socket's bindings (`crate::notws::rtc::deliver_edge_reply`). Nothing else is taken.
pub(crate) fn local_takes(edge: &str) -> Take {
    let edge = edge.to_string();
    std::sync::Arc::new(move |topic: &str, message: &TopicMessage| {
        if topic != moqui_core::notify::RTC_SIGNAL || kind(message) != "reply" {
            return Err(format!(
                "the edge node sent a message of kind {:?} on {topic}",
                kind(message)
            ));
        }
        crate::notws::rtc::deliver_edge_reply(&edge, message)
    })
}

/// The other end's messages of one `Signal` call, each handed to `take` in order once `fenced` passes its
/// generation. A message that cannot be read or is not taken is logged and skipped; the call goes on.
pub(crate) async fn receive(
    mut messages: tonic::Streaming<SignalMessage>,
    fenced: impl Fn(u64) -> Result<(), Status>,
    take: Take,
) -> Result<SignalReply, Status> {
    while let Some(message) = messages.message().await? {
        fenced(message.generation)?;
        let body: serde_json::Value = match serde_json::from_str(&message.body_json) {
            Ok(body) => body,
            Err(err) => {
                tracing::debug!("edge link signal: a message on {} is not JSON: {err}", message.topic);
                continue;
            }
        };
        let topic = message.topic;
        let message = TopicMessage {
            origin_id: message.origin_id,
            message_id: message.message_id,
            body,
        };
        // the listeners run on the publishing thread, and may wait briefly for the media driver's queue
        let take = std::sync::Arc::clone(&take);
        let published = tokio::task::spawn_blocking(move || take(&topic, &message).map_err(|err| (topic, err))).await;
        match published {
            Ok(Ok(())) => {}
            Ok(Err((topic, err))) => tracing::warn!("edge link signal on {topic} not taken: {err}"),
            Err(err) => tracing::error!("edge link signal: publishing a message failed: {err}"),
        }
    }
    Ok(SignalReply {})
}

#[cfg(test)]
#[path = "signal_tests.rs"]
mod tests;
