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

//! The tokens of the engine (plans/Dogfood.md §8.3, docs/interfaces/Pipelines.md "Job tokens"): random, stored only as
//! their SHA-256. A job token is good for one job run while it runs: it reads that repository at that commit over
//! git HTTP and calls the runner endpoints of that job, and nothing else. A runner token is a registered runner's; a
//! pool token registers one.

use sha2::{Digest, Sha256};

use moqui_lib::object::Value;

use moqui_core::context::Error;
use moqui_core::entity::EntityFacade;

use super::{field, millis, now_millis, JOB_RUN, PIPELINE_RUN, RUNNER, RUNNER_POOL};

/// A job token starts with this, so the git server and the runner endpoints tell it from an access token.
pub const JOB_PREFIX: &str = "mqcj_";
pub const RUNNER_PREFIX: &str = "mqcr_";
pub const POOL_PREFIX: &str = "mqcp_";

/// The SHA-256 of a token, lowercase hex: what is stored.
pub fn hash(token: &str) -> String {
    moqui_lib::codec::hex_encode(&Sha256::digest(token.as_bytes()))
}

/// A new token with `prefix`: 32 random bytes in hex. The token, to hand out once, and its hash, to store.
///
/// # Errors
///
/// When the system has no random bytes.
pub fn new_token(prefix: &str) -> Result<(String, String), Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|err| Error::new(format!("no random bytes for a token: {err}")))?;
    let token = format!("{prefix}{}", moqui_lib::codec::hex_encode(&bytes));
    let hashed = hash(&token);
    Ok((token, hashed))
}

/// The token of an `Authorization` header: `Bearer <token>`, or the password of `Basic` (as git sends it).
pub fn from_authorization(header: Option<&str>) -> Option<String> {
    let header = header?.trim();
    if let Some(token) = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
    {
        return Some(token.trim().to_string());
    }
    moqui_core::web::basic_credentials(header).map(|(_, password)| password)
}

/// What a job token stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobAuth {
    pub job_run_id: String,
    pub pipeline_run_id: String,
    pub repository_id: String,
    pub commit_id: String,
    /// The merge request of a merge request's run: its head in `repository_id` is what the job fetches.
    pub merge_request_id: Option<String>,
}

/// Whether `token` has the form of a token of `prefix`: the prefix and 64 lowercase hex digits ([`new_token`]).
pub fn well_formed(token: &str, prefix: &str) -> bool {
    token
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.len() == 64 && rest.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
}

/// What a job token is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobToken {
    /// The token of a running job, within its time.
    Running(JobAuth),
    /// The token of a job that has ended (cancelled, timed out, finished) or is past its time: the runner REST answers
    /// 410 Gone, which is no failed sign-in.
    Ended,
    /// Not a job token, or not one this server made: 401.
    Unknown,
}

/// The running job `token` is for. `None` for a token that is not a job token, is not known, whose job has ended or
/// is past its time.
pub fn job_of(entity: &EntityFacade, token: &str) -> Result<Option<JobAuth>, Error> {
    Ok(match job_token(entity, token)? {
        JobToken::Running(auth) => Some(auth),
        JobToken::Ended | JobToken::Unknown => None,
    })
}

/// What `token` is: a running job's (and what it stands for), an ended job's, or unknown. A job keeps its token's hash
/// when it ends, so a runner's late call (after a cancel or a timeout) is told the job is gone; nothing takes the token
/// of a job that is not running.
pub fn job_token(entity: &EntityFacade, token: &str) -> Result<JobToken, Error> {
    if !well_formed(token, JOB_PREFIX) {
        return Ok(JobToken::Unknown);
    }
    let _rows = crate::rows_guard();
    let Some(job) = entity
        .find(JOB_RUN)?
        .eq("tokenHash", Value::string(hash(token)))
        .one()?
    else {
        return Ok(JobToken::Unknown);
    };
    if field(&job, "statusId").as_deref() != Some("running")
        || millis(&job, "tokenExpireDate").is_none_or(|expires| expires <= now_millis())
    {
        return Ok(JobToken::Ended);
    }
    let Some(auth) = auth_of(entity, &job)? else {
        return Ok(JobToken::Ended);
    };
    Ok(JobToken::Running(auth))
}

