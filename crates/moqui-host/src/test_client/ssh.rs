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

//! SSH of a client spec and a load script (plans/archive/LoadRunner.md L7): `ssh.connect` logs in to a server
//! (`crate::ssh::client::login`, the SFTP client's), `exec` runs one command, and `shell` asks for a pty and a shell to
//! `send` to and `expect` from. `agentApps` is the prelude's, on `exec`.
//!
//! The calls run on the edge runtime and the user's thread waits in short slices, so a load stopped now ends the wait
//! and a call never waits past its timeout. In a load a connect, an exec and an expect are samples of kind `ssh`: an
//! exec's outcome is its exit status (`exit 0`, an error from 1 unless `expect` lists it), an expect's `ok`, `timeout`
//! or `closed`.
//!
//! Host keys: a pinned `fingerprint` must match; a server on this machine, or any with `--insecure`, is taken; any
//! other is refused with its fingerprint, to pin.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_core::sftp::{SftpConfig, SftpKey};
use moqui_lib::sync::lock;
use russh::client::{Handle, Msg};
use russh::{Channel, ChannelMsg};
use serde_json::{json, Value};

use super::http::{is_loopback, Server};
use super::load::{self, Kind};
use crate::ssh::client::{login, KeyCheck, Trust};

/// The longest one wait of a load user's thread on an SSH call: a stop now waits no longer than this.
const SLICE: Duration = Duration::from_millis(100);

/// The shell output an `expect` keeps unmatched, at most: past it the oldest half goes.
const MAX_TEXT: usize = 1 << 20;

/// What the SSH calls need of the client host.
pub(crate) struct Env<'a> {
    pub(crate) server: &'a Server,
    pub(crate) insecure: bool,
    /// A call's wait when its `timeout` sets none, and the cap of one that does.
    pub(crate) timeout: Duration,
    pub(crate) cap: &'a dyn Fn(Duration) -> Duration,
    /// Whether the run may connect to `host:port` (a load run in a server's target rule).
    pub(crate) allow: &'a dyn Fn(&str, u16) -> Result<(), String>,
    /// A new id for a connection or a shell.
    pub(crate) id: &'a dyn Fn() -> u64,
}

struct Connection {
    handle: Arc<Handle<KeyCheck>>,
    /// `user@host:port`, for errors.
    target: String,
}

/// A pty and shell, its output so far as text.
struct Shell {
    connection: u64,
    channel: Channel<Msg>,
    screen: Screen,
    /// Why the channel ended, once it has.
    ended: Option<String>,
    /// The last send, or the last match: where an expect's time starts.
    since: Instant,
}

/// The connections and shells a run's specs or users opened.
#[derive(Default)]
pub(crate) struct Ssh {
    connections: Mutex<HashMap<u64, Connection>>,
    shells: Mutex<HashMap<u64, Arc<tokio::sync::Mutex<Shell>>>>,
}

/// Why an SSH call did not answer.
enum Failed {
    Timeout,
    /// The load was stopped now: [`load::check_stopped`]'s error.
    Stopped(String),
    Error(String),
}

/// Run `future` on the edge runtime, waiting in [`SLICE`]s: the wait ends at `timeout`, and for a load user on a stop
/// now. The future is dropped when the wait ends without its answer.
fn wait<T: Send + 'static>(
    future: impl Future<Output = Result<T, String>> + Send + 'static,
    timeout: Duration,
) -> Result<T, Failed> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let task = crate::edge::handle().spawn(async move {
        let _ = tx.send(future.await);
    });
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            task.abort();
            return Err(Failed::Timeout);
        }
        match rx.recv_timeout(left.min(SLICE)) {
            Ok(answer) => return answer.map_err(Failed::Error),
            Err(RecvTimeoutError::Timeout) => {
                if let Err(stopped) = load::check_stopped() {
                    task.abort();
                    return Err(Failed::Stopped(stopped));
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(Failed::Error("the SSH call ended before it answered".to_string()))
            }
        }
    }
}

