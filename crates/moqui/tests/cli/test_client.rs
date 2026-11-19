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

//! `moqui test-client` as a process: option errors, nothing answering at the server, `--wait` for a server that
//! starts later, JUnit, a failing spec's exit status and line, and the runtime files a client must leave alone
//! (`.env`, the log and the postmaster lock). The server is a stub that answers every request; the client specs over a
//! real server are `crates/moqui-host/tests/it/test_client.rs`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// A directory removed on drop.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(tag: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!("mctc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(dir)
}

fn test_client(cwd: &Path, runtime: &Path, args: &[&str]) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_moqui"))
        .arg("test-client")
        .args(args)
        .current_dir(cwd)
        .env("MOQUI_RUNTIME", runtime)
        .env_remove("MOQUI_TEST_SERVER")
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// A port nothing listens on, for now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Answers every request with 200 and a session cookie, after `delay`, until dropped.
struct Stub {
    stop: Arc<AtomicBool>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn stub(port: u16, delay: Duration) -> Stub {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        while !stopped.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => answer(stream),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    });
    Stub { stop }
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
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nSet-Cookie: stub.session=1; Path=/; HttpOnly\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    );
}

/// A component with two client specs, one that passes and one that fails.
fn component(dir: &Path) -> PathBuf {
    let component = dir.join("stub-component");
    std::fs::create_dir_all(component.join("test/client")).unwrap();
    std::fs::write(
        component.join("component.xml"),
        "<component name=\"stub-component\"/>\n",
    )
    .unwrap();
    std::fs::write(
        component.join("test/client/Passing.spec.js"),
        "feature('the stub answers', {\n    when() { return { res: http.get('/', { params: { q: params.q } }) } },\n    then({ res }) {\n        res.status == 200\n        res.text == 'ok'\n        res.cookie('stub.session').httpOnly\n        res.save('answer.txt').endsWith('answer.txt')\n    },\n})\n",
    )
    .unwrap();
    std::fs::write(
        component.join("test/client/Failing.spec.js"),
        "feature('a false condition', {\n    expect() {\n        http.get('/').text == 'not ok'\n    },\n})\n",
    )
    .unwrap();
    component
}

#[test]
fn option_errors_name_the_option() {
    let dir = temp("options");
    for (args, says) in [
        (&["--bogus"][..], "unknown option --bogus"),
        (&["--param", "novalue"][..], "--param needs name=value"),
        (&["--wait"][..], "--wait needs a value"),
        (
            &["--server", "http://erp.example.com"][..],
            "plain http sends logins over the network",
        ),
        (
            &["--serve", "--server", "http://localhost:8080"][..],
            "--serve starts the servers the suites declare: leave out --server, --ssh and --wait",
        ),
        (
            &["no-such-component"][..],
            "component no-such-component has no test/client/*.spec.js files",
        ),
    ] {
        let (out, err, ok) = test_client(&dir.0, &dir.0, args);
        assert!(!ok, "{args:?}: {out}");
        assert!(err.contains(says), "{args:?}: {err}");
    }
    let (out, _, ok) = test_client(&dir.0, &dir.0, &["--help"]);
    assert!(
        ok && out.starts_with("moqui test-client [component | path ...] [options]"),
        "{out}"
    );
}

#[test]
fn nothing_answering_ends_the_run_with_what_to_do() {
    let dir = temp("nothing");
    let specs = component(&dir.0).join("test/client");
    let port = free_port();
    let server = format!("http://127.0.0.1:{port}");
    let (out, err, ok) = test_client(&dir.0, &dir.0, &[specs.to_str().unwrap(), "--server", &server]);
    assert!(!ok, "{out}");
    assert!(
        err.contains(&format!(
            "nothing answers at {server}: start a server (./moq run), or pass --server or --serve"
        )),
        "{err}"
    );
    assert!(!out.contains(" > "), "no spec ran: {out}");
}