/// What the running job row `job` stands for; `None` when its run is gone.
fn auth_of(entity: &EntityFacade, job: &moqui_core::entity::EntityValue) -> Result<Option<JobAuth>, Error> {
    let (Some(job_run_id), Some(pipeline_run_id)) = (field(job, "jobRunId"), field(job, "pipelineRunId")) else {
        return Ok(None);
    };
    let Some(run) = super::one(entity, PIPELINE_RUN, "pipelineRunId", &pipeline_run_id)? else {
        return Ok(None);
    };
    let (Some(repository_id), Some(commit_id)) = (field(&run, "repositoryId"), field(&run, "commitId")) else {
        return Ok(None);
    };
    Ok(Some(JobAuth {
        job_run_id,
        pipeline_run_id,
        repository_id,
        commit_id,
        merge_request_id: field(&run, "mergeRequestId"),
    }))
}

/// The active registered runner `token` is: its id and its pool.
pub fn runner_of(entity: &EntityFacade, token: &str) -> Result<Option<(String, String)>, Error> {
    Ok(match runner_token(entity, token)? {
        RunnerToken::Active { runner_id, pool_id, .. } => Some((runner_id, pool_id)),
        RunnerToken::Expired | RunnerToken::Unknown => None,
    })
}

/// What a runner token is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunnerToken {
    /// An active runner's token within its time: the runner, its pool, and when the token expires (`None`: never).
    Active {
        runner_id: String,
        pool_id: String,
        expires: Option<i64>,
    },
    /// An active runner's token past its time (`git-ci` `runner-token-days`): the runner registers again.
    Expired,
    /// Not a runner token this server made, or a revoked runner's.
    Unknown,
}

/// What `token` is: an active runner's within its time, an active runner's that expired, or unknown.
pub fn runner_token(entity: &EntityFacade, token: &str) -> Result<RunnerToken, Error> {
    if !token.starts_with(RUNNER_PREFIX) {
        return Ok(RunnerToken::Unknown);
    }
    let _rows = crate::rows_guard();
    let Some(row) = entity
        .find(RUNNER)?
        .eq("tokenHash", Value::string(hash(token)))
        .eq("statusId", Value::string("active"))
        .one()?
    else {
        return Ok(RunnerToken::Unknown);
    };
    let expires = super::registered::runner_token_expires(&row);
    if expires.is_some_and(|expires| expires <= now_millis()) {
        return Ok(RunnerToken::Expired);
    }
    let (Some(runner_id), Some(pool_id)) = (field(&row, "runnerId"), field(&row, "runnerPoolId")) else {
        return Ok(RunnerToken::Unknown);
    };
    Ok(RunnerToken::Active {
        runner_id,
        pool_id,
        expires,
    })
}

/// The enabled pool of registered runners `token` registers a runner with.
pub fn pool_of(entity: &EntityFacade, token: &str) -> Result<Option<String>, Error> {
    if !token.starts_with(POOL_PREFIX) {
        return Ok(None);
    }
    let _rows = crate::rows_guard();
    // a token past its expiry registers nothing, nor one with none (made before tokens expired: make a new one)
    Ok(entity
        .find(RUNNER_POOL)?
        .eq("registrationTokenHash", Value::string(hash(token)))
        .eq("poolKind", Value::string("registered"))
        .eq("enabled", Value::string("Y"))
        .one()?
        .filter(|row| millis(row, "registrationTokenExpireDate").is_some_and(|expires| expires > now_millis()))
        .and_then(|row| field(&row, "runnerPoolId")))
}
