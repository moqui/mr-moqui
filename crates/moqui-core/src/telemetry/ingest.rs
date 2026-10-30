//! OTLP/HTTP decoding and normalization. Authentication and request limits belong to the host.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::Value as Json;

use crate::entity::document::Document;

#[derive(Clone, Copy, Debug)]
pub enum Signal {
    Traces,
    Logs,
    Metrics,
}

pub struct Decoded {
    pub index: &'static str,
    pub documents: Vec<Document>,
    pub rejected: u64,
    /// The part of `rejected` that is a cumulative series' first, reset or rebased point: a baseline the next point is
    /// measured from, not a record lost. A puller sees one per series on its first scrape.
    pub baselines: u64,
    pub message: String,
}

#[derive(Default)]
pub struct Decoder {
    cumulative: Mutex<CumulativeCache>,
}

struct CumulativeCache {
    points: HashMap<String, Previous>,
    swept: Instant,
}
impl Default for CumulativeCache {
    fn default() -> Self {
        Self {
            points: HashMap::new(),
            swept: Instant::now(),
        }
    }
}

struct Previous {
    start: String,
    time: i64,
    count: u64,
    sum: Option<f64>,
    integer_sum: Option<i64>,
    buckets: Vec<u64>,
    exponential: Option<super::exponential::Histogram>,
    received: Instant,
}

impl Decoder {
    pub fn decode(&self, signal: Signal, content_type: &str, body: &[u8], source: &str) -> Result<Decoded, Error> {
        self.decode_scoped(signal, content_type, body, source, source)
    }

    /// `source_id` is authenticated server identity; `source` is only a display label.
    pub fn decode_scoped(
        &self,
        signal: Signal,
        content_type: &str,
        body: &[u8],
        source_id: &str,
        source: &str,
    ) -> Result<Decoded, Error> {
        let json: Json = match content_type.split(';').next().unwrap_or("").trim() {
            "application/json" => {
                serde_json::from_slice(body).map_err(|error| Error::new(format!("invalid OTLP JSON: {error}")))?
            }
            "application/x-protobuf" => decode_protobuf(signal, body)?,
            _ => return Err(Error::new("OTLP requires application/json or application/x-protobuf")),
        };
        Ok(self.decode_value(signal, &json, source_id, source))
    }

    /// An OTLP export request already in its JSON form: the OTLP/JSON body, or a protobuf message through its serde
    /// form, as the gRPC receiver has it.
    pub fn decode_value(&self, signal: Signal, json: &Json, source_id: &str, source: &str) -> Decoded {
        let mut decoded = Decoded {
            index: match signal {
                Signal::Traces => "otel_spans",
                Signal::Logs => "otel_logs",
                Signal::Metrics => "otel_metrics",
            },
            documents: Vec::new(),
            rejected: 0,
            baselines: 0,
            message: String::new(),
        };
        let (resources, scope_key, entries) = match signal {
            Signal::Traces => ("resourceSpans", "scopeSpans", "spans"),
            Signal::Logs => ("resourceLogs", "scopeLogs", "logRecords"),
            Signal::Metrics => ("resourceMetrics", "scopeMetrics", "metrics"),
        };
        for resource in array(field(json, resources)) {
            let resource_attrs = attributes(field(field(resource, "resource"), "attributes"));
            for scope in array(field(resource, scope_key)) {
                for entry in array(field(scope, entries)) {
                    match signal {
                        Signal::Traces => match span_document(entry) {
                            Some(mut document) => {
                                enrich(&mut document, &resource_attrs, source_id, "otel_spans");
                                decoded.documents.push(document);
                            }
                            None => decoded.rejected += 1,
                        },
                        Signal::Logs => {
                            let Some(mut document) = log_document(entry) else {
                                decoded.rejected += 1;
                                continue;
                            };
                            document.insert(
                                "scope_name".into(),
                                Value::String(string(field(field(scope, "scope"), "name"))),
                            );
                            enrich(&mut document, &resource_attrs, source_id, "otel_logs");
                            decoded.documents.push(document);
                        }
                        Signal::Metrics => self.metric_documents(entry, &resource_attrs, source_id, &mut decoded),
                    }
                }
            }
        }
        for document in &mut decoded.documents {
            document.insert("source.id".into(), Value::string(source_id));
            document.insert("source.name".into(), Value::string(source));
        }
        if decoded.rejected > 0 {
            decoded.message = if decoded.message.is_empty() {
                format!("{} invalid or unsupported records were skipped", decoded.rejected)
            } else {
                format!("{}; {} records skipped", decoded.message, decoded.rejected)
            };
        }
        decoded
    }

