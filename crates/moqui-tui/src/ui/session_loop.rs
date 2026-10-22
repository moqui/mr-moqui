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

//! The session loop (section 3.5): the client on a terminal the process does not own, an SSH channel. Bytes come in
//! as [`SessionEvent`]s and go through [`Decoder`]; frames go out through [`Screen`] to any `Write`. Nothing here
//! touches the process terminal, its signals or its panic hook. The loop blocks until an event or the next deadline
//! (a message's expiry, the counters, a search's wait, the clock's minute, a lone ESC, the spinner while a request
//! runs), so an idle session wakes about once a minute.

use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::decode::{Decoded, Decoder};
use crate::link::{Push, Reply, ScreenLink};
use crate::session::Session;
use crate::theme::Theme;

use super::encode::{osc52, Screen, ENTER, LEAVE, MOUSE_OFF, MOUSE_ON};
use super::run::send;
use super::AppView;

/// What the channel, the window and the worker threads tell the loop.
#[derive(Debug)]
pub enum SessionEvent {
    /// Bytes the user's terminal sent.
    Input(Vec<u8>),
    /// The window changed size (an SSH window-change request).
    Resize { columns: u16, rows: u16 },
    /// A request's answer, from a worker thread. Boxed: a screen model is large, and input is most of the traffic.
    Reply(u64, Box<Reply>),
    /// A notification the link pushed for this user.
    Push(Push),
    /// A line for the log pane, for a user who may read the log.
    Log(String),
    /// Whether Alt+L offers the log pane, decided again after a login in the session's dialog.
    LogOffered(bool),
    /// The channel closed: end without writing more.
    Close,
}

/// What a channel passes to the session loop.
#[derive(Debug, Default)]
pub struct SessionOptions {
    pub path: String,
    pub theme: Theme,
    pub columns: u16,
    pub rows: u16,
    /// Called with a note for the log: a theme value that did not apply.
    pub notes: Option<fn(&str)>,
    /// The caller sends [`SessionEvent::Log`] lines, so Alt+L offers the log pane.
    pub log: bool,
    /// The username the login dialog starts with, for a session that starts with no user (the SSH login name).
    pub login_name: Option<String>,
}

/// OSC 11 with `?`: the terminal answers with its background color, which the decoder reads. It ends with ST, as the
/// OSC 8 links do.
const BACKGROUND_QUERY: &[u8] = b"\x1b]11;?\x1b\\";

/// How often the title line's spinner moves while a request runs.
const SPIN: Duration = Duration::from_millis(100);

/// Run the client on `out` until the user quits or the channel closes. `events` brings the input; `replies` is its
/// sender, which the worker threads answer on. The loop keeps a clone of `replies` for them, so the channel never
/// disconnects while it runs: the caller sends [`SessionEvent::Close`] when its input ends. The terminal is put back as
/// it was on every way out, a panic included.
///
/// # Errors
///
/// A write to `out` that failed, or a panic in the client, as an error for the caller's log.
pub fn run_session<L, W>(
    link: L,
    options: SessionOptions,
    events: &mpsc::Receiver<SessionEvent>,
    replies: &mpsc::Sender<SessionEvent>,
    out: &mut W,
) -> io::Result<()>
where
    L: ScreenLink + 'static,
    W: Write,
{
    out.write_all(ENTER)?;
    // Ask the terminal for its background (OSC 11); one that answers picks the dark or light colors, one that does not
    // keeps what COLORFGBG said.
    out.write_all(BACKGROUND_QUERY)?;
    let result = panic::catch_unwind(AssertUnwindSafe(|| run(link, options, events, replies, out)));
    let _ = out.write_all(LEAVE);
    let _ = out.flush();
    match result {
        Ok(result) => result,
        Err(panic) => {
            let message = panic
                .downcast_ref::<&str>()
                .map(ToString::to_string)
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a panic".to_string());
            Err(io::Error::other(format!("the terminal session stopped: {message}")))
        }
    }
}

