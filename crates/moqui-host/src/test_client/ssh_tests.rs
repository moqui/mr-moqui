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

use super::*;

fn regex(source: &str) -> regex::Regex {
    regex::Regex::new(source).unwrap()
}

#[test]
fn a_screen_takes_out_escapes_and_breaks_lines_at_cursor_moves() {
    let mut screen = Screen::default();
    // a move, a style, a title (OSC to BEL), a charset, a carriage return
    screen.feed(b"\x1b[2J\x1b[1;1HFind \x1b[1;32mOrders\x1b[0m\r\n\x1b]0;moqui\x07\x1b(BRow 1\x1b[3;1HRow 2");
    let found = screen.take_match(&regex(r"Find Orders\n")).unwrap();
    assert_eq!(found["match"], "Find Orders\n");
    assert_eq!(found["before"], "\n");
    let found = screen.take_match(&regex(r"Row (\d)\nRow (\d)")).unwrap();
    assert_eq!(found["groups"], json!(["1", "2"]));
    // what matched is read: the same text does not match again
    assert!(screen.take_match(&regex("Row")).is_none());
}

#[test]
fn a_sequence_or_a_character_split_across_reads_is_whole() {
    let mut screen = Screen::default();
    screen.feed(b"Total: \x1b[");
    screen.feed(b"1mcaf\xc3");
    assert!(screen.take_match(&regex("café")).is_none());
    screen.feed(b"\xa9\x1b]2;title\x1b");
    screen.feed(b"\\ done");
    let found = screen.take_match(&regex(r"Total: café done")).unwrap();
    assert_eq!(found["before"], "");
    // an OSC ended by ESC \ leaves no backslash, and a bad byte reads as U+FFFD
    screen.feed(b"a\xffb");
    assert!(screen.take_match(&regex("a\u{fffd}b")).is_some());
}

#[test]
fn a_screen_keeps_at_most_its_cap_of_unmatched_text() {
    let mut screen = Screen::default();
    let chunk = "x".repeat(64 * 1024);
    for _ in 0..40 {
        screen.feed(chunk.as_bytes());
    }
    screen.feed(b"END");
    assert!(screen.text.len() <= MAX_TEXT);
    assert!(screen.take_match(&regex("x+END")).is_some());
    assert_eq!(screen.tail(3), "");
}

#[test]
fn expect_reads_a_literal_or_a_pattern_with_its_flags() {
    let literal = pattern(&json!({ "literal": "a.b (c)" })).unwrap();
    assert!(literal.is_match("a.b (c)"));
    assert!(!literal.is_match("axb (c)"));
    let insensitive = pattern(&json!({ "source": "^find\\s+orders$", "flags": "im" })).unwrap();
    assert!(insensitive.is_match("top\nFind  Orders\nend"));
    assert!(pattern(&json!({})).is_err());
    // a JavaScript lookahead is not in this harness's patterns: the error says so
    let err = pattern(&json!({ "source": "a(?=b)" })).unwrap_err();
    assert!(err.contains("not a pattern this harness reads"), "{err}");
}

#[test]
fn an_exit_status_in_expect_is_its_label() {
    assert_eq!(
        labels(&json!({ "expect": [1, "2", "timeout", "exit 3"] })),
        vec!["exit 1", "exit 2", "timeout", "exit 3"]
    );
    assert!(labels(&json!({})).is_empty());
}