    fn metric_documents(&self, metric: &Json, resource: &Document, source: &str, decoded: &mut Decoded) {
        let name = string(field(metric, "name"));
        let unit = string(field(metric, "unit"));
        if !field(metric, "exponentialHistogram").is_null() {
            for point in array(field(field(metric, "exponentialHistogram"), "dataPoints")) {
                let Some((time, safe)) = exponential_point(point).filter(|_| !name.is_empty()) else {
                    decoded.rejected += 1;
                    continue;
                };
                let attrs = attributes(field(point, "attributes"));
                let fingerprint = series_fingerprint(&format!("{name}\n{unit}\nexponential"), resource, &attrs, source);
                let parsed = (|| -> Result<_, Error> {
                    let histogram = super::exponential::Histogram::parse(&Json::Object(safe))?;
                    let start = if field(point, "startTimeUnixNano").is_null() {
                        0
                    } else {
                        checked_integer(field(point, "startTimeUnixNano"))
                            .ok_or_else(|| Error::new("invalid exponential histogram start"))?
                    };
                    if start < 0 || start > time {
                        return Err(Error::new("invalid exponential histogram interval"));
                    }
                    Ok((histogram, start))
                })();
                let Ok((mut histogram, mut interval_start)) = parsed else {
                    decoded.rejected += 1;
                    continue;
                };
                let cumulative = matches!(
                    string(field(field(metric, "exponentialHistogram"), "aggregationTemporality")).as_str(),
                    "2" | "AGGREGATION_TEMPORALITY_CUMULATIVE"
                );
                if cumulative {
                    let mut state = super::lock(&self.cumulative);
                    if state.swept.elapsed() > Duration::from_secs(60) {
                        state
                            .points
                            .retain(|_, point| point.received.elapsed() < Duration::from_secs(3600));
                        state.swept = Instant::now();
                    }
                    if state.points.len() >= 20_000 && !state.points.contains_key(&fingerprint) {
                        decoded.rejected += 1;
                        decoded.message = "cumulative series limit reached".into();
                        continue;
                    }
                    let next = Previous {
                        start: interval_start.to_string(),
                        time,
                        count: 0,
                        sum: None,
                        integer_sum: None,
                        buckets: Vec::new(),
                        exponential: Some(histogram.clone()),
                        received: Instant::now(),
                    };
                    if state
                        .points
                        .get(&fingerprint)
                        .is_some_and(|previous| time <= previous.time)
                    {
                        decoded.rejected += 1;
                        continue;
                    }
                    if interval_start == time {
                        state.points.insert(fingerprint.clone(), next);
                        decoded.rejected += 1;
                        decoded.baselines += 1;
                        decoded.message = "zero-duration cumulative point established an unknown-start baseline".into();
                        continue;
                    }
                    if let Some(previous) = state.points.get(&fingerprint) {
                        if previous.start != next.start
                            && (interval_start <= 0 || time.saturating_sub(interval_start) > 60_000_000_000)
                        {
                            state.points.insert(fingerprint.clone(), next);
                            decoded.rejected += 1;
                            decoded.baselines += 1;
                            decoded.message =
                                "cumulative reset established a baseline; interval data is unavailable".into();
                            continue;
                        }
                        if previous.start == next.start {
                            let Some(previous_histogram) = &previous.exponential else {
                                decoded.rejected += 1;
                                continue;
                            };
                            match histogram.delta(previous_histogram) {
                                Ok(Some(delta)) => {
                                    histogram = delta;
                                    interval_start = previous.time;
                                }
                                Ok(None) => {
                                    state.points.insert(fingerprint.clone(), next);
                                    decoded.rejected += 1;
                                    decoded.baselines += 1;
                                    decoded.message = "cumulative population decreased without a new start; reset baseline established".into();
                                    continue;
                                }
                                Err(error) => {
                                    decoded.rejected += 1;
                                    decoded.message = error.message().to_string();
                                    continue;
                                }
                            }
                        }
                    } else if interval_start <= 0 || time.saturating_sub(interval_start) > 60_000_000_000 {
                        state.points.insert(fingerprint.clone(), next);
                        decoded.rejected += 1;
                        decoded.baselines += 1;
                        decoded.message =
                            "initial cumulative point established a baseline; interval data is unavailable".into();
                        continue;
                    }
                    state.points.insert(fingerprint.clone(), next);
                }
                let mut document = Document::new();
                document.insert("name".into(), Value::string(&name));
                document.insert("unit".into(), Value::string(&unit));
                document.insert("series_fingerprint".into(), Value::String(fingerprint));
                let start = if interval_start > 0 { interval_start } else { time };
                document.insert(
                    "bucket_start".into(),
                    Value::I64((start / 1_000_000).div_euclid(60_000) * 60_000),
                );
                document.insert("bucket_end".into(), Value::I64(time / 1_000_000));
                document.insert("granularity".into(), Value::string("minute"));
                document.insert("aggregation".into(), Value::string("exponential_histogram"));
                histogram.store(&mut document);
                flatten_attributes(&mut document, attrs, "otel_metrics");
                enrich(&mut document, resource, source, "otel_metrics");
                decoded.documents.push(document);
            }
            return;
        }
        let (data, histogram) = if !field(metric, "histogram").is_null() {
            (field(metric, "histogram"), true)
        } else if !field(metric, "sum").is_null() {
            (field(metric, "sum"), false)
        } else if !field(metric, "gauge").is_null() {
            (field(metric, "gauge"), false)
        } else {
            decoded.rejected += 1;
            return;
        };
        let monotonic = field(data, "isMonotonic").as_bool().unwrap_or(false);
        let cumulative = matches!(
            string(field(data, "aggregationTemporality")).as_str(),
            "2" | "AGGREGATION_TEMPORALITY_CUMULATIVE"
        );
        for point in array(field(data, "dataPoints")) {
            let attrs = attributes(field(point, "attributes"));
            let parsed = (|| {
                let time = checked_integer(field(point, "timeUnixNano"))?;
                if time <= 0 || name.is_empty() {
                    return None;
                }
                let start = if field(point, "startTimeUnixNano").is_null() {
                    0
                } else {
                    checked_integer(field(point, "startTimeUnixNano"))?
                };
                if start < 0 || start > time {
                    return None;
                }
                let count = if histogram {
                    count_value(field(point, "count"))?
                } else {
                    1
                };
                let (sum, integer_sum) = if histogram {
                    (optional_float(field(point, "sum"))?, None)
                } else {
                    let double = field(point, "asDouble");
                    let int = field(point, "asInt");
                    if double.is_null() == int.is_null() {
                        return None;
                    }
                    if !double.is_null() {
                        (Some(checked_float(double)?), None)
                    } else {
                        let value = checked_integer(int)?;
                        (Some(value as f64), Some(value))
                    }
                };
                let bounds = array(field(point, "explicitBounds"))
                    .iter()
                    .map(checked_float)
                    .collect::<Option<Vec<_>>>()?;
                let buckets = array(field(point, "bucketCounts"))
                    .iter()
                    .map(count_value)
                    .collect::<Option<Vec<_>>>()?;
                if histogram
                    && (buckets.len() != bounds.len() + 1
                        || bounds.windows(2).any(|pair| pair[0] >= pair[1])
                        || buckets
                            .iter()
                            .try_fold(0u64, |total, value| total.checked_add(*value))?
                            != count)
                {
                    return None;
                }
                let minimum = optional_float(field(point, "min"))?;
                let maximum = optional_float(field(point, "max"))?;
                if minimum.zip(maximum).is_some_and(|(minimum, maximum)| minimum > maximum) {
                    return None;
                }
                Some((time, start, count, sum, integer_sum, bounds, buckets, minimum, maximum))
            })();
            let Some((
                time,
                mut interval_start,
                mut count,
                mut sum,
                integer_sum,
                bounds,
                mut buckets,
                minimum,
                maximum,
            )) = parsed
            else {
                decoded.rejected += 1;
                continue;
            };
            let start = interval_start.to_string();
            let fingerprint = series_fingerprint(
                &format!("{name}\n{unit}\n{histogram}\n{monotonic}\n{bounds:?}"),
                resource,
                &attrs,
                source,
            );
            let mut interval_extrema = !cumulative;
            if cumulative {
                interval_extrema = true;
                let mut state = super::lock(&self.cumulative);
                if state.swept.elapsed() > Duration::from_secs(60) {
                    state
                        .points
                        .retain(|_, point| point.received.elapsed() < Duration::from_secs(3600));
                    state.swept = Instant::now();
                }
                let cache = &mut state.points;
                if cache.len() >= 20_000 && !cache.contains_key(&fingerprint) {
                    decoded.rejected += 1;
                    if decoded.message.is_empty() {
                        decoded.message = "cumulative series limit reached".into();
                    }
                    continue;
                }
                let previous = Previous {
                    start: start.clone(),
                    time,
                    count,
                    sum,
                    integer_sum,
                    buckets: buckets.clone(),
                    exponential: None,
                    received: Instant::now(),
                };
                if cache.get(&fingerprint).is_some_and(|previous| time <= previous.time) {
                    decoded.rejected += 1;
                    continue;
                }
                if interval_start == time {
                    cache.insert(fingerprint.clone(), previous);
                    decoded.rejected += 1;
                    decoded.baselines += 1;
                    decoded.message = "zero-duration cumulative point established an unknown-start baseline".into();
                    continue;
                }
                if !cache.contains_key(&fingerprint)
                    && (interval_start <= 0 || time.saturating_sub(interval_start) > 60_000_000_000)
                {
                    cache.insert(fingerprint, previous);
                    decoded.rejected += 1;
                    decoded.baselines += 1;
                    if decoded.message.is_empty() {
                        decoded.message =
                            "initial cumulative point established a baseline; interval data is unavailable".into();
                    }
                    continue;
                }
                if let Some(old) = cache.get(&fingerprint) {
                    if time <= old.time {
                        decoded.rejected += 1;
                        continue;
                    }
                    // A double cannot carry the low bits of a large integer snapshot. A
                    // representation switch there provides no exact common baseline.
                    if start == old.start
                        && integer_sum.is_some() != old.integer_sum.is_some()
                        && integer_sum
                            .or(old.integer_sum)
                            .is_some_and(|value| value.unsigned_abs() > 9_007_199_254_740_991)
                    {
                        cache.insert(fingerprint.clone(), previous);
                        decoded.rejected += 1;
                        decoded.baselines += 1;
                        decoded.message = "cumulative numeric representation changed outside the exact double range; baseline established".into();
                        continue;
                    }
                    let decreased = match integer_sum.zip(old.integer_sum) {
                        Some((new, old)) => new < old,
                        None => sum.zip(old.sum).is_some_and(|(new, old)| new < old),
                    };
                    let reset = start != old.start
                        || (histogram
                            && (count < old.count || buckets.iter().zip(&old.buckets).any(|(new, old)| new < old)))
                        || (!histogram && monotonic && decreased);
                    if reset
                        && (start == old.start
                            || interval_start <= 0
                            || time.saturating_sub(interval_start) > 60_000_000_000)
                    {
                        cache.insert(fingerprint.clone(), previous);
                        decoded.rejected += 1;
                        decoded.baselines += 1;
                        decoded.message =
                            "cumulative reset established a baseline; interval data is unavailable".into();
                        continue;
                    }
                    if !reset {
                        interval_extrema = false;
                        interval_start = old.time;
                        count -= if histogram { old.count } else { 0 };
                        sum = match integer_sum.zip(old.integer_sum) {
                            // i128 also handles a valid nonmonotonic signed span whose
                            // difference exceeds i64, without overflowing the snapshots.
                            Some((new, old)) => Some((i128::from(new) - i128::from(old)) as f64),
                            None => sum.zip(old.sum).and_then(|(new, old)| {
                                let value = new - old;
                                value.is_finite().then_some(value)
                            }),
                        };
                        if buckets.len() == old.buckets.len() {
                            for (bucket, old) in buckets.iter_mut().zip(&old.buckets) {
                                *bucket -= old;
                            }
                        } else {
                            decoded.rejected += 1;
                            cache.insert(fingerprint, previous);
                            continue;
                        }
                    }
                }
                cache.insert(fingerprint.clone(), previous);
            }
            let mut document = Document::new();
            document.insert("name".into(), Value::string(&name));
            document.insert("unit".into(), Value::string(&unit));
            document.insert("series_fingerprint".into(), Value::String(fingerprint));
            document.insert(
                "bucket_start".into(),
                Value::I64(
                    (if interval_start > 0 && interval_start <= time {
                        interval_start
                    } else {
                        time
                    } / 1_000_000)
                        .div_euclid(60_000)
                        * 60_000,
                ),
            );
            document.insert("bucket_end".into(), Value::I64(time / 1_000_000));
            document.insert("granularity".into(), Value::string("minute"));
            document.insert(
                "aggregation".into(),
                Value::string(if histogram {
                    "histogram"
                } else if !field(metric, "gauge").is_null() {
                    "gauge"
                } else {
                    "sum"
                }),
            );
            document.insert("count".into(), Value::I64(count as i64));
            if !histogram && field(metric, "gauge").is_null() {
                // A Prometheus counter on `/metrics` only for a monotonic sum.
                document.insert("monotonic".into(), Value::Bool(monotonic));
            }
            if let Some(sum) = sum {
                document.insert("sum".into(), Value::F64(sum));
            }
            if let Some(sum) = sum.filter(|_| !field(metric, "gauge").is_null()) {
                document.insert("min".into(), Value::F64(sum));
                document.insert("max".into(), Value::F64(sum));
                document.insert("last".into(), Value::F64(sum));
                document.insert("last_time".into(), Value::I64(time / 1_000_000));
            }
            if interval_extrema {
                for (key, value) in [("min", minimum), ("max", maximum)] {
                    if let Some(value) = value {
                        document.insert(key.into(), Value::F64(value));
                    }
                }
            }
            document.insert(
                "bounds".into(),
                Value::List(bounds.into_iter().map(Value::F64).collect()),
            );
            document.insert(
                "buckets".into(),
                Value::List(buckets.into_iter().map(|value| Value::I64(value as i64)).collect()),
            );
            flatten_attributes(&mut document, attrs, "otel_metrics");
            enrich(&mut document, resource, source, "otel_metrics");
            decoded.documents.push(document);
        }
    }
}

