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

//! Operational vaults (plans/archive/Vault-Instances.md V2): every rule of the V2 Tests column, against the in-memory entity
//! facade, with clients that are the vault crate's own sessions. Two registries stand for two cluster members.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::entity::EntityFacade;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::{call, new_id, open_vault, setup, strong_login, sys, text, Client};
use crate::lease::{self, Ask, LeaseRegistry};

/// The lease key's seed and the field pass, as a test's factory reads them (process properties of their environment names).
fn keys() {
    for (name, value) in [
        (
            "moqui_conf__vault__lease_key",
            "dGVzdC1sZWFzZS1rZXktc2VlZC0zMi1ieXRlcy0hISE=",
        ),
        (
            "moqui_conf__entity_facade__crypt_pass",
            "test-field-pass-for-the-machine-key",
        ),
    ] {
        if moqui_lib::env::get_property(name).is_none() {
            moqui_lib::env::set_property(name, value);
        }
    }
}

fn create(facade: &EntityFacade, entity: &str, fields: Json) {
    sys(|| {
        let mut row = facade.make_value(entity).unwrap();
        for (name, given) in fields.as_object().unwrap() {
            row.set(name.as_str(), moqui_lib::json::from_json(given));
        }
        row.create(facade).unwrap();
    });
}

/// An operational vault: `maker` manages it (with VAULT_ADMIN), `approver` approves and `revealer` reveals; two items,
/// a password and a log level, and the approver has checked the lease key.
struct Fixture {
    maker: Client,
    approver: Client,
    revealer: Client,
    second: Client,
    vault_id: String,
    password: String,
    log_level: String,
}

const PASSWORD: &str = "AppInstance:demo:dbPassword";
const LOG_LEVEL: &str = "AppInstance:demo:logLevel";
const SECRET: &str = "correct horse battery staple";

fn fixture(facade: &EntityFacade, tag: &str) -> Fixture {
    keys();
    let mut maker = Client::new(facade, &format!("{tag}-maker"));
    let mut approver = Client::new(facade, &format!("{tag}-approver"));
    let mut revealer = Client::new(facade, &format!("{tag}-revealer"));
    let mut second = Client::new(facade, &format!("{tag}-second"));
    let group = format!("VT_{tag}");
    create(facade, "moqui.security.UserGroup", json!({ "userGroupId": group }));
    create(
        facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": group, "userId": maker.user, "fromDate": 0 }),
    );
    create(
        facade,
        "moqui.security.UserGroupPermission",
        json!({ "userGroupId": group, "userPermissionId": "VAULT_ADMIN", "fromDate": 0 }),
    );
    sys(|| {
        if facade
            .find("moqui.security.vault.VaultItemType")
            .unwrap()
            .eq("vaultItemTypeId", Value::string("login"))
            .one()
            .unwrap()
            .is_none()
        {
            let mut row = facade.make_value("moqui.security.vault.VaultItemType").unwrap();
            row.set("vaultItemTypeId", Value::string("login"));
            row.create(facade).unwrap();
        }
    });
    maker.check(&approver.user);
    maker.check(&revealer.user);
    maker.check(&second.user);
    let vault_id = new_id("vault");
    let members = json!([
        maker.member("manage"),
        approver.member("approve"),
        revealer.member("reveal"),
        second.member("approve")
    ]);
    let checks = maker.checks();
    let entry = maker
        .op(json!({ "op": "key_version_first", "vault_id": vault_id, "members": members, "key_checks": checks }))
        .unwrap();
    maker
        .op(json!({ "op": "vault_key_new", "vault_id": vault_id, "key_version": 1 }))
        .unwrap();
    let mut wraps = Vec::new();
    for member in members.as_array().unwrap() {
        let wrap = json!({ "op": "vault_key_wrap_member", "vault_id": vault_id, "key_version": 1,
                           "member_user_id": member["user_id"], "member_key_id": member["key_id"],
                           "member_public": member["public"], "key_checks": checks });
        wraps.push(maker.op(wrap).unwrap());
    }
    let create_vault = json!({ "vaultId": vault_id, "vaultKindEnumId": "VkOperational", "lockEnumId": "VlSharedKey",
                               "vaultName": "prod", "entry": entry, "memberWraps": wraps });
    maker.call("create#Vault", create_vault).unwrap();
    approver.check(&maker.user);
    revealer.check(&maker.user);
    second.check(&maker.user);
    let password = item(
        &mut maker,
        &vault_id,
        PASSWORD,
        &json!({ "username": "app", "password": SECRET }),
    );
    let log_level = item(&mut maker, &vault_id, LOG_LEVEL, &json!({ "level": "info-and-above" }));
    check_lease_key(&mut approver);
    check_lease_key(&mut second);
    Fixture {
        maker,
        approver,
        revealer,
        second,
        vault_id,
        password,
        log_level,
    }
}

