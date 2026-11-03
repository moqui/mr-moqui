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

//! The node as the `moqui` binary in a child process of the app (plans/Mobile-Desktop.md, configuration 1). The app
//! starts it on the runtime with the loopback listen properties, takes it as started at its `Listening on` line once
//! `/health` answers ready, and stops it by closing its stdin (`moqui_stop_on_stdin_close`), then with SIGTERM, then a
//! kill. The app ending closes that pipe too, so the node never outlives it. A restart the node asks for replaces the
//! process in place on Unix (same pid and pipes), and its next `Listening on` line is a second start.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;

use super::Event;

/// How long a node has after its stdin closes to stop by itself, Postgres included, before SIGTERM.
const STOP_GRACE: Duration = Duration::from_secs(30);
/// How long after SIGTERM before the kill.
const TERM_GRACE: Duration = Duration::from_secs(10);
/// How long `/health` has to answer ready after the `Listening on` line. The boot, a first start's seed data load
/// included, is done by then; there is no limit before the line.
const READY_TIMEOUT: Duration = Duration::from_secs(60);
/// The output lines kept for the message when the node ends by itself.
const TAIL_LINES: usize = 50;

/// The `moqui` child and the threads that read its output and watch for its end.
pub(crate) struct Node {
    child: Arc<Mutex<Child>>,
    /// Held open for the node's life: closing it stops the node.
    stdin: Option<ChildStdin>,
    stopping: Arc<AtomicBool>,
    watch: JoinHandle<()>,
}

