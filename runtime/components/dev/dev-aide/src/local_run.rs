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
//! this machine and the session's OS user, not the job's image, which the page and the script both say (question 20).
//! The steps go to a script under the runtime's `tmp`, and the session is opened with `sh <script>` typed into its
//! shell, so what runs is there to read.
//!
//! Where Docker or Podman answers on this machine, a job also runs in its image: a container of the CI driver's
//! (`moqui_git::ci::driver::docker::start_local`, kept in as a pool's job is), the working tree mounted at the working
//! directory and written as its owner, the same steps and variables, its secrets unset, its log read by the checks
//! dialog until it ends (`get#LocalRun`) and the container removed by Remove or the next run.

use std::path::{Path, PathBuf};

use moqui_core::context::{Error, ExecutionContext};
use moqui_core::native::{NativeRegistry, Parameters, ServiceInfo};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::Value;
use serde::Serialize;
use serde_json::json;

use moqui_git::ci::driver::docker::{ContainerInfo, DockerHost, Engine, Hardening, LocalRun};

use crate::tier::Tier;
use crate::vcs::git_root;
use crate::workspace::{json_value, text, Context};

/// The CI file at a working tree's root (docs/interfaces/Pipelines.md).
pub const CI_FILE: &str = "ci.xml";

/// What the page and the script say of every local run (question 20).
pub const NOT_THE_IMAGE: &str =
    "Run locally runs the job's steps on this machine, as the server's OS user, in a shell \
    of the Terminal: not in the job's image, with none of its secrets, caches, services or artifacts.";

/// What the page says of a run in the job's image.
pub const IN_THE_IMAGE: &str = "In the job's image runs its steps in a container of that image on this machine, the \
    working tree mounted at the working directory and written as its owner: with none of the job's secrets, caches, \
    services or artifacts.";

/// The labels of a local run's container: whose it is (with the CI driver's server label, which says this server made
/// it) and its working tree.
pub const LABEL_USER: &str = "moqui.devaide.user";
pub const LABEL_ROOT: &str = "moqui.devaide.root";
/// A local run's container name starts so, then this server's name part and a random tag.
pub const NAME_PREFIX: &str = "moqui-devaide-";

pub(crate) fn register(registry: &mut NativeRegistry) -> Result<(), Error> {
    registry.service_function(moqui_core::service_location!(get_local_jobs), get_local_jobs)?;
    registry.service_function(moqui_core::service_location!(start_local_run), start_local_run)?;
    registry.service_function(moqui_core::service_location!(get_local_run), get_local_run)?;
    registry.service_function(moqui_core::service_location!(stop_local_run), stop_local_run)
}

/// Where a local run runs (question 20).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// A host session of the Terminal: this machine and its shell's user.
    Host,
    /// A container of the job's image, the working tree mounted at the working directory.
    Image,
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

/// The script of a local run of `job` in `dir` on this machine ([`script_in`] of [`Place::Host`]).
#[must_use]
pub fn script(job: &LocalJob, dir: &Path) -> String {
    script_in(job, dir, Place::Host)
}

