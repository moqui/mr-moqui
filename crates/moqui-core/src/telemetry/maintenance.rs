//! Idempotent metric rollups and per-signal retention, run by ordinary ServiceJobs.

use std::collections::{BTreeMap, HashSet};

use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::context::ExecutionContextFactory;
use crate::entity::document::Document;
use crate::search::query::Query;
use crate::search::{SearchFacade, SearchRequest, SortField, Target};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
const MAX_GROUPS: usize = 100_000;

/// Delete each independent index/granularity even if one retention operation fails.
pub fn retain(factory: &ExecutionContextFactory) -> Result<Document, Error> {
    let facade = crate::search::installed(factory).ok_or_else(|| Error::new("search is not installed"))?;
    let _suppressed = super::suppress();
    let now = crate::user::now_millis();
    let mut deleted = 0_u64;
    let mut failed = 0_i64;
    for (index, time, setting, default, granularity) in [
        ("otel_spans", "start", "span-retention-days", 7, None),
        ("otel_logs", "timestamp", "log-retention-days", 7, None),
        ("otel_errors", "last_seen", "error-retention-days", 90, None),
        ("monitor_annotations", "time", "annotation-retention-days", 730, None),
        (
            "otel_metrics",
            "bucket_start",
            "minute-retention-days",
            7,
            Some("minute"),
        ),
        ("otel_metrics", "bucket_start", "hour-retention-days", 90, Some("hour")),
        ("otel_metrics", "bucket_start", "day-retention-days", 730, Some("day")),
    ] {
        let days = factory.conf_number::<i64>("telemetry", setting, default);
        if days <= 0 {
            continue;
        }
        let mut filters = vec![Query::range(
            time,
            None,
            Some(Value::I64(now.saturating_sub(days.saturating_mul(DAY)))),
        )];
        if let Some(granularity) = granularity {
            filters.push(Query::eq("granularity", Value::string(granularity)));
        }
        match facade.delete_matching(&Target::Index(index.to_string()), &Query::all(filters)) {
            Ok(count) => deleted += count,
            Err(error) => {
                failed += 1;
                tracing::error!(index, ?granularity, "telemetry retention failed: {error}");
            }
        }
    }
    Ok(Document::from_iter([
        ("deleted".into(), Value::I64(deleted.min(i64::MAX as u64) as i64)),
        ("failed".into(), Value::I64(failed)),
    ]))
}

/// Replay completed periods with deterministic ids. Raw data is retained independently, so a
/// failed period can be retried without losing source evidence or doubling its counters.
pub fn rollup_metrics(factory: &ExecutionContextFactory) -> Result<Document, Error> {
    let facade = crate::search::installed(factory).ok_or_else(|| Error::new("search is not installed"))?;
    let _suppressed = super::suppress();
    let now = crate::user::now_millis();
    let mut written = 0_i64;
    let mut failed = 0_i64;
    for (source, target, width, max_periods) in [("minute", "hour", HOUR, 24), ("hour", "day", DAY, 7)] {
        let end = now.div_euclid(width) * width;
        let (latest, oldest) = match boundary(&facade, target, true)
            .and_then(|latest| boundary(&facade, source, false).map(|oldest| (latest, oldest)))
        {
            Ok(boundaries) => boundaries,
            Err(error) => {
                failed += 1;
                tracing::error!(source, target, "telemetry rollup boundary failed: {error}");
                continue;
            }
        };
        let Some(oldest) = oldest else {
            continue;
        };
        let mut cursor = latest
            .map_or(oldest, |latest| latest.saturating_sub(width))
            .div_euclid(width)
            * width;
        cursor = cursor.max(oldest.div_euclid(width) * width);
        for _ in 0..max_periods {
            if cursor >= end {
                break;
            }
            match read_period(&facade, source, cursor, cursor + width, target, width) {
                Ok(documents) => {
                    let mut complete = true;
                    for chunk in documents.chunks(256) {
                        match facade.index_entries("otel_metrics", chunk.to_vec()) {
                            Ok(count) if count == chunk.len() => written += count as i64,
                            Ok(count) => {
                                written += count as i64;
                                failed += 1;
                                complete = false;
                                tracing::error!(source, target, start = cursor, "partial telemetry rollup write");
                            }
                            Err(error) => {
                                failed += 1;
                                complete = false;
                                tracing::error!(
                                    source,
                                    target,
                                    start = cursor,
                                    "telemetry rollup write failed: {error}"
                                );
                            }
                        }
                    }
                    // Later periods depend on a complete watermark; independent chunks above still run.
                    if !complete {
                        break;
                    }
                }
                Err(error) => {
                    failed += 1;
                    tracing::error!(source, target, start = cursor, "telemetry rollup failed: {error}");
                    break;
                }
            }
            cursor += width;
        }
    }
    Ok(Document::from_iter([
        ("written".into(), Value::I64(written)),
        ("failed".into(), Value::I64(failed)),
    ]))
}

