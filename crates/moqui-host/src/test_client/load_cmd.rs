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

//! `moqui test-load [component | test/load dir | file.load.js] [--script Name]` (plans/LoadRunner.md §9): a load
//! script of a component against a server that is already running, from this process, as `moqui test-client` runs a
//! component's client specs. With no script named it lists the component's scripts and their params. While a load
//! runs it prints a line every 5 seconds; Ctrl-C stops the users and still writes the result, by default to
//! `runtime/tmp/test-load/<runId>/result.json`, where Tools › Load › Runs finds it. `--compare` prints result files
//! side by side.
//!
//! A pure client, as `moqui test-client`: no database, conf, listener or log file.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_devkit::testkit::SpecHost;
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use serde_json::{json, Map, Value};

use super::http::Server;
use super::load;
use super::{camel_case, check_server, http_client, new_host, ClientArgs, DEFAULT_SERVER};
use crate::load_runner::results;
use crate::load_runner::scripts::scripts_in;
use crate::test_cmd::{component_dir_named, component_name};

/// How often a running load prints its line.
const PROGRESS_EVERY: Duration = Duration::from_secs(5);

/// A load's `--timeout` by default, in seconds: longer than a client spec's, since a server under load answers late.
const REQUEST_TIMEOUT: f64 = 60.0;

/// The Ctrl-Cs this process has had: the first stops the users, a second ends the process.
static INTERRUPTS: AtomicU32 = AtomicU32::new(0);

/// What `moqui test-load` was asked to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadArgs {
    /// A component name, a `test/load` directory or a `.load.js` file.
    pub target: Option<String>,
    /// `--script Name`: `test/load/Name.load.js` of the component.
    pub script: Option<String>,
    pub server: Option<String>,
    /// `--param name=value`, in order; a later one wins.
    pub params: Vec<(String, String)>,
    pub insecure: bool,
    pub allow_http: bool,
    pub out: Option<PathBuf>,
    pub log_level: Option<String>,
    /// `--timeout seconds`: what a request, a WebSocket receive and `waitUntil` wait at most ([`REQUEST_TIMEOUT`]
    /// by default).
    pub timeout: Option<f64>,
    pub users: Option<u64>,
    pub ramp_up: Option<f64>,
    pub duration: Option<f64>,
    pub iterations: Option<u64>,
    /// `--scenario name`: run only this scenario, and the settings above are its.
    pub scenario: Option<String>,
    pub label: Option<String>,
    /// `--compare a.json b.json ...`.
    pub compare: Vec<PathBuf>,
}

fn number_option(name: &str, text: &str) -> Result<f64, Error> {
    text.parse::<f64>()
        .ok()
        .filter(|number| number.is_finite() && *number >= 0.0)
        .ok_or_else(|| Error::new(format!("{name} is a number from 0, not '{text}'")))
}

fn whole_option(name: &str, text: &str) -> Result<u64, Error> {
    text.parse::<u64>()
        .ok()
        .filter(|number| *number >= 1)
        .ok_or_else(|| Error::new(format!("{name} is a whole number from 1, not '{text}'")))
}

