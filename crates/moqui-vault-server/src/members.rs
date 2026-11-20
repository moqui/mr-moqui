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

//! Members, the member-list chain and rotation (plans/Vault-Instances.md §3.9, decisions (a) and (b) of §9).
//!
//! - The chain is the `VaultKeyVersion` rows. The server takes a new entry only when it follows the latest, is signed
//!   by a manager of the latest with the key listed for them, and that manager is the caller with their active key
//!   (the vault crate's `Chain`, anchored at the maker's key).
//! - Add: the next revision of the same key version, with the new member's wrap, signed by a manager. The new member's
//!   key is their active personal key, and the signer has a stored check of it (`VaultKeyCheck`): nothing is wrapped
//!   to a changed key before it is checked again.
//! - Remove: revision 1 of the next key version, every item key rewrapped under it and every remaining member's wrap
//!   remade, all in one transaction that names the vault's revision it was made from (a compare-and-swap); the removed
//!   member's wraps are deleted and a write at the old version is refused from then on.
//! - The remaining members' wraps of the earlier version are kept until each opens the new one (`ack#VaultKeyVersion`).

use std::collections::{BTreeMap, BTreeSet};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::fingerprint;
use moqui_vault::keyversion::{Chain, KeyVersion, Member, Role};
use moqui_vault::ops::shapes;
use moqui_vault::text::{hex, unhex};
use moqui_vault::vault::MemberWrap;
use serde_json::{json, Value as Json};

use crate::common::{
    authz_off, fail, field, find, int, int_field, json, now, out, refused, required, role_enum, role_fits, user, value,
    vault_for, Access, Out, Params, ITEM, KEY_VERSION, MEMBER, OPERATIONAL, SINGLE_SECRET,
};
use crate::events::{self, Event};
use crate::personal::{active_key, checks_by, my_key, public_of};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, add_vault_member)?;
    moqui_core::rust_service!(map, remove_vault_member)?;
    moqui_core::rust_service!(map, ack_vault_key_version)?;
    Ok(())
}

/// A stored entry as the vault crate reads it.
fn entry_of_row(row: &EntityValue) -> Result<KeyVersion, LibError> {
    let members: Json = serde_json::from_str(&field(row, "memberList")).map_err(|_| fail("a stored member list"))?;
    shapes::key_version_of(&json!({
        "vault_id": field(row, "vaultId"), "version": int_field(row, "keyVersion"),
        "revision": int_field(row, "revision"), "members": members,
        "signer_user_id": field(row, "signerUserId"), "signer_key_id": field(row, "signerKeyId"),
        "signature": field(row, "signature"),
    }))
    .map_err(refused)
}

/// A vault's chain, oldest first.
pub(crate) fn chain_entries(facade: &EntityFacade, vault_id: &str) -> Result<Vec<KeyVersion>, LibError> {
    let rows = find(facade, KEY_VERSION)?
        .eq("vaultId", Value::string(vault_id))
        .order_by("keyVersion", false)
        .order_by("revision", false)
        .list()?;
    rows.iter().map(entry_of_row).collect()
}

pub(crate) fn entry_param(params: &Params) -> Result<KeyVersion, LibError> {
    shapes::key_version_of(&json(params, "entry")?).map_err(refused)
}

/// The chain's entries checked as a whole, from the maker's key: each signed by a manager of the one before.
pub(crate) fn verify_chain(entries: &[KeyVersion]) -> Result<Chain<'_>, LibError> {
    let [first, ..] = entries else {
        return Err(fail("A vault has no key version"));
    };
    let maker = first
        .member(&first.signer_user_id, &first.signer_key_id)
        .ok_or_else(|| fail("The first key version is not signed by a member of it"))?;
    Chain::verify(entries, &[maker.public.verify]).map_err(refused)
}

/// Refuses an entry not signed by the caller with their active key.
pub(crate) fn require_signed_by_me(facade: &EntityFacade, entry: &KeyVersion, me: &str) -> Result<(), LibError> {
    let mine = my_key(facade, me)?;
    if entry.signer_user_id != me || entry.signer_key_id != field(&mine, "personalKeyId") {
        return Err(fail("Sign the member list yourself, with your active personal key"));
    }
    Ok(())
}

