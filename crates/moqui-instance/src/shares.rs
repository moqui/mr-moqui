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

//! A managed instance's recovery shares on the manager (plans/archive/Vault-Instances.md §4.5, M6). The manager sees no share:
//!
//! - **Holders** ([`holders`]): the members of the environment's recovery group with an active personal key, each with
//!   the key checks of that key. The instance takes a holder's key only with a check signed by an approver it pinned at
//!   pairing, so the manager cannot put a key of its own in a holder's place without an approver's signature.
//! - **The shares** ([`record_shares`]): the instance seals each share to one holder and signs the delivery with its
//!   link key; the manager checks that key against the fingerprint it pinned, that each delivery goes to a current
//!   holder's active key, one share to a holder, and stores them as deliveries (`VaultDelivery`, sender `instance`) with
//!   an `InstanceRecoveryShare` row each. The holder's client confirms with their signing key ([`confirm`]); the
//!   instance checks the confirmations itself ([`confirmations`] hands them over). An unconfirmed share is reported.
//! - **The relay** ([`offer_unseal_key`], [`post_sealed_share`]): a sealed instance makes a key for one unseal, with a
//!   time it stops being taken, and signs it with its link key; a holder's `moqui vault seal-share` checks it against
//!   the link key fingerprint the holder filed with their share and seals their share to it. The key, its signature and
//!   the sealed shares are rows (any member of a manager cluster answers), ciphertext only, dropped when the instance
//!   takes them, unseals ([`drop_relay`]) or its link ends.
//! - **The recovery bundle** ([`deposit_seal_bundle`]): what `export-seal` gives (wrapped values only), deposited as an
//!   operational item of the environment's vault beside the instance's other items.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::delivery::Delivery;
use moqui_vault::fingerprint::KeyCheck;
use moqui_vault::managed::{self, UnsealKey};
use moqui_vault::text::{hex, unhex};
use serde_json::json;

use crate::common::{authz_off, fail, field, find, new_item_id, now, one, rows, ENVIRONMENT};
use crate::link::instance;

const SHARE: &str = "moqui.server.instance.InstanceRecoveryShare";
const DELIVERY: &str = "moqui.security.vault.VaultDelivery";
const PERSONAL_KEY: &str = "moqui.security.vault.VaultPersonalKey";
const KEY_CHECK: &str = "moqui.security.vault.VaultKeyCheck";

/// The most holders and key checks the manager hands an instance.
const MAX_HOLDERS: usize = 32;
const MAX_CHECKS: usize = 16;

/// A recovery holder as the instance is given them.
#[derive(Clone, Debug)]
pub struct HolderKey {
    pub user_id: String,
    pub personal_key_id: String,
    pub encrypt_public: [u8; 32],
    pub verify_public: [u8; 32],
    pub checks: Vec<KeyCheck>,
}

fn key32(text: &str) -> Option<[u8; 32]> {
    unhex(text)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
}

