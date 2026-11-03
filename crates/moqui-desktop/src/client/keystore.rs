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

//! Where the client keeps its device keys (plans/Mobile-Desktop.md §4, "Remote applications and authentication"): the
//! OS key store (the macOS Keychain, the Secret Service on Linux), one entry per account under the app's identifier.
//! Where there is none (a Linux desktop with no Secret Service running), a file in the app's config directory that
//! only the user can read, as `moqui tui --server` keeps its key; the shell says so.
//!
//! `MOQUI_DESKTOP_KEY_STORE` (`os` or `file`) chooses. A development run uses the file unless it says `os`: each build
//! is a new unsigned binary, which the Keychain would ask about again.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use moqui_client::keys::{FileKeyStore, KeyStore};
use moqui_lib::error::Error;
use serde_json::Value;

/// The service the keys are under in the OS key store.
const SERVICE: &str = crate::os::IDENTIFIER;

/// The OS key store.
struct OsKeyStore;

impl OsKeyStore {
    /// Whether the store answers: a missing entry does, a store that is not there or locked out does not.
    fn available() -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, "availability-check").map_err(|err| err.to_string())?;
        match entry.get_password() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn entry(account: &str) -> Result<keyring::Entry, Error> {
        keyring::Entry::new(SERVICE, account).map_err(|err| Error::new(format!("key store: {err}")))
    }
}

impl KeyStore for OsKeyStore {
    fn get(&self, account: &str) -> Option<String> {
        match Self::entry(account).ok()?.get_password() {
            Ok(key) => Some(key).filter(|key| !key.is_empty()),
            Err(keyring::Error::NoEntry) => None,
            Err(err) => {
                tracing::warn!("The key of {account} could not be read from the key store: {err}");
                None
            }
        }
    }

    fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        Self::entry(account)?
            .set_password(key)
            .map_err(|err| Error::new(format!("key store: {err}")))
    }

    fn delete(&self, account: &str) -> Result<(), Error> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(Error::new(format!("key store: {err}"))),
        }
    }
}

/// Which store to use: `os`, `file`, or none said.
fn chosen(variable: Option<&str>, dev: bool) -> &'static str {
    match variable.map(str::trim) {
        Some("os") => "os",
        Some("file") => "file",
        Some(other) if !other.is_empty() => {
            tracing::warn!("MOQUI_DESKTOP_KEY_STORE is {other:?}, not os or file; it is passed over");
            if dev {
                "file"
            } else {
                "os"
            }
        }
        _ if dev => "file",
        _ => "os",
    }
}

/// The device keys, kept in memory once read: a page's every request is forwarded with its account's key, and a
/// Keychain or Secret Service read for each would be slow (and could ask the user again and again).
pub(crate) struct Keys {
    store: Box<dyn KeyStore>,
    /// `os` for the OS key store, `file` for the fallback, which the shell tells the user of.
    pub(crate) kind: &'static str,
    known: Mutex<HashMap<String, Option<String>>>,
}

impl Keys {
    pub(crate) fn open(dir: &Path) -> Self {
        let (store, kind) = open(dir);
        Self::with_store(store, kind)
    }

