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
pub mod collect;
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
    /// Whether a step inherits the runner's environment (a job's container, whose image sets it up). A registered
    /// runner's steps do not ([`STEP_BASE_ENV`] and [`Self::pass_env`] only): the machine's environment is not the
    /// job's (plans/Security-Review.md SEC-GIT-02).
    pub inherit_env: bool,
    /// Names of the runner's variables a step gets when it does not inherit the environment (`--pass-env`).
    pub pass_env: Vec<String>,
    /// The user the steps run as (`ci-run poll --step-user`, unix, the runner running as root): the workspace and the
    /// step home are handed to it before the first step, and the runner's files (its token file) are out of its reach.
    pub step_user: Option<StepUser>,
}

/// Another user a registered runner runs its steps as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepUser {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
}

/// The step user `name`, from what `id -u` says of the runner and `id -u name` and `id -g name` of the user: refused
/// unless the runner is root (only root runs a process as another user), the name resolves, and it is not root.
///
/// # Errors
///
/// Why the user cannot be the step user.
pub fn step_user_of(name: &str, runner_uid: &str, uid: &str, gid: &str) -> Result<StepUser, String> {
    if runner_uid.trim() != "0" {
        return Err("--step-user needs the runner to run as root, which starts each step as that user".into());
    }
    let (Ok(uid), Ok(gid)) = (uid.trim().parse::<u32>(), gid.trim().parse::<u32>()) else {
        return Err(format!("--step-user {name}: no such user"));
    };
    if uid == 0 {
        return Err(format!("--step-user {name} is root: give a user of its own"));
    }
    Ok(StepUser {
        name: name.to_string(),
        uid,
        gid,
    })
}

