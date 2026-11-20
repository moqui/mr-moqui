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

//! The manager's rules of the manager link (plans/Vault-Instances.md §4.4), on its rows; the link itself is the host's
//! (`moqui_host::manage_link`), which calls these.
//!
//! - **Pairing.** A provision gives a new mr-moqui instance a one-time token as a file (§4.2), and System › Instances
//!   makes a code for an instance the manager did not make ([`make_pairing_code`]). Either is kept only as its SHA-256
//!   on the instance's row, with when it stops working (15 minutes). [`pair`] takes it once: it must match that row's
//!   hash and be in time, and it is cleared as the instance's link key and bundle key are pinned on the row, so it
//!   works once, for that instance only.
//! - **Approvers.** At pairing the instance takes its environment's approvers' signing keys (the Ed25519 halves of
//!   their personal keys), version 1, and pins them; the row keeps what it was given. A changed set goes to it signed
//!   by an approver of the set it pinned ([`check_approver_update`], `moqui_vault::approvers`).
//! - **Calls.** Each call of the manager on an instance is checked against the instance's environment and the caller
//!   ([`check_caller`]): INSTANCE_ADMIN, and a member of the environment's approver group when it names one.
//! - **The bootstrap bundle.** A provision holds the new instance's bootstrap secrets in memory ([`hold_bundle`]), never
//!   in a row, until the instance takes them over the link sealed to the bundle key it pinned ([`take_bundle`]); a
//!   manager restart or the token's 15 minutes end them.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::transaction::TransactionAttribute;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use moqui_vault::approvers::{Approver, ApproverSet};
use serde_json::json;
use sha2::Digest as _;
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::common::{
    authz_off, fail, field, find, new_credential, one, require_instance_admin, APP_INSTANCE, ENVIRONMENT,
};

/// An instance the manager paired: its row, environment and approvers as pinned.
#[derive(Clone, Debug)]
pub struct Paired {
    pub app_instance_id: String,
    pub environment_id: String,
    pub approvers: ApproverSet,
}

/// An instance whose link key is pinned: what a link of that key is for.
#[derive(Clone, Debug)]
pub struct Linked {
    pub app_instance_id: String,
    pub environment_id: String,
    /// The X25519 key its bootstrap bundle is sealed to, pinned at pairing.
    pub bundle_key: Vec<u8>,
}

/// The manager's own link address and key fingerprint, which a provision gives a new mr-moqui instance in its settings
/// (`manage@url`, `manage@pin`; neither is a secret). The host sets them when it starts.
static MANAGER: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Keep the manager's link URL and key fingerprint for the provisions of this process.
pub fn set_manager(url: &str, pin: &str) {
    *lock(&MANAGER) = Some((url.to_string(), pin.to_string()));
}

/// The manager's link URL and key fingerprint, once the host has set them and the URL is not empty.
#[must_use]
pub fn manager() -> Option<(String, String)> {
    lock(&MANAGER).clone().filter(|(url, _)| !url.is_empty())
}

fn sha256_hex(secret: &str) -> String {
    moqui_vault::text::hex(&sha2::Sha256::digest(secret.trim().as_bytes()))
}

