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

//! `ec.search`. Each call takes one options map. A target is `documentType` (one id, a comma-separated list, or a
//! list) or `indexName`. Query and aggregation trees use the map form in `crate::search::query` and
//! `crate::search::agg`; a query may also be a query string.
//!
//! ```text
//! ec.search.search({indexName, documentType, query, queryString, filter, sort, offset, limit, highlight,
//!         rank, consistency, freshTimeout})
//!     -> {hits: [{documentId, id, score, body, highlights}], total, totalRelation}
//! ec.search.aggregate({documentType, filter, aggs, consistency, freshTimeout}) -> {name: result, ..}
//! ec.search.parseQuery(text), ec.search.elasticQuery(mapOrJson) -> query tree
//! ec.search.indexDocuments(documentList) -> count
//! ec.search.deleteDocument(documentType, id) -> boolean
//! ec.search.deleteMatching({indexName | documentType, filter}) -> count
//! ec.search.getDocument(documentType, id) -> map or null
//! ec.search.ensureIndex(target), ec.search.dropIndex(target) -> how many document types
//! ec.search.documentTypes(target) -> document ids; a target is a type name or an options map
//! ec.search.indexExists({indexName | documentType}) -> boolean
//! ec.search.isAvailable(), ec.search.backendName()
//! ec.search.embedPending({indexName | documentType, batch, maxChars}) -> embeddings stored
//! ec.search.hasEmbedder() -> whether an embedding model is installed (rankTexts needs one)
//! ec.search.rankTexts({query, texts, limit}) -> [{index, score, meaning, words}], best first (search::rank); an
//!     empty query embeds the texts for later calls and ranks none
//! ec.search.capabilities() -> names: proximity, fuzzy, autocomplete, weightedRank, accent, semantic, hybrid
//! ec.search.flatten(map) -> map with dotted keys
//! ec.search.processFeedPending(ids?, stuckMinutes?) -> {documentsIndexed, documentsDeleted, rowsDone, rowsLeft, rowsHeld,
//!     rowsStuck, rowsPending, oldestPendingSeconds, stuckError}
//! ec.search.rebuildFeed({dataFeedId, dataDocumentId, fromUpdateStamp, thruUpdateStamp, batchSize}) -> count
//! ```
//!
//! `retrieval: "hybrid"` or `"semantic"` with `text`, `k`, `weights`, `fuzzyBelow`, and `qualityBoost` fuses branches
//! (`search::hybrid`). `rank: "coverDensity"` ranks with the `searchWeight` field weights. `consistency:
//! "readYourWrites"` waits up to `freshTimeout` milliseconds (default 5000, at most 60000) for this session's writes to
//! be indexed and, with a read replica, replayed there; see `search::fresh`.

use moqui_lib::object::{to_plain_string, Value};

use super::{method, Call, HostClass, HostError, Ret};
use crate::search::{self, Agg, Query, SearchRequest, SortField, Target};

