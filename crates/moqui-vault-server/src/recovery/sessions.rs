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

//! A recovery (plans/archive/Vault-Instances.md §3.10): a session the recovering holder starts, the k − 1 shares other holders
//! seal to it, and its end, which gives the member their new personal key in the vault's member list. The server
//! stores signed records and ciphertext; the shares, the recovery key and the vault key stay in the clients.

use std::collections::{BTreeMap, BTreeSet};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::fingerprint;
use moqui_vault::keyversion::{recovery_member_id, KeyVersion, Role};
use moqui_vault::ops::shapes;
use moqui_vault::recovery::RecoverySession;
use moqui_vault::text::hex;
use serde_json::{json, Value as Json};

use super::{
    holder_row, holder_share_json, key_row, listed_recovery, maker_public, notify_members, people, record_of_row,
    require_holder_rights, require_permission,
};
use crate::common::{
    authz_off, bump_revision, check_id, fail, field, find, int_field, json, membership, now, out, refused, required,
    user, value, vault_row, Out, Params, MAX_WRAP_HEX, MEMBER, PERSONAL_KEY, RECOVERY_DELIVERY, RECOVERY_SESSION,
    SHARED_KEY,
};
use crate::events::{self, Event};
use crate::members::{
    chain_entries, check_revision, entry_param, store_entry, store_wrap, verify_chain, version, wrap_json, wrap_param,
    wraps_param,
};
use crate::personal::{active_key, checks_by, my_key, public_of};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, start_vault_recovery)?;
    moqui_core::rust_service!(map, list_vault_recovery_sessions)?;
    moqui_core::rust_service!(map, deliver_vault_recovery_share)?;
    moqui_core::rust_service!(map, get_vault_recovery_session)?;
    moqui_core::rust_service!(map, complete_vault_recovery)?;
    moqui_core::rust_service!(map, cancel_vault_recovery)?;
    Ok(())
}

const OPEN: &str = "VrsOpen";
const COMPLETED: &str = "VrsCompleted";
const CANCELLED: &str = "VrsCancelled";

/// How long a session takes shares and may complete: a day. Its ephemeral key is in the recovering client's memory
/// only, so a session outlives no restart of that client in any case.
const SESSION_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// A session's row as the vault crate's record.
fn session_of_row(row: &EntityValue) -> Result<RecoverySession, LibError> {
    shapes::session_from(&session_json(row)).map_err(refused)
}

fn session_json(row: &EntityValue) -> Json {
    json!({
        "session_id": field(row, "recoverySessionId"), "recovery_key_id": field(row, "recoveryKeyId"),
        "vault_id": field(row, "vaultId"), "subject_user_id": field(row, "subjectUserId"),
        "subject_key_id": field(row, "subjectKeyId"), "subject_public": field(row, "subjectPublic"),
        "ephemeral": field(row, "ephemeralPublic"), "holder_user_id": field(row, "holderUserId"),
        "holder_key_id": field(row, "holderKeyId"), "signature": field(row, "signature"),
    })
}

fn session_row(facade: &EntityFacade, session_id: &str, lock: bool) -> Result<EntityValue, LibError> {
    find(facade, RECOVERY_SESSION)?
        .eq("recoverySessionId", Value::string(session_id))
        .for_update(lock)
        .one()?
        .ok_or_else(|| fail(format!("No recovery session {session_id}")))
}

/// Refuses a session that has ended or run out.
fn require_open(row: &EntityValue) -> Result<(), LibError> {
    if field(row, "statusId") != OPEN {
        return Err(fail("That recovery has ended"));
    }
    if int_field(row, "expiresAt") < moqui_core::user::now_millis() {
        return Err(fail("That recovery ran out: start it again"));
    }
    Ok(())
}

/// The users who delivered a share to a session.
fn delivered_by(facade: &EntityFacade, session_id: &str) -> Result<Vec<EntityValue>, LibError> {
    Ok(find(facade, RECOVERY_DELIVERY)?
        .eq("recoverySessionId", Value::string(session_id))
        .order_by("deliveredDate", false)
        .list()?
        .into_values())
}

