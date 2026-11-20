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

use super::*;
use crate::rng::TestRng;
use crate::sign::SigningKey;

fn call(session: &mut Session, rng: &mut TestRng, request: Value) -> Result<Value, String> {
    let answer: Value = serde_json::from_str(&session.call(&request.to_string(), rng)).unwrap();
    if answer["ok"] == json!(true) {
        Ok(answer["result"].clone())
    } else {
        Err(answer["error"].as_str().unwrap().to_string())
    }
}

#[test]
fn the_vectors_call_returns_the_checked_in_vectors() {
    let mut rng = TestRng::new("ops-vectors");
    let made = call(&mut Session::default(), &mut rng, json!({ "op": "vectors" })).unwrap();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/vectors.json");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(made, file);
}

#[test]
fn a_personal_key_is_made_locked_and_opened_by_its_passphrase() {
    let mut rng = TestRng::new("ops-personal");
    let mut session = Session::default();
    let made = call(
        &mut session,
        &mut rng,
        json!({ "op": "personal_make", "personal_key_id": "pk-1", "user_id": "alice", "passphrase": "a long passphrase" }),
    )
    .unwrap();
    assert_eq!(made["fingerprint"].as_str().unwrap().split('-').count(), 6);
    assert!(made["recovery_code"].is_string());
    call(&mut session, &mut rng, json!({ "op": "lock" })).unwrap();
    let status = call(&mut session, &mut rng, json!({ "op": "status" })).unwrap();
    assert_eq!(status["personal"], Value::Null);

    let open = json!({
        "op": "personal_open", "personal_key_id": "pk-1", "user_id": "alice", "public": made["public"],
        "passphrase_wrap": made["passphrase_wrap"], "passphrase": "a long passphrase",
    });
    let opened = call(&mut session, &mut rng, open.clone()).unwrap();
    assert_eq!(opened["public"], made["public"]);
    assert_eq!(opened["rewrapped"], Value::Null);
    let mut wrong = open;
    wrong["passphrase"] = json!("not it");
    assert!(call(&mut Session::default(), &mut rng, wrong)
        .unwrap_err()
        .contains("does not open"));
}

#[test]
fn items_seal_and_open_only_while_the_vault_is_open() {
    let mut rng = TestRng::new("ops-items");
    let mut session = Session::default();
    call(
        &mut session,
        &mut rng,
        json!({ "op": "vault_key_new", "vault_id": "v", "key_version": 1 }),
    )
    .unwrap();
    let label = json!({ "item_type": "login", "binding": "b", "name": "n" });
    let sealed = call(
        &mut session,
        &mut rng,
        json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label, "body": "{\"v\":1}" }),
    )
    .unwrap();
    let open = json!({
        "op": "item_open", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label,
        "wrapped_item_key": sealed["wrapped_item_key"], "sealed_body": sealed["sealed_body"],
    });
    assert_eq!(
        call(&mut session, &mut rng, open.clone()).unwrap()["body"],
        json!("{\"v\":1}")
    );
    let mut moved = open.clone();
    moved["label"]["binding"] = json!("other");
    assert!(call(&mut session, &mut rng, moved).is_err());
    call(&mut session, &mut rng, json!({ "op": "lock" })).unwrap();
    assert!(call(&mut session, &mut rng, open).unwrap_err().contains("locked"));
}

#[test]
fn a_member_wrap_crosses_from_one_session_to_another() {
    let mut rng = TestRng::new("ops-member");
    let mut alice = Session::default();
    let mut bob = Session::default();
    let make = |id: &str, user: &str| json!({ "op": "personal_make", "personal_key_id": id, "user_id": user, "passphrase": "pass phrase" });
    let alice_key = call(&mut alice, &mut rng, make("pk-a", "alice")).unwrap();
    let bob_key = call(&mut bob, &mut rng, make("pk-b", "bob")).unwrap();
    call(
        &mut alice,
        &mut rng,
        json!({ "op": "vault_key_new", "vault_id": "v", "key_version": 1 }),
    )
    .unwrap();
    let wrap = call(
        &mut alice,
        &mut rng,
        json!({ "op": "vault_key_wrap_member", "vault_id": "v", "key_version": 1, "member_user_id": "bob",
                "member_key_id": "pk-b", "member_public": bob_key["public"] }),
    )
    .unwrap();
    let alice_verify = &alice_key["public"].as_str().unwrap()[64..];
    let opened = call(
        &mut bob,
        &mut rng,
        json!({ "op": "vault_key_open_member", "wrap": wrap, "signer_verify": alice_verify }),
    )
    .unwrap();
    assert_eq!(opened["vault_id"], json!("v"));
    let bob_verify = &bob_key["public"].as_str().unwrap()[64..];
    let refused = call(
        &mut bob,
        &mut rng,
        json!({ "op": "vault_key_open_member", "wrap": wrap, "signer_verify": bob_verify }),
    );
    assert!(refused.unwrap_err().contains("signature"));
}

