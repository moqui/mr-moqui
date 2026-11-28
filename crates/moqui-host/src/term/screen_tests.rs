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

use super::{Scope, Screen};

#[test]
fn a_sequence_split_across_reads_is_read_whole() {
    let mut screen = Screen::new(20, 4, 100, b"", false);
    screen.feed(b"red \x1b[3");
    screen.feed(b"1mword\x1b[0m done\r\nnext");
    let read = screen.read(Scope::Visible, false);
    assert_eq!(read.lines, ["red word done", "next", "", ""]);
    assert_eq!(read.cursor, (1, 4));
    assert_eq!((read.cols, read.rows), (20, 4));
    assert!(!read.rebuilt);
    // a character split across reads
    screen.feed("\r\né".as_bytes()[..3].as_ref());
    screen.feed(&"é".as_bytes()[1..]);
    assert_eq!(screen.read(Scope::Visible, false).lines[2], "é");
}

#[test]
fn the_scrollback_and_the_last_lines_read_in_order() {
    let mut screen = Screen::new(20, 3, 100, b"", false);
    for n in 1..=8 {
        screen.feed(format!("line {n}\r\n").as_bytes());
    }
    let all = screen.read(Scope::Scrollback, false).lines;
    let expected: Vec<String> = (1..=8).map(|n| format!("line {n}")).chain([String::new()]).collect();
    assert_eq!(all, expected);
    // the last lines leave out the empty rows below the cursor
    assert_eq!(screen.read(Scope::Last(3), false).lines, ["line 7", "line 8", ""]);
    assert_eq!(screen.read(Scope::Last(1), false).lines, [""]);
    screen.feed(b"$ ");
    assert_eq!(screen.read(Scope::Last(2), false).lines, ["line 8", "$"]);
    // the screen is in view again after a read of the scrollback
    assert_eq!(screen.read(Scope::Visible, false).lines, ["line 7", "line 8", "$"]);
}

#[test]
fn a_resize_and_the_alternate_screen_are_followed() {
    let mut screen = Screen::new(10, 3, 100, b"prompt$ ", true);
    assert!(screen.read(Scope::Visible, false).rebuilt, "made from the kept output");
    screen.resize(30, 5);
    let read = screen.read(Scope::Visible, false);
    assert_eq!((read.cols, read.rows), (30, 5));
    assert!(!read.rebuilt, "a program draws again on a change of size");
    screen.feed(b"\x1b[?1049h\x1b[Hfull screen");
    let read = screen.read(Scope::Visible, true);
    assert!(read.alternate);
    assert_eq!(read.lines[0], "full screen");
    assert!(read.ansi.contains("full screen"), "{:?}", read.ansi);
    screen.feed(b"\x1b[?1049l");
    let read = screen.read(Scope::Visible, false);
    assert!(!read.alternate);
    assert_eq!(read.lines[0], "prompt$");
}

#[test]
fn a_rebuilt_screen_is_whole_again_once_cleared() {
    let mut screen = Screen::new(20, 3, 100, b"half", true);
    screen.feed(b"more");
    assert!(screen.read(Scope::Visible, false).rebuilt);
    screen.feed(b"\x1b[H\x1b[2Jdrawn");
    assert!(!screen.read(Scope::Visible, false).rebuilt);
    assert_eq!(Scope::parse("last", 0), Some(Scope::Last(1)));
    assert_eq!(Scope::parse("sideways", 3), None);
}
