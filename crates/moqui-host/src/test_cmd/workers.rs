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

//! `moqui test --workers N` (plans/Test-Reduction.md, "Cheaper machinery"): the specs of the run in N child `moqui test`
//! processes, each on its own copy of the `--db snapshot` database. The specs are cut into units that share no data: a
//! component's `suite.js` chain is one unit, and a spec joins the unit of each spec its `dependsOn` names; every other
//! spec is a unit of its own. The units go to the workers longest first (by the size of their files), so the workers
//! end at about the same time. A spec that reads what another spec wrote without naming it in `dependsOn` passes in a
//! serial run and fails here: name it.
//!
//! Each child writes its results as JSON (`--results-json`); the parent prints each child's lines when it ends, then
//! one summary and the JUnit files of the whole run.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use moqui_devkit::testkit::{spec_depends_on, suite_order, CaseResult, Failure, Outcome, SpecResult};
use moqui_lib::error::Error;

use super::{report, suite_tree, DbMode, TestArgs, TestComponent};

/// One unit: specs that run together, in order, as `(component index, spec index)`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Unit {
    pub(crate) specs: Vec<(usize, usize)>,
    pub(crate) weight: usize,
}

/// The units of `components` (in run order): the `suite.js` chain of each component, joined with what its specs'
/// `dependsOn` names (in its own component first, then the others), and each other spec on its own or with what it
/// depends on.
pub(crate) fn units(components: &[TestComponent], acceptance: bool) -> Vec<Unit> {
    let nodes: Vec<(usize, usize)> = components
        .iter()
        .enumerate()
        .flat_map(|(c, component)| (0..component.specs.len()).map(move |s| (c, s)))
        .collect();
    let index_of = |c: usize, s: usize| nodes.iter().position(|node| *node == (c, s)).unwrap_or(0);
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn find(parent: &mut [usize], mut at: usize) -> usize {
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }
    let join = |parent: &mut Vec<usize>, a: usize, b: usize| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[rb] = ra;
        }
    };
    for (c, component) in components.iter().enumerate() {
        // the suite.js chain: the specs it lists share a unit
        let mut chain: Vec<usize> = Vec::new();
        for tree in [suite_tree(false, false), suite_tree(acceptance, false)] {
            if let Ok(text) = std::fs::read_to_string(component.dir.join(tree).join("suite.js")) {
                for name in suite_order(&text).unwrap_or_default() {
                    if let Some(s) = component.specs.iter().position(|spec| spec.name == name) {
                        chain.push(index_of(c, s));
                    }
                }
            }
        }
        for pair in chain.windows(2) {
            join(&mut parent, pair[0], pair[1]);
        }
        for (s, spec) in component.specs.iter().enumerate() {
            for depends in spec_depends_on(&spec.source) {
                let owner = std::iter::once(c)
                    .chain((0..components.len()).filter(|other| *other != c))
                    .find_map(|at| {
                        components[at]
                            .specs
                            .iter()
                            .position(|other| other.name == depends)
                            .map(|found| (at, found))
                    });
                if let Some((at, found)) = owner {
                    join(&mut parent, index_of(c, s), index_of(at, found));
                }
            }
        }
    }
    let mut units: Vec<(usize, Unit)> = Vec::new();
    for (at, &(c, s)) in nodes.iter().enumerate() {
        let root = find(&mut parent, at);
        let weight = components[c].specs[s].source.len();
        match units.iter_mut().find(|(have, _)| *have == root) {
            Some((_, unit)) => {
                unit.specs.push((c, s));
                unit.weight += weight;
            }
            None => units.push((
                root,
                Unit {
                    specs: vec![(c, s)],
                    weight,
                },
            )),
        }
    }
    units.into_iter().map(|(_, unit)| unit).collect()
}

/// The units dealt to `workers` bins, longest first, each to the bin with the least so far; empty bins are dropped.
pub(crate) fn deal(mut units: Vec<Unit>, workers: usize) -> Vec<Vec<Unit>> {
    units.sort_by_key(|unit| std::cmp::Reverse(unit.weight));
    let mut bins: Vec<(usize, Vec<Unit>)> = (0..workers.max(1)).map(|_| (0, Vec::new())).collect();
    for unit in units {
        if let Some(bin) = bins.iter_mut().min_by_key(|(load, _)| *load) {
            bin.0 += unit.weight;
            bin.1.push(unit);
        }
    }
    bins.into_iter()
        .map(|(_, units)| units)
        .filter(|units| !units.is_empty())
        .collect()
}

/// The arguments of a worker: `test`, its components, its specs, the snapshot, the results file, and the options of
/// the run that a child takes as they are.
pub(crate) fn worker_args(args: &TestArgs, components: &[TestComponent], bin: &[Unit], results: &Path) -> Vec<String> {
    let mut specs: Vec<(usize, usize)> = bin.iter().flat_map(|unit| unit.specs.iter().copied()).collect();
    specs.sort();
    let mut names: Vec<String> = Vec::new();
    let mut component_names: Vec<String> = Vec::new();
    for (c, s) in specs {
        // qualified, as two components may have a spec of one name
        let name = format!("{}/{}", components[c].name, components[c].specs[s].name);
        if !names.contains(&name) {
            names.push(name);
        }
        if !component_names.contains(&components[c].name) {
            component_names.push(components[c].name.clone());
        }
    }
    let mut out = vec!["test".to_string()];
    out.extend(component_names);
    out.push("--spec".to_string());
    out.push(names.join(","));
    if args.acceptance {
        out.push("--acceptance".to_string());
    }
    if let Some(feature) = &args.feature {
        out.push("--feature".to_string());
        out.push(feature.clone());
    }
    if let Some(level) = &args.log_level {
        out.push("--log-level".to_string());
        out.push(level.clone());
    }
    for arg in ["--db", "snapshot", "--results-json"] {
        out.push(arg.to_string());
    }
    out.push(results.display().to_string());
    out
}

