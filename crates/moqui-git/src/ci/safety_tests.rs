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

//! The engine's rules of who runs what (docs/interfaces/Pipelines.md): the runner pools are an operator's, a pool of
//! registered runners is one owner's with tokens that expire and runners that are revoked, a shared pool takes no job
//! of an owner with its own, a refused or never approved run is not run again, what a repository keeps is capped, and
//! a host's errors do not erase the caller's.

use moqui_core::context::ExecutionContext;
use moqui_core::entity::EntityValue;
use moqui_lib::object::{to_plain_string, Value};

use super::artifact::{self, Kind, Upload};
use super::{engine, registered, token, JOB_RUN, PIPELINE_RUN, RUNNER, RUNNER_POOL};
use crate::server::hosting_tests::{call, grant_entities, ok};
use crate::server::tests::{create, fixture_with, Fixture, READER, WRITER};

const SERVICES: &str = "org.moqui.git.GitCiServices";

fn s(text: &str) -> Value {
    Value::string(text)
}

fn row(fixture: &Fixture, entity: &str, fields: &[(&str, Value)]) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut row = EntityValue::make(&fixture.entity, entity).unwrap();
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.create(&fixture.entity).unwrap();
}

fn one(fixture: &Fixture, entity: &str, key: &str, id: &str) -> EntityValue {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    super::one(&fixture.entity, entity, key, id).unwrap().unwrap()
}

fn text(row: &EntityValue, name: &str) -> String {
    super::field(row, name).unwrap_or_default()
}

/// The writer as an operator of the pools (`GIT_CI_ADMIN`); the reader not.
fn operator_fixture(name: &str, conf: &str) -> Fixture {
    let fixture = fixture_with(name, moqui_core::context::framework_dir(), conf);
    grant_entities(&fixture);
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    create(
        &fixture.entity,
        "moqui.security.UserPermission",
        &[("userPermissionId", super::service::OPERATOR_PERMISSION)],
    );
    let mut grant = EntityValue::make(&fixture.entity, "moqui.security.UserGroupPermission").unwrap();
    grant.set("userGroupId", s("GIT_WRITERS"));
    grant.set("userPermissionId", s(super::service::OPERATOR_PERMISSION));
    grant.set(
        "fromDate",
        moqui_lib::object::basic_convert(&Value::I64(1_000), "Timestamp").value,
    );
    grant.create(&fixture.entity).unwrap();
    drop(_authz);
    drop(_entered);
    fixture
}

#[test]
fn the_pools_are_an_operators_and_a_registered_pool_is_one_owners() {
    let fixture = operator_fixture(
        "ci-safety-pools",
        r#"<git-ci run-check-service="org.example.check#Run"/>"#,
    );
    let store = format!("{SERVICES}.store#RunnerPool");
    let registered = |id: &str| {
        vec![
            ("runnerPoolId", s(id)),
            ("poolName", s("mac")),
            ("poolKind", s("registered")),
        ]
    };
    // not an operator: none of the pool services
    for (service, parameters) in [
        ("store#RunnerPool", registered("P1")),
        ("find#RunnerPools", vec![]),
        ("delete#RunnerPool", vec![("runnerPoolId", s("P1"))]),
        ("create#RunnerPoolToken", vec![("runnerPoolId", s("P1"))]),
        ("revoke#Runner", vec![("runnerId", s("R1"))]),
    ] {
        let (_, errors) = call(&fixture, READER, &format!("{SERVICES}.{service}"), &parameters);
        assert!(errors.contains("is an operator's"), "{service}: {errors}");
    }
    // on a server with tenants (a run check) a pool of registered runners has an owner
    let (_, errors) = call(&fixture, WRITER, &store, &registered("P1"));
    assert!(errors.contains("needs an ownerId"), "{errors}");
    let mut owned = registered("P1");
    owned.push(("ownerId", s("acme")));
    ok(&fixture, WRITER, &store, &owned);
    // never the host's network or another container's
    for rule in ["host", "container:db"] {
        let mut escaping = owned.clone();
        escaping.push(("egressRule", s(rule)));
        let (_, errors) = call(&fixture, WRITER, &store, &escaping);
        assert!(
            errors.contains("is the host's network or another container's"),
            "{rule}: {errors}"
        );
    }

    // a pool token expires; a new one with revokeRunners revokes the runners the old one registered
    let made = ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.create#RunnerPoolToken"),
        &[("runnerPoolId", s("P1")), ("validDays", Value::I64(2))],
    );
    let pool_token = to_plain_string(&made["token"]);
    let entity = fixture.entity.clone();
    let registered_runner = {
        let _entered = fixture.context.enter();
        assert_eq!(token::pool_of(&entity, &pool_token).unwrap().as_deref(), Some("P1"));
        registered::register(&entity, &pool_token, "mini", None)
            .unwrap()
            .unwrap()
    };
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        let mut pool = super::one(&entity, RUNNER_POOL, "runnerPoolId", "P1").unwrap().unwrap();
        pool.set(
            "registrationTokenExpireDate",
            moqui_core::user::stamp_value(super::now_millis() - 1),
        );
        pool.update(&entity).unwrap();
        assert_eq!(token::pool_of(&entity, &pool_token).unwrap(), None, "expired");
        assert!(token::runner_of(&entity, &registered_runner.1).unwrap().is_some());
    }
    ok(
        &fixture,
        WRITER,
        &format!("{SERVICES}.create#RunnerPoolToken"),
        &[("runnerPoolId", s("P1")), ("revokeRunners", Value::Bool(true))],
    );
    let _entered = fixture.context.enter();
    assert_eq!(
        token::runner_of(&entity, &registered_runner.1).unwrap(),
        None,
        "revoked"
    );
    assert_eq!(
        text(&one(&fixture, RUNNER, "runnerId", &registered_runner.0), "statusId"),
        "revoked"
    );
}

