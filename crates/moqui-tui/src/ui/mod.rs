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

//! The Revue view (section 3.2). [`AppView`] holds the session, the theme and the breakpoints, and its `render` draws the
//! frame and the canvas from them every frame: the session owns the state, the view owns none but the canvas scroll.

mod agent;
mod draw;
mod encode;
mod overlays;
pub(crate) mod paint;
mod run;
mod session_loop;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use revue::testing::PipelineHarness;
use revue::widget::{RenderContext, View};

use crate::keys::{KeyPress, Mouse, MouseKind, Target};
use crate::layout::Breakpoints;
use crate::link::{FakeLink, ScreenLink, ShellInfo};
use crate::model::ScreenModel;
use crate::session::Session;
use crate::theme::Theme;

pub use agent::{Agent, AgentAnswer, ChangedField, Snapshot as AgentSnapshot, AGENT_HELP};
pub use encode::{Screen, ENTER, LEAVE, MOUSE_OFF, MOUSE_ON};
pub use paint::{Hit, Hits};
pub use run::{from_revue, run_local, snapshot, LocalOptions};
pub use session_loop::{run_session, SessionEvent, SessionOptions};

pub struct AppView<L: ScreenLink = FakeLink> {
    session: Session,
    link: L,
    theme: Theme,
    base_theme: Theme,
    breakpoints: Breakpoints,
    scroll: Cell<usize>,
    /// The title line's clock, fixed for a snapshot; live when None.
    clock: Option<String>,
    /// The shell data the theme and breakpoints were last taken from.
    applied: ShellInfo,
    notes: Vec<String>,
    /// The newest log lines, for the log pane (Alt+L).
    log: VecDeque<String>,
    /// What a click acts on in the last frame, and that frame's characters by row (a wide glyph's second cell is NUL):
    /// the mouse reads them (Phase 5).
    hits: RefCell<Hits>,
    frame: RefCell<Vec<Vec<char>>>,
    pointer: Pointer,
    /// The wheel moved the canvas: it stays there until a key or a click.
    free_scroll: Cell<bool>,
}

/// The mouse between its reports: where a button went down and on what, the last click for a double click, and a
/// drag's selection.
#[derive(Default)]
struct Pointer {
    press: Option<(u16, u16, Option<Target>)>,
    last_click: Option<(Instant, u16, u16, u8)>,
    selection: Option<Selection>,
}

/// Cells selected by a drag, from the press to the pointer: in reading order, or a rectangle with Alt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub from: (u16, u16),
    pub to: (u16, u16),
    pub rect: bool,
}

impl Selection {
    /// The selected columns of `row`, as a range, on a frame `width` wide.
    fn columns(&self, row: u16, width: u16) -> Option<std::ops::Range<u16>> {
        let ((c1, r1), (c2, r2)) = (self.from, self.to);
        let (top, bottom) = (r1.min(r2), r1.max(r2));
        if row < top || row > bottom {
            return None;
        }
        if self.rect {
            return Some(c1.min(c2)..c1.max(c2) + 1);
        }
        let ((start_col, start_row), (end_col, end_row)) = if (r1, c1) <= (r2, c2) {
            ((c1, r1), (c2, r2))
        } else {
            ((c2, r2), (c1, r1))
        };
        let start = if row == start_row { start_col } else { 0 };
        let end = if row == end_row { end_col + 1 } else { width };
        (start < end).then_some(start..end)
    }
}

/// Two clicks on one cell this close together are a double click, three a triple.
const CLICK_GAP: Duration = Duration::from_millis(400);
/// Rows the wheel scrolls the canvas or a text area.
const WHEEL_ROWS: isize = 3;

/// Lines the log pane keeps.
const LOG_LINES: usize = 500;

impl<L: ScreenLink> AppView<L> {
    pub fn new(model: ScreenModel, link: L) -> Self {
        Self::with_session(Session::new(model), link)
    }

