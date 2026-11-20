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
//! `{"ok": true, "result": ...}` or `{"ok": false, "error": ...}` out. Bytes cross as hex.
//!
//! A [`Session`] holds the opened personal key, the unwrapped vault keys and a combined recovery key, so they stay in
//! the module's memory; the worker that hosts it sees public keys, wraps and the one item it asked for. The module
//! decides what it trusts by itself, never by the page's say-so:
//!
//! - a trust anchor or a key to wrap to is only a key named by a `VaultKeyCheck` row that this session's own personal
//!   key signed ([`KeyCheck`]), or the session's own key;
//! - a vault key opens only through the member-list chain (verified, checked against what the client remembers, and
//!   the wrap accepted by it), never with a signer key the page names;
//! - each signed statement is built here from its parts (a key version, a member wrap, a deposit key, a key check
//!   after the fingerprint the user compared, a recovery wrap, a lease approval); there is no "sign these bytes";
//! - a recovery key is combined here and used here: no secret share or key goes back to the page.
//!
//! The request's strings are overwritten after the call and the answer is built in a buffer of its exact size.
//! It is compiled for the wasm module, natively under the feature `json` (the `moqui vault` command and the test
//! client's `vault` member), and for the crate's tests. [`shapes`] reads and writes the records' JSON, which the
//! server's vault services take and answer in the same form.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::delivery::{self, Delivery, Filed, Sender, To};
use crate::fingerprint::{self, KeyCheck};
use crate::hpke;
use crate::keys::Key32;
use crate::keyversion::{self, Chain, KeyVersion, Member};
use crate::lease::{self, ApproveInput};
use crate::personal::{self, PersonalKey, PersonalPublic};
use crate::recovery;
use crate::rng::Rng;
use crate::sign::VerifyingKey;
use crate::text::{hex, unhex};
use crate::vault::{self, ItemRef, Recipient, RotateInput, Signer};
use crate::Error;

#[path = "ops_json.rs"]
pub mod shapes;

use shapes::{
    array_of, bytes_of, delivery_json, delivery_of, deposit_json, entries_of, holder_share_of, i64_of, key_check_json,
    key_checks_of, key_version_json, label_of, lease_of, member_wrap_json, member_wrap_of, members_of, object_of,
    password_wrap_json, password_wrap_of, public_of, record_of, recovery_wrap_json, recovery_wrap_of, remembered_json,
    remembered_of, scrub, str_of, to_text, u64_of, verify_of,
};

/// The opened personal key and who it is.
struct Me {
    key_id: String,
    user_id: String,
    key: PersonalKey,
}

/// What the module holds between calls.
#[derive(Default)]
pub struct Session {
    me: Option<Me>,
    vault_keys: BTreeMap<(String, u64), Key32>,
    recovery_keys: BTreeMap<String, hpke::SecretKey>,
    /// The deliveries opened in this session: a repeat is refused, never filed twice.
    filed: Filed,
}

fn opened_json(id: &str, key: &PersonalKey) -> Value {
    let public = key.public();
    json!({ "personal_key_id": id, "public": hex(&public.to_bytes()), "fingerprint": fingerprint::personal(&public) })
}

impl Session {
    /// Answers one JSON request.
    pub fn call(&mut self, request: &str, rng: &mut dyn Rng) -> String {
        let answer = match serde_json::from_str::<Value>(request) {
            Ok(mut args) => {
                let answer = self.dispatch(&args, rng);
                scrub(&mut args);
                answer
            }
            Err(_) => Err(Error::Format("the request is not JSON")),
        };
        let mut answer = match answer {
            Ok(result) => json!({ "ok": true, "result": result }),
            Err(err) => json!({ "ok": false, "error": err.to_string() }),
        };
        let text = to_text(&answer);
        scrub(&mut answer);
        text
    }

    fn me(&self) -> Result<&Me, Error> {
        self.me.as_ref().ok_or(Error::Refused("the personal key is locked"))
    }