/// Parse `moqui test-load`'s arguments.
///
/// # Errors
///
/// For an unknown option, an option without its value, or a value out of range.
pub fn parse_load_args(args: &[String]) -> Result<LoadArgs, Error> {
    let mut parsed = LoadArgs::default();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| Error::new(format!("{name} needs a value\nRun 'moqui help test-load' for details.")))
        };
        match arg.as_str() {
            "--script" => parsed.script = Some(value("--script")?),
            "--server" => parsed.server = Some(value("--server")?),
            "--param" => {
                let pair = value("--param")?;
                let (name, text) = pair
                    .split_once('=')
                    .filter(|(name, _)| !name.trim().is_empty())
                    .ok_or_else(|| Error::new(format!("--param needs name=value, not '{pair}'")))?;
                parsed.params.push((name.trim().to_string(), text.to_string()));
            }
            "--insecure" => parsed.insecure = true,
            "--allow-http" => parsed.allow_http = true,
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
            "--log-level" => {
                let level = value("--log-level")?;
                if !["trace", "debug", "info", "warn", "error"].contains(&level.to_ascii_lowercase().as_str()) {
                    return Err(Error::new(format!(
                        "--log-level is trace, debug, info, warn or error, not '{level}'"
                    )));
                }
                parsed.log_level = Some(level);
            }
            "--timeout" => {
                let seconds = number_option("--timeout", &value("--timeout")?)?;
                if seconds <= 0.0 {
                    return Err(Error::new("--timeout is more than 0 seconds"));
                }
                parsed.timeout = Some(seconds);
            }
            "--users" => parsed.users = Some(whole_option("--users", &value("--users")?)?),
            "--ramp-up" => parsed.ramp_up = Some(number_option("--ramp-up", &value("--ramp-up")?)?),
            "--duration" => {
                let seconds = number_option("--duration", &value("--duration")?)?;
                if seconds <= 0.0 {
                    return Err(Error::new("--duration is more than 0 seconds"));
                }
                parsed.duration = Some(seconds);
            }
            "--iterations" => parsed.iterations = Some(whole_option("--iterations", &value("--iterations")?)?),
            "--scenario" => parsed.scenario = Some(value("--scenario")?),
            "--label" => parsed.label = Some(value("--label")?),
            "--compare" => {
                // the files up to the next option
                while let Some(file) = iter.next_if(|next| !next.starts_with("--")) {
                    parsed.compare.push(PathBuf::from(file));
                }
                if parsed.compare.len() < 2 {
                    return Err(Error::new("--compare needs two or more result files"));
                }
            }
            other if other.starts_with("--") => {
                return Err(Error::new(format!(
                    "unknown option {other}\nRun 'moqui help test-load' for details."
                )));
            }
            target => {
                if parsed.target.is_some() {
                    return Err(Error::new(format!(
                        "one component, test/load directory or .load.js file, not also '{target}'"
                    )));
                }
                parsed.target = Some(target.to_string());
            }
        }
    }
    if !parsed.compare.is_empty() {
        if let Some(target) = &parsed.target {
            return Err(Error::new(format!(
                "--compare reads result files and runs nothing: leave out '{target}'"
            )));
        }
        return Ok(parsed);
    }
    if parsed.script.is_some()
        && parsed
            .target
            .as_deref()
            .is_some_and(|target| target.ends_with(".load.js"))
    {
        return Err(Error::new(
            "--script names a script of a component: leave it out with a .load.js file",
        ));
    }
    if parsed.target.is_none() {
        return Err(Error::new(
            "moqui test-load needs a component, a test/load directory or a .load.js file\nRun 'moqui help test-load' for details.",
        ));
    }
    Ok(parsed)
}

/// Run `moqui test-load` with console logging. An error, so the exit status is 1, when the run could not start, a
/// threshold was broken, or no request succeeded.
pub fn dispatch_test_load(args: &[String]) -> Result<(), Error> {
    let parsed = parse_load_args(args)?;
    let console = match parsed.log_level.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("info") => tracing::Level::INFO,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::WARN,
    };
    // Console only: no log file is attached, so `runtime/log/moqui.log` is never opened.
    crate::logging::install_with_console(Some(console));
    let out = run(&parsed, &mut |line| println!("{line}"));
    if INTERRUPTS.load(Ordering::Relaxed) > 0 {
        // stopped by Ctrl-C, after the result was written: the status a shell gives an interrupted command
        if let Err(err) = &out {
            eprintln!("{err}");
        }
        std::process::exit(130);
    }
    out
}

/// The component a target names, and the script file when it names one.
struct Target {
    component: String,
    dir: PathBuf,
    file: Option<PathBuf>,
}

/// The component directory of a path in it: the nearest directory above with a `component.xml`, else the one above
/// `test/load`.
fn component_of(path: &Path) -> PathBuf {
    if let Some(found) = path.ancestors().skip(1).find(|dir| dir.join("component.xml").is_file()) {
        return found.to_path_buf();
    }
    let parent = path.parent().unwrap_or(path);
    match (parent.file_name(), parent.parent()) {
        (Some(load), Some(test)) if load == "load" && test.file_name().is_some_and(|name| name == "test") => {
            test.parent().unwrap_or(test).to_path_buf()
        }
        _ => parent.to_path_buf(),
    }
}

