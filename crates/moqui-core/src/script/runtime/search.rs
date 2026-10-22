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

//! `ec.search`, and the data-document calls on `ec.entity`.
//!
//! Each `ec.search` call takes one options map. A target is `documentType` (one id, a
//! comma-separated list, or a list) or `indexName`. Query and aggregation trees use the map form
//! in `crate::search::query` and `crate::search::agg`; a query may also be a query string.
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
//! ec.search.capabilities() -> names: proximity, fuzzy, autocomplete, weightedRank, accent, semantic, hybrid
//! ec.search.flatten(map) -> map with dotted keys
//! ec.search.processFeedPending(ids?) -> {documentsIndexed, documentsDeleted, rowsDone, rowsLeft}
//! ec.search.rebuildFeed({dataFeedId, dataDocumentId, fromUpdateStamp, thruUpdateStamp, batchSize}) -> count
//! ec.entity.getDataDocuments(dataDocumentId, condition?, fromUpdateStamp?, thruUpdateStamp?) -> list
//! ```
//!
//! `retrieval: "hybrid"` or `"semantic"` with `text`, `k`, `weights`, `fuzzyBelow`, and `qualityBoost`
//! fuses branches (`search::hybrid`). `rank: "coverDensity"` ranks with the `searchWeight` field weights. `consistency: "readYourWrites"` waits up to `freshTimeout` milliseconds (default 5000, at most 60000) for this
//! session's writes to be indexed and, with a read replica, replayed there; see `search::fresh`.

use super::*;
use crate::search::{self, Agg, Query, SearchRequest, SortField, Target};

pub(super) fn search_object<'js>(ctx: &Ctx<'js>) -> Result<Object<'js>, Error> {
    let object = Object::new(ctx.clone()).map_err(js_error)?;
    macro_rules! method {
        ($name:literal, $function:expr) => {
            let function = Function::new(ctx.clone(), $function).map_err(js_error)?;
            object.set($name, function).map_err(js_error)?;
        };
    }
    method!("search", search_search);
    method!("aggregate", search_aggregate);
    method!("parseQuery", search_parse_query);
    method!("elasticQuery", search_elastic_query);
    method!("indexDocuments", search_index_documents);
    method!("deleteDocument", search_delete_document);
    method!("deleteMatching", search_delete_matching);
    method!("getDocument", search_get_document);
    method!("ensureIndex", search_ensure_index);
    method!("dropIndex", search_drop_index);
    method!("indexExists", search_index_exists);
    method!("documentTypes", search_document_types);
    method!("isAvailable", search_is_available);
    method!("hasElasticApi", search_has_elastic_api);
    method!("backendName", search_backend_name);
    method!("capabilities", search_capabilities);
    method!("embedPending", search_embed_pending);
    method!("flatten", search_flatten);
    method!("processFeedPending", search_process_feed_pending);
    method!("rebuildFeed", search_rebuild_feed);
    Ok(object)
}

fn facade(ctx: &Ctx<'_>) -> rquickjs::Result<std::sync::Arc<search::SearchFacade>> {
    search::current().map_err(|err| throw_lib(ctx, &err))
}

fn lib<T>(ctx: &Ctx<'_>, result: Result<T, moqui_lib::error::Error>) -> rquickjs::Result<T> {
    result.map_err(|err| throw_lib(ctx, &err))
}

fn options(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<indexmap::IndexMap<String, Value>> {
    match js_to_host(ctx, value)? {
        Value::Map(map) => Ok(map),
        Value::Null => Ok(indexmap::IndexMap::new()),
        _ => Err(Exception::throw_message(ctx, "ec.search takes an options map")),
    }
}

fn text_list(value: Option<&Value>) -> Vec<String> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::List(items)) => items
            .iter()
            .map(moqui_lib::object::to_plain_string)
            .filter(|text| !text.trim().is_empty())
            .map(|text| text.trim().to_string())
            .collect(),
        Some(other) => moqui_lib::object::to_plain_string(other)
            .split(',')
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect(),
    }
}

fn number(value: Option<&Value>, default: usize) -> usize {
    value
        .map(moqui_lib::object::to_plain_string)
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(default)
}

