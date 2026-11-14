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

//! Deployments end to end (plans/Dogfood.md §8.6, DG10; docs/interfaces/Pipelines.md "Deployments"): a real push of a
//! component with a deploy pipeline, `request#GitDeployment`, the pipeline's job on the fake host type ended as a
//! runner would end it, the approval, and the deploy step's call over loopback to a target that verifies the
//! attestation with the same code the target's `deploy#AttestedComponent` uses (`ci::attest::verify`; the target's own
//! gate is tested in moqui-devkit, `deploy/git_tests.rs`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::*;
use crate::ci::{attest, deploy};

const TOKEN: &str = "the-target-issued-this-0123456789";
const DEPLOYMENTS: &str = "org.moqui.git.GitCiServices";

/// What the fake target was sent: the whole request, and the attestation when it verified.
#[derive(Clone, Debug, Default)]
struct Received {
    request: String,
    attestation: Option<attest::Attestation>,
}

/// A target over loopback: it verifies each attestation with `secret`, answers 200 with what a deploy answers when it
/// does, else 400 with the refusal, and keeps what it was sent.
fn fake_target(secret: &'static str) -> (String, Arc<Mutex<Vec<Received>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let received: Arc<Mutex<Vec<Received>>> = Arc::default();
    let kept = Arc::clone(&received);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0_usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                head.push_str(&line);
            }
            let mut body = vec![0_u8; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8_lossy(&body).into_owned();
            let json: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let verified = attest::verify(
                secret,
                json["attestation"].as_str().unwrap_or_default(),
                json["signature"].as_str().unwrap_or_default(),
                crate::ci::now_millis(),
                300_000,
            );
            let (status, answer) = match &verified {
                Ok(attestation) => (
                    "200 OK",
                    serde_json::json!({
                        "componentName": attestation.component,
                        "commitId": attestation.commit_id,
                        "deployStatus": "deployed",
                        "deploymentId": attestation.deployment_id,
                        "report": {"problems": []},
                    }),
                ),
                Err(refusal) => (
                    "400 Bad Request",
                    serde_json::json!({"errorCode": 400, "errors": refusal.to_string()}),
                ),
            };
            moqui_lib::sync::lock(&kept).push(Received {
                request: format!("{head}\r\n{body}"),
                attestation: verified.ok(),
            });
            let answer = answer.to_string();
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    (base, received)
}

const SITE_CI: &str = r#"<ci>
    <pipeline name="release" on="deploy">
        <job name="build" image="moqui-build:1" pool="linux"><step run="./moq test site"/></job>
    </pipeline>
</ci>"#;

