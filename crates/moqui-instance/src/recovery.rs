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

//! The manager's end of Monitor's `instance` recovery adapter (plans/archive/Vault-Instances.md §4.8): a recovery action on a
//! managed instance runs through the manager's own actions, not a webhook to an outside controller. Monitor calls
//! `run#InstanceRecovery` by name with its `RecoveryAction`'s id (moqui-report has no dependency on this crate), and
//! follows the `InstanceAction` it names; `cancel#InstanceRecovery` ends one that waits.
//!
//! What runs, and who it runs for, come from Monitor's rows, never from the call's parameters: the action and its
//! target from the `RecoveryAction`, which must be active, the approval from its `RecoveryDecision`, which must be
//! Approved and be of the same incident, action, target and adapter, and the instance from the step's target or the
//! incident's instance (exactly one managed instance; its id in telemetry is its own, since Monitor's receiver takes it
//! only under that instance's ingest key).
//!
//! - **Approved by a person** (approval mode, or an advisory proposal approved): that user must hold INSTANCE_ADMIN and
//!   be a member of the environment's approver group when it names one, as a person asking the action on the instance's
//!   page must.
//! - **Automatic** (no approver): the environment must allow automatic recovery (`automaticRecovery`, off by default)
//!   and have machine access, for every action: nobody approves a lease, and nobody chose the action.
//!
//! The actions:
//!
//! - `restart` and `rollback` are instance actions ([`crate::actions::request`]): run now when they open no vault item
//!   or the environment has machine access, else their lease is asked for and they wait for an approver, who opens it
//!   from the incident (Approve goes on to the lease dialog). A rollback is the deploy path's (M3).
//! - `isolate-instance` drains the instance over its manager link.
//! - `pause-rollout` stops the environment's rolling deploys at the next member ([`crate::monitor::rollout_paused`]),
//!   and `promote` lets them go on.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::{EntityFacade, EntityValue};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault_server::lease::LeaseRegistry;

use crate::actions;
use crate::common::{fail, field, one, APP_INSTANCE, ENVIRONMENT};
use crate::plan::{environment_of, instance};

const RECOVERY_ACTION: &str = "moqui.monitor.RecoveryAction";
const RECOVERY_DECISION: &str = "moqui.monitor.RecoveryDecision";
const INCIDENT: &str = "moqui.monitor.MonitorIncident";

/// What the manager did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    /// done, or waiting (for its lease).
    pub status: String,
    pub instance_action_id: Option<String>,
    pub lease_id: Option<String>,
    pub state: String,
}

/// Monitor's step as its rows say it.
#[derive(Debug, Clone)]
struct Step {
    id: String,
    action: String,
    target: String,
    /// The incident's instances (a regression's may be several).
    incident_instances: Vec<String>,
    /// Who approved the decision; empty for an automatic policy's.
    approved_by: String,
}

/// Reads a recovery action and its decision: refused when the action is not active, or the decision is not Approved or
/// not the one the action names.
fn step_of(facade: &EntityFacade, recovery_action_id: &str) -> Result<Step, LibError> {
    let row = one(facade, RECOVERY_ACTION, "actionId", recovery_action_id)?
        .ok_or_else(|| fail(format!("No recovery action {recovery_action_id}")))?;
    if !matches!(field(&row, "statusId").as_str(), "Persisted" | "Running") {
        return Err(fail(format!(
            "Recovery action {recovery_action_id} is {}, not active",
            field(&row, "statusId")
        )));
    }
    let decision_id = field(&row, "decisionId");
    let decision = one(facade, RECOVERY_DECISION, "decisionId", &decision_id)?
        .ok_or_else(|| fail(format!("Recovery action {recovery_action_id} names no decision")))?;
    if field(&decision, "statusId") != "Approved"
        || ["incidentId", "action", "target", "adapterId"]
            .iter()
            .any(|name| field(&decision, name) != field(&row, name))
    {
        return Err(fail(format!(
            "Recovery decision {decision_id} is not an approved decision of this action"
        )));
    }
    let incident_instances = one(facade, INCIDENT, "incidentId", &field(&row, "incidentId"))?
        .map(|incident| split(&field(&incident, "instanceId")))
        .unwrap_or_default();
    Ok(Step {
        id: recovery_action_id.to_string(),
        action: field(&row, "action"),
        target: field(&row, "target"),
        incident_instances,
        approved_by: field(&decision, "approvedByUserId"),
    })
}

