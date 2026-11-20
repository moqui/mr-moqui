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

//! Machine access (plans/Vault-Instances.md §3.8): an environment that needs unattended work holds the item keys of
//! the items its operations need, sealed by the vault crate's AEAD under the server's machine key with the environment,
//! vault and item ids as associated data, so a row moved to another environment or item does not open.
//!
//! - A holder of `VAULT_ADMIN` asks for it (`ask#VaultMachineAccess`): the server makes a lease for the grant, which a
//!   member with the approve role approves in their client as any lease. Once it opens on the member that asked
//!   ([`complete_grant`]), each item key is sealed under the machine key into a `VaultMachineAccess` row, with a
//!   `VeMachineGiven` event, and the lease ends.
//! - The machine key is HKDF of `entity-facade@crypt-pass` with the label `vault-machine` until Sealed-Secrets S1
//!   ([`machine_key`]); the pass is read through the factory's secret handle, as the entity facade's cipher is made
//!   (since S4 from `seal.json` when the conf leaves it at `CHANGEME`). Whoever holds that pass and the database opens
//!   these items, so they are as strong as the server's own keys. After `operator rotate` of the pass, a row sealed
//!   under the pass before it (now `entity-facade@crypt-pass-old`) still opens and is sealed again under the new one.
//!   The column is not an entity `encrypt="true"` field, so no find, cache, export or sync decrypts it; only this code
//!   opens it.
//! - An operation of the environment uses it as a lease approved by "machine" ([`machine_lease`]). The environment is
//!   the operation's own: the provision, deploy and rotation operations (M1, M3, M5) pass the `InstanceEnvironment` of
//!   the instance they act on, never one a request names, so an environment reaches only the items it was given.
//! - An item written with a new item key (decision (c) of §9, after a key-version change) leaves the sealed key stale:
//!   the row is marked stale at that write, and a lease under it is refused until machine access is given again.
//! - Revoking it deletes the sealed keys, ends the machine leases of the environment open on the vault, and names the
//!   items to rotate, since the server could have kept them (`revoke#VaultMachineAccess`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{Value, ValueMap};
use moqui_vault::lease::{Lease, LeaseItem};
use moqui_vault::rng::OsRng;
use moqui_vault::text::{hex, unhex};
use moqui_vault::Key32;

use crate::common::{
    authz_off, check_id, fail, field, find, has_permission, now, out, refused, required, text, user, value, vault_for,
    vault_row, Access, Out, Params, MACHINE_ACCESS,
};
use crate::events::{self, Event};
use crate::lease::{lease_json, registry, services::bindings_param, Ask, LeaseRegistry};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, ask_vault_machine_access)?;
    moqui_core::rust_service!(map, list_vault_machine_access)?;
    moqui_core::rust_service!(map, revoke_vault_machine_access)?;
    Ok(())
}

/// The operation a grant's lease names.
const GRANT: &str = "machine-access";

const CRYPT_PASS: &str = "entity-facade@crypt-pass";
const CRYPT_PASS_OLD: &str = "entity-facade@crypt-pass-old";

/// The server's machine key: HKDF of the field pass, read through the factory's secret handle as the entity facade's
/// cipher is made from it, until the keyring's `vault-machine` purpose key (Sealed-Secrets S1).
pub(crate) fn machine_key() -> Result<Key32, LibError> {
    let pass = crate::common::server_secret(CRYPT_PASS)?.ok_or_else(|| {
        fail("Machine access needs entity-facade@crypt-pass, which the first start generates into the seal's store")
    })?;
    Ok(moqui_vault::machine::machine_key_from_crypt_pass(pass.as_bytes()))
}

/// The machine key of the field pass before the last rotation, when there is one.
fn old_machine_key() -> Option<Key32> {
    let pass = crate::common::server_secret(CRYPT_PASS_OLD).ok().flatten()?;
    // the shipped default of crypt-pass-old is the public default pass, which never sealed a row
    Some(moqui_vault::machine::machine_key_from_crypt_pass(pass.as_bytes()))
}

