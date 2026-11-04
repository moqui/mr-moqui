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

//! The device keys on Android (plans/Mobile-Desktop.md "Remote applications and authentication"): each account's key
//! sealed by an AES-GCM key of the Android Keystore, which never leaves it (`KeyVault.kt`, called through JNI,
//! `crate::android`), and the sealed bytes kept in a file of the app's private files directory
//! (`client/keys/<account>.sealed`, owner-only). A copy of the files is no use without the device's Keystore.
//!
//! [`SealedFileStore`] is the file side over any [`Sealer`]; where the Keystore cannot be used (it refused to make its
//! key) the app falls back to `moqui_client::keys::FileKeyStore` in the same private directory, which Android keeps
//! from other apps, and the shell says the keys are in a file.

// The sealed store is used on Android only; its tests run everywhere.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::{Path, PathBuf};

use moqui_client::keys::KeyStore;
use moqui_lib::error::Error;

/// Seals and opens a key's bytes.
pub(crate) trait Sealer: Send + Sync {
    /// `plain` sealed.
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String>;

    /// `sealed` opened.
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String>;
}

/// Keys sealed by a [`Sealer`] in `<dir>/<account>.sealed`.
pub(crate) struct SealedFileStore<S: Sealer> {
    dir: PathBuf,
    sealer: S,
}

impl<S: Sealer> SealedFileStore<S> {
    pub(crate) fn new(dir: &Path, sealer: S) -> Self {
        Self {
            dir: dir.to_path_buf(),
            sealer,
        }
    }

    /// The file of `account`, or none for an id that could leave the directory.
    fn file(&self, account: &str) -> Option<PathBuf> {
        let plain = !account.is_empty()
            && !account.starts_with('.')
            && account
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        plain.then(|| self.dir.join(format!("{account}.sealed")))
    }
}

impl<S: Sealer> KeyStore for SealedFileStore<S> {
    fn get(&self, account: &str) -> Option<String> {
        let path = self.file(account)?;
        let sealed = std::fs::read(&path).ok()?;
        match self.sealer.open(&sealed) {
            Ok(plain) => String::from_utf8(plain).ok().filter(|key| !key.is_empty()),
            Err(err) => {
                // the Keystore's key is gone (the app's data cleared, a restore on another device): as no key
                log::error!("The key of {account} does not open: {err}");
                None
            }
        }
    }

    fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        let path = self
            .file(account)
            .ok_or_else(|| Error::new(format!("not an account id: {account}")))?;
        let sealed = self
            .sealer
            .seal(key.as_bytes())
            .map_err(|err| Error::new(format!("the key store did not seal the key: {err}")))?;
        std::fs::create_dir_all(&self.dir).map_err(|err| Error::new(format!("{}: {err}", self.dir.display())))?;
        let temp = path.with_extension("sealed.part");
        std::fs::write(&temp, &sealed).map_err(|err| Error::new(format!("{}: {err}", temp.display())))?;
        restrict(&temp);
        std::fs::rename(&temp, &path).map_err(|err| Error::new(format!("{}: {err}", path.display())))
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

/// Only the owner reads `path` (the app's files are private already; this keeps it so if the directory changes).
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            log::warn!("{} is not owner-only: {err}", path.display());
        }
    }
}

impl<S: Sealer> SealedFileStore<S> {
    /// Seal the plain keys a run without the Keystore left in the directory (`FileKeyStore`'s `<account>.key`), and
    /// remove each once it is sealed; one that does not seal is kept and logged, and the rest go on. How many moved.
    pub(crate) fn seal_plain_keys(&self) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        let plain = moqui_client::keys::FileKeyStore::new(&self.dir);
        let mut moved = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(account) = name.strip_suffix(".key") else {
                continue;
            };
            let Some(key) = plain.get(account) else {
                continue;
            };
            if let Err(err) = self.set(account, &key) {
                log::error!("The plain key of {account} was not sealed, and is kept: {err}");
                continue;
            }
            match plain.delete(account) {
                Ok(()) => moved += 1,
                Err(err) => log::error!("The plain key of {account} is sealed but was not removed: {err}"),
            }
        }
        moved
    }
}

/// The app's keys: the Keystore's sealed files on Android when its key can be made (tried twice: a Keystore can be
/// busy just after boot), with any plain key a run without it left sealed and removed; else a plain owner-only file,
/// which the shell names (`keyStore: file`).
pub(crate) fn open_keys(dir: &Path) -> moqui_client::keys::Keys {
    #[cfg(target_os = "android")]
    {
        let vault = crate::android::KeyVault::ready().or_else(|err| {
            log::warn!("The Android Keystore did not answer; trying again: {err}");
            std::thread::sleep(std::time::Duration::from_millis(250));
            crate::android::KeyVault::ready()
        });
        match vault {
            Ok(vault) => {
                let store = SealedFileStore::new(dir, vault);
                let moved = store.seal_plain_keys();
                if moved > 0 {
                    log::info!("{moved} plain keys sealed by the Android Keystore");
                }
                return moqui_client::keys::Keys::new(Box::new(store), "os");
            }
            Err(err) => log::error!("The Android Keystore cannot be used; the keys are kept in a private file: {err}"),
        }
    }
    moqui_client::keys::Keys::new(Box::new(moqui_client::keys::FileKeyStore::new(dir)), "file")
}

#[cfg(test)]
#[path = "keystore_tests.rs"]
mod tests;