/// Write a run's results for the parent of a worker.
pub(crate) fn write_results(path: &Path, results: &[report::ComponentSpec]) -> Result<(), Error> {
    let list: Vec<serde_json::Value> = results
        .iter()
        .map(|spec| {
            serde_json::json!({
                "component": spec.component,
                "result": serde_json::to_value(&spec.result).unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();
    std::fs::write(path, serde_json::Value::Array(list).to_string())
        .map_err(|err| Error::new(format!("could not write {}", path.display())).with_source(err))
}

fn read_results(path: &Path) -> Option<Vec<report::ComponentSpec>> {
    let text = std::fs::read_to_string(path).ok()?;
    let list: Vec<serde_json::Value> = serde_json::from_str(&text).ok()?;
    list.into_iter()
        .map(|item| {
            Some(report::ComponentSpec {
                component: item.get("component")?.as_str()?.to_string(),
                result: serde_json::from_value::<SpecResult>(item.get("result")?.clone()).ok()?,
            })
        })
        .collect()
}

/// Each spec of a worker that left no results, failed with the end of its output.
fn worker_failed(components: &[TestComponent], bin: &[Unit], output: &str) -> Vec<report::ComponentSpec> {
    let tail: Vec<&str> = output.lines().rev().take(30).collect();
    let message = format!(
        "the worker ended without its results:\n{}",
        tail.into_iter().rev().collect::<Vec<_>>().join("\n")
    );
    bin.iter()
        .flat_map(|unit| unit.specs.iter())
        .map(|&(c, s)| {
            let spec = &components[c].specs[s];
            report::ComponentSpec {
                component: components[c].name.clone(),
                result: SpecResult {
                    name: spec.name.clone(),
                    display: spec.display.clone(),
                    cases: vec![CaseResult {
                        name: "(every feature)".to_string(),
                        outcome: Outcome::Failed(Failure {
                            kind: "error".to_string(),
                            block: "worker".to_string(),
                            message: message.clone(),
                            location: None,
                            stack: String::new(),
                            details: Vec::new(),
                            attachments: Vec::new(),
                        }),
                        duration: std::time::Duration::ZERO,
                        output: String::new(),
                    }],
                    setup_failure: None,
                    cleanup_failure: None,
                    duration: std::time::Duration::ZERO,
                },
            }
        })
        .collect()
}

/// Run `components` (selected, loaded and in load order) on `workers` children, after the snapshot is in place, and
/// report the whole run as one.
pub(crate) fn run(
    args: &TestArgs,
    runtime: &Path,
    framework: &Path,
    components: &mut Vec<TestComponent>,
    started: Instant,
) -> Result<(), Error> {
    // The snapshot, made once here so the workers only restore it; and the load order of the components.
    {
        let fixture = super::fixture::open(runtime, framework, DbMode::Snapshot, false)?;
        super::drop_unloaded(components, &fixture.factory);
        super::order_by_load(components, &fixture.factory);
    }
    let bins = deal(units(components, args.acceptance), args.workers);
    let out_dir = runtime.join("tmp").join(format!("test-workers-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir)
        .map_err(|err| Error::new(format!("could not create {}", out_dir.display())).with_source(err))?;
    let exe = std::env::current_exe().map_err(|err| Error::new("could not find this binary").with_source(err))?;
    println!(
        "Running {} specs on {} workers",
        components.iter().map(|component| component.specs.len()).sum::<usize>(),
        bins.len()
    );
    let mut children = Vec::new();
    for (index, bin) in bins.iter().enumerate() {
        let results = out_dir.join(format!("worker-{}.json", index + 1));
        let log = out_dir.join(format!("worker-{}.log", index + 1));
        let log_file = std::fs::File::create(&log)
            .map_err(|err| Error::new(format!("could not create {}", log.display())).with_source(err))?;
        let err_file = log_file
            .try_clone()
            .map_err(|err| Error::new(format!("could not open {}", log.display())).with_source(err))?;
        let child = Command::new(&exe)
            .args(worker_args(args, components, bin, &results))
            .stdin(Stdio::null())
            .stdout(log_file)
            .stderr(err_file)
            .spawn()
            .map_err(|err| Error::new("could not start a worker").with_source(err))?;
        children.push((child, results, log));
    }
    let mut all: Vec<report::ComponentSpec> = Vec::new();
    for (index, (mut child, results, log)) in children.into_iter().enumerate() {
        let _ = child.wait();
        let output = std::fs::read_to_string(&log).unwrap_or_default();
        for line in output.lines().filter(|line| line.contains(" > ")) {
            println!("{line}");
        }
        match read_results(&results) {
            Some(list) => all.extend(list),
            None => all.extend(worker_failed(components, &bins[index], &output)),
        }
    }
    // in run order, as a serial run reports them
    let order = |result: &report::ComponentSpec| {
        components
            .iter()
            .enumerate()
            .find_map(|(c, component)| {
                component
                    .specs
                    .iter()
                    .position(|spec| spec.display == result.result.display)
                    .map(|s| (c, s))
            })
            .unwrap_or((usize::MAX, usize::MAX))
    };
    all.sort_by_key(order);
    println!("Worker logs: {}", out_dir.display());
    super::finish_report(args, None, &all, started, false)
}

#[cfg(test)]
pub(crate) fn unit_names(components: &[TestComponent], units: &[Unit]) -> Vec<Vec<String>> {
    units
        .iter()
        .map(|unit| {
            unit.specs
                .iter()
                .map(|&(c, s)| components[c].specs[s].name.clone())
                .collect()
        })
        .collect()
}
