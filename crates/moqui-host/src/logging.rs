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

//! Server log output, log files, and shipping logs to the search backend's log indexes.
//!
//! [`install`] sets the process's `tracing` subscriber: one line per event on stderr, at
//! `moqui_log_level` (default `info`) and above. The stderr line has a structured header and, when
//! color is on, a dim stamp, a colored level, a dim thread, and a blue target. Color follows
//! `moqui_log_color` (default `auto`: a terminal, and `NO_COLOR` unset). Spans are not tracked;
//! artifact hits are counted when a frame pops, not here.
//!
//! [`attach_files`] opens `moqui.log` and `moqui-error.log` once the runtime directory and the
//! conf's `logging` element are known, writes the lines logged before that, and rolls the files over by
//! size and by day. Those files stay the plain line, with no color and no console header. Without
//! it lines go to stderr only.
//!
//! When the search backend keeps log indexes (OpenSearch or Postgres), [`ship_to_search`] starts a
//! thread that bulk-indexes queued events to `moqui_logs` and HTTP requests to `moqui_http_log`
//! through the SearchFacade, as Java's `ElasticSearchLogger` and `ElasticRequestLogFilter` do.
//! Events are queued from [`install`] on, so the boot lines ship once search attaches. DEBUG and
//! TRACE are not shipped, a full queue drops new entries, and events logged while shipping are not
//! queued again. [`stop_shipping`] sends what is left and ends the thread.

use std::cell::Cell;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::document::Document;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Entries waiting for the shipper, per index. Java's `QUEUE_LIMIT`.
const QUEUE_LIMIT: usize = 16384;
/// Entries per bulk request.
const BATCH: usize = 1000;
const FLUSH_EVERY: Duration = Duration::from_secs(2);
/// The shipper checks for a stop this often while it waits to flush.
const POLL: Duration = Duration::from_millis(100);

static APP_QUEUE: Mutex<VecDeque<Document>> = Mutex::new(VecDeque::new());
static HTTP_QUEUE: Mutex<VecDeque<Document>> = Mutex::new(VecDeque::new());
/// Set by a test that wants the HTTP entries [`queue_http_request`] would ship.
#[cfg(test)]
static HTTP_CAPTURE: Mutex<Option<Vec<Document>>> = Mutex::new(None);
static SHIPPING: AtomicBool = AtomicBool::new(false);
static STOP_SHIPPING: AtomicBool = AtomicBool::new(false);
static SHIPPER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
/// Lines logged before [`attach_files`] and the sinks after it.
static FILES: Mutex<FileState> = Mutex::new(FileState::Pending(Vec::new()));
/// Lines held for the files before they attach. A failed boot never attaches, so this is capped.
const PENDING_LINE_LIMIT: usize = 20000;

thread_local! {
    /// Set on the shipper thread, so the client's own events are not shipped.
    static IN_SHIPPER: Cell<bool> = const { Cell::new(false) };
}

/// Install the stderr subscriber once per process. Later calls, and a process that already has a
/// global subscriber, change nothing.
pub fn install() {
    install_with_console(None);
}

/// [`install`], with stderr limited to `console` and above while the log files still get every line at
/// `moqui_log_level`: `moqui test` keeps its report readable this way.
pub fn install_with_console(console: Option<Level>) {
    install_subscriber(Some(console));
}

/// [`install`] with nothing on stderr: `moqui tui` draws on the terminal, and any stderr line would tear the screen. The
/// log files and the [`tap`]s get every line. It has to be the first install of the process.
pub fn install_without_console() {
    install_subscriber(None);
}

/// `console` is None for no stderr, `Some(None)` for every line at `moqui_log_level`, `Some(Some(level))` for that level
/// and above.
fn install_subscriber(console: Option<Option<Level>>) {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let level = match moqui_lib::env::get_prop_or_env("moqui_log_level")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "trace" => Level::TRACE,
            "debug" => Level::DEBUG,
            "warn" | "warning" => Level::WARN,
            "error" => Level::ERROR,
            _ => Level::INFO,
        };
        let color = color_enabled(
            &moqui_lib::env::get_prop_or_env("moqui_log_color"),
            std::env::var_os("NO_COLOR").is_some(),
            std::io::stderr().is_terminal(),
        );
        let _ = tracing::subscriber::set_global_default(LineSubscriber {
            level,
            console: console.map(|console| console.unwrap_or(level)),
            next_span: AtomicU64::new(1),
            color,
        });
    });
}

/// The receivers of [`tap`], each with the least level it takes. A full one drops lines rather than block the thread
/// that logs; a dropped one is removed.
static TAPS: Mutex<Vec<(std::sync::mpsc::SyncSender<String>, Level)>> = Mutex::new(Vec::new());

