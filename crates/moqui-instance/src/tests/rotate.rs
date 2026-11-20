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

//! M5's Tests column (plans/Vault-Instances.md §6), against the fake engine and a fake of the instances' link: a
//! rotation changes the source, then the instances, then the item, and puts the old value back when the check fails;
//! an item past its age shows on the fleet screen. And the rules beside them: nothing changes while an instance that
//! uses the credential is not up and unsealed, the rotation list and its by-hand items, a binding's own age, the offer a
//! member's removal makes, and the environment's approver group.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine as _;
use moqui_core::entity::EntityFacade;
use moqui_lib::docker::fake::FakeEngine;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_vault_server::lease::{self, Ask, LeaseRegistry};
use serde_json::{json, Value as Json};

use super::provision::{fleet, pg_and_app, set_row, Fleet};
use super::{all_text, create, field_of, login, setup, sys, Vault};
use crate::deploy::LinkStatus;
use crate::rotate::{self, Instances, Options};

const ITEM: &str = "moqui.security.vault.VaultItem";
const HISTORY: &str = "moqui.security.vault.VaultItemHistory";
const PENDING: &str = "moqui.security.vault.VaultItemPending";
const ACTION: &str = "moqui.server.instance.InstanceAction";

/// The instances as a rotation sees them: each call in order, each value put, whether an instance comes back up after
/// its restart, and at each put what the engine and the vault held then (the one-shots that set a password, and the
/// item's revision and wrap).
struct FakeInstances<'a> {
    facade: &'a EntityFacade,
    engine: &'a FakeEngine,
    binding: String,
    up_before: bool,
    up_after_restart: bool,
    /// At the first put, another write of the item moves its revision.
    write_meanwhile: bool,
    /// At the first put, every one-shot from then on fails (the check, and the old value's put-back at the source).
    shots_fail_after_put: bool,
    restarted: RefCell<bool>,
    calls: RefCell<Vec<String>>,
    puts: RefCell<Vec<String>>,
    seen_at_put: RefCell<Vec<(usize, usize, String, String)>>,
}

impl<'a> FakeInstances<'a> {
    fn new(facade: &'a EntityFacade, engine: &'a FakeEngine, binding: &str) -> Self {
        FakeInstances {
            facade,
            engine,
            binding: binding.to_string(),
            up_before: true,
            up_after_restart: true,
            write_meanwhile: false,
            shots_fail_after_put: false,
            restarted: RefCell::new(false),
            calls: RefCell::new(Vec::new()),
            puts: RefCell::new(Vec::new()),
            seen_at_put: RefCell::new(Vec::new()),
        }
    }
}

impl Instances for FakeInstances<'_> {
    fn status(&self, instance_id: &str) -> Result<LinkStatus, LibError> {
        self.calls.borrow_mut().push(format!("status {instance_id}"));
        let up = if *self.restarted.borrow() {
            self.up_after_restart
        } else {
            self.up_before
        };
        Ok(LinkStatus {
            seal_state: if up { "unsealed" } else { "sealed" }.to_string(),
            health: if up { "ok" } else { "starting" }.to_string(),
            schema_hash: String::new(),
        })
    }

    fn put_secret(&self, instance_id: &str, name: &str, value: &str) -> Result<(), LibError> {
        self.calls.borrow_mut().push(format!("put {instance_id} {name}"));
        let first = self.puts.borrow().is_empty();
        self.puts.borrow_mut().push(value.to_string());
        if first && self.write_meanwhile {
            sys(|| {
                let mut row = self
                    .facade
                    .find(ITEM)
                    .unwrap()
                    .eq("bindingKey", Value::string(&self.binding))
                    .one()
                    .unwrap()
                    .unwrap();
                let revision: i64 = crate::common::field(&row, "revision").parse().unwrap();
                row.set("revision", Value::I64(revision + 1));
                row.update(self.facade).unwrap();
            });
        }
        if first && self.shots_fail_after_put {
            self.engine.one_shot(6, "moqui: refused");
        }
        self.seen_at_put.borrow_mut().push((
            shots(self.engine, "password.sql").len(),
            logins(self.engine),
            field_of(self.facade, ITEM, "bindingKey", &self.binding, "revision"),
            field_of(self.facade, ITEM, "bindingKey", &self.binding, "itemKeyWrapEnumId"),
        ));
        Ok(())
    }

    fn restart(&self, instance_id: &str) -> Result<(), LibError> {
        self.calls.borrow_mut().push(format!("restart {instance_id}"));
        *self.restarted.borrow_mut() = true;
        Ok(())
    }
}