/// Take a pairing token or code once (see the module doc): the instance whose row holds its hash, in time, pins
/// `key_fingerprint` (its link key) and `bundle_key`, and the token is cleared. Refused: an unknown, used or expired
/// secret, a bundle key that is not one, and an instance whose environment is gone.
///
/// Once: the row is read for update in a transaction and its hash checked again under that lock, and this process
/// takes one pairing at a time, so two calls with one token never both pair.
pub fn pair(facade: &EntityFacade, secret: &str, key_fingerprint: &str, bundle_key: &[u8]) -> Result<Paired, LibError> {
    const UNKNOWN: &str =
        "the pairing token or code is unknown, used or expired: make a new code in System › Instances";
    static PAIRING: Mutex<()> = Mutex::new(());
    let _authz = authz_off();
    if secret.trim().is_empty() || moqui_vault::hpke::PublicKey::from_bytes(bundle_key).is_err() {
        return Err(fail(UNKNOWN));
    }
    let hash = sha256_hex(secret);
    let _one = lock(&PAIRING);
    let tx = facade.transactions_for(APP_INSTANCE)?;
    let taken = tx.run(
        TransactionAttribute::UseOrBegin,
        None,
        || -> Result<Result<Paired, &'static str>, LibError> {
            let Some(mut row) = find(facade, APP_INSTANCE)?
                .eq("pairingTokenHash", Value::string(&hash))
                .for_update(true)
                .one()?
            else {
                return Ok(Err(UNKNOWN));
            };
            // the row was found by its hash, under its lock; compared again in constant time, so a match is never decided
            // by the database's comparison alone, and a token another call took meanwhile is gone from it
            if !bool::from(field(&row, "pairingTokenHash").as_bytes().ct_eq(hash.as_bytes())) {
                return Ok(Err(UNKNOWN));
            }
            let expires = row
                .get("pairingTokenExpires")
                .and_then(moqui_core::user::value_millis)
                .unwrap_or(0);
            let id = field(&row, "appInstanceId");
            row.set("pairingTokenHash", Value::Null);
            row.set("pairingTokenExpires", Value::Null);
            if expires <= moqui_core::user::now_millis() {
                // spent either way: an expired token is cleared, so it is refused the same again
                row.update(facade)?;
                tracing::warn!("Manager link: an expired pairing token or code for AppInstance {id} was refused");
                return Ok(Err(UNKNOWN));
            }
            let environment_id = field(&row, "environmentId");
            let approvers = approver_set(facade, &environment_id, 1)?;
            if approvers.approvers.is_empty() {
                // nothing for the instance to check its recovery holders' keys against, and no approver can sign a new
                // set: it pairs, and pairs again once the environment has approvers with personal keys
                tracing::warn!(
                "Manager link: AppInstance {id} pins no approver: its environment {environment_id} names no approver \
                 group, or none of its members has a personal key; pair it again once one does"
            );
            }
            row.set("linkKeyFingerprint", Value::string(key_fingerprint));
            row.set("linkBundleKey", Value::string(moqui_vault::text::hex(bundle_key)));
            row.set("linkPairedDate", moqui_core::user::now_value());
            row.set("linkState", Value::string("paired"));
            row.set("approverSet", Value::string(approvers_json(&approvers)));
            row.update(facade)?;
            tracing::info!("Manager link: AppInstance {id} paired with link key {key_fingerprint}");
            Ok(Ok(Paired {
                app_instance_id: id,
                environment_id,
                approvers,
            }))
        },
    )?;
    taken.map_err(fail)
}

/// A pairing code for `instance_id`, an instance the manager did not make (§4.4: an existing server being adopted):
/// shown once, kept as its hash, good for 15 minutes. INSTANCE_ADMIN and the environment's approver group.
pub fn make_pairing_code(facade: &EntityFacade, instance_id: &str) -> Result<Zeroizing<String>, LibError> {
    // the caller is checked under its own authorization: with it disabled, both checks would pass
    let mut row = instance(facade, instance_id)?;
    check_caller(facade, &row)?;
    let _authz = authz_off();
    let code = new_credential();
    let expires = moqui_core::user::now_millis() + crate::kinds::PAIRING_TOKEN_MINUTES * 60 * 1000;
    row.set("pairingTokenHash", Value::string(sha256_hex(&code)));
    row.set("pairingTokenExpires", moqui_core::user::stamp_value(expires));
    row.update(facade)?;
    tracing::info!("Manager link: a pairing code was made for AppInstance {instance_id}");
    Ok(code)
}

/// The instance whose pinned link key is `key_fingerprint`, or none: a key never paired, or revoked.
pub fn linked(facade: &EntityFacade, key_fingerprint: &str) -> Result<Option<Linked>, LibError> {
    if key_fingerprint.is_empty() {
        return Ok(None);
    }
    let _authz = authz_off();
    let row = find(facade, APP_INSTANCE)?
        .eq("linkKeyFingerprint", Value::string(key_fingerprint))
        .one()?;
    Ok(row.map(|row| Linked {
        app_instance_id: field(&row, "appInstanceId"),
        environment_id: field(&row, "environmentId"),
        bundle_key: moqui_vault::text::unhex(&field(&row, "linkBundleKey")).unwrap_or_default(),
    }))
}

/// The instance's row, or an error naming it.
pub fn instance(facade: &EntityFacade, instance_id: &str) -> Result<EntityValue, LibError> {
    one(facade, APP_INSTANCE, "appInstanceId", instance_id)?
        .ok_or_else(|| fail(format!("no AppInstance {instance_id}")))
}

