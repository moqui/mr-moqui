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

//! `code`: an editor for one text with line numbers, light highlighting and the problems of a check (docs/ui/TUI.md,
//! Components). The terminal's stand-in for the browser's CodeMirror in the Dev AIDE twins.
//!
//! Props:
//! - `text`, the text; `language` (`xml`, `js`, `json`, `sql`, `md`, else plain); `readOnly`; `cursorLine` (1-based),
//!   where the cursor starts; `rows`, the lines shown (else what fits the terminal).
//! - `form` and `field`: the form field the text goes back to. Each edit sets it (as typing in a field does), so the
//!   form's own submit (Ctrl+S, Alt+Enter) posts it.
//! - `problems`, `[{line, column, severity, message}]` (1-based; severity `error`, `warning` or `info`), marked in the
//!   gutter.
//! - `check`: a transition that answers `{problems}` for the text, posted with `text` and `language` 800 ms after the
//!   typing stops; its problems take the place of the props'.
//! - `complete`: a transition for Ctrl+Space. For XML it answers a schema digest (`{digest: {elements}}`, as
//!   `get#SchemaDigest` does for its `rootElement`), which is kept, and the elements and attributes valid where the cursor
//!   is are offered from it; otherwise it answers a list of choices (`items`, `completions` or a list: a string, or
//!   `{label, insert, detail}`), asked for with `text`, `line`, `column`, `prefix` and `language`.
//! - `parameters`: an object of parameters posted with `check` and `complete` besides (a root and a path).
//!
//! The text, the cursor (a char offset, as `edit.rs` keeps it), the selection and an undo log are kept in the state's
//! `local`; the cursor moves through `edit::apply`, as in a text area.

mod complete;
mod highlight;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{prop, Act, Component, ComponentState, Line, Span};
use crate::edit::{self, Edited, Limits};
use crate::keys::{Key, KeyPress};
use crate::layout::truncate_width;
use crate::theme::Role;
use complete::Choice;
use highlight::{Carry, Language, Paint};

pub struct Code;

const INDENT: &str = "    ";
const CHECK_MILLIS: u64 = 800;
/// The terminal rows that are not the editor's lines: the app's frame, the screen's title, the status line.
const CHROME_ROWS: u16 = 10;
const MIN_ROWS: usize = 5;
/// The lines shown before the terminal is sized.
const DEFAULT_ROWS: usize = 20;
const UNDO_DEPTH: usize = 200;
const POPUP_ROWS: usize = 8;

/// A problem of the text, as the props and a check give it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Problem {
    line: usize,
    column: usize,
    severity: String,
    message: String,
}

impl Problem {
    fn is_error(&self) -> bool {
        !matches!(self.severity.as_str(), "warning" | "info")
    }
}

fn number(value: &Value) -> usize {
    value
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        .unwrap_or(0)
}

/// The problems of a list, in line and column order.
fn problems_of(value: &Value) -> Vec<Problem> {
    let mut problems: Vec<Problem> = value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|item| Problem {
            line: number(&item["line"]),
            column: number(&item["column"]),
            severity: item["severity"].as_str().unwrap_or("error").to_ascii_lowercase(),
            message: item["message"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    problems.sort_by_key(|problem| (problem.line, problem.column));
    problems
}

/// One edit, for undo: `removed` at `at` became `inserted`, with the cursor and selection before and the cursor after.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Op {
    at: usize,
    removed: String,
    inserted: String,
    before: usize,
    anchor: Option<usize>,
    after: usize,
    /// Typed characters, which the next typed one joins.
    typed: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
enum Ask {
    #[default]
    Find,
    Line,
}

/// The one-line input of Ctrl+F and Ctrl+G.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Prompt {
    ask: Ask,
    text: String,
    cursor: usize,
}

/// The completion list: its choices, the char offset of the letters they replace, and the one chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Popup {
    from: usize,
    all: Vec<Choice>,
    at: usize,
}

/// What the component keeps in its state's `local`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Editor {
    loaded: bool,
    /// The `text` prop it was loaded from: new props load again.
    base: String,
    text: String,
    cursor: usize,
    /// The other end of a selection.
    anchor: Option<usize>,
    /// The first line shown.
    top: usize,
    undo: Vec<Op>,
    redo: Vec<Op>,
    /// What Ctrl+C and Ctrl+X took, for Ctrl+V.
    clip: String,
    /// Esc gave the focus back: Tab and Shift+Tab move on.
    released: bool,
    prompt: Option<Prompt>,
    find: String,
    /// The problems of the last check, in place of the props'.
    checked: Option<Vec<Problem>>,
    check_due: bool,
    /// The schema digest of `digest_root`, for XML completion.
    digest: Value,
    digest_root: String,
    popup: Option<Popup>,
    /// Ctrl+Space asked, and the answer opens the list.
    want_popup: bool,
    message: String,
    /// The text changed during this key.
    #[serde(skip)]
    changed: bool,
}

/// The char offset `index` of `text` as a byte offset.
fn byte_of(text: &str, index: usize) -> usize {
    text.char_indices().nth(index).map_or(text.len(), |(at, _)| at)
}

