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

//! The harness without a run: the spec prelude's plan and checks in the script engine, recordings and drift, the
//! measures and the baseline, and the endpoint probe a live run is skipped by.

use std::collections::BTreeMap;

use serde_json::json;

use super::metrics::{group, regressions, summarize, TaskMetrics, TaskOutcome, TaskResult};
use super::observe::{held_transition, is_error_text, ModelReply, ReplyCall};
use super::profile::endpoint_answers;
use super::spec;
use super::transcript::{drift, normalize, reply_of, Recording};
use crate::context::ExecutionContextFactory;
use crate::llm::conf::{EndpointConf, ProtocolId};
use crate::llm::provider::fake::Step;
use crate::testkit::SpecFile;

const SPEC: &str = r#"
agentSpec({
    agent: "assist",
    user: "john.doe",
    tasks: [
        {
            name: "Open the list",
            start: "/apps/system/Visit/VisitList?initialRequest=%2Fx",
            say: "Show the visits",
            approvals: ["approve", { edit: { description: "B" } }, { reject: "not now" }],
            expect: {
                path: "/apps/system/Visit/VisitList",
                parameters: { initialRequest: /^\/x/ },
                values: { description: "A" },
                answer: [/visits/i],
                answerNot: /deleted/,
                called: ["screen", { tool: "data", args: { where: { status: "Open" } } }],
                notCalled: [{ tool: "screen", args: { do: /.+/ } }],
                results: [{ tool: "data", result: /showing 2 of 2/ }],
                held: 0,
                noWrite: true,
                maxWrongCalls: 0,
            },
            verify(run) { assert(run.calls.length === 2, "two calls") },
        },
        {
            name: "Two turns",
            turns: [
                { say: "first", expect: { called: ["skill"] } },
                { say: "second", expect: { notCalled: ["skill"], held: { transition: "updateThing" } } },
            ],
        },
    ],
})
"#;

fn with_spec<T>(source: &str, body: impl FnOnce(Result<spec::Plan, String>) -> T) -> T {
    crate::script::clear_context_stack();
    let context = ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let _entered = context.enter();
    let file = SpecFile {
        name: "sample".to_string(),
        display: "c/test/agent/sample.agent.js".to_string(),
        source: source.to_string(),
    };
    body(spec::load(&file))
}

fn call(tool: &str, arguments: serde_json::Value, result: &str) -> serde_json::Value {
    json!({"tool": tool, "arguments": arguments, "status": "done", "result": result, "error": false,
        "classification": "read", "repaired": false})
}

