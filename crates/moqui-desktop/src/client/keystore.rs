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

use std::path::Path;

use moqui_client::keys::{FileKeyStore, KeyStore, Keys};
use moqui_lib::error::Error;

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

/// The client's device keys (`moqui_client::keys::Keys`, kept in memory once read) in the OS key store unless it is not
/// there, then in a file in `dir`.
pub(crate) fn open_keys(dir: &Path) -> Keys {
    let (store, kind) = open(dir);
    Keys::new(store, kind)
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
    use super::chosen;

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