/// Seals an operational item in `client`'s session and creates it. Its id.
fn item(client: &mut Client, vault_id: &str, binding: &str, fields: &Json) -> String {
    let item_id = new_id("item");
    let label = json!({ "item_type": "login", "binding": binding, "name": binding });
    let body = json!({ "v": 1, "type": "login", "name": binding, "fields": fields });
    let sealed = client
        .op(
            json!({ "op": "item_seal", "vault_id": vault_id, "item_id": item_id, "key_version": 1, "label": label,
                    "body": body.to_string() }),
        )
        .unwrap();
    let create = json!({ "vaultId": vault_id, "vaultItemId": item_id, "keyVersion": 1,
                         "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                         "vaultItemTypeId": "login", "bindingKey": binding, "itemName": binding });
    client.call("create#VaultItem", create).unwrap();
    item_id
}

/// `client` compares the lease key's fingerprint and stores its check.
fn check_lease_key(client: &mut Client) {
    let key = client.call("get#VaultLeaseKey", json!({})).unwrap();
    let check = client
        .op(
            json!({ "op": "key_check_make", "lease_key": key["leaseKey"], "fingerprint": key["fingerprint"],
                    "checked_at": moqui_core::user::now_millis() }),
        )
        .unwrap();
    client.call("store#KeyCheck", json!({ "check": check })).unwrap();
}

/// Asks a lease of `bindings` through `registry`, as the maker's operation.
fn ask(registry: &LeaseRegistry, facade: &EntityFacade, fixture: &Fixture, bindings: &[&str]) -> String {
    super::login(&fixture.maker.user);
    let bindings: Vec<String> = bindings.iter().map(|binding| binding.to_string()).collect();
    let lease = registry
        .ask(
            facade,
            &Ask {
                vault_id: &fixture.vault_id,
                bindings: &bindings,
                operation: "deploy run 7",
                limit: Some(Duration::from_secs(600)),
                environment_id: None,
            },
        )
        .unwrap();
    lease.lease_id
}

/// The approver's client approves the lease as the dialog does: the statement from get#VaultLease, checked against
/// the lease key its user checked. The client's answer, or its refusal.
fn approve_in_client(approver: &mut Client, lease: &Json, wrapped: &Json) -> Result<Json, String> {
    let key = approver.call("get#VaultLeaseKey", json!({})).unwrap();
    let checks = approver.checks();
    approver.op(
        json!({ "op": "lease_approve", "lease": lease, "lease_key": key["leaseKey"], "key_checks": checks,
                        "wrapped_item_keys": wrapped, "now": moqui_core::user::now_millis() }),
    )
}

/// The approver opens the vault, approves in their client and stores the approval with a recent sign-in.
fn approve(fixture: &mut Fixture, lease_id: &str) -> Result<Json, String> {
    if let Err(err) = open_vault(&mut fixture.approver, &fixture.vault_id) {
        assert!(err.contains("already open"), "{err}");
    }
    let got = fixture
        .approver
        .call("get#VaultLease", json!({ "leaseId": lease_id }))
        .unwrap();
    let approval = approve_in_client(&mut fixture.approver, &got["lease"], &got["wrappedItemKeys"])?;
    strong_login(&fixture.approver.user, 0);
    call(
        "approve#VaultLease",
        json!({ "leaseId": lease_id, "approval": approval }),
    )
}

fn status(facade: &EntityFacade, lease_id: &str) -> String {
    sys(|| {
        let row = facade
            .find("moqui.security.vault.VaultLease")
            .unwrap()
            .eq("leaseId", Value::string(lease_id))
            .one()
            .unwrap()
            .unwrap();
        moqui_lib::object::to_plain_string(row.get("statusId").unwrap_or(&Value::Null))
    })
}

fn lease_events(facade: &EntityFacade, lease_id: &str) -> Vec<String> {
    sys(|| {
        facade
            .find("moqui.security.vault.VaultEvent")
            .unwrap()
            .eq("leaseId", Value::string(lease_id))
            .list()
            .unwrap()
            .iter()
            .map(|row| moqui_lib::object::to_plain_string(row.get("eventTypeEnumId").unwrap_or(&Value::Null)))
            .collect()
    })
}

/// A lease opens only the items it names, and nothing it held opens after it ends: its keys are gone from memory, and
/// the sealed keys on its row open with no key the server keeps.
#[test]
fn a_lease_opens_only_the_items_it_names_and_nothing_after_it_ends() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "names");
    let registry = LeaseRegistry::new("member-1");
    let lease_id = ask(&registry, &facade, &fixture, &[PASSWORD]);
    assert_eq!(status(&facade, &lease_id), "VlsRequested");
    let waiting = registry
        .leased(&facade, &lease_id, PASSWORD, Some("password"))
        .unwrap_err();
    assert!(waiting.message().contains("waiting for approval"), "{waiting}");
    approve(&mut fixture, &lease_id).unwrap();
    registry.wait_open(&facade, &lease_id, Duration::from_secs(5)).unwrap();
    let value = registry.leased(&facade, &lease_id, PASSWORD, Some("password")).unwrap();
    assert_eq!(value, Value::string(SECRET));
    let other = registry.leased(&facade, &lease_id, LOG_LEVEL, None).unwrap_err();
    assert!(other.message().contains("names no item"), "{other}");
    // every use is an event, and none names the value
    let events = lease_events(&facade, &lease_id);
    assert!(events.contains(&"VeLeaseUsed".to_string()), "{events:?}");
    assert!(!super::all_text(&facade, "moqui.security.vault.VaultEvent")
        .iter()
        .any(|text| text.contains(SECRET)));

    registry.end(&facade, &lease_id).unwrap();
    assert_eq!(status(&facade, &lease_id), "VlsEnded");
    let after = registry
        .leased(&facade, &lease_id, PASSWORD, Some("password"))
        .unwrap_err();
    assert!(after.message().contains("ask again"), "{after}");
    assert!(!registry.holds(&lease_id));
    // no row holds the value, or a key that opens it: the lease row has only what its private key opened
    for entity in ["moqui.security.vault.VaultLease", "moqui.security.vault.VaultItem"] {
        assert!(!super::all_text(&facade, entity)
            .iter()
            .any(|text| text.contains(SECRET)));
    }
}

/// An approval without the approve role, or without a recent sign-in, is refused.
#[test]
fn an_approval_without_the_approve_role_or_a_recent_sign_in_is_refused() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "role");
    let registry = LeaseRegistry::new("member-1");
    let lease_id = ask(&registry, &facade, &fixture, &[PASSWORD]);
    open_vault(&mut fixture.approver, &fixture.vault_id).unwrap();
    let got = fixture
        .approver
        .call("get#VaultLease", json!({ "leaseId": lease_id }))
        .unwrap();
    let approval = approve_in_client(&mut fixture.approver, &got["lease"], &got["wrappedItemKeys"]).unwrap();

    // the manager has no approve role: even with the approver's approval in hand, and a recent sign-in
    strong_login(&fixture.maker.user, 0);
    let by_manager = call(
        "approve#VaultLease",
        json!({ "leaseId": lease_id, "approval": approval }),
    )
    .unwrap_err();
    assert!(by_manager.contains("approve role"), "{by_manager}");
    // the approver, signed in ten minutes ago
    strong_login(&fixture.approver.user, 10 * 60 * 1000);
    let stale = call(
        "approve#VaultLease",
        json!({ "leaseId": lease_id, "approval": approval }),
    )
    .unwrap_err();
    assert!(stale.contains("reauthRequired"), "{stale}");
    assert_eq!(status(&facade, &lease_id), "VlsRequested");
    strong_login(&fixture.approver.user, 0);
    call(
        "approve#VaultLease",
        json!({ "leaseId": lease_id, "approval": approval }),
    )
    .unwrap();
    assert_eq!(status(&facade, &lease_id), "VlsOpen");
}

