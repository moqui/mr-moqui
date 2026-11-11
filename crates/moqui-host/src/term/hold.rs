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

//! The holder (plans/Terminal-Fancy.md §7.5, D14): `moqui term-hold`, a process the server starts on its first host
//! session, one per runtime and protocol version, in a process group of its own so it outlives the server: a restart
//! of the server (`./moq run` again, Restart Now, a rebuild) finds it and its sessions. It owns the ptys, a ring of
//! each session's output and its tracker; the server keeps the limits, the permissions and the audit, and the holder
//! trusts the owner of its socket and nothing else (`0600`, in a `0700` directory under the runtime's `tmp`).
//!
//! The protocol is a line of JSON and its answer, a line of JSON, on a connection of its own. `hello`, `list`,
//! `status`, `resize`, `signal` and `forget` answer and close; `list` and `forget` are kept the same in every version,
//! so a server can end the sessions of a holder of an earlier build. `open` and `attach` answer and the connection then
//! carries the session's bytes both ways, raw: the program's output from that point (`attach` with `replay` sends the
//! modes in force and the ring first) and its input. A session has one such connection; a new one takes it. When the
//! program ends its connection closes, after the holder has waited for it, and `status` says how it ended.
//!
//! The holder ends itself a minute after its last session ended, and at once (its programs hung up) when its runtime
//! directory is gone.

use std::collections::HashMap;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use moqui_lib::sync::lock;
use rustix::process::{Pid, Signal};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};

use super::exec;
use super::ring::Ring;
use super::track::Tracker;

/// The holder's protocol. A server speaks to the holder of its own version; one of another is an earlier build's.
pub(crate) const PROTOCOL: u32 = 1;
/// Where the holder's runtime directory is named for it.
pub(crate) const HOLD_ENV: &str = "MOQUI_TERM_HOLD";
/// The holder ends this long after its last session ended.
const LINGER: Duration = Duration::from_secs(60);
/// An ended session nobody asked about is forgotten after this long.
const ENDED_KEEP: Duration = Duration::from_secs(600);
/// How often the holder looks at its directory and its sessions.
const TICK: Duration = Duration::from_secs(2);

/// The holder's socket directory under `runtime`.
pub(crate) fn socket_dir(runtime: &Path) -> PathBuf {
    runtime.join("tmp").join("term-hold")
}

/// The socket of the holder of protocol `version`.
pub(crate) fn socket_path(dir: &Path, version: u32) -> PathBuf {
    dir.join(format!("hold-v{version}.sock"))
}

/// One session the holder keeps.
struct Held {
    /// What the server said of it when it opened it (its user, profile, title): answered by `list`.
    meta: serde_json::Value,
    master: Arc<AsyncFd<OwnedFd>>,
    pid: Option<Pid>,
    output: Mutex<Output>,
    /// The connection the output goes to now, if any.
    subscriber: tokio::sync::Mutex<Option<OwnedWriteHalf>>,
    /// The program's exit status and when it ended.
    ended: Mutex<Option<(u32, Instant)>>,
}

struct Output {
    ring: Ring,
    tracker: Tracker,
    count: u64,
}

impl Held {
    /// What a window that comes back is sent first: the modes in force where the kept output starts, and that output.
    fn replay(&self) -> Vec<u8> {
        let output = lock(&self.output);
        let (_, bytes, modes) = output.ring.replay_from_rest(output.count, output.tracker.modes());
        let mut replay = modes.preamble();
        replay.extend_from_slice(&bytes);
        replay
    }
}

type Sessions = Arc<Mutex<HashMap<String, Arc<Held>>>>;

/// `moqui term-hold`: serve the holder's socket in the directory [`HOLD_ENV`] names until it ends itself. Answers the
/// exit status.
pub fn term_hold() -> i32 {
    let Some(dir) = std::env::var_os(HOLD_ENV).map(PathBuf::from) else {
        eprintln!("term-hold needs {HOLD_ENV}");
        return 2;
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("moqui-term-hold")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("term-hold: {err}");
            return 1;
        }
    };
    match runtime.block_on(serve(&dir)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("term-hold: {err}");
            1
        }
    }
}

/// The socket directory, `0700`, made when it is not there.
pub(crate) fn make_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.is_dir() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

