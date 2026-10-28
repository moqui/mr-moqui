//! Pulled telemetry: Prometheus-text scrape targets written to `otel_metrics`, and synthetic HTTP checks recorded as
//! spans. Scraped samples become OTLP JSON and go through the receiver's decoder, so cumulative counters and histograms
//! become deltas the same way pushed ones do. One failing target or check is logged and recorded on its row, and the
//! others still run.
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::{json, Value as Json};

use super::ingest::{Decoder, Signal};
use super::SpanKind;
use crate::context::ExecutionContext;
use crate::native::{Parameters, ServiceInfo};

const TARGETS: &str = "moqui.monitor.ScrapeTarget";
const CHECKS: &str = "moqui.monitor.SyntheticCheck";
/// A larger exposition is refused rather than parsed.
pub const MAX_BODY: usize = 8 * 1024 * 1024;
/// Data points written per target and scrape; the rest are counted as skipped.
pub const MAX_POINTS: usize = 10_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

/// The samples of one metric family, with its `# TYPE` (`untyped` when none was given).
#[derive(Debug, Clone, PartialEq)]
pub struct Family {
    pub name: String,
    pub kind: String,
    pub help: String,
    pub samples: Vec<Sample>,
}

/// Families in the order they first appear, and the number of lines that could not be read.
#[derive(Debug, Default)]
pub struct Exposition {
    pub families: Vec<Family>,
    pub skipped: usize,
}

/// Prometheus text format 0.0.4, and the OpenMetrics text that shares its syntax (`# EOF`, `# UNIT`, `_created`).
/// A line that cannot be read is counted in `skipped`; the rest are still read.
pub fn parse(text: &str) -> Exposition {
    let mut out = Exposition::default();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut types: HashMap<String, String> = HashMap::new();
    let mut helps: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(comment) = line.strip_prefix('#') {
            let mut parts = comment.trim_start().splitn(3, ' ');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("TYPE"), Some(name), Some(kind)) => {
                    types.insert(name.to_string(), kind.trim().to_ascii_lowercase());
                }
                (Some("HELP"), Some(name), help) => {
                    helps.insert(name.to_string(), unescape(help.unwrap_or("")));
                }
                (Some("EOF"), ..) => break,
                _ => {}
            }
            continue;
        }
        let Some(sample) = sample(line) else {
            out.skipped += 1;
            continue;
        };
        let family = family_name(&sample.name, &types);
        let position = *index.entry(family.clone()).or_insert_with(|| {
            out.families.push(Family {
                kind: types.get(&family).cloned().unwrap_or_else(|| "untyped".into()),
                help: helps.get(&family).cloned().unwrap_or_default(),
                name: family.clone(),
                samples: Vec::new(),
            });
            out.families.len() - 1
        });
        out.families[position].samples.push(sample);
    }
    // A `# TYPE` may follow the first sample in hand-written text; take it where it is known.
    for family in &mut out.families {
        if family.kind == "untyped" {
            if let Some(kind) = types.get(&family.name) {
                family.kind = kind.clone();
            }
        }
        if family.help.is_empty() {
            family.help = helps.get(&family.name).cloned().unwrap_or_default();
        }
    }
    out
}

/// The family a sample belongs to: its own name, or the name without a suffix its family's type gives it.
fn family_name(sample: &str, types: &HashMap<String, String>) -> String {
    if types.contains_key(sample) {
        return sample.to_string();
    }
    for suffix in ["_bucket", "_count", "_sum", "_total", "_created", "_gcount", "_gsum"] {
        if let Some(base) = sample.strip_suffix(suffix) {
            if types.contains_key(base) {
                return base.to_string();
            }
        }
    }
    sample.to_string()
}

