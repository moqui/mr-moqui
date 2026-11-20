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

//! Vaults of both locks (plans/Vault-Instances.md §3.1, §3.10, §3.11).
//!
//! - A single-secret vault is personal, has one owner, its wrapped vault key on its row, and no members and no
//!   recovery: making one with either is refused.
//! - A shared-key vault starts with its first key version (1.1), signed by its maker, a manager in it, and a wrap of
//!   the vault key to each member it lists, each member's key checked by the maker. An operational vault needs
//!   `VAULT_ADMIN`, has its name in the clear, and keeps recovery on (its default) or at least two members; a personal
//!   shared-key vault has recovery off by default (§9, Nov 19). Recovery is on when the member list lists the
//!   organization's recovery key (crate::recovery).
//! - A personal vault's name is in its sealed header; no column of it holds a name in the clear.

use std::collections::{BTreeMap, BTreeSet};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::object::Value;
use serde_json::Value as Json;

use crate::common::{
    authz_off, check_id, fail, field, find, flag, int_field, now, out, required, text, user, value, vault_for, Access,
    Out, Params, ITEM, ITEM_HISTORY, KEY_VERSION, MEMBER, OPERATIONAL, PERSONAL, SHARED_KEY, SINGLE_SECRET, VAULT,
};
use crate::events::{self, Event};
use crate::members::{
    chain_entries, check_new_member, entry_param, my_wraps, require_signed_by_me, store_entry, store_wrap,
    verify_chain, wraps_param,
};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, create_vault)?;
    moqui_core::rust_service!(map, list_vaults)?;
    moqui_core::rust_service!(map, get_vault)?;
    moqui_core::rust_service!(map, delete_vault)?;
    moqui_core::rust_service!(map, update_vault_secret_wrap)?;
    Ok(())
}