fn resolve_target(runtime: &Path, target: &str) -> Result<Target, Error> {
    let path = Path::new(target);
    if target.ends_with(".load.js") {
        let file = std::fs::canonicalize(path).map_err(|err| Error::new(format!("{target}: {err}")))?;
        let dir = component_of(&file);
        return Ok(Target {
            component: component_name(&dir),
            dir,
            file: Some(file),
        });
    }
    if path.is_dir() {
        let dir = std::fs::canonicalize(path).map_err(|err| Error::new(format!("{target}: {err}")))?;
        let dir = if dir.ends_with("test/load") {
            dir.parent().and_then(Path::parent).unwrap_or(&dir).to_path_buf()
        } else {
            dir
        };
        return Ok(Target {
            component: component_name(&dir),
            dir,
            file: None,
        });
    }
    let dir = component_dir_named(runtime, target).ok_or_else(|| {
        Error::new(format!(
            "no component, test/load directory or .load.js file named {target} (components are looked for under {})",
            runtime.display()
        ))
    })?;
    Ok(Target {
        component: target.to_string(),
        dir,
        file: None,
    })
}

/// A script: its file, the name errors give, and its location for the run's record.
struct Chosen {
    script: load::LoadScript,
    location: String,
}

fn choose(target: &Target, script: Option<&str>) -> Result<Option<Chosen>, Error> {
    let found = scripts_in([(target.component.as_str(), target.dir.as_path())]);
    let path = match (&target.file, script) {
        (Some(file), _) => file.clone(),
        (None, None) => return Ok(None),
        (None, Some(name)) => {
            let name = name.strip_suffix(".load.js").unwrap_or(name);
            match found.iter().find(|known| known.name == name) {
                Some(known) => known.path.clone(),
                None => {
                    let names: Vec<&str> = found.iter().map(|known| known.name.as_str()).collect();
                    return Err(Error::new(format!(
                        "{} has no load script {name} (test/load/{name}.load.js){}",
                        target.component,
                        if names.is_empty() {
                            String::new()
                        } else {
                            format!("; it has {}", names.join(", "))
                        }
                    )));
                }
            }
        }
    };
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let in_component = found
        .iter()
        .find(|known| std::fs::canonicalize(&known.path).is_ok_and(|known| known == canonical));
    let (location, display) = match in_component {
        Some(known) => (
            known.location.clone(),
            format!("{}/test/load/{}", known.component, known.file_name),
        ),
        None => (path.display().to_string(), path.display().to_string()),
    };
    let script = load::LoadScript::from_file(&path, &display).map_err(Error::new)?;
    Ok(Some(Chosen { script, location }))
}

/// The `MOQUI_TEST_<NAME>` variables as params (`MOQUI_TEST_PASSWORD` is `password`): they fill the declared params
/// that `--param` leaves out, so a password need not show in `ps`.
fn env_params() -> Value {
    let found: Map<String, Value> = std::env::vars()
        .filter_map(|(name, value)| {
            let rest = name.strip_prefix("MOQUI_TEST_")?;
            (!rest.is_empty() && rest != "SERVER").then(|| (camel_case(rest), Value::String(value)))
        })
        .collect();
    Value::Object(found)
}