/// [`step_user_of`] for this machine (`id`).
///
/// # Errors
///
/// Why the user cannot be the step user; on a system that is not unix, always.
pub fn step_user(name: &str) -> Result<StepUser, String> {
    #[cfg(unix)]
    {
        step_user_of(
            name,
            &output_of("id", &["-u"]),
            &output_of("id", &["-u", name]),
            &output_of("id", &["-g", name]),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        Err("--step-user is for unix".into())
    }
}

/// Hand `path` over to the step user (`chown -R`), so the steps write the checkout and their home.
fn hand_over(path: &Path, user: &StepUser) -> Result<(), String> {
    let status = Command::new("chown")
        .arg("-R")
        .arg(format!("{}:{}", user.uid, user.gid))
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .map_err(|err| format!("chown did not run: {err}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("chown of {} to {} failed: {status}", path.display(), user.name))
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            workspace: PathBuf::from("."),
            shell: "sh".to_string(),
            flush: Duration::from_millis(1000),
            env: Vec::new(),
            inherit_env: true,
            pass_env: Vec::new(),
            step_user: None,
        }
    }
}

/// The runner's variables a step of a registered runner gets, with the `LC_` ones; the job's variables come on top.
pub const STEP_BASE_ENV: &[&str] =
    &["PATH", "HOME", "USER", "LOGNAME", "LANG", "LANGUAGE", "TZ", "TMPDIR", "SHELL", "TERM"];

/// Whether a variable of the runner is a credential of the runner (its job token, its runner token, a pool token, and
/// the file that holds the runner token), which no step gets, whatever passes the environment on.
fn runner_credential(name: &str) -> bool {
    name.starts_with("MOQUI_CI_") && (name.ends_with("_TOKEN") || name.ends_with("_TOKEN_FILE"))
}

/// The runner's variables a step starts from: all of them in a job's container (`inherit_env`), else [`STEP_BASE_ENV`],
/// the `LC_` ones and `pass_env`; never a runner credential, nor (without `inherit_env`) any other `MOQUI_CI_` one.
pub fn step_base_env(
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    options: &Options,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    vars.into_iter()
        .filter(|(name, _)| {
            let Some(name) = name.to_str() else {
                return options.inherit_env;
            };
            if runner_credential(name) {
                return false;
            }
            options.inherit_env
                || (!name.starts_with("MOQUI_CI_")
                    && (STEP_BASE_ENV.contains(&name)
                        || name.starts_with("LC_")
                        || options.pass_env.iter().any(|passed| passed == name)))
        })
        .collect()
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
        401 | 403 | 410 => Err(Stop::Gone),
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
    expand_key_noting(key, workspace, &mut Vec::new())
}

/// [`expand_key`], with a note for the job's log for each `${hash:FILE}` whose file is there but was not read (a link,
/// a directory, a file outside the workspace), which hashes to nothing and so leaves that part of the key constant.
pub fn expand_key_noting(key: &str, workspace: &Path, notes: &mut Vec<String>) -> String {
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
            let mut bytes = Vec::new();
            let path = workspace.join(file);
            match copy_regular(workspace, &path, &mut bytes) {
                Ok(()) => {
                    let digest = moqui_lib::codec::hex_encode(&Sha256::digest(&bytes));
                    out.push_str(digest.get(..16).unwrap_or(&digest));
                }
                Err(_) if path.symlink_metadata().is_err() => {}
                Err(_) => notes.push(format!(
                    "Cache key {key}: ${{hash:{file}}} hashes to nothing, as {file} is not a regular file of the \
                     workspace (a link is not followed)"
                )),
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// Whether what `path` names, through every link on the way, is under `root` (a workspace resolved with
/// `canonicalize`). A path that does not resolve (missing, a dangling link) is not.
pub(crate) fn resolves_inside(root: &Path, path: &Path) -> bool {
    path.canonicalize().is_ok_and(|real| real.starts_with(root))
}

/// Open `path` to read when it is a regular file and not a link: `O_NOFOLLOW` refuses a link as the last name, and
/// `O_NONBLOCK` keeps a FIFO put in its place from holding the open; what was opened is checked to be a regular file
/// (plans/Security-Review.md SEC-GIT-81).
///
/// # Errors
///
/// When it does not open (a link among them) or is not a regular file.
pub(crate) fn open_regular(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK)
                .bits()
                .cast_signed(),
        );
    }
    #[cfg(not(unix))]
    if path.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(format!("{} is a symbolic link", path.display()));
    }
    let file = options.open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    match file.metadata() {
        Ok(meta) if meta.is_file() => Ok(file),
        Ok(_) => Err(format!("{} is not a regular file", path.display())),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// Copy the regular file `file`, which is under `workspace` and not a link, to `out`. The directories on its path may
/// be links, if what they name is in the workspace.
///
/// # Errors
///
/// When the file is outside the workspace, a link, not a regular file, or does not read.
pub(crate) fn copy_regular(workspace: &Path, file: &Path, out: &mut dyn std::io::Write) -> Result<(), String> {
    let root = workspace
        .canonicalize()
        .map_err(|err| format!("{}: {err}", workspace.display()))?;
    if !resolves_inside(&root, file) {
        return Err(format!("{} is outside the workspace", file.display()));
    }
    let mut opened = open_regular(file)?;
    std::io::copy(&mut opened, out)
        .map(|_| ())
        .map_err(|err| format!("{}: {err}", file.display()))
}

/// The files and directories `pattern` names in the workspace: a path, or one whose segments have `*` in them, each
/// `*` any run of characters of one name (`test-results/*/*.xml`). Sorted; no more than a few thousand. A link is
/// followed only while what it names is in the workspace: a path whose target is outside it (a link of the last
/// name, or of a directory on the way) is left out, and a `*` never lists a directory out of it (SEC-GIT-81).
pub fn matches(workspace: &Path, pattern: &str) -> Vec<PathBuf> {
    find(workspace, pattern)
        .found
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

/// What a matched path is, by `symlink_metadata`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Link,
    Dir,
    Other,
}

impl Kind {
    fn of(path: &Path) -> Self {
        match path.symlink_metadata().map(|meta| meta.file_type()) {
            Ok(kind) if kind.is_symlink() => Self::Link,
            Ok(kind) if kind.is_file() => Self::File,
            Ok(kind) if kind.is_dir() => Self::Dir,
            _ => Self::Other,
        }
    }
}

/// What a pattern names in the workspace ([`find`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matches {
    /// The paths [`matches`] answers, each with what it is.
    pub found: Vec<(PathBuf, Kind)>,
    /// The paths left out because they do not resolve inside the workspace (a link out of it, or a dangling one), at
    /// most a hundred, for the job's log.
    pub refused: Vec<PathBuf>,
}

