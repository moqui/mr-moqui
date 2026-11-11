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

use super::*;

const GIB: u64 = 1024 * 1024 * 1024;

const LOADAVG: &str = "1.50 4.00 2.25 3/1234 56789\n";
const STATUS: &str = "Name:\tmoqui\nVmHWM:\t  600000 kB\nVmRSS:\t  524288 kB\nThreads:\t42\n";
const MEMINFO: &str = "MemTotal:       16777216 kB\nMemFree:          100000 kB\nMemAvailable:    1048576 kB\n\
                       SwapTotal:       2097152 kB\nSwapFree:        1572864 kB\n";
const LIMITS: &str = "Limit                     Soft Limit           Hard Limit           Units     \n\
                      Max cpu time              unlimited            unlimited            seconds   \n\
                      Max open files            1024                 524288               files     \n";

fn sample() -> Readings {
    Readings {
        processors: 8,
        ..parse_readings(Some(LOADAVG), Some(STATUS), Some(MEMINFO), Some(LIMITS))
    }
}

fn filesystem(total: u64, available: u64) -> Filesystem {
    Filesystem {
        mount_point: "/var".into(),
        holds: vec!["runtime".into()],
        total,
        free: available,
        available,
        inodes: Some(Inodes { total: 1000, free: 100 }),
    }
}

/// The value at `path` (`Memory.Level`) of a `Resources` map.
fn at<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(value, |value, name| {
        let Value::Map(map) = value else {
            panic!("{name} is in a map")
        };
        map.get(name).unwrap_or_else(|| panic!("{name} is in the map"))
    })
}

#[test]
fn a_share_used_is_graded_below_at_and_above_each_threshold() {
    let limits = Limits::of(80.0, 90.0);
    for (percent, level) in [
        (79.99, Level::Ok),
        (80.0, Level::Warning),
        (89.99, Level::Warning),
        (90.0, Level::Critical),
        (150.0, Level::Critical),
    ] {
        assert_eq!(limits.level_of_used(Some(percent)), Some(level), "{percent}");
    }
    assert_eq!(limits.level_of_used(None), None);
    // a level that is off is never given
    let warning_only = Limits {
        warning: Some(50.0),
        critical: None,
    };
    assert_eq!(warning_only.level_of_used(Some(100.0)), Some(Level::Warning));
    let neither = Limits {
        warning: None,
        critical: None,
    };
    assert_eq!(neither.level_of_used(Some(100.0)), Some(Level::Ok));
}

#[test]
fn a_share_left_is_graded_above_at_and_below_each_threshold() {
    let limits = Thresholds::default().memory_available;
    for (percent, level) in [
        (10.01, Level::Ok),
        (10.0, Level::Warning),
        (5.01, Level::Warning),
        (5.0, Level::Critical),
        (0.0, Level::Critical),
    ] {
        assert_eq!(limits.level_of_left(Some(percent)), Some(level), "{percent}");
    }
    assert_eq!(limits.level_of_left(None), None);
}

#[test]
fn the_default_thresholds_grade_each_reading() {
    let thresholds = Thresholds::default();
    for (limits, ok, warning, critical) in [
        (thresholds.disk, 79.0, 80.0, 90.0),
        (thresholds.inodes, 79.0, 80.0, 90.0),
        (thresholds.memory, 79.0, 80.0, 90.0),
        (thresholds.cpu_load, 79.0, 80.0, 100.0),
        (thresholds.open_files, 79.0, 80.0, 95.0),
    ] {
        assert_eq!(limits.level_of_used(Some(ok)), Some(Level::Ok));
        assert_eq!(limits.level_of_used(Some(warning)), Some(Level::Warning));
        assert_eq!(limits.level_of_used(Some(critical - 0.01)), Some(Level::Warning));
        assert_eq!(limits.level_of_used(Some(critical)), Some(Level::Critical));
    }
    // swap warns and is never critical
    assert_eq!(thresholds.swap.level_of_used(Some(49.0)), Some(Level::Ok));
    assert_eq!(thresholds.swap.level_of_used(Some(50.0)), Some(Level::Warning));
    assert_eq!(thresholds.swap.level_of_used(Some(100.0)), Some(Level::Warning));
}

