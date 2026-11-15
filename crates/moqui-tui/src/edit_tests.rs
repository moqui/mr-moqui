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
