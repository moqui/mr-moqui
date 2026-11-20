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

//! M4's Tests column (plans/Vault-Instances.md §6), against the fake engine: a provision sets the instance id and an
//! ingest key; the recovery adapter's restart takes the instance action path and waits for a lease in approval mode
//! (and needs machine access when automatic); a sealed instance raises its alert. And what the alert rules and the fleet
//! read: an unseal from an address the instance never used, the rollout pause.

use std::time::{Duration, Instant};

use moqui_core::entity::EntityFacade;
use moqui_lib::docker::fake::FakeEngine;
use moqui_lib::object::Value;
use moqui_vault_server::lease;
use serde_json::{json, Value as Json};

use super::provision::{fleet, pg_and_app, set_row};
use super::{create, field_of, login, setup, sys};
use crate::services::NewInstance;

const INSTANCE: &str = "moqui.server.instance.AppInstance";
const ACTION: &str = "moqui.server.instance.InstanceAction";
const INGEST_KEY: &str = "moqui.monitor.IngestKey";
const SCRAPE_TARGET: &str = "moqui.monitor.ScrapeTarget";

/// The maker of `tag`'s vault may also own an ingest key and let a scrape target reach a private address.
fn grant_monitor(facade: &EntityFacade, tag: &str) {
    for permission in ["MONITOR_INGEST", "MONITOR_PRIVATE_NETWORK"] {
        create(
            facade,
            "moqui.security.UserGroupPermission",
            json!({ "userGroupId": format!("IT_{tag}"), "userPermissionId": permission, "fromDate": 0 }),
        );
    }
}

