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

//! The `moqui>` prompt's line editor (plans/Terminal-Fancy.md §6), since a person now lives there: Left, Right, Home
//! and End; Ctrl+A, E, K, U and W; Delete and Backspace; history on Up and Down; Tab asks the caller to complete; a
//! bracketed paste is taken whole, its lines ending lines as typed ones do. It reads the terminal's bytes as they come,
//! a sequence split across reads included, and writes what the terminal shows: the line drawn again after each change.
//! With no echo (an SSH exec with no pty) it only gathers lines.

/// What a read of input asks of the caller, beside what the editor wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    /// A line, ended.
    Line(String),
    /// A line longer than the editor keeps, dropped.
    TooLong,
    /// Ctrl+C: the line was dropped.
    Cancel,
    /// Ctrl+D on an empty line.
    End,
    /// Tab: complete the word before the cursor ([`LineEditor::before_cursor`], then [`LineEditor::insert`]).
    Complete,
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

pub(crate) struct LineEditor {
    prompt: String,
    echo: bool,
    max: usize,
    line: Vec<char>,
    cursor: usize,
    overflow: bool,
    history: Vec<String>,
    history_max: usize,
    /// The history entry shown, while Up and Down move through it, and the line that was being typed.
    recall: Option<usize>,
    draft: Vec<char>,
    /// The bytes of an escape sequence or a UTF-8 character not yet whole.
    pending: Vec<u8>,
    /// Inside a bracketed paste.
    pasting: bool,
    /// The last byte ended a line with CR, so a LF right after it is the same line end.
    after_cr: bool,
}

impl LineEditor {
    pub fn new(prompt: &str, echo: bool, max: usize, history: Vec<String>, history_max: usize) -> Self {
        Self {
            prompt: prompt.to_string(),
            echo,
            max,
            line: Vec::new(),
            cursor: 0,
            overflow: false,
            history,
            history_max,
            recall: None,
            draft: Vec::new(),
            pending: Vec::new(),
            pasting: false,
            after_cr: false,
        }
    }

    pub fn set_prompt(&mut self, prompt: &str) {
        self.prompt = prompt.to_string();
    }

    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// The line up to the cursor, for completion.
    pub fn before_cursor(&self) -> String {
        self.line[..self.cursor].iter().collect()
    }

    /// Put `text` in at the cursor, as typed.
    pub fn insert(&mut self, text: &str, out: &mut Vec<u8>) {
        for ch in text.chars() {
            self.insert_char(ch);
        }
        self.redraw(out);
    }

