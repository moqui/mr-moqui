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

//! The manager's transit and its view of "unsealed" (plans/Vault-Instances.md §4.4, §4.5, M6); the link that carries
//! them is the host's (`moqui_host::manage_link`).
//!
//! - **The transit key.** One X25519 key per instance, in this manager's keyring (a `SecretValue`, Sealed-Secrets S4:
//!   an unattended unseal needs it, so it is a platform secret), made at the instance's first link. The instance wraps
//!   its root key to the public half (`moqui_vault::managed::wrap_root`), so the manager sees no root key at init.
//! - **An unseal** ([`unseal`]): the instance sends the wrapped root key with an ephemeral key; while its link key is
//!   pinned (a revoke unpins it) and the environment's policy allows, the manager opens it with that instance's transit
//!   key and seals it to the ephemeral key. Under the approval policy each unseal waits for a member of the approver
//!   group ([`approve`]). Each one is an `InstanceAction` (unseal) and a Monitor unseal, told apart when its address is
//!   new (§4.8). As every transit seal does, the manager sees the root key during the unwrap (§4.9).
//! - **"Unsealed".** The manager takes an instance's claim of unsealed on a link only when it unsealed it there, or
//!   the instance proves it ([`check_proof`]) with the Ed25519 key derived from its root key
//!   (`moqui_core::seal::unsealed_proof_seed`), whose public half the manager pinned: derived from the root key at a
//!   transit unseal (`unsealProofTrust` transit), or, for an environment whose method the manager does not unseal (tpm,
//!   passphrase, shamir), taken from the instance at its first unsealed link (first-use: what that link said is not
//!   checked by anything the manager holds, so a holder of a new instance's link key before its first unseal could pin
//!   a key of its own; the instance's later proofs then fail, which shows).

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::hpke::{PublicKey, SecretKey};
use moqui_vault::managed::{self, ProofLink};
use moqui_vault::rng::OsRng;
use moqui_vault::sign::{SigningKey, VerifyingKey};
use moqui_vault::text::{hex, unhex};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::actions::{DONE, RUNNING, WAITING};
use crate::common::{authz_off, fail, field, find, now, one, ACTION, ENVIRONMENT};
use crate::link::instance;

/// The name of an instance's transit key among the manager's secrets.
pub const SECRET_PREFIX: &str = "instance-transit/";
/// The action of an unseal.
pub const UNSEAL_ACTION: &str = "IatUnseal";
/// The notification topic of an unseal that waits for an approval.
pub const APPROVAL_TOPIC: &str = "InstanceUnsealApproval";
/// What `unsealProofTrust` says of a key derived from the root key the manager unwrapped.
pub const TRUST_TRANSIT: &str = "transit";
/// What it says of a key the instance gave on its first unsealed link.
pub const TRUST_FIRST_USE: &str = "first-use";

/// The unseal method an instance's environment names: `transit` (the manager's, the default), `tpm`, `passphrase` or
/// `shamir`.
pub fn environment_method(facade: &EntityFacade, row: &EntityValue) -> Result<String, LibError> {
    let _authz = authz_off();
    let method = one(facade, ENVIRONMENT, "environmentId", &field(row, "environmentId"))?
        .map(|environment| field(&environment, "unsealMethod"))
        .unwrap_or_default();
    Ok(if method.is_empty() {
        "transit".to_string()
    } else {
        method
    })
}

#[cfg(not(test))]
fn stored_key(name: &str) -> Result<Option<Zeroizing<String>>, LibError> {
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| fail("the transit key is read in an execution context"))?;
    moqui_core::seal::secrets::get(&factory, name).map_err(|err| fail(err.message().to_string()))
}

#[cfg(not(test))]
fn store_key(name: &str, value: &str) -> Result<(), LibError> {
    moqui_core::seal::secrets::put_named(name, value).map_err(|err| fail(err.message().to_string()))
}

// a test's factory has no seal to keep a secret under: its transit keys are this process's
#[cfg(test)]
static TEST_KEYS: std::sync::Mutex<std::collections::BTreeMap<String, String>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

#[cfg(test)]
fn stored_key(name: &str) -> Result<Option<Zeroizing<String>>, LibError> {
    Ok(moqui_lib::sync::lock(&TEST_KEYS).get(name).cloned().map(Zeroizing::new))
}

#[cfg(test)]
fn store_key(name: &str, value: &str) -> Result<(), LibError> {
    moqui_lib::sync::lock(&TEST_KEYS).insert(name.to_string(), value.to_string());
    Ok(())
}