/// Asks for a grant's lease: the items an environment's operations need, by binding, approved as any lease. Its
/// statement to approve; [`complete_grant`] seals the keys once it opens.
pub fn ask_grant(
    registry: &LeaseRegistry,
    facade: &EntityFacade,
    environment_id: &str,
    vault_id: &str,
    bindings: &[String],
) -> Result<Lease, LibError> {
    check_id("environmentId", environment_id)?;
    registry.ask(
        facade,
        &Ask {
            vault_id,
            bindings,
            operation: &format!("{GRANT} for environment {environment_id}"),
            limit: None,
            environment_id: Some(environment_id),
        },
    )
}

/// Waits up to `timeout` for a grant's lease to open here, then seals each item key under the machine key for the
/// environment, writes the `VaultMachineAccess` rows and their events, and ends the lease.
pub fn complete_grant(
    registry: &LeaseRegistry,
    facade: &EntityFacade,
    lease_id: &str,
    timeout: Duration,
) -> Result<(), LibError> {
    /// The grant's lease ends however this returns: its keys never outlive the grant.
    struct EndLease<'a> {
        registry: &'a LeaseRegistry,
        facade: &'a EntityFacade,
        lease_id: &'a str,
    }
    impl Drop for EndLease<'_> {
        fn drop(&mut self) {
            if let Err(err) = self.registry.end(self.facade, self.lease_id) {
                self.registry.forget(self.lease_id);
                tracing::error!("Vault: the grant's lease {} was not marked ended: {err}", self.lease_id);
            }
        }
    }
    let _end = EndLease {
        registry,
        facade,
        lease_id,
    };
    // the environment and the vault as this member asked them, never read back from the row's unsigned columns
    let environment_id = registry
        .environment_of(lease_id)
        .ok_or_else(|| fail(format!("The lease {lease_id} grants no machine access")))?;
    let lease = registry
        .held_lease(lease_id)
        .ok_or_else(|| fail(format!("The lease {lease_id} has ended")))?;
    registry.wait_open(facade, lease_id, timeout)?;
    let _authz = authz_off();
    let row = find(facade, crate::common::LEASE)?
        .eq("leaseId", Value::string(lease_id))
        .one()?
        .ok_or_else(|| fail(format!("No lease {lease_id}")))?;
    let approver = field(&row, "approvedByUserId");
    let machine_key = machine_key()?;
    let keys = registry.item_keys(lease_id)?;
    let tx = facade.transactions_for(MACHINE_ACCESS)?;
    tx.run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
        for (item_id, item_key) in &keys {
            let sealed = moqui_vault::machine::seal(
                &machine_key,
                &environment_id,
                &lease.vault_id,
                item_id,
                item_key,
                &mut OsRng,
            );
            let existing = find(facade, MACHINE_ACCESS)?
                .eq("environmentId", Value::string(&environment_id))
                .eq("vaultItemId", Value::string(item_id))
                .one()?;
            if let Some(existing) = existing {
                existing.delete(facade)?;
            }
            let mut access = facade.make_value(MACHINE_ACCESS)?;
            access.set("environmentId", Value::string(&environment_id));
            access.set("vaultItemId", Value::string(item_id));
            access.set("vaultId", Value::string(&lease.vault_id));
            access.set("sealedItemKey", Value::string(hex(&sealed)));
            access.set("grantedByUserId", Value::string(&approver));
            access.set("grantedDate", now());
            access.create(facade)?;
            events::write(
                facade,
                Event {
                    kind: "VeMachineGiven",
                    vault_id: Some(&lease.vault_id),
                    item_id: Some(item_id),
                    lease_id: Some(lease_id),
                    actor: Some(&approver),
                    detail: Some(format!("environment {environment_id}")),
                    ..Event::default()
                },
            )?;
        }
        Ok(())
    })
}