fn chars_in(text: &str) -> usize {
    text.chars().count()
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The lines shown: the `rows` prop, else what fits the terminal.
fn rows_shown(props: &Value, terminal_rows: u16) -> usize {
    if let Some(rows) = props["rows"].as_u64() {
        return usize::try_from(rows).unwrap_or(MIN_ROWS).max(1);
    }
    if terminal_rows == 0 {
        return DEFAULT_ROWS;
    }
    usize::from(terminal_rows.saturating_sub(CHROME_ROWS)).max(MIN_ROWS)
}

fn read_only(props: &Value) -> bool {
    props["readOnly"] == true || props["readOnly"] == "true"
}

fn language(props: &Value) -> Language {
    Language::of(prop(props, "language", "plain"))
}

/// The first column shown when the cursor is at `col`: the lines scroll by half their width, so the view stays put
/// while the cursor moves within it.
fn scroll_left(col: usize, width: usize) -> usize {
    if col < width {
        return 0;
    }
    let half = (width / 2).max(1);
    (col + 1 - width).div_ceil(half) * half
}

impl Editor {
    /// The editor of the state, or a new one from the props.
    fn of(props: &Value, state: &ComponentState) -> Self {
        let base = prop(props, "text", "");
        if let Ok(editor) = Self::deserialize(&state.local) {
            if editor.loaded && editor.base == base {
                return editor;
            }
        }
        let text = base.replace("\r\n", "\n");
        let mut editor = Self {
            loaded: true,
            base: base.to_string(),
            text,
            ..Self::default()
        };
        let line = number(&props["cursorLine"]);
        if line > 0 {
            editor.cursor = edit::offset_of(&editor.text, line - 1, 0);
            editor.top = (line - 1).saturating_sub(rows_shown(props, state.rows) / 3);
        }
        editor
    }

    fn store(&self, state: &mut ComponentState) {
        state.local = serde_json::to_value(self).unwrap_or(Value::Null);
    }

    fn modified(&self) -> bool {
        if self.base.contains('\r') {
            self.text != self.base.replace("\r\n", "\n")
        } else {
            self.text != self.base
        }
    }

    fn len(&self) -> usize {
        chars_in(&self.text)
    }

    fn line_count(&self) -> usize {
        self.text.split('\n').count()
    }

    fn row_col(&self) -> (usize, usize) {
        edit::row_col(&self.text, self.cursor)
    }

    /// The selection, start then end, when it is not empty.
    fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    fn slice(&self, start: usize, end: usize) -> &str {
        &self.text[byte_of(&self.text, start)..byte_of(&self.text, end)]
    }

    fn problems(&self, props: &Value) -> Vec<Problem> {
        self.checked.clone().unwrap_or_else(|| problems_of(&props["problems"]))
    }

    /// The cursor to `to`, the selection kept and grown with Shift, else dropped.
    fn move_to(&mut self, to: usize, select: bool) {
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to.min(self.len());
    }

    /// Where a plain cursor key takes the cursor, through the text area's editing.
    fn step(&mut self, key: &Key) -> usize {
        let mut cursor = self.cursor;
        let limits = Limits {
            multiline: true,
            ..Limits::default()
        };
        match edit::apply(&mut self.text, &mut cursor, &KeyPress::plain(key.clone()), &limits) {
            Edited::No if *key == Key::Up => 0,
            Edited::No if *key == Key::Down => self.len(),
            _ => cursor,
        }
    }

    /// The start of the word before the cursor, or of the line's start when it is at a word's start.
    fn word_left(&self) -> usize {
        let chars: Vec<char> = self.text.chars().take(self.cursor).collect();
        let mut at = chars.len();
        while at > 0 && chars[at - 1].is_whitespace() {
            at -= 1;
        }
        if at > 0 && is_word(chars[at - 1]) {
            while at > 0 && is_word(chars[at - 1]) {
                at -= 1;
            }
        } else {
            while at > 0 && !is_word(chars[at - 1]) && !chars[at - 1].is_whitespace() {
                at -= 1;
            }
        }
        at
    }

    fn word_right(&self) -> usize {
        let chars: Vec<char> = self.text.chars().skip(self.cursor).collect();
        let mut at = 0;
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
        if at < chars.len() && is_word(chars[at]) {
            while at < chars.len() && is_word(chars[at]) {
                at += 1;
            }
        } else {
            while at < chars.len() && !is_word(chars[at]) && !chars[at].is_whitespace() {
                at += 1;
            }
        }
        self.cursor + at
    }

    /// Replace the chars `start..end` with `inserted`, for undo; the cursor goes after it and the selection is dropped.
    fn replace(&mut self, start: usize, end: usize, inserted: &str, typed: bool) {
        let (from, to) = (byte_of(&self.text, start), byte_of(&self.text, end));
        let removed = self.text[from..to].to_string();
        self.text.replace_range(from..to, inserted);
        let after = start + chars_in(inserted);
        self.redo.clear();
        let joins = typed
            && removed.is_empty()
            && !inserted.contains('\n')
            && self
                .undo
                .last()
                .is_some_and(|last| last.typed && last.at + chars_in(&last.inserted) == start);
        match self.undo.last_mut() {
            Some(last) if joins => {
                last.inserted.push_str(inserted);
                last.after = after;
            }
            _ => {
                self.undo.push(Op {
                    at: start,
                    removed,
                    inserted: inserted.to_string(),
                    before: self.cursor,
                    anchor: self.anchor,
                    after,
                    typed,
                });
                if self.undo.len() > UNDO_DEPTH {
                    self.undo.remove(0);
                }
            }
        }
        self.cursor = after;
        self.anchor = None;
        self.changed = true;
    }

    /// Put `text` in place of the selection, or at the cursor.
    fn insert(&mut self, text: &str, typed: bool) {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));
        self.replace(start, end, text, typed);
    }

    fn undo(&mut self) {
        let Some(op) = self.undo.pop() else {
            self.message = "Nothing to undo".to_string();
            return;
        };
        let (from, to) = (
            byte_of(&self.text, op.at),
            byte_of(&self.text, op.at + chars_in(&op.inserted)),
        );
        self.text.replace_range(from..to, &op.removed);
        self.cursor = op.before;
        self.anchor = op.anchor;
        self.changed = true;
        self.redo.push(op);
    }

    fn redo(&mut self) {
        let Some(op) = self.redo.pop() else {
            self.message = "Nothing to redo".to_string();
            return;
        };
        let (from, to) = (
            byte_of(&self.text, op.at),
            byte_of(&self.text, op.at + chars_in(&op.removed)),
        );
        self.text.replace_range(from..to, &op.inserted);
        self.cursor = op.after;
        self.anchor = None;
        self.changed = true;
        self.undo.push(op);
    }

    /// The text of the selection, else of the cursor's line with its newline, and its range.
    fn taken(&self) -> (usize, usize, String) {
        if let Some((start, end)) = self.selection() {
            return (start, end, self.slice(start, end).to_string());
        }
        let (row, _) = self.row_col();
        let start = edit::offset_of(&self.text, row, 0);
        let end = edit::offset_of(&self.text, row, usize::MAX);
        let end = if end < self.len() { end + 1 } else { end };
        let mut text = self.slice(start, end).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        (start, end, text)
    }

    /// Tab and Shift+Tab: the lines of the selection, or the cursor's line, in or out by one indent.
    fn indent(&mut self, out: bool) {
        let selected = self.selection();
        let (start, end) = selected.unwrap_or((self.cursor, self.cursor));
        let (first, _) = edit::row_col(&self.text, start);
        let (mut last, last_col) = edit::row_col(&self.text, end);
        if last > first && last_col == 0 {
            last -= 1;
        }
        let block_start = edit::offset_of(&self.text, first, 0);
        let block_end = edit::offset_of(&self.text, last, usize::MAX);
        let old = self.slice(block_start, block_end).to_string();
        let several = last > first;
        let lines: Vec<String> = old
            .split('\n')
            .map(|line| {
                if out {
                    let spaces = line.chars().take(INDENT.len()).take_while(|ch| *ch == ' ').count();
                    match line.strip_prefix('\t') {
                        Some(rest) if spaces == 0 => rest.to_string(),
                        _ => line.split_at_checked(spaces).map_or(line, |(_, rest)| rest).to_string(),
                    }
                } else if several && line.trim().is_empty() {
                    line.to_string()
                } else {
                    format!("{INDENT}{line}")
                }
            })
            .collect();
        let new = lines.join("\n");
        if new == old {
            self.message = "Nothing to outdent".to_string();
            return;
        }
        let (row, col) = self.row_col();
        let first_old = chars_in(old.split('\n').next().unwrap_or(""));
        let first_new = chars_in(lines.first().map_or("", String::as_str));
        self.replace(block_start, block_end, &new, false);
        if selected.is_some() {
            self.anchor = Some(block_start);
            self.cursor = block_start + chars_in(&new);
        } else {
            let col = (col + first_new).saturating_sub(first_old);
            self.cursor = edit::offset_of(&self.text, row, col);
        }
    }

    /// Enter: a new line, indented as the line it breaks.
    fn newline(&mut self) {
        let (row, col) = self.row_col();
        let start = edit::offset_of(&self.text, row, 0);
        let indent: String = self
            .slice(start, start + col)
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        self.insert(&format!("\n{indent}"), false);
    }

    /// The next match of the find text from `from` (back from it when `!forward`), selected; around the end when there
    /// is none before it.
    fn find_from(&mut self, from: usize, forward: bool) {
        let fold =
            |text: &str| -> Vec<char> { text.chars().map(|ch| ch.to_lowercase().next().unwrap_or(ch)).collect() };
        let query = fold(&self.find);
        if query.is_empty() {
            return;
        }
        let text = fold(&self.text);
        let hits = |at: &usize| text[*at..].starts_with(&query);
        let last = text.len().saturating_sub(query.len());
        let found = if forward {
            (from.min(text.len())..=last)
                .find(hits)
                .map(|at| (at, false))
                .or_else(|| (0..from.min(last + 1)).find(hits).map(|at| (at, true)))
        } else {
            (0..from.min(last + 1))
                .rev()
                .find(hits)
                .map(|at| (at, false))
                .or_else(|| (from.min(last + 1)..=last).rev().find(hits).map(|at| (at, true)))
        };
        match found {
            Some((at, wrapped)) if text.len() >= query.len() => {
                self.anchor = Some(at);
                self.cursor = at + query.len();
                if wrapped {
                    self.message = if forward {
                        "Found from the top".to_string()
                    } else {
                        "Found from the bottom".to_string()
                    };
                }
            }
            _ => self.message = format!("Not found: {}", self.find),
        }
    }

    /// F3 and Shift+F3.
    fn find_next(&mut self, forward: bool) {
        if self.find.is_empty() {
            self.open_prompt(Ask::Find);
            return;
        }
        let from = match self.selection() {
            Some((start, _)) if forward => start + 1,
            Some((start, _)) => start,
            None => self.cursor,
        };
        self.find_from(from, forward);
    }

    fn open_prompt(&mut self, ask: Ask) {
        let text = match (ask, self.selection()) {
            (Ask::Find, Some((start, end))) if !self.slice(start, end).contains('\n') => {
                self.slice(start, end).to_string()
            }
            (Ask::Find, _) => self.find.clone(),
            (Ask::Line, _) => String::new(),
        };
        self.prompt = Some(Prompt {
            ask,
            cursor: chars_in(&text),
            text,
        });
    }

    fn go_to_line(&mut self, line: usize, rows: usize) {
        let row = line.clamp(1, self.line_count()) - 1;
        let start = edit::offset_of(&self.text, row, 0);
        let end = edit::offset_of(&self.text, row, usize::MAX);
        let blank = self
            .slice(start, end)
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .count();
        self.move_to(start + blank, false);
        self.top = row.saturating_sub(rows / 3);
    }

    /// F8 and Shift+F8: the cursor to the next or previous problem, around the end.
    fn next_problem(&mut self, props: &Value, forward: bool) {
        let problems = self.problems(props);
        if problems.is_empty() {
            self.message = "No problems".to_string();
            return;
        }
        let (row, col) = self.row_col();
        let here = (row + 1, col + 1);
        let place = |problem: &Problem| (problem.line.max(1), problem.column.max(1));
        let found = if forward {
            problems
                .iter()
                .find(|problem| place(problem) > here)
                .or_else(|| problems.first())
        } else {
            problems
                .iter()
                .rev()
                .find(|problem| place(problem) < here)
                .or_else(|| problems.last())
        };
        if let Some(problem) = found {
            let (line, column) = place(problem);
            let to = edit::offset_of(&self.text, line - 1, column - 1);
            self.move_to(to, false);
        }
    }

    /// The problem the status line shows: the one at or before the cursor on its line, else the line's first.
    fn problem_here(&self, props: &Value) -> Option<Problem> {
        let (row, col) = self.row_col();
        let on_line: Vec<Problem> = self
            .problems(props)
            .into_iter()
            .filter(|problem| problem.line == row + 1)
            .collect();
        on_line
            .iter()
            .rev()
            .find(|problem| problem.column <= col + 1)
            .or_else(|| on_line.first())
            .cloned()
    }

    /// The parameters of a `check` or `complete` call: the url's own, the `parameters` prop's, then `extra`.
    fn call(props: &Value, url: &str, extra: &[(&str, String)]) -> Act {
        let location = crate::nav::Location::of_url(url);
        let mut parameters = location.parameters;
        if let Some(fixed) = props["parameters"].as_object() {
            for (name, value) in fixed {
                let value = match value {
                    Value::String(text) => text.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                parameters.insert(name.clone(), value);
            }
        }
        for (name, value) in extra {
            parameters.insert((*name).to_string(), value.clone());
        }
        Act::Call {
            url: location.path,
            parameters,
        }
    }

    /// Ctrl+Space: the list from the digest kept, else ask `complete`.
    fn complete(&mut self, props: &Value) -> Vec<Act> {
        let url = prop(props, "complete", "");
        if url.is_empty() {
            self.message = "No completion for this text".to_string();
            return Vec::new();
        }
        let language = language(props);
        let (row, col) = self.row_col();
        let from = complete::word_start(&self.text, self.cursor);
        let prefix = self.slice(from, self.cursor).to_string();
        let mut extra = vec![
            ("language", language.name().to_string()),
            ("line", (row + 1).to_string()),
            ("column", (col + 1).to_string()),
            ("prefix", prefix),
        ];
        if language == Language::Xml {
            let root = complete::root_element(&self.text);
            if self.digest.is_object() && self.digest_root == root {
                let (from, choices) = complete::xml(&self.digest, &self.text, self.cursor);
                self.open_popup(from, choices);
                return Vec::new();
            }
            extra.push(("rootElement", root));
        } else {
            extra.push(("text", self.text.clone()));
        }
        self.want_popup = true;
        vec![Self::call(props, url, &extra)]
    }

    fn open_popup(&mut self, from: usize, all: Vec<Choice>) {
        let popup = Popup { from, all, at: 0 };
        if self.shown(&popup).is_empty() {
            self.message = "No completions here".to_string();
            self.popup = None;
        } else {
            self.popup = Some(popup);
        }
    }

    /// The choices that begin with what is typed since the list opened.
    fn shown<'a>(&self, popup: &'a Popup) -> Vec<&'a Choice> {
        let typed = if self.cursor >= popup.from {
            self.slice(popup.from, self.cursor).to_lowercase()
        } else {
            String::new()
        };
        popup
            .all
            .iter()
            .filter(|choice| choice.label.to_lowercase().starts_with(&typed))
            .collect()
    }

    /// The list's own keys: None when the key goes on to the editor (typing goes on into the text and narrows it; any
    /// other key closes it first).
    fn popup_key(&mut self, press: &KeyPress) -> Option<Vec<Act>> {
        let popup = self.popup.clone()?;
        let count = self.shown(&popup).len().max(1);
        if press.ctrl || press.alt {
            self.popup = None;
            return None;
        }
        match press.key {
            Key::Up => {
                self.popup = Some(Popup {
                    at: (popup.at + count - 1) % count,
                    ..popup
                });
                Some(Vec::new())
            }
            Key::Down => {
                self.popup = Some(Popup {
                    at: (popup.at + 1) % count,
                    ..popup
                });
                Some(Vec::new())
            }
            Key::Enter | Key::Tab => {
                self.popup = None;
                if let Some(choice) = self.shown(&popup).get(popup.at).copied().cloned() {
                    self.replace(popup.from, self.cursor.max(popup.from), &choice.insert, false);
                    self.cursor = self.cursor.saturating_sub(choice.back);
                }
                Some(Vec::new())
            }
            Key::Escape => {
                self.popup = None;
                Some(Vec::new())
            }
            Key::Char(ch) if highlight::is_name_char(ch) => None,
            Key::Backspace => None,
            _ => {
                self.popup = None;
                None
            }
        }
    }

    /// After a key typed into the list: it closes when the cursor left its letters or nothing begins with them.
    fn narrow_popup(&mut self) {
        if let Some(popup) = self.popup.clone() {
            let count = self.shown(&popup).len();
            if self.cursor < popup.from || count == 0 {
                self.popup = None;
            } else {
                self.popup = Some(Popup {
                    at: popup.at.min(count - 1),
                    ..popup
                });
            }
        }
    }

    /// The prompt's keys: typing edits it, Enter acts, Esc closes. None when a Ctrl or Alt key closes it and goes on.
    fn prompt_key(&mut self, press: &KeyPress, rows: usize) -> Option<Vec<Act>> {
        let mut prompt = self.prompt.clone()?;
        if (press.ctrl || press.alt) && press.key != Key::Char('f') && press.key != Key::Char('g') {
            self.prompt = None;
            return None;
        }
        match (&press.key, prompt.ask) {
            (Key::Escape, _) => self.prompt = None,
            (Key::Enter, Ask::Find) => {
                self.prompt = None;
                self.find = prompt.text;
                let from = self.selection().map_or(self.cursor, |(start, _)| start);
                self.find_from(from, true);
            }
            (Key::F(3), Ask::Find) => {
                self.prompt = None;
                self.find = prompt.text;
                self.find_next(!press.shift);
            }
            (Key::Enter, Ask::Line) => {
                self.prompt = None;
                match prompt.text.trim().parse::<usize>() {
                    Ok(line) => self.go_to_line(line, rows),
                    Err(_) => self.message = format!("Not a line number: {}", prompt.text),
                }
            }
            (Key::Char(ch), Ask::Line) if !ch.is_ascii_digit() => {}
            _ => {
                edit::apply(&mut prompt.text, &mut prompt.cursor, press, &Limits::default());
                self.prompt = Some(prompt);
            }
        }
        Some(Vec::new())
    }

    /// A key: None when it is not the editor's, so it goes on to the screen's.
    fn key(&mut self, props: &Value, press: &KeyPress, rows: usize) -> Option<Vec<Act>> {
        if self.prompt.is_some() {
            if let Some(acts) = self.prompt_key(press, rows) {
                return Some(acts);
            }
        }
        if self.popup.is_some() {
            if let Some(acts) = self.popup_key(press) {
                return Some(acts);
            }
        }
        let acts = self.editor_key(props, press, rows)?;
        self.narrow_popup();
        if !matches!(press.key, Key::Tab | Key::BackTab | Key::Escape) {
            self.released = false;
        }
        Some(acts)
    }

    fn refuse(&mut self) -> Option<Vec<Act>> {
        self.message = "Read only".to_string();
        Some(Vec::new())
    }

    fn editor_key(&mut self, props: &Value, press: &KeyPress, rows: usize) -> Option<Vec<Act>> {
        let locked = read_only(props);
        let select = press.shift;
        if press.alt {
            return None;
        }
        if press.ctrl {
            match press.key {
                Key::Left => self.move_to(self.word_left(), select),
                Key::Right => self.move_to(self.word_right(), select),
                Key::Home => self.move_to(0, select),
                Key::End => self.move_to(self.len(), select),
                Key::Char('c') => {
                    let (_, _, text) = self.taken();
                    self.clip.clone_from(&text);
                    return Some(vec![Act::Copy(text)]);
                }
                Key::Char('x') if locked => return self.refuse(),
                Key::Char('x') => {
                    let (start, end, text) = self.taken();
                    self.clip.clone_from(&text);
                    self.replace(start, end, "", false);
                    return Some(vec![Act::Copy(text)]);
                }
                Key::Char('v') if locked => return self.refuse(),
                Key::Char('v') => {
                    if self.clip.is_empty() {
                        self.message =
                            "Nothing copied here yet: the terminal's paste (Ctrl+Shift+V) brings the clipboard in"
                                .to_string();
                    } else {
                        let clip = self.clip.clone();
                        self.insert(&clip, false);
                    }
                }
                Key::Char('z') if locked => return self.refuse(),
                Key::Char('z') => self.undo(),
                Key::Char('y') if locked => return self.refuse(),
                Key::Char('y') => self.redo(),
                Key::Char('f') => self.open_prompt(Ask::Find),
                Key::Char('g') => self.open_prompt(Ask::Line),
                Key::Char(' ') => return Some(self.complete(props)),
                _ => return None,
            }
            return Some(Vec::new());
        }
        match &press.key {
            key @ (Key::Up | Key::Down | Key::Left | Key::Right | Key::Home | Key::End) => {
                let to = self.step(key);
                self.move_to(to, select);
            }
            Key::PageUp | Key::PageDown => {
                let (row, col) = self.row_col();
                let row = if press.key == Key::PageUp {
                    self.top = self.top.saturating_sub(rows);
                    row.saturating_sub(rows)
                } else {
                    self.top = (self.top + rows).min(self.line_count().saturating_sub(1));
                    (row + rows).min(self.line_count() - 1)
                };
                let to = edit::offset_of(&self.text, row, col);
                self.move_to(to, select);
            }
            Key::Tab | Key::BackTab if locked || self.released => return None,
            Key::Tab => self.indent(press.shift),
            Key::BackTab => self.indent(true),
            Key::Escape => {
                if locked || self.released {
                    if self.anchor.take().is_some() {
                        return Some(Vec::new());
                    }
                    return None;
                }
                self.released = true;
                self.anchor = None;
                self.message = "Tab and Shift+Tab move on now".to_string();
            }
            Key::F(3) => self.find_next(!press.shift),
            Key::F(8) => self.next_problem(props, !press.shift),
            Key::F(_) => return None,
            _ if locked => return self.refuse(),
            Key::Enter => self.newline(),
            Key::Backspace => match self.selection() {
                Some((start, end)) => self.replace(start, end, "", false),
                None if self.cursor > 0 => self.replace(self.cursor - 1, self.cursor, "", false),
                None => {}
            },
            Key::Delete => match self.selection() {
                Some((start, end)) => self.replace(start, end, "", false),
                None if self.cursor < self.len() => self.replace(self.cursor, self.cursor + 1, "", false),
                None => {}
            },
            Key::Char(ch) => {
                if ch.is_control() {
                    return Some(Vec::new());
                }
                self.insert(&ch.to_string(), true);
            }
        }
        Some(Vec::new())
    }

    /// The top line moved, as little as it must, so the cursor's line shows.
    fn follow(&mut self, rows: usize) {
        let (row, _) = self.row_col();
        if row < self.top {
            self.top = row;
        } else if row >= self.top + rows {
            self.top = row + 1 - rows;
        }
        self.top = self.top.min(self.line_count().saturating_sub(1));
    }

    /// What a change of the text asks: the form field set, and a check once the typing stops.
    fn after_change(&mut self, props: &Value) -> Vec<Act> {
        if !std::mem::take(&mut self.changed) {
            return Vec::new();
        }
        if !prop(props, "check", "").is_empty() {
            self.check_due = true;
        }
        let (form, field) = (prop(props, "form", ""), prop(props, "field", ""));
        if form.is_empty() || field.is_empty() {
            return Vec::new();
        }
        vec![Act::SetFormField {
            form: form.to_string(),
            field: field.to_string(),
            value: self.text.clone(),
        }]
    }
}