/// The contents of the file `name` of every one-shot the engine ran that had it.
fn shots(engine: &FakeEngine, name: &str) -> Vec<String> {
    engine
        .containers_ever()
        .values()
        .flat_map(|container| container.files.clone())
        .filter(|(_, file)| file.path.ends_with(&format!("/{name}")))
        .map(|(_, file)| String::from_utf8(file.bytes).unwrap())
        .collect()
}

/// How many one-shots signed in to check a value.
fn logins(engine: &FakeEngine) -> usize {
    shots(engine, "database.sh")
        .iter()
        .filter(|script| script.contains("signed in"))
        .count()
}

/// Whether the statement `sql` sets a SCRAM verifier of `password`.
fn sets_password(sql: &str, password: &str) -> bool {
    let Some(verifier) = sql.split('\'').nth(1) else {
        return false;
    };
    let Some(rest) = verifier.strip_prefix("SCRAM-SHA-256$") else {
        return false;
    };
    let Some((iterations, rest)) = rest.split_once(':') else {
        return false;
    };
    let Some((salt, _)) = rest.split_once('$') else {
        return false;
    };
    let salt = base64::engine::general_purpose::STANDARD.decode(salt).unwrap();
    crate::database::scram_verifier_with(password, &salt, iterations.parse().unwrap()) == verifier
}

/// A member takes a deposited item in, as their client does (`take#VaultDeposit`): its key unwrapped with the deposit
/// key and wrapped by the vault key, its body as it is.
pub(super) fn take_in(vault_of: &mut Vault, facade: &EntityFacade, binding: &str) {
    let vault_id = vault_of.vault_id.clone();
    let vault = vault_of
        .maker
        .call("get#Vault", json!({ "vaultId": vault_id }))
        .unwrap();
    let text = |name: &str| field_of(facade, ITEM, "bindingKey", binding, name);
    let label = json!({ "item_type": text("vaultItemTypeId"), "binding": binding, "name": text("itemName") });
    let adopted = vault_of
        .maker
        .op(
            json!({ "op": "deposit_adopt", "vault_id": vault_id, "item_id": text("vaultItemId"), "key_version": 1,
                    "label": label, "deposit_wrapped_private": vault["depositWrappedPrivate"],
                    "wrapped_item_key": text("wrappedItemKey") }),
        )
        .unwrap();
    vault_of
        .maker
        .call(
            "take#VaultDeposit",
            json!({ "vaultItemId": text("vaultItemId"), "expectedRevision": text("revision").parse::<i64>().unwrap(),
                    "keyVersion": 1, "wrappedItemKey": adopted["wrapped_item_key"] }),
        )
        .unwrap();
}

/// The item's fields, opened in the maker's client (once it is taken in).
fn opened(fleet: &mut Fleet, facade: &EntityFacade, binding: &str) -> Json {
    body_of(fleet, facade, binding)["fields"].clone()
}

/// A member writes notes and tags on the item, in their client: a new item key, the fields as they were.
fn with_notes(fleet: &mut Fleet, facade: &EntityFacade, binding: &str) {
    let text = |name: &str| field_of(facade, ITEM, "bindingKey", binding, name);
    let label = json!({ "item_type": text("vaultItemTypeId"), "binding": binding, "name": text("itemName") });
    let mut body = body_of(fleet, facade, binding);
    body["notes"] = json!("rotate with care");
    body["tags"] = json!(["db"]);
    let sealed = fleet
        .vault
        .maker
        .op(
            json!({ "op": "item_seal", "vault_id": fleet.vault.vault_id, "item_id": text("vaultItemId"), "key_version": 1,
                    "label": label, "body": body.to_string() }),
        )
        .unwrap();
    fleet
        .vault
        .maker
        .call(
            "update#VaultItem",
            json!({ "vaultItemId": text("vaultItemId"), "expectedRevision": text("revision").parse::<i64>().unwrap(),
                    "keyVersion": 1, "wrappedItemKey": sealed["wrapped_item_key"], "bodyCipher": sealed["sealed_body"],
                    "newItemKey": true }),
        )
        .unwrap();
}

/// The item's whole body, opened in the maker's client (once it is taken in).
fn body_of(fleet: &mut Fleet, facade: &EntityFacade, binding: &str) -> Json {
    let text = |name: &str| field_of(facade, ITEM, "bindingKey", binding, name);
    let label = json!({ "item_type": text("vaultItemTypeId"), "binding": binding, "name": text("itemName") });
    let body = fleet
        .vault
        .maker
        .op(
            json!({ "op": "item_open", "vault_id": fleet.vault.vault_id, "item_id": text("vaultItemId"), "key_version": 1,
                    "label": label, "wrapped_item_key": text("wrappedItemKey"), "sealed_body": text("bodyCipher") }),
        )
        .unwrap();
    serde_json::from_str::<Json>(body["body"].as_str().unwrap()).unwrap()
}