/// The members of `instance_id`'s environment's recovery group, now, with their active personal keys and the checks of
/// them by a member of the environment's approver group (see the module doc); none when the environment names no group.
/// A user with two active keys is given with the newest; a key given for two users is left out.
pub fn holders(facade: &EntityFacade, instance_id: &str) -> Result<Vec<HolderKey>, LibError> {
    let _authz = authz_off();
    let row = instance(facade, instance_id)?;
    let environment = one(facade, ENVIRONMENT, "environmentId", &field(&row, "environmentId"))?;
    let group = environment
        .as_ref()
        .map(|environment| field(environment, "recoveryUserGroupId"))
        .unwrap_or_default();
    if group.is_empty() {
        return Ok(Vec::new());
    }
    let approver_group = environment
        .as_ref()
        .map(|environment| field(environment, "approverUserGroupId"))
        .unwrap_or_default();
    let now_millis = moqui_core::user::now_millis();
    let members = |group: &str| -> Result<Vec<String>, LibError> {
        let mut users: Vec<String> = rows(facade, "moqui.security.UserGroupMember", "userGroupId", group)?
            .iter()
            .filter(|member| {
                member
                    .get("thruDate")
                    .and_then(moqui_core::user::value_millis)
                    .is_none_or(|thru| thru > now_millis)
            })
            .map(|member| field(member, "userId"))
            .collect();
        users.sort_unstable();
        users.dedup();
        Ok(users)
    };
    let approvers = members(&approver_group)?;
    let mut holders: Vec<HolderKey> = Vec::new();
    for user_id in members(&group)?.into_iter().take(MAX_HOLDERS) {
        let mut keys = find(facade, PERSONAL_KEY)?
            .eq("userId", Value::string(&user_id))
            .eq("statusId", Value::string("VpkActive"))
            .order_by("fromDate", false)
            .list()?
            .into_values();
        if keys.len() > 1 {
            tracing::error!(
                "Instances: recovery holder {user_id} has {} active personal keys; the newest is taken",
                keys.len()
            );
        }
        let Some(key) = (!keys.is_empty()).then(|| keys.swap_remove(0)) else {
            tracing::warn!("Instances: recovery holder {user_id} has no personal vault key; left out");
            continue;
        };
        let (Some(encrypt_public), Some(verify_public)) = (
            key32(&field(&key, "encryptPublic")),
            key32(&field(&key, "verifyPublic")),
        ) else {
            tracing::error!("Instances: the personal key of recovery holder {user_id} does not read; left out");
            continue;
        };
        if holders
            .iter()
            .any(|other| other.encrypt_public == encrypt_public || other.verify_public == verify_public)
        {
            tracing::error!("Instances: recovery holder {user_id}'s key is another holder's too; left out");
            continue;
        }
        let personal_key_id = field(&key, "personalKeyId");
        // the checks by the environment's approvers only, before the limit: another user's check is of no use to the
        // instance, and a flood of them must not push an approver's out
        let checks = find(facade, KEY_CHECK)?
            .eq("checkedKeyId", Value::string(&personal_key_id))
            .eq("checkedUserId", Value::string(&user_id))
            .list()?
            .into_values()
            .iter()
            .filter(|check| approvers.binary_search(&field(check, "checkerUserId")).is_ok())
            .take(MAX_CHECKS)
            .filter_map(|check| {
                Some(KeyCheck {
                    checker_user_id: field(check, "checkerUserId"),
                    checker_key_id: field(check, "checkerKeyId"),
                    checked_key_id: field(check, "checkedKeyId"),
                    checked_user_id: field(check, "checkedUserId"),
                    checked_encrypt: unhex(&field(check, "checkedEncrypt")).ok()?,
                    checked_verify: unhex(&field(check, "checkedVerify")).ok()?,
                    checked_at: field(check, "checkedAt").parse().ok()?,
                    signature: unhex(&field(check, "signature")).ok()?,
                })
            })
            .collect();
        holders.push(HolderKey {
            user_id,
            personal_key_id,
            encrypt_public,
            verify_public,
            checks,
        });
    }
    Ok(holders)
}

/// One share's delivery as the instance sent it.
#[derive(Clone, Debug)]
pub struct SentShare {
    pub index: u32,
    pub delivery: Delivery,
}

