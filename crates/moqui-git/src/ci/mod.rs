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

//! The CI engine (plans/Dogfood.md §8 and F7, F8; docs/interfaces/Pipelines.md). A pipeline is `ci.xml` at a repository's
//! root ([`file`]), or the built-in one of a component repository ([`builtin`]). gitECA rules and the merge request
//! events queue runs ([`queue`]), each job of a run a `GitJobRun` planned from the file ([`plan`]). A job never runs on
//! the server: the engine ([`engine`]) starts a container through the pool's host type ([`driver`]) or leaves the job
//! for a registered runner ([`registered`]), and the runner (`moqui ci-run`, [`runner`]) talks to the runner REST
//! ([`runner_http`]) with a job token ([`token`]) that also reads the repository at the job's commit. What an untrusted
//! run may have is decided in [`trust`]. The application's services are in [`service`].

use std::collections::BTreeMap;

use moqui_lib::object::{is_empty, to_plain_string, Value};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};

pub mod artifact;
pub mod builtin;
pub mod driver;
pub mod engine;
pub mod file;
pub mod junit;
pub mod log;
pub mod plan;
pub mod queue;
pub mod registered;
pub mod runner;
pub mod runner_http;
pub mod service;
pub mod status;
pub mod token;
pub mod trust;

pub(crate) const PIPELINE_RUN: &str = "moqui.resource.GitPipelineRun";
pub(crate) const JOB_RUN: &str = "moqui.resource.GitJobRun";
pub(crate) const JOB_STEP: &str = "moqui.resource.GitJobStep";
pub(crate) const LOG_CHUNK: &str = "moqui.resource.GitRunLogChunk";
pub(crate) const RUN_ARTIFACT: &str = "moqui.resource.GitRunArtifact";
pub(crate) const TEST_FAILURE: &str = "moqui.resource.GitRunTestFailure";
pub(crate) const CI_SECRET: &str = "moqui.resource.GitCiSecret";
pub(crate) const CI_ENVIRONMENT: &str = "moqui.resource.GitCiEnvironment";
pub(crate) const RUNNER_POOL: &str = "moqui.resource.GitRunnerPool";
pub(crate) const RUNNER: &str = "moqui.resource.GitRunner";
pub(crate) const RUNNER_USAGE: &str = "moqui.resource.GitRunnerUsage";
pub(crate) const DEPLOYMENT: &str = "moqui.resource.GitDeployment";
pub(crate) const MERGE_REQUEST: &str = "moqui.resource.GitMergeRequest";

/// The states a job or a run ends in.
pub(crate) const ENDED: [&str; 5] = ["success", "failure", "error", "cancelled", "skipped"];

/// The services of the engine: the application's ([`service`]), the jobs' ([`engine`]), the drivers' ([`driver`]).
pub(crate) fn register(map: &mut BTreeMap<&'static str, moqui_core::extension::RustServiceFn>) -> Result<(), String> {
    service::register(map)?;
    driver::register(map)
}

/// An attribute of the conf's `git-ci` element, `${}` expanded and trimmed; empty when it is left out.
pub(crate) fn setting(name: &str) -> String {
    ExecutionContext::with_current(|ec| ec.factory().conf_setting("git-ci", name)).unwrap_or_default()
}

/// [`setting`] as a positive number, else `default`.
pub(crate) fn number_setting(name: &str, default: u64) -> u64 {
    setting(name)
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .unwrap_or(default)
}

/// What a pipeline file may hold on this server.
pub(crate) fn limits() -> file::Limits {
    let default = file::Limits::default();
    let number = |name: &str, default: usize| usize::try_from(number_setting(name, default as u64)).unwrap_or(default);
    let small = |name: &str, default: u32| u32::try_from(number_setting(name, u64::from(default))).unwrap_or(default);
    file::Limits {
        max_bytes: number("max-file-bytes", default.max_bytes),
        max_pipelines: number("max-pipelines", default.max_pipelines),
        max_jobs: number("max-jobs", default.max_jobs),
        max_matrix: number("max-matrix", default.max_matrix),
        max_steps: number("max-steps", default.max_steps),
        max_timeout_minutes: small("max-timeout-minutes", default.max_timeout_minutes),
        default_timeout_minutes: small("default-timeout-minutes", default.default_timeout_minutes),
        max_fetch_depth: small("max-fetch-depth", default.max_fetch_depth),
    }
}

pub(crate) fn field(row: &EntityValue, name: &str) -> Option<String> {
    row.get(name)
        .filter(|value| !is_empty(value))
        .map(to_plain_string)
        .filter(|text| !text.trim().is_empty())
}

