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

/// How a new pool token is made.
#[derive(Clone, Debug, Default)]
pub struct TokenOptions {
    /// Days it registers runners for; the conf's `git-ci` `pool-token-days` (7) when `None`.
    pub valid_days: Option<u64>,
    /// Revoke the pool's runners too: a pool token that leaked may have registered some.
    pub revoke_runners: bool,
}

/// Give a pool of registered runners a new registration token, replacing the one it had, good for the days of
/// `options`. The token, shown once, and when it expires (epoch milliseconds).
///
/// # Errors
///
/// When there is no such pool, or it is not of registered runners.
pub fn new_pool_token(
    entity: &EntityFacade,
    runner_pool_id: &str,
    options: &TokenOptions,
) -> Result<(String, i64), Error> {
    let _rows = crate::rows_guard();
    let pool = super::one(entity, RUNNER_POOL, "runnerPoolId", runner_pool_id)?
        .ok_or_else(|| Error::new(format!("no runner pool {runner_pool_id}")))?;
    if field(&pool, "poolKind").as_deref() != Some("registered") {
        return Err(Error::new(format!(
            "runner pool {runner_pool_id} is not of registered runners"
        )));
    }
    let (plain, hashed) = token::new_token(token::POOL_PREFIX)?;
    let days = options
        .valid_days
        .unwrap_or_else(|| super::number_setting("pool-token-days", 7));
    let expires = super::now_millis().saturating_add(i64::try_from(days).unwrap_or(i64::MAX / 86_400_000) * 86_400_000);
    super::set_fields(
        entity,
        RUNNER_POOL,
        "runnerPoolId",
        runner_pool_id,
        None,
        vec![
            ("registrationTokenHash", Value::string(hashed)),
            ("registrationTokenExpireDate", moqui_core::user::stamp_value(expires)),
        ],
    )?;
    if options.revoke_runners {
        let runners = entity
            .find(RUNNER)?
            .eq("runnerPoolId", Value::string(runner_pool_id))
            .eq("statusId", Value::string("active"))
            .list()?;
        for runner in runners.iter() {
            if let Some(id) = field(runner, "runnerId") {
                revoke(entity, &id)?;
            }
        }
    }
    Ok((plain, expires))
}

/// Revoke a registered runner: its token is refused from then on (it asks for no job and registers nothing), and the
/// job it runs ends as an error. Whether it was active.
///
/// # Errors
///
/// When its rows cannot be read or written.
pub fn revoke(entity: &EntityFacade, runner_id: &str) -> Result<bool, Error> {
    let _rows = crate::rows_guard();
    let Some(runner) = super::one(entity, RUNNER, "runnerId", runner_id)? else {
        return Ok(false);
    };
    let revoked = super::set_fields(
        entity,
        RUNNER,
        "runnerId",
        runner_id,
        Some("active"),
        vec![("statusId", Value::string("revoked"))],
    )?;
    if let Some(job) = field(&runner, "currentJobRunId") {
        super::engine::finish_job(entity, &job, &super::engine::Outcome::error("The runner was revoked"))?;
    }
    Ok(revoked)
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
    claim_for(entity, &runner_id, &pool_id)
}

