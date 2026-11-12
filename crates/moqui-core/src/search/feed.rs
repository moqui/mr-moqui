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

//! The data feed's receiving side: `DataFeedPending` rows become calls to each feed's services.
//!
//! A commit inserts one pending row per touched document (the primary record id, and `index` or
//! `delete`). After commit a service worker runs [`process_pending`] for those rows: it assembles
//! the documents, calls the receive service of every real-time feed that has the document with
//! `documentList`, calls the delete service for a document that no longer assembles, and deletes
//! the rows that were delivered. A row that cannot be delivered stays, and a later call with no
//! ids (the `IndexDataFeedDocuments` drain) tries it again.
//!
//! The after-commit worker and the drain can read the same rows at once. A worker claims a row by
//! deleting it, in the transaction that delivers it and before the delivery: where the database
//! skips locked rows it first locks the rows no other worker holds (`SKIP LOCKED`); elsewhere the
//! second worker's delete waits for the first transaction, then removes nothing and leaves the row
//! to it. So one worker delivers each row; a delivery whose commit fails rolls the claim back and is
//! tried again, so a document is delivered at least once.
//!
//! A worker given ids also claims the other rows still pending for the same records. When another
//! worker is delivering an earlier change of a record, this one waits for it before it assembles the
//! record, so an earlier delivery that finishes late (a document read before a delete, written
//! after it) is never the last word. With `SKIP LOCKED` that wait is outside any transaction: the
//! record is left out of this delivery and tried again once the other worker's rows are gone, for
//! up to [`HELD_WAIT`]. The drain waits the same way, so the rows pending when it was called are
//! delivered when it returns.
//!
//! A feed's receive service defaults to `index#DataDocuments` and its delete service to
//! `delete#DataDocument`. Other feeds, such as notifications, receive through their own service.
//!
//! A row that keeps failing stays, and the drain says so (plans/archive/High-Level-Review.md HL-043): a row whose delivery
//! fails when it has been pending for the drain's `stuck_after` or longer makes [`FeedReport::stuck`], which the drain
//! service turns into an error, so its job run is kept and the job's error handling fires. Each drain also sets the
//! gauges `moqui.search.feed.pending`, `moqui.search.feed.oldest_pending_seconds` and `moqui.search.feed.search_down`,
//! and rows held while search is down are logged as a warning, at most once an hour while they stay.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use jiff::Timestamp;
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::document::{self, Document};
use crate::entity::{Condition, EntityFacade};
use crate::transaction::TransactionAttribute;

pub const DEFAULT_RECEIVE: &str = "org.moqui.search.SearchServices.index#DataDocuments";
pub const DEFAULT_DELETE: &str = "org.moqui.search.SearchServices.delete#DataDocument";
const PENDING: &str = "moqui.entity.feed.DataFeedPending";
const REALTIME_PUSH: &str = "DTFDTP_RT_PUSH";
/// Pending rows one drain page reads.
const DRAIN_BATCH: i64 = 1000;
/// How long a delivery waits for another worker that holds rows of the same records, before it leaves its own rows to the
/// next drain.
const HELD_WAIT: std::time::Duration = std::time::Duration::from_secs(60);
/// How often a delivery looks again at the records another worker holds.
const HELD_POLL: std::time::Duration = std::time::Duration::from_millis(20);
/// How long a row may keep failing before the drain is an error, when its caller names no time.
pub const DEFAULT_STUCK_AFTER: Duration = Duration::from_secs(60 * 60);
/// How often the rows held while search is down are logged again while they stay.
const HELD_LOG_EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedReport {
    pub documents_indexed: usize,
    pub documents_deleted: usize,
    pub rows_done: usize,
    pub rows_left: usize,
    /// Rows left because search is down (counted in `rows_left` too).
    pub rows_held: usize,
    /// Rows whose delivery failed in this drain after being pending for its `stuck_after` or longer.
    pub rows_stuck: usize,
    /// The document types of the stuck rows.
    pub stuck_documents: Vec<String>,
    /// Rows pending once the drain is done, and the age of the oldest, in seconds; a drain only.
    pub rows_pending: usize,
    pub oldest_pending_seconds: Option<u64>,
}

