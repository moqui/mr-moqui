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

//! Organization recovery, split k of n (plans/archive/Vault-Instances.md §3.10, §3.12, §3.13, V4; the owner's decision of
//! Nov 19, 2026).
//!
//! - [`keys`]: a holder with `VAULT_RECOVERY` and a recent sign-in makes the organization's recovery key in their
//!   client, by user group or by instance: its public halves signed by them, its seed split k of n, one share wrapped
//!   to each holder's active personal key (each holder holds `VAULT_RECOVERY`, and the maker has checked their key)
//!   and signed by the maker. The server keeps the record and the holders' sealed shares.
//! - Recovery on a vault: the recovery key is listed in the vault's member list as a manager, under
//!   `recovery:<recoveryKeyId>`, and the vault key is wrapped to it as to a member ([`check_recovery_member`], through
//!   `create#Vault` and `add#VaultMember`), by a manager who checked the recovery key's maker. A single-secret vault has
//!   no member list, so it never gets one. Turning it off is `remove#VaultMember` of that member, which rotates the
//!   key; an operational vault keeps recovery on or two members.
//! - [`sessions`]: a recovery. The recovering holder's client makes an ephemeral key and starts a session naming the
//!   vault, the member and their new personal key (which that holder checked); k − 1 other holders seal their shares to
//!   it; the recovering client combines them, opens the vault key through the recovery member's wrap, and completes
//!   with the next member list (signed by the recovery key, giving that member the new key and changing nothing else)
//!   and the vault key wrapped to the new key. Each participating holder needs `VAULT_RECOVERY` and a recent sign-in.
//!   A start, each share, the end and a cancel are `VaultEvent`s, and a start and the end are notices to every member.

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::fingerprint;
use moqui_vault::keyversion::{recovery_key_of, KeyVersion, Member, Role};
use moqui_vault::ops::shapes;
use moqui_vault::personal::PersonalPublic;
use moqui_vault::recovery::RecoveryKeyRecord;
use serde_json::json;

use crate::common::{
    fail, field, find, int_field, refused, PERSONAL_KEY, RECOVERY_DELIVERY, RECOVERY_HOLDER, RECOVERY_KEY,
    RECOVERY_SESSION,
};
use crate::personal::{checks_by, public_of};

pub mod keys;
pub mod sessions;

pub(crate) fn register(
    map: &mut std::collections::BTreeMap<&'static str, moqui_core::extension::RustServiceFn>,
) -> Result<(), String> {
    keys::register(map)?;
    sessions::register(map)
}

/// The sealed permission a holder needs (§3.10).
const VAULT_RECOVERY: &str = "VAULT_RECOVERY";

/// The notification topic of a recovery's start and end, to every member of the vault.
const TOPIC: &str = "VaultRecovery";

/// Refuses a caller without `VAULT_RECOVERY`, or whose sign-in is not recent (`require#RecentAuth`'s rule): what every
/// holder's step of a recovery needs.
pub(crate) fn require_holder_rights() -> Result<(), LibError> {
    require_permission()?;
    if moqui_core::user::recent_auth(RECENT_AUTH_MILLIS) {
        return Ok(());
    }
    Err(fail(
        "Confirm it is you: sign in again to take part in a vault recovery (reasonCode reauthRequired)",
    ))
}

/// How recent a holder's sign-in must be: `require#RecentAuth`'s five minutes.
const RECENT_AUTH_MILLIS: i64 = 5 * 60 * 1000;

/// Refuses a caller without `VAULT_RECOVERY`.
pub(crate) fn require_permission() -> Result<(), LibError> {
    if moqui_core::user::has_permission(VAULT_RECOVERY).map_err(|err| fail(err.message()))? {
        return Ok(());
    }
    Err(fail("Vault recovery needs the VAULT_RECOVERY permission"))
}

/// Whether `user_id` holds `VAULT_RECOVERY`.
pub(crate) fn user_may_hold(user_id: &str) -> Result<bool, LibError> {
    moqui_core::user::user_has_permission(user_id, VAULT_RECOVERY).map_err(|err| fail(err.message()))
}