#[cfg(unix)]
#[test]
fn wait_finds_a_late_server_and_the_runtime_files_are_untouched() {
    let dir = temp("wait");
    let specs = component(&dir.0).join("test/client");
    // A runtime the client must not touch, as when it runs beside ./moq run.
    let runtime = dir.0.join("runtime");
    std::fs::create_dir_all(runtime.join("components/base")).unwrap();
    std::fs::create_dir_all(runtime.join("log")).unwrap();
    std::fs::create_dir_all(runtime.join("db/embedded/data")).unwrap();
    let kept = [
        (
            runtime.join(".env"),
            "moqui_conf__webapp_list__webapp__WEBROOT__http_port=8080\n",
        ),
        (runtime.join("log/moqui.log"), "a line of the server\n"),
        (runtime.join("db/embedded/data/postmaster.pid"), "4242\n"),
    ];
    for (path, text) in &kept {
        std::fs::write(path, text).unwrap();
    }
    let before: Vec<SystemTime> = kept
        .iter()
        .map(|(path, _)| std::fs::metadata(path).unwrap().modified().unwrap())
        .collect();
    let port = free_port();
    let _stub = stub(port, Duration::from_millis(1500));
    let server = format!("http://127.0.0.1:{port}");
    let junit = dir.0.join("junit");
    let (out, err, ok) = test_client(
        &dir.0,
        &runtime,
        &[
            specs.join("Passing.spec.js").to_str().unwrap(),
            "--server",
            &server,
            "--wait",
            "30",
            "--param",
            "q=1",
            "--junit",
            junit.to_str().unwrap(),
        ],
    );
    assert!(ok, "stdout:\n{out}\nstderr:\n{err}");
    assert!(out.contains("Passing > the stub answers  PASSED ("), "{out}");
    assert!(
        out.contains("1 features: 1 passed, 0 failed, 0 ignored, 0 skipped; 1 specs in "),
        "{out}"
    );
    let xml = std::fs::read_to_string(junit.join("TEST-stub-component.client.Passing.xml")).unwrap();
    assert!(
        xml.contains("tests=\"1\" failures=\"0\" errors=\"0\" skipped=\"0\""),
        "{xml}"
    );
    // the saved body is under runtime/tmp/test-client/<run>, the only place the client writes in the runtime
    let output_line = out.lines().find(|line| line.starts_with("Output: ")).expect(&out);
    let output = PathBuf::from(output_line.trim_start_matches("Output: "));
    assert!(output.starts_with(runtime.join("tmp/test-client")), "{output_line}");
    assert_eq!(std::fs::read_to_string(output.join("answer.txt")).unwrap(), "ok");

    // A failing spec exits 1 with its line.
    let (out, err, ok) = test_client(
        &dir.0,
        &runtime,
        &[specs.join("Failing.spec.js").to_str().unwrap(), "--server", &server],
    );
    assert!(!ok, "{out}");
    assert!(
        out.contains("Failing > a false condition  FAILED (")
            && out.contains("stub-component/test/client/Failing.spec.js:3"),
        "{out}"
    );
    assert!(err.contains("1 test failure(s)"), "{err}");

    for ((path, text), modified) in kept.iter().zip(before) {
        assert_eq!(std::fs::read_to_string(path).unwrap(), *text, "{}", path.display());
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified,
            "{}",
            path.display()
        );
    }
    let mut entries: Vec<String> = std::fs::read_dir(&runtime)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec![".env", "components", "db", "log", "tmp"],
        "nothing else is made in the runtime"
    );
    assert_eq!(
        std::fs::read_dir(runtime.join("log")).unwrap().count(),
        1,
        "no new log file"
    );
}

