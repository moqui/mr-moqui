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

//! One kept terminal session (plans/Terminal-Fancy.md §5.1–§5.4): a [`Program`] on a byte stream, run by a task of the
//! edge runtime that outlives the sockets attached to it. The task (the pump) reads the program's output into the
//! session's ring and to the one client attached, gathered into frames; a second task writes the client's input to the
//! program, so a program that does not read never holds up its output.
//!
//! A client attaches with how far it got. Inside the ring it continues from there with nothing repeated; otherwise it
//! is told to reset and gets the modes in force and the ring from its oldest point at rest. An apps session keeps no
//! ring and is asked to draw itself again. While the client is more than [`HIGH_WATER`] bytes behind what it has
//! acknowledged the program is not read, and it is read again below [`LOW_WATER`]; one that acknowledges nothing for
//! [`ACK_WAIT`] while behind is let go. With no client nothing waits.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_lib::sync::lock;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::attention::{Attention, Status};
use super::registry::Registry;
use super::ring::Ring;
use super::track::Tracker;
use super::{Ended, SessionSlot};

/// The client this far behind what it acknowledged stops the program's output being read.
pub(crate) const HIGH_WATER: u64 = 500_000;
/// Below this the output is read again.
pub(crate) const LOW_WATER: u64 = 100_000;
/// A client behind by more than [`HIGH_WATER`] that acknowledges nothing for this long is let go.
const ACK_WAIT: Duration = Duration::from_secs(30);
/// Output is gathered for up to this long into one message,
const GATHER: Duration = Duration::from_millis(8);
/// or up to this many bytes.
const GATHER_MAX: usize = 64 * 1024;
/// An ended command or host session stays listed this long, with its exit status and its last output.
const ENDED_KEEP: Duration = Duration::from_secs(600);
/// A program asked to hang up that has not ended after this long is killed.
const KILL_AFTER: Duration = Duration::from_secs(5);
/// Messages waiting for a client's socket; past it the client fell behind and is let go.
const CLIENT_QUEUE: usize = 1024;
/// Input messages waiting for the program.
const INPUT_QUEUE: usize = 256;

/// What a session runs (D1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// The terminal client of the apps, as the user.
    Apps,
    /// The `moqui>` command line.
    Command,
    /// A program on the server's operating system, on a pty.
    Host,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apps => "apps",
            Self::Command => "command",
            Self::Host => "host",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "apps" | "" => Some(Self::Apps),
            "command" => Some(Self::Command),
            "host" => Some(Self::Host),
            _ => None,
        }
    }
}

/// A program on a byte stream, as a session runs it: the stream's two halves and what else the session may ask of it.
pub(crate) struct Program {
    pub output: Pin<Box<dyn AsyncRead + Send>>,
    pub input: Pin<Box<dyn AsyncWrite + Send>>,
    /// The window's new columns and rows.
    pub resize: Box<dyn FnMut(u16, u16) + Send>,
    /// Draw the whole screen again, for a program that keeps no ring (an apps session).
    pub redraw: Option<Box<dyn FnMut() + Send>>,
    /// Ask the program to end, beside closing its input (SIGHUP to a host program's group).
    pub hang_up: Option<Box<dyn FnOnce() + Send>>,
    /// End it now, when it has not ended [`KILL_AFTER`] the hang-up (SIGKILL).
    pub kill: Option<Box<dyn FnOnce() + Send>>,
    /// How the program ended, once its output has closed.
    pub ended: Pin<Box<dyn Future<Output = Ended> + Send>>,
}

/// To an attached client's socket.
#[derive(Debug)]
pub(crate) enum Out {
    /// The terminal's bytes.
    Bytes(Vec<u8>),
    /// A JSON control message.
    Text(String),
    /// Close the socket with this reason.
    Close(String),
}

/// A client's place on a session, from [`Session::attach`].
pub(crate) struct Attached {
    pub client: u64,
    pub out: mpsc::Receiver<Out>,
}