/// Checks a member the entry adds: a role of the vault's kind, their active personal key as stored, and, unless the
/// member is the signer, a check of that key stored by the signer's key.
pub(crate) fn check_new_member(
    facade: &EntityFacade,
    member: &Member,
    signer: &Member,
    operational: bool,
) -> Result<(), LibError> {
    if !role_fits(member.role, operational) {
        return Err(fail(format!(
            "The role {} is not a role of this kind of vault",
            member.role.as_str()
        )));
    }
    let key = active_key(facade, &member.user_id)?
        .ok_or_else(|| fail(format!("{} has no personal vault key", member.user_id)))?;
    if field(&key, "personalKeyId") != member.key_id || public_of(&key)? != member.public {
        return Err(fail(format!(
            "The key listed for {} is not their active personal key",
            member.user_id
        )));
    }
    if member.user_id == signer.user_id && member.key_id == signer.key_id {
        return Ok(());
    }
    let checks = checks_by(facade, &signer.key_id)?;
    fingerprint::require_checked(&checks, &signer.public.verify, &member.public).map_err(|_| {
        fail(format!(
            "Check {}'s key fingerprint before wrapping the vault key to it",
            member.user_id
        ))
    })
}

pub(crate) fn wraps_param(params: &Params, name: &str) -> Result<Vec<MemberWrap>, LibError> {
    let given = json(params, name)?;
    let Some(rows) = given.as_array() else {
        return Err(fail(format!("{name} is a list")));
    };
    rows.iter()
        .map(|row| shapes::member_wrap_of(row).map_err(refused))
        .collect()
}

pub(crate) fn wrap_param(params: &Params, name: &str) -> Result<MemberWrap, LibError> {
    shapes::member_wrap_of(&json(params, name)?).map_err(refused)
}

/// Stores an entry of the chain.
pub(crate) fn store_entry(facade: &EntityFacade, entry: &KeyVersion) -> Result<(), LibError> {
    let shaped = shapes::key_version_json(entry);
    let mut row = facade.make_value(KEY_VERSION)?;
    row.set("vaultId", Value::string(&entry.vault_id));
    row.set("keyVersion", Value::I64(version(entry.version)?));
    row.set("revision", Value::I64(version(entry.revision)?));
    row.set("memberList", Value::string(shaped["members"].to_string()));
    row.set("signerUserId", Value::string(&entry.signer_user_id));
    row.set("signerKeyId", Value::string(&entry.signer_key_id));
    row.set("signature", Value::string(hex(&entry.signature)));
    row.set("createdDate", now());
    row.create(facade)?;
    Ok(())
}

pub(crate) fn version(number: u64) -> Result<i64, LibError> {
    i64::try_from(number).map_err(|_| fail("a key version out of range"))
}

/// Stores a member's wrap, as the chain accepted it, with the role the entry gives them.
pub(crate) fn store_wrap(facade: &EntityFacade, wrap: &MemberWrap, role: Role) -> Result<(), LibError> {
    let mut row = facade.make_value(MEMBER)?;
    row.set("vaultId", Value::string(&wrap.vault_id));
    row.set("userId", Value::string(&wrap.member_user_id));
    row.set("keyVersion", Value::I64(version(wrap.key_version)?));
    row.set("personalKeyId", Value::string(&wrap.member_key_id));
    row.set("roleEnumId", Value::string(role_enum(role)));
    row.set("wrappedVaultKey", Value::string(hex(&wrap.sealed)));
    row.set("wrapSignerUserId", Value::string(&wrap.signer_user_id));
    row.set("wrapSignerKeyId", Value::string(&wrap.signer_key_id));
    row.set("wrapSignature", Value::string(hex(&wrap.signature)));
    row.set("fromDate", now());
    row.create(facade)?;
    Ok(())
}

/// A member row as the vault crate's wrap.
pub(crate) fn wrap_json(row: &EntityValue) -> Json {
    json!({
        "vault_id": field(row, "vaultId"), "key_version": int_field(row, "keyVersion"),
        "member_user_id": field(row, "userId"), "member_key_id": field(row, "personalKeyId"),
        "sealed": field(row, "wrappedVaultKey"), "signer_user_id": field(row, "wrapSignerUserId"),
        "signer_key_id": field(row, "wrapSignerKeyId"), "signature": field(row, "wrapSignature"),
    })
}

/// The compare-and-swap of decision (a): the change names the vault revision it was made from.
pub(crate) fn check_revision(params: &Params, vault: &EntityValue) -> Result<(), LibError> {
    let current = int_field(vault, "revision");
    match int(params, "expectedRevision") {
        Some(expected) if expected == current => Ok(()),
        _ => Err(fail(format!(
            "The vault changed since you read it (revision {current}): read it again"
        ))),
    }
}

fn same(a: &Member, b: &Member) -> bool {
    a.user_id == b.user_id && a.key_id == b.key_id && a.public == b.public && a.role == b.role
}

