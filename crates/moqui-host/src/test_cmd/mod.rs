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
//! their `dependsOn` names, in suite order. Each spec runs in its own execution context. A `dependsOn` name a spec's
//! component lacks comes from the run's other components. Each component's `test/MoquiConf.xml` merges after its
//! `MoquiConf.xml` in a test run, and `--serve` then serves the scratch database (`serve.rs`, plans/archive/ClientTestHarness.md 6).
//!
//! Discovery, argument parsing, factory boot and `run_spec` are in every build. `--db none` boots the factory and
//! leaves the entity facade unset. `fixture::open`, `--serve`, coverage and `--agents` need `embedded-postgres`.
//! The discovery, the spec selection and the report also serve `moqui test-client` ([`crate::test_client`]), over
//! `test/client`.

#[cfg(all(feature = "llm", feature = "embedded-postgres"))]
mod agents;
#[cfg(feature = "embedded-postgres")]
mod coverage;
#[cfg(feature = "embedded-postgres")]
mod fixture;
pub(crate) mod report;
#[cfg(feature = "embedded-postgres")]
mod serve;
#[cfg(feature = "llm")]
mod skills;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::testkit::{
    CaseResult, Outcome, RunOptions, SpecFile, SpecResult, run_spec, spec_database_free, spec_depends_on,
    spec_missing_features, suite_order,
};
use moqui_lib::error::Error;

#[cfg(feature = "embedded-postgres")]
pub use fixture::TestFixture;

/// How `moqui test` gets a database. `None` boots the factory and skips Postgres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbMode {
    None,
    Fresh,
    Snapshot,
    Existing,
}

impl DbMode {
    pub fn parse(text: &str) -> Result<Self, Error> {
        match text {
            "none" => Ok(Self::None),
            "fresh" => Ok(Self::Fresh),
            "snapshot" => Ok(Self::Snapshot),
            "existing" => Ok(Self::Existing),
            other => Err(Error::new(format!(
                "--db is none, fresh, snapshot or existing, not {other}"
            ))),
        }
    }

    /// Used by a build without embedded-postgres, when `--db fresh` (and the other database modes) is refused.
    #[cfg_attr(feature = "embedded-postgres", allow(dead_code))]
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fresh => "fresh",
            Self::Snapshot => "snapshot",
            Self::Existing => "existing",
        }
    }
}

/// What `moqui test` was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct TestArgs {
    pub components: Vec<String>,
    pub specs: Vec<String>,
    pub feature: Option<String>,
    pub db: DbMode,
    /// The user passed `--db`. When they did not, a run whose every spec sets `database: false` uses [`DbMode::None`].
    pub db_explicit: bool,
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
    /// `--serve [address]`: after the specs pass, serve the scratch database on this address until stopped.
    pub serve: Option<std::net::SocketAddr>,
    /// `--ready-file path`: with `--serve`, the server's URL is written here once it listens.
    pub ready_file: Option<PathBuf>,
}

pub fn parse_test_args(args: &[String]) -> Result<TestArgs, Error> {
    let mut parsed = TestArgs {
        components: Vec::new(),
        specs: Vec::new(),
        feature: None,
        db: DbMode::Fresh,
        db_explicit: false,
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
        serve: None,
        ready_file: None,
    };
    let mut iter = args.iter().peekable();
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
            "--db" => {
                parsed.db = DbMode::parse(&value("--db")?)?;
                parsed.db_explicit = true;
            }
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
                parsed.seed = Some(
                    value("--seed")?
                        .parse()
                        .map_err(|_| Error::new("--seed is a whole number"))?,
                );
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
            "--ready-file" => parsed.ready_file = Some(PathBuf::from(value("--ready-file")?)),
            "--serve" => {
                // The address is optional: the next argument when it is one, so a component name after --serve stays
                // a component.
                let address = iter.peek().and_then(|next| serve_address(next));
                if address.is_some() {
                    iter.next();
                }
                parsed.serve = Some(address.unwrap_or_else(|| std::net::SocketAddr::from(([127, 0, 0, 1], 0))));
            }
            other if other.starts_with("--") => {
                return Err(Error::new(format!(
                    "unknown option {other}\nRun 'moqui help test' for details."
                )));
            }
            name => parsed.components.push(name.to_string()),
        }
    }
    if parsed.serve.is_some() {
        if parsed.db == DbMode::None {
            return Err(Error::new("--db none does not go with --serve"));
        }
        if parsed.db == DbMode::Existing {
            return Err(Error::new(
                "--serve runs on a scratch runtime: --db fresh or snapshot (a server on the existing database is ./moq run)",
            ));
        }
        if parsed.agents || parsed.skills {
            return Err(Error::new(
                "--serve serves after the specs; it does not go with --agents or --skills",
            ));
        }
    } else if parsed.ready_file.is_some() {
        return Err(Error::new(
            "--ready-file goes with --serve\nRun 'moqui help test' for details.",
        ));
    }
    Ok(parsed)
}

