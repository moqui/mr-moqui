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

fn keys(bytes: &[u8]) -> Vec<Decoded> {
    Decoder::new().push(bytes)
}

#[test]
fn xterm_and_gnome_function_keys() {
    assert_eq!(keys(b"\x1bOP"), vec![key(Key::F(1))]);
    assert_eq!(keys(b"\x1bOS"), vec![key(Key::F(4))]);
    assert_eq!(keys(b"\x1b[15~"), vec![key(Key::F(5))]);
    assert_eq!(keys(b"\x1b[18~"), vec![key(Key::F(7))]);
    assert_eq!(keys(b"\x1b[24~"), vec![key(Key::F(12))]);
    assert_eq!(keys(b"\x1b[A"), vec![key(Key::Up)]);
}

#[test]
fn kitty_unicode_keys_and_alt() {
    assert_eq!(keys(b"\x1b[13u"), vec![key(Key::Enter)]);
    assert_eq!(keys(b"\x1b[97;3u"), vec![alt(Key::Char('a'))]);
}

#[test]
fn linux_console_function_key() {
    assert_eq!(keys(b"\x1b[[A"), vec![key(Key::F(1))]);
}

#[test]
fn tmux_and_xterm_alt_is_an_escape_prefix() {
    assert_eq!(keys(b"\x1ba"), vec![alt(Key::Char('a'))]);
    assert_eq!(keys(b"\x1b[1;3A"), vec![alt(Key::Up)]);
}

#[test]
fn paste_keeps_a_newline_as_one_event() {
    let decoded = keys(b"\x1b[200~hello\nworld\x1b[201~");
    assert_eq!(decoded, vec![Decoded::Paste("hello\nworld".to_string())]);
}

#[test]
fn a_sequence_split_across_reads_still_parses() {
    let mut decoder = Decoder::new();
    assert!(decoder.push(b"\x1b[1").is_empty());
    assert_eq!(decoder.push(b"5~"), vec![key(Key::F(5))]);
}

#[test]
fn a_lone_escape_waits_then_is_the_escape_key() {
    let mut decoder = Decoder::new();
    assert!(decoder.push(b"\x1b").is_empty());
    assert!(decoder.deadline().is_some());
    assert_eq!(decoder.flush(), vec![key(Key::Escape)]);
    assert_eq!(decoder.deadline(), None);
    // An ESC and `[` in two reads, before the wait ends, are still one sequence.
    assert!(decoder.push(b"\x1b").is_empty());
    assert!(decoder.push(b"[").is_empty());
    assert_eq!(decoder.push(b"B"), vec![key(Key::Down)]);
    assert_eq!(decoder.deadline(), None);
    // A paste waits for its end without a deadline.
    assert!(decoder.push(b"\x1b[200~part").is_empty());
    assert_eq!(decoder.deadline(), None);
}

/// SEC-FIL-99: a paste holds at most MAX_PASTE bytes however much comes, each read searching only its own bytes
/// for the end marker (one split across reads included); past it the rest is dropped and the paste is cut. A paste
/// of MAX_PASTE bytes comes whole.
#[test]
fn a_paste_keeps_at_most_its_limit_and_ends_at_its_marker() {
    let mut decoder = Decoder::new();
    assert!(decoder.push(b"\x1b[200~").is_empty());
    let chunk = vec![b'a'; 4096];
    for _ in 0..(MAX_PASTE / chunk.len()) {
        assert!(decoder.push(&chunk).is_empty());
    }
    assert!(decoder.push(b"\x1b[20").is_empty());
    let whole = decoder.push(b"1~x");
    assert_eq!(whole.len(), 2);
    assert!(matches!(&whole[0], Decoded::Paste(text) if text.len() == MAX_PASTE));
    assert_eq!(whole[1], key(Key::Char('x')));
    assert!(!decoder.take_cut());

    // ten times the limit: what is held stays near the limit, and the paste is its first MAX_PASTE bytes
    assert!(decoder.push(b"\x1b[200~").is_empty());
    for _ in 0..(10 * MAX_PASTE / chunk.len()) {
        assert!(decoder.push(&chunk).is_empty());
        let held = decoder.paste.as_ref().map_or(0, |paste| paste.bytes.len());
        assert!(held < MAX_PASTE + PASTE_END.len(), "{held}");
    }
    let cut = decoder.push(b"b\x1b[201~");
    assert!(matches!(&cut[..], [Decoded::Paste(text)] if text.len() == MAX_PASTE && !text.contains('b')));
    assert!(decoder.take_cut());
    assert!(!decoder.take_cut());
    assert_eq!(decoder.push(b"y"), vec![key(Key::Char('y'))]);

    // a few bytes past the limit, held with the marker's room, are a cut paste too; a cut does not split a character
    let mut decoder = Decoder::new();
    let mut over = b"\x1b[200~".to_vec();
    over.extend(std::iter::repeat_n(b'a', MAX_PASTE - 1));
    over.extend_from_slice("é".as_bytes());
    over.extend_from_slice(b"zz\x1b[201~");
    let decoded = decoder.push(&over);
    assert!(
        matches!(&decoded[..], [Decoded::Paste(text)] if text.len() == MAX_PASTE - 1 && !text.contains('\u{fffd}')),
        "{:?}",
        decoded
            .iter()
            .map(|d| matches!(d, Decoded::Paste(_)))
            .collect::<Vec<_>>()
    );
    assert!(decoder.take_cut());
}