/// A lease whose event cannot be written does not open: its approval rolls back, and a use returns no value.
#[test]
fn a_lease_whose_event_cannot_be_written_does_not_open() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "fault");
    let registry = LeaseRegistry::new("member-1");
    let lease_id = ask(&registry, &facade, &fixture, &[PASSWORD]);
    open_vault(&mut fixture.approver, &fixture.vault_id).unwrap();
    let got = fixture
        .approver
        .call("get#VaultLease", json!({ "leaseId": lease_id }))
        .unwrap();
    let approval = approve_in_client(&mut fixture.approver, &got["lease"], &got["wrappedItemKeys"]).unwrap();
    strong_login(&fixture.approver.user, 0);
    crate::events::FAIL_WRITES.with(|fail| fail.set(true));
    let refused = call(
        "approve#VaultLease",
        json!({ "leaseId": lease_id, "approval": approval }),
    );
    crate::events::FAIL_WRITES.with(|fail| fail.set(false));
    assert!(refused.is_err());
    assert_eq!(status(&facade, &lease_id), "VlsRequested");

    call(
        "approve#VaultLease",
        json!({ "leaseId": lease_id, "approval": approval }),
    )
    .unwrap();
    registry.wait_open(&facade, &lease_id, Duration::from_secs(5)).unwrap();
    crate::events::FAIL_WRITES.with(|fail| fail.set(true));
    let used = registry.leased(&facade, &lease_id, PASSWORD, Some("password"));
    crate::events::FAIL_WRITES.with(|fail| fail.set(false));
    let err = used.unwrap_err();
    assert!(err.message().contains("could not be written"), "{err}");
    assert!(
        !registry.holds_value(SECRET),
        "a value whose use was not recorded was handed out"
    );
    // with the event written, it opens
    assert_eq!(
        registry.leased(&facade, &lease_id, PASSWORD, Some("password")).unwrap(),
        Value::string(SECRET)
    );
}

/// A lease approved through one cluster member runs on the member that asked for it, and on no other.
#[test]
fn a_lease_approved_through_one_member_runs_on_another() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "members");
    let asking = LeaseRegistry::new("member-1");
    let other = LeaseRegistry::new("member-2");
    let lease_id = ask(&asking, &facade, &fixture, &[PASSWORD]);
    let statement = sys(|| {
        let row = facade
            .find("moqui.security.vault.VaultLease")
            .unwrap()
            .eq("leaseId", Value::string(&lease_id))
            .one()
            .unwrap()
            .unwrap();
        lease::lease_of_row(&row).unwrap()
    });
    assert_eq!(statement.cluster_member, "member-1");
    // the approval is a service call, which any member takes: it needs no registry
    approve(&mut fixture, &lease_id).unwrap();
    asking.wait_open(&facade, &lease_id, Duration::from_secs(5)).unwrap();
    assert_eq!(
        asking.leased(&facade, &lease_id, PASSWORD, Some("password")).unwrap(),
        Value::string(SECRET)
    );
    let elsewhere = other
        .leased(&facade, &lease_id, PASSWORD, Some("password"))
        .unwrap_err();
    assert!(elsewhere.message().contains("ask again"), "{elsewhere}");
    // a revoke on any member ends it where it runs, read from the row
    super::login(&fixture.approver.user);
    call("revoke#VaultLease", json!({ "leaseId": lease_id })).unwrap();
    let revoked = asking
        .leased(&facade, &lease_id, PASSWORD, Some("password"))
        .unwrap_err();
    assert!(revoked.message().contains("ask again"), "{revoked}");
    assert!(lease_events(&facade, &lease_id).contains(&"VeLeaseRevoked".to_string()));
}

/// Machine access opens only its items for its environment, and its sealed keys moved to another environment's row do
/// not open.
#[test]
fn machine_access_opens_only_its_items_for_its_environment() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "machine");
    let registry = LeaseRegistry::new("member-1");
    super::login(&fixture.maker.user);
    let grant = crate::machine::ask_grant(
        &registry,
        &facade,
        "env-demo1",
        &fixture.vault_id,
        &[PASSWORD.to_string()],
    )
    .unwrap();
    approve(&mut fixture, &grant.lease_id).unwrap();
    crate::machine::complete_grant(&registry, &facade, &grant.lease_id, Duration::from_secs(5)).unwrap();
    assert_eq!(status(&facade, &grant.lease_id), "VlsEnded");
    // the column is not an encrypted field: the facade reads it as the sealed hex, never the item key
    let access = super::all_text(&facade, "moqui.security.vault.VaultMachineAccess");
    assert!(access.iter().any(|text| text == &fixture.password));

    let bindings = [PASSWORD.to_string()];
    let lease =
        crate::machine::machine_lease(&registry, &facade, "env-demo1", &fixture.vault_id, &bindings, "deploy").unwrap();
    assert_eq!(
        registry
            .leased(&facade, &lease.lease_id, PASSWORD, Some("password"))
            .unwrap(),
        Value::string(SECRET)
    );
    let events = lease_events(&facade, &lease.lease_id);
    assert!(events.contains(&"VeMachineUsed".to_string()), "{events:?}");
    // an item it was not given
    let not_given = crate::machine::machine_lease(
        &registry,
        &facade,
        "env-demo1",
        &fixture.vault_id,
        &[LOG_LEVEL.to_string()],
        "deploy",
    )
    .unwrap_err();
    assert!(not_given.message().contains("no machine access"), "{not_given}");
    // another environment, and the first one's sealed key copied to its row
    let elsewhere =
        crate::machine::machine_lease(&registry, &facade, "env-other", &fixture.vault_id, &bindings, "deploy")
            .unwrap_err();
    assert!(elsewhere.message().contains("no machine access"), "{elsewhere}");
    sys(|| {
        let row = facade
            .find("moqui.security.vault.VaultMachineAccess")
            .unwrap()
            .eq("environmentId", Value::string("env-demo1"))
            .one()
            .unwrap()
            .unwrap();
        let mut moved = row.clone();
        moved.set("environmentId", Value::string("env-other"));
        moved.create(&facade).unwrap();
    });
    let moved = crate::machine::machine_lease(&registry, &facade, "env-other", &fixture.vault_id, &bindings, "deploy")
        .unwrap_err();
    assert!(!moved.message().contains("no machine access"), "{moved}");
    // a rotation of the field pass: a row sealed under the pass before it opens with it, and is sealed again under the
    // new pass, so it opens once the old one is gone
    let sealed_before = sealed_key(&facade, "env-demo1");
    let pass = moqui_lib::env::get_property("moqui_conf__entity_facade__crypt_pass").unwrap();
    moqui_lib::env::set_property("moqui_conf__entity_facade__crypt_pass_old", pass.clone());
    moqui_lib::env::set_property(
        "moqui_conf__entity_facade__crypt_pass",
        "the-field-pass-after-a-rotation",
    );
    let rotated =
        crate::machine::machine_lease(&registry, &facade, "env-demo1", &fixture.vault_id, &bindings, "deploy");
    moqui_lib::env::clear_property("moqui_conf__entity_facade__crypt_pass_old");
    let rotated = rotated.unwrap();
    assert_ne!(sealed_key(&facade, "env-demo1"), sealed_before);
    let again = crate::machine::machine_lease(&registry, &facade, "env-demo1", &fixture.vault_id, &bindings, "deploy");
    moqui_lib::env::set_property("moqui_conf__entity_facade__crypt_pass", pass);
    again.unwrap();
    assert_eq!(
        registry
            .leased(&facade, &rotated.lease_id, PASSWORD, Some("password"))
            .unwrap(),
        Value::string(SECRET)
    );

    // revoking deletes the sealed keys, ends the environment's machine leases open on the vault, and names the items to
    // rotate
    super::login(&fixture.maker.user);
    let revoked = call(
        "revoke#VaultMachineAccess",
        json!({ "environmentId": "env-demo1", "vaultId": fixture.vault_id }),
    )
    .unwrap();
    assert_eq!(revoked["rotateItemIds"], json!([fixture.password]));
    let gone = crate::machine::machine_lease(&registry, &facade, "env-demo1", &fixture.vault_id, &bindings, "deploy")
        .unwrap_err();
    assert!(gone.message().contains("no machine access"), "{gone}");
    let ended = registry
        .leased(&facade, &lease.lease_id, PASSWORD, Some("password"))
        .unwrap_err();
    assert!(ended.message().contains("ask again"), "{ended}");
    assert_eq!(status(&facade, &lease.lease_id), "VlsRevoked");
}

