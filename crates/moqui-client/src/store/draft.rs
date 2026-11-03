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

//! Drafts: what the page keeps while the user writes (a chat message, a compose), by the page's key, with the files
//! attached to it while offline. Never evicted.

use rusqlite::{params, OptionalExtension};

use super::{failed, files_json, now_ms, Store};
use moqui_lib::error::Error;

/// A draft: the JSON text the page keeps under its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub key: String,
    pub value: String,
    /// The store's files attached to it ([`Store::save_file`]).
    pub files: Vec<String>,
    /// When it was last written, ms since the epoch.
    pub updated_ms: i64,
}

impl Store {
    /// Keep `value` (JSON text) and its `files` as the draft of `key`, replacing any.
    ///
    /// # Errors
    ///
    /// Returns an error for a file name that is not plain, or when the row cannot be written.
    pub fn put_draft(&self, key: &str, value: &str, files: &[String]) -> Result<(), Error> {
        let files = files_json(files)?;
        self.conn
            .execute(
                "INSERT INTO draft (key, value, files, updated_ms) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (key) DO UPDATE SET value = excluded.value, files = excluded.files,
                    updated_ms = excluded.updated_ms",
                params![key, value, files, now_ms()],
            )
            .map_err(failed("draft not kept"))?;
        Ok(())
    }

    /// The draft of `key`; none when there is none, or it cannot be read (logged).
    pub fn draft(&self, key: &str) -> Option<Draft> {
        let found = self
            .conn
            .query_row(
                "SELECT key, value, files, updated_ms FROM draft WHERE key = ?1",
                [key],
                draft_of,
            )
            .optional();
        found.unwrap_or_else(|err| {
            tracing::warn!("Client store {}: draft {key} not read: {err}", self.account);
            None
        })
    }

    /// The drafts whose key starts with `prefix` (all for an empty one), by key. A row that cannot be read is logged and
    /// left out.
    pub fn drafts(&self, prefix: &str) -> Vec<Draft> {
        self.rows(
            "drafts",
            "SELECT key, value, files, updated_ms FROM draft WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key",
            [prefix],
            draft_of,
        )
    }

    /// Forget the draft of `key`, and its files that nothing else refers to; false when there was none.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be removed.
    pub fn remove_draft(&self, key: &str) -> Result<bool, Error> {
        let files: Option<String> = self
            .conn
            .query_row("DELETE FROM draft WHERE key = ?1 RETURNING files", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(failed("draft not removed"))?;
        let Some(files) = files else { return Ok(false) };
        self.release_files(&files);
        Ok(true)
    }
}

fn draft_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Draft> {
    let files: String = row.get(2)?;
    Ok(Draft {
        key: row.get(0)?,
        value: row.get(1)?,
        files: serde_json::from_str(&files)
            .map_err(|err| rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err)))?,
        updated_ms: row.get(3)?,
    })
}
