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

//! The runner REST, at `/ci/runner/` (docs/interfaces/Pipelines.md "The runner REST", docs/security/Attack-Surface.md).
//! It takes no user session: every request carries a token in `Authorization: Bearer`. A job token reaches only
//! `jobs/<its job run>/...`, while that job runs: the job (its steps, variables and, for a trusted run, its secrets),
//! log lines, step results, uploads (artifacts, reports, caches, streamed to content blobs with a size limit), cache
//! downloads and the final result, after which the token is dead. A pool token registers a runner (`register`) and a
//! runner token asks for a job (`claim`). Requests of a job are counted per minute.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use serde_json::json;

use moqui_lib::object::Value;

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::entity::{EntityFacade, EntityValue};

use super::artifact::{self, Kind, Upload};
use super::engine::{self, Outcome};
use super::{field, int, long, millis, now_millis, token, trust, JOB_RUN, JOB_STEP, PIPELINE_RUN, RUNNER_POOL};
use crate::server::HttpResponder;

/// The most bytes of a JSON request body, unless the conf's `git-ci` `max-log-batch-bytes` says otherwise.
const DEFAULT_MAX_JSON: u64 = 1024 * 1024;
/// A job's runner calls are counted per minute: the conf's `git-ci` `runner-max-requests-per-minute`.
const DEFAULT_MAX_REQUESTS: u64 = 1200;
/// `lastContactDate` is written at most this often.
const CONTACT_MILLIS: i64 = 15 * 1000;

/// One request of the runner REST.
#[derive(Clone, Debug)]
pub struct RunnerHttpRequest {
    pub method: String,
    /// The path after `/ci/runner/`.
    pub path: String,
    pub query: String,
    pub authorization: Option<String>,
    pub content_length: Option<u64>,
    /// A token may be taken: the request came over HTTPS, from loopback, or `git-server` `allow-http` is set.
    pub secure: bool,
    pub client_ip: String,
}

/// An answer: a status and JSON, or a refusal with its text.
struct Refused(u16, String);

impl Refused {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self(status, message.into())
    }
}

impl From<Error> for Refused {
    fn from(err: Error) -> Self {
        Self(500, err.message().to_string())
    }
}

impl From<moqui_lib::error::Error> for Refused {
    fn from(err: moqui_lib::error::Error) -> Self {
        Self(500, err.message().to_string())
    }
}

/// Per factory: each job's requests in the current minute.
#[derive(Default)]
struct Rates(Mutex<HashMap<String, (i64, u64)>>);

fn over_rate(key: &str) -> bool {
    let Some(rates) = ExecutionContext::with_current(|ec| ec.factory().extension(|_| Rates::default())) else {
        return false;
    };
    let max = super::number_setting("runner-max-requests-per-minute", DEFAULT_MAX_REQUESTS);
    let minute = now_millis() / 60_000;
    let mut rates = moqui_lib::sync::lock(&rates.0);
    if rates.len() > 10_000 {
        rates.retain(|_, (at, _)| *at == minute);
    }
    let entry = rates.entry(key.to_string()).or_insert((minute, 0));
    if entry.0 != minute {
        *entry = (minute, 0);
    }
    entry.1 += 1;
    entry.1 > max
}

fn respond(out: &mut dyn HttpResponder, status: u16, body: &serde_json::Value) {
    let text = body.to_string();
    let headers = [
        ("Content-Type", "application/json".to_string()),
        ("Content-Length", text.len().to_string()),
        ("Cache-Control", "no-store".to_string()),
    ];
    if out.head(status, &headers).is_ok() {
        let _ = out.body().write_all(text.as_bytes());
        let _ = out.body().flush();
    }
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = text
                    .get(index + 1..index + 3)
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                match hex {
                    Some(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| decode(value))
        .filter(|value| !value.is_empty())
}

fn read_json(request: &RunnerHttpRequest, body: &mut dyn Read) -> Result<serde_json::Value, Refused> {
    let max = super::number_setting("max-log-batch-bytes", DEFAULT_MAX_JSON);
    if request.content_length.is_some_and(|length| length > max) {
        return Err(Refused::new(413, format!("the request is over {max} bytes")));
    }
    let mut bytes = Vec::new();
    body.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| Refused::new(400, format!("the request body was not read: {err}")))?;
    if bytes.len() as u64 > max {
        return Err(Refused::new(413, format!("the request is over {max} bytes")));
    }
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|err| Refused::new(400, format!("the request body is not JSON: {err}")))
}

