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

//! Aggregation trees to SQL, one statement per bucket level.
//!
//! A level groups by the keys of every bucket aggregation above it plus its own. Metrics and
//! filter counts that sit in the level, with no bucket aggregation between, are columns of that
//! statement. A `Filter` adds `FILTER (WHERE ..)` to the columns below it, and becomes a `WHERE`
//! condition for any bucket level below it. A sum of a child-array number is a lateral per-row
//! sum, so the total sees every element, as Elastic's `include_in_root` does.
//!
//! Terms ordering, terms size, and date histogram gap buckets are done in Rust on the grouped rows.

use std::collections::HashMap;

use indexmap::IndexMap;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use moqui_lib::error::Error;
use moqui_lib::object::{compare_obj, to_plain_string, Value};

use super::compile::{literal, Compiler, Located, Sql, TextSetup};
use crate::driver::DbRow;
use crate::search::agg::{
    bucket_start, histogram_zone, key_display, next_bucket, zone_name, Agg, DateHistogram, Terms, TermsOrder,
};
use crate::search::agg_empty_results;
use crate::search::mapping::{DocumentMapping, FieldKind};
use crate::search::query::Query;

/// Runs one statement.
pub type Runner<'r> = dyn FnMut(&Sql) -> Result<Vec<DbRow>, Error> + 'r;

/// The most groups one level may return before Rust orders and trims them.
#[cfg(test)]
const MAX_GROUPS: usize = 65_536;

#[derive(Clone)]
struct KeyDef {
    select: String,
    lateral: Option<String>,
}

enum Item {
    HistogramMerge {
        path: Vec<String>,
        conds: Vec<Sql>,
        bounds: Vec<f64>,
    },
    Metric {
        path: Vec<String>,
        conds: Vec<Sql>,
        field: String,
        kind: crate::search::agg::Metric,
    },
    /// `count(*)` under the filters on the path, stored at `path` + `doc_count`.
    DocCount { path: Vec<String>, conds: Vec<Sql> },
    Sum {
        path: Vec<String>,
        conds: Vec<Sql>,
        field: String,
    },
    Extreme {
        path: Vec<String>,
        conds: Vec<Sql>,
        field: String,
        max: bool,
    },
    CountDistinct {
        path: Vec<String>,
        conds: Vec<Sql>,
        field: String,
    },
}

struct ChildLevel<'t> {
    path: Vec<String>,
    conds: Vec<Sql>,
    agg: &'t Agg,
}

type Row = (Vec<Value>, IndexMap<String, Value>);
/// A child level's rows by the parent key tuple: the own key and the bucket content.
type Grouped = HashMap<String, Vec<(Value, IndexMap<String, Value>)>>;

/// Run `aggs` over the documents matching `filter`.
///
/// # Errors
///
/// Returns an error when a field cannot be aggregated as asked, or a statement fails.
pub fn aggregate(
    table: &str,
    mapping: &DocumentMapping,
    text: &TextSetup,
    filter: &Query,
    aggs: &IndexMap<String, Agg>,
    run: &mut Runner<'_>,
) -> Result<IndexMap<String, Value>, Error> {
    aggregate_population(table, mapping, text, filter, aggs, None, run)
}

pub fn aggregate_population(
    table: &str,
    mapping: &DocumentMapping,
    text: &TextSetup,
    filter: &Query,
    aggs: &IndexMap<String, Agg>,
    documents: Option<&[std::sync::Arc<DocumentMapping>]>,
    run: &mut Runner<'_>,
) -> Result<IndexMap<String, Value>, Error> {
    let base = Compiler::with_text(mapping, text)
        .with_documents(documents)
        .where_sql(filter)?;
    let mut ctx = LevelCtx {
        table,
        mapping,
        text,
        base,
        documents,
        run,
    };
    let rows = run_level(&mut ctx, &[], &[], aggs)?;
    let mut root = rows
        .into_iter()
        .next()
        .map(|(_, content)| content)
        .unwrap_or_else(|| agg_empty_results(aggs));
    root.shift_remove("doc_count");
    Ok(root)
}

/// What every level of one aggregation shares: the table and mapping, the documents' filter, and the statement runner.
struct LevelCtx<'a, 'r> {
    table: &'a str,
    mapping: &'a DocumentMapping,
    text: &'a TextSetup,
    base: Sql,
    documents: Option<&'a [std::sync::Arc<DocumentMapping>]>,
    run: &'a mut Runner<'r>,
}

