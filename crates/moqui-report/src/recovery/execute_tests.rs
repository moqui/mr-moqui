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

use super::property_allowed;

/// SEC-TLS-12: an adapter reads a `recovery_` property, or one the conf lists; it sends no secret (by its name) unless
/// listed, and never any other property the process holds.
#[test]
fn an_adapter_reads_only_recovery_properties_and_sends_no_conf_secret() {
    let ec = moqui_core::context::ExecutionContextFactory::test_context_with_conf("<moqui-conf/>");
    let _entered = ec.enter();
    for sent in [true, false] {
        assert!(property_allowed("entity_ds_crypt_pass", sent).is_err());
        assert!(property_allowed("OPS_HOOK_TOKEN", sent).is_err());
    }
    assert!(property_allowed("recovery_auth", true).is_ok());
    // a secret by its name is not sent; it may key the HMAC, which is not sent
    let refused = property_allowed("recovery_token", true).unwrap_err();
    assert!(refused.contains("telemetry/recovery@properties"), "{refused}");
    assert!(property_allowed("recovery_signing_key", true).is_err());
    assert!(property_allowed("recovery_signing_key", false).is_ok());
    // the conf's list names any other, secret or not
    moqui_lib::env::set_property(
        "moqui_conf__telemetry__recovery__properties",
        "recovery_token, OPS_HOOK_TOKEN",
    );
    let listed = (
        property_allowed("recovery_token", true),
        property_allowed("OPS_HOOK_TOKEN", true),
    );
    moqui_lib::env::clear_property("moqui_conf__telemetry__recovery__properties");
    assert!(listed.0.is_ok() && listed.1.is_ok(), "{listed:?}");
}

// ----- the instance adapter (plans/archive/Vault-Instances.md §4.8) -----

use std::collections::BTreeMap;
use std::sync::Mutex;

use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::json;

use super::{advance, attempt, cancel, revert, ACTION, ADAPTER, INSTANCE_ACTION};
use crate::regression::field;

/// What the fake manager was asked, each call's parameters.
static ASKED: Mutex<Vec<BTreeMap<String, Value>>> = Mutex::new(Vec::new());

/// A row's field as text.
fn text_of(row: &EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

/// The manager's `run#InstanceRecovery` as moqui-instance registers it, faked: it is given the recovery action's id
/// alone and reads the step from Monitor's rows, as the manager does; an automatic decision's step (no approver) is
/// refused (no machine access), and any other is recorded as an `InstanceAction` that waits for its lease, as a step
/// that opens vault items does. What it read is kept in [`ASKED`].
fn fake_instance_recovery(
    params: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let id = params.get("recoveryActionId").map(to_plain_string).unwrap_or_default();
    let action = facade.find(ACTION)?.eq("actionId", Value::string(&id)).one()?.unwrap();
    let decision = facade
        .find("moqui.monitor.RecoveryDecision")?
        .eq("decisionId", Value::string(text_of(&action, "decisionId")))
        .one()?
        .unwrap();
    let incident = facade
        .find(crate::incident::INCIDENT)?
        .eq("incidentId", Value::string(text_of(&action, "incidentId")))
        .one()?
        .unwrap();
    let asked: BTreeMap<String, Value> = [
        ("action", text_of(&action, "action")),
        ("target", text_of(&action, "target")),
        ("approvedBy", text_of(&decision, "approvedByUserId")),
        ("incidentInstanceId", text_of(&incident, "instanceId")),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), Value::String(value)))
    .collect();
    ASKED.lock().unwrap().push(asked);
    if text_of(&decision, "approvedByUserId").is_empty() {
        return Err(LibError::new(
            "An automatic restart has no approver (reasonCode machineAccessRequired)",
        ));
    }
    let action_id = format!("IA-{id}");
    let mut row = facade.make_value(INSTANCE_ACTION)?;
    row.set("instanceActionId", Value::string(&action_id));
    row.set("appInstanceId", Value::string(text_of(&incident, "instanceId")));
    row.set("statusEnumId", Value::string("IasWaiting"));
    row.set("leaseId", Value::string("LEASE-1"));
    row.create(facade)?;
    Ok([
        ("status", Value::string("waiting")),
        ("instanceActionId", Value::string(&action_id)),
        ("leaseId", Value::string("LEASE-1")),
        (
            "state",
            Value::string(format!("{} waits for lease LEASE-1", text_of(&action, "action"))),
        ),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value))
    .collect())
}

