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

//! The accounts' stores (`moqui_client::store`, plans/Mobile-Desktop.md §6, MD4): one per account, opened when first
//! used and kept open. Each is behind a lock of its own, held only while the store is read or written, never across a
//! request to the server.
//!
//! An account's store opens with the client conf's size cap. When it opens, its timelines' heads are marked stale (the
//! app was not running, so pushes may have been missed) and the answers MD3 kept as files are moved into it once.
//!
//! [`Stores::forget`] closes an account's store and deletes it: the slot is emptied under its lock, so a reader that
//! got the slot before finds nothing, and the files go once no connection is open. The caller removes the account from
//! its list first; a store is opened only when the caller's check that the account is kept passes under this set's
//! lock (lock order: the stores, then the accounts), so nothing opens it again after.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use moqui_client::store::Store;

/// An account's store, or none once it was forgotten.
type Slot = Arc<Mutex<Option<Store>>>;

/// The stores of the accounts, in the client's directory (`data/`, beside `accounts.json`).
pub(crate) struct Stores {
    dir: PathBuf,
    open: Mutex<HashMap<String, Slot>>,
}

impl Stores {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            open: Mutex::new(HashMap::new()),
        }
    }

    /// Run `work` on the store of `account`, opened with the cap `max_bytes` if it is not open yet and `may_open` says
    /// the account is still kept here. `may_open` is asked under this set's lock, which [`Self::forget`] takes too, so a
    /// late write for an account being forgotten never makes its store again. None when it may not or cannot be opened
    /// (logged), or was forgotten while `work` waited for it.
    pub(crate) fn with<T>(
        &self,
        account: &str,
        max_bytes: u64,
        may_open: impl FnOnce() -> bool,
        work: impl FnOnce(&Store) -> T,
    ) -> Option<T> {
        let slot = {
            let mut open = moqui_lib::sync::lock(&self.open);
            match open.get(account) {
                Some(slot) => Arc::clone(slot),
                None => {
                    if !may_open() {
                        return None;
                    }
                    let store = self.open_store(account, max_bytes)?;
                    let slot = Arc::new(Mutex::new(Some(store)));
                    open.insert(account.to_string(), Arc::clone(&slot));
                    slot
                }
            }
        };
        let held = moqui_lib::sync::lock(&slot);
        held.as_ref().map(work)
    }

    /// Close the store of `account` and delete it with its files, and any MD3 answer files left of it. Logged when a
    /// part cannot be removed.
    pub(crate) fn forget(&self, account: &str) {
        let mut open = moqui_lib::sync::lock(&self.open);
        let removed = match open.remove(account) {
            // under the slot's lock: a reader holding the slot sees it empty after this
            Some(slot) => match moqui_lib::sync::lock(&slot).take() {
                Some(store) => store.forget(),
                None => Ok(()),
            },
            None => moqui_client::store::forget(&self.dir, account),
        };
        drop(open);
        if let Err(err) = removed {
            tracing::error!("The data of {account} on this device was not all removed: {err}");
        }
        moqui_client::cache::forget(&self.dir.join("cache"), account);
    }

    fn open_store(&self, account: &str, max_bytes: u64) -> Option<Store> {
        let store = match Store::open(&self.dir, account, max_bytes) {
            Ok(store) => store,
            Err(err) => {
                tracing::error!("The store of {account} did not open: {err}");
                return None;
            }
        };
        // pushes may have been missed while it was closed: a timeline's newer events are a gap until read again
        if let Err(err) = store.mark_chat_heads_stale() {
            tracing::warn!("The chat heads of {account} were not marked stale: {err}");
        }
        let moved = moqui_client::cache::migrate(&self.dir.join("cache"), account, &store);
        if moved > 0 {
            tracing::info!("Moved {moved} kept answers of {account} into its store");
        }
        Some(store)
    }
}

#[cfg(test)]
#[path = "stores_tests.rs"]
mod tests;
