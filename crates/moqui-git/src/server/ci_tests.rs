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

//! The CI engine end to end against the fake host type (plans/Dogfood.md §14): a real push with the `git` command
//! queues a run through the framework's gitECA rule; the start job starts a container on the fake host with a job
//! token; the runner (`moqui ci-run`'s logic, `ci::runner`) runs the job in this process against the runner REST,
//! with its checkout a real shallow fetch with the token; and the engine writes the statuses and removes the
//! container. With them the safety rules (plans/Dogfood.md §8.4), the job token's scope, timeouts and the cleanup of
//! lost containers, retention, the built-in pipeline and registered runners.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use moqui_lib::object::{to_plain_string, Value};

use super::hosting_tests::{call, grant_entities, ok, s, Work as Cloned};
use super::review::tests::merge_requests;
use super::tests::{
    create, define, fixture_with, git_available, git_in, git_ok, http_server, settled, url_with, write_file, Fixture,
    READER, WRITER,
};
use super::{GitHttpRequest, GitService};
use crate::ci::driver::fake::FakeHosts;
use crate::ci::file::Event;
use crate::ci::queue::{self, RunRequest};
use crate::ci::runner::{self, Options, Transport};
use crate::ci::runner_http::{self, MemoryResponse, RunnerHttpRequest};
use crate::ci::{artifact, engine, token};
use crate::service::commit::tests::internal;
use moqui_core::entity::EntityValue;

const SERVICES: &str = "org.moqui.git.GitCiServices";
const RUN_URL: &str = "https://ci.example.com/{repositoryPath}/runs/{runNum}/jobs/{jobRunId}";

/// The framework's rules as a server has them: its gitECA rules but the report definitions' pull (a test factory has no report services) and the
/// push mirror (an async call, and a test factory's service pool does not run).
fn framework_rules(fixture: &Fixture) {
    let mut xml =
        std::fs::read_to_string(moqui_core::context::framework_dir().join("service/Git.gitecas.xml")).unwrap();
    for rule in ["FrameworkPullDefinitionGitSyncs", "FrameworkPushMirror"] {
        let id = xml.find(&format!(r#"id="{rule}""#)).unwrap();
        let start = xml[..id].rfind("<giteca ").unwrap();
        let end = id + xml[id..].find("</giteca>").unwrap() + "</giteca>".len();
        xml.replace_range(start..end, "");
    }
    assert!(
        xml.contains("org.moqui.git.GitCiServices.queue#PushRuns"),
        "the pipelines rule is there"
    );
    // and the service ECA rule that queues a merge request's runs after its events
    let secas = std::fs::read_to_string(moqui_core::context::framework_dir().join("service/GitCi.secas.xml")).unwrap();
    assert!(
        secas.contains("org.moqui.git.GitCiServices.queue#MergeRequestRuns"),
        "the merge request rule is there"
    );
    moqui_core::service::git_eca::install_with_secas(fixture.context.factory(), &secas, &xml);
}

/// A fixture with the engine on, jobs started only when a test says so, the fake host type with a host that answers
/// as `mode` says, and two pools named linux: a trusted one and one for untrusted runs.
fn ci_fixture(name: &str, mode: &str, more_conf: &str) -> Fixture {
    let fixture = fixture_with(
        name,
        moqui_core::context::framework_dir(),
        &format!(r#"<git-ci start-inline="false" run-url="{RUN_URL}" lost-seconds="1" {more_conf}/>"#),
    );
    grant_entities(&fixture);
    framework_rules(&fixture);
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        create(
            &fixture.entity,
            "moqui.server.instance.InstanceHostType",
            &[
                ("hostTypeId", "fake"),
                ("runStartService", "org.moqui.dev.InstanceServices.start#RunFake"),
                ("runInspectService", "org.moqui.dev.InstanceServices.inspect#RunFake"),
                ("runRemoveService", "org.moqui.dev.InstanceServices.remove#RunFake"),
                ("runCleanupService", "org.moqui.dev.InstanceServices.cleanup#RunsFake"),
            ],
        );
        create(
            &fixture.entity,
            "moqui.server.instance.InstanceHost",
            &[
                ("instanceHostId", "FAKE"),
                ("hostTypeId", "fake"),
                ("hostAddress", mode),
            ],
        );
    }
    for (id, trusted) in [("P_TRUSTED", true), ("P_OUTSIDE", false)] {
        ok(
            &fixture,
            WRITER,
            &format!("{SERVICES}.store#RunnerPool"),
            &[
                ("runnerPoolId", s(id)),
                ("poolName", s("linux")),
                ("instanceHostId", s("FAKE")),
                ("trusted", Value::Bool(trusted)),
                ("maxConcurrent", Value::I64(2)),
                ("allowedImages", s("moqui-build* registry.example.com/ci/*")),
                ("cpuLimit", s("2.5")),
                ("memoryMb", Value::I64(4096)),
                ("maxMinutes", Value::I64(45)),
                ("egressRule", s("ci-egress")),
            ],
        );
    }
    fixture
}

fn rows(fixture: &Fixture, entity: &str, conditions: &[(&str, &str)]) -> Vec<EntityValue> {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut find = fixture.entity.find(entity).unwrap();
    for (field, value) in conditions {
        find = find.eq(field, s(value));
    }
    find.list().unwrap().iter().cloned().collect()
}

fn text(row: &EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

fn set(fixture: &Fixture, entity: &str, key: &str, id: &str, fields: &[(&str, Value)]) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let changed = fixture
        .entity
        .find(entity)
        .unwrap()
        .eq(key, s(id))
        .update_all(fields.iter().map(|(name, value)| ((*name).to_string(), value.clone())))
        .unwrap();
    assert_eq!(changed, 1, "{entity} {id}");
}

/// The latest state of each `ci/` status context of a commit.
fn statuses(fixture: &Fixture, repository: &str, commit: &str) -> BTreeMap<String, (String, String, String)> {
    let mut all = rows(
        fixture,
        "moqui.resource.GitCommitStatus",
        &[("repositoryId", repository), ("commitId", commit)],
    );
    all.sort_by_key(|row| text(row, "commitStatusId"));
    all.into_iter()
        .map(|row| {
            (
                text(&row, "context"),
                (text(&row, "state"), text(&row, "description"), text(&row, "targetUrl")),
            )
        })
        .collect()
}

/// A work tree of a repository, pushed with the real git client.
struct Work {
    dir: PathBuf,
    home: PathBuf,
}

static WORKS: AtomicU32 = AtomicU32::new(0);

impl Work {
    fn new(fixture: &Fixture, base: &str, user: &str, id: &str) -> Self {
        let n = WORKS.fetch_add(1, Ordering::Relaxed);
        let dir = fixture.dir.join(format!("work-{id}-{n}"));
        let home = fixture.dir.join(format!("home-{n}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        git_ok(&dir, &home, &["init", "-q", "."]);
        git_ok(&dir, &home, &["remote", "add", "origin", &url_with(base, user, id)]);
        Self { dir, home }
    }

    /// Commit `files` and push `branch`. The commit's id.
    fn push(&self, fixture: &Fixture, files: &[(&str, &str)], branch: &str) -> String {
        for (name, content) in files {
            write_file(&self.dir, name, content);
        }
        git_ok(&self.dir, &self.home, &["checkout", "-q", "-B", branch]);
        git_ok(&self.dir, &self.home, &["add", "-A"]);
        git_ok(&self.dir, &self.home, &["commit", "-q", "-m", "change"]);
        git_ok(&self.dir, &self.home, &["push", "-q", "origin", branch]);
        for update in settled(fixture) {
            assert_ne!(text(&update, "ecaStatus"), "failed", "{}", text(&update, "ecaError"));
        }
        git_ok(&self.dir, &self.home, &["rev-parse", "HEAD"]).trim().to_string()
    }
}

/// The runner's transport in this process: the runner REST served here, and the checkout a real fetch over HTTP.
struct Local<'a> {
    fixture: &'a Fixture,
    base: String,
    job_run_id: String,
    token: String,
}

impl Local<'_> {
    fn request(&self, method: &str, path_and_query: &str, body: &[u8]) -> MemoryResponse {
        let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));
        let request = RunnerHttpRequest {
            method: method.to_string(),
            path: format!("jobs/{}/{path}", self.job_run_id),
            query: query.to_string(),
            authorization: Some(format!("Bearer {}", self.token)),
            content_length: Some(body.len() as u64),
            secure: true,
            client_ip: "127.0.0.1".into(),
        };
        let _entered = self.fixture.context.enter();
        let mut response = MemoryResponse::default();
        let mut input = body;
        runner_http::serve(&request, &mut input, &mut response);
        response
    }
}

fn json_of(response: &MemoryResponse) -> serde_json::Value {
    serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null)
}

impl Transport for Local<'_> {
    fn json(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, serde_json::Value), String> {
        let bytes = body.map(|body| body.to_string().into_bytes()).unwrap_or_default();
        let response = self.request(method, path, &bytes);
        Ok((response.status, json_of(&response)))
    }

    fn upload(&self, path_and_query: &str, file: &Path) -> Result<(u16, serde_json::Value), String> {
        let bytes = std::fs::read(file).map_err(|err| err.to_string())?;
        let response = self.request("PUT", path_and_query, &bytes);
        Ok((response.status, json_of(&response)))
    }

    fn download(&self, path_and_query: &str, to: &Path) -> Result<u16, String> {
        let response = self.request("GET", path_and_query, &[]);
        if response.status == 200 {
            std::fs::write(to, &response.body).map_err(|err| err.to_string())?;
        }
        Ok(response.status)
    }

    fn checkout(&self, job: &serde_json::Value, workspace: &Path) -> Result<String, String> {
        // the server's own clone URL is its public one; the test's listener is elsewhere
        let path = job["repositoryPath"].as_str().unwrap();
        runner::http::checkout(
            &format!("{}/git/{path}.git", self.base),
            job["commitId"].as_str().unwrap(),
            job["spec"]["fetchDepth"].as_u64().unwrap_or(1),
            &self.token,
            workspace,
        )
    }
}

/// Start the queued jobs. What the start pass did.
fn start(fixture: &Fixture) -> engine::Started {
    let _entered = fixture.context.enter();
    engine::start_queued(&fixture.entity).unwrap()
}

/// The fake container of a running job run: its id and its job token.
fn container_of(fixture: &Fixture, job_run_id: &str) -> (String, String) {
    let containers = FakeHosts::of(fixture.context.factory()).containers();
    let (id, container) = containers
        .iter()
        .find(|(_, container)| container.labels.get("moqui.ci.job").map(String::as_str) == Some(job_run_id))
        .unwrap_or_else(|| panic!("no container for job run {job_run_id}: {containers:?}"));
    (id.clone(), container.env["MOQUI_CI_TOKEN"].clone())
}

/// Play the runner of a running job run in this process. The runner's exit code and the transport, to call again.
fn run_job<'a>(fixture: &'a Fixture, base: &str, job_run_id: &str, job_token: &str) -> (i32, Local<'a>) {
    let n = WORKS.fetch_add(1, Ordering::Relaxed);
    let transport = Local {
        fixture,
        base: base.to_string(),
        job_run_id: job_run_id.to_string(),
        token: job_token.to_string(),
    };
    let options = Options {
        workspace: fixture.dir.join(format!("runner-{n}/src")),
        flush: std::time::Duration::from_millis(50),
        ..Options::default()
    };
    (runner::run(&transport, &options), transport)
}

fn job_named(fixture: &Fixture, run_id: &str, name: &str) -> EntityValue {
    rows(
        fixture,
        "moqui.resource.GitJobRun",
        &[("pipelineRunId", run_id), ("jobName", name)],
    )
    .into_iter()
    .rfind(|job| text(job, "superseded") != "Y")
    .unwrap_or_else(|| panic!("run {run_id} has no job {name}"))
}

