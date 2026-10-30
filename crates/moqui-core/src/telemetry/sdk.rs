//! The only place business telemetry sees OpenTelemetry SDK types.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::{
    Span as _, SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState, Tracer, TracerProvider,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider, SpanData, SpanExporter};

use super::{lock, LogRecord, Shared, Span, SpanKind, TraceContext};
use crate::entity::document::Document;
use moqui_lib::object::Value;

const ARTIFACT_DURATION: &str = "moqui.artifact.duration";
const DB_DURATION: &str = "db.client.operation.duration";
const HTTP_DURATION: &str = "http.server.request.duration";

/// Durations in seconds, with the semantic-convention boundaries, so documents of one series merge.
fn duration_histogram(meter: &opentelemetry::metrics::Meter, name: &str) -> opentelemetry::metrics::Histogram<f64> {
    meter
        .f64_histogram(name.to_string())
        .with_unit("s")
        .with_boundaries(vec![
            0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
        ])
        .build()
}

pub(super) struct Runtime {
    provider: SdkTracerProvider,
    tracer: SdkTracer,
    shared: Arc<Shared>,
    meter: opentelemetry::metrics::Meter,
    meter_provider: opentelemetry_sdk::metrics::SdkMeterProvider,
    /// [`ARTIFACT_DURATION`], [`DB_DURATION`] and [`HTTP_DURATION`].
    hot: [opentelemetry::metrics::Histogram<f64>; 3],
    histograms: Mutex<HashMap<String, opentelemetry::metrics::Histogram<f64>>>,
    counters: Mutex<HashMap<String, opentelemetry::metrics::Counter<u64>>>,
    logger_provider: opentelemetry_sdk::logs::SdkLoggerProvider,
    logger: opentelemetry_sdk::logs::SdkLogger,
    gauges: Mutex<Vec<opentelemetry::metrics::ObservableGauge<f64>>>,
    /// The cumulative reader behind [`Self::prometheus_text`], which collects only when asked.
    pull: Arc<opentelemetry_sdk::metrics::ManualReader>,
    /// The configured attribute allow-lists by instrument (telemetry/metric), in place of [`ALLOWED_ATTRIBUTES`].
    allowed: HashMap<String, Vec<opentelemetry::Key>>,
}

/// A reader the meter provider owns that the runtime can still collect from.
#[derive(Debug)]
struct SharedReader(Arc<opentelemetry_sdk::metrics::ManualReader>);

impl opentelemetry_sdk::metrics::reader::MetricReader for SharedReader {
    fn register_pipeline(&self, pipeline: std::sync::Weak<opentelemetry_sdk::metrics::Pipeline>) {
        self.0.register_pipeline(pipeline);
    }
    fn collect(&self, metrics: &mut opentelemetry_sdk::metrics::data::ResourceMetrics) -> OTelSdkResult {
        self.0.collect(metrics)
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.0.force_flush()
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.0.shutdown_with_timeout(timeout)
    }
    fn temporality(&self, kind: opentelemetry_sdk::metrics::InstrumentKind) -> opentelemetry_sdk::metrics::Temporality {
        self.0.temporality(kind)
    }
}