fn target_of(ctx: &Ctx<'_>, map: &indexmap::IndexMap<String, Value>) -> rquickjs::Result<Target> {
    let documents = text_list(map.get("documentType").or_else(|| map.get("documentTypes")));
    if !documents.is_empty() {
        return Ok(Target::Documents(documents));
    }
    match map.get("indexName").map(moqui_lib::object::to_plain_string) {
        Some(name) if !name.is_empty() => Ok(Target::Index(name)),
        _ => Err(Exception::throw_message(
            ctx,
            "a search needs documentType or indexName",
        )),
    }
}

fn query_of(ctx: &Ctx<'_>, value: Option<&Value>) -> rquickjs::Result<Query> {
    match value {
        None | Some(Value::Null) => Ok(Query::MatchAll),
        Some(value) => lib(ctx, Query::from_value(value)),
    }
}

pub(super) fn search_search<'js>(ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let map = options(&ctx, &args)?;
        let target = target_of(&ctx, &map)?;
        let facade = facade(&ctx)?;
        // The log viewer's string goes to OpenSearch as written, as query_string did in Java.
        let log_index = matches!(&target, Target::Index(name) if facade.is_log_index(name));
        let mut query = query_of(&ctx, map.get("query"))?;
        if let Some(text) = map.get("queryString").map(moqui_lib::object::to_plain_string) {
            let parsed = if log_index {
                Query::Lucene(text)
            } else {
                lib(&ctx, search::parse::parse_query_string(&text))?
            };
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
        request.retrieval = retrieval(&ctx, &map)?;
        // A trusted constraint from the service, such as authorization; it does not score.
        request.filter = match map.get("filter") {
            None | Some(Value::Null) => None,
            Some(value) => Some(lib(&ctx, Query::from_value(value))?),
        };
        request.rank = match map.get("rank").map(moqui_lib::object::to_plain_string).as_deref() {
            None | Some("") | Some("default") => search::Rank::Default,
            Some("coverDensity") => search::Rank::CoverDensity,
            Some(other) => {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("search rank {other} is not default or coverDensity"),
                ))
            }
        };
        let page = lib(&ctx, facade.search(&target, &request))?;
        let mut hits = Vec::with_capacity(page.hits.len());
        for hit in page.hits {
            let mut entry = indexmap::IndexMap::new();
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
        let mut out = indexmap::IndexMap::new();
        out.insert("hits".to_string(), Value::List(hits));
        out.insert(
            "total".to_string(),
            Value::I64(i64::try_from(page.total).unwrap_or(i64::MAX)),
        );
        out.insert("totalRelation".to_string(), Value::string(page.total_relation.name()));
        host_to_js(&ctx, Value::Map(out))
    })
}

/// `retrieval: "hybrid" | "semantic"` with `text`, `k`, `weights: {lexical, prefix, fuzzy, semantic}`,
/// `fuzzyBelow`, and `qualityBoost`. Absent or `"lexical"` is the query alone.
fn retrieval(ctx: &Ctx<'_>, map: &indexmap::IndexMap<String, Value>) -> rquickjs::Result<search::Retrieval> {
    let kind = map
        .get("retrieval")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    if kind.is_empty() || kind == "lexical" {
        return Ok(search::Retrieval::Lexical);
    }
    let float = |value: Option<&Value>, default: f64| {
        value
            .map(moqui_lib::object::to_plain_string)
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or(default)
    };
    let mut options = search::hybrid::HybridOptions {
        text: map
            .get("text")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default(),
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
        other => Err(Exception::throw_message(
            ctx,
            &format!("search retrieval {other} is not lexical, hybrid, or semantic"),
        )),
    }
}

/// The longest a read-your-writes search waits, in milliseconds: the wait holds a request worker.
const MAX_FRESH_TIMEOUT: u64 = 60_000;

/// The read-your-writes deadline from `consistency` and `freshTimeout`, which is at most [`MAX_FRESH_TIMEOUT`].
fn fresh_by(map: &indexmap::IndexMap<String, Value>) -> Option<std::time::Instant> {
    let consistency = map.get("consistency").map(moqui_lib::object::to_plain_string)?;
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

pub(super) fn search_embed_pending<'js>(ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let map = options(&ctx, &args)?;
        let target = target_of(&ctx, &map)?;
        let batch = number(map.get("batch"), 100);
        let max_chars = number(map.get("maxChars"), 2000);
        let stored = lib(&ctx, facade(&ctx)?.embed_pending(&target, batch, max_chars))?;
        host_to_js(&ctx, Value::I64(i64::try_from(stored).unwrap_or(i64::MAX)))
    })
}

