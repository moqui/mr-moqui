//! Application telemetry. SDK types deliberately stay inside this module.
//!
//! Recording never writes through the entity/search facade on a business thread. Exporters run
//! on a bounded worker, outside business transactions, and independent failed records are skipped.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::document::Document;

mod exponential;
pub mod ingest;
pub mod maintenance;
#[cfg(feature = "http")]
pub mod otlp;

pub fn register(registry: &mut crate::native::NativeRegistry) -> Result<(), crate::context::Error> {
    registry.service_function("moqui_core::telemetry::clean", clean)?;
    registry.service_function("moqui_core::telemetry::rollup", rollup)?;
    Ok(())
}

pub fn clean(
    ec: &ExecutionContext,
    _service: &crate::native::ServiceInfo<'_>,
    _parameters: &crate::native::Parameters,
) -> Result<crate::native::Parameters, Error> {
    maintenance::retain(ec.factory()).map(|document| document.into_iter().collect())
}

pub fn rollup(
    ec: &ExecutionContext,
    _service: &crate::native::ServiceInfo<'_>,
    _parameters: &crate::native::Parameters,
) -> Result<crate::native::Parameters, Error> {
    maintenance::rollup_metrics(ec.factory()).map(|document| document.into_iter().collect())
}
#[cfg(feature = "telemetry")]
mod sdk;

const MAX_QUEUE: usize = 8192;
const MAX_BUFFER_BYTES: usize = 32 * 1024 * 1024;
const ROOT_RESERVED_RECORDS: usize = 512;
const ROOT_RESERVED_BYTES: usize = 2 * 1024 * 1024;
const MAX_TRACES: usize = 1024;
const MAX_TRACE_SPANS: usize = 512;
const MAX_DECISIONS: usize = 4096;
const MAX_SERIES: usize = 20_000;

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A W3C context that can cross worker and async boundaries without an SDK or a thread guard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub sampled: bool,
    pub tracestate: String,
}