impl Component for Code {
    fn name(&self) -> &'static str {
        "code"
    }

    fn source(&self, _props: &Value, _state: &ComponentState) -> Option<(String, BTreeMap<String, String>)> {
        None
    }

    fn lines(&self, props: &Value, state: &ComponentState, width: u16, _wide: bool) -> Vec<Line> {
        let editor = Editor::of(props, state);
        draw(&editor, props, state, usize::from(width).max(20))
    }

    fn key(&self, props: &Value, state: &mut ComponentState, press: &KeyPress, _wide: bool) -> Option<Vec<Act>> {
        let mut editor = Editor::of(props, state);
        let rows = rows_shown(props, state.rows);
        editor.message.clear();
        let mut acts = editor.key(props, press, rows)?;
        editor.follow(rows);
        acts.extend(editor.after_change(props));
        editor.store(state);
        Some(acts)
    }

    fn receive(&self, props: &Value, state: &mut ComponentState, value: Value) {
        let mut editor = Editor::of(props, state);
        let digest = if value["digest"].is_object() {
            Some(value["digest"].clone())
        } else if value["elements"].is_array() {
            Some(value.clone())
        } else {
            None
        };
        if let Some(problems) = value.get("problems") {
            editor.checked = Some(problems_of(problems));
        } else if let Some(digest) = digest {
            editor.digest_root = complete::root_element(&editor.text);
            editor.digest = digest;
            if std::mem::take(&mut editor.want_popup) {
                let (from, choices) = complete::xml(&editor.digest, &editor.text, editor.cursor);
                editor.open_popup(from, choices);
            }
        } else if let Some(choices) = complete::items(&value) {
            if std::mem::take(&mut editor.want_popup) {
                let from = complete::word_start(&editor.text, editor.cursor);
                editor.open_popup(from, choices);
            }
        } else if let Some(errors) = value["errors"].as_str().filter(|errors| !errors.is_empty()) {
            editor.want_popup = false;
            editor.message = errors.to_string();
        } else if std::mem::take(&mut editor.want_popup) {
            editor.message = "No completions here".to_string();
        }
        editor.store(state);
    }

    fn paste(&self, props: &Value, state: &mut ComponentState, text: &str) -> Option<Vec<Act>> {
        let mut editor = Editor::of(props, state);
        editor.message.clear();
        editor.popup = None;
        if read_only(props) {
            editor.message = "Read only".to_string();
        } else {
            let text = crate::model::clean_text(&text.replace("\r\n", "\n").replace('\r', "\n"));
            editor.insert(&text, false);
            editor.released = false;
        }
        editor.follow(rows_shown(props, state.rows));
        let acts = editor.after_change(props);
        editor.store(state);
        Some(acts)
    }

    fn poll_millis(&self, props: &Value, state: &ComponentState) -> Option<u64> {
        (state.local["check_due"] == true && !prop(props, "check", "").is_empty()).then_some(CHECK_MILLIS)
    }

    fn tick(&self, props: &Value, state: &mut ComponentState) -> Vec<Act> {
        let mut editor = Editor::of(props, state);
        if !std::mem::take(&mut editor.check_due) {
            return Vec::new();
        }
        editor.store(state);
        let url = prop(props, "check", "");
        if url.is_empty() {
            return Vec::new();
        }
        vec![Editor::call(
            props,
            url,
            &[
                ("text", editor.text.clone()),
                ("language", language(props).name().to_string()),
            ],
        )]
    }

    fn help(&self) -> &'static [(&'static str, &'static str)] {
        &[
            (
                "Arrows, Home, End, PgUp, PgDn",
                "Move; Ctrl+Home and Ctrl+End to the start and end",
            ),
            ("Ctrl+Left, Ctrl+Right", "Move by word"),
            ("Shift with a move", "Select"),
            (
                "Ctrl+C, Ctrl+X, Ctrl+V",
                "Copy, cut (the line when nothing is selected), paste what was copied",
            ),
            ("Tab, Shift+Tab", "Indent, outdent the line or the lines selected"),
            ("Esc", "Let go of the focus: then Tab and Shift+Tab move on"),
            ("Ctrl+Z, Ctrl+Y", "Undo, redo"),
            ("Ctrl+G", "Go to a line"),
            ("Ctrl+F, F3, Shift+F3", "Find in the text, the next match, the previous"),
            (
                "Ctrl+Space",
                "Complete: Up and Down choose, Enter or Tab takes, Esc closes",
            ),
            ("F8, Shift+F8", "The next, the previous problem"),
            ("Ctrl+S, Alt+Enter", "Save: submit the form"),
        ]
    }
}