async fn serve(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    make_dir(dir)?;
    let path = socket_path(dir, PROTOCOL);
    // a holder that answers is there already; a socket file nobody answers is left from one that is gone
    if UnixStream::connect(&path).await.is_ok() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
    let mut idle_since = Some(Instant::now());
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    tokio::spawn(connection(stream, Arc::clone(&sessions)));
                }
            }
            _ = tick.tick() => {
                if !dir.is_dir() || !path.exists() {
                    for held in lock(&sessions).values() {
                        exec::signal(held.pid, Signal::HUP);
                    }
                    return Ok(());
                }
                let running = {
                    let mut sessions = lock(&sessions);
                    sessions.retain(|_, held| {
                        lock(&held.ended).is_none_or(|(_, at)| at.elapsed() < ENDED_KEEP)
                    });
                    sessions.values().filter(|held| lock(&held.ended).is_none()).count()
                };
                if running > 0 {
                    idle_since = None;
                } else if idle_since.get_or_insert_with(Instant::now).elapsed() >= LINGER {
                    let _ = std::fs::remove_file(&path);
                    return Ok(());
                }
            }
        }
    }
}

/// Read a line of JSON, one byte at a time, so nothing past it is taken from the stream.
pub(crate) async fn read_line<R: tokio::io::AsyncRead + Unpin>(stream: &mut R) -> io::Result<serde_json::Value> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte).await? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the line did not end"));
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > 1 << 20 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "the line is too long"));
        }
    }
    serde_json::from_slice(&line).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

pub(crate) async fn write_line<W: tokio::io::AsyncWrite + Unpin>(
    stream: &mut W,
    value: &serde_json::Value,
) -> io::Result<()> {
    let mut line = value.to_string().into_bytes();
    line.push(b'\n');
    stream.write_all(&line).await
}

fn text<'a>(request: &'a serde_json::Value, key: &str) -> &'a str {
    request.get(key).and_then(serde_json::Value::as_str).unwrap_or_default()
}

fn dimension(request: &serde_json::Value, key: &str, default: u16) -> u16 {
    request
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(default)
}

fn error(message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({"error": message.into()})
}

async fn connection(mut stream: UnixStream, sessions: Sessions) {
    let Ok(request) = read_line(&mut stream).await else {
        return;
    };
    let find = |id: &str| lock(&sessions).get(id).cloned();
    let answer = match text(&request, "op") {
        "hello" => serde_json::json!({"version": PROTOCOL, "pid": std::process::id()}),
        "list" => {
            let listed: Vec<serde_json::Value> = lock(&sessions)
                .iter()
                .map(|(id, held)| {
                    let ended = *lock(&held.ended);
                    serde_json::json!({"id": id, "meta": held.meta, "ended": ended.map(|(status, _)| status)})
                })
                .collect();
            serde_json::json!({"sessions": listed})
        }
        "status" => match find(text(&request, "id")) {
            Some(held) => serde_json::json!({"ended": lock(&held.ended).map(|(status, _)| status)}),
            None => error("no such session"),
        },
        "resize" => match find(text(&request, "id")) {
            Some(held) => {
                exec::resize(
                    &held.master,
                    dimension(&request, "cols", 80),
                    dimension(&request, "rows", 24),
                );
                serde_json::json!({})
            }
            None => error("no such session"),
        },
        "signal" => match find(text(&request, "id")) {
            Some(held) => {
                let signal = if text(&request, "signal") == "KILL" {
                    Signal::KILL
                } else {
                    Signal::HUP
                };
                exec::signal(held.pid, signal);
                serde_json::json!({})
            }
            None => error("no such session"),
        },
        "forget" => {
            let id = text(&request, "id");
            let removed = lock(&sessions).remove(id);
            if let Some(held) = removed {
                if lock(&held.ended).is_none() {
                    exec::signal(held.pid, Signal::HUP);
                }
            }
            serde_json::json!({})
        }
        "open" => {
            open(stream, &request, &sessions).await;
            return;
        }
        "attach" => {
            let id = text(&request, "id").to_string();
            match find(&id) {
                Some(held) if lock(&held.ended).is_none() => {
                    let replay = request
                        .get("replay")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    subscribe(stream, held, replay).await;
                    return;
                }
                _ => error("no such session"),
            }
        }
        other => error(format!("unknown op {other}")),
    };
    let _ = write_line(&mut stream, &answer).await;
}

