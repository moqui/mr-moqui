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

//! `moqui test-client --serve` (plans/archive/ClientTestHarness.md 6): the selected components grouped by the server their
//! `test/client/suite.js` declares (`suite([...], { server: { components, specs, conf } })`), and for each group this
//! binary started as `moqui test <components> --spec <specs> --db snapshot --serve 127.0.0.1:0 --ready-file <file>`,
//! with `MOQUI_CONF` set to the declared conf. Its standard input is a pipe this process holds, so the child also stops
//! when the client dies without cleaning up. Once the ready file names the URL the group's specs run against it, then
//! the pipe closes and the child stops and removes its scratch runtime.
//!
//! The client still opens no database and binds no port: the child does. A child that does not start fails its
//! group's specs with the last lines of its output, and the other groups still run (D14). The child gets `--slow` when
//! the client has it, and renders PDF with the browser the client drives when nothing names one for PDF. A component
//! whose specs are all slow starts no server without `--slow`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use moqui_core::testkit::{spec_offline, spec_slow, suite_server, Failure, SpecResult, SuiteServer};
use moqui_lib::error::Error;

use super::{run_against, ClientArgs, Server};
use crate::test_cmd::{report, slow_spec, TestComponent};

/// How long a child has to load or restore its database and start listening. A first load of Mantle's data is minutes.
const START_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a child has to stop once its standard input closes.
const STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// The lines of a child's output a failure shows.
const TAIL_LINES: usize = 30;

/// One declared server and the components whose client specs run against it.
struct Group {
    server: SuiteServer,
    components: Vec<TestComponent>,
}

/// Run every group against its own server, in the order of their first components.
pub(super) fn run_served(
    args: &ClientArgs,
    components: &[TestComponent],
    out_dir: &Path,
    print: &mut dyn FnMut(&str),
) -> Vec<report::ComponentSpec> {
    let mut results = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for component in components {
        // Offline specs have no server to declare. --offline runs them; --serve leaves them out.
        if !component.specs.is_empty() && component.specs.iter().all(|spec| spec_offline(&spec.source)) {
            print(&format!(
                "Skipping {}: its specs set server: false and run with --offline",
                component.name
            ));
            continue;
        }
        // Without --slow a slow spec is not run, so a component of only slow specs needs no server.
        if !args.slow && !component.specs.is_empty() && component.specs.iter().all(|spec| spec_slow(&spec.source)) {
            for spec in &component.specs {
                let result = slow_spec(spec);
                for case in &result.cases {
                    print(&report::case_line(&spec.name, case));
                }
                results.push(report::ComponentSpec {
                    component: component.name.clone(),
                    result,
                });
            }
            continue;
        }
        match declared_server(component) {
            Ok(server) => match groups.iter_mut().find(|group| group.server == server) {
                Some(group) => group.components.push(component.clone()),
                None => groups.push(Group {
                    server,
                    components: vec![component.clone()],
                }),
            },
            Err(err) => results.extend(failed_specs(std::slice::from_ref(component), &err.to_string(), print)),
        }
    }
    for (index, group) in groups.iter().enumerate() {
        let names = group.server.components.join(", ");
        let log = out_dir.join(format!("server-{}.log", index + 1));
        print(&format!(
            "Starting the server for {names}: moqui {}",
            child_args(&group.server, Path::new("<ready-file>"), args.slow).join(" ")
        ));
        let mut child = match start(&group.server, &log, index, args) {
            Ok(child) => child,
            Err(err) => {
                results.extend(failed_specs(&group.components, &err.to_string(), print));
                continue;
            }
        };
        match child.ready() {
            Ok(url) => {
                print(&format!(
                    "Server for {names}: {url} (pid {}, log {})",
                    child.process.id(),
                    log.display()
                ));
                let ran = Server::parse(&url, true)
                    .and_then(|server| run_against(args, server, Some(30.0), &group.components, out_dir, print));
                match ran {
                    Ok(ran) => results.extend(ran),
                    Err(err) => results.extend(failed_specs(&group.components, &err.to_string(), print)),
                }
            }
            Err(err) => results.extend(failed_specs(&group.components, &err.to_string(), print)),
        }
        child.stop();
    }
    results
}