enum Command {
    Attach {
        have: Option<u64>,
        cols: u16,
        rows: u16,
        reply: oneshot::Sender<Attached>,
    },
    Detach {
        client: u64,
    },
    Resize {
        client: u64,
        cols: u16,
        rows: u16,
    },
    Ack {
        client: u64,
        count: u64,
    },
    /// Whether the client's page is looking at the session (§11.1): shown, in a page the user sees.
    View {
        client: u64,
        visible: bool,
    },
    End {
        reason: String,
    },
    /// Let go of a held session without ending it: the server is stopping, and the holder keeps it (§7.5).
    Release,
}

/// What the session list shows of a session.
#[derive(Clone, Debug, Default)]
pub(crate) struct Info {
    pub title: String,
    /// The user named it: the program's own title (OSC 0, 2) no longer changes it.
    pub named: bool,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub attached: bool,
    /// Millis of the last output.
    pub last_output: i64,
    /// How it ended: the exit status and the reason; `None` while it runs.
    pub ended: Option<(u32, String)>,
    /// The program's own word about itself (§11.1): idle, working, done, blocked, error, or empty.
    pub status: String,
    /// What it said with its status or notification, for the list and a notification.
    pub status_text: String,
    /// Its progress in percent (OSC 9;4), or `None`.
    pub progress: Option<u8>,
    /// Set when attention was wanted and nobody has looked since (§11.1).
    pub unseen: bool,
    /// Whether this session notifies: default (the user's settings), all, or off.
    pub notify: String,
}

/// A kept session: found in the [`Registry`] by its id, only by its own user.
pub(crate) struct Session {
    pub id: String,
    pub username: String,
    pub kind: Kind,
    /// The host profile it was opened with, empty for the others.
    pub profile: String,
    /// Millis it started.
    pub started: i64,
    /// The holder runs its program (§7.5).
    pub held: bool,
    info: Mutex<Info>,
    commands: mpsc::Sender<Command>,
    input: mpsc::Sender<Vec<u8>>,
    next_client: AtomicU64,
    /// The user typed since the pump last looked.
    typed: std::sync::atomic::AtomicBool,
}

/// What a session that wants its user while nobody looks tells (§11.1): the notification's sender.
pub(crate) type Attend = std::sync::Arc<dyn Fn(Notice) + Send + Sync>;

/// A session's attention, for its user. Only a build with command and host sessions sends one.
#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
pub(crate) struct Notice {
    pub id: String,
    pub username: String,
    pub title: String,
    pub cwd: String,
    /// The session's own notify setting.
    pub notify: String,
    pub attention: Attention,
}

/// What [`spawn`] needs beside the program.
pub(crate) struct Spec {
    pub id: String,
    pub username: String,
    pub kind: Kind,
    pub profile: String,
    pub title: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    /// How long it is kept with no client before it is ended.
    pub detached_for: Duration,
    /// The bytes of output kept for a client that comes back; 0 keeps none (an apps session).
    pub replay: usize,
    /// The session's place in its limits, given back when it ends.
    pub slot: Option<SessionSlot>,
    /// Called once the session ended, with its exit status and the reason (the audit row's end).
    pub on_end: Option<Box<dyn FnOnce(u32, String) + Send>>,
    /// The holder runs its program: the server's stop lets go of it rather than ending it.
    pub held: bool,
    /// An agent's session: its quiet output reads as done (§11.1).
    pub agent: bool,
    /// What an attention is told to; none for a session that notifies nothing (the apps).
    pub attend: Option<Attend>,
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

impl Session {
    pub fn info(&self) -> Info {
        lock(&self.info).clone()
    }

    pub(super) fn update_info(&self, change: impl FnOnce(&mut Info)) {
        change(&mut lock(&self.info));
    }

    /// The user's own name for the session; empty gives the title back to the program.
    pub fn rename(&self, title: &str) {
        let mut info = lock(&self.info);
        info.named = !title.is_empty();
        if info.named {
            info.title = title.to_string();
        }
    }

