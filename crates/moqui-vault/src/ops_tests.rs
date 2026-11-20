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
    // the tests' key ids are pk-<user>
    let user = key_id.strip_prefix("pk-").unwrap_or(key_id);
    let request = json!({ "op": "key_check_make", "checked_key_id": key_id, "checked_user_id": user, "checked_public": public,
                          "fingerprint": fingerprint, "checked_at": 1 });
    call(session, rng, request).unwrap()
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
    let wrong = json!({ "op": "key_check_make", "checked_key_id": "pk-carol", "checked_user_id": "carol", "checked_public": carol_made["public"],
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

/// `session`'s signed check of `user`'s personal key `key_id` (`public`), after comparing its fingerprint.
fn check_as(session: &mut Session, rng: &mut TestRng, user: &str, key_id: &str, public: &Value) -> Value {
    let fingerprint =
        call(session, rng, json!({ "op": "fingerprint", "public": public })).unwrap()["fingerprint"].clone();
    let request = json!({ "op": "key_check_make", "checked_key_id": key_id, "checked_user_id": user, "checked_public": public,
                          "fingerprint": fingerprint, "checked_at": 1 });
    call(session, rng, request).unwrap()
}

#[test]
fn a_recovery_of_k_holders_combines_in_the_module_and_rewraps_to_the_new_key() {
    let mut rng = TestRng::new("ops-recovery");
    let mut s = shared(&mut rng);
    let seal = json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "body": "{\"x\":1}" });
    let sealed = call(&mut s.alice, &mut rng, seal).unwrap();

    // Three holders; h0 makes the recovery key, split 2 of 3, after checking the other two.
    let mut holders: Vec<Session> = (0..3).map(|_| Session::default()).collect();
    let made: Vec<Value> = holders
        .iter_mut()
        .enumerate()
        .map(|(i, h)| make(h, &mut rng, &format!("h{i}")))
        .collect();
    let h0_checks: Vec<Value> = (1..3)
        .map(|i| check(&mut holders[0], &mut rng, &format!("pk-h{i}"), &made[i]["public"]))
        .collect();
    let holder_rows: Vec<Value> = (0..3)
        .map(|i| json!({ "user_id": format!("h{i}"), "key_id": format!("pk-h{i}"), "public": made[i]["public"] }))
        .collect();
    let unchecked = json!({ "op": "recovery_make", "recovery_key_id": "rk", "k": 2, "holders": holder_rows });
    assert!(call(&mut holders[0], &mut rng, unchecked)
        .unwrap_err()
        .contains("has not been checked"));
    let request = json!({ "op": "recovery_make", "recovery_key_id": "rk", "k": 2, "holders": holder_rows,
                         "key_checks": h0_checks });
    let made_key = call(&mut holders[0], &mut rng, request).unwrap();
    let record = made_key["record"].clone();
    let shares = made_key["shares"].as_array().unwrap().clone();
    let maker_public = made[0]["public"].clone();
    let recovery_keys = json!([{ "record": record, "maker_public": maker_public }]);

    // Alice turns recovery on: the recovery key, whose maker she checked, joins the member list as a manager.
    let alice_checks: Vec<Value> = vec![
        check(&mut s.alice, &mut rng, "pk-bob", &s.bob_made["public"]),
        check(&mut s.alice, &mut rng, "pk-h0", &maker_public),
    ];
    let unchecked_member = json!({ "op": "recovery_member", "record": record, "maker_public": maker_public });
    assert!(call(&mut s.alice, &mut rng, unchecked_member).is_err());
    let member_request = json!({ "op": "recovery_member", "record": record, "maker_public": maker_public,
                                "key_checks": alice_checks });
    let recovery_member = call(&mut s.alice, &mut rng, member_request).unwrap();
    assert_eq!(recovery_member["user_id"], json!("recovery:rk"));
    let members = json!([
        { "user_id": "alice", "role": "manage", "key_id": "pk-alice", "public": s.alice_made["public"] },
        { "user_id": "bob", "role": "read", "key_id": "pk-bob", "public": s.bob_made["public"] },
        recovery_member,
    ]);
    let next = json!({ "op": "key_version_next", "entries": [s.entry], "members": members, "version": 1, "revision": 2,
                       "key_checks": alice_checks, "recovery_keys": recovery_keys });
    let entry2 = call(&mut s.alice, &mut rng, next).unwrap();
    let wrap_request = json!({ "op": "vault_key_wrap_member", "vault_id": "v", "key_version": 1,
                              "member_user_id": "recovery:rk", "member_key_id": "rk",
                              "member_public": recovery_member["public"], "key_checks": alice_checks,
                              "recovery_keys": recovery_keys });
    let recovery_wrap = call(&mut s.alice, &mut rng, wrap_request).unwrap();

    // Alice loses her key and makes a new one; h2 checks it, and also knows her old one (a signer of the chain).
    let mut alice2 = Session::default();
    let alice2_made = call(
        &mut alice2,
        &mut rng,
        json!({ "op": "personal_make", "personal_key_id": "pk-alice2", "user_id": "alice", "passphrase": "pass phrase" }),
    )
    .unwrap();
    let h2_checks: Vec<Value> = vec![
        check(&mut holders[2], &mut rng, "pk-h0", &maker_public),
        check(&mut holders[2], &mut rng, "pk-h1", &made[1]["public"]),
        check_as(&mut holders[2], &mut rng, "alice", "pk-alice", &s.alice_made["public"]),
        check_as(&mut holders[2], &mut rng, "alice", "pk-alice2", &alice2_made["public"]),
    ];
    let start = json!({ "op": "recovery_session_start", "session_id": "rs-1", "record": record, "maker_public": maker_public,
                        "held": shares[2], "vault_id": "v", "subject_user_id": "alice", "subject_key_id": "pk-alice2",
                        "subject_public": alice2_made["public"], "key_checks": h2_checks });
    let session = call(&mut holders[2], &mut rng, start).unwrap();

    // Alone, h2's share is one of the two needed.
    let combine = |deliveries: Value| {
        json!({ "op": "recovery_combine", "session_id": "rs-1", "record": record, "maker_public": maker_public,
                "held": shares[2], "deliveries": deliveries, "key_checks": h2_checks })
    };
    assert!(call(&mut holders[2], &mut rng, combine(json!([])))
        .unwrap_err()
        .contains("fewer shares"));

    // h1 checks h2's key, the maker's and Alice's new key, sees the session (started by a holder: h2's row) and seals
    // its share to the session's ephemeral key.
    let mut h1_checks: Vec<Value> = vec![
        check(&mut holders[1], &mut rng, "pk-h0", &maker_public),
        check(&mut holders[1], &mut rng, "pk-h2", &made[2]["public"]),
    ];
    let deliver = json!({ "op": "recovery_share_deliver", "record": record, "maker_public": maker_public, "held": shares[1],
                          "session": session, "session_holder_public": made[2]["public"], "session_holder": shares[2],
                          "key_checks": h1_checks });
    assert!(call(&mut holders[1], &mut rng, deliver.clone())
        .unwrap_err()
        .contains("has not been checked"));
    h1_checks.push(check_as(
        &mut holders[1],
        &mut rng,
        "alice",
        "pk-alice2",
        &alice2_made["public"],
    ));
    let mut deliver = deliver;
    deliver["key_checks"] = json!(h1_checks);
    let mut not_a_holder_row = deliver.clone();
    not_a_holder_row["session_holder"] = shares[0].clone();
    assert!(call(&mut holders[1], &mut rng, not_a_holder_row)
        .unwrap_err()
        .contains("not started by a holder"));
    let sent = call(&mut holders[1], &mut rng, deliver.clone()).unwrap();
    assert!(sent.get("value").is_none() && sent.get("share").is_none());
    // another holder's share does not go out under h1's name, and a session someone else signed is refused
    let mut not_mine = deliver.clone();
    not_mine["held"] = shares[0].clone();
    assert!(call(&mut holders[1], &mut rng, not_mine).is_err());
    let mut forged = deliver.clone();
    forged["session"]["ephemeral"] = made[1]["public"].as_str().unwrap()[..64].into();
    assert!(call(&mut holders[1], &mut rng, forged)
        .unwrap_err()
        .contains("signature"));

    // A delivery under another holder's row is refused; with h1's row, the two shares give the recovery key.
    let delivered = |holder: &Value, public: &Value| json!([{ "share": sent, "holder": holder, "public": public }]);
    assert!(call(
        &mut holders[2],
        &mut rng,
        combine(delivered(&shares[0], &made[0]["public"]))
    )
    .is_err());
    let combined = call(
        &mut holders[2],
        &mut rng,
        combine(delivered(&shares[1], &made[1]["public"])),
    )
    .unwrap();
    assert_eq!(combined, json!({ "recovery_key_id": "rk", "session_id": "rs-1" }));

    // h2 opens the vault key through the chain (anchored at Alice's old key, which h2 checked) and rekeys Alice. The
    // combined key serves only the next open and rekey: any other call drops it, and the combine is made again.
    let entries = json!([s.entry, entry2]);
    let open = json!({ "op": "recovery_open_vault", "entries": entries, "key_checks": h2_checks, "session_id": "rs-1",
                       "wrap": recovery_wrap });
    call(
        &mut holders[2],
        &mut rng,
        json!({ "op": "fingerprint", "public": made[0]["public"] }),
    )
    .unwrap();
    assert!(call(&mut holders[2], &mut rng, open.clone())
        .unwrap_err()
        .contains("no recovery key is combined"));
    let mut elsewhere = open.clone();
    elsewhere["session_id"] = json!("rs-other");
    assert!(call(&mut holders[2], &mut rng, elsewhere).is_err());
    call(
        &mut holders[2],
        &mut rng,
        combine(delivered(&shares[1], &made[1]["public"])),
    )
    .unwrap();
    assert_eq!(
        call(&mut holders[2], &mut rng, open.clone()).unwrap()["key_version"],
        json!(1)
    );
    let rekey = json!({ "op": "recovery_rekey", "session_id": "rs-1", "entries": entries, "key_checks": h2_checks });
    let rekeyed = call(&mut holders[2], &mut rng, rekey.clone()).unwrap();
    assert_eq!(rekeyed["entry"]["signer_user_id"], json!("recovery:rk"));
    // Alice was a manager: the wraps she signed (Bob's, the recovery key's) are remade
    assert_eq!(rekeyed["rewraps"].as_array().unwrap().len(), 2);
    // and then h2's module keeps nothing of the recovery: no vault key, no recovery key, no session
    let status = call(&mut holders[2], &mut rng, json!({ "op": "status" })).unwrap();
    assert_eq!(status["vaults"], json!([]));
    assert!(call(&mut holders[2], &mut rng, open).is_err());
    assert!(call(&mut holders[2], &mut rng, rekey).is_err());

    // Alice's new client accepts the chain through the recovery key (its maker checked) and opens the item.
    let alice2_checks = vec![check(&mut alice2, &mut rng, "pk-h0", &maker_public)];
    let entries3 = json!([s.entry, entry2, rekeyed["entry"]]);
    let opened = call(
        &mut alice2,
        &mut rng,
        json!({ "op": "vault_key_open", "entries": entries3, "key_checks": alice2_checks, "recovery_keys": recovery_keys,
                "wrap": rekeyed["wrap"] }),
    )
    .unwrap();
    assert_eq!(opened["key_version"], json!(1));
    let item = json!({ "op": "item_open", "vault_id": "v", "item_id": "i", "key_version": 1,
                       "wrapped_item_key": sealed["wrapped_item_key"], "sealed_body": sealed["sealed_body"] });
    assert_eq!(call(&mut alice2, &mut rng, item).unwrap()["body"], json!("{\"x\":1}"));
    // Bob's client, which checked Alice's old key, accepts the recovery's entry too.
    let verified = call(
        &mut s.bob,
        &mut rng,
        json!({ "op": "chain_verify", "entries": entries3, "key_checks": [s.bob_checks_alice] }),
    )
    .unwrap();
    assert_eq!(verified["revision"], json!(3));

    // A holder who is also a member has the version open already: h1 recovers through rs-2 with no forget first. The
    // recovery key's wrap must open that same key (another is refused, and the combined key is kept for the open), and
    // the version stays open after the rekey, as before it.
    h1_checks.push(check_as(
        &mut holders[1],
        &mut rng,
        "alice",
        "pk-alice",
        &s.alice_made["public"],
    ));
    let start2 = json!({ "op": "recovery_session_start", "session_id": "rs-2", "record": record, "maker_public": maker_public,
                         "held": shares[1], "vault_id": "v", "subject_user_id": "alice", "subject_key_id": "pk-alice2",
                         "subject_public": alice2_made["public"], "key_checks": h1_checks });
    let session2 = call(&mut holders[1], &mut rng, start2).unwrap();
    let deliver2 = json!({ "op": "recovery_share_deliver", "record": record, "maker_public": maker_public, "held": shares[2],
                           "session": session2, "session_holder_public": made[1]["public"], "session_holder": shares[1],
                           "key_checks": h2_checks });
    let sent2 = call(&mut holders[2], &mut rng, deliver2).unwrap();
    let slot = ("v".to_string(), 1);
    let alice_key = s.alice.vault_keys.get(&slot).unwrap().clone();
    holders[1].vault_keys.insert(slot.clone(), Key32::from_bytes([7; 32]));
    let combine2 = json!({ "op": "recovery_combine", "session_id": "rs-2", "record": record, "maker_public": maker_public,
                           "held": shares[1], "key_checks": h1_checks,
                           "deliveries": [{ "share": sent2, "holder": shares[2], "public": made[2]["public"] }] });
    call(&mut holders[1], &mut rng, combine2).unwrap();
    let open2 = json!({ "op": "recovery_open_vault", "entries": entries, "key_checks": h1_checks, "session_id": "rs-2",
                        "wrap": recovery_wrap });
    assert!(call(&mut holders[1], &mut rng, open2.clone())
        .unwrap_err()
        .contains("another key"));
    holders[1].vault_keys.insert(slot.clone(), alice_key);
    assert_eq!(call(&mut holders[1], &mut rng, open2).unwrap()["key_version"], json!(1));
    let rekey2 = json!({ "op": "recovery_rekey", "session_id": "rs-2", "entries": entries, "key_checks": h1_checks });
    let rekeyed2 = call(&mut holders[1], &mut rng, rekey2).unwrap();
    assert_eq!(rekeyed2["entry"]["signer_user_id"], json!("recovery:rk"));
    let status = call(&mut holders[1], &mut rng, json!({ "op": "status" })).unwrap();
    assert_eq!(status["vaults"], json!([{ "vault_id": "v", "key_version": 1 }]));

    // There is no call that hands back a combined secret.
    let combine_op = json!({ "op": "shamir_combine", "shares": [] });
    assert!(call(&mut holders[2], &mut rng, combine_op)
        .unwrap_err()
        .contains("unknown op"));
}