pub(super) fn search_capabilities<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let names = facade(&ctx)?
            .capabilities()
            .into_iter()
            .map(|capability| Value::string(capability.name()))
            .collect();
        host_to_js(&ctx, Value::List(names))
    })
}

pub(super) fn search_aggregate<'js>(ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let map = options(&ctx, &args)?;
        let target = target_of(&ctx, &map)?;
        let filter = query_of(&ctx, map.get("filter").or_else(|| map.get("query")))?;
        let aggs = lib(&ctx, Agg::map_from_value(map.get("aggs").unwrap_or(&Value::Null)))?;
        let results = lib(
            &ctx,
            facade(&ctx)?.aggregate_fresh(&target, &filter, &aggs, fresh_by(&map)),
        )?;
        host_to_js(&ctx, Value::Map(results))
    })
}

pub(super) fn search_parse_query<'js>(ctx: Ctx<'js>, text: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let query = lib(&ctx, search::parse::parse_query_string(&text))?;
        host_to_js(&ctx, query.to_value())
    })
}

pub(super) fn search_elastic_query<'js>(ctx: Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let value = js_to_host(&ctx, &value)?;
        let query = lib(&ctx, search::es_json::from_es(&value))?;
        host_to_js(&ctx, query.to_value())
    })
}

pub(super) fn search_index_documents<'js>(ctx: Ctx<'js>, list: JsValue<'js>) -> rquickjs::Result<i64> {
    host_guard(&ctx, || {
        let documents: Vec<crate::entity::document::Document> = match js_to_host(&ctx, &list)? {
            Value::List(items) => items
                .into_iter()
                .filter_map(|item| match item {
                    Value::Map(map) => Some(map),
                    _ => None,
                })
                .collect(),
            Value::Null => Vec::new(),
            _ => {
                return Err(Exception::throw_message(
                    &ctx,
                    "indexDocuments takes a list of documents",
                ))
            }
        };
        let count = lib(&ctx, facade(&ctx)?.index_documents(&documents))?;
        Ok(i64::try_from(count).unwrap_or(i64::MAX))
    })
}

pub(super) fn search_delete_document(ctx: Ctx<'_>, document_type: String, id: String) -> rquickjs::Result<bool> {
    host_guard(&ctx, || lib(&ctx, facade(&ctx)?.delete_document(&document_type, &id)))
}

pub(super) fn search_delete_matching<'js>(ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<i64> {
    host_guard(&ctx, || {
        let map = options(&ctx, &args)?;
        let target = target_of(&ctx, &map)?;
        let filter = query_of(&ctx, map.get("filter").or_else(|| map.get("query")))?;
        let count = lib(&ctx, facade(&ctx)?.delete_matching(&target, &filter))?;
        Ok(i64::try_from(count).unwrap_or(i64::MAX))
    })
}

pub(super) fn search_get_document<'js>(
    ctx: Ctx<'js>,
    document_type: String,
    id: String,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let found = lib(&ctx, facade(&ctx)?.get_document(&document_type, &id))?;
        host_to_js(&ctx, found.map_or(Value::Null, Value::Map))
    })
}

/// A document type name, or an options map with `documentType` or `indexName`.
fn document_types(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<Vec<String>> {
    if let Some(text) = value.as_string() {
        return Ok(text_list(Some(&Value::String(text.to_string()?))));
    }
    let map = options(ctx, value)?;
    let target = target_of(ctx, &map)?;
    lib(ctx, facade(ctx)?.target_documents(&target))
}

pub(super) fn search_ensure_index<'js>(ctx: Ctx<'js>, target: JsValue<'js>) -> rquickjs::Result<i64> {
    host_guard(&ctx, || {
        let ids = document_types(&ctx, &target)?;
        let search = facade(&ctx)?;
        for id in &ids {
            lib(&ctx, search.ensure_index(id))?;
        }
        Ok(i64::try_from(ids.len()).unwrap_or(i64::MAX))
    })
}

