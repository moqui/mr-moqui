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

//! The runner, what `moqui ci-run` does inside a job's container or on a registered runner (plans/Dogfood.md §8.3,
//! docs/interfaces/Pipelines.md "moqui ci-run"): get the job, check the commit out shallow, restore the caches, run each
//! step with a shell in the workspace with a time limit, send the log lines as they are written, upload the reports,
//! the artifacts and the caches, and post the result. It needs this binary, `git` and a shell, and no framework: it
//! talks to the server only through a [`Transport`] ([`http::HttpTransport`] over the runner REST).

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};

pub mod archive;
pub mod http;

/// How the runner reaches the server. `path` is relative to the job's endpoints (`job`, `log`, `steps/1`, ...).
pub trait Transport {
    /// A JSON call: the status and the answer.
    fn json(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), String>;
    /// Upload a file as a request body: the status and the answer.
    fn upload(&self, path_and_query: &str, file: &Path) -> Result<(u16, serde_json::Value), String>;
    /// Download a response body into a file: the status.
    fn download(&self, path_and_query: &str, to: &Path) -> Result<u16, String>;
    /// Check the job's commit out into the empty directory `workspace`.
    fn checkout(&self, job: &serde_json::Value, workspace: &Path) -> Result<String, String>;
}

/// How to run a job.
#[derive(Clone, Debug)]
pub struct Options {
    /// An empty directory the commit is checked out into, where the steps run.
    pub workspace: PathBuf,
    /// The shell the steps run with (`sh`).
    pub shell: String,
    /// Log lines are sent at least this often.
    pub flush: Duration,
    /// Variables of every step, under the job's own (a `HOME` the steps can write).
    pub env: Vec<(String, String)>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("."),
            shell: "sh".to_string(),
            flush: Duration::from_millis(1000),
            env: Vec::new(),
        }
    }
}

/// The exit codes of `moqui ci-run`.
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_FAILURE: i32 = 1;
pub const EXIT_ERROR: i32 = 2;
/// The server no longer takes this job's calls: it was cancelled or timed out.
pub const EXIT_GONE: i32 = 3;

/// Why the runner stopped before the job's end.
enum Stop {
    /// The server refused the job's token: the job has ended there.
    Gone,
    Error(String),
}

fn check(status: u16, what: &str, answer: &serde_json::Value) -> Result<(), Stop> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(Stop::Gone),
        _ => Err(Stop::Error(format!(
            "{what}: {status} {}",
            answer["error"].as_str().unwrap_or_default()
        ))),
    }
}

/// Sends a job's log lines in batches.
struct LogSender<'a> {
    transport: &'a dyn Transport,
    max_batch: usize,
    pending: String,
    step: Option<i64>,
}

impl LogSender<'_> {
    fn line(&mut self, step: Option<i64>, text: &str) -> Result<(), Stop> {
        if step != self.step && !self.pending.is_empty() {
            self.flush()?;
        }
        self.step = step;
        self.pending.push_str(text);
        if !text.ends_with('\n') {
            self.pending.push('\n');
        }
        if self.pending.len() >= self.max_batch / 2 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Stop> {
        while !self.pending.is_empty() {
            let mut cut = self.pending.len().min(self.max_batch.max(1024) - 512);
            while !self.pending.is_char_boundary(cut) {
                cut -= 1;
            }
            let rest = self.pending.split_off(cut);
            let batch = std::mem::replace(&mut self.pending, rest);
            let (status, answer) = self
                .transport
                .json("POST", "log", Some(&json!({ "stepSeq": self.step, "text": batch })))
                .map_err(Stop::Error)?;
            check(status, "sending log lines", &answer)?;
        }
        Ok(())
    }
}