/// A vault's latest member-list entry, after checking that it lists `recovery_key_id` as its recovery key.
fn latest_with_recovery(
    facade: &EntityFacade,
    vault_id: &str,
    recovery_key_id: &str,
) -> Result<(Vec<KeyVersion>, KeyVersion), LibError> {
    let entries = chain_entries(facade, vault_id)?;
    let latest = entries
        .last()
        .cloned()
        .ok_or_else(|| fail("A vault has no key version"))?;
    let listed = listed_recovery(&latest).is_some_and(|m| m.user_id == recovery_member_id(recovery_key_id));
    if !listed {
        return Err(fail(format!(
            "Recovery is not on for vault {vault_id} with recovery key {recovery_key_id}"
        )));
    }
    Ok((entries, latest))
}

/// `start#VaultRecovery`: the recovering holder's session (`session`, signed in their client), as the module doc of
/// [`crate::recovery`] says.
fn start_vault_recovery(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_holder_rights()?;
    let mine = my_key(facade, &me)?;
    let my_key_id = field(&mine, "personalKeyId");
    let session = shapes::session_from(&json(params, "session")?).map_err(refused)?;
    let session_id = session.session_id.clone();
    check_id("sessionId", &session_id)?;
    let taken = find(facade, RECOVERY_SESSION)?
        .eq("recoverySessionId", Value::string(&session_id))
        .one()?
        .is_some();
    if taken {
        return Err(crate::common::id_not_available("Recovery session", &session_id));
    }
    if session.holder_user_id != me || session.holder_key_id != my_key_id {
        return Err(fail(
            "Sign the recovery session yourself, with your active personal key",
        ));
    }
    session.verify(&public_of(&mine)?.verify).map_err(refused)?;
    let key = key_row(facade, &session.recovery_key_id)?
        .filter(|row| field(row, "thruDate").is_empty())
        .ok_or_else(|| fail(format!("No recovery key {}", session.recovery_key_id)))?;
    let holds =
        holder_row(facade, &session.recovery_key_id, &me)?.is_some_and(|row| field(&row, "personalKeyId") == my_key_id);
    if !holds {
        return Err(fail(
            "You hold no share of this recovery key with your active personal key",
        ));
    }
    let vault = vault_row(facade, &session.vault_id, false)?;
    if field(&vault, "lockEnumId") != SHARED_KEY {
        return Err(fail("A single-secret vault has no recovery"));
    }
    let (_, latest) = latest_with_recovery(facade, &session.vault_id, &session.recovery_key_id)?;
    let subject = &session.subject_user_id;
    let Some(listed) = people(&latest).find(|m| m.user_id == *subject) else {
        return Err(fail(format!("{subject} is not a member of this vault")));
    };
    let active = active_key(facade, subject)?
        .filter(|row| field(row, "personalKeyId") == session.subject_key_id)
        .ok_or_else(|| fail(format!("The key named for {subject} is not their active personal key")))?;
    if public_of(&active)? != session.subject_public {
        return Err(fail(format!(
            "The key named for {subject} is not their active personal key"
        )));
    }
    if listed.key_id == session.subject_key_id {
        return Err(fail(format!(
            "{subject} holds a wrap to that key already: a recovery is to a new key"
        )));
    }
    // §3.5: nothing is wrapped to a changed key before it is checked again
    let checks = checks_by(facade, &my_key_id)?;
    fingerprint::require_checked(&checks, &public_of(&mine)?.verify, subject, &session.subject_public).map_err(
        |_| {
            fail(format!(
                "Check {subject}'s new key fingerprint before recovering their vault"
            ))
        },
    )?;

    let mut row = facade.make_value(RECOVERY_SESSION)?;
    row.set("recoverySessionId", Value::string(&session_id));
    row.set("recoveryKeyId", Value::string(&session.recovery_key_id));
    row.set("vaultId", Value::string(&session.vault_id));
    row.set("subjectUserId", Value::string(subject));
    row.set("subjectKeyId", Value::string(&session.subject_key_id));
    row.set("subjectPublic", Value::string(hex(&session.subject_public.to_bytes())));
    row.set("ephemeralPublic", Value::string(hex(session.ephemeral.as_bytes())));
    row.set("holderUserId", Value::string(&me));
    row.set("holderKeyId", Value::string(&my_key_id));
    row.set("signature", Value::string(hex(&session.signature)));
    row.set("statusId", Value::string(OPEN));
    row.set("expiresAt", Value::I64(moqui_core::user::now_millis() + SESSION_MILLIS));
    row.set("startedDate", now());
    row.create(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeRecoveryStarted",
            vault_id: Some(&session.vault_id),
            subject_user_id: Some(subject),
            personal_key_id: Some(&session.subject_key_id),
            detail: Some(format!(
                "session {session_id}, recovery key {}, {} shares needed",
                session.recovery_key_id,
                int_field(&key, "sharesNeeded")
            )),
            ..Event::default()
        },
    )?;
    notify_members(
        &latest,
        &session.vault_id,
        subject,
        &format!("A recovery of a vault you are a member of was started, for {subject}"),
    )?;
    Ok(out([("recoverySessionId", Value::string(session_id))]))
}

