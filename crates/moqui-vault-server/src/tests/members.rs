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

//! Members, the chain and rotation (§3.9, decisions (a) and (b) of §9).

use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use moqui_vault::keyversion::{KeyVersion, Member, Role};
use moqui_vault::ops::shapes;
use moqui_vault::personal::PersonalKey;
use moqui_vault::rng::OsRng;
use moqui_vault::text::hex;
use moqui_vault::vault::{self, Recipient, Signer};
use serde_json::{json, Value as Json};

use super::{
    add_item, call, event_kinds, login, make_user, new_id, open_vault, setup, shared_vault, strong_login, sys, text,
    Client,
};

/// The rotation `remover` makes to remove `removed`: the next entry, every item key rewrapped and each remaining
/// member's wrap, from the vault as the server gives it.
fn rotation(remover: &mut Client, vault_id: &str, removed: &str) -> Json {
    let vault = remover.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let latest = vault["entries"].as_array().unwrap().last().unwrap().clone();
    let members: Vec<Json> = latest["members"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["user_id"] != json!(removed))
        .cloned()
        .collect();
    let items = remover.call("list#VaultItems", json!({ "vaultId": vault_id })).unwrap();
    let items: Vec<Json> = items["itemList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| json!({ "item_id": item["vaultItemId"], "wrapped_item_key": item["wrappedItemKey"] }))
        .collect();
    let checks = remover.checks();
    let rotated = remover
        .op(
            json!({ "op": "vault_rotate", "entries": vault["entries"], "key_checks": checks, "members": members,
                    "items": items }),
        )
        .unwrap();
    json!({ "vaultId": vault_id, "userId": removed, "expectedRevision": vault["vault"]["revision"],
            "entry": rotated["entry"], "itemKeys": rotated["item_keys"], "memberWraps": rotated["member_wraps"] })
}

/// A removal rotates the key in one compare-and-swap: a stale revision or a rotation that leaves out an item is
/// refused; afterwards the removed member has no wrap and is refused the vault, a write at the old key version is
/// refused, and the others keep their earlier wraps until they open the new version. Each change writes its event.
#[test]
fn a_removed_member_has_no_wrap_and_a_write_at_the_old_key_version_is_refused() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let mut carol = Client::new(&facade, "carol");
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "write"), (&mut carol, "read")]);
    let body = json!({ "v": 1, "type": "note", "name": "one", "fields": {} });
    let first = add_item(&mut alice, &vault_id, 1, &body);
    add_item(&mut alice, &vault_id, 1, &body);
    open_vault(&mut bob, &vault_id).unwrap();

    let removal = rotation(&mut alice, &vault_id, "bob");
    let mut stale = removal.clone();
    stale["expectedRevision"] = json!(0);
    assert!(alice
        .call("remove#VaultMember", stale)
        .unwrap_err()
        .contains("changed since"));
    let mut partial = removal.clone();
    partial["itemKeys"].as_array_mut().unwrap().pop();
    assert!(alice
        .call("remove#VaultMember", partial)
        .unwrap_err()
        .contains("every item"));
    let mut myself = removal.clone();
    myself["userId"] = json!("alice");
    assert!(alice
        .call("remove#VaultMember", myself)
        .unwrap_err()
        .contains("Another manager"));
    let removed = alice.call("remove#VaultMember", removal).unwrap();
    assert_eq!(removed["keyVersion"], json!(2));

    // bob: no wrap of any version, and the vault is not found to him
    let bobs: Vec<String> = sys(|| {
        facade
            .find("moqui.security.vault.VaultMember")
            .unwrap()
            .eq("userId", Value::string("bob"))
            .list()
            .unwrap()
            .iter()
            .map(|row| super::super::common::field(row, "vaultId"))
            .collect()
    });
    assert!(bobs.is_empty(), "{bobs:?}");
    assert!(bob
        .call("get#Vault", json!({ "vaultId": vault_id }))
        .unwrap_err()
        .contains("No vault"));
    assert!(bob.call("list#VaultItems", json!({ "vaultId": vault_id })).is_err());

    // a write at the old key version is refused, a new item too; at the new one it gets a new item key
    let old = alice
        .op(json!({ "op": "item_seal", "vault_id": vault_id, "item_id": first, "key_version": 1, "body": "{}" }))
        .unwrap();
    let write = json!({ "vaultItemId": first, "expectedRevision": 1, "keyVersion": 1,
                        "wrappedItemKey": old["wrapped_item_key"], "bodyCipher": old["sealed_body"] });
    assert!(alice
        .call("update#VaultItem", write.clone())
        .unwrap_err()
        .contains("key changed"));
    let mut new_item = write;
    new_item["vaultId"] = json!(vault_id);
    new_item["vaultItemId"] = json!(new_id("item"));
    assert!(alice
        .call("create#VaultItem", new_item)
        .unwrap_err()
        .contains("key changed"));

    // carol holds both versions until she opens the new one; her item key opens under it
    let vault = carol.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    assert_eq!(vault["wraps"].as_array().unwrap().len(), 2);
    let opened = open_vault(&mut carol, &vault_id).unwrap();
    assert_eq!(opened["key_version"], json!(2));
    let items = carol.call("list#VaultItems", json!({ "vaultId": vault_id })).unwrap();
    let item = items["itemList"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["vaultItemId"] == json!(first))
        .unwrap()
        .clone();
    let body = carol
        .op(
            json!({ "op": "item_open", "vault_id": vault_id, "item_id": first, "key_version": 2,
                    "wrapped_item_key": item["wrappedItemKey"], "sealed_body": item["bodyCipher"] }),
        )
        .unwrap();
    assert!(body["body"].as_str().unwrap().contains("one"));
    carol
        .call("ack#VaultKeyVersion", json!({ "vaultId": vault_id, "keyVersion": 2 }))
        .unwrap();
    let vault = carol.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    assert_eq!(vault["wraps"].as_array().unwrap().len(), 1);

    let kinds = event_kinds(&alice, &vault_id);
    for kind in [
        "VeVaultCreated",
        "VeItemCreated",
        "VeMemberRemoved",
        "VeKeyRotated",
        "VeKeyVersionAcked",
    ] {
        assert!(kinds.contains(&kind.to_string()), "{kind} not in {kinds:?}");
    }
}

