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

//! Vaults and items (§3.1 to §3.4, §3.10 to §3.12).

use serde_json::json;

use super::{add_item, all_text, event_kinds, new_id, open_vault, setup, shared_vault, Client};

/// No row of a personal vault holds an item's name, type or value in the clear: the body is sealed in the client,
/// the vault's name is in its sealed header, and a write that gives a clear name or type is refused.
#[test]
fn no_row_of_a_personal_vault_holds_an_items_name_type_or_value_in_the_clear() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let vault_id = shared_vault(&mut alice, &mut []);
    let body = json!({ "v": 1, "type": "login", "name": "Bank of Example", "fields": { "username": "alice-at-bank",
                       "password": "hunter2-very-secret" } });
    let item_id = add_item(&mut alice, &vault_id, 1, &body);
    let sealed = alice
        .op(
            json!({ "op": "item_seal", "vault_id": vault_id, "item_id": item_id, "key_version": 1,
                    "body": body.to_string().replace("hunter2", "hunter3") }),
        )
        .unwrap();
    alice
        .call(
            "update#VaultItem",
            json!({ "vaultItemId": item_id, "expectedRevision": 1, "keyVersion": 1,
                    "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"] }),
        )
        .unwrap();
    for entity in [
        "moqui.security.vault.Vault",
        "moqui.security.vault.VaultItem",
        "moqui.security.vault.VaultItemHistory",
        "moqui.security.vault.VaultMember",
        "moqui.security.vault.VaultKeyVersion",
    ] {
        for value in all_text(&facade, entity) {
            for clear in ["Bank of Example", "hunter2", "hunter3", "alice-at-bank", "login"] {
                assert!(!value.contains(clear), "{entity} holds {clear} in the clear: {value}");
            }
        }
    }
    // the client opens it, at the history's revision too
    let items = alice.call("list#VaultItems", json!({ "vaultId": vault_id })).unwrap();
    let item = &items["itemList"][0];
    let opened = alice
        .op(
            json!({ "op": "item_open", "vault_id": vault_id, "item_id": item_id, "key_version": 1,
                    "wrapped_item_key": item["wrappedItemKey"], "sealed_body": item["bodyCipher"] }),
        )
        .unwrap();
    assert!(opened["body"].as_str().unwrap().contains("hunter3"));
    // a clear name or type on a personal item, or a clear name on a personal vault, is refused
    let clear = json!({ "vaultId": vault_id, "vaultItemId": new_id("item"), "keyVersion": 1,
                        "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                        "itemName": "Bank of Example" });
    assert!(alice
        .call("create#VaultItem", clear)
        .unwrap_err()
        .contains("never in the clear"));
    let named = json!({ "vaultId": new_id("vault"), "vaultKindEnumId": "VkPersonal", "lockEnumId": "VlSingleSecret",
                        "vaultName": "My bank", "secretKindEnumId": "VsPassword", "secretWrap": { "x": 1 } });
    assert!(alice
        .call("create#Vault", named)
        .unwrap_err()
        .contains("never in the clear"));
    let kinds = event_kinds(&alice, &vault_id);
    assert_eq!(kinds, ["VeVaultCreated", "VeItemCreated", "VeItemUpdated"]);
}

/// A non-member gets no wrap and no body: the vault, its items and an item's history are "not found" to them, and a
/// write is refused; a reader reads and does not write.
#[test]
fn a_non_member_gets_no_wrap_and_no_body() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let mut bob = Client::new(&facade, "bob");
    let carol = Client::new(&facade, "carol");
    let vault_id = shared_vault(&mut alice, &mut [(&mut bob, "read")]);
    let item_id = add_item(
        &mut alice,
        &vault_id,
        1,
        &json!({ "v": 1, "type": "note", "name": "n", "fields": {} }),
    );

    for (name, parameters) in [
        ("get#Vault", json!({ "vaultId": vault_id })),
        ("list#VaultItems", json!({ "vaultId": vault_id })),
        ("list#VaultItemHistory", json!({ "vaultItemId": item_id })),
        ("list#VaultEvents", json!({ "vaultId": vault_id })),
        ("delete#VaultItem", json!({ "vaultItemId": item_id })),
    ] {
        let refused = carol.call(name, parameters).unwrap_err();
        assert!(
            refused.contains("No vault") || refused.contains("No item"),
            "{name}: {refused}"
        );
    }
    let listed = carol.call("list#Vaults", json!({})).unwrap();
    assert_eq!(listed["vaultList"], json!([]));

    // the reader opens the vault through the chain and reads the item, and may not write
    open_vault(&mut bob, &vault_id).unwrap();
    let items = bob.call("list#VaultItems", json!({ "vaultId": vault_id })).unwrap();
    let item = &items["itemList"][0];
    let opened = bob
        .op(
            json!({ "op": "item_open", "vault_id": vault_id, "item_id": item_id, "key_version": 1,
                    "wrapped_item_key": item["wrappedItemKey"], "sealed_body": item["bodyCipher"] }),
        )
        .unwrap();
    assert!(opened["body"].as_str().unwrap().contains("note"));
    let write = json!({ "vaultItemId": item_id, "expectedRevision": 1, "keyVersion": 1,
                        "wrappedItemKey": item["wrappedItemKey"], "bodyCipher": item["bodyCipher"] });
    assert!(bob.call("update#VaultItem", write).unwrap_err().contains("role"));
    let vault = bob.call("get#Vault", json!({ "vaultId": vault_id })).unwrap();
    let wraps = vault["wraps"].as_array().unwrap();
    assert_eq!(wraps.len(), 1);
    assert_eq!(wraps[0]["member_user_id"], json!("bob"));
}