/// A fixture whose deploy calls may reach loopback, and the repository `site`: a component with the release pipeline,
/// pushed to main and to feature by the writer. The work tree and main's commit.
fn site(name: &str) -> (Fixture, String, Work, String) {
    let fixture = ci_fixture(name, "success", r#"deploy-private-network="true""#);
    let base = http_server(fixture.context.factory());
    define(&fixture, "site", "db");
    let work = Work::new(&fixture, &base, WRITER, "site");
    let commit = work.push(
        &fixture,
        &[("component.xml", r#"<component name="site"/>"#), ("ci.xml", SITE_CI)],
        "main",
    );
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        // a second maintainer, to approve, and an account that does not view the repository
        crate::server::tests::user(&fixture.entity, "git.second");
        crate::server::tests::user(&fixture.entity, "git.nobody");
        let mut member = EntityValue::make(&fixture.entity, "moqui.security.UserGroupMember").unwrap();
        member.set("userId", s("git.second"));
        member.set("userGroupId", s("GIT_WRITERS"));
        member.set(
            "fromDate",
            moqui_lib::object::basic_convert(&Value::I64(1_000), "Timestamp").value,
        );
        member.create(&fixture.entity).unwrap();
    }
    (fixture, base, work, commit)
}

fn environment(fixture: &Fixture, target: &str, token: &str, approval: bool) -> String {
    let out = ok(
        fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.store#CiEnvironment"),
        &[
            ("repositoryId", s("site")),
            ("name", s("prod")),
            ("targetUrl", s(target)),
            ("token", s(token)),
            ("requiredPipeline", s("release")),
            ("approvalRequired", Value::Bool(approval)),
            ("allowedRefs", s("main")),
        ],
    );
    to_plain_string(&out["environmentId"])
}

fn deployment(fixture: &Fixture, id: &str) -> EntityValue {
    rows(fixture, "moqui.resource.GitDeployment", &[("deploymentId", id)]).remove(0)
}

/// Ask for a deployment of main as the writer, and end its run's one job as `status`. The deployment's id.
fn requested_and_run(fixture: &Fixture, status: &'static str) -> String {
    let out = ok(
        fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.request#GitDeployment"),
        &[("repositoryId", s("site")), ("environment", s("prod"))],
    );
    assert_eq!(to_plain_string(&out["statusId"]), deploy::WAITING_PIPELINE, "{out:?}");
    let id = to_plain_string(&out["deploymentId"]);
    let run_id = to_plain_string(&out["pipelineRunId"]);
    let run = rows(fixture, "moqui.resource.GitPipelineRun", &[("pipelineRunId", &run_id)]).remove(0);
    assert_eq!(
        (text(&run, "event"), text(&run, "pipelineName"), text(&run, "trusted")),
        ("deploy".into(), "release".into(), "Y".into())
    );
    start(fixture);
    let job = job_named(fixture, &run_id, "build");
    assert_eq!(text(&job, "statusId"), "running", "{}", text(&job, "statusMessage"));
    let _entered = fixture.context.enter();
    let outcome = engine::Outcome {
        status,
        exit_code: Some(i64::from(status != "success")),
        message: None,
    };
    assert!(engine::finish_job(&fixture.entity, &text(&job, "jobRunId"), &outcome).unwrap());
    id
}

fn step(fixture: &Fixture, id: &str) {
    let _entered = fixture.context.enter();
    deploy::step(&fixture.entity, id).unwrap();
}

#[test]
fn a_deployment_runs_its_pipeline_then_posts_a_signed_attestation_its_target_verifies() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, work, commit) = site("ci-deploy-run");
    let (target, received) = fake_target(TOKEN);
    let environment_id = environment(&fixture, &target, TOKEN, false);
    let request = format!("{DEPLOYMENTS}.request#GitDeployment");

    // the update right, and a repository the caller does not view is not found
    let (_, errors) = call(
        &fixture,
        READER,
        &request,
        &[("repositoryId", s("site")), ("environment", s("prod"))],
    );
    assert!(errors.contains("needs the update right"), "{errors}");
    let (_, errors) = call(
        &fixture,
        "git.nobody",
        &request,
        &[("repositoryId", s("site")), ("environment", s("prod"))],
    );
    assert!(errors.contains("not found"), "{errors}");
    // a branch neither protected nor in allowedRefs, and a commit that is not the branch's head
    let feature = work.push(&fixture, &[("notes.txt", "a feature")], "feature");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &request,
        &[
            ("repositoryId", s("site")),
            ("environment", s("prod")),
            ("refName", s("feature")),
        ],
    );
    assert!(errors.contains("is neither"), "{errors}");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &request,
        &[
            ("repositoryId", s("site")),
            ("environment", s("prod")),
            ("commitId", s(&feature)),
        ],
    );
    assert!(errors.contains("is not the head of"), "{errors}");

    let id = requested_and_run(&fixture, "success");
    let row = deployment(&fixture, &id);
    assert_eq!(text(&row, "statusId"), deploy::DEPLOYING, "{}", text(&row, "errorText"));
    assert!(received.lock().unwrap().is_empty(), "nothing is sent before the step");
    step(&fixture, &id);
    let row = deployment(&fixture, &id);
    assert_eq!(text(&row, "statusId"), deploy::DEPLOYED, "{}", text(&row, "errorText"));
    assert_eq!(text(&row, "componentName"), "site");
    assert!(
        text(&row, "deployResult").contains("\"deployed\""),
        "{}",
        text(&row, "deployResult")
    );
    // a second step of the same deployment calls nothing
    step(&fixture, &id);
    let sent = received.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "one call");
    let attestation = sent[0]
        .attestation
        .clone()
        .expect("the target verified the attestation");
    assert_eq!(
        (
            attestation.commit_id.as_str(),
            attestation.component.as_str(),
            attestation.pipeline.as_str(),
            attestation.ref_name.as_str(),
            attestation.environment.as_str(),
            attestation.deployment_id.as_str(),
            attestation.run_status.as_str(),
        ),
        (
            commit.as_str(),
            "site",
            "release",
            "refs/heads/main",
            "prod",
            id.as_str(),
            "success"
        )
    );
    assert!(
        attestation.repository_url.ends_with("/git/site.git"),
        "{}",
        attestation.repository_url
    );
    assert!(
        sent[0].request.starts_with("POST /rest/s1/moqui/components/deploy "),
        "{}",
        sent[0].request
    );
    // the token is in no request, no answer and no row a service reads
    assert!(!sent[0].request.contains(TOKEN));
    let got = ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.get#GitDeployment"),
        &[("deploymentId", s(&id))],
    );
    let environments = ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.find#CiEnvironments"),
        &[("repositoryId", s("site"))],
    );
    for answer in [format!("{got:?}"), format!("{environments:?}"), format!("{row:?}")] {
        assert!(!answer.contains(TOKEN), "{answer}");
    }
    assert!(format!("{environments:?}").contains("hasToken"));
    let environment = rows(
        &fixture,
        "moqui.resource.GitCiEnvironment",
        &[("environmentId", &environment_id)],
    )
    .remove(0);
    assert_eq!(
        (
            text(&environment, "lastCommitId"),
            text(&environment, "lastDeploymentId")
        ),
        (commit.clone(), id.clone())
    );
    // the commit's status, under a context of its own
    let states = statuses(&fixture, "site", &commit);
    assert_eq!(states["deploy/prod"].0, "success", "{states:?}");
    // read with the view right; not found to one who does not view the repository
    let found = ok(
        &fixture,
        READER,
        &format!("{DEPLOYMENTS}.find#GitDeployments"),
        &[("repositoryId", s("site"))],
    );
    assert_eq!(found["deploymentCount"], Value::I64(1));
    let (_, errors) = call(
        &fixture,
        "git.nobody",
        &format!("{DEPLOYMENTS}.get#GitDeployment"),
        &[("deploymentId", s(&id))],
    );
    assert!(errors.contains("no deployment"), "{errors}");
    let (_, errors) = call(
        &fixture,
        "git.nobody",
        &format!("{DEPLOYMENTS}.find#GitDeployments"),
        &[("repositoryId", s("site"))],
    );
    assert!(errors.contains("not found"), "{errors}");

    // a token the target did not issue: the target refuses the forged attestation, and the deployment fails with its
    // answer, the token still in no row
    ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.store#CiEnvironment"),
        &[
            ("repositoryId", s("site")),
            ("environmentId", s(&environment_id)),
            ("token", s("not-what-the-target-issued-00")),
        ],
    );
    let forged = requested_and_run(&fixture, "success");
    step(&fixture, &forged);
    let row = deployment(&fixture, &forged);
    assert_eq!(text(&row, "statusId"), deploy::FAILED);
    assert!(
        text(&row, "errorText").contains("signature does not verify"),
        "{}",
        text(&row, "errorText")
    );
    assert!(!format!("{row:?}").contains("not-what-the-target-issued-00"));
    assert!(received.lock().unwrap()[1].attestation.is_none());
}

