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

//! The kept terminal sessions of this process (plans/Terminal-Fancy.md §5.2), by id. A session is found only by its
//! own user: another user's id answers as one that does not exist. An apps session is kept `terminal`
//! `detach-seconds` (120) with no window, a command or host session `host-detach-hours` (72); a logout leaves them
//! (D2), and the server's stop ends them all. Sessions are this process's own: on a cluster an id reaches only the
//! member that holds it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync::lock;
use tokio::sync::{watch, Notify};

use super::session::{self, Kind, Program, Session, Spec};
use super::{terminal_number, Admission, Ended, Sessions, Start, Terminal, Who};

const DETACH_SECONDS: usize = 120;
const HOST_DETACH_HOURS: usize = 72;
const REPLAY_KB: usize = 2048;
/// The pipe between an apps or command session's program and its pump, each way.
const PIPE: usize = 1 << 20;

pub(crate) struct Registry {
    live: Mutex<HashMap<String, Arc<Session>>>,
    /// An apps session with no window this long is ended.
    pub detach: Duration,
    /// A command or host session with no window this long is ended.
    pub host_detach: Duration,
    /// The output a command or host session keeps for a window that comes back.
    pub replay: usize,
}

impl Registry {
    /// The factory's sessions, by the conf's `terminal` element when first asked for.
    pub fn of(factory: &ExecutionContextFactory) -> Arc<Self> {
        factory.extension(|factory| {
            let mut registry = Self::new(Duration::from_secs(
                terminal_number(factory, "detach-seconds", DETACH_SECONDS) as u64,
            ));
            registry.host_detach =
                Duration::from_secs(terminal_number(factory, "host-detach-hours", HOST_DETACH_HOURS) as u64 * 3600);
            registry.replay = terminal_number(factory, "replay-kb", REPLAY_KB) * 1024;
            registry
        })
    }

    pub fn new(detach: Duration) -> Self {
        Self {
            live: Mutex::new(HashMap::new()),
            detach,
            host_detach: Duration::from_secs(HOST_DETACH_HOURS as u64 * 3600),
            replay: REPLAY_KB * 1024,
        }
    }

    pub(super) fn insert(&self, session: Arc<Session>) {
        lock(&self.live).insert(session.id.clone(), session);
    }

    pub(super) fn remove(&self, id: &str) {
        lock(&self.live).remove(id);
    }

    /// The session `id` when it is `username`'s.
    pub fn find(&self, id: &str, username: &str) -> Option<Arc<Session>> {
        lock(&self.live)
            .get(id)
            .filter(|session| session.username == username)
            .cloned()
    }

    /// The sessions of `username`, oldest first.
    pub fn list(&self, username: &str) -> Vec<Arc<Session>> {
        let mut sessions: Vec<Arc<Session>> = lock(&self.live)
            .values()
            .filter(|session| session.username == username)
            .cloned()
            .collect();
        sessions.sort_by_key(|session| session.started);
        sessions
    }

    /// The server is stopping: every session ends.
    pub fn end_all(&self) {
        let sessions: Vec<Arc<Session>> = lock(&self.live).values().cloned().collect();
        for session in sessions {
            session.end("the server is stopping");
        }
    }

    /// Open an apps session of `username` at `cols` by `rows`: its place in the `terminal` limits, then the client
    /// running as the user with no window yet. The refusal is the limit's reason.
    pub fn open_apps(
        self: &Arc<Self>,
        factory: &Arc<ExecutionContextFactory>,
        username: &str,
        title: &str,
        cols: u16,
        rows: u16,
    ) -> Result<Arc<Session>, String> {
        let sessions = Sessions::of(factory);
        let slot = sessions.take(username)?;
        let id = new_id()?;
        let (size, size_in) = watch::channel((cols, rows));
        let redraw = Arc::new(Notify::new());
        let start = Start {
            factory: Arc::downgrade(factory),
            sessions,
            who: Who::User(username.to_string()),
            terminal: browser_terminal(),
            size: size_in,
            label: format!("browser terminal {id} of {username}"),
            client_ip: String::new(),
            // the client holds the place it was opened with, and gives it back when it ends
            admission: Admission::Held(slot),
            redraw: Some(Arc::clone(&redraw)),
        };
        let program = apps_program(start, size, redraw);
        let spec = Spec {
            id,
            username: username.to_string(),
            kind: Kind::Apps,
            profile: String::new(),
            title: if title.is_empty() {
                "Apps".to_string()
            } else {
                title.to_string()
            },
            cwd: String::new(),
            cols,
            rows,
            detached_for: self.detach,
            replay: 0,
            slot: None,
            on_end: None,
        };
        Ok(session::spawn(self, spec, program))
    }
}

