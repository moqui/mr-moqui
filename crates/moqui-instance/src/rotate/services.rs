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

//! The rotation services of `org.moqui.dev.InstanceServices` (plans/archive/Vault-Instances.md §4.7): the kinds'
//! `rotate#Credential`, `request#InstanceRotation` (an instance's Rotate, an environment's incident rotation, a member
//! removal's offer), the rotation list and the job at the policy age. Each checks its caller under the caller's own
//! authorization (INSTANCE_ADMIN, and the environment's approver group when it names one) before it turns authorization
//! off.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::context::{ExecutionContext, ExecutionContextFactory};
use moqui_core::entity::EntityFacade;
use moqui_core::extension::RustServiceFn;
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value, ValueMap};

use crate::common::{
    authz_off, fail, field, find, one, out, require_instance_admin, required, text, Out, Params, ACTION, APP_INSTANCE,
    ENVIRONMENT,
};

pub(crate) fn register(map: &mut BTreeMap<&'static str, RustServiceFn>) -> Result<(), String> {
    moqui_core::rust_service!(map, rotate_credential)?;
    moqui_core::rust_service!(map, request_instance_rotation)?;
    moqui_core::rust_service!(map, get_instance_rotations)?;
    moqui_core::rust_service!(map, rotate_due_credentials)?;
    Ok(())
}

fn factory() -> Result<Arc<ExecutionContextFactory>, LibError> {
    ExecutionContext::with_current(|ec| Arc::clone(ec.factory())).ok_or_else(|| fail("No execution context is entered"))
}

fn flag(params: &Params, name: &str) -> bool {
    matches!(
        text(params, name).to_ascii_lowercase().as_str(),
        "y" | "yes" | "true" | "1"
    )
}

/// The caller's check on an environment, under its own authorization: INSTANCE_ADMIN, and the environment's approver
/// group when it names one.
fn check_environment_caller(facade: &EntityFacade, environment_id: &str) -> Result<(), LibError> {
    require_instance_admin()?;
    let environment = {
        let _authz = authz_off();
        one(facade, ENVIRONMENT, "environmentId", environment_id)?
    }
    .ok_or_else(|| fail(format!("No InstanceEnvironment {environment_id}")))?;
    crate::actions::check_caller(&environment)
}

fn not_rotated(plan: &super::Plan) -> Value {
    Value::List(
        plan.not_rotated
            .iter()
            .map(|not| {
                let mut map = ValueMap::default();
                map.insert("bindingKey".to_string(), Value::string(&not.binding));
                map.insert("reason".to_string(), Value::string(&not.reason));
                Value::Map(map)
            })
            .collect(),
    )
}

/// `rotate#Credential`: the kinds' rotate service, one instance's secret, now, under the lease handed to it or the
/// environment's machine access.
pub(crate) fn rotate_credential(params: &Params, facade: &EntityFacade) -> Out {
    let instance_id = required(params, "appInstanceId")?;
    let secret_name = required(params, "secretName")?;
    crate::actions::check_instance_caller(facade, &instance_id)?;
    let _authz = authz_off();
    let instance = crate::plan::instance(facade, &instance_id)?;
    let binding = format!("AppInstance:{instance_id}:{secret_name}");
    let plan = super::plan(
        facade,
        &field(&instance, "environmentId"),
        std::slice::from_ref(&binding),
    )?;
    let registry = moqui_vault_server::lease::registry(&*factory()?);
    let lease = text(params, "leaseId");
    let outcome = super::run(
        facade,
        &registry,
        &super::LinkInstances { facade },
        &plan,
        Some(lease.as_str()).filter(|lease| !lease.is_empty()),
        &super::Options::from_conf(),
    )?;
    if let Some((_, why)) = outcome.failed.first() {
        return Err(fail(format!(
            "{binding} was not rotated: {why} (action {})",
            outcome.action_ids.join(", ")
        )));
    }
    Ok(out([
        (
            "instanceActionId",
            Value::string(outcome.action_ids.first().cloned().unwrap_or_default()),
        ),
        (
            "revision",
            outcome
                .rotated
                .first()
                .map_or(Value::Null, |(_, revision)| Value::I64(*revision)),
        ),
        ("report", Value::string(outcome.report.join("\n"))),
    ]))
}

