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

//! What the machine and this process have left (plans/Tools-Fancy.md B2): CPU load, memory, the filesystems the
//! instance writes to, open files and swap, each with a level from the conf's `resource-watch` thresholds. The status
//! map carries them as `Resources` ([`super::status`]), the telemetry gauges record them each minute
//! (`telemetry::register_runtime_gauges`) and the System home's attention list names those that are not `ok`.
//!
//! The readings are kept for [`KEEP`], so a page, the gauges and the attention service share one read; nothing wakes
//! to take them. Linux reports all of them (`/proc`, the cgroup's `memory.max`, `statvfs`), macOS the filesystems,
//! Windows none. A reading the platform does not report is `None`, null in the map: never zero, and it has no level.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::object::Value;
use moqui_lib::sync::lock;

use super::ExecutionContextFactory;

/// How long one read is kept.
pub const KEEP: Duration = Duration::from_secs(10);

/// Over less than this the clock ticks of `/proc/self/stat` are too coarse to give a share.
const MIN_CPU_INTERVAL: Duration = Duration::from_secs(1);

/// The directories of the runtime that are often on a filesystem of their own, with what the row says is on it.
const RUNTIME_DIRECTORIES: [(&str, &str); 5] = [
    ("db", "database"),
    ("log", "logs"),
    ("tmp", "temp"),
    ("txlog", "transaction log"),
    ("opensearch", "search"),
];

/// How a reading stands against its thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warning,
    Critical,
}

impl Level {
    /// `ok`, `warning` or `critical`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// The warning and critical values of one reading, in percent; `None` is no such level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub warning: Option<f64>,
    pub critical: Option<f64>,
}

impl Limits {
    const fn of(warning: f64, critical: f64) -> Self {
        Self {
            warning: Some(warning),
            critical: Some(critical),
        }
    }

    /// The level of a share used: at or above a threshold is that level.
    #[must_use]
    pub fn level_of_used(&self, percent: Option<f64>) -> Option<Level> {
        let percent = percent?;
        Some(if self.critical.is_some_and(|critical| percent >= critical) {
            Level::Critical
        } else if self.warning.is_some_and(|warning| percent >= warning) {
            Level::Warning
        } else {
            Level::Ok
        })
    }

    /// The level of a share left: at or below a threshold is that level.
    #[must_use]
    pub fn level_of_left(&self, percent: Option<f64>) -> Option<Level> {
        let percent = percent?;
        Some(if self.critical.is_some_and(|critical| percent <= critical) {
            Level::Critical
        } else if self.warning.is_some_and(|warning| percent <= warning) {
            Level::Warning
        } else {
            Level::Ok
        })
    }
}

/// The `resource-watch` thresholds; the defaults are those of plans/Tools-Fancy.md §3.4.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// Each filesystem's share of its space used.
    pub disk: Limits,
    /// A filesystem with fewer bytes than this available is critical whatever its share, unless it is no larger
    /// than this itself (a small tmpfs).
    pub disk_free_critical: Option<u64>,
    /// Each filesystem's share of its inodes used.
    pub inodes: Limits,
    /// The process's memory as a share of the limit in force.
    pub memory: Limits,
    /// The system's available memory as a share of its total: a share left.
    pub memory_available: Limits,
    /// The 5-minute load as a share of the processors.
    pub cpu_load: Limits,
    /// Open files as a share of the soft limit.
    pub open_files: Limits,
    /// Swap in use as a share of the swap.
    pub swap: Limits,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            disk: Limits::of(80.0, 90.0),
            disk_free_critical: Some(1024 * 1024 * 1024),
            inodes: Limits::of(80.0, 90.0),
            memory: Limits::of(80.0, 90.0),
            memory_available: Limits::of(10.0, 5.0),
            cpu_load: Limits::of(80.0, 100.0),
            open_files: Limits::of(80.0, 95.0),
            swap: Limits {
                warning: Some(50.0),
                critical: None,
            },
        }
    }
}

impl Thresholds {
    /// The conf's `resource-watch` thresholds. An attribute the conf leaves out, or one that is not a number, keeps
    /// its default; an empty one is no such level.
    #[must_use]
    pub fn from_conf(factory: &ExecutionContextFactory) -> Self {
        Self::from_settings(|name| factory.conf_setting_if_set("resource-watch", name))
    }