    fn signer(&self) -> Result<Signer<'_>, Error> {
        let me = self.me()?;
        Ok(Signer {
            user_id: &me.user_id,
            key_id: &me.key_id,
            key: &me.key,
        })
    }

    fn vault_key(&self, vault_id: &str, key_version: u64) -> Result<&Key32, Error> {
        self.vault_keys
            .get(&(vault_id.to_string(), key_version))
            .ok_or(Error::Refused("the vault is locked"))
    }

    /// Keeps a vault key; one already open for that vault and version is never replaced.
    fn keep_vault_key(&mut self, vault_id: &str, key_version: u64, key: Key32) -> Result<(), Error> {
        let slot = (vault_id.to_string(), key_version);
        if self.vault_keys.contains_key(&slot) {
            return Err(Error::Refused("that vault key version is already open"));
        }
        self.vault_keys.insert(slot, key);
        Ok(())
    }

    /// The session's key checks among the rows the page passed: those this personal key signed.
    fn key_checks(&self, args: &Value) -> Result<Vec<KeyCheck>, Error> {
        let mine = self.me()?.key.public().verify;
        Ok(key_checks_of(args)?
            .into_iter()
            .filter(|check| check.checked_verify_key(&mine).is_ok())
            .collect())
    }

    /// Trust anchors: this session's own signing key and every key a verified check names.
    fn anchors(&self, args: &Value) -> Result<Vec<VerifyingKey>, Error> {
        let mine = self.me()?.key.public().verify;
        let mut anchors = vec![mine];
        anchors.extend(
            self.key_checks(args)?
                .iter()
                .filter_map(|check| check.checked_verify_key(&mine).ok()),
        );
        Ok(anchors)
    }

    /// Refuses a personal key that is neither this session's own nor covered by a verified check of it as
    /// `user_id`'s key.
    fn require_checked(&self, args: &Value, user_id: &str, public: &PersonalPublic) -> Result<(), Error> {
        let me = self.me()?;
        if *public == me.key.public() && user_id == me.user_id {
            return Ok(());
        }
        fingerprint::require_checked(&self.key_checks(args)?, &me.key.public().verify, user_id, public)
    }

    fn dispatch(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        match str_of(args, "op")? {
            #[cfg(any(test, feature = "test-vectors"))]
            "vectors" => crate::vectors::generate(),
            "lock" => {
                // Dropping the keys zeroizes them.
                self.me = None;
                self.vault_keys.clear();
                self.recovery_keys.clear();
                Ok(json!({ "locked": true }))
            }
            "status" => {
                let vaults: Vec<Value> = self
                    .vault_keys
                    .keys()
                    .map(|(id, version)| json!({ "vault_id": id, "key_version": version }))
                    .collect();
                Ok(json!({ "personal": self.me.as_ref().map(|me| &me.key_id), "vaults": vaults }))
            }
            "personal_make" => self.personal_make(args, rng),
            "personal_open" => self.personal_open(args, rng),
            "personal_open_recovery" => self.personal_open_recovery(args),
            "personal_rewrap" => {
                // a new key passphrase for the open key: one value rewrapped (§3.5)
                let me = self.me()?;
                let passphrase = str_of(args, "passphrase")?.as_bytes();
                let wrap = personal::wrap_with_passphrase(&me.key, &me.key_id, passphrase, rng)?;
                Ok(json!({ "personal_key_id": me.key_id, "passphrase_wrap": password_wrap_json(&wrap) }))
            }
            "fingerprint" => Ok(json!({ "fingerprint": fingerprint::personal(&public_of(args, "public")?) })),
            "key_check_make" => self.key_check_make(args),
            "verify" => {
                let parts: Vec<Vec<u8>> = array_of(args, "parts")?
                    .iter()
                    .map(|part| unhex(part.as_str().ok_or(Error::Format("parts"))?))
                    .collect::<Result<_, _>>()?;
                let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
                let kind = statement_of(str_of(args, "kind")?)?;
                let valid = verify_of(args, "verify")?
                    .verify(kind, &refs, &bytes_of(args, "signature")?)
                    .is_ok();
                Ok(json!({ "valid": valid }))
            }
            "chain_verify" => {
                let entries = entries_of(args)?;
                let chain = Chain::verify(&entries, &self.anchors(args)?)?;
                Ok(remembered_json(&keyversion::check_remembered(
                    &chain,
                    remembered_of(args)?.as_ref(),
                )?))
            }
            "key_version_first" => {
                let members = members_of(args)?;
                for member in &members {
                    self.require_checked(args, &member.user_id, &member.public)?;
                }
                let me = self.me()?;
                let entry = KeyVersion::first(str_of(args, "vault_id")?, members, &me.user_id, &me.key_id, &me.key)?;
                Ok(key_version_json(&entry))
            }
            "key_version_next" => {
                let entries = entries_of(args)?;
                let chain = Chain::verify(&entries, &self.anchors(args)?)?;
                keyversion::check_remembered(&chain, remembered_of(args)?.as_ref())?;
                let members = self.new_members_checked(args, chain.latest())?;
                let me = self.me()?;
                let entry = chain.next(
                    u64_of(args, "version")?,
                    u64_of(args, "revision")?,
                    members,
                    &me.user_id,
                    &me.key_id,
                    &me.key,
                )?;
                Ok(key_version_json(&entry))
            }
            "vault_key_new" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                self.keep_vault_key(vault_id, key_version, vault::new_vault_key(rng))?;
                Ok(json!({ "vault_id": vault_id, "key_version": key_version }))
            }
            "vault_key_wrap_member" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let member_public = public_of(args, "member_public")?;
                self.require_checked(args, str_of(args, "member_user_id")?, &member_public)?;
                let to = Recipient {
                    user_id: str_of(args, "member_user_id")?,
                    key_id: str_of(args, "member_key_id")?,
                    public: &member_public,
                };
                let key = self.vault_key(vault_id, key_version)?;
                let wrap = vault::wrap_vault_key_to_member(key, vault_id, key_version, to, self.signer()?, rng)?;
                Ok(member_wrap_json(&wrap))
            }
            "vault_key_open" => self.vault_key_open(args),
            "vault_key_password_wrap" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let key = self.vault_key(vault_id, key_version)?;
                let password = str_of(args, "password")?.as_bytes();
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
                self.keep_vault_key(vault_id, key_version, key)?;
                Ok(json!({
                    "vault_id": vault_id, "key_version": key_version,
                    "rewrapped": rewrapped.as_ref().map(password_wrap_json),
                }))
            }
            "vault_rotate" => self.vault_rotate(args, rng),
            "deposit_key_new" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let key = self.vault_key(vault_id, key_version)?;
                let (public, wrapped) = vault::new_deposit_key(key, vault_id, key_version, self.signer()?, rng)?;
                Ok(json!({ "deposit": deposit_json(&public), "wrapped_private": hex(&wrapped) }))
            }
            // an item the server deposited, taken in: its key unwrapped with the deposit key's private half (which only
            // the vault key opens) and wrapped by the vault key, so a lease can name it (§3.2)
            "deposit_adopt" => {
                let item = ItemRef {
                    vault_id: str_of(args, "vault_id")?,
                    item_id: str_of(args, "item_id")?,
                    key_version: u64_of(args, "key_version")?,
                    label: label_of(args)?,
                };
                let vault_key = self.vault_key(item.vault_id, item.key_version)?;
                let deposit = vault::open_deposit_key(
                    vault_key,
                    item.vault_id,
                    item.key_version,
                    &bytes_of(args, "deposit_wrapped_private")?,
                )?;
                let item_key = vault::unwrap_deposited_item_key(&deposit, &bytes_of(args, "wrapped_item_key")?, item)?;
                let wrapped = vault::wrap_item_key(vault_key, &item_key, item, rng);
                Ok(json!({ "wrapped_item_key": hex(&wrapped) }))
            }
            "item_seal" => self.item_seal(args, rng),
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
            "lease_approve" => self.lease_approve(args, rng),
            "recovery_wrap" => {
                let record = record_of(args)?;
                let maker = public_of(args, "maker_public")?;
                self.require_checked(args, &record.maker_user_id, &maker)?;
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let key = self.vault_key(vault_id, key_version)?;
                let wrap =
                    recovery::wrap_vault_key(key, vault_id, key_version, &record, &maker.verify, self.signer()?, rng)?;
                Ok(recovery_wrap_json(&wrap))
            }
            "recovery_share_deliver" => {
                let held = holder_share_of(args)?;
                let maker = public_of(args, "maker_public")?;
                let to_public = public_of(args, "to_public")?;
                self.require_checked(args, str_of(args, "to_user_id")?, &to_public)?;
                let to = To {
                    user_id: str_of(args, "to_user_id")?,
                    key_id: str_of(args, "to_key_id")?,
                    public: &to_public,
                };
                let me = self.me()?;
                let delivery_id = str_of(args, "delivery_id")?;
                let sent = recovery::deliver_share(&held, &me.key, &me.user_id, &maker.verify, delivery_id, to, rng)?;
                Ok(delivery_json(&sent))
            }
            "recovery_combine" => {
                let record = record_of(args)?;
                let maker = public_of(args, "maker_public")?;
                self.require_checked(args, &record.maker_user_id, &maker)?;
                let deliveries = array_of(args, "deliveries")?
                    .iter()
                    .map(|row| Ok((delivery_of(row)?, verify_of(row, "sender_verify")?)))
                    .collect::<Result<Vec<(Delivery, VerifyingKey)>, Error>>()?;
                let key = recovery::combine_deliveries(&record, &maker.verify, &deliveries, &self.me()?.key)?;
                self.recovery_keys.insert(record.recovery_key_id.clone(), key);
                Ok(json!({ "recovery_key_id": record.recovery_key_id }))
            }
            "recovery_open_vault" => self.recovery_open_vault(args),
            "delivery_send" => self.delivery_send(args, rng),
            "delivery_open" => self.delivery_open(args),
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
        answer["passphrase_wrap"] = password_wrap_json(&wrap);
        answer["recovery_code"] = Value::String(code.to_string());
        answer["recovery_wrap"] = Value::String(hex(&by_code));
        self.me = Some(Me {
            key_id: id.to_string(),
            user_id: str_of(args, "user_id")?.to_string(),
            key,
        });
        Ok(answer)
    }

    fn personal_open(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let id = str_of(args, "personal_key_id")?;
        let wrap = password_wrap_of(args, "passphrase_wrap")?;
        let public = public_of(args, "public")?;
        let opened = personal::open_with_passphrase(&wrap, id, &public, str_of(args, "passphrase")?.as_bytes(), rng)?;
        let mut answer = opened_json(id, &opened.key);
        answer["rewrapped"] = opened.rewrapped.as_ref().map_or(Value::Null, password_wrap_json);
        self.me = Some(Me {
            key_id: id.to_string(),
            user_id: str_of(args, "user_id")?.to_string(),
            key: opened.key,
        });
        Ok(answer)
    }

    fn personal_open_recovery(&mut self, args: &Value) -> Result<Value, Error> {
        let id = str_of(args, "personal_key_id")?;
        let public = public_of(args, "public")?;
        let sealed = bytes_of(args, "recovery_wrap")?;
        let key = personal::open_with_recovery_code(&sealed, id, &public, str_of(args, "recovery_code")?)?;
        let answer = opened_json(id, &key);
        self.me = Some(Me {
            key_id: id.to_string(),
            user_id: str_of(args, "user_id")?.to_string(),
            key,
        });
        Ok(answer)
    }

    /// A key check, made only when the page passes the fingerprint the user compared and it is this key's.
    fn key_check_make(&self, args: &Value) -> Result<Value, Error> {
        let me = self.me()?;
        let compared = str_of(args, "fingerprint")?;
        let checked_at = i64_of(args, "checked_at")?;
        let check = if args.get("lease_key").is_some() {
            let lease_key = verify_of(args, "lease_key")?;
            if fingerprint::lease_key(&lease_key) != compared {
                return Err(Error::Refused("the fingerprint compared is not this lease key's"));
            }
            KeyCheck::of_lease_key(&me.user_id, &me.key_id, &me.key, &lease_key, checked_at)
        } else {
            let public = public_of(args, "checked_public")?;
            if fingerprint::personal(&public) != compared {
                return Err(Error::Refused("the fingerprint compared is not this key's"));
            }
            let checked_key_id = str_of(args, "checked_key_id")?;
            let checked_user_id = str_of(args, "checked_user_id")?;
            if checked_user_id.is_empty() {
                return Err(Error::Format("checked_user_id"));
            }
            KeyCheck::of_personal(
                &me.user_id,
                &me.key_id,
                &me.key,
                checked_user_id,
                checked_key_id,
                &public,
                checked_at,
            )
        };
        Ok(key_check_json(&check))
    }

    /// The members of a next entry, each one not already in the latest entry with the same key checked first.
    fn new_members_checked(&self, args: &Value, latest: &KeyVersion) -> Result<Vec<Member>, Error> {
        let members = members_of(args)?;
        for member in &members {
            let listed = latest
                .member(&member.user_id, &member.key_id)
                .is_some_and(|m| m.public == member.public);
            if !listed {
                self.require_checked(args, &member.user_id, &member.public)?;
            }
        }
        Ok(members)
    }

    /// Opens this member's wrap of a vault key, only through the vault's member-list chain.
    fn vault_key_open(&mut self, args: &Value) -> Result<Value, Error> {
        let entries = entries_of(args)?;
        let chain = Chain::verify(&entries, &self.anchors(args)?)?;
        let remembered = keyversion::check_remembered(&chain, remembered_of(args)?.as_ref())?;
        let wrap = member_wrap_of(object_of(args, "wrap")?)?;
        let me = self.me()?;
        if wrap.member_user_id != me.user_id || wrap.member_key_id != me.key_id {
            return Err(Error::Refused("the wrap is for someone else"));
        }
        let (_, signer) = chain.accept_wrap(&wrap)?;
        let key = vault::open_member_wrap(&wrap, &me.key, &signer)?;
        self.keep_vault_key(&wrap.vault_id, wrap.key_version, key)?;
        Ok(
            json!({ "vault_id": wrap.vault_id, "key_version": wrap.key_version, "remembered": remembered_json(&remembered) }),
        )
    }

    /// Rotates the open vault key to the next version (§3.9, Remove): signs the next entry, rewraps the item keys,
    /// wraps to each remaining member (each key checked) and, for an operational vault, makes a new deposit key.
    fn vault_rotate(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let entries = entries_of(args)?;
        let chain = Chain::verify(&entries, &self.anchors(args)?)?;
        keyversion::check_remembered(&chain, remembered_of(args)?.as_ref())?;
        let latest = chain.latest();
        let vault_id = latest.vault_id.clone();
        let old_version = latest.version;
        let members = members_of(args)?;
        for member in &members {
            self.require_checked(args, &member.user_id, &member.public)?;
        }
        let item_rows = array_of(args, "items")?;
        let mut wrapped = Vec::with_capacity(item_rows.len());
        for row in item_rows {
            wrapped.push(bytes_of(row, "wrapped_item_key")?);
        }
        let mut items = Vec::with_capacity(item_rows.len());
        for (row, wrapped) in item_rows.iter().zip(wrapped.iter()) {
            let item = ItemRef {
                vault_id: &vault_id,
                item_id: str_of(row, "item_id")?,
                key_version: old_version,
                label: label_of(row)?,
            };
            items.push((item, wrapped.as_slice()));
        }
        let recipients: Vec<Recipient> = members
            .iter()
            .map(|m| Recipient {
                user_id: &m.user_id,
                key_id: &m.key_id,
                public: &m.public,
            })
            .collect();
        let me = self.me()?;
        let entry = chain.next(old_version + 1, 1, members.clone(), &me.user_id, &me.key_id, &me.key)?;
        let rotated = vault::rotate(
            RotateInput {
                vault_id: &vault_id,
                old_version,
                old_key: self.vault_key(&vault_id, old_version)?,
                items: &items,
                members: &recipients,
                operational: args.get("operational").and_then(Value::as_bool).unwrap_or(false),
                signer: self.signer()?,
            },
            rng,
        )?;
        let item_keys: Vec<Value> = rotated
            .item_keys
            .iter()
            .map(|(id, w)| json!({ "item_id": id, "wrapped_item_key": hex(w) }))
            .collect();
        let member_wraps: Vec<Value> = rotated.member_wraps.iter().map(member_wrap_json).collect();
        let deposit = rotated
            .deposit
            .as_ref()
            .map(|(public, wrapped)| json!({ "deposit": deposit_json(public), "wrapped_private": hex(wrapped) }));
        let key_version = rotated.key_version;
        self.keep_vault_key(&vault_id, key_version, rotated.vault_key)?;
        Ok(json!({
            "entry": key_version_json(&entry), "key_version": key_version, "item_keys": item_keys,
            "member_wraps": member_wraps, "deposit": deposit,
        }))
    }

    /// Seals an item body. The supplied item key is used again only when its wrap is of the current key version;
    /// after a key-version change the item gets a new item key at this write (decision (c)).
    fn item_seal(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let item = ItemRef {
            vault_id: str_of(args, "vault_id")?,
            item_id: str_of(args, "item_id")?,
            key_version: u64_of(args, "key_version")?,
            label: label_of(args)?,
        };
        let vault_key = self.vault_key(item.vault_id, item.key_version)?;
        let current = match args.get("wrapped_item_key").and_then(Value::as_str) {
            Some(wrapped) if u64_of(args, "wrapped_key_version")? == item.key_version => Some(unhex(wrapped)?),
            _ => None,
        };
        let (item_key, wrapped, new_item_key) = match current {
            Some(wrapped) => (vault::unwrap_item_key(vault_key, &wrapped, item)?, wrapped, false),
            None => {
                let item_key = vault::new_item_key(rng);
                let wrapped = vault::wrap_item_key(vault_key, &item_key, item, rng);
                (item_key, wrapped, true)
            }
        };
        let sealed = vault::seal_item_body(&item_key, item, str_of(args, "body")?.as_bytes(), rng)?;
        Ok(json!({ "wrapped_item_key": hex(&wrapped), "sealed_body": hex(&sealed), "new_item_key": new_item_key }))
    }

    fn lease_approve(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let lease = lease_of(args)?;
        let lease_key = verify_of(args, "lease_key")?;
        let me = self.me()?;
        let mine = me.key.public().verify;
        let checked: Vec<VerifyingKey> = if self
            .key_checks(args)?
            .iter()
            .any(|check| check.covers_lease_key(&mine, &lease_key).is_ok())
        {
            vec![lease_key]
        } else {
            Vec::new()
        };
        let Some(Value::Object(rows)) = args.get("wrapped_item_keys") else {
            return Err(Error::Format("wrapped_item_keys"));
        };
        let wrapped = rows
            .iter()
            .map(|(id, w)| {
                Ok((
                    id.as_str(),
                    unhex(w.as_str().ok_or(Error::Format("wrapped_item_keys"))?)?,
                ))
            })
            .collect::<Result<Vec<(&str, Vec<u8>)>, Error>>()?;
        let wrapped_refs: Vec<(&str, &[u8])> = wrapped.iter().map(|(id, w)| (*id, w.as_slice())).collect();
        let approval = lease::approve(
            ApproveInput {
                lease: &lease,
                lease_key: &lease_key,
                checked: &checked,
                vault_key: self.vault_key(&lease.vault_id, lease.key_version)?,
                wrapped_item_keys: &wrapped_refs,
                approver_user_id: &me.user_id,
                approver: &me.key,
                now: i64_of(args, "now")?,
            },
            rng,
        )?;
        let sealed: Vec<Value> = approval
            .sealed
            .iter()
            .map(|s| json!({ "item_id": s.item_id, "sealed": hex(&s.sealed) }))
            .collect();
        Ok(json!({
            "lease_id": approval.lease_id, "sealed": sealed,
            "approver_user_id": approval.approver_user_id, "signature": hex(&approval.signature),
        }))
    }

    /// A value sealed to a user's personal key (a checked key, or this session's own) and signed with this session's
    /// key, as `VaultDelivery` stores it (§3.3).
    fn delivery_send(&self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let to_public = public_of(args, "to_public")?;
        self.require_checked(args, str_of(args, "to_user_id")?, &to_public)?;
        let to = To {
            user_id: str_of(args, "to_user_id")?,
            key_id: str_of(args, "to_key_id")?,
            public: &to_public,
        };
        let me = self.me()?;
        let sender = Sender {
            kind: "user",
            id: &me.user_id,
            key: me.key.signing_key(),
        };
        let value = str_of(args, "value")?.as_bytes();
        let sent = delivery::send(
            str_of(args, "delivery_id")?,
            to,
            str_of(args, "purpose")?,
            value,
            sender,
            rng,
        )?;
        Ok(delivery_json(&sent))
    }

    /// Opens a delivery to this session's key: its sender's signature first, under a key this session checked (or
    /// its own), then the value. A delivery id already opened here is refused, so a repeat is never filed twice.
    fn delivery_open(&mut self, args: &Value) -> Result<Value, Error> {
        let sent = delivery_of(object_of(args, "delivery")?)?;
        let sender = public_of(args, "sender_public")?;
        // the sender is the user the delivery names: a key checked as theirs, not just any checked key
        if sent.sender_kind != "user" {
            return Err(Error::Refused("only a user's delivery opens here"));
        }
        self.require_checked(args, &sent.sender_id, &sender)?;
        let me = self.me()?;
        if sent.recipient_user_id != me.user_id || sent.recipient_key_id != me.key_id {
            return Err(Error::Refused("the delivery is for someone else"));
        }
        let value = delivery::open(&sent, &me.key, &sender.verify)?;
        let text = std::str::from_utf8(&value).map_err(|_| Error::Format("a delivery's value is UTF-8"))?;
        let answer = json!({ "delivery_id": sent.delivery_id, "purpose": sent.purpose, "value": text });
        if !self.filed.first_time(&sent) {
            return Err(Error::Refused("that delivery was opened already"));
        }
        Ok(answer)
    }

    /// Opens a vault's recovery wrap with the recovery key combined in this session; the wrap's signer must be a
    /// manager of its key version in the vault's chain.
    fn recovery_open_vault(&mut self, args: &Value) -> Result<Value, Error> {
        let entries = entries_of(args)?;
        let chain = Chain::verify(&entries, &self.anchors(args)?)?;
        keyversion::check_remembered(&chain, remembered_of(args)?.as_ref())?;
        let record = record_of(args)?;
        let wrap = recovery_wrap_of(args)?;
        let Some(recovery_key) = self.recovery_keys.get(&record.recovery_key_id) else {
            return Err(Error::Refused("that recovery key has not been combined here"));
        };
        if recovery_key.public()? != record.public {
            return Err(Error::Refused("the record is not of the recovery key combined here"));
        }
        let signer = chain.manager_at(wrap.key_version, &wrap.signer_user_id, &wrap.signer_key_id)?;
        let key = recovery::open_vault_key(&wrap, &record, recovery_key, &signer.public.verify)?;
        self.keep_vault_key(&wrap.vault_id, wrap.key_version, key)?;
        Ok(json!({ "vault_id": wrap.vault_id, "key_version": wrap.key_version }))
    }
}

fn statement_of(name: &str) -> Result<crate::sign::Statement, Error> {
    use crate::sign::Statement;
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

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
