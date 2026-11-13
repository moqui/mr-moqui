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

//! `moqui test-client [component | path ...]` (plans/archive/ClientTestHarness.md): run the spec files of a component's
//! `test/client` directory against a server that is already running, over HTTP, WebSocket and headless Chromium
//! ([`browser`]). The specs have the
//! format, the runner and the report of `moqui test`; they run under [`ClientHost`], a [`SpecHost`] with no execution
//! context.
//!
//! A pure client: no execution context factory, database, listener, conf, `runtime/.env`, `runtime/log/moqui.log` or
//! service workers. It reads the spec files and its components' files, and writes only under `--out` and `--junit`,
//! so it runs beside a server in the same runtime directory, or from another machine.

mod browser;
pub mod cdp;
mod http;
pub(crate) mod load;
mod load_cmd;
mod offline;
mod raw;
mod serve;
#[cfg(feature = "ssh")]
mod ssh;
#[cfg(not(feature = "ssh"))]
#[path = "ssh_off.rs"]
mod ssh;
mod video;
mod ws;

pub use load_cmd::{dispatch_test_load, parse_load_args, LoadArgs};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use moqui_core::service::{hmac_base64, timestamp_header};
use moqui_devkit::testkit::{
    run_on_thread, run_spec, spec_missing_features, spec_offline, CaseExtras, RunOptions, SpecHost,
};
use moqui_lib::error::Error;
use moqui_lib::sync::lock;
use serde_json::{json, Value};

use crate::test_cmd::{
    acceptance_only, component_name, component_specs, discover, discover_suite, report, run_order, select_across,
    select_specs, skipped_spec, suite_tree, TestComponent,
};
use http::{Ask, Server, Session, SshTarget};

const PRELUDE: &str = include_str!("prelude.js");

/// The server when neither `--server` nor `MOQUI_TEST_SERVER` names one.
const DEFAULT_SERVER: &str = "http://localhost:8080";

/// A request's limit when its own `timeout` does not set one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The response bodies kept for `save`, the latest.
const KEPT_BODIES: usize = 32;

/// What `moqui test-client` was asked to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClientArgs {
    /// Component names, `test/client` directories or `.spec.js` files.
    pub targets: Vec<String>,
    pub server: Option<String>,
    /// `--ssh host:port` (or a port, on the server's host): where `ssh.connect` goes by default; else
    /// `MOQUI_TEST_SSH`.
    pub ssh: Option<String>,
    pub specs: Vec<String>,
    pub feature: Option<String>,
    /// `--acceptance`: run the acceptance client specs (`acceptance/client/`) in place of `test/client/`; with
    /// `--serve`, against the server `acceptance/client/suite.js` declares, whose specs resolve across both trees.
    pub acceptance: bool,
    /// `--param name=value`, in order; a later one wins.
    pub params: Vec<(String, String)>,
    pub junit: Option<PathBuf>,
    pub out: Option<PathBuf>,
    /// `--wait seconds`: poll until the server answers.
    pub wait: Option<f64>,
    /// `--timeout seconds`: what `waitUntil` and a socket wait by default.
    pub timeout: f64,
    pub insecure: bool,
    pub allow_http: bool,
    pub fail_on_skip: bool,
    pub log_level: Option<String>,
    /// `--serve`: start each suite's declared server as a child `moqui test ... --serve` and run against it.
    pub serve: bool,
    /// `--chromium path`: the browser, else `moqui_chromium_path`, else the first found ([`crate::chromium::find_test_browser`]).
    pub chromium: Option<String>,
    /// `--headed`: a visible browser, for debugging.
    pub headed: bool,
    /// `--offline`: run only specs with `server: false`, and do not check or start a server.
    pub offline: bool,
    /// `--video`: record each spec's pages into `<out>/video/<Spec>.webm`, with `<Spec>.timeline.json`.
    pub video: bool,
    /// A request's limit when its own `timeout` does not set one, in seconds: 60 when not given (`moqui test-load
    /// --timeout`).
    pub request_timeout: Option<f64>,
}

fn seconds_option(name: &str, text: &str) -> Result<f64, Error> {
    text.parse::<f64>()
        .ok()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .ok_or_else(|| Error::new(format!("{name} is a number of seconds, not '{text}'")))
}

pub fn parse_client_args(args: &[String]) -> Result<ClientArgs, Error> {
    let mut parsed = ClientArgs {
        timeout: 10.0,
        ..ClientArgs::default()
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next().cloned().ok_or_else(|| {
                Error::new(format!(
                    "{name} needs a value\nRun 'moqui help test-client' for details."
                ))
            })
        };
        match arg.as_str() {
            "--server" => parsed.server = Some(value("--server")?),
            "--ssh" => parsed.ssh = Some(value("--ssh")?),
            "--spec" => parsed.specs.extend(
                value("--spec")?
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            ),
            "--feature" => parsed.feature = Some(value("--feature")?),
            "--acceptance" => parsed.acceptance = true,
            "--param" => {
                let pair = value("--param")?;
                let (name, text) = pair
                    .split_once('=')
                    .filter(|(name, _)| !name.trim().is_empty())
                    .ok_or_else(|| Error::new(format!("--param needs name=value, not '{pair}'")))?;
                parsed.params.push((name.trim().to_string(), text.to_string()));
            }
            "--junit" => parsed.junit = Some(PathBuf::from(value("--junit")?)),
            "--out" => parsed.out = Some(PathBuf::from(value("--out")?)),
            "--wait" => parsed.wait = Some(seconds_option("--wait", &value("--wait")?)?),
            "--timeout" => {
                parsed.timeout = seconds_option("--timeout", &value("--timeout")?)?;
                if parsed.timeout <= 0.0 {
                    return Err(Error::new("--timeout is more than 0 seconds"));
                }
            }
            "--log-level" => parsed.log_level = Some(value("--log-level")?),
            "--insecure" => parsed.insecure = true,
            "--allow-http" => parsed.allow_http = true,
            "--fail-on-skip" => parsed.fail_on_skip = true,
            "--serve" => parsed.serve = true,
            "--chromium" => parsed.chromium = Some(value("--chromium")?),
            "--headed" => parsed.headed = true,
            "--offline" => parsed.offline = true,
            "--video" => parsed.video = true,
            other if other.starts_with("--") => {
                return Err(Error::new(format!(
                    "unknown option {other}\nRun 'moqui help test-client' for details."
                )));
            }
            target => parsed.targets.push(target.to_string()),
        }
    }
    if parsed.serve && parsed.offline {
        return Err(Error::new(
            "--offline runs specs with server: false and does not start a server: leave out --serve",
        ));
    }
    if parsed.serve && (parsed.server.is_some() || parsed.wait.is_some() || parsed.ssh.is_some()) {
        return Err(Error::new(
            "--serve starts the servers the suites declare: leave out --server, --ssh and --wait",
        ));
    }
    Ok(parsed)
}

/// Run `moqui test-client` with console logging. An error when the server does not answer or anything failed, so
/// the exit status is 1.
pub fn dispatch_test_client(args: &[String]) -> Result<(), Error> {
    let parsed = parse_client_args(args)?;
    let console = match parsed.log_level.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("info") => tracing::Level::INFO,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::WARN,
    };
    // Console only: no log file is attached, so `runtime/log/moqui.log` is never opened.
    crate::logging::install_with_console(Some(console));
    run(&parsed, &mut |line| println!("{line}"))
}

/// Run the client specs `args` selects, each line of the report to `print`. Logging is the caller's.
///
/// # Errors
///
/// Returns an error for a bad target or server, when nothing answers at the server, or when a spec or feature failed
/// (or, with `--fail-on-skip`, was skipped).
pub fn run(args: &ClientArgs, print: &mut dyn FnMut(&str)) -> Result<(), Error> {
    let (runtime, _framework) = crate::runtime_framework_dirs();
    let server = if args.serve {
        None
    } else {
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
        let ssh = ssh_target(args.ssh.as_deref(), &server)?;
        Some(server.with_ssh(ssh))
    };
    let components = select(&runtime, args)?;
    // a cycle in the dependsOn of the run is an error before any server starts
    run_order(&components, args.acceptance, true)?;
    let out_dir = args.out.clone().unwrap_or_else(|| default_out_dir(&runtime));
    let started = Instant::now();
    let results = match server {
        Some(server) => run_against(args, server, args.wait, &components, &out_dir, print)?,
        None => serve::run_served(args, &components, &out_dir, print),
    };
    if let Some(dir) = &args.junit {
        report::write_junit(dir, &results, ".client")?;
    }
    let (text, mut failed) = report::summary(&results, started.elapsed());
    for line in text.trim_end_matches('\n').split('\n') {
        print(line);
    }
    // the regression budget of the client specs (test/budget's client line); not the acceptance suite
    if !args.acceptance {
        let (text, over) = report::budget_check(&results, &|name| {
            crate::test_cmd::component_dir_named(&runtime, name).and_then(|dir| report::read_budget(&dir, "client"))
        });
        for line in text
            .trim_start_matches('\n')
            .trim_end_matches('\n')
            .split('\n')
            .filter(|line| !line.is_empty())
        {
            print(line);
        }
        failed += over;
    }
    if out_dir.is_dir() {
        print(&format!("Output: {}", out_dir.display()));
    }
    if failed > 0 {
        return Err(Error::new(format!("{failed} test failure(s)")));
    }
    let skipped = results
        .iter()
        .flat_map(|spec| &spec.result.cases)
        .filter(|case| matches!(case.outcome, moqui_devkit::testkit::Outcome::Skipped(_)))
        .count();
    if args.fail_on_skip && skipped > 0 {
        return Err(Error::new(format!("{skipped} skipped, and --fail-on-skip")));
    }
    Ok(())
}