/// [`matches`], with what each path is and the paths refused.
pub fn find(workspace: &Path, pattern: &str) -> Matches {
    const MAX: usize = 5000;
    const MAX_REFUSED: usize = 100;
    let mut refused = Vec::new();
    if !crate::ci::file::valid_path(pattern) {
        return Matches::default();
    }
    let Ok(root) = workspace.canonicalize() else {
        return Matches::default();
    };
    let mut keep = |path: PathBuf, next: &mut Vec<PathBuf>| {
        if resolves_inside(&root, &path) {
            next.push(path);
        } else if refused.len() < MAX_REFUSED && path.symlink_metadata().is_ok() {
            refused.push(path);
        }
    };
    let mut found = vec![workspace.to_path_buf()];
    for segment in pattern
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
    {
        let mut next = Vec::new();
        for dir in &found {
            if !segment.contains('*') {
                keep(dir.join(segment), &mut next);
                continue;
            }
            // every path kept so far resolves inside the workspace, so this lists no directory out of it
            let Ok(entries) = std::fs::read_dir(dir) else { continue };
            let mut named: Vec<PathBuf> = entries
                .filter_map(Result::ok)
                .filter(|entry| crate::rules::glob(segment, &entry.file_name().to_string_lossy()))
                .map(|entry| entry.path())
                .collect();
            named.sort();
            for path in named {
                keep(path, &mut next);
            }
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
    Matches {
        found: found
            .into_iter()
            .map(|path| {
                let kind = Kind::of(&path);
                (path, kind)
            })
            .collect(),
        refused,
    }
}

/// End a step's process and what it started: the process group on Unix, the process elsewhere.
fn kill(child: &mut std::process::Child) {
    end_group(child.id());
    let _ = child.kill();
}

/// Kill what is left of a step's process group (on Unix): a step that ended may have left a process running in the
/// background, which would see the next step's or the next job's environment.
fn end_group(leader: u32) {
    #[cfg(unix)]
    {
        // the group the shell leads; `kill` is a shell builtin and a program everywhere a shell is
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{leader}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = leader;
}

/// The words a command printed, or nothing when it did not run.
#[cfg(unix)]
fn output_of(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

/// Seconds from a `ps` `etime` (`[[dd-]hh:]mm:ss`), the time since a process started.
fn etime_seconds(text: &str) -> Option<u64> {
    let text = text.trim();
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<u64>().ok()?, clock),
        None => (0, text),
    };
    let mut seconds = 0u64;
    for part in clock.split(':') {
        seconds = seconds.checked_mul(60)?.checked_add(part.parse::<u64>().ok()?)?;
    }
    Some(days * 86_400 + seconds)
}

/// The processes of a `ps -o pid=,etime=` listing started within `within_seconds`, but those of `keep`.
fn started_within(listing: &str, within_seconds: u64, keep: &[u32]) -> Vec<u32> {
    listing
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid: u32 = words.next()?.parse().ok()?;
            let age = etime_seconds(words.next()?)?;
            (age <= within_seconds && !keep.contains(&pid)).then_some(pid)
        })
        .collect()
}

