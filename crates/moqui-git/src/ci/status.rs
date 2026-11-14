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

//! The commit statuses of the engine (docs/interfaces/Pipelines.md "Statuses"): each job run writes a `GitCommitStatus`
//! with the context `ci/<pipeline>/<job>`, pending when it is queued, then success, failure or error, so a branch
//! rule's required contexts gate a merge as they do for outside CI. The engine writes the row itself, as the system:
//! `create#GitCommitStatus` is a user's, and an application may keep the `ci/` contexts from users; the commit is the
//! run's, which the engine resolved when it queued the run (a hidden merge request's head among them).

use moqui_lib::object::Value;

use moqui_core::context::Error;
use moqui_core::entity::{EntityFacade, EntityValue};

use super::{field, opt, parameters, plan, COMMIT_STATUS, JOB_RUN};

/// The longest description a status row takes.
const DESCRIPTION_MAX: usize = 250;

/// The page of a run, for a status's target: what the conf's `git-ci` `run-url-service` answers (in `repositoryId`,
/// `pipelineRunId`, `runNum`, `jobRunId`; out `url`), else its `run-url` template with `{repositoryId}`,
/// `{repositoryPath}`, `{pipelineRunId}`, `{runNum}` and `{jobRunId}`, else none.
pub fn run_url(run: &EntityValue, job_run_id: Option<&str>) -> Option<String> {
    let repository = field(run, "repositoryId").unwrap_or_default();
    let run_id = field(run, "pipelineRunId").unwrap_or_default();
    let run_num = field(run, "runNum").unwrap_or_default();
    let asked = super::call_hook(
        "run-url-service",
        parameters([
            ("repositoryId", Value::string(&repository)),
            ("pipelineRunId", Value::string(&run_id)),
            ("runNum", Value::string(&run_num)),
            ("jobRunId", opt(job_run_id.map(str::to_string))),
        ]),
    );
    let url = match asked {
        Some(Ok(out)) => out
            .get("url")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default(),
        Some(Err(err)) => {
            tracing::warn!(run = %run_id, "the run URL service failed: {err}");
            String::new()
        }
        None => super::setting("run-url")
            .replace("{repositoryId}", &repository)
            .replace("{repositoryPath}", &crate::server::path::path_of(&repository))
            .replace("{pipelineRunId}", &run_id)
            .replace("{runNum}", &run_num)
            .replace("{jobRunId}", job_run_id.unwrap_or_default()),
    };
    Some(url).filter(|url| url.starts_with("https://") || url.starts_with("http://"))
}

/// Write a status of `commit_id`. Its id.
pub fn write(
    entity: &EntityFacade,
    repository_id: &str,
    commit_id: &str,
    context: &str,
    state: &str,
    description: &str,
    target_url: Option<String>,
) -> Result<String, Error> {
    let _rows = crate::rows_guard();
    let mut row = EntityValue::make(entity, COMMIT_STATUS)?;
    row.set_sequenced_id_primary(entity)?;
    row.set("repositoryId", Value::string(repository_id));
    row.set("commitId", Value::string(commit_id));
    row.set("context", Value::string(context));
    row.set("state", Value::string(state));
    row.set("targetUrl", opt(target_url));
    let description: String = description.chars().take(DESCRIPTION_MAX).collect();
    row.set("description", Value::string(description));
    row.set("createdDate", moqui_core::user::now_value());
    row.create(entity)?;
    Ok(field(&row, "commitStatusId").unwrap_or_default())
}

/// The status state of a job's state.
pub fn state_of(job_status: &str) -> &'static str {
    match job_status {
        "success" => "success",
        "failure" | "skipped" => "failure",
        "error" | "cancelled" => "error",
        _ => "pending",
    }
}

/// Write the status of a job run as it is now, and keep its id on the job. A failure is logged: the job's own state
/// stands.
pub fn write_job(entity: &EntityFacade, run: &EntityValue, job: &EntityValue, description: &str) {
    let (Some(repository), Some(commit), Some(job_run_id)) = (
        field(run, "repositoryId"),
        field(run, "commitId"),
        field(job, "jobRunId"),
    ) else {
        return;
    };
    let context = plan::context(
        &field(run, "pipelineName").unwrap_or_default(),
        &field(job, "jobName").unwrap_or_default(),
        &field(job, "matrixKey").unwrap_or_default(),
    );
    let state = state_of(&field(job, "statusId").unwrap_or_default());
    let written = write(
        entity,
        &repository,
        &commit,
        &context,
        state,
        description,
        run_url(run, Some(&job_run_id)),
    )
    .and_then(|id| {
        super::set_fields(
            entity,
            JOB_RUN,
            "jobRunId",
            &job_run_id,
            None,
            vec![("commitStatusId", Value::string(id))],
        )
    });
    if let Err(err) = written {
        tracing::error!(job = %job_run_id, "the commit status of a job run was not written: {err}");
    }
}