// ---- drawing ----

/// One character as drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    paint: Paint,
    selected: bool,
    cursor: bool,
}

fn spans_of(chars: &[char], cells: &[Cell]) -> Line {
    let mut line: Line = Vec::new();
    let mut run = String::new();
    let mut look: Option<Cell> = None;
    let flush = |line: &mut Line, run: &mut String, cell: Cell| {
        if run.is_empty() {
            return;
        }
        let role = if cell.cursor {
            Role::Focus
        } else if cell.selected {
            Role::Selection
        } else {
            cell.paint.0
        };
        let mut span = Span::new(std::mem::take(run), role);
        if cell.paint.1 && !cell.cursor && !cell.selected {
            span = span.bold();
        }
        if cell.cursor || cell.selected {
            span = span.reverse();
        }
        line.push(span);
    };
    for (ch, cell) in chars.iter().zip(cells) {
        if let Some(previous) = look.filter(|previous| previous != cell) {
            flush(&mut line, &mut run, previous);
        }
        look = Some(*cell);
        run.push(if *ch == '\t' { ' ' } else { *ch });
    }
    if let Some(look) = look {
        flush(&mut line, &mut run, look);
    }
    line
}

fn draw(editor: &Editor, props: &Value, state: &ComponentState, width: usize) -> Vec<Line> {
    let language = language(props);
    let rows = rows_shown(props, state.rows);
    let texts: Vec<&str> = editor.text.split('\n').collect();
    let total = texts.len();
    let digits = total.to_string().len().max(3);
    let gutter = digits + 2;
    let room = width.saturating_sub(gutter).max(8);
    let (cursor_row, cursor_col) = editor.row_col();
    let left = scroll_left(cursor_col, room);
    let top = editor.top.min(total.saturating_sub(1));
    let problems = editor.problems(props);
    let mut marks: BTreeMap<usize, bool> = BTreeMap::new();
    for problem in &problems {
        let error = marks.entry(problem.line).or_insert(false);
        *error |= problem.is_error();
    }
    let selection = editor
        .selection()
        .map(|(start, end)| (edit::row_col(&editor.text, start), edit::row_col(&editor.text, end)));
    let mut carry = Carry::default();
    for text in &texts[..top] {
        carry = highlight::line(language, text, carry).1;
    }
    let mut lines: Vec<Line> = Vec::new();
    for (row, text) in texts.iter().enumerate().skip(top).take(rows) {
        let (paints, next) = highlight::line(language, text, carry);
        carry = next;
        let mut line = vec![Span::new(
            format!("{:>digits$}", row + 1),
            if row == cursor_row { Role::Text } else { Role::Muted },
        )];
        line.push(match marks.get(&(row + 1)) {
            Some(true) => Span::new("!", Role::Negative).bold(),
            Some(false) => Span::new("·", Role::Warning),
            None => Span::new(" ", Role::Muted),
        });
        line.push(Span::new(" ", Role::Muted));
        let mut chars: Vec<char> = text.chars().collect();
        let mut cells: Vec<Cell> = paints
            .iter()
            .enumerate()
            .map(|(col, paint)| Cell {
                paint: *paint,
                selected: selection.is_some_and(|(start, end)| (row, col) >= start && (row, col) < end),
                cursor: row == cursor_row && col == cursor_col,
            })
            .collect();
        if row == cursor_row && cursor_col >= chars.len() {
            chars.push(' ');
            cells.push(Cell {
                paint: (Role::Text, false),
                selected: false,
                cursor: true,
            });
        }
        let shown_chars: Vec<char> = chars.iter().skip(left).take(room).copied().collect();
        let shown_cells: Vec<Cell> = cells.iter().skip(left).take(room).copied().collect();
        line.extend(spans_of(&shown_chars, &shown_cells));
        lines.push(line);
    }
    if let Some(popup) = &editor.popup {
        popup_lines(
            editor,
            popup,
            cursor_row - top.min(cursor_row),
            rows,
            gutter,
            left,
            &mut lines,
        );
    }
    lines.push(status_line(editor, props, state, language, width));
    if let Some(prompt) = &editor.prompt {
        lines.push(prompt_line(prompt, width));
    }
    lines
}

