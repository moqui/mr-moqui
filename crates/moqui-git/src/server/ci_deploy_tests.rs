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
const FORGED_TOKEN: &str = "not-what-the-target-issued-0123456789";
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
    let own = base.clone();
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
            )
            .and_then(|attestation| {
                attest::check_audience(&attestation, std::slice::from_ref(&own)).map(|()| attestation)
            });
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

/// Protect main of `repository`: pushed to by its writers only.
fn protect_main(fixture: &Fixture, repository: &str) {
    ok(
        fixture,
        WRITER,
        "org.moqui.git.GitServices.store#GitBranchRule",
        &[
            ("repositoryId", s(repository)),
            ("refPattern", s("main")),
            ("pushUserGroupId", s("GIT_WRITERS")),
        ],
    );
}

/// A fixture whose deploy calls may reach loopback, and the repository `site`: a component with the release pipeline,
/// pushed to main (protected) by the writer. The work tree and main's commit.
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
    protect_main(&fixture, "site");
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
    assert_eq!(
        text(&row, "approvalRequired"),
        "N",
        "main is protected and no approval is required"
    );
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
    // addressed to the target it was posted to, the environment's data types signed with it (none named)
    assert_eq!(attestation.audience, attest::base_url(&target));
    assert!(attestation.data_types.is_empty());
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
    // the target's and clone URLs to a maintainer only
    assert!(format!("{environments:?}").contains(&target));
    let read = ok(
        &fixture,
        READER,
        &format!("{DEPLOYMENTS}.find#CiEnvironments"),
        &[("repositoryId", s("site"))],
    );
    assert!(!format!("{read:?}").contains("127.0.0.1"), "{read:?}");
    assert!(format!("{read:?}").contains("prod"));
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
            ("token", s(FORGED_TOKEN)),
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
    assert!(!format!("{row:?}").contains(FORGED_TOKEN));
    assert!(received.lock().unwrap()[1].attestation.is_none());
    // the target's words are on the row for a maintainer, not on the public status nor for a reader
    assert_eq!(statuses(&fixture, "site", &commit)["deploy/prod"].1, "Deploy failed");
    let get = format!("{DEPLOYMENTS}.get#GitDeployment");
    let got = ok(&fixture, WRITER, &get, &[("deploymentId", s(&forged))]);
    assert!(format!("{got:?}").contains("signature does not verify"));
    let got = ok(&fixture, READER, &get, &[("deploymentId", s(&forged))]);
    assert!(!format!("{got:?}").contains("signature"), "{got:?}");
    let found = ok(
        &fixture,
        READER,
        &format!("{DEPLOYMENTS}.find#GitDeployments"),
        &[("repositoryId", s("site"))],
    );
    assert!(!format!("{found:?}").contains("signature"), "{found:?}");
    // paged in the database, with the count of all
    let page = |index: i64| {
        ok(
            &fixture,
            READER,
            &format!("{DEPLOYMENTS}.find#GitDeployments"),
            &[
                ("repositoryId", s("site")),
                ("pageSize", Value::I64(1)),
                ("pageIndex", Value::I64(index)),
            ],
        )
    };
    let (first, second) = (page(0), page(1));
    assert_eq!(first["deploymentCount"], Value::I64(2));
    let ids = |answer: &BTreeMap<String, Value>| match &answer["deploymentList"] {
        Value::List(list) => list
            .iter()
            .map(|row| match row {
                Value::Map(row) => to_plain_string(&row["deploymentId"]),
                _ => String::new(),
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    assert_eq!((ids(&first), ids(&second)), (vec![forged.clone()], vec![id.clone()]));
}

/// decide#GitDeployment is the one approval step: a run an approver approved (its job in the environment) does not
/// approve the deployment; and a branch no rule protects, deployed because allowedRefs names it, waits for an approver
/// though the environment requires none.
#[test]
fn an_approved_run_still_waits_for_decide_and_an_unprotected_branch_waits_for_an_approver() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, work, _commit) = site("ci-deploy-decide");
    let (target, received) = fake_target(TOKEN);
    let environment_id = environment(&fixture, &target, TOKEN, false);
    let request = format!("{DEPLOYMENTS}.request#GitDeployment");
    ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.store#CiEnvironment"),
        &[
            ("repositoryId", s("site")),
            ("environmentId", s(&environment_id)),
            ("allowedRefs", s("main feature")),
        ],
    );
    // feature: named by allowedRefs, protected by no rule
    work.push(&fixture, &[("notes.txt", "a feature")], "feature");
    let out = ok(
        &fixture,
        WRITER,
        &request,
        &[
            ("repositoryId", s("site")),
            ("environment", s("prod")),
            ("refName", s("feature")),
        ],
    );
    let unprotected = to_plain_string(&out["deploymentId"]);
    assert_eq!(text(&deployment(&fixture, &unprotected), "approvalRequired"), "Y");
    start(&fixture);
    let job = job_named(&fixture, &to_plain_string(&out["pipelineRunId"]), "build");
    {
        let _entered = fixture.context.enter();
        let outcome = engine::Outcome {
            status: "success",
            exit_code: Some(0),
            message: None,
        };
        assert!(engine::finish_job(&fixture.entity, &text(&job, "jobRunId"), &outcome).unwrap());
    }
    assert_eq!(
        text(&deployment(&fixture, &unprotected), "statusId"),
        deploy::WAITING_APPROVAL
    );

    // an environment that requires approval, whose pipeline's job is in it: the run waits for its own approval
    ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.store#CiEnvironment"),
        &[
            ("repositoryId", s("site")),
            ("environmentId", s(&environment_id)),
            ("approvalRequired", Value::Bool(true)),
        ],
    );
    let in_environment = SITE_CI.replace(r#"pool="linux">"#, r#"pool="linux" environment="prod">"#);
    work.push(&fixture, &[("ci.xml", &in_environment)], "main");
    let out = ok(
        &fixture,
        WRITER,
        &request,
        &[("repositoryId", s("site")), ("environment", s("prod"))],
    );
    let id = to_plain_string(&out["deploymentId"]);
    let run_id = to_plain_string(&out["pipelineRunId"]);
    let run = rows(&fixture, "moqui.resource.GitPipelineRun", &[("pipelineRunId", &run_id)]).remove(0);
    assert_eq!(
        text(&run, "approvalState"),
        "waiting",
        "{}",
        text(&run, "statusMessage")
    );
    ok(
        &fixture,
        "git.second",
        "org.moqui.git.GitCiServices.approve#PipelineRun",
        &[("pipelineRunId", s(&run_id))],
    );
    start(&fixture);
    let job = job_named(&fixture, &run_id, "build");
    assert_eq!(text(&job, "statusId"), "running", "{}", text(&job, "statusMessage"));
    {
        let _entered = fixture.context.enter();
        let outcome = engine::Outcome {
            status: "success",
            exit_code: Some(0),
            message: None,
        };
        assert!(engine::finish_job(&fixture.entity, &text(&job, "jobRunId"), &outcome).unwrap());
    }
    let row = deployment(&fixture, &id);
    assert_eq!(
        (text(&row, "statusId"), text(&row, "approvedByUserId")),
        (deploy::WAITING_APPROVAL.into(), String::new()),
        "the run's approval is not the deployment's"
    );
    ok(
        &fixture,
        "git.second",
        &format!("{DEPLOYMENTS}.decide#GitDeployment"),
        &[("deploymentId", s(&id)), ("decision", s("approve"))],
    );
    assert_eq!(text(&deployment(&fixture, &id), "statusId"), deploy::DEPLOYING);
    assert!(received.lock().unwrap().is_empty());
}