impl Runtime {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        let current_resource = lock(&shared.resource).clone();
        let resource = opentelemetry_sdk::Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", current_resource.service_name),
                KeyValue::new("service.instance.id", current_resource.instance_id),
                KeyValue::new("service.version", current_resource.version),
                KeyValue::new("vcs.ref.head.revision", current_resource.revision),
            ])
            .build();
        // These exporters only normalize and try-enqueue in memory. The core worker batches
        // transport, so SDK pre-export queues cannot hide losses or discard request roots.
        let provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            .with_simple_exporter(LocalSpanExporter(Arc::clone(&shared)))
            .build();
        let tracer = provider.tracer("moqui");
        let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(LocalMetricExporter(Arc::clone(&shared)))
            .with_interval(shared.config.interval)
            .build();
        // Scrapes read totals since start, apart from the minute deltas the periodic reader writes.
        let pull = Arc::new(
            opentelemetry_sdk::metrics::ManualReader::builder()
                .with_temporality(opentelemetry_sdk::metrics::Temporality::Cumulative)
                .build(),
        );
        let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_view(|_: &opentelemetry_sdk::metrics::Instrument| {
                opentelemetry_sdk::metrics::Stream::builder()
                    .with_cardinality_limit(super::MAX_SERIES)
                    .build()
                    .ok()
            })
            .with_reader(reader)
            .with_reader(SharedReader(Arc::clone(&pull)))
            .build();
        use opentelemetry::metrics::MeterProvider;
        let meter = meter_provider.meter("moqui");
        let hot = [ARTIFACT_DURATION, DB_DURATION, HTTP_DURATION].map(|name| duration_histogram(&meter, name));
        let logger_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_simple_exporter(LocalLogExporter(Arc::clone(&shared)))
            .build();
        use opentelemetry::logs::LoggerProvider;
        let logger = logger_provider.logger("moqui");
        let allowed = shared
            .config
            .metric_attributes
            .iter()
            .map(|(name, keys)| {
                let keys = keys
                    .iter()
                    .map(|key| opentelemetry::Key::from(Arc::<str>::from(key.as_str())))
                    .collect();
                (name.clone(), keys)
            })
            .collect();
        Self {
            provider,
            tracer,
            shared,
            meter,
            meter_provider,
            hot,
            histograms: Mutex::default(),
            counters: Mutex::default(),
            logger_provider,
            logger,
            gauges: Mutex::default(),
            pull,
            allowed,
        }
    }

    /// Collect the cumulative reader and render it for a Prometheus scrape.
    pub(super) fn prometheus_text(&self) -> Option<String> {
        use opentelemetry_sdk::metrics::reader::MetricReader;
        let mut metrics = opentelemetry_sdk::metrics::data::ResourceMetrics::default();
        if let Err(error) = self.pull.collect(&mut metrics) {
            tracing::debug!(%error, "telemetry metrics could not be collected for a scrape");
            return None;
        }
        let request = opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::from(&metrics);
        Some(super::prometheus::render(&request, &lock(&self.shared.resource)))
    }

    /// The attributes `name` keeps: its configured list, or the global one.
    fn metric_attributes(&self, name: &str, attributes: Document) -> Vec<KeyValue> {
        match self.allowed.get(name) {
            Some(keys) => attributes
                .into_iter()
                .filter_map(|(key, value)| {
                    keys.iter()
                        .find(|allowed| allowed.as_str() == key)
                        .map(|allowed| KeyValue::new(allowed.clone(), metric_value(&key, value)))
                })
                .collect(),
            None => metric_attributes(attributes),
        }
    }

    pub(super) fn start_span(
        &self,
        name: &str,
        kind: SpanKind,
        parent: Option<TraceContext>,
        started: Option<SystemTime>,
        retain: bool,
        sample_ratio: f64,
    ) -> Span {
        let parent_context = parent.as_ref().map_or_else(Context::new, sdk_context);
        let mut builder = self.tracer.span_builder(name.to_string());
        builder.start_time = started;
        builder.span_kind = Some(match kind {
            SpanKind::Server => opentelemetry::trace::SpanKind::Server,
            SpanKind::Client => opentelemetry::trace::SpanKind::Client,
            SpanKind::Internal => opentelemetry::trace::SpanKind::Internal,
            SpanKind::Producer => opentelemetry::trace::SpanKind::Producer,
            SpanKind::Consumer => opentelemetry::trace::SpanKind::Consumer,
        });
        let mut span = self.tracer.build_with_context(builder, &parent_context);
        let span_context = span.span_context();
        let trace_id = span_context.trace_id().to_string();
        // The configured ratio decides from the trace id, not a caller's sampled flag, so a client cannot turn full
        // tracing on, and every span of a trace (and every service with the same ratio) decides alike.
        let sampled = retain || {
            let value = trace_id
                .get(..16)
                .and_then(|high| u64::from_str_radix(high, 16).ok())
                .unwrap_or(0);
            (value as f64 / u64::MAX as f64) < sample_ratio
        };
        let context = TraceContext {
            trace_id,
            span_id: span_context.span_id().to_string(),
            sampled,
            tracestate: parent.map_or_else(String::new, |parent| parent.tracestate),
            local: true,
        };
        span.set_attribute(KeyValue::new("moqui.head.sampled", sampled));
        Span {
            context: Some(context),
            inner: Some(OwnedSpan {
                span,
                suppressed: false,
            }),
        }
    }

    pub(super) fn record_duration(&self, name: &str, seconds: f64, attributes: Document) {
        if !seconds.is_finite() || seconds < 0.0 {
            return;
        }
        // The instruments every frame, statement and request records are fields, without the shared map's lock.
        let hot = match name {
            ARTIFACT_DURATION => Some(&self.hot[0]),
            DB_DURATION => Some(&self.hot[1]),
            HTTP_DURATION => Some(&self.hot[2]),
            _ => None,
        };
        if let Some(instrument) = hot {
            instrument.record(seconds, &self.metric_attributes(name, attributes));
            return;
        }
        let instrument = {
            let mut histograms = lock(&self.histograms);
            if let Some(instrument) = histograms.get(name) {
                instrument.clone()
            } else {
                if histograms.len() >= 32 {
                    return;
                }
                let instrument = duration_histogram(&self.meter, name);
                histograms.insert(name.to_string(), instrument.clone());
                instrument
            }
        };
        instrument.record(seconds, &self.metric_attributes(name, attributes));
    }

    pub(super) fn add_counter(&self, name: &str, value: u64, attributes: Document) {
        let instrument = {
            let mut counters = lock(&self.counters);
            if let Some(instrument) = counters.get(name) {
                instrument.clone()
            } else {
                if counters.len() >= 32 {
                    return;
                }
                let instrument = self.meter.u64_counter(name.to_string()).build();
                counters.insert(name.to_string(), instrument.clone());
                instrument
            }
        };
        instrument.add(value, &self.metric_attributes(name, attributes));
    }

    pub(super) fn observe_gauge(&self, name: &str, callback: impl Fn() -> Option<f64> + Send + Sync + 'static) {
        let mut gauges = lock(&self.gauges);
        if gauges.len() >= 64 {
            return;
        }
        let gauge_name = name.to_string();
        let failed = std::sync::atomic::AtomicBool::new(false);
        gauges.push(
            self.meter
                .f64_observable_gauge(name.to_string())
                .with_callback(move |observer| {
                    let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&callback)) {
                        Ok(value) => value,
                        Err(_) => {
                            if !failed.swap(true, std::sync::atomic::Ordering::Relaxed) {
                                tracing::warn!(instrument = %gauge_name,"telemetry gauge callback panicked");
                            }
                            None
                        }
                    };
                    if let Some(value) = value.filter(|value| value.is_finite()) {
                        observer.observe(value, &[]);
                    }
                })
                .build(),
        );
    }

    pub(super) fn record_log(&self, input: LogRecord) {
        use opentelemetry::logs::{LogRecord as _, Logger, Severity};
        let mut record = self.logger.create_log_record();
        record.set_timestamp(UNIX_EPOCH + Duration::from_millis(input.timestamp.max(0) as u64));
        record.set_severity_number(match input.severity_number {
            21.. => Severity::Fatal,
            17..=20 => Severity::Error,
            13..=16 => Severity::Warn,
            9..=12 => Severity::Info,
            5..=8 => Severity::Debug,
            _ => Severity::Trace,
        });
        record.set_severity_text(match input.severity_number {
            21.. => "FATAL",
            17..=20 => "ERROR",
            13..=16 => "WARN",
            9..=12 => "INFO",
            5..=8 => "DEBUG",
            _ => "TRACE",
        });
        let body = super::redact_owned(clip_owned(input.body, 16_384));
        record.set_body(opentelemetry::logs::AnyValue::String(body.into()));
        if let Some(context) = input.context {
            if let (Ok(trace), Ok(span)) = (TraceId::from_hex(&context.trace_id), SpanId::from_hex(&context.span_id)) {
                record.set_trace_context(
                    trace,
                    span,
                    Some(if context.sampled {
                        TraceFlags::SAMPLED
                    } else {
                        TraceFlags::default()
                    }),
                );
            }
        }
        record.add_attribute("scope.name", input.scope_name);
        for (key, value) in input
            .attributes
            .into_iter()
            .filter(|(key, _)| !super::sensitive_key(key))
        {
            record.add_attribute(opentelemetry::Key::new(key), log_value(value));
        }
        self.logger.emit(record);
    }

    pub(super) fn flush(&self) {
        let _ = self.provider.force_flush();
        let _ = self.meter_provider.force_flush();
        let _ = self.logger_provider.force_flush();
    }
    pub(super) fn shutdown(&self) {
        let _ = self.provider.shutdown();
        let _ = self.meter_provider.shutdown();
        let _ = self.logger_provider.shutdown();
    }
}

