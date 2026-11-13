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
//! A component's specs, in `test` or a subdirectory of it, run in the order of its `test/suite.js`
//! (`suite(["SpecA", "SpecB"])`), then the other `*.spec.js` files by name. Components run in the order they load, and
//! a spec waits for the specs its `dependsOn` names, wherever they are in the run ([`run_order`]). `--spec` picks specs
//! by name and adds the specs their `dependsOn` names, from any component, which joins the run. Each spec runs in its
//! own execution context. A `dependsOn` name a spec's component lacks comes from the run's other components. Each
//! component's `test/MoquiConf.xml` merges after its `MoquiConf.xml` in a test run, and `--serve` then serves the
//! scratch database (`serve.rs`, plans/archive/ClientTestHarness.md 6).
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
#[cfg(feature = "embedded-postgres")]
mod interrupt;
pub(crate) mod report;
#[cfg(feature = "embedded-postgres")]
mod serve;
#[cfg(feature = "llm")]
mod skills;
#[cfg(feature = "embedded-postgres")]
mod workers;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use moqui_core::context::layout;
use moqui_core::context::ExecutionContextFactory;
use moqui_devkit::testkit::{
    run_spec, spec_database_free, spec_depends_on, spec_missing_features, suite_order, CaseResult, Outcome, RunOptions,
    SpecFile, SpecResult,
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
    /// `--acceptance`: run the acceptance suite (`test/acceptance/*.spec.js`) in place of the regression suite (`test/`);
    /// `--spec` and `dependsOn` resolve across both.
    pub acceptance: bool,
    /// `--workers N`: run the specs in N child processes, each on its own copy of the `--db snapshot` database.
    pub workers: usize,
    /// `--results-json file`: write the results here as JSON, for the parent of a worker.
    pub results_json: Option<PathBuf>,
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
    /// `--serve [address]`: after the specs pass, serve the run's database (the scratch one, or the runtime's own with
    /// `--db existing`) on this address until stopped.
    pub serve: Option<std::net::SocketAddr>,
    /// `--ready-file path`: with `--serve`, the server's URL is written here once it listens.
    pub ready_file: Option<PathBuf>,
}

