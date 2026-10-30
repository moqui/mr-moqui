//! Application telemetry. SDK types deliberately stay inside this module.
//!
//! Recording never writes through the entity/search facade on a business thread. Exporters run
//! on a bounded worker, outside business transactions, and independent failed records are skipped.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::document::Document;

mod exponential;
pub mod ingest;
pub mod maintenance;
#[cfg(feature = "http")]
pub mod otlp;
pub mod received;
pub mod scrape;

pub fn register(registry: &mut crate::native::NativeRegistry) -> Result<(), crate::context::Error> {
    registry.service_function("moqui_core::telemetry::clean", clean)?;
    registry.service_function("moqui_core::telemetry::rollup", rollup)?;
    registry.service_function("moqui_core::telemetry::partition_tables", partition_tables)?;
    registry.service_function("moqui_core::telemetry::scrape_targets", scrape::scrape_targets)?;
    registry.service_function("moqui_core::telemetry::run_checks", scrape::run_checks)?;
    Ok(())
}

pub fn clean(
    ec: &ExecutionContext,
    _service: &crate::native::ServiceInfo<'_>,
    _parameters: &crate::native::Parameters,
) -> Result<crate::native::Parameters, Error> {
    maintenance::retain(ec.factory()).map(|document| document.into_iter().collect())
}

pub fn partition_tables(
    ec: &ExecutionContext,
    _service: &crate::native::ServiceInfo<'_>,
    parameters: &crate::native::Parameters,
) -> Result<crate::native::Parameters, Error> {
    let index = parameters
        .get("index")
        .and_then(Value::as_str)
        .filter(|index| !index.is_empty());
    maintenance::partition_tables(ec.factory(), index).map(|document| document.into_iter().collect())
}

pub fn rollup(
    ec: &ExecutionContext,
    _service: &crate::native::ServiceInfo<'_>,
    _parameters: &crate::native::Parameters,
) -> Result<crate::native::Parameters, Error> {
    maintenance::rollup_metrics(ec.factory()).map(|document| document.into_iter().collect())
}
#[cfg(feature = "telemetry")]
mod prometheus;
#[cfg(feature = "telemetry")]
mod sdk;

/// The defaults of [`Capacity`].
const MAX_QUEUE: usize = 8192;
const MAX_BUFFER_BYTES: usize = 32 * 1024 * 1024;
#[cfg(test)]
const ROOT_RESERVED_RECORDS: usize = MAX_QUEUE / ROOT_RESERVE_SHARE;
#[cfg(test)]
const ROOT_RESERVED_BYTES: usize = MAX_BUFFER_BYTES / ROOT_RESERVE_SHARE;
/// Request and job roots may use the last 1/16 of the queue, which other records may not.
const ROOT_RESERVE_SHARE: usize = 16;
const MAX_TRACES: usize = 1024;
const MAX_TRACE_SPANS: usize = 512;
const MAX_DECISIONS: usize = 4096;
const MAX_SERIES: usize = 20_000;

/// A W3C context that can cross worker and async boundaries without an SDK or a thread guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub sampled: bool,
    pub tracestate: String,
    /// Made by this process's SDK, so `sampled` is this server's own head decision. A context extracted from a caller's
    /// `traceparent` is not local: its flag never turns sampling on here.
    pub local: bool,
}

impl TraceContext {
    pub fn extract(traceparent: &str, tracestate: &str) -> Option<Self> {
        let parts: Vec<_> = traceparent.trim().split('-').collect();
        let ["00", trace_id, span_id, flags] = parts.as_slice() else {
            return None;
        };
        if !valid_hex(trace_id, 32)
            || !valid_hex(span_id, 16)
            || !valid_hex(flags, 2)
            || trace_id.bytes().all(|b| b == b'0')
            || span_id.bytes().all(|b| b == b'0')
        {
            return None;
        }
        // A tracestate that is too long or splits a header line is vendor data only: drop it, keep the trace.
        let tracestate = if tracestate.len() > 512 || tracestate.contains(['\r', '\n']) {
            ""
        } else {
            tracestate
        };
        Some(Self {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            sampled: u8::from_str_radix(flags, 16).ok()? & 1 != 0,
            tracestate: tracestate.to_string(),
            local: false,
        })
    }

    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-{:02x}", self.trace_id, self.span_id, u8::from(self.sampled))
    }

    pub fn tracestate(&self) -> &str {
        &self.tracestate
    }
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

thread_local! {
    static CURRENT: RefCell<Option<TraceContext>> = const { RefCell::new(None) };
    static SUPPRESSED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static ARTIFACT_SUPPRESSED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// The first reason a store gave for not persisting part of the batch this thread is exporting.
    static WRITE_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// A store that persisted only part of a telemetry batch names its first error here, rather than logging each
/// document. The exporting worker logs it once, rate-limited, with the count of documents not persisted.
pub(crate) fn note_write_error(error: &str) {
    WRITE_ERROR.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = Some(error.chars().take(1024).collect());
        }
    });
}

pub(crate) fn take_write_error() -> Option<String> {
    WRITE_ERROR.with(|slot| slot.borrow_mut().take())
}

pub fn capture() -> Option<TraceContext> {
    CURRENT.with(|context| context.borrow().clone())
}

/// Restore this guard on the same thread; never hold it across an async suspension.
pub struct ContextGuard {
    previous: Option<TraceContext>,
    _thread: PhantomData<Rc<()>>,
}

pub fn attach(context: Option<TraceContext>) -> ContextGuard {
    ContextGuard {
        previous: CURRENT.with(|current| current.replace(context)),
        _thread: PhantomData,
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        CURRENT.with(|context| *context.borrow_mut() = self.previous.take());
    }
}

impl ContextGuard {
    pub(crate) fn restore_parent(&self) {
        CURRENT.with(|context| *context.borrow_mut() = self.previous.clone());
    }
}

pub struct SuppressionGuard {
    artifacts_only: bool,
    _thread: PhantomData<Rc<()>>,
}

pub fn suppress() -> SuppressionGuard {
    SUPPRESSED.with(|depth| depth.set(depth.get().saturating_add(1)));
    SuppressionGuard {
        artifacts_only: false,
        _thread: PhantomData,
    }
}

pub(crate) fn suppress_artifacts() -> SuppressionGuard {
    ARTIFACT_SUPPRESSED.with(|depth| depth.set(depth.get().saturating_add(1)));
    SuppressionGuard {
        artifacts_only: true,
        _thread: PhantomData,
    }
}