/// Run what `args` asks, each line of output to `print`. Logging is the caller's.
///
/// # Errors
///
/// When a target, script or server is wrong, the run cannot start, a threshold is broken, or no request succeeded.
pub fn run(args: &LoadArgs, print: &mut dyn FnMut(&str)) -> Result<(), Error> {
    if !args.compare.is_empty() {
        return compare(&args.compare, print);
    }
    let (runtime, _framework) = crate::runtime_framework_dirs();
    let target = resolve_target(&runtime, args.target.as_deref().unwrap_or_default())?;
    let server_text = args
        .server
        .clone()
        .or_else(|| {
            std::env::var("MOQUI_TEST_SERVER")
                .ok()
                .filter(|text| !text.trim().is_empty())
        })
        .unwrap_or_else(|| DEFAULT_SERVER.to_string());
    let server = Server::parse(&server_text, args.allow_http)?;
    let client_args = ClientArgs {
        insecure: args.insecure,
        allow_http: args.allow_http,
        timeout: args.timeout.unwrap_or(REQUEST_TIMEOUT),
        request_timeout: Some(args.timeout.unwrap_or(REQUEST_TIMEOUT)),
        ..ClientArgs::default()
    };
    let client = http_client(args.insecure)?;
    // the plan's host writes nothing: the run's, made once its checks pass, has the run's directory
    let reader = new_host(&client_args, server.clone(), client.clone(), &runtime.join("tmp"));
    *lock(&reader.component_dir) = target.dir.clone();
    let reader: Arc<dyn SpecHost> = reader;
    let Some(chosen) = choose(&target, args.script.as_deref())? else {
        list(&reader, &target, print);
        return Ok(());
    };
    let raw: Map<String, Value> = args
        .params
        .iter()
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect();
    let plan =
        load::script::read_plan(&reader, &chosen.script, &Value::Object(raw), &env_params()).map_err(Error::new)?;
    let uses = plan.uses();
    if uses.iter().any(|kind| kind == "service") {
        return Err(Error::new(format!(
            "{} uses service: its calls run in a server's own process, so run it from Tools › Load, not moqui test-load",
            chosen.location
        )));
    }
    if !uses.iter().any(|kind| kind == "http") {
        return Err(Error::new(format!(
            "{} uses {}, which moqui test-load cannot make (it makes http)",
            chosen.location,
            uses.join(", ")
        )));
    }
    let scenarios = load::script::settings(&plan, &overrides(args)).map_err(Error::new)?;
    if args.insecure {
        tracing::warn!("--insecure: the certificate of {} is not checked", server.url);
    }
    check_server(&client, &server, None)?;
    let out = out_location(&runtime, args.out.as_deref())?;
    let host = new_host(&client_args, server.clone(), client, &out.dir);
    *lock(&host.component_dir) = target.dir.clone();
    let host_dyn: Arc<dyn SpecHost> = host;
    let mut run = Map::new();
    run.insert("script".into(), json!(chosen.location));
    run.insert("label".into(), json!(args.label));
    run.insert("target".into(), json!(server.url));
    run.insert("params".into(), plan.public_params());
    run.insert("generator".into(), json!("same-machine"));
    if let Some(id) = &out.run_id {
        run.insert("runId".into(), json!(id));
    }
    let start = load::Start {
        scenarios,
        source: load::Source::Script {
            script: Arc::new(chosen.script),
            params: plan.params.clone(),
        },
        timeout: None,
        thresholds: plan.thresholds().clone(),
        run,
        generator: "same-machine",
        around: None,
        stop_after: None,
        stop_now_after: None,
    };
    let users: u64 = start.scenarios.iter().map(|scenario| scenario.users).sum();
    print(&format!(
        "{} against {}: {users} users, result in {}",
        chosen.location,
        server.url,
        out.file.display()
    ));
    watch_interrupts();
    let running = match load::start(&host_dyn, start) {
        Ok(running) => running,
        Err(err) => {
            // a run that did not start leaves no run directory for Tools › Load to list
            if out.run_id.is_some() {
                let _ = std::fs::remove_dir(&out.dir);
            }
            return Err(Error::new(err));
        }
    };
    let (stats, order) = wait_printing(running, print)?;
    let text = serde_json::to_string_pretty(&stats).map_err(|err| Error::new(err.to_string()))?;
    let partial = out.file.with_extension("json.partial");
    std::fs::write(&partial, text).map_err(|err| Error::new(format!("{}: {err}", partial.display())))?;
    std::fs::rename(&partial, &out.file).map_err(|err| Error::new(format!("{}: {err}", out.file.display())))?;
    for line in load::table(&stats, &order).trim_end().lines() {
        print(line);
    }
    print(&format!("Result: {}", out.file.display()));
    verdict(&stats)
}

