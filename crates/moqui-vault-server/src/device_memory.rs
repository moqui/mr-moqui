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

//! The devices that remember a user's personal key (plans/archive/Vault-Instances.md §3.5, V5). The desktop and mobile apps keep
//! the open key for a time the user chooses, wrapped by a key made of two halves: one in the device's own key store, and
//! a 32-byte secret of this record, which the server hands the device only while the record is live. So the record
//! ending (taken off the user's list, past its time, the personal key replaced) or a device key the server no longer
//! takes is a memory that cannot open again: the cryptography keeps the rule, not only the device. The server alone
//! opens nothing: it has no part of the device's half or of the wrap.
//!
//! - Only a device's key (the apps' sign-in) records a memory or reads its secret, for the caller's active personal key
//!   (the one the device names), for at most `vault@device-memory-max-hours`.
//! - Each user reads, checks and ends only their own records. The secret is an encrypted field.

use std::collections::BTreeMap;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault::rng::{OsRng, Rng as _};
use moqui_vault::text::hex;
use zeroize::Zeroizing;

use crate::common::{
    authz_off, check_id, fail, field, find, int, int_field, now, now_millis, out, required, text, user, Out, Params,
    DEVICE_MEMORY,
};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, remember_vault_device_memory)?;
    moqui_core::rust_service!(map, check_vault_device_memory)?;
    moqui_core::rust_service!(map, forget_vault_device_memory)?;
    moqui_core::rust_service!(map, list_vault_device_memories)?;
    Ok(())
}

/// The longest a device may remember the key, in hours: `vault@device-memory-max-hours`, 8 when unset, at most 24.
fn max_hours() -> i64 {
    moqui_core::context::conf_env::setting("vault@device-memory-max-hours")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|hours| *hours >= 0)
        .unwrap_or(8)
        .min(24)
}

/// Refuses unless the caller signed in by a device's key (the desktop or mobile app).
fn require_device() -> Result<(), LibError> {
    if moqui_core::user::login_key_kind() != Some(moqui_core::user::LoginKeyKind::Device) {
        return Err(fail(
            "Only the desktop or mobile app remembers the vault key on a device",
        ));
    }
    Ok(())
}

/// The caller's own record `id`.
fn own(facade: &EntityFacade, me: &str, id: &str) -> Result<Option<EntityValue>, LibError> {
    Ok(find(facade, DEVICE_MEMORY)?
        .eq("deviceMemoryId", Value::string(id))
        .one()?
        .filter(|row| field(row, "userId") == me))
}

/// `remember#VaultDeviceMemory`: the calling device remembers the caller's active personal key (`personalKeyId`, which
/// must be it) for `minutes`, at most the setting's; answers when the record ends (`thruStamp`, epoch millis) and the
/// record's secret, the server's half of the memory's key (`memorySecret`, hex). Only a device's key may ask.
fn remember_vault_device_memory(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_device()?;
    let id = required(params, "deviceMemoryId")?;
    check_id("deviceMemoryId", &id)?;
    let device_name = text(params, "deviceName");
    if device_name.chars().count() > 255 {
        return Err(fail("deviceName is too long"));
    }
    let max_minutes = max_hours() * 60;
    if max_minutes == 0 {
        return Err(fail("This server does not let devices remember the vault key"));
    }
    let minutes = int(params, "minutes").unwrap_or(0);
    if minutes < 1 {
        return Err(fail("minutes must be at least 1"));
    }
    let key = crate::personal::my_key(facade, &me)?;
    if required(params, "personalKeyId")? != field(&key, "personalKeyId") {
        return Err(fail("That is not your active personal key"));
    }
    if find(facade, DEVICE_MEMORY)?
        .eq("deviceMemoryId", Value::string(&id))
        .one()?
        .is_some()
    {
        return Err(fail("That device memory id is taken"));
    }
    let thru = now_millis().saturating_add(minutes.min(max_minutes).saturating_mul(60_000));
    let mut secret = Zeroizing::new([0u8; 32]);
    OsRng.fill(&mut secret[..]);
    let secret = Zeroizing::new(hex(&secret[..]));
    let mut row = facade.make_value(DEVICE_MEMORY)?;
    row.set("deviceMemoryId", Value::string(&id));
    row.set("userId", Value::string(&me));
    row.set("personalKeyId", Value::string(field(&key, "personalKeyId")));
    row.set("deviceName", Value::string(device_name));
    row.set("createdDate", now());
    row.set("thruStamp", Value::I64(thru));
    row.set("memorySecret", Value::string(secret.as_str()));
    row.create(facade)?;
    Ok(out([
        ("deviceMemoryId", Value::string(id)),
        ("thruStamp", Value::I64(thru)),
        ("memorySecret", Value::string(secret.as_str())),
    ]))
}