fn field<'a>(args: &'a Value, name: &str) -> &'a Value {
    args.get(name).unwrap_or(&Value::Null)
}

fn text(args: &Value, name: &str) -> Option<String> {
    field(args, name).as_str().map(str::to_string)
}

/// A call's `expect`: the outcome labels a load does not count as errors; a number is an exit status (`exit 1`).
fn labels(args: &Value) -> Vec<String> {
    field(args, "expect")
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| match label {
                    Value::Number(number) => Some(format!("exit {number}")),
                    Value::String(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
                        Some(format!("exit {text}"))
                    }
                    Value::String(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Count a call in the user's load, if any, named by the call's `name` or `default`.
fn observe(
    args: &Value,
    default: impl FnOnce() -> String,
    outcome: Cow<'static, str>,
    failure: Option<String>,
    elapsed: Duration,
) -> Result<(), String> {
    let name = || text(args, "name").unwrap_or_else(default);
    load::observe_outcome(name, Kind::Ssh, outcome, failure, elapsed, &labels(args))
}

/// Count a call that did not answer, and answer its error.
fn observe_failed(
    args: &Value,
    default: impl FnOnce() -> String,
    failed: Failed,
    elapsed: Duration,
    what: &str,
) -> String {
    let (outcome, message) = match failed {
        // a stop asked for is not the server's error, and was counted as the stop
        Failed::Stopped(stopped) => return stopped,
        Failed::Timeout => (
            "timeout",
            format!("{what}: no answer within {:.1} s", elapsed.as_secs_f64()),
        ),
        Failed::Error(err) => ("error", format!("{what}: {err}")),
    };
    match observe(args, default, Cow::Borrowed(outcome), Some(message.clone()), elapsed) {
        Err(thrown) => thrown,
        Ok(()) => message,
    }
}

/// What an exec answered.
#[derive(Default)]
struct Executed {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    status: Option<u32>,
    /// The server refused the channel request (the command is not one this user may run).
    refused: bool,
}

async fn exec(handle: Arc<Handle<KeyCheck>>, command: String, input: Option<String>) -> Result<Executed, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|err| format!("could not open a channel: {err}"))?;
    channel.exec(true, command).await.map_err(|err| err.to_string())?;
    if let Some(input) = input {
        channel.data(input.as_bytes()).await.map_err(|err| err.to_string())?;
    }
    // the command reads no more input: one that reads its standard input to the end does not wait for more
    let _ = channel.eof().await;
    let mut executed = Executed::default();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => executed.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => executed.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => executed.status = Some(exit_status),
            ChannelMsg::Failure => {
                executed.refused = true;
                break;
            }
            _ => {}
        }
    }
    Ok(executed)
}

async fn open_shell(handle: Arc<Handle<KeyCheck>>, term: String, cols: u32, rows: u32) -> Result<Channel<Msg>, String> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|err| format!("could not open a channel: {err}"))?;
    channel
        .request_pty(true, &term, cols, rows, 0, 0, &[])
        .await
        .map_err(|err| format!("pty: {err}"))?;
    channel
        .request_shell(true)
        .await
        .map_err(|err| format!("shell: {err}"))?;
    Ok(channel)
}

/// What a shell's `expect` looks for.
fn pattern(args: &Value) -> Result<regex::Regex, String> {
    let flags = text(args, "flags").unwrap_or_default();
    let source = match (text(args, "literal"), text(args, "source")) {
        (Some(literal), _) => regex::escape(&literal),
        (None, Some(source)) => source,
        (None, None) => return Err("expect needs a string or a RegExp".to_string()),
    };
    regex::RegexBuilder::new(&source)
        .case_insensitive(flags.contains('i'))
        .multi_line(flags.contains('m'))
        .dot_matches_new_line(flags.contains('s'))
        .build()
        .map_err(|err| format!("expect: /{source}/ is not a pattern this harness reads: {err}"))
}

