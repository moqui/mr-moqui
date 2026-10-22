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

//! The local loop on Revue's `App::run` (section 3.5), and the headless snapshot (4.5). Requests run on worker threads,
//! one per request, which the session's lanes keep to one per purpose; answers come back on a channel and are applied
//! on the next tick, so a slow request never stops the keys.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

use revue::core::app::App;
use revue::event::{Event, Key as RevueKey, KeyEvent};

use crate::keys::{Key, KeyPress, Mouse, MouseKind};
use crate::link::{Push, RawBody, Reply, ScreenLink};
use crate::session::Session;
use crate::theme::Theme;

use super::encode::{osc52, MOUSE_OFF, MOUSE_ON};
use super::AppView;

/// What `moqui tui` passes to the local loop.
#[derive(Debug, Default)]
pub struct LocalOptions {
    pub path: String,
    pub theme: Theme,
    /// Called with a note for the log: a theme value that did not apply.
    pub notes: Option<fn(&str)>,
    /// Log lines for the log pane (Alt+L).
    pub log: Option<mpsc::Receiver<String>>,
}

/// Run the client on this terminal until the user quits.
///
/// # Errors
///
/// Returns Revue's error when the terminal cannot be set up or drawn.
pub fn run_local<L: ScreenLink + 'static>(link: L, options: LocalOptions) -> revue::Result<()> {
    let link = Arc::new(link);
    let mut view = AppView::with_session(Session::start(&options.path), Arc::clone(&link));
    view.set_theme(options.theme.clone());
    let (done_tx, done_rx) = mpsc::channel::<(u64, Reply)>();
    let done_tx = move |id, reply| {
        let _ = done_tx.send((id, reply));
    };
    let notes = options.notes;
    let log = options.log;
    // Revue's terminal setup does not turn on bracketed paste, so a paste would arrive as keys and a newline in it would
    // submit a form; ask for it before the loop and turn it off after (section 3.3).
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[?2004h");
    let _ = out.flush();
    send(&mut view, &link, &done_tx);
    let (push_tx, push_rx) = mpsc::channel::<Push>();
    let subscription = link.subscribe(Arc::new(move |push| {
        let _ = push_tx.send(push);
    }));
    view.session_mut().set_pushed(subscription.is_some());
    // The title line's clock moves once a minute; nothing else redraws an idle screen.
    let mut minute = jiff::Zoned::now().minute();
    let result = App::builder()
        .mouse_capture(false)
        .build()
        .run(view, move |event, view, app| {
            let changed = match event {
                Event::Key(key) => {
                    let quit = from_revue(key).is_some_and(|press| view.key(&press));
                    if quit {
                        app.quit();
                    }
                    true
                }
                Event::Paste(text) => {
                    view.session_mut().paste(text);
                    true
                }
                Event::Resize(width, height) => {
                    view.session_mut().resize(*width, *height);
                    true
                }
                Event::Tick => {
                    let mut any = false;
                    while let Ok((id, reply)) = done_rx.try_recv() {
                        view.session_mut().on_reply(id, reply);
                        any = true;
                    }
                    while let Ok(push) = push_rx.try_recv() {
                        view.session_mut().on_push(&push);
                        any = true;
                    }
                    if let Some(log) = &log {
                        while let Ok(line) = log.try_recv() {
                            view.push_log(line);
                            any |= view.session().log_open();
                        }
                    }
                    // Timers: the message's expiry, the counters, a search waiting for the typing to stop.
                    any |= view.session_mut().on_time(Instant::now());
                    let now = jiff::Zoned::now().minute();
                    any |= now != minute;
                    minute = now;
                    // The spinner moves while a request runs.
                    any || view.session().is_busy()
                }
                // Revue's mouse capture stays off (it is fixed when the app is built); the session's mouse mode is
                // written below, and crossterm reads the reports either way.
                Event::Mouse(mouse) => {
                    from_revue_mouse(mouse).is_some_and(|mouse| view.on_mouse(&mouse, Instant::now()))
                }
                Event::FocusGained | Event::FocusLost => false,
            };
            let mut stdout = std::io::stdout();
            if let Some(on) = view.session_mut().take_mouse_mode() {
                let _ = stdout.write_all(if on { MOUSE_ON } else { MOUSE_OFF });
                let _ = stdout.flush();
            }
            if let Some(text) = view.session_mut().take_copy() {
                let _ = stdout.write_all(&osc52(&text));
                let _ = stdout.flush();
            }
            view.sync_shell();
            if let Some(log) = notes {
                for note in view.take_notes() {
                    log(&note);
                }
            }
            for body in view.session_mut().take_downloads() {
                let saved = save_download(&body);
                view.session_mut().saved(saved);
            }
            send(view, &link, &done_tx);
            changed
        });
    let _ = out.write_all(MOUSE_OFF);
    let _ = out.write_all(b"\x1b[?2004l");
    let _ = out.flush();
    result
}