    /// [`Self::from_conf`] over any source of the attributes.
    pub fn from_settings(setting: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let number = |name: &str, default: Option<f64>| match setting(name) {
            None => default,
            Some(text) if text.trim().is_empty() => None,
            Some(text) => text
                .trim()
                .parse()
                .ok()
                .filter(|value: &f64| value.is_finite())
                .or(default),
        };
        let limits = |name: &str, default: Limits| Limits {
            warning: number(&format!("{name}-warning"), default.warning),
            critical: number(&format!("{name}-critical"), default.critical),
        };
        let disk_free_critical = match setting("disk-free-critical") {
            None => defaults.disk_free_critical,
            Some(text) if text.trim().is_empty() => None,
            Some(text) => text.trim().parse().ok().or(defaults.disk_free_critical),
        };
        Self {
            disk: limits("disk", defaults.disk),
            disk_free_critical,
            inodes: limits("inodes", defaults.inodes),
            memory: limits("memory", defaults.memory),
            memory_available: limits("memory-available", defaults.memory_available),
            cpu_load: limits("cpu-load", defaults.cpu_load),
            open_files: limits("open-files", defaults.open_files),
            swap: limits("swap", defaults.swap),
        }
    }

    /// A filesystem's level: its share used, or critical when less than the floor is available.
    #[must_use]
    pub fn disk_level(&self, filesystem: &Filesystem) -> Option<Level> {
        let by_share = self.disk.level_of_used(filesystem.used_percent());
        if self
            .disk_free_critical
            .is_some_and(|floor| filesystem.total > floor && filesystem.available < floor)
        {
            return Some(Level::Critical);
        }
        by_share
    }
}

/// `part` of `whole` in percent; `None` when either is missing or the whole is zero.
#[allow(clippy::cast_precision_loss)] // Byte and file counts fit a double well enough for a percentage.
fn percent(part: Option<u64>, whole: Option<u64>) -> Option<f64> {
    let (part, whole) = (part?, whole?);
    (whole > 0).then(|| part as f64 / whole as f64 * 100.0)
}

/// One filesystem the instance writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filesystem {
    /// Where it is mounted.
    pub mount_point: String,
    /// What of the instance is on it: `runtime`, `database`, `logs`, `temp`, `transaction log`, `search`,
    /// `system temp`.
    pub holds: Vec<String>,
    /// Its size in bytes.
    pub total: u64,
    /// The bytes free, with those kept for the superuser.
    pub free: u64,
    /// The bytes the server's user may still write.
    pub available: u64,
    /// Its inodes and those free; `None` where the filesystem does not count them.
    pub inodes: Option<Inodes>,
}

/// A filesystem's file entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inodes {
    pub total: u64,
    pub free: u64,
}

impl Filesystem {
    /// The share used, as `df` has it: what is used over what is used and what the user may still write.
    #[must_use]
    pub fn used_percent(&self) -> Option<f64> {
        let used = self.total.saturating_sub(self.free);
        percent(Some(used), Some(used.saturating_add(self.available)))
    }

    /// The share of the inodes used.
    #[must_use]
    pub fn inodes_used_percent(&self) -> Option<f64> {
        let inodes = self.inodes?;
        percent(Some(inodes.total.saturating_sub(inodes.free)), Some(inodes.total))
    }
}

/// Where the memory limit in force comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitSource {
    /// The cgroup's `memory.max`: a container's or a service unit's.
    Container,
    /// The machine's memory.
    System,
}

impl LimitSource {
    /// `container` or `system`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::System => "system",
        }
    }
}

/// One read of the machine and the process. Sizes are bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readings {
    pub processors: usize,
    /// The 1, 5 and 15-minute load averages.
    pub load: Option<[f64; 3]>,
    /// This process's share of the machine's processors between the read before this one and this one, 0 to 1.
    pub process_cpu: Option<f64>,
    pub process_memory: Option<u64>,
    pub process_memory_peak: Option<u64>,
    pub threads: Option<u64>,
    pub memory_total: Option<u64>,
    pub memory_available: Option<u64>,
    /// The cgroup's memory limit, when one is set.
    pub memory_container_limit: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_free: Option<u64>,
    pub filesystems: Vec<Filesystem>,
    pub open_files: Option<u64>,
    pub open_files_limit: Option<u64>,
}

