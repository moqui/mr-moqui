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
#[path = "pump_tests.rs"]
mod tests;