/// The deploy step is claimed by one conditional update: two steps of one deployment at once call the target once.
/// `resume` starts a step nobody took, and fails one that began past its time and never ended.
#[test]
fn the_deploy_step_is_claimed_once_and_resume_starts_or_ends_a_step() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, _work, commit) = site("ci-deploy-step");
    let (target, received) = fake_target(TOKEN);
    environment(&fixture, &target, TOKEN, false);
    let id = requested_and_run(&fixture, "success");
    let factory = Arc::clone(fixture.context.factory());
    let steps: Vec<_> = (0..4)
        .map(|_| {
            let factory = Arc::clone(&factory);
            let id = id.clone();
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                deploy::step(&factory.entity().unwrap(), &id).unwrap();
            })
        })
        .collect();
    for step in steps {
        step.join().unwrap();
    }
    assert_eq!(received.lock().unwrap().len(), 1, "one call of the target");
    assert_eq!(text(&deployment(&fixture, &id), "statusId"), deploy::DEPLOYED);

    // a step nobody took (the server stopped before its thread ran): resume starts it
    let waiting = requested_and_run(&fixture, "success");
    let now = crate::ci::now_millis();
    {
        let _entered = fixture.context.enter();
        assert_eq!(deploy::resume(&fixture.entity, now + 120_000).unwrap(), (1, 0));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while text(&deployment(&fixture, &waiting), "statusId") == deploy::DEPLOYING && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(text(&deployment(&fixture, &waiting), "statusId"), deploy::DEPLOYED);
    assert_eq!(received.lock().unwrap().len(), 2);

    // a step that began and never ended (the server stopped while it ran): failed past its time, said generically
    let stuck = requested_and_run(&fixture, "success");
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let mut row = deployment(&fixture, &stuck);
        row.set("startedDate", moqui_core::user::stamp_value(now));
        row.update(&fixture.entity).unwrap();
        assert_eq!(deploy::resume(&fixture.entity, now + 16 * 60_000).unwrap(), (0, 1));
    }
    let row = deployment(&fixture, &stuck);
    assert_eq!(text(&row, "statusId"), deploy::FAILED);
    assert!(
        text(&row, "errorText").contains("never ended"),
        "{}",
        text(&row, "errorText")
    );
    assert_eq!(statuses(&fixture, "site", &commit)["deploy/prod"].1, "Deploy failed");
    assert_eq!(received.lock().unwrap().len(), 2);
}