/// `${hash:FILE}` in a cache key replaced by the SHA-256 of that file of the workspace (the first 16 hex characters;
/// empty when it is not there or is outside the workspace).
pub fn expand_key(key: &str, workspace: &Path) -> String {
    let mut out = String::new();
    let mut rest = key;
    while let Some((before, after)) = rest.split_once("${hash:") {
        out.push_str(before);
        let Some((file, tail)) = after.split_once('}') else {
            out.push_str("${hash:");
            rest = after;
            continue;
        };
        if crate::ci::file::valid_path(file) {
            if let Ok(bytes) = std::fs::read(workspace.join(file)) {
                let digest = moqui_lib::codec::hex_encode(&Sha256::digest(&bytes));
                out.push_str(digest.get(..16).unwrap_or(&digest));
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The files and directories `pattern` names in the workspace: a path, or one whose segments have `*` in them, each
/// `*` any run of characters of one name (`test-results/*/*.xml`). Sorted; nothing outside the workspace, and no
/// more than a few thousand.
pub fn matches(workspace: &Path, pattern: &str) -> Vec<PathBuf> {
    const MAX: usize = 5000;
    if !crate::ci::file::valid_path(pattern) {
        return Vec::new();
    }
    let mut found = vec![workspace.to_path_buf()];
    for segment in pattern
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
    {
        let mut next = Vec::new();
        for dir in &found {
            if !segment.contains('*') {
                let path = dir.join(segment);
                // a link is not followed out of the workspace
                if path.symlink_metadata().is_ok() {
                    next.push(path);
                }
                continue;
            }
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut named: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|entry| crate::rules::glob(segment, &entry.file_name().to_string_lossy()))
                .map(|entry| entry.path())
                .collect();
            named.sort();
            next.extend(named);
            if next.len() > MAX {
                next.truncate(MAX);
                break;
            }
        }
        found = next;
        if found.is_empty() {
            break;
        }
    }
    found.retain(|path| path != workspace);
    found
}

/// End a step's process and what it started: the process group on Unix, the process elsewhere.
fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // the group the shell leads; `kill` is a shell builtin and a program everywhere a shell is
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// How a step ended.
struct StepEnd {
    exit: Option<i64>,
    timed_out: bool,
}

/// Run one step's command with the shell in the workspace, its output sent as log lines, until it ends or `deadline`.
fn run_step(
    options: &Options,
    command: &str,
    env: &[(String, String)],
    seq: i64,
    deadline: Instant,
    log: &mut LogSender<'_>,
) -> Result<StepEnd, Stop> {
    let mut spawn = Command::new(&options.shell);
    spawn
        .arg("-c")
        .arg(command)
        .current_dir(&options.workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // a step does not need the job token: what it may reach it reaches through its own tools
        .env_remove("MOQUI_CI_TOKEN");
    for (name, value) in env {
        spawn.env(name, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        spawn.process_group(0);
    }
    let mut child = spawn
        .spawn()
        .map_err(|err| Stop::Error(format!("the shell {} did not start: {err}", options.shell)))?;
    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    let streams: [Option<Box<dyn Read + Send>>; 2] = [
        child.stdout.take().map(|out| Box::new(out) as Box<dyn Read + Send>),
        child.stderr.take().map(|err| Box::new(err) as Box<dyn Read + Send>),
    ];
    for stream in streams.into_iter().flatten() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            loop {
                line.clear();
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(String::from_utf8_lossy(&line).into_owned()).is_err() {
                            break;
                        }
                    }
                }
            }
        }));
    }
    drop(tx);
    let mut last_flush = Instant::now();
    let mut timed_out = false;
    let mut stopped: Option<Stop> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if stopped.is_none() {
                    if let Err(stop) = log.line(Some(seq), &line) {
                        stopped = Some(stop);
                        kill(&mut child);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // both streams closed: the step and everything holding its output ended
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if stopped.is_none() && last_flush.elapsed() >= options.flush {
            last_flush = Instant::now();
            if let Err(stop) = log.flush() {
                stopped = Some(stop);
                kill(&mut child);
            }
        }
        if !timed_out && Instant::now() >= deadline {
            timed_out = true;
            kill(&mut child);
        }
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().ok();
    if let Some(stop) = stopped {
        return Err(stop);
    }
    Ok(StepEnd {
        exit: status.and_then(|status| status.code()).map(i64::from),
        timed_out,
    })
}

fn pairs(value: &serde_json::Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| Some((name.clone(), value.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn query(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", http::encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A temporary file beside the workspace, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(workspace: &Path, name: &str) -> Self {
        let dir = workspace.parent().unwrap_or(workspace);
        Self(dir.join(format!(".moqui-ci-{}-{name}", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn run_job(transport: &dyn Transport, options: &Options) -> Result<bool, Stop> {
    let (status, job) = transport.json("GET", "job", None).map_err(Stop::Error)?;
    check(status, "getting the job", &job)?;
    let spec = &job["spec"];
    let max_batch = usize::try_from(job["limits"]["maxLogBatchBytes"].as_u64().unwrap_or(1024 * 1024))
        .unwrap_or(1024 * 1024)
        .clamp(4096, 4 * 1024 * 1024);
    let mut log = LogSender {
        transport,
        max_batch,
        pending: String::new(),
        step: None,
    };
    let started = Instant::now();
    let job_deadline = started + Duration::from_secs(job["timeoutMinutes"].as_u64().unwrap_or(60).max(1) * 60);
    log.line(
        None,
        &format!(
            "Job {} of pipeline {} at {}",
            spec["job"].as_str().unwrap_or_default(),
            spec["pipeline"].as_str().unwrap_or_default(),
            job["commitId"].as_str().unwrap_or_default()
        ),
    )?;
    std::fs::create_dir_all(&options.workspace).map_err(|err| {
        Stop::Error(format!(
            "the workspace {} was not made: {err}",
            options.workspace.display()
        ))
    })?;
    match transport.checkout(&job, &options.workspace) {
        Ok(text) => {
            if !text.trim().is_empty() {
                log.line(None, text.trim_end())?;
            }
        }
        Err(err) => {
            log.line(None, &format!("The checkout failed: {err}"))?;
            log.flush()?;
            return Err(Stop::Error(format!("the checkout failed: {err}")));
        }
    }
    // caches: restored before the first step
    let mut missed: Vec<(String, String)> = Vec::new();
    for cache in spec["caches"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let (Some(key), Some(path)) = (cache["key"].as_str(), cache["path"].as_str()) else {
            continue;
        };
        let key = expand_key(key, &options.workspace);
        let scratch = Scratch::new(&options.workspace, "cache.tar.gz");
        match transport.download(&format!("caches?{}", query(&[("key", &key)])), &scratch.0) {
            Ok(200) => match archive::unpack(&scratch.0, &options.workspace) {
                Ok(count) => log.line(None, &format!("Cache {key} restored to {path} ({count} entries)"))?,
                Err(err) => log.line(None, &format!("Cache {key} was not restored: {err}"))?,
            },
            Ok(404) => {
                log.line(None, &format!("No cache for {key}"))?;
                missed.push((key, path.to_string()));
            }
            Ok(401 | 403) => return Err(Stop::Gone),
            Ok(status) => log.line(None, &format!("Cache {key} was not read: {status}"))?,
            Err(err) => log.line(None, &format!("Cache {key} was not read: {err}"))?,
        }
    }
    // the job's variables, then what every job has, then its secrets
    let mut env = options.env.clone();
    env.extend(pairs(&spec["env"]));
    env.extend(pairs(&job["ciEnv"]));
    env.extend(pairs(&job["secrets"]));
    let mut succeeded = true;
    let steps = spec["steps"].as_array().map(Vec::as_slice).unwrap_or_default();
    for step in steps {
        let seq = step["seq"].as_i64().unwrap_or(0);
        let run = step["run"].as_str().unwrap_or_default();
        if !succeeded {
            break;
        }
        let (status, answer) = transport
            .json("POST", &format!("steps/{seq}"), Some(&json!({ "status": "running" })))
            .map_err(Stop::Error)?;
        check(status, "starting a step", &answer)?;
        let shown = step["name"]
            .as_str()
            .unwrap_or_else(|| run.lines().next().unwrap_or_default());
        log.line(Some(seq), &format!("$ {shown}"))?;
        let deadline = step["timeoutMinutes"].as_u64().map_or(job_deadline, |minutes| {
            job_deadline.min(Instant::now() + Duration::from_secs(minutes * 60))
        });
        let end = run_step(options, run, &env, seq, deadline, &mut log)?;
        let ok = !end.timed_out && end.exit == Some(0);
        if end.timed_out {
            log.line(Some(seq), "The step was ended: it ran past its time limit")?;
        } else if !ok {
            log.line(
                Some(seq),
                &format!(
                    "The step failed{}",
                    end.exit.map_or(String::new(), |code| format!(" with exit code {code}"))
                ),
            )?;
        }
        log.flush()?;
        let (status, answer) = transport
            .json(
                "POST",
                &format!("steps/{seq}"),
                Some(&json!({ "status": if ok { "success" } else { "failure" }, "exitCode": end.exit })),
            )
            .map_err(Stop::Error)?;
        check(status, "ending a step", &answer)?;
        if !ok && step["continueOnError"].as_bool() != Some(true) {
            succeeded = false;
        }
        if Instant::now() >= job_deadline {
            succeeded = false;
        }
    }
    // reports and artifacts: also of a job that failed
    for pattern in spec["reports"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let Some(pattern) = pattern.as_str() else { continue };
        for file in matches(&options.workspace, pattern)
            .into_iter()
            .filter(|path| path.is_file())
        {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let sent = transport.upload(
                &format!(
                    "artifacts?{}",
                    query(&[("kind", "report"), ("name", &name), ("path", pattern)])
                ),
                &file,
            );
            match sent {
                Ok((401 | 403, _)) => return Err(Stop::Gone),
                Ok((200..=299, _)) => {}
                Ok((status, answer)) => log.line(
                    None,
                    &format!(
                        "Report {name} was not uploaded: {status} {}",
                        answer["error"].as_str().unwrap_or_default()
                    ),
                )?,
                Err(err) => log.line(None, &format!("Report {name} was not uploaded: {err}"))?,
            }
        }
    }
    for artifact in spec["artifacts"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let (Some(name), Some(pattern)) = (artifact["name"].as_str(), artifact["path"].as_str()) else {
            continue;
        };
        let files = matches(&options.workspace, pattern);
        if files.is_empty() {
            log.line(None, &format!("Artifact {name}: nothing at {pattern}"))?;
            continue;
        }
        let scratch = Scratch::new(&options.workspace, "artifact.tar.gz");
        if let Err(err) = archive::pack(&options.workspace, &files, &scratch.0) {
            log.line(None, &format!("Artifact {name} was not packed: {err}"))?;
            continue;
        }
        let days = artifact["retentionDays"].as_i64().map(|days| days.to_string());
        let mut pairs = vec![("kind", "artifact"), ("name", name), ("path", pattern)];
        if let Some(days) = &days {
            pairs.push(("retentionDays", days));
        }
        match transport.upload(&format!("artifacts?{}", query(&pairs)), &scratch.0) {
            Ok((401 | 403, _)) => return Err(Stop::Gone),
            Ok((200..=299, _)) => log.line(None, &format!("Artifact {name} uploaded"))?,
            Ok((status, answer)) => log.line(
                None,
                &format!(
                    "Artifact {name} was not uploaded: {status} {}",
                    answer["error"].as_str().unwrap_or_default()
                ),
            )?,
            Err(err) => log.line(None, &format!("Artifact {name} was not uploaded: {err}"))?,
        }
    }
    // caches: saved by a trusted job that succeeded, for a key nothing was stored under
    if succeeded && job["trusted"].as_bool() == Some(true) {
        for (key, path) in &missed {
            let dir = options.workspace.join(path);
            if !dir.exists() {
                continue;
            }
            let scratch = Scratch::new(&options.workspace, "cache-out.tar.gz");
            if let Err(err) = archive::pack(&options.workspace, &[dir], &scratch.0) {
                log.line(None, &format!("Cache {key} was not packed: {err}"))?;
                continue;
            }
            match transport.upload(
                &format!(
                    "artifacts?{}",
                    query(&[("kind", "cache"), ("key", key), ("name", key), ("path", path)])
                ),
                &scratch.0,
            ) {
                Ok((401 | 403, _)) => return Err(Stop::Gone),
                Ok((200..=299, _)) => log.line(None, &format!("Cache {key} saved from {path}"))?,
                Ok((status, answer)) => log.line(
                    None,
                    &format!(
                        "Cache {key} was not saved: {status} {}",
                        answer["error"].as_str().unwrap_or_default()
                    ),
                )?,
                Err(err) => log.line(None, &format!("Cache {key} was not saved: {err}"))?,
            }
        }
    }
    log.flush()?;
    Ok(succeeded)
}

/// Run the job the transport's token is for, and post its result. The exit code of `moqui ci-run`.
pub fn run(transport: &dyn Transport, options: &Options) -> i32 {
    let (result, code) = match run_job(transport, options) {
        Ok(true) => (json!({ "status": "success", "exitCode": 0 }), EXIT_SUCCESS),
        Ok(false) => (json!({ "status": "failure", "exitCode": 1 }), EXIT_FAILURE),
        Err(Stop::Gone) => {
            eprintln!("moqui ci-run: the server no longer takes this job's calls (cancelled, or past its time)");
            return EXIT_GONE;
        }
        Err(Stop::Error(message)) => {
            eprintln!("moqui ci-run: {message}");
            (json!({ "status": "error", "message": message }), EXIT_ERROR)
        }
    };
    match transport.json("POST", "result", Some(&result)) {
        Ok((200..=299, _)) => code,
        Ok((401 | 403, _)) => EXIT_GONE,
        Ok((status, answer)) => {
            eprintln!(
                "moqui ci-run: the result was not posted: {status} {}",
                answer["error"].as_str().unwrap_or_default()
            );
            EXIT_ERROR
        }
        Err(err) => {
            eprintln!("moqui ci-run: the result was not posted: {err}");
            EXIT_ERROR
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
