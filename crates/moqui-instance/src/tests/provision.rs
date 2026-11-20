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

//! M1's Tests column (plans/Vault-Instances.md §6), against the fake engine on Docker and on Podman: a provision
//! deposits its items and puts no secret in any container's environment, labels or command; a provision that opens
//! items needs a lease or machine access; dependencies are made first. And what the review of M1 added: the database is
//! made from a one-shot on the server's network, a published port listens on loopback, a login goes only where its
//! item says, an existing user, database or volume is refused, a failed provision is undone and runs again, an
//! environment's approver group, and one action per lease.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::entity::EntityFacade;
use moqui_lib::docker::fake::FakeEngine;
use moqui_lib::object::Value;
use moqui_vault_server::lease::{self, Ask, LeaseRegistry};
use serde_json::{json, Value as Json};
use sha2::Digest;

use super::{all_text, create, field_of, load, login, recorded_for, setup, sys, vault, Vault};
use crate::services::{create_instance, NewInstance};

const ITEM: &str = "moqui.security.vault.VaultItem";
const INSTANCE: &str = "moqui.server.instance.AppInstance";
const ACTION: &str = "moqui.server.instance.InstanceAction";

/// An environment with its operational vault, and a host that is the fake engine.
pub(super) struct Fleet {
    pub(super) engine: FakeEngine,
    pub(super) vault: Vault,
    pub(super) environment: String,
    pub(super) host: String,
}

pub(super) fn fleet(facade: &EntityFacade, tag: &str, engine: FakeEngine) -> Fleet {
    load(facade, "InstanceTypeData.xml");
    load(facade, "VaultTypeData.xml");
    let vault = vault(facade, tag);
    let environment = format!("env-{tag}");
    create(
        facade,
        "moqui.server.instance.InstanceEnvironment",
        json!({ "environmentId": environment, "environmentTypeEnumId": "IetDemo", "vaultId": vault.vault_id }),
    );
    let host = format!("host-{tag}");
    let address = engine.address();
    create(
        facade,
        "moqui.server.instance.InstanceHost",
        json!({ "instanceHostId": host, "hostTypeId": "docker", "hostProtocol": "http", "hostAddress": "127.0.0.1",
                "adminPort": address.port }),
    );
    Fleet {
        engine,
        vault,
        environment,
        host,
    }
}

impl Fleet {
    pub(super) fn instance(&self, facade: &EntityFacade, given: NewInstance) -> String {
        let given = NewInstance {
            environment_id: self.environment.clone(),
            instance_host_id: self.host.clone(),
            ..given
        };
        sys(|| create_instance(facade, &given)).unwrap()
    }