/// An environment refuses a clone URL with credentials in it, an approver group that does not exist and a token
/// shorter than 32 characters.
#[test]
fn an_environment_refuses_a_clone_url_with_credentials_an_unknown_group_and_a_short_token() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, _work, _commit) = site("ci-deploy-environment");
    let store = format!("{DEPLOYMENTS}.store#CiEnvironment");
    let stored = |more: &[(&str, Value)]| {
        let mut parameters = vec![
            ("repositoryId", s("site")),
            ("name", s("prod")),
            ("targetUrl", s("https://target.example.com")),
        ];
        parameters.extend(more.iter().cloned());
        call(&fixture, WRITER, &store, &parameters).1
    };
    for url in [
        "https://user:secret@source.example.com/site.git",
        "https://token@source.example.com/site.git",
    ] {
        let errors = stored(&[("cloneUrl", s(url))]);
        assert!(errors.contains("no user or password"), "{url}: {errors}");
        assert!(!errors.contains("secret@"), "{errors}");
    }
    let errors = stored(&[("approverGroupId", s("NO_SUCH_GROUP"))]);
    assert!(errors.contains("no user group NO_SUCH_GROUP"), "{errors}");
    let errors = stored(&[("token", s("only-31-characters-long-0123456"))]);
    assert!(errors.contains("at least 32 characters"), "{errors}");
    assert!(stored(&[
        ("cloneUrl", s("ssh://git@source.example.com/site.git")),
        ("token", s(TOKEN)),
        ("dataTypes", s("seed, seed-initial")),
    ])
    .is_empty());
    let found = ok(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.find#CiEnvironments"),
        &[("repositoryId", s("site"))],
    );
    assert!(format!("{found:?}").contains("seed, seed-initial"), "{found:?}");
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
    protect_main(&fixture, "site");
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

/// Call `name` as `user` inside a transaction with the transaction cache on (as `service-facade@tx-cache` makes an
/// operation's transaction once an XML service runs in it), so its entity writes are held until the commit.
fn call_cached(fixture: &Fixture, user: &str, name: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let _entered = fixture.context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect();
    let options = moqui_core::service::call::CallOptions {
        use_transaction_cache: Some(true),
        ..moqui_core::service::call::CallOptions::default()
    };
    let out = moqui_core::service::call::sync_with(name, &parameters, &options).unwrap();
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| {
        let text = ec.message().errors_text().join("\n");
        ec.message().clear();
        text
    })
    .unwrap_or_default();
    moqui_core::user::logout().unwrap();
    assert!(errors.trim().is_empty(), "{name}: {errors}");
    out
}

/// A deployment's moves are right inside a transaction with the transaction cache on: its row is written before the
/// conditional updates that move it (they are SQL of their own, which reads no held row), so a request leaves it
/// waiting for its pipeline as it answers, and a refusal ends it refused, its notice saying so.
#[test]
fn a_deployment_moves_on_inside_a_cached_transaction() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, _work, _commit) = site("ci-deploy-cached");
    let (target, _received) = fake_target(TOKEN);
    environment(&fixture, &target, TOKEN, true);
    let out = call_cached(
        &fixture,
        WRITER,
        &format!("{DEPLOYMENTS}.request#GitDeployment"),
        &[("repositoryId", s("site")), ("environment", s("prod"))],
    );
    assert_eq!(to_plain_string(&out["statusId"]), deploy::WAITING_PIPELINE, "{out:?}");
    let id = to_plain_string(&out["deploymentId"]);
    let run_id = to_plain_string(&out["pipelineRunId"]);
    let row = deployment(&fixture, &id);
    assert_eq!(
        (text(&row, "statusId"), text(&row, "pipelineRunId")),
        (deploy::WAITING_PIPELINE.into(), run_id.clone())
    );
    // its run passes: it waits for approval; refused in a cached transaction, it ends refused, and so does its notice
    start(&fixture);
    let job = job_named(&fixture, &run_id, "build");
    {
        let _entered = fixture.context.enter();
        let outcome = engine::Outcome {
            status: "success",
            exit_code: Some(0),
            message: None,
        };
        assert!(engine::finish_job(&fixture.entity, &text(&job, "jobRunId"), &outcome).unwrap());
    }
    assert_eq!(text(&deployment(&fixture, &id), "statusId"), deploy::WAITING_APPROVAL);
    let out = call_cached(
        &fixture,
        "git.second",
        &format!("{DEPLOYMENTS}.decide#GitDeployment"),
        &[
            ("deploymentId", s(&id)),
            ("decision", s("refuse")),
            ("reason", s("not today")),
        ],
    );
    assert_eq!(to_plain_string(&out["statusId"]), deploy::REFUSED);
    assert_eq!(text(&deployment(&fixture, &id), "statusId"), deploy::REFUSED);
    let notices = rows(
        &fixture,
        "moqui.security.user.NotificationMessage",
        &[("topic", deploy::TOPIC)],
    );
    let refused: Vec<String> = notices
        .iter()
        .map(|notice| text(notice, "messageJson"))
        .filter(|json| json.contains(&id) && json.contains(deploy::REFUSED))
        .collect();
    assert!(
        !refused.is_empty(),
        "the refusal's notice carries its final state: {notices:?}"
    );
    let titles: Vec<String> = notices.iter().map(|notice| text(notice, "titleText")).collect();
    for (title, json) in titles
        .iter()
        .zip(notices.iter().map(|notice| text(notice, "messageJson")))
    {
        if title.contains("was refused") {
            assert!(json.contains(deploy::REFUSED), "{title}: {json}");
        }
    }
}

