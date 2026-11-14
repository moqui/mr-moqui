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

//! Planning a run: which pipelines of a file an event starts, and the jobs of one, a job for each combination of a
//! matrix (docs/interfaces/Pipelines.md "Jobs"). A planned job carries its `jobSpec`, the JSON its runner is given.

use serde_json::json;

use super::file::{Event, Job, Pipeline};
use crate::rules::glob;

/// One job of a run.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedJob {
    pub name: String,
    /// The matrix values, joined by a comma and a space; empty with no matrix.
    pub matrix_key: String,
    /// Job names: every job of each name must succeed first.
    pub needs: Vec<String>,
    pub pool: Option<String>,
    pub image: String,
    pub timeout_minutes: u32,
    /// The commands of its steps, in order, with their names.
    pub steps: Vec<(Option<String>, String)>,
    pub spec: serde_json::Value,
}

/// Whether `pipeline` runs for `event` on `ref_name` (a full ref: the branch of a push, the target branch of a merge
/// request, the tag). A manual, schedule or deploy run is asked for by name, so only the event is checked.
pub fn applies(pipeline: &Pipeline, event: Event, ref_name: &str) -> bool {
    if !pipeline.events.contains(&event) {
        return false;
    }
    let any = |patterns: &[String], name: &str| patterns.is_empty() || patterns.iter().any(|p| glob(p, name));
    match event {
        Event::Push | Event::MergeRequest => ref_name
            .strip_prefix("refs/heads/")
            .is_some_and(|branch| any(&pipeline.branches, branch)),
        Event::Tag => ref_name
            .strip_prefix("refs/tags/")
            .is_some_and(|tag| any(&pipeline.tags, tag)),
        Event::Schedule | Event::Manual | Event::Deploy => true,
    }
}

/// `${matrix.NAME}` in `text` replaced by the combination's values; a name that is not an axis stays as written.
pub fn expand(text: &str, matrix: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (name, value) in matrix {
        out = out.replace(&format!("${{matrix.{name}}}"), value);
    }
    out
}

/// Every combination of the axes' values, in the order of the file; one empty combination with no matrix.
fn combinations(job: &Job) -> Vec<Vec<(String, String)>> {
    let mut all: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for axis in &job.matrix {
        let mut next = Vec::with_capacity(all.len() * axis.values.len());
        for combination in &all {
            for value in &axis.values {
                let mut with = combination.clone();
                with.push((axis.name.clone(), value.clone()));
                next.push(with);
            }
        }
        all = next;
    }
    all
}

fn env_json(pairs: &[(String, String)], matrix: &[(String, String)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(name, value)| (name.clone(), json!(expand(value, matrix))))
        .collect()
}

/// The jobs of a run of `pipeline`.
pub fn plan(pipeline: &Pipeline) -> Vec<PlannedJob> {
    let mut planned = Vec::new();
    for job in &pipeline.jobs {
        for matrix in combinations(job) {
            let values: Vec<&str> = matrix.iter().map(|(_, value)| value.as_str()).collect();
            let matrix_key = values.join(", ");
            let image = expand(&job.image, &matrix);
            // the pipeline's variables, then the job's, then the matrix values
            let mut env = env_json(&pipeline.env, &matrix);
            env.extend(env_json(&job.env, &matrix));
            for (name, value) in &matrix {
                let variable: String = name
                    .chars()
                    .map(|ch| {
                        if ch.is_ascii_alphanumeric() {
                            ch.to_ascii_uppercase()
                        } else {
                            '_'
                        }
                    })
                    .collect();
                env.insert(format!("MATRIX_{variable}"), json!(value));
            }
            let steps: Vec<(Option<String>, String)> = job
                .steps
                .iter()
                .map(|step| (step.name.clone(), expand(&step.run, &matrix)))
                .collect();
            let step_specs: Vec<serde_json::Value> = job
                .steps
                .iter()
                .zip(&steps)
                .enumerate()
                .map(|(index, (step, (_, run)))| {
                    json!({
                        "seq": index + 1,
                        "name": step.name,
                        "run": run,
                        "timeoutMinutes": step.timeout_minutes,
                        "continueOnError": step.continue_on_error,
                    })
                })
                .collect();
            let spec = json!({
                "pipeline": pipeline.name,
                "job": job.name,
                "matrixKey": matrix_key,
                "image": image,
                "fetchDepth": job.fetch_depth,
                "timeoutMinutes": job.timeout_minutes,
                "environment": job.environment,
                "env": env,
                "secrets": job.secrets.iter().map(|ask| json!({"name": ask.name, "env": ask.env})).collect::<Vec<_>>(),
                "services": job.services.iter().map(|service| json!({
                    "name": service.name,
                    "image": expand(&service.image, &matrix),
                    "env": env_json(&service.env, &matrix),
                })).collect::<Vec<_>>(),
                "caches": job.caches.iter().map(|cache| json!({
                    "key": expand(&cache.key, &matrix),
                    "path": cache.path,
                })).collect::<Vec<_>>(),
                "steps": step_specs,
                "artifacts": job.artifacts.iter().map(|artifact| json!({
                    "name": expand(&artifact.name, &matrix),
                    "path": artifact.path,
                    "retentionDays": artifact.retention_days,
                })).collect::<Vec<_>>(),
                "reports": job.reports,
            });
            planned.push(PlannedJob {
                name: job.name.clone(),
                matrix_key,
                needs: job.needs.clone(),
                pool: job.pool.clone(),
                image,
                timeout_minutes: job.timeout_minutes,
                steps,
                spec,
            });
        }
    }
    planned
}

/// The commit status context of a job: `ci/<pipeline>/<job>`, with the matrix values in brackets.
pub fn context(pipeline: &str, job: &str, matrix_key: &str) -> String {
    if matrix_key.is_empty() {
        format!("ci/{pipeline}/{job}")
    } else {
        format!("ci/{pipeline}/{job} ({matrix_key})")
    }
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