impl FeedReport {
    /// The error of a drain with stuck rows, for the drain service to report; `None` when no row is stuck.
    #[must_use]
    pub fn stuck(&self, stuck_after: Duration) -> Option<String> {
        (self.rows_stuck > 0).then(|| {
            format!(
                "{} data feed rows of {} keep failing delivery after {} minutes pending; they stay pending",
                self.rows_stuck,
                self.stuck_documents.join(", "),
                stuck_after.as_secs() / 60
            )
        })
    }
}

/// The feed gauges of a factory, as the last drain left them ([`FeedReport`]), and when the held rows were last logged.
#[derive(Default)]
struct FeedGauges {
    pending: AtomicU64,
    /// When the oldest pending row was made, in epoch milliseconds; 0 when none is pending or none has a stamp.
    oldest_millis: AtomicI64,
    search_down: AtomicBool,
    held_logged: Mutex<Option<Instant>>,
}

impl FeedGauges {
    /// The factory's gauges, registered on its telemetry when first made. The callbacks hold them weakly.
    fn of(factory: &crate::context::ExecutionContextFactory) -> Arc<Self> {
        factory
            .extension(|factory| {
                let gauges = Arc::new(Self::default());
                let telemetry = crate::telemetry::for_factory(factory);
                let weak = Arc::downgrade(&gauges);
                telemetry.observe_gauge("moqui.search.feed.pending", move || {
                    Some(weak.upgrade()?.pending.load(Ordering::Relaxed) as f64)
                });
                let weak = Arc::downgrade(&gauges);
                telemetry.observe_gauge("moqui.search.feed.oldest_pending_seconds", move || {
                    let oldest = weak.upgrade()?.oldest_millis.load(Ordering::Relaxed);
                    if oldest == 0 {
                        return Some(0.0);
                    }
                    let age = Timestamp::now().as_millisecond().saturating_sub(oldest).max(0);
                    Some(age as f64 / 1000.0)
                });
                let weak = Arc::downgrade(&gauges);
                telemetry.observe_gauge("moqui.search.feed.search_down", move || {
                    Some(f64::from(u8::from(weak.upgrade()?.search_down.load(Ordering::Relaxed))))
                });
                FeedGaugesSlot(gauges)
            })
            .0
            .clone()
    }

