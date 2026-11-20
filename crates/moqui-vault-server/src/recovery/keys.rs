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

//! The organization's recovery keys and their holders (plans/Vault-Instances.md §3.10): made in a holder's client,
//! kept here as the signed record and one sealed share per holder.

use std::collections::{BTreeMap, BTreeSet};

use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::object::Value;
use moqui_vault::fingerprint;
use moqui_vault::ops::shapes;
use moqui_vault::recovery::HolderShare;
use moqui_vault::text::hex;
use serde_json::{json, Value as Json};

use super::{holder_share_json, maker_public, record_of_row, require_holder_rights, user_may_hold};
use crate::common::{
    authz_off, check_id, fail, field, find, json, now, out, refused, text, user, value, Out, Params, MAX_WRAP_HEX,
    PERSONAL_KEY, RECOVERY_HOLDER, RECOVERY_KEY,
};
use crate::events::{self, Event};
use crate::personal::{active_key, checks_by, my_key, public_of};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, create_vault_recovery_key)?;
    moqui_core::rust_service!(map, list_vault_recovery_keys)?;
    Ok(())
}

/// `create#VaultRecoveryKey`: a recovery key the caller made in their client, as the module doc of
/// [`crate::recovery`] says.
fn create_vault_recovery_key(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_holder_rights()?;
    let mine = my_key(facade, &me)?;
    let record = shapes::record_of(&json!({ "record": json(params, "record")? })).map_err(refused)?;
    let recovery_key_id = record.recovery_key_id.clone();
    check_id("recoveryKeyId", &recovery_key_id)?;
    if super::key_row(facade, &recovery_key_id)?.is_some() {
        return Err(crate::common::id_not_available("Recovery key", &recovery_key_id));
    }
    if record.maker_user_id != me || record.maker_key_id != field(&mine, "personalKeyId") {
        return Err(fail("Sign the recovery key yourself, with your active personal key"));
    }
    let maker = public_of(&mine)?;
    record.verify(&maker.verify).map_err(refused)?;
    let given = json(params, "shares")?;
    let Some(rows) = given.as_array() else {
        return Err(fail("shares is a list"));
    };
    let shares: Vec<HolderShare> = rows
        .iter()
        .map(|row| shapes::holder_share_from(row).map_err(refused))
        .collect::<Result<_, _>>()?;
    if record.k < 2 || record.k > record.n || shares.len() != usize::from(record.n) {
        return Err(fail(
            "A recovery key is split k of n, 2 ≤ k ≤ n, with one share for each of its n holders",
        ));
    }
    let users: BTreeSet<&str> = shares.iter().map(|s| s.holder_user_id.as_str()).collect();
    let indexes: BTreeSet<u8> = shares.iter().map(|s| s.index).collect();
    if users.len() != shares.len() || indexes.len() != shares.len() {
        return Err(fail("Each holder has one share, each share its own index"));
    }
    let checks = checks_by(facade, &field(&mine, "personalKeyId"))?;
    for share in &shares {
        let holder = &share.holder_user_id;
        if share.recovery_key_id != recovery_key_id || 2 * share.sealed.len() > MAX_WRAP_HEX {
            return Err(fail(format!("{holder}'s share is not of this recovery key")));
        }
        let key = active_key(facade, holder)?
            .filter(|key| field(key, "personalKeyId") == share.holder_key_id)
            .ok_or_else(|| fail(format!("{holder}'s share is not wrapped to their active personal key")))?;
        let public = public_of(&key)?;
        share.verify(&public, &maker.verify).map_err(refused)?;
        if !user_may_hold(holder)? {
            return Err(fail(format!("{holder} does not hold the VAULT_RECOVERY permission")));
        }
        if *holder != me {
            fingerprint::require_checked(&checks, &maker.verify, holder, &public)
                .map_err(|_| fail(format!("Check {holder}'s key fingerprint before giving them a share")))?;
        }
    }

    let group = text(params, "userGroupId");
    let instance = text(params, "instanceId");
    let mut row = facade.make_value(RECOVERY_KEY)?;
    row.set("recoveryKeyId", Value::string(&recovery_key_id));
    row.set("publicKey", Value::string(hex(&record.public.to_bytes())));
    row.set("sharesNeeded", Value::I64(i64::from(record.k)));
    row.set("sharesTotal", Value::I64(i64::from(record.n)));
    row.set("makerUserId", Value::string(&me));
    row.set("makerKeyId", Value::string(&record.maker_key_id));
    row.set("signature", Value::string(hex(&record.signature)));
    if !group.is_empty() {
        row.set("userGroupId", Value::string(&group));
    }
    if !instance.is_empty() {
        row.set("instanceId", Value::string(&instance));
    }
    row.set("fromDate", now());
    row.create(facade)?;
    for share in &shares {
        let mut held = facade.make_value(RECOVERY_HOLDER)?;
        held.set("recoveryKeyId", Value::string(&recovery_key_id));
        held.set("userId", Value::string(&share.holder_user_id));
        held.set("personalKeyId", Value::string(&share.holder_key_id));
        held.set("shareIndex", Value::I64(i64::from(share.index)));
        held.set("sealedShare", Value::string(hex(&share.sealed)));
        held.set("signature", Value::string(hex(&share.signature)));
        held.set("fromDate", now());
        held.create(facade)?;
    }
    let holders: Vec<&str> = users.into_iter().collect();
    events::write(
        facade,
        Event {
            kind: "VeRecoveryKeyMade",
            detail: Some(format!(
                "recovery key {recovery_key_id}, {} of {}, holders {}",
                record.k,
                record.n,
                holders.join(", ")
            )),
            ..Event::default()
        },
    )?;
    Ok(out([("recoveryKeyId", Value::string(recovery_key_id))]))
}

