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
//! rule's required contexts gate a merge as they do for outside CI. The engine writes them as the system through
//! `create#GitCommitStatus`, with authorization disabled and `includeHidden`: one path for every status, so the
//! service ECA rules on it run for the engine's too (an application's notice of a failed check), the commit is one
//! the repository's refs reach (a hidden merge request's head among them), and an application may keep the `ci/`
//! contexts from users (`git-server` `reserved-status-contexts`).

use moqui_lib::object::Value;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};

use super::{field, opt, parameters, plan, JOB_RUN};

const CREATE_STATUS: &str = "org.moqui.git.GitServices.create#GitCommitStatus";

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

/// Write a status of `commit_id`, as the system. Its id.
///
/// # Errors
///
/// What `create#GitCommitStatus` refuses: a commit no ref of the repository reaches any more. Its errors are taken
/// back out of the message facade: a status that is not written does not fail what the engine was doing.
pub fn write(
    repository_id: &str,
    commit_id: &str,
    context: &str,
    state: &str,
    description: &str,
    target_url: Option<String>,
) -> Result<String, Error> {
    // plain text, as the service takes it from outside CI: a message may quote an element (`<step>`) or a runner's
    // own words, and the run's `statusMessage` keeps them whole
    let description: String = description
        .chars()
        .filter(|c| !matches!(c, '<' | '>'))
        .take(DESCRIPTION_MAX)
        .collect();
    let options = moqui_core::service::call::CallOptions {
        disable_authz: true,
        ..moqui_core::service::call::CallOptions::default()
    };
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let called = moqui_core::service::call::sync_with_owned(
        CREATE_STATUS,
        parameters([
            ("repositoryId", Value::string(repository_id)),
            ("commitId", Value::string(commit_id)),
            ("context", Value::string(context)),
            ("state", Value::string(state)),
            ("description", Value::string(description)),
            ("targetUrl", opt(target_url)),
            ("includeHidden", Value::Bool(true)),
        ]),
        &options,
    );
    let errors = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    let out = called?;
    if !errors.is_empty() {
        return Err(Error::new(errors.join("\n")));
    }
    out.get("commitStatusId")
        .filter(|id| !moqui_lib::object::is_empty(id))
        .map(moqui_lib::object::to_plain_string)
        .ok_or_else(|| Error::new("create#GitCommitStatus answered no status"))
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
        tracing::warn!(job = %job_run_id, "the commit status of a job run was not written: {err}");
    }
}