fn sample(line: &str) -> Option<Sample> {
    let name_end = line.find(|c: char| c == '{' || c.is_whitespace())?;
    let name = &line[..name_end];
    if name.is_empty()
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
        || name.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    let mut rest = &line[name_end..];
    let mut labels = Vec::new();
    if let Some(after) = rest.strip_prefix('{') {
        let mut chars = after.char_indices().peekable();
        let mut consumed = None;
        loop {
            while chars.peek().is_some_and(|(_, c)| c.is_whitespace() || *c == ',') {
                chars.next();
            }
            match chars.peek() {
                Some((at, '}')) => {
                    consumed = Some(at + 1);
                    break;
                }
                None => break,
                _ => {}
            }
            let mut label = String::new();
            while let Some((_, c)) = chars.peek() {
                if c.is_ascii_alphanumeric() || *c == '_' {
                    label.push(*c);
                    chars.next();
                } else {
                    break;
                }
            }
            while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                chars.next();
            }
            if label.is_empty() || chars.next().map(|(_, c)| c) != Some('=') {
                return None;
            }
            while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                chars.next();
            }
            if chars.next().map(|(_, c)| c) != Some('"') {
                return None;
            }
            let mut value = String::new();
            loop {
                match chars.next()?.1 {
                    '"' => break,
                    '\\' => match chars.next()?.1 {
                        'n' => value.push('\n'),
                        other => value.push(other),
                    },
                    other => value.push(other),
                }
            }
            labels.push((label, value));
        }
        rest = &after[consumed?..];
    }
    let mut fields = rest.split_whitespace();
    let value = number(fields.next()?)?;
    // An optional timestamp may follow; the scrape time is used for every sample so cumulative points stay ordered.
    if let Some(timestamp) = fields.next() {
        if timestamp != "#" {
            timestamp.parse::<f64>().ok()?;
        }
    }
    labels.sort();
    Some(Sample {
        name: name.to_string(),
        labels,
        value,
    })
}

