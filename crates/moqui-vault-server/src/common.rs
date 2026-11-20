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

//! What the vault services share: the parameters and fields they read, the signed-in user, the records' JSON (the
//! vault crate's shapes, `moqui_vault::ops::shapes`), a vault and its caller's membership.

use std::collections::BTreeMap;

use moqui_core::context::{AuthzGuard, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityFind, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};
use serde_json::Value as Json;

pub(crate) type Params = BTreeMap<String, Value>;
pub(crate) type Out = Result<BTreeMap<String, Value>, LibError>;

pub(crate) const VAULT: &str = "moqui.security.vault.Vault";
pub(crate) const KEY_VERSION: &str = "moqui.security.vault.VaultKeyVersion";
pub(crate) const MEMBER: &str = "moqui.security.vault.VaultMember";
pub(crate) const ITEM: &str = "moqui.security.vault.VaultItem";
pub(crate) const ITEM_HISTORY: &str = "moqui.security.vault.VaultItemHistory";
pub(crate) const ITEM_PENDING: &str = "moqui.security.vault.VaultItemPending";
pub(crate) const PERSONAL_KEY: &str = "moqui.security.vault.VaultPersonalKey";
pub(crate) const KEY_CHECK: &str = "moqui.security.vault.VaultKeyCheck";
pub(crate) const DELIVERY: &str = "moqui.security.vault.VaultDelivery";
pub(crate) const EVENT: &str = "moqui.security.vault.VaultEvent";
pub(crate) const RECOVERY_KEY: &str = "moqui.security.vault.VaultRecoveryKey";
pub(crate) const RECOVERY_HOLDER: &str = "moqui.security.vault.VaultRecoveryHolder";
pub(crate) const RECOVERY_SESSION: &str = "moqui.security.vault.VaultRecoverySession";
pub(crate) const RECOVERY_DELIVERY: &str = "moqui.security.vault.VaultRecoveryDelivery";
pub(crate) const LEASE: &str = "moqui.security.vault.VaultLease";
pub(crate) const MACHINE_ACCESS: &str = "moqui.security.vault.VaultMachineAccess";
pub(crate) const DEVICE_MEMORY: &str = "moqui.security.vault.VaultDeviceMemory";

pub(crate) const PERSONAL: &str = "VkPersonal";
pub(crate) const OPERATIONAL: &str = "VkOperational";
pub(crate) const SINGLE_SECRET: &str = "VlSingleSecret";
pub(crate) const SHARED_KEY: &str = "VlSharedKey";
pub(crate) const ACTIVE: &str = "VpkActive";

pub(crate) fn fail(text: impl Into<String>) -> LibError {
    LibError::new(text)
}

/// A refusal of the vault crate, in its own words (which never carry key material).
pub(crate) fn refused(err: moqui_vault::Error) -> LibError {
    fail(format!("Vault: {err}"))
}

/// Authorization off while a service reads and writes the vault tables: each service checks its caller itself.
pub(crate) fn authz_off() -> Option<AuthzGuard> {
    ExecutionContext::with_current(ExecutionContext::disable_authz)
}

/// The signed-in user's id.
pub(crate) fn user() -> Result<String, LibError> {
    let user_id = moqui_core::user::current_user_id()
        .map_err(|err| fail(err.message()))?
        .filter(|id| !id.is_empty())
        .ok_or_else(|| fail("Sign in first"))?;
    // `recovery:` names a recovery key in a member list (§3.10): no account under such an id acts in a vault
    if moqui_vault::keyversion::recovery_key_of(&user_id).is_some() {
        return Err(fail(
            "A user id that starts with recovery: is reserved for recovery keys",
        ));
    }
    Ok(user_id)
}

pub(crate) fn find(facade: &EntityFacade, entity: &str) -> Result<EntityFind, LibError> {
    Ok(facade.find(entity)?.disable_authz())
}

