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

//! Vault items (plans/Vault-Instances.md §3.2 to §3.4): each with its own item key, wrapped by the vault key, and its
//! body sealed whole, both made in the client.
//!
//! - A write is taken only at the vault's current key version, so after a removal a write at the old version is
//!   refused; an update names the item revision it was made from, and the revision before is kept as history.
//! - A personal vault's item holds no name, type or value in the clear: a write that gives one is refused. An
//!   operational item has its type, binding and name in the clear, the binding unique in its vault.
//! - Bodies go only to members. An operational item's body goes to no client here: a reveal is V2's, by role.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};

use crate::common::{
    authz_off, bump_revision, check_hex, check_id, fail, field, find, id_has_history, id_not_available, int, int_field,
    now, out, refused, require_recent_auth, required, text, user, vault_for, Access, Out, Params, ITEM, ITEM_HISTORY,
    MAX_WRAP_HEX, OPERATIONAL,
};
use crate::events::{self, Event};

/// The largest sealed body, in hex characters: the vault crate's 64 KiB item, padded and sealed, with room to spare.
const MAX_BODY_HEX: usize = 2 * (64 * 1024 + 1024);

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, create_vault_item)?;
    moqui_core::rust_service!(map, update_vault_item)?;
    moqui_core::rust_service!(map, list_vault_items)?;
    moqui_core::rust_service!(map, list_vault_item_history)?;
    moqui_core::rust_service!(map, delete_vault_item)?;
    moqui_core::rust_service!(map, set_vault_item_rotation)?;
    Ok(())
}

/// The sealed key and body of a write, and its key version, which must be the vault's current one.
fn sealed_parts(params: &Params, vault: &EntityValue) -> Result<(String, String, i64), LibError> {
    let key_version = int(params, "keyVersion").ok_or_else(|| fail("keyVersion is required"))?;
    let current = int_field(vault, "keyVersion");
    if key_version != current {
        return Err(fail(format!(
            "The vault key changed (version {current}): open the vault again and write at the current version"
        )));
    }
    let wrapped = required(params, "wrappedItemKey")?;
    let body = required(params, "bodyCipher")?;
    if body.len() > MAX_BODY_HEX {
        return Err(fail("An item is at most 64 KiB"));
    }
    check_hex("wrappedItemKey", &wrapped, MAX_WRAP_HEX)?;
    moqui_vault::text::unhex(&body).map_err(refused)?;
    Ok((wrapped, body, key_version))
}

/// An operational item's clear columns; a personal item has none.
fn clear_columns(
    params: &Params,
    operational: bool,
    facade: &EntityFacade,
) -> Result<Vec<(&'static str, String)>, LibError> {
    let given: Vec<(&'static str, String)> = ["vaultItemTypeId", "bindingKey", "itemName"]
        .into_iter()
        .map(|name| (name, text(params, name)))
        .collect();
    if !operational {
        if given.iter().any(|(_, given)| !given.is_empty()) {
            return Err(fail(
                "A personal vault's item keeps its type and name in its sealed body, never in the clear",
            ));
        }
        return Ok(Vec::new());
    }
    if given.iter().any(|(_, given)| given.is_empty()) {
        return Err(fail("An operational item has its type, binding and name"));
    }
    // a newline or an escape in a name could forge lines of an approver's prompt (the lease dialog, moqui vault approve)
    if given.iter().any(|(_, given)| crate::lease::has_control(given)) {
        return Err(fail("An item's type, binding and name have no control character"));
    }
    let type_id = &given[0].1;
    if find(facade, "moqui.security.vault.VaultItemType")?
        .eq("vaultItemTypeId", Value::string(type_id))
        .one()?
        .is_none()
    {
        return Err(fail(format!("No item type {type_id}")));
    }
    Ok(given)
}

fn item_row(facade: &EntityFacade, item_id: &str) -> Result<EntityValue, LibError> {
    find(facade, ITEM)?
        .eq("vaultItemId", Value::string(item_id))
        .one()?
        .ok_or_else(|| fail(format!("No item {item_id}")))
}

/// `create#VaultItem`: a new item at the vault's current key version, by a member who may write.
fn create_vault_item(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Write, true)?;
    let item_id = required(params, "vaultItemId")?;
    check_id("vaultItemId", &item_id)?;
    let taken = find(facade, ITEM)?
        .eq("vaultItemId", Value::string(&item_id))
        .one()?
        .is_some();
    if taken || id_has_history(facade, "vaultItemId", &item_id)? {
        return Err(id_not_available("Item", &item_id));
    }
    let (wrapped, body, key_version) = sealed_parts(params, &vault)?;
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    let clear = clear_columns(params, operational, facade)?;
    if let Some((_, binding)) = clear.iter().find(|(name, _)| *name == "bindingKey") {
        let taken = find(facade, ITEM)?
            .eq("vaultId", Value::string(&vault_id))
            .eq("bindingKey", Value::string(binding))
            .one()?;
        if taken.is_some() {
            return Err(fail(format!("The binding {binding} is taken in this vault")));
        }
    }
    let mut row = facade.make_value(ITEM)?;
    row.set("vaultItemId", Value::string(&item_id));
    row.set("vaultId", Value::string(&vault_id));
    row.set("itemKeyWrapEnumId", Value::string("VwVault"));
    row.set("wrappedItemKey", Value::string(wrapped));
    row.set("keyVersion", Value::I64(key_version));
    row.set("itemKeyVersion", Value::I64(key_version));
    row.set("bodyCipher", Value::string(body));
    row.set("revision", Value::I64(1));
    for (name, given) in clear {
        row.set(name, Value::string(given));
    }
    row.set("createdDate", now());
    row.set("createdByUserId", Value::string(&me));
    row.set("lastModifiedDate", now());
    row.set("lastModifiedByUserId", Value::string(&me));
    row.set("rotatedDate", now());
    row.create(facade)?;
    bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeItemCreated",
            vault_id: Some(&vault_id),
            item_id: Some(&item_id),
            key_version: Some(key_version),
            ..Event::default()
        },
    )?;
    Ok(out([
        ("vaultItemId", Value::string(item_id)),
        ("revision", Value::I64(1)),
    ]))
}

