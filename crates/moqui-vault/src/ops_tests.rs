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
use crate::lease::{Lease, LeaseItem};
use crate::recovery::Holder;
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

/// A session with a new personal key; the answer of `personal_make`.
fn make(session: &mut Session, rng: &mut TestRng, user: &str) -> Value {
    let request = json!({ "op": "personal_make", "personal_key_id": format!("pk-{user}"), "user_id": user, "passphrase": "pass phrase" });
    call(session, rng, request).unwrap()
}

/// `session`'s signed check of the personal key `public`, after comparing its fingerprint.
fn check(session: &mut Session, rng: &mut TestRng, key_id: &str, public: &Value) -> Value {
    let fingerprint =
        call(session, rng, json!({ "op": "fingerprint", "public": public })).unwrap()["fingerprint"].clone();
    let request = json!({ "op": "key_check_make", "checked_key_id": key_id, "checked_public": public, "fingerprint": fingerprint, "checked_at": 1 });
    call(session, rng, request).unwrap()
}

fn verify_half(made: &Value) -> String {
    made["public"].as_str().unwrap()[64..].to_string()
}

/// Alice makes vault `v` (1.1) with Bob as a reader, checks Bob first, and wraps the key to him.
struct Shared {
    alice: Session,
    bob: Session,
    alice_made: Value,
    bob_made: Value,
    entry: Value,
    wrap: Value,
    bob_checks_alice: Value,
}