/// `instance_id`'s transit key, made and kept in the manager's keyring at first use. A key that cannot be read is an
/// error, never a new key: a new one would leave the instance's seal unopenable by transit.
pub fn transit_key(instance_id: &str) -> Result<SecretKey, LibError> {
    let name = format!("{SECRET_PREFIX}{instance_id}");
    if let Some(kept) = stored_key(&name)? {
        let bytes = Zeroizing::new(unhex(kept.trim()).map_err(|_| fail(format!("the transit key {name} is not hex")))?);
        let key = moqui_vault::keys::Key32::from_slice(&bytes)
            .map_err(|_| fail(format!("the transit key {name} is not 32 bytes")))?;
        return Ok(SecretKey::from_key(key));
    }
    let key = SecretKey::random(&mut OsRng);
    let text = Zeroizing::new(hex(key.as_key().as_bytes()));
    store_key(&name, &text)?;
    tracing::info!("Instances: made the transit key of AppInstance {instance_id}");
    Ok(key)
}

/// The public half of `instance_id`'s transit key, which its root key is wrapped to.
pub fn transit_public(instance_id: &str) -> Result<[u8; 32], LibError> {
    let public = transit_key(instance_id)?
        .public()
        .map_err(|err| fail(format!("the transit key of AppInstance {instance_id}: {err}")))?;
    Ok(*public.as_bytes())
}

/// What an instance asks at an unseal.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    pub instance_id: &'a str,
    /// The link key its connection presented.
    pub link_fingerprint: &'a str,
    /// The address its link comes from.
    pub remote: &'a str,
    pub generation: u64,
    pub wrapped_root: &'a [u8],
    pub ephemeral_public: &'a [u8],
}

/// The manager's answer.
#[derive(Debug)]
pub enum Answer {
    /// The root key sealed to the ephemeral key.
    Sealed {
        sealed_root: Vec<u8>,
        /// The instance unsealed from an address it never unsealed from before (§4.8).
        new_address: bool,
    },
    /// The environment's approval policy: why it waits.
    Waiting(String),
}

/// An unseal by transit (see the module doc). Refused: an instance whose link key is not the one pinned (never paired,
/// or revoked), a wrap that does not open with its transit key, and an ephemeral key that is not one.
pub fn unseal(facade: &EntityFacade, ask: &Ask<'_>) -> Result<Answer, LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, ask.instance_id)?;
    let pinned = field(&row, "linkKeyFingerprint");
    if pinned.is_empty() || !bool::from(pinned.as_bytes().ct_eq(ask.link_fingerprint.as_bytes())) {
        tracing::warn!(
            "Instances: an unseal of AppInstance {} by a link key that is not pinned (revoked, or never paired) was \
             refused",
            ask.instance_id
        );
        return Err(fail(
            "this instance's link key is not pinned on the manager, or was revoked: transit refuses it",
        ));
    }
    let environment = one(facade, ENVIRONMENT, "environmentId", &field(&row, "environmentId"))?
        .ok_or_else(|| fail(format!("AppInstance {} names no InstanceEnvironment", ask.instance_id)))?;
    let approved = if field(&environment, "unsealPolicyEnumId") == "IupApproval" {
        match approved_action(facade, ask.instance_id)? {
            Some(action) => Some(action),
            None => return Ok(Answer::Waiting(ask_approval(facade, &environment, ask)?)),
        }
    } else {
        None
    };
    let ephemeral = PublicKey::from_bytes(ask.ephemeral_public).map_err(|_| fail("the ephemeral key is not one"))?;
    let transit = transit_key(ask.instance_id)?;
    let root = managed::open_root(&transit, ask.instance_id, ask.wrapped_root).map_err(|_| {
        fail("the wrapped root key does not open with this instance's transit key (another instance's, or changed)")
    })?;
    let sealed_root = managed::seal_reply(&ephemeral, ask.instance_id, &root, &mut OsRng)
        .map_err(|err| fail(format!("the answer was not sealed: {err}")))?;
    // the unsealed proof's key, from the root key this manager just saw: the instance's proofs on its links are
    // checked against it (§4.4)
    let proof_public = {
        let root =
            moqui_core::seal::RootKey::from_slice(root.as_slice()).map_err(|err| fail(err.message().to_string()))?;
        let seed = moqui_core::seal::unsealed_proof_seed(&root);
        *SigningKey::from_seed(&seed).verifying_key().as_bytes()
    };
    let was = field(&row, "unsealProofKey");
    if !was.is_empty() && was != hex(&proof_public) {
        tracing::warn!(
            "Instances: AppInstance {} has a new root key since its last unseal (a rotate-root): its unsealed proof key \
             is pinned again",
            ask.instance_id
        );
    }
    row.set("unsealProofKey", Value::string(hex(&proof_public)));
    row.set("unsealProofTrust", Value::string(TRUST_TRANSIT));
    let known = field(&row, "unsealAddresses");
    let new_address =
        !known.is_empty() && !ask.remote.is_empty() && !known.split(',').any(|seen| seen.trim() == ask.remote);
    row.update(facade)?;
    // the Monitor event of an unseal, and its new-address alert (§4.8)
    crate::link::note_state(facade, ask.instance_id, "linked", Some("unsealed"), ask.remote);
    let mut report = format!(
        "unsealed by transit on link generation {} from {}",
        ask.generation, ask.remote
    );
    if new_address {
        report.push_str(", an address it never unsealed from before");
    }
    match approved {
        Some(action) => {
            let approver = field(&action, "userId");
            report.push_str(&format!(", approved by {approver}"));
            crate::actions::finish(facade, &field(&action, "instanceActionId"), DONE, &report)?;
        }
        None => {
            record(facade, ask.instance_id, DONE, &report)?;
        }
    }
    tracing::info!("Instances: AppInstance {} {report}", ask.instance_id);
    Ok(Answer::Sealed {
        sealed_root,
        new_address,
    })
}