#[cfg(feature = "telemetry")]
fn decode_protobuf(signal: Signal, bytes: &[u8]) -> Result<Json, Error> {
    use opentelemetry_proto::tonic::collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    };
    use prost::Message;
    let invalid = |error: prost::DecodeError| Error::new(format!("invalid OTLP protobuf: {error}"));
    let json = match signal {
        Signal::Traces => serde_json::to_value(ExportTraceServiceRequest::decode(bytes).map_err(invalid)?),
        Signal::Logs => serde_json::to_value(ExportLogsServiceRequest::decode(bytes).map_err(invalid)?),
        Signal::Metrics => serde_json::to_value(ExportMetricsServiceRequest::decode(bytes).map_err(invalid)?),
    };
    json.map_err(|error| Error::new(format!("OTLP normalization failed: {error}")))
}

#[cfg(not(feature = "telemetry"))]
fn decode_protobuf(_signal: Signal, _bytes: &[u8]) -> Result<Json, Error> {
    Err(Error::new("telemetry is not built into this host"))
}

fn span_document(span: &Json) -> Option<Document> {
    let trace_id = string(field(span, "traceId"));
    let span_id = string(field(span, "spanId"));
    if !super::valid_hex(&trace_id, 32)
        || !super::valid_hex(&span_id, 16)
        || trace_id.bytes().all(|b| b == b'0')
        || span_id.bytes().all(|b| b == b'0')
    {
        return None;
    }
    let start = checked_integer(field(span, "startTimeUnixNano"))?;
    let end = checked_integer(field(span, "endTimeUnixNano"))?;
    if start <= 0 || end < start {
        return None;
    }
    let mut document = Document::new();
    document.insert("trace_id".into(), Value::String(trace_id));
    document.insert("span_id".into(), Value::String(span_id));
    document.insert(
        "parent_span_id".into(),
        Value::String(string(field(span, "parentSpanId"))),
    );
    document.insert("name".into(), Value::String(string(field(span, "name"))));
    let kind = match string(field(span, "kind")).as_str() {
        "2" | "SPAN_KIND_SERVER" => "SERVER",
        "3" | "SPAN_KIND_CLIENT" => "CLIENT",
        "4" | "SPAN_KIND_PRODUCER" => "PRODUCER",
        "5" | "SPAN_KIND_CONSUMER" => "CONSUMER",
        _ => "INTERNAL",
    };
    document.insert("kind".into(), Value::string(kind));
    document.insert("start".into(), Value::I64(start / 1_000_000));
    document.insert("start_nano".into(), Value::I64(start));
    document.insert("duration_nano".into(), Value::I64(end - start));
    let status = match string(field(field(span, "status"), "code")).as_str() {
        "2" | "STATUS_CODE_ERROR" => "ERROR",
        "1" | "STATUS_CODE_OK" => "OK",
        _ => "UNSET",
    };
    document.insert("status_code".into(), Value::string(status));
    document.insert("sampled".into(), Value::Bool(true));
    flatten_attributes(&mut document, attributes(field(span, "attributes")), "otel_spans");
    let events = array(field(span, "events"))
        .iter()
        .map(|event| {
            let event_time = if field(event, "timeUnixNano").is_null() {
                start
            } else {
                checked_integer(field(event, "timeUnixNano"))?
            };
            if event_time < 0 {
                return None;
            }
            let mut values = attributes(field(event, "attributes"));
            values.insert("name".into(), Value::String(string(field(event, "name"))));
            values.insert("timestamp".into(), Value::I64(event_time / 1_000_000));
            if string(field(event, "name")) == "exception" {
                for key in ["exception.type", "exception.message", "exception.stacktrace"] {
                    if let Some(value) = values.get(key) {
                        document.insert(key.into(), value.clone());
                    }
                }
            }
            Some(Value::Map(values))
        })
        .collect::<Option<Vec<_>>>()?;
    document.insert("events".into(), Value::List(events));
    Some(document)
}

