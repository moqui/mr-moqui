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

//! The apps session (plans/archive/Screen-TUI-Impl.md 4.2, 4.4): `moqui_tui::run_session` on a thread of its own, the
//! link it calls through with the login gate, and [`serve`], which carries a stream's bytes and window changes to it
//! and its frames back.

use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync::lock;
use moqui_tui::{
    Choice, LinkError, PushSink, RawBody, Reply, ScreenLink, ScreenModel, SessionEvent, SessionOptions, ShellInfo,
    Subscription, TransitionOutcome,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use super::{Ended, SessionSlot, Start, Who, IDLE, LOGIN_WAIT, MAX_LOGIN_FAILURES};
use crate::tui::HostLink;

/// Where a session starts: the apps' root, whose menu lists the apps the user may open.
const START_PATH: &str = "/apps";
/// Frames the session may have waiting for the stream; past it the session's thread waits for the client.
const FRAMES_WAITING: usize = 64;
/// A write to the client that takes nothing for this long ends the session (`plans/archive/High-Level-Review.md` HL-027).
const WRITE_STALL: Duration = Duration::from_secs(60);
/// Log lines a session's pane holds while it is closed.
const LOG_LINES: usize = 500;

/// Frames to the stream task: each `flush` sends what was written since as one chunk, so a frame goes out whole. The
/// channel is bounded, so a client that reads slowly holds up the session's thread rather than filling memory.
struct FrameWriter {
    pending: Vec<u8>,
    out: tokio_mpsc::Sender<Vec<u8>>,
}

impl Write for FrameWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.out
            .blocking_send(std::mem::take(&mut self.pending))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the stream closed"))
    }
}

/// Whether `username` may read Monitor Logs: view the System app (`component://tools/screen/System.xml`, which Monitor and its
/// Logs screen inherits) and the `otel_logs` dataset that screen reads, checked on the request pool as that user. A
/// check that fails is a no. Blocks until the pool answers.
fn may_read_log_now(factory: &Weak<ExecutionContextFactory>, username: &str) -> bool {
    let Some(factory) = factory.upgrade() else {
        return false;
    };
    let username = username.to_string();
    let owner = Arc::clone(&factory);
    let Ok(rx) = factory.request_pool().try_submit(owner, move || {
        moqui_core::user::internal_login_user(&username)?;
        Ok(
            moqui_core::user::authorize("AT_XML_SCREEN", "component://tools/screen/System.xml", "view").is_ok()
                && moqui_core::user::authorize("AT_SERVICE", "org.moqui.report.dataset.otel_logs", "view").is_ok(),
        )
    }) else {
        return false;
    };
    matches!(rx.recv(), Ok(Ok(true)))
}

async fn may_read_log(factory: &Weak<ExecutionContextFactory>, username: &str) -> bool {
    let factory = Weak::clone(factory);
    let username = username.to_string();
    tokio::task::spawn_blocking(move || may_read_log_now(&factory, &username))
        .await
        .unwrap_or(false)
}

/// The log pane's lines, WARN and ERROR only, forwarded to the session until it is gone.
fn forward_log(events: &mpsc::Sender<SessionEvent>, username: &str) {
    let lines = crate::logging::tap_at(LOG_LINES, tracing::Level::WARN);
    let to_session = events.clone();
    let _ = std::thread::Builder::new()
        .name(format!("term-log-{username}"))
        .spawn(move || {
            // Ends when the session's inbox is gone, at the next line after it.
            while let Ok(line) = lines.recv() {
                if to_session.send(SessionEvent::Log(line)).is_err() {
                    break;
                }
            }
        });
}

/// What the session and its login dialog share with [`serve`].
struct Gate {
    factory: Weak<ExecutionContextFactory>,
    /// The session's place in the limits; none when the session it runs in holds the place.
    slot: Mutex<Option<SessionSlot>>,
    events: mpsc::Sender<SessionEvent>,
    label: String,
    logged_in: AtomicBool,
    /// The transport's flag for a dialog login.
    done: Option<Arc<AtomicBool>>,
    failures: AtomicUsize,
    /// Why the gate closed the session, for the user.
    note: Mutex<Option<String>>,
}

impl Gate {
    fn logged_in(&self) -> bool {
        self.logged_in.load(Ordering::Acquire)
    }
}

/// The in-process link with the login gate: a dialog login takes the user's session slot, decides the log pane, and
/// counts toward [`MAX_LOGIN_FAILURES`].
struct GatedLink {
    link: HostLink,
    gate: Arc<Gate>,
}

impl ScreenLink for GatedLink {
    fn level(&self, path: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        self.link.level(path, parameters)
    }