fn number(text: &str) -> Option<f64> {
    match text {
        "+Inf" | "Inf" | "+inf" | "inf" => Some(f64::INFINITY),
        "-Inf" | "-inf" => Some(f64::NEG_INFINITY),
        "NaN" | "nan" => Some(f64::NAN),
        _ => text.parse().ok(),
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// What [`to_otlp`] built: the OTLP JSON request, how many points it holds, and how many were left out (over
/// [`MAX_POINTS`], or a value OTLP cannot carry such as NaN, or a histogram whose buckets do not add up).
pub struct Converted {
    pub request: Json,
    pub points: usize,
    pub skipped: usize,
}

/// A histogram series' `(le, cumulative count)` buckets, `_sum` and `_count`.
type HistogramSamples = (Vec<(f64, f64)>, Option<f64>, Option<f64>);

/// One OTLP metrics request of every family, all points at `time_nanos`. Counters and histograms are cumulative with
/// no start time, so the decoder takes a series' first scrape as its baseline and later scrapes as deltas; a counter
/// that goes down is a reset. A summary is a gauge per quantile plus cumulative `_sum` and `_count` sums, since OTLP
/// summaries are not stored.
pub fn to_otlp(families: &[Family], resource: &[(String, String)], time_nanos: i64) -> Converted {
    let time = time_nanos.to_string();
    let mut metrics = Vec::new();
    let mut points = 0usize;
    let mut skipped = 0usize;
    let mut room = |n: usize, skipped: &mut usize| {
        if points + n > MAX_POINTS {
            *skipped += n;
            false
        } else {
            points += n;
            true
        }
    };
    let point = |labels: &[(String, String)], value: f64| json!({"attributes": attributes(labels), "timeUnixNano": time, "asDouble": value});
    for family in families {
        match family.kind.as_str() {
            "counter" => {
                let mut data = Vec::new();
                for sample in &family.samples {
                    if sample.name.ends_with("_created") {
                        continue;
                    }
                    if !sample.value.is_finite() || sample.value < 0.0 {
                        skipped += 1;
                    } else if room(1, &mut skipped) {
                        data.push(point(&sample.labels, sample.value));
                    }
                }
                let name = family
                    .samples
                    .iter()
                    .find(|sample| !sample.name.ends_with("_created"))
                    .map_or(family.name.as_str(), |sample| sample.name.as_str());
                push_sum(&mut metrics, name, &family.help, data);
            }
            "histogram" => {
                let mut series: BTreeMap<Vec<(String, String)>, HistogramSamples> = BTreeMap::new();
                for sample in &family.samples {
                    let mut labels = sample.labels.clone();
                    let le = labels
                        .iter()
                        .position(|(key, _)| key == "le")
                        .map(|at| labels.remove(at).1);
                    let entry = series.entry(labels).or_default();
                    if sample.name.ends_with("_bucket") {
                        match le.as_deref().and_then(number) {
                            Some(bound) if !bound.is_nan() => entry.0.push((bound, sample.value)),
                            _ => skipped += 1,
                        }
                    } else if sample.name.ends_with("_sum") {
                        entry.1 = Some(sample.value);
                    } else if sample.name.ends_with("_count") {
                        entry.2 = Some(sample.value);
                    }
                }
                let mut data = Vec::new();
                for (labels, (mut buckets, sum, count)) in series {
                    buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let Some(converted) = histogram_point(&buckets, sum, count) else {
                        skipped += 1;
                        continue;
                    };
                    if room(1, &mut skipped) {
                        let (bounds, counts, total, sum) = converted;
                        let mut value = json!({
                            "attributes": attributes(&labels),
                            "timeUnixNano": time,
                            "count": total.to_string(),
                            "explicitBounds": bounds,
                            "bucketCounts": counts.iter().map(u64::to_string).collect::<Vec<_>>(),
                        });
                        if let Some(sum) = sum {
                            value["sum"] = json!(sum);
                        }
                        data.push(value);
                    }
                }
                if !data.is_empty() {
                    metrics.push(json!({"name": family.name, "description": family.help,
                        "histogram": {"aggregationTemporality": 2, "dataPoints": data}}));
                }
            }
            "summary" => {
                let (mut quantiles, mut sums, mut counts) = (Vec::new(), Vec::new(), Vec::new());
                for sample in &family.samples {
                    let target = if sample.name.ends_with("_sum") {
                        &mut sums
                    } else if sample.name.ends_with("_count") {
                        &mut counts
                    } else if sample.name.ends_with("_created") {
                        continue;
                    } else {
                        &mut quantiles
                    };
                    if !sample.value.is_finite() {
                        skipped += 1;
                    } else if room(1, &mut skipped) {
                        target.push(point(&sample.labels, sample.value));
                    }
                }
                push_gauge(&mut metrics, &family.name, &family.help, quantiles);
                push_sum(&mut metrics, &format!("{}_sum", family.name), &family.help, sums);
                push_sum(&mut metrics, &format!("{}_count", family.name), &family.help, counts);
            }
            _ => {
                // gauge, untyped, unknown, info, stateset: each sample is a gauge of its own name.
                let mut by_name: Vec<(&str, Vec<Json>)> = Vec::new();
                for sample in &family.samples {
                    if !sample.value.is_finite() {
                        skipped += 1;
                        continue;
                    }
                    if !room(1, &mut skipped) {
                        continue;
                    }
                    let value = point(&sample.labels, sample.value);
                    match by_name.iter_mut().find(|(name, _)| *name == sample.name) {
                        Some((_, data)) => data.push(value),
                        None => by_name.push((&sample.name, vec![value])),
                    }
                }
                for (name, data) in by_name {
                    push_gauge(&mut metrics, name, &family.help, data);
                }
            }
        }
    }
    Converted {
        request: json!({"resourceMetrics": [{
            "resource": {"attributes": attributes(resource)},
            "scopeMetrics": [{"scope": {"name": "moqui.scrape"}, "metrics": metrics}],
        }]}),
        points,
        skipped,
    }
}

/// Cumulative `le` buckets to OTLP's bounds and per-bucket counts. None when they are not a valid histogram: no `+Inf`
/// bucket, a fractional or decreasing count, or a `_count` other than the `+Inf` bucket.
#[allow(clippy::type_complexity)]
fn histogram_point(
    buckets: &[(f64, f64)],
    sum: Option<f64>,
    count: Option<f64>,
) -> Option<(Vec<f64>, Vec<u64>, u64, Option<f64>)> {
    let (last_bound, total) = *buckets.last()?;
    if last_bound != f64::INFINITY || total.fract() != 0.0 || total < 0.0 {
        return None;
    }
    if count.is_some_and(|count| count != total) {
        return None;
    }
    let mut bounds = Vec::new();
    let mut counts = Vec::new();
    let mut previous = 0.0;
    for (bound, cumulative) in buckets {
        if cumulative.fract() != 0.0 || *cumulative < previous {
            return None;
        }
        if bound.is_finite() {
            if bounds.last().is_some_and(|last| last >= bound) {
                return None;
            }
            bounds.push(*bound);
        }
        counts.push((cumulative - previous) as u64);
        previous = *cumulative;
    }
    Some((bounds, counts, total as u64, sum.filter(|sum| sum.is_finite())))
}

fn push_sum(metrics: &mut Vec<Json>, name: &str, help: &str, data: Vec<Json>) {
    if !data.is_empty() {
        metrics.push(json!({"name": name, "description": help,
            "sum": {"aggregationTemporality": 2, "isMonotonic": true, "dataPoints": data}}));
    }
}

fn push_gauge(metrics: &mut Vec<Json>, name: &str, help: &str, data: Vec<Json>) {
    if !data.is_empty() {
        metrics.push(json!({"name": name, "description": help, "gauge": {"dataPoints": data}}));
    }
}

fn attributes(pairs: &[(String, String)]) -> Json {
    Json::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key, "value": {"stringValue": value}}))
            .collect(),
    )
}

