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

//! The known-answer vectors (plans/Vault-Instances.md §6 V0: the same bytes natively and in wasm). Each section is
//! made from its own [`TestRng`] stream, so the whole of `testdata/vectors.json` is a function of this code alone.
//! `MOQUI_REGENERATE_VECTORS=1 cargo test -p moqui-vault` writes the file; otherwise the test compares, and the
//! browser module's `vectors` call returns the same JSON for the client spec to compare.

use serde_json::{json, Value};

use crate::delivery::{self, Sender, To};
use crate::fingerprint;
use crate::hpke;
use crate::kdf::{self, Argon2Params};
use crate::keys::Key32;
use crate::lease::{self, ApproveInput, Lease, LeaseItem};
use crate::machine;
use crate::names::{ItemLabel, Names, Purpose};
use crate::personal::{self, PersonalKey};
use crate::rng::{Rng, TestRng};
use crate::shamir;
use crate::sign::{SigningKey, Statement};
use crate::text::hex;
use crate::vault::{self, ItemRef, Recipient, Signer};
use crate::{aead, Error};

/// The CFRG test vector for HPKE base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM (kem 0x20, kdf 1,
/// aead 2) from draft-irtf-cfrg-hpke's test-vectors.json, its first encryption.
pub const HPKE_CFRG: [(&str, &str); 8] = [
    ("info", "4f6465206f6e2061204772656369616e2055726e"),
    (
        "ikmE",
        "2cd7c601cefb3d42a62b04b7a9041494c06c7843818e0ce28a8f704ae7ab20f9",
    ),
    (
        "ikmR",
        "dac33b0e9db1b59dbbea58d59a14e7b5896e9bdf98fad6891e99d1686492b9ee",
    ),
    (
        "pkRm",
        "430f4b9859665145a6b1ba274024487bd66f03a2dd577d7753c68d7d7d00c00c",
    ),
    (
        "enc",
        "6c93e09869df3402d7bf231bf540fadd35cd56be14f97178f0954db94b7fc256",
    ),
    ("pt", "4265617574792069732074727574682c20747275746820626561757479"),
    ("aad", "436f756e742d30"),
    (
        "ct",
        "e5d84cd531cfb583096e7cfa9641bd3079cf3a91cda813c52deb5f512be9931980a41de125a925cdad859d5b7a",
    ),
];

fn h(bytes: &[u8]) -> Value {
    Value::String(hex(bytes))
}

fn cfrg(name: &str) -> Vec<u8> {
    let (_, value) = HPKE_CFRG
        .iter()
        .find(|(key, _)| *key == name)
        .expect("a CFRG vector field");
    crate::text::unhex(value).expect("the CFRG vector is hex")
}

/// Gives back fixed bytes: the CFRG vector's ephemeral input keying material.
struct FixedRng(Vec<u8>);

impl Rng for FixedRng {
    fn fill(&mut self, out: &mut [u8]) {
        let take = out.len().min(self.0.len());
        out[..take].copy_from_slice(&self.0[..take]);
        self.0.drain(..take);
    }
}

/// Every vector, as the JSON of `testdata/vectors.json`.
pub fn generate() -> Result<Value, Error> {
    Ok(json!({
        "format": "moqui-vault vectors 1",
        "aead": aead_vector(),
        "body": body_vector()?,
        "argon2id": argon2id_vector()?,
        "hkdf": hkdf_vector(),
        "hpke_cfrg": hpke_cfrg_vector()?,
        "hpke_wrap": hpke_wrap_vector()?,
        "sign": sign_vector(),
        "personal": personal_vector()?,
        "vault": vault_vector()?,
        "lease": lease_vector()?,
        "machine": machine_vector(),
        "shamir": shamir_vector()?,
        "delivery": delivery_vector()?,
    }))
}

fn aead_vector() -> Value {
    let mut rng = TestRng::new("vectors aead");
    let key = Key32::random(&mut rng);
    let ad: [&[u8]; 2] = [b"vault-1", b"item-key"];
    let sealed = aead::seal(&key, &ad, b"a secret value", &mut rng);
    json!({ "key": h(key.as_bytes()), "ad": [h(ad[0]), h(ad[1])], "plaintext": h(b"a secret value"), "sealed": h(&sealed) })
}

fn body_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors body");
    let key = Key32::random(&mut rng);
    let body = br#"{"v":1,"type":"login","name":"Example","fields":{"username":"me","password":"pw"}}"#;
    let sealed = aead::seal_body(&key, &[b"body"], body, &mut rng)?;
    Ok(json!({ "key": h(key.as_bytes()), "ad": [h(b"body")], "body": h(body), "sealed": h(&sealed) }))
}

fn argon2id_vector() -> Result<Value, Error> {
    let params = Argon2Params::FLOOR;
    let key = kdf::argon2id(b"correct horse battery staple", b"moqui-vault salt", params)?;
    Ok(json!({
        "password": h(b"correct horse battery staple"),
        "salt": h(b"moqui-vault salt"),
        "m_kib": params.m_kib, "t": params.t, "p": params.p,
        "key": h(key.as_bytes()),
    }))
}