/// The script of a local run of `job` in `dir`, at `place`: its variables and CI's own exported, its secrets' variables
/// unset, and each step run with `sh -c` in order, ending at the first that fails unless it continues on error. On the
/// host it is a file that removes itself when it ends; in the image it is the container's command.
#[must_use]
pub fn script_in(job: &LocalJob, dir: &Path, place: Place) -> String {
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
    match place {
        Place::Host => {
            out.push_str(&format!(
                "# This machine and this shell's user, not the job's image ({}); its secrets are not set.\n",
                one_line(&job.image)
            ));
            // the script is gone when it ends, however it ends
            out.push_str("trap 'rm -f -- \"$0\"' EXIT\n");
        }
        Place::Image => out.push_str(&format!(
            "# In the job's image ({}), the working tree mounted here; its secrets are not set.\n",
            one_line(&job.image)
        )),
    }
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
    let banner = match place {
        Place::Host => format!("== Run locally: {} (not the job's image)", one_line(&title)),
        Place::Image => format!(
            "== Run locally: {} in the job's image {}",
            one_line(&title),
            one_line(&job.image)
        ),
    };
    out.push_str(&format!("printf '%s\\n' {}\n", quoted(&banner)));
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
    // a container's exit code is the job's
    if place == Place::Image {
        out.push_str("exit \"$moqui_status\"\n");
    }
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
    let engine = local_engine();
    out.insert(
        "container".into(),
        json_value(&match &engine {
            Some(engine) => {
                // what refuses a run in the image of this root, said before it is offered
                let may = moqui_core::user::has_permission(HOST_PERMISSION).unwrap_or(false);
                let refusal = image_refusal(
                    &root.label,
                    &root.path,
                    root.read_only,
                    context.factory.runtime_dir(),
                    may,
                );
                json!({ "available": true, "engine": engine.name(), "socket": engine.socket.display().to_string(),
                "note": IN_THE_IMAGE, "refusal": refusal })
            }
            None => json!({ "available": false }),
        }),
    );
    let workdir = Hardening::from_conf().workdir;
    let limit = ImageLimits::of(&context.settings).timeout.as_secs();
    let combination = |job: &LocalJob, dir: &Path| combination(job, dir, Path::new(&workdir), limit);
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

/// A combination as the page shows it before it runs: the job, the script it runs whole on this machine and in its
/// image (in `workdir`), and each script's digest, which `start#LocalRun` holds the script to.
fn combination(job: &LocalJob, dir: &Path, workdir: &Path, limit_secs: u64) -> serde_json::Value {
    let text = script(job, dir);
    let in_image = with_time_limit(&script_in(job, workdir, Place::Image), limit_secs);
    let mut value = serde_json::to_value(job).unwrap_or_default();
    value["digest"] = json!(digest(&text));
    value["script"] = json!(text);
    value["imageDigest"] = json!(digest(&in_image));
    value["imageScript"] = json!(in_image);
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
    // a run in the image writes the tree: a root the tier may write, as a git command that changes it
    let root = if text(parameters, "where") == "image" {
        crate::vcs::git_write_root(&context, parameters, "runs a job in its image")?
    } else {
        git_root(&context, parameters)?
    };
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
    if text(parameters, "where") == "image" {
        return start_in_image(&context, root, job, &text(parameters, "digest"));
    }
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

// ---- in the job's image ----

/// A container engine on this machine, as Run locally reaches it.
pub(crate) struct LocalEngine {
    pub host: DockerHost,
    pub socket: PathBuf,
    pub engine: Engine,
}

impl LocalEngine {
    fn name(&self) -> &'static str {
        match self.engine {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
        }
    }
}

/// The sockets of a container engine on this machine, in the order they are tried: `DOCKER_HOST` when it names a Unix
/// socket, Docker's, then a rootless Podman's (in `XDG_RUNTIME_DIR`, else `/run/user/<uid>` of the tree's owner `uid`)
/// and a rootful Podman's.
#[must_use]
pub fn engine_sockets(docker_host: Option<&str>, runtime_dir: Option<&str>, uid: Option<u32>) -> Vec<PathBuf> {
    let mut sockets = Vec::new();
    if let Some(socket) = docker_host.and_then(|host| host.trim().strip_prefix("unix://")) {
        sockets.push(PathBuf::from(socket));
    }
    sockets.push(PathBuf::from("/var/run/docker.sock"));
    if let Some(dir) = runtime_dir.map(str::trim).filter(|dir| dir.starts_with('/')) {
        sockets.push(Path::new(dir).join("podman/podman.sock"));
    }
    if let Some(uid) = uid {
        sockets.push(PathBuf::from(format!("/run/user/{uid}/podman/podman.sock")));
    }
    sockets.push(PathBuf::from("/run/podman/podman.sock"));
    sockets.dedup();
    sockets
}

/// The owner of a path, `uid:gid`, unless it is root: the user a run in the image writes the working tree as.
fn owner_of(path: &Path) -> Option<(u32, u32)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        (meta.uid() != 0).then(|| (meta.uid(), meta.gid()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// The container engine that answers on this machine, if any: the first of [`engine_sockets`] that is there and
/// answers its version.
pub(crate) fn local_engine_for(tree: Option<&Path>) -> Option<LocalEngine> {
    let docker_host = std::env::var("DOCKER_HOST").ok();
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").ok();
    let uid = tree.and_then(owner_of).map(|(uid, _)| uid);
    for socket in engine_sockets(docker_host.as_deref(), runtime_dir.as_deref(), uid) {
        if !socket.exists() {
            continue;
        }
        let host = DockerHost::unix(&socket);
        match host.engine() {
            Ok(engine) => return Some(LocalEngine { host, socket, engine }),
            Err(err) => tracing::debug!("Dev AIDE: no container engine at {}: {err}", socket.display()),
        }
    }
    None
}

fn local_engine() -> Option<LocalEngine> {
    local_engine_for(None)
}

/// What becomes of a run in the image, by its container's name, while this server follows it: being made (its image
/// pulled), running, ended, or failed before it started, with whose it is and whether Remove asked for it to stop.
/// Kept from `start#LocalRun` until `stop#LocalRun`; the engine says the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Starting,
    Running,
    Ended,
    TimedOut,
    Failed(String),
}

struct Start {
    user: String,
    phase: Phase,
    cancelled: bool,
}

fn starts() -> &'static std::sync::Mutex<std::collections::BTreeMap<String, Start>> {
    static STARTS: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<String, Start>>> =
        std::sync::OnceLock::new();
    STARTS.get_or_init(Default::default)
}

/// Set the phase of run `name`, and answer whether Remove asked for it to stop.
fn set_phase(name: &str, phase: Phase) -> bool {
    let mut starts = moqui_lib::sync::lock(starts());
    match starts.get_mut(name) {
        Some(start) => {
            start.phase = phase;
            start.cancelled
        }
        None => true,
    }
}

/// A local run's container name: [`NAME_PREFIX`], then letters, digits and `-` alone (what [`start_in_image`] makes).
#[must_use]
pub fn valid_run_name(name: &str) -> bool {
    name.strip_prefix(NAME_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && name.len() <= 128
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
    })
}

