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

//! The deposit key of an operational vault (plans/Vault-Instances.md §3.2): a key pair whose public half is on the
//! vault in the clear, signed by the manager who made it, and whose private half is wrapped by the vault key. The server
//! writes a new item with no lease (a password it makes while provisioning) by wrapping its item key to the public
//! half, after it checks that signature against the vault's signed member list (`Chain::accept_deposit`): it can write
//! the item and cannot read it back. A deposit key with no manager's signature, or of an earlier key version, is
//! refused, both when it is stored (`set#VaultDepositKey`) and at each deposit ([`deposit_item`]). A removal's
//! rotation stores the new version's deposit key (`remove#VaultMember`, [`store_rotated`]).

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::hpke;
use moqui_vault::keyversion::Chain;
use moqui_vault::rng::OsRng;
use moqui_vault::text::{hex, unhex};
use moqui_vault::vault::{self, DepositPublic, ItemRef};
use serde_json::{json, Value as Json};

use crate::common::{
    authz_off, bump_revision, check_hex, check_id, fail, field, find, id_has_history, id_not_available, int_field,
    json, now, out, refused, required, user, vault_for, vault_row, Access, Out, Params, ITEM, MAX_WRAP_HEX,
    OPERATIONAL,
};
use crate::events::{self, Event};
use crate::members::{chain_entries, check_revision, verify_chain};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, set_vault_deposit_key)?;
    Ok(())
}

/// A deposit key's public half from the vault crate's JSON shape (`ops_json::deposit_json`).
pub(crate) fn deposit_of(given: &Json) -> Result<DepositPublic, LibError> {
    let text = |name: &str| given[name].as_str().unwrap_or_default().to_string();
    let public = unhex(&text("public")).map_err(refused)?;
    Ok(DepositPublic {
        vault_id: text("vault_id"),
        key_version: given["key_version"]
            .as_u64()
            .ok_or_else(|| fail("a deposit key's key_version"))?,
        public: hpke::PublicKey::from_bytes(&public).map_err(refused)?,
        signer_user_id: text("signer_user_id"),
        signer_key_id: text("signer_key_id"),
        signature: unhex(&text("signature")).map_err(refused)?,
    })
}

/// The vault's deposit key as the vault crate shapes it, none when it has none.
pub(crate) fn deposit_json(vault: &EntityValue) -> Json {
    let public = field(vault, "depositPublic");
    if public.is_empty() {
        return Json::Null;
    }
    json!({
        "vault_id": field(vault, "vaultId"), "key_version": int_field(vault, "depositKeyVersion"), "public": public,
        "signer_user_id": field(vault, "depositSignerUserId"), "signer_key_id": field(vault, "depositSignerKeyId"),
        "signature": field(vault, "depositSignature"),
    })
}

/// Checks a deposit key against the chain (of the current key version, signed by a manager of it) and that the
/// caller made it, and puts it on the vault row.
fn accept_and_set(
    facade: &EntityFacade,
    vault: &mut EntityValue,
    chain: &Chain,
    deposit: &DepositPublic,
    wrapped_private: &str,
    me: &str,
) -> Result<(), LibError> {
    let mine = crate::personal::my_key(facade, me)?;
    if deposit.signer_user_id != me || deposit.signer_key_id != field(&mine, "personalKeyId") {
        return Err(fail("Sign the deposit key yourself, with your active personal key"));
    }
    chain.accept_deposit(deposit).map_err(refused)?;
    check_hex("depositWrappedPrivate", wrapped_private, MAX_WRAP_HEX)?;
    vault.set("depositPublic", Value::string(hex(deposit.public.as_bytes())));
    vault.set(
        "depositKeyVersion",
        Value::I64(crate::members::version(deposit.key_version)?),
    );
    vault.set("depositSignerUserId", Value::string(&deposit.signer_user_id));
    vault.set("depositSignerKeyId", Value::string(&deposit.signer_key_id));
    vault.set("depositSignature", Value::string(hex(&deposit.signature)));
    vault.set("depositWrappedPrivate", Value::string(wrapped_private));
    Ok(())
}

/// `set#VaultDepositKey`: a manager of an operational vault puts its deposit key, made in their client with the vault
/// key of the current version and signed by them; the change names the vault's revision. An event.
fn set_vault_deposit_key(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = required(params, "vaultId")?;
    let mut vault = vault_for(facade, &vault_id, &me, Access::Manage, true)?;
    if field(&vault, "vaultKindEnumId") != OPERATIONAL {
        return Err(fail("Only an operational vault has a deposit key"));
    }
    check_revision(params, &vault)?;
    let deposit = deposit_of(&json(params, "deposit")?)?;
    let wrapped = required(params, "depositWrappedPrivate")?;
    let entries = chain_entries(facade, &vault_id)?;
    let chain = verify_chain(&entries)?;
    accept_and_set(facade, &mut vault, &chain, &deposit, &wrapped, &me)?;
    let revision = bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeDepositKeySet",
            vault_id: Some(&vault_id),
            key_version: Some(crate::members::version(deposit.key_version)?),
            ..Event::default()
        },
    )?;
    Ok(out([("revision", Value::I64(revision))]))
}

