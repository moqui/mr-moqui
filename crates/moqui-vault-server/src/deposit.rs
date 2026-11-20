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
//! rotation stores the new version's deposit key (`remove#VaultMember`, [`store_rotated`]). A credential the instance
//! manager rotated at its source (§4.7) is written the same way, as the item's next revision with the one before kept in
//! its history: staged before its source changes ([`stage_revision`]), then promoted ([`promote_revision`]).

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
    json, now, out, refused, required, user, vault_for, vault_row, Access, Out, Params, ITEM, ITEM_PENDING,
    MACHINE_ACCESS, MAX_WRAP_HEX, OPERATIONAL,
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

/// The vault's deposit key once its signature is checked against the vault's signed member list (of the current key
/// version, signed by a manager of it), with its signer.
fn accepted_key(
    facade: &EntityFacade,
    vault: &EntityValue,
) -> Result<(DepositPublic, moqui_vault::sign::VerifyingKey), LibError> {
    let vault_id = field(vault, "vaultId");
    if field(vault, "vaultKindEnumId") != OPERATIONAL {
        return Err(fail("The server deposits only into an operational vault"));
    }
    let shaped = deposit_json(vault);
    if shaped.is_null() {
        return Err(fail(format!(
            "Vault {vault_id} has no deposit key: a manager makes one in their client"
        )));
    }
    let public = deposit_of(&shaped)?;
    let entries = chain_entries(facade, &vault_id)?;
    let chain = verify_chain(&entries)?;
    let signer = chain.accept_deposit(&public).map_err(refused)?;
    Ok((public, signer))
}

/// Whether the server could deposit into `vault_id` now: its deposit key is there and its signature checks. A rotation
/// asks this before it changes anything at a source (plans/Vault-Instances.md §4.7), since its last step deposits.
pub fn check_deposit_key(facade: &EntityFacade, vault_id: &str) -> Result<(), LibError> {
    let _authz = authz_off();
    let vault = vault_row(facade, vault_id, false)?;
    accepted_key(facade, &vault).map(|_| ())
}

fn deposit_in_tx(facade: &EntityFacade, deposit: &Deposit) -> Result<(), LibError> {
    let mut vault = vault_row(facade, deposit.vault_id, true)?;
    let (public, signer) = accepted_key(facade, &vault)?;
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
    row.set("itemKeyVersion", Value::I64(key_version));
    row.set("bodyCipher", Value::string(hex(&sealed)));
    row.set("revision", Value::I64(1));
    row.set("vaultItemTypeId", Value::string(deposit.item_type));
    row.set("bindingKey", Value::string(deposit.binding));
    row.set("itemName", Value::string(deposit.name));
    row.set("createdDate", now());
    row.set("lastModifiedDate", now());
    row.set("rotatedDate", now());
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

/// Withdraws an item the server deposited when the operation that made it failed before using it (M1: a provision
/// undone): the row and its `VeItemDeleted` event in the caller's transaction. Only an item still wrapped to the deposit
/// key, which no member has taken in, of the vault and binding named; any other is refused, so this deletes nothing a
/// member wrote or adopted.
pub fn withdraw_item(facade: &EntityFacade, vault_id: &str, item_id: &str, binding: &str) -> Result<(), LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(ITEM)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let row = find(facade, ITEM)?
            .eq("vaultItemId", Value::string(item_id))
            .one()?
            .ok_or_else(|| fail(format!("No item {item_id}")))?;
        // revision 1 only: a rotated item is wrapped to the deposit key too, and is not the provision's to withdraw
        if field(&row, "vaultId") != vault_id
            || field(&row, "bindingKey") != binding
            || field(&row, "itemKeyWrapEnumId") != "VwDeposit"
            || int_field(&row, "revision") != 1
        {
            return Err(fail(format!(
                "Item {item_id} is not the server's deposit of {binding}: it is not withdrawn"
            )));
        }
        let mut vault = vault_row(facade, vault_id, true)?;
        row.delete(facade)?;
        bump_revision(facade, &mut vault)?;
        events::write(
            facade,
            Event {
                kind: "VeItemDeleted",
                vault_id: Some(vault_id),
                item_id: Some(item_id),
                actor: Some("server"),
                detail: Some(format!(
                    "{binding} withdrawn: the operation that deposited it was undone"
                )),
                ..Event::default()
            },
        )
    })
}

// ----- a rotation's new value (plans/Vault-Instances.md §4.7) -----