fn hkdf_vector() -> Value {
    let key = kdf::hkdf_sha256(b"input keying material", b"salt", b"vault-machine");
    json!({ "ikm": h(b"input keying material"), "salt": h(b"salt"), "info": h(b"vault-machine"), "key": h(key.as_bytes()) })
}

fn hpke_cfrg_vector() -> Result<Value, Error> {
    let recipient = hpke::SecretKey::derive(&cfrg("ikmR"));
    let public = recipient.public()?;
    let sealed = hpke::seal_raw(
        &public,
        &cfrg("info"),
        &cfrg("aad"),
        &cfrg("pt"),
        &mut FixedRng(cfrg("ikmE")),
    )?;
    let opened = hpke::open_raw(&recipient, &cfrg("info"), &cfrg("aad"), &sealed)?;
    let mut fields: serde_json::Map<String, Value> = HPKE_CFRG
        .iter()
        .map(|(name, value)| (name.to_string(), Value::String(value.to_string())))
        .collect();
    fields.insert("computed_pkRm".to_string(), h(public.as_bytes()));
    fields.insert("computed_enc".to_string(), h(&sealed[..hpke::ENC_LEN]));
    fields.insert("computed_ct".to_string(), h(&sealed[hpke::ENC_LEN..]));
    fields.insert("computed_pt".to_string(), h(&opened));
    Ok(Value::Object(fields))
}

fn hpke_wrap_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors hpke");
    let secret = hpke::SecretKey::random(&mut rng);
    let public = secret.public()?;
    let names = Names::new(Purpose::VaultKey, "vault-1", "", 3);
    let sealed = hpke::wrap(&public, &names, &[b"user-1"], b"a vault key", &mut rng)?;
    Ok(json!({
        "secret": h(secret.as_key().as_bytes()),
        "public": h(public.as_bytes()),
        "purpose": "vault-key", "vault_id": "vault-1", "item_id": "", "key_version": 3, "extra": [h(b"user-1")],
        "info": h(&names.encode(&[b"user-1"])),
        "plaintext": h(b"a vault key"),
        "sealed": h(&sealed),
    }))
}

fn sign_vector() -> Value {
    let mut rng = TestRng::new("vectors sign");
    let key = SigningKey::random(&mut rng);
    let parts: [&[u8]; 2] = [b"vault-1", b"user-1"];
    let signature = key.sign(Statement::MemberWrap, &parts);
    json!({
        "seed": h(key.seed().as_bytes()),
        "public": h(key.verifying_key().as_bytes()),
        "statement": "member-wrap",
        "parts": [h(parts[0]), h(parts[1])],
        "signature": h(&signature),
    })
}

fn personal_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors personal");
    let key = PersonalKey::generate(&mut rng);
    let public = key.public();
    let wrap = personal::wrap_with_passphrase(&key, "pk-1", b"a long key passphrase", &mut rng)?;
    let code = personal::new_recovery_code(&mut rng);
    let by_code = personal::wrap_with_recovery_code(&key, "pk-1", &code, &mut rng)?;
    Ok(json!({
        "encrypt_secret": h(key.encrypt_key().as_key().as_bytes()),
        "sign_seed": h(key.signing_key().seed().as_bytes()),
        "public": h(&public.to_bytes()),
        "fingerprint": fingerprint::personal(&public),
        "personal_key_id": "pk-1",
        "passphrase": h(b"a long key passphrase"),
        "passphrase_wrap": {
            "m_kib": wrap.params.m_kib, "t": wrap.params.t, "p": wrap.params.p,
            "salt": h(&wrap.salt), "sealed": h(&wrap.sealed),
        },
        "recovery_code": &*code,
        "recovery_wrap": h(&by_code),
    }))
}

const LABEL: ItemLabel = ItemLabel {
    item_type: "database-credential",
    binding: "AppInstance:100:dbPassword",
    name: "db",
};