/// Wait on `shell` until its text matches `pattern`: the match, its groups and the text before it, which are taken
/// out of the shell's text. `None` when the channel ends first.
async fn expect(shell: Arc<tokio::sync::Mutex<Shell>>, pattern: regex::Regex) -> Result<Value, String> {
    let mut shell = shell.lock().await;
    loop {
        if let Some(found) = shell.screen.take_match(&pattern) {
            shell.since = Instant::now();
            return Ok(found);
        }
        if let Some(ended) = &shell.ended {
            return Err(ended.clone());
        }
        match shell.channel.wait().await {
            Some(ChannelMsg::Data { data }) => shell.screen.feed(&data),
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                shell.ended = Some(format!("the shell exited {exit_status}"))
            }
            Some(ChannelMsg::Failure) => shell.ended = Some("the server refused the pty or the shell".to_string()),
            Some(_) => {}
            None => shell.ended = Some("the shell closed".to_string()),
        }
    }
}

impl Ssh {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// One `ssh.*` call of the prelude.
    pub(crate) fn call(&self, name: &str, args: &Value, env: &Env<'_>) -> Result<Value, String> {
        match name {
            "ssh.connect" => self.connect(args, env),
            "ssh.exec" => self.exec(args, env),
            "ssh.shell" => self.shell(args, env),
            "ssh.send" => self.send(args, env),
            "ssh.expect" => self.expect(args, env),
            "ssh.closeShell" => {
                let id = field(args, "shell").as_u64().unwrap_or(0);
                self.close(&[id]);
                Ok(Value::Null)
            }
            "ssh.close" => {
                let id = field(args, "id").as_u64().unwrap_or(0);
                self.close(&[id]);
                Ok(Value::Null)
            }
            other => Err(format!("{other} is not a call of the client host")),
        }
    }

    fn timeout(args: &Value, env: &Env<'_>) -> Duration {
        let seconds = field(args, "timeout")
            .as_f64()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
        (env.cap)(seconds.map_or(env.timeout, load::seconds))
    }

    fn connect(&self, args: &Value, env: &Env<'_>) -> Result<Value, String> {
        let default = env.server.ssh.as_ref();
        let host = text(args, "host")
            .filter(|host| !host.is_empty())
            .or_else(|| default.map(|ssh| ssh.host.clone()))
            .unwrap_or_else(|| env.server.host.clone());
        let port = match field(args, "port").as_u64() {
            Some(port) => u16::try_from(port).map_err(|_| format!("ssh.connect: {port} is not a port"))?,
            None => default.map_or(moqui_core::sftp::SSH_PORT, |ssh| ssh.port),
        };
        let Some(user) = text(args, "user").filter(|user| !user.is_empty()) else {
            return Err("ssh.connect needs a user".to_string());
        };
        (env.allow)(&host, port)?;
        let fingerprint = text(args, "fingerprint").filter(|print| !print.is_empty());
        let trust = if env.insecure || is_loopback(&host) {
            Trust::Any
        } else {
            Trust::Nothing
        };
        let config = SftpConfig {
            host: host.clone(),
            port,
            username: user.clone(),
            password: text(args, "password"),
            keys: text(args, "key")
                .map(|private_key| SftpKey {
                    private_key,
                    public_key: None,
                })
                .into_iter()
                .collect(),
            host_key_fingerprint: fingerprint,
        };
        let target = format!("{user}@{host}:{port}");
        let timeout = Self::timeout(args, env);
        load::before_request()?;
        let started = Instant::now();
        let logged_in = wait(
            async move { login(&config, trust).await.map_err(|err| err.message().to_string()) },
            timeout,
        );
        let elapsed = started.elapsed();
        let default_name = || "ssh connect".to_string();
        let handle = match logged_in {
            Ok(handle) => handle,
            Err(failed) => return Err(observe_failed(args, default_name, failed, elapsed, &target)),
        };
        observe(args, default_name, Cow::Borrowed("ok"), None, elapsed)?;
        let id = (env.id)();
        lock(&self.connections).insert(
            id,
            Connection {
                handle: Arc::new(handle),
                target,
            },
        );
        load::track_session(id);
        Ok(json!({ "id": id }))
    }