    /// Attach a client that has drawn `have` of the session's bytes (`None` for none): the one attached before is let
    /// go with "opened elsewhere". `None` when the session's task is gone.
    pub async fn attach(&self, have: Option<u64>, cols: u16, rows: u16) -> Option<Attached> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Attach {
                have,
                cols,
                rows,
                reply,
            })
            .await
            .ok()?;
        answer.await.ok()
    }

    pub async fn detach(&self, client: u64) {
        let _ = self.commands.send(Command::Detach { client }).await;
    }

    pub async fn resize(&self, client: u64, cols: u16, rows: u16) {
        let _ = self.commands.send(Command::Resize { client, cols, rows }).await;
    }

    pub async fn ack(&self, client: u64, count: u64) {
        let _ = self.commands.send(Command::Ack { client, count }).await;
    }

    /// The client's input, to the program; false when the program no longer reads.
    pub async fn write(&self, bytes: Vec<u8>) -> bool {
        self.typed.store(true, Ordering::Relaxed);
        self.input.send(bytes).await.is_ok()
    }

    /// Whether the client's page looks at the session now.
    pub async fn view(&self, client: u64, visible: bool) {
        let _ = self.commands.send(Command::View { client, visible }).await;
    }

    /// The session's own notify setting: default, all or off.
    pub fn set_notify(&self, notify: &str) {
        let notify = match notify {
            "all" | "off" => notify,
            _ => "default",
        };
        lock(&self.info).notify = notify.to_string();
    }

    /// Let go of a held session, which the holder keeps: the server is stopping.
    pub fn release(&self) {
        let _ = self.commands.try_send(Command::Release);
    }

    /// End the session: its program is asked to end, and killed when it has not.
    pub fn end(&self, reason: &str) {
        let _ = self.commands.try_send(Command::End {
            reason: reason.to_string(),
        });
    }

    /// The session for the list (`find#TerminalSessions`).
    pub fn to_json(&self) -> serde_json::Value {
        let info = self.info();
        let (state, exit_status, reason) = match &info.ended {
            Some((status, reason)) => ("ended", Some(*status), reason.clone()),
            None => ("running", None, String::new()),
        };
        serde_json::json!({
            "id": self.id,
            "kind": self.kind.as_str(),
            "profile": self.profile,
            "title": info.title,
            "cwd": info.cwd,
            "cols": info.cols,
            "rows": info.rows,
            "state": state,
            "exitStatus": exit_status,
            "reason": reason,
            "attached": info.attached,
            "status": info.status,
            "statusText": info.status_text,
            "progress": info.progress,
            "unseen": info.unseen,
            "notify": info.notify,
            "started": self.started,
            "lastOutput": info.last_output,
        })
    }
}

/// Start `program` as the session `spec` names, on the edge runtime, and list it in `registry`.
pub(crate) fn spawn(registry: &Arc<Registry>, spec: Spec, program: Program) -> Arc<Session> {
    let (commands, commands_in) = mpsc::channel(64);
    let (input, input_in) = mpsc::channel(INPUT_QUEUE);
    let session = Arc::new(Session {
        id: spec.id.clone(),
        username: spec.username.clone(),
        kind: spec.kind,
        profile: spec.profile.clone(),
        started: now_millis(),
        held: spec.held,
        info: Mutex::new(Info {
            title: spec.title.clone(),
            cwd: spec.cwd.clone(),
            cols: spec.cols,
            rows: spec.rows,
            notify: "default".to_string(),
            ..Info::default()
        }),
        commands,
        input,
        next_client: AtomicU64::new(1),
        typed: std::sync::atomic::AtomicBool::new(false),
    });
    registry.insert(Arc::clone(&session));
    let pump = Pump {
        session: Arc::clone(&session),
        registry: Arc::downgrade(registry),
        ring: (spec.replay > 0).then(|| Ring::new(spec.replay)),
        tracker: Tracker::default(),
        count: 0,
        client: None,
        pending: Vec::new(),
        detached_since: Some(Instant::now()),
        detached_for: spec.detached_for,
        _slot: spec.slot,
        on_end: spec.on_end,
        status: Status::for_agent(spec.agent),
        attend: spec.attend,
    };
    crate::edge::handle().spawn(pump.run(program, commands_in, input_in));
    session
}

