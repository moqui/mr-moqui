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


//! The workers of a pool waiting for work, the one idle most recently on top (plans/Profiling3.md W13).
//!
//! A pool wakes the worker idle most recently for each item it queues, instead of every idle worker. Work then stays
//! on the few threads a light load needs, their script runtimes stay warm, and the others wait out their idle time and
//! let their runtimes go. A condition variable of the pool's own wakes all its waiters, and one waking a single waiter
//! wakes about the one that waited longest, the opposite; so each worker waits on a condition variable of its own,
//! under the pool's state lock.

use std::sync::{Arc, Condvar};

#[derive(Default)]
pub(crate) struct Idle(Vec<Arc<Condvar>>);

impl Idle {
    /// A worker begins to wait on `waiter` (under the pool's state lock).
    pub(crate) fn enter(&mut self, waiter: &Arc<Condvar>) {
        self.0.push(Arc::clone(waiter));
    }

    /// The worker waiting on `waiter` woke: it is no longer idle, if a wake did not already take it off.
    pub(crate) fn leave(&mut self, waiter: &Arc<Condvar>) {
        if let Some(at) = self.0.iter().rposition(|idle| Arc::ptr_eq(idle, waiter)) {
            self.0.remove(at);
        }
    }

    /// Wake the worker idle most recently, if one is.
    pub(crate) fn wake_one(&mut self) {
        if let Some(waiter) = self.0.pop() {
            waiter.notify_one();
        }
    }

    /// Wake every idle worker (a stop).
    pub(crate) fn wake_all(&mut self) {
        for waiter in self.0.drain(..) {
            waiter.notify_one();
        }
    }
}

#[cfg(test)]
#[path = "idle_tests.rs"]
mod tests;