/// [`claim`] for the active runner `runner_id` of pool `pool_id`, its token already looked up (the runner REST counts
/// its requests by runner once the token is known good).
pub fn claim_for(entity: &EntityFacade, runner_id: &str, pool_id: &str) -> Result<Option<Claimed>, Error> {
    let (runner_id, pool_id) = (runner_id.to_string(), pool_id.to_string());
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
    let pool_owner = field(&pool, "ownerId");
    // a registered runner keeps its machine from one job to the next, so on a server with tenants (a run check) its
    // pool is one owner's: one stored before that rule, or with its owner taken off, gets no job
    if pool_owner.is_none() && !super::setting("run-check-service").is_empty() {
        return Err(Error::new(format!(
            "runner pool {pool_id} has no ownerId: on a server with git-ci run-check-service a pool of registered \
             runners serves one owner, since a runner is not cleaned between jobs"
        )));
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
    free_stale_job(entity, &runner_id)?;
    let trusted = super::is_yes(&pool, "trusted");
    let pool_name = field(&pool, "poolName").unwrap_or_else(|| "default".to_string());
    let queued = entity
        .find(JOB_RUN)?
        .eq("statusId", Value::string("queued"))
        .order_by("queuedDate", false)
        .order_by("jobRunId", false)
        .limit(200)
        .list()?;
    // nothing queued: no lease taken (a runner asks every few seconds)
    if queued.is_empty() {
        return Ok(None);
    }
    // the count and the claim under the pool's lease, so two members never both take its last place
    let Some(_turn) = super::engine::pool_turn(entity, &pool_id)? else {
        return Ok(None);
    };
    if super::engine::running_in(entity, &pool_id)? >= long(&pool, "maxConcurrent").unwrap_or(1).max(1) {
        return Ok(None);
    }
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
        // a run that ended (cancelled as this claim read its jobs) gives none
        if field(&run, "statusId").is_some_and(|status| super::ENDED.contains(&status.as_str())) {
            continue;
        }
        if super::is_yes(&run, "trusted") != trusted
            || pool_owner
                .as_ref()
                .is_some_and(|owner| field(&run, "ownerId").as_ref() != Some(owner))
        {
            continue;
        }
        // the owner's own pool before a shared one, as the engine picks (pool_for): a shared pool takes no job of an
        // owner that has its own
        if pool_owner.is_none() && field(&run, "ownerId").is_some() {
            if let super::engine::PoolChoice::Pool(chosen) = super::engine::pool_for(entity, &run, job)? {
                if field(&chosen, "ownerId").is_some() {
                    continue;
                }
            }
        }
        let job_run_id = field(job, "jobRunId").unwrap_or_default();
        // the runner first, by one conditional update: of two claims of one runner at once (two members, two
        // requests) only one gets a job
        if !reserve_runner(entity, &runner_id, &job_run_id)? {
            return Ok(None);
        }
        let Some(job_token) = super::engine::claim_job(entity, &run, job, &pool, Some(&runner_id))? else {
            release_runner(entity, &runner_id, &job_run_id);
            continue;
        };
        return Ok(Some(Claimed { job_run_id, job_token }));
    }
    Ok(None)
}

/// `GitRunner.currentJobRunId = job_run_id` where it names no job, committed at once: whether this claim has the
/// runner.
fn reserve_runner(entity: &EntityFacade, runner_id: &str, job_run_id: &str) -> Result<bool, Error> {
    let condition = moqui_core::entity::Condition::and(vec![
        moqui_core::entity::Condition::equals("runnerId", Value::string(runner_id)),
        moqui_core::entity::Condition::is_null("currentJobRunId"),
    ]);
    Ok(crate::conditional::update_where(
        entity,
        RUNNER,
        &[("currentJobRunId", Value::string(job_run_id))],
        &condition,
        crate::conditional::Commit::Own,
    )? > 0)
}

/// Give the runner back when the job it was reserved for went to another claim.
fn release_runner(entity: &EntityFacade, runner_id: &str, job_run_id: &str) {
    let condition = moqui_core::entity::Condition::and(vec![
        moqui_core::entity::Condition::equals("runnerId", Value::string(runner_id)),
        moqui_core::entity::Condition::equals("currentJobRunId", Value::string(job_run_id)),
    ]);
    let released = crate::conditional::update_where(
        entity,
        RUNNER,
        &[("currentJobRunId", Value::Null)],
        &condition,
        crate::conditional::Commit::Own,
    );
    if let Err(err) = released {
        tracing::error!(
            runner = runner_id,
            "a runner reserved for a job another claim took was not freed: {err}"
        );
    }
}

/// Free a runner whose `currentJobRunId` names a job that has ended or is gone (a member that stopped before it freed
/// it): it is free, as its running jobs were just counted.
fn free_stale_job(entity: &EntityFacade, runner_id: &str) -> Result<(), Error> {
    let Some(runner) = super::one(entity, RUNNER, "runnerId", runner_id)? else {
        return Ok(());
    };
    let Some(current) = field(&runner, "currentJobRunId") else {
        return Ok(());
    };
    let ended = super::one(entity, JOB_RUN, "jobRunId", &current)?
        .is_none_or(|job| field(&job, "statusId").is_some_and(|status| super::ENDED.contains(&status.as_str())));
    if ended {
        release_runner(entity, runner_id, &current);
    }
    Ok(())
}