#[test]
fn control_letters_alt_enter_and_home_end() {
    assert_eq!(keys(b"\x0b"), vec![ctrl(Key::Char('k'))]);
    assert_eq!(keys(b"\x13"), vec![ctrl(Key::Char('s'))]);
    assert_eq!(keys(b"\t"), vec![key(Key::Tab)]);
    assert_eq!(keys(b"\x1b\r"), vec![alt(Key::Enter)]);
    assert_eq!(keys(b"\x1b[1~\x1b[4~"), vec![key(Key::Home), key(Key::End)]);
    assert_eq!(keys(b"\x1bOH"), vec![key(Key::Home)]);
}

#[test]
fn function_keys_one_to_four_take_their_modifiers() {
    let shifted = |key: Key| KeyPress {
        shift: true,
        ..KeyPress::plain(key)
    };
    assert_eq!(keys(b"\x1b[1;2R"), vec![Decoded::Key(shifted(Key::F(3)))]);
    assert_eq!(keys(b"\x1b[13;2~"), vec![Decoded::Key(shifted(Key::F(3)))]);
    assert_eq!(keys(b"\x1b[19;2~"), vec![Decoded::Key(shifted(Key::F(8)))]);
    assert_eq!(keys(b"\x1bOR"), vec![key(Key::F(3))]);
    assert_eq!(keys(b"\x00"), vec![ctrl(Key::Char(' '))]);
}

#[test]
fn resize_report_is_columns_then_rows() {
    assert_eq!(
        keys(b"\x1b[8;50;180t"),
        vec![Decoded::Resize { columns: 180, rows: 50 }]
    );
}

#[test]
fn the_background_answer_is_read_and_other_commands_dropped() {
    assert_eq!(
        keys(b"\x1b]11;rgb:0000/0000/0000\x07"),
        vec![Decoded::Background { dark: true }]
    );
    assert_eq!(
        keys(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\x"),
        vec![Decoded::Background { dark: false }, key(Key::Char('x'))]
    );
    let mut decoder = Decoder::new();
    assert!(decoder.push(b"\x1b]11;rgb:fa").is_empty(), "a split answer waits");
    assert_eq!(
        decoder.push(b"fa/fafa/fafa\x07"),
        vec![Decoded::Background { dark: false }]
    );
    assert_eq!(keys(b"\x1b]52;c;aGk=\x07z"), vec![key(Key::Char('z'))]);
    assert_eq!(keys(b"\x1b]a"), vec![alt(Key::Char(']')), key(Key::Char('a'))]);
}

fn mouse(kind: MouseKind, column: u16, row: u16) -> Decoded {
    Decoded::Mouse(Mouse::at(kind, column, row))
}

#[test]
fn sgr_mouse_press_drag_release_and_wheel() {
    assert_eq!(keys(b"\x1b[<0;10;5M"), vec![mouse(MouseKind::Press, 9, 4)]);
    assert_eq!(keys(b"\x1b[<32;11;5M"), vec![mouse(MouseKind::Drag, 10, 4)]);
    assert_eq!(keys(b"\x1b[<0;11;5m"), vec![mouse(MouseKind::Release, 10, 4)]);
    assert_eq!(keys(b"\x1b[<64;1;1M"), vec![mouse(MouseKind::WheelUp, 0, 0)]);
    assert_eq!(keys(b"\x1b[<65;1;1M"), vec![mouse(MouseKind::WheelDown, 0, 0)]);
    // Shift 4, Alt 8, Ctrl 16; the right button is 2.
    let Decoded::Mouse(with) = keys(b"\x1b[<30;3;4M").remove(0) else {
        panic!("a mouse report");
    };
    assert!(with.shift && with.alt && with.ctrl && with.button == 2, "{with:?}");
    // Motion with no button is not reported, and the keys after it still are.
    assert_eq!(keys(b"\x1b[<35;3;4Mx"), vec![key(Key::Char('x'))]);
    // SGR has no coordinate limit, unlike X10's 223.
    assert_eq!(keys(b"\x1b[<0;300;80M"), vec![mouse(MouseKind::Press, 299, 79)]);
}

#[test]
fn x10_mouse_and_reports_split_across_reads() {
    // Button 0 at column 1, row 1: 32, 33, 33. A release is code 3.
    assert_eq!(keys(b"\x1b[M !!"), vec![mouse(MouseKind::Press, 0, 0)]);
    assert_eq!(keys(b"\x1b[M#*%"), vec![mouse(MouseKind::Release, 9, 4)]);
    let mut decoder = Decoder::new();
    assert!(decoder.push(b"\x1b[M ").is_empty());
    assert_eq!(decoder.push(b"!!"), vec![mouse(MouseKind::Press, 0, 0)]);
    assert!(decoder.push(b"\x1b[<0;1").is_empty());
    assert_eq!(decoder.push(b"0;5M"), vec![mouse(MouseKind::Press, 9, 4)]);
}
