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

//! Checks in the Workspace (plans/archive/design-dev-aide.md 21.5, P12): the combined state of the statuses of commits (a chip
//! on Commits and on Branches), a commit's checks with its pipeline runs and their jobs, a job's log (followed while it
//! runs), its failed tests as links to the working tree's file and line, and Re-run of a run or a job for a user who
//! may. All through the framework's REST as the connected user (`commits/{id}/statuses`, `pipelines`, `jobs`), which
//! reads a repository an application keeps as any other; a re-run of one the application keeps is refused there, with
//! the application's REST named.

use std::path::Path;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::client::query_text;
use super::review::{need_hosted, Call, Hosted, Via};
use crate::tier::Tier;
use crate::vcs::{audit_git, git_root};
use crate::workspace::{json_value, text, Context};

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_commit_states), get_commit_states)?;
    registry.service_function(moqui_core::service_location!(get_checks), get_checks)?;
    registry.service_function(moqui_core::service_location!(get_job), get_job)?;
    registry.service_function(moqui_core::service_location!(rerun_check), rerun_check)
}

/// The most commits one call asks the states of: a page of Commits.
const MAX_COMMITS: usize = 50;

/// The state a chip shows of a combined state: `failure` (failed or erred), `pending`, `success`, or none.
#[must_use]
pub fn chip_state(state: &str) -> &'static str {
    match state.to_ascii_lowercase().as_str() {
        "failure" | "error" | "failed" | "gcsfailure" | "gcserror" => "failure",
        "pending" | "running" | "queued" | "gcspending" => "pending",
        "success" | "gcssuccess" => "success",
        _ => "",
    }
}

/// A failed test's file as a link into the working tree: its path relative to `root` (as the server names it, or a
/// path that ends in a file of the tree: a runner's absolute path), with its line; `None` for a file the tree does not
/// have.
#[must_use]
pub fn test_link(root: &Path, file_path: &str, line: Option<i64>) -> Option<Json> {
    let file_path = file_path.trim().replace('\\', "/");
    if file_path.is_empty() || file_path.split('/').any(|part| part == "..") {
        return None;
    }
    let relative = file_path.trim_start_matches("./").trim_start_matches('/');
    let segments: Vec<&str> = relative.split('/').filter(|part| !part.is_empty()).collect();
    // the server's path, else the shortest tail of a runner's absolute path that is a file here
    // a candidate is a plain relative path: no drive (`C:`), root or `..` that would make root.join leave the tree
    let plain = |candidate: &str| {
        !candidate.contains(':')
            && Path::new(candidate)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
    };
    let found = (0..segments.len()).find_map(|start| {
        let candidate = segments[start..].join("/");
        (plain(&candidate) && root.join(&candidate).is_file()).then_some(candidate)
    })?;
    Some(json!({ "path": found, "line": line.filter(|line| *line > 0) }))
}

/// The framework's REST of the hosted repository.
fn repository_path(hosted: &Hosted) -> String {
    format!(
        "{}/{}",
        hosted.server.rest_path("repositories"),
        query_text(&hosted.id())
    )
}

/// `get#CommitStates`: the combined state of each commit's statuses (`commitIds`, comma separated: at most a page of
/// Commits, or the heads of Branches), as chips: `{commitId: state}`. A commit with no status, or one the user may not
/// read, has none.
fn get_commit_states(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let mut out = Parameters::new();
    let Some(hosted) = super::review::hosted(&context, root) else {
        out.insert("states".into(), json_value(&json!({})));
        return Ok(out);
    };
    let ids: Vec<String> = text(parameters, "commitIds")
        .split(',')
        .map(str::trim)
        .filter(|id| id.len() >= 7 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .take(MAX_COMMITS)
        .map(str::to_string)
        .collect();
    let mut states = serde_json::Map::new();
    for id in &ids {
        let path = format!("{}/commits/{}/statuses", repository_path(&hosted), query_text(id));
        match hosted.server.get(&path, &[]) {
            Ok(answer) => {
                let state = chip_state(answer["state"].as_str().unwrap_or(""));
                if !state.is_empty() {
                    states.insert(id.clone(), json!(state));
                }
            }
            // one commit that is not answered leaves the others' chips
            Err(err) => tracing::debug!("Dev AIDE: no statuses of {id}: {err}"),
        }
    }
    out.insert("states".into(), json_value(&Json::Object(states)));
    Ok(out)
}

/// `get#Checks`: a commit's checks: its statuses (each context, its state, description and link) and the pipeline
/// runs of the commit with their jobs.
fn get_checks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let commit = text(parameters, "commitId");
    if commit.is_empty() || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(LibError::new("a commit id is required"));
    }
    let base = repository_path(&hosted);
    let statuses = hosted
        .server
        .get(&format!("{base}/commits/{}/statuses", query_text(&commit)), &[])?;
    let status_list: Vec<Json> = statuses["statusList"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|status| {
            json!({ "context": status["context"], "state": chip_state(status["statusId"].as_str()
                    .or_else(|| status["state"].as_str()).unwrap_or("")),
                "description": status["description"],
                "url": super::review::page_url(&hosted.server, &status["targetUrl"]) })
        })
        .collect();
    let runs = hosted
        .server
        .get(&format!("{base}/pipelines"), &[("commitId", commit.as_str())])
        .map(|answer| answer["runList"].as_array().cloned().unwrap_or_default())
        .unwrap_or_default();
    let mut run_list = Vec::new();
    for run in runs.iter().take(10) {
        let number = match &run["runNum"] {
            Json::Number(number) => number.to_string(),
            Json::String(number) => number.clone(),
            _ => continue,
        };
        let detail = hosted
            .server
            .get(&format!("{base}/pipelines/{}", query_text(&number)), &[])
            .unwrap_or(Json::Null);
        let jobs: Vec<Json> = detail["jobList"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|job| {
                json!({ "jobRunId": job["jobRunId"], "jobName": job["jobName"], "matrixKey": job["matrixKey"],
                    "state": chip_state(job["statusId"].as_str().unwrap_or("")), "statusId": job["statusId"],
                    "testFailedCount": job["testFailedCount"], "hasLog": job["hasLog"] })
            })
            .collect();
        run_list.push(json!({
            "runNum": number, "pipelineName": run["pipelineName"], "statusId": run["statusId"],
            "state": chip_state(run["statusId"].as_str().unwrap_or("")), "jobs": jobs,
            "canUpdate": detail["canUpdate"].as_bool().unwrap_or(false),
        }));
    }
    let mut out = Parameters::new();
    out.insert(
        "state".into(),
        Value::string(chip_state(statuses["state"].as_str().unwrap_or(""))),
    );
    out.insert("statuses".into(), json_value(&status_list));
    out.insert("runs".into(), json_value(&run_list));
    Ok(out)
}

