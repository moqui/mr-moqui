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
//!
//! The device keys are used while the phone is locked (the outbox, an incoming call, a push), so their Keystore key is
//! not tied to the lock. What no background work reads, each account's chat store passphrases (`<account>.chat-store`,
//! asked for by the shell's page only), is sealed by a second Keystore key that opens only while the phone is
//! unlocked, from Android 9 ([`SealedFileStore::with_unlocked`], `<entry>.unlocked`; plans/Security-Review.md
//! SEC-CLI-113). One sealed by the first key before is moved to the second as it is read.

// The sealed store is used on Android only; its tests run everywhere.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::{Path, PathBuf};

use moqui_client::keys::KeyStore;
use moqui_client::vault::memory::Key32;
use moqui_lib::error::Error;

/// Seals and opens a key's bytes.
pub(crate) trait Sealer: Send + Sync {
    /// `plain` sealed.
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String>;

    /// `sealed` opened.
    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String>;
}

/// Keys sealed by a [`Sealer`] in `<dir>/<account>.sealed`; with a second sealer for an unlocked phone, the chat store
/// passphrases by it in `<dir>/<account>.chat-store.unlocked` ([`unlocked_only`]).
pub(crate) struct SealedFileStore<S: Sealer> {
    dir: PathBuf,
    sealer: S,
    unlocked: Option<S>,
}

/// Whether the store entry `entry` is one no background work reads, sealed by the key of an unlocked phone where there
/// is one: an account's chat store passphrases (`moqui_client::keys::Keys::chat_store_key`).
pub(crate) fn unlocked_only(entry: &str) -> bool {
    entry.ends_with(".chat-store")
}

impl<S: Sealer> SealedFileStore<S> {
    pub(crate) fn new(dir: &Path, sealer: S) -> Self {
        Self {
            dir: dir.to_path_buf(),
            sealer,
            unlocked: None,
        }
    }

    /// The store with `sealer` for what no background work reads ([`unlocked_only`]), a key that opens only while the
    /// phone is unlocked.
    pub(crate) fn with_unlocked(mut self, sealer: S) -> Self {
        self.unlocked = Some(sealer);
        self
    }

    /// The file of `account` with `suffix` (`sealed`, `unlocked`), or none for an id that could leave the directory.
    fn file_as(&self, account: &str, suffix: &str) -> Option<PathBuf> {
        let plain = !account.is_empty()
            && !account.starts_with('.')
            && account
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
        plain.then(|| self.dir.join(format!("{account}.{suffix}")))
    }

    /// The file of `account` sealed by the first key.
    fn file(&self, account: &str) -> Option<PathBuf> {
        self.file_as(account, "sealed")
    }

    /// The sealer of an unlocked phone, where `account` is an entry for it and there is one.
    fn unlocked_for(&self, account: &str) -> Option<&S> {
        self.unlocked.as_ref().filter(|_| unlocked_only(account))
    }

    /// The text `sealer` opens from `path`, or none (no file; a key that does not open now, logged).
    fn open_file(sealer: &S, path: &Path, account: &str, unlocked: bool) -> Option<String> {
        let sealed = std::fs::read(path).ok()?;
        match sealer.open(&sealed) {
            Ok(plain) => String::from_utf8(plain).ok().filter(|key| !key.is_empty()),
            Err(err) if unlocked => {
                // the phone is locked (or the key was invalidated): none now, asked again next time
                log::warn!("The key of {account} opens only on an unlocked phone: {err}");
                None
            }
            Err(err) => {
                // the Keystore's key is gone (the app's data cleared, a restore on another device): as no key
                log::error!("The key of {account} does not open: {err}");
                None
            }
        }
    }

    /// Seal `key` by `sealer` into `path`, owner-only, through a part file.
    fn write_sealed(&self, sealer: &S, path: &Path, key: &str) -> Result<(), Error> {
        let sealed = sealer
            .seal(key.as_bytes())
            .map_err(|err| Error::new(format!("the key store did not seal the key: {err}")))?;
        std::fs::create_dir_all(&self.dir).map_err(|err| Error::new(format!("{}: {err}", self.dir.display())))?;
        let mut temp = path.as_os_str().to_owned();
        temp.push(".part");
        let temp = PathBuf::from(temp);
        std::fs::write(&temp, &sealed).map_err(|err| Error::new(format!("{}: {err}", temp.display())))?;
        restrict(&temp);
        std::fs::rename(&temp, path).map_err(|err| Error::new(format!("{}: {err}", path.display())))
    }
}

/// Remove `path`; none there is not an error.
fn remove(path: &Path) -> Result<(), Error> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::new(format!("{}: {err}", path.display()))),
    }
}

impl<S: Sealer> KeyStore for SealedFileStore<S> {
    fn get(&self, account: &str) -> Option<String> {
        let path = self.file(account)?;
        let Some(unlocked) = self.unlocked_for(account) else {
            return Self::open_file(&self.sealer, &path, account, false);
        };
        let moved = self.file_as(account, "unlocked")?;
        if moved.exists() {
            return Self::open_file(unlocked, &moved, account, true);
        }
        // sealed by the first key before the second was there: moved to the second as it is read
        let key = Self::open_file(&self.sealer, &path, account, false)?;
        match self.write_sealed(unlocked, &moved, &key) {
            Ok(()) => {
                if let Err(err) = remove(&path) {
                    log::error!("The key of {account} is sealed again but its old file stays: {err}");
                }
            }
            Err(err) => log::warn!("The key of {account} stays sealed by the first key for now: {err}"),
        }
        Some(key)
    }

    fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        let path = self
            .file(account)
            .ok_or_else(|| Error::new(format!("not an account id: {account}")))?;
        let Some(unlocked) = self.unlocked_for(account) else {
            return self.write_sealed(&self.sealer, &path, key);
        };
        let moved = self
            .file_as(account, "unlocked")
            .ok_or_else(|| Error::new(format!("not an account id: {account}")))?;
        self.write_sealed(unlocked, &moved, key)?;
        remove(&path)
    }

    fn delete(&self, account: &str) -> Result<(), Error> {
        let Some(path) = self.file(account) else {
            return Ok(());
        };
        let moved = self.file_as(account, "unlocked");
        let first = remove(&path);
        moved.map_or(Ok(()), |moved| remove(&moved))?;
        first
    }

    fn fresh(&self, entry: &str) -> bool {
        self.unlocked_for(entry).is_some()
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

/// Seals and opens with a Keystore key that opens only after the user confirmed with the device's lock screen or
/// biometrics (`KeyVault.sealAuth`, `openAuth`): the vault's device memory keys (plans/Vault-Instances.md §3.5, V5).
pub(crate) trait AuthSealer: Send + Sync {
    /// `plain` sealed; refused on a phone with no secure lock screen.
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String>;

    /// `sealed` opened: none while the user has not confirmed (the page then asks for the lock or biometrics), and an
    /// empty value for a key the Keystore no longer has (invalidated by a new biometric, the lock screen removed).
    fn open(&self, sealed: &[u8]) -> Result<Option<Vec<u8>>, String>;
}

/// The vault's device memory keys on the phone: each sealed by the [`AuthSealer`] in `<dir>/<entry>.sealed`. A copy of
/// the files opens nothing without the Keystore and the user's confirmation.
pub(crate) struct DeviceLockKeys<S: AuthSealer> {
    files: SealedFileStore<NoSealer>,
    sealer: S,
}

/// The file side only: [`DeviceLockKeys`] seals itself, as its sealer answers differently.
pub(crate) struct NoSealer;

impl Sealer for NoSealer {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.to_vec())
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        Ok(sealed.to_vec())
    }
}

impl<S: AuthSealer> DeviceLockKeys<S> {
    pub(crate) fn new(dir: &Path, sealer: S) -> Self {
        Self {
            files: SealedFileStore::new(dir, NoSealer),
            sealer,
        }
    }
}

impl<S: AuthSealer> moqui_client::vault::memory::MemoryKeys for DeviceLockKeys<S> {
    fn kind(&self) -> &'static str {
        moqui_client::vault::memory::KIND_DEVICE_LOCK
    }

    fn put(&self, entry: &str, key: &Key32) -> Result<(), String> {
        use base64::Engine as _;
        let path = self.files.file(entry).ok_or_else(|| format!("not an entry: {entry}"))?;
        let sealed = self.sealer.seal(key.as_bytes())?;
        let text = base64::engine::general_purpose::STANDARD.encode(sealed);
        self.files
            .write_sealed(&self.files.sealer, &path, &text)
            .map_err(|err| err.to_string())
    }

    fn take(&self, entry: &str) -> Result<Option<Key32>, moqui_client::vault::memory::KeyError> {
        use base64::Engine as _;
        use moqui_client::vault::memory::KeyError;
        let Some(path) = self.files.file(entry) else {
            return Ok(None);
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(None);
        };
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .map_err(|err| KeyError::Failed(err.to_string()))?;
        match self.sealer.open(&sealed).map_err(KeyError::Failed)? {
            None => Err(KeyError::NeedsUnlock),
            Some(plain) if plain.is_empty() => Ok(None),
            Some(mut plain) => {
                let key = Key32::from_slice(&plain);
                plain.iter_mut().for_each(|byte| *byte = 0);
                key.map(Some).map_err(|err| KeyError::Failed(err.to_string()))
            }
        }
    }

    fn delete(&self, entry: &str) -> Result<(), String> {
        self.files.delete(entry).map_err(|err| err.to_string())
    }
}

/// The vault's device memory keys of `dir`'s client in the Keystore key that needs the device's lock, once per
/// process; none where the Keystore cannot make it (no secure lock screen), and the vault offers no device memory.
pub(crate) fn set_vault_memory_keys(core: &moqui_client::app::Client, dir: &Path) {
    if core.vault().has_memory_keys() {
        return;
    }
    #[cfg(target_os = "android")]
    match crate::android::AuthVault::ready() {
        Ok(vault) => core
            .vault()
            .set_memory_keys(Box::new(DeviceLockKeys::new(&dir.join("vault-keys"), vault))),
        Err(err) => log::warn!("No Keystore key for the vault's device memory (a screen lock is needed): {err}"),
    }
    #[cfg(not(target_os = "android"))]
    let _ = dir;
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
                let mut store = SealedFileStore::new(dir, vault);
                match crate::android::KeyVault::ready_unlocked() {
                    Ok(unlocked) => store = store.with_unlocked(unlocked),
                    Err(err) => log::warn!(
                        "The Keystore has no key for an unlocked phone (Android 9 and later); the chat store passphrases \
                         are sealed by the device keys' key: {err}"
                    ),
                }
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