/// Every log line from now on, as the files get it, on a channel that holds up to `capacity` lines: the TUI's log pane.
/// The subscriber has no layers, so this is a third sink beside stderr and the files.
pub fn tap(capacity: usize) -> std::sync::mpsc::Receiver<String> {
    tap_at(capacity, Level::TRACE)
}

/// [`tap`] with only the lines at `least` and above: WARN and ERROR for the log pane of an SSH session.
pub fn tap_at(capacity: usize, least: Level) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::sync_channel(capacity.max(1));
    TAPS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((tx, least));
    rx
}

fn feed_taps(level: Level, line: &str) {
    let mut taps = TAPS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if taps.is_empty() {
        return;
    }
    // A Level orders by verbosity: ERROR is the least, so a line goes to a tap whose least level it is at or under.
    taps.retain(|(tap, least)| {
        level > *least
            || !matches!(
                tap.try_send(line.to_string()),
                Err(std::sync::mpsc::TrySendError::Disconnected(_))
            )
    });
}

/// Start shipping log events and HTTP requests to the backend's log indexes, when it keeps them.
/// Returns false and starts nothing for a backend that does not.
pub fn ship_to_search(factory: &Arc<ExecutionContextFactory>) -> bool {
    let Some(facade) = moqui_core::search::installed(factory) else {
        return false;
    };
    if !facade.backend().keeps_log_indexes() || SHIPPING.swap(true, Ordering::SeqCst) {
        return false;
    }
    STOP_SHIPPING.store(false, Ordering::SeqCst);
    if let Err(err) = facade.ensure_log_indexes() {
        tracing::warn!("Could not create the log indexes, the first flush tries again: {err}");
    }
    let backend_name = facade.backend().name();
    let shipper_facade = Arc::clone(&facade);
    let spawned = std::thread::Builder::new()
        .name("moqui-log-ship".into())
        .spawn(move || {
            IN_SHIPPER.with(|flag| flag.set(true));
            loop {
                let mut waited = Duration::ZERO;
                while waited < FLUSH_EVERY && !STOP_SHIPPING.load(Ordering::SeqCst) {
                    std::thread::sleep(POLL);
                    waited += POLL;
                }
                flush(&shipper_facade);
                if STOP_SHIPPING.load(Ordering::SeqCst) {
                    break;
                }
            }
        });
    match spawned {
        Ok(handle) => {
            *lock(&SHIPPER) = Some(handle);
            tracing::info!(
                "Shipping log events to the {backend_name} search backend indexes {} and {}",
                moqui_core::search::logs::APP_LOG,
                moqui_core::search::logs::HTTP_LOG
            );
            true
        }
        Err(_) => {
            SHIPPING.store(false, Ordering::SeqCst);
            false
        }
    }
}

/// Send what is queued and end the shipper thread, so it no longer holds the search facade and its
/// database connections. Does nothing when shipping never started.
pub fn stop_shipping() {
    let handle = lock(&SHIPPER).take();
    if let Some(handle) = handle {
        STOP_SHIPPING.store(true, Ordering::SeqCst);
        let _ = handle.join();
        SHIPPING.store(false, Ordering::SeqCst);
    }
}

/// Index what is queued now. The shipper thread calls this; tests call it to avoid waiting.
pub fn flush(facade: &moqui_core::search::SearchFacade) {
    for (queue, index) in [
        (&APP_QUEUE, moqui_core::search::logs::APP_LOG),
        (&HTTP_QUEUE, moqui_core::search::logs::HTTP_LOG),
    ] {
        loop {
            let batch: Vec<Document> = {
                let mut queue = lock(queue);
                let count = queue.len().min(BATCH);
                queue.drain(..count).collect()
            };
            if batch.is_empty() {
                break;
            }
            let full = batch.len() == BATCH;
            if let Err(err) = facade.index_log_entries(index, batch) {
                // stderr, not tracing: an event here would be queued for the store that just failed
                let _ = writeln!(std::io::stderr(), "could not ship log entries to {index}: {err}");
                break;
            }
            if !full {
                break;
            }
        }
    }
}

/// Queue one HTTP request for `moqui_http_log`. Nothing is queued unless shipping has started.
pub fn queue_http_request(entry: Document) {
    #[cfg(test)]
    if let Some(captured) = lock(&HTTP_CAPTURE).as_mut() {
        captured.push(entry.clone());
    }
    if SHIPPING.load(Ordering::Relaxed) {
        push(&HTTP_QUEUE, entry);
    }
}

/// Keep HTTP log entries until [`take_http_capture`]. A test holds the result across the requests.
/// The lock is a Tokio mutex, so that hold may cross `.await`.
#[cfg(test)]
pub(crate) async fn begin_http_capture() -> tokio::sync::MutexGuard<'static, ()> {
    let guard = HTTP_TEST.lock().await;
    *lock(&HTTP_CAPTURE) = Some(Vec::new());
    guard
}

