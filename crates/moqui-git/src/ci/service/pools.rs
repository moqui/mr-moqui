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

//! The runner pools of `org.moqui.git.GitCiServices` (docs/interfaces/Pipelines.md "Runners and drivers"): an
//! operator's (`GIT_CI_ADMIN`, or authorization disabled), with their registration tokens and registered runners.

use std::collections::BTreeMap;

use moqui_core::rust_service;

use super::{Out, Parameters};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;

use moqui_core::entity::{EntityFacade, EntityValue};

use super::{add_error, authz_disabled, flag, none, number, out, text};
use crate::ci::{field, field_value, int, is_yes, opt, yes_no, JOB_RUN, RUNNER_POOL};
use crate::values::map;

pub(super) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    rust_service!(map, find_runner_pools)?;
    rust_service!(map, store_runner_pool)?;
    rust_service!(map, delete_runner_pool)?;
    rust_service!(map, create_runner_pool_token)?;
    rust_service!(map, revoke_runner)?;
    Ok(())
}

fn pool_value(pool: &EntityValue) -> Value {
    let copy = |name: &'static str| (name, field_value(pool, name));
    map([
        copy("runnerPoolId"),
        copy("poolName"),
        copy("description"),
        copy("ownerId"),
        copy("instanceHostId"),
        copy("poolKind"),
        ("trusted", Value::Bool(is_yes(pool, "trusted"))),
        ("enabled", Value::Bool(is_yes(pool, "enabled"))),
        copy("maxConcurrent"),
        copy("allowedImages"),
        copy("cpuLimit"),
        copy("memoryMb"),
        copy("diskMb"),
        copy("pidsLimit"),
        copy("maxMinutes"),
        copy("egressRule"),
        copy("serverUrl"),
        (
            "hasRegistrationToken",
            Value::Bool(field(pool, "registrationTokenHash").is_some()),
        ),
    ])
}

/// The permission of who runs the CI's pools (registered runners and their tokens, container hosts): an operator.
pub(crate) const OPERATOR_PERMISSION: &str = "GIT_CI_ADMIN";

/// Whether the caller runs the pools: authorization disabled (the application), or the `GIT_CI_ADMIN` permission.
/// The error recorded when not.
fn operator(what: &str) -> bool {
    if authz_disabled() || moqui_core::user::has_permission(OPERATOR_PERMISSION).unwrap_or(false) {
        return true;
    }
    add_error(format!(
        "{what} is an operator's: the {OPERATOR_PERMISSION} permission, or authorization disabled"
    ));
    false
}

/// `find#RunnerPools`: the pools, with the jobs running and queued in each. Out: `poolList`.
fn find_runner_pools(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !operator("reading the runner pools") {
        return none();
    }
    let _rows = crate::rows_guard();
    let mut find = facade.find(RUNNER_POOL)?;
    if let Some(owner) = text(parameters, "ownerId") {
        find = find.eq("ownerId", Value::string(owner));
    }
    let mut pools = Vec::new();
    for pool in find.order_by("poolName", false).list()?.iter() {
        let id = field(pool, "runnerPoolId").unwrap_or_default();
        let count = |status: &str| -> Result<i64, LibError> {
            facade
                .find(JOB_RUN)?
                .eq("statusId", Value::string(status))
                .eq("runnerPoolId", Value::string(&id))
                .count()
        };
        let Value::Map(mut entries) = pool_value(pool) else {
            continue;
        };
        entries.insert("runningCount".to_string(), int(count("running")?));
        entries.insert("queuedCount".to_string(), int(count("queued")?));
        pools.push(Value::Map(entries));
    }
    Ok(out([("poolList", Value::List(pools))]))
}

