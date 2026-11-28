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

//! One kept terminal session (plans/archive/Terminal-Fancy.md §5.1–§5.4): a [`Program`] on a byte stream, run by a task
//! of the edge runtime that outlives the sockets attached to it. The task (the pump) reads the program's output into
//! the session's ring and to the one client attached, gathered into frames; a second task writes the client's input to
//! the program, so a program that does not read never holds up its output.
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
use super::events::{Event, Events};
use super::registry::Registry;
use super::ring::Ring;
#[cfg(feature = "devtools")]
use super::screen::{Read, Scope, Screen};
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
    /// The client now only watches (true), its place taken by another window, or writes again (plans/Terminal-Fancy2.md
    /// §4.4, D7): what it types is dropped while it watches.
    Viewer(bool),
}

/// A client's place on a session, from [`Session::attach`].
pub(crate) struct Attached {
    pub client: u64,
    pub out: mpsc::Receiver<Out>,
    /// It watches only: it never pauses or resizes the program, and what it types is dropped.
    pub viewer: bool,
}

/// How a client joins a session (plans/Terminal-Fancy2.md §4.4).
#[derive(Clone, Debug)]
pub(crate) struct Joining {
    /// To watch only.
    pub observe: bool,
    /// Its user: the owner, or a user the owner shared the session with.
    pub username: String,
}

enum Command {
    Attach {
        have: Option<u64>,
        cols: u16,
        rows: u16,
        joining: Joining,
        reply: oneshot::Sender<Attached>,
    },
    /// Let go of every client of `username`: their share was revoked.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    Revoke {
        username: String,
    },
    /// Tell the windows the session's state again (its recording changed).
    State,
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
    /// The screen as text (plans/Terminal-Fancy2.md §4.2), made on this first read; `None` for a session with no ring.
    #[cfg(feature = "devtools")]
    Screen {
        scope: Scope,
        ansi: bool,
        reply: oneshot::Sender<Option<Read>>,
    },
    /// Start (`Some`) or stop (`None`) the recording of a session in process (§4.5); a held one's is the holder's.
    #[cfg(all(feature = "devtools", unix))]
    Record {
        keep: Option<super::record::Keep>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// A program's input (§4.3): `text` as typed, then the named `keys`, in the modes in force; answers the count of
    /// the output so far, or why it was refused.
    #[cfg(feature = "devtools")]
    Send {
        text: String,
        keys: Vec<String>,
        reply: oneshot::Sender<Result<u64, String>>,
    },
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
    /// The users watching it now, one for each window that watches (plans/Terminal-Fancy2.md §4.4).
    pub viewers: Vec<String>,
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
    /// Its last events, numbered (plans/Terminal-Fancy2.md §4.3).
    events: Mutex<Events>,
    /// The users the owner shared it with, and whether each may type (§4.4, D3).
    shares: Mutex<std::collections::BTreeMap<String, bool>>,
    /// Its recording now (§4.5).
    recording: Mutex<Option<Recording>>,
}

