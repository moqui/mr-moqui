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

//! `moqui test-load` as a process (plans/archive/LoadRunner.md §9, L2): option errors, its help, the script list of a
//! component, the refusal of a script that uses `service` (or a kind it does not know) and the run of one that uses
//! `ssh`, a run's result where Tools › Load finds it, and `--compare`.
//! The server is a stub that answers every request with 200; no test here runs the order to cash walk.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A directory removed on drop.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("mctl-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn test_load(runtime: &Path, args: &[&str]) -> (String, String, bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_moqui"));
    command
        .arg("test-load")
        .args(args)
        .current_dir(runtime)
        .env("MOQUI_RUNTIME", runtime);
    // nothing of the caller's shell: MOQUI_TEST_* would be params, and a proxy would take the stub's requests
    for (name, _) in std::env::vars() {
        if name.starts_with("MOQUI_TEST_")
            || name.eq_ignore_ascii_case("http_proxy")
            || name.eq_ignore_ascii_case("https_proxy")
            || name.eq_ignore_ascii_case("all_proxy")
        {
            command.env_remove(name);
        }
    }
    let output = command.output().unwrap();
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// Answers every request with 200, a GET of /missing with 404, until dropped.
struct Stub {
    stop: Arc<AtomicBool>,
    port: u16,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn stub() -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => answer(stream),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    });
    Stub { stop, port }
}

fn answer(mut stream: TcpStream) {
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    while !buf.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(count) => buf.extend_from_slice(&chunk[..count]),
        }
    }
    let status: &[u8] = if buf.starts_with(b"GET /missing") {
        b"404 Not Found"
    } else {
        b"200 OK"
    };
    let mut answer = b"HTTP/1.1 ".to_vec();
    answer.extend_from_slice(status);
    answer.extend_from_slice(b"\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    let _ = stream.write_all(&answer);
}

/// A runtime (it has base-component) with a component of two load scripts.
fn runtime(dir: &Path) -> PathBuf {
    let runtime = dir.join("runtime");
    std::fs::create_dir_all(runtime.join("base-component")).unwrap();
    let component = runtime.join("component/load-stub");
    std::fs::create_dir_all(component.join("test/load")).unwrap();
    std::fs::write(component.join("component.xml"), "<component name=\"load-stub\"/>\n").unwrap();
    std::fs::write(
        component.join("test/load/Ping.load.js"),
        r#"plan({
    title: "Ping", description: "One GET a user",
    params: { path: { default: "/ping", description: "What to get" }, password: { default: "x", secret: true } },
    thresholds: { errorRate: 0 },
})
scenario("pings", { users: 2, iterations: 2, run() { http.get(params.path, { name: "ping" }) } })
"#,
    )
    .unwrap();
    std::fs::write(
        component.join("test/load/Needs.load.js"),
        "plan({ title: \"Needs\", params: { token: { required: true } } })\nscenario(\"one\", { iterations: 1, run() {} })\n",
    )
    .unwrap();
    std::fs::write(
        component.join("test/load/InProcess.load.js"),
        "plan({ title: \"In process\", uses: [\"service\"] })\nscenario(\"calls\", { iterations: 1, run() {} })\n",
    )
    .unwrap();
    std::fs::write(
        component.join("test/load/Telnet.load.js"),
        "plan({ title: \"Telnet\", uses: [\"telnet\"] })\nscenario(\"one\", { iterations: 1, run() {} })\n",
    )
    .unwrap();
    // nothing listens on the port --ssh names: the connect is the run's one sample, an error
    std::fs::write(
        component.join("test/load/Shell.load.js"),
        "plan({ title: \"Shell\", uses: [\"ssh\"] })\nscenario(\"one\", { iterations: 1, run() { ssh.connect({ user: \"x\", password: \"y\", timeout: 5 }) } })\n",
    )
    .unwrap();
    runtime
}

#[test]
fn options_and_help() {
    let dir = temp("options");
    let runtime = runtime(&dir.0);
    let (_, err, ok) = test_load(&runtime, &[]);
    assert!(!ok);
    assert!(err.contains("moqui test-load needs a component"), "{err}");
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--users", "none"]);
    assert!(!ok);
    assert!(err.contains("--users is a whole number from 1, not 'none'"), "{err}");
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--bogus"]);
    assert!(!ok && err.contains("unknown option --bogus"), "{err}");
    let (_, err, ok) = test_load(&runtime, &["no-such-component"]);
    assert!(
        !ok && err.contains("no component, test/load directory or .load.js file named no-such-component"),
        "{err}"
    );
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--script", "Missing"]);
    assert!(!ok && err.contains("load-stub has no load script Missing"), "{err}");
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--compare", "a.json", "b.json"]);
    assert!(
        !ok && err.contains("--compare reads result files and runs nothing"),
        "{err}"
    );
    let (_, err, ok) = test_load(&runtime, &["x.load.js", "--script", "Ping"]);
    assert!(!ok && err.contains("--script names a script of a component"), "{err}");
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--log-level", "loud"]);
    assert!(
        !ok && err.contains("--log-level is trace, debug, info, warn or error, not 'loud'"),
        "{err}"
    );
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--timeout", "0"]);
    assert!(!ok && err.contains("--timeout is more than 0 seconds"), "{err}");
    let (out, _, ok) = test_load(&runtime, &["--help"]);
    assert!(ok, "{out}");
    assert!(
        out.starts_with("moqui test-load [component | path] [--script Name] [options]"),
        "{out}"
    );
    assert!(out.contains("--compare a.json b.json"), "{out}");
}

