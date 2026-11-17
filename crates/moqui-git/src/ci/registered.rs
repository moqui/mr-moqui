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
/// no enabled pool's, or a pool that has its most runners ([`register_in`]).
pub fn register(
    entity: &EntityFacade,
    pool_token: &str,
    name: &str,
    platform: Option<&str>,
) -> Result<Option<(String, String)>, Error> {
    let Some(pool_id) = token::pool_of(entity, pool_token)? else {
        return Ok(None);
    };
    register_in(entity, &pool_id, name, platform)
}

/// The most active runners a pool of registered runners has (`git-ci` `max-runners-per-pool`).
pub const DEFAULT_MAX_RUNNERS: u64 = 50;

/// Register a runner in the pool `pool_id`, its pool token already found good: its id and its runner token, shown once;
/// `None` when the pool has `max-runners-per-pool` (50) active runners already. A pool token that leaked can then mint
/// only so many runner tokens, each a credential the web protection exempts once found good; revoke a runner, or make
/// a new pool token that revokes them, to register more.
pub fn register_in(
    entity: &EntityFacade,
    pool_id: &str,
    name: &str,
    platform: Option<&str>,
) -> Result<Option<(String, String)>, Error> {
    let _rows = crate::rows_guard();
    // 0 is no cap, as the other CI caps
    let max = match super::setting("max-runners-per-pool").trim().parse::<u64>() {
        Ok(0) => i64::MAX,
        Ok(max) => i64::try_from(max).unwrap_or(i64::MAX),
        Err(_) => i64::try_from(DEFAULT_MAX_RUNNERS).unwrap_or(i64::MAX),
    };
    // the count and the insert under the pool's lease, so registrations at once, on any member, cannot pass the cap
    // together; a pool whose lease another member keeps past a few seconds is refused as full, to be tried again
    let _turn = if max < i64::MAX {
        match super::engine::pool_turn(entity, pool_id)? {
            Some(turn) => Some(turn),
            None => {
                tracing::warn!(pool = pool_id, "a runner was not registered: the pool's lease is held");
                return Ok(None);
            }
        }
    } else {
        None
    };
    let active = entity
        .find(RUNNER)?
        .eq("runnerPoolId", Value::string(pool_id))
        .eq("statusId", Value::string("active"))
        .count()?;
    if active >= max {
        tracing::warn!(
            pool = pool_id,
            "a runner was not registered: the pool has max-runners-per-pool runners"
        );
        return Ok(None);
    }
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
    row.set(
        "tokenExpireDate",
        token_expiry(super::now_millis()).map_or(Value::Null, moqui_core::user::stamp_value),
    );
    row.create(entity)?;
    Ok(field(&row, "runnerId").map(|id| (id, plain)))
}

/// How long a runner token is good for (`git-ci` `runner-token-days`).
pub const DEFAULT_RUNNER_TOKEN_DAYS: u64 = 90;

/// The days a runner token is good for: `git-ci` `runner-token-days` (90); `None` when it is 0, no expiry.
pub fn runner_token_days() -> Option<u64> {
    match super::setting("runner-token-days").trim().parse::<u64>() {
        Ok(0) => None,
        Ok(days) => Some(days),
        Err(_) => Some(DEFAULT_RUNNER_TOKEN_DAYS),
    }
}

fn days_millis(days: u64) -> i64 {
    i64::try_from(days).unwrap_or(i64::MAX / 86_400_000).saturating_mul(86_400_000)
}

/// When a runner token made at `now` (epoch milliseconds) expires; `None` with no expiry.
fn token_expiry(now: i64) -> Option<i64> {
    runner_token_days().map(|days| now.saturating_add(days_millis(days)))
}

/// When a runner's token expires: its `tokenExpireDate`, else (a runner registered before tokens expired)
/// `runner-token-days` after its `registeredDate`; `None` with no expiry, or for a row with neither date, which only a
/// direct write of the row makes (registration sets both).
pub fn runner_token_expires(runner: &EntityValue) -> Option<i64> {
    super::millis(runner, "tokenExpireDate").or_else(|| {
        let days = runner_token_days()?;
        Some(super::millis(runner, "registeredDate")?.saturating_add(days_millis(days)))
    })
}

/// When a runner rotates a token that expires at `expires`: halfway through its life.
pub fn rotate_after(expires: Option<i64>) -> Option<i64> {
    let expires = expires?;
    Some(expires.saturating_sub(days_millis(runner_token_days()?) / 2))
}

/// Give the active runner `runner_id` a new token, which replaces its old one at once: the new token, shown once, and
/// when it expires. `None` when the runner is not active (revoked meanwhile).
///
/// # Errors
///
/// When its row cannot be written, or the system has no random bytes.
pub fn rotate(entity: &EntityFacade, runner_id: &str) -> Result<Option<(String, Option<i64>)>, Error> {
    let _rows = crate::rows_guard();
    let (plain, hashed) = token::new_token(token::RUNNER_PREFIX)?;
    let expires = token_expiry(super::now_millis());
    let rotated = super::set_fields(
        entity,
        RUNNER,
        "runnerId",
        runner_id,
        Some("active"),
        vec![
            ("tokenHash", Value::string(hashed)),
            ("tokenExpireDate", expires.map_or(Value::Null, moqui_core::user::stamp_value)),
        ],
    )?;
    Ok(rotated.then_some((plain, expires)))
}