/// The entries queued since [`begin_http_capture`], and capture turns off.
#[cfg(test)]
pub(crate) fn take_http_capture() -> Vec<Document> {
    lock(&HTTP_CAPTURE).take().unwrap_or_default()
}

#[cfg(test)]
static HTTP_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The first queued application-log entry whose message contains `part`.
#[cfg(test)]
pub(crate) fn take_app_log(part: &str) -> Option<Document> {
    let mut queue = lock(&APP_QUEUE);
    let index = queue.iter().position(|entry| {
        entry
            .get("message")
            .and_then(moqui_lib::object::Value::as_str)
            .is_some_and(|text| text.contains(part))
    })?;
    queue.remove(index)
}

fn push(queue: &Mutex<VecDeque<Document>>, entry: Document) {
    let mut queue = lock(queue);
    if queue.len() < QUEUE_LIMIT {
        queue.push_back(entry);
    }
}

enum FileState {
    /// Before [`attach_files`]: lines wait here.
    Pending(Vec<(Level, String)>),
    Attached(Box<FileSinks>),
    /// No directory, or it could not be opened.
    Off,
}

struct FileSinks {
    all: Rolling,
    errors: Rolling,
}

/// One log file that rolls over by size and by day.
struct Rolling {
    dir: PathBuf,
    base: &'static str,
    file: Option<std::fs::File>,
    size: u64,
    day: String,
    max_bytes: u64,
    max_files: usize,
    /// After a failed roll or open, neither is tried again before this.
    retry_at: Option<std::time::Instant>,
    retry_every: Duration,
    /// The current failure was said on stderr. Cleared once the file opens or rolls again.
    reported: bool,
}

/// How long a log file that could not be rolled or opened waits before the next try.
const FILE_RETRY: Duration = Duration::from_secs(10);

impl Rolling {
    fn open(dir: &Path, base: &'static str, max_bytes: u64, max_files: usize) -> std::io::Result<Self> {
        let mut rolling = Self {
            dir: dir.to_path_buf(),
            base,
            file: None,
            size: 0,
            day: String::new(),
            max_bytes,
            max_files,
            retry_at: None,
            retry_every: FILE_RETRY,
            reported: false,
        };
        let path = rolling.path();
        let modified = std::fs::metadata(&path).ok().and_then(|meta| meta.modified().ok());
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        rolling.size = file.metadata().map_or(0, |meta| meta.len());
        // An existing file belongs to the day it was last written, so a restart on a later day rolls it first.
        rolling.day = modified
            .and_then(|time| jiff::Timestamp::try_from(time).ok())
            .map_or_else(today, |at| at.strftime("%Y-%m-%d").to_string());
        rolling.file = Some(file);
        Ok(rolling)
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.log", self.base))
    }

    fn write(&mut self, line: &str) {
        let day = today();
        let incoming = line.len() as u64 + 1;
        if self.file.is_none() {
            if self.retry_due() {
                match self.open_current() {
                    Ok(()) => {
                        self.retry_at = None;
                        self.reported = false;
                    }
                    Err(err) => {
                        let path = self.path();
                        self.failed(&format!(
                            "could not open {}: {err}; its lines are dropped until it opens",
                            path.display()
                        ));
                    }
                }
            }
        } else if self.size > 0 && (day != self.day || self.size + incoming > self.max_bytes) && self.retry_due() {
            self.roll();
        }
        if self.size == 0 {
            self.day = day;
        }
        if let Some(file) = self.file.as_mut() {
            if writeln!(file, "{line}").is_ok() {
                self.size += incoming;
            }
        }
    }

    /// Move the current file to `base.yyyy-MM-dd.N.log`, start a new one, and drop the oldest rolled files. When
    /// the move fails the current file is kept and written on, and the roll is tried again later.
    fn roll(&mut self) {
        // Closed first: an open file cannot be renamed on Windows.
        self.file = None;
        let mut index = 1;
        let rolled = loop {
            let candidate = self.dir.join(format!("{}.{}.{index}.log", self.base, self.day));
            if !candidate.exists() {
                break candidate;
            }
            index += 1;
        };
        let path = self.path();
        if let Err(err) = std::fs::rename(&path, &rolled) {
            self.failed(&format!(
                "could not roll {} to {}: {err}; still writing to it",
                path.display(),
                rolled.display()
            ));
            let _ = self.open_current();
            return;
        }
        match self.open_current() {
            Ok(()) => {
                self.retry_at = None;
                self.reported = false;
            }
            Err(err) => self.failed(&format!(
                "could not open {}: {err}; its lines are dropped until it opens",
                path.display()
            )),
        }
        self.prune();
    }

