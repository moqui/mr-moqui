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

//! A pairing's caps on what its local node gives an edge node to keep (SEC-CLU-02): the replicated records of all its
//! sets (`<edge replica-max-rows>`, or the pairing's `EdgeClient.replicaMaxRows`), and the content its link sends
//! before deliveries, kept under [`CONTENT_IN`]`<node>/` until each delivery is answered, against the pairing's queue
//! cap (`queue-max-megabytes`, or its `EdgeClient.queueMaxMegabytes`).
//!
//! At a cap the pairing pauses: its `Replicate` and `PutContent` calls are refused as "try again later", so the sender
//! keeps what it has and nothing is dropped, and the edge node's administrators are told (the [`PAUSED_TOPIC`]
//! notification). It stays paused (`EdgeClient.capPausedDate`) until an administrator resumes it
//! (`resume#EdgeClient`) or sets its caps (`update#EdgeClient`); a send that still passes a cap pauses it again.
//!
//! A local node checks the content its edge node sends against its own queue cap, with no pause of its own (it has one
//! pairing and no `EdgeClient` row): content that would pass the cap is refused for now, logged.

use moqui_lib::error::Error;
use moqui_lib::json::canonical_sha256;
use moqui_lib::object::Value;

use super::queue::{node_cap, number, text, transaction, CONTENT_IN};
use super::replica::{Batch, VALUE};
use super::EdgeSettings;
use crate::context::ExecutionContextFactory;
use crate::entity::{Condition, EntityFacade};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value};

/// The notification an edge node's administrators get when a pairing pauses at a cap.
pub const PAUSED_TOPIC: &str = "EdgePairingPaused";
const CLIENT: &str = "moqui.edge.EdgeClient";
/// Who is told: the edge node's administrators, as of a takeover.
const ADMINISTRATORS: &str = "ADMIN";

/// Whether a pairing's send is taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admit {
    Take,
    /// Not now: the pairing is paused, or this send would pass a cap (why). The sender tries again later.
    Paused(String),
}

/// What a pairing's row says of its caps and pause.
struct ClientCaps {
    paused: Option<String>,
    max_rows: u64,
}

fn client_caps(facade: &EntityFacade, settings: &EdgeSettings, node: &str) -> Result<ClientCaps, Error> {
    let row = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .select_field("capPausedDate")
            .select_field("capPausedReason")
            .select_field("replicaMaxRows")
            .disable_authz()
            .one()
    })?;
    let Some(row) = row else {
        return Ok(ClientCaps {
            paused: None,
            max_rows: settings.replica_max_rows,
        });
    };
    let paused = (!text(&row, "capPausedDate").is_empty()).then(|| {
        let reason = text(&row, "capPausedReason");
        if reason.is_empty() {
            "the pairing is paused".to_string()
        } else {
            reason
        }
    });
    let own = number(&row, "replicaMaxRows");
    Ok(ClientCaps {
        paused,
        max_rows: u64::try_from(own)
            .ok()
            .filter(|rows| *rows > 0)
            .unwrap_or(settings.replica_max_rows),
    })
}

/// Why pairing `node` is paused, or `None` when it is not (or has no row).
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn paused(facade: &EntityFacade, node: &str) -> Result<Option<String>, Error> {
    Ok(client_caps(facade, &EdgeSettings::default(), node)?.paused)
}

/// Whether the edge node takes Replicate batch `batch` of pairing `node`: not while the pairing is paused, nor when its
/// new records would take the pairing's records past its cap, which pauses it. A batch that adds no record (deletes,
/// updates of records it keeps, the probe at a cursor) is taken at the cap. On a node that takes no links, always.
///
/// # Errors
///
/// Returns the entity facade's error.
pub fn admit_batch(facade: &EntityFacade, settings: &EdgeSettings, node: &str, batch: &Batch) -> Result<Admit, Error> {
    if !settings.takes_links() {
        return Ok(Admit::Take);
    }
    let caps = client_caps(facade, settings, node)?;
    if let Some(why) = caps.paused {
        return Ok(Admit::Paused(why));
    }
    let writes = batch
        .changes
        .iter()
        .filter(|change| change.fields_json.is_some())
        .count() as u64;
    if writes == 0 {
        return Ok(Admit::Take);
    }
    let held = held_rows(facade, node)?;
    if held.saturating_add(writes) <= caps.max_rows {
        return Ok(Admit::Take);
    }
    // near the cap: an update of a record kept adds none, and a delete of one makes room
    let (kept_writes, kept_deletes) = kept_in_batch(facade, node, batch)?;
    let after = held.saturating_add(writes - kept_writes).saturating_sub(kept_deletes);
    if after <= caps.max_rows {
        return Ok(Admit::Take);
    }
    let reason = format!(
        "its replicated records would be {after}, over its cap of {} (replica-max-rows, or the pairing's own)",
        caps.max_rows
    );
    pause_at_cap(facade, node, reason)
}

/// Whether the edge node keeps `size` bytes of content pairing `node` sends before a delivery: not while the pairing
/// is paused, nor when the content kept for it under [`CONTENT_IN`] would pass its queue cap, which pauses it. On a
/// local node (`node` its own id) the content its edge node sends is checked against its queue cap alone: refused,
/// logged, with no pause.
///
/// # Errors
///
/// Returns the entity facade's or the resource facade's error.
pub fn admit_content(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    settings: &EdgeSettings,
    node: &str,
    size: u64,
) -> Result<Admit, Error> {
    if settings.takes_links() {
        if let Some(why) = paused(facade, node)? {
            return Ok(Admit::Paused(why));
        }
    }
    let cap = node_cap(facade, settings, node);
    let directory = format!("{CONTENT_IN}{node}");
    let held = transaction(facade, TransactionAttribute::UseOrBegin, || {
        factory.resource().tree_size(&directory).map_err(Error::from)
    })?;
    let after = held.saturating_add(size);
    if after <= cap {
        return Ok(Admit::Take);
    }
    let reason = format!("the content kept for its deliveries would be {after} bytes, over its queue cap of {cap}");
    if settings.takes_links() {
        pause_at_cap(facade, node, reason)
    } else {
        tracing::error!("Edge link: content from the edge node is refused for now: {reason}");
        Ok(Admit::Paused(reason))
    }
}

