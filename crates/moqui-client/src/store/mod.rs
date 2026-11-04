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

//! An account's data on the device, for reading and writing with no network (plans/Mobile-Desktop.md §6, MD4): the
//! cached answers of the server, chat rooms and their events, mail folders, threads and messages, the outbox of
//! operations waiting to be sent, drafts, and the files the outbox and drafts send.
//!
//! One SQLite file per account, `<dir>/data/<account>.db`, with its files in `<dir>/data/<account>.files/`, where
//! `dir` is the client's directory the host gives (the one with `accounts.json`). Owner-only on Unix: the directories
//! 0700, the files 0600 (SQLite gives its WAL and shared-memory files the database's mode). The data is not encrypted
//! here; the OS's disk encryption covers it, and what the page encrypts stays encrypted.
//!
//! The schema moves forward by `PRAGMA user_version`, one transaction per step of [`MIGRATIONS`]. A database of a
//! newer client (a version past the last step) is not opened, so this client does not write rows it does not know.
//!
//! Every write is synchronous to disk (`synchronous = FULL`), so a queued send survives a power loss. Reads do not
//! write: the read times that order eviction are kept in memory and written in one unit now and then (every
//! [`READS_HELD`] reads, before a trim, and when the store is dropped); a crash loses only some of that order.
//!
//! Beside them (MD5) the cursors of the server's changes feeds and the progress of files sent in chunks.
//!
//! The cached reads (answers, rooms with their events, mail threads with their messages) are kept under a size cap:
//! past it the least recently read go first. The outbox, the drafts, the files and the mail folders are never evicted.
//!
//! [`Store`] is synchronous and owns one connection: callers on several threads share it in a `Mutex`, and run it on
//! a plain thread, not in an async task, as the rest of this crate.

mod answer;
mod attach;
mod cap;
mod chat;
mod draft;
mod mail;
mod outbox;
mod sync;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use moqui_lib::error::Error;
use rusqlite::{params, Connection};

pub use answer::{answer_key, Answer};
pub use attach::FILE_GRACE;
pub use chat::{ChatEvent, ChatRoom, EventPage, EventSlice, ROOM_CURSOR};
pub use draft::Draft;
pub use mail::{MailFolder, MailMessage, MailThread};
pub use outbox::{new_client_ref, order_group, OutboxOp, OutboxState, OutboxUpdate};
pub use sync::{SyncCursor, Upload};