/// The settings `--users`, `--ramp-up`, `--duration` and `--iterations` give: every scenario's, or with `--scenario`
/// that one's alone, which then is the only one run.
fn overrides(args: &LoadArgs) -> Value {
    let mut settings = Map::new();
    if let Some(users) = args.users {
        settings.insert("users".into(), json!(users));
    }
    if let Some(seconds) = args.ramp_up {
        settings.insert("rampUp".into(), json!(seconds));
    }
    if let Some(seconds) = args.duration {
        settings.insert("duration".into(), json!(seconds));
    }
    if let Some(count) = args.iterations {
        settings.insert("iterations".into(), json!(count));
    }
    match &args.scenario {
        Some(name) => json!({ "scenarios": { name.clone(): settings } }),
        None => Value::Object(settings),
    }
}

/// Exit 1 when a threshold was broken or no request succeeded.
fn verdict(stats: &Value) -> Result<(), Error> {
    let succeeded: u64 = stats
        .get("names")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, entry)| entry.get("kind").and_then(Value::as_str) != Some("step"))
        .map(|(_, entry)| {
            let count = entry.get("count").and_then(Value::as_u64).unwrap_or(0);
            count.saturating_sub(entry.get("errors").and_then(Value::as_u64).unwrap_or(0))
        })
        .sum();
    if succeeded == 0 {
        return Err(Error::new("no request succeeded"));
    }
    if stats.get("passed") == Some(&json!(false)) {
        return Err(Error::new("a threshold was broken"));
    }
    Ok(())
}

/// Where a run's result goes: `<--out>/result.json`, else a run directory of its own under `runtime/tmp/test-load`
/// (or `test-load-out` in the working directory without a runtime), named as a run in the server is.
struct Out {
    dir: PathBuf,
    file: PathBuf,
    run_id: Option<String>,
}

fn out_location(runtime: &Path, out: Option<&Path>) -> Result<Out, Error> {
    if let Some(dir) = out {
        std::fs::create_dir_all(dir).map_err(|err| Error::new(format!("--out {}: {err}", dir.display())))?;
        return Ok(Out {
            dir: dir.to_path_buf(),
            file: dir.join(results::RESULT_FILE),
            run_id: None,
        });
    }
    let root = if runtime.join("base-component").is_dir() {
        runtime.join("tmp/test-load")
    } else {
        PathBuf::from("test-load-out")
    };
    let base = results::run_id_base(jiff::Timestamp::now(), std::process::id());
    let (id, dir) = results::create_run_dir(&root, &base).map_err(Error::new)?;
    Ok(Out {
        file: dir.join(results::RESULT_FILE),
        dir,
        run_id: Some(id),
    })
}

/// Ctrl-C: the first stops the users before their next request, a second ends the process.
fn watch_interrupts() {
    static WATCHING: std::sync::Once = std::sync::Once::new();
    WATCHING.call_once(|| {
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::warn!("Not watching for Ctrl-C: {err}");
                return;
            }
        };
        let started = std::thread::Builder::new()
            .name("moqui-test-load-signals".to_string())
            .spawn(move || {
                runtime.block_on(async {
                    while tokio::signal::ctrl_c().await.is_ok() {
                        if INTERRUPTS.fetch_add(1, Ordering::Relaxed) >= 1 {
                            eprintln!("Interrupted again: ending without a result");
                            std::process::exit(130);
                        }
                    }
                });
            });
        if let Err(err) = started {
            tracing::warn!("Not watching for Ctrl-C: {err}");
        }
    });
}

