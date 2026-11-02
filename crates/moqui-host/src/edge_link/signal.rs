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

//! Messages on cluster topics over the link (plans/Edge-Node.md §5.4, "Calls (WebRTC)", E7): while a link is up, each
//! end routes the messages `moqui_core::cluster::send_to` has for the node at the other end ([`route`]) into one
//! client-streaming call, `Link.Signal` up and `LocalLink.Signal` down ([`forward`]), so they arrive in the order they
//! were sent (an offer before its candidates) without a round trip each. The receiver ([`receive`]) publishes each on
//! its bus through `moqui_core::edge::signal::receive`, one after another.
//!
//! As the cluster's direct send: a bounded queue that refuses at once when full (`overloaded`), and one try; a call
//! the link lost is opened again for the next message, and what it was carrying is not sent again (signaling is
//! renegotiated, a control's node ends its calls when its heartbeat stops).

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
                            tracing::debug!("edge link signal: the call is behind; a message is dropped");
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

/// The other end's messages of one `Signal` call, each published here in order once `fenced` passes its generation.
/// A message that cannot be read or published is logged and skipped; the call goes on.
pub(crate) async fn receive(
    mut messages: tonic::Streaming<SignalMessage>,
    fenced: impl Fn(u64) -> Result<(), Status>,
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
        let published = tokio::task::spawn_blocking(move || {
            moqui_core::edge::signal::receive(&topic, &message).map_err(|err| (topic, err))
        })
        .await;
        match published {
            Ok(Ok(())) => {}
            Ok(Err((topic, err))) => tracing::warn!("edge link signal on {topic} not taken: {}", err.message()),
            Err(err) => tracing::error!("edge link signal: publishing a message failed: {err}"),
        }
    }
    Ok(SignalReply {})
}
