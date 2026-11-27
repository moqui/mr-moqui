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

//! Organization recovery, split k of n (plans/archive/Vault-Instances.md §3.10, V4's Tests column): k holders recover a vault
//! to a member's new personal key and every member is told; k − 1 cannot; a single-secret vault never gets a recovery
//! wrap; a holder's step without `VAULT_RECOVERY` or a recent sign-in is refused; an operational vault cannot drop to
//! one member with recovery off.

use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{
    add_item, all_text, call, event_kinds, new_id, open_vault, setup, shared_vault, strong_login, sys, text, Client,
};

/// Puts `users` in a group that holds `permission`.
fn grant(facade: &EntityFacade, group: &str, permission: &str, users: &[&str]) {
    sys(|| {
        let mut rows = vec![
            ("moqui.security.UserGroup", json!({ "userGroupId": group })),
            (
                "moqui.security.UserGroupPermission",
                json!({ "userGroupId": group, "userPermissionId": permission, "fromDate": 0 }),
            ),
        ];
        for user in users {
            rows.push((
                "moqui.security.UserGroupMember",
                json!({ "userGroupId": group, "userId": user, "fromDate": 0 }),
            ));
        }
        for (entity, fields) in rows {
            let mut row = facade.make_value(entity).unwrap();
            for (name, given) in fields.as_object().unwrap() {
                row.set(name.as_str(), moqui_lib::json::from_json(given));
            }
            row.create(facade).unwrap();
        }
    });
}

/// `call` as `user` right after a strong sign-in: a recent one.
fn recent(user: &str, name: &str, parameters: Json) -> Result<Json, String> {
    strong_login(user, 0);
    call(name, parameters)
}

/// A strong sign-in ten minutes ago, in a new session: not a recent one.
fn stale_login(user: &str) {
    sys(|| moqui_core::user::logout_local().unwrap());
    strong_login(user, 10 * 60 * 1000);
}

/// A recovery key made by `holders[0]`, split `k` of the holders, after it checked the others.
struct Org {
    recovery_key_id: String,
    record: Json,
    maker_public: String,
}

impl Org {
    fn recovery_keys(&self) -> Json {
        json!([{ "record": self.record, "maker_public": self.maker_public }])
    }
}

fn make_org(holders: &mut [&mut Client], k: u64) -> Org {
    let others: Vec<String> = holders[1..].iter().map(|h| h.user.clone()).collect();
    for other in &others {
        holders[0].check(other);
    }
    let rows: Vec<Json> = holders
        .iter()
        .map(|h| json!({ "user_id": h.user, "key_id": h.key_id, "public": h.public }))
        .collect();
    let recovery_key_id = new_id("rk");
    let checks = holders[0].checks();
    let made = holders[0]
        .op(
            json!({ "op": "recovery_make", "recovery_key_id": recovery_key_id, "k": k, "holders": rows,
                    "key_checks": checks }),
        )
        .unwrap();
    recent(
        &holders[0].user,
        "create#VaultRecoveryKey",
        json!({ "record": made["record"], "shares": made["shares"], "userGroupId": "VAULT_TEST_HOLDERS" }),
    )
    .unwrap();
    Org {
        recovery_key_id,
        record: made["record"].clone(),
        maker_public: holders[0].public.clone(),
    }
}

