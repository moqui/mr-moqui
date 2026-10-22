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

//! `moqui test [component ...]`: run the JavaScript spec files in each component's `test` directory, Moqui Java's
//! Spock tests on the script engine (`plans/archive/TestHarness.md`).
//!
//! A component's specs run in the order of its `test/suite.js` (`suite(["SpecA", "SpecB"])`), then the other
//! `*.spec.js` files by name. Components run in the order they load. `--spec` picks specs by name and adds the specs
//! their `dependsOn` names, in suite order. Each spec runs in its own execution context on one database, as the Spock
//! specs of a suite share one.

#[cfg(feature = "llm")]
mod agents;
#[cfg(feature = "llm")]
mod skills;
mod coverage;
mod fixture;
mod report;

use std::path::{Path, PathBuf};
use std::time::Instant;

use moqui_core::testkit::{
    run_spec, spec_depends_on, spec_missing_features, suite_order, CaseResult, Outcome, RunOptions, SpecFile,
    SpecResult,
};
use moqui_lib::error::Error;

pub use fixture::{DbMode, TestFixture};

/// What `moqui test` was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct TestArgs {
    pub components: Vec<String>,
    pub specs: Vec<String>,
    pub feature: Option<String>,
    pub db: DbMode,
    pub junit: Option<PathBuf>,
    pub keep: bool,
    pub log_level: Option<String>,
    /// `--coverage`: report which services of the components the specs called.
    pub coverage: bool,
    /// `--coverage-report file`: write the full report here (with the called services) and print the total.
    pub coverage_report: Option<PathBuf>,
    /// `--agents`: run the agent evaluation tasks of `test/agent/*.agent.js` instead of the specs.
    pub agents: bool,
    /// `--llm-profile model`: the model the agents run on.
    pub llm_profile: Option<String>,
    /// `--replay dir`: the recorded model replies to play instead of a model.
    pub replay: Option<PathBuf>,
    /// `--runs N`: runs of each agent task.
    pub runs: u32,
    /// `--seed S`: the seed of the first run.
    pub seed: Option<u64>,
    /// `--min-pass F`: the pass@1 a live agent run needs.
    pub min_pass: Option<f64>,
    /// `--write-baseline`: write the component's agent baseline from this run.
    pub write_baseline: bool,
    /// `--skills`: run the skill retrieval sets of `test/skill-retrieval.json` instead of the specs.
    pub skills: bool,
}

pub fn parse_test_args(args: &[String]) -> Result<TestArgs, Error> {
    let mut parsed = TestArgs {
        components: Vec::new(),
        specs: Vec::new(),
        feature: None,
        db: DbMode::Fresh,
        junit: None,
        keep: false,
        log_level: None,
        coverage: false,
        coverage_report: None,
        agents: false,
        llm_profile: None,
        replay: None,
        runs: 1,
        seed: None,
        min_pass: None,
        write_baseline: false,
        skills: false,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| Error::new(format!("{name} needs a value\nRun 'moqui help test' for details.")))
        };
        match arg.as_str() {
            "--spec" => parsed.specs.extend(
                value("--spec")?
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            ),
            "--feature" => parsed.feature = Some(value("--feature")?),
            "--db" => parsed.db = DbMode::parse(&value("--db")?)?,
            "--junit" => parsed.junit = Some(PathBuf::from(value("--junit")?)),
            "--log-level" => parsed.log_level = Some(value("--log-level")?),
            "--keep" => parsed.keep = true,
            "--coverage" => parsed.coverage = true,
            "--coverage-report" => {
                parsed.coverage = true;
                parsed.coverage_report = Some(PathBuf::from(value("--coverage-report")?));
            }
            "--agents" => parsed.agents = true,
            "--skills" => parsed.skills = true,
            "--llm-profile" => parsed.llm_profile = Some(value("--llm-profile")?),
            "--replay" => parsed.replay = Some(PathBuf::from(value("--replay")?)),
            "--runs" => {
                parsed.runs = value("--runs")?
                    .parse()
                    .ok()
                    .filter(|runs| *runs > 0)
                    .ok_or_else(|| Error::new("--runs is a whole number of at least 1"))?;
            }
            "--seed" => {
                parsed.seed = Some(value("--seed")?.parse().map_err(|_| Error::new("--seed is a whole number"))?);
            }
            "--min-pass" => {
                parsed.min_pass = Some(
                    value("--min-pass")?
                        .parse()
                        .ok()
                        .filter(|share: &f64| (0.0..=1.0).contains(share))
                        .ok_or_else(|| Error::new("--min-pass is a share from 0 to 1, such as 0.75"))?,
                );
            }
            "--write-baseline" => parsed.write_baseline = true,
            other if other.starts_with("--") => {
                return Err(Error::new(format!(
                    "unknown option {other}\nRun 'moqui help test' for details."
                )));
            }
            name => parsed.components.push(name.to_string()),
        }
    }
    Ok(parsed)
}

