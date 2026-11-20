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
    authz_off, check_id, fail, field, find, int, int_field, now, out, refused, required, text, user, vault_for, Access,
    Out, Params, ITEM, ITEM_HISTORY, OPERATIONAL,
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
    moqui_vault::text::unhex(&wrapped).map_err(refused)?;
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
    let vault = vault_for(facade, &vault_id, &me, Access::Write, true)?;
    let item_id = required(params, "vaultItemId")?;
    check_id("vaultItemId", &item_id)?;
    if find(facade, ITEM)?
        .eq("vaultItemId", Value::string(&item_id))
        .one()?
        .is_some()
    {
        return Err(fail(format!("Item {item_id} exists already")));
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
    row.set("bodyCipher", Value::string(body));
    row.set("revision", Value::I64(1));
    for (name, given) in clear {
        row.set(name, Value::string(given));
    }
    row.set("createdDate", now());
    row.set("createdByUserId", Value::string(&me));
    row.set("lastModifiedDate", now());
    row.set("lastModifiedByUserId", Value::string(&me));
    row.create(facade)?;
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
    let vault = vault_for(facade, &vault_id, &me, Access::Write, true)?;
    let revision = int_field(&row, "revision");
    if int(params, "expectedRevision") != Some(revision) {
        return Err(fail(format!(
            "The item changed since you read it (revision {revision}): read it again"
        )));
    }
    let (wrapped, body, key_version) = sealed_parts(params, &vault)?;
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    let clear = if operational && text(params, "bindingKey").is_empty() {
        Vec::new()
    } else {
        clear_columns(params, operational, facade)?
    };
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
    row.set("itemKeyWrapEnumId", Value::string("VwVault"));
    row.set("wrappedItemKey", Value::string(wrapped));
    row.set("keyVersion", Value::I64(key_version));
    row.set("bodyCipher", Value::string(body));
    row.set("revision", Value::I64(revision + 1));
    for (name, given) in clear {
        row.set(name, Value::string(given));
    }
    row.set("lastModifiedDate", now());
    row.set("lastModifiedByUserId", Value::string(&me));
    row.update(facade)?;
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
    vault_for(facade, &vault_id, &me, Access::Write, true)?;
    let history = find(facade, ITEM_HISTORY)?
        .eq("vaultItemId", Value::string(&item_id))
        .list()?;
    for old in history.iter() {
        old.delete(facade)?;
    }
    row.delete(facade)?;
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