/// An unseal action row; its id.
fn record(facade: &EntityFacade, instance_id: &str, status: &str, report: &str) -> Result<String, LibError> {
    let mut row = facade.make_value(ACTION)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("appInstanceId", Value::string(instance_id));
    row.set("actionEnumId", Value::string(UNSEAL_ACTION));
    row.set("statusEnumId", Value::string(status));
    row.set("machineAccess", Value::string("N"));
    row.set("report", Value::string(report));
    row.set("startDate", now());
    if status != WAITING && status != RUNNING {
        row.set("endDate", now());
    }
    let id = field(&row, "instanceActionId");
    row.create(facade)?;
    Ok(id)
}

/// The unseal actions of `instance_id` in `status`, oldest first.
fn unseal_actions(facade: &EntityFacade, instance_id: &str, status: &str) -> Result<Vec<EntityValue>, LibError> {
    Ok(find(facade, ACTION)?
        .eq("appInstanceId", Value::string(instance_id))
        .eq("actionEnumId", Value::string(UNSEAL_ACTION))
        .eq("statusEnumId", Value::string(status))
        .order_by("startDate", true)
        .list()?
        .into_values())
}

/// An approved unseal of `instance_id` not used yet (running: a member approved it, and the next unseal takes it).
fn approved_action(facade: &EntityFacade, instance_id: &str) -> Result<Option<EntityValue>, LibError> {
    Ok(unseal_actions(facade, instance_id, RUNNING)?.into_iter().next())
}

/// Under the approval policy: the unseal waits; a waiting action is written once and the approver group told.
fn ask_approval(facade: &EntityFacade, environment: &EntityValue, ask: &Ask<'_>) -> Result<String, LibError> {
    let group = field(environment, "approverUserGroupId");
    let waiting = unseal_actions(facade, ask.instance_id, WAITING)?;
    let action_id = match waiting.first() {
        Some(action) => field(action, "instanceActionId"),
        None => {
            let id = record(
                facade,
                ask.instance_id,
                WAITING,
                &format!(
                    "waiting for a member's approval: an unseal asked on link generation {} from {}",
                    ask.generation, ask.remote
                ),
            )?;
            tell_approvers(ask.instance_id, &group, &id, ask.remote);
            id
        }
    };
    Ok(format!(
        "waiting for {} to approve the unseal (InstanceAction {action_id}; the environment's approval policy)",
        if group.is_empty() {
            "an instance administrator".to_string()
        } else {
            format!("a member of {group}")
        }
    ))
}

fn tell_approvers(instance_id: &str, group: &str, action_id: &str, remote: &str) {
    let mut message = moqui_lib::object::ValueMap::default();
    for (name, value) in [
        ("appInstanceId", instance_id),
        ("instanceActionId", action_id),
        ("remoteAddress", remote),
    ] {
        message.insert(name.to_string(), Value::string(value));
    }
    let clean = |text: &str| text.replace(['$', '{', '}'], "");
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(APPROVAL_TOPIC)
        .user_group_id(if group.is_empty() { "ADMIN" } else { group })
        .notification_type("warning")
        .message_map(message)
        .title(format!(
            "Instance {}: an unseal waits for your approval",
            clean(instance_id)
        ))
        .send();
    if let Err(err) = sent {
        tracing::error!(
            "Instances: the approvers of an unseal of AppInstance {instance_id} were not told: {}",
            err.message()
        );
    }
}