fn sdk_context(context: &TraceContext) -> Context {
    let span_context = SpanContext::new(
        TraceId::from_hex(&context.trace_id).unwrap_or(TraceId::INVALID),
        SpanId::from_hex(&context.span_id).unwrap_or(SpanId::INVALID),
        if context.sampled {
            TraceFlags::SAMPLED
        } else {
            TraceFlags::default()
        },
        true,
        context.tracestate.parse::<TraceState>().unwrap_or_default(),
    );
    Context::new().with_remote_span_context(span_context)
}

pub(super) struct OwnedSpan {
    span: opentelemetry_sdk::trace::Span,
    suppressed: bool,
}

impl OwnedSpan {
    pub(super) fn set_attribute(&mut self, key: &str, value: Value) {
        let safe = safe_key(key);
        if !safe && super::sensitive_key(key) {
            return;
        }
        let value = match value {
            Value::String(value) if safe => Value::String(clip_owned(value, 4096)),
            Value::String(value) => Value::String(super::redact_owned(clip_owned(value, 4096))),
            value => value,
        };
        self.span
            .set_attribute(KeyValue::new(attribute_key(key), sdk_value(value)));
    }
    pub(super) fn set_error(&mut self, message: &str) {
        let message = super::redact_text(&clip_text(message, 4096));
        let message = message.as_str();
        self.span
            .set_status(opentelemetry::trace::Status::error(message.to_string()));
        self.span.set_attribute(KeyValue::new(
            "exception.message",
            message.chars().take(4096).collect::<String>(),
        ));
        self.span.add_event(
            "exception",
            vec![KeyValue::new(
                "exception.message",
                message.chars().take(4096).collect::<String>(),
            )],
        );
    }
    pub(super) fn suppress(&mut self) {
        self.suppressed = true;
        self.span.set_attribute(KeyValue::new("moqui.suppressed", true));
    }
    pub(super) fn finish(mut self) {
        self.span.end();
    }
}