/// The limits of a run in the image (the conf's `dev-aide` `local-run-*` attributes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageLimits {
    pub memory_mb: i64,
    pub pids: i64,
    pub timeout: std::time::Duration,
    pub per_user: usize,
}

impl ImageLimits {
    fn of(settings: &crate::settings::Settings) -> Self {
        Self {
            memory_mb: settings.local_run_memory_mb,
            pids: settings.local_run_pids,
            timeout: std::time::Duration::from_secs(settings.local_run_timeout_minutes.saturating_mul(60)),
            per_user: settings.local_run_limit,
        }
    }
}

/// The [`LocalRun`] of `job` in its image for `user`: its container's name, the script as its command, the labels
/// that say whose it is, the working tree and its owner, its `.git` (a directory, or a worktree's file) mounted read
/// only over itself, the network (none on a shared dev server: `shared`), and the limits.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn image_run(
    job: &LocalJob,
    tree: &Path,
    user: &str,
    root_id: &str,
    name: &str,
    workdir: &Path,
    limits: &ImageLimits,
    shared: bool,
) -> LocalRun {
    // what git reads to run a hook or a command (its hooks, config, a worktree's link) the job never writes
    let read_only = if std::fs::symlink_metadata(tree.join(".git")).is_ok() {
        vec![".git".to_string()]
    } else {
        Vec::new()
    };
    LocalRun {
        name: name.to_string(),
        image: job.image.clone(),
        command: vec!["sh".into(), "-c".into(), script_in(job, workdir, Place::Image)],
        labels: vec![
            (
                moqui_git::ci::driver::LABEL_SERVER.to_string(),
                moqui_git::ci::driver::server_id(),
            ),
            (LABEL_USER.to_string(), user.to_string()),
            (LABEL_ROOT.to_string(), root_id.to_string()),
        ],
        tree: tree.to_path_buf(),
        user: owner_of(tree).map(|(uid, gid)| format!("{uid}:{gid}")),
        memory_mb: limits.memory_mb,
        pids_limit: limits.pids,
        read_only,
        network_mode: if shared { "none".into() } else { "bridge".into() },
    }
}

