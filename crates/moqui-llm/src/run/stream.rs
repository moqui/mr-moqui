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

//! The replay-then-follow of a run's events that every reader of them shares (plans/archive/High-Level-Review.md HL-014):
//! the Assist stream (`moqui-host` `llm/sse.rs`), A2A streams and resubscribes, the blocking `message:send` wait and
//! push ([`crate::a2a::ops::follow`]).
//!
//! [`replay_then_follow`] sends the stored events after the reader's last, page by page, then holds a watch on the
//! run and follows its hub ring ([`super::hub::Follower`], which fills a gap from the database), checking every
//! [`POLL`] whether to stop, keeping the connection alive after [`PING_EVERY`] with nothing written, and every
//! [`CHECK_EVERY`] whether the run is settled for the reader on another node, when it sends what is stored and ends.
//! What a reader sends of each event (its projection, what it does with a live-only event), how it writes, its
//! authentication, its deadline and whether a pause ends it are the reader's ([`Reader`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;
use serde_json::Value as Json;

use super::{engine, events, hub};

/// Stored events read at once.
pub const PAGE: i64 = 500;
/// A keep-alive goes out after this long with nothing written.
pub const PING_EVERY: Duration = Duration::from_secs(15);
/// How often the run is read to see whether it ended or paused on another node.
pub const CHECK_EVERY: Duration = Duration::from_secs(5);
/// The longest a follow waits on the hub before it checks again whether to stop (the server stops, the client left,
/// a deadline passed).
pub const POLL: Duration = Duration::from_secs(1);

/// What a reader did with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Nothing was written; go on.
    Next,
    /// Something was written; go on.
    Wrote,
    /// The stream is over (after what was written).
    End,
    /// The client is gone.
    Gone,
}

/// How a follow ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The reader's end: an event it ended at, or the run settled for it.
    Over,
    /// The client is gone.
    Gone,
    /// The reader stopped it (the server stops, a closed client noticed by its check, a deadline).
    Stopped,
}

/// One reader of a run's events.
pub trait Reader {
    /// One event: a stored one (`seq`) or a live-only one (`None`).
    ///
    /// # Errors
    ///
    /// The reader's error ends the follow with it.
    fn event(
        &mut self,
        facade: &EntityFacade,
        seq: Option<i64>,
        event_type: &str,
        payload: &Json,
    ) -> Result<Flow, Error>;

    /// Nothing written for [`PING_EVERY`]: a keep-alive. False when the client is gone. A waiter writes nothing.
    fn keep_alive(&mut self) -> bool {
        true
    }

    /// Whether to stop now: the server stops, the client closed, the reader's deadline passed. Checked before each
    /// wait on the hub, so at least every [`POLL`].
    fn stopped(&mut self) -> bool;

    /// Whether the run as it is now ends this reader (ended; paused, for a reader a pause ends). Read every
    /// [`CHECK_EVERY`]; the follow ends on it only when no step of the run is still running on this node.
    ///
    /// # Errors
    ///
    /// The run cannot be read.
    fn settled(&mut self, facade: &EntityFacade, run_id: &str) -> Result<bool, Error>;
}

/// What a follow came to: how it ended and the last stored sequence it handed the reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Followed {
    pub ended: Ended,
    pub last: i64,
}

/// Replay the run's stored events after `after` to `reader`, then follow the run as the module doc says, until the
/// reader ends it, the client is gone or the reader stops it.
///
/// # Errors
///
/// Returns an error when the events cannot be read, or the reader's error.
pub fn replay_then_follow(
    factory: &Arc<ExecutionContextFactory>,
    run_id: &str,
    after: i64,
    reader: &mut dyn Reader,
) -> Result<Followed, Error> {
    let facade = factory
        .entity()
        .ok_or_else(|| Error::new("the entity facade is not installed"))?;
    let mut last = after;
    if let Some(ended) = replay(&facade, run_id, &mut last, reader)? {
        return Ok(Followed { ended, last });
    }
    let engine = engine(factory);
    let hub = engine.hub();
    let _watch = hub.watch(factory, run_id, last);
    let mut follower = hub::Follower::new(&hub, run_id, last);
    let mut wrote = Instant::now();
    let mut checked = Instant::now();
    loop {
        if reader.stopped() {
            return Ok(Followed {
                ended: Ended::Stopped,
                last,
            });
        }
        for event in follower.next(&hub, &facade, POLL)? {
            if let Some(seq) = event.seq {
                last = seq;
            }
            match reader.event(&facade, event.seq, &event.event_type, &event.payload)? {
                Flow::Next => {}
                Flow::Wrote => wrote = Instant::now(),
                Flow::End => {
                    return Ok(Followed {
                        ended: Ended::Over,
                        last,
                    })
                }
                Flow::Gone => {
                    return Ok(Followed {
                        ended: Ended::Gone,
                        last,
                    })
                }
            }
        }
        if wrote.elapsed() >= PING_EVERY {
            if !reader.keep_alive() {
                return Ok(Followed {
                    ended: Ended::Gone,
                    last,
                });
            }
            wrote = Instant::now();
        }
        if checked.elapsed() >= CHECK_EVERY {
            checked = Instant::now();
            if reader.settled(&facade, run_id)? && !engine.is_local(run_id) {
                // the last events may have come after the hub was read: send what is stored, then end
                let ended = replay(&facade, run_id, &mut last, reader)?.unwrap_or(Ended::Over);
                return Ok(Followed { ended, last });
            }
        }
    }
}

/// The stored events after `last`, page by page, to `reader`. Out: how the stream ended, or `None` when it caught up.
fn replay(
    facade: &EntityFacade,
    run_id: &str,
    last: &mut i64,
    reader: &mut dyn Reader,
) -> Result<Option<Ended>, Error> {
    loop {
        let page = events::stored_after(facade, run_id, *last, PAGE)?;
        for event in &page {
            *last = event.seq;
            match reader.event(facade, Some(event.seq), &event.event_type, &event.payload)? {
                Flow::Next | Flow::Wrote => {}
                Flow::End => return Ok(Some(Ended::Over)),
                Flow::Gone => return Ok(Some(Ended::Gone)),
            }
        }
        if i64::try_from(page.len()).unwrap_or(0) < PAGE {
            return Ok(None);
        }
    }
}
