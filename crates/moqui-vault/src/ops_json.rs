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

//! The JSON shapes of the module's calls: each record of the crate read from and written to a `serde_json::Value`,
//! bytes as hex.

use serde_json::{json, Value};
use zeroize::Zeroize as _;

use crate::delivery::Delivery;
use crate::fingerprint::KeyCheck;
use crate::hpke;
use crate::kdf::{Argon2Params, PasswordWrap, MIN_SALT_LEN};
use crate::keyversion::{KeyVersion, Member, Remembered, Role};
use crate::lease::{Lease, LeaseItem};
use crate::names::ItemLabel;
use crate::personal::PersonalPublic;
use crate::recovery::{HolderShare, RecoveryKeyRecord, RecoveryWrap};
use crate::sign::VerifyingKey;
use crate::text::{hex, unhex};
use crate::vault::{DepositPublic, MemberWrap};
use crate::Error;

pub fn str_of<'a>(args: &'a Value, name: &'static str) -> Result<&'a str, Error> {
    args.get(name).and_then(Value::as_str).ok_or(Error::Format(name))
}

pub fn string_of(args: &Value, name: &'static str) -> Result<String, Error> {
    str_of(args, name).map(str::to_string)
}

pub fn bytes_of(args: &Value, name: &'static str) -> Result<Vec<u8>, Error> {
    unhex(str_of(args, name)?)
}

pub fn u64_of(args: &Value, name: &'static str) -> Result<u64, Error> {
    args.get(name).and_then(Value::as_u64).ok_or(Error::Format(name))
}

pub fn i64_of(args: &Value, name: &'static str) -> Result<i64, Error> {
    args.get(name).and_then(Value::as_i64).ok_or(Error::Format(name))
}

fn u8_of(args: &Value, name: &'static str) -> Result<u8, Error> {
    u8::try_from(u64_of(args, name)?).map_err(|_| Error::Format(name))
}

fn u32_of(args: &Value, name: &'static str) -> Result<u32, Error> {
    u32::try_from(u64_of(args, name)?).map_err(|_| Error::Format(name))
}

pub fn array_of<'a>(args: &'a Value, name: &'static str) -> Result<&'a Vec<Value>, Error> {
    args.get(name).and_then(Value::as_array).ok_or(Error::Format(name))
}

pub fn object_of<'a>(args: &'a Value, name: &'static str) -> Result<&'a Value, Error> {
    args.get(name)
        .filter(|value| value.is_object())
        .ok_or(Error::Format(name))
}

pub fn public_of(args: &Value, name: &'static str) -> Result<PersonalPublic, Error> {
    PersonalPublic::from_bytes(&bytes_of(args, name)?)
}

pub fn verify_of(args: &Value, name: &'static str) -> Result<VerifyingKey, Error> {
    VerifyingKey::from_bytes(&bytes_of(args, name)?)
}

pub fn label_of(args: &Value) -> Result<Option<ItemLabel<'_>>, Error> {
    match args.get("label") {
        None | Some(Value::Null) => Ok(None),
        Some(label) => Ok(Some(ItemLabel {
            item_type: str_of(label, "item_type")?,
            binding: str_of(label, "binding")?,
            name: str_of(label, "name")?,
        })),
    }
}

/// Overwrites every string in `value` with zeros before it is dropped: the request's passphrase, the answer's body.
pub fn scrub(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(scrub),
        Value::Object(fields) => fields.values_mut().for_each(scrub),
        _ => {}
    }
}

/// Counts the bytes a serialization writes, so the answer's buffer is allocated once at its size and never grown
/// (a grown buffer leaves its old copy behind).
struct Counter(usize);

impl std::io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// `value` as JSON text in a buffer of exactly its size.
pub fn to_text(value: &Value) -> String {
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).expect("a Value serializes");
    let mut out = Vec::with_capacity(counter.0);
    serde_json::to_writer(&mut out, value).expect("a Value serializes");
    String::from_utf8(out).expect("serde_json writes UTF-8")
}

pub fn password_wrap_json(wrap: &PasswordWrap) -> Value {
    json!({ "m_kib": wrap.params.m_kib, "t": wrap.params.t, "p": wrap.params.p, "salt": hex(&wrap.salt), "sealed": hex(&wrap.sealed) })
}

pub fn password_wrap_of(args: &Value, name: &'static str) -> Result<PasswordWrap, Error> {
    let wrap = object_of(args, name)?;
    let salt: [u8; MIN_SALT_LEN] = bytes_of(wrap, "salt")?.try_into().map_err(|_| Error::Format("salt"))?;
    Ok(PasswordWrap {
        params: Argon2Params {
            m_kib: u32_of(wrap, "m_kib")?,
            t: u32_of(wrap, "t")?,
            p: u32_of(wrap, "p")?,
        },
        salt,
        sealed: bytes_of(wrap, "sealed")?,
    })
}

