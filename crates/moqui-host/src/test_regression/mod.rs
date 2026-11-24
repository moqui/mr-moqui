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

//! `moqui test-regression <baseline> <candidate> [component | spec ...]` (plans/Regression-Test.md): run the same
//! regression specs (`test/regression/*.spec.js`) on two engines and compare what they did.
//!
//! Each target is an engine: `rust:.` (this binary), `rust:<git ref>` (built from that commit), or `java:<ref>` (Moqui
//! Java from `--java-repo`). The targets run one after the other, each on a new database of one embedded Postgres
//! cluster, with the test data clock (`MOQUI_TEST_DATA_CLOCK_FILE`) at a fixed time. A target's run is a recording
//! ([`record::Recording`]): the database after boot, and for each case the service answers and the row changes. The
//! two recordings are then compared ([`compare`]), with the volatile columns (`test/regression/volatile.json`) left
//! out, and for a Java target the approved differences (`test/regression/approved-differences.json`) too.

mod compare;
mod db;
mod engine;
mod host;
mod record;
mod target;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_devkit::testkit::{run_spec, RunOptions, SpecHost};
use moqui_lib::error::Error;

use crate::test_cmd::{component_dir_named, component_specs, discover, TestComponent, REGRESSION_TREE};

/// The data clock's time at boot: 2026-01-01T00:00:00Z. Each case moves it on by [`CLOCK_STEP_MS`].
pub(crate) const CLOCK_EPOCH_MS: i64 = 1_767_225_600_000;
pub(crate) const CLOCK_STEP_MS: i64 = 1_000;

/// What `moqui test-regression` was asked to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RegressionArgs {
    pub baseline: String,
    pub candidate: String,
    /// Component names or `.spec.js` files; none is every component with `test/regression`.
    pub targets: Vec<String>,
    /// `--spec A,B`: only these specs.
    pub specs: Vec<String>,
    /// `--feature text`: only the cases whose name contains it.
    pub feature: Option<String>,
    pub out: Option<PathBuf>,
    /// `--java-repo`, else `MOQUI_JAVA_REPO`: the Moqui Java checkout (moqui-framework with its runtime).
    pub java_repo: Option<PathBuf>,
    /// `--keep`: leave the database cluster and the engines' runtimes in the output directory.
    pub keep: bool,
    /// `--start-timeout seconds`: how long an engine may take to boot.
    pub start_timeout: f64,
    pub log_level: Option<String>,
}

pub(crate) fn parse_args(args: &[String]) -> Result<RegressionArgs, Error> {
    let mut parsed = RegressionArgs {
        start_timeout: 900.0,
        ..RegressionArgs::default()
    };
    let mut positional = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| Error::new(format!("moqui test-regression: {name} needs a value")))
        };
        match arg.as_str() {
            "--spec" => parsed.specs.extend(
                value("--spec")?
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| name.trim_end_matches(".spec.js").to_string()),
            ),
            "--feature" => parsed.feature = Some(value("--feature")?),
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
            "--java-repo" => parsed.java_repo = Some(PathBuf::from(value("--java-repo")?)),
            "--keep" => parsed.keep = true,
            "--start-timeout" => {
                let text = value("--start-timeout")?;
                parsed.start_timeout = text
                    .parse::<f64>()
                    .ok()
                    .filter(|seconds| *seconds > 0.0)
                    .ok_or_else(|| Error::new(format!("--start-timeout {text}: give seconds")))?;
            }
            "--log-level" => parsed.log_level = Some(value("--log-level")?),
            other if other.starts_with("--") => {
                return Err(Error::new(format!(
                    "moqui test-regression: unknown option {other} (moqui help test-regression)"
                )))
            }
            other => positional.push(other.to_string()),
        }
    }
    let mut positional = positional.into_iter();
    let (Some(baseline), Some(candidate)) = (positional.next(), positional.next()) else {
        return Err(Error::new(
            "moqui test-regression <baseline> <candidate> [component | spec ...]: give two targets, such as rust:master rust:.",
        ));
    };
    parsed.baseline = baseline;
    parsed.candidate = candidate;
    parsed.targets = positional.collect();
    if parsed.java_repo.is_none() {
        parsed.java_repo = std::env::var_os("MOQUI_JAVA_REPO")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from);
    }
    Ok(parsed)
}

