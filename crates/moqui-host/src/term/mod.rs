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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::sync::lock;
use moqui_tui::Theme;
use tokio::sync::watch;

mod apps;
pub(crate) mod registry;
mod ring;
mod services;
pub(crate) mod session;
mod track;

pub(crate) use apps::serve;

const DEFAULT_MAX: usize = 16;
const DEFAULT_MAX_PER_USER: usize = 4;
/// A session ends after this long without input, with a note. The client's minute clock writes to the stream, which
/// would keep a transport's own inactivity timer from ever firing.
pub(crate) const IDLE: Duration = Duration::from_secs(600);
/// How long a session with no user has to log in through the dialog.
pub(crate) const LOGIN_WAIT: Duration = Duration::from_secs(120);
/// Refused logins in the dialog before the session closes. The account's own lockout counts them too.
pub(crate) const MAX_LOGIN_FAILURES: usize = 5;

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
        let mut live = lock(&self.live);
        if live.values().sum::<usize>() >= self.max {
            return Err(format!(
                "{} terminal sessions are open (terminal max-sessions)",
                self.max
            ));
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
        let mut live = lock(&self.sessions.live);
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
        let mut live = lock(&self.sessions.live);
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
    /// The place in [`Sessions`] already taken for this session (a kept session takes it when it is opened, so a
    /// refusal is the opening's answer); `None` takes one when the session starts.
    pub slot: Option<SessionSlot>,
    /// Notified when a client attaches again to a kept session: the session writes its whole screen again.
    pub redraw: Option<Arc<tokio::sync::Notify>>,
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
            slot: None,
            redraw: None,
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