fn log_document(log: &Json) -> Option<Document> {
    let mut document = Document::new();
    let timestamp = if field(log, "timeUnixNano").is_null() {
        0
    } else {
        checked_integer(field(log, "timeUnixNano"))?
    };
    if timestamp < 0 {
        return None;
    }
    document.insert(
        "timestamp".into(),
        Value::I64(if timestamp > 0 {
            timestamp / 1_000_000
        } else {
            crate::user::now_millis()
        }),
    );
    document.insert(
        "severity_number".into(),
        Value::I64(integer(field(log, "severityNumber"))),
    );
    document.insert(
        "severity_text".into(),
        Value::String(string(field(log, "severityText"))),
    );
    document.insert("body".into(), any_value(field(log, "body")));
    for (from, to) in [("traceId", "trace_id"), ("spanId", "span_id")] {
        document.insert(to.into(), Value::String(string(field(log, from))));
    }
    flatten_attributes(&mut document, attributes(field(log, "attributes")), "otel_logs");
    Some(document)
}

fn field<'a>(value: &'a Json, key: &str) -> &'a Json {
    if let Some(found) = value.get(key) {
        return found;
    }
    let mut snake = String::new();
    for ch in key.chars() {
        if ch.is_uppercase() {
            snake.push('_');
            snake.extend(ch.to_lowercase());
        } else {
            snake.push(ch);
        }
    }
    value.get(&snake).unwrap_or(&Json::Null)
}
fn array(value: &Json) -> &[Json] {
    value.as_array().map_or(&[], Vec::as_slice)
}
fn string(value: &Json) -> String {
    value.as_str().map(str::to_string).unwrap_or_else(|| {
        if value.is_null() {
            String::new()
        } else {
            value.to_string()
        }
    })
}
fn exponential_point(point: &Json) -> Option<(i64, serde_json::Map<String, Json>)> {
    let time = checked_integer(field(point, "timeUnixNano"))?;
    if time <= 0 {
        return None;
    }
    let count = count_value(field(point, "count"))?;
    let zero = if field(point, "zeroCount").is_null() {
        0
    } else {
        count_value(field(point, "zeroCount"))?
    };
    let scale = if field(point, "scale").is_null() {
        0
    } else {
        checked_integer(field(point, "scale"))?
    };
    if !(-10..=20).contains(&scale) {
        return None;
    }
    let mut safe = serde_json::Map::new();
    safe.insert("count".into(), serde_json::json!(count));
    safe.insert("zeroCount".into(), serde_json::json!(zero));
    safe.insert("scale".into(), serde_json::json!(scale));
    let mut total = zero;
    for key in ["positive", "negative"] {
        let values = field(point, key);
        if values.is_null() {
            continue;
        }
        if !values.is_object() {
            return None;
        }
        let offset = if field(values, "offset").is_null() {
            0
        } else {
            checked_integer(field(values, "offset"))?
        };
        i32::try_from(offset).ok()?;
        let raw = field(values, "bucketCounts");
        if (!raw.is_null() && !raw.is_array()) || array(raw).len() > super::exponential::MAX_BUCKETS {
            return None;
        }
        let buckets = array(raw).iter().map(count_value).collect::<Option<Vec<_>>>()?;
        total = buckets
            .iter()
            .try_fold(total, |total, value| total.checked_add(*value))?;
        safe.insert(key.into(), serde_json::json!({"offset":offset,"bucketCounts":buckets}));
    }
    if total != count {
        return None;
    }
    let minimum = optional_float(field(point, "min"))?;
    let maximum = optional_float(field(point, "max"))?;
    if minimum.zip(maximum).is_some_and(|(min, max)| min > max) {
        return None;
    }
    for key in ["sum", "zeroThreshold", "min", "max"] {
        if let Some(value) = optional_float(field(point, key))? {
            if key == "zeroThreshold" && value < 0.0 {
                return None;
            }
            safe.insert(key.into(), serde_json::json!(value));
        }
    }
    Some((time, safe))
}

