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

//! Lock guards and condition-variable waits that ignore poisoning, and a process-wide install slot.
//!
//! A panic while a lock is held leaves the data as the panicking thread left it. Every lock here
//! guards state that stays usable after such a panic (maps, queues, counters), so the guard is taken
//! anyway rather than spreading the panic to every later caller.

use std::sync::{
    Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, WaitTimeoutResult,
};
use std::time::Duration;

/// Lock `mutex`, taking the guard of a poisoned lock too.
pub fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read-lock `lock`, taking the guard of a poisoned lock too.
pub fn read<T: ?Sized>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Write-lock `lock`, taking the guard of a poisoned lock too.
pub fn write<T: ?Sized>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Wait on `condvar`, taking the guard back from a poisoned lock too.
pub fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar.wait(guard).unwrap_or_else(PoisonError::into_inner)
}

/// Wait on `condvar` for at most `timeout`, taking the guard back from a poisoned lock too.
pub fn wait_timeout<'a, T>(
    condvar: &Condvar,
    guard: MutexGuard<'a, T>,
    timeout: Duration,
) -> (MutexGuard<'a, T>, WaitTimeoutResult) {
    condvar
        .wait_timeout(guard, timeout)
        .unwrap_or_else(PoisonError::into_inner)
}

/// One installed implementation of `T` for the process, such as the client a host crate installs
/// for a core facade. Empty until [`InstallSlot::install`].
pub struct InstallSlot<T: ?Sized> {
    slot: RwLock<Option<Arc<T>>>,
}

impl<T: ?Sized> InstallSlot<T> {
    /// An empty slot, usable in a `static`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slot: RwLock::new(None),
        }
    }

    /// Use `value` from now on, replacing any installed before.
    pub fn install(&self, value: Arc<T>) {
        *write(&self.slot) = Some(value);
    }

    /// Remove the installed value.
    pub fn clear(&self) {
        *write(&self.slot) = None;
    }

    /// The installed value, if any.
    pub fn get(&self) -> Option<Arc<T>> {
        read(&self.slot).clone()
    }
}

impl<T: ?Sized> Default for InstallSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_lock_still_yields_its_guard() {
        let mutex = Arc::new(Mutex::new(1));
        let rw = Arc::new(RwLock::new(2));
        let (m, r) = (Arc::clone(&mutex), Arc::clone(&rw));
        let _ = std::thread::spawn(move || {
            let _held = m.lock().unwrap();
            let _written = r.write().unwrap();
            panic!("poison both");
        })
        .join();
        assert!(mutex.is_poisoned() && rw.is_poisoned());
        *lock(&mutex) += 1;
        *write(&rw) += 1;
        assert_eq!((*lock(&mutex), *read(&rw)), (2, 3));
        let condvar = Condvar::new();
        let (guard, waited) = wait_timeout(&condvar, lock(&mutex), Duration::from_millis(1));
        assert!(waited.timed_out());
        assert_eq!(*guard, 2);
    }

    #[test]
    fn install_slot_installs_replaces_and_clears() {
        trait Named: Send + Sync {
            fn name(&self) -> &'static str;
        }
        struct A;
        struct B;
        impl Named for A {
            fn name(&self) -> &'static str {
                "a"
            }
        }
        impl Named for B {
            fn name(&self) -> &'static str {
                "b"
            }
        }
        static SLOT: InstallSlot<dyn Named> = InstallSlot::new();
        assert!(SLOT.get().is_none());
        SLOT.install(Arc::new(A));
        assert_eq!(SLOT.get().map(|named| named.name()), Some("a"));
        SLOT.install(Arc::new(B));
        assert_eq!(SLOT.get().map(|named| named.name()), Some("b"));
        SLOT.clear();
        assert!(SLOT.get().is_none());
    }
}
