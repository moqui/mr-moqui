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

use std::path::{Path, PathBuf};

use super::{archive, expand_key, matches};

/// A directory of the test's own under the repository's `target/`.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-scratch")
        .join(format!("moqui-ci-runner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
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

/// A workspace (`<root>/src`) whose files a step made links in, beside a directory outside it (`<root>/outside`) that
/// holds what the links point at (SEC-GIT-81): a report that is a link out, a report that is a link to a file in the
/// workspace, a directory that is a link out (`d`), an artifact directory holding a link to a file and to a directory
/// out, a cache key's file that is a link out; and an ordinary report and artifact.
#[cfg(unix)]
fn linked_workspace(workspace: &Path) {
    use std::os::unix::fs::symlink;

    let outside = workspace.parent().unwrap().join("outside");
    write(&outside, "secret.txt", "SECRET-OUTSIDE");
    write(&outside, "x.xml", "SECRET-X");
    write(workspace, "reports/ok.xml", "<ok/>");
    symlink(outside.join("secret.txt"), workspace.join("reports/out.xml")).unwrap();
    symlink("ok.xml", workspace.join("reports/inner.xml")).unwrap();
    symlink(&outside, workspace.join("d")).unwrap();
    write(workspace, "real/x.xml", "<real/>");
    write(workspace, "dist/app.bin", "binary");
    symlink(outside.join("secret.txt"), workspace.join("dist/leak")).unwrap();
    symlink(&outside, workspace.join("dist/sub")).unwrap();
    symlink(outside.join("secret.txt"), workspace.join("Cargo.lock")).unwrap();
}

/// The entries of a tar.gz: each path, whether it is a link, and the bytes of the files.
#[cfg(unix)]
fn entries_of(archive_file: &Path) -> Vec<(String, bool, Vec<u8>)> {
    use std::io::Read as _;

    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(archive_file).unwrap()));
    tar.entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let path = entry
                .path()
                .unwrap()
                .to_string_lossy()
                .trim_end_matches('/')
                .to_string();
            let link = entry.header().entry_type() == tar::EntryType::Symlink;
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (path, link, bytes)
        })
        .collect()
}

/// SEC-GIT-81: a pattern keeps only what resolves inside the workspace: a report linked out, a directory linked out on
/// the way (`d -> outside` with `d/x.xml`), and a `*` that meets a link out are left out (and said as refused); a link
/// to a file in the workspace is kept for the caller to judge.
#[cfg(unix)]
#[test]
fn patterns_never_resolve_out_of_the_workspace() {
    let root = scratch("links-glob");
    let workspace = root.join("src");
    linked_workspace(&workspace);
    let names = |pattern: &str| -> (Vec<String>, Vec<String>) {
        let shown = |paths: Vec<PathBuf>| -> Vec<String> {
            paths
                .iter()
                .map(|path| path.strip_prefix(&workspace).unwrap().to_string_lossy().into_owned())
                .collect()
        };
        let (found, refused) = super::matches_and_refused(&workspace, pattern);
        (shown(found), shown(refused))
    };
    let strings = |names: &[&str]| -> Vec<String> { names.iter().map(|name| (*name).to_string()).collect() };
    assert_eq!(
        names("reports/*.xml"),
        (
            strings(&["reports/inner.xml", "reports/ok.xml"]),
            strings(&["reports/out.xml"])
        )
    );
    assert_eq!(names("d/x.xml"), (strings(&[]), strings(&["d"])));
    assert_eq!(names("d/*"), (strings(&[]), strings(&["d"])));
    assert_eq!(
        names("*/x.xml"),
        (strings(&["real/x.xml"]), strings(&["Cargo.lock", "d"]))
    );
    assert_eq!(names("dist").0, ["dist"]);
    assert!(matches(&workspace, "d").is_empty());
    std::fs::remove_dir_all(&root).unwrap();
}

/// SEC-GIT-81: a report is read only when it is a regular file of the workspace, never through a link of its own name
/// (`O_NOFOLLOW`), even one to a file of the workspace, nor through a directory linked out; a cache key's
/// `${hash:FILE}` of a link out hashes to nothing.
#[cfg(unix)]
#[test]
fn a_report_is_read_only_as_a_regular_file_in_the_workspace() {
    let root = scratch("links-report");
    let workspace = root.join("src");
    linked_workspace(&workspace);
    let read = |name: &str| -> Result<String, String> {
        let mut out = Vec::new();
        super::copy_regular(&workspace, &workspace.join(name), &mut out).map(|()| String::from_utf8(out).unwrap())
    };
    assert_eq!(read("reports/ok.xml").unwrap(), "<ok/>");
    assert!(read("reports/out.xml").unwrap_err().contains("outside the workspace"));
    assert!(read("reports/inner.xml").is_err());
    assert!(super::open_regular(&workspace.join("reports/inner.xml")).is_err());
    assert!(read("d/x.xml").unwrap_err().contains("outside the workspace"));
    assert!(read("real").unwrap_err().contains("not a regular file"));
    assert_eq!(expand_key("k-${hash:Cargo.lock}", &workspace), "k-");
    std::fs::remove_dir_all(&root).unwrap();
}

