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

//! The approval matrix of plans/LLM-A2A-Assist.md 13.4 against the scripted fake provider on the in-memory database,
//! with the test tools of `tools/tests.rs` (`test_approve`, a write that asks for its approval as a service tool does,
//! and `test_held`, a read tool that holds its writes as `screen` does): approve and execute, another user's approval
//! not found, decisions refused for header auth, agent and agent-apps origins and internal code, expiry, a hash
//! mismatch, a double approve, a conflict, reject with feedback, a hand-off saved (after a refused try) and abandoned,
//! policy approval for a background agent, a READ-capped agent, and a run that ends with its approval open. The screen
//! writes themselves are checked on embedded Postgres (`crates/moqui-host/tests/it/agent_screens.rs`).

use std::sync::Arc;
use std::time::Duration;

use moqui_lib::object::Value;
use serde_json::{json, Map, Value as Json};

use super::*;
use crate::context::{ExecutionContext, ExecutionContextFactory, AgentOrigin};
use crate::llm::provider::fake::{self, Step};
use crate::llm::run::{self, RunState, StartRequest};
use crate::llm::tools::tests::{calls, TEST_CAP, TEST_HANDOFF_SCREEN};
use crate::screen::{RenderOrigin, TransitionOutcome};

const USER: &str = "approval.tester";
const OTHER: &str = "approval.other";
const WAIT: Duration = Duration::from_secs(15);

struct Bed {
    ctx: ExecutionContext,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
    endpoint: String,
}

fn bed(name: &str) -> Bed {
    let endpoint = format!("approval-{name}");
    let conf = format!(
        r#"<moqui-conf><llm-facade>
            <endpoint id="{endpoint}" protocol="fake"/>
            <model id="{endpoint}" endpoint="{endpoint}" name="fake-1" context-window="32768"/>
            <bind feature="assist" model="{endpoint}" reasoning="none"/>
        </llm-facade></moqui-conf>"#
    );
    let ctx = ExecutionContextFactory::test_context_in(
        &conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    let facade = crate::testing::open();
    ctx.factory().set_entity(facade.clone());
    fake::clear(&endpoint);
    for (id, name) in [(USER, "Approval Tester"), (OTHER, "Approval Other")] {
        row(
            &facade,
            "moqui.security.UserAccount",
            &[("userId", id), ("username", id), ("userFullName", name)],
        );
    }
    row(
        &facade,
        "moqui.agent.AgentDefinition",
        &[
            ("agentId", "assist"),
            ("agentName", "Assist"),
            ("featureId", "assist"),
            ("surfaces", "drawer,chat,job,tui"),
        ],
    );
    Bed {
        factory: Arc::clone(ctx.factory()),
        ctx,
        facade,
        endpoint,
    }
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (field, text) in fields {
        value.set(*field, Value::string(*text));
    }
    value.create(facade).unwrap();
}

fn tool_step(id: &str, name: &str, arguments: Json) -> fake::Reply {
    fake::Reply::steps(vec![
        Step::Tool {
            id: id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
            pieces: 1,
        },
        Step::Finish("tool_calls".to_string()),
    ])
}

/// A drawer run that calls `tool` with `args` and then answers: paused for the approval, its id.
fn paused_run(bed: &Bed, tool: &str, args: Json) -> (String, String) {
    fake::script(
        &bed.endpoint,
        vec![tool_step("w1", tool, args), fake::Reply::text("Done.")],
    );
    let started = run::start_run(
        &bed.factory,
        &StartRequest {
            user_id: USER.to_string(),
            text: "Write it".to_string(),
            extra_caps: vec![TEST_CAP.to_string()],
            ..StartRequest::default()
        },
    )
    .unwrap();
    assert_eq!(
        run::wait_for(&bed.factory, &started.run_id, WAIT).unwrap(),
        RunState::InputRequired
    );
    let paused = run::load_run(&bed.facade, &started.run_id).unwrap().unwrap();
    assert_eq!(paused.pause_kind, Some(PauseKind::Approval));
    (started.run_id, paused.pause_ref.unwrap())
}

fn view(bed: &Bed, approval_id: &str) -> ApprovalView {
    load(&bed.facade, approval_id).unwrap().unwrap()
}

fn user() -> Decider {
    Decider::scripted(USER, Channel::Drawer)
}

fn events(bed: &Bed, run_id: &str) -> Vec<String> {
    run::events::stored_after(&bed.facade, run_id, 0, 10_000)
        .unwrap()
        .into_iter()
        .map(|event| event.event_type)
        .collect()
}

/// The text the model read for the call after it resumed: the second request's tool result.
fn result_text(bed: &Bed) -> String {
    let sent = fake::requests(&bed.endpoint);
    let messages = sent.last().unwrap()["messages"].as_array().unwrap().clone();
    messages
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["text"].as_str())
        .unwrap_or_default()
        .to_string()
}

