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

//! The manager's transit and its view of "unsealed" (plans/archive/Vault-Instances.md §4.4, §4.5, M6); the link that carries
//! them is the host's (`moqui_host::manage_link`).
//!
//! - **The transit key.** One X25519 key per instance whose environment's method is the manager's transit, in this
//!   manager's keyring (a `SecretValue` row, Sealed-Secrets S4: an unattended unseal needs it, so it is a platform
//!   secret). It is made once, when no row holds it ([`transit_public`], at the instance's link), and only read after:
//!   a row that does not open, or a manager with no seal of its own, is an error, never a new key, which would leave
//!   the instance's seal unopenable by transit. The instance wraps its root key to the public half
//!   (`moqui_vault::managed::wrap_root`), so the manager sees no root key at init.
//! - **An unseal** ([`unseal`]): the instance sends the wrapped root key with an ephemeral key; while its link key is
//!   pinned (a revoke unpins it), its environment's method is transit and the policy allows, the manager opens it with
//!   that instance's transit key and seals it to the ephemeral key. Anyone with the transit key's public half can wrap
//!   a key of their own (HPKE base mode names no sender), so the root key that opens must be the instance's: its proof
//!   key, derived from it, must be the one pinned. The first one is pinned only at the instance's first start (within
//!   the pairing window: a new instance unseals then) or with a member's approval; after it, a root key whose proof key
//!   is another is refused and told to the administrators, never pinned in its place. A rotate-root's new key is pinned
//!   only signed by the old one ([`rotate_proof`]), or after an operator forgot the old ([`forget_proof_key`]). Under
//!   the approval policy each unseal waits for a member of the approver group ([`approve`]), the approval bound to the
//!   link generation and address that asked, for 15 minutes. Each unseal is an `InstanceAction` and a Monitor unseal,
//!   told apart when its address is new (§4.8). The manager sees the root key during the unwrap (§4.9).
//! - **"Unsealed".** The manager takes an instance's claim of unsealed on a link only when it unsealed it there, or
//!   the instance proves it ([`check_proof`]) with the Ed25519 key derived from its root key
//!   (`moqui_core::seal::unsealed_proof_seed`), whose public half the manager pinned: derived from the root key at a
//!   transit unseal (`unsealProofTrust` transit), or, for an environment whose method the manager does not unseal (tpm,
//!   passphrase, shamir), the key the instance gives at its first unsealed link, held (pending) until an operator
//!   confirms it ([`confirm_proof_key`], first-use).

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

use crate::actions::{DONE, REFUSED, RUNNING, WAITING};
use crate::common::{authz_off, fail, field, find, now, one, ACTION, ENVIRONMENT};
use crate::link::instance;

/// The name of an instance's transit key among the manager's secrets.
pub const SECRET_PREFIX: &str = "instance-transit/";
/// The action of an unseal.
pub const UNSEAL_ACTION: &str = "IatUnseal";
/// The notification topic of an unseal that waits for an approval.
pub const APPROVAL_TOPIC: &str = "InstanceUnsealApproval";
/// The notification topic of an unseal refused for a root key that is not the instance's, and of a proof key that
/// waits for an operator's confirmation.
pub const REFUSED_TOPIC: &str = "InstanceUnsealRefused";
/// What `unsealProofTrust` says of a key derived from the root key the manager unwrapped.
pub const TRUST_TRANSIT: &str = "transit";
/// What it says of a key the instance gave on its first unsealed link, once an operator confirmed it.
pub const TRUST_FIRST_USE: &str = "first-use";
/// A key the instance gave on its first unsealed link, waiting for an operator's confirmation.
pub const TRUST_PENDING: &str = "pending";
/// How long an approval of an unseal stands.
const APPROVAL_MILLIS: i64 = 15 * 60_000;

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
fn keeper_and_facade() -> Result<(std::sync::Arc<moqui_core::seal::Keeper>, EntityFacade), LibError> {
    let factory = moqui_core::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory()))
        .ok_or_else(|| fail("the transit key is read in an execution context"))?;
    let keeper = factory
        .seal_keeper()
        .ok_or_else(|| fail("this manager has no seal of its own: it keeps no transit key"))?;
    let facade = factory
        .entity()
        .ok_or_else(|| fail("the manager's database is not open"))?;
    Ok((keeper, facade))
}

/// The transit key's row, read now; none when there is no row.
#[cfg(not(test))]
fn stored_key(name: &str) -> Result<Option<Zeroizing<String>>, LibError> {
    let (keeper, facade) = keeper_and_facade()?;
    moqui_core::seal::secrets::row_value(&keeper, &facade, name).map_err(|err| fail(err.message().to_string()))
}

