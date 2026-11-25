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

//! Personal keys and key checks (plans/archive/Vault-Instances.md §3.5).
//!
//! - A user has one active personal key: both public halves in the clear, the private halves wrapped by the key
//!   passphrase and by the recovery code, made in the client. The server keeps no hash of either.
//! - The wraps go only to their own user, after a strong sign-in in the last `vault@key-fetch-max-age` seconds (§9,
//!   Nov 19): a password and a second factor, SSO, or a paired device's key, never an app password or a login, API
//!   or scoped key ([`moqui_core::user::strong_recent_auth`]). A new passphrase and a new key need the same.
//! - A key check is stored only when it is signed by its maker's own active key and names a key as it is stored.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::fingerprint::{self, KeyCheck};
use moqui_vault::kdf::PasswordWrap;
use moqui_vault::ops::shapes;
use moqui_vault::personal::PersonalPublic;
use moqui_vault::text::{hex, unhex};
use serde_json::{json, Value as Json};

use crate::common::{
    authz_off, check_hex, check_id, fail, field, find, id_has_history, id_not_available, json, now, out, refused,
    required, text, user, value, Out, Params, ACTIVE, KEY_CHECK, MAX_SMALL_TEXT, MAX_WRAP_HEX, PERSONAL_KEY,
};
use crate::events::{self, Event};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, setup_personal_key)?;
    moqui_core::rust_service!(map, get_personal_key)?;
    moqui_core::rust_service!(map, get_wrapped_personal_key)?;
    moqui_core::rust_service!(map, rewrap_personal_key)?;
    moqui_core::rust_service!(map, replace_personal_key)?;
    moqui_core::rust_service!(map, get_user_public_key)?;
    moqui_core::rust_service!(map, store_key_check)?;
    moqui_core::rust_service!(map, list_key_checks)?;
    Ok(())
}

/// The seconds a strong sign-in counts for a fetch: `vault@key-fetch-max-age`, 300 when unset.
fn key_fetch_max_age_millis() -> i64 {
    moqui_core::context::conf_env::setting("vault@key-fetch-max-age")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|seconds| *seconds >= 0)
        .unwrap_or(300)
        .saturating_mul(1000)
}

/// Refuses unless the session's user signed in strongly and recently, saying what to do: set up a second factor
/// when the account has a password and none (an account without a password signs in by SSO, which is strong), else
/// confirm who they are again.
pub(crate) fn require_strong_sign_in(facade: &EntityFacade, user_id: &str) -> Result<(), LibError> {
    if moqui_core::user::strong_recent_auth(key_fetch_max_age_millis()) {
        return Ok(());
    }
    let has_password = find(facade, "moqui.security.UserAccount")?
        .eq("userId", Value::string(user_id))
        .one()?
        .is_some_and(|account| !field(&account, "currentPassword").is_empty());
    // a device's key made after a password alone is no strong sign-in either: such an account sets up a factor
    let has_factor = moqui_core::user::second_factor_required(user_id).map_err(|err| fail(err.message()))?;
    if has_password && !has_factor {
        return Err(fail(
            "Your vault key is sent only after a sign-in with a password and a second factor, SSO, or a paired device: \
             set up a second factor first (reasonCode strongAuthNeedsFactor)",
        ));
    }
    Err(fail(
        "Confirm it is you with your password and second factor, or sign in again, to open your vault key \
         (reasonCode strongAuthRequired)",
    ))
}

/// A personal key's two public halves from its row.
pub(crate) fn public_of(row: &EntityValue) -> Result<PersonalPublic, LibError> {
    let bytes = unhex(&format!(
        "{}{}",
        field(row, "encryptPublic"),
        field(row, "verifyPublic")
    ))
    .map_err(refused)?;
    PersonalPublic::from_bytes(&bytes).map_err(refused)
}

/// The user's active personal key row.
pub(crate) fn active_key(facade: &EntityFacade, user_id: &str) -> Result<Option<EntityValue>, LibError> {
    find(facade, PERSONAL_KEY)?
        .eq("userId", Value::string(user_id))
        .eq("statusId", Value::string(ACTIVE))
        .one()
}

/// The caller's active personal key, or the error that they have none.
pub(crate) fn my_key(facade: &EntityFacade, user_id: &str) -> Result<EntityValue, LibError> {
    active_key(facade, user_id)?.ok_or_else(|| fail("Set up your personal vault key first"))
}