pub(super) fn search_drop_index<'js>(ctx: Ctx<'js>, target: JsValue<'js>) -> rquickjs::Result<i64> {
    host_guard(&ctx, || {
        let ids = document_types(&ctx, &target)?;
        let search = facade(&ctx)?;
        for id in &ids {
            lib(&ctx, search.drop_index(id))?;
        }
        Ok(i64::try_from(ids.len()).unwrap_or(i64::MAX))
    })
}

pub(super) fn search_document_types<'js>(ctx: Ctx<'js>, target: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let ids = document_types(&ctx, &target)?;
        host_to_js(&ctx, Value::List(ids.into_iter().map(Value::String).collect()))
    })
}

pub(super) fn search_index_exists<'js>(ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        let map = options(&ctx, &args)?;
        let target = target_of(&ctx, &map)?;
        lib(&ctx, facade(&ctx)?.target_exists(&target))
    })
}

pub(super) fn search_is_available(ctx: Ctx<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(search::current().is_ok()))
}

/// Whether the search backend answers the Elasticsearch REST API (OpenSearch), so a proxy to it has something to reach.
pub(super) fn search_has_elastic_api(ctx: Ctx<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || {
        Ok(search::current().is_ok_and(|found| found.backend().has_elastic_api()))
    })
}

pub(super) fn search_backend_name<'js>(ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || match search::current() {
        Ok(found) => found.backend().name().into_js(&ctx),
        Err(_) => Null.into_js(&ctx),
    })
}

pub(super) fn search_flatten<'js>(ctx: Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || match js_to_host(&ctx, &value)? {
        Value::Map(map) => host_to_js(&ctx, Value::Map(moqui_lib::collection::flatten_nested_map(&map))),
        other => host_to_js(&ctx, other),
    })
}

pub(super) fn search_process_feed_pending<'js>(
    ctx: Ctx<'js>,
    ids: Opt<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        let ids = match ids.0 {
            Some(value) => match js_to_host(&ctx, &value)? {
                Value::Null => None,
                other => Some(text_list(Some(&other))),
            },
            None => None,
        };
        let report = lib(&ctx, search::feed::process_pending(ids.as_deref()))?;
        let mut out = indexmap::IndexMap::new();
        let count = |value: usize| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
        out.insert("documentsIndexed".to_string(), count(report.documents_indexed));
        out.insert("documentsDeleted".to_string(), count(report.documents_deleted));
        out.insert("rowsDone".to_string(), count(report.rows_done));
        out.insert("rowsLeft".to_string(), count(report.rows_left));
        host_to_js(&ctx, Value::Map(out))
    })
}

fn timestamp_at(value: Option<&Value>) -> Option<jiff::Timestamp> {
    value.and_then(crate::search::postgres::compile::parse_date_bound)
}

pub(super) fn search_rebuild_feed<'js>(ctx: Ctx<'js>, args: JsValue<'js>) -> rquickjs::Result<i64> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        let map = options(&ctx, &args)?;
        let feed = map
            .get("dataFeedId")
            .map(moqui_lib::object::to_plain_string)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| Exception::throw_message(&ctx, "rebuildFeed needs a dataFeedId"))?;
        let document = map
            .get("dataDocumentId")
            .map(moqui_lib::object::to_plain_string)
            .filter(|text| !text.is_empty());
        let count = lib(
            &ctx,
            search::feed::rebuild_feed(
                &feed,
                document.as_deref(),
                timestamp_at(map.get("fromUpdateStamp")),
                timestamp_at(map.get("thruUpdateStamp")),
                number(map.get("batchSize"), 1000),
            ),
        )?;
        Ok(i64::try_from(count).unwrap_or(i64::MAX))
    })
}

