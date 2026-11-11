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

//! `ExecutionContextFactoryImpl.getStatusMap` and `getComponentInfoList` for the System home.
//!
//! The Java map reads the JVM management beans: heap, non-heap, garbage collection, JIT, and class loading. A Rust
//! process has none of those, so `Process` (resident and peak memory, threads, from `/proc/self/status` on Linux)
//! stands in for `Heap`, `NonHeap`, and `JavaStats`, and `Runtime` for `JavaRuntime`. What the machine has left (CPU
//! load, memory against its limit, the filesystems, open files, swap) is the `Resources` map, each reading with its
//! level ([`super::resources`], plans/Tools-Fancy.md B2); `Ready` and `Draining` are what `/health` answers, and
//! `RequestPool` the request workers.

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

/// The status map. `include_sensitive` adds the CPU and OS names, the version, and the data sources, as in Java.
pub fn status_map(factory: &ExecutionContextFactory, include_sensitive: bool) -> moqui_lib::object::ValueMap {
    let readings = super::resources::readings(factory);
    let processors = readings.processors;
    let load = readings.load.map_or(0.0, |load| load[0]);
    #[allow(clippy::cast_precision_loss)] // A processor count fits a double.
    let load_percent = load / processors.max(1) as f64 * 100.0;
    let mib = |bytes: Option<u64>| {
        #[allow(clippy::cast_precision_loss)] // Memory sizes fit a double.
        bytes.map_or(Value::Null, |bytes| decimal(bytes as f64 / (1024.0 * 1024.0), 3))
    };
    let now = jiff::Timestamp::now().as_millisecond();
    let started = factory.started_millis();
    #[allow(clippy::cast_precision_loss)] // Milliseconds of uptime fit a double.
    let uptime_hours = (now - started) as f64 / 3_600_000.0;
    let mut out = IndexMap::default();
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
            ("MemoryUsed", mib(readings.process_memory)),
            ("MemoryPeak", mib(readings.process_memory_peak)),
            ("Threads", readings.threads.map_or(Value::Null, count)),
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
    out.insert(
        "Resources".to_string(),
        readings.to_value(&super::resources::Thresholds::from_conf(factory)),
    );
    out.insert("Ready".to_string(), Value::Bool(is_ready(factory)));
    out.insert("Draining".to_string(), Value::Bool(crate::drain::is_draining(factory)));
    let requests = factory.request_pool().status();
    out.insert(
        "RequestPool".to_string(),
        map(vec![
            ("Workers", size(requests.workers)),
            ("Busy", size(requests.busy)),
            ("Queued", size(requests.queued)),
            ("QueueCap", size(requests.queue_cap)),
        ]),
    );
    if include_sensitive {
        out.insert("MoquiFramework".to_string(), Value::string(env!("CARGO_PKG_VERSION")));
        out.insert("DataSources".to_string(), Value::List(data_sources(factory)));
    }
    out
}

fn count(value: u64) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

fn size(value: usize) -> Value {
    Value::I64(i64::try_from(value).unwrap_or(i64::MAX))
}

/// Whether the server can serve: the default group's database is open, as `/health` asks.
#[must_use]
pub fn is_ready(factory: &ExecutionContextFactory) -> bool {
    factory
        .entity()
        .is_some_and(|facade| facade.transactions_for("moqui.basic.Enumeration").is_ok())
}

fn os_release() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// `EntityFacadeImpl.getDataSourcesInfo`: each entity group with its database and pool, the pool as a sentence
/// (`detail`) and as numbers; `embedded` says the database is the cluster this process runs.
fn data_sources(factory: &ExecutionContextFactory) -> Vec<Value> {
    let Some(facade) = factory.entity() else {
        return Vec::new();
    };
    let conf = moqui_lib::node::MNode::parse_shared("moqui-conf", &factory.conf_text()).ok();
    let mut groups: Vec<_> = facade.groups().collect();
    groups.sort_by(|left, right| left.0.cmp(right.0));
    groups
        .into_iter()
        .map(|(name, dialect, transactions)| {
            let stats = transactions.stats();
            let embedded = conf
                .as_ref()
                .and_then(|conf| crate::entity::datasource::datasource_conf(conf, name).ok())
                .map(|datasource| datasource.kind == crate::entity::datasource::DatasourceKind::Embedded);
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
                ("checkedOut", count(u64::from(stats.checked_out))),
                ("parked", count(u64::from(stats.parked))),
                ("waiting", count(u64::from(stats.waiting))),
                ("idle", count(u64::from(stats.ordinary_idle))),
                ("reserveIdle", count(u64::from(stats.reserve_idle))),
                ("max", count(u64::from(transactions.pool_max()))),
                ("embedded", embedded.map_or(Value::Null, Value::Bool)),
            ])
        })
        .collect()
}