fn vault_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors vault");
    let manager = PersonalKey::generate(&mut rng);
    let member = PersonalKey::generate(&mut rng);
    let member_public = member.public();
    let vault_key = vault::new_vault_key(&mut rng);
    let to = Recipient {
        user_id: "member",
        key_id: "pk-m",
        public: &member_public,
    };
    let by = Signer {
        user_id: "manager",
        key_id: "pk-a",
        key: &manager,
    };
    let wrap = vault::wrap_vault_key_to_member(&vault_key, "vault-1", 1, to, by, &mut rng)?;
    let item_key = vault::new_item_key(&mut rng);
    let item = ItemRef {
        vault_id: "vault-1",
        item_id: "item-1",
        key_version: 1,
        label: Some(LABEL),
    };
    let wrapped_item = vault::wrap_item_key(&vault_key, &item_key, item, &mut rng);
    let body = vault::seal_item_body(&item_key, item, br#"{"v":1,"type":"database-credential"}"#, &mut rng)?;
    Ok(json!({
        "manager_public": h(&manager.public().to_bytes()),
        "member_encrypt_secret": h(member.encrypt_key().as_key().as_bytes()),
        "member_sign_seed": h(member.signing_key().seed().as_bytes()),
        "vault_key": h(vault_key.as_bytes()),
        "member_wrap": { "sealed": h(&wrap.sealed), "signature": h(&wrap.signature) },
        "item_key": h(item_key.as_bytes()),
        "item": { "item_type": LABEL.item_type, "binding": LABEL.binding, "name": LABEL.name },
        "wrapped_item_key": h(&wrapped_item),
        "body": h(br#"{"v":1,"type":"database-credential"}"#),
        "sealed_body": h(&body),
    }))
}

fn lease_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors lease");
    let server = SigningKey::random(&mut rng);
    let approver = PersonalKey::generate(&mut rng);
    let vault_key = vault::new_vault_key(&mut rng);
    let item_key = vault::new_item_key(&mut rng);
    let item = ItemRef {
        vault_id: "vault-1",
        item_id: "item-1",
        key_version: 1,
        label: Some(LABEL),
    };
    let wrapped = vault::wrap_item_key(&vault_key, &item_key, item, &mut rng);
    let (lease_secret, public) = lease::new_lease_key(&mut rng)?;
    let mut lease = Lease {
        lease_id: "lease-1".to_string(),
        vault_id: "vault-1".to_string(),
        key_version: 1,
        items: vec![LeaseItem {
            item_id: "item-1".to_string(),
            item_type: LABEL.item_type.to_string(),
            binding: LABEL.binding.to_string(),
            name: LABEL.name.to_string(),
        }],
        operation: "provision#AppInstance".to_string(),
        cluster_member: "member-a".to_string(),
        expires_at: 1_800_000_000_000,
        public,
        signature: Vec::new(),
    };
    lease.sign(&server);
    let lease_key = server.verifying_key();
    let approval = lease::approve(
        ApproveInput {
            lease: &lease,
            lease_key: &lease_key,
            checked: &[lease_key],
            vault_key: &vault_key,
            wrapped_item_keys: &[("item-1", &wrapped)],
            approver_user_id: "approver",
            approver: &approver,
            now: 1_799_990_000_000,
        },
        &mut rng,
    )?;
    Ok(json!({
        "server_seed": h(server.seed().as_bytes()),
        "lease_public": h(lease.public.as_bytes()),
        "lease_secret": h(lease_secret.as_key().as_bytes()),
        "lease_signature": h(&lease.signature),
        "approver_public": h(&approver.public().to_bytes()),
        "sealed_item_key": h(&approval.sealed[0].sealed),
        "approval_signature": h(&approval.signature),
        "item_key": h(item_key.as_bytes()),
    }))
}

fn machine_vector() -> Value {
    let mut rng = TestRng::new("vectors machine");
    let machine_key = machine::machine_key_from_crypt_pass(b"field pass");
    let item_key = Key32::random(&mut rng);
    let sealed = machine::seal(&machine_key, "env-demo", "vault-1", "item-1", &item_key, &mut rng);
    json!({
        "crypt_pass": h(b"field pass"),
        "machine_key": h(machine_key.as_bytes()),
        "item_key": h(item_key.as_bytes()),
        "sealed": h(&sealed),
    })
}

fn shamir_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors shamir");
    let secret = Key32::random(&mut rng);
    let shares = shamir::split(secret.as_bytes(), 3, 5, &mut rng)?;
    let texts: Vec<Value> = shares
        .iter()
        .map(|share| Value::String(share.to_text().to_string()))
        .collect();
    Ok(json!({ "secret": h(secret.as_bytes()), "k": 3, "n": 5, "shares": texts }))
}

fn delivery_vector() -> Result<Value, Error> {
    let mut rng = TestRng::new("vectors delivery");
    let sender = PersonalKey::generate(&mut rng);
    let recipient = PersonalKey::generate(&mut rng);
    let recipient_public = recipient.public();
    let to = To {
        user_id: "bob",
        key_id: "pk-b",
        public: &recipient_public,
    };
    let from = Sender {
        kind: "user",
        id: "alice",
        key: sender.signing_key(),
    };
    let sent = delivery::send("d-1", to, "login", b"{\"v\":1}", from, &mut rng)?;
    Ok(json!({
        "sender_public": h(&sender.public().to_bytes()),
        "recipient_encrypt_secret": h(recipient.encrypt_key().as_key().as_bytes()),
        "recipient_sign_seed": h(recipient.signing_key().seed().as_bytes()),
        "sealed": h(&sent.sealed),
        "signature": h(&sent.signature),
    }))
}

#[cfg(test)]
#[path = "vectors_tests.rs"]
mod tests;