impl Node {
    /// Start `moqui` on `runtime`, listening on 127.0.0.1 at `port`. `events` hears when it starts, fails or ends by
    /// itself, on the threads that read and watch it.
    pub(super) fn start(
        runtime: &Path,
        port: u16,
        properties: &[(&'static str, String)],
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let binary = binary().map_err(std::io::Error::other)?;
        Self::start_binary(&binary, runtime, port, properties, events)
    }

    /// [`Self::start`] with `binary` as `moqui`.
    fn start_binary(
        binary: &Path,
        runtime: &Path,
        port: u16,
        properties: &[(&'static str, String)],
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        tracing::info!("Starting the sidecar node {}", binary.display());
        let mut command = Command::new(binary);
        command
            .args(args(port, properties))
            .env("MOQUI_RUNTIME", runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // A process group of its own: a signal to the app's group (ctrl-c in a terminal, `cargo tauri dev` replacing
        // the app) reaches only the app, and its end closes the node's stdin, which stops the node gracefully. Killed
        // with the app's group, the node would leave its Postgres for the next start to take over.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        // CEF opens descriptors without close-on-exec (a development run's DevTools socket among them), which the node,
        // and the Postgres it starts, would hold past the app's end
        #[cfg(unix)]
        close_fds::set_fds_cloexec_threadsafe(3, &[]);
        let mut child = command
            .spawn()
            .map_err(|err| std::io::Error::other(format!("could not start {}: {err}", binary.display())))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let events: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(events);
        let stopping = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let (done, readers_done) = channel::<()>();
        if let Some(stdout) = stdout {
            let reader = Reader {
                tail: Arc::clone(&tail),
                events: Arc::clone(&events),
                stopping: Arc::clone(&stopping),
                exited: Arc::clone(&exited),
                done: done.clone(),
            };
            spawn("moqui-desktop-node-out", move || reader.stdout(stdout))?;
        }
        if let Some(stderr) = stderr {
            let reader = Reader {
                tail: Arc::clone(&tail),
                events: Arc::clone(&events),
                stopping: Arc::clone(&stopping),
                exited: Arc::clone(&exited),
                done,
            };
            spawn("moqui-desktop-node-err", move || reader.stderr(stderr))?;
        }
        let child = Arc::new(Mutex::new(child));
        let watched = Arc::clone(&child);
        let watch_stopping = Arc::clone(&stopping);
        let watch = spawn("moqui-desktop-node-watch", move || {
            let status = loop {
                match lock(&watched).try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {}
                    Err(err) => {
                        tracing::error!("Could not watch the sidecar node: {err}");
                        break None;
                    }
                }
                if watch_stopping.load(Ordering::SeqCst) {
                    // `stop` waits for the child from here on.
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            exited.store(true, Ordering::SeqCst);
            // The readers have the last lines once the pipes end; a process the node started that kept them open
            // (none should) does not hold this up for long.
            for _ in 0..2 {
                let _ = readers_done.recv_timeout(Duration::from_secs(1));
            }
            if !watch_stopping.load(Ordering::SeqCst) {
                let status = status.map_or_else(|| "unknown".to_string(), |status| status.to_string());
                let tail = lock(&tail);
                tracing::error!(
                    "The sidecar node ended by itself ({status}); its last output:\n{}",
                    tail_text(&tail)
                );
                events(Event::Failed(ended_message(&status, &tail)));
            }
        })?;
        Ok(Self {
            child,
            stdin,
            stopping,
            watch,
        })
    }

    /// Stop the node and wait for it: close its stdin, then SIGTERM after [`STOP_GRACE`], then kill after
    /// [`TERM_GRACE`].
    pub(super) fn stop(mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        if self.watch.join().is_err() {
            tracing::error!("The sidecar node's watch thread panicked");
        }
        match stop_child(&self.child, self.stdin.take(), STOP_GRACE, TERM_GRACE) {
            Some(status) => tracing::info!("The sidecar node stopped ({status})"),
            None => tracing::error!("The sidecar node did not stop"),
        }
    }
}

/// What reads one of the child's output pipes.
struct Reader {
    tail: Arc<Mutex<VecDeque<String>>>,
    events: Arc<dyn Fn(Event) + Send + Sync>,
    stopping: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    done: Sender<()>,
}

impl Reader {
    /// The node's stdout: its `Listening on` line, then whatever else it prints, to the log. Each `Listening on` line
    /// (the first, and one after each restart in place) is a start once `/health` answers ready.
    fn stdout(self, stdout: impl Read) {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            tracing::info!(target: "moqui_desktop::sidecar", "{line}");
            let address = listening_address(&line);
            self.keep(line);
            let Some(address) = address else {
                continue;
            };
            match wait_ready(address, READY_TIMEOUT, || {
                self.stopping.load(Ordering::SeqCst) || self.exited.load(Ordering::SeqCst)
            }) {
                Ok(true) => (self.events)(Event::Started(address)),
                // Stopping, or ended: the watch says why.
                Ok(false) => {}
                Err(err) => {
                    let tail = lock(&self.tail);
                    tracing::error!(
                        "The sidecar node did not get ready: {err}; its last output:\n{}",
                        tail_text(&tail)
                    );
                    (self.events)(Event::Failed(format!("{err}\n\n{}", summary(&tail).join("\n"))));
                }
            }
        }
        let _ = self.done.send(());
    }

    /// The node's stderr, its console log. Its log files already have these lines, so they are echoed to the app's
    /// stderr rather than to its log (one runtime, one set of files), and kept for the message if it ends.
    fn stderr(self, stderr: impl Read) {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = writeln!(std::io::stderr(), "{line}");
            self.keep(line);
        }
        let _ = self.done.send(());
    }

    fn keep(&self, line: String) {
        let mut tail = lock(&self.tail);
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

fn spawn(name: &str, run: impl FnOnce() + Send + 'static) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name(name.to_string()).spawn(run)
}

/// The `moqui` binary of this run ([`find_binary`]).
pub(super) fn binary() -> Result<PathBuf, String> {
    find_binary(
        std::env::var_os("MOQUI_DESKTOP_MOQUI").map(PathBuf::from),
        tauri::is_dev(),
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"),
        std::env::current_exe().ok().as_deref().and_then(Path::parent),
    )
}

/// The `moqui` binary: `MOQUI_DESKTOP_MOQUI` (`env`); in a development run, the release then the debug build under
/// this repository's `target`; in a bundle, `moqui` beside the app's executable (`exe_dir`), where `bundle.externalBin`
/// puts it.
pub(super) fn find_binary(
    env: Option<PathBuf>,
    dev: bool,
    target: &Path,
    exe_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let name = format!("moqui{}", std::env::consts::EXE_SUFFIX);
    if let Some(path) = env.filter(|path| !path.as_os_str().is_empty()) {
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "MOQUI_DESKTOP_MOQUI is {}, which is not a file",
                path.display()
            ))
        };
    }
    let candidates: Vec<PathBuf> = if dev {
        vec![target.join("release").join(&name), target.join("debug").join(&name)]
    } else {
        exe_dir.map(|dir| dir.join(&name)).into_iter().collect()
    };
    candidates.iter().find(|path| path.is_file()).cloned().ok_or_else(|| {
        let looked: Vec<String> = candidates.iter().map(|path| path.display().to_string()).collect();
        let hint = if dev { "; run ./moq build first" } else { "" };
        format!(
            "the moqui binary for the sidecar node is not at {}{hint} (or set MOQUI_DESKTOP_MOQUI)",
            looked.join(" or ")
        )
    })
}

/// The arguments of the node: the listen properties, `properties`, and stopping when the app closes its stdin.
pub(super) fn args(port: u16, properties: &[(&'static str, String)]) -> Vec<String> {
    let mut args = Vec::new();
    for (name, value) in super::listen_properties(port)
        .into_iter()
        .chain(properties.iter().cloned())
        .chain([("moqui_stop_on_stdin_close", "true".to_string())])
    {
        args.push("-D".to_string());
        args.push(format!("{name}={value}"));
    }
    args
}

/// The address of `moqui`'s `Listening on <address>` line (`moqui_host::serve`).
pub(super) fn listening_address(line: &str) -> Option<SocketAddr> {
    line.trim()
        .strip_prefix("Listening on ")?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Whether an HTTP response from `/health` says ready (`{"ready":true}`).
pub(super) fn health_ready(response: &str) -> bool {
    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    head.starts_with("HTTP/1.1 200") && body.replace(char::is_whitespace, "").contains("\"ready\":true")
}

/// Ask `/health` at `address` until it answers ready (true), `cancel` is true (false), or `timeout` passes (an error).
fn wait_ready(address: SocketAddr, timeout: Duration, cancel: impl Fn() -> bool) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel() {
            return Ok(false);
        }
        if get_health(address).is_some_and(|response| health_ready(&response)) {
            return Ok(true);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the node listens on {address} but /health did not answer ready within {} seconds",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `GET /health` at `address`, the whole response; None when it does not answer.
pub(super) fn get_health(address: SocketAddr) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET /health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAccept: application/json\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

/// Stop `child`: close `stdin`, wait `grace` for it to end, SIGTERM it (Unix), wait `term_grace`, then kill it. Its
/// status, waited for in every case; None only when waiting fails.
pub(super) fn stop_child(
    child: &Mutex<Child>,
    stdin: Option<ChildStdin>,
    grace: Duration,
    term_grace: Duration,
) -> Option<ExitStatus> {
    drop(stdin);
    if let Some(status) = wait_for(child, grace) {
        return Some(status);
    }
    #[cfg(unix)]
    {
        let pid = lock(child).id();
        tracing::warn!(
            "The sidecar node did not stop {} seconds after its stdin closed; sending SIGTERM",
            grace.as_secs()
        );
        if let Ok(pid) = libc::pid_t::try_from(pid) {
            // SAFETY: kill(2) takes any pid and signal number; the child is not yet waited for, so its pid is not reused.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        if let Some(status) = wait_for(child, term_grace) {
            return Some(status);
        }
    }
    #[cfg(not(unix))]
    let _ = term_grace;
    tracing::warn!("The sidecar node did not stop; killing it");
    let mut child = lock(child);
    if let Err(err) = child.kill() {
        tracing::error!("Could not kill the sidecar node: {err}");
    }
    child.wait().ok()
}

/// The child's status once it ends within `timeout`.
fn wait_for(child: &Mutex<Child>, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = lock(child).try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The most lines of the output a message shows: warnings and errors, else the last lines.
const SUMMARY_PROBLEMS: usize = 8;
const SUMMARY_LAST: usize = 5;
/// The longest line a message shows, in characters; more is cut.
const SUMMARY_LINE_MAX: usize = 300;

/// The message for a node that ended by itself: its status and the lines of its output most likely to say why
/// ([`summary`]). The whole tail goes to the app's log.
pub(super) fn ended_message(status: &str, tail: &VecDeque<String>) -> String {
    let lines = summary(tail);
    if lines.is_empty() {
        format!("The node ended ({status}).")
    } else {
        format!("The node ended ({status}):\n\n{}", lines.join("\n"))
    }
}

/// The lines of `tail` that say why a node failed: its last warnings and errors when there are any, else its last
/// lines; a boot that went well writes many lines that say nothing of the failure. Each cut to
/// [`SUMMARY_LINE_MAX`] characters.
pub(super) fn summary(tail: &VecDeque<String>) -> Vec<String> {
    let problems: Vec<&String> = tail.iter().filter(|line| is_problem(line)).collect();
    let chosen: Vec<&String> = if problems.is_empty() {
        tail.iter().skip(tail.len().saturating_sub(SUMMARY_LAST)).collect()
    } else {
        problems[problems.len().saturating_sub(SUMMARY_PROBLEMS)..].to_vec()
    };
    chosen.into_iter().map(|line| clip(line, SUMMARY_LINE_MAX)).collect()
}

/// A warning or an error: a log line at WARN or ERROR (its level is in the first few words,
/// `moqui_host::logging`), an `Error:` line or a panic.
fn is_problem(line: &str) -> bool {
    line.split_whitespace()
        .take(4)
        .any(|word| word == "WARN" || word == "ERROR")
        || line.trim_start().starts_with("Error")
        || line.trim_start().starts_with("error:")
        || line.contains("panicked at")
}

fn clip(line: &str, max: usize) -> String {
    match line.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_string(),
    }
}

fn tail_text(tail: &VecDeque<String>) -> String {
    tail.iter().map(String::as_str).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
#[path = "sidecar_tests.rs"]
mod tests;
