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

//! The terminal client in a session of this server (plans/archive/Screen-TUI-Impl.md 4.2, 4.4): `apps` over SSH and the
//! browser terminal on `/termws`. The client's session loop (`moqui_tui::run_session`) runs on a thread of its own,
//! since it and the in-process link block; a task here carries the stream's bytes and window changes to it and its
//! frames back. Every request runs as the user through the request pool, so a session gives nothing beyond the screens
//! the user may already see.
//!
//! A session starts either as a user the transport already logged in (an SSH key or password, the browser's session
//! cookie) or with no user, and then the client's login dialog logs in (an SSH connection with no key). Such a session
//! closes after [`LOGIN_WAIT`] without a login, or after [`MAX_LOGIN_FAILURES`] refused ones. At most
//! `terminal` `max-sessions` run at once in this process, `terminal` `max-sessions-per-user` of them for one user; a session with
//! no user yet counts toward the first only. Files are not downloaded.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_tui::{
    Choice, LinkError, PushSink, RawBody, ScreenLink, ScreenModel, SessionEvent, SessionOptions, ShellInfo,
    Subscription, Theme, TransitionOutcome,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc as tokio_mpsc, oneshot, watch};

use crate::tui::HostLink;

const DEFAULT_MAX: usize = 16;
const DEFAULT_MAX_PER_USER: usize = 4;
/// Where a session starts: the apps' root, whose menu lists the apps the user may open.
const START_PATH: &str = "/apps";
/// A session ends after this long without input, with a note. The client's minute clock writes to the stream, which
/// would keep a transport's own inactivity timer from ever firing.
pub(crate) const IDLE: Duration = Duration::from_secs(600);
/// How long a session with no user has to log in through the dialog.
pub(crate) const LOGIN_WAIT: Duration = Duration::from_secs(120);
/// Refused logins in the dialog before the session closes. The account's own lockout counts them too.
pub(crate) const MAX_LOGIN_FAILURES: usize = 5;
/// Log lines a session's pane holds while it is closed.
const LOG_LINES: usize = 500;

/// What the client said about its terminal: `TERM`, and the variables that say how it draws (`COLORTERM`,
/// `NO_COLOR`, `COLORFGBG`, the locale).
#[derive(Clone, Debug, Default)]
pub(crate) struct Terminal {
    pub term: String,
    pub env: BTreeMap<String, String>,
}

/// The variables an SSH `env` request may set for a session.
#[cfg(feature = "ssh")]
pub(crate) fn is_terminal_variable(name: &str) -> bool {
    matches!(
        name,
        "COLORTERM" | "NO_COLOR" | "COLORFGBG" | "LANG" | "LC_ALL" | "LC_CTYPE"
    )
}

impl Terminal {
    /// The theme for this terminal. A locale that is set and is not UTF-8 draws in ASCII.
    fn theme(&self) -> Theme {
        let env = |name: &str| match name {
            "TERM" => Some(self.term.clone()),
            other => self.env.get(other).cloned(),
        };
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(|name| self.env.get(name).filter(|value| !value.is_empty()));
        let ascii = locale.is_some_and(|locale| {
            let locale = locale.to_ascii_lowercase();
            !(locale.contains("utf-8") || locale.contains("utf8"))
        });
        Theme::detect(&env, ascii, false)
    }
}