pub(crate) fn text(params: &Params, name: &str) -> String {
    params
        .get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

pub(crate) fn required(params: &Params, name: &str) -> Result<String, LibError> {
    let value = text(params, name);
    if value.is_empty() {
        return Err(fail(format!("{name} is required")));
    }
    Ok(value)
}

pub(crate) fn int(params: &Params, name: &str) -> Option<i64> {
    text(params, name).trim().parse().ok()
}

/// A yes-or-no parameter: Y, yes, true or 1 (in any case) is yes, anything else given is no; none when not given.
pub(crate) fn flag(params: &Params, name: &str) -> Option<bool> {
    match text(params, name).trim().to_ascii_lowercase().as_str() {
        "" => None,
        "y" | "yes" | "true" | "1" => Some(true),
        _ => Some(false),
    }
}

/// The longest wrapped key (an item key, a member's wrap, a recovery code's wrap), in hex characters.
pub(crate) const MAX_WRAP_HEX: usize = 1024;
/// The longest small sealed or JSON value (a vault's header, a password wrap), in characters: what text-long holds.
pub(crate) const MAX_SMALL_TEXT: usize = 4000;

/// A hex value of at most `max` characters.
pub(crate) fn check_hex(name: &str, value: &str, max: usize) -> Result<(), LibError> {
    if value.len() > max {
        return Err(fail(format!("{name} is too long")));
    }
    moqui_vault::text::unhex(value).map_err(refused)?;
    Ok(())
}

/// The refusal of a client-made id that is taken, or that a deleted record had: the same words whatever the reason,
/// so the id tells nothing more.
pub(crate) fn id_not_available(name: &str, id: &str) -> LibError {
    fail(format!("{name} {id} is not available: make another id"))
}

/// Whether events name `id` in `field`: a record that once had the id, whose events stay after it is deleted, so the
/// id is never given to another (whose readers would read the old one's events).
pub(crate) fn id_has_history(facade: &EntityFacade, field_name: &str, id: &str) -> Result<bool, LibError> {
    Ok(find(facade, EVENT)?
        .eq(field_name, Value::string(id))
        .limit(1)
        .one()?
        .is_some())
}

/// The next vault revision: every change of its member list, key or items moves it, so a change made from a read of
/// the vault (a removal's rotation) is refused once anything changed since (decision (a) of §9).
pub(crate) fn bump_revision(facade: &EntityFacade, vault: &mut EntityValue) -> Result<i64, LibError> {
    let next = int_field(vault, "revision") + 1;
    vault.set("revision", Value::I64(next));
    vault.update(facade)?;
    Ok(next)
}

/// The rule of `require#RecentAuth` for a deletion: the session's user signed in, or confirmed who they are, in the
/// last five minutes ([`moqui_core::user::recent_auth`]: a password, a second factor's code, SSO; never a key or an
/// app password). Asked directly, not through the service, which passes any caller with authorization off, as every
/// vault service runs its own reads and writes.
pub(crate) fn require_recent_auth() -> Result<(), LibError> {
    require_recent_auth_to("delete this")
}

/// [`require_recent_auth`] for another change, which the refusal names.
pub(crate) fn require_recent_auth_to(what: &str) -> Result<(), LibError> {
    if moqui_core::user::recent_auth(5 * 60 * 1000) {
        return Ok(());
    }
    Err(fail(format!(
        "Confirm it is you: sign in again to {what} (reasonCode reauthRequired)"
    )))
}

/// Whether the signed-in user holds `permission`.
pub(crate) fn has_permission(permission: &str) -> Result<bool, LibError> {
    moqui_core::user::has_permission(permission).map_err(|err| fail(err.message()))
}

/// A secret of the server by its conf selector, through the entered factory's secret handle (since Sealed-Secrets S4:
/// the conf files and the environment, else the seal's store, where a generated one is made at its first read); a
/// test's factory, or code with no context entered, reads the setting (a process property of its environment name).
/// None when it is not set, or still at `CHANGEME`.
pub(crate) fn server_secret(selector: &str) -> Result<Option<zeroize::Zeroizing<String>>, LibError> {
    let unset = |value: &str| value.trim().is_empty() || value.ends_with("CHANGEME");
    let handle = ExecutionContext::with_current(|ec| ec.factory().secret(selector));
    if let Some(found) = handle {
        match found {
            Ok(Some(value)) if !unset(&value) => return Ok(Some(value)),
            Ok(_) => {}
            Err(err) => {
                return Err(fail(format!(
                    "The secret {selector} could not be read: {}",
                    err.message()
                )))
            }
        }
    }
    let value = zeroize::Zeroizing::new(moqui_core::context::conf_env::setting(selector));
    Ok((!unset(&value)).then_some(value))
}

/// The epoch millis now, on the framework's clock.
pub(crate) fn now_millis() -> i64 {
    moqui_core::user::now_millis()
}

/// A map or list parameter as JSON: as the request gave it, or JSON text.
pub(crate) fn json(params: &Params, name: &str) -> Result<Json, LibError> {
    match params.get(name) {
        None | Some(Value::Null) => Err(fail(format!("{name} is required"))),
        Some(Value::String(text)) => serde_json::from_str(text).map_err(|_| fail(format!("{name} is not JSON"))),
        Some(value) => Ok(moqui_lib::json::to_json(value)),
    }
}

pub(crate) fn field(row: &EntityValue, name: &str) -> String {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .unwrap_or_default()
}

pub(crate) fn int_field(row: &EntityValue, name: &str) -> i64 {
    field(row, name).trim().parse().unwrap_or(0)
}

/// A JSON record as a service result.
pub(crate) fn value(json: &Json) -> Value {
    moqui_lib::json::from_json(json)
}

pub(crate) fn out<const N: usize>(pairs: [(&str, Value); N]) -> BTreeMap<String, Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

/// A client-made id: the vault crate binds it into what it seals and signs, so the client names it before the row
/// exists. Letters, digits, `_`, `-` and `.`, 8 to 40 characters.
pub(crate) fn check_id(name: &str, id: &str) -> Result<(), LibError> {
    let plain = id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c));
    if !plain || !(8..=40).contains(&id.len()) {
        return Err(fail(format!("{name} must be 8 to 40 letters, digits, '_', '-' or '.'")));
    }
    Ok(())
}