impl Readings {
    /// The 5-minute load as a share of the processors, in percent.
    #[must_use]
    pub fn load_percent(&self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)] // A processor count fits a double.
        self.load.map(|load| load[1] / self.processors.max(1) as f64 * 100.0)
    }

    /// The memory limit in force: the container's when it is below the machine's memory (or the machine's is not
    /// known), else the machine's.
    #[must_use]
    pub fn memory_limit(&self) -> Option<(u64, LimitSource)> {
        match (self.memory_container_limit, self.memory_total) {
            (Some(limit), Some(total)) if limit < total => Some((limit, LimitSource::Container)),
            (_, Some(total)) => Some((total, LimitSource::System)),
            (Some(limit), None) => Some((limit, LimitSource::Container)),
            (None, None) => None,
        }
    }

    /// The process's memory as a share of the limit in force, in percent.
    #[must_use]
    pub fn process_memory_percent(&self) -> Option<f64> {
        percent(self.process_memory, self.memory_limit().map(|(limit, _)| limit))
    }

    /// The system's available memory as a share of its total, in percent.
    #[must_use]
    pub fn memory_available_percent(&self) -> Option<f64> {
        percent(self.memory_available, self.memory_total)
    }

    /// The swap in use, in bytes.
    #[must_use]
    pub fn swap_used(&self) -> Option<u64> {
        Some(self.swap_total?.saturating_sub(self.swap_free?))
    }

    /// The swap in use as a share of the swap, in percent; `None` with no swap.
    #[must_use]
    pub fn swap_used_percent(&self) -> Option<f64> {
        percent(self.swap_used(), self.swap_total)
    }

    /// Open files as a share of the soft limit, in percent.
    #[must_use]
    pub fn open_files_percent(&self) -> Option<f64> {
        percent(self.open_files, self.open_files_limit)
    }

    /// The `Resources` map of the status map: `Cpu`, `Memory`, `Swap`, `Filesystems` and `OpenFiles`, each reading
    /// with its level under `thresholds` (`ok`, `warning`, `critical`, or null when it is not reported).
    #[must_use]
    pub fn to_value(&self, thresholds: &Thresholds) -> Value {
        let load = |index: usize| number(self.load.map(|load| load[index]));
        let limit = self.memory_limit();
        let filesystems = self
            .filesystems
            .iter()
            .map(|filesystem| {
                map(vec![
                    ("mountPoint", Value::string(&filesystem.mount_point)),
                    (
                        "holds",
                        Value::List(filesystem.holds.iter().map(Value::string).collect()),
                    ),
                    ("totalBytes", count(Some(filesystem.total))),
                    ("availableBytes", count(Some(filesystem.available))),
                    ("usedPercent", number(filesystem.used_percent())),
                    ("inodesTotal", count(filesystem.inodes.map(|inodes| inodes.total))),
                    ("inodesFree", count(filesystem.inodes.map(|inodes| inodes.free))),
                    ("inodesUsedPercent", number(filesystem.inodes_used_percent())),
                    ("level", level(thresholds.disk_level(filesystem))),
                    (
                        "inodesLevel",
                        level(thresholds.inodes.level_of_used(filesystem.inodes_used_percent())),
                    ),
                ])
            })
            .collect();
        map(vec![
            (
                "Cpu",
                map(vec![
                    ("Load1", load(0)),
                    ("Load5", load(1)),
                    ("Load15", load(2)),
                    ("Processors", count(u64::try_from(self.processors).ok())),
                    ("LoadPercent", number(self.load_percent())),
                    ("ProcessPercent", number(self.process_cpu.map(|share| share * 100.0))),
                    ("Level", level(thresholds.cpu_load.level_of_used(self.load_percent()))),
                ]),
            ),
            (
                "Memory",
                map(vec![
                    ("ProcessBytes", count(self.process_memory)),
                    ("ProcessPeakBytes", count(self.process_memory_peak)),
                    ("TotalBytes", count(self.memory_total)),
                    ("AvailableBytes", count(self.memory_available)),
                    ("AvailablePercent", number(self.memory_available_percent())),
                    ("LimitBytes", count(limit.map(|(bytes, _)| bytes))),
                    (
                        "LimitSource",
                        limit.map_or(Value::Null, |(_, source)| Value::string(source.word())),
                    ),
                    ("ProcessPercent", number(self.process_memory_percent())),
                    (
                        "Level",
                        level(thresholds.memory.level_of_used(self.process_memory_percent())),
                    ),
                    (
                        "AvailableLevel",
                        level(
                            thresholds
                                .memory_available
                                .level_of_left(self.memory_available_percent()),
                        ),
                    ),
                ]),
            ),
            (
                "Swap",
                map(vec![
                    ("TotalBytes", count(self.swap_total)),
                    ("UsedBytes", count(self.swap_used())),
                    ("UsedPercent", number(self.swap_used_percent())),
                    ("Level", level(thresholds.swap.level_of_used(self.swap_used_percent()))),
                ]),
            ),
            ("Filesystems", Value::List(filesystems)),
            (
                "OpenFiles",
                map(vec![
                    ("Count", count(self.open_files)),
                    ("Limit", count(self.open_files_limit)),
                    ("UsedPercent", number(self.open_files_percent())),
                    (
                        "Level",
                        level(thresholds.open_files.level_of_used(self.open_files_percent())),
                    ),
                ]),
            ),
        ])
    }
}

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