/// The schema, a step per release that changes it; `PRAGMA user_version` is the number of steps applied. Add a step,
/// never change one that has shipped.
const MIGRATIONS: &[&str] = &[
    // 1: MD4's tables. read_ms is the read clock (Store::tick) that orders eviction; size is the row's bytes as the
    // size cap counts them (its text and a fixed overhead). chat_range: the seq ranges of a room's timeline (thread_seq
    // 0) or of a thread that the store holds whole, from the server's pages; low 0 is the timeline's start, head 1 a
    // range that reached the latest event and is kept up by pushes while the socket stays up.
    "CREATE TABLE answer (
        key TEXT PRIMARY KEY,
        body BLOB NOT NULL,
        content_type TEXT NOT NULL,
        stored_ms INTEGER NOT NULL,
        read_ms INTEGER NOT NULL,
        size INTEGER NOT NULL
    );
    CREATE TABLE chat_room (
        chat_room_id TEXT PRIMARY KEY,
        row TEXT NOT NULL,
        last_seq INTEGER NOT NULL,
        sort_ms INTEGER NOT NULL,
        read_ms INTEGER NOT NULL,
        size INTEGER NOT NULL
    );
    CREATE INDEX chat_room_sort ON chat_room (sort_ms);
    CREATE TABLE chat_event (
        chat_room_id TEXT NOT NULL,
        event_seq INTEGER NOT NULL,
        event TEXT NOT NULL,
        thread_seq INTEGER,
        size INTEGER NOT NULL,
        PRIMARY KEY (chat_room_id, event_seq)
    ) WITHOUT ROWID;
    CREATE INDEX chat_event_thread ON chat_event (chat_room_id, thread_seq);
    CREATE TABLE chat_range (
        chat_room_id TEXT NOT NULL,
        thread_seq INTEGER NOT NULL,
        low INTEGER NOT NULL,
        high INTEGER NOT NULL,
        head INTEGER NOT NULL,
        PRIMARY KEY (chat_room_id, thread_seq, low)
    ) WITHOUT ROWID;
    CREATE TABLE mail_folder (
        folder_id TEXT PRIMARY KEY,
        row TEXT NOT NULL,
        size INTEGER NOT NULL
    );
    CREATE TABLE mail_thread (
        folder_id TEXT NOT NULL,
        thread_id TEXT NOT NULL,
        sort_ms INTEGER NOT NULL,
        row TEXT NOT NULL,
        read_ms INTEGER NOT NULL,
        size INTEGER NOT NULL,
        PRIMARY KEY (folder_id, thread_id)
    ) WITHOUT ROWID;
    CREATE INDEX mail_thread_sort ON mail_thread (folder_id, sort_ms);
    CREATE INDEX mail_thread_id ON mail_thread (thread_id);
    CREATE TABLE mail_message (
        message_id TEXT PRIMARY KEY,
        thread_id TEXT NOT NULL,
        sort_ms INTEGER NOT NULL,
        row TEXT NOT NULL,
        size INTEGER NOT NULL
    );
    CREATE INDEX mail_message_thread ON mail_message (thread_id, sort_ms);
    CREATE TABLE outbox (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        target TEXT NOT NULL,
        state TEXT NOT NULL,
        attempts INTEGER NOT NULL,
        next_ms INTEGER NOT NULL,
        created_ms INTEGER NOT NULL,
        updated_ms INTEGER NOT NULL,
        payload TEXT NOT NULL,
        error TEXT,
        files TEXT NOT NULL
    );
    CREATE INDEX outbox_created ON outbox (created_ms);
    CREATE INDEX outbox_target ON outbox (kind, target, created_ms);
    CREATE TABLE draft (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        files TEXT NOT NULL,
        updated_ms INTEGER NOT NULL
    );",
    // 2: MD5's sync on the server's changes feeds (docs/ui/Desktop-Client.md "The server's sync contracts").
    // sync_cursor: where a feed was read to, by name (`chat`, `chat-heads`, `room:<id>`, `mail`), its seq and a value
    // (`unsupported` for a mail feed the server has none of). upload: a mail send's file sent in chunks, how far it got
    // (resumed from the server's uploadStatus after a restart). outbox status and current: the HTTP status of a refusal
    // (0 for none) and, for a conflict, the server's state the op met (JSON).
    "CREATE TABLE sync_cursor (
        name TEXT PRIMARY KEY,
        seq INTEGER,
        value TEXT,
        updated_ms INTEGER NOT NULL
    );
    CREATE TABLE upload (
        op_id TEXT NOT NULL,
        file TEXT NOT NULL,
        upload_id TEXT NOT NULL,
        offset INTEGER NOT NULL,
        total INTEGER NOT NULL,
        PRIMARY KEY (op_id, file)
    ) WITHOUT ROWID;
    ALTER TABLE outbox ADD COLUMN status INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE outbox ADD COLUMN current TEXT;",
];

/// What the size cap counts for a row beside its text: keys, indexes and page slack, roughly.
const ROW_BYTES: i64 = 64;

/// Read times held in memory before they are written.
const READS_HELD: usize = 256;

/// A row whose read time is held in memory.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Read {
    Answer(String),
    Room(String),
    Thread(String),
}