/// A session's recording: its `TerminalRecording` row, the file, and whether the conf forces it (D2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Recording {
    pub id: String,
    pub location: String,
    pub forced: bool,
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
    /// Taken back from the holder after a restart: its kept output was written across sizes this server did not see.
    pub adopted: bool,
    /// The count of its first byte here: the holder's, for a session taken back (D17); else 0.
    pub count_from: u64,
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

    /// Attach the owner's window that has drawn `have` of the session's bytes (`None` for none) as the one that types:
    /// the one that typed before watches from then on, told so (D7). `None` when the session's task is gone.
    #[cfg(test)]
    pub async fn attach(&self, have: Option<u64>, cols: u16, rows: u16) -> Option<Attached> {
        let joining = Joining {
            observe: false,
            username: self.username.clone(),
        };
        self.attach_as(have, cols, rows, joining).await
    }

    /// Attach a window as `joining` says: one that types, or one that watches (§4.4).
    pub async fn attach_as(&self, have: Option<u64>, cols: u16, rows: u16, joining: Joining) -> Option<Attached> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Attach {
                have,
                cols,
                rows,
                joining,
                reply,
            })
            .await
            .ok()?;
        answer.await.ok()
    }

    /// Its recording now, if any.
    pub fn recording(&self) -> Option<Recording> {
        lock(&self.recording).clone()
    }

    /// Its recording changed: the windows are told (the REC mark).
    #[cfg_attr(not(all(feature = "devtools", unix)), allow(dead_code))]
    pub fn set_recording(&self, recording: Option<Recording>) {
        *lock(&self.recording) = recording;
        let _ = self.commands.try_send(Command::State);
    }

    /// Start (`Some`) or stop (`None`) the recording of a session in process; a held session's is the holder's.
    #[cfg(all(feature = "devtools", unix))]
    pub async fn record(&self, keep: Option<super::record::Keep>) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Record { keep, reply })
            .await
            .map_err(|_| "the session has ended".to_string())?;
        answer.await.map_err(|_| "the session has ended".to_string())?
    }

    /// Share the session with `username`, who may type in it when `write` (§4.4, D3).
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn share(&self, username: &str, write: bool) {
        lock(&self.shares).insert(username.to_string(), write);
    }

    /// Take `username`'s share back: their windows on it are let go. Whether they had one.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn unshare(&self, username: &str) -> bool {
        let had = lock(&self.shares).remove(username).is_some();
        if had {
            let _ = self.commands.try_send(Command::Revoke {
                username: username.to_string(),
            });
        }
        had
    }

    /// Whether `username` holds a share, and whether it lets them type; `None` with none.
    pub fn share_of(&self, username: &str) -> Option<bool> {
        lock(&self.shares).get(username).copied()
    }

    /// The users it is shared with, and whether each may type.
    pub fn shares(&self) -> Vec<(String, bool)> {
        lock(&self.shares)
            .iter()
            .map(|(user, write)| (user.clone(), *write))
            .collect()
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

    /// Text the session's program reads as if typed, before any client attached: a host session opened with an
    /// `input` (the Dev AIDE's Run locally). False when the queue is full or the program no longer reads.
    pub fn type_in(&self, bytes: Vec<u8>) -> bool {
        self.input.try_send(bytes).is_ok()
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

    /// The lines of the session's screen that `scope` names (plans/Terminal-Fancy2.md §4.2); with `ansi`, the screen as
    /// escape sequences too. `None` for a session that keeps no ring (an apps session) or whose task is gone.
    #[cfg(feature = "devtools")]
    pub async fn screen(&self, scope: Scope, ansi: bool) -> Option<Read> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Screen { scope, ansi, reply }).await.ok()?;
        answer.await.ok().flatten()
    }

    /// `text` as typed, then the named `keys` (§4.3), from a program: it does not count as the user typing. Answers the
    /// count of the session's output so far; `Err` with why for input refused, `Ok(Err)` when the task is gone.
    #[cfg(feature = "devtools")]
    pub async fn send_keys(&self, text: String, keys: Vec<String>) -> Result<u64, String> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Send { text, keys, reply })
            .await
            .map_err(|_| "the session has ended".to_string())?;
        answer.await.map_err(|_| "the session has ended".to_string())?
    }

    /// The number of its last event, 0 before any.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn last_event(&self) -> u64 {
        lock(&self.events).last()
    }

    /// Its events after number `after`, oldest first.
    #[cfg_attr(not(feature = "devtools"), allow(dead_code))]
    pub fn events_after(&self, after: u64) -> Vec<Event> {
        lock(&self.events).after(after)
    }

    fn event(&self, kind: &'static str, text: impl Into<String>, status: Option<i64>) {
        lock(&self.events).push(now_millis(), kind, text, status);
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
            "owner": self.username,
            "viewers": info.viewers,
            "recording": self.recording().is_some(),
            "recordingForced": self.recording().is_some_and(|recording| recording.forced),
            "shares": self.shares().iter().map(|(user, write)| serde_json::json!({"username": user, "write": write})).collect::<Vec<_>>(),
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
        events: Mutex::new(Events::default()),
        shares: Mutex::new(std::collections::BTreeMap::new()),
        recording: Mutex::new(None),
    });
    registry.insert(Arc::clone(&session));
    let pump = Pump {
        session: Arc::clone(&session),
        registry: Arc::downgrade(registry),
        ring: (spec.replay > 0).then(|| Ring::starting_at(spec.replay, spec.count_from)),
        tracker: Tracker::default(),
        count: spec.count_from,
        client: None,
        pending: Vec::new(),
        detached_since: Some(Instant::now()),
        detached_for: spec.detached_for,
        _slot: spec.slot,
        on_end: spec.on_end,
        status: Status::for_agent(spec.agent),
        attend: spec.attend,
        #[cfg(feature = "devtools")]
        screen: None,
        resized: spec.adopted,
        observers: Vec::new(),
        #[cfg(all(feature = "devtools", unix))]
        recorder: None,
    };
    crate::edge::handle().spawn(pump.run(program, commands_in, input_in));
    session
}