fn log_of(fixture: &Fixture, user: &str, job_run_id: &str) -> String {
    let out = ok(
        fixture,
        user,
        &format!("{SERVICES}.get#JobLog"),
        &[("jobRunId", s(job_run_id)), ("maxBytes", Value::I64(1_000_000))],
    );
    assert_eq!(out["finished"], Value::Bool(true));
    to_plain_string(&out["text"])
}

const JUNIT: &str = r#"<testsuite name="unit" tests="2" failures="1"><testcase name="passes"/><testcase name="breaks" file="src/lib.rs" line="12"><failure message="expected 1, got 2"/></testcase></testsuite>"#;

fn pipeline_file() -> String {
    // the report's text, written by a step, inside an attribute
    let junit = JUNIT.replace('<', "&lt;").replace('"', "&quot;");
    format!(
        r#"<ci>
    <pipeline name="check" on="push merge-request manual">
        <env name="GREETING" value="hello"/>
        <job name="test" image="moqui-build:1" pool="linux" timeout-minutes="20">
            <secret name="DEPLOY_KEY"/>
            <cache key="deps-${{hash:lock.txt}}" path="deps"/>
            <step name="Build" run="echo &quot;$GREETING from $MOQUI_CI_JOB at $MOQUI_CI_COMMIT&quot;; test -f marker.txt; echo &quot;key=[$DEPLOY_KEY] token=[$MOQUI_CI_TOKEN]&quot;"/>
            <step run="if test -f deps/stamp; then echo cache hit; else mkdir -p deps; echo made > deps/stamp; echo cache miss; fi"/>
            <step run="mkdir -p test-results dist; printf '%s' '{junit}' > test-results/TEST-unit.xml; echo built > dist/app.txt; cat fail.txt 2>/dev/null; test ! -f fail.txt"/>
            <artifact name="app" path="dist/*.txt" retention-days="3"/>
            <report junit="test-results/*.xml"/>
        </job>
        <job name="after" image="moqui-build:1" pool="linux" needs="test"><step run="echo after"/></job>
    </pipeline>
</ci>"#
    )
}

/// A repository `id` with the pipeline file, pushed by the writer. The work tree and the commit.
fn pushed(fixture: &Fixture, base: &str, id: &str, more: &[(&str, &str)]) -> (Work, String) {
    define(fixture, id, "db");
    let work = Work::new(fixture, base, WRITER, id);
    let file = pipeline_file();
    let mut files = vec![("ci.xml", file.as_str()), ("marker.txt", "here"), ("lock.txt", "v1")];
    files.extend_from_slice(more);
    let commit = work.push(fixture, &files, "main");
    (work, commit)
}

fn only_run(fixture: &Fixture, repository: &str, commit: &str) -> EntityValue {
    let runs = rows(
        fixture,
        "moqui.resource.GitPipelineRun",
        &[("repositoryId", repository), ("commitId", commit)],
    );
    assert_eq!(runs.len(), 1, "one run for {commit}");
    runs.into_iter().next().unwrap()
}

/// Start what is queued and play the runner of job `name` of the run. Its job run id and the runner's exit code.
fn start_and_run(fixture: &Fixture, base: &str, run_id: &str, name: &str) -> (String, i32) {
    start(fixture);
    let job = job_named(fixture, run_id, name);
    assert_eq!(
        text(&job, "statusId"),
        "running",
        "job {name}: {}",
        text(&job, "statusMessage")
    );
    let job_run_id = text(&job, "jobRunId");
    let (_, job_token) = container_of(fixture, &job_run_id);
    let (code, _) = run_job(fixture, base, &job_run_id, &job_token);
    (job_run_id, code)
}

#[test]
fn a_push_runs_a_pipeline_on_the_fake_host_to_its_statuses() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-push", "success", "");
    let base = http_server(fixture.context.factory());
    define(&fixture, "app", "db");
    // main is protected, so its runs are given the repository's protected secrets
    ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.store#GitBranchRule",
        &[("repositoryId", s("app")), ("refPattern", s("main"))],
    );
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.store#CiSecret"),
        &[
            ("repositoryId", s("app")),
            ("name", s("DEPLOY_KEY")),
            ("value", s("s3cr3t-deploy-key")),
        ],
    );
    let work = Work::new(&fixture, &base, WRITER, "app");
    let file = pipeline_file();
    let commit = work.push(
        &fixture,
        &[("ci.xml", &file), ("marker.txt", "here"), ("lock.txt", "v1")],
        "main",
    );

    // queued by the push: the run, its jobs, their pending statuses
    let run = only_run(&fixture, "app", &commit);
    let run_id = text(&run, "pipelineRunId");
    assert_eq!(
        (
            text(&run, "pipelineName"),
            text(&run, "event"),
            text(&run, "refName"),
            text(&run, "statusId"),
            text(&run, "trusted"),
            text(&run, "protectedRef"),
            text(&run, "runNum"),
            text(&run, "definitionSource"),
            text(&run, "triggeredByUserId"),
        ),
        (
            "check".into(),
            "push".into(),
            "refs/heads/main".into(),
            "queued".into(),
            "Y".into(),
            "Y".into(),
            "1".into(),
            "file".into(),
            WRITER.into()
        )
    );
    assert_eq!(text(&job_named(&fixture, &run_id, "test"), "statusId"), "queued");
    assert_eq!(text(&job_named(&fixture, &run_id, "after"), "statusId"), "waiting");
    let pending = statuses(&fixture, "app", &commit);
    assert_eq!(pending["ci/check/test"].0, "pending");
    assert_eq!(pending["ci/check/after"].0, "pending");

    // started: one container on the fake host, in the trusted pool, with the pool's limits and the job token
    assert_eq!(start(&fixture), engine::Started { started: 1, refused: 0 });
    let job = job_named(&fixture, &run_id, "test");
    let job_run_id = text(&job, "jobRunId");
    assert_eq!(
        (
            text(&job, "statusId"),
            text(&job, "runnerPoolId"),
            text(&job, "timeoutMinutes")
        ),
        ("running".into(), "P_TRUSTED".into(), "20".into())
    );
    let hosts = FakeHosts::of(fixture.context.factory());
    let started = hosts.calls().into_iter().find(|call| call.action == "start").unwrap();
    assert_eq!(to_plain_string(&started.parameters["image"]), "moqui-build:1");
    assert_eq!(
        to_plain_string(&started.parameters["name"]),
        format!("moqui-ci-{job_run_id}")
    );
    assert_eq!(to_plain_string(&started.parameters["cpuLimit"]), "2.5");
    assert_eq!(to_plain_string(&started.parameters["memoryMb"]), "4096");
    assert_eq!(to_plain_string(&started.parameters["networkMode"]), "ci-egress");
    assert_eq!(
        started.parameters["command"],
        Value::List(vec![s("moqui"), s("ci-run")])
    );
    let (container, job_token) = container_of(&fixture, &job_run_id);
    let held = &hosts.containers()[&container];
    assert_eq!(held.env["MOQUI_CI_JOB_RUN_ID"], job_run_id);
    assert!(held.env.contains_key("MOQUI_CI_SERVER_URL"));
    // nothing of the job's secrets is in the container's environment: the runner asks for them
    assert!(
        !held.env.values().any(|value| value.contains("s3cr3t")),
        "{:?}",
        held.env
    );
    assert_eq!(held.labels["moqui.ci.run"], run_id);
    assert!(job_token.starts_with("mqcj_") && text(&job, "tokenHash") == token::hash(&job_token));

    // the runner: checkout with the token, the steps, the log, a report, an artifact, a cache
    let (code, transport) = run_job(&fixture, &base, &job_run_id, &job_token);
    assert_eq!(code, 0);
    let job = job_named(&fixture, &run_id, "test");
    assert_eq!(
        (text(&job, "statusId"), text(&job, "exitCode")),
        ("success".into(), "0".into())
    );
    let log = log_of(&fixture, READER, &job_run_id);
    assert!(log.contains(&format!("hello from test at {commit}")), "{log}");
    assert!(log.contains("cache miss") && log.contains("$ Build"), "{log}");
    // the secret is given (the brackets are not empty) and masked as it is stored; the job token is not passed on
    assert!(log.contains("key=[***] token=[]"), "{log}");
    assert!(!log.contains("s3cr3t"), "{log}");
    // the chunks became one blob
    assert!(!text(&job, "logBlobId").is_empty());
    assert!(rows(&fixture, "moqui.resource.GitRunLogChunk", &[("jobRunId", &job_run_id)]).is_empty());
    let read = ok(
        &fixture,
        READER,
        &format!("{SERVICES}.get#JobRun"),
        &[("jobRunId", s(&job_run_id))],
    );
    let Value::List(steps) = &read["stepList"] else {
        panic!("stepList")
    };
    let step_states: Vec<String> = steps
        .iter()
        .map(|step| match step {
            Value::Map(step) => format!(
                "{}:{}",
                to_plain_string(&step["statusId"]),
                to_plain_string(&step["exitCode"])
            ),
            _ => String::new(),
        })
        .collect();
    assert_eq!(step_states, ["success:0", "success:0", "success:0"]);
    let Value::List(failures) = &read["failedTestList"] else {
        panic!("failedTestList")
    };
    assert_eq!(failures.len(), 1);
    let Value::Map(failure) = &failures[0] else {
        panic!("failure")
    };
    assert_eq!(
        (
            to_plain_string(&failure["suiteName"]),
            to_plain_string(&failure["testName"]),
            to_plain_string(&failure["filePath"]),
            to_plain_string(&failure["lineNumber"]),
            to_plain_string(&failure["message"]),
        ),
        (
            "unit".into(),
            "breaks".into(),
            "src/lib.rs".into(),
            "12".into(),
            "expected 1, got 2".into()
        )
    );
    assert_eq!(
        (text(&job, "testCount"), text(&job, "testFailedCount")),
        ("2".into(), "1".into())
    );
    // the artifact and the report are downloads; the cache is not listed
    let Value::List(artifacts) = &read["artifactList"] else {
        panic!("artifactList")
    };
    let kinds: Vec<String> = artifacts
        .iter()
        .map(|artifact| match artifact {
            Value::Map(artifact) => format!(
                "{}:{}",
                to_plain_string(&artifact["artifactKind"]),
                to_plain_string(&artifact["fileName"])
            ),
            _ => String::new(),
        })
        .collect();
    assert_eq!(kinds, ["report:TEST-unit.xml", "artifact:app.tar.gz"]);
    let report_id = match &artifacts[0] {
        Value::Map(artifact) => to_plain_string(&artifact["artifactId"]),
        _ => String::new(),
    };
    let download = ok(
        &fixture,
        READER,
        &format!("{SERVICES}.get#RunArtifact"),
        &[("artifactId", s(&report_id))],
    );
    assert_eq!(download["bytes"], Value::Bytes(JUNIT.as_bytes().to_vec()));
    assert_eq!(download["done"], Value::Bool(true));
    let caches = rows(
        &fixture,
        "moqui.resource.GitRunArtifact",
        &[("repositoryId", "app"), ("artifactKind", "cache")],
    );
    assert_eq!(caches.len(), 1);
    assert_eq!(text(&caches[0], "protectedRef"), "Y");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.get#RunArtifact"),
        &[("artifactId", s(&text(&caches[0], "artifactId")))],
    );
    assert!(errors.contains("no such artifact"), "{errors}");

    // the status, with the run's page as its target, and the container removed
    let done = statuses(&fixture, "app", &commit);
    assert_eq!(done["ci/check/test"].0, "success");
    assert!(
        done["ci/check/test"].1.starts_with("Succeeded in "),
        "{:?}",
        done["ci/check/test"]
    );
    assert_eq!(
        done["ci/check/test"].2,
        format!("https://ci.example.com/app/runs/1/jobs/{job_run_id}")
    );
    assert!(hosts.containers().is_empty());
    assert!(hosts
        .calls()
        .iter()
        .any(|call| call.action == "remove" && to_plain_string(&call.parameters["instanceRef"]) == container));

    // the job token is dead: the runner REST and the git server refuse it
    assert_eq!(transport.request("GET", "job", &[]).status, 401);
    assert_eq!(transport.request("POST", "log", br#"{"text":"late"}"#).status, 401);
    let late = fixture.dir.join("late");
    std::fs::create_dir_all(&late).unwrap();
    assert!(runner::http::checkout(&format!("{base}/git/app.git"), &commit, 1, &job_token, &late).is_err());

    // the job that needed it is queued now, runs, and the run ends
    assert_eq!(text(&job_named(&fixture, &run_id, "after"), "statusId"), "queued");
    let (_, code) = start_and_run(&fixture, &base, &run_id, "after");
    assert_eq!(code, 0);
    let run = only_run(&fixture, "app", &commit);
    assert_eq!(text(&run, "statusId"), "success");
    assert_eq!(statuses(&fixture, "app", &commit)["ci/check/after"].0, "success");
    // usage, for a quota
    let usage = ok(
        &fixture,
        READER,
        &format!("{SERVICES}.get#RunnerUsage"),
        &[("repositoryId", s("app"))],
    );
    assert_eq!(to_plain_string(&usage["jobCount"]), "2");
    assert_eq!(to_plain_string(&usage["minutesUsed"]), "2");
    // the run reads as its repository does
    let got = ok(
        &fixture,
        READER,
        &format!("{SERVICES}.get#PipelineRun"),
        &[("repositoryId", s("app")), ("runNum", Value::I64(1))],
    );
    let Value::List(jobs) = &got["jobList"] else {
        panic!("jobList")
    };
    assert_eq!(jobs.len(), 2);
    assert_eq!(got["canUpdate"], Value::Bool(false));
    let found = ok(
        &fixture,
        READER,
        &format!("{SERVICES}.find#PipelineRuns"),
        &[("repositoryId", s("app"))],
    );
    assert_eq!(to_plain_string(&found["runCount"]), "1");

    // a second push with the same lock file restores the cache
    let second = work.push(&fixture, &[("marker.txt", "again")], "main");
    let run = only_run(&fixture, "app", &second);
    assert_eq!(text(&run, "runNum"), "2");
    let (second_job, code) = start_and_run(&fixture, &base, &text(&run, "pipelineRunId"), "test");
    assert_eq!(code, 0);
    let log = log_of(&fixture, WRITER, &second_job);
    assert!(log.contains("cache hit") && log.contains("restored to deps"), "{log}");
    assert_eq!(
        rows(
            &fixture,
            "moqui.resource.GitRunArtifact",
            &[("repositoryId", "app"), ("artifactKind", "cache")]
        )
        .len(),
        1,
        "the first cache of a key stands"
    );
}

#[test]
fn a_failed_step_fails_the_job_skips_what_needs_it_and_a_retry_runs_it_again() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-fail", "success", "");
    let base = http_server(fixture.context.factory());
    let (work, commit) = pushed(&fixture, &base, "app", &[("fail.txt", "the build is broken")]);
    let run = only_run(&fixture, "app", &commit);
    let run_id = text(&run, "pipelineRunId");
    // not a protected branch: trusted, and no secret of the protected scope
    assert_eq!(
        (text(&run, "trusted"), text(&run, "protectedRef")),
        ("Y".into(), "N".into())
    );
    let (job_run_id, code) = start_and_run(&fixture, &base, &run_id, "test");
    assert_eq!(code, 1);
    let job = job_named(&fixture, &run_id, "test");
    assert_eq!(text(&job, "statusId"), "failure");
    let log = log_of(&fixture, WRITER, &job_run_id);
    assert!(
        log.contains("the build is broken") && log.contains("The step failed with exit code 1"),
        "{log}"
    );
    // the reports and artifacts of a failed job are still uploaded
    assert_eq!(
        rows(&fixture, "moqui.resource.GitRunArtifact", &[("jobRunId", &job_run_id)]).len(),
        2
    );
    let after = job_named(&fixture, &run_id, "after");
    assert_eq!(text(&after, "statusId"), "skipped");
    let ended = statuses(&fixture, "app", &commit);
    assert_eq!(ended["ci/check/test"].0, "failure");
    assert_eq!(
        (ended["ci/check/after"].0.as_str(), ended["ci/check/after"].1.as_str()),
        ("failure", "Skipped: job test did not succeed")
    );
    assert_eq!(text(&only_run(&fixture, "app", &commit), "statusId"), "failure");
    assert!(FakeHosts::of(fixture.context.factory()).containers().is_empty());

    // a retry needs the update right, and makes a new attempt in the same run
    let (_, errors) = call(
        &fixture,
        READER,
        &format!("{SERVICES}.retry#JobRun"),
        &[("jobRunId", s(&job_run_id))],
    );
    assert!(errors.contains("needs the update right"), "{errors}");
    let retried = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.retry#JobRun"),
        &[("jobRunId", s(&job_run_id))],
    );
    let next_id = to_plain_string(&retried["jobRunId"]);
    assert_ne!(next_id, job_run_id);
    assert_eq!(text(&job_named(&fixture, &run_id, "test"), "jobRunId"), next_id);
    assert_eq!(text(&job_named(&fixture, &run_id, "after"), "statusId"), "waiting");
    assert_eq!(text(&only_run(&fixture, "app", &commit), "statusId"), "queued");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.retry#JobRun"),
        &[("jobRunId", s(&job_run_id))],
    );
    assert!(errors.contains("already run again"), "{errors}");

    // cancel: the queued attempt and the waiting job are cancelled, and a run that ended is not cancelled twice
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    assert_eq!(text(&only_run(&fixture, "app", &commit), "statusId"), "cancelled");
    assert_eq!(text(&job_named(&fixture, &run_id, "test"), "statusId"), "cancelled");
    assert_eq!(text(&job_named(&fixture, &run_id, "after"), "statusId"), "cancelled");
    assert_eq!(statuses(&fixture, "app", &commit)["ci/check/test"].0, "error");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    assert!(errors.contains("has ended"), "{errors}");
    assert_eq!(start(&fixture), engine::Started::default());

    // a running job is cancelled with its container, and its runner is told to stop
    let third = work.push(&fixture, &[("fail.txt", "still broken")], "main");
    let run_id = text(&only_run(&fixture, "app", &third), "pipelineRunId");
    start(&fixture);
    let running = text(&job_named(&fixture, &run_id, "test"), "jobRunId");
    let (_, job_token) = container_of(&fixture, &running);
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    assert!(FakeHosts::of(fixture.context.factory()).containers().is_empty());
    let (code, _) = run_job(&fixture, &base, &running, &job_token);
    assert_eq!(code, runner::EXIT_GONE);

    // a manual run needs the update right and a pipeline with the manual event
    let (_, errors) = call(
        &fixture,
        READER,
        &format!("{SERVICES}.run#Pipeline"),
        &[("repositoryId", s("app")), ("pipelineName", s("check"))],
    );
    assert!(errors.contains("needs the update right"), "{errors}");
    let manual = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.run#Pipeline"),
        &[
            ("repositoryId", s("app")),
            ("pipelineName", s("check")),
            ("refName", s("main")),
        ],
    );
    assert_eq!(to_plain_string(&manual["statusId"]), "queued");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.run#Pipeline"),
        &[("repositoryId", s("app")), ("pipelineName", s("nope"))],
    );
    assert!(
        errors.contains("has no pipeline nope with the manual event"),
        "{errors}"
    );
}

