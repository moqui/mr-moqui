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

//! `moqui test <component> --agents`: the agent evaluation tasks of each component's `test/agent/*.agent.js`
//! ([`moqui_core::testkit::agents`]) on a live model (`--llm-profile`) or a recording (`--replay`), on the database a
//! test run uses, with the agent engine started and the screen sessions installed as a server has them. Transcripts go
//! to `runtime/log/agent-eval/<run>/<spec>/<task>-<n>.jsonl` with a summary per component; the component's
//! `test/agent/baseline-<profile>.json`, when there is one, flags regressions.
//!
//! A replay passes when every task passes. A live run passes when its pass@1 reaches `--min-pass` (0.75, the plan's
//! 6 of 8) and nothing regressed against the baseline; a live endpoint that does not answer skips the run.

use std::path::{Path, PathBuf};
use std::time::Instant;

use moqui_core::testkit::agents::{self, AgentSpecFile, EvalOptions, Mode, TaskOutcome, TaskResult};
use moqui_core::testkit::SpecFile;
use moqui_lib::error::Error;

use super::{drop_unloaded, fixture, order_by_load, TestArgs, TestComponent};

/// The pass@1 a live run needs unless `--min-pass` says.
const MIN_PASS: f64 = 0.75;

/// Run the agent tasks of the components asked for. An error when a task of a replay failed, a live run fell short
/// of its pass@1 or regressed, or nothing could run.
pub(super) fn dispatch(args: &TestArgs, runtime: &Path, framework: &Path) -> Result<(), Error> {
    let mode = match (&args.llm_profile, &args.replay) {
        (_, Some(dir)) => Mode::Replay {
            dir: std::fs::canonicalize(dir)
                .map_err(|err| Error::new(format!("--replay {}: no such directory", dir.display())).with_source(err))?,
        },
        (Some(profile), None) => Mode::Live {
            profile: profile.clone(),
        },
        (None, None) => {
            return Err(Error::new(
                "--agents needs --llm-profile <model> or --replay <dir>\nRun 'moqui help test' for details.",
            ))
        }
    };
    let mut components = discover(runtime)?;
    if !args.components.is_empty() {
        for name in &args.components {
            if !components.iter().any(|component| &component.name == name) {
                return Err(Error::new(format!("component {name} has no test/agent/*.agent.js files")));
            }
        }
        components.retain(|component| args.components.contains(&component.name));
    }
    if !args.specs.is_empty() {
        for component in &mut components {
            component.specs.retain(|spec| args.specs.contains(&spec.name));
        }
        components.retain(|component| !component.specs.is_empty());
        for name in &args.specs {
            if !components.iter().any(|component| component.specs.iter().any(|spec| &spec.name == name)) {
                return Err(Error::new(format!("no component has an agent spec {name}")));
            }
        }
    }
    if components.is_empty() {
        return Err(Error::new(format!(
            "no test/agent/*.agent.js files under {}/component or {}/base-component",
            runtime.display(),
            runtime.display()
        )));
    }
    let started = Instant::now();
    let fixture = fixture::open(runtime, framework, args.db, args.keep)?;
    for name in drop_unloaded(&mut components, &fixture) {
        if args.components.contains(&name) {
            return Err(Error::new(format!("component {name} is not loaded; its component.xml may be disabled")));
        }
    }
    order_by_load(&mut components, &fixture);
    // the request pool (the screen sessions render on it), the engine and the screen sessions, as a server starts them
    fixture
        .factory
        .start_request_pool()
        .map_err(|err| Error::new(err.to_string()))?;
    moqui_core::llm::start(&fixture.factory);
    #[cfg(all(feature = "axum", feature = "tui"))]
    crate::llm::screens::install(&fixture.factory);

    let label = mode.label();
    let stamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
    let run_dir = std::fs::canonicalize(runtime)
        .unwrap_or_else(|_| runtime.to_path_buf())
        .join("log/agent-eval")
        .join(format!("{stamp}-{}", file_safe(&label)));
    let mut problems = Vec::new();
    for component in &components {
        let options = EvalOptions {
            mode: mode.clone(),
            runs: args.runs.max(1),
            seed: args.seed,
            task_filter: args.feature.clone(),
            out_dir: run_dir.clone(),
        };
        let specs: Vec<AgentSpecFile> = component
            .specs
            .iter()
            .map(|file| AgentSpecFile {
                component: component.name.clone(),
                file: file.clone(),
            })
            .collect();
        let evaluation = agents::evaluate(&fixture.factory, &specs, &options, &mut |result| print_result(result))?;
        if let Some(reason) = &evaluation.skipped {
            println!("Agent evaluation of {} skipped: {reason}", component.name);
            continue;
        }
        let summary = agents::summarize(&label, options.runs, &evaluation.results);
        print!("{}", summary.text(&component.name));
        write_json(
            &run_dir.join(format!("summary-{}.json", component.name)),
            &serde_json::json!({"component": component.name, "summary": summary,
                "results": evaluation.results.iter().map(result_json).collect::<Vec<_>>()}),
        );
        let baseline_path = component.dir.join("test/agent").join(format!("baseline-{}.json", file_safe(&label)));
        if let Some(baseline) = std::fs::read_to_string(&baseline_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        {
            let mut found = agents::regressions(&summary, &baseline);
            // a task of the baseline that did not run (a recording lost) is one, unless only some were asked for
            if args.feature.is_none() && args.specs.is_empty() {
                found.extend(agents::not_run(&summary, &baseline));
            }
            for line in &found {
                println!("REGRESSION {}: {line}", component.name);
            }
            if !found.is_empty() {
                problems.push(format!("{} regressed against {}", component.name, baseline_path.display()));
            }
        }
        if args.write_baseline {
            let text = serde_json::to_string_pretty(&summary.baseline()).unwrap_or_default() + "\n";
            std::fs::write(&baseline_path, text)
                .map_err(|err| Error::new(format!("could not write {}", baseline_path.display())).with_source(err))?;
            println!("Baseline written: {}", baseline_path.display());
        }
        let failed = evaluation
            .results
            .iter()
            .filter(|result| matches!(result.outcome, TaskOutcome::Failed(_)))
            .count();
        match &mode {
            Mode::Replay { .. } if failed > 0 => problems.push(format!("{failed} replayed task run(s) of {} failed", component.name)),
            Mode::Live { .. } => {
                let need = args.min_pass.unwrap_or(MIN_PASS);
                if summary.tasks_run > 0 && summary.pass_at1 + 1e-9 < need {
                    problems.push(format!(
                        "{}: pass@1 {:.2} is under {need:.2}",
                        component.name, summary.pass_at1
                    ));
                }
            }
            _ => {}
        }
    }
    let log_dir = fixture.log_dir.clone();
    drop(fixture);
    if let Some(dir) = &log_dir {
        println!("Log: {}", dir.join("moqui.log").display());
    }
    if run_dir.is_dir() {
        println!("Transcripts: {}", run_dir.display());
    }
    println!("Agent evaluation took {:.1}s", started.elapsed().as_secs_f64());
    if !problems.is_empty() {
        return Err(Error::new(problems.join("\n")));
    }
    Ok(())
}

fn print_result(result: &TaskResult) {
    println!("{}", result.line());
    if let TaskOutcome::Failed(failures) = &result.outcome {
        for failure in failures {
            println!("    {failure}");
        }
    }
    for note in &result.notes {
        println!("    note: {note}");
    }
}

fn result_json(result: &TaskResult) -> serde_json::Value {
    let (outcome, details) = match &result.outcome {
        TaskOutcome::Passed => ("passed", Vec::new()),
        TaskOutcome::Failed(failures) => ("failed", failures.clone()),
        TaskOutcome::Skipped(reason) => ("skipped", vec![reason.clone()]),
        TaskOutcome::Ignored(reason) => ("ignored", vec![reason.clone()]),
    };
    serde_json::json!({"spec": result.spec, "task": result.task, "run": result.run, "outcome": outcome,
        "details": details, "notes": result.notes, "metrics": result.metrics,
        "transcript": result.transcript.as_ref().map(|path| path.display().to_string())})
}

fn write_json(path: &Path, value: &serde_json::Value) {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, text));
    if let Err(error) = written {
        tracing::error!("could not write {}: {error}", path.display());
    }
}

