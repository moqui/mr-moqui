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

/// A body that sends its parts with a pause before each.
struct Slow {
    parts: std::collections::VecDeque<(Duration, Vec<u8>)>,
}

impl Read for Slow {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some((pause, bytes)) = self.parts.pop_front() else {
            return Ok(0);
        };
        std::thread::sleep(pause);
        if bytes.is_empty() {
            return Err(std::io::Error::other("reset by peer"));
        }
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

fn slow(parts: &[(u64, &str)]) -> Box<dyn Read + Send> {
    Box::new(Slow {
        parts: parts
            .iter()
            .map(|(ms, text)| (Duration::from_millis(*ms), text.as_bytes().to_vec()))
            .collect(),
    })
}

fn limits(idle_ms: u64, deadline_ms: Option<u64>) -> Limits {
    Limits {
        deadline: deadline_ms.map(|ms| Instant::now() + Duration::from_millis(ms)),
        idle: Duration::from_millis(idle_ms),
        wait: Duration::from_millis(20),
    }
}

#[test]
fn the_body_arrives_whole_and_in_order() {
    let mut got = String::new();
    let end = pump(
        slow(&[(5, "ab"), (5, "cd"), (0, "e")]),
        &limits(1000, None),
        &CancelToken::new(),
        |bytes| {
            got.push_str(std::str::from_utf8(bytes).unwrap());
            true
        },
    );
    assert_eq!((end, got.as_str()), (End::Done, "abcde"));
    let end = pump(
        slow(&[(0, "ab"), (0, "cd")]),
        &limits(1000, None),
        &CancelToken::new(),
        |_| false,
    );
    assert_eq!(end, End::Stopped);
    assert_eq!(
        pump(
            slow(&[(0, "ab"), (0, "")]),
            &limits(1000, None),
            &CancelToken::new(),
            |_| true
        ),
        End::Failed("reset by peer".to_string())
    );
}

#[test]
fn the_wait_for_headers_is_bounded_and_the_kept_value_lives_on_the_reader_thread() {
    struct Flag(Arc<AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let slow: Opener = Box::new(|| {
        std::thread::sleep(Duration::from_millis(400));
        Ok((
            200,
            Vec::new(),
            Box::new(std::io::Cursor::new(Vec::new())) as Box<dyn Read + Send>,
        ))
    });
    let started = Instant::now();
    let ended = open(
        slow,
        Flag(Arc::clone(&dropped)),
        &limits(100, None),
        &CancelToken::new(),
    );
    assert!(matches!(ended, Opened::Ended(End::Idle)));
    assert!(
        started.elapsed() < Duration::from_millis(350),
        "{:?}",
        started.elapsed()
    );
    // the reader thread still holds the kept value until its open returns
    assert!(!dropped.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(600));
    assert!(dropped.load(Ordering::SeqCst));

    let cancel = CancelToken::new();
    cancel.cancel();
    let slow: Opener = Box::new(|| {
        std::thread::sleep(Duration::from_millis(300));
        Err("late".to_string())
    });
    assert!(matches!(
        open(slow, (), &limits(10_000, None), &cancel),
        Opened::Ended(End::Canceled)
    ));
    let failing: Opener = Box::new(|| Err("connection refused".to_string()));
    assert!(
        matches!(open(failing, (), &limits(10_000, None), &CancelToken::new()), Opened::Failed(error) if error == "connection refused")
    );
}

#[test]
fn a_silent_server_does_not_hold_a_cancel_the_deadline_or_the_idle_limit() {
    // the reader blocks for 3 s; each end comes long before that
    let started = Instant::now();
    let cancel = CancelToken::new();
    let canceler = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        canceler.cancel();
    });
    assert_eq!(
        pump(slow(&[(3000, "late")]), &limits(10_000, None), &cancel, |_| true),
        End::Canceled
    );
    assert!(started.elapsed() < Duration::from_millis(1000));

    let started = Instant::now();
    assert_eq!(
        pump(
            slow(&[(3000, "late")]),
            &limits(10_000, Some(80)),
            &CancelToken::new(),
            |_| true
        ),
        End::Deadline
    );
    assert!(started.elapsed() < Duration::from_millis(1000));

    let started = Instant::now();
    // bytes keep the idle limit off until they stop
    let end = pump(
        slow(&[(30, "a"), (30, "b"), (30, "c"), (3000, "late")]),
        &limits(100, None),
        &CancelToken::new(),
        |_| true,
    );
    assert_eq!(end, End::Idle);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(150) && elapsed < Duration::from_millis(1000),
        "{elapsed:?}"
    );
}