/// The manager's `cancel#InstanceRecovery`, faked: the instance action the recovery action names is marked failed.
fn fake_cancel_instance_recovery(
    params: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let id = params.get("recoveryActionId").map(to_plain_string).unwrap_or_default();
    let action = facade.find(ACTION)?.eq("actionId", Value::string(&id)).one()?.unwrap();
    let mut row = facade
        .find(INSTANCE_ACTION)?
        .eq("instanceActionId", Value::string(text_of(&action, "externalId")))
        .one()?
        .unwrap();
    row.set("statusEnumId", Value::string("IasFailed"));
    row.set(
        "report",
        Value::string(params.get("reason").map(to_plain_string).unwrap_or_default()),
    );
    row.update(facade)?;
    Ok(BTreeMap::from([("cancelled".to_string(), Value::Bool(true))]))
}

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["instances"],
        register_services: |map| {
            map.insert("moqui_instance::services::run_instance_recovery", fake_instance_recovery);
            map.insert("moqui_instance::services::cancel_instance_recovery", fake_cancel_instance_recovery);
            Ok(())
        },
        ..moqui_core::extension::Extension::EMPTY
    }
}

fn make(facade: &EntityFacade, entity: &str, fields: serde_json::Value) -> EntityValue {
    let mut row = facade.make_value(entity).unwrap();
    for (name, given) in fields.as_object().unwrap() {
        row.set(name.as_str(), moqui_lib::json::from_json(given));
    }
    row.create(facade).unwrap();
    row
}

fn set_status(facade: &EntityFacade, action_id: &str, status: &str, report: &str) {
    let mut row = facade
        .find(INSTANCE_ACTION)
        .unwrap()
        .eq("instanceActionId", Value::string(action_id))
        .one()
        .unwrap()
        .unwrap();
    row.set("statusEnumId", Value::string(status));
    row.set("report", Value::string(report));
    row.update(facade).unwrap();
}

