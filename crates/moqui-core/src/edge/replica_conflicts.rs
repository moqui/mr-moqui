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

//! Replication conflicts (plans/archive/Edge-Node.md §10, E12; plans/Edge-Node2.md §4.1, E16): a record of a paired
//! node's set that claims an item another owner holds ([`claims::copy_claim`]: a mail address, a chat bridge id, a phone
//! number) is not applied. On an all-in-one node the server's own item wins ([`claims::own_claim`]); on any node that
//! takes links, another active pairing's copy wins ([`claims::claim_among`]: the first claim holds), except a chat bridge
//! id, which two pairings may share (the request's host name tells them apart). The record is logged as an error and
//! kept as a `moqui.edge.EdgeReplicaConflict` row, keyed as its `EdgeReplicaValue` would be, and Edge Clients shows it;
//! the rest of the batch goes on. A later batch that writes the record again checks it again, one that deletes it (or
//! a full send without it) removes the row, and [`recheck_conflicts`] (also at the start of each batch of the set)
//! applies the record once no other owner holds the item. A dedicated edge node with one pairing checks nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::json::canonical;
use moqui_lib::object::Value;

use super::super::claims::{self, ClaimKind};
use super::{text, transaction, Batch, Change, VALUE};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value, value_millis};

pub const CONFLICT: &str = "moqui.edge.EdgeReplicaConflict";
/// The most conflict rows one check reads.
const MAX_CONFLICTS: i64 = 10_000;

/// Who else may hold what a paired node's records claim: the server (on an all-in-one node) and the other active
/// pairings, read once for a batch or a recheck.
pub(super) struct Owners {
    factory: Option<Arc<ExecutionContextFactory>>,
    all_in_one: bool,
    /// The active pairings, in id order.
    active: Vec<String>,
    /// The copies' mail addresses, read once for the batch or recheck.
    mail: claims::MailIndex,
}

impl Owners {
    /// The factory a batch's checks run on.
    pub(super) fn factory(&self) -> Option<&Arc<ExecutionContextFactory>> {
        self.factory.as_ref()
    }

    /// The other holders `node`'s records are checked against: none on a node with no other owner.
    fn others(&self, node: &str) -> Vec<String> {
        self.active.iter().filter(|other| *other != node).cloned().collect()
    }

    /// Whether a record of `node` can claim anything another owner holds.
    fn checks(&self, node: &str) -> bool {
        self.all_in_one || self.active.iter().any(|other| other != node)
    }
}

/// The owners of the current context's node when it takes links and has more than one owner: an all-in-one node (the
/// server and its pairings), or a dedicated edge node with several active pairings. `None` otherwise, when nothing is
/// checked. A list of pairings that cannot be read is logged, and the server alone is checked.
pub(super) fn owners(facade: &EntityFacade) -> Option<Owners> {
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))?;
    let mut owners = owners_of(&factory, facade)?;
    owners.factory = Some(factory);
    (owners.all_in_one || owners.active.len() > 1).then_some(owners)
}

/// The owners on `factory`'s node, `None` on a node that takes no links.
fn owners_of(factory: &ExecutionContextFactory, facade: &EntityFacade) -> Option<Owners> {
    let settings = super::super::settings(factory);
    if !settings.takes_links() {
        return None;
    }
    let active = match claims::active_nodes(facade) {
        Ok(active) => active,
        Err(error) => {
            tracing::error!(
                "Edge replication: the paired nodes were not read, so other pairings are not checked: {error}"
            );
            Vec::new()
        }
    };
    Some(Owners {
        factory: None,
        all_in_one: settings.all_in_one(),
        active,
        mail: claims::MailIndex::default(),
    })
}

/// A record of a paired node's set not applied for a claim of the server's own item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conflict {
    pub paired_node_id: String,
    pub set_name: String,
    pub entity_name: String,
    pub pk_hash: String,
    /// [`ClaimKind::id`].
    pub claim_kind: String,
    pub item_key: String,
    /// The server's holder ("the server's ChatBridge B1").
    pub owner_text: String,
    pub first_date: Option<i64>,
    pub last_date: Option<i64>,
}

fn conflict_of(row: &EntityValue) -> Conflict {
    Conflict {
        paired_node_id: text(row, "pairedNodeId"),
        set_name: text(row, "setName"),
        entity_name: text(row, "entityName"),
        pk_hash: text(row, "pkHash"),
        claim_kind: text(row, "claimKind"),
        item_key: text(row, "itemKey"),
        owner_text: text(row, "ownerText"),
        first_date: row.get("firstDate").and_then(value_millis),
        last_date: row.get("lastDate").and_then(value_millis),
    }
}

