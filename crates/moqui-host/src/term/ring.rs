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

//! A session's output ring (plans/Terminal-Fancy.md §5.4): its last `replay-kb` of output, placed by the count of all
//! the bytes the program wrote, and the points in it where the stream was at rest with the modes in force there
//! ([`super::track`]). A client that drew up to a count still in the ring goes on from it; one that did not is given
//! the ring from its oldest point at rest, so a replay never starts inside a sequence. Nothing in it reads the
//! factory, so the holder (§7.5) keeps it too.

use std::collections::VecDeque;

use super::track::Modes;

/// The points at rest kept; older ones are dropped first, and one is kept in each stretch of the ring.
const RESTS_MAX: usize = 4096;

#[derive(Debug)]
pub(crate) struct Ring {
    capacity: usize,
    bytes: VecDeque<u8>,
    /// The count of the ring's first byte.
    start: u64,
    /// The counts at rest in the ring, oldest first, with their modes.
    rests: VecDeque<(u64, Modes)>,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            bytes: VecDeque::new(),
            start: 0,
            rests: VecDeque::from([(0, Modes::default())]),
        }
    }

    /// `bytes`, written by the program from count `at`, with the points at rest the tracker found in them (offsets
    /// into `bytes`).
    pub fn push(&mut self, bytes: &[u8], at: u64, rests: &[(usize, Modes)]) {
        for (offset, modes) in rests {
            self.rests.push_back((at + *offset as u64, *modes));
        }
        // past the cap the second oldest goes, so the oldest, the longest replay, stays
        while self.rests.len() > RESTS_MAX {
            self.rests.remove(1.min(self.rests.len() - 1));
        }
        let end = at + bytes.len() as u64;
        let tail = &bytes[bytes.len() - bytes.len().min(self.capacity)..];
        let over = (self.bytes.len() + tail.len()).saturating_sub(self.capacity);
        self.bytes.drain(..over);
        self.bytes.extend(tail);
        self.start = end - self.bytes.len() as u64;
        while self.rests.front().is_some_and(|(count, _)| *count < self.start) {
            self.rests.pop_front();
        }
    }

    /// The bytes after `have`, when the ring holds all of them up to `count`.
    pub fn since(&self, have: u64, count: u64) -> Option<Vec<u8>> {
        if have < self.start || have > count {
            return None;
        }
        let skip = usize::try_from(have - self.start).ok()?;
        Some(self.bytes.iter().skip(skip).copied().collect())
    }

    /// The ring from its oldest point at rest: that point's count, the bytes, and the modes in force there. With no
    /// point at rest left in it, nothing: `count`, and the modes in force now, `current`.
    pub fn replay_from_rest(&self, count: u64, current: Modes) -> (u64, Vec<u8>, Modes) {
        match self.rests.front() {
            Some((from, modes)) => (*from, self.since(*from, count).unwrap_or_default(), *modes),
            None => (count, Vec::new(), current),
        }
    }
}

#[cfg(test)]
#[path = "ring_tests.rs"]
mod tests;