pub(super) static SEARCH: HostClass = HostClass {
    name: "host:Search",
    methods: &[
        method!("ec.search", "search", [Value], None, search_search),
        method!("ec.search", "aggregate", [Value], None, aggregate),
        method!("ec.search", "parseQuery", [Text], parse_query),
        method!("ec.search", "elasticQuery", [Value], elastic_query),
        method!("ec.search", "indexDocuments", [Value], None, index_documents),
        method!("ec.search", "deleteDocument", [Text, Text], delete_document),
        method!("ec.search", "deleteMatching", [Value], delete_matching),
        method!("ec.search", "getDocument", [Text, Text], get_document),
        method!("ec.search", "ensureIndex", [Value], ensure_index),
        method!("ec.search", "dropIndex", [Value], drop_index),
        method!("ec.search", "indexExists", [Value], index_exists),
        method!("ec.search", "documentTypes", [Value], document_types_of),
        method!("ec.search", "isAvailable", [], is_available),
        method!("ec.search", "hasElasticApi", [], has_elastic_api),
        method!("ec.search", "backendName", [], backend_name),
        method!("ec.search", "capabilities", [], capabilities),
        method!("ec.search", "embedPending", [Value], None, embed_pending),
        method!("ec.search", "rankTexts", [Value], None, rank_texts),
        method!("ec.search", "hasEmbedder", [], has_embedder),
        method!("ec.search", "flatten", [Value], flatten),
        method!(
            "ec.search",
            "processFeedPending",
            [OptValue, OptValue],
            Release,
            process_feed_pending
        ),
        method!("ec.search", "rebuildFeed", [Value], Release, rebuild_feed),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

fn facade() -> Result<std::sync::Arc<search::SearchFacade>, HostError> {
    search::current().map_err(HostError::Lib)
}

fn count(value: usize) -> Ret {
    Ret::Value(Value::I64(i64::try_from(value).unwrap_or(i64::MAX)))
}

fn options(value: Value) -> Result<moqui_lib::object::ValueMap, HostError> {
    match value {
        Value::Map(map) => Ok(map),
        Value::Null => Ok(indexmap::IndexMap::default()),
        _ => Err(HostError::message("ec.search takes an options map")),
    }
}

/// The options map of the first argument.
fn options_arg(call: &Call<'_>) -> Result<moqui_lib::object::ValueMap, HostError> {
    options(call.value_or_null(0))
}

fn text_list(value: Option<&Value>) -> Vec<String> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|text| !text.trim().is_empty())
            .map(|text| text.trim().to_string())
            .collect(),
        Some(other) => to_plain_string(other)
            .split(',')
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

fn number(value: Option<&Value>, default: usize) -> usize {
    value
        .map(to_plain_string)
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(default)
}

fn target_of(map: &moqui_lib::object::ValueMap) -> Result<Target, HostError> {
    // Session replays are read only through the replay services, which need MONITOR_REPLAY (moqui-report browser.rs).
    let replays = || HostError::message("browser replays are read through the replay services, not ec.search");
    let documents = text_list(map.get("documentType").or_else(|| map.get("documentTypes")));
    if !documents.is_empty() {
        if documents.iter().any(|id| id == search::telemetry::REPLAYS) {
            return Err(replays());
        }
        return Ok(Target::Documents(documents));
    }
    match map.get("indexName").map(to_plain_string) {
        Some(name) if name == search::telemetry::REPLAYS => Err(replays()),
        Some(name) if !name.is_empty() => Ok(Target::Index(name)),
        _ => Err(HostError::message("a search needs documentType or indexName")),
    }
}

fn query_of(value: Option<&Value>) -> Result<Query, HostError> {
    match value {
        None | Some(Value::Null) => Ok(Query::MatchAll),
        Some(value) => Query::from_value(value).map_err(HostError::Lib),
    }
}

fn search_search(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let target = target_of(&map)?;
    let facade = facade()?;
    let mut query = query_of(map.get("query"))?;
    if let Some(text) = map.get("queryString").map(to_plain_string) {
        let parsed = search::parse::parse_query_string(&text).map_err(HostError::Lib)?;
        query = match query {
            Query::MatchAll => parsed,
            other => Query::all(vec![other, parsed]),
        };
    }
    let mut request = SearchRequest::new(query);
    request.sort = text_list(map.get("sort").or_else(|| map.get("orderByFields")))
        .iter()
        .filter_map(|text| SortField::parse(text))
        .collect();
    request.offset = number(map.get("offset"), 0);
    request.limit = number(map.get("limit"), 20);
    request.highlight = text_list(map.get("highlight").or_else(|| map.get("highlightFields")));
    request.fresh_by = fresh_by(&map);
    request.retrieval = retrieval(&map)?;
    // A trusted constraint from the service, such as authorization; it does not score.
    request.filter = match map.get("filter") {
        None | Some(Value::Null) => None,
        Some(value) => Some(Query::from_value(value).map_err(HostError::Lib)?),
    };
    request.rank = match map.get("rank").map(to_plain_string).as_deref() {
        None | Some("") | Some("default") => search::Rank::Default,
        Some("coverDensity") => search::Rank::CoverDensity,
        Some(other) => {
            return Err(HostError::message(format!(
                "search rank {other} is not default or coverDensity"
            )))
        }
    };
    let page = facade.search(&target, &request).map_err(HostError::Lib)?;
    let mut hits = Vec::with_capacity(page.hits.len());
    for hit in page.hits {
        let mut entry = indexmap::IndexMap::default();
        entry.insert("documentId".to_string(), Value::String(hit.document_id));
        entry.insert("id".to_string(), Value::String(hit.id));
        entry.insert("index".to_string(), Value::String(hit.index));
        entry.insert("score".to_string(), Value::F64(hit.score));
        entry.insert("body".to_string(), Value::Map(hit.body));
        let highlights = hit
            .highlights
            .into_iter()
            .map(|(name, list)| (name, Value::List(list.into_iter().map(Value::String).collect())))
            .collect();
        entry.insert("highlights".to_string(), Value::Map(highlights));
        hits.push(Value::Map(entry));
    }
    let mut out = indexmap::IndexMap::default();
    out.insert("hits".to_string(), Value::List(hits));
    out.insert(
        "total".to_string(),
        Value::I64(i64::try_from(page.total).unwrap_or(i64::MAX)),
    );
    out.insert("totalRelation".to_string(), Value::string(page.total_relation.name()));
    Ok(Ret::Value(Value::Map(out)))
}

/// `retrieval: "hybrid" | "semantic"` with `text`, `k`, `weights: {lexical, prefix, fuzzy, semantic}`, `fuzzyBelow`,
/// and `qualityBoost`. Absent or `"lexical"` is the query alone.
fn retrieval(map: &moqui_lib::object::ValueMap) -> Result<search::Retrieval, HostError> {
    let kind = map.get("retrieval").map(to_plain_string).unwrap_or_default();
    if kind.is_empty() || kind == "lexical" {
        return Ok(search::Retrieval::Lexical);
    }
    let float = |value: Option<&Value>, default: f64| {
        value
            .map(to_plain_string)
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or(default)
    };
    let mut options = search::hybrid::HybridOptions {
        text: map.get("text").map(to_plain_string).unwrap_or_default(),
        ..Default::default()
    };
    options.k = number(map.get("k"), options.k);
    options.fuzzy_below = u64::try_from(number(map.get("fuzzyBelow"), 5)).unwrap_or(5);
    options.quality_boost = float(map.get("qualityBoost"), 0.0);
    if let Some(Value::Map(weights)) = map.get("weights") {
        options.weights.lexical = float(weights.get("lexical"), options.weights.lexical);
        options.weights.prefix = float(weights.get("prefix"), options.weights.prefix);
        options.weights.fuzzy = float(weights.get("fuzzy"), options.weights.fuzzy);
        options.weights.semantic = float(weights.get("semantic"), options.weights.semantic);
    }
    match kind.as_str() {
        "hybrid" => Ok(search::Retrieval::Hybrid(options)),
        "semantic" => Ok(search::Retrieval::Semantic(options)),
        other => Err(HostError::message(format!(
            "search retrieval {other} is not lexical, hybrid, or semantic"
        ))),
    }
}

/// The longest a read-your-writes search waits, in milliseconds: the wait holds a request worker.
const MAX_FRESH_TIMEOUT: u64 = 60_000;

/// The read-your-writes deadline from `consistency` and `freshTimeout`, which is at most [`MAX_FRESH_TIMEOUT`].
fn fresh_by(map: &moqui_lib::object::ValueMap) -> Option<std::time::Instant> {
    let consistency = map.get("consistency").map(to_plain_string)?;
    if !consistency.eq_ignore_ascii_case("readYourWrites") {
        return None;
    }
    let millis = u64::try_from(number(map.get("freshTimeout"), 5000)).unwrap_or(MAX_FRESH_TIMEOUT);
    let now = std::time::Instant::now();
    Some(
        now.checked_add(std::time::Duration::from_millis(millis.min(MAX_FRESH_TIMEOUT)))
            .unwrap_or(now),
    )
}

fn embed_pending(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let target = target_of(&map)?;
    let batch = number(map.get("batch"), 100);
    let max_chars = number(map.get("maxChars"), 2000);
    let stored = facade()?
        .embed_pending(&target, batch, max_chars)
        .map_err(HostError::Lib)?;
    Ok(count(stored))
}

fn has_embedder(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(facade()?.embedder().is_some().into())
}

fn rank_texts(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let query = map.get("query").map(to_plain_string).unwrap_or_default();
    // Every text keeps its place, empty ones too: a result's index is its position in the caller's list.
    let texts: Vec<String> = match map.get("texts") {
        Some(Value::List(items)) => items.iter().map(to_plain_string).collect(),
        None | Some(Value::Null) => Vec::new(),
        Some(_) => return Err(HostError::message("rankTexts texts is a list of texts")),
    };
    let limit = number(map.get("limit"), 10);
    let ranked = facade()?
        .rank_texts(&query, &texts, limit)
        .map_err(HostError::Lib)?;
    let list = ranked
        .into_iter()
        .map(|text| {
            let mut entry = indexmap::IndexMap::default();
            entry.insert("index".to_string(), Value::I64(i64::try_from(text.index).unwrap_or(i64::MAX)));
            entry.insert("score".to_string(), Value::F64(f64::from(text.score)));
            entry.insert("meaning".to_string(), Value::F64(f64::from(text.meaning)));
            entry.insert("words".to_string(), Value::F64(f64::from(text.words)));
            Value::Map(entry)
        })
        .collect();
    Ok(Ret::Value(Value::List(list)))
}

fn capabilities(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let names = facade()?
        .capabilities()
        .into_iter()
        .map(|capability| Value::string(capability.name()))
        .collect();
    Ok(Ret::Value(Value::List(names)))
}

fn aggregate(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let target = target_of(&map)?;
    let filter = query_of(map.get("filter").or_else(|| map.get("query")))?;
    let aggs = Agg::map_from_value(map.get("aggs").unwrap_or(&Value::Null)).map_err(HostError::Lib)?;
    let results = facade()?
        .aggregate_fresh(&target, &filter, &aggs, fresh_by(&map))
        .map_err(HostError::Lib)?;
    Ok(Ret::Value(Value::Map(results)))
}

fn parse_query(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let query = search::parse::parse_query_string(call.text(0).unwrap_or_default()).map_err(HostError::Lib)?;
    Ok(Ret::Value(query.to_value()))
}

fn elastic_query(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let query = search::es_json::from_es(&call.value_or_null(0)).map_err(HostError::Lib)?;
    Ok(Ret::Value(query.to_value()))
}

fn index_documents(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let documents: Vec<crate::entity::document::Document> = match call.value_or_null(0) {
        Value::List(items) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::Map(map) => Some(map),
                _ => None,
            })
            .collect(),
        Value::Null => Vec::new(),
        _ => return Err(HostError::message("indexDocuments takes a list of documents")),
    };
    let indexed = facade()?.index_documents(&documents).map_err(HostError::Lib)?;
    Ok(count(indexed))
}