/// The replicated records kept for `node`, all its sets.
fn held_rows(facade: &EntityFacade, node: &str) -> Result<u64, Error> {
    let count = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade
            .find(VALUE)?
            .eq("pairedNodeId", Value::string(node))
            .disable_authz()
            .count()
    })?;
    Ok(u64::try_from(count).unwrap_or(0))
}

/// Of `batch`'s changes, the writes and the deletes of records `node` already has kept in the batch's set.
fn kept_in_batch(facade: &EntityFacade, node: &str, batch: &Batch) -> Result<(u64, u64), Error> {
    let changes: Vec<(String, String, bool)> = batch
        .changes
        .iter()
        .filter_map(|change| {
            let pk: serde_json::Value = serde_json::from_str(&change.pk_json).ok()?;
            Some((
                change.entity_name.clone(),
                canonical_sha256(&pk),
                change.fields_json.is_some(),
            ))
        })
        .collect();
    if changes.is_empty() {
        return Ok((0, 0));
    }
    let hashes: Vec<Value> = changes.iter().map(|(_, hash, _)| Value::string(hash)).collect();
    let rows = transaction(facade, TransactionAttribute::UseOrBegin, || {
        facade
            .find(VALUE)?
            .eq("pairedNodeId", Value::string(node))
            .eq("setName", Value::string(&batch.set_name))
            .condition(Condition::is_in("pkHash", hashes))
            .select_field("entityName")
            .select_field("pkHash")
            .disable_authz()
            .list()
    })?;
    let kept: std::collections::BTreeSet<(String, String)> = rows
        .values()
        .iter()
        .map(|row| (text(row, "entityName"), text(row, "pkHash")))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let (mut writes, mut deletes) = (0, 0);
    for (entity, hash, write) in changes {
        // a record twice in one batch counts once
        if !seen.insert((entity.clone(), hash.clone())) || !kept.contains(&(entity, hash)) {
            continue;
        }
        if write {
            writes += 1;
        } else {
            deletes += 1;
        }
    }
    Ok((writes, deletes))
}

/// Pause pairing `node` at a cap for `reason`, and tell the administrators when it was not paused already: the send is
/// refused either way.
fn pause_at_cap(facade: &EntityFacade, node: &str, reason: String) -> Result<Admit, Error> {
    if pause(facade, node, &reason)? {
        tracing::error!(
            "Edge pairing {node} is paused: {reason}. Its replication and content wait until an administrator \
             resumes it or raises its caps (System, Edge Clients)"
        );
        if let Err(err) = notify_paused(node, &reason) {
            tracing::error!("Edge pairing {node}: the administrators were not told of its pause: {err}");
        }
    }
    Ok(Admit::Paused(reason))
}

/// Mark pairing `node` paused for `reason`: true when it was not paused before. A pairing with no row is not marked.
fn pause(facade: &EntityFacade, node: &str, reason: &str) -> Result<bool, Error> {
    transaction(facade, TransactionAttribute::ForceNew, || {
        let Some(mut row) = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .for_update(true)
            .disable_authz()
            .one()?
        else {
            return Ok(false);
        };
        if !text(&row, "capPausedDate").is_empty() {
            return Ok(false);
        }
        row.set("capPausedDate", stamp_value(now_millis()));
        row.set(
            "capPausedReason",
            Value::string(reason.chars().take(250).collect::<String>()),
        );
        row.update(facade)?;
        Ok(true)
    })
}

/// Let pairing `node` send again after a pause at a cap: true when it was paused. Its next send over a cap pauses it
/// again. In the caller's transaction, when there is one.
///
/// # Errors
///
/// Refuses a pairing with no row; returns the entity facade's error.
pub fn resume(facade: &EntityFacade, node: &str) -> Result<bool, Error> {
    transaction(facade, TransactionAttribute::UseOrBegin, || {
        let Some(mut row) = facade
            .find(CLIENT)?
            .eq("pairedNodeId", Value::string(node))
            .for_update(true)
            .disable_authz()
            .one()?
        else {
            return Err(Error::new(format!(
                "no local node {node} is paired with this edge node"
            )));
        };
        if text(&row, "capPausedDate").is_empty() {
            return Ok(false);
        }
        row.set("capPausedDate", Value::Null);
        row.set("capPausedReason", Value::Null);
        row.update(facade)?;
        Ok(true)
    })
}

/// Tell the edge node's administrators that pairing `node` paused at a cap. In the current context.
fn notify_paused(node: &str, reason: &str) -> Result<(), Error> {
    // plain text: an id holds no template
    let clean = |text: &str| text.replace(['$', '{', '}'], "");
    let mut message = moqui_lib::object::ValueMap::default();
    message.insert("pairedNodeId".to_string(), Value::string(node));
    message.insert("reason".to_string(), Value::string(reason));
    crate::notify::NotificationMessage::new()
        .topic(PAUSED_TOPIC)
        .user_group_id(ADMINISTRATORS)
        .notification_type("danger")
        .message_map(message)
        .title(format!("Edge pairing {} is paused: {}", clean(node), clean(reason)))
        .send()
        .map_err(|err| Error::new(err.message().to_string()))
}

#[cfg(test)]
#[path = "caps_tests.rs"]
mod tests;