impl Drop for SuppressionGuard {
    fn drop(&mut self) {
        if self.artifacts_only {
            ARTIFACT_SUPPRESSED.with(|depth| depth.set(depth.get().saturating_sub(1)));
        } else {
            SUPPRESSED.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
}

pub fn is_suppressed() -> bool {
    SUPPRESSED.with(|depth| depth.get() != 0)
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub service_name: String,
    pub instance_id: String,
    pub version: String,
    pub revision: String,
    pub attribution_uncertain: bool,
}

impl Default for Resource {
    fn default() -> Self {
        static INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        Self {
            service_name: "moqui".to_string(),
            instance_id: INSTANCE.get_or_init(|| random_hex(16)).clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            revision: option_env!("MOQUI_GIT_REVISION").unwrap_or("unknown").to_string(),
            attribution_uncertain: true,
        }
    }
}

/// The bounds of the export queue and of the tail buffer, which holds the spans of a trace until its root decides.
/// The tail buffer as a whole shares the queue's record and byte limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity {
    /// Records waiting for the exporter (telemetry queue-records).
    pub queue_records: usize,
    /// Estimated bytes waiting for the exporter (queue-bytes).
    pub queue_bytes: usize,
    /// Traces whose spans wait for their root (tail-traces).
    pub tail_traces: usize,
    /// Spans one trace may hold there (tail-trace-spans).
    pub tail_trace_spans: usize,
}

impl Default for Capacity {
    fn default() -> Self {
        Self {
            queue_records: MAX_QUEUE,
            queue_bytes: MAX_BUFFER_BYTES,
            tail_traces: MAX_TRACES,
            tail_trace_spans: MAX_TRACE_SPANS,
        }
    }
}

impl Capacity {
    /// The records and bytes every record but a local request or job root may use.
    fn ordinary(&self) -> (usize, usize) {
        (
            self.queue_records - self.queue_records / ROOT_RESERVE_SHARE,
            self.queue_bytes - self.queue_bytes / ROOT_RESERVE_SHARE,
        )
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    /// The share of traces whose spans below the root are kept, decided from the trace id. At 0 only request and job
    /// roots are kept, with the whole trace of an error or a slow root (the tail rule).
    pub sample_ratio: f64,
    pub slow_root: Duration,
    pub interval: Duration,
    pub artifact_types: HashMap<String, (bool, bool)>,
    /// A ratio for the spans of an artifact type (telemetry/artifact sample-ratio), in place of [`Self::sample_ratio`].
    pub artifact_sample_ratios: HashMap<String, f64>,
    /// The attributes an instrument keeps (telemetry/metric attributes), in place of the global list.
    pub metric_attributes: HashMap<String, Vec<String>>,
    pub capacity: Capacity,
    /// Configuration problems, logged once when the factory's telemetry starts.
    warnings: Vec<String>,
    legacy_slow_types: HashSet<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: cfg!(feature = "telemetry"),
            sample_ratio: 0.0,
            slow_root: Duration::from_secs(1),
            interval: Duration::from_secs(60),
            artifact_types: HashMap::from([
                ("AT_ENTITY".into(), (false, false)),
                ("AT_XML_SCREEN_CONTENT".into(), (false, true)),
            ]),
            artifact_sample_ratios: HashMap::new(),
            metric_attributes: HashMap::new(),
            capacity: Capacity::default(),
            warnings: Vec::new(),
            legacy_slow_types: HashSet::new(),
        }
    }
}

impl Config {
    pub fn from_conf(text: &str) -> Self {
        let mut config = Self::default();
        let Ok(root) = MNode::parse_shared("telemetry-conf", text) else {
            return config;
        };
        let setting = |node: &MNode, name: &str| {
            node.attribute(name)
                .map(|value| moqui_lib::env::expand(&value).unwrap_or(value))
        };
        if let Some(stats) = root.first_named("server-stats") {
            if stats
                .attribute("stats-skip-condition")
                .is_some_and(|value| !value.trim().is_empty())
            {
                config.warnings.push(
                    "server-stats stats-skip-condition is deprecated and not read; configure artifact telemetry under telemetry".into(),
                );
            }
            let mut legacy = false;
            for item in stats.named_iter("artifact-stats") {
                if let Some(kind) = item.attribute("type") {
                    let values = (
                        setting(&item, "persist-hit").as_deref() == Some("true"),
                        setting(&item, "persist-bin").as_deref() == Some("true"),
                    );
                    legacy = true;
                    if values == (false, true) {
                        config.legacy_slow_types.insert(kind.clone());
                    }
                    config.artifact_types.insert(kind, values);
                }
            }
            if legacy {
                config.warnings.push(
                    "artifact-stats is deprecated; configure artifact spans and metrics under telemetry instead".into(),
                );
            }
        }
        if let Some(node) = root.first_named("telemetry") {
            config.enabled &= setting(&node, "enabled").as_deref() != Some("false");
            if let Some(text) = setting(&node, "sample-ratio").filter(|text| !text.trim().is_empty()) {
                match ratio(&text) {
                    Some(value) => config.sample_ratio = value,
                    None => config.warnings.push(format!(
                        "telemetry sample-ratio [{text}] is not a number; using {}",
                        config.sample_ratio
                    )),
                }
            }
            // A size that is not a whole number, or is below its floor, is warned about and the default kept.
            for (name, slot, floor) in [
                ("queue-records", &mut config.capacity.queue_records, 64),
                ("queue-bytes", &mut config.capacity.queue_bytes, 1024 * 1024),
                ("tail-traces", &mut config.capacity.tail_traces, 1),
                ("tail-trace-spans", &mut config.capacity.tail_trace_spans, 1),
            ] {
                let Some(text) = setting(&node, name).filter(|text| !text.trim().is_empty()) else {
                    continue;
                };
                match text.trim().parse::<usize>().ok().filter(|value| *value >= floor) {
                    Some(value) => *slot = value,
                    None => config.warnings.push(format!(
                        "telemetry {name} [{text}] is not a whole number of at least {floor}; using {slot}"
                    )),
                }
            }
            for item in node.named_iter("metric") {
                let Some(name) = item.attribute("name").filter(|name| !name.is_empty()) else {
                    config
                        .warnings
                        .push("telemetry metric without a name is ignored".into());
                    continue;
                };
                let attributes = setting(&item, "attributes").unwrap_or_default();
                config.metric_attributes.insert(
                    name,
                    attributes
                        .split(',')
                        .map(str::trim)
                        .filter(|key| !key.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
            }
            // Metric documents are minute documents: the rollups and the dashboards' buckets assume it, so another
            // interval is for tests.
            config.interval = Duration::from_secs(
                setting(&node, "interval-seconds")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(60)
                    .max(1),
            );
            if config.interval != Duration::from_secs(60) {
                config.warnings.push(format!(
                    "telemetry interval-seconds is {}; metric documents are minute documents, so use 60 outside tests",
                    config.interval.as_secs()
                ));
            }
            for item in node.named_iter("artifact") {
                if let Some(kind) = item.attribute("type") {
                    config.legacy_slow_types.remove(&kind);
                    config.artifact_types.insert(
                        kind.clone(),
                        (
                            setting(&item, "spans").as_deref() != Some("false"),
                            setting(&item, "metrics").as_deref() != Some("false"),
                        ),
                    );
                    match setting(&item, "sample-ratio").filter(|text| !text.trim().is_empty()) {
                        Some(text) => match ratio(&text) {
                            Some(value) => {
                                config.artifact_sample_ratios.insert(kind, value);
                            }
                            None => config.warnings.push(format!(
                                "telemetry artifact {kind} sample-ratio [{text}] is not a number; using the telemetry sample-ratio"
                            )),
                        },
                        None => {
                            config.artifact_sample_ratios.remove(&kind);
                        }
                    }
                }
            }
        }
        config
    }

    /// The head sampling ratio of an artifact type's spans.
    pub fn artifact_sample_ratio(&self, kind: &str) -> f64 {
        self.artifact_sample_ratios
            .get(kind)
            .copied()
            .unwrap_or(self.sample_ratio)
    }
}

fn ratio(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0))
}