/// The server a component's `test/client/suite.js` declares, its own component when it names none.
fn declared_server(component: &TestComponent) -> Result<SuiteServer, Error> {
    let suite = component.dir.join("test/client/suite.js");
    let declared = if suite.is_file() {
        let text = std::fs::read_to_string(&suite)
            .map_err(|err| Error::new(format!("could not read {}", suite.display())).with_source(err))?;
        suite_server(&text).map_err(|err| Error::new(format!("{}/test/client/suite.js: {err}", component.name)))?
    } else {
        None
    };
    let mut server = declared.ok_or_else(|| {
        Error::new(format!(
            "{} declares no server for --serve: add one to its test/client/suite.js, suite([...], {{ server: {{ components: [\"{}\"], specs: [...] }} }}), or run against a server with --server",
            component.name, component.name
        ))
    })?;
    if server.components.is_empty() {
        server.components.push(component.name.clone());
    }
    Ok(server)
}

/// The arguments of the child: `test`, the components, the specs, `--slow` when the client has it, and the serve
/// options.
fn child_args(server: &SuiteServer, ready_file: &Path, slow: bool) -> Vec<String> {
    let mut args = vec!["test".to_string()];
    args.extend(server.components.iter().cloned());
    if !server.specs.is_empty() {
        args.push("--spec".to_string());
        args.push(server.specs.join(","));
    }
    if slow {
        args.push("--slow".to_string());
    }
    for arg in ["--db", "snapshot", "--serve", "127.0.0.1:0", "--ready-file"] {
        args.push(arg.to_string());
    }
    args.push(ready_file.display().to_string());
    args
}

/// A started child, the pipe on its standard input, and the files it writes.
struct ServeChild {
    process: Child,
    stdin: Option<std::process::ChildStdin>,
    ready_file: PathBuf,
    log: PathBuf,
    names: String,
}

/// The browser the child renders PDF with: the one this client drives, when nothing names one for PDF
/// (`pdf_chromium_path` as a property, in the environment or in `runtime/.env`). The child's own search would leave out
/// Google Chrome, which PDF uses only when named, and the specs that check a PDF would wait for one that never comes.
fn pdf_browser(args: &ClientArgs) -> Option<PathBuf> {
    if !moqui_lib::env::get_prop_or_env("pdf_chromium_path").trim().is_empty() {
        return None;
    }
    let configured = args
        .chromium
        .clone()
        .or_else(|| std::env::var("MOQUI_CHROMIUM").ok())
        .unwrap_or_default();
    crate::chromium::find_test_browser(Some(&crate::runtime_framework_dirs().0), &configured).ok()
}

fn start(server: &SuiteServer, log: &Path, index: usize, args: &ClientArgs) -> Result<ServeChild, Error> {
    let names = server.components.join(", ");
    let ready_file = std::env::temp_dir().join(format!("moqui-serve-{}-{}.url", std::process::id(), index + 1));
    let _ = std::fs::remove_file(&ready_file);
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
    }
    let out = std::fs::File::create(log)
        .map_err(|err| Error::new(format!("could not create {}", log.display())).with_source(err))?;
    let err_out = out
        .try_clone()
        .map_err(|err| Error::new(format!("could not open {}", log.display())).with_source(err))?;
    let exe = std::env::current_exe()
        .map_err(|err| Error::new("could not find this binary to start the server").with_source(err))?;
    let mut command = Command::new(exe);
    command
        .args(child_args(server, &ready_file, args.slow))
        .stdin(Stdio::piped())
        .stdout(out)
        .stderr(err_out);
    if let Some(conf) = &server.conf {
        command.env("MOQUI_CONF", conf);
    }
    if let Some(browser) = pdf_browser(args) {
        command.env("pdf_chromium_path", browser);
    }
    let mut process = command
        .spawn()
        .map_err(|err| Error::new(format!("the server for {names} did not start")).with_source(err))?;
    let stdin = process.stdin.take();
    Ok(ServeChild {
        process,
        stdin,
        ready_file,
        log: log.to_path_buf(),
        names,
    })
}

