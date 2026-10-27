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

//! `moqui test-client --serve` (plans/archive/ClientTestHarness.md 6) over the fixture component: the server its
//! `test/client/suite.js` declares starts as a child `moqui test --serve` on a scratch runtime, with the data of its
//! Basics spec and its `test/MoquiConf.xml`; the client specs run against it; then it stops and its scratch runtime
//! goes. Killing the client stops the child too, through the pipe on its standard input. A server that does not start
//! fails only the specs of its suite, and a suite that declares none fails with what to add.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::repo;
use super::test::{fixture_component, test_runtime, TestRuntime};

fn client_command(runtime: &TestRuntime, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_moqui"));
    command
        .arg("test-client")
        .args(args)
        .current_dir(repo())
        .env("MOQUI_RUNTIME", &runtime.dir)
        .env("MOQUI_FRAMEWORK", repo().join("runtime/base-component/framework"))
        .env("MOQUI_INSTALL", repo().join("runtime/install"))
        .env_remove("MOQUI_TEST_SERVER")
        .env_remove("MOQUI_CONF");
    command
}

/// The pid of the child from the client's `Server for <names>: <url> (pid N, log <path>)` line.
fn server_pid(line: &str) -> Option<u32> {
    let rest = line.split("(pid ").nth(1)?;
    rest.split(',').next()?.parse().ok()
}

fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| stat.contains(") Z "))
}