    /// Open the current file for appending; the size is what it already holds.
    fn open_current(&mut self) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())?;
        self.size = file.metadata().map_or(0, |meta| meta.len());
        self.file = Some(file);
        Ok(())
    }

    fn retry_due(&self) -> bool {
        self.retry_at.is_none_or(|at| std::time::Instant::now() >= at)
    }

    /// Wait `retry_every` before the next roll or open. The first failure in a row goes to stderr, not to tracing,
    /// which would write to this file.
    fn failed(&mut self, message: &str) {
        self.retry_at = Some(std::time::Instant::now() + self.retry_every);
        if !self.reported {
            self.reported = true;
            let _ = writeln!(std::io::stderr(), "{message}");
        }
    }

    fn prune(&self) {
        let prefix = format!("{}.", self.base);
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut rolled: Vec<(std::time::SystemTime, PathBuf)> = read
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with(&prefix) && name.ends_with(".log") && *name != format!("{}.log", self.base)
            })
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        rolled.sort();
        let excess = rolled.len().saturating_sub(self.max_files);
        for (_, path) in rolled.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn today() -> String {
    jiff::Timestamp::now().strftime("%Y-%m-%d").to_string()
}

impl FileSinks {
    fn write(&mut self, level: Level, line: &str) {
        self.all.write(line);
        if level <= Level::WARN {
            self.errors.write(line);
        }
    }
}

fn write_file_line(level: Level, line: &str) {
    let mut state = lock(&FILES);
    match &mut *state {
        FileState::Pending(lines) => {
            if lines.len() < PENDING_LINE_LIMIT {
                lines.push((level, line.to_string()));
            }
        }
        FileState::Attached(sinks) => sinks.write(level, line),
        FileState::Off => {}
    }
}

/// Open the log files of the conf's `logging` element: `dir` (`moqui_log_dir`, a path or a `runtime://` location;
/// empty means no files), `max-size-mb` and `max-files`; with no factory yet, `moqui_log_dir` and the defaults. Write
/// the lines logged so far. The first call decides; later calls change nothing. Returns the directory when files are
/// being written.
pub fn attach_files(
    runtime_dir: &Path,
    factory: Option<&moqui_core::context::ExecutionContextFactory>,
) -> Option<PathBuf> {
    let mut state = lock(&FILES);
    let FileState::Pending(pending) = &mut *state else {
        return None;
    };
    let pending = std::mem::take(pending);
    let setting = |name: &str| factory.map(|factory| factory.conf_setting("logging", name)).unwrap_or_default();
    let mut configured_dir = setting("dir");
    if configured_dir.is_empty() {
        configured_dir = moqui_lib::env::get_prop_or_env("moqui_log_dir");
    }
    let dir = file_dir(runtime_dir, &configured_dir);
    let max_mb = setting("max-size-mb").parse::<u64>().ok().filter(|mb| *mb > 0).unwrap_or(100);
    let max_files = setting("max-files").parse::<usize>().unwrap_or(30);
    let opened = dir.as_ref().and_then(|dir| {
        let sinks = std::fs::create_dir_all(dir).and_then(|()| {
            Ok(FileSinks {
                all: Rolling::open(dir, "moqui", max_mb * 1024 * 1024, max_files)?,
                errors: Rolling::open(dir, "moqui-error", max_mb * 1024 * 1024, max_files)?,
            })
        });
        match sinks {
            Ok(sinks) => Some((dir.clone(), sinks)),
            Err(err) => {
                // stderr, not tracing: this runs under the state lock
                let _ = writeln!(
                    std::io::stderr(),
                    "could not open log files in {}: {err}",
                    dir.display()
                );
                None
            }
        }
    });
    match opened {
        Some((dir, mut sinks)) => {
            for (level, line) in &pending {
                sinks.write(*level, line);
            }
            *state = FileState::Attached(Box::new(sinks));
            Some(dir)
        }
        None => {
            *state = FileState::Off;
            None
        }
    }
}

fn file_dir(runtime_dir: &Path, configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if configured.is_empty() {
        return None;
    }
    let path = match configured.strip_prefix("runtime://") {
        Some(rest) => runtime_dir.join(rest.trim_start_matches('/')),
        None => PathBuf::from(configured),
    };
    Some(if path.is_absolute() {
        path
    } else {
        runtime_dir.join(path)
    })
}

/// The time now in epoch millis.
pub fn now_millis() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// This host's name, as Java's `InetAddress.getLocalHost().hostName`, read once.
pub fn host_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::fs::read_to_string("/proc/sys/kernel/hostname")
            .ok()
            .or_else(|| std::env::var("HOSTNAME").ok())
            .or_else(|| std::env::var("COMPUTERNAME").ok())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "localhost".to_string())
    })
}

struct LineSubscriber {
    level: Level,
    /// The lowest level written to stderr, or None for no stderr; the files take every line at `level`.
    console: Option<Level>,
    next_span: AtomicU64,
    /// Stderr uses the colored console header. Decided once in [`install`].
    color: bool,
}