/// The conf's `terminal` attribute `name` as a positive number, else `default`.
fn terminal_number(factory: &ExecutionContextFactory, name: &str, default: usize) -> usize {
    Some(factory.conf_number("terminal", name, default))
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

/// The live terminal sessions of this process, over every transport, against `terminal` `max-sessions` and
/// `terminal` `max-sessions-per-user`. A session with no user yet is counted under the empty name, toward the first only.
pub(crate) struct Sessions {
    max: usize,
    max_per_user: usize,
    live: Mutex<HashMap<String, usize>>,
}

impl Sessions {
    /// The factory's sessions, by the conf's `terminal` element when first asked for.
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|factory| {
            Self::new(
                terminal_number(factory, "max-sessions", DEFAULT_MAX),
                terminal_number(factory, "max-sessions-per-user", DEFAULT_MAX_PER_USER),
            )
        })
    }

    pub fn new(max: usize, max_per_user: usize) -> Self {
        Self {
            max,
            max_per_user,
            live: Mutex::new(HashMap::new()),
        }
    }

    /// A place for one more session of `username` (empty for no user yet), or why there is none. Drop gives it back.
    pub fn take(self: &Arc<Self>, username: &str) -> Result<SessionSlot, String> {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if live.values().sum::<usize>() >= self.max {
            return Err(format!("{} terminal sessions are open (terminal max-sessions)", self.max));
        }
        self.full_for(&live, username)?;
        *live.entry(username.to_string()).or_default() += 1;
        Ok(SessionSlot {
            sessions: Arc::clone(self),
            username: username.to_string(),
        })
    }

    fn full_for(&self, live: &HashMap<String, usize>, username: &str) -> Result<(), String> {
        if !username.is_empty() && live.get(username).copied().unwrap_or(0) >= self.max_per_user {
            return Err(format!(
                "you have {} terminal sessions open (terminal max-sessions-per-user)",
                self.max_per_user
            ));
        }
        Ok(())
    }
}

/// One session's place in [`Sessions`].
pub(crate) struct SessionSlot {
    sessions: Arc<Sessions>,
    username: String,
}

impl SessionSlot {
    /// Count this session as `username`'s from now on, or why not; unchanged when refused.
    pub fn assign(&mut self, username: &str) -> Result<(), String> {
        if self.username == username {
            return Ok(());
        }
        let mut live = self.sessions.live.lock().unwrap_or_else(PoisonError::into_inner);
        self.sessions.full_for(&live, username)?;
        release(&mut live, &self.username);
        *live.entry(username.to_string()).or_default() += 1;
        self.username = username.to_string();
        Ok(())
    }
}

fn release(live: &mut HashMap<String, usize>, username: &str) {
    if let Some(count) = live.get_mut(username) {
        *count = count.saturating_sub(1);
        if *count == 0 {
            live.remove(username);
        }
    }
}

impl Drop for SessionSlot {
    fn drop(&mut self) {
        let mut live = self.sessions.live.lock().unwrap_or_else(PoisonError::into_inner);
        release(&mut live, &self.username);
    }
}

/// Who a session runs as.
#[derive(Clone, Debug)]
pub(crate) enum Who {
    /// A user the transport logged in.
    User(String),
    /// No user yet: the client's dialog logs in, starting with `name` (the SSH login name). `done` is set when it has.
    /// Only SSH starts one.
    #[cfg_attr(not(feature = "ssh"), allow(dead_code))]
    Login {
        name: Option<String>,
        done: Option<Arc<AtomicBool>>,
    },
}

/// What a session needs from its transport.
pub(crate) struct Start {
    pub factory: Weak<ExecutionContextFactory>,
    pub sessions: Arc<Sessions>,
    pub who: Who,
    pub terminal: Terminal,
    /// The window's columns and rows: the first size, then each change.
    pub size: watch::Receiver<(u16, u16)>,
    /// For the log: what and from where, such as `SSH apps from 127.0.0.1:50122`.
    pub label: String,
    /// The client's address, which a login in the dialog checks `ipAllowed` against; empty checks nothing.
    pub client_ip: String,
}

impl Start {
    /// The same start again with its own window watch: `apps` typed a second time at the SSH prompt.
    #[cfg(feature = "ssh")]
    pub fn again(&self) -> Self {
        Self {
            factory: Weak::clone(&self.factory),
            sessions: Arc::clone(&self.sessions),
            who: self.who.clone(),
            terminal: self.terminal.clone(),
            size: self.size.clone(),
            label: self.label.clone(),
            client_ip: self.client_ip.clone(),
        }
    }
}

/// How a session ended: the exit status, and a line for the user (a refusal, a timeout), which the transport shows.
pub(crate) struct Ended {
    /// The channel's exit status over SSH; the browser socket has none.
    #[cfg_attr(not(feature = "ssh"), allow(dead_code))]
    pub status: u32,
    pub note: Option<String>,
}

/// Frames to the stream task: each `flush` sends what was written since as one chunk, so a frame goes out whole.
struct FrameWriter {
    pending: Vec<u8>,
    out: tokio_mpsc::UnboundedSender<Vec<u8>>,
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
            .send(std::mem::take(&mut self.pending))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the stream closed"))
    }
}