/// A component with specs.
#[derive(Clone, Debug)]
pub(crate) struct TestComponent {
    pub(crate) name: String,
    pub(crate) dir: PathBuf,
    /// The specs to run, in order.
    pub(crate) specs: Vec<SpecFile>,
}

/// The components under `runtime/base-component` and `runtime/component` that have a `test` directory with specs.
fn discover(runtime: &Path) -> Result<Vec<TestComponent>, Error> {
    let mut found = Vec::new();
    for parent in ["base-component", "component"] {
        let Ok(entries) = std::fs::read_dir(runtime.join(parent)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|dir| dir.is_dir())
            .collect();
        dirs.sort();
        for dir in dirs {
            let test_dir = dir.join("test");
            if !test_dir.is_dir() {
                continue;
            }
            let name = component_name(&dir);
            let specs = component_specs(&name, &test_dir)?;
            if !specs.is_empty() {
                found.push(TestComponent { name, dir, specs });
            }
        }
    }
    Ok(found)
}

/// The `name` of `component.xml`, else the directory name.
pub(crate) fn component_name(dir: &Path) -> String {
    let dir_name = dir
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    moqui_lib::node::MNode::parse_file(dir.join("component.xml"))
        .ok()
        .and_then(|node| node.attribute("name"))
        .filter(|name| !name.is_empty())
        .unwrap_or(dir_name)
}

/// The specs of one component's test directory: the `suite.js` order first, then the rest by name.
fn component_specs(component: &str, test_dir: &Path) -> Result<Vec<SpecFile>, Error> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(test_dir)
        .map_err(|err| Error::new(format!("could not read {}", test_dir.display())).with_source(err))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with(".spec.js"))
        .collect();
    files.sort();
    let mut specs = Vec::new();
    for path in files {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = file_name.trim_end_matches(".spec.js").to_string();
        let source = std::fs::read_to_string(&path)
            .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
        specs.push(SpecFile {
            name,
            display: format!("{component}/test/{file_name}"),
            source,
        });
    }
    let suite = test_dir.join("suite.js");
    if suite.is_file() {
        let text = std::fs::read_to_string(&suite)
            .map_err(|err| Error::new(format!("could not read {}", suite.display())).with_source(err))?;
        let order = suite_order(&text).map_err(|err| Error::new(format!("{component}/test/suite.js: {err}")))?;
        let mut ordered = Vec::new();
        for name in &order {
            let index = specs.iter().position(|spec| &spec.name == name).ok_or_else(|| {
                Error::new(format!(
                    "{component}/test/suite.js names {name}, which has no {name}.spec.js"
                ))
            })?;
            ordered.push(specs.remove(index));
        }
        ordered.extend(specs);
        specs = ordered;
    }
    Ok(specs)
}

