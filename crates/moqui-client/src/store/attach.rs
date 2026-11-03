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

//! The store's files: attachments saved while offline, beside the database in `<account>.files/`, which outbox
//! operations and drafts refer to by name. A file stays while one refers to it; one no row refers to is removed when
//! its row goes, or by [`Store::gc_files`] once it is older than a grace (so a file saved just before its row is
//! written is not lost).

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use super::Store;
use moqui_lib::error::Error;

/// How old a file no row refers to must be before [`Store::open`] removes it.
pub const FILE_GRACE: Duration = Duration::from_secs(10 * 60);

impl Store {
    /// Keep `bytes` as a file of the store, for an outbox operation or a draft to refer to, and give its name: a random
    /// part and `name` made plain, so two files of one name are apart. Refer to it from a row soon: one no row refers
    /// to is removed after a grace.
    ///
    /// # Errors
    ///
    /// Returns an error when there is no random source or the file cannot be written.
    pub fn save_file(&self, name: &str, bytes: &[u8]) -> Result<String, Error> {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).map_err(|err| Error::new(format!("random source failed: {err}")))?;
        // the last 80 characters, where the extension is
        let tail = name
            .char_indices()
            .rev()
            .nth(79)
            .and_then(|(at, _)| name.get(at..))
            .unwrap_or(name);
        let plain: String = tail
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let stored = format!("{}-{plain}", moqui_lib::codec::hex_encode(&random));
        crate::files::write_private(&self.files.join(&stored), bytes)
            .map_err(|err| Error::new(format!("Client store file {stored} not written: {err}")))?;
        Ok(stored)
    }

    /// Where the file `name` is kept, for a sender that streams it; none for a name that is not a plain file name.
    pub fn file_path(&self, name: &str) -> Option<PathBuf> {
        crate::files::plain_name(name).then(|| self.files.join(name))
    }

    /// The bytes of the file `name`; none when there is no such file, or it cannot be read (logged).
    pub fn read_file(&self, name: &str) -> Option<Vec<u8>> {
        match std::fs::read(self.file_path(name)?) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                tracing::warn!("Client store {}: file {name} not read: {err}", self.account);
                None
            }
        }
    }

    /// Remove the file `name`; none there is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error for a name that is not plain, or a file that cannot be removed.
    pub fn remove_file(&self, name: &str) -> Result<(), Error> {
        let Some(path) = self.file_path(name) else {
            return Err(Error::new(format!("Client store: {name} is not a file name")));
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::new(format!("Client store file {name} not removed: {err}"))),
        }
    }

    /// The bytes of the store's files, for a cap on what pages keep; files that cannot be read count none.
    pub fn files_bytes(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(&self.files) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|meta| meta.len())
            .sum()
    }

    /// Remove the files no outbox operation or draft refers to and older than `grace`; gives how many. Nothing is
    /// removed inside a unit (it could still be undone), nor when a row's file list cannot be read (it might refer to
    /// any of them).
    pub fn gc_files(&self, grace: Duration) -> usize {
        if !self.conn.is_autocommit() {
            return 0;
        }
        let Some(referred) = self.referred_files() else {
            return 0;
        };
        let Ok(entries) = std::fs::read_dir(&self.files) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if referred.contains(&name) || name.starts_with('.') {
                continue;
            }
            let old = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= grace);
            if !old {
                continue;
            }
            match self.remove_file(&name) {
                Ok(()) => removed += 1,
                Err(err) => tracing::warn!("Client store {}: {err}", self.account),
            }
        }
        removed
    }

    /// Remove the files of a removed row (its `files` column) that no row refers to now. Inside a unit, or when the
    /// list does not parse, they are left for [`Self::gc_files`].
    pub(super) fn release_files(&self, files: &str) {
        if !self.conn.is_autocommit() {
            return;
        }
        let Ok(names) = serde_json::from_str::<Vec<String>>(files) else {
            tracing::warn!(
                "Client store {}: a file list does not parse, left for the collector",
                self.account
            );
            return;
        };
        if names.is_empty() {
            return;
        }
        let Some(referred) = self.referred_files() else { return };
        for name in names.iter().filter(|name| !referred.contains(*name)) {
            if let Err(err) = self.remove_file(name) {
                tracing::warn!("Client store {}: {err}", self.account);
            }
        }
    }

    /// The files the outbox and the drafts refer to; none when a list cannot be read (logged).
    fn referred_files(&self) -> Option<HashSet<String>> {
        // not Store::rows, which gives none for a query that fails: here that would remove every file
        let lists: rusqlite::Result<Vec<String>> = self
            .conn
            .prepare_cached("SELECT files FROM outbox UNION ALL SELECT files FROM draft")
            .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect());
        let lists = match lists {
            Ok(lists) => lists,
            Err(err) => {
                tracing::warn!(
                    "Client store {}: file lists not read, no file removed: {err}",
                    self.account
                );
                return None;
            }
        };
        let mut referred = HashSet::new();
        for list in lists {
            match serde_json::from_str::<Vec<String>>(&list) {
                Ok(names) => referred.extend(names),
                Err(err) => {
                    tracing::warn!(
                        "Client store {}: a file list does not parse, no file removed: {err}",
                        self.account
                    );
                    return None;
                }
            }
        }
        Some(referred)
    }
}