impl Drop for OwnedSpan {
    fn drop(&mut self) {
        if std::thread::panicking() && !self.suppressed {
            self.set_error("execution panicked");
        }
        self.span.end();
    }
}

/// Keys whose values the framework sets from artifact names, ids and routes, never from a caller's text, so neither the
/// secret-name rule nor the credential pattern needs to run on them for every frame.
fn safe_key(key: &str) -> bool {
    matches!(
        key,
        "moqui.artifact.type"
            | "moqui.artifact.name"
            | "moqui.artifact.action"
            | "moqui.artifact.sub_type"
            | "user.id"
            | "moqui.visit.id"
            | "http.route"
    )
}

fn attribute_key(key: &str) -> opentelemetry::Key {
    macro_rules! keys {($($name:literal),*$(,)?)=>{match key {$($name=>opentelemetry::Key::from_static_str($name),)*_=>opentelemetry::Key::new(key.to_string())}};}
    keys!(
        "moqui.artifact.type",
        "moqui.artifact.name",
        "moqui.artifact.action",
        "moqui.artifact.sub_type",
        "moqui.artifact.slow",
        "user.id",
        "moqui.visit.id",
        "http.request.method",
        "http.route",
        "http.response.status_code",
        "db.system.name",
        "db.query.text",
        "moqui.db.system.uncertain",
        "url.full",
        "outcome",
        "moqui.job.root",
        "moqui.job.name",
        "moqui.job.run.id",
        "moqui.synthetic.check",
        "synthetic.check.id",
        "synthetic.check.name",
        "gen_ai.operation.name",
        "gen_ai.provider.name",
        "gen_ai.request.model",
        "gen_ai.response.model",
        "gen_ai.usage.input_tokens",
        "gen_ai.usage.output_tokens",
        "gen_ai.response.finish_reasons",
        "exception.message"
    )
}

fn clip_owned(mut value: String, limit: usize) -> String {
    if value.len() > limit {
        if let Some((boundary, _)) = value.char_indices().nth(limit) {
            value.truncate(boundary);
            value.push_str(" [truncated]");
        }
    }
    value
}