pub fn member_wrap_json(wrap: &MemberWrap) -> Value {
    json!({
        "vault_id": wrap.vault_id, "key_version": wrap.key_version,
        "member_user_id": wrap.member_user_id, "member_key_id": wrap.member_key_id,
        "sealed": hex(&wrap.sealed),
        "signer_user_id": wrap.signer_user_id, "signer_key_id": wrap.signer_key_id,
        "signature": hex(&wrap.signature),
    })
}

pub fn member_wrap_of(args: &Value) -> Result<MemberWrap, Error> {
    Ok(MemberWrap {
        vault_id: string_of(args, "vault_id")?,
        key_version: u64_of(args, "key_version")?,
        member_user_id: string_of(args, "member_user_id")?,
        member_key_id: string_of(args, "member_key_id")?,
        sealed: bytes_of(args, "sealed")?,
        signer_user_id: string_of(args, "signer_user_id")?,
        signer_key_id: string_of(args, "signer_key_id")?,
        signature: bytes_of(args, "signature")?,
    })
}

pub fn members_of(args: &Value) -> Result<Vec<Member>, Error> {
    array_of(args, "members")?
        .iter()
        .map(|m| {
            Ok(Member {
                user_id: string_of(m, "user_id")?,
                role: Role::parse(str_of(m, "role")?)?,
                key_id: string_of(m, "key_id")?,
                public: public_of(m, "public")?,
            })
        })
        .collect()
}

pub fn key_version_json(entry: &KeyVersion) -> Value {
    let members: Vec<Value> = entry
        .members
        .iter()
        .map(|m| json!({ "user_id": m.user_id, "role": m.role.as_str(), "key_id": m.key_id, "public": hex(&m.public.to_bytes()) }))
        .collect();
    json!({
        "vault_id": entry.vault_id, "version": entry.version, "revision": entry.revision, "members": members,
        "signer_user_id": entry.signer_user_id, "signer_key_id": entry.signer_key_id, "signature": hex(&entry.signature),
    })
}

pub fn key_version_of(entry: &Value) -> Result<KeyVersion, Error> {
    Ok(KeyVersion {
        vault_id: string_of(entry, "vault_id")?,
        version: u64_of(entry, "version")?,
        revision: u64_of(entry, "revision")?,
        members: members_of(entry)?,
        signer_user_id: string_of(entry, "signer_user_id")?,
        signer_key_id: string_of(entry, "signer_key_id")?,
        signature: bytes_of(entry, "signature")?,
    })
}

pub fn entries_of(args: &Value) -> Result<Vec<KeyVersion>, Error> {
    array_of(args, "entries")?.iter().map(key_version_of).collect()
}

pub fn remembered_of(args: &Value) -> Result<Option<Remembered>, Error> {
    match args.get("remembered") {
        None | Some(Value::Null) => Ok(None),
        Some(r) => Ok(Some(Remembered {
            version: u64_of(r, "version")?,
            revision: u64_of(r, "revision")?,
            member_list_hash: bytes_of(r, "member_list_hash")?
                .try_into()
                .map_err(|_| Error::Format("member_list_hash"))?,
        })),
    }
}

pub fn remembered_json(r: &Remembered) -> Value {
    json!({ "version": r.version, "revision": r.revision, "member_list_hash": hex(&r.member_list_hash) })
}

pub fn key_check_json(check: &KeyCheck) -> Value {
    json!({
        "checker_user_id": check.checker_user_id, "checker_key_id": check.checker_key_id,
        "checked_key_id": check.checked_key_id, "checked_encrypt": hex(&check.checked_encrypt),
        "checked_verify": hex(&check.checked_verify), "checked_at": check.checked_at, "signature": hex(&check.signature),
    })
}

pub fn key_checks_of(args: &Value) -> Result<Vec<KeyCheck>, Error> {
    let Some(rows) = args.get("key_checks") else {
        return Ok(Vec::new());
    };
    let Some(rows) = rows.as_array() else {
        return Err(Error::Format("key_checks"));
    };
    rows.iter()
        .map(|row| {
            Ok(KeyCheck {
                checker_user_id: string_of(row, "checker_user_id")?,
                checker_key_id: string_of(row, "checker_key_id")?,
                checked_key_id: string_of(row, "checked_key_id")?,
                checked_encrypt: bytes_of(row, "checked_encrypt")?,
                checked_verify: bytes_of(row, "checked_verify")?,
                checked_at: i64_of(row, "checked_at")?,
                signature: bytes_of(row, "signature")?,
            })
        })
        .collect()
}