fn sealed_key(facade: &EntityFacade, environment_id: &str) -> String {
    sys(|| {
        let row = facade
            .find("moqui.security.vault.VaultMachineAccess")
            .unwrap()
            .eq("environmentId", Value::string(environment_id))
            .one()
            .unwrap()
            .unwrap();
        moqui_lib::object::to_plain_string(row.get("sealedItemKey").unwrap())
    })
}

/// Machine access goes stale when its item is written with a new item key, and a lease under it is refused clearly;
/// a grant whose lease is never approved ends, and its keys are not kept.
#[test]
fn machine_access_goes_stale_with_a_new_item_key_and_an_unapproved_grant_ends() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "stale");
    let registry = LeaseRegistry::new("member-1");
    super::login(&fixture.maker.user);
    let bindings = [PASSWORD.to_string()];
    let unapproved = crate::machine::ask_grant(&registry, &facade, "env-stale1", &fixture.vault_id, &bindings).unwrap();
    let refused = crate::machine::complete_grant(&registry, &facade, &unapproved.lease_id, Duration::from_millis(50))
        .unwrap_err();
    assert!(refused.message().contains("not approved in time"), "{refused}");
    assert!(!registry.holds(&unapproved.lease_id));
    assert_eq!(status(&facade, &unapproved.lease_id), "VlsEnded");

    super::login(&fixture.maker.user);
    let grant = crate::machine::ask_grant(&registry, &facade, "env-stale1", &fixture.vault_id, &bindings).unwrap();
    approve(&mut fixture, &grant.lease_id).unwrap();
    crate::machine::complete_grant(&registry, &facade, &grant.lease_id, Duration::from_secs(5)).unwrap();
    crate::machine::machine_lease(&registry, &facade, "env-stale1", &fixture.vault_id, &bindings, "deploy").unwrap();
    // the maker writes the item with a new item key
    let label = json!({ "item_type": "login", "binding": PASSWORD, "name": PASSWORD });
    let body = json!({ "v": 1, "type": "login", "name": PASSWORD, "fields": { "password": "a-new-password-value" } });
    let sealed = fixture
        .maker
        .op(
            json!({ "op": "item_seal", "vault_id": fixture.vault_id, "item_id": fixture.password, "key_version": 1,
                    "label": label, "body": body.to_string() }),
        )
        .unwrap();
    assert_eq!(sealed["new_item_key"], json!(true));
    fixture
        .maker
        .call(
            "update#VaultItem",
            json!({ "vaultItemId": fixture.password, "expectedRevision": 1, "keyVersion": 1, "newItemKey": true,
                    "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"] }),
        )
        .unwrap();
    let stale = crate::machine::machine_lease(&registry, &facade, "env-stale1", &fixture.vault_id, &bindings, "deploy")
        .unwrap_err();
    assert!(stale.message().contains("machineAccessStale"), "{stale}");
    let listed = fixture
        .maker
        .call("list#VaultMachineAccess", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    assert!(!text(&listed["accessList"][0], "staleDate").is_empty());
}

/// The registry is named by the member's cluster id once the cluster starts, and the start ends only that member's
/// leases left open, never another member's.
#[test]
fn the_registry_takes_the_cluster_member_id_and_ends_only_its_own_leases() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fixture = fixture(&facade, "cluster");
    let mine = LeaseRegistry::new("vault-member-a");
    let theirs = LeaseRegistry::new("vault-member-b");
    let my_lease = ask(&mine, &facade, &fixture, &[PASSWORD]);
    let their_lease = ask(&theirs, &facade, &fixture, &[PASSWORD]);
    assert_eq!(lease::registry(ctx.factory()).member_id(), "local");
    // the cluster's start as the host runs it, with what it reads: this member's id, the session cookie key and the
    // cluster's certificate pair (whose fingerprint is all the start reads of it)
    let settings = [
        ("moqui_conf__instance__id", "vault-member-a"),
        (
            "moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key",
            "dGVzdC1jbHVzdGVyLWNvb2tpZS1rZXktMzItYnl0ZXMhIQ==",
        ),
        ("moqui_conf__cluster_member__tls_cert", "dGVzdC1jZXJ0aWZpY2F0ZQ=="),
        ("moqui_conf__cluster_member__tls_key", "dGVzdC1rZXk="),
    ];
    for (name, value) in settings {
        moqui_lib::env::set_property(name, value);
    }
    let started = moqui_core::cluster::start(ctx.factory());
    for (name, _) in settings {
        moqui_lib::env::clear_property(name);
    }
    started.unwrap();
    assert_eq!(lease::registry(ctx.factory()).member_id(), "vault-member-a");
    assert_eq!(status(&facade, &my_lease), "VlsEnded");
    assert_eq!(status(&facade, &their_lease), "VlsRequested");
}