/// A run with one queued job `J1` in pool name `mac`, owned by `owner`.
fn queued_run(fixture: &Fixture, owner: Option<&str>) {
    let mut run = vec![
        ("pipelineRunId", s("R1")),
        ("repositoryId", s("app")),
        ("statusId", s("queued")),
        ("trusted", s("Y")),
        ("commitId", s("0123456789012345678901234567890123456789")),
    ];
    if let Some(owner) = owner {
        run.push(("ownerId", s(owner)));
    }
    row(fixture, PIPELINE_RUN, &run);
    row(
        fixture,
        JOB_RUN,
        &[
            ("jobRunId", s("J1")),
            ("pipelineRunId", s("R1")),
            ("repositoryId", s("app")),
            ("jobName", s("build")),
            ("poolName", s("mac")),
            ("image", s("macos")),
            ("statusId", s("queued")),
            ("queuedDate", moqui_core::user::now_value()),
            ("attempt", Value::I64(1)),
        ],
    );
}

fn runner(fixture: &Fixture, runner_id: &str, pool: &str) -> String {
    let runner_token = format!("{}{}", token::RUNNER_PREFIX, runner_id.to_lowercase().repeat(32));
    row(
        fixture,
        RUNNER,
        &[
            ("runnerId", s(runner_id)),
            ("runnerPoolId", s(pool)),
            ("tokenHash", s(&token::hash(&runner_token))),
            ("statusId", s("active")),
        ],
    );
    runner_token
}

fn registered_pool(fixture: &Fixture, id: &str, owner: Option<&str>) {
    let mut pool = vec![
        ("runnerPoolId", s(id)),
        ("poolName", s("mac")),
        ("poolKind", s("registered")),
        ("trusted", s("Y")),
        ("enabled", s("Y")),
        ("maxConcurrent", Value::I64(2)),
        ("allowedImages", s("*")),
    ];
    if let Some(owner) = owner {
        pool.push(("ownerId", s(owner)));
    }
    row(fixture, RUNNER_POOL, &pool);
}

#[test]
fn a_shared_pool_takes_no_job_of_an_owner_with_its_own_and_a_revoked_runner_loses_its_job() {
    let fixture = operator_fixture("ci-safety-owner", "");
    registered_pool(&fixture, "SHARED", None);
    registered_pool(&fixture, "OWN", Some("acme"));
    queued_run(&fixture, Some("acme"));
    let shared = runner(&fixture, "RS", "SHARED");
    let own = runner(&fixture, "RO", "OWN");
    let entity = fixture.entity.clone();
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    assert_eq!(
        registered::claim(&entity, &shared, false).unwrap(),
        None,
        "the owner has its own pool"
    );
    let claimed = registered::claim(&entity, &own, false)
        .unwrap()
        .expect("its own pool's runner");
    assert_eq!(claimed.job_run_id, "J1");
    // revoked: its token is refused and its job ends
    assert!(registered::revoke(&entity, "RO").unwrap());
    assert!(registered::claim(&entity, &own, false).is_err());
    let job = super::one(&entity, JOB_RUN, "jobRunId", "J1").unwrap().unwrap();
    assert_eq!(
        (text(&job, "statusId"), text(&job, "statusMessage")),
        ("error".into(), "The runner was revoked".into())
    );
}