/// Keep a share set the instance sent (see the module doc). Refused whole: a link key that is not the one pinned, a
/// delivery it did not sign, one to someone who is not a holder now or not to their active key, two to one holder, or
/// a delivery id taken.
pub fn record_shares(
    facade: &EntityFacade,
    instance_id: &str,
    link_public: &[u8],
    set_id: &str,
    threshold: u32,
    shares: &[SentShare],
) -> Result<(), LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, instance_id)?;
    let link = managed::pinned_link_key(link_public, &field(&row, "linkKeyFingerprint"))
        .map_err(|err| fail(format!("the share set of AppInstance {instance_id} is refused: {err}")))?;
    if set_id.is_empty() || set_id.len() > 40 || shares.is_empty() {
        return Err(fail(
            "a share set has an id of at most 40 characters and at least one share",
        ));
    }
    // each index once, from 1 to the count, and a threshold the shares can meet
    let count = u32::try_from(shares.len()).unwrap_or(u32::MAX);
    let mut indexes: Vec<u32> = shares.iter().map(|share| share.index).collect();
    indexes.sort_unstable();
    indexes.dedup();
    if indexes.len() != shares.len() || indexes.first() != Some(&1) || indexes.last() != Some(&count) {
        return Err(fail("a share set's indexes are 1 to its count, each once"));
    }
    if threshold < 2 || threshold > count {
        return Err(fail("a share set's threshold is at least 2 and at most its count"));
    }
    let current: BTreeMap<String, String> = holders(facade, instance_id)?
        .into_iter()
        .map(|holder| (holder.user_id, holder.personal_key_id))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for share in shares {
        let delivery = &share.delivery;
        delivery
            .verify(&link)
            .map_err(|_| fail("a share's delivery is not signed by the instance's link key"))?;
        if delivery.sender_kind != "instance"
            || delivery.sender_id != instance_id
            || delivery.purpose != managed::SHARE_PURPOSE
        {
            return Err(fail("a share's delivery names another sender or purpose"));
        }
        if current.get(&delivery.recipient_user_id) != Some(&delivery.recipient_key_id) {
            return Err(fail(format!(
                "a share goes to {}, who is not a recovery holder of this instance's environment with that key",
                delivery.recipient_user_id
            )));
        }
        // no place holds two shares (§4.5)
        if !seen.insert(delivery.recipient_user_id.clone()) {
            return Err(fail(format!(
                "two shares go to {}: each holder holds one",
                delivery.recipient_user_id
            )));
        }
        if delivery.delivery_id.is_empty()
            || delivery.delivery_id.len() > 40
            || one(facade, DELIVERY, "deliveryId", &delivery.delivery_id)?.is_some()
        {
            return Err(fail(format!("the delivery id {} is not free", delivery.delivery_id)));
        }
    }
    let tx = facade.transactions_for(SHARE)?;
    tx.run(
        moqui_core::transaction::TransactionAttribute::UseOrBegin,
        None,
        || -> Result<(), LibError> {
            for share in shares {
                let delivery = &share.delivery;
                let mut sent = facade.make_value(DELIVERY)?;
                sent.set("deliveryId", Value::string(&delivery.delivery_id));
                sent.set("recipientUserId", Value::string(&delivery.recipient_user_id));
                sent.set("recipientKeyId", Value::string(&delivery.recipient_key_id));
                sent.set("purpose", Value::string(&delivery.purpose));
                sent.set("senderKind", Value::string("instance"));
                sent.set("senderId", Value::string(instance_id));
                sent.set("sealedValue", Value::string(hex(&delivery.sealed)));
                sent.set("signature", Value::string(hex(&delivery.signature)));
                sent.set("sentDate", now());
                sent.create(facade)?;
                let mut kept = facade.make_value(SHARE)?;
                kept.set("appInstanceId", Value::string(instance_id));
                kept.set("setId", Value::string(set_id));
                kept.set("shareIndex", Value::I64(i64::from(share.index)));
                kept.set("holderUserId", Value::string(&delivery.recipient_user_id));
                kept.set("holderKeyId", Value::string(&delivery.recipient_key_id));
                kept.set("deliveryId", Value::string(&delivery.delivery_id));
                kept.set("threshold", Value::I64(i64::from(threshold)));
                kept.set("sentDate", now());
                kept.create(facade)?;
            }
            row.set("linkSignPublic", Value::string(hex(link.as_bytes())));
            row.set("recoverySetId", Value::string(set_id));
            row.update(facade)?;
            Ok(())
        },
    )?;
    tracing::info!(
        "Instances: AppInstance {instance_id} sent its recovery share set {set_id}: {} shares, {threshold} to unseal",
        shares.len()
    );
    Ok(())
}

/// A holder's confirmation, as the instance checks it.
#[derive(Clone, Debug)]
pub struct Confirmation {
    pub index: u32,
    pub holder_user_id: String,
    pub signature: Vec<u8>,
}

/// The confirmations of `instance_id`'s share set `set_id` so far.
pub fn confirmations(facade: &EntityFacade, instance_id: &str, set_id: &str) -> Result<Vec<Confirmation>, LibError> {
    let _authz = authz_off();
    Ok(find(facade, SHARE)?
        .eq("appInstanceId", Value::string(instance_id))
        .eq("setId", Value::string(set_id))
        .list()?
        .into_values()
        .iter()
        .filter(|share| !field(share, "confirmation").is_empty())
        .filter_map(|share| {
            Some(Confirmation {
                index: u32::try_from(field(share, "shareIndex").parse::<i64>().ok()?).ok()?,
                holder_user_id: field(share, "holderUserId"),
                signature: unhex(&field(share, "confirmation")).ok()?,
            })
        })
        .collect())
}