/// A personal vault's header (its name) is sealed by the vault key of a version, does not open under another, and a
/// rotation seals it again under the new version.
#[test]
fn a_header_is_sealed_by_its_version_and_a_rotation_reseals_it() {
    let mut rng = TestRng::new("ops-header");
    let mut s = shared(&mut rng);
    let sealed = call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "header_seal", "vault_id": "v", "key_version": 1, "header": "{\"name\":\"Family\"}" }),
    )
    .unwrap();
    let open = |version: u64, cipher: &Value| json!({ "op": "header_open", "vault_id": "v", "key_version": version, "header_cipher": cipher });
    assert_eq!(
        call(&mut s.alice, &mut rng, open(1, &sealed["header_cipher"])).unwrap()["header"],
        json!("{\"name\":\"Family\"}")
    );
    let checks = vec![check(&mut s.alice, &mut rng, "pk-bob", &s.bob_made["public"])];
    let members = json!([
        { "user_id": "alice", "role": "manage", "key_id": "pk-alice", "public": s.alice_made["public"] },
        { "user_id": "bob", "role": "read", "key_id": "pk-bob", "public": s.bob_made["public"] },
    ]);
    let rotated = call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "vault_rotate", "entries": [s.entry], "members": members, "items": [], "key_checks": checks,
                "header_cipher": sealed["header_cipher"] }),
    )
    .unwrap();
    assert_eq!(rotated["key_version"], json!(2));
    assert_eq!(
        call(&mut s.alice, &mut rng, open(2, &rotated["header_cipher"])).unwrap()["header"],
        json!("{\"name\":\"Family\"}")
    );
    // the old version's header does not open as the new version's
    assert!(call(&mut s.alice, &mut rng, open(2, &sealed["header_cipher"])).is_err());
}