fn delete_document(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (document_type, id) = (call.text(0).unwrap_or_default(), call.text(1).unwrap_or_default());
    let deleted = facade()?.delete_document(document_type, id).map_err(HostError::Lib)?;
    Ok(deleted.into())
}

fn delete_matching(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let target = target_of(&map)?;
    let filter = query_of(map.get("filter").or_else(|| map.get("query")))?;
    let deleted = facade()?.delete_matching(&target, &filter).map_err(HostError::Lib)?;
    Ok(Ret::Value(Value::I64(i64::try_from(deleted).unwrap_or(i64::MAX))))
}

fn get_document(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let (document_type, id) = (call.text(0).unwrap_or_default(), call.text(1).unwrap_or_default());
    let found = facade()?.get_document(document_type, id).map_err(HostError::Lib)?;
    Ok(Ret::Value(found.map_or(Value::Null, Value::Map)))
}

/// A document type name, or an options map with `documentType` or `indexName`.
fn document_types(value: Value) -> Result<Vec<String>, HostError> {
    if let Value::String(_) = &value {
        return Ok(text_list(Some(&value)));
    }
    let map = options(value)?;
    let target = target_of(&map)?;
    facade()?.target_documents(&target).map_err(HostError::Lib)
}

fn ensure_index(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = document_types(call.value_or_null(0))?;
    let search = facade()?;
    for id in &ids {
        search.ensure_index(id).map_err(HostError::Lib)?;
    }
    Ok(count(ids.len()))
}