/// When each target and check last ran, by id, so a job each minute runs only those whose interval has passed.
#[derive(Default)]
struct LastRun(Mutex<HashMap<String, Instant>>);

fn due(ec: &ExecutionContext, kind: &str, id: &str, interval: Duration) -> bool {
    let runs = ec.factory().extension(|_| LastRun::default());
    let mut runs = super::lock(&runs.0);
    let key = format!("{kind}:{id}");
    // A little early is on time: a job each minute should run a 60 second target each minute.
    if runs
        .get(&key)
        .is_some_and(|last| last.elapsed() + Duration::from_secs(5) < interval)
    {
        return false;
    }
    runs.insert(key, Instant::now());
    true
}

fn text<'a>(row: &'a crate::entity::EntityValue, key: &str) -> &'a str {
    row.get(key).and_then(Value::as_str).unwrap_or("")
}

fn seconds(row: &crate::entity::EntityValue, key: &str, default: u64, max: u64) -> Duration {
    let value = row
        .get(key)
        .map(to_plain_string)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 1.0)
        .map_or(default, |value| value as u64);
    Duration::from_secs(value.min(max))
}

fn entity(ec: &ExecutionContext) -> Result<crate::entity::EntityFacade, Error> {
    ec.factory()
        .entity()
        .ok_or_else(|| Error::new("entity facade is unavailable"))
}

/// Due enabled rows of `entity_name`, or just the one named by `id_field` in `parameters` whatever its schedule.
fn rows(
    ec: &ExecutionContext,
    entity_name: &str,
    id_field: &str,
    parameters: &Parameters,
) -> Result<(Vec<crate::entity::EntityValue>, bool), Error> {
    let only = parameters
        .get(id_field)
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    let mut find = entity(ec)?.find(entity_name)?.use_cache(false);
    if let Some(id) = only {
        find = find.eq(id_field, Value::string(id));
    }
    let rows = find.list()?.into_values();
    if only.is_some() && rows.is_empty() {
        return Err(Error::new(format!("{entity_name} {} not found", only.unwrap_or(""))));
    }
    Ok((rows, only.is_some()))
}

fn record(ec: &ExecutionContext, mut row: crate::entity::EntityValue, fields: Vec<(&str, Value)>) {
    for (key, value) in fields {
        row.set(key, value);
    }
    if let Err(error) = entity(ec).and_then(|facade| row.update(&facade)) {
        tracing::error!(entity = %row.entity_name(), %error, "could not record the pull result, continuing");
    }
}

fn now_nanos() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as i64)
}

/// The URL's `host:port`, for `service.instance.id` and a default `service.name`.
fn authority(url: &str) -> String {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@')
        .next()
        .unwrap_or("")
        .to_string()
}

