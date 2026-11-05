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

//! The waits of a client's socket that connects again after a drop: doubling from 2 seconds up to a minute, each a
//! random point in the upper half of its step, so the clients of a server that restarted do not all connect again at
//! the same moment (as the edge link's dialer waits, `moqui-host`'s `edge_link::Backoff`). `moqui tui --server`'s
//! notification socket and the desktop and mobile client's page sockets wait so.

use std::time::Duration;

/// The first step.
const FIRST: Duration = Duration::from_secs(2);

/// The longest step.
const MAX: Duration = Duration::from_secs(60);

/// The waits between a socket's connections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    step: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { step: FIRST }
    }
}

impl Backoff {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start the steps over, after a connection that opened: its drop is new trouble, not the same.
    pub fn reset(&mut self) {
        self.step = FIRST;
    }

    /// The next wait, from `jitter` (0 to 1; [`jitter`] in use, a fixed number in a test): half the step plus that
    /// part of the other half. The step then doubles, up to [`MAX`].
    pub fn next_wait(&mut self, jitter: f64) -> Duration {
        let half = self.step / 2;
        let wait = half + half.mul_f64(jitter.clamp(0.0, 1.0));
        self.step = (self.step * 2).min(MAX);
        wait
    }
}

/// A number from 0 to 1 that differs from call to call, for [`Backoff::next_wait`]. Not for anything secret.
#[must_use]
pub fn jitter() -> f64 {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    (hasher.finish() % 10_000) as f64 / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_double_to_a_minute_within_the_upper_half_of_each_step() {
        let mut low = Backoff::new();
        let mut high = Backoff::new();
        let steps = [2, 4, 8, 16, 32, 60, 60];
        for step in steps {
            let step = Duration::from_secs(step);
            assert_eq!(low.next_wait(0.0), step / 2);
            assert_eq!(high.next_wait(1.0), step);
        }
        let mut mid = Backoff::new();
        assert_eq!(mid.next_wait(0.5), Duration::from_millis(1500));
        // out of range is held to the bounds
        assert_eq!(mid.next_wait(7.0), Duration::from_secs(4));
        mid.reset();
        assert_eq!(mid.next_wait(-1.0), Duration::from_secs(1));
    }

    #[test]
    fn jitter_is_between_0_and_1_and_varies() {
        let draws: Vec<f64> = (0..64).map(|_| jitter()).collect();
        assert!(draws.iter().all(|draw| (0.0..1.0).contains(draw)));
        assert!(
            draws.iter().any(|draw| *draw != draws[0]),
            "64 draws were all {}",
            draws[0]
        );
    }
}