/// `moqui test-regression ...`: the exit code is 0 when the two targets match, 1 when they differ or a spec failed.
pub fn dispatch(args: &[String]) -> Result<i32, Error> {
    let parsed = parse_args(args)?;
    let console = match parsed.log_level.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("warn") => tracing::Level::WARN,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    };
    crate::logging::install_with_console(Some(console));
    moqui_core::seal::set_test_run();
    run(&parsed, &mut |line| println!("{line}"))
}

/// Run both targets and compare them, each line of the report to `print`.
fn run(args: &RegressionArgs, print: &mut dyn FnMut(&str)) -> Result<i32, Error> {
    let (runtime, _) = crate::runtime_framework_dirs();
    let components = select(&runtime, args)?;
    let out = args.out.clone().unwrap_or_else(|| default_out_dir(&runtime));
    std::fs::create_dir_all(&out).map_err(|err| Error::new(format!("could not create {}", out.display())).with_source(err))?;
    let out = std::fs::canonicalize(&out).map_err(|err| Error::new("could not resolve the output directory").with_source(err))?;
    let baseline = target::Target::parse(&args.baseline, args.java_repo.as_deref())?;
    let candidate = target::Target::parse(&args.candidate, args.java_repo.as_deref())?;
    let rules = compare::Rules::load(&components, baseline.is_java() || candidate.is_java())?;
    print(&format!("test-regression: {} → {}; output {}", baseline.name(), candidate.name(), out.display()));
    // both builds first, so a failed build stops the run before anything boots
    let prepared = [
        ("baseline", baseline.prepare(&runtime, &out, print)?),
        ("candidate", candidate.prepare(&runtime, &out, print)?),
    ];
    let cluster = db::Cluster::start(&runtime, &out.join("db"))?;
    let mut recordings = Vec::new();
    for (label, engine) in &prepared {
        recordings.push(run_target(args, label, engine, &cluster, &components, &out, print)?);
    }
    drop(cluster);
    let report = compare::compare(&recordings[0], &recordings[1], &rules);
    let report_file = out.join("report.json");
    write_json(&report_file, &report.to_json())?;
    for line in report.lines() {
        print(&line);
    }
    print(&format!("report: {}", report_file.display()));
    if !args.keep {
        for (label, _) in &prepared {
            let _ = std::fs::remove_dir_all(out.join(label).join("runtime"));
        }
        let _ = std::fs::remove_dir_all(out.join("db"));
    }
    Ok(i32::from(!report.passed()))
}

