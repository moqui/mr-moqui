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

//! Claims across cluster members (docs/interfaces/Pipelines.md, "The engine"): two engines (threads on one database,
//! as two members are) racing for one job, and for the last place of a pool; the tokens of ended jobs; the URL a
//! container's runner is given.

use std::sync::{Arc, Barrier};

use moqui_core::entity::EntityValue;
use moqui_lib::object::Value;

use super::runner_http::{self, MemoryResponse, RunnerHttpRequest};
use super::{engine, registered, token, JOB_RUN, PIPELINE_RUN, RUNNER, RUNNER_POOL};
use crate::server::tests::{fixture_with, Fixture};

fn row(fixture: &Fixture, entity: &str, fields: &[(&str, Value)]) {
    let mut row = EntityValue::make(&fixture.entity, entity).unwrap();
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.create(&fixture.entity).unwrap();
}

fn s(text: &str) -> Value {
    Value::string(text)
}

/// A registered pool with room for one job, a trusted run with two queued jobs, and two runners of the pool.
fn claims_fixture(name: &str) -> Fixture {
    let fixture = fixture_with(name, moqui_core::context::framework_dir(), "");
    fill(&fixture);
    fixture
}

fn fill(fixture: &Fixture) {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    row(
        fixture,
        RUNNER_POOL,
        &[
            ("runnerPoolId", s("P1")),
            ("poolName", s("mac")),
            ("poolKind", s("registered")),
            ("trusted", s("Y")),
            ("enabled", s("Y")),
            ("maxConcurrent", Value::I64(1)),
            ("allowedImages", s("*")),
        ],
    );
    row(
        fixture,
        PIPELINE_RUN,
        &[
            ("pipelineRunId", s("R1")),
            ("repositoryId", s("app")),
            ("statusId", s("queued")),
            ("trusted", s("Y")),
            ("commitId", s("0123456789012345678901234567890123456789")),
        ],
    );
    for job in ["J1", "J2"] {
        row(
            fixture,
            JOB_RUN,
            &[
                ("jobRunId", s(job)),
                ("pipelineRunId", s("R1")),
                ("repositoryId", s("app")),
                ("jobName", s(job)),
                ("poolName", s("mac")),
                ("image", s("macos")),
                ("statusId", s("queued")),
                ("queuedDate", moqui_core::user::now_value()),
                ("attempt", Value::I64(1)),
            ],
        );
    }
    for runner in ["RA", "RB"] {
        row(
            fixture,
            RUNNER,
            &[
                ("runnerId", s(runner)),
                ("runnerPoolId", s("P1")),
                ("tokenHash", s(&token::hash(&runner_token(runner)))),
                ("statusId", s("active")),
            ],
        );
    }
}

fn runner_token(runner: &str) -> String {
    format!("{}{}", token::RUNNER_PREFIX, runner.repeat(32).to_lowercase())
}

/// Run `body` on `threads` threads at once, each in a context of its own with authorization off.
fn race<T: Send + 'static>(
    fixture: &Fixture,
    threads: usize,
    body: impl Fn(usize) -> T + Send + Sync + 'static,
) -> Vec<T> {
    let factory = Arc::clone(fixture.context.factory());
    let barrier = Arc::new(Barrier::new(threads));
    let body = Arc::new(body);
    let handles: Vec<_> = (0..threads)
        .map(|index| {
            let (factory, barrier, body) = (Arc::clone(&factory), Arc::clone(&barrier), Arc::clone(&body));
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                let _authz = context.disable_authz();
                barrier.wait();
                body(index)
            })
        })
        .collect();
    handles.into_iter().map(|handle| handle.join().unwrap()).collect()
}

fn status_of(fixture: &Fixture, job: &str) -> String {
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    super::one(&fixture.entity, JOB_RUN, "jobRunId", job)
        .unwrap()
        .and_then(|row| super::field(&row, "statusId"))
        .unwrap_or_default()
}

#[test]
fn of_engines_racing_for_one_queued_job_exactly_one_starts_it() {
    let fixture = claims_fixture("ci-claim-one");
    let (run, job, pool) = {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        (
            super::one(&fixture.entity, PIPELINE_RUN, "pipelineRunId", "R1")
                .unwrap()
                .unwrap(),
            super::one(&fixture.entity, JOB_RUN, "jobRunId", "J1").unwrap().unwrap(),
            super::one(&fixture.entity, RUNNER_POOL, "runnerPoolId", "P1")
                .unwrap()
                .unwrap(),
        )
    };
    // the test facade writes no audit log unless asked
    fixture.entity.set_disable_audit_log(false);
    let entity = fixture.entity.clone();
    let tokens = race(&fixture, 4, move |_| {
        engine::claim_job(&entity, &run, &job, &pool, None).unwrap()
    });
    assert_eq!(tokens.iter().flatten().count(), 1, "{tokens:?}");
    assert_eq!(status_of(&fixture, "J1"), "running");
    // the move is in the status's audit log once
    let _entered = fixture.context.enter();
    let _authz = fixture.context.disable_authz();
    let audits = fixture
        .entity
        .find("moqui.entity.EntityAuditLog")
        .unwrap()
        .eq("changedEntityName", s(JOB_RUN))
        .eq("pkPrimaryValue", s("J1"))
        .list()
        .unwrap();
    assert_eq!(audits.len(), 1);
    assert_eq!(
        super::field(&audits.iter().next().unwrap().clone(), "newValueText").as_deref(),
        Some("running")
    );
}

