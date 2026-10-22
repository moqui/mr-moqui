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
//! deleting it, in the transaction that delivers it and before the delivery: the second worker's
//! delete waits for the first transaction, then removes nothing and leaves the row to it. So one
//! worker delivers each row; a delivery whose commit fails rolls the claim back and is tried again,
//! so a document is delivered at least once.
//!
//! A worker given ids also claims the other rows still pending for the same records. When another
//! worker is delivering an earlier change of a record, this one waits for it before it assembles the
//! record, so an earlier delivery that finishes late (a document read before a delete, written
//! after it) is never the last word.
//!
//! A feed's receive service defaults to `index#DataDocuments` and its delete service to
//! `delete#DataDocument`. Other feeds, such as notifications, receive through their own service.

use std::collections::BTreeMap;

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FeedReport {
    pub documents_indexed: usize,
    pub documents_deleted: usize,
    pub rows_done: usize,
    pub rows_left: usize,
}

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

/// Deliver pending rows: the given ids, or every row, a page at a time, when `ids` is `None`.
///
/// # Errors
///
/// Returns an error when no context is entered or the pending rows cannot be read. A failed
/// delivery is logged and its rows stay.
pub fn process_pending(ids: Option<&[String]>) -> Result<FeedReport, Error> {
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
        let mut seen = std::collections::HashSet::new();
        let mut records: Vec<Condition> = Vec::new();
        for row in given.iter() {
            let text = |name: &str| row.get(name).map(to_plain_string).unwrap_or_default();
            let (document_id, record_id) = (text("dataDocumentId"), text("primaryKey"));
            if document_id.is_empty() || record_id.is_empty() || !seen.insert((document_id.clone(), record_id.clone()))
            {
                continue;
            }
            records.push(Condition::and(vec![
                Condition::equals("dataDocumentId", Value::string(&document_id)),
                Condition::equals("primaryKey", Value::string(&record_id)),
            ]));
        }
        if records.is_empty() {
            return Ok(report);
        }
        let rows = facade
            .find(PENDING)?
            .condition(Condition::or(records))
            .order_by("dataFeedPendingId", false)
            .list()?;
        deliver_rows(&facade, &rows, &feeds, search_up, &mut report);
        return Ok(report);
    }
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
            tracing::debug!(rows = kept, "search is not configured; data feed rows stay");
            report.rows_left += usize::try_from(kept).unwrap_or(usize::MAX);
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
        deliver_rows(&facade, &rows, &feeds, search_up, &mut report);
        match last {
            Some(last) if i64::try_from(rows.len()).unwrap_or(i64::MAX) >= DRAIN_BATCH => after = Some(last),
            _ => break,
        }
    }
    Ok(report)
}

/// Deliver pending rows by document type, each type in its own transaction, adding to `report`.
fn deliver_rows(
    facade: &EntityFacade,
    rows: &crate::entity::EntityList,
    feeds: &IndexMap<String, Vec<Feed>>,
    search_up: bool,
    report: &mut FeedReport,
) {
    // Document id, then record id to the pending row ids for it.
    let mut by_document: IndexMap<String, IndexMap<String, Vec<String>>> = IndexMap::new();
    // The agent that made a pending row's writes (`originAgentId`), by row id.
    let mut row_origins: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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
                    return Ok((0, 0, 0));
                }
                let origins: std::collections::HashMap<String, String> = origins
                    .into_iter()
                    .filter_map(|(record_id, origin)| origin.map(|origin| (record_id, origin)))
                    .collect();
                let (indexed, deleted) = deliver(facade, &document_id, &claimed_records, &document_feeds, &origins)?;
                Ok((indexed, deleted, claimed))
            })
        });
        match delivered {
            Ok((indexed, deleted, claimed)) => {
                if claimed < row_count {
                    tracing::debug!(document = %document_id, rows = row_count - claimed, "data feed rows another worker took");
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
            }
        }
    }
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
            let shown = if feed.receive == DEFAULT_RECEIVE || origins.is_empty() { &documents } else { &tagged };
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

/// Claim a pending row by deleting it: true when this delete removed it, false when another worker already had.
fn claim_pending(facade: &EntityFacade, pending_id: &str) -> Result<bool, Error> {
    let mut value = facade.make_value(PENDING)?;
    value.set("dataFeedPendingId", Value::string(pending_id));
    Ok(facade.delete_value_counted(&value)? > 0)
}

