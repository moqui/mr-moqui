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

//! Byte sequences from a pty into keys, paste, and a window report.
//!
//! crossterm 0.29 reads the process terminal from `EventReader` and does not export a
//! parser for an arbitrary byte stream, so this decoder is the one the spike keeps.
//! A lone ESC stays buffered until the next byte, because it may be the start of a sequence; [`Decoder::deadline`] says
//! when it has waited long enough, and [`Decoder::flush`] then gives it as Escape.
//! Resize on a real SSH session arrives as a window-change, not as bytes. The `8;rows;cols t`
//! report is accepted so a test can feed a size without a pty.

use std::time::{Duration, Instant};

use crate::keys::{Key, KeyPress, Mouse, MouseKind};

/// How long an unfinished sequence waits for its next byte before it is taken as typed: a lone ESC is the Escape key.
/// Terminals send a sequence in one write, so a gap this long means a person pressed Escape.
pub const ESC_WAIT: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    Key(KeyPress),
    /// Columns, then rows. The xterm report is `8 ; rows ; cols t`.
    Resize {
        columns: u16,
        rows: u16,
    },
    Paste(String),
    /// The terminal's answer to the OSC 11 query: whether its background is dark.
    Background {
        dark: bool,
    },
    /// A mouse report: SGR (`ESC [ < b ; x ; y M` or `m`), or the X10 encoding (`ESC [ M` and three bytes).
    Mouse(Mouse),
}

#[derive(Debug)]
pub struct Decoder {
    pending: Vec<u8>,
    paste: Option<Vec<u8>>,
    /// When the bytes still pending stopped short of a whole sequence.
    waiting: Option<Instant>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            paste: None,
            waiting: None,
        }
    }

    /// When [`Self::flush`] should run: an unfinished sequence has waited [`ESC_WAIT`]. None inside a paste, which
    /// waits for its end however long it takes.
    pub fn deadline(&self) -> Option<Instant> {
        self.waiting.map(|since| since + ESC_WAIT)
    }

    /// Take what is pending as typed: an ESC is the Escape key, and the bytes after it are read again.
    pub fn flush(&mut self) -> Vec<Decoded> {
        self.waiting = None;
        if self.paste.is_some() || self.pending.first() != Some(&b'\x1b') {
            return Vec::new();
        }
        let rest = self.pending.split_off(1);
        self.pending.clear();
        let mut out = vec![key(Key::Escape)];
        out.extend(self.push(&rest));
        out
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<Decoded> {
        if let Some(paste) = &mut self.paste {
            paste.extend_from_slice(bytes);
        } else {
            self.pending.extend_from_slice(bytes);
        }
        let mut out = Vec::new();
        let paste_end: &[u8] = b"\x1b[201~";
        loop {
            if let Some(buf) = &self.paste {
                let Some(at) = find_sub(buf, paste_end) else {
                    break;
                };
                let text = String::from_utf8_lossy(&buf[..at]).into_owned();
                let rest = buf[at + paste_end.len()..].to_vec();
                self.paste = None;
                self.pending = rest;
                out.push(Decoded::Paste(text));
                continue;
            }
            match take_one(&self.pending) {
                Take::NeedMore => break,
                Take::Skip(len) | Take::PasteStart(len) => {
                    let start = matches!(take_one(&self.pending), Take::PasteStart(_));
                    self.pending.drain(..len);
                    if start {
                        self.paste = Some(std::mem::take(&mut self.pending));
                    }
                }
                Take::One(decoded, len) => {
                    self.pending.drain(..len);
                    out.push(decoded);
                }
            }
        }
        self.waiting = (self.paste.is_none() && !self.pending.is_empty()).then(Instant::now);
        out
    }
}

enum Take {
    NeedMore,
    Skip(usize),
    One(Decoded, usize),
    PasteStart(usize),
}

fn take_one(buf: &[u8]) -> Take {
    if buf.is_empty() {
        return Take::NeedMore;
    }
    match buf[0] {
        b'\x1b' => take_esc(buf),
        b'\r' => Take::One(key(Key::Enter), 1 + usize::from(buf.get(1) == Some(&b'\n'))),
        b'\n' => Take::One(key(Key::Enter), 1),
        b'\t' => Take::One(key(Key::Tab), 1),
        0x7f | 0x08 => Take::One(key(Key::Backspace), 1),
        // Ctrl+Space, and Ctrl with a letter (the bytes of Tab, Enter and Backspace are those keys, above).
        0x00 => Take::One(press(Key::Char(' '), false, true, false), 1),
        byte @ 0x01..=0x1a => Take::One(press(Key::Char(char::from(b'a' + byte - 1)), false, true, false), 1),
        byte if byte.is_ascii_graphic() || byte == b' ' => Take::One(key(Key::Char(byte as char)), 1),
        byte if byte >= 0x80 => take_utf8(buf),
        _ => Take::Skip(1),
    }
}