/// `create#Vault`: a vault of either lock, as the module doc says.
fn create_vault(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    check_id("vaultId", &vault_id)?;
    let kind = required(params, "vaultKindEnumId")?;
    let lock = required(params, "lockEnumId")?;
    if ![PERSONAL, OPERATIONAL].contains(&kind.as_str()) || ![SINGLE_SECRET, SHARED_KEY].contains(&lock.as_str()) {
        return Err(fail("A vault is personal or operational, single-secret or shared-key"));
    }
    let taken = find(facade, VAULT)?
        .eq("vaultId", Value::string(&vault_id))
        .one()?
        .is_some();
    // a deleted vault's events stay, readable by whoever holds its id: the id is never given again
    if taken || crate::common::id_has_history(facade, "vaultId", &vault_id)? {
        return Err(crate::common::id_not_available("Vault", &vault_id));
    }
    let operational = kind == OPERATIONAL;
    if operational && !moqui_core::user::has_permission("VAULT_ADMIN").map_err(|err| fail(err.message()))? {
        return Err(fail("Making an operational vault needs the VAULT_ADMIN permission"));
    }
    // recovery is on when the first member list lists the organization's recovery key (§3.10): recoveryOn Y without it
    // is refused, and an operational vault without it keeps two members. A single-secret vault never has it.
    let recovery = flag(params, "recoveryOn").unwrap_or(operational);
    let mut vault = facade.make_value(VAULT)?;
    vault.set("vaultId", Value::string(&vault_id));
    vault.set("vaultKindEnumId", Value::string(&kind));
    vault.set("lockEnumId", Value::string(&lock));
    vault.set("ownerUserId", Value::string(&me));
    vault.set("keyVersion", Value::I64(1));
    vault.set("revision", Value::I64(1));
    vault.set("recoveryOn", Value::string(if recovery { "Y" } else { "N" }));
    vault.set("createdDate", now());
    if operational {
        vault.set("vaultName", Value::string(required(params, "vaultName")?));
    } else {
        if !text(params, "vaultName").is_empty() {
            return Err(fail(
                "A personal vault's name goes in its sealed header, never in the clear",
            ));
        }
        let header = text(params, "headerCipher");
        if !header.is_empty() {
            crate::common::check_hex("headerCipher", &header, crate::common::MAX_SMALL_TEXT)?;
            vault.set("headerCipher", Value::string(header));
        }
    }

    let member_count = if lock == SINGLE_SECRET {
        create_single_secret(params, &mut vault, operational, recovery)?;
        vault.create(facade)?;
        1
    } else {
        let entry = entry_param(params)?;
        if entry.vault_id != vault_id || entry.version != 1 || entry.revision != 1 {
            return Err(fail("A vault starts at its first key version, 1.1, of this vault"));
        }
        require_signed_by_me(facade, &entry, &me)?;
        let entries = vec![entry.clone()];
        let chain = verify_chain(&entries)?;
        let maker = entry
            .member(&entry.signer_user_id, &entry.signer_key_id)
            .ok_or_else(|| fail("The maker is a manager in the first member list"))?;
        let mut listed = BTreeSet::new();
        for member in &entry.members {
            check_new_member(facade, member, maker, operational)?;
            if !listed.insert(member.user_id.clone()) {
                return Err(fail("A member is listed once"));
            }
        }
        crate::recovery::check_one_recovery(&entry)?;
        let has_recovery = crate::recovery::listed_recovery(&entry).is_some();
        if flag(params, "recoveryOn") == Some(true) && !has_recovery {
            return Err(fail(
                "Recovery on lists the organization's recovery key in the first member list, with its wrap",
            ));
        }
        if operational && !has_recovery && crate::recovery::people(&entry).count() < 2 {
            return Err(fail("An operational vault keeps recovery on or at least two members"));
        }
        vault.set("recoveryOn", Value::string(if has_recovery { "Y" } else { "N" }));
        let wraps = wraps_param(params, "memberWraps")?;
        let mut wrapped = BTreeSet::new();
        for wrap in &wraps {
            if wrap.key_version != 1 {
                return Err(fail("A member's wrap is of key version 1"));
            }
            chain.accept_wrap(wrap).map_err(crate::common::refused)?;
            wrapped.insert(wrap.member_user_id.clone());
        }
        if wrapped != listed || wraps.len() != listed.len() {
            return Err(fail("Wrap the vault key to every member, once each"));
        }
        vault.create(facade)?;
        store_entry(facade, &entry)?;
        for wrap in &wraps {
            let role = entry
                .member(&wrap.member_user_id, &wrap.member_key_id)
                .map(|m| m.role)
                .ok_or_else(|| fail("A wrap to someone not listed"))?;
            store_wrap(facade, wrap, role)?;
        }
        entry.members.len()
    };
    events::write(
        facade,
        Event {
            kind: "VeVaultCreated",
            vault_id: Some(&vault_id),
            key_version: Some(1),
            detail: Some(format!("{kind} {lock}, {member_count} members")),
            ..Event::default()
        },
    )?;
    Ok(out([("vaultId", Value::string(vault_id)), ("revision", Value::I64(1))]))
}

/// A single-secret vault's own columns: personal only, its wrapped vault key, and no members or recovery (§3.10,
/// §3.11).
fn create_single_secret(
    params: &Params,
    vault: &mut EntityValue,
    operational: bool,
    recovery: bool,
) -> Result<(), moqui_lib::error::Error> {
    if operational {
        return Err(fail("An operational vault is shared-key"));
    }
    if recovery {
        return Err(fail(
            "A single-secret vault has no recovery: only its password or key opens it",
        ));
    }
    let has = |name: &str| {
        params
            .get(name)
            .is_some_and(|given| !moqui_lib::object::is_empty(given))
    };
    if has("entry") || has("memberWraps") {
        return Err(fail("A single-secret vault has no members"));
    }
    let secret_kind = required(params, "secretKindEnumId")?;
    if !["VsPassword", "VsGenerated"].contains(&secret_kind.as_str()) {
        return Err(fail("A single-secret vault opens by a password or a generated key"));
    }
    let wrap = secret_wrap_param(params, &secret_kind)?;
    vault.set("secretKindEnumId", Value::string(secret_kind));
    vault.set("secretWrap", Value::string(wrap.to_string()));
    Ok(())
}