    pub fn with_session(session: Session, link: L) -> Self {
        let theme = Theme::default();
        Self {
            session,
            link,
            base_theme: theme.clone(),
            theme,
            breakpoints: Breakpoints::default(),
            scroll: Cell::new(0),
            clock: None,
            applied: ShellInfo::default(),
            notes: Vec::new(),
            log: VecDeque::new(),
            hits: RefCell::new(Hits::default()),
            frame: RefCell::new(Vec::new()),
            pointer: Pointer::default(),
            free_scroll: Cell::new(false),
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn session_mut(&mut self) -> &mut Session {
        &mut self.session
    }

    pub fn link(&self) -> &L {
        &self.link
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The terminal's theme (colors, ASCII); the server theme's colors are merged over it when the shell data comes.
    pub fn set_theme(&mut self, theme: Theme) {
        self.base_theme = theme.clone();
        self.theme = theme;
        self.applied = ShellInfo::default();
        self.sync_shell();
    }

    /// The terminal said its background is dark or light (OSC 11): the built-in pair for it, with the same depth and
    /// ASCII choice, and the server's colors merged again.
    pub fn set_dark(&mut self, dark: bool) {
        if self.base_theme.dark != dark {
            let theme = self.base_theme.with_background(dark);
            self.set_theme(theme);
        }
    }

    /// Show this in place of the clock, so a snapshot does not change by the minute.
    pub fn set_clock(&mut self, clock: &str) {
        self.clock = Some(clock.to_string());
    }

    /// A key, and its requests run on this thread. Returns true when the client should end.
    pub fn on_key(&mut self, press: &KeyPress) -> bool {
        self.pointer_off();
        let quit = self.session.press(press, &self.link);
        self.sync_shell();
        quit
    }

    /// A key for a loop that runs the requests itself: the selection and a wheel's scroll end, and the session takes
    /// the key. Returns true when the client should end.
    pub fn key(&mut self, press: &KeyPress) -> bool {
        self.pointer_off();
        self.session.on_key(press)
    }

    fn pointer_off(&mut self) {
        self.pointer.selection = None;
        self.free_scroll.set(false);
    }

    /// The target drawn at a cell of the last frame.
    pub fn target_at(&self, column: u16, row: u16) -> Option<Target> {
        self.hits.borrow().at(column, row).cloned()
    }

    /// The drag's selection on the last frame, if one is shown.
    pub fn selection(&self) -> Option<Selection> {
        self.pointer.selection
    }

    /// A mouse report (Phase 5): a press and release on one target is a click, which the session acts on as Enter; a
    /// press that moves to another cell is a drag that selects the frame's text, copied on release; the wheel moves a
    /// list, a text area or the canvas. Returns true when the frame changes. Requests are queued, not run.
    pub fn on_mouse(&mut self, mouse: &Mouse, now: Instant) -> bool {
        if !self.session.mouse_on() {
            return false;
        }
        let (column, row) = (mouse.column, mouse.row);
        match mouse.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let down = mouse.kind == MouseKind::WheelDown;
                if self.session.wheel(down) {
                    return true;
                }
                let rows = if down { WHEEL_ROWS } else { -WHEEL_ROWS };
                if let Some(Target::Focus(key)) = self.target_at(column, row) {
                    if matches!(
                        self.session.field_widget(&key),
                        Some(crate::model::Widget::TextArea { .. })
                    ) {
                        self.session.scroll_area(&key, rows);
                        return true;
                    }
                }
                self.free_scroll.set(true);
                self.scroll.set(self.scroll.get().saturating_add_signed(rows));
                true
            }
            MouseKind::Press => {
                let shown = self.pointer.selection.take().is_some();
                if mouse.button != 0 {
                    return shown;
                }
                self.pointer.press = Some((column, row, self.target_at(column, row)));
                shown
            }
            MouseKind::Drag => {
                let Some((from_column, from_row, _)) = self.pointer.press.clone() else {
                    return false;
                };
                if (from_column, from_row) == (column, row) && self.pointer.selection.is_none() {
                    return false;
                }
                self.pointer.selection = Some(Selection {
                    from: (from_column, from_row),
                    to: (column, row),
                    rect: mouse.alt,
                });
                true
            }
            MouseKind::Release => {
                let Some((from_column, from_row, pressed)) = self.pointer.press.take() else {
                    return false;
                };
                if let Some(selection) = self.pointer.selection {
                    let text = self.selected_text(&selection);
                    self.session.copy_text(text);
                    return true;
                }
                let count = match self.pointer.last_click {
                    Some((at, last_column, last_row, count))
                        if now.duration_since(at) <= CLICK_GAP && (last_column, last_row) == (column, row) =>
                    {
                        (count + 1).min(3)
                    }
                    _ => 1,
                };
                self.pointer.last_click = Some((now, column, row, count));
                let released = self.target_at(column, row);
                match (pressed, released) {
                    (Some(pressed), Some(released)) if pressed == released => {
                        self.free_scroll.set(false);
                        self.session.click(&released, count);
                        true
                    }
                    // Text that is no target: a double click selects the word and copies it, a triple the row.
                    (None, None) if (from_column, from_row) == (column, row) && count >= 2 => {
                        let selection = self.word_or_row(column, row, count == 3);
                        if let Some(selection) = selection {
                            self.pointer.selection = Some(selection);
                            let text = self.selected_text(&selection);
                            self.session.copy_text(text);
                        }
                        true
                    }
                    _ => false,
                }
            }
        }
    }

    /// The frame's text under a selection: what is on screen, a wide glyph once, each row's trailing blanks dropped.
    fn selected_text(&self, selection: &Selection) -> String {
        let frame = self.frame.borrow();
        let width = frame
            .first()
            .map_or(0, |row| u16::try_from(row.len()).unwrap_or(u16::MAX));
        let mut lines = Vec::new();
        for (at, cells) in frame.iter().enumerate() {
            let Ok(row) = u16::try_from(at) else {
                break;
            };
            let Some(range) = selection.columns(row, width) else {
                continue;
            };
            let line: String = cells
                .iter()
                .skip(usize::from(range.start))
                .take(usize::from(range.end - range.start))
                .filter(|ch| **ch != '\0')
                .collect();
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    /// The word at a cell (letters, digits and `_.-@/:`), or the whole row.
    fn word_or_row(&self, column: u16, row: u16, whole: bool) -> Option<Selection> {
        let frame = self.frame.borrow();
        let cells = frame.get(usize::from(row))?;
        let last = u16::try_from(cells.len().checked_sub(1)?).ok()?;
        if whole {
            return Some(Selection {
                from: (0, row),
                to: (last, row),
                rect: false,
            });
        }
        let word = |ch: char| ch.is_alphanumeric() || "_.-@/:".contains(ch);
        if !cells.get(usize::from(column)).copied().is_some_and(word) {
            return None;
        }
        let mut start = usize::from(column);
        while start > 0 && cells[start - 1] != '\0' && word(cells[start - 1]) {
            start -= 1;
        }
        let mut end = usize::from(column);
        while end + 1 < cells.len() && word(cells[end + 1]) {
            end += 1;
        }
        Some(Selection {
            from: (u16::try_from(start).ok()?, row),
            to: (u16::try_from(end).ok()?, row),
            rect: false,
        })
    }

    /// Run what the session queued, on this thread.
    pub fn run_requests(&mut self) {
        self.session.run_now(&self.link);
        self.sync_shell();
    }

    /// Take the theme's colors, breakpoints and key bindings once the shell data changes. What does not read is noted
    /// for the log, and the defaults stay (D11).
    pub fn sync_shell(&mut self) {
        if self.session.shell() == &self.applied {
            return;
        }
        let shell = self.session.shell().clone();
        let mut theme = self.base_theme.clone();
        self.notes.extend(theme.merge_colors(&shell.colors));
        self.theme = theme;
        let (breakpoints, skipped) = Breakpoints::with(&shell.breakpoints);
        self.breakpoints = breakpoints;
        self.notes.extend(skipped);
        self.notes
            .extend(self.session.key_skips().iter().map(ToString::to_string));
        self.applied = shell;
    }

    /// A log line for the log pane.
    pub fn push_log(&mut self, line: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    /// What did not apply from the theme, for the log.
    pub fn take_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notes)
    }

    /// The screen as text at `width` by `height`, drawn through Revue's real pipeline.
    pub fn screen_text(&mut self, width: u16, height: u16) -> String {
        let mut harness = PipelineHarness::new(width, height);
        harness.draw(self);
        // The second cell of a wide glyph is a NUL spacer. Drop it so the text is the characters a reader sees.
        harness
            .screen_text()
            .replace('\0', "")
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl<L: ScreenLink> View for AppView<L> {
    fn render(&self, ctx: &mut RenderContext) {
        let log = self.session.log_open().then_some(&self.log);
        let mut hits = Hits::default();
        draw::draw(
            &self.session,
            &self.theme,
            &self.breakpoints,
            draw::Kept {
                scroll: &self.scroll,
                free_scroll: self.free_scroll.get(),
                hits: &mut hits,
            },
            self.clock.as_deref(),
            log,
            ctx,
        );
        *self.hits.borrow_mut() = hits;
        // The frame's characters, for a selection's text; then the selection over the frame, in reverse video.
        let area = ctx.area;
        let mut frame = Vec::with_capacity(usize::from(area.height));
        for row in 0..area.height {
            let cells = (0..area.width)
                .map(|column| {
                    ctx.buffer.get(area.x + column, area.y + row).map_or(' ', |cell| {
                        if cell.is_continuation() {
                            '\0'
                        } else {
                            cell.symbol
                        }
                    })
                })
                .collect();
            frame.push(cells);
        }
        *self.frame.borrow_mut() = frame;
        if let Some(selection) = self.pointer.selection {
            for row in 0..area.height {
                let Some(range) = selection.columns(row, area.width) else {
                    continue;
                };
                for column in range {
                    if let Some(cell) = ctx.buffer.get_mut(area.x + column, area.y + row) {
                        cell.modifier |= revue::render::Modifier::REVERSE;
                    }
                }
            }
        }
    }
}

/// One screen as text, for a snapshot test.
pub fn render_text(model: &ScreenModel, link: &FakeLink, width: u16, height: u16) -> String {
    let mut view = AppView::new(model.clone(), link.clone());
    view.set_clock("Tue 14:32");
    view.screen_text(width, height)
}