/// The one client attached.
struct Client {
    id: u64,
    out: mpsc::Sender<Out>,
    /// The session's count when its last frame was sent.
    sent: u64,
    /// The count it acknowledged it drew.
    acked: u64,
    last_ack: Instant,
    /// Its output is not read until it catches up.
    paused: bool,
    /// Its page looks at the session (§11.1): it says so, and is taken to until it says not.
    visible: bool,
}

impl Client {
    fn behind(&self) -> u64 {
        self.sent.saturating_sub(self.acked)
    }
}

struct Pump {
    session: Arc<Session>,
    registry: Weak<Registry>,
    ring: Option<Ring>,
    tracker: Tracker,
    /// Every byte the program wrote.
    count: u64,
    client: Option<Client>,
    /// Output gathered for the client since its last frame: the bytes from `client.sent` to `count`.
    pending: Vec<u8>,
    detached_since: Option<Instant>,
    detached_for: Duration,
    _slot: Option<SessionSlot>,
    on_end: Option<Box<dyn FnOnce(u32, String) + Send>>,
    status: Status,
    attend: Option<Attend>,
}

/// Write the client's input to the program until the session ends it or the program stops reading.
async fn feed(
    mut input: Pin<Box<dyn AsyncWrite + Send>>,
    mut bytes: mpsc::Receiver<Vec<u8>>,
    mut stop: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            next = bytes.recv() => match next {
                Some(next) => {
                    if input.write_all(&next).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            _ = &mut stop => break,
        }
    }
    let _ = input.shutdown().await;
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(86_400 * 365)
}