/// A rotation's new body of an item, to stage before its source changes: the item, by id and binding, the revision the
/// rotation started from, and the whole new JSON body (the item's own, with the rotated field changed: the lease's
/// [`crate::lease::LeaseRegistry::stage_rotation`] makes it, so the rest of the body never leaves this crate).
#[derive(Debug, Clone, Copy)]
pub struct Staged<'a> {
    pub vault_id: &'a str,
    pub item_id: &'a str,
    pub binding: &'a str,
    pub expected_revision: i64,
    pub body: &'a [u8],
}

fn pending_row(facade: &EntityFacade, item_id: &str, pending_id: &str) -> Result<EntityValue, LibError> {
    find(facade, ITEM_PENDING)?
        .eq("vaultItemId", Value::string(item_id))
        .eq("pendingId", Value::string(pending_id))
        .one()?
        .ok_or_else(|| fail(format!("No pending revision {pending_id} of item {item_id}")))
}

/// The item of a rotation, refused unless it is `binding` of `vault_id` at `expected` (another write or rotation came
/// first: reasonCode itemChanged).
fn item_at(
    facade: &EntityFacade,
    vault_id: &str,
    item_id: &str,
    binding: &str,
    expected: i64,
) -> Result<EntityValue, LibError> {
    let row = find(facade, ITEM)?
        .eq("vaultItemId", Value::string(item_id))
        .one()?
        .ok_or_else(|| fail(format!("No item {item_id}")))?;
    if field(&row, "vaultId") != vault_id || field(&row, "bindingKey") != binding {
        return Err(fail(format!("Item {item_id} is not {binding} of vault {vault_id}")));
    }
    let revision = int_field(&row, "revision");
    if revision != expected {
        return Err(fail(format!(
            "{binding} changed since the rotation read it (revision {revision}, not {expected}): another write or rotation \
             came first (reasonCode itemChanged)"
        )));
    }
    Ok(row)
}

/// Stages a rotation's new value before its source is changed, with no lease: a new item key wrapped to the vault's
/// deposit key (checked as [`deposit_item`] checks it), the body sealed under it at the vault's current key version,
/// kept as a `VaultItemPending` row the server cannot read back. Refused when the item is not at the revision the
/// rotation started from. The vault's revision moves, so a member removal computed before it is refused. Its pending id.
pub fn stage_revision(facade: &EntityFacade, staged: &Staged) -> Result<String, LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(ITEM)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut vault = vault_row(facade, staged.vault_id, true)?;
        let (public, signer) = accepted_key(facade, &vault)?;
        let row = item_at(
            facade,
            staged.vault_id,
            staged.item_id,
            staged.binding,
            staged.expected_revision,
        )?;
        let (item_type, name) = (field(&row, "vaultItemTypeId"), field(&row, "itemName"));
        let key_version = int_field(&vault, "keyVersion");
        let item = ItemRef {
            vault_id: staged.vault_id,
            item_id: staged.item_id,
            key_version: u64::try_from(key_version).map_err(|_| fail("a key version out of range"))?,
            label: Some(moqui_vault::names::ItemLabel {
                item_type: &item_type,
                binding: staged.binding,
                name: &name,
            }),
        };
        let item_key = vault::new_item_key(&mut OsRng);
        let wrapped =
            vault::wrap_item_key_to_deposit(&public, &signer, &item_key, item, &mut OsRng).map_err(refused)?;
        let sealed = vault::seal_item_body(&item_key, item, staged.body, &mut OsRng).map_err(refused)?;
        let mut tag = [0_u8; 8];
        moqui_vault::rng::Rng::fill(&mut OsRng, &mut tag);
        let pending_id = format!("rot-{}", hex(&tag));
        let mut pending = facade.make_value(ITEM_PENDING)?;
        pending.set("vaultItemId", Value::string(staged.item_id));
        pending.set("pendingId", Value::string(&pending_id));
        pending.set("vaultId", Value::string(staged.vault_id));
        pending.set("itemKeyWrapEnumId", Value::string("VwDeposit"));
        pending.set("wrappedItemKey", Value::string(hex(&wrapped)));
        pending.set("keyVersion", Value::I64(key_version));
        pending.set("bodyCipher", Value::string(hex(&sealed)));
        pending.set("vaultItemTypeId", Value::string(&item_type));
        pending.set("bindingKey", Value::string(staged.binding));
        pending.set("itemName", Value::string(&name));
        pending.set("expectedRevision", Value::I64(staged.expected_revision));
        pending.set("createdDate", now());
        if let Ok(Some(actor)) = moqui_core::user::current_user_id() {
            pending.set("createdByUserId", Value::string(actor));
        }
        pending.create(facade)?;
        bump_revision(facade, &mut vault)?;
        Ok(pending_id)
    })
}

