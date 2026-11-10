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

//! What the transaction cache did, per frame and per thread (plans/archive/TX-Cache.md D1): writes held, finds answered from
//! memory or the database, early flushes by cause, and the commit batch. A frame's counts go to a debug event when it
//! ends and into this thread's totals ([`tx_cache_stats`]), which tests and the speed test read. An early flush is
//! logged with its cause, entity and service; in strict mode it marks the frame rollback-only, and in census mode it
//! is logged as a census line (and appended to `MOQUI_TX_CACHE_CENSUS_FILE` when that is set).

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;

use moqui_lib::error::Error;

use super::Op;
use crate::transaction::TransactionManager;

/// Why held rows were written before the commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlushCause {
    /// A find or count of a view with a member the frame holds rows of.
    View,
    /// A `distinct`, `having` or aggregate find of an entity the frame holds rows of.
    SelectiveFind,
    /// Raw SQL run in the frame.
    RawSql,
    /// A service marked `no-tx-cache` joined the frame.
    NoTxCache,
    /// The frame held more than its size cap.
    Size,
}

impl FlushCause {
    pub const ALL: [FlushCause; 5] = [
        FlushCause::View,
        FlushCause::SelectiveFind,
        FlushCause::RawSql,
        FlushCause::NoTxCache,
        FlushCause::Size,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            FlushCause::View => "view",
            FlushCause::SelectiveFind => "selective find",
            FlushCause::RawSql => "raw sql",
            FlushCause::NoTxCache => "no-tx-cache",
            FlushCause::Size => "size",
        }
    }

    fn index(self) -> usize {
        match self {
            FlushCause::View => 0,
            FlushCause::SelectiveFind => 1,
            FlushCause::RawSql => 2,
            FlushCause::NoTxCache => 3,
            FlushCause::Size => 4,
        }
    }
}

/// Counts of the transaction cache, for one frame or summed over this thread's frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TxCacheStats {
    /// Frames that held or read through the cache and ended.
    pub frames: u64,
    pub held_creates: u64,
    pub held_updates: u64,
    pub held_deletes: u64,
    pub held_upserts: u64,
    /// Finds and counts answered from the frame's memory with no query.
    pub memory_reads: u64,
    /// Finds and counts in a cache frame that queried the database.
    pub db_reads: u64,
    /// Early flushes, by [`FlushCause`] in the order of [`FlushCause::ALL`].
    pub early_flushes: [u64; 5],
    /// View reads that saw the frame's held rows through shadowed member tables, with no early flush.
    pub shadowed_reads: u64,
    /// Statements, rows and waited round trips of the flushes at commit.
    pub batch_statements: u64,
    pub batch_rows: u64,
    pub batch_round_trips: u64,
}

impl TxCacheStats {
    #[must_use]
    pub fn held(&self) -> u64 {
        self.held_creates + self.held_updates + self.held_deletes + self.held_upserts
    }

    #[must_use]
    pub fn early_flush_count(&self) -> u64 {
        self.early_flushes.iter().sum()
    }

    #[must_use]
    pub fn early_flushes_of(&self, cause: FlushCause) -> u64 {
        self.early_flushes[cause.index()]
    }

    /// Add `other`'s counts to these.
    pub fn add(&mut self, other: &TxCacheStats) {
        self.frames += other.frames;
        self.held_creates += other.held_creates;
        self.held_updates += other.held_updates;
        self.held_deletes += other.held_deletes;
        self.held_upserts += other.held_upserts;
        self.memory_reads += other.memory_reads;
        self.db_reads += other.db_reads;
        for (sum, count) in self.early_flushes.iter_mut().zip(other.early_flushes) {
            *sum += count;
        }
        self.shadowed_reads += other.shadowed_reads;
        self.batch_statements += other.batch_statements;
        self.batch_rows += other.batch_rows;
        self.batch_round_trips += other.batch_round_trips;
    }