fn file_safe(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' { ch } else { '-' })
        .collect()
}

/// The components under `runtime/base-component` and `runtime/component` with `test/agent/*.agent.js` files, their
/// specs by file name.
fn discover(runtime: &Path) -> Result<Vec<TestComponent>, Error> {
    let mut found = Vec::new();
    for parent in ["base-component", "component"] {
        let Ok(entries) = std::fs::read_dir(runtime.join(parent)) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|dir| dir.is_dir()).collect();
        dirs.sort();
        for dir in dirs {
            let agent_dir = dir.join("test/agent");
            let Ok(files) = std::fs::read_dir(&agent_dir) else {
                continue;
            };
            let name = super::component_name(&dir);
            let mut paths: Vec<PathBuf> = files
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.to_string_lossy().ends_with(".agent.js"))
                .collect();
            paths.sort();
            let mut specs = Vec::new();
            for path in paths {
                let file_name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
                let source = std::fs::read_to_string(&path)
                    .map_err(|err| Error::new(format!("could not read {}", path.display())).with_source(err))?;
                specs.push(SpecFile {
                    name: file_name.trim_end_matches(".agent.js").to_string(),
                    display: format!("{name}/test/agent/{file_name}"),
                    source,
                });
            }
            if !specs.is_empty() {
                found.push(TestComponent { name, dir, specs });
            }
        }
    }
    Ok(found)
}
