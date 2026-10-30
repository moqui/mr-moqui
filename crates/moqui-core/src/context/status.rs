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

//! `ExecutionContextFactoryImpl.getStatusMap` and `getComponentInfoList` for the System dashboard.
//!
//! The Java map reads the JVM management beans: heap, non-heap, garbage collection, JIT, and class loading. A Rust
//! process has none of those, so `Process` (resident and peak memory, threads, from `/proc/self/status` on Linux)
//! stands in for `Heap`, `NonHeap`, and `JavaStats`, and `Runtime` for `JavaRuntime`. Disk space needs `statvfs`,
//! which the standard library does not have, so `Disk` is not reported.

use indexmap::IndexMap;
use moqui_lib::object::Value;

use super::ExecutionContextFactory;

fn map(entries: Vec<(&str, Value)>) -> Value {
    Value::Map(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

fn decimal(number: f64, scale: u32) -> Value {
    let text = format!("{number:.prec$}", prec = scale as usize);
    moqui_lib::object::basic_convert(&Value::string(text), "BigDecimal").value
}

/// The fields of `/proc/self/status` in KiB or counts. Empty off Linux.
fn proc_status() -> IndexMap<String, i64> {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return IndexMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let number = rest.split_whitespace().next()?.parse().ok()?;
            Some((name.to_string(), number))
        })
        .collect()
}

/// The one-minute load average, `None` off Linux.
fn load_average() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The status map. `include_sensitive` adds the CPU and OS names, the version, and the data sources, as in Java.
pub fn status_map(factory: &ExecutionContextFactory, include_sensitive: bool) -> IndexMap<String, Value> {
    let processors = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let load = load_average().unwrap_or(0.0);
    #[allow(clippy::cast_precision_loss)] // A processor count fits a double.
    let load_percent = load / processors as f64 * 100.0;
    let status = proc_status();
    let mib = |name: &str| {
        #[allow(clippy::cast_precision_loss)] // Memory sizes in KiB fit a double.
        status
            .get(name)
            .map_or(Value::Null, |kib| decimal(*kib as f64 / 1024.0, 3))
    };
    let now = jiff::Timestamp::now().as_millisecond();
    let started = factory.started_millis();
    #[allow(clippy::cast_precision_loss)] // Milliseconds of uptime fit a double.
    let uptime_hours = (now - started) as f64 / 3_600_000.0;
    let mut out = IndexMap::new();
    out.insert(
        "Utilization".to_string(),
        map(vec![("LoadPercent", decimal(load_percent, 2))]),
    );
    out.insert(
        "Web".to_string(),
        map(vec![
            ("ServerName", Value::string(crate::web::host_name(false))),
            ("HostName", Value::string(crate::web::host_name(true))),
        ]),
    );
    out.insert(
        "Process".to_string(),
        map(vec![
            ("MemoryUsed", mib("VmRSS")),
            ("MemoryPeak", mib("VmHWM")),
            (
                "Threads",
                status.get("Threads").map_or(Value::Null, |count| Value::I64(*count)),
            ),
        ]),
    );
    let mut system = vec![
        ("Load", decimal(load, 2)),
        ("Processors", Value::I64(i64::try_from(processors).unwrap_or(i64::MAX))),
    ];
    if include_sensitive {
        system.push(("CPU", Value::string(std::env::consts::ARCH)));
        system.push(("OsName", Value::string(std::env::consts::OS)));
        system.push(("OsVersion", Value::string(os_release())));
    }
    out.insert("System".to_string(), map(system));
    let start = jiff::Timestamp::from_millisecond(started).map_or(Value::Null, Value::Timestamp);
    out.insert(
        "Runtime".to_string(),
        map(vec![("Start", start), ("UptimeHours", decimal(uptime_hours, 2))]),
    );
    if include_sensitive {
        out.insert("MoquiFramework".to_string(), Value::string(env!("CARGO_PKG_VERSION")));
        out.insert("DataSources".to_string(), Value::List(data_sources(factory)));
    }
    out
}