    /// What changed from `earlier` to this, both totals of one thread.
    #[must_use]
    pub fn since(&self, earlier: &TxCacheStats) -> TxCacheStats {
        let mut early_flushes = [0; 5];
        for (index, slot) in early_flushes.iter_mut().enumerate() {
            *slot = self.early_flushes[index].saturating_sub(earlier.early_flushes[index]);
        }
        TxCacheStats {
            frames: self.frames.saturating_sub(earlier.frames),
            held_creates: self.held_creates.saturating_sub(earlier.held_creates),
            held_updates: self.held_updates.saturating_sub(earlier.held_updates),
            held_deletes: self.held_deletes.saturating_sub(earlier.held_deletes),
            held_upserts: self.held_upserts.saturating_sub(earlier.held_upserts),
            memory_reads: self.memory_reads.saturating_sub(earlier.memory_reads),
            db_reads: self.db_reads.saturating_sub(earlier.db_reads),
            early_flushes,
            shadowed_reads: self.shadowed_reads.saturating_sub(earlier.shadowed_reads),
            batch_statements: self.batch_statements.saturating_sub(earlier.batch_statements),
            batch_rows: self.batch_rows.saturating_sub(earlier.batch_rows),
            batch_round_trips: self.batch_round_trips.saturating_sub(earlier.batch_round_trips),
        }
    }
}

thread_local! {
    /// The counts of each open cache frame, by frame token.
    static FRAMES: RefCell<HashMap<u64, TxCacheStats>> = RefCell::new(HashMap::new());
    /// The counts of this thread's ended frames.
    static TOTAL: RefCell<TxCacheStats> = RefCell::new(TxCacheStats::default());
}

/// This thread's totals: every ended cache frame's counts.
#[must_use]
pub fn tx_cache_stats() -> TxCacheStats {
    TOTAL.with(|total| total.borrow().clone())
}

fn with_frame(token: u64, body: impl FnOnce(&mut TxCacheStats)) {
    FRAMES.with(|frames| body(frames.borrow_mut().entry(token).or_default()));
}

pub(super) fn note_held(token: u64, op: Op) {
    with_frame(token, |stats| match op {
        Op::Create => stats.held_creates += 1,
        Op::Update => stats.held_updates += 1,
        Op::Delete => stats.held_deletes += 1,
        Op::Upsert => stats.held_upserts += 1,
    });
}

/// A find or count in the active frame of `tx`, when it has the cache on; `memory` when no query ran.
pub(super) fn note_read(tx: &TransactionManager, memory: bool) {
    if let Some((token, _)) = tx.cache_frame() {
        note_read_of(token, memory);
    }
}

pub(super) fn note_read_of(token: u64, memory: bool) {
    with_frame(token, |stats| {
        if memory {
            stats.memory_reads += 1;
        } else {
            stats.db_reads += 1;
        }
    });
}

/// A view read of the frame `token` that saw its held rows through shadowed member tables.
pub(super) fn note_shadowed(token: u64) {
    with_frame(token, |stats| stats.shadowed_reads += 1);
}

/// The flush at commit: its statements, rows and waited round trips.
pub(super) fn note_batch(token: u64, statements: u64, rows: u64, round_trips: u64) {
    with_frame(token, |stats| {
        stats.batch_statements += statements;
        stats.batch_rows += rows;
        stats.batch_round_trips += round_trips;
    });
}

/// An early flush of the frame `token` of `tx`, of the held rows of `entity`: counted, logged, and in strict mode
/// the frame is marked rollback-only.
pub(super) fn note_early_flush(tx: &TransactionManager, token: u64, cause: FlushCause, entity: &str) {
    with_frame(token, |stats| stats.early_flushes[cause.index()] += 1);
    let service = (crate::hooks::get().artifact_name)().artifact.unwrap_or_default();
    tracing::debug!(cause = cause.name(), entity, service = %service, "transaction cache early flush");
    if tx.tx_cache_everywhere() {
        census(&format!(
            "{{\"kind\":\"early-flush\",\"cause\":\"{}\",\"entity\":\"{entity}\",\"service\":\"{service}\"}}",
            cause.name()
        ));
    }
    if tx.tx_cache_strict() {
        let message = format!(
            "transaction cache strict mode: early flush ({}) of {entity} in {service}",
            cause.name()
        );
        if let Err(err) = tx.set_rollback_only(message) {
            tracing::warn!(error = %err, "could not mark the frame rollback-only for a strict early flush");
        }
    }
}