/// An account's store: one SQLite connection. Not `Sync`; share it in a `Mutex`.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
    dir: PathBuf,
    account: String,
    files: PathBuf,
    max_bytes: i64,
    /// The read clock: ms since the epoch, but strictly increasing, so the order of reads is kept within a millisecond.
    clock: Cell<i64>,
    /// Bytes written since the size was last checked.
    written: Cell<i64>,
    /// Read times not yet written, by row.
    reads: RefCell<HashMap<Read, i64>>,
}

impl Store {
    /// Open (or make) the store of `account_id` in `<dir>/data/`, with a size cap of `max_bytes`, and bring its schema
    /// up to date. Operations a crash left `sending` are made due again (the server takes each `clientRef` once),
    /// acknowledged ones past their keep are dropped, and files nothing refers to are removed.
    ///
    /// # Errors
    ///
    /// Returns an error for an account id that is not a plain file name, a directory or file that cannot be made, a
    /// database that does not open or migrate, and one written by a newer client.
    pub fn open(dir: &Path, account_id: &str, max_bytes: u64) -> Result<Self, Error> {
        let Some((db, files)) = paths(dir, account_id) else {
            return Err(Error::new(format!("Client store: {account_id} is not an account id")));
        };
        if let Some(data) = db.parent() {
            crate::files::create_private_dir(data)
                .map_err(|err| Error::new(format!("Client store {}: {err}", data.display())))?;
        }
        create_private_file(&db).map_err(|err| Error::new(format!("Client store {}: {err}", db.display())))?;
        let failed_open = |err: rusqlite::Error| Error::new(format!("Client store {} not opened: {err}", db.display()));
        let mut conn = Connection::open(&db).map_err(failed_open)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(failed_open)?;
        // auto_vacuum takes effect only before the first table: a new file shrinks after an eviction (Store::trim).
        conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")
            .map_err(failed_open)?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get::<_, String>(0))
            .map_err(failed_open)?;
        conn.pragma_update(None, "synchronous", "FULL").map_err(failed_open)?;
        migrate(&mut conn, &db)?;
        let store = Self {
            conn,
            dir: dir.to_path_buf(),
            account: account_id.to_string(),
            files,
            max_bytes: i64::try_from(max_bytes).unwrap_or(i64::MAX),
            clock: Cell::new(0),
            written: Cell::new(0),
            reads: RefCell::new(HashMap::new()),
        };
        let last_read = store
            .conn
            .query_row(
                "SELECT MAX(m) FROM (SELECT MAX(read_ms) m FROM answer UNION ALL SELECT MAX(read_ms) FROM chat_room
                    UNION ALL SELECT MAX(read_ms) FROM mail_thread)",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(failed("read clock not read"))?;
        store.clock.set(last_read.unwrap_or(0));
        store.recover_outbox();
        store.drop_orphan_uploads();
        store.gc_files(attach::FILE_GRACE);
        Ok(store)
    }

    /// Change the size cap; the next write past it evicts.
    pub fn set_max_bytes(&mut self, max_bytes: u64) {
        self.max_bytes = i64::try_from(max_bytes).unwrap_or(i64::MAX);
    }

    /// Close the store and forget it, as [`forget`].
    ///
    /// # Errors
    ///
    /// Returns an error naming what could not be removed.
    pub fn forget(self) -> Result<(), Error> {
        let (dir, account) = (self.dir.clone(), self.account.clone());
        drop(self);
        forget(&dir, &account)
    }

    /// Run `body` as one unit: all of its writes are kept, or none when it returns an error (or panics). A local change
    /// and its outbox operation go together this way (plans/Mobile-Desktop.md §6). Units nest: an inner unit's error
    /// undoes only the inner writes, and the outer unit may go on.
    ///
    /// # Errors
    ///
    /// Returns the error of `body`, or of the database when the unit cannot begin or end (its writes then undone).
    pub fn atomic<T>(&self, body: impl FnOnce(&Self) -> Result<T, Error>) -> Result<T, Error> {
        self.conn
            .execute_batch("SAVEPOINT unit")
            .map_err(failed("unit not begun"))?;
        let mut unit = Unit {
            store: self,
            done: false,
        };
        let value = body(self)?;
        self.conn
            .execute_batch("RELEASE unit")
            .map_err(failed("unit not kept"))?;
        unit.done = true;
        Ok(value)
    }

    // ---- helpers for the table modules ----

    /// The next value of the read clock.
    fn tick(&self) -> i64 {
        let next = now_ms().max(self.clock.get() + 1);
        self.clock.set(next);
        next
    }

    /// Note a read of a row for the size cap, written later ([`Self::flush_reads`]).
    fn touch(&self, read: Read) {
        let tick = self.tick();
        let held = {
            let mut reads = self.reads.borrow_mut();
            reads.insert(read, tick);
            reads.len()
        };
        if held >= READS_HELD {
            self.flush_reads();
        }
    }

    /// Write the read times held in memory, in one unit; a time never moves back (a write since may be later).
    fn flush_reads(&self) {
        let reads: Vec<(Read, i64)> = self.reads.borrow_mut().drain().collect();
        if reads.is_empty() {
            return;
        }
        let written = self.atomic(|store| {
            for (read, tick) in &reads {
                let (sql, key) = match read {
                    Read::Answer(key) => ("UPDATE answer SET read_ms = MAX(read_ms, ?1) WHERE key = ?2", key),
                    Read::Room(key) => (
                        "UPDATE chat_room SET read_ms = MAX(read_ms, ?1) WHERE chat_room_id = ?2",
                        key,
                    ),
                    Read::Thread(key) => (
                        "UPDATE mail_thread SET read_ms = MAX(read_ms, ?1) WHERE thread_id = ?2",
                        key,
                    ),
                };
                store
                    .conn
                    .prepare_cached(sql)
                    .and_then(|mut statement| statement.execute(params![tick, key]))
                    .map_err(failed("read time not written"))?;
            }
            Ok(())
        });
        if let Err(err) = written {
            tracing::warn!("Client store {}: read times not written: {err}", self.account);
        }
    }

    /// Count `bytes` written, and trim when they add up to a thirty-second of the cap, so the size is not summed on
    /// every write. A failed trim is logged: the write it follows is kept.
    fn wrote(&self, bytes: i64) {
        let written = self.written.get() + bytes;
        self.written.set(written);
        if written >= self.max_bytes / 32 {
            if let Err(err) = self.trim() {
                tracing::warn!("Client store {}: not trimmed: {err}", self.account);
            }
        }
    }

    /// Write each item in one unit with `put`, which gives the row's size or the item's id and its error: a failed
    /// item is logged and left out. Gives how many were kept.
    fn put_each<T>(
        &self,
        what: &str,
        items: &[T],
        put: impl Fn(&Self, &T) -> Result<i64, (String, rusqlite::Error)>,
    ) -> Result<usize, Error> {
        let mut size = 0;
        let kept = self.atomic(|store| {
            let mut kept = 0;
            for item in items {
                match put(store, item) {
                    Ok(row_size) => {
                        kept += 1;
                        size += row_size;
                    }
                    Err((id, err)) => tracing::warn!("Client store {}: {what} {id} not kept: {err}", store.account),
                }
            }
            Ok(kept)
        })?;
        self.wrote(size);
        Ok(kept)
    }

    /// The rows of a query, each made by `map`; a row that does not map is logged and left out, and a query that does
    /// not run is logged and gives none. `what` names the rows in the log.
    fn rows<T, P: rusqlite::Params>(
        &self,
        what: &str,
        sql: &str,
        params: P,
        map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> Vec<T> {
        let mut statement = match self.conn.prepare_cached(sql) {
            Ok(statement) => statement,
            Err(err) => {
                tracing::warn!("Client store {}: {what} not read: {err}", self.account);
                return Vec::new();
            }
        };
        let rows = match statement.query_map(params, map) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!("Client store {}: {what} not read: {err}", self.account);
                return Vec::new();
            }
        };
        let mut found = Vec::new();
        for row in rows {
            match row {
                Ok(value) => found.push(value),
                Err(err) => tracing::warn!("Client store {}: a row of {what} left out: {err}", self.account),
            }
        }
        found
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.flush_reads();
    }
}