/// A recovery key's row, ended or not.
pub(crate) fn key_row(facade: &EntityFacade, recovery_key_id: &str) -> Result<Option<EntityValue>, LibError> {
    find(facade, RECOVERY_KEY)?
        .eq("recoveryKeyId", Value::string(recovery_key_id))
        .one()
}

/// The record a recovery key's row holds, as its maker signed it.
pub(crate) fn record_of_row(row: &EntityValue) -> Result<RecoveryKeyRecord, LibError> {
    shapes::record_from(&json!({
        "recovery_key_id": field(row, "recoveryKeyId"), "public": field(row, "publicKey"),
        "k": int_field(row, "sharesNeeded"), "n": int_field(row, "sharesTotal"),
        "maker_user_id": field(row, "makerUserId"), "maker_key_id": field(row, "makerKeyId"),
        "signature": field(row, "signature"),
    }))
    .map_err(refused)
}

/// The maker's personal key's public halves, active or since replaced: the key the record is checked with.
pub(crate) fn maker_public(facade: &EntityFacade, record: &RecoveryKeyRecord) -> Result<PersonalPublic, LibError> {
    let row = find(facade, PERSONAL_KEY)?
        .eq("personalKeyId", Value::string(&record.maker_key_id))
        .one()?
        .filter(|row| field(row, "userId") == record.maker_user_id)
        .ok_or_else(|| fail("The recovery key's maker has no such personal key"))?;
    public_of(&row)
}

pub(crate) fn holder_share_json(row: &EntityValue) -> serde_json::Value {
    json!({
        "recovery_key_id": field(row, "recoveryKeyId"), "holder_user_id": field(row, "userId"),
        "holder_key_id": field(row, "personalKeyId"), "index": int_field(row, "shareIndex"),
        "sealed": field(row, "sealedShare"), "signature": field(row, "signature"),
    })
}

/// The holder row of `user_id` for a recovery key.
pub(crate) fn holder_row(
    facade: &EntityFacade,
    recovery_key_id: &str,
    user_id: &str,
) -> Result<Option<EntityValue>, LibError> {
    find(facade, RECOVERY_HOLDER)?
        .eq("recoveryKeyId", Value::string(recovery_key_id))
        .eq("userId", Value::string(user_id))
        .one()
}

/// The persons of a member list: every member but a recovery key.
pub(crate) fn people(entry: &KeyVersion) -> impl Iterator<Item = &Member> {
    entry.members.iter().filter(|m| recovery_key_of(&m.user_id).is_none())
}

/// The recovery key a member list lists, if any.
pub(crate) fn listed_recovery(entry: &KeyVersion) -> Option<&Member> {
    entry.members.iter().find(|m| recovery_key_of(&m.user_id).is_some())
}

/// Checks a member list's recovery member (`recovery:<id>`) as `create#Vault` and `add#VaultMember` take it: a
/// recovery key that has not ended, listed under its own id with its signed public halves and the manage role, its
/// record signed by its maker, and that maker's key checked by the signer (or the signer is the maker).
pub(crate) fn check_recovery_member(facade: &EntityFacade, member: &Member, signer: &Member) -> Result<(), LibError> {
    let recovery_key_id = recovery_key_of(&member.user_id).unwrap_or_default();
    let row = key_row(facade, recovery_key_id)?
        .filter(|row| field(row, "thruDate").is_empty())
        .ok_or_else(|| fail(format!("No recovery key {recovery_key_id}")))?;
    let record = record_of_row(&row)?;
    if member.key_id != recovery_key_id || member.public != record.public || member.role != Role::Manage {
        return Err(fail(
            "A recovery key is listed under its own id, with its public keys and the manage role",
        ));
    }
    let maker = maker_public(facade, &record)?;
    record.verify(&maker.verify).map_err(refused)?;
    if signer.user_id == record.maker_user_id && signer.public == maker {
        return Ok(());
    }
    let checks = checks_by(facade, &signer.key_id)?;
    fingerprint::require_checked(&checks, &signer.public.verify, &record.maker_user_id, &maker).map_err(|_| {
        fail(format!(
            "Check the key fingerprint of {}, who made recovery key {recovery_key_id}, before turning recovery on",
            record.maker_user_id
        ))
    })
}

