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

//! The pipeline file's reader: a valid file, each thing that is not valid with its line, the limits, and MR Moqui's
//! own `ci.xml`.

use super::{parse, Event, Limits};

const VALID: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ci xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:noNamespaceSchemaLocation="http://moqui.org/xsd/git-ci-3.xsd">
    <pipeline name="check" on="push merge-request" branches="main release/*">
        <env name="RUSTFLAGS" value="-D warnings"/>
        <job name="build" image="moqui-build:${matrix.rust}" pool="linux" timeout-minutes="30" fetch-depth="5">
            <env name="MODE" value="${matrix.db}"/>
            <secret name="DEPLOY_KEY" env="KEY"/>
            <service name="db" image="postgres:16"><env name="POSTGRES_PASSWORD" value="x"/></service>
            <matrix><axis name="rust" values="1.99 nightly"/><axis name="db" values="postgres mysql"/></matrix>
            <cache key="cargo-${hash:Cargo.lock}" path="target"/>
            <step run="cargo lint"/>
            <step name="Tests" run="./moq test-framework" timeout-minutes="20" continue-on-error="true"/>
            <artifact name="logs" path="runtime/log" retention-days="7"/>
            <report junit="test-results/*.xml"/>
        </job>
        <job name="package" image="moqui-build" needs="build" environment="staging">
            <step run="./moq archive"/>
        </job>
    </pipeline>
    <pipeline name="nightly" on="schedule manual" cron="0 0 3 * * ?">
        <job name="acceptance" image="moqui-build"><step run="./moq accept-all"/></job>
    </pipeline>
    <pipeline name="release" on="tag deploy" tags="v*">
        <job name="archive" image="moqui-build"><step run="./moq archive"/></job>
    </pipeline>
</ci>"#;

#[test]
fn a_valid_file_is_read_whole() {
    let file = parse(VALID, &Limits::default()).unwrap();
    assert_eq!(file.pipelines.len(), 3);
    let check = &file.pipelines[0];
    assert_eq!(check.name, "check");
    assert_eq!(check.events, [Event::Push, Event::MergeRequest]);
    assert_eq!(check.branches, ["main", "release/*"]);
    assert_eq!(check.env, [("RUSTFLAGS".to_string(), "-D warnings".to_string())]);
    let build = &check.jobs[0];
    assert_eq!(
        (
            build.image.as_str(),
            build.pool.as_deref(),
            build.timeout_minutes,
            build.fetch_depth
        ),
        ("moqui-build:${matrix.rust}", Some("linux"), 30, 5)
    );
    assert_eq!(
        (build.secrets[0].name.as_str(), build.secrets[0].env.as_str()),
        ("DEPLOY_KEY", "KEY")
    );
    assert_eq!(build.services[0].name, "db");
    assert_eq!(build.matrix.len(), 2);
    assert_eq!(build.matrix[0].values, ["1.99", "nightly"]);
    assert_eq!(
        (build.caches[0].key.as_str(), build.caches[0].path.as_str()),
        ("cargo-${hash:Cargo.lock}", "target")
    );
    assert_eq!(build.steps.len(), 2);
    assert_eq!(build.steps[1].name.as_deref(), Some("Tests"));
    assert_eq!(build.steps[1].timeout_minutes, Some(20));
    assert!(build.steps[1].continue_on_error && !build.steps[0].continue_on_error);
    assert_eq!(build.artifacts[0].retention_days, Some(7));
    assert_eq!(build.reports, ["test-results/*.xml"]);
    let package = &check.jobs[1];
    assert_eq!(package.needs, ["build"]);
    assert_eq!(package.environment.as_deref(), Some("staging"));
    // what a job leaves out is the server's default
    assert_eq!((package.timeout_minutes, package.fetch_depth), (60, 1));
    assert_eq!(file.pipelines[1].cron.as_deref(), Some("0 0 3 * * ?"));
    assert_eq!(file.pipelines[2].events, [Event::Tag, Event::Deploy]);
    assert_eq!(file.pipelines[2].tags, ["v*"]);
}

/// The error of a file made of `pipelines` inside `<ci>`, each on a line of its own from line 2.
fn error_of(pipelines: &str) -> String {
    parse(&format!("<ci>\n{pipelines}\n</ci>"), &Limits::default())
        .unwrap_err()
        .to_string()
}

