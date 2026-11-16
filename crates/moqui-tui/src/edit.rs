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

//! Editing one field's text: the cursor as a char index, `maxlength` and an input mask enforced while typing (section
//! 1.6), and the lines of a text area. Revue's `Input` takes no cursor from outside and no length or mask, so the session
//! edits and the field widget draws. Over that, [`apply_field`] gives a field the desktop editor's keys: a selection,
//! the clipboard, undo and redo, word moves and deletes; the undo log and the word moves are the `code` editor's too.

use serde::{Deserialize, Serialize};

use crate::keys::{Key, KeyPress};

/// The edits an undo log keeps.
const UNDO_DEPTH: usize = 200;

/// How a field takes text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// 0 is no limit.
    pub maxlength: usize,
    /// An Inputmask pattern: `9` a digit, `a` a letter, `*` a letter or digit; any other character is written for the
    /// user as it is reached.
    pub mask: String,
    /// Enter is a newline.
    pub multiline: bool,
}

/// What a key did to the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edited {
    /// Not an editing key: the session handles it.
    No,
    /// The cursor moved.
    Moved,
    /// The text changed.
    Changed,
}

/// Apply one key. Plain characters, Backspace, Delete, Left, Right, Home and End are editing keys; in a text area so are
/// Enter, and Up and Down while there is a line to move to.
pub fn apply(text: &mut String, cursor: &mut usize, press: &KeyPress, limits: &Limits) -> Edited {
    let len = text.chars().count();
    *cursor = (*cursor).min(len);
    if press.ctrl || press.alt {
        return Edited::No;
    }
    match &press.key {
        Key::Char(ch) => insert(text, cursor, *ch, limits),
        Key::Enter if limits.multiline => insert(text, cursor, '\n', limits),
        Key::Backspace => {
            if *cursor == 0 {
                return Edited::Moved;
            }
            remove(text, *cursor - 1);
            *cursor -= 1;
            Edited::Changed
        }
        Key::Delete => {
            if *cursor >= len {
                return Edited::Moved;
            }
            remove(text, *cursor);
            Edited::Changed
        }
        Key::Left if !press.shift => {
            *cursor = cursor.saturating_sub(1);
            Edited::Moved
        }
        Key::Right if !press.shift => {
            *cursor = (*cursor + 1).min(len);
            Edited::Moved
        }
        Key::Home => {
            *cursor = line_start(text, *cursor);
            Edited::Moved
        }
        Key::End => {
            *cursor = line_end(text, *cursor);
            Edited::Moved
        }
        Key::Up if limits.multiline => vertical(text, cursor, -1),
        Key::Down if limits.multiline => vertical(text, cursor, 1),
        _ => Edited::No,
    }
}

fn insert(text: &mut String, cursor: &mut usize, ch: char, limits: &Limits) -> Edited {
    if ch.is_control() && ch != '\n' {
        return Edited::No;
    }
    let mask: Vec<char> = limits.mask.chars().collect();
    if !mask.is_empty() {
        // Literal characters of the mask are written as the cursor reaches them, unless the user types the literal.
        while *cursor < mask.len() && !is_slot(mask[*cursor]) && mask[*cursor] != ch {
            put(text, *cursor, mask[*cursor]);
            *cursor += 1;
        }
        let Some(slot) = mask.get(*cursor) else {
            return Edited::Moved;
        };
        if is_slot(*slot) && !fits(*slot, ch) {
            return Edited::Moved;
        }
    }
    let len = text.chars().count();
    if limits.maxlength > 0 && len >= limits.maxlength {
        return Edited::Moved;
    }
    put(text, *cursor, ch);
    *cursor += 1;
    Edited::Changed
}

fn is_slot(ch: char) -> bool {
    matches!(ch, '9' | 'a' | '*')
}

fn fits(slot: char, ch: char) -> bool {
    match slot {
        '9' => ch.is_ascii_digit(),
        'a' => ch.is_alphabetic(),
        _ => ch.is_alphanumeric(),
    }
}