/// Refuses a member list that lists more than one recovery key.
pub(crate) fn check_one_recovery(entry: &KeyVersion) -> Result<(), LibError> {
    if entry
        .members
        .iter()
        .filter(|m| recovery_key_of(&m.user_id).is_some())
        .count()
        > 1
    {
        return Err(fail("A vault lists one recovery key"));
    }
    Ok(())
}

/// Marks a recovery key used: a recovery rebuilt it whole in the recovering holder's client (the limit of Shamir,
/// §3.12). The managers of every vault that lists it are told to replace it (`remove#VaultMember` of it with a new
/// recovery key listed in the same rotation), and the key list shows it as used.
pub(crate) fn mark_used(facade: &EntityFacade, recovery_key_id: &str) -> Result<(), LibError> {
    if let Some(mut row) = key_row(facade, recovery_key_id)? {
        row.set("usedDate", crate::common::now());
        row.update(facade)?;
    }
    let listed = find(facade, crate::common::MEMBER)?
        .eq(
            "userId",
            Value::string(moqui_vault::keyversion::recovery_member_id(recovery_key_id)),
        )
        .list()?;
    let mut managers = std::collections::BTreeSet::new();
    let mut vaults = std::collections::BTreeSet::new();
    for wrap in listed.iter() {
        let vault_id = field(wrap, "vaultId");
        let Ok(vault) = crate::common::vault_row(facade, &vault_id, false) else {
            continue;
        };
        if int_field(&vault, "keyVersion") != int_field(wrap, "keyVersion") || !vaults.insert(vault_id.clone()) {
            continue;
        }
        let rows = find(facade, crate::common::MEMBER)?
            .eq("vaultId", Value::string(&vault_id))
            .eq("keyVersion", Value::I64(int_field(&vault, "keyVersion")))
            .eq("roleEnumId", Value::string("VrManage"))
            .list()?;
        for row in rows.iter() {
            let user_id = field(row, "userId");
            if recovery_key_of(&user_id).is_none() {
                managers.insert(user_id);
            }
        }
    }
    if managers.is_empty() {
        return Ok(());
    }
    let mut message = moqui_lib::object::ValueMap::default();
    message.insert("recoveryKeyId".to_string(), Value::string(recovery_key_id));
    let vault_ids: Vec<String> = vaults.into_iter().collect();
    message.insert("vaultIds".to_string(), Value::string(vault_ids.join(",")));
    moqui_core::notify::NotificationMessage::new()
        .topic(TOPIC)
        .user_ids(managers)
        .title(format!(
            "Recovery key {recovery_key_id} was used: replace it in the vaults you manage that list it"
        ))
        .message_map(message)
        .persist(true)
        .send()
        .map_err(|err| fail(format!("The notice to replace a used recovery key: {}", err.message())))
}

/// Deletes a deleted vault's recovery sessions and their shares.
pub(crate) fn forget_vault(facade: &EntityFacade, vault_id: &str) -> Result<(), LibError> {
    let sessions = find(facade, RECOVERY_SESSION)?
        .eq("vaultId", Value::string(vault_id))
        .list()?;
    for session in sessions.iter() {
        let shares = find(facade, RECOVERY_DELIVERY)?
            .eq("recoverySessionId", Value::string(field(session, "recoverySessionId")))
            .list()?;
        for share in shares.iter() {
            share.delete(facade)?;
        }
        session.delete(facade)?;
    }
    Ok(())
}

/// Tells every person of a vault's member list about a recovery: a notification persisted in the transaction of the
/// change, so a recovery whose notice cannot be written does not happen, and sent to them only once it commits.
pub(crate) fn notify_members(
    entry: &KeyVersion,
    vault_id: &str,
    subject_user_id: &str,
    title: &str,
) -> Result<(), LibError> {
    let users: Vec<String> = people(entry).map(|m| m.user_id.clone()).collect();
    let mut message = moqui_lib::object::ValueMap::default();
    message.insert("vaultId".to_string(), Value::string(vault_id));
    message.insert("subjectUserId".to_string(), Value::string(subject_user_id));
    moqui_core::notify::NotificationMessage::new()
        .topic(TOPIC)
        .user_ids(users)
        .title(title)
        .message_map(message)
        .persist(true)
        .send()
        .map_err(|err| fail(format!("A recovery's notice to the vault's members: {}", err.message())))
}
