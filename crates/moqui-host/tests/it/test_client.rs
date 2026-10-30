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

//! `moqui test-client` against the seeded server: the fixture component's client specs
//! (`tests/fixtures/test-component/test/client`) over HTTP, logins, the session token with and without, redirects off,
//! raw requests and WebSocket, with a skipped spec and JUnit; then a spec that fails on purpose, which fails the run
//! with its line. The runner runs in this process, as `moqui test-client` runs it; the binary's own run, with the
//! runtime files it must leave alone, is `crates/moqui/tests/cli/test_client.rs`.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum", feature = "test-client"))]

use std::path::{Path, PathBuf};

use moqui_host::test_client::{parse_client_args, run};

use crate::common::{start, temp_dir, DirGuard};

fn fixture_component() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-component")
}

/// The report lines of a run, and whether it passed.
fn run_client(args: &[&str]) -> (String, Result<(), String>) {
    let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    let parsed = parse_client_args(&owned).unwrap();
    let mut lines = Vec::new();
    let result = run(&parsed, &mut |line| lines.push(line.to_string())).map_err(|err| err.to_string());
    (lines.join("\n"), result)
}

#[test]
fn client_specs_run_against_a_server_and_a_failure_fails_the_run() {
    if !crate::common::own_process() {
        return;
    }
    let gate = start("test-client");
    let server = format!("http://{}", gate.server.address());
    let out = temp_dir("moqui-test-client-out");
    let _out = DirGuard(out.clone());
    let junit = out.join("junit");
    let files = out.join("files");
    let specs = fixture_component().join("test/client");
    let (text, result) = run_client(&[
        specs.to_str().unwrap(),
        "--server",
        &server,
        "--param",
        "greeting=hello",
        "--junit",
        junit.to_str().unwrap(),
        "--out",
        files.to_str().unwrap(),
    ]);
    assert!(result.is_ok(), "{result:?}\n{text}");
    let lines: Vec<&str> = text.lines().filter(|line| line.contains(" > ")).collect();
    // suite.js order
    assert!(
        lines[0].starts_with(
            "Http > an anonymous GET /Login is HTML with a session cookie that is HttpOnly and SameSite=Lax  PASSED"
        ),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Http > redirects are off unless asked: \"/apps/system/dashboard\"  PASSED")),
        "{text}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with(
            "Sessions > a POST runs with the session token and stores nothing without it (csrf: false)  PASSED"
        )),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("WebSocket > a logged-in session opens /notws, sends and closes  PASSED")),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Skipped > needs the fixture user  SKIPPED")
                && line.ends_with("the moqui.test.no.fixture user is not on this server")),
        "{text}"
    );
    // Served needs the fixtures of the suite's declared server (Basics), which this server has not: it skips
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Served > the enum type Basics made is there  SKIPPED")),
        "{text}"
    );
    // a tiny load: its table after its line, and its JSON
    let load_line = lines
        .iter()
        .position(|line| line.starts_with("Load > a tiny load times its requests by name  PASSED"))
        .unwrap_or_else(|| panic!("{text}"));
    let after: Vec<&str> = text
        .lines()
        .skip_while(|line| !line.starts_with("Load > a tiny load"))
        .skip(1)
        .take(4)
        .collect();
    assert!(load_line > 0 && after[0].starts_with("    2 users, "), "{text}");
    assert!(
        after[0].ends_with(": 12 requests, ") || after[0].contains(": 12 requests, "),
        "{text}"
    );
    assert!(
        after[1].starts_with("    name ") && after[2].starts_with("    enum types "),
        "{text}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("Load > parallel threads log in on sessions of their own  PASSED")),
        "{text}"
    );
    let json =
        std::fs::read_to_string(files.join("load/moqui-test-fixture.Load.a-tiny-load-times-its-requests-by-name.json"))
            .unwrap();
    let stats: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(stats["requests"], 12, "{json}");
    assert_eq!(stats["run"]["spec"], "Load", "{json}");
    let xml = std::fs::read_to_string(junit.join("TEST-moqui-test-fixture.client.Load.xml")).unwrap();
    assert!(xml.contains("<system-out>2 users, "), "{xml}");
    assert!(text.contains(" failed, 0 ignored, 3 skipped; 6 specs in "), "{text}");
    assert!(text.contains(" 0 failed, "), "{text}");
    assert!(text.contains(&format!("Output: {}", files.display())), "{text}");
    assert!(files.join("login.html").is_file());
    let xml = std::fs::read_to_string(junit.join("TEST-moqui-test-fixture.client.Sessions.xml")).unwrap();
    assert!(
        xml.contains("<testsuite name=\"moqui-test-fixture.client.Sessions\""),
        "{xml}"
    );
    assert!(xml.contains("failures=\"0\" errors=\"0\" skipped=\"0\""), "{xml}");
    let skipped = std::fs::read_to_string(junit.join("TEST-moqui-test-fixture.client.Skipped.xml")).unwrap();
    assert!(skipped.contains("skipped=\"1\""), "{skipped}");

    // A spec that fails: the run is an error, with the feature's line, and the features after it still run.
    let failing = temp_dir("moqui-test-client-failing");
    let _failing = DirGuard(failing.clone());
    std::fs::create_dir_all(failing.join("test/client")).unwrap();
    std::fs::write(
        failing.join("component.xml"),
        "<component name=\"moqui-test-failing\" version=\"1.0.0\"/>\n",
    )
    .unwrap();
    copy(
        &fixture_component().join("failing-client/Failing.spec.js"),
        &failing.join("test/client/Failing.spec.js"),
    );
    let (text, result) = run_client(&[
        failing.join("test/client/Failing.spec.js").to_str().unwrap(),
        "--server",
        &server,
        "--out",
        files.to_str().unwrap(),
    ]);
    assert_eq!(result, Err("1 test failure(s)".to_string()), "{text}");
    assert!(text.contains("Failing > passes first  PASSED"), "{text}");
    assert!(
        text.contains("Failing > a false condition over HTTP  FAILED (")
            && text.contains("moqui-test-failing/test/client/Failing.spec.js:6"),
        "{text}"
    );
    assert!(text.contains("    res.status == 404\n"), "{text}");
    assert!(text.contains("Failing > a skip is not a failure  SKIPPED"), "{text}");
    assert!(
        text.contains("3 features: 1 passed, 1 failed, 0 ignored, 1 skipped; 1 specs in "),
        "{text}"
    );
    // --fail-on-skip fails a run that only skips
    let (text, result) = run_client(&[
        specs.join("Skipped.spec.js").to_str().unwrap(),
        "--server",
        &server,
        "--fail-on-skip",
        "--out",
        files.to_str().unwrap(),
    ]);
    assert_eq!(result, Err("1 skipped, and --fail-on-skip".to_string()), "{text}");

    // A browser spec with no Chromium to start skips each feature, naming why; --fail-on-skip fails it.
    let browser = temp_dir("moqui-test-client-browser");
    let _browser = DirGuard(browser.clone());
    std::fs::create_dir_all(browser.join("test/client")).unwrap();
    std::fs::write(
        browser.join("component.xml"),
        "<component name=\"moqui-test-browser\" version=\"1.0.0\"/>\n",
    )
    .unwrap();
    copy(
        &fixture_component().join("browser-client/Browser.spec.js"),
        &browser.join("test/client/Browser.spec.js"),
    );
    let browser_spec = browser.join("test/client/Browser.spec.js");
    let no_browser = [
        browser_spec.to_str().unwrap(),
        "--server",
        &server,
        "--chromium",
        "/nowhere/chromium",
        "--out",
        files.to_str().unwrap(),
    ];
    let (text, result) = run_client(&no_browser);
    assert!(result.is_ok(), "{result:?}\n{text}");
    assert!(
        text.lines().any(|line| line
            .starts_with("Browser > the login form signs in with fill, click and waitForURL  SKIPPED")
            && line
                .ends_with("no browser: --chromium (or MOQUI_CHROMIUM) names /nowhere/chromium, which is not a file")),
        "{text}"
    );
    assert!(
        text.contains("8 features: 0 passed, 0 failed, 0 ignored, 8 skipped"),
        "{text}"
    );
    let mut strict = no_browser.to_vec();
    strict.push("--fail-on-skip");
    let (text, result) = run_client(&strict);
    assert_eq!(result, Err("8 skipped, and --fail-on-skip".to_string()), "{text}");
}

fn copy(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap();
}