/// The address of `--serve`: `host:port` with an IP address, or a port alone on the loopback address.
fn serve_address(text: &str) -> Option<std::net::SocketAddr> {
    text.parse().ok().or_else(|| {
        text.parse::<u16>()
            .ok()
            .map(|port| std::net::SocketAddr::from(([127, 0, 0, 1], port)))
    })
}

/// A component with specs.
#[derive(Clone, Debug)]
pub(crate) struct TestComponent {
    pub(crate) name: String,
    pub(crate) dir: PathBuf,
    /// The specs to run, in order.
    pub(crate) specs: Vec<SpecFile>,
}

/// The components under `runtime/base-component`, `runtime/component` and `runtime/dev-component` (which load on a dev
/// instance) that have specs in `sub`: `test` for `moqui test`, `test/client` for `moqui test-client`. By directory
/// name within each.
pub(crate) fn discover(runtime: &Path, sub: &str) -> Result<Vec<TestComponent>, Error> {
    let mut found = Vec::new();
    for parent in ["base-component", "component", "dev-component"] {
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
            let test_dir = dir.join(sub);
            if !test_dir.is_dir() {
                continue;
            }
            let name = component_name(&dir);
            let specs = component_specs(&name, &test_dir, sub)?;
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

/// The specs of one component's test directory (`sub` under the component, `test` or `test/client`): the `suite.js`
/// order first, then the rest by name.
pub(crate) fn component_specs(component: &str, test_dir: &Path, sub: &str) -> Result<Vec<SpecFile>, Error> {
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
            display: format!("{component}/{sub}/{file_name}"),
            source,
        });
    }
    let suite = test_dir.join("suite.js");
    if suite.is_file() {
        let text = std::fs::read_to_string(&suite)
            .map_err(|err| Error::new(format!("could not read {}", suite.display())).with_source(err))?;
        let order = suite_order(&text).map_err(|err| Error::new(format!("{component}/{sub}/suite.js: {err}")))?;
        let mut ordered = Vec::new();
        for name in &order {
            let index = specs.iter().position(|spec| &spec.name == name).ok_or_else(|| {
                Error::new(format!(
                    "{component}/{sub}/suite.js names {name}, which has no {name}.spec.js"
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
pub(crate) fn select_specs(component: &TestComponent, names: &[String]) -> Result<Vec<SpecFile>, Error> {
    if let Some(name) = names
        .iter()
        .find(|name| !component.specs.iter().any(|spec| &spec.name == *name))
    {
        return Err(Error::new(format!("{} has no spec {name}", component.name)));
    }
    Ok(select_across(std::slice::from_ref(component), names)?
        .into_iter()
        .next()
        .map(|component| component.specs)
        .unwrap_or_default())
}

/// The specs named, in every component of the run that has them, and the specs they depend on, each component with
/// its specs in its own order; components left with none are dropped. A `dependsOn` name the spec's own component
/// lacks is looked up in the run's other components, in order (plans/archive/ClientTestHarness.md 6): mr-marble's specs
/// depend on mr-mantle's.
pub(crate) fn select_across(components: &[TestComponent], names: &[String]) -> Result<Vec<TestComponent>, Error> {
    let mut wanted: Vec<(usize, String)> = Vec::new();
    let mut pending: Vec<(usize, String)> = Vec::new();
    for name in names.iter().rev() {
        for (index, component) in components.iter().enumerate() {
            if component.specs.iter().any(|spec| &spec.name == name) {
                pending.push((index, name.clone()));
            }
        }
    }
    while let Some((index, name)) = pending.pop() {
        if wanted.contains(&(index, name.clone())) {
            continue;
        }
        let Some(spec) = components[index].specs.iter().find(|spec| spec.name == name) else {
            continue;
        };
        for depends in spec_depends_on(&spec.source) {
            let owner = std::iter::once(index)
                .chain((0..components.len()).filter(|other| *other != index))
                .find(|at| components[*at].specs.iter().any(|spec| spec.name == depends))
                .ok_or_else(|| {
                    Error::new(format!(
                        "{}/{name} depends on {depends}, which is not a spec of {} or of the other components of the run",
                        components[index].name, components[index].name
                    ))
                })?;
            pending.push((owner, depends));
        }
        wanted.push((index, name));
    }
    Ok(components
        .iter()
        .enumerate()
        .filter_map(|(index, component)| {
            let specs: Vec<SpecFile> = component
                .specs
                .iter()
                .filter(|spec| wanted.contains(&(index, spec.name.clone())))
                .cloned()
                .collect();
            (!specs.is_empty()).then(|| TestComponent {
                specs,
                ..component.clone()
            })
        })
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
        return Err(Error::new(
            "moqui test --skills: this build has no agents (the llm feature)",
        ));
    }
    if args.agents {
        if args.db == DbMode::None {
            return Err(Error::new("--agents needs a database; it does not go with --db none"));
        }
        #[cfg(all(feature = "llm", feature = "embedded-postgres"))]
        return agents::dispatch(&args, &runtime, &framework);
        #[cfg(not(all(feature = "llm", feature = "embedded-postgres")))]
        return Err(agents_unavailable());
    } else if agent_only {
        return Err(Error::new(
            "--llm-profile, --replay, --runs, --seed, --min-pass and --write-baseline go with --agents\nRun 'moqui help test' for details.",
        ));
    }
    if args.serve.is_some() && !cfg!(feature = "embedded-postgres") {
        return Err(needs_embedded("--serve"));
    }
    if args.coverage && !cfg!(feature = "embedded-postgres") {
        return Err(needs_embedded("--coverage"));
    }
    // Before the factory boots, so the ports a user already set stay as they are.
    #[cfg(feature = "embedded-postgres")]
    let serve_setup = args.serve.is_some().then(serve::Setup::before_boot);
    #[cfg(all(feature = "axum", any(feature = "postgres", feature = "mysql")))]
    crate::TEST_CONFS.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut components = discover(&runtime, "test")?;
    // With --serve a named component may have no specs: it is there for the server, and it must load.
    let mut serve_only: Vec<String> = Vec::new();
    if !args.components.is_empty() {
        for name in &args.components {
            if !components.iter().any(|component| &component.name == name) {
                if args.serve.is_none() {
                    return Err(Error::new(format!("component {name} has no test/*.spec.js files")));
                }
                serve_only.push(name.clone());
            }
        }
        components.retain(|component| args.components.contains(&component.name));
    }
    if !args.specs.is_empty() {
        let picked = select_across(&components, &args.specs)?;
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
    if components.is_empty() && args.serve.is_none() {
        return Err(Error::new(format!(
            "no test/*.spec.js files under {}/component, {}/base-component or {}/dev-component",
            runtime.display(),
            runtime.display(),
            runtime.display()
        )));
    }
    // A dev component's specs run on a dev instance, where it loads (plans/design-dev-aide.md 3.1).
    if components
        .iter()
        .any(|component| component.dir.starts_with(runtime.join("dev-component")))
        && moqui_lib::env::get_prop_or_env("instance_purpose").is_empty()
    {
        moqui_lib::env::set_property("instance_purpose", "dev");
    }
    let mode = database_mode(&args, &components)?;
    let started = Instant::now();
    if mode == DbMode::None {
        return run_without_database(&args, &runtime, &framework, &mut components, started);
    }
    #[cfg(feature = "embedded-postgres")]
    {
        run_with_database(
            &args,
            &runtime,
            &framework,
            &mut components,
            &serve_only,
            started,
            serve_setup,
        )
    }
    #[cfg(not(feature = "embedded-postgres"))]
    {
        let _ = (serve_only, started);
        Err(needs_embedded(&format!("moqui test --db {}", mode.as_str())))
    }
}

#[cfg_attr(all(feature = "llm", feature = "embedded-postgres"), allow(dead_code))]
fn agents_unavailable() -> Error {
    if cfg!(feature = "llm") {
        needs_embedded("moqui test --agents")
    } else {
        Error::new("moqui test --agents: this build has no agents (the llm feature)")
    }
}

fn needs_embedded(what: &str) -> Error {
    Error::new(format!(
        "{what} runs on a scratch embedded Postgres database; this build has no embedded-postgres"
    ))
}

/// `None` when the user asked for it, or when every selected spec sets `database: false` and nothing else needs a
/// database. An explicit `--db none` with a spec that does not opt out fails here, before boot, naming the spec.
fn database_mode(args: &TestArgs, components: &[TestComponent]) -> Result<DbMode, Error> {
    let all_free = !components.is_empty()
        && components
            .iter()
            .all(|component| component.specs.iter().all(|spec| spec_database_free(&spec.source)));
    let mode = if !args.db_explicit && all_free && args.serve.is_none() && !args.coverage {
        DbMode::None
    } else {
        args.db
    };
    if mode == DbMode::None {
        if let Some(name) = components.iter().find_map(|component| {
            component.specs.iter().find_map(|spec| {
                (!spec_database_free(&spec.source)).then(|| format!("{}/{}", component.name, spec.name))
            })
        }) {
            return Err(Error::new(format!(
                "--db none cannot run {name}, which does not set database: false"
            )));
        }
        if args.coverage {
            return Err(Error::new(
                "--coverage needs a database (--db fresh, snapshot, or existing)",
            ));
        }
    }
    Ok(mode)
}

/// Boot the factory with test confs and no entity facade. `ec.resource` is live; nothing opens Postgres.
fn boot_test_factory(runtime: &Path, framework: &Path) -> Result<Arc<ExecutionContextFactory>, Error> {
    let options = moqui_core::context::BootOptions {
        test_confs: true,
        ..moqui_core::context::BootOptions::default()
    };
    ExecutionContextFactory::boot_with_options(runtime, framework, options).map_err(|err| Error::new(err.to_string()))
}

/// Shuts the factory down when the run ends. There is no database to close.
struct FactoryGuard {
    factory: Arc<ExecutionContextFactory>,
}

impl Drop for FactoryGuard {
    fn drop(&mut self) {
        crate::wait_for_workers(&self.factory, std::time::Duration::from_secs(30));
        self.factory.shutdown();
    }
}

fn run_without_database(
    args: &TestArgs,
    runtime: &Path,
    framework: &Path,
    components: &mut Vec<TestComponent>,
    started: Instant,
) -> Result<(), Error> {
    let log_dir = crate::logging::attach_files(runtime, None);
    let factory = match boot_test_factory(runtime, framework) {
        Ok(factory) => factory,
        Err(err) => {
            return Err(err);
        }
    };
    let _guard = FactoryGuard {
        factory: Arc::clone(&factory),
    };
    for name in drop_unloaded(components, &factory) {
        if args.components.contains(&name) {
            return Err(not_loaded(&name));
        }
        tracing::info!("Not running the specs of component {name}, it is not loaded");
    }
    if components.is_empty() {
        return Err(Error::new("none of the components with specs is loaded"));
    }
    order_by_load(components, &factory);
    let mut results = Vec::new();
    run_component_specs(&factory, components, args.feature.as_deref(), &mut results);
    finish_report(args, log_dir, &results, started, false)
}

#[cfg(feature = "embedded-postgres")]
fn run_with_database(
    args: &TestArgs,
    runtime: &Path,
    framework: &Path,
    components: &mut Vec<TestComponent>,
    serve_only: &[String],
    started: Instant,
    serve_setup: Option<serve::Setup>,
) -> Result<(), Error> {
    let fixture = fixture::open(runtime, framework, args.db, args.keep)?;
    for name in serve_only {
        if !fixture.factory.components().iter().any(|info| &info.name == name) {
            return Err(not_loaded(name));
        }
    }
    for name in drop_unloaded(components, &fixture.factory) {
        if args.components.contains(&name) {
            return Err(not_loaded(&name));
        }
        tracing::info!("Not running the specs of component {name}, it is not loaded");
    }
    if components.is_empty() && args.serve.is_none() {
        return Err(Error::new("none of the components with specs is loaded"));
    }
    order_by_load(components, &fixture.factory);
    let mut results = Vec::new();
    if args.coverage {
        moqui_core::service::coverage::enable();
    }
    run_component_specs(&fixture.factory, components, args.feature.as_deref(), &mut results);
    if args.coverage {
        print_coverage(components, args, &moqui_core::service::coverage::finish())?;
    }
    let log_dir = fixture.log_dir.clone();
    let serving = args.serve.is_some();
    let fixture = serving.then_some(fixture);
    finish_report(args, log_dir, &results, started, serving)?;
    if let (Some(fixture), Some(address), Some(setup)) = (fixture, args.serve, serve_setup) {
        return serve::serve(fixture, framework, address, args.ready_file.as_deref(), &setup);
    }
    Ok(())
}

fn not_loaded(name: &str) -> Error {
    Error::new(format!(
        "component {name} is not loaded; its component.xml may be disabled (see its README for how to turn it on)"
    ))
}

fn run_component_specs(
    factory: &Arc<ExecutionContextFactory>,
    components: &[TestComponent],
    feature: Option<&str>,
    results: &mut Vec<report::ComponentSpec>,
) {
    let options = RunOptions {
        feature_filter: feature.map(str::to_string),
        ..RunOptions::default()
    };
    for component in components {
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
            let context = factory.new_context();
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
}

fn finish_report(
    args: &TestArgs,
    log_dir: Option<PathBuf>,
    results: &[report::ComponentSpec],
    started: Instant,
    serving: bool,
) -> Result<(), Error> {
    if let Some(dir) = &log_dir {
        println!("Log: {}", dir.join("moqui.log").display());
    }
    if let Some(dir) = &args.junit {
        report::write_junit(dir, results, "")?;
    }
    let (text, failed) = report::summary(results, started.elapsed());
    print!("{text}");
    if failed > 0 {
        if serving {
            println!("Not serving: the specs did not pass");
        }
        return Err(Error::new(format!("{failed} test failure(s)")));
    }
    Ok(())
}

/// A spec that `requires` features this build leaves out: one skipped case that names them.
pub(crate) fn skipped_spec(spec: &SpecFile, missing: &[String]) -> SpecResult {
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
            output: String::new(),
        }],
        setup_failure: None,
        cleanup_failure: None,
        duration: std::time::Duration::ZERO,
    }
}

/// Leave out the components the factory did not load, and return their names.
pub(crate) fn drop_unloaded(components: &mut Vec<TestComponent>, factory: &ExecutionContextFactory) -> Vec<String> {
    let loaded = loaded_locations(factory);
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

fn loaded_locations(factory: &ExecutionContextFactory) -> Vec<PathBuf> {
    factory
        .components()
        .iter()
        .map(|info| std::fs::canonicalize(&info.location).unwrap_or_else(|_| info.location.clone()))
        .collect()
}

/// The service coverage of each component that ran, on the console, and in full in the `--coverage-report` file.
#[cfg(feature = "embedded-postgres")]
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
pub(crate) fn order_by_load(components: &mut [TestComponent], factory: &ExecutionContextFactory) {
    let loaded = loaded_locations(factory);
    components.sort_by_key(|component| {
        let dir = std::fs::canonicalize(&component.dir).unwrap_or_else(|_| component.dir.clone());
        loaded
            .iter()
            .position(|location| *location == dir)
            .unwrap_or(usize::MAX)
    });
}

#[cfg(test)]
mod tests;