/// A number to two decimals, or null.
fn number(value: Option<f64>) -> Value {
    value
        .filter(|value| value.is_finite())
        .map_or(Value::Null, |value| Value::F64((value * 100.0).round() / 100.0))
}

fn count(value: Option<u64>) -> Value {
    value.map_or(Value::Null, |value| {
        Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
    })
}

fn level(value: Option<Level>) -> Value {
    value.map_or(Value::Null, |level| Value::string(level.word()))
}

// ---- parsing ----

/// The three load averages of `/proc/loadavg`.
pub(crate) fn parse_load(text: &str) -> Option<[f64; 3]> {
    let mut fields = text.split_whitespace();
    let mut next = || -> Option<f64> { fields.next()?.parse().ok() };
    Some([next()?, next()?, next()?])
}

/// The `Name:   123 kB` lines of `/proc/self/status` and `/proc/meminfo`, by name, in KiB or counts.
pub(crate) fn parse_fields(text: &str) -> IndexMap<String, u64> {
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let number = rest.split_whitespace().next()?.parse().ok()?;
            Some((name.to_string(), number))
        })
        .collect()
}

/// A process's CPU time (user and system, every thread) from its `/proc/<pid>/stat`.
pub(crate) fn parse_cpu_time(stat: &str) -> Option<Duration> {
    // USER_HZ is 100 on Linux, so a tick is ten milliseconds.
    const TICK_MILLIS: u64 = 10;
    // The fields after the name in parentheses: state is the first, utime and stime the 12th and 13th.
    let (_, rest) = stat.rsplit_once(')')?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(Duration::from_millis(
        utime.saturating_add(stime).saturating_mul(TICK_MILLIS),
    ))
}

/// The cgroup v2 path of `/proc/self/cgroup` (its `0::/path` line).
pub(crate) fn parse_cgroup_path(text: &str) -> Option<&str> {
    text.lines().find_map(|line| line.strip_prefix("0::"))
}