/// `get#Job`: a job of a run: its state, its log from `offset` (a running job's next piece, to follow it), and its
/// failed tests, each with a link to its file and line in the working tree where the tree has the file.
fn get_job(ec: &ExecutionContext, _service: &ServiceInfo<'_>, parameters: &Parameters) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let job = text(parameters, "jobRunId");
    if job.is_empty() {
        return Err(LibError::new("jobRunId is required"));
    }
    let base = format!("{}/jobs/{}", repository_path(&hosted), query_text(&job));
    let detail = hosted.server.get(&base, &[])?;
    let offset = text(parameters, "offset");
    let offset = if offset.trim().is_empty() {
        "0".to_string()
    } else {
        offset
    };
    let log = hosted
        .server
        .get(&format!("{base}/log"), &[("offset", offset.as_str())])
        .unwrap_or(Json::Null);
    let tests: Vec<Json> = detail["failedTestList"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|test| {
            let line = test["lineNumber"].as_i64();
            json!({ "suiteName": test["suiteName"], "testName": test["testName"], "message": test["message"],
                "filePath": test["filePath"], "lineNumber": line,
                "link": test_link(&root.path, test["filePath"].as_str().unwrap_or(""), line) })
        })
        .collect();
    let mut out = Parameters::new();
    out.insert("job".into(), json_value(&detail["job"]));
    out.insert("steps".into(), json_value(&detail["stepList"]));
    out.insert("failedTests".into(), json_value(&tests));
    out.insert("text".into(), Value::string(log["text"].as_str().unwrap_or("")));
    out.insert("nextOffset".into(), json_value(&log["nextOffset"]));
    out.insert(
        "finished".into(),
        Value::Bool(log["finished"].as_bool().unwrap_or(true)),
    );
    Ok(out)
}

/// A re-run of a run (`run`, its number), or a retry of one of its jobs (`job`): through the framework's REST
/// (`pipelines/{n}/rerun`, `jobs/{id}/retry`), or for a repository an application keeps through its operations
/// (Smithy's `rerunPipelineRun`, `retryJobRun`), which the framework's REST refuses for it. `None` without a run's
/// number that is a whole number.
#[must_use]
pub(crate) fn rerun_call(via: &Via, repository_path: &str, repository_id: &str, run: &str, job: &str) -> Option<Call> {
    let run_ok = !run.is_empty() && run.bytes().all(|byte| byte.is_ascii_digit());
    let path = match via {
        Via::Framework if !job.is_empty() => format!("{repository_path}/jobs/{}/retry", query_text(job)),
        Via::Framework if run_ok => format!("{repository_path}/pipelines/{run}/rerun"),
        Via::Framework => return None,
        Via::Application(application) => {
            if !run_ok {
                return None;
            }
            let base = format!(
                "{}/repositories/{}/pipelines/{run}",
                application.rest_base,
                query_text(repository_id)
            );
            if job.is_empty() {
                format!("{base}/rerun")
            } else {
                format!("{base}/jobs/{}/retry", query_text(job))
            }
        }
    };
    let body = match via {
        Via::Framework => json!({}),
        Via::Application(_) => json!({ "operationId": super::client::operation_id() }),
    };
    Some(Call {
        post: true,
        path,
        query: Vec::new(),
        body: Some(body),
    })
}

/// `rerun#Check`: a run again (`runNum`), or one job of it (`jobRunId`), on the server as the user, who may.
fn rerun_check(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "A re-run is asked from a session, not a request logged in by a header",
        ));
    }
    let _authz = ec.disable_authz();
    let root = git_root(&context, parameters)?;
    let hosted = need_hosted(&context, root)?;
    let (job, run) = (text(parameters, "jobRunId"), text(parameters, "runNum"));
    let call = rerun_call(&hosted.via(), &repository_path(&hosted), &hosted.id(), &run, &job)
        .ok_or_else(|| LibError::new("a run's number is required, with its job for a retry"))?;
    let answer = super::client::result_of(&call.send(&hosted.server)?).clone();
    audit_git(&context, root, "rerun", json!({ "run": run, "job": job }));
    let mut out = Parameters::new();
    out.insert("runNum".into(), json_value(&answer["runNum"]));
    out.insert("statusId".into(), json_value(&answer["statusId"]));
    Ok(out)
}