    fn record(&self, report: &FeedReport, oldest: Option<Timestamp>, search_down: bool) {
        self.pending.store(
            u64::try_from(report.rows_pending).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.oldest_millis
            .store(oldest.map_or(0, |stamp| stamp.as_millisecond()), Ordering::Relaxed);
        self.search_down.store(search_down, Ordering::Relaxed);
        self.log_held(report.rows_held, Instant::now());
    }

    /// A warning when rows are first held for search, again each [`HELD_LOG_EVERY`] while they stay, and a note when
    /// none is held any more.
    fn log_held(&self, held: usize, now: Instant) {
        let mut logged = moqui_lib::sync::lock(&self.held_logged);
        if held == 0 {
            if logged.take().is_some() {
                tracing::info!("search is up; no data feed rows are held for it");
            }
            return;
        }
        if logged.is_none_or(|at| now.duration_since(at) >= HELD_LOG_EVERY) {
            tracing::warn!(
                rows = held,
                "search is not configured; data feed rows that go to it stay pending until it is"
            );
            *logged = Some(now);
        }
    }
}

/// The extension slot of [`FeedGauges`].
struct FeedGaugesSlot(Arc<FeedGauges>);

struct Feed {
    data_feed_id: String,
    receive: String,
    delete: String,
}

impl Feed {
    /// False when the feed goes to search and search is not configured.
    fn deliverable(&self, search_up: bool) -> bool {
        search_up || (self.receive != DEFAULT_RECEIVE && self.delete != DEFAULT_DELETE)
    }
}

/// Deliver pending rows: the given ids, or every row, a page at a time, when `ids` is `None` (the drain). A drain counts
/// the rows whose delivery fails after being pending `stuck_after` or longer ([`FeedReport::stuck`]), and sets the
/// feed gauges.
///
/// # Errors
///
/// Returns an error when no context is entered or the pending rows cannot be read. A failed
/// delivery is logged and its rows stay.
pub fn process_pending(ids: Option<&[String]>, stuck_after: Duration) -> Result<FeedReport, Error> {
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("data feed processing needs an entity facade"))?;
    let _authz = ExecutionContext::with_current(|ec| ec.disable_authz());
    if matches!(ids, Some([])) {
        return Ok(FeedReport::default());
    }
    let feeds = realtime_feeds(&facade)?;
    let search_up = ExecutionContext::with_current(|ec| super::installed(ec.factory()).is_some()).unwrap_or(false);
    let mut report = FeedReport::default();
    if let Some(ids) = ids {
        let given = facade
            .find(PENDING)?
            .condition(Condition::is_in(
                "dataFeedPendingId",
                ids.iter().map(Value::string).collect(),
            ))
            .list()?;
        // The given rows and the others pending for their records, so a delivery of an earlier change still under way
        // finishes before this one assembles the record.
        let mut records: Vec<(String, String)> = Vec::new();
        for row in given.iter() {
            let text = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
            let record = (text("dataDocumentId"), text("primaryKey"));
            if record.0.is_empty() || record.1.is_empty() || records.contains(&record) {
                continue;
            }
            records.push(record);
        }
        if records.is_empty() {
            return Ok(report);
        }
        let rows = pending_for_records(&facade, &records)?;
        deliver_rows(&facade, &rows, &feeds, search_up, None, &mut report);
        return Ok(report);
    }
    let stuck_before = Timestamp::now()
        .checked_sub(jiff::SignedDuration::try_from(stuck_after).unwrap_or(jiff::SignedDuration::MAX))
        .unwrap_or(Timestamp::MIN);
    // The drain leaves out the types that cannot be delivered now, reads in id order, and pages
    // past the rows that stay, so rows that keep failing never hide the rows behind them.
    let held: Vec<Value> = feeds
        .iter()
        .filter(|(_, list)| !list.iter().all(|feed| feed.deliverable(search_up)))
        .map(|(document_id, _)| Value::string(document_id))
        .collect();
    if !held.is_empty() {
        let kept = facade
            .find(PENDING)?
            .condition(Condition::is_in("dataDocumentId", held.clone()))
            .count()?;
        if kept > 0 {
            let kept = usize::try_from(kept).unwrap_or(usize::MAX);
            report.rows_held += kept;
            report.rows_left += kept;
        }
    }
    let mut after: Option<String> = None;
    loop {
        let mut find = facade
            .find(PENDING)?
            .order_by("dataFeedPendingId", false)
            .limit(DRAIN_BATCH);
        if !held.is_empty() {
            find = find.condition(Condition::not_in("dataDocumentId", held.clone()));
        }
        if let Some(after) = &after {
            find = find.condition(Condition::greater("dataFeedPendingId", Value::string(after)));
        }
        let rows = find.list()?;
        let last = rows
            .values()
            .last()
            .and_then(|row| row.get("dataFeedPendingId"))
            .map(to_plain_string);
        deliver_rows(&facade, &rows, &feeds, search_up, Some(stuck_before), &mut report);
        match last {
            Some(last) if i64::try_from(rows.len()).unwrap_or(i64::MAX) >= DRAIN_BATCH => after = Some(last),
            _ => break,
        }
    }
    // What stays, for the gauges; a failure to read it is logged and leaves the gauges as they were.
    match pending_state(&facade) {
        Ok((pending, oldest)) => {
            report.rows_pending = pending;
            report.oldest_pending_seconds =
                oldest.map(|stamp| u64::try_from(Timestamp::now().duration_since(stamp).as_secs()).unwrap_or(0));
            if let Some(factory) = ExecutionContext::with_current(|ec| Arc::clone(ec.factory())) {
                FeedGauges::of(&factory).record(&report, oldest, !search_up);
            }
        }
        Err(err) => tracing::warn!(error = %err, "the pending data feed rows could not be counted"),
    }
    if report.rows_stuck > 0 {
        tracing::warn!(
            rows = report.rows_stuck,
            documents = %report.stuck_documents.join(", "),
            "data feed rows keep failing delivery"
        );
    }
    Ok(report)
}

