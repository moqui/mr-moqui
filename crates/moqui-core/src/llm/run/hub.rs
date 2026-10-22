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

//! The per-node live hub of run events (plan 4.5): a ring buffer per run of the events a stream sends, the live-only
//! ones (`text.delta`, `reasoning.delta`, `tool.args`, `canvas.delta`, `status`) and the stored ones once committed,
//! with their sequence numbers.
//!
//! A run this node holds the lease of feeds its ring as it works. For a run another node works, the remote tail
//! ([`RunHub::watch`] while a stream is open) reads the new `AgentRunEvent` rows of every watched remote run in one
//! query every 400 ms and puts them in the rings: chunkier, but complete, with no cluster fan-out. A stored event is
//! put in a ring once, by its sequence, whenever it comes (one the tail reads late still goes in), so a run that moves
//! between nodes while watched is not sent twice. A stream follows a ring with a [`Follower`]: its own place in the
//! ring and the last sequence it sent; a stored event past the next one brings the missing ones from the database
//! first, so a stream has no gap whatever the ring missed.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_db::sql::Condition;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use serde_json::Value as Json;

use crate::context::ExecutionContextFactory;

/// Events kept per run.
const RING: usize = 2048;
/// Runs kept; the least recently used ring goes first.
const RINGS: usize = 512;
/// How often the remote tail reads.
pub const TAIL_EVERY: Duration = Duration::from_millis(400);

/// One event as a stream sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct HubEvent {
    /// Its place in the ring, increasing.
    pub index: u64,
    /// The `eventSeq` of a stored event; `None` for a live one.
    pub seq: Option<i64>,
    pub event_type: String,
    pub payload: Json,
}

#[derive(Default)]
struct Ring {
    next: u64,
    events: VecDeque<HubEvent>,
    /// The highest stored sequence put in.
    last_seq: i64,
    /// The stored sequences the ring holds.
    seqs: std::collections::BTreeSet<i64>,
    touched: Option<Instant>,
}

#[derive(Default)]
struct State {
    rings: HashMap<String, Ring>,
    /// Open streams per run: each watch's id and the sequence it replayed through.
    watchers: HashMap<String, Vec<(u64, i64)>>,
    next_watch: u64,
    tail_started: bool,
}

/// The hub of one node.
#[derive(Default)]
pub struct RunHub {
    state: Mutex<State>,
    wake: Condvar,
}

impl RunHub {
    pub fn new() -> Self {
        Self::default()
    }

