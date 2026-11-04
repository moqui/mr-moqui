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

//! What the client keeps of its sync with the server's MD5 contracts (docs/ui/Desktop-Client.md "The server's sync
//! contracts"): where each changes feed was read to ([`SyncCursor`]: the user's chat feed and its `headsSince`, each
//! room's message changes, the mail feed), and how far a file sent in chunks got ([`Upload`]). Neither is a cached
//! read: the size cap never evicts them. A room's cursor goes with the room ([`Store::remove_chat_room`]), an
//! upload with its operation ([`Store::outbox_remove`]).

use rusqlite::{params, OptionalExtension};

use super::{failed, now_ms, Store};
use moqui_lib::error::Error;

/// Where a feed was read to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncCursor {
    /// The feed's `lastSeq` (or `headsSince`); none when the server gave none.
    pub seq: Option<i64>,
    /// A word beside it: `unsupported` for a mail feed the server does not keep.
    pub value: Option<String>,
}

/// A file of an operation sent in chunks (`uploadChunk`), by the store's file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub op_id: String,
    pub file: String,
    /// The client's own name of the upload on the server.
    pub upload_id: String,
    /// The bytes the server has taken, as it last said.
    pub offset: i64,
    pub total: i64,
}

impl Store {
    /// The cursor `name`; none when the feed was never read (or it cannot be read, logged).
    pub fn sync_cursor(&self, name: &str) -> Option<SyncCursor> {
        let found = self
            .conn
            .prepare_cached("SELECT seq, value FROM sync_cursor WHERE name = ?1")
            .and_then(|mut statement| {
                statement
                    .query_row([name], |row| {
                        Ok(SyncCursor {
                            seq: row.get(0)?,
                            value: row.get(1)?,
                        })
                    })
                    .optional()
            });
        found.unwrap_or_else(|err| {
            tracing::warn!("Client store {}: sync cursor {name} not read: {err}", self.account);
            None
        })
    }

    /// The seq of the cursor `name`, if it has one.
    pub fn sync_seq(&self, name: &str) -> Option<i64> {
        self.sync_cursor(name).and_then(|cursor| cursor.seq)
    }

    /// Keep the cursor `name`.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn set_sync_cursor(&self, name: &str, cursor: &SyncCursor) -> Result<(), Error> {
        self.conn
            .prepare_cached(
                "INSERT INTO sync_cursor (name, seq, value, updated_ms) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (name) DO UPDATE SET seq = excluded.seq, value = excluded.value,
                    updated_ms = excluded.updated_ms",
            )
            .and_then(|mut statement| statement.execute(params![name, cursor.seq, cursor.value, now_ms()]))
            .map_err(failed("sync cursor not kept"))?;
        Ok(())
    }

    /// Keep the cursor `name` at `seq`, with no value.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn set_sync_seq(&self, name: &str, seq: i64) -> Result<(), Error> {
        self.set_sync_cursor(
            name,
            &SyncCursor {
                seq: Some(seq),
                value: None,
            },
        )
    }

    /// Forget the cursors whose name starts with `prefix` (all of them for an empty one).
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be removed.
    pub fn remove_sync_cursors(&self, prefix: &str) -> Result<usize, Error> {
        self.conn
            .execute(
                "DELETE FROM sync_cursor WHERE substr(name, 1, length(?1)) = ?1",
                [prefix],
            )
            .map_err(failed("sync cursors not removed"))
    }

    /// The chunked upload of an operation's file; none when it was not started.
    pub fn upload(&self, op_id: &str, file: &str) -> Option<Upload> {
        self.rows(
            "upload",
            "SELECT op_id, file, upload_id, offset, total FROM upload WHERE op_id = ?1 AND file = ?2",
            params![op_id, file],
            |row| {
                Ok(Upload {
                    op_id: row.get(0)?,
                    file: row.get(1)?,
                    upload_id: row.get(2)?,
                    offset: row.get(3)?,
                    total: row.get(4)?,
                })
            },
        )
        .pop()
    }

    /// Keep how far a chunked upload got.
    ///
    /// # Errors
    ///
    /// Returns an error when the row cannot be written.
    pub fn put_upload(&self, upload: &Upload) -> Result<(), Error> {
        self.conn
            .prepare_cached(
                "INSERT INTO upload (op_id, file, upload_id, offset, total) VALUES (?1, ?2, ?3, ?4, ?5)
                ON CONFLICT (op_id, file) DO UPDATE SET upload_id = excluded.upload_id, offset = excluded.offset,
                    total = excluded.total",
            )
            .and_then(|mut statement| {
                statement.execute(params![
                    upload.op_id,
                    upload.file,
                    upload.upload_id,
                    upload.offset,
                    upload.total
                ])
            })
            .map_err(failed("upload not kept"))?;
        Ok(())
    }

    /// Forget the chunked uploads of an operation (removed, or sent).
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be removed.
    pub fn remove_uploads(&self, op_id: &str) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM upload WHERE op_id = ?1", [op_id])
            .map_err(failed("uploads not removed"))?;
        Ok(())
    }

    /// At open: the uploads of operations no longer kept go.
    pub(super) fn drop_orphan_uploads(&self) {
        if let Err(err) = self.conn.execute(
            "DELETE FROM upload WHERE NOT EXISTS (SELECT 1 FROM outbox WHERE outbox.id = upload.op_id)",
            [],
        ) {
            tracing::warn!("Client store {}: old uploads not dropped: {err}", self.account);
        }
    }
}