fn shared(rng: &mut TestRng) -> Shared {
    let (mut alice, mut bob) = (Session::default(), Session::default());
    let alice_made = make(&mut alice, rng, "alice");
    let bob_made = make(&mut bob, rng, "bob");
    let alice_checks_bob = check(&mut alice, rng, "pk-bob", &bob_made["public"]);
    let members = json!([
        { "user_id": "alice", "role": "manage", "key_id": "pk-alice", "public": alice_made["public"] },
        { "user_id": "bob", "role": "read", "key_id": "pk-bob", "public": bob_made["public"] },
    ]);
    let first =
        json!({ "op": "key_version_first", "vault_id": "v", "members": members, "key_checks": [alice_checks_bob] });
    let entry = call(&mut alice, rng, first).unwrap();
    call(
        &mut alice,
        rng,
        json!({ "op": "vault_key_new", "vault_id": "v", "key_version": 1 }),
    )
    .unwrap();
    let wrap = call(
        &mut alice,
        rng,
        json!({ "op": "vault_key_wrap_member", "vault_id": "v", "key_version": 1, "member_user_id": "bob",
                "member_key_id": "pk-bob", "member_public": bob_made["public"], "key_checks": [alice_checks_bob] }),
    )
    .unwrap();
    let bob_checks_alice = check(&mut bob, rng, "pk-alice", &alice_made["public"]);
    Shared {
        alice,
        bob,
        alice_made,
        bob_made,
        entry,
        wrap,
        bob_checks_alice,
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
    let made = make(&mut session, &mut rng, "alice");
    assert_eq!(made["fingerprint"].as_str().unwrap().split('-').count(), 6);
    assert!(made["recovery_code"].is_string());
    call(&mut session, &mut rng, json!({ "op": "lock" })).unwrap();
    assert_eq!(
        call(&mut session, &mut rng, json!({ "op": "status" })).unwrap()["personal"],
        Value::Null
    );

    let open = json!({
        "op": "personal_open", "personal_key_id": "pk-alice", "user_id": "alice", "public": made["public"],
        "passphrase_wrap": made["passphrase_wrap"], "passphrase": "pass phrase",
    });
    let opened = call(&mut session, &mut rng, open.clone()).unwrap();
    assert_eq!(opened["public"], made["public"]);
    let mut wrong = open;
    wrong["passphrase"] = json!("not it");
    assert!(call(&mut Session::default(), &mut rng, wrong)
        .unwrap_err()
        .contains("does not open"));
}

#[test]
fn a_vault_key_opens_only_through_the_checked_chain() {
    let mut rng = TestRng::new("ops-member");
    let mut s = shared(&mut rng);
    let open =
        json!({ "op": "vault_key_open", "entries": [s.entry], "key_checks": [s.bob_checks_alice], "wrap": s.wrap });
    let opened = call(&mut s.bob, &mut rng, open.clone()).unwrap();
    assert_eq!(opened["vault_id"], json!("v"));
    assert_eq!(opened["remembered"]["version"], json!(1));

    // With no check of Alice's key, or a check that this session did not sign, the chain has no anchor.
    let mut fresh = Session::default();
    let bob_again = json!({
        "op": "personal_open", "personal_key_id": "pk-bob", "user_id": "bob", "public": s.bob_made["public"],
        "passphrase_wrap": s.bob_made["passphrase_wrap"], "passphrase": "pass phrase",
    });
    call(&mut fresh, &mut rng, bob_again).unwrap();
    let mut unchecked = open.clone();
    unchecked["key_checks"] = json!([]);
    assert!(call(&mut fresh, &mut rng, unchecked).unwrap_err().contains("checked"));
    let mut carol = Session::default();
    make(&mut carol, &mut rng, "carol2");
    let not_mine = check(&mut carol, &mut rng, "pk-alice", &s.alice_made["public"]);
    let mut foreign = open.clone();
    foreign["key_checks"] = json!([not_mine]);
    assert!(call(&mut fresh, &mut rng, foreign).unwrap_err().contains("checked"));
    // A wrap signed by a key the chain does not name as a manager is refused, whatever the page says.
    let mut forged = s.wrap.clone();
    forged["signer_user_id"] = json!("bob");
    forged["signer_key_id"] = json!("pk-bob");
    let mut forged_open = open.clone();
    forged_open["wrap"] = forged;
    assert!(call(&mut fresh, &mut rng, forged_open).unwrap_err().contains("manage"));
    // An already-open vault version is not replaced.
    assert!(call(&mut s.bob, &mut rng, open).unwrap_err().contains("already open"));
}

#[test]
fn nothing_is_wrapped_to_an_unchecked_key_and_a_check_needs_the_right_fingerprint() {
    let mut rng = TestRng::new("ops-unchecked");
    let mut s = shared(&mut rng);
    let carol_made = make(&mut Session::default(), &mut rng, "carol");
    let wrap = json!({ "op": "vault_key_wrap_member", "vault_id": "v", "key_version": 1, "member_user_id": "carol",
                       "member_key_id": "pk-carol", "member_public": carol_made["public"], "key_checks": [] });
    assert!(call(&mut s.alice, &mut rng, wrap)
        .unwrap_err()
        .contains("not been checked"));
    let wrong = json!({ "op": "key_check_make", "checked_key_id": "pk-carol", "checked_public": carol_made["public"],
                        "fingerprint": s.bob_made["fingerprint"], "checked_at": 1 });
    assert!(call(&mut s.alice, &mut rng, wrong).unwrap_err().contains("fingerprint"));
    // There is no call that signs arbitrary bytes.
    let sign = json!({ "op": "sign", "kind": "key-version", "parts": ["00"] });
    assert!(call(&mut s.alice, &mut rng, sign).unwrap_err().contains("unknown op"));
}

#[test]
fn items_seal_open_and_get_a_new_key_after_a_rotation() {
    let mut rng = TestRng::new("ops-items");
    let mut s = shared(&mut rng);
    let label = json!({ "item_type": "login", "binding": "b", "name": "n" });
    let seal = json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label, "body": "{\"v\":1}" });
    let sealed = call(&mut s.alice, &mut rng, seal).unwrap();
    assert_eq!(sealed["new_item_key"], json!(true));
    let open = json!({ "op": "item_open", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label,
                       "wrapped_item_key": sealed["wrapped_item_key"], "sealed_body": sealed["sealed_body"] });
    assert_eq!(
        call(&mut s.alice, &mut rng, open.clone()).unwrap()["body"],
        json!("{\"v\":1}")
    );
    let mut moved = open.clone();
    moved["label"]["binding"] = json!("other");
    assert!(call(&mut s.alice, &mut rng, moved).is_err());
    let again = json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label, "body": "{}",
                        "wrapped_item_key": sealed["wrapped_item_key"], "wrapped_key_version": 1 });
    assert_eq!(
        call(&mut s.alice, &mut rng, again).unwrap()["new_item_key"],
        json!(false)
    );
    assert!(call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "vault_key_new", "vault_id": "v", "key_version": 1 })
    )
    .unwrap_err()
    .contains("already open"));

    // Alice removes Bob: a new key version, the item key rewrapped, a new deposit key; the old body still opens.
    let members =
        json!([{ "user_id": "alice", "role": "manage", "key_id": "pk-alice", "public": s.alice_made["public"] }]);
    let rotate = json!({ "op": "vault_rotate", "entries": [s.entry], "members": members, "operational": true,
                         "items": [{ "item_id": "i", "label": label, "wrapped_item_key": sealed["wrapped_item_key"] }] });
    let rotated = call(&mut s.alice, &mut rng, rotate).unwrap();
    assert_eq!(rotated["key_version"], json!(2));
    assert_eq!(rotated["entry"]["version"], json!(2));
    assert_eq!(rotated["deposit"]["deposit"]["key_version"], json!(2));
    let mut after = open.clone();
    after["key_version"] = json!(2);
    after["wrapped_item_key"] = rotated["item_keys"][0]["wrapped_item_key"].clone();
    assert_eq!(call(&mut s.alice, &mut rng, after).unwrap()["body"], json!("{\"v\":1}"));
    // The first write after the change makes a new item key, not the old one under the new version.
    let stale = json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 2, "label": label, "body": "{}",
                        "wrapped_item_key": sealed["wrapped_item_key"], "wrapped_key_version": 1 });
    assert_eq!(
        call(&mut s.alice, &mut rng, stale).unwrap()["new_item_key"],
        json!(true)
    );

    call(&mut s.alice, &mut rng, json!({ "op": "lock" })).unwrap();
    assert!(call(&mut s.alice, &mut rng, open).unwrap_err().contains("locked"));
}