/// The request of a run as a fork's merge request makes it, for what the application's check is asked and answers
/// (a real fork's merge request is in `an_untrusted_run_waits_for_approval...`).
fn outside_request(repository: &str, commit: &str) -> RunRequest {
    let mut request = RunRequest::new(repository, Event::MergeRequest, "refs/heads/main", commit);
    request.user_id = Some(READER.to_string());
    request.merge_request_id = Some("MR1".to_string());
    request.source_repository_id = Some("someones-fork".to_string());
    request
}

fn queue_request(fixture: &Fixture, request: &RunRequest) -> Vec<queue::Queued> {
    let _entered = fixture.context.enter();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    queue::queue(&provider, &fixture.entity, request).unwrap()
}

#[test]
fn an_untrusted_run_waits_for_approval_and_gets_no_secrets_no_cache_writes_and_only_an_untrusted_pool() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-untrusted", "success", "");
    let base = http_server(fixture.context.factory());
    define(&fixture, "app", "db");
    ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.store#GitBranchRule",
        &[("repositoryId", s("app")), ("refPattern", s("main"))],
    );
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.store#CiSecret"),
        &[
            ("repositoryId", s("app")),
            ("name", s("DEPLOY_KEY")),
            ("value", s("s3cr3t-deploy-key")),
        ],
    );
    let work = Work::new(&fixture, &base, WRITER, "app");
    let file = pipeline_file();
    let commit = work.push(
        &fixture,
        &[("ci.xml", &file), ("marker.txt", "here"), ("lock.txt", "v1")],
        "main",
    );
    // the trusted run of the push stores the cache of the protected branch
    let trusted_run = text(&only_run(&fixture, "app", &commit), "pipelineRunId");
    assert_eq!(start_and_run(&fixture, &base, &trusted_run, "test").1, 0);
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[("pipelineRunId", s(&trusted_run))],
    );

    // a merge request from a fork: its head is kept in the target (refs/merge-requests/1/head), and its run is the
    // target's, untrusted for where its commits come from though its author has every right in both
    ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.fork#GitRepository",
        &[("sourceRepositoryId", s("app")), ("repositoryId", s("fork"))],
    );
    let forker = Cloned::clone(&fixture, &base, "fork", WRITER);
    forker.ok(&["checkout", "-q", "-b", "feature"]);
    let fork_head = forker.commit(&[("feature.txt", "from a fork")], "Feature");
    forker.ok(&["push", "-q", "origin", "feature"]);
    settled(&fixture);
    // the push to the fork is the fork's own run, by its writer
    let fork_run = only_run(&fixture, "fork", &fork_head);
    assert_eq!(
        (text(&fork_run, "event"), text(&fork_run, "trusted")),
        ("push".into(), "Y".into())
    );
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[("pipelineRunId", s(&text(&fork_run, "pipelineRunId")))],
    );
    let opened = ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("app")),
            ("sourceRepositoryId", s("fork")),
            ("sourceRef", s("feature")),
            ("targetRef", s("main")),
            ("title", s("From a fork")),
        ],
    );
    let fork_request_id = to_plain_string(&opened["mergeRequestId"]);
    // queued by the rule on the merge request's event, in the target, at the fork's head
    let run = only_run(&fixture, "app", &fork_head);
    let run_id = text(&run, "pipelineRunId");
    assert_eq!(
        (
            text(&run, "event"),
            text(&run, "refName"),
            text(&run, "mergeRequestId"),
            text(&run, "sourceRepositoryId"),
            text(&run, "triggeredByUserId"),
        ),
        (
            "merge-request".into(),
            "refs/heads/main".into(),
            fork_request_id.clone(),
            "fork".into(),
            WRITER.into()
        )
    );
    assert_eq!(
        (
            text(&run, "trusted"),
            text(&run, "protectedRef"),
            text(&run, "approvalState"),
            text(&run, "statusId")
        ),
        ("N".into(), "N".into(), "waiting".into(), "waiting".into())
    );
    // the status says why nothing runs
    let waiting = statuses(&fixture, "app", &fork_head)["ci/check/test"].clone();
    assert_eq!(
        (waiting.0.as_str(), waiting.1.as_str()),
        ("pending", "Waiting for approval")
    );
    assert!(waiting.2.starts_with("https://ci.example.com/app/runs/"), "{waiting:?}");

    // a push for review by an account that may only view: its merge request's run, untrusted; the push itself is no
    // push pipeline's (refs/for/main is no branch), and the head ref of the merge request queues nothing
    let reader = Cloned::clone(&fixture, &base, "app", READER);
    let proposed = reader.commit(&[("proposed.txt", "for review")], "Proposed");
    reader.ok(&["push", "-q", "origin", "HEAD:refs/for/main"]);
    settled(&fixture);
    let review_run = only_run(&fixture, "app", &proposed);
    assert_eq!(
        (
            text(&review_run, "event"),
            text(&review_run, "refName"),
            text(&review_run, "trusted"),
            text(&review_run, "statusId"),
            text(&review_run, "triggeredByUserId"),
        ),
        (
            "merge-request".into(),
            "refs/heads/main".into(),
            "N".into(),
            "waiting".into(),
            READER.into()
        )
    );
    // the same push by an account with every right is untrusted too: a head that no branch of the repository holds
    let writer_too = Cloned::clone(&fixture, &base, "app", WRITER);
    let offered = writer_too.commit(&[("offered.txt", "for review")], "Offered");
    writer_too.ok(&["push", "-q", "origin", "HEAD:refs/for/main"]);
    settled(&fixture);
    let offered_run = only_run(&fixture, "app", &offered);
    assert_eq!(
        (text(&offered_run, "trusted"), text(&offered_run, "statusId")),
        ("N".into(), "waiting".into())
    );
    // a merge request that stopped following its fork stays untrusted when it is run again
    let (_, errors) = internal(
        &fixture,
        WRITER,
        "org.moqui.git.GitReviewServices.detach#MergeRequestSource",
        &[("mergeRequestId", s(&fork_request_id))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    {
        let request = merge_requests(&fixture, "app").remove(0);
        let detached = queue::merge_request_run(&request, Some(WRITER.to_string())).unwrap();
        assert!(detached.source_detached && detached.source_repository_id.as_deref() == Some("fork"));
        let _entered = fixture.context.enter();
        let origin = crate::ci::trust::Origin {
            repository_id: "app",
            user_id: Some(WRITER),
            source_detached: true,
            ..Default::default()
        };
        assert!(!crate::ci::trust::is_trusted(Event::MergeRequest, &origin));
    }
    let mut by_reader = RunRequest::new("app", Event::Manual, "refs/heads/main", &commit);
    by_reader.user_id = Some(READER.to_string());
    by_reader.pipeline_name = Some("check".to_string());
    let reader_run = queue_request(&fixture, &by_reader).remove(0);
    assert_eq!(reader_run.status, "waiting");
    let mut by_writer = by_reader.clone();
    by_writer.user_id = Some(WRITER.to_string());
    let writer_run = queue_request(&fixture, &by_writer).remove(0);
    assert_eq!(writer_run.status, "queued");
    for other in [&reader_run.pipeline_run_id, &writer_run.pipeline_run_id] {
        ok(
            &fixture,
            WRITER,
            &format!("{SERVICES}.cancel#PipelineRun"),
            &[("pipelineRunId", s(other))],
        );
    }

    // waiting: nothing starts
    assert_eq!(start(&fixture), engine::Started::default());
    assert!(FakeHosts::of(fixture.context.factory()).containers().is_empty());

    // approval needs the update right; then the run is queued, still untrusted
    let (_, errors) = call(
        &fixture,
        READER,
        &format!("{SERVICES}.approve#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    assert!(errors.contains("needs the update right"), "{errors}");
    let approved = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.approve#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    assert_eq!(to_plain_string(&approved["approvalState"]), "approved");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.approve#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    assert!(errors.contains("does not wait for approval"), "{errors}");
    let run = rows(&fixture, "moqui.resource.GitPipelineRun", &[("pipelineRunId", &run_id)]).remove(0);
    assert_eq!(
        (text(&run, "trusted"), text(&run, "approvedByUserId")),
        ("N".into(), WRITER.into())
    );

    // only the pool that takes untrusted runs
    assert_eq!(start(&fixture).started, 1);
    let job = job_named(&fixture, &run_id, "test");
    let job_run_id = text(&job, "jobRunId");
    assert_eq!(text(&job, "runnerPoolId"), "P_OUTSIDE");
    let (_, job_token) = container_of(&fixture, &job_run_id);

    // no secrets, though the job asks for one and the target branch is protected
    let transport = Local {
        fixture: &fixture,
        base: base.clone(),
        job_run_id: job_run_id.clone(),
        token: job_token.clone(),
    };
    let got = json_of(&transport.request("GET", "job", &[]));
    assert_eq!(got["trusted"], false);
    assert_eq!(got["secrets"], serde_json::json!({}));
    // it reads the target branch's cache and writes none
    let scratch = fixture.dir.join("cache-probe.tar.gz");
    let key = runner::expand_key("deps-${hash:lock.txt}", &work.dir);
    assert_eq!(transport.download(&format!("caches?key={key}"), &scratch).unwrap(), 200);
    std::fs::write(&scratch, b"poison").unwrap();
    let (status, answer) = transport
        .upload("artifacts?kind=cache&key=poisoned&name=poisoned", &scratch)
        .unwrap();
    assert_eq!(status, 422, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("writes none"), "{answer}");
    let (code, _) = run_job(&fixture, &base, &job_run_id, &job_token);
    assert_eq!(code, 0);
    let log = log_of(&fixture, WRITER, &job_run_id);
    assert!(log.contains("key=[] token=[]") && log.contains("cache hit"), "{log}");
    assert_eq!(
        rows(
            &fixture,
            "moqui.resource.GitRunArtifact",
            &[("repositoryId", "app"), ("artifactKind", "cache")]
        )
        .len(),
        1
    );

    // with no pool that takes untrusted runs, the next job is an error and never reaches the trusted pool
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.store#RunnerPool"),
        &[("runnerPoolId", s("P_OUTSIDE")), ("enabled", Value::Bool(false))],
    );
    assert_eq!(start(&fixture), engine::Started { started: 0, refused: 1 });
    let after = job_named(&fixture, &run_id, "after");
    assert_eq!(text(&after, "statusId"), "error");
    assert!(
        text(&after, "statusMessage").contains("takes untrusted runs"),
        "{}",
        text(&after, "statusMessage")
    );
    assert!(FakeHosts::of(fixture.context.factory()).containers().is_empty());

    // deleting the fork takes the fork's runs with it; its merge request's run is the target's, and stays
    let delete = "org.moqui.git.GitServices.delete#GitRepository";
    ok(&fixture, WRITER, delete, &[("repositoryId", s("fork"))]);
    for entity in ["moqui.resource.GitPipelineRun", "moqui.resource.GitJobRun"] {
        assert!(
            rows(&fixture, entity, &[("repositoryId", "fork")]).is_empty(),
            "{entity}"
        );
    }
    assert_eq!(
        rows(&fixture, "moqui.resource.GitPipelineRun", &[("pipelineRunId", &run_id)]).len(),
        1
    );
    // deleting a repository takes what the engine kept of it, with no call of the application's
    assert!(!rows(&fixture, "moqui.resource.GitCiSecret", &[("repositoryId", "app")]).is_empty());
    ok(&fixture, WRITER, delete, &[("repositoryId", s("app"))]);
    for entity in [
        "moqui.resource.GitPipelineRun",
        "moqui.resource.GitJobRun",
        "moqui.resource.GitRunArtifact",
        "moqui.resource.GitRunnerUsage",
        "moqui.resource.GitCiSecret",
        "moqui.resource.GitCommitStatus",
    ] {
        assert!(
            rows(&fixture, entity, &[("repositoryId", "app")]).is_empty(),
            "{entity}"
        );
    }
    for entity in [
        "moqui.resource.GitJobStep",
        "moqui.resource.GitRunLogChunk",
        "moqui.resource.GitRunTestFailure",
    ] {
        assert!(rows(&fixture, entity, &[]).is_empty(), "{entity}");
    }
}

const CHECK_SERVICE: &str = r#"<services>
    <service verb="check" noun="Run">
        <implements service="org.moqui.git.GitCiServices.check#PipelineRun"/>
        <actions><script><![CDATA[
            ownerId = 'GROUP_A'
            if (event === 'manual') { decision = 'wait'; message = 'Runner minutes are used up' }
            else if (mergeRequestId === 'MR-REFUSED') ec.message.addError('quotaExceeded: no runner minutes left')
            else if (mergeRequestId === 'MR-ALLOWED') decision = 'allow'
            else if (mergeRequestId === 'MR-LOWERED') trusted = false
        ]]></script></actions>
    </service>
</services>"#;

#[test]
fn the_applications_check_refuses_holds_allows_and_lowers_a_run() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-check", "success", r#"run-check-service="test.CiCheck.check#Run""#);
    let base = http_server(fixture.context.factory());
    {
        let _entered = fixture.context.enter();
        std::fs::create_dir_all(fixture.dir.join("service/test")).unwrap();
        let stub = fixture.dir.join("service/test/CiCheck.xml");
        std::fs::write(&stub, CHECK_SERVICE).unwrap();
        let framework = moqui_core::context::framework_dir();
        let failures = moqui_core::service::lint(&[
            framework.join("service/org/moqui/git/GitServices.xml"),
            framework.join("service/org/moqui/git/GitCiServices.xml"),
            framework.join("service/org/moqui/dev/InstanceServices.xml"),
            stub,
        ])
        .unwrap();
        assert!(failures.is_empty(), "{failures:?}");
    }
    let (_work, commit) = pushed(&fixture, &base, "app", &[]);
    // the push: no decision, so the engine's own (trusted: queued), under the owner the check named
    let run = only_run(&fixture, "app", &commit);
    assert_eq!(
        (text(&run, "statusId"), text(&run, "ownerId")),
        ("queued".into(), "GROUP_A".into())
    );
    // refused: an error run whose statuses say why
    let mut request = outside_request("app", &commit);
    request.merge_request_id = Some("MR-REFUSED".into());
    let refused = queue_request(&fixture, &request).remove(0);
    assert_eq!(refused.status, "error");
    let job = job_named(&fixture, &refused.pipeline_run_id, "test");
    assert_eq!(
        (text(&job, "statusId"), text(&job, "statusMessage")),
        ("error".into(), "quotaExceeded: no runner minutes left".into())
    );
    assert_eq!(
        statuses(&fixture, "app", &commit)["ci/check/test"].1,
        "quotaExceeded: no runner minutes left"
    );
    // allowed: an untrusted run starts with no approval, still untrusted
    request.merge_request_id = Some("MR-ALLOWED".into());
    let allowed = queue_request(&fixture, &request).remove(0);
    assert_eq!(allowed.status, "queued");
    let run = rows(
        &fixture,
        "moqui.resource.GitPipelineRun",
        &[("pipelineRunId", &allowed.pipeline_run_id)],
    )
    .remove(0);
    assert_eq!(
        (text(&run, "trusted"), text(&run, "approvalState")),
        ("N".into(), "none".into())
    );
    // held: a trusted run waits, with the check's words
    let mut manual = RunRequest::new("app", Event::Manual, "refs/heads/main", &commit);
    manual.user_id = Some(WRITER.into());
    manual.pipeline_name = Some("check".into());
    let held = queue_request(&fixture, &manual).remove(0);
    assert_eq!(held.status, "waiting");
    let run = rows(
        &fixture,
        "moqui.resource.GitPipelineRun",
        &[("pipelineRunId", &held.pipeline_run_id)],
    )
    .remove(0);
    assert_eq!(
        (text(&run, "trusted"), text(&run, "statusMessage")),
        ("Y".into(), "Runner minutes are used up".into())
    );
    // lowered: the check may take trust away, never give it
    let mut lowered = RunRequest::new("app", Event::MergeRequest, "refs/heads/main", &commit);
    lowered.user_id = Some(WRITER.into());
    lowered.merge_request_id = Some("MR-LOWERED".into());
    let lowered = queue_request(&fixture, &lowered).remove(0);
    let run = rows(
        &fixture,
        "moqui.resource.GitPipelineRun",
        &[("pipelineRunId", &lowered.pipeline_run_id)],
    )
    .remove(0);
    assert_eq!(
        (text(&run, "trusted"), lowered.status.as_str()),
        ("N".into(), "waiting")
    );
    // the owner's usage is the application's to read
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.get#RunnerUsage"),
        &[("ownerId", s("GROUP_A"))],
    );
    assert!(errors.contains("authorization disabled"), "{errors}");
}

