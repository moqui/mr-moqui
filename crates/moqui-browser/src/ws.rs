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

//! WebSocket frames between the worker's sockets (`js/moqui-ws.js`) and this module.
//!
//! The sockets live in JavaScript. A frame never enters WASM from a socket callback: the worker
//! queues it and delivers it between requests, so no call suspends for the network.
//!
//! - Inbound text: [`deliver_text`]. A `/notws` notification is published on the local
//!   [`NOTIFICATION_MESSAGE`] topic, as the server's own publish would be.
//! - Inbound binary: [`deliver_binary`] keeps the bytes behind a handle for the envelope reader
//!   (`plans/proposals/Messaging.md`), which takes them with [`take_binary`].
//! - Outbound: [`send_text`] and [`send_bytes`] queue a frame; the worker drains the queue after
//!   each request ([`take_outbox`]) and writes it to the named socket.
//!
//! A connection is a name the worker chose (`notws` for notifications). State is per thread; the
//! browser module has one.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

use moqui_core::notify::{publish, TopicMessage, NOTIFICATION_MESSAGE, ROUTE_USER_IDS};
use moqui_lib::error::Error;
use serde_json::Value as Json;

/// Outbound frames waiting for the worker. A full queue refuses the send.
pub const OUTBOX_FRAMES: usize = 256;
/// Inbound binary frames kept for a reader. Past this the oldest is dropped.
pub const BINARY_FRAMES: usize = 256;
/// Inbound binary bytes kept for a reader. Past this the oldest frames are dropped.
pub const BINARY_BYTES: usize = 16 * 1024 * 1024;

/// One frame for a socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outbound {
    Text { conn: String, text: String },
    Binary { conn: String, bytes: Vec<u8> },
}

/// One inbound binary frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboundBinary {
    pub conn: String,
    pub bytes: Vec<u8>,
}

/// What an inbound text frame was.
#[derive(Clone, Debug, PartialEq)]
pub enum Delivered {
    /// A notification, published locally. The body is what the server sent.
    Notification(Json),
    /// Any other text. Nothing in this module reads it.
    Text(String),
}

#[derive(Default)]
struct State {
    outbox: VecDeque<Outbound>,
    next_handle: u32,
    binary: BTreeMap<u32, InboundBinary>,
    binary_bytes: usize,
    dropped_binary: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Handle one inbound text frame from `conn`.
///
/// A JSON object with a `topic` or `notificationMessageId` is a notification. It is published on
/// [`NOTIFICATION_MESSAGE`] with its `notificationMessageId` as the message id, so the bus delivers
/// a repeated id once. Local listeners run on this thread, inside the caller's request.
///
/// # Errors
///
/// Returns the topic bus error.
pub fn deliver_text(conn: &str, text: &str) -> Result<Delivered, Error> {
    let Ok(Json::Object(mut body)) = serde_json::from_str::<Json>(text) else {
        return Ok(Delivered::Text(text.to_string()));
    };
    if !body.contains_key("topic") && !body.contains_key("notificationMessageId") {
        return Ok(Delivered::Text(text.to_string()));
    }
    // Routing is the server's; the server already strips it, and a stale one must not leak.
    body.remove(ROUTE_USER_IDS);
    let body = Json::Object(body);
    let message_id = body
        .get("notificationMessageId")
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string();
    publish(
        NOTIFICATION_MESSAGE,
        &TopicMessage {
            origin_id: format!("ws:{conn}"),
            message_id,
            body: body.clone(),
        },
    )
    .map_err(|err| Error::new(err.to_string()))?;
    Ok(Delivered::Notification(body))
}

/// Keep one inbound binary frame from `conn` and return its handle. Handle 0 is never issued.
pub fn deliver_binary(conn: &str, bytes: Vec<u8>) -> u32 {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.next_handle = state.next_handle.wrapping_add(1).max(1);
        let handle = state.next_handle;
        state.binary_bytes += bytes.len();
        state.binary.insert(
            handle,
            InboundBinary {
                conn: conn.to_string(),
                bytes,
            },
        );
        while state.binary.len() > BINARY_FRAMES || state.binary_bytes > BINARY_BYTES {
            let Some((_, oldest)) = state.binary.pop_first() else {
                break;
            };
            state.binary_bytes -= oldest.bytes.len();
            state.dropped_binary += 1;
            tracing::warn!(conn = %oldest.conn, "inbound binary frame dropped: no reader took it");
        }
        handle
    })
}

/// Take the binary frame behind `handle`.
pub fn take_binary(handle: u32) -> Option<InboundBinary> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let frame = state.binary.remove(&handle)?;
        state.binary_bytes -= frame.bytes.len();
        Some(frame)
    })
}

/// Handles of the binary frames no reader has taken, oldest first.
pub fn pending_binary() -> Vec<u32> {
    STATE.with(|state| state.borrow().binary.keys().copied().collect())
}

/// Binary frames dropped because no reader took them.
pub fn dropped_binary() -> u64 {
    STATE.with(|state| state.borrow().dropped_binary)
}

/// Queue a text frame for the socket named `conn`.
///
/// # Errors
///
/// Returns an error when [`OUTBOX_FRAMES`] frames are already waiting.
pub fn send_text(conn: &str, text: impl Into<String>) -> Result<(), Error> {
    push(Outbound::Text {
        conn: conn.to_string(),
        text: text.into(),
    })
}

/// Queue a binary frame for the socket named `conn`.
///
/// # Errors
///
/// Returns an error when [`OUTBOX_FRAMES`] frames are already waiting.
pub fn send_bytes(conn: &str, bytes: Vec<u8>) -> Result<(), Error> {
    push(Outbound::Binary {
        conn: conn.to_string(),
        bytes,
    })
}

fn push(frame: Outbound) -> Result<(), Error> {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.outbox.len() >= OUTBOX_FRAMES {
            return Err(Error::new(format!(
                "{OUTBOX_FRAMES} websocket frames are already waiting to be sent"
            )));
        }
        state.outbox.push_back(frame);
        Ok(())
    })
}

/// Every queued outbound frame, in order. The worker calls this after each request.
pub fn take_outbox() -> Vec<Outbound> {
    STATE.with(|state| state.borrow_mut().outbox.drain(..).collect())
}

/// Outbound frames waiting.
pub fn outbox_len() -> usize {
    STATE.with(|state| state.borrow().outbox.len())
}

#[cfg(test)]
mod tests;