impl Subscriber for LineSubscriber {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= self.level
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed).max(1))
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed").to_string();
        let millis = now_millis();
        let stamp = jiff::Timestamp::from_millisecond(millis)
            .map(|at| at.strftime("%Y-%m-%d %H:%M:%S%.3f").to_string())
            .unwrap_or_default();
        let level = *metadata.level();
        let text = LogText {
            stamp: &stamp,
            level,
            thread: &thread_name,
            target: metadata.target(),
            message: &fields.message,
            extra: &fields.extra,
        };
        if self.console.is_some_and(|console| level <= console) {
            let _ = writeln!(std::io::stderr(), "{}", console_line(&text, self.color));
        }
        let plain = plain_line(&text);
        feed_taps(level, &plain);
        write_file_line(level, &plain);

        // Queued from the start, so the boot lines ship once search attaches.
        if level == Level::DEBUG || level == Level::TRACE || IN_SHIPPER.with(Cell::get) {
            return;
        }
        let mut entry = Document::new();
        entry.insert("@timestamp".into(), Value::I64(millis));
        entry.insert("level".into(), Value::string(level.as_str()));
        entry.insert("thread_name".into(), Value::String(thread_name));
        entry.insert("thread_id".into(), Value::I64(thread_number(&thread)));
        entry.insert("logger_name".into(), Value::string(metadata.target()));
        entry.insert("message".into(), Value::String(fields.message));
        entry.insert("source_host".into(), Value::string(host_name()));
        if let Some((user_id, visitor_id)) = moqui_core::user::current_log_actor() {
            if let Some(user_id) = user_id {
                entry.insert("user_id".into(), Value::String(user_id));
            }
            if let Some(visitor_id) = visitor_id {
                entry.insert("visitor_id".into(), Value::String(visitor_id));
            }
        }
        if let Some((name, message)) = fields.error {
            let mut thrown = Document::new();
            thrown.insert("name".into(), Value::string(name));
            thrown.insert("message".into(), Value::string(message));
            entry.insert("thrown".into(), Value::Map(thrown));
        }
        if !fields.extra.is_empty() {
            let mdc = fields
                .extra
                .into_iter()
                .map(|(name, value)| (name, Value::String(value)))
                .collect();
            entry.insert("mdc".into(), Value::Map(mdc));
        }
        push(&APP_QUEUE, entry);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// The number in `ThreadId(n)`; `ThreadId::as_u64` is not stable.
fn thread_number(thread: &std::thread::Thread) -> i64 {
    let text = format!("{:?}", thread.id());
    text.trim_start_matches("ThreadId(")
        .trim_end_matches(')')
        .parse()
        .unwrap_or(0)
}

#[derive(Default)]
struct Fields {
    message: String,
    /// `name` and `message` of a field named `error`. It stays out of `mdc`.
    error: Option<(String, String)>,
    extra: Vec<(String, String)>,
}

