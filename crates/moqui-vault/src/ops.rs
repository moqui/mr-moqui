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
//!   key signed ([`KeyCheck`]), or the session's own key, or a recovery key whose maker's key is so named;
//! - a vault key opens only through the member-list chain (verified, checked against what the client remembers, and
//!   the wrap accepted by it), never with a signer key the page names;
//! - each signed statement is built here from its parts (a key version, a member wrap, a deposit key, a key check
//!   after the fingerprint the user compared, a recovery key and its session, a lease approval); there is no "sign these bytes";
//! - a recovery key is combined here and used here: no secret share or key goes back to the page.
//!
//! The request's strings are overwritten after the call and the answer is built in a buffer of its exact size.
//! It is compiled for the wasm module, natively under the feature `json` (the `moqui vault` command and the test
//! client's `vault` member), and for the crate's tests. [`shapes`] reads and writes the records' JSON, which the
//! server's vault services take and answer in the same form.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::delivery::{self, Filed, Sender, To};
use crate::fingerprint::{self, KeyCheck};
use crate::hpke;
use crate::keys::Key32;
use crate::keyversion::{self, recovery_key_of, recovery_member_id, Chain, KeyVersion, Member, Role};
use crate::lease::{self, ApproveInput};
use crate::personal::{self, PersonalKey, PersonalPublic};
use crate::recovery::{self, Delivered, HolderShare, RecoveryKeyRecord, RecoverySession, SessionShare};
use crate::rng::Rng;
use crate::sign::VerifyingKey;
use crate::text::{hex, unhex};
use crate::vault::{self, ItemRef, Recipient, RotateInput, Signer};
use crate::Error;

#[path = "ops_json.rs"]
pub mod shapes;

use shapes::{
    array_of, bytes_of, delivery_json, delivery_of, deposit_json, entries_of, holder_share_from, holder_share_json,
    holder_share_of, i64_of, key_check_json, key_checks_of, key_version_json, label_of, lease_of, member_wrap_json,
    member_wrap_of, members_of, object_of, password_wrap_json, password_wrap_of, public_of, record_json, record_of,
    remembered_json, remembered_of, scrub, session_from, session_json, session_share_from, session_share_json, str_of,
    to_text, u64_of, verify_of,
};

/// The opened personal key and who it is.
struct Me {
    key_id: String,
    user_id: String,
    key: PersonalKey,
}

/// A recovery key combined for one session, and the vault key it opened.
struct Pending {
    session_id: String,
    key: PersonalKey,
    opened: Option<(String, u64)>,
}

/// The calls that may follow a combine without dropping the combined recovery key.
const RECOVERY_STEPS: [&str; 3] = ["recovery_open_vault", "recovery_rekey", "status"];

/// What the module holds between calls.
#[derive(Default)]
pub struct Session {
    me: Option<Me>,
    vault_keys: BTreeMap<(String, u64), Key32>,
    /// A recovery key combined here, for the session it was combined in, until that session's rekey (or any other
    /// call): the whole key exists only between the combine and the rekey.
    pending: Option<Pending>,
    /// The recovery sessions this client started, by id, and the ephemeral key of each until its shares combine.
    recovery_sessions: BTreeMap<String, RecoverySession>,
    ephemerals: BTreeMap<String, hpke::SecretKey>,
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

    /// Trust anchors: this session's own signing key, every key a verified check names, and each recovery key whose
    /// maker's key a verified check names (an entry a recovery signed, §3.10).
    fn anchors(&self, args: &Value) -> Result<Vec<VerifyingKey>, Error> {
        let mine = self.me()?.key.public().verify;
        let mut anchors = vec![mine];
        anchors.extend(
            self.key_checks(args)?
                .iter()
                .filter_map(|check| check.checked_verify_key(&mine).ok()),
        );
        anchors.extend(self.recovery_records(args)?.iter().map(|record| record.public.verify));
        Ok(anchors)
    }