impl TraceContext {
    pub fn extract(traceparent: &str, tracestate: &str) -> Option<Self> {
        let parts: Vec<_> = traceparent.trim().split('-').collect();
        if parts.len() != 4
            || parts[0] != "00"
            || !valid_hex(parts[1], 32)
            || !valid_hex(parts[2], 16)
            || !valid_hex(parts[3], 2)
            || parts[1].bytes().all(|b| b == b'0')
            || parts[2].bytes().all(|b| b == b'0')
            || tracestate.len() > 512
            || tracestate.contains(['\r', '\n'])
        {
            return None;
        }
        Some(Self {
            trace_id: parts[1].to_string(),
            span_id: parts[2].to_string(),
            sampled: u8::from_str_radix(parts[3], 16).ok()? & 1 != 0,
            tracestate: tracestate.to_string(),
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

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    pub sample_ratio: f64,
    pub slow_root: Duration,
    pub interval: Duration,
    pub artifact_types: HashMap<String, (bool, bool)>,
    legacy_warning: bool,
    legacy_slow_types: HashSet<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: cfg!(feature = "telemetry"),
            sample_ratio: 1.0,
            slow_root: Duration::from_secs(1),
            interval: Duration::from_secs(60),
            artifact_types: HashMap::from([
                ("AT_ENTITY".into(), (false, false)),
                ("AT_XML_SCREEN_CONTENT".into(), (false, true)),
            ]),
            legacy_warning: false,
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
            for item in stats.named_iter("artifact-stats") {
                if let Some(kind) = item.attribute("type") {
                    let values = (
                        setting(&item, "persist-hit").as_deref() == Some("true"),
                        setting(&item, "persist-bin").as_deref() == Some("true"),
                    );
                    config.legacy_warning = true;
                    if values == (false, true) {
                        config.legacy_slow_types.insert(kind.clone());
                    }
                    config.artifact_types.insert(kind, values);
                }
            }
        }
        if let Some(node) = root.first_named("telemetry") {
            config.enabled &= setting(&node, "enabled").as_deref() != Some("false");
            config.sample_ratio = setting(&node, "sample-ratio")
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite())
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            config.interval = Duration::from_secs(
                setting(&node, "interval-seconds")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(60)
                    .max(1),
            );
            for item in node.named_iter("artifact") {
                if let Some(kind) = item.attribute("type") {
                    config.legacy_slow_types.remove(&kind);
                    config.artifact_types.insert(
                        kind,
                        (
                            setting(&item, "spans").as_deref() != Some("false"),
                            setting(&item, "metrics").as_deref() != Some("false"),
                        ),
                    );
                }
            }
        }
        config
    }
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
    if telemetry.shared.config.legacy_warning && !telemetry.shared.warned_legacy.swap(true, Ordering::Relaxed) {
        tracing::warn!("artifact-stats is deprecated; configure artifact spans and metrics under telemetry instead");
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

#[derive(Default)]
struct Counters {
    queued: AtomicU64,
    exported: AtomicU64,
    dropped: AtomicU64,
    failed: AtomicU64,
    remote_exported: AtomicU64,
    remote_dropped: AtomicU64,
    remote_failed: AtomicU64,
}

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
    slow: Mutex<HashMap<String, RunningStats>>,
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
        let (sender, receiver) = mpsc::sync_channel(MAX_QUEUE);
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
        });
        let worker = if shared.config.enabled {
            let state = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("moqui-telemetry".to_string())
                .spawn(move || worker(state, receiver))
                .ok()
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
                return sdk.start_span(name, kind, parent.or_else(capture), None, false);
            }
        }
        let _ = (name, kind, parent);
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
                return sdk.start_span(name, SpanKind::Internal, parent, Some(started), true);
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
        if !self.shared.config.enabled || self.shared.stopped.load(Ordering::Acquire) {
            return Err(Error::new("telemetry is disabled"));
        }
        self.shared.try_enqueue(index, documents, false)
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
        if self.artifact_enabled(kind) == (false, false) {
            return false;
        }
        let key = format!("{kind}:{name}:{action}");
        let slow = {
            let mut stats = lock(&self.shared.slow);
            if stats.len() >= MAX_SERIES && !stats.contains_key(&key) {
                false
            } else {
                stats.entry(key).or_default().record(seconds)
            }
        };
        if self.artifact_enabled(kind).1 {
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
        if !reserve(
            &self.queued_records,
            count,
            if root {
                MAX_QUEUE
            } else {
                MAX_QUEUE - ROOT_RESERVED_RECORDS
            },
        ) {
            self.counters.dropped.fetch_add(count as u64, Ordering::Relaxed);
            return Err(Error::new("telemetry queue record limit reached"));
        }
        if !reserve(
            &self.queued_bytes,
            bytes,
            if root {
                MAX_BUFFER_BYTES
            } else {
                MAX_BUFFER_BYTES - ROOT_RESERVED_BYTES
            },
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

/// Literal values are never exported, including SQL outside the parameterized entity paths.
pub fn redact_sql(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len().min(4096));
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if output.len() >= 4096 {
            break;
        }
        if ch == '\'' {
            output.push('?');
            while let Some(next) = chars.next() {
                if next == '\\' {
                    chars.next();
                    continue;
                }
                if next == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
        } else if ch == '-' && chars.peek() == Some(&'-') {
            for next in chars.by_ref() {
                if next == '\n' {
                    break;
                }
            }
            output.push(' ');
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = ' ';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            output.push(' ');
        } else if ch == '$' {
            let mut ahead = chars.clone();
            let mut delimiter = String::from("$");
            let mut found = false;
            for next in ahead.by_ref().take(64) {
                if next == '$' {
                    delimiter.push('$');
                    found = true;
                    break;
                }
                if !next.is_ascii_alphanumeric() && next != '_' {
                    break;
                }
                delimiter.push(next);
            }
            if found {
                for _ in 1..delimiter.len() {
                    chars.next();
                }
                let mut suffix = String::new();
                for next in chars.by_ref() {
                    suffix.push(next);
                    if suffix.ends_with(&delimiter) {
                        break;
                    }
                    if suffix.len() > delimiter.len() * 2 {
                        let keep = suffix
                            .char_indices()
                            .rev()
                            .nth(delimiter.len())
                            .map_or(0, |(index, _)| index);
                        suffix.drain(..keep);
                    }
                }
                output.push('?');
            } else {
                output.push('$');
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    if let Some(next) = chars.next() {
                        output.push(next);
                    }
                }
            }
        } else if ch.is_ascii_digit()
            && !output
                .chars()
                .last()
                .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_')
        {
            output.push('?');
            while chars.peek().is_some_and(|next| next.is_ascii_digit() || *next == '.') {
                chars.next();
            }
        } else {
            output.push(ch);
        }
    }
    output
}

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