/// One scrape: the body's families, the scrape's own `up`, `scrape_duration_seconds` and `scrape_samples_scraped`
/// gauges, then decode and queue. Returns (points written, points skipped, status text).
fn scrape_one(ec: &ExecutionContext, row: &crate::entity::EntityValue) -> Result<(usize, usize, String), Error> {
    let target_id = text(row, "targetId");
    let url = text(row, "url");
    let mut headers = vec![(
        "Accept".to_string(),
        "text/plain;version=0.0.4;q=0.9,application/openmetrics-text;q=0.5,*/*;q=0.1".to_string(),
    )];
    let token = text(row, "bearerToken");
    if !token.is_empty() {
        headers.push(("Authorization".into(), format!("Bearer {token}")));
    }
    let started = Instant::now();
    let response = crate::outbound::send(crate::outbound::Request {
        method: crate::outbound::Method::Get,
        url: url.to_string(),
        headers,
        body: None,
        timeout: Some(seconds(row, "timeoutSeconds", 10, 60)),
        // Targets are set by a monitoring administrator, and an exporter on this host or network is the usual case.
        deny_internal: false,
    });
    let elapsed = started.elapsed().as_secs_f64();
    let (mut exposition, failure) = match &response {
        Ok(response) if !(200..300).contains(&response.status) => {
            (Exposition::default(), Some(format!("HTTP {}", response.status)))
        }
        Ok(response) if response.body.len() > MAX_BODY => (
            Exposition::default(),
            Some(format!("body over {} MiB", MAX_BODY / 1024 / 1024)),
        ),
        Ok(response) => (parse(&response.body), None),
        Err(error) => (Exposition::default(), Some(error.message().to_string())),
    };
    let scraped: usize = exposition.families.iter().map(|family| family.samples.len()).sum();
    for (name, value) in [
        ("up", if failure.is_some() { 0.0 } else { 1.0 }),
        ("scrape_duration_seconds", elapsed),
        ("scrape_samples_scraped", scraped as f64),
    ] {
        exposition.families.push(Family {
            name: name.into(),
            kind: "gauge".into(),
            help: String::new(),
            samples: vec![Sample {
                name: name.into(),
                labels: Vec::new(),
                value,
            }],
        });
    }
    let instance = authority(url);
    let service_name = match text(row, "serviceName") {
        "" => instance.clone(),
        name => name.to_string(),
    };
    let resource = vec![
        ("service.name".to_string(), service_name),
        ("service.instance.id".to_string(), instance),
        ("scrape.target.id".to_string(), target_id.to_string()),
    ];
    let converted = to_otlp(&exposition.families, &resource, now_nanos());
    let telemetry = super::for_factory(ec.factory());
    let decoder = ec.factory().extension(|_| Decoder::default());
    let body = serde_json::to_vec(&converted.request).map_err(|error| Error::new(error.to_string()))?;
    let source_id = format!("scrape:{target_id}");
    let decoded = decoder.decode_scoped(Signal::Metrics, "application/json", &body, &source_id, target_id)?;
    // A first scrape is every cumulative series' baseline: not data lost.
    telemetry.note_dropped(decoded.rejected.saturating_sub(decoded.baselines));
    let written = decoded.documents.len();
    telemetry.try_ingest(decoded.index, decoded.documents)?;
    let skipped = converted.skipped + exposition.skipped;
    match failure {
        Some(failure) => Err(Error::new(failure)),
        None => Ok((
            written,
            skipped,
            format!("{scraped} samples, {written} written, {skipped} skipped"),
        )),
    }
}