/// The transit key's row, made by `make` when there is none (create-only: another member's wins).
#[cfg(not(test))]
fn stored_or_made(name: &str, make: impl FnOnce() -> Zeroizing<String>) -> Result<Zeroizing<String>, LibError> {
    let (keeper, facade) = keeper_and_facade()?;
    moqui_core::seal::secrets::row_or_create(&keeper, &facade, name, make)
        .map_err(|err| fail(err.message().to_string()))
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
fn stored_or_made(name: &str, make: impl FnOnce() -> Zeroizing<String>) -> Result<Zeroizing<String>, LibError> {
    let mut keys = moqui_lib::sync::lock(&TEST_KEYS);
    let kept = keys.entry(name.to_string()).or_insert_with(|| make().to_string());
    Ok(Zeroizing::new(kept.clone()))
}

fn key_of(name: &str, kept: &str) -> Result<SecretKey, LibError> {
    let bytes = Zeroizing::new(unhex(kept.trim()).map_err(|_| fail(format!("the transit key {name} is not hex")))?);
    let key = moqui_vault::keys::Key32::from_slice(&bytes)
        .map_err(|_| fail(format!("the transit key {name} is not 32 bytes")))?;
    Ok(SecretKey::from_key(key))
}

/// `instance_id`'s transit key, read from its row; refused when there is none (it is made only at the instance's
/// link, [`transit_public`]).
fn transit_key(instance_id: &str) -> Result<SecretKey, LibError> {
    let name = format!("{SECRET_PREFIX}{instance_id}");
    let kept = stored_key(&name)?.ok_or_else(|| fail(format!("AppInstance {instance_id} has no transit key")))?;
    key_of(&name, &kept)
}

/// The public half of `instance_id`'s transit key, which its root key is wrapped to: made when no row holds it, for an
/// instance whose environment's method is the manager's transit only.
pub fn transit_public(facade: &EntityFacade, instance_id: &str) -> Result<[u8; 32], LibError> {
    let row = instance(facade, instance_id)?;
    if environment_method(facade, &row)? != "transit" {
        return Err(fail(format!(
            "AppInstance {instance_id}'s environment does not unseal by the manager's transit"
        )));
    }
    let name = format!("{SECRET_PREFIX}{instance_id}");
    let mut made = false;
    let kept = stored_or_made(&name, || {
        made = true;
        Zeroizing::new(hex(SecretKey::random(&mut OsRng).as_key().as_bytes()))
    })?;
    if made {
        tracing::info!("Instances: made the transit key of AppInstance {instance_id}");
    }
    let public = key_of(&name, &kept)?
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
    /// The environment's approval policy, or a first unseal outside the instance's first start: why it waits.
    Waiting(String),
}

fn link_pinned(row: &EntityValue, link_fingerprint: &str) -> bool {
    let pinned = field(row, "linkKeyFingerprint");
    !pinned.is_empty() && bool::from(pinned.as_bytes().ct_eq(link_fingerprint.as_bytes()))
}

/// The proof key of a root key.
fn proof_public_of(root: &[u8; 32]) -> Result<[u8; 32], LibError> {
    let root = moqui_core::seal::RootKey::from_slice(root).map_err(|err| fail(err.message().to_string()))?;
    let seed = moqui_core::seal::unsealed_proof_seed(&root);
    Ok(*SigningKey::from_seed(&seed).verifying_key().as_bytes())
}

/// Whether `row`'s instance is at its first start: paired within the pairing window.
fn in_first_start(row: &EntityValue) -> bool {
    row.get("linkPairedDate")
        .and_then(moqui_core::user::value_millis)
        .is_some_and(|paired| moqui_core::user::now_millis() - paired <= crate::kinds::PAIRING_TOKEN_MINUTES * 60_000)
}

/// An unseal by transit (see the module doc). Refused: an instance whose link key is not the one pinned (never paired,
/// or revoked), an environment whose method is not the manager's transit, a wrap that does not open with its transit
/// key, a root key whose proof key is not the one pinned, and an ephemeral key that is not one.
pub fn unseal(facade: &EntityFacade, ask: &Ask<'_>) -> Result<Answer, LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, ask.instance_id)?;
    if !link_pinned(&row, ask.link_fingerprint) {
        tracing::warn!(
            "Instances: an unseal of AppInstance {} by a link key that is not pinned (revoked, or never paired) was \
             refused",
            ask.instance_id
        );
        return Err(fail(
            "this instance's link key is not pinned on the manager, or was revoked: transit refuses it",
        ));
    }
    if environment_method(facade, &row)? != "transit" {
        return Err(fail(
            "this instance's environment does not unseal by the manager's transit: refused",
        ));
    }
    let environment = one(facade, ENVIRONMENT, "environmentId", &field(&row, "environmentId"))?
        .ok_or_else(|| fail(format!("AppInstance {} names no InstanceEnvironment", ask.instance_id)))?;
    let pinned = field(&row, "unsealProofKey");
    // a first pin outside the instance's first start, or the approval policy: a member approves this ask
    let first_late = pinned.is_empty() && !in_first_start(&row);
    let approved = if first_late || field(&environment, "unsealPolicyEnumId") == "IupApproval" {
        match approved_action(facade, ask)? {
            Some(action) => Some(action),
            None => return Ok(Answer::Waiting(ask_approval(facade, &environment, ask, first_late)?)),
        }
    } else {
        None
    };
    let ephemeral = PublicKey::from_bytes(ask.ephemeral_public).map_err(|_| fail("the ephemeral key is not one"))?;
    let transit = transit_key(ask.instance_id)?;
    let root = managed::open_root(&transit, ask.instance_id, ask.wrapped_root).map_err(|_| {
        fail("the wrapped root key does not open with this instance's transit key (another instance's, or changed)")
    })?;
    let proof_public = proof_public_of(&root)?;
    if !pinned.is_empty() && !bool::from(pinned.as_bytes().ct_eq(hex(&proof_public).as_bytes())) {
        // a root key of someone else's making (wrapped to the public transit key by a holder of the link key), or a
        // rotate-root never told: refused and held for an operator, never pinned in place of the instance's
        let report = format!(
            "refused: an unseal on link generation {} from {} sent a root key that is not the instance's (its proof key \
             is not the one pinned); an operator looks (a stolen link key, or a rotate-root: forget#InstanceUnsealProofKey)",
            ask.generation, ask.remote
        );
        record(facade, ask, REFUSED, &report)?;
        tell(
            ask.instance_id,
            "ADMIN",
            REFUSED_TOPIC,
            "an unseal with a root key that is not the instance's was refused",
            ask.remote,
        );
        tracing::error!("Alert: Instances: AppInstance {} {report}", ask.instance_id);
        return Err(fail(
            "the root key sent is not this instance's: refused, and held for an operator",
        ));
    }
    let sealed_root = managed::seal_reply(&ephemeral, ask.instance_id, &root, &mut OsRng)
        .map_err(|err| fail(format!("the answer was not sealed: {err}")))?;
    if pinned.is_empty() {
        row.set("unsealProofKey", Value::string(hex(&proof_public)));
        row.set("unsealProofTrust", Value::string(TRUST_TRANSIT));
    }
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
    if pinned.is_empty() {
        report.push_str("; its unsealed proof key pinned");
    }
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
            record(facade, ask, DONE, &report)?;
        }
    }
    tracing::info!("Instances: AppInstance {} {report}", ask.instance_id);
    Ok(Answer::Sealed {
        sealed_root,
        new_address,
    })
}