    fn ring<'a>(state: &'a mut State, run_id: &str) -> &'a mut Ring {
        if !state.rings.contains_key(run_id) && state.rings.len() >= RINGS {
            let oldest = state
                .rings
                .iter()
                .filter(|(id, _)| !state.watchers.contains_key(*id))
                .min_by_key(|(_, ring)| ring.touched)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                state.rings.remove(&oldest);
            }
        }
        let ring = state.rings.entry(run_id.to_string()).or_default();
        ring.touched = Some(Instant::now());
        ring
    }

    fn push(ring: &mut Ring, seq: Option<i64>, event_type: &str, payload: Json) {
        let index = ring.next;
        ring.next += 1;
        if let Some(seq) = seq {
            ring.last_seq = ring.last_seq.max(seq);
            ring.seqs.insert(seq);
        }
        ring.events.push_back(HubEvent {
            index,
            seq,
            event_type: event_type.to_string(),
            payload,
        });
        while ring.events.len() > RING {
            if let Some(Some(seq)) = ring.events.pop_front().map(|event| event.seq) {
                ring.seqs.remove(&seq);
            }
        }
    }

    /// A live-only event.
    pub fn push_live(&self, run_id: &str, event_type: &str, payload: Json) {
        {
            let mut state = lock(&self.state);
            Self::push(Self::ring(&mut state, run_id), None, event_type, payload);
        }
        self.wake.notify_all();
    }

    /// Stored events after their commit; one the ring already holds is left out, and one older than the ring's newest
    /// still goes in (a follower that sent past it leaves it out).
    pub fn push_stored(&self, run_id: &str, events: Vec<(i64, String, Json)>) {
        {
            let mut state = lock(&self.state);
            let ring = Self::ring(&mut state, run_id);
            for (seq, event_type, payload) in events {
                if !ring.seqs.contains(&seq) {
                    Self::push(ring, Some(seq), &event_type, payload);
                }
            }
        }
        self.wake.notify_all();
    }

    /// The highest stored sequence in the run's ring.
    pub fn last_seq(&self, run_id: &str) -> i64 {
        lock(&self.state).rings.get(run_id).map_or(0, |ring| ring.last_seq)
    }

    /// Where a stream that has sent the stored events through `seq` goes on from: after that event when the ring still
    /// has it, else from the ring's start (the stream leaves out the stored events it sent, and a client places text
    /// by its offset).
    pub fn cursor_after_seq(&self, run_id: &str, seq: i64) -> Option<u64> {
        let state = lock(&self.state);
        let ring = state.rings.get(run_id)?;
        ring.events
            .iter()
            .find(|event| event.seq == Some(seq))
            .map(|event| event.index)
    }

    /// The events after `cursor` (all the ring has with `None`), waiting up to `timeout` for one.
    pub fn wait(&self, run_id: &str, cursor: Option<u64>, timeout: Duration) -> Vec<HubEvent> {
        let deadline = Instant::now() + timeout;
        let mut state = lock(&self.state);
        loop {
            let found: Vec<HubEvent> = state
                .rings
                .get(run_id)
                .map(|ring| {
                    ring.events
                        .iter()
                        .filter(|event| cursor.is_none_or(|cursor| event.index > cursor))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let now = Instant::now();
            if !found.is_empty() || now >= deadline {
                return found;
            }
            state = self
                .wake
                .wait_timeout(state, deadline - now)
                .map(|(state, _)| state)
                .unwrap_or_else(|error| error.into_inner().0);
        }
    }

    /// Hold a watch on a run while a stream is open: the remote tail reads its stored events whenever this node does
    /// not work it, from the newest the ring has or the lowest sequence a watch replayed through, whichever is later.
    /// A watch changes nothing in the ring itself, so one stream's place never hides events from another. The tail
    /// starts with the first watch.
    pub fn watch(self: &Arc<Self>, factory: &Arc<ExecutionContextFactory>, run_id: &str, from_seq: i64) -> Watch {
        let (id, start_tail) = {
            let mut state = lock(&self.state);
            let id = state.next_watch;
            state.next_watch += 1;
            state
                .watchers
                .entry(run_id.to_string())
                .or_default()
                .push((id, from_seq));
            Self::ring(&mut state, run_id);
            (id, !std::mem::replace(&mut state.tail_started, true))
        };
        if start_tail {
            start_remote_tail(Arc::downgrade(self), Arc::downgrade(factory));
        }
        Watch {
            hub: Arc::clone(self),
            run_id: run_id.to_string(),
            id,
        }
    }

    /// Each watched run and the sequence the tail reads after.
    pub(crate) fn watched(&self) -> Vec<(String, i64)> {
        let state = lock(&self.state);
        state
            .watchers
            .iter()
            .map(|(run_id, watches)| {
                let lowest = watches.iter().map(|(_, from)| *from).min().unwrap_or(0);
                let newest = state.rings.get(run_id).map_or(0, |ring| ring.last_seq);
                (run_id.clone(), newest.max(lowest))
            })
            .collect()
    }
}

/// An open stream's watch on a run; dropped when the stream ends.
pub struct Watch {
    hub: Arc<RunHub>,
    run_id: String,
    id: u64,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut state = lock(&self.hub.state);
        if let Some(watches) = state.watchers.get_mut(&self.run_id) {
            watches.retain(|(id, _)| *id != self.id);
            if watches.is_empty() {
                state.watchers.remove(&self.run_id);
            }
        }
    }
}

/// One stream's place in a run's ring: the ring position it read through and the last stored sequence it sent.
pub struct Follower {
    run_id: String,
    cursor: Option<u64>,
    last: i64,
}