/// A holder's confirmation of the share delivered as `delivery_id` (`confirm#InstanceRecoveryShare`): only its
/// recipient confirms it; the signature is the instance's to check.
pub fn confirm(facade: &EntityFacade, delivery_id: &str, signature: &str) -> Result<String, LibError> {
    let user = moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .ok_or_else(|| fail("a confirmation is its holder's, signed in"))?;
    let _authz = authz_off();
    let mut share = find(facade, SHARE)?
        .eq("deliveryId", Value::string(delivery_id))
        .one()?
        .filter(|share| field(share, "holderUserId") == user)
        .ok_or_else(|| fail(format!("No recovery share of yours was delivered as {delivery_id}")))?;
    let signature = unhex(signature.trim()).map_err(|_| fail("the confirmation is hex"))?;
    if signature.len() != moqui_vault::sign::SIGNATURE_LEN {
        return Err(fail("the confirmation is an Ed25519 signature"));
    }
    share.set("confirmation", Value::string(hex(&signature)));
    share.set("confirmedDate", now());
    share.update(facade)?;
    let instance_id = field(&share, "appInstanceId");
    tracing::info!(
        "Instances: {user} confirmed recovery share {} of AppInstance {instance_id}",
        field(&share, "shareIndex")
    );
    Ok(instance_id)
}

/// The shares of `instance_id`'s current set and whether each is confirmed, for its page: (index, holder, confirmed).
pub fn share_report(facade: &EntityFacade, instance_id: &str) -> Result<Vec<(i64, String, bool)>, LibError> {
    let _authz = authz_off();
    let row = instance(facade, instance_id)?;
    let set_id = field(&row, "recoverySetId");
    if set_id.is_empty() {
        return Ok(Vec::new());
    }
    let mut report: Vec<(i64, String, bool)> = find(facade, SHARE)?
        .eq("appInstanceId", Value::string(instance_id))
        .eq("setId", Value::string(&set_id))
        .list()?
        .into_values()
        .iter()
        .map(|share| {
            (
                field(share, "shareIndex").parse().unwrap_or(0),
                field(share, "holderUserId"),
                !field(share, "confirmation").is_empty(),
            )
        })
        .collect();
    report.sort();
    Ok(report)
}

const SEALED_SHARE: &str = "moqui.server.instance.InstanceSealedShare";

/// A sealed instance's key for this unseal (see the module doc): checked against its pinned link key and its expiry,
/// kept on its row in place of one it made before (whose sealed shares are dropped), and the shares sealed to it since
/// handed over once, as ciphertext.
pub fn offer_unseal_key(
    facade: &EntityFacade,
    instance_id: &str,
    link_public: &[u8],
    key: &UnsealKey,
) -> Result<Vec<(String, Vec<u8>)>, LibError> {
    let _authz = authz_off();
    let mut row = instance(facade, instance_id)?;
    if key.instance_id != instance_id {
        return Err(fail("the unseal key names another instance"));
    }
    key.verify(
        link_public,
        &field(&row, "linkKeyFingerprint"),
        moqui_core::user::now_millis(),
    )
    .map_err(|err| fail(format!("the unseal key of AppInstance {instance_id} is refused: {err}")))?;
    let unseal_public = hex(&key.unseal_public);
    let mut taken = Vec::new();
    if field(&row, "unsealKeyPublic") == unseal_public {
        for sealed in rows(facade, SEALED_SHARE, "appInstanceId", instance_id)? {
            if field(&sealed, "unsealPublic") == unseal_public {
                if let Ok(bytes) = unhex(&field(&sealed, "sealedShare")) {
                    taken.push((field(&sealed, "holderUserId"), bytes));
                }
            }
            sealed.delete(facade)?;
        }
        return Ok(taken);
    }
    // a new unseal's key: what was sealed to the one before is dropped
    for sealed in rows(facade, SEALED_SHARE, "appInstanceId", instance_id)? {
        sealed.delete(facade)?;
    }
    row.set("unsealKeyPublic", Value::string(&unseal_public));
    row.set("unsealKeySignature", Value::string(hex(&key.signature)));
    row.set("unsealKeyExpires", Value::I64(key.expires));
    row.set("linkSignPublic", Value::string(hex(link_public)));
    row.update(facade)?;
    Ok(taken)
}