fn checked_integer(value: &Json) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}
fn checked_float(value: &Json) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .filter(|value| value.is_finite())
}
fn optional_float(value: &Json) -> Option<Option<f64>> {
    if value.is_null() {
        Some(None)
    } else {
        checked_float(value).map(Some)
    }
}
fn count_value(value: &Json) -> Option<u64> {
    checked_integer(value).and_then(|value| u64::try_from(value).ok())
}
fn integer(value: &Json) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .unwrap_or(0)
}
fn float(value: &Json) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

fn sensitive(key: &str) -> bool {
    super::sensitive_key(key)
}

fn attributes(value: &Json) -> Document {
    array(value)
        .iter()
        .filter_map(|item| {
            let key = string(field(item, "key"));
            (!key.is_empty() && !matches!(key.as_str(), "source" | "source.name" | "source.id") && !sensitive(&key))
                .then(|| (key, any_value(field(item, "value"))))
        })
        .take(128)
        .collect()
}

fn any_value(value: &Json) -> Value {
    for key in [
        "stringValue",
        "boolValue",
        "intValue",
        "doubleValue",
        "arrayValue",
        "kvlistValue",
        "bytesValue",
    ] {
        let item = field(value, key);
        if item.is_null() {
            continue;
        }
        return match key {
            "stringValue" | "bytesValue" => {
                Value::String(super::redact_text(&string(item)).chars().take(16_384).collect())
            }
            "boolValue" => Value::Bool(item.as_bool().unwrap_or(false)),
            "intValue" => Value::I64(integer(item)),
            "doubleValue" => Value::F64(float(item)),
            "arrayValue" => Value::List(array(field(item, "values")).iter().take(128).map(any_value).collect()),
            "kvlistValue" => Value::Map(attributes(field(item, "values"))),
            _ => Value::Null,
        };
    }
    Value::Null
}