/// A Postgres and an mr-moqui instance on it, provisioned, with the app's database password and the database's admin
/// login taken in by a member.
fn provisioned(facade: &EntityFacade, fleet: &mut Fleet, registry: &LeaseRegistry, tag: &str) -> (String, String) {
    let (pg, app) = pg_and_app(facade, fleet, tag, "MoquiLocal");
    login(&fleet.vault.maker.user);
    crate::provision::provision(facade, registry, &app, None).unwrap();
    for binding in [
        format!("AppInstance:{pg}:admin"),
        format!("AppInstance:{app}:dbPassword"),
    ] {
        take_in(&mut fleet.vault, facade, &binding);
    }
    (pg, app)
}

/// A lease of a rotation's plan, asked and approved.
pub(super) fn approved(
    vault: &mut Vault,
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    plan: &rotate::Plan,
) -> String {
    login(&vault.maker.user);
    let lease = registry
        .ask(
            facade,
            &Ask {
                vault_id: &plan.vault_id,
                bindings: &plan.bindings,
                operation: &plan.operation,
                limit: None,
                environment_id: None,
            },
        )
        .unwrap();
    vault.approve(&lease.lease_id);
    registry
        .wait_open(facade, &lease.lease_id, Duration::from_secs(5))
        .unwrap();
    login(&vault.maker.user);
    lease.lease_id
}

fn actions(facade: &EntityFacade, instance_id: &str) -> Vec<(String, String, String)> {
    sys(|| {
        facade
            .find(ACTION)
            .unwrap()
            .eq("appInstanceId", Value::string(instance_id))
            .eq("actionEnumId", Value::string("IatRotate"))
            .order_by("instanceActionId", false)
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    crate::common::field(row, "statusEnumId"),
                    crate::common::field(row, "report"),
                    crate::common::field(row, "leaseId"),
                )
            })
            .collect()
    })
}