impl Fields {
    fn record_error_field(&mut self, field: &Field, name: &str, message: String) -> bool {
        if field.name() != "error" {
            return false;
        }
        self.error = Some((name.to_string(), message));
        true
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else if !self.record_error_field(field, "error", value.to_string()) {
            self.extra.push((field.name().to_string(), value.to_string()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else if !self.record_error_field(field, "error", format!("{value:?}")) {
            self.extra.push((field.name().to_string(), format!("{value:?}")));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        if !self.record_error_field(field, "error", value.to_string()) {
            self.extra.push((field.name().to_string(), value.to_string()));
        }
    }
}

/// One log event, before it is rendered for stderr or for a file.
struct LogText<'a> {
    stamp: &'a str,
    level: Level,
    thread: &'a str,
    target: &'a str,
    message: &'a str,
    extra: &'a [(String, String)],
}

/// The file line. Same text the files have always stored: no color, full target.
fn plain_line(text: &LogText<'_>) -> String {
    let mut line = String::with_capacity(96 + text.message.len());
    let _ = write!(
        line,
        "{} {:<5} [{}] {} - {}",
        text.stamp, text.level, text.thread, text.target, text.message
    );
    append_extra(&mut line, text.extra, false);
    line
}

/// The stderr line. The level sits in a fixed column. Color is SGR around the header pieces.
fn console_line(text: &LogText<'_>, color: bool) -> String {
    let mut line = String::with_capacity(96 + text.message.len());
    paint(&mut line, "2", text.stamp, color);
    // The stamp is 23 columns, then two spaces, then a 5-column level.
    let mut col = text.stamp.len() + 2;
    push_spaces(&mut line, 2);
    let word = text.level.as_str();
    paint(&mut line, level_sgr(text.level), word, color);
    let pad = 5usize.saturating_sub(word.len());
    push_spaces(&mut line, pad + 2);
    col += 5 + 2;
    let thread = format!("[{}]", text.thread);
    paint(&mut line, "2", &thread, color);
    col += thread.len() + 1;
    push_spaces(&mut line, 1);
    let target = short_target(text.target);
    paint(&mut line, "34", &target, color);
    col += target.len() + 2;
    push_spaces(&mut line, 2);
    write_console_message(&mut line, text.message, col, color);
    append_extra(&mut line, text.extra, color);
    line
}

/// `auto` (and anything else, including empty) is a terminal with `NO_COLOR` unset.
/// `always` wins over a pipe and over `NO_COLOR`. `never` wins over a terminal.
fn color_enabled(mode: &str, no_color: bool, is_tty: bool) -> bool {
    match mode.trim().to_ascii_lowercase().as_str() {
        "never" => false,
        "always" => true,
        _ => is_tty && !no_color,
    }
}

/// Console only. `moqui_core::context::factory` becomes `core::context::factory`.
/// `moqui` itself and any other target stay as they are.
fn short_target(target: &str) -> String {
    let (first, rest) = match target.split_once("::") {
        Some((first, rest)) => (first, Some(rest)),
        None => (target, None),
    };
    let short = match first {
        "moqui_core" => "core",
        "moqui_host" => "host",
        "moqui_lib" => "lib",
        "moqui_db" => "db",
        _ => return target.to_string(),
    };
    match rest {
        Some(rest) => format!("{short}::{rest}"),
        None => short.to_string(),
    }
}

fn level_sgr(level: Level) -> &'static str {
    match level {
        Level::TRACE => "35",
        Level::DEBUG => "1;35",
        Level::INFO => "1;32",
        Level::WARN => "1;33",
        Level::ERROR => "1;31",
    }
}

fn paint(out: &mut String, sgr: &str, text: &str, color: bool) {
    if color {
        out.push_str("\u{1b}[");
        out.push_str(sgr);
        out.push('m');
        out.push_str(text);
        out.push_str("\u{1b}[0m");
    } else {
        out.push_str(text);
    }
}

fn push_spaces(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push(' ');
    }
}

/// The first line stays on the header. Later lines indent to the message column, with a bar so a
/// stack trace stays attached to its header.
fn write_console_message(out: &mut String, message: &str, message_col: usize, color: bool) {
    let mut lines = message.split('\n');
    if let Some(first) = lines.next() {
        out.push_str(first);
    }
    let bar_at = message_col.saturating_sub(2);
    for line in lines {
        out.push('\n');
        push_spaces(out, bar_at);
        paint(out, "2", "|", color);
        out.push(' ');
        out.push_str(line);
    }
}

fn append_extra(out: &mut String, extra: &[(String, String)], color: bool) {
    for (name, value) in extra {
        out.push(' ');
        paint(out, "2", name, color);
        out.push('=');
        out.push_str(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_takes_the_lines_at_its_level_and_above() {
        let warnings = tap_at(8, Level::WARN);
        let all = tap(8);
        feed_taps(Level::INFO, "an info line");
        feed_taps(Level::WARN, "a warning");
        feed_taps(Level::ERROR, "an error");
        assert_eq!(warnings.try_iter().collect::<Vec<_>>(), ["a warning", "an error"]);
        assert_eq!(all.try_iter().count(), 3);
    }

    #[test]
    fn an_app_log_carries_the_user_the_visitor_and_the_thrown_message() {
        install();
        let bare = format!("log-bare-{}", std::process::id());
        tracing::error!("{bare}");
        let entry = take_app_log(&bare).expect("the subscriber did not queue the line");
        assert!(
            entry.get("user_id").is_none() && entry.get("visitor_id").is_none(),
            "{entry:?}"
        );

        let context = moqui_core::context::ExecutionContextFactory::test_context();
        let _entered = context.enter();
        moqui_core::user::set_current_log_actor("user-1", "visitor-1");
        let marked = format!("log-actor-{}", std::process::id());
        tracing::error!(error = "disk full", "{marked}");
        let entry = take_app_log(&marked).expect("the subscriber did not queue the line");
        assert_eq!(entry.get("user_id").and_then(Value::as_str), Some("user-1"));
        assert_eq!(entry.get("visitor_id").and_then(Value::as_str), Some("visitor-1"));
        let thrown = match entry.get("thrown") {
            Some(Value::Map(map)) => map,
            other => panic!("thrown missing: {other:?}"),
        };
        assert_eq!(thrown.get("message").and_then(Value::as_str), Some("disk full"));
        assert!(entry.get("mdc").is_none(), "{entry:?}");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moqui-logging-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rolled(dir: &Path, base: &str) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(&format!("{base}.")) && name != &format!("{base}.log"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_rolls_by_size_and_keeps_the_newest() {
        let dir = scratch("size");
        let mut log = Rolling::open(&dir, "moqui", 100, 2).unwrap();
        for index in 0..20 {
            log.write(&format!("line {index:02} {}", "x".repeat(30)));
            // distinct modified times, so the oldest is well defined
            std::thread::sleep(Duration::from_millis(15));
        }
        let names = rolled(&dir, "moqui");
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(
            names
                .iter()
                .all(|name| name.ends_with(".log") && name.contains(&today())),
            "{names:?}"
        );
        let current = std::fs::read_to_string(dir.join("moqui.log")).unwrap();
        assert!(current.contains("line 19"), "{current}");
        assert!(!std::fs::read_to_string(dir.join(&names[0]))
            .unwrap()
            .contains("line 00"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_an_earlier_day_rolls_before_the_first_write() {
        let dir = scratch("day");
        let mut log = Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap();
        log.write("first");
        log.day = "2000-01-01".to_string();
        log.write("second");
        let names = rolled(&dir, "moqui");
        assert_eq!(names, vec!["moqui.2000-01-01.1.log".to_string()]);
        assert_eq!(std::fs::read_to_string(dir.join("moqui.log")).unwrap(), "second\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_reopened_is_tried_again() {
        let dir = scratch("reopen");
        let mut log = Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap();
        log.retry_every = Duration::from_millis(50);
        log.write("first");
        // The roll's move and reopen both fail while the directory is gone.
        std::fs::remove_dir_all(&dir).unwrap();
        log.day = "2000-01-01".to_string();
        log.write("lost");
        assert!(log.file.is_none());
        std::fs::create_dir_all(&dir).unwrap();
        log.write("too soon");
        assert!(log.file.is_none());
        std::thread::sleep(Duration::from_millis(60));
        log.write("back");
        assert_eq!(std::fs::read_to_string(dir.join("moqui.log")).unwrap(), "back\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_roll_keeps_writing_the_current_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("keep");
        let mut log = Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap();
        log.retry_every = Duration::from_millis(50);
        log.write("first");
        // A directory that takes no new names: the move to the rolled name fails.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let locked = std::fs::write(dir.join("probe"), "").is_err();
        if locked {
            log.day = "2000-01-01".to_string();
            log.write("second");
            assert!(rolled(&dir, "moqui").is_empty());
            assert_eq!(
                std::fs::read_to_string(dir.join("moqui.log")).unwrap(),
                "first\nsecond\n"
            );
        }
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if locked {
            std::thread::sleep(Duration::from_millis(60));
            log.write("third");
            assert_eq!(rolled(&dir, "moqui"), vec!["moqui.2000-01-01.1.log".to_string()]);
            assert_eq!(
                std::fs::read_to_string(dir.join("moqui.2000-01-01.1.log")).unwrap(),
                "first\nsecond\n"
            );
            assert_eq!(std::fs::read_to_string(dir.join("moqui.log")).unwrap(), "third\n");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn warnings_and_errors_go_to_the_error_file_too() {
        let dir = scratch("errors");
        let mut sinks = FileSinks {
            all: Rolling::open(&dir, "moqui", 1_000_000, 5).unwrap(),
            errors: Rolling::open(&dir, "moqui-error", 1_000_000, 5).unwrap(),
        };
        sinks.write(Level::INFO, "info line");
        sinks.write(Level::WARN, "warn line");
        sinks.write(Level::ERROR, "error line");
        assert_eq!(
            std::fs::read_to_string(dir.join("moqui.log")).unwrap().lines().count(),
            3
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("moqui-error.log")).unwrap(),
            "warn line\nerror line\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_log_dir_is_a_runtime_location_a_path_or_empty() {
        let runtime = Path::new("/srv/moqui/runtime");
        assert_eq!(
            file_dir(runtime, "runtime://log"),
            Some(PathBuf::from("/srv/moqui/runtime/log"))
        );
        assert_eq!(
            file_dir(runtime, "/var/log/moqui"),
            Some(PathBuf::from("/var/log/moqui"))
        );
        assert_eq!(
            file_dir(runtime, "logs"),
            Some(PathBuf::from("/srv/moqui/runtime/logs"))
        );
        assert_eq!(file_dir(runtime, "  "), None);
    }

    const STAMP: &str = "2026-10-15 01:48:49.933";

    fn text<'a>(level: Level, target: &'a str, message: &'a str, extra: &'a [(String, String)]) -> LogText<'a> {
        LogText {
            stamp: STAMP,
            level,
            thread: "moqui-http",
            target,
            message,
            extra,
        }
    }

    #[test]
    fn the_file_line_stays_plain() {
        let message = "Setting property instance_purpose from default with value [dev]";
        let line = plain_line(&text(Level::INFO, "moqui_core::context::factory", message, &[]));
        assert_eq!(
            line,
            "2026-10-15 01:48:49.933 INFO  [moqui-http] moqui_core::context::factory - Setting property instance_purpose from default with value [dev]"
        );
        assert!(!line.contains('\u{1b}'));
        let extra = [("name".to_string(), "value".to_string())];
        let with_extra = plain_line(&text(Level::WARN, "moqui_host::db::postgres", "hello", &extra));
        assert_eq!(
            with_extra,
            "2026-10-15 01:48:49.933 WARN  [moqui-http] moqui_host::db::postgres - hello name=value"
        );
        assert!(!with_extra.contains('\u{1b}'));
    }

    #[test]
    fn the_console_line_is_structured_and_the_level_is_colored() {
        let message = "Setting property instance_purpose from default with value [dev]";
        let plain = console_line(&text(Level::INFO, "moqui_core::context::factory", message, &[]), false);
        assert_eq!(
            plain,
            "2026-10-15 01:48:49.933  INFO   [moqui-http] core::context::factory  Setting property instance_purpose from default with value [dev]"
        );
        assert!(!plain.contains('\u{1b}'));

        let cases = [
            (Level::TRACE, "35", "TRACE"),
            (Level::DEBUG, "1;35", "DEBUG"),
            (Level::INFO, "1;32", "INFO"),
            (Level::WARN, "1;33", "WARN"),
            (Level::ERROR, "1;31", "ERROR"),
        ];
        for (level, sgr, word) in cases {
            let line = console_line(&text(level, "moqui_core::context::factory", "hello", &[]), true);
            assert!(
                line.contains(&format!("\u{1b}[{sgr}m{word}\u{1b}[0m")),
                "{word} in {line:?}"
            );
            let at = line.find("hello").unwrap();
            let last = line[..at].rfind('\u{1b}').unwrap();
            assert!(
                line[last..at].starts_with("\u{1b}[0m"),
                "{word} message span in {line:?}"
            );
        }
        let extra = [("name".to_string(), "value".to_string())];
        let named = console_line(
            &text(Level::INFO, "moqui_core::context::factory", "hello", &extra),
            true,
        );
        assert!(named.contains(" \u{1b}[2mname\u{1b}[0m=value"), "{named:?}");
    }

    #[test]
    fn the_console_shortens_only_the_moqui_crate_prefix() {
        let message = "hello";
        for (full, short) in [
            ("moqui_core::context::factory", "core::context::factory"),
            ("moqui_host::db::postgres", "host::db::postgres"),
            ("moqui_lib::env", "lib::env"),
            ("moqui_db::sql", "db::sql"),
            ("moqui_core", "core"),
        ] {
            let shown = console_line(&text(Level::INFO, full, message, &[]), false);
            assert!(shown.contains(short), "{full} -> {shown}");
            assert!(!shown.contains(full), "{full} stayed in {shown}");
            let stored = plain_line(&text(Level::INFO, full, message, &[]));
            assert!(stored.contains(full), "{stored}");
        }
        for keep in ["moqui::main", "other::target"] {
            let shown = console_line(&text(Level::INFO, keep, message, &[]), false);
            assert!(shown.contains(keep), "{shown}");
        }
    }

    #[test]
    fn a_second_line_indents_on_the_console_and_stays_flush_in_the_file() {
        let message = "first\nsecond";
        let stored = plain_line(&text(Level::ERROR, "moqui_core::context::factory", message, &[]));
        assert!(stored.contains("first\nsecond"), "{stored:?}");
        assert!(!stored.contains('|'));

        let shown = console_line(&text(Level::ERROR, "moqui_core::context::factory", message, &[]), false);
        let (first, second) = shown.split_once('\n').unwrap();
        let col = first.find("first").unwrap();
        assert_eq!(&second[col..], "second");
        assert_eq!(&second[col - 2..col], "| ");
        assert!(second[..col - 2].chars().all(|ch| ch == ' '));
        assert!(!shown.contains('\u{1b}'));

        let colored = console_line(&text(Level::ERROR, "moqui_core::context::factory", message, &[]), true);
        assert!(colored.contains("\u{1b}[2m|\u{1b}[0m second"), "{colored:?}");
        let stripped = strip_ansi(&colored);
        let (first, second) = stripped.split_once('\n').unwrap();
        let col = first.find("first").unwrap();
        assert_eq!(&second[col..], "second");
    }

    #[test]
    fn color_follows_the_mode_the_terminal_and_no_color() {
        assert!(color_enabled("auto", false, true));
        assert!(!color_enabled("auto", false, false));
        assert!(!color_enabled("auto", true, true));
        assert!(color_enabled("", false, true));
        assert!(color_enabled("always", false, false));
        assert!(!color_enabled("never", false, true));
        assert!(color_enabled("always", true, false));
        assert!(!color_enabled(" NEVER ", false, true));
    }

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find('\u{1b}') {
            out.push_str(&rest[..start]);
            let after = &rest[start..];
            let end = after.find('m').map_or(after.len(), |at| at + 1);
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }
}
