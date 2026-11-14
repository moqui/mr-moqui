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

//! Run locally (plans/design-dev-aide.md 21.5, question 20): one job of the working tree's `ci.xml`, read with the CI
//! engine's own parser and planner (`moqui_git::ci::file`, `moqui_git::ci::plan`), run in a host session of the
//! Terminal in the working tree. The job's script steps run in order as a runner runs them (`sh -c` each, ending at the
//! first that fails unless it continues on error), with the pipeline's and the job's variables, a matrix job's one
//! combination, and the variables every job has; a secret it asks for is unset, as for a run that is not given it. It is
//! this machine and the session's OS user, not the job's image, which the page and the script both say (question 20,
//! as proposed: a run in a local container waits for Dogfood F8's `docker` driver). The steps go to a script under the
//! runtime's `tmp`, and the session is opened with `sh <script>` typed into its shell, so what runs is there to read.

use std::path::{Path, PathBuf};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde::Serialize;
use serde_json::json;

use crate::tier::Tier;
use crate::vcs::git_root;
use crate::workspace::{json_value, text, Context};

/// The CI file at a working tree's root (docs/interfaces/Pipelines.md).
pub const CI_FILE: &str = "ci.xml";

/// What the page and the script say of every local run (question 20).
pub const NOT_THE_IMAGE: &str =
    "Run locally runs the job's steps on this machine, as the server's OS user, in a shell \
    of the Terminal: not in the job's image, with none of its secrets, caches, services or artifacts.";

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_local_jobs), get_local_jobs)?;
    registry.service_function(moqui_core::service_location!(start_local_run), start_local_run)
}

/// A step of a job, as a runner runs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStep {
    pub name: Option<String>,
    pub run: String,
    pub continue_on_error: bool,
}

/// One job of a pipeline, one combination of its matrix, planned as the engine plans it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalJob {
    pub pipeline: String,
    pub job: String,
    pub matrix_key: String,
    pub image: String,
    /// The pipeline's variables, the job's and the matrix values, in that order.
    pub env: Vec<(String, String)>,
    pub steps: Vec<LocalStep>,
    /// The variables of the secrets it asks for: unset in a local run.
    pub secrets: Vec<String>,
}