    /// The recovery keys the page passed (`recovery_keys`: each `{record, maker_public}`), each signed by a maker
    /// whose key this session checked. One that is not is refused, never left out: a client refuses a recovery key
    /// with no signature it can check.
    fn recovery_records(&self, args: &Value) -> Result<Vec<RecoveryKeyRecord>, Error> {
        let Some(rows) = args.get("recovery_keys").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        rows.iter().map(|row| self.checked_record(args, row)).collect()
    }

    /// `row`'s recovery key record, after checking its signature by its maker, a key this session checked.
    fn checked_record(&self, args: &Value, row: &Value) -> Result<RecoveryKeyRecord, Error> {
        let record = record_of(row)?;
        let maker = public_of(row, "maker_public")?;
        self.require_checked_person(args, &record.maker_user_id, &maker)?;
        record.verify(&maker.verify)?;
        Ok(record)
    }

    /// Refuses a key that is neither this session's own, nor covered by a verified check of it as `user_id`'s key,
    /// nor (for a member list's recovery member) a recovery key whose maker this session checked.
    fn require_checked(&self, args: &Value, user_id: &str, public: &PersonalPublic) -> Result<(), Error> {
        let Some(recovery_key_id) = recovery_key_of(user_id) else {
            return self.require_checked_person(args, user_id, public);
        };
        let listed = self
            .recovery_records(args)?
            .iter()
            .any(|record| record.recovery_key_id == recovery_key_id && record.public == *public);
        if !listed {
            return Err(Error::Refused("a recovery key whose maker's key has not been checked"));
        }
        Ok(())
    }

    /// [`Self::require_checked`] for a person's key.
    fn require_checked_person(&self, args: &Value, user_id: &str, public: &PersonalPublic) -> Result<(), Error> {
        let me = self.me()?;
        if *public == me.key.public() && user_id == me.user_id {
            return Ok(());
        }
        fingerprint::require_checked(&self.key_checks(args)?, &me.key.public().verify, user_id, public)
    }

    /// Drops a combined recovery key, and the vault key it opened.
    fn drop_pending(&mut self) {
        if let Some(Pending { opened: Some(slot), .. }) = self.pending.take() {
            self.vault_keys.remove(&slot);
        }
    }

