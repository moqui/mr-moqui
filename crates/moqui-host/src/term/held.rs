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

//! The server's side of the holder ([`super::hold`], plans/Terminal-Fancy.md §7.5): it starts the holder when there is
//! none, opens host sessions there, and on the first look at the sessions after a start takes back those the holder
//! kept from the server before (adoption). A held session is a [`Program`] like any other: its output and input are
//! its connection to the holder, its resize and signals are requests of their own. The server's stop lets go of its
//! held sessions without ending them. When no holder starts, host sessions run in the server, and end with it.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use tokio::net::UnixStream;

use super::hold::{self, ask, read_line, write_line, PROTOCOL};
use super::host::Launch;
use super::session::Program;
use super::Ended;

/// A session of an earlier build's holder: a line that says so, and an end that asks that holder to end it.
fn earlier_program(path: PathBuf, id: String) -> Program {
    use tokio::io::AsyncWriteExt;
    let (mut note, output) = tokio::io::duplex(1024);
    let (end, ended) = tokio::sync::oneshot::channel::<()>();
    // the line now; the output ends once the user ended it and that holder was asked to
    crate::edge::handle().spawn(async move {
        let _ = note
            .write_all(b"\r\nThis session is held by an earlier build of the server; it can only be ended.\r\n")
            .await;
        let _ = ended.await;
        let _ = ask(&path, &serde_json::json!({"op": "forget", "id": id})).await;
    });
    Program {
        output: Box::pin(output),
        input: Box::pin(tokio::io::sink()),
        resize: Box::new(|_, _| {}),
        redraw: None,
        hang_up: Some(Box::new(move || {
            let _ = end.send(());
        })),
        kill: None,
        ended: Box::pin(async move {
            Ended {
                status: 0,
                note: Some("ended: it was held by an earlier build".to_string()),
            }
        }),
    }
}

/// How long a holder that was started has to answer.
const START_WAIT: Duration = Duration::from_secs(5);

pub(crate) struct Holder {
    dir: PathBuf,
    starting: tokio::sync::Mutex<()>,
    /// The held sessions were looked for since this server started.
    adopted: AtomicBool,
    /// No holder could be started: host sessions run in the server.
    unavailable: AtomicBool,
}