/// The browser's terminal: what the apps and the command line are told about it.
fn browser_terminal() -> Terminal {
    Terminal {
        term: "xterm-256color".to_string(),
        env: BTreeMap::from([
            ("COLORTERM".to_string(), "truecolor".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
        ]),
    }
}

/// What a command line session is opened with.
#[cfg(feature = "devtools")]
pub(crate) struct CommandSpec {
    pub id: String,
    pub username: String,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    pub on_end: Option<Box<dyn FnOnce(u32, String) + Send>>,
}

#[cfg(feature = "devtools")]
impl Registry {
    /// Open a `moqui>` command line of `username` (plans/Terminal-Fancy.md §6): its place in the `terminal` limits,
    /// then the prompt's loop on a pipe. The caller checked who may and from where (`super::access`).
    pub fn open_command(
        self: &Arc<Self>,
        factory: &Arc<ExecutionContextFactory>,
        spec: CommandSpec,
    ) -> Result<Arc<Session>, String> {
        let sessions = Sessions::of(factory);
        let slot = sessions.take(&spec.username)?;
        let (size, size_in) = watch::channel((spec.cols, spec.rows));
        let redraw = Arc::new(Notify::new());
        // `apps` at this prompt runs in the place the command line holds
        let apps = Start {
            factory: Arc::downgrade(factory),
            sessions,
            who: Who::User(spec.username.clone()),
            terminal: browser_terminal(),
            size: size_in,
            label: format!("browser command line {} of {}: apps", spec.id, spec.username),
            client_ip: String::new(),
            admission: Admission::Counted,
            redraw: Some(Arc::clone(&redraw)),
        };
        let (ours, theirs) = tokio::io::duplex(PIPE);
        let (output, input) = tokio::io::split(ours);
        let weak = Arc::downgrade(factory);
        let username = spec.username.clone();
        let running = crate::edge::handle()
            .spawn(async move { crate::command::interactive(theirs, &weak, &username, true, Ok(apps)).await });
        let program = Program {
            output: Box::pin(output),
            input: Box::pin(input),
            resize: Box::new(move |cols, rows| {
                size.send_replace((cols, rows));
            }),
            // the ring replays the prompt; `apps` running at it draws itself again
            redraw: Some(Box::new(move || redraw.notify_one())),
            hang_up: None,
            kill: None,
            ended: Box::pin(async move {
                Ended {
                    status: running.await.unwrap_or(1),
                    note: None,
                }
            }),
        };
        let spec = Spec {
            id: spec.id,
            username: spec.username,
            kind: Kind::Command,
            profile: String::new(),
            title: if spec.title.is_empty() {
                "moqui>".to_string()
            } else {
                spec.title
            },
            cwd: String::new(),
            cols: spec.cols,
            rows: spec.rows,
            detached_for: self.host_detach,
            replay: self.replay,
            slot: Some(slot),
            on_end: spec.on_end,
        };
        Ok(session::spawn(self, spec, program))
    }
}

/// A new session id: 22 URL-safe characters, 132 random bits.
pub(crate) fn new_id() -> Result<String, String> {
    moqui_core::user::random_string(22).map_err(|err| err.message().to_string())
}

/// The apps client on a pipe, as a session's program. Its input closing ends it, as a socket closing does.
fn apps_program(mut start: Start, size: watch::Sender<(u16, u16)>, redraw: Arc<Notify>) -> Program {
    let (ours, mut theirs) = tokio::io::duplex(PIPE);
    let (output, input) = tokio::io::split(ours);
    let running = crate::edge::handle().spawn(async move { super::serve(&mut theirs, &mut start).await });
    Program {
        output: Box::pin(output),
        input: Box::pin(input),
        resize: Box::new(move |cols, rows| {
            size.send_replace((cols, rows));
        }),
        redraw: Some(Box::new(move || redraw.notify_one())),
        hang_up: None,
        kill: None,
        ended: Box::pin(async move {
            running.await.unwrap_or(Ended {
                status: 1,
                note: Some("the session stopped".to_string()),
            })
        }),
    }
}

/// The server's stop, through the extension: every kept session ends.
pub(crate) fn before_shutdown(factory: &ExecutionContextFactory) {
    Registry::of(factory).end_all();
}