    /// The order in which the engine was asked to create each container, by name.
    fn created(&self) -> Vec<String> {
        self.engine
            .requests()
            .iter()
            .filter(|request| request.method == "POST" && request.path().ends_with("/containers/create"))
            .map(|request| {
                let body = request.json();
                body["name"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| request.target.split("name=").nth(1).map(str::to_string))
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Each secret file the engine was given, by container name and file name, with its value: archives put into a
    /// Docker container, Podman secrets mounted by a Podman one; removed containers too.
    pub(super) fn secret_files(&self) -> BTreeMap<(String, String), Vec<u8>> {
        let mut files = BTreeMap::new();
        let podman = self.engine.podman_secrets_ever();
        for container in self.engine.containers_ever().values() {
            for (_, file) in &container.files {
                if !file.directory {
                    let name = file.path.rsplit('/').next().unwrap_or_default().to_string();
                    files.insert((container.name.clone(), name), file.bytes.clone());
                }
            }
            for secret in container.body["secrets"].as_array().cloned().unwrap_or_default() {
                let source = secret["source"].as_str().unwrap_or_default();
                if let Some(value) = podman.get(source) {
                    let target = secret["target"].as_str().unwrap_or_default().to_string();
                    files.insert((container.name.clone(), target), value.clone());
                }
            }
        }
        files
    }

    /// The create body of the container named `name`.
    pub(super) fn body_of(&self, name: &str) -> Json {
        self.engine
            .containers_ever()
            .values()
            .find(|container| container.name == name)
            .map(|container| container.body.clone())
            .unwrap_or(Json::Null)
    }
}

fn deposited(facade: &EntityFacade, binding: &str) -> String {
    field_of(facade, ITEM, "bindingKey", binding, "itemKeyWrapEnumId")
}

fn actions(facade: &EntityFacade, instance_id: &str) -> Vec<(String, String)> {
    sys(|| {
        facade
            .find(ACTION)
            .unwrap()
            .eq("appInstanceId", Value::string(instance_id))
            .list()
            .unwrap()
            .iter()
            .map(|row| {
                (
                    crate::common::field(row, "actionEnumId"),
                    crate::common::field(row, "statusEnumId"),
                )
            })
            .collect()
    })
}

pub(super) fn set_row(facade: &EntityFacade, entity: &str, key: &str, id: &str, fields: &[(&str, &str)]) {
    sys(|| {
        let mut row = facade
            .find(entity)
            .unwrap()
            .eq(key, Value::string(id))
            .one()
            .unwrap()
            .unwrap();
        for (name, value) in fields {
            row.set(
                *name,
                if value.is_empty() {
                    Value::Null
                } else {
                    Value::string(*value)
                },
            );
        }
        row.update(facade).unwrap();
    });
}

/// A Postgres instance and an mr-moqui instance on it, made by one provision.
pub(super) fn pg_and_app(facade: &EntityFacade, fleet: &Fleet, tag: &str, app_image: &str) -> (String, String) {
    let pg = fleet.instance(
        facade,
        NewInstance {
            instance_image_id: "Postgres18".to_string(),
            instance_name: format!("pg-{tag}"),
            ..NewInstance::default()
        },
    );
    let app = fleet.instance(
        facade,
        NewInstance {
            instance_image_id: app_image.to_string(),
            instance_name: format!("app-{tag}"),
            host_name: format!("app-{tag}.example.test"),
            host_port: "28080".to_string(),
            database_instance_id: pg.clone(),
            ..NewInstance::default()
        },
    );
    (pg, app)
}

/// Dependencies first, deposits, the database from a one-shot on the network, and no secret anywhere but in files.
fn provisions_on(engine: FakeEngine, tag: &str) {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, tag, engine);
    let (pg, app) = pg_and_app(&facade, &fleet, tag, "MoquiLocal");
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    // the preview: both instances, dependencies first, three items deposited, and no lease needed on this host
    let plan = crate::plan::build(&facade, &app).unwrap();
    let steps: Vec<String> = plan.steps.iter().map(crate::plan::Step::id).collect();
    assert_eq!(steps, [pg.clone(), app.clone()]);
    assert!(plan.lease_bindings.is_empty(), "{:?}", plan.lease_bindings);
    let preview = crate::plan::preview(&facade, &plan).unwrap();
    let Some(Value::List(deposits)) = preview.get("deposits") else {
        panic!("{preview:?}")
    };
    assert_eq!(deposits.len(), 3, "{deposits:?}");

    let outcome = crate::provision::provision(&facade, &registry, &app, None).unwrap();
    assert!(
        outcome.report.iter().any(|line| line.contains("deposited")),
        "{:?}",
        outcome.report
    );
    // dependencies are made first: the database's container, then the one-shot that makes the app's database on its
    // network, then the app's container
    let created = fleet.created();
    assert_eq!(created.len(), 3, "{created:?}");
    assert_eq!(created[0], format!("pg-{tag}"));
    assert!(created[1].starts_with(&format!("{app}-db-")), "{created:?}");
    assert_eq!(created[2], format!("app-{tag}"));
    // the one-shot ran on the server's network, with its logins as files, and is gone
    let shot = fleet.body_of(&created[1]);
    assert!(
        shot.to_string().contains(&format!("moqui-env_{tag}"))
            || shot.to_string().contains(&format!("moqui-env-{tag}")),
        "{shot}"
    );
    assert!(!fleet
        .engine
        .containers()
        .values()
        .any(|container| container.name == created[1]));
    // the credentials it made are deposited: wrapped to the deposit key, so the server cannot read them back
    for binding in [
        format!("AppInstance:{pg}:admin"),
        format!("AppInstance:{app}:dbPassword"),
        format!("AppInstance:{app}:ingestKey"),
    ] {
        assert_eq!(deposited(&facade, &binding), "VwDeposit", "{binding}");
    }
    let files = fleet.secret_files();
    let admin = files[&(format!("pg-{tag}"), "postgres-password".to_string())].clone();
    let pgpass = String::from_utf8(files[&(created[1].clone(), "pgpass".to_string())].clone()).unwrap();
    assert_eq!(
        pgpass,
        format!(
            "pg-{tag}:5432:*:postgres:{}\n",
            String::from_utf8(admin.clone()).unwrap()
        )
    );
    let statements = String::from_utf8(files[&(created[1].clone(), "make.sql".to_string())].clone()).unwrap();
    let database = format!("app_{tag}");
    assert!(
        statements.contains(&format!("CREATE DATABASE {database} OWNER {database}")),
        "{statements}"
    );
    assert!(statements.contains("SCRAM-SHA-256$4096:"), "{statements}");
    // no conf connection crossed a network to make it
    assert!(recorded_for(&format!("pg-{tag}:5432")).sql.is_empty());
    // the Postgres image reads its password through POSTGRES_PASSWORD_FILE; the app gets its pairing token as a file,
    // and its port is published on the host's loopback only
    assert!(fleet
        .body_of(&format!("pg-{tag}"))
        .to_string()
        .contains("POSTGRES_PASSWORD_FILE"));
    let app_body = fleet.body_of(&format!("app-{tag}")).to_string();
    assert!(
        app_body.contains("127.0.0.1") && app_body.contains("28080"),
        "{app_body}"
    );
    let token = files[&(format!("app-{tag}"), crate::kinds::PAIRING_TOKEN_FILE.to_string())].clone();
    let hash = moqui_vault::text::hex(&sha2::Sha256::digest(&token));
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &app, "pairingTokenHash"),
        hash
    );
    // its bootstrap bundle waits in memory for its link (M2), with its database password, and is taken once
    assert!(crate::link::bundle_ready(&app));
    let bundle = crate::link::take_bundle(&app).unwrap();
    assert!(
        bundle.contains_key(moqui_core::seal::secrets::DATASOURCE_PASSWORD),
        "{:?}",
        bundle.keys()
    );
    assert!(crate::link::take_bundle(&app).is_none());
    // no secret in any container's environment, labels or command, nor in any other request; none in a vault row
    let vault_rows = all_text(&facade, ITEM);
    for ((container, file), secret) in &files {
        assert!(
            fleet.engine.secret_outside_files(secret).is_empty(),
            "{container} {file}: {:?}",
            fleet.engine.secret_outside_files(secret)
        );
        let text = String::from_utf8_lossy(secret).to_string();
        assert!(!vault_rows.iter().any(|row| row.contains(&text)));
        assert!(!all_text(&facade, ACTION).iter().any(|row| row.contains(&text)));
    }
    for id in [&pg, &app] {
        assert_eq!(
            field_of(&facade, INSTANCE, "appInstanceId", id, "observedState"),
            "running"
        );
        assert!(!field_of(&facade, INSTANCE, "appInstanceId", id, "instanceUuid").is_empty());
    }
    assert_eq!(
        actions(&facade, &app),
        [("IatProvision".to_string(), "IasDone".to_string())]
    );
    // made already: a second provision is refused
    assert!(crate::provision::provision(&facade, &registry, &app, None).is_err());
}