fn sdk_value(value: Value) -> opentelemetry::Value {
    match value {
        Value::Bool(value) => value.into(),
        Value::String(value) => value.into(),
        Value::I32(value) => i64::from(value).into(),
        Value::I64(value) => value.into(),
        Value::F64(value) => value.into(),
        Value::List(values) if values.iter().all(|value| matches!(value, Value::String(_))) => {
            opentelemetry::Value::Array(opentelemetry::Array::String(
                values
                    .into_iter()
                    .map(|value| moqui_lib::object::to_plain_string(&value).into())
                    .collect(),
            ))
        }
        _ => moqui_lib::object::to_plain_string(&value).into(),
    }
}
fn core_value(value: &opentelemetry::Value) -> Value {
    match value {
        opentelemetry::Value::Bool(value) => Value::Bool(*value),
        opentelemetry::Value::I64(value) => Value::I64(*value),
        opentelemetry::Value::F64(value) => Value::F64(*value),
        opentelemetry::Value::String(value) => Value::string(value.as_str()),
        opentelemetry::Value::Array(opentelemetry::Array::String(values)) => {
            Value::List(values.iter().map(|value| Value::string(value.as_str())).collect())
        }
        _ => Value::string(value.to_string()),
    }
}
fn log_value(value: Value) -> opentelemetry::logs::AnyValue {
    match value {
        Value::Bool(value) => value.into(),
        Value::I32(value) => i64::from(value).into(),
        Value::I64(value) => value.into(),
        Value::F64(value) => value.into(),
        Value::List(values) => {
            opentelemetry::logs::AnyValue::ListAny(Box::new(values.into_iter().map(log_value).collect()))
        }
        Value::Map(values) => opentelemetry::logs::AnyValue::Map(Box::new(
            values
                .into_iter()
                .filter(|(key, _)| !super::sensitive_key(key))
                .map(|(key, value)| (opentelemetry::Key::new(key), log_value(value)))
                .collect(),
        )),
        _ => super::redact_text(&clip_text(&moqui_lib::object::to_plain_string(&value), 16_384)).into(),
    }
}

fn clip_text(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let mut clipped: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        clipped.push_str(" [truncated]");
    }
    clipped
}
/// The attributes a metric keeps unless its instrument has its own list (telemetry/metric attributes).
const ALLOWED_ATTRIBUTES: &[&str] = &[
    "moqui.artifact.type",
    "moqui.artifact.name",
    "moqui.artifact.action",
    "moqui.artifact.sub_type",
    "http.request.method",
    "http.route",
    "http.response.status_code",
    "db.system.name",
    "outcome",
];

fn metric_attributes(attributes: Document) -> Vec<KeyValue> {
    attributes
        .into_iter()
        .filter_map(|(key, value)| {
            ALLOWED_ATTRIBUTES
                .iter()
                .find(|allowed| **allowed == key)
                .map(|allowed| KeyValue::new(*allowed, metric_value(allowed, value)))
        })
        .collect()
}

/// A metric attribute's value, redacted unless the framework sets it from names and routes.
fn metric_value(key: &str, value: Value) -> opentelemetry::Value {
    sdk_value(match value {
        Value::String(value) if !safe_key(key) => Value::String(super::redact_owned(value)),
        other => other,
    })
}

/// Owned SDK payloads cross only the bounded core queue. Formatting, JSON-compatible
/// values, resource flattening, and tail decisions happen on its worker.
pub(super) enum ExportBatch {
    Spans(Vec<SpanData>),
    Logs(
        Vec<(
            opentelemetry_sdk::logs::SdkLogRecord,
            opentelemetry::InstrumentationScope,
        )>,
    ),
}
fn mandatory_root(span: &SpanData) -> bool {
    span.span_kind == opentelemetry::trace::SpanKind::Server
        || span.attributes.iter().any(|attribute| {
            matches!(attribute.key.as_str(), "moqui.job.root" | "moqui.synthetic.check")
                && (attribute.value == opentelemetry::Value::Bool(true)
                    || matches!(&attribute.value, opentelemetry::Value::String(value) if value.as_str() == "true"))
        })
}

impl ExportBatch {
    /// The common one-span path keeps its owned SDK vector without allocating.
    pub(super) fn uniform_root_priority(&self) -> Option<bool> {
        match self {
            Self::Spans(spans) => {
                let first = spans.first().is_some_and(mandatory_root);
                spans
                    .iter()
                    .skip(1)
                    .all(|span| mandatory_root(span) == first)
                    .then_some(first)
            }
            Self::Logs(_) => Some(false),
        }
    }