/// An approver approves a lease they asked for only when no other member holds the approve role; the member that
/// holds the lease refuses an approval whose approver no longer holds the approve role.
#[test]
fn an_approver_does_not_approve_their_own_lease_and_a_demoted_approver_does_not_open_one() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "self");
    let registry = LeaseRegistry::new("member-1");
    super::login(&fixture.approver.user);
    let names = [PASSWORD.to_string()];
    let own = registry
        .ask(
            &facade,
            &Ask {
                vault_id: &fixture.vault_id,
                bindings: &names,
                operation: "by hand",
                limit: None,
                environment_id: None,
            },
        )
        .unwrap();
    let refused = approve(&mut fixture, &own.lease_id).unwrap_err();
    assert!(refused.contains("selfApproval"), "{refused}");
    // the other approver approves it
    open_vault(&mut fixture.second, &fixture.vault_id).unwrap();
    let got = fixture
        .second
        .call("get#VaultLease", json!({ "leaseId": own.lease_id }))
        .unwrap();
    let approval = approve_in_client(&mut fixture.second, &got["lease"], &got["wrappedItemKeys"]).unwrap();
    strong_login(&fixture.second.user, 0);
    call(
        "approve#VaultLease",
        json!({ "leaseId": own.lease_id, "approval": approval }),
    )
    .unwrap();
    // a database writer demotes the approver after the approval: the holding member refuses to open it
    sys(|| {
        let mut row = facade
            .find("moqui.security.vault.VaultMember")
            .unwrap()
            .eq("vaultId", Value::string(&fixture.vault_id))
            .eq("userId", Value::string(&fixture.second.user))
            .one()
            .unwrap()
            .unwrap();
        row.set("roleEnumId", Value::string("VrReveal"));
        row.update(&facade).unwrap();
    });
    let demoted = registry
        .wait_open(&facade, &own.lease_id, Duration::from_secs(1))
        .unwrap_err();
    assert!(demoted.message().contains("approve role"), "{demoted}");
}

