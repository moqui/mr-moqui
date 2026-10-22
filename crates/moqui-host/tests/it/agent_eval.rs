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

//! The agent evaluation harness (plans/LLM-A2A-Assist.md 9, `moqui test --agents`) on a seeded server on embedded
//! Postgres: the tools component's 24 tasks (`assist.agent.js` in the drawer, `headless.agent.js` on the a2a surface)
//! pass on their recordings with no model, and the JSON tool mode set of a small model (`replay-small/`, 9 of them)
//! passes with the others skipped; a live profile runs through the normal provider path, on the scripted fake and on a Chat
//! Completions endpoint (a loopback server that answers one stream and keeps the request, so the seed is seen in the
//! body), with the feature binding put back after; a profile whose endpoint does not answer is skipped, not failed. A user with only `AgentUse` runs an agent end to
//! end: the calls, the run's rows, a question, a spilled result read back and the screen state are written for them.

#![cfg(all(
    unix,
    feature = "embedded-postgres",
    feature = "axum",
    feature = "llm",
    feature = "tui"
))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm::provider::fake::{self, Reply, Step};
use moqui_core::testkit::agents::{evaluate, AgentSpecFile, EvalOptions, Mode, TaskOutcome};
use moqui_core::testkit::SpecFile;
use moqui_lib::object::{to_plain_string, Value};

use crate::common::{start_with_data, temp_dir};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="eval-fake" protocol="fake" description="Eval fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="eval-fake-model" endpointId="eval-fake" providerModel="fake-1" tier="fast"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="eval-fake-model" reasoningLevel="low"/>
</entity-facade-xml>"#;

/// A live task: the agent opens a screen and answers.
const LIVE_SPEC: &str = r#"
agentSpec({
    agent: "assist",
    user: "john.doe",
    tasks: [{
        name: "ping",
        start: "/apps/system/dashboard",
        say: "Say pong.",
        expect: { answer: /pong/, noWrite: true },
    }],
})
"#;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn options(mode: Mode, out: &std::path::Path) -> EvalOptions {
    EvalOptions {
        mode,
        runs: 1,
        seed: None,
        task_filter: None,
        out_dir: out.to_path_buf(),
    }
}

fn live_spec() -> Vec<AgentSpecFile> {
    vec![AgentSpecFile {
        component: "test".to_string(),
        file: SpecFile {
            name: "live".to_string(),
            display: "test/agent/live.agent.js".to_string(),
            source: LIVE_SPEC.to_string(),
        },
    }]
}

/// Write an endpoint and a model row.
fn model_rows(factory: &Arc<ExecutionContextFactory>, endpoint: &str, protocol: &str, url: &str, model: &str) {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let mut row = facade.make_value("moqui.llm.LlmEndpoint").unwrap();
    for (field, value) in [
        ("endpointId", endpoint),
        ("protocol", protocol),
        ("baseUrl", url),
        ("isLocal", "Y"),
    ] {
        row.set(field, Value::string(value));
    }
    row.create_or_update(&facade).unwrap();
    let mut row = facade.make_value("moqui.llm.LlmModel").unwrap();
    for (field, value) in [("modelId", model), ("endpointId", endpoint), ("providerModel", "mini"), ("tier", "fast")] {
        row.set(field, Value::string(value));
    }
    row.create_or_update(&facade).unwrap();
}

fn binding_model(factory: &Arc<ExecutionContextFactory>) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find("moqui.llm.LlmFeatureBinding")
        .unwrap()
        .eq("featureId", Value::string("assist"))
        .one()
        .unwrap()
        .map(|row| to_plain_string(row.get("modelId").unwrap()))
        .unwrap_or_default()
}

#[test]
fn the_agent_tasks_replay_with_no_model_and_run_live_or_skip() {
    let gate = start_with_data("agent-eval", DATA);
    let factory = Arc::clone(gate.server.factory());
    let out = temp_dir("agent-eval-out");
    replayed(&factory, &out);
    live_on_the_fake(&factory, &out);
    live_on_chat_completions(&factory, &out);
    skipped_when_down(&factory, &out);
    a_plain_user_runs_an_agent(&factory);
    let _ = std::fs::remove_dir_all(&out);
}

fn tools_specs(dir: &std::path::Path) -> Vec<AgentSpecFile> {
    ["assist", "headless"]
        .into_iter()
        .map(|name| AgentSpecFile {
            component: "tools".to_string(),
            file: SpecFile {
                name: name.to_string(),
                display: format!("tools/test/agent/{name}.agent.js"),
                source: std::fs::read_to_string(dir.join(format!("{name}.agent.js"))).unwrap(),
            },
        })
        .collect()
}

