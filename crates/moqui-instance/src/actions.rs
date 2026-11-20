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

//! Instance actions (plans/Vault-Instances.md §4.1, §4.3 step 3): each change to an instance is an `InstanceAction`
//! row with its state, the lease it ran under (or the environment's machine access), who did it, the versions before
//! and after and a report that names what was made, never a value.
//!
//! What lets an action open its items ([`open_authority`]): nothing when it needs none; the environment's machine
//! access, a lease approved by "machine" for the environment the instance's own row names (never one a request
//! names); or a lease handed to it, which this cluster member holds, whose signed statement names this action on this
//! instance and the environment's vault, and which is open. An action that needs items and has none of these is
//! refused, and the refusal is an action row too.
//!
//! From a screen ([`request`]): an action that can run now runs; one that needs a lease asks for it, records the action
//! as waiting with the lease (the page links to the lease dialog), and a thread on this member runs the action once
//! an approver opens the lease, or marks it failed when the lease is not approved in its time limit.

use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use moqui_vault_server::lease::{Ask, LeaseRegistry};

use crate::common::{fail, field, now, one, ACTION};
use crate::plan::{environment_of, instance, lease_limit, operation};

pub const WAITING: &str = "IasWaiting";
pub const RUNNING: &str = "IasRunning";
pub const DONE: &str = "IasDone";
pub const FAILED: &str = "IasFailed";
pub const REFUSED: &str = "IasRefused";

/// The actions an operator asks for, by their enumeration id and the verb a lease's operation names.
pub const ACTIONS: &[(&str, &str)] = &[
    ("IatProvision", "provision"),
    ("IatStart", "start"),
    ("IatStop", "stop"),
    ("IatRestart", "restart"),
    ("IatRemove", "remove"),
];

/// The verb of an action's enumeration id.
pub fn verb(action_enum: &str) -> Result<&'static str, LibError> {
    ACTIONS
        .iter()
        .find(|(id, _)| *id == action_enum)
        .map(|(_, verb)| *verb)
        .ok_or_else(|| {
            fail(format!(
                "{action_enum} is not an instance action: provision, start, stop, restart or remove"
            ))
        })
}

fn action_enum(verb: &str) -> &'static str {
    ACTIONS
        .iter()
        .find(|(_, name)| *name == verb)
        .map_or("IatProvision", |(id, _)| *id)
}

/// Writes an action row; its id.
pub fn record(
    facade: &EntityFacade,
    instance_id: &str,
    verb: &str,
    status: &str,
    lease_id: Option<&str>,
    machine: bool,
    report: &str,
) -> Result<String, LibError> {
    let mut row = facade.make_value(ACTION)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("appInstanceId", Value::string(instance_id));
    row.set("actionEnumId", Value::string(action_enum(verb)));
    row.set("statusEnumId", Value::string(status));
    if let Some(lease_id) = lease_id {
        row.set("leaseId", Value::string(lease_id));
    }
    row.set("machineAccess", Value::string(if machine { "Y" } else { "N" }));
    if let Ok(Some(user)) = moqui_core::user::current_user_id() {
        row.set("userId", Value::string(user));
    }
    if let Ok(Some(instance)) = one(facade, crate::common::APP_INSTANCE, "appInstanceId", instance_id) {
        row.set("versionBefore", Value::string(field(&instance, "imageDigest")));
    }
    if !report.is_empty() {
        row.set("report", Value::string(report));
    }
    row.set("startDate", now());
    if status != WAITING && status != RUNNING {
        row.set("endDate", now());
    }
    let id = field(&row, "instanceActionId");
    row.create(facade)?;
    Ok(id)
}

