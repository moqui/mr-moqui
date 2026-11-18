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
//! token, then ask for jobs and run each. `collect` is the child a `--step-user` runner reads a job's files with, as
//! that user. It opens no runtime, no conf and no database: the work is `moqui_git::ci::runner`, which needs `git` and
//! a shell.

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
    job_home(workspace)
}

/// A `home` beside the workspace, as the steps' `HOME`.
fn job_home(workspace: &Path) -> Vec<(String, String)> {
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

/// Run one job. `registered`: a registered runner's, whose steps get only the base environment and `pass_env`, not the
/// machine's; else a job's container, whose image sets the environment up.
fn run_one(
    server: &str,
    job_run_id: &str,
    token: &str,
    workspace: PathBuf,
    shell: Option<String>,
    registered: Option<Vec<String>>,
    step_user: Option<runner::StepUser>,
) -> i32 {
    let transport = HttpTransport::new(server, job_run_id, token);
    // a step user gets a home of its own beside the workspace, never the runner's (root's)
    let env = if step_user.is_some() { job_home(&workspace) } else { step_home(&workspace) };
    let options = Options {
        env,
        workspace,
        shell: shell.unwrap_or_else(|| "sh".to_string()),
        inherit_env: registered.is_none(),
        pass_env: registered.unwrap_or_default(),
        step_user,
        ..Options::default()
    };
    runner::run(&transport, &options)
}

/// A registered runner's token: from the file `--token-file` or `MOQUI_CI_RUNNER_TOKEN_FILE` names, which only the
/// runner's user should read; else `MOQUI_CI_RUNNER_TOKEN`, with a warning, since a process's starting environment can
/// be read by any process of its user (`/proc/<pid>/environ`), the steps it runs included (plans/Security-Review.md
/// SEC-GIT-02).
/// The token, and the file it came from (where a rotated token is written), if any.
fn runner_token(args: &[String]) -> Result<(String, Option<PathBuf>), Error> {
    let file = option(args, "--token-file").or_else(|| env("MOQUI_CI_RUNNER_TOKEN_FILE").ok());
    if let Some(file) = file {
        let text = std::fs::read_to_string(&file)
            .map_err(|err| Error::new(format!("moqui ci-run poll: cannot read the runner token file {file}: {err}")))?;
        let token = text.trim().to_string();
        if token.is_empty() {
            return Err(Error::new(format!("moqui ci-run poll: the runner token file {file} is empty")));
        }
        return Ok((token, Some(PathBuf::from(file))));
    }
    let token = env("MOQUI_CI_RUNNER_TOKEN")?;
    eprintln!(
        "moqui ci-run poll: the runner token is in MOQUI_CI_RUNNER_TOKEN, which a step can read from this process's \
         environment, and a token there cannot be rotated before it expires; keep it in a file only the runner's user \
         reads and give --token-file or MOQUI_CI_RUNNER_TOKEN_FILE"
    );
    Ok((token, None))
}

/// Replace the token file at `path` with `token`: a new file beside it, readable by its owner only, renamed over it, so
/// a crash leaves the old token or the new one, never part of one.
fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let name = path.file_name().map_or_else(|| "token".into(), |name| name.to_string_lossy().into_owned());
    let fresh = path.with_file_name(format!(".{name}.new"));
    let _ = std::fs::remove_file(&fresh);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&fresh)?;
    file.write_all(format!("{token}\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&fresh, path)
}

/// After a claim's answer, rotate the runner token once its `rotateAfter` has passed: the new token is written to the
/// token file and used from then on. With no file the token cannot be kept, so the runner says when it expires instead
/// (once). A failed rotation is said and tried again after the next claim; the old token is good until it expires.
fn rotate_when_due(server: &str, answer: &serde_json::Value, token: &mut String, file: Option<&Path>, warned: &mut bool) {
    let Some(after) = answer["rotateAfter"].as_i64() else {
        return;
    };
    if jiff::Timestamp::now().as_millisecond() < after {
        return;
    }
    let Some(file) = file else {
        if !*warned {
            eprintln!(
                "moqui ci-run poll: the runner token is due to be rotated and cannot be, as it is not in a token file; \
                 it stops working when it expires: give it by --token-file"
            );
            *warned = true;
        }
        return;
    };
    match runner::http::call(server, "rotate", token, &serde_json::json!({})) {
        Ok((200, rotated)) => {
            let Some(fresh) = rotated["token"].as_str() else {
                eprintln!("moqui ci-run poll: the server answered a rotation with no token");
                return;
            };
            // the old token is dead once the server answered: keep the new one even if the file cannot be written
            if let Err(err) = write_token_file(file, fresh) {
                eprintln!(
                    "moqui ci-run poll: the runner token was rotated but could not be written to {}: {err}; it is kept \
                     in memory only, so register the runner again before restarting it",
                    file.display()
                );
            } else {
                eprintln!("moqui ci-run poll: the runner token was rotated");
            }
            *token = fresh.to_string();
        }
        Ok((status, rotated)) => eprintln!(
            "moqui ci-run poll: rotating the runner token was answered {status}: {}",
            rotated["error"].as_str().unwrap_or_default()
        ),
        Err(err) => eprintln!("moqui ci-run poll: rotating the runner token: {err}"),
    }
}

/// The names `--pass-env NAME,...` lets through to a registered runner's steps (repeatable).
fn pass_env(args: &[String]) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "--pass-env")
        .filter_map(|(at, _)| args.get(at + 1))
        .flat_map(|names| names.split(','))
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
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
            // the runner token, shown this once: to keep in a file only the runner's user reads, for poll --token-file
            println!("{token}");
            eprintln!(
                "registered runner {} ({name}, {platform})",
                answer["runnerId"].as_str().unwrap_or_default()
            );
            Ok(0)
        }
        Some("poll") => {
            let server = env("MOQUI_CI_SERVER_URL")?;
            let (mut runner_token, token_file) = runner_token(args)?;
            let mut warned = false;
            let step_user = match option(args, "--step-user") {
                Some(name) => Some(runner::step_user(&name).map_err(|why| Error::new(format!("moqui ci-run poll: {why}")))?),
                None => None,
            };
            let passed = pass_env(args);
            let once = args.iter().any(|arg| arg == "--once");
            let interval = option(args, "--interval")
                .and_then(|seconds| seconds.parse::<u64>().ok())
                .unwrap_or(10)
                .clamp(2, 3600);
            let work = option(args, "--work").map_or_else(|| std::env::temp_dir().join("moqui-ci"), PathBuf::from);
            // absolute, as a collect child runs in another directory and is given paths of the workspace (SEC-GIT-81)
            let work = std::path::absolute(&work)
                .map_err(|err| Error::new(format!("moqui ci-run poll: --work {}: {err}", work.display())))?;
            // the step user must run this binary, which reads the reports, artifacts and caches as that user: asked
            // once here, so a runner that could not read them takes no job
            #[cfg(unix)]
            if let Some(user) = &step_user {
                std::fs::create_dir_all(&work)
                    .map_err(|err| Error::new(format!("moqui ci-run poll: --work {}: {err}", work.display())))?;
                runner::collect::probe(user, &work).map_err(|why| {
                    Error::new(format!(
                        "moqui ci-run poll: --step-user {} cannot run this program, which reads each job's reports, \
                         artifacts and caches as that user: {why}",
                        user.name
                    ))
                })?;
            }
            let shell = option(args, "--shell");
            let keep_processes = args.iter().any(|arg| arg == "--keep-processes");
            // a pool's untrusted runs go only to a runner that runs its steps as another user (SEC-GIT-19)
            let claim = serde_json::json!({ "stepUser": step_user.is_some() });
            let mut noticed = false;
            loop {
                let claimed = runner::http::call(&server, "claim", &runner_token, &claim);
                match claimed {
                    Ok((200, answer)) => {
                        rotate_when_due(&server, &answer, &mut runner_token, token_file.as_deref(), &mut warned);
                        if let Some(notice) = answer["notice"].as_str().filter(|_| !noticed) {
                            eprintln!("moqui ci-run poll: {notice}");
                            noticed = true;
                        }
                        if let (Some(job), Some(token)) = (answer["jobRunId"].as_str(), answer["token"].as_str()) {
                            let dir = work.join(job);
                            let _ = std::fs::remove_dir_all(&dir);
                            let began = std::time::Instant::now();
                            let code = run_one(
                                &server,
                                job,
                                token,
                                dir.join("src"),
                                shell.clone(),
                                Some(passed.clone()),
                                step_user.clone(),
                            );
                            eprintln!("job run {job} ended with exit code {code}");
                            let _ = std::fs::remove_dir_all(&dir);
                            // a registered runner is not cleaned between jobs but for this: what the job left
                            // running does not see the next one (the runner runs as a user of its own)
                            if !keep_processes {
                                let killed = runner::end_leftovers(began.elapsed().as_secs() + 1, step_user.as_ref());
                                if killed > 0 {
                                    eprintln!("killed {killed} processes job run {job} left");
                                }
                            }
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
        // the child a --step-user runner reads a job's reports, artifacts and caches with, as the step user
        // (runner::collect; SEC-GIT-81): not for a person to run
        Some("collect") => {
            #[cfg(unix)]
            {
                Ok(runner::collect::collect_child(args.get(1).map_or("", String::as_str)))
            }
            #[cfg(not(unix))]
            {
                Err(Error::new("moqui ci-run collect is for unix"))
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
                None,
                None,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    /// A rotated runner token replaces the file whole, readable by its owner only (SEC-GIT-02).
    #[test]
    fn a_rotated_token_replaces_the_token_file() {
        let dir = std::env::temp_dir().join(format!("moqui-ci-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, "mqcr_old\n").unwrap();
        super::write_token_file(&file, "mqcr_new").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "mqcr_new\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // nothing is left beside it
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