/// A window attached: the one that types, or one that watches.
struct Client {
    id: u64,
    out: mpsc::Sender<Out>,
    /// Its user, and whether that is the session's owner.
    username: String,
    owner: bool,
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
    /// The window that types.
    client: Option<Client>,
    /// The windows that watch (plans/Terminal-Fancy2.md §4.4): they never pause or resize the program.
    observers: Vec<Client>,
    /// Output gathered for the windows since their last frame: the bytes from their `sent` to `count`.
    pending: Vec<u8>,
    detached_since: Option<Instant>,
    detached_for: Duration,
    _slot: Option<SessionSlot>,
    on_end: Option<Box<dyn FnOnce(u32, String) + Send>>,
    status: Status,
    attend: Option<Attend>,
    /// The screen as text, once something read it (D6).
    #[cfg(feature = "devtools")]
    screen: Option<Screen>,
    /// The kept output was written at more than one size (or before an adoption): a screen made from it is `rebuilt`.
    resized: bool,
    /// The recording of a session in process (§4.5).
    #[cfg(all(feature = "devtools", unix))]
    recorder: Option<super::record::Recorder>,
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
                        if self.any_client() && flush_at.is_none() {
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
                        if !self.any_client() {
                            flush_at = None;
                        }
                    }
                    None => break,
                },
                () = tokio::time::sleep_until(detach_deadline), if self.detached_since.is_some() && hung_up.is_none() => {
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
            for client in self.client.take().into_iter().chain(self.observers.drain(..)) {
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
        self.session
            .event("ended", reason.clone(), Some(i64::from(ended.status)));
        // Its place in the limits goes back now; an ended session is only listed.
        self._slot = None;
        if let Some(on_end) = self.on_end.take() {
            on_end(ended.status, reason.clone());
        }
        for client in self.client.take().into_iter().chain(self.observers.drain(..)) {
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
        #[cfg(feature = "devtools")]
        if let Some(screen) = self.screen.as_mut() {
            screen.feed(bytes);
        }
        #[cfg(all(feature = "devtools", unix))]
        if let Some(recorder) = self.recorder.as_mut() {
            if !matches!(recorder.output(bytes), Ok(super::record::Wrote::Kept)) {
                // at its limit (or a write that failed) the recording stops, and the windows are told
                tracing::info!(
                    "terminal session {}: its recording stopped at its limit",
                    self.session.id
                );
                self.recorder = None;
                *lock(&self.session.recording) = None;
                self.session.event("recording", "stopped at its limit", None);
                self.send_state();
            }
        }
        if let Some(ring) = self.ring.as_mut() {
            ring.push(bytes, self.count, &changes.rests);
        }
        self.count += bytes.len() as u64;
        let now = now_millis();
        if changes.any() || !changes.marks.is_empty() {
            lock(&self.session.events).changes(now, &changes);
        }
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
        if self.any_client() {
            self.pending.extend_from_slice(bytes);
        }
    }

    fn send_state(&self) {
        let message = self.state_message();
        for client in self.client.iter().chain(&self.observers) {
            let _ = client.out.try_send(Out::Text(message.clone()));
        }
    }

    fn any_client(&self) -> bool {
        self.client.is_some() || !self.observers.is_empty()
    }

    /// After a window came or went: the owner has none (the detach timeout runs) or has one (it does not), and the
    /// list says who types and who watches. Another user watching keeps nothing alive.
    fn settle(&mut self) {
        let owner_here = self.client.is_some() || self.observers.iter().any(|client| client.owner);
        if owner_here {
            self.detached_since = None;
        } else if self.detached_since.is_none() {
            self.detached_since = Some(Instant::now());
        }
        let (attached, viewers) = (
            self.client.is_some(),
            self.observers
                .iter()
                .map(|client| client.username.clone())
                .collect::<Vec<_>>(),
        );
        self.session.update_info(|info| {
            info.attached = attached;
            info.viewers = viewers;
        });
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
            || self.observers.iter().any(|client| client.owner && client.visible)
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
        #[cfg(feature = "devtools")]
        if self
            .screen
            .as_ref()
            .is_some_and(|screen| screen.idle(std::time::Instant::now()))
        {
            self.screen = None;
        }
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
    /// The gathered output, as one frame to each window. A watching window far behind is let go ("behind"); its page
    /// comes back by what it drew. Only the window that types holds the program up.
    fn flush(&mut self) {
        if !self.any_client() || self.pending.is_empty() {
            self.pending.clear();
            return;
        }
        let frame = std::mem::take(&mut self.pending);
        let count = self.count;
        let before = self.observers.len();
        self.observers.retain_mut(|client| {
            let why = if client.out.try_send(Out::Bytes(frame.clone())).is_err() {
                "the window fell behind"
            } else {
                client.sent = count;
                if client.behind() <= HIGH_WATER {
                    return true;
                }
                "behind"
            };
            let _ = client.out.try_send(Out::Close(why.to_string()));
            false
        });
        if self.observers.len() != before {
            self.settle();
        }
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if client.out.try_send(Out::Bytes(frame)).is_err() {
            self.let_go("the window fell behind");
            return;
        }
        client.sent = count;
        if client.behind() > HIGH_WATER {
            client.paused = true;
        }
    }

    /// Let the window that types go, with `reason` as its socket's close reason.
    fn let_go(&mut self, reason: &str) {
        if let Some(client) = self.client.take() {
            let _ = client.out.try_send(Out::Close(reason.to_string()));
            self.session.event("detach", reason, None);
        }
        if self.observers.is_empty() {
            self.pending.clear();
        }
        self.settle();
    }

    /// Let every window of `username` go: their share was taken back.
    fn revoke(&mut self, username: &str) {
        if self.client.as_ref().is_some_and(|client| client.username == username) {
            self.let_go("the share was revoked");
        }
        self.observers.retain(|client| {
            let keep = client.username != username;
            if !keep {
                let _ = client.out.try_send(Out::Close("the share was revoked".to_string()));
            }
            keep
        });
        self.settle();
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
                joining,
                reply,
            } => {
                if let Some(attached) = self.attach(have, cols, rows, &joining, program) {
                    let _ = reply.send(attached);
                }
            }
            Command::Revoke { username } => self.revoke(&username),
            Command::State => self.send_state(),
            #[cfg(all(feature = "devtools", unix))]
            Command::Record { keep, reply } => {
                if let Some(mut before) = self.recorder.take() {
                    let _ = before.finish();
                }
                let started = match keep {
                    Some(keep) => {
                        let info = self.session.info();
                        super::record::Recorder::start(keep, info.cols, info.rows)
                            .map(|recorder| self.recorder = Some(recorder))
                            .map_err(|err| format!("the recording did not start: {err}"))
                    }
                    None => Ok(()),
                };
                self.session.event(
                    "recording",
                    if self.recorder.is_some() { "started" } else { "stopped" },
                    None,
                );
                let _ = reply.send(started);
            }
            Command::Detach { client } => {
                if self.client.as_ref().is_some_and(|current| current.id == client) {
                    self.client = None;
                    self.session.event("detach", "", None);
                    if self.observers.is_empty() {
                        self.pending.clear();
                    }
                    self.settle();
                } else if let Some(at) = self.observers.iter().position(|current| current.id == client) {
                    let gone = self.observers.remove(at);
                    self.session
                        .event("detach", format!("{} stopped watching", gone.username), None);
                    self.settle();
                }
            }
            Command::Resize { client, cols, rows } => {
                if self.client.as_ref().is_some_and(|current| current.id == client) {
                    (program.resize)(cols, rows);
                    self.sized(cols, rows);
                }
            }
            Command::Ack { client, count } => {
                let watching = self.observers.iter_mut().find(|current| current.id == client);
                if let Some(current) = watching {
                    current.acked = current.acked.max(count.min(current.sent));
                    current.last_ack = Instant::now();
                } else if let Some(current) = self.client.as_mut().filter(|current| current.id == client) {
                    current.acked = current.acked.max(count.min(current.sent));
                    current.last_ack = Instant::now();
                    if current.paused && current.behind() < LOW_WATER {
                        current.paused = false;
                    }
                }
            }
            Command::View { client, visible } => {
                let owner_watching = self
                    .observers
                    .iter_mut()
                    .find(|current| current.id == client && current.owner);
                let current = match owner_watching {
                    Some(current) => Some(current),
                    None => self.client.as_mut().filter(|current| current.id == client),
                };
                if let Some(current) = current {
                    current.visible = visible;
                    if visible && self.session.info().unseen {
                        self.session.update_info(|info| info.unseen = false);
                        self.send_state();
                    }
                }
            }
            #[cfg(feature = "devtools")]
            Command::Screen { scope, ansi, reply } => {
                let _ = reply.send(self.read_screen(scope, ansi));
            }
            #[cfg(feature = "devtools")]
            Command::Send { text, keys, reply } => {
                let sent = super::keys::send(&text, &keys, &self.tracker.modes()).and_then(|bytes| {
                    self.session
                        .input
                        .try_send(bytes)
                        .map_err(|_| "the session's program is not reading its input".to_string())
                });
                let _ = reply.send(sent.map(|()| self.count));
            }
            Command::End { .. } | Command::Release => {}
        }
    }

    /// The window's size is the session's: the info, and the screen, follow it.
    fn sized(&mut self, cols: u16, rows: u16) {
        let info = self.session.info();
        if self.count > 0 && (info.cols, info.rows) != (cols, rows) {
            self.resized = true;
        }
        self.session.update_info(|info| {
            info.cols = cols;
            info.rows = rows;
        });
        #[cfg(all(feature = "devtools", unix))]
        if let Some(recorder) = self.recorder.as_mut() {
            let _ = recorder.resize(cols, rows);
        }
        #[cfg(feature = "devtools")]
        if let Some(screen) = self.screen.as_mut() {
            screen.resize(cols, rows);
        }
    }

    /// The screen read, made first from the replay a new window would get when there is none (D6).
    #[cfg(feature = "devtools")]
    fn read_screen(&mut self, scope: Scope, ansi: bool) -> Option<Read> {
        self.ring.as_ref()?;
        if self.screen.is_none() {
            let (from, _, replay) = self.replay(None);
            let info = self.session.info();
            let scrollback = self
                .registry
                .upgrade()
                .map_or(super::screen::SCROLLBACK, |registry| registry.screen_scrollback);
            self.screen = Some(Screen::new(
                info.cols,
                info.rows,
                scrollback,
                &replay,
                from > 0 || self.resized,
            ));
        }
        let count = self.count;
        self.screen.as_mut().map(|screen| Read {
            count,
            ..screen.read(scope, ansi)
        })
    }

    /// A window joins: one that watches (§4.4) gets the replay and what follows, and changes nothing of the program;
    /// one that types takes the place of the one before, which watches from then on (D7). An apps session, with no ring
    /// to come back by, has no watchers.
    fn attach(
        &mut self,
        have: Option<u64>,
        cols: u16,
        rows: u16,
        joining: &Joining,
        program: &mut Program,
    ) -> Option<Attached> {
        if joining.observe && self.ring.is_none() {
            return None;
        }
        // what is gathered goes to the windows there now; the new one starts after it
        self.flush();
        let id = self.session.next_client.fetch_add(1, Ordering::Relaxed);
        let (out, receiver) = mpsc::channel(CLIENT_QUEUE);
        let (from, reset, replay) = self.replay(have);
        let info = self.session.info();
        let owner = joining.username == self.session.username;
        let attached = serde_json::json!({
            "type": "attached",
            "kind": self.session.kind.as_str(),
            "title": info.title,
            "cwd": info.cwd,
            "cols": if joining.observe { info.cols } else { cols },
            "rows": if joining.observe { info.rows } else { rows },
            "from": from,
            "reset": reset,
            "viewer": joining.observe,
            "owner": self.session.username,
        });
        out.try_send(Out::Text(attached.to_string())).ok()?;
        if !replay.is_empty() {
            out.try_send(Out::Bytes(replay)).ok()?;
        }
        out.try_send(Out::Text(self.state_message())).ok()?;
        let client = Client {
            id,
            out,
            username: joining.username.clone(),
            owner,
            sent: self.count,
            acked: from,
            last_ack: Instant::now(),
            paused: false,
            visible: true,
        };
        if joining.observe {
            // its replay is not held against it: it is let go only when the output after it goes unacknowledged
            let client = Client {
                acked: self.count,
                ..client
            };
            self.observers.push(client);
            self.session
                .event("attach", format!("{} watches", joining.username), None);
            self.settle();
            return Some(Attached {
                client: id,
                out: receiver,
                viewer: true,
            });
        }
        if let Some(mut before) = self.client.take() {
            if self.ring.is_some() {
                before.paused = false;
                let _ = before.out.try_send(Out::Viewer(true));
                self.observers.push(before);
            } else {
                // an apps session draws itself for one window: the one before is let go
                let _ = before.out.try_send(Out::Close("opened elsewhere".to_string()));
            }
        }
        self.client = Some(client);
        if let Some(client) = self.client.as_mut() {
            client.paused = client.behind() > HIGH_WATER;
        }
        self.session.event("attach", "", None);
        self.settle();
        self.sized(cols, rows);
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
            viewer: false,
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
        let (_, kept, modes) = ring.replay_from_rest(self.count, self.tracker.modes());
        let mut bytes = modes.preamble();
        bytes.extend_from_slice(&without_queries(&kept));
        // the window counts from where its bytes end at the session's count, whatever was left out of them
        (self.count.saturating_sub(bytes.len() as u64), true, bytes)
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
            "cols": info.cols,
            "rows": info.rows,
            "viewers": info.viewers,
            "recording": self.session.recording().is_some(),
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
                    Some(Command::Attach { cols, rows, joining, reply, .. }) => {
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
                        let _ = reply.send(Attached { client: 0, out: receiver, viewer: joining.observe });
                    }
                    Some(Command::End { .. }) | None => return,
                    Some(_) => {}
                },
            }
        }
    }
}

/// The kept output of a replay without the terminal's queries (plans/Terminal-Fancy2.md §4.5): a new window's terminal
/// would answer an old query, and the answer, typed into the program, is garbage.
fn without_queries(kept: &[u8]) -> Vec<u8> {
    #[cfg(all(feature = "devtools", unix))]
    {
        super::record::strip_queries(kept, true).0
    }
    #[cfg(not(all(feature = "devtools", unix)))]
    {
        kept.to_vec()
    }
}

fn ended_message(status: u32, reason: &str) -> String {
    serde_json::json!({"type": "ended", "status": status, "reason": reason}).to_string()
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