fn take_esc(buf: &[u8]) -> Take {
    if buf.len() < 2 {
        return Take::NeedMore;
    }
    match buf[1] {
        b'[' => take_csi(buf),
        b'O' => take_ss3(buf),
        b']' => take_osc(buf),
        b'\x1b' => Take::One(key(Key::Escape), 1),
        b'\r' => Take::One(press(Key::Enter, true, false, false), 2),
        0x7f => Take::One(press(Key::Backspace, true, false, false), 2),
        byte if byte.is_ascii_graphic() || byte == b' ' => {
            Take::One(press(Key::Char(byte as char), true, false, false), 2)
        }
        _ => Take::One(key(Key::Escape), 1),
    }
}

/// An operating system command, `ESC ] … BEL` or `ESC ] … ESC \`: the terminal's answer to the background query
/// (`11;rgb:RRRR/GGGG/BBBB`) is read, any other is dropped. `ESC ]` with no digit after it is Alt+].
fn take_osc(buf: &[u8]) -> Take {
    match buf.get(2) {
        None => return Take::NeedMore,
        Some(byte) if !byte.is_ascii_digit() => return Take::One(press(Key::Char(']'), true, false, false), 2),
        Some(_) => {}
    }
    let end = buf.iter().enumerate().skip(2).find_map(|(at, byte)| match byte {
        0x07 => Some((at, at + 1)),
        b'\\' if buf[at - 1] == 0x1b => Some((at - 1, at + 1)),
        _ => None,
    });
    let Some((body_end, len)) = end else {
        // A runaway sequence is dropped rather than held forever.
        return if buf.len() > 512 {
            Take::Skip(buf.len())
        } else {
            Take::NeedMore
        };
    };
    let body = String::from_utf8_lossy(&buf[2..body_end]);
    match body.strip_prefix("11;").and_then(background_dark) {
        Some(dark) => Take::One(Decoded::Background { dark }, len),
        None => Take::Skip(len),
    }
}

/// `rgb:RRRR/GGGG/BBBB` (1 to 4 hex digits each): dark when its luminance is under half.
fn background_dark(color: &str) -> Option<bool> {
    let parts: Vec<f64> = color
        .strip_prefix("rgb:")?
        .split('/')
        .map(|part| {
            let digits = part.get(..part.len().min(4))?;
            let value = u32::from_str_radix(digits, 16).ok()?;
            let max = (1_u32 << (4 * digits.len())) - 1;
            Some(f64::from(value) / f64::from(max))
        })
        .collect::<Option<_>>()?;
    let [r, g, b] = parts.as_slice() else {
        return None;
    };
    Some(0.2126 * r + 0.7152 * g + 0.0722 * b < 0.5)
}

fn take_ss3(buf: &[u8]) -> Take {
    if buf.len() < 3 {
        return Take::NeedMore;
    }
    let key = match buf[2] {
        b'P' => Key::F(1),
        b'Q' => Key::F(2),
        b'R' => Key::F(3),
        b'S' => Key::F(4),
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        _ => return Take::Skip(3),
    };
    Take::One(Decoded::Key(KeyPress::plain(key)), 3)
}