/// The holder other than `node` of the item a record claims, or `None`: the server's (on an all-in-one node), else
/// another pairing's ("paired node local-b"). A check that fails is a holder too, logged: the record waits as a
/// conflict until a check reads.
fn held(
    factory: &ExecutionContextFactory,
    owners: &Owners,
    facade: &EntityFacade,
    node: &str,
    kind: ClaimKind,
    key: &str,
) -> Option<String> {
    if owners.all_in_one {
        match claims::own_claim(factory, facade, kind, key) {
            Ok(Some(holder)) => return Some(holder),
            Ok(None) => {}
            Err(error) => {
                tracing::error!(
                    "Edge replication: whether the server holds {} {key} could not be read: {error}",
                    kind.label()
                );
                return Some(format!("the server (the check failed: {error})"));
            }
        }
    }
    if kind == ClaimKind::ChatBridge {
        return None;
    }
    match claims::claim_among_with(facade, kind, key, &owners.others(node), &owners.mail) {
        Ok(holder) => holder.map(|other| format!("paired node {other}")),
        Err(error) => {
            tracing::error!(
                "Edge replication: whether another pairing holds {} {key} could not be read: {error}",
                kind.label()
            );
            Some(format!("another pairing (the check failed: {error})"))
        }
    }
}

fn conflict_find(
    facade: &EntityFacade,
    node: &str,
    set_name: &str,
    entity_name: &str,
    hash: &str,
) -> Result<crate::entity::EntityFind, Error> {
    Ok(facade
        .find(CONFLICT)?
        .eq("pairedNodeId", Value::string(node))
        .eq("setName", Value::string(set_name))
        .eq("entityName", Value::string(entity_name))
        .eq("pkHash", Value::string(hash))
        .disable_authz())
}

/// In a batch from `node`: whether `change` is skipped because its record claims an item another owner holds. Then the
/// record is logged and kept as a conflict, and a copy applied before is removed. Otherwise (and for a delete) a
/// conflict of the record is removed and the caller applies the change.
///
/// # Errors
///
/// Returns the database's error writing the rows.
#[allow(clippy::too_many_arguments)]
pub(super) fn skipped(
    owners: &Owners,
    facade: &EntityFacade,
    node: &str,
    batch: &Batch,
    change: &Change,
    pk: &serde_json::Value,
    hash: &str,
    now: &Value,
) -> Result<bool, Error> {
    let find = || conflict_find(facade, node, &batch.set_name, &change.entity_name, hash);
    let claim = change.fields_json.as_deref().and_then(|fields| {
        let fields: serde_json::Value = serde_json::from_str(fields).unwrap_or_default();
        claims::copy_claim(&change.entity_name, pk, &fields)
    });
    let holder = match (&owners.factory, claim.as_ref().filter(|_| owners.checks(node))) {
        (Some(factory), Some((kind, key))) => {
            held(factory, owners, facade, node, *kind, key).map(|holder| (*kind, key, holder))
        }
        _ => None,
    };
    let Some((kind, key, holder)) = holder else {
        find()?.delete_all()?;
        return Ok(false);
    };
    tracing::error!(
        "Edge replication from {node}: {} record {} of set {} claims {}, so it is skipped: the first holder keeps it",
        change.entity_name,
        change.pk_json,
        batch.set_name,
        claims::claimed_message(kind, key, &holder)
    );
    let first = find()?.one()?.and_then(|row| row.get("firstDate").cloned());
    let mut row = facade.make_value(CONFLICT)?;
    row.set("pairedNodeId", Value::string(node));
    row.set("setName", Value::string(&batch.set_name));
    row.set("entityName", Value::string(&change.entity_name));
    row.set("pkHash", Value::string(hash));
    row.set("claimKind", Value::string(kind.id()));
    row.set("itemKey", Value::string(key));
    row.set("ownerText", Value::string(&holder));
    row.set("pkJson", Value::string(canonical(pk)));
    row.set(
        "fieldsJson",
        Value::string(change.fields_json.as_deref().unwrap_or_default()),
    );
    row.set("replicaEpoch", Value::string(&batch.epoch));
    row.set("firstDate", first.unwrap_or_else(|| now.clone()));
    row.set("lastDate", now.clone());
    row.create_or_update(facade)?;
    facade
        .find(VALUE)?
        .eq("pairedNodeId", Value::string(node))
        .eq("setName", Value::string(&batch.set_name))
        .eq("entityName", Value::string(&change.entity_name))
        .eq("pkHash", Value::string(hash))
        .disable_authz()
        .delete_all()?;
    Ok(true)
}

/// At the end of a full send from `node`: the conflicts of the set from older epochs go, as its values do.
pub(super) fn drop_older(facade: &EntityFacade, node: &str, batch: &Batch) -> Result<(), Error> {
    facade
        .find(CONFLICT)?
        .eq("pairedNodeId", Value::string(node))
        .eq("setName", Value::string(&batch.set_name))
        .condition(Condition::or(vec![
            Condition::is_null("replicaEpoch"),
            Condition::not_equals("replicaEpoch", Value::string(&batch.epoch)),
        ]))
        .disable_authz()
        .delete_all()?;
    Ok(())
}

