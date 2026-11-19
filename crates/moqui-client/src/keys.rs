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

//! Where a device's login keys are kept, one per account id. The host supplies the OS key store (Keychain, Secret
//! Service); [`FileKeyStore`] is the fallback where there is none, a file its owner alone can read, as
//! `moqui tui --server` keeps its key (plans/Mobile-Desktop.md §4, "Remote applications and authentication").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use moqui_lib::error::Error;
use serde_json::Value;

/// A store of device keys by account id.
pub trait KeyStore: Send + Sync {
    /// The key of `account`, if one is kept.
    fn get(&self, account: &str) -> Option<String>;

    /// Keep `key` for `account`, replacing any.
    ///
    /// # Errors
    ///
    /// Returns an error when the store refuses or cannot be written.
    fn set(&self, account: &str, key: &str) -> Result<(), Error>;

    /// Forget the key of `account`; none kept is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be changed.
    fn delete(&self, account: &str) -> Result<(), Error>;

    /// Whether `entry` is read from the store each time and never kept in memory ([`Keys`]): one the store opens only
    /// at some times (on the phone, a chat store passphrase while it is unlocked, plans/Security-Review.md SEC-CLI-113),
    /// so a read at another time must not stay as none, nor a read at the right time outlive it.
    fn fresh(&self, entry: &str) -> bool {
        let _ = entry;
        false
    }
}

/// Keys in `<dir>/<account>.key`, mode 0600 on Unix (`files::write_private`).
#[derive(Debug, Clone)]
pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf() }
    }

    /// The file of `account`, or none for an id that could leave the directory.
    fn file(&self, account: &str) -> Option<PathBuf> {
        let plain = !account.is_empty()
            && account
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
            && !account.starts_with('.');
        plain.then(|| self.dir.join(format!("{account}.key")))
    }
}

impl KeyStore for FileKeyStore {
    fn get(&self, account: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.file(account)?).ok()?;
        Some(text.trim().to_string()).filter(|key| !key.is_empty())
    }

    fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        let path = self
            .file(account)
            .ok_or_else(|| Error::new(format!("not an account id: {account}")))?;
        let written = crate::files::write_private(&path, key.as_bytes());
        written.map_err(|err| Error::new(format!("{}: {err}", path.display())))
    }

    fn delete(&self, account: &str) -> Result<(), Error> {
        let Some(path) = self.file(account) else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::new(format!("{}: {err}", path.display()))),
        }
    }
}

/// The device keys, kept in memory once read: a page's every request is forwarded with its account's key, and a
/// Keychain or Secret Service read for each would be slow (and could ask the user again and again). Also each account's
/// chat store passphrases, by crypto device.
pub struct Keys {
    store: Box<dyn KeyStore>,
    /// `os` for the OS key store, `file` for the fallback, which the shell tells the user of.
    pub kind: &'static str,
    known: Mutex<HashMap<String, Option<String>>>,
}

impl Keys {
    /// The keys in `store`, of the kind `kind` (`os` or `file`).
    pub fn new(store: Box<dyn KeyStore>, kind: &'static str) -> Self {
        Self {
            store,
            kind,
            known: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, account: &str) -> Option<String> {
        if self.store.fresh(account) {
            return self.store.get(account);
        }
        if let Some(known) = moqui_lib::sync::lock(&self.known).get(account) {
            return known.clone();
        }
        let key = self.store.get(account);
        moqui_lib::sync::lock(&self.known).insert(account.to_string(), key.clone());
        key
    }

    /// Keep `key` for `account`.
    ///
    /// # Errors
    ///
    /// Returns an error when the store does not take it.
    pub fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        self.store.set(account, key)?;
        if !self.store.fresh(account) {
            moqui_lib::sync::lock(&self.known).insert(account.to_string(), Some(key.to_string()));
        }
        Ok(())
    }

    /// Forget the key; it is gone here whether or not the store could delete it (logged by the caller).
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot delete it.
    pub fn delete(&self, account: &str) -> Result<(), Error> {
        moqui_lib::sync::lock(&self.known).insert(account.to_string(), None);
        self.store.delete(account)
    }

