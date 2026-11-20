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

//! The lease services of `org.moqui.security.VaultServices` (plans/Vault-Instances.md §3.7): ask, read and list
//! leases, approve one (the approve role and a recent sign-in, on any cluster member), revoke one, and the lease key
//! that approvers check. The SSH admin shell lists and revokes but never approves: an approval is made in the member's
//! client, where their passphrase is.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};
use moqui_vault::text::hex;
use serde_json::{json, Value as Json};

use super::{
    approval_json, approval_of, key, lease_json, lease_of_row, mark_ended, registry, tell_member, Ask, OPEN, REQUESTED,
    REVOKED,
};
use crate::common::{
    authz_off, fail, field, find, has_permission, int, int_field, membership, now, now_millis, out, refused,
    require_recent_auth_to, required, text, user, value, vault_for, vault_row, Access, Out, Params, ITEM, LEASE,
    MEMBER, OPERATIONAL,
};
use crate::events::{self, Event};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, ask_vault_lease)?;
    moqui_core::rust_service!(map, get_vault_lease)?;
    moqui_core::rust_service!(map, list_vault_leases)?;
    moqui_core::rust_service!(map, approve_vault_lease)?;
    moqui_core::rust_service!(map, revoke_vault_lease)?;
    moqui_core::rust_service!(map, get_vault_lease_key)?;
    Ok(())
}

/// The bindings a caller names: a list, or text with one per line or comma.
pub(crate) fn bindings_param(params: &Params) -> Result<Vec<String>, LibError> {
    let found: Vec<String> = match params.get("bindings") {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Some(Value::String(text)) => text.split([',', '\n']).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let found: Vec<String> = found
        .into_iter()
        .map(|binding| binding.trim().to_string())
        .filter(|binding| !binding.is_empty())
        .collect();
    if found.is_empty() {
        return Err(fail("bindings is required"));
    }
    Ok(found)
}

/// The vault's members with the approve role: whom a waiting lease is shown to.
pub(crate) fn approvers(facade: &EntityFacade, vault_id: &str) -> Result<Vec<String>, LibError> {
    let vault = vault_row(facade, vault_id, false)?;
    let rows = find(facade, MEMBER)?
        .eq("vaultId", Value::string(vault_id))
        .eq("keyVersion", Value::I64(int_field(&vault, "keyVersion")))
        .eq("roleEnumId", Value::string("VrApprove"))
        .list()?;
    Ok(rows.iter().map(|row| field(row, "userId")).collect())
}

/// Tells the vault's approvers a lease waits for them (topic `VaultLeaseWaiting`), with a link to its dialog. A
/// notification that cannot be sent is logged: the lease waits on the vault's page either way.
pub(crate) fn notify_approvers(facade: &EntityFacade, lease: &moqui_vault::lease::Lease) {
    let users = match approvers(facade, &lease.vault_id) {
        Ok(users) if !users.is_empty() => users,
        Ok(_) => return,
        Err(err) => {
            tracing::error!("Vault lease {}: its approvers were not found: {err}", lease.lease_id);
            return;
        }
    };
    let mut message = ValueMap::default();
    message.insert("leaseId".to_string(), Value::string(&lease.lease_id));
    message.insert("vaultId".to_string(), Value::string(&lease.vault_id));
    message.insert("operation".to_string(), Value::string(&lease.operation));
    message.insert("itemCount".to_string(), Value::I64(lease.items.len() as i64));
    let mut note = moqui_core::notify::NotificationMessage::new();
    note.topic("VaultLeaseWaiting")
        .user_ids(users)
        .message_map(message)
        .title(format!("A lease waits for your approval: {}", lease.operation))
        .link(format!("/qapps/system/Vaults/Lease?leaseId={}", lease.lease_id));
    if let Err(err) = note.send() {
        tracing::error!("Vault lease {}: its approvers were not notified: {err}", lease.lease_id);
    }
}

/// Who may read a lease: a member of its vault, its requester, or a holder of `VAULT_ADMIN`.
fn may_read(facade: &EntityFacade, row: &EntityValue, me: &str) -> Result<bool, LibError> {
    if field(row, "requestedByUserId") == me || has_permission("VAULT_ADMIN")? {
        return Ok(true);
    }
    let vault = vault_row(facade, &field(row, "vaultId"), false)?;
    Ok(membership(facade, &vault, me)?.is_some())
}

/// A lease's state as its readers see it: one past its time limit has ended, whatever its row still says (its member
/// dropped its keys at the limit; the row is marked at its next use or the member's next start).
fn shown_status(row: &EntityValue) -> String {
    let status = field(row, "statusId");
    if [REQUESTED, OPEN].contains(&status.as_str()) && int_field(row, "expiresAt") <= now_millis() {
        return super::ENDED.to_string();
    }
    status
}

fn lease_row(facade: &EntityFacade, lease_id: &str, lock: bool) -> Result<EntityValue, LibError> {
    find(facade, LEASE)?
        .eq("leaseId", Value::string(lease_id))
        .for_update(lock)
        .one()?
        .ok_or_else(|| fail(format!("No lease {lease_id}")))
}

/// `ask#VaultLease`: a lease of an operational vault's items for an operation, asked by a member of the vault or a
/// holder of `VAULT_ADMIN` (an operation a person starts by hand; the framework's operations ask in Rust). The signed
/// statement to approve.
fn ask_vault_lease(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let vault = vault_row(facade, &vault_id, false).map_err(|_| fail(format!("No vault {vault_id}")))?;
    let member = membership(facade, &vault, &me)?.is_some();
    if !member && !has_permission("VAULT_ADMIN")? {
        return Err(fail(format!("No vault {vault_id}")));
    }
    let bindings = bindings_param(params)?;
    let operation = required(params, "operation")?;
    let limit = int(params, "limitMinutes")
        .filter(|minutes| *minutes > 0)
        .map(|minutes| Duration::from_secs(u64::try_from(minutes).unwrap_or(0).saturating_mul(60)));
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| fail("No execution context is entered"))?;
    let lease = registry(&factory).ask(
        facade,
        &Ask {
            vault_id: &vault_id,
            bindings: &bindings,
            operation: &operation,
            limit,
            environment_id: None,
        },
    )?;
    Ok(out([
        ("leaseId", Value::string(&lease.lease_id)),
        ("lease", value(&lease_json(&lease))),
    ]))
}