#[test]
fn a_provision_makes_dependencies_first_deposits_its_items_and_puts_no_secret_in_a_container_on_docker() {
    provisions_on(FakeEngine::docker(), "docker");
}

#[test]
fn a_provision_makes_dependencies_first_deposits_its_items_and_puts_no_secret_in_a_container_on_podman() {
    provisions_on(FakeEngine::podman(), "podman");
}

/// A failed provision is undone, all of it: its containers and volumes removed, its deposits withdrawn; then it runs
/// again. A user or database there already on the server is refused and named, and undone the same way.
#[test]
fn a_failed_provision_is_undone_and_runs_again() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "retry", FakeEngine::docker());
    create(
        &facade,
        "moqui.server.instance.InstanceImage",
        json!({ "instanceImageId": "Broken", "instanceKindId": "mr-moqui", "imageName": "broken" }),
    );
    let (pg, app) = pg_and_app(&facade, &fleet, "retry", "Broken");
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let bindings = [
        format!("AppInstance:{pg}:admin"),
        format!("AppInstance:{app}:dbPassword"),
    ];
    let undone = |what: &str| {
        assert!(
            fleet.engine.containers().is_empty(),
            "{what}: {:?}",
            fleet.engine.containers()
        );
        assert!(
            fleet.engine.volumes().is_empty(),
            "{what}: {:?}",
            fleet.engine.volumes()
        );
        for binding in &bindings {
            assert!(deposited(&facade, binding).is_empty(), "{what}: {binding}");
        }
        for id in [&pg, &app] {
            assert!(
                field_of(&facade, INSTANCE, "appInstanceId", id, "instanceUuid").is_empty(),
                "{what}"
            );
        }
        assert!(sys(|| {
            facade
                .find("moqui.server.instance.AppInstanceSecret")
                .unwrap()
                .list()
                .unwrap()
                .iter()
                .all(|row| crate::common::field(row, "depositedDate").is_empty())
        }));
    };
    // the engine refuses the app's image: everything before it is undone
    let failed = crate::provision::provision(&facade, &registry, &app, None).unwrap_err();
    assert!(failed.message().contains("unwell"), "{failed}");
    undone("a refused image");
    let report = field_of(&facade, ACTION, "appInstanceId", &app, "report");
    assert!(report.contains("undone: deposit"), "{report}");

    // the user is there already on the server: refused, named, and undone
    set_row(
        &facade,
        INSTANCE,
        "appInstanceId",
        &app,
        &[("instanceImageId", "MoquiLocal")],
    );
    fleet.engine.one_shot(3, "moqui: user app_retry exists\n");
    let refused = crate::provision::provision(&facade, &registry, &app, None).unwrap_err();
    assert!(refused.message().contains("user app_retry exists"), "{refused}");
    assert!(refused.message().contains("databaseExists"), "{refused}");
    undone("an existing user");

    // and then it runs
    fleet
        .engine
        .one_shot(0, "moqui: made database app_retry and user app_retry\n");
    crate::provision::provision(&facade, &registry, &app, None).unwrap();
    for binding in &bindings {
        assert_eq!(deposited(&facade, binding), "VwDeposit");
    }
    assert_eq!(fleet.engine.containers().len(), 2);
}

