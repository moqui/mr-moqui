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

//! The replication sender of a pool (plans/Edge-Node2.md §4.4, "Work from the pool to the edge node"). Every member of
//! a local node's cluster links to the edge node, but replication's change notes are in memory in the member that
//! wrote, and the edge node keeps one cursor per set. So one member sends: the one that holds the lock row
//! `moqui.edge.EdgeSenderLock` `replicate`, claimed while its link is up and renewed while it holds it, by the
//! database's clock as the job lease is (`crate::service::job_lease`). A lease that lapsed is taken over by the next
//! linked member that asks. The same member keeps the link's own rows (`CallMediaNode`, `SipNode`) and runs the
//! minute's `refresh#PhoneFallbacks`.
//!
//! The other members pass their notes to it on the cluster topic [`crate::notify::EDGE_REPLICA_NOTE`]
//! ([`forward_note`], [`take_notes`]): every member sends every note it makes, the sender among them, so a note is not
//! lost while the lock moves. A member keeps notes only while it is the sender ([`super::Replica::set_keeping`]), and a
//! member that becomes the sender sends each set whole under a new epoch, as a restart does: the full send reads every
//! record after the lock was taken, and the notes that arrive after that are sent after it.
//!
//! The host's link runs the claim ([`claim`]); [`set_sender`] records the result for this process, and a member that
//! stops lets the lock go ([`super::disable`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use crate::context::ExecutionContextFactory;
use crate::entity::EntityFacade;
use crate::transaction::TransactionAttribute;
use crate::user::stamp_value;

pub const LOCK: &str = "moqui.edge.EdgeSenderLock";
/// The lock of the replication sender, which also keeps the link's own rows.
pub const REPLICATE: &str = "replicate";
/// The keys one note message carries at most; more go as their sets whole.
const MAX_NOTE_KEYS: usize = 500;
/// How long a member gathers its notes before it sends them to the others.
const NOTE_GATHER: Duration = Duration::from_millis(100);

/// How the sender lock is timed: a factory extension, [`timing`] gives the defaults, and a test inserts its own first.
#[derive(Clone, Debug)]
pub struct SenderTiming {
    /// How often a member that does not hold the lock asks for it.
    pub poll: Duration,
    /// How often the holder renews.
    pub renew: Duration,
    /// How long a claim or renewal lasts.
    pub lease: Duration,
}

impl Default for SenderTiming {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(3),
            renew: Duration::from_secs(10),
            lease: Duration::from_secs(30),
        }
    }
}

/// The factory's sender timing.
pub fn timing(factory: &ExecutionContextFactory) -> Arc<SenderTiming> {
    factory.extension(|_| SenderTiming::default())
}

/// What a [`claim`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// This member held the lock and renewed it; when it took it (its token).
    Renewed(u64),
    /// This member took the lock, as of its token (now): it was free, or its lease had lapsed.
    Taken(u64),
    /// Another member holds it.
    Held(String),
}

impl Claim {
    pub fn holds(&self) -> bool {
        self.token().is_some()
    }

    /// While this member holds the lock, when it took it (epoch milliseconds by the database's clock): every batch it
    /// sends carries it, and the edge node refuses an earlier one for a set a later sender wrote
    /// ([`super::Batch::sender_token`]).
    pub fn token(&self) -> Option<u64> {
        match self {
            Self::Renewed(token) | Self::Taken(token) => Some(*token),
            Self::Held(_) => None,
        }
    }
}

fn transaction<T>(facade: &EntityFacade, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let _authz = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::disable_authz);
    facade
        .transactions_for(LOCK)?
        .run(TransactionAttribute::ForceNew, Some(Duration::from_secs(30)), body)
}

/// Claim or renew the lock `name` for `instance_id` for `lease`: renewed when it holds it, taken when it is free or its
/// lease lapsed (the database's time), else who holds it. The row is made the first time.
///
/// # Errors
///
/// When the row cannot be read or written.
pub fn claim(facade: &EntityFacade, name: &str, instance_id: &str, lease: Duration) -> Result<Claim, Error> {
    if instance_id.is_empty() {
        return Err(Error::new("the sender lock needs this member's instance id"));
    }
    let now = crate::cluster::db_now(facade);
    let until = now.saturating_add(i64::try_from(lease.as_millis()).unwrap_or(i64::MAX));
    transaction(facade, || {
        let there = facade
            .find(LOCK)?
            .eq("lockName", Value::string(name))
            .use_cache(false)
            .disable_authz()
            .one()?;
        if there.is_none() {
            let mut row = facade.make_value(LOCK)?;
            row.set("lockName", Value::string(name));
            row.create_if_missing(facade)?;
        }
        // read for update, so a member that asks at the same time waits for this claim and then finds it held
        let mut row = facade
            .find(LOCK)?
            .eq("lockName", Value::string(name))
            .use_cache(false)
            .for_update(true)
            .disable_authz()
            .one()?
            .ok_or_else(|| Error::new(format!("the sender lock {name} could not be made")))?;
        let holder = row.get("instanceId").map(to_plain_string).unwrap_or_default();
        let lease = row.get("leaseExpireDate").and_then(crate::user::value_millis);
        let token = |millis: i64| u64::try_from(millis).unwrap_or(0);
        let claim = if holder == instance_id {
            match row.get("claimedDate").and_then(crate::user::value_millis) {
                Some(claimed) => Claim::Renewed(token(claimed)),
                None => {
                    row.set("claimedDate", stamp_value(now));
                    Claim::Renewed(token(now))
                }
            }
        } else if holder.is_empty() || lease.is_none_or(|lease| lease <= now) {
            row.set("instanceId", Value::string(instance_id));
            row.set("claimedDate", stamp_value(now));
            Claim::Taken(token(now))
        } else {
            return Ok(Claim::Held(holder));
        };
        row.set("leaseExpireDate", stamp_value(until));
        row.update(facade)?;
        Ok(claim)
    })
}