/// One bucket level: grouped by `keys` under `conds`, with the levels of the bucket aggregations in it below.
fn run_level(
    ctx: &mut LevelCtx<'_, '_>,
    keys: &[KeyDef],
    conds: &[Sql],
    aggs: &IndexMap<String, Agg>,
) -> Result<Vec<Row>, Error> {
    let compiler = Compiler::with_text(ctx.mapping, ctx.text).with_documents(ctx.documents);
    let mut items = Vec::new();
    let mut children = Vec::new();
    collect(&compiler, aggs, &[], &[], &mut items, &mut children)?;
    let sql = level_sql(ctx, &compiler, keys, conds, &items)?;
    let rows = (ctx.run)(&sql)?;
    let cap = crate::search::limits::buckets();
    if rows.len() > cap {
        return Err(Error::new(format!(
            "an aggregation level returned more than {cap} groups"
        )));
    }
    let mut out = read_rows(&rows, keys.len(), &items, aggs)?;
    for child in &children {
        attach_children(ctx, &compiler, keys, conds, child, &mut out)?;
    }
    Ok(out)
}

/// The statement of a level: its keys, `count(*)`, and a column for each metric and filter count in it.
fn level_sql(
    ctx: &LevelCtx<'_, '_>,
    compiler: &Compiler<'_>,
    keys: &[KeyDef],
    conds: &[Sql],
    items: &[Item],
) -> Result<Sql, Error> {
    let mut sql = Sql::new("SELECT ");
    for (index, key) in keys.iter().enumerate() {
        sql.push(&format!("{} AS k{index}, ", key.select));
    }
    sql.push("count(*) AS dc");
    let mut laterals = Vec::new();
    for (index, item) in items.iter().enumerate() {
        sql.push(", ");
        match item {
            Item::HistogramMerge { conds, bounds, .. } => {
                let bounds_json = serde_json::to_string(bounds).map_err(|e| Error::new(e.to_string()))?;
                sql.push(&format!("jsonb_build_object('bounds', {}::jsonb, 'compatible', bool_and(body -> 'bounds' = {}::jsonb AND jsonb_array_length(body -> 'buckets') = {})",literal(&bounds_json),literal(&bounds_json),bounds.len()+1));
                filter_clause(&mut sql, conds);
                sql.push(", 'buckets', jsonb_build_array(");
                for index in 0..=bounds.len() {
                    if index > 0 {
                        sql.push(", ");
                    }
                    sql.push(&format!("COALESCE(sum((body -> 'buckets' ->> {index})::bigint)"));
                    filter_clause(&mut sql, conds);
                    sql.push(", 0)");
                }
                sql.push("))::text");
            }
            Item::Metric { conds, field, kind, .. } => {
                use crate::search::agg::Metric;
                let (arrays, value, field_kind) = match compiler.locate_root(field) {
                    Located::Missing => (Vec::new(), "NULL::numeric".into(), FieldKind::Double),
                    Located::Field { arrays, expr, field } => (arrays, expr, field.kind),
                };
                if !arrays.is_empty() {
                    return Err(Error::new(format!(
                        "report metric {field} on child arrays is not supported"
                    )));
                }
                if !matches!(kind, Metric::ValueCount) && !matches!(field_kind, FieldKind::Long | FieldKind::Double) {
                    return Err(Error::new(format!("report metric needs numeric field {field}")));
                }
                match kind {
                    Metric::Avg => sql.push(&format!("avg({value})")),
                    Metric::ValueCount => sql.push(&format!("count({value})")),
                    Metric::Percentile(percent) => sql.push(&format!(
                        "percentile_cont({}) WITHIN GROUP (ORDER BY {value})",
                        percent / 100.0
                    )),
                };
                filter_clause(&mut sql, conds);
            }
            Item::DocCount { conds, .. } => {
                sql.push("count(*)");
                filter_clause(&mut sql, conds);
            }
            Item::Sum { conds, field, .. } => {
                let value = number_value(compiler, field, "sum", &format!("s{index}"), &mut laterals)?;
                sql.push(&format!("COALESCE(sum({value})"));
                filter_clause(&mut sql, conds);
                sql.push(", 0)");
            }
            Item::Extreme { conds, field, max, .. } => {
                let name = if *max { "max" } else { "min" };
                let value = number_value(compiler, field, name, &format!("s{index}"), &mut laterals)?;
                sql.push(&format!("{name}({value})"));
                filter_clause(&mut sql, conds);
            }
            Item::CountDistinct { conds, field, .. } => {
                let value = match compiler.locate_root(field) {
                    Located::Missing => "NULL".to_string(),
                    Located::Field { arrays, expr, .. } => {
                        if !arrays.is_empty() {
                            return Err(Error::new(format!(
                                "count_distinct of the child field {field} is not supported"
                            )));
                        }
                        expr
                    }
                };
                sql.push(&format!("count(DISTINCT {value})"));
                filter_clause(&mut sql, conds);
            }
        }
        sql.push(&format!(" AS c{index}"));
    }
    sql.push(&format!(" FROM {}", ctx.table));
    for key in keys {
        if let Some(lateral) = &key.lateral {
            sql.push(" ");
            sql.push(lateral);
        }
    }
    for lateral in &laterals {
        sql.push(" ");
        sql.push(lateral);
    }
    sql.push(" WHERE ");
    sql.append(ctx.base.clone());
    for cond in conds {
        sql.push(" AND ");
        sql.append(cond.clone());
    }
    for key in keys {
        sql.push(&format!(" AND {} IS NOT NULL", key.select));
    }
    if !keys.is_empty() {
        let ordinals: Vec<String> = (1..=keys.len()).map(|index| index.to_string()).collect();
        sql.push(&format!(" GROUP BY {}", ordinals.join(", ")));
        // One more than the cap is enough to tell it was exceeded, without reading every group.
        sql.push(&format!(" LIMIT {}", crate::search::limits::buckets() + 1));
    }
    Ok(sql)
}