/// A volume there already holds an earlier instance's data and credentials: the provision is refused before it makes
/// or deposits anything.
#[test]
fn a_volume_there_already_is_refused_before_anything_is_made() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "kept", FakeEngine::docker());
    let pg = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "Postgres18".to_string(),
            instance_name: "pg-kept".to_string(),
            ..NewInstance::default()
        },
    );
    fleet.engine.host().create_volume("pg-kept-data", &[]).unwrap();
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let refused = crate::provision::provision(&facade, &registry, &pg, None).unwrap_err();
    assert!(refused.message().contains("volumeKept"), "{refused}");
    assert!(deposited(&facade, &format!("AppInstance:{pg}:admin")).is_empty());
    assert!(fleet.created().is_empty());
}

const ADMIN: &str = "a-database-host-admin-password";

/// An app on a database host, whose admin login is a vault item the provision must lease; the item names `item_host`.
fn on_database_host(facade: &EntityFacade, fleet: &mut Fleet, tag: &str, name: &str, item_host: &str) -> String {
    let database_host = format!("dbhost-{tag}");
    if field_of(
        facade,
        "moqui.server.instance.DatabaseHost",
        "databaseHostId",
        &database_host,
        "databaseHostId",
    )
    .is_empty()
    {
        create(
            facade,
            "moqui.server.instance.DatabaseHost",
            json!({ "databaseHostId": database_host, "instanceKindId": "postgres", "hostAddress": format!("{tag}.db.test"),
                    "hostPort": 5432 }),
        );
        fleet.vault.item(
            "database-credential",
            &format!("DatabaseHost:{database_host}:admin"),
            &json!({ "username": "dbadmin", "password": ADMIN, "host": item_host, "port": "5432" }),
        );
    }
    fleet.instance(
        facade,
        NewInstance {
            instance_image_id: "MoquiLocal".to_string(),
            instance_name: name.to_string(),
            database_host_id: database_host,
            ..NewInstance::default()
        },
    )
}