/// The source first, then the instance, then the item: the database user's password set by a one-shot on the
/// database's network, then put on the instance and the instance restarted, then the new value signed in with and the
/// instance up again, then the item's next revision deposited with the one before in its history.
#[test]
fn a_rotation_changes_the_source_then_the_instances_then_the_item() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "rot", FakeEngine::docker());
    let registry = lease::registry(ctx.factory());
    let (pg, app) = provisioned(&facade, &mut fleet, &registry, "rot");
    let binding = format!("AppInstance:{app}:dbPassword");
    // a member's notes and tags on the item, which a rotation keeps: revision 3
    with_notes(&mut fleet, &facade, &binding);
    let old = opened(&mut fleet, &facade, &binding)["password"]
        .as_str()
        .unwrap()
        .to_string();
    let plan = sys(|| rotate::plan(&facade, &fleet.environment, std::slice::from_ref(&binding))).unwrap();
    assert_eq!(plan.credentials.len(), 1);
    // the lease names the item, the database's admin login that sets it, and where each goes
    assert_eq!(plan.bindings, [binding.clone(), format!("AppInstance:{pg}:admin")]);
    assert!(
        plan.operation
            .contains(&format!("AppInstance:{pg}:admin to pg-rot:5432")),
        "{}",
        plan.operation
    );
    let lease_id = approved(&mut fleet.vault, &facade, &registry, &plan);
    let vault_revision = fleet
        .vault
        .maker
        .call("get#Vault", json!({ "vaultId": fleet.vault.vault_id }))
        .unwrap()["vault"]["revision"]
        .clone();
    let instances = FakeInstances::new(&facade, &fleet.engine, &binding);
    let outcome = sys(|| {
        rotate::run(
            &facade,
            &registry,
            &instances,
            &plan,
            Some(&lease_id),
            &Options {
                wait: Duration::from_secs(5),
            },
        )
    })
    .unwrap();
    assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
    assert_eq!(outcome.rotated, [(binding.clone(), 4)]);

    // the instance was up before, then took the value and was restarted, then came back
    let calls = instances.calls.borrow().clone();
    assert_eq!(
        calls,
        [
            format!("status {app}"),
            format!("put {app} {}", moqui_core::seal::secrets::DATASOURCE_PASSWORD),
            format!("restart {app}"),
            format!("status {app}"),
        ]
    );
    let new = instances.puts.borrow()[0].clone();
    assert_ne!(new, old);
    // when the instance took it, the source had it already (one one-shot set it, none had signed in with it yet),
    // and the item was still the revision a member took in
    assert_eq!(
        instances.seen_at_put.borrow()[0],
        (1, 0, "3".to_string(), "VwVault".to_string())
    );
    let set = shots(&fleet.engine, "password.sql");
    assert_eq!(set.len(), 1);
    assert!(
        set[0].starts_with("ALTER ROLE app_rot WITH PASSWORD 'SCRAM-SHA-256$"),
        "{}",
        set[0]
    );
    assert!(
        sets_password(&set[0], &new),
        "the statement sets the value put on the instance"
    );
    assert!(!set[0].contains(&new), "the password itself is in no statement");
    // then the check signed in with it, as the user, on its database
    assert_eq!(logins(&fleet.engine), 1);
    let pgpass = shots(&fleet.engine, "pgpass");
    assert!(
        pgpass
            .iter()
            .any(|line| line == &format!("pg-rot:5432:*:app_rot:{new}\n")),
        "{pgpass:?}"
    );

    // then the item: its next revision, deposited (a member takes it in), the one before in its history
    assert_eq!(field_of(&facade, ITEM, "bindingKey", &binding, "revision"), "4");
    assert_eq!(
        field_of(&facade, ITEM, "bindingKey", &binding, "itemKeyWrapEnumId"),
        "VwDeposit"
    );
    let item_id = field_of(&facade, ITEM, "bindingKey", &binding, "vaultItemId");
    let kept: Vec<(String, String)> = sys(|| {
        facade
            .find(HISTORY)
            .unwrap()
            .eq("vaultItemId", Value::string(&item_id))
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    crate::common::field(row, "revision"),
                    crate::common::field(row, "itemKeyWrapEnumId"),
                )
            })
            .collect()
    });
    assert!(kept.contains(&("3".to_string(), "VwVault".to_string())), "{kept:?}");
    assert!(
        all_text(&facade, PENDING).is_empty(),
        "the staged value became the revision"
    );
    // the provision's withdraw does not take a rotated deposit
    let refused =
        sys(|| moqui_vault_server::deposit::withdraw_item(&facade, &fleet.vault.vault_id, &item_id, &binding))
            .unwrap_err();
    assert!(refused.message().contains("not withdrawn"), "{refused}");
    // a member removal computed before the rotation is refused: the rotation moved the vault's revision
    let refused = fleet
        .vault
        .maker
        .call(
            "remove#VaultMember",
            json!({ "vaultId": fleet.vault.vault_id, "userId": fleet.vault.approver.user,
                    "expectedRevision": vault_revision, "entry": { "version": 2 }, "memberWraps": [{ "user_id": "x" }] }),
        )
        .unwrap_err();
    assert!(refused.contains("The vault changed since you read it"), "{refused}");
    take_in(&mut fleet.vault, &facade, &binding);
    let body = body_of(&mut fleet, &facade, &binding);
    assert_eq!(body["fields"]["password"], json!(new));
    assert_eq!(
        body["fields"]["username"],
        json!("app_rot"),
        "the item's other fields stay"
    );
    assert_eq!(body["notes"], json!("rotate with care"), "its notes stay");
    assert_eq!(body["tags"], json!(["db"]), "its tags stay");
    // no row holds the new value in the clear; the action says what was done and ran under the lease
    for entity in [ACTION, "moqui.security.vault.VaultEvent", ITEM, HISTORY] {
        assert!(
            !all_text(&facade, entity).iter().any(|text| text.contains(&new)),
            "{entity}"
        );
    }
    let done = actions(&facade, &app);
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].0, "IasDone");
    assert_eq!(done[0].2, lease_id);
    assert!(done[0].1.contains("deposited revision 4"), "{}", done[0].1);
    // the lease ended with the rotation
    assert!(!registry.holds(&lease_id));
}