/// The completion list under the cursor's line (over the lines below it), or above it where there is no room below.
fn popup_lines(
    editor: &Editor,
    popup: &Popup,
    cursor_at: usize,
    rows: usize,
    gutter: usize,
    left: usize,
    lines: &mut Vec<Line>,
) {
    let shown = editor.shown(popup);
    if shown.is_empty() {
        return;
    }
    let count = shown.len().min(POPUP_ROWS);
    let first = popup.at.saturating_sub(count - 1).min(shown.len() - count);
    let label_width = shown.iter().map(|choice| chars_in(&choice.label)).max().unwrap_or(0);
    let (_, from_col) = edit::row_col(&editor.text, popup.from);
    let pad = gutter + from_col.saturating_sub(left);
    let below = rows.saturating_sub(cursor_at + 1);
    let start = if below >= count || cursor_at < count {
        cursor_at + 1
    } else {
        cursor_at - count
    };
    for (offset, choice) in shown.iter().skip(first).take(count).enumerate() {
        let text = if choice.detail.is_empty() {
            format!(" {:<label_width$} ", choice.label)
        } else {
            format!(" {:<label_width$}  {} ", choice.label, choice.detail)
        };
        let mut span = Span::new(text, Role::Field);
        if first + offset == popup.at {
            span = span.reverse();
        }
        let line = vec![Span::new(" ".repeat(pad), Role::Text), span];
        let at = start + offset;
        if at < lines.len() {
            lines[at] = line;
        } else {
            lines.push(line);
        }
    }
}

