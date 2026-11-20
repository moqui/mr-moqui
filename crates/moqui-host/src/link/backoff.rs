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

//! The waits between dials of a lost link.

use std::time::Duration;

/// The delays between attempts to dial a lost link: `first`, doubling up to `max`, each with jitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    pub first: Duration,
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            max: Duration::from_secs(30),
        }
    }
}

impl Backoff {
    /// The waits of a node the other end refused: dialing again helps only once its operator acts.
    pub fn refused() -> Self {
        Self {
            first: Duration::from_secs(30),
            max: Duration::from_secs(600),
        }
    }

    /// The wait before attempt `attempt` (0 is the first after a loss): half the doubled delay plus a random part of
    /// the other half (`jitter` in 0 to 1), so nodes that lost their links together do not dial together.
    pub fn delay(&self, attempt: u32, jitter: f64) -> Duration {
        let doubled = self.first.saturating_mul(1_u32 << attempt.min(16));
        let base = doubled.min(self.max).max(Duration::from_millis(1));
        let half = base / 2;
        half + half.mul_f64(jitter.clamp(0.0, 1.0))
    }
}

/// A number from 0 to 1 that differs from call to call, for [`Backoff::delay`].
pub(crate) fn jitter() -> f64 {
    use std::hash::{BuildHasher as _, Hasher as _};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos()),
    );
    (hasher.finish() % 10_000) as f64 / 10_000.0
}