/// Wait for the run on a thread of its own, printing its line every 5 seconds and stopping its users on Ctrl-C.
fn wait_printing(running: load::Running, print: &mut dyn FnMut(&str)) -> Result<(Value, Vec<String>), Error> {
    let load = Arc::clone(running.load());
    let waiting = std::thread::Builder::new()
        .name("moqui-test-load-wait".to_string())
        .spawn(move || running.wait())
        .map_err(|err| Error::new(format!("the load's thread did not start: {err}")))?;
    let started = Instant::now();
    let mut next = started + PROGRESS_EVERY;
    let mut last = (started, 0u64);
    let mut stopping = false;
    while !waiting.is_finished() {
        std::thread::sleep(Duration::from_millis(100));
        if !stopping && INTERRUPTS.load(Ordering::Relaxed) > 0 {
            stopping = true;
            load.stop_now();
            print("Stopping the users (Ctrl-C again ends at once, with no result)");
        }
        let now = Instant::now();
        if now >= next {
            next = now + PROGRESS_EVERY;
            let snapshot = load.snapshot();
            let requests = snapshot.get("requests").and_then(Value::as_u64).unwrap_or(0);
            let seconds = now.duration_since(last.0).as_secs_f64();
            let rate = if seconds > 0.0 {
                requests.saturating_sub(last.1) as f64 / seconds
            } else {
                0.0
            };
            last = (now, requests);
            print(&progress_line(&snapshot, rate));
        }
    }
    waiting.join().map_err(|_| Error::new("the load's thread panicked"))
}

/// `35s  10 users  241.2/s  0 errors  p95 38ms`: the time so far, the active users, the requests a second since the
/// last line, the errors so far and the p95 so far.
fn progress_line(snapshot: &Value, rate: f64) -> String {
    let number = |key: &str| snapshot.get(key).and_then(Value::as_u64).unwrap_or(0);
    let elapsed = snapshot.get("elapsed").and_then(Value::as_f64).unwrap_or(0.0);
    let p95 = snapshot
        .get("latency")
        .and_then(|latency| latency.get("p95"))
        .unwrap_or(&Value::Null);
    format!(
        "{:.0}s  {} users  {rate:.1}/s  {} errors  p95 {}",
        elapsed,
        number("activeUsers"),
        load::grouped(number("errors")),
        load::table_ms(p95)
    )
}

/// The scripts of the target's component, each with its title, scenarios and params; one whose plan does not read
/// is listed with its error.
fn list(host: &Arc<dyn SpecHost>, target: &Target, print: &mut dyn FnMut(&str)) {
    let found = scripts_in([(target.component.as_str(), target.dir.as_path())]);
    if found.is_empty() {
        print(&format!(
            "{} has no load scripts ({}/test/load/<Name>.load.js)",
            target.component,
            target.dir.display()
        ));
        return;
    }
    for known in found {
        print(&format!("{}  {}", known.name, known.location));
        let display = format!("{}/test/load/{}", known.component, known.file_name);
        let plan = load::LoadScript::from_file(&known.path, &display)
            .and_then(|script| load::script::read_plan_listed(host, &script, &env_params()));
        let plan = match plan {
            Ok(plan) => plan,
            Err(err) => {
                print(&format!("    does not read: {}", err.lines().next().unwrap_or("")));
                continue;
            }
        };
        let text = |key: &str| plan.plan.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        match (text("title"), text("description")) {
            (title, description) if !title.is_empty() && !description.is_empty() => {
                print(&format!("    {title}: {description}"));
            }
            (title, description) if !title.is_empty() || !description.is_empty() => {
                print(&format!("    {title}{description}"));
            }
            _ => {}
        }
        print(&format!("    uses: {}", plan.uses().join(", ")));
        for (name, defaults) in &plan.scenarios {
            let mut line = format!("    scenario {name}:");
            for (key, label) in [("users", "users"), ("iterations", "iterations"), ("duration", "s")] {
                if let Some(value) = defaults.get(key).filter(|value| !value.is_null()) {
                    let _ = write!(line, " {value} {label}");
                }
            }
            print(&line);
        }
        let params: BTreeMap<String, Value> = plan
            .plan
            .get("params")
            .and_then(Value::as_object)
            .map(|params| params.iter().map(|(name, def)| (name.clone(), def.clone())).collect())
            .unwrap_or_default();
        for (name, def) in params {
            let shown = if def.get("secret").and_then(Value::as_bool).unwrap_or(false) {
                "(secret)".to_string()
            } else {
                match def.get("default") {
                    Some(Value::String(text)) => text.clone(),
                    Some(Value::Null) | None => String::new(),
                    Some(other) => other.to_string(),
                }
            };
            let description = def.get("description").and_then(Value::as_str).unwrap_or("");
            let required = if def.get("required").and_then(Value::as_bool).unwrap_or(false) {
                " (required)"
            } else {
                ""
            };
            let assigned = if shown.is_empty() {
                name.clone()
            } else {
                format!("{name}={shown}")
            };
            print(format!("    --param {assigned}{required}  {description}").trim_end());
        }
    }
}

