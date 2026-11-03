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

//! The answers the shell opens on, which MD3 kept as files (`<dir>/<account>/<name>.json`) and MD4 keeps in the
//! account's [`crate::store`] as cached answers: [`migrate`] moves an account's files into its store once and removes
//! them; [`forget`] removes any left of an account that is removed before its store was opened.

use std::path::{Path, PathBuf};

use crate::store::Store;

/// The requests the shell opens on: its settings, apps and counts, kept by path alone (no query).
pub const PATHS: [&str; 3] = ["/qapps/clientConfig", "/shellData", "/shellCounts"];

/// Move the answers kept for `account` in `dir` (the MD3 cache directory) into its store, as answers of their paths
/// with no query, and remove the files. An answer the store already has (written since) is not replaced. A file that
/// cannot be read or kept is logged and left for the next try; the directory goes once it is empty. Gives how many
/// were moved.
pub fn migrate(dir: &Path, account: &str, store: &Store) -> usize {
    let Some(account_dir) = account_dir(dir, account) else {
        return 0;
    };
    if !account_dir.is_dir() {
        return 0;
    }
    let mut moved = 0;
    for path in PATHS {
        let file = account_dir.join(file_name(path));
        let Ok(body) = std::fs::read(&file) else { continue };
        if store.answer(path, "").is_none() {
            if let Err(err) = store.put_answer(path, "", "application/json", &body) {
                tracing::warn!("Client cache {} not moved into the store: {err}", file.display());
                continue;
            }
            moved += 1;
        }
        if let Err(err) = std::fs::remove_file(&file) {
            tracing::warn!("Client cache {} not removed: {err}", file.display());
        }
    }
    // only once empty: a file not moved stays for the next try
    if std::fs::remove_dir(&account_dir).is_ok() {
        let _ = std::fs::remove_dir(dir);
    }
    moved
}

/// Remove the files kept for `account` in `dir`, when the account is removed.
pub fn forget(dir: &Path, account: &str) {
    let Some(account_dir) = account_dir(dir, account) else {
        return;
    };
    match std::fs::remove_dir_all(&account_dir) {
        Ok(()) => {
            let _ = std::fs::remove_dir(dir);
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!("Client cache {} not removed: {err}", account_dir.display()),
    }
}

fn account_dir(dir: &Path, account: &str) -> Option<PathBuf> {
    crate::files::plain_name(account).then(|| dir.join(account))
}

/// The file name MD3 kept an answer of `path` in.
fn file_name(path: &str) -> String {
    format!("{}.json", path.trim_start_matches('/').replace('/', "_"))
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