#[test]
fn a_component_lists_its_scripts_and_a_service_script_is_refused() {
    let dir = temp("list");
    let runtime = runtime(&dir.0);
    let (out, err, ok) = test_load(&runtime, &["load-stub"]);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("InProcess  component://load-stub/test/load/InProcess.load.js"),
        "{out}"
    );
    assert!(
        out.contains("Ping  component://load-stub/test/load/Ping.load.js"),
        "{out}"
    );
    assert!(out.contains("    Ping: One GET a user"), "{out}");
    assert!(out.contains("    scenario pings: 2 users 2 iterations"), "{out}");
    assert!(out.contains("    --param path=/ping  What to get"), "{out}");
    assert!(out.contains("    --param password=(secret)"), "{out}");
    // a required param with no value still lists, as required
    assert!(
        out.contains("Needs  component://load-stub/test/load/Needs.load.js"),
        "{out}"
    );
    assert!(out.contains("    --param token (required)"), "{out}");
    assert!(!out.contains("does not read"), "{out}");
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--script", "InProcess"]);
    assert!(!ok);
    assert!(
        err.contains("component://load-stub/test/load/InProcess.load.js uses service"),
        "{err}"
    );
    assert!(err.contains("Tools › Load"), "{err}");
    let (_, err, ok) = test_load(&runtime, &["load-stub", "--script", "Telnet"]);
    assert!(!ok);
    assert!(
        err.contains("Telnet.load.js uses telnet, which moqui test-load cannot make (it makes http and ssh)"),
        "{err}"
    );
    // a refused run leaves no run directory
    assert!(!runtime.join("tmp/test-load").exists());
    // a script that uses ssh runs, its connect going where --ssh says
    let server = stub();
    let url = format!("http://127.0.0.1:{}", server.port);
    let (out, err, ok) = test_load(
        &runtime,
        &[
            "load-stub",
            "--script",
            "Shell",
            "--server",
            &url,
            "--ssh",
            "127.0.0.1:1",
        ],
    );
    assert!(!ok, "no request succeeded: {out}{err}");
    assert!(!err.contains("cannot make"), "{err}");
    assert!(out.contains("ssh connect"), "{out}{err}");
    assert!(format!("{out}{err}").contains("x@127.0.0.1:1"), "{out}{err}");
}

#[test]
fn a_run_writes_its_result_where_tools_finds_it_and_compare_reads_two() {
    let dir = temp("run");
    let runtime = runtime(&dir.0);
    let server = stub();
    let url = format!("http://127.0.0.1:{}", server.port);
    let (out, err, ok) = test_load(
        &runtime,
        &[
            "load-stub",
            "--script",
            "Ping",
            "--server",
            &url,
            "--label",
            "stub",
            "--param",
            "password=y",
        ],
    );
    assert!(ok, "{out}{err}");
    let runs: Vec<PathBuf> = std::fs::read_dir(runtime.join("tmp/test-load"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let run_id = runs[0].file_name().unwrap().to_string_lossy().to_string();
    // <yyyyMMdd-HHmmss>-<pid>, as a run in the server is named
    let parts: Vec<&str> = run_id.split('-').collect();
    assert_eq!(parts.len(), 3, "{run_id}");
    assert_eq!((parts[0].len(), parts[1].len()), (8, 6), "{run_id}");
    let file = runs[0].join("result.json");
    assert!(out.contains(&format!("Result: {}", file.display())), "{out}");
    assert!(out.contains("2 users, "), "{out}");
    assert!(out.contains("ping "), "{out}");
    let result: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(result["requests"], 4);
    assert_eq!(result["errors"], 0);
    assert_eq!(result["passed"], true);
    assert_eq!(result["run"]["script"], "component://load-stub/test/load/Ping.load.js");
    assert_eq!(result["run"]["label"], "stub");
    assert_eq!(result["run"]["status"], "completed");
    assert_eq!(result["run"]["runId"], run_id.as_str());
    assert_eq!(result["run"]["generator"], "same-machine");
    assert_eq!(result["run"]["params"], serde_json::json!({ "path": "/ping" }));
    assert!(result["latency"].is_object() && result["names"]["ping"].is_object());

    // a file and --out
    let out_dir = dir.0.join("out");
    let (out, err, ok) = test_load(
        &runtime,
        &[
            runtime
                .join("component/load-stub/test/load/Ping.load.js")
                .to_str()
                .unwrap(),
            "--server",
            &url,
            "--users",
            "1",
            "--iterations",
            "3",
            "--out",
            out_dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "{out}{err}");
    let second: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("result.json")).unwrap()).unwrap();
    assert_eq!(second["requests"], 3);
    // --duration alone replaces the script's 2 iterations
    let timed_dir = dir.0.join("timed");
    let (out, err, ok) = test_load(
        &runtime,
        &[
            "load-stub",
            "--script",
            "Ping",
            "--server",
            &url,
            "--users",
            "1",
            "--duration",
            "0.5",
            "--out",
            timed_dir.to_str().unwrap(),
        ],
    );
    assert!(ok, "{out}{err}");
    let timed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(timed_dir.join("result.json")).unwrap()).unwrap();
    assert!(timed["requests"].as_u64().unwrap() > 2, "{timed}");
    assert_eq!(
        timed["run"]["scenarios"]["pings"]["iterations"],
        serde_json::Value::Null
    );
    // a broken threshold (errorRate 0) exits 1, with the result written
    let broken_dir = dir.0.join("broken");
    let (out, err, ok) = test_load(
        &runtime,
        &[
            "load-stub",
            "--script",
            "Ping",
            "--server",
            &url,
            "--param",
            "path=/missing",
            "--out",
            broken_dir.to_str().unwrap(),
        ],
    );
    assert!(!ok, "{out}");
    assert!(err.contains("no request succeeded"), "{err}");
    assert!(out.contains("threshold broken: errorRate 1 over 0"), "{out}");
    let broken: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(broken_dir.join("result.json")).unwrap()).unwrap();
    assert_eq!(broken["passed"], false);

    let (out, err, ok) = test_load(
        &runtime,
        &[
            "--compare",
            file.to_str().unwrap(),
            out_dir.join("result.json").to_str().unwrap(),
        ],
    );
    assert!(ok, "{out}{err}");
    let requests = out
        .lines()
        .find(|line| line.starts_with("requests"))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(requests.contains(" 4") && requests.contains("3 (-25.0%)"), "{out}");
    assert!(out.lines().any(|line| line.starts_with("ping count")), "{out}");
    let not_a_result = dir.0.join("other.json");
    std::fs::write(&not_a_result, "{\"a\": 1}").unwrap();
    let (_, err, ok) = test_load(
        &runtime,
        &["--compare", file.to_str().unwrap(), not_a_result.to_str().unwrap()],
    );
    assert!(!ok && err.contains("is not a load result"), "{err}");
}

