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

//! The OpenSearch backend: the shared `_search`, `_bulk`, and mapping subset of OpenSearch and
//! ElasticSearch 7+, sent through the blocking [`RestClient`].
//!
//! The compiler emits the JSON the Java services emitted. `Text` is a `query_string` in `must`,
//! so it scores. `Field` clauses are `term`, `terms`, `prefix`, `wildcard`, `range`, and `exists`.
//! `Child` is `nested`. Many-relationships are mapped `nested` with `include_in_root`, so dotted
//! names and OpenSearch Dashboards keep working. A date histogram sends `calendar_interval`.
//! `CountDistinct` sends `precision_threshold` and is approximate past it. Each stored document
//! carries `_indexed_at`, the index time a retention delete filters on.
//!
//! Highlights are made from `_source` by [`super::highlight`], the same as on Postgres.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use indexmap::IndexMap;
use jiff::Timestamp;
use moqui_lib::error::Error;
use moqui_lib::http::RestClient;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Map, Value as Json};

use super::agg::{histogram_zone, zone_name, Agg, DateHistogram, Terms, TermsOrder};
use super::conf::ClusterConf;
use super::mapping::{DocumentMapping, FieldKind, ObjectMapping, Resolved};
use super::postgres::body_json_map;
use super::postgres::compile::parse_date_bound;
use super::query::{FieldOp, FieldQuery, Query, TextFields, TextMatch};
use super::{lock, Capability, Rank, SearchBackend, SearchHit, SearchPage, SearchRequest, SortField};
use crate::entity::document::Document;

/// The index-time field each stored document carries.
pub const INDEXED_AT: &str = "_indexed_at";
const PRECISION_THRESHOLD: u32 = 40_000;

/// A mapping as Elastic JSON: `{_meta, properties}`.
/// The keyword subfield of the `title` role, lower-cased by [`NORMALIZER`], for autocomplete.
pub const NORM_SUBFIELD: &str = "norm";
/// The normalizer [`NORM_SUBFIELD`] uses.
pub const NORMALIZER: &str = "moqui_fold";

/// Index settings the mapping needs: the title normalizer when there is a title.
pub fn settings_json(mapping: &DocumentMapping) -> Option<Json> {
    mapping.display.role("title")?;
    Some(json!({"analysis": {"normalizer": {NORMALIZER: {"type": "custom", "filter": ["lowercase"]}}}}))
}

fn mapping_signature(mapping: &DocumentMapping) -> String {
    let mut signature = mapping.signature_hash();
    if super::telemetry::is_telemetry_index(&mapping.document_id) {
        // Include the opaque-payload policy, which is backend-specific, in compatibility checks.
        signature.push_str("-opaque-payload-v1");
    }
    signature
}

pub fn mapping_json(mapping: &DocumentMapping) -> Json {
    let mut properties = object_properties(&mapping.root);
    if let Some(Json::Object(title)) = mapping.display.role("title").and_then(|name| properties.get_mut(name)) {
        let fields = title.entry("fields").or_insert_with(|| json!({}));
        if let Json::Object(fields) = fields {
            fields.insert(
                NORM_SUBFIELD.into(),
                json!({"type": "keyword", "normalizer": NORMALIZER}),
            );
        }
    }
    properties.insert(
        INDEXED_AT.to_string(),
        json!({"type": "date", "format": "strict_date_optional_time||epoch_millis"}),
    );
    properties.insert(
        "__cursor".into(),
        json!({"type":"text","fields":{"keyword":{"type":"keyword"}}}),
    );
    let mut result = json!({
        "_meta": {"moqui_document_id": mapping.document_id, "moqui_signature": mapping_signature(mapping)},
        "properties": properties,
    });
    if super::telemetry::is_telemetry_index(&mapping.document_id) {
        // Producer payloads are retained in _source but must not create mappings, or a
        // scalar custom attribute can collide with a later dotted semantic field.
        result["dynamic"] = json!(false);
        for field in ["attributes", "resource", "events"] {
            result["properties"][field] = json!({"type":"object","enabled":false});
        }
    }
    result
}

fn object_properties(object: &ObjectMapping) -> Map<String, Json> {
    let mut properties = Map::new();
    for (name, field) in &object.fields {
        let mut property = Map::new();
        property.insert("type".into(), Json::String(field.kind.search_type().to_string()));
        if field.kind == FieldKind::Date {
            property.insert(
                "format".into(),
                Json::String("strict_date_optional_time||epoch_millis||yyyy-MM-dd HH:mm:ss.SSS||yyyy-MM-dd".into()),
            );
        }
        if field.kind == FieldKind::Text && field.sortable {
            property.insert("fields".into(), json!({"keyword": {"type": "keyword"}}));
        }
        properties.insert(name.clone(), Json::Object(property));
    }
    for (name, child) in &object.children {
        properties.insert(
            name.clone(),
            json!({"type": "nested", "include_in_root": true, "properties": object_properties(child)}),
        );
    }
    properties
}

/// The name a query, sort, or aggregation sends for a field. Sortable text sorts and buckets on
/// its `.keyword` subfield. Other `.keyword` suffixes are dropped, since the field is one.
fn field_name(mapping: &DocumentMapping, name: &str, unanalyzed: bool) -> String {
    match name {
        // A log index has its own `@timestamp`, the event time.
        "@timestamp" if mapping.root.fields.contains_key(name) => return name.to_string(),
        "@timestamp" | "_timestamp" | "indexed_at" => return INDEXED_AT.to_string(),
        "_id" | "_score" => return name.to_string(),
        _ => {}
    }
    let base = name.strip_suffix(".keyword").unwrap_or(name);
    match mapping.resolve(base) {
        Resolved::Root { field, .. } | Resolved::Child { field, .. }
            if field.kind == FieldKind::Text && field.sortable && (unanalyzed || name.ends_with(".keyword")) =>
        {
            format!("{base}.keyword")
        }
        _ => base.to_string(),
    }
}

fn kind_of(mapping: &DocumentMapping, name: &str) -> Option<FieldKind> {
    match name {
        "@timestamp" if mapping.root.fields.contains_key(name) => Some(FieldKind::Date),
        "@timestamp" | "_timestamp" | "indexed_at" => Some(FieldKind::Date),
        _ => mapping.resolve(name).field().map(|field| field.kind),
    }
}

