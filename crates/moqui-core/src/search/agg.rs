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

//! The aggregation tree, its map form, and calendar bucket arithmetic.
//!
//! ```text
//! {filter:         {query: <query>, aggs: {..}}}
//! {date_histogram: {field, unit: "day"|"week"|"month"|"quarter"|"year" (or d w M q y, 1w),
//!                   time_zone, format: "dd MMM yyyy", offset: "+1d", order: "asc"|"desc",
//!                   min_doc_count: 0, aggs: {..}}}
//! {terms:          {field, size: 10, order: {totalSales: "desc"} | {_count: "desc"} | {_key: "asc"},
//!                   aggs: {..}}}
//! {sum:            {field}}
//! {min:            {field}}        also max; null over nothing
//! {count_distinct: {field}}        also "cardinality"
//! ```
//!
//! Results are maps keyed by aggregation name:
//!
//! - a bucket aggregation is `{buckets: [{key, key_display?, doc_count, <sub>..}]}`;
//! - a filter is `{doc_count, <sub>..}`;
//! - a metric is a number. A sum over nothing is 0, and a distinct count is an exact integer.
//!
//! A date histogram with `min_doc_count` 0 has a bucket for every period between the first and
//! last non-empty period, as Elastic does. It does not reach out to the filter's date range.

use indexmap::IndexMap;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp, ToSpan, Zoned};
use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};