    /// Preserve span completion order: error children must reach the worker before
    /// their root computes its tail decision. Mixed classes use contiguous runs.
    pub(super) fn into_priority_runs(self) -> Vec<(Self, bool)> {
        let spans = match self {
            Self::Spans(spans) => spans,
            other => return vec![(other, false)],
        };
        let mut result = Vec::new();
        let mut run = Vec::new();
        let mut root = spans.first().is_some_and(mandatory_root);
        for span in spans {
            let next = mandatory_root(&span);
            if next != root {
                result.push((Self::Spans(std::mem::take(&mut run)), root));
                root = next;
            }
            run.push(span);
        }
        if !run.is_empty() {
            result.push((Self::Spans(run), root));
        }
        result
    }

    pub(super) fn len(&self) -> usize {
        match self {
            Self::Spans(values) => values.len(),
            Self::Logs(values) => values.len(),
        }
    }
    pub(super) fn estimated_bytes(&self) -> usize {
        fn value_bytes(value: &opentelemetry::Value) -> usize {
            use opentelemetry::{Array, Value as V};
            match value {
                V::String(value) => value.as_str().len().saturating_mul(2),
                V::Array(Array::String(values)) => values
                    .iter()
                    .map(|value| 128 + value.as_str().len().saturating_mul(2))
                    .sum(),
                V::Array(Array::Bool(values)) => values.len() * 8,
                V::Array(Array::I64(values)) => values.len() * 8,
                V::Array(Array::F64(values)) => values.len() * 8,
                _ => 0,
            }
        }
        fn attribute_bytes(attributes: &[KeyValue]) -> usize {
            attributes
                .iter()
                .map(|attribute| 128 + attribute.key.as_str().len() + value_bytes(&attribute.value))
                .sum()
        }
        fn log_bytes(value: &opentelemetry::logs::AnyValue) -> usize {
            use opentelemetry::logs::AnyValue;
            128 + match value {
                AnyValue::String(value) => value.as_str().len().saturating_mul(2),
                AnyValue::Bytes(value) => value.len(),
                AnyValue::ListAny(values) => values.iter().map(log_bytes).sum(),
                AnyValue::Map(values) => values
                    .iter()
                    .map(|(key, value)| key.as_str().len() + log_bytes(value))
                    .sum(),
                _ => 0,
            }
        }
        match self {
            Self::Spans(values) => values
                .iter()
                .map(|span| {
                    2048 + span.name.len() * 2
                        + attribute_bytes(&span.attributes)
                        + span
                            .events
                            .events
                            .iter()
                            .map(|event| 128 + event.name.len() * 2 + attribute_bytes(&event.attributes))
                            .sum::<usize>()
                        + span
                            .links
                            .links
                            .iter()
                            .map(|link| 128 + attribute_bytes(&link.attributes))
                            .sum::<usize>()
                })
                .sum(),
            Self::Logs(values) => values
                .iter()
                .map(|(record, scope)| {
                    2048 + scope.name().len() * 2
                        + record.body().map_or(0, log_bytes)
                        + record
                            .attributes_iter()
                            .map(|(key, value)| 128 + key.as_str().len() + log_bytes(value))
                            .sum::<usize>()
                })
                .sum(),
        }
    }
    pub(super) fn normalize(self, shared: &Shared) -> (&'static str, Vec<Document>) {
        match self {
            Self::Spans(values) => ("otel_spans", normalize_spans(values, shared)),
            Self::Logs(values) => ("otel_logs", normalize_logs(values, shared)),
        }
    }
}

struct LocalSpanExporter(Arc<Shared>);
impl std::fmt::Debug for LocalSpanExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MoquiSpanExporter")
    }
}
impl SpanExporter for LocalSpanExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let mut batch = batch;
        batch.retain(|span| {
            !span.attributes.iter().any(|attribute| {
                attribute.key.as_str() == "moqui.suppressed" && attribute.value == opentelemetry::Value::Bool(true)
            })
        });
        self.0.enqueue_sdk(ExportBatch::Spans(batch));
        Ok(())
    }
}