pub fn parse_test_args(args: &[String]) -> Result<TestArgs, Error> {
    let mut parsed = TestArgs {
        components: Vec::new(),
        specs: Vec::new(),
        feature: None,
        acceptance: false,
        workers: 1,
        results_json: None,
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
            "--acceptance" => parsed.acceptance = true,
            "--workers" => {
                parsed.workers = value("--workers")?
                    .parse()
                    .ok()
                    .filter(|workers| *workers > 0)
                    .ok_or_else(|| Error::new("--workers is a whole number of at least 1"))?;
            }
            "--results-json" => parsed.results_json = Some(PathBuf::from(value("--results-json")?)),
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
        if parsed.agents || parsed.skills {
            return Err(Error::new(
                "--serve serves after the specs; it does not go with --agents or --skills",
            ));
        }
        if parsed.workers > 1 {
            return Err(Error::new(
                "--workers runs the specs on copies of the database; it does not go with --serve",
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

/// The components in each of the runtime's component sets (`context::layout`; the dev set loads on a dev instance)
/// that have specs in `sub`: `test` for `moqui test`, `test/client` for `moqui test-client`, `test/acceptance` and
/// `test/acceptance/client` for their `--acceptance`. By directory name within each set. Not a template, whose name is a
/// placeholder (the dev set's `template`).
pub(crate) fn discover(runtime: &Path, sub: &str) -> Result<Vec<TestComponent>, Error> {
    let mut found = Vec::new();
    for (_, dir) in layout::component_dirs(runtime, &layout::Set::ALL) {
        let test_dir = dir.join(sub);
        if !test_dir.is_dir() {
            continue;
        }
        let name = component_name(&dir);
        if name.contains('@') {
            continue;
        }
        let specs = component_specs(&name, &test_dir, sub)?;
        if !specs.is_empty() {
            found.push(TestComponent { name, dir, specs });
        }
    }
    Ok(found)
}

/// The directory of the component named `name` in one of the runtime's component sets.
pub(crate) fn component_dir_named(runtime: &Path, name: &str) -> Option<PathBuf> {
    layout::component_dirs(runtime, &layout::Set::ALL)
        .into_iter()
        .map(|(_, dir)| dir)
        .find(|dir| component_name(dir) == name)
}

/// Whether a component of `components` is in a set that loads only on some instance purposes, so the run boots on the
/// dev purpose, where it loads (plans/design-dev-aide.md 3.1).
pub(crate) fn needs_dev_purpose(runtime: &Path, components: &[TestComponent]) -> bool {
    components
        .iter()
        .any(|component| layout::set_of(runtime, &component.dir).is_some_and(layout::Set::is_restricted))
}

/// The runtime's component set directories, for a message that none of them has what was asked for.
pub(crate) fn set_dirs_text(runtime: &Path) -> String {
    let dirs: Vec<String> = layout::Set::ALL
        .iter()
        .map(|set| layout::set_dir(runtime, *set).display().to_string())
        .collect();
    dirs.join(", ")
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

/// The subdirectories of a test directory that hold another command's files: `test/client` (`moqui test-client`) and
/// `test/agent` (`--agents`) under `test`, `test/client/process` (named process specs) under `test/client`.
fn reserved_subdirs(sub: &str) -> &'static [&'static str] {
    match sub {
        // test/load holds load scripts, another command's (moqui test-load)
        "test" => &["client", "agent", "acceptance", "load"],
        "test/acceptance" => &["client", "agent"],
        "test/client" | "test/acceptance/client" => &["process"],
        _ => &[],
    }
}

/// The tree of a suite: `test` for the regression suite, `test/acceptance` for the acceptance suite; with `client`, its
/// `client` subdirectory.
pub(crate) fn suite_tree(acceptance: bool, client: bool) -> &'static str {
    match (acceptance, client) {
        (false, false) => "test",
        (false, true) => "test/client",
        (true, false) => "test/acceptance",
        (true, true) => "test/acceptance/client",
    }
}

/// The specs a run picks from. For the regression suite, the specs of `test` (or `test/client`). For the acceptance
/// suite, each component's regression specs and then its acceptance specs, so that `--spec` and `dependsOn` resolve
/// across both trees and an acceptance spec can depend on a regression fixture; [`acceptance_only`] is what such a run
/// takes when no spec is named. A spec name is unique across both trees of a component.
pub(crate) fn discover_suite(runtime: &Path, client: bool, acceptance: bool) -> Result<Vec<TestComponent>, Error> {
    let mut components = discover(runtime, suite_tree(false, client))?;
    if !acceptance {
        return Ok(components);
    }
    for extra in discover(runtime, suite_tree(true, client))? {
        match components.iter_mut().find(|known| known.name == extra.name) {
            Some(known) => {
                for spec in extra.specs {
                    if let Some(other) = known.specs.iter().find(|have| have.name == spec.name) {
                        return Err(Error::new(format!(
                            "{} and {} are both spec {}; a spec name is unique in its component, across test/ and test/acceptance/",
                            spec.display, other.display, spec.name
                        )));
                    }
                    known.specs.push(spec);
                }
            }
            None => components.push(extra),
        }
    }
    Ok(components)
}

/// Whether a spec is in the acceptance suite: its display name is `<component>/test/acceptance/...`.
pub(crate) fn is_acceptance(spec: &SpecFile) -> bool {
    is_acceptance_display(&spec.display)
}

/// Whether a spec's display name, `<component>/<tree>/...`, is in the acceptance suite's tree.
pub(crate) fn is_acceptance_display(display: &str) -> bool {
    display
        .split_once('/')
        .is_some_and(|(_, tree)| tree.starts_with("test/acceptance/"))
}

/// The components of `names` (or all, when it is empty) that still have their acceptance suite in a top-level
/// `acceptance/` directory, which moved to `test/acceptance/` (plans/Components-Dir.md), so its specs are not left out
/// without a word: an acceptance run is refused, and a regression run, which does not read it, says so on stderr.
pub(crate) fn check_stale_acceptance(runtime: &Path, acceptance: bool, names: &[String]) -> Result<(), Error> {
    let stale: Vec<String> = layout::component_dirs(runtime, &layout::Set::ALL)
        .into_iter()
        .filter(|(_, dir)| dir.join("acceptance").is_dir())
        .filter(|(_, dir)| names.is_empty() || names.contains(&component_name(dir)))
        .map(|(_, dir)| dir.display().to_string())
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    let text = format!(
        "the acceptance suite is in test/acceptance/ now; move acceptance/ there (git mv acceptance test/acceptance) in {}",
        stale.join(", ")
    );
    if acceptance {
        return Err(Error::new(text));
    }
    eprintln!("Warning: {text}");
    Ok(())
}

/// The acceptance specs of `components`, without the regression specs of the pool; components left with none are
/// dropped.
pub(crate) fn acceptance_only(components: Vec<TestComponent>) -> Vec<TestComponent> {
    components
        .into_iter()
        .filter_map(|mut component| {
            component.specs.retain(is_acceptance);
            (!component.specs.is_empty()).then_some(component)
        })
        .collect()
}

/// The `*.spec.js` files under `dir` and its subdirectories, other than the `skip` subdirectories of the top one.
fn spec_paths(dir: &Path, skip: &[&str], found: &mut Vec<PathBuf>) -> Result<(), Error> {
    let entries = std::fs::read_dir(dir)
        .map_err(|err| Error::new(format!("could not read {}", dir.display())).with_source(err))?;
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.is_dir() {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            if !skip.contains(&name.as_str()) {
                spec_paths(&path, &[], found)?;
            }
        } else if path.to_string_lossy().ends_with(".spec.js") {
            found.push(path);
        }
    }
    Ok(())
}

/// The specs of one component's test directory (`sub` under the component, `test` or `test/client`), in it or in a
/// subdirectory: the `suite.js` order first, then the rest by name. A spec's name is its file name, so it is unique
/// in the component.
pub(crate) fn component_specs(component: &str, test_dir: &Path, sub: &str) -> Result<Vec<SpecFile>, Error> {
    let mut files: Vec<PathBuf> = Vec::new();
    spec_paths(test_dir, reserved_subdirs(sub), &mut files)?;
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then_with(|| a.cmp(b)));
    let mut specs: Vec<SpecFile> = Vec::new();
    for path in files {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = file_name.trim_end_matches(".spec.js").to_string();
        let relative = path
            .strip_prefix(test_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let display = format!("{component}/{sub}/{relative}");
        if let Some(other) = specs.iter().find(|spec| spec.name == name) {
            return Err(Error::new(format!(
                "{display} and {} are both spec {name}; a spec name is unique in its component",
                other.display
            )));
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
        specs.push(SpecFile { name, display, source });
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
        // `component/Spec` names the spec of that component only
        let (only, name) = match name.split_once('/') {
            Some((component, spec)) => (Some(component), spec.to_string()),
            None => (None, name.clone()),
        };
        for (index, component) in components.iter().enumerate() {
            if only.is_none_or(|only| only == component.name) && component.specs.iter().any(|spec| spec.name == name) {
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

/// The `--spec` names of a run that names components: an unqualified name becomes `component/Spec` for each named
/// component that has it, so a spec of that name elsewhere in the pool is not picked; a name none of them has is an
/// error. With no component named, or a name already qualified, the names stay as they are.
pub(crate) fn qualify_spec_names(
    components: &[TestComponent],
    named: &[String],
    specs: &[String],
) -> Result<Vec<String>, Error> {
    if named.is_empty() {
        return Ok(specs.to_vec());
    }
    let mut out = Vec::new();
    for name in specs {
        if name.contains('/') {
            out.push(name.clone());
            continue;
        }
        let owners: Vec<&TestComponent> = components
            .iter()
            .filter(|component| {
                named.contains(&component.name) && component.specs.iter().any(|spec| &spec.name == name)
            })
            .collect();
        if owners.is_empty() {
            return Err(Error::new(format!(
                "none of the components named ({}) has a spec {name}",
                named.join(", ")
            )));
        }
        out.extend(owners.iter().map(|component| format!("{}/{name}", component.name)));
    }
    Ok(out)
}

/// The specs of a component that its own client suites (`test/client/suite.js`, `test/acceptance/client/suite.js`) name in
/// `server.specs`: fixtures that `moqui test-client --serve` runs on its server.
fn served_fixture_names(component: &TestComponent) -> Vec<String> {
    let mut names = Vec::new();
    for tree in ["test/client", "test/acceptance/client"] {
        let Ok(text) = std::fs::read_to_string(component.dir.join(tree).join("suite.js")) else {
            continue;
        };
        let Ok(Some(server)) = moqui_devkit::testkit::suite_server(&text) else {
            continue;
        };
        for name in server.specs {
            if component.specs.iter().any(|spec| spec.name == name) && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// Leave out of a plain run each fixture spec a served suite of its component runs, unless a spec that stays depends on
/// it; return the names left out, as `component/Spec`.
pub(crate) fn drop_served_fixtures(components: &mut [TestComponent]) -> Vec<String> {
    let mut left_out = Vec::new();
    for component in components.iter_mut() {
        let fixtures = served_fixture_names(component);
        if fixtures.is_empty() {
            continue;
        }
        // the specs that stay, and what they depend on, through the fixtures too
        let mut keep: Vec<String> = component
            .specs
            .iter()
            .filter(|spec| !fixtures.contains(&spec.name))
            .map(|spec| spec.name.clone())
            .collect();
        let mut index = 0;
        while index < keep.len() {
            let name = keep[index].clone();
            if let Some(spec) = component.specs.iter().find(|spec| spec.name == name) {
                for depends in spec_depends_on(&spec.source) {
                    if !keep.contains(&depends) {
                        keep.push(depends);
                    }
                }
            }
            index += 1;
        }
        component.specs.retain(|spec| {
            let stays = keep.contains(&spec.name);
            if !stays {
                left_out.push(format!("{}/{}", component.name, spec.name));
            }
            stays
        });
    }
    left_out
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
    let tree = suite_tree(args.acceptance, false);
    check_stale_acceptance(&runtime, args.acceptance, &args.components)?;
    let mut components = discover_suite(&runtime, false, args.acceptance)?;
    // The components the run takes: those named, and those an acceptance spec's dependsOn reaches.
    let mut wanted = args.components.clone();
    if args.acceptance && args.specs.is_empty() {
        // The acceptance specs of the components named (or of all), with what their dependsOn names from the whole pool:
        // a sweep that depends on another component's regression flows brings them, as --spec does.
        let names: Vec<String> = acceptance_only(components.clone())
            .iter()
            .filter(|component| args.components.is_empty() || args.components.contains(&component.name))
            .flat_map(|component| component.specs.iter().map(|spec| spec.name.clone()))
            .collect();
        components = select_across(&components, &names)?;
        if !wanted.is_empty() {
            for component in &components {
                if !wanted.contains(&component.name) {
                    wanted.push(component.name.clone());
                }
            }
        }
    }
    // --spec: the specs named, in the components named (or in any), and what their dependsOn reaches in the whole pool,
    // which brings the components it reaches into the run.
    let picked = if args.specs.is_empty() {
        None
    } else {
        let names = qualify_spec_names(&components, &args.components, &args.specs)?;
        let picked = select_across(&components, &names)?;
        for name in &names {
            let (only, spec_name) = match name.split_once('/') {
                Some((component, spec)) => (Some(component), spec),
                None => (None, name.as_str()),
            };
            if !picked.iter().any(|component| {
                only.is_none_or(|only| only == component.name)
                    && component.specs.iter().any(|spec| spec.name == spec_name)
            }) {
                return Err(Error::new(format!("no component has a spec {name}")));
            }
        }
        if !wanted.is_empty() {
            for component in &picked {
                if !wanted.contains(&component.name) {
                    wanted.push(component.name.clone());
                }
            }
        }
        Some(picked)
    };
    // With --serve a named component may have no specs: it is there for the server, and it must load.
    let mut serve_only: Vec<String> = Vec::new();
    if !wanted.is_empty() {
        for name in &wanted {
            if !components.iter().any(|component| &component.name == name) {
                if args.serve.is_none() {
                    return Err(Error::new(format!("component {name} has no {tree}/*.spec.js files")));
                }
                serve_only.push(name.clone());
            }
        }
        components.retain(|component| wanted.contains(&component.name));
    }
    if let Some(picked) = picked {
        components = picked;
    }
    // A fixture runs where a served suite names it, not again in a plain run (plans/archive/Test-Reduction.md,
    // "Fixtures once").
    if !args.acceptance && args.specs.is_empty() && args.serve.is_none() {
        let left_out = drop_served_fixtures(&mut components);
        if !left_out.is_empty() {
            println!(
                "Left out the fixture specs their served client suites run: {} (name one with --spec to run it)",
                left_out.join(", ")
            );
        }
    }
    if components.is_empty() && args.serve.is_none() {
        return Err(Error::new(format!(
            "no {tree}/*.spec.js files in the components of {}",
            set_dirs_text(&runtime)
        )));
    }
    // A dev component's specs run on a dev instance, where it loads (plans/design-dev-aide.md 3.1).
    if needs_dev_purpose(&runtime, &components) && moqui_lib::env::get_prop_or_env("instance_purpose").is_empty() {
        moqui_lib::env::set_property("instance_purpose", "dev");
    }
    // a spec may answer outbound HTTP with fixtures (docs/dev/JS-Testkit.md, "HTTP fixtures"), only in a test run
    moqui_core::service::http_fixtures::allow();
    let mode = database_mode(&args, &components)?;
    let started = Instant::now();
    #[cfg(feature = "embedded-postgres")]
    if args.workers > 1 && matches!(mode, DbMode::Fresh | DbMode::Snapshot) && !args.coverage {
        return workers::run(&args, &runtime, &framework, &mut components, started);
    }
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
    _pdf: crate::PdfSlot,
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
        _pdf: crate::install_pdf(&factory),
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
    run_component_specs(&factory, components, args, &mut results)?;
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
    // Nothing to run on the runtime's own database: no test factory to boot and shut down, the server starts at once.
    if let (true, DbMode::Existing, Some(address), Some(setup)) =
        (components.is_empty(), args.db, args.serve, serve_setup.as_ref())
    {
        return serve::serve_existing(
            runtime,
            framework,
            serve_only,
            address,
            args.ready_file.as_deref(),
            setup,
        );
    }
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
    run_component_specs(&fixture.factory, components, args, &mut results)?;
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

/// The `suite.js` chains of a component in a run, `test/suite.js` and in an acceptance run `test/acceptance/suite.js`
/// (with `client`, those of `test/client` and `test/acceptance/client`), each as the indexes of the specs it lists that the
/// component has, in its order. A suite.js that cannot be read lists nothing (discovery has already read it).
pub(crate) fn suite_chains(component: &TestComponent, acceptance: bool, client: bool) -> Vec<Vec<usize>> {
    let trees: &[&str] = if acceptance {
        &[suite_tree(false, client), suite_tree(true, client)]
    } else {
        &[suite_tree(false, client)]
    };
    trees
        .iter()
        .filter_map(|tree| std::fs::read_to_string(component.dir.join(tree).join("suite.js")).ok())
        .map(|text| {
            suite_order(&text)
                .unwrap_or_default()
                .iter()
                .filter_map(|name| component.specs.iter().position(|spec| &spec.name == name))
                .collect()
        })
        .collect()
}

/// The order a run takes its specs, as `(component index, spec index)`: the components' order and each component's
/// own, except that a spec runs after the specs its `dependsOn` names, wherever they are in the run. A stable
/// topological sort: of the specs whose dependencies have run, the one first in the components' order goes next, so a
/// spec moves only to follow what it depends on. A `dependsOn` name resolves as in [`select_across`] (the spec's own
/// component first, then the others in order); a name not in the run orders nothing. The specs a `suite.js` lists
/// keep its order among themselves, so a spec that waits holds back the ones after it in its chain, as a chain shares
/// data (a worker unit, `workers.rs`). A cycle is an error naming its specs. `moqui test-client` orders its run the
/// same way, with `client` for the suite.js chains of `test/client` (and `test/acceptance/client`).
pub(crate) fn run_order(
    components: &[TestComponent],
    acceptance: bool,
    client: bool,
) -> Result<Vec<(usize, usize)>, Error> {
    let nodes: Vec<(usize, usize)> = components
        .iter()
        .enumerate()
        .flat_map(|(c, component)| (0..component.specs.len()).map(move |s| (c, s)))
        .collect();
    // for each node, the nodes it depends on
    let mut depends: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (at, &(c, s)) in nodes.iter().enumerate() {
        for name in spec_depends_on(&components[c].specs[s].source) {
            let owner = std::iter::once(c)
                .chain((0..components.len()).filter(|other| *other != c))
                .find_map(|other| {
                    let found = components[other].specs.iter().position(|spec| spec.name == name)?;
                    nodes.iter().position(|node| *node == (other, found))
                });
            if let Some(owner) = owner {
                if !depends[at].contains(&owner) {
                    depends[at].push(owner);
                }
            }
        }
    }
    for (c, component) in components.iter().enumerate() {
        for chain in suite_chains(component, acceptance, client) {
            for pair in chain.windows(2) {
                let (Some(before), Some(after)) = (
                    nodes.iter().position(|node| *node == (c, pair[0])),
                    nodes.iter().position(|node| *node == (c, pair[1])),
                ) else {
                    continue;
                };
                if !depends[after].contains(&before) {
                    depends[after].push(before);
                }
            }
        }
    }
    let mut done = vec![false; nodes.len()];
    let mut order = Vec::with_capacity(nodes.len());
    while order.len() < nodes.len() {
        let next = (0..nodes.len()).find(|at| !done[*at] && depends[*at].iter().all(|dep| done[*dep]));
        let Some(next) = next else {
            let waiting: Vec<String> = (0..nodes.len())
                .filter(|at| !done[*at])
                .map(|at| {
                    format!(
                        "{}/{}",
                        components[nodes[at].0].name, components[nodes[at].0].specs[nodes[at].1].name
                    )
                })
                .collect();
            return Err(Error::new(format!(
                "the dependsOn of these specs make a cycle (or wait on one), so none of them can run first: {}",
                waiting.join(", ")
            )));
        };
        done[next] = true;
        order.push(nodes[next]);
    }
    Ok(order)
}

fn run_component_specs(
    factory: &Arc<ExecutionContextFactory>,
    components: &[TestComponent],
    args: &TestArgs,
    results: &mut Vec<report::ComponentSpec>,
) -> Result<(), Error> {
    let options = RunOptions {
        feature_filter: args.feature.clone(),
        ..RunOptions::default()
    };
    for (c, s) in run_order(components, args.acceptance, false)? {
        let component = &components[c];
        let spec = &component.specs[s];
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
    Ok(())
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
    #[cfg(feature = "embedded-postgres")]
    if let Some(path) = &args.results_json {
        workers::write_results(path, results)?;
    }
    let (text, mut failed) = report::summary(results, started.elapsed());
    print!("{text}");
    write_script_stats();
    // the regression budget: not a worker's part of a run (its parent checks the whole), not the acceptance suite, not
    // the fixture run of a served suite
    if !args.acceptance && args.results_json.is_none() && args.serve.is_none() {
        let runtime = crate::runtime_framework_dirs().0;
        let (text, over) = report::budget_check(results, &|name| {
            component_dir_named(&runtime, name).and_then(|dir| report::read_budget(&dir, "server"))
        });
        print!("{text}");
        failed += over;
    }
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
    spec_not_run(
        spec,
        Outcome::Skipped(format!(
            "requires {}, not built into this binary (moqui version)",
            missing.join(", ")
        )),
    )
}

/// A spec that does not run, as one "(every feature)" case with `outcome`.
fn spec_not_run(spec: &SpecFile, outcome: Outcome) -> SpecResult {
    SpecResult {
        name: spec.name.clone(),
        display: spec.display.clone(),
        cases: vec![CaseResult {
            name: "(every feature)".to_string(),
            outcome,
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

/// With `MOQUI_SCRIPT_STATS_FILE` set, a JSON line of this process's script engine counts appended to that file
/// (`moqui_core::script::engine_stats`): a worker of a run writes its own. The runtimes' counts are under `oxide`, and
/// the scripts run by entry kind under `scripts_by_kind`.
fn write_script_stats() {
    let path = moqui_lib::env::get_prop_or_env("MOQUI_SCRIPT_STATS_FILE");
    if path.is_empty() {
        return;
    }
    let stats = moqui_core::script::engine_stats();
    let by_kind: serde_json::Map<String, serde_json::Value> = stats
        .scripts_by_kind
        .iter()
        .map(|(kind, runs)| (kind.to_string(), serde_json::json!(runs)))
        .collect();
    let line = serde_json::json!({
        "pid": std::process::id(),
        "depth_max": stats.depth_max,
        "oxide": {
            "engines_started": stats.oxide_engines_started,
            "engines_live": stats.oxide_engines_live,
            "engines_live_max": stats.oxide_engines_live_max,
            "heap_bytes": stats.oxide_heap_bytes,
            "heap_bytes_max": stats.oxide_heap_bytes_max,
            "compiled": stats.oxide_compiled,
            "poisoned": stats.oxide_poisoned,
            "restarted": stats.oxide_restarted,
            "shed_fragmented": stats.oxide_shed_fragmented,
            "released_idle": stats.oxide_released_idle,
            "images_made": stats.oxide_images_made,
            "image_loads": stats.oxide_image_loads,
            "image_failures": stats.oxide_image_failures,
            "image_bytes": stats.oxide_image_bytes,
            "heap_bytes_peak_threads_sum": stats.oxide_heap_peaks.iter().map(|(_, bytes)| bytes).sum::<usize>(),
            "heap_bytes_peak_by_thread": stats
                .oxide_heap_peaks
                .iter()
                .map(|(thread, bytes)| serde_json::json!({"thread": thread, "peak": bytes}))
                .collect::<Vec<_>>(),
            "heap_at_max": stats.oxide_heap_at_max.0,
            "heap_at_max_by_runtime": stats
                .oxide_heap_at_max
                .1
                .iter()
                .map(|(thread, bytes)| serde_json::json!({"thread": thread, "bytes": bytes}))
                .collect::<Vec<_>>(),
        },
        "scripts_by_kind": by_kind,
    });
    let line = format!("{line}\n");
    use std::io::Write as _;
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut file) => {
            if let Err(err) = file.write_all(line.as_bytes()) {
                tracing::warn!(error = %err, path, "could not write the script stats");
            }
        }
        Err(err) => tracing::warn!(error = %err, path, "could not open the script stats"),
    }
}
