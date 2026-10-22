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

//! The surface adapters' view of runs (plans/LLM-A2A-Assist.md 4.13, 4.14): after the stored events of a run commit
//! ([`super::events::EventSink::flush`]), the few that a surface acts on (a state change, the end, an approval or a
//! question asked or decided, a tool call) are handed to this node's observer worker, one thread per factory that takes
//! them in the order they were written. It gives them to the chat members ([`crate::llm::chat`]: the reply in the room,
//! the cards, the typing status) and to the triggers and the inbox ([`crate::llm::trigger`]: the breaker, the background
//! results and the approvals that wait). Only the node that wrote an event sees it, so it is acted on once; a remote
//! tail's copies never come here.
//!
//! Each batch runs in an execution context of its own with authorization off (the adapters act as the framework: they
//! post as an agent's chat user, count a trigger's failures). An adapter that fails is logged and the others go on. The
//! worker also ticks about every half second, which the chat members' typing status uses. [`settle`] waits for the
//! queue, for the tests.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::Value as Json;

use crate::context::ExecutionContextFactory;

/// The event types an adapter acts on.
pub const OBSERVED: &[&str] = &[
    "run.state",
    "run.done",
    "approval.requested",
    "approval.decided",
    "question.asked",
    "tool.call",
];

/// How often the worker ticks when no event comes.
const TICK: Duration = Duration::from_millis(500);

/// One committed batch of a run's events, in sequence order.
#[derive(Clone, Debug)]
pub struct RunEvents {
    pub run_id: String,
    /// `(eventSeq, type, payload)`.
    pub events: Vec<(i64, String, Json)>,
}

/// The factory's observer worker: its queue (the thread starts with the first batch) and how many batches wait or run.
#[derive(Default)]
pub struct Observers {
    queue: Mutex<Option<Sender<RunEvents>>>,
    pending: Arc<AtomicUsize>,
}

impl Observers {
    /// The queue, with the worker started on first use.
    fn sender(&self, factory: &Arc<ExecutionContextFactory>) -> Option<Sender<RunEvents>> {
        let mut queue = moqui_lib::sync::lock(&self.queue);
        if queue.is_none() {
            let (sender, receiver) = channel::<RunEvents>();
            let counted = Arc::clone(&self.pending);
            let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
            let spawned = std::thread::Builder::new()
                .name("agent-observers".to_string())
                .spawn(move || loop {
                    let next = receiver.recv_timeout(TICK);
                    let Some(factory) = weak.upgrade() else {
                        return;
                    };
                    match next {
                        Ok(batch) => {
                            dispatch(&factory, &batch);
                            counted.fetch_sub(1, Ordering::SeqCst);
                        }
                        Err(RecvTimeoutError::Timeout) => tick(&factory),
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                });
            match spawned {
                Ok(_) => *queue = Some(sender),
                Err(error) => {
                    tracing::error!(
                        "the agent observer worker could not start, so chat members and triggers hear nothing: {error}"
                    );
                    return None;
                }
            }
        }
        queue.clone()
    }
}

fn observers(factory: &ExecutionContextFactory) -> Arc<Observers> {
    factory.extension(|_| Observers::default())
}

/// Hand the observed events of a committed batch to the worker; the others are left out here, so a text chunk costs
/// nothing.
pub(crate) fn notify(factory: &Weak<ExecutionContextFactory>, run_id: &str, stored: &[(i64, String, Json)]) {
    let events: Vec<(i64, String, Json)> = stored
        .iter()
        .filter(|(_, event_type, _)| OBSERVED.contains(&event_type.as_str()))
        .cloned()
        .collect();
    if events.is_empty() {
        return;
    }
    let Some(factory) = factory.upgrade() else {
        return;
    };
    let workers = observers(&factory);
    let Some(sender) = workers.sender(&factory) else {
        return;
    };
    workers.pending.fetch_add(1, Ordering::SeqCst);
    let sent = sender.send(RunEvents {
        run_id: run_id.to_string(),
        events,
    });
    if sent.is_err() {
        workers.pending.fetch_sub(1, Ordering::SeqCst);
        tracing::error!(
            "the agent observer worker is gone, so the events of run {run_id} reach no chat room or trigger"
        );
    }
}

/// Wait up to `timeout` until no batch waits or runs; true when that came.
pub fn settle(factory: &ExecutionContextFactory, timeout: Duration) -> bool {
    let workers = observers(factory);
    let until = Instant::now() + timeout;
    while workers.pending.load(Ordering::SeqCst) > 0 {
        if Instant::now() >= until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// Run `body` in a fresh execution context of the factory with authorization off; a panic is logged.
fn in_context(factory: &Arc<ExecutionContextFactory>, what: &str, body: impl FnOnce()) {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        tracing::error!("the agent observer {what} panicked");
    }
}

fn dispatch(factory: &Arc<ExecutionContextFactory>, batch: &RunEvents) {
    in_context(factory, "chat", || crate::llm::chat::observe(factory, batch));
    in_context(factory, "trigger", || crate::llm::trigger::observe(factory, batch));
}

/// The tick: an execution context only when a typing status is due.
fn tick(factory: &Arc<ExecutionContextFactory>) {
    let due = crate::llm::chat::typing_due(factory);
    if !due.is_empty() {
        in_context(factory, "chat tick", || crate::llm::chat::send_typing(&due));
    }
}
