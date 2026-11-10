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

//! A response sent while it is written, as a servlet's response commits when its buffer fills
//! (docs/ui/Screen-Output.md).
//!
//! The host offers a sink for a request ([`offer`]); a screen render or a web script arms it with the threshold
//! ([`arm`]). Until the body reaches the threshold it is kept in the slot as any response, so a small page is sent
//! whole, with its error page, its 304 and the headers set late. When it reaches the threshold the response commits:
//! the head hooks ([`on_head`]) finish the status and headers (the session cookie and token, the conf headers), the
//! sink takes the head and what is buffered, and each later write goes to the sink. After the commit a header set is
//! dropped with a warning, and a failure can only end the body abnormally ([`abort`]).
//!
//! The slot is the thread's, so all of this runs on the request worker; the sink is what crosses to the connection.

use super::{WebResponse, WebSlot, WEB};
use crate::context::Error;

/// Where a committed response goes: the host's connection. Every call is on the request worker.
pub trait OutputSink {
    /// The status and headers, once, before any of the body; `head.body` is empty.
    ///
    /// # Errors
    ///
    /// Returns an error when the client is gone.
    fn head(&mut self, head: WebResponse) -> std::io::Result<()>;

    /// Bytes of the body, in order.
    ///
    /// # Errors
    ///
    /// Returns an error when the client is gone or took nothing for the write timeout.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()>;

    /// Send what is held now ([`flush`], a web script's `flushBuffer`).
    ///
    /// # Errors
    ///
    /// As [`OutputSink::write`].
    fn flush(&mut self) -> std::io::Result<()>;

    /// The body is complete.
    fn finish(&mut self);

    /// End the body abnormally, so a client does not take the part it has as the whole response.
    fn abort(&mut self, reason: &str);
}

/// Finishes the head of a response that commits while it is written: what would otherwise be added once the response
/// is whole (the session cookie and token, the conf headers). Hooks run last registered first, the render's before the
/// request's.
pub(crate) type HeadHook = Box<dyn FnOnce(&mut WebResponse) -> Result<(), Error>>;

/// The streamed response's state in the web slot.
pub(super) struct Streaming {
    sink: Box<dyn OutputSink>,
    /// The body kept before the response commits; 0 while not armed, when the response is kept whole.
    threshold: usize,
    committing: bool,
    committed: bool,
    failed: bool,
    hooks: Vec<HeadHook>,
}

impl Streaming {
    pub(super) fn is_committed(&self) -> bool {
        self.committed
    }
}

/// What a write did.
pub(super) enum Written {
    /// Kept or sent; false when the response failed (the client went away) and the bytes were dropped.
    Done(bool),
    /// The body reached the threshold: the response commits now.
    Commit,
}

/// Give this request's response a sink, for a render or a script to arm. Nothing without a web slot.
pub fn offer(sink: Box<dyn OutputSink>) {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            web.streaming = Some(Streaming {
                sink,
                threshold: 0,
                committing: false,
                committed: false,
                failed: false,
                hooks: Vec::new(),
            });
        }
    });
}

/// Keep this response whole: a path whose filters see the whole response, or a servlet whose output another step
/// reads (a PDF). Nothing once it committed.
pub(crate) fn withdraw() {
    WEB.with(|slot| {
        if let Some(web) = slot.borrow_mut().as_mut() {
            if web.streaming.as_ref().is_some_and(|streaming| !streaming.committed) {
                web.streaming = None;
            }
        }
    });
}

/// Let the response commit once its body reaches `threshold` bytes. False when there is no sink (no socket, a filter
/// on the path, a render inside another) or `threshold` is 0.
pub(crate) fn arm(threshold: usize) -> bool {
    WEB.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(streaming) = borrowed.as_mut().and_then(|web| web.streaming.as_mut()) else {
            return false;
        };
        if threshold == 0 || streaming.committed {
            return streaming.committed;
        }
        streaming.threshold = threshold;
        true
    })
}

/// Add a hook that finishes the head if the response commits. Nothing when the response has no sink.
pub(crate) fn on_head(hook: HeadHook) {
    WEB.with(|slot| {
        if let Some(streaming) = slot.borrow_mut().as_mut().and_then(|web| web.streaming.as_mut()) {
            streaming.hooks.push(hook);
        }
    });
}

/// Whether the response's head went to the client.
pub(crate) fn committed() -> bool {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|web| web.streaming.as_ref())
            .is_some_and(|streaming| streaming.committed)
    })
}

/// Whether a committed response could not be sent (the client went away, or a hook failed): later writes are dropped.
pub(crate) fn failed() -> bool {
    WEB.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|web| web.streaming.as_ref())
            .is_some_and(|streaming| streaming.failed)
    })
}