/// Nothing is wrapped to a changed personal key before it is checked again: the manager's client refuses to sign a
/// member list with the unchecked key, the server refuses a wrap to it until the manager's check of it is stored, and
/// then takes it.
#[test]
fn nothing_is_wrapped_to_a_changed_personal_key_before_it_is_checked_again() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "read")]);

    // bob loses his passphrase and recovery code and makes a new key
    let key_id = new_id("pk");
    let mut session = moqui_vault::ops::Session::default();
    let made = super::op(
        &mut session,
        json!({ "op": "personal_make", "personal_key_id": key_id, "user_id": "bob", "passphrase": "a new pass phrase" }),
    )
    .unwrap();
    strong_login("bob", 0);
    call(
        "replace#PersonalKey",
        json!({ "personalKeyId": key_id, "public": made["public"], "passphraseWrap": made["passphrase_wrap"],
                "recoveryWrap": made["recovery_wrap"] }),
    )
    .unwrap();
    bob.session = session;
    bob.key_id = key_id;
    bob.public = text(&made, "public");

    let vault = alice.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let members = json!([alice.member("manage"), bob.member("read")]);
    let next = json!({ "op": "key_version_next", "entries": vault["entries"], "key_checks": alice.checks(),
                       "members": members, "version": 1, "revision": 2 });
    assert!(alice.op(next.clone()).unwrap_err().contains("checked"));

    // a check made in alice's client but not stored: her client signs, the server refuses
    let key = alice.call("get#UserPublicKey", json!({ "userId": "bob" })).unwrap();
    let local = alice
        .op(
            json!({ "op": "key_check_make", "checked_key_id": key["personalKeyId"], "checked_public": key["public"],
                    "fingerprint": key["fingerprint"], "checked_at": 2 }),
        )
        .unwrap();
    let mut checks = alice.checks().as_array().unwrap().clone();
    checks.push(local.clone());
    let mut signed = next;
    signed["key_checks"] = json!(checks);
    let entry = alice.op(signed).unwrap();
    let wrap = alice
        .op(
            json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1, "member_user_id": "bob",
                    "member_key_id": bob.key_id, "member_public": bob.public, "key_checks": checks }),
        )
        .unwrap();
    let add =
        json!({ "vaultId": vault_id, "expectedRevision": vault["vault"]["revision"], "entry": entry, "wrap": wrap });
    let refused = alice.call("add#VaultMember", add.clone()).unwrap_err();
    assert!(refused.contains("Check bob's key fingerprint"), "{refused}");
    alice.call("store#KeyCheck", json!({ "check": local })).unwrap();
    alice.call("add#VaultMember", add.clone()).unwrap();
    // the same change again is stale
    assert!(alice
        .call("add#VaultMember", add)
        .unwrap_err()
        .contains("changed since"));
    // bob's new key opens the vault once he has checked alice with it
    assert!(open_vault(&mut bob, &vault_id).is_err());
    bob.check("alice");
    open_vault(&mut bob, &vault_id).unwrap();
    assert!(event_kinds(&alice, &vault_id).contains(&"VeMemberAdded".to_string()));
}

/// A user whose personal key the test holds, so it can sign as a member gone bad.
fn raw_user(facade: &EntityFacade, user: &str) -> (PersonalKey, String) {
    make_user(facade, user);
    let key = PersonalKey::generate(&mut OsRng);
    let key_id = new_id("pk");
    let public = key.public();
    sys(|| {
        let mut row = facade.make_value("moqui.security.vault.VaultPersonalKey").unwrap();
        row.set("personalKeyId", Value::string(&key_id));
        row.set("userId", Value::string(user));
        row.set("encryptPublic", Value::string(hex(public.encrypt.as_bytes())));
        row.set("verifyPublic", Value::string(hex(public.verify.as_bytes())));
        row.set("statusId", Value::string("VpkActive"));
        row.create(facade).unwrap();
    });
    (key, key_id)
}