fn normalize_spans(batch: Vec<SpanData>, shared: &Shared) -> Vec<Document> {
    let mut documents = Vec::new();
    for span in batch {
        let mut document = Document::new();
        document.insert(
            "trace_id".into(),
            Value::String(span.span_context.trace_id().to_string()),
        );
        document.insert("span_id".into(), Value::String(span.span_context.span_id().to_string()));
        document.insert(
            "parent_span_id".into(),
            Value::String(if span.parent_span_id == SpanId::INVALID {
                String::new()
            } else {
                span.parent_span_id.to_string()
            }),
        );
        document.insert("name".into(), Value::string(span.name.as_ref()));
        document.insert(
            "kind".into(),
            Value::string(match span.span_kind {
                opentelemetry::trace::SpanKind::Server => "SERVER",
                opentelemetry::trace::SpanKind::Client => "CLIENT",
                opentelemetry::trace::SpanKind::Producer => "PRODUCER",
                opentelemetry::trace::SpanKind::Consumer => "CONSUMER",
                opentelemetry::trace::SpanKind::Internal => "INTERNAL",
            }),
        );
        document.insert("start".into(), Value::I64(millis(span.start_time)));
        document.insert("start_nano".into(), Value::I64(nanos(span.start_time)));
        document.insert(
            "duration_nano".into(),
            Value::I64(
                span.end_time
                    .duration_since(span.start_time)
                    .unwrap_or_default()
                    .as_nanos()
                    .min(i64::MAX as u128) as i64,
            ),
        );
        document.insert(
            "status_code".into(),
            Value::string(match span.status {
                opentelemetry::trace::Status::Error { .. } => "ERROR",
                opentelemetry::trace::Status::Ok => "OK",
                _ => "UNSET",
            }),
        );
        let attrs: Document = span
            .attributes
            .iter()
            .map(|attribute| (attribute.key.as_str().to_string(), core_value(&attribute.value)))
            .collect();
        document.insert(
            "sampled".into(),
            attrs.get("moqui.head.sampled").cloned().unwrap_or(Value::Bool(true)),
        );
        super::ingest::flatten_attributes(&mut document, attrs, "otel_spans");
        let events = span
            .events
            .events
            .into_iter()
            .map(|event| {
                let mut values: Document = event
                    .attributes
                    .iter()
                    .map(|attribute| (attribute.key.as_str().to_string(), core_value(&attribute.value)))
                    .collect();
                values.insert("name".into(), Value::string(event.name.as_ref()));
                values.insert("timestamp".into(), Value::I64(millis(event.timestamp)));
                Value::Map(values)
            })
            .collect();
        document.insert("events".into(), Value::List(events));
        shared.add_resource(&mut document);
        documents.push(document);
    }
    documents
}

fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn nanos(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(i64::MAX as u128) as i64
}

struct LocalMetricExporter(Arc<Shared>);
impl std::fmt::Debug for LocalMetricExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MoquiMetricExporter")
    }
}

impl opentelemetry_sdk::metrics::exporter::PushMetricExporter for LocalMetricExporter {
    async fn export(&self, metrics: &opentelemetry_sdk::metrics::data::ResourceMetrics) -> OTelSdkResult {
        let mut request =
            opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest::from(metrics);
        let resource = lock(&self.0.resource).clone();
        let resource = opentelemetry_sdk::Resource::builder_empty()
            .with_attributes([
                KeyValue::new("service.name", resource.service_name),
                KeyValue::new("service.instance.id", resource.instance_id),
                KeyValue::new("service.version", resource.version),
                KeyValue::new("vcs.ref.head.revision", resource.revision),
            ])
            .build();
        for metrics in &mut request.resource_metrics {
            metrics.resource = Some((&resource).into());
        }
        // A collection that cannot be converted is lost: count its points and say why, once per collection.
        let lost = |reason: String| {
            let points = data_points(&request);
            self.0
                .counters
                .dropped
                .fetch_add(points, std::sync::atomic::Ordering::Relaxed);
            tracing::warn!(points, "telemetry metric collection dropped: {reason}");
        };
        let json = match serde_json::to_vec(&request) {
            Ok(json) => json,
            Err(error) => {
                lost(format!("cannot encode it: {error}"));
                return Ok(());
            }
        };
        match super::ingest::Decoder::default().decode(
            super::ingest::Signal::Metrics,
            "application/json",
            &json,
            "local",
        ) {
            Ok(mut decoded) => {
                if decoded.rejected > 0 {
                    self.0
                        .counters
                        .dropped
                        .fetch_add(decoded.rejected, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(
                        points = decoded.rejected,
                        "telemetry metric points dropped: {}",
                        decoded.message
                    );
                }
                for document in &mut decoded.documents {
                    self.0.add_resource(document);
                }
                self.0.enqueue("otel_metrics", decoded.documents);
            }
            Err(error) => lost(format!("cannot decode it: {error}")),
        }
        Ok(())
    }
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }
    fn temporality(&self) -> opentelemetry_sdk::metrics::Temporality {
        opentelemetry_sdk::metrics::Temporality::Delta
    }
}