/// A single-secret vault's generated key wraps by HKDF (no Argon2id) and opens only with that key.
#[test]
fn a_generated_key_wraps_and_opens_a_single_secret_vault() {
    let mut rng = TestRng::new("ops-generated");
    let mut maker = Session::default();
    call(
        &mut maker,
        &mut rng,
        json!({ "op": "vault_key_new", "vault_id": "g", "key_version": 1 }),
    )
    .unwrap();
    let wrapped = call(
        &mut maker,
        &mut rng,
        json!({ "op": "vault_key_generated_wrap", "vault_id": "g", "key_version": 1 }),
    )
    .unwrap();
    let generated = wrapped["generated"].as_str().unwrap().to_string();
    assert_eq!(generated.replace('-', "").len(), 52);
    let open = |key: &str| {
        json!({ "op": "vault_key_generated_open", "vault_id": "g", "key_version": 1, "wrap": wrapped["wrap"],
                "generated": key })
    };
    let mut other = Session::default();
    let wrong = generated.replace(&generated[..4], "0000");
    assert!(call(&mut other, &mut rng, open(&wrong)).is_err());
    assert_eq!(
        call(&mut other, &mut rng, open(&generated)).unwrap()["key_version"],
        json!(1)
    );
    let mut moved = open(&generated);
    moved["vault_id"] = json!("h");
    assert!(call(&mut Session::default(), &mut rng, moved).is_err());
}