/// A held frame's commit flush failed: in census mode, a census line naming the service.
pub(super) fn note_flush_failed(tx: &TransactionManager, error: &Error) {
    if tx.tx_cache_everywhere() {
        let service = (crate::hooks::get().artifact_name)().artifact.unwrap_or_default();
        let text = error.to_string().replace(['"', '\\', '\n'], " ");
        census(&format!(
            "{{\"kind\":\"flush-failed\",\"service\":\"{service}\",\"error\":\"{text}\"}}"
        ));
    }
}

/// Whether frames are measured for the census: `MOQUI_TX_CACHE_CENSUS_FILE` is set (read once).
pub(crate) fn census_measures() -> bool {
    !census_path().is_empty()
}

fn census_path() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| moqui_lib::env::get_prop_or_env("MOQUI_TX_CACHE_CENSUS_FILE"))
}

/// A frame ended with the census file set, in census mode or not: a census line with the service that ended it, its
/// cache mode, its waited round trips, its time from begin to the end of its commit, and what its cache did, so a run with the cache everywhere and one without give
/// per service what the cache saves and whether it held cleanly.
pub(crate) fn census_frame(
    mode: &str,
    stats: Option<&TxCacheStats>,
    committed: bool,
    round_trips: u64,
    micros: u64,
    shapes: &str,
) {
    let service = (crate::hooks::get().artifact_name)().artifact.unwrap_or_default();
    let none = TxCacheStats::default();
    let stats = stats.unwrap_or(&none);
    census(&format!(
        "{{\"kind\":\"frame\",\"service\":\"{service}\",\"mode\":\"{mode}\",\"committed\":{committed},\
         \"creates\":{},\"updates\":{},\"deletes\":{},\"upserts\":{},\"memory_reads\":{},\"db_reads\":{},\
         \"early_flushes\":{},\"shadowed_reads\":{},\"batch_statements\":{},\"batch_rows\":{},\
         \"round_trips\":{round_trips},\"micros\":{micros}{shapes}}}",
        stats.held_creates,
        stats.held_updates,
        stats.held_deletes,
        stats.held_upserts,
        stats.memory_reads,
        stats.db_reads,
        stats.early_flush_count(),
        stats.shadowed_reads,
        stats.batch_statements,
        stats.batch_rows,
    ));
}

/// The frame `token` ended (committed or rolled back): its counts go to a debug event and this thread's totals, and
/// back to the transaction manager for the transaction's span.
pub(crate) fn end_frame(token: u64) -> Option<TxCacheStats> {
    let mut stats = FRAMES.with(|frames| frames.borrow_mut().remove(&token))?;
    stats.frames = 1;
    tracing::debug!(
        held = stats.held(),
        memory_reads = stats.memory_reads,
        db_reads = stats.db_reads,
        early_flushes = stats.early_flush_count(),
        batch_statements = stats.batch_statements,
        batch_rows = stats.batch_rows,
        batch_round_trips = stats.batch_round_trips,
        "transaction cache frame"
    );
    TOTAL.with(|total| total.borrow_mut().add(&stats));
    Some(stats)
}

/// Forget the counts of every open frame on this thread (a thread with no frame left).
pub(super) fn clear_frames() {
    FRAMES.with(|frames| frames.borrow_mut().clear());
}

fn census(line: &str) {
    tracing::warn!(target: "moqui::tx_cache::census", "{line}");
    let path = census_path();
    if path.is_empty() {
        return;
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut file) => {
            // one write of the whole line: appends from other threads and processes do not split it
            if let Err(err) = file.write_all(format!("{line}\n").as_bytes()) {
                tracing::warn!(error = %err, path, "could not write the transaction cache census");
            }
        }
        Err(err) => tracing::warn!(error = %err, path, "could not open the transaction cache census"),
    }
}