    fn connection(&self, args: &Value) -> Result<(Arc<Handle<KeyCheck>>, String), String> {
        let id = field(args, "id").as_u64().unwrap_or(0);
        lock(&self.connections)
            .get(&id)
            .map(|connection| (Arc::clone(&connection.handle), connection.target.clone()))
            .ok_or_else(|| "the SSH connection is closed".to_string())
    }

    fn exec(&self, args: &Value, env: &Env<'_>) -> Result<Value, String> {
        let (handle, target) = self.connection(args)?;
        let command = text(args, "command").unwrap_or_default();
        if command.trim().is_empty() {
            return Err("exec needs a command".to_string());
        }
        let first = command.split_whitespace().next().unwrap_or("").to_string();
        let default_name = || format!("ssh {first}");
        let timeout = Self::timeout(args, env);
        load::before_request()?;
        let started = Instant::now();
        let ran = wait(exec(handle, command, text(args, "input")), timeout);
        let elapsed = started.elapsed();
        let executed = match ran {
            Ok(executed) => executed,
            Err(failed) => return Err(observe_failed(args, default_name, failed, elapsed, &target)),
        };
        let stdout = String::from_utf8_lossy(&executed.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&executed.stderr).into_owned();
        let (outcome, failure) = match executed.status {
            _ if executed.refused => (Cow::Borrowed("refused"), Some(format!("{target} refused the command"))),
            Some(0) => (Cow::Borrowed("exit 0"), None),
            Some(status) => {
                let said: String = if stderr.trim().is_empty() { &stdout } else { &stderr }
                    .chars()
                    .take(200)
                    .collect();
                (
                    Cow::Owned(format!("exit {status}")),
                    Some(format!("exit {status}: {}", said.trim())),
                )
            }
            None => (
                Cow::Borrowed("closed"),
                Some("the channel closed with no exit status".to_string()),
            ),
        };
        // a refusal is never expected: the command did not run
        if executed.refused {
            let message = failure.clone().unwrap_or_default();
            load::observe_outcome(default_name, Kind::Ssh, outcome, failure, elapsed, &[])?;
            return Err(message);
        }
        observe(args, default_name, outcome, failure, elapsed)?;
        Ok(json!({ "status": executed.status, "stdout": stdout, "stderr": stderr }))
    }

    fn shell(&self, args: &Value, env: &Env<'_>) -> Result<Value, String> {
        let (handle, target) = self.connection(args)?;
        let connection = field(args, "id").as_u64().unwrap_or(0);
        let cols = field(args, "cols").as_u64().map_or(120, |cols| cols.clamp(20, 500)) as u32;
        let rows = field(args, "rows").as_u64().map_or(40, |rows| rows.clamp(5, 500)) as u32;
        let term = text(args, "term").unwrap_or_else(|| "xterm-256color".to_string());
        load::check_stopped()?;
        let channel =
            wait(open_shell(handle, term, cols, rows), Self::timeout(args, env)).map_err(|failed| match failed {
                Failed::Stopped(stopped) => stopped,
                Failed::Timeout => format!("{target}: no shell within the timeout"),
                Failed::Error(err) => format!("{target}: {err}"),
            })?;
        let id = (env.id)();
        let shell = Shell {
            connection,
            channel,
            screen: Screen::default(),
            ended: None,
            since: Instant::now(),
        };
        lock(&self.shells).insert(id, Arc::new(tokio::sync::Mutex::new(shell)));
        load::track_session(id);
        Ok(json!({ "id": id }))
    }

    fn shell_of(&self, args: &Value) -> Result<Arc<tokio::sync::Mutex<Shell>>, String> {
        let id = field(args, "shell").as_u64().unwrap_or(0);
        lock(&self.shells)
            .get(&id)
            .cloned()
            .ok_or_else(|| "the shell is closed".to_string())
    }

