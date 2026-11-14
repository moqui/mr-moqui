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

//! `moqui ci-run`: the runner of a pipeline job (docs/interfaces/Pipelines.md "moqui ci-run"). With no argument it runs the
//! one job its environment names (`MOQUI_CI_SERVER_URL`, `MOQUI_CI_TOKEN`, `MOQUI_CI_JOB_RUN_ID`), as the engine
//! starts it in a job's container. `register` and `poll` are a registered runner's: register once with the pool's
//! token, then ask for jobs and run each. It opens no runtime, no conf and no database: the work is
//! `moqui_git::ci::runner`, which needs `git` and a shell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use moqui_git::ci::runner::{self, http::HttpTransport, Options};
use moqui_lib::error::Error;

fn env(name: &str) -> Result<String, Error> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("moqui ci-run: {name} is not set")))
}

/// The value after `flag` in `args`.
fn option(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|at| args.get(at + 1))
        .cloned()
}

/// Give the steps a home they can write when the process has none (a container with a read-only root and a user
/// with no home directory): `home` beside the workspace.
fn step_home(workspace: &Path) -> Vec<(String, String)> {
    let writable = std::env::var_os("HOME")
        .map(PathBuf::from)
        .is_some_and(|home| home.is_dir() && !std::fs::metadata(&home).is_ok_and(|meta| meta.permissions().readonly()));
    let probe = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(format!(".moqui-ci-probe-{}", std::process::id())));
    let can_write = writable
        && probe.is_some_and(|probe| {
            let made = std::fs::write(&probe, b"").is_ok();
            let _ = std::fs::remove_file(&probe);
            made
        });
    if can_write {
        return Vec::new();
    }
    let home = workspace.parent().unwrap_or(workspace).join("home");
    match std::fs::create_dir_all(&home) {
        Ok(()) => vec![("HOME".to_string(), home.to_string_lossy().into_owned())],
        Err(_) => Vec::new(),
    }
}

fn workspace_of(args: &[String], default: PathBuf) -> Result<PathBuf, Error> {
    let workspace = option(args, "--workspace").map_or(default, PathBuf::from);
    let workspace = if workspace.is_absolute() {
        workspace
    } else {
        std::env::current_dir()
            .map_err(|err| Error::new(format!("moqui ci-run: no working directory: {err}")))?
            .join(workspace)
    };
    if workspace.exists()
        && std::fs::read_dir(&workspace)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    {
        return Err(Error::new(format!(
            "moqui ci-run: the workspace {} is not empty",
            workspace.display()
        )));
    }
    Ok(workspace)
}

fn run_one(server: &str, job_run_id: &str, token: &str, workspace: PathBuf, shell: Option<String>) -> i32 {
    let transport = HttpTransport::new(server, job_run_id, token);
    let options = Options {
        env: step_home(&workspace),
        workspace,
        shell: shell.unwrap_or_else(|| "sh".to_string()),
        ..Options::default()
    };
    runner::run(&transport, &options)
}

/// `moqui ci-run [register | poll] [options]`. The exit code: 0 the job succeeded, 1 it failed, 2 an error, 3 the
/// server no longer takes the job's calls.
pub(crate) fn dispatch(args: &[String]) -> Result<i32, Error> {
    match args.first().map(String::as_str) {
        Some("register") => {
            let server = env("MOQUI_CI_SERVER_URL")?;
            let pool_token = env("MOQUI_CI_POOL_TOKEN")?;
            let name = option(args, "--name").unwrap_or_else(|| "runner".to_string());
            let platform = option(args, "--platform")
                .unwrap_or_else(|| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
            let (status, answer) = runner::http::call(
                &server,
                "register",
                &pool_token,
                &serde_json::json!({ "name": name, "platform": platform }),
            )
            .map_err(|err| Error::new(format!("moqui ci-run register: {err}")))?;
            let Some(token) = answer["token"].as_str().filter(|_| status == 200) else {
                return Err(Error::new(format!(
                    "moqui ci-run register: {status} {}",
                    answer["error"].as_str().unwrap_or_default()
                )));
            };
            // the runner token, to keep as MOQUI_CI_RUNNER_TOKEN: shown this once
            println!("{token}");
            eprintln!(
                "registered runner {} ({name}, {platform})",
                answer["runnerId"].as_str().unwrap_or_default()
            );
            Ok(0)
        }
        Some("poll") => {
            let server = env("MOQUI_CI_SERVER_URL")?;
            let runner_token = env("MOQUI_CI_RUNNER_TOKEN")?;
            let once = args.iter().any(|arg| arg == "--once");
            let interval = option(args, "--interval")
                .and_then(|seconds| seconds.parse::<u64>().ok())
                .unwrap_or(10)
                .clamp(2, 3600);
            let work = option(args, "--work").map_or_else(|| std::env::temp_dir().join("moqui-ci"), PathBuf::from);
            let shell = option(args, "--shell");
            loop {
                let claimed = runner::http::call(&server, "claim", &runner_token, &serde_json::json!({}));
                match claimed {
                    Ok((200, answer)) => {
                        if let (Some(job), Some(token)) = (answer["jobRunId"].as_str(), answer["token"].as_str()) {
                            let dir = work.join(job);
                            let _ = std::fs::remove_dir_all(&dir);
                            let code = run_one(&server, job, token, dir.join("src"), shell.clone());
                            eprintln!("job run {job} ended with exit code {code}");
                            let _ = std::fs::remove_dir_all(&dir);
                            if once {
                                return Ok(code);
                            }
                            continue;
                        }
                    }
                    Ok((401 | 403, answer)) => {
                        return Err(Error::new(format!(
                            "moqui ci-run poll: {}",
                            answer["error"].as_str().unwrap_or("the runner token is not valid")
                        )));
                    }
                    Ok((status, _)) => eprintln!("moqui ci-run poll: the server answered {status}"),
                    Err(err) => eprintln!("moqui ci-run poll: {err}"),
                }
                if once {
                    return Ok(0);
                }
                std::thread::sleep(Duration::from_secs(interval));
            }
        }
        Some(other) if !other.starts_with("--") => Err(crate::usage::command_usage("ci-run")),
        _ => {
            let server = env("MOQUI_CI_SERVER_URL")?;
            let token = env("MOQUI_CI_TOKEN")?;
            let job_run_id = env("MOQUI_CI_JOB_RUN_ID")?;
            let default = std::env::current_dir()
                .map_err(|err| Error::new(format!("moqui ci-run: no working directory: {err}")))?
                .join("src");
            let workspace = workspace_of(args, default)?;
            Ok(run_one(
                &server,
                &job_run_id,
                &token,
                workspace,
                option(args, "--shell"),
            ))
        }
    }
}