pub fn for_factory(factory: &ExecutionContextFactory) -> Arc<Telemetry> {
    let slot = factory.extension(|_| std::sync::OnceLock::<Arc<Telemetry>>::new());
    let telemetry = Arc::clone(slot.get_or_init(|| {
        let mut resource = Resource::default();
        let configured = moqui_lib::env::get_prop_or_env("instance_id");
        let known = crate::cluster::instance_id(factory)
            .or_else(|| (!configured.is_empty()).then_some(configured))
            .or_else(|| {
                std::fs::read_to_string(factory.runtime_dir().join("db/instance_id"))
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            });
        if let Some(instance) = known {
            resource.instance_id = instance;
            resource.attribution_uncertain = false;
        }
        let name = factory.conf_setting("telemetry", "service-name");
        if !name.is_empty() {
            resource.service_name = name;
        }
        Arc::new(Telemetry::from_conf(factory.conf_text(), resource))
    }));
    if !telemetry.shared.config.warnings.is_empty() && !telemetry.shared.warned_legacy.swap(true, Ordering::Relaxed) {
        for warning in &telemetry.shared.config.warnings {
            tracing::warn!("{warning}");
        }
    }
    telemetry
}

pub fn current() -> Option<Arc<Telemetry>> {
    #[cfg(feature = "telemetry")]
    {
        if is_suppressed() {
            return None;
        }
        ExecutionContext::with_current(ExecutionContext::telemetry).filter(|telemetry| telemetry.shared.config.enabled)
    }
    #[cfg(not(feature = "telemetry"))]
    {
        None
    }
}

/// An exporter is installed after SearchFacade starts. All methods run on the telemetry worker.
pub trait Exporter: Send + Sync {
    fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error>;
    fn export_received(&self, index: &str, documents: Vec<Document>) -> Result<usize, Error> {
        self.export(index, documents)
    }
    fn upsert_error(&self, document: Document) -> Result<(), Error> {
        self.export("otel_errors", vec![document]).map(|_| ())
    }
}

/// `dropped` counts every record lost, for any reason, an error group's update among them. `failed` counts those of
/// them an exporter did not persist (the store refused them, or the exporter failed or panicked), so an outage shows
/// apart from overload.
#[derive(Default)]
struct Counters {
    queued: AtomicU64,
    exported: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
    /// Failed export calls, which rate-limit their log lines.
    export_errors: AtomicU64,
    remote_exported: AtomicU64,
    remote_dropped: AtomicU64,
    remote_failed: AtomicU64,
}

/// See [`Counters`]: `failed` records are also in `dropped`.
#[derive(Clone, Debug, Default)]
pub struct CounterSnapshot {
    pub queued: u64,
    pub exported: u64,
    pub dropped: u64,
    pub failed: u64,
    pub remote_exported: u64,
    pub remote_dropped: u64,
    pub remote_failed: u64,
}

enum Command {
    #[cfg(feature = "telemetry")]
    Sdk(sdk::ExportBatch, usize),
    Documents(&'static str, Vec<Document>, usize, bool, usize),
    Install(Arc<dyn Exporter>),
    Flush(mpsc::SyncSender<()>),
    Stop,
}

// Administrative callers may wait for queue capacity; business producers remain
// nonblocking. Enqueue and acknowledgement share one bounded deadline.
fn flush_barrier(queue: &mpsc::SyncSender<Command>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut command = Command::Flush(sender);
    loop {
        match queue.try_send(command) {
            Ok(()) => {
                return receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_ok()
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
            Err(mpsc::TrySendError::Full(returned)) => command = returned,
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(1)));
    }
}

struct Shared {
    sender: mpsc::SyncSender<Command>,
    counters: Arc<Counters>,
    queued_records: AtomicUsize,
    queued_bytes: AtomicUsize,
    config: Config,
    resource: Mutex<Resource>,
    stopped: AtomicBool,
    drained: AtomicBool,
    warned_legacy: AtomicBool,
    gauges_registered: AtomicBool,
    slow: Mutex<HashMap<u64, RunningStats>>,
    /// Running totals of received metrics, for `/metrics`.
    received: Mutex<received::Received>,
}

pub struct Telemetry {
    shared: Arc<Shared>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    #[cfg(feature = "telemetry")]
    sdk: Option<sdk::Runtime>,
}

impl Telemetry {
    pub fn from_conf(text: &str, resource: Resource) -> Self {
        let config = Config::from_conf(text);
        let (sender, receiver) = mpsc::sync_channel(config.capacity.queue_records);
        let shared = Arc::new(Shared {
            sender,
            counters: Arc::default(),
            queued_records: AtomicUsize::new(0),
            queued_bytes: AtomicUsize::new(0),
            config,
            resource: Mutex::new(resource),
            stopped: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            warned_legacy: AtomicBool::new(false),
            gauges_registered: AtomicBool::new(false),
            slow: Mutex::default(),
            received: Mutex::default(),
        });
        let worker = if shared.config.enabled {
            let state = Arc::clone(&shared);
            match std::thread::Builder::new()
                .name("moqui-telemetry".to_string())
                .spawn(move || worker(state, receiver))
            {
                Ok(handle) => Some(handle),
                Err(error) => {
                    // Recording stays nonblocking; the queue fills and counts what it drops.
                    tracing::error!(%error, "telemetry worker did not start; telemetry will be dropped");
                    None
                }
            }
        } else {
            None
        };
        #[cfg(feature = "telemetry")]
        let sdk = shared.config.enabled.then(|| sdk::Runtime::new(Arc::clone(&shared)));
        Self {
            shared,
            worker: Mutex::new(worker),
            #[cfg(feature = "telemetry")]
            sdk,
        }
    }

    pub fn config(&self) -> &Config {
        &self.shared.config
    }
    pub fn resource(&self) -> Resource {
        lock(&self.shared.resource).clone()
    }

    pub fn set_instance_id(&self, instance_id: String) {
        if instance_id.is_empty() {
            return;
        }
        let mut resource = lock(&self.shared.resource);
        resource.instance_id = instance_id;
        resource.attribution_uncertain = false;
    }

    pub fn note_dropped(&self, count: u64) {
        self.shared.counters.dropped.fetch_add(count, Ordering::Relaxed);
    }
    pub fn note_remote_export(&self, exported: u64, dropped: u64, failed: bool) {
        self.shared
            .counters
            .remote_exported
            .fetch_add(exported, Ordering::Relaxed);
        self.shared
            .counters
            .remote_dropped
            .fetch_add(dropped, Ordering::Relaxed);
        if failed {
            self.shared.counters.remote_failed.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn set_exporter(&self, exporter: Arc<dyn Exporter>) {
        if self.shared.config.enabled {
            let _ = self.shared.sender.send(Command::Install(exporter));
        }
    }

    pub fn start_span(&self, name: &str, kind: SpanKind, parent: Option<TraceContext>) -> Span {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                return sdk.start_span(
                    name,
                    kind,
                    parent.or_else(capture),
                    None,
                    false,
                    self.shared.config.sample_ratio,
                );
            }
        }
        let _ = (name, kind, parent);
        Span::noop()
    }

