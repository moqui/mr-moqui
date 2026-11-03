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

//! Mail folders, threads and messages in the store, as the mail client's transitions give them; each row is the
//! server's JSON, kept as text. A thread is kept per folder, so one in two folders (a label) has a row in each, and
//! its messages are kept once. A folder page or a thread read again replaces what the store had for it, so mail moved
//! or deleted on the server goes from the store too. Folders are few and small: never evicted.

use rusqlite::params;

use super::{failed, text_len, Read, Store, ROW_BYTES};
use moqui_lib::error::Error;

/// A folder of the folder list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailFolder {
    pub folder_id: String,
    /// The folder's JSON.
    pub row: String,
}

/// A thread as a folder's list gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailThread {
    pub folder_id: String,
    pub thread_id: String,
    /// What the folder's list sorts by, newest first: the thread's last message date, ms since the epoch.
    pub sort_ms: i64,
    /// The thread's JSON.
    pub row: String,
}

/// A message of a thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailMessage {
    pub message_id: String,
    pub thread_id: String,
    /// Its date, ms since the epoch: a thread's messages are read oldest first.
    pub sort_ms: i64,
    /// The message's JSON.
    pub row: String,
}

impl Store {
    /// Keep the whole folder list: each folder written, the folders not in it removed (their threads stay until their
    /// own folder pages or eviction remove them). A folder that cannot be written is logged and left out; gives how
    /// many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit cannot begin or end, or the old folders cannot be removed.
    pub fn replace_mail_folders(&self, folders: &[MailFolder]) -> Result<usize, Error> {
        self.atomic(|store| {
            store
                .conn
                .execute("DELETE FROM mail_folder", [])
                .map_err(failed("mail folders not removed"))?;
            store.put_each("mail folder", folders, |store, folder| {
                let size = ROW_BYTES + text_len(&folder.folder_id) + text_len(&folder.row);
                store
                    .conn
                    .execute(
                        "INSERT INTO mail_folder (folder_id, row, size) VALUES (?1, ?2, ?3)
                        ON CONFLICT (folder_id) DO UPDATE SET row = excluded.row, size = excluded.size",
                        params![folder.folder_id, folder.row, size],
                    )
                    .map(|_| size)
                    .map_err(|err| (folder.folder_id.clone(), err))
            })
        })
    }

    /// The kept folders, by id.
    pub fn mail_folders(&self) -> Vec<MailFolder> {
        self.rows(
            "mail folders",
            "SELECT folder_id, row FROM mail_folder ORDER BY folder_id",
            [],
            |row| {
                Ok(MailFolder {
                    folder_id: row.get(0)?,
                    row: row.get(1)?,
                })
            },
        )
    }

    /// Keep threads, replacing their rows in their folders; writing counts as a read. A thread that cannot be written
    /// is logged and left out; gives how many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit of writes cannot begin or end.
    pub fn put_mail_threads(&self, threads: &[MailThread]) -> Result<usize, Error> {
        self.put_each("mail thread", threads, |store, thread| {
            store
                .write_mail_thread(thread)
                .map_err(|err| (thread.thread_id.clone(), err))
        })
    }

    /// Keep a page of a folder the server gave, asked `before_ms` (none for the latest): its threads written, and the
    /// folder's other threads in the stretch the page covers taken out of the folder. The stretch runs from the page's
    /// oldest thread (or from the folder's start when `has_more` is false) to `before_ms`. Gives how many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit cannot begin or end, or a thread cannot be taken out.
    pub fn replace_mail_page(
        &self,
        folder_id: &str,
        before_ms: Option<i64>,
        threads: &[MailThread],
        has_more: bool,
    ) -> Result<usize, Error> {
        self.atomic(|store| {
            let oldest = threads.iter().map(|thread| thread.sort_ms).min();
            let from = match (has_more, oldest) {
                (false, _) => Some(i64::MIN),
                (true, oldest) => oldest,
            };
            if let Some(from) = from {
                let covered: Vec<String> = store.rows(
                    "mail threads",
                    "SELECT thread_id FROM mail_thread WHERE folder_id = ?1 AND sort_ms >= ?2
                        AND (?3 IS NULL OR sort_ms < ?3)",
                    params![folder_id, from, before_ms],
                    |row| row.get(0),
                );
                let gone = covered
                    .iter()
                    .filter(|id| !threads.iter().any(|thread| &thread.thread_id == *id));
                for thread_id in gone {
                    store.remove_mail_thread(folder_id, thread_id)?;
                }
            }
            store.put_mail_threads(threads)
        })
    }

    /// A page of a folder's threads, newest first: the `limit` before `before_ms` (ms since the epoch), or the latest.
    pub fn mail_threads(&self, folder_id: &str, before_ms: Option<i64>, limit: u32) -> Vec<MailThread> {
        self.rows(
            "mail threads",
            "SELECT folder_id, thread_id, sort_ms, row FROM mail_thread
            WHERE folder_id = ?1 AND (?2 IS NULL OR sort_ms < ?2) ORDER BY sort_ms DESC, thread_id LIMIT ?3",
            params![folder_id, before_ms, limit.max(1)],
            |row| {
                Ok(MailThread {
                    folder_id: row.get(0)?,
                    thread_id: row.get(1)?,
                    sort_ms: row.get(2)?,
                    row: row.get(3)?,
                })
            },
        )
    }