/// Changes an action's state, with its report and the instance's version after it.
pub fn finish(facade: &EntityFacade, action_id: &str, status: &str, report: &str) -> Result<(), LibError> {
    let Some(mut row) = one(facade, ACTION, "instanceActionId", action_id)? else {
        return Err(fail(format!("No InstanceAction {action_id}")));
    };
    row.set("statusEnumId", Value::string(status));
    if !report.is_empty() {
        row.set("report", Value::string(report));
    }
    if let Ok(Some(instance)) = one(
        facade,
        crate::common::APP_INSTANCE,
        "appInstanceId",
        &field(&row, "appInstanceId"),
    ) {
        row.set("versionAfter", Value::string(field(&instance, "imageDigest")));
    }
    if status != WAITING && status != RUNNING {
        row.set("endDate", now());
    }
    row.update(facade)?;
    Ok(())
}

/// What lets an action open its items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opened {
    /// The lease it runs under, none when it needs no item.
    pub lease_id: Option<String>,
    /// The lease was opened from the environment's machine access.
    pub machine: bool,
}

/// What an action needs to open `bindings` of `vault_id` for `operation` (see the module doc). A refusal names what
/// to do, never a value.
#[allow(clippy::too_many_arguments)]
pub fn open_authority(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    environment_id: &str,
    vault_id: &str,
    machine_access: bool,
    bindings: &[String],
    operation: &str,
    lease_id: Option<&str>,
) -> Result<Opened, LibError> {
    if bindings.is_empty() {
        return Ok(Opened::default());
    }
    if let Some(lease_id) = lease_id.filter(|id| !id.is_empty()) {
        let Some((named, vault)) = registry.statement_of(lease_id) else {
            return Err(fail(format!(
                "The lease {lease_id} is not held by this server, or it ended: ask again (reasonCode leaseEnded)"
            )));
        };
        if named != operation || vault != vault_id {
            return Err(fail(format!(
                "The lease {lease_id} is for {named:?} in vault {vault}, not {operation:?} in vault {vault_id}"
            )));
        }
        registry.wait_open(facade, lease_id, Duration::ZERO).map_err(|_| {
            fail(format!(
                "The lease {lease_id} is waiting for approval: an approver opens it in the lease dialog"
            ))
        })?;
        return Ok(Opened {
            lease_id: Some(lease_id.to_string()),
            machine: false,
        });
    }
    if machine_access {
        let lease = moqui_vault_server::machine::machine_lease(
            registry,
            facade,
            environment_id,
            vault_id,
            bindings,
            operation,
        )?;
        return Ok(Opened {
            lease_id: Some(lease.lease_id),
            machine: true,
        });
    }
    Err(fail(format!(
        "{operation} opens vault items ({}) and runs only under a lease of them that an approver opened, or under its \
         environment's machine access: ask for the lease (request#InstanceAction) and approve it (reasonCode leaseRequired)",
        bindings.join(", ")
    )))
}

/// Ends the lease an action ran under, at the action's end (§3.7, step 4).
pub fn end_lease(facade: &EntityFacade, registry: &LeaseRegistry, opened: &Opened) {
    if let Some(lease_id) = &opened.lease_id {
        if let Err(err) = registry.end(facade, lease_id) {
            registry.forget(lease_id);
            tracing::error!("Instances: the lease {lease_id} was not marked ended: {err}");
        }
    }
}

/// The vault items an action needs, by binding: a provision's plan, else the host's client key.
pub fn bindings_of(facade: &EntityFacade, instance_id: &str, verb: &str) -> Result<Vec<String>, LibError> {
    if verb == "provision" {
        return Ok(crate::plan::build(facade, instance_id)?.lease_bindings);
    }
    crate::lifecycle::host_bindings(facade, instance_id)
}

/// Runs an action now: a provision, or start, stop, restart or remove through the host type's services.
pub fn run(
    facade: &EntityFacade,
    registry: &LeaseRegistry,
    instance_id: &str,
    verb: &str,
    lease_id: Option<&str>,
    remove_volumes: bool,
) -> Result<crate::provision::Outcome, LibError> {
    match verb {
        "provision" => crate::provision::provision(facade, registry, instance_id, lease_id),
        _ => crate::lifecycle::act(facade, registry, instance_id, verb, lease_id, remove_volumes),
    }
}