/// The ids of a comma-separated list.
pub(crate) fn split(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// The one managed instance a step acts on: its target when that is an `AppInstance`, else the incident's instance;
/// refused when there is none, or several (a version-wide regression names them all: the step must name one).
fn instance_of(facade: &EntityFacade, step: &Step) -> Result<String, LibError> {
    if one(facade, APP_INSTANCE, "appInstanceId", &step.target)?.is_some() {
        return Ok(step.target.clone());
    }
    let mut managed = Vec::new();
    for id in &step.incident_instances {
        if one(facade, APP_INSTANCE, "appInstanceId", id)?.is_some() {
            managed.push(id.clone());
        }
    }
    match managed.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(fail(format!(
            "{} names no managed instance (target {:?}, the incident's instances {:?})",
            step.action, step.target, step.incident_instances
        ))),
        several => Err(fail(format!(
            "The incident names several managed instances ({}): the policy's step must name one as its target",
            several.join(", ")
        ))),
    }
}

/// The environment a step acts on: its target when it is one, else its instance's.
fn environment_for(facade: &EntityFacade, step: &Step) -> Result<EntityValue, LibError> {
    if let Some(environment) = one(facade, ENVIRONMENT, "environmentId", &step.target)? {
        return Ok(environment);
    }
    let instance_id = instance_of(facade, step)?;
    environment_of(facade, &instance(facade, &instance_id)?)
}

/// Whether the step may run in `environment`: its approver's rights, or for an automatic policy's the environment's
/// automatic recovery and machine access (see the module doc).
fn authorize(environment: &EntityValue, step: &Step) -> Result<(), LibError> {
    if !step.approved_by.is_empty() {
        return actions::check_user(environment, &step.approved_by);
    }
    let environment_id = field(environment, "environmentId");
    if field(environment, "automaticRecovery") != "Y" {
        return Err(fail(format!(
            "An automatic {} has no approver, and environment {environment_id} does not allow automatic recovery: \
             approve the decision instead (reasonCode automaticRecoveryOff)",
            step.action
        )));
    }
    if field(environment, "machineAccess") != "Y" {
        return Err(fail(format!(
            "An automatic {} has no approver, and runs only under environment {environment_id}'s machine access: \
             approve the decision instead (reasonCode machineAccessRequired)",
            step.action
        )));
    }
    Ok(())
}

