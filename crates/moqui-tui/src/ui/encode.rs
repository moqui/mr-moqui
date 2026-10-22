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

//! Frames as terminal bytes for a session the process does not own (section 3.5, route 4). The view draws into a
//! Revue `Buffer` through the public `RenderContext`, `revue::render::diff` finds the cells that changed since the last
//! frame, and this writes them: a cursor move only on a jump, the SGR of each change of look in the colors the
//! terminal has, OSC 8 by URL. Revue's own `Terminal::render` is not used because it compares hyperlinks against a
//! buffer without the registry, so every linked cell would be rewritten each frame, and it always writes true color.

use std::io::{self, Write};

use revue::layout::Rect;
use revue::render::{diff, Buffer, Cell, Change, Modifier};
use revue::style::Color;
use revue::widget::{RenderContext, View};

use crate::layout::char_width;
use crate::theme::{sgr_color, Depth, Rgb};

/// The alternate screen, a hidden cursor, bracketed paste, and a clear screen; no mouse.
pub const ENTER: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[?2004h\x1b[0m\x1b[2J";
/// What [`ENTER`] and [`MOUSE_ON`] turned on, off again, and the user's own screen back.
pub const LEAVE: &[u8] = b"\x1b[?1006l\x1b[?1002l\x1b]8;;\x1b\\\x1b[0m\x1b[?2004l\x1b[?25h\x1b[?1049l";
/// Mouse reports while a button is down (`?1002`, so a drag is seen; not `?1003`, a report for every move), in SGR
/// coordinates (`?1006`), which have no 223-column limit. Written when the session's mouse is on (Phase 5).
pub const MOUSE_ON: &[u8] = b"\x1b[?1002h\x1b[?1006h";
pub const MOUSE_OFF: &[u8] = b"\x1b[?1006l\x1b[?1002l";

/// The terminal at the other end: its size, its colors, and the frame it shows.
pub struct Screen {
    width: u16,
    height: u16,
    depth: Depth,
    shown: Option<Buffer>,
}

/// The look a run of cells is written in.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Look {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    modifier: Modifier,
}

impl Screen {
    pub fn new(width: u16, height: u16, depth: Depth) -> Self {
        Self {
            width,
            height,
            depth,
            shown: None,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// A new size: the next frame is written whole, after a clear.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.shown = None;
    }

    /// Draw `view` and write what changed since the last frame, as one write.
    pub fn draw<V: View>(&mut self, view: &V, out: &mut impl Write) -> io::Result<()> {
        let mut buffer = Buffer::new(self.width, self.height);
        {
            let mut ctx = RenderContext::new(&mut buffer, Rect::new(0, 0, self.width, self.height));
            view.render(&mut ctx);
        }
        let mut bytes = Vec::new();
        match &self.shown {
            Some(shown) => self.encode(&buffer, &diff(shown, &buffer, &[]), &mut bytes),
            None => {
                bytes.extend_from_slice(b"\x1b[0m\x1b[2J");
                let all: Vec<Change> = (0..self.height)
                    .flat_map(|y| (0..self.width).map(move |x| (x, y)))
                    .filter_map(|(x, y)| buffer.get(x, y).map(|cell| Change { x, y, cell: *cell }))
                    .collect();
                self.encode(&buffer, &all, &mut bytes);
            }
        }
        self.shown = Some(buffer);
        if bytes.is_empty() {
            return Ok(());
        }
        out.write_all(&bytes)?;
        out.flush()
    }

    fn encode(&self, buffer: &Buffer, changes: &[Change], out: &mut Vec<u8>) {
        if changes.is_empty() {
            return;
        }
        // Where the terminal's cursor is, when known; None after the last column, where terminals differ.
        let mut cursor: Option<(u16, u16)> = None;
        let mut look: Option<Look> = None;
        let mut link: Option<&str> = None;
        for change in changes {
            let cell = &change.cell;
            if cell.is_continuation() {
                continue;
            }
            let url = cell.hyperlink_id.and_then(|id| buffer.get_hyperlink(id));
            if cursor != Some((change.x, change.y)) {
                if link.is_some() {
                    out.extend_from_slice(b"\x1b]8;;\x1b\\");
                    link = None;
                }
                let _ = write!(out, "\x1b[{};{}H", change.y + 1, change.x + 1);
            }
            let next = self.look(cell);
            if look != Some(next) {
                self.write_look(next, out);
                look = Some(next);
            }
            if url != link {
                if link.is_some() {
                    out.extend_from_slice(b"\x1b]8;;\x1b\\");
                }
                if let Some(url) = url {
                    let _ = write!(out, "\x1b]8;;{url}\x1b\\");
                }
                link = url;
            }
            // The painter writes only drawable characters (D9); a control character here would be Revue's own.
            let symbol = if cell.symbol.is_control() { ' ' } else { cell.symbol };
            let mut utf8 = [0u8; 4];
            out.extend_from_slice(symbol.encode_utf8(&mut utf8).as_bytes());
            let x = change
                .x
                .saturating_add(u16::try_from(char_width(symbol).max(1)).unwrap_or(1));
            cursor = (x < self.width).then_some((x, change.y));
        }
        if link.is_some() {
            out.extend_from_slice(b"\x1b]8;;\x1b\\");
        }
        out.extend_from_slice(b"\x1b[0m");
    }

    fn look(&self, cell: &Cell) -> Look {
        let rgb = |color: Option<Color>| {
            color
                .filter(|color| color.a != 0 && self.depth != Depth::Mono)
                .map(|color| Rgb(color.r, color.g, color.b))
        };
        Look {
            fg: rgb(cell.fg),
            bg: rgb(cell.bg),
            modifier: cell.modifier,
        }
    }

    fn write_look(&self, look: Look, out: &mut Vec<u8>) {
        let mut params = vec!["0".to_string()];
        for (flag, code) in [
            (Modifier::BOLD, "1"),
            (Modifier::DIM, "2"),
            (Modifier::ITALIC, "3"),
            (Modifier::UNDERLINE, "4"),
            (Modifier::REVERSE, "7"),
            (Modifier::CROSSED_OUT, "9"),
        ] {
            if look.modifier.contains(flag) {
                params.push(code.to_string());
            }
        }
        params.extend(look.fg.and_then(|rgb| sgr_color(self.depth, rgb, false)));
        params.extend(look.bg.and_then(|rgb| sgr_color(self.depth, rgb, true)));
        let _ = write!(out, "\x1b[{}m", params.join(";"));
    }
}

/// `text` for the terminal's clipboard: OSC 52 to the clipboard selection (`c`), base64, ended with ST. The terminal
/// decides whether it takes it (kitty, WezTerm, foot, Windows Terminal and xterm with its window ops do; VTE terminals
/// do not).
pub fn osc52(text: &str) -> Vec<u8> {
    let mut out = b"\x1b]52;c;".to_vec();
    out.extend(base64(text.as_bytes()));
    out.extend(b"\x1b\\");
    out
}

/// Standard base64 with padding, for [`osc52`].
fn base64(bytes: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (at, byte)| acc | (u32::from(*byte) << (16 - 8 * at)));
        for at in 0..4 {
            if at <= chunk.len() {
                out.push(ALPHABET[((triple >> (18 - 6 * at)) & 63) as usize]);
            } else {
                out.push(b'=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_is_base64_with_padding() {
        assert_eq!(osc52("Man"), b"\x1b]52;c;TWFu\x1b\\".to_vec());
        assert_eq!(osc52("Ma"), b"\x1b]52;c;TWE=\x1b\\".to_vec());
        assert_eq!(osc52("M"), b"\x1b]52;c;TQ==\x1b\\".to_vec());
        assert_eq!(osc52("é\n"), b"\x1b]52;c;w6kK\x1b\\".to_vec());
    }
}
