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

//! The size cap: the cached reads (answers, rooms with their events, mail threads with their messages) are evicted,
//! least recently read first, until they fit beside what is never evicted (the outbox, drafts, files, mail folders).
//! The cached reads always keep a share of the cap ([`READS_FLOOR`]), so a large queued attachment does not empty
//! them on every write.

use super::{failed, Store, ROW_BYTES};
use moqui_lib::error::Error;

/// The share of the cap (1/n) the cached reads may keep however much is pinned.
const READS_FLOOR: i64 = 4;

/// The bytes the cap counts, apart.
pub(super) struct Bytes {
    /// The cached reads, which may be evicted.
    pub(super) reads: i64,
    /// The outbox, drafts, files and mail folders, which are not.
    pub(super) pinned: i64,
}

impl Store {
    /// The bytes the size cap counts: the rows as [`ROW_BYTES`] and their text, and the files.
    pub fn size_bytes(&self) -> u64 {
        let bytes = self.bytes();
        u64::try_from(bytes.reads.saturating_add(bytes.pinned)).unwrap_or(0)
    }

    /// Evict the least recently read cached reads until they fit in the cap beside what is pinned, but keeping at least
    /// a quarter of the cap for them: answers, rooms with their events, and threads with their messages (events and
    /// messages of no kept room or thread first). Gives how many were evicted. Writes call it as they add up, so a
    /// caller needs it only after changing the cap.
    ///
    /// # Errors
    ///
    /// Returns an error when the candidates cannot be read or one cannot be removed (what was removed before it stays).
    pub fn trim(&self) -> Result<usize, Error> {
        self.written.set(0);
        self.flush_reads();
        let mut evicted = 0;
        // A thread in two folders counts its messages in both, so one pass can stop short: a few more passes settle it.
        for _ in 0..4 {
            let bytes = self.bytes();
            let room = (self.max_bytes - bytes.pinned).max(self.max_bytes / READS_FLOOR);
            if bytes.reads <= room {
                break;
            }
            let removed = self.atomic(|store| store.evict(bytes.reads - room))?;
            if removed == 0 {
                break;
            }
            evicted += removed;
        }
        if evicted > 0 {
            if let Err(err) = self.conn.execute_batch("PRAGMA incremental_vacuum") {
                tracing::warn!("Client store {}: not vacuumed: {err}", self.account);
            }
        }
        Ok(evicted)
    }

    pub(super) fn bytes(&self) -> Bytes {
        let rows = self.conn.query_row(
            "SELECT (SELECT COALESCE(SUM(size), 0) FROM answer) + (SELECT COALESCE(SUM(size), 0) FROM chat_room)
                + (SELECT COALESCE(SUM(size), 0) FROM chat_event) + (SELECT COALESCE(SUM(size), 0) FROM mail_thread)
                + (SELECT COALESCE(SUM(size), 0) FROM mail_message),
            (SELECT COALESCE(SUM(size), 0) FROM mail_folder)
                + (SELECT COALESCE(SUM(?1 + LENGTH(CAST(payload AS BLOB)) + LENGTH(CAST(files AS BLOB))
                    + COALESCE(LENGTH(CAST(error AS BLOB)), 0)), 0) FROM outbox)
                + (SELECT COALESCE(SUM(?1 + LENGTH(CAST(key AS BLOB)) + LENGTH(CAST(value AS BLOB))
                    + LENGTH(CAST(files AS BLOB))), 0) FROM draft)",
            [ROW_BYTES],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        );
        let (reads, pinned) = rows.unwrap_or_else(|err| {
            tracing::warn!("Client store {}: size not read: {err}", self.account);
            (0, 0)
        });
        let files: u64 = std::fs::read_dir(&self.files)
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok()?.metadata().ok())
                    .filter(std::fs::Metadata::is_file)
                    .map(|meta| meta.len())
                    .sum()
            })
            .unwrap_or(0);
        Bytes {
            reads,
            pinned: pinned.saturating_add(i64::try_from(files).unwrap_or(i64::MAX)),
        }
    }

    /// Evict the least recently read until `over` bytes are gone; how many were.
    fn evict(&self, mut over: i64) -> Result<usize, Error> {
        let candidates = {
            let mut statement = self
                .conn
                .prepare(
                    "SELECT 'answer', key, '', size, read_ms FROM answer
                    UNION ALL SELECT 'room', r.chat_room_id, '', r.size
                        + COALESCE((SELECT SUM(e.size) FROM chat_event e WHERE e.chat_room_id = r.chat_room_id), 0),
                        r.read_ms FROM chat_room r
                    UNION ALL SELECT 'room', e.chat_room_id, '', SUM(e.size), 0 FROM chat_event e
                        WHERE NOT EXISTS (SELECT 1 FROM chat_room r WHERE r.chat_room_id = e.chat_room_id)
                        GROUP BY e.chat_room_id
                    UNION ALL SELECT 'thread', t.folder_id, t.thread_id, t.size
                        + COALESCE((SELECT SUM(m.size) FROM mail_message m WHERE m.thread_id = t.thread_id), 0),
                        t.read_ms FROM mail_thread t
                    UNION ALL SELECT 'thread', '', m.thread_id, SUM(m.size), 0 FROM mail_message m
                        WHERE NOT EXISTS (SELECT 1 FROM mail_thread t WHERE t.thread_id = m.thread_id)
                        GROUP BY m.thread_id
                    ORDER BY 5",
                )
                .map_err(failed("eviction not read"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(failed("eviction not read"))?;
            let mut candidates = Vec::new();
            for row in rows {
                match row {
                    Ok(candidate) => candidates.push(candidate),
                    Err(err) => tracing::warn!("Client store {}: an eviction candidate not read: {err}", self.account),
                }
            }
            candidates
        };
        let mut evicted = 0;
        for (kind, first, second, size) in candidates {
            if over <= 0 {
                break;
            }
            match kind.as_str() {
                "answer" => {
                    self.conn
                        .execute("DELETE FROM answer WHERE key = ?1", [&first])
                        .map_err(failed("answer not evicted"))?;
                }
                "room" => self.remove_chat_room(&first)?,
                _ => self.remove_mail_thread(&first, &second)?,
            }
            over -= size;
            evicted += 1;
        }
        Ok(evicted)
    }
}