/// Scrape each enabled target whose interval has passed (or `targetId` now, enabled or not).
pub fn scrape_targets(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, Error> {
    let (rows, forced) = rows(ec, TARGETS, "targetId", parameters)?;
    let (mut scraped, mut failed, mut written) = (0i64, 0i64, 0i64);
    for row in rows {
        let target_id = text(&row, "targetId").to_string();
        if !forced
            && (text(&row, "enabled") == "N"
                || !due(ec, "scrape", &target_id, seconds(&row, "intervalSeconds", 60, 86_400)))
        {
            continue;
        }
        if forced {
            due(ec, "scrape", &target_id, Duration::ZERO);
        }
        scraped += 1;
        let now = Value::I64(crate::user::now_millis());
        match scrape_one(ec, &row) {
            Ok((points, _, status)) => {
                written += points as i64;
                record(
                    ec,
                    row,
                    vec![
                        ("lastScrapeDate", now),
                        ("lastStatus", Value::string("ok")),
                        ("lastSamples", Value::I64(points as i64)),
                        ("lastError", Value::string(status)),
                    ],
                );
            }
            Err(error) => {
                failed += 1;
                tracing::error!(target = %target_id, %error, "telemetry scrape failed, continuing");
                record(
                    ec,
                    row,
                    vec![
                        ("lastScrapeDate", now),
                        ("lastStatus", Value::string("failed")),
                        ("lastError", Value::string(error.message())),
                    ],
                );
            }
        }
    }
    Ok(BTreeMap::from([
        ("scraped".into(), Value::I64(scraped)),
        ("failed".into(), Value::I64(failed)),
        ("written".into(), Value::I64(written)),
    ]))
}

/// One check as a root span of its own trace (kept whatever the sample ratio), with the request as its CLIENT child so
/// a target that reads `traceparent` joins the trace. Returns the status text, or the failure.
fn check_one(ec: &ExecutionContext, row: &crate::entity::EntityValue) -> (Result<String, String>, Option<String>) {
    let check_id = text(row, "checkId");
    let name = match text(row, "name") {
        "" => check_id,
        name => name,
    };
    let url = text(row, "url");
    let telemetry = super::for_factory(ec.factory());
    let _root = super::attach(None);
    let mut span = telemetry.start_sampled_root(&format!("check {name}"), SpanKind::Internal);
    span.set_attribute("moqui.synthetic.check", Value::Bool(true));
    span.set_attribute("synthetic.check.id", Value::string(check_id));
    span.set_attribute("synthetic.check.name", Value::string(name));
    span.set_attribute("url.full", Value::string(crate::outbound::redacted(url)));
    let trace_id = span.context().map(|context| context.trace_id.clone());
    let method = match text(row, "method").to_ascii_uppercase().as_str() {
        "HEAD" => crate::outbound::Method::Head,
        "POST" => crate::outbound::Method::Post,
        _ => crate::outbound::Method::Get,
    };
    let expect_status = row
        .get("expectStatus")
        .map(to_plain_string)
        .and_then(|value| value.parse::<f64>().ok())
        .map_or(200, |value| value as u16);
    let expect_body = text(row, "expectBodyContains");
    let started = Instant::now();
    let response = {
        let _child = super::attach(span.context());
        crate::outbound::send(crate::outbound::Request {
            method,
            url: url.to_string(),
            headers: Vec::new(),
            body: None,
            timeout: Some(seconds(row, "timeoutSeconds", 10, 60)),
            deny_internal: false,
        })
    };
    let millis = started.elapsed().as_millis();
    let result = match response {
        Err(error) => Err(error.message().to_string()),
        Ok(response) => {
            span.set_attribute("http.response.status_code", Value::I64(i64::from(response.status)));
            if response.status != expect_status {
                Err(format!("HTTP {} (expected {expect_status})", response.status))
            } else if !expect_body.is_empty() && !response.body.contains(expect_body) {
                Err(format!("body does not contain {expect_body:?}"))
            } else {
                Ok(format!("HTTP {} in {millis} ms", response.status))
            }
        }
    };
    if let Err(failure) = &result {
        span.set_error(failure);
    }
    span.finish();
    (result, trace_id)
}

/// Run each enabled check whose interval has passed (or `checkId` now, enabled or not).
pub fn run_checks(
    ec: &ExecutionContext,
    _service: &ServiceInfo<'_>,
    parameters: &Parameters,
) -> Result<Parameters, Error> {
    let (rows, forced) = rows(ec, CHECKS, "checkId", parameters)?;
    let (mut run, mut failed) = (0i64, 0i64);
    let mut last_trace = None;
    for row in rows {
        let check_id = text(&row, "checkId").to_string();
        if !forced
            && (text(&row, "enabled") == "N"
                || !due(ec, "check", &check_id, seconds(&row, "intervalSeconds", 60, 86_400)))
        {
            continue;
        }
        if forced {
            due(ec, "check", &check_id, Duration::ZERO);
        }
        run += 1;
        let (result, trace_id) = check_one(ec, &row);
        let now = Value::I64(crate::user::now_millis());
        let trace = Value::string(trace_id.clone().unwrap_or_default());
        last_trace = trace_id;
        match result {
            Ok(status) => record(
                ec,
                row,
                vec![
                    ("lastCheckDate", now),
                    ("lastStatus", Value::string("ok")),
                    ("lastMessage", Value::string(status)),
                    ("lastTraceId", trace),
                ],
            ),
            Err(failure) => {
                failed += 1;
                tracing::error!(check = %check_id, error = %failure, "synthetic check failed, continuing");
                record(
                    ec,
                    row,
                    vec![
                        ("lastCheckDate", now),
                        ("lastStatus", Value::string("failed")),
                        ("lastMessage", Value::string(failure)),
                        ("lastTraceId", trace),
                    ],
                );
            }
        }
    }
    let mut out = BTreeMap::from([("run".into(), Value::I64(run)), ("failed".into(), Value::I64(failed))]);
    if forced {
        out.insert("traceId".into(), Value::string(last_trace.unwrap_or_default()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"# HELP http_requests_total Requests "served".
# TYPE http_requests_total counter
http_requests_total{method="get",code="200"} 1027 1395066363000
http_requests_total{method="post",code="400"}    3
http_requests_created{method="get",code="200"} 1395066363
# TYPE temperature gauge
temperature{room="a \"b\" \\ c\nd"} -2.5
# TYPE latency histogram
latency_bucket{le="0.1"} 2
latency_bucket{le="0.5"} 5
latency_bucket{le="+Inf"} 6
latency_sum 1.75
latency_count 6
# TYPE rpc summary
rpc{quantile="0.5"} 0.2
rpc{quantile="0.9"} NaN
rpc_sum 17
rpc_count 40
no_type_here 4
bad line
# EOF
after_eof 1
"#;

    #[test]
    fn parses_every_type_and_skips_bad_lines() {
        let parsed = parse(TEXT);
        assert_eq!(parsed.skipped, 1);
        let names: Vec<_> = parsed
            .families
            .iter()
            .map(|family| (family.name.as_str(), family.kind.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("http_requests_total", "counter"),
                ("http_requests_created", "untyped"),
                ("temperature", "gauge"),
                ("latency", "histogram"),
                ("rpc", "summary"),
                ("no_type_here", "untyped"),
            ]
        );
        assert_eq!(parsed.families[0].help, "Requests \"served\".");
        assert_eq!(
            parsed.families[2].samples[0].labels,
            [("room".into(), "a \"b\" \\ c\nd".into())]
        );
        assert_eq!(parsed.families[2].samples[0].value, -2.5);
        assert_eq!(parsed.families[3].samples.len(), 5);
        assert!(parsed.families[4].samples[1].value.is_nan());
    }

    #[test]
    fn openmetrics_counter_family_takes_total_and_created_samples() {
        let parsed = parse("# TYPE jobs counter\njobs_total 4\njobs_created 1.7e9\n# EOF\n");
        assert_eq!(parsed.families.len(), 1);
        let converted = to_otlp(&parsed.families, &[], 5);
        let metric = &converted.request["resourceMetrics"][0]["scopeMetrics"][0]["metrics"][0];
        assert_eq!(metric["name"], "jobs_total");
        assert_eq!(metric["sum"]["dataPoints"].as_array().unwrap().len(), 1);
        assert_eq!(metric["sum"]["isMonotonic"], true);
    }

    #[test]
    fn histogram_buckets_become_per_bucket_counts() {
        let parsed = parse(TEXT);
        let converted = to_otlp(&parsed.families, &[("service.name".into(), "x".into())], 1_000);
        let metrics = converted.request["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
            .as_array()
            .unwrap()
            .clone();
        let latency = metrics.iter().find(|metric| metric["name"] == "latency").unwrap();
        let point = &latency["histogram"]["dataPoints"][0];
        assert_eq!(point["explicitBounds"], json!([0.1, 0.5]));
        assert_eq!(point["bucketCounts"], json!(["2", "3", "1"]));
        assert_eq!(point["count"], "6");
        assert_eq!(point["sum"], json!(1.75));
        // The NaN quantile is skipped; the other quantile and the sum and count remain.
        assert_eq!(converted.skipped, 1);
        assert!(metrics
            .iter()
            .any(|metric| metric["name"] == "rpc_count" && !metric["sum"].is_null()));
        assert_eq!(
            metrics.iter().find(|metric| metric["name"] == "rpc").unwrap()["gauge"]["dataPoints"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn invalid_histograms_are_skipped() {
        assert!(histogram_point(&[(1.0, 3.0), (f64::INFINITY, 2.0)], None, None).is_none());
        assert!(histogram_point(&[(1.0, 3.0)], None, None).is_none());
        assert!(histogram_point(&[(1.0, 1.0), (f64::INFINITY, 2.0)], None, Some(3.0)).is_none());
        assert_eq!(
            histogram_point(&[(f64::INFINITY, 2.0)], Some(f64::NAN), None),
            Some((vec![], vec![2], 2, None))
        );
    }

    #[test]
    fn scrapes_become_deltas_and_first_scrape_is_a_baseline_not_a_loss() {
        let decoder = Decoder::default();
        let decode = |text: &str, time: i64| {
            let converted = to_otlp(
                &parse(text).families,
                &[("service.name".into(), "exporter".into())],
                time,
            );
            let body = serde_json::to_vec(&converted.request).unwrap();
            decoder
                .decode_scoped(Signal::Metrics, "application/json", &body, "scrape:t1", "t1")
                .unwrap()
        };
        let text = |count: u64, bucket: u64| {
            format!(
                "# TYPE c counter\nc_total {count}\n# TYPE g gauge\ng 7\n# TYPE h histogram\nh_bucket{{le=\"1\"}} {bucket}\nh_bucket{{le=\"+Inf\"}} {bucket}\nh_sum {bucket}\nh_count {bucket}\n"
            )
        };
        let first = decode(&text(100, 10), 60_000_000_000);
        // The gauge is written; the counter and histogram are baselines.
        assert_eq!(first.documents.len(), 1);
        assert_eq!(first.rejected, 2);
        assert_eq!(first.baselines, 2);
        let second = decode(&text(130, 14), 120_000_000_000);
        assert_eq!(second.rejected, 0);
        let counter = second
            .documents
            .iter()
            .find(|document| document.get("name").and_then(Value::as_str) == Some("c_total"))
            .unwrap();
        assert_eq!(counter.get("sum").map(to_plain_string).as_deref(), Some("30"));
        let histogram = second
            .documents
            .iter()
            .find(|document| document.get("name").and_then(Value::as_str) == Some("h"))
            .unwrap();
        assert_eq!(histogram.get("count").map(to_plain_string).as_deref(), Some("4"));
        assert_eq!(histogram.get("source.id").and_then(Value::as_str), Some("scrape:t1"));
        // A counter that went down is a reset: a new baseline, still not a loss.
        let third = decode(&text(5, 15), 180_000_000_000);
        assert_eq!(third.rejected, third.baselines);
    }

    #[test]
    fn caps_points_per_scrape() {
        let mut text = String::from("# TYPE g gauge\n");
        for n in 0..(MAX_POINTS + 5) {
            text.push_str(&format!("g{{n=\"{n}\"}} 1\n"));
        }
        let converted = to_otlp(&parse(&text).families, &[], 1);
        assert_eq!(converted.points, MAX_POINTS);
        assert_eq!(converted.skipped, 5);
    }

    #[test]
    fn authority_drops_credentials_and_path() {
        assert_eq!(authority("http://u:p@host:9100/metrics?x=1"), "host:9100");
        assert_eq!(authority("https://example.com"), "example.com");
    }
}