/// A lease row for a reader: its state and the signed statement (which the dialog shows, never a column), with the
/// item keys an approver unwraps and seals.
fn lease_answer(
    facade: &EntityFacade,
    row: &EntityValue,
    held_here: bool,
) -> Result<BTreeMap<String, Value>, LibError> {
    let lease = lease_of_row(row)?;
    let mut wrapped = serde_json::Map::new();
    for item in &lease.items {
        if let Some(item_row) = find(facade, ITEM)?
            .eq("vaultItemId", Value::string(&item.item_id))
            .one()?
        {
            wrapped.insert(item.item_id.clone(), json!(field(&item_row, "wrappedItemKey")));
        }
    }
    let lease_key = key::verifying_key()?;
    let mut answer = out([
        ("leaseId", Value::string(&lease.lease_id)),
        ("lease", value(&lease_json(&lease))),
        ("statusId", Value::string(shown_status(row))),
        ("wrappedItemKeys", value(&Json::Object(wrapped))),
        ("leaseKey", Value::string(hex(lease_key.as_bytes()))),
        (
            "leaseKeyFingerprint",
            Value::string(moqui_vault::fingerprint::lease_key(&lease_key)),
        ),
        ("held", Value::Bool(held_here)),
    ]);
    for name in [
        "requestedByUserId",
        "requestedDate",
        "approvedByUserId",
        "approvedDate",
        "endedDate",
        "machineAccess",
        "environmentId",
    ] {
        if let Some(given) = row.get(name).filter(|given| !moqui_lib::object::is_empty(given)) {
            answer.insert(name.to_string(), given.clone());
        }
    }
    Ok(answer)
}

/// `get#VaultLease`: a lease to a member of its vault, its requester or a holder of `VAULT_ADMIN`.
fn get_vault_lease(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let lease_id = required(params, "leaseId")?;
    let row = lease_row(facade, &lease_id, false)?;
    if !may_read(facade, &row, &me)? {
        return Err(fail(format!("No lease {lease_id}")));
    }
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| fail("No execution context is entered"))?;
    let held = registry(&factory).holds(&lease_id);
    lease_answer(facade, &row, held)
}