/// Why a working tree may not be mounted read-write in a job's image: its user may not run a program on this machine
/// (`TERMINAL_HOST`, as a run in a terminal needs), the root is read only, or it holds the runtime (its database,
/// `.env` and conf). `None` when it may.
#[must_use]
pub fn image_refusal(
    label: &str,
    tree: &Path,
    read_only: bool,
    runtime: &Path,
    may_run_on_host: bool,
) -> Option<String> {
    if !may_run_on_host {
        return Some(format!(
            "A run in the job's image needs {}, as a run in a terminal does",
            HOST_PERMISSION
        ));
    }
    if read_only {
        return Some(format!("{label} is read only: a run in the image would write it"));
    }
    let runtime = std::fs::canonicalize(runtime).unwrap_or_else(|_| runtime.to_path_buf());
    let tree = std::fs::canonicalize(tree).unwrap_or_else(|_| tree.to_path_buf());
    (runtime == tree || crate::confine::inside(&runtime, &tree)).then(|| {
        format!("{label} holds this server's runtime (its database, .env and conf): it is not mounted in an image")
    })
}

/// `script` with a time limit of its own: a watchdog that ends every process of the container's user after `secs`, so
/// a run in the image stops by itself whatever becomes of the server that started it.
#[must_use]
pub fn with_time_limit(script: &str, secs: u64) -> String {
    let (first, rest) = script.split_once('\n').unwrap_or((script, ""));
    format!(
        "{first}\n( sleep {secs}; printf '\\n%s\\n' '== Stopped: the run went past its time ({secs} seconds)'; \
         kill -TERM -1 ) &\n{rest}"
    )
}