/// A cgroup's `memory.max`: bytes, or `None` for `max` (no limit).
pub(crate) fn parse_memory_max(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// The soft limit of open files from `/proc/self/limits`; `None` when it is unlimited.
pub(crate) fn parse_open_files_limit(text: &str) -> Option<u64> {
    let rest = text.lines().find_map(|line| line.strip_prefix("Max open files"))?;
    rest.split_whitespace().next()?.parse().ok()
}

// ---- reading ----

/// This process's CPU time (user and system, every thread), from `/proc/self/stat`; `None` off Linux.
#[must_use]
pub fn process_cpu_time() -> Option<Duration> {
    parse_cpu_time(&std::fs::read_to_string("/proc/self/stat").ok()?)
}

/// The share of the machine's processors a process used between two reads of its CPU time, 0 to 1; `None` when the
/// reads are less than a second apart.
#[must_use]
pub fn cpu_share(before: (Instant, Duration), after: (Instant, Duration), processors: usize) -> Option<f64> {
    let wall = after.0.checked_duration_since(before.0)?;
    if wall < MIN_CPU_INTERVAL {
        return None;
    }
    #[allow(clippy::cast_precision_loss)] // A processor count fits a double.
    let capacity = wall.as_secs_f64() * processors.max(1) as f64;
    Some((after.1.saturating_sub(before.1).as_secs_f64() / capacity).clamp(0.0, 1.0))
}

/// The lowest `memory.max` of this process's cgroup and its parents (cgroup v2), which is the limit that binds.
fn container_memory_limit() -> Option<u64> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let path = parse_cgroup_path(&cgroup)?.trim_start_matches('/');
    let root = Path::new("/sys/fs/cgroup");
    let mut directory = root.join(path);
    let mut lowest: Option<u64> = None;
    loop {
        let limit = std::fs::read_to_string(directory.join("memory.max"))
            .ok()
            .and_then(|text| parse_memory_max(&text));
        lowest = match (lowest, limit) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        };
        if directory == root || !directory.pop() || !directory.starts_with(root) {
            return lowest;
        }
    }
}

/// The files this process has open: the entries of `/proc/self/fd` less the one the listing itself holds.
fn open_files() -> Option<u64> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    u64::try_from(entries.count().saturating_sub(1)).ok()
}

/// The directories whose filesystems are reported for an instance at `runtime`, each with what the row says is on
/// it: the runtime, those of its directories that are often mounted apart, and the system temp directory.
fn watched_directories(runtime: &Path) -> Vec<(String, PathBuf)> {
    let mut directories = vec![("runtime".to_string(), runtime.to_path_buf())];
    for (name, holds) in RUNTIME_DIRECTORIES {
        directories.push((holds.to_string(), runtime.join(name)));
    }
    directories.push(("system temp".to_string(), std::env::temp_dir()));
    directories
}

/// One row per filesystem `directories` are on, in their order: a second directory on a filesystem already listed
/// adds what it holds to that row, unless it is inside a directory of the row (the runtime's `log` on the runtime's
/// own filesystem says nothing new). A directory that is not there, or cannot be read, is left out.
#[cfg(unix)]
pub(crate) fn filesystems(directories: &[(String, PathBuf)]) -> Vec<Filesystem> {
    use std::os::unix::fs::MetadataExt;
    struct Row {
        device: u64,
        paths: Vec<PathBuf>,
        filesystem: Filesystem,
    }
    let mut rows: Vec<Row> = Vec::new();
    for (holds, directory) in directories {
        let Ok(directory) = directory.canonicalize() else {
            continue;
        };
        let Ok(device) = std::fs::metadata(&directory).map(|metadata| metadata.dev()) else {
            continue;
        };
        if let Some(row) = rows.iter_mut().find(|row| row.device == device) {
            if !row.paths.iter().any(|path| directory.starts_with(path)) {
                row.filesystem.holds.push(holds.clone());
                row.paths.push(directory);
            }
            continue;
        }
        let Ok(stat) = rustix::fs::statvfs(&directory) else {
            continue;
        };
        // The mount point is the highest parent on the same device.
        let mut mount_point = directory.clone();
        while let Some(parent) = mount_point.parent() {
            match std::fs::metadata(parent) {
                Ok(metadata) if metadata.dev() == device => mount_point = parent.to_path_buf(),
                _ => break,
            }
        }
        let block = if stat.f_frsize > 0 { stat.f_frsize } else { stat.f_bsize };
        rows.push(Row {
            device,
            paths: vec![directory],
            filesystem: Filesystem {
                mount_point: mount_point.display().to_string(),
                holds: vec![holds.clone()],
                total: stat.f_blocks.saturating_mul(block),
                free: stat.f_bfree.saturating_mul(block),
                available: stat.f_bavail.saturating_mul(block),
                inodes: (stat.f_files > 0).then_some(Inodes {
                    total: stat.f_files,
                    free: stat.f_ffree,
                }),
            },
        });
    }
    rows.into_iter().map(|row| row.filesystem).collect()
}

