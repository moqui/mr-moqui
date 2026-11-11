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

//! The tracker (plans/Terminal-Fancy.md §5.5): a state machine over a session's output that keeps no screen. It reads
//! the DEC private modes a replay must set again, the title (OSC 0, 2), the working directory (OSC 7), the bell and
//! notifications (BEL, OSC 9, 777 and 99), shell marks (OSC 133), a program's own status (OSC 7501, Rex's program status
//! protocol) and progress (OSC 9;4), and the points where the stream is at rest: outside any escape sequence and any
//! UTF-8 character, where a replay may start.
//!
//! A scanner of our own rather than `vte`: these few sequences need a few states, and a sequence split across reads
//! is carried in the state. Nothing in it reads the factory, so the holder (§7.5) runs it too.

/// The modes in force that a replay sets before the kept bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Modes {
    /// The alternate screen: 47, 1047 or 1049.
    pub alternate: Option<u16>,
    /// Application cursor keys (1).
    pub app_cursor: bool,
    /// Mouse reports: 1000, 1002 or 1003.
    pub mouse: Option<u16>,
    /// SGR mouse coordinates (1006).
    pub mouse_sgr: bool,
    /// Bracketed paste (2004).
    pub bracketed_paste: bool,
    /// The cursor hidden (25 reset).
    pub cursor_hidden: bool,
    /// The application keypad (ESC =).
    pub app_keypad: bool,
}

impl Modes {
    /// The sequences that set these modes on a terminal that was just reset.
    pub fn preamble(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(mode) = self.alternate {
            out.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
        }
        if self.app_cursor {
            out.extend_from_slice(b"\x1b[?1h");
        }
        if let Some(mode) = self.mouse {
            out.extend_from_slice(format!("\x1b[?{mode}h").as_bytes());
        }
        if self.mouse_sgr {
            out.extend_from_slice(b"\x1b[?1006h");
        }
        if self.bracketed_paste {
            out.extend_from_slice(b"\x1b[?2004h");
        }
        if self.cursor_hidden {
            out.extend_from_slice(b"\x1b[?25l");
        }
        if self.app_keypad {
            out.extend_from_slice(b"\x1b=");
        }
        out
    }
}

/// What a program said about itself in one read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub title: Option<String>,
    pub cwd: Option<String>,
    /// A BEL outside any sequence.
    pub bell: bool,
    /// A notification (OSC 9, 777 notify, 99): its title and body.
    pub notify: Option<(String, String)>,
    /// The program's status (OSC 7501, or from OSC 133): idle, working, done, blocked, error.
    pub status: Option<String>,
    /// The status is the program's own word (OSC 7501), which outranks every other source (§11.1).
    pub program: bool,
    /// Text with the status: a blocked program's kind, an error.
    pub status_text: Option<String>,
    /// Progress in percent; `Some(None)` when it was cleared.
    pub progress: Option<Option<u8>>,
    /// OSC 133 marks seen: 'A' prompt, 'B' input, 'C' command, 'D' finished.
    pub marks: Vec<char>,
    /// The exit status OSC 133;D gave.
    pub exit_status: Option<i32>,
    /// The points at rest in the read, as offsets into it, with the modes in force there.
    pub rests: Vec<(usize, Modes)>,
}

impl Changes {
    /// Anything a client is told of beside the bytes.
    pub fn any(&self) -> bool {
        self.title.is_some()
            || self.cwd.is_some()
            || self.status.is_some()
            || self.progress.is_some()
            || self.notify.is_some()
            || self.bell
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    /// ESC followed by an intermediate (ESC ( B and the like): one more byte.
    EscapeIntermediate,
    Csi,
    Osc,
    /// ESC inside an OSC: ST if `\` follows.
    OscEscape,
    /// A DCS, SOS, PM or APC string, passed over to its ST.
    String,
    StringEscape,
}

/// The longest OSC kept; past it the rest is passed over.
const OSC_MAX: usize = 4096;
/// Rest points are noted at a line end, and failing one, at most this far apart.
const REST_EVERY: u64 = 16 * 1024;

/// The tracker of one session.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    state: State,
    /// The CSI's parameters and intermediates, or the OSC's text.
    buffer: Vec<u8>,
    /// UTF-8 continuation bytes still to come.
    utf8_left: u8,
    modes: Modes,
    /// Bytes since the last rest point.
    since_rest: u64,
}