/// Lucene query-string escaping for one term.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for ch in text.chars() {
        if "+-=&|><!(){}[]^\"~*?:\\/ ".contains(ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// A field value for Elastic: dates as epoch millis, decimals as numbers.
fn value_json(kind: Option<FieldKind>, value: &Value) -> Json {
    if kind == Some(FieldKind::Date) {
        if let Some(at) = parse_date_bound(value) {
            return Json::from(at.as_millisecond());
        }
    }
    moqui_lib::json::to_json(value)
}

/// [`Query`] to Elastic query JSON. `path` is the enclosing nested path, which full names need.
pub fn query_json(mapping: &DocumentMapping, query: &Query) -> Json {
    compile(mapping, query, "")
}

fn full(prefix: &str, name: &str) -> String {
    if prefix.is_empty() || name.starts_with(prefix) {
        name.to_string()
    } else {
        format!("{prefix}{name}")
    }
}

fn compile(mapping: &DocumentMapping, query: &Query, prefix: &str) -> Json {
    if prefix.is_empty() {
        if let Some(query) = super::telemetry::attribute_query(mapping, query) {
            return compile(mapping, &query, prefix);
        }
    }
    match query {
        Query::MatchAll => json!({"match_all": {}}),
        Query::Lucene(text) => json!({"query_string": {"query": text, "lenient": true, "analyze_wildcard": true}}),
        Query::Near {
            terms,
            fields,
            distance,
        } => {
            let names: Vec<String> = match fields {
                // Every text field in scope: intervals need one field each.
                TextFields::All => mapping
                    .textual_fields()
                    .into_iter()
                    .filter(|(name, field)| field.kind == FieldKind::Text && name.starts_with(prefix))
                    .map(|(name, _)| name)
                    .collect(),
                TextFields::Only(names) => names
                    .iter()
                    .map(|name| field_name(mapping, &full(prefix, name), false))
                    .collect(),
            };
            let should: Vec<Json> = names
                .iter()
                .map(|name| {
                    json!({"intervals": {name: {"match": {"query": terms, "max_gaps": distance, "ordered": true}}}})
                })
                .collect();
            if should.is_empty() {
                json!({"match_none": {}})
            } else {
                json!({"bool": {"should": should, "minimum_should_match": 1}})
            }
        }
        Query::Boost { query, weight } => {
            json!({"function_score":{"query":compile(mapping, query, prefix),"boost":weight}})
        }
        Query::FuzzyIn { terms, fields } => {
            let fields = match fields {
                TextFields::Only(names) => TextFields::Only(names.iter().map(|name| full(prefix, name)).collect()),
                TextFields::All => TextFields::All,
            };
            let fields = super::query::fuzzy_field_names(mapping, &fields)
                .into_iter()
                .filter(|name| prefix.is_empty() || name.starts_with(prefix))
                .collect::<Vec<_>>();
            if fields.is_empty() {
                json!({"match_none":{}})
            } else {
                json!({"multi_match":{"query":terms,"fields":fields,"fuzziness":"AUTO","operator":"and"}})
            }
        }
        Query::Fuzzy { terms } => match mapping.display.role("title") {
            Some(title) if prefix.is_empty() && !terms.trim().is_empty() => {
                json!({"match": {title: {"query": terms.trim(), "fuzziness": "AUTO", "operator": "and"}}})
            }
            _ => json!({"match_none": {}}),
        },
        Query::Suggest { text } => match mapping.display.role("title") {
            Some(_) if !prefix.is_empty() => json!({"match_none": {}}),
            Some(_) if text.trim().is_empty() => json!({"match_all": {}}),
            Some(title) => json!({"bool": {"should": [
                {"prefix": {format!("{title}.{NORM_SUBFIELD}"): {"value": text.trim().to_lowercase(), "boost": 2.0}}},
                {"match_bool_prefix": {title: {"query": text.trim(), "operator": "and"}}}
            ], "minimum_should_match": 1}}),
            None => json!({"match_none": {}}),
        },
        Query::Text { terms, fields, kind } => {
            let text = match kind {
                TextMatch::Term => terms.split_whitespace().map(escape).collect::<Vec<_>>().join(" "),
                TextMatch::Prefix => terms
                    .split_whitespace()
                    .map(|word| format!("{}*", escape(word)))
                    .collect::<Vec<_>>()
                    .join(" "),
                TextMatch::Contains => format!("*{}*", escape(terms)),
                TextMatch::Phrase => format!("\"{}\"", terms.replace('\\', "\\\\").replace('"', "\\\"")),
                // Already escaped by the parser: `\*` is literal, `*` and `?` are wildcards.
                TextMatch::Wildcard => terms.clone(),
            };
            let names: Vec<Json> = match fields {
                TextFields::All => vec![Json::String("*".into())],
                TextFields::Only(names) => names
                    .iter()
                    .map(|name| Json::String(field_name(mapping, &full(prefix, name), false)))
                    .collect(),
            };
            json!({"query_string": {"query": text, "fields": names, "default_operator": "AND",
                "lenient": true, "analyze_wildcard": true}})
        }
        Query::Field(field) if prefix.is_empty() && field.name == super::query::DOCUMENT_FIELD => {
            if field.document_match(&mapping.document_id).unwrap_or(false) {
                json!({"match_all": {}})
            } else {
                json!({"match_none": {}})
            }
        }
        Query::Field(field) => {
            let positive = field_json(mapping, field, prefix);
            if field.negate {
                json!({"bool": {"must_not": [positive]}})
            } else {
                positive
            }
        }
        Query::Bool {
            all,
            any,
            none,
            min_any,
        } => {
            let mut body = Map::new();
            let list = |items: &[Query]| Json::Array(items.iter().map(|item| compile(mapping, item, prefix)).collect());
            if !all.is_empty() {
                body.insert("must".into(), list(all));
            }
            if !any.is_empty() {
                body.insert("should".into(), list(any));
            }
            if !none.is_empty() {
                body.insert("must_not".into(), list(none));
            }
            if let Some(min) = min_any {
                body.insert("minimum_should_match".into(), Json::from(*min));
            }
            json!({"bool": body})
        }
        Query::Child { path, query } => {
            let path = full(prefix, path);
            let inner = compile(mapping, query, &format!("{path}."));
            json!({"nested": {"path": path, "query": inner, "score_mode": "avg"}})
        }
    }
}

fn field_json(mapping: &DocumentMapping, field: &FieldQuery, prefix: &str) -> Json {
    let name = full(prefix, &field.name);
    let kind = kind_of(mapping, name.strip_suffix(".keyword").unwrap_or(&name));
    let sent = field_name(mapping, &name, false);
    match field.op {
        FieldOp::Exists => json!({"exists": {"field": sent}}),
        FieldOp::Eq if kind == Some(FieldKind::Text) => {
            json!({"match": {sent: {"query": to_plain_string(&field.value), "operator": "and"}}})
        }
        FieldOp::Eq => json!({"term": {sent: value_json(kind, &field.value)}}),
        FieldOp::In => {
            let values: Vec<Json> = if field.values.is_empty() {
                vec![value_json(kind, &field.value)]
            } else {
                field.values.iter().map(|value| value_json(kind, value)).collect()
            };
            json!({"terms": {sent: values}})
        }
        FieldOp::Prefix => json!({"prefix": {sent: to_plain_string(&field.value)}}),
        FieldOp::Wildcard => json!({"wildcard": {sent: to_plain_string(&field.value)}}),
        FieldOp::Range => {
            let mut bounds = Map::new();
            for (key, bound) in [
                ("gt", &field.gt),
                ("gte", &field.gte),
                ("lt", &field.lt),
                ("lte", &field.lte),
            ] {
                if let Some(bound) = bound {
                    // Relative dates resolve here, as on Postgres, so both backends read one instant.
                    let rounded = (kind == Some(FieldKind::Date) && matches!(key, "gt" | "lte"))
                        .then(|| super::postgres::compile::date_range_bound(bound, true))
                        .flatten();
                    bounds.insert(
                        key.into(),
                        rounded.map_or_else(|| value_json(kind, bound), |at| Json::from(at.as_millisecond())),
                    );
                }
            }
            json!({"range": {sent: bounds}})
        }
    }
}

/// The query of a request for one document type. Cover-density ranking adds the positive text
/// terms as an optional `multi_match` over the weighted fields, boosted A 4 through D 1, so the
/// weights change the score and not what matches.
pub fn request_query(mapping: &DocumentMapping, request: &SearchRequest) -> Json {
    let query = scored_query(mapping, request);
    match &request.filter {
        // The trusted filter narrows without scoring.
        Some(filter) => json!({"bool": {"must": [query], "filter": [query_json(mapping, filter)]}}),
        None => query,
    }
}

fn scored_query(mapping: &DocumentMapping, request: &SearchRequest) -> Json {
    let query = query_json(mapping, &request.query);
    if request.rank != Rank::CoverDensity || mapping.display.weights.is_empty() {
        return query;
    }
    let mut terms = Vec::new();
    positive_terms(&request.query, false, &mut terms);
    if terms.is_empty() {
        return query;
    }
    let fields: Vec<Json> = mapping
        .display
        .weights
        .iter()
        .map(|(name, weight)| {
            let boost = match weight {
                'A' => 4,
                'B' => 3,
                'C' => 2,
                _ => 1,
            };
            Json::String(format!("{name}^{boost}"))
        })
        .collect();
    json!({"bool": {"must": [query], "should": [
        {"multi_match": {"query": terms.join(" "), "fields": fields, "type": "best_fields"}}
    ]}})
}

fn positive_terms(query: &Query, negated: bool, out: &mut Vec<String>) {
    match query {
        Query::Text { terms, .. } | Query::Near { terms, .. } if !negated => out.push(terms.clone()),
        Query::Bool { all, any, none, .. } => {
            for item in all.iter().chain(any) {
                positive_terms(item, negated, out);
            }
            for item in none {
                positive_terms(item, !negated, out);
            }
        }
        Query::Child { query, .. } => positive_terms(query, negated, out),
        _ => {}
    }
}

/// The `_search` body for a request.
pub fn search_json(mapping: &DocumentMapping, request: &SearchRequest) -> Json {
    search_body(&[mapping], request)
}

/// The `_search` body over several document types: the query is the first type's, and each sort
/// key is resolved against every type, see [`sort_json`].
fn search_body(mappings: &[&DocumentMapping], request: &SearchRequest) -> Json {
    let mut body = Map::new();
    if let Some(first) = mappings.first() {
        body.insert("query".into(), request_query(first, request));
    }
    body.insert("from".into(), Json::from(request.offset));
    body.insert("size".into(), Json::from(request.limit));
    if let Some(after) = &request.search_after {
        body.remove("from");
        body.insert(
            "search_after".into(),
            Json::Array(
                after
                    .iter()
                    .enumerate()
                    .map(|(index, value)| {
                        let long = request.sort.get(index).is_some_and(|sort| {
                            mappings
                                .iter()
                                .any(|mapping| kind_of(mapping, &sort.field) == Some(FieldKind::Long))
                        });
                        if long {
                            if let Ok(value) = to_plain_string(value).parse::<i64>() {
                                return json!(value);
                            }
                        }
                        moqui_lib::json::to_json(value)
                    })
                    .collect(),
            ),
        );
    }
    body.insert("track_total_hits".into(), Json::Bool(true));
    if !request.sort.is_empty() {
        let sort: Vec<Json> = request.sort.iter().map(|sort| sort_json(mappings, sort)).collect();
        body.insert("sort".into(), Json::Array(sort));
    }
    Json::Object(body)
}

/// One sort clause. The name is the `.keyword` subfield when any type maps the field as sortable
/// text, since sorting a text field fails. `unmapped_type` is the field's kind (keyword when the
/// types disagree, as the Postgres backend does), so an index without the field sorts its hits
/// last instead of failing its shards.
fn sort_json(mappings: &[&DocumentMapping], sort: &SortField) -> Json {
    let order = if sort.descending { "desc" } else { "asc" };
    if sort.field == "__cursor" {
        return json!({"__cursor.keyword":{"order":order,"missing":"_last","unmapped_type":"keyword"}});
    }
    let mut name: Option<String> = None;
    let mut kind: Option<FieldKind> = None;
    for mapping in mappings {
        let Some(found) = kind_of(mapping, sort.field.strip_suffix(".keyword").unwrap_or(&sort.field)) else {
            continue;
        };
        let resolved = field_name(mapping, &sort.field, true);
        if name.is_none() || resolved.ends_with(".keyword") {
            name = Some(resolved);
        }
        kind = match kind {
            None => Some(found),
            Some(existing) if existing == found => Some(existing),
            Some(_) => Some(FieldKind::Keyword),
        };
    }
    let name = name.unwrap_or_else(|| {
        mappings
            .first()
            .map_or_else(|| sort.field.clone(), |mapping| field_name(mapping, &sort.field, true))
    });
    match name.as_str() {
        "_score" => json!({"_score": {"order": order}}),
        "_id" => json!({"_id": {"order": order}}),
        _ => {
            let mode = if sort.descending { "max" } else { "min" };
            let unmapped = match kind {
                Some(FieldKind::Text) | None => "keyword",
                Some(kind) => kind.search_type(),
            };
            json!({name: {"order": order, "missing": "_last", "mode": mode, "unmapped_type": unmapped}})
        }
    }
}

/// Named aggregations as Elastic JSON.
pub fn aggs_json(mapping: &DocumentMapping, aggs: &IndexMap<String, Agg>) -> Result<Json, Error> {
    let mut out = Map::new();
    for (name, agg) in aggs {
        out.insert(name.clone(), agg_json(mapping, agg)?);
    }
    Ok(Json::Object(out))
}

fn agg_json(mapping: &DocumentMapping, agg: &Agg) -> Result<Json, Error> {
    let sub = |aggs: &IndexMap<String, Agg>, body: &mut Map<String, Json>| -> Result<(), Error> {
        if !aggs.is_empty() {
            body.insert("aggs".into(), aggs_json(mapping, aggs)?);
        }
        Ok(())
    };
    Ok(match agg {
        Agg::TopHits { size, sort } => {
            json!({"top_hits":{"size":size,"sort":sort.iter().map(|s|sort_json(&[mapping],s)).collect::<Vec<_>>()}})
        }
        Agg::HistogramMerge { bounds } => json!({"scripted_metric":{
            "params":{"bounds":bounds},
            "init_script":"state.buckets = new long[params.bounds.size() + 1]; state.compatible = true;",
            "map_script":"def point = params._source; if (point.bounds == null || point.buckets == null || point.bounds.size() != params.bounds.size() || point.buckets.size() != state.buckets.length) { state.compatible = false; return; } for (int i = 0; i < params.bounds.size(); i++) { if (point.bounds[i].doubleValue() != params.bounds[i].doubleValue()) { state.compatible = false; return; } } for (int i = 0; i < state.buckets.length; i++) { state.buckets[i] += point.buckets[i].longValue(); }",
            "combine_script":"return state;",
            "reduce_script":"def buckets = new long[params.bounds.size() + 1]; for (def part : states) { if (!part.compatible) { throw new IllegalArgumentException('cannot merge metric histogram boundaries'); } for (int i = 0; i < buckets.length; i++) { buckets[i] += part.buckets[i]; } } return ['bounds': params.bounds, 'buckets': buckets];"
        }}),
        Agg::Histogram {
            field,
            interval,
            offset,
            aggs,
        } => {
            let mut body = json!({"histogram":{"field":field_name(mapping,field,false),"interval":interval,"offset":offset,"min_doc_count":1}});
            body["aggs"] = aggs_json(mapping, aggs)?;
            body
        }
        Agg::NamedFilters { filters, aggs } => {
            let filters: Map<String, Json> = filters
                .iter()
                .map(|(k, q)| (k.clone(), query_json(mapping, q)))
                .collect();
            json!({"filters":{"filters":filters},"aggs":aggs_json(mapping,aggs)?})
        }
        Agg::Metric { field, kind } => match kind {
            super::agg::Metric::Avg => json!({"avg":{"field":field_name(mapping,field,false)}}),
            super::agg::Metric::ValueCount => json!({"value_count":{"field":field_name(mapping,field,true)}}),
            super::agg::Metric::Percentile(percent) => {
                json!({"percentiles":{"field":field_name(mapping,field,false),"percents":[percent]}})
            }
        },
        Agg::Sum { field } => json!({"sum": {"field": field_name(mapping, field, false)}}),
        Agg::Extreme { field, max } => {
            let mut inner = json!({"field":field_name(mapping,field,false)});
            if kind_of(mapping, field) == Some(FieldKind::Date) {
                inner["format"] = json!("strict_date_optional_time");
            }
            json!({(if *max { "max" } else { "min" }): inner})
        }
        Agg::CountDistinct { field } => json!({"cardinality": {"field": field_name(mapping, field, true),
            "precision_threshold": PRECISION_THRESHOLD}}),
        Agg::Filter { query, aggs } => {
            let mut body = Map::new();
            body.insert("filter".into(), query_json(mapping, query));
            sub(aggs, &mut body)?;
            Json::Object(body)
        }
        Agg::DateHistogram(histogram) => {
            let zone = histogram_zone(histogram.time_zone.as_deref())?;
            let mut inner = Map::new();
            inner.insert(
                "field".into(),
                Json::String(field_name(mapping, &histogram.field, false)),
            );
            inner.insert(
                if histogram.fixed_millis.is_some() {
                    "fixed_interval"
                } else {
                    "calendar_interval"
                }
                .into(),
                Json::String(
                    histogram
                        .fixed_millis
                        .map(|ms| format!("{ms}ms"))
                        .unwrap_or_else(|| histogram.unit.calendar_interval().into()),
                ),
            );
            if let Some((min, max)) = histogram.extended_bounds {
                inner.insert(
                    "extended_bounds".into(),
                    json!({"min":min.as_millisecond(),"max":max.as_millisecond()}),
                );
            }
            inner.insert("time_zone".into(), Json::String(zone_name(&zone)));
            if let Some(format) = &histogram.format {
                inner.insert("format".into(), Json::String(format.clone()));
            }
            if let Some(offset) = &histogram.offset_text {
                inner.insert("offset".into(), Json::String(offset.clone()));
            }
            inner.insert(
                "order".into(),
                json!({"_key": if histogram.descending { "desc" } else { "asc" }}),
            );
            inner.insert("min_doc_count".into(), Json::from(histogram.min_doc_count));
            let mut body = Map::new();
            body.insert("date_histogram".into(), Json::Object(inner));
            sub(&histogram.aggs, &mut body)?;
            Json::Object(body)
        }
        Agg::Terms(terms) => {
            let order = match &terms.order {
                TermsOrder::Count { descending } => json!({"_count": dir(*descending)}),
                TermsOrder::Key { descending } => json!({"_key": dir(*descending)}),
                TermsOrder::Metric { name, descending } => json!({name: dir(*descending)}),
            };
            let mut body = Map::new();
            body.insert(
                "terms".into(),
                json!({"field": field_name(mapping, &terms.field, true), "size": terms.size, "order": order}),
            );
            if let Some(Json::Object(inner)) = body.get_mut("terms") {
                inner.insert("min_doc_count".into(), json!(terms.min_doc_count));
            }
            sub(&terms.aggs, &mut body)?;
            Json::Object(body)
        }
    })
}

fn dir(descending: bool) -> &'static str {
    if descending {
        "desc"
    } else {
        "asc"
    }
}