    /// A child that keeps its local parent's head decision: a sampled frame keeps its statements, an unsampled one
    /// leaves them to tail promotion (errors and slow roots). A parent extracted from a caller uses the global ratio,
    /// so a caller's flag cannot turn on full tracing.
    pub fn start_child_span(&self, name: &str, kind: SpanKind, parent: TraceContext) -> Span {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                let ratio = match (parent.local, parent.sampled) {
                    (true, true) => 1.0,
                    (true, false) => 0.0,
                    (false, _) => self.shared.config.sample_ratio,
                };
                return sdk.start_span(name, kind, Some(parent), None, false, ratio);
            }
        }
        let _ = (name, kind, parent);
        Span::noop()
    }

    /// A root of its own trace that is always head-sampled, so its children are kept with it: for low-volume work
    /// whose every trace is wanted whole, such as a synthetic check.
    pub fn start_sampled_root(&self, name: &str, kind: SpanKind) -> Span {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                return sdk.start_span(name, kind, None, None, false, 1.0);
            }
        }
        let _ = (name, kind);
        Span::noop()
    }

    /// A span of an artifact frame, head-sampled by its type's ratio (telemetry/artifact sample-ratio) when it has one.
    pub fn start_artifact_span(&self, artifact_type: &str, name: &str, kind: SpanKind) -> Span {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                let ratio = self.shared.config.artifact_sample_ratio(artifact_type);
                return sdk.start_span(name, kind, capture(), None, false, ratio);
            }
        }
        let _ = (artifact_type, name, kind);
        Span::noop()
    }

    /// Legacy persist-bin settings retain statistically slow hits over the old one-second
    /// impact threshold even when ordinary hit persistence was disabled.
    pub(crate) fn legacy_slow_enabled(&self, kind: &str) -> bool {
        self.artifact_enabled(kind) == (false, true) && self.shared.config.legacy_slow_types.contains(kind)
    }

    pub(crate) fn start_legacy_slow_span(&self, name: &str, parent: Option<TraceContext>, started: SystemTime) -> Span {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                let ratio = self.shared.config.sample_ratio;
                return sdk.start_span(name, SpanKind::Internal, parent, Some(started), true, ratio);
            }
        }
        let _ = (name, parent, started);
        Span::noop()
    }

    pub fn record_log(&self, record: LogRecord) {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                sdk.record_log(record);
                return;
            }
        }
        let _ = record;
    }

    pub fn record_duration(&self, name: &str, seconds: f64, attributes: Document) {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                sdk.record_duration(name, seconds, attributes);
                return;
            }
        }
        let _ = (name, seconds, attributes);
    }

    pub fn add_counter(&self, name: &str, value: u64, attributes: Document) {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled && !is_suppressed() {
            if let Some(sdk) = &self.sdk {
                sdk.add_counter(name, value, attributes);
                return;
            }
        }
        let _ = (name, value, attributes);
    }

    /// Callbacks must use weak factory references so observation never extends its lifetime.
    pub fn observe_gauge(&self, name: &str, callback: impl Fn() -> Option<f64> + Send + Sync + 'static) {
        #[cfg(feature = "telemetry")]
        if let Some(sdk) = &self.sdk {
            sdk.observe_gauge(name, callback);
            return;
        }
        let _ = (name, callback);
    }

    /// External records have already passed authentication and redaction in the receiver.
    pub fn ingest(&self, index: &'static str, documents: Vec<Document>) {
        let _ = self.try_ingest(index, documents);
    }

    pub fn try_ingest(&self, index: &'static str, documents: Vec<Document>) -> Result<(), Error> {
        if !self.accepting() {
            return Err(Error::new("telemetry is disabled"));
        }
        if index == "otel_metrics" {
            lock(&self.shared.received).record(&documents);
        }
        self.shared.try_enqueue(index, documents, false)
    }

    /// Whether telemetry is enabled and not shut down, so received records can be queued.
    pub fn accepting(&self) -> bool {
        self.shared.config.enabled && !self.shared.stopped.load(Ordering::Acquire)
    }

    /// The SDK's instruments as Prometheus text (exposition format 0.0.4), cumulative since start, then the metrics
    /// received over OTLP and from scrape targets (a received family named like one of this server's is left out);
    /// None when telemetry is off or its metrics cannot be collected.
    pub fn prometheus_text(&self) -> Option<String> {
        #[cfg(feature = "telemetry")]
        if self.shared.config.enabled {
            if let Some(sdk) = &self.sdk {
                let mut text = sdk.prometheus_text()?;
                let received = prometheus::render_received(&lock(&self.shared.received), &text);
                text.push_str(&received);
                return Some(text);
            }
        }
        None
    }

    pub fn counters(&self) -> CounterSnapshot {
        let counters = &self.shared.counters;
        CounterSnapshot {
            queued: counters.queued.load(Ordering::Relaxed),
            exported: counters.exported.load(Ordering::Relaxed),
            dropped: counters.dropped.load(Ordering::Relaxed),
            failed: counters.failed.load(Ordering::Relaxed),
            remote_exported: counters.remote_exported.load(Ordering::Relaxed),
            remote_dropped: counters.remote_dropped.load(Ordering::Relaxed),
            remote_failed: counters.remote_failed.load(Ordering::Relaxed),
        }
    }

    pub fn flush(&self) {
        let _ = self.flush_complete();
    }

    /// Request SDK collection and wait for the accepted queue's bounded flush
    /// barrier. Transport failures/loss remain in `counters`; a false result
    /// means the barrier was not acknowledged, rather than a business error.
    pub fn flush_complete(&self) -> bool {
        if !self.shared.config.enabled {
            return true;
        }
        if self.shared.stopped.load(Ordering::Relaxed) {
            return false;
        }
        #[cfg(feature = "telemetry")]
        if let Some(sdk) = &self.sdk {
            sdk.flush();
        }
        flush_barrier(&self.shared.sender, Duration::from_secs(5))
    }

    pub fn shutdown(&self) {
        if self.shared.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        #[cfg(feature = "telemetry")]
        if let Some(sdk) = &self.sdk {
            sdk.shutdown();
        }
        self.shared.drained.store(true, Ordering::Release);
        let _ = self.shared.sender.try_send(Command::Stop);
        // Exporters use bounded transport deadlines. Detach rather than block business shutdown forever.
        if let Some(handle) = lock(&self.worker).take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }

    pub fn artifact_enabled(&self, kind: &str) -> (bool, bool) {
        if !self.shared.config.enabled || is_suppressed() || ARTIFACT_SUPPRESSED.with(|depth| depth.get() != 0) {
            return (false, false);
        }
        self.shared
            .config
            .artifact_types
            .get(kind)
            .copied()
            .unwrap_or((true, true))
    }

    pub fn record_artifact(&self, kind: &str, name: &str, action: &str, sub_type: Option<&str>, seconds: f64) -> bool {
        let (_, metrics) = match self.artifact_enabled(kind) {
            (false, false) => return false,
            enabled => enabled,
        };
        // A fixed hash of the parts keys the running statistics, so a frame allocates no key.
        let key = {
            use std::hash::BuildHasher;
            std::hash::BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default()
                .hash_one((kind, name, action))
        };
        let slow = {
            let mut stats = lock(&self.shared.slow);
            if stats.len() >= MAX_SERIES && !stats.contains_key(&key) {
                false
            } else {
                stats.entry(key).or_default().record(seconds)
            }
        };
        if metrics {
            let mut attributes = Document::new();
            attributes.insert("moqui.artifact.type".into(), Value::string(kind));
            attributes.insert("moqui.artifact.name".into(), Value::string(name));
            attributes.insert("moqui.artifact.action".into(), Value::string(action));
            if let Some(sub_type) = sub_type {
                attributes.insert("moqui.artifact.sub_type".into(), Value::string(sub_type));
            }
            if slow {
                self.record_duration("moqui.artifact.duration", seconds, attributes.clone());
                self.add_counter("moqui.artifact.slow", 1, attributes);
            } else {
                self.record_duration("moqui.artifact.duration", seconds, attributes);
            }
        }
        slow
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn register_runtime_gauges(factory: &Arc<ExecutionContextFactory>) {
    let telemetry = for_factory(factory);
    if telemetry.shared.gauges_registered.swap(true, Ordering::AcqRel) {
        return;
    }
    for (name, which) in [
        ("moqui.db.pool.checked_out", 0),
        ("moqui.db.pool.parked", 1),
        ("moqui.db.pool.waiting", 2),
        ("moqui.db.pool.idle", 3),
    ] {
        let weak = Arc::downgrade(factory);
        telemetry.observe_gauge(name, move || {
            let factory = weak.upgrade()?;
            let facade = factory.entity()?;
            let stats = facade.transactions_for("moqui.basic.Enumeration").ok()?.stats();
            Some(match which {
                0 => stats.checked_out,
                1 => stats.parked,
                2 => stats.waiting,
                _ => stats.ordinary_idle + stats.reserve_idle,
            } as f64)
        });
    }
    let weak = Arc::downgrade(factory);
    telemetry.observe_gauge("moqui.request.queue.depth", move || {
        Some(weak.upgrade()?.request_pool().queue_depth() as f64)
    });
    for (name, key) in [
        ("process.memory.usage", "MemoryUsed"),
        ("process.thread.count", "Threads"),
    ] {
        let weak = Arc::downgrade(factory);
        telemetry.observe_gauge(name, move || {
            let factory = weak.upgrade()?;
            let status = crate::context::status::status_map(&factory, false);
            let Value::Map(process) = status.get("Process")? else {
                return None;
            };
            let value = process.get(key)?;
            if value.is_null() {
                return None;
            }
            let value = moqui_lib::object::to_plain_string(value).parse::<f64>().ok()?;
            Some(if key == "MemoryUsed" {
                value * 1024.0 * 1024.0
            } else {
                value
            })
        });
    }
    for (name, which) in [("moqui.telemetry.dropped", 0), ("moqui.telemetry.failed", 1)] {
        let weak = Arc::downgrade(&telemetry);
        telemetry.observe_gauge(name, move || {
            let telemetry = weak.upgrade()?;
            let counters = telemetry.counters();
            Some(if which == 0 { counters.dropped } else { counters.failed } as f64)
        });
    }
}

impl Shared {
    #[cfg(feature = "telemetry")]
    fn enqueue(&self, index: &'static str, documents: Vec<Document>) {
        if documents.is_empty() || !self.config.enabled {
            return;
        }
        let mut documents = documents.into_iter();
        loop {
            let batch: Vec<_> = documents.by_ref().take(512).collect();
            if batch.is_empty() {
                break;
            }
            let _ = self.try_enqueue(index, batch, true);
        }
    }

    fn try_enqueue(&self, index: &'static str, documents: Vec<Document>, local: bool) -> Result<(), Error> {
        if local && index == "otel_spans" && documents.iter().any(mandatory_root) {
            if documents.iter().all(mandatory_root) {
                return self.send_documents(index, documents, local, true);
            }
            // Preserve child evidence before a normal root's tail decision. Only
            // contiguous runs share an admission class; failed children do not
            // stop a following mandatory root from using its reserve.
            let mut result = Ok(());
            let mut run = Vec::new();
            let mut root = mandatory_root(&documents[0]);
            for document in documents {
                let next = mandatory_root(&document);
                if next != root {
                    let sent = self.send_documents(index, std::mem::take(&mut run), local, root);
                    result = result.and(sent);
                    root = next;
                }
                run.push(document);
            }
            return result.and(self.send_documents(index, run, local, root));
        }
        self.send_documents(index, documents, local, false)
    }

    fn send_documents(
        &self,
        index: &'static str,
        documents: Vec<Document>,
        local: bool,
        root: bool,
    ) -> Result<(), Error> {
        let count = documents.len();
        if count == 0 {
            return Ok(());
        }
        let bytes = documents
            .iter()
            .map(document_bytes)
            .fold(0_usize, usize::saturating_add);
        self.send_records(
            count,
            bytes,
            Command::Documents(index, documents, bytes, local, count),
            root,
        )
    }

    #[cfg(feature = "telemetry")]
    fn enqueue_sdk(&self, batch: sdk::ExportBatch) {
        if let Some(root) = batch.uniform_root_priority() {
            self.enqueue_sdk_batch(batch, root);
        } else {
            for (batch, root) in batch.into_priority_runs() {
                self.enqueue_sdk_batch(batch, root);
            }
        }
    }

    #[cfg(feature = "telemetry")]
    fn enqueue_sdk_batch(&self, batch: sdk::ExportBatch, root: bool) {
        let count = batch.len();
        if count == 0 || !self.config.enabled {
            return;
        }
        let bytes = batch.estimated_bytes();
        let _ = self.send_records(count, bytes, Command::Sdk(batch, bytes), root);
    }

    fn send_records(&self, count: usize, bytes: usize, command: Command, root: bool) -> Result<(), Error> {
        #[allow(deprecated)] // fetch_update remains available on the workspace's Rust 1.95 minimum.
        let reserve = |counter: &AtomicUsize, value: usize, max: usize| {
            counter
                .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |current| {
                    current.checked_add(value).filter(|next| *next <= max)
                })
                .is_ok()
        };
        let capacity = &self.config.capacity;
        let (ordinary_records, ordinary_bytes) = capacity.ordinary();
        if !reserve(
            &self.queued_records,
            count,
            if root { capacity.queue_records } else { ordinary_records },
        ) {
            self.counters.dropped.fetch_add(count as u64, Ordering::Relaxed);
            return Err(Error::new("telemetry queue record limit reached"));
        }
        if !reserve(
            &self.queued_bytes,
            bytes,
            if root { capacity.queue_bytes } else { ordinary_bytes },
        ) {
            self.queued_records.fetch_sub(count, Ordering::AcqRel);
            self.counters.dropped.fetch_add(count as u64, Ordering::Relaxed);
            return Err(Error::new("telemetry queue byte limit reached"));
        }
        match self.sender.try_send(command) {
            Ok(()) => {
                self.counters.queued.fetch_add(count as u64, Ordering::Relaxed);
                Ok(())
            }
            Err(_) => {
                self.queued_records.fetch_sub(count, Ordering::AcqRel);
                self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                self.counters.dropped.fetch_add(count as u64, Ordering::Relaxed);
                Err(Error::new("telemetry queue is full"))
            }
        }
    }

    #[cfg(feature = "telemetry")]
    fn add_resource(&self, document: &mut Document) {
        let resource = lock(&self.resource);
        let mut attributes = match document.get("resource") {
            Some(Value::Map(attributes)) => attributes.clone(),
            _ => Document::new(),
        };
        for (key, value) in [
            ("service.name", &resource.service_name),
            ("service.instance.id", &resource.instance_id),
            ("service.version", &resource.version),
            ("vcs.ref.head.revision", &resource.revision),
        ] {
            document.insert(key.to_string(), Value::string(value));
            attributes.insert(key.to_string(), Value::string(value));
        }
        document.insert(
            "moqui.identity.uncertain".into(),
            Value::Bool(resource.attribution_uncertain),
        );
        attributes.insert(
            "moqui.identity.uncertain".into(),
            Value::Bool(resource.attribution_uncertain),
        );
        document.insert("resource".into(), Value::Map(attributes));
        document.insert("source.name".into(), Value::string("local"));
        document.insert("source.id".into(), Value::string("local"));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    Server,
    Internal,
    Client,
    Producer,
    Consumer,
}

impl SpanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Server => "SERVER",
            Self::Internal => "INTERNAL",
            Self::Client => "CLIENT",
            Self::Producer => "PRODUCER",
            Self::Consumer => "CONSUMER",
        }
    }
}