/// The `instance` adapter's restart and rollback go to the manager's own actions by name (the deploy path), and in
/// approval mode wait for the lease those actions ask: the action is Running with the lease named until its
/// InstanceAction ends, then Succeeded or Failed as it did. A decision no person approved is an automatic policy's,
/// which the manager refuses without machine access.
#[test]
fn the_instance_adapter_takes_the_managers_actions_and_waits_for_their_lease() {
    let facade = moqui_core::testing::open();
    facade.assume_db_tables();
    let ctx = moqui_core::context::ExecutionContextFactory::test_context_in(
        "",
        moqui_core::service::ServicePools::idle(),
        moqui_core::context::framework_dir(),
    );
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let now = moqui_core::user::now_millis();
    make(
        &facade,
        ADAPTER,
        json!({"adapterId": "MANAGER", "adapterType": "instance"}),
    );
    let incident = make(
        &facade,
        crate::incident::INCIDENT,
        json!({"incidentId": "INC1", "instanceId": "app-1", "serviceName": "app-1", "statusId": "Open",
               "severity": "major", "title": "app-1 sealed"}),
    );
    let action = |id: &str, verb: &str, target: &str, approver: Option<&str>| {
        let mut decision = json!({"decisionId": format!("D-{id}"), "incidentId": "INC1", "action": verb, "target": target,
                                  "adapterId": "MANAGER", "statusId": "Approved"});
        if let Some(approver) = approver {
            decision["approvedByUserId"] = json!(approver);
        }
        make(&facade, "moqui.monitor.RecoveryDecision", decision);
        make(
            &facade,
            ACTION,
            json!({"actionId": id, "decisionId": format!("D-{id}"), "incidentId": "INC1", "action": verb,
                   "target": target, "adapterId": "MANAGER", "statusId": "Persisted", "attempts": 0}),
        )
    };
    let contact = json!({});

    // approval mode: the restart waits for its lease, then ends as the manager's action did
    let mut restart = action("A1", "restart", "", Some("OPS"));
    attempt(&ctx, &facade, &mut restart, &incident, &contact, None, now).unwrap();
    assert_eq!(
        field(&restart, "statusId"),
        "Running",
        "{}",
        field(&restart, "responseText")
    );
    assert_eq!(field(&restart, "externalId"), "IA-A1");
    assert_eq!(field(&restart, "vaultLeaseId"), "LEASE-1");
    {
        let asked = ASKED.lock().unwrap();
        let last = asked.last().unwrap();
        let text = |name: &str| last.get(name).map(to_plain_string).unwrap_or_default();
        assert_eq!(
            (text("action"), text("incidentInstanceId"), text("approvedBy")),
            ("restart".to_string(), "app-1".to_string(), "OPS".to_string())
        );
    }
    advance(&ctx, &facade, &mut restart, &incident, &contact, now + 1000).unwrap();
    assert_eq!(field(&restart, "statusId"), "Running");
    set_status(&facade, "IA-A1", "IasDone", "ran as action IA-9");
    advance(&ctx, &facade, &mut restart, &incident, &contact, now + 2000).unwrap();
    assert_eq!(field(&restart, "statusId"), "Succeeded");
    assert!(
        field(&restart, "resultingState").contains("IA-A1 done"),
        "{}",
        field(&restart, "resultingState")
    );
    // the manager's action is not reverted from Monitor
    assert!(revert(&ctx, &facade, &restart).is_err());

    // the rollback takes the same path, to the version the decision names, and fails as its action does
    let mut rollback = action("A2", "rollback", "1.2.3", Some("OPS"));
    attempt(&ctx, &facade, &mut rollback, &incident, &contact, None, now).unwrap();
    assert_eq!(field(&rollback, "statusId"), "Running");
    assert_eq!(
        ASKED.lock().unwrap().last().unwrap().get("target").map(to_plain_string),
        Some("1.2.3".to_string())
    );
    set_status(&facade, "IA-A2", "IasFailed", "the lease was not approved in its time");
    advance(&ctx, &facade, &mut rollback, &incident, &contact, now + 2000).unwrap();
    assert_eq!(field(&rollback, "statusId"), "Failed");

    // a cancelled action ends the manager's waiting action too, so it runs nothing later (and its lease ends there)
    let mut cancelled = action("A4", "restart", "", Some("OPS"));
    attempt(&ctx, &facade, &mut cancelled, &incident, &contact, None, now).unwrap();
    assert_eq!(field(&cancelled, "statusId"), "Running");
    cancel(&ctx, &facade, &mut cancelled, now + 1000).unwrap();
    assert_eq!(field(&cancelled, "statusId"), "Cancelled");
    let waiting = |id: &str| {
        facade
            .find(INSTANCE_ACTION)
            .unwrap()
            .eq("instanceActionId", Value::string(id))
            .one()
            .unwrap()
            .map(|row| text_of(&row, "statusEnumId"))
            .unwrap_or_default()
    };
    assert_eq!(waiting("IA-A4"), "IasFailed");
    // and so does one that timed out
    let mut late = action("A5", "restart", "", Some("OPS"));
    attempt(&ctx, &facade, &mut late, &incident, &contact, None, now).unwrap();
    late.set("deadlineDate", crate::regression::stamp(now + 1000));
    advance(&ctx, &facade, &mut late, &incident, &contact, now + 60_000).unwrap();
    assert_eq!(field(&late, "statusId"), "TimedOut");
    assert_eq!(waiting("IA-A5"), "IasFailed");

    // an automatic policy's decision: the manager refuses a step that needs a lease without machine access
    let mut automatic = action("A3", "restart", "", None);
    attempt(&ctx, &facade, &mut automatic, &incident, &contact, None, now).unwrap();
    assert_eq!(field(&automatic, "statusId"), "Failed");
    assert!(
        field(&automatic, "responseText").contains("machineAccessRequired"),
        "{}",
        field(&automatic, "responseText")
    );
}