/// Let the lock `name` go when `instance_id` holds it, so another member takes it at its next claim. True when it did.
///
/// # Errors
///
/// When the row cannot be written.
pub fn release(facade: &EntityFacade, name: &str, instance_id: &str) -> Result<bool, Error> {
    transaction(facade, || {
        let row = facade
            .find(LOCK)?
            .eq("lockName", Value::string(name))
            .use_cache(false)
            .for_update(true)
            .disable_authz()
            .one()?;
        let Some(mut row) =
            row.filter(|row| row.get("instanceId").map(to_plain_string).as_deref() == Some(instance_id))
        else {
            return Ok(false);
        };
        row.set("instanceId", Value::Null);
        row.set("leaseExpireDate", Value::Null);
        row.update(facade)?;
        Ok(true)
    })
}

/// Who holds the lock `name` now and until when (epoch milliseconds), if anyone does; a lapsed lease is still named.
///
/// # Errors
///
/// When the row cannot be read.
pub fn holder(facade: &EntityFacade, name: &str) -> Result<Option<(String, Option<i64>)>, Error> {
    transaction(facade, || {
        let row = facade
            .find(LOCK)?
            .eq("lockName", Value::string(name))
            .use_cache(false)
            .disable_authz()
            .one()?;
        Ok(row.and_then(|row| {
            let instance = row.get("instanceId").map(to_plain_string).unwrap_or_default();
            (!instance.is_empty()).then(|| (instance, row.get("leaseExpireDate").and_then(crate::user::value_millis)))
        }))
    })
}

/// This process's part in the lock, per facade: whether it is the sender, and the instance id it holds it as.
#[derive(Default)]
struct Role {
    sending: AtomicBool,
    instance: Mutex<Option<String>>,
}

#[derive(Default)]
struct RoleSlot(Option<Arc<Role>>);

fn role(facade: &EntityFacade) -> Arc<Role> {
    facade.with_slot(RoleSlot::default, |slot| {
        Arc::clone(slot.0.get_or_insert_with(Default::default))
    })
}

/// Whether this member is the replication sender now: it keeps the link's own rows and sends replication.
pub fn is_sender(facade: &EntityFacade) -> bool {
    facade
        .with_slot_read(RoleSlot::default, |slot| slot.0.clone())
        .is_some_and(|role| role.sending.load(Ordering::Acquire))
}

/// Record whether this member, `instance_id`, is the sender after a [`claim`] (`None`: it is not, or it does not know).
/// A member that becomes the sender sends each set whole; one that stops being it drops its notes. True when it
/// changed.
pub fn set_sender(facade: &EntityFacade, instance_id: Option<&str>) -> bool {
    let role = role(facade);
    let sending = instance_id.is_some();
    *lock(&role.instance) = instance_id.map(str::to_string);
    let changed = role.sending.swap(sending, Ordering::AcqRel) != sending;
    if changed {
        if sending {
            tracing::info!("Edge replication: this member is the sender now; each set goes whole");
        } else {
            tracing::info!("Edge replication: another member is the sender now");
        }
    }
    if let Some(replica) = super::replica(facade) {
        replica.set_keeping(sending);
        if changed && sending {
            // a new sender: every set whole, under a new epoch
            replica.resync_all();
        }
    }
    changed
}

/// Let the lock go when this member holds it (its server stops, or it unpairs). A failure is logged: the lease
/// lapses on its own.
pub(super) fn let_go(facade: &EntityFacade) {
    let Some(role) = facade.with_slot_read(RoleSlot::default, |slot| slot.0.clone()) else {
        return;
    };
    let instance = lock(&role.instance).take();
    role.sending.store(false, Ordering::Release);
    let Some(instance) = instance else {
        return;
    };
    match release(facade, REPLICATE, &instance) {
        Ok(true) => tracing::info!("Edge replication: this member let the sender lock go"),
        Ok(false) => {}
        Err(err) => tracing::warn!("Edge replication: the sender lock was not let go ({err}); its lease lapses"),
    }
}