fn public_param(params: &BTreeMap<String, Value>) -> Result<PersonalPublic, LibError> {
    let bytes = unhex(&required(params, "public")?).map_err(refused)?;
    PersonalPublic::from_bytes(&bytes).map_err(refused)
}

/// The passphrase wrap, refused when its Argon2id parameters are under the floor or over the ceiling.
fn passphrase_wrap_param(params: &BTreeMap<String, Value>) -> Result<(PasswordWrap, Json), LibError> {
    let given = json(params, "passphraseWrap")?;
    if given.to_string().len() > MAX_SMALL_TEXT {
        return Err(fail("passphraseWrap is too long"));
    }
    let wrap = shapes::password_wrap_of(&json!({ "wrap": given }), "wrap").map_err(refused)?;
    wrap.params.check().map_err(refused)?;
    Ok((wrap, given))
}

fn key_json(row: &EntityValue) -> Result<BTreeMap<String, Value>, LibError> {
    let public = public_of(row)?;
    Ok(out([
        ("hasKey", Value::Bool(true)),
        ("userId", Value::string(field(row, "userId"))),
        ("personalKeyId", Value::string(field(row, "personalKeyId"))),
        ("public", Value::string(hex(&public.to_bytes()))),
        ("fingerprint", Value::string(fingerprint::personal(&public))),
        ("fromDate", row.get("fromDate").cloned().unwrap_or(Value::Null)),
    ]))
}

/// Makes the row of a new active personal key.
fn create_key(facade: &EntityFacade, user_id: &str, params: &Params) -> Result<(String, PersonalPublic), LibError> {
    let key_id = required(params, "personalKeyId")?;
    check_id("personalKeyId", &key_id)?;
    let public = public_param(params)?;
    let (_, wrap) = passphrase_wrap_param(params)?;
    let recovery = required(params, "recoveryWrap")?;
    check_hex("recoveryWrap", &recovery, MAX_WRAP_HEX)?;
    let taken = find(facade, PERSONAL_KEY)?
        .eq("personalKeyId", Value::string(&key_id))
        .one()?
        .is_some();
    if taken || id_has_history(facade, "personalKeyId", &key_id)? {
        return Err(id_not_available("Personal key", &key_id));
    }
    let mut row = facade.make_value(PERSONAL_KEY)?;
    row.set("personalKeyId", Value::string(&key_id));
    row.set("userId", Value::string(user_id));
    row.set("encryptPublic", Value::string(hex(public.encrypt.as_bytes())));
    row.set("verifyPublic", Value::string(hex(public.verify.as_bytes())));
    row.set("passphraseWrap", Value::string(wrap.to_string()));
    row.set("recoveryWrap", Value::string(recovery));
    row.set("statusId", Value::string(ACTIVE));
    row.set("fromDate", now());
    row.create(facade)?;
    Ok((key_id, public))
}

/// `setup#PersonalKey`: the caller's first personal key, after a strong, recent sign-in (so a weak session or a key
/// cannot plant a key for a user who has none). Refused when they have one (replace#PersonalKey).
fn setup_personal_key(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_strong_sign_in(facade, &me)?;
    if active_key(facade, &me)?.is_some() {
        return Err(fail(
            "You have a personal vault key already: replace it to make a new one",
        ));
    }
    let (key_id, public) = create_key(facade, &me, params)?;
    events::write(
        facade,
        Event {
            kind: "VePersonalKeyMade",
            personal_key_id: Some(&key_id),
            ..Event::default()
        },
    )?;
    Ok(out([
        ("personalKeyId", Value::string(key_id)),
        ("fingerprint", Value::string(fingerprint::personal(&public))),
    ]))
}

/// `get#PersonalKey`: the caller's active key, public halves and fingerprint only, and whether a fetch of its wrap
/// would be taken now.
fn get_personal_key(_params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let mut answer = match active_key(facade, &me)? {
        Some(row) => key_json(&row)?,
        None => out([("hasKey", Value::Bool(false)), ("userId", Value::string(&me))]),
    };
    answer.insert(
        "fetchAllowed".to_string(),
        Value::Bool(moqui_core::user::strong_recent_auth(key_fetch_max_age_millis())),
    );
    Ok(answer)
}