/// `add#VaultMember`: the next revision of the member list, adding a member or changing one member's role or key, with
/// the wrap of the current key version to that member.
fn add_vault_member(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Manage, true)?;
    if field(&vault, "lockEnumId") == SINGLE_SECRET {
        return Err(fail("A single-secret vault has no members"));
    }
    check_revision(params, &vault)?;
    let entry = entry_param(params)?;
    require_signed_by_me(facade, &entry, &me)?;
    let mut entries = chain_entries(facade, &vault_id)?;
    let latest = entries
        .last()
        .cloned()
        .ok_or_else(|| fail("A vault has no key version"))?;
    if entry.version != latest.version {
        return Err(fail(
            "Adding a member keeps the key version; a removal rotates it (remove#VaultMember)",
        ));
    }
    entries.push(entry.clone());
    let chain = verify_chain(&entries)?;
    let added: Vec<&Member> = entry
        .members
        .iter()
        .filter(|m| !latest.members.iter().any(|l| same(l, m)))
        .collect();
    let dropped: Vec<&Member> = latest
        .members
        .iter()
        .filter(|l| !entry.members.iter().any(|m| same(l, m)))
        .collect();
    let [member] = added.as_slice() else {
        return Err(fail("Add one member at a time"));
    };
    let users: BTreeSet<&str> = entry.members.iter().map(|m| m.user_id.as_str()).collect();
    if users.len() != entry.members.len() {
        return Err(fail("A member list names each user once"));
    }
    if dropped.iter().any(|d| d.user_id != member.user_id) {
        return Err(fail(
            "A member is removed with remove#VaultMember, which rotates the key",
        ));
    }
    let signer = latest
        .member(&entry.signer_user_id, &entry.signer_key_id)
        .ok_or_else(|| fail("The signer is not a member"))?;
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    check_new_member(facade, member, signer, operational)?;
    let wrap = wrap_param(params, "wrap")?;
    if wrap.member_user_id != member.user_id || wrap.member_key_id != member.key_id || wrap.key_version != entry.version
    {
        return Err(fail("The wrap is not of the current key version to the member added"));
    }
    chain.accept_wrap(&wrap).map_err(refused)?;

    store_entry(facade, &entry)?;
    let current = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("userId", Value::string(&member.user_id))
        .eq("keyVersion", Value::I64(version(entry.version)?))
        .one()?;
    if let Some(row) = current {
        row.delete(facade)?;
    }
    store_wrap(facade, &wrap, member.role)?;
    vault.set("revision", Value::I64(int_field(&vault, "revision") + 1));
    vault.update(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeMemberAdded",
            vault_id: Some(&vault_id),
            subject_user_id: Some(&member.user_id),
            personal_key_id: Some(&member.key_id),
            key_version: Some(version(entry.version)?),
            detail: Some(format!("role {}", member.role.as_str())),
            ..Event::default()
        },
    )?;
    Ok(out([("revision", Value::I64(int_field(&vault, "revision")))]))
}