/// SEC-GIT-91: who widens which runs an environment gives its secrets to (a ref `allowedRefs` did not name, approval
/// no longer required) gives them again: their values are cleared, as a secret moved to another environment takes a
/// new value; and a new target URL takes no token given for the old one. A change that widens nothing keeps them.
#[test]
fn widening_an_environment_clears_its_secrets_and_a_new_target_its_token() {
    if !git_available() {
        eprintln!("skipped: git is not on the PATH");
        return;
    }
    let (fixture, _base, _work, _commit) = site("ci-deploy-widen");
    let id = environment(&fixture, "https://target.example.com", TOKEN, true);
    let store = format!("{DEPLOYMENTS}.store#CiEnvironment");
    let secret = |secret_id: Option<&str>| {
        let mut parameters = vec![
            ("repositoryId", s("site")),
            ("name", s("PROD_KEY")),
            ("scope", s("environment")),
            ("environmentId", s(&id)),
            ("value", s("the-prod-key")),
        ];
        if let Some(secret_id) = secret_id {
            parameters.push(("secretId", s(secret_id)));
        }
        let out = ok(&fixture, WRITER, &format!("{DEPLOYMENTS}.store#CiSecret"), &parameters);
        to_plain_string(&out["secretId"])
    };
    let value = || {
        let row = rows(&fixture, crate::ci::CI_SECRET, &[("environmentId", &id)]).remove(0);
        text(&row, "secretValue")
    };
    let token = || {
        text(
            &rows(&fixture, crate::ci::CI_ENVIRONMENT, &[("environmentId", &id)])[0],
            "targetToken",
        )
    };
    let change = |fields: &[(&str, Value)]| {
        let mut parameters = vec![("repositoryId", s("site")), ("environmentId", s(&id))];
        parameters.extend(fields.iter().cloned());
        to_plain_string(&ok(&fixture, WRITER, &store, &parameters)["cleared"])
    };
    let secret_id = secret(None);
    assert_eq!(
        change(&[("dataTypes", s("seed")), ("allowedRefs", s("main"))]),
        "",
        "nothing cleared"
    );
    assert_eq!(value(), "the-prod-key", "narrowing or keeping nothing new keeps it");
    let cleared = change(&[("allowedRefs", s("main feature/*"))]);
    assert!(cleared.contains("the values of its 1 secrets"), "{cleared}");
    assert_eq!(value(), "", "a ref allowedRefs did not name");
    secret(Some(&secret_id));
    // another group may approve the runs that are given the secrets
    change(&[("approverGroupId", s("GIT_WRITERS"))]);
    assert_eq!(value(), "", "another approver group");
    secret(Some(&secret_id));
    // a caller with no groups sends null on every save: the stored group stays, and so do the secrets
    assert_eq!(change(&[("approverGroupId", Value::Null)]), "", "nothing cleared");
    assert_eq!(value(), "the-prod-key");
    assert_eq!(
        text(
            &rows(&fixture, crate::ci::CI_ENVIRONMENT, &[("environmentId", &id)])[0],
            "approverGroupId"
        ),
        "GIT_WRITERS"
    );
    change(&[("approvalRequired", Value::Bool(false))]);
    assert_eq!(value(), "", "approval no longer required");
    assert_eq!(token(), TOKEN);
    change(&[("targetUrl", s("https://elsewhere.example.com"))]);
    assert_eq!(token(), "", "a new target takes no old token");
}