/// `get#WrappedPersonalKey`: the caller's own wrapped private key, after a strong, recent sign-in; an event.
fn get_wrapped_personal_key(_params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_strong_sign_in(facade, &me)?;
    let row = my_key(facade, &me)?;
    let key_id = field(&row, "personalKeyId");
    events::write(
        facade,
        Event {
            kind: "VeKeyFetched",
            personal_key_id: Some(&key_id),
            ..Event::default()
        },
    )?;
    let wrap: Json = serde_json::from_str(&field(&row, "passphraseWrap")).unwrap_or(Json::Null);
    let mut answer = key_json(&row)?;
    answer.insert("passphraseWrap".to_string(), value(&wrap));
    answer.insert("recoveryWrap".to_string(), Value::string(field(&row, "recoveryWrap")));
    Ok(answer)
}

/// `rewrap#PersonalKey`: a new passphrase wrap of the caller's active key (one value rewrapped), after a strong,
/// recent sign-in.
fn rewrap_personal_key(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_strong_sign_in(facade, &me)?;
    let mut row = my_key(facade, &me)?;
    let key_id = field(&row, "personalKeyId");
    if required(params, "personalKeyId")? != key_id {
        return Err(fail("That is not your active personal key"));
    }
    let (_, wrap) = passphrase_wrap_param(params)?;
    row.set("passphraseWrap", Value::string(wrap.to_string()));
    let recovery = text(params, "recoveryWrap");
    if !recovery.is_empty() {
        check_hex("recoveryWrap", &recovery, MAX_WRAP_HEX)?;
        row.set("recoveryWrap", Value::string(recovery));
    }
    row.update(facade)?;
    events::write(
        facade,
        Event {
            kind: "VePassphraseChanged",
            personal_key_id: Some(&key_id),
            ..Event::default()
        },
    )?;
    Ok(out([("personalKeyId", Value::string(key_id))]))
}

/// `replace#PersonalKey`: a new personal key in place of the active one (lost passphrase and recovery code), after a
/// strong, recent sign-in. The old key is marked replaced; nothing is wrapped to the new key until its fingerprint is
/// checked again (add#VaultMember).
fn replace_personal_key(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_strong_sign_in(facade, &me)?;
    let old = active_key(facade, &me)?;
    let replaced = old.as_ref().map(|row| field(row, "personalKeyId"));
    if let Some(mut row) = old {
        row.set("statusId", Value::string("VpkReplaced"));
        row.set("thruDate", now());
        row.update(facade)?;
    }
    let (key_id, public) = create_key(facade, &me, params)?;
    events::write(
        facade,
        Event {
            kind: "VePersonalKeyReplaced",
            personal_key_id: Some(&key_id),
            detail: replaced.map(|old| format!("replaces {old}")),
            ..Event::default()
        },
    )?;
    Ok(out([
        ("personalKeyId", Value::string(key_id)),
        ("fingerprint", Value::string(fingerprint::personal(&public))),
    ]))
}

/// `get#UserPublicKey`: another user's active personal key (public halves and fingerprint), to add them as a member
/// or send them a delivery once its fingerprint is compared.
fn get_user_public_key(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    user()?;
    let mut user_id = text(params, "userId");
    if user_id.is_empty() {
        let username = required(params, "username")?;
        let account = find(facade, "moqui.security.UserAccount")?
            .eq("username", Value::string(&username))
            .one()?;
        user_id = account.map(|row| field(&row, "userId")).unwrap_or_default();
    }
    let mut answer = match active_key(facade, &user_id)? {
        Some(row) => key_json(&row)?,
        None => out([("hasKey", Value::Bool(false))]),
    };
    answer.insert("userId".to_string(), Value::string(user_id));
    Ok(answer)
}

/// A key check from its JSON shape.
pub(crate) fn key_check_of(given: &Json) -> Result<KeyCheck, LibError> {
    let mut checks = shapes::key_checks_of(&json!({ "key_checks": [given] })).map_err(refused)?;
    checks.pop().ok_or_else(|| fail("check is required"))
}

