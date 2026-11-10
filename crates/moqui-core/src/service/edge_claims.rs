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

//! The ownership services of an all-in-one node (plans/Edge-Node.md §10, E12), in `org.moqui.edge.EdgeServices`:
//! `check#ReplicaClaim`, which the server's own create and update services of mail accounts, identities, chat bridges
//! and numbers run (`EdgeClaims.secas.xml`) so the server does not take an item a paired node holds, and the
//! replication conflicts Edge Clients shows (`get#ReplicaConflicts`, `recheck#ReplicaConflicts`,
//! `crate::edge::replica::conflicts`). On a node that is not an all-in-one node each returns at once.

use std::collections::BTreeMap;

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::edge::claims::{self, ClaimKind};
use crate::entity::{EntityFacade, EntityValue};
use crate::user::stamp_value;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, check_replica_claim)?;
    rust_service!(map, get_replica_conflicts)?;
    rust_service!(map, recheck_replica_conflicts)?;
    Ok(())
}

type Out = Result<BTreeMap<String, Value>, LibError>;

const ACCOUNT: &str = "moqui.basic.email.EmailAccount";
const IDENTITY: &str = "moqui.basic.email.EmailIdentity";
/// The account kinds that receive mail here, whose addresses are claims.
const RECEIVING_KINDS: [&str; 2] = ["EakHosted", "EakShared"];

fn text(parameters: &BTreeMap<String, Value>, name: &str) -> Option<String> {
    parameters
        .get(name)
        .filter(|value| !is_empty(value))
        .map(|value| to_plain_string(value).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .map(to_plain_string)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn all_in_one() -> bool {
    ExecutionContext::with_current(|ec| crate::edge::settings(ec.factory()).all_in_one()).unwrap_or(false)
}

/// The address an account receives at, when it is a hosted or shared account not removed.
fn receiving_address(account: &EntityValue) -> Option<String> {
    let receives = RECEIVING_KINDS.contains(&field(account, "accountKindEnumId").as_str())
        && field(account, "statusId") != "EasRemoved";
    Some(field(account, "emailAddress")).filter(|address| receives && !address.is_empty())
}

fn account(facade: &EntityFacade, id: &str) -> Result<Option<EntityValue>, LibError> {
    facade
        .find(ACCOUNT)?
        .eq("emailAccountId", Value::string(id))
        .use_cache(false)
        .disable_authz()
        .one()
}

/// The item a record of the server's own claims: by `itemType` and `itemId` (an `EmailAccount` or `EmailIdentity`
/// read here, so a post-service rule checks what was written; a `ChatBridge` or `PhoneNumber` by its id), else
/// `claimKind` and `itemKey` as given. `None` when the record claims nothing (an external account's address).
fn item_of(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<Option<(ClaimKind, String)>, LibError> {
    let id = text(parameters, "itemId");
    match (text(parameters, "itemType").as_deref(), id) {
        (Some("EmailAccount"), Some(id)) => Ok(account(facade, &id)?
            .and_then(|row| receiving_address(&row))
            .map(|key| (ClaimKind::MailAddress, key))),
        (Some("EmailIdentity"), Some(id)) => {
            let identity = facade
                .find(IDENTITY)?
                .eq("emailIdentityId", Value::string(&id))
                .use_cache(false)
                .disable_authz()
                .one()?;
            let Some(identity) = identity else {
                return Ok(None);
            };
            let receives = account(facade, &field(&identity, "emailAccountId"))?
                .is_some_and(|row| receiving_address(&row).is_some());
            let address = field(&identity, "emailAddress");
            Ok((receives && !address.is_empty()).then_some((ClaimKind::MailAddress, address)))
        }
        (Some("ChatBridge"), Some(id)) => Ok(Some((ClaimKind::ChatBridge, id))),
        (Some("PhoneNumber"), Some(id)) => Ok(Some((ClaimKind::PhoneNumber, id))),
        (Some(other), _) if !other.is_empty() => Err(LibError::new(format!("unknown itemType {other}"))),
        _ => {
            let kind = text(parameters, "claimKind").unwrap_or_default();
            let kind = ClaimKind::from_id(&kind).ok_or_else(|| LibError::new(format!("unknown claimKind {kind}")))?;
            Ok(text(parameters, "itemKey").map(|key| (kind, key)))
        }
    }
}

/// `check#ReplicaClaim`: on an all-in-one node, the paired node whose replica holds the item, with an error naming the
/// item and the node (unless `refuse` is false). Nothing on another node.
fn check_replica_claim(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let mut out = BTreeMap::new();
    if !all_in_one() {
        return Ok(out);
    }
    let Some((kind, key)) = item_of(parameters, facade)? else {
        return Ok(out);
    };
    let Some(node) = claims::replica_claim(facade, kind, &key)? else {
        return Ok(out);
    };
    let message = claims::claimed_message(kind, &key, &format!("paired node {node}"));
    let refuse = !matches!(text(parameters, "refuse").as_deref(), Some("false" | "N"));
    if refuse {
        tracing::warn!("The server's own {} {key} was refused: {message}", kind.label());
        let added = ExecutionContext::with_current(|ec| ec.message().add_error(message.clone()));
        if added.is_none() {
            return Err(LibError::new(message));
        }
    }
    out.insert("pairedNodeId".to_string(), Value::string(&node));
    out.insert("claimedMessage".to_string(), Value::string(&message));
    Ok(out)
}

/// `get#ReplicaConflicts`: the records of the paired nodes' sets not applied for a claim of the server's own item.
fn get_replica_conflicts(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let node = text(parameters, "pairedNodeId");
    let rows = crate::edge::replica::conflicts::list_conflicts(facade, node.as_deref())?;
    let list = rows
        .into_iter()
        .map(|conflict| {
            let label = ClaimKind::from_id(&conflict.claim_kind).map_or("item", ClaimKind::label);
            let mut row = indexmap::IndexMap::default();
            row.insert("pairedNodeId".to_string(), Value::string(&conflict.paired_node_id));
            row.insert("setName".to_string(), Value::string(&conflict.set_name));
            row.insert("entityName".to_string(), Value::string(&conflict.entity_name));
            row.insert("pkHash".to_string(), Value::string(&conflict.pk_hash));
            row.insert("claimKind".to_string(), Value::string(&conflict.claim_kind));
            row.insert("claimLabel".to_string(), Value::string(label));
            row.insert("itemKey".to_string(), Value::string(&conflict.item_key));
            row.insert("ownerText".to_string(), Value::string(&conflict.owner_text));
            row.insert(
                "firstDate".to_string(),
                conflict.first_date.map_or(Value::Null, stamp_value),
            );
            row.insert(
                "lastDate".to_string(),
                conflict.last_date.map_or(Value::Null, stamp_value),
            );
            Value::Map(row)
        })
        .collect();
    Ok(BTreeMap::from([("conflictList".to_string(), Value::List(list))]))
}

/// `recheck#ReplicaConflicts`: apply each record whose item the server no longer holds.
fn recheck_replica_conflicts(parameters: &BTreeMap<String, Value>, facade: &EntityFacade) -> Out {
    let node = text(parameters, "pairedNodeId");
    let factory = ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| LibError::new("no execution context"))?;
    let applied = crate::edge::replica::conflicts::recheck_conflicts(&factory, facade, node.as_deref())?;
    Ok(BTreeMap::from([(
        "applied".to_string(),
        Value::I64(i64::try_from(applied).unwrap_or(i64::MAX)),
    )]))
}

#[cfg(test)]
#[path = "edge_claims_tests.rs"]
mod tests;