/// The SQL of a number field for the aggregate function `func`: the column, or for a field of child objects the
/// value of a lateral that applies `func` over the object's array, added to `laterals`.
fn number_value(
    compiler: &Compiler<'_>,
    field: &str,
    func: &str,
    alias: &str,
    laterals: &mut Vec<String>,
) -> Result<String, Error> {
    Ok(match compiler.locate_root(field) {
        Located::Missing => "NULL::numeric".to_string(),
        Located::Field {
            arrays,
            expr,
            field: found,
        } => {
            if !matches!(found.kind, FieldKind::Long | FieldKind::Double)
                && !(matches!(func, "min" | "max") && found.kind == FieldKind::Date)
            {
                return Err(Error::new(format!("{func} needs a number field, {field} is not")));
            }
            if arrays.is_empty() {
                expr
            } else {
                laterals.push(format!(
                    "LEFT JOIN LATERAL (SELECT {func}({expr}) AS v FROM {}) AS {alias} ON TRUE",
                    from_list(&arrays)
                ));
                format!("{alias}.v")
            }
        }
    })
}

/// Each row of a level as its key tuple and its content: `doc_count`, the empty results of its aggregations, and the
/// metric and filter count columns at their paths.
fn read_rows(
    rows: &[DbRow],
    key_count: usize,
    items: &[Item],
    aggs: &IndexMap<String, Agg>,
) -> Result<Vec<Row>, Error> {
    let mut out: Vec<Row> = Vec::with_capacity(rows.len());
    for row in rows {
        let tuple: Vec<Value> = (0..key_count)
            .map(|index| row.get(&format!("k{index}")).cloned().unwrap_or(Value::Null))
            .collect();
        let mut content = IndexMap::new();
        content.insert("doc_count".to_string(), count_value(row.get("dc")));
        for (name, value) in agg_empty_results(aggs) {
            content.insert(name, value);
        }
        for (index, item) in items.iter().enumerate() {
            let cell = row.get(&format!("c{index}")).cloned().unwrap_or(Value::Null);
            match item {
                Item::HistogramMerge { path, .. } => {
                    let mut value = match &cell {
                        Value::String(s) => {
                            serde_json::from_str::<serde_json::Value>(s).map_err(|e| Error::new(e.to_string()))?
                        }
                        _ => moqui_lib::json::to_json(&cell),
                    };
                    if value["compatible"] == false {
                        return Err(Error::new("cannot merge metric histogram boundaries"));
                    }
                    if let Some(map) = value.as_object_mut() {
                        map.remove("compatible");
                    }
                    set_path(&mut content, path, moqui_lib::json::from_json(&value));
                }
                Item::Metric { path, kind, .. } => set_path(
                    &mut content,
                    path,
                    if matches!(kind, crate::search::agg::Metric::ValueCount) {
                        count_value(Some(&cell))
                    } else {
                        extreme_value(cell)
                    },
                ),
                Item::DocCount { path, .. } => {
                    let mut full = path.clone();
                    full.push("doc_count".to_string());
                    set_path(&mut content, &full, count_value(Some(&cell)));
                }
                Item::Sum { path, .. } => set_path(&mut content, path, sum_value(cell)),
                Item::Extreme { path, .. } => set_path(&mut content, path, extreme_value(cell)),
                Item::CountDistinct { path, .. } => set_path(&mut content, path, count_value(Some(&cell))),
            }
        }
        out.push((tuple, content));
    }
    Ok(out)
}

