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

//! Server log output, log files, and factory-scoped OpenTelemetry logs.
//!
//! [`install`] sets the process's `tracing` subscriber: one line per event on stderr, at
//! `moqui_log_level` (default `info`) and above. The stderr line has a structured header and, when
//! color is on, a dim stamp, a colored level, a dim thread, and a blue target. Color follows
//! `moqui_log_color` (default `auto`: a terminal, and `NO_COLOR` unset). Spans are not tracked;
//! telemetry spans follow artifact frames.
//!
//! [`attach_files`] opens `moqui.log` and `moqui-error.log` once the runtime directory and the
//! conf's `logging` element are known, writes the lines logged before that, and rolls the files over by
//! size and by day. Those files stay the plain line, with no color and no console header. Without
//! it lines go to stderr only.
//!
//! Events enter the factory-scoped OpenTelemetry logger; a bounded early-boot queue is
//! drained when search attaches. Exporter instrumentation is suppressed to prevent recursion.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::IsTerminal;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::document::Document;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

/// Application log entries from before telemetry attaches, exported when it does; capped at `QUEUE_LIMIT`.
const QUEUE_LIMIT: usize = 16384;
static APP_QUEUE: Mutex<VecDeque<Document>> = Mutex::new(VecDeque::new());
static TELEMETRY: Mutex<Option<std::sync::Weak<moqui_core::telemetry::Telemetry>>> = Mutex::new(None);

/// Attach the process fallback logger after SearchFacade starts; entered contexts use their own factory.
pub(crate) fn attach_telemetry(factory: &ExecutionContextFactory) {
    let telemetry = moqui_core::telemetry::for_factory(factory);
    *lock(&TELEMETRY) = Some(Arc::downgrade(&telemetry));
    let pending: Vec<_> = lock(&APP_QUEUE).drain(..).collect();
    for entry in pending {
        export_telemetry_log(&telemetry, &entry);
    }
}

fn export_telemetry_log(telemetry: &moqui_core::telemetry::Telemetry, entry: &Document) {
    use moqui_core::telemetry::LogRecord;
    let value = |key| entry.get(key).and_then(Value::as_str).unwrap_or("");
    let severity = match value("level") {
        "ERROR" => 17,
        "WARN" => 13,
        _ => 9,
    };
    let mut record = LogRecord::new(severity, value("level"), value("message"));
    record.timestamp = entry
        .get("@timestamp")
        .and_then(|v| match v {
            Value::I64(v) => Some(*v),
            _ => None,
        })
        .unwrap_or_else(now_millis);
    record.scope_name = value("logger_name").to_string();
    for (old, new) in [
        ("thread_name", "thread.name"),
        ("thread_id", "thread.id"),
        ("user_id", "user.id"),
        ("visitor_id", "moqui.visitor.id"),
        ("source_host", "host.name"),
    ] {
        if let Some(value) = entry.get(old) {
            record.attributes.insert(new.into(), value.clone());
        }
    }
    if let Some(Value::Map(fields)) = entry.get("mdc") {
        record.attributes.extend(fields.clone());
    }
    if let Some(Value::Map(thrown)) = entry.get("thrown") {
        for (old, new) in [
            ("name", "exception.type"),
            ("message", "exception.message"),
            ("stackTrace", "exception.stacktrace"),
        ] {
            if let Some(value) = thrown.get(old) {
                record.attributes.insert(new.into(), value.clone());
            }
        }
    }
    telemetry.record_log(record);
}
/// Lines logged before [`attach_files`] and the sinks after it.
static FILES: Mutex<FileState> = Mutex::new(FileState::Pending(Vec::new()));
/// Lines held for the files before they attach. A failed boot never attaches, so this is capped.
const PENDING_LINE_LIMIT: usize = 20000;

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
    lock(&TAPS).push((tx, least));
    rx
}

fn feed_taps(level: Level, line: &str) {
    let mut taps = lock(&TAPS);
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
    let setting = |name: &str| {
        factory
            .map(|factory| factory.conf_setting("logging", name))
            .unwrap_or_default()
    };
    let mut configured_dir = setting("dir");
    if configured_dir.is_empty() {
        configured_dir = moqui_lib::env::get_prop_or_env("moqui_log_dir");
    }
    let dir = file_dir(runtime_dir, &configured_dir);
    let max_mb = setting("max-size-mb")
        .parse::<u64>()
        .ok()
        .filter(|mb| *mb > 0)
        .unwrap_or(100);
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
        // a run that keeps its own log lines (a Try in sim of the dev tools)
        moqui_core::devtools::log_capture::note(&plain);
        write_file_line(level, &plain);

        // Queued from the start, so the boot lines ship once search attaches.
        if level == Level::DEBUG || level == Level::TRACE || moqui_core::telemetry::is_suppressed() {
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
        let telemetry = moqui_core::context::ExecutionContext::with_current(|context| {
            moqui_core::telemetry::for_factory(context.factory())
        })
        .or_else(|| lock(&TELEMETRY).as_ref().and_then(std::sync::Weak::upgrade));
        if let Some(telemetry) = telemetry {
            export_telemetry_log(&telemetry, &entry);
        } else {
            push(&APP_QUEUE, entry.clone());
        }
        #[cfg(test)]
        if lock(&APP_QUEUE).iter().all(|queued| queued != &entry) {
            push(&APP_QUEUE, entry);
        }
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
#[path = "logging_tests.rs"]
mod tests;