/// A lease row in a list: what the vault screens show, never a sealed key.
fn list_row(row: &EntityValue) -> Value {
    let mut map = ValueMap::default();
    for name in [
        "leaseId",
        "vaultId",
        "keyVersion",
        "operation",
        "clusterMemberId",
        "expiresAt",
        "statusId",
        "requestedByUserId",
        "requestedDate",
        "approvedByUserId",
        "approvedDate",
        "endedDate",
        "machineAccess",
        "environmentId",
    ] {
        if let Some(given) = row.get(name) {
            map.insert(name.to_string(), given.clone());
        }
    }
    let items: Json = serde_json::from_str(&field(row, "leaseItems")).unwrap_or(Json::Null);
    let bindings: Vec<String> = items
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["binding"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    map.insert("bindings".to_string(), Value::string(bindings.join(", ")));
    map.insert("statusId".to_string(), Value::string(shown_status(row)));
    Value::Map(map)
}

/// `list#VaultLeases`: a vault's leases to its members (by status when given); with no vault, those waiting for the
/// caller's approval in any vault and those they asked for, or every lease to a holder of `VAULT_ADMIN`.
fn list_vault_leases(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = text(params, "vaultId");
    let status = text(params, "statusId");
    let limit = int(params, "limit").filter(|n| (1..=1000).contains(n)).unwrap_or(200);
    let mut rows: Vec<EntityValue> = Vec::new();
    if !vault_id.is_empty() {
        vault_for(facade, &vault_id, &me, Access::Read, false)?;
        let mut found = find(facade, LEASE)?.eq("vaultId", Value::string(&vault_id));
        if !status.is_empty() {
            found = found.eq("statusId", Value::string(&status));
        }
        rows.extend(found.order_by("requestedDate", true).limit(limit).list()?.into_values());
    } else if has_permission("VAULT_ADMIN")? {
        let mut found = find(facade, LEASE)?;
        if !status.is_empty() {
            found = found.eq("statusId", Value::string(&status));
        }
        rows.extend(found.order_by("requestedDate", true).limit(limit).list()?.into_values());
    } else {
        let approving = find(facade, MEMBER)?
            .eq("userId", Value::string(&me))
            .eq("roleEnumId", Value::string("VrApprove"))
            .list()?;
        for member in approving.iter() {
            let waiting = find(facade, LEASE)?
                .eq("vaultId", Value::string(field(member, "vaultId")))
                .eq("keyVersion", Value::I64(int_field(member, "keyVersion")))
                .eq("statusId", Value::string(REQUESTED))
                .list()?;
            rows.extend(waiting.into_values());
        }
        let mine = find(facade, LEASE)?
            .eq("requestedByUserId", Value::string(&me))
            .order_by("requestedDate", true)
            .limit(limit)
            .list()?;
        rows.extend(mine.into_values());
        if !status.is_empty() {
            rows.retain(|row| field(row, "statusId") == status);
        }
        rows.sort_by_key(|row| std::cmp::Reverse(field(row, "requestedDate")));
        rows.dedup_by_key(|row| field(row, "leaseId"));
        rows.truncate(usize::try_from(limit).unwrap_or(200));
    }
    Ok(out([("leaseList", Value::List(rows.iter().map(list_row).collect()))]))
}

/// `approve#VaultLease`: a member with the approve role, with a recent sign-in, on any cluster member: the item keys
/// their client sealed to the lease, their signature and the `VeLeaseApproved` event in one transaction; the member that
/// holds the lease is woken at its commit.
fn approve_vault_lease(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let lease_id = required(params, "leaseId")?;
    let mut row = lease_row(facade, &lease_id, true)?;
    let vault_id = field(&row, "vaultId");
    let vault =
        vault_for(facade, &vault_id, &me, Access::Read, false).map_err(|_| fail(format!("No lease {lease_id}")))?;
    if field(&vault, "vaultKindEnumId") != OPERATIONAL {
        return Err(fail(format!("No lease {lease_id}")));
    }
    vault_for(facade, &vault_id, &me, Access::Approve, false)
        .map_err(|_| fail("Approving a lease needs the approve role in its vault"))?;
    require_recent_auth_to("approve this lease")?;
    if field(&row, "statusId") != REQUESTED {
        return Err(fail(format!("The lease {lease_id} is not waiting for approval")));
    }
    // two people for a lease: its requester approves it only when no other member holds the approve role
    if field(&row, "requestedByUserId") == me && approvers(facade, &vault_id)?.iter().any(|other| *other != me) {
        return Err(fail(
            "You asked for this lease: another member with the approve role approves it (reasonCode selfApproval)",
        ));
    }
    let lease = lease_of_row(&row)?;
    // the row as the server signed it: a database writer cannot put its own key or items on it
    let lease_key = key::verifying_key()?;
    lease.verify(&lease_key, &[lease_key]).map_err(refused)?;
    if lease.expires_at <= now_millis() {
        return Err(fail(format!("The lease {lease_id} is past its time limit")));
    }
    if int_field(&vault, "keyVersion") != int_field(&row, "keyVersion") {
        return Err(fail("The vault key changed since the lease was asked for: ask again"));
    }
    let approval = approval_of(&crate::common::json(params, "approval")?)?;
    if approval.approver_user_id != me {
        return Err(fail("An approval is signed by its approver, the caller"));
    }
    let mine = crate::personal::my_key(facade, &me)?;
    let verify = crate::personal::public_of(&mine)?.verify;
    approval.verify(&lease, &verify).map_err(refused)?;
    let named: Vec<&str> = lease.items.iter().map(|item| item.item_id.as_str()).collect();
    let mut sealed: Vec<&str> = approval.sealed.iter().map(|s| s.item_id.as_str()).collect();
    sealed.sort_unstable();
    let mut expected = named.clone();
    expected.sort_unstable();
    if sealed != expected {
        return Err(fail("Seal the key of every item the lease names, once each"));
    }
    row.set(
        "sealedItemKeys",
        Value::string(approval_json(&approval)["sealed"].to_string()),
    );
    row.set("approvedByUserId", Value::string(&me));
    row.set("approverKeyId", Value::string(field(&mine, "personalKeyId")));
    row.set("approvalSignature", Value::string(hex(&approval.signature)));
    row.set("approvedDate", now());
    row.set("statusId", Value::string(OPEN));
    row.update(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeLeaseApproved",
            vault_id: Some(&vault_id),
            lease_id: Some(&lease_id),
            detail: Some(format!("{}: {} items", lease.operation, lease.items.len())),
            ..Event::default()
        },
    )?;
    tell_member(facade, &lease_id, &lease.cluster_member, "approved");
    Ok(out([
        ("leaseId", Value::string(lease_id)),
        ("statusId", Value::string(OPEN)),
    ]))
}