/// The running job of a job token and its run, for a request at `jobs/<job_run_id>/...`.
fn job_of(entity: &EntityFacade, bearer: &str, job_run_id: &str) -> Result<(EntityValue, EntityValue), Refused> {
    let Some(auth) = token::job_of(entity, bearer)? else {
        moqui_core::web::note_auth_failure();
        return Err(Refused::new(
            401,
            "the job token is not valid: its job has ended, or it is not a job token",
        ));
    };
    if auth.job_run_id != job_run_id {
        return Err(Refused::new(403, "the job token is for another job run"));
    }
    if over_rate(&auth.job_run_id) {
        return Err(Refused::new(429, "too many requests of this job in a minute"));
    }
    let _rows = crate::rows_guard();
    let job = super::one(entity, JOB_RUN, "jobRunId", &auth.job_run_id)?
        .ok_or_else(|| Refused::new(401, "the job run is gone"))?;
    let run = super::one(entity, PIPELINE_RUN, "pipelineRunId", &auth.pipeline_run_id)?
        .ok_or_else(|| Refused::new(401, "the pipeline run is gone"))?;
    if millis(&job, "lastContactDate").is_none_or(|last| now_millis() - last > CONTACT_MILLIS) {
        super::set_fields(
            entity,
            JOB_RUN,
            "jobRunId",
            &auth.job_run_id,
            Some("running"),
            vec![("lastContactDate", moqui_core::user::now_value())],
        )?;
    }
    Ok((job, run))
}

/// `GET jobs/<id>/job`: what the runner needs to run the job.
fn get_job(entity: &EntityFacade, run: &EntityValue, job: &EntityValue) -> Result<serde_json::Value, Refused> {
    let _rows = crate::rows_guard();
    let spec: serde_json::Value = serde_json::from_str(&field(job, "jobSpec").unwrap_or_default())
        .map_err(|err| Refused::new(500, format!("the job's plan does not read: {err}")))?;
    let repository = field(run, "repositoryId").unwrap_or_default();
    let server = field(job, "runnerPoolId")
        .map(|id| super::one(entity, RUNNER_POOL, "runnerPoolId", &id))
        .transpose()?
        .flatten()
        .map_or_else(
            || crate::server::public_base_url().trim_end_matches('/').to_string(),
            |pool| engine::server_url(&pool),
        );
    let path = crate::server::path::path_of(&repository);
    let secrets: serde_json::Map<String, serde_json::Value> = trust::secrets_for(entity, run, &spec, true)?
        .into_iter()
        .map(|given| (given.env, json!(given.value)))
        .collect();
    let commit = field(run, "commitId").unwrap_or_default();
    let ref_name = field(run, "refName").unwrap_or_default();
    Ok(json!({
        "jobRunId": field(job, "jobRunId"),
        "pipelineRunId": field(run, "pipelineRunId"),
        "runNum": long(run, "runNum"),
        "repositoryId": repository,
        "repositoryPath": path,
        "cloneUrl": format!("{server}/git/{path}.git"),
        "commitId": commit,
        "refName": ref_name,
        "event": field(run, "event"),
        "trusted": super::is_yes(run, "trusted"),
        "timeoutMinutes": long(job, "timeoutMinutes"),
        "spec": spec,
        "secrets": secrets,
        // what every job has in its environment
        "ciEnv": {
            "CI": "true",
            "MOQUI_CI": "true",
            "MOQUI_CI_COMMIT": commit,
            "MOQUI_CI_REF": ref_name,
            "MOQUI_CI_EVENT": field(run, "event"),
            "MOQUI_CI_PIPELINE": field(run, "pipelineName"),
            "MOQUI_CI_JOB": field(job, "jobName"),
            "MOQUI_CI_RUN_NUM": field(run, "runNum"),
            "MOQUI_CI_REPOSITORY": path,
            "MOQUI_CI_MERGE_REQUEST": field(run, "mergeRequestId"),
        },
        "limits": {
            "maxLogBatchBytes": super::number_setting("max-log-batch-bytes", DEFAULT_MAX_JSON),
        },
    }))
}

