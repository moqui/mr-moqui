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

//! A session's screen as text (plans/Terminal-Fancy2.md §4.2): what the session shows, kept by the server's pump for a
//! program or an agent to read, never for drawing (D5: a page draws the bytes with xterm.js). It is made the first time
//! something reads it, from the replay a new window would get, and fed beside the tracker from then on (D6); one that
//! nobody read for [`IDLE`] is dropped and made again when asked.

use std::num::NonZeroU16;
use std::time::{Duration, Instant};

/// A screen nobody read for this long is dropped.
pub(crate) const IDLE: Duration = Duration::from_secs(600);
/// The scrollback kept when the conf says nothing (`terminal@screen-scrollback`).
pub(crate) const SCROLLBACK: usize = 1000;

/// Which lines a read wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    /// The rows on the screen now.
    Visible,
    /// What scrolled off, oldest first, then the rows on the screen.
    Scrollback,
    /// The last lines of the scrollback and the screen, the empty rows below the cursor left out.
    Last(usize),
}

impl Scope {
    /// `visible`, `scrollback` or `last` with its count of `lines`; else `None`.
    pub fn parse(scope: &str, lines: usize) -> Option<Self> {
        match scope {
            "" | "visible" => Some(Self::Visible),
            "scrollback" => Some(Self::Scrollback),
            "last" => Some(Self::Last(lines.max(1))),
            _ => None,
        }
    }
}

/// What a read answers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Read {
    pub lines: Vec<String>,
    /// The cursor's row and column on the screen, from 0.
    pub cursor: (u16, u16),
    pub cols: u16,
    pub rows: u16,
    pub alternate: bool,
    /// Made from the kept output, written across the session's resizes: a full-screen program's screen may be partial
    /// until it draws again.
    pub rebuilt: bool,
    /// The screen as escape sequences, for a preview; empty unless asked for.
    pub ansi: String,
    /// The count of the session's output when it was read (the pump's).
    pub count: u64,
}

pub(crate) struct Screen {
    parser: vt100::Parser,
    rebuilt: bool,
    last_read: Instant,
}

fn size(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap_or(NonZeroU16::MIN)
}

impl Screen {
    /// A screen of `cols` by `rows` keeping `scrollback` lines, made from `replay` (the modes' preamble and the kept
    /// output). `rebuilt` when the replay is not the whole of the session's output.
    pub fn new(cols: u16, rows: u16, scrollback: usize, replay: &[u8], rebuilt: bool) -> Self {
        let mut parser = vt100::Parser::new(size(rows), size(cols), scrollback);
        parser.process(replay);
        Self {
            parser,
            rebuilt,
            last_read: Instant::now(),
        }
    }

    /// The program's output from where the screen is.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        // a clear of the whole screen, or the alternate screen entered, draws it anew: the screen is whole again
        if self.rebuilt && (contains(bytes, b"\x1b[2J") || contains(bytes, b"\x1b[?1049h")) {
            self.rebuilt = false;
        }
    }

    /// The window's new size; a program draws its screen again on a change of size.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (now_rows, now_cols) = self.parser.screen().size();
        if (now_rows.get(), now_cols.get()) != (rows, cols) {
            self.parser.set_size(size(rows), size(cols));
            self.rebuilt = false;
        }
    }

    /// Whether nobody read it for [`IDLE`].
    pub fn idle(&self, now: Instant) -> bool {
        now.duration_since(self.last_read) >= IDLE
    }

    /// The lines `scope` names, with the cursor, the size and the modes; with `ansi`, the screen as escape sequences.
    pub fn read(&mut self, scope: Scope, ansi: bool) -> Read {
        self.last_read = Instant::now();
        let screen = self.parser.screen_mut();
        let (rows, cols) = screen.size();
        let (rows, cols) = (rows.get(), cols.get());
        screen.set_scrollback(0);
        let visible: Vec<String> = screen.rows(0, cols).collect();
        let lines = match scope {
            Scope::Visible => visible,
            Scope::Scrollback => {
                let mut lines = scrolled_off(screen, rows);
                lines.extend(visible);
                lines
            }
            Scope::Last(count) => {
                let cursor_row = usize::from(screen.cursor_position().0);
                // the rows below the cursor that hold nothing are not part of what was written
                let used = visible
                    .iter()
                    .rposition(|line| !line.trim_end().is_empty())
                    .map_or(0, |last| last + 1)
                    .max(cursor_row + 1)
                    .min(visible.len());
                let mut lines = if count > used {
                    scrolled_off(screen, rows)
                } else {
                    Vec::new()
                };
                lines.extend(visible.into_iter().take(used));
                let skip = lines.len().saturating_sub(count);
                lines.split_off(skip)
            }
        };
        let screen = self.parser.screen();
        Read {
            lines: lines.into_iter().map(|line| line.trim_end().to_string()).collect(),
            cursor: screen.cursor_position(),
            cols,
            rows,
            alternate: screen.alternate_screen(),
            rebuilt: self.rebuilt,
            ansi: if ansi {
                screen.contents_formatted()
            } else {
                String::new()
            },
            count: 0,
        }
    }
}

/// The scrollback's lines, oldest first: each page of it in view in turn, the screen put back in view after.
fn scrolled_off(screen: &mut vt100::Screen, rows: u16) -> Vec<String> {
    let (_, cols) = screen.size();
    screen.set_scrollback(usize::MAX);
    let total = screen.scrollback();
    let page = usize::from(rows.max(1));
    let mut lines = Vec::with_capacity(total);
    let mut offset = total;
    while offset > 0 {
        screen.set_scrollback(offset);
        // at `offset` the first rows in view are the scrollback's from `total - offset`
        lines.extend(screen.rows(0, cols.get()).take(offset.min(page)));
        offset = offset.saturating_sub(page);
    }
    screen.set_scrollback(0);
    lines
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