/// Read an Elastic `aggregations` response into the facade's result shape.
pub fn read_aggs(aggs: &IndexMap<String, Agg>, response: &Json) -> IndexMap<String, Value> {
    let mut out = IndexMap::new();
    for (name, agg) in aggs {
        let part = response.get(name).unwrap_or(&Json::Null);
        out.insert(name.clone(), read_agg(agg, part));
    }
    out
}

fn read_agg(agg: &Agg, part: &Json) -> Value {
    match agg {
        Agg::TopHits { .. } => Value::List(
            part.pointer("/hits/hits")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .map(|hit| {
                    moqui_lib::json::from_json(
                        &json!({"id":hit["_id"],"documentId":hit["_moqui_document_type"],"body":hit["_source"]}),
                    )
                })
                .collect(),
        ),
        Agg::HistogramMerge { .. } => moqui_lib::json::from_json(part.get("value").unwrap_or(&Json::Null)),
        Agg::Histogram { aggs, .. } => {
            let buckets: Vec<Value> = part["buckets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|bucket| {
                    let mut map = read_aggs(aggs, bucket);
                    map.insert("key".into(), moqui_lib::json::from_json(&bucket["key"]));
                    map.insert(
                        "doc_count".into(),
                        Value::I64(bucket["doc_count"].as_i64().unwrap_or(0)),
                    );
                    Value::Map(map)
                })
                .collect();
            Value::Map(IndexMap::from([("buckets".into(), Value::List(buckets))]))
        }
        Agg::NamedFilters { aggs, .. } => {
            let buckets: Vec<Value> = part["buckets"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.iter())
                .map(|(key, bucket)| {
                    let mut map = read_aggs(aggs, bucket);
                    map.insert("key".into(), Value::string(key));
                    map.insert(
                        "doc_count".into(),
                        Value::I64(bucket["doc_count"].as_i64().unwrap_or(0)),
                    );
                    Value::Map(map)
                })
                .collect();
            Value::Map(IndexMap::from([("buckets".into(), Value::List(buckets))]))
        }
        Agg::Metric { kind, .. } => match kind {
            super::agg::Metric::Percentile(p) => {
                // The key is Java's rendering of the requested double ("99.0", "99.95"); match it by
                // value. Each request asks for one percent, so a lone entry is that percent.
                let value = match part.get("values") {
                    Some(Json::Object(values)) => values
                        .iter()
                        .find(|(key, _)| key.parse::<f64>().is_ok_and(|key| (key - p).abs() < 1e-9))
                        .map(|(_, value)| value)
                        .or_else(|| values.values().next().filter(|_| values.len() == 1)),
                    Some(Json::Array(values)) => values
                        .iter()
                        .find(|entry| entry["key"].as_f64().is_some_and(|key| (key - p).abs() < 1e-9))
                        .map(|entry| &entry["value"]),
                    _ => None,
                };
                moqui_lib::json::from_json(value.unwrap_or(&Json::Null))
            }
            _ => moqui_lib::json::from_json(part.get("value").unwrap_or(&Json::Null)),
        },
        Agg::Sum { .. } => {
            let value = part.get("value").cloned().unwrap_or(Json::Null);
            match moqui_lib::json::from_json(&value) {
                Value::Null => Value::Decimal(Default::default()),
                Value::I64(whole) => Value::Decimal(whole.into()),
                Value::F64(float) => moqui_lib::object::basic_convert(&Value::F64(float), "BigDecimal").value,
                other => other,
            }
        }
        Agg::Extreme { .. } => {
            if let Some(time) = part
                .get("value_as_string")
                .and_then(Json::as_str)
                .and_then(|s| s.parse::<Timestamp>().ok())
            {
                Value::Timestamp(time)
            } else {
                match moqui_lib::json::from_json(part.get("value").unwrap_or(&Json::Null)) {
                    Value::I64(whole) => Value::Decimal(whole.into()),
                    Value::F64(float) => moqui_lib::object::basic_convert(&Value::F64(float), "BigDecimal").value,
                    other => other,
                }
            }
        }
        Agg::CountDistinct { .. } => Value::I64(part.get("value").and_then(Json::as_i64).unwrap_or(0)),
        Agg::Filter { aggs, .. } => {
            let mut map = IndexMap::new();
            map.insert(
                "doc_count".into(),
                Value::I64(part.get("doc_count").and_then(Json::as_i64).unwrap_or(0)),
            );
            map.extend(read_aggs(aggs, part));
            Value::Map(map)
        }
        Agg::DateHistogram(DateHistogram { aggs, .. }) | Agg::Terms(Terms { aggs, .. }) => {
            let histogram = matches!(agg, Agg::DateHistogram(_));
            let mut buckets = Vec::new();
            for bucket in part.get("buckets").and_then(Json::as_array).into_iter().flatten() {
                let mut map = IndexMap::new();
                let key = bucket.get("key").cloned().unwrap_or(Json::Null);
                if histogram {
                    let at = key.as_i64().and_then(|millis| Timestamp::from_millisecond(millis).ok());
                    if let (Some(at), Agg::DateHistogram(histogram)) = (at, agg) {
                        if let Ok(partial) = super::agg::partial_bucket(histogram, at) {
                            if partial && histogram.drop_partial {
                                continue;
                            }
                            if partial {
                                map.insert("partial".into(), Value::Bool(true));
                            }
                        }
                    }
                    map.insert("key".into(), at.map_or(Value::Null, Value::Timestamp));
                    let shown = bucket.get("key_as_string").and_then(Json::as_str).unwrap_or_default();
                    map.insert("key_display".into(), Value::string(shown));
                } else {
                    map.insert("key".into(), moqui_lib::json::from_json(&key));
                }
                map.insert(
                    "doc_count".into(),
                    Value::I64(bucket.get("doc_count").and_then(Json::as_i64).unwrap_or(0)),
                );
                map.extend(read_aggs(aggs, bucket));
                buckets.push(Value::Map(map));
            }
            let mut map = IndexMap::new();
            map.insert("buckets".into(), Value::List(buckets));
            Value::Map(map)
        }
    }
}