impl Holder {
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|factory| Self {
            dir: hold::socket_dir(factory.runtime_dir()),
            starting: tokio::sync::Mutex::new(()),
            adopted: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
        })
    }

    fn path(&self) -> PathBuf {
        hold::socket_path(&self.dir, PROTOCOL)
    }

    /// The held sessions were looked for since this server started.
    pub fn adopted(&self) -> bool {
        self.adopted.load(Ordering::Acquire)
    }

    /// No holder could be started: host sessions run in the server from now on.
    pub fn give_up(&self) {
        self.unavailable.store(true, Ordering::Relaxed);
    }

    /// Whether host sessions go to the holder, so they outlive the server.
    pub fn in_use(&self) -> bool {
        !self.unavailable.load(Ordering::Relaxed)
    }

    async fn answers(&self) -> bool {
        matches!(
            ask(&self.path(), &serde_json::json!({"op": "hello"})).await,
            Ok(answer) if answer.get("version").and_then(serde_json::Value::as_u64) == Some(u64::from(PROTOCOL))
        )
    }

    /// The holder, started when it is not there.
    pub async fn ensure(&self) -> io::Result<()> {
        if self.answers().await {
            return Ok(());
        }
        let _starting = self.starting.lock().await;
        if self.answers().await {
            return Ok(());
        }
        hold::make_dir(&self.dir)?;
        let (program, args) = super::exec::helper("term-hold")?;
        let mut command = std::process::Command::new(program);
        command
            .args(args)
            .env(hold::HOLD_ENV, &self.dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // a group of its own: a Ctrl+C or a stop of the server's group does not reach it
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;
        // waited for, so it is no zombie when it ends while the server runs
        let _ = std::thread::Builder::new()
            .name("term-hold-wait".to_string())
            .spawn(move || child.wait());
        let until = tokio::time::Instant::now() + START_WAIT;
        while tokio::time::Instant::now() < until {
            if self.answers().await {
                tracing::info!("the terminal holder started in {}", self.dir.display());
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(io::Error::other("the terminal holder did not answer"))
    }

    /// Start `launch` in the holder as session `id`, with what the server knows of it in `meta`.
    pub async fn open(
        self: &Arc<Self>,
        id: &str,
        launch: &Launch,
        replay: usize,
        meta: serde_json::Value,
    ) -> io::Result<Program> {
        self.ensure().await?;
        let helper = super::exec::helper("term-exec")?;
        let mut helper_line = vec![helper.0];
        helper_line.extend(helper.1);
        let request = serde_json::json!({
            "op": "open", "id": id, "argv": launch.argv, "arg0": launch.arg0, "helper": helper_line,
            "directory": launch.directory.to_string_lossy(), "env": launch.env, "cols": launch.cols, "rows": launch.rows,
            "replay": replay, "meta": meta,
        });
        let stream = self.stream(&request).await?;
        Ok(self.program(id, stream))
    }

    /// Take back the session `id` the holder kept, its last output first.
    pub async fn attach(self: &Arc<Self>, id: &str) -> io::Result<Program> {
        let stream = self
            .stream(&serde_json::json!({"op": "attach", "id": id, "replay": true}))
            .await?;
        Ok(self.program(id, stream))
    }

    async fn stream(&self, request: &serde_json::Value) -> io::Result<UnixStream> {
        let mut stream = UnixStream::connect(self.path()).await?;
        write_line(&mut stream, request).await?;
        let answer = read_line(&mut stream).await?;
        if let Some(error) = answer.get("error").and_then(serde_json::Value::as_str) {
            return Err(io::Error::other(error.to_string()));
        }
        Ok(stream)
    }

    /// The sessions the holder keeps: id, what the server said of each, and an ended one's status.
    pub async fn list(&self) -> io::Result<Vec<serde_json::Value>> {
        let answer = ask(&self.path(), &serde_json::json!({"op": "list"})).await?;
        Ok(answer
            .get("sessions")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    fn request(self: &Arc<Self>, request: serde_json::Value) {
        let path = self.path();
        crate::edge::handle().spawn(async move {
            if let Err(err) = ask(&path, &request).await {
                tracing::debug!("the terminal holder did not take {request}: {err}");
            }
        });
    }

    fn program(self: &Arc<Self>, id: &str, stream: UnixStream) -> Program {
        let (output, input) = stream.into_split();
        let (resize, hang_up, kill, end) = (Arc::clone(self), Arc::clone(self), Arc::clone(self), Arc::clone(self));
        let (id_resize, id_hup, id_kill, id_end) = (id.to_string(), id.to_string(), id.to_string(), id.to_string());
        Program {
            output: Box::pin(output),
            input: Box::pin(input),
            resize: Box::new(move |cols, rows| {
                resize.request(serde_json::json!({"op": "resize", "id": id_resize, "cols": cols, "rows": rows}));
            }),
            redraw: None,
            hang_up: Some(Box::new(move || {
                hang_up.request(serde_json::json!({"op": "signal", "id": id_hup, "signal": "HUP"}));
            })),
            kill: Some(Box::new(move || {
                kill.request(serde_json::json!({"op": "signal", "id": id_kill, "signal": "KILL"}));
            })),
            ended: Box::pin(async move {
                // the holder closes the stream once it has waited for the program, so its status is known
                let status = ask(&end.path(), &serde_json::json!({"op": "status", "id": id_end})).await;
                let ended = status
                    .ok()
                    .and_then(|answer| answer.get("ended").and_then(serde_json::Value::as_u64));
                let _ = ask(&end.path(), &serde_json::json!({"op": "forget", "id": id_end})).await;
                match ended {
                    Some(status) => Ended {
                        status: u32::try_from(status).unwrap_or(1),
                        note: None,
                    },
                    None => Ended {
                        status: 1,
                        note: Some("the terminal holder let the session go".to_string()),
                    },
                }
            }),
        }
    }

    /// The running sessions of a holder of an earlier build (§7.5): listed for their users, with a line that says what
    /// they are, and only ended. `list` and `forget` are the same in every version of the protocol.
    async fn adopt_earlier(
        &self,
        factory: &Arc<ExecutionContextFactory>,
        registry: &Arc<super::registry::Registry>,
        version: u32,
    ) {
        let path = hold::socket_path(&self.dir, version);
        if !path.exists() {
            return;
        }
        let Ok(answer) = ask(&path, &serde_json::json!({"op": "list"})).await else {
            return;
        };
        let listed = answer
            .get("sessions")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for held in listed {
            let id = held
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let meta = held.get("meta").cloned().unwrap_or_default();
            let text = |key: &str| {
                meta.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let username = text("username");
            if id.is_empty()
                || username.is_empty()
                || registry.contains(&id)
                || held.get("ended").is_some_and(|ended| !ended.is_null())
            {
                continue;
            }
            tracing::warn!(
                "terminal session {id} of {username} is held by a holder of an earlier build; it can only be ended"
            );
            registry.adopt_host(
                factory,
                super::registry::Adopted {
                    id: id.clone(),
                    username,
                    profile: text("profile"),
                    title: format!("{} (earlier build)", text("title")),
                    cwd: text("cwd"),
                    cols: 80,
                    rows: 24,
                    agent: false,
                    program: earlier_program(path.clone(), id),
                },
            );
        }
    }

    /// The sessions this server's holder kept from the server before, taken back once (§7.5): each running one listed
    /// again for its user, its last output replayed to its next window. Ended ones are forgotten. A session that
    /// cannot be taken back is logged and the others go on.
    pub async fn adopt(self: &Arc<Self>, factory: Weak<ExecutionContextFactory>) {
        if self.adopted.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(strong) = factory.upgrade() else {
            return;
        };
        let registry = super::registry::Registry::of(&strong);
        // empty while this is the first version of the protocol
        #[allow(clippy::reversed_empty_ranges)]
        for version in 1..PROTOCOL {
            self.adopt_earlier(&strong, &registry, version).await;
        }
        if !self.answers().await {
            return;
        }
        let listed = match self.list().await {
            Ok(listed) => listed,
            Err(err) => {
                tracing::error!("the terminal holder's sessions were not listed: {err}");
                return;
            }
        };
        for held in listed {
            let id = held
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if id.is_empty() || registry.contains(&id) {
                continue;
            }
            if held.get("ended").is_some_and(|ended| !ended.is_null()) {
                let _ = ask(&self.path(), &serde_json::json!({"op": "forget", "id": id})).await;
                continue;
            }
            let meta = held.get("meta").cloned().unwrap_or_default();
            let text = |key: &str| {
                meta.get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let username = text("username");
            if username.is_empty() {
                continue;
            }
            match self.attach(&id).await {
                Ok(program) => {
                    let size = |key: &str, default: u16| {
                        meta.get(key)
                            .and_then(serde_json::Value::as_u64)
                            .and_then(|v| u16::try_from(v).ok())
                            .unwrap_or(default)
                    };
                    registry.adopt_host(
                        &strong,
                        super::registry::Adopted {
                            id: id.clone(),
                            username,
                            profile: text("profile"),
                            title: text("title"),
                            cwd: text("cwd"),
                            cols: size("cols", 80),
                            rows: size("rows", 24),
                            agent: meta.get("agent").and_then(serde_json::Value::as_bool).unwrap_or(false),
                            program,
                        },
                    );
                    tracing::info!("terminal session {id} taken back from the holder");
                }
                Err(err) => tracing::error!("terminal session {id} was not taken back from the holder: {err}"),
            }
        }
    }
}
