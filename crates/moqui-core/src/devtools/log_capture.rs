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

//! The log lines of one run on this thread (plans/design-dev-aide.md 9.4: a Try in sim answers its log): the host's
//! log subscriber hands each line it writes to [`note`], kept only while [`with`] runs on the line's thread, at most
//! [`MAX_LINES`] of them.

use std::cell::RefCell;

/// The most lines a capture keeps; the later ones are counted, not kept.
pub const MAX_LINES: usize = 200;

thread_local! {
    static LINES: RefCell<Option<(Vec<String>, usize)>> = const { RefCell::new(None) };
}

/// Run `body` keeping the log lines written on this thread meanwhile; answers its result, the lines and how many more
/// were left out.
pub fn with<T>(body: impl FnOnce() -> T) -> (T, Vec<String>, usize) {
    let before = LINES.with(|lines| lines.borrow_mut().replace((Vec::new(), 0)));
    let result = body();
    let (kept, more) = LINES
        .with(|lines| std::mem::replace(&mut *lines.borrow_mut(), before))
        .unwrap_or_default();
    (result, kept, more)
}

/// A line the log wrote on this thread: kept when a capture runs here.
pub fn note(line: &str) {
    LINES.with(|lines| {
        if let Ok(mut lines) = lines.try_borrow_mut() {
            if let Some((kept, more)) = lines.as_mut() {
                if kept.len() < MAX_LINES {
                    kept.push(line.to_string());
                } else {
                    *more += 1;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_keeps_the_lines_of_its_thread_only_while_it_runs() {
        note("before");
        let ((), lines, more) = with(|| {
            note("one");
            std::thread::spawn(|| note("elsewhere")).join().unwrap();
            for _ in 0..MAX_LINES {
                note("many");
            }
        });
        note("after");
        assert_eq!(lines[0], "one");
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(more, 1);
    }
}