/// `manager` turns recovery on for a vault: checks the recovery key's maker, lists the recovery key in the next member
/// list and wraps the vault key to it (the vault key open in their client).
fn turn_recovery_on(manager: &mut Client, vault_id: &str, org: &Org, maker: &str) -> Result<Json, String> {
    manager.check(maker);
    let checks = manager.checks();
    let member = manager.op(
        json!({ "op": "recovery_member", "record": org.record, "maker_public": org.maker_public,
                                    "key_checks": checks }),
    )?;
    let vault = manager.call("get#Vault", json!({ "vaultId": vault_id }))?;
    let entries = vault["entries"].as_array().unwrap().clone();
    let latest = entries.last().unwrap();
    let mut members = latest["members"].as_array().unwrap().clone();
    members.push(member.clone());
    let version = latest["version"].as_u64().unwrap();
    let entry = manager.op(
        json!({ "op": "key_version_next", "entries": entries, "members": members, "version": version,
                                   "revision": latest["revision"].as_u64().unwrap() + 1, "key_checks": checks,
                                   "recovery_keys": org.recovery_keys() }),
    )?;
    let wrap = manager.op(
        json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": version,
                                  "member_user_id": member["user_id"], "member_key_id": member["key_id"],
                                  "member_public": member["public"], "key_checks": checks,
                                  "recovery_keys": org.recovery_keys() }),
    )?;
    manager.call(
        "add#VaultMember",
        json!({ "vaultId": vault_id, "expectedRevision": vault["vault"]["revision"], "entry": entry, "wrap": wrap }),
    )
}

