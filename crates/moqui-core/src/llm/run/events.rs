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

//! The events of a run (plan 4.5). [`EventSink`] takes them as the run works:
//!
//! - **Stored** events (`run.state`, `message.start`, `text.chunk`, `part.done`, `tool.call`, `tool.result`, `usage`,
//!   `compaction`, `step.restart`, `question.asked`, `error`, `run.done` and the rest of the plan's table) are kept
//!   until [`EventSink::flush`], which writes them to `AgentRunEvent` in one short transaction, numbered from
//!   `AgentRun.lastEventSeq` read for update (as chat numbers `ChatRoom.lastEventSeq`), and puts them in the hub once
//!   committed.
//! - **Live** events (`text.delta`, `reasoning.delta`, `tool.args`, `canvas.delta`, `status`) go only to this node's
//!   [`RunHub`](super::hub::RunHub).
//!
//! Text is coalesced: the deltas go live at once, and a `text.chunk` is stored every 300 ms (the conf's
//! `event-text-flush-ms`) or 512 characters. Each text event carries `(messageId, part, offset)`, the offset in
//! characters of its text within the part, so a client that re-attaches appends only what it lacks.
//!
//! The low-rate signals (a run started, paused or ended) go after the commit as a non-stored notification of topic
//! `AgentRun` to the run's user, as chat's `push#Signal`, so every tab, the other nodes and the terminal hear of them.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use moqui_db::sql::Condition;
use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::hub::RunHub;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;

pub(crate) const RUN: &str = "moqui.agent.AgentRun";
pub(crate) const RUN_EVENT: &str = "moqui.agent.AgentRunEvent";

/// The notification topic of the run signals.
pub const SIGNAL_TOPIC: &str = "AgentRun";

/// A stored text chunk is written at this many characters, whatever the time.
pub const TEXT_FLUSH_CHARS: usize = 512;

/// The live-only event types.
pub const LIVE_TYPES: &[&str] = &["text.delta", "reasoning.delta", "tool.args", "canvas.delta", "status"];

/// A stored event as read back.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEvent {
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    pub payload: Json,
    pub date_millis: Option<i64>,
}

impl StoredEvent {
    pub fn of(row: &EntityValue) -> Self {
        Self {
            run_id: crate::llm::text_of(row, "runId").unwrap_or_default(),
            seq: crate::llm::int_of(row, "eventSeq").unwrap_or_default(),
            event_type: crate::llm::text_of(row, "eventType").unwrap_or_default(),
            payload: crate::llm::text_of(row, "payloadJson")
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or(Json::Null),
            date_millis: row.get("eventDate").and_then(crate::llm::millis_of),
        }
    }

    /// `{seq, type, payload}`, as the catch-up returns it.
    pub fn to_json(&self) -> Json {
        json!({"seq": self.seq, "type": self.event_type, "payload": self.payload})
    }
}

/// The stored events of a run after `after`, oldest first, at most `limit`.
///
/// # Errors
///
/// Returns an error when the rows cannot be read.
pub fn stored_after(facade: &EntityFacade, run_id: &str, after: i64, limit: i64) -> Result<Vec<StoredEvent>, Error> {
    let rows = facade
        .find(RUN_EVENT)?
        .eq("runId", Value::string(run_id))
        .condition(Condition::greater("eventSeq", Value::I64(after)))
        .order_by("eventSeq", false)
        .limit(limit)
        .disable_authz()
        .list()?;
    Ok(rows.values().iter().map(StoredEvent::of).collect())
}

/// Whether an event ends what a stream follows: the run is done (`run.done`, after the end's `run.state`), or waits
/// for its user.
pub fn ends_stream(event_type: &str, payload: &Json) -> bool {
    match event_type {
        "run.done" => true,
        "run.state" => payload
            .get("state")
            .and_then(Json::as_str)
            .and_then(super::RunState::from_wire)
            .is_some_and(super::RunState::is_paused),
        _ => false,
    }
}

struct Coalesce {
    message_id: String,
    part: u32,
    /// The offset of `text` in its part.
    offset: usize,
    text: String,
    started: Instant,
}

/// A run's event sink: what one step job (or one tool call on a thread of its own) emits.
pub struct EventSink {
    run_id: String,
    user_id: String,
    facade: Option<EntityFacade>,
    hub: Arc<RunHub>,
    factory: Weak<ExecutionContextFactory>,
    /// This node: a flush writes only while it holds the run's lease, or no one does.
    node: String,
    pending: Vec<(String, Json)>,
    signals: Vec<Json>,
    coalesce: Option<Coalesce>,
    /// Characters of text so far per (message, part).
    offsets: HashMap<(String, u32), usize>,
    flush_after: Duration,
    /// Kept to be absorbed by another sink, never flushed itself.
    detached: bool,
}