/// SEC-GIT-81: an artifact directory's links are stored as links, never followed, so nothing outside the workspace is
/// read; a named path that is, or goes through, a link out is refused; the ordinary files are packed.
#[cfg(unix)]
#[test]
fn an_artifact_reads_nothing_outside_the_workspace() {
    let root = scratch("links-pack");
    let workspace = root.join("src");
    linked_workspace(&workspace);
    let archive_file = root.join("artifact.tar.gz");
    archive::pack(&workspace, &[workspace.join("dist")], &archive_file).unwrap();
    let mut entries = entries_of(&archive_file);
    entries.sort();
    assert_eq!(
        entries,
        [
            ("dist".to_string(), false, Vec::new()),
            ("dist/app.bin".to_string(), false, b"binary".to_vec()),
            ("dist/leak".to_string(), true, Vec::new()),
            ("dist/sub".to_string(), true, Vec::new()),
        ]
    );
    for named in ["d", "d/x.xml", "dist/leak", "dist/sub", "reports/out.xml"] {
        let refused = archive::pack(&workspace, &[workspace.join(named)], &archive_file).unwrap_err();
        assert!(refused.contains("outside the workspace"), "{named}: {refused}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// A transport with no server: the job it was made with, a checkout that makes the linked files, and what was
/// uploaded and logged.
#[cfg(unix)]
struct Recorder {
    job: serde_json::Value,
    uploads: std::cell::RefCell<Vec<(String, Vec<u8>)>>,
    log: std::cell::RefCell<String>,
}

#[cfg(unix)]
impl super::Transport for Recorder {
    fn json(
        &self,
        _method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), String> {
        if path == "job" {
            return Ok((200, self.job.clone()));
        }
        if path == "log" {
            let text = body.and_then(|body| body["text"].as_str()).unwrap_or_default();
            self.log.borrow_mut().push_str(text);
        }
        Ok((200, serde_json::json!({})))
    }

    fn upload(&self, path_and_query: &str, file: &Path) -> Result<(u16, serde_json::Value), String> {
        let bytes = std::fs::read(file).map_err(|err| err.to_string())?;
        self.uploads.borrow_mut().push((path_and_query.to_string(), bytes));
        Ok((200, serde_json::json!({})))
    }

    fn download(&self, _path_and_query: &str, _to: &Path) -> Result<u16, String> {
        Ok(404)
    }

    fn checkout(&self, _job: &serde_json::Value, workspace: &Path) -> Result<String, String> {
        linked_workspace(workspace);
        Ok(String::new())
    }
}

/// SEC-GIT-81, a whole job: the reports and artifacts a step linked out of the workspace are not uploaded, each named
/// in the job's log, and the ordinary report and artifact are.
#[cfg(unix)]
#[test]
fn a_job_uploads_no_report_or_artifact_through_a_link() {
    let root = scratch("links-job");
    let transport = Recorder {
        job: serde_json::json!({
            "commitId": "abc123",
            "timeoutMinutes": 1,
            "spec": {
                "job": "build",
                "pipeline": "ci",
                "steps": [],
                "reports": ["reports/*.xml", "d/x.xml"],
                "artifacts": [{ "name": "dist", "path": "dist" }, { "name": "leak", "path": "d/*" }],
            },
        }),
        uploads: std::cell::RefCell::default(),
        log: std::cell::RefCell::default(),
    };
    let options = super::Options {
        workspace: root.join("src"),
        ..super::Options::default()
    };
    assert_eq!(super::run(&transport, &options), super::EXIT_SUCCESS);
    let log = transport.log.borrow().clone();
    for said in [
        "Report reports/out.xml was not uploaded: it does not resolve inside the workspace",
        "Report reports/inner.xml was not uploaded: it is a symbolic link",
        "Report d was not uploaded: it does not resolve inside the workspace",
        "Artifact leak: d was left out, as it does not resolve inside the workspace",
        "Artifact leak: nothing at d/*",
        "Artifact dist uploaded",
    ] {
        assert!(log.contains(said), "{said} not in the log:\n{log}");
    }
    let uploads = transport.uploads.borrow();
    let kinds: Vec<&str> = uploads
        .iter()
        .map(|(query, _)| query.split_once("kind=").unwrap().1.split('&').next().unwrap())
        .collect();
    assert_eq!(kinds, ["report", "artifact"]);
    assert!(uploads[0].0.contains("name=ok.xml"));
    assert_eq!(uploads[0].1, b"<ok/>");
    let archive_file = root.join("uploaded.tar.gz");
    std::fs::write(&archive_file, &uploads[1].1).unwrap();
    let packed = entries_of(&archive_file);
    assert!(packed
        .iter()
        .any(|(path, _, bytes)| path == "dist/app.bin" && bytes == b"binary"));
    assert!(packed
        .iter()
        .all(|(_, _, bytes)| !String::from_utf8_lossy(bytes).contains("SECRET")));
    drop(uploads);
    std::fs::remove_dir_all(&root).unwrap();
}