/// The caller may call the manager link of `instance` (§4.4, Authorization): INSTANCE_ADMIN, and when the instance's
/// environment names an approver group, a member of it; a caller with authorization disabled (the framework's own
/// call, a job) passes, so a service checks its caller before it turns authorization off (the row reads here turn it
/// off for themselves).
pub fn check_caller(facade: &EntityFacade, instance: &EntityValue) -> Result<(), LibError> {
    require_instance_admin()?;
    let environment_id = field(instance, "environmentId");
    let Some(environment) = one(facade, ENVIRONMENT, "environmentId", &environment_id)? else {
        return Err(fail(format!(
            "AppInstance {} names no InstanceEnvironment",
            field(instance, "appInstanceId")
        )));
    };
    crate::actions::check_caller(&environment)
}

/// How the manager reaches `instance` (§4.4): its environment's transport, `link`, `ssh` or `both` (the link, and SSH
/// while it is down); `link` when the environment names none.
pub fn transport(facade: &EntityFacade, instance: &EntityValue) -> Result<&'static str, LibError> {
    let _authz = authz_off();
    let environment = one(facade, ENVIRONMENT, "environmentId", &field(instance, "environmentId"))?;
    Ok(match environment.map(|row| field(&row, "transportEnumId")).as_deref() {
        Some("ItrSsh") => "ssh",
        Some("ItrBoth") => "both",
        _ => "link",
    })
}

/// The environment's approvers now: each member of its approver group with an active personal key, as set `version`.
pub fn approver_set(facade: &EntityFacade, environment_id: &str, version: u64) -> Result<ApproverSet, LibError> {
    let _authz = authz_off();
    let group = one(facade, ENVIRONMENT, "environmentId", environment_id)?
        .map(|environment| field(&environment, "approverUserGroupId"))
        .unwrap_or_default();
    let mut approvers = Vec::new();
    if !group.is_empty() {
        let now = moqui_core::user::now_millis();
        let members = find(facade, "moqui.security.UserGroupMember")?
            .eq("userGroupId", Value::string(&group))
            .list()?
            .into_values();
        for member in members {
            let ended = member
                .get("thruDate")
                .and_then(moqui_core::user::value_millis)
                .is_some_and(|thru| thru <= now);
            if ended {
                continue;
            }
            let user_id = field(&member, "userId");
            let key = find(facade, "moqui.security.vault.VaultPersonalKey")?
                .eq("userId", Value::string(&user_id))
                .eq("statusId", Value::string("VpkActive"))
                .one()?;
            let Some(key) = key else { continue };
            match moqui_vault::text::unhex(&field(&key, "verifyPublic"))
                .ok()
                .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
            {
                Some(verify_public) => approvers.push(Approver { user_id, verify_public }),
                None => tracing::error!(
                    "Manager link: the personal key of approver {user_id} has no signing key; it is left out"
                ),
            }
        }
    }
    Ok(ApproverSet { version, approvers }.sorted())
}

/// The approver set as the instance's row keeps it.
#[must_use]
pub fn approvers_json(set: &ApproverSet) -> String {
    json!({
        "version": set.version,
        "approvers": set.approvers.iter().map(|approver| json!({
            "userId": approver.user_id,
            "verifyPublic": moqui_vault::text::hex(&approver.verify_public),
        })).collect::<Vec<_>>(),
    })
    .to_string()
}