    fn transition(
        &self,
        url: &str,
        method: &str,
        fields: &BTreeMap<String, String>,
    ) -> Result<TransitionOutcome, LinkError> {
        self.link.transition(url, method, fields)
    }

    fn navigation(&self, path: &str, parameters: &BTreeMap<String, String>) -> Reply {
        self.link.navigation(path, parameters)
    }

    fn transition_reply(&self, url: &str, method: &str, fields: &BTreeMap<String, String>) -> Reply {
        self.link.transition_reply(url, method, fields)
    }

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError> {
        self.link.options(url, parameters)
    }

    fn shell(&self, path: &str) -> Result<ShellInfo, LinkError> {
        self.link.shell(path)
    }

    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        let gate = &self.gate;
        // The user's slot is taken before the password is checked, and given back when the login fails.
        let before = lock(&gate.slot)
            .as_ref()
            .map(|slot| slot.username.clone())
            .unwrap_or_default();
        if let Some(slot) = lock(&gate.slot).as_mut() {
            slot.assign(username).map_err(LinkError::Remote)?;
        }
        match self.link.login(username, password, code) {
            Ok(()) => {
                gate.logged_in.store(true, Ordering::Release);
                if let Some(done) = &gate.done {
                    done.store(true, Ordering::Release);
                }
                tracing::info!("{}: {username} logged in", gate.label);
                let log = may_read_log_now(&gate.factory, username);
                if log {
                    forward_log(&gate.events, username);
                }
                let _ = gate.events.send(SessionEvent::LogOffered(log));
                Ok(())
            }
            Err(err) => {
                let back = lock(&gate.slot).as_mut().map_or(Ok(()), |slot| slot.assign(&before));
                if let Err(reason) = back {
                    tracing::warn!(
                        "{}: the session slot did not go back to {before:?}: {reason}",
                        gate.label
                    );
                }
                // A right password that only wants its code is not a refused login; a wrong code is.
                if matches!(err, LinkError::Code(_)) && code.is_empty() {
                    return Err(err);
                }
                let failures = gate.failures.fetch_add(1, Ordering::AcqRel) + 1;
                tracing::warn!("{}: login of {username} refused ({failures}): {err}", gate.label);
                if failures >= MAX_LOGIN_FAILURES {
                    *lock(&gate.note) = Some(format!("closed after {MAX_LOGIN_FAILURES} refused logins"));
                    let _ = gate.events.send(SessionEvent::Close);
                }
                Err(err)
            }
        }
    }

    fn send_code(&self, username: &str, password: &str, factor: &str) -> Result<String, LinkError> {
        self.link.send_code(username, password, factor)
    }

    fn json(
        &self,
        url: &str,
        method: &str,
        parameters: &BTreeMap<String, String>,
    ) -> Result<serde_json::Value, LinkError> {
        self.link.json(url, method, parameters)
    }

    fn fragment(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<ScreenModel, LinkError> {
        self.link.fragment(url, parameters)
    }

    fn raw(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<RawBody, LinkError> {
        self.link.raw(url, parameters)
    }

    fn subscribe(&self, sink: PushSink) -> Option<Subscription> {
        self.link.subscribe(sink)
    }
}

/// Run the client on `stream` until the user quits, the stream closes, or the session times out.
pub(crate) async fn serve<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, start: &mut Start) -> Ended {
    let label = start.label.clone();
    let username = match &start.who {
        Who::User(username) => username.clone(),
        Who::Login { .. } => String::new(),
    };
    let admission = std::mem::replace(&mut start.admission, super::Admission::Counted);
    let slot = match admission {
        super::Admission::Take => start.sessions.take(&username).map(Some),
        super::Admission::Held(slot) => Ok(Some(slot)),
        super::Admission::Counted => Ok(None),
    };
    let slot = match slot {
        Ok(slot) => slot,
        Err(reason) => {
            tracing::warn!("{label} refused: {reason}");
            return Ended {
                status: 1,
                note: Some(reason),
            };
        }
    };
    let (columns, rows) = *start.size.borrow_and_update();
    let (events, inbox) = mpsc::channel::<SessionEvent>();
    let gate = Arc::new(Gate {
        factory: Weak::clone(&start.factory),
        slot: Mutex::new(slot),
        events: events.clone(),
        label: label.clone(),
        logged_in: AtomicBool::new(matches!(start.who, Who::User(_))),
        done: match &start.who {
            Who::Login { done, .. } => done.clone(),
            Who::User(_) => None,
        },
        failures: AtomicUsize::new(0),
        note: Mutex::new(None),
    });
    // The log pane, WARN and ERROR only, for a user who may read Monitor Logs, which shows the same log.
    let (log, link, login_name) = match &start.who {
        Who::User(username) => {
            let log = may_read_log(&start.factory, username).await;
            if log {
                forward_log(&events, username);
            }
            let link = HostLink::weak_as_user(Weak::clone(&start.factory), username.clone());
            (log, link, None)
        }
        Who::Login { name, .. } => (
            false,
            HostLink::weak(Weak::clone(&start.factory)).from_address(start.client_ip.clone()),
            name.clone(),
        ),
    };
    let link = GatedLink {
        link,
        gate: Arc::clone(&gate),
    };
    let (out, mut frames) = tokio_mpsc::channel::<Vec<u8>>(FRAMES_WAITING);
    let (finished, ended) = oneshot::channel();
    let options = SessionOptions {
        path: START_PATH.to_string(),
        theme: start.terminal.theme(),
        columns: columns.max(20),
        rows: rows.max(10),
        notes: Some(|note| tracing::warn!("terminal theme: {note}")),
        log,
        login_name,
    };
    let replies = events.clone();
    let spawned = std::thread::Builder::new()
        .name(format!(
            "term-{}",
            if username.is_empty() { "login" } else { &username }
        ))
        .spawn(move || {
            let mut writer = FrameWriter {
                pending: Vec::new(),
                out,
            };
            let result = moqui_tui::run_session(link, options, &inbox, &replies, &mut writer);
            let _ = writer.flush();
            drop(writer);
            let _ = finished.send(result);
        });
    if let Err(err) = spawned {
        tracing::error!("{label}: could not start its thread: {err}");
        return Ended {
            status: 1,
            note: Some("could not start".to_string()),
        };
    }
    tracing::info!("{label} started at {columns}x{rows}");
    let mut buffer = [0u8; 4096];
    let mut reading = true;
    // Off once the transport drops the size's sender, so a closed watch does not wake the loop again and again.
    let mut sizing = true;
    let mut idle_until = tokio::time::Instant::now() + IDLE;
    let login_until = tokio::time::Instant::now() + LOGIN_WAIT;
    let mut note = None;
    // off once a write failed or stalled: later frames are dropped while the session ends
    let mut writing = true;
    let redraw = start.redraw.clone();
    loop {
        tokio::select! {
            read = stream.read(&mut buffer), if reading => match read {
                Ok(0) | Err(_) => {
                    reading = false;
                    let _ = events.send(SessionEvent::Close);
                }
                Ok(read) => {
                    idle_until = tokio::time::Instant::now() + IDLE;
                    let _ = events.send(SessionEvent::Input(buffer[..read].to_vec()));
                }
            },
            () = tokio::time::sleep_until(idle_until), if reading => {
                reading = false;
                tracing::info!("{label} ended after {IDLE:?} without input");
                note = Some(format!("closed after {} minutes without input", IDLE.as_secs() / 60));
                let _ = events.send(SessionEvent::Close);
            }
            () = tokio::time::sleep_until(login_until), if reading && !gate.logged_in() => {
                reading = false;
                tracing::info!("{label} ended: no login within {LOGIN_WAIT:?}");
                note = Some(format!("closed: no login within {} minutes", LOGIN_WAIT.as_secs() / 60));
                let _ = events.send(SessionEvent::Close);
            }
            frame = frames.recv() => match frame {
                Some(frame) => {
                    let written = writing && matches!(
                        tokio::time::timeout(WRITE_STALL, stream.write_all(&frame)).await,
                        Ok(Ok(()))
                    );
                    if writing && !written {
                        writing = false;
                        reading = false;
                        let _ = events.send(SessionEvent::Close);
                    }
                }
                // The session ended and dropped its writer.
                None => break,
            },
            () = async {
                match &redraw {
                    Some(redraw) => redraw.notified().await,
                    None => std::future::pending().await,
                }
            }, if reading => {
                let _ = events.send(SessionEvent::Reattach);
            }
            changed = start.size.changed(), if sizing => {
                if changed.is_ok() {
                    let (columns, rows) = *start.size.borrow_and_update();
                    let _ = events.send(SessionEvent::Resize { columns, rows });
                } else {
                    sizing = false;
                }
            }
        }
    }
    if writing {
        let _ = tokio::time::timeout(WRITE_STALL, stream.flush()).await;
    }
    let note = note.or_else(|| lock(&gate.note).take());
    // The slot goes back with the gate, once the session's link has dropped it too.
    drop(gate);
    let status = match ended.await {
        Ok(Ok(())) => {
            tracing::info!("{label} ended");
            0
        }
        Ok(Err(err)) => {
            tracing::error!("{label} failed: {err}");
            1
        }
        Err(_) => {
            tracing::error!("{label} stopped without a result");
            1
        }
    };
    Ended { status, note }
}
