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

//! Reveal (plans/Vault-Instances.md §3.7): a member with the reveal role sees one operational value. The server sends
//! that item's ciphertext and wrapped key to the member's client, which opens it with the vault key there, and the
//! reveal is a `VeReveal` event, written before the ciphertext is answered. Otherwise no operational body goes to any
//! client (`list#VaultItems` leaves it out).

use std::collections::BTreeMap;

use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::object::Value;

use crate::common::{
    authz_off, fail, field, find, out, required, user, vault_for, Access, Out, Params, ITEM, OPERATIONAL,
};
use crate::events::{self, Event};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, reveal_vault_item)?;
    Ok(())
}

/// `reveal#VaultItem`: one operational item's wrapped key and body, with its clear columns (its associated data), to a
/// member with the reveal role. An event, in the same transaction: no event, no ciphertext.
fn reveal_vault_item(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let item_id = required(params, "vaultItemId")?;
    let not_found = || fail(format!("No item {item_id}"));
    let row = find(facade, ITEM)?
        .eq("vaultItemId", Value::string(&item_id))
        .one()?
        .ok_or_else(not_found)?;
    let vault_id = field(&row, "vaultId");
    let vault = vault_for(facade, &vault_id, &me, Access::Read, false).map_err(|_| not_found())?;
    if field(&vault, "vaultKindEnumId") != OPERATIONAL {
        return Err(fail(
            "A reveal is of an operational item: a personal vault's member reads its items",
        ));
    }
    vault_for(facade, &vault_id, &me, Access::Reveal, false)
        .map_err(|_| fail("Seeing an operational value needs the reveal role in its vault"))?;
    events::write(
        facade,
        Event {
            kind: "VeReveal",
            vault_id: Some(&vault_id),
            item_id: Some(&item_id),
            detail: Some(format!("{} revealed", field(&row, "bindingKey"))),
            ..Event::default()
        },
    )?;
    let mut answer = BTreeMap::new();
    for name in [
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
    ] {
        if let Some(given) = row.get(name) {
            answer.insert(name.to_string(), given.clone());
        }
    }
    answer.extend(out([("revealed", Value::Bool(true))]));
    Ok(answer)
}