#[test]
fn a_lease_is_approved_only_under_a_checked_lease_key_and_its_time_limit() {
    let mut rng = TestRng::new("ops-lease");
    let mut s = shared(&mut rng);
    let label = json!({ "item_type": "database-credential", "binding": "AppInstance:1:dbPassword", "name": "db" });
    let seal =
        json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label, "body": "{}" });
    let sealed = call(&mut s.alice, &mut rng, seal).unwrap();
    let server = SigningKey::random(&mut rng);
    let (_, public) = lease::new_lease_key(&mut rng).unwrap();
    let expires_at = 10 * 60 * 60 * 1000;
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
        expires_at,
        public,
        signature: Vec::new(),
    };
    signed.sign(&server);
    let lease_json = json!({
        "lease_id": "l", "vault_id": "v", "key_version": 1, "operation": "deploy", "cluster_member": "m", "expires_at": expires_at,
        "items": [{ "item_id": "i", "item_type": "database-credential", "binding": "AppInstance:1:dbPassword", "name": "db" }],
        "public": hex(signed.public.as_bytes()), "signature": hex(&signed.signature),
    });
    let server_key = hex(server.verifying_key().as_bytes());
    let fingerprint = fingerprint::lease_key(&server.verifying_key());
    let lease_check = call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "key_check_make", "lease_key": server_key, "fingerprint": fingerprint, "checked_at": 1 }),
    )
    .unwrap();
    let approve = |checks: Value, now: i64| {
        json!({ "op": "lease_approve", "lease": lease_json, "lease_key": server_key, "key_checks": checks, "now": now,
                "wrapped_item_keys": { "i": sealed["wrapped_item_key"] } })
    };
    let approved = call(
        &mut s.alice,
        &mut rng,
        approve(json!([lease_check]), expires_at - 60_000),
    )
    .unwrap();
    assert_eq!(approved["sealed"][0]["item_id"], json!("i"));
    assert!(call(&mut s.alice, &mut rng, approve(json!([]), expires_at - 60_000))
        .unwrap_err()
        .contains("not been checked"));
    // A lease key check another user signed is not this session's.
    assert!(call(&mut s.bob, &mut rng, approve(json!([lease_check]), expires_at - 60_000)).is_err());
    assert!(call(&mut s.alice, &mut rng, approve(json!([lease_check]), expires_at))
        .unwrap_err()
        .contains("expired"));
    assert!(call(&mut s.alice, &mut rng, approve(json!([lease_check]), 0))
        .unwrap_err()
        .contains("4 hours"));
}

