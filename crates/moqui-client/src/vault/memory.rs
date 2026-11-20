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

//! Device memory on the device (plans/Vault-Instances.md §3.5, V5): the open personal key wrapped
//! (`moqui_vault::personal::wrap_with_device_key`, its memory, user and time in the associated data), kept in a file of
//! the client's directory (`vault/<account>.memory.json`, owner-only), and the device's half of the wrap's key in the
//! host's key store ([`MemoryKeys`]): the OS key store on the desktop, a Keystore key that opens only after the device's
//! lock or biometrics, each time, on the phone. The other half is the server's ([`super::server`]). A copy of the file
//! is no use without both.

use std::path::{Path, PathBuf};

use moqui_vault::ops::DeviceSealed;
use moqui_vault::text::{hex, unhex};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::keys::KeyStore;

/// A memory key: 32 bytes, cleared on drop.
pub use moqui_vault::keys::Key32;

/// Why a memory key was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The store opens the key only once the user has confirmed with the device's lock or biometrics.
    NeedsUnlock,
    /// The store did not answer, or held something that is not a key.
    Failed(String),
}

/// Where the memory keys are kept, one per account, by entry name ([`entry`]). Filled by the host.
pub trait MemoryKeys: Send + Sync {
    /// How the store guards a key: [`KIND_OS`], [`KIND_DEVICE_LOCK`] or [`KIND_FILE`]. Device memory is refused on a
    /// file, which lies beside the wrap it would open.
    fn kind(&self) -> &'static str;

    /// Keep `key` as `entry`, replacing any.
    ///
    /// # Errors
    ///
    /// Returns why the store did not take it.
    fn put(&self, entry: &str, key: &Key32) -> Result<(), String>;

    /// The key of `entry`; none when the store holds none.
    ///
    /// # Errors
    ///
    /// [`KeyError::NeedsUnlock`] when the user must confirm first, else why the store did not answer.
    fn take(&self, entry: &str) -> Result<Option<Key32>, KeyError>;

    /// Forget `entry`; none kept is not an error.
    ///
    /// # Errors
    ///
    /// Returns why the store could not be changed.
    fn delete(&self, entry: &str) -> Result<(), String>;
}

/// The OS key store, opened by the device's sign-in (the desktops' Keychain, Credential Manager, Secret Service).
pub const KIND_OS: &str = "os";
/// A key store that opens a key only after the device's lock screen or biometrics (the Android Keystore with user
/// authentication).
pub const KIND_DEVICE_LOCK: &str = "device-lock";
/// A file only the user can read: no device memory.
pub const KIND_FILE: &str = "file";

/// The key store entry of `account`'s memory key: an account id has no `.vault-memory` of its own, so it names no
/// device key or chat store entry.
pub fn entry(account: &str) -> String {
    format!("{account}.vault-memory")
}

/// Memory keys in a [`KeyStore`] as hex: the desktop's OS key store, or the file store where there is none (which then
/// says [`KIND_FILE`], and device memory is not offered).
pub struct StoreMemoryKeys {
    store: Box<dyn KeyStore>,
    kind: &'static str,
}

impl StoreMemoryKeys {
    pub fn new(store: Box<dyn KeyStore>, kind: &'static str) -> Self {
        Self { store, kind }
    }
}

impl MemoryKeys for StoreMemoryKeys {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn put(&self, entry: &str, key: &Key32) -> Result<(), String> {
        let text = Zeroizing::new(hex(key.as_bytes()));
        self.store.set(entry, &text).map_err(|err| err.to_string())
    }

    fn take(&self, entry: &str) -> Result<Option<Key32>, KeyError> {
        let Some(text) = self.store.get(entry).map(Zeroizing::new) else {
            return Ok(None);
        };
        let bytes = Zeroizing::new(unhex(&text).map_err(|err| KeyError::Failed(err.to_string()))?);
        Key32::from_slice(&bytes)
            .map(Some)
            .map_err(|err| KeyError::Failed(err.to_string()))
    }

    fn delete(&self, entry: &str) -> Result<(), String> {
        self.store.delete(entry).map_err(|err| err.to_string())
    }
}

/// A memory as its file keeps it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Record {
    /// The memory's id: in the wrap's associated data and the server's record of it.
    pub memory_id: String,
    pub personal_key_id: String,
    pub user_id: String,
    /// Both public halves, hex.
    pub public: String,
    /// The private halves wrapped by the memory key, hex.
    pub sealed: String,
    /// When it ends, epoch millis.
    pub until: i64,
}

impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Record")
            .field("memory_id", &self.memory_id)
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}

impl Record {
    pub(crate) fn new(memory_id: &str, sealed: &DeviceSealed) -> Self {
        Self {
            memory_id: memory_id.to_string(),
            personal_key_id: sealed.personal_key_id.clone(),
            user_id: sealed.user_id.clone(),
            public: hex(&sealed.public),
            sealed: hex(&sealed.sealed),
            until: sealed.until,
        }
    }

    pub(crate) fn sealed(&self) -> Result<DeviceSealed, String> {
        Ok(DeviceSealed {
            personal_key_id: self.personal_key_id.clone(),
            user_id: self.user_id.clone(),
            public: unhex(&self.public).map_err(|err| err.to_string())?,
            until: self.until,
            sealed: unhex(&self.sealed).map_err(|err| err.to_string())?,
        })
    }
}

/// Whether `account` can name a file: the characters of an account id, not starting with a dot.
pub(crate) fn plain_name(account: &str) -> bool {
    !account.is_empty()
        && !account.starts_with('.')
        && account
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn record_path(dir: &Path, account: &str) -> Option<PathBuf> {
    plain_name(account).then(|| dir.join(format!("{account}.memory.json")))
}

/// The memory of `account`, if one is kept. One that does not parse is removed and logged.
pub(crate) fn load(dir: &Path, account: &str) -> Option<Record> {
    let path = record_path(dir, account)?;
    let text = Zeroizing::new(std::fs::read_to_string(&path).ok()?);
    match serde_json::from_str(&text) {
        Ok(record) => Some(record),
        Err(err) => {
            tracing::error!("The vault device memory of {account} does not parse, and is removed: {err}");
            if let Err(err) = remove(dir, account) {
                tracing::error!("{err}");
            }
            None
        }
    }
}

/// Keep `record` as `account`'s memory.
pub(crate) fn save(dir: &Path, account: &str, record: &Record) -> Result<(), String> {
    let path = record_path(dir, account).ok_or_else(|| format!("not an account id: {account}"))?;
    let text = Zeroizing::new(serde_json::to_string(record).map_err(|err| err.to_string())?);
    crate::files::write_private(&path, text.as_bytes()).map_err(|err| format!("{}: {err}", path.display()))
}

/// Remove `account`'s memory file; none is not an error.
pub(crate) fn remove(dir: &Path, account: &str) -> Result<(), String> {
    let Some(path) = record_path(dir, account) else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// The accounts with a memory file in `dir`.
pub(crate) fn accounts(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".memory.json")
                .filter(|account| plain_name(account))
                .map(str::to_string)
        })
        .collect()
}