/// The unseal key `instance_id` offers now, with its link key, for a holder's client to check against the link key
/// fingerprint the holder filed with their share (`get#InstanceUnsealKey`); none when it asks for no shares.
pub fn unseal_key(facade: &EntityFacade, instance_id: &str) -> Result<Option<(UnsealKey, [u8; 32])>, LibError> {
    let (_, row) = require_holder(facade, instance_id)?;
    let (Some(unseal_public), Some(link_public)) = (
        key32(&field(&row, "unsealKeyPublic")),
        key32(&field(&row, "linkSignPublic")),
    ) else {
        return Ok(None);
    };
    let expires: i64 = field(&row, "unsealKeyExpires").parse().unwrap_or(0);
    if expires <= moqui_core::user::now_millis() {
        return Ok(None);
    }
    Ok(Some((
        UnsealKey {
            instance_id: instance_id.to_string(),
            unseal_public,
            expires,
            signature: unhex(&field(&row, "unsealKeySignature")).unwrap_or_default(),
        },
        link_public,
    )))
}

/// A holder's share sealed to `instance_id`'s unseal key `unseal_public` (`send#InstanceSealedShare`): kept, as
/// ciphertext, for the instance's next ask, when that key is still the one it offers and in time.
pub fn post_sealed_share(
    facade: &EntityFacade,
    instance_id: &str,
    unseal_public: &[u8],
    sealed: &[u8],
) -> Result<(), LibError> {
    let (user, row) = require_holder(facade, instance_id)?;
    if sealed.is_empty() || sealed.len() > 4096 {
        return Err(fail("a sealed share is 1 to 4096 bytes"));
    }
    let _authz = authz_off();
    let expires: i64 = field(&row, "unsealKeyExpires").parse().unwrap_or(0);
    if field(&row, "unsealKeyPublic") != hex(unseal_public) || expires <= moqui_core::user::now_millis() {
        return Err(fail(format!(
            "AppInstance {instance_id} does not ask for shares with that key now: fetch its unseal key again"
        )));
    }
    let mut kept = facade.make_value(SEALED_SHARE)?;
    kept.set("appInstanceId", Value::string(instance_id));
    kept.set("holderUserId", Value::string(&user));
    kept.set("unsealPublic", Value::string(hex(unseal_public)));
    kept.set("sealedShare", Value::string(hex(sealed)));
    kept.set("postedDate", now());
    kept.create_or_update(facade)?;
    tracing::info!("Instances: {user} sealed a recovery share to AppInstance {instance_id}'s unseal");
    Ok(())
}

/// What the relay holds for `instance_id`: the sealed shares as kept (a test reads it to see that it is ciphertext
/// only).
pub fn relay_rows(facade: &EntityFacade, instance_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let _authz = authz_off();
    rows(facade, SEALED_SHARE, "appInstanceId", instance_id)
}

/// Forget a sealed instance's unseal key and the shares sealed to it (it unsealed, its link ended, or it was revoked);
/// a failure is logged.
pub fn drop_relay(facade: &EntityFacade, instance_id: &str) {
    let _authz = authz_off();
    let dropped = (|| -> Result<(), LibError> {
        for sealed in rows(facade, SEALED_SHARE, "appInstanceId", instance_id)? {
            sealed.delete(facade)?;
        }
        if let Some(mut row) = one(facade, crate::common::APP_INSTANCE, "appInstanceId", instance_id)? {
            if !field(&row, "unsealKeyPublic").is_empty() {
                row.set("unsealKeyPublic", Value::Null);
                row.set("unsealKeySignature", Value::Null);
                row.set("unsealKeyExpires", Value::Null);
                row.update(facade)?;
            }
        }
        Ok(())
    })();
    if let Err(err) = dropped {
        tracing::error!("Instances: the unseal relay of AppInstance {instance_id} was not dropped: {err}");
    }
}