#[test]
fn a_recovery_combines_in_the_module_and_opens_the_vault_there() {
    let mut rng = TestRng::new("ops-recovery");
    let mut s = shared(&mut rng);
    let label = json!({ "item_type": "login", "binding": "b", "name": "n" });
    let seal = json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label, "body": "{\"x\":1}" });
    let sealed = call(&mut s.alice, &mut rng, seal).unwrap();

    // Three holders; the recovery key is split 2 of 3 by a maker whose key each of them checks.
    let mut holders: Vec<Session> = (0..3).map(|_| Session::default()).collect();
    let made: Vec<Value> = holders
        .iter_mut()
        .enumerate()
        .map(|(i, h)| make(h, &mut rng, &format!("h{i}")))
        .collect();
    let publics: Vec<PersonalPublic> = made
        .iter()
        .map(|m| PersonalPublic::from_bytes(&unhex(m["public"].as_str().unwrap()).unwrap()).unwrap())
        .collect();
    let ids: Vec<String> = (0..3).map(|i| format!("h{i}")).collect();
    let key_ids: Vec<String> = (0..3).map(|i| format!("pk-h{i}")).collect();
    let list: Vec<Holder> = (0..3)
        .map(|i| Holder {
            user_id: &ids[i],
            key_id: &key_ids[i],
            public: &publics[i],
        })
        .collect();
    let maker = PersonalKey::generate(&mut rng);
    let maker_public = hex(&maker.public().to_bytes());
    let signer = Signer {
        user_id: "maker",
        key_id: "pk-maker",
        key: &maker,
    };
    let (record, shares) = recovery::make("rk", 2, &list, signer, &mut rng).unwrap();
    let record_json = json!({ "recovery_key_id": "rk", "public": hex(record.public.as_bytes()), "k": 2, "n": 3,
                              "maker_user_id": "maker", "maker_key_id": "pk-maker", "signature": hex(&record.signature) });
    let held_json = |i: usize| {
        json!({ "recovery_key_id": "rk", "holder_user_id": shares[i].holder_user_id, "holder_key_id": shares[i].holder_key_id,
                "index": shares[i].index, "sealed": hex(&shares[i].sealed), "signature": hex(&shares[i].signature) })
    };

    // Alice turns recovery on, having checked the maker's key.
    let alice_checks_maker = check(&mut s.alice, &mut rng, "pk-maker", &json!(maker_public));
    let wrap = call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "recovery_wrap", "vault_id": "v", "key_version": 1, "record": record_json,
                "maker_public": maker_public, "key_checks": [alice_checks_maker] }),
    )
    .unwrap();

    // Holders 0 and 1 deliver their shares to holder 2, whose key they checked; the shares never reach a page.
    let mut deliveries = Vec::new();
    for i in 0..2 {
        let checks_h2 = check(&mut holders[i], &mut rng, "pk-h2", &made[2]["public"]);
        let sent = call(
            &mut holders[i],
            &mut rng,
            json!({ "op": "recovery_share_deliver", "held": held_json(i), "maker_public": maker_public,
                    "delivery_id": format!("d{i}"), "to_user_id": "h2", "to_key_id": "pk-h2",
                    "to_public": made[2]["public"], "key_checks": [checks_h2] }),
        )
        .unwrap();
        assert!(sent.get("value").is_none() && sent.get("share").is_none());
        let mut row = sent.clone();
        row["sender_verify"] = json!(verify_half(&made[i]));
        deliveries.push(row);
    }
    let h2_checks_maker = check(&mut holders[2], &mut rng, "pk-maker", &json!(maker_public));
    // The same delivery twice counts once: with only it, two copies of share 0 are not two shares.
    let doubled = json!([deliveries[0], deliveries[0]]);
    let combine = |rows: Value| {
        json!({ "op": "recovery_combine", "record": record_json, "maker_public": maker_public,
                "key_checks": [h2_checks_maker], "deliveries": rows })
    };
    assert!(call(&mut holders[2], &mut rng, combine(doubled)).is_err());
    let combined = call(&mut holders[2], &mut rng, combine(json!(deliveries))).unwrap();
    assert_eq!(combined, json!({ "recovery_key_id": "rk" }));

    let h2_checks_alice = check(&mut holders[2], &mut rng, "pk-alice", &s.alice_made["public"]);
    let opened = call(
        &mut holders[2],
        &mut rng,
        json!({ "op": "recovery_open_vault", "entries": [s.entry], "key_checks": [h2_checks_alice],
                "record": record_json, "wrap": wrap }),
    )
    .unwrap();
    assert_eq!(opened["key_version"], json!(1));
    let open = json!({ "op": "item_open", "vault_id": "v", "item_id": "i", "key_version": 1, "label": label,
                       "wrapped_item_key": sealed["wrapped_item_key"], "sealed_body": sealed["sealed_body"] });
    assert_eq!(
        call(&mut holders[2], &mut rng, open).unwrap()["body"],
        json!("{\"x\":1}")
    );
    // There is no call that hands back a combined secret.
    let combine_op = json!({ "op": "shamir_combine", "shares": [] });
    assert!(call(&mut holders[2], &mut rng, combine_op)
        .unwrap_err()
        .contains("unknown op"));
}