#[test]
fn a_refused_or_never_approved_run_is_not_run_again() {
    let fixture = operator_fixture("ci-safety-retry", "");
    registered_pool(&fixture, "OWN", None);
    queued_run(&fixture, None);
    let entity = fixture.entity.clone();
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut job = super::one(&entity, JOB_RUN, "jobRunId", "J1").unwrap().unwrap();
    job.set("statusId", s("error"));
    job.set("runnerPoolId", s("OWN"));
    job.update(&entity).unwrap();
    for (state, refusal) in [("refused", "was refused"), ("waiting", "never approved")] {
        let mut run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
            .unwrap()
            .unwrap();
        run.set("statusId", s("error"));
        run.set("approvalState", s(state));
        run.update(&entity).unwrap();
        let refused = engine::retry_job(&entity, "J1").unwrap_err();
        assert!(refused.message().contains(refusal), "{refused}");
        let run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
            .unwrap()
            .unwrap();
        assert!(super::queue::check_again(&run).unwrap_err().contains(refusal));
    }
    // one that was allowed runs again, in the pool the attempt ran in
    let mut run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
        .unwrap()
        .unwrap();
    run.set("approvalState", s("none"));
    run.update(&entity).unwrap();
    assert!(super::queue::check_again(&run).is_ok());
    let next = engine::retry_job(&entity, "J1").unwrap();
    let next = super::one(&entity, JOB_RUN, "jobRunId", &next).unwrap().unwrap();
    assert_eq!(
        (text(&next, "statusId"), text(&next, "runnerPoolId")),
        ("queued".into(), "OWN".into())
    );
}

#[test]
fn what_a_repository_keeps_is_capped_and_its_caches_count() {
    let fixture = operator_fixture(
        "ci-safety-quota",
        r#"<git-ci max-repository-upload-bytes="100" max-artifacts="4"/>"#,
    );
    queued_run(&fixture, Some("acme"));
    let entity = fixture.entity.clone();
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
        .unwrap()
        .unwrap();
    run.set("protectedRef", s("Y"));
    run.update(&entity).unwrap();
    let job = super::one(&entity, JOB_RUN, "jobRunId", "J1").unwrap().unwrap();
    let upload = |kind: Kind, name: &str, bytes: &[u8]| {
        let key = format!("key-{name}");
        let upload = Upload {
            kind,
            name,
            path: None,
            cache_key: (kind == Kind::Cache).then_some(key.as_str()),
            retention_days: None,
        };
        artifact::store(&entity, &run, &job, &upload, &mut &bytes[..])
    };
    upload(Kind::Artifact, "a", &[1u8; 60]).unwrap();
    upload(Kind::Cache, "c", &[2u8; 30]).unwrap();
    // the cap: what is left is 10 bytes, so a larger upload is cut there and refused as the quota's
    let refused = upload(Kind::Artifact, "b", &[3u8; 50]).unwrap_err();
    assert!(refused.message().starts_with("GitCiStorageQuota: "), "{refused}");
    let usage = super::artifact::stored_by_kind(&entity, "repositoryId", "app").unwrap();
    assert_eq!(usage, (60, 30));
    assert_eq!(super::artifact::stored_bytes(&entity, "ownerId", "acme").unwrap(), 90);
    upload(Kind::Report, "r", &[4u8; 10]).unwrap();
    let refused = upload(Kind::Report, "r2", &[5u8; 1]).unwrap_err();
    assert!(refused.message().contains("GitCiStorageQuota"), "{refused}");
    // every upload of a job counts toward max-artifacts, its cache too
    {
        let mut capped = EntityValue::make(&entity, "moqui.resource.GitRunArtifact").unwrap();
        capped.set("artifactId", s("A-OTHER"));
        capped.set("repositoryId", s("elsewhere"));
        capped.set("jobRunId", s("J1"));
        capped.set("sizeBytes", Value::I64(0));
        capped.create(&entity).unwrap();
    }
    let refused = upload(Kind::Cache, "c2", &[6u8; 1]).unwrap_err();
    assert!(
        refused.message().contains("at most 4 artifacts, reports and caches"),
        "{refused}"
    );
}