pub fn deposit_json(deposit: &DepositPublic) -> Value {
    json!({
        "vault_id": deposit.vault_id, "key_version": deposit.key_version, "public": hex(deposit.public.as_bytes()),
        "signer_user_id": deposit.signer_user_id, "signer_key_id": deposit.signer_key_id,
        "signature": hex(&deposit.signature),
    })
}

pub fn lease_of(args: &Value) -> Result<Lease, Error> {
    let lease = object_of(args, "lease")?;
    let items = array_of(lease, "items")?
        .iter()
        .map(|item| {
            Ok(LeaseItem {
                item_id: string_of(item, "item_id")?,
                item_type: string_of(item, "item_type")?,
                binding: string_of(item, "binding")?,
                name: string_of(item, "name")?,
            })
        })
        .collect::<Result<Vec<LeaseItem>, Error>>()?;
    Ok(Lease {
        lease_id: string_of(lease, "lease_id")?,
        vault_id: string_of(lease, "vault_id")?,
        key_version: u64_of(lease, "key_version")?,
        items,
        operation: string_of(lease, "operation")?,
        cluster_member: string_of(lease, "cluster_member")?,
        expires_at: i64_of(lease, "expires_at")?,
        public: hpke::PublicKey::from_bytes(&bytes_of(lease, "public")?)?,
        signature: bytes_of(lease, "signature")?,
    })
}

pub fn record_of(args: &Value) -> Result<RecoveryKeyRecord, Error> {
    let record = object_of(args, "record")?;
    Ok(RecoveryKeyRecord {
        recovery_key_id: string_of(record, "recovery_key_id")?,
        public: hpke::PublicKey::from_bytes(&bytes_of(record, "public")?)?,
        k: u8_of(record, "k")?,
        n: u8_of(record, "n")?,
        maker_user_id: string_of(record, "maker_user_id")?,
        maker_key_id: string_of(record, "maker_key_id")?,
        signature: bytes_of(record, "signature")?,
    })
}

pub fn holder_share_of(args: &Value) -> Result<HolderShare, Error> {
    let held = object_of(args, "held")?;
    Ok(HolderShare {
        recovery_key_id: string_of(held, "recovery_key_id")?,
        holder_user_id: string_of(held, "holder_user_id")?,
        holder_key_id: string_of(held, "holder_key_id")?,
        index: u8_of(held, "index")?,
        sealed: bytes_of(held, "sealed")?,
        signature: bytes_of(held, "signature")?,
    })
}

pub fn recovery_wrap_json(wrap: &RecoveryWrap) -> Value {
    json!({
        "vault_id": wrap.vault_id, "key_version": wrap.key_version, "recovery_key_id": wrap.recovery_key_id,
        "sealed": hex(&wrap.sealed), "signer_user_id": wrap.signer_user_id, "signer_key_id": wrap.signer_key_id,
        "signature": hex(&wrap.signature),
    })
}

pub fn recovery_wrap_of(args: &Value) -> Result<RecoveryWrap, Error> {
    let wrap = object_of(args, "wrap")?;
    Ok(RecoveryWrap {
        vault_id: string_of(wrap, "vault_id")?,
        key_version: u64_of(wrap, "key_version")?,
        recovery_key_id: string_of(wrap, "recovery_key_id")?,
        sealed: bytes_of(wrap, "sealed")?,
        signer_user_id: string_of(wrap, "signer_user_id")?,
        signer_key_id: string_of(wrap, "signer_key_id")?,
        signature: bytes_of(wrap, "signature")?,
    })
}

pub fn delivery_json(sent: &Delivery) -> Value {
    json!({
        "delivery_id": sent.delivery_id, "recipient_user_id": sent.recipient_user_id,
        "recipient_key_id": sent.recipient_key_id, "purpose": sent.purpose, "sender_kind": sent.sender_kind,
        "sender_id": sent.sender_id, "sealed": hex(&sent.sealed), "signature": hex(&sent.signature),
    })
}

pub fn delivery_of(row: &Value) -> Result<Delivery, Error> {
    Ok(Delivery {
        delivery_id: string_of(row, "delivery_id")?,
        recipient_user_id: string_of(row, "recipient_user_id")?,
        recipient_key_id: string_of(row, "recipient_key_id")?,
        purpose: string_of(row, "purpose")?,
        sender_kind: string_of(row, "sender_kind")?,
        sender_id: string_of(row, "sender_id")?,
        sealed: bytes_of(row, "sealed")?,
        signature: bytes_of(row, "signature")?,
    })
}