/// An unseal action row for `ask`, with the link generation and address that asked; its id.
fn record(facade: &EntityFacade, ask: &Ask<'_>, status: &str, report: &str) -> Result<String, LibError> {
    let mut row = facade.make_value(ACTION)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("appInstanceId", Value::string(ask.instance_id));
    row.set("actionEnumId", Value::string(UNSEAL_ACTION));
    row.set("statusEnumId", Value::string(status));
    row.set("machineAccess", Value::string("N"));
    row.set("report", Value::string(report));
    row.set(
        "linkGeneration",
        Value::I64(i64::try_from(ask.generation).unwrap_or(i64::MAX)),
    );
    row.set("remoteAddress", Value::string(ask.remote));
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

/// Whether `action` was asked on `ask`'s link generation, from its address.
fn same_ask(action: &EntityValue, ask: &Ask<'_>) -> bool {
    field(action, "linkGeneration") == ask.generation.to_string() && field(action, "remoteAddress") == ask.remote
}

/// An approval of this very ask not used yet and not expired (running: a member approved it, and the next unseal on
/// the same link generation, from the same address, takes it); an expired one ends as refused.
fn approved_action(facade: &EntityFacade, ask: &Ask<'_>) -> Result<Option<EntityValue>, LibError> {
    let now_millis = moqui_core::user::now_millis();
    for action in unseal_actions(facade, ask.instance_id, RUNNING)? {
        let until = action
            .get("approvedUntil")
            .and_then(moqui_core::user::value_millis)
            .unwrap_or(0);
        if until <= now_millis {
            crate::actions::finish(
                facade,
                &field(&action, "instanceActionId"),
                REFUSED,
                "the approval expired before the unseal used it",
            )?;
            continue;
        }
        if same_ask(&action, ask) {
            return Ok(Some(action));
        }
    }
    Ok(None)
}

/// The unseal waits for a member: one waiting action, bound to the latest ask (its link generation and address), and
/// the approver group told when it is new.
fn ask_approval(
    facade: &EntityFacade,
    environment: &EntityValue,
    ask: &Ask<'_>,
    first_late: bool,
) -> Result<String, LibError> {
    let group = field(environment, "approverUserGroupId");
    let why = if first_late {
        "its first unseal by transit outside its first start"
    } else {
        "the environment's approval policy"
    };
    let action_id = match unseal_actions(facade, ask.instance_id, WAITING)?.into_iter().next() {
        Some(mut action) => {
            if !same_ask(&action, ask) {
                // what a member approves is the ask that waits now
                action.set(
                    "linkGeneration",
                    Value::I64(i64::try_from(ask.generation).unwrap_or(i64::MAX)),
                );
                action.set("remoteAddress", Value::string(ask.remote));
                action.set(
                    "report",
                    Value::string(format!(
                        "waiting for a member's approval ({why}): an unseal asked on link generation {} from {}",
                        ask.generation, ask.remote
                    )),
                );
                action.update(facade)?;
            }
            field(&action, "instanceActionId")
        }
        None => {
            let id = record(
                facade,
                ask,
                WAITING,
                &format!(
                    "waiting for a member's approval ({why}): an unseal asked on link generation {} from {}",
                    ask.generation, ask.remote
                ),
            )?;
            tell(
                ask.instance_id,
                if group.is_empty() { "ADMIN" } else { &group },
                APPROVAL_TOPIC,
                "an unseal waits for your approval",
                ask.remote,
            );
            id
        }
    };
    Ok(format!(
        "waiting for {} to approve the unseal (InstanceAction {action_id}; {why})",
        if group.is_empty() {
            "an instance administrator".to_string()
        } else {
            format!("a member of {group}")
        }
    ))
}

fn tell(instance_id: &str, group: &str, topic: &str, what: &str, remote: &str) {
    let mut message = moqui_lib::object::ValueMap::default();
    for (name, value) in [("appInstanceId", instance_id), ("remoteAddress", remote)] {
        message.insert(name.to_string(), Value::string(value));
    }
    let clean = |text: &str| text.replace(['$', '{', '}'], "");
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(topic)
        .user_group_id(group)
        .notification_type("warning")
        .message_map(message)
        .title(format!("Instance {}: {what}", clean(instance_id)))
        .send();
    if let Err(err) = sent {
        tracing::error!(
            "Instances: {group} was not told of AppInstance {instance_id} ({what}): {}",
            err.message()
        );
    }
}

/// A member approves a waiting unseal (`approve#InstanceUnseal`): the caller is checked under its own authorization
/// first (INSTANCE_ADMIN and the environment's approver group), then the action runs, and the instance's next unseal on
/// the link generation and from the address that asked takes it, within 15 minutes.
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
        "approvedUntil",
        moqui_core::user::stamp_value(moqui_core::user::now_millis() + APPROVAL_MILLIS),
    );
    action.set(
        "report",
        Value::string(format!("{}; approved by {user}", field(&action, "report"))),
    );
    action.update(facade)?;
    tracing::warn!("Instances: {user} approved an unseal of AppInstance {instance_id} (InstanceAction {action_id})");
    Ok(instance_id)
}

