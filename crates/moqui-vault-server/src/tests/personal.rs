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

//! Personal keys and key checks (§3.5, §9 Nov 19).

use moqui_lib::object::Value;
use serde_json::json;

use super::{call, call_as, login, op, setup, strong_login, sys, text, Client};

/// The wrapped personal key goes only to its own user after a strong, recent sign-in: an internal login is refused,
/// with the advice to set up a second factor on an account with a password and none; an SSO sign-in now is taken
/// (and the wrap opens in a new client with the passphrase), one older than vault@key-fetch-max-age is not. A fetch
/// is an event; the public key alone needs no strong sign-in.
#[test]
fn a_wrapped_personal_key_needs_a_strong_recent_sign_in() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let alice = Client::new(&facade, "alice");
    let public = call_as("alice", "get#PersonalKey", json!({})).unwrap();
    assert_eq!(public["personalKeyId"], json!(alice.key_id));
    assert_eq!(public["fetchAllowed"], json!(false));
    assert!(public.get("passphraseWrap").is_none());

    let refused = call_as("alice", "get#WrappedPersonalKey", json!({})).unwrap_err();
    assert!(refused.contains("strongAuthRequired"), "{refused}");
    // an account with a password and no second factor is told to set one up
    sys(|| {
        let mut account = facade
            .find("moqui.security.UserAccount")
            .unwrap()
            .eq("userId", Value::string("alice"))
            .one()
            .unwrap()
            .unwrap();
        account.set("currentPassword", Value::string("not-a-real-hash"));
        account.update(&facade).unwrap();
    });
    let refused = call_as("alice", "get#WrappedPersonalKey", json!({})).unwrap_err();
    assert!(refused.contains("set up a second factor"), "{refused}");

    strong_login("alice", 600_000);
    let stale = call("get#WrappedPersonalKey", json!({})).unwrap_err();
    assert!(stale.contains("strongAuth"), "{stale}");
    strong_login("alice", 0);
    let fetched = call("get#WrappedPersonalKey", json!({})).unwrap();
    let mut fresh = moqui_vault::ops::Session::default();
    let opened = op(
        &mut fresh,
        json!({ "op": "personal_open", "personal_key_id": fetched["personalKeyId"], "user_id": "alice",
                "public": fetched["public"], "passphrase_wrap": fetched["passphraseWrap"], "passphrase": "a long pass phrase" }),
    )
    .unwrap();
    assert_eq!(opened["fingerprint"], alice.made["fingerprint"]);
    let events = call("list#VaultEvents", json!({})).unwrap();
    let kinds: Vec<String> = events["eventList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| text(event, "eventTypeEnumId"))
        .collect();
    assert!(kinds.contains(&"VeKeyFetched".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"VePersonalKeyMade".to_string()), "{kinds:?}");
}

/// A second key is made only by replacing the first, after a strong sign-in; a new passphrase likewise. A key check
/// is stored only signed by its maker's active key and naming a key as it is stored.
#[test]
fn keys_are_replaced_and_rewrapped_after_a_strong_sign_in_and_checks_are_signed_by_their_maker() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let again = json!({ "personalKeyId": super::new_id("pk"), "public": alice.public,
                        "passphraseWrap": alice.made["passphrase_wrap"], "recoveryWrap": alice.made["recovery_wrap"] });
    // setting up a key needs a strong sign-in too, so a weak session or a key cannot plant one
    assert!(alice
        .call("setup#PersonalKey", again.clone())
        .unwrap_err()
        .contains("strongAuth"));
    strong_login("alice", 0);
    assert!(call("setup#PersonalKey", again.clone())
        .unwrap_err()
        .contains("already"));
    let carol_key = super::new_id("pk");
    super::make_user(&facade, "carol");
    let carols = json!({ "personalKeyId": carol_key, "public": alice.public,
                         "passphraseWrap": alice.made["passphrase_wrap"], "recoveryWrap": alice.made["recovery_wrap"] });
    super::login("carol");
    assert!(call("setup#PersonalKey", carols).unwrap_err().contains("strongAuth"));
    assert!(alice
        .call("replace#PersonalKey", again)
        .unwrap_err()
        .contains("strongAuth"));
    let rewrapped = alice
        .op(json!({ "op": "personal_rewrap", "passphrase": "another pass phrase" }))
        .unwrap();
    let rewrap = json!({ "personalKeyId": alice.key_id, "passphraseWrap": rewrapped["passphrase_wrap"] });
    assert!(alice
        .call("rewrap#PersonalKey", rewrap.clone())
        .unwrap_err()
        .contains("strongAuth"));
    strong_login("alice", 0);
    call("rewrap#PersonalKey", rewrap).unwrap();
    // a passphrase wrap under the Argon2id floor is refused
    let mut weak = rewrapped["passphrase_wrap"].clone();
    weak["m_kib"] = json!(1024);
    let refused = call(
        "rewrap#PersonalKey",
        json!({ "personalKeyId": alice.key_id, "passphraseWrap": weak }),
    )
    .unwrap_err();
    assert!(refused.contains("parameters"), "{refused}");

    // a check signed by another key, or of a key not as stored, is refused
    let key = bob.call("get#UserPublicKey", json!({ "userId": "alice" })).unwrap();
    let check = bob
        .op(
            json!({ "op": "key_check_make", "checked_key_id": key["personalKeyId"], "checked_user_id": key["userId"], "checked_public": key["public"],
                    "fingerprint": key["fingerprint"], "checked_at": 1 }),
        )
        .unwrap();
    assert!(alice
        .call("store#KeyCheck", json!({ "check": check.clone() }))
        .unwrap_err()
        .contains("its maker"));
    let mut misnamed = check.clone();
    misnamed["checked_key_id"] = json!(bob.key_id);
    assert!(bob.call("store#KeyCheck", json!({ "check": misnamed })).is_err());
    bob.call("store#KeyCheck", json!({ "check": check })).unwrap();
    assert_eq!(bob.checks().as_array().unwrap().len(), 1);
    login("alice");
    assert_eq!(alice.checks().as_array().unwrap().len(), 0);
    alice.check("bob");
    assert_eq!(alice.checks().as_array().unwrap().len(), 1);
}
