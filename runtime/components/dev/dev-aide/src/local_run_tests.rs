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

use std::path::Path;

#[cfg(unix)]
use super::{copy_tree, TreeCopy};
use super::{
    digest, digest_refusal, engine_sockets, image_refusal, image_run, jobs_of, pick, quoted, script, script_in,
    valid_run_name, with_time_limit, ImageLimits, Place, LABEL_ROOT, LABEL_USER,
};

const LIMITS: ImageLimits = ImageLimits {
    memory_mb: 2048,
    pids: 512,
    timeout: std::time::Duration::from_secs(600),
    per_user: 2,
    copy_bytes: 64 * 1024 * 1024,
};

const CI: &str = r#"<ci>
    <pipeline name="check" on="push merge-request">
        <env name="CARGO_BUILD_JOBS" value="4"/>
        <job name="unit" image="moqui-build">
            <env name="MODE" value="fast"/>
            <secret name="DEPLOY_KEY"/>
            <step name="Lint" run="cargo lint"/>
            <step run="echo 'quoted' &amp;&amp; ./moq test" continue-on-error="true"/>
            <step name="Last" run="echo done"/>
        </job>
        <job name="db" image="moqui-${matrix.db}">
            <matrix><axis name="db" values="postgres mysql"/></matrix>
            <step run="./moq test --db ${matrix.db}"/>
        </job>
    </pipeline>
</ci>"#;

/// A job's steps become the session's commands in order, with its variables and CI's own; a step that fails ends the
/// job unless it continues on error.
#[test]
fn a_jobs_steps_become_the_commands_of_its_script() {
    let jobs = jobs_of(CI).unwrap();
    let unit = pick(&jobs, "check", "unit", "").unwrap();
    assert_eq!(unit.steps.len(), 3);
    let text = script(unit, Path::new("/work/my-site"));
    assert!(text.contains("cd '/work/my-site' || exit 1"), "{text}");
    assert!(text.contains("export CARGO_BUILD_JOBS='4'"), "{text}");
    assert!(text.contains("export MODE='fast'"), "{text}");
    assert!(text.contains("export MOQUI_CI_JOB='unit'"), "{text}");
    let lint = text.find("sh -c 'cargo lint'").unwrap();
    let test = text
        .find(&format!("sh -c {}", quoted("echo 'quoted' && ./moq test")))
        .unwrap();
    let last = text.find("sh -c 'echo done'").unwrap();
    assert!(lint < test && test < last, "{text}");
    assert!(text.contains("continues on error"), "{text}");
    assert!(text.contains("not the job's image (moqui-build)"), "{text}");
}

/// A secret the job asks for is never set: its variable is unset, as for a run that is not given it.
#[test]
fn a_local_run_has_none_of_the_jobs_secrets() {
    let jobs = jobs_of(CI).unwrap();
    let unit = pick(&jobs, "check", "unit", "").unwrap();
    assert_eq!(unit.secrets, vec!["DEPLOY_KEY".to_string()]);
    let text = script(unit, Path::new("/w"));
    assert!(text.contains("unset DEPLOY_KEY"), "{text}");
    assert!(!text.contains("DEPLOY_KEY="), "{text}");
}

/// A matrix job runs one combination: the one asked for, or the first.
#[test]
fn a_matrix_job_picks_one_combination() {
    let jobs = jobs_of(CI).unwrap();
    let keys: Vec<&str> = jobs
        .iter()
        .filter(|job| job.job == "db")
        .map(|job| job.matrix_key.as_str())
        .collect();
    assert_eq!(keys, ["postgres", "mysql"]);
    let mysql = pick(&jobs, "check", "db", "mysql").unwrap();
    let text = script(mysql, Path::new("/w"));
    assert!(text.contains("sh -c './moq test --db mysql'"), "{text}");
    assert!(text.contains("export MATRIX_DB='mysql'"), "{text}");
    assert!(!text.contains("postgres"), "{text}");
    assert_eq!(pick(&jobs, "check", "db", "").unwrap().matrix_key, "postgres");
    assert!(pick(&jobs, "check", "db", "oracle").is_none());
}