/// The rows pending, and when the oldest with a stamp was made.
fn pending_state(facade: &EntityFacade) -> Result<(usize, Option<Timestamp>), Error> {
    let pending = facade.find(PENDING)?.count()?;
    let oldest = facade
        .find(PENDING)?
        .condition(Condition::is_not_null("lastUpdatedStamp"))
        .select_field("lastUpdatedStamp")
        .order_by("lastUpdatedStamp", false)
        .limit(1)
        .list()?
        .iter()
        .find_map(|row| row_stamp(row.get("lastUpdatedStamp")));
    Ok((usize::try_from(pending).unwrap_or(usize::MAX), oldest))
}

/// A pending row's `lastUpdatedStamp`, when it has one.
fn row_stamp(value: Option<&Value>) -> Option<Timestamp> {
    match value? {
        Value::Timestamp(stamp) => Some(*stamp),
        Value::Null => None,
        other => to_plain_string(other).parse().ok(),
    }
}

/// The pending rows of these (document id, record id) pairs, in id order.
fn pending_for_records(
    facade: &EntityFacade,
    records: &[(String, String)],
) -> Result<crate::entity::EntityList, Error> {
    let conditions = records
        .iter()
        .map(|(document_id, record_id)| {
            Condition::and(vec![
                Condition::equals("dataDocumentId", Value::string(document_id)),
                Condition::equals("primaryKey", Value::string(record_id)),
            ])
        })
        .collect();
    facade
        .find(PENDING)?
        .condition(Condition::or(conditions))
        .order_by("dataFeedPendingId", false)
        .list()
}

/// Deliver pending rows by document type, each type in its own transaction, adding to `report`. A record with a row
/// another worker holds is tried again once that worker's rows are gone, for up to [`HELD_WAIT`]; its rows then stay.
/// A failed row made at or before `stuck_before` is counted stuck.
fn deliver_rows(
    facade: &EntityFacade,
    rows: &crate::entity::EntityList,
    feeds: &IndexMap<String, Vec<Feed>>,
    search_up: bool,
    stuck_before: Option<Timestamp>,
    report: &mut FeedReport,
) {
    let deadline = std::time::Instant::now() + HELD_WAIT;
    let mut held = deliver_once(facade, rows, feeds, search_up, stuck_before, report);
    while !held.is_empty() {
        if std::time::Instant::now() >= deadline {
            let rows: usize = held.iter().map(|(_, count)| count).sum();
            tracing::warn!(
                records = held.len(),
                rows,
                "data feed records still held by another worker after {}s; their rows stay for the drain",
                HELD_WAIT.as_secs()
            );
            report.rows_left += rows;
            return;
        }
        std::thread::sleep(HELD_POLL);
        let records: Vec<(String, String)> = held.into_iter().map(|(record, _)| record).collect();
        let rows = match pending_for_records(facade, &records) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "data feed rows held by another worker could not be read again; they stay");
                return;
            }
        };
        held = deliver_once(facade, &rows, feeds, search_up, stuck_before, report);
    }
}