pub struct OpenSearchBackend {
    client: RestClient,
    base_url: String,
    authorization: Option<String>,
    index_prefix: String,
    known: Mutex<HashMap<String, String>>,
}

impl OpenSearchBackend {
    fn index_documents_with_refresh(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
        read_at: Option<Timestamp>,
        deferred_refresh: bool,
        outcomes: Option<&mut Vec<super::TelemetryItem>>,
    ) -> Result<usize, Error> {
        if documents.is_empty() {
            return Ok(0);
        }
        let index = self.index_name(mapping);
        let now = read_at.unwrap_or_else(Timestamp::now).as_millisecond();
        let mut text = String::new();
        // Preserve the legacy generated-id write behavior; exporter deferral is explicit.
        let generated_ids = documents.iter().all(|(id, _)| id.is_empty());
        for (id, body) in documents {
            let generated;
            let id = if id.is_empty() {
                generated = super::postgres::generated_doc_id();
                &generated
            } else {
                id
            };
            let action = if id.is_empty() {
                json!({"index": {"_index": index}})
            } else if read_at.is_some() {
                json!({"index": {"_index": index, "_id": id, "version": now, "version_type": "external_gte"}})
            } else {
                json!({"index": {"_index": index, "_id": id}})
            };
            text.push_str(&action.to_string());
            text.push('\n');
            // Dates as ISO-8601, which the date format reads, wherever the script or the caller gave them as text.
            let mut typed = body.clone();
            mapping.root.type_dates(&mut typed);
            let mut source = body_json_map(&typed);
            if let Json::Object(map) = &mut source {
                map.insert(INDEXED_AT.into(), Json::from(now));
                map.insert("__cursor".into(), json!(format!("{}:{id}", mapping.document_id)));
            }
            text.push_str(&source.to_string());
            text.push('\n');
        }
        let path = if deferred_refresh || generated_ids {
            "_bulk"
        } else {
            "_bulk?refresh=wait_for"
        };
        let response = self.checked("POST", path, Some(Body::NdJson(text)))?;
        if let Some(outcomes) = outcomes {
            let items = response
                .get("items")
                .and_then(Json::as_array)
                .filter(|items| items.len() == documents.len())
                .ok_or_else(|| Error::new("OpenSearch bulk response has incomplete item acknowledgements"))?;
            for item in items {
                let item = &item["index"];
                let status = item["status"].as_u64().unwrap_or(0);
                let error = item["error"].to_string();
                outcomes.push(if (200..300).contains(&status) {
                    super::TelemetryItem::Accepted
                } else if status == 0 || matches!(status, 408 | 429) || status >= 500 {
                    super::TelemetryItem::Retryable(error)
                } else {
                    super::TelemetryItem::Rejected(error)
                });
            }
            return Ok(documents.len());
        }
        if response.get("errors").and_then(Json::as_bool) == Some(true) {
            // A version conflict is a document already stored from a later read; it stays.
            let first = response.get("items").and_then(Json::as_array).and_then(|items| {
                items.iter().find_map(|item| {
                    let error = item.pointer("/index/error")?;
                    let conflict =
                        error.get("type").and_then(Json::as_str) == Some("version_conflict_engine_exception");
                    (!conflict).then(|| error.get("reason").cloned().unwrap_or(Json::Null))
                })
            });
            if let Some(reason) = first {
                return Err(Error::new(format!("OpenSearch bulk index failed: {reason}")));
            }
        }
        Ok(documents.len())
    }

