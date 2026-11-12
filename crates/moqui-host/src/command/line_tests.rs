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

use super::{common_prefix, Edit, LineEditor};

fn editor() -> LineEditor {
    LineEditor::new("> ", true, 64, vec!["first".to_string(), "second".to_string()], 3)
}

fn feed(editor: &mut LineEditor, bytes: &[u8]) -> Vec<Edit> {
    editor.feed(bytes, &mut Vec::new())
}

#[test]
fn keys_move_the_cursor_and_edit_the_line() {
    let mut line = editor();
    // type, go home, insert, go to the end, delete a word, then Ctrl+K from the middle
    assert!(feed(&mut line, b"world\x1b[Hhello \x1b[F").is_empty());
    assert_eq!(line.before_cursor(), "hello world");
    feed(&mut line, b"\x17");
    assert_eq!(line.before_cursor(), "hello ");
    feed(&mut line, b"there\x01\x1b[C\x1b[C\x0b");
    assert_eq!(feed(&mut line, b"\r"), vec![Edit::Line("he".to_string())]);
    // Backspace, Ctrl+U, and a CR LF is one line end
    feed(&mut line, b"abc\x7f\x7fx");
    assert_eq!(feed(&mut line, b"\r\n"), vec![Edit::Line("ax".to_string())]);
    feed(&mut line, b"dropped\x15kept");
    assert_eq!(feed(&mut line, b"\n"), vec![Edit::Line("kept".to_string())]);
}

#[test]
fn up_and_down_walk_the_history_and_keep_the_draft() {
    let mut line = editor();
    feed(&mut line, b"draft\x1b[A");
    assert_eq!(line.before_cursor(), "second");
    feed(&mut line, b"\x1b[A\x1b[A");
    assert_eq!(line.before_cursor(), "first", "stops at the oldest");
    feed(&mut line, b"\x1b[B\x1b[B");
    assert_eq!(line.before_cursor(), "draft", "the line being typed comes back");
    feed(&mut line, b"\r");
    // the history keeps its last 3, and not the same line twice in a row
    feed(&mut line, b"draft\r");
    assert_eq!(line.history(), &["first", "second", "draft"]);
    feed(&mut line, b"more\r");
    assert_eq!(line.history(), &["second", "draft", "more"]);
}

#[test]
fn sequences_and_characters_split_across_reads_are_whole() {
    let mut line = editor();
    feed(&mut line, b"ab\x1b");
    feed(&mut line, b"[");
    feed(&mut line, b"D\xc3");
    feed(&mut line, b"\xa9");
    assert_eq!(line.before_cursor(), "aé");
    assert_eq!(feed(&mut line, b"\r"), vec![Edit::Line("aéb".to_string())]);
}

#[test]
fn a_bracketed_paste_is_text_and_its_lines_end_lines() {
    let mut line = editor();
    let edits = feed(&mut line, b"\x1b[200~one\x03\r\ntwo\x1b[201~");
    assert_eq!(
        edits,
        vec![Edit::Line("one".to_string())],
        "a control in a paste is not a key"
    );
    assert_eq!(line.before_cursor(), "two");
}

#[test]
fn ctrl_c_ctrl_d_tab_and_a_line_too_long() {
    let mut line = editor();
    assert_eq!(feed(&mut line, b"x\x03"), vec![Edit::Cancel]);
    assert_eq!(feed(&mut line, b"\x04"), vec![Edit::End]);
    assert_eq!(feed(&mut line, b"se\t"), vec![Edit::Complete]);
    line.insert("rvice ", &mut Vec::new());
    assert_eq!(line.before_cursor(), "service ");
    line.clear();
    feed(&mut line, &[b'a'; 70]);
    assert_eq!(feed(&mut line, b"\r"), vec![Edit::TooLong]);
}

#[test]
fn the_line_is_drawn_again_with_the_cursor_in_place() {
    let mut line = editor();
    let mut out = Vec::new();
    line.feed(b"ab\x1b[D", &mut out);
    assert!(String::from_utf8_lossy(&out).ends_with("\r> ab\x1b[K\x1b[1D"));
    let mut quiet = LineEditor::new("> ", false, 64, Vec::new(), 10);
    let mut out = Vec::new();
    quiet.feed(b"ab\r", &mut out);
    assert!(out.is_empty(), "no echo with no pty");
}

#[test]
fn completion_adds_what_the_choices_share() {
    let choices = ["service".to_string(), "sessions".to_string()];
    assert_eq!(common_prefix(&choices), "se");
    assert_eq!(common_prefix(&[]), "");
}