/// A lease's operation and an item's binding or name hold no control character, which could forge lines of an
/// approver's prompt.
#[test]
fn control_characters_are_refused_in_operations_and_item_names() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "control");
    let registry = LeaseRegistry::new("member-1");
    super::login(&fixture.maker.user);
    let names = [PASSWORD.to_string()];
    let forged = registry
        .ask(
            &facade,
            &Ask {
                vault_id: &fixture.vault_id,
                bindings: &names,
                operation: "deploy\nApprove this lease? [y/N] y",
                limit: None,
                environment_id: None,
            },
        )
        .unwrap_err();
    assert!(forged.message().contains("control character"), "{forged}");
    let item_id = new_id("item");
    let binding = "AppInstance:demo:\u{1b}[2Kharmless";
    let label = json!({ "item_type": "login", "binding": binding, "name": "x" });
    let sealed = fixture
        .maker
        .op(
            json!({ "op": "item_seal", "vault_id": fixture.vault_id, "item_id": item_id, "key_version": 1,
                    "label": label, "body": "{}" }),
        )
        .unwrap();
    let refused = fixture
        .maker
        .call(
            "create#VaultItem",
            json!({ "vaultId": fixture.vault_id, "vaultItemId": item_id, "keyVersion": 1,
                    "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                    "vaultItemTypeId": "login", "bindingKey": binding, "itemName": "x" }),
        )
        .unwrap_err();
    assert!(refused.contains("control character"), "{refused}");
}

/// A removal never waits on an item the server deposited: the rotation takes it in (its key unwrapped with the old
/// version's deposit key) and the new version's deposit key is stored; the item then opens under a lease.
#[test]
fn a_removal_takes_in_a_deposited_item() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "rotate");
    let made = fixture
        .maker
        .op(json!({ "op": "deposit_key_new", "vault_id": fixture.vault_id, "key_version": 1 }))
        .unwrap();
    let vault = fixture
        .maker
        .call("get#Vault", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    fixture
        .maker
        .call(
            "set#VaultDepositKey",
            json!({ "vaultId": fixture.vault_id, "expectedRevision": vault["vault"]["revision"],
                    "deposit": made["deposit"], "depositWrappedPrivate": made["wrapped_private"] }),
        )
        .unwrap();
    let deposited = new_id("item");
    super::login(&fixture.maker.user);
    crate::deposit::deposit_item(
        &facade,
        &crate::deposit::Deposit {
            vault_id: &fixture.vault_id,
            item_id: &deposited,
            item_type: "login",
            binding: "AppInstance:demo:redis",
            name: "AppInstance:demo:redis",
            body: br#"{"v":1,"type":"login","name":"x","fields":{"password":"made-by-the-server"}}"#,
        },
    )
    .unwrap();
    // the maker removes the revealer
    let vault = fixture
        .maker
        .call("get#Vault", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    let rows = fixture
        .maker
        .call("list#VaultItems", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    let items: Vec<Json> = rows["itemList"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let binding = text(row, "bindingKey");
            json!({ "item_id": row["vaultItemId"], "wrapped_item_key": row["wrappedItemKey"],
                    "label": { "item_type": "login", "binding": binding, "name": text(row, "itemName") },
                    "deposited": row["itemKeyWrapEnumId"] == json!("VwDeposit") })
        })
        .collect();
    let members = json!([
        fixture.maker.member("manage"),
        fixture.approver.member("approve"),
        fixture.second.member("approve")
    ]);
    let checks = fixture.maker.checks();
    let rotated = fixture
        .maker
        .op(
            json!({ "op": "vault_rotate", "entries": vault["entries"], "key_checks": checks, "members": members,
                    "items": items, "operational": true, "deposit_wrapped_private": vault["depositWrappedPrivate"] }),
        )
        .unwrap();
    fixture
        .maker
        .call(
            "remove#VaultMember",
            json!({ "vaultId": fixture.vault_id, "userId": fixture.revealer.user,
                    "expectedRevision": vault["vault"]["revision"], "entry": rotated["entry"],
                    "itemKeys": rotated["item_keys"], "memberWraps": rotated["member_wraps"],
                    "deposit": rotated["deposit"]["deposit"], "depositWrappedPrivate": rotated["deposit"]["wrapped_private"] }),
        )
        .unwrap();
    let after = fixture
        .maker
        .call("get#Vault", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    assert_eq!(after["deposit"]["key_version"], json!(2));
    let rows = fixture
        .maker
        .call("list#VaultItems", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    assert!(rows["itemList"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["itemKeyWrapEnumId"] == json!("VwVault")));
    // the approver opens version 2 and approves a lease of the item that was deposited
    let registry = LeaseRegistry::new("member-1");
    let lease_id = ask(&registry, &facade, &fixture, &["AppInstance:demo:redis"]);
    approve(&mut fixture, &lease_id).unwrap();
    registry.wait_open(&facade, &lease_id, Duration::from_secs(5)).unwrap();
    assert_eq!(
        registry
            .leased(&facade, &lease_id, "AppInstance:demo:redis", Some("password"))
            .unwrap(),
        Value::string("made-by-the-server")
    );
}

/// A client refuses a lease whose public key or item list was changed after it was signed, and one signed by a lease
/// key its user has not checked.
#[test]
fn a_client_refuses_a_changed_lease_and_an_unchecked_lease_key() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "client");
    let registry = LeaseRegistry::new("member-1");
    let lease_id = ask(&registry, &facade, &fixture, &[PASSWORD]);
    open_vault(&mut fixture.approver, &fixture.vault_id).unwrap();
    let got = fixture
        .approver
        .call("get#VaultLease", json!({ "leaseId": lease_id }))
        .unwrap();
    let (lease, wrapped) = (got["lease"].clone(), got["wrappedItemKeys"].clone());

    let mut other_key = lease.clone();
    let public = text(&lease, "public");
    other_key["public"] = json!(format!(
        "{}{}",
        if public.starts_with('0') { "1" } else { "0" },
        &public[1..]
    ));
    let refused = approve_in_client(&mut fixture.approver, &other_key, &wrapped).unwrap_err();
    assert!(refused.contains("signature"), "{refused}");

    let mut other_items = lease.clone();
    other_items["items"][0]["binding"] = json!(LOG_LEVEL);
    let refused = approve_in_client(&mut fixture.approver, &other_items, &wrapped).unwrap_err();
    assert!(refused.contains("signature"), "{refused}");

    // a lease signed by a key the approver never checked, presented with that key
    let forger = moqui_vault::sign::SigningKey::random(&mut moqui_vault::rng::OsRng);
    let mut forged = lease::lease_of_row(&sys(|| {
        facade
            .find("moqui.security.vault.VaultLease")
            .unwrap()
            .eq("leaseId", Value::string(&lease_id))
            .one()
            .unwrap()
            .unwrap()
    }))
    .unwrap();
    forged.sign(&forger);
    let checks = fixture.approver.checks();
    let refused = fixture
        .approver
        .op(json!({ "op": "lease_approve", "lease": lease::lease_json(&forged),
                    "lease_key": moqui_vault::text::hex(forger.verifying_key().as_bytes()), "key_checks": checks,
                    "wrapped_item_keys": wrapped, "now": moqui_core::user::now_millis() }))
        .unwrap_err();
    assert!(refused.contains("has not been checked"), "{refused}");

    // the revealer never checked the lease key: the real lease is refused too
    open_vault(&mut fixture.revealer, &fixture.vault_id).unwrap();
    let refused = approve_in_client(&mut fixture.revealer, &lease, &wrapped).unwrap_err();
    assert!(refused.contains("has not been checked"), "{refused}");
    // the untouched lease approves
    approve_in_client(&mut fixture.approver, &lease, &wrapped).unwrap();
}

/// A binding pointed at another item fails at the unwrap: a database writer who moves a harmless binding onto a secret
/// item gets a lease that names it, but the approver's client cannot unwrap its key under that binding.
#[test]
fn a_binding_pointed_at_another_item_fails_at_the_unwrap() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "binding");
    sys(|| {
        for (item_id, binding) in [
            (&fixture.log_level, "AppInstance:demo:moved"),
            (&fixture.password, LOG_LEVEL),
        ] {
            let mut row = facade
                .find("moqui.security.vault.VaultItem")
                .unwrap()
                .eq("vaultItemId", Value::string(item_id))
                .one()
                .unwrap()
                .unwrap();
            row.set("bindingKey", Value::string(binding));
            row.update(&facade).unwrap();
        }
    });
    let registry = LeaseRegistry::new("member-1");
    let lease_id = ask(&registry, &facade, &fixture, &[LOG_LEVEL]);
    let refused = approve(&mut fixture, &lease_id).unwrap_err();
    assert!(
        refused.contains("decrypt") || refused.contains("open") || refused.contains("authentic"),
        "{refused}"
    );
    assert_eq!(status(&facade, &lease_id), "VlsRequested");
}