/// Asks the lease of a provision as its plan names it (with where each item goes) and has it approved.
fn approved_lease(facade: &EntityFacade, fleet: &mut Fleet, registry: &LeaseRegistry, app: &str) -> String {
    login(&fleet.vault.maker.user);
    let plan = crate::plan::build(facade, app).unwrap();
    let asked = registry
        .ask(
            facade,
            &Ask {
                vault_id: &fleet.vault.vault_id,
                bindings: &plan.lease_bindings,
                operation: &plan.operation,
                limit: Some(Duration::from_secs(600)),
                environment_id: None,
            },
        )
        .unwrap();
    fleet.vault.approve(&asked.lease_id);
    login(&fleet.vault.maker.user);
    asked.lease_id
}

#[test]
fn a_provision_that_opens_items_needs_a_lease_or_machine_access() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "lease", FakeEngine::docker());
    let app = on_database_host(&facade, &mut fleet, "lease", "app-lease", "lease.db.test");
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let binding = "DatabaseHost:dbhost-lease:admin".to_string();
    let plan = crate::plan::build(&facade, &app).unwrap();
    assert_eq!(plan.lease_bindings, std::slice::from_ref(&binding));
    // the lease says where the login goes, which the approver reads
    assert_eq!(
        plan.operation,
        "provision AppInstance app-lease; DatabaseHost:dbhost-lease:admin to lease.db.test:5432"
    );

    // no lease, no machine access: refused, and nothing is deposited or sent to the host
    let refused = crate::provision::provision(&facade, &registry, &app, None).unwrap_err();
    assert!(refused.message().contains("leaseRequired"), "{refused}");
    assert!(deposited(&facade, &format!("AppInstance:{app}:dbPassword")).is_empty());
    assert!(fleet.engine.requests().is_empty(), "{:?}", fleet.engine.calls());
    assert_eq!(
        actions(&facade, &app),
        [("IatProvision".to_string(), "IasRefused".to_string())]
    );

    // a lease asked for another operation does not do, approved or not
    let other = registry
        .ask(
            &facade,
            &Ask {
                vault_id: &fleet.vault.vault_id,
                bindings: std::slice::from_ref(&binding),
                operation: "remove AppInstance app-lease",
                limit: Some(Duration::from_secs(600)),
                environment_id: None,
            },
        )
        .unwrap();
    fleet.vault.approve(&other.lease_id);
    login(&fleet.vault.maker.user);
    let refused = crate::provision::provision(&facade, &registry, &app, Some(&other.lease_id)).unwrap_err();
    assert!(
        refused.message().contains("not \"provision AppInstance app-lease;"),
        "{refused}"
    );

    // the provision's own lease: one action at a time under it, and it runs over verified TLS to the database host,
    // with the login its item names, and the lease ends with it
    let lease_id = approved_lease(&facade, &mut fleet, &registry, &app);
    let held = crate::actions::open_authority(
        &facade,
        &registry,
        &fleet.environment,
        &fleet.vault.vault_id,
        false,
        &plan.lease_bindings,
        &plan.operation,
        Some(&lease_id),
    )
    .unwrap();
    let busy = crate::provision::provision(&facade, &registry, &app, Some(&lease_id)).unwrap_err();
    assert!(busy.message().contains("leaseBusy"), "{busy}");
    drop(held);
    crate::provision::provision(&facade, &registry, &app, Some(&lease_id)).unwrap();
    let recorded = recorded_for("lease.db.test:5432");
    assert_eq!((recorded.user.as_str(), recorded.password.as_str()), ("dbadmin", ADMIN));
    assert_eq!(recorded.ssl_mode, "verify-full");
    assert!(fleet.engine.secret_outside_files(ADMIN.as_bytes()).is_empty());
    assert_eq!(
        field_of(
            &facade,
            "moqui.security.vault.VaultLease",
            "leaseId",
            &lease_id,
            "statusId"
        ),
        "VlsEnded"
    );
    assert!(!registry.holds(&lease_id));
    assert_eq!(
        deposited(&facade, &format!("AppInstance:{app}:dbPassword")),
        "VwDeposit"
    );

    // machine access: the environment holds the item, and its operations run under a lease approved by "machine"
    let second = on_database_host(&facade, &mut fleet, "lease", "app-machine", "lease.db.test");
    login(&fleet.vault.maker.user);
    let grant = moqui_vault_server::machine::ask_grant(
        &registry,
        &facade,
        &fleet.environment,
        &fleet.vault.vault_id,
        std::slice::from_ref(&binding),
    )
    .unwrap();
    fleet.vault.approve(&grant.lease_id);
    login(&fleet.vault.maker.user);
    moqui_vault_server::machine::complete_grant(&registry, &facade, &grant.lease_id, Duration::from_secs(5)).unwrap();
    // given, but not turned on for the environment: still refused
    assert!(crate::provision::provision(&facade, &registry, &second, None).is_err());
    set_row(
        &facade,
        "moqui.server.instance.InstanceEnvironment",
        "environmentId",
        &fleet.environment,
        &[("machineAccess", "Y")],
    );
    let outcome = crate::provision::provision(&facade, &registry, &second, None).unwrap();
    let lease_id = field_of(&facade, ACTION, "instanceActionId", &outcome.action_id, "leaseId");
    assert_eq!(
        field_of(
            &facade,
            "moqui.security.vault.VaultLease",
            "leaseId",
            &lease_id,
            "machineAccess"
        ),
        "Y"
    );
    assert_eq!(
        field_of(&facade, ACTION, "instanceActionId", &outcome.action_id, "machineAccess"),
        "Y"
    );

    // an environment with an approver group takes actions only from its members
    set_row(
        &facade,
        "moqui.server.instance.InstanceEnvironment",
        "environmentId",
        &fleet.environment,
        &[("approverUserGroupId", "IT_LEASE_APPROVERS")],
    );
    let third = on_database_host(&facade, &mut fleet, "lease", "app-third", "lease.db.test");
    login(&fleet.vault.maker.user);
    let refused = crate::provision::provision(&facade, &registry, &third, None).unwrap_err();
    assert!(refused.message().contains("notEnvironmentMember"), "{refused}");
}

