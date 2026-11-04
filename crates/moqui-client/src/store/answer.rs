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

//! Cached answers of the server, by path and query, for the scheme handler to answer from when the server cannot be
//! reached.

use rusqlite::{params, OptionalExtension};

use super::{failed, now_ms, text_len, Read, Store, ROW_BYTES};
use moqui_lib::error::Error;

/// A kept answer of the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub body: Vec<u8>,
    pub content_type: String,
    /// When it was kept, ms since the epoch.
    pub stored_ms: i64,
}

impl Store {
    /// Keep `body` as the answer of `path` with `query` (the request's query string, with or without its `?`).
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn put_answer(&self, path: &str, query: &str, content_type: &str, body: &[u8]) -> Result<(), Error> {
        let key = answer_key(path, query);
        let size = ROW_BYTES + text_len(&key) + text_len(content_type) + i64::try_from(body.len()).unwrap_or(i64::MAX);
        self.conn
            .prepare_cached(
                "INSERT INTO answer (key, body, content_type, stored_ms, read_ms, size) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT (key) DO UPDATE SET body = excluded.body, content_type = excluded.content_type,
                    stored_ms = excluded.stored_ms, read_ms = excluded.read_ms, size = excluded.size",
            )
            .and_then(|mut statement| statement.execute(params![key, body, content_type, now_ms(), self.tick(), size]))
            .map_err(failed("answer not kept"))?;
        self.wrote(size);
        Ok(())
    }

    /// The kept answer of `path` with `query`, which counts as a read for the size cap. None when there is none, or it
    /// cannot be read (logged).
    pub fn answer(&self, path: &str, query: &str) -> Option<Answer> {
        let key = answer_key(path, query);
        let found = self
            .conn
            .prepare_cached("SELECT body, content_type, stored_ms FROM answer WHERE key = ?1")
            .and_then(|mut statement| {
                statement
                    .query_row([&key], |row| {
                        Ok(Answer {
                            body: row.get(0)?,
                            content_type: row.get(1)?,
                            stored_ms: row.get(2)?,
                        })
                    })
                    .optional()
            });
        match found {
            Ok(Some(answer)) => {
                self.touch(Read::Answer(key));
                Some(answer)
            }
            Ok(None) => None,
            Err(err) => {
                tracing::warn!("Client store {}: answer {key} not read: {err}", self.account);
                None
            }
        }
    }

    /// Forget the answer of `path` with `query`.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be removed.
    pub fn remove_answer(&self, path: &str, query: &str) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM answer WHERE key = ?1", [answer_key(path, query)])
            .map_err(failed("answer not removed"))?;
        Ok(())
    }
}

impl Store {
    /// The keys of the kept answers whose key starts with `prefix` (a path, with or without the start of a query).
    pub fn answer_keys(&self, prefix: &str) -> Vec<String> {
        self.rows(
            "answers",
            "SELECT key FROM answer WHERE substr(key, 1, length(?1)) = ?1",
            [prefix],
            |row| row.get(0),
        )
    }

    /// Forget the answers whose key starts with `prefix` and passes `keep_out` (the cached reads of a feed reset);
    /// gives how many went.
    ///
    /// # Errors
    ///
    /// Returns an error when a row cannot be removed.
    pub fn remove_answers(&self, prefix: &str, keep_out: impl Fn(&str) -> bool) -> Result<usize, Error> {
        let keys: Vec<String> = self
            .answer_keys(prefix)
            .into_iter()
            .filter(|key| keep_out(key))
            .collect();
        self.atomic(|store| {
            for key in &keys {
                store
                    .conn
                    .execute("DELETE FROM answer WHERE key = ?1", [key])
                    .map_err(failed("answer not removed"))?;
            }
            Ok(keys.len())
        })
    }
}

/// The key of an answer: the path, and the query's parameters sorted by name (the order of one name's values kept),
/// so one request asked with its parameters in another order finds the same answer.
pub fn answer_key(path: &str, query: &str) -> String {
    let mut parameters: Vec<&str> = query
        .trim_start_matches('?')
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .collect();
    if parameters.is_empty() {
        return path.to_string();
    }
    parameters.sort_by_key(|parameter| parameter.split_once('=').map_or(*parameter, |(name, _)| name));
    format!("{path}?{}", parameters.join("&"))
}