/// Keep the specs named and the specs they depend on, in the component's order.
fn select_specs(component: &TestComponent, names: &[String]) -> Result<Vec<SpecFile>, Error> {
    let mut wanted: Vec<String> = Vec::new();
    let mut pending: Vec<String> = names.to_vec();
    while let Some(name) = pending.pop() {
        if wanted.contains(&name) {
            continue;
        }
        let spec = component
            .specs
            .iter()
            .find(|spec| spec.name == name)
            .ok_or_else(|| Error::new(format!("{} has no spec {name}", component.name)))?;
        pending.extend(spec_depends_on(&spec.source));
        wanted.push(name);
    }
    Ok(component
        .specs
        .iter()
        .filter(|spec| wanted.contains(&spec.name))
        .cloned()
        .collect())
}

/// Run `moqui test`. An error when any feature or spec phase failed, so the exit status is 1.
pub fn dispatch_test(args: &[String]) -> Result<(), Error> {
    let args = parse_test_args(args)?;
    let console = match args.log_level.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("info") => tracing::Level::INFO,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::WARN,
    };
    crate::logging::install_with_console(Some(console));
    let (runtime, framework) = crate::runtime_framework_dirs();
    let agent_only = args.llm_profile.is_some()
        || args.replay.is_some()
        || args.seed.is_some()
        || args.min_pass.is_some()
        || args.write_baseline
        || args.runs != 1;
    if args.skills && !agent_only && !args.agents {
        #[cfg(feature = "llm")]
        return skills::dispatch(&args, &runtime, &framework);
        #[cfg(not(feature = "llm"))]
        return Err(Error::new("moqui test --skills: this build has no agents (the llm feature)"));
    }
    if args.agents {
        #[cfg(feature = "llm")]
        return agents::dispatch(&args, &runtime, &framework);
        #[cfg(not(feature = "llm"))]
        return Err(Error::new("moqui test --agents: this build has no agents (the llm feature)"));
    } else if agent_only {
        return Err(Error::new(
            "--llm-profile, --replay, --runs, --seed, --min-pass and --write-baseline go with --agents\nRun 'moqui help test' for details.",
        ));
    }
    let mut components = discover(&runtime)?;
    if !args.components.is_empty() {
        for name in &args.components {
            if !components.iter().any(|component| &component.name == name) {
                return Err(Error::new(format!("component {name} has no test/*.spec.js files")));
            }
        }
        components.retain(|component| args.components.contains(&component.name));
    }
    if !args.specs.is_empty() {
        let mut picked = Vec::new();
        for mut component in components {
            let names: Vec<String> = args
                .specs
                .iter()
                .filter(|name| component.specs.iter().any(|spec| &spec.name == *name))
                .cloned()
                .collect();
            if names.is_empty() {
                continue;
            }
            component.specs = select_specs(&component, &names)?;
            picked.push(component);
        }
        for name in &args.specs {
            if !picked
                .iter()
                .any(|component| component.specs.iter().any(|spec| &spec.name == name))
            {
                return Err(Error::new(format!("no component has a spec {name}")));
            }
        }
        components = picked;
    }
    if components.is_empty() {
        return Err(Error::new(format!(
            "no test/*.spec.js files under {}/component or {}/base-component",
            runtime.display(),
            runtime.display()
        )));
    }
    let started = Instant::now();
    let fixture = fixture::open(&runtime, &framework, args.db, args.keep)?;
    // A component directory the factory did not load (its component.xml is disabled) has nothing to test.
    for name in drop_unloaded(&mut components, &fixture) {
        if args.components.contains(&name) {
            return Err(Error::new(format!(
                "component {name} is not loaded; its component.xml may be disabled (see its README for how to turn it on)"
            )));
        }
        tracing::info!("Not running the specs of component {name}, it is not loaded");
    }
    if components.is_empty() {
        return Err(Error::new("none of the components with specs is loaded"));
    }
    order_by_load(&mut components, &fixture);
    let options = RunOptions {
        feature_filter: args.feature.clone(),
    };
    let mut results = Vec::new();
    if args.coverage {
        moqui_core::service::coverage::enable();
    }
    for component in &components {
        for spec in &component.specs {
            let missing = spec_missing_features(&spec.source);
            if !missing.is_empty() {
                let result = skipped_spec(spec, &missing);
                for case in &result.cases {
                    println!("{}", report::case_line(&spec.name, case));
                }
                results.push(report::ComponentSpec {
                    component: component.name.clone(),
                    result,
                });
                continue;
            }
            let context = fixture.factory.new_context();
            let entered = context.enter();
            let result = run_spec(spec, &options, &mut |case| {
                println!("{}", report::case_line(&spec.name, case));
            });
            drop(entered);
            drop(context);
            if let Some(failure) = &result.setup_failure {
                println!(
                    "{} setup FAILED: {}",
                    spec.name,
                    failure.message.lines().next().unwrap_or("")
                );
            }
            results.push(report::ComponentSpec {
                component: component.name.clone(),
                result,
            });
        }
    }
    if args.coverage {
        print_coverage(&components, &args, &moqui_core::service::coverage::finish())?;
    }
    let log_dir = fixture.log_dir.clone();
    drop(fixture);
    if let Some(dir) = &log_dir {
        println!("Log: {}", dir.join("moqui.log").display());
    }
    if let Some(dir) = &args.junit {
        report::write_junit(dir, &results)?;
    }
    let (text, failed) = report::summary(&results, started.elapsed());
    print!("{text}");
    if failed > 0 {
        return Err(Error::new(format!("{failed} test failure(s)")));
    }
    Ok(())
}

