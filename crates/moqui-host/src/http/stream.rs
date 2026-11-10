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

//! A screen's response sent as it renders (docs/ui/Screen-Output.md): the request worker's side of the connection.
//!
//! The handler waits for whichever comes first: the head, sent through a oneshot when the render's body passes the
//! threshold, or the whole response, when it never does. After the head the body comes through a bounded channel in
//! chunks, each sent within the write timeout ([`crate::stall::send_within`]), so a client that stops reading ends the
//! render instead of holding its worker. A failure after the head is an error chunk, which ends the body abnormally.

use std::io;
use std::time::Duration;

use bytes::Bytes;
use moqui_core::web::stream::OutputSink;
use moqui_core::web::WebResponse;
use tokio::sync::{mpsc, oneshot};

use crate::channel_body::Chunk;

/// Chunks queued for the client.
pub(crate) const QUEUED: usize = 16;

/// The body is sent in chunks of about this size: large enough for compression to work on, small enough that a slow
/// page shows as it renders.
const CHUNK: usize = 32 * 1024;

/// The sink a request worker writes a screen response to.
pub(crate) struct ScreenSink {
    head: Option<oneshot::Sender<WebResponse>>,
    body: mpsc::Sender<Chunk>,
    pending: Vec<u8>,
    /// The first write after the head goes at once, so the client has the page's start when the response commits.
    send_next: bool,
    stall: Duration,
    ended: bool,
}

impl ScreenSink {
    pub(crate) fn new(head: oneshot::Sender<WebResponse>, body: mpsc::Sender<Chunk>, stall: Duration) -> Self {
        Self {
            head: Some(head),
            body,
            pending: Vec::new(),
            send_next: false,
            stall,
            ended: false,
        }
    }

    fn send(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let bytes = Bytes::from(std::mem::take(&mut self.pending));
        crate::stall::send_within(&self.body, Ok(bytes), self.stall)
    }
}

impl OutputSink for ScreenSink {
    fn head(&mut self, head: WebResponse) -> io::Result<()> {
        let sender = self
            .head
            .take()
            .ok_or_else(|| io::Error::other("the response head was already sent"))?;
        self.send_next = true;
        sender
            .send(head)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the client went away"))
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.pending.extend_from_slice(bytes);
        if self.send_next || self.pending.len() >= CHUNK {
            self.send_next = false;
            self.send()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }

    fn finish(&mut self) {
        if let Err(err) = self.send() {
            tracing::debug!("The end of a response being written was not sent: {err}");
        }
        self.ended = true;
    }

    /// What is held goes first, so the client has the page up to where it stopped and why.
    fn abort(&mut self, reason: &str) {
        if self.send().is_ok() {
            let _ = crate::stall::send_within(&self.body, Err(io::Error::other(reason.to_string())), self.stall);
        }
        self.ended = true;
    }
}

impl Drop for ScreenSink {
    /// A sink whose head went and that was neither finished nor aborted (a panic on the worker) ends its body
    /// abnormally: what the client has is not the whole response.
    fn drop(&mut self) {
        if !self.ended && self.head.is_none() {
            self.abort("the response ended before it was finished");
        }
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