/// Whether `username` may view the System app (`component://tools/screen/System.xml`, which its Log Viewer inherits),
/// checked on the request pool as that user. A check that fails is a no. Blocks until the pool answers.
fn may_read_log_now(factory: &Weak<ExecutionContextFactory>, username: &str) -> bool {
    let Some(factory) = factory.upgrade() else {
        return false;
    };
    let username = username.to_string();
    let owner = Arc::clone(&factory);
    let Ok(rx) = factory.request_pool().try_submit(owner, move || {
        moqui_core::user::internal_login_user(&username)?;
        Ok(moqui_core::user::authorize("AT_XML_SCREEN", "component://tools/screen/System.xml", "view").is_ok())
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
    slot: Mutex<SessionSlot>,
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

    fn options(&self, url: &str, parameters: &BTreeMap<String, String>) -> Result<Vec<Choice>, LinkError> {
        self.link.options(url, parameters)
    }

    fn shell(&self, path: &str) -> Result<ShellInfo, LinkError> {
        self.link.shell(path)
    }

    fn login(&self, username: &str, password: &str, code: &str) -> Result<(), LinkError> {
        let gate = &self.gate;
        let lock = || gate.slot.lock().unwrap_or_else(PoisonError::into_inner);
        // The user's slot is taken before the password is checked, and given back when the login fails.
        let before = lock().username.clone();
        lock().assign(username).map_err(LinkError::Remote)?;
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
                if let Err(reason) = lock().assign(&before) {
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
                    *gate.note.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(format!("closed after {MAX_LOGIN_FAILURES} refused logins"));
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
    let slot = match start.sessions.take(&username) {
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
    // The log pane, WARN and ERROR only, for a user who may view the System app, whose Log Viewer reads the same log.
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
    let (out, mut frames) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
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
                    if stream.write_all(&frame).await.is_err() {
                        reading = false;
                        let _ = events.send(SessionEvent::Close);
                    }
                }
                // The session ended and dropped its writer.
                None => break,
            },
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
    let _ = stream.flush().await;
    let note = note.or_else(|| gate.note.lock().unwrap_or_else(PoisonError::into_inner).take());
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Sessions, Terminal};

    #[test]
    fn sessions_are_limited_in_all_and_per_user() {
        let sessions = Arc::new(Sessions::new(3, 2));
        let first = sessions.take("a").unwrap();
        let _second = sessions.take("a").unwrap();
        assert!(sessions.take("a").err().unwrap().contains("max-sessions-per-user"));
        let _third = sessions.take("b").unwrap();
        assert!(sessions.take("c").err().unwrap().contains("max-sessions"));
        drop(first);
        assert!(sessions.take("c").is_ok());
    }

    #[test]
    fn a_session_with_no_user_takes_the_users_slot_at_login() {
        let sessions = Arc::new(Sessions::new(4, 1));
        let _a = sessions.take("a").unwrap();
        let mut login = sessions.take("").unwrap();
        let _other = sessions.take("").unwrap();
        assert!(login.assign("a").unwrap_err().contains("max-sessions-per-user"));
        login.assign("b").unwrap();
        assert!(sessions.take("b").is_err());
        login.assign("").unwrap();
        let b = sessions.take("b").unwrap();
        drop(login);
        drop(b);
        assert!(sessions.take("b").is_ok());
    }

    #[test]
    fn the_terminal_sets_colors_and_ascii() {
        let mut terminal = Terminal {
            term: "xterm-256color".to_string(),
            ..Terminal::default()
        };
        assert_eq!(terminal.theme().depth, moqui_tui::Depth::Ansi256);
        assert!(!terminal.theme().ascii);
        terminal.env.insert("COLORTERM".to_string(), "truecolor".to_string());
        terminal.env.insert("LANG".to_string(), "C".to_string());
        assert_eq!(terminal.theme().depth, moqui_tui::Depth::TrueColor);
        assert!(terminal.theme().ascii);
        terminal.env.insert("LC_ALL".to_string(), "en_US.UTF-8".to_string());
        assert!(!terminal.theme().ascii);
    }
}