#[test]
fn two_runners_of_a_pool_with_room_for_one_job_get_one_between_them() {
    let fixture = claims_fixture("ci-claim-pool");
    let entity = fixture.entity.clone();
    let claimed = race(&fixture, 2, move |index| {
        registered::claim(&entity, &runner_token(["RA", "RB"][index])).unwrap()
    });
    assert_eq!(claimed.iter().flatten().count(), 1, "{claimed:?}");
    let running: Vec<String> = ["J1", "J2"]
        .iter()
        .filter(|job| status_of(&fixture, job) == "running")
        .map(|job| (*job).to_string())
        .collect();
    assert_eq!(running.len(), 1, "{running:?}");
}

#[test]
fn the_token_of_an_ended_job_is_gone_and_an_unknown_one_is_not_valid() {
    let fixture = claims_fixture("ci-claim-gone");
    let ended = format!("{}{}", token::JOB_PREFIX, "ab".repeat(32));
    let overdue = format!("{}{}", token::JOB_PREFIX, "cd".repeat(32));
    {
        let _entered = fixture.context.enter();
        let _authz = fixture.context.disable_authz();
        for (job, status, token, expires) in [
            ("J1", "cancelled", &ended, crate::ci::now_millis() + 60_000),
            ("J2", "running", &overdue, crate::ci::now_millis() - 1_000),
        ] {
            let mut row = super::one(&fixture.entity, JOB_RUN, "jobRunId", job).unwrap().unwrap();
            row.set("statusId", s(status));
            row.set("tokenHash", s(&token::hash(token)));
            row.set("tokenExpireDate", moqui_core::user::stamp_value(expires));
            row.update(&fixture.entity).unwrap();
        }
    }
    let get = |job: &str, bearer: &str| -> u16 {
        let request = RunnerHttpRequest {
            method: "GET".into(),
            path: format!("jobs/{job}/job"),
            query: String::new(),
            authorization: Some(format!("Bearer {bearer}")),
            content_length: None,
            secure: true,
            client_ip: "10.0.0.9".into(),
        };
        let _entered = fixture.context.enter();
        let mut response = MemoryResponse::default();
        runner_http::serve(&request, &mut std::io::empty(), &mut response);
        response.status
    };
    assert_eq!(get("J1", &ended), 410, "cancelled");
    assert_eq!(get("J2", &overdue), 410, "past its time");
    assert_eq!(
        get("J1", &format!("{}{}", token::JOB_PREFIX, "ef".repeat(32))),
        401,
        "not this server's"
    );
    assert_eq!(get("J1", "mqcj_short"), 401, "malformed");
    assert!(!token::well_formed("mqcj_short", token::JOB_PREFIX));
    assert!(!token::well_formed(
        &format!("{}{}", token::JOB_PREFIX, "AB".repeat(32)),
        token::JOB_PREFIX
    ));
}

#[test]
fn a_container_runner_is_given_an_https_url_or_a_loopback_one() {
    let fixture = claims_fixture("ci-claim-url");
    let _entered = fixture.context.enter();
    let pool = |url: &str| {
        let mut pool = EntityValue::make(&fixture.entity, RUNNER_POOL).unwrap();
        pool.set("poolName", s("linux"));
        pool.set("serverUrl", s(url));
        pool
    };
    assert_eq!(
        engine::runner_server_url(&pool("https://moqui.example.com/")).unwrap(),
        "https://moqui.example.com"
    );
    assert!(engine::runner_server_url(&pool("http://127.0.0.1:8080")).is_ok());
    assert!(engine::runner_server_url(&pool("http://[::1]:8080")).is_ok());
    assert!(engine::runner_server_url(&pool("http://localhost")).is_ok());
    let refused = engine::runner_server_url(&pool("http://moqui.example.com:8080")).unwrap_err();
    assert!(
        refused.contains("git-ci server-url") && refused.contains("git_public_url") && refused.contains("linux"),
        "{refused}"
    );
}