fn approve() -> Decision {
    Decision::Approve { arguments: None }
}

#[test]
fn an_approved_write_runs_once_and_only_for_its_user() {
    let bed = bed("approve");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "approve-1"}));
    let pending = view(&bed, &approval_id);
    assert_eq!(pending.status, ApprovalStatus::Pending);
    assert_eq!(pending.user_id, USER);
    assert_eq!(approval_id.len(), 32, "128 random bits as hex");
    assert_eq!(
        pending.args_hash,
        approval_hash(USER, "assist", "test_approve", TargetKind::Tool, "test#approve-1", &pending.args)
    );
    assert_eq!(pending.changes()[0].proposed, "approve-1");
    assert!(calls("approve-1").is_empty(), "nothing runs before the decision");
    assert!(events(&bed, &run_id).contains(&"approval.requested".to_string()));
    // another user's approval is not found, for a decision, a preview or a read
    let other = Decider::scripted(OTHER, Channel::Drawer);
    assert_eq!(decide(&bed.factory, &approval_id, &other, &approve()), Err(DecideError::NotFound));
    assert_eq!(
        preview::preview_approval(&bed.factory, &approval_id, OTHER),
        Err(DecideError::NotFound)
    );
    assert!(load_for_owner(&bed.facade, &approval_id, OTHER).unwrap().is_none());
    // a decision is not run input
    assert!(run::submit_input(&bed.factory, &run_id, USER, "decision", "approve", None).is_err());
    // a message meanwhile waits: the run stays paused for the decision
    run::submit_input(&bed.factory, &run_id, USER, "message", "and hurry", None).unwrap();
    assert_eq!(run::load_run(&bed.facade, &run_id).unwrap().unwrap().state, RunState::InputRequired);

    let decided = decide(&bed.factory, &approval_id, &user(), &approve()).unwrap();
    assert_eq!(decided.status, ApprovalStatus::Approved);
    assert!(
        matches!(decide(&bed.factory, &approval_id, &user(), &approve()), Err(DecideError::Refused(text)) if text.contains("decided already")),
        "a double approve is refused"
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(calls("approve-1").len(), 1, "it ran once");
    let done = view(&bed, &approval_id);
    assert_eq!(done.status, ApprovalStatus::Executed);
    assert_eq!(done.decision_channel.as_deref(), Some("drawer"));
    assert_eq!(result_text(&bed), "wrote approve-1");
    let kinds = events(&bed, &run_id);
    assert!(kinds.contains(&"approval.decided".to_string()), "{kinds:?}");
    // the message sent meanwhile was folded in after the call
    assert!(kinds.contains(&"input.folded".to_string()), "{kinds:?}");
    assert!(matches!(decide(&bed.factory, &approval_id, &user(), &approve()), Err(DecideError::Refused(_))));
}