/// Ends an [`Store::atomic`] unit that did not finish (an error, a panic, or a release that failed) by undoing its
/// writes, so a later unit does not nest inside one that is never released.
struct Unit<'a> {
    store: &'a Store,
    done: bool,
}

impl Drop for Unit<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        if let Err(err) = self.store.conn.execute_batch("ROLLBACK TO unit; RELEASE unit") {
            tracing::error!("Client store {}: unit not undone: {err}", self.store.account);
        }
    }
}

/// Forget the store of `account_id` in `<dir>/data/`: the database with its WAL and shared-memory files, and its files.
/// The account's [`Store`] must be dropped first (or use [`Store::forget`]): an open connection would write the files
/// again. Each part is tried even when one before it fails.
///
/// # Errors
///
/// Returns an error naming what could not be removed, or an account id that is not a plain file name.
pub fn forget(dir: &Path, account_id: &str) -> Result<(), Error> {
    let Some((db, files)) = paths(dir, account_id) else {
        return Err(Error::new(format!("Client store: {account_id} is not an account id")));
    };
    let mut failures = Vec::new();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let file = PathBuf::from(format!("{}{suffix}", db.display()));
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => failures.push(format!("{}: {err}", file.display())),
        }
    }
    match std::fs::remove_dir_all(&files) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => failures.push(format!("{}: {err}", files.display())),
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::new(format!(
            "Client store not all removed: {}",
            failures.join("; ")
        )))
    }
}