/// Check the conflicts (of `node` and `set_name` when given) again in the caller's transaction: a record whose item
/// no other owner holds any more is applied as its `EdgeReplicaValue` and its conflict removed; one still held has its
/// holder and `lastDate` written again. One that fails is logged and the rest go on. The set of each record applied.
pub(super) fn recheck_in(
    factory: &ExecutionContextFactory,
    owners: &Owners,
    facade: &EntityFacade,
    node: Option<&str>,
    set_name: Option<&str>,
) -> Vec<String> {
    let mut applied = Vec::new();
    let rows = facade.find(CONFLICT).and_then(|find| {
        let mut find = find.limit(MAX_CONFLICTS).disable_authz();
        if let Some(node) = node {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        if let Some(set) = set_name {
            find = find.eq("setName", Value::string(set));
        }
        find.list()
    });
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!("Edge replication: the conflicts were not read: {error}");
            return applied;
        }
    };
    let now = stamp_value(now_millis());
    for row in rows.values() {
        let conflict = conflict_of(row);
        let Some(kind) = ClaimKind::from_id(&conflict.claim_kind) else {
            tracing::error!(
                "Edge replication: conflict {} {} of {} has an unknown kind {}; it is left",
                conflict.entity_name,
                conflict.pk_hash,
                conflict.paired_node_id,
                conflict.claim_kind
            );
            continue;
        };
        let done = match held(
            factory,
            owners,
            facade,
            &conflict.paired_node_id,
            kind,
            &conflict.item_key,
        ) {
            Some(holder) => {
                let mut row = row.clone();
                row.set("ownerText", Value::string(&holder));
                row.set("lastDate", now.clone());
                row.update(facade).map(|_| false)
            }
            None => apply_value(facade, row, &now).map(|()| true),
        };
        match done {
            Ok(true) => {
                tracing::info!(
                    "Edge replication from {}: {} no longer held by another owner, so its {} record of set {} is applied",
                    conflict.paired_node_id,
                    format_args!("{} {}", kind.label(), conflict.item_key),
                    conflict.entity_name,
                    conflict.set_name
                );
                applied.push(conflict.set_name);
            }
            Ok(false) => {}
            Err(error) => tracing::error!(
                "Edge replication: conflict {} {} of {} was not checked again: {error}",
                conflict.entity_name,
                conflict.pk_hash,
                conflict.paired_node_id
            ),
        }
    }
    applied
}

/// Write a conflict's record as its value, and remove the conflict.
fn apply_value(facade: &EntityFacade, conflict: &EntityValue, now: &Value) -> Result<(), Error> {
    let mut value = facade.make_value(VALUE)?;
    for field in [
        "pairedNodeId",
        "setName",
        "entityName",
        "pkHash",
        "pkJson",
        "fieldsJson",
        "replicaEpoch",
    ] {
        value.set(field, conflict.get(field).cloned().unwrap_or(Value::Null));
    }
    value.set("updatedDate", now.clone());
    value.create_or_update(facade)?;
    conflict_find(
        facade,
        &text(conflict, "pairedNodeId"),
        &text(conflict, "setName"),
        &text(conflict, "entityName"),
        &text(conflict, "pkHash"),
    )?
    .delete_all()?;
    Ok(())
}

/// On a node that takes links, check the replication conflicts (of `paired_node_id` when given) again in a transaction
/// of their own: each record whose item no other owner holds any more is applied (Edge Clients' Check again, and
/// `recheck#ReplicaConflicts`). How many were applied; 0 on a local node. On a dedicated edge node left with one
/// pairing, every conflict is applied.
///
/// # Errors
///
/// Returns the transaction's error.
pub fn recheck_conflicts(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    paired_node_id: Option<&str>,
) -> Result<usize, Error> {
    let Some(owners) = owners_of(factory, facade) else {
        return Ok(0);
    };
    let applied = transaction(facade, CONFLICT, TransactionAttribute::ForceNew, || {
        Ok(recheck_in(factory, &owners, facade, paired_node_id, None))
    })?;
    // a reader that keeps something built from a set's copies builds it again, here and on the other members of an
    // edge cluster (plans/Edge-Node2.md §4.6)
    let sets: BTreeSet<&String> = applied.iter().collect();
    for set in &sets {
        super::note_applied(facade, set);
    }
    let names: Vec<&str> = sets.iter().map(|set| set.as_str()).collect();
    super::super::cluster_state::tell(facade, false, &names);
    Ok(applied.len())
}

/// The replication conflicts (of `paired_node_id` when given), by node, set and entity.
///
/// # Errors
///
/// Returns the database's error.
pub fn list_conflicts(facade: &EntityFacade, paired_node_id: Option<&str>) -> Result<Vec<Conflict>, Error> {
    transaction(facade, CONFLICT, TransactionAttribute::UseOrBegin, || {
        let mut find = facade
            .find(CONFLICT)?
            .order_by("pairedNodeId", false)
            .order_by("setName", false)
            .order_by("entityName", false)
            .order_by("pkHash", false)
            .limit(MAX_CONFLICTS)
            .disable_authz();
        if let Some(node) = paired_node_id {
            find = find.eq("pairedNodeId", Value::string(node));
        }
        Ok(find.list()?.values().iter().map(conflict_of).collect())
    })
}

#[cfg(test)]
#[path = "replica_conflicts_tests.rs"]
mod tests;