/// One pass of [`deliver_rows`]. Returns the records left out because another worker holds one of their rows, as
/// ((document id, record id), pending rows of it in this pass).
fn deliver_once(
    facade: &EntityFacade,
    rows: &crate::entity::EntityList,
    feeds: &IndexMap<String, Vec<Feed>>,
    search_up: bool,
    stuck_before: Option<Timestamp>,
    report: &mut FeedReport,
) -> Vec<((String, String), usize)> {
    let mut held_records: Vec<((String, String), usize)> = Vec::new();
    // Document id, then record id to the pending row ids for it.
    let mut by_document: IndexMap<String, IndexMap<String, Vec<String>>> = IndexMap::new();
    // The agent that made a pending row's writes (`originAgentId`), by row id.
    let mut row_origins: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // When each row was made, by row id, to tell a stuck row.
    let mut row_stamps: std::collections::HashMap<String, Timestamp> = std::collections::HashMap::new();
    for row in rows.iter() {
        let text = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
        let (pending_id, document_id, record_id) =
            (text("dataFeedPendingId"), text("dataDocumentId"), text("primaryKey"));
        if document_id.is_empty() || record_id.is_empty() {
            continue;
        }
        let origin = text("originAgentId");
        if !origin.is_empty() {
            row_origins.insert(pending_id.clone(), origin);
        }
        if let Some(stamp) = row_stamp(row.get("lastUpdatedStamp")) {
            row_stamps.insert(pending_id.clone(), stamp);
        }
        by_document
            .entry(document_id)
            .or_default()
            .entry(record_id)
            .or_default()
            .push(pending_id);
    }
    for (document_id, records) in by_document {
        let row_count: usize = records.values().map(Vec::len).sum();
        let document_feeds: Vec<&Feed> = feeds
            .get(&document_id)
            .map(|list| list.iter().collect())
            .unwrap_or_default();
        if !document_feeds.iter().all(|feed| feed.deliverable(search_up)) {
            tracing::debug!(document = %document_id, "search is not configured; data feed rows stay");
            report.rows_left += row_count;
            continue;
        }
        // Each document type in its own transaction: a type that fails (a manual data service that errors, say) rolls
        // back only its own delivery and keeps only its own rows, not the other types' in this batch.
        let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
        let delivered = facade.transactions_for(PENDING).and_then(|tx| {
            tx.run(TransactionAttribute::ForceNew, None, || {
                // Claim the rows before delivering, in id order so two workers take their locks in the same order. A
                // record none of whose rows this worker claimed is another worker's.
                let mut claims: Vec<(&String, &String)> = records
                    .iter()
                    .flat_map(|(record_id, pending_ids)| {
                        pending_ids.iter().map(move |pending_id| (pending_id, record_id))
                    })
                    .collect();
                claims.sort();
                // Where the database skips locked rows, only the rows no other worker holds are claimed, without waiting
                // for that worker's delivery to commit; elsewhere the delete of each row waits as before. A record with a
                // row still there but held waits for that worker: delivered now, its document could be overwritten by
                // the other worker's older read.
                let mut held: Vec<String> = Vec::new();
                if let Some(free) = lock_free_rows(facade, claims.iter().map(|(pending_id, _)| *pending_id))? {
                    let present = present_rows(
                        facade,
                        claims
                            .iter()
                            .map(|(pending_id, _)| *pending_id)
                            .filter(|pending_id| !free.contains(*pending_id)),
                    )?;
                    for (pending_id, record_id) in &claims {
                        if present.contains(*pending_id) && !held.contains(record_id) {
                            held.push((*record_id).clone());
                        }
                    }
                    claims.retain(|(pending_id, record_id)| free.contains(*pending_id) && !held.contains(record_id));
                }
                let mut claimed_records: Vec<String> = Vec::new();
                let mut claimed = 0_usize;
                // a record's origin: the agent of every row claimed for it, else none (the agent triggers' loop guard)
                let mut origins: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
                for (pending_id, record_id) in claims {
                    if claim_pending(facade, pending_id)? {
                        claimed += 1;
                        if !claimed_records.contains(record_id) {
                            claimed_records.push(record_id.clone());
                        }
                        let origin = row_origins.get(pending_id).cloned();
                        origins
                            .entry(record_id.clone())
                            .and_modify(|known| {
                                if *known != origin {
                                    *known = None;
                                }
                            })
                            .or_insert(origin);
                    }
                }
                if claimed_records.is_empty() {
                    return Ok((0, 0, 0, held));
                }
                let origins: std::collections::HashMap<String, String> = origins
                    .into_iter()
                    .filter_map(|(record_id, origin)| origin.map(|origin| (record_id, origin)))
                    .collect();
                let (indexed, deleted) = deliver(facade, &document_id, &claimed_records, &document_feeds, &origins)?;
                Ok((indexed, deleted, claimed, held))
            })
        });
        match delivered {
            Ok((indexed, deleted, claimed, held)) => {
                let mut held_rows = 0;
                for record_id in held {
                    let count = records.get(&record_id).map_or(0, Vec::len);
                    held_rows += count;
                    held_records.push(((document_id.clone(), record_id), count));
                }
                if claimed + held_rows < row_count {
                    tracing::debug!(
                        document = %document_id,
                        rows = row_count - claimed - held_rows,
                        "data feed rows another worker took"
                    );
                }
                report.documents_indexed += indexed;
                report.documents_deleted += deleted;
                report.rows_done += claimed;
            }
            Err(err) => {
                tracing::warn!(document = %document_id, error = %err, "data feed delivery failed; pending rows stay");
                // The failure is logged; its message errors would stop the next document type's services. The
                // caller's earlier errors stay.
                if let Some(mark) = mark {
                    ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark));
                }
                report.rows_left += row_count;
                if let Some(stuck_before) = stuck_before {
                    let stuck = records
                        .values()
                        .flatten()
                        .filter(|pending_id| row_stamps.get(*pending_id).is_some_and(|stamp| *stamp <= stuck_before))
                        .count();
                    if stuck > 0 {
                        report.rows_stuck += stuck;
                        if !report.stuck_documents.contains(&document_id) {
                            report.stuck_documents.push(document_id.clone());
                        }
                    }
                }
            }
        }
    }
    held_records
}