/// `list#VaultRecoveryKeys`: the recovery keys that have not ended, each its signed record, its maker's public halves
/// and its holders with their keys' public halves; to a caller with `VAULT_RECOVERY`, also each holder's sealed share
/// (ciphertext to that holder's key, which the recovering client checks a delivered share against).
fn list_vault_recovery_keys(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    user()?;
    let may_hold = super::require_permission().is_ok();
    let mut keys = find(facade, RECOVERY_KEY)?.order_by("fromDate", false);
    let group = text(params, "userGroupId");
    if !group.is_empty() {
        keys = keys.eq("userGroupId", Value::string(group));
    }
    let mut listed = Vec::new();
    for row in keys.list()?.iter().filter(|row| field(row, "thruDate").is_empty()) {
        let recovery_key_id = field(row, "recoveryKeyId");
        // a row that does not read is logged and left out
        let (record, maker) = match record_of_row(row).and_then(|record| Ok((maker_public(facade, &record)?, record))) {
            Ok((maker, record)) => (record, maker),
            Err(err) => {
                tracing::error!("VaultRecoveryKey {recovery_key_id} does not read: {err}");
                continue;
            }
        };
        let holder_rows = find(facade, RECOVERY_HOLDER)?
            .eq("recoveryKeyId", Value::string(&recovery_key_id))
            .order_by("shareIndex", false)
            .list()?;
        let mut holders = Vec::new();
        for held in holder_rows.iter() {
            let key = find(facade, PERSONAL_KEY)?
                .eq("personalKeyId", Value::string(field(held, "personalKeyId")))
                .one()?;
            let public = match key.as_ref().map(public_of).transpose()? {
                Some(public) => hex(&public.to_bytes()),
                None => String::new(),
            };
            let mut holder = json!({
                "userId": field(held, "userId"), "personalKeyId": field(held, "personalKeyId"),
                "shareIndex": crate::common::int_field(held, "shareIndex"), "public": public,
                "keyActive": key.as_ref().is_some_and(|key| field(key, "statusId") == crate::common::ACTIVE),
            });
            if may_hold {
                holder["held"] = holder_share_json(held);
            }
            holders.push(holder);
        }
        listed.push(json!({
            "recoveryKeyId": recovery_key_id, "record": shapes::record_json(&record),
            "makerPublic": hex(&maker.to_bytes()), "userGroupId": field(row, "userGroupId"),
            "instanceId": field(row, "instanceId"), "holders": holders,
        }));
    }
    Ok(out([("recoveryKeyList", value(&Json::Array(listed)))]))
}
