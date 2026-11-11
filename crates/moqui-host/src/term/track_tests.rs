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

use super::{Modes, Tracker};

#[test]
fn modes_are_read_across_reads_and_set_again_by_the_preamble() {
    let mut tracker = Tracker::default();
    tracker.feed(b"\x1b[?10");
    tracker.feed(b"49h\x1b[?2004h\x1b[?25l");
    tracker.feed(b"\x1b[?1002;1006h\x1b=");
    let modes = tracker.modes();
    assert_eq!(modes.alternate, Some(1049));
    assert!(modes.bracketed_paste && modes.cursor_hidden && modes.mouse_sgr && modes.app_keypad);
    assert_eq!(modes.mouse, Some(1002));
    assert_eq!(
        modes.preamble(),
        b"\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?25l\x1b=".to_vec()
    );
    tracker.feed(b"\x1b[?1049l\x1b[?1002l\x1b>");
    assert_eq!(tracker.modes().alternate, None);
    assert_eq!(tracker.modes().mouse, None);
    tracker.feed(b"\x1bc");
    assert_eq!(tracker.modes(), Modes::default(), "a full reset");
}

#[test]
fn the_title_directory_bell_and_notifications_are_read() {
    let mut tracker = Tracker::default();
    let changes = tracker.feed(b"\x1b]0;vim main.rs\x07");
    assert_eq!(changes.title.as_deref(), Some("vim main.rs"));
    // split, and ended by ST
    tracker.feed(b"\x1b]2;bu");
    let changes = tracker.feed(b"ild\x1b\\");
    assert_eq!(changes.title.as_deref(), Some("build"));
    let changes = tracker.feed(b"\x1b]7;file://host/home/dev/my%20project\x1b\\");
    assert_eq!(changes.cwd.as_deref(), Some("/home/dev/my project"));
    let changes = tracker.feed(b"done\x07");
    assert!(changes.bell);
    let changes = tracker.feed(b"\x1b]777;notify;claude;needs your input\x07");
    assert_eq!(
        changes.notify,
        Some(("claude".to_string(), "needs your input".to_string()))
    );
    let changes = tracker.feed(b"\x1b]9;tests finished\x07");
    assert_eq!(changes.notify, Some((String::new(), "tests finished".to_string())));
    assert!(!changes.bell, "a BEL that ends an OSC is no bell");
}

#[test]
fn progress_marks_and_a_programs_status_are_read() {
    let mut tracker = Tracker::default();
    assert_eq!(tracker.feed(b"\x1b]9;4;1;42\x07").progress, Some(Some(42)));
    assert_eq!(tracker.feed(b"\x1b]9;4;0;0\x07").progress, Some(None));
    let changes = tracker.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
    assert_eq!(changes.marks, vec!['A', 'B']);
    assert_eq!(changes.status.as_deref(), Some("idle"));
    assert_eq!(tracker.feed(b"\x1b]133;C\x07").status.as_deref(), Some("working"));
    let changes = tracker.feed(b"\x1b]133;D;2\x07");
    assert_eq!(
        (changes.status.as_deref(), changes.exit_status),
        (Some("error"), Some(2))
    );
    let changes = tracker.feed(b"\x1b]7501;state=blocked;kind=permission\x07");
    assert_eq!(changes.status.as_deref(), Some("blocked"));
    assert_eq!(changes.status_text.as_deref(), Some("permission"));
    assert_eq!(
        tracker.feed(b"\x1b]7501;working;progress=10\x07").progress,
        Some(Some(10))
    );
}

#[test]
fn rests_are_at_line_ends_outside_sequences_and_characters() {
    let mut tracker = Tracker::default();
    let changes = tracker.feed(b"a\nb\x1b]0;x\ny\x07c\n");
    // the newline inside the OSC is no rest
    assert_eq!(changes.rests.iter().map(|(at, _)| *at).collect::<Vec<_>>(), vec![2, 13]);
    // a long run with no line end has a rest each 16 KiB, never inside a UTF-8 character
    let long = "é".repeat(20_000);
    let changes = tracker.feed(long.as_bytes());
    assert!(!changes.rests.is_empty());
    for (at, _) in changes.rests {
        assert!(long.is_char_boundary(at), "{at}");
    }
}