#[test]
fn only_an_interactive_request_of_the_user_decides() {
    let bed = bed("origins");
    let _entered = bed.ctx.enter();
    {
        let _authz = bed.ctx.disable_authz();
        crate::user::internal_login_user(USER).unwrap();
    }
    // internal code: a job, a service called from Rust
    assert!(Decider::from_request(None).is_err());
    let from = |origin: RenderOrigin, header: bool, token: bool| {
        crate::screen::gate::with_request_frame(origin, header, token, || Decider::from_request(Some(Channel::Chat)))
    };
    assert!(from(RenderOrigin::Browser, true, true).is_err(), "header authentication");
    assert!(from(RenderOrigin::Browser, false, false).is_err(), "no session token");
    let agent = RenderOrigin::Agent {
        agent_id: "assist".to_string(),
        run_id: None,
    };
    assert!(from(agent, false, true).is_err(), "the agent's own screen session");
    assert!(from(RenderOrigin::AgentApps, false, true).is_err(), "agent-apps");
    assert!(from(RenderOrigin::Internal, false, true).is_err(), "an internal render");
    let browser = from(RenderOrigin::Browser, false, true).unwrap();
    assert_eq!((browser.user_id(), browser.channel()), (USER, Channel::Chat));
    let terminal = from(RenderOrigin::Terminal, true, false).unwrap();
    assert_eq!(terminal.channel(), Channel::Tui);
    // a render inside an agent's render is the agent's
    let nested = crate::screen::gate::with_request_frame(
        RenderOrigin::Agent {
            agent_id: "assist".to_string(),
            run_id: None,
        },
        false,
        true,
        || {
            crate::screen::gate::with_request_frame(RenderOrigin::Browser, false, true, || {
                Decider::from_request(None)
            })
        },
    );
    assert!(nested.is_err());
    // a context an agent's run works in never decides, whatever the render
    bed.ctx.set_agent_origin(Some(AgentOrigin {
        agent_id: "assist".to_string(),
        run_id: Some("R1".to_string()),
    }));
    assert!(from(RenderOrigin::Browser, false, true).is_err(), "the agent's own context");
    bed.ctx.set_agent_origin(None);
}

#[test]
fn a_rejection_is_the_feedback_and_runs_nothing() {
    let bed = bed("reject");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "reject-1"}));
    let decided = decide(
        &bed.factory,
        &approval_id,
        &user(),
        &Decision::Reject {
            feedback: "Use reject-2 instead".to_string(),
        },
    )
    .unwrap();
    assert_eq!(decided.status, ApprovalStatus::Rejected);
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(calls("reject-1").is_empty());
    assert_eq!(view(&bed, &approval_id).feedback.as_deref(), Some("Use reject-2 instead"));
    let text = result_text(&bed);
    assert!(
        text.contains("rejected") && text.contains("only task: Use reject-2 instead"),
        "{text}"
    );
}