fn boundary(facade: &SearchFacade, granularity: &str, descending: bool) -> Result<Option<i64>, Error> {
    let mut query = SearchRequest::new(Query::eq("granularity", Value::string(granularity)));
    query.limit = 1;
    query.sort.push(SortField {
        field: "bucket_start".into(),
        descending,
    });
    facade
        .search(&Target::Index("otel_metrics".into()), &query)?
        .hits
        .first()
        .map(|hit| bucket_time(&hit.body))
        .transpose()
}

fn read_period(
    facade: &SearchFacade,
    granularity: &str,
    start: i64,
    end: i64,
    target: &str,
    width: i64,
) -> Result<Vec<Document>, Error> {
    let mut request = SearchRequest::new(Query::all(vec![
        Query::eq("granularity", Value::string(granularity)),
        Query::range("bucket_start", Some(Value::I64(start)), Some(Value::I64(end))),
    ]));
    request.limit = 1000;
    request.sort = vec![
        SortField {
            field: "bucket_start".into(),
            descending: false,
        },
        SortField {
            field: "__cursor".into(),
            descending: false,
        },
    ];
    let mut aggregate = RollupAccumulator::default();
    let mut rows = 0;
    loop {
        let page = facade.search(&Target::Index("otel_metrics".into()), &request)?;
        if page.hits.is_empty() {
            break;
        }
        rows += page.hits.len();
        if rows > 2_000_000 {
            return Err(Error::new(
                "telemetry rollup period exceeds the row budget; source data was retained",
            ));
        }
        if let Some(last) = page.hits.last() {
            request.search_after = Some(vec![
                last.body.get("bucket_start").cloned().unwrap_or(Value::Null),
                Value::string(format!("{}:{}", last.document_id, last.id)),
            ]);
        }
        let complete = page.hits.len() < request.limit;
        for hit in page.hits {
            if let Err(error) = aggregate.push(hit.body, target, width) {
                if aggregate.groups.len() + aggregate.bad.len() >= MAX_GROUPS {
                    return Err(error);
                }
                tracing::error!(id = %hit.id, "telemetry rollup record skipped: {error}");
            }
        }
        if complete {
            break;
        }
    }
    Ok(aggregate.groups.into_values().collect())
}

/// Histogram boundaries must match exactly; mismatched series are rejected rather than merged
/// into a misleading percentile. The caller retains the raw period for a corrected retry.
pub fn rollup_documents(documents: Vec<Document>, granularity: &str, width: i64) -> Result<Vec<Document>, Error> {
    let mut aggregate = RollupAccumulator::default();
    for document in documents {
        aggregate.push(document, granularity, width)?;
    }
    Ok(aggregate.groups.into_values().collect())
}

#[derive(Default)]
struct RollupAccumulator {
    groups: BTreeMap<String, Document>,
    bad: HashSet<String>,
}

