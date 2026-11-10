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

//! A response body a blocking thread writes as it runs (a git job, an event stream, a screen sent as it renders): its
//! chunks come through a bounded channel, which the thread fills within a stall time ([`crate::stall::send_within`]).
//! The body ends when the thread drops its sender; an error chunk ends it abnormally, so the client does not take the
//! part it has for the whole.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use hyper::body::Frame;
use tokio::sync::mpsc;

/// What the thread sends: bytes of the body, or why it ends early.
pub(crate) type Chunk = std::io::Result<Bytes>;

/// The response body: chunks from the thread until it drops its sender.
pub(crate) struct ChannelBody {
    rx: mpsc::Receiver<Chunk>,
    /// An error that came after data, given on the next poll ([`hyper::body::Body::poll_frame`]).
    held: Option<std::io::Error>,
}

impl ChannelBody {
    pub(crate) fn new(rx: mpsc::Receiver<Chunk>) -> Self {
        Self { rx, held: None }
    }
}

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = std::io::Error;

    /// An error is given one poll after it came: hyper drops a connection on a body error without writing out what it
    /// still holds, and a body that waits is flushed first, so the client gets the data sent before the error.
    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        if let Some(err) = self.held.take() {
            return Poll::Ready(Some(Err(err)));
        }
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(Err(err))) => {
                self.held = Some(err);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            other => other.map(|chunk| chunk.map(|chunk| chunk.map(Frame::data))),
        }
    }
}

#[cfg(test)]
#[path = "channel_body_tests.rs"]
mod tests;