fn status_line(editor: &Editor, props: &Value, state: &ComponentState, language: Language, width: usize) -> Line {
    let (row, col) = editor.row_col();
    let mut line = vec![
        Span::new(format!("Ln {}, Col {}", row + 1, col + 1), Role::Text),
        Span::new(format!("  {}", language.name()), Role::Muted),
    ];
    if editor.modified() {
        line.push(Span::new("  ●", Role::Changed));
    }
    if read_only(props) {
        line.push(Span::new("  read only", Role::Muted));
    }
    if !editor.message.is_empty() {
        line.push(Span::new(format!("  {}", editor.message), Role::Info));
    } else if !state.error.is_empty() {
        line.push(Span::new(format!("  ! {}", state.error), Role::Negative));
    } else if let Some(problem) = editor.problem_here(props) {
        let (mark, role) = if problem.is_error() {
            ("!", Role::Negative)
        } else {
            ("·", Role::Warning)
        };
        line.push(Span::new(format!("  {mark} {}", problem.message), role));
    } else if editor.released {
        line.push(Span::new("  Tab moves on", Role::Muted));
    }
    // cut to the width
    let mut left = width;
    let mut fitted = Vec::new();
    for mut span in line {
        if left == 0 {
            break;
        }
        let text = truncate_width(&span.text, left);
        left = left.saturating_sub(crate::layout::display_width(&text));
        span.text = text;
        fitted.push(span);
    }
    fitted
}

fn prompt_line(prompt: &Prompt, width: usize) -> Line {
    let (label, hint) = match prompt.ask {
        Ask::Find => ("Find: ", "  Enter finds, F3 the next, Esc closes"),
        Ask::Line => ("Go to line: ", "  Enter goes, Esc closes"),
    };
    let chars: Vec<char> = prompt.text.chars().collect();
    let at = prompt.cursor.min(chars.len());
    let before: String = chars[..at].iter().collect();
    let under: String = chars.get(at).map_or(" ".to_string(), char::to_string);
    let after: String = chars.iter().skip(at + 1).collect();
    let line = vec![
        Span::new(label, Role::Primary).bold(),
        Span::new(before, Role::Field),
        Span::new(under, Role::Focus).reverse(),
        Span::new(after, Role::Field),
        Span::new(hint, Role::Muted),
    ];
    let mut left = width;
    line.into_iter()
        .filter_map(|mut span| {
            if left == 0 {
                return None;
            }
            span.text = truncate_width(&span.text, left);
            left = left.saturating_sub(crate::layout::display_width(&span.text));
            Some(span)
        })
        .collect()
}

#[cfg(test)]
mod tests;