impl Pump {
    async fn run(
        mut self,
        mut program: Program,
        mut commands: mpsc::Receiver<Command>,
        input: mpsc::Receiver<Vec<u8>>,
    ) {
        let label = format!("terminal session {} of {}", self.session.id, self.session.username);
        let mut input = Some(input);
        let (stop_input, stopped) = oneshot::channel();
        let mut stop_input = Some(stop_input);
        let program_input = std::mem::replace(&mut program.input, Box::pin(tokio::io::sink()));
        if let Some(input) = input.take() {
            tokio::spawn(feed(program_input, input, stopped));
        }
        let mut buffer = vec![0u8; 16 * 1024];
        let mut flush_at: Option<Instant> = None;
        // When the program was asked to end, and why.
        let mut hung_up: Option<Instant> = None;
        let mut reason: Option<String> = None;
        let mut released = false;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let paused = self.client.as_ref().is_some_and(|client| client.paused);
            let ack_deadline = self
                .client
                .as_ref()
                .map_or_else(far, |client| client.last_ack + ACK_WAIT);
            let detach_deadline = self.detached_since.map_or_else(far, |since| since + self.detached_for);
            tokio::select! {
                read = program.output.read(&mut buffer), if !paused => match read {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        self.output(&buffer[..read]);
                        if self.client.is_some() && flush_at.is_none() {
                            flush_at = Some(Instant::now() + GATHER);
                        }
                        if self.pending.len() >= GATHER_MAX {
                            self.flush();
                            flush_at = None;
                        }
                    }
                },
                () = tokio::time::sleep_until(flush_at.unwrap_or_else(far)), if flush_at.is_some() => {
                    self.flush();
                    flush_at = None;
                }
                command = commands.recv() => match command {
                    Some(Command::End { reason: why }) => {
                        if hung_up.is_none() {
                            tracing::info!("{label}: ended by its user");
                            reason = Some(why);
                            hung_up = Some(Instant::now());
                            self.hang_up(&mut program, &mut stop_input);
                        }
                    }
                    Some(Command::Release) => {
                        released = true;
                        break;
                    }
                    Some(command) => {
                        self.command(command, &mut program);
                        if self.client.is_none() {
                            flush_at = None;
                        }
                    }
                    None => break,
                },
                () = tokio::time::sleep_until(detach_deadline), if self.client.is_none() && hung_up.is_none() => {
                    let hours = self.detached_for.as_secs() / 3600;
                    let note = if hours > 0 {
                        format!("closed after {hours} hours with no window")
                    } else {
                        format!("closed after {} seconds with no window", self.detached_for.as_secs())
                    };
                    tracing::info!("{label}: {note}");
                    reason = Some(note);
                    hung_up = Some(Instant::now());
                    self.hang_up(&mut program, &mut stop_input);
                }
                _ = tick.tick() => self.tick(),
                () = tokio::time::sleep_until(ack_deadline), if paused => {
                    tracing::info!("{label}: its window acknowledged nothing for {ACK_WAIT:?}; let go");
                    self.let_go("the window stopped answering");
                }
                () = tokio::time::sleep_until(hung_up.map_or_else(far, |at| at + KILL_AFTER)), if hung_up.is_some() && program.kill.is_some() => {
                    if let Some(kill) = program.kill.take() {
                        tracing::warn!("{label}: did not end within {KILL_AFTER:?} of the hang-up; killed");
                        kill();
                    }
                }
            }
        }
        self.flush();
        if released {
            // the holder keeps the program; this server only lets go of it
            tracing::info!("{label}: let go to the terminal holder");
            if let Some(client) = self.client.take() {
                let _ = client.out.try_send(Out::Close("the server is stopping".to_string()));
            }
            if let Some(registry) = self.registry.upgrade() {
                registry.remove(&self.session.id);
            }
            return;
        }
        if let Some(stop) = stop_input.take() {
            let _ = stop.send(());
        }
        let ended = program.ended.await;
        let reason = reason.or(ended.note).unwrap_or_else(|| match ended.status {
            0 => "the session ended".to_string(),
            status => format!("the session ended with status {status}"),
        });
        tracing::info!("{label} ended ({}): {reason}", ended.status);
        self.session
            .update_info(|info| info.ended = Some((ended.status, reason.clone())));
        // Its place in the limits goes back now; an ended session is only listed.
        self._slot = None;
        if let Some(on_end) = self.on_end.take() {
            on_end(ended.status, reason.clone());
        }
        if let Some(client) = self.client.take() {
            let _ = client.out.try_send(Out::Text(ended_message(ended.status, &reason)));
            let _ = client.out.try_send(Out::Close(reason.clone()));
        }
        if self.ring.is_some() {
            self.keep_ended(&mut commands, ended.status, &reason).await;
        }
        if let Some(registry) = self.registry.upgrade() {
            registry.remove(&self.session.id);
        }
    }

    /// Bytes the program wrote: counted, kept, read for its state, and gathered for the client.
    fn output(&mut self, bytes: &[u8]) {
        let changes = self.tracker.feed(bytes);
        if let Some(ring) = self.ring.as_mut() {
            ring.push(bytes, self.count, &changes.rests);
        }
        self.count += bytes.len() as u64;
        let now = now_millis();
        self.session.update_info(|info| {
            info.last_output = now;
            if let Some(title) = &changes.title {
                if !info.named {
                    info.title.clone_from(title);
                }
            }
            if let Some(cwd) = &changes.cwd {
                info.cwd.clone_from(cwd);
            }
        });
        let (status_changed, attention) = self.status.output(&changes, std::time::Instant::now());
        if let Some(progress) = changes.progress {
            self.session.update_info(|info| info.progress = progress);
        }
        if status_changed {
            self.publish_status();
        }
        if let Some(attention) = attention {
            self.attend(attention);
        }
        if changes.any() || status_changed {
            self.send_state();
        }
        if self.client.is_some() {
            self.pending.extend_from_slice(bytes);
        }
    }

    fn send_state(&self) {
        if let Some(client) = &self.client {
            let _ = client.out.try_send(Out::Text(self.state_message()));
        }
    }

    fn publish_status(&self) {
        let (status, text) = (self.status.status.clone(), self.status.text.clone());
        self.session.update_info(|info| {
            info.status = status;
            info.status_text = text;
        });
    }

    /// Whether the user looks at the session now.
    fn looking(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.visible)
    }

    /// The session wants its user: unseen, and told when nobody looks.
    fn attend(&mut self, attention: Attention) {
        if self.looking() {
            return;
        }
        self.session.update_info(|info| info.unseen = true);
        let Some(attend) = &self.attend else {
            return;
        };
        let info = self.session.info();
        attend(Notice {
            id: self.session.id.clone(),
            username: self.session.username.clone(),
            title: info.title,
            cwd: info.cwd,
            notify: info.notify,
            attention,
        });
    }

    /// Each second: typing answers a bell, and quiet output ends working.
    fn tick(&mut self) {
        let mut changed = false;
        if self.session.typed.swap(false, Ordering::Relaxed) {
            changed |= self.status.input();
            if self.session.info().unseen {
                self.session.update_info(|info| info.unseen = false);
                changed = true;
            }
        }
        let (quiet, attention) = self.status.tick(std::time::Instant::now());
        changed |= quiet;
        if changed {
            self.publish_status();
            self.send_state();
        }
        if let Some(attention) = attention {
            self.attend(attention);
        }
    }

    /// The gathered output, as one frame to the client.
    fn flush(&mut self) {
        let Some(client) = self.client.as_mut() else {
            self.pending.clear();
            return;
        };
        if self.pending.is_empty() {
            return;
        }
        let frame = std::mem::take(&mut self.pending);
        if client.out.try_send(Out::Bytes(frame)).is_err() {
            self.let_go("the window fell behind");
            return;
        }
        client.sent = self.count;
        if client.behind() > HIGH_WATER {
            client.paused = true;
        }
    }

    /// Let the client go, with `reason` as its socket's close reason.
    fn let_go(&mut self, reason: &str) {
        if let Some(client) = self.client.take() {
            let _ = client.out.try_send(Out::Close(reason.to_string()));
        }
        self.pending.clear();
        self.detached_since = Some(Instant::now());
        self.session.update_info(|info| info.attached = false);
    }

    fn hang_up(&mut self, program: &mut Program, stop_input: &mut Option<oneshot::Sender<()>>) {
        if let Some(stop) = stop_input.take() {
            let _ = stop.send(());
        }
        if let Some(hang_up) = program.hang_up.take() {
            hang_up();
        }
    }

    fn command(&mut self, command: Command, program: &mut Program) {
        match command {
            Command::Attach {
                have,
                cols,
                rows,
                reply,
            } => {
                if let Some(attached) = self.attach(have, cols, rows, program) {
                    let _ = reply.send(attached);
                }
            }
            Command::Detach { client } => {
                if self.client.as_ref().is_some_and(|current| current.id == client) {
                    self.client = None;
                    self.pending.clear();
                    self.detached_since = Some(Instant::now());
                    self.session.update_info(|info| info.attached = false);
                }
            }
            Command::Resize { client, cols, rows } => {
                if self.client.as_ref().is_some_and(|current| current.id == client) {
                    (program.resize)(cols, rows);
                    self.session.update_info(|info| {
                        info.cols = cols;
                        info.rows = rows;
                    });
                }
            }
            Command::Ack { client, count } => {
                if let Some(current) = self.client.as_mut().filter(|current| current.id == client) {
                    current.acked = current.acked.max(count.min(current.sent));
                    current.last_ack = Instant::now();
                    if current.paused && current.behind() < LOW_WATER {
                        current.paused = false;
                    }
                }
            }
            Command::View { client, visible } => {
                if let Some(current) = self.client.as_mut().filter(|current| current.id == client) {
                    current.visible = visible;
                    if visible && self.session.info().unseen {
                        self.session.update_info(|info| info.unseen = false);
                        self.send_state();
                    }
                }
            }
            Command::End { .. } | Command::Release => {}
        }
    }

    fn attach(&mut self, have: Option<u64>, cols: u16, rows: u16, program: &mut Program) -> Option<Attached> {
        if let Some(before) = self.client.take() {
            let _ = before.out.try_send(Out::Close("opened elsewhere".to_string()));
        }
        self.pending.clear();
        let id = self.session.next_client.fetch_add(1, Ordering::Relaxed);
        let (out, receiver) = mpsc::channel(CLIENT_QUEUE);
        let (from, reset, replay) = self.replay(have);
        let info = self.session.info();
        let attached = serde_json::json!({
            "type": "attached",
            "kind": self.session.kind.as_str(),
            "title": info.title,
            "cwd": info.cwd,
            "cols": cols,
            "rows": rows,
            "from": from,
            "reset": reset,
        });
        out.try_send(Out::Text(attached.to_string())).ok()?;
        if !replay.is_empty() {
            out.try_send(Out::Bytes(replay)).ok()?;
        }
        out.try_send(Out::Text(self.state_message())).ok()?;
        self.client = Some(Client {
            id,
            out,
            sent: self.count,
            acked: from,
            last_ack: Instant::now(),
            paused: false,
            visible: true,
        });
        if let Some(client) = self.client.as_mut() {
            client.paused = client.behind() > HIGH_WATER;
        }
        self.detached_since = None;
        self.session.update_info(|info| {
            info.attached = true;
            info.cols = cols;
            info.rows = rows;
        });
        let alternate = self.tracker.alternate_screen();
        if reset && alternate && self.session.kind == Kind::Host {
            // A full-screen program repaints on a change of size; the replay alone may not hold its whole screen.
            (program.resize)(cols, rows.saturating_sub(1).max(1));
        }
        (program.resize)(cols, rows);
        if let Some(redraw) = program.redraw.as_mut() {
            redraw();
        }
        Some(Attached {
            client: id,
            out: receiver,
        })
    }

    /// Where a client that drew `have` bytes goes on from: `(from, reset, bytes)`.
    fn replay(&self, have: Option<u64>) -> (u64, bool, Vec<u8>) {
        let Some(ring) = &self.ring else {
            return (self.count, true, Vec::new());
        };
        if let Some(have) = have {
            if let Some(after) = ring.since(have, self.count) {
                return (have, false, after);
            }
        }
        let (from, kept, modes) = ring.replay_from_rest(self.count, self.tracker.modes());
        let mut bytes = modes.preamble();
        bytes.extend_from_slice(&kept);
        (from, true, bytes)
    }

    fn state_message(&self) -> String {
        let info = self.session.info();
        serde_json::json!({
            "type": "state",
            "title": info.title,
            "cwd": info.cwd,
            "status": info.status,
            "statusText": info.status_text,
            "progress": info.progress,
            "unseen": info.unseen,
        })
        .to_string()
    }

    /// An ended session with a ring stays for [`ENDED_KEEP`], so a client that comes back sees its last output and why
    /// it ended; ended again by its user, it goes at once.
    async fn keep_ended(&mut self, commands: &mut mpsc::Receiver<Command>, status: u32, reason: &str) {
        let until = Instant::now() + ENDED_KEEP;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(until) => return,
                command = commands.recv() => match command {
                    Some(Command::Attach { cols, rows, reply, .. }) => {
                        let (out, receiver) = mpsc::channel(4);
                        let (from, _, replay) = self.replay(None);
                        let info = self.session.info();
                        let attached = serde_json::json!({
                            "type": "attached", "kind": self.session.kind.as_str(), "title": info.title,
                            "cwd": info.cwd, "cols": cols, "rows": rows, "from": from, "reset": true,
                        });
                        let _ = out.try_send(Out::Text(attached.to_string()));
                        let _ = out.try_send(Out::Bytes(replay));
                        let _ = out.try_send(Out::Text(ended_message(status, reason)));
                        let _ = out.try_send(Out::Close(reason.to_string()));
                        let _ = reply.send(Attached { client: 0, out: receiver });
                    }
                    Some(Command::End { .. }) | None => return,
                    Some(_) => {}
                },
            }
        }
    }
}

fn ended_message(status: u32, reason: &str) -> String {
    serde_json::json!({"type": "ended", "status": status, "reason": reason}).to_string()
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