/// The held share of `user` from the recovery key list.
fn held(user: &str, recovery_key_id: &str) -> Json {
    let keys = recent(user, "list#VaultRecoveryKeys", json!({})).unwrap();
    let key = keys["recoveryKeyList"]
        .as_array()
        .unwrap()
        .iter()
        .find(|key| text(key, "recoveryKeyId") == recovery_key_id)
        .cloned()
        .unwrap();
    key["holders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|holder| text(holder, "userId") == user)
        .unwrap()["held"]
        .clone()
}

/// The notifications of the recovery topic `user` was sent.
fn recovery_notices(facade: &EntityFacade, user: &str) -> usize {
    sys(|| {
        let messages = facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", Value::string("VaultRecovery"))
            .list()
            .unwrap();
        messages
            .iter()
            .filter(|message| {
                let id = moqui_lib::object::to_plain_string(message.get("notificationMessageId").unwrap());
                facade
                    .find("moqui.security.user.NotificationMessageUser")
                    .unwrap()
                    .eq("notificationMessageId", Value::string(id))
                    .eq("userId", Value::string(user))
                    .one()
                    .unwrap()
                    .is_some()
            })
            .count()
    })
}

/// k holders recover a vault to a member's new personal key, and every member is told; k − 1 cannot.
#[test]
fn k_holders_recover_a_vault_to_a_members_new_key_and_every_member_is_told() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let mut h0 = Client::new(&facade, "h0");
    let mut h1 = Client::new(&facade, "h1");
    let mut h2 = Client::new(&facade, "h2");
    grant(&facade, "VAULT_TEST_HOLDERS", "VAULT_RECOVERY", &["h0", "h1", "h2"]);
    let org = make_org(&mut [&mut h0, &mut h1, &mut h2], 2);
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "write")]);
    let body = json!({ "v": 1, "type": "note", "name": "n", "fields": { "text": "the vault's secret" } });
    let item_id = add_item(&mut alice, &vault_id, 1, &body);
    turn_recovery_on(&mut alice, &vault_id, &org, "h0").unwrap();
    let vault = alice.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    assert_eq!(vault["vault"]["recoveryOn"], json!("Y"));

    // h1 has checked Alice's key before she loses it (a signer of the vault's chain) and the recovery key's maker
    h1.check("alice");
    h1.check("h0");
    // Alice loses her passphrase and recovery code, and makes a new personal key
    let mut alice2 = moqui_vault::ops::Session::default();
    let alice2_key = new_id("pk");
    let made = super::op(
        &mut alice2,
        json!({ "op": "personal_make", "personal_key_id": alice2_key, "user_id": "alice", "passphrase": "another pass phrase" }),
    )
    .unwrap();
    recent(
        "alice",
        "replace#PersonalKey",
        json!({ "personalKeyId": alice2_key, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                "recoveryWrap": made["recovery_wrap"] }),
    )
    .unwrap();
    // nothing is wrapped to the new key before h1 checks it again
    let session_id = new_id("rs");
    let start = |h1: &mut Client| {
        let checks = h1.checks();
        h1.op(
            json!({ "op": "recovery_session_start", "session_id": session_id, "record": org.record,
                      "maker_public": org.maker_public, "held": held("h1", &org.recovery_key_id), "vault_id": vault_id,
                      "subject_user_id": "alice", "subject_key_id": alice2_key, "subject_public": made["public"],
                      "key_checks": checks }),
        )
    };
    assert!(start(&mut h1).unwrap_err().contains("has not been checked"));
    h1.check("alice");
    let session = start(&mut h1).unwrap();
    recent("h1", "start#VaultRecovery", json!({ "session": session })).unwrap();

    // k - 1 = 1: h1's share alone does not combine, and the server refuses to complete
    let open = recent(
        "h1",
        "get#VaultRecoverySession",
        json!({ "recoverySessionId": session_id }),
    )
    .unwrap();
    let combine = |h1: &mut Client, open: &Json| {
        let checks = h1.checks();
        h1.op(
            json!({ "op": "recovery_combine", "session_id": session_id, "record": open["record"],
                      "maker_public": open["makerPublic"], "held": open["held"], "deliveries": open["deliveries"],
                      "key_checks": checks }),
        )
    };
    assert!(combine(&mut h1, &open).unwrap_err().contains("fewer shares"));
    let early = recent(
        "h1",
        "complete#VaultRecovery",
        json!({ "recoverySessionId": session_id, "expectedRevision": open["revision"], "entry": { "x": 1 },
                "wrap": { "x": 1 } }),
    )
    .unwrap_err();
    assert!(early.contains("0 of the 1 other holders"), "{early}");

    // h2 sees the session, checks h1's key, the maker's and Alice's new key, and seals its share to the session
    h2.check("h1");
    h2.check("h0");
    h2.check("alice");
    let listed = recent("h2", "list#VaultRecoverySessions", json!({})).unwrap();
    let row = listed["sessionList"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| text(row, "recoverySessionId") == session_id)
        .cloned()
        .unwrap();
    let checks = h2.checks();
    let share = h2
        .op(
            json!({ "op": "recovery_share_deliver", "record": org.record, "maker_public": org.maker_public,
                    "held": held("h2", &org.recovery_key_id), "session": row["session"],
                    "session_holder_public": row["holderPublic"], "session_holder": row["holderShare"],
                    "key_checks": checks }),
        )
        .unwrap();
    let deliver = json!({ "recoverySessionId": session_id, "share": share });
    recent("h2", "deliver#VaultRecoveryShare", deliver.clone()).unwrap();
    assert!(recent("h2", "deliver#VaultRecoveryShare", deliver)
        .unwrap_err()
        .contains("already"));

    // h1 combines, opens the vault key through the recovery key's wrap, and gives Alice her new key
    let open = recent(
        "h1",
        "get#VaultRecoverySession",
        json!({ "recoverySessionId": session_id }),
    )
    .unwrap();
    combine(&mut h1, &open).unwrap();
    let checks = h1.checks();
    h1.op(
        json!({ "op": "recovery_open_vault", "entries": open["entries"], "key_checks": checks,
                  "session_id": session_id, "wrap": open["recoveryWrap"] }),
    )
    .unwrap();
    let rekeyed = h1
        .op(json!({ "op": "recovery_rekey", "session_id": session_id, "entries": open["entries"], "key_checks": checks }))
        .unwrap();
    recent(
        "h1",
        "complete#VaultRecovery",
        json!({ "recoverySessionId": session_id, "expectedRevision": open["revision"], "entry": rekeyed["entry"],
                "wrap": rekeyed["wrap"], "rewraps": rekeyed["rewraps"] }),
    )
    .unwrap();

    // Alice's new client checks the recovery key's maker, opens the vault through the chain and reads the item
    let vault = super::call_as("alice", "get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let wrap = vault["wraps"].as_array().unwrap()[0].clone();
    assert_eq!(wrap["member_key_id"], json!(alice2_key));
    let key = super::call_as("alice", "get#UserPublicKey", json!({ "userId": "h0" })).unwrap();
    let alice2_check = super::op(
        &mut alice2,
        json!({ "op": "key_check_make", "checked_key_id": key["personalKeyId"], "checked_user_id": "h0",
                "checked_public": key["public"], "fingerprint": key["fingerprint"], "checked_at": 1 }),
    )
    .unwrap();
    super::op(
        &mut alice2,
        json!({ "op": "vault_key_open", "entries": vault["entries"], "key_checks": [alice2_check],
                "recovery_keys": org.recovery_keys(), "wrap": wrap }),
    )
    .unwrap();
    let items = super::call_as("alice", "list#VaultItems", json!({ "vaultId": vault_id })).unwrap();
    let row = items["itemList"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| text(row, "vaultItemId") == item_id)
        .cloned()
        .unwrap();
    let opened = super::op(
        &mut alice2,
        json!({ "op": "item_open", "vault_id": vault_id, "item_id": item_id, "key_version": 1,
                "wrapped_item_key": row["wrappedItemKey"], "sealed_body": row["bodyCipher"] }),
    )
    .unwrap();
    assert!(text(&opened, "body").contains("the vault's secret"));
    // Bob's wrap, which Alice's old key signed, was remade by the recovery key, and his client still opens the vault
    open_vault(&mut bob, &vault_id).unwrap();

    // every member was told, at the start and at the end, and Alice, a manager, to replace the used recovery key;
    // each step is an event
    assert_eq!(recovery_notices(&facade, "alice"), 3);
    assert_eq!(recovery_notices(&facade, "bob"), 2);
    assert_eq!(recovery_notices(&facade, "h2"), 0);
    let keys = super::call_as("bob", "list#VaultRecoveryKeys", json!({})).unwrap();
    assert_eq!(keys["recoveryKeyList"][0]["used"], json!(true));
    let kinds = event_kinds(&bob, &vault_id);
    for kind in ["VeRecoveryStarted", "VeRecoveryShared", "VeRecovery"] {
        assert!(kinds.iter().any(|k| k == kind), "{kind} in {kinds:?}");
    }
    let session_rows = all_text(&facade, "moqui.security.vault.VaultRecoverySession");
    assert!(session_rows.iter().any(|text| text == "VrsCompleted"));
}