    fn send(&self, args: &Value, env: &Env<'_>) -> Result<Value, String> {
        let shell = self.shell_of(args)?;
        let bytes = text(args, "text").unwrap_or_default().into_bytes();
        load::before_request()?;
        let sent = wait(
            async move {
                let mut shell = shell.lock().await;
                shell.since = Instant::now();
                shell.channel.data(&bytes[..]).await.map_err(|err| err.to_string())
            },
            Self::timeout(args, env),
        );
        match sent {
            Ok(()) => Ok(Value::Null),
            Err(Failed::Stopped(stopped)) => Err(stopped),
            Err(Failed::Timeout) => Err("send: the shell took no input within the timeout".to_string()),
            Err(Failed::Error(err)) => Err(format!("send: {err}")),
        }
    }

    fn expect(&self, args: &Value, env: &Env<'_>) -> Result<Value, String> {
        let shell = self.shell_of(args)?;
        let pattern = pattern(args)?;
        let shown = pattern.as_str().chars().take(80).collect::<String>();
        let default_name = || "ssh expect".to_string();
        let timeout = Self::timeout(args, env);
        load::check_stopped()?;
        let waited = Instant::now();
        // the time is from the last send or match, which may be before this call
        let since = shell.try_lock().map_or(waited, |shell| shell.since);
        let found = wait(expect(Arc::clone(&shell), pattern), timeout);
        let elapsed = since.elapsed();
        match found {
            Ok(found) => {
                observe(args, default_name, Cow::Borrowed("ok"), None, elapsed)?;
                Ok(found)
            }
            Err(Failed::Stopped(stopped)) => Err(stopped),
            Err(failed) => {
                let (outcome, failure) = match failed {
                    Failed::Timeout => {
                        let seen = shell.try_lock().map(|shell| shell.screen.tail(300)).unwrap_or_default();
                        let failure = format!(
                            "/{shown}/ did not come within {:.1} s; the end of the output:\n{seen}",
                            timeout.as_secs_f64()
                        );
                        ("timeout", failure)
                    }
                    Failed::Error(ended) | Failed::Stopped(ended) => ("closed", format!("{ended} before /{shown}/")),
                };
                observe(
                    args,
                    default_name,
                    Cow::Borrowed(outcome),
                    Some(failure.clone()),
                    elapsed,
                )?;
                // null only where expect names the outcome; else it throws, in a spec as in a load's setup
                if labels(args).iter().any(|label| label == outcome) {
                    Ok(Value::Null)
                } else {
                    Err(failure)
                }
            }
        }
    }

    /// Close the connections and shells among `ids`, and the shells of a connection closed.
    pub(crate) fn close(&self, ids: &[u64]) {
        let connections: Vec<(u64, Connection)> = {
            let mut held = lock(&self.connections);
            ids.iter()
                .filter_map(|id| held.remove(id).map(|connection| (*id, connection)))
                .collect()
        };
        let shells: Vec<Arc<tokio::sync::Mutex<Shell>>> = {
            let mut held = lock(&self.shells);
            let closed: Vec<u64> = held
                .iter()
                .filter(|(id, shell)| {
                    ids.contains(id)
                        || shell.try_lock().is_ok_and(|shell| {
                            connections
                                .iter()
                                .any(|(connection, _)| *connection == shell.connection)
                        })
                })
                .map(|(id, _)| *id)
                .collect();
            closed.iter().filter_map(|id| held.remove(id)).collect()
        };
        disconnect(
            connections.into_iter().map(|(_, connection)| connection).collect(),
            shells,
        );
    }

    /// Close everything: after a spec, and when the host goes.
    pub(crate) fn close_all(&self) {
        let connections: Vec<Connection> = lock(&self.connections)
            .drain()
            .map(|(_, connection)| connection)
            .collect();
        let shells: Vec<_> = lock(&self.shells).drain().map(|(_, shell)| shell).collect();
        disconnect(connections, shells);
    }
}