/// A single-secret vault is personal, with no members and no recovery; an operational vault needs VAULT_ADMIN and keeps
/// recovery on or two members.
#[test]
fn a_single_secret_vault_refuses_members_and_recovery_and_an_operational_vault_needs_its_rules() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut alice = Client::new(&facade, "alice");
    let secret = json!({ "vaultId": new_id("vault"), "vaultKindEnumId": "VkPersonal", "lockEnumId": "VlSingleSecret",
                         "secretKindEnumId": "VsGenerated", "secretWrap": { "sealed": "00" } });
    let mut with_members = secret.clone();
    with_members["memberWraps"] = json!([{ "vault_id": "x" }]);
    assert!(alice
        .call("create#Vault", with_members)
        .unwrap_err()
        .contains("no members"));
    let mut with_recovery = secret.clone();
    with_recovery["recoveryOn"] = json!("Y");
    assert!(alice
        .call("create#Vault", with_recovery)
        .unwrap_err()
        .contains("no recovery"));
    let mut operational = secret.clone();
    operational["vaultKindEnumId"] = json!("VkOperational");
    operational["vaultName"] = json!("prod");
    assert!(alice
        .call("create#Vault", operational)
        .unwrap_err()
        .contains("VAULT_ADMIN"));
    // a password's wrap under the Argon2id floor is refused
    let mut weak = secret.clone();
    weak["secretKindEnumId"] = json!("VsPassword");
    weak["secretWrap"] = json!({ "m_kib": 1024, "t": 3, "p": 1, "salt": "00".repeat(16), "sealed": "ab".repeat(60) });
    assert!(alice.call("create#Vault", weak).unwrap_err().contains("parameters"));
    alice.call("create#Vault", secret.clone()).unwrap();
    // its owner alone reads it
    let vault = alice
        .call("get#Vault", json!({ "vaultId": secret["vaultId"] }))
        .unwrap();
    assert_eq!(vault["secretWrap"], json!({ "sealed": "00" }));
    let bob = Client::new(&facade, "bob");
    assert!(bob.call("get#Vault", json!({ "vaultId": secret["vaultId"] })).is_err());

    // an operational vault with one member and recovery off is refused (with VAULT_ADMIN granted)
    super::sys(|| {
        for (entity, fields) in [
            ("moqui.security.UserGroup", json!({ "userGroupId": "VAULT_TEST_ADMIN" })),
            (
                "moqui.security.UserGroupMember",
                json!({ "userGroupId": "VAULT_TEST_ADMIN", "userId": "alice", "fromDate": 0 }),
            ),
            (
                "moqui.security.UserGroupPermission",
                json!({ "userGroupId": "VAULT_TEST_ADMIN", "userPermissionId": "VAULT_ADMIN", "fromDate": 0 }),
            ),
        ] {
            let mut row = facade.make_value(entity).unwrap();
            for (name, given) in fields.as_object().unwrap() {
                row.set(name.as_str(), moqui_lib::json::from_json(given));
            }
            row.create(&facade).unwrap();
        }
    });
    let vault_id = new_id("vault");
    let members = json!([alice.member("manage")]);
    let entry = alice
        .op(json!({ "op": "key_version_first", "vault_id": vault_id, "members": members }))
        .unwrap();
    alice
        .op(json!({ "op": "vault_key_new", "vault_id": vault_id, "key_version": 1 }))
        .unwrap();
    let wrap = alice
        .op(
            json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1, "member_user_id": "alice",
                    "member_key_id": alice.key_id, "member_public": alice.public }),
        )
        .unwrap();
    let create = json!({ "vaultId": vault_id, "vaultKindEnumId": "VkOperational", "lockEnumId": "VlSharedKey",
                         "vaultName": "prod", "recoveryOn": "N", "entry": entry, "memberWraps": [wrap] });
    let refused = alice.call("create#Vault", create.clone()).unwrap_err();
    assert!(refused.contains("recovery on or at least two members"), "{refused}");
    let mut with_recovery = create;
    with_recovery["recoveryOn"] = json!("Y");
    alice.call("create#Vault", with_recovery).unwrap();

    // an operational item's binding is unique in its vault, on update too, and its clear columns change all together
    super::sys(|| {
        let mut row = facade.make_value("moqui.security.vault.VaultItemType").unwrap();
        row.set("vaultItemTypeId", moqui_lib::object::Value::string("login"));
        row.create(&facade).unwrap();
    });
    let mut item =
        |binding: &str| {
            let item_id = new_id("item");
            let label = json!({ "item_type": "login", "binding": binding, "name": binding });
            let sealed = alice
            .op(json!({ "op": "item_seal", "vault_id": vault_id, "item_id": item_id, "key_version": 1, "label": label,
                        "body": "{}" }))
            .unwrap();
            let create = json!({ "vaultId": vault_id, "vaultItemId": item_id, "keyVersion": 1,
                             "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                             "vaultItemTypeId": "login", "bindingKey": binding, "itemName": binding });
            alice.call("create#VaultItem", create.clone()).unwrap();
            (item_id, create)
        };
    let (_, first) = item("b1");
    let (second_id, second) = item("b2");
    let mut moved = second.clone();
    moved["vaultItemId"] = json!(second_id);
    moved["expectedRevision"] = json!(1);
    moved["bindingKey"] = json!("b1");
    assert!(alice
        .call("update#VaultItem", moved.clone())
        .unwrap_err()
        .contains("taken"));
    let mut renamed = moved;
    renamed.as_object_mut().unwrap().remove("bindingKey");
    renamed.as_object_mut().unwrap().remove("vaultItemTypeId");
    renamed["itemName"] = json!("other");
    assert!(alice
        .call("update#VaultItem", renamed)
        .unwrap_err()
        .contains("type, binding and name"));
    let mut again = first;
    again["vaultItemId"] = json!(new_id("item"));
    assert!(alice.call("create#VaultItem", again).unwrap_err().contains("taken"));
}

/// Deleting a vault needs a recent sign-in, and its id is never given again: its events stay, and a vault made with
/// the same id would show them to its members. A taken id is refused in the same words.
#[test]
fn a_deletion_needs_a_recent_sign_in_and_a_deleted_vaults_id_is_not_given_again() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let alice = Client::new(&facade, "alice");
    let bob = Client::new(&facade, "bob");
    let vault_id = new_id("vault");
    let secret = json!({ "vaultId": vault_id, "vaultKindEnumId": "VkPersonal", "lockEnumId": "VlSingleSecret",
                         "secretKindEnumId": "VsGenerated", "secretWrap": { "sealed": "00" } });
    alice.call("create#Vault", secret.clone()).unwrap();
    let taken = bob.call("create#Vault", secret.clone()).unwrap_err();
    assert!(taken.contains("is not available"), "{taken}");
    let refused = alice.call("delete#Vault", json!({ "vaultId": vault_id })).unwrap_err();
    assert!(refused.contains("reauthRequired"), "{refused}");
    super::strong_login("alice", 0);
    super::call("delete#Vault", json!({ "vaultId": vault_id })).unwrap();
    let reused = super::call("create#Vault", secret).unwrap_err();
    assert!(reused.contains("is not available"), "{reused}");
    assert_eq!(reused.replace("alice", ""), taken.replace("bob", ""));
}
