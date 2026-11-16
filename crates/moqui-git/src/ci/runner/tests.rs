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

//! The runner's own pieces, with no server: cache keys, path patterns and archives. A whole job is run against the
//! runner REST in `server/ci_tests.rs`.

use std::path::PathBuf;

use super::{archive, expand_key, matches};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-ci-runner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &std::path::Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn a_cache_key_takes_the_hash_of_a_file_of_the_checkout() {
    let dir = scratch("key");
    write(&dir, "Cargo.lock", "one");
    let first = expand_key("cargo-${hash:Cargo.lock}-x", &dir);
    assert!(
        first.starts_with("cargo-") && first.ends_with("-x") && first.len() == "cargo--x".len() + 16,
        "{first}"
    );
    assert_eq!(first, expand_key("cargo-${hash:Cargo.lock}-x", &dir));
    write(&dir, "Cargo.lock", "two");
    assert_ne!(first, expand_key("cargo-${hash:Cargo.lock}-x", &dir));
    // a file that is not there, or outside the checkout, hashes to nothing
    assert_eq!(expand_key("k-${hash:missing}", &dir), "k-");
    assert_eq!(expand_key("k-${hash:../../etc/passwd}", &dir), "k-");
    assert_eq!(expand_key("plain", &dir), "plain");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn patterns_name_files_inside_the_workspace_only() {
    let dir = scratch("glob");
    write(&dir, "test-results/20260101/TEST-a.xml", "a");
    write(&dir, "test-results/20260101/TEST-b.xml", "b");
    write(&dir, "test-results/20260101/notes.txt", "n");
    write(&dir, "dist/app.zip", "z");
    let names = |pattern: &str| -> Vec<String> {
        matches(&dir, pattern)
            .iter()
            .map(|path| path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
            .collect()
    };
    assert_eq!(
        names("test-results/*/*.xml"),
        ["test-results/20260101/TEST-a.xml", "test-results/20260101/TEST-b.xml"]
    );
    assert_eq!(names("dist/*.zip"), ["dist/app.zip"]);
    assert_eq!(names("dist"), ["dist"]);
    assert_eq!(names("dist/app.zip"), ["dist/app.zip"]);
    assert!(names("missing/*.xml").is_empty());
    assert!(names("../*").is_empty());
    assert!(names("/etc/*").is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_archive_round_trips_by_workspace_paths() {
    let from = scratch("pack-from");
    let to = scratch("pack-to");
    write(&from, "target/debug/app", "binary");
    write(&from, "target/cache/x.bin", "x");
    write(&from, "notes.txt", "notes");
    let archive_file = from
        .parent()
        .unwrap()
        .join(format!("moqui-ci-runner-{}.tar.gz", std::process::id()));
    archive::pack(&from, &[from.join("target"), from.join("notes.txt")], &archive_file).unwrap();
    assert!(archive::unpack(&archive_file, &to).unwrap() >= 3);
    assert_eq!(std::fs::read_to_string(to.join("target/debug/app")).unwrap(), "binary");
    assert_eq!(std::fs::read_to_string(to.join("target/cache/x.bin")).unwrap(), "x");
    assert_eq!(std::fs::read_to_string(to.join("notes.txt")).unwrap(), "notes");
    // a file outside the workspace is not packed
    assert!(archive::pack(&from, &[to.join("notes.txt")], &archive_file).is_err());
    for path in [&from, &to] {
        std::fs::remove_dir_all(path).unwrap();
    }
    std::fs::remove_file(&archive_file).unwrap();
}

/// Between a registered runner's jobs only what started since the job began is killed: never the runner, what it runs
/// under, or a process started before the job (the `tee` its output goes through). `ps`'s elapsed time reads in its
/// forms (`mm:ss`, `hh:mm:ss`, `dd-hh:mm:ss`).
#[test]
fn only_processes_started_during_the_job_are_leftovers() {
    assert_eq!(super::etime_seconds("00:07"), Some(7));
    assert_eq!(super::etime_seconds("1:02:03"), Some(3723));
    assert_eq!(super::etime_seconds("2-00:00:01"), Some(172_801));
    assert_eq!(super::etime_seconds("x"), None);
    let listing = "  100 3-01:00:00\n  200 10:00\n  300 00:05\n  301 00:40\n  400 00:02\n";
    // the job ran 45 seconds; 400 is the runner itself
    assert_eq!(super::started_within(listing, 45, &[400]), [300, 301]);
}

/// SEC-GIT-02: a registered runner's step gets the base variables, the `LC_` ones and `--pass-env` names, never the
/// machine's others nor a runner token; a job's container passes its image's environment on, still without a token.
#[test]
fn a_step_gets_no_runner_token_and_on_a_registered_runner_only_the_base_environment() {
    use std::ffi::OsString;

    use super::{step_base_env, Options};

    let vars = || {
        [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/runner"),
            ("LC_ALL", "C.UTF-8"),
            ("CARGO_HOME", "/opt/cargo"),
            ("AWS_SECRET_ACCESS_KEY", "machine-secret"),
            ("MOQUI_CI_RUNNER_TOKEN", "mqcr_x"),
            ("MOQUI_CI_RUNNER_TOKEN_FILE", "/home/runner/.token"),
            ("MOQUI_CI_POOL_TOKEN", "mqcp_x"),
            ("MOQUI_CI_TOKEN", "job"),
            ("MOQUI_CI_SERVER_URL", "https://moqui.example.com"),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    };
    let names = |options: &Options| -> Vec<String> {
        step_base_env(vars(), options)
            .into_iter()
            .map(|(name, _)| name.into_string().unwrap())
            .collect()
    };
    let registered = Options {
        inherit_env: false,
        pass_env: vec!["CARGO_HOME".to_string(), "MOQUI_CI_SERVER_URL".to_string()],
        ..Options::default()
    };
    assert_eq!(names(&registered), ["PATH", "HOME", "LC_ALL", "CARGO_HOME"]);
    let container = Options::default();
    assert_eq!(
        names(&container),
        ["PATH", "HOME", "LC_ALL", "CARGO_HOME", "AWS_SECRET_ACCESS_KEY", "MOQUI_CI_SERVER_URL"]
    );
}

/// SEC-GIT-02: `--step-user` runs the steps as another user, out of reach of the runner's token file: only a root runner
/// can, and never as root.
#[test]
fn a_step_user_needs_a_root_runner_and_is_not_root() {
    use super::{step_user_of, StepUser};

    assert_eq!(
        step_user_of("ci", "0\n", "1001\n", "1002\n"),
        Ok(StepUser {
            name: "ci".into(),
            uid: 1001,
            gid: 1002
        })
    );
    assert!(step_user_of("ci", "1000\n", "1001\n", "1002\n").unwrap_err().contains("root"));
    assert!(step_user_of("nobody-here", "0\n", "", "").unwrap_err().contains("no such user"));
    assert!(step_user_of("root", "0\n", "0\n", "0\n").unwrap_err().contains("is root"));
}