/// `update#VaultItem`: a new revision of an item, at the vault's current key version, naming the revision it was made
/// from; the one before is kept as history.
fn update_vault_item(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let item_id = required(params, "vaultItemId")?;
    let mut row = item_row(facade, &item_id)?;
    let vault_id = field(&row, "vaultId");
    vault_for(facade, &vault_id, &me, Access::Read, false).map_err(|_| fail(format!("No item {item_id}")))?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Write, true)?;
    let revision = int_field(&row, "revision");
    if int(params, "expectedRevision") != Some(revision) {
        return Err(fail(format!(
            "The item changed since you read it (revision {revision}): read it again"
        )));
    }
    let (wrapped, body, key_version) = sealed_parts(params, &vault)?;
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    // an operational item keeps its clear columns unless the write gives all three; a binding that changes must be
    // free in the vault
    let gives_clear = ["vaultItemTypeId", "bindingKey", "itemName"]
        .iter()
        .any(|name| !text(params, name).is_empty());
    let clear = if operational && !gives_clear {
        Vec::new()
    } else {
        clear_columns(params, operational, facade)?
    };
    if let Some((_, binding)) = clear.iter().find(|(name, _)| *name == "bindingKey") {
        let taken = find(facade, ITEM)?
            .eq("vaultId", Value::string(&vault_id))
            .eq("bindingKey", Value::string(binding))
            .one()?;
        if taken.is_some_and(|other| field(&other, "vaultItemId") != item_id) {
            return Err(fail(format!("The binding {binding} is taken in this vault")));
        }
    }
    let mut history = facade.make_value(ITEM_HISTORY)?;
    for name in [
        "vaultItemId",
        "revision",
        "vaultId",
        "itemKeyWrapEnumId",
        "wrappedItemKey",
        "keyVersion",
        "bodyCipher",
        "vaultItemTypeId",
        "bindingKey",
        "itemName",
    ] {
        if let Some(given) = row.get(name).cloned() {
            history.set(name, given);
        }
    }
    history.set("changedDate", now());
    history.set("changedByUserId", Value::string(&me));
    history.create(facade)?;
    // decision (c) of §9: an item key made before the vault's current key version (a rotation rewraps item keys, it does
    // not make new ones) is replaced at the item's first write after it
    let new_item_key = crate::common::flag(params, "newItemKey").unwrap_or(false);
    let made_at = int_field(&row, "itemKeyVersion");
    if made_at < key_version && !new_item_key {
        return Err(fail(
            "The item's key was made before the vault's current key version: seal it with a new item key (newItemKey)",
        ));
    }
    if new_item_key {
        row.set("itemKeyVersion", Value::I64(key_version));
    }
    row.set("itemKeyWrapEnumId", Value::string("VwVault"));
    row.set("wrappedItemKey", Value::string(wrapped));
    row.set("keyVersion", Value::I64(key_version));
    row.set("bodyCipher", Value::string(body));
    row.set("revision", Value::I64(revision + 1));
    for (name, given) in clear {
        row.set(name, Value::string(given));
    }
    // a new item key (decision (c) of §9: the first write after a key-version change, or the writer's choice) leaves an
    // environment's sealed key of this item stale (§3.8): marked here, so a machine lease under it is refused clearly
    row.set("lastModifiedDate", now());
    row.set("lastModifiedByUserId", Value::string(&me));
    // a value the writer changed at its source by hand (a credential the manager cannot rotate, §4.7): its age starts
    // again; taking a deposit in, or a change of its notes, leaves it
    if crate::common::flag(params, "rotated").unwrap_or(false) {
        row.set("rotatedDate", now());
    }
    row.update(facade)?;
    if new_item_key {
        let access = find(facade, crate::common::MACHINE_ACCESS)?
            .eq("vaultItemId", Value::string(&item_id))
            .list()?;
        for granted in access.iter() {
            let mut granted = granted.clone();
            granted.set("staleDate", now());
            granted.update(facade)?;
        }
    }
    bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeItemUpdated",
            vault_id: Some(&vault_id),
            item_id: Some(&item_id),
            key_version: Some(key_version),
            ..Event::default()
        },
    )?;
    Ok(out([("revision", Value::I64(revision + 1))]))
}