/// A list view gets an item's header and unmasked fields only; an item key made before the current key version is
/// replaced at the next seal; a page forgets a vault key version it opened.
#[test]
fn a_summary_holds_no_secret_and_an_old_item_key_is_replaced() {
    let mut rng = TestRng::new("ops-summary");
    let mut s = shared(&mut rng);
    let body = json!({ "v": 1, "type": "login", "name": "Bank", "uris": ["https://bank.example"], "tags": ["money"],
                       "notes": "the PIN is 1234",
                       "fields": { "username": "ada", "password": "hunter2", "pin": "1234", "account": "777" } });
    let sealed = call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "body": body.to_string() }),
    )
    .unwrap();
    let summary = call(
        &mut s.alice,
        &mut rng,
        json!({ "op": "item_summary", "vault_id": "v", "item_id": "i", "key_version": 1, "masked": ["account"],
                "wrapped_item_key": sealed["wrapped_item_key"], "sealed_body": sealed["sealed_body"] }),
    )
    .unwrap();
    assert_eq!(summary["name"], json!("Bank"));
    assert_eq!(summary["fields"], json!({ "username": "ada" }));
    assert!(!summary.to_string().contains("hunter2") && !summary.to_string().contains("1234"));

    // the same key version with an item key made at it is kept; one made at an earlier version is replaced
    let reseal = |made_at: u64| {
        json!({ "op": "item_seal", "vault_id": "v", "item_id": "i", "key_version": 1, "body": "{}",
                "wrapped_item_key": sealed["wrapped_item_key"], "wrapped_key_version": 1, "item_key_version": made_at })
    };
    assert_eq!(
        call(&mut s.alice, &mut rng, reseal(1)).unwrap()["new_item_key"],
        json!(false)
    );
    assert_eq!(
        call(&mut s.alice, &mut rng, reseal(0)).unwrap()["new_item_key"],
        json!(true)
    );

    let forget = json!({ "op": "vault_key_forget", "vault_id": "v", "key_version": 1 });
    assert_eq!(
        call(&mut s.alice, &mut rng, forget.clone()).unwrap()["forgotten"],
        json!(true)
    );
    assert_eq!(call(&mut s.alice, &mut rng, forget).unwrap()["forgotten"], json!(false));
    assert!(call(&mut s.alice, &mut rng, reseal(1)).unwrap_err().contains("locked"));
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