/// Phase 13.14 (plan 7.5): the requester's feedback on a rejected service write becomes their proposed lesson on that
/// service; a tool call has no anchor, so its rejection makes none.
#[test]
fn a_rejection_with_feedback_on_a_service_is_a_proposed_lesson_of_the_requester() {
    let bed = bed("reject-lesson");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "lesson-1"}));
    bed.facade
        .find(APPROVAL)
        .unwrap()
        .eq("approvalId", Value::string(&approval_id))
        .update_all([
            ("targetKind".to_string(), Value::string("service")),
            ("targetName".to_string(), Value::string("org.example.Things.update#Thing")),
        ])
        .unwrap();
    decide(
        &bed.factory,
        &approval_id,
        &user(),
        &Decision::Reject {
            feedback: "Close the thing instead of changing it".to_string(),
        },
    )
    .unwrap();
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    let anchor = crate::llm::memory::Anchor::Service("org.example.Things.update#Thing".to_string());
    assert_eq!(
        crate::llm::memory::active_lessons(&bed.facade, USER, &anchor, 3),
        ["Close the thing instead of changing it"]
    );
    let lessons = crate::llm::memory::own_lessons(&bed.facade, USER).unwrap();
    assert_eq!(lessons.len(), 1);
    assert_eq!(crate::llm::text_of(&lessons[0], "statusId").as_deref(), Some("AglProposed"));
    assert!(crate::llm::text_of(&lessons[0], "triggerText").unwrap().starts_with("Rejected: "));
    assert!(crate::llm::memory::active_lessons(&bed.facade, "someone.else", &anchor, 3).is_empty());

    // with the requester's memory turned off, a rejection's feedback is kept as no lesson
    let mut off = bed.facade.make_value("moqui.security.UserPreference").unwrap();
    off.set("userId", Value::string(USER));
    off.set("preferenceKey", Value::string(crate::llm::memory::PREFERENCE));
    off.set("preferenceValue", Value::string("false"));
    off.create(&bed.facade).unwrap();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "lesson-2"}));
    bed.facade
        .find(APPROVAL)
        .unwrap()
        .eq("approvalId", Value::string(&approval_id))
        .update_all([
            ("targetKind".to_string(), Value::string("service")),
            ("targetName".to_string(), Value::string("org.example.Things.update#Thing")),
        ])
        .unwrap();
    decide(&bed.factory, &approval_id, &user(), &Decision::Reject { feedback: "Never touch it".to_string() }).unwrap();
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(crate::llm::memory::own_lessons(&bed.facade, USER).unwrap().len(), 1, "no lesson with memory off");
}

#[test]
fn arguments_that_do_not_hash_to_the_approved_ones_run_nothing() {
    let bed = bed("mismatch");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "mismatch-1"}));
    // what is stored to run is changed after the hash was made
    bed.facade
        .find(APPROVAL)
        .unwrap()
        .eq("approvalId", Value::string(&approval_id))
        .update_all([(
            "argumentsJson".to_string(),
            Value::string(json!({"key": "mismatch-2"}).to_string()),
        )])
        .unwrap();
    decide(&bed.factory, &approval_id, &user(), &approve()).unwrap();
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(calls("mismatch-1").is_empty() && calls("mismatch-2").is_empty());
    assert_eq!(view(&bed, &approval_id).status, ApprovalStatus::Failed);
    assert!(result_text(&bed).contains("not the arguments the user approved"), "{}", result_text(&bed));
}

#[test]
fn edited_arguments_are_what_runs_and_what_the_hash_binds() {
    let bed = bed("edited");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "edited-1"}));
    decide(
        &bed.factory,
        &approval_id,
        &user(),
        &Decision::Approve {
            arguments: Some(json!({"key": "edited-2"})),
        },
    )
    .unwrap();
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(calls("edited-1").is_empty());
    assert_eq!(calls("edited-2").len(), 1);
    let done = view(&bed, &approval_id);
    assert_eq!(done.status, ApprovalStatus::Executed);
    assert_eq!(done.snapshot["proposedArgs"]["key"], "edited-1");
}

#[test]
fn a_conflict_found_before_the_run_runs_nothing() {
    let bed = bed("conflict");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "conflict-1", "conflict": true}));
    decide(&bed.factory, &approval_id, &user(), &approve()).unwrap();
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(calls("conflict-1").is_empty());
    let done = view(&bed, &approval_id);
    assert_eq!(done.status, ApprovalStatus::Conflict);
    assert_eq!(done.result.unwrap()["conflicts"][0]["current"], "changed");
}