impl ServeChild {
    /// The URL from the ready file, once the child writes it. An error with the end of its output when it exits first
    /// or does not start in time.
    fn ready(&mut self) -> Result<String, Error> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            if let Ok(text) = std::fs::read_to_string(&self.ready_file) {
                let url = text.trim();
                if !url.is_empty() {
                    return Ok(url.to_string());
                }
            }
            match self.process.try_wait() {
                Ok(Some(status)) => {
                    return Err(Error::new(format!(
                        "the server for {} exited before it served ({status}); the end of {}:\n{}",
                        self.names,
                        self.log.display(),
                        tail(&self.log)
                    )))
                }
                Ok(None) => {}
                Err(err) => {
                    return Err(
                        Error::new(format!("the server for {} could not be watched", self.names)).with_source(err),
                    )
                }
            }
            if Instant::now() >= deadline {
                return Err(Error::new(format!(
                    "the server for {} did not start in {} minutes; the end of {}:\n{}",
                    self.names,
                    START_TIMEOUT.as_secs() / 60,
                    self.log.display(),
                    tail(&self.log)
                )));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Close the pipe and wait for the child to stop; kill it when it does not in time.
    fn stop(mut self) {
        // the child's watcher reads end of file
        drop(self.stdin.take());
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            match self.process.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        tracing::warn!(
                            "The server for {} exited with {status}; see {}",
                            self.names,
                            self.log.display()
                        );
                    }
                    break;
                }
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                Ok(None) | Err(_) => {
                    tracing::warn!(
                        "The server for {} did not stop in {}s, killing it; its scratch runtime may be left in /tmp",
                        self.names,
                        STOP_TIMEOUT.as_secs()
                    );
                    let _ = self.process.kill();
                    let _ = self.process.wait();
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(&self.ready_file);
    }
}

/// The last lines of a log file.
fn tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    lines[start..]
        .iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Each spec of `components` failed in setup with `message`, as when its server does not start.
fn failed_specs(
    components: &[TestComponent],
    message: &str,
    print: &mut dyn FnMut(&str),
) -> Vec<report::ComponentSpec> {
    let mut results = Vec::new();
    for component in components {
        for spec in &component.specs {
            print(&format!(
                "{} setup FAILED: {}",
                spec.name,
                message.lines().next().unwrap_or("")
            ));
            results.push(report::ComponentSpec {
                component: component.name.clone(),
                result: SpecResult {
                    name: spec.name.clone(),
                    display: spec.display.clone(),
                    cases: Vec::new(),
                    setup_failure: Some(Failure {
                        kind: "error".to_string(),
                        block: "server".to_string(),
                        message: message.to_string(),
                        location: None,
                        stack: String::new(),
                        details: Vec::new(),
                        attachments: Vec::new(),
                    }),
                    cleanup_failure: None,
                    duration: Duration::ZERO,
                },
            });
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_child_runs_the_declared_specs_on_a_snapshot() {
        let server = SuiteServer {
            components: vec!["mr-mantle".to_string(), "mr-marble".to_string()],
            specs: vec!["A".to_string(), "B".to_string()],
            conf: None,
        };
        assert_eq!(
            child_args(&server, Path::new("/tmp/r.url"), false).join(" "),
            "test mr-mantle mr-marble --spec A,B --db snapshot --serve 127.0.0.1:0 --ready-file /tmp/r.url"
        );
        // --slow goes on to the server's own specs
        assert_eq!(
            child_args(&server, Path::new("/tmp/r.url"), true).join(" "),
            "test mr-mantle mr-marble --spec A,B --slow --db snapshot --serve 127.0.0.1:0 --ready-file /tmp/r.url"
        );
        let all = SuiteServer {
            components: vec!["c".to_string()],
            ..SuiteServer::default()
        };
        assert_eq!(
            child_args(&all, Path::new("r"), false).join(" "),
            "test c --db snapshot --serve 127.0.0.1:0 --ready-file r"
        );
    }
}