/// The paths under a working tree that a run in its image gets read only over themselves: what git reads to run a
/// hook or a command there (`core.fsmonitor`, a hook, a worktree's link), so a step plants nothing the Dev AIDE's next
/// git command runs on the host. Each `.git` (a directory or a file) found under the tree without following a link,
/// each submodule's path (`gitlinks`, from `git ls-files -s`: an uninitialized one is an empty directory a step could
/// make a repository of), and each nested root's `.git`; relative to the tree, sorted. Refused (the tree is not mounted)
/// when the tree has more than [`MAX_SCANNED`] entries to look through.
///
/// # Errors
///
/// The refusal, when the tree is too big to look through, or a directory cannot be read.
pub fn git_paths(tree: &Path, gitlinks: &[String], nested_roots: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut found: Vec<String> = Vec::new();
    let mut pending: Vec<PathBuf> = vec![tree.to_path_buf()];
    let mut scanned = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
            scanned += 1;
            if scanned > MAX_SCANNED {
                return Err(format!(
                    "the working tree has more than {MAX_SCANNED} files and directories to look through for a .git: run it \
                     in a terminal"
                ));
            }
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if entry.file_name() == ".git" {
                if let Ok(relative) = path.strip_prefix(tree) {
                    found.push(relative.display().to_string());
                }
            } else if kind.is_dir() && !kind.is_symlink() {
                pending.push(path);
            }
        }
    }
    for link in gitlinks {
        let plain = !link.is_empty()
            && link
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != "..");
        if plain {
            found.push(link.clone());
        }
    }
    for nested in nested_roots {
        if let Ok(relative) = nested.strip_prefix(tree) {
            if !relative.as_os_str().is_empty() && std::fs::symlink_metadata(nested.join(".git")).is_ok() {
                found.push(relative.join(".git").display().to_string());
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

/// The most entries [`git_paths`] looks through.
pub const MAX_SCANNED: usize = 200_000;

/// The submodules of the repository at `tree`: the paths of its gitlinks (mode 160000 in `git ls-files -s`).
#[must_use]
pub fn gitlinks_of(ls_files: &str) -> Vec<String> {
    ls_files
        .split('\0')
        .filter_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            meta.starts_with("160000 ").then(|| path.to_string())
        })
        .collect()
}

/// The permission a program on this machine needs, a run in the image as the Terminal's host session.
pub const HOST_PERMISSION: &str = "TERMINAL_HOST";

/// How many runs in the image `user` has running on the engine.
fn running_runs(host: &DockerHost, user: &str) -> usize {
    let labels = [
        format!(
            "{}={}",
            moqui_git::ci::driver::LABEL_SERVER,
            moqui_git::ci::driver::server_id()
        ),
        format!("{LABEL_USER}={user}"),
    ];
    let running = host
        .list(&labels)
        .map(|listed| {
            listed
                .iter()
                .filter(|container| {
                    host.inspect(&container.id)
                        .ok()
                        .flatten()
                        .is_some_and(|info| info.running || info.status == "created")
                })
                .count()
        })
        .unwrap_or(0);
    running
}

/// Start `job` in its image: refused unless the script is the one the page showed, the user may run a program here,
/// the tree may be mounted ([`image_refusal`]), an engine answers, the job names an image and the user has fewer runs
/// going than the conf's `local-run-limit`. The container is made on a thread of its own (its image may be pulled
/// first), which follows it: a Remove while it is made stops it once it is, and one that runs past
/// `local-run-timeout-minutes` is stopped. The user's earlier ended runs are removed. Out: `runId`, the container's name,
/// which `get#LocalRun` follows.
fn start_in_image(
    context: &Context,
    root: &crate::roots::Root,
    job: &LocalJob,
    seen: &str,
) -> Result<Parameters, LibError> {
    let hardening = Hardening::from_conf();
    let limits = ImageLimits::of(&context.settings);
    let body = with_time_limit(
        &script_in(job, Path::new(&hardening.workdir), Place::Image),
        limits.timeout.as_secs(),
    );
    if let Some(why) = digest_refusal(&body, seen) {
        return Err(LibError::new(why));
    }
    let may_run_on_host = moqui_core::user::has_permission(HOST_PERMISSION).unwrap_or(false);
    if let Some(why) = image_refusal(
        &root.label,
        &root.path,
        root.read_only,
        context.factory.runtime_dir(),
        may_run_on_host,
    ) {
        return Err(LibError::new(why));
    }
    if job.image.trim().is_empty() {
        return Err(LibError::new(format!("{}/{} names no image", job.pipeline, job.job)));
    }
    // every .git under the tree, each submodule and each nested root's, read only in the container
    let gitlinks = context
        .git()
        .read(&root.path, &["ls-files", "-s", "-z"])
        .map(|out| gitlinks_of(&out))
        .unwrap_or_default();
    let nested: Vec<PathBuf> = context
        .roots
        .iter()
        .filter(|other| other.path != root.path && crate::confine::inside(&other.path, &root.path))
        .map(|other| other.path.clone())
        .collect();
    let read_only = git_paths(&root.path, &gitlinks, &nested).map_err(LibError::new)?;
    let engine = local_engine_for(Some(&root.path))
        .ok_or_else(|| LibError::new("Neither Docker nor Podman answers on this machine: run it in a terminal"))?;
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    clear_ended(&engine.host, &user);
    let running = running_runs(&engine.host, &user);
    let tag = moqui_core::user::random_string(12)?.to_ascii_lowercase();
    let name = format!("{NAME_PREFIX}{}-{tag}", moqui_git::ci::driver::server_name_part());
    let mut run = image_run(
        job,
        &root.path,
        &user,
        &root.id,
        &name,
        Path::new(&hardening.workdir),
        &limits,
        context.settings.shared,
    );
    run.read_only = read_only;
    run.command[2] = body;
    {
        // counted and kept under one lock: two starts at once do not both pass the limit
        let mut starts = moqui_lib::sync::lock(starts());
        let starting = starts
            .values()
            .filter(|start| start.user == user && start.phase == Phase::Starting)
            .count();
        if starting + running >= limits.per_user {
            return Err(LibError::new(format!(
                "You have {} runs in an image going, which is the most at once: remove one first",
                limits.per_user
            )));
        }
        starts.insert(
            name.clone(),
            Start {
                user: user.clone(),
                phase: Phase::Starting,
                cancelled: false,
            },
        );
    }
    let socket = engine.socket.clone();
    let thread_name = name.clone();
    std::thread::Builder::new()
        .name("dev-aide-local-run".into())
        .spawn(move || follow(&socket, &thread_name, &run, &hardening, limits.timeout))
        .map_err(|err| LibError::new(format!("the local run could not start: {err}")))?;
    crate::vcs::audit_git(
        context,
        root,
        "run-locally",
        json!({ "pipeline": job.pipeline, "job": job.job, "matrixKey": job.matrix_key, "image": job.image,
            "container": name, "engine": engine.name() }),
    );
    let mut out = Parameters::new();
    out.insert("runId".into(), Value::string(&name));
    out.insert("engine".into(), Value::string(engine.name()));
    out.insert("note".into(), Value::string(IN_THE_IMAGE));
    Ok(out)
}

/// The thread of a run in the image: make and start its container, then follow it until it ends, Remove asks for it
/// to stop (it is removed), or it runs past `timeout` (it is stopped, and its log kept). The container's own watchdog
/// ([`with_time_limit`]) stops it at the same time if this server is gone.
fn follow(socket: &Path, name: &str, run: &LocalRun, hardening: &Hardening, timeout: std::time::Duration) {
    let host = DockerHost::unix(socket);
    let id = match moqui_git::ci::driver::docker::start_local(&host, run, hardening) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("Dev AIDE: the local run {name} did not start: {err}");
            set_phase(name, Phase::Failed(err.message().to_string()));
            return;
        }
    };
    let started = std::time::Instant::now();
    let mut cancelled = set_phase(name, Phase::Running);
    loop {
        if cancelled {
            // Remove may have taken it already: only one still there is worth a word
            if let Err(err) = host.remove(&id) {
                if host.inspect(&id).ok().flatten().is_some() {
                    tracing::warn!("Dev AIDE: the local run {name}, removed while it started, was not removed: {err}");
                }
            }
            moqui_lib::sync::lock(starts()).remove(name);
            return;
        }
        match host.inspect(&id) {
            Ok(Some(info)) if info.running || info.status == "created" => {}
            Ok(_) => {
                set_phase(name, Phase::Ended);
                return;
            }
            Err(err) => tracing::debug!("Dev AIDE: the local run {name} was not inspected: {err}"),
        }
        if started.elapsed() > timeout {
            if let Err(err) = host.stop(&id, std::time::Duration::from_secs(10)) {
                tracing::warn!("Dev AIDE: the local run {name} past its time was not stopped: {err}");
            }
            set_phase(name, Phase::TimedOut);
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
        cancelled = moqui_lib::sync::lock(starts())
            .get(name)
            .is_none_or(|start| start.cancelled);
    }
}