#[test]
fn an_approval_expires_and_its_run_fails() {
    let bed = bed("expire");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "expire-1"}));
    let past = run::stamp(run::now_millis() - 60_000);
    bed.facade
        .find(APPROVAL)
        .unwrap()
        .eq("approvalId", Value::string(&approval_id))
        .update_all([("expireDate".to_string(), past.clone())])
        .unwrap();
    // a decision after the expiry is refused and expires it
    assert!(
        matches!(decide(&bed.factory, &approval_id, &user(), &approve()), Err(DecideError::Refused(text)) if text.contains("expired"))
    );
    assert_eq!(view(&bed, &approval_id).status, ApprovalStatus::Expired);
    // the sweep fails the paused run at its pause's expiry
    bed.facade
        .find(crate::llm::run::RUN)
        .unwrap()
        .eq("runId", Value::string(&run_id))
        .update_all([("pauseExpireDate".to_string(), past)])
        .unwrap();
    assert_eq!(run::lease::expire_pauses(&bed.factory).unwrap(), 1);
    let ended = run::load_run(&bed.facade, &run_id).unwrap().unwrap();
    assert_eq!(ended.state, RunState::Failed);
    assert!(calls("expire-1").is_empty());
}

#[test]
fn an_approval_whose_run_ended_expires() {
    let bed = bed("canceled");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "canceled-1"}));
    run::cancel_run(&bed.factory, &run_id, Some(USER)).unwrap();
    assert_eq!(view(&bed, &approval_id).status, ApprovalStatus::Expired);
    assert!(events(&bed, &run_id).contains(&"approval.decided".to_string()));
    assert!(matches!(decide(&bed.factory, &approval_id, &user(), &approve()), Err(DecideError::Refused(_))));
}

fn outcome(user_id: &str, origin: RenderOrigin, value: &str, validation: &[&str]) -> TransitionOutcome {
    let mut parameters = Map::new();
    parameters.insert("value".to_string(), json!(value));
    TransitionOutcome {
        origin,
        user_id: Some(user_id.to_string()),
        screen_location: TEST_HANDOFF_SCREEN.to_string(),
        transition: "saveTest".to_string(),
        method: "POST".to_string(),
        parameters,
        extra_path: Vec::new(),
        inline_parameters: Map::new(),
        held: false,
        header_authenticated: false,
        status: 200,
        location: None,
        error: None,
        errors: Vec::new(),
        validation_errors: validation.iter().map(|text| (*text).to_string()).collect(),
        messages: vec!["Saved".to_string()],
    }
}

#[test]
fn a_hand_off_watches_for_the_users_own_save() {
    let bed = bed("handoff");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "handoff-1", "handoff": true}));
    let decided = decide(&bed.factory, &approval_id, &user(), &Decision::Edit).unwrap();
    assert_eq!(decided.status, ApprovalStatus::HandedOff);
    let handoff_id = decided.handoff_id.unwrap();
    let paused = run::load_run(&bed.facade, &run_id).unwrap().unwrap();
    assert_eq!(
        (paused.state, paused.pause_kind, paused.pause_ref.as_deref()),
        (RunState::InputRequired, Some(PauseKind::Handoff), Some(handoff_id.as_str()))
    );
    let kinds = events(&bed, &run_id);
    for kind in ["handoff.started", "ui.navigate", "ui.fill"] {
        assert!(kinds.contains(&kind.to_string()), "{kind} in {kinds:?}");
    }
    // the agent's own render, another user's save and another transition are not the user's save
    let agent = RenderOrigin::Agent {
        agent_id: "assist".to_string(),
        run_id: None,
    };
    assert_eq!(handoff::observe(&bed.factory, &outcome(USER, agent, "x", &[])).unwrap(), None);
    assert_eq!(
        handoff::observe(&bed.factory, &outcome(OTHER, RenderOrigin::Browser, "x", &[])).unwrap(),
        None
    );
    let mut keyed = outcome(USER, RenderOrigin::Browser, "by a program", &[]);
    keyed.header_authenticated = true;
    assert_eq!(handoff::observe(&bed.factory, &keyed).unwrap(), None, "a header login is not the user's save");
    // a refused save keeps the watch for the next try
    let refused = outcome(USER, RenderOrigin::Browser, "", &["value: required"]);
    assert_eq!(handoff::observe(&bed.factory, &refused).unwrap().as_deref(), Some(handoff_id.as_str()));
    assert_eq!(
        handoff::of_approval(&bed.facade, &approval_id).unwrap().unwrap().status,
        handoff::HandoffStatus::Failed
    );
    assert_eq!(run::load_run(&bed.facade, &run_id).unwrap().unwrap().state, RunState::InputRequired);
    let saved = outcome(USER, RenderOrigin::Browser, "handoff-edited", &[]);
    assert_eq!(handoff::observe(&bed.factory, &saved).unwrap().as_deref(), Some(handoff_id.as_str()));
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(calls("handoff-1").is_empty(), "the agent ran nothing: the user saved it");
    assert_eq!(view(&bed, &approval_id).status, ApprovalStatus::Executed);
    let text = result_text(&bed);
    assert!(
        text.contains("saved this write themselves") && text.contains("Value: handoff-edited (you proposed handoff-1)"),
        "{text}"
    );
    assert!(events(&bed, &run_id).contains(&"handoff.done".to_string()));
}