/// Whether a record is still good: before its time, for the user's active personal key.
fn live(facade: &EntityFacade, me: &str, row: &EntityValue) -> Result<bool, LibError> {
    if int_field(row, "thruStamp") <= now_millis() {
        return Ok(false);
    }
    let active = crate::personal::active_key(facade, me)?.map(|key| field(&key, "personalKeyId"));
    Ok(active.is_some_and(|key| key == field(row, "personalKeyId")))
}

/// `check#VaultDeviceMemory`: whether the caller's record `deviceMemoryId` is still good (`live`), and while it is, its
/// secret (`memorySecret`); false and no secret for one ended, past its time, made for a personal key since replaced, or
/// not the caller's. Only a device's key may ask.
fn check_vault_device_memory(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    require_device()?;
    let id = required(params, "deviceMemoryId")?;
    let row = match own(facade, &me, &id)? {
        Some(row) if live(facade, &me, &row)? => row,
        _ => return Ok(out([("live", Value::Bool(false))])),
    };
    let secret = Zeroizing::new(field(&row, "memorySecret"));
    Ok(out([
        ("live", Value::Bool(true)),
        ("thruStamp", Value::I64(int_field(&row, "thruStamp"))),
        ("memorySecret", Value::string(secret.as_str())),
    ]))
}

/// `forget#VaultDeviceMemory`: ends the caller's record (the device forgot the key, or the user took it off the list).
fn forget_vault_device_memory(params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let id = required(params, "deviceMemoryId")?;
    let ended = match own(facade, &me, &id)? {
        Some(row) => {
            row.delete(facade)?;
            true
        }
        None => false,
    };
    Ok(out([("ended", Value::Bool(ended))]))
}

/// `list#VaultDeviceMemories`: the caller's devices that remember their key now, newest first; those past their time
/// are removed on the way.
fn list_vault_device_memories(_params: &Params, facade: &EntityFacade) -> Out {
    let _authz = authz_off();
    let me = user()?;
    let mut rows: Vec<EntityValue> = find(facade, DEVICE_MEMORY)?
        .eq("userId", Value::string(&me))
        .list()?
        .iter()
        .cloned()
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(int_field(row, "thruStamp")));
    let mut listed = Vec::new();
    for row in rows {
        if int_field(&row, "thruStamp") <= now_millis() {
            row.delete(facade)?;
            continue;
        }
        if !live(facade, &me, &row)? {
            continue;
        }
        let mut entry = BTreeMap::new();
        for name in ["deviceMemoryId", "deviceName", "personalKeyId"] {
            entry.insert(name.to_string(), Value::string(field(&row, name)));
        }
        entry.insert(
            "createdDate".to_string(),
            row.get("createdDate").cloned().unwrap_or(Value::Null),
        );
        entry.insert("thruStamp".to_string(), Value::I64(int_field(&row, "thruStamp")));
        listed.push(Value::Map(entry.into_iter().collect()));
    }
    Ok(out([("deviceMemoryList", Value::List(listed))]))
}
