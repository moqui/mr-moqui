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

//! Blocking streams over channels, for the git server (`moqui_git::server`), which speaks the
//! protocol with blocking `Read` and `Write` on a job of its own pool, while the HTTP connection or SSH channel is an
//! async task here. A bounded channel each way gives back pressure: a slow client stalls the pack writer rather than
//! filling memory. A side that goes away ends the other: a closed channel is the end of the input, or a broken pipe
//! for the output.

use std::io::{self, Read, Write};
use std::time::Duration;

use bytes::{Buf, Bytes};
use tokio::sync::mpsc;

/// Chunks queued each way.
pub(crate) const QUEUED: usize = 16;

/// The output is sent in chunks of this size.
const CHUNK: usize = 64 * 1024;

/// What the async side feeds the job: data, or why the input failed (too large, too slow).
pub(crate) type Chunk = io::Result<Bytes>;

/// The job's input.
pub(crate) struct ChannelReader {
    rx: mpsc::Receiver<Chunk>,
    current: Bytes,
}

impl ChannelReader {
    pub(crate) fn new(rx: mpsc::Receiver<Chunk>) -> Self {
        Self {
            rx,
            current: Bytes::new(),
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.current.is_empty() {
            match self.rx.blocking_recv() {
                Some(Ok(bytes)) => self.current = bytes,
                Some(Err(err)) => return Err(err),
                None => return Ok(0),
            }
        }
        let count = buf.len().min(self.current.len());
        buf[..count].copy_from_slice(&self.current[..count]);
        self.current.advance(count);
        Ok(count)
    }
}

/// The conf's `git-server` `idle-timeout` in seconds, else 120: how long a clone or push may send nothing, or take
/// nothing of its output.
pub(crate) fn idle_timeout(factory: &moqui_core::context::ExecutionContextFactory) -> Duration {
    Some(factory.conf_number("git-server", "idle-timeout", 0u64))
        .filter(|seconds| *seconds > 0)
        .map_or(DEFAULT_IDLE, Duration::from_secs)
}

const DEFAULT_IDLE: Duration = Duration::from_secs(120);

/// The job's output, gathered into chunks; [`Write::flush`] sends what is gathered. A send waits for the client at
/// most `stall` ([`crate::stall::send_within`]).
pub(crate) struct ChannelWriter {
    tx: mpsc::Sender<Chunk>,
    pending: Vec<u8>,
    stall: Duration,
}

impl ChannelWriter {
    pub(crate) fn new(tx: mpsc::Sender<Chunk>, stall: Duration) -> Self {
        Self {
            tx,
            pending: Vec::with_capacity(CHUNK),
            stall,
        }
    }

    fn send(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let bytes = Bytes::from(std::mem::replace(&mut self.pending, Vec::with_capacity(CHUNK)));
        crate::stall::send_within(&self.tx, Ok(bytes), self.stall)
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let room = CHUNK - self.pending.len();
        let taken = room.min(buf.len());
        self.pending.extend_from_slice(&buf[..taken]);
        if self.pending.len() == CHUNK {
            self.send()?;
        }
        Ok(taken)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

impl Drop for ChannelWriter {
    fn drop(&mut self) {
        let _ = self.send();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn bytes_cross_in_order_and_a_closed_side_ends_the_other() {
        let (in_tx, in_rx) = mpsc::channel(QUEUED);
        let (out_tx, mut out_rx) = mpsc::channel(QUEUED);
        let job = std::thread::spawn(move || {
            let mut reader = ChannelReader::new(in_rx);
            let mut text = String::new();
            reader.read_to_string(&mut text).unwrap();
            let mut writer = ChannelWriter::new(out_tx, Duration::from_secs(5));
            writer.write_all(text.to_uppercase().as_bytes()).unwrap();
        });
        for part in ["one ", "two"] {
            in_tx.blocking_send(Ok(Bytes::from(part))).unwrap();
        }
        drop(in_tx);
        job.join().unwrap();
        let mut got = Vec::new();
        while let Some(chunk) = out_rx.blocking_recv() {
            got.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(got, b"ONE TWO");

        let (out_tx, out_rx) = mpsc::channel(1);
        drop(out_rx);
        let mut writer = ChannelWriter::new(out_tx, Duration::from_secs(5));
        writer.write_all(b"x").unwrap();
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    /// A client that stops reading fails the job's write after the stall time, so the git worker is free again.
    #[test]
    fn a_client_that_takes_nothing_fails_the_write_after_the_stall_time() {
        let (out_tx, _out_rx) = mpsc::channel(1);
        let job = std::thread::spawn(move || {
            let mut writer = ChannelWriter::new(out_tx, Duration::from_millis(200));
            let started = Instant::now();
            let failed = (0..4).find_map(|_| writer.write_all(&[7u8; CHUNK]).err());
            (failed.map(|err| err.kind()), started.elapsed())
        });
        let (failed, took) = job.join().unwrap();
        assert_eq!(failed, Some(io::ErrorKind::TimedOut));
        assert!(
            took >= Duration::from_millis(200) && took < Duration::from_secs(3),
            "{took:?}"
        );
    }
}