/// The HTTP client of a run: redirects are the session's to follow, and `insecure` accepts any certificate.
fn http_client(insecure: bool) -> Result<reqwest::blocking::Client, Error> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(insecure)
        .user_agent(concat!("moqui-test-client/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| Error::new(format!("HTTP client: {err}")))
}

/// The client host of a run of `args` against `server`, writing under `out_dir`.
fn new_host(args: &ClientArgs, server: Server, client: reqwest::blocking::Client, out_dir: &Path) -> Arc<ClientHost> {
    Arc::new_cyclic(|me| ClientHost {
        me: me.clone(),
        server,
        params: params(args),
        client,
        insecure: args.insecure,
        timeout: load::seconds(args.timeout),
        request_timeout: args.request_timeout.map_or(REQUEST_TIMEOUT, load::seconds),
        out_dir: out_dir.to_path_buf(),
        component_dir: Mutex::new(PathBuf::new()),
        next_id: AtomicU64::new(1),
        sessions: Mutex::new(HashMap::new()),
        sockets: Mutex::new(HashMap::new()),
        ssh: ssh::Ssh::new(),
        bodies: Mutex::new(VecDeque::new()),
        current: Mutex::new((String::new(), String::new())),
        output: Mutex::new(Vec::new()),
        browser_config: browser::BrowserConfig {
            chromium: args.chromium.clone(),
            headed: args.headed,
            insecure: args.insecure,
            runtime: Some(crate::runtime_framework_dirs().0),
        },
        browser: Mutex::new(None),
        offline: Mutex::new(false),
        video: args.video,
        recording: Arc::new(Mutex::new(None)),
        in_server: None,
    })
}

/// Run the specs of `components` against `server`, once it answers (within `wait` seconds when given), each line of
/// the report to `print`. The specs run in the order of `moqui test` ([`run_order`]): the components' order and each
/// one's own, except that a spec runs after the specs its `dependsOn` names, in any of the components.
fn run_against(
    args: &ClientArgs,
    server: Server,
    wait: Option<f64>,
    components: &[TestComponent],
    out_dir: &Path,
    print: &mut dyn FnMut(&str),
) -> Result<Vec<report::ComponentSpec>, Error> {
    if args.insecure {
        tracing::warn!("--insecure: the certificate of {} is not checked", server.url);
    }
    let client = http_client(args.insecure)?;
    let all_offline = components
        .iter()
        .all(|component| component.specs.iter().all(|spec| spec_offline(&spec.source)));
    if !all_offline {
        check_server(&client, &server, wait)?;
    }
    let encode_video = args.video && video_tools(args.chromium.as_deref());
    let host = new_host(args, server, client, out_dir);
    let host_dyn: Arc<dyn SpecHost> = host.clone();
    let options = RunOptions {
        feature_filter: args.feature.clone(),
        host: Some(host_dyn),
    };
    let mut results = Vec::new();
    for (c, s) in run_order(components, args.acceptance, true)? {
        let component = &components[c];
        let spec = &component.specs[s];
        *lock(&host.component_dir) = component.dir.clone();
        *lock(&host.current) = (component.name.clone(), spec.name.clone());
        *lock(&host.offline) = spec_offline(&spec.source);
        if args.video {
            *lock(&host.recording) = Some(video::Recorder::new(&spec.name, out_dir, encode_video));
        }
        let note_case = |case: &moqui_devkit::testkit::CaseResult| {
            if let Some(recorder) = lock(&host.recording).as_mut() {
                recorder.case(case);
            }
        };
        let missing = spec_missing_features(&spec.source);
        let not_run = if !missing.is_empty() {
            Some(skipped_spec(spec, &missing))
        } else {
            None
        };
        let result = if let Some(result) = not_run {
            for case in &result.cases {
                note_case(case);
                print(&report::case_line(&spec.name, case));
            }
            result
        } else {
            let result = run_spec(spec, &options, &mut |case| {
                note_case(case);
                print(&report::case_line(&spec.name, case));
                for line in report::output_lines(case) {
                    print(&line);
                }
            });
            if let Some(failure) = &result.setup_failure {
                print(&format!(
                    "{} setup FAILED: {}",
                    spec.name,
                    failure.message.lines().next().unwrap_or("")
                ));
            }
            result
        };
        let recorder = lock(&host.recording).take();
        if let Some(path) = recorder.and_then(video::Recorder::finish) {
            print(&format!("{} video: {}", spec.name, path.display()));
        }
        results.push(report::ComponentSpec {
            component: component.name.clone(),
            result,
        });
    }
    Ok(results)
}

/// With `--video`: whether the video can be encoded, that is Chromium and ffmpeg are found. When one is not, an error
/// names it, and the specs still run with only their timelines written.
fn video_tools(chromium: Option<&str>) -> bool {
    let runtime = crate::runtime_framework_dirs().0;
    let mut found = true;
    if let Err(reason) = crate::chromium::find_test_browser(Some(&runtime), chromium.unwrap_or("")) {
        tracing::error!("--video: no Chromium ({reason}); the specs run and no video is written");
        found = false;
    }
    if let Err(reason) = video::check_ffmpeg() {
        tracing::error!("--video: {reason}; the specs run and no video is written");
        found = false;
    }
    found
}

/// The components and specs to run: those named (a component, a `test/client` directory or a spec file), or every
/// component with client specs; then `--spec` with the specs their `dependsOn` names, where a process spec
/// (`test/client/process/*.spec.js`) can be named too.
fn select(runtime: &Path, args: &ClientArgs) -> Result<Vec<TestComponent>, Error> {
    let add = |component: TestComponent, components: &mut Vec<TestComponent>| match components
        .iter_mut()
        .find(|known| known.name == component.name)
    {
        Some(known) => {
            for spec in component.specs {
                if !known.specs.iter().any(|have| have.name == spec.name) {
                    known.specs.push(spec);
                }
            }
        }
        None => components.push(component),
    };
    let tree = suite_tree(args.acceptance, true);
    let mut discovered = if args.targets.iter().any(|target| !is_path(target)) || args.targets.is_empty() {
        let pool = discover_suite(runtime, true, args.acceptance)?;
        if args.acceptance && args.specs.is_empty() {
            acceptance_only(pool)
        } else {
            pool
        }
    } else {
        Vec::new()
    };
    // Process specs (<tree>/process/<businessProcessId>.spec.js) run only when --spec names them
    if !args.specs.is_empty() && (args.targets.iter().any(|target| !is_path(target)) || args.targets.is_empty()) {
        for component in discover(runtime, &format!("{tree}/process"))? {
            add(component, &mut discovered);
        }
    }
    let mut components: Vec<TestComponent> = Vec::new();
    if args.targets.is_empty() {
        components = discovered;
    } else {
        for target in &args.targets {
            if is_path(target) {
                add(path_component(Path::new(target), args.acceptance)?, &mut components);
            } else {
                let component = discovered
                    .iter()
                    .find(|component| &component.name == target)
                    .cloned()
                    .ok_or_else(|| Error::new(format!("component {target} has no {tree}/*.spec.js files")))?;
                add(component, &mut components);
            }
        }
    }
    order_by_depends_on(&mut components);
    if !args.specs.is_empty() {
        let picked = select_across(&components, &args.specs)?;
        for name in &args.specs {
            if !picked
                .iter()
                .any(|component| component.specs.iter().any(|spec| &spec.name == name))
            {
                return Err(Error::new(format!("no component has a client spec {name}")));
            }
        }
        components = picked;
    }
    if args.offline {
        for component in &mut components {
            component.specs.retain(|spec| spec_offline(&spec.source));
        }
        components.retain(|component| !component.specs.is_empty());
    }
    if components.is_empty() {
        return Err(Error::new(if args.offline {
            "no client spec has server: false".to_string()
        } else {
            format!(
                "no {tree}/*.spec.js files in the components of {}",
                crate::test_cmd::set_dirs_text(runtime)
            )
        }));
    }
    Ok(components)
}

fn is_path(target: &str) -> bool {
    target.contains('/') || target.ends_with(".spec.js") || target == "." || target == ".."
}

/// The component of a client directory (`test/client` or `acceptance/client`), a component directory (its
/// `acceptance/client` with `acceptance`, else its `test/client`), or one spec file in a client directory (or a
/// subdirectory of it) or its `process` directory: the directory above `test` or `acceptance`. A file brings its own
/// spec only, with what its `dependsOn` names.
fn path_component(path: &Path, acceptance: bool) -> Result<TestComponent, Error> {
    let missing = || {
        Error::new(format!(
            "{} is not a test/client or acceptance/client directory or a client .spec.js file",
            path.display()
        ))
    };
    let is_client_dir = |dir: &Path| dir.ends_with("test/client") || dir.ends_with("acceptance/client");
    let path = std::fs::canonicalize(path).map_err(|_| missing())?;
    let process_dir = path
        .parent()
        .filter(|parent| {
            path.is_file() && (parent.ends_with("test/client/process") || parent.ends_with("acceptance/client/process"))
        })
        .map(Path::to_path_buf);
    let own_tree = suite_tree(acceptance, true);
    let (client_dir, file) = if let Some(process_dir) = &process_dir {
        (
            process_dir.parent().ok_or_else(missing)?.to_path_buf(),
            Some(path.clone()),
        )
    } else if path.is_file() {
        // in a client directory or a subdirectory of it
        let client_dir = path
            .ancestors()
            .skip(1)
            .find(|dir| is_client_dir(dir))
            .ok_or_else(missing)?;
        (client_dir.to_path_buf(), Some(path.clone()))
    } else if path.join(own_tree).is_dir() {
        (path.join(own_tree), None)
    } else {
        (path.clone(), None)
    };
    if !is_client_dir(&client_dir) {
        return Err(missing());
    }
    let tree = if client_dir.ends_with("acceptance/client") {
        "acceptance/client"
    } else {
        "test/client"
    };
    let dir = client_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(missing)?
        .to_path_buf();
    let name = component_name(&dir);
    let mut component = TestComponent {
        specs: component_specs(&name, &client_dir, tree)?,
        name,
        dir,
    };
    // An acceptance spec may depend on a regression client spec of its component.
    if tree == "acceptance/client" && component.dir.join("test/client").is_dir() {
        let mut pool = component_specs(&component.name, &component.dir.join("test/client"), "test/client")?;
        pool.retain(|spec| !component.specs.iter().any(|have| have.name == spec.name));
        pool.append(&mut component.specs);
        component.specs = pool;
        if file.is_none() {
            component.specs.retain(crate::test_cmd::is_acceptance);
        }
    }
    if let Some(process_dir) = &process_dir {
        for spec in component_specs(&component.name, process_dir, &format!("{tree}/process"))? {
            if !component.specs.iter().any(|have| have.name == spec.name) {
                component.specs.push(spec);
            }
        }
    }
    if let Some(file) = file {
        let spec_name = file
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".spec.js"))
            .ok_or_else(missing)?
            .to_string();
        component.specs = select_specs(&component, &[spec_name])?;
    }
    Ok(component)
}