fn byte_at(text: &str, index: usize) -> usize {
    text.char_indices().nth(index).map_or(text.len(), |(at, _)| at)
}

fn put(text: &mut String, index: usize, ch: char) {
    let at = byte_at(text, index);
    text.insert(at, ch);
}

fn remove(text: &mut String, index: usize) {
    let at = byte_at(text, index);
    if at < text.len() {
        text.remove(at);
    }
}

fn line_start(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut at = cursor.min(chars.len());
    while at > 0 && chars[at - 1] != '\n' {
        at -= 1;
    }
    at
}

fn line_end(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut at = cursor.min(chars.len());
    while at < chars.len() && chars[at] != '\n' {
        at += 1;
    }
    at
}

/// Up or Down a line, keeping the column. At the first or last line it is not an editing key, so focus moves on.
fn vertical(text: &str, cursor: &mut usize, delta: isize) -> Edited {
    let start = line_start(text, *cursor);
    let column = *cursor - start;
    let len = text.chars().count();
    if delta < 0 {
        if start == 0 {
            return Edited::No;
        }
        let above = line_start(text, start - 1);
        *cursor = (above + column).min(start - 1);
    } else {
        let end = line_end(text, *cursor);
        if end >= len {
            return Edited::No;
        }
        let below = end + 1;
        *cursor = (below + column).min(line_end(text, below));
    }
    Edited::Moved
}

/// The cursor at `row` and `col` of a text, the column cut to the row's end: the inverse of [`row_col`].
pub fn offset_of(text: &str, row: usize, col: usize) -> usize {
    let mut offset = 0;
    for (at, line) in text.split('\n').enumerate() {
        let len = line.chars().count();
        if at == row {
            return offset + col.min(len);
        }
        offset += len + 1;
    }
    text.chars().count()
}

/// The row and column of the cursor in a text area's text, for drawing.
pub fn row_col(text: &str, cursor: usize) -> (usize, usize) {
    let mut row = 0;
    let mut col = 0;
    for (index, ch) in text.chars().enumerate() {
        if index == cursor {
            break;
        }
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}

/// One edit, for undo: `removed` at `at` became `inserted`, with the cursor and selection before and the cursor after.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Op {
    pub at: usize,
    pub removed: String,
    pub inserted: String,
    pub before: usize,
    pub anchor: Option<usize>,
    pub after: usize,
    /// Typed characters, which the next typed one joins.
    pub typed: bool,
}

/// An undo log and its redo, of [`Op`]s on one text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    undo: Vec<Op>,
    redo: Vec<Op>,
}

impl History {
    /// Keep an edit. A typed character right after the typed ones before it joins their edit, so undo takes back a
    /// word's typing at once; any edit clears the redo.
    pub fn record(&mut self, op: Op) {
        self.redo.clear();
        let joins = op.typed
            && op.removed.is_empty()
            && !op.inserted.contains('\n')
            && self
                .undo
                .last()
                .is_some_and(|last| last.typed && last.at + chars_in(&last.inserted) == op.at);
        match self.undo.last_mut() {
            Some(last) if joins => {
                last.inserted.push_str(&op.inserted);
                last.after = op.after;
            }
            _ => {
                self.undo.push(op);
                if self.undo.len() > UNDO_DEPTH {
                    self.undo.remove(0);
                }
            }
        }
    }

    /// Take back the last edit of `text`: the cursor and the selection's other end it had before, or None with
    /// nothing to undo.
    pub fn undo(&mut self, text: &mut String) -> Option<(usize, Option<usize>)> {
        let op = self.undo.pop()?;
        let (from, to) = (byte_at(text, op.at), byte_at(text, op.at + chars_in(&op.inserted)));
        text.replace_range(from..to, &op.removed);
        let back = (op.before, op.anchor);
        self.redo.push(op);
        Some(back)
    }

    /// Make the last undone edit again: the cursor after it, or None with nothing to redo.
    pub fn redo(&mut self, text: &mut String) -> Option<usize> {
        let op = self.redo.pop()?;
        let (from, to) = (byte_at(text, op.at), byte_at(text, op.at + chars_in(&op.removed)));
        text.replace_range(from..to, &op.inserted);
        let after = op.after;
        self.undo.push(op);
        Some(after)
    }
}