/// A spec that `requires` features this build leaves out: one skipped case that names them.
fn skipped_spec(spec: &SpecFile, missing: &[String]) -> SpecResult {
    SpecResult {
        name: spec.name.clone(),
        display: spec.display.clone(),
        cases: vec![CaseResult {
            name: "(every feature)".to_string(),
            outcome: Outcome::Skipped(format!(
                "requires {}, not built into this binary (moqui version)",
                missing.join(", ")
            )),
            duration: std::time::Duration::ZERO,
        }],
        setup_failure: None,
        cleanup_failure: None,
        duration: std::time::Duration::ZERO,
    }
}

/// Leave out the components the factory did not load, and return their names.
pub(crate) fn drop_unloaded(components: &mut Vec<TestComponent>, fixture: &TestFixture) -> Vec<String> {
    let loaded = loaded_locations(fixture);
    let mut dropped = Vec::new();
    components.retain(|component| {
        let dir = std::fs::canonicalize(&component.dir).unwrap_or_else(|_| component.dir.clone());
        let keep = loaded.contains(&dir);
        if !keep {
            dropped.push(component.name.clone());
        }
        keep
    });
    dropped
}

fn loaded_locations(fixture: &TestFixture) -> Vec<PathBuf> {
    fixture
        .factory
        .components()
        .iter()
        .map(|info| std::fs::canonicalize(&info.location).unwrap_or_else(|_| info.location.clone()))
        .collect()
}

/// The service coverage of each component that ran, on the console, and in full in the `--coverage-report` file.
fn print_coverage(
    components: &[TestComponent],
    args: &TestArgs,
    counts: &std::collections::BTreeMap<String, u64>,
) -> Result<(), Error> {
    let mut full = String::new();
    for component in components {
        let files = coverage::coverage(&component.dir, counts);
        if files.is_empty() {
            continue;
        }
        let text = coverage::render(&component.name, &files, true);
        full.push_str(&text);
        if args.coverage_report.is_none() {
            print!("{}", coverage::render(&component.name, &files, false));
        } else if let Some(total) = text.lines().last() {
            println!("Service coverage {}: {total}", component.name);
        }
    }
    if let Some(path) = &args.coverage_report {
        std::fs::write(path, full)
            .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))?;
        println!("Service coverage report: {}", path.display());
    }
    Ok(())
}