/// Makes a staged value the item's next revision, once the source and the instances took it: refused unless the item
/// is still at the revision the rotation started from and the vault at the key version it was staged at. The revision
/// before is kept in `VaultItemHistory` with its wrapped key and body as they were (the way back, which a member opens
/// in their client); the item is wrapped to the deposit key, so a member takes it in before a lease names it again, and
/// an environment's machine access to it is marked stale (§3.8: its sealed key is of the old item key). The item's
/// `rotatedDate` is now, the vault's revision moves, and the `VeItemRotated` event is in the same transaction. Its new
/// revision.
pub fn promote_revision(
    facade: &EntityFacade,
    vault_id: &str,
    item_id: &str,
    binding: &str,
    pending_id: &str,
) -> Result<i64, LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(ITEM)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut vault = vault_row(facade, vault_id, true)?;
        let pending = pending_row(facade, item_id, pending_id)?;
        let expected = int_field(&pending, "expectedRevision");
        let mut row = item_at(facade, vault_id, item_id, binding, expected)?;
        let key_version = int_field(&vault, "keyVersion");
        if int_field(&pending, "keyVersion") != key_version {
            return Err(fail(format!(
                "The vault key of {vault_id} changed during the rotation of {binding}: its new value is not written"
            )));
        }
        let actor = moqui_core::user::current_user_id().ok().flatten();
        crate::items::keep_history(facade, &row, actor.as_deref())?;
        let next = expected + 1;
        for name in ["itemKeyWrapEnumId", "wrappedItemKey", "bodyCipher"] {
            row.set(name, pending.get(name).cloned().unwrap_or(Value::Null));
        }
        row.set("keyVersion", Value::I64(key_version));
        row.set("itemKeyVersion", Value::I64(key_version));
        row.set("revision", Value::I64(next));
        row.set("rotatedDate", now());
        row.set("lastModifiedDate", now());
        if let Some(actor) = &actor {
            row.set("lastModifiedByUserId", Value::string(actor));
        }
        row.update(facade)?;
        pending.delete(facade)?;
        for granted in find(facade, MACHINE_ACCESS)?
            .eq("vaultItemId", Value::string(item_id))
            .list()?
            .iter()
        {
            let mut granted = granted.clone();
            granted.set("staleDate", now());
            granted.update(facade)?;
        }
        bump_revision(facade, &mut vault)?;
        events::write(
            facade,
            Event {
                kind: "VeItemRotated",
                vault_id: Some(vault_id),
                item_id: Some(item_id),
                key_version: Some(key_version),
                actor: Some("server"),
                detail: Some(format!(
                    "{binding} rotated at its source: revision {next} deposited by the server, the one before kept in \
                     its history"
                )),
                ..Event::default()
            },
        )?;
        Ok(next)
    })
}

/// Drops a staged value: the rotation put the old value back, so nothing holds the new one.
pub fn drop_pending(facade: &EntityFacade, item_id: &str, pending_id: &str) -> Result<(), LibError> {
    let _authz = authz_off();
    pending_row(facade, item_id, pending_id)?.delete(facade)
}

/// Keeps a staged value when the rotation could not put the old value back at its source, which then holds this one:
/// a member opens it in their client (list#VaultItemHistory) to recover it. An event, with `why` and no value.
pub fn keep_pending(facade: &EntityFacade, item_id: &str, pending_id: &str, why: &str) -> Result<(), LibError> {
    let _authz = authz_off();
    let tx = facade.transactions_for(ITEM)?;
    tx.run(TransactionAttribute::UseOrBegin, None, || {
        let mut pending = pending_row(facade, item_id, pending_id)?;
        pending.set("keptDate", now());
        pending.set("keptReason", Value::string(why.chars().take(250).collect::<String>()));
        pending.update(facade)?;
        let vault_id = field(&pending, "vaultId");
        events::write(
            facade,
            Event {
                kind: "VeItemUpdated",
                vault_id: Some(&vault_id),
                item_id: Some(item_id),
                actor: Some("server"),
                detail: Some(format!(
                    "{}: a rotation's new value is kept as pending revision {pending_id}, since its source may hold it: \
                     open it in your client",
                    field(&pending, "bindingKey")
                )),
                ..Event::default()
            },
        )
    })
}
