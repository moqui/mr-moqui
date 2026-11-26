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

use super::{
    digest, digest_refusal, engine_sockets, image_run, jobs_of, pick, quoted, script, script_in, valid_run_name, Place,
    LABEL_ROOT, LABEL_USER,
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
    );
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

/// A job run in its image on a real engine (opt-in, as moqui-git's `ci_docker`): its steps run from the mounted working
/// tree, write there as the tree's owner, and its log says what ran.
/// `MOQUI_CI_DOCKER_SOCKET=/var/run/docker.sock cargo test -p dev-aide --lib in_a_real -- --ignored --nocapture` (an
/// image `busybox` on the engine; `MOQUI_CI_IMAGE` names another).
#[test]
#[ignore = "opt-in: a real Docker or Podman socket"]
fn in_a_real_engine_a_job_runs_in_its_image_on_the_mounted_tree() {
    let Some(socket) = std::env::var("MOQUI_CI_DOCKER_SOCKET")
        .ok()
        .filter(|socket| !socket.is_empty())
    else {
        eprintln!("MOQUI_CI_DOCKER_SOCKET is not set: nothing run");
        return;
    };
    let image = std::env::var("MOQUI_CI_IMAGE").unwrap_or_else(|_| "busybox".into());
    let ci = format!(
        r#"<ci><pipeline name="check" on="push"><job name="unit" image="{image}">
            <env name="MODE" value="local"/><secret name="DEPLOY_KEY"/>
            <step name="Write" run="echo made-in-$MODE > made.txt"/>
            <step name="Show" run="cat made.txt; echo key=${{DEPLOY_KEY:-unset}}; id -u"/>
        </job></pipeline></ci>"#
    );
    let jobs = jobs_of(&ci).unwrap();
    let job = pick(&jobs, "check", "unit", "").unwrap();
    let tree = std::env::temp_dir().join(format!("dev-aide-image-run-{}", std::process::id()));
    std::fs::create_dir_all(&tree).unwrap();
    let host = moqui_git::ci::driver::docker::DockerHost::unix(&socket);
    let hardening = moqui_git::ci::driver::docker::Hardening::default();
    let name = format!("moqui-devaide-test-{}", std::process::id());
    let run = image_run(job, &tree, "EX_TEST", "test", &name, Path::new(&hardening.workdir));
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
    let made = std::fs::read_to_string(tree.join("made.txt")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&tree);
    eprintln!("{log}");
    assert_eq!(info.exit_code, Some(0), "{log}");
    assert_eq!(made.trim(), "made-in-local", "{log}");
    assert!(
        log.contains("key=unset") && log.contains("== The job succeeded"),
        "{log}"
    );
    assert!(!log.lines().any(|line| line.trim() == "0"), "not root: {log}");
}