/// A run of the security proofs against the stub: the summary by requirement joins the matrix the proofs' suite.js
/// names, a server the run did not start is the deployment profile (a feature that needs the fixtures skips), a proof's
/// path runs only with --proofs, and a passing feature filed under no requirement fails the run.
#[cfg(unix)]
#[test]
fn proofs_write_the_summary_by_requirement() {
    let dir = temp("proofs");
    let component = dir.0.join("stub-component");
    let proofs = component.join("test/proofs");
    std::fs::create_dir_all(&proofs).unwrap();
    std::fs::write(
        component.join("component.xml"),
        "<component name=\"stub-component\"/>\n",
    )
    .unwrap();
    std::fs::write(
        component.join("Matrix.md"),
        "| Requirement | L | Topic | Status | Evidence | Proof |\n| --- | --- | --- | --- | --- | --- |\n\
         | v5.0.0-3.4.1 | 1 | The stub answers | Framework | stub | - |\n\
         | v5.0.0-6.3.2 | 1 | A fixture user | Framework | stub | - |\n\
         | v5.0.0-7.2.1 | 1 | Not proved here | Gap | stub | - |\n",
    )
    .unwrap();
    std::fs::write(proofs.join("suite.js"), "suite(['V3Stub'], { matrix: 'Matrix.md' })\n").unwrap();
    let filed = "feature('v5.0.0-3.4.1: the stub answers', {\n    expect() {\n        http.get('/').text == 'ok'\n    },\n})\n\
                 feature('v5.0.0-6.3.2: a fixture user signs in', {\n    expect() {\n        needsFixtures('the stub user')\n        profile == 'fixtures'\n    },\n})\n";
    std::fs::write(proofs.join("V3Stub.spec.js"), filed).unwrap();
    let port = free_port();
    let _stub = stub(port, Duration::ZERO);
    let server = format!("http://127.0.0.1:{port}");
    let out = dir.0.join("out");
    let run = |extra: &[&str]| {
        let mut args = vec![
            proofs.to_str().unwrap(),
            "--server",
            &server,
            "--wait",
            "30",
            "--out",
            out.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        test_client(&dir.0, &dir.0, &args)
    };
    let (stdout, err, ok) = run(&["--proofs"]);
    assert!(ok, "stdout:\n{stdout}\nstderr:\n{err}");
    assert!(
        stdout.contains("V3Stub > v5.0.0-3.4.1: the stub answers  PASSED ("),
        "{stdout}"
    );
    assert!(
        stdout.contains("V3Stub > v5.0.0-6.3.2: a fixture user signs in  SKIPPED (")
            && stdout.contains("needs the stub user, a fixture of the project's run, and the profile is deployment"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "Proofs of stub-component: 2 requirements with proofs: 1 passed, 0 partial, 0 failed, 1 skipped; 3 requirements in its matrix"
        ),
        "{stdout}"
    );
    let markdown = std::fs::read_to_string(out.join("proofs.md")).unwrap();
    assert!(
        markdown.contains(
            "| v5.0.0-3.4.1 | 1 | The stub answers | Framework | passed | V3Stub › the stub answers: passed |"
        ),
        "{markdown}"
    );
    assert!(
        markdown.contains("| v5.0.0-7.2.1 | 1 | Not proved here | Gap | none | - |"),
        "{markdown}"
    );
    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("proofs.json")).unwrap()).unwrap();
    assert_eq!(summary["profile"], "deployment");
    assert_eq!(summary["server"], server.as_str());
    assert_eq!(summary["components"][0]["counts"]["withProofs"], 2);
    assert_eq!(summary["components"][0]["requirements"][1]["result"], "skipped");

    // --profile fixtures: the feature that needs them runs
    let (stdout, _, ok) = run(&["--proofs", "--profile", "fixtures"]);
    assert!(
        ok && stdout.contains("V3Stub > v5.0.0-6.3.2: a fixture user signs in  PASSED ("),
        "{stdout}"
    );

    // a proof's path without --proofs is refused
    let (stdout, err, ok) = run(&[]);
    assert!(
        !ok && err.contains("is a proof: run it with --proofs"),
        "stdout:\n{stdout}\nstderr:\n{err}"
    );

    // a passing feature filed under no requirement fails the run, and is named
    std::fs::write(
        proofs.join("V3Stub.spec.js"),
        format!("{filed}feature('A05 the stub answers again', {{\n    expect() {{\n        http.get('/').status == 200\n    }},\n}})\n"),
    )
    .unwrap();
    let (stdout, err, ok) = run(&["--proofs"]);
    assert!(!ok, "{stdout}");
    assert!(
        stdout.contains("FAILED: features of test/proofs filed under no requirement")
            && stdout.contains("  stub-component/V3Stub > A05 the stub answers again"),
        "{stdout}"
    );
    assert!(err.contains("1 test failure(s)"), "{err}");
}