    pub fn new(conf: &ClusterConf) -> Self {
        let authorization = conf.user.as_ref().map(|user| {
            let pair = format!("{user}:{}", conf.password.as_deref().unwrap_or_default());
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(pair))
        });
        Self {
            client: RestClient::new(),
            base_url: conf.url.trim_end_matches('/').to_string(),
            authorization,
            index_prefix: conf.index_prefix.clone().unwrap_or_default(),
            known: Mutex::new(HashMap::new()),
        }
    }

    /// True when the cluster answers `GET /`.
    pub fn reachable(&self) -> bool {
        matches!(self.send("GET", "/", None), Ok((status, _)) if status < 300)
    }

    pub fn index_name(&self, mapping: &DocumentMapping) -> String {
        format!("{}{}", self.index_prefix, mapping.physical_name)
    }

    fn send(&self, method: &str, path: &str, body: Option<Body>) -> Result<(u16, Json), Error> {
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        let method =
            moqui_lib::http::Method::parse(method).ok_or_else(|| Error::new(format!("HTTP method {method}")))?;
        let mut call = self.client.request(method, url);
        if let Some(timeout) = super::limits::remaining()? {
            call = call.timeout(timeout);
        }
        if let Some(auth) = &self.authorization {
            call = call.header("authorization", auth.clone());
        }
        call = match body {
            Some(Body::Json(json)) => call.json(&json),
            Some(Body::NdJson(text)) => call.text("application/x-ndjson", text),
            None => call,
        };
        let response = call.send(false)?;
        let status = response.status();
        let json = if response.body().trim().is_empty() {
            Json::Null
        } else {
            serde_json::from_str(response.body()).unwrap_or_else(|_| Json::String(response.body().to_string()))
        };
        Ok((status, json))
    }

    fn checked(&self, method: &str, path: &str, body: Option<Body>) -> Result<Json, Error> {
        let (status, json) = self.send(method, path, body)?;
        if status >= 300 {
            return Err(status_error(method, path, status, &json));
        }
        Ok(json)
    }

    /// [`Self::checked`] for a `_search`. A response where some shards failed is an error: its
    /// hits, total, and aggregations miss those shards' documents, and no caller can tell.
    fn searched(&self, path: &str, body: Json) -> Result<Json, Error> {
        let mut body = body;
        if let Some(timeout) = super::limits::remaining()? {
            body["timeout"] = json!(format!("{}ms", timeout.as_millis().max(1)));
        }
        let response = self.checked("POST", path, Some(Body::Json(body)))?;
        if response["timed_out"] == true {
            return Err(Error::new("report query timeout exceeded"));
        }
        if let Some(reason) = shard_failure(&response) {
            return Err(Error::new(format!(
                "OpenSearch POST {path} failed on some shards: {reason}"
            )));
        }
        Ok(response)
    }

    fn create(&self, mapping: &DocumentMapping) -> Result<(), Error> {
        let index = self.index_name(mapping);
        let mut mappings = mapping_json(mapping);
        let mut settings = Json::Null;
        if let Some(service) = &mapping.manual_mapping_service {
            let mut params = std::collections::BTreeMap::new();
            params.insert("mapping".to_string(), moqui_lib::json::from_json(&mappings));
            // A failure fails the index creation, so a later ensure tries again instead of keeping
            // an index made with the default mapping under this signature.
            let result = super::feed::call_checked(service, &params).map_err(|err| {
                Error::new(format!(
                    "manual mapping service for {} failed: {}",
                    mapping.document_id,
                    err.message()
                ))
            })?;
            if let Some(changed) = result.get("mapping").filter(|value| !value.is_null()) {
                mappings = moqui_lib::json::to_json(changed);
            }
            if let Some(given) = result.get("settings").filter(|value| !value.is_null()) {
                settings = moqui_lib::json::to_json(given);
            }
        }
        if let Some(needed) = settings_json(mapping) {
            settings = match settings {
                Json::Object(mut given) => {
                    if let Json::Object(needed) = needed {
                        for (key, value) in needed {
                            given.entry(key).or_insert(value);
                        }
                    }
                    Json::Object(given)
                }
                _ => needed,
            };
        }
        let mut body = Map::new();
        body.insert("mappings".into(), mappings);
        if !settings.is_null() {
            body.insert("settings".into(), settings);
        }
        if let Some(alias) = &mapping.index_name {
            body.insert("aliases".into(), json!({format!("{}{alias}", self.index_prefix): {}}));
        }
        let (status, json) = self.send("PUT", &index, Some(Body::Json(Json::Object(body))))?;
        // Another thread or node made it between the mapping check and here: the same definition.
        let exists = json.pointer("/error/type").and_then(Json::as_str) == Some("resource_already_exists_exception");
        if status >= 300 && !exists {
            return Err(status_error("PUT", &index, status, &json));
        }
        Ok(())
    }
}