/// Whether the manager takes `link`'s claim of unsealed from a proof (see the module doc): signed by the key pinned
/// for the instance and confirmed. With none pinned and an environment whose method this manager does not unseal, the
/// key given is held as pending, the administrators told, until an operator confirms it ([`confirm_proof_key`]).
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
    if !link_pinned(&row, link.link_fingerprint) {
        return Ok(false);
    }
    let pinned = field(&row, "unsealProofKey");
    let trust = field(&row, "unsealProofTrust");
    if !pinned.is_empty() {
        if !bool::from(pinned.as_bytes().ct_eq(hex(key.as_bytes()).as_bytes())) {
            tracing::warn!(
                "Instances: AppInstance {} proved unsealed with another key than the one pinned (a new root key, or a \
                 claim without its root key): taken as sealed",
                link.instance_id
            );
            return Ok(false);
        }
        // a key that waits for an operator proves nothing yet
        return Ok(trust != TRUST_PENDING && managed::verify_proof(&key, link, signature).is_ok());
    }
    if environment_method(facade, &row)? == "transit" {
        // the manager unseals these: the key it pins is the one it derives from the root key at a transit unseal
        return Ok(false);
    }
    if managed::verify_proof(&key, link, signature).is_err() {
        return Ok(false);
    }
    row.set("unsealProofKey", Value::string(hex(key.as_bytes())));
    row.set("unsealProofTrust", Value::string(TRUST_PENDING));
    row.update(facade)?;
    tell(
        link.instance_id,
        "ADMIN",
        REFUSED_TOPIC,
        "its first unsealed proof key waits for an operator's confirmation",
        "",
    );
    tracing::warn!(
        "Instances: AppInstance {} gave its unsealed proof key on its first unsealed link (its environment's method is \
         not the manager's transit): held until an operator confirms it (confirm#InstanceUnsealProofKey)",
        link.instance_id
    );
    Ok(false)
}