fn root_fields(index: &str) -> Option<&'static std::collections::HashSet<String>> {
    static FIELDS: std::sync::OnceLock<HashMap<&'static str, std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    FIELDS
        .get_or_init(|| {
            crate::search::telemetry::TELEMETRY_INDEXES
                .iter()
                .filter_map(|index| {
                    crate::search::telemetry::telemetry_mapping(index)
                        .map(|mapping| (*index, mapping.root.fields.into_keys().collect()))
                })
                .collect()
        })
        .get(index)
}

pub(super) fn flatten_attributes(document: &mut Document, attrs: Document, index: &str) {
    let fields = root_fields(index);
    for (key, value) in &attrs {
        if fields.is_some_and(|fields| fields.contains(key)) && !document.contains_key(key) {
            document.insert(key.clone(), value.clone());
        }
    }
    document.insert("attributes".into(), Value::Map(attrs));
}
fn enrich(document: &mut Document, resource: &Document, source: &str, index: &str) {
    let fields = root_fields(index);
    for (key, value) in resource {
        if fields.is_some_and(|fields| fields.contains(key)) && !document.contains_key(key) {
            document.insert(key.clone(), value.clone());
        }
    }
    document.insert("resource".into(), Value::Map(resource.clone()));
    // The authenticated source cannot be replaced by a sender-controlled attribute.
    document.insert("source.name".into(), Value::string(source));
    document.insert("source.id".into(), Value::string(source));
}