/// The edit that made `new` of `old`: what lies between their common start and their common end; None when they are
/// the same.
pub fn diff_op(old: &str, new: &str, before: usize, anchor: Option<usize>, after: usize, typed: bool) -> Option<Op> {
    if old == new {
        return None;
    }
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - start;
    let tail = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    Some(Op {
        at: start,
        removed: old[start..old.len() - tail].iter().collect(),
        inserted: new[start..new.len() - tail].iter().collect(),
        before,
        anchor,
        after,
        typed,
    })
}

pub fn chars_in(text: &str) -> usize {
    text.chars().count()
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The start of the word before `cursor`, or of the run of marks before it.
pub fn word_left(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().take(cursor).collect();
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

/// The end of the word after `cursor`, or of the run of marks after it.
pub fn word_right(text: &str, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().skip(cursor).collect();
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
    cursor + at
}

/// The chars `start..end` of `text`.
pub fn slice(text: &str, start: usize, end: usize) -> &str {
    &text[byte_at(text, start)..byte_at(text, end)]
}

/// A text field's selection (the end other than the cursor) and its undo log, which the session keeps by field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldEdit {
    pub anchor: Option<usize>,
    pub history: History,
}

impl FieldEdit {
    /// The selection as a start and an end, when there is one.
    pub fn selection(&self, cursor: usize) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        (anchor != cursor).then(|| (anchor.min(cursor), anchor.max(cursor)))
    }
}

/// What a key did in a field: to the text, and what goes on the clipboard (Ctrl+C, Ctrl+X) or the message line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldOutcome {
    pub edited: Option<Edited>,
    pub copy: Option<String>,
    pub message: Option<&'static str>,
}

impl FieldOutcome {
    fn of(edited: Edited) -> Self {
        Self {
            edited: Some(edited),
            ..Self::default()
        }
    }
}

/// What a field's keys may use besides the key: the clipboard text for Ctrl+V, and whether Ctrl+A selects all (Modern;
/// Classic leaves Ctrl+A to GNU screen).
#[derive(Clone, Copy, Debug, Default)]
pub struct FieldKeys<'a> {
    pub clip: &'a str,
    pub select_all: bool,
}

/// One key in a text field, the desktop editor's keys over [`apply`]:
///
/// - Shift with Left, Right, Home and End (and Up and Down in a text area) selects; Ctrl+Left and Ctrl+Right move a word,
///   Ctrl+Home and Ctrl+End to the text's ends; Left or Right with a selection goes to its start or end.
/// - Ctrl+C copies the selection, else all of the field; Ctrl+X cuts the same; Ctrl+V puts `clip` in, through the
///   field's length and mask.
/// - Ctrl+Z undoes, Ctrl+Y redoes.
/// - Ctrl+Backspace and Alt+Backspace delete the word before the cursor, Ctrl+Delete the word after it, Ctrl+U back to
///   the line's start; Ctrl+E goes to the line's end.
/// - With `select_all`, Ctrl+A selects all of it. Esc clears a selection.
/// - Typing, Backspace and Delete take the selection first.
pub fn apply_field(
    text: &mut String,
    cursor: &mut usize,
    state: &mut FieldEdit,
    press: &KeyPress,
    limits: &Limits,
    keys: FieldKeys,
) -> FieldOutcome {
    let len = chars_in(text);
    *cursor = (*cursor).min(len);
    state.anchor = state.anchor.map(|anchor| anchor.min(len));
    let old = text.clone();
    let (before, anchor_before) = (*cursor, state.anchor);
    let selected = state.selection(*cursor);
    let outcome = field_key(text, cursor, state, press, limits, keys, selected);
    // Undo and redo move through the log; they are not edits of their own.
    let undoing = press.ctrl && matches!(press.key, Key::Char('z' | 'y'));
    if outcome.edited == Some(Edited::Changed) && !undoing {
        let typed = press.is_plain() && matches!(press.key, Key::Char(_)) && selected.is_none();
        if let Some(op) = diff_op(&old, text, before, anchor_before, *cursor, typed) {
            state.history.record(op);
        }
    }
    outcome
}