/// `POST jobs/<id>/steps/<seq>`: a step started, or ended with its exit code.
fn post_step(
    entity: &EntityFacade,
    job: &EntityValue,
    seq: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, Refused> {
    let _rows = crate::rows_guard();
    let seq: i64 = seq.parse().map_err(|_| Refused::new(400, "the step is a number"))?;
    let status = match body["status"].as_str() {
        Some("running") => "running",
        Some("success") => "success",
        Some("failure") => "failure",
        Some("skipped") => "skipped",
        _ => return Err(Refused::new(400, "status is running, success, failure or skipped")),
    };
    let Some(mut step) = entity
        .find(JOB_STEP)?
        .eq("jobRunId", Value::string(field(job, "jobRunId").unwrap_or_default()))
        .eq("stepSeq", int(seq))
        .one()?
    else {
        return Err(Refused::new(404, format!("the job has no step {seq}")));
    };
    step.set("statusId", Value::string(status));
    if status == "running" {
        step.set("startedDate", moqui_core::user::now_value());
    } else {
        step.set("exitCode", body["exitCode"].as_i64().map_or(Value::Null, int));
        step.set("finishedDate", moqui_core::user::now_value());
    }
    step.update(entity)?;
    Ok(json!({ "stepSeq": seq, "status": status }))
}

/// `PUT jobs/<id>/artifacts?kind=&name=&path=&key=&retentionDays=`: the body is the content.
fn put_artifact(
    entity: &EntityFacade,
    request: &RunnerHttpRequest,
    run: &EntityValue,
    job: &EntityValue,
    body: &mut dyn Read,
) -> Result<serde_json::Value, Refused> {
    let Some(kind) = query_value(&request.query, "kind").and_then(|kind| Kind::parse(&kind)) else {
        return Err(Refused::new(400, "kind is artifact, report or cache"));
    };
    if request.content_length.is_some_and(|length| length > kind.max_bytes()) {
        return Err(Refused::new(
            413,
            format!("the {} is over {} bytes", kind.name(), kind.max_bytes()),
        ));
    }
    let key = query_value(&request.query, "key");
    let name = query_value(&request.query, "name")
        .or_else(|| key.clone())
        .filter(|name| name.len() <= 200 && !name.contains(['/', '\\', '\0']))
        .ok_or_else(|| Refused::new(400, "name is required: at most 200 characters, no slash"))?;
    let path = query_value(&request.query, "path").filter(|path| path.len() <= 500);
    let upload = Upload {
        kind,
        name: &name,
        path: path.as_deref(),
        cache_key: key.as_deref().filter(|_| kind == Kind::Cache),
        retention_days: query_value(&request.query, "retentionDays").and_then(|days| days.parse().ok()),
    };
    match artifact::store(entity, run, job, &upload, body) {
        Ok(id) => Ok(json!({ "artifactId": id, "stored": id.is_some() })),
        Err(err) => Err(Refused::new(422, err.message())),
    }
}

/// `GET jobs/<id>/caches?key=`: the cache archive, streamed; 404 when there is none.
fn get_cache(
    entity: &EntityFacade,
    request: &RunnerHttpRequest,
    run: &EntityValue,
    out: &mut dyn HttpResponder,
) -> Result<(), Refused> {
    let key = query_value(&request.query, "key").ok_or_else(|| Refused::new(400, "key is required"))?;
    let found = artifact::find_cache(entity, run, &key)?;
    let Some((row, (blobs, info))) = found
        .map(|row| artifact::content(entity, &row).map(|content| content.map(|content| (row, content))))
        .transpose()?
        .flatten()
    else {
        return Err(Refused::new(404, format!("no cache for the key {key}")));
    };
    artifact::touch_cache(entity, &row);
    let headers = [
        ("Content-Type", "application/gzip".to_string()),
        ("Content-Length", info.size.to_string()),
        ("Cache-Control", "no-store".to_string()),
    ];
    if out.head(200, &headers).is_err() {
        return Ok(());
    }
    let _rows = crate::rows_guard();
    let mut offset = 0;
    while offset < info.size {
        let Ok(bytes) = blobs.read_range(&info, offset, 1024 * 1024) else {
            break;
        };
        if bytes.is_empty() || out.body().write_all(&bytes).is_err() {
            break;
        }
        offset += bytes.len() as u64;
    }
    let _ = out.body().flush();
    Ok(())
}

/// `POST jobs/<id>/result`: the job ended. Its token is dead after this.
fn post_result(
    entity: &EntityFacade,
    job: &EntityValue,
    body: &serde_json::Value,
) -> Result<serde_json::Value, Refused> {
    let status = match body["status"].as_str() {
        Some("success") => "success",
        Some("failure") => "failure",
        Some("error") => "error",
        _ => return Err(Refused::new(400, "status is success, failure or error")),
    };
    let message: Option<String> = body["message"]
        .as_str()
        .map(|message| message.chars().take(500).collect());
    let job_run_id = field(job, "jobRunId").unwrap_or_default();
    engine::finish_job(
        entity,
        &job_run_id,
        &Outcome {
            status,
            exit_code: body["exitCode"].as_i64(),
            message,
        },
    )?;
    Ok(json!({ "jobRunId": job_run_id, "status": status }))
}

fn job_request(
    entity: &EntityFacade,
    request: &RunnerHttpRequest,
    bearer: &str,
    rest: &str,
    body: &mut dyn Read,
    out: &mut dyn HttpResponder,
) -> Result<Option<serde_json::Value>, Refused> {
    let (job_run_id, action) = rest
        .split_once('/')
        .ok_or_else(|| Refused::new(404, "not a runner endpoint"))?;
    let (job, run) = job_of(entity, bearer, job_run_id)?;
    let method = request.method.as_str();
    match (method, action) {
        ("GET", "job") => get_job(entity, &run, &job).map(Some),
        ("POST", "log") => {
            let body = read_json(request, body)?;
            let text = body["text"].as_str().unwrap_or_default();
            let next = super::log::append(entity, &job, body["stepSeq"].as_i64(), text)?;
            Ok(Some(json!({ "nextChunkSeq": next })))
        }
        ("POST", "result") => {
            let body = read_json(request, body)?;
            post_result(entity, &job, &body).map(Some)
        }
        ("PUT", "artifacts") => put_artifact(entity, request, &run, &job, body).map(Some),
        ("GET", "caches") => get_cache(entity, request, &run, out).map(|()| None),
        ("POST", step) if step.starts_with("steps/") => {
            let body = read_json(request, body)?;
            post_step(entity, &job, step.trim_start_matches("steps/"), &body).map(Some)
        }
        _ => Err(Refused::new(404, "not a runner endpoint")),
    }
}

/// Serve one request of the runner REST, in an entered context. A refusal is its status with `{"error": ...}`.
pub fn serve(request: &RunnerHttpRequest, body: &mut dyn Read, out: &mut dyn HttpResponder) {
    let outcome = (|| -> Result<Option<serde_json::Value>, Refused> {
        let factory = ExecutionContext::with_current(|ec| Arc::clone(ec.factory()))
            .ok_or_else(|| Refused::new(503, "no execution context"))?;
        let entity = factory.entity().ok_or_else(|| Refused::new(503, "no entity facade"))?;
        let Some(bearer) = token::from_authorization(request.authorization.as_deref()) else {
            return Err(Refused::new(
                401,
                "a runner request carries its token in Authorization: Bearer",
            ));
        };
        if !request.secure {
            return Err(Refused::new(403, "a token over plain HTTP is refused; use HTTPS"));
        }
        let path = request.path.trim_matches('/');
        if let Some(rest) = path.strip_prefix("jobs/") {
            return job_request(&entity, request, &bearer, rest, body, out);
        }
        match (request.method.as_str(), path) {
            ("POST", "register") => {
                let body = read_json(request, body)?;
                let name = body["name"].as_str().unwrap_or("runner");
                match super::registered::register(&entity, &bearer, name, body["platform"].as_str())? {
                    Some((runner_id, runner_token)) => {
                        Ok(Some(json!({ "runnerId": runner_id, "token": runner_token })))
                    }
                    None => {
                        moqui_core::web::note_auth_failure();
                        Err(Refused::new(401, "the pool token is not valid"))
                    }
                }
            }
            ("POST", "claim") => {
                if over_rate(&format!("runner:{}", token::hash(&bearer))) {
                    return Err(Refused::new(429, "too many requests of this runner in a minute"));
                }
                match super::registered::claim(&entity, &bearer) {
                    Ok(Some(claimed)) => Ok(Some(json!({
                        "jobRunId": claimed.job_run_id,
                        "token": claimed.job_token,
                    }))),
                    Ok(None) => Ok(Some(json!({}))),
                    Err(err) => {
                        moqui_core::web::note_auth_failure();
                        Err(Refused::new(401, err.message()))
                    }
                }
            }
            _ => Err(Refused::new(404, "not a runner endpoint")),
        }
    })();
    match outcome {
        Ok(Some(body)) => respond(out, 200, &body),
        Ok(None) => {}
        Err(Refused(status, message)) => {
            // what a service left in the message facade is this request's, and is answered here
            ExecutionContext::with_current(|ec| ec.message().clear_errors());
            respond(out, status, &json!({ "error": message }));
        }
    }
}

/// A response kept in memory: for a caller in this process (a test's runner).
#[derive(Default)]
pub struct MemoryResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponder for MemoryResponse {
    fn head(&mut self, status: u16, headers: &[(&str, String)]) -> std::io::Result<()> {
        self.status = status;
        self.headers = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        Ok(())
    }

    fn body(&mut self) -> &mut dyn Write {
        &mut self.body
    }
}