/// `list#VaultRecoverySessions`: to a holder with `VAULT_RECOVERY`, the open sessions of the recovery keys they hold:
/// each session as signed, the recovering holder's public halves (to check its signature against a key the caller
/// checked), the new key's fingerprint, the vault, and who has sealed a share to it.
fn list_vault_recovery_sessions(_params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_permission()?;
    let held = find(facade, crate::common::RECOVERY_HOLDER)?
        .eq("userId", Value::string(&me))
        .list()?;
    let mut listed = Vec::new();
    for holder in held.iter() {
        let sessions = find(facade, RECOVERY_SESSION)?
            .eq("recoveryKeyId", Value::string(field(holder, "recoveryKeyId")))
            .eq("statusId", Value::string(OPEN))
            .list()?;
        for row in sessions.iter().filter(|row| require_open(row).is_ok()) {
            let session_id = field(row, "recoverySessionId");
            let recovering = find(facade, PERSONAL_KEY)?
                .eq("personalKeyId", Value::string(field(row, "holderKeyId")))
                .one()?
                .map(|key| public_of(&key))
                .transpose()?;
            let subject = session_of_row(row)?.subject_public;
            let delivered: Vec<String> = delivered_by(facade, &session_id)?
                .iter()
                .map(|d| field(d, "userId"))
                .collect();
            let vault = find(facade, crate::common::VAULT)?
                .eq("vaultId", Value::string(field(row, "vaultId")))
                .one()?;
            listed.push(json!({
                "recoverySessionId": session_id, "session": session_json(row),
                "holderPublic": recovering.map(|public| hex(&public.to_bytes())).unwrap_or_default(),
                "subjectFingerprint": fingerprint::personal(&subject),
                "vaultKindEnumId": vault.as_ref().map(|v| field(v, "vaultKindEnumId")).unwrap_or_default(),
                "vaultName": vault.as_ref().map(|v| field(v, "vaultName")).unwrap_or_default(),
                "deliveredBy": delivered, "expiresAt": int_field(row, "expiresAt"),
                // the starter's holder row, which a holder's client checks against the maker's key before sealing
                "holderShare": holder_row(facade, &field(row, "recoveryKeyId"), &field(row, "holderUserId"))?
                    .map(|held| holder_share_json(&held)).unwrap_or(Json::Null),
            }));
        }
    }
    Ok(out([("sessionList", value(&Json::Array(listed)))]))
}

/// `deliver#VaultRecoveryShare`: another holder's share, sealed in their client to the session's ephemeral key and
/// signed with their active key. One share per holder per session; never the recovering holder's own.
fn deliver_vault_recovery_share(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_holder_rights()?;
    let mine = my_key(facade, &me)?;
    let my_key_id = field(&mine, "personalKeyId");
    let session_id = required(params, "recoverySessionId")?;
    let row = session_row(facade, &session_id, true)?;
    require_open(&row)?;
    if field(&row, "holderUserId") == me {
        return Err(fail("The recovering holder combines their own share in their client"));
    }
    let session = session_of_row(&row)?;
    let holds = holder_row(facade, &session.recovery_key_id, &me)?
        .is_some_and(|held| field(&held, "personalKeyId") == my_key_id);
    if !holds {
        return Err(fail(
            "You hold no share of this recovery key with your active personal key",
        ));
    }
    let share = shapes::session_share_from(&json(params, "share")?).map_err(refused)?;
    if share.session_id != session_id || share.holder_user_id != me || share.holder_key_id != my_key_id {
        return Err(fail(
            "Seal your own share to this session, signed with your active personal key",
        ));
    }
    if 2 * share.sealed.len() > MAX_WRAP_HEX {
        return Err(fail("A share is too long"));
    }
    share.verify(&session, &public_of(&mine)?.verify).map_err(refused)?;
    let already = find(facade, RECOVERY_DELIVERY)?
        .eq("recoverySessionId", Value::string(&session_id))
        .eq("userId", Value::string(&me))
        .one()?
        .is_some();
    if already {
        return Err(fail("You sealed your share to this recovery already"));
    }
    let mut delivery = facade.make_value(RECOVERY_DELIVERY)?;
    delivery.set("recoverySessionId", Value::string(&session_id));
    delivery.set("userId", Value::string(&me));
    delivery.set("personalKeyId", Value::string(&my_key_id));
    delivery.set("sealedShare", Value::string(hex(&share.sealed)));
    delivery.set("signature", Value::string(hex(&share.signature)));
    delivery.set("deliveredDate", now());
    delivery.create(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeRecoveryShared",
            vault_id: Some(&session.vault_id),
            subject_user_id: Some(&session.subject_user_id),
            detail: Some(format!("session {session_id}")),
            ..Event::default()
        },
    )?;
    Ok(out([("recoverySessionId", Value::string(session_id))]))
}

