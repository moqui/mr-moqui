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

//! The last answers of a few requests the shell opens on, kept per account, so the shell starts with no network
//! (plans/Mobile-Desktop.md §8, MD3: "the app opens its UI without a network"). The host keeps a good answer of one of
//! [`PATHS`] and serves it, marked offline, when the server cannot be reached. Durable client data is MD4's.

use std::path::{Path, PathBuf};

/// The requests whose answers are kept: the shell's settings, apps and counts. Query strings are not part of the name,
/// so only these exact paths are kept.
pub const PATHS: [&str; 3] = ["/qapps/clientConfig", "/shellData", "/shellCounts"];

/// Answers kept in `<dir>/<account>/<name>.json`.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf() }
    }

    /// The kept answer of `path` for `account`.
    pub fn get(&self, account: &str, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.file(account, path)?).ok()
    }

    /// Keep `body` as the answer of `path` for `account`; a path not in [`PATHS`] is not kept. A failure is logged:
    /// the cache only helps.
    pub fn put(&self, account: &str, path: &str, body: &[u8]) {
        let Some(file) = self.file(account, path) else { return };
        let written = crate::files::write_private(&file, body);
        if let Err(err) = written {
            tracing::warn!("Client cache {} not written: {err}", file.display());
        }
    }

    /// Forget an account's answers, when it is removed.
    pub fn forget(&self, account: &str) {
        let Some(dir) = self.account_dir(account) else { return };
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!("Client cache {} not removed: {err}", dir.display()),
        }
    }

    fn account_dir(&self, account: &str) -> Option<PathBuf> {
        let plain = !account.is_empty()
            && !account.starts_with('.')
            && account
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
        plain.then(|| self.dir.join(account))
    }

    fn file(&self, account: &str, path: &str) -> Option<PathBuf> {
        if !PATHS.contains(&path) {
            return None;
        }
        let name = path.trim_start_matches('/').replace('/', "_");
        Some(self.account_dir(account)?.join(format!("{name}.json")))
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