/// A login goes only where its item says: a database host whose row was pointed elsewhere, and a registry login of
/// another registry or for an image of another registry, are refused.
#[test]
fn a_login_goes_only_to_the_server_its_item_names() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "dest", FakeEngine::docker());
    // the item names another server than the row: the row was changed after the item was made
    let app = on_database_host(&facade, &mut fleet, "dest", "app-dest", "real.db.example");
    let registry = lease::registry(ctx.factory());
    let lease_id = approved_lease(&facade, &mut fleet, &registry, &app);
    let refused = crate::provision::provision(&facade, &registry, &app, Some(&lease_id)).unwrap_err();
    assert!(refused.message().contains("destinationMismatch"), "{refused}");
    assert!(
        recorded_for("dest.db.test:5432").password.is_empty(),
        "no connection with the login"
    );
    assert!(
        deposited(&facade, &format!("AppInstance:{app}:dbPassword")).is_empty(),
        "undone"
    );

    // an image whose name pulls from another registry than its row's: refused before any lease
    create(
        &facade,
        "moqui.server.instance.InstanceImage",
        json!({ "instanceImageId": "Elsewhere", "instanceKindId": "mr-moqui", "imageName": "evil.example/moqui:1",
                "registryLocation": "registry.example.com", "registryLogin": "Y" }),
    );
    let pulled = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "Elsewhere".to_string(),
            instance_name: "app-elsewhere".to_string(),
            ..NewInstance::default()
        },
    );
    let refused = crate::plan::build(&facade, &pulled).unwrap_err();
    assert!(refused.message().contains("registryMismatch"), "{refused}");

    // a registry login whose item is another registry's: refused at the pull, before the login is sent
    create(
        &facade,
        "moqui.server.instance.InstanceImage",
        json!({ "instanceImageId": "Private", "instanceKindId": "mr-moqui", "imageName": "registry.example.com/moqui:1",
                "registryLocation": "https://registry.example.com/", "registryLogin": "Y" }),
    );
    fleet.vault.item(
        "registry-login",
        "InstanceImage:Private:registry",
        &json!({ "registry": "other.example.com", "username": "puller", "password": "a-registry-token-value" }),
    );
    let private = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "Private".to_string(),
            instance_name: "app-private".to_string(),
            ..NewInstance::default()
        },
    );
    let plan = crate::plan::build(&facade, &private).unwrap();
    assert!(
        plan.operation
            .contains("InstanceImage:Private:registry to registry.example.com"),
        "{}",
        plan.operation
    );
    let lease_id = approved_lease(&facade, &mut fleet, &registry, &private);
    let refused = crate::provision::provision(&facade, &registry, &private, Some(&lease_id)).unwrap_err();
    assert!(refused.message().contains("destinationMismatch"), "{refused}");
    assert!(fleet.engine.secret_outside_files(b"a-registry-token-value").is_empty());
    assert!(
        !fleet.engine.calls().iter().any(|call| call.contains("/images/create")),
        "{:?}",
        fleet.engine.calls()
    );
}