pub(crate) fn key_check_json(row: &EntityValue) -> Json {
    let mut shaped = json!({
        "checker_user_id": field(row, "checkerUserId"), "checker_key_id": field(row, "checkerKeyId"),
        "checked_key_id": field(row, "checkedKeyId"), "checked_user_id": field(row, "checkedUserId"),
        "checked_encrypt": field(row, "checkedEncrypt"),
        "checked_verify": field(row, "checkedVerify"),
        "checked_at": crate::common::int_field(row, "checkedAt"), "signature": field(row, "signature"),
    });
    shaped["keyCheckId"] = json!(field(row, "keyCheckId"));
    shaped
}

/// `store#KeyCheck`: a check the caller made after comparing a fingerprint, signed by their own active key; the key it
/// names must be as stored (a personal key's two halves). A later check of the same key by the same key replaces it.
fn store_key_check(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let mine = my_key(facade, &me)?;
    let check = key_check_of(&json(params, "check")?)?;
    if check.checker_user_id != me || check.checker_key_id != field(&mine, "personalKeyId") {
        return Err(fail("A key check is stored only by its maker, with their active key"));
    }
    check.checked_verify_key(&public_of(&mine)?.verify).map_err(refused)?;
    if check.checked_key_id == "lease-key" {
        if !check.checked_user_id.is_empty() {
            return Err(fail("A check of the lease key names no user"));
        }
    } else {
        let checked = find(facade, PERSONAL_KEY)?
            .eq("personalKeyId", Value::string(&check.checked_key_id))
            .one()?
            .ok_or_else(|| fail(format!("No personal key {}", check.checked_key_id)))?;
        if field(&checked, "userId") != check.checked_user_id {
            return Err(fail("The check names another user than the key's"));
        }
        let public = public_of(&checked)?;
        check
            .covers_personal(&public_of(&mine)?.verify, &public)
            .map_err(refused)?;
    }
    let earlier = find(facade, KEY_CHECK)?
        .eq("checkerKeyId", Value::string(&check.checker_key_id))
        .eq("checkedKeyId", Value::string(&check.checked_key_id))
        .list()?;
    for row in earlier.iter() {
        row.delete(facade)?;
    }
    let mut row = facade.make_value(KEY_CHECK)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("checkerUserId", Value::string(&check.checker_user_id));
    row.set("checkerKeyId", Value::string(&check.checker_key_id));
    row.set("checkedKeyId", Value::string(&check.checked_key_id));
    row.set("checkedUserId", Value::string(&check.checked_user_id));
    row.set("checkedEncrypt", Value::string(hex(&check.checked_encrypt)));
    row.set("checkedVerify", Value::string(hex(&check.checked_verify)));
    row.set("checkedAt", Value::I64(check.checked_at));
    row.set("signature", Value::string(hex(&check.signature)));
    row.set("createdDate", now());
    row.create(facade)?;
    Ok(out([("keyCheckId", Value::string(field(&row, "keyCheckId")))]))
}

/// `list#KeyChecks`: the caller's own key checks, in the vault crate's shape for a client to verify and use.
fn list_key_checks(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let mut found = find(facade, KEY_CHECK)?.eq("checkerUserId", Value::string(&me));
    let checked = text(params, "checkedKeyId");
    if !checked.is_empty() {
        found = found.eq("checkedKeyId", Value::string(checked));
    }
    let rows: Vec<Json> = found.list()?.iter().map(key_check_json).collect();
    Ok(out([("keyCheckList", value(&Json::Array(rows)))]))
}

/// The key checks a user's active key signed, as the vault crate reads them: the server's own check that a member's
/// key was checked by the signer before anything is wrapped to it.
pub(crate) fn checks_by(facade: &EntityFacade, checker_key_id: &str) -> Result<Vec<KeyCheck>, LibError> {
    let rows = find(facade, KEY_CHECK)?
        .eq("checkerKeyId", Value::string(checker_key_id))
        .list()?;
    // a row that does not read is logged and left out: one check fewer refuses more, never less
    let mut checks = Vec::with_capacity(rows.len());
    // a lease-key check vouches for no personal key
    for row in rows.iter().filter(|row| field(row, "checkedKeyId") != "lease-key") {
        match key_check_of(&key_check_json(row)) {
            Ok(check) => checks.push(check),
            Err(err) => tracing::error!("VaultKeyCheck {} does not read: {err}", field(row, "keyCheckId")),
        }
    }
    Ok(checks)
}
