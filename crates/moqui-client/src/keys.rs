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

use std::path::{Path, PathBuf};

use moqui_lib::error::Error;

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

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