impl RollupAccumulator {
    fn push(&mut self, mut document: Document, granularity: &str, width: i64) -> Result<(), Error> {
        let bucket = bucket_time(&document)?.div_euclid(width) * width;
        let series = super::text(&document, "series_fingerprint");
        if series.is_empty() {
            return Err(Error::new("metric has no series fingerprint"));
        }
        let key = format!("{series}:{granularity}:{bucket}");
        if self.bad.contains(&key) {
            return Ok(());
        }
        if super::text(&document, "aggregation") == "exponential_histogram" {
            let normalized = (|| {
                if matches!(
                    super::text(&document, "aggregation_temporality").as_str(),
                    "2" | "AGGREGATION_TEMPORALITY_CUMULATIVE"
                ) {
                    return Err(Error::new(
                        "raw cumulative exponential histogram cannot be rolled up without interval data",
                    ));
                }
                let point = document
                    .get("exponential_histogram")
                    .ok_or_else(|| Error::new("exponential histogram population missing"))?;
                let histogram = super::exponential::Histogram::parse(&moqui_lib::json::to_json(point))?;
                histogram.store(&mut document);
                Ok(())
            })();
            if let Err(error) = normalized {
                self.groups.remove(&key);
                self.bad.insert(key);
                return Err(error);
            }
        }
        if let Some(group) = self.groups.get_mut(&key) {
            if let Err(error) = merge(group, &document) {
                self.groups.remove(&key);
                self.bad.insert(key);
                return Err(error);
            }
        } else {
            if self.groups.len() + self.bad.len() >= MAX_GROUPS {
                return Err(Error::new(
                    "telemetry rollup exceeds the group budget; source data was retained",
                ));
            }
            let mut group = document;
            group.insert("_id".into(), Value::string(&key));
            group.insert("granularity".into(), Value::string(granularity));
            group.insert("bucket_start".into(), Value::I64(bucket));
            group.insert("bucket_end".into(), Value::I64(bucket + width));
            self.groups.insert(key, group);
        }
        Ok(())
    }
}

fn metric_time(document: &Document, key: &str) -> Option<i64> {
    document
        .get(key)
        .and_then(crate::search::postgres::compile::parse_date_bound)
        .map(|time| time.as_millisecond())
}
fn bucket_time(document: &Document) -> Result<i64, Error> {
    metric_time(document, "bucket_start").ok_or_else(|| Error::new("metric bucket_start is missing or invalid"))
}

fn merge(group: &mut Document, document: &Document) -> Result<(), Error> {
    if group.get("aggregation") != document.get("aggregation") {
        return Err(Error::new("cannot merge different metric aggregations"));
    }
    if super::text(group, "aggregation") == "exponential_histogram" {
        let point = |document: &Document| {
            let value = document
                .get("exponential_histogram")
                .ok_or_else(|| Error::new("exponential histogram population missing"))?;
            super::exponential::Histogram::parse(&moqui_lib::json::to_json(value))
        };
        point(group)?.merge(&point(document)?)?.store(group);
        return Ok(());
    }
    if metric_time(document, "last_time") >= metric_time(group, "last_time") {
        for key in ["last", "last_time"] {
            if let Some(value) = document.get(key) {
                group.insert(key.into(), value.clone());
            }
        }
    }
    let number = |value: &Value| {
        moqui_lib::object::to_plain_string(value)
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
    };
    let integer = |value: &Value| {
        moqui_lib::object::to_plain_string(value)
            .parse::<i64>()
            .ok()
            .filter(|number| *number >= 0)
    };
    let count = group
        .get("count")
        .and_then(integer)
        .zip(document.get("count").and_then(integer))
        .and_then(|(old, new)| old.checked_add(new))
        .ok_or_else(|| Error::new("invalid or overflowing metric count"))?;
    group.insert("count".into(), Value::I64(count));
    for (key, minimum) in [("sum", None), ("min", Some(true)), ("max", Some(false))] {
        if let Some((old, new)) = group.get(key).and_then(number).zip(document.get(key).and_then(number)) {
            let merged = match minimum {
                None => old + new,
                Some(true) => old.min(new),
                Some(false) => old.max(new),
            };
            if !merged.is_finite() {
                return Err(Error::new("non-finite metric aggregate"));
            }
            group.insert(key.into(), Value::F64(merged));
        } else {
            // An absent interval statistic cannot be reconstructed from the other intervals.
            group.shift_remove(key);
        }
    }
    if group.get("bounds") != document.get("bounds") {
        return Err(Error::new("cannot merge metric histogram boundaries"));
    }
    if let Some(Value::List(incoming)) = document.get("buckets") {
        if let Some(Value::List(buckets)) = group.get_mut("buckets") {
            if buckets.len() != incoming.len() {
                return Err(Error::new("cannot merge metric histogram bucket counts"));
            }
            for (bucket, incoming) in buckets.iter_mut().zip(incoming) {
                let value = integer(bucket)
                    .zip(integer(incoming))
                    .and_then(|(old, new)| old.checked_add(new))
                    .ok_or_else(|| Error::new("invalid or overflowing histogram bucket"))?;
                *bucket = Value::I64(value);
            }
        }
    }
    Ok(())
}