#[test]
fn a_filesystem_is_critical_by_its_share_or_under_the_free_floor() {
    let thresholds = Thresholds::default();
    // 100 GiB: 79 %, 80 % and 90 % used
    assert_eq!(thresholds.disk_level(&filesystem(100 * GIB, 21 * GIB)), Some(Level::Ok));
    assert_eq!(
        thresholds.disk_level(&filesystem(100 * GIB, 20 * GIB)),
        Some(Level::Warning)
    );
    assert_eq!(
        thresholds.disk_level(&filesystem(100 * GIB, 10 * GIB)),
        Some(Level::Critical)
    );
    // 5 GiB at 79 % used is ok by its share and has just over 1 GiB left; one byte under the floor is critical
    assert_eq!(
        thresholds.disk_level(&filesystem(5 * GIB, GIB + GIB / 20)),
        Some(Level::Ok)
    );
    assert_eq!(thresholds.disk_level(&filesystem(5 * GIB, GIB)), Some(Level::Warning));
    assert_eq!(
        thresholds.disk_level(&filesystem(5 * GIB, GIB - 1)),
        Some(Level::Critical)
    );
    // a filesystem no larger than the floor is graded by its share alone
    assert_eq!(thresholds.disk_level(&filesystem(GIB / 2, GIB / 4)), Some(Level::Ok));
    let no_floor = Thresholds {
        disk_free_critical: None,
        ..thresholds
    };
    assert_eq!(no_floor.disk_level(&filesystem(5 * GIB, GIB - 1)), Some(Level::Warning));
}

#[test]
fn the_thresholds_come_from_the_conf_and_a_bad_value_keeps_its_default() {
    let conf = |name: &str| match name {
        "disk-warning" => Some("70".to_string()),
        "disk-critical" => Some(String::new()),
        "disk-free-critical" => Some(" 2048 ".to_string()),
        "memory-warning" => Some("lots".to_string()),
        "swap-critical" => Some("75".to_string()),
        _ => None,
    };
    let thresholds = Thresholds::from_settings(conf);
    let defaults = Thresholds::default();
    assert_eq!(
        thresholds.disk,
        Limits {
            warning: Some(70.0),
            critical: None
        }
    );
    assert_eq!(thresholds.disk_free_critical, Some(2048));
    assert_eq!(thresholds.memory, defaults.memory);
    assert_eq!(thresholds.swap.critical, Some(75.0));
    assert_eq!(thresholds.cpu_load, defaults.cpu_load);
    assert_eq!(Thresholds::from_settings(|_| None), defaults);
}

#[test]
fn readings_are_parsed_from_the_proc_texts() {
    let readings = sample();
    assert_eq!(readings.load, Some([1.5, 4.0, 2.25]));
    assert_eq!(readings.load_percent(), Some(50.0));
    assert_eq!(readings.process_memory, Some(512 * 1024 * 1024));
    assert_eq!(readings.process_memory_peak, Some(600_000 * 1024));
    assert_eq!(readings.threads, Some(42));
    assert_eq!(readings.memory_total, Some(16 * GIB));
    assert_eq!(readings.memory_available, Some(GIB));
    assert_eq!(readings.memory_available_percent(), Some(6.25));
    assert_eq!(readings.swap_used(), Some(GIB / 2));
    assert_eq!(readings.swap_used_percent(), Some(25.0));
    assert_eq!(readings.open_files_limit, Some(1024));
    // the name of the process may hold spaces and parentheses; utime and stime are the 14th and 15th fields
    let stat = "4242 (moqui (worker) 1) S 1 4242 4242 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 30 0 100 1000 200";
    assert_eq!(parse_cpu_time(stat), Some(Duration::from_secs(3)));
    assert_eq!(parse_cpu_time("not a stat line"), None);
    assert_eq!(
        parse_cgroup_path("0::/system.slice/moqui.service\n"),
        Some("/system.slice/moqui.service")
    );
    assert_eq!(parse_cgroup_path("12:memory:/docker/abc\n"), None);
    assert_eq!(parse_memory_max("4294967296\n"), Some(4 * GIB));
    assert_eq!(parse_memory_max("max\n"), None);
    assert_eq!(
        parse_open_files_limit("Max open files   unlimited   unlimited   files\n"),
        None
    );
}