/// The caller holds a share of `instance_id`: a member of its environment's recovery group, signed in. Its user id and
/// the instance's row. An unknown instance and one the caller holds no share of are refused alike.
fn require_holder(facade: &EntityFacade, instance_id: &str) -> Result<(String, EntityValue), LibError> {
    let user = moqui_core::user::current_user_id()
        .ok()
        .flatten()
        .ok_or_else(|| fail("a recovery holder signs in"))?;
    let refused = || {
        fail(format!(
            "{user} holds no recovery share of AppInstance {instance_id} (reasonCode notRecoveryHolder)"
        ))
    };
    let groups = moqui_core::user::user_group_ids(Some(&user)).unwrap_or_default();
    let _authz = authz_off();
    let row = one(facade, crate::common::APP_INSTANCE, "appInstanceId", instance_id)?.ok_or_else(refused)?;
    let group = one(facade, ENVIRONMENT, "environmentId", &field(&row, "environmentId"))?
        .map(|environment| field(&environment, "recoveryUserGroupId"))
        .unwrap_or_default();
    if group.is_empty() || !groups.contains(&group) {
        return Err(refused());
    }
    Ok((user, row))
}

/// The most of a recovery bundle the manager deposits.
const MAX_BUNDLE: usize = 1024 * 1024;

/// Deposit `instance_id`'s recovery bundle (`export-seal`'s text: wrapped values only) as an operational item of its
/// environment's vault, `AppInstance:{id}:sealBundle:{time}`, when it is a bundle and not the one deposited last (a new
/// one after a rekey or rotate-root); its SHA-256 and the date are kept on the instance's row. None when it was
/// deposited already.
pub fn deposit_seal_bundle(facade: &EntityFacade, instance_id: &str, bundle: &str) -> Result<Option<String>, LibError> {
    let _authz = authz_off();
    if bundle.is_empty() || bundle.len() > MAX_BUNDLE {
        return Err(fail(format!("a seal bundle is 1 byte to {MAX_BUNDLE} bytes")));
    }
    let parsed: serde_json::Value =
        serde_json::from_str(bundle).map_err(|_| fail("the seal bundle is not a bundle (not JSON)"))?;
    if parsed["format"] != json!(moqui_core::seal::export::BUNDLE_FORMAT) {
        return Err(fail("the seal bundle is not a moqui-seal-bundle"));
    }
    let hash = hex(&<sha2::Sha256 as sha2::Digest>::digest(bundle.as_bytes()));
    let mut row = instance(facade, instance_id)?;
    if field(&row, "sealBundleHash") == hash {
        return Ok(None);
    }
    let vault_id = one(facade, ENVIRONMENT, "environmentId", &field(&row, "environmentId"))?
        .map(|environment| field(&environment, "vaultId"))
        .filter(|vault| !vault.is_empty())
        .ok_or_else(|| {
            fail(format!(
                "AppInstance {instance_id}'s environment names no operational vault"
            ))
        })?;
    let stamp = moqui_core::user::now_millis();
    let binding = format!("AppInstance:{instance_id}:sealBundle:{stamp}");
    let name = format!("{instance_id} seal bundle");
    let body = zeroize::Zeroizing::new(
        json!({ "v": 1, "type": "note", "name": name, "fields": { "text": bundle } }).to_string(),
    );
    let item_id = new_item_id();
    moqui_vault_server::deposit::deposit_item(
        facade,
        &moqui_vault_server::deposit::Deposit {
            vault_id: &vault_id,
            item_id: &item_id,
            item_type: "note",
            binding: &binding,
            name: &name,
            body: body.as_bytes(),
        },
    )?;
    row.set("sealBundleDate", now());
    row.set("sealBundleHash", Value::string(hash));
    row.update(facade)?;
    tracing::info!("Instances: deposited the seal bundle of AppInstance {instance_id} into vault {vault_id}");
    Ok(Some(item_id))
}

/// The share rows of an instance (its page).
pub fn share_rows(facade: &EntityFacade, instance_id: &str) -> Result<Vec<EntityValue>, LibError> {
    let _authz = authz_off();
    rows(facade, SHARE, "appInstanceId", instance_id)
}