/// Deliver the documents of `record_ids`; a record whose every write came from one agent's run (`origins`) carries it as
/// `_originAgentId` (and a delete as `originAgentId`), so an agent trigger leaves out its own agent's changes.
fn deliver(
    facade: &EntityFacade,
    document_id: &str,
    record_ids: &[String],
    feeds: &[&Feed],
    origins: &std::collections::HashMap<String, String>,
) -> Result<(usize, usize), Error> {
    let documents = document::documents_by_ids(facade, document_id, record_ids)?;
    // the search index gets the documents as they are; another receive gets the origin too
    let tagged: Vec<_> = documents
        .iter()
        .filter(|_| !origins.is_empty())
        .map(|doc| {
            let mut doc = doc.clone();
            let id = doc.get("_id").map(to_plain_string).unwrap_or_default();
            if let Some(origin) = origins.get(&id) {
                doc.insert("_originAgentId".to_string(), Value::string(origin));
            }
            doc
        })
        .collect();
    let found: std::collections::HashSet<String> = documents
        .iter()
        .filter_map(|doc| doc.get("_id").map(to_plain_string))
        .collect();
    let gone: Vec<&String> = record_ids.iter().filter(|id| !found.contains(*id)).collect();
    let stamp = Value::Timestamp(Timestamp::now());
    for feed in feeds {
        if !documents.is_empty() {
            let mut params = BTreeMap::new();
            params.insert("dataFeedId".to_string(), Value::string(&feed.data_feed_id));
            params.insert("feedStamp".to_string(), stamp.clone());
            let shown = if feed.receive == DEFAULT_RECEIVE || origins.is_empty() {
                &documents
            } else {
                &tagged
            };
            params.insert(
                "documentList".to_string(),
                Value::List(shown.iter().cloned().map(Value::Map).collect()),
            );
            call_checked(&feed.receive, &params)?;
        }
        for id in &gone {
            let mut params = BTreeMap::new();
            params.insert("dataFeedId".to_string(), Value::string(&feed.data_feed_id));
            params.insert("feedStamp".to_string(), stamp.clone());
            params.insert("dataDocumentId".to_string(), Value::string(document_id));
            params.insert("documentId".to_string(), Value::string(*id));
            if let Some(origin) = origins.get(*id) {
                params.insert("originAgentId".to_string(), Value::string(origin));
            }
            call_checked(&feed.delete, &params)?;
        }
    }
    Ok((documents.len(), gone.len()))
}