#[test]
fn the_process_cpu_share_is_of_every_processor_between_two_reads() {
    let start = Instant::now();
    let at = |seconds: u64, cpu: u64| (start + Duration::from_secs(seconds), Duration::from_secs(cpu));
    // four seconds of CPU in ten seconds on four processors
    assert_eq!(cpu_share(at(0, 10), at(10, 14), 4), Some(0.1));
    assert_eq!(cpu_share(at(0, 10), at(10, 10), 4), Some(0.0));
    // reads too close together give no share, and neither does a clock that went back
    assert_eq!(
        cpu_share(
            (start, Duration::ZERO),
            (start + Duration::from_millis(200), Duration::ZERO),
            4
        ),
        None
    );
    assert_eq!(cpu_share(at(10, 0), at(0, 0), 4), None);
}

#[test]
fn a_container_limit_below_the_systems_is_the_limit_in_force() {
    let mut readings = sample();
    assert_eq!(readings.memory_limit(), Some((16 * GIB, LimitSource::System)));
    assert_eq!(readings.process_memory_percent(), Some(3.125));
    readings.memory_container_limit = Some(GIB);
    assert_eq!(readings.memory_limit(), Some((GIB, LimitSource::Container)));
    assert_eq!(readings.process_memory_percent(), Some(50.0));
    // a limit above the machine's memory does not bind
    readings.memory_container_limit = Some(64 * GIB);
    assert_eq!(readings.memory_limit(), Some((16 * GIB, LimitSource::System)));
    let value = Readings {
        memory_container_limit: Some(GIB / 2 + GIB / 8),
        ..sample()
    }
    .to_value(&Thresholds::default());
    assert_eq!(at(&value, "Memory.LimitBytes"), &Value::I64((GIB / 2 + GIB / 8) as i64));
    assert_eq!(at(&value, "Memory.LimitSource").as_str(), Some("container"));
    // 512 MiB of 640 MiB
    assert_eq!(at(&value, "Memory.ProcessPercent"), &Value::F64(80.0));
    assert_eq!(at(&value, "Memory.Level").as_str(), Some("warning"));
}