fn field_key(
    text: &mut String,
    cursor: &mut usize,
    state: &mut FieldEdit,
    press: &KeyPress,
    limits: &Limits,
    keys: FieldKeys,
    selected: Option<(usize, usize)>,
) -> FieldOutcome {
    let len = chars_in(text);
    // Take the selection out, for a key that writes over it.
    let take = |text: &mut String, cursor: &mut usize, state: &mut FieldEdit| {
        if let Some((start, end)) = selected {
            text.replace_range(byte_at(text, start)..byte_at(text, end), "");
            *cursor = start;
        }
        state.anchor = None;
    };
    let target = |key: &Key, ctrl: bool| -> Option<usize> {
        Some(match key {
            Key::Left if ctrl => word_left(text, *cursor),
            Key::Right if ctrl => word_right(text, *cursor),
            Key::Left => cursor.saturating_sub(1),
            Key::Right => (*cursor + 1).min(len),
            Key::Home if ctrl => 0,
            Key::End if ctrl => len,
            Key::Home => line_start(text, *cursor),
            Key::End => line_end(text, *cursor),
            Key::Up | Key::Down if limits.multiline && !ctrl => {
                let mut to = *cursor;
                let delta = if *key == Key::Up { -1 } else { 1 };
                if vertical(text, &mut to, delta) == Edited::No {
                    return None;
                }
                to
            }
            _ => return None,
        })
    };
    if press.alt && !press.ctrl {
        if press.key == Key::Backspace {
            return delete_to(text, cursor, state, selected, word_left(text, *cursor));
        }
        return FieldOutcome::default();
    }
    if press.ctrl {
        return match &press.key {
            Key::Char('c') => {
                let (start, end) = selected.unwrap_or((0, len));
                let copied = slice(text, start, end).to_string();
                FieldOutcome {
                    edited: Some(Edited::Moved),
                    message: copied.is_empty().then_some("Nothing to copy"),
                    copy: (!copied.is_empty()).then_some(copied),
                }
            }
            Key::Char('x') => {
                let (start, end) = selected.unwrap_or((0, len));
                let copied = slice(text, start, end).to_string();
                if copied.is_empty() {
                    return FieldOutcome {
                        edited: Some(Edited::Moved),
                        message: Some("Nothing to cut"),
                        ..FieldOutcome::default()
                    };
                }
                text.replace_range(byte_at(text, start)..byte_at(text, end), "");
                *cursor = start;
                state.anchor = None;
                FieldOutcome {
                    edited: Some(Edited::Changed),
                    copy: Some(copied),
                    message: None,
                }
            }
            Key::Char('v') if keys.clip.is_empty() => FieldOutcome {
                edited: Some(Edited::Moved),
                message: Some("Nothing copied here yet: the terminal's paste (Ctrl+Shift+V) brings the clipboard in"),
                ..FieldOutcome::default()
            },
            Key::Char('v') => {
                take(text, cursor, state);
                put_text(text, cursor, keys.clip, limits);
                FieldOutcome::of(Edited::Changed)
            }
            Key::Char('z') => match state.history.undo(text) {
                Some((back, anchor)) => {
                    *cursor = back.min(chars_in(text));
                    state.anchor = anchor;
                    FieldOutcome::of(Edited::Changed)
                }
                None => FieldOutcome {
                    edited: Some(Edited::Moved),
                    message: Some("Nothing to undo"),
                    ..FieldOutcome::default()
                },
            },
            Key::Char('y') => match state.history.redo(text) {
                Some(after) => {
                    *cursor = after.min(chars_in(text));
                    state.anchor = None;
                    FieldOutcome::of(Edited::Changed)
                }
                None => FieldOutcome {
                    edited: Some(Edited::Moved),
                    message: Some("Nothing to redo"),
                    ..FieldOutcome::default()
                },
            },
            Key::Char('a') if keys.select_all => {
                state.anchor = Some(0);
                *cursor = len;
                FieldOutcome::of(Edited::Moved)
            }
            Key::Char('u') => delete_to(text, cursor, state, selected, line_start(text, *cursor)),
            Key::Char('e') => {
                *cursor = line_end(text, *cursor);
                state.anchor = None;
                FieldOutcome::of(Edited::Moved)
            }
            Key::Backspace => delete_to(text, cursor, state, selected, word_left(text, *cursor)),
            Key::Delete => delete_to(text, cursor, state, selected, word_right(text, *cursor)),
            key => match target(key, true) {
                Some(to) => {
                    move_to(cursor, state, to, press.shift);
                    FieldOutcome::of(Edited::Moved)
                }
                None => FieldOutcome::default(),
            },
        };
    }
    if press.shift && !matches!(press.key, Key::Char(_)) {
        return match target(&press.key, false) {
            Some(to) => {
                move_to(cursor, state, to, true);
                FieldOutcome::of(Edited::Moved)
            }
            None => FieldOutcome::default(),
        };
    }
    match (&press.key, selected) {
        (Key::Left, Some((start, _))) => {
            *cursor = start;
            state.anchor = None;
            FieldOutcome::of(Edited::Moved)
        }
        (Key::Right, Some((_, end))) => {
            *cursor = end;
            state.anchor = None;
            FieldOutcome::of(Edited::Moved)
        }
        (Key::Escape, Some(_)) => {
            state.anchor = None;
            FieldOutcome::of(Edited::Moved)
        }
        (Key::Backspace | Key::Delete, Some(_)) => {
            take(text, cursor, state);
            FieldOutcome::of(Edited::Changed)
        }
        (Key::Char(_), Some(_)) => {
            take(text, cursor, state);
            apply(text, cursor, press, limits);
            FieldOutcome::of(Edited::Changed)
        }
        (Key::Enter, Some(_)) if limits.multiline => {
            take(text, cursor, state);
            apply(text, cursor, press, limits);
            FieldOutcome::of(Edited::Changed)
        }
        _ => {
            let edited = apply(text, cursor, press, limits);
            if edited != Edited::No {
                state.anchor = None;
            }
            FieldOutcome::of(edited)
        }
    }
}

