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

//! The reader-thread pump (plan 4.2): a short-lived thread drains a response body into a bounded channel, and the
//! step thread waits on the channel [`WAIT`] at a time, checking the cancel token, the deadline and the idle limit in
//! between, so a read blocked on a silent server never delays a cancel. The request itself is sent on the reader
//! thread ([`open`]), so the wait for the response headers is bounded the same way. The reader thread ends at the end
//! of the body, at a read error, or at its next read once the step thread has stopped listening.

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the step thread waits for bytes before it checks the cancel token, the deadline and the idle limit.
pub const WAIT: Duration = Duration::from_millis(500);

/// The bytes of one read.
const READ_SIZE: usize = 16 * 1024;

/// Reads in the channel before the reader waits for the step thread.
const CHANNEL_SIZE: usize = 32;

/// A flag a run (or a test) sets to stop a call. Clones share it.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The limits of one pump.
#[derive(Clone, Debug)]
pub struct Limits {
    /// When the whole call must end.
    pub deadline: Option<Instant>,
    /// The longest time without a byte.
    pub idle: Duration,
    /// How long one wait on the channel is.
    pub wait: Duration,
}

impl Limits {
    pub fn new(idle: Duration, deadline: Option<Instant>) -> Self {
        Self {
            deadline,
            idle,
            wait: WAIT,
        }
    }
}

/// How a pump ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The body ended.
    Done,
    /// The handler asked to stop (it saw the end of the stream, or a fatal frame).
    Stopped,
    Canceled,
    /// The deadline passed.
    Deadline,
    /// No byte came within the idle limit.
    Idle,
    /// A read failed.
    Failed(String),
}

enum Chunk {
    Head {
        status: u16,
        headers: Vec<(String, String)>,
    },
    OpenFailed(String),
    Bytes(Vec<u8>),
    End,
    Failed(String),
}

/// What opens a response on the reader thread: its status, headers and body, or why it failed.
pub type Opener = Box<dyn FnOnce() -> Result<(u16, Vec<(String, String)>, Box<dyn Read + Send>), String> + Send>;

/// The body of a response opened by [`open`], read on its reader thread.
pub struct Stream {
    receiver: mpsc::Receiver<Chunk>,
    last_bytes: Instant,
}

/// How [`open`] went.
pub enum Opened {
    Head {
        status: u16,
        headers: Vec<(String, String)>,
        stream: Stream,
    },
    /// The request failed before its response headers.
    Failed(String),
    /// A limit or the cancel token ended the wait for the headers.
    Ended(End),
}

/// Send the request on a reader thread (`opener`), which then drains the body, and wait for the response headers as
/// the body is waited for: [`WAIT`] at a time, checking the cancel token, the deadline and the idle limit. `keep` lives
/// on the reader thread until it ends, so a resource the call holds (an endpoint's slot) is not given back while the
/// server may still be working on the request.
pub fn open<K: Send + 'static>(opener: Opener, keep: K, limits: &Limits, cancel: &CancelToken) -> Opened {
    let (sender, receiver) = mpsc::sync_channel::<Chunk>(CHANNEL_SIZE);
    let spawned = std::thread::Builder::new()
        .name("llm-stream".to_string())
        .spawn(move || {
            let _keep = keep;
            match opener() {
                Ok((status, headers, reader)) => {
                    if sender.send(Chunk::Head { status, headers }).is_ok() {
                        read_into(reader, &sender);
                    }
                }
                Err(error) => {
                    let _ = sender.send(Chunk::OpenFailed(error));
                }
            }
        });
    if let Err(error) = spawned {
        return Opened::Failed(format!("the stream reader thread did not start: {error}"));
    }
    let mut stream = Stream {
        receiver,
        last_bytes: Instant::now(),
    };
    match stream.next(limits, cancel) {
        Ok(Chunk::Head { status, headers }) => Opened::Head {
            status,
            headers,
            stream,
        },
        Ok(Chunk::OpenFailed(error) | Chunk::Failed(error)) => Opened::Failed(error),
        Ok(Chunk::Bytes(_) | Chunk::End) => Opened::Failed("the response had no headers".to_string()),
        Err(end) => Opened::Ended(end),
    }
}

impl Stream {
    /// The next chunk, waiting [`WAIT`] at a time with the checks in between.
    fn next(&mut self, limits: &Limits, cancel: &CancelToken) -> Result<Chunk, End> {
        loop {
            if cancel.is_canceled() {
                return Err(End::Canceled);
            }
            let now = Instant::now();
            if limits.deadline.is_some_and(|deadline| now >= deadline) {
                return Err(End::Deadline);
            }
            let quiet = now.duration_since(self.last_bytes);
            if quiet >= limits.idle {
                return Err(End::Idle);
            }
            // wait no longer than the nearest limit
            let mut wait = limits.wait.min(limits.idle.saturating_sub(quiet));
            if let Some(deadline) = limits.deadline {
                wait = wait.min(deadline.saturating_duration_since(now));
            }
            match self.receiver.recv_timeout(wait.max(Duration::from_millis(1))) {
                Ok(chunk) => {
                    self.last_bytes = Instant::now();
                    return Ok(chunk);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(Chunk::End),
            }
        }
    }

    /// Drain the body, passing each read to `on_bytes` (which returns false to stop), until it ends or a limit, the
    /// cancel token or the handler stops it.
    pub fn pump(mut self, limits: &Limits, cancel: &CancelToken, mut on_bytes: impl FnMut(&[u8]) -> bool) -> End {
        loop {
            match self.next(limits, cancel) {
                Ok(Chunk::Bytes(bytes)) => {
                    if !on_bytes(&bytes) {
                        return End::Stopped;
                    }
                }
                Ok(Chunk::End) => return End::Done,
                Ok(Chunk::Failed(error) | Chunk::OpenFailed(error)) => return End::Failed(error),
                Ok(Chunk::Head { .. }) => {}
                Err(end) => return end,
            }
        }
    }
}

/// Drain `reader` on a reader thread, passing each read to `on_bytes` (which returns false to stop), until the body
/// ends or a limit, the cancel token or the handler stops it.
pub fn pump(
    reader: Box<dyn Read + Send>,
    limits: &Limits,
    cancel: &CancelToken,
    on_bytes: impl FnMut(&[u8]) -> bool,
) -> End {
    match open(Box::new(move || Ok((200, Vec::new(), reader))), (), limits, cancel) {
        Opened::Head { stream, .. } => stream.pump(limits, cancel, on_bytes),
        Opened::Failed(error) => End::Failed(error),
        Opened::Ended(end) => end,
    }
}

fn read_into(mut reader: Box<dyn Read + Send>, sender: &mpsc::SyncSender<Chunk>) {
    let mut buffer = vec![0u8; READ_SIZE];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(Chunk::End);
                return;
            }
            Ok(count) => {
                if sender.send(Chunk::Bytes(buffer[..count].to_vec())).is_err() {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = sender.send(Chunk::Failed(error.to_string()));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