fn runs_of(fixture: &Fixture, merge_request_id: &str) -> Vec<EntityValue> {
    let mut runs = rows(
        fixture,
        "moqui.resource.GitPipelineRun",
        &[("mergeRequestId", merge_request_id)],
    );
    runs.sort_by_key(|run| text(run, "runNum").parse::<i64>().unwrap_or(0));
    runs
}

#[test]
fn a_merge_request_runs_its_pipelines_by_the_rule_and_a_merge_runs_the_push_pipelines_once() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-merge-request", "success", "");
    let base = http_server(fixture.context.factory());
    let (work, _main) = pushed(&fixture, &base, "app", &[]);
    let head = work.push(&fixture, &[("feature.txt", "new")], "feature");
    let merge_request = ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitReviewServices.create#MergeRequest",
        &[
            ("repositoryId", s("app")),
            ("sourceRef", s("feature")),
            ("targetRef", s("main")),
            ("title", s("A feature")),
        ],
    );
    let merge_request_id = to_plain_string(&merge_request["mergeRequestId"]);
    // the service ECA rule on queue#MergeRequestEvent (GitCi.secas.xml) queued it: nothing here called the engine
    let runs = runs_of(&fixture, &merge_request_id);
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    // at the merge request's head, for its target branch; the repository's own branch by a writer: trusted, and
    // never given a protected secret
    assert_eq!(
        (
            text(run, "event"),
            text(run, "refName"),
            text(run, "commitId"),
            text(run, "trusted"),
            text(run, "protectedRef"),
            text(run, "hidden"),
            text(run, "statusId"),
        ),
        (
            "merge-request".into(),
            "refs/heads/main".into(),
            head.clone(),
            "Y".into(),
            "N".into(),
            "N".into(),
            "queued".into()
        )
    );
    assert_eq!(statuses(&fixture, "app", &head)["ci/check/test"].0, "pending");

    // the engine's own services are the rules': a user names no user, commit or merge request to run for
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.queue#MergeRequestRuns"),
        &[("mergeRequestId", s(&merge_request_id)), ("action", s("synchronize"))],
    );
    assert!(errors.contains("queue#MergeRequestEvent only"), "{errors}");
    let (_, errors) = call(
        &fixture,
        READER,
        &format!("{SERVICES}.queue#PushRuns"),
        &[("repositoryId", s("app")), ("userId", s(WRITER))],
    );
    assert!(errors.contains("gitECA rule only"), "{errors}");
    // an action that moves no head queues nothing, and an event told twice queues once
    let event = "org.moqui.git.GitReviewServices.queue#MergeRequestEvent";
    for action in ["reviewed", "opened", "synchronize"] {
        ok(
            &fixture,
            WRITER,
            event,
            &[("mergeRequestId", s(&merge_request_id)), ("action", s(action))],
        );
    }
    assert_eq!(runs_of(&fixture, &merge_request_id).len(), 1);
    // once the run has ended: opened, told again, is still the head that had its run; a refused event (a caller
    // without the update right) queues nothing; a new event by who may update is a new run
    let run_id = text(run, "pipelineRunId");
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[("pipelineRunId", s(&run_id))],
    );
    ok(
        &fixture,
        WRITER,
        event,
        &[("mergeRequestId", s(&merge_request_id)), ("action", s("opened"))],
    );
    let (_, errors) = call(
        &fixture,
        READER,
        event,
        &[("mergeRequestId", s(&merge_request_id)), ("action", s("synchronize"))],
    );
    assert!(errors.contains("may not update"), "{errors}");
    assert_eq!(runs_of(&fixture, &merge_request_id).len(), 1);
    ok(
        &fixture,
        WRITER,
        event,
        &[("mergeRequestId", s(&merge_request_id)), ("action", s("synchronize"))],
    );
    let runs = runs_of(&fixture, &merge_request_id);
    assert_eq!(runs.len(), 2);
    assert_eq!(text(&runs[1], "commitId"), head);

    // merged through the service: the target branch moved by a write, so its push pipelines run for it, once, as
    // the user who merged; the merge request's own head ref and its merged event queue nothing
    ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitReviewServices.merge#MergeRequest",
        &[("mergeRequestId", s(&merge_request_id))],
    );
    for update in settled(&fixture) {
        assert_ne!(text(&update, "ecaStatus"), "failed", "{}", text(&update, "ecaError"));
    }
    let merged = super::tests::head_of(&fixture, "app", "refs/heads/main").unwrap();
    let all = rows(&fixture, "moqui.resource.GitPipelineRun", &[("repositoryId", "app")]);
    let of_merge: Vec<&EntityValue> = all
        .iter()
        .filter(|run| text(run, "commitId") == merged && text(run, "refName") == "refs/heads/main")
        .filter(|run| text(run, "event") == "push")
        .collect();
    assert_eq!(of_merge.len(), 1, "one push run for the merge");
    assert_eq!(
        (
            text(of_merge[0], "trusted"),
            text(of_merge[0], "triggeredByUserId"),
            text(of_merge[0], "mergeRequestId")
        ),
        ("Y".into(), WRITER.into(), String::new())
    );
    assert!(
        all.iter().all(|run| text(run, "refName").starts_with("refs/heads/")),
        "a run is for a branch or a tag"
    );
    assert_eq!(runs_of(&fixture, &merge_request_id).len(), 2);
}