/// Remove `user`'s local runs that have ended (this server's, by its label): what Remove did not take. One that does
/// not go is logged.
fn clear_ended(host: &DockerHost, user: &str) {
    let labels = [
        format!(
            "{}={}",
            moqui_git::ci::driver::LABEL_SERVER,
            moqui_git::ci::driver::server_id()
        ),
        format!("{LABEL_USER}={user}"),
    ];
    let Ok(listed) = host.list(&labels) else {
        return;
    };
    for container in listed {
        match host.inspect(&container.id) {
            Ok(Some(info)) if !info.running && info.status != "created" => {
                if let Err(err) = host.remove(&container.id) {
                    tracing::warn!("Dev AIDE: an ended local run was not removed: {err}");
                } else {
                    moqui_lib::sync::lock(starts()).remove(&info.name);
                }
            }
            Ok(_) => {}
            Err(err) => tracing::warn!("Dev AIDE: a local run was not inspected: {err}"),
        }
    }
}

/// The current user's run `runId`: its name, its entry here (taken off when `take`), the engine and the container as
/// the engine has it. Refused when it is not a local run's name, or the run (its entry, or its container's labels) is
/// another user's or another server's.
type OwnRun = (String, Option<Phase>, Option<LocalEngine>, Option<ContainerInfo>);