/// `getComponentInfoList`: name, location, and version of each component, in load order.
pub fn component_info_list(factory: &ExecutionContextFactory) -> Vec<moqui_lib::object::ValueMap> {
    factory
        .components()
        .iter()
        .map(|info| {
            let mut row = IndexMap::default();
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

/// The threads of this process for the thread list screen, from `/proc/self/task` on Linux: name, id, state
/// (`RUNNABLE`, `WAITING`, ... from the kernel's state letter), and CPU and user time in nanoseconds. Java reads a
/// `ThreadMXBean` for locks and stacks; here a thread that runs an artifact has what it runs instead
/// ([`super::activity`]): `work` and `workSeconds` (the first frame and how long it has run), `running` (the top
/// frame), `userId`, `traceId` and `stackList` (the artifact frames, top first). Where the platform lists no threads
/// the rows are the threads that run an artifact, with no state or times.
pub fn thread_list() -> Vec<moqui_lib::object::ValueMap> {
    let mut rows = kernel_threads();
    for activity in super::activity::running() {
        let known = rows
            .iter_mut()
            .find(|row| activity.thread_id != 0 && row.get("threadId") == Some(&Value::I64(activity.thread_id)));
        match known {
            Some(row) => add_activity(row, &activity),
            None => {
                let mut row = IndexMap::default();
                row.insert("threadName".to_string(), Value::string(&activity.thread_name));
                if activity.thread_id != 0 {
                    row.insert("threadId".to_string(), Value::I64(activity.thread_id));
                }
                add_activity(&mut row, &activity);
                rows.push(row);
            }
        }
    }
    rows.sort_by_key(|row| match row.get("threadId") {
        Some(Value::I64(id)) => *id,
        _ => 0,
    });
    rows
}

/// What a thread runs, on its row of the thread list.
fn add_activity(row: &mut moqui_lib::object::ValueMap, activity: &super::activity::ThreadActivity) {
    // AT_XML_SCREEN_TRANS as "xml screen trans"
    let kind = |artifact_type: &str| artifact_type.trim_start_matches("AT_").replace('_', " ").to_lowercase();
    if let Some(first) = activity.frames.first() {
        row.insert("work".to_string(), Value::string(&first.name));
        row.insert("workType".to_string(), Value::string(kind(&first.artifact_type)));
        row.insert("workSeconds".to_string(), Value::F64(first.seconds));
    }
    if let Some(top) = activity.frames.last() {
        row.insert("running".to_string(), Value::string(&top.name));
        row.insert("runningType".to_string(), Value::string(kind(&top.artifact_type)));
    }
    if let Some(user_id) = &activity.user_id {
        row.insert("userId".to_string(), Value::string(user_id));
    }
    if let Some(trace_id) = &activity.trace_id {
        row.insert("traceId".to_string(), Value::string(trace_id));
    }
    let stack = activity
        .frames
        .iter()
        .rev()
        .map(|frame| {
            Value::string(format!(
                "{} {} ({}) {:.3}s",
                kind(&frame.artifact_type),
                frame.name,
                frame.action,
                frame.seconds
            ))
        })
        .collect();
    row.insert("stackList".to_string(), Value::List(stack));
}

/// The kernel's threads of this process (`/proc/self/task`), none where there is no such list.
fn kernel_threads() -> Vec<moqui_lib::object::ValueMap> {
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
        let mut row = IndexMap::default();
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
    rows
}