use super::query::Query;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarUnit {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl CalendarUnit {
    /// `day`, `d`, `1d`, and the like. `M` is month and `m` is minute, as Elastic has them.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let trimmed = text.trim().trim_start_matches('1');
        Ok(match trimmed {
            "m" | "minute" => Self::Minute,
            "h" | "hour" => Self::Hour,
            "d" | "day" => Self::Day,
            "w" | "week" => Self::Week,
            "M" | "month" => Self::Month,
            "q" | "quarter" => Self::Quarter,
            "y" | "year" => Self::Year,
            _ => return Err(Error::new(format!("calendar interval {text} is not known"))),
        })
    }

    /// The Postgres `date_trunc` field.
    pub fn trunc_field(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Quarter => "quarter",
            Self::Year => "year",
        }
    }

    /// The Elastic `calendar_interval`.
    pub fn calendar_interval(self) -> &'static str {
        match self {
            Self::Minute => "1m",
            Self::Hour => "1h",
            Self::Day => "1d",
            Self::Week => "1w",
            Self::Month => "1M",
            Self::Quarter => "1q",
            Self::Year => "1y",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DateHistogram {
    pub field: String,
    pub unit: CalendarUnit,
    /// A fixed epoch-aligned interval, in milliseconds; calendar unit is unused when set.
    pub fixed_millis: Option<i64>,
    pub extended_bounds: Option<(Timestamp, Timestamp)>,
    pub range_start: Option<Timestamp>,
    pub range_end: Option<Timestamp>,
    pub drop_partial: bool,
    /// IANA name. `None` is the server zone.
    pub time_zone: Option<String>,
    /// Java date pattern for `key_display`.
    pub format: Option<String>,
    /// Bucket boundaries shift by this much, `+6d` or `-1d`.
    pub offset: SignedDuration,
    /// The text the offset came from, to send on to Elastic as it was given.
    pub offset_text: Option<String>,
    pub descending: bool,
    pub min_doc_count: u64,
    pub aggs: IndexMap<String, Agg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermsOrder {
    Count { descending: bool },
    Key { descending: bool },
    Metric { name: String, descending: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Terms {
    pub field: String,
    pub size: usize,
    pub order: TermsOrder,
    pub aggs: IndexMap<String, Agg>,
    pub missing: Option<String>,
    pub other: Option<String>,
    pub min_doc_count: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Metric {
    Avg,
    ValueCount,
    Percentile(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Agg {
    TopHits {
        size: usize,
        sort: Vec<super::SortField>,
    },
    /// Sum corresponding histogram buckets after checking the boundary contract on every point.
    HistogramMerge {
        bounds: Vec<f64>,
    },
    Metric {
        field: String,
        kind: Metric,
    },
    Histogram {
        field: String,
        interval: f64,
        offset: f64,
        aggs: IndexMap<String, Agg>,
    },
    NamedFilters {
        filters: IndexMap<String, Query>,
        aggs: IndexMap<String, Agg>,
    },
    Filter {
        query: Query,
        aggs: IndexMap<String, Agg>,
    },
    DateHistogram(DateHistogram),
    Terms(Terms),
    Sum {
        field: String,
    },
    /// The least (or, with `max`, the greatest) value of a number field, null over nothing.
    Extreme {
        field: String,
        max: bool,
    },
    CountDistinct {
        field: String,
    },
}

impl Agg {
    pub fn sub_aggs(&self) -> Option<&IndexMap<String, Agg>> {
        match self {
            Self::Filter { aggs, .. } => Some(aggs),
            Self::Histogram { aggs, .. } | Self::NamedFilters { aggs, .. } => Some(aggs),
            Self::DateHistogram(histogram) => Some(&histogram.aggs),
            Self::Terms(terms) => Some(&terms.aggs),
            Self::Sum { .. }
            | Self::Extreme { .. }
            | Self::CountDistinct { .. }
            | Self::Metric { .. }
            | Self::HistogramMerge { .. }
            | Self::TopHits { .. } => None,
        }
    }

    /// Read named aggregations in the map form.
    ///
    /// # Errors
    ///
    /// Returns an error naming the aggregation that is not understood.
    pub fn map_from_value(value: &Value) -> Result<IndexMap<String, Agg>, Error> {
        let mut out = IndexMap::new();
        match value {
            Value::Null => {}
            Value::Map(map) => {
                for (name, body) in map {
                    let agg = Self::from_value(body).map_err(|err| Error::new(format!("aggregation {name}: {err}")))?;
                    out.insert(name.clone(), agg);
                }
            }
            _ => return Err(Error::new("aggregations are a map of name to aggregation")),
        }
        Ok(out)
    }

    /// Read one aggregation in the map form.
    ///
    /// # Errors
    ///
    /// Returns an error when the map is not one of the aggregation types.
    pub fn from_value(value: &Value) -> Result<Self, Error> {
        let Value::Map(map) = value else {
            return Err(Error::new("an aggregation is a map"));
        };
        let mut kind = None;
        let mut sub = None;
        for (key, body) in map {
            match key.as_str() {
                "aggs" | "aggregations" => sub = Some(body),
                _ if kind.is_none() => kind = Some((key.as_str(), body)),
                _ => return Err(Error::new(format!("an aggregation has one type, found {key} too"))),
            }
        }
        let (kind, body) = kind.ok_or_else(|| Error::new("an aggregation needs a type"))?;
        let Value::Map(body) = body else {
            return Err(Error::new(format!("{kind} needs a map")));
        };
        let nested = || -> Result<IndexMap<String, Agg>, Error> {
            match sub.or_else(|| body.get("aggs")) {
                Some(value) => Self::map_from_value(value),
                None => Ok(IndexMap::new()),
            }
        };
        let field = || -> Result<String, Error> {
            body.get("field")
                .map(to_plain_string)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| Error::new(format!("{kind} needs a field")))
        };
        Ok(match kind {
            "top_hits" => {
                let size = body
                    .get("size")
                    .map(to_plain_string)
                    .map(|s| s.parse::<usize>())
                    .transpose()
                    .map_err(|_| Error::new("top_hits size must be an integer"))?
                    .unwrap_or(3);
                if size > 100 {
                    return Err(Error::new("top_hits supports at most 100 samples"));
                }
                let sort = match body.get("sort") {
                    Some(Value::List(items)) => items
                        .iter()
                        .map(to_plain_string)
                        .filter_map(|s| super::SortField::parse(&s))
                        .collect(),
                    None => Vec::new(),
                    _ => return Err(Error::new("top_hits sort must be a list of field names")),
                };
                Self::TopHits { size, sort }
            }
            "histogram_merge" => {
                let Some(Value::List(values)) = body.get("bounds") else {
                    return Err(Error::new("histogram_merge requires explicit bounds"));
                };
                if values.len() > 256 {
                    return Err(Error::new("histogram_merge supports at most 256 bounds"));
                }
                let bounds = values
                    .iter()
                    .map(|v| {
                        to_plain_string(v)
                            .parse::<f64>()
                            .ok()
                            .filter(|v| v.is_finite())
                            .ok_or_else(|| Error::new("histogram bounds must be finite numbers"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if bounds.windows(2).any(|v| v[0] >= v[1]) {
                    return Err(Error::new("histogram bounds must increase"));
                }
                Self::HistogramMerge { bounds }
            }
            "filter" => {
                // Both `{filter: {query: q}}` and Elastic's `{filter: q}` read here.
                let query = match body.get("query") {
                    Some(query) => Query::from_value(query)?,
                    None => super::es_json::from_es(&Value::Map(body.clone()))?,
                };
                Self::Filter { query, aggs: nested()? }
            }
            "date_histogram" => {
                let fixed_millis = body
                    .get("fixed_interval")
                    .map(|v| fixed_interval(&to_plain_string(v)))
                    .transpose()?;
                let unit_text = body
                    .get("unit")
                    .or_else(|| body.get("calendar_interval"))
                    .or_else(|| body.get("interval"))
                    .map(to_plain_string)
                    .unwrap_or_else(|| {
                        if fixed_millis.is_some() {
                            "minute".into()
                        } else {
                            String::new()
                        }
                    });
                let extended_bounds = body
                    .get("extended_bounds")
                    .map(|v| {
                        let Value::Map(bounds) = v else {
                            return Err(Error::new("extended_bounds requires min and max"));
                        };
                        let parse = |key: &str| {
                            bounds
                                .get(key)
                                .and_then(|v| match v {
                                    Value::Timestamp(t) => Some(*t),
                                    _ => to_plain_string(v).parse::<Timestamp>().ok().or_else(|| {
                                        to_plain_string(v)
                                            .parse::<i64>()
                                            .ok()
                                            .and_then(|n| Timestamp::from_millisecond(n).ok())
                                    }),
                                })
                                .ok_or_else(|| Error::new(format!("invalid extended_bounds {key}")))
                        };
                        Ok((parse("min")?, parse("max")?))
                    })
                    .transpose()?;
                let offset_text = body.get("offset").map(to_plain_string).filter(|text| !text.is_empty());
                let offset = match &offset_text {
                    Some(text) => parse_offset(text)?,
                    None => SignedDuration::ZERO,
                };
                Self::DateHistogram(DateHistogram {
                    field: field()?,
                    unit: CalendarUnit::parse(&unit_text)?,
                    fixed_millis,
                    extended_bounds,
                    range_start: body.get("range_start").map(parse_boundary).transpose()?,
                    range_end: body.get("range_end").map(parse_boundary).transpose()?,
                    drop_partial: body.get("drop_partial") == Some(&Value::Bool(true)),
                    time_zone: body
                        .get("time_zone")
                        .map(to_plain_string)
                        .filter(|text| !text.is_empty()),
                    format: body.get("format").map(to_plain_string).filter(|text| !text.is_empty()),
                    offset,
                    offset_text,
                    descending: order_descending(body.get("order"))?,
                    min_doc_count: body
                        .get("min_doc_count")
                        .map(|value| to_plain_string(value).parse::<u64>())
                        .transpose()
                        .map_err(|_| Error::new("min_doc_count must be a whole number"))?
                        .unwrap_or(0),
                    aggs: nested()?,
                })
            }
            "terms" => Self::Terms(Terms {
                field: field()?,
                size: body
                    .get("size")
                    .map(|value| to_plain_string(value).parse::<usize>())
                    .transpose()
                    .map_err(|_| Error::new("terms size must be a whole number"))?
                    .unwrap_or(10),
                order: terms_order(body.get("order"))?,
                aggs: nested()?,
                missing: body.get("missing").map(to_plain_string),
                other: body.get("other_bucket").filter(|v| **v != Value::Bool(false)).map(|_| {
                    body.get("other_bucket_key")
                        .map(to_plain_string)
                        .unwrap_or_else(|| "Other".into())
                }),
                min_doc_count: body
                    .get("min_doc_count")
                    .map(to_plain_string)
                    .map(|v| v.parse::<u64>())
                    .transpose()
                    .map_err(|_| Error::new("min_doc_count must be a whole number"))?
                    .unwrap_or(1),
            }),
            "avg" => Self::Metric {
                field: field()?,
                kind: Metric::Avg,
            },
            "value_count" => Self::Metric {
                field: field()?,
                kind: Metric::ValueCount,
            },
            "median" => Self::Metric {
                field: field()?,
                kind: Metric::Percentile(50.0),
            },
            "percentiles" => {
                let field = field()?;
                let percents = match body.get("percents") {
                    Some(Value::List(items)) => items.clone(),
                    None => vec![Value::I64(50), Value::I64(95), Value::I64(99)],
                    _ => return Err(Error::new("percentiles percents must be a list")),
                };
                let mut aggs = IndexMap::new();
                for percent in percents {
                    let shown = to_plain_string(&percent);
                    let p = shown
                        .parse::<f64>()
                        .ok()
                        .filter(|p| p.is_finite() && (0.0..=100.0).contains(p))
                        .ok_or_else(|| Error::new("percentile must be between 0 and 100"))?;
                    aggs.insert(
                        shown,
                        Self::Metric {
                            field: field.clone(),
                            kind: Metric::Percentile(p),
                        },
                    );
                }
                Self::Filter {
                    query: Query::MatchAll,
                    aggs,
                }
            }
            "stats" => {
                let field = field()?;
                let aggs = IndexMap::from([
                    (
                        "count".into(),
                        Self::Metric {
                            field: field.clone(),
                            kind: Metric::ValueCount,
                        },
                    ),
                    ("sum".into(), Self::Sum { field: field.clone() }),
                    (
                        "min".into(),
                        Self::Extreme {
                            field: field.clone(),
                            max: false,
                        },
                    ),
                    (
                        "max".into(),
                        Self::Extreme {
                            field: field.clone(),
                            max: true,
                        },
                    ),
                    (
                        "avg".into(),
                        Self::Metric {
                            field,
                            kind: Metric::Avg,
                        },
                    ),
                ]);
                Self::Filter {
                    query: Query::MatchAll,
                    aggs,
                }
            }
            "histogram" => {
                let interval = body
                    .get("interval")
                    .map(to_plain_string)
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|v| v.is_finite() && *v > 0.0)
                    .ok_or_else(|| Error::new("histogram requires a positive finite interval"))?;
                let offset = body
                    .get("offset")
                    .map(to_plain_string)
                    .map(|s| s.parse::<f64>())
                    .transpose()
                    .map_err(|_| Error::new("invalid histogram offset"))?
                    .unwrap_or(0.0);
                if !offset.is_finite() {
                    return Err(Error::new("invalid histogram offset"));
                }
                Self::Histogram {
                    field: field()?,
                    interval,
                    offset,
                    aggs: nested()?,
                }
            }
            "filters" => {
                let Some(Value::Map(values)) = body.get("filters") else {
                    return Err(Error::new("filters aggregation needs named filters"));
                };
                let filters = values
                    .iter()
                    .map(|(k, v)| {
                        Query::from_value(v)
                            .or_else(|_| super::es_json::from_es(v))
                            .map(|q| (k.clone(), q))
                    })
                    .collect::<Result<IndexMap<_, _>, _>>()?;
                Self::NamedFilters {
                    filters,
                    aggs: nested()?,
                }
            }
            "range" | "date_range" => {
                let field = field()?;
                let Some(Value::List(ranges)) = body.get("ranges") else {
                    return Err(Error::new("range aggregation requires ranges"));
                };
                let mut filters = IndexMap::new();
                for (index, range) in ranges.iter().enumerate() {
                    let Value::Map(range) = range else {
                        return Err(Error::new("range entry must be a map"));
                    };
                    let name = range
                        .get("key")
                        .map(to_plain_string)
                        .unwrap_or_else(|| index.to_string());
                    if filters.contains_key(&name) {
                        return Err(Error::new("range keys must be unique"));
                    }
                    filters.insert(
                        name,
                        Query::range(&field, range.get("from").cloned(), range.get("to").cloned()),
                    );
                }
                Self::NamedFilters {
                    filters,
                    aggs: nested()?,
                }
            }
            "sum" => Self::Sum { field: field()? },
            "min" => Self::Extreme {
                field: field()?,
                max: false,
            },
            "max" => Self::Extreme {
                field: field()?,
                max: true,
            },
            "count_distinct" | "cardinality" => Self::CountDistinct { field: field()? },
            other => return Err(Error::new(format!("aggregation type {other} is not supported"))),
        })
    }
}

fn parse_boundary(value: &Value) -> Result<Timestamp, Error> {
    match value {
        Value::Timestamp(t) => Ok(*t),
        _ => to_plain_string(value)
            .parse::<Timestamp>()
            .or_else(|_| {
                to_plain_string(value)
                    .parse::<i64>()
                    .map_err(|e| Error::new(e.to_string()))
                    .and_then(|n| Timestamp::from_millisecond(n).map_err(|e| Error::new(e.to_string())))
            })
            .map_err(|e| Error::new(e.to_string())),
    }
}

pub(crate) fn partial_bucket(histogram: &DateHistogram, at: Timestamp) -> Result<bool, Error> {
    let end = if let Some(millis) = histogram.fixed_millis {
        Timestamp::from_millisecond(
            at.as_millisecond()
                .checked_add(millis)
                .ok_or_else(|| Error::new("histogram time overflow"))?,
        )
        .map_err(|e| Error::new(e.to_string()))?
    } else {
        next_bucket(
            at,
            histogram.unit,
            &histogram_zone(histogram.time_zone.as_deref())?,
            histogram.offset,
        )?
    };
    Ok(histogram.range_start.is_some_and(|start| at < start) || histogram.range_end.is_some_and(|thru| end > thru))
}

/// Fixed intervals are durations, distinct from calendar months and DST-aware days.
pub fn fixed_interval(text: &str) -> Result<i64, Error> {
    let units = [
        ("ms", 1_i64),
        ("s", 1000),
        ("m", 60_000),
        ("h", 3_600_000),
        ("d", 86_400_000),
        ("w", 604_800_000),
    ];
    for (unit, multiplier) in units {
        if let Some(value) = text.strip_suffix(unit) {
            if let Some(ms) = value
                .parse::<i64>()
                .ok()
                .filter(|v| *v > 0)
                .and_then(|v| v.checked_mul(multiplier))
            {
                return Ok(ms);
            }
            break;
        }
    }
    Err(Error::new(format!("invalid fixed interval {text}")))
}

/// Other is a negated filter over selected keys, so a multi-valued document is counted once
/// even when it occurs in several omitted groups. Both backends use this same definition.
pub(crate) fn fill_other_buckets(
    backend: &dyn super::SearchBackend,
    mapping: &super::DocumentMapping,
    filter: &Query,
    aggs: &IndexMap<String, Agg>,
    results: &mut IndexMap<String, Value>,
) -> Result<(), Error> {
    let mut aggregate = |filter: &Query, aggs: &IndexMap<String, Agg>| backend.aggregate(mapping, filter, aggs);
    fill_other_buckets_with(&mut aggregate, filter, aggs, results)
}

type AggregateCallback<'a> = dyn FnMut(&Query, &IndexMap<String, Agg>) -> Result<IndexMap<String, Value>, Error> + 'a;

pub(crate) fn fill_other_buckets_with(
    aggregate: &mut AggregateCallback<'_>,
    filter: &Query,
    aggs: &IndexMap<String, Agg>,
    results: &mut IndexMap<String, Value>,
) -> Result<(), Error> {
    fn has_other(aggs: &IndexMap<String, Agg>) -> bool {
        aggs.values().any(|agg| {
            matches!(agg,Agg::Terms(terms)if terms.other.is_some()||terms.missing.is_some())
                || agg.sub_aggs().is_some_and(has_other)
        })
    }
    if !has_other(aggs) {
        return Ok(());
    }
    use super::query::{FieldOp, FieldQuery};
    fn negated(query: Query) -> Query {
        Query::Bool {
            all: Vec::new(),
            any: Vec::new(),
            none: vec![query],
            min_any: None,
        }
    }
    fn bucket_map(value: &mut Value) -> Option<&mut Vec<Value>> {
        if let Value::Map(holder) = value {
            if let Some(Value::List(buckets)) = holder.get_mut("buckets") {
                return Some(buckets);
            }
        }
        None
    }
    for (name, agg) in aggs {
        let Some(value) = results.get_mut(name) else {
            continue;
        };
        if let Agg::Filter { query, aggs } = agg {
            if let Value::Map(map) = value {
                fill_other_buckets_with(aggregate, &Query::all(vec![filter.clone(), query.clone()]), aggs, map)?;
            }
            continue;
        }
        let Some(buckets) = bucket_map(value) else {
            continue;
        };
        if let Agg::Terms(terms) = agg {
            if let Some(label) = &terms.missing {
                let missing = Query::all(vec![
                    filter.clone(),
                    negated(Query::Field(Box::new(FieldQuery {
                        name: terms.field.clone(),
                        op: FieldOp::Exists,
                        value: Value::Null,
                        values: Vec::new(),
                        gt: None,
                        gte: None,
                        lt: None,
                        lte: None,
                        negate: false,
                    }))),
                ]);
                let counts = aggregate(
                    &missing,
                    &IndexMap::from([(
                        "population".into(),
                        Agg::Filter {
                            query: Query::MatchAll,
                            aggs: IndexMap::new(),
                        },
                    )]),
                )?;
                let count = match counts.get("population") {
                    Some(Value::Map(map)) => map
                        .get("doc_count")
                        .map(to_plain_string)
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0),
                    _ => 0,
                };
                if count > 0 && count >= terms.min_doc_count {
                    let mut bucket = aggregate(&missing, &terms.aggs)?;
                    bucket.insert("key".into(), Value::Null);
                    bucket.insert("key_display".into(), Value::string(label));
                    bucket.insert("missing".into(), Value::Bool(true));
                    bucket.insert("doc_count".into(), Value::I64(count.min(i64::MAX as u64) as i64));
                    buckets.push(Value::Map(bucket));
                }
                fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
                    match value {
                        Value::Map(map) => map.get(path).or_else(|| {
                            path.split_once('.')
                                .and_then(|(key, rest)| map.get(key).and_then(|value| at(value, rest)))
                        }),
                        _ => None,
                    }
                }
                buckets.sort_by(|left, right| {
                    let (path, desc) = match &terms.order {
                        TermsOrder::Count { descending } => ("doc_count", *descending),
                        TermsOrder::Key { descending } => ("key", *descending),
                        TermsOrder::Metric { name, descending } => (name.as_str(), *descending),
                    };
                    let order = moqui_lib::object::compare_obj(
                        at(left, path).unwrap_or(&Value::Null),
                        at(right, path).unwrap_or(&Value::Null),
                    )
                    .unwrap_or(std::cmp::Ordering::Equal);
                    (if desc { order.reverse() } else { order }).then_with(|| {
                        to_plain_string(at(left, "key").unwrap_or(&Value::Null))
                            .cmp(&to_plain_string(at(right, "key").unwrap_or(&Value::Null)))
                    })
                });
                buckets.truncate(terms.size);
            }
        }
        for bucket in buckets.iter_mut() {
            let Value::Map(map) = bucket else {
                continue;
            };
            if map.get("missing") == Some(&Value::Bool(true)) {
                continue;
            }
            let key = map.get("key").cloned().unwrap_or(Value::Null);
            let predicate = match agg {
                Agg::Terms(terms) => {
                    if map.get("missing") == Some(&Value::Bool(true)) {
                        negated(Query::Field(Box::new(FieldQuery {
                            name: terms.field.clone(),
                            op: FieldOp::Exists,
                            value: Value::Null,
                            values: Vec::new(),
                            gt: None,
                            gte: None,
                            lt: None,
                            lte: None,
                            negate: false,
                        })))
                    } else {
                        Query::eq(&terms.field, key)
                    }
                }
                Agg::DateHistogram(histogram) => {
                    let Some(start) = (match key {
                        Value::Timestamp(t) => Some(t),
                        _ => None,
                    }) else {
                        continue;
                    };
                    let end = if let Some(ms) = histogram.fixed_millis {
                        Timestamp::from_millisecond(
                            start
                                .as_millisecond()
                                .checked_add(ms)
                                .ok_or_else(|| Error::new("histogram overflow"))?,
                        )
                        .map_err(|e| Error::new(e.to_string()))?
                    } else {
                        next_bucket(
                            start,
                            histogram.unit,
                            &histogram_zone(histogram.time_zone.as_deref())?,
                            histogram.offset,
                        )?
                    };
                    Query::range(
                        &histogram.field,
                        Some(Value::Timestamp(start)),
                        Some(Value::Timestamp(end)),
                    )
                }
                Agg::Histogram { field, interval, .. } => {
                    let start = to_plain_string(&key)
                        .parse::<f64>()
                        .map_err(|_| Error::new("histogram key is not numeric"))?;
                    Query::range(field, Some(Value::F64(start)), Some(Value::F64(start + interval)))
                }
                Agg::NamedFilters { filters, .. } => match filters.get(&to_plain_string(&key)) {
                    Some(query) => query.clone(),
                    None => continue,
                },
                _ => continue,
            };
            if let Some(nested) = agg.sub_aggs() {
                fill_other_buckets_with(aggregate, &Query::all(vec![filter.clone(), predicate]), nested, map)?;
            }
        }
        if let Agg::Terms(terms) = agg {
            if let Some(label) = &terms.other {
                let mut selected = Vec::new();
                let mut include_missing = false;
                for bucket in buckets.iter() {
                    if let Value::Map(map) = bucket {
                        if let Some(key) = map.get("key") {
                            if map.get("missing") == Some(&Value::Bool(true)) {
                                include_missing = true;
                            } else {
                                selected.push(key.clone());
                            }
                        }
                    }
                }
                let mut excluded = Vec::new();
                if !selected.is_empty() {
                    excluded.push(Query::is_in(&terms.field, selected));
                }
                if include_missing {
                    excluded.push(negated(Query::Field(Box::new(FieldQuery {
                        name: terms.field.clone(),
                        op: FieldOp::Exists,
                        value: Value::Null,
                        values: Vec::new(),
                        gt: None,
                        gte: None,
                        lt: None,
                        lte: None,
                        negate: false,
                    }))));
                }
                let other = if excluded.is_empty() {
                    filter.clone()
                } else {
                    Query::all(vec![filter.clone(), negated(Query::any(excluded))])
                };
                let counted = aggregate(
                    &other,
                    &IndexMap::from([(
                        "population".into(),
                        Agg::Filter {
                            query: Query::MatchAll,
                            aggs: IndexMap::new(),
                        },
                    )]),
                )?;
                let count = match counted.get("population") {
                    Some(Value::Map(map)) => map
                        .get("doc_count")
                        .map(to_plain_string)
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0),
                    _ => 0,
                };
                let mut bucket = aggregate(&other, &terms.aggs)?;
                bucket.insert("key".into(), Value::string(label));
                bucket.insert("doc_count".into(), Value::I64(count.min(i64::MAX as u64) as i64));
                bucket.insert("other".into(), Value::Bool(true));
                buckets.push(Value::Map(bucket));
            }
        }
    }
    Ok(())
}

fn order_descending(value: Option<&Value>) -> Result<bool, Error> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(text)) => direction(text),
        Some(Value::Map(map)) => match map.get("_key") {
            Some(value) => direction(&to_plain_string(value)),
            None => Err(Error::new("a date histogram orders by _key")),
        },
        Some(_) => Err(Error::new("order is asc, desc, or {_key: desc}")),
    }
}

fn direction(text: &str) -> Result<bool, Error> {
    match text.to_ascii_lowercase().as_str() {
        "asc" => Ok(false),
        "desc" => Ok(true),
        other => Err(Error::new(format!("order direction {other} is not asc or desc"))),
    }
}

fn terms_order(value: Option<&Value>) -> Result<TermsOrder, Error> {
    let Some(value) = value else {
        return Ok(TermsOrder::Count { descending: true });
    };
    let Value::Map(map) = value else {
        return match value {
            Value::Null => Ok(TermsOrder::Count { descending: true }),
            _ => Err(Error::new("terms order is a map")),
        };
    };
    if let Some(by) = map.get("by") {
        let descending = map
            .get("direction")
            .map_or(Ok(true), |value| direction(&to_plain_string(value)))?;
        return Ok(order_named(&to_plain_string(by), descending));
    }
    let (name, dir) = map.iter().next().ok_or_else(|| Error::new("terms order is empty"))?;
    Ok(order_named(name, direction(&to_plain_string(dir))?))
}

fn order_named(name: &str, descending: bool) -> TermsOrder {
    match name {
        "_count" => TermsOrder::Count { descending },
        "_key" | "_term" => TermsOrder::Key { descending },
        other => TermsOrder::Metric {
            name: other.to_string(),
            descending,
        },
    }
}

/// `+6d`, `-1d`, `+3h`, `30m`, `10s`, `500ms`.
pub fn parse_offset(text: &str) -> Result<SignedDuration, Error> {
    let trimmed = text.trim();
    let (negative, rest) = match trimmed.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    let split = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .ok_or_else(|| Error::new(format!("offset {text} has no unit")))?;
    let amount: i64 = rest[..split]
        .parse()
        .map_err(|_| Error::new(format!("offset {text} has no amount")))?;
    let per_unit = match &rest[split..] {
        "d" => 86_400,
        "h" => 3_600,
        "m" => 60,
        "s" => 1,
        "ms" => {
            let duration = SignedDuration::from_millis(amount);
            return Ok(if negative { -duration } else { duration });
        }
        other => return Err(Error::new(format!("offset unit {other} is not d, h, m, s, or ms"))),
    };
    let seconds = amount
        .checked_mul(per_unit)
        .ok_or_else(|| Error::new(format!("offset {text} is too large")))?;
    let duration = SignedDuration::from_secs(seconds);
    Ok(if negative { -duration } else { duration })
}

/// The zone for a histogram: an IANA name, or a fixed ISO offset such as `+05:00` as Elastic
/// takes. `None` is the server's system zone, or UTC with a warning when that zone has no IANA
/// name and is not a fixed offset (a copied `/etc/localtime`, say): its rules could not be given
/// to the search store, and an offset taken from today would lose DST.
pub fn histogram_zone(name: Option<&str>) -> Result<TimeZone, Error> {
    match name {
        None => {
            let zone = TimeZone::system();
            if zone.iana_name().is_some() || zone.to_fixed_offset().is_ok() {
                return Ok(zone);
            }
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!("the system time zone has no IANA name; date histograms with no time_zone use UTC");
            });
            Ok(TimeZone::UTC)
        }
        Some(name) if name.starts_with(['+', '-']) => {
            static PARSER: jiff::fmt::temporal::DateTimeParser = jiff::fmt::temporal::DateTimeParser::new();
            PARSER
                .parse_time_zone(name)
                .ok()
                .filter(|zone| zone.to_fixed_offset().is_ok())
                .ok_or_else(|| Error::new(format!("time zone offset {name} is not +HH:MM or -HH:MM")))
        }
        Some(name) => {
            TimeZone::get(name).map_err(|err| Error::new(format!("time zone {name} is not known")).with_source(err))
        }
    }
}

/// The IANA name of the zone, for Elastic. A zone with no name is its ISO offset, `+05:00`.
pub fn zone_name(zone: &TimeZone) -> String {
    match zone.iana_name() {
        Some(name) => name.to_string(),
        None => {
            let offset = zone.to_offset(Timestamp::now());
            let seconds = offset.seconds();
            let sign = if seconds < 0 { '-' } else { '+' };
            let total = seconds.unsigned_abs() / 60;
            format!("{sign}{:02}:{:02}", total / 60, total % 60)
        }
    }
}

/// Start of the bucket that holds `at`: `trunc(local(at) - offset) + offset`.
///
/// # Errors
///
/// Returns an error when the civil arithmetic leaves the supported range.
pub fn bucket_start(
    at: Timestamp,
    unit: CalendarUnit,
    zone: &TimeZone,
    offset: SignedDuration,
) -> Result<Timestamp, Error> {
    let local = at.to_zoned(zone.clone()).datetime();
    let shifted = local.checked_sub(offset).map_err(time_error)?;
    let truncated = truncate(shifted, unit)?;
    let start = truncated.checked_add(offset).map_err(time_error)?;
    Ok(start.to_zoned(zone.clone()).map_err(time_error)?.timestamp())
}

/// Start of the bucket after the one that starts at `start`.
///
/// # Errors
///
/// Returns an error when the civil arithmetic leaves the supported range.
pub fn next_bucket(
    start: Timestamp,
    unit: CalendarUnit,
    zone: &TimeZone,
    offset: SignedDuration,
) -> Result<Timestamp, Error> {
    let local = start.to_zoned(zone.clone()).datetime();
    // Step from the unshifted boundary, so a month offset stays on the same day of the month.
    let base = local.checked_sub(offset).map_err(time_error)?;
    let stepped = match unit {
        CalendarUnit::Minute => base.checked_add(1.minute()),
        CalendarUnit::Hour => base.checked_add(1.hour()),
        CalendarUnit::Day => base.checked_add(1.day()),
        CalendarUnit::Week => base.checked_add(1.week()),
        CalendarUnit::Month => base.checked_add(1.month()),
        CalendarUnit::Quarter => base.checked_add(3.months()),
        CalendarUnit::Year => base.checked_add(1.year()),
    }
    .map_err(time_error)?;
    let next = stepped.checked_add(offset).map_err(time_error)?;
    Ok(next.to_zoned(zone.clone()).map_err(time_error)?.timestamp())
}

fn truncate(local: DateTime, unit: CalendarUnit) -> Result<DateTime, Error> {
    let date = local.date();
    let midnight = |date: jiff::civil::Date| date.at(0, 0, 0, 0);
    Ok(match unit {
        CalendarUnit::Minute => date.at(local.hour(), local.minute(), 0, 0),
        CalendarUnit::Hour => date.at(local.hour(), 0, 0, 0),
        CalendarUnit::Day => midnight(date),
        CalendarUnit::Week => {
            let back = i64::from(date.weekday().to_monday_zero_offset());
            midnight(date.checked_sub(back.days()).map_err(time_error)?)
        }
        CalendarUnit::Month => midnight(date.first_of_month()),
        CalendarUnit::Quarter => {
            let month = ((date.month() - 1) / 3) * 3 + 1;
            midnight(jiff::civil::Date::new(date.year(), month, 1).map_err(time_error)?)
        }
        CalendarUnit::Year => midnight(jiff::civil::Date::new(date.year(), 1, 1).map_err(time_error)?),
    })
}

/// The display text of a bucket key: the Java pattern in the zone, or ISO-8601 with no pattern.
///
/// # Errors
///
/// Returns an error when the pattern has a letter outside the supported subset.
pub fn key_display(key: Timestamp, zone: &TimeZone, format: Option<&str>) -> Result<String, Error> {
    let zoned: Zoned = key.to_zoned(zone.clone());
    match format {
        Some(pattern) => moqui_lib::datefmt::format(&zoned, pattern),
        None => moqui_lib::datefmt::format(&zoned, "yyyy-MM-dd'T'HH:mm:ss.SSSXXX"),
    }
}

fn time_error(err: jiff::Error) -> Error {
    Error::new(format!("date histogram arithmetic: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn units_offsets_and_orders_parse() {
        assert_eq!(CalendarUnit::parse("1q").unwrap(), CalendarUnit::Quarter);
        assert_eq!(CalendarUnit::parse("M").unwrap(), CalendarUnit::Month);
        assert_eq!(CalendarUnit::parse("m").unwrap(), CalendarUnit::Minute);
        assert_eq!(parse_offset("-1d").unwrap(), SignedDuration::from_hours(-24));
        assert_eq!(parse_offset("+6d").unwrap(), SignedDuration::from_hours(144));
        assert!(parse_offset("6").is_err());
        assert!(parse_offset("+9223372036854775807d").is_err());
        assert!(parse_offset("-200000000000000000h").is_err());
        assert_eq!(
            parse_offset("9223372036854775807s").unwrap(),
            SignedDuration::from_secs(i64::MAX)
        );
    }

    #[test]
    fn week_buckets_start_monday_and_shift_by_offset() {
        let utc = TimeZone::UTC;
        // 2026-01-07 is a Wednesday. The Monday week starts 2026-01-05.
        let at = ts("2026-01-07T10:00:00Z");
        assert_eq!(
            bucket_start(at, CalendarUnit::Week, &utc, SignedDuration::ZERO).unwrap(),
            ts("2026-01-05T00:00:00Z")
        );
        // A Sunday week start is offset -1d.
        let sunday = parse_offset("-1d").unwrap();
        assert_eq!(
            bucket_start(at, CalendarUnit::Week, &utc, sunday).unwrap(),
            ts("2026-01-04T00:00:00Z")
        );
        assert_eq!(
            bucket_start(ts("2026-01-04T01:00:00Z"), CalendarUnit::Week, &utc, sunday).unwrap(),
            ts("2026-01-04T00:00:00Z")
        );
        assert_eq!(
            next_bucket(ts("2026-01-04T00:00:00Z"), CalendarUnit::Week, &utc, sunday).unwrap(),
            ts("2026-01-11T00:00:00Z")
        );
    }

    #[test]
    fn quarter_and_zone_boundaries() {
        let denver = TimeZone::get("America/Denver").unwrap();
        // 2026-04-01T03:00Z is still March 31 in Denver.
        let at = ts("2026-04-01T03:00:00Z");
        let start = bucket_start(at, CalendarUnit::Quarter, &denver, SignedDuration::ZERO).unwrap();
        assert_eq!(start, ts("2026-01-01T07:00:00Z"));
        let next = next_bucket(start, CalendarUnit::Quarter, &denver, SignedDuration::ZERO).unwrap();
        // DST began in March, so April 1 local midnight is 06:00Z.
        assert_eq!(next, ts("2026-04-01T06:00:00Z"));
        assert_eq!(key_display(next, &denver, Some("dd MMM yyyy")).unwrap(), "01 Apr 2026");
    }

    #[test]
    fn sales_order_tree_reads() {
        let tree = moqui_lib::json::from_json(
            &serde_json::from_str(
                r#"{"DateOuter":{"date_histogram":{"field":"placedDate","calendar_interval":"1w",
                    "time_zone":"UTC","format":"dd MMM yyyy","order":{"_key":"desc"},"offset":"-1d"},
                  "aggs":{"orderCount":{"cardinality":{"field":"orderId","precision_threshold":40000}},
                          "ItemProduct":{"filter":{"term":{"itemTypeEnumId":"ItemProduct"}},
                                         "aggs":{"productIssuedTotal":{"sum":{"field":"issuances.issuedSaleTotal"}}}}}},
                  "TopTerms":{"terms":{"field":"productId","size":15,"order":{"totalSales":"desc"}},
                              "aggs":{"totalSales":{"sum":{"field":"itemSaleTotal"}}}}}"#,
            )
            .unwrap(),
        );
        let aggs = Agg::map_from_value(&tree).unwrap();
        let Agg::DateHistogram(histogram) = &aggs["DateOuter"] else {
            panic!("histogram");
        };
        assert!(histogram.descending);
        assert_eq!(histogram.unit, CalendarUnit::Week);
        assert!(matches!(histogram.aggs["ItemProduct"], Agg::Filter { .. }));
        let Agg::Terms(terms) = &aggs["TopTerms"] else {
            panic!("terms");
        };
        assert_eq!(
            terms.order,
            TermsOrder::Metric {
                name: "totalSales".into(),
                descending: true
            }
        );
    }
}
