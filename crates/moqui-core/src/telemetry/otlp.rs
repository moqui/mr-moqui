//! Optional OTLP/HTTP export over the framework's existing reqwest 0.13 transport, as protobuf (the OTLP default) or
//! JSON. No collector, async runtime, second HTTP stack, or tracing bridge is required. The remote export runs on its
//! own bounded queue and thread, so a slow or unreachable collector drops and counts records instead of holding up
//! local storage.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Weak};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;
use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::Exporter;
use crate::entity::document::Document;

/// Records waiting for the remote exporter, past which a batch is dropped and counted. The same as the local queue.
const REMOTE_QUEUE_RECORDS: usize = 8192;
/// Batches waiting for the remote exporter.
const REMOTE_QUEUE_BATCHES: usize = 64;
/// The most of a collector's response read, for its partial success.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

/// The OTLP/HTTP encoding, named as `OTEL_EXPORTER_OTLP_PROTOCOL` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// `http/protobuf`, `application/x-protobuf`: the default, as in OTLP.
    Protobuf,
    /// `http/json`, `application/json`.
    Json,
}

impl Protocol {
    /// `http/protobuf` (or `protobuf`) and `http/json` (or `json`); empty is the default, protobuf.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "http/protobuf" | "protobuf" => Some(Self::Protobuf),
            "http/json" | "json" => Some(Self::Json),
            _ => None,
        }
    }
}

pub struct OtlpExporter {
    endpoint: String,
    headers: Vec<(String, String)>,
    protocol: Protocol,
    client: moqui_lib::http::RestClient,
    conversion_errors: AtomicU64,
}

impl OtlpExporter {
    /// An exporter sending protobuf; [`Self::with_protocol`] picks JSON.
    pub fn new(endpoint: &str, headers: Vec<(String, String)>) -> Result<Self, Error> {
        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            return Err(Error::new("OTLP endpoint must use http or https"));
        }
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            headers,
            protocol: Protocol::Protobuf,
            client: moqui_lib::http::RestClient::new().connect_timeout(Duration::from_secs(2)),
            conversion_errors: AtomicU64::new(0),
        }
        .with_protocol(Protocol::Protobuf))
    }

    /// Protobuf needs the `telemetry` feature (the generated OTLP messages); without it this stays JSON.
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = if cfg!(feature = "telemetry") {
            protocol
        } else {
            Protocol::Json
        };
        self
    }

    pub fn protocol(&self) -> Protocol {
        self.protocol
    }
}

impl Exporter for OtlpExporter {
    fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
        use std::io::Read;
        let encoded = encode(index, &documents)?;
        let (content_type, body, lost) = match self.protocol {
            Protocol::Json => (
                "application/json",
                serde_json::to_vec(&encoded.body).map_err(|error| Error::new(format!("OTLP JSON failed: {error}")))?,
                0,
            ),
            Protocol::Protobuf => {
                let (body, lost, error) = protobuf(index, &encoded.body)?;
                if let Some(error) = error {
                    let failures = self.conversion_errors.fetch_add(1, Ordering::Relaxed);
                    if failures == 0 || failures.is_power_of_two() {
                        tracing::warn!(
                            index,
                            records = lost,
                            failures = failures + 1,
                            "telemetry records did not convert to OTLP protobuf and were not exported: {error}"
                        );
                    }
                }
                ("application/x-protobuf", body, lost)
            }
        };
        if encoded.records <= lost {
            // Nothing left to send: rollups are not exported, and records that did not convert are already counted.
            return Ok(documents.len().saturating_sub(lost));
        }
        let mut headers = self.headers.clone();
        headers.push(("Content-Type".to_string(), content_type.to_string()));
        let response = self.client.open(moqui_lib::http::OpenRequest {
            method: moqui_lib::http::Method::Post,
            url: format!("{}{}", self.endpoint, encoded.path),
            headers,
            body: Some(body.into()),
            timeout: Some(Duration::from_secs(5)),
        })?;
        if !(200..300).contains(&response.status) {
            return Err(Error::new(format!("OTLP export returned HTTP {}", response.status)));
        }
        let response_type = response
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.to_ascii_lowercase())
            .unwrap_or_default();
        let mut reply = Vec::new();
        // The records are accepted once the status is in; a reply that cannot be read only hides partial success.
        let _ = response.reader.take(MAX_RESPONSE_BYTES).read_to_end(&mut reply);
        let rejected = rejected(index, &response_type, &reply) as usize;
        Ok(documents.len().saturating_sub(lost.saturating_add(rejected)))
    }
}

