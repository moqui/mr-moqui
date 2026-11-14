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

use super::{jobs_of, pick, quoted, script};

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