/// Append `bytes` to the body of `web`, or send them when it committed.
pub(super) fn write(web: &mut WebSlot, bytes: &[u8]) -> Written {
    match web.streaming.as_mut() {
        Some(streaming) if streaming.committed => {
            if streaming.failed {
                return Written::Done(false);
            }
            if let Err(err) = streaming.sink.write(bytes) {
                streaming.failed = true;
                note_failure(&err);
            }
            Written::Done(!streaming.failed)
        }
        Some(streaming) if streaming.threshold > 0 && !streaming.committing => {
            web.body.extend_from_slice(bytes);
            if web.body.len() >= streaming.threshold {
                Written::Commit
            } else {
                Written::Done(true)
            }
        }
        _ => {
            web.body.extend_from_slice(bytes);
            Written::Done(true)
        }
    }
}

/// Commit now what is written so far, when the response is armed and something was written: a web script's
/// `flushBuffer`. A committed response sends what the sink holds.
pub(crate) fn flush() {
    let commit = WEB.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(web) = borrowed.as_mut() else {
            return false;
        };
        let sent = web.sent;
        let Some(streaming) = web.streaming.as_mut() else {
            return false;
        };
        if streaming.committed {
            if !streaming.failed {
                if let Err(err) = streaming.sink.flush() {
                    streaming.failed = true;
                    note_failure(&err);
                }
            }
            return false;
        }
        sent && streaming.threshold > 0
    });
    if commit {
        self::commit();
    }
}

/// Send the head and the body so far. A response that names its version (an `ETag`) stays whole, so it can still be
/// answered 304; one whose head hooks fail is not sent, and the render goes on with nowhere to write.
pub(super) fn commit() {
    let taken = WEB.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let web = borrowed.as_mut()?;
        let streaming = web.streaming.as_mut()?;
        if streaming.committed || streaming.committing || streaming.threshold == 0 {
            return None;
        }
        if web.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("etag")) {
            streaming.threshold = 0;
            return None;
        }
        streaming.committing = true;
        let hooks = std::mem::take(&mut streaming.hooks);
        let head = WebResponse {
            status: web.status,
            headers: web.headers.clone(),
            body: Vec::new(),
            stream: None,
            http_log: super::HttpLogIdentity::default(),
        };
        Some((hooks, head))
    });
    let Some((hooks, mut head)) = taken else {
        return;
    };
    // the hooks read and write the slot and the session, so it is not borrowed while they run
    let mut finished = Ok(());
    for hook in hooks.into_iter().rev() {
        finished = hook(&mut head);
        if finished.is_err() {
            break;
        }
    }
    WEB.with(|slot| {
        let mut borrowed = slot.borrow_mut();
        let Some(web) = borrowed.as_mut() else {
            return;
        };
        let body = std::mem::take(&mut web.body);
        let Some(streaming) = web.streaming.as_mut() else {
            return;
        };
        streaming.committing = false;
        streaming.committed = true;
        if let Err(err) = finished {
            tracing::error!("A response could not be sent as it was written; its head failed: {err}");
            streaming.failed = true;
            streaming.sink.abort(&format!("the response head failed: {err}"));
            return;
        }
        let sent = streaming.sink.head(head).and_then(|()| streaming.sink.write(&body));
        if let Err(err) = sent {
            streaming.failed = true;
            note_failure(&err);
        }
    });
}

/// End a committed response abnormally: a failure after its head went to the client.
pub(crate) fn abort(reason: &str) {
    WEB.with(|slot| {
        if let Some(streaming) = slot.borrow_mut().as_mut().and_then(|web| web.streaming.as_mut()) {
            if streaming.committed && !streaming.failed {
                streaming.failed = true;
                streaming.sink.abort(reason);
            }
        }
    });
}

/// End the response's body: a committed one gets what is held; an uncommitted one is sent whole by the caller, and the
/// sink goes with no head.
pub(crate) fn finish() {
    let streaming = WEB.with(|slot| slot.borrow_mut().as_mut().and_then(|web| web.streaming.take()));
    if let Some(mut streaming) = streaming {
        if streaming.committed && !streaming.failed {
            streaming.sink.finish();
        }
    }
}

/// A client that went away is not the server's error; one that stopped reading is logged as such.
fn note_failure(err: &std::io::Error) {
    if err.kind() == std::io::ErrorKind::TimedOut {
        tracing::warn!("A response being written stopped: {err}");
    } else {
        tracing::debug!("A response being written stopped: {err}");
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