/// V5: device memory opens the personal key in a new session only with the key both halves make (the device's and the
/// server's), and only for its memory, user, time and personal key; a locked session has nothing to keep, and no call
/// of the module hands a page anything of it.
#[test]
fn device_memory_opens_the_personal_key_only_with_its_key_and_memory() {
    let mut rng = TestRng::new("ops-device-memory");
    let mut session = Session::default();
    let device_key = Key32::random(&mut rng);
    let server_secret = Key32::random(&mut rng);
    let wrap_key = personal::device_wrap_key(&device_key, &server_secret, "m1");
    assert!(
        Session::default().device_seal("m1", 1000, &wrap_key, &mut rng).is_err(),
        "a locked session keeps nothing"
    );
    let made = make(&mut session, &mut rng, "alice");
    let kept = session.device_seal("m1", 1000, &wrap_key, &mut rng).unwrap();
    assert!(
        !format!("{kept:?}").contains(&hex(&kept.sealed)),
        "Debug shows no bytes"
    );

    let mut later = Session::default();
    let opened = later.device_open(&kept, "m1", &wrap_key).unwrap();
    assert_eq!(opened["public"], made["public"]);
    assert_eq!(later.personal_key_id(), Some("pk-alice"));
    let status = call(&mut later, &mut rng, json!({ "op": "status" })).unwrap();
    assert_eq!(status["personal"], json!("pk-alice"));

    // either half alone, or another memory's key, opens nothing
    let other = Key32::random(&mut rng);
    for wrong in [
        personal::device_wrap_key(&device_key, &other, "m1"),
        personal::device_wrap_key(&other, &server_secret, "m1"),
        personal::device_wrap_key(&device_key, &server_secret, "m2"),
    ] {
        assert!(Session::default().device_open(&kept, "m1", &wrong).is_err());
    }
    assert!(
        Session::default().device_open(&kept, "m2", &wrap_key).is_err(),
        "another memory"
    );
    let mut renamed = kept.clone();
    renamed.personal_key_id = "pk-mallory".to_string();
    assert!(
        Session::default().device_open(&renamed, "m1", &wrap_key).is_err(),
        "another personal key id"
    );
    let mut other_user = kept.clone();
    other_user.user_id = "mallory".to_string();
    assert!(
        Session::default().device_open(&other_user, "m1", &wrap_key).is_err(),
        "another user"
    );
    let mut longer = kept.clone();
    longer.until += 1;
    assert!(
        Session::default().device_open(&longer, "m1", &wrap_key).is_err(),
        "a time made longer"
    );
    let mut changed = kept.clone();
    let last = changed.sealed.len() - 1;
    changed.sealed[last] ^= 1;
    assert!(
        Session::default().device_open(&changed, "m1", &wrap_key).is_err(),
        "a changed wrap"
    );
    for op in ["device_seal", "device_open"] {
        let refused = call(&mut session, &mut rng, json!({ "op": op, "memory_id": "m1" })).unwrap_err();
        assert_eq!(refused, "bad format: unknown op");
    }
}