fn data_points(request: &opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest) -> u64 {
    use opentelemetry_proto::tonic::metrics::v1::metric::Data;
    request
        .resource_metrics
        .iter()
        .flat_map(|resource| &resource.scope_metrics)
        .flat_map(|scope| &scope.metrics)
        .map(|metric| match &metric.data {
            Some(Data::Gauge(data)) => data.data_points.len(),
            Some(Data::Sum(data)) => data.data_points.len(),
            Some(Data::Histogram(data)) => data.data_points.len(),
            Some(Data::ExponentialHistogram(data)) => data.data_points.len(),
            Some(Data::Summary(data)) => data.data_points.len(),
            None => 0,
        } as u64)
        .sum()
}

struct LocalLogExporter(Arc<Shared>);
impl std::fmt::Debug for LocalLogExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MoquiLogExporter")
    }
}

impl opentelemetry_sdk::logs::LogExporter for LocalLogExporter {
    async fn export(&self, batch: opentelemetry_sdk::logs::LogBatch<'_>) -> OTelSdkResult {
        self.0.enqueue_sdk(ExportBatch::Logs(
            batch
                .iter()
                .map(|(record, scope)| (record.clone(), scope.clone()))
                .collect(),
        ));
        Ok(())
    }
}

fn normalize_logs(
    batch: Vec<(
        opentelemetry_sdk::logs::SdkLogRecord,
        opentelemetry::InstrumentationScope,
    )>,
    shared: &Shared,
) -> Vec<Document> {
    let documents = batch
        .into_iter()
        .map(|(record, scope)| {
            let mut document = Document::new();
            document.insert(
                "timestamp".into(),
                Value::I64(millis(record.timestamp().unwrap_or_else(SystemTime::now))),
            );
            document.insert(
                "severity_number".into(),
                Value::I64(record.severity_number().map_or(0, |severity| severity as i64)),
            );
            document.insert(
                "severity_text".into(),
                Value::string(record.severity_text().unwrap_or("")),
            );
            document.insert("body".into(), record.body().map(core_log_value).unwrap_or(Value::Null));
            document.insert("scope_name".into(), Value::string(scope.name()));
            if let Some(context) = record.trace_context() {
                document.insert("trace_id".into(), Value::String(context.trace_id.to_string()));
                document.insert("span_id".into(), Value::String(context.span_id.to_string()));
            }
            let attributes: Document = record
                .attributes_iter()
                .map(|(key, value)| (key.as_str().to_string(), core_log_value(value)))
                .collect();
            if let Some(scope_name) = attributes.get("scope.name") {
                document.insert("scope_name".into(), scope_name.clone());
            }
            super::ingest::flatten_attributes(&mut document, attributes, "otel_logs");
            shared.add_resource(&mut document);
            document
        })
        .collect();
    documents
}

fn core_log_value(value: &opentelemetry::logs::AnyValue) -> Value {
    use opentelemetry::logs::AnyValue;
    match value {
        AnyValue::Int(value) => Value::I64(*value),
        AnyValue::Double(value) => Value::F64(*value),
        AnyValue::String(value) => Value::string(value.as_str()),
        AnyValue::Boolean(value) => Value::Bool(*value),
        AnyValue::Bytes(value) => Value::Bytes(value.as_ref().clone()),
        AnyValue::ListAny(values) => Value::List(values.iter().map(core_log_value).collect()),
        AnyValue::Map(values) => Value::Map(
            values
                .iter()
                .map(|(key, value)| (key.as_str().to_string(), core_log_value(value)))
                .collect(),
        ),
        _ => Value::Null,
    }
}