/// Move the cursor to `to`; with `select`, the selection grows from where it was.
fn move_to(cursor: &mut usize, state: &mut FieldEdit, to: usize, select: bool) {
    if select {
        let anchor = *state.anchor.get_or_insert(*cursor);
        *cursor = to;
        if anchor == to {
            state.anchor = None;
        }
    } else {
        *cursor = to;
        state.anchor = None;
    }
}

/// Delete from the cursor to `to` (a word or the line's start or end), or the selection when there is one.
fn delete_to(
    text: &mut String,
    cursor: &mut usize,
    state: &mut FieldEdit,
    selected: Option<(usize, usize)>,
    to: usize,
) -> FieldOutcome {
    let (start, end) = selected.unwrap_or((to.min(*cursor), to.max(*cursor)));
    state.anchor = None;
    if start == end {
        return FieldOutcome::of(Edited::Moved);
    }
    text.replace_range(byte_at(text, start)..byte_at(text, end), "");
    *cursor = start;
    FieldOutcome::of(Edited::Changed)
}

/// Text put in at the cursor a character at a time through the field's length and mask, as typing would; a newline in
/// a one-line field is a space.
pub fn put_text(text: &mut String, cursor: &mut usize, added: &str, limits: &Limits) {
    for ch in added.chars().filter(|ch| *ch != '\r') {
        let press = if ch == '\n' && limits.multiline {
            KeyPress::plain(Key::Enter)
        } else if ch == '\n' || ch == '\t' {
            KeyPress::plain(Key::Char(' '))
        } else {
            KeyPress::plain(Key::Char(ch))
        };
        apply(text, cursor, &press, limits);
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
