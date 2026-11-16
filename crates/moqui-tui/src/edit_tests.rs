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

//! The field editing of `edit.rs`: typing under a length and a mask, and a text area's lines.

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

/// A field's text, cursor and state, and the keys pressed in it.
struct Field {
    text: String,
    cursor: usize,
    state: FieldEdit,
    limits: Limits,
    clip: String,
    select_all: bool,
    copied: Vec<String>,
    said: Vec<&'static str>,
}

impl Field {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            cursor: chars_in(text),
            state: FieldEdit::default(),
            limits: Limits::default(),
            clip: String::new(),
            select_all: true,
            copied: Vec::new(),
            said: Vec::new(),
        }
    }

    fn press(&mut self, press: KeyPress) -> Option<Edited> {
        let keys = FieldKeys {
            clip: &self.clip,
            select_all: self.select_all,
        };
        let outcome = apply_field(
            &mut self.text,
            &mut self.cursor,
            &mut self.state,
            &press,
            &self.limits,
            keys,
        );
        if let Some(copied) = outcome.copy {
            self.clip.clone_from(&copied);
            self.copied.push(copied);
        }
        self.said.extend(outcome.message);
        outcome.edited
    }

    fn key(&mut self, key: Key) -> Option<Edited> {
        self.press(KeyPress::plain(key))
    }

    fn shift(&mut self, key: Key) -> Option<Edited> {
        self.press(KeyPress {
            shift: true,
            ..KeyPress::plain(key)
        })
    }

    fn ctrl(&mut self, key: Key) -> Option<Edited> {
        self.press(KeyPress::with_ctrl(key))
    }

    fn typed(&mut self, text: &str) {
        for ch in text.chars() {
            self.key(Key::Char(ch));
        }
    }

    fn selected(&self) -> Option<&str> {
        self.state
            .selection(self.cursor)
            .map(|(start, end)| slice(&self.text, start, end))
    }
}

#[test]
fn shift_selects_and_typing_backspace_and_left_take_the_selection() {
    let mut field = Field::new("hello world");
    field.shift(Key::Left);
    field.shift(Key::Left);
    assert_eq!(field.selected(), Some("ld"));
    field.typed("k");
    assert_eq!(field.text, "hello work");
    assert_eq!(field.selected(), None);
    // Ctrl+Shift+Left selects a word; Backspace deletes it.
    field.press(KeyPress {
        ctrl: true,
        shift: true,
        ..KeyPress::plain(Key::Left)
    });
    assert_eq!(field.selected(), Some("work"));
    assert_eq!(field.key(Key::Backspace), Some(Edited::Changed));
    assert_eq!((field.text.as_str(), field.cursor), ("hello ", 6));
    // Shift+Home selects to the start; Left goes to the selection's start; Escape drops a selection.
    field.shift(Key::Home);
    assert_eq!(field.selected(), Some("hello "));
    field.key(Key::Left);
    assert_eq!((field.cursor, field.selected()), (0, None));
    field.shift(Key::End);
    assert_eq!(field.key(Key::Escape), Some(Edited::Moved));
    assert_eq!(field.selected(), None);
    // with no selection Escape is not the field's
    assert_eq!(field.key(Key::Escape), Some(Edited::No));
}

#[test]
fn copy_cut_and_paste_go_through_the_clip_and_the_fields_limits() {
    let mut field = Field::new("abc def");
    // nothing selected: Ctrl+C copies all of the field
    field.ctrl(Key::Char('c'));
    assert_eq!(field.copied, ["abc def"]);
    field.ctrl(Key::Left);
    field.press(KeyPress {
        ctrl: true,
        shift: true,
        ..KeyPress::plain(Key::Right)
    });
    assert_eq!(field.selected(), Some("def"));
    assert_eq!(field.ctrl(Key::Char('x')), Some(Edited::Changed));
    assert_eq!(field.text, "abc ");
    assert_eq!(field.clip, "def");
    field.ctrl(Key::Home);
    field.ctrl(Key::Char('v'));
    assert_eq!(field.text, "defabc ");
    // a paste keeps to the length and the mask, and a newline in one line is a space
    let mut short = Field::new("");
    short.limits.maxlength = 4;
    short.clip = "a\nbcdef".to_string();
    short.ctrl(Key::Char('v'));
    assert_eq!(short.text, "a bc");
    let mut empty = Field::new("x");
    assert_eq!(empty.ctrl(Key::Char('v')), Some(Edited::Moved));
    assert!(empty.said[0].starts_with("Nothing copied"), "{:?}", empty.said);
}

#[test]
fn undo_takes_back_a_run_of_typing_at_once_and_redo_makes_it_again() {
    let mut field = Field::new("");
    field.typed("one");
    field.key(Key::Char(' '));
    field.typed("two");
    field.ctrl(Key::Backspace);
    assert_eq!(field.text, "one ");
    field.ctrl(Key::Char('z'));
    assert_eq!((field.text.as_str(), field.cursor), ("one two", 7));
    field.ctrl(Key::Char('z'));
    assert_eq!(field.text, "");
    assert_eq!(field.ctrl(Key::Char('z')), Some(Edited::Moved));
    assert_eq!(field.said, ["Nothing to undo"]);
    field.ctrl(Key::Char('y'));
    assert_eq!(field.text, "one two");
    // a new edit clears the redo
    field.typed("!");
    assert_eq!(field.ctrl(Key::Char('y')), Some(Edited::Moved));
    assert_eq!(field.text, "one two!");
}

#[test]
fn word_and_line_deletes_ctrl_a_and_keys_that_are_not_the_fields() {
    let mut field = Field::new("alpha beta gamma");
    field.press(KeyPress::with_alt(Key::Backspace));
    assert_eq!(field.text, "alpha beta ");
    field.ctrl(Key::Home);
    field.ctrl(Key::Delete);
    assert_eq!(field.text, " beta ");
    field.ctrl(Key::Char('e'));
    assert_eq!(field.cursor, 6);
    field.ctrl(Key::Char('u'));
    assert_eq!(field.text, "");
    let mut all = Field::new("select me");
    all.ctrl(Key::Char('a'));
    assert_eq!(all.selected(), Some("select me"));
    all.select_all = false;
    all.key(Key::End);
    assert_eq!(all.ctrl(Key::Char('a')), None, "Classic leaves Ctrl+A to the keymap");
    // the screen's Ctrl keys are not the field's
    for ch in ['s', 'k', 'q', 'r', 'f'] {
        assert_eq!(all.ctrl(Key::Char(ch)), None, "Ctrl+{ch}");
    }
    assert_eq!(all.press(KeyPress::with_alt(Key::Left)), None);
}

#[test]
fn diff_op_finds_the_changed_middle() {
    let op = diff_op("hello world", "hello there world", 6, None, 12, false).unwrap();
    assert_eq!((op.at, op.removed.as_str(), op.inserted.as_str()), (6, "", "there "));
    let op = diff_op("aaa", "aa", 3, None, 2, false).unwrap();
    assert_eq!((op.at, op.removed.as_str(), op.inserted.as_str()), (2, "a", ""));
    assert_eq!(diff_op("same", "same", 0, None, 0, false), None);
    assert_eq!((word_left("foo.bar baz", 7), word_right("foo.bar baz", 0)), (4, 3));
}