enum Body {
    Json(Json),
    NdJson(String),
}

impl SearchBackend for OpenSearchBackend {
    fn upsert_counter(
        &self,
        mapping: &DocumentMapping,
        id: &str,
        document: &Document,
        counter: &str,
    ) -> Result<(), Error> {
        let mut typed = document.clone();
        mapping.root.type_dates(&mut typed);
        let body = body_json_map(&typed);
        let increment = body
            .get(counter)
            .and_then(Json::as_i64)
            .ok_or_else(|| Error::new("counter increment must be an integer"))?;
        let script="def first = ctx._source.first_seen; def last = ctx._source.last_seen; def count = ctx._source.containsKey(params.counter) ? ctx._source[params.counter] : 0; ctx._source.putAll(params.document); ctx._source[params.counter] = count + params.increment; if (first != null && first.compareTo(ctx._source.first_seen) < 0) { ctx._source.first_seen = first; } if (last != null && last.compareTo(ctx._source.last_seen) > 0) { ctx._source.last_seen = last; }";
        let request = json!({"script":{"lang":"painless","source":script,"params":{"counter":counter,"increment":increment,"document":body}},"upsert":body});
        self.checked(
            "POST",
            &format!(
                "{}/_update/{}?retry_on_conflict=8",
                self.index_name(mapping),
                url_part(id)
            ),
            Some(Body::Json(request)),
        )?;
        Ok(())
    }