#[test]
fn a_hidden_merge_requests_run_is_hidden_and_only_its_job_token_fetches_its_head() {
    if !git_available() {
        return;
    }
    // a server that holds what is pushed for review until the application has looked (review-start-hidden)
    let fixture = ci_fixture(
        "ci-hidden",
        "success",
        r#"/><git-server review-start-hidden="true" reserved-status-contexts="ci/""#,
    );
    let base = http_server(fixture.context.factory());
    let (_work, main) = pushed(&fixture, &base, "app", &[]);
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.cancel#PipelineRun"),
        &[(
            "pipelineRunId",
            s(&text(&only_run(&fixture, "app", &main), "pipelineRunId")),
        )],
    );
    let reader = Cloned::clone(&fixture, &base, "app", READER);
    let first = reader.commit(&[("held.txt", "one")], "Held");
    reader.ok(&["push", "-q", "origin", "HEAD:refs/for/main"]);
    settled(&fixture);
    let request = merge_requests(&fixture, "app").remove(0);
    assert!(!text(&request, "hiddenDate").is_empty(), "it starts hidden");
    let merge_request_id = text(&request, "mergeRequestId");

    // the event of a hidden merge request sends no webhook and its rule still runs: one run, at the hidden head,
    // hidden with it, and no push pipeline for refs/for/main
    let run = only_run(&fixture, "app", &first);
    let run_id = text(&run, "pipelineRunId");
    assert_eq!(
        (
            text(&run, "event"),
            text(&run, "mergeRequestId"),
            text(&run, "hidden"),
            text(&run, "trusted"),
            text(&run, "statusId"),
            text(&run, "triggeredByUserId"),
        ),
        (
            "merge-request".into(),
            merge_request_id.clone(),
            "Y".into(),
            "N".into(),
            "waiting".into(),
            READER.into()
        )
    );
    // the engine's status is at the hidden head, where a user writes none and reads none
    assert_eq!(statuses(&fixture, "app", &first)["ci/check/test"].0, "pending");
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.create#GitCommitStatus",
        &[
            ("repositoryId", s("app")),
            ("commitId", s(&first)),
            ("state", s("success")),
            ("context", s("mine")),
        ],
    );
    assert!(errors.contains("no commit"), "{errors}");
    // and where the application keeps the ci/ contexts for the system, the engine's statuses are still written
    // (through create#GitCommitStatus, as the system) and a user's are refused, at a commit anyone reads too
    let (_, errors) = call(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.create#GitCommitStatus",
        &[
            ("repositoryId", s("app")),
            ("commitId", s(&main)),
            ("state", s("success")),
            ("context", s("ci/check/test")),
        ],
    );
    assert!(errors.contains("GitReservedContext"), "{errors}");
    assert_eq!(
        statuses(&fixture, "app", &main)["ci/check/test"].0,
        "error",
        "cancelled"
    );

    // the run is read by the user it is for and by the application; not by another user, whatever their rights
    let get = format!("{SERVICES}.get#PipelineRun");
    let hidden_too = [("pipelineRunId", s(&run_id)), ("includeHidden", Value::Bool(true))];
    ok(&fixture, READER, &get, &[("pipelineRunId", s(&run_id))]);
    let (_, errors) = call(&fixture, WRITER, &get, &[("pipelineRunId", s(&run_id))]);
    assert!(errors.contains("no such pipeline run"), "{errors}");
    let (_, errors) = call(&fixture, WRITER, &get, &hidden_too);
    assert!(
        errors.contains("no such pipeline run"),
        "includeHidden is the application's: {errors}"
    );
    let (got, errors) = internal(&fixture, WRITER, &get, &hidden_too);
    assert!(errors.trim().is_empty() && got.contains_key("run"), "{errors}");
    let job_run_id = text(&job_named(&fixture, &run_id, "test"), "jobRunId");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.get#JobLog"),
        &[("jobRunId", s(&job_run_id))],
    );
    assert!(errors.contains("no such job run"), "{errors}");
    let listed = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.find#PipelineRuns"),
        &[("repositoryId", s("app"))],
    );
    let Value::List(listed) = &listed["runList"] else {
        panic!("runList")
    };
    assert!(
        !listed
            .iter()
            .any(|run| matches!(run, Value::Map(run) if to_plain_string(&run["pipelineRunId"]) == run_id)),
        "a hidden run is not listed for another user"
    );

    // approved by the application, which sees it; a user with the update right does not
    let approve = format!("{SERVICES}.approve#PipelineRun");
    let (_, errors) = call(&fixture, WRITER, &approve, &[("pipelineRunId", s(&run_id))]);
    assert!(errors.contains("no such pipeline run"), "{errors}");
    let (_, errors) = internal(&fixture, WRITER, &approve, &hidden_too);
    assert!(errors.trim().is_empty(), "{errors}");
    assert_eq!(start(&fixture).started, 1);
    let (_, job_token) = container_of(&fixture, &job_run_id);

    // the job's token fetches the hidden head, from the repository the merge request is in; nobody else fetches it,
    // by its ref or by its id
    let url = format!("{base}/git/app.git");
    let checkout = |name: &str, commit: &str| {
        let dir = fixture.dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        runner::http::checkout(&url, commit, 1, &job_token, &dir).map(|_| dir)
    };
    let dir = checkout("hidden-own", &first).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("held.txt")).unwrap(), "one");
    let writer = Cloned::clone(&fixture, &base, "app", WRITER);
    for wanted in [first.as_str(), "refs/merge-requests/1/head"] {
        let (fetched, out) = writer.run(&["fetch", "origin", wanted]);
        assert!(!fetched, "{wanted}: {out}");
    }

    // a new head replaces the first (an amended commit): the first is no ref's any more, so its job's token no
    // longer fetches it, and the new head has its own hidden run
    reader.ok(&["commit", "-q", "--amend", "-m", "Held, said better"]);
    let second = reader.head();
    reader.ok(&["push", "-q", "origin", "HEAD:refs/for/main"]);
    settled(&fixture);
    assert_eq!(
        super::tests::head_of(&fixture, "app", "refs/merge-requests/1/head").as_deref(),
        Some(second.as_str())
    );
    let err = checkout("hidden-left", &first).unwrap_err();
    assert!(err.contains("no longer its repository's to give"), "{err}");
    let next = only_run(&fixture, "app", &second);
    assert_eq!(text(&next, "hidden"), "Y");

    // shown by the application: its runs are shown with it, and no second run is queued for the head, by the
    // showing or by an opened event told again
    let hide = "org.moqui.git.GitReviewServices.set#MergeRequestHidden";
    let (_, errors) = internal(
        &fixture,
        WRITER,
        hide,
        &[("mergeRequestId", s(&merge_request_id)), ("hidden", s("false"))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    let (_, errors) = internal(
        &fixture,
        WRITER,
        "org.moqui.git.GitReviewServices.queue#MergeRequestEvent",
        &[("mergeRequestId", s(&merge_request_id)), ("action", s("opened"))],
    );
    assert!(errors.trim().is_empty(), "{errors}");
    let runs = runs_of(&fixture, &merge_request_id);
    assert_eq!(runs.len(), 2, "one run for each head");
    assert!(runs.iter().all(|run| text(run, "hidden") == "N"));
    ok(&fixture, WRITER, &get, &[("pipelineRunId", s(&run_id))]);
    // and hidden again with it
    let (_, errors) = internal(&fixture, WRITER, hide, &[("mergeRequestId", s(&merge_request_id))]);
    assert!(errors.trim().is_empty(), "{errors}");
    assert!(runs_of(&fixture, &merge_request_id)
        .iter()
        .all(|run| text(run, "hidden") == "Y"));
    let (_, errors) = call(&fixture, WRITER, &get, &[("pipelineRunId", s(&run_id))]);
    assert!(errors.contains("no such pipeline run"), "{errors}");

    // the repository is deleted with a job still running: its container is removed and its token is dead
    assert_eq!(FakeHosts::of(fixture.context.factory()).containers().len(), 1);
    ok(
        &fixture,
        WRITER,
        "org.moqui.git.GitServices.delete#GitRepository",
        &[("repositoryId", s("app"))],
    );
    assert!(FakeHosts::of(fixture.context.factory()).containers().is_empty());
    assert!(rows(&fixture, "moqui.resource.GitPipelineRun", &[("repositoryId", "app")]).is_empty());
    assert!(rows(&fixture, "moqui.resource.GitJobRun", &[]).is_empty());
    let bearer = format!("Bearer {job_token}");
    assert_eq!(
        git_status(&fixture, &git_request("app", GitService::UploadPack, true, &bearer)).0,
        401
    );
}

fn git_request(repository: &str, service: GitService, advertise: bool, authorization: &str) -> GitHttpRequest {
    GitHttpRequest {
        repository: repository.to_string(),
        service,
        advertise,
        git_protocol: None,
        gzip: false,
        authorization: Some(authorization.to_string()),
        api_key: None,
        secure: true,
        client_ip: "127.0.0.1".into(),
    }
}

fn git_status(fixture: &Fixture, request: &GitHttpRequest) -> (u16, String) {
    let _entered = fixture.context.enter();
    let mut response = MemoryResponse::default();
    let mut body: &[u8] = &[];
    super::serve_http(request, &mut body, &mut response);
    (response.status, String::from_utf8_lossy(&response.body).into_owned())
}

#[test]
fn a_job_token_reads_its_commit_of_its_repository_and_nothing_else() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-token", "success", "");
    let base = http_server(fixture.context.factory());
    let (work, first) = pushed(&fixture, &base, "app", &[]);
    let second = work.push(&fixture, &[("marker.txt", "second")], "main");
    define(&fixture, "other", "db");
    let other = Work::new(&fixture, &base, WRITER, "other");
    other.push(&fixture, &[("readme.txt", "another repository")], "main");

    // two running jobs: of the first commit and of the second
    assert_eq!(start(&fixture).started, 2);
    let first_job = text(
        &job_named(
            &fixture,
            &text(&only_run(&fixture, "app", &first), "pipelineRunId"),
            "test",
        ),
        "jobRunId",
    );
    let second_job = text(
        &job_named(
            &fixture,
            &text(&only_run(&fixture, "app", &second), "pipelineRunId"),
            "test",
        ),
        "jobRunId",
    );
    let (_, first_token) = container_of(&fixture, &first_job);
    let (_, second_token) = container_of(&fixture, &second_job);
    let url = format!("{base}/git/app.git");
    let checkout = |name: &str, url: &str, commit: &str, depth: u64, job_token: &str| {
        let dir = fixture.dir.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        runner::http::checkout(url, commit, depth, job_token, &dir).map(|_| dir)
    };

    // its own commit, shallow: the files of that commit and one commit of history
    let dir = checkout("own", &url, &first, 1, &first_token).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("marker.txt")).unwrap(), "here");
    let home = fixture.dir.join("home-token");
    std::fs::create_dir_all(&home).unwrap();
    assert_eq!(git_ok(&dir, &home, &["rev-list", "--count", "HEAD"]).trim(), "1");
    assert_eq!(git_ok(&dir, &home, &["rev-parse", "HEAD"]).trim(), first);
    // the one ref it is shown is its commit: no branch, no tag, no other head
    let listed = {
        let (ok, out) = git_in(
            &dir,
            &home,
            &[
                "-c",
                &format!("http.extraHeader=Authorization: Bearer {first_token}"),
                "ls-remote",
                &url,
            ],
        );
        assert!(ok, "{out}");
        out
    };
    assert!(listed.contains(&first) && !listed.contains(&second), "{listed}");
    assert!(!listed.contains("refs/heads/main"), "{listed}");

    // another commit of the same repository, a deep fetch, another repository, a push: refused
    let err = checkout("newer", &url, &second, 1, &first_token).unwrap_err();
    assert!(
        err.contains("its job's commit and nothing else") || err.contains("not our ref"),
        "{err}"
    );
    let err = checkout("deep", &url, &first, 5000, &first_token).unwrap_err();
    assert!(err.contains("shallow"), "{err}");
    assert!(checkout("other", &format!("{base}/git/other.git"), &first, 1, &first_token).is_err());
    let bearer = format!("Bearer {first_token}");
    let (status, body) = git_status(&fixture, &git_request("other", GitService::UploadPack, true, &bearer));
    assert_eq!((status, body.trim()), (403, "the job token is for another repository"));
    let (status, body) = git_status(&fixture, &git_request("app", GitService::ReceivePack, true, &bearer));
    assert_eq!((status, body.trim()), (403, "a job token does not push"));
    let (pushed_ok, out) = git_in(
        &dir,
        &home,
        &[
            "-c",
            &format!("http.extraHeader=Authorization: Bearer {first_token}"),
            "push",
            &url,
            "HEAD:refs/heads/stolen",
        ],
    );
    assert!(!pushed_ok, "{out}");
    assert!(super::tests::head_of(&fixture, "app", "refs/heads/stolen").is_none());
    // the token is no user's: it is not an access token, and over plain HTTP it is refused
    let mut plain = git_request("app", GitService::UploadPack, true, &bearer);
    plain.secure = false;
    assert_eq!(git_status(&fixture, &plain).0, 403);
    // as git sends it, the password of Basic
    let basic = format!(
        "Basic {}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("ci:{first_token}"))
    );
    assert_eq!(
        git_status(&fixture, &git_request("app", GitService::UploadPack, true, &basic)).0,
        200
    );
    assert_eq!(
        git_status(
            &fixture,
            &git_request("app", GitService::UploadPack, true, "Bearer mqcj_0000")
        )
        .0,
        401
    );

    // the runner endpoints of another job are not this token's
    let wrong = Local {
        fixture: &fixture,
        base: base.clone(),
        job_run_id: second_job.clone(),
        token: first_token.clone(),
    };
    for (method, path) in [
        ("GET", "job"),
        ("POST", "log"),
        ("POST", "result"),
        ("PUT", "artifacts?kind=artifact&name=x"),
    ] {
        let response = wrong.request(method, path, b"{}");
        assert_eq!(response.status, 403, "{method} {path}");
    }
    assert_eq!(
        text(
            &job_named(
                &fixture,
                &text(&only_run(&fixture, "app", &second), "pipelineRunId"),
                "test"
            ),
            "statusId"
        ),
        "running"
    );
    // no token, an access token, plain HTTP, an endpoint that is not one
    let right = Local {
        fixture: &fixture,
        base: base.clone(),
        job_run_id: first_job.clone(),
        token: first_token.clone(),
    };
    assert_eq!(right.request("GET", "job", &[]).status, 200);
    assert_eq!(right.request("GET", "nothing", &[]).status, 404);
    assert_eq!(
        right.request("POST", "steps/99", br#"{"status":"running"}"#).status,
        404
    );
    assert_eq!(right.request("POST", "result", br#"{"status":"winning"}"#).status, 400);
    assert_eq!(
        right.request("PUT", "artifacts?kind=cache&name=k", b"x").status,
        422,
        "a cache needs a key"
    );
    assert_eq!(
        right
            .request("PUT", "artifacts?kind=artifact&name=..%2Fup", b"x")
            .status,
        400
    );
    {
        let _entered = fixture.context.enter();
        let mut response = MemoryResponse::default();
        let mut body: &[u8] = &[];
        let mut request = RunnerHttpRequest {
            method: "GET".into(),
            path: format!("jobs/{first_job}/job"),
            query: String::new(),
            authorization: None,
            content_length: None,
            secure: true,
            client_ip: "127.0.0.1".into(),
        };
        runner_http::serve(&request, &mut body, &mut response);
        assert_eq!(response.status, 401);
        request.authorization = Some(format!("Bearer {first_token}"));
        request.secure = false;
        let mut response = MemoryResponse::default();
        runner_http::serve(&request, &mut body, &mut response);
        assert_eq!(response.status, 403);
    }
    // a log batch over the size limit is refused whole
    let big = serde_json::json!({ "text": "x".repeat(1_100_000) }).to_string();
    assert_eq!(right.request("POST", "log", big.as_bytes()).status, 413);

    // the result ends the job, and the token with it: for the runner REST and for git
    assert_eq!(
        right
            .request("POST", "result", br#"{"status":"success","exitCode":0}"#)
            .status,
        200
    );
    assert_eq!(right.request("GET", "job", &[]).status, 401);
    assert!(checkout("after-end", &url, &first, 1, &first_token).is_err());
    // the other job's token still works
    assert!(checkout("still", &url, &second, 1, &second_token).is_ok());

    // a branch deleted while its job runs: in a repository that is in no fork network and has had no hidden merge
    // request every commit was a reader's, so the job still fetches its commit (a hidden head that was replaced is
    // refused: a_hidden_merge_requests_run_is_hidden_and_only_its_job_token_fetches_its_head)
    for commit in [&first, &second] {
        ok(
            &fixture,
            WRITER,
            &format!("{SERVICES}.cancel#PipelineRun"),
            &[(
                "pipelineRunId",
                s(&text(&only_run(&fixture, "app", commit), "pipelineRunId")),
            )],
        );
    }
    let side = work.push(&fixture, &[("marker.txt", "side")], "side");
    assert_eq!(start(&fixture).started, 1);
    let side_job = text(
        &job_named(
            &fixture,
            &text(&only_run(&fixture, "app", &side), "pipelineRunId"),
            "test",
        ),
        "jobRunId",
    );
    let (_, side_token) = container_of(&fixture, &side_job);
    git_ok(&work.dir, &work.home, &["push", "-q", "origin", ":side"]);
    settled(&fixture);
    assert!(super::tests::head_of(&fixture, "app", "refs/heads/side").is_none());
    assert!(checkout("left", &url, &side, 1, &side_token).is_ok());
}

#[test]
fn jobs_past_their_time_and_lost_containers_are_ended_and_cleaned_up() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-overdue", "hang", "");
    let base = http_server(fixture.context.factory());
    let (work, commit) = pushed(&fixture, &base, "app", &[]);
    let run_id = text(&only_run(&fixture, "app", &commit), "pipelineRunId");
    assert_eq!(start(&fixture).started, 1);
    let job_run_id = text(&job_named(&fixture, &run_id, "test"), "jobRunId");
    let hosts = FakeHosts::of(fixture.context.factory());
    assert_eq!(hosts.containers().len(), 1);
    let end = |now: i64| {
        let _entered = fixture.context.enter();
        engine::end_overdue(&fixture.entity, now).unwrap()
    };
    let now = crate::ci::now_millis();
    // in time, its runner silent but its container there: left alone
    assert_eq!(end(now + 5_000), engine::Ended::default());
    assert_eq!(text(&job_named(&fixture, &run_id, "test"), "statusId"), "running");
    // past the job's 20 minutes: an error, the container removed, the token dead, the next job skipped
    let ended = end(now + 21 * 60_000);
    assert_eq!((ended.timed_out, ended.lost), (1, 0));
    let job = job_named(&fixture, &run_id, "test");
    assert_eq!(
        (
            text(&job, "statusId"),
            text(&job, "statusMessage"),
            text(&job, "tokenHash")
        ),
        ("error".into(), "Timed out after 20 minutes".into(), String::new())
    );
    assert!(hosts.containers().is_empty());
    assert_eq!(statuses(&fixture, "app", &commit)["ci/check/test"], {
        let mut expected = (
            "error".to_string(),
            "Timed out after 20 minutes".to_string(),
            String::new(),
        );
        expected.2 = format!("https://ci.example.com/app/runs/1/jobs/{job_run_id}");
        expected
    });
    assert_eq!(text(&only_run(&fixture, "app", &commit), "statusId"), "error");

    // a container the host no longer has: the job is lost
    set(
        &fixture,
        "moqui.server.instance.InstanceHost",
        "instanceHostId",
        "FAKE",
        &[("hostAddress", s("lost"))],
    );
    let second = work.push(&fixture, &[("marker.txt", "two")], "main");
    let second_run = text(&only_run(&fixture, "app", &second), "pipelineRunId");
    assert_eq!(start(&fixture).started, 1);
    let ended = end(crate::ci::now_millis() + 5_000);
    assert_eq!((ended.timed_out, ended.lost), (0, 1));
    assert_eq!(
        text(&job_named(&fixture, &second_run, "test"), "statusMessage"),
        "The runner's container was lost"
    );
    // one that exited with no result
    set(
        &fixture,
        "moqui.server.instance.InstanceHost",
        "instanceHostId",
        "FAKE",
        &[("hostAddress", s("exit"))],
    );
    let third = work.push(&fixture, &[("marker.txt", "three")], "main");
    let third_run = text(&only_run(&fixture, "app", &third), "pipelineRunId");
    start(&fixture);
    assert_eq!(end(crate::ci::now_millis() + 5_000).lost, 1);
    assert_eq!(
        text(&job_named(&fixture, &third_run, "test"), "statusMessage"),
        "The runner exited (code 137) without a result"
    );

    // what a job run left on the host with no running job behind it is removed by its label; a running job's stays
    set(
        &fixture,
        "moqui.server.instance.InstanceHost",
        "instanceHostId",
        "FAKE",
        &[("hostAddress", s("success"))],
    );
    let fourth = work.push(&fixture, &[("marker.txt", "four")], "main");
    let fourth_run = text(&only_run(&fixture, "app", &fourth), "pipelineRunId");
    start(&fixture);
    let kept = text(&job_named(&fixture, &fourth_run, "test"), "jobRunId");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let labels = |job: &str| {
            let mut map = moqui_lib::object::ValueMap::default();
            map.insert("moqui.ci.job".to_string(), s(job));
            map.insert("moqui.ci.server".to_string(), s(&crate::ci::driver::server_id()));
            Value::Map(map)
        };
        let parameters: BTreeMap<String, Value> = [
            ("instanceHostId".to_string(), s("FAKE")),
            ("name".to_string(), s("moqui-ci-leftover")),
            ("image".to_string(), s("moqui-build:1")),
            ("labels".to_string(), labels("GONE-JOB")),
        ]
        .into_iter()
        .collect();
        moqui_core::service::sync("org.moqui.dev.InstanceServices.start#RunFake", &parameters).unwrap();
    }
    assert_eq!(hosts.containers().len(), 2);
    assert_eq!(end(crate::ci::now_millis()).removed, 1);
    let left = hosts.containers();
    assert_eq!(left.len(), 1);
    assert_eq!(left.values().next().unwrap().labels["moqui.ci.job"], kept);

    // a host that starts nothing: the job is an error with the host's words
    set(
        &fixture,
        "moqui.server.instance.InstanceHost",
        "instanceHostId",
        "FAKE",
        &[("hostAddress", s("fail"))],
    );
    let fifth = work.push(&fixture, &[("marker.txt", "five")], "main");
    let fifth_run = text(&only_run(&fixture, "app", &fifth), "pipelineRunId");
    assert_eq!(start(&fixture), engine::Started { started: 0, refused: 1 });
    let failed = job_named(&fixture, &fifth_run, "test");
    assert_eq!(text(&failed, "statusId"), "error");
    assert!(
        text(&failed, "statusMessage").starts_with("the runner did not start:"),
        "{}",
        text(&failed, "statusMessage")
    );

    // an image the pool does not allow, and a pool nobody has
    let custom = r#"<ci><pipeline name="p" on="push">
        <job name="bad-image" image="docker.io/evil/miner" pool="linux"><step run="x"/></job>
        <job name="no-pool" image="moqui-build" pool="windows"><step run="x"/></job>
    </pipeline></ci>"#;
    let sixth = work.push(&fixture, &[("ci.xml", custom)], "main");
    let sixth_run = text(&only_run(&fixture, "app", &sixth), "pipelineRunId");
    assert_eq!(start(&fixture), engine::Started { started: 0, refused: 2 });
    assert_eq!(
        text(&job_named(&fixture, &sixth_run, "bad-image"), "statusMessage"),
        "runner pool linux does not allow the image docker.io/evil/miner"
    );
    assert_eq!(
        text(&job_named(&fixture, &sixth_run, "no-pool"), "statusMessage"),
        "no enabled runner pool named windows takes trusted runs"
    );
}

#[test]
fn a_file_that_is_not_valid_is_an_error_status_and_a_component_gets_the_built_in_pipeline() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture(
        "ci-definition",
        "success",
        r#"builtin-image="moqui-build:{frameworkVersion}""#,
    );
    let base = http_server(fixture.context.factory());
    define(&fixture, "broken", "db");
    let work = Work::new(&fixture, &base, WRITER, "broken");
    let commit = work.push(
        &fixture,
        &[(
            "ci.xml",
            "<ci>\n<pipeline name=\"p\" on=\"push\">\n<job name=\"j\" image=\"i\"/>\n</pipeline></ci>",
        )],
        "main",
    );
    let run = only_run(&fixture, "broken", &commit);
    assert_eq!(
        (text(&run, "statusId"), text(&run, "statusMessage")),
        ("error".into(), "ci.xml line 3: job j has no <step>".into())
    );
    assert_eq!(
        statuses(&fixture, "broken", &commit)["ci/ci.xml"],
        (
            "error".into(),
            // a status's description is plain text, as the service takes it; the run keeps the message whole
            "ci.xml line 3: job j has no step".into(),
            "https://ci.example.com/broken/runs/1/jobs/".into()
        )
    );
    assert!(rows(&fixture, "moqui.resource.GitJobRun", &[("repositoryId", "broken")]).is_empty());

    // a component repository with no ci.xml: the built-in pipeline, on push
    define(&fixture, "mr-thing", "db");
    let component = Work::new(&fixture, &base, WRITER, "mr-thing");
    let commit = component.push(
        &fixture,
        &[(
            "component.xml",
            r#"<component name="mr-thing" version="1.0.0"><depends-on name="framework" version="4.2.0"/></component>"#,
        )],
        "main",
    );
    let run = only_run(&fixture, "mr-thing", &commit);
    assert_eq!(
        (
            text(&run, "pipelineName"),
            text(&run, "definitionSource"),
            text(&run, "statusId")
        ),
        ("component".into(), "builtin".into(), "queued".into())
    );
    let job = job_named(&fixture, &text(&run, "pipelineRunId"), "test");
    assert_eq!(text(&job, "image"), "moqui-build:4.2.0");
    let spec: serde_json::Value = serde_json::from_str(&text(&job, "jobSpec")).unwrap();
    assert_eq!(spec["env"]["MOQUI_CI_COMPONENT"], "mr-thing");
    assert!(spec["steps"][0]["run"]
        .as_str()
        .unwrap()
        .contains("./moq test \"$name\""));
    assert_eq!(
        statuses(&fixture, "mr-thing", &commit)["ci/component/test"].0,
        "pending"
    );
    // a tag of it runs nothing: the built-in pipeline is for pushes and merge requests
    git_ok(&component.dir, &component.home, &["tag", "v1"]);
    git_ok(&component.dir, &component.home, &["push", "-q", "origin", "v1"]);
    settled(&fixture);
    assert_eq!(
        rows(
            &fixture,
            "moqui.resource.GitPipelineRun",
            &[("repositoryId", "mr-thing")]
        )
        .len(),
        1
    );

    // a repository with neither file runs nothing, and so does one whose branch the pipeline does not name
    define(&fixture, "plain", "db");
    let plain = Work::new(&fixture, &base, WRITER, "plain");
    plain.push(&fixture, &[("readme.txt", "no pipelines")], "main");
    plain.push(
        &fixture,
        &[("ci.xml", r#"<ci><pipeline name="p" on="push tag" branches="release/*" tags="v*"><job name="j" image="moqui-build"><step run="x"/></job></pipeline></ci>"#)],
        "main",
    );
    assert!(rows(&fixture, "moqui.resource.GitPipelineRun", &[("repositoryId", "plain")]).is_empty());
    // a tag the pipeline names does
    git_ok(&plain.dir, &plain.home, &["tag", "v2"]);
    git_ok(&plain.dir, &plain.home, &["push", "-q", "origin", "v2"]);
    settled(&fixture);
    let tagged = rows(&fixture, "moqui.resource.GitPipelineRun", &[("repositoryId", "plain")]);
    assert_eq!(tagged.len(), 1);
    assert_eq!(
        (text(&tagged[0], "event"), text(&tagged[0], "refName")),
        ("tag".into(), "refs/tags/v2".into())
    );

    // a pipeline on schedule: kept as a row when the default branch moves, and queued when its cron comes due
    plain.push(
        &fixture,
        &[("ci.xml", r#"<ci><pipeline name="nightly" on="schedule" cron="0 0 3 * * ?"><job name="j" image="moqui-build"><step run="x"/></job></pipeline></ci>"#)],
        "main",
    );
    let schedules = rows(
        &fixture,
        "moqui.resource.GitPipelineSchedule",
        &[("repositoryId", "plain")],
    );
    assert_eq!(schedules.len(), 1);
    assert_eq!(
        (
            text(&schedules[0], "pipelineName"),
            text(&schedules[0], "cron"),
            text(&schedules[0], "refName")
        ),
        ("nightly".into(), "0 0 3 * * ?".into(), "refs/heads/main".into())
    );
    let due = |now: i64| {
        let _entered = fixture.context.enter();
        let provider = crate::provider(fixture.context.factory()).unwrap();
        queue::queue_due(&provider, &fixture.entity, now).unwrap()
    };
    let now = crate::ci::now_millis();
    assert_eq!(due(now + 1000), 0);
    assert_eq!(due(now + 25 * 60 * 60 * 1000), 1);
    assert_eq!(
        due(now + 25 * 60 * 60 * 1000 + 1000),
        0,
        "once for each time it comes due"
    );
    let scheduled: Vec<EntityValue> = rows(&fixture, "moqui.resource.GitPipelineRun", &[("repositoryId", "plain")])
        .into_iter()
        .filter(|run| text(run, "event") == "schedule")
        .collect();
    assert_eq!(scheduled.len(), 1);
    // nobody pushed it: it is the repository's own branch by its own file, and trusted
    assert_eq!(
        (text(&scheduled[0], "trusted"), text(&scheduled[0], "triggeredByUserId")),
        ("Y".into(), String::new())
    );
}

#[test]
fn retention_removes_what_expired_and_secrets_are_scoped_and_never_read_back() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-retention", "success", "");
    let base = http_server(fixture.context.factory());
    let (_work, commit) = pushed(&fixture, &base, "app", &[]);
    let run_id = text(&only_run(&fixture, "app", &commit), "pipelineRunId");
    let (job_run_id, code) = start_and_run(&fixture, &base, &run_id, "test");
    assert_eq!(code, 0);
    let expire = |now: i64| {
        let _entered = fixture.context.enter();
        artifact::expire(&fixture.entity, now).unwrap()
    };
    let now = crate::ci::now_millis();
    let day = 24 * 60 * 60 * 1000;
    assert_eq!(expire(now), artifact::Expired::default());
    // the artifact asked for three days; a kept one stays
    let all = rows(&fixture, "moqui.resource.GitRunArtifact", &[("jobRunId", &job_run_id)]);
    let report = all.iter().find(|row| text(row, "artifactKind") == "report").unwrap();
    set(
        &fixture,
        "moqui.resource.GitRunArtifact",
        "artifactId",
        &text(report, "artifactId"),
        &[("keep", s("Y"))],
    );
    assert_eq!(expire(now + 4 * day).artifacts, 1);
    let left: Vec<String> = rows(&fixture, "moqui.resource.GitRunArtifact", &[("jobRunId", &job_run_id)])
        .iter()
        .map(|row| text(row, "artifactKind"))
        .collect();
    assert_eq!(left.len(), 2);
    assert!(
        left.contains(&"report".to_string()) && left.contains(&"cache".to_string()),
        "{left:?}"
    );
    // the cache goes when it was not used for a month, the log after three
    assert_eq!(expire(now + 31 * day).artifacts, 1);
    assert!(!text(&job_named(&fixture, &run_id, "test"), "logBlobId").is_empty());
    assert_eq!(expire(now + 91 * day).logs, 1);
    assert!(text(&job_named(&fixture, &run_id, "test"), "logBlobId").is_empty());

    // secrets: written by who has the update right, listed without values, scoped
    let store = format!("{SERVICES}.store#CiSecret");
    let (_, errors) = call(
        &fixture,
        READER,
        &store,
        &[("repositoryId", s("app")), ("name", s("KEY")), ("value", s("v"))],
    );
    assert!(errors.contains("needs the update right"), "{errors}");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &store,
        &[("ownerId", s("GROUP")), ("name", s("KEY")), ("value", s("v"))],
    );
    assert!(errors.contains("is the application's"), "{errors}");
    let environment = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.store#CiEnvironment"),
        &[
            ("repositoryId", s("app")),
            ("name", s("staging")),
            ("token", s("target-token")),
        ],
    );
    let environment_id = to_plain_string(&environment["environmentId"]);
    ok(
        &fixture,
        WRITER,
        &store,
        &[
            ("repositoryId", s("app")),
            ("name", s("KEY")),
            ("value", s("protected-value")),
        ],
    );
    ok(
        &fixture,
        WRITER,
        &store,
        &[
            ("repositoryId", s("app")),
            ("name", s("KEY")),
            ("value", s("staging-value")),
            ("scope", s("environment")),
            ("environmentId", s(&environment_id)),
        ],
    );
    ok(
        &fixture,
        WRITER,
        &store,
        &[
            ("repositoryId", s("app")),
            ("name", s("TAG_KEY")),
            ("value", s("tag-value")),
            ("scope", s("tags")),
        ],
    );
    let (_, errors) = call(
        &fixture,
        WRITER,
        &store,
        &[("repositoryId", s("app")), ("name", s("KEY")), ("value", s("again"))],
    );
    assert!(errors.contains("already there"), "{errors}");
    let (_, errors) = call(
        &fixture,
        READER,
        &format!("{SERVICES}.find#CiSecrets"),
        &[("repositoryId", s("app"))],
    );
    assert!(errors.contains("needs the update right"), "{errors}");
    let listed = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.find#CiSecrets"),
        &[("repositoryId", s("app"))],
    );
    let shown = format!("{:?}", listed["secretList"]);
    assert!(shown.contains("KEY") && shown.contains("staging"), "{shown}");
    assert!(!shown.contains("-value"), "no value is read back: {shown}");
    let environments = ok(
        &fixture,
        READER,
        &format!("{SERVICES}.find#CiEnvironments"),
        &[("repositoryId", s("app"))],
    );
    let shown = format!("{:?}", environments["environmentList"]);
    assert!(shown.contains("staging") && !shown.contains("target-token"), "{shown}");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.delete#CiEnvironment"),
        &[("repositoryId", s("app")), ("environmentId", s(&environment_id))],
    );
    assert!(errors.contains("secrets scoped to it"), "{errors}");

    // which secrets a job is given, by its run and its environment
    let given = |trusted: bool, protected: bool, ref_name: &str, environment: Option<&str>| -> Vec<(String, String)> {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let mut run = EntityValue::make(&fixture.entity, "moqui.resource.GitPipelineRun").unwrap();
        run.set("repositoryId", s("app"));
        run.set("refName", s(ref_name));
        run.set("trusted", s(if trusted { "Y" } else { "N" }));
        run.set("protectedRef", s(if protected { "Y" } else { "N" }));
        let spec = serde_json::json!({
            "secrets": [{"name": "KEY", "env": "KEY"}, {"name": "TAG_KEY", "env": "TAG"}, {"name": "MISSING", "env": "M"}],
            "environment": environment,
        });
        crate::ci::trust::secrets_for(&fixture.entity, &run, &spec, false)
            .unwrap()
            .into_iter()
            .map(|given| (given.env, given.value))
            .collect()
    };
    let pair = |env: &str, value: &str| (env.to_string(), value.to_string());
    // an untrusted run: nothing, whatever it asks for
    assert!(given(false, true, "refs/heads/main", Some("staging")).is_empty());
    // a trusted run on an unprotected branch: nothing of the protected scope
    assert!(given(true, false, "refs/heads/feature", None).is_empty());
    // on a protected branch: the protected secret, not the tags one
    assert_eq!(
        given(true, true, "refs/heads/main", None),
        [pair("KEY", "protected-value")]
    );
    // on a protected tag: both
    assert_eq!(
        given(true, true, "refs/tags/v1", None),
        [pair("KEY", "protected-value"), pair("TAG", "tag-value")]
    );
    // in its environment: the environment's secret, on any ref of a trusted run
    assert_eq!(
        given(true, false, "refs/heads/feature", Some("staging")),
        [pair("KEY", "staging-value")]
    );
    assert!(given(true, false, "refs/heads/feature", Some("production")).is_empty());

    // a repository that goes takes its runs with it
    let (_, errors) = call(
        &fixture,
        READER,
        &format!("{SERVICES}.delete#RepositoryRuns"),
        &[("repositoryId", s("app"))],
    );
    assert!(errors.contains("not found"), "{errors}");
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.delete#RepositoryRuns"),
        &[("repositoryId", s("app"))],
    );
    for entity in [
        "moqui.resource.GitPipelineRun",
        "moqui.resource.GitJobRun",
        "moqui.resource.GitCiSecret",
        "moqui.resource.GitCiEnvironment",
        "moqui.resource.GitRunArtifact",
    ] {
        assert!(
            rows(&fixture, entity, &[("repositoryId", "app")]).is_empty(),
            "{entity}"
        );
    }
}