/// An operator confirms the unsealed proof key an instance gave on its first unsealed link
/// (`confirm#InstanceUnsealProofKey`): its proofs are taken from then on. INSTANCE_ADMIN and the environment's group.
pub fn confirm_proof_key(facade: &EntityFacade, instance_id: &str) -> Result<String, LibError> {
    let mut row = instance(facade, instance_id)?;
    crate::link::check_caller(facade, &row)?;
    let _authz = authz_off();
    if field(&row, "unsealProofTrust") != TRUST_PENDING {
        return Err(fail(format!(
            "AppInstance {instance_id} has no unsealed proof key waiting for a confirmation"
        )));
    }
    let key = field(&row, "unsealProofKey");
    row.set("unsealProofTrust", Value::string(TRUST_FIRST_USE));
    row.update(facade)?;
    tracing::warn!("Instances: an operator confirmed the unsealed proof key {key} of AppInstance {instance_id}");
    Ok(key)
}

/// A rotate-root's new proof key (the `RotateProof` call): taken only on the instance's pinned link, signed by the
/// proof key pinned now (the old root key's), and under the manager's transit only when the new wrap opens to a root
/// key whose proof key it is. The new key keeps the old one's trust.
pub fn rotate_proof(
    facade: &EntityFacade,
    instance_id: &str,
    link_fingerprint: &str,
    new_public: &[u8],
    wrapped: &[u8],
    signature: &[u8],
) -> Result<(), LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, instance_id)?;
    if !link_pinned(&row, link_fingerprint) {
        return Err(fail(
            "this instance's link key is not pinned on the manager, or was revoked",
        ));
    }
    let pinned = field(&row, "unsealProofKey");
    if pinned.is_empty() || field(&row, "unsealProofTrust") == TRUST_PENDING {
        return Err(fail(
            "no confirmed unsealed proof key is pinned for this instance to sign a new one",
        ));
    }
    let old = unhex(&pinned)
        .ok()
        .and_then(|bytes| VerifyingKey::from_bytes(&bytes).ok())
        .ok_or_else(|| fail("the pinned unsealed proof key does not read"))?;
    let new_public: [u8; 32] = new_public
        .try_into()
        .map_err(|_| fail("the new proof key is an Ed25519 key"))?;
    managed::verify_rotation(&old, instance_id, link_fingerprint, &new_public, wrapped, signature)
        .map_err(|_| fail("the new proof key is not signed by the one pinned: refused"))?;
    if environment_method(facade, &row)? == "transit" {
        let transit = transit_key(instance_id)?;
        let root = managed::open_root(&transit, instance_id, wrapped)
            .map_err(|_| fail("the new wrap does not open with this instance's transit key"))?;
        if proof_public_of(&root)? != new_public {
            return Err(fail(
                "the new wrap's root key is not the one the new proof key is of: refused",
            ));
        }
    }
    row.set("unsealProofKey", Value::string(hex(&new_public)));
    row.update(facade)?;
    tracing::warn!(
        "Instances: AppInstance {instance_id} rotated its root key: its new unsealed proof key, signed by the old one, \
         is pinned"
    );
    Ok(())
}

/// Forget the unsealed proof key pinned for `instance_id` (`forget#InstanceUnsealProofKey`): an operator takes the
/// instance's new root key when a rotate-root could not be told (its old proof key lost with a restart). Under the
/// manager's transit its next unseal pins the new key with a member's approval; under another method its next proof
/// waits for a confirmation. INSTANCE_ADMIN and the environment's group.
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