fn series_fingerprint(name: &str, resource: &Document, attributes: &Document, source: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut fields: Vec<_> = resource
        .iter()
        .chain(attributes.iter())
        .map(|(key, value)| format!("{key}={}", moqui_lib::object::to_plain_string(value)))
        .collect();
    fields.sort();
    format!(
        "{:x}",
        Sha256::digest(format!("{source}\n{name}\n{}", fields.join("\n")).as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cumulative_sum(decoder: &Decoder, time: i64, integer: Option<i64>, double: Option<f64>) -> Decoded {
        let mut point = json!({"startTimeUnixNano":"1000000000","timeUnixNano":time.to_string()});
        if let Some(value) = integer {
            point["asInt"] = json!(value.to_string());
        }
        if let Some(value) = double {
            point["asDouble"] = json!(value);
        }
        let body = json!({"resourceMetrics":[{"scopeMetrics":[{"metrics":[{"name":"exact-cumulative",
            "sum":{"aggregationTemporality":2,"isMonotonic":true,"dataPoints":[point]}}]}]}]});
        decoder
            .decode(
                Signal::Metrics,
                "application/json",
                body.to_string().as_bytes(),
                "exact-fixture",
            )
            .unwrap()
    }

    #[test]
    fn cumulative_integer_sum_preserves_adjacent_large_values_and_detects_reset() {
        let decoder = Decoder::default();
        let large = 9_007_199_254_740_992_i64;
        assert_eq!(cumulative_sum(&decoder, 60_000_000_000, Some(large), None).rejected, 0);
        let increment = cumulative_sum(&decoder, 120_000_000_000, Some(large + 1), None);
        assert_eq!(increment.documents[0]["sum"], Value::F64(1.0));
        let decrease = cumulative_sum(&decoder, 180_000_000_000, Some(large), None);
        assert_eq!(
            decrease.rejected, 1,
            "a one-unit decrease must reset even when float snapshots compare equal"
        );
        assert!(decrease.documents.is_empty());
        let next = cumulative_sum(&decoder, 240_000_000_000, Some(large + 1), None);
        assert_eq!(next.documents[0]["sum"], Value::F64(1.0));
        assert_eq!(next.documents[0]["bucket_start"], Value::I64(180_000));
    }

    #[test]
    fn cumulative_doubles_and_safe_mixed_values_keep_deltas_while_unsafe_switch_rebaselines() {
        let decoder = Decoder::default();
        assert_eq!(cumulative_sum(&decoder, 60_000_000_000, Some(9), None).rejected, 0);
        assert_eq!(
            cumulative_sum(&decoder, 120_000_000_000, None, Some(11.5)).documents[0]["sum"],
            Value::F64(2.5)
        );
        assert_eq!(
            cumulative_sum(&decoder, 180_000_000_000, None, Some(12.0)).documents[0]["sum"],
            Value::F64(0.5)
        );
        assert_eq!(
            cumulative_sum(&decoder, 240_000_000_000, Some(14), None).documents[0]["sum"],
            Value::F64(2.0)
        );
        let large = 9_007_199_254_740_992_i64;
        let decoder = Decoder::default();
        cumulative_sum(&decoder, 60_000_000_000, Some(large + 1), None);
        let switched = cumulative_sum(&decoder, 120_000_000_000, None, Some(large as f64));
        assert_eq!(switched.rejected, 1);
        assert!(switched.documents.is_empty());
        assert!(switched.message.contains("representation"));
        assert_eq!(
            cumulative_sum(&decoder, 180_000_000_000, None, Some(large as f64 + 2.0)).documents[0]["sum"],
            Value::F64(2.0)
        );
    }
}