/// Components in the order the factory loads them, which follows `depends-on`.
pub(crate) fn order_by_load(components: &mut [TestComponent], fixture: &TestFixture) {
    let loaded = loaded_locations(fixture);
    components.sort_by_key(|component| {
        let dir = std::fs::canonicalize(&component.dir).unwrap_or_else(|_| component.dir.clone());
        loaded
            .iter()
            .position(|location| *location == dir)
            .unwrap_or(usize::MAX)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, source: &str) -> SpecFile {
        SpecFile {
            name: name.to_string(),
            display: format!("c/test/{name}.spec.js"),
            source: source.to_string(),
        }
    }

    #[test]
    fn args_parse() {
        let args: Vec<String> = [
            "mantle-usl",
            "--spec",
            "A,B",
            "--db",
            "snapshot",
            "--junit",
            "out",
            "--keep",
        ]
        .iter()
        .map(|arg| (*arg).to_string())
        .collect();
        let parsed = parse_test_args(&args).unwrap();
        assert_eq!(parsed.components, vec!["mantle-usl".to_string()]);
        assert_eq!(parsed.specs, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(parsed.db, DbMode::Snapshot);
        assert_eq!(parsed.junit, Some(PathBuf::from("out")));
        assert!(parsed.keep);
        assert!(parse_test_args(&["--db".to_string(), "nope".to_string()]).is_err());
        assert!(parse_test_args(&["--bogus".to_string()]).is_err());
    }

    #[test]
    fn agent_args_parse() {
        let args: Vec<String> = [
            "tools",
            "--agents",
            "--llm-profile",
            "local-qwen",
            "--runs",
            "3",
            "--seed",
            "7",
            "--min-pass",
            "0.5",
            "--write-baseline",
        ]
        .iter()
        .map(|arg| (*arg).to_string())
        .collect();
        let parsed = parse_test_args(&args).unwrap();
        assert!(parsed.agents && parsed.write_baseline);
        assert_eq!(parsed.llm_profile.as_deref(), Some("local-qwen"));
        assert_eq!((parsed.runs, parsed.seed, parsed.min_pass), (3, Some(7), Some(0.5)));
        let replay = parse_test_args(&["--agents".to_string(), "--replay".to_string(), "dir".to_string()]).unwrap();
        assert_eq!(replay.replay, Some(PathBuf::from("dir")));
        assert_eq!(replay.runs, 1);
        for bad in [["--runs", "0"], ["--seed", "x"], ["--min-pass", "2"]] {
            let bad: Vec<String> = bad.iter().map(|arg| (*arg).to_string()).collect();
            assert!(parse_test_args(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn spec_selection_adds_depends_on_in_suite_order() {
        let component = TestComponent {
            name: "c".to_string(),
            dir: PathBuf::from("c"),
            specs: vec![
                spec("First", "spec({})"),
                spec("Second", "spec({ dependsOn: ['First'] })"),
                spec("Other", "spec({})"),
                spec("Third", "spec({ dependsOn: ['Second'] })"),
            ],
        };
        let picked = select_specs(&component, &["Third".to_string()]).unwrap();
        let names: Vec<&str> = picked.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, vec!["First", "Second", "Third"]);
        assert!(select_specs(&component, &["Missing".to_string()]).is_err());
    }

    #[test]
    fn suite_order_comes_first() {
        let dir = std::env::temp_dir().join(format!("moqui-test-discover-{}", std::process::id()));
        let test_dir = dir.join("test");
        std::fs::create_dir_all(&test_dir).unwrap();
        for name in ["A", "B", "C"] {
            std::fs::write(test_dir.join(format!("{name}.spec.js")), "spec({})").unwrap();
        }
        std::fs::write(test_dir.join("suite.js"), "suite(['C', 'A'])").unwrap();
        let specs = component_specs("c", &test_dir).unwrap();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, vec!["C", "A", "B"]);
        assert_eq!(specs[0].display, "c/test/C.spec.js");
        std::fs::write(test_dir.join("suite.js"), "suite(['Nope'])").unwrap();
        assert!(component_specs("c", &test_dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