fn job(body: &str) -> String {
    format!(r#"<pipeline name="p" on="push"><job name="j" image="i">{body}</job></pipeline>"#)
}

#[test]
fn each_thing_that_is_not_valid_is_an_error_that_names_its_line() {
    let step = r#"<step run="true"/>"#;
    let cases: Vec<(String, &str)> = vec![
        // the shape of the file
        ("<pipelines/>".into(), "<ci> has no <pipelines>"),
        (r#"<pipeline on="push"><job name="j" image="i"><step run="x"/></job></pipeline>"#.into(), "needs name"),
        (r#"<pipeline name="a b" on="push"/>"#.into(), "must be letters, digits"),
        (r#"<pipeline name="p" on="push" when="x"/>"#.into(), "has no attribute when"),
        (r#"<pipeline name="p"/>"#.into(), "needs on"),
        (r#"<pipeline name="p" on="commit"/>"#.into(), "'commit' is not an event"),
        (r#"<pipeline name="p" on="schedule"/>"#.into(), "runs on schedule and has no cron"),
        (r#"<pipeline name="p" on="push" cron="0 0 3 * * ?"/>"#.into(), "has a cron and does not run on schedule"),
        (r#"<pipeline name="p" on="schedule" cron="0 3 * * *"/>"#.into(), "needs six fields"),
        (r#"<pipeline name="p" on="push"/>"#.into(), "has no <job>"),
        (r#"<pipeline name="p" on="push"><stage/></pipeline>"#.into(), "<pipeline> has no <stage>"),
        // jobs
        (r#"<pipeline name="p" on="push"><job name="j"><step run="x"/></job></pipeline>"#.into(), "needs image"),
        (job(""), "job j has no <step>"),
        (job(r#"<step/>"#), "<step> needs run"),
        (job(r#"<step run="x" shell="bash"/>"#), "has no attribute shell"),
        (job(r#"<step run="x" continue-on-error="maybe"/>"#), "is not true or false"),
        (job(r#"<step run="x" timeout-minutes="0"/>"#), "is not a positive number"),
        (job(r#"<script run="x"/>"#), "<job> has no <script>"),
        (job(&format!(r#"{step}<env name="1X" value="y"/>"#)), "is not an environment variable name"),
        (job(&format!(r#"{step}<env name="X"/>"#)), "needs value"),
        (job(&format!(r#"{step}<secret name="has-dash"/>"#)), "must be environment variable names"),
        (job(&format!(r#"{step}<cache key="k" path="../outside"/>"#)), "must be inside the checkout"),
        (job(&format!(r#"{step}<cache key="k" path="/etc"/>"#)), "must be inside the checkout"),
        (job(&format!(r#"{step}<artifact name="a" path="x/../../y"/>"#)), "must be inside the checkout"),
        (job(&format!(r#"{step}<artifact name="a" path="x"/><artifact name="a" path="y"/>"#)), "two artifacts are named a"),
        (job(&format!(r#"{step}<report junit=".git/config"/>"#)), "must be inside the checkout"),
        (job(&format!(r#"{step}<service name="db" image="x"/><service name="db" image="y"/>"#)), "two services are named db"),
        (job(&format!(r#"{step}<service name="db" image="x"><volume/></service>"#)), "<service> has no <volume>"),
        (job(&format!(r#"{step}<matrix/>"#)), "<matrix> needs an <axis>"),
        (job(&format!(r#"{step}<matrix><axis name="a" values="x y"/></matrix><matrix><axis name="b" values="z"/></matrix>"#)), "two <matrix> elements"),
        (job(&format!(r#"{step}<matrix><axis name="a" values="x y"/><axis name="a" values="z"/></matrix>"#)), "two axes are named a"),
        (job(&format!(r#"{step}<matrix><axis name="a" values="ok no/slash"/></matrix>"#)), "the value 'no/slash' of axis a"),
        (
            r#"<pipeline name="p" on="push"><job name="j" image="i" timeout-minutes="100000"><step run="x"/></job></pipeline>"#.into(),
            "over this server's limit of 360",
        ),
        (
            r#"<pipeline name="p" on="push"><job name="j" image="i" fetch-depth="1000"><step run="x"/></job></pipeline>"#.into(),
            "fetch-depth 1000 of job j is over",
        ),
        (
            r#"<pipeline name="p" on="push"><job name="j" image="a b"><step run="x"/></job></pipeline>"#.into(),
            "has a space in it",
        ),
        (
            r#"<pipeline name="p" on="push"><job name="j" image="i"><step run="x"/></job><job name="j" image="i"><step run="x"/></job></pipeline>"#.into(),
            "two jobs of pipeline p are named j",
        ),
        // needs
        (
            r#"<pipeline name="p" on="push"><job name="j" image="i" needs="other"><step run="x"/></job></pipeline>"#.into(),
            "needs other, which is not another job",
        ),
        (
            r#"<pipeline name="p" on="push"><job name="a" image="i" needs="b"><step run="x"/></job><job name="b" image="i" needs="a"><step run="x"/></job></pipeline>"#.into(),
            "need each other in a circle",
        ),
        // two pipelines of one name
        (
            format!("{}\n{}", job(step), job(step)),
            "two pipelines are named p",
        ),
    ];
    for (text, expected) in cases {
        let error = error_of(&text);
        assert!(
            error.contains(expected),
            "{text}\n  gave: {error}\n  wanted: {expected}"
        );
        assert!(error.starts_with("ci.xml line "), "{error}");
    }
    // the line is the element's
    let error = error_of("<pipeline name=\"p\" on=\"push\">\n<job name=\"j\" image=\"i\">\n<step/>\n</job></pipeline>");
    assert!(error.starts_with("ci.xml line 4:"), "{error}");
    // not XML, another root, an empty file
    assert!(parse("<ci><pipeline", &Limits::default())
        .unwrap_err()
        .to_string()
        .contains("not well-formed XML"));
    assert!(parse("<gitecas/>", &Limits::default())
        .unwrap_err()
        .to_string()
        .contains("the root element is <gitecas>, not <ci>"));
    assert!(parse("<ci/>", &Limits::default())
        .unwrap_err()
        .to_string()
        .contains("<ci> has no <pipeline>"));
}

#[test]
fn the_limits_of_the_server_bound_a_file() {
    let limits = Limits {
        max_bytes: 2000,
        max_pipelines: 2,
        max_jobs: 3,
        max_matrix: 4,
        max_steps: 2,
        ..Limits::default()
    };
    let error = |text: &str| parse(text, &limits).unwrap_err().to_string();
    let pipeline = |name: &str| {
        format!(r#"<pipeline name="{name}" on="push"><job name="j" image="i"><step run="x"/></job></pipeline>"#)
    };
    assert!(
        error(&format!("<ci>{}{}{}</ci>", pipeline("a"), pipeline("b"), pipeline("c")))
            .contains("3 pipelines, over this server's limit of 2")
    );
    assert!(error(&format!(
        "<ci>{}</ci>",
        job(r#"<step run="1"/><step run="2"/><step run="3"/>"#)
    ))
    .contains("3 steps, over this server's limit of 2"));
    assert!(error(&format!(
        "<ci>{}</ci>",
        job(r#"<step run="1"/><matrix><axis name="a" values="1 2 3"/><axis name="b" values="1 2"/></matrix>"#)
    ))
    .contains("6 combinations, over this server's limit of 4"));
    // the jobs of a pipeline are counted after its matrices are expanded
    let jobs = r#"<pipeline name="p" on="push">
        <job name="a" image="i"><step run="1"/><matrix><axis name="x" values="1 2 3"/></matrix></job>
        <job name="b" image="i"><step run="1"/></job></pipeline>"#;
    assert!(error(&format!("<ci>{jobs}</ci>")).contains("4 jobs, over this server's limit of 3"));
    let big = format!("<ci>{}<!-- {} --></ci>", pipeline("a"), "x".repeat(2000));
    assert!(error(&big).contains("over this server's limit of 2000"));
}

#[test]
fn mr_moquis_own_pipelines_are_valid() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ci.xml");
    let text = std::fs::read_to_string(&path).unwrap();
    let file = parse(&text, &Limits::default()).unwrap();
    let names: Vec<&str> = file.pipelines.iter().map(|pipeline| pipeline.name.as_str()).collect();
    assert_eq!(names, ["check", "main", "nightly", "release"]);
    let check = &file.pipelines[0];
    assert_eq!(check.events, [Event::Push, Event::MergeRequest]);
    let commands: Vec<&str> = check
        .jobs
        .iter()
        .flat_map(|job| job.steps.iter().map(|step| step.run.as_str()))
        .collect();
    for wanted in ["cargo lint", "scripts/check-packages.sh", "./moq test-framework"] {
        assert!(commands.contains(&wanted), "the check pipeline runs {wanted}");
    }
    assert_eq!(file.pipelines[2].events, [Event::Schedule, Event::Manual]);
    assert!(file.pipelines[2].jobs[0]
        .steps
        .iter()
        .any(|step| step.run.contains("accept-all --junit")));
    assert_eq!(file.pipelines[3].events, [Event::Tag]);
    assert!(!file.pipelines[3].jobs[0].artifacts.is_empty());
    // every job names the build image and a pool, so the pool's allowed images and limits decide what runs
    for job in file.pipelines.iter().flat_map(|pipeline| &pipeline.jobs) {
        assert_eq!(
            (job.image.as_str(), job.pool.as_deref()),
            ("moqui-build", Some("linux"))
        );
    }
}