/// The notes this member made and has not sent to the others yet, per facade.
#[derive(Default)]
struct Outgoing {
    notes: Vec<(String, Option<String>)>,
    /// A thread is gathering them.
    gathering: bool,
}

#[derive(Default)]
struct OutgoingSlot(Arc<Mutex<Outgoing>>);

/// Send a note this member made to the other members of its cluster, gathered for a moment with the ones after it, on
/// [`crate::notify::EDGE_REPLICA_NOTE`]: whichever of them is the sender takes it ([`take_notes`]). Nothing goes on a
/// node that is not in a cluster with peers.
pub(super) fn forward_note(facade: &EntityFacade, entity: &str, key: Option<String>) {
    if !crate::cluster::is_linked(facade) {
        return;
    }
    let outgoing = facade.with_slot(OutgoingSlot::default, |slot| Arc::clone(&slot.0));
    {
        let mut waiting = lock(&outgoing);
        waiting.notes.push((entity.to_string(), key));
        if waiting.gathering {
            return;
        }
        waiting.gathering = true;
    }
    let sending = facade.clone();
    let spawned = std::thread::Builder::new()
        .name("moqui-edge-notes".to_string())
        .spawn(move || {
            std::thread::sleep(NOTE_GATHER);
            let notes = {
                let mut waiting = lock(&outgoing);
                waiting.gathering = false;
                std::mem::take(&mut waiting.notes)
            };
            publish_notes(&sending, notes);
        });
    if let Err(err) = spawned {
        tracing::error!("Edge replication: the notes for the sender were not sent: {err}");
        facade.with_slot(OutgoingSlot::default, |slot| lock(&slot.0).gathering = false);
    }
}

/// The body of a note message: this member, and each note as `[entity, key]` (a `null` key, or `whole`, sends the
/// entity's sets whole).
fn notes_body(from: &str, notes: Vec<(String, Option<String>)>) -> serde_json::Value {
    let mut whole: Vec<String> = Vec::new();
    let mut keyed: Vec<serde_json::Value> = Vec::new();
    for (entity, key) in notes {
        match key {
            Some(key) if keyed.len() < MAX_NOTE_KEYS => keyed.push(serde_json::json!([entity, key])),
            _ => {
                if !whole.contains(&entity) {
                    whole.push(entity);
                }
            }
        }
    }
    serde_json::json!({ "from": from, "notes": keyed, "whole": whole })
}

fn publish_notes(facade: &EntityFacade, notes: Vec<(String, Option<String>)>) {
    if notes.is_empty() {
        return;
    }
    let from = facade_instance(facade);
    let message = crate::notify::TopicMessage {
        origin_id: crate::notify::origin_id().to_string(),
        message_id: String::new(),
        body: notes_body(&from, notes),
    };
    if let Err(err) = crate::cluster::publish(facade, crate::notify::EDGE_REPLICA_NOTE, &message) {
        tracing::warn!("Edge replication: the notes for the sender were not sent: {err}");
    }
}

/// This member's instance id, as the notes name it.
fn facade_instance(facade: &EntityFacade) -> String {
    crate::cluster::linked_instance_id(facade).unwrap_or_default()
}

/// Take the notes another member sent ([`forward_note`]): kept when this member is the sender, else dropped; a member's
/// own are not taken again. The count taken.
pub fn take_notes(facade: &EntityFacade, body: &serde_json::Value) -> usize {
    let from = body.get("from").and_then(serde_json::Value::as_str).unwrap_or_default();
    if from.is_empty() || from == facade_instance(facade) || !is_sender(facade) {
        return 0;
    }
    let Some(replica) = super::replica(facade) else {
        return 0;
    };
    let mut taken = 0;
    for note in body
        .get("notes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_NOTE_KEYS)
    {
        let (Some(entity), Some(key)) = (
            note.get(0).and_then(serde_json::Value::as_str),
            note.get(1).and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        if replica.note_local(entity, Some(key.to_string())) {
            taken += 1;
        }
    }
    for entity in body
        .get("whole")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .take(MAX_NOTE_KEYS)
    {
        if replica.note_local(entity, None) {
            taken += 1;
        }
    }
    taken
}

/// Listen for the other members' notes on this factory's cluster topic; kept with the factory.
pub fn install_note_listener(factory: &Arc<ExecutionContextFactory>) {
    struct NoteListener {
        _subscription: crate::notify::Subscription,
    }
    let weak = Arc::downgrade(factory);
    let listener: crate::notify::Listener = Arc::new(move |message| {
        let Some(factory) = weak.upgrade() else {
            return;
        };
        if let Some(facade) = factory.entity() {
            take_notes(&facade, &message.body);
        }
    });
    match crate::notify::subscribe(crate::notify::EDGE_REPLICA_NOTE, listener) {
        Ok(subscription) => {
            factory.extension(move |_| NoteListener {
                _subscription: subscription,
            });
        }
        Err(err) => tracing::error!(
            "Edge replication: the notes topic was not subscribed: {}",
            err.message()
        ),
    }
}

#[cfg(test)]
#[path = "replica_sender_tests.rs"]
mod tests;