pub(crate) fn field_value(row: &EntityValue, name: &str) -> Value {
    row.get(name).cloned().unwrap_or(Value::Null)
}

pub(crate) fn is_yes(row: &EntityValue, name: &str) -> bool {
    field(row, name).as_deref() == Some("Y")
}

pub(crate) fn long(row: &EntityValue, name: &str) -> Option<i64> {
    field(row, name).and_then(|text| text.split('.').next().and_then(|whole| whole.parse().ok()))
}

/// A date field in milliseconds.
pub(crate) fn millis(row: &EntityValue, name: &str) -> Option<i64> {
    match moqui_lib::object::basic_convert(row.get(name)?, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

pub(crate) fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

pub(crate) fn opt(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::string)
}

pub(crate) fn yes_no(value: bool) -> Value {
    Value::string(if value { "Y" } else { "N" })
}

pub(crate) fn int(value: i64) -> Value {
    Value::I64(value)
}

/// The row of `entity_name` whose key field `key` is `id`.
pub(crate) fn one(entity: &EntityFacade, entity_name: &str, key: &str, id: &str) -> Result<Option<EntityValue>, Error> {
    Ok(entity.find(entity_name)?.eq(key, Value::string(id)).one()?)
}

/// Set fields of the row of `entity_name` whose key field `key` is `id`, when `status` is still its `statusId`
/// (any when `None`). Whether a row changed: a claim another worker made first gives false.
pub(crate) fn set_fields(
    entity: &EntityFacade,
    entity_name: &str,
    key: &str,
    id: &str,
    status: Option<&str>,
    fields: Vec<(&str, Value)>,
) -> Result<bool, Error> {
    let mut find = entity.find(entity_name)?.eq(key, Value::string(id));
    if let Some(status) = status {
        find = find.eq("statusId", Value::string(status));
    }
    let changed = find.update_all(fields.into_iter().map(|(name, value)| (name.to_string(), value)))?;
    Ok(changed > 0)
}

/// Call the service the conf names, with authorization disabled, as the LFS upload check is called. `None` when no
/// service is named.
pub(crate) fn call_hook(
    conf_name: &str,
    parameters: BTreeMap<String, Value>,
) -> Option<Result<BTreeMap<String, Value>, Error>> {
    let name = setting(conf_name);
    if name.is_empty() {
        return None;
    }
    let options = moqui_core::service::call::CallOptions {
        disable_authz: true,
        ..moqui_core::service::call::CallOptions::default()
    };
    Some(moqui_core::service::call::sync_checked(&name, &parameters, &options))
}

pub(crate) fn parameters(entries: impl IntoIterator<Item = (&'static str, Value)>) -> BTreeMap<String, Value> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// Remove what the engine keeps of a repository: its runs with their jobs, steps, log chunks, failed tests, uploads
/// and usage, its secrets, environments, deployments and schedules. The jobs that have not ended are stopped first
/// (their tokens go with their rows, their containers are removed, their runners freed); no status is written, the
/// repository's go with it. For `delete#GitRepository`, in its transaction, and `delete#RepositoryRuns`. A fork's
/// runs are its own: the runs of a merge request from it are its target's, and stay. The count of runs removed.
pub(crate) fn delete_repository_rows(entity: &EntityFacade, repository_id: &str) -> Result<usize, Error> {
    let _rows = crate::rows_guard();
    if !entity.table_ready(PIPELINE_RUN) {
        return Ok(0);
    }
    let runs = entity
        .find(PIPELINE_RUN)?
        .eq("repositoryId", Value::string(repository_id))
        .list()?;
    for run in runs.iter() {
        let Some(run_id) = field(run, "pipelineRunId") else {
            continue;
        };
        let jobs = entity
            .find(JOB_RUN)?
            .eq("pipelineRunId", Value::string(&run_id))
            .list()?;
        for job in jobs.iter() {
            let Some(job_id) = field(job, "jobRunId") else { continue };
            if field(job, "statusId").is_some_and(|status| !ENDED.contains(&status.as_str())) {
                engine::release(entity, job);
            }
            for entity_name in [JOB_STEP, LOG_CHUNK, TEST_FAILURE, RUN_ARTIFACT, RUNNER_USAGE] {
                entity
                    .find(entity_name)?
                    .eq("jobRunId", Value::string(&job_id))
                    .delete_all()?;
            }
            job.delete(entity)?;
        }
        run.delete(entity)?;
    }
    for entity_name in [RUN_ARTIFACT, CI_SECRET, DEPLOYMENT, CI_ENVIRONMENT, queue::SCHEDULE] {
        entity
            .find(entity_name)?
            .eq("repositoryId", Value::string(repository_id))
            .delete_all()?;
    }
    Ok(runs.len())
}