fn drop_index(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = document_types(call.value_or_null(0))?;
    let search = facade()?;
    for id in &ids {
        search.drop_index(id).map_err(HostError::Lib)?;
    }
    Ok(count(ids.len()))
}

fn document_types_of(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = document_types(call.value_or_null(0))?;
    Ok(Ret::Value(Value::List(ids.into_iter().map(Value::String).collect())))
}

fn index_exists(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let target = target_of(&map)?;
    Ok(facade()?.target_exists(&target).map_err(HostError::Lib)?.into())
}

fn is_available(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(search::current().is_ok().into())
}

/// Whether the search backend answers the Elasticsearch REST API (OpenSearch), so a proxy to it has something to reach.
fn has_elastic_api(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(search::current()
        .is_ok_and(|found| found.backend().has_elastic_api())
        .into())
}

fn backend_name(_: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(match search::current() {
        Ok(found) => Ret::Value(Value::string(found.backend().name())),
        Err(_) => Ret::Value(Value::Null),
    })
}

fn flatten(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::Value(match call.value_or_null(0) {
        Value::Map(map) => Value::Map(moqui_lib::collection::flatten_nested_map(&map)),
        other => other,
    }))
}

/// `processFeedPending(ids, stuckMinutes)`: deliver the rows of `ids`, or drain every pending row when `ids` is null.
/// A drain reports as `stuckError` the rows that still fail after `stuckMinutes` (default 60) pending.
fn process_feed_pending(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ids = match call.value(0) {
        None | Some(Value::Null) => None,
        Some(other) => Some(text_list(Some(other))),
    };
    let stuck_after = match call.value(1) {
        None | Some(Value::Null) => search::feed::DEFAULT_STUCK_AFTER,
        Some(other) => {
            let minutes = to_plain_string(other)
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|minutes| minutes.is_finite() && *minutes >= 0.0)
                .ok_or_else(|| HostError::message("stuckMinutes is not a number of minutes"))?;
            std::time::Duration::from_secs_f64(minutes * 60.0)
        }
    };
    let report = search::feed::process_pending(ids.as_deref(), stuck_after).map_err(HostError::Lib)?;
    let mut out = indexmap::IndexMap::default();
    let count = |value: usize| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    out.insert("documentsIndexed".to_string(), count(report.documents_indexed));
    out.insert("documentsDeleted".to_string(), count(report.documents_deleted));
    out.insert("rowsDone".to_string(), count(report.rows_done));
    out.insert("rowsLeft".to_string(), count(report.rows_left));
    out.insert("rowsHeld".to_string(), count(report.rows_held));
    out.insert("rowsStuck".to_string(), count(report.rows_stuck));
    out.insert("rowsPending".to_string(), count(report.rows_pending));
    out.insert(
        "oldestPendingSeconds".to_string(),
        report.oldest_pending_seconds.map_or(Value::Null, |seconds| {
            Value::I64(i64::try_from(seconds).unwrap_or(i64::MAX))
        }),
    );
    out.insert(
        "stuckError".to_string(),
        report.stuck(stuck_after).map_or(Value::Null, Value::String),
    );
    Ok(Ret::Value(Value::Map(out)))
}