/// Local persistence and optional external export fail independently. Local success is the
/// returned count; an unavailable collector never prevents local investigation. The remote side is a bounded queue
/// with its own thread: when it is full a batch is dropped and counted (`remote_dropped`), never waited for.
pub struct TeeExporter {
    local: Arc<dyn Exporter>,
    sender: Option<mpsc::SyncSender<(&'static str, Vec<Document>)>>,
    pending: Arc<AtomicUsize>,
    capacity: usize,
    full: AtomicU64,
    telemetry: Weak<super::Telemetry>,
}

impl TeeExporter {
    pub fn new(local: Arc<dyn Exporter>, remote: Arc<dyn Exporter>, telemetry: Weak<super::Telemetry>) -> Self {
        Self::with_capacity(local, remote, telemetry, REMOTE_QUEUE_RECORDS)
    }

    fn with_capacity(
        local: Arc<dyn Exporter>,
        remote: Arc<dyn Exporter>,
        telemetry: Weak<super::Telemetry>,
        capacity: usize,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(REMOTE_QUEUE_BATCHES);
        let pending = Arc::new(AtomicUsize::new(0));
        let sender = {
            let pending = Arc::clone(&pending);
            let telemetry = telemetry.clone();
            match std::thread::Builder::new()
                .name("moqui-telemetry-otlp".to_string())
                .spawn(move || remote_worker(remote, receiver, pending, telemetry))
            {
                Ok(_) => Some(sender),
                Err(error) => {
                    tracing::error!(%error, "external telemetry export thread did not start; it will be dropped");
                    None
                }
            }
        };
        Self {
            local,
            sender,
            pending,
            capacity,
            full: AtomicU64::new(0),
            telemetry,
        }
    }

    /// Wait until the remote queue is empty and its last request is done, for tests and benchmarks. False when
    /// `timeout` passed first.
    pub fn wait_remote(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.pending.load(Ordering::Acquire) > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    fn offer(&self, index: &'static str, documents: &[Document]) {
        let count = documents.len();
        let reserved = self
            .pending
            .try_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                (pending + count <= self.capacity).then_some(pending + count)
            })
            .is_ok();
        if reserved {
            if let Some(sender) = &self.sender {
                if sender.try_send((index, documents.to_vec())).is_ok() {
                    return;
                }
            }
            self.pending.fetch_sub(count, Ordering::AcqRel);
        }
        if let Some(telemetry) = self.telemetry.upgrade() {
            telemetry.note_remote_export(0, count as u64, false);
        }
        let full = self.full.fetch_add(1, Ordering::Relaxed);
        if full == 0 || full.is_power_of_two() {
            tracing::warn!(
                index,
                records = count,
                times = full + 1,
                "external telemetry export is behind; records dropped"
            );
        }
    }
}

impl Exporter for TeeExporter {
    fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
        match index {
            "otel_spans" => self.offer("otel_spans", &documents),
            "otel_logs" => self.offer("otel_logs", &documents),
            "otel_metrics" => self.offer("otel_metrics", &documents),
            _ => {}
        }
        self.local.export(index, documents)
    }
    fn export_received(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
        self.local.export_received(index, documents)
    }
    fn upsert_error(&self, document: Document) -> Result<(), Error> {
        self.local.upsert_error(document)
    }
}

/// The remote export thread: one request per queued batch. A failing or panicking exporter costs its batch, which is
/// counted and logged (the first time, then at powers of two).
fn remote_worker(
    remote: Arc<dyn Exporter>,
    receiver: mpsc::Receiver<(&'static str, Vec<Document>)>,
    pending: Arc<AtomicUsize>,
    telemetry: Weak<super::Telemetry>,
) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    let mut failures = 0u64;
    while let Ok((index, documents)) = receiver.recv() {
        let count = documents.len() as u64;
        let outcome = catch_unwind(AssertUnwindSafe(|| remote.export(index, documents)))
            .unwrap_or_else(|_| Err(Error::new("the external telemetry exporter panicked")));
        match outcome {
            Ok(exported) => {
                let exported = (exported as u64).min(count);
                let dropped = count - exported;
                if let Some(telemetry) = telemetry.upgrade() {
                    telemetry.note_remote_export(exported, dropped, false);
                }
                if dropped > 0 {
                    failures += 1;
                    if failures == 1 || (failures - 1).is_power_of_two() {
                        tracing::warn!(
                            index,
                            rejected = dropped,
                            "external collector partially rejected telemetry"
                        );
                    }
                }
            }
            Err(error) => {
                if let Some(telemetry) = telemetry.upgrade() {
                    telemetry.note_remote_export(0, count, true);
                }
                failures += 1;
                if failures == 1 || (failures - 1).is_power_of_two() {
                    tracing::warn!(index, failures, "external telemetry export failed: {error}");
                }
            }
        }
        pending.fetch_sub(count as usize, Ordering::AcqRel);
    }
}