#[test]
fn verify_and_bad_requests() {
    let mut rng = TestRng::new("ops-misc");
    let mut session = Session::default();
    let key = SigningKey::random(&mut rng);
    let signature = key.sign(crate::sign::Statement::KeyCheck, &[&[1], &[2]]);
    let request = json!({ "op": "verify", "verify": hex(key.verifying_key().as_bytes()), "kind": "key-check",
                          "parts": ["01", "02"], "signature": hex(&signature) });
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

#[test]
fn a_delivery_opens_only_for_its_recipient_under_a_checked_sender_and_once() {
    let mut rng = TestRng::new("ops-delivery");
    let mut s = shared(&mut rng);
    let alice_checks_bob = check(&mut s.alice, &mut rng, "pk-bob", &s.bob_made["public"]);
    let send = json!({
        "op": "delivery_send", "delivery_id": "d1", "to_user_id": "bob", "to_key_id": "pk-bob",
        "to_public": s.bob_made["public"], "purpose": "item", "value": "{\"v\":1}", "key_checks": [alice_checks_bob],
    });
    let sent = call(&mut s.alice, &mut rng, send.clone()).unwrap();
    assert_eq!(sent["sender_id"], json!("alice"));
    // nothing is sealed to a key the sender has not checked
    let mut unchecked = send;
    unchecked["key_checks"] = json!([]);
    assert!(call(&mut s.alice, &mut rng, unchecked).unwrap_err().contains("checked"));

    let open = json!({ "op": "delivery_open", "delivery": sent, "sender_public": s.alice_made["public"],
                       "key_checks": [s.bob_checks_alice] });
    // a sender key Bob has not checked, and a changed signature, are refused
    let mut no_check = open.clone();
    no_check["key_checks"] = json!([]);
    assert!(call(&mut s.bob, &mut rng, no_check).unwrap_err().contains("checked"));
    let mut forged = open.clone();
    let mut signature = forged["delivery"]["signature"].as_str().unwrap().to_string();
    let flipped = if signature.starts_with('0') { "1" } else { "0" };
    signature.replace_range(0..1, flipped);
    forged["delivery"]["signature"] = json!(signature);
    assert!(call(&mut s.bob, &mut rng, forged).unwrap_err().contains("signature"));
    // the recipient opens it once; a repeat is refused, and Alice cannot open what is sealed to Bob
    let opened = call(&mut s.bob, &mut rng, open.clone()).unwrap();
    assert_eq!(opened["value"], json!("{\"v\":1}"));
    assert!(call(&mut s.bob, &mut rng, open.clone())
        .unwrap_err()
        .contains("already"));
    assert!(call(&mut s.alice, &mut rng, open).unwrap_err().contains("someone else"));
}

#[test]
fn a_new_passphrase_rewraps_the_open_key() {
    let mut rng = TestRng::new("ops-rewrap");
    let mut session = Session::default();
    let made = make(&mut session, &mut rng, "alice");
    let rewrapped = call(
        &mut session,
        &mut rng,
        json!({ "op": "personal_rewrap", "passphrase": "new phrase" }),
    )
    .unwrap();
    let open = |phrase: &str| {
        json!({ "op": "personal_open", "personal_key_id": "pk-alice", "user_id": "alice", "public": made["public"],
                "passphrase_wrap": rewrapped["passphrase_wrap"], "passphrase": phrase })
    };
    let opened = call(&mut Session::default(), &mut rng, open("new phrase")).unwrap();
    assert_eq!(opened["public"], made["public"]);
    assert!(call(&mut Session::default(), &mut rng, open("pass phrase")).is_err());
    let locked = json!({ "op": "personal_rewrap", "passphrase": "x" });
    assert!(call(&mut Session::default(), &mut rng, locked)
        .unwrap_err()
        .contains("locked"));
}
