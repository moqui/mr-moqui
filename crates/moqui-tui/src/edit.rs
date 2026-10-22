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
//! edits and the field widget draws.

use crate::keys::{Key, KeyPress};

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

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(keys: &str, limits: &Limits) -> String {
        let mut text = String::new();
        let mut cursor = 0;
        for ch in keys.chars() {
            apply(&mut text, &mut cursor, &KeyPress::plain(Key::Char(ch)), limits);
        }
        text
    }

    #[test]
    fn maxlength_and_mask_hold_while_typing() {
        let short = Limits {
            maxlength: 3,
            ..Limits::default()
        };
        assert_eq!(typed("abcdef", &short), "abc");
        let phone = Limits {
            mask: "(999) 999-9999".to_string(),
            ..Limits::default()
        };
        assert_eq!(typed("555x1234567", &phone), "(555) 123-4567");
        let mut text = "héllo".to_string();
        let mut cursor = 2;
        apply(
            &mut text,
            &mut cursor,
            &KeyPress::plain(Key::Backspace),
            &Limits::default(),
        );
        assert_eq!((text.as_str(), cursor), ("hllo", 1));
    }

    #[test]
    fn a_text_area_moves_between_lines_and_hands_off_at_the_edges() {
        let area = Limits {
            multiline: true,
            ..Limits::default()
        };
        let text = "one\ntwo".to_string();
        let mut cursor = 5;
        assert_eq!(
            apply(&mut text.clone(), &mut cursor, &KeyPress::plain(Key::Up), &area),
            Edited::Moved
        );
        assert_eq!(cursor, 1);
        assert_eq!(
            apply(&mut text.clone(), &mut cursor, &KeyPress::plain(Key::Up), &area),
            Edited::No
        );
        assert_eq!(row_col("one\ntwo", 5), (1, 1));
    }
}