/// A single-secret vault's wrap, as the vault crate writes it for its kind: a password's carries its Argon2id
/// parameters, refused under the floor or over the ceiling; a generated key's is its HKDF seal (`sealed`, hex).
fn secret_wrap_param(params: &Params, secret_kind: &str) -> Result<Json, moqui_lib::error::Error> {
    let wrap = crate::common::json(params, "secretWrap")?;
    if !wrap.is_object() || wrap.to_string().len() > crate::common::MAX_SMALL_TEXT {
        return Err(fail("secretWrap is the vault crate's wrap"));
    }
    if secret_kind == "VsPassword" {
        let parsed = moqui_vault::ops::shapes::password_wrap_of(&serde_json::json!({ "wrap": wrap }), "wrap")
            .map_err(crate::common::refused)?;
        parsed.params.check().map_err(crate::common::refused)?;
    } else {
        let sealed = wrap["sealed"].as_str().unwrap_or_default();
        crate::common::check_hex("secretWrap.sealed", sealed, crate::common::MAX_SMALL_TEXT)?;
    }
    Ok(wrap)
}

/// `update#VaultSecretWrap`: a single-secret vault's owner stores its vault key wrapped again (a password's wrap
/// raised to the Argon2id target on an open, a new password or generated key), after a strong, recent sign-in. The
/// kind may change between a password and a generated key; the wrap is checked as `create#Vault` checks it.
fn update_vault_secret_wrap(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Manage, true)?;
    if field(&vault, "lockEnumId") != SINGLE_SECRET {
        return Err(fail("Only a single-secret vault has a secret wrap"));
    }
    crate::members::check_revision(params, &vault)?;
    crate::personal::require_strong_sign_in(facade, &me)?;
    let secret_kind = match text(params, "secretKindEnumId") {
        kind if kind.is_empty() => field(&vault, "secretKindEnumId"),
        kind => kind,
    };
    if !["VsPassword", "VsGenerated"].contains(&secret_kind.as_str()) {
        return Err(fail("A single-secret vault opens by a password or a generated key"));
    }
    let wrap = secret_wrap_param(params, &secret_kind)?;
    vault.set("secretKindEnumId", Value::string(&secret_kind));
    vault.set("secretWrap", Value::string(wrap.to_string()));
    let revision = crate::common::bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeSecretRewrapped",
            vault_id: Some(&vault_id),
            detail: Some(secret_kind),
            ..Event::default()
        },
    )?;
    Ok(out([("revision", Value::I64(revision))]))
}

fn vault_json(vault: &EntityValue, role: &str, item_count: usize) -> Value {
    let mut map = moqui_lib::object::ValueMap::default();
    for name in [
        "vaultId",
        "vaultKindEnumId",
        "lockEnumId",
        "ownerUserId",
        "keyVersion",
        "revision",
        "vaultName",
        "headerCipher",
        "recoveryOn",
        "secretKindEnumId",
        "createdDate",
    ] {
        if let Some(given) = vault.get(name) {
            map.insert(name.to_string(), given.clone());
        }
    }
    map.insert("roleEnumId".to_string(), Value::string(role));
    map.insert("itemCount".to_string(), Value::I64(item_count as i64));
    Value::Map(map)
}

fn item_count(facade: &EntityFacade, vault_id: &str) -> Result<usize, moqui_lib::error::Error> {
    Ok(find(facade, ITEM)?.eq("vaultId", Value::string(vault_id)).list()?.len())
}