/// Runs Monitor's recovery action `recovery_action_id` through the manager's actions (see the module doc).
pub fn run(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    registry: &Arc<LeaseRegistry>,
    recovery_action_id: &str,
) -> Result<Answer, LibError> {
    let step = step_of(facade, recovery_action_id)?;
    let by = format!("recovery action {}", step.id);
    match step.action.as_str() {
        "restart" | "rollback" => {
            let verb = step.action.as_str();
            if !actions::ACTIONS.iter().any(|(_, known)| *known == verb) {
                return Err(fail(format!(
                    "{verb} of a managed instance is not built in this server (the deploy path, M3)"
                )));
            }
            let instance_id = instance_of(facade, &step)?;
            authorize(&environment_of(facade, &instance(facade, &instance_id)?)?, &step)?;
            let requested = actions::request(factory, facade, registry, &instance_id, verb, false)?;
            let waiting = requested.status == actions::WAITING;
            Ok(Answer {
                status: if waiting { "waiting" } else { "done" }.to_string(),
                state: if waiting {
                    format!(
                        "{verb} of AppInstance {instance_id} waits for lease {} (action {})",
                        requested.lease_id.clone().unwrap_or_default(),
                        requested.action_id
                    )
                } else {
                    format!(
                        "{verb} of AppInstance {instance_id} done (action {})",
                        requested.action_id
                    )
                },
                instance_action_id: Some(requested.action_id),
                lease_id: requested.lease_id,
            })
        }
        "isolate-instance" => {
            let instance_id = instance_of(facade, &step)?;
            authorize(&environment_of(facade, &instance(facade, &instance_id)?)?, &step)?;
            let action_id = actions::record(facade, &instance_id, "drain", actions::RUNNING, None, false, &by)?;
            // a drain is not one of the actions a screen asks for, so its row is named here
            if let Some(mut row) = one(facade, crate::common::ACTION, "instanceActionId", &action_id)? {
                row.set("actionEnumId", Value::string("IatDrain"));
                row.update(facade)?;
            }
            let parameters: BTreeMap<String, Value> = [
                ("appInstanceId".to_string(), Value::string(&instance_id)),
                ("command".to_string(), Value::string("drain")),
            ]
            .into_iter()
            .collect();
            match call("org.moqui.dev.InstanceLinkServices.call#InstanceCommand", &parameters) {
                Ok(_) => {
                    actions::finish(
                        facade,
                        &action_id,
                        actions::DONE,
                        &format!("drained over its link ({by})"),
                    )?;
                    Ok(Answer {
                        status: "done".to_string(),
                        instance_action_id: Some(action_id),
                        lease_id: None,
                        state: format!("AppInstance {instance_id} drained"),
                    })
                }
                Err(err) => {
                    actions::finish(facade, &action_id, actions::FAILED, err.message())?;
                    Err(err)
                }
            }
        }
        "pause-rollout" | "promote" => {
            let mut row = environment_for(facade, &step)?;
            authorize(&row, &step)?;
            let environment_id = field(&row, "environmentId");
            let pause = step.action == "pause-rollout";
            if pause {
                row.set("rolloutPausedDate", moqui_core::user::now_value());
                row.set("rolloutPausedReason", Value::string(&by));
            } else {
                row.set("rolloutPausedDate", Value::Null);
                row.set("rolloutPausedReason", Value::Null);
            }
            row.update(facade)?;
            Ok(Answer {
                status: "done".to_string(),
                instance_action_id: None,
                lease_id: None,
                state: format!(
                    "rolling deploys of environment {environment_id} {}",
                    if pause { "paused" } else { "go on" }
                ),
            })
        }
        other => Err(fail(format!(
            "{other} is not a recovery action of a managed instance: restart, rollback, isolate-instance, \
             pause-rollout or promote"
        ))),
    }
}

/// Monitor cancelled or timed out its recovery action: the instance action it waits on is marked failed and its lease
/// ended, so nothing runs ([`crate::actions::cancel_waiting`]). True when one was waiting.
pub fn cancel(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    recovery_action_id: &str,
    why: &str,
) -> Result<bool, LibError> {
    let row = one(facade, RECOVERY_ACTION, "actionId", recovery_action_id)?
        .ok_or_else(|| fail(format!("No recovery action {recovery_action_id}")))?;
    let action_id = field(&row, "externalId");
    if action_id.is_empty() {
        return Ok(false);
    }
    actions::cancel_waiting(
        facade,
        registry,
        &action_id,
        &format!("{why} (recovery action {recovery_action_id})"),
    )
}

/// Calls a service by name; its message errors are taken and returned as an error.
fn call(name: &str, parameters: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, LibError> {
    let answer = moqui_core::service::sync(name, parameters);
    let errors = ExecutionContext::with_current(|ec| {
        let text = ec.message().errors().join("; ");
        ec.message().clear_errors();
        text
    })
    .unwrap_or_default();
    match answer {
        Err(err) => Err(fail(format!("{name}: {} {errors}", err.message()))),
        Ok(_) if !errors.is_empty() => Err(fail(format!("{name}: {errors}"))),
        Ok(out) => Ok(out),
    }
}