/// Owned and Send; attaching the context is an explicit, separate operation.
pub struct Span {
    context: Option<TraceContext>,
    #[cfg(feature = "telemetry")]
    inner: Option<sdk::OwnedSpan>,
}

impl Span {
    pub(crate) fn noop() -> Self {
        Self {
            context: None,
            #[cfg(feature = "telemetry")]
            inner: None,
        }
    }
    pub fn context(&self) -> Option<TraceContext> {
        self.context.clone()
    }
    pub fn is_recording(&self) -> bool {
        self.context.is_some()
    }
    pub fn set_attribute(&mut self, key: &str, value: Value) {
        #[cfg(feature = "telemetry")]
        if let Some(span) = &mut self.inner {
            span.set_attribute(key, value);
            return;
        }
        let _ = (key, value);
    }
    pub fn set_error(&mut self, message: &str) {
        #[cfg(feature = "telemetry")]
        if let Some(span) = &mut self.inner {
            span.set_error(message);
            return;
        }
        let _ = message;
    }
    pub fn suppress(&mut self) {
        #[cfg(feature = "telemetry")]
        if let Some(span) = &mut self.inner {
            span.suppress();
        }
    }
    pub fn finish(self) {
        #[cfg(feature = "telemetry")]
        if let Some(span) = self.inner {
            span.finish();
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogRecord {
    pub timestamp: i64,
    pub severity_number: i32,
    pub severity_text: String,
    pub body: String,
    pub scope_name: String,
    pub attributes: Document,
    pub context: Option<TraceContext>,
}

impl LogRecord {
    pub fn new(severity_number: i32, severity_text: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            timestamp: crate::user::now_millis(),
            severity_number,
            severity_text: severity_text.into(),
            body: body.into(),
            scope_name: String::new(),
            attributes: Document::new(),
            context: capture(),
        }
    }
}

#[derive(Default)]
struct RunningStats {
    count: u64,
    total: f64,
    squared: f64,
}
impl RunningStats {
    fn record(&mut self, value: f64) -> bool {
        self.count += 1;
        let count = self.count as f64;
        let threshold = if self.count >= 50 {
            self.total / count + 2.6 * ((self.squared - self.total * self.total / count).abs() / (count - 1.0)).sqrt()
        } else {
            0.0
        };
        let slow = self.count >= 50 && threshold != 0.0 && value > threshold;
        if self.count == 2 && self.total > value * 3.0 {
            self.total = value * 2.0;
            self.squared = value * value * 2.0;
        } else {
            self.total += value;
            self.squared += value * value;
        }
        slow
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    if getrandom::fill(&mut buffer).is_err() {
        return format!("{:0width$x}", crate::user::now_millis(), width = bytes * 2);
    }
    buffer.iter().map(|value| format!("{value:02x}")).collect()
}

pub use crate::entity::exec::redact_sql;

/// Conservative masking for credential assignments in messages and URL query strings.
fn credential_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(||regex::Regex::new(r#"(?i)((?:password|authorization|api[_\.-]?key|access[_-]?token|refresh[_-]?token|token|secret)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|(?:Bearer\s+)?[^\s,;&]+)"#).expect("constant credential regex"))
}
pub fn redact_text(value: &str) -> String {
    credential_pattern().replace_all(value, "$1\"[redacted]\"").into_owned()
}
#[cfg(feature = "telemetry")]
fn redact_owned(value: String) -> String {
    match credential_pattern().replace_all(&value, "$1\"[redacted]\"") {
        std::borrow::Cow::Borrowed(_) => value,
        std::borrow::Cow::Owned(value) => value,
    }
}

/// Whether an attribute, parameter or query name names a secret: the framework's one rule,
/// [`crate::context::secret_parameter`], which the HTTP log's query redaction uses too.
pub fn sensitive_key(key: &str) -> bool {
    crate::context::secret_parameter(key)
}

fn text(document: &Document, key: &str) -> String {
    document
        .get(key)
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default()
}
fn number(document: &Document, key: &str) -> f64 {
    match document.get(key) {
        Some(Value::I64(value)) => *value as f64,
        Some(Value::I32(value)) => f64::from(*value),
        Some(Value::F64(value)) => *value,
        Some(Value::String(value)) => value.parse().unwrap_or(0.0),
        Some(value) => moqui_lib::object::to_plain_string(value).parse().unwrap_or(0.0),
        None => 0.0,
    }
}
fn string_field<'a>(document: &'a Document, key: &str) -> &'a str {
    document.get(key).and_then(Value::as_str).unwrap_or("")
}
fn true_field(document: &Document, key: &str) -> bool {
    matches!(document.get(key), Some(Value::Bool(true))) || string_field(document, key) == "true"
}

fn mandatory_root(document: &Document) -> bool {
    string_field(document, "kind") == "SERVER"
        || true_field(document, "moqui.job.root")
        || true_field(document, "moqui.synthetic.check")
}

/// Tail promotion may make a large batch. Charge mandatory local roots first,
/// retaining the original document order while dropping only excess children.
fn admit_ready(
    ready: &mut Vec<Document>,
    local_roots: bool,
    capacity: &Capacity,
    count: &mut usize,
    bytes: &mut usize,
) -> usize {
    let original = ready.len();
    let mut admit = |document: &Document, record_limit: usize, byte_limit: usize| {
        let document_bytes = document_bytes(document);
        if *count < record_limit && bytes.checked_add(document_bytes).is_some_and(|next| next <= byte_limit) {
            *count += 1;
            *bytes += document_bytes;
            true
        } else {
            false
        }
    };
    if local_roots {
        ready.retain(|document| {
            !mandatory_root(document) || admit(document, capacity.queue_records, capacity.queue_bytes)
        });
    }
    let (ordinary_records, ordinary_bytes) = capacity.ordinary();
    ready.retain(|document| {
        (local_roots && mandatory_root(document)) || admit(document, ordinary_records, ordinary_bytes)
    });
    original - ready.len()
}

fn has_exception(document: &Document) -> bool {
    string_field(document, "status_code") == "ERROR"
        || document.contains_key("exception.type")
        || document.contains_key("exception.message")
        || document.contains_key("exception.stacktrace")
}

fn document_bytes(document: &Document) -> usize {
    fn value_bytes(value: &Value) -> usize {
        128_usize.saturating_add(match value {
            Value::String(value) => value.len(),
            Value::Bytes(value) => value.len(),
            Value::List(values) => values.iter().map(value_bytes).fold(0, usize::saturating_add),
            Value::Map(values) => document_bytes(values),
            _ => 0,
        })
    }
    document
        .iter()
        .map(|(key, value)| key.len().saturating_add(value_bytes(value)))
        .fold(128, usize::saturating_add)
}

/// How long a tail decision, or a counted error occurrence, is remembered for spans that end after their root.
const RECENT_AGE: Duration = Duration::from_secs(120);

/// Values by trace-derived key, remembered for [`RECENT_AGE`] and at most [`MAX_DECISIONS`]. Full, it forgets the
/// oldest first, which is also the first to expire, so the recent entries of a busy instance stay.
#[derive(Default)]
struct Recent {
    values: HashMap<String, (bool, Instant)>,
    order: VecDeque<(String, Instant)>,
}

impl Recent {
    fn get(&self, key: &str) -> Option<bool> {
        self.values.get(key).map(|(value, _)| *value)
    }

    fn insert(&mut self, key: &str, value: bool, now: Instant) {
        while self.values.len() >= MAX_DECISIONS || self.order.len() >= 2 * MAX_DECISIONS {
            if !self.forget_oldest() {
                break;
            }
        }
        self.values.insert(key.to_string(), (value, now));
        self.order.push_back((key.to_string(), now));
    }

    fn expire(&mut self, now: Instant) {
        while self
            .order
            .front()
            .is_some_and(|(_, at)| now.duration_since(*at) >= RECENT_AGE)
        {
            self.forget_oldest();
        }
    }

    /// The order may hold an older entry of a key inserted again; that one leaves the value alone.
    fn forget_oldest(&mut self) -> bool {
        let Some((key, at)) = self.order.pop_front() else {
            return false;
        };
        if self.values.get(&key).is_some_and(|(_, inserted)| *inserted == at) {
            self.values.remove(&key);
        }
        true
    }
}

fn worker(shared: Arc<Shared>, receiver: mpsc::Receiver<Command>) {
    let _suppressed = suppress();
    let mut exporter: Option<Arc<dyn Exporter>> = None;
    let mut pending = Vec::<(&'static str, Vec<Document>, bool)>::new();
    let mut pending_count = 0;
    let mut pending_bytes = 0_usize;
    let mut tail_bytes = 0_usize;
    let mut tail_count = 0_usize;
    let mut traces = HashMap::<String, (Instant, Vec<Document>)>::new();
    let mut decisions = Recent::default();
    let mut counted_errors = Recent::default();
    let mut next_command = None;
    let mut last_sweep = Instant::now();
    loop {
        let command = match next_command
            .take()
            .map(Ok)
            .unwrap_or_else(|| receiver.recv_timeout(Duration::from_millis(100)))
        {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) if !shared.drained.load(Ordering::Acquire) => continue,
            Err(_) => break,
        };
        #[cfg(feature = "telemetry")]
        let command = match command {
            Command::Sdk(batch, bytes) => {
                let count = batch.len();
                let (index, documents) = batch.normalize(&shared);
                Command::Documents(index, documents, bytes, true, count)
            }
            other => other,
        };
        let mut flush = None;
        let mut stopping = false;
        match command {
            #[cfg(feature = "telemetry")]
            Command::Sdk(..) => unreachable!("SDK payload was normalized above"),
            Command::Documents(index, documents, bytes, local, queued_count) => {
                shared.queued_records.fetch_sub(queued_count, Ordering::AcqRel);
                shared.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
                let mut ready = Vec::new();
                if last_sweep.elapsed() >= Duration::from_secs(1) {
                    let now = Instant::now();
                    traces.retain(|_, (started, spans)| {
                        if now.duration_since(*started) > Duration::from_secs(120) {
                            tail_count = tail_count.saturating_sub(spans.len());
                            tail_bytes = tail_bytes.saturating_sub(spans.iter().map(document_bytes).sum::<usize>());
                            shared.counters.dropped.fetch_add(spans.len() as u64, Ordering::Relaxed);
                            false
                        } else {
                            true
                        }
                    });
                    decisions.expire(now);
                    counted_errors.expire(now);
                    last_sweep = now;
                }
                for document in documents {
                    if index == "otel_spans" && local {
                        let trace = string_field(&document, "trace_id");
                        let mandatory_root = mandatory_root(&document);
                        let root = mandatory_root || string_field(&document, "parent_span_id").is_empty();
                        let sampled = true_field(&document, "sampled");
                        let important = has_exception(&document)
                            || number(&document, "duration_nano") > shared.config.slow_root.as_nanos() as f64;
                        if root {
                            let keep = sampled
                                || important
                                || traces.get(trace).is_some_and(|(_, spans)| {
                                    spans.iter().any(|span| {
                                        has_exception(span)
                                            || (true_field(span, "moqui.artifact.slow")
                                                && number(span, "duration_nano") > 1e9)
                                    })
                                });
                            if let Some((_, spans)) = traces.remove(trace) {
                                tail_count = tail_count.saturating_sub(spans.len());
                                tail_bytes = tail_bytes.saturating_sub(spans.iter().map(document_bytes).sum::<usize>());
                                if keep {
                                    ready.extend(spans);
                                }
                            }
                            decisions.insert(trace, keep, Instant::now());
                            if mandatory_root || keep {
                                ready.push(document);
                            }
                        } else if sampled || decisions.get(trace) == Some(true) {
                            ready.push(document);
                        } else if decisions.get(trace).is_some() {
                            // The root was decided without this span. An error that ends after it, such as one
                            // in work the request queued, is still kept.
                            if has_exception(&document) {
                                ready.push(document);
                            }
                        } else {
                            let capacity = &shared.config.capacity;
                            if traces.len() < capacity.tail_traces || traces.contains_key(trace) {
                                let (_, buffered) = traces
                                    .entry(trace.to_string())
                                    .or_insert_with(|| (Instant::now(), Vec::new()));
                                let bytes = document_bytes(&document);
                                if buffered.len() < capacity.tail_trace_spans
                                    && tail_count < capacity.queue_records
                                    && tail_bytes.saturating_add(bytes) <= capacity.queue_bytes
                                {
                                    tail_count += 1;
                                    tail_bytes += bytes;
                                    buffered.push(document);
                                } else {
                                    shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
                                }
                            } else {
                                shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    } else {
                        ready.push(document);
                    }
                }
                if !ready.is_empty() {
                    let dropped = admit_ready(
                        &mut ready,
                        local && index == "otel_spans",
                        &shared.config.capacity,
                        &mut pending_count,
                        &mut pending_bytes,
                    );
                    shared.counters.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
                    if !ready.is_empty() {
                        if let Some((_, documents, _)) = pending
                            .iter_mut()
                            .find(|(queued_index, _, queued_local)| *queued_index == index && *queued_local == local)
                        {
                            documents.extend(ready);
                        } else {
                            pending.push((index, ready, local));
                        }
                    }
                }
            }
            Command::Install(installed) => {
                exporter = Some(installed);
            }
            Command::Flush(reply) => {
                flush = Some(reply);
            }
            Command::Stop => stopping = true,
        }
        // Coalesce ready work already queued by synchronous SDK processors. Bound each export
        // burst so sustained producers cannot indefinitely postpone persistence or flush barriers.
        if !stopping && flush.is_none() && pending_count < 512 {
            if let Ok(command) = receiver.try_recv() {
                next_command = Some(command);
                continue;
            }
        }
        if let Some(exporter) = &exporter {
            for (index, documents, local) in pending.drain(..) {
                let mut documents = documents.into_iter();
                loop {
                    let batch: Vec<_> = documents.by_ref().take(512).collect();
                    if batch.is_empty() {
                        break;
                    }
                    export_batch(&shared, exporter.as_ref(), index, batch, local, &mut counted_errors);
                }
            }
            pending_count = 0;
            pending_bytes = 0;
        }
        if let Some(reply) = flush {
            let _ = reply.send(());
        }
        if stopping {
            break;
        }
    }
    shared.counters.dropped.fetch_add(
        (pending_count + traces.values().map(|(_, spans)| spans.len()).sum::<usize>()) as u64,
        Ordering::Relaxed,
    );
}

/// Export one batch: its error groups, then its documents. A failing or panicking exporter costs this batch, which
/// is counted and logged once; the worker goes on with the next.
fn export_batch(
    shared: &Shared,
    exporter: &dyn Exporter,
    index: &'static str,
    batch: Vec<Document>,
    local: bool,
    counted_errors: &mut Recent,
) {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    if index == "otel_spans" || index == "otel_logs" {
        for document in &batch {
            if !has_exception(document) {
                continue;
            }
            let error = error_document(document);
            // An error is one occurrence per trace, though every frame it unwinds through and the request root
            // carry it, and they may reach the worker in different batches.
            let trace = string_field(document, "trace_id");
            if !trace.is_empty() {
                let key = format!("{}\n{trace}", string_field(&error, "fingerprint"));
                if counted_errors.get(&key).is_some() {
                    continue;
                }
                counted_errors.insert(&key, true, Instant::now());
            }
            let outcome = catch_unwind(AssertUnwindSafe(|| exporter.upsert_error(error)))
                .unwrap_or_else(|_| Err(Error::new("the telemetry exporter panicked")));
            if let Err(error) = outcome {
                shared.counters.dropped.fetch_add(1, Ordering::Relaxed);
                report_export_error(shared, "otel_errors", 1, &error.to_string());
            }
        }
    }
    let count = batch.len() as u64;
    let _ = take_write_error();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        if local {
            exporter.export(index, batch)
        } else {
            exporter.export_received(index, batch)
        }
    }))
    .unwrap_or_else(|_| Err(Error::new("the telemetry exporter panicked")));
    match outcome {
        Ok(done) => {
            let done = (done as u64).min(count);
            shared.counters.exported.fetch_add(done, Ordering::Relaxed);
            let lost = count - done;
            if lost > 0 {
                shared.counters.dropped.fetch_add(lost, Ordering::Relaxed);
                let reason = take_write_error()
                    .unwrap_or_else(|| "the store acknowledged fewer documents than it was sent".into());
                report_export_error(shared, index, lost, &reason);
            }
        }
        Err(error) => {
            shared.counters.dropped.fetch_add(count, Ordering::Relaxed);
            report_export_error(shared, index, count, &error.to_string());
        }
    }
}

/// Count `documents` an exporter did not persist, and log the failure: the first and then powers of two, which give
/// useful outage diagnostics without flooding the log.
fn report_export_error(shared: &Shared, index: &str, documents: u64, error: &str) {
    shared.counters.failed.fetch_add(documents, Ordering::Relaxed);
    let failures = shared.counters.export_errors.fetch_add(1, Ordering::Relaxed);
    if failures == 0 || failures.is_power_of_two() {
        tracing::warn!(
            index,
            documents,
            failures = failures + 1,
            "telemetry export failed: {error}"
        );
    }
}

/// Group by exception kind, a normalized message, and frame names (line changes do not split groups).
pub fn error_fingerprint(kind: &str, message: &str, stack: &str) -> String {
    use sha2::{Digest, Sha256};
    let numbers = regex_for_fingerprint();
    let normalized = numbers.replace_all(message, "?");
    let frames: String = stack
        .lines()
        .take(12)
        .map(|line| numbers.replace_all(line, "?").into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{:x}",
        Sha256::digest(format!("{kind}\n{normalized}\n{frames}").as_bytes())
    )
}

/// `text` with each variable part (quoted text, hex, a UUID, a number) replaced by `replacement`: the masking that
/// error fingerprints and log patterns share.
pub fn mask_message(text: &str, replacement: &str) -> String {
    regex_for_fingerprint().replace_all(text, replacement).into_owned()
}

fn regex_for_fingerprint() -> &'static regex::Regex {
    static REGEX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    REGEX.get_or_init(|| {
        regex::Regex::new(r#"(?i)"[^"]*"|'[^']*'|\b(?:0x[0-9a-f]+|[0-9a-f]{8}-[0-9a-f-]{27,}|\d+(?:\.\d+)?)\b"#)
            .expect("constant regex")
    })
}

fn error_document(span: &Document) -> Document {
    let kind = text(span, "exception.type");
    let message = if span.contains_key("exception.message") {
        text(span, "exception.message")
    } else {
        text(span, "body")
    };
    let stack = text(span, "exception.stacktrace");
    let mut document = Document::new();
    use sha2::{Digest, Sha256};
    let fingerprint = error_fingerprint(&kind, &message, &stack);
    document.insert(
        "fingerprint".into(),
        Value::String(format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}\n{}\n{fingerprint}",
                    if span.contains_key("source.id") {
                        text(span, "source.id")
                    } else {
                        text(span, "source.name")
                    },
                    text(span, "service.name")
                )
                .as_bytes()
            )
        )),
    );
    let timestamp = span
        .get("start")
        .or_else(|| span.get("timestamp"))
        .cloned()
        .unwrap_or_else(|| Value::I64(crate::user::now_millis()));
    document.insert("first_seen".into(), timestamp.clone());
    document.insert("last_seen".into(), timestamp);
    document.insert("count".into(), Value::I64(1));
    document.insert("exception.type".into(), Value::String(kind));
    document.insert("message".into(), Value::String(message));
    document.insert("stacktrace".into(), Value::String(stack));
    document.insert(
        "sample_trace_ids".into(),
        Value::List(vec![Value::String(text(span, "trace_id"))]),
    );
    for key in ["service.name", "service.instance.id", "source.name", "source.id"] {
        if let Some(value) = span.get(key) {
            document.insert(key.into(), value.clone());
        }
    }
    document
}

#[cfg(test)]
mod tests;