/// `get#VaultRecoverySession`: to the recovering holder, all their client needs to finish: the session, the recovery
/// key's record and its maker's public halves, their own sealed share, the delivered shares (each with its holder's
/// row and public halves), the vault's member-list chain, the recovery member's wrap of the current key version, and
/// the vault's revision.
fn get_vault_recovery_session(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_permission()?;
    let session_id = required(params, "recoverySessionId")?;
    let row = session_row(facade, &session_id, false)?;
    if field(&row, "holderUserId") != me {
        return Err(fail(format!("No recovery session {session_id}")));
    }
    let recovery_key_id = field(&row, "recoveryKeyId");
    let key = key_row(facade, &recovery_key_id)?.ok_or_else(|| fail("The session's recovery key is gone"))?;
    let record = record_of_row(&key)?;
    let maker = maker_public(facade, &record)?;
    let own = holder_row(facade, &recovery_key_id, &me)?.ok_or_else(|| fail("You hold no share of this key"))?;
    let mut deliveries = Vec::new();
    for delivered in delivered_by(facade, &session_id)?.iter() {
        let user_id = field(delivered, "userId");
        let Some(held) = holder_row(facade, &recovery_key_id, &user_id)? else {
            continue;
        };
        let Some(sender) = find(facade, PERSONAL_KEY)?
            .eq("personalKeyId", Value::string(field(delivered, "personalKeyId")))
            .one()?
        else {
            continue;
        };
        deliveries.push(json!({
            "share": {
                "session_id": session_id, "recovery_key_id": recovery_key_id, "holder_user_id": user_id,
                "holder_key_id": field(delivered, "personalKeyId"), "sealed": field(delivered, "sealedShare"),
                "signature": field(delivered, "signature"),
            },
            "holder": holder_share_json(&held),
            "public": hex(&public_of(&sender)?.to_bytes()),
        }));
    }
    let vault_id = field(&row, "vaultId");
    let vault = vault_row(facade, &vault_id, false)?;
    let entries: Vec<Json> = chain_entries(facade, &vault_id)?
        .iter()
        .map(shapes::key_version_json)
        .collect();
    let recovery_wrap = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("userId", Value::string(recovery_member_id(&recovery_key_id)))
        .eq("keyVersion", Value::I64(int_field(&vault, "keyVersion")))
        .one()?
        .map(|wrap| wrap_json(&wrap))
        .unwrap_or(Json::Null);
    Ok(out([
        ("session", value(&session_json(&row))),
        ("statusId", Value::string(field(&row, "statusId"))),
        ("record", value(&shapes::record_json(&record))),
        ("makerPublic", Value::string(hex(&maker.to_bytes()))),
        ("held", value(&holder_share_json(&own))),
        ("deliveries", value(&Json::Array(deliveries))),
        ("entries", value(&Json::Array(entries))),
        ("recoveryWrap", value(&recovery_wrap)),
        ("revision", Value::I64(int_field(&vault, "revision"))),
    ]))
}