    fn has_elastic_api(&self) -> bool {
        true
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::Proximity,
            Capability::Fuzzy,
            Capability::Autocomplete,
            Capability::WeightedRank,
            Capability::Hybrid,
        ]
    }

    fn name(&self) -> &'static str {
        "opensearch"
    }

    fn ensure_index(&self, mapping: &DocumentMapping) -> Result<(), Error> {
        let signature = mapping_signature(mapping);
        if lock(&self.known).get(&mapping.document_id) == Some(&signature) {
            return Ok(());
        }
        let index = self.index_name(mapping);
        let (status, json) = self.send("GET", &format!("{index}/_mapping"), None)?;
        if status == 200 {
            let stored = json
                .get(&index)
                .and_then(|entry| entry.pointer("/mappings/_meta/moqui_signature"))
                .and_then(Json::as_str);
            if stored == Some(signature.as_str()) {
                lock(&self.known).insert(mapping.document_id.clone(), signature);
                return Ok(());
            }
            if super::telemetry::is_telemetry_index(&mapping.document_id)
                || super::telemetry::is_telemetry_index(&mapping.physical_name)
            {
                return Err(Error::new(format!("telemetry index {} mapping changed; existing history is preserved. Apply an explicit non-destructive mapping migration before retrying",mapping.document_id)));
            }
            tracing::warn!(index = %index, "search index shape changed; the index is rebuilt and must be reindexed");
            self.checked("DELETE", &index, None)?;
        } else if status != 404 {
            return Err(status_error("GET", &format!("{index}/_mapping"), status, &json));
        }
        self.create(mapping)?;
        lock(&self.known).insert(mapping.document_id.clone(), signature);
        Ok(())
    }

    fn index_exists(&self, mapping: &DocumentMapping) -> Result<bool, Error> {
        if lock(&self.known).contains_key(&mapping.document_id) {
            return Ok(true);
        }
        // Only a 404 is a missing index. An unreachable, overloaded, or unauthorized cluster is an
        // error, so a search does not come back empty and a feed delete is not taken as done.
        let path = format!("{}/_mapping", self.index_name(mapping));
        let (status, json) = self.send("GET", &path, None)?;
        match status {
            404 => Ok(false),
            300.. => Err(status_error("GET", &path, status, &json)),
            _ => Ok(true),
        }
    }

    fn index_documents(&self, mapping: &DocumentMapping, documents: &[(String, Document)]) -> Result<usize, Error> {
        self.index_documents_at(mapping, documents, None)
    }

    /// With a read time, the version is its epoch millis with `external_gte`, so a document read earlier than the
    /// stored one is refused, and that refusal is not an error.
    fn index_documents_at(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
        read_at: Option<Timestamp>,
    ) -> Result<usize, Error> {
        self.index_documents_with_refresh(mapping, documents, read_at, false, None)
    }

    fn index_telemetry_documents(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
    ) -> Result<usize, Error> {
        if !super::telemetry::is_telemetry_index(&mapping.document_id) {
            return Err(Error::new("deferred refresh is restricted to telemetry indexes"));
        }
        self.index_documents_with_refresh(mapping, documents, None, true, None)
    }

    fn index_telemetry_batch(
        &self,
        mapping: &DocumentMapping,
        documents: &[(String, Document)],
    ) -> Result<Vec<super::TelemetryItem>, Error> {
        if !super::telemetry::is_telemetry_index(&mapping.document_id) {
            return Err(Error::new("telemetry batch requires a telemetry index"));
        }
        let mut outcomes = Vec::with_capacity(documents.len());
        self.index_documents_with_refresh(mapping, documents, None, true, Some(&mut outcomes))?;
        Ok(outcomes)
    }

    fn delete_document(&self, mapping: &DocumentMapping, id: &str) -> Result<bool, Error> {
        let path = format!("{}/_doc/{}?refresh=wait_for", self.index_name(mapping), url_part(id));
        let (status, json) = self.send("DELETE", &path, None)?;
        match status {
            200 => Ok(true),
            404 => Ok(false),
            _ => Err(Error::new(format!(
                "OpenSearch DELETE {path} returned {status}: {json}"
            ))),
        }
    }

    fn delete_matching(&self, mapping: &DocumentMapping, filter: &Query) -> Result<u64, Error> {
        let path = format!("{}/_delete_by_query?refresh=true", self.index_name(mapping));
        let body = json!({"query": query_json(mapping, filter)});
        let response = self.checked("POST", &path, Some(Body::Json(body)))?;
        let deleted = response.get("deleted").and_then(Json::as_u64).unwrap_or(0);
        // A delete that stopped part way answers 200 with its failures. It is an error, so a
        // retention run reports it and the next run deletes the rest.
        if let Some(failures) = response
            .get("failures")
            .and_then(Json::as_array)
            .filter(|failures| !failures.is_empty())
        {
            let first = failures.first().map_or_else(String::new, |failure| {
                failure
                    .pointer("/cause/reason")
                    .and_then(Json::as_str)
                    .map_or_else(|| failure.to_string(), str::to_string)
            });
            return Err(Error::new(format!(
                "OpenSearch POST {path} deleted {deleted} and had {} failures: {first}",
                failures.len()
            )));
        }
        if response.get("timed_out").and_then(Json::as_bool) == Some(true) {
            return Err(Error::new(format!(
                "OpenSearch POST {path} timed out after deleting {deleted}"
            )));
        }
        Ok(deleted)
    }

    fn get_document(&self, mapping: &DocumentMapping, id: &str) -> Result<Option<Document>, Error> {
        let path = format!("{}/_doc/{}", self.index_name(mapping), url_part(id));
        let (status, json) = self.send("GET", &path, None)?;
        if status == 404 || json.get("found").and_then(Json::as_bool) == Some(false) {
            return Ok(None);
        }
        if status >= 300 {
            return Err(Error::new(format!("OpenSearch GET {path} returned {status}")));
        }
        Ok(Some(source_of(json.get("_source"))))
    }

    fn search(&self, mappings: &[Arc<DocumentMapping>], request: &SearchRequest) -> Result<SearchPage, Error> {
        if mappings.is_empty() {
            return Ok(SearchPage::default());
        }
        let indexes: Vec<String> = mappings.iter().map(|mapping| self.index_name(mapping)).collect();
        let refs: Vec<&DocumentMapping> = mappings.iter().map(|mapping| &**mapping).collect();
        let mut body = search_body(&refs, request);
        if mappings.len() > 1 {
            // Each type's fields and roles differ: its own query, only on its own index.
            let should: Vec<Json> = mappings
                .iter()
                .zip(&indexes)
                .map(|(mapping, index)| {
                    json!({"bool": {"filter": [{"term": {"_index": index}}], "must": [request_query(mapping, request)]}})
                })
                .collect();
            if let Json::Object(map) = &mut body {
                map.insert(
                    "query".into(),
                    json!({"bool": {"should": should, "minimum_should_match": 1}}),
                );
            }
        }
        let response = self.searched(&format!("{}/_search", indexes.join(",")), body)?;
        let total = response
            .pointer("/hits/total/value")
            .and_then(Json::as_u64)
            .unwrap_or(0);
        let mut hits = Vec::new();
        for hit in response
            .pointer("/hits/hits")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let index = hit.get("_index").and_then(Json::as_str).unwrap_or_default();
            let physical = index.strip_prefix(&self.index_prefix).unwrap_or(index).to_string();
            let document_id = mappings
                .iter()
                .find(|mapping| mapping.physical_name == physical)
                .map_or_else(
                    || super::mapping::document_id_of(&physical),
                    |mapping| mapping.document_id.clone(),
                );
            let mut body = source_of(hit.get("_source"));
            if request.sort.last().is_some_and(|s| s.field == "__cursor") {
                for (index, sort) in request.sort.iter().enumerate().filter(|(_, s)| s.field != "__cursor") {
                    if !body.contains_key(&sort.field) {
                        if let Some(value) = hit
                            .get("sort")
                            .and_then(Json::as_array)
                            .and_then(|values| values.get(index))
                        {
                            body.insert(sort.field.clone(), moqui_lib::json::from_json(value));
                        }
                    }
                }
            }
            let highlights = if request.highlight.is_empty() {
                IndexMap::new()
            } else {
                super::highlight::highlights(&request.query, &request.highlight, &body)
            };
            hits.push(SearchHit {
                document_id,
                id: hit.get("_id").and_then(Json::as_str).unwrap_or_default().to_string(),
                index: physical,
                score: hit.get("_score").and_then(Json::as_f64).unwrap_or(0.0),
                body,
                highlights,
            });
        }
        Ok(SearchPage {
            hits,
            total,
            total_relation: super::TotalRelation::Eq,
        })
    }

    fn aggregate(
        &self,
        mapping: &DocumentMapping,
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
    ) -> Result<IndexMap<String, Value>, Error> {
        let body = json!({"size": 0, "query": {"bool": {"filter": [query_json(mapping, filter)]}},
            "aggs": aggs_json(mapping, aggs)?});
        let response = self.searched(&format!("{}/_search", self.index_name(mapping)), body)?;
        fn bucket_count(value: &Json) -> usize {
            match value {
                Json::Object(map) => map
                    .iter()
                    .map(|(key, v)| {
                        if key == "buckets" {
                            v.as_array()
                                .map_or_else(|| v.as_object().map_or(0, |m| m.len()), Vec::len)
                                + bucket_count(v)
                        } else {
                            bucket_count(v)
                        }
                    })
                    .sum(),
                Json::Array(items) => items.iter().map(bucket_count).sum(),
                _ => 0,
            }
        }
        if bucket_count(&response["aggregations"]) > super::limits::buckets() {
            return Err(super::limits::too_many_groups("report query returned too many groups"));
        }
        let mut result = read_aggs(aggs, response.get("aggregations").unwrap_or(&Json::Null));
        super::agg::fill_other_buckets(self, mapping, filter, aggs, &mut result)?;
        Ok(result)
    }

    fn aggregate_many(
        &self,
        mappings: &[Arc<DocumentMapping>],
        filter: &Query,
        aggs: &IndexMap<String, Agg>,
        _fresh_by: Option<std::time::Instant>,
    ) -> Result<IndexMap<String, Value>, Error> {
        let merged = super::mapping::combined_mapping(mappings)?;
        let population = |query: &Query| json!({"bool":{"should":mappings.iter().map(|mapping|json!({"bool":{"filter":[{"term":{"_index":self.index_name(mapping)}},query_json(mapping,query)]}})).collect::<Vec<_>>(),"minimum_should_match":1}});
        fn compile(
            mapping: &DocumentMapping,
            aggs: &IndexMap<String, Agg>,
            population: &dyn Fn(&Query) -> Json,
        ) -> Result<Json, Error> {
            let mut body = aggs_json(mapping, aggs)?;
            for (name, agg) in aggs {
                match agg {
                    Agg::Filter { query, .. } => body[name]["filter"] = population(query),
                    Agg::NamedFilters { filters, .. } => {
                        body[name]["filters"]["filters"] = Json::Object(
                            filters
                                .iter()
                                .map(|(name, query)| (name.clone(), population(query)))
                                .collect(),
                        )
                    }
                    Agg::Terms(terms) if terms.field == super::query::DOCUMENT_FIELD => {
                        body[name]["terms"]["field"] = json!("_index")
                    }
                    _ => {}
                }
                if let Some(nested) = agg.sub_aggs() {
                    if !nested.is_empty() {
                        body[name]["aggs"] = compile(mapping, nested, population)?;
                    }
                }
            }
            Ok(body)
        }
        let indexes = mappings
            .iter()
            .map(|mapping| self.index_name(mapping))
            .collect::<Vec<_>>();
        let mut response = self.searched(
            &format!("{}/_search", indexes.join(",")),
            json!({"size":0,"query":population(filter),"aggs":compile(&merged,aggs,&population)?}),
        )?;
        fn count(value: &Json) -> usize {
            match value {
                Json::Object(map) => map
                    .iter()
                    .map(|(key, value)| {
                        if key == "buckets" {
                            value
                                .as_array()
                                .map_or_else(|| value.as_object().map_or(0, |map| map.len()), Vec::len)
                                + count(value)
                        } else {
                            count(value)
                        }
                    })
                    .sum(),
                Json::Array(values) => values.iter().map(count).sum(),
                _ => 0,
            }
        }
        if count(&response["aggregations"]) > super::limits::buckets() {
            return Err(super::limits::too_many_groups(
                "combined aggregation returned too many groups",
            ));
        }
        let identities = mappings
            .iter()
            .map(|mapping| (self.index_name(mapping), mapping.document_id.clone()))
            .collect::<std::collections::HashMap<_, _>>();
        rewrite_population_response(aggs, &mut response["aggregations"], &identities);
        let mut result = read_aggs(aggs, &response["aggregations"]);
        let mut again =
            |filter: &Query, aggs: &IndexMap<String, Agg>| self.aggregate_many(mappings, filter, aggs, None);
        super::agg::fill_other_buckets_with(&mut again, filter, aggs, &mut result)?;
        Ok(result)
    }

    fn drop_index(&self, mapping: &DocumentMapping) -> Result<(), Error> {
        let index = self.index_name(mapping);
        let (status, json) = self.send("DELETE", &index, None)?;
        lock(&self.known).remove(&mapping.document_id);
        if status >= 300 && status != 404 {
            return Err(Error::new(format!(
                "OpenSearch DELETE {index} returned {status}: {json}"
            )));
        }
        Ok(())
    }
}

