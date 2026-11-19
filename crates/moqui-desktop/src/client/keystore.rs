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
//! is a new unsigned binary, which the Keychain would ask about again. A run that has the OS store moves into it the
//! keys an earlier run left in the file ([`move_file_keys`]).

use std::path::Path;
use std::sync::OnceLock;

use moqui_client::keys::{FileKeyStore, KeyStore, Keys};
use moqui_lib::error::Error;

/// The service the keys are under in the OS key store.
const SERVICE: &str = crate::os::IDENTIFIER;

/// The OS key store.
struct OsKeyStore;

impl OsKeyStore {
    /// Makes this OS's store keyring-core's default, once: the Keychain, the Credential Manager or the Secret Service.
    fn open_store() -> Result<(), String> {
        static OPENED: OnceLock<Result<(), String>> = OnceLock::new();
        OPENED
            .get_or_init(|| {
                #[cfg(target_os = "macos")]
                let store = apple_native_keyring_store::keychain::Store::new();
                #[cfg(windows)]
                let store = windows_native_keyring_store::Store::new();
                #[cfg(all(unix, not(target_os = "macos")))]
                let store = zbus_secret_service_keyring_store::Store::new();
                keyring_core::set_default_store(store.map_err(|err| err.to_string())?);
                Ok(())
            })
            .clone()
    }

    /// Whether the store answers: a missing entry does, a store that is not there or locked out does not.
    fn available() -> Result<(), String> {
        Self::open_store()?;
        let entry = keyring_core::Entry::new(SERVICE, "availability-check").map_err(|err| err.to_string())?;
        match entry.get_password() {
            Ok(_) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn entry(account: &str) -> Result<keyring_core::Entry, Error> {
        keyring_core::Entry::new(SERVICE, account).map_err(|err| Error::new(format!("key store: {err}")))
    }
}

impl KeyStore for OsKeyStore {
    fn get(&self, account: &str) -> Option<String> {
        match Self::entry(account).ok()?.get_password() {
            Ok(key) => Some(key).filter(|key| !key.is_empty()),
            Err(keyring_core::Error::NoEntry) => None,
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
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
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
            Ok(()) => {
                let moved = move_file_keys(dir, &OsKeyStore);
                if moved > 0 {
                    tracing::info!("{moved} device keys moved from {} to the OS key store", dir.display());
                }
                return (Box::new(OsKeyStore), "os");
            }
            Err(err) => tracing::warn!(
                "No OS key store ({err}): the device keys are kept in {}, readable by this user only",
                dir.display()
            ),
        }
    }
    (Box::new(FileKeyStore::new(dir)), "file")
}

/// Where the app keeps a secret of its own, the backup key (`crate::backup_key`): the OS key store, or where there is
/// none, a file in a directory of its own that only the user can read, chosen as the device keys' store is ([`open`]).
/// Unlike [`Keys`], a read tells an entry that is not there from a store that did not answer, so no caller takes the
/// one for the other and makes a new secret over one it could not read.
pub(crate) enum Secrets {
    Os,
    File(std::path::PathBuf),
}

impl Secrets {
    /// The store for `dir`, the file store's directory.
    pub(crate) fn open(dir: &Path) -> Self {
        let variable = std::env::var("MOQUI_DESKTOP_KEY_STORE").ok();
        if chosen(variable.as_deref(), tauri::is_dev()) == "os" {
            match OsKeyStore::available() {
                Ok(()) => return Self::Os,
                Err(err) => tracing::warn!("No OS key store ({err}): the backup key is kept in {}", dir.display()),
            }
        }
        Self::File(dir.to_path_buf())
    }

    /// The secret `name`; none when the store holds none.
    ///
    /// # Errors
    ///
    /// Returns why when the store does not answer.
    pub(crate) fn read(&self, name: &str) -> Result<Option<String>, String> {
        let text = match self {
            Self::Os => match OsKeyStore::entry(name).map_err(|err| err.to_string())?.get_password() {
                Ok(text) => text,
                Err(keyring_core::Error::NoEntry) => return Ok(None),
                Err(err) => return Err(format!("the key store did not answer: {err}")),
            },
            Self::File(dir) => match std::fs::read_to_string(dir.join(format!("{name}.key"))) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(err) => return Err(format!("{}: {err}", dir.display())),
            },
        };
        Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty()))
    }

    /// Keep `value` as the secret `name`.
    ///
    /// # Errors
    ///
    /// Returns why when the store does not take it.
    pub(crate) fn write(&self, name: &str, value: &str) -> Result<(), String> {
        match self {
            Self::Os => OsKeyStore.set(name, value).map_err(|err| err.to_string()),
            Self::File(dir) => {
                moqui_lib::private_fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
                FileKeyStore::new(dir).set(name, value).map_err(|err| err.to_string())
            }
        }
    }
}

/// Move into `store` (the OS key store of this run) the keys a run with the file store left in `dir`
/// (`<account>.key`: a development run, `MOQUI_DESKTOP_KEY_STORE=file`, a session with no Secret Service), and remove
/// each file. Else it stays in plain text, never read again (the account shows signed out, and its sign-out and removal
/// go to the OS store only), while the key is still good on the server (plans/Security-Review.md SEC-CLI-111). An
/// account `store` already holds a key of keeps that one, and the file goes. One that does not move is kept and logged,
/// and the rest go on, as the phone seals its plain keys (`SealedFileStore::seal_plain_keys`). How many moved.
pub(crate) fn move_file_keys(dir: &Path, store: &dyn KeyStore) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let file = FileKeyStore::new(dir);
    let mut moved = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(account) = name.strip_suffix(".key") else {
            continue;
        };
        let Some(key) = file.get(account) else {
            continue;
        };
        if store.get(account).is_none() {
            if let Err(err) = store.set(account, &key) {
                tracing::error!("The device key of {account} was not moved to the OS key store, and is kept: {err}");
                continue;
            }
        }
        match file.delete(account) {
            Ok(()) => moved += 1,
            Err(err) => tracing::error!("The device key of {account} is in the OS key store but its file stays: {err}"),
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::{chosen, move_file_keys};
    use moqui_client::keys::{FileKeyStore, KeyStore};

    /// SEC-CLI-111: a run with the OS store takes the keys a file-store run left, keeps one it already holds, and leaves
    /// no key file behind; other files of the directory stay.
    #[test]
    fn a_run_with_the_os_store_moves_the_file_keys_into_it() {
        let base = std::env::temp_dir().join(format!("moqui-desktop-keys-{}", std::process::id()));
        let (dir, os) = (base.join("file"), base.join("os"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&os).unwrap();
        let (file, store) = (FileKeyStore::new(&dir), FileKeyStore::new(&os));
        file.set("example.com-0123456789ab", "old-key").unwrap();
        file.set("example.org-ba9876543210", "file-key").unwrap();
        file.set("example.org-ba9876543210.chat-store", "passphrase").unwrap();
        store.set("example.com-0123456789ab", "os-key").unwrap();
        std::fs::write(dir.join("accounts.json"), "{}").unwrap();

        assert_eq!(move_file_keys(&dir, &store), 3);
        assert_eq!(
            store.get("example.com-0123456789ab").as_deref(),
            Some("os-key"),
            "the OS store's own is kept"
        );
        assert_eq!(store.get("example.org-ba9876543210").as_deref(), Some("file-key"));
        assert_eq!(
            store.get("example.org-ba9876543210.chat-store").as_deref(),
            Some("passphrase")
        );
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["accounts.json".to_string()]);
        assert_eq!(move_file_keys(&dir, &store), 0);
        std::fs::remove_dir_all(&base).unwrap();
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