/// The server refuses a deposit key with no manager's signature: one an approver signed, or one whose signature does
/// not verify, is refused when stored and at a deposit; the manager's own deposit key takes a deposit, which the server
/// cannot read back.
#[test]
fn the_server_refuses_a_deposit_key_with_no_managers_signature() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "deposit");
    open_vault(&mut fixture.approver, &fixture.vault_id).unwrap();
    let by_approver = fixture
        .approver
        .op(json!({ "op": "deposit_key_new", "vault_id": fixture.vault_id, "key_version": 1 }))
        .unwrap();
    let vault_id = fixture.vault_id.clone();
    let maker_user = fixture.maker.user.clone();
    let revision = |client: &Client| {
        client.call("get#Vault", json!({ "vaultId": vault_id })).unwrap()["vault"]["revision"].clone()
    };
    let refused = fixture
        .maker
        .call(
            "set#VaultDepositKey",
            json!({ "vaultId": fixture.vault_id, "expectedRevision": revision(&fixture.maker),
                    "deposit": by_approver["deposit"], "depositWrappedPrivate": by_approver["wrapped_private"] }),
        )
        .unwrap_err();
    assert!(refused.contains("yourself"), "{refused}");
    // a database writer puts the approver's deposit key on the vault: the deposit checks the member list and refuses
    let put = |deposit: &Json| {
        sys(|| {
            let mut row = facade
                .find("moqui.security.vault.Vault")
                .unwrap()
                .eq("vaultId", Value::string(&vault_id))
                .one()
                .unwrap()
                .unwrap();
            for (column, name) in [
                ("depositPublic", "public"),
                ("depositKeyVersion", "key_version"),
                ("depositSignerUserId", "signer_user_id"),
                ("depositSignerKeyId", "signer_key_id"),
                ("depositSignature", "signature"),
            ] {
                row.set(column, moqui_lib::json::from_json(&deposit[name]));
            }
            row.update(&facade).unwrap();
        });
    };
    put(&by_approver["deposit"]);
    let deposit = |binding: &str| {
        super::login(&maker_user);
        let item_id = new_id("item");
        crate::deposit::deposit_item(
            &facade,
            &crate::deposit::Deposit {
                vault_id: &vault_id,
                item_id: &item_id,
                item_type: "login",
                binding,
                name: binding,
                body: br#"{"v":1,"type":"login","name":"x","fields":{"password":"made-by-the-server"}}"#,
            },
        )
        .map(|()| item_id)
    };
    let refused = deposit("AppInstance:demo:redis").unwrap_err();
    assert!(refused.message().contains("manage role"), "{refused}");

    let by_maker = fixture
        .maker
        .op(json!({ "op": "deposit_key_new", "vault_id": fixture.vault_id, "key_version": 1 }))
        .unwrap();
    let mut unsigned = by_maker["deposit"].clone();
    let signature = text(&unsigned, "signature");
    unsigned["signature"] = json!(format!(
        "{}{}",
        if signature.starts_with('0') { "1" } else { "0" },
        &signature[1..]
    ));
    let refused = fixture
        .maker
        .call(
            "set#VaultDepositKey",
            json!({ "vaultId": fixture.vault_id, "expectedRevision": revision(&fixture.maker),
                    "deposit": unsigned, "depositWrappedPrivate": by_maker["wrapped_private"] }),
        )
        .unwrap_err();
    assert!(refused.contains("signature"), "{refused}");
    put(&unsigned);
    assert!(deposit("AppInstance:demo:redis").is_err());

    fixture
        .maker
        .call(
            "set#VaultDepositKey",
            json!({ "vaultId": fixture.vault_id, "expectedRevision": revision(&fixture.maker),
                    "deposit": by_maker["deposit"], "depositWrappedPrivate": by_maker["wrapped_private"] }),
        )
        .unwrap();
    let deposited = deposit("AppInstance:demo:redis").unwrap();
    assert!(!super::all_text(&facade, "moqui.security.vault.VaultItem")
        .iter()
        .any(|text| text.contains("made-by-the-server")));
    // the client sees the signed deposit key, to check on each open
    let vault = fixture
        .maker
        .call("get#Vault", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    assert_eq!(vault["deposit"]["signer_user_id"], json!(fixture.maker.user));

    // a lease cannot name it until a member takes it in: its key rewrapped by the vault key, its body as it was
    let registry = LeaseRegistry::new("member-1");
    super::login(&fixture.maker.user);
    let names = ["AppInstance:demo:redis".to_string()];
    let ask_redis = Ask {
        vault_id: &fixture.vault_id,
        bindings: &names,
        operation: "provision",
        limit: None,
        environment_id: None,
    };
    let refused = registry.ask(&facade, &ask_redis).unwrap_err();
    assert!(refused.message().contains("take it in"), "{refused}");
    let items = fixture
        .maker
        .call("list#VaultItems", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    let row = items["itemList"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["vaultItemId"] == json!(deposited))
        .cloned()
        .unwrap();
    assert_eq!(row["itemKeyWrapEnumId"], json!("VwDeposit"));
    let label = json!({ "item_type": "login", "binding": "AppInstance:demo:redis", "name": "AppInstance:demo:redis" });
    let adopted = fixture
        .maker
        .op(
            json!({ "op": "deposit_adopt", "vault_id": fixture.vault_id, "item_id": deposited, "key_version": 1,
                    "label": label, "deposit_wrapped_private": vault["depositWrappedPrivate"],
                    "wrapped_item_key": row["wrappedItemKey"] }),
        )
        .unwrap();
    let body = sys(|| {
        let item = facade
            .find("moqui.security.vault.VaultItem")
            .unwrap()
            .eq("vaultItemId", Value::string(&deposited))
            .one()
            .unwrap()
            .unwrap();
        moqui_lib::object::to_plain_string(item.get("bodyCipher").unwrap())
    });
    fixture
        .maker
        .call(
            "update#VaultItem",
            json!({ "vaultItemId": deposited, "expectedRevision": 1, "keyVersion": 1,
                    "wrappedItemKey": adopted["wrapped_item_key"], "bodyCipher": body }),
        )
        .unwrap();
    let lease = registry.ask(&facade, &ask_redis).unwrap();
    approve(&mut fixture, &lease.lease_id).unwrap();
    registry
        .wait_open(&facade, &lease.lease_id, Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        registry
            .leased(&facade, &lease.lease_id, "AppInstance:demo:redis", Some("password"))
            .unwrap(),
        Value::string("made-by-the-server")
    );
}

/// A leased value is not in a job's stored parameters: the job run is refused, and no stored row holds it. The value
/// comes through `ec.vault.leased` in a script run under the lease, on the factory's own registry.
#[test]
fn a_leased_value_is_not_in_a_jobs_stored_parameters() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "job");
    let registry = lease::registry(ctx.factory());
    let lease_id = ask(&registry, &facade, &fixture, &[PASSWORD]);
    approve(&mut fixture, &lease_id).unwrap();
    registry.wait_open(&facade, &lease_id, Duration::from_secs(5)).unwrap();
    // a script reads it by binding from the thread's lease, and with no lease it gets nothing
    let outside = moqui_core::script::eval_value(&format!("ec.vault.leased('{PASSWORD}', 'password')"));
    assert!(outside.is_err());
    let value = lease::run_under(&lease_id, || {
        moqui_core::script::eval_value(&format!("ec.vault.leased('{PASSWORD}', 'password')"))
    })
    .unwrap();
    assert_eq!(value, Value::string(SECRET));

    create(
        &facade,
        "moqui.service.job.ServiceJob",
        json!({ "jobName": "vault_test_job", "serviceName": "org.moqui.impl.BasicServices.noop", "paused": "N" }),
    );
    let mut parameters = BTreeMap::new();
    parameters.insert(
        "dbUrl".to_string(),
        Value::string(format!("postgres://app:{SECRET}@db/app")),
    );
    super::login(&fixture.maker.user);
    let refused = moqui_core::service::run_job_now("vault_test_job", &parameters).unwrap_err();
    assert!(refused.message().contains("leased from a vault"), "{refused}");
    assert!(!refused.message().contains(SECRET));
    for entity in ["moqui.service.job.ServiceJobRun", "moqui.service.job.ServiceJob"] {
        assert!(!super::all_text(&facade, entity)
            .iter()
            .any(|text| text.contains(SECRET)));
    }
    // a map key holds it as surely as a value
    let mut keyed = BTreeMap::new();
    let mut inner = moqui_lib::object::ValueMap::default();
    inner.insert(SECRET.to_string(), Value::Bool(true));
    keyed.insert("options".to_string(), Value::Map(inner));
    assert!(moqui_core::vault_hooks::refuse_leased("a call", &keyed).is_err());
    // the user name of the item is no secret, and another thread's operation did not read the value
    let mut plain = BTreeMap::new();
    plain.insert("user".to_string(), Value::string("app"));
    assert!(moqui_core::vault_hooks::refuse_leased("a call", &plain).is_ok());
    let elsewhere = std::thread::scope(|scope| scope.spawn(|| registry.holds_value(SECRET)).join().unwrap());
    assert!(!elsewhere);
    // once the lease ends, the guard forgets the value with it
    registry.end(&facade, &lease_id).unwrap();
    assert!(!registry.holds_value(SECRET));
}