#[test]
fn a_spec_plans_its_tasks_and_turns() {
    with_spec(SPEC, |plan| {
        let plan = plan.unwrap();
        assert_eq!(plan.spec.agent, "assist");
        assert_eq!(plan.spec.surface, "drawer");
        assert_eq!(plan.tasks.len(), 2);
        let first = &plan.tasks[0];
        assert_eq!(first.slug, "open-the-list");
        assert_eq!(first.start.as_ref().unwrap().path, "/apps/system/Visit/VisitList?initialRequest=%2Fx");
        assert_eq!(first.turns.len(), 1);
        assert_eq!(first.expect_held, Some(0));
        let kinds: Vec<&str> = first.approvals.iter().map(|answer| answer.kind.as_str()).collect();
        assert_eq!(kinds, ["approve", "edit", "reject"]);
        assert_eq!(first.approvals[1].values["description"], "B");
        assert_eq!(first.approvals[2].feedback, "not now");
        let second = &plan.tasks[1];
        assert_eq!(second.turns.iter().map(|turn| turn.say.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        assert_eq!(second.expect_held, None);
    });
}

#[test]
fn a_spec_with_mistakes_does_not_load() {
    with_spec(
        r#"agentSpec({ tasks: [{ name: "a", say: "x", expect: { pathh: "/" } }, { name: "a", turns: [{}] }, { say: "y", judge: true }] })"#,
        |plan| {
            let error = plan.unwrap_err();
            assert!(error.contains("expect has an unknown field pathh"), "{error}");
            assert!(error.contains("another task has the same name"), "{error}");
            assert!(error.contains("turn 1 needs say"), "{error}");
            assert!(error.contains("needs a name"), "{error}");
            assert!(error.contains("the LLM judge is not built"), "{error}");
        },
    );
    with_spec("const x = 1", |plan| assert!(plan.unwrap_err().contains("does not call agentSpec()")));
}

#[test]
fn the_checks_read_what_the_run_did() {
    with_spec(SPEC, |plan| {
        plan.unwrap();
        let good = json!({"turns": [{
            "say": "Show the visits", "state": "Completed", "answer": "Two visits.", "held": [], "questions": [],
            "screen": {"path": "/apps/system/Visit/VisitList", "parameters": {"initialRequest": "/x/1"},
                "values": {"VisitFind": {"description": "A"}}},
            "calls": [call("screen", json!({"go": "/apps/system/Visit/VisitList"}), "--- ok"),
                call("data", json!({"path": "/apps/system/Visit/VisitList", "where": {"status": "Open", "x": 1}}),
                    "VisitList (showing 2 of 2)")],
        }]});
        assert_eq!(spec::check(0, &good), Vec::<String>::new());

        let bad = json!({"turns": [{
            "say": "Show the visits", "state": "Completed", "answer": "I deleted them.",
            "held": [{"tool": "screen", "transition": "deleteVisit", "text": "approval pending: deleteVisit"}],
            "questions": [],
            "screen": {"path": "/apps/system/Visit/VisitDetail", "parameters": {}, "values": {}},
            "calls": [call("screen", json!({"do": "Delete"}), "approval pending: deleteVisit")],
        }]});
        let failures = spec::check(0, &bad).join("\n");
        for expected in [
            "the screen is \"/apps/system/Visit/VisitDetail\"",
            "screen parameter initialRequest is nothing",
            "no form has description \"A\"",
            "the answer does not match /visits/i",
            "the answer matches /deleted/",
            "no call matches {\"tool\":\"data\"",
            "which should not be called",
            "no result of data matches /showing 2 of 2/",
            "1 write(s) held for approval, expected 0",
            "a write was attempted: [\"deleteVisit\"]",
            "a call matches {\"tool\":\"screen\",\"args\":{\"do\":\"/.+/\"}}",
            "verify: Assertion failed: two calls",
        ] {
            assert!(failures.contains(expected), "{expected} in:\n{failures}");
        }

        let turns = json!({"turns": [
            {"answer": "", "calls": [call("data", json!({}), "")], "held": []},
            {"answer": "", "calls": [call("skill", json!({"name": "x"}), "")],
                "held": [{"transition": "updateThing", "text": ""}]},
        ]});
        let failures = spec::check(1, &turns);
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(failures[0].starts_with("turn 1: no call matches \"skill\""), "{failures:?}");
        assert!(failures[1].starts_with("turn 2: a call matches \"skill\""), "{failures:?}");
    });
}

#[test]
fn a_recording_plays_its_replies_and_its_results_show_drift() {
    let text = [
        json!({"type": "header", "model": {"promptVariant": "small"}}).to_string(),
        json!({"type": "user", "turn": 0, "text": "hi"}).to_string(),
        json!({"type": "model", "turn": 0, "step": 0, "text": "Looking.", "toolCalls": [
            {"id": "c1", "name": "screen", "arguments": {"go": "/apps"}},
            {"id": "c2", "name": "data", "raw": "{bad"}]}).to_string(),
        json!({"type": "tool", "name": "screen", "result": "path /apps\n john.doe  Wed 22:49\n--- ok"}).to_string(),
        json!({"type": "tool", "name": "data", "result": "Error: bad"}).to_string(),
        json!({"type": "model", "turn": 0, "step": 1, "text": "Done."}).to_string(),
    ]
    .join("\n");
    let recording = Recording::parse(&text).unwrap();
    assert_eq!(recording.model["promptVariant"], "small");
    assert_eq!(recording.replies.len(), 2);
    assert_eq!(recording.results.len(), 2);
    let first = reply_of(&recording.replies[0]);
    assert_eq!(first.steps[0], Step::Text("Looking.".to_string()));
    assert!(matches!(&first.steps[1], Step::Tool { id, arguments, .. } if id == "c1" && arguments == r#"{"go":"/apps"}"#));
    assert!(matches!(&first.steps[2], Step::Tool { arguments, .. } if arguments == "{bad"));
    assert_eq!(first.steps[3], Step::Finish("tool_calls".to_string()));
    assert_eq!(reply_of(&recording.replies[1]).steps.last(), Some(&Step::Finish("stop".to_string())));
    assert!(Recording::parse(&json!({"type": "header"}).to_string()).is_err(), "no replies");

    // the clock, dates and ids are not drift; other text is
    let now = vec![
        ("screen".to_string(), "path /apps\n john.doe  Thu 09:05\n--- ok".to_string()),
        ("data".to_string(), "Error: worse".to_string()),
    ];
    let lines = drift(&recording.results, &now);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("result 2 (data) changed"), "{lines:?}");
    assert_eq!(drift(&recording.results, &now[..1]).len(), 1, "a call fewer");
    assert!(drift(&[], &now).is_empty(), "nothing recorded, nothing compared");
    assert_eq!(
        normalize("V 2026-10-20 22:49 | id 100023 | 7 of 12 | at 9:05, 10.0.0.7"),
        "V <date> | id # | 7 of 12 | at <time>, 10.0.0.7"
    );
}

#[test]
fn results_are_read_for_held_writes_and_errors() {
    assert_eq!(
        held_transition("path /x\napproval pending: updateUserGroup waits for the user's approval\n--- ok").as_deref(),
        Some("updateUserGroup")
    );
    assert_eq!(held_transition("--- ok"), None);
    assert!(is_error_text(r#"{"error":"unknown_skill","instruction":"…"}"#));
    assert!(is_error_text("Error: the tool failed"));
    assert!(is_error_text("path /x\n screen text\n--- error no element named \"Find\"; see elements"));
    assert!(!is_error_text("path /x\nerror updateUserGroup waits\n--- ok"));
    let call = ReplyCall {
        id: "c".to_string(),
        name: "screen".to_string(),
        arguments: json!({}),
        raw: None,
    };
    let reply = ModelReply {
        tool_calls: vec![call],
        ..ModelReply::default()
    };
    assert_eq!(serde_json::to_value(&reply).unwrap()["toolCalls"][0]["name"], "screen");
}

fn result(task: &str, run: u32, passed: bool, tokens: u64) -> TaskResult {
    TaskResult {
        spec: "s".to_string(),
        task: task.to_string(),
        slug: task.to_string(),
        run,
        outcome: if passed {
            TaskOutcome::Passed
        } else {
            TaskOutcome::Failed(vec!["no".to_string()])
        },
        metrics: TaskMetrics {
            model_calls: 2,
            tool_calls: 1,
            prompt_tokens: tokens,
            cached_tokens: tokens / 2,
            completion_tokens: 0,
            held: 1,
            held_expected: Some(1),
            ..TaskMetrics::default()
        },
        notes: Vec::new(),
        transcript: None,
    }
}

#[test]
fn the_summary_measures_pass_rates_and_flags_regressions() {
    let results = vec![
        result("a", 1, true, 1000),
        result("a", 2, true, 1000),
        result("b", 1, true, 2000),
        result("b", 2, false, 2000),
        TaskResult {
            outcome: TaskOutcome::Skipped("no endpoint".to_string()),
            ..result("c", 1, true, 0)
        },
    ];
    let summary = summarize("local-qwen", 2, &results);
    assert_eq!(summary.tasks_run, 2);
    assert_eq!(summary.skipped, 1);
    assert!((summary.pass_at1 - 0.75).abs() < 1e-9, "{}", summary.pass_at1);
    assert!((summary.pass_all - 0.5).abs() < 1e-9);
    assert_eq!(summary.tasks_passed, 1);
    assert_eq!(summary.tasks["s/a"].tokens, 1000);
    assert!((summary.cache_hit - 0.5).abs() < 1e-9);
    assert_eq!((summary.approvals_as_expected, summary.approvals_expected), (4, 4));
    assert!(summary.text("tools").contains("pass@1 0.75  pass^2 0.50  1 of 2 tasks passed every run"));

    let baseline = summary.baseline();
    assert!(regressions(&summary, &baseline).is_empty(), "a run is no regression of itself");
    let mut worse = baseline.clone();
    worse["tasks"]["s/b"]["passAt1"] = json!(1.0);
    worse["tasks"]["s/a"]["tokens"] = json!(800);
    let found = regressions(&summary, &worse);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().any(|line| line.starts_with("s/b: success 0.50, the baseline's 1.00")), "{found:?}");
    assert!(found.iter().any(|line| line.starts_with("s/a: 1,000 tokens a run, the baseline's 800 (+25%)")), "{found:?}");
    let mut fewer: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    fewer.insert("s/a".to_string(), json!({"passAt1": 1.0, "tokens": 1100}));
    assert!(regressions(&summary, &json!({"tasks": fewer})).is_empty(), "within 20% and as good");
    assert_eq!(group(1_234_567), "1,234,567");
}

#[test]
fn an_endpoint_that_does_not_answer_is_found_before_the_run() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let open = EndpointConf::new("up", ProtocolId::ChatCompletions, format!("http://127.0.0.1:{port}/v1"));
    assert_eq!(endpoint_answers(&open), Ok(()));
    drop(listener);
    let closed = EndpointConf::new("down", ProtocolId::ChatCompletions, format!("http://127.0.0.1:{port}"));
    let reason = endpoint_answers(&closed).unwrap_err();
    assert!(reason.starts_with(&format!("endpoint down at http://127.0.0.1:{port} does not answer")), "{reason}");
    assert!(endpoint_answers(&EndpointConf::new("f", ProtocolId::Fake, "")).is_ok(), "the fake always answers");
    assert!(endpoint_answers(&EndpointConf::new("x", ProtocolId::ChatCompletions, "localhost")).is_err());
}