/// `open`: start the program and make this connection its stream.
async fn open(mut stream: UnixStream, request: &serde_json::Value, sessions: &Sessions) {
    let id = text(request, "id").to_string();
    let strings = |key: &str| -> Vec<String> {
        request
            .get(key)
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let argv = strings("argv");
    let helper = strings("helper");
    let env: Vec<(String, String)> = request
        .get("env")
        .and_then(serde_json::Value::as_array)
        .map(|pairs| {
            pairs
                .iter()
                .filter_map(|pair| {
                    let pair = pair.as_array()?;
                    Some((pair.first()?.as_str()?.to_string(), pair.get(1)?.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let arg0 = request.get("arg0").and_then(serde_json::Value::as_str);
    let replay = request
        .get("replay")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(2048 * 1024);
    let (Some((program, helper_args)), false, false) = (helper.split_first(), argv.is_empty(), id.is_empty()) else {
        let _ = write_line(&mut stream, &error("open needs id, argv and helper")).await;
        return;
    };
    let started = exec::start(
        &(program.clone(), helper_args.to_vec()),
        &argv,
        arg0,
        Path::new(text(request, "directory")),
        &env,
        dimension(request, "cols", 80),
        dimension(request, "rows", 24),
    );
    let started = match started {
        Ok(started) => started,
        Err(err) => {
            let _ = write_line(&mut stream, &error(format!("{} did not start: {err}", argv.join(" ")))).await;
            return;
        }
    };
    let exec::Started {
        mut reader,
        writer,
        master,
        pid,
        child,
    } = started;
    let held = Arc::new(Held {
        meta: request.get("meta").cloned().unwrap_or(serde_json::Value::Null),
        master,
        pid,
        output: Mutex::new(Output {
            ring: Ring::new(usize::try_from(replay).unwrap_or(2048 * 1024)),
            tracker: Tracker::default(),
            count: 0,
        }),
        subscriber: tokio::sync::Mutex::new(None),
        ended: Mutex::new(None),
    });
    lock(sessions).insert(id, Arc::clone(&held));
    // the program's output, to the ring and to whichever connection has it; its end is waited for before the connection
    // closes, so a status asked then is known
    let pump = Arc::clone(&held);
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    tokio::spawn(async move {
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            let read = match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let bytes = &buffer[..read];
            let mut subscriber = pump.subscriber.lock().await;
            {
                let mut output = lock(&pump.output);
                let changes = output.tracker.feed(bytes);
                let at = output.count;
                output.ring.push(bytes, at, &changes.rests);
                output.count += read as u64;
            }
            if let Some(connection) = subscriber.as_mut() {
                if connection.write_all(bytes).await.is_err() {
                    *subscriber = None;
                }
            }
        }
        let mut child = child;
        let status = tokio::task::spawn_blocking(move || child.wait()).await;
        let status = match status {
            Ok(Ok(status)) => exit_status(status),
            _ => 1,
        };
        *lock(&pump.ended) = Some((status, Instant::now()));
        if let Some(mut connection) = pump.subscriber.lock().await.take() {
            let _ = connection.shutdown().await;
        }
    });
    if write_line(&mut stream, &serde_json::json!({"id": text(request, "id")}))
        .await
        .is_err()
    {
        return;
    }
    attach_stream(stream, held, writer, Vec::new()).await;
}

/// `attach`: this connection takes the session's stream, with its replay first when asked.
async fn subscribe(mut stream: UnixStream, held: Arc<Held>, replay: bool) {
    if write_line(&mut stream, &serde_json::json!({"attached": true}))
        .await
        .is_err()
    {
        return;
    }
    let first = if replay { held.replay() } else { Vec::new() };
    let writer = Arc::new(tokio::sync::Mutex::new(exec::PtyWriter::of(&held.master)));
    attach_stream(stream, held, writer, first).await;
}

/// Make `stream` the session's: its output goes there (after `first`), and what comes on it goes to the program.
async fn attach_stream(
    stream: UnixStream,
    held: Arc<Held>,
    writer: Arc<tokio::sync::Mutex<exec::PtyWriter>>,
    first: Vec<u8>,
) {
    let (mut from_server, mut to_server) = stream.into_split();
    {
        let mut subscriber = held.subscriber.lock().await;
        if !first.is_empty() && to_server.write_all(&first).await.is_err() {
            return;
        }
        // a connection that had it is let go
        if let Some(mut before) = subscriber.replace(to_server) {
            let _ = before.shutdown().await;
        }
    }
    let mut buffer = vec![0u8; 4096];
    loop {
        match from_server.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if writer.lock().await.write_all(&buffer[..read]).await.is_err() {
                    break;
                }
            }
        }
    }
}

fn exit_status(status: std::process::ExitStatus) -> u32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .map(|code| u32::try_from(code).unwrap_or(1))
        .or_else(|| status.signal().map(|signal| 128 + u32::try_from(signal).unwrap_or(0)))
        .unwrap_or(1)
}

/// A request's answer from the holder at `path`.
pub(crate) async fn ask(path: &Path, request: &serde_json::Value) -> io::Result<serde_json::Value> {
    let mut stream = UnixStream::connect(path).await?;
    write_line(&mut stream, request).await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    serde_json::from_str(&line).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

#[cfg(test)]
#[path = "hold_tests.rs"]
mod tests;