fn take_csi(buf: &[u8]) -> Take {
    // Linux console F1–F5 is ESC [ [ A–E. '[' is in the CSI final range, so this
    // is matched before the generic final-byte scan, which would stop on it.
    if buf.len() >= 3 && buf[2] == b'[' {
        if buf.len() < 4 {
            return Take::NeedMore;
        }
        let key = match buf[3] {
            b'A' => Key::F(1),
            b'B' => Key::F(2),
            b'C' => Key::F(3),
            b'D' => Key::F(4),
            b'E' => Key::F(5),
            _ => return Take::Skip(4),
        };
        return Take::One(Decoded::Key(KeyPress::plain(key)), 4);
    }
    // X10 mouse: ESC [ M and three bytes of 32 plus the button, column and row. 'M' is a final byte, so this comes
    // before the scan as well.
    if buf.len() >= 3 && buf[2] == b'M' {
        if buf.len() < 6 {
            return Take::NeedMore;
        }
        let code = u16::from(buf[3].saturating_sub(32));
        let column = u16::from(buf[4].saturating_sub(33));
        let row = u16::from(buf[5].saturating_sub(33));
        // X10 has no release of a button of its own: code 3 is "a button went up".
        return match mouse_of(code, column, row, code & 3 == 3 && code & 64 == 0) {
            Some(mouse) => Take::One(Decoded::Mouse(mouse), 6),
            None => Take::Skip(6),
        };
    }
    let Some(final_at) = buf.iter().skip(2).position(|byte| (0x40..=0x7e).contains(byte)) else {
        return if buf.len() > 64 { Take::Skip(1) } else { Take::NeedMore };
    };
    let final_at = final_at + 2;
    let final_byte = buf[final_at];
    let body = &buf[2..final_at];
    let len = final_at + 1;
    let body = std::str::from_utf8(body).unwrap_or("");
    if let Some(report) = body.strip_prefix('<').filter(|_| matches!(final_byte, b'M' | b'm')) {
        let mut parts = report.split(';').map(|part| part.parse::<u16>().ok());
        let (Some(Some(code)), Some(Some(column)), Some(Some(row))) = (parts.next(), parts.next(), parts.next()) else {
            return Take::Skip(len);
        };
        return match mouse_of(
            code,
            column.saturating_sub(1),
            row.saturating_sub(1),
            final_byte == b'm',
        ) {
            Some(mouse) => Take::One(Decoded::Mouse(mouse), len),
            None => Take::Skip(len),
        };
    }
    if final_byte == b'~' {
        return take_tilde(body, len);
    }
    if final_byte == b'u' {
        return take_kitty(body, len);
    }
    if final_byte == b't' {
        let mut parts = body.split(';');
        if parts.next() == Some("8") {
            let rows = parts.next().and_then(|text| text.parse().ok()).unwrap_or(0);
            let columns = parts.next().and_then(|text| text.parse().ok()).unwrap_or(0);
            return Take::One(Decoded::Resize { columns, rows }, len);
        }
        return Take::Skip(len);
    }
    let (mods, _) = mods_and_first(body);
    let key = match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'Z' => Key::BackTab,
        _ => return Take::Skip(len),
    };
    Take::One(Decoded::Key(apply_mods(key, mods)), len)
}

/// A report's button code: the button in bits 0 and 1, Shift 4, Alt 8, Ctrl 16, motion 32, the wheel 64. Wheel
/// left and right, and motion with no button, are left out.
fn mouse_of(code: u16, column: u16, row: u16, release: bool) -> Option<Mouse> {
    let button = u8::try_from(code & 3).unwrap_or(0);
    let kind = if code & 64 != 0 {
        match button {
            0 => MouseKind::WheelUp,
            1 => MouseKind::WheelDown,
            _ => return None,
        }
    } else if release {
        MouseKind::Release
    } else if code & 32 != 0 {
        if button == 3 {
            return None;
        }
        MouseKind::Drag
    } else {
        MouseKind::Press
    };
    Some(Mouse {
        kind,
        // A wheel has no button, and X10's release does not say which went up.
        button: if matches!(kind, MouseKind::WheelUp | MouseKind::WheelDown) || button == 3 {
            0
        } else {
            button
        },
        column,
        row,
        shift: code & 4 != 0,
        alt: code & 8 != 0,
        ctrl: code & 16 != 0,
    })
}

fn take_tilde(body: &str, len: usize) -> Take {
    let (mods, code) = mods_and_first(body);
    if code == "200" {
        return Take::PasteStart(len);
    }
    if code == "201" {
        return Take::Skip(len);
    }
    let key = match code {
        "1" | "7" => Key::Home,
        "4" | "8" => Key::End,
        "3" => Key::Delete,
        "5" => Key::PageUp,
        "6" => Key::PageDown,
        "15" => Key::F(5),
        "17" => Key::F(6),
        "18" => Key::F(7),
        "19" => Key::F(8),
        "20" => Key::F(9),
        "21" => Key::F(10),
        "23" => Key::F(11),
        "24" => Key::F(12),
        _ => return Take::Skip(len),
    };
    Take::One(Decoded::Key(apply_mods(key, mods)), len)
}

fn take_kitty(body: &str, len: usize) -> Take {
    let mut parts = body.split(';');
    let code: u32 = parts.next().unwrap_or("").parse().unwrap_or(0);
    let mods = xterm_mods(parts.next().unwrap_or("1"));
    let key = match code {
        13 => Key::Enter,
        27 => Key::Escape,
        9 => Key::Tab,
        127 => Key::Backspace,
        32..=126 => Key::Char(char::from_u32(code).unwrap_or('\u{FFFD}')),
        _ => return Take::Skip(len),
    };
    Take::One(Decoded::Key(apply_mods(key, mods)), len)
}