fn own_run(parameters: &Parameters, take: bool) -> Result<OwnRun, LibError> {
    let name = text(parameters, "runId");
    if !valid_run_name(&name) {
        return Err(LibError::new(format!("{name} is not a local run")));
    }
    let user = moqui_core::user::current_user_id().ok().flatten().unwrap_or_default();
    let not_yours = || LibError::new(format!("{name} is not a local run of yours"));
    if user.is_empty() {
        return Err(not_yours());
    }
    let phase = {
        let mut starts = moqui_lib::sync::lock(starts());
        match starts.get_mut(&name) {
            Some(start) if start.user != user => return Err(not_yours()),
            Some(start) => {
                if take {
                    start.cancelled = true;
                }
                Some(start.phase.clone())
            }
            None => None,
        }
    };
    let engine = local_engine();
    let info = match &engine {
        Some(engine) if phase.as_ref() != Some(&Phase::Starting) => engine.host.inspect(&name)?,
        _ => None,
    };
    if let Some(info) = &info {
        let ours = info.labels.get(moqui_git::ci::driver::LABEL_SERVER) == Some(&moqui_git::ci::driver::server_id())
            && info.labels.get(LABEL_USER) == Some(&user);
        if !ours {
            return Err(not_yours());
        }
    } else if phase.is_none() && engine.is_none() {
        return Err(LibError::new("Neither Docker nor Podman answers on this machine"));
    }
    Ok((name, phase, engine, info))
}

/// `get#LocalRun`: a run in the image of the current user's as it stands: `state` (starting, running, exited, failed
/// or missing), `exitCode`, `timedOut`, and the last lines of its log.
fn get_local_run(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::Extend)?;
    let (name, phase, engine, info) = own_run(parameters, false)?;
    let mut out = Parameters::new();
    match (&phase, &info) {
        (Some(Phase::Starting), _) => {
            out.insert("state".into(), Value::string("starting"));
            return Ok(out);
        }
        (Some(Phase::Failed(why)), None) => {
            out.insert("state".into(), Value::string("failed"));
            out.insert("error".into(), Value::string(why));
            return Ok(out);
        }
        (_, None) => {
            out.insert("state".into(), Value::string("missing"));
            return Ok(out);
        }
        _ => {}
    }
    let (Some(info), Some(engine)) = (info, engine) else {
        out.insert("state".into(), Value::string("missing"));
        return Ok(out);
    };
    out.insert(
        "state".into(),
        Value::string(if info.running || info.status == "created" {
            "running"
        } else {
            "exited"
        }),
    );
    if !info.running {
        out.insert("exitCode".into(), info.exit_code.map_or(Value::Null, Value::I64));
    }
    out.insert("timedOut".into(), Value::Bool(phase == Some(Phase::TimedOut)));
    out.insert("log".into(), Value::string(engine.host.logs(&name, 2000)?));
    Ok(out)
}

/// `stop#LocalRun`: a run in the image removed, running or not; one still being made is stopped and removed once it is
/// made.
fn stop_local_run(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, LibError> {
    let _context = Context::of(ec, Tier::Extend)?;
    crate::workspace::unfenced(moqui_core::entity::fence::Effect::Process, "stop#LocalRun")?;
    let (name, phase, engine, info) = own_run(parameters, true)?;
    // one being made is the following thread's to remove once it is: it reads the cancel
    if phase != Some(Phase::Starting) {
        moqui_lib::sync::lock(starts()).remove(&name);
        if let (Some(engine), Some(_)) = (&engine, &info) {
            engine.host.remove(&name)?;
        }
    }
    let mut out = Parameters::new();
    out.insert(
        "removed".into(),
        Value::Bool(info.is_some() || phase == Some(Phase::Starting)),
    );
    Ok(out)
}

#[cfg(test)]
#[path = "local_run_tests.rs"]
mod tests;
