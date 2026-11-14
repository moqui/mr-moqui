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

//! Planning: which pipelines an event starts, the jobs of a matrix, and what a job's runner is given.

use super::{applies, context, plan};
use crate::ci::builtin;
use crate::ci::file::{parse, Event, Limits};

const FILE: &str = r#"<ci>
    <pipeline name="check" on="push merge-request" branches="main release/*">
        <env name="A" value="pipeline"/><env name="B" value="pipeline"/>
        <job name="build" image="build:${matrix.rust}" pool="linux" timeout-minutes="30">
            <env name="B" value="job-${matrix.db}"/>
            <service name="db" image="${matrix.db}:latest"><env name="NAME" value="${matrix.db}"/></service>
            <matrix><axis name="rust" values="stable nightly"/><axis name="db" values="postgres mysql"/></matrix>
            <cache key="cargo-${matrix.rust}-${hash:Cargo.lock}" path="target"/>
            <step run="cargo test --features ${matrix.db}"/>
            <artifact name="logs-${matrix.rust}-${matrix.db}" path="log"/>
        </job>
        <job name="package" image="build" needs="build"><step name="Zip" run="zip"/></job>
    </pipeline>
    <pipeline name="tagged" on="tag" tags="v*"><job name="j" image="i"><step run="x"/></job></pipeline>
    <pipeline name="by-hand" on="manual schedule" cron="0 0 3 * * ?"><job name="j" image="i"><step run="x"/></job></pipeline>
</ci>"#;

#[test]
fn a_pipeline_applies_by_its_events_and_its_ref_patterns() {
    let file = parse(FILE, &Limits::default()).unwrap();
    let (check, tagged, by_hand) = (&file.pipelines[0], &file.pipelines[1], &file.pipelines[2]);
    assert!(applies(check, Event::Push, "refs/heads/main"));
    assert!(applies(check, Event::Push, "refs/heads/release/1.2"));
    assert!(!applies(check, Event::Push, "refs/heads/feature"));
    // a merge request is matched by its target branch
    assert!(applies(check, Event::MergeRequest, "refs/heads/main"));
    assert!(!applies(check, Event::MergeRequest, "refs/heads/feature"));
    // a push of a tag is not a push of a branch, and the other way
    assert!(!applies(check, Event::Push, "refs/tags/v1"));
    assert!(!applies(check, Event::Tag, "refs/tags/v1"));
    assert!(applies(tagged, Event::Tag, "refs/tags/v1.0"));
    assert!(!applies(tagged, Event::Tag, "refs/tags/nightly"));
    assert!(!applies(tagged, Event::Push, "refs/heads/v1"));
    // asked for by name: only the event decides
    assert!(applies(by_hand, Event::Manual, "refs/heads/anything"));
    assert!(applies(by_hand, Event::Schedule, "refs/heads/main"));
    assert!(!applies(by_hand, Event::Push, "refs/heads/main"));
}