    fn dispatch(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let op = str_of(args, "op")?;
        if self.pending.is_some() && !RECOVERY_STEPS.contains(&op) {
            self.drop_pending();
        }
        match op {
            #[cfg(any(test, feature = "test-vectors"))]
            "vectors" => crate::vectors::generate(),
            "lock" => {
                // Dropping the keys zeroizes them.
                self.me = None;
                self.drop_pending();
                self.vault_keys.clear();
                self.recovery_sessions.clear();
                self.ephemerals.clear();
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
            "vault_key_generated_wrap" => {
                // a single-secret vault's generated key (§3.11): 256 random bits, HKDF, no Argon2id; made here unless
                // the page passes the one the user keeps
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let key = self.vault_key(vault_id, key_version)?;
                let generated = match args.get("generated").and_then(Value::as_str) {
                    Some(given) => zeroize::Zeroizing::new(given.to_string()),
                    None => vault::new_generated_key(rng),
                };
                let sealed = vault::wrap_vault_key_with_generated(key, vault_id, key_version, &generated, rng)?;
                Ok(json!({ "generated": generated.as_str(), "wrap": { "sealed": hex(&sealed) } }))
            }
            "vault_key_generated_open" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let sealed = bytes_of(object_of(args, "wrap")?, "sealed")?;
                let key =
                    vault::open_vault_key_with_generated(&sealed, vault_id, key_version, str_of(args, "generated")?)?;
                self.keep_vault_key(vault_id, key_version, key)?;
                Ok(json!({ "vault_id": vault_id, "key_version": key_version }))
            }
            "vault_key_forget" => {
                // a page undoes a vault_rotate whose server call failed: the version's key is dropped, which zeroes it
                let slot = (str_of(args, "vault_id")?.to_string(), u64_of(args, "key_version")?);
                Ok(json!({ "forgotten": self.vault_keys.remove(&slot).is_some() }))
            }
            "item_summary" => self.item_summary(args),
            "header_seal" => {
                // a personal vault's header (its name), sealed by the vault key of its current version
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let key = self.vault_key(vault_id, key_version)?;
                let sealed = vault::seal_header(key, vault_id, key_version, str_of(args, "header")?.as_bytes(), rng)?;
                Ok(json!({ "header_cipher": hex(&sealed) }))
            }
            "header_open" => {
                let vault_id = str_of(args, "vault_id")?;
                let key_version = u64_of(args, "key_version")?;
                let key = self.vault_key(vault_id, key_version)?;
                let header = vault::open_header(key, vault_id, key_version, &bytes_of(args, "header_cipher")?)?;
                let text = std::str::from_utf8(&header).map_err(|_| Error::Format("a header is UTF-8"))?;
                Ok(json!({ "header": text }))
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
            "recovery_make" => self.recovery_make(args, rng),
            "recovery_member" => {
                // how a checked recovery key is listed in a member list, to turn recovery on (key_version_next and
                // vault_key_wrap_member, with the record in recovery_keys)
                let record = self.checked_record(args, args)?;
                let member = record.member();
                Ok(
                    json!({ "user_id": member.user_id, "role": member.role.as_str(), "key_id": member.key_id,
                           "public": hex(&member.public.to_bytes()) }),
                )
            }
            "recovery_session_start" => self.recovery_session_start(args, rng),
            "recovery_share_deliver" => {
                let record = self.checked_record(args, args)?;
                let maker = public_of(args, "maker_public")?;
                let held = holder_share_of(args)?;
                if held.recovery_key_id != record.recovery_key_id {
                    return Err(Error::Refused("a share of another recovery key"));
                }
                let session = session_from(object_of(args, "session")?)?;
                let holder_public = public_of(args, "session_holder_public")?;
                self.require_checked_person(args, &session.holder_user_id, &holder_public)?;
                // §3.5: this holder, too, has checked the member's new key before their share helps wrap to it
                self.require_checked_person(args, &session.subject_user_id, &session.subject_public)?;
                let starter_held = holder_share_from(object_of(args, "session_holder")?)?;
                let starter = recovery::Starter {
                    held: &starter_held,
                    public: &holder_public,
                };
                let sent = recovery::deliver_share(&held, self.signer()?, &maker.verify, &session, starter, rng)?;
                Ok(session_share_json(&sent))
            }
            "recovery_combine" => self.recovery_combine(args),
            "recovery_open_vault" => self.recovery_open_vault(args),
            "recovery_rekey" => self.recovery_rekey(args, rng),
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
        // an item the server deposited (its key wrapped to the old version's deposit key) is taken in on the way: its key
        // unwrapped with the deposit key's private half, which the old vault key opens, so a removal never waits on it
        let deposit = match args.get("deposit_wrapped_private").and_then(Value::as_str) {
            Some(wrapped) if !wrapped.is_empty() => Some(vault::open_deposit_key(
                self.vault_key(&vault_id, old_version)?,
                &vault_id,
                old_version,
                &unhex(wrapped)?,
            )?),
            _ => None,
        };
        let mut wrapped = Vec::with_capacity(item_rows.len());
        for row in item_rows {
            let given = bytes_of(row, "wrapped_item_key")?;
            if row.get("deposited").and_then(Value::as_bool) != Some(true) {
                wrapped.push(given);
                continue;
            }
            let Some(deposit) = &deposit else {
                return Err(Error::Format("deposit_wrapped_private, to take in a deposited item"));
            };
            let item = ItemRef {
                vault_id: &vault_id,
                item_id: str_of(row, "item_id")?,
                key_version: old_version,
                label: label_of(row)?,
            };
            let item_key = vault::unwrap_deposited_item_key(deposit, &given, item)?;
            wrapped.push(vault::wrap_item_key(
                self.vault_key(&vault_id, old_version)?,
                &item_key,
                item,
                rng,
            ));
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
        // a personal vault's header goes to the new version with its items
        let header_cipher = match args.get("header_cipher").and_then(Value::as_str) {
            Some(sealed) if !sealed.is_empty() => {
                let old_key = self.vault_key(&vault_id, old_version)?;
                let header = vault::open_header(old_key, &vault_id, old_version, &unhex(sealed)?)?;
                Value::String(hex(&vault::seal_header(
                    &rotated.vault_key,
                    &vault_id,
                    key_version,
                    &header,
                    rng,
                )?))
            }
            _ => Value::Null,
        };
        self.keep_vault_key(&vault_id, key_version, rotated.vault_key)?;
        Ok(json!({
            "entry": key_version_json(&entry), "key_version": key_version, "item_keys": item_keys,
            "member_wraps": member_wraps, "deposit": deposit, "header_cipher": header_cipher,
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
            // decision (c): an item key made before this key version (item_key_version, which a rotation leaves) is
            // replaced at this write, though its wrap was moved to this version
            Some(wrapped)
                if u64_of(args, "wrapped_key_version")? == item.key_version
                    && args
                        .get("item_key_version")
                        .and_then(Value::as_u64)
                        .is_none_or(|made_at| made_at >= item.key_version) =>
            {
                Some(unhex(wrapped)?)
            }
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

    /// An item's header for a list view: its name, type, URIs, tags and unmasked fields, never a masked field's value or
    /// its notes, so a page lists a vault without holding its secrets. A field is masked when its type says so (the
    /// page passes the type's masked field names, `masked`) or when its name says it holds a secret.
    fn item_summary(&self, args: &Value) -> Result<Value, Error> {
        let item = ItemRef {
            vault_id: str_of(args, "vault_id")?,
            item_id: str_of(args, "item_id")?,
            key_version: u64_of(args, "key_version")?,
            label: label_of(args)?,
        };
        let vault_key = self.vault_key(item.vault_id, item.key_version)?;
        let item_key = vault::unwrap_item_key(vault_key, &bytes_of(args, "wrapped_item_key")?, item)?;
        let body = vault::open_item_body(&item_key, item, &bytes_of(args, "sealed_body")?)?;
        let mut parsed: Value = serde_json::from_slice(&body).map_err(|_| Error::Format("a body is UTF-8 JSON"))?;
        let masked: Vec<&str> = args
            .get("masked")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut fields = serde_json::Map::new();
        if let Some(Value::Object(given)) = parsed.get("fields") {
            for (name, value) in given {
                if !masked.contains(&name.as_str()) && !secret_name(name) {
                    fields.insert(name.clone(), value.clone());
                }
            }
        }
        let summary = json!({
            "type": parsed.get("type").cloned().unwrap_or(Value::Null),
            "name": parsed.get("name").cloned().unwrap_or(Value::Null),
            "uris": parsed.get("uris").cloned().unwrap_or(Value::Null),
            "tags": parsed.get("tags").cloned().unwrap_or(Value::Null),
            "fields": fields,
        });
        scrub(&mut parsed);
        Ok(summary)
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

    /// Makes the organization's recovery key here (§3.10): split `k` of the holders' count, one share wrapped to each
    /// holder's key, which must be a key this session checked (or its own), and the seed dropped.
    fn recovery_make(&self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let rows = array_of(args, "holders")?;
        let mut listed = Vec::with_capacity(rows.len());
        for row in rows {
            let public = public_of(row, "public")?;
            self.require_checked_person(args, str_of(row, "user_id")?, &public)?;
            listed.push((str_of(row, "user_id")?, str_of(row, "key_id")?, public));
        }
        let holders: Vec<recovery::Holder> = listed
            .iter()
            .map(|(user_id, key_id, public)| recovery::Holder {
                user_id,
                key_id,
                public,
            })
            .collect();
        let k = u8::try_from(u64_of(args, "k")?).map_err(|_| Error::Format("k"))?;
        let recovery_key_id = str_of(args, "recovery_key_id")?;
        let (record, shares) = recovery::make(recovery_key_id, k, &holders, self.signer()?, rng)?;
        let shares: Vec<Value> = shares.iter().map(holder_share_json).collect();
        Ok(json!({ "record": record_json(&record), "shares": shares }))
    }

    /// Starts a recovery here, as the recovering holder: a new ephemeral key kept in this session, and the session
    /// signed by this key, naming the vault and the member's new personal key, which must be a key this session
    /// checked (§3.5: a changed key is checked again before anything is wrapped to it).
    fn recovery_session_start(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let record = self.checked_record(args, args)?;
        let held = holder_share_of(args)?;
        let me = self.me()?;
        if held.recovery_key_id != record.recovery_key_id || held.holder_user_id != me.user_id {
            return Err(Error::Refused(
                "only a holder of this recovery key starts a recovery with it",
            ));
        }
        let subject_public = public_of(args, "subject_public")?;
        let subject_user_id = str_of(args, "subject_user_id")?;
        if recovery_key_of(subject_user_id).is_some() {
            return Err(Error::Refused("a recovery is for a person"));
        }
        self.require_checked_person(args, subject_user_id, &subject_public)?;
        let session_id = str_of(args, "session_id")?;
        if self.recovery_sessions.contains_key(session_id) {
            return Err(Error::Refused("that recovery session is already started here"));
        }
        let subject = vault::Recipient {
            user_id: subject_user_id,
            key_id: str_of(args, "subject_key_id")?,
            public: &subject_public,
        };
        let vault_id = str_of(args, "vault_id")?;
        let (session, ephemeral) = recovery::start_session(
            session_id,
            &record.recovery_key_id,
            vault_id,
            subject,
            self.signer()?,
            rng,
        )?;
        let answer = session_json(&session);
        self.ephemerals.insert(session_id.to_string(), ephemeral);
        self.recovery_sessions.insert(session_id.to_string(), session);
        Ok(answer)
    }

    /// Combines this holder's own share with the shares delivered to a session it started (each `{share, holder,
    /// public}`: the sealed share, the sender's holder row and their public key), checked against the signed record.
    /// The recovery key stays here for this session's open and rekey only (any other call drops it); the session and
    /// its ephemeral key stay until the rekey, so a dropped combine can be made again.
    fn recovery_combine(&mut self, args: &Value) -> Result<Value, Error> {
        let record = self.checked_record(args, args)?;
        let maker = public_of(args, "maker_public")?;
        let session_id = str_of(args, "session_id")?;
        let (Some(session), Some(ephemeral)) =
            (self.recovery_sessions.get(session_id), self.ephemerals.get(session_id))
        else {
            return Err(Error::Refused("that recovery session was not started here"));
        };
        let held = holder_share_of(args)?;
        let me = self.me()?;
        if held.holder_user_id != me.user_id || held.holder_key_id != me.key_id {
            return Err(Error::Refused("the share is someone else's"));
        }
        let own = recovery::open_share(&held, &me.key, &maker.verify)?;
        let rows = array_of(args, "deliveries")?;
        let mut parsed: Vec<(SessionShare, HolderShare, PersonalPublic)> = Vec::with_capacity(rows.len());
        for row in rows {
            parsed.push((
                session_share_from(object_of(row, "share")?)?,
                holder_share_from(object_of(row, "holder")?)?,
                public_of(row, "public")?,
            ));
        }
        let delivered: Vec<Delivered> = parsed
            .iter()
            .map(|(share, holder, public)| Delivered { share, holder, public })
            .collect();
        let key = recovery::combine_session(&record, &maker.verify, session, ephemeral, own, &delivered)?;
        self.pending = Some(Pending {
            session_id: session_id.to_string(),
            key,
            opened: None,
        });
        Ok(json!({ "recovery_key_id": record.recovery_key_id, "session_id": session_id }))
    }

    /// Opens the vault key of a session this module started, with the recovery key combined for it: the recovery
    /// member's wrap of the session's vault, accepted by the vault's chain like any member's (a chain this session can
    /// anchor: a signer whose key it checked). The vault key is kept only until the rekey.
    fn recovery_open_vault(&mut self, args: &Value) -> Result<Value, Error> {
        let session_id = str_of(args, "session_id")?;
        let Some(session) = self.recovery_sessions.get(session_id) else {
            return Err(Error::Refused("that recovery session was not started here"));
        };
        let Some(pending) = self
            .pending
            .as_ref()
            .filter(|p| p.session_id == session_id && p.opened.is_none())
        else {
            return Err(Error::Refused("no recovery key is combined here for that session"));
        };
        let entries = entries_of(args)?;
        let chain = Chain::verify(&entries, &self.anchors(args)?)?;
        keyversion::check_remembered(&chain, remembered_of(args)?.as_ref())?;
        let wrap = member_wrap_of(object_of(args, "wrap")?)?;
        let recovery_key_id = &session.recovery_key_id;
        if wrap.vault_id != session.vault_id
            || wrap.member_user_id != recovery_member_id(recovery_key_id)
            || wrap.member_key_id != *recovery_key_id
        {
            return Err(Error::Refused(
                "the wrap is not the recovery key's wrap of the session's vault",
            ));
        }
        let (_, signer) = chain.accept_wrap(&wrap)?;
        let key = vault::open_member_wrap(&wrap, &pending.key, &signer)?;
        let (vault_id, key_version) = (wrap.vault_id.clone(), wrap.key_version);
        self.keep_vault_key(&vault_id, key_version, key)?;
        if let Some(pending) = self.pending.as_mut() {
            pending.opened = Some((vault_id.clone(), key_version));
        }
        Ok(json!({ "vault_id": vault_id, "key_version": key_version }))
    }

    /// The recovery's last step, signed by the recovery key as the vault's recovery member: the next member list,
    /// with the session's member under their new personal key (checked again here) and nothing else changed, the
    /// vault key wrapped to that key, and, when that member was a manager whose wraps the change leaves unsigned, the
    /// other members' wraps remade. Then the combined recovery key, the session and the vault key it opened are
    /// dropped, whatever the answer: the module keeps nothing of the recovery.
    fn recovery_rekey(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let answer = self.rekey(args, rng);
        if let Ok(session_id) = str_of(args, "session_id") {
            self.recovery_sessions.remove(session_id);
            self.ephemerals.remove(session_id);
        }
        self.drop_pending();
        answer
    }

    fn rekey(&mut self, args: &Value, rng: &mut dyn Rng) -> Result<Value, Error> {
        let session_id = str_of(args, "session_id")?;
        let Some(session) = self.recovery_sessions.get(session_id) else {
            return Err(Error::Refused("that recovery session was not started here"));
        };
        let Some(pending) = self.pending.as_ref().filter(|p| p.session_id == session_id) else {
            return Err(Error::Refused("no recovery key is combined here for that session"));
        };
        let recovery_key = &pending.key;
        let entries = entries_of(args)?;
        let chain = Chain::verify(&entries, &self.anchors(args)?)?;
        keyversion::check_remembered(&chain, remembered_of(args)?.as_ref())?;
        self.require_checked_person(args, &session.subject_user_id, &session.subject_public)?;
        let latest = chain.latest();
        if latest.vault_id != session.vault_id {
            return Err(Error::Refused("a chain of another vault than the session's"));
        }
        let recovery_user = recovery_member_id(&session.recovery_key_id);
        if latest
            .member(&recovery_user, &session.recovery_key_id)
            .is_none_or(|m| m.public != recovery_key.public() || m.role != Role::Manage)
        {
            return Err(Error::Refused(
                "recovery is not on for this vault with this recovery key",
            ));
        }
        let Some(was) = latest.members.iter().find(|m| m.user_id == session.subject_user_id) else {
            return Err(Error::Refused("the session's member is not a member of this vault"));
        };
        let members: Vec<Member> = latest
            .members
            .iter()
            .map(|m| {
                if m.user_id != session.subject_user_id {
                    return m.clone();
                }
                Member {
                    key_id: session.subject_key_id.clone(),
                    public: session.subject_public,
                    ..m.clone()
                }
            })
            .collect();
        let entry = chain.next(
            latest.version,
            latest.revision + 1,
            members,
            &recovery_user,
            &session.recovery_key_id,
            recovery_key,
        )?;
        let signer = Signer {
            user_id: &recovery_user,
            key_id: &session.recovery_key_id,
            key: recovery_key,
        };
        let vault_key = self.vault_key(&latest.vault_id, latest.version)?;
        let to = Recipient {
            user_id: &session.subject_user_id,
            key_id: &session.subject_key_id,
            public: &session.subject_public,
        };
        let wrap = vault::wrap_vault_key_to_member(vault_key, &latest.vault_id, latest.version, to, signer, rng)?;
        let mut rewraps = Vec::new();
        if was.role == Role::Manage {
            for member in entry.members.iter().filter(|m| m.user_id != session.subject_user_id) {
                let to = Recipient {
                    user_id: &member.user_id,
                    key_id: &member.key_id,
                    public: &member.public,
                };
                let rewrap =
                    vault::wrap_vault_key_to_member(vault_key, &latest.vault_id, latest.version, to, signer, rng)?;
                rewraps.push(member_wrap_json(&rewrap));
            }
        }
        Ok(json!({ "entry": key_version_json(&entry), "wrap": member_wrap_json(&wrap), "rewraps": rewraps }))
    }
}

/// The open personal key as a desktop or mobile app's device memory keeps it (§3.5, V5): its private halves wrapped by a
/// device memory key ([`personal::wrap_with_device_key`]), with who it is. `Debug` shows no bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceSealed {
    pub personal_key_id: String,
    pub user_id: String,
    /// Both public halves, encrypt then verify (64 bytes).
    pub public: Vec<u8>,
    pub sealed: Vec<u8>,
}

impl std::fmt::Debug for DeviceSealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceSealed")
            .field("personal_key_id", &self.personal_key_id)
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// Device memory (§3.5, V5). These are not calls of [`Session::call`]: no page can ask the module for its personal key
/// in any form. A shell calls them natively, and keeps the device memory key in the OS key store.
impl Session {
    /// The open personal key wrapped by `device_key` for the memory `memory_id`.
    ///
    /// # Errors
    ///
    /// [`Error::Refused`] when the personal key is locked.
    pub fn device_seal(&self, memory_id: &str, device_key: &Key32, rng: &mut dyn Rng) -> Result<DeviceSealed, Error> {
        let me = self.me()?;
        Ok(DeviceSealed {
            personal_key_id: me.key_id.clone(),
            user_id: me.user_id.clone(),
            public: me.key.public().to_bytes().to_vec(),
            sealed: personal::wrap_with_device_key(&me.key, &me.key_id, memory_id, device_key, rng),
        })
    }

    /// Opens the personal key from device memory, as `personal_open` does with the passphrase; answers as it does.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] for another key or memory, or a changed wrap.
    pub fn device_open(&mut self, kept: &DeviceSealed, memory_id: &str, device_key: &Key32) -> Result<Value, Error> {
        let public = PersonalPublic::from_bytes(&kept.public)?;
        let key = personal::open_with_device_key(&kept.sealed, &kept.personal_key_id, memory_id, &public, device_key)?;
        let answer = opened_json(&kept.personal_key_id, &key);
        self.me = Some(Me {
            key_id: kept.personal_key_id.clone(),
            user_id: kept.user_id.clone(),
            key,
        });
        Ok(answer)
    }

    /// Which personal key is open, if one is.
    pub fn personal_key_id(&self) -> Option<&str> {
        self.me.as_ref().map(|me| me.key_id.as_str())
    }
}

/// Whether a field's name says it holds a secret, whatever its type registers: a password, passphrase, secret, key,
/// token, PIN, one-time code seed, card number or security code.
fn secret_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "pass", "secret", "key", "token", "pin", "totp", "seed", "code", "number", "cvv", "share",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn statement_of(name: &str) -> Result<crate::sign::Statement, Error> {
    use crate::sign::Statement;
    let all = [
        Statement::MemberWrap,
        Statement::KeyVersion,
        Statement::DepositKey,
        Statement::RecoveryKey,
        Statement::RecoverySession,
        Statement::RecoverySessionShare,
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
