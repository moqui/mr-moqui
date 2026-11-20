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
//! deposits its items and puts no secret in any container's environment, labels or command; a provision without a
//! lease or machine access is refused; dependencies are made first. And the actions after it, and the reconcile job.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::entity::EntityFacade;
use moqui_lib::docker::fake::FakeEngine;
use moqui_lib::object::Value;
use moqui_vault_server::lease::{self, Ask};
use serde_json::json;
use sha2::Digest;

use super::{all_text, create, field_of, load, login, recorded_for, setup, sys, vault, Vault};
use crate::services::{create_instance, NewInstance};

const ITEM: &str = "moqui.security.vault.VaultItem";
const INSTANCE: &str = "moqui.server.instance.AppInstance";

/// An environment with its operational vault, and a host that is the fake engine.
struct Fleet {
    engine: FakeEngine,
    vault: Vault,
    environment: String,
    host: String,
}

fn fleet(facade: &EntityFacade, tag: &str, engine: FakeEngine) -> Fleet {
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
    fn instance(&self, facade: &EntityFacade, given: NewInstance) -> String {
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
    /// Docker container, Podman secrets mounted by a Podman one.
    fn secret_files(&self) -> BTreeMap<(String, String), Vec<u8>> {
        let mut files = BTreeMap::new();
        let podman = self.engine.podman_secrets();
        for container in self.engine.containers().values() {
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
}

fn deposited(facade: &EntityFacade, binding: &str) -> String {
    field_of(facade, ITEM, "bindingKey", binding, "itemKeyWrapEnumId")
}

fn actions(facade: &EntityFacade, instance_id: &str) -> Vec<(String, String)> {
    sys(|| {
        facade
            .find("moqui.server.instance.InstanceAction")
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

/// Dependencies first, deposits, and no secret anywhere but in the files, on one engine.
fn provisions_on(engine: FakeEngine, tag: &str) {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, tag, engine);
    let port = if tag == "docker" { "25432" } else { "25433" };
    let pg = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "Postgres18".to_string(),
            instance_name: format!("pg-{tag}"),
            host_port: port.to_string(),
            ..NewInstance::default()
        },
    );
    let app = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "MoquiLocal".to_string(),
            instance_name: format!("app-{tag}"),
            host_name: format!("app-{tag}.example.test"),
            database_instance_id: pg.clone(),
            ..NewInstance::default()
        },
    );
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
    // dependencies are made first: the database's container before the app's, and the app's database on it before
    // the app's container
    assert_eq!(fleet.created(), [format!("pg-{tag}"), format!("app-{tag}")]);
    // the credentials it made are deposited: wrapped to the deposit key, so the server cannot read them back
    for binding in [
        format!("AppInstance:{pg}:admin"),
        format!("AppInstance:{app}:dbPassword"),
        format!("AppInstance:{app}:ingestKey"),
    ] {
        assert_eq!(deposited(&facade, &binding), "VwDeposit", "{binding}");
    }
    // the database and user on the new Postgres, with the admin login it was made with
    let files = fleet.secret_files();
    let admin = files[&(format!("pg-{tag}"), "postgres-password".to_string())].clone();
    let recorded = recorded_for(&format!("127.0.0.1:{port}"));
    assert_eq!(recorded.user, "postgres");
    assert_eq!(recorded.password.as_bytes(), admin.as_slice());
    let database = format!("app_{tag}");
    assert!(
        recorded
            .sql
            .contains(&format!("CREATE DATABASE {database} OWNER {database}")),
        "{recorded:?}"
    );
    let create_user = recorded.sql.iter().find(|sql| sql.starts_with("CREATE USER")).unwrap();
    let db_password = create_user
        .rsplit_once(" PASSWORD '")
        .unwrap()
        .1
        .trim_end_matches('\'')
        .to_string();
    // the Postgres image reads its password through POSTGRES_PASSWORD_FILE; the app gets its pairing token as a file
    let pg_body = &fleet
        .engine
        .containers()
        .values()
        .find(|c| c.name == format!("pg-{tag}"))
        .unwrap()
        .body
        .clone();
    assert!(pg_body.to_string().contains("POSTGRES_PASSWORD_FILE"), "{pg_body}");
    let token = files[&(format!("app-{tag}"), crate::kinds::PAIRING_TOKEN_FILE.to_string())].clone();
    let hash = moqui_vault::text::hex(&sha2::Sha256::digest(&token));
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &app, "pairingTokenHash"),
        hash
    );
    assert_eq!(files.len(), 2, "{:?}", files.keys());
    // no secret in any container's environment, labels or command, nor in any other request; none in a vault row
    let mut secrets: Vec<Vec<u8>> = files.values().cloned().collect();
    secrets.push(db_password.into_bytes());
    let vault_rows = all_text(&facade, ITEM);
    for secret in &secrets {
        assert!(
            fleet.engine.secret_outside_files(secret).is_empty(),
            "{:?}",
            fleet.engine.secret_outside_files(secret)
        );
        let text = String::from_utf8_lossy(secret).to_string();
        assert!(!vault_rows.iter().any(|row| row.contains(&text)));
        assert!(!all_text(&facade, "moqui.server.instance.InstanceAction")
            .iter()
            .any(|row| row.contains(&text)));
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

const ADMIN: &str = "a-database-host-admin-password";

/// An app on a database host, whose admin login is a vault item the provision must lease.
fn on_database_host(facade: &EntityFacade, fleet: &mut Fleet, tag: &str, name: &str) -> String {
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
            &json!({ "username": "dbadmin", "password": ADMIN }),
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

#[test]
fn a_provision_without_a_lease_or_machine_access_is_refused() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "lease", FakeEngine::docker());
    let app = on_database_host(&facade, &mut fleet, "lease", "app-lease");
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let binding = "DatabaseHost:dbhost-lease:admin".to_string();
    assert_eq!(
        crate::plan::build(&facade, &app).unwrap().lease_bindings,
        std::slice::from_ref(&binding)
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
        refused.message().contains("not \"provision AppInstance app-lease\""),
        "{refused}"
    );

    // the provision's own lease, not approved yet: refused; approved: it runs, and the lease ends with it
    let asked = registry
        .ask(
            &facade,
            &Ask {
                vault_id: &fleet.vault.vault_id,
                bindings: std::slice::from_ref(&binding),
                operation: &crate::plan::operation("provision", &app),
                limit: Some(Duration::from_secs(600)),
                environment_id: None,
            },
        )
        .unwrap();
    let waiting = crate::provision::provision(&facade, &registry, &app, Some(&asked.lease_id)).unwrap_err();
    assert!(waiting.message().contains("waiting for approval"), "{waiting}");
    fleet.vault.approve(&asked.lease_id);
    login(&fleet.vault.maker.user);
    crate::provision::provision(&facade, &registry, &app, Some(&asked.lease_id)).unwrap();
    let recorded = recorded_for("lease.db.test:5432");
    assert_eq!((recorded.user.as_str(), recorded.password.as_str()), ("dbadmin", ADMIN));
    assert!(fleet.engine.secret_outside_files(ADMIN.as_bytes()).is_empty());
    assert_eq!(
        field_of(
            &facade,
            "moqui.security.vault.VaultLease",
            "leaseId",
            &asked.lease_id,
            "statusId"
        ),
        "VlsEnded"
    );
    assert!(!registry.holds(&asked.lease_id));
    assert_eq!(
        deposited(&facade, &format!("AppInstance:{app}:dbPassword")),
        "VwDeposit"
    );

    // machine access: the environment holds the item, and its operations run under a lease approved by "machine"
    let second = on_database_host(&facade, &mut fleet, "lease", "app-machine");
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
    sys(|| {
        let mut row = facade
            .find("moqui.server.instance.InstanceEnvironment")
            .unwrap()
            .eq("environmentId", Value::string(&fleet.environment))
            .one()
            .unwrap()
            .unwrap();
        row.set("machineAccess", Value::string("Y"));
        row.update(&facade).unwrap();
    });
    let outcome = crate::provision::provision(&facade, &registry, &second, None).unwrap();
    let action = &outcome.action_id;
    let lease_id = field_of(
        &facade,
        "moqui.server.instance.InstanceAction",
        "instanceActionId",
        action,
        "leaseId",
    );
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
        field_of(
            &facade,
            "moqui.server.instance.InstanceAction",
            "instanceActionId",
            action,
            "machineAccess"
        ),
        "Y"
    );
}

#[test]
fn an_action_asked_from_a_screen_waits_for_its_lease_then_runs() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "request", FakeEngine::docker());
    let app = on_database_host(&facade, &mut fleet, "request", "app-request");
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
            "moqui.server.instance.InstanceAction",
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