/// The bindings a request names: a list, one binding, an instance's secret, or every item of the environment the
/// manager rotates (all; due only past its age).
fn asked_bindings(params: &Params, facade: &EntityFacade, environment_id: &str) -> Result<Vec<String>, LibError> {
    let mut bindings: Vec<String> = match params.get("bindingList") {
        Some(Value::List(list)) => list
            .iter()
            .map(to_plain_string)
            .filter(|text| !text.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let one_binding = text(params, "bindingKey");
    if !one_binding.is_empty() {
        bindings.push(one_binding);
    }
    let (instance_id, secret_name) = (text(params, "appInstanceId"), text(params, "secretName"));
    if !instance_id.is_empty() && !secret_name.is_empty() {
        bindings.push(format!("AppInstance:{instance_id}:{secret_name}"));
    }
    let (all, due_only) = (flag(params, "all"), flag(params, "dueOnly"));
    if all || due_only {
        let ages = super::due::list(facade, environment_id, moqui_core::user::now_millis())?;
        bindings.extend(
            ages.into_iter()
                .filter(|age| age.rotatable && (all || age.due))
                .map(|age| age.binding),
        );
    }
    if bindings.is_empty() {
        return Err(fail(
            "Name what to rotate: bindingList, bindingKey, appInstanceId with secretName, all or dueOnly",
        ));
    }
    Ok(bindings)
}

/// `request#InstanceRotation`: rotates now under the environment's machine access, or asks for the lease and rotates
/// once an approver opens it.
pub(crate) fn request_instance_rotation(params: &Params, facade: &EntityFacade) -> Out {
    let environment_id = match text(params, "environmentId") {
        given if !given.is_empty() => given,
        _ => {
            let instance_id = required(params, "appInstanceId")?;
            let _authz = authz_off();
            field(&crate::plan::instance(facade, &instance_id)?, "environmentId")
        }
    };
    check_environment_caller(facade, &environment_id)?;
    let _authz = authz_off();
    let bindings = asked_bindings(params, facade, &environment_id)?;
    let factory = factory()?;
    let registry = moqui_vault_server::lease::registry(&factory);
    let (requested, plan) = super::request(&factory, facade, &registry, &environment_id, &bindings)?;
    Ok(out([
        ("instanceActionId", Value::string(requested.action_id)),
        ("statusEnumId", Value::string(requested.status)),
        ("leaseId", requested.lease_id.map_or(Value::Null, Value::string)),
        (
            "bindingList",
            Value::List(
                plan.credentials
                    .iter()
                    .map(|credential| Value::string(&credential.binding))
                    .collect(),
            ),
        ),
        ("notRotatedList", not_rotated(&plan)),
    ]))
}

/// `get#InstanceRotations`: the rotation list of an environment (each item of its vault: age, policy, due, whether the
/// manager rotates it, else what to do by hand). No value.
pub(crate) fn get_instance_rotations(params: &Params, facade: &EntityFacade) -> Out {
    let environment_id = required(params, "environmentId")?;
    check_environment_caller(facade, &environment_id)?;
    let _authz = authz_off();
    let ages = super::due::list(facade, &environment_id, moqui_core::user::now_millis())?;
    let due = ages.iter().filter(|age| age.due).count();
    Ok(out([
        (
            "itemList",
            Value::List(ages.iter().map(super::due::ItemAge::to_value).collect()),
        ),
        ("dueCount", Value::I64(i64::try_from(due).unwrap_or(0))),
    ]))
}

/// The notification topic of a credential the job does not retry after its rotation failed.
pub const FAILED_TOPIC: &str = "InstanceRotationFailed";

/// `rotate#DueCredentials`, the job at the policy age: in each environment, the items past their age that the manager
/// rotates and a member has taken in are rotated under its machine access, or a lease of them is asked for, once while
/// one waits. An item whose last rotation failed within `manage@rotate-retry-days` (7) is not tried again: the
/// administrators are told (topic `InstanceRotationFailed`). Called with authorization on, only the environments whose
/// approver group the caller is in (or that name none) are rotated. One environment that fails is logged and the rest go
/// on.
pub(crate) fn rotate_due_credentials(_params: &Params, facade: &EntityFacade) -> Out {
    require_instance_admin()?;
    let as_job = ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false);
    let environments = {
        let _authz = authz_off();
        find(facade, ENVIRONMENT)?.list()?.into_values()
    };
    // the caller's own authorization, before it is turned off: a person runs the job only where they take actions
    let allowed: Vec<bool> = environments
        .iter()
        .map(|environment| as_job || crate::actions::check_caller(environment).is_ok())
        .collect();
    let _authz = authz_off();
    let factory = factory()?;
    let registry = moqui_vault_server::lease::registry(&factory);
    let now = moqui_core::user::now_millis();
    let retry_days: i64 = moqui_core::context::conf_env::setting("manage@rotate-retry-days")
        .parse()
        .unwrap_or(7);
    let (mut rotated, mut asked, mut failed, mut held, mut skipped) = (0_i64, 0_i64, 0_i64, 0_i64, 0_i64);
    for (environment, allowed) in environments.iter().zip(allowed) {
        let environment_id = field(environment, "environmentId");
        let vault_id = field(environment, "vaultId");
        if vault_id.is_empty() {
            continue;
        }
        if !allowed {
            skipped += 1;
            continue;
        }
        let ran = (|| -> Result<(), LibError> {
            let mut due: Vec<String> = Vec::new();
            for age in super::due::list(facade, &environment_id, now)? {
                if !(age.due && age.rotatable && !age.deposited) {
                    continue;
                }
                let since = now - retry_days.max(0) * 86_400_000;
                match super::claim::failed_since(facade, &vault_id, &age.binding, since)? {
                    Some(action_id) => {
                        held += 1;
                        tell_failed(&environment_id, &age.binding, &action_id, retry_days);
                    }
                    None => due.push(age.binding),
                }
            }
            if due.is_empty() {
                return Ok(());
            }
            if field(environment, "machineAccess") != "Y" && rotation_waits(facade, &environment_id)? {
                return Ok(());
            }
            let (requested, _) = super::request(&factory, facade, &registry, &environment_id, &due)?;
            if requested.lease_id.is_some() {
                asked += 1;
            } else {
                rotated += 1;
            }
            Ok(())
        })();
        if let Err(err) = ran {
            failed += 1;
            tracing::error!("Instances: the due rotation of environment {environment_id} failed: {err}");
        }
    }
    Ok(out([
        ("rotatedCount", Value::I64(rotated)),
        ("askedCount", Value::I64(asked)),
        ("failedCount", Value::I64(failed)),
        ("heldBackCount", Value::I64(held)),
        ("skippedCount", Value::I64(skipped)),
    ]))
}

/// Tells the administrators that a due credential is not rotated again because its last rotation failed; a failure to
/// tell is logged.
fn tell_failed(environment_id: &str, binding: &str, action_id: &str, retry_days: i64) {
    tracing::warn!(
        "Instances: {binding} of environment {environment_id} is due, and its last rotation failed (action {action_id}): \
         not retried for {retry_days} days"
    );
    let mut message = ValueMap::default();
    for (name, value) in [
        ("environmentId", environment_id),
        ("bindingKey", binding),
        ("instanceActionId", action_id),
    ] {
        message.insert(name.to_string(), Value::string(value));
    }
    let clean = |text: &str| text.replace(['$', '{', '}'], "");
    let sent = moqui_core::notify::NotificationMessage::new()
        .topic(FAILED_TOPIC)
        .user_group_id("ADMIN")
        .notification_type("warning")
        .message_map(message)
        .title(format!(
            "{} is due and its last rotation failed (action {}): see its report, then rotate it again",
            clean(binding),
            clean(action_id)
        ))
        .send();
    if let Err(err) = sent {
        tracing::error!(
            "Instances: the administrators were not told of {binding}'s failed rotation: {}",
            err.message()
        );
    }
}

/// Whether a rotation of an instance of the environment waits for its lease now.
fn rotation_waits(facade: &EntityFacade, environment_id: &str) -> Result<bool, LibError> {
    let waiting = find(facade, ACTION)?
        .eq("actionEnumId", Value::string("IatRotate"))
        .eq("statusEnumId", Value::string(crate::actions::WAITING))
        .list()?
        .into_values();
    for action in waiting {
        let instance = one(facade, APP_INSTANCE, "appInstanceId", &field(&action, "appInstanceId"))?;
        if instance.is_some_and(|row| field(&row, "environmentId") == environment_id) {
            return Ok(true);
        }
    }
    Ok(false)
}