/// The approver set pinned on an instance's row; an empty set at version 0 when there is none or it does not read.
#[must_use]
pub fn pinned_approvers(row: &EntityValue) -> ApproverSet {
    let text = field(row, "approverSet");
    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let approvers = parsed["approvers"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|approver| {
                    let key = moqui_vault::text::unhex(approver["verifyPublic"].as_str()?).ok()?;
                    Some(Approver {
                        user_id: approver["userId"].as_str()?.to_string(),
                        verify_public: <[u8; 32]>::try_from(key.as_slice()).ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    ApproverSet {
        version: parsed["version"].as_u64().unwrap_or(0),
        approvers,
    }
}

/// A changed approver set for `instance_id`, signed by `signer`: checked against the set its row pinned, as the
/// instance checks it, before it is sent (the instance's own check is the one that protects it).
pub fn check_approver_update(
    facade: &EntityFacade,
    instance_id: &str,
    set: &ApproverSet,
    signer: &[u8],
    signature: &[u8],
) -> Result<(), LibError> {
    let row = instance(facade, instance_id)?;
    check_caller(facade, &row)?;
    set.verify_update(
        instance_id,
        &field(&row, "linkKeyFingerprint"),
        &pinned_approvers(&row),
        signer,
        signature,
    )
    .map_err(|err| {
        fail(format!(
            "the approver set for AppInstance {instance_id} is refused: {err}"
        ))
    })
}

/// Keep `set` as the approvers `instance_id` pinned, once it took it.
pub fn store_approvers(facade: &EntityFacade, instance_id: &str, set: &ApproverSet) -> Result<(), LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, instance_id)?;
    row.set("approverSet", Value::string(approvers_json(set)));
    row.update(facade)?;
    Ok(())
}

/// Write what the link says of `instance_id`: its link state (linked, down, revoked), its seal state when known, and
/// when; a failure is logged, since the link goes on either way.
pub fn note_state(facade: &EntityFacade, instance_id: &str, link_state: &str, seal_state: Option<&str>, address: &str) {
    let _authz = authz_off();
    let written = (|| -> Result<(), LibError> {
        let mut row = instance(facade, instance_id)?;
        row.set("linkState", Value::string(link_state));
        row.set("linkCheckedDate", moqui_core::user::now_value());
        if !address.is_empty() {
            row.set("linkAddress", Value::string(address));
        }
        // Monitor (§4.8): since when it is sealed, and an unseal with whether its address is new
        let unsealed = seal_state.and_then(|seal_state| crate::monitor::note_seal(&mut row, seal_state, address));
        if let Some(seal_state) = seal_state {
            row.set("sealState", Value::string(seal_state));
            row.set("sealCheckedDate", moqui_core::user::now_value());
        }
        row.update(facade)?;
        if let Some(new_address) = unsealed {
            crate::monitor::count_unseal(instance_id, &field(&row, "environmentId"), new_address);
        }
        Ok(())
    })();
    if let Err(err) = written {
        tracing::error!("Manager link: the link state of AppInstance {instance_id} was not written: {err}");
    }
    crate::monitor::refresh(facade);
}

/// Note that `instance_id` took its bootstrap bundle.
pub fn note_bootstrap(facade: &EntityFacade, instance_id: &str) -> Result<(), LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, instance_id)?;
    row.set("bootstrapDate", moqui_core::user::now_value());
    row.update(facade)?;
    Ok(())
}

/// Revoke `instance_id`'s link: its link key is no longer pinned, so a link of it is refused until it pairs again with
/// a new code. INSTANCE_ADMIN and the environment's group.
pub fn revoke(facade: &EntityFacade, instance_id: &str) -> Result<(), LibError> {
    let mut row = instance(facade, instance_id)?;
    check_caller(facade, &row)?;
    let _authz = authz_off();
    // a pairing token or code not yet used would pair it again
    row.set("pairingTokenHash", Value::Null);
    row.set("pairingTokenExpires", Value::Null);
    row.set("linkKeyFingerprint", Value::Null);
    row.set("linkBundleKey", Value::Null);
    row.set("linkState", Value::string("revoked"));
    row.set("linkCheckedDate", moqui_core::user::now_value());
    row.update(facade)?;
    drop_bundle(instance_id);
    tracing::warn!("Manager link: the link of AppInstance {instance_id} was revoked");
    Ok(())
}

/// The bootstrap secrets of an instance, by secret name (`entity-facade/datasource[transactional]@password`): what its
/// boot needs before its database opens (Sealed-Secrets S4, `moqui_core::seal::secrets::is_bootstrap`).
pub type BundleValues = BTreeMap<String, Zeroizing<String>>;

struct Held {
    values: BundleValues,
    until: i64,
}

static BUNDLES: LazyLock<Mutex<BTreeMap<String, Held>>> = LazyLock::new(Mutex::default);

/// Hold `values` for `instance_id` until it takes them or `until` (epoch millis) passes: in memory only.
pub fn hold_bundle(instance_id: &str, values: BundleValues, until: i64) {
    lock(&BUNDLES).insert(instance_id.to_string(), Held { values, until });
}

/// Whether a bundle waits for `instance_id`.
#[must_use]
pub fn bundle_ready(instance_id: &str) -> bool {
    let now = moqui_core::user::now_millis();
    lock(&BUNDLES).get(instance_id).is_some_and(|held| held.until > now)
}

/// The bundle held for `instance_id`, once: none when there is none or its time passed.
#[must_use]
pub fn take_bundle(instance_id: &str) -> Option<BundleValues> {
    let held = lock(&BUNDLES).remove(instance_id)?;
    (held.until > moqui_core::user::now_millis()).then_some(held.values)
}

/// Forget the bundle held for `instance_id` (its provision was undone, or its link revoked).
pub fn drop_bundle(instance_id: &str) {
    lock(&BUNDLES).remove(instance_id);
}