fn run<L: ScreenLink + 'static, W: Write>(
    link: L,
    options: SessionOptions,
    events: &mpsc::Receiver<SessionEvent>,
    replies: &mpsc::Sender<SessionEvent>,
    out: &mut W,
) -> io::Result<()> {
    let link = Arc::new(link);
    let mut view = AppView::with_session(Session::start(&options.path), Arc::clone(&link));
    view.set_theme(options.theme.clone());
    view.session_mut().resize(options.columns, options.rows);
    let mut screen = Screen::new(options.columns, options.rows, options.theme.depth);
    let mut decoder = Decoder::new();
    let pushes = replies.clone();
    let replies = replies.clone();
    let done = move |id, reply| {
        let _ = replies.send(SessionEvent::Reply(id, Box::new(reply)));
    };
    let subscription = link.subscribe(Arc::new(move |push| {
        let _ = pushes.send(SessionEvent::Push(push));
    }));
    view.session_mut().set_pushed(subscription.is_some());
    view.session_mut().set_log_offered(options.log);
    view.session_mut().set_login_name(options.login_name.clone());
    let mut minute = jiff::Zoned::now().minute();
    settle(&mut view, &link, &done, options.notes, out)?;
    screen.draw(&view, out)?;
    loop {
        let now = Instant::now();
        let wake = [
            view.session().next_deadline(),
            decoder.deadline(),
            view.session().is_busy().then(|| now + SPIN),
        ]
        .into_iter()
        .flatten()
        .fold(next_minute(now), Instant::min);
        let mut batch = Vec::new();
        match events.recv_timeout(wake.saturating_duration_since(now)) {
            Ok(event) => batch.push(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        // Everything already queued is applied before one draw: a held key or a burst of answers is one frame.
        batch.extend(std::iter::from_fn(|| events.try_recv().ok()));
        let mut changed = false;
        for event in batch {
            match event {
                SessionEvent::Input(bytes) => {
                    for decoded in decoder.push(&bytes) {
                        if apply(&mut view, &mut screen, decoded) {
                            return Ok(());
                        }
                        changed = true;
                    }
                }
                SessionEvent::Resize { columns, rows } => {
                    resize(&mut view, &mut screen, columns, rows);
                    changed = true;
                }
                SessionEvent::Reply(id, reply) => {
                    view.session_mut().on_reply(id, *reply);
                    changed = true;
                }
                SessionEvent::Log(line) => {
                    view.push_log(line);
                    changed |= view.session().log_open();
                }
                SessionEvent::Push(push) => {
                    view.session_mut().on_push(&push);
                    changed = true;
                }
                SessionEvent::LogOffered(offered) => {
                    view.session_mut().set_log_offered(offered);
                    changed = true;
                }
                SessionEvent::Close => return Ok(()),
            }
        }
        let now = Instant::now();
        if decoder.deadline().is_some_and(|at| now >= at) {
            for decoded in decoder.flush() {
                if apply(&mut view, &mut screen, decoded) {
                    return Ok(());
                }
                changed = true;
            }
        }
        changed |= view.session_mut().on_time(now);
        let current = jiff::Zoned::now().minute();
        changed |= current != minute;
        minute = current;
        changed |= view.session().is_busy();
        settle(&mut view, &link, &done, options.notes, out)?;
        if changed {
            screen.draw(&view, out)?;
        }
    }
}

/// A decoded key, paste or size. Returns true when the user quit.
fn apply<L: ScreenLink>(view: &mut AppView<Arc<L>>, screen: &mut Screen, decoded: Decoded) -> bool {
    match decoded {
        Decoded::Key(press) => view.key(&press),
        Decoded::Mouse(mouse) => {
            view.on_mouse(&mouse, Instant::now());
            false
        }
        Decoded::Paste(text) => {
            view.session_mut().paste(&text);
            false
        }
        Decoded::Resize { columns, rows } => {
            resize(view, screen, columns, rows);
            false
        }
        Decoded::Background { dark } => {
            view.set_dark(dark);
            false
        }
    }
}

fn resize<L: ScreenLink>(view: &mut AppView<Arc<L>>, screen: &mut Screen, columns: u16, rows: u16) {
    if columns == 0 || rows == 0 || screen.size() == (columns, rows) {
        return;
    }
    view.session_mut().resize(columns, rows);
    screen.resize(columns, rows);
}

/// After each turn: the shell data's theme, notes for the log, the mouse mode and a copy for the terminal, downloads
/// refused, and the queued requests started.
fn settle<L, F, W>(
    view: &mut AppView<Arc<L>>,
    link: &Arc<L>,
    done: &F,
    notes: Option<fn(&str)>,
    out: &mut W,
) -> io::Result<()>
where
    L: ScreenLink + 'static,
    F: Fn(u64, Reply) + Clone + Send + 'static,
    W: Write,
{
    if let Some(on) = view.session_mut().take_mouse_mode() {
        out.write_all(if on { MOUSE_ON } else { MOUSE_OFF })?;
    }
    if let Some(text) = view.session_mut().take_copy() {
        out.write_all(&osc52(&text))?;
    }
    view.sync_shell();
    if let Some(log) = notes {
        for note in view.take_notes() {
            log(&note);
        }
    }
    // The client runs in the server here, so a file would land on the server, not with the user (section 4.2).
    for _ in view.session_mut().take_downloads() {
        view.session_mut()
            .saved(Err("downloads are off in a remote session; use the browser".to_string()));
    }
    send(view, link, done);
    Ok(())
}

/// When the clock on the title line next changes.
fn next_minute(now: Instant) -> Instant {
    let clock = jiff::Zoned::now();
    let into = u64::try_from(clock.second()).unwrap_or(0) * 1000
        + u64::try_from(clock.subsec_nanosecond() / 1_000_000).unwrap_or(0);
    now + Duration::from_millis(60_000 - into.min(59_999))
}