/// `remove#VaultMember`: the member's wraps deleted and the vault key rotated, in one transaction that names the
/// vault's revision: revision 1 of the next key version without the member, every item key rewrapped under it and
/// every remaining member's wrap remade, all signed by the caller, a manager. A manager does not remove themselves, an
/// operational vault keeps recovery on or two members, and every vault keeps a manager.
fn remove_vault_member(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let removed = required(params, "userId")?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Manage, true)?;
    if field(&vault, "lockEnumId") == SINGLE_SECRET {
        return Err(fail("A single-secret vault has no members"));
    }
    if removed == me {
        return Err(fail(
            "Another manager removes you: whoever rotates the key knows the new one",
        ));
    }
    check_revision(params, &vault)?;
    let entry = entry_param(params)?;
    require_signed_by_me(facade, &entry, &me)?;
    let mut entries = chain_entries(facade, &vault_id)?;
    let latest = entries
        .last()
        .cloned()
        .ok_or_else(|| fail("A vault has no key version"))?;
    if !latest.members.iter().any(|m| m.user_id == removed) {
        return Err(fail(format!("{removed} is not a member")));
    }
    if entry.version != latest.version + 1 || entry.revision != 1 {
        return Err(fail("A removal rotates the vault key to the next version"));
    }
    let staying: Vec<&Member> = latest.members.iter().filter(|m| m.user_id != removed).collect();
    let unchanged =
        entry.members.len() == staying.len() && staying.iter().all(|s| entry.members.iter().any(|m| same(s, m)));
    if !unchanged {
        return Err(fail("A removal changes no other member"));
    }
    if !entry.members.iter().any(|m| m.role == Role::Manage) {
        return Err(fail("A vault keeps at least one manager"));
    }
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    if operational && field(&vault, "recoveryOn") != "Y" && entry.members.len() < 2 {
        return Err(fail("An operational vault keeps recovery on or at least two members"));
    }
    entries.push(entry.clone());
    let chain = verify_chain(&entries)?;

    // every remaining member's wrap of the new version, accepted by the chain
    let wraps = wraps_param(params, "memberWraps")?;
    let mut wrapped = BTreeSet::new();
    for wrap in &wraps {
        if wrap.key_version != entry.version {
            return Err(fail("A member's wrap is not of the new key version"));
        }
        chain.accept_wrap(wrap).map_err(refused)?;
        wrapped.insert((wrap.member_user_id.clone(), wrap.member_key_id.clone()));
    }
    let needed: BTreeSet<(String, String)> = entry
        .members
        .iter()
        .map(|m| (m.user_id.clone(), m.key_id.clone()))
        .collect();
    if wrapped != needed || wraps.len() != needed.len() {
        return Err(fail("Remake the wrap of every remaining member, once each"));
    }
    // every item key rewrapped under the new version
    let given = json(params, "itemKeys")?;
    let Some(rows) = given.as_array() else {
        return Err(fail("itemKeys is a list"));
    };
    let mut rewrapped = BTreeMap::new();
    for row in rows {
        let item_id = row["item_id"].as_str().unwrap_or_default().to_string();
        let key = row["wrapped_item_key"].as_str().unwrap_or_default().to_string();
        if item_id.is_empty() || unhex(&key).is_err() || rewrapped.insert(item_id, key).is_some() {
            return Err(fail("Each item key once, with its item_id and wrapped_item_key"));
        }
    }
    let items = find(facade, ITEM)?.eq("vaultId", Value::string(&vault_id)).list()?;
    let ids: BTreeSet<String> = items.iter().map(|row| field(row, "vaultItemId")).collect();
    if ids != rewrapped.keys().cloned().collect::<BTreeSet<String>>() {
        return Err(fail("Rewrap the key of every item of the vault"));
    }

    store_entry(facade, &entry)?;
    let new_version = version(entry.version)?;
    // the removed member's wraps, of every version
    let gone = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("userId", Value::string(&removed))
        .list()?;
    for row in gone.iter() {
        row.delete(facade)?;
    }
    for wrap in &wraps {
        let role = entry
            .member(&wrap.member_user_id, &wrap.member_key_id)
            .map_or(Role::Read, |m| m.role);
        store_wrap(facade, wrap, role)?;
    }
    for row in items.iter() {
        let mut row = row.clone();
        let key = rewrapped.get(&field(&row, "vaultItemId")).cloned().unwrap_or_default();
        row.set("wrappedItemKey", Value::string(key));
        row.set("itemKeyWrapEnumId", Value::string("VwVault"));
        row.set("keyVersion", Value::I64(new_version));
        row.update(facade)?;
    }
    vault.set("keyVersion", Value::I64(new_version));
    vault.set("revision", Value::I64(int_field(&vault, "revision") + 1));
    vault.update(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeMemberRemoved",
            vault_id: Some(&vault_id),
            subject_user_id: Some(&removed),
            key_version: Some(new_version - 1),
            ..Event::default()
        },
    )?;
    events::write(
        facade,
        Event {
            kind: "VeKeyRotated",
            vault_id: Some(&vault_id),
            key_version: Some(new_version),
            detail: Some(format!("{} item keys rewrapped", items.len())),
            ..Event::default()
        },
    )?;
    Ok(out([
        ("keyVersion", Value::I64(new_version)),
        ("revision", Value::I64(int_field(&vault, "revision"))),
    ]))
}

/// `ack#VaultKeyVersion`: the caller opened `keyVersion` of the vault key; their wraps of earlier versions are deleted
/// (decision (b) of §9).
fn ack_vault_key_version(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let vault = vault_for(facade, &vault_id, &me, Access::Read, false)?;
    let acked = int(params, "keyVersion").ok_or_else(|| fail("keyVersion is required"))?;
    if acked > int_field(&vault, "keyVersion") {
        return Err(fail("No such key version yet"));
    }
    let rows = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("userId", Value::string(&me))
        .list()?;
    let Some(mut held) = rows.iter().find(|row| int_field(row, "keyVersion") == acked).cloned() else {
        return Err(fail("You hold no wrap of that key version"));
    };
    held.set("ackedDate", now());
    held.update(facade)?;
    let mut dropped = 0;
    for row in rows.iter().filter(|row| int_field(row, "keyVersion") < acked) {
        row.delete(facade)?;
        dropped += 1;
    }
    events::write(
        facade,
        Event {
            kind: "VeKeyVersionAcked",
            vault_id: Some(&vault_id),
            key_version: Some(acked),
            detail: Some(format!("{dropped} earlier wraps deleted")),
            ..Event::default()
        },
    )?;
    Ok(out([("dropped", Value::I64(dropped))]))
}

/// The caller's own wraps of a vault, every version they still hold, as the vault crate reads them.
pub(crate) fn my_wraps(facade: &EntityFacade, vault_id: &str, me: &str) -> Result<Value, LibError> {
    let rows = find(facade, MEMBER)?
        .eq("vaultId", Value::string(vault_id))
        .eq("userId", Value::string(me))
        .order_by("keyVersion", false)
        .list()?;
    Ok(value(&Json::Array(rows.iter().map(wrap_json).collect())))
}