/// Run a bucket aggregation's own level, grouped by the parent keys and its key, and put its buckets in each parent row.
fn attach_children(
    ctx: &mut LevelCtx<'_, '_>,
    compiler: &Compiler<'_>,
    keys: &[KeyDef],
    conds: &[Sql],
    child: &ChildLevel<'_>,
    out: &mut [Row],
) -> Result<(), Error> {
    if let Agg::TopHits { size, sort } = child.agg {
        for (tuple, content) in out {
            let identity = if ctx.documents.is_some() {
                "__document".into()
            } else {
                literal(&ctx.mapping.document_id)
            };
            let mut sql = Sql::new(format!(
                "SELECT doc_id, {identity} AS document_id, body::text AS body FROM {}",
                ctx.table
            ));
            for key in keys {
                if let Some(lateral) = &key.lateral {
                    sql.push(" ");
                    sql.push(lateral);
                }
            }
            sql.push(" WHERE ");
            sql.append(ctx.base.clone());
            for condition in conds.iter().chain(&child.conds) {
                sql.push(" AND ");
                sql.append(condition.clone());
            }
            for (key, value) in keys.iter().zip(tuple) {
                sql.push(&format!(" AND {} IS NOT DISTINCT FROM ", key.select));
                sql.param(value.clone());
            }
            let mut order = Vec::new();
            for sort in sort {
                let expr = match compiler.locate_root(&sort.field) {
                    Located::Field { expr, arrays, .. } if arrays.is_empty() => expr,
                    Located::Missing => "NULL".into(),
                    _ => return Err(Error::new("top_hits can sort only root fields")),
                };
                order.push(format!(
                    "{expr} {} NULLS LAST",
                    if sort.descending { "DESC" } else { "ASC" }
                ));
            }
            order.push("doc_id ASC".into());
            if ctx.documents.is_some() {
                order.push("__document ASC".into());
            }
            sql.push(&format!(" ORDER BY {} LIMIT {size}", order.join(", ")));
            let rows = (ctx.run)(&sql)?;
            let mut samples = Vec::new();
            for row in rows {
                let body = row
                    .get("body")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .map(|v| moqui_lib::json::from_json(&v))
                    .unwrap_or(Value::Null);
                samples.push(Value::Map(IndexMap::from([
                    ("id".into(), row.get("doc_id").cloned().unwrap_or(Value::Null)),
                    ("body".into(), body),
                    (
                        "documentId".into(),
                        row.get("document_id").cloned().unwrap_or(Value::Null),
                    ),
                ])));
            }
            set_path(content, &child.path, Value::List(samples));
        }
        return Ok(());
    }
    if let Agg::NamedFilters { filters, aggs } = child.agg {
        let mut buckets: HashMap<String, Vec<Value>> = HashMap::new();
        for (name, filter) in filters {
            let mut conditions = conds.to_vec();
            conditions.extend(child.conds.iter().cloned());
            conditions.push(compiler.where_sql(filter)?);
            let rows = run_level(ctx, keys, &conditions, aggs)?;
            let mut found: HashMap<String, IndexMap<String, Value>> = rows
                .into_iter()
                .map(|(tuple, content)| (tuple_key(&tuple), content))
                .collect();
            for (parent, _) in out.iter() {
                let key = tuple_key(parent);
                let mut content = found.remove(&key).unwrap_or_else(|| {
                    let mut c = agg_empty_results(aggs);
                    c.insert("doc_count".into(), Value::I64(0));
                    c
                });
                content.insert("key".into(), Value::string(name));
                buckets.entry(key).or_default().push(Value::Map(content));
            }
        }
        for (parent, content) in out {
            let mut holder = IndexMap::new();
            holder.insert(
                "buckets".into(),
                Value::List(buckets.remove(&tuple_key(parent)).unwrap_or_default()),
            );
            set_path(content, &child.path, Value::Map(holder));
        }
        return Ok(());
    }
    let (key, sub_aggs) = match child.agg {
        Agg::DateHistogram(histogram) => (histogram_key(compiler, histogram, keys.len())?, &histogram.aggs),
        Agg::Terms(terms) => (terms_key(compiler, terms, keys.len())?, &terms.aggs),
        Agg::Histogram {
            field,
            interval,
            offset,
            aggs,
        } => {
            let (expr, lateral) = match compiler.locate_root(field) {
                Located::Missing => ("NULL::numeric".into(), None),
                Located::Field {
                    expr,
                    arrays,
                    field: found,
                } => {
                    if !arrays.is_empty() || !matches!(found.kind, FieldKind::Long | FieldKind::Double) {
                        return Err(Error::new("histogram requires a numeric root field"));
                    }
                    (expr, None)
                }
            };
            (
                KeyDef {
                    select: format!("floor(({expr} - {offset}) / {interval}) * {interval} + {offset}"),
                    lateral,
                },
                aggs,
            )
        }
        _ => return Ok(()),
    };
    let mut child_keys: Vec<KeyDef> = keys.to_vec();
    child_keys.push(key);
    let mut child_conds: Vec<Sql> = conds.to_vec();
    child_conds.extend(child.conds.iter().cloned());
    let child_rows = run_level(ctx, &child_keys, &child_conds, sub_aggs)?;
    let mut grouped: Grouped = HashMap::new();
    for (tuple, content) in child_rows {
        let (own, parent) = tuple
            .split_last()
            .map(|(own, parent)| (own.clone(), parent.to_vec()))
            .unwrap_or((Value::Null, Vec::new()));
        grouped.entry(tuple_key(&parent)).or_default().push((own, content));
    }
    for (parent_tuple, parent_content) in out.iter_mut() {
        let rows = grouped.remove(&tuple_key(parent_tuple)).unwrap_or_default();
        let buckets = match child.agg {
            Agg::DateHistogram(histogram) => histogram_buckets(histogram, rows)?,
            Agg::Terms(terms) => terms_buckets(terms, rows),
            Agg::Histogram { .. } => {
                let mut rows = rows;
                rows.sort_by(|(a, _), (b, _)| compare_obj(a, b).unwrap_or(std::cmp::Ordering::Equal));
                rows.into_iter()
                    .map(|(key, mut content)| {
                        content.insert("key".into(), key);
                        Value::Map(content)
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        let mut holder = IndexMap::new();
        holder.insert("buckets".to_string(), Value::List(buckets));
        set_path(parent_content, &child.path, Value::Map(holder));
    }
    Ok(())
}

fn collect<'t>(
    compiler: &Compiler<'_>,
    aggs: &'t IndexMap<String, Agg>,
    path: &[String],
    conds: &[Sql],
    items: &mut Vec<Item>,
    children: &mut Vec<ChildLevel<'t>>,
) -> Result<(), Error> {
    for (name, agg) in aggs {
        let mut here = path.to_vec();
        here.push(name.clone());
        match agg {
            Agg::HistogramMerge { bounds } => items.push(Item::HistogramMerge {
                path: here,
                conds: conds.to_vec(),
                bounds: bounds.clone(),
            }),
            Agg::Metric { field, kind } => items.push(Item::Metric {
                path: here,
                conds: conds.to_vec(),
                field: field.clone(),
                kind: kind.clone(),
            }),
            Agg::Sum { field } => items.push(Item::Sum {
                path: here,
                conds: conds.to_vec(),
                field: field.clone(),
            }),
            Agg::Extreme { field, max } => items.push(Item::Extreme {
                path: here,
                conds: conds.to_vec(),
                field: field.clone(),
                max: *max,
            }),
            Agg::CountDistinct { field } => items.push(Item::CountDistinct {
                path: here,
                conds: conds.to_vec(),
                field: field.clone(),
            }),
            Agg::Filter { query, aggs } => {
                let mut inner = conds.to_vec();
                inner.push(compiler.where_sql(query)?);
                items.push(Item::DocCount {
                    path: here.clone(),
                    conds: inner.clone(),
                });
                collect(compiler, aggs, &here, &inner, items, children)?;
            }
            Agg::DateHistogram(_)
            | Agg::Terms(_)
            | Agg::Histogram { .. }
            | Agg::NamedFilters { .. }
            | Agg::TopHits { .. } => children.push(ChildLevel {
                path: here,
                conds: conds.to_vec(),
                agg,
            }),
        }
    }
    Ok(())
}

fn filter_clause(sql: &mut Sql, conds: &[Sql]) {
    if conds.is_empty() {
        return;
    }
    sql.push(" FILTER (WHERE ");
    for (index, cond) in conds.iter().enumerate() {
        if index > 0 {
            sql.push(" AND ");
        }
        sql.append(cond.clone());
    }
    sql.push(")");
}

fn from_list(arrays: &[(String, String)]) -> String {
    arrays
        .iter()
        .map(|(var, source)| format!("{source} AS {var}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The zone for `AT TIME ZONE`: its IANA name, or an interval for a fixed offset. Postgres reads a
/// text offset such as `'+05:00'` as a POSIX zone, east of Greenwich negative, so an offset is
/// never sent as text.
fn zone_sql(zone: &TimeZone) -> Result<String, Error> {
    if let Some(name) = zone.iana_name() {
        if name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '+' | '-'))
        {
            return Ok(literal(name));
        }
        return Err(Error::new(format!("time zone {name} cannot be used in a histogram")));
    }
    let offset = zone
        .to_fixed_offset()
        .map_err(|_| Error::new(format!("time zone {} has no name or fixed offset", zone_name(zone))))?;
    Ok(format!("interval '{} seconds'", offset.seconds()))
}

fn histogram_key(compiler: &Compiler<'_>, histogram: &DateHistogram, depth: usize) -> Result<KeyDef, Error> {
    let zone = histogram_zone(histogram.time_zone.as_deref())?;
    let zone_text = zone_sql(&zone)?;
    let offset = format!("interval '{} milliseconds'", histogram.offset.as_millis());
    let (value, lateral) = date_value(compiler, &histogram.field, depth)?;
    let select = if let Some(millis) = histogram.fixed_millis {
        format!(
            "to_timestamp((floor((extract(epoch FROM {value}) * 1000 - {}) / {millis}) * {millis} + {}) / 1000)",
            histogram.offset.as_millis(),
            histogram.offset.as_millis()
        )
    } else {
        format!(
            "((date_trunc('{}', ({value} AT TIME ZONE {zone_text}) - {offset}) + {offset}) AT TIME ZONE {zone_text})",
            histogram.unit.trunc_field()
        )
    };
    Ok(KeyDef { select, lateral })
}

fn date_value(compiler: &Compiler<'_>, field: &str, depth: usize) -> Result<(String, Option<String>), Error> {
    match compiler.locate_root(field) {
        Located::Missing => Ok(("NULL::timestamptz".to_string(), None)),
        Located::Field {
            arrays,
            expr,
            field: found,
        } => {
            if found.kind != FieldKind::Date {
                return Err(Error::new(format!(
                    "a date histogram needs a date field, {field} is not"
                )));
            }
            if arrays.is_empty() {
                Ok((expr, None))
            } else {
                let alias = format!("k{depth}d");
                Ok((
                    format!("{alias}.v"),
                    Some(format!(
                        "CROSS JOIN LATERAL (SELECT DISTINCT {expr} AS v FROM {}) AS {alias}",
                        from_list(&arrays)
                    )),
                ))
            }
        }
    }
}

fn terms_key(compiler: &Compiler<'_>, terms: &Terms, depth: usize) -> Result<KeyDef, Error> {
    match compiler.locate_root(&terms.field) {
        Located::Missing => Ok(KeyDef {
            select: "NULL::text".into(),
            lateral: None,
        }),
        Located::Field { arrays, expr, field } => {
            if field.kind == FieldKind::Text && !field.sortable {
                return Err(Error::new(format!(
                    "terms on the text field {} needs sortable=Y",
                    terms.field
                )));
            }
            if arrays.is_empty() {
                Ok(KeyDef {
                    select: expr,
                    lateral: None,
                })
            } else {
                let alias = format!("k{depth}t");
                Ok(KeyDef {
                    select: format!("{alias}.v"),
                    lateral: Some(format!(
                        "CROSS JOIN LATERAL (SELECT DISTINCT {expr} AS v FROM {}) AS {alias}",
                        from_list(&arrays)
                    )),
                })
            }
        }
    }
}

fn histogram_buckets(
    histogram: &DateHistogram,
    rows: Vec<(Value, IndexMap<String, Value>)>,
) -> Result<Vec<Value>, Error> {
    let zone = histogram_zone(histogram.time_zone.as_deref())?;
    let mut by_key: Vec<(Timestamp, IndexMap<String, Value>)> = Vec::with_capacity(rows.len());
    for (key, content) in rows {
        let Some(at) = timestamp_of(&key) else {
            continue;
        };
        by_key.push((at, content));
    }
    by_key.sort_by_key(|(at, _)| *at);
    if histogram.min_doc_count == 0 && (by_key.len() > 1 || histogram.extended_bounds.is_some()) {
        let first = histogram
            .extended_bounds
            .map(|b| b.0)
            .or_else(|| by_key.first().map(|b| b.0))
            .unwrap();
        let last = histogram
            .extended_bounds
            .map(|b| b.1)
            .or_else(|| by_key.last().map(|b| b.0))
            .unwrap();
        let mut filled = Vec::new();
        let mut existing = by_key.into_iter().peekable();
        // Normalize, in case the database bucket and the Rust bucket disagree at a zone edge.
        let fixed_start = |time: Timestamp, millis: i64| {
            Timestamp::from_millisecond(
                (time.as_millisecond() - histogram.offset.as_millis() as i64).div_euclid(millis) * millis
                    + histogram.offset.as_millis() as i64,
            )
            .map_err(|e| Error::new(e.to_string()))
        };
        let mut at = if let Some(millis) = histogram.fixed_millis {
            fixed_start(first, millis)?
        } else {
            bucket_start(first, histogram.unit, &zone, histogram.offset)?
        };
        let mut guard = 0usize;
        while at <= last {
            guard += 1;
            if guard > crate::search::limits::buckets() {
                return Err(Error::new("a date histogram would have too many buckets"));
            }
            match existing.peek() {
                Some((key, _)) if *key == at => {
                    if let Some(entry) = existing.next() {
                        filled.push(entry);
                    }
                }
                Some((key, _)) if *key < at => {
                    if let Some(entry) = existing.next() {
                        filled.push(entry);
                    }
                    continue;
                }
                _ => {
                    let mut empty = IndexMap::new();
                    empty.insert("doc_count".to_string(), Value::I64(0));
                    for (name, value) in agg_empty_results(&histogram.aggs) {
                        empty.insert(name, value);
                    }
                    filled.push((at, empty));
                }
            }
            at = if let Some(millis) = histogram.fixed_millis {
                Timestamp::from_millisecond(
                    at.as_millisecond()
                        .checked_add(millis)
                        .ok_or_else(|| Error::new("histogram time overflow"))?,
                )
                .map_err(|e| Error::new(e.to_string()))?
            } else {
                next_bucket(at, histogram.unit, &zone, histogram.offset)?
            };
        }
        filled.extend(existing);
        by_key = filled;
    }
    if histogram.min_doc_count > 1 {
        by_key.retain(|(_, content)| doc_count(content) >= histogram.min_doc_count);
    }
    if histogram.descending {
        by_key.reverse();
    }
    let mut out = Vec::with_capacity(by_key.len());
    for (at, content) in by_key {
        let partial = crate::search::agg::partial_bucket(histogram, at)?;
        if partial && histogram.drop_partial {
            continue;
        }
        let mut bucket = IndexMap::new();
        if partial {
            bucket.insert("partial".into(), Value::Bool(true));
        }
        bucket.insert("key".to_string(), Value::Timestamp(at));
        bucket.insert(
            "key_display".to_string(),
            Value::String(key_display(at, &zone, histogram.format.as_deref())?),
        );
        for (name, value) in content {
            bucket.insert(name, value);
        }
        out.push(Value::Map(bucket));
    }
    Ok(out)
}

fn terms_buckets(terms: &Terms, rows: Vec<(Value, IndexMap<String, Value>)>) -> Vec<Value> {
    let mut rows = rows;
    rows.retain(|(_, content)| doc_count(content) >= terms.min_doc_count);
    let key_order =
        |a: &Value, b: &Value| compare_obj(a, b).unwrap_or_else(|_| to_plain_string(a).cmp(&to_plain_string(b)));
    rows.sort_by(|(key_a, a), (key_b, b)| {
        let primary = match &terms.order {
            TermsOrder::Count { descending } => {
                let order = doc_count(a).cmp(&doc_count(b));
                if *descending {
                    order.reverse()
                } else {
                    order
                }
            }
            TermsOrder::Key { descending } => {
                let order = key_order(key_a, key_b);
                if *descending {
                    order.reverse()
                } else {
                    order
                }
            }
            TermsOrder::Metric { name, descending } => {
                let left = a.get(name).cloned().unwrap_or(Value::Null);
                let right = b.get(name).cloned().unwrap_or(Value::Null);
                let order = compare_obj(&left, &right).unwrap_or(std::cmp::Ordering::Equal);
                if *descending {
                    order.reverse()
                } else {
                    order
                }
            }
        };
        primary.then_with(|| key_order(key_a, key_b))
    });
    rows.truncate(terms.size);
    rows.into_iter()
        .map(|(key, content)| {
            let mut bucket = IndexMap::new();
            bucket.insert("key".to_string(), key);
            for (name, value) in content {
                bucket.insert(name, value);
            }
            Value::Map(bucket)
        })
        .collect()
}

fn doc_count(content: &IndexMap<String, Value>) -> u64 {
    match content.get("doc_count") {
        Some(Value::I64(count)) => u64::try_from(*count).unwrap_or(0),
        Some(Value::I32(count)) => u64::try_from(*count).unwrap_or(0),
        _ => 0,
    }
}

fn count_value(value: Option<&Value>) -> Value {
    match value {
        Some(Value::I64(count)) => Value::I64(*count),
        Some(Value::I32(count)) => Value::I64(i64::from(*count)),
        Some(other) if !other.is_null() => to_plain_string(other).parse::<i64>().map_or(Value::I64(0), Value::I64),
        _ => Value::I64(0),
    }
}

fn sum_value(value: Value) -> Value {
    match value {
        Value::Decimal(_) => value,
        Value::Null => Value::Decimal(Default::default()),
        other => match moqui_lib::object::basic_convert(&other, "BigDecimal").value {
            decimal @ Value::Decimal(_) => decimal,
            _ => Value::Decimal(Default::default()),
        },
    }
}

/// A minimum or maximum as a decimal, null when there was no value.
fn extreme_value(value: Value) -> Value {
    match value {
        Value::Null | Value::Decimal(_) | Value::Timestamp(_) => value,
        other => match moqui_lib::object::basic_convert(&other, "BigDecimal").value {
            decimal @ Value::Decimal(_) => decimal,
            _ => Value::Null,
        },
    }
}

fn timestamp_of(value: &Value) -> Option<Timestamp> {
    match value {
        Value::Timestamp(at) => Some(*at),
        other => super::compile::parse_date_bound(other),
    }
}

fn tuple_key(values: &[Value]) -> String {
    values.iter().map(to_plain_string).collect::<Vec<_>>().join("\u{1}")
}

fn set_path(map: &mut IndexMap<String, Value>, path: &[String], value: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut current = map;
    for name in parents {
        let entry = current
            .entry(name.clone())
            .or_insert_with(|| Value::Map(IndexMap::new()));
        if !matches!(entry, Value::Map(_)) {
            *entry = Value::Map(IndexMap::new());
        }
        let Value::Map(next) = entry else {
            return;
        };
        current = next;
    }
    current.insert(last.clone(), value);
}

#[cfg(test)]
#[path = "agg_tests.rs"]
mod tests;