#[test]
fn a_matrix_makes_a_job_for_each_combination_and_needs_stay_by_name() {
    let file = parse(FILE, &Limits::default()).unwrap();
    let jobs = plan(&file.pipelines[0]);
    let keys: Vec<(&str, &str)> = jobs
        .iter()
        .map(|job| (job.name.as_str(), job.matrix_key.as_str()))
        .collect();
    assert_eq!(
        keys,
        [
            ("build", "stable, postgres"),
            ("build", "stable, mysql"),
            ("build", "nightly, postgres"),
            ("build", "nightly, mysql"),
            ("package", ""),
        ]
    );
    let job = &jobs[3];
    assert_eq!(job.image, "build:nightly");
    assert_eq!(job.pool.as_deref(), Some("linux"));
    assert_eq!(job.timeout_minutes, 30);
    assert_eq!(job.steps, [(None, "cargo test --features mysql".to_string())]);
    let spec = &job.spec;
    assert_eq!(spec["pipeline"], "check");
    assert_eq!(spec["job"], "build");
    assert_eq!(spec["matrixKey"], "nightly, mysql");
    assert_eq!(spec["image"], "build:nightly");
    // the pipeline's variables, the job's over them, and the matrix values
    assert_eq!(spec["env"]["A"], "pipeline");
    assert_eq!(spec["env"]["B"], "job-mysql");
    assert_eq!(spec["env"]["MATRIX_RUST"], "nightly");
    assert_eq!(spec["env"]["MATRIX_DB"], "mysql");
    assert_eq!(spec["services"][0]["image"], "mysql:latest");
    assert_eq!(spec["services"][0]["env"]["NAME"], "mysql");
    // a hash is the runner's to expand: it has the checkout
    assert_eq!(spec["caches"][0]["key"], "cargo-nightly-${hash:Cargo.lock}");
    assert_eq!(spec["steps"][0]["seq"], 1);
    assert_eq!(spec["steps"][0]["run"], "cargo test --features mysql");
    assert_eq!(spec["artifacts"][0]["name"], "logs-nightly-mysql");
    // the job that needs build waits for every build job
    let package = &jobs[4];
    assert_eq!(package.needs, ["build"]);
    assert_eq!(package.steps, [(Some("Zip".to_string()), "zip".to_string())]);
    assert_eq!(package.spec["timeoutMinutes"], 60);
    // the status contexts
    assert_eq!(
        context("check", "build", "nightly, mysql"),
        "ci/check/build (nightly, mysql)"
    );
    assert_eq!(context("check", "package", ""), "ci/check/package");
}

#[test]
fn a_component_repository_gets_the_built_in_pipeline() {
    let component = builtin::component(
        r#"<component name="mr-example" version="1.2.0"><depends-on name="mr-mantle"/>
            <depends-on name="framework" version="4.0.1"/></component>"#,
    )
    .unwrap();
    assert_eq!(component.name, "mr-example");
    assert_eq!(component.framework_version.as_deref(), Some("4.0.1"));
    let pipeline = builtin::pipeline(
        &component,
        "registry/moqui-build:{frameworkVersion}",
        None,
        3,
        &Limits::default(),
    );
    assert_eq!(pipeline.name, "component");
    assert_eq!(pipeline.events, [Event::Push, Event::MergeRequest]);
    assert!(applies(&pipeline, Event::Push, "refs/heads/any-branch"));
    assert!(!applies(&pipeline, Event::Tag, "refs/tags/v1"));
    let jobs = plan(&pipeline);
    assert_eq!(jobs.len(), 1);
    assert_eq!(
        context(&pipeline.name, &jobs[0].name, &jobs[0].matrix_key),
        "ci/component/test"
    );
    assert_eq!(jobs[0].image, "registry/moqui-build:4.0.1");
    let spec = &jobs[0].spec;
    assert_eq!(spec["env"]["MOQUI_CI_COMPONENT"], "mr-example");
    assert_eq!(spec["env"]["MOQUI_CI_FRAMEWORK_VERSION"], "4.0.1");
    assert_eq!(spec["env"]["MOQUI_CI_WORKERS"], "3");
    assert_eq!(spec["reports"][0], "test-results/*.xml");
    // the job is the component's regression suite, as ./moq test runs it, with its dependencies fetched
    let script = spec["steps"][0]["run"].as_str().unwrap();
    assert!(script.contains("./moq component-get \"$name\""), "{script}");
    assert!(
        script.contains("./moq test \"$name\" --nobuild --junit \"$src/test-results\" --workers"),
        "{script}"
    );
    // no framework version named: the image's, whatever it is
    let plain = builtin::component(r#"<component name="plain"/>"#).unwrap();
    let pipeline = builtin::pipeline(
        &plain,
        "moqui-build:{frameworkVersion}",
        Some("linux".into()),
        2,
        &Limits::default(),
    );
    assert_eq!(pipeline.jobs[0].image, "moqui-build:latest");
    assert_eq!(pipeline.jobs[0].pool.as_deref(), Some("linux"));
    assert!(!plan(&pipeline)[0].spec["env"]
        .as_object()
        .unwrap()
        .contains_key("MOQUI_CI_FRAMEWORK_VERSION"));
    // not a component.xml
    assert!(builtin::component("<moqui-conf/>").is_none());
    assert!(builtin::component("not xml").is_none());
    assert!(builtin::component(r#"<component name="bad name; rm -rf"/>"#).is_none());
}