impl Drop for Ssh {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// Close `shells` and log out of `connections` on the edge runtime, without waiting for the servers.
fn disconnect(connections: Vec<Connection>, shells: Vec<Arc<tokio::sync::Mutex<Shell>>>) {
    if connections.is_empty() && shells.is_empty() {
        return;
    }
    crate::edge::handle().spawn(async move {
        for shell in shells {
            let shell = shell.lock().await;
            let _ = shell.channel.close().await;
        }
        for connection in connections {
            let _ = connection
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    });
}

/// What a pty wrote, as text: escape sequences taken out, a cursor move a line break, `\r` dropped. Bytes are read as
/// they come, so a sequence or a character split across two reads is still whole.
#[derive(Default)]
pub(crate) struct Screen {
    text: String,
    /// The start of a UTF-8 character the next read finishes.
    partial: Vec<u8>,
    state: Escape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    Text,
    /// After `ESC`.
    Esc,
    /// `ESC [` up to its final byte.
    Csi,
    /// `ESC ]` up to `BEL` or `ESC \`.
    Osc,
    /// `ESC` inside an OSC.
    OscEsc,
    /// `ESC (` or `ESC )`: one more character, the charset.
    Charset,
}

impl Screen {
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
        let ready = match std::str::from_utf8(&self.partial) {
            Ok(_) => self.partial.len(),
            // only a character cut at the end waits; a bad byte inside reads as U+FFFD
            Err(err) if err.error_len().is_none() => err.valid_up_to(),
            Err(_) => self.partial.len(),
        };
        let bytes: Vec<u8> = self.partial.drain(..ready).collect();
        for ch in String::from_utf8_lossy(&bytes).chars() {
            self.push(ch);
        }
        if self.text.len() > MAX_TEXT {
            let mut cut = self.text.len() - MAX_TEXT / 2;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }

    fn push(&mut self, ch: char) {
        self.state = match (self.state, ch) {
            (Escape::Text, '\x1b') => Escape::Esc,
            (Escape::Text, '\n' | '\t') => {
                self.text.push(ch);
                Escape::Text
            }
            (Escape::Text, ch) if ch.is_control() => Escape::Text,
            (Escape::Text, ch) => {
                self.text.push(ch);
                Escape::Text
            }
            (Escape::Esc, '[') => Escape::Csi,
            (Escape::Esc, ']') => Escape::Osc,
            (Escape::Esc, '(' | ')') => Escape::Charset,
            (Escape::Esc, _) | (Escape::Charset, _) => Escape::Text,
            (Escape::Csi, '\x40'..='\x7e') => {
                // a style is not a move; a move ends a run of text
                if ch != 'm' && !self.text.ends_with('\n') {
                    self.text.push('\n');
                }
                Escape::Text
            }
            (Escape::Csi, _) => Escape::Csi,
            (Escape::Osc, '\x07') => Escape::Text,
            (Escape::Osc, '\x1b') => Escape::OscEsc,
            (Escape::Osc, _) => Escape::Osc,
            (Escape::OscEsc, '\\') => Escape::Text,
            (Escape::OscEsc, _) => Escape::Osc,
        };
    }

    /// The first match of `pattern` in the text: `{ match, groups, before }`, the text up to its end taken out.
    pub(crate) fn take_match(&mut self, pattern: &regex::Regex) -> Option<Value> {
        let captures = pattern.captures(&self.text)?;
        let whole = captures.get(0)?;
        let groups: Vec<Value> = captures
            .iter()
            .skip(1)
            .map(|group| group.map_or(Value::Null, |group| Value::String(group.as_str().to_string())))
            .collect();
        let found = json!({
            "match": whole.as_str(),
            "groups": groups,
            "before": &self.text[..whole.start()],
        });
        let end = whole.end();
        self.text.drain(..end);
        Some(found)
    }

    /// The last `chars` characters of the text, for an error.
    pub(crate) fn tail(&self, chars: usize) -> String {
        let count = self.text.chars().count();
        self.text.chars().skip(count.saturating_sub(chars)).collect()
    }
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