fn mods_and_first(body: &str) -> (u8, &str) {
    let mut parts = body.split(';').filter(|part| !part.is_empty());
    let first = parts.next().unwrap_or("");
    let second = parts.next();
    if let Some(mods) = second {
        (xterm_mods(mods), first)
    } else {
        (0, first)
    }
}

/// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
fn xterm_mods(text: &str) -> u8 {
    let value: u8 = text.parse().unwrap_or(1);
    value.saturating_sub(1)
}

fn apply_mods(key: Key, mods: u8) -> KeyPress {
    KeyPress {
        key,
        shift: mods & 1 != 0,
        alt: mods & 2 != 0,
        ctrl: mods & 4 != 0,
    }
}

fn take_utf8(buf: &[u8]) -> Take {
    let width = utf8_width(buf[0]);
    if buf.len() < width {
        return Take::NeedMore;
    }
    let text = std::str::from_utf8(&buf[..width]).unwrap_or("\u{FFFD}");
    let ch = text.chars().next().unwrap_or('\u{FFFD}');
    Take::One(key(Key::Char(ch)), width)
}

fn utf8_width(byte: u8) -> usize {
    if byte & 0b1110_0000 == 0b1100_0000 {
        2
    } else if byte & 0b1111_0000 == 0b1110_0000 {
        3
    } else if byte & 0b1111_1000 == 0b1111_0000 {
        4
    } else {
        1
    }
}

fn key(key: Key) -> Decoded {
    Decoded::Key(KeyPress::plain(key))
}