/// Kill the processes of this runner's user that a job left running, between the jobs of a registered runner
/// (`moqui ci-run poll`): those started since the job began (`job_seconds` ago), in a group of their own or not, so
/// they do not see the next job (docs/interfaces/Pipelines.md, "Registered runners"); never the runner, the processes
/// it runs under, or one started before the job (a `tee` the runner's output goes through). Never as root: with a
/// `step_user` the processes of that user, which the runner (root) can end. How many were killed.
pub fn end_leftovers(job_seconds: u64, step_user: Option<&StepUser>) -> usize {
    #[cfg(unix)]
    {
        // the step user's processes when the steps ran as one (the runner is root then), else the runner's own user's,
        // never root's
        let uid = match step_user {
            Some(user) => user.uid.to_string(),
            None => output_of("id", &["-u"]).trim().to_string(),
        };
        if uid.is_empty() || uid == "0" {
            return 0;
        }
        let mut keep = vec![std::process::id()];
        let mut pid = std::os::unix::process::parent_id();
        for _ in 0..64 {
            if pid <= 1 || keep.contains(&pid) {
                break;
            }
            keep.push(pid);
            pid = output_of("ps", &["-o", "ppid=", "-p", &pid.to_string()])
                .trim()
                .parse()
                .unwrap_or(0);
        }
        let listed = output_of("ps", &["-U", &uid, "-o", "pid=,etime="]);
        let mut killed = 0;
        for pid in started_within(&listed, job_seconds, &keep) {
            let ended = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if ended {
                killed += 1;
            }
        }
        killed
    }
    #[cfg(not(unix))]
    {
        let _ = (job_seconds, step_user);
        0
    }
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
        // a step does not get the runner's tokens (what it may reach it reaches through its own tools), and on a
        // registered runner not the machine's environment either: only what step_base_env lets through
        .env_clear()
        .envs(step_base_env(std::env::vars_os(), options));
    for (name, value) in env {
        spawn.env(name, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        spawn.process_group(0);
        // as the step user: std drops root's supplementary groups when it sets the uid
        if let Some(user) = &options.step_user {
            spawn.uid(user.uid).gid(user.gid).env("USER", &user.name).env("LOGNAME", &user.name);
        }
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
    // what the step left in its group goes with it: before the step's process is waited for, while its id (the
    // group's) cannot be another's, so no group that took a reused id is killed
    end_group(child.id());
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

/// `path` as the job's log names it: by its path in the workspace.
fn shown_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace).unwrap_or(path).display().to_string()
}

/// A temporary file beside the workspace, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(workspace: &Path, name: &str) -> Self {
        Self::in_dir(workspace.parent().unwrap_or(workspace), name)
    }

    fn in_dir(dir: &Path, name: &str) -> Self {
        Self(dir.join(format!(".moqui-ci-{}-{name}", std::process::id())))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Write a line to the job's log for each of `refused`, named by its path in the workspace.
fn log_refused(
    log: &mut LogSender<'_>,
    workspace: &Path,
    refused: &[PathBuf],
    said: impl Fn(&str) -> String,
) -> Result<(), Stop> {
    for path in refused {
        log.line(None, &said(&shown_path(workspace, path)))?;
    }
    Ok(())
}

fn run_job(transport: &dyn Transport, options: &Options) -> Result<bool, Stop> {
    // an absolute workspace: the paths of it a collect child is given hold in any directory (`--work` may be relative)
    let workspace = std::path::absolute(&options.workspace).map_err(|err| {
        Stop::Error(format!(
            "the workspace {} has no absolute path: {err}",
            options.workspace.display()
        ))
    })?;
    let options = &Options {
        workspace,
        ..options.clone()
    };
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
        let mut notes = Vec::new();
        let key = expand_key_noting(key, &options.workspace, &mut notes);
        for note in notes {
            log.line(None, &note)?;
        }
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
            Ok(401 | 403 | 410) => return Err(Stop::Gone),
            Ok(status) => log.line(None, &format!("Cache {key} was not read: {status}"))?,
            Err(err) => log.line(None, &format!("Cache {key} was not read: {err}"))?,
        }
    }
    // the steps' user owns what they work in: the checkout with its caches, and the home the runner gave them
    if let Some(user) = &options.step_user {
        let home = options.env.iter().find(|(name, _)| name == "HOME").map(|(_, home)| PathBuf::from(home));
        for path in std::iter::once(options.workspace.clone()).chain(home) {
            if let Err(err) = hand_over(&path, user) {
                log.line(None, &format!("The workspace was not handed to {}: {err}", user.name))?;
                log.flush()?;
                return Err(Stop::Error(err));
            }
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
    // with a step user, what the steps left running (a process that left its group with setsid) is ended before
    // anything is read: none of it can stop the child that reads, or change the files under it while it reads
    if let Some(user) = &options.step_user {
        for _ in 0..5 {
            if end_leftovers(started.elapsed().as_secs() + 1, Some(user)) == 0 {
                break;
            }
        }
    }
    // reports, artifacts and caches are read within one time limit, as the step user if there is one (collect)
    let collecting = Instant::now() + collect::COLLECT_LIMIT;
    // reports and artifacts: also of a job that failed
    for pattern in spec["reports"].as_array().map(Vec::as_slice).unwrap_or_default() {
        let Some(pattern) = pattern.as_str() else { continue };
        let found = match collect::find_in(options, pattern, collecting) {
            Ok(found) => found,
            Err(err) => {
                log.line(None, &format!("Reports {pattern} were not read: {err}"))?;
                continue;
            }
        };
        log_refused(&mut log, &options.workspace, &found.refused, |shown| {
            format!("Report {shown} was not uploaded: it does not resolve inside the workspace")
        })?;
        for (file, kind) in found.found {
            let shown = shown_path(&options.workspace, &file);
            // a report is a regular file of its own: a link is refused, even to a file in the workspace
            if kind == Kind::Link {
                log.line(None, &format!("Report {shown} was not uploaded: it is a symbolic link"))?;
                continue;
            }
            if kind != Kind::File {
                continue;
            }
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let scratch = Scratch::new(&options.workspace, "report");
            if let Err(err) = collect::report(options, &file, &scratch.0, collecting) {
                log.line(None, &format!("Report {shown} was not uploaded: {err}"))?;
                continue;
            }
            let sent = transport.upload(
                &format!(
                    "artifacts?{}",
                    query(&[("kind", "report"), ("name", &name), ("path", pattern)])
                ),
                &scratch.0,
            );
            match sent {
                Ok((401 | 403 | 410, _)) => return Err(Stop::Gone),
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
        let found = match collect::find_in(options, pattern, collecting) {
            Ok(found) => found,
            Err(err) => {
                log.line(None, &format!("Artifact {name} was not read: {err}"))?;
                continue;
            }
        };
        log_refused(&mut log, &options.workspace, &found.refused, |shown| {
            format!("Artifact {name}: {shown} was left out, as it does not resolve inside the workspace")
        })?;
        let files: Vec<PathBuf> = found.found.into_iter().map(|(path, _)| path).collect();
        if files.is_empty() {
            log.line(None, &format!("Artifact {name}: nothing at {pattern}"))?;
            continue;
        }
        let scratch = Scratch::new(&options.workspace, "artifact.tar.gz");
        if let Err(err) = collect::pack(options, &files, &scratch.0, collecting) {
            log.line(None, &format!("Artifact {name} was not packed: {err}"))?;
            continue;
        }
        let days = artifact["retentionDays"].as_i64().map(|days| days.to_string());
        let mut pairs = vec![("kind", "artifact"), ("name", name), ("path", pattern)];
        if let Some(days) = &days {
            pairs.push(("retentionDays", days));
        }
        match transport.upload(&format!("artifacts?{}", query(&pairs)), &scratch.0) {
            Ok((401 | 403 | 410, _)) => return Err(Stop::Gone),
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
            let found = match collect::find_in(options, path, collecting) {
                Ok(found) => found,
                Err(err) => {
                    log.line(None, &format!("Cache {key} was not read: {err}"))?;
                    continue;
                }
            };
            log_refused(&mut log, &options.workspace, &found.refused, |shown| {
                format!("Cache {key}: {shown} was left out, as it does not resolve inside the workspace")
            })?;
            let dirs: Vec<PathBuf> = found.found.into_iter().map(|(path, _)| path).collect();
            if dirs.is_empty() {
                continue;
            }
            let scratch = Scratch::new(&options.workspace, "cache-out.tar.gz");
            if let Err(err) = collect::pack(options, &dirs, &scratch.0, collecting) {
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
                Ok((401 | 403 | 410, _)) => return Err(Stop::Gone),
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
        Ok((401 | 403 | 410, _)) => EXIT_GONE,
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