/// Call a service and return its results. An error message it leaves is a failure, and is taken
/// back out so it does not fail the caller; the caller's earlier messages and errors stay.
pub(super) fn call_checked(name: &str, params: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>, Error> {
    let mark = ExecutionContext::with_current(|ec| ec.message().error_mark());
    let result = crate::service::call::sync(name, params);
    let errors: Vec<String> = mark
        .and_then(|mark| ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)))
        .unwrap_or_default();
    match result {
        Err(err) => Err(Error::new(format!("{name}: {}", err.message()))),
        Ok(_) if !errors.is_empty() => Err(Error::new(format!("{name}: {}", errors.join("; ")))),
        Ok(results) => Ok(results),
    }
}

/// The pending rows among `ids` this transaction now holds, locked with `SKIP LOCKED` so a row another worker holds is left
/// out without waiting; None where the database cannot skip locked rows.
fn lock_free_rows<'a>(
    facade: &EntityFacade,
    ids: impl Iterator<Item = &'a String>,
) -> Result<Option<std::collections::HashSet<String>>, Error> {
    if !facade.with_entity(PENDING, |_, dialect, _| Ok(dialect.skips_locked()))? {
        return Ok(None);
    }
    let ids: Vec<Value> = ids.map(Value::string).collect();
    if ids.is_empty() {
        return Ok(Some(std::collections::HashSet::new()));
    }
    let rows = facade
        .find(PENDING)?
        .condition(Condition::is_in("dataFeedPendingId", ids))
        .order_by("dataFeedPendingId", false)
        .for_update(true)
        .skip_locked(true)
        .list()?;
    Ok(Some(
        rows.iter()
            .filter_map(|row| row.get("dataFeedPendingId").map(to_plain_string))
            .collect(),
    ))
}

/// The rows among `ids` still there, read without a lock: a row another worker has claimed but not yet committed is still
/// there.
fn present_rows<'a>(
    facade: &EntityFacade,
    ids: impl Iterator<Item = &'a String>,
) -> Result<std::collections::HashSet<String>, Error> {
    let ids: Vec<Value> = ids.map(Value::string).collect();
    if ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows = facade
        .find(PENDING)?
        .condition(Condition::is_in("dataFeedPendingId", ids))
        .select_field("dataFeedPendingId")
        .list()?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get("dataFeedPendingId").map(to_plain_string))
        .collect())
}

/// Claim a pending row by deleting it: true when this delete removed it, false when another worker already had.
fn claim_pending(facade: &EntityFacade, pending_id: &str) -> Result<bool, Error> {
    let mut value = facade.make_value(PENDING)?;
    value.set("dataFeedPendingId", Value::string(pending_id));
    Ok(facade.delete_value_counted(&value)? > 0)
}