impl Tracker {
    pub fn modes(&self) -> Modes {
        self.modes
    }

    pub fn alternate_screen(&self) -> bool {
        self.modes.alternate.is_some()
    }

    /// Read `bytes`, the next of the session's output.
    pub fn feed(&mut self, bytes: &[u8]) -> Changes {
        let mut changes = Changes::default();
        for (at, &byte) in bytes.iter().enumerate() {
            self.step(byte, &mut changes);
            self.since_rest += 1;
            if self.state == State::Ground && self.utf8_left == 0 && (byte == b'\n' || self.since_rest >= REST_EVERY) {
                self.since_rest = 0;
                changes.rests.push((at + 1, self.modes));
            }
        }
        changes
    }

    fn step(&mut self, byte: u8, changes: &mut Changes) {
        match self.state {
            State::Ground => match byte {
                0x1b => {
                    self.utf8_left = 0;
                    self.state = State::Escape;
                }
                0x07 => changes.bell = true,
                0x80..=0xbf => self.utf8_left = self.utf8_left.saturating_sub(1),
                0xc0..=0xdf => self.utf8_left = 1,
                0xe0..=0xef => self.utf8_left = 2,
                0xf0..=0xf7 => self.utf8_left = 3,
                _ => self.utf8_left = 0,
            },
            State::Escape => {
                self.buffer.clear();
                self.state = match byte {
                    b'[' => State::Csi,
                    b']' => State::Osc,
                    b'P' | b'X' | b'^' | b'_' => State::String,
                    b'(' | b')' | b'*' | b'+' | b'#' | b'%' | b' ' => State::EscapeIntermediate,
                    b'=' => {
                        self.modes.app_keypad = true;
                        State::Ground
                    }
                    b'>' => {
                        self.modes.app_keypad = false;
                        State::Ground
                    }
                    // ESC c, a full reset
                    b'c' => {
                        self.modes = Modes::default();
                        State::Ground
                    }
                    0x1b => State::Escape,
                    _ => State::Ground,
                };
            }
            State::EscapeIntermediate => self.state = State::Ground,
            State::Csi => match byte {
                0x20..=0x3f => {
                    if self.buffer.len() < 64 {
                        self.buffer.push(byte);
                    }
                }
                0x40..=0x7e => {
                    self.csi(byte);
                    self.state = State::Ground;
                }
                0x1b => self.state = State::Escape,
                // a control inside a CSI is acted on by a terminal; nothing here needs it
                _ => {}
            },
            State::Osc => match byte {
                0x07 => {
                    self.osc(changes);
                    self.state = State::Ground;
                }
                0x1b => self.state = State::OscEscape,
                _ => {
                    if self.buffer.len() < OSC_MAX {
                        self.buffer.push(byte);
                    }
                }
            },
            State::OscEscape => {
                if byte == b'\\' {
                    self.osc(changes);
                    self.state = State::Ground;
                } else {
                    // not ST: the OSC was cut off, and this ESC starts another sequence
                    self.state = State::Escape;
                    self.step(byte, changes);
                }
            }
            State::String => match byte {
                0x1b => self.state = State::StringEscape,
                0x07 => self.state = State::Ground,
                _ => {}
            },
            State::StringEscape => {
                self.state = if byte == b'\\' { State::Ground } else { State::String };
            }
        }
    }

    /// A CSI ended by `last`: the DEC private modes it sets or resets.
    fn csi(&mut self, last: u8) {
        let Some(params) = self.buffer.strip_prefix(b"?") else {
            return;
        };
        let set = match last {
            b'h' => true,
            b'l' => false,
            _ => return,
        };
        let text = String::from_utf8_lossy(params).into_owned();
        for mode in text.split(';').filter_map(|part| part.parse::<u16>().ok()) {
            match mode {
                1 => self.modes.app_cursor = set,
                25 => self.modes.cursor_hidden = !set,
                47 | 1047 | 1049 => self.modes.alternate = set.then_some(mode),
                1000 | 1002 | 1003 => {
                    if set {
                        self.modes.mouse = Some(mode);
                    } else if self.modes.mouse == Some(mode) {
                        self.modes.mouse = None;
                    }
                }
                1006 => self.modes.mouse_sgr = set,
                2004 => self.modes.bracketed_paste = set,
                _ => {}
            }
        }
    }