    /// Move a thread from one folder to another (archive, move), as one unit: its row in `to` is the one it had in
    /// `from`, unless `to` has it already. False when `from` does not have it.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be written.
    pub fn move_mail_thread(&self, thread_id: &str, from: &str, to: &str) -> Result<bool, Error> {
        if from == to {
            return Ok(self
                .mail_threads_of(thread_id)
                .iter()
                .any(|thread| thread.folder_id == from));
        }
        self.atomic(|store| {
            let moved = store
                .conn
                .execute(
                    "INSERT OR IGNORE INTO mail_thread (folder_id, thread_id, sort_ms, row, read_ms, size)
                    SELECT ?3, thread_id, sort_ms, row, read_ms, size FROM mail_thread
                        WHERE folder_id = ?2 AND thread_id = ?1",
                    params![thread_id, from, to],
                )
                .and_then(|_| {
                    store.conn.execute(
                        "DELETE FROM mail_thread WHERE folder_id = ?1 AND thread_id = ?2",
                        params![from, thread_id],
                    )
                })
                .map_err(failed("mail thread not moved"))?;
            Ok(moved > 0)
        })
    }

    /// Take a thread out of a folder (moved, or evicted); its messages go too when no folder keeps it.
    ///
    /// # Errors
    ///
    /// Returns an error when the rows cannot be removed.
    pub fn remove_mail_thread(&self, folder_id: &str, thread_id: &str) -> Result<(), Error> {
        self.atomic(|store| {
            store
                .conn
                .execute(
                    "DELETE FROM mail_thread WHERE folder_id = ?1 AND thread_id = ?2",
                    params![folder_id, thread_id],
                )
                .and_then(|_| {
                    store.conn.execute(
                        "DELETE FROM mail_message WHERE thread_id = ?1
                            AND NOT EXISTS (SELECT 1 FROM mail_thread WHERE thread_id = ?1)",
                        [thread_id],
                    )
                })
                .map_err(failed("mail thread not removed"))?;
            Ok(())
        })
    }

    /// Keep messages, replacing any of the same id. A message that cannot be written is logged and left out; gives how
    /// many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit of writes cannot begin or end.
    pub fn put_mail_messages(&self, messages: &[MailMessage]) -> Result<usize, Error> {
        self.put_each("mail message", messages, |store, message| {
            let size =
                ROW_BYTES + text_len(&message.message_id) + text_len(&message.thread_id) + text_len(&message.row);
            store
                .conn
                .prepare_cached(
                    "INSERT INTO mail_message (message_id, thread_id, sort_ms, row, size) VALUES (?1, ?2, ?3, ?4, ?5)
                    ON CONFLICT (message_id) DO UPDATE SET thread_id = excluded.thread_id,
                        sort_ms = excluded.sort_ms, row = excluded.row, size = excluded.size",
                )
                .and_then(|mut statement| {
                    statement.execute(params![
                        message.message_id,
                        message.thread_id,
                        message.sort_ms,
                        message.row,
                        size
                    ])
                })
                .map(|_| size)
                .map_err(|err| (message.message_id.clone(), err))
        })
    }

    /// Keep a thread as the server gave it whole: its messages written and the ones it no longer has (deleted on the
    /// server) removed. Gives how many were kept.
    ///
    /// # Errors
    ///
    /// Returns an error when the unit cannot begin or end, or the old messages cannot be removed.
    pub fn replace_thread_messages(&self, thread_id: &str, messages: &[MailMessage]) -> Result<usize, Error> {
        self.atomic(|store| {
            let kept_ids: Vec<String> = store.rows(
                "mail messages",
                "SELECT message_id FROM mail_message WHERE thread_id = ?1",
                [thread_id],
                |row| row.get(0),
            );
            for gone in kept_ids
                .iter()
                .filter(|id| !messages.iter().any(|message| &message.message_id == *id))
            {
                store
                    .conn
                    .execute("DELETE FROM mail_message WHERE message_id = ?1", [gone])
                    .map_err(failed("mail message not removed"))?;
            }
            store.put_mail_messages(messages)
        })
    }

    /// A thread's messages, oldest first; reading counts as a read of the thread in each folder.
    pub fn mail_messages(&self, thread_id: &str) -> Vec<MailMessage> {
        self.touch(Read::Thread(thread_id.to_string()));
        self.rows(
            "mail messages",
            "SELECT message_id, thread_id, sort_ms, row FROM mail_message WHERE thread_id = ?1
            ORDER BY sort_ms, message_id",
            [thread_id],
            |row| {
                Ok(MailMessage {
                    message_id: row.get(0)?,
                    thread_id: row.get(1)?,
                    sort_ms: row.get(2)?,
                    row: row.get(3)?,
                })
            },
        )
    }

    /// The rows of a thread in each folder that keeps it (a state change made offline is applied to each).
    pub fn mail_threads_of(&self, thread_id: &str) -> Vec<MailThread> {
        self.rows(
            "mail threads",
            "SELECT folder_id, thread_id, sort_ms, row FROM mail_thread WHERE thread_id = ?1",
            [thread_id],
            |row| {
                Ok(MailThread {
                    folder_id: row.get(0)?,
                    thread_id: row.get(1)?,
                    sort_ms: row.get(2)?,
                    row: row.get(3)?,
                })
            },
        )
    }

    fn write_mail_thread(&self, thread: &MailThread) -> rusqlite::Result<i64> {
        let size = ROW_BYTES + text_len(&thread.folder_id) + text_len(&thread.thread_id) + text_len(&thread.row);
        self.conn
            .prepare_cached(
                "INSERT INTO mail_thread (folder_id, thread_id, sort_ms, row, read_ms, size)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                ON CONFLICT (folder_id, thread_id) DO UPDATE SET sort_ms = excluded.sort_ms, row = excluded.row,
                    read_ms = excluded.read_ms, size = excluded.size",
            )
            .and_then(|mut statement| {
                statement.execute(params![
                    thread.folder_id,
                    thread.thread_id,
                    thread.sort_ms,
                    thread.row,
                    self.tick(),
                    size
                ])
            })
            .map(|_| size)
    }
}
