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

//! The browser module's JSON calls (plans/Vault-Instances.md §3.6): one request `{"op": ..., ...}` in, one
//! `{"ok": true, "result": ...}` or `{"ok": false, "error": ...}` out. A [`Session`] holds the opened personal key and
//! the unwrapped vault keys, so they stay in the module's memory: the worker that hosts it sees only handles, public
//! keys, wraps and the one item it was asked for. `lock` clears them. Bytes cross as hex; text as strings.
//!
//! It is compiled for the wasm module and for the crate's tests, which call it natively.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::fingerprint;
use crate::kdf::{Argon2Params, PasswordWrap, MIN_SALT_LEN};
use crate::keys::Key32;
use crate::keyversion::{self, Chain, KeyVersion, Member, Role};
use crate::lease::{self, ApproveInput, Lease, LeaseItem};
use crate::names::ItemLabel;
use crate::personal::{self, PersonalKey, PersonalPublic};
use crate::rng::Rng;
use crate::shamir::{self, Share};
use crate::sign::{Statement, VerifyingKey};
use crate::text::{hex, unhex};
use crate::vault::{self, ItemRef, MemberWrap, Recipient, Signer};
use crate::Error;

/// What the module holds between calls.
#[derive(Default)]
pub struct Session {
    personal: Option<(String, String, PersonalKey)>,
    vault_keys: BTreeMap<(String, u64), Key32>,
}

fn str_of<'a>(args: &'a Value, name: &'static str) -> Result<&'a str, Error> {
    args.get(name).and_then(Value::as_str).ok_or(Error::Format(name))
}

fn bytes_of(args: &Value, name: &'static str) -> Result<Vec<u8>, Error> {
    unhex(str_of(args, name)?)
}

fn u64_of(args: &Value, name: &'static str) -> Result<u64, Error> {
    args.get(name).and_then(Value::as_u64).ok_or(Error::Format(name))
}

fn u8_of(args: &Value, name: &'static str) -> Result<u8, Error> {
    u8::try_from(u64_of(args, name)?).map_err(|_| Error::Format(name))
}

fn u32_of(args: &Value, name: &'static str) -> Result<u32, Error> {
    u32::try_from(u64_of(args, name)?).map_err(|_| Error::Format(name))
}

fn public_of(args: &Value, name: &'static str) -> Result<PersonalPublic, Error> {
    PersonalPublic::from_bytes(&bytes_of(args, name)?)
}

fn verify_of(args: &Value, name: &'static str) -> Result<VerifyingKey, Error> {
    VerifyingKey::from_bytes(&bytes_of(args, name)?)
}

fn label_of(args: &Value) -> Result<Option<ItemLabel<'_>>, Error> {
    match args.get("label") {
        None | Some(Value::Null) => Ok(None),
        Some(label) => Ok(Some(ItemLabel {
            item_type: str_of(label, "item_type")?,
            binding: str_of(label, "binding")?,
            name: str_of(label, "name")?,
        })),
    }
}

fn statement_of(name: &str) -> Result<Statement, Error> {
    let all = [
        Statement::MemberWrap,
        Statement::KeyVersion,
        Statement::DepositKey,
        Statement::RecoveryKey,
        Statement::RecoveryWrap,
        Statement::RecoveryShare,
        Statement::Delivery,
        Statement::Lease,
        Statement::LeaseApproval,
        Statement::KeyCheck,
    ];
    all.into_iter()
        .find(|kind| kind.as_str() == name)
        .ok_or(Error::Format("statement kind"))
}

fn parts_of(args: &Value) -> Result<Vec<Vec<u8>>, Error> {
    let Some(parts) = args.get("parts").and_then(Value::as_array) else {
        return Err(Error::Format("parts"));
    };
    parts
        .iter()
        .map(|part| unhex(part.as_str().ok_or(Error::Format("parts"))?))
        .collect()
}

fn password_wrap_json(wrap: &PasswordWrap) -> Value {
    json!({ "m_kib": wrap.params.m_kib, "t": wrap.params.t, "p": wrap.params.p, "salt": hex(&wrap.salt), "sealed": hex(&wrap.sealed) })
}