#[test]
fn an_action_asked_from_a_screen_waits_for_its_lease_then_runs() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "request", FakeEngine::docker());
    let app = on_database_host(&facade, &mut fleet, "request", "app-request", "request.db.test");
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let requested = crate::actions::request(ctx.factory(), &facade, &registry, &app, "provision", false).unwrap();
    assert_eq!(requested.status, crate::actions::WAITING);
    let lease_id = requested.lease_id.clone().unwrap();
    fleet.vault.approve(&lease_id);
    let until = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        let status = field_of(
            &facade,
            ACTION,
            "instanceActionId",
            &requested.action_id,
            "statusEnumId",
        );
        if status != crate::actions::WAITING || std::time::Instant::now() > until {
            break status;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status, crate::actions::DONE);
    assert!(!field_of(&facade, INSTANCE, "appInstanceId", &app, "instanceUuid").is_empty());
}

#[test]
fn stop_start_and_remove_go_through_the_host_type_and_the_reconcile_job_observes() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "life", FakeEngine::docker());
    let pg = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "Postgres18".to_string(),
            instance_name: "pg-life".to_string(),
            ..NewInstance::default()
        },
    );
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    crate::provision::provision(&facade, &registry, &pg, None).unwrap();
    let container = field_of(&facade, INSTANCE, "appInstanceId", &pg, "instanceUuid");
    sys(|| crate::lifecycle::act(&facade, &registry, &pg, "stop", None, false)).unwrap();
    assert_eq!(fleet.engine.containers()[&container].status, "exited");
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &pg, "desiredStateEnumId"),
        "IdsStopped"
    );
    sys(|| crate::lifecycle::act(&facade, &registry, &pg, "start", None, false)).unwrap();
    assert_eq!(fleet.engine.containers()[&container].status, "running");
    // the job observes and changes nothing on the host
    let before = fleet.engine.calls().len();
    let done = sys(|| crate::reconcile::reconcile(&facade)).unwrap();
    assert_eq!(done.observed, 1);
    assert!(
        fleet.engine.calls()[before..]
            .iter()
            .all(|call| call.starts_with("GET ")),
        "{:?}",
        &fleet.engine.calls()[before..]
    );
    sys(|| crate::lifecycle::act(&facade, &registry, &pg, "remove", None, true)).unwrap();
    assert!(fleet.engine.removed().iter().any(|(id, _)| *id == container));
    assert!(fleet.engine.volumes().is_empty(), "{:?}", fleet.engine.volumes());
    assert!(field_of(&facade, INSTANCE, "appInstanceId", &pg, "instanceUuid").is_empty());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &pg, "desiredStateEnumId"),
        "IdsRemoved"
    );
    let verbs: Vec<String> = actions(&facade, &pg).into_iter().map(|(verb, _)| verb).collect();
    for verb in ["IatProvision", "IatStop", "IatStart", "IatRemove"] {
        assert!(verbs.contains(&verb.to_string()), "{verbs:?}");
    }
}