/// The read fence (an admin's READ override of a write service included) and the sim preview's fence stop a resource
/// write and the publish of a notification, so neither happens for real.
#[test]
fn the_fences_stop_resource_writes_and_notifications() {
    let bed = bed("effects");
    let _entered = bed.ctx.enter();
    let _authz = bed.ctx.disable_authz();
    let dir = std::env::temp_dir().join(format!("approval-effects-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let kept = dir.join("keep.txt");
    std::fs::write(&kept, "keep").unwrap();
    let location = format!("file://{}", kept.display());
    let resources = bed.factory.resource();
    for sim in [false, true] {
        let fence = if sim {
            crate::entity::fence::enter_sim("a preview")
        } else {
            crate::entity::fence::enter("tool a_read_service")
        };
        let refused = resources.delete(&location).unwrap_err();
        assert!(refused.message().contains(crate::entity::fence::FENCE_ERROR), "{refused}");
        assert!(resources.write_bytes(&location, b"changed").is_err());
        let sent = crate::notify::NotificationMessage::new()
            .topic("ApprovalTest")
            .user_id(USER)
            .send();
        assert!(sent.is_err_and(|error| error.message().contains("a notification (ApprovalTest)")));
        assert_eq!(fence.violations().len(), 3, "{:?}", fence.violations());
    }
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "keep");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_hand_off_that_expires_is_abandoned_and_the_run_goes_on() {
    let bed = bed("abandon");
    let _entered = bed.ctx.enter();
    let (run_id, approval_id) = paused_run(&bed, "test_approve", json!({"key": "abandon-1", "handoff": true}));
    decide(&bed.factory, &approval_id, &user(), &Decision::Edit).unwrap();
    bed.facade
        .find(crate::llm::run::RUN)
        .unwrap()
        .eq("runId", Value::string(&run_id))
        .update_all([("pauseExpireDate".to_string(), run::stamp(run::now_millis() - 1000))])
        .unwrap();
    assert_eq!(run::lease::expire_pauses(&bed.factory).unwrap(), 1);
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(view(&bed, &approval_id).status, ApprovalStatus::Expired);
    assert_eq!(
        handoff::of_approval(&bed.facade, &approval_id).unwrap().unwrap().status,
        handoff::HandoffStatus::Abandoned
    );
    let text = result_text(&bed);
    assert!(text.contains("did not save it (expired)"), "{text}");
    // a write with no form to fill has no Edit in screen
    let (_, plain) = paused_run(&bed, "test_approve", json!({"key": "abandon-2"}));
    assert!(matches!(
        decide(&bed.factory, &plain, &user(), &Decision::Edit),
        Err(DecideError::Refused(_))
    ));
}

#[test]
fn a_background_agent_writes_by_policy_and_a_read_capped_one_gets_no_approval() {
    let bed = bed("policy");
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.agent.AgentDefinition",
        &[("agentId", "jobber"), ("agentName", "Jobber"), ("featureId", "assist"), ("surfaces", "job")],
    );
    row(
        &bed.facade,
        "moqui.agent.AgentToolPolicy",
        &[("agentId", "jobber"), ("sequenceNum", "1"), ("toolPattern", "test_*"), ("approvalModeEnumId", "AgamPolicy")],
    );
    fake::script(
        &bed.endpoint,
        vec![tool_step("p1", "test_approve", json!({"key": "policy-1"})), fake::Reply::text("Done.")],
    );
    let started = run::start_run(
        &bed.factory,
        &StartRequest {
            user_id: USER.to_string(),
            agent_id: "jobber".to_string(),
            surface: crate::llm::prompt::Surface::Job,
            text: "Write it".to_string(),
            extra_caps: vec![TEST_CAP.to_string()],
            ..StartRequest::default()
        },
    )
    .unwrap();
    assert_eq!(run::wait_for(&bed.factory, &started.run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(calls("policy-1").len(), 1, "policy approved it without asking");
    let decided = bed
        .facade
        .find(APPROVAL)
        .unwrap()
        .eq("runId", Value::string(&started.run_id))
        .one()
        .unwrap()
        .map(|row| ApprovalView::of(&row))
        .unwrap();
    assert_eq!(decided.status, ApprovalStatus::Executed);
    assert_eq!(decided.decision_channel.as_deref(), Some("policy"));
    assert_eq!(decided.policy.as_deref(), Some("AgentToolPolicy jobber#1"));

    // an agent capped at READ: its held write is no approval, and the model reads why
    row(
        &bed.facade,
        "moqui.agent.AgentDefinition",
        &[("agentId", "reader"), ("agentName", "Reader"), ("featureId", "assist")],
    );
    row(
        &bed.facade,
        "moqui.agent.AgentToolPolicy",
        &[("agentId", "reader"), ("sequenceNum", "1"), ("toolPattern", "*"), ("classificationEnumId", "AgclRead")],
    );
    fake::script(
        &bed.endpoint,
        vec![tool_step("r1", "test_held", json!({"key": "read-1"})), fake::Reply::text("Cannot.")],
    );
    let reader = run::start_run(
        &bed.factory,
        &StartRequest {
            user_id: USER.to_string(),
            agent_id: "reader".to_string(),
            text: "Write it".to_string(),
            extra_caps: vec![TEST_CAP.to_string()],
            ..StartRequest::default()
        },
    )
    .unwrap();
    assert_eq!(run::wait_for(&bed.factory, &reader.run_id, WAIT).unwrap(), RunState::Completed);
    assert!(result_text(&bed).contains("may not make write calls with test_held"), "{}", result_text(&bed));
    let none = bed
        .facade
        .find(APPROVAL)
        .unwrap()
        .eq("runId", Value::string(&reader.run_id))
        .list()
        .unwrap();
    assert!(none.is_empty());
    // a job with an ask-mode policy has nobody to ask
    fake::script(
        &bed.endpoint,
        vec![tool_step("j1", "test_approve", json!({"key": "job-1"})), fake::Reply::text("Cannot.")],
    );
    let job = run::start_run(
        &bed.factory,
        &StartRequest {
            user_id: USER.to_string(),
            surface: crate::llm::prompt::Surface::Job,
            text: "Write it".to_string(),
            extra_caps: vec![TEST_CAP.to_string()],
            ..StartRequest::default()
        },
    )
    .unwrap();
    assert_eq!(run::wait_for(&bed.factory, &job.run_id, WAIT).unwrap(), RunState::Completed);
    assert!(calls("job-1").is_empty());
    assert!(result_text(&bed).contains("nobody can give it here"), "{}", result_text(&bed));
}