/// Each component after the components its `component.xml` `depends-on` names, otherwise in the order found.
fn order_by_depends_on(components: &mut Vec<TestComponent>) {
    let depends: Vec<Vec<String>> = components
        .iter()
        .map(|component| {
            moqui_lib::node::MNode::parse_file(component.dir.join("component.xml"))
                .map(|node| {
                    node.children_named("depends-on")
                        .iter()
                        .filter_map(|child| child.attribute("name"))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let names: Vec<String> = components.iter().map(|component| component.name.clone()).collect();
    let mut placed: Vec<usize> = Vec::new();
    while placed.len() < components.len() {
        let next = (0..components.len()).find(|index| {
            !placed.contains(index)
                && depends[*index].iter().all(|name| {
                    names
                        .iter()
                        .position(|known| known == name)
                        .is_none_or(|at| placed.contains(&at) || at == *index)
                })
        });
        // A cycle keeps the order found for what is left.
        let next = next.unwrap_or_else(|| {
            (0..components.len())
                .find(|index| !placed.contains(index))
                .unwrap_or_default()
        });
        placed.push(next);
    }
    let mut taken: Vec<Option<TestComponent>> = components.drain(..).map(Some).collect();
    components.extend(placed.into_iter().filter_map(|index| taken[index].take()));
}

/// `GET /` with redirects off until anything answers, for `--wait` seconds when given.
fn check_server(client: &reqwest::blocking::Client, server: &Server, wait: Option<f64>) -> Result<(), Error> {
    let deadline = Instant::now() + Duration::from_secs_f64(wait.unwrap_or(0.0));
    let url = server.join("/");
    loop {
        match client.get(&url).timeout(Duration::from_secs(10)).send() {
            Ok(_) => return Ok(()),
            Err(err) => {
                if Instant::now() >= deadline {
                    tracing::debug!("{url}: {}", http::error_chain(&err));
                    return Err(Error::new(format!(
                        "nothing answers at {}: start a server (./moq run), or pass --server or --serve",
                        server.url
                    )));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// `runtime/tmp/test-client/<yyyyMMdd-HHmmss>-<pid>`, or `test-client-out/<run>` in the working directory without a
/// runtime. Made when something is written to it.
fn default_out_dir(runtime: &Path) -> PathBuf {
    let run = format!(
        "{}-{}",
        jiff::Zoned::now().strftime("%Y%m%d-%H%M%S"),
        std::process::id()
    );
    if moqui_core::context::layout::is_runtime(runtime) {
        runtime.join("tmp/test-client").join(run)
    } else {
        PathBuf::from("test-client-out").join(run)
    }
}

/// The SSH listener of `--ssh`, else of `MOQUI_TEST_SSH`, on `server`'s host when it names only a port.
pub(crate) fn ssh_target(given: Option<&str>, server: &Server) -> Result<Option<SshTarget>, Error> {
    let text = given.map(str::to_string).or_else(|| {
        std::env::var("MOQUI_TEST_SSH")
            .ok()
            .filter(|text| !text.trim().is_empty())
    });
    text.map(|text| SshTarget::parse(&text, &server.host).map_err(Error::new))
        .transpose()
}

/// The `MOQUI_TEST_` variables that are options, not params.
pub(crate) const NOT_PARAMS: [&str; 2] = ["SERVER", "SSH"];

/// The `MOQUI_TEST_<NAME>` variables (`MOQUI_TEST_LOAD_PASSWORD` is `params.loadPassword`), then `--param`.
fn params(args: &ClientArgs) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = std::env::vars()
        .filter_map(|(name, value)| {
            let rest = name.strip_prefix("MOQUI_TEST_")?;
            (!rest.is_empty() && !NOT_PARAMS.contains(&rest)).then(|| (camel_case(rest), value))
        })
        .collect();
    for (name, value) in &args.params {
        out.insert(name.clone(), value.clone());
    }
    out
}

fn camel_case(name: &str) -> String {
    let mut out = String::new();
    for (index, part) in name.split('_').filter(|part| !part.is_empty()).enumerate() {
        let lower = part.to_ascii_lowercase();
        if index == 0 {
            out.push_str(&lower);
        } else {
            let mut chars = lower.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.extend(chars);
            }
        }
    }
    out
}

/// What a load run in a server (`crate::load_runner`, plans/archive/LoadRunner.md §7) adds to the client host: the check of
/// every URL its users reach, each redirect's included (§10 Targets), and `service.call` in process (§6).
pub(crate) trait InServer: Send + Sync {
    /// Whether the run may send a request to `url`; the error says why not.
    fn allow(&self, url: &str) -> Result<(), String>;
    /// Whether the run may connect over SSH to `host:port` (plans/archive/LoadRunner.md L7); the error says why not.
    fn allow_ssh(&self, host: &str, port: u16) -> Result<(), String>;
    /// `service.call(name, parameters, opts)` on the user's thread, whose ExecutionContext the run entered.
    fn service_call(&self, args: &Value) -> Result<Value, String>;
    /// The thread's `config` calls, the first before any of its scripts: the run binds what it must to the thread, and
    /// answers the first one with the token of the prelude's own `thread.begin`.
    fn thread_start(&self) -> Option<String>;
    /// Every host call of the run's threads, before it is answered: a `thread.begin` or `thread.end` that is not the
    /// thread's own is refused, whatever a script passes.
    fn check_call(&self, name: &str, args: &Value) -> Result<(), String>;
}

/// The client host of a run: the server, the params, the HTTP client, and what the current spec opened.
pub(crate) struct ClientHost {
    /// This host, for the threads of `load` and `parallel`.
    me: Weak<ClientHost>,
    server: Server,
    params: BTreeMap<String, String>,
    client: reqwest::blocking::Client,
    insecure: bool,
    /// `--timeout`.
    timeout: Duration,
    /// A request's limit when its own `timeout` does not set one.
    request_timeout: Duration,
    out_dir: PathBuf,
    /// The component of the spec running, the root of `files`.
    component_dir: Mutex<PathBuf>,
    next_id: AtomicU64,
    sessions: Mutex<HashMap<u64, Arc<Mutex<Session>>>>,
    sockets: Mutex<HashMap<u64, Arc<Mutex<ws::Socket>>>>,
    /// `ssh`'s connections and shells.
    ssh: ssh::Ssh,
    bodies: Mutex<VecDeque<(u64, Arc<Vec<u8>>)>>,
    /// The component and spec running, which name a load's JSON.
    current: Mutex<(String, String)>,
    /// The case's output so far (load tables), taken by `after_case`.
    output: Mutex<Vec<String>>,
    browser_config: browser::BrowserConfig,
    /// The spec's browser, started on its first page and stopped after the spec.
    browser: Mutex<Option<browser::SpecBrowser>>,
    /// The spec running has `server: false`.
    offline: Mutex<bool>,
    /// `--video`: each spec's browser screencasts its pages into `recording`.
    video: bool,
    recording: video::Shared,
    /// A load run in a server: none for `moqui test-client`.
    in_server: Option<Arc<dyn InServer>>,
}

/// The calls a load run in a server refuses: a browser, nested loads and `parallel` threads have no place there.
/// The longest a request, a socket connect or a socket wait of a load run in a server may take: a stop reaches a user
/// between requests, so this bounds how long a stopped run's threads still send.
const IN_SERVER_MAX_WAIT: Duration = Duration::from_secs(30);

/// The host functions a script's thread of a load run in a server may call: the client host's calls and the pure
/// helpers. Not `System`, `SftpClient`, `ResourceReference`, the `ec` facades, or the spec host's `setProperty`,
/// `parallel` and the like: in the server those would be the server's own (plans/archive/LoadRunner.md §10).
fn in_server_host_call(path: &str) -> bool {
    const PURE: [&str; 5] = [
        "global.",
        "StringUtilities.",
        "CollectionUtilities.",
        "WebUtilities.",
        "ObjectUtilities.",
    ];
    path == "__moquiTestHost.call" || PURE.iter().any(|prefix| path.starts_with(prefix))
}

/// `response.save` would write into the server's files.
const NOT_IN_SERVER: [&str; 6] = ["browser.", "page.", "locator.", "load.run", "parallel", "response.save"];

/// A request's `expect: [401, ...]`: the statuses a load does not count as errors.
fn expected(opts: &Value) -> Option<Vec<u16>> {
    field(opts, "expect").as_array().map(|statuses| {
        statuses
            .iter()
            .filter_map(Value::as_u64)
            .filter_map(|status| u16::try_from(status).ok())
            .collect()
    })
}

/// Count a request in the load this thread is a user of, if any: an `Err` (to throw) when it is an error there.
fn observe(
    opts: &Value,
    default_name: impl FnOnce() -> String,
    sent: &Result<http::Answer, String>,
    started: Instant,
) -> Result<(), String> {
    let name = || field(opts, "name").as_str().map_or_else(default_name, str::to_string);
    match sent {
        Ok(answer) => {
            let head = &answer.body[..answer.body.len().min(400)];
            let text = String::from_utf8_lossy(head);
            let observed = load::Observed {
                status: Some(answer.status),
                text: &text,
                elapsed: answer.elapsed,
                expect: expected(opts),
            };
            load::observe(name, load::Kind::Http, &observed)
        }
        Err(err) => load::observe(
            name,
            load::Kind::Http,
            &load::Observed {
                status: None,
                text: err,
                elapsed: started.elapsed(),
                expect: None,
            },
        ),
    }
}

/// A sample's `name` from the call's arguments, else `default`.
fn sample_name(args: &Value, default: impl FnOnce() -> String) -> String {
    field(args, "name").as_str().map_or_else(default, str::to_string)
}

/// A call's `expect`: the outcome labels a load does not count as errors (a number is its text: a status, an exit
/// status).
fn labels(args: &Value) -> Vec<String> {
    field(args, "expect")
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| match label {
                    Value::String(text) => Some(text.clone()),
                    Value::Number(number) => Some(number.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn field<'a>(args: &'a Value, name: &str) -> &'a Value {
    args.get(name).unwrap_or(&Value::Null)
}

fn text_field(args: &Value, name: &str) -> String {
    field(args, name).as_str().unwrap_or("").to_string()
}

/// Unix seconds from a JSON number, or now when the value was left out.
fn unix_seconds(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|seconds| i64::try_from(seconds).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|seconds| seconds.is_finite())
                .map(|seconds| seconds as i64)
        })
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        })
}

/// `hmac.sha256Base64` and `hmac.sha256Timestamp`: [`hmac_base64`] and [`timestamp_header`].
fn hmac_sign(name: &str, args: &Value) -> Result<Value, String> {
    let secret = text_field(args, "secret");
    let body = text_field(args, "body");
    let signed = match name {
        "hmac.sha256Base64" => hmac_base64(&secret, &body),
        "hmac.sha256Timestamp" => timestamp_header(&secret, &body, unix_seconds(field(args, "unixSeconds"))),
        other => return Err(format!("{other} is not a call of the client host")),
    };
    Ok(Value::String(signed))
}

fn pairs(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(name, value)| {
                    let text = value.as_str().map_or_else(|| value.to_string(), str::to_string);
                    (name.clone(), text)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A load user's request says it is one: `User-Agent: moqui-load/<version>`, unless the request or its session names
/// an agent of its own.
fn agent_header(ask: &mut Ask, session_headers: &[(String, String)]) {
    let named = |name: &str| name.eq_ignore_ascii_case("user-agent");
    if session_headers.iter().any(|(name, _)| named(name)) {
        return;
    }
    let headers = ask
        .opts
        .entry("headers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !headers.is_object() {
        *headers = Value::Object(serde_json::Map::new());
    }
    if let Some(headers) = headers.as_object_mut() {
        if !headers.keys().any(|name| named(name)) {
            headers.insert("User-Agent".to_string(), json!(load::LOAD_AGENT));
        }
    }
}

/// Seconds from a JSON number, when it is one from 0.
fn seconds_of(value: &Value) -> Option<Duration> {
    value
        .as_f64()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(load::seconds)
}

/// A load's function source, or `null`.
fn source_of(args: &Value, name: &str) -> String {
    field(args, name)
        .as_str()
        .map_or_else(|| "null".to_string(), str::to_string)
}

/// A script's thresholds with the run's over them: each top-level limit, and each name's.
fn merge_limits(script: &Value, run: &Value) -> Value {
    let mut out = script.as_object().cloned().unwrap_or_default();
    let Some(run) = run.as_object() else {
        return Value::Object(out);
    };
    for (key, value) in run {
        match (
            key.as_str(),
            value.as_object(),
            out.get_mut(key).and_then(Value::as_object_mut),
        ) {
            ("names", Some(names), Some(into)) => {
                for (name, limits) in names {
                    let entry = into.entry(name.clone()).or_insert_with(|| json!({}));
                    if let (Some(entry), Some(limits)) = (entry.as_object_mut(), limits.as_object()) {
                        entry.extend(limits.clone());
                    }
                }
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(out)
}

impl ClientHost {
    /// The host of a load run in a server (plans/archive/LoadRunner.md §7): its users' sessions go to `server`, every URL
    /// through `in_server`'s check, `files` stays in `component_dir` (the script's component), and `service.call`
    /// runs in process. No browser.
    pub(crate) fn for_server_run(
        server: &str,
        ssh: Option<(String, u16)>,
        params: BTreeMap<String, String>,
        insecure: bool,
        component_dir: PathBuf,
        in_server: Arc<dyn InServer>,
    ) -> Result<Arc<ClientHost>, String> {
        // the target rule is in_server's, which also knows when plain http is allowed
        let server = Server::parse(server, true)
            .map_err(|err| err.to_string())?
            .with_ssh(ssh.map(|(host, port)| SshTarget { host, port }));
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(insecure)
            .user_agent(load::LOAD_AGENT)
            .build()
            .map_err(|err| format!("HTTP client: {err}"))?;
        Ok(Arc::new_cyclic(|me| ClientHost {
            me: me.clone(),
            server,
            params,
            client,
            insecure,
            timeout: REQUEST_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            out_dir: std::env::temp_dir(),
            component_dir: Mutex::new(component_dir),
            next_id: AtomicU64::new(1),
            sessions: Mutex::new(HashMap::new()),
            sockets: Mutex::new(HashMap::new()),
            ssh: ssh::Ssh::new(),
            bodies: Mutex::new(VecDeque::new()),
            current: Mutex::new((String::new(), String::new())),
            output: Mutex::new(Vec::new()),
            browser_config: browser::BrowserConfig {
                chromium: None,
                headed: false,
                insecure,
                runtime: None,
            },
            browser: Mutex::new(None),
            offline: Mutex::new(false),
            video: false,
            recording: Arc::new(Mutex::new(None)),
            in_server: Some(in_server),
        }))
    }

    /// A load run in a server checks each URL its users reach; `moqui test-client` checks none.
    fn allow(&self, url: &str) -> Result<(), String> {
        match &self.in_server {
            Some(in_server) => in_server.allow(url),
            None => Ok(()),
        }
    }

    /// A wait of a load run in a server, at most [`IN_SERVER_MAX_WAIT`]; any wait for `moqui test-client`.
    fn capped(&self, wait: Duration) -> Duration {
        if self.in_server.is_some() {
            wait.min(IN_SERVER_MAX_WAIT)
        } else {
            wait
        }
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn session(&self, args: &Value) -> Result<Arc<Mutex<Session>>, String> {
        let id = field(args, "session").as_u64().unwrap_or(0);
        lock(&self.sessions)
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("session {id} is closed: a session lives for its spec"))
    }

    fn socket(&self, args: &Value) -> Result<Arc<Mutex<ws::Socket>>, String> {
        let id = field(args, "id").as_u64().unwrap_or(0);
        lock(&self.sockets)
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("socket {id} is closed"))
    }

    fn seconds(&self, args: &Value, name: &str, default: Duration) -> Duration {
        field(args, name)
            .as_f64()
            .filter(|seconds| *seconds >= 0.0)
            .map_or(default, load::seconds)
    }

    /// Keep a body for `save` and answer the response JSON.
    fn answer(&self, answer: http::Answer) -> Value {
        let id = self.id();
        let json = answer.to_json(id);
        let mut bodies = lock(&self.bodies);
        bodies.push_back((id, Arc::new(answer.body)));
        while bodies.len() > KEPT_BODIES {
            bodies.pop_front();
        }
        json
    }

    fn request(&self, args: &Value) -> Result<Value, String> {
        let session = self.session(args)?;
        let path = text_field(args, "path");
        let mut ask = Ask {
            method: text_field(args, "method"),
            url: self.server.join(&path),
            opts: field(args, "opts").as_object().cloned().unwrap_or_default(),
        };
        load::before_request()?;
        if load::is_user() {
            agent_header(&mut ask, &lock(&session).headers);
        }
        if self.in_server.is_some() {
            let timeout = self.capped(ask.timeout(REQUEST_TIMEOUT));
            ask.opts.insert("timeout".to_string(), json!(timeout.as_secs_f64()));
        }
        let started = Instant::now();
        let allow = |url: &str| self.allow(url);
        let sent = http::send(&self.client, &mut lock(&session), &ask, self.request_timeout, &allow);
        let opts = field(args, "opts");
        observe(opts, || load::default_name(&ask.method, &path), &sent, started)?;
        Ok(self.answer(sent?))
    }

    fn raw(&self, args: &Value) -> Result<Value, String> {
        let session = self.session(args)?;
        let opts = field(args, "opts");
        let cookies = field(opts, "cookies").as_bool().unwrap_or(true);
        let timeout = self.capped(self.seconds(opts, "timeout", self.timeout));
        let text = text_field(args, "text");
        let target = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");
        // the connection is to the server whatever the request line names: both are checked
        self.allow(&self.server.url)?;
        self.allow(&self.server.join(target))?;
        let answer = {
            let held = lock(&session);
            raw::send(&self.server, &held, &text, cookies, self.insecure, timeout)?
        };
        let content_type = answer.header("content-type").unwrap_or("").to_string();
        lock(&session).absorb(&answer.url, &answer.headers, &content_type, &answer.body);
        Ok(self.answer(answer))
    }

    /// `http.raw` under a load: timed as a request named `RAW` and its target, unless it has a `name`.
    fn raw_timed(&self, args: &Value) -> Result<Value, String> {
        load::before_request()?;
        let started = Instant::now();
        let text = text_field(args, "text");
        let target = text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");
        let sent = self.raw(args);
        let observed = match &sent {
            Ok(value) => load::Observed {
                status: value
                    .get("status")
                    .and_then(Value::as_u64)
                    .and_then(|status| u16::try_from(status).ok()),
                text: value.get("text").and_then(Value::as_str).unwrap_or(""),
                elapsed: value
                    .get("timeMs")
                    .and_then(Value::as_f64)
                    .map_or_else(|| started.elapsed(), |ms| Duration::from_secs_f64(ms / 1000.0)),
                expect: expected(field(args, "opts")),
            },
            Err(err) => load::Observed {
                status: None,
                text: err,
                elapsed: started.elapsed(),
                expect: None,
            },
        };
        let name = || {
            field(field(args, "opts"), "name")
                .as_str()
                .map_or_else(|| load::default_name("RAW", target), str::to_string)
        };
        load::observe(name, load::Kind::Raw, &observed)?;
        sent
    }

    /// Start `count` threads, each `source` (a function's text) called with the `args` that `item` gives it, and
    /// wait for them until `timeout`, then 10 seconds more. Each thread's answer, or why it has none.
    fn run_threads(
        &self,
        kind: &str,
        count: u64,
        source: &str,
        item: &dyn Fn(u64) -> String,
        timeout: Duration,
    ) -> Vec<(u64, Result<String, String>)> {
        let Some(me) = self.me.upgrade() else {
            return Vec::new();
        };
        let host: Arc<dyn SpecHost> = me;
        let mut running = Vec::new();
        let mut out = Vec::new();
        for index in 0..count {
            match run_on_thread(
                Arc::clone(&host),
                &format!("moqui-{kind}-{index}"),
                source,
                &item(index),
            ) {
                Ok(handle) => running.push((index, handle)),
                Err(err) => out.push((index, Err(format!("the thread did not start: {err}")))),
            }
        }
        out.extend(load::join_all(running, timeout, &|| {}, &mut |_| false));
        out.sort_by_key(|(index, _)| *index);
        out
    }

    /// `load(opts)` (5.6; plans/archive/LoadRunner.md section 9): the users of its functions, or of a load script's scenarios,
    /// until the duration ends or each has run its iterations; the result, its table for the case's output, and its
    /// JSON in `<out>/load`.
    fn run_load(&self, args: &Value) -> Result<Value, String> {
        let Some(me) = self.me.upgrade() else {
            return Err("load: the client host is closing".to_string());
        };
        let host: Arc<dyn SpecHost> = me;
        let mut thresholds = field(args, "thresholds").clone();
        let mut run = serde_json::Map::new();
        let (scenarios, source) = match field(args, "script").as_str() {
            Some(relative) => {
                let component = lock(&self.component_dir).clone();
                let script = load::LoadScript::read(&component, relative)?;
                let plan = load::script::read_plan(&host, &script, field(args, "params"), &Value::Null)?;
                if !plan.uses().iter().any(|uses| uses == "http") {
                    return Err(format!(
                        "load: {} uses {}, which a client cannot make: it runs in a server (Tools › Load)",
                        script.display,
                        plan.uses().join(", ")
                    ));
                }
                let scenarios = load::script::settings(&plan, args)?;
                thresholds = merge_limits(plan.thresholds(), &thresholds);
                run.insert("script".into(), json!(script.display));
                run.insert("params".into(), plan.public_params());
                let source = load::Source::Script {
                    script: Arc::new(script),
                    params: plan.params.clone(),
                };
                (scenarios, source)
            }
            None => {
                let settings = load::ScenarioSettings::from_json("load", args, "load")?;
                let source = format!(
                    "(spec) => __moquiThread.load(spec, {}, {}, {})",
                    source_of(args, "setup"),
                    source_of(args, "scenario"),
                    source_of(args, "teardown")
                );
                run.insert("script".into(), Value::Null);
                let source = load::Source::Functions {
                    source,
                    args: field(args, "args").clone(),
                };
                (vec![settings], source)
            }
        };
        run.insert("target".into(), json!(self.server.url));
        run.insert("label".into(), field(args, "label").clone());
        let start = load::Start {
            scenarios,
            source,
            timeout: field(args, "timeout").as_f64(),
            thresholds,
            run,
            generator: "same-machine",
            around: None,
            stop_after: seconds_of(field(args, "stopAfter")),
            stop_now_after: seconds_of(field(args, "stopNowAfter")),
        };
        let running = load::start(&host, start)?;
        let (stats, order) = running.wait();
        let mut text = load::table(&stats, &order);
        if let Err(err) = self.write_load(args, &stats) {
            tracing::error!("{err}");
            let _ = writeln!(text, "{err}");
        }
        lock(&self.output).push(text);
        Ok(stats)
    }

    /// The load result into `<out>/load/<component>.<Spec>.<feature>.json`, with `-2` and on for another load of the
    /// same feature.
    fn write_load(&self, args: &Value, stats: &Value) -> Result<PathBuf, String> {
        let (component, spec) = lock(&self.current).clone();
        let feature = text_field(args, "feature");
        let dir = self.out_dir.join("load");
        std::fs::create_dir_all(&dir).map_err(|err| format!("load: {}: {err}", dir.display()))?;
        let stem = format!(
            "{}.{}.{}",
            load::file_part(&component),
            load::file_part(&spec),
            load::file_part(&feature)
        );
        let mut path = dir.join(format!("{stem}.json"));
        let mut number = 2;
        while path.exists() {
            path = dir.join(format!("{stem}-{number}.json"));
            number += 1;
        }
        let mut file = stats.clone();
        if let Some(run) = file.get_mut("run").and_then(Value::as_object_mut) {
            let more = json!({ "component": component, "spec": spec, "feature": feature, "server": self.server.url,
                "finished": jiff::Timestamp::now().to_string() });
            if let Value::Object(more) = more {
                run.extend(more);
            }
        }
        let text = serde_json::to_string_pretty(&file).map_err(|err| format!("load: {err}"))?;
        std::fs::write(&path, text).map_err(|err| format!("load: {}: {err}", path.display()))?;
        Ok(path)
    }

    /// The client's `parallel(n, fn, args)`: `fn(index, args)` on `n` threads, each with its own engine and default
    /// session; `{ results, errors }`.
    fn run_parallel(&self, args: &Value) -> Result<Value, String> {
        let count = field(args, "n").as_f64().unwrap_or(0.0);
        if !((1.0..=10_000.0).contains(&count) && count.fract() == 0.0) {
            return Err(format!("parallel: n is a whole number from 1 to 10000, not {count}"));
        }
        let count = count as u64;
        let source = format!(
            "(index, args) => __moquiThread.parallel(index, args, {})",
            text_field(args, "source")
        );
        let thread_args = field(args, "args").clone();
        let item = |index: u64| json!([index, thread_args]).to_string();
        let timeout = self.seconds(args, "timeout", Duration::from_secs(120));
        let answers = self.run_threads("parallel", count, &source, &item, timeout);
        let mut results = vec![Value::Null; count as usize];
        let mut errors = Vec::new();
        for (index, answer) in answers {
            match answer {
                Ok(json) => results[index as usize] = serde_json::from_str(&json).unwrap_or(Value::Null),
                Err(err) => errors.push(format!("thread {index}: {err}")),
            }
        }
        Ok(json!({ "results": results, "errors": errors }))
    }

    fn connect(&self, args: &Value) -> Result<Value, String> {
        let session = self.session(args)?;
        let origin = field(args, "origin").as_str().map(str::to_string);
        let path = text_field(args, "path");
        // the socket connects to the server, its request names the path: both are checked
        self.allow(&self.server.url)?;
        self.allow(&self.server.join(&path))?;
        load::before_request()?;
        let started = Instant::now();
        let connected = {
            let held = lock(&session);
            ws::connect(
                &self.server,
                &held,
                &path,
                &pairs(field(args, "headers")),
                origin.as_deref(),
                self.insecure,
                self.capped(self.seconds(args, "timeout", self.timeout)),
            )
        };
        let name = || sample_name(args, || format!("ws {}", path.split('?').next().unwrap_or("")));
        ws::observe_connect(name, &connected, started.elapsed(), &labels(args))?;
        let connected = connected?;
        let id = connected.socket.is_some().then(|| self.id());
        let json = connected.to_json(id);
        if let (Some(id), Some(socket)) = (id, connected.socket) {
            lock(&self.sockets).insert(id, Arc::new(Mutex::new(socket)));
        }
        Ok(json)
    }

    /// An `ssh.*` call (`ssh.rs`), with this host's server, timeouts and, in a server run, its target rule.
    fn ssh_call(&self, name: &str, args: &Value) -> Result<Value, String> {
        let allow = |host: &str, port: u16| match &self.in_server {
            Some(in_server) => in_server.allow_ssh(host, port),
            None => Ok(()),
        };
        let cap = |wait: Duration| self.capped(wait);
        let id = || self.id();
        let env = ssh::Env {
            server: &self.server,
            insecure: self.insecure,
            timeout: self.timeout,
            cap: &cap,
            allow: &allow,
            id: &id,
        };
        self.ssh.call(name, args, &env)
    }

    /// Save a kept body into the output directory under `name`, a plain file name.
    fn save(&self, args: &Value) -> Result<Value, String> {
        let id = field(args, "id").as_u64().unwrap_or(0);
        let name = text_field(args, "name");
        if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
            return Err(format!("save: '{name}' is not a plain file name"));
        }
        let body = lock(&self.bodies)
            .iter()
            .find(|(kept, _)| *kept == id)
            .map(|(_, body)| Arc::clone(body))
            .ok_or_else(|| format!("save: the body of response {id} is no longer kept (the last {KEPT_BODIES} are)"))?;
        std::fs::create_dir_all(&self.out_dir).map_err(|err| format!("{}: {err}", self.out_dir.display()))?;
        let path = self.out_dir.join(&name);
        std::fs::write(&path, body.as_slice()).map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Value::String(path.display().to_string()))
    }

    /// A path within the spec's component directory: relative, with no `..`, and not out through a link.
    fn confined(&self, relative: &str) -> Result<PathBuf, String> {
        let root = lock(&self.component_dir).clone();
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir | std::path::Component::Prefix(_)))
        {
            return Err(format!("files: {relative} is not a path within the component"));
        }
        let root = std::fs::canonicalize(&root).map_err(|err| format!("files: {}: {err}", root.display()))?;
        let full = std::fs::canonicalize(root.join(path)).map_err(|err| format!("files: {relative}: {err}"))?;
        if !full.starts_with(&root) {
            return Err(format!("files: {relative} is outside the component"));
        }
        Ok(full)
    }

    fn list(&self, args: &Value) -> Result<Value, String> {
        let dir = text_field(args, "dir");
        let root = self.confined(".")?;
        let start = self.confined(if dir.is_empty() { "." } else { &dir })?;
        let mut found = Vec::new();
        let mut pending = vec![start];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() {
                    if let Ok(relative) = path.strip_prefix(&root) {
                        found.push(relative.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        found.sort();
        Ok(json!(found))
    }

    /// An offline spec has no server. `hmac`, `files`, `sleep` and `log` stay.
    fn require_online(&self, what: &str) -> Result<(), String> {
        if *lock(&self.offline) {
            Err(format!("{what} needs a server; this spec has server: false"))
        } else {
            Ok(())
        }
    }

    /// `browser.newPage(opts)`: the spec's browser started first when it has none. With no Chromium to start, the
    /// answer says to skip.
    fn new_page(&self, args: &Value) -> Result<Value, String> {
        let mut held = lock(&self.browser);
        if held.is_none() {
            if let Err(reason) = crate::chromium::find_test_browser(
                self.browser_config.runtime.as_deref(),
                self.browser_config.chromium.as_deref().unwrap_or(""),
            ) {
                return Ok(json!({ "skipped": reason }));
            }
            let mut launched = browser::SpecBrowser::launch(&self.browser_config)?;
            if self.video {
                launched.record_to(Arc::clone(&self.recording));
            }
            *held = Some(launched);
            if *lock(&self.offline) {
                let runtime = self.browser_config.runtime.clone().unwrap_or_default();
                held.as_mut()
                    .ok_or("the browser did not start")?
                    .use_offline(offline::Roots::from_runtime(&runtime));
            }
        }
        let spec_browser = held.as_mut().ok_or("the browser did not start")?;
        let id = self.id();
        spec_browser.new_page(id, &self.server, args, self.timeout)?;
        Ok(json!({ "id": id }))
    }

    /// The page file's `test()` calls. No browser is a skip, the same as `newPage`.
    fn run_in_page(&self, args: &Value) -> Result<Value, String> {
        let mut held = lock(&self.browser);
        if held.is_none() {
            if let Err(reason) = crate::chromium::find_test_browser(
                self.browser_config.runtime.as_deref(),
                self.browser_config.chromium.as_deref().unwrap_or(""),
            ) {
                return Ok(json!({ "skipped": reason }));
            }
            let mut launched = browser::SpecBrowser::launch(&self.browser_config)?;
            if self.video {
                launched.record_to(Arc::clone(&self.recording));
            }
            *held = Some(launched);
            if *lock(&self.offline) {
                let runtime = self.browser_config.runtime.clone().unwrap_or_default();
                held.as_mut()
                    .ok_or("the browser did not start")?
                    .use_offline(offline::Roots::from_runtime(&runtime));
            }
        }
        if !*lock(&self.offline) {
            return Err("runInPage needs a spec with server: false".to_string());
        }
        let spec_browser = held.as_mut().ok_or("the browser did not start")?;
        let id = self.id();
        spec_browser.run_in_page(id, &self.server, args, self.timeout)
    }

    fn dispatch(&self, name: &str, args: &Value) -> Result<Value, String> {
        if let Some(in_server) = &self.in_server {
            in_server.check_call(name, args)?;
        }
        if let Some(answer) = load::host_call(name, args) {
            return answer;
        }
        if self.in_server.is_some() && NOT_IN_SERVER.iter().any(|refused| name.starts_with(refused)) {
            return Err(format!("{name} is not available in a load run in a server"));
        }
        if name.starts_with("page.") || name.starts_with("locator.") {
            let mut held = lock(&self.browser);
            let spec_browser = held
                .as_mut()
                .ok_or_else(|| "the browser is closed: a page lives for its spec".to_string())?;
            return spec_browser.call(name, args, &self.server, &self.out_dir);
        }
        match name {
            "browser.newPage" => self.new_page(args),
            "browser.runInPage" => self.run_in_page(args),
            "config" => {
                // the client prelude's first call, before any script of the thread: in a server run the thread's
                // scripts reach only the client harness's host functions from here on
                let mut token = None;
                if let Some(in_server) = &self.in_server {
                    moqui_core::script::restrict_host_calls(in_server_host_call);
                    token = in_server.thread_start();
                }
                Ok(json!({
                    "server": self.server.to_json(),
                    "params": self.params,
                    "timeout": self.timeout.as_secs_f64(),
                    "inServer": self.in_server.is_some(),
                    "threadToken": token,
                }))
            }
            "service.call" => match &self.in_server {
                Some(in_server) => in_server.service_call(args),
                None => Err("service.call runs in a server's own process (a load run from Tools › Load)".to_string()),
            },
            "session.new" => {
                let id = self.id();
                let session = Session {
                    headers: pairs(field(args, "headers")),
                    ..Session::default()
                };
                lock(&self.sessions).insert(id, Arc::new(Mutex::new(session)));
                load::track_session(id);
                Ok(json!(id))
            }
            "session.close" => {
                let id = field(args, "session").as_u64().unwrap_or(0);
                lock(&self.sessions).remove(&id);
                Ok(Value::Null)
            }
            "session.cookies" => {
                let session = self.session(args)?;
                let cookies = lock(&session).jar.to_json();
                Ok(cookies)
            }
            "session.clearCookies" => {
                let session = self.session(args)?;
                lock(&session).jar.clear();
                Ok(Value::Null)
            }
            "session.token" => {
                let session = self.session(args)?;
                let token = lock(&session).token.clone();
                Ok(json!(token))
            }
            "http.request" => {
                self.require_online("http")?;
                self.request(args)
            }
            "http.raw" => {
                self.require_online("http.raw")?;
                self.raw_timed(args)
            }
            "hmac.sha256Base64" | "hmac.sha256Timestamp" => hmac_sign(name, args),
            "load.run" => self.run_load(args),
            "parallel" => self.run_parallel(args),
            "thread.end" => {
                // in a server run the thread's recorder ends with the thread (load_runner/run.rs around), not with a
                // call a script can make early; its sessions go with the run's host
                let mut ended = if self.in_server.is_some() {
                    Vec::new()
                } else {
                    load::end()
                };
                ended.extend(field(args, "session").as_u64());
                // a user's SSH connections and shells are tracked as its sessions are
                self.ssh.close(&ended);
                let mut sessions = lock(&self.sessions);
                for id in ended {
                    sessions.remove(&id);
                }
                Ok(Value::Null)
            }
            "response.save" => self.save(args),
            "ws.connect" => {
                self.require_online("ws")?;
                self.connect(args)
            }
            "ws.send" => {
                let socket = self.socket(args)?;
                let text = text_field(args, "text");
                // a send is a sample only when named: most are a subscribe before the receives that are timed
                if let Some(name) = field(args, "name").as_str() {
                    load::before_request()?;
                    let started = Instant::now();
                    let sent = lock(&socket).send(&text);
                    let (outcome, failure) = match &sent {
                        Ok(()) => ("ok", None),
                        Err(err) => ("error", Some(err.clone())),
                    };
                    ws::observe(|| name.to_string(), outcome.into(), failure, started.elapsed(), &[])?;
                    sent?;
                } else {
                    lock(&socket).send(&text)?;
                }
                Ok(Value::Null)
            }
            "ws.receive" => {
                let timeout = self.capped(self.seconds(args, "timeout", self.timeout));
                let socket = self.socket(args)?;
                let mut held = lock(&socket);
                let frame = if load::is_user() {
                    let name = || sample_name(args, || "ws receive".to_string());
                    ws::receive_observed(&mut held, timeout, name, &labels(args))?
                } else {
                    held.receive(timeout)?
                };
                Ok(json!(frame))
            }
            "ws.close" => {
                let id = field(args, "id").as_u64().unwrap_or(0);
                if let Some(socket) = lock(&self.sockets).remove(&id) {
                    lock(&socket).close();
                }
                Ok(Value::Null)
            }
            name if name.starts_with("ssh.") => {
                self.require_online("ssh")?;
                self.ssh_call(name, args)
            }
            "files.list" => self.list(args),
            "files.read" => {
                let path = self.confined(&text_field(args, "path"))?;
                std::fs::read_to_string(&path)
                    .map(Value::String)
                    .map_err(|err| format!("files: {}: {err}", path.display()))
            }
            "sleep" => {
                let ms = field(args, "ms").as_f64().unwrap_or(0.0).clamp(0.0, 3_600_000.0);
                if load::is_user() || self.in_server.is_some() {
                    // a load user's wait ends on a stop, as its pauses do
                    load::pause(&load::Pause::Fixed(ms), None);
                } else {
                    std::thread::sleep(Duration::from_secs_f64(ms / 1000.0));
                }
                Ok(Value::Null)
            }
            "log" => {
                let message = text_field(args, "message");
                match text_field(args, "level").as_str() {
                    "trace" => tracing::trace!("{message}"),
                    "debug" => tracing::debug!("{message}"),
                    "warn" => tracing::warn!("{message}"),
                    "error" => tracing::error!("{message}"),
                    _ => tracing::info!("{message}"),
                }
                Ok(Value::Null)
            }
            other => Err(format!("{other} is not a call of the client host")),
        }
    }
}

impl SpecHost for ClientHost {
    fn prelude(&self) -> &str {
        PRELUDE
    }

    fn call(&self, name: &str, args: &str) -> Result<String, String> {
        let args: Value =
            serde_json::from_str(args).map_err(|err| format!("{name}: the arguments did not read: {err}"))?;
        self.dispatch(name, &args).map(|value| value.to_string())
    }

    /// The case's load tables as its output; its pages' problems, and on a failure their screenshots and text.
    fn after_case(&self, case: &str, failed: bool, allow_problems: bool) -> CaseExtras {
        let output = std::mem::take(&mut *lock(&self.output));
        let (problems, attachments) = match lock(&self.browser).as_mut() {
            Some(spec_browser) => {
                let feature: String = case.chars().take(80).collect();
                spec_browser.after_case(failed, allow_problems, &feature, &self.out_dir)
            }
            None => (Vec::new(), Vec::new()),
        };
        CaseExtras {
            problems,
            attachments,
            output: output.join("\n"),
        }
    }

    /// Close the spec's browser (its profile removed) and sockets, and drop its sessions and kept bodies.
    fn after_spec(&self) {
        let mut spec_browser = lock(&self.browser).take();
        if let Some(spec_browser) = spec_browser.as_mut() {
            spec_browser.stop_video();
        }
        if let Some(recorder) = lock(&self.recording).as_mut() {
            recorder.mark_end();
        }
        drop(spec_browser);
        let sockets: Vec<_> = lock(&self.sockets).drain().map(|(_, socket)| socket).collect();
        for socket in sockets {
            lock(&socket).close();
        }
        self.ssh.close_all();
        lock(&self.sessions).clear();
        lock(&self.bodies).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ssh_target_is_a_host_and_port_or_a_port_on_the_servers_host() {
        let server = Server::parse("http://erp.local:8080", true).unwrap();
        let target = |text: &str| ssh_target(Some(text), &server).map_err(|err| err.to_string());
        let parsed = |host: &str, port: u16| {
            Ok(Some(SshTarget {
                host: host.to_string(),
                port,
            }))
        };
        assert_eq!(target("2222"), parsed("erp.local", 2222));
        assert_eq!(target("127.0.0.1:2200"), parsed("127.0.0.1", 2200));
        assert_eq!(target("[::1]:22"), parsed("::1", 22));
        assert!(target("host:none").unwrap_err().contains("give host:port or a port"));
        assert!(target("0").is_err());
        assert_eq!(server.with_ssh(None).to_json()["ssh"], Value::Null);
    }

    #[test]
    fn process_specs_run_only_by_name() {
        let runtime = std::env::temp_dir().join(format!("moqui-test-client-process-{}", std::process::id()));
        let process = runtime.join("component/pv/test/client/process");
        std::fs::create_dir_all(&process).unwrap();
        std::fs::write(runtime.join("component/pv/component.xml"), "<component name=\"pv\"/>").unwrap();
        std::fs::write(process.join("Greeting.spec.js"), "spec({})").unwrap();
        let names = |components: &[TestComponent]| -> Vec<String> {
            components
                .iter()
                .flat_map(|component| component.specs.iter().map(|spec| spec.display.clone()))
                .collect()
        };
        let named = ClientArgs {
            targets: vec!["pv".to_string()],
            specs: vec!["Greeting".to_string()],
            ..ClientArgs::default()
        };
        assert_eq!(
            names(&select(&runtime, &named).unwrap()),
            vec!["pv/test/client/process/Greeting.spec.js"]
        );
        let whole = ClientArgs {
            targets: vec!["pv".to_string()],
            ..ClientArgs::default()
        };
        assert!(select(&runtime, &whole).is_err());
        let file = ClientArgs {
            targets: vec![process.join("Greeting.spec.js").to_string_lossy().into_owned()],
            ..ClientArgs::default()
        };
        assert_eq!(
            names(&select(&runtime, &file).unwrap()),
            vec!["pv/test/client/process/Greeting.spec.js"]
        );
        let _ = std::fs::remove_dir_all(runtime);
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn options_parse_and_errors_name_the_option() {
        let parsed = parse_client_args(&args(&[
            "framework",
            "path/to/test/client",
            "--server",
            "https://test.example.com",
            "--spec",
            "A,B",
            "--param",
            "users=50",
            "--param",
            "note=a=b",
            "--junit",
            "out",
            "--wait",
            "30",
            "--timeout",
            "2.5",
            "--insecure",
            "--fail-on-skip",
            "--video",
        ]))
        .unwrap();
        assert_eq!(
            parsed.targets,
            vec!["framework".to_string(), "path/to/test/client".to_string()]
        );
        assert_eq!(parsed.server.as_deref(), Some("https://test.example.com"));
        assert_eq!(parsed.specs, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(
            parsed.params,
            vec![
                ("users".to_string(), "50".to_string()),
                ("note".to_string(), "a=b".to_string())
            ]
        );
        assert_eq!((parsed.wait, parsed.timeout), (Some(30.0), 2.5));
        assert!(parsed.insecure && parsed.fail_on_skip && parsed.video && !parsed.allow_http && !parsed.serve);
        assert!(parse_client_args(&args(&["framework", "--serve"])).unwrap().serve);
        assert!(
            !parsed.acceptance
                && parse_client_args(&args(&["framework", "--acceptance"]))
                    .unwrap()
                    .acceptance
        );
        for (bad, says) in [
            (&["--param", "novalue"][..], "--param needs name=value"),
            (&["--wait", "soon"][..], "--wait is a number of seconds"),
            (&["--timeout", "0"][..], "--timeout is more than 0"),
            (&["--server"][..], "--server needs a value"),
            (&["--bogus"][..], "unknown option --bogus"),
            (&["--serve", "--wait", "5"][..], "leave out --server, --ssh and --wait"),
            (
                &["--serve", "--ssh", "2222"][..],
                "leave out --server, --ssh and --wait",
            ),
            (&["--ssh"][..], "--ssh needs a value"),
            (&["--offline", "--serve"][..], "leave out --serve"),
            (&["--chromium"][..], "--chromium needs a value"),
        ] {
            let err = parse_client_args(&args(bad)).unwrap_err().to_string();
            assert!(err.contains(says), "{bad:?}: {err}");
        }
    }

    #[test]
    fn params_take_the_environment_names_in_camel_case() {
        assert_eq!(camel_case("LOAD_PASSWORD"), "loadPassword");
        assert_eq!(camel_case("USER"), "user");
        assert_eq!(camel_case("MAX_P95"), "maxP95");
    }

    #[test]
    fn components_follow_depends_on() {
        let dir = std::env::temp_dir().join(format!("moqui-test-client-order-{}", std::process::id()));
        let make = |name: &str, depends: &[&str]| {
            let component = dir.join(name);
            std::fs::create_dir_all(&component).unwrap();
            let children: String = depends
                .iter()
                .map(|depend| format!("<depends-on name=\"{depend}\"/>"))
                .collect();
            std::fs::write(
                component.join("component.xml"),
                format!("<component name=\"{name}\">{children}</component>"),
            )
            .unwrap();
            TestComponent {
                name: name.to_string(),
                dir: component,
                specs: Vec::new(),
            }
        };
        let mut components = vec![make("a", &["c"]), make("b", &[]), make("c", &["missing"])];
        order_by_depends_on(&mut components);
        let names: Vec<&str> = components.iter().map(|component| component.name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "a"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A client run takes the order of `moqui test` (Commerce F106): a spec named before its dependsOn in a later
    /// component runs after it, holding back the rest of its test/client/suite.js chain; with no dependsOn the order
    /// is the components' (component.xml depends-on), then suite.js, then name, as before; a cycle is an error.
    #[test]
    fn a_client_run_orders_specs_after_their_depends_on() {
        let runtime = std::env::temp_dir().join(format!("moqui-test-client-run-order-{}", std::process::id()));
        let write = |path: &str, text: &str| {
            let path = runtime.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        let layout = |orders_depend: &str| {
            write("component/boss/component.xml", r#"<component name="boss"/>"#);
            write(
                "component/pass/component.xml",
                r#"<component name="pass"><depends-on name="boss"/></component>"#,
            );
            write(
                "component/boss/test/client/suite.js",
                r#"suite(["Walk", "CommerceOrders", "After"])"#,
            );
            write("component/boss/test/client/Walk.spec.js", "spec({})");
            write("component/boss/test/client/CommerceOrders.spec.js", orders_depend);
            write("component/boss/test/client/After.spec.js", "spec({})");
            write("component/boss/test/client/Other.spec.js", "spec({})");
            write("component/pass/test/client/ShopOrders.spec.js", "spec({})");
            write("component/pass/test/client/Theme.spec.js", "spec({})");
        };
        let ordered = || -> Vec<String> {
            let components = select(&runtime, &ClientArgs::default()).unwrap();
            run_order(&components, false, true)
                .unwrap()
                .into_iter()
                .map(|(c, s)| format!("{}/{}", components[c].name, components[c].specs[s].name))
                .collect()
        };
        layout("spec({})");
        assert_eq!(
            ordered(),
            vec![
                "boss/Walk",
                "boss/CommerceOrders",
                "boss/After",
                "boss/Other",
                "pass/ShopOrders",
                "pass/Theme"
            ]
        );
        layout("spec({ dependsOn: ['ShopOrders'] })");
        assert_eq!(
            ordered(),
            vec![
                "boss/Walk",
                "boss/Other",
                "pass/ShopOrders",
                "boss/CommerceOrders",
                "boss/After",
                "pass/Theme"
            ]
        );
        write(
            "component/pass/test/client/ShopOrders.spec.js",
            "spec({ dependsOn: ['CommerceOrders'] })",
        );
        let components = select(&runtime, &ClientArgs::default()).unwrap();
        let err = run_order(&components, false, true).unwrap_err().to_string();
        assert!(err.contains("cycle") && err.contains("boss/CommerceOrders"), "{err}");
        let _ = std::fs::remove_dir_all(&runtime);
    }

    #[test]
    fn a_path_target_is_its_component_and_one_file_its_spec() {
        let dir = std::env::temp_dir().join(format!("moqui-test-client-path-{}", std::process::id()));
        let client = dir.join("comp/test/client");
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(dir.join("comp/component.xml"), "<component name=\"my-comp\"/>").unwrap();
        std::fs::write(client.join("A.spec.js"), "spec({})").unwrap();
        std::fs::write(client.join("B.spec.js"), "spec({ dependsOn: ['A'] })").unwrap();
        let whole = path_component(&client, false).unwrap();
        assert_eq!(whole.name, "my-comp");
        assert_eq!(whole.specs.len(), 2);
        assert_eq!(whole.specs[0].display, "my-comp/test/client/A.spec.js");
        assert_eq!(path_component(&dir.join("comp"), false).unwrap().specs.len(), 2);
        let one = path_component(&client.join("B.spec.js"), false).unwrap();
        assert_eq!(
            one.specs.iter().map(|spec| spec.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert!(path_component(&dir, false).is_err());
        assert!(path_component(&dir.join("nowhere"), false).is_err());
        // acceptance/client: a component directory with --acceptance, a file there, and its dependsOn in test/client
        let accept = dir.join("comp/acceptance/client");
        std::fs::create_dir_all(&accept).unwrap();
        std::fs::write(accept.join("Walk.spec.js"), "spec({ dependsOn: ['A'] })").unwrap();
        let walks = path_component(&dir.join("comp"), true).unwrap();
        assert_eq!(
            walks.specs.iter().map(|spec| spec.name.as_str()).collect::<Vec<_>>(),
            vec!["Walk"]
        );
        assert_eq!(walks.specs[0].display, "my-comp/acceptance/client/Walk.spec.js");
        let walk = path_component(&accept.join("Walk.spec.js"), false).unwrap();
        assert_eq!(
            walk.specs.iter().map(|spec| spec.name.as_str()).collect::<Vec<_>>(),
            vec!["A", "Walk"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Java `WebUtilities` vectors: Base64 of the fox sentence, and the timestamp header at a fixed second.
    #[test]
    fn hmac_matches_system_message() {
        let base64 = hmac_sign(
            "hmac.sha256Base64",
            &json!({ "secret": "key", "body": "The quick brown fox jumps over the lazy dog" }),
        )
        .unwrap();
        assert_eq!(base64.as_str(), Some("97yD9DBThCSxMpjmqm+xQ+9NWaFJRhdZl0edvC0aPNg="));
        let header = hmac_sign(
            "hmac.sha256Timestamp",
            &json!({ "secret": "key", "body": "body", "unixSeconds": 1_492_774_577 }),
        )
        .unwrap();
        assert_eq!(
            header.as_str(),
            Some("t=1492774577,v1=8873a6cf255dadf109c64c83dc66b532eec7bd05304ed07365b8be223a257e3b")
        );
    }
}