/// Wrap a local exporter when a configured endpoint exists. Header values are never logged. `otlp-protocol` is
/// `http/protobuf` (the default) or `http/json`.
pub fn configured(factory: &crate::context::ExecutionContextFactory, local: Arc<dyn Exporter>) -> Arc<dyn Exporter> {
    let endpoint = factory.conf_setting("telemetry", "otlp-endpoint");
    if endpoint.is_empty() {
        return local;
    }
    let headers = factory
        .conf_setting("telemetry", "otlp-headers")
        .split(',')
        .filter_map(|item| {
            item.split_once('=')
                .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let named = factory.conf_setting("telemetry", "otlp-protocol");
    let protocol = Protocol::parse(&named).unwrap_or_else(|| {
        tracing::warn!(
            protocol = named.as_str(),
            "telemetry otlp-protocol is not http/protobuf or http/json; using http/protobuf"
        );
        Protocol::Protobuf
    });
    match OtlpExporter::new(&endpoint, headers) {
        Ok(remote) => {
            let remote = remote.with_protocol(protocol);
            if remote.protocol() != protocol {
                tracing::warn!("this build has no protobuf OTLP export; sending OTLP JSON");
            }
            Arc::new(TeeExporter::new(
                local,
                Arc::new(remote),
                Arc::downgrade(&super::for_factory(factory)),
            ))
        }
        Err(error) => {
            tracing::warn!("external telemetry exporter is disabled: {error}");
            local
        }
    }
}

/// One OTLP request body as OTLP JSON: records grouped by resource and then by scope, so a batch from one service
/// is one resource with one scope rather than a resource per record.
struct Encoded {
    path: &'static str,
    body: Json,
    /// Records put in the body; the others (metric rollups other than minute) are not sent, by design.
    records: usize,
}

fn encode(index: &str, documents: &[Document]) -> Result<Encoded, Error> {
    let (path, resources_key, scope_key, entries_key) = match index {
        "otel_spans" => ("/v1/traces", "resourceSpans", "scopeSpans", "spans"),
        "otel_logs" => ("/v1/logs", "resourceLogs", "scopeLogs", "logRecords"),
        "otel_metrics" => ("/v1/metrics", "resourceMetrics", "scopeMetrics", "metrics"),
        _ => return Err(Error::new("this index is not an OTLP signal")),
    };
    let mut groups = Groups::default();
    for document in documents {
        let resource = resource_attributes(document);
        let scope = match super::text(document, "scope_name") {
            name if name.is_empty() => "moqui".to_string(),
            name => name,
        };
        match index {
            "otel_spans" => {
                let start = integer(document, "start_nano");
                let mut span = json!({"traceId":super::text(document,"trace_id"),"spanId":super::text(document,"span_id"),
                    "name":super::text(document,"name"),"kind":match super::text(document,"kind").as_str(){"SERVER"=>2,"CLIENT"=>3,"PRODUCER"=>4,"CONSUMER"=>5,_=>1},
                    "startTimeUnixNano":start.to_string(),"endTimeUnixNano":start.saturating_add(integer(document,"duration_nano")).to_string(),
                    "attributes": attributes(document),"status":{"code":if super::text(document,"status_code")=="ERROR"{2}else{1}}});
                let parent = super::text(document, "parent_span_id");
                if !parent.is_empty() {
                    span["parentSpanId"] = json!(parent);
                }
                if let Some(Value::List(events)) = document.get("events") {
                    span["events"] = Json::Array(events.iter().filter_map(|event| if let Value::Map(event) = event {
                        Some(json!({"name":super::text(event,"name"),"timeUnixNano":integer(event,"timestamp").saturating_mul(1_000_000).to_string(),
                            "attributes":event.iter().filter(|(key,_)|key.as_str()!="name"&&key.as_str()!="timestamp").map(|(key,value)|kv(key,value)).collect::<Vec<_>>()}))
                    } else { None }).collect());
                }
                groups.push(resource, scope, span);
            }
            "otel_logs" => {
                let mut log = json!({"timeUnixNano":integer(document,"timestamp").saturating_mul(1_000_000).to_string(),
                    "severityNumber":integer(document,"severity_number"),"severityText":super::text(document,"severity_text"),
                    "body":any(document.get("body").unwrap_or(&Value::Null)),"attributes":attributes(document)});
                for (from, to) in [("trace_id", "traceId"), ("span_id", "spanId")] {
                    let value = super::text(document, from);
                    if !value.is_empty() {
                        log[to] = json!(value);
                    }
                }
                groups.push(resource, scope, log);
            }
            "otel_metrics" => {
                if super::text(document, "granularity") != "minute" {
                    continue;
                }
                let mut point = json!({"startTimeUnixNano":integer(document,"bucket_start").saturating_mul(1_000_000).to_string(),
                    "timeUnixNano":integer(document,"bucket_end").saturating_mul(1_000_000).to_string(),"attributes":attributes(document)});
                let mut metric = json!({"name":super::text(document,"name"),"unit":super::text(document,"unit")});
                if let (Some(Value::List(bounds)), Some(Value::List(buckets))) =
                    (document.get("bounds"), document.get("buckets"))
                {
                    if !buckets.is_empty() {
                        point["count"] = json!(integer(document, "count").to_string());
                        if document.contains_key("sum") {
                            point["sum"] = json!(super::number(document, "sum"));
                        }
                        point["explicitBounds"] =
                            json!(bounds.iter().map(moqui_lib::json::to_json).collect::<Vec<_>>());
                        point["bucketCounts"] = json!(buckets
                            .iter()
                            .map(moqui_lib::object::to_plain_string)
                            .collect::<Vec<_>>());
                        for key in ["min", "max"] {
                            if document.contains_key(key) {
                                point[key] = json!(super::number(document, key));
                            }
                        }
                        metric["histogram"] = json!({"aggregationTemporality":1,"dataPoints":[point]});
                    } else {
                        point["asDouble"] = json!(super::number(document, "sum"));
                        metric["sum"] = json!({"aggregationTemporality":1,"isMonotonic":true,"dataPoints":[point]});
                    }
                } else {
                    point["asDouble"] = json!(super::number(document, "sum"));
                    metric["sum"] = json!({"aggregationTemporality":1,"isMonotonic":true,"dataPoints":[point]});
                }
                if super::text(document, "aggregation") == "gauge" {
                    let points = metric["sum"]["dataPoints"].clone();
                    if let Some(values) = metric.as_object_mut() {
                        values.remove("sum");
                    }
                    metric["gauge"] = json!({"dataPoints":points});
                }
                if let Some(raw) = document.get("exponential_histogram") {
                    let mut point = moqui_lib::json::to_json(raw);
                    point["startTimeUnixNano"] =
                        json!(integer(document, "bucket_start").saturating_mul(1_000_000).to_string());
                    point["timeUnixNano"] =
                        json!(integer(document, "bucket_end").saturating_mul(1_000_000).to_string());
                    point["attributes"] = json!(attributes(document));
                    if let Some(values) = metric.as_object_mut() {
                        values.remove("sum");
                        values.remove("histogram");
                    }
                    let temporality = super::text(document, "aggregation_temporality");
                    metric["exponentialHistogram"] = json!({"aggregationTemporality":if temporality=="2"||temporality=="AGGREGATION_TEMPORALITY_CUMULATIVE"{2}else{1},"dataPoints":[point]});
                }
                groups.push(resource, scope, metric);
            }
            _ => unreachable!("the signal was matched above"),
        }
    }
    let records = groups.records;
    Ok(Encoded {
        path,
        body: json!({ resources_key: groups.into_json(scope_key, entries_key) }),
        records,
    })
}

/// A scope's name and its records.
type Scope = (String, Vec<Json>);

/// Resources (their attributes) in first-seen order, each with its scopes in first-seen order.
#[derive(Default)]
struct Groups {
    resources: Vec<(Vec<Json>, Vec<Scope>)>,
    by_resource: std::collections::HashMap<String, usize>,
    records: usize,
}
impl Groups {
    fn push(&mut self, resource: Vec<Json>, scope: String, entry: Json) {
        let key = Json::Array(resource.clone()).to_string();
        let resources = &mut self.resources;
        let at = *self.by_resource.entry(key).or_insert_with(|| {
            resources.push((resource, Vec::new()));
            resources.len() - 1
        });
        let scopes = &mut self.resources[at].1;
        match scopes.iter_mut().find(|(name, _)| *name == scope) {
            Some((_, entries)) => entries.push(entry),
            None => scopes.push((scope, vec![entry])),
        }
        self.records += 1;
    }
    fn into_json(self, scope_key: &str, entries_key: &str) -> Vec<Json> {
        self.resources
            .into_iter()
            .map(|(attributes, scopes)| {
                let scopes = scopes
                    .into_iter()
                    .map(|(name, entries)| json!({"scope": {"name": name}, entries_key: entries}))
                    .collect::<Vec<_>>();
                json!({"resource": {"attributes": attributes}, scope_key: scopes})
            })
            .collect()
    }
}

/// OTLP JSON to protobuf through the generated messages. A resource that will not convert is retried a record at a
/// time, so one bad record (an id that is not hex, say) costs only itself. Returns the body, the records lost, and the
/// first conversion error.
#[cfg(feature = "telemetry")]
fn protobuf(index: &str, request: &Json) -> Result<(Vec<u8>, usize, Option<String>), Error> {
    use opentelemetry_proto::tonic::collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    };
    use prost::Message;
    let mut lost = 0;
    let mut error = None;
    let bytes = match index {
        "otel_spans" => ExportTraceServiceRequest {
            resource_spans: convert(&request["resourceSpans"], "scopeSpans", "spans", &mut lost, &mut error),
        }
        .encode_to_vec(),
        "otel_logs" => ExportLogsServiceRequest {
            resource_logs: convert(
                &request["resourceLogs"],
                "scopeLogs",
                "logRecords",
                &mut lost,
                &mut error,
            ),
        }
        .encode_to_vec(),
        "otel_metrics" => ExportMetricsServiceRequest {
            resource_metrics: convert(
                &request["resourceMetrics"],
                "scopeMetrics",
                "metrics",
                &mut lost,
                &mut error,
            ),
        }
        .encode_to_vec(),
        _ => return Err(Error::new("this index is not an OTLP signal")),
    };
    Ok((bytes, lost, error))
}

#[cfg(not(feature = "telemetry"))]
fn protobuf(_index: &str, _request: &Json) -> Result<(Vec<u8>, usize, Option<String>), Error> {
    Err(Error::new("protobuf OTLP export needs the telemetry feature"))
}

#[cfg(feature = "telemetry")]
fn convert<G: serde::de::DeserializeOwned>(
    groups: &Json,
    scope_key: &str,
    entries_key: &str,
    lost: &mut usize,
    error: &mut Option<String>,
) -> Vec<G> {
    let mut converted = Vec::new();
    for group in groups.as_array().into_iter().flatten() {
        let first = match serde_json::from_value::<G>(group.clone()) {
            Ok(message) => {
                converted.push(message);
                continue;
            }
            Err(first) => first,
        };
        error.get_or_insert_with(|| first.to_string());
        let mut kept = group.clone();
        let mut good = 0;
        for scope in kept[scope_key].as_array_mut().into_iter().flatten() {
            let entries = std::mem::take(&mut scope[entries_key]);
            let mut passing = Vec::new();
            for entry in entries.as_array().into_iter().flatten() {
                let single = json!({"resource": group["resource"], scope_key: [{"scope": scope["scope"], entries_key: [entry]}]});
                if serde_json::from_value::<G>(single).is_ok() {
                    passing.push(entry.clone());
                } else {
                    *lost += 1;
                }
            }
            good += passing.len();
            scope[entries_key] = Json::Array(passing);
        }
        match serde_json::from_value::<G>(kept) {
            Ok(message) if good > 0 => converted.push(message),
            Ok(_) => {}
            Err(_) => *lost += good,
        }
    }
    converted
}

/// The records an OTLP response says were rejected, from its partial success in either encoding.
fn rejected(index: &str, content_type: &str, body: &[u8]) -> u64 {
    if body.is_empty() {
        return 0;
    }
    if content_type.contains("protobuf") {
        return rejected_protobuf(index, body);
    }
    serde_json::from_slice::<Json>(body)
        .ok()
        .and_then(|body| {
            body.get("partialSuccess").and_then(|partial| {
                ["rejectedSpans", "rejectedLogRecords", "rejectedDataPoints"]
                    .iter()
                    .find_map(|key| {
                        partial.get(*key).and_then(|value| {
                            value
                                .as_u64()
                                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                        })
                    })
            })
        })
        .unwrap_or(0)
}

#[cfg(feature = "telemetry")]
fn rejected_protobuf(index: &str, body: &[u8]) -> u64 {
    use opentelemetry_proto::tonic::collector::{
        logs::v1::ExportLogsServiceResponse, metrics::v1::ExportMetricsServiceResponse,
        trace::v1::ExportTraceServiceResponse,
    };
    use prost::Message;
    let rejected = match index {
        "otel_spans" => ExportTraceServiceResponse::decode(body)
            .ok()
            .and_then(|response| response.partial_success)
            .map(|partial| partial.rejected_spans),
        "otel_logs" => ExportLogsServiceResponse::decode(body)
            .ok()
            .and_then(|response| response.partial_success)
            .map(|partial| partial.rejected_log_records),
        _ => ExportMetricsServiceResponse::decode(body)
            .ok()
            .and_then(|response| response.partial_success)
            .map(|partial| partial.rejected_data_points),
    };
    rejected.unwrap_or(0).max(0) as u64
}

#[cfg(not(feature = "telemetry"))]
fn rejected_protobuf(_index: &str, _body: &[u8]) -> u64 {
    0
}

fn integer(document: &Document, key: &str) -> i64 {
    match document.get(key) {
        Some(Value::I64(value)) => *value,
        Some(Value::I32(value)) => i64::from(*value),
        Some(value) => moqui_lib::object::to_plain_string(value).parse().unwrap_or(0),
        None => 0,
    }
}
fn resource_attributes(document: &Document) -> Vec<Json> {
    if let Some(Value::Map(resource)) = document.get("resource") {
        return resource.iter().map(|(key, value)| kv(key, value)).collect();
    }
    [
        "service.name",
        "service.instance.id",
        "service.version",
        "vcs.ref.head.revision",
    ]
    .iter()
    .filter_map(|key| document.get(*key).map(|value| kv(key, value)))
    .collect()
}
fn attributes(document: &Document) -> Vec<Json> {
    match document.get("attributes") {
        Some(Value::Map(values)) => values
            .iter()
            .filter(|(key, _)| !key.starts_with("moqui.head."))
            .map(|(key, value)| kv(key, value))
            .collect(),
        _ => Vec::new(),
    }
}
fn kv(key: &str, value: &Value) -> Json {
    json!({"key":key,"value":any(value)})
}
fn any(value: &Value) -> Json {
    match value {
        Value::Bool(value) => json!({"boolValue":value}),
        Value::I32(value) => json!({"intValue":value.to_string()}),
        Value::I64(value) => json!({"intValue":value.to_string()}),
        Value::F64(value) => json!({"doubleValue":value}),
        Value::List(values) => json!({"arrayValue":{"values":values.iter().map(any).collect::<Vec<_>>()}}),
        Value::Map(values) => {
            json!({"kvlistValue":{"values":values.iter().map(|(key,value)|kv(key,value)).collect::<Vec<_>>()}})
        }
        _ => json!({"stringValue":moqui_lib::object::to_plain_string(value)}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::ingest::{Decoder, Signal};

    fn document(values: Json) -> Document {
        match moqui_lib::json::from_json(&values) {
            Value::Map(document) => document,
            _ => unreachable!(),
        }
    }

    fn span(span_id: &str, service: &str, scope: &str) -> Document {
        document(
            json!({"trace_id":"0123456789abcdef0123456789abcdef","span_id":span_id,"parent_span_id":"1111111111111111",
            "name":"test","kind":"SERVER","start_nano":1_000_000_000i64,"duration_nano":100_000_000i64,"status_code":"ERROR",
            "scope_name":scope,"service.name":service,"attributes":{"http.route":"/a","count":3,"ratio":0.5,"ok":true},
            "events":[{"name":"exception","timestamp":1000,"exception.message":"boom"}]}),
        )
    }

    fn telemetry() -> Arc<super::super::Telemetry> {
        Arc::new(super::super::Telemetry::from_conf(
            "<moqui-conf><telemetry enabled=\"false\"/></moqui-conf>",
            super::super::Resource::default(),
        ))
    }

    fn decode(signal: Signal, content_type: &str, body: &[u8]) -> Vec<Document> {
        Decoder::default()
            .decode(signal, content_type, body, "source")
            .unwrap()
            .documents
    }

    #[test]
    fn export_json_round_trips_through_the_receiver() {
        let mut document = Document::new();
        for (key, value) in [
            ("trace_id", Value::string("0123456789abcdef0123456789abcdef")),
            ("span_id", Value::string("0123456789abcdef")),
            ("name", Value::string("test")),
            ("kind", Value::string("SERVER")),
            ("start_nano", Value::I64(1_000_000_000)),
            ("duration_nano", Value::I64(100_000_000)),
            ("service.name", Value::string("demo")),
        ] {
            document.insert(key.into(), value);
        }
        let encoded = encode("otel_spans", &[document]).unwrap();
        assert_eq!(encoded.path, "/v1/traces");
        let decoded = decode(
            Signal::Traces,
            "application/json",
            &serde_json::to_vec(&encoded.body).unwrap(),
        );
        assert_eq!(decoded.len(), 1);
        assert_eq!(super::super::text(&decoded[0], "service.name"), "demo");
    }

    /// Every signal sent as protobuf decodes in the receiver to the same documents as when sent as JSON.
    #[cfg(feature = "telemetry")]
    #[test]
    fn export_protobuf_decodes_like_json_in_the_receiver() {
        let spans = vec![
            span("0123456789abcdef", "demo", "moqui"),
            span("0123456789abcdee", "demo", "other"),
        ];
        let logs = vec![document(
            json!({"timestamp":1_700_000_000_000i64,"severity_number":17,"severity_text":"ERROR",
            "body":"it failed","trace_id":"0123456789abcdef0123456789abcdef","span_id":"0123456789abcdef",
            "service.name":"demo","attributes":{"code.function":"run","moqui.head.hidden":"x"}}),
        )];
        let metrics = vec![
            document(
                json!({"name":"http.server.duration","unit":"s","granularity":"minute","aggregation":"histogram",
                "bucket_start":1_700_000_000_000i64,"bucket_end":1_700_000_060_000i64,"count":3,"sum":0.6,"min":0.1,"max":0.3,
                "bounds":[0.1,0.2],"buckets":[1,1,1],"service.name":"demo","attributes":{"http.route":"/a"}}),
            ),
            document(
                json!({"name":"jobs.run","unit":"1","granularity":"minute","aggregation":"sum",
                "bucket_start":1_700_000_000_000i64,"bucket_end":1_700_000_060_000i64,"sum":4.0,"service.name":"demo"}),
            ),
            document(
                json!({"name":"pool.size","unit":"1","granularity":"minute","aggregation":"gauge",
                "bucket_start":1_700_000_000_000i64,"bucket_end":1_700_000_060_000i64,"sum":7.0,"service.name":"demo"}),
            ),
            document(json!({"name":"hourly","granularity":"hour","bucket_start":0,"bucket_end":1,"sum":1.0})),
        ];
        for (index, signal, documents, expected) in [
            ("otel_spans", Signal::Traces, spans, 2),
            ("otel_logs", Signal::Logs, logs, 1),
            ("otel_metrics", Signal::Metrics, metrics, 3),
        ] {
            let encoded = encode(index, &documents).unwrap();
            let json = decode(signal, "application/json", &serde_json::to_vec(&encoded.body).unwrap());
            let (bytes, lost, error) = protobuf(index, &encoded.body).unwrap();
            assert_eq!((lost, error), (0, None), "{index}");
            let proto = decode(signal, "application/x-protobuf", &bytes);
            assert_eq!(json.len(), expected, "{index}");
            assert_eq!(proto, json, "{index}");
        }
        let decoded = decode(
            Signal::Traces,
            "application/x-protobuf",
            &protobuf(
                "otel_spans",
                &encode("otel_spans", &[span("0123456789abcdef", "demo", "moqui")])
                    .unwrap()
                    .body,
            )
            .unwrap()
            .0,
        );
        assert_eq!(super::super::text(&decoded[0], "parent_span_id"), "1111111111111111");
        assert_eq!(super::super::text(&decoded[0], "status_code"), "ERROR");
    }

    #[test]
    fn records_group_by_resource_then_scope() {
        let documents = vec![
            span("0000000000000001", "a", "moqui"),
            span("0000000000000002", "b", "moqui"),
            span("0000000000000003", "a", "other"),
            span("0000000000000004", "a", "moqui"),
            span("0000000000000005", "b", ""),
        ];
        let encoded = encode("otel_spans", &documents).unwrap();
        assert_eq!(encoded.records, 5);
        let resources = encoded.body["resourceSpans"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        let shape = resources
            .iter()
            .map(|resource| {
                resource["scopeSpans"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|scope| {
                        (
                            scope["scope"]["name"].as_str().unwrap().to_string(),
                            scope["spans"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|span| span["spanId"].as_str().unwrap()[15..].to_string())
                                .collect::<Vec<_>>()
                                .join(""),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            shape,
            vec![
                vec![
                    ("moqui".to_string(), "14".to_string()),
                    ("other".to_string(), "3".to_string())
                ],
                vec![("moqui".to_string(), "25".to_string())],
            ]
        );
        let decoded = decode(
            Signal::Traces,
            "application/json",
            &serde_json::to_vec(&encoded.body).unwrap(),
        );
        assert_eq!(decoded.len(), 5);
    }

    /// A record that will not convert (an id that is not hex) is lost alone; the rest of its resource is sent.
    #[cfg(feature = "telemetry")]
    #[test]
    fn a_record_that_will_not_convert_costs_only_itself() {
        let documents = vec![
            span("0000000000000001", "a", "moqui"),
            span("not-hex-at-all!!", "a", "moqui"),
            span("0000000000000003", "a", "moqui"),
        ];
        let (bytes, lost, error) = protobuf("otel_spans", &encode("otel_spans", &documents).unwrap().body).unwrap();
        assert_eq!(lost, 1);
        assert!(error.is_some());
        assert_eq!(decode(Signal::Traces, "application/x-protobuf", &bytes).len(), 2);
    }

    /// A stub OTLP endpoint: each request's content type and body go to the channel, and it answers `reply`.
    fn stub(reply: Vec<u8>, reply_type: &'static str) -> (String, mpsc::Receiver<(String, Vec<u8>)>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut buffer = [0u8; 8192];
                let (head, length) = loop {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break (String::new(), 0);
                    };
                    if read == 0 {
                        break (String::new(), 0);
                    }
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                        request.drain(..end + 4);
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse().ok())
                            .unwrap_or(0);
                        break (head, length);
                    }
                };
                while request.len() < length {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&buffer[..read]),
                    }
                }
                let content_type = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-type:"))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let _ = sender.send((content_type, request));
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {reply_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = stream.write_all(&reply);
            }
        });
        (url, receiver)
    }

    #[cfg(feature = "telemetry")]
    #[test]
    fn protobuf_export_posts_one_grouped_request_and_reads_partial_success() {
        use opentelemetry_proto::tonic::collector::trace::v1::{ExportTracePartialSuccess, ExportTraceServiceResponse};
        use prost::Message;
        let reply = ExportTraceServiceResponse {
            partial_success: Some(ExportTracePartialSuccess {
                rejected_spans: 1,
                error_message: "one too many".into(),
            }),
        }
        .encode_to_vec();
        let (url, requests) = stub(reply, "application/x-protobuf");
        let exporter = OtlpExporter::new(&url, vec![("x-key".into(), "secret".into())]).unwrap();
        assert_eq!(exporter.protocol(), Protocol::Protobuf);
        let documents = vec![
            span("0000000000000001", "a", "moqui"),
            span("0000000000000002", "a", "moqui"),
        ];
        assert_eq!(exporter.export("otel_spans", documents).unwrap(), 1);
        let (content_type, body) = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(content_type, "application/x-protobuf");
        let request =
            opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest::decode(&body[..]).unwrap();
        assert_eq!(request.resource_spans.len(), 1);
        assert_eq!(request.resource_spans[0].scope_spans.len(), 1);
        assert_eq!(request.resource_spans[0].scope_spans[0].spans.len(), 2);
    }

    #[test]
    fn json_export_posts_json_and_reads_partial_success() {
        let (url, requests) = stub(
            br#"{"partialSuccess":{"rejectedLogRecords":"2"}}"#.to_vec(),
            "application/json",
        );
        let exporter = OtlpExporter::new(&url, Vec::new())
            .unwrap()
            .with_protocol(Protocol::Json);
        let log = document(json!({"timestamp":1,"severity_number":9,"body":"x","service.name":"a"}));
        assert_eq!(exporter.export("otel_logs", vec![log; 3]).unwrap(), 1);
        let (content_type, body) = requests.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(content_type, "application/json");
        assert_eq!(decode(Signal::Logs, "application/json", &body).len(), 3);
    }

    #[test]
    fn protocol_names() {
        assert_eq!(Protocol::parse(""), Some(Protocol::Protobuf));
        assert_eq!(Protocol::parse("http/protobuf"), Some(Protocol::Protobuf));
        assert_eq!(Protocol::parse("HTTP/JSON"), Some(Protocol::Json));
        assert_eq!(Protocol::parse("grpc"), None);
    }

    struct Count {
        calls: Arc<AtomicUsize>,
        partial: bool,
    }
    impl Exporter for Count {
        fn export(&self, _: &str, documents: Vec<Document>) -> Result<usize, Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(if self.partial {
                documents.len() / 2
            } else {
                documents.len()
            })
        }
    }

    #[test]
    fn received_signals_never_forward_and_partial_external_loss_is_separate() {
        let telemetry = telemetry();
        let local_calls = Arc::new(AtomicUsize::new(0));
        let remote_calls = Arc::new(AtomicUsize::new(0));
        let exporter = TeeExporter::new(
            Arc::new(Count {
                calls: local_calls.clone(),
                partial: false,
            }),
            Arc::new(Count {
                calls: remote_calls.clone(),
                partial: true,
            }),
            Arc::downgrade(&telemetry),
        );
        let document = Document::from_iter([("source".into(), Value::string("local"))]);
        assert_eq!(
            exporter.export_received("otel_logs", vec![document.clone()]).unwrap(),
            1
        );
        assert!(exporter.wait_remote(Duration::from_secs(5)));
        assert_eq!(remote_calls.load(Ordering::Relaxed), 0);
        assert_eq!(exporter.export("otel_logs", vec![document; 4]).unwrap(), 4);
        assert!(exporter.wait_remote(Duration::from_secs(5)));
        assert_eq!(local_calls.load(Ordering::Relaxed), 2);
        assert_eq!(remote_calls.load(Ordering::Relaxed), 1);
        let counters = telemetry.counters();
        assert_eq!(counters.remote_exported, 2);
        assert_eq!(counters.remote_dropped, 2);
        assert_eq!(counters.dropped, 0);
        assert_eq!(counters.remote_failed, 0);
    }

    /// A collector that accepts connections and never answers holds up only the remote thread: local export goes on
    /// at full speed, and what does not fit the remote queue is dropped and counted.
    #[test]
    fn a_stalled_collector_never_blocks_local_export() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                held.push(stream);
            }
        });
        let telemetry = telemetry();
        let local_calls = Arc::new(AtomicUsize::new(0));
        let exporter = TeeExporter::with_capacity(
            Arc::new(Count {
                calls: local_calls.clone(),
                partial: false,
            }),
            Arc::new(OtlpExporter::new(&url, Vec::new()).unwrap()),
            Arc::downgrade(&telemetry),
            1000,
        );
        let batch = (0..100)
            .map(|at| span(&format!("{:016x}", at + 1), "demo", "moqui"))
            .collect::<Vec<_>>();
        let started = Instant::now();
        for _ in 0..30 {
            assert_eq!(exporter.export("otel_spans", batch.clone()).unwrap(), 100);
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "local export waited on the collector: {:?}",
            started.elapsed()
        );
        assert_eq!(local_calls.load(Ordering::Relaxed), 30);
        let counters = telemetry.counters();
        // The queue holds 1,000 records, the batch on the wire among them; the rest are dropped.
        assert_eq!(counters.remote_dropped, 2000);
        assert_eq!(counters.remote_exported, 0);
        assert!(!exporter.wait_remote(Duration::from_millis(10)));
    }
}