/// What [`request`] did.
#[derive(Debug, Clone, Default)]
pub struct Requested {
    pub action_id: String,
    pub status: String,
    /// The lease to approve, when the action waits for one.
    pub lease_id: Option<String>,
}

/// `request#InstanceAction`: runs the action now when it needs no item or the environment has machine access; else asks
/// for its lease, records the action as waiting, and runs it on this member once the lease opens.
pub fn request(
    factory: &Arc<ExecutionContextFactory>,
    facade: &EntityFacade,
    registry: &Arc<LeaseRegistry>,
    instance_id: &str,
    verb: &str,
    remove_volumes: bool,
) -> Result<Requested, LibError> {
    let row = instance(facade, instance_id)?;
    let environment = environment_of(facade, &row)?;
    let bindings = bindings_of(facade, instance_id, verb)?;
    if bindings.is_empty() || field(&environment, "machineAccess") == "Y" {
        let outcome = run(facade, registry, instance_id, verb, None, remove_volumes)?;
        return Ok(Requested {
            action_id: outcome.action_id,
            status: DONE.to_string(),
            lease_id: None,
        });
    }
    let operation = operation(verb, instance_id);
    let vault_id = field(&environment, "vaultId");
    let lease = registry.ask(
        facade,
        &Ask {
            vault_id: &vault_id,
            bindings: &bindings,
            operation: &operation,
            limit: lease_limit(&environment),
            environment_id: None,
        },
    )?;
    let lease_id = lease.lease_id.clone();
    let action_id = record(
        facade,
        instance_id,
        verb,
        WAITING,
        Some(&lease_id),
        false,
        &format!("waiting for an approver to open lease {lease_id}"),
    )?;
    let limit = Duration::from_millis(u64::try_from(lease.expires_at - moqui_core::user::now_millis()).unwrap_or(0));
    let (factory, registry) = (Arc::clone(factory), Arc::clone(registry));
    let (instance_id, verb, waiting_action) = (instance_id.to_string(), verb.to_string(), action_id.clone());
    let thread_lease = lease_id.clone();
    // the user who asked, so the action and its lease's use are theirs on the thread too
    let user = moqui_core::user::current_user_id().ok().flatten();
    let spawned = std::thread::Builder::new()
        .name(format!("instance-{verb}-{instance_id}"))
        .spawn(move || {
            let context = factory.new_context();
            let _entered = context.enter();
            let Some(facade) = factory.entity() else {
                return;
            };
            let _authz = crate::common::authz_off();
            if let Some(user) = &user {
                if let Err(err) = moqui_core::user::internal_login_user(user) {
                    tracing::warn!("Instances: the waiting action {waiting_action} runs with no user: {err}");
                }
            }
            let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let Err(err) = registry.wait_open(&facade, &thread_lease, limit) {
                    registry.forget(&thread_lease);
                    return Err(err);
                }
                run(
                    &facade,
                    &registry,
                    &instance_id,
                    &verb,
                    Some(&thread_lease),
                    remove_volumes,
                )
            }));
            let (status, report) = match ran {
                Ok(Ok(outcome)) => (DONE, format!("ran as action {}", outcome.action_id)),
                Ok(Err(err)) => (FAILED, err.message().to_string()),
                Err(_) => (FAILED, "the action failed".to_string()),
            };
            if let Err(err) = finish(&facade, &waiting_action, status, &report) {
                tracing::error!("Instances: the waiting action {waiting_action} was not updated: {err}");
            }
        });
    if let Err(err) = spawned {
        finish(facade, &action_id, FAILED, "the action could not wait for its lease")?;
        return Err(fail(format!("The action could not wait for its lease: {err}")));
    }
    Ok(Requested {
        action_id,
        status: WAITING.to_string(),
        lease_id: Some(lease_id),
    })
}