pub(crate) fn now() -> Value {
    moqui_core::user::now_value()
}

/// A vault row, locked for the change when `lock`.
pub(crate) fn vault_row(facade: &EntityFacade, vault_id: &str, lock: bool) -> Result<EntityValue, LibError> {
    find(facade, VAULT)?
        .eq("vaultId", Value::string(vault_id))
        .for_update(lock)
        .one()?
        .ok_or_else(|| fail(format!("No vault {vault_id}")))
}

/// The caller's member row of a vault's current key version: their role there. None for a non-member.
pub(crate) fn membership(
    facade: &EntityFacade,
    vault: &EntityValue,
    user_id: &str,
) -> Result<Option<EntityValue>, LibError> {
    // a recovery key is listed as a member, but is no one's membership
    if moqui_vault::keyversion::recovery_key_of(user_id).is_some() {
        return Ok(None);
    }
    find(facade, MEMBER)?
        .eq("vaultId", Value::string(field(vault, "vaultId")))
        .eq("userId", Value::string(user_id))
        .eq("keyVersion", Value::I64(int_field(vault, "keyVersion")))
        .one()
}

/// What a caller may do in a vault, by its lock and their role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    /// Read the vault and its items: a member, or a single-secret vault's owner.
    Read,
    /// Write items: manage or write in a personal vault, manage in an operational one; a single-secret vault's owner.
    Write,
    /// Change the member list and delete the vault: manage; a single-secret vault's owner deletes it.
    Manage,
    /// Approve a lease (§3.7): the approve role of an operational vault.
    Approve,
    /// See one value in the member's client (§3.7, Reveal): the reveal role of an operational vault.
    Reveal,
}

/// The vault, after the caller's access to it is checked. A vault the caller may not read is "not found" to them, as
/// one that does not exist, so its id tells them nothing.
pub(crate) fn vault_for(
    facade: &EntityFacade,
    vault_id: &str,
    user_id: &str,
    access: Access,
    lock: bool,
) -> Result<EntityValue, LibError> {
    let not_found = || fail(format!("No vault {vault_id}"));
    let vault = vault_row(facade, vault_id, lock).map_err(|_| not_found())?;
    if field(&vault, "lockEnumId") == SINGLE_SECRET {
        if field(&vault, "ownerUserId") != user_id {
            return Err(not_found());
        }
        return Ok(vault);
    }
    let Some(member) = membership(facade, &vault, user_id)? else {
        return Err(not_found());
    };
    let role = field(&member, "roleEnumId");
    let operational = field(&vault, "vaultKindEnumId") == OPERATIONAL;
    let allowed = match access {
        Access::Read => true,
        Access::Write => role == "VrManage" || (!operational && role == "VrWrite"),
        Access::Manage => role == "VrManage",
        Access::Approve => operational && role == "VrApprove",
        Access::Reveal => operational && role == "VrReveal",
    };
    if !allowed {
        return Err(fail(format!("Your role in vault {vault_id} does not allow this")));
    }
    Ok(vault)
}

/// A role's enum id from the chain's name of it.
pub(crate) fn role_enum(role: moqui_vault::keyversion::Role) -> &'static str {
    use moqui_vault::keyversion::Role;
    match role {
        Role::Manage => "VrManage",
        Role::Write => "VrWrite",
        Role::Read => "VrRead",
        Role::Approve => "VrApprove",
        Role::Reveal => "VrReveal",
    }
}

/// Whether a role belongs to a vault of this kind: manage, write and read for a personal vault; manage, approve and
/// reveal for an operational one (§3.9).
pub(crate) fn role_fits(role: moqui_vault::keyversion::Role, operational: bool) -> bool {
    use moqui_vault::keyversion::Role;
    match role {
        Role::Manage => true,
        Role::Write | Role::Read => !operational,
        Role::Approve | Role::Reveal => operational,
    }
}