/// An environment's operation under its machine access (§3.8): a lease of the named items approved by "machine",
/// opened at once from the sealed keys. A binding the environment holds no access to is refused.
pub fn machine_lease(
    registry: &LeaseRegistry,
    facade: &EntityFacade,
    environment_id: &str,
    vault_id: &str,
    bindings: &[String],
    operation: &str,
) -> Result<Lease, LibError> {
    let machine_key = machine_key()?;
    let open_key = |item: &LeaseItem| -> Result<Key32, LibError> {
        let row = find(facade, MACHINE_ACCESS)?
            .eq("environmentId", Value::string(environment_id))
            .eq("vaultItemId", Value::string(&item.item_id))
            .one()?
            .ok_or_else(|| {
                fail(format!(
                    "Environment {environment_id} has no machine access to {}",
                    item.binding
                ))
            })?;
        if field(&row, "vaultId") != vault_id {
            return Err(fail(format!(
                "Environment {environment_id} has no machine access to {}",
                item.binding
            )));
        }
        if !field(&row, "staleDate").is_empty() {
            return Err(fail(format!(
                "Environment {environment_id}'s machine access to {} is stale: the item was written with a new item key; \
                 give machine access again (reasonCode machineAccessStale)",
                item.binding
            )));
        }
        let sealed = unhex(&field(&row, "sealedItemKey")).map_err(refused)?;
        match moqui_vault::machine::open(&machine_key, environment_id, vault_id, &item.item_id, &sealed) {
            Ok(key) => Ok(key),
            Err(err) => {
                // sealed under the field pass before its rotation: opened with it and sealed again under the new one
                let Some(old) = old_machine_key() else {
                    return Err(refused(err));
                };
                let key = moqui_vault::machine::open(&old, environment_id, vault_id, &item.item_id, &sealed)
                    .map_err(|_| refused(err))?;
                let resealed =
                    moqui_vault::machine::seal(&machine_key, environment_id, vault_id, &item.item_id, &key, &mut OsRng);
                let mut row = row.clone();
                row.set("sealedItemKey", Value::string(hex(&resealed)));
                row.update(facade)?;
                Ok(key)
            }
        }
    };
    let lease = registry.open_with_keys(
        facade,
        &Ask {
            vault_id,
            bindings,
            operation,
            limit: None,
            environment_id: Some(environment_id),
        },
        &open_key,
    )?;
    for item in &lease.items {
        events::write(
            facade,
            Event {
                kind: "VeMachineUsed",
                vault_id: Some(vault_id),
                item_id: Some(&item.item_id),
                lease_id: Some(&lease.lease_id),
                actor: Some("machine"),
                detail: Some(format!("environment {environment_id}: {operation}")),
                ..Event::default()
            },
        )?;
    }
    Ok(lease)
}

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    moqui_core::context::ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
        .ok_or_else(|| fail("No execution context is entered"))
}

/// `ask#VaultMachineAccess`: a holder of `VAULT_ADMIN` asks to give an environment machine access to named items. The
/// grant's lease to approve; once an approver approves it, the member that asked seals the keys and ends the lease.
fn ask_vault_machine_access(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    user()?;
    if !has_permission("VAULT_ADMIN")? {
        return Err(fail("Giving machine access needs the VAULT_ADMIN permission"));
    }
    let environment_id = required(params, "environmentId")?;
    let vault_id = required(params, "vaultId")?;
    let bindings = bindings_param(params)?;
    let factory = factory()?;
    let registry = registry(&factory);
    let lease = ask_grant(&registry, facade, &environment_id, &vault_id, &bindings)?;
    let limit = Duration::from_millis(u64::try_from(lease.expires_at - crate::common::now_millis()).unwrap_or(0));
    let lease_id = lease.lease_id.clone();
    // the grant completes on this member once the lease opens: a thread of its own waits for it (the lease's row
    // wakes it at its approval, or its poll finds it)
    let spawned = std::thread::Builder::new()
        .name(format!("vault-grant-{lease_id}"))
        .spawn(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let Some(facade) = factory.entity() else {
                return;
            };
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                complete_grant(&registry, &facade, &lease_id, limit)
            }));
            match ran {
                Ok(Ok(())) => tracing::info!("Vault: machine access given under lease {lease_id}"),
                Ok(Err(err)) => tracing::warn!("Vault: machine access under lease {lease_id} was not given: {err}"),
                Err(_) => tracing::error!("Vault: machine access under lease {lease_id} failed"),
            }
        });
    if let Err(err) = spawned {
        return Err(fail(format!("The grant could not wait for its approval: {err}")));
    }
    Ok(out([
        ("leaseId", Value::string(&lease.lease_id)),
        ("lease", value(&lease_json(&lease))),
    ]))
}