/// `complete#VaultRecovery`: the recovering holder's end of a session, in one transaction that names the vault's
/// revision: the next member list (`entry`), signed by the recovery key as the vault's recovery member, giving the
/// session's member its new key and changing nothing else; the vault key wrapped to that key (`wrap`); and, when the
/// member was a manager, the other members' wraps it signed, remade (`rewraps`). Needs at least k − 1 other holders'
/// shares in the session. An event, and a notice to every member.
fn complete_vault_recovery(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_holder_rights()?;
    let mine = my_key(facade, &me)?;
    let session_id = required(params, "recoverySessionId")?;
    let mut row = session_row(facade, &session_id, true)?;
    require_open(&row)?;
    if field(&row, "holderUserId") != me || field(&row, "holderKeyId") != field(&mine, "personalKeyId") {
        return Err(fail(
            "Only the holder who started a recovery completes it, with the same key",
        ));
    }
    let session = session_of_row(&row)?;
    let key = key_row(facade, &session.recovery_key_id)?.ok_or_else(|| fail("The session's recovery key is gone"))?;
    let shares: BTreeSet<String> = delivered_by(facade, &session_id)?
        .iter()
        .map(|d| field(d, "userId"))
        .filter(|user_id| *user_id != me)
        .collect();
    let needed = int_field(&key, "sharesNeeded") - 1;
    // a delivering holder who no longer holds VAULT_RECOVERY, or the share, does not count
    let mut counted = BTreeSet::new();
    for user_id in &shares {
        if super::user_may_hold(user_id)? && holder_row(facade, &session.recovery_key_id, user_id)?.is_some() {
            counted.insert(user_id.clone());
        }
    }
    let shares = counted;
    if (shares.len() as i64) < needed {
        return Err(fail(format!(
            "{} of the {needed} other holders' shares needed are in this recovery",
            shares.len()
        )));
    }
    let vault_id = session.vault_id.clone();
    let mut vault = vault_row(facade, &vault_id, true)?;
    check_revision(params, &vault)?;
    let (mut entries, latest) = latest_with_recovery(facade, &vault_id, &session.recovery_key_id)?;
    let entry = entry_param(params)?;
    let recovery_user = recovery_member_id(&session.recovery_key_id);
    if entry.signer_user_id != recovery_user || entry.signer_key_id != session.recovery_key_id {
        return Err(fail("A recovery's member list is signed by the recovery key"));
    }
    if entry.version != latest.version {
        return Err(fail("A recovery keeps the key version"));
    }
    let subject = &session.subject_user_id;
    // the new key is still the member's active one
    let still_active =
        active_key(facade, subject)?.is_some_and(|row| field(&row, "personalKeyId") == session.subject_key_id);
    if !still_active {
        return Err(fail(format!(
            "The session's new key is no longer {subject}'s active key"
        )));
    }
    // each person once, on both sides, so the member's old key is gone (the chain checks it too)
    let people: BTreeSet<&str> = entry.members.iter().map(|m| m.user_id.as_str()).collect();
    let before: BTreeSet<&str> = latest.members.iter().map(|m| m.user_id.as_str()).collect();
    if people.len() != entry.members.len() || people != before {
        return Err(fail("A recovery keeps the same members, each once"));
    }
    let given = entry.member(subject, &session.subject_key_id);
    if given.is_none_or(|m| m.public != session.subject_public) {
        return Err(fail(format!(
            "The member list does not give {subject} the session's new key"
        )));
    }
    entries.push(entry.clone());
    // the chain refuses a recovery's entry that changes more than one member's key
    let chain = verify_chain(&entries)?;
    let wrap = wrap_param(params, "wrap")?;
    if wrap.member_user_id != *subject
        || wrap.member_key_id != session.subject_key_id
        || wrap.key_version != entry.version
    {
        return Err(fail(
            "The wrap is not of the current key version to the member's new key",
        ));
    }
    chain.accept_wrap(&wrap).map_err(refused)?;
    let checks = checks_by(facade, &field(&mine, "personalKeyId"))?;
    fingerprint::require_checked(&checks, &public_of(&mine)?.verify, subject, &session.subject_public).map_err(
        |_| {
            fail(format!(
                "Check {subject}'s new key fingerprint before recovering their vault"
            ))
        },
    )?;

    // wraps of this version whose signer no longer manages with that key are remade by the recovery key
    let current = version(entry.version)?;
    let still_manages =
        |user_id: &str, key_id: &str| entry.member(user_id, key_id).is_some_and(|m| m.role == Role::Manage);
    let rows = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("keyVersion", Value::I64(current))
        .list()?;
    let stranded: Vec<EntityValue> = rows
        .iter()
        .filter(|row| field(row, "userId") != *subject)
        .filter(|row| !still_manages(&field(row, "wrapSignerUserId"), &field(row, "wrapSignerKeyId")))
        .cloned()
        .collect();
    let rewraps = match params.get("rewraps") {
        Some(given) if !moqui_lib::object::is_empty(given) => wraps_param(params, "rewraps")?,
        _ => Vec::new(),
    };
    let mut remade = Vec::with_capacity(stranded.len());
    for row in &stranded {
        let user_id = field(row, "userId");
        let Some(rewrap) = rewraps.iter().find(|w| w.member_user_id == user_id) else {
            return Err(fail(format!(
                "Remake {user_id}'s wrap: the manager who signed it no longer manages this vault with that key"
            )));
        };
        if rewrap.member_key_id != field(row, "personalKeyId") || rewrap.key_version != entry.version {
            return Err(fail(format!(
                "The new wrap for {user_id} is not of their key and this key version"
            )));
        }
        chain.accept_wrap(rewrap).map_err(refused)?;
        remade.push((row, rewrap));
    }

    store_entry(facade, &entry)?;
    // the member's wraps to the lost key, of every version, give way to the new one
    let lost = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("userId", Value::string(subject))
        .list()?;
    for old in lost.iter() {
        old.delete(facade)?;
    }
    let role = entry
        .member(subject, &session.subject_key_id)
        .map_or(Role::Read, |m| m.role);
    store_wrap(facade, &wrap, role)?;
    for (old, rewrap) in remade {
        old.delete(facade)?;
        let role = entry
            .member(&rewrap.member_user_id, &rewrap.member_key_id)
            .map_or(Role::Read, |m| m.role);
        store_wrap(facade, rewrap, role)?;
    }
    let revision = bump_revision(facade, &mut vault)?;
    row.set("statusId", Value::string(COMPLETED));
    row.set("endedDate", now());
    row.update(facade)?;
    // the recovering client held the whole recovery key: it is used, and every vault that lists it should replace it
    super::mark_used(facade, &session.recovery_key_id)?;
    let mut holders: Vec<String> = shares.into_iter().collect();
    holders.insert(0, me.clone());
    events::write(
        facade,
        Event {
            kind: "VeRecovery",
            vault_id: Some(&vault_id),
            subject_user_id: Some(subject),
            personal_key_id: Some(&session.subject_key_id),
            key_version: Some(current),
            detail: Some(format!(
                "session {session_id}, recovery key {}, holders {}",
                session.recovery_key_id,
                holders.join(", ")
            )),
            ..Event::default()
        },
    )?;
    notify_members(
        &entry,
        &vault_id,
        subject,
        &format!("A vault you are a member of was recovered for {subject}, to their new personal key"),
    )?;
    Ok(out([("revision", Value::I64(revision))]))
}

/// `cancel#VaultRecovery`: the recovering holder, or a manager of the vault (told of the start), ends a session that
/// has not completed. An event.
fn cancel_vault_recovery(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let session_id = required(params, "recoverySessionId")?;
    let mut row = session_row(facade, &session_id, true)?;
    let vault_id = field(&row, "vaultId");
    // any person of the vault's member list (the member recovered among them), told of the start, may stop it
    let member = match vault_row(facade, &vault_id, false) {
        Ok(vault) => membership(facade, &vault, &me)?.is_some(),
        Err(_) => false,
    };
    if field(&row, "holderUserId") != me && field(&row, "subjectUserId") != me && !member {
        return Err(fail(format!("No recovery session {session_id}")));
    }
    if field(&row, "statusId") != OPEN {
        return Err(fail("That recovery has ended"));
    }
    row.set("statusId", Value::string(CANCELLED));
    row.set("endedDate", now());
    row.update(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeRecoveryCancelled",
            vault_id: Some(&vault_id),
            subject_user_id: Some(&field(&row, "subjectUserId")),
            detail: Some(format!("session {session_id}")),
            ..Event::default()
        },
    )?;
    Ok(out([("recoverySessionId", Value::string(session_id))]))
}