fn rewrite_population_response(
    aggs: &IndexMap<String, Agg>,
    response: &mut Json,
    identities: &std::collections::HashMap<String, String>,
) {
    for (name, agg) in aggs {
        let part = &mut response[name];
        if let Agg::TopHits { .. } = agg {
            for hit in part["hits"]["hits"].as_array_mut().into_iter().flatten() {
                if let Some(identity) = hit["_index"].as_str().and_then(|index| identities.get(index)) {
                    hit["_moqui_document_type"] = json!(identity);
                }
            }
        }
        if let Agg::Terms(terms) = agg {
            if terms.field == super::query::DOCUMENT_FIELD {
                for bucket in part["buckets"].as_array_mut().into_iter().flatten() {
                    if let Some(identity) = bucket["key"].as_str().and_then(|index| identities.get(index)) {
                        bucket["key"] = json!(identity);
                    }
                }
            }
        }
        if let Some(nested) = agg.sub_aggs() {
            if matches!(agg, Agg::Filter { .. }) {
                rewrite_population_response(nested, part, identities);
            } else if let Some(buckets) = part["buckets"].as_array_mut() {
                for bucket in buckets {
                    rewrite_population_response(nested, bucket, identities);
                }
            } else if let Some(buckets) = part["buckets"].as_object_mut() {
                for bucket in buckets.values_mut() {
                    rewrite_population_response(nested, bucket, identities);
                }
            }
        }
    }
}

/// The error for a status at or above 300, with the cluster's reason when it gave one.
fn status_error(method: &str, path: &str, status: u16, json: &Json) -> Error {
    let reason = json
        .pointer("/error/reason")
        .and_then(Json::as_str)
        .map_or_else(|| json.to_string(), str::to_string);
    Error::new(format!("OpenSearch {method} {path} returned {status}: {reason}"))
}

/// How many shards failed and the first one's reason, when `_shards.failed` is above zero.
fn shard_failure(response: &Json) -> Option<String> {
    let failed = response.pointer("/_shards/failed").and_then(Json::as_u64).unwrap_or(0);
    if failed == 0 {
        return None;
    }
    Some(
        match response
            .pointer("/_shards/failures/0/reason/reason")
            .and_then(Json::as_str)
        {
            Some(reason) => format!("{failed} failed, {reason}"),
            None => format!("{failed} failed"),
        },
    )
}

fn source_of(source: Option<&Json>) -> Document {
    match source.map(moqui_lib::json::from_json) {
        Some(Value::Map(mut map)) => {
            map.shift_remove(INDEXED_AT);
            map.shift_remove("__cursor");
            map
        }
        _ => Document::new(),
    }
}

/// Percent-encode a document id for a URL path.
fn url_part(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
#[path = "opensearch_tests.rs"]
mod tests;