impl EventSink {
    pub fn new(factory: &Arc<ExecutionContextFactory>, facade: &EntityFacade, run_id: &str, user_id: &str) -> Self {
        let flush_ms = crate::llm::facade(factory).conf().limits.event_text_flush_ms;
        Self {
            run_id: run_id.to_string(),
            user_id: user_id.to_string(),
            facade: Some(facade.clone()),
            hub: super::engine(factory).hub(),
            factory: Arc::downgrade(factory),
            node: super::engine(factory).node().to_string(),
            pending: Vec::new(),
            signals: Vec::new(),
            coalesce: None,
            offsets: HashMap::new(),
            flush_after: Duration::from_millis(u64::from(flush_ms.max(1))),
            detached: false,
        }
    }

    /// A sink for a tool call on another thread: its live events go to the hub at once, its stored ones wait to be
    /// [`absorb`](Self::absorb)ed in model order.
    pub fn detached(&self) -> Self {
        Self {
            run_id: self.run_id.clone(),
            user_id: self.user_id.clone(),
            facade: None,
            hub: Arc::clone(&self.hub),
            factory: self.factory.clone(),
            node: self.node.clone(),
            pending: Vec::new(),
            signals: Vec::new(),
            coalesce: None,
            offsets: HashMap::new(),
            flush_after: self.flush_after,
            detached: true,
        }
    }