/// A server that answers every request with 200 at once on keep-alive connections, a thread for each, until the
/// process ends: as little server as there can be, so a run's rate is the generator's.
fn fast_stub() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = Vec::new();
                let mut chunk = [0_u8; 8192];
                loop {
                    // each request head is answered; a GET has no body
                    while let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                        buf.drain(..end + 4);
                        if stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok")
                            .is_err()
                        {
                            return;
                        }
                    }
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return,
                        Ok(count) => buf.extend_from_slice(&chunk[..count]),
                    }
                }
            });
        }
    });
    port
}

/// The generator's ceiling (plans/archive/LoadRunner.md §16.1): requests a second of `moqui test-load` for a script of
/// one GET against [`fast_stub`], at 1 to 256 users for 10 seconds each, with the generator's CPU share and the share
/// of the users' time in script. A measurement, not a check: it prints a table, and asserts only that each run made
/// requests with no errors. Build the release binary first for numbers worth keeping (`cargo test --release`).
#[test]
#[ignore = "a measurement of about a minute: cargo test --release -p moqui --test cli -- --ignored load_ceiling --nocapture"]
fn load_ceiling() {
    let dir = temp("ceiling");
    let runtime = runtime(&dir.0);
    std::fs::write(
        runtime.join("component/load-stub/test/load/Ceiling.load.js"),
        "plan({ title: \"Ceiling\" })\nscenario(\"get\", { users: 1, duration: 10, run() { http.get(\"/x\", { name: \"get\" }) } })\n",
    )
    .unwrap();
    let url = format!("http://127.0.0.1:{}", fast_stub());
    println!("users  requests/s  per core  p50 ms  p99 ms  generator cpu  script share");
    for users in ["1", "4", "16", "64", "256"] {
        let out = dir.0.join(format!("out-{users}"));
        let (stdout, stderr, ok) = test_load(
            &runtime,
            &[
                "load-stub",
                "--script",
                "Ceiling",
                "--server",
                &url,
                "--users",
                users,
                "--ramp-up",
                "1",
                "--duration",
                "10",
                "--out",
                out.to_str().unwrap(),
            ],
        );
        assert!(ok, "{stdout}{stderr}");
        let result: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("result.json")).unwrap()).unwrap();
        assert_eq!(result["errors"], 0, "{stdout}");
        let rps = result["rps"].as_f64().unwrap();
        let cores = result["generator"]["cores"].as_f64().unwrap_or(1.0);
        let cpu = result["generator"]["cpu"].as_f64().unwrap_or(f64::NAN);
        println!(
            "{users:>5}  {rps:>10.0}  {:>8.0}  {:>6.2}  {:>6.2}  {:>13.3}  {:>12}",
            rps / cores,
            result["latency"]["p50"].as_f64().unwrap_or(f64::NAN),
            result["latency"]["p99"].as_f64().unwrap_or(f64::NAN),
            cpu,
            result["generator"]["scriptShare"]
        );
    }
}