/// Boot one target on its own database, run the specs on it and stop it: its recording, also in
/// `<out>/<label>/recording.json`.
fn run_target(
    args: &RegressionArgs,
    label: &str,
    prepared: &target::Prepared,
    cluster: &db::Cluster,
    components: &[TestComponent],
    out: &Path,
    print: &mut dyn FnMut(&str),
) -> Result<record::Recording, Error> {
    let dir = out.join(label);
    let database = format!("regression_{label}");
    let driver = cluster.database(&database)?;
    let clock = engine::Clock::new(&dir.join("clock"), CLOCK_EPOCH_MS)?;
    let started = std::time::Instant::now();
    let mut running = engine::Engine::start(prepared, &dir, cluster.connection(&database), &clock, args.start_timeout)?;
    print(&format!("{label}: {} boots at {} ({:.0}s)", prepared.name, running.url(), started.elapsed().as_secs_f64()));
    let recorder = record::Recorder::new(driver, label, &prepared.name)?;
    let client = crate::test_client::regression_client(&running.url(), &dir.join("client"))?;
    let host = Arc::new(host::RegressionHost::new(client.host(), recorder, clock));
    let host_dyn: Arc<dyn SpecHost> = host.clone();
    let options = RunOptions {
        feature_filter: args.feature.clone(),
        host: Some(host_dyn),
    };
    for component in components {
        for spec in &component.specs {
            client.begin_spec(component, spec);
            host.begin_spec(&spec.name);
            let result = run_spec(spec, &options, &mut |case| {
                host.case_finished(case);
                print(&format!("{label}: {}", crate::test_cmd::report::case_line(&spec.name, case)));
            });
            if let Some(failure) = &result.setup_failure {
                host.spec_failed(&spec.name, &failure.message);
                print(&format!("{label}: {} setup FAILED: {}", spec.name, failure.message.lines().next().unwrap_or("")));
            }
        }
    }
    let stop = running.stop();
    let mut recording = host.finish();
    if let Err(err) = stop {
        recording.problems.push(format!("the engine did not stop cleanly: {err}"));
    }
    write_json(&dir.join("recording.json"), &recording.to_json())?;
    Ok(recording)
}

/// The components and their regression specs: those named, or every component with `test/regression`; then `--spec`.
fn select(runtime: &Path, args: &RegressionArgs) -> Result<Vec<TestComponent>, Error> {
    let mut components: Vec<TestComponent> = Vec::new();
    if args.targets.is_empty() {
        components = discover(runtime, REGRESSION_TREE)?;
    }
    for target in &args.targets {
        let path = Path::new(target);
        if target.ends_with(".spec.js") && path.is_file() {
            let test_dir = path.parent().unwrap_or(Path::new("."));
            let name = target_component_name(test_dir);
            let wanted = path.file_name().map(|name| name.to_string_lossy().trim_end_matches(".spec.js").to_string());
            let specs = component_specs(&name, test_dir, REGRESSION_TREE)?
                .into_iter()
                .filter(|spec| Some(&spec.name) == wanted.as_ref())
                .collect();
            components.push(TestComponent {
                name,
                dir: test_dir.to_path_buf(),
                specs,
            });
            continue;
        }
        let dir = component_dir_named(runtime, target)
            .ok_or_else(|| Error::new(format!("moqui test-regression: no component {target} in {}", runtime.display())))?;
        let test_dir = dir.join(REGRESSION_TREE);
        let specs = if test_dir.is_dir() {
            component_specs(target, &test_dir, REGRESSION_TREE)?
        } else {
            Vec::new()
        };
        components.push(TestComponent {
            name: target.clone(),
            dir,
            specs,
        });
    }
    if !args.specs.is_empty() {
        for component in &mut components {
            component.specs.retain(|spec| args.specs.contains(&spec.name));
        }
    }
    components.retain(|component| !component.specs.is_empty());
    if components.is_empty() {
        return Err(Error::new(format!(
            "moqui test-regression: no regression specs selected ({REGRESSION_TREE}/*.spec.js in a component)"
        )));
    }
    Ok(components)
}

/// The component of a spec file's directory, `<component>/test/regression`.
fn target_component_name(test_dir: &Path) -> String {
    test_dir
        .ancestors()
        .nth(2)
        .and_then(Path::file_name)
        .map_or_else(|| "spec".to_string(), |name| name.to_string_lossy().to_string())
}

/// `runtime/tmp/test-regression/<yyyyMMdd-HHmmss>-<pid>`.
fn default_out_dir(runtime: &Path) -> PathBuf {
    let run = format!("{}-{}", jiff::Zoned::now().strftime("%Y%m%d-%H%M%S"), std::process::id());
    runtime.join("tmp/test-regression").join(run)
}

pub(crate) fn write_json(path: &Path, value: &serde_json::Value) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|err| Error::new("could not write JSON").with_source(err))?;
    std::fs::write(path, text + "\n").map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))
}