/// Real-time push feeds, by the document ids they contain.
fn realtime_feeds(facade: &EntityFacade) -> Result<IndexMap<String, Vec<Feed>>, Error> {
    let feeds = facade
        .find("moqui.entity.feed.DataFeed")?
        .eq("dataFeedTypeEnumId", Value::string(REALTIME_PUSH))
        .list()?;
    let links = facade.find("moqui.entity.feed.DataFeedDocument")?.list()?;
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
mod tests {
    use super::*;
    use crate::context::ExecutionContextFactory;

    fn create_row(facade: &EntityFacade, entity: &str, pairs: &[(&str, &str)]) {
        let mut row = facade.make_value(entity).unwrap();
        for (name, value) in pairs {
            row.set(*name, Value::string(*value));
        }
        row.create(facade).unwrap();
    }

    fn pending(facade: &EntityFacade, id: &str, document_id: &str) {
        create_row(
            facade,
            PENDING,
            &[
                ("dataFeedPendingId", id),
                ("dataDocumentId", document_id),
                ("primaryKey", "F1"),
                ("operation", "index"),
            ],
        );
    }

    #[test]
    fn the_drain_pages_past_rows_that_stay() {
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        // GOOD assembles (its record is gone, so it is a delete with no feed to tell); BAD has no
        // definition, so its delivery fails; HELD goes to search, which is not configured.
        create_row(
            &facade,
            "moqui.entity.document.DataDocument",
            &[("dataDocumentId", "GOOD"), ("primaryEntityName", "moqui.test.Foo")],
        );
        create_row(
            &facade,
            "moqui.entity.feed.DataFeed",
            &[("dataFeedId", "SEARCH"), ("dataFeedTypeEnumId", REALTIME_PUSH)],
        );
        create_row(
            &facade,
            "moqui.entity.feed.DataFeedDocument",
            &[("dataFeedId", "SEARCH"), ("dataDocumentId", "HELD")],
        );
        pending(&facade, "A0000", "HELD");
        let bad = usize::try_from(DRAIN_BATCH).unwrap() + 1;
        for index in 0..bad {
            pending(&facade, &format!("P{index:05}"), "BAD");
        }
        pending(&facade, "Q00000", "GOOD");

        let report = process_pending(None).unwrap();
        assert_eq!(report.rows_done, 1, "{report:?}");
        assert_eq!(report.documents_deleted, 1, "{report:?}");
        assert_eq!(report.rows_left, bad + 1, "{report:?}");
        let left = facade.find(PENDING).unwrap().count().unwrap();
        assert_eq!(left, i64::try_from(bad + 1).unwrap());
        let good = facade
            .find(PENDING)
            .unwrap()
            .eq("dataFeedPendingId", Value::string("Q00000"))
            .one()
            .unwrap();
        assert!(good.is_none());
    }

    #[test]
    fn a_row_another_worker_took_is_not_delivered_again() {
        let ctx = ExecutionContextFactory::test_context();
        let facade = crate::testing::open();
        ctx.factory().set_entity(facade.clone());
        let _entered = ctx.enter();
        create_row(
            &facade,
            "moqui.entity.document.DataDocument",
            &[("dataDocumentId", "GOOD"), ("primaryEntityName", "moqui.test.Foo")],
        );
        pending(&facade, "R00001", "GOOD");
        // This worker read the row; another delivered it and deleted it before this one got to it.
        let rows = facade.find(PENDING).unwrap().list().unwrap();
        assert_eq!(rows.len(), 1);
        let mut gone = facade.make_value(PENDING).unwrap();
        gone.set("dataFeedPendingId", Value::string("R00001"));
        gone.delete(&facade).unwrap();

        let mut report = FeedReport::default();
        deliver_rows(&facade, &rows, &realtime_feeds(&facade).unwrap(), false, &mut report);
        assert_eq!(report, FeedReport::default(), "{report:?}");

        // A row still there is claimed and delivered once; the second pass finds it gone.
        pending(&facade, "R00002", "GOOD");
        let rows = facade.find(PENDING).unwrap().list().unwrap();
        let mut first = FeedReport::default();
        deliver_rows(&facade, &rows, &realtime_feeds(&facade).unwrap(), false, &mut first);
        assert_eq!((first.rows_done, first.documents_deleted), (1, 1), "{first:?}");
        let mut second = FeedReport::default();
        deliver_rows(&facade, &rows, &realtime_feeds(&facade).unwrap(), false, &mut second);
        assert_eq!(second, FeedReport::default(), "{second:?}");

        // A worker given the later row of a record takes the earlier one too (its own delivery waits on whoever has
        // it), and leaves another record's row alone.
        pending(&facade, "R00003", "GOOD");
        pending(&facade, "R00004", "GOOD");
        create_row(
            &facade,
            PENDING,
            &[
                ("dataFeedPendingId", "R00005"),
                ("dataDocumentId", "GOOD"),
                ("primaryKey", "F2"),
                ("operation", "index"),
            ],
        );
        let report = process_pending(Some(&["R00004".to_string()])).unwrap();
        assert_eq!(report.rows_done, 2, "{report:?}");
        let left: Vec<String> = facade
            .find(PENDING)
            .unwrap()
            .list()
            .unwrap()
            .iter()
            .filter_map(|row| row.get("dataFeedPendingId").map(to_plain_string))
            .collect();
        assert_eq!(left, vec!["R00005".to_string()]);
    }

    #[test]
    fn a_failed_call_takes_back_only_its_own_errors() {
        let ctx = ExecutionContextFactory::test_context();
        ctx.factory().set_entity(crate::testing::open());
        let _entered = ctx.enter();
        ctx.message().add_message("kept");
        ctx.message().add_error("earlier");
        assert!(call_checked("moqui.test.NoSuchServices.receive#Documents", &BTreeMap::new()).is_err());
        assert_eq!(ctx.message().errors(), ["earlier"]);
        assert_eq!(ctx.message().messages(), ["kept"]);
    }
}
