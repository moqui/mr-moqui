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

//! What a program sends a session as typed (plans/Terminal-Fancy2.md §4.3, `send#TerminalInput`): named keys as the
//! bytes a terminal sends for them, in the program's cursor mode, and text as typed or, with several lines to a program
//! that asked for it, as a bracketed paste.

use super::track::Modes;

/// The most one send may carry.
pub(crate) const MAX_SEND: usize = 64 * 1024;

/// The bytes of the named key `name`: `Enter`, `Tab`, `Esc`, `Up`, `F5`, `C-c` (Ctrl), `M-x` (Alt), `S-Tab`. `None` for
/// a name that is none of these.
pub(crate) fn key(name: &str, modes: &Modes) -> Option<Vec<u8>> {
    if let Some(rest) = name.strip_prefix("M-").or_else(|| name.strip_prefix("A-")) {
        let mut bytes = vec![0x1b];
        bytes.extend(key(rest, modes).or_else(|| single(rest))?);
        return Some(bytes);
    }
    if let Some(rest) = name.strip_prefix("C-") {
        return ctrl(rest);
    }
    // the cursor keys and Home and End: SS3 in application cursor mode, CSI otherwise
    let cursor = |last: u8| -> Vec<u8> {
        if modes.app_cursor {
            vec![0x1b, b'O', last]
        } else {
            vec![0x1b, b'[', last]
        }
    };
    let tilde = |code: &str| format!("\x1b[{code}~").into_bytes();
    let bytes = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "s-tab" | "backtab" => b"\x1b[Z".to_vec(),
        "esc" | "escape" => b"\x1b".to_vec(),
        "backspace" | "bs" => b"\x7f".to_vec(),
        "space" => b" ".to_vec(),
        "up" => cursor(b'A'),
        "down" => cursor(b'B'),
        "right" => cursor(b'C'),
        "left" => cursor(b'D'),
        "home" => cursor(b'H'),
        "end" => cursor(b'F'),
        "insert" => tilde("2"),
        "delete" | "del" => tilde("3"),
        "pageup" | "pgup" => tilde("5"),
        "pagedown" | "pgdn" => tilde("6"),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde("15"),
        "f6" => tilde("17"),
        "f7" => tilde("18"),
        "f8" => tilde("19"),
        "f9" => tilde("20"),
        "f10" => tilde("21"),
        "f11" => tilde("23"),
        "f12" => tilde("24"),
        _ => return None,
    };
    Some(bytes)
}

/// One character as itself, for `M-x`.
fn single(text: &str) -> Option<Vec<u8>> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then(|| first.to_string().into_bytes())
}

/// Ctrl with a letter or one of `@[\]^_ ?`: the control character a terminal sends.
fn ctrl(rest: &str) -> Option<Vec<u8>> {
    let [byte] = rest.as_bytes() else {
        return match rest.to_ascii_lowercase().as_str() {
            "space" => Some(vec![0]),
            _ => None,
        };
    };
    let upper = byte.to_ascii_uppercase();
    match upper {
        b'?' => Some(vec![0x7f]),
        b' ' => Some(vec![0]),
        b'@'..=b'_' => Some(vec![upper - b'@']),
        _ => None,
    }
}

/// `text` as typed: each line end an Enter; with more than one line to a program that asked for bracketed paste
/// (mode 2004), one paste.
pub(crate) fn text(text: &str, modes: &Modes) -> Vec<u8> {
    let typed = text.replace("\r\n", "\r").replace('\n', "\r");
    if modes.bracketed_paste && typed.trim_end_matches('\r').contains('\r') {
        // what a paste must not hold: its own end
        let inside = typed.replace("\x1b[201~", "");
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(inside.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        typed.into_bytes()
    }
}

/// The bytes of a send of `text` then `keys`, or why it is refused: a name that is no key, a NUL, more than
/// [`MAX_SEND`].
pub(crate) fn send(text_in: &str, keys: &[String], modes: &Modes) -> Result<Vec<u8>, String> {
    if text_in.contains('\0') {
        return Err("A terminal's input has no NUL".to_string());
    }
    let mut bytes = text(text_in, modes);
    for name in keys {
        match key(name.trim(), modes) {
            Some(more) => bytes.extend(more),
            None => return Err(format!("No such key: {name}")),
        }
    }
    if bytes.len() > MAX_SEND {
        return Err(format!("A terminal's input is at most {MAX_SEND} bytes at once"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{key, send, text};
    use crate::term::track::Modes;

    #[test]
    fn named_keys_follow_the_cursor_mode() {
        let normal = Modes::default();
        let app = Modes {
            app_cursor: true,
            ..Modes::default()
        };
        assert_eq!(key("Up", &normal).unwrap(), b"\x1b[A");
        assert_eq!(key("Up", &app).unwrap(), b"\x1bOA");
        assert_eq!(key("Enter", &normal).unwrap(), b"\r");
        assert_eq!(key("C-c", &normal).unwrap(), [3]);
        assert_eq!(key("C-[", &normal).unwrap(), [0x1b]);
        assert_eq!(key("M-x", &normal).unwrap(), b"\x1bx");
        assert_eq!(key("M-Left", &normal).unwrap(), b"\x1b\x1b[D");
        assert_eq!(key("F12", &normal).unwrap(), b"\x1b[24~");
        assert_eq!(key("Hyper", &normal), None);
        assert_eq!(key("C-cc", &normal), None);
    }

    #[test]
    fn lines_are_typed_or_pasted_and_a_send_is_bounded() {
        let paste = Modes {
            bracketed_paste: true,
            ..Modes::default()
        };
        assert_eq!(text("ls -l\n", &Modes::default()), b"ls -l\r");
        assert_eq!(text("ls -l\n", &paste), b"ls -l\r", "one line is typed");
        assert_eq!(text("a\nb\n", &paste), b"\x1b[200~a\rb\r\x1b[201~");
        assert_eq!(text("a\x1b[201~b\nc", &paste), b"\x1b[200~ab\rc\x1b[201~");
        assert_eq!(
            send("make", &["Enter".to_string()], &Modes::default()).unwrap(),
            b"make\r"
        );
        assert!(send("a\0b", &[], &Modes::default()).is_err());
        assert!(send("", &["Nope".to_string()], &Modes::default()).is_err());
        assert!(send(&"x".repeat(64 * 1024 + 1), &[], &Modes::default()).is_err());
    }
}