/// Revue's mouse event as the decoder's: a left, middle or right press, drag or release, or the wheel.
pub fn from_revue_mouse(mouse: &revue::event::MouseEvent) -> Option<Mouse> {
    use revue::event::{MouseButton, MouseEventKind};
    let button = |button: &MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (kind, pressed) = match &mouse.kind {
        MouseEventKind::Down(which) => (MouseKind::Press, button(which)),
        MouseEventKind::Drag(which) => (MouseKind::Drag, button(which)),
        MouseEventKind::Up(which) => (MouseKind::Release, button(which)),
        MouseEventKind::ScrollUp => (MouseKind::WheelUp, 0),
        MouseEventKind::ScrollDown => (MouseKind::WheelDown, 0),
        _ => return None,
    };
    Some(Mouse {
        kind,
        button: pressed,
        column: mouse.x,
        row: mouse.y,
        alt: mouse.alt,
        ctrl: mouse.ctrl,
        shift: mouse.shift,
    })
}

/// Where an export is saved: `XDG_DOWNLOAD_DIR`, else `~/Downloads`, else the current directory.
fn downloads_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DOWNLOAD_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join("Downloads"),
        None => PathBuf::from("."),
    }
}

/// Save a file the server sent under its own name (made safe), not over one already there. In local mode the client
/// runs on the user's machine, so this is their downloads directory (section 1.6).
fn save_download(body: &RawBody) -> Result<String, String> {
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let name: String = body
        .file_name
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let name = if name.trim_matches(['.', '_']).is_empty() {
        let extension = body.content_type.split(['/', ';']).nth(1).unwrap_or("bin");
        format!("export.{}", extension.trim())
    } else {
        name
    };
    let (stem, extension) = name
        .rsplit_once('.')
        .map_or((name.as_str(), ""), |(stem, ext)| (stem, ext));
    let mut path = dir.join(&name);
    let mut n = 1;
    while path.exists() {
        let next = if extension.is_empty() {
            format!("{stem}-{n}")
        } else {
            format!("{stem}-{n}.{extension}")
        };
        path = dir.join(next);
        n += 1;
    }
    std::fs::write(&path, &body.bytes).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(path.display().to_string())
}

/// Start the session's queued requests on worker threads; `done` gets each answer. Both loops use it.
pub(super) fn send<L, F>(view: &mut AppView<Arc<L>>, link: &Arc<L>, done: &F)
where
    L: ScreenLink + 'static,
    F: Fn(u64, Reply) + Clone + Send + 'static,
{
    for request in view.session_mut().take_requests() {
        let link = Arc::clone(link);
        let done = done.clone();
        std::thread::spawn(move || {
            let reply = link.call(&request.call);
            done(request.id, reply);
        });
    }
}

/// The screen at `path` drawn at `width` by `height`, as text: `moqui tui --snapshot`, and a check of a design without a
/// terminal.
pub fn snapshot<L: ScreenLink>(link: L, path: &str, width: u16, height: u16, theme: Theme) -> String {
    let mut view = AppView::with_session(Session::start(path), link);
    view.set_theme(theme);
    view.session_mut().resize(width, height);
    view.run_requests();
    view.screen_text(width, height)
}

/// A Revue key as the client's.
pub fn from_revue(event: &KeyEvent) -> Option<KeyPress> {
    let key = match event.key {
        RevueKey::Char(ch) => Key::Char(ch),
        RevueKey::Enter => Key::Enter,
        RevueKey::Escape => Key::Escape,
        RevueKey::Tab => Key::Tab,
        RevueKey::BackTab => Key::BackTab,
        RevueKey::Backspace => Key::Backspace,
        RevueKey::Delete => Key::Delete,
        RevueKey::Up => Key::Up,
        RevueKey::Down => Key::Down,
        RevueKey::Left => Key::Left,
        RevueKey::Right => Key::Right,
        RevueKey::Home => Key::Home,
        RevueKey::End => Key::End,
        RevueKey::PageUp => Key::PageUp,
        RevueKey::PageDown => Key::PageDown,
        RevueKey::F(n) => Key::F(n),
        RevueKey::Insert | RevueKey::Null | RevueKey::Unknown => return None,
    };
    Some(KeyPress {
        key,
        alt: event.alt,
        ctrl: event.ctrl,
        shift: event.shift,
    })
}