#[test]
fn a_lease_is_approved_in_the_module() {
    let mut rng = TestRng::new("ops-lease");
    let mut session = Session::default();
    let make =
        json!({ "op": "personal_make", "personal_key_id": "pk-a", "user_id": "approver", "passphrase": "p p p" });
    let me = call(&mut session, &mut rng, make).unwrap();
    call(
        &mut session,
        &mut rng,
        json!({ "op": "vault_key_new", "vault_id": "v", "key_version": 1 }),
    )
    .unwrap();
    let label = json!({ "item_type": "database-credential", "binding": "AppInstance:1:dbPassword", "name": "db" });
    let sealed = call(
        &mut session,
        &mut rng,
        json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label, "body": "{}" }),
    )
    .unwrap();
    let server = SigningKey::random(&mut rng);
    let (lease_secret, public) = lease::new_lease_key(&mut rng).unwrap();
    let mut signed = Lease {
        lease_id: "l".to_string(),
        vault_id: "v".to_string(),
        key_version: 1,
        items: vec![LeaseItem {
            item_id: "i".to_string(),
            item_type: "database-credential".to_string(),
            binding: "AppInstance:1:dbPassword".to_string(),
            name: "db".to_string(),
        }],
        operation: "deploy".to_string(),
        cluster_member: "m".to_string(),
        expires_at: 5,
        public,
        signature: Vec::new(),
    };
    signed.sign(&server);
    let lease_json = json!({
        "lease_id": "l", "vault_id": "v", "key_version": 1, "operation": "deploy", "cluster_member": "m", "expires_at": 5,
        "items": [{ "item_id": "i", "item_type": "database-credential", "binding": "AppInstance:1:dbPassword", "name": "db" }],
        "public": hex(signed.public.as_bytes()), "signature": hex(&signed.signature),
    });
    let server_key = hex(server.verifying_key().as_bytes());
    let approved = call(
        &mut session,
        &mut rng,
        json!({ "op": "lease_approve", "lease": lease_json, "lease_key": server_key, "checked": [server_key],
                "wrapped_item_keys": { "i": sealed["wrapped_item_key"] } }),
    )
    .unwrap();
    let approval = lease::Approval {
        lease_id: "l".to_string(),
        sealed: vec![lease::SealedItemKey {
            item_id: "i".to_string(),
            sealed: unhex(approved["sealed"][0]["sealed"].as_str().unwrap()).unwrap(),
        }],
        approver_user_id: "approver".to_string(),
        signature: unhex(approved["signature"].as_str().unwrap()).unwrap(),
    };
    let approver = PersonalPublic::from_bytes(&unhex(me["public"].as_str().unwrap()).unwrap()).unwrap();
    let keys = lease::open_approval(&signed, &lease_secret, &approval, &approver.verify).unwrap();
    assert_eq!(keys.len(), 1);
    // Not checked: refused in the module.
    let unchecked = call(
        &mut session,
        &mut rng,
        json!({ "op": "lease_approve", "lease": lease_json, "lease_key": server_key, "checked": [],
                "wrapped_item_keys": { "i": sealed["wrapped_item_key"] } }),
    );
    assert!(unchecked.unwrap_err().contains("not been checked"));
}

#[test]
fn shares_signatures_and_bad_requests() {
    let mut rng = TestRng::new("ops-misc");
    let mut session = Session::default();
    let split = call(
        &mut session,
        &mut rng,
        json!({ "op": "shamir_split", "secret": "00112233", "k": 2, "n": 3 }),
    )
    .unwrap();
    let two = json!([split["shares"][0], split["shares"][2]]);
    let combined = call(&mut session, &mut rng, json!({ "op": "shamir_combine", "shares": two })).unwrap();
    assert_eq!(combined["secret"], json!("00112233"));

    let make = json!({ "op": "personal_make", "personal_key_id": "pk", "user_id": "u", "passphrase": "p" });
    let me = call(&mut session, &mut rng, make).unwrap();
    let signed = call(
        &mut session,
        &mut rng,
        json!({ "op": "sign", "kind": "key-check", "parts": ["01", "02"] }),
    )
    .unwrap();
    let verify = &me["public"].as_str().unwrap()[64..];
    let request = json!({ "op": "verify", "verify": verify, "kind": "key-check", "parts": ["01", "02"], "signature": signed["signature"] });
    assert_eq!(
        call(&mut session, &mut rng, request.clone()).unwrap()["valid"],
        json!(true)
    );
    let mut other = request;
    other["kind"] = json!("lease");
    assert_eq!(call(&mut session, &mut rng, other).unwrap()["valid"], json!(false));

    assert!(call(&mut session, &mut rng, json!({ "op": "nope" })).is_err());
    assert!(call(&mut session, &mut rng, json!({})).is_err());
    let answer: Value = serde_json::from_str(&session.call("not json", &mut rng)).unwrap();
    assert_eq!(answer["ok"], json!(false));
}