/// `store#RunnerPool`: make or change a pool. Out: `runnerPoolId`.
fn store_runner_pool(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !operator("changing a runner pool") {
        return none();
    }
    let _rows = crate::rows_guard();
    let existing = match text(parameters, "runnerPoolId") {
        Some(id) => crate::ci::one(facade, RUNNER_POOL, "runnerPoolId", &id)?,
        None => None,
    };
    let creating = existing.is_none();
    let mut pool = match existing {
        Some(pool) => pool,
        None => {
            let mut pool = EntityValue::make(facade, RUNNER_POOL)?;
            match text(parameters, "runnerPoolId") {
                Some(id) => pool.set("runnerPoolId", Value::string(id)),
                None => pool.set_sequenced_id_primary(facade)?,
            }
            pool.set("poolName", Value::string("default"));
            pool.set("poolKind", Value::string("container"));
            pool.set("trusted", Value::string("Y"));
            pool.set("enabled", Value::string("Y"));
            pool.set("maxConcurrent", int(1));
            pool
        }
    };
    for name in [
        "poolName",
        "description",
        "ownerId",
        "instanceHostId",
        "allowedImages",
        "cpuLimit",
        "egressRule",
        "serverUrl",
    ] {
        if parameters.contains_key(name) {
            pool.set(name, opt(text(parameters, name)));
        }
    }
    for name in ["maxConcurrent", "memoryMb", "diskMb", "pidsLimit", "maxMinutes"] {
        if parameters.contains_key(name) {
            pool.set(
                name,
                text(parameters, name).map_or(Value::Null, |_| int(number(parameters, name, 0))),
            );
        }
    }
    for name in ["trusted", "enabled"] {
        if parameters.contains_key(name) {
            pool.set(name, yes_no(flag(parameters, name)));
        }
    }
    if let Some(kind) = text(parameters, "poolKind") {
        if !matches!(kind.as_str(), "container" | "registered") {
            add_error("poolKind is container or registered");
            return none();
        }
        pool.set("poolKind", Value::string(kind));
    }
    if field(&pool, "poolName").is_none_or(|name| !crate::ci::file::valid_name(&name)) {
        add_error("poolName must be letters, digits, dot, dash and underscore");
        return none();
    }
    if field(&pool, "poolKind").as_deref() == Some("container") && field(&pool, "instanceHostId").is_none() {
        add_error("a pool of containers needs an instanceHostId");
        return none();
    }
    // a registered runner keeps its machine from one job to the next (its files, what a job left running), so its
    // pool is one tenant's; a pool of containers, a fresh container a job, may be shared. A server with the run check
    // has tenants (the application names each run's owner); one without has one, whose runs have no owner, and its
    // pool is that tenant's
    if field(&pool, "poolKind").as_deref() == Some("registered")
        && field(&pool, "ownerId").is_none()
        && !crate::ci::setting("run-check-service").is_empty()
    {
        add_error(
            "a pool of registered runners needs an ownerId: a runner is not cleaned between jobs, so it serves one owner",
        );
        return none();
    }
    if let Some(rule) = field(&pool, "egressRule").filter(|rule| !crate::ci::driver::docker::valid_egress(rule)) {
        add_error(format!(
            "egressRule {rule} is the host's network or another container's: bridge, none, or a network of the host"
        ));
        return none();
    }
    if creating {
        pool.create(facade)?;
    } else {
        pool.update(facade)?;
    }
    Ok(out([("runnerPoolId", field_value(&pool, "runnerPoolId"))]))
}

/// `delete#RunnerPool`: remove a pool with no running job; its queued jobs are then errors when they would start.
fn delete_runner_pool(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !operator("deleting a runner pool") {
        return none();
    }
    let Some(id) = text(parameters, "runnerPoolId") else {
        add_error("runnerPoolId is required");
        return none();
    };
    let _rows = crate::rows_guard();
    let running = facade
        .find(JOB_RUN)?
        .eq("statusId", Value::string("running"))
        .eq("runnerPoolId", Value::string(&id))
        .count()?;
    if running > 0 {
        add_error(format!("runner pool {id} has {running} running jobs"));
        return none();
    }
    facade
        .find(crate::ci::RUNNER)?
        .eq("runnerPoolId", Value::string(&id))
        .delete_all()?;
    facade
        .find(RUNNER_POOL)?
        .eq("runnerPoolId", Value::string(&id))
        .delete_all()?;
    none()
}

/// `create#RunnerPoolToken`: a new registration token for a pool of registered runners, replacing the one it had.
/// Out: `token`, shown once.
fn create_runner_pool_token(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !operator("making a runner pool token") {
        return none();
    }
    let Some(id) = text(parameters, "runnerPoolId") else {
        add_error("runnerPoolId is required");
        return none();
    };
    let days = number(parameters, "validDays", 0);
    let options = crate::ci::registered::TokenOptions {
        valid_days: u64::try_from(days).ok().filter(|days| *days > 0),
        revoke_runners: flag(parameters, "revokeRunners"),
    };
    match crate::ci::registered::new_pool_token(facade, &id, &options) {
        Ok((token, expires)) => Ok(out([
            ("token", Value::string(token)),
            ("expireDate", moqui_core::user::stamp_value(expires)),
        ])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}

/// `revoke#Runner`: a registered runner's token is refused from now on, and the job it runs ends as an error.
fn revoke_runner(parameters: &Parameters, facade: &EntityFacade) -> Out {
    if !operator("revoking a runner") {
        return none();
    }
    let Some(id) = text(parameters, "runnerId") else {
        add_error("runnerId is required");
        return none();
    };
    match crate::ci::registered::revoke(facade, &id) {
        Ok(revoked) => Ok(out([("revoked", Value::Bool(revoked))])),
        Err(err) => {
            add_error(err.message());
            none()
        }
    }
}