/// The jobs of a `ci.xml` text: every job of every pipeline, a matrix job once per combination.
///
/// # Errors
///
/// The file's error with its line, as `moqui check` and a run say it.
pub fn jobs_of(text: &str) -> Result<Vec<LocalJob>, String> {
    let file = moqui_git::ci::file::parse(text, &moqui_git::ci::file::Limits::default())
        .map_err(|err| format!("{CI_FILE} line {}: {}", err.line, err.message))?;
    let mut jobs = Vec::new();
    for pipeline in &file.pipelines {
        for planned in moqui_git::ci::plan::plan(pipeline) {
            let spec = &planned.spec;
            let env = spec["env"]
                .as_object()
                .map(|env| {
                    env.iter()
                        .map(|(name, value)| (name.clone(), value.as_str().unwrap_or("").to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let steps = spec["steps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|step| LocalStep {
                    name: step["name"].as_str().map(str::to_string),
                    run: step["run"].as_str().unwrap_or("").to_string(),
                    continue_on_error: step["continueOnError"].as_bool().unwrap_or(false),
                })
                .collect();
            let secrets = spec["secrets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|secret| secret["env"].as_str().or_else(|| secret["name"].as_str()))
                .map(str::to_string)
                .collect();
            jobs.push(LocalJob {
                pipeline: pipeline.name.clone(),
                job: planned.name.clone(),
                matrix_key: planned.matrix_key.clone(),
                image: planned.image.clone(),
                env,
                steps,
                secrets,
            });
        }
    }
    Ok(jobs)
}

/// The job to run: `pipeline`'s `job`, the combination `matrix_key` of a matrix job (the first when it names none).
#[must_use]
pub fn pick<'a>(jobs: &'a [LocalJob], pipeline: &str, job: &str, matrix_key: &str) -> Option<&'a LocalJob> {
    let mut of_job = jobs
        .iter()
        .filter(|found| found.pipeline == pipeline && found.job == job);
    if matrix_key.trim().is_empty() {
        of_job.next()
    } else {
        of_job.find(|found| found.matrix_key == matrix_key.trim())
    }
}

/// `text` quoted for a POSIX shell.
#[must_use]
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Whether `name` is an environment variable's name a shell takes.
fn env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

/// The script of a local run of `job` in `dir`: its variables and CI's own exported, its secrets' variables unset, and
/// each step run with `sh -c` in order, ending at the first that fails unless it continues on error.
#[must_use]
pub fn script(job: &LocalJob, dir: &Path) -> String {
    let title = if job.matrix_key.is_empty() {
        format!("{}/{}", job.pipeline, job.job)
    } else {
        format!("{}/{} ({})", job.pipeline, job.job, job.matrix_key)
    };
    let one_line = |text: &str| text.replace(['\n', '\r'], " ");
    let mut out = String::from("#!/bin/sh\n");
    out.push_str(&format!(
        "# Run locally by the Dev AIDE (plans/design-dev-aide.md 21.5): the job {} of {CI_FILE}.\n",
        one_line(&title)
    ));
    out.push_str(&format!(
        "# This machine and this shell's user, not the job's image ({}); its secrets are not set.\n",
        one_line(&job.image)
    ));
    // the script is gone when it ends, however it ends
    out.push_str("trap 'rm -f -- \"$0\"' EXIT\n");
    out.push_str(&format!("cd {} || exit 1\n", quoted(&dir.display().to_string())));
    let mut env: Vec<(String, String)> = vec![
        ("CI".into(), "true".into()),
        ("MOQUI_CI".into(), "true".into()),
        ("MOQUI_CI_LOCAL".into(), "true".into()),
        ("MOQUI_CI_EVENT".into(), "manual".into()),
        ("MOQUI_CI_PIPELINE".into(), job.pipeline.clone()),
        ("MOQUI_CI_JOB".into(), job.job.clone()),
    ];
    env.extend(job.env.iter().cloned());
    for (name, value) in env.iter().filter(|(name, _)| env_name(name)) {
        out.push_str(&format!("export {name}={}\n", quoted(value)));
    }
    for secret in job.secrets.iter().filter(|name| env_name(name)) {
        out.push_str(&format!("unset {secret}\n"));
    }
    out.push_str(&format!(
        "printf '%s\\n' {}\n",
        quoted(&format!("== Run locally: {} (not the job's image)", one_line(&title)))
    ));
    out.push_str("moqui_status=0\n");
    for (index, step) in job.steps.iter().enumerate() {
        let name = step.name.as_deref().map_or_else(|| one_line(&step.run), one_line);
        let shown = format!("== Step {}: {}", index + 1, name.chars().take(120).collect::<String>());
        out.push_str("if [ \"$moqui_status\" -eq 0 ]; then\n");
        out.push_str(&format!("  printf '\\n%s\\n' {}\n", quoted(&shown)));
        out.push_str(&format!("  sh -c {}\n", quoted(&step.run)));
        out.push_str("  moqui_step=$?\n");
        if step.continue_on_error {
            out.push_str(
                "  [ \"$moqui_step\" -eq 0 ] || printf '%s\\n' \"== failed ($moqui_step), and continues on error\"\n",
            );
        } else {
            out.push_str(
                "  [ \"$moqui_step\" -eq 0 ] || { printf '%s\\n' \"== failed ($moqui_step)\"; moqui_status=$moqui_step; }\n",
            );
        }
        out.push_str("fi\n");
    }
    out.push_str(
        "if [ \"$moqui_status\" -eq 0 ]; then printf '\\n%s\\n' '== The job succeeded'; \
         else printf '\\n%s\\n' \"== The job failed ($moqui_status)\"; fi\n",
    );
    out
}

/// The working tree's `ci.xml` jobs, or why there are none.
fn read_jobs(root: &Path) -> Result<Vec<LocalJob>, String> {
    let path = root.join(CI_FILE);
    let text = std::fs::read_to_string(&path).map_err(|_| format!("This working tree has no {CI_FILE} at its root"))?;
    jobs_of(&text)
}

/// `get#LocalJobs`: the jobs of the working tree's `ci.xml`, each with its matrix combinations, steps, variables and
/// the secrets it asks for; or the error of the file.
fn get_local_jobs(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::View)?;
    let root = git_root(&context, parameters)?;
    let mut out = Parameters::new();
    out.insert("note".into(), Value::string(NOT_THE_IMAGE));
    match read_jobs(&root.path) {
        Ok(jobs) => {
            // one entry a job, with its combinations
            let mut listed: Vec<serde_json::Value> = Vec::new();
            for job in &jobs {
                let same = listed
                    .iter_mut()
                    .find(|have| have["pipeline"] == job.pipeline.as_str() && have["job"] == job.job.as_str());
                match same {
                    Some(have) => {
                        if let Some(keys) = have["matrixKeys"].as_array_mut() {
                            keys.push(json!(job.matrix_key));
                        }
                        if let Some(combinations) = have["combinations"].as_array_mut() {
                            combinations.push(combination(job, &root.path));
                        }
                    }
                    None => {
                        let mut entry = serde_json::to_value(job).unwrap_or_default();
                        entry["matrixKeys"] = if job.matrix_key.is_empty() {
                            json!([])
                        } else {
                            json!([job.matrix_key])
                        };
                        // each combination as it runs (its steps and variables with the matrix values in them)
                        entry["combinations"] = json!([combination(job, &root.path)]);
                        listed.push(entry);
                    }
                }
            }
            out.insert("jobs".into(), json_value(&listed));
        }
        Err(error) => {
            out.insert("jobs".into(), Value::List(Vec::new()));
            out.insert("error".into(), Value::string(error));
        }
    }
    Ok(out)
}

/// A combination as the page shows it before it runs: the job, the script it runs whole, and that script's digest,
/// which `start#LocalRun` holds the script to.
fn combination(job: &LocalJob, dir: &Path) -> serde_json::Value {
    let text = script(job, dir);
    let mut value = serde_json::to_value(job).unwrap_or_default();
    value["digest"] = json!(digest(&text));
    value["script"] = json!(text);
    value
}

/// The digest of a script: what the user saw is what runs.
#[must_use]
pub fn digest(script: &str) -> String {
    crate::workspace::sha256(script.as_bytes())
}

/// Why a local run does not start: the script rebuilt now is not the one the user saw (`ci.xml` or the job changed
/// between the page and Run).
#[must_use]
pub fn digest_refusal(script: &str, seen: &str) -> Option<String> {
    (seen.trim().is_empty() || digest(script) != seen.trim())
        .then(|| format!("{CI_FILE} changed since the job was shown: show it again, then run what it shows"))
}

/// Where a local run's script goes: the runtime's `tmp`, which no root reaches, in a directory of the server's OS
/// user alone (0700), the script readable by it alone (0600).
fn script_path(runtime: &Path) -> Result<PathBuf, LibError> {
    let dir = runtime.join("tmp").join("dev-aide-run");
    std::fs::create_dir_all(&dir).map_err(|err| LibError::new(format!("{}: {err}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|err| LibError::new(format!("{}: {err}", dir.display())))?;
    }
    Ok(dir.join(format!("{}.sh", moqui_core::user::random_string(12)?)))
}

/// Write a script readable by the server's OS user alone, never over a file that is there.
fn write_script(path: &Path, text: &str) -> Result<(), LibError> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|err| LibError::new(format!("{}: {err}", path.display())))?;
    file.write_all(text.as_bytes())
        .map_err(|err| LibError::new(format!("{}: {err}", path.display())))
}

/// `start#LocalRun`: the job's script written, and a new host session of the Terminal opened in the working tree with
/// `sh <script>` typed into its shell. The Terminal decides who may open one (`TERMINAL_HOST`, a recent sign-in from
/// elsewhere); its refusal comes back as `reasonCode`.
fn start_local_run(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let context = Context::of(ec, Tier::Extend)?;
    // a program the user runs: never from an agent's fence or a sim, never from a header login
    crate::workspace::unfenced(moqui_core::entity::fence::Effect::Process, "start#LocalRun")?;
    if moqui_core::screen::gate::header_authenticated_request() {
        return Err(LibError::new(
            "Run locally is a person's, from a session: not a request logged in by a header",
        ));
    }
    let root = git_root(&context, parameters)?;
    let jobs = read_jobs(&root.path).map_err(LibError::new)?;
    let (pipeline, name, matrix_key) = (
        text(parameters, "pipeline"),
        text(parameters, "job"),
        text(parameters, "matrixKey"),
    );
    let job = pick(&jobs, &pipeline, &name, &matrix_key).ok_or_else(|| {
        LibError::new(format!(
            "{CI_FILE} has no job {pipeline}/{name}{}",
            if matrix_key.is_empty() {
                String::new()
            } else {
                format!(" ({matrix_key})")
            }
        ))
    })?;
    let body = script(job, &root.path);
    if let Some(why) = digest_refusal(&body, &text(parameters, "digest")) {
        return Err(LibError::new(why));
    }
    let path = script_path(context.factory.runtime_dir())?;
    write_script(&path, &body)?;
    let title = if job.matrix_key.is_empty() {
        format!("Run locally: {}/{}", job.pipeline, job.job)
    } else {
        format!("Run locally: {}/{} ({})", job.pipeline, job.job, job.matrix_key)
    };
    let input = format!("sh {}\n", quoted(&path.display().to_string()));
    let opened = moqui_core::service::sync(
        "org.moqui.terminal.TerminalServices.open#TerminalSession",
        &[
            ("kind", "host".to_string()),
            // the server user's shell: a profile the conf or the user names never stands in for it
            ("profileName", "shell".to_string()),
            ("directory", root.path.display().to_string()),
            ("title", title),
            ("input", input),
            ("cols", "120".to_string()),
            ("rows", "32".to_string()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), Value::string(value)))
        .collect(),
    )?;
    crate::vcs::audit_git(
        &context,
        root,
        "run-locally",
        json!({ "pipeline": job.pipeline, "job": job.job, "matrixKey": job.matrix_key,
            "script": path.display().to_string() }),
    );
    let mut out = Parameters::new();
    for key in ["sessionId", "reasonCode"] {
        if let Some(value) = opened.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    out.insert("script".into(), Value::string(path.display().to_string()));
    out.insert("note".into(), Value::string(NOT_THE_IMAGE));
    Ok(out)
}

#[cfg(test)]
#[path = "local_run_tests.rs"]
mod tests;