/// A file the engine refuses is refused here too, with its line.
#[test]
fn a_bad_ci_file_says_its_line() {
    let error =
        jobs_of("<ci><pipeline name=\"p\" on=\"push\"><job name=\"j\" image=\"i\"/></pipeline></ci>").unwrap_err();
    assert!(error.starts_with("ci.xml line "), "{error}");
}

/// A run is held to the script the page showed: one rebuilt to something else is refused; the script removes itself.
#[test]
fn a_local_run_runs_only_the_script_that_was_shown() {
    let jobs = jobs_of(CI).unwrap();
    let unit = pick(&jobs, "check", "unit", "").unwrap();
    let shown = script(unit, Path::new("/w"));
    assert!(shown.contains("trap 'rm -f -- \"$0\"' EXIT"), "{shown}");
    assert!(digest_refusal(&shown, &digest(&shown)).is_none());
    let changed = script(pick(&jobs, "check", "db", "").unwrap(), Path::new("/w"));
    assert!(digest_refusal(&changed, &digest(&shown))
        .unwrap()
        .contains("changed since"));
    assert!(digest_refusal(&shown, "").is_some());
}

/// In the job's image the same steps run with the same variables and no secret, from the working directory the tree is
/// mounted at, as the container's command: no script file to remove, and it says it is the image.
#[test]
fn in_the_image_the_same_steps_run_from_the_mounted_tree() {
    let jobs = jobs_of(CI).unwrap();
    let unit = pick(&jobs, "check", "unit", "").unwrap();
    let text = script_in(unit, Path::new("/workspace"), Place::Image);
    assert!(text.contains("cd '/workspace' || exit 1"), "{text}");
    assert!(text.contains("export MODE='fast'"), "{text}");
    assert!(text.contains("unset DEPLOY_KEY"), "{text}");
    assert!(text.contains("sh -c 'cargo lint'"), "{text}");
    assert!(text.contains("# In the job's image (moqui-build)"), "{text}");
    assert!(
        !text.contains("trap") && !text.contains("not the job's image"),
        "{text}"
    );
    assert_ne!(digest(&text), digest(&script(unit, Path::new("/workspace"))));

    let tree = std::env::temp_dir();
    let run = image_run(
        unit,
        &tree,
        "EX_ANN",
        "my-site",
        "moqui-devaide-dev-abc123",
        Path::new("/workspace"),
        &LIMITS,
        false,
    );
    assert_eq!((run.memory_mb, run.pids_limit), (2048, 512));
    assert_eq!(run.network_mode, "bridge");
    assert_eq!(run.image, "moqui-build");
    assert_eq!(run.command[..2], ["sh".to_string(), "-c".to_string()]);
    assert_eq!(run.command[2], text);
    assert_eq!(run.tree, tree);
    let label = |name: &str| {
        run.labels
            .iter()
            .find(|(have, _)| have == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(label(LABEL_USER), Some("EX_ANN"));
    assert_eq!(label(LABEL_ROOT), Some("my-site"));
    assert!(label(moqui_git::ci::driver::LABEL_SERVER).is_some());
    // the tree's owner writes it, never root
    assert!(
        run.user.as_deref().is_none_or(|user| !user.starts_with("0:")),
        "{:?}",
        run.user
    );
}

/// The engine is looked for where Docker and Podman put their sockets, DOCKER_HOST's first; a run is named only as a
/// local run's container is.
#[test]
fn the_engine_is_found_by_its_socket_and_a_run_by_its_name() {
    let sockets = engine_sockets(
        Some("unix:///home/ann/.docker/run/docker.sock"),
        Some("/run/user/1000"),
        Some(1000),
    );
    let sockets: Vec<String> = sockets.iter().map(|path| path.display().to_string()).collect();
    assert_eq!(
        sockets,
        [
            "/home/ann/.docker/run/docker.sock",
            "/var/run/docker.sock",
            "/run/user/1000/podman/podman.sock",
            "/run/podman/podman.sock"
        ]
    );
    let plain = engine_sockets(Some("tcp://10.0.0.2:2375"), None, None);
    assert_eq!(
        plain[0],
        Path::new("/var/run/docker.sock"),
        "only a unix socket of DOCKER_HOST"
    );
    assert!(valid_run_name("moqui-devaide-team.example.com-a1b2c3"));
    for bad in [
        "moqui-devaide-",
        "moqui-ci-x-1",
        "moqui-devaide-a/../b",
        "moqui-devaide-a?b",
        "other",
    ] {
        assert!(!valid_run_name(bad), "{bad}");
    }
}

/// git in `dir`, isolated from the desktop's config and credential helpers.
#[cfg(unix)]
fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS", "false")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Every path under `dir` (no link followed) with a file's text: what a run must leave as it was.
#[cfg(unix)]
fn snapshot(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap().flatten() {
            let path = entry.path();
            let kind = entry.file_type().unwrap();
            let relative = path.strip_prefix(dir).unwrap().display().to_string();
            if kind.is_symlink() {
                out.push((relative, format!("-> {}", std::fs::read_link(&path).unwrap().display())));
            } else if kind.is_dir() {
                out.push((relative, String::new()));
                pending.push(path);
            } else {
                out.push((relative, std::fs::read_to_string(&path).unwrap_or_default()));
            }
        }
    }
    out.sort();
    out
}

/// A job run in its image on a real engine (opt-in, as moqui-git's `ci_docker`) works on a copy of the working tree's
/// files: no `.git` comes with it, a link is a link, an ignored file stays behind, and what the steps do (a hook
/// written, a write through a link to `/`, a nested repository renamed and another planted) leaves the working tree as
/// it was; the copy is deleted after.
/// `MOQUI_CI_DOCKER_SOCKET=/var/run/docker.sock cargo test -p dev-aide --lib in_a_real -- --ignored --nocapture` (an
/// image `busybox` on the engine; `MOQUI_CI_IMAGE` names another).
#[cfg(unix)]
#[test]
#[ignore = "opt-in: a real Docker or Podman socket"]
fn in_a_real_engine_a_job_runs_on_a_copy_of_the_tree() {
    let Some(socket) = std::env::var("MOQUI_CI_DOCKER_SOCKET")
        .ok()
        .filter(|socket| !socket.is_empty())
    else {
        eprintln!("MOQUI_CI_DOCKER_SOCKET is not set: nothing run");
        return;
    };
    let image = std::env::var("MOQUI_CI_IMAGE").unwrap_or_else(|_| "busybox".into());
    let escape = format!("/tmp/dev-aide-escape-{}", std::process::id());
    let ci = format!(
        r#"<ci><pipeline name="check" on="push"><job name="unit" image="{image}">
            <env name="MODE" value="local"/><secret name="DEPLOY_KEY"/>
            <step name="Write" run="echo made-in-$MODE > made.txt; cat made.txt"/>
            <step name="Show" run="echo key=${{DEPLOY_KEY:-unset}}; id -u; cat README.md"/>
            <step name="Git" run="test -e .git &amp;&amp; echo git-came || echo git-absent"/>
            <step name="Ignored" run="test -e secret.env &amp;&amp; echo secret-came || echo secret-absent"/>
            <step name="Hook" run="mkdir -p .git/hooks &amp;&amp; echo x > .git/hooks/post-checkout &amp;&amp; echo hook-in-copy"/>
            <step name="Link" run="echo escape > rootlink{escape} &amp;&amp; echo link-in-container"/>
            <step name="Rename" run="mv component component_old; mkdir -p component/.git &amp;&amp; echo fsmonitor > component/.git/config &amp;&amp; echo planted"/>
        </job></pipeline></ci>"#
    );
    let jobs = jobs_of(&ci).unwrap();
    let job = pick(&jobs, "check", "unit", "").unwrap();
    let base = std::env::temp_dir().join(format!("dev-aide-image-run-{}", std::process::id()));
    let tree = base.join("site");
    std::fs::create_dir_all(tree.join("component")).unwrap();
    git_in(&tree, &["init", "-q", "-b", "main"]);
    std::fs::write(tree.join("README.md"), "the site\n").unwrap();
    std::fs::write(tree.join(".gitignore"), "secret.env\n").unwrap();
    std::fs::write(tree.join("secret.env"), "TOKEN=x\n").unwrap();
    git_in(&tree, &["add", "README.md", ".gitignore"]);
    git_in(&tree, &["commit", "-q", "-m", "Start"]);
    // a nested repository, and a link to the host's root, untracked
    git_in(&tree.join("component"), &["init", "-q", "-b", "main"]);
    std::fs::write(tree.join("component/a.txt"), "a\n").unwrap();
    std::os::unix::fs::symlink("/", tree.join("rootlink")).unwrap();
    let before = snapshot(&tree);
    let listed: Vec<String> = git_in(&tree, &["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    let copy = TreeCopy(base.join("copy"));
    copy_tree(&tree, &listed, &copy.0, LIMITS.copy_bytes).unwrap();
    assert!(std::fs::symlink_metadata(copy.0.join("rootlink"))
        .unwrap()
        .file_type()
        .is_symlink());
    let host = moqui_git::ci::driver::docker::DockerHost::unix(&socket);
    let hardening = moqui_git::ci::driver::docker::Hardening::default();
    let name = format!("moqui-devaide-test-{}", std::process::id());
    let run = image_run(
        job,
        &copy.0,
        "EX_TEST",
        "test",
        &name,
        Path::new(&hardening.workdir),
        &LIMITS,
        false,
    );
    let id = moqui_git::ci::driver::docker::start_local(&host, &run, &hardening).unwrap();
    let started = std::time::Instant::now();
    let info = loop {
        let info = host.inspect(&id).unwrap().unwrap();
        if !info.running && info.status != "created" {
            break info;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the run did not end"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let log = host.logs(&id, 200).unwrap();
    host.remove(&id).unwrap();
    let copy_path = copy.0.clone();
    drop(copy);
    let after = snapshot(&tree);
    let escaped = Path::new(&escape).exists();
    let _ = std::fs::remove_file(&escape);
    let _ = std::fs::remove_dir_all(&base);
    eprintln!("{log}");
    assert_eq!(info.exit_code, Some(0), "{log}");
    for said in [
        "made-in-local",
        "key=unset",
        "the site",
        "git-absent",
        "secret-absent",
        "hook-in-copy",
        "link-in-container",
        "planted",
        "== The job succeeded",
    ] {
        assert!(log.contains(said), "{said}: {log}");
    }
    assert!(!log.lines().any(|line| line.trim() == "0"), "not root: {log}");
    assert_eq!(after, before, "the working tree is as it was");
    assert!(!escaped, "a write through a link to / stays in the container");
    assert!(!copy_path.exists(), "the copy is deleted");
}

/// A working tree's copy has the files git lists and nothing else: no `.git` part, no path through a link out of the
/// tree, a link as a link, no directory; one over its size is refused, and the copy goes when it is dropped.
#[cfg(unix)]
#[test]
fn a_runs_copy_has_the_files_git_sees_and_no_git() {
    let base = std::env::temp_dir().join(format!("dev-aide-copy-{}", std::process::id()));
    let tree = base.join("site");
    let outside = base.join("outside");
    for dir in [
        tree.join(".git"),
        tree.join("sub"),
        tree.join("module"),
        outside.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(tree.join("a.txt"), "a\n").unwrap();
    std::fs::write(tree.join(".git/config"), "[core]\n").unwrap();
    std::fs::write(tree.join("sub/.git"), "gitdir: x\n").unwrap();
    std::fs::write(tree.join("sub/b.txt"), "b\n").unwrap();
    std::fs::write(outside.join("x"), "outside\n").unwrap();
    std::os::unix::fs::symlink("/", tree.join("rootlink")).unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("linkdir")).unwrap();
    let listed: Vec<String> = [
        "a.txt",
        ".git/config",
        "sub/.git",
        "sub/b.txt",
        "rootlink",
        "linkdir/x",
        "module",
        "../a.txt",
    ]
    .iter()
    .map(|path| (*path).to_string())
    .collect();
    let copy = TreeCopy(base.join("copy"));
    let copied = copy_tree(&tree, &listed, &copy.0, 1024).unwrap();
    assert_eq!(copied, 4);
    let mut found = snapshot(&copy.0);
    found.sort();
    assert_eq!(
        found,
        [
            ("a.txt".to_string(), "a\n".to_string()),
            ("rootlink".to_string(), "-> /".to_string()),
            ("sub".to_string(), String::new()),
            ("sub/b.txt".to_string(), "b\n".to_string()),
        ]
    );
    let path = copy.0.clone();
    drop(copy);
    assert!(!path.exists(), "dropped, the copy is gone");
    let small = TreeCopy(base.join("small"));
    let refused = copy_tree(&tree, &listed, &small.0, 2).unwrap_err();
    assert!(refused.contains("local-run-copy-mb"), "{refused}");
    drop(small);
    let _ = std::fs::remove_dir_all(&base);
}

/// A run in the image needs TERMINAL_HOST, a root the run may write, and never copies a tree that holds the runtime; a
/// shared server's run has no network.
#[test]
fn a_run_in_the_image_is_refused_a_tree_it_may_not_write() {
    let base = std::env::temp_dir().join(format!("dev-aide-image-refusal-{}", std::process::id()));
    let runtime = base.join("runtime");
    let site = runtime.join("component-site");
    let elsewhere = base.join("my-site");
    for dir in [&site, &elsewhere.join(".git")] {
        std::fs::create_dir_all(dir).unwrap();
    }
    assert!(image_refusal("my-site", &elsewhere, false, &runtime, true).is_none());
    let no_permission = image_refusal("my-site", &elsewhere, false, &runtime, false).unwrap();
    assert!(no_permission.contains("TERMINAL_HOST"), "{no_permission}");
    assert!(image_refusal("my-site", &elsewhere, true, &runtime, true)
        .unwrap()
        .contains("read only"));
    // the checkout that holds the runtime, and the runtime itself; a tree inside the runtime holds none of it
    assert!(image_refusal("mr-moqui", &base, false, &runtime, true)
        .unwrap()
        .contains("runtime"));
    assert!(image_refusal("runtime", &runtime, false, &runtime, true).is_some());
    assert!(image_refusal("site", &site, false, &runtime, true).is_none());

    let jobs = jobs_of(CI).unwrap();
    let unit = pick(&jobs, "check", "unit", "").unwrap();
    let run = image_run(
        unit,
        &elsewhere,
        "EX_ANN",
        "my-site",
        "moqui-devaide-x-1",
        Path::new("/workspace"),
        &LIMITS,
        true,
    );
    assert_eq!(run.network_mode, "none", "a shared server's run reaches no network");
    let plain = image_run(
        unit,
        &site,
        "EX_ANN",
        "site",
        "moqui-devaide-x-2",
        Path::new("/workspace"),
        &LIMITS,
        false,
    );
    assert_eq!(plain.network_mode, "bridge");
    let _ = std::fs::remove_dir_all(&base);
}

/// A run in the image has a time limit of its own, so it stops whatever becomes of the server that started it.
#[test]
fn a_run_in_the_image_stops_itself_past_its_time() {
    let jobs = jobs_of(CI).unwrap();
    let unit = pick(&jobs, "check", "unit", "").unwrap();
    let text = with_time_limit(&script_in(unit, Path::new("/workspace"), Place::Image), 3600);
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("#!/bin/sh"));
    let watchdog = lines.next().unwrap();
    assert!(
        watchdog.starts_with("( sleep 3600;") && watchdog.ends_with("kill -TERM -1 ) &"),
        "{watchdog}"
    );
    assert!(text.ends_with("exit \"$moqui_status\"\n"), "{text}");
}