/// An item row for a member: its wrapped key and body (a personal vault's), or its clear columns and no body (an
/// operational vault's).
fn item_json(row: &EntityValue, operational: bool, names: &[&str]) -> Value {
    let mut map = ValueMap::default();
    for name in names {
        if operational && *name == "bodyCipher" {
            continue;
        }
        if let Some(given) = row.get(name) {
            map.insert(name.to_string(), given.clone());
        }
    }
    Value::Map(map)
}

const ITEM_FIELDS: &[&str] = &[
    "vaultItemId",
    "vaultId",
    "itemKeyWrapEnumId",
    "wrappedItemKey",
    "keyVersion",
    "itemKeyVersion",
    "bodyCipher",
    "revision",
    "vaultItemTypeId",
    "bindingKey",
    "itemName",
    "createdDate",
    "createdByUserId",
    "lastModifiedDate",
    "lastModifiedByUserId",
];

/// `list#VaultItems`: a vault's items, to a member.
fn list_vault_items(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let vault = vault_for(facade, &vault_id, &me, Access::Read, false)?;
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    let rows = find(facade, ITEM)?
        .eq("vaultId", Value::string(&vault_id))
        .order_by("createdDate", false)
        .list()?;
    let items = rows
        .iter()
        .map(|row| item_json(row, operational, ITEM_FIELDS))
        .collect();
    Ok(out([
        ("itemList", Value::List(items)),
        ("keyVersion", Value::I64(int_field(&vault, "keyVersion"))),
    ]))
}

/// `list#VaultItemHistory`: an item's earlier revisions, to a member of its vault.
fn list_vault_item_history(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let item_id = required(params, "vaultItemId")?;
    let row = item_row(facade, &item_id)?;
    let vault = vault_for(facade, &field(&row, "vaultId"), &me, Access::Read, false)
        .map_err(|_| fail(format!("No item {item_id}")))?;
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    let rows = find(facade, ITEM_HISTORY)?
        .eq("vaultItemId", Value::string(&item_id))
        .order_by("revision", true)
        .list()?;
    let names = [
        "vaultItemId",
        "revision",
        "itemKeyWrapEnumId",
        "wrappedItemKey",
        "keyVersion",
        "bodyCipher",
        "vaultItemTypeId",
        "bindingKey",
        "itemName",
        "changedDate",
        "changedByUserId",
    ];
    let history = rows.iter().map(|row| item_json(row, operational, &names)).collect();
    Ok(out([("historyList", Value::List(history))]))
}

/// `delete#VaultItem`: an item and its history, by a member who may write.
fn delete_vault_item(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let item_id = required(params, "vaultItemId")?;
    let row = item_row(facade, &item_id)?;
    let vault_id = field(&row, "vaultId");
    vault_for(facade, &vault_id, &me, Access::Read, false).map_err(|_| fail(format!("No item {item_id}")))?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Write, true)?;
    require_recent_auth()?;
    let history = find(facade, ITEM_HISTORY)?
        .eq("vaultItemId", Value::string(&item_id))
        .list()?;
    for old in history.iter() {
        old.delete(facade)?;
    }
    row.delete(facade)?;
    bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeItemDeleted",
            vault_id: Some(&vault_id),
            item_id: Some(&item_id),
            ..Event::default()
        },
    )?;
    Ok(out([("vaultItemId", Value::string(item_id))]))
}

/// `set#VaultItemRotation`: an operational item's own rotation age in days, over its type's (plans/Vault-Instances.md
/// §4.7), by a manager of its vault; empty clears it, so the type's applies. An event, with no value.
fn set_vault_item_rotation(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let item_id = required(params, "vaultItemId")?;
    let mut row = item_row(facade, &item_id)?;
    let vault_id = field(&row, "vaultId");
    vault_for(facade, &vault_id, &me, Access::Read, false).map_err(|_| fail(format!("No item {item_id}")))?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Manage, true)?;
    if field(&vault, "vaultKindEnumId") != OPERATIONAL {
        return Err(fail("A rotation age is an operational item's"));
    }
    let given = text(params, "rotationDays");
    let days = if given.is_empty() {
        None
    } else {
        Some(
            given
                .parse::<i64>()
                .ok()
                .filter(|days| (1..=3650).contains(days))
                .ok_or_else(|| fail("rotationDays is a whole number of days from 1 to 3650, or empty"))?,
        )
    };
    row.set("rotationDays", days.map_or(Value::Null, Value::I64));
    row.update(facade)?;
    bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeItemUpdated",
            vault_id: Some(&vault_id),
            item_id: Some(&item_id),
            detail: Some(match days {
                Some(days) => format!("rotation age {days} days"),
                None => "rotation age of its type".to_string(),
            }),
            ..Event::default()
        },
    )?;
    Ok(out([("rotationDays", days.map_or(Value::Null, Value::I64))]))
}
