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

//! Registered runners (plans/Dogfood.md §8.3, docs/interfaces/Pipelines.md "Registered runners"): a long-lived machine
//! that asks for jobs, for what a container cannot be (macOS, Windows). It registers once with its pool's token and
//! gets a runner token; with that it asks for a job, and is given a queued job of its pool with a job token, after
//! which it is a runner like any other on the runner REST. The engine starts nothing for such a pool.

use moqui_lib::object::Value;

use moqui_core::context::Error;
use moqui_core::entity::{EntityFacade, EntityValue};

use super::{field, long, opt, token, JOB_RUN, PIPELINE_RUN, RUNNER, RUNNER_POOL};

/// Give a pool of registered runners a new registration token, replacing the one it had. The token, shown once.
///
/// # Errors
///
/// When there is no such pool, or it is not of registered runners.
pub fn new_pool_token(entity: &EntityFacade, runner_pool_id: &str) -> Result<String, Error> {
    let _rows = crate::rows_guard();
    let pool = super::one(entity, RUNNER_POOL, "runnerPoolId", runner_pool_id)?
        .ok_or_else(|| Error::new(format!("no runner pool {runner_pool_id}")))?;
    if field(&pool, "poolKind").as_deref() != Some("registered") {
        return Err(Error::new(format!(
            "runner pool {runner_pool_id} is not of registered runners"
        )));
    }
    let (plain, hashed) = token::new_token(token::POOL_PREFIX)?;
    super::set_fields(
        entity,
        RUNNER_POOL,
        "runnerPoolId",
        runner_pool_id,
        None,
        vec![("registrationTokenHash", Value::string(hashed))],
    )?;
    Ok(plain)
}

/// Register a runner in the pool of `pool_token`. Its id and its runner token, shown once; `None` for a token that is
/// no enabled pool's.
pub fn register(
    entity: &EntityFacade,
    pool_token: &str,
    name: &str,
    platform: Option<&str>,
) -> Result<Option<(String, String)>, Error> {
    let Some(pool_id) = token::pool_of(entity, pool_token)? else {
        return Ok(None);
    };
    let _rows = crate::rows_guard();
    let (plain, hashed) = token::new_token(token::RUNNER_PREFIX)?;
    let mut row = EntityValue::make(entity, RUNNER)?;
    row.set_sequenced_id_primary(entity)?;
    row.set("runnerPoolId", Value::string(pool_id));
    let name: String = name.chars().take(100).collect();
    row.set("name", Value::string(name));
    row.set(
        "platform",
        opt(platform.map(|platform| platform.chars().take(100).collect())),
    );
    row.set("tokenHash", Value::string(hashed));
    row.set("statusId", Value::string("active"));
    row.set("registeredDate", moqui_core::user::now_value());
    row.set("lastContactDate", moqui_core::user::now_value());
    row.create(entity)?;
    Ok(field(&row, "runnerId").map(|id| (id, plain)))
}

/// A job a runner was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub job_run_id: String,
    pub job_token: String,
}

/// Give the runner of `runner_token` the oldest queued job of its pool, when the pool has room and the runner has no
/// job. `Ok(None)` with nothing to run; an error for a token that is no active runner's.
pub fn claim(entity: &EntityFacade, runner_token: &str) -> Result<Option<Claimed>, Error> {
    let Some((runner_id, pool_id)) = token::runner_of(entity, runner_token)? else {
        return Err(Error::new("the runner token is not valid"));
    };
    let _rows = crate::rows_guard();
    super::set_fields(
        entity,
        RUNNER,
        "runnerId",
        &runner_id,
        None,
        vec![("lastContactDate", moqui_core::user::now_value())],
    )?;
    let Some(pool) = super::one(entity, RUNNER_POOL, "runnerPoolId", &pool_id)? else {
        return Ok(None);
    };
    if !super::is_yes(&pool, "enabled") || field(&pool, "poolKind").as_deref() != Some("registered") {
        return Ok(None);
    }
    // a runner runs one job at a time
    let busy = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("running"))
        .eq("runnerId", Value::string(&runner_id))
        .count()?;
    if busy > 0 {
        return Ok(None);
    }
    let running = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("running"))
        .eq("runnerPoolId", Value::string(&pool_id))
        .count()?;
    if running >= long(&pool, "maxConcurrent").unwrap_or(1).max(1) {
        return Ok(None);
    }
    let trusted = super::is_yes(&pool, "trusted");
    let pool_name = field(&pool, "poolName").unwrap_or_else(|| "default".to_string());
    let pool_owner = field(&pool, "ownerId");
    let queued = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("queued"))
        .order_by("queuedDate", false)
        .order_by("jobRunId", false)
        .limit(200)
        .list()?;
    for job in queued.iter() {
        // the pool the application named, else the pool's name
        let named = match field(job, "runnerPoolId") {
            Some(id) => id == pool_id,
            None => field(job, "poolName").unwrap_or_else(|| "default".to_string()) == pool_name,
        };
        if !named || !super::engine::image_allowed(&pool, &field(job, "image").unwrap_or_default()) {
            continue;
        }
        let Some(run) = field(job, "pipelineRunId")
            .map(|id| super::one(entity, PIPELINE_RUN, "pipelineRunId", &id))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        if super::is_yes(&run, "trusted") != trusted
            || pool_owner
                .as_ref()
                .is_some_and(|owner| field(&run, "ownerId").as_ref() != Some(owner))
        {
            continue;
        }
        let Some(job_token) = super::engine::claim_job(entity, &run, job, &pool, Some(&runner_id))? else {
            continue;
        };
        let job_run_id = field(job, "jobRunId").unwrap_or_default();
        super::set_fields(
            entity,
            RUNNER,
            "runnerId",
            &runner_id,
            None,
            vec![("currentJobRunId", Value::string(&job_run_id))],
        )?;
        return Ok(Some(Claimed { job_run_id, job_token }));
    }
    Ok(None)
}