#[test]
fn the_resources_map_has_each_reading_with_its_level() {
    let mut readings = sample();
    readings.process_cpu = Some(0.125);
    readings.open_files = Some(990);
    readings.filesystems = vec![filesystem(100 * GIB, 9 * GIB)];
    let value = readings.to_value(&Thresholds::default());
    assert_eq!(at(&value, "Cpu.Load1"), &Value::F64(1.5));
    assert_eq!(at(&value, "Cpu.Load5"), &Value::F64(4.0));
    assert_eq!(at(&value, "Cpu.Load15"), &Value::F64(2.25));
    assert_eq!(at(&value, "Cpu.Processors"), &Value::I64(8));
    assert_eq!(at(&value, "Cpu.LoadPercent"), &Value::F64(50.0));
    assert_eq!(at(&value, "Cpu.ProcessPercent"), &Value::F64(12.5));
    assert_eq!(at(&value, "Cpu.Level").as_str(), Some("ok"));
    assert_eq!(at(&value, "Memory.ProcessBytes"), &Value::I64(512 * 1024 * 1024));
    assert_eq!(at(&value, "Memory.LimitSource").as_str(), Some("system"));
    assert_eq!(at(&value, "Memory.Level").as_str(), Some("ok"));
    assert_eq!(at(&value, "Memory.AvailablePercent"), &Value::F64(6.25));
    assert_eq!(at(&value, "Memory.AvailableLevel").as_str(), Some("warning"));
    assert_eq!(at(&value, "Swap.UsedPercent"), &Value::F64(25.0));
    assert_eq!(at(&value, "Swap.Level").as_str(), Some("ok"));
    assert_eq!(at(&value, "OpenFiles.Count"), &Value::I64(990));
    assert_eq!(at(&value, "OpenFiles.Limit"), &Value::I64(1024));
    assert_eq!(at(&value, "OpenFiles.Level").as_str(), Some("critical"));
    let Value::List(rows) = at(&value, "Filesystems") else {
        panic!("Filesystems is a list")
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(at(&rows[0], "mountPoint").as_str(), Some("/var"));
    assert_eq!(at(&rows[0], "usedPercent"), &Value::F64(91.0));
    assert_eq!(at(&rows[0], "availableBytes"), &Value::I64((9 * GIB) as i64));
    assert_eq!(at(&rows[0], "level").as_str(), Some("critical"));
    assert_eq!(at(&rows[0], "inodesUsedPercent"), &Value::F64(90.0));
    assert_eq!(at(&rows[0], "inodesLevel").as_str(), Some("critical"));
}

#[test]
fn a_reading_that_is_not_there_is_null_and_not_zero() {
    // a platform with no /proc: nothing is read
    let readings = Readings {
        processors: 4,
        filesystems: vec![Filesystem {
            inodes: None,
            ..filesystem(100 * GIB, 50 * GIB)
        }],
        ..parse_readings(None, None, None, None)
    };
    let value = readings.to_value(&Thresholds::default());
    for path in [
        "Cpu.Load1",
        "Cpu.Load5",
        "Cpu.Load15",
        "Cpu.LoadPercent",
        "Cpu.ProcessPercent",
        "Cpu.Level",
        "Memory.ProcessBytes",
        "Memory.ProcessPeakBytes",
        "Memory.TotalBytes",
        "Memory.AvailableBytes",
        "Memory.AvailablePercent",
        "Memory.LimitBytes",
        "Memory.LimitSource",
        "Memory.ProcessPercent",
        "Memory.Level",
        "Memory.AvailableLevel",
        "Swap.TotalBytes",
        "Swap.UsedBytes",
        "Swap.UsedPercent",
        "Swap.Level",
        "OpenFiles.Count",
        "OpenFiles.Limit",
        "OpenFiles.UsedPercent",
        "OpenFiles.Level",
    ] {
        assert!(at(&value, path).is_null(), "{path} is {:?}", at(&value, path));
    }
    assert_eq!(at(&value, "Cpu.Processors"), &Value::I64(4));
    // a filesystem that does not count inodes has no inode reading or level
    let Value::List(rows) = at(&value, "Filesystems") else {
        panic!("Filesystems is a list")
    };
    for name in ["inodesTotal", "inodesFree", "inodesUsedPercent", "inodesLevel"] {
        assert!(at(&rows[0], name).is_null(), "{name}");
    }
    assert_eq!(at(&rows[0], "level").as_str(), Some("ok"));
    // a machine with no swap has no share of it in use, and so no level
    let no_swap = parse_readings(
        None,
        None,
        Some("MemTotal: 1024 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n"),
        None,
    );
    let value = no_swap.to_value(&Thresholds::default());
    assert_eq!(at(&value, "Swap.TotalBytes"), &Value::I64(0));
    assert!(at(&value, "Swap.UsedPercent").is_null());
    assert!(at(&value, "Swap.Level").is_null());
}

#[cfg(unix)]
#[test]
fn a_filesystem_is_listed_once_when_two_directories_are_on_it() {
    let temp = std::env::temp_dir().join(format!("moqui-resources-test-{}", std::process::id()));
    let runtime = temp.join("runtime");
    let other = temp.join("other");
    std::fs::create_dir_all(runtime.join("log")).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let rows = filesystems(&[
        ("runtime".to_string(), runtime.clone()),
        // inside the runtime, on its filesystem: nothing new
        ("logs".to_string(), runtime.join("log")),
        // not there: left out
        ("database".to_string(), runtime.join("db")),
        // beside the runtime, on the same filesystem: named on the one row
        ("system temp".to_string(), other),
    ]);
    let canonical = runtime.canonicalize().unwrap();
    std::fs::remove_dir_all(&temp).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].holds, ["runtime", "system temp"]);
    assert!(rows[0].total > 0);
    assert!(rows[0].available <= rows[0].total);
    assert!(canonical.starts_with(&rows[0].mount_point), "{}", rows[0].mount_point);
}
