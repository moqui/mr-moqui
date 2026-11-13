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

//! Replication conflicts on an all-in-one node (plans/archive/Edge-Node.md §10, E12): a record of a paired node's set that
//! claims an item the server holds itself ([`claims::copy_claim`], [`claims::own_claim`]: a mail address, a chat bridge
//! id, a phone number) is not applied. The server's own item wins. The record is logged as an error and kept as a
//! `moqui.edge.EdgeReplicaConflict` row, keyed as its `EdgeReplicaValue` would be, and Edge Clients shows it; the rest
//! of the batch goes on. A later batch that writes the record again checks it again, one that deletes it (or a full
//! send without it) removes the row, and [`recheck_conflicts`] (also at the start of each batch of the set) applies the
//! record once the server no longer holds the item. A dedicated edge node holds no such items and checks nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use moqui_lib::error::Error;
use moqui_lib::json::canonical;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::super::claims::{self, ClaimKind};
use super::{text, transaction, AppliedCounts, Batch, Change, VALUE};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::transaction::TransactionAttribute;
use crate::user::{now_millis, stamp_value, value_millis};

pub const CONFLICT: &str = "moqui.edge.EdgeReplicaConflict";
/// The most conflict rows one check reads.
const MAX_CONFLICTS: i64 = 10_000;

/// The current context's factory when this node is an all-in-one node, whose own items replication must not take.
pub(super) fn all_in_one_factory() -> Option<Arc<ExecutionContextFactory>> {
    let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))?;
    super::super::settings(&factory).all_in_one().then_some(factory)
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

/// The server's holder of the item `copy` claims, or `None`. A check that fails is a holder too, logged: the record
/// waits as a conflict until a check reads.
fn held(factory: &ExecutionContextFactory, facade: &EntityFacade, kind: ClaimKind, key: &str) -> Option<String> {
    match claims::own_claim(factory, facade, kind, key) {
        Ok(holder) => holder,
        Err(error) => {
            tracing::error!(
                "Edge replication: whether the server holds {} {key} could not be read: {error}",
                kind.label()
            );
            Some(format!("the server (the check failed: {error})"))
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

/// In a batch from `node` on an all-in-one node: whether `change` is skipped because its record claims an item the
/// server holds. Then the record is logged and kept as a conflict, and a copy applied before is removed. Otherwise
/// (and for a delete) a conflict of the record is removed and the caller applies the change.
///
/// # Errors
///
/// Returns the database's error writing the rows.
#[allow(clippy::too_many_arguments)]
pub(super) fn skipped(
    factory: &ExecutionContextFactory,
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
    let holder = claim
        .as_ref()
        .and_then(|(kind, key)| held(factory, facade, *kind, key).map(|holder| (*kind, key, holder)));
    let Some((kind, key, holder)) = holder else {
        find()?.delete_all()?;
        return Ok(false);
    };
    tracing::error!(
        "Edge replication from {node}: {} record {} of set {} claims {}, so it is skipped: the server's own item wins",
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
/// the server no longer holds is applied as its `EdgeReplicaValue` and its conflict removed; one still held has its
/// holder and `lastDate` written again. One that fails is logged and the rest go on. The set of each record applied.
pub(super) fn recheck_in(
    factory: &ExecutionContextFactory,
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
        let done = match held(factory, facade, kind, &conflict.item_key) {
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
                    "Edge replication from {}: {} no longer held by the server, so its {} record of set {} is applied",
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

/// On an all-in-one node, check the replication conflicts (of `paired_node_id` when given) again in a transaction of
/// their own: each record whose item the server no longer holds is applied (Edge Clients' Check again, and
/// `recheck#ReplicaConflicts`). How many were applied; 0 on another node.
///
/// # Errors
///
/// Returns the transaction's error.
pub fn recheck_conflicts(
    factory: &ExecutionContextFactory,
    facade: &EntityFacade,
    paired_node_id: Option<&str>,
) -> Result<usize, Error> {
    if !super::super::settings(factory).all_in_one() {
        return Ok(0);
    }
    let applied = transaction(facade, CONFLICT, TransactionAttribute::ForceNew, || {
        Ok(recheck_in(factory, facade, paired_node_id, None))
    })?;
    // a reader that keeps something built from a set's copies builds it again
    let sets: BTreeSet<&String> = applied.iter().collect();
    facade.with_slot(AppliedCounts::default, |counts| {
        let mut counts = lock(&counts.0);
        for set in sets {
            *counts.entry(set.clone()).or_insert(0) += 1;
        }
    });
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