/// A member list and a wrap that no manager signed are refused: by the server, though a database writer made the
/// signer a manager in its rows; and by a member's client, when a database writer stores them.
#[test]
fn a_key_version_or_wrap_no_manager_signed_is_refused_by_the_server_and_the_client() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "read")]);
    let (mallory, mallory_key) = raw_user(&facade, "mallory");
    let (_, eve_key) = raw_user(&facade, "eve");

    // alice adds mallory as a writer, the proper way
    alice.check("mallory");
    let vault = alice.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let mallory_public = hex(&mallory.public().to_bytes());
    let mallory_member =
        json!({ "user_id": "mallory", "role": "write", "key_id": mallory_key, "public": mallory_public });
    let members = json!([alice.member("manage"), bob.member("read"), mallory_member]);
    let checks = alice.checks();
    let entry = alice
        .op(
            json!({ "op": "key_version_next", "entries": vault["entries"], "key_checks": checks, "members": members,
                    "version": 1, "revision": 2 }),
        )
        .unwrap();
    let wrap = alice
        .op(
            json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1, "member_user_id": "mallory",
                    "member_key_id": mallory_key, "member_public": mallory_public, "key_checks": checks }),
        )
        .unwrap();
    alice
        .call(
            "add#VaultMember",
            json!({ "vaultId": vault_id, "expectedRevision": vault["vault"]["revision"], "entry": entry, "wrap": wrap }),
        )
        .unwrap();

    // mallory signs a list adding eve, with a vault key of her own; a database writer has made her a manager
    let vault = alice.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let entries: Vec<KeyVersion> = vault["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| shapes::key_version_of(entry).unwrap())
        .collect();
    let mut forged_members = entries.last().unwrap().members.clone();
    forged_members.push(Member {
        user_id: "eve".to_string(),
        role: Role::Manage,
        key_id: eve_key.clone(),
        public: mallory.public(),
    });
    let forged = KeyVersion::sign(&vault_id, 1, 3, forged_members, "mallory", &mallory_key, &mallory).unwrap();
    let own_key = vault::new_vault_key(&mut OsRng);
    let signer = Signer {
        user_id: "mallory",
        key_id: &mallory_key,
        key: &mallory,
    };
    let bob_public =
        moqui_vault::personal::PersonalPublic::from_bytes(&moqui_vault::text::unhex(&bob.public).unwrap()).unwrap();
    let to_bob = Recipient {
        user_id: "bob",
        key_id: &bob.key_id,
        public: &bob_public,
    };
    let bad_wrap = vault::wrap_vault_key_to_member(&own_key, &vault_id, 1, to_bob, signer, &mut OsRng).unwrap();
    sys(|| {
        facade
            .find("moqui.security.vault.VaultMember")
            .unwrap()
            .eq("userId", Value::string("mallory"))
            .update_all([("roleEnumId".to_string(), Value::string("VrManage"))])
            .unwrap();
    });
    login("mallory");
    let refused = call(
        "add#VaultMember",
        json!({ "vaultId": vault_id, "expectedRevision": vault["vault"]["revision"],
                "entry": shapes::key_version_json(&forged), "wrap": shapes::member_wrap_json(&bad_wrap) }),
    )
    .unwrap_err();
    assert!(refused.contains("manage role"), "{refused}");

    // a database writer stores mallory's wrap to bob in place of alice's: bob's client refuses it
    let real_wrap = sys(|| {
        facade
            .find("moqui.security.vault.VaultMember")
            .unwrap()
            .eq("userId", Value::string("bob"))
            .one()
            .unwrap()
            .unwrap()
    });
    let store_wrap = |wrap: &moqui_vault::vault::MemberWrap| {
        sys(|| {
            let mut row = real_wrap.clone();
            row.set("wrappedVaultKey", Value::string(hex(&wrap.sealed)));
            row.set("wrapSignerUserId", Value::string(&wrap.signer_user_id));
            row.set("wrapSignerKeyId", Value::string(&wrap.signer_key_id));
            row.set("wrapSignature", Value::string(hex(&wrap.signature)));
            row.update(&facade).unwrap();
        })
    };
    store_wrap(&bad_wrap);
    let refused = open_vault(&mut bob, &vault_id).unwrap_err();
    assert!(refused.contains("manage role"), "{refused}");
    // and with the forged entry in the chain too
    sys(|| {
        let mut row = facade.make_value("moqui.security.vault.VaultKeyVersion").unwrap();
        row.set("vaultId", Value::string(&vault_id));
        row.set("keyVersion", Value::I64(1));
        row.set("revision", Value::I64(3));
        row.set(
            "memberList",
            Value::string(shapes::key_version_json(&forged)["members"].to_string()),
        );
        row.set("signerUserId", Value::string("mallory"));
        row.set("signerKeyId", Value::string(&mallory_key));
        row.set("signature", Value::string(hex(&forged.signature)));
        row.create(&facade).unwrap();
    });
    let refused = open_vault(&mut bob, &vault_id).unwrap_err();
    assert!(refused.contains("manage role"), "{refused}");
}