/// Each holder's step needs `VAULT_RECOVERY` and a recent sign-in: making a recovery key, starting a recovery and
/// sealing a share to one.
#[test]
fn a_holders_step_without_vault_recovery_or_a_recent_sign_in_is_refused() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut h0 = Client::new(&facade, "h0");
    let mut h1 = Client::new(&facade, "h1");
    let h2 = Client::new(&facade, "h2");
    super::make_user(&facade, "outsider");
    grant(&facade, "VAULT_TEST_HOLDERS", "VAULT_RECOVERY", &["h0", "h1"]);
    h0.check("h1");
    h0.check("h2");
    let rows: Vec<Json> = [&h0, &h1, &h2]
        .iter()
        .map(|h| json!({ "user_id": h.user, "key_id": h.key_id, "public": h.public }))
        .collect();
    let checks = h0.checks();
    let made = h0
        .op(
            json!({ "op": "recovery_make", "recovery_key_id": new_id("rk"), "k": 2, "holders": rows,
                    "key_checks": checks }),
        )
        .unwrap();
    let create = json!({ "record": made["record"], "shares": made["shares"] });
    // h2 does not hold VAULT_RECOVERY, so no share goes to them
    let refused = recent("h0", "create#VaultRecoveryKey", create.clone()).unwrap_err();
    assert!(
        refused.contains("h2 does not hold the VAULT_RECOVERY permission"),
        "{refused}"
    );
    let outsider_made = recent("outsider", "create#VaultRecoveryKey", create.clone()).unwrap_err();
    assert!(outsider_made.contains("VAULT_RECOVERY"), "{outsider_made}");
    grant(&facade, "VAULT_TEST_LATE", "VAULT_RECOVERY", &["h2"]);
    // a sign-in ten minutes old is not recent
    stale_login("h0");
    let stale = call("create#VaultRecoveryKey", create.clone()).unwrap_err();
    assert!(stale.contains("reauthRequired"), "{stale}");
    recent("h0", "create#VaultRecoveryKey", create).unwrap();

    // a recovery to start and a share to seal: refused without the permission or a recent sign-in
    let mut alice = Client::new(&facade, "alice");
    let org = Org {
        recovery_key_id: made["record"]["recovery_key_id"].as_str().unwrap().to_string(),
        record: made["record"].clone(),
        maker_public: h0.public.clone(),
    };
    let mut bob = Client::new(&facade, "bob");
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "read")]);
    turn_recovery_on(&mut alice, &vault_id, &org, "h0").unwrap();
    h1.check("alice");
    h1.check("h0");
    h1.check("bob");
    let session_id = new_id("rs");
    // a recovery of Bob to a key he holds already is refused by the server; here the client is asked for a session
    let checks = h1.checks();
    let session = h1
        .op(
            json!({ "op": "recovery_session_start", "session_id": session_id, "record": org.record,
                    "maker_public": org.maker_public, "held": held("h1", &org.recovery_key_id), "vault_id": vault_id,
                    "subject_user_id": "bob", "subject_key_id": bob.key_id, "subject_public": bob.public,
                    "key_checks": checks }),
        )
        .unwrap();
    let not_new = recent("h1", "start#VaultRecovery", json!({ "session": session })).unwrap_err();
    assert!(not_new.contains("a recovery is to a new key"), "{not_new}");
    stale_login("h1");
    let stale = call("start#VaultRecovery", json!({ "session": session })).unwrap_err();
    assert!(stale.contains("reauthRequired"), "{stale}");
    let by_outsider = recent("outsider", "start#VaultRecovery", json!({ "session": session })).unwrap_err();
    assert!(by_outsider.contains("VAULT_RECOVERY"), "{by_outsider}");
    let sessions = recent("outsider", "list#VaultRecoverySessions", json!({})).unwrap_err();
    assert!(sessions.contains("VAULT_RECOVERY"), "{sessions}");
    let shares = recent(
        "outsider",
        "deliver#VaultRecoveryShare",
        json!({ "recoverySessionId": session_id, "share": { "x": 1 } }),
    )
    .unwrap_err();
    assert!(shares.contains("VAULT_RECOVERY"), "{shares}");
}