/// Whether pool `pool_id` takes a runner request from `client_ip`: its `allowedAddresses` ranges, any address when it
/// has none. A list that names no range it can read takes no address, so a mistyped list does not open the pool.
///
/// # Errors
///
/// When the pool cannot be read.
pub fn pool_admits(entity: &EntityFacade, pool_id: &str, client_ip: &str) -> Result<bool, Error> {
    let _rows = crate::rows_guard();
    let Some(pool) = super::one(entity, RUNNER_POOL, "runnerPoolId", pool_id)? else {
        return Ok(false);
    };
    let Some(allowed) = field(&pool, "allowedAddresses") else {
        return Ok(true);
    };
    let ranges = moqui_core::web::address::parse_cidrs(&allowed, &format!("runner pool {pool_id} allowedAddresses"));
    let address = client_ip
        .trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .or_else(|| client_ip.trim().parse::<std::net::SocketAddr>().ok().map(|socket| socket.ip()));
    Ok(address.is_some_and(|address| ranges.iter().any(|range| range.contains(address))))
}

/// Note where runner `runner_id` was last heard from.
///
/// # Errors
///
/// When its row cannot be written.
pub fn note_address(entity: &EntityFacade, runner_id: &str, client_ip: &str) -> Result<(), Error> {
    let _rows = crate::rows_guard();
    let address: String = client_ip.chars().take(60).collect();
    super::set_fields(entity, RUNNER, "runnerId", runner_id, None, vec![("lastAddress", Value::string(address))])?;
    Ok(())
}

/// A job a runner was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claimed {
    pub job_run_id: String,
    pub job_token: String,
}

/// Give the runner of `runner_token` the oldest queued job of its pool, when the pool has room and the runner has no
/// job. `step_user`: the runner runs its steps as another user (`ci-run poll --step-user`), which a pool that takes
/// untrusted runs needs ([`claim_for`]). `Ok(None)` with nothing to run; an error for a token that is no active
/// runner's.
pub fn claim(entity: &EntityFacade, runner_token: &str, step_user: bool) -> Result<Option<Claimed>, Error> {
    let Some((runner_id, pool_id)) = token::runner_of(entity, runner_token)? else {
        return Err(Error::new("the runner token is not valid"));
    };
    claim_for(entity, &runner_id, &pool_id, step_user)
}

/// Whether a runner of pool `pool_id` is given jobs only when it runs its steps as another user: the pool is of
/// registered runners and takes untrusted runs (plans/Security-Review-Low.md SEC-GIT-19).
pub fn needs_step_user(entity: &EntityFacade, pool_id: &str) -> Result<bool, Error> {
    let _rows = crate::rows_guard();
    Ok(super::one(entity, RUNNER_POOL, "runnerPoolId", pool_id)?.is_some_and(|pool| {
        field(&pool, "poolKind").as_deref() == Some("registered") && !super::is_yes(&pool, "trusted")
    }))
}

/// [`claim`] for the active runner `runner_id` of pool `pool_id`, its token already looked up (the runner REST counts
/// its requests by runner once the token is known good). A pool that takes untrusted runs gives a job only to a runner
/// that runs its steps as another user (`step_user`): an untrusted run is code from outside the repository, and a
/// registered runner's machine is not cleaned between jobs, so without it the run waits for such a runner
/// (SEC-GIT-19).
pub fn claim_for(
    entity: &EntityFacade,
    runner_id: &str,
    pool_id: &str,
    step_user: bool,
) -> Result<Option<Claimed>, Error> {
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
    let trusted = super::is_yes(&pool, "trusted");
    if !trusted && !step_user {
        return Ok(None);
    }
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
    // under the pool's lease, so no claim of this runner is between its reservation and its job's claim
    free_stale_job(entity, &runner_id)?;
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
        // from here every way out but the job's claim gives the runner back, or it would stay reserved for a job it
        // never runs and ask in vain from then on
        let job_token = match super::engine::claim_job(entity, &run, job, &pool, Some(&runner_id)) {
            Ok(Some(job_token)) => job_token,
            Ok(None) => {
                release_runner(entity, &runner_id, &job_run_id);
                continue;
            }
            Err(err) => {
                // a claim that failed after the job was claimed gave the job back itself (claim_job)
                release_runner(entity, &runner_id, &job_run_id);
                return Err(err);
            }
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
            "a runner reserved for a job it is not running was not freed: {err}"
        );
    }
}

/// Free a runner whose `currentJobRunId` names a job it is not running: one that has ended or is gone (a member that
/// stopped before it freed it), or one that is not running with this runner (a claim that failed or stopped between
/// the reservation and the job's claim, or a job another claim took). Called under the pool's lease, where no claim of
/// this runner is between the two, after its running jobs were counted.
fn free_stale_job(entity: &EntityFacade, runner_id: &str) -> Result<(), Error> {
    let Some(runner) = super::one(entity, RUNNER, "runnerId", runner_id)? else {
        return Ok(());
    };
    let Some(current) = field(&runner, "currentJobRunId") else {
        return Ok(());
    };
    let running_here = super::one(entity, JOB_RUN, "jobRunId", &current)?.is_some_and(|job| {
        field(&job, "statusId").as_deref() == Some("running") && field(&job, "runnerId").as_deref() == Some(runner_id)
    });
    if !running_here {
        release_runner(entity, runner_id, &current);
    }
    Ok(())
}