/// A member approves a waiting unseal (`approve#InstanceUnseal`): the caller is checked under its own authorization
/// first (INSTANCE_ADMIN and the environment's approver group), then the action runs, and the instance's next unseal
/// takes it.
pub fn approve(facade: &EntityFacade, action_id: &str) -> Result<String, LibError> {
    let action = one(facade, ACTION, "instanceActionId", action_id)?
        .filter(|action| field(action, "actionEnumId") == UNSEAL_ACTION)
        .ok_or_else(|| fail(format!("No unseal InstanceAction {action_id}")))?;
    let instance_id = field(&action, "appInstanceId");
    crate::link::check_caller(facade, &instance(facade, &instance_id)?)?;
    if field(&action, "statusEnumId") != WAITING {
        return Err(fail(format!(
            "InstanceAction {action_id} is not waiting for an approval"
        )));
    }
    let user = moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .ok_or_else(|| fail("an approval is a signed-in member's"))?;
    let _authz = authz_off();
    let mut action = action;
    action.set("statusEnumId", Value::string(RUNNING));
    action.set("userId", Value::string(&user));
    action.set(
        "report",
        Value::string(format!("{}; approved by {user}", field(&action, "report"))),
    );
    action.update(facade)?;
    tracing::warn!("Instances: {user} approved an unseal of AppInstance {instance_id} (InstanceAction {action_id})");
    Ok(instance_id)
}

/// Whether the manager takes `link`'s claim of unsealed from a proof (see the module doc): signed by the key pinned
/// for the instance; or, with none pinned and an environment whose method this manager does not unseal, by the key
/// given, which is pinned then (first-use).
pub fn check_proof(
    facade: &EntityFacade,
    link: &ProofLink<'_>,
    proof_public: &[u8],
    signature: &[u8],
) -> Result<bool, LibError> {
    let _authz = authz_off();
    if proof_public.is_empty() || signature.is_empty() {
        return Ok(false);
    }
    let Ok(key) = VerifyingKey::from_bytes(proof_public) else {
        return Ok(false);
    };
    let mut row = instance(facade, link.instance_id)?;
    if field(&row, "linkKeyFingerprint") != link.link_fingerprint {
        return Ok(false);
    }
    let pinned = field(&row, "unsealProofKey");
    if !pinned.is_empty() {
        if !bool::from(pinned.as_bytes().ct_eq(hex(key.as_bytes()).as_bytes())) {
            tracing::warn!(
                "Instances: AppInstance {} proved unsealed with another key than the one pinned (a new root key, or a \
                 claim without its root key): taken as sealed until a transit unseal or an operator pins it again",
                link.instance_id
            );
            return Ok(false);
        }
        return Ok(managed::verify_proof(&key, link, signature).is_ok());
    }
    if environment_method(facade, &row)? == "transit" {
        // the manager unseals these: the key it pins is the one it derives from the root key at a transit unseal
        return Ok(false);
    }
    if managed::verify_proof(&key, link, signature).is_err() {
        return Ok(false);
    }
    row.set("unsealProofKey", Value::string(hex(key.as_bytes())));
    row.set("unsealProofTrust", Value::string(TRUST_FIRST_USE));
    row.update(facade)?;
    tracing::warn!(
        "Instances: pinned the unsealed proof key of AppInstance {} on its first unsealed link (first-use: its \
         environment's method is not the manager's transit)",
        link.instance_id
    );
    Ok(true)
}

/// Forget the unsealed proof key pinned for `instance_id`: an operator confirms the instance's new root key (a
/// rotate-root under a method the manager does not unseal), which its next proof pins again. INSTANCE_ADMIN and the
/// environment's group.
pub fn forget_proof_key(facade: &EntityFacade, instance_id: &str) -> Result<(), LibError> {
    let mut row = instance(facade, instance_id)?;
    crate::link::check_caller(facade, &row)?;
    let _authz = authz_off();
    row.set("unsealProofKey", Value::Null);
    row.set("unsealProofTrust", Value::Null);
    row.update(facade)?;
    tracing::warn!("Instances: the unsealed proof key of AppInstance {instance_id} was forgotten by an operator");
    Ok(())
}
