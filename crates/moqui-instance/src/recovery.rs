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

//! The manager's end of Monitor's `instance` recovery adapter (plans/Vault-Instances.md §4.8): a recovery action on a
//! managed instance runs through the manager's own actions, not a webhook to an outside controller. Monitor calls
//! `run#InstanceRecovery` by name (moqui-report has no dependency on this crate) and follows the `InstanceAction` it
//! names.
//!
//! - `restart` and `rollback` are instance actions ([`crate::actions::request`]), as a screen asks them: run now when
//!   they open no vault item or the environment has machine access, else their lease is asked for and they wait for an
//!   approver, who opens it in the lease dialog (from the incident, beside Approve). A rollback is the deploy path's
//!   (M3): to the versions recorded on the instance's actions.
//! - `isolate-instance` drains the instance over its manager link, so its health answers draining and its load
//!   balancer or edge node stops sending it requests.
//! - `pause-rollout` stops the environment's rolling deploys at the next member ([`crate::monitor::rollout_paused`]),
//!   and `promote` lets them go on.
//!
//! An automatic policy's action has no person to approve a lease: a step that needs one runs only under the
//! environment's machine access, and is refused otherwise (`reasonCode machineAccessRequired`).

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault_server::lease::LeaseRegistry;

use crate::actions;
use crate::common::{fail, field, one, APP_INSTANCE, ENVIRONMENT};
use crate::plan::{environment_of, instance};

/// What Monitor asks.
#[derive(Debug, Clone, Default)]
pub struct Asked {
    /// restart, rollback, isolate-instance, pause-rollout or promote.
    pub action: String,
    /// The step's target: an `AppInstance` id, or for pause-rollout and promote an environment's.
    pub target: String,
    /// The incident's instance (its `service.instance.id`, the `AppInstance` id this manager gave it).
    pub incident_instance: String,
    /// The decision is an automatic policy's, with no person to approve a lease.
    pub automatic: bool,
    /// Monitor's `RecoveryAction`, named in the report.
    pub recovery_action_id: String,
}

/// What the manager did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    /// done, or waiting (for its lease).
    pub status: String,
    pub instance_action_id: Option<String>,
    pub lease_id: Option<String>,
    pub state: String,
}

/// The `AppInstance` a step acts on: its target, else the incident's instance.
fn instance_of(facade: &EntityFacade, asked: &Asked) -> Result<String, LibError> {
    for id in [&asked.target, &asked.incident_instance] {
        if one(facade, APP_INSTANCE, "appInstanceId", id)?.is_some() {
            return Ok(id.clone());
        }
    }
    Err(fail(format!(
        "{} names no managed instance (target {:?}, the incident's instance {:?})",
        asked.action, asked.target, asked.incident_instance
    )))
}

/// The environment a step acts on: its target when it is one, else the instance's.
fn environment_for(facade: &EntityFacade, asked: &Asked) -> Result<String, LibError> {
    if one(facade, ENVIRONMENT, "environmentId", &asked.target)?.is_some() {
        return Ok(asked.target.clone());
    }
    let instance_id = instance_of(facade, asked)?;
    Ok(field(&instance(facade, &instance_id)?, "environmentId"))
}

/// Runs Monitor's recovery step through the manager's actions (see the module doc).
pub fn run(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    registry: &Arc<LeaseRegistry>,
    asked: &Asked,
) -> Result<Answer, LibError> {
    let by = format!("recovery action {}", asked.recovery_action_id);
    match asked.action.as_str() {
        "restart" | "rollback" => {
            let verb = asked.action.as_str();
            if !actions::ACTIONS.iter().any(|(_, known)| *known == verb) {
                return Err(fail(format!(
                    "{verb} of a managed instance is not built in this server (the deploy path, M3)"
                )));
            }
            let instance_id = instance_of(facade, asked)?;
            let environment = environment_of(facade, &instance(facade, &instance_id)?)?;
            if asked.automatic && field(&environment, "machineAccess") != "Y" {
                let (bindings, _) = actions::needs(facade, &instance_id, verb)?;
                if !bindings.is_empty() {
                    return Err(fail(format!(
                        "An automatic {verb} of AppInstance {instance_id} opens vault items ({}) and has no approver: \
                         it runs only under environment {}'s machine access; approve the decision instead (reasonCode \
                         machineAccessRequired)",
                        bindings.join(", "),
                        field(&environment, "environmentId")
                    )));
                }
            }
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
            let instance_id = instance_of(facade, asked)?;
            let environment = environment_of(facade, &instance(facade, &instance_id)?)?;
            actions::check_caller(&environment)?;
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
            let drained = call("org.moqui.dev.InstanceLinkServices.call#InstanceCommand", &parameters);
            match drained {
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
            let environment_id = environment_for(facade, asked)?;
            let mut row = one(facade, ENVIRONMENT, "environmentId", &environment_id)?
                .ok_or_else(|| fail(format!("No InstanceEnvironment {environment_id}")))?;
            actions::check_caller(&row)?;
            let pause = asked.action == "pause-rollout";
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