/// When the check fails (the instance does not come back), the old value is put back at the source and on the
/// instance, and the item is not changed; and while an instance that uses the credential is not up and unsealed, a
/// rotation changes nothing at all.
#[test]
fn a_failed_check_puts_the_old_value_back() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "back", FakeEngine::docker());
    let registry = lease::registry(ctx.factory());
    let (_pg, app) = provisioned(&facade, &mut fleet, &registry, "back");
    let binding = format!("AppInstance:{app}:dbPassword");
    let old = opened(&mut fleet, &facade, &binding)["password"]
        .as_str()
        .unwrap()
        .to_string();
    let plan = sys(|| rotate::plan(&facade, &fleet.environment, std::slice::from_ref(&binding))).unwrap();
    let options = Options { wait: Duration::ZERO };

    // the instance is sealed: nothing is changed, not even the source
    let lease_id = approved(&mut fleet.vault, &facade, &registry, &plan);
    let mut sealed = FakeInstances::new(&facade, &fleet.engine, &binding);
    sealed.up_before = false;
    let outcome = sys(|| rotate::run(&facade, &registry, &sealed, &plan, Some(&lease_id), &options)).unwrap();
    assert!(outcome.rotated.is_empty());
    assert!(
        outcome.failed[0].1.contains("nothing was changed"),
        "{:?}",
        outcome.failed
    );
    assert!(shots(&fleet.engine, "password.sql").is_empty());
    assert!(sealed.puts.borrow().is_empty());

    // it does not come back after the restart: the old value goes back to the source and to the instance
    let lease_id = approved(&mut fleet.vault, &facade, &registry, &plan);
    let mut down = FakeInstances::new(&facade, &fleet.engine, &binding);
    down.up_after_restart = false;
    let outcome = sys(|| rotate::run(&facade, &registry, &down, &plan, Some(&lease_id), &options)).unwrap();
    assert!(outcome.rotated.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0].1.contains("the old value was put back"),
        "{:?}",
        outcome.failed
    );
    let puts = down.puts.borrow().clone();
    assert_eq!(puts.len(), 2, "the new value, then the old one back");
    let new = puts[0].clone();
    assert_ne!(new, old);
    assert_eq!(puts[1], old);
    let set = shots(&fleet.engine, "password.sql");
    assert_eq!(set.len(), 2, "{set:?}");
    assert!(set.iter().any(|sql| sets_password(sql, &new)));
    assert!(
        set.iter().any(|sql| sets_password(sql, &old)),
        "the old value is back at the source"
    );
    assert_eq!(
        down.calls
            .borrow()
            .iter()
            .filter(|call| call.starts_with("restart "))
            .count(),
        2,
        "restarted with the new value, then with the old"
    );
    // the item is the revision a member took in, with no history of this rotation
    assert_eq!(field_of(&facade, ITEM, "bindingKey", &binding, "revision"), "2");
    assert_eq!(
        field_of(&facade, ITEM, "bindingKey", &binding, "itemKeyWrapEnumId"),
        "VwVault"
    );
    assert_eq!(opened(&mut fleet, &facade, &binding)["password"], json!(old));
    assert!(
        all_text(&facade, PENDING).is_empty(),
        "the staged value is dropped once the old one is back"
    );
    let failed = actions(&facade, &app);
    assert_eq!(failed.len(), 2);
    assert!(failed.iter().all(|(status, _, _)| status == "IasFailed"), "{failed:?}");
    assert!(failed[1].1.contains("put the old value of"), "{}", failed[1].1);
    assert!(!failed[1].1.contains(&new) && !failed[1].1.contains(&old));
}