pub(super) fn sensitive_key(key: &str) -> bool {
    let lowered;
    let key = if key.bytes().any(|byte| byte.is_ascii_uppercase()) {
        lowered = key.to_ascii_lowercase();
        lowered.as_str()
    } else {
        key
    };
    if key.ends_with(".tokens") || key.ends_with("_tokens") || key.ends_with("token_count") {
        return false;
    }
    [
        "password",
        "secret",
        "token",
        "authorization",
        "cookie",
        "credential",
        "arguments",
        "locals",
        "api.key",
        "api_key",
        "apikey",
        "private_key",
    ]
    .iter()
    .any(|part| key.contains(part))
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
    string_field(document, "kind") == "SERVER" || true_field(document, "moqui.job.root")
}

/// Tail promotion may make a large batch. Charge mandatory local roots first,
/// retaining the original document order while dropping only excess children.
fn admit_ready(ready: &mut Vec<Document>, local_roots: bool, count: &mut usize, bytes: &mut usize) -> usize {
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
        ready.retain(|document| !mandatory_root(document) || admit(document, MAX_QUEUE, MAX_BUFFER_BYTES));
    }
    ready.retain(|document| {
        (local_roots && mandatory_root(document))
            || admit(
                document,
                MAX_QUEUE - ROOT_RESERVED_RECORDS,
                MAX_BUFFER_BYTES - ROOT_RESERVED_BYTES,
            )
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

fn worker(shared: Arc<Shared>, receiver: mpsc::Receiver<Command>) {
    let _suppressed = suppress();
    let mut exporter: Option<Arc<dyn Exporter>> = None;
    let mut pending = Vec::<(&'static str, Vec<Document>, bool)>::new();
    let mut pending_count = 0;
    let mut pending_bytes = 0_usize;
    let mut tail_bytes = 0_usize;
    let mut tail_count = 0_usize;
    let mut traces = HashMap::<String, (Instant, Vec<Document>)>::new();
    let mut decisions = HashMap::<String, (bool, Instant)>::new();
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
                    decisions.retain(|_, (_, at)| now.duration_since(*at) < Duration::from_secs(120));
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
                            if decisions.len() >= MAX_DECISIONS {
                                decisions.clear();
                            }
                            decisions.insert(trace.to_string(), (keep, Instant::now()));
                            if mandatory_root || keep {
                                ready.push(document);
                            }
                        } else if sampled || decisions.get(trace).is_some_and(|(keep, _)| *keep) {
                            ready.push(document);
                        } else if !decisions.contains_key(trace) {
                            if traces.len() < MAX_TRACES || traces.contains_key(trace) {
                                let (_, buffered) = traces
                                    .entry(trace.to_string())
                                    .or_insert_with(|| (Instant::now(), Vec::new()));
                                let bytes = document_bytes(&document);
                                if buffered.len() < MAX_TRACE_SPANS
                                    && tail_count < MAX_QUEUE
                                    && tail_bytes.saturating_add(bytes) <= MAX_BUFFER_BYTES
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
                    if index == "otel_spans" || index == "otel_logs" {
                        for document in &batch {
                            if has_exception(document) {
                                if let Err(err) = exporter.upsert_error(error_document(document)) {
                                    report_export_error(&shared, "otel_errors", &err);
                                }
                            }
                        }
                    }
                    let count = batch.len() as u64;
                    match if local {
                        exporter.export(index, batch)
                    } else {
                        exporter.export_received(index, batch)
                    } {
                        Ok(done) => {
                            shared.counters.exported.fetch_add(done as u64, Ordering::Relaxed);
                            shared
                                .counters
                                .dropped
                                .fetch_add(count.saturating_sub(done as u64), Ordering::Relaxed);
                        }
                        Err(err) => {
                            shared.counters.dropped.fetch_add(count, Ordering::Relaxed);
                            report_export_error(&shared, index, &err);
                        }
                    }
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

fn report_export_error(shared: &Shared, index: &str, error: &Error) {
    let failures = shared.counters.failed.fetch_add(1, Ordering::Relaxed);
    // The first and powers of two give useful outage diagnostics without flooding the log.
    if failures == 0 || failures.is_power_of_two() {
        tracing::warn!(index, failures = failures + 1, "telemetry export failed: {error}");
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