/// `revoke#VaultLease`: a lease waiting or open, revoked by a manager or approver of its vault, its requester, or a
/// holder of `VAULT_ADMIN`: its row marked revoked with its event, and the member that holds it forgets its keys.
fn revoke_vault_lease(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let lease_id = required(params, "leaseId")?;
    let row = lease_row(facade, &lease_id, false)?;
    let vault = vault_row(facade, &field(&row, "vaultId"), false)?;
    let role = membership(facade, &vault, &me)?
        .map(|member| field(&member, "roleEnumId"))
        .unwrap_or_default();
    let allowed = ["VrManage", "VrApprove"].contains(&role.as_str())
        || field(&row, "requestedByUserId") == me
        || has_permission("VAULT_ADMIN")?;
    if !allowed {
        return Err(fail(format!("No lease {lease_id}")));
    }
    if ![REQUESTED, OPEN].contains(&field(&row, "statusId").as_str()) {
        return Err(fail(format!("The lease {lease_id} has ended")));
    }
    mark_ended(facade, &lease_id, REVOKED, None, "revoked")?;
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| fail("No execution context is entered"))?;
    registry(&factory).forget(&lease_id);
    tell_member(facade, &lease_id, &field(&row, "clusterMemberId"), "revoked");
    Ok(out([
        ("leaseId", Value::string(lease_id)),
        ("statusId", Value::string(REVOKED)),
    ]))
}

/// `get#VaultLeaseKey`: the server's lease key and its fingerprint, for a member to compare once (by another channel,
/// with the operator who reads it from the server's console) and store as a key check.
fn get_vault_lease_key(_params: &Params, _facade: &EntityFacade) -> Out {
    user()?;
    let lease_key = key::verifying_key()?;
    Ok(out([
        ("leaseKey", Value::string(hex(lease_key.as_bytes()))),
        (
            "fingerprint",
            Value::string(moqui_vault::fingerprint::lease_key(&lease_key)),
        ),
    ]))
}