/// A single-secret vault never gets a recovery wrap, and an operational vault cannot drop to one member with recovery
/// off.
#[test]
fn a_single_secret_vault_gets_no_recovery_and_an_operational_vault_keeps_it_or_two_members() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut h0 = Client::new(&facade, "h0");
    let mut h1 = Client::new(&facade, "h1");
    grant(&facade, "VAULT_TEST_HOLDERS", "VAULT_RECOVERY", &["h0", "h1"]);
    grant(&facade, "VAULT_TEST_ADMIN", "VAULT_ADMIN", &["alice"]);
    let org = make_org(&mut [&mut h0, &mut h1], 2);
    alice.check("h0");
    let checks = alice.checks();
    let member = alice
        .op(
            json!({ "op": "recovery_member", "record": org.record, "maker_public": org.maker_public,
                    "key_checks": checks }),
        )
        .unwrap();

    // a single-secret vault has no member list for the recovery key to join
    let secret_id = new_id("vault");
    let single = json!({ "vaultId": secret_id, "vaultKindEnumId": "VkPersonal", "lockEnumId": "VlSingleSecret",
                         "secretKindEnumId": "VsGenerated", "secretWrap": { "sealed": "00" } });
    alice.call("create#Vault", single).unwrap();
    let entry = alice
        .op(
            json!({ "op": "key_version_first", "vault_id": secret_id, "members": [alice.member("manage"), member],
                    "key_checks": checks, "recovery_keys": org.recovery_keys() }),
        )
        .unwrap();
    let joined = alice
        .call(
            "add#VaultMember",
            json!({ "vaultId": secret_id, "expectedRevision": 1, "entry": entry, "wrap": entry }),
        )
        .unwrap_err();
    assert!(joined.contains("has no members"), "{joined}");
    h1.check("alice");
    h1.check("h0");
    let checks_h1 = h1.checks();
    let session = h1
        .op(
            json!({ "op": "recovery_session_start", "session_id": new_id("rs"), "record": org.record,
                    "maker_public": org.maker_public, "held": held("h1", &org.recovery_key_id), "vault_id": secret_id,
                    "subject_user_id": "alice", "subject_key_id": alice.key_id, "subject_public": alice.public,
                    "key_checks": checks_h1 }),
        )
        .unwrap();
    let refused = recent("h1", "start#VaultRecovery", json!({ "session": session })).unwrap_err();
    assert!(refused.contains("A single-secret vault has no recovery"), "{refused}");
    assert!(!all_text(&facade, "moqui.security.vault.VaultMember")
        .iter()
        .any(|text| text.starts_with("recovery:")));

    // an operational vault of one member with the recovery key listed
    let vault_id = new_id("vault");
    let members = json!([alice.member("manage"), member]);
    let entry = alice
        .op(
            json!({ "op": "key_version_first", "vault_id": vault_id, "members": members, "key_checks": checks,
                    "recovery_keys": org.recovery_keys() }),
        )
        .unwrap();
    alice
        .op(json!({ "op": "vault_key_new", "vault_id": vault_id, "key_version": 1 }))
        .unwrap();
    let wraps: Vec<Json> = members
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            alice
                .op(
                    json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1,
                            "member_user_id": m["user_id"], "member_key_id": m["key_id"], "member_public": m["public"],
                            "key_checks": checks, "recovery_keys": org.recovery_keys() }),
                )
                .unwrap()
        })
        .collect();
    alice
        .call(
            "create#Vault",
            json!({ "vaultId": vault_id, "vaultKindEnumId": "VkOperational", "lockEnumId": "VlSharedKey",
                    "vaultName": "prod", "entry": entry, "memberWraps": wraps }),
        )
        .unwrap();
    // turning recovery off (removing the recovery key, which rotates the key) would leave one member: refused
    let vault = alice.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let rotated = alice
        .op(
            json!({ "op": "vault_rotate", "entries": vault["entries"], "members": [alice.member("manage")],
                    "items": [], "operational": true, "key_checks": checks }),
        )
        .unwrap();
    let off = alice
        .call(
            "remove#VaultMember",
            json!({ "vaultId": vault_id, "userId": format!("recovery:{}", org.recovery_key_id),
                    "expectedRevision": vault["vault"]["revision"], "entry": rotated["entry"],
                    "itemKeys": rotated["item_keys"], "memberWraps": rotated["member_wraps"] }),
        )
        .unwrap_err();
    assert!(off.contains("recovery on or at least two members"), "{off}");

    // replacing the recovery key (after a recovery used it): the old one removed and a new one listed in one rotation
    let org2 = make_org(&mut [&mut h0, &mut h1], 2);
    let checks = alice.checks();
    let member2 = alice
        .op(
            json!({ "op": "recovery_member", "record": org2.record, "maker_public": org2.maker_public,
                    "key_checks": checks }),
        )
        .unwrap();
    // the failed removal's rotation is undone in the client first
    alice
        .op(json!({ "op": "vault_key_forget", "vault_id": vault_id, "key_version": 2 }))
        .unwrap();
    let rotated = alice
        .op(
            json!({ "op": "vault_rotate", "entries": vault["entries"], "members": [alice.member("manage"), member2],
                    "items": [], "operational": true, "key_checks": checks, "recovery_keys": org2.recovery_keys() }),
        )
        .unwrap();
    alice
        .call(
            "remove#VaultMember",
            json!({ "vaultId": vault_id, "userId": format!("recovery:{}", org.recovery_key_id),
                    "expectedRevision": vault["vault"]["revision"], "entry": rotated["entry"],
                    "itemKeys": rotated["item_keys"], "memberWraps": rotated["member_wraps"],
                    "deposit": rotated["deposit"]["deposit"], "depositWrappedPrivate": rotated["deposit"]["wrapped_private"] }),
        )
        .unwrap();
    let replaced = alice.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    assert_eq!(replaced["vault"]["recoveryOn"], json!("Y"));
    let listed: Vec<String> = replaced["memberList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| text(m, "userId"))
        .collect();
    assert!(
        listed.contains(&format!("recovery:{}", org2.recovery_key_id)),
        "{listed:?}"
    );
    assert!(
        !listed.contains(&format!("recovery:{}", org.recovery_key_id)),
        "{listed:?}"
    );
}