#[test]
fn a_registered_runner_registers_asks_for_a_job_and_runs_it() {
    if !git_available() {
        return;
    }
    let fixture = ci_fixture("ci-registered", "success", "");
    let base = http_server(fixture.context.factory());
    // a pool of registered runners named mac, and its registration token
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.store#RunnerPool"),
        &[
            ("runnerPoolId", s("P_MAC")),
            ("poolName", s("mac")),
            ("poolKind", s("registered")),
            ("allowedImages", s("*")),
            ("maxConcurrent", Value::I64(1)),
        ],
    );
    let (_, errors) = call(
        &fixture,
        WRITER,
        &format!("{SERVICES}.create#RunnerPoolToken"),
        &[("runnerPoolId", s("P_TRUSTED"))],
    );
    assert!(errors.contains("is not of registered runners"), "{errors}");
    let pool_token = to_plain_string(
        &ok(
            &fixture,
            WRITER,
            &format!("{SERVICES}.create#RunnerPoolToken"),
            &[("runnerPoolId", s("P_MAC"))],
        )["token"],
    );
    let post = |path: &str, bearer: &str, body: &str| -> (u16, serde_json::Value) {
        let request = RunnerHttpRequest {
            method: "POST".into(),
            path: path.into(),
            query: String::new(),
            authorization: Some(format!("Bearer {bearer}")),
            content_length: Some(body.len() as u64),
            secure: true,
            client_ip: "127.0.0.1".into(),
        };
        let _entered = fixture.context.enter();
        let mut response = MemoryResponse::default();
        let mut input = body.as_bytes();
        runner_http::serve(&request, &mut input, &mut response);
        (response.status, json_of(&response))
    };
    assert_eq!(post("register", "mqcp_wrong", r#"{"name":"x"}"#).0, 401);
    let (status, registered) = post("register", &pool_token, r#"{"name":"mini-1","platform":"macos-arm64"}"#);
    assert_eq!(status, 200, "{registered}");
    let runner_token = registered["token"].as_str().unwrap().to_string();
    assert!(runner_token.starts_with("mqcr_"));
    // nothing to run yet; a pool token is not a runner token
    assert_eq!(post("claim", &runner_token, "{}"), (200, serde_json::json!({})));
    assert_eq!(post("claim", &pool_token, "{}").0, 401);

    define(&fixture, "app", "db");
    let work = Work::new(&fixture, &base, WRITER, "app");
    let commit = work.push(
        &fixture,
        &[(
            "ci.xml",
            r#"<ci><pipeline name="mac" on="push"><job name="build" image="macos" pool="mac"><step run="echo on the mac"/></job></pipeline></ci>"#,
        )],
        "main",
    );
    let run_id = text(&only_run(&fixture, "app", &commit), "pipelineRunId");
    // the engine starts nothing for such a pool: the job waits for its runner
    assert_eq!(start(&fixture), engine::Started::default());
    assert_eq!(text(&job_named(&fixture, &run_id, "build"), "statusId"), "queued");
    let (status, claimed) = post("claim", &runner_token, "{}");
    assert_eq!(status, 200);
    let job_run_id = claimed["jobRunId"].as_str().unwrap().to_string();
    let job_token = claimed["token"].as_str().unwrap().to_string();
    let job = job_named(&fixture, &run_id, "build");
    assert_eq!(
        (
            text(&job, "statusId"),
            text(&job, "runnerPoolId"),
            text(&job, "jobRunId")
        ),
        ("running".into(), "P_MAC".into(), job_run_id.clone())
    );
    // one job at a time
    assert_eq!(post("claim", &runner_token, "{}"), (200, serde_json::json!({})));
    let (code, _) = run_job(&fixture, &base, &job_run_id, &job_token);
    assert_eq!(code, 0);
    assert_eq!(text(&job_named(&fixture, &run_id, "build"), "statusId"), "success");
    assert!(log_of(&fixture, WRITER, &job_run_id).contains("on the mac"));
    assert!(
        FakeHosts::of(fixture.context.factory()).calls().is_empty(),
        "no container was asked for"
    );
    // a runner that stops answering loses its job
    let second = work.push(&fixture, &[("more.txt", "x")], "main");
    let second_run = text(&only_run(&fixture, "app", &second), "pipelineRunId");
    assert_eq!(post("claim", &runner_token, "{}").0, 200);
    {
        let _entered = fixture.context.enter();
        let ended = engine::end_overdue(&fixture.entity, crate::ci::now_millis() + 5_000).unwrap();
        assert_eq!(ended.lost, 1);
    }
    assert_eq!(
        text(&job_named(&fixture, &second_run, "build"), "statusMessage"),
        "The runner stopped answering"
    );
}