    /// The prompt and the line, drawn again with the cursor in its place.
    pub fn redraw(&self, out: &mut Vec<u8>) {
        if !self.echo {
            return;
        }
        out.extend_from_slice(b"\r");
        out.extend_from_slice(self.prompt.as_bytes());
        let text: String = self.line.iter().collect();
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\x1b[K");
        let after = self.line.len() - self.cursor;
        if after > 0 {
            out.extend_from_slice(format!("\x1b[{after}D").as_bytes());
        }
    }

    /// Clear the line, as after a line ended or was dropped.
    pub fn clear(&mut self) {
        self.line.clear();
        self.cursor = 0;
        self.overflow = false;
        self.recall = None;
    }

    fn insert_char(&mut self, ch: char) {
        if self.line.len() >= self.max {
            self.overflow = true;
            return;
        }
        self.line.insert(self.cursor, ch);
        self.cursor += 1;
    }

    /// The line ended: kept in the history unless it is empty or the same as the last.
    fn end_line(&mut self, edits: &mut Vec<Edit>, out: &mut Vec<u8>) {
        if self.echo {
            out.extend_from_slice(b"\r\n");
        }
        if std::mem::take(&mut self.overflow) {
            self.clear();
            edits.push(Edit::TooLong);
            return;
        }
        let text: String = self.line.iter().collect();
        if !text.trim().is_empty() && self.history.last() != Some(&text) && text.len() <= 4096 {
            self.history.push(text.clone());
            let over = self.history.len().saturating_sub(self.history_max);
            self.history.drain(..over);
        }
        self.clear();
        edits.push(Edit::Line(text));
    }

    fn recall_step(&mut self, older: bool) {
        if self.history.is_empty() {
            return;
        }
        let next = match (self.recall, older) {
            (None, true) => {
                self.draft = self.line.clone();
                Some(self.history.len() - 1)
            }
            (None, false) => return,
            (Some(at), true) => Some(at.saturating_sub(1)),
            (Some(at), false) if at + 1 < self.history.len() => Some(at + 1),
            (Some(_), false) => None,
        };
        self.recall = next;
        self.line = match next {
            Some(at) => self.history[at].chars().collect(),
            None => std::mem::take(&mut self.draft),
        };
        self.cursor = self.line.len();
    }

    /// Read `bytes` of input; write to `out` what the terminal shows, and answer what the caller must do.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) -> Vec<Edit> {
        let mut edits = Vec::new();
        for &byte in bytes {
            if !self.pending.is_empty() || byte == 0x1b {
                self.pending.push(byte);
                self.escape(&mut edits, out);
                continue;
            }
            if self.pasting {
                self.paste_byte(byte, &mut edits, out);
                continue;
            }
            let after_cr = std::mem::take(&mut self.after_cr);
            match byte {
                b'\r' => {
                    self.after_cr = true;
                    self.end_line(&mut edits, out);
                }
                b'\n' if after_cr => {}
                b'\n' => self.end_line(&mut edits, out),
                // Ctrl+A, Home
                0x01 => {
                    self.cursor = 0;
                    self.redraw(out);
                }
                // Ctrl+B, Left
                0x02 => {
                    self.cursor = self.cursor.saturating_sub(1);
                    self.redraw(out);
                }
                0x03 => {
                    if self.echo {
                        out.extend_from_slice(b"^C\r\n");
                    }
                    self.clear();
                    edits.push(Edit::Cancel);
                }
                0x04 => {
                    if self.line.is_empty() {
                        edits.push(Edit::End);
                    } else if self.cursor < self.line.len() {
                        self.line.remove(self.cursor);
                        self.redraw(out);
                    }
                }
                // Ctrl+E, End
                0x05 => {
                    self.cursor = self.line.len();
                    self.redraw(out);
                }
                // Ctrl+F, Right
                0x06 => {
                    self.cursor = (self.cursor + 1).min(self.line.len());
                    self.redraw(out);
                }
                b'\t' => edits.push(Edit::Complete),
                // Ctrl+K: to the end of the line
                0x0b => {
                    self.line.truncate(self.cursor);
                    self.redraw(out);
                }
                // Ctrl+L: clear the screen
                0x0c => {
                    if self.echo {
                        out.extend_from_slice(b"\x1b[H\x1b[2J");
                    }
                    self.redraw(out);
                }
                // Ctrl+N, Ctrl+P: the history
                0x0e => {
                    self.recall_step(false);
                    self.redraw(out);
                }
                0x10 => {
                    self.recall_step(true);
                    self.redraw(out);
                }
                // Ctrl+U: to the start of the line
                0x15 => {
                    self.line.drain(..self.cursor);
                    self.cursor = 0;
                    self.redraw(out);
                }
                // Ctrl+W: the word before the cursor
                0x17 => {
                    let mut start = self.cursor;
                    while start > 0 && self.line[start - 1] == ' ' {
                        start -= 1;
                    }
                    while start > 0 && self.line[start - 1] != ' ' {
                        start -= 1;
                    }
                    self.line.drain(start..self.cursor);
                    self.cursor = start;
                    self.redraw(out);
                }
                0x7f | 0x08 => {
                    if self.cursor > 0 {
                        self.cursor -= 1;
                        self.line.remove(self.cursor);
                        self.redraw(out);
                    }
                }
                0x00..=0x1f => {}
                _ => self.typed(byte, out),
            }
        }
        edits
    }

    /// A byte of typed text: a character once its UTF-8 bytes are whole.
    fn typed(&mut self, byte: u8, out: &mut Vec<u8>) {
        if byte < 0x80 {
            self.insert_char(char::from(byte));
            self.redraw(out);
        } else {
            self.pending.push(byte);
            self.utf8(out);
        }
    }