#[test]
fn a_hosts_errors_are_its_answer_and_the_callers_earlier_ones_stay() {
    let fixture = operator_fixture("ci-safety-errors", "");
    let _entered = fixture.context.enter();
    ExecutionContext::with_current(|ec| ec.message().add_error("the caller's own".to_string()));
    let answered: Result<(), moqui_core::context::Error> = super::with_errors_taken(|| {
        ExecutionContext::with_current(|ec| ec.message().add_error("the host's".to_string()));
        Ok(())
    });
    assert_eq!(answered.unwrap_err().message(), "the host's");
    let left = ExecutionContext::with_current(|ec| ec.message().errors_text()).unwrap();
    assert_eq!(left, ["the caller's own"]);
}

#[test]
fn a_schedule_runs_at_most_once_in_its_minimum_interval() {
    let fixture = operator_fixture(
        "ci-safety-schedule",
        r#"<git-ci enabled="true" schedule-min-minutes="15"/>"#,
    );
    let now = super::now_millis();
    let provider = crate::provider(fixture.context.factory()).unwrap();
    let checked = |minutes_ago: i64| -> bool {
        let entity = fixture.entity.clone();
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        entity.find(super::queue::SCHEDULE).unwrap().delete_all().unwrap();
        let last = now - minutes_ago * 60_000;
        let mut schedule = EntityValue::make(&entity, super::queue::SCHEDULE).unwrap();
        schedule.set("repositoryId", s("nosuch"));
        schedule.set("pipelineName", s("nightly"));
        // a cron of every second: due at once, by the cron alone
        schedule.set("cron", s("* * * * * ?"));
        schedule.set("refName", s("refs/heads/main"));
        schedule.set("lastCheckDate", moqui_core::user::stamp_value(last));
        schedule.create(&entity).unwrap();
        super::queue::queue_due(&provider, &entity, now).unwrap();
        let schedule = super::one(&entity, super::queue::SCHEDULE, "repositoryId", "nosuch")
            .unwrap()
            .unwrap();
        super::millis(&schedule, "lastCheckDate") != Some(last)
    };
    assert!(!checked(5), "five minutes after its last run: not yet");
    assert!(checked(16), "past its fifteen minutes: due");
}