#[test]
fn a_failed_pipeline_refuses_and_an_approval_is_another_approvers() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, _work, commit) = site("ci-deploy-approve");
    let (target, received) = fake_target(TOKEN);
    environment(&fixture, &target, TOKEN, true);
    let decide = format!("{DEPLOYMENTS}.decide#GitDeployment");

    // a run that fails fails the deployment, and nothing is sent
    let failed = requested_and_run(&fixture, "failure");
    let row = deployment(&fixture, &failed);
    assert_eq!(text(&row, "statusId"), deploy::FAILED);
    assert!(
        text(&row, "errorText").contains("ended failure"),
        "{}",
        text(&row, "errorText")
    );
    assert_eq!(statuses(&fixture, "site", &commit)["deploy/prod"].0, "failure");

    // a run that passes waits for an approver: not the requester, not a reader
    let waiting = requested_and_run(&fixture, "success");
    assert_eq!(
        text(&deployment(&fixture, &waiting), "statusId"),
        deploy::WAITING_APPROVAL
    );
    assert_eq!(statuses(&fixture, "site", &commit)["deploy/prod"].0, "pending");
    let (_, errors) = call(
        &fixture,
        WRITER,
        &decide,
        &[("deploymentId", s(&waiting)), ("decision", s("approve"))],
    );
    assert!(errors.contains("another than who asked"), "{errors}");
    let (_, errors) = call(
        &fixture,
        READER,
        &decide,
        &[("deploymentId", s(&waiting)), ("decision", s("approve"))],
    );
    assert!(errors.contains("delete right"), "{errors}");
    let got = ok(
        &fixture,
        "git.second",
        &format!("{DEPLOYMENTS}.get#GitDeployment"),
        &[("deploymentId", s(&waiting))],
    );
    assert_eq!(got["canDecide"], Value::Bool(true));
    ok(
        &fixture,
        "git.second",
        &decide,
        &[("deploymentId", s(&waiting)), ("decision", s("approve"))],
    );
    let row = deployment(&fixture, &waiting);
    assert_eq!(
        (text(&row, "statusId"), text(&row, "approvedByUserId")),
        (deploy::DEPLOYING.into(), "git.second".into())
    );
    step(&fixture, &waiting);
    assert_eq!(text(&deployment(&fixture, &waiting), "statusId"), deploy::DEPLOYED);
    assert_eq!(received.lock().unwrap().len(), 1);

    // refused, with its reason; and one cancelled by its requester while its run waits, with the run
    let refused = requested_and_run(&fixture, "success");
    ok(
        &fixture,
        "git.second",
        &decide,
        &[
            ("deploymentId", s(&refused)),
            ("decision", s("refuse")),
            ("reason", s("not on a Friday")),
        ],
    );
    let row = deployment(&fixture, &refused);
    assert_eq!(text(&row, "statusId"), deploy::REFUSED);
    assert!(text(&row, "errorText").contains("not on a Friday"));
    let out = ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.request#GitDeployment"),
        &[("repositoryId", s("site")), ("environment", s("prod"))],
    );
    let cancelled = to_plain_string(&out["deploymentId"]);
    ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.cancel#GitDeployment"),
        &[("deploymentId", s(&cancelled))],
    );
    assert_eq!(text(&deployment(&fixture, &cancelled), "statusId"), deploy::CANCELLED);
    let run = rows(
        &fixture,
        "moqui.resource.GitPipelineRun",
        &[("pipelineRunId", &to_plain_string(&out["pipelineRunId"]))],
    )
    .remove(0);
    assert_eq!(text(&run, "statusId"), "cancelled");
    assert_eq!(received.lock().unwrap().len(), 1, "only the approved one was sent");
}

#[test]
fn a_repository_that_is_not_a_component_is_not_deployable_here() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let fixture = ci_fixture("ci-deploy-framework", "success", r#"deploy-private-network="true""#);
    let base = http_server(fixture.context.factory());
    define(&fixture, "site", "db");
    let work = Work::new(&fixture, &base, WRITER, "site");
    work.push(&fixture, &[("Cargo.toml", "[workspace]"), ("ci.xml", SITE_CI)], "main");
    let (target, received) = fake_target(TOKEN);
    environment(&fixture, &target, TOKEN, false);
    let id = requested_and_run(&fixture, "success");
    step(&fixture, &id);
    let row = deployment(&fixture, &id);
    assert_eq!(text(&row, "statusId"), deploy::FAILED);
    assert!(
        text(&row, "errorText").contains("not deployable here"),
        "{}",
        text(&row, "errorText")
    );
    assert!(received.lock().unwrap().is_empty());
}