fn timestamp_at(value: Option<&Value>) -> Option<jiff::Timestamp> {
    value.and_then(crate::search::postgres::compile::parse_date_bound)
}

fn rebuild_feed(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = options_arg(call)?;
    let feed = map
        .get("dataFeedId")
        .map(to_plain_string)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| HostError::message("rebuildFeed needs a dataFeedId"))?;
    let document = map
        .get("dataDocumentId")
        .map(to_plain_string)
        .filter(|text| !text.is_empty());
    let rebuilt = search::feed::rebuild_feed(
        &feed,
        document.as_deref(),
        timestamp_at(map.get("fromUpdateStamp")),
        timestamp_at(map.get("thruUpdateStamp")),
        number(map.get("batchSize"), 1000),
    )
    .map_err(HostError::Lib)?;
    Ok(count(rebuilt))
}

#[cfg(test)]
mod tests {
    use super::{fresh_by, MAX_FRESH_TIMEOUT};
    use moqui_lib::object::Value;
    use std::time::{Duration, Instant};

    #[test]
    fn a_huge_fresh_timeout_waits_at_most_the_maximum() {
        let mut map = indexmap::IndexMap::default();
        map.insert("consistency".to_string(), Value::string("readYourWrites"));
        map.insert("freshTimeout".to_string(), Value::string("18446744073709551615"));
        let before = Instant::now();
        let deadline = fresh_by(&map).expect("a deadline");
        assert!(deadline <= Instant::now() + Duration::from_millis(MAX_FRESH_TIMEOUT));
        assert!(deadline >= before + Duration::from_millis(MAX_FRESH_TIMEOUT));
        map.insert("freshTimeout".to_string(), Value::I32(250));
        let deadline = fresh_by(&map).expect("a deadline");
        assert!(deadline <= Instant::now() + Duration::from_millis(250));
    }
}