    fn with_store(store: Box<dyn KeyStore>, kind: &'static str) -> Self {
        Self {
            store,
            kind,
            known: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn get(&self, account: &str) -> Option<String> {
        if let Some(known) = moqui_lib::sync::lock(&self.known).get(account) {
            return known.clone();
        }
        let key = self.store.get(account);
        moqui_lib::sync::lock(&self.known).insert(account.to_string(), key.clone());
        key
    }

    pub(crate) fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        self.store.set(account, key)?;
        moqui_lib::sync::lock(&self.known).insert(account.to_string(), Some(key.to_string()));
        Ok(())
    }

    /// Forget the key; it is gone here whether or not the store could delete it (logged by the caller).
    pub(crate) fn delete(&self, account: &str) -> Result<(), Error> {
        moqui_lib::sync::lock(&self.known).insert(account.to_string(), None);
        self.store.delete(account)
    }

    /// The chat store passphrase of `account`'s crypto device `device` (the server's `/ChatKeys/storeKey` answer), kept
    /// so the page's encrypted store opens offline.
    pub(crate) fn chat_store_key(&self, account: &str, device: &str) -> Option<String> {
        self.chat_store_keys(account)
            .get(device)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Keep the chat store passphrase of a device of `account`, replacing the one kept (a revision since changes it).
    /// One entry per account holds its devices' (`<account>.chat-store`), so the account's go with one delete.
    pub(crate) fn set_chat_store_key(&self, account: &str, device: &str, key: &str) -> Result<(), Error> {
        let mut keys = self.chat_store_keys(account);
        if keys.get(device).and_then(Value::as_str) == Some(key) {
            return Ok(());
        }
        keys.insert(device.to_string(), Value::String(key.to_string()));
        self.set(&chat_store_entry(account), &Value::Object(keys).to_string())
    }

    /// Forget the chat store passphrase of a device of `account` (revoked, or no longer the server's).
    pub(crate) fn remove_chat_store_key(&self, account: &str, device: &str) -> Result<(), Error> {
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
    pub(crate) fn forget_chat_store_keys(&self, account: &str) -> Result<(), Error> {
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

/// The key store entry of an account's chat store passphrases: an account id is a host and a hex digest, so this never
/// names another account's device key.
fn chat_store_entry(account: &str) -> String {
    format!("{account}.chat-store")
}

/// The key store, and `os` or `file` for which it is. The OS store unless it is not there, then the file in `dir`.
fn open(dir: &Path) -> (Box<dyn KeyStore>, &'static str) {
    let variable = std::env::var("MOQUI_DESKTOP_KEY_STORE").ok();
    if chosen(variable.as_deref(), tauri::is_dev()) == "os" {
        match OsKeyStore::available() {
            Ok(()) => return (Box::new(OsKeyStore), "os"),
            Err(err) => tracing::warn!(
                "No OS key store ({err}): the device keys are kept in {}, readable by this user only",
                dir.display()
            ),
        }
    }
    (Box::new(FileKeyStore::new(dir)), "file")
}

#[cfg(test)]
mod tests {
    use super::{chosen, Keys};
    use moqui_client::keys::FileKeyStore;

    #[test]
    fn chat_store_keys_are_kept_per_device_and_go_with_the_account() {
        let dir = std::env::temp_dir().join(format!("moqui-desktop-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let keys = Keys::with_store(Box::new(FileKeyStore::new(&dir)), "file");
        keys.set("a", "device-key").unwrap();
        keys.set_chat_store_key("a", "DEV1", "k1").unwrap();
        keys.set_chat_store_key("a", "DEV2", "k2").unwrap();
        keys.set_chat_store_key("a", "DEV1", "k1b").unwrap();
        assert_eq!(keys.chat_store_key("a", "DEV1").as_deref(), Some("k1b"));
        assert_eq!(keys.chat_store_key("b", "DEV1"), None);
        keys.remove_chat_store_key("a", "DEV2").unwrap();
        assert_eq!(keys.chat_store_key("a", "DEV2"), None);
        // read again from the file, not the memory
        let again = Keys::with_store(Box::new(FileKeyStore::new(&dir)), "file");
        assert_eq!(again.chat_store_key("a", "DEV1").as_deref(), Some("k1b"));
        again.forget_chat_store_keys("a").unwrap();
        assert_eq!(again.chat_store_key("a", "DEV1"), None);
        // the device key is another entry
        assert_eq!(again.get("a").as_deref(), Some("device-key"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_variable_chooses_and_a_development_run_keeps_its_keys_in_a_file() {
        assert_eq!(chosen(None, false), "os");
        assert_eq!(chosen(None, true), "file");
        assert_eq!(chosen(Some("os"), true), "os");
        assert_eq!(chosen(Some(" file "), false), "file");
        assert_eq!(chosen(Some("vault"), false), "os");
        assert_eq!(chosen(Some(""), true), "file");
    }
}
