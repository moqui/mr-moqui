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

//! The outcome of an outbound leg's INVITE when answers, a cancel and the ring timeout race (plans/archive/Comms-Framework.md
//! §5.4 "Call state": the first successful answer wins and the others are ended). Sans-IO with the time passed in, so
//! the races are tested with simulated time; [`crate::ua`] acts on its answers.
//!
//! - A forking proxy may deliver 2xx from several phones. The first confirms the dialog; each later one is ACKed and
//!   ended with a BYE (RFC 3261 §13.2.2.4).
//! - The caller giving up, or the ring timing out, sends CANCEL while nothing answered. A 2xx that crosses the CANCEL
//!   established a session at the far end anyway: it is ACKed and ended with a BYE (RFC 3261 §9.1), and the leg counts
//!   as canceled, not answered.

use std::time::{Duration, Instant};

/// What to do with an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// This answer is the call.
    Confirm,
    /// ACK and BYE this answer: another one won, or the leg was canceled.
    EndLate,
    /// Send CANCEL for the INVITE.
    Cancel,
    Nothing,
}

/// How the leg ended, once it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    Canceled,
    TimedOut,
    Failed(u16),
}

/// One outbound INVITE's race.
#[derive(Debug, Clone)]
pub struct Race {
    deadline: Instant,
    winner: Option<String>,
    canceled: Option<Outcome>,
    failed: Option<u16>,
}

impl Race {
    pub fn new(now: Instant, ring_timeout: Duration) -> Self {
        Race {
            deadline: now + ring_timeout,
            winner: None,
            canceled: None,
            failed: None,
        }
    }

    /// The remote tag of the answer that won.
    pub fn winner(&self) -> Option<&str> {
        self.winner.as_deref()
    }

    pub fn outcome(&self) -> Option<Outcome> {
        if self.winner.is_some() {
            return Some(Outcome::Answered);
        }
        if let Some(code) = self.failed {
            return Some(Outcome::Failed(code));
        }
        self.canceled
    }

    /// A 2xx with this remote tag (one per fork).
    pub fn answered(&mut self, tag: &str) -> Action {
        match (&self.winner, self.canceled, self.failed) {
            (Some(winner), _, _) if winner == tag => Action::Nothing,
            (Some(_), _, _) | (None, Some(_), _) | (None, None, Some(_)) => Action::EndLate,
            (None, None, None) => {
                self.winner = Some(tag.to_string());
                Action::Confirm
            }
        }
    }

    /// A final failure (4xx–6xx) before any answer.
    pub fn failed(&mut self, code: u16) {
        if self.winner.is_none() && self.canceled.is_none() {
            self.failed = Some(code);
        }
    }

    /// The caller gave up.
    pub fn cancel(&mut self) -> Action {
        self.give_up(Outcome::Canceled)
    }

    fn give_up(&mut self, outcome: Outcome) -> Action {
        if self.winner.is_some() || self.failed.is_some() || self.canceled.is_some() {
            return Action::Nothing;
        }
        self.canceled = Some(outcome);
        Action::Cancel
    }

    /// The time passed: the ring times out once.
    pub fn poll(&mut self, now: Instant) -> Action {
        if now >= self.deadline {
            self.give_up(Outcome::TimedOut)
        } else {
            Action::Nothing
        }
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_answer_wins_and_a_late_one_is_ended() {
        let t0 = Instant::now();
        let mut race = Race::new(t0, Duration::from_secs(30));
        assert_eq!(race.answered("phone-a"), Action::Confirm);
        assert_eq!(race.answered("phone-a"), Action::Nothing, "a retransmitted 2xx");
        assert_eq!(race.answered("phone-b"), Action::EndLate, "a fork");
        assert_eq!(race.cancel(), Action::Nothing, "too late to cancel: hang up instead");
        assert_eq!(race.poll(t0 + Duration::from_secs(60)), Action::Nothing);
        assert_eq!(race.outcome(), Some(Outcome::Answered));
        assert_eq!(race.winner(), Some("phone-a"));
    }

    #[test]
    fn a_cancel_or_the_ring_timeout_beats_a_crossing_answer() {
        let t0 = Instant::now();
        let mut race = Race::new(t0, Duration::from_secs(30));
        assert_eq!(race.poll(t0 + Duration::from_secs(29)), Action::Nothing);
        assert_eq!(race.cancel(), Action::Cancel);
        assert_eq!(race.cancel(), Action::Nothing, "once");
        assert_eq!(race.answered("phone-a"), Action::EndLate, "the 2xx crossed the CANCEL");
        assert_eq!(race.outcome(), Some(Outcome::Canceled));

        let mut timed = Race::new(t0, Duration::from_secs(30));
        assert_eq!(timed.poll(t0 + Duration::from_secs(30)), Action::Cancel);
        assert_eq!(timed.poll(t0 + Duration::from_secs(31)), Action::Nothing);
        assert_eq!(timed.outcome(), Some(Outcome::TimedOut));

        let mut busy = Race::new(t0, Duration::from_secs(30));
        busy.failed(486);
        assert_eq!(busy.answered("x"), Action::EndLate);
        assert_eq!(busy.outcome(), Some(Outcome::Failed(486)));
    }
}