/// A restart ends the member's leases: the new process holds none of them, a use is refused with "ask again", and the
/// boot marks the leases it asked for ended.
#[test]
fn a_restart_ends_the_members_leases() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "restart");
    let before = LeaseRegistry::new("member-restart");
    let lease_id = ask(&before, &facade, &fixture, &[PASSWORD]);
    approve(&mut fixture, &lease_id).unwrap();
    before.wait_open(&facade, &lease_id, Duration::from_secs(5)).unwrap();
    drop(before);
    let after = LeaseRegistry::new("member-restart");
    let refused = after
        .leased(&facade, &lease_id, PASSWORD, Some("password"))
        .unwrap_err();
    assert!(refused.message().contains("ask again"), "{refused}");
    assert_eq!(lease::end_left_open(&facade, "member-restart").unwrap(), 1);
    assert_eq!(status(&facade, &lease_id), "VlsEnded");
    assert!(lease_events(&facade, &lease_id).contains(&"VeLeaseEnded".to_string()));

    // a lease past its time limit is dropped and its row marked ended by the sweep, read again or not
    super::login(&fixture.maker.user);
    let names = [PASSWORD.to_string()];
    let short = after
        .ask(
            &facade,
            &Ask {
                vault_id: &fixture.vault_id,
                bindings: &names,
                operation: "a short one",
                limit: Some(Duration::from_millis(1)),
                environment_id: None,
            },
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    after.expire(&facade);
    assert!(!after.holds(&short.lease_id));
    assert_eq!(status(&facade, &short.lease_id), "VlsEnded");
}

/// Reveal is by role: the revealer gets the item's ciphertext and opens it in their client, with an event; the
/// approver does not.
#[test]
fn a_reveal_goes_only_to_the_reveal_role_and_is_an_event() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "reveal");
    let refused = fixture
        .approver
        .call("reveal#VaultItem", json!({ "vaultItemId": fixture.password }))
        .unwrap_err();
    assert!(refused.contains("reveal role"), "{refused}");
    let revealed = fixture
        .revealer
        .call("reveal#VaultItem", json!({ "vaultItemId": fixture.password }))
        .unwrap();
    open_vault(&mut fixture.revealer, &fixture.vault_id).unwrap();
    let label = json!({ "item_type": "login", "binding": PASSWORD, "name": PASSWORD });
    let opened = fixture
        .revealer
        .op(json!({ "op": "item_open", "vault_id": fixture.vault_id, "item_id": fixture.password, "key_version": 1,
                    "label": label, "wrapped_item_key": revealed["wrappedItemKey"], "sealed_body": revealed["bodyCipher"] }))
        .unwrap();
    assert!(text(&opened, "body").contains(SECRET));
    assert!(super::event_kinds(&fixture.revealer, &fixture.vault_id).contains(&"VeReveal".to_string()));
    // the item list gives no operational body to any member
    let items = fixture
        .maker
        .call("list#VaultItems", json!({ "vaultId": fixture.vault_id }))
        .unwrap();
    assert!(items["itemList"][0].get("bodyCipher").is_none());
}

/// The machine-access hook an unattended caller runs under (a CI run's call to its runner host, M1): the value of an
/// item the environment holds, under a lease approved by "machine" that ends with the call; an item it holds no access
/// to is refused, naming what to give.
#[test]
fn the_machine_run_hook_opens_only_what_the_environment_holds_and_ends_its_lease() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fixture = fixture(&facade, "machinerun");
    let registry = LeaseRegistry::new("member-1");
    super::login(&fixture.maker.user);
    let grant = crate::machine::ask_grant(&registry, &facade, "env-ci-runners", &fixture.vault_id, &[PASSWORD.to_string()])
        .unwrap();
    approve(&mut fixture, &grant.lease_id).unwrap();
    crate::machine::complete_grant(&registry, &facade, &grant.lease_id, Duration::from_secs(5)).unwrap();
    super::login(&fixture.maker.user);
    let binding = PASSWORD.to_string();
    let mut lease_id = None;
    let value = moqui_core::vault_hooks::machine_run("env-ci-runners", std::slice::from_ref(&binding), "CI run on InstanceHost H", || {
        lease_id = crate::lease::current_lease();
        moqui_core::vault_hooks::leased(&binding, Some("password"))
    })
    .unwrap();
    assert_eq!(value, Value::string(SECRET));
    let lease_id = lease_id.unwrap();
    assert_eq!(status(&facade, &lease_id), "VlsEnded");
    let events = lease_events(&facade, &lease_id);
    assert!(events.contains(&"VeMachineUsed".to_string()), "{events:?}");
    // another environment, and an item the environment was not given
    let refused = moqui_core::vault_hooks::machine_run("env-other", std::slice::from_ref(&binding), "CI run", || Ok(()))
        .unwrap_err();
    assert!(refused.message().contains("machineAccessMissing"), "{refused}");
    let refused =
        moqui_core::vault_hooks::machine_run("env-ci-runners", &[LOG_LEVEL.to_string()], "CI run", || Ok(())).unwrap_err();
    assert!(refused.message().contains("machineAccessMissing"), "{refused}");
}