/// An item past its age shows on the fleet screen and in the rotation list, with what the manager rotates and what is
/// done by hand; a binding's own age is over its type's; a member's removal offers the vault's items; an environment's
/// approver group is asked of a rotation's caller, and a rotation without machine access waits for its lease.
#[test]
fn an_item_past_its_age_shows_on_the_fleet_screen() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "age", FakeEngine::docker());
    let registry = lease::registry(ctx.factory());
    let (pg, app) = provisioned(&facade, &mut fleet, &registry, "age");
    let now = moqui_core::user::now_millis();
    let fleet_row = |id: &str| {
        sys(|| crate::fleet::fleet(&facade, now))
            .unwrap()
            .into_iter()
            .find(|row| row.get("appInstanceId") == Some(&Value::string(id)))
            .unwrap()
    };
    assert_eq!(fleet_row(&app).get("itemsPastRotation"), Some(&Value::I64(0)));

    // the database password was set 100 days ago: past a database credential's 90
    let db_password = format!("AppInstance:{app}:dbPassword");
    let long_ago = moqui_core::user::stamp_value(now - 100 * 86_400_000);
    sys(|| {
        let mut row = facade
            .find(ITEM)
            .unwrap()
            .eq("bindingKey", Value::string(&db_password))
            .one()
            .unwrap()
            .unwrap();
        row.set("rotatedDate", long_ago.clone());
        row.update(&facade).unwrap();
    });
    assert_eq!(fleet_row(&app).get("itemsPastRotation"), Some(&Value::I64(1)));
    assert_eq!(fleet_row(&pg).get("itemsPastRotation"), Some(&Value::I64(0)));
    let by_environment = sys(|| rotate::due::past_by_environment(&facade, now)).unwrap();
    assert!(
        by_environment.contains(&(fleet.environment.clone(), 1)),
        "{by_environment:?}"
    );

    // the list: the database password due and rotated by the manager; the ingest key by hand, with what to do
    let list = sys(|| rotate::due::list(&facade, &fleet.environment, now)).unwrap();
    let of = |binding: &str| list.iter().find(|age| age.binding == binding).cloned().unwrap();
    let password = of(&db_password);
    assert!(password.due && password.rotatable && !password.deposited);
    assert_eq!((password.rotation_days, password.age_days), (Some(90), 100));
    let ingest = of(&format!("AppInstance:{app}:ingestKey"));
    assert!(!ingest.due && !ingest.rotatable && ingest.deposited);
    assert!(
        ingest
            .by_hand
            .contains(&format!("Monitor › Sources for the source instance:{app}")),
        "{}",
        ingest.by_hand
    );
    // a binding's own age, over its type's 365 days: set by a manager of the vault
    fleet
        .vault
        .maker
        .call(
            "set#VaultItemRotation",
            json!({ "vaultItemId": ingest.item_id, "rotationDays": "1" }),
        )
        .unwrap();
    let refused = fleet
        .vault
        .approver
        .call(
            "set#VaultItemRotation",
            json!({ "vaultItemId": ingest.item_id, "rotationDays": "1" }),
        )
        .unwrap_err();
    assert!(refused.contains("does not allow"), "{refused}");
    let later = now + 2 * 86_400_000;
    let ingest = sys(|| rotate::due::list(&facade, &fleet.environment, later))
        .unwrap()
        .into_iter()
        .find(|age| age.binding == ingest.binding)
        .unwrap();
    assert!(ingest.due && ingest.rotation_days == Some(1));
    // the incident list rotates what has a rotate service, and names the rest with why
    let plan = sys(|| {
        rotate::plan(
            &facade,
            &fleet.environment,
            &list.iter().map(|age| age.binding.clone()).collect::<Vec<_>>(),
        )
    })
    .unwrap();
    let rotated: Vec<&str> = plan
        .credentials
        .iter()
        .map(|credential| credential.binding.as_str())
        .collect();
    // the database's users before its admin login, whose old value their change signs in with
    assert_eq!(rotated, [db_password.clone(), format!("AppInstance:{pg}:admin")]);
    assert!(plan
        .not_rotated
        .iter()
        .any(|not| not.binding.ends_with(":ingestKey") && not.reason.contains("Monitor")));

    // a member's removal offers each item of the vault, with what the manager can rotate
    let offer = sys(|| rotate::due::offer(&facade, &fleet.vault.vault_id));
    let offered: BTreeMap<String, bool> = offer
        .iter()
        .map(|row| {
            let Value::Map(row) = row else { panic!("{row:?}") };
            (
                to_plain_string(&row["bindingKey"]),
                matches!(row["rotatable"], Value::Bool(true)),
            )
        })
        .collect();
    assert_eq!(offered.get(&db_password), Some(&true));
    assert_eq!(offered.get(&format!("AppInstance:{app}:ingestKey")), Some(&false));

    // a member who changed it by hand writes it marked rotated: its age starts again; a write not so marked keeps it
    let write = |vault: &mut super::Vault, rotated: bool| {
        let text = |name: &str| field_of(&facade, ITEM, "bindingKey", &db_password, name);
        vault
            .maker
            .call(
                "update#VaultItem",
                json!({ "vaultItemId": text("vaultItemId"), "expectedRevision": text("revision").parse::<i64>().unwrap(),
                        "keyVersion": 1, "wrappedItemKey": text("wrappedItemKey"), "bodyCipher": text("bodyCipher"),
                        "rotated": rotated }),
            )
            .unwrap();
        sys(|| rotate::due::list(&facade, &fleet.environment, moqui_core::user::now_millis()))
            .unwrap()
            .into_iter()
            .find(|age| age.binding == db_password)
            .unwrap()
            .due
    };
    assert!(
        write(&mut fleet.vault, false),
        "a write not marked rotated keeps its age"
    );
    assert!(!write(&mut fleet.vault, true), "a write marked rotated starts it again");

    // an item written before rotation ages has no rotatedDate: the start gives it the upgrade's time, so it is not due
    let ingest_binding = format!("AppInstance:{app}:ingestKey");
    let set_date = |binding: &str, value: Value| {
        sys(|| {
            let mut row = facade
                .find(ITEM)
                .unwrap()
                .eq("bindingKey", Value::string(binding))
                .one()
                .unwrap()
                .unwrap();
            row.set("rotatedDate", value);
            row.update(&facade).unwrap();
        });
    };
    set_date(&ingest_binding, Value::Null);
    moqui_vault_server::items::backfill_rotated_dates(ctx.factory());
    assert!(!field_of(&facade, ITEM, "bindingKey", &ingest_binding, "rotatedDate").is_empty());

    // the job backs off an item whose last rotation failed lately, and tells the administrators instead
    set_date(&db_password, long_ago.clone());
    create(
        &facade,
        "moqui.server.instance.InstanceRotation",
        json!({ "vaultId": fleet.vault.vault_id, "bindingKey": db_password, "lastStatusEnumId": "IasFailed",
                "lastEndDate": moqui_core::user::now_millis(), "lastActionId": "an-earlier-try" }),
    );
    let (errors, ran) = sys(|| call("rotate#DueCredentials", &[]));
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(ran["heldBackCount"], Value::I64(1), "{ran:?}");
    assert_eq!(ran["askedCount"], Value::I64(0), "{ran:?}");

    // the environment's approver group: an INSTANCE_ADMIN who is not in it is refused, and nothing is asked
    create(
        &facade,
        "moqui.security.UserGroup",
        json!({ "userGroupId": "IT_ROTATORS" }),
    );
    set_row(
        &facade,
        "moqui.server.instance.InstanceEnvironment",
        "environmentId",
        &fleet.environment,
        &[("approverUserGroupId", "IT_ROTATORS")],
    );
    login(&fleet.vault.maker.user);
    let leases_before = all_text(&facade, "moqui.security.vault.VaultLease").len();
    let (refused, _) = call(
        "request#InstanceRotation",
        &[("environmentId", &fleet.environment), ("bindingKey", &db_password)],
    );
    assert!(refused.contains("notEnvironmentMember"), "{refused}");
    let (refused, _) = call("get#InstanceRotations", &[("environmentId", &fleet.environment)]);
    assert!(refused.contains("notEnvironmentMember"), "{refused}");
    // the job, run by such a caller, leaves the environment alone
    let (errors, ran) = call("rotate#DueCredentials", &[]);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(ran["skippedCount"], Value::I64(1), "{ran:?}");
    assert_eq!(
        all_text(&facade, "moqui.security.vault.VaultLease").len(),
        leases_before
    );
    assert!(actions(&facade, &app).is_empty());

    // a member of the group asks: with no machine access the rotation waits for its lease
    create(
        &facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": "IT_ROTATORS", "userId": fleet.vault.maker.user, "fromDate": 0 }),
    );
    login(&fleet.vault.maker.user);
    let (errors, asked) = call(
        "request#InstanceRotation",
        &[("environmentId", &fleet.environment), ("bindingKey", &db_password)],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(asked["statusEnumId"], Value::string("IasWaiting"));
    assert!(!moqui_lib::object::is_empty(&asked["leaseId"]));
    let waiting = to_plain_string(&asked["instanceActionId"]);
    assert_eq!(actions(&facade, &app).len(), 1);
    // let the waiting thread go
    assert!(sys(|| crate::actions::cancel_waiting(&facade, &registry, &waiting, "test over")).unwrap());
}

