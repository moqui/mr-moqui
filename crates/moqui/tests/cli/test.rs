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

//! `moqui test` over the fixture component (`crates/moqui-host/tests/fixtures/test-component`) on a runtime made for
//! the test: the passing suite, `--spec` with `dependsOn`, JUnit, and a spec that fails on purpose.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::repo;

/// A runtime under `/tmp` (the Postgres socket path must be short) with the repo's conf, `base-component`, and the
/// given components linked. It uses the repo's shared Postgres install through `MOQUI_INSTALL`. Removed on drop.
pub(super) struct TestRuntime {
    pub(super) dir: PathBuf,
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(super) fn fixture_component() -> PathBuf {
    repo().join("crates/moqui-host/tests/fixtures/test-component")
}

/// None when the shared Postgres install is not unpacked yet: the test is skipped rather than unpacking one.
pub(super) fn test_runtime(tag: &str, components: &[(&str, &Path)]) -> Option<TestRuntime> {
    if !repo().join("runtime/install/postgres").is_dir() {
        eprintln!("skipped: no runtime/install/postgres");
        return None;
    }
    let dir = PathBuf::from("/tmp").join(format!("mct-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("component")).unwrap();
    std::fs::create_dir_all(dir.join("conf")).unwrap();
    std::fs::create_dir_all(dir.join("db")).unwrap();
    for entry in std::fs::read_dir(repo().join("runtime/conf")).unwrap().flatten() {
        if entry.path().is_file() {
            std::fs::copy(entry.path(), dir.join("conf").join(entry.file_name())).unwrap();
        }
    }
    std::os::unix::fs::symlink(repo().join("runtime/base-component"), dir.join("base-component")).unwrap();
    for (name, path) in components {
        std::os::unix::fs::symlink(path, dir.join("component").join(name)).unwrap();
    }
    Some(TestRuntime { dir })
}

pub(super) fn moqui_test(runtime: &TestRuntime, args: &[&str]) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_moqui"))
        .arg("test")
        .args(args)
        .current_dir(repo())
        .env("MOQUI_RUNTIME", &runtime.dir)
        .env("MOQUI_FRAMEWORK", repo().join("runtime/base-component/framework"))
        .env("MOQUI_INSTALL", repo().join("runtime/install"))
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn the_fixture_suite_passes_in_suite_order_and_writes_junit() {
    let Some(runtime) = test_runtime("pass", &[("moqui-test-fixture", &fixture_component())]) else {
        return;
    };
    let junit = runtime.dir.join("junit");
    // Named, as the other components have specs of their own.
    let (out, err, ok) = moqui_test(&runtime, &["moqui-test-fixture", "--junit", junit.to_str().unwrap()]);
    assert!(ok, "stdout:\n{out}\nstderr:\n{err}");
    let lines: Vec<&str> = out.lines().filter(|line| line.contains(" > ")).collect();
    assert_eq!(lines.len(), 8, "{out}");
    assert!(
        lines[0].starts_with("Basics > a component service with a pinned id and clock  PASSED"),
        "{out}"
    );
    assert!(
        lines[2].starts_with("Basics > enum \"55500\" has description \"First\"  PASSED"),
        "{out}"
    );
    assert!(
        lines[6].starts_with("Dependent > data from the spec it depends on is there  PASSED"),
        "{out}"
    );
    assert!(
        lines[7].starts_with("Dependent > the component's test/MoquiConf.xml merges in a test run  PASSED"),
        "{out}"
    );
    assert!(
        out.contains("8 features: 8 passed, 0 failed, 0 ignored, 0 skipped; 2 specs in "),
        "{out}"
    );
    let xml = std::fs::read_to_string(junit.join("TEST-moqui-test-fixture.Basics.xml")).unwrap();
    assert!(
        xml.contains("tests=\"6\" failures=\"0\" errors=\"0\" skipped=\"0\""),
        "{xml}"
    );
    // The scratch runtime of a fresh run is removed.
    assert!(!err.contains("could not remove the scratch runtime"), "{err}");
}

#[test]
fn spec_selection_runs_what_it_depends_on_and_a_failure_exits_one() {
    let failing = PathBuf::from("/tmp").join(format!("mct-failing-component-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&failing);
    std::fs::create_dir_all(failing.join("test")).unwrap();
    std::fs::write(
        failing.join("component.xml"),
        "<component name=\"moqui-test-failing\" version=\"1.0.0\"/>\n",
    )
    .unwrap();
    std::fs::copy(
        fixture_component().join("failing-test/Failing.spec.js"),
        failing.join("test/Failing.spec.js"),
    )
    .unwrap();
    let Some(runtime) = test_runtime(
        "fail",
        &[
            ("moqui-test-fixture", &fixture_component()),
            ("moqui-test-failing", &failing),
        ],
    ) else {
        let _ = std::fs::remove_dir_all(&failing);
        return;
    };
    let (out, err, ok) = moqui_test(
        &runtime,
        &["moqui-test-fixture", "--spec", "Dependent", "--db", "snapshot"],
    );
    assert!(ok, "stdout:\n{out}\nstderr:\n{err}");
    assert!(out.contains("8 features: 8 passed"), "{out}");
    let (out, _err, ok) = moqui_test(&runtime, &["moqui-test-failing", "--db", "snapshot"]);
    let _ = std::fs::remove_dir_all(&failing);
    assert!(!ok, "{out}");
    assert!(
        out.contains("Failing > a false condition  FAILED (")
            && out.contains("moqui-test-failing/test/Failing.spec.js:7"),
        "{out}"
    );
    assert!(out.contains("Failing > skipped after the failure  SKIPPED"), "{out}");
    assert!(
        out.contains("    Condition not satisfied:\n    \n    priceMap.price == 16.99\n    |        |     |\n    |        16.98 false\n"),
        "{out}"
    );
    assert!(
        out.contains("2 features: 0 passed, 1 failed, 0 ignored, 1 skipped; 1 specs in "),
        "{out}"
    );
}