    /// The chat store passphrase of `account`'s crypto device `device` (the server's `/ChatKeys/storeKey` answer), kept
    /// so the page's encrypted store opens offline.
    pub fn chat_store_key(&self, account: &str, device: &str) -> Option<String> {
        self.chat_store_keys(account)
            .get(device)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Keep the chat store passphrase of a device of `account`, replacing the one kept (a revision since changes it).
    /// One entry per account holds its devices' (`<account>.chat-store`), so the account's go with one delete.
    ///
    /// # Errors
    ///
    /// Returns an error when the store does not take it.
    pub fn set_chat_store_key(&self, account: &str, device: &str, key: &str) -> Result<(), Error> {
        let mut keys = self.chat_store_keys(account);
        if keys.get(device).and_then(Value::as_str) == Some(key) {
            return Ok(());
        }
        keys.insert(device.to_string(), Value::String(key.to_string()));
        self.set(&chat_store_entry(account), &Value::Object(keys).to_string())
    }

    /// Forget the chat store passphrase of a device of `account` (revoked, or no longer the server's).
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be changed.
    pub fn remove_chat_store_key(&self, account: &str, device: &str) -> Result<(), Error> {
        let mut keys = self.chat_store_keys(account);
        if keys.remove(device).is_none() {
            return Ok(());
        }
        if keys.is_empty() {
            return self.delete(&chat_store_entry(account));
        }
        self.set(&chat_store_entry(account), &Value::Object(keys).to_string())
    }

    /// Forget every chat store passphrase of `account`.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot delete them.
    pub fn forget_chat_store_keys(&self, account: &str) -> Result<(), Error> {
        self.delete(&chat_store_entry(account))
    }

    fn chat_store_keys(&self, account: &str) -> serde_json::Map<String, Value> {
        match self
            .get(&chat_store_entry(account))
            .map(|text| serde_json::from_str::<Value>(&text))
        {
            Some(Ok(Value::Object(keys))) => keys,
            Some(_) => {
                tracing::warn!("The chat store keys of {account} do not parse; taken as none");
                serde_json::Map::new()
            }
            None => serde_json::Map::new(),
        }
    }
}

/// The key store entry of the device keys whose sign-out the server did not hear: an account id has no `_`, so it names
/// no account's key (plans/Security-Review.md SEC-CLI-108).
const PENDING_REVOKES: &str = "_revoke";
/// At most this many keys wait to be revoked; the oldest go first.
const PENDING_MAX: usize = 32;
/// How long a key waits to be revoked: by then the server has ended it (`login-key@device-expire-hours`, 2160).
pub const PENDING_MILLIS: i64 = 90 * 24 * 60 * 60 * 1000;

/// A device key signed out here whose server did not hear it: kept, sealed as the keys are, until the server revokes it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingRevoke {
    /// The server's base url.
    pub server: String,
    pub key: String,
    /// When it was signed out here, in epoch millis.
    pub at: i64,
    /// When it was last tried, in epoch millis.
    #[serde(default)]
    pub tried: i64,
}

impl Keys {
    /// The device keys waiting to be revoked on their servers.
    pub fn pending_revokes(&self) -> Vec<PendingRevoke> {
        let Some(text) = self.get(PENDING_REVOKES) else {
            return Vec::new();
        };
        serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!("The keys waiting to be revoked do not parse; taken as none: {err}");
            Vec::new()
        })
    }

    /// Keep the waiting keys, `pending`: the newest [`PENDING_MAX`]; none deletes the entry.
    ///
    /// # Errors
    ///
    /// Returns an error when the store does not take them.
    pub fn set_pending_revokes(&self, mut pending: Vec<PendingRevoke>) -> Result<(), Error> {
        if pending.is_empty() {
            return self.delete(PENDING_REVOKES);
        }
        pending.sort_by_key(|entry| entry.at);
        let over = pending.len().saturating_sub(PENDING_MAX);
        pending.drain(..over);
        let text = serde_json::to_string(&pending).map_err(|err| Error::new(err.to_string()))?;
        self.set(PENDING_REVOKES, &text)
    }

    /// `key` of `server` signed out here at `at` with its server not told: kept to revoke there later.
    ///
    /// # Errors
    ///
    /// Returns an error when the store does not take it.
    pub fn add_pending_revoke(&self, server: &str, key: &str, at: i64) -> Result<(), Error> {
        let mut pending = self.pending_revokes();
        pending.retain(|entry| entry.key != key);
        pending.push(PendingRevoke {
            server: server.to_string(),
            key: key.to_string(),
            at,
            tried: at,
        });
        self.set_pending_revokes(pending)
    }
}

/// The key store entry of an account's chat store passphrases: an account id is a host and a hex digest, so this never
/// names another account's device key.
fn chat_store_entry(account: &str) -> String {
    format!("{account}.chat-store")
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