fn os_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// `EntityFacadeImpl.getDataSourcesInfo`: each entity group with its database and pool.
fn data_sources(factory: &ExecutionContextFactory) -> Vec<Value> {
    let Some(facade) = factory.entity() else {
        return Vec::new();
    };
    let mut groups: Vec<_> = facade.groups().collect();
    groups.sort_by(|left, right| left.0.cmp(right.0));
    groups
        .into_iter()
        .map(|(name, dialect, transactions)| {
            let stats = transactions.stats();
            map(vec![
                ("group", Value::string(name)),
                ("uniqueName", Value::string(format!("{name}_{}", dialect.name))),
                ("database", Value::string(&dialect.name)),
                (
                    "detail",
                    Value::string(format!(
                        "checked out {}, parked {}, waiting {}, idle {} (+{} reserve)",
                        stats.checked_out, stats.parked, stats.waiting, stats.ordinary_idle, stats.reserve_idle
                    )),
                ),
            ])
        })
        .collect()
}

/// `getComponentInfoList`: name, location, and version of each component, in load order.
pub fn component_info_list(factory: &ExecutionContextFactory) -> Vec<IndexMap<String, Value>> {
    factory
        .components()
        .iter()
        .map(|info| {
            let mut row = IndexMap::new();
            row.insert("name".to_string(), Value::string(&info.name));
            row.insert(
                "location".to_string(),
                Value::string(info.location.display().to_string()),
            );
            row.insert("version".to_string(), Value::string(&info.version));
            row.insert(
                "dependsOn".to_string(),
                Value::List(info.depends_on.iter().map(Value::string).collect()),
            );
            row
        })
        .collect()
}

/// The threads of this process for the thread list screen, from `/proc/self/task` on Linux (empty elsewhere): name,
/// id, state (`RUNNABLE`, `WAITING`, ... from the kernel's state letter), and CPU and user time in nanoseconds. Java
/// reads a `ThreadMXBean`; there are no lock or stack details here.
pub fn thread_list() -> Vec<IndexMap<String, Value>> {
    // USER_HZ is 100 on Linux, so a tick is ten milliseconds.
    const TICK_NANOS: i64 = 10_000_000;
    let Ok(entries) = std::fs::read_dir("/proc/self/task") else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let id: i64 = entry.file_name().to_string_lossy().parse().unwrap_or(0);
        let name = std::fs::read_to_string(path.join("comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let stat = std::fs::read_to_string(path.join("stat")).unwrap_or_default();
        // The fields after the name in parentheses: state is the first, utime and stime the 12th and 13th.
        let after: Vec<&str> = stat
            .rsplit_once(')')
            .map_or_else(Vec::new, |(_, rest)| rest.split_whitespace().collect());
        let state = match after.first().copied().unwrap_or("") {
            "R" => "RUNNABLE",
            "S" | "I" => "WAITING",
            "D" => "BLOCKED",
            "T" | "t" => "STOPPED",
            "Z" | "X" => "TERMINATED",
            _ => "UNKNOWN",
        };
        let ticks = |index: usize| after.get(index).and_then(|text| text.parse::<i64>().ok()).unwrap_or(0);
        let user = ticks(11);
        let system = ticks(12);
        let mut row = IndexMap::new();
        row.insert("threadName".to_string(), Value::string(name));
        row.insert("threadId".to_string(), Value::I64(id));
        row.insert("threadState".to_string(), Value::string(state));
        row.insert(
            "cpuTime".to_string(),
            Value::I64((user + system).saturating_mul(TICK_NANOS)),
        );
        row.insert("userTime".to_string(), Value::I64(user.saturating_mul(TICK_NANOS)));
        rows.push(row);
    }
    rows.sort_by_key(|row| match row.get("threadId") {
        Some(Value::I64(id)) => *id,
        _ => 0,
    });
    rows
}