/// Any person of the vault's member list may cancel a recovery they did not ask for, and a user id that names a
/// recovery key acts in no vault.
#[test]
fn a_member_cancels_a_recovery_and_the_recovery_prefix_is_reserved() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let mut h0 = Client::new(&facade, "h0");
    let mut h1 = Client::new(&facade, "h1");
    grant(&facade, "VAULT_TEST_HOLDERS", "VAULT_RECOVERY", &["h0", "h1"]);
    let org = make_org(&mut [&mut h0, &mut h1], 2);
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "read")]);
    turn_recovery_on(&mut alice, &vault_id, &org, "h0").unwrap();
    h1.check("bob");
    h1.check("h0");
    // Bob makes a new key; h1 checks it and starts a recovery of Bob
    let mut bob2 = moqui_vault::ops::Session::default();
    let bob2_key = new_id("pk");
    let made = super::op(
        &mut bob2,
        json!({ "op": "personal_make", "personal_key_id": bob2_key, "user_id": "bob", "passphrase": "another pass phrase" }),
    )
    .unwrap();
    recent(
        "bob",
        "replace#PersonalKey",
        json!({ "personalKeyId": bob2_key, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                "recoveryWrap": made["recovery_wrap"] }),
    )
    .unwrap();
    h1.check("bob");
    let session_id = new_id("rs");
    let checks = h1.checks();
    let session = h1
        .op(
            json!({ "op": "recovery_session_start", "session_id": session_id, "record": org.record,
                    "maker_public": org.maker_public, "held": held("h1", &org.recovery_key_id), "vault_id": vault_id,
                    "subject_user_id": "bob", "subject_key_id": bob2_key, "subject_public": made["public"],
                    "key_checks": checks }),
        )
        .unwrap();
    recent("h1", "start#VaultRecovery", json!({ "session": session })).unwrap();
    // a stranger cannot cancel it; Alice, a member who did not ask for it, can
    let carol = Client::new(&facade, "carol");
    let refused = carol
        .call("cancel#VaultRecovery", json!({ "recoverySessionId": session_id }))
        .unwrap_err();
    assert!(refused.contains("No recovery session"), "{refused}");
    alice
        .call("cancel#VaultRecovery", json!({ "recoverySessionId": session_id }))
        .unwrap();
    let ended = recent(
        "h0",
        "deliver#VaultRecoveryShare",
        json!({ "recoverySessionId": session_id, "share": { "x": 1 } }),
    )
    .unwrap_err();
    assert!(ended.contains("ended"), "{ended}");
    assert!(event_kinds(&alice, &vault_id)
        .iter()
        .any(|k| k == "VeRecoveryCancelled"));

    super::make_user(&facade, "recovery:rk-imposter");
    let reserved = super::call_as("recovery:rk-imposter", "list#Vaults", json!({})).unwrap_err();
    assert!(reserved.contains("reserved for recovery keys"), "{reserved}");
}