/// One rotation of an item at a time, and only from the revision it read: a binding another rotation holds is refused
/// before anything changes; an item written while it ran is not overwritten (the old value goes back and the staged
/// one is dropped); and when the old value cannot be put back at the source, the new one is kept as a pending revision
/// for a member to recover.
#[test]
fn a_rotation_is_one_at_a_time_and_keeps_a_value_it_cannot_take_back() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "once", FakeEngine::docker());
    let registry = lease::registry(ctx.factory());
    let (_pg, app) = provisioned(&facade, &mut fleet, &registry, "once");
    let binding = format!("AppInstance:{app}:dbPassword");
    let plan = sys(|| rotate::plan(&facade, &fleet.environment, std::slice::from_ref(&binding))).unwrap();
    let options = Options { wait: Duration::ZERO };

    // another rotation of it runs (its claim, on any cluster member): refused, nothing changed
    let until = moqui_core::user::now_millis() + 60_000;
    sys(|| rotate::claim::claim(&facade, &fleet.vault.vault_id, &binding, "another-run", until)).unwrap();
    let lease_id = approved(&mut fleet.vault, &facade, &registry, &plan);
    let instances = FakeInstances::new(&facade, &fleet.engine, &binding);
    let outcome = sys(|| rotate::run(&facade, &registry, &instances, &plan, Some(&lease_id), &options)).unwrap();
    assert!(outcome.failed[0].1.contains("rotationRunning"), "{:?}", outcome.failed);
    assert!(shots(&fleet.engine, "password.sql").is_empty());
    assert!(instances.calls.borrow().is_empty());
    sys(|| rotate::claim::release(&facade, &fleet.vault.vault_id, &binding, "another-run", None));
    // and a request while one waits for its lease is refused with no second lease
    login(&fleet.vault.maker.user);
    let (_first, _) = sys(|| {
        rotate::request(
            ctx.factory(),
            &facade,
            &registry,
            &fleet.environment,
            std::slice::from_ref(&binding),
        )
    })
    .unwrap();
    let lease_rows = || {
        sys(|| {
            facade
                .find("moqui.security.vault.VaultLease")
                .unwrap()
                .list()
                .unwrap()
                .len()
        })
    };
    let leases = lease_rows();
    let refused = sys(|| {
        rotate::request(
            ctx.factory(),
            &facade,
            &registry,
            &fleet.environment,
            std::slice::from_ref(&binding),
        )
    })
    .map(|_| ())
    .unwrap_err();
    assert!(refused.message().contains("rotationRunning"), "{refused}");
    assert_eq!(lease_rows(), leases, "no second lease is asked");
    let waiting = actions(&facade, &app)
        .into_iter()
        .filter(|(status, _, _)| status == "IasWaiting")
        .count();
    assert_eq!(waiting, 1);
    let waiting_id = sys(|| {
        facade
            .find(ACTION)
            .unwrap()
            .eq("appInstanceId", Value::string(&app))
            .eq("statusEnumId", Value::string("IasWaiting"))
            .one()
            .unwrap()
            .map(|row| crate::common::field(&row, "instanceActionId"))
            .unwrap()
    });
    assert!(sys(|| crate::actions::cancel_waiting(&facade, &registry, &waiting_id, "test")).unwrap());
    // its claim held until its lease's time; the test lets it go
    let token = sys(|| {
        facade
            .find("moqui.server.instance.InstanceRotation")
            .unwrap()
            .eq("bindingKey", Value::string(&binding))
            .one()
            .unwrap()
            .map(|row| crate::common::field(&row, "claimToken"))
            .unwrap()
    });
    sys(|| rotate::claim::release(&facade, &fleet.vault.vault_id, &binding, &token, None));

    // the item is written while the rotation runs: its new value is not written over it, the old one goes back
    let revision = field_of(&facade, ITEM, "bindingKey", &binding, "revision");
    let lease_id = approved(&mut fleet.vault, &facade, &registry, &plan);
    let mut meanwhile = FakeInstances::new(&facade, &fleet.engine, &binding);
    meanwhile.write_meanwhile = true;
    let outcome = sys(|| rotate::run(&facade, &registry, &meanwhile, &plan, Some(&lease_id), &options)).unwrap();
    assert!(outcome.failed[0].1.contains("itemChanged"), "{:?}", outcome.failed);
    assert!(
        outcome.failed[0].1.contains("the old value was put back"),
        "{:?}",
        outcome.failed
    );
    let written: i64 = revision.parse::<i64>().unwrap() + 1;
    assert_eq!(
        field_of(&facade, ITEM, "bindingKey", &binding, "revision"),
        written.to_string()
    );
    assert_eq!(
        field_of(&facade, ITEM, "bindingKey", &binding, "itemKeyWrapEnumId"),
        "VwVault"
    );
    assert!(all_text(&facade, PENDING).is_empty());
    let puts = meanwhile.puts.borrow().clone();
    let set = shots(&fleet.engine, "password.sql");
    assert!(
        set.iter().any(|sql| sets_password(sql, &puts[1])),
        "the old value is back at the source"
    );

    // the check fails and so does the old value's put-back at the source: the new value is kept, pending
    let plan = sys(|| rotate::plan(&facade, &fleet.environment, std::slice::from_ref(&binding))).unwrap();
    let lease_id = approved(&mut fleet.vault, &facade, &registry, &plan);
    let mut stuck = FakeInstances::new(&facade, &fleet.engine, &binding);
    stuck.shots_fail_after_put = true;
    let outcome = sys(|| rotate::run(&facade, &registry, &stuck, &plan, Some(&lease_id), &options)).unwrap();
    fleet.engine.one_shot(0, "");
    assert!(
        outcome.failed[0].1.contains("NOT put back everywhere"),
        "{:?}",
        outcome.failed
    );
    assert_eq!(
        field_of(&facade, ITEM, "bindingKey", &binding, "revision"),
        written.to_string()
    );
    let kept = field_of(&facade, PENDING, "bindingKey", &binding, "keptDate");
    assert!(!kept.is_empty(), "the new value is kept for a member");
    let item_id = field_of(&facade, ITEM, "bindingKey", &binding, "vaultItemId");
    let history = fleet
        .vault
        .maker
        .call("list#VaultItemHistory", json!({ "vaultItemId": item_id }))
        .unwrap();
    assert_eq!(history["pendingList"].as_array().unwrap().len(), 1, "{history}");
    assert!(
        history["pendingList"][0].get("bodyCipher").is_none(),
        "no operational body in a list"
    );
    let events = all_text(&facade, "moqui.security.vault.VaultEvent").join("\n");
    assert!(events.contains("kept as pending revision"), "{events}");
    // the rotation's claim is let go, with its failure for the job to back off from
    let failed = sys(|| rotate::claim::failed_since(&facade, &fleet.vault.vault_id, &binding, 0)).unwrap();
    assert!(failed.is_some());
}

fn call(service: &str, parameters: &[(&str, &str)]) -> (String, BTreeMap<String, Value>) {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), Value::string(*value)))
        .collect();
    let answer = moqui_core::service::sync(&format!("org.moqui.dev.InstanceServices.{service}"), &parameters);
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("\n");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    match answer {
        Err(err) => (format!("{err} {errors}"), BTreeMap::new()),
        Ok(out) => (errors, out),
    }
}