/// A result's value at `path` (`latency.p95`, `names.<name>.count`), as a number.
fn at(result: &Value, path: &[&str]) -> Option<f64> {
    path.iter()
        .try_fold(result, |value, key| value.get(*key))
        .and_then(Value::as_f64)
}

fn shown(value: Option<f64>) -> String {
    match value {
        None => "-".to_string(),
        Some(value) if value.fract() == 0.0 && value.abs() < 1e15 => load::grouped(value as u64),
        Some(value) => format!("{value:.3}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string(),
    }
}

/// `--compare`: the results side by side, each value after the first with its change from the first in percent.
fn compare(files: &[PathBuf], print: &mut dyn FnMut(&str)) -> Result<(), Error> {
    let mut loaded = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file).map_err(|err| Error::new(format!("{}: {err}", file.display())))?;
        let value: Value =
            serde_json::from_str(&text).map_err(|err| Error::new(format!("{}: {err}", file.display())))?;
        if !results::is_result(&value) {
            return Err(Error::new(format!("{} is not a load result", file.display())));
        }
        loaded.push(value);
    }
    for (index, (file, result)) in files.iter().zip(&loaded).enumerate() {
        let run = result.get("run").unwrap_or(&Value::Null);
        let label = run.get("label").and_then(Value::as_str).unwrap_or("");
        let target = run.get("target").and_then(Value::as_str).unwrap_or("");
        print(format!("[{}] {}  {target}  {label}", index + 1, file.display()).trim_end());
    }
    let mut rows: Vec<(String, Vec<&str>)> = [
        ("requests", vec!["requests"]),
        ("per second", vec!["rps"]),
        ("errors", vec!["errors"]),
        ("error rate", vec!["errorRate"]),
        ("p50 ms", vec!["latency", "p50"]),
        ("p95 ms", vec!["latency", "p95"]),
        ("p99 ms", vec!["latency", "p99"]),
        ("max ms", vec!["latency", "max"]),
    ]
    .into_iter()
    .map(|(label, path)| (label.to_string(), path))
    .collect();
    // the names in each result's own order (nameOrder: a JSON object keeps none), the first result's first
    let mut names: Vec<String> = Vec::new();
    for result in &loaded {
        let ordered = result
            .get("nameOrder")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        let rest = result
            .get("names")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(name, _)| name.as_str());
        for name in ordered.chain(rest) {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
    }
    let mut table: Vec<(String, Vec<Option<f64>>)> = Vec::new();
    for (label, path) in rows.drain(..) {
        table.push((label, loaded.iter().map(|result| at(result, &path)).collect()));
    }
    for name in &names {
        for (metric, label) in [
            ("count", "count"),
            ("p50", "p50 ms"),
            ("p95", "p95 ms"),
            ("p99", "p99 ms"),
            ("errors", "errors"),
        ] {
            let values = loaded
                .iter()
                .map(|result| at(result, &["names", name, metric]))
                .collect();
            table.push((format!("{name} {label}"), values));
        }
    }
    let width = table.iter().map(|(label, _)| label.chars().count()).max().unwrap_or(0);
    let mut header = format!("{:<width$}", "");
    for index in 0..loaded.len() {
        let _ = write!(header, "  {:>22}", format!("[{}]", index + 1));
    }
    print(header.trim_end());
    for (label, values) in table {
        let mut line = format!("{label:<width$}");
        let first = values.first().copied().flatten();
        for (index, value) in values.iter().enumerate() {
            let mut cell = shown(*value);
            if index > 0 {
                if let (Some(first), Some(value)) = (first, value) {
                    if first != 0.0 {
                        let _ = write!(cell, " ({:+.1}%)", (value - first) / first * 100.0);
                    }
                }
            }
            let _ = write!(line, "  {cell:>22}");
        }
        print(line.trim_end());
    }
    Ok(())
}

#[cfg(test)]
#[path = "load_cmd_tests.rs"]
mod tests;