/// The committed recordings of the tools component pass with no model: every task of both specs, each a transcript;
/// then the JSON tool mode set, whose tasks without a recording are skipped.
fn replayed(factory: &Arc<ExecutionContextFactory>, out: &std::path::Path) {
    let dir = repo().join("runtime/base-component/tools/test/agent");
    let specs = tools_specs(&dir);
    let mode = Mode::Replay {
        dir: dir.join("replay"),
    };
    let evaluation = evaluate(factory, &specs, &options(mode, out), &mut |_| {}).unwrap();
    assert!(evaluation.skipped.is_none());
    assert_eq!(evaluation.results.len(), 24, "21 drawer tasks and 3 headless ones");
    for result in &evaluation.results {
        assert_eq!(result.outcome, TaskOutcome::Passed, "{}: {:?}", result.task, result.notes);
        let transcript = result.transcript.as_ref().unwrap();
        assert!(transcript.starts_with(out.join(&result.spec)), "{}", transcript.display());
        let text = std::fs::read_to_string(transcript).unwrap();
        assert!(text.lines().last().unwrap().contains(r#""passed":true"#), "{text}");
    }
    assert_eq!(evaluation.results.iter().filter(|result| result.spec == "headless").count(), 3);
    let held: u64 = evaluation.results.iter().map(|result| result.metrics.held).sum();
    assert_eq!(held, 5, "the approve, edit, reject, service tool and headless write tasks each ask for one approval");
    let invented: u64 = evaluation.results.iter().map(|result| result.metrics.invented).sum();
    assert_eq!(invented, 0);
    assert_eq!(binding_model(factory), "eval-fake-model", "the replay's binding is put back");

    // the JSON tool mode set: 9 tasks recorded as protocol objects, the rest skipped
    let small = dir.join("replay-small");
    let evaluation = evaluate(factory, &specs, &options(Mode::Replay { dir: small.clone() }, out), &mut |_| {}).unwrap();
    assert_eq!(moqui_core::testkit::agents::replay_label(&small), "replay-small");
    let passed: Vec<&str> = evaluation
        .results
        .iter()
        .filter(|result| result.outcome == TaskOutcome::Passed)
        .map(|result| result.slug.as_str())
        .collect();
    let failed: Vec<String> = evaluation
        .results
        .iter()
        .filter(|result| matches!(result.outcome, TaskOutcome::Failed(_)))
        .map(|result| format!("{}: {:?} {:?}", result.task, result.outcome, result.notes))
        .collect();
    assert_eq!(passed.len(), 9, "{passed:?} failed: {failed:#?}");
    for result in &evaluation.results {
        assert!(
            matches!(result.outcome, TaskOutcome::Passed | TaskOutcome::Skipped(_)),
            "{}: {:?} {:?}",
            result.task,
            result.outcome,
            result.notes
        );
        if result.outcome == TaskOutcome::Passed {
            let text = std::fs::read_to_string(result.transcript.as_ref().unwrap()).unwrap();
            let header: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
            assert_eq!(header["model"]["toolMode"], "json", "{}", result.task);
            // every reply is played as the text the model wrote
            assert!(text.contains(r#""raw":""#), "{text}");
        }
    }
    let held: u64 = evaluation.results.iter().map(|result| result.metrics.held).sum();
    assert_eq!(held, 1, "multi-step with approval");
    assert_eq!(binding_model(factory), "eval-fake-model");
}

/// A live profile on the scripted fake: the run goes through the provider, the checks see the screen call.
fn live_on_the_fake(factory: &Arc<ExecutionContextFactory>, out: &std::path::Path) {
    fake::clear("eval-fake");
    fake::script(
        "eval-fake",
        vec![
            Reply::steps(vec![
                Step::Tool {
                    id: "s1".to_string(),
                    name: "screen".to_string(),
                    arguments: r#"{"go": "/apps/system/dashboard"}"#.to_string(),
                    pieces: 2,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            Reply::text("pong"),
        ],
    );
    let mode = Mode::Live {
        profile: "eval-fake-model".to_string(),
    };
    let evaluation = evaluate(factory, &live_spec(), &options(mode, out), &mut |_| {}).unwrap();
    assert!(evaluation.skipped.is_none());
    let result = &evaluation.results[0];
    assert_eq!(result.outcome, TaskOutcome::Passed, "{:?}", result.notes);
    assert_eq!((result.metrics.model_calls, result.metrics.tool_calls), (2, 1));
    assert!(result.metrics.prompt_tokens > 0);
    let text = std::fs::read_to_string(result.transcript.as_ref().unwrap()).unwrap();
    let header: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(header["profile"], "eval-fake-model");
    assert_eq!(header["model"]["modelId"], "eval-fake-model");
    assert_eq!(fake::requests("eval-fake").len(), 2);
}

/// A live profile on a Chat Completions endpoint: the request carries the seed, the answer comes from its stream, and
/// the model row the seed went in is put back.
fn live_on_chat_completions(factory: &Arc<ExecutionContextFactory>, out: &std::path::Path) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            seen.lock().unwrap().push(String::from_utf8_lossy(&body).into_owned());
            let events = [
                r#"{"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":"pong"},"finish_reason":null}]}"#,
                r#"{"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":120,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":100}}}"#,
                "[DONE]",
            ];
            let mut response =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".to_string();
            for event in events {
                response.push_str(&format!("data: {event}\n\n"));
            }
            let mut stream = stream;
            let _ = stream.write_all(response.as_bytes());
        }
    });
    model_rows(factory, "eval-cc", "chat-completions", &format!("http://127.0.0.1:{port}"), "eval-cc-model");
    let mut options = options(
        Mode::Live {
            profile: "eval-cc-model".to_string(),
        },
        out,
    );
    options.seed = Some(7);
    let evaluation = evaluate(factory, &live_spec(), &options, &mut |_| {}).unwrap();
    assert!(evaluation.skipped.is_none());
    let result = &evaluation.results[0];
    assert_eq!(result.outcome, TaskOutcome::Passed, "{:?} {:?}", result.outcome, result.notes);
    assert_eq!(result.metrics.cached_tokens, 100);
    // the probe's connection sends nothing
    let bodies: Vec<String> = bodies.lock().unwrap().iter().filter(|body| !body.is_empty()).cloned().collect();
    assert_eq!(bodies.len(), 1, "one model call");
    let body: serde_json::Value = serde_json::from_str(&bodies[0]).unwrap();
    assert_eq!(body["seed"], 7, "the seed of run 1");
    assert_eq!(body["model"], "mini");
    assert!(body["tools"].as_array().is_some_and(|tools| !tools.is_empty()), "the agent's tools are offered");

    // the seed's model row and the binding are as they were
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let model = facade
        .find("moqui.llm.LlmModel")
        .unwrap()
        .eq("modelId", Value::string("eval-cc-model"))
        .one()
        .unwrap()
        .unwrap();
    assert!(model.get("apiConfigJson").is_none_or(|value| value.is_null()), "{model:?}");
    drop(_authz);
    assert_eq!(binding_model(factory), "eval-fake-model");
}

/// A profile whose endpoint takes no connection: the run is skipped with the reason, nothing fails.
fn skipped_when_down(factory: &Arc<ExecutionContextFactory>, out: &std::path::Path) {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    model_rows(factory, "eval-down", "chat-completions", &format!("http://127.0.0.1:{port}"), "eval-down-model");
    let mode = Mode::Live {
        profile: "eval-down-model".to_string(),
    };
    let evaluation = evaluate(factory, &live_spec(), &options(mode, out), &mut |_| {}).unwrap();
    let reason = evaluation.skipped.expect("skipped");
    assert!(reason.contains("does not answer"), "{reason}");
    assert!(evaluation.results.is_empty());
    let unknown = evaluate(
        factory,
        &live_spec(),
        &options(
            Mode::Live {
                profile: "no-such-model".to_string(),
            },
            out,
        ),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("there is no model no-such-model"), "{unknown}");
}

fn rows(factory: &Arc<ExecutionContextFactory>, entity: &str, field: &str, value: &str) -> Vec<moqui_core::entity::EntityValue> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find(entity)
        .unwrap()
        .eq(field, Value::string(value))
        .list()
        .unwrap()
        .values()
        .to_vec()
}

fn text(row: &moqui_core::entity::EntityValue, field: &str) -> String {
    row.get(field).map(to_plain_string).unwrap_or_default()
}

/// A user whose only permission is `AgentUse`: a run with a screen, a data read spilled past the model's cap, a
/// question, and a second turn reading the spilled artifact ends Completed, and every row of it is written.
fn a_plain_user_runs_an_agent(factory: &Arc<ExecutionContextFactory>) {
    use moqui_core::llm::run::{start_run, submit_input, wait_for, RunState, StartRequest};
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let rows: [(&str, &[(&str, &str)]); 5] = [
            ("moqui.security.UserAccount", &[("userId", "AGENT_PLAIN"), ("username", "agent.plain"), ("userFullName", "Agent Plain")]),
            ("moqui.security.UserGroup", &[("userGroupId", "AGENT_PLAIN_GROUP"), ("description", "Agents only")]),
            ("moqui.security.UserGroupMember", &[("userGroupId", "AGENT_PLAIN_GROUP"), ("userId", "AGENT_PLAIN"), ("fromDate", "0")]),
            ("moqui.security.UserGroupPermission", &[("userGroupId", "AGENT_PLAIN_GROUP"), ("userPermissionId", "AgentUse"), ("fromDate", "0")]),
            // a small result cap, so the data read spills
            ("moqui.llm.LlmModel", &[("modelId", "eval-fake-model"), ("resultCapChars", "300")]),
        ];
        for (entity, fields) in rows {
            let mut row = facade.make_value(entity).unwrap();
            for &(field, value) in fields {
                let value = if field == "fromDate" {
                    Value::Timestamp(jiff::Timestamp::UNIX_EPOCH)
                } else {
                    Value::string(value)
                };
                row.set(field, value);
            }
            row.create_or_update(&facade).unwrap();
        }
    }
    let tool = |id: &str, name: &str, arguments: serde_json::Value| Step::Tool {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
        pieces: 1,
    };
    fake::clear("eval-fake");
    fake::script(
        "eval-fake",
        vec![
            Reply::steps(vec![
                tool("p1", "screen", serde_json::json!({"go": "/apps"})),
                tool("p2", "data", serde_json::json!({"entity": "moqui.basic.Enumeration", "where": {"enumTypeId": "AgentSurface"}})),
                Step::Finish("tool_calls".to_string()),
            ]),
            Reply::steps(vec![
                tool("p3", "ask", serde_json::json!({"question": "Which surface?", "options": ["Drawer", "Chat"]})),
                Step::Finish("tool_calls".to_string()),
            ]),
            Reply::text("The drawer, then."),
        ],
    );
    let context = factory.new_context();
    let _entered = context.enter();
    let started = start_run(
        factory,
        &StartRequest {
            user_id: "AGENT_PLAIN".to_string(),
            text: "Which surfaces are there?".to_string(),
            ..StartRequest::default()
        },
    )
    .unwrap();
    let paused = wait_for(factory, &started.run_id, std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(paused, RunState::InputRequired, "the question pauses the run");
    submit_input(factory, &started.run_id, "AGENT_PLAIN", "answer", "Drawer", None).unwrap();
    let ended = wait_for(factory, &started.run_id, std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(ended, RunState::Completed);

    let calls = rows(factory, "moqui.agent.AgentToolCall", "runId", &started.run_id);
    let statuses: Vec<(String, String)> = calls.iter().map(|row| (text(row, "toolName"), text(row, "statusId"))).collect();
    assert_eq!(calls.len(), 3, "{statuses:?}");
    assert!(statuses.iter().all(|(_, status)| status == "AgtcDone"), "{statuses:?}");
    let data = calls.iter().find(|row| text(row, "toolName") == "data").unwrap();
    assert!(text(data, "spillLocation").starts_with("dbresource://agent/"), "the result spilled");
    assert!(!rows(factory, "moqui.llm.LlmCall", "userId", "AGENT_PLAIN").is_empty(), "the calls are logged");
    assert_eq!(rows(factory, "moqui.agent.AgentRunStep", "runId", &started.run_id).len(), 3);
    assert!(rows(factory, "moqui.agent.AgentRunEvent", "runId", &started.run_id).len() > 3);
    assert_eq!(rows(factory, "moqui.agent.AgentQuestion", "runId", &started.run_id).len(), 1);
    let conversation = &rows(factory, "moqui.agent.AgentConversation", "conversationId", &started.conversation_id)[0];
    assert!(text(conversation, "screenStateJson").contains("/apps"), "the screen state is kept");

    // the next turn reads the spilled artifact back
    let artifact = format!("A{}", text(data, "toolCallId"));
    fake::script(
        "eval-fake",
        vec![
            Reply::steps(vec![
                tool("p4", "data", serde_json::json!({"artifact": artifact, "grep": "Drawer"})),
                Step::Finish("tool_calls".to_string()),
            ]),
            Reply::text("Read."),
        ],
    );
    let second = start_run(
        factory,
        &StartRequest {
            user_id: "AGENT_PLAIN".to_string(),
            conversation_id: Some(started.conversation_id.clone()),
            text: "Read it again.".to_string(),
            ..StartRequest::default()
        },
    )
    .unwrap();
    assert_eq!(
        wait_for(factory, &second.run_id, std::time::Duration::from_secs(60)).unwrap(),
        RunState::Completed
    );
    let read = &rows(factory, "moqui.agent.AgentToolCall", "runId", &second.run_id)[0];
    assert_eq!(text(read, "statusId"), "AgtcDone", "{}", text(read, "resultSummary"));
    assert!(text(read, "resultSummary").contains("Drawer"), "{}", text(read, "resultSummary"));
}