fn press(key: Key, alt: bool, ctrl: bool, shift: bool) -> Decoded {
    Decoded::Key(KeyPress { key, alt, ctrl, shift })
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> Vec<Decoded> {
        Decoder::new().push(bytes)
    }

    #[test]
    fn xterm_and_gnome_function_keys() {
        assert_eq!(keys(b"\x1bOP"), vec![key(Key::F(1))]);
        assert_eq!(keys(b"\x1bOS"), vec![key(Key::F(4))]);
        assert_eq!(keys(b"\x1b[15~"), vec![key(Key::F(5))]);
        assert_eq!(keys(b"\x1b[18~"), vec![key(Key::F(7))]);
        assert_eq!(keys(b"\x1b[24~"), vec![key(Key::F(12))]);
        assert_eq!(keys(b"\x1b[A"), vec![key(Key::Up)]);
    }

    #[test]
    fn kitty_unicode_keys_and_alt() {
        assert_eq!(keys(b"\x1b[13u"), vec![key(Key::Enter)]);
        assert_eq!(keys(b"\x1b[97;3u"), vec![press(Key::Char('a'), true, false, false)]);
    }

    #[test]
    fn linux_console_function_key() {
        assert_eq!(keys(b"\x1b[[A"), vec![key(Key::F(1))]);
    }

    #[test]
    fn tmux_and_xterm_alt_is_an_escape_prefix() {
        assert_eq!(keys(b"\x1ba"), vec![press(Key::Char('a'), true, false, false)]);
        assert_eq!(keys(b"\x1b[1;3A"), vec![press(Key::Up, true, false, false)]);
    }

    #[test]
    fn paste_keeps_a_newline_as_one_event() {
        let decoded = keys(b"\x1b[200~hello\nworld\x1b[201~");
        assert_eq!(decoded, vec![Decoded::Paste("hello\nworld".to_string())]);
    }

    #[test]
    fn a_sequence_split_across_reads_still_parses() {
        let mut decoder = Decoder::new();
        assert!(decoder.push(b"\x1b[1").is_empty());
        assert_eq!(decoder.push(b"5~"), vec![key(Key::F(5))]);
    }

    #[test]
    fn a_lone_escape_waits_then_is_the_escape_key() {
        let mut decoder = Decoder::new();
        assert!(decoder.push(b"\x1b").is_empty());
        assert!(decoder.deadline().is_some());
        assert_eq!(decoder.flush(), vec![key(Key::Escape)]);
        assert_eq!(decoder.deadline(), None);
        // An ESC and `[` in two reads, before the wait ends, are still one sequence.
        assert!(decoder.push(b"\x1b").is_empty());
        assert!(decoder.push(b"[").is_empty());
        assert_eq!(decoder.push(b"B"), vec![key(Key::Down)]);
        assert_eq!(decoder.deadline(), None);
        // A paste waits for its end without a deadline.
        assert!(decoder.push(b"\x1b[200~part").is_empty());
        assert_eq!(decoder.deadline(), None);
    }

    #[test]
    fn control_letters_alt_enter_and_home_end() {
        assert_eq!(keys(b"\x0b"), vec![press(Key::Char('k'), false, true, false)]);
        assert_eq!(keys(b"\x13"), vec![press(Key::Char('s'), false, true, false)]);
        assert_eq!(keys(b"\t"), vec![key(Key::Tab)]);
        assert_eq!(keys(b"\x1b\r"), vec![press(Key::Enter, true, false, false)]);
        assert_eq!(keys(b"\x1b[1~\x1b[4~"), vec![key(Key::Home), key(Key::End)]);
        assert_eq!(keys(b"\x1bOH"), vec![key(Key::Home)]);
    }

    #[test]
    fn resize_report_is_columns_then_rows() {
        assert_eq!(
            keys(b"\x1b[8;50;180t"),
            vec![Decoded::Resize { columns: 180, rows: 50 }]
        );
    }

    #[test]
    fn the_background_answer_is_read_and_other_commands_dropped() {
        assert_eq!(
            keys(b"\x1b]11;rgb:0000/0000/0000\x07"),
            vec![Decoded::Background { dark: true }]
        );
        assert_eq!(
            keys(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\x"),
            vec![Decoded::Background { dark: false }, key(Key::Char('x'))]
        );
        let mut decoder = Decoder::new();
        assert!(decoder.push(b"\x1b]11;rgb:fa").is_empty(), "a split answer waits");
        assert_eq!(
            decoder.push(b"fa/fafa/fafa\x07"),
            vec![Decoded::Background { dark: false }]
        );
        assert_eq!(keys(b"\x1b]52;c;aGk=\x07z"), vec![key(Key::Char('z'))]);
        assert_eq!(
            keys(b"\x1b]a"),
            vec![press(Key::Char(']'), true, false, false), key(Key::Char('a'))]
        );
    }

    fn mouse(kind: MouseKind, column: u16, row: u16) -> Decoded {
        Decoded::Mouse(Mouse::at(kind, column, row))
    }

    #[test]
    fn sgr_mouse_press_drag_release_and_wheel() {
        assert_eq!(keys(b"\x1b[<0;10;5M"), vec![mouse(MouseKind::Press, 9, 4)]);
        assert_eq!(keys(b"\x1b[<32;11;5M"), vec![mouse(MouseKind::Drag, 10, 4)]);
        assert_eq!(keys(b"\x1b[<0;11;5m"), vec![mouse(MouseKind::Release, 10, 4)]);
        assert_eq!(keys(b"\x1b[<64;1;1M"), vec![mouse(MouseKind::WheelUp, 0, 0)]);
        assert_eq!(keys(b"\x1b[<65;1;1M"), vec![mouse(MouseKind::WheelDown, 0, 0)]);
        // Shift 4, Alt 8, Ctrl 16; the right button is 2.
        let Decoded::Mouse(with) = keys(b"\x1b[<30;3;4M").remove(0) else {
            panic!("a mouse report");
        };
        assert!(with.shift && with.alt && with.ctrl && with.button == 2, "{with:?}");
        // Motion with no button is not reported, and the keys after it still are.
        assert_eq!(keys(b"\x1b[<35;3;4Mx"), vec![key(Key::Char('x'))]);
        // SGR has no coordinate limit, unlike X10's 223.
        assert_eq!(keys(b"\x1b[<0;300;80M"), vec![mouse(MouseKind::Press, 299, 79)]);
    }

    #[test]
    fn x10_mouse_and_reports_split_across_reads() {
        // Button 0 at column 1, row 1: 32, 33, 33. A release is code 3.
        assert_eq!(keys(b"\x1b[M !!"), vec![mouse(MouseKind::Press, 0, 0)]);
        assert_eq!(keys(b"\x1b[M#*%"), vec![mouse(MouseKind::Release, 9, 4)]);
        let mut decoder = Decoder::new();
        assert!(decoder.push(b"\x1b[M ").is_empty());
        assert_eq!(decoder.push(b"!!"), vec![mouse(MouseKind::Press, 0, 0)]);
        assert!(decoder.push(b"\x1b[<0;1").is_empty());
        assert_eq!(decoder.push(b"0;5M"), vec![mouse(MouseKind::Press, 9, 4)]);
    }
}