/// A pool token with no expiry (made before tokens expired) registers nothing; a runner in a pool with no owner on a
/// server with a run check gets no job; a retry twice makes one attempt; a runner left reserved for a job it is not
/// running (a claim that failed between the reservation and the job's claim) is freed at its next claim; and a job of
/// a run that ended is given to no runner.
#[test]
fn old_tokens_ownerless_pools_double_retries_and_ended_runs_get_nothing() {
    let fixture = operator_fixture(
        "ci-safety-claims",
        r#"<git-ci run-check-service="org.example.check#Run"/>"#,
    );
    registered_pool(&fixture, "SHARED", None);
    registered_pool(&fixture, "OWN", Some("acme"));
    queued_run(&fixture, Some("acme"));
    let shared = runner(&fixture, "RS", "SHARED");
    let own = runner(&fixture, "RO", "OWN");
    let entity = fixture.entity.clone();
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    // a token with no expiry date
    let pool_token = format!("{}{}", token::POOL_PREFIX, "ab".repeat(32));
    let mut pool = super::one(&entity, RUNNER_POOL, "runnerPoolId", "OWN")
        .unwrap()
        .unwrap();
    pool.set("registrationTokenHash", s(&token::hash(&pool_token)));
    pool.update(&entity).unwrap();
    assert_eq!(
        token::pool_of(&entity, &pool_token).unwrap(),
        None,
        "no expiry is expired"
    );
    // an ownerless pool of registered runners on a server with tenants
    let refused = registered::claim(&entity, &shared, false).unwrap_err();
    assert!(refused.message().contains("has no ownerId"), "{refused}");
    // the run ended (cancelled as the claim read its jobs): its job goes to no runner
    let mut run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
        .unwrap()
        .unwrap();
    run.set("statusId", s("cancelled"));
    run.update(&entity).unwrap();
    assert_eq!(registered::claim(&entity, &own, false).unwrap(), None);
    run.set("statusId", s("queued"));
    run.update(&entity).unwrap();
    // a runner left reserved for a job it never ran (still queued: the claim failed after the reservation) is freed
    // under the pool's lease, where no claim of it is in progress, and gets a job
    let mut taken = super::one(&entity, RUNNER, "runnerId", "RO").unwrap().unwrap();
    taken.set("currentJobRunId", s("J-ELSEWHERE"));
    taken.update(&entity).unwrap();
    row(
        &fixture,
        JOB_RUN,
        &[
            ("jobRunId", s("J-ELSEWHERE")),
            ("pipelineRunId", s("R-OTHER")),
            ("statusId", s("queued")),
        ],
    );
    let claimed = registered::claim(&entity, &own, false)
        .unwrap()
        .expect("freed and given a job");
    assert_eq!(claimed.job_run_id, "J1");
    assert_eq!(text(&one(&fixture, RUNNER, "runnerId", "RO"), "currentJobRunId"), "J1");

    // a retry twice: one attempt
    assert!(engine::finish_job(&entity, "J1", &engine::Outcome::error("failed".to_string())).unwrap());
    let mut run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
        .unwrap()
        .unwrap();
    run.set("statusId", s("error"));
    run.update(&entity).unwrap();
    engine::retry_job(&entity, "J1").unwrap();
    let again = engine::retry_job(&entity, "J1").unwrap_err();
    assert!(again.message().contains("was already run again"), "{again}");
    let attempts = entity
        .find(JOB_RUN)
        .unwrap()
        .eq("pipelineRunId", s("R1"))
        .count()
        .unwrap();
    assert_eq!(attempts, 2, "the first and one retry");
}

/// A cache written by a job that was given secrets (an environment whose allowedRefs names an unprotected branch) is of
/// the protected class, which no untrusted run reads.
#[test]
fn a_cache_of_a_job_given_secrets_is_protected() {
    let fixture = operator_fixture("ci-safety-cache-class", "");
    queued_run(&fixture, None);
    row(
        &fixture,
        super::CI_ENVIRONMENT,
        &[
            ("environmentId", s("E1")),
            ("repositoryId", s("app")),
            ("name", s("staging")),
            ("allowedRefs", s("feature-*")),
        ],
    );
    row(
        &fixture,
        super::CI_SECRET,
        &[
            ("secretId", s("S1")),
            ("repositoryId", s("app")),
            ("name", s("DEPLOY_KEY")),
            ("secretValue", s("k")),
            ("scope", s("environment")),
            ("environmentId", s("E1")),
        ],
    );
    let entity = fixture.entity.clone();
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let mut run = super::one(&entity, PIPELINE_RUN, "pipelineRunId", "R1")
        .unwrap()
        .unwrap();
    run.set("refName", s("refs/heads/feature-x"));
    run.set("event", s("push"));
    run.update(&entity).unwrap();
    let mut job = super::one(&entity, JOB_RUN, "jobRunId", "J1").unwrap().unwrap();
    job.set(
        "jobSpec",
        s(r#"{"environment":"staging","secrets":[{"name":"DEPLOY_KEY","env":"DEPLOY_KEY"}]}"#),
    );
    job.update(&entity).unwrap();
    let upload = Upload {
        kind: Kind::Cache,
        name: "deps",
        path: None,
        cache_key: Some("deps-1"),
        retention_days: None,
    };
    let id = artifact::store(&entity, &run, &job, &upload, &mut &b"cached"[..])
        .unwrap()
        .expect("stored");
    let stored = super::one(&entity, super::RUN_ARTIFACT, "artifactId", &id)
        .unwrap()
        .unwrap();
    assert_eq!(text(&stored, "protectedRef"), "Y", "the class of a run given secrets");
    let mut untrusted = run.clone();
    untrusted.set("trusted", s("N"));
    assert!(artifact::find_cache(&entity, &untrusted, "deps-1").unwrap().is_none());
}