/// `list#Vaults`: the vaults the caller is a member of at their current key version, and their single-secret vaults.
fn list_vaults(_params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let mut listed = Vec::new();
    let memberships = find(facade, MEMBER)?.eq("userId", Value::string(&me)).list()?;
    let mut seen = BTreeSet::new();
    for member in memberships.iter() {
        let vault_id = field(member, "vaultId");
        let Some(vault) = find(facade, VAULT)?.eq("vaultId", Value::string(&vault_id)).one()? else {
            continue;
        };
        if int_field(&vault, "keyVersion") != int_field(member, "keyVersion") || !seen.insert(vault_id.clone()) {
            continue;
        }
        listed.push(vault_json(
            &vault,
            &field(member, "roleEnumId"),
            item_count(facade, &vault_id)?,
        ));
    }
    let owned = find(facade, VAULT)?
        .eq("ownerUserId", Value::string(&me))
        .eq("lockEnumId", Value::string(SINGLE_SECRET))
        .list()?;
    for vault in owned.iter() {
        let vault_id = field(vault, "vaultId");
        listed.push(vault_json(vault, "VrManage", item_count(facade, &vault_id)?));
    }
    Ok(out([("vaultList", Value::List(listed))]))
}

/// `get#Vault`: to a member (or a single-secret vault's owner): the vault, its member-list chain and current members,
/// and the caller's own wraps of every key version they hold; a single-secret vault's wrapped key to its owner.
fn get_vault(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let vault = vault_for(facade, &vault_id, &me, Access::Read, false)?;
    let single = field(&vault, "lockEnumId") == SINGLE_SECRET;
    let members = find(facade, MEMBER)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("keyVersion", Value::I64(int_field(&vault, "keyVersion")))
        .list()?;
    let role = if single {
        "VrManage".to_string()
    } else {
        members
            .iter()
            .find(|row| field(row, "userId") == me)
            .map(|row| field(row, "roleEnumId"))
            .unwrap_or_default()
    };
    let member_list: Vec<Value> = members
        .iter()
        .map(|row| {
            let mut map = moqui_lib::object::ValueMap::default();
            for name in ["userId", "personalKeyId", "roleEnumId", "keyVersion", "ackedDate"] {
                if let Some(given) = row.get(name) {
                    map.insert(name.to_string(), given.clone());
                }
            }
            Value::Map(map)
        })
        .collect();
    let entries: Vec<Json> = chain_entries(facade, &vault_id)?
        .iter()
        .map(moqui_vault::ops::shapes::key_version_json)
        .collect();
    let secret_wrap: Json = serde_json::from_str(&field(&vault, "secretWrap")).unwrap_or(Json::Null);
    Ok(out([
        ("vault", vault_json(&vault, &role, item_count(facade, &vault_id)?)),
        ("entries", value(&Json::Array(entries))),
        ("memberList", Value::List(member_list)),
        ("wraps", my_wraps(facade, &vault_id, &me)?),
        ("secretWrap", value(&secret_wrap)),
        // an operational vault's deposit key: its client checks the signature on each open (§3.2)
        ("deposit", value(&crate::deposit::deposit_json(&vault))),
        (
            "depositWrappedPrivate",
            Value::string(field(&vault, "depositWrappedPrivate")),
        ),
    ]))
}

/// `delete#Vault`: a manager (or a single-secret vault's owner) deletes the vault, its items and their history, its
/// members, key versions and recovery sessions. Its events stay.
fn delete_vault(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let vault = vault_for(facade, &vault_id, &me, Access::Manage, true)?;
    if field(&vault, "vaultKindEnumId") == OPERATIONAL
        && !moqui_core::user::has_permission("VAULT_ADMIN").map_err(|err| fail(err.message()))?
    {
        return Err(fail("Deleting an operational vault needs the VAULT_ADMIN permission"));
    }
    crate::common::require_recent_auth()?;
    for entity in [ITEM_HISTORY, ITEM, MEMBER, KEY_VERSION] {
        let rows = find(facade, entity)?.eq("vaultId", Value::string(&vault_id)).list()?;
        for row in rows.iter() {
            row.delete(facade)?;
        }
    }
    crate::recovery::forget_vault(facade, &vault_id)?;
    vault.delete(facade)?;
    events::write(
        facade,
        Event {
            kind: "VeVaultDeleted",
            vault_id: Some(&vault_id),
            ..Event::default()
        },
    )?;
    Ok(out([("vaultId", Value::string(vault_id))]))
}