    fn utf8(&mut self, out: &mut Vec<u8>) {
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                let text = text.to_string();
                self.pending.clear();
                for ch in text.chars() {
                    self.insert_char(ch);
                }
                self.redraw(out);
            }
            Err(err) if err.error_len().is_none() && self.pending.len() < 4 => {}
            Err(_) => self.pending.clear(),
        }
    }

    /// The pending bytes of an escape sequence (or UTF-8 character): acted on once whole.
    fn escape(&mut self, edits: &mut Vec<Edit>, out: &mut Vec<u8>) {
        let pending = self.pending.clone();
        if pending.first() != Some(&0x1b) {
            self.utf8(out);
            return;
        }
        if self.pasting {
            if PASTE_END.starts_with(&pending) {
                if pending == PASTE_END {
                    self.pending.clear();
                    self.pasting = false;
                    self.redraw(out);
                }
                return;
            }
            // not the paste's end: its bytes are text
            self.pending.clear();
            for byte in pending {
                self.paste_byte(byte, edits, out);
            }
            return;
        }
        let whole = match pending.as_slice() {
            [0x1b] => return,
            [0x1b, b'[' | b'O'] => return,
            [0x1b, b'[', rest @ ..] => match rest.last() {
                Some(0x40..=0x7e) => true,
                _ if rest.len() < 16 => return,
                _ => true,
            },
            [0x1b, b'O', _] => true,
            _ => true,
        };
        if !whole {
            return;
        }
        self.pending.clear();
        match pending.as_slice() {
            b"\x1b[D" | b"\x1bOD" => self.cursor = self.cursor.saturating_sub(1),
            b"\x1b[C" | b"\x1bOC" => self.cursor = (self.cursor + 1).min(self.line.len()),
            b"\x1b[A" | b"\x1bOA" => self.recall_step(true),
            b"\x1b[B" | b"\x1bOB" => self.recall_step(false),
            b"\x1b[H" | b"\x1bOH" | b"\x1b[1~" | b"\x1b[7~" => self.cursor = 0,
            b"\x1b[F" | b"\x1bOF" | b"\x1b[4~" | b"\x1b[8~" => self.cursor = self.line.len(),
            b"\x1b[3~" => {
                if self.cursor < self.line.len() {
                    self.line.remove(self.cursor);
                }
            }
            PASTE_START => {
                self.pasting = true;
                return;
            }
            // Alt+B, Alt+F: a word back, a word on
            b"\x1bb" => {
                while self.cursor > 0 && self.line[self.cursor - 1] == ' ' {
                    self.cursor -= 1;
                }
                while self.cursor > 0 && self.line[self.cursor - 1] != ' ' {
                    self.cursor -= 1;
                }
            }
            b"\x1bf" => {
                while self.cursor < self.line.len() && self.line[self.cursor] == ' ' {
                    self.cursor += 1;
                }
                while self.cursor < self.line.len() && self.line[self.cursor] != ' ' {
                    self.cursor += 1;
                }
            }
            [0x1b, rest @ ..] if rest.first().is_some_and(|byte| *byte != b'[' && *byte != b'O') => {
                // ESC then a key that is not a sequence: the key, as typed
                self.feed(rest, out);
                return;
            }
            _ => return,
        }
        self.redraw(out);
    }

    /// A byte inside a bracketed paste: text, and its line ends end lines.
    fn paste_byte(&mut self, byte: u8, edits: &mut Vec<Edit>, out: &mut Vec<u8>) {
        match byte {
            0x1b => self.pending.push(byte),
            b'\r' => {
                self.after_cr = true;
                self.end_line(edits, out);
            }
            b'\n' if std::mem::take(&mut self.after_cr) => {}
            b'\n' => self.end_line(edits, out),
            b'\t' => self.insert_char(' '),
            0x00..=0x1f => {}
            _ => {
                self.after_cr = false;
                if byte < 0x80 {
                    self.insert_char(char::from(byte));
                } else {
                    self.pending.push(byte);
                    self.utf8_quiet();
                }
            }
        }
    }

    fn utf8_quiet(&mut self) {
        match std::str::from_utf8(&self.pending) {
            Ok(text) => {
                let chars: Vec<char> = text.chars().collect();
                self.pending.clear();
                for ch in chars {
                    self.insert_char(ch);
                }
            }
            Err(err) if err.error_len().is_none() && self.pending.len() < 4 => {}
            Err(_) => self.pending.clear(),
        }
    }
}

/// What every one of `choices` starts with: what a completion adds when there are several.
pub(crate) fn common_prefix(choices: &[String]) -> String {
    let Some(first) = choices.first() else {
        return String::new();
    };
    let mut prefix: Vec<char> = first.chars().collect();
    for choice in &choices[1..] {
        let shared = prefix.iter().zip(choice.chars()).take_while(|(a, b)| **a == *b).count();
        prefix.truncate(shared);
    }
    prefix.into_iter().collect()
}

#[cfg(test)]
#[path = "line_tests.rs"]
mod tests;