/// `list#VaultMachineAccess`: a vault's machine access to its members, or an environment's to a holder of
/// `VAULT_ADMIN`: which items, for which environment, given by whom and when; never a sealed key.
fn list_vault_machine_access(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let vault_id = text(params, "vaultId");
    let environment_id = text(params, "environmentId");
    let mut found = find(facade, MACHINE_ACCESS)?;
    if !vault_id.is_empty() {
        vault_for(facade, &vault_id, &me, Access::Read, false)?;
        found = found.eq("vaultId", Value::string(&vault_id));
    } else if !has_permission("VAULT_ADMIN")? {
        return Err(fail("Name a vault you are a member of"));
    }
    if !environment_id.is_empty() {
        found = found.eq("environmentId", Value::string(&environment_id));
    }
    let rows = found.list()?;
    let mut listed = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        let mut map = ValueMap::default();
        for name in [
            "environmentId",
            "vaultItemId",
            "vaultId",
            "grantedByUserId",
            "grantedDate",
            "staleDate",
        ] {
            if let Some(given) = row.get(name) {
                map.insert(name.to_string(), given.clone());
            }
        }
        if let Some(item) = find(facade, crate::common::ITEM)?
            .eq("vaultItemId", Value::string(field(row, "vaultItemId")))
            .one()?
        {
            map.insert("bindingKey".to_string(), Value::string(field(&item, "bindingKey")));
            map.insert("itemName".to_string(), Value::string(field(&item, "itemName")));
        }
        listed.push(Value::Map(map));
    }
    Ok(out([("accessList", Value::List(listed))]))
}

/// `revoke#VaultMachineAccess`: a holder of `VAULT_ADMIN` deletes an environment's sealed keys of a vault (or of one
/// item), with an event each. The items to rotate at their source, since the server could have kept them.
fn revoke_vault_machine_access(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    user()?;
    if !has_permission("VAULT_ADMIN")? {
        return Err(fail("Revoking machine access needs the VAULT_ADMIN permission"));
    }
    let environment_id = required(params, "environmentId")?;
    let vault_id = required(params, "vaultId")?;
    vault_row(facade, &vault_id, false)?;
    let item_id = text(params, "vaultItemId");
    let mut found = find(facade, MACHINE_ACCESS)?
        .eq("environmentId", Value::string(&environment_id))
        .eq("vaultId", Value::string(&vault_id));
    if !item_id.is_empty() {
        found = found.eq("vaultItemId", Value::string(&item_id));
    }
    let rows = found.list()?;
    let mut rotate = Vec::with_capacity(rows.len());
    for row in rows.iter() {
        let revoked = field(row, "vaultItemId");
        row.delete(facade)?;
        events::write(
            facade,
            Event {
                kind: "VeMachineRevoked",
                vault_id: Some(&vault_id),
                item_id: Some(&revoked),
                detail: Some(format!("environment {environment_id}")),
                ..Event::default()
            },
        )?;
        rotate.push(Value::string(revoked));
    }
    // the machine leases of the environment open on the vault end too: the server forgets their keys
    let open = find(facade, crate::common::LEASE)?
        .eq("vaultId", Value::string(&vault_id))
        .eq("environmentId", Value::string(&environment_id))
        .eq("machineAccess", Value::string("Y"))
        .eq("statusId", Value::string(crate::lease::OPEN))
        .list()?;
    let registry = registry(&*factory()?);
    for lease in open.iter() {
        let lease_id = field(lease, "leaseId");
        crate::lease::mark_ended(
            facade,
            &lease_id,
            crate::lease::REVOKED,
            None,
            "its machine access was revoked",
        )?;
        registry.forget(&lease_id);
        crate::lease::tell_member(facade, &lease_id, &field(lease, "clusterMemberId"), "revoked");
    }
    Ok(out([
        ("revokedCount", Value::I64(rotate.len() as i64)),
        ("rotateItemIds", Value::List(rotate)),
    ]))
}