fn password_wrap_of(args: &Value, name: &'static str) -> Result<PasswordWrap, Error> {
    let wrap = args.get(name).ok_or(Error::Format(name))?;
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

fn member_wrap_json(wrap: &MemberWrap) -> Value {
    json!({
        "vault_id": wrap.vault_id, "key_version": wrap.key_version,
        "member_user_id": wrap.member_user_id, "member_key_id": wrap.member_key_id,
        "sealed": hex(&wrap.sealed),
        "signer_user_id": wrap.signer_user_id, "signer_key_id": wrap.signer_key_id,
        "signature": hex(&wrap.signature),
    })
}

fn member_wrap_of(args: &Value) -> Result<MemberWrap, Error> {
    Ok(MemberWrap {
        vault_id: str_of(args, "vault_id")?.to_string(),
        key_version: u64_of(args, "key_version")?,
        member_user_id: str_of(args, "member_user_id")?.to_string(),
        member_key_id: str_of(args, "member_key_id")?.to_string(),
        sealed: bytes_of(args, "sealed")?,
        signer_user_id: str_of(args, "signer_user_id")?.to_string(),
        signer_key_id: str_of(args, "signer_key_id")?.to_string(),
        signature: bytes_of(args, "signature")?,
    })
}

fn key_version_of(entry: &Value) -> Result<KeyVersion, Error> {
    let Some(members) = entry.get("members").and_then(Value::as_array) else {
        return Err(Error::Format("members"));
    };
    let members = members
        .iter()
        .map(|m| {
            Ok(Member {
                user_id: str_of(m, "user_id")?.to_string(),
                role: Role::parse(str_of(m, "role")?)?,
                key_id: str_of(m, "key_id")?.to_string(),
                public: public_of(m, "public")?,
            })
        })
        .collect::<Result<Vec<Member>, Error>>()?;
    Ok(KeyVersion {
        vault_id: str_of(entry, "vault_id")?.to_string(),
        version: u64_of(entry, "version")?,
        revision: u64_of(entry, "revision")?,
        members,
        signer_user_id: str_of(entry, "signer_user_id")?.to_string(),
        signer_key_id: str_of(entry, "signer_key_id")?.to_string(),
        signature: bytes_of(entry, "signature")?,
    })
}

fn lease_of(args: &Value) -> Result<Lease, Error> {
    let lease = args.get("lease").ok_or(Error::Format("lease"))?;
    let Some(items) = lease.get("items").and_then(Value::as_array) else {
        return Err(Error::Format("lease items"));
    };
    let items = items
        .iter()
        .map(|item| {
            Ok(LeaseItem {
                item_id: str_of(item, "item_id")?.to_string(),
                item_type: str_of(item, "item_type")?.to_string(),
                binding: str_of(item, "binding")?.to_string(),
                name: str_of(item, "name")?.to_string(),
            })
        })
        .collect::<Result<Vec<LeaseItem>, Error>>()?;
    Ok(Lease {
        lease_id: str_of(lease, "lease_id")?.to_string(),
        vault_id: str_of(lease, "vault_id")?.to_string(),
        key_version: u64_of(lease, "key_version")?,
        items,
        operation: str_of(lease, "operation")?.to_string(),
        cluster_member: str_of(lease, "cluster_member")?.to_string(),
        expires_at: lease
            .get("expires_at")
            .and_then(Value::as_i64)
            .ok_or(Error::Format("expires_at"))?,
        public: crate::hpke::PublicKey::from_bytes(&bytes_of(lease, "public")?)?,
        signature: bytes_of(lease, "signature")?,
    })
}

fn opened_json(id: &str, key: &PersonalKey) -> Value {
    let public = key.public();
    json!({ "personal_key_id": id, "public": hex(&public.to_bytes()), "fingerprint": fingerprint::personal(&public) })
}

impl Session {
    /// Answers one JSON request.
    pub fn call(&mut self, request: &str, rng: &mut dyn Rng) -> String {
        let answer = match serde_json::from_str::<Value>(request) {
            Ok(args) => self.dispatch(&args, rng),
            Err(_) => Err(Error::Format("the request is not JSON")),
        };
        match answer {
            Ok(result) => json!({ "ok": true, "result": result }).to_string(),
            Err(err) => json!({ "ok": false, "error": err.to_string() }).to_string(),
        }
    }

    fn personal(&self) -> Result<&(String, String, PersonalKey), Error> {
        self.personal
            .as_ref()
            .ok_or(Error::Refused("the personal key is locked"))
    }

    fn vault_key(&self, vault_id: &str, key_version: u64) -> Result<&Key32, Error> {
        self.vault_keys
            .get(&(vault_id.to_string(), key_version))
            .ok_or(Error::Refused("the vault is locked"))
    }

    fn dispatch(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        match str_of(args, "op")? {
            "vectors" => crate::vectors::generate(),
            "lock" => {
                // Dropping the keys zeroizes them.
                self.personal = None;
                self.vault_keys.clear();
                Ok(json!({ "locked": true }))
            }
            "status" => {
                let vaults: Vec<Value> = self
                    .vault_keys
                    .keys()
                    .map(|(id, version)| json!({ "vault_id": id, "key_version": version }))
                    .collect();
                Ok(json!({ "personal": self.personal.as_ref().map(|(id, _, _)| id), "vaults": vaults }))
            }
            "personal_make" => self.personal_make(args, rng),
            "personal_open" => self.personal_open(args, rng),
            "personal_open_recovery" => {
                let id = str_of(args, "personal_key_id")?;
                let public = public_of(args, "public")?;
                let key = personal::open_with_recovery_code(
                    &bytes_of(args, "recovery_wrap")?,
                    id,
                    &public,
                    str_of(args, "recovery_code")?,
                )?;
                let answer = opened_json(id, &key);
                self.personal = Some((id.to_string(), str_of(args, "user_id")?.to_string(), key));
                Ok(answer)
            }
            "vault_key_new" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                self.vault_keys
                    .insert((vault_id.to_string(), key_version), vault::new_vault_key(rng));
                Ok(json!({ "vault_id": vault_id, "key_version": key_version }))
            }
            "vault_key_wrap_member" => {
                let (key_id, user_id, me) = self.personal()?;
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let member_public = public_of(args, "member_public")?;
                let to = Recipient {
                    user_id: str_of(args, "member_user_id")?,
                    key_id: str_of(args, "member_key_id")?,
                    public: &member_public,
                };
                let by = Signer {
                    user_id,
                    key_id,
                    key: me,
                };
                let wrap = vault::wrap_vault_key_to_member(
                    self.vault_key(vault_id, key_version)?,
                    vault_id,
                    key_version,
                    to,
                    by,
                    rng,
                )?;
                Ok(member_wrap_json(&wrap))
            }
            "vault_key_open_member" => {
                let wrap = member_wrap_of(args.get("wrap").ok_or(Error::Format("wrap"))?)?;
                let key = vault::open_member_wrap(&wrap, &self.personal()?.2, &verify_of(args, "signer_verify")?)?;
                self.vault_keys.insert((wrap.vault_id.clone(), wrap.key_version), key);
                Ok(json!({ "vault_id": wrap.vault_id, "key_version": wrap.key_version }))
            }
            "vault_key_password_wrap" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let password = str_of(args, "password")?.as_bytes();
                let key = self.vault_key(vault_id, key_version)?;
                let wrap = vault::wrap_vault_key_with_password(key, vault_id, key_version, password, rng)?;
                Ok(password_wrap_json(&wrap))
            }
            "vault_key_password_open" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let wrap = password_wrap_of(args, "wrap")?;
                let password = str_of(args, "password")?.as_bytes();
                let (key, rewrapped) =
                    vault::open_vault_key_with_password(&wrap, vault_id, key_version, password, rng)?;
                self.vault_keys.insert((vault_id.to_string(), key_version), key);
                Ok(
                    json!({ "vault_id": vault_id, "key_version": key_version, "rewrapped": rewrapped.as_ref().map(password_wrap_json) }),
                )
            }
            "chain_verify" => {
                let Some(entries) = args.get("entries").and_then(Value::as_array) else {
                    return Err(Error::Format("entries"));
                };
                let entries = entries
                    .iter()
                    .map(key_version_of)
                    .collect::<Result<Vec<KeyVersion>, Error>>()?;
                let Some(checked) = args.get("checked").and_then(Value::as_array) else {
                    return Err(Error::Format("checked"));
                };
                let checked = checked
                    .iter()
                    .map(|key| VerifyingKey::from_bytes(&unhex(key.as_str().ok_or(Error::Format("checked"))?)?))
                    .collect::<Result<Vec<VerifyingKey>, Error>>()?;
                let chain = Chain::verify(&entries, &checked)?;
                let remembered = match args.get("remembered") {
                    None | Some(Value::Null) => None,
                    Some(r) => Some(keyversion::Remembered {
                        version: u64_of(r, "version")?,
                        revision: u64_of(r, "revision")?,
                        member_list_hash: bytes_of(r, "member_list_hash")?
                            .try_into()
                            .map_err(|_| Error::Format("member_list_hash"))?,
                    }),
                };
                let now = keyversion::check_remembered(chain.latest(), remembered.as_ref())?;
                Ok(
                    json!({ "version": now.version, "revision": now.revision, "member_list_hash": hex(&now.member_list_hash) }),
                )
            }
            "item_seal" => {
                let item = ItemRef {
                    vault_id: str_of(args, "vault_id")?,
                    item_id: str_of(args, "item_id")?,
                    key_version: u64_of(args, "key_version")?,
                    label: label_of(args)?,
                };
                let vault_key = self.vault_key(item.vault_id, item.key_version)?;
                let (item_key, wrapped) = match args.get("wrapped_item_key").and_then(Value::as_str) {
                    Some(wrapped) => {
                        let wrapped = unhex(wrapped)?;
                        (vault::unwrap_item_key(vault_key, &wrapped, item)?, wrapped)
                    }
                    None => {
                        let item_key = vault::new_item_key(rng);
                        let wrapped = vault::wrap_item_key(vault_key, &item_key, item, rng);
                        (item_key, wrapped)
                    }
                };
                let sealed = vault::seal_item_body(&item_key, item, str_of(args, "body")?.as_bytes(), rng)?;
                Ok(json!({ "wrapped_item_key": hex(&wrapped), "sealed_body": hex(&sealed) }))
            }
            "item_open" => {
                let item = ItemRef {
                    vault_id: str_of(args, "vault_id")?,
                    item_id: str_of(args, "item_id")?,
                    key_version: u64_of(args, "key_version")?,
                    label: label_of(args)?,
                };
                let vault_key = self.vault_key(item.vault_id, item.key_version)?;
                let item_key = vault::unwrap_item_key(vault_key, &bytes_of(args, "wrapped_item_key")?, item)?;
                let body = vault::open_item_body(&item_key, item, &bytes_of(args, "sealed_body")?)?;
                let text = std::str::from_utf8(&body).map_err(|_| Error::Format("a body is UTF-8 JSON"))?;
                Ok(json!({ "body": text }))
            }
            "sign" => {
                let parts = parts_of(args)?;
                let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
                let signature = self
                    .personal()?
                    .2
                    .signing_key()
                    .sign(statement_of(str_of(args, "kind")?)?, &refs);
                Ok(json!({ "signature": hex(&signature) }))
            }
            "verify" => {
                let parts = parts_of(args)?;
                let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
                let key = verify_of(args, "verify")?;
                let valid = key
                    .verify(
                        statement_of(str_of(args, "kind")?)?,
                        &refs,
                        &bytes_of(args, "signature")?,
                    )
                    .is_ok();
                Ok(json!({ "valid": valid }))
            }
            "fingerprint" => Ok(json!({ "fingerprint": fingerprint::personal(&public_of(args, "public")?) })),
            "lease_approve" => self.lease_approve(args, rng),
            "shamir_split" => {
                let shares = shamir::split(&bytes_of(args, "secret")?, u8_of(args, "k")?, u8_of(args, "n")?, rng)?;
                let texts: Vec<Value> = shares.iter().map(|s| Value::String(s.to_text().to_string())).collect();
                Ok(json!({ "shares": texts }))
            }
            "shamir_combine" => {
                let Some(texts) = args.get("shares").and_then(Value::as_array) else {
                    return Err(Error::Format("shares"));
                };
                let shares = texts
                    .iter()
                    .map(|t| Share::from_text(t.as_str().ok_or(Error::Format("shares"))?))
                    .collect::<Result<Vec<Share>, Error>>()?;
                Ok(json!({ "secret": hex(&shamir::combine(&shares)?) }))
            }
            _ => Err(Error::Format("unknown op")),
        }
    }

    fn personal_make(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let id = str_of(args, "personal_key_id")?;
        let key = PersonalKey::generate(rng);
        let wrap = personal::wrap_with_passphrase(&key, id, str_of(args, "passphrase")?.as_bytes(), rng)?;
        let code = personal::new_recovery_code(rng);
        let by_code = personal::wrap_with_recovery_code(&key, id, &code, rng)?;
        let mut answer = opened_json(id, &key);
        if let Value::Object(fields) = &mut answer {
            fields.insert("passphrase_wrap".to_string(), password_wrap_json(&wrap));
            fields.insert("recovery_code".to_string(), Value::String(code.to_string()));
            fields.insert("recovery_wrap".to_string(), Value::String(hex(&by_code)));
        }
        self.personal = Some((id.to_string(), str_of(args, "user_id")?.to_string(), key));
        Ok(answer)
    }

    fn personal_open(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let id = str_of(args, "personal_key_id")?;
        let wrap = password_wrap_of(args, "passphrase_wrap")?;
        let public = public_of(args, "public")?;
        let opened = personal::open_with_passphrase(&wrap, id, &public, str_of(args, "passphrase")?.as_bytes(), rng)?;
        let mut answer = opened_json(id, &opened.key);
        if let Value::Object(fields) = &mut answer {
            fields.insert(
                "rewrapped".to_string(),
                opened.rewrapped.as_ref().map_or(Value::Null, password_wrap_json),
            );
        }
        self.personal = Some((id.to_string(), str_of(args, "user_id")?.to_string(), opened.key));
        Ok(answer)
    }

    fn lease_approve(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let lease = lease_of(args)?;
        let lease_key = verify_of(args, "lease_key")?;
        let Some(checked) = args.get("checked").and_then(Value::as_array) else {
            return Err(Error::Format("checked"));
        };
        let checked = checked
            .iter()
            .map(|key| VerifyingKey::from_bytes(&unhex(key.as_str().ok_or(Error::Format("checked"))?)?))
            .collect::<Result<Vec<VerifyingKey>, Error>>()?;
        let Some(Value::Object(wrapped)) = args.get("wrapped_item_keys") else {
            return Err(Error::Format("wrapped_item_keys"));
        };
        let wrapped = wrapped
            .iter()
            .map(|(id, w)| {
                Ok((
                    id.as_str(),
                    unhex(w.as_str().ok_or(Error::Format("wrapped_item_keys"))?)?,
                ))
            })
            .collect::<Result<Vec<(&str, Vec<u8>)>, Error>>()?;
        let wrapped_refs: Vec<(&str, &[u8])> = wrapped.iter().map(|(id, w)| (*id, w.as_slice())).collect();
        let (_, user_id, me) = self.personal()?;
        let approval = lease::approve(
            ApproveInput {
                lease: &lease,
                lease_key: &lease_key,
                checked: &checked,
                vault_key: self.vault_key(&lease.vault_id, lease.key_version)?,
                wrapped_item_keys: &wrapped_refs,
                approver_user_id: user_id,
                approver: me,
            },
            rng,
        )?;
        let sealed: Vec<Value> = approval
            .sealed
            .iter()
            .map(|s| json!({ "item_id": s.item_id, "sealed": hex(&s.sealed) }))
            .collect();
        Ok(json!({
            "lease_id": approval.lease_id,
            "sealed": sealed,
            "approver_user_id": approval.approver_user_id,
            "signature": hex(&approval.signature),
        }))
    }
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