impl Follower {
    /// A follower that has sent the stored events through `last` (from its replay).
    pub fn new(hub: &RunHub, run_id: &str, last: i64) -> Self {
        Self {
            run_id: run_id.to_string(),
            cursor: hub.cursor_after_seq(run_id, last),
            last,
        }
    }

    /// The last stored sequence sent.
    pub fn last(&self) -> i64 {
        self.last
    }

    /// The events to send next, waiting up to `timeout` for the ring: live ones as they are, stored ones only past
    /// the last sent and in order. A stored event past the next sequence first brings the ones between from the
    /// database (the ring missed them: the run moved between nodes, or the ring dropped them).
    ///
    /// # Errors
    ///
    /// Returns an error when the missing events cannot be read.
    pub fn next(
        &mut self,
        hub: &RunHub,
        facade: &crate::entity::EntityFacade,
        timeout: Duration,
    ) -> Result<Vec<HubEvent>, moqui_lib::error::Error> {
        let mut out = Vec::new();
        for event in hub.wait(&self.run_id, self.cursor, timeout) {
            self.cursor = Some(event.index);
            let Some(seq) = event.seq else {
                out.push(event);
                continue;
            };
            if seq <= self.last {
                continue;
            }
            if seq > self.last + 1 {
                let missing = super::events::stored_after(facade, &self.run_id, self.last, seq - self.last - 1)?;
                for stored in missing.into_iter().filter(|stored| stored.seq < seq) {
                    self.last = stored.seq;
                    out.push(HubEvent {
                        index: event.index,
                        seq: Some(stored.seq),
                        event_type: stored.event_type,
                        payload: stored.payload,
                    });
                }
            }
            self.last = seq;
            out.push(event);
        }
        Ok(out)
    }
}

/// The remote tail's thread: every [`TAIL_EVERY`], one query for the stored events of every watched run this node
/// does not work. It ends with the factory or the hub. A query that fails is logged and tried at the next round.
fn start_remote_tail(hub: Weak<RunHub>, factory: Weak<ExecutionContextFactory>) {
    let spawned = std::thread::Builder::new()
        .name("moqui-agent-tail".to_string())
        .spawn(move || loop {
            std::thread::sleep(TAIL_EVERY);
            let (Some(hub), Some(factory)) = (hub.upgrade(), factory.upgrade()) else {
                return;
            };
            let local = super::engine(&factory).local_ids();
            let remote: Vec<(String, i64)> = hub
                .watched()
                .into_iter()
                .filter(|(run_id, _)| !local.contains(run_id))
                .collect();
            if remote.is_empty() {
                continue;
            }
            if let Err(error) = tail_once(&factory, &hub, &remote) {
                tracing::error!("the remote tail of agent run events could not read: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::error!("the remote tail of agent run events did not start: {error}");
    }
}

/// One read of the remote tail: the events of each run after the sequence its ring has.
pub(crate) fn tail_once(
    factory: &Arc<ExecutionContextFactory>,
    hub: &RunHub,
    runs: &[(String, i64)],
) -> Result<(), moqui_lib::error::Error> {
    let Some(facade) = factory.entity() else {
        return Ok(());
    };
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let condition = Condition::or(
        runs.iter()
            .map(|(run_id, after)| {
                Condition::and(vec![
                    Condition::equals("runId", Value::string(run_id)),
                    Condition::greater("eventSeq", Value::I64(*after)),
                ])
            })
            .collect(),
    );
    let rows = facade
        .find(super::events::RUN_EVENT)?
        .condition(condition)
        .order_by("runId", false)
        .order_by("eventSeq", false)
        .limit(2000)
        .list()?;
    let mut by_run: HashMap<String, Vec<(i64, String, Json)>> = HashMap::new();
    for row in rows.values() {
        let event = super::events::StoredEvent::of(row);
        by_run
            .entry(event.run_id.clone())
            .or_default()
            .push((event.seq, event.event_type, event.payload));
    }
    for (run_id, events) in by_run {
        hub.push_stored(&run_id, events);
    }
    Ok(())
}