/// The database and the files directory of an account, or none for an id that could leave the directory.
fn paths(dir: &Path, account_id: &str) -> Option<(PathBuf, PathBuf)> {
    crate::files::plain_name(account_id).then(|| {
        let data = dir.join("data");
        (
            data.join(format!("{account_id}.db")),
            data.join(format!("{account_id}.files")),
        )
    })
}

/// Make the database file 0600 before SQLite opens it, so it (and the WAL and shared-memory files SQLite gives its
/// mode) is never readable by others; an existing file is set to 0600.
fn create_private_file(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    drop(file);
    Ok(())
}

/// Apply the steps of [`MIGRATIONS`] past the file's `user_version`, each in a transaction with its version.
fn migrate(conn: &mut Connection, db: &Path) -> Result<(), Error> {
    let failed_step = |err: rusqlite::Error| Error::new(format!("Client store {} not migrated: {err}", db.display()));
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(failed_step)?;
    let applied = usize::try_from(version).unwrap_or(0);
    if applied > MIGRATIONS.len() {
        return Err(Error::new(format!(
            "Client store {} is of a newer client (version {version}, this one knows {})",
            db.display(),
            MIGRATIONS.len()
        )));
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        let transaction = conn.transaction().map_err(failed_step)?;
        transaction.execute_batch(sql).map_err(failed_step)?;
        transaction
            .pragma_update(None, "user_version", i64::try_from(index + 1).unwrap_or(i64::MAX))
            .map_err(failed_step)?;
        transaction.commit().map_err(failed_step)?;
    }
    Ok(())
}

/// A database error as this crate's, saying what was not done.
fn failed(what: &'static str) -> impl Fn(rusqlite::Error) -> Error {
    move |err| Error::new(format!("Client store: {what}: {err}"))
}

/// A list of file names as the `files` column keeps it.
fn files_json(files: &[String]) -> Result<String, Error> {
    if let Some(bad) = files.iter().find(|name| !crate::files::plain_name(name)) {
        return Err(Error::new(format!("Client store: {bad} is not a file name")));
    }
    serde_json::to_string(files).map_err(|err| Error::new(err.to_string()))
}

fn text_len(text: &str) -> i64 {
    i64::try_from(text.len()).unwrap_or(i64::MAX)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests;