/// Wait until the process is gone.
fn wait_gone(pid: u32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// The scratch runtimes `moqui test` process `pid` made under /tmp (`mt-<pid>-<nanos>`) that are still there.
fn scratch_left(pid: u32) -> Vec<PathBuf> {
    std::fs::read_dir("/tmp")
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(&format!("mt-{pid}-")))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
#[test]
fn serve_starts_the_declared_server_runs_the_specs_and_stops_it() {
    let Some(runtime) = test_runtime("serve", &[("moqui-test-fixture", &fixture_component())]) else {
        return;
    };
    let out_dir = runtime.dir.join("client-out");
    let junit = runtime.dir.join("junit");
    let output = client_command(
        &runtime,
        &[
            "moqui-test-fixture",
            "--serve",
            "--param",
            "greeting=hello",
            "--out",
            out_dir.to_str().unwrap(),
            "--junit",
            junit.to_str().unwrap(),
        ],
    )
    .output()
    .unwrap();
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    let log = std::fs::read_to_string(out_dir.join("server-1.log")).unwrap_or_default();
    assert!(
        output.status.success(),
        "stdout:\n{out}\nstderr:\n{err}\nserver log:\n{log}"
    );
    assert!(
        out.contains(
            "Starting the server for moqui-test-fixture: moqui test moqui-test-fixture --spec Basics --db snapshot --serve 127.0.0.1:0 --ready-file"
        ),
        "{out}"
    );
    let line = out
        .lines()
        .find(|line| line.starts_with("Server for moqui-test-fixture: http://127.0.0.1:"))
        .unwrap_or_else(|| panic!("{out}"));
    let pid = server_pid(line).unwrap_or_else(|| panic!("{line}"));
    // the fixture data of Basics and the component's test/MoquiConf.xml, over HTTP
    assert!(
        out.contains("Served > the enum type Basics made is there  PASSED"),
        "{out}"
    );
    assert!(
        out.contains("Served > the component's test/MoquiConf.xml is merged  PASSED"),
        "{out}"
    );
    assert!(out.contains("Http > an anonymous GET /Login is HTML"), "{out}");
    assert!(out.contains(" 0 failed, 0 ignored, 1 skipped; 6 specs in "), "{out}");
    assert!(junit.join("TEST-moqui-test-fixture.client.Served.xml").is_file());
    // the child ran Basics (and only it) before it served, and stopped when the client was done
    assert!(
        log.contains("Basics > a component service with a pinned id and clock  PASSED"),
        "{log}"
    );
    assert!(!log.contains("Dependent >"), "{log}");
    assert!(log.contains("Serving http://127.0.0.1:"), "{log}");
    assert!(log.contains("Stopped"), "{log}");
    assert!(
        wait_gone(pid, Duration::from_secs(30)),
        "the server {pid} is still running"
    );
    assert!(scratch_left(pid).is_empty(), "{:?}", scratch_left(pid));
}

#[cfg(target_os = "linux")]
#[test]
fn killing_the_client_stops_the_server() {
    // A client spec that waits, so the client is still running when it is killed.
    let slow = PathBuf::from("/tmp").join(format!("mct-slow-component-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&slow);
    std::fs::create_dir_all(slow.join("test/client")).unwrap();
    std::fs::write(slow.join("component.xml"), "<component name=\"moqui-test-slow\"/>\n").unwrap();
    std::fs::write(
        slow.join("test/client/suite.js"),
        "suite([\"Slow\"], { server: { components: [\"moqui-test-fixture\"], specs: [\"Basics\"] } })\n",
    )
    .unwrap();
    std::fs::write(
        slow.join("test/client/Slow.spec.js"),
        "feature('waits', { expect() { sleep(120000); true } })\n",
    )
    .unwrap();
    let Some(runtime) = test_runtime(
        "serve-kill",
        &[("moqui-test-fixture", &fixture_component()), ("moqui-test-slow", &slow)],
    ) else {
        let _ = std::fs::remove_dir_all(&slow);
        return;
    };
    let out_dir = runtime.dir.join("client-out");
    let mut client = client_command(
        &runtime,
        &["moqui-test-slow", "--serve", "--out", out_dir.to_str().unwrap()],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let stdout = client.stdout.take().unwrap();
    let mut seen = Vec::new();
    let mut pid = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if line.starts_with("Server for moqui-test-fixture: ") {
            pid = server_pid(&line);
            break;
        }
        seen.push(line);
    }
    let Some(pid) = pid else {
        let _ = client.kill();
        let _ = client.wait();
        let log = std::fs::read_to_string(out_dir.join("server-1.log")).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&slow);
        panic!("the server did not start:\n{}\nserver log:\n{log}", seen.join("\n"));
    };
    assert!(alive(pid));
    // SIGKILL: the client cleans nothing up, and the child sees the end of its standard input
    client.kill().unwrap();
    let _ = client.wait();
    let gone = wait_gone(pid, Duration::from_secs(120));
    let log = std::fs::read_to_string(out_dir.join("server-1.log")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&slow);
    assert!(gone, "the server {pid} still runs after the client was killed:\n{log}");
    assert!(log.contains("Stopped"), "{log}");
    assert!(scratch_left(pid).is_empty(), "{:?}", scratch_left(pid));
}

/// A component under /tmp with one client spec and the given `test/client/suite.js`.
fn client_component(tag: &str, name: &str, suite: &str, spec: &str) -> PathBuf {
    let dir = PathBuf::from("/tmp").join(format!("mct-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("test/client")).unwrap();
    std::fs::write(dir.join("component.xml"), format!("<component name=\"{name}\"/>\n")).unwrap();
    std::fs::write(dir.join("test/client/suite.js"), suite).unwrap();
    std::fs::write(dir.join("test/client/Probe.spec.js"), spec).unwrap();
    dir
}

#[cfg(target_os = "linux")]
#[test]
fn a_server_that_does_not_start_fails_its_suite_only() {
    let probe = "feature('the server answers', { expect() { http.get('/Login').status == 200 } })\n";
    let broken = client_component(
        "broken-component",
        "moqui-test-broken",
        "suite([\"Probe\"], { server: { components: [\"moqui-test-fixture\"], specs: [\"NoSuchSpec\"] } })\n",
        probe,
    );
    let working = client_component(
        "working-component",
        "moqui-test-working",
        "suite([\"Probe\"], { server: { components: [\"moqui-test-fixture\"], specs: [\"Basics\"] } })\n",
        probe,
    );
    let undeclared = client_component(
        "undeclared-component",
        "moqui-test-undeclared",
        "suite([\"Probe\"])\n",
        probe,
    );
    let Some(runtime) = test_runtime(
        "serve-broken",
        &[
            ("moqui-test-fixture", &fixture_component()),
            ("moqui-test-broken", &broken),
            ("moqui-test-working", &working),
            ("moqui-test-undeclared", &undeclared),
        ],
    ) else {
        for dir in [&broken, &working, &undeclared] {
            let _ = std::fs::remove_dir_all(dir);
        }
        return;
    };
    let out_dir = runtime.dir.join("client-out");
    let output = client_command(
        &runtime,
        &[
            "moqui-test-broken",
            "moqui-test-working",
            "moqui-test-undeclared",
            "--serve",
            "--out",
            out_dir.to_str().unwrap(),
        ],
    )
    .output()
    .unwrap();
    for dir in [&broken, &working, &undeclared] {
        let _ = std::fs::remove_dir_all(dir);
    }
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(!output.status.success(), "{out}");
    // the broken server's suite fails with the end of the child's output; the working one still runs
    assert!(
        out.contains("Probe setup FAILED: the server for moqui-test-fixture exited before it served (exit status: 1)"),
        "{out}"
    );
    assert!(out.contains("no component has a spec NoSuchSpec"), "{out}");
    assert!(out.contains("Probe > the server answers  PASSED"), "{out}");
    assert!(
        out.contains("Probe setup FAILED: moqui-test-undeclared declares no server for --serve"),
        "{out}"
    );
    assert!(
        out.contains(
            "1 features: 1 passed, 0 failed, 0 ignored, 0 skipped; 3 specs, 2 spec setup or cleanup failures in "
        ),
        "{out}"
    );
}