    pub fn absorb(&mut self, other: EventSink) {
        self.pending.extend(other.pending);
        self.signals.extend(other.signals);
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The node this sink writes for.
    pub fn node(&self) -> &str {
        &self.node
    }

    /// Drop what is pending: the run's lease went to another node, which writes its events now.
    pub(crate) fn discard(&mut self) {
        self.pending.clear();
        self.signals.clear();
        self.coalesce = None;
    }

    /// A stored event, written at the next flush.
    pub fn emit(&mut self, event_type: &str, payload: Json) {
        self.pending.push((event_type.to_string(), payload));
    }

    /// A live-only event.
    pub fn live(&mut self, event_type: &str, payload: Json) {
        self.hub.push_live(&self.run_id, event_type, payload);
    }

    /// A signal to the run's user's sockets, sent after the next flush commits.
    pub fn signal(&mut self, payload: Json) {
        self.signals.push(payload);
    }

    /// A delta of a message part's text: live at once, stored coalesced.
    pub fn text_delta(&mut self, message_id: &str, part: u32, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let key = (message_id.to_string(), part);
        let offset = *self.offsets.get(&key).unwrap_or(&0);
        self.offsets.insert(key, offset + delta.chars().count());
        self.live(
            "text.delta",
            json!({"messageId": message_id, "part": part, "offset": offset, "text": delta}),
        );
        let same = self
            .coalesce
            .as_ref()
            .is_some_and(|chunk| chunk.message_id == message_id && chunk.part == part);
        if !same {
            self.end_chunk();
            self.coalesce = Some(Coalesce {
                message_id: message_id.to_string(),
                part,
                offset,
                text: String::new(),
                started: Instant::now(),
            });
        }
        if let Some(chunk) = self.coalesce.as_mut() {
            chunk.text.push_str(delta);
            if chunk.text.chars().count() >= TEXT_FLUSH_CHARS || chunk.started.elapsed() >= self.flush_after {
                self.end_chunk();
                self.flush_logged();
            }
        }
    }

    /// The pending text as a stored `text.chunk`.
    pub fn end_chunk(&mut self) {
        if let Some(chunk) = self.coalesce.take().filter(|chunk| !chunk.text.is_empty()) {
            self.emit(
                "text.chunk",
                json!({"messageId": chunk.message_id, "part": chunk.part, "offset": chunk.offset, "text": chunk.text}),
            );
        }
    }

    /// A message part is complete: its last chunk and `part.done` with its length.
    pub fn part_done(&mut self, message_id: &str, part: u32) {
        self.end_chunk();
        let length = self.offsets.get(&(message_id.to_string(), part)).copied().unwrap_or(0);
        self.emit(
            "part.done",
            json!({"messageId": message_id, "part": part, "length": length}),
        );
    }

    /// The call starts again: the text streamed for the message so far is void.
    pub fn restart(&mut self, message_id: &str, attempt: u32, reason: &str) {
        self.coalesce = None;
        self.offsets.retain(|(id, _), _| id != message_id);
        self.emit(
            "step.restart",
            json!({"messageId": message_id, "attempt": attempt, "reason": reason}),
        );
        self.flush_logged();
    }

    /// Write the pending stored events. A failure is logged and they wait for the next flush.
    pub fn flush_logged(&mut self) {
        if let Err(error) = self.flush() {
            tracing::error!("the events of agent run {} could not be stored: {error}", self.run_id);
        }
    }

    /// Write the pending stored events in one transaction (or the one in place), numbered from the run's
    /// `lastEventSeq` read for update, then put them in the hub and send the signals once committed.
    ///
    /// # Errors
    ///
    /// Returns an error when they cannot be written; they stay pending.
    pub fn flush(&mut self) -> Result<(), Error> {
        if self.detached || (self.pending.is_empty() && self.signals.is_empty()) {
            return Ok(());
        }
        let Some(facade) = self.facade.clone() else {
            return Ok(());
        };
        let tx = facade.transactions_for(RUN_EVENT)?;
        let in_place = tx.in_transaction();
        let pending = std::mem::take(&mut self.pending);
        let written = tx.run(TransactionAttribute::UseOrBegin, None, || {
            write_events(&facade, &self.run_id, &self.node, &pending)
        });
        let stored = match written {
            Ok(stored) => stored,
            Err(error) => {
                self.pending = pending;
                return Err(error);
            }
        };
        let signals = std::mem::take(&mut self.signals);
        let hub = Arc::clone(&self.hub);
        let factory = self.factory.clone();
        let run_id = self.run_id.clone();
        let user_id = self.user_id.clone();
        let publish = move || {
            // the surface adapters (chat members, triggers) act on what committed, on this node only
            super::observe::notify(&factory, &run_id, &stored);
            hub.push_stored(&run_id, stored);
            for signal in signals {
                send_signal(&factory, &user_id, &signal);
            }
        };
        if in_place {
            let held = Arc::new(std::sync::Mutex::new(Some(publish)));
            let later = Arc::clone(&held);
            let registered = tx.on_commit(move || {
                if let Some(publish) = moqui_lib::sync::lock(&later).take() {
                    publish();
                }
            });
            if let Err(error) = registered {
                tracing::error!(
                    "could not wait for the commit to publish the events of agent run, publishing now: {error}"
                );
                if let Some(publish) = moqui_lib::sync::lock(&held).take() {
                    publish();
                }
            }
        } else {
            publish();
        }
        Ok(())
    }
}

/// Number and write events in the transaction in place.
fn write_events(
    facade: &EntityFacade,
    run_id: &str,
    node: &str,
    events: &[(String, Json)],
) -> Result<Vec<(i64, String, Json)>, Error> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let mut run = super::held(facade, run_id, node, super::Hold::LeaseOrNone)?;
    let mut seq = crate::llm::int_of(&run, "lastEventSeq").unwrap_or_default();
    let now = crate::user::stamp_value(jiff::Timestamp::now().as_millisecond());
    let mut stored = Vec::with_capacity(events.len());
    for (event_type, payload) in events {
        seq += 1;
        let mut row = facade.make_value(RUN_EVENT)?;
        row.set("runId", Value::string(run_id));
        row.set("eventSeq", Value::I64(seq));
        row.set("eventType", Value::string(event_type));
        row.set("payloadJson", Value::string(payload.to_string()));
        row.set("eventDate", now.clone());
        row.create(facade)?;
        stored.push((seq, event_type.clone(), payload.clone()));
    }
    run.set("lastEventSeq", Value::I64(seq));
    run.update(facade)?;
    Ok(stored)
}

/// Send one run signal to `user_id`'s sockets, not stored and with no alert. A failure is logged: a signal is a hint,
/// and a client reads the run's events again when it reconnects.
pub(crate) fn send_signal(factory: &Weak<ExecutionContextFactory>, user_id: &str, payload: &Json) {
    let message = match moqui_lib::json::from_json(payload) {
        Value::Map(map) => map,
        _ => indexmap::IndexMap::new(),
    };
    let send = |message: indexmap::IndexMap<String, Value>| {
        crate::notify::NotificationMessage::new()
            .topic(SIGNAL_TOPIC)
            .user_id(user_id)
            .message_map(message)
            .persist(false)
            .show_alert(false)
            .send()
    };
    let outcome = if ExecutionContext::with_current(|_| ()).is_some() {
        send(message)
    } else if let Some(factory) = factory.upgrade() {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        send(message)
    } else {
        return;
    };
    if let Err(error) = outcome {
        tracing::error!("the agent run signal to {user_id} could not be sent: {error}");
    }
}