/// `ec.entity.getDataDocuments(dataDocumentId, condition?, fromUpdateStamp?, thruUpdateStamp?)`.
/// The condition is one from `conditionFactory` or a map of alias to value.
pub(super) fn entity_get_data_documents<'js>(
    ctx: Ctx<'js>,
    data_document_id: String,
    rest: Rest<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        let facade = facade_of(&ctx)?;
        let condition = match rest.0.first() {
            None => None,
            Some(value) if value.is_null() || value.is_undefined() => None,
            Some(value) => match condition_of(value) {
                Some(condition) => Some(condition),
                None => match js_to_host(&ctx, value)? {
                    Value::Map(map) => join(
                        map.into_iter()
                            .map(|(field, value)| Condition::equals(field.as_str(), value))
                            .collect(),
                        "and",
                    ),
                    _ => {
                        return Err(Exception::throw_message(
                            &ctx,
                            "getDataDocuments takes a condition or a map",
                        ))
                    }
                },
            },
        };
        let stamp = |index: usize| -> rquickjs::Result<Option<jiff::Timestamp>> {
            match rest.0.get(index) {
                None => Ok(None),
                Some(value) => Ok(timestamp_at(Some(&js_to_host(&ctx, value)?))),
            }
        };
        let (from, thru) = (stamp(1)?, stamp(2)?);
        let documents = lib(
            &ctx,
            crate::entity::document::data_documents(&facade, &data_document_id, condition, from, thru),
        )?;
        host_to_js(&ctx, Value::List(documents.into_iter().map(Value::Map).collect()))
    })
}

/// The host side of `ec.entity.getEntityDataDocument()`'s writers: `mode` `text` gives `{text, count}` for
/// `writeDocumentsToWriter`, `file` and `dir` write under `path` (`writeDocumentsToFile`, `writeDocumentsToDirectory`)
/// and give `{count}`. After the condition come `fromUpdateStamp`, `thruUpdateStamp` and `prettyPrint`.
pub(super) fn write_data_documents<'js>(
    ctx: Ctx<'js>,
    mode: String,
    path: String,
    ids: Vec<String>,
    condition: JsValue<'js>,
    rest: Rest<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let undefined = JsValue::new_undefined(ctx.clone());
        let from = rest.0.first().cloned().unwrap_or_else(|| undefined.clone());
        let thru = rest.0.get(1).cloned().unwrap_or_else(|| undefined.clone());
        let pretty = rest.0.get(2).and_then(JsValue::as_bool).unwrap_or(false);
        flush_current(&ctx, true)?;
        let facade = facade_of(&ctx)?;
        let condition = if condition.is_null() || condition.is_undefined() {
            None
        } else {
            match condition_of(&condition) {
                Some(condition) => Some(condition),
                None => match js_to_host(&ctx, &condition)? {
                    Value::Map(map) => join(
                        map.into_iter()
                            .map(|(field, value)| Condition::equals(field.as_str(), value))
                            .collect(),
                        "and",
                    ),
                    _ => {
                        return Err(Exception::throw_message(
                            &ctx,
                            "the document writers take a condition or a map",
                        ))
                    }
                },
            }
        };
        let stamp = |value: &JsValue<'js>| -> rquickjs::Result<Option<jiff::Timestamp>> {
            if value.is_null() || value.is_undefined() {
                return Ok(None);
            }
            Ok(timestamp_at(Some(&js_to_host(&ctx, value)?)))
        };
        let (from, thru) = (stamp(&from)?, stamp(&thru)?);
        let mut out = indexmap::IndexMap::new();
        let count = match mode.as_str() {
            "text" => {
                let (text, count) = lib(
                    &ctx,
                    crate::entity::document::documents_json(&facade, &ids, condition, from, thru, pretty),
                )?;
                out.insert("text".to_string(), Value::String(text));
                count
            }
            "file" => lib(
                &ctx,
                crate::entity::document::write_documents_to_file(
                    &facade,
                    std::path::Path::new(&path),
                    &ids,
                    condition,
                    from,
                    thru,
                    pretty,
                ),
            )?,
            "dir" => lib(
                &ctx,
                crate::entity::document::write_documents_to_directory(
                    &facade,
                    std::path::Path::new(&path),
                    &ids,
                    condition,
                    from,
                    thru,
                    pretty,
                ),
            )?,
            other => {
                return Err(Exception::throw_message(
                    &ctx,
                    &format!("unknown document write mode {other}"),
                ))
            }
        };
        out.insert(
            "count".to_string(),
            Value::I64(i64::try_from(count).unwrap_or(i64::MAX)),
        );
        host_to_js(&ctx, Value::Map(out))
    })
}

#[cfg(test)]
mod tests {
    use super::{fresh_by, MAX_FRESH_TIMEOUT};
    use moqui_lib::object::Value;
    use std::time::{Duration, Instant};

    #[test]
    fn a_huge_fresh_timeout_waits_at_most_the_maximum() {
        let mut map = indexmap::IndexMap::new();
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
