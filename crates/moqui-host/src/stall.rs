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

//! A bounded wait for a client that stops reading, for a blocking thread that feeds an async response (a git job's
//! output, an Assist event stream): `plans/archive/High-Level-Review.md` HL-027.

use std::io;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// Send `item` on `tx` from a blocking thread, waiting while the channel is full, but only until the other side has
/// taken nothing for `stall`: then the error is `TimedOut`, so a client that stops reading cannot hold the thread, and
/// the pool slot or stream slot it has, for ever. A closed channel is `BrokenPipe`.
pub(crate) fn send_within<T>(tx: &mpsc::Sender<T>, item: T, stall: Duration) -> io::Result<()> {
    let deadline = Instant::now() + stall;
    let mut item = item;
    let mut wait = Duration::from_millis(1);
    loop {
        match tx.try_send(item) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Closed(_)) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "the client went away"));
            }
            Err(TrySendError::Full(back)) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the client took nothing for the idle timeout",
                    ));
                }
                item = back;
                std::thread::sleep(wait.min(left));
                wait = (wait * 2).min(Duration::from_millis(50));
            }
        }
    }
}