/// Real-time push feeds, by the document ids they contain. Read through the entity cache: each delivery of pending rows reads them.
fn realtime_feeds(facade: &EntityFacade) -> Result<IndexMap<String, Vec<Feed>>, Error> {
    let feeds = facade
        .find("moqui.entity.feed.DataFeed")?
        .eq("dataFeedTypeEnumId", Value::string(REALTIME_PUSH))
        .use_cache(true)
        .list()?;
    let links = facade
        .find("moqui.entity.feed.DataFeedDocument")?
        .use_cache(true)
        .list()?;
    let mut out: IndexMap<String, Vec<Feed>> = IndexMap::new();
    for feed in feeds.iter() {
        let text = |name: &str| feed.get(name).map(to_plain_string).filter(|text| !text.is_empty());
        let Some(data_feed_id) = text("dataFeedId") else {
            continue;
        };
        for link in links.iter() {
            if link.get("dataFeedId").map(to_plain_string).as_deref() != Some(data_feed_id.as_str()) {
                continue;
            }
            let Some(document_id) = link.get("dataDocumentId").map(to_plain_string) else {
                continue;
            };
            out.entry(document_id).or_default().push(Feed {
                data_feed_id: data_feed_id.clone(),
                receive: text("feedReceiveServiceName").unwrap_or_else(|| DEFAULT_RECEIVE.to_string()),
                delete: text("feedDeleteServiceName").unwrap_or_else(|| DEFAULT_DELETE.to_string()),
            });
        }
    }
    Ok(out)
}

/// `index#DataFeedDocuments`: rebuild every document of a feed, or one document of it, in
/// batches sent to the feed's receive service. Returns how many documents were sent.
///
/// # Errors
///
/// Returns an error when the feed has no documents, a definition is bad, or a receive call fails.
pub fn rebuild_feed(
    data_feed_id: &str,
    data_document_id: Option<&str>,
    from: Option<Timestamp>,
    thru: Option<Timestamp>,
    batch_size: usize,
) -> Result<usize, Error> {
    let facade = ExecutionContext::with_current(|ec| ec.factory().entity())
        .flatten()
        .ok_or_else(|| Error::new("data feed rebuild needs an entity facade"))?;
    let _authz = ExecutionContext::with_current(|ec| ec.disable_authz());
    let feed = facade
        .find("moqui.entity.feed.DataFeed")?
        .eq("dataFeedId", Value::string(data_feed_id))
        .one()?
        .ok_or_else(|| Error::new(format!("No DataFeed found for ID {data_feed_id}")))?;
    let receive = feed
        .get("feedReceiveServiceName")
        .map(to_plain_string)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| DEFAULT_RECEIVE.to_string());
    let mut find = facade
        .find("moqui.entity.feed.DataFeedDocument")?
        .eq("dataFeedId", Value::string(data_feed_id));
    if let Some(id) = data_document_id {
        find = find.eq("dataDocumentId", Value::string(id));
    }
    let links = find.list()?;
    if links.is_empty() {
        return Err(Error::new(format!(
            "No DataDocuments found for DataFeed ID {data_feed_id}{}",
            data_document_id
                .map(|id| format!(" and DataDocument ID {id}"))
                .unwrap_or_default()
        )));
    }
    let mut total = 0;
    for link in links.iter() {
        let Some(document_id) = link.get("dataDocumentId").map(to_plain_string) else {
            continue;
        };
        if receive == DEFAULT_RECEIVE {
            if let Ok(search) = super::current() {
                search.ensure_index(&document_id)?;
            }
        }
        total += document::feed_documents(&facade, &document_id, from, thru, batch_size, |batch: Vec<Document>| {
            let mut params = BTreeMap::new();
            params.insert("dataFeedId".to_string(), Value::string(data_feed_id));
            params.insert("feedStamp".to_string(), Value::Timestamp(Timestamp::now()));
            params.insert(
                "documentList".to_string(),
                Value::List(batch.into_iter().map(Value::Map).collect()),
            );
            call_checked(&receive, &params)?;
            Ok(true)
        })?;
    }
    Ok(total)
}

#[cfg(test)]
#[path = "feed_tests.rs"]
mod tests;