    /// An OSC that ended.
    fn osc(&mut self, changes: &mut Changes) {
        let text = String::from_utf8_lossy(&self.buffer).into_owned();
        self.buffer.clear();
        let (number, rest) = text.split_once(';').unwrap_or((text.as_str(), ""));
        match number {
            "0" | "2" => changes.title = Some(clean(rest)),
            "7" => changes.cwd = Some(directory_of(rest)),
            "9" => {
                if let Some(progress) = rest.strip_prefix("4;") {
                    changes.progress = Some(progress_of(progress));
                } else {
                    changes.notify = Some((String::new(), clean(rest)));
                }
            }
            "777" => {
                let mut parts = rest.splitn(3, ';');
                if parts.next() == Some("notify") {
                    let title = parts.next().unwrap_or_default();
                    let body = parts.next().unwrap_or_default();
                    changes.notify = Some((clean(title), clean(body)));
                }
            }
            // kitty's notifications: OSC 99 ; metadata ; payload
            "99" => {
                let (_, payload) = rest.split_once(';').unwrap_or(("", rest));
                changes.notify = Some((String::new(), clean(payload)));
            }
            "133" => {
                let mut parts = rest.split(';');
                if let Some(mark) = parts.next().and_then(|mark| mark.chars().next()) {
                    changes.marks.push(mark);
                    match mark {
                        // a prompt after a command keeps the command's done or error on the dot
                        'A' if !matches!(changes.status.as_deref(), Some("done" | "error")) => {}
                        'C' => changes.status = Some("working".to_string()),
                        'D' => {
                            let exit = parts.next().and_then(|code| code.trim().parse::<i32>().ok());
                            changes.exit_status = exit;
                            changes.status = Some(if exit.unwrap_or(0) == 0 { "done" } else { "error" }.to_string());
                        }
                        _ => {}
                    }
                }
            }
            "7501" => self.program_status(rest, changes),
            _ => {}
        }
    }

    /// OSC 7501: `state[;key=value...]` or `key=value;...` with `state=`: idle, working, done, blocked (with its kind),
    /// error (with its text), and progress.
    fn program_status(&self, rest: &str, changes: &mut Changes) {
        for part in rest.split(';').map(str::trim).filter(|part| !part.is_empty()) {
            let (key, value) = part.split_once('=').unwrap_or(("state", part));
            match key {
                "state" | "status" => {
                    let state = value.to_ascii_lowercase();
                    if matches!(state.as_str(), "idle" | "working" | "done" | "blocked" | "error") {
                        changes.status = Some(state);
                        changes.program = true;
                    }
                }
                "kind" | "reason" | "message" | "text" => changes.status_text = Some(clean(value)),
                "progress" => changes.progress = Some(progress_of(value)),
                _ => {}
            }
        }
    }
}

/// A program's text with its controls taken out, and at most 200 characters.
fn clean(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).take(200).collect()
}

/// OSC 7's `file://host/path`, as the path, its `%xx` decoded.
fn directory_of(text: &str) -> String {
    let path = match text.strip_prefix("file://") {
        Some(rest) => rest.find('/').map_or("", |at| &rest[at..]),
        None => text,
    };
    let mut out = Vec::with_capacity(path.len());
    let mut rest = path;
    while let Some((before, after)) = rest.split_once('%') {
        out.extend_from_slice(before.as_bytes());
        match after.get(..2).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            Some(byte) => {
                out.push(byte);
                rest = &after[2..];
            }
            None => {
                out.push(b'%');
                rest = after;
            }
        }
    }
    out.extend_from_slice(rest.as_bytes());
    clean(&String::from_utf8_lossy(&out))
}

/// OSC 9;4's `state;percent`: state 0 clears it; a percent alone is taken too.
fn progress_of(text: &str) -> Option<u8> {
    let mut parts = text.split(';');
    let first = parts.next().unwrap_or_default().trim();
    let (state, percent) = match parts.next() {
        Some(percent) => (first, percent.trim()),
        None => ("1", first),
    };
    if state == "0" {
        return None;
    }
    percent.parse::<u8>().ok().map(|percent| percent.min(100))
}

#[cfg(test)]
#[path = "track_tests.rs"]
mod tests;