/// A shipped instance alert rule (tools/report/instance-*.json).
fn shipped_rule(id: &str) -> Json {
    let path = moqui_core::context::framework_dir()
        .parent()
        .unwrap()
        .join(format!("tools/report/{id}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Sets a date-time field of an instance's row to `millis`.
fn set_time(facade: &EntityFacade, instance_id: &str, name: &str, millis: i64) {
    sys(|| {
        let mut row = crate::plan::instance(facade, instance_id).unwrap();
        row.set(name, moqui_core::user::stamp_value(millis));
        row.update(facade).unwrap();
    });
}

#[test]
fn a_provision_sets_the_instance_id_and_an_ingest_key() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "mon", FakeEngine::docker());
    grant_monitor(&facade, "mon");
    moqui_lib::env::set_property("moqui_conf__manage__monitor_url", "https://manager.test:8443/");
    let (pg, app) = pg_and_app(&facade, &fleet, "mon", "MoquiLocal");
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    let outcome = crate::provision::provision(&facade, &registry, &app, None).unwrap();
    moqui_lib::env::clear_property("moqui_conf__manage__monitor_url");

    // its identity: its id is its service.instance.id, its environment, the manager's Monitor; settings, not secrets
    let body = fleet.body_of("app-mon");
    let env: Vec<String> = body["Env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap_or_default().to_string())
        .collect();
    for setting in [
        format!("moqui_conf__instance__id={app}"),
        format!("moqui_conf__telemetry__deployment_environment={}", fleet.environment),
        format!("moqui_conf__telemetry__service_name=moqui-{}", fleet.environment),
        "moqui_conf__telemetry__otlp_endpoint=https://manager.test:8443".to_string(),
        "moqui_conf__telemetry__otlp_headers_file=/run/secrets/moqui-ingest-key".to_string(),
    ] {
        assert!(env.contains(&setting), "{setting} not in {env:?}");
    }
    // its ingest key, in Monitor's format, is a file and nowhere else, and is registered as its source's key
    let files = fleet.secret_files();
    let key = String::from_utf8(files[&("app-mon".to_string(), "moqui-ingest-key".to_string())].clone()).unwrap();
    assert!(key.starts_with("mqi_") && key.len() == 47, "{key}");
    assert!(fleet.engine.secret_outside_files(key.as_bytes()).is_empty());
    let key_id = crate::monitor::ingest_key_id(&key);
    assert_eq!(
        field_of(&facade, INGEST_KEY, "keyId", &key_id, "source"),
        format!("instance:{app}")
    );
    assert_eq!(field_of(&facade, INGEST_KEY, "keyId", &key_id, "revoked"), "N");
    assert_eq!(
        field_of(&facade, INGEST_KEY, "keyId", &key_id, "userId"),
        fleet.vault.maker.user
    );
    assert!(
        outcome
            .report
            .iter()
            .any(|line| line.contains("registered the ingest key")),
        "{:?}",
        outcome.report
    );
    // the database's exporter is a scrape target whose metrics carry the instance's id
    let target = crate::monitor::scrape_target_id(&pg);
    assert_eq!(
        field_of(&facade, SCRAPE_TARGET, "targetId", &target, "url"),
        "http://pg-mon-exporter:9187/metrics"
    );
    assert_eq!(
        field_of(&facade, SCRAPE_TARGET, "targetId", &target, "serviceInstanceId"),
        pg
    );
    assert_eq!(
        field_of(&facade, SCRAPE_TARGET, "targetId", &target, "allowPrivateNetwork"),
        "Y"
    );
    // removing an instance takes it out of Monitor
    sys(|| crate::lifecycle::act(&facade, &registry, &app, "remove", None, true)).unwrap();
    assert_eq!(field_of(&facade, INGEST_KEY, "keyId", &key_id, "revoked"), "Y");
    sys(|| crate::lifecycle::act(&facade, &registry, &pg, "remove", None, true)).unwrap();
    assert!(field_of(&facade, SCRAPE_TARGET, "targetId", &target, "url").is_empty());
}

#[test]
fn the_recovery_restart_waits_for_its_lease_in_approval_mode_and_needs_machine_access_when_automatic() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let mut fleet = fleet(&facade, "rec", FakeEngine::docker());
    let pg = fleet.instance(
        &facade,
        NewInstance {
            instance_image_id: "Postgres18".to_string(),
            instance_name: "pg-rec".to_string(),
            ..NewInstance::default()
        },
    );
    login(&fleet.vault.maker.user);
    let registry = lease::registry(ctx.factory());
    crate::provision::provision(&facade, &registry, &pg, None).unwrap();
    let container = field_of(&facade, INSTANCE, "appInstanceId", &pg, "instanceUuid");
    // the host is reached with a client key from the vault, so a restart opens it under a lease
    let binding = crate::common::host_key_binding(&fleet.host);
    fleet
        .vault
        .item("tls-key-pair", &binding, &json!({ "privateKey": "a-host-client-key" }));
    set_row(
        &facade,
        "moqui.server.instance.InstanceHost",
        "instanceHostId",
        &fleet.host,
        &[("hostProtocol", "https"), ("tlsClientCert", "a-client-certificate")],
    );
    let asked = |automatic: bool| crate::recovery::Asked {
        action: "restart".to_string(),
        incident_instance: pg.clone(),
        automatic,
        recovery_action_id: "RA1".to_string(),
        ..crate::recovery::Asked::default()
    };
    let factory = std::sync::Arc::clone(ctx.factory());

    // automatic, no machine access: refused, and no lease is asked
    let leases = || {
        sys(|| {
            facade
                .find("moqui.security.vault.VaultLease")
                .unwrap()
                .list()
                .unwrap()
                .len()
        })
    };
    let before = leases();
    let refused = sys(|| crate::recovery::run(&factory, &facade, &registry, &asked(true))).unwrap_err();
    assert!(refused.message().contains("machineAccessRequired"), "{refused}");
    assert_eq!(leases(), before);

    // approval mode: the restart is an instance action that waits for its lease
    let answer = sys(|| crate::recovery::run(&factory, &facade, &registry, &asked(false))).unwrap();
    assert_eq!(answer.status, "waiting", "{answer:?}");
    let action_id = answer.instance_action_id.clone().unwrap();
    let lease_id = answer.lease_id.clone().unwrap();
    assert_eq!(
        field_of(&facade, ACTION, "instanceActionId", &action_id, "actionEnumId"),
        "IatRestart"
    );
    assert_eq!(
        field_of(&facade, ACTION, "instanceActionId", &action_id, "statusEnumId"),
        crate::actions::WAITING
    );
    assert!(!fleet
        .engine
        .calls()
        .iter()
        .any(|call| call.contains(&format!("/containers/{container}/stop"))));
    // the fake engine speaks no TLS: the host is reached plainly again before the approver opens the lease
    set_row(
        &facade,
        "moqui.server.instance.InstanceHost",
        "instanceHostId",
        &fleet.host,
        &[("hostProtocol", "http"), ("tlsClientCert", "")],
    );
    fleet.vault.approve(&lease_id);
    let until = Instant::now() + Duration::from_secs(10);
    let status = loop {
        let status = field_of(&facade, ACTION, "instanceActionId", &action_id, "statusEnumId");
        if status != crate::actions::WAITING || Instant::now() > until {
            break status;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(status, crate::actions::DONE);
    assert!(
        fleet
            .engine
            .calls()
            .iter()
            .any(|call| call.contains(&format!("/containers/{container}/stop"))),
        "{:?}",
        fleet.engine.calls()
    );
}

#[test]
fn pause_rollout_and_promote_stop_and_let_go_on_the_environments_rolling_deploys() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "roll", FakeEngine::docker());
    let factory = std::sync::Arc::clone(ctx.factory());
    let registry = lease::registry(ctx.factory());
    let asked = |action: &str| crate::recovery::Asked {
        action: action.to_string(),
        target: fleet.environment.clone(),
        ..crate::recovery::Asked::default()
    };
    sys(|| crate::recovery::run(&factory, &facade, &registry, &asked("pause-rollout"))).unwrap();
    assert!(sys(|| crate::monitor::rollout_paused(&facade, &fleet.environment)).unwrap());
    sys(|| crate::recovery::run(&factory, &facade, &registry, &asked("promote"))).unwrap();
    assert!(!sys(|| crate::monitor::rollout_paused(&facade, &fleet.environment)).unwrap());
    // a step this adapter does not carry out is refused
    assert!(sys(|| crate::recovery::run(&factory, &facade, &registry, &asked("pause-job"))).is_err());
}

#[test]
fn a_sealed_instance_raises_its_alert_and_an_unseal_from_a_new_address_is_told_apart() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let fleet = fleet(&facade, "seal", FakeEngine::docker());
    let made = |name: &str| {
        let id = fleet.instance(
            &facade,
            NewInstance {
                instance_image_id: "MoquiLocal".to_string(),
                instance_name: name.to_string(),
                ..NewInstance::default()
            },
        );
        set_row(
            &facade,
            INSTANCE,
            "appInstanceId",
            &id,
            &[("instanceUuid", "c0ffee"), ("observedState", "running")],
        );
        set_time(&facade, &id, "linkPairedDate", moqui_core::user::now_millis());
        id
    };
    let (sealed, open) = (made("app-sealed"), made("app-open"));
    crate::link::note_state(&facade, &sealed, "linked", Some("sealed"), "10.0.0.1");
    crate::link::note_state(&facade, &open, "linked", Some("unsealed"), "10.0.0.9");
    // sealed for 20 minutes, past the shipped rule's 15
    let now = moqui_core::user::now_millis();
    set_time(&facade, &sealed, "sealedSince", now - 20 * 60_000);
    sys(|| crate::monitor::refresh(&facade));

    let rule = shipped_rule("instance-sealed");
    let metric = "moqui.instance.sealed_seconds";
    assert_eq!(
        rule["body"]["query"]["queryString"],
        json!(format!("name:\"{metric}\""))
    );
    assert_eq!(
        rule["body"]["query"]["aggregations"][0]["params"]["field"],
        json!("moqui.instance.id")
    );
    assert_eq!(rule["body"]["condition"]["type"], json!("above"));
    let threshold = rule["body"]["condition"]["threshold"].as_f64().unwrap();
    // the gauge keeps the attribute the rule splits by (the conf's metric allow-list)
    let conf = std::fs::read_to_string(moqui_core::context::framework_dir().join("conf/MoquiDefaultConf.xml")).unwrap();
    assert!(
        conf.contains(&format!("<metric name=\"{metric}\" attributes=\"moqui.instance.id,")),
        "the conf lists no attributes of {metric}"
    );
    let value_of = |id: &str| {
        crate::monitor::series(metric, now)
            .into_iter()
            .find(|(attributes, _)| attributes.get("moqui.instance.id") == Some(&Value::string(id)))
            .map(|(_, value)| value)
    };
    assert!(value_of(&sealed).unwrap() > threshold, "{:?}", value_of(&sealed));
    assert!(value_of(&open).unwrap() <= threshold);
    let row = crate::fleet::fleet(&facade, now)
        .unwrap()
        .into_iter()
        .find(|row| row.get("appInstanceId") == Some(&Value::string(&sealed)))
        .unwrap();
    assert_eq!(row.get("sealState"), Some(&Value::string("sealed")));

    // its unseal clears it, and keeps where it came from: the first address is no new one
    crate::link::note_state(&facade, &sealed, "linked", Some("unsealed"), "");
    sys(|| crate::monitor::refresh(&facade));
    assert_eq!(value_of(&sealed), Some(0.0));
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &sealed, "unsealAddresses"),
        "10.0.0.1"
    );
    let mut row = sys(|| crate::plan::instance(&facade, &sealed)).unwrap();
    row.set("sealState", Value::string("sealed"));
    let mut again = row.clone();
    assert_eq!(
        crate::monitor::note_seal(&mut again, "unsealed", "10.0.0.1"),
        Some(false)
    );
    assert_eq!(crate::monitor::note_seal(&mut row, "unsealed", "10.0.0.2"), Some(true));
    assert_eq!(crate::common::field(&row, "unsealAddresses"), "10.0.0.1,10.0.0.2");
    // the shipped rule of that tells the new address apart
    let rule = shipped_rule("instance-unseal-new-address");
    assert_eq!(
        rule["body"]["query"]["filters"][0]["field"]["name"],
        json!("moqui.instance.new_address")
    );
}