/// Off Unix no filesystem is reported until a reader without `unsafe` is chosen (plans/Tools-Fancy.md §10).
#[cfg(not(unix))]
pub(crate) fn filesystems(_directories: &[(String, PathBuf)]) -> Vec<Filesystem> {
    Vec::new()
}

/// The readings that are parsed from texts, each `None` where its text is: `/proc/loadavg`, `/proc/self/status`,
/// `/proc/meminfo` and `/proc/self/limits`.
pub(crate) fn parse_readings(
    load: Option<&str>,
    status: Option<&str>,
    meminfo: Option<&str>,
    limits: Option<&str>,
) -> Readings {
    let status = status.map(parse_fields).unwrap_or_default();
    let meminfo = meminfo.map(parse_fields).unwrap_or_default();
    let bytes = |fields: &IndexMap<String, u64>, name: &str| fields.get(name).map(|kib| kib.saturating_mul(1024));
    Readings {
        load: load.and_then(parse_load),
        process_memory: bytes(&status, "VmRSS"),
        process_memory_peak: bytes(&status, "VmHWM"),
        threads: status.get("Threads").copied(),
        memory_total: bytes(&meminfo, "MemTotal"),
        memory_available: bytes(&meminfo, "MemAvailable"),
        swap_total: bytes(&meminfo, "SwapTotal"),
        swap_free: bytes(&meminfo, "SwapFree"),
        open_files_limit: limits.and_then(parse_open_files_limit),
        ..Readings::default()
    }
}

/// Read everything now. `last_cpu` is the process's CPU time at the read before, replaced by this one's.
fn read(runtime: &Path, last_cpu: &mut Option<(Instant, Duration)>) -> Readings {
    let text = |path: &str| std::fs::read_to_string(path).ok();
    let processors = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let cpu = process_cpu_time().map(|time| (Instant::now(), time));
    let process_cpu = match (*last_cpu, cpu) {
        (Some(before), Some(after)) => cpu_share(before, after, processors),
        _ => None,
    };
    // a read too soon after the last keeps the older mark, so the next share is over a long enough interval
    if cpu.is_some() && (process_cpu.is_some() || last_cpu.is_none()) {
        *last_cpu = cpu;
    }
    Readings {
        processors,
        process_cpu,
        memory_container_limit: container_memory_limit(),
        filesystems: filesystems(&watched_directories(runtime)),
        open_files: open_files(),
        ..parse_readings(
            text("/proc/loadavg").as_deref(),
            text("/proc/self/status").as_deref(),
            text("/proc/meminfo").as_deref(),
            text("/proc/self/limits").as_deref(),
        )
    }
}

/// The last read and the process's CPU time then, per factory.
#[derive(Default)]
struct Kept {
    state: Mutex<KeptState>,
}

#[derive(Default)]
struct KeptState {
    readings: Option<(Instant, Arc<Readings>)>,
    last_cpu: Option<(Instant, Duration)>,
}

/// The readings, read now when the last read is older than [`KEEP`].
pub fn readings(factory: &ExecutionContextFactory) -> Arc<Readings> {
    let kept = factory.extension(|_| Kept::default());
    // held over the read, so callers that arrive together share one
    let mut state = lock(&kept.state);
    if let Some((at, readings)) = &state.readings {
        if at.elapsed() < KEEP {
            return Arc::clone(readings);
        }
    }
    let readings = Arc::new(read(factory.runtime_dir(), &mut state.last_cpu));
    state.readings = Some((Instant::now(), Arc::clone(&readings)));
    readings
}

/// The `Resources` map of the status map, from the kept readings and the conf's thresholds.
pub fn resources_value(factory: &ExecutionContextFactory) -> Value {
    readings(factory).to_value(&Thresholds::from_conf(factory))
}

#[cfg(test)]
#[path = "resources_tests.rs"]
mod tests;