/// Merge explicit histogram buckets across compatible series and periods.
pub fn merge_histogram_buckets(documents: &[Document]) -> Result<(Vec<f64>, Vec<u64>), Error> {
    let mut bounds = None;
    let mut counts = Vec::<u64>::new();
    let mut total = 0_u64;
    for document in documents {
        let Some(Value::List(bound_values)) = document.get("bounds") else {
            return Err(Error::new("histogram bounds are missing"));
        };
        let Some(Value::List(count_values)) = document.get("buckets") else {
            return Err(Error::new("histogram buckets are missing"));
        };
        let incoming_bounds = bound_values
            .iter()
            .map(|value| {
                moqui_lib::object::to_plain_string(value)
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| Error::new("invalid histogram boundary"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let incoming_counts = count_values
            .iter()
            .map(|value| {
                let text = match value {
                    Value::I32(value) => value.to_string(),
                    Value::I64(value) => value.to_string(),
                    Value::Decimal(value) => value.normalize().to_string(),
                    Value::String(value) => value.clone(),
                    _ => return Err(Error::new("histogram bucket counts require exact nonnegative integers")),
                };
                text.parse::<u64>()
                    .map_err(|_| Error::new("histogram bucket counts require exact nonnegative integers"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if incoming_counts.len() != incoming_bounds.len() + 1
            || incoming_bounds.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(Error::new("invalid histogram boundaries or bucket count"));
        }
        for count in &incoming_counts {
            total = total
                .checked_add(*count)
                .ok_or_else(|| Error::new("histogram observation total overflows u64"))?;
        }
        if let Some(bounds) = &bounds {
            if bounds != &incoming_bounds {
                return Err(Error::new("cannot merge different histogram boundaries"));
            }
        } else {
            counts = vec![0; incoming_counts.len()];
            bounds = Some(incoming_bounds);
        }
        for (count, incoming) in counts.iter_mut().zip(incoming_counts) {
            *count = count
                .checked_add(incoming)
                .ok_or_else(|| Error::new("histogram bucket count overflows u64"))?;
        }
    }
    Ok((bounds.unwrap_or_default(), counts))
}

/// Linear interpolation within a bucket; the unbounded last bucket reports its lower bound.
pub fn histogram_percentile(bounds: &[f64], counts: &[u64], quantile: f64) -> Option<f64> {
    if counts.len() != bounds.len() + 1
        || !quantile.is_finite()
        || !(0.0..=1.0).contains(&quantile)
        || bounds.iter().any(|bound| !bound.is_finite())
        || bounds.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return None;
    }
    let total = counts
        .iter()
        .try_fold(0_u64, |total, count| total.checked_add(*count))?;
    if total == 0 {
        return None;
    }
    let target = quantile * total as f64;
    let mut previous = 0_u64;
    for (index, count) in counts.iter().copied().enumerate() {
        let next = previous.checked_add(count)?;
        if count > 0 && next as f64 >= target {
            let lower = index
                .checked_sub(1)
                .and_then(|index| bounds.get(index))
                .copied()
                .unwrap_or_else(|| bounds.first().copied().unwrap_or(0.0).min(0.0));
            let upper = bounds.get(index).copied().unwrap_or(lower);
            let fraction = ((target - previous as f64) / count as f64).clamp(0.0, 1.0);
            let value = lower * (1.0 - fraction) + upper * fraction;
            return value.is_finite().then_some(value);
        }
        previous = next;
    }
    bounds.last().copied()
}