/// A removal's rotation (§3.9): the new version's deposit key when the remover's client made one, else the old one
/// cleared, since a deposit to it would be refused.
pub(crate) fn store_rotated(
    facade: &EntityFacade,
    params: &Params,
    vault: &mut EntityValue,
    chain: &Chain,
    me: &str,
) -> Result<(), LibError> {
    let given = params
        .get("deposit")
        .is_some_and(|given| !moqui_lib::object::is_empty(given));
    if given {
        let deposit = deposit_of(&json(params, "deposit")?)?;
        let wrapped = required(params, "depositWrappedPrivate")?;
        return accept_and_set(facade, vault, chain, &deposit, &wrapped, me);
    }
    for name in [
        "depositPublic",
        "depositKeyVersion",
        "depositSignerUserId",
        "depositSignerKeyId",
        "depositSignature",
        "depositWrappedPrivate",
    ] {
        vault.set(name, Value::Null);
    }
    Ok(())
}

/// What the server deposits: a new operational item it made (a password it generated while provisioning).
#[derive(Debug, Clone, Copy)]
pub struct Deposit<'a> {
    pub vault_id: &'a str,
    pub item_id: &'a str,
    pub item_type: &'a str,
    pub binding: &'a str,
    pub name: &'a str,
    /// The item's JSON body (`{"v": 1, "type", "name", "fields": {...}}`).
    pub body: &'a [u8],
}

/// Deposits a new item into an operational vault with no lease: its item key wrapped to the vault's deposit key once
/// that key's signature is checked against the vault's signed member list, its body sealed under the item key, the row
/// and its `VeItemCreated` event in the caller's transaction. The server cannot read the item back.
pub fn deposit_item(facade: &EntityFacade, deposit: &Deposit) -> Result<(), LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(ITEM)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        deposit_in_tx(facade, deposit)
    })
}

fn deposit_in_tx(facade: &EntityFacade, deposit: &Deposit) -> Result<(), LibError> {
    let mut vault = vault_row(facade, deposit.vault_id, true)?;
    if field(&vault, "vaultKindEnumId") != OPERATIONAL {
        return Err(fail("The server deposits only into an operational vault"));
    }
    let shaped = deposit_json(&vault);
    if shaped.is_null() {
        return Err(fail(format!(
            "Vault {} has no deposit key: a manager makes one in their client",
            deposit.vault_id
        )));
    }
    let public = deposit_of(&shaped)?;
    let entries = chain_entries(facade, deposit.vault_id)?;
    let chain = verify_chain(&entries)?;
    let signer = chain.accept_deposit(&public).map_err(refused)?;
    check_id("vaultItemId", deposit.item_id)?;
    if [deposit.item_type, deposit.binding, deposit.name]
        .iter()
        .any(|given| given.is_empty() || crate::lease::has_control(given))
    {
        return Err(fail(
            "A deposited item has its type, binding and name, with no control character",
        ));
    }
    let taken = find(facade, ITEM)?
        .eq("vaultItemId", Value::string(deposit.item_id))
        .one()?
        .is_some();
    if taken || id_has_history(facade, "vaultItemId", deposit.item_id)? {
        return Err(id_not_available("Item", deposit.item_id));
    }
    if find(facade, ITEM)?
        .eq("vaultId", Value::string(deposit.vault_id))
        .eq("bindingKey", Value::string(deposit.binding))
        .one()?
        .is_some()
    {
        return Err(fail(format!("The binding {} is taken in this vault", deposit.binding)));
    }
    if find(facade, "moqui.security.vault.VaultItemType")?
        .eq("vaultItemTypeId", Value::string(deposit.item_type))
        .one()?
        .is_none()
    {
        return Err(fail(format!("No item type {}", deposit.item_type)));
    }
    let key_version = int_field(&vault, "keyVersion");
    let item = ItemRef {
        vault_id: deposit.vault_id,
        item_id: deposit.item_id,
        key_version: u64::try_from(key_version).map_err(|_| fail("a key version out of range"))?,
        label: Some(moqui_vault::names::ItemLabel {
            item_type: deposit.item_type,
            binding: deposit.binding,
            name: deposit.name,
        }),
    };
    let item_key = vault::new_item_key(&mut OsRng);
    let wrapped = vault::wrap_item_key_to_deposit(&public, &signer, &item_key, item, &mut OsRng).map_err(refused)?;
    let sealed = vault::seal_item_body(&item_key, item, deposit.body, &mut OsRng).map_err(refused)?;
    let actor = moqui_core::user::current_user_id().ok().flatten();
    let mut row = facade.make_value(ITEM)?;
    row.set("vaultItemId", Value::string(deposit.item_id));
    row.set("vaultId", Value::string(deposit.vault_id));
    row.set("itemKeyWrapEnumId", Value::string("VwDeposit"));
    row.set("wrappedItemKey", Value::string(hex(&wrapped)));
    row.set("keyVersion", Value::I64(key_version));
    row.set("bodyCipher", Value::string(hex(&sealed)));
    row.set("revision", Value::I64(1));
    row.set("vaultItemTypeId", Value::string(deposit.item_type));
    row.set("bindingKey", Value::string(deposit.binding));
    row.set("itemName", Value::string(deposit.name));
    row.set("createdDate", now());
    row.set("lastModifiedDate", now());
    if let Some(actor) = &actor {
        row.set("createdByUserId", Value::string(actor));
        row.set("lastModifiedByUserId", Value::string(actor));
    }
    row.create(facade)?;
    bump_revision(facade, &mut vault)?;
    events::write(
        facade,
        Event {
            kind: "VeItemCreated",
            vault_id: Some(deposit.vault_id),
            item_id: Some(deposit.item_id),
            key_version: Some(key_version),
            actor: Some("server"),
            detail: Some(format!("{} deposited by the server", deposit.binding)),
            ..Event::default()
        },
    )
}
