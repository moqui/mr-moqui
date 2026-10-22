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

//! The agent triggers' loop guard on a seeded server on embedded Postgres (plans/LLM-A2A-Assist.md 4.4, 4.14; phase
//! 13.9): a person's change of an Enumeration reaches a document trigger through the real data feed
//! (`DataFeedPending`, `process#DataFeedPending`, the AgentTriggers feed), its run of the scripted fake provider changes
//! the record again through a service tool its policy approves (a background agent), the feed records that write with
//! the run's agent as its origin, and the trigger does not fire again; a write the run's tool service makes on another
//! service worker (`ec.service.async`) keeps the origin too. The chat members, the breaker, batching and the AI
//! tasks are checked on the in-memory database (`crates/moqui-core/src/llm/members_tests.rs`).

#![cfg(all(unix, feature = "embedded-postgres", feature = "llm"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm::provider::fake::{self, Step};
use moqui_core::llm::run::state::RunState;
use moqui_lib::object::{to_plain_string, Value};
use serde_json::json;

use crate::common::{basic, exchange, http_request, start_with_files};

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="members-fake" protocol="fake" description="Members fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="members-model" endpointId="members-fake" providerModel="fake-1" tier="fast" contextWindow="32768"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="members-model" reasoningLevel="none"/>
    <moqui.agent.AgentDefinition agentId="loopwatch" agentName="Loop Watcher" featureId="assist" surfaces="job"
            runAsUserId="EX_JOHN_DOE"/>
    <moqui.agent.AgentToolExposure toolName="touch_enum" serviceName="update#moqui.basic.Enumeration" title="Change an enumeration"
            lockVersion="1"/>
    <moqui.agent.AgentToolPolicy agentId="loopwatch" sequenceNum="1" toolPattern="touch_enum" approvalModeEnumId="AgamPolicy"/>
    <moqui.agent.AgentToolExposure toolName="touch_later" serviceName="agentmembers.MembersServices.touch#Later"
            title="Change an enumeration later" lockVersion="1"/>
    <moqui.agent.AgentToolPolicy agentId="loopwatch" sequenceNum="2" toolPattern="touch_later" approvalModeEnumId="AgamPolicy"/>
    <moqui.basic.EnumerationType enumTypeId="AGT_LOOP" description="Loop guard"/>
    <moqui.security.UserAccount userId="AGM_OTHER" username="agm.other" userFullName="Another User"/>
    <moqui.entity.document.DataDocument dataDocumentId="AgtLoopEnum" primaryEntityName="moqui.basic.Enumeration" documentName="Loop Enum"/>
    <moqui.entity.document.DataDocumentField dataDocumentId="AgtLoopEnum" fieldSeqId="01" fieldPath="enumId"/>
    <moqui.entity.document.DataDocumentField dataDocumentId="AgtLoopEnum" fieldSeqId="02" fieldPath="description"/>
    <moqui.security.ArtifactGroup artifactGroupId="AGENT_MEMBERS" description="Agent members test"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_MEMBERS" artifactName="moqui.basic.Enumeration" artifactTypeEnumId="AT_ENTITY"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_MEMBERS" artifactName=".*#moqui.basic.Enumeration" nameIsPattern="Y"
            artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_MEMBERS" artifactName="agentmembers\..*" nameIsPattern="Y"
            artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="AGENT_MEMBERS_ADMIN" userGroupId="ADMIN" artifactGroupId="AGENT_MEMBERS"
            authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
</entity-facade-xml>"#;

const COMPONENT: &str = r#"<component name="agentmembers"/>"#;

/// A tool service whose write happens on another service worker: the agent's origin must go with it.
const SERVICES: &str = r#"<services>
    <service verb="touch" noun="Later">
        <in-parameters><parameter name="enumId" required="true"/><parameter name="description" required="true"/></in-parameters>
        <actions><script>
            ec.service.async().name('update#moqui.basic.Enumeration').parameters({enumId: enumId, description: description}).call()
        </script></actions>
    </service>
</services>"#;

const WAIT: Duration = Duration::from_secs(60);

fn in_context<T>(factory: &Arc<ExecutionContextFactory>, body: impl FnOnce() -> T) -> T {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    body()
}

fn rows(
    factory: &Arc<ExecutionContextFactory>,
    entity: &str,
    field: &str,
    value: &str,
) -> Vec<moqui_core::entity::EntityValue> {
    in_context(factory, || {
        factory
            .entity()
            .unwrap()
            .find(entity)
            .unwrap()
            .eq(field, Value::string(value))
            .list()
            .unwrap()
            .values()
            .to_vec()
    })
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_trigger_leaves_out_its_own_agents_writes_through_the_data_feed() {
    let files = [
        ("component/agentmembers/component.xml", COMPONENT),
        ("component/agentmembers/service/agentmembers/MembersServices.xml", SERVICES),
    ];
    let gate = start_with_files("agent-members", Some(DATA), &[], &files);
    let factory = Arc::clone(gate.server.factory());
    fake::clear("members-fake");
    // the trigger, stored as an admin: its document joins the AgentTriggers feed
    in_context(&factory, || {
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let parameters = [
            ("triggerId", "LOOPWATCH"),
            ("agentId", "loopwatch"),
            ("triggerTypeEnumId", "AgttDocument"),
            ("dataDocumentId", "AgtLoopEnum"),
            ("instructions", "Mark each changed enumeration."),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), Value::string(value)))
        .chain([("batchSeconds".to_string(), Value::I64(0))])
        .collect();
        moqui_core::service::sync("org.moqui.agent.AgentTriggerServices.store#AgentTrigger", &parameters).unwrap();
    });
    assert_eq!(
        rows(
            &factory,
            "moqui.entity.feed.DataFeedDocument",
            "dataDocumentId",
            "AgtLoopEnum"
        )
        .len(),
        1
    );
    fake::script(
        "members-fake",
        vec![
            fake::Reply::steps(vec![
                Step::Tool {
                    id: "touch1".to_string(),
                    name: "run_tool".to_string(),
                    arguments: json!({"name": "touch_enum", "parameters": {"enumId": "AGT_LOOP_1", "description": "touched by the agent"}})
                        .to_string(),
                    pieces: 1,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("Marked AGT_LOOP_1."),
        ],
    );
    // a person's change
    in_context(&factory, || {
        moqui_core::user::internal_login_user("john.doe").unwrap();
        // in a transaction, as a service's write is: the feed is inserted at its commit
        let facade = factory.entity().unwrap();
        facade.begin("moqui.basic.Enumeration", None).unwrap();
        let mut row = facade.make_value("moqui.basic.Enumeration").unwrap();
        row.set("enumId", Value::string("AGT_LOOP_1"));
        row.set("enumTypeId", Value::string("AGT_LOOP"));
        row.set("description", Value::string("made by a person"));
        row.create(&facade).unwrap();
        facade.commit("moqui.basic.Enumeration").unwrap();
    });
    let started = Instant::now() + WAIT;
    while rows(&factory, "moqui.agent.AgentRun", "triggerId", "LOOPWATCH").is_empty() {
        if Instant::now() > started {
            let count = |entity: &str| {
                in_context(&factory, || {
                    factory.entity().unwrap().find(entity).unwrap().count().unwrap()
                })
            };
            let trigger = rows(&factory, "moqui.agent.AgentTrigger", "triggerId", "LOOPWATCH");
            let events = rows(&factory, "moqui.agent.AgentTriggerEvent", "triggerId", "LOOPWATCH");
            panic!(
                "no run of the trigger: {} pending feed rows, {} trigger events, {} runs; trigger {:?}; events {:?}",
                count("moqui.entity.feed.DataFeedPending"),
                count("moqui.agent.AgentTriggerEvent"),
                count("moqui.agent.AgentRun"),
                trigger.first().map(|row| (
                    row.get("failureCount").map(to_plain_string),
                    row.get("lastFailureText").map(to_plain_string)
                )),
                events
                    .iter()
                    .map(|row| row.get("runId").map(to_plain_string))
                    .collect::<Vec<_>>()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let run = rows(&factory, "moqui.agent.AgentRun", "triggerId", "LOOPWATCH").remove(0);
    let run_id = to_plain_string(run.get("runId").unwrap());
    assert_eq!(
        moqui_core::llm::run::wait_for(&factory, &run_id, WAIT).unwrap(),
        RunState::Completed
    );
    let touched = rows(&factory, "moqui.basic.Enumeration", "enumId", "AGT_LOOP_1").remove(0);
    assert_eq!(
        to_plain_string(touched.get("description").unwrap()),
        "touched by the agent",
        "the policy approved the write"
    );
    let approvals = rows(&factory, "moqui.agent.AgentApproval", "runId", &run_id);
    assert_eq!(to_plain_string(approvals[0].get("decisionChannel").unwrap()), "policy");
    // the run's own change goes through the feed with its origin and is left out
    wait_until("the feed of the run's change", || {
        rows(
            &factory,
            "moqui.entity.feed.DataFeedPending",
            "dataDocumentId",
            "AgtLoopEnum",
        )
        .is_empty()
    });
    assert!(moqui_core::llm::run::observe::settle(&factory, WAIT));
    std::thread::sleep(Duration::from_millis(500));
    let events = rows(&factory, "moqui.agent.AgentTriggerEvent", "triggerId", "LOOPWATCH");
    assert_eq!(events.len(), 1, "only the person's change is an event");
    assert!(to_plain_string(events[0].get("eventJson").unwrap()).contains("made by a person"));
    assert_eq!(
        rows(&factory, "moqui.agent.AgentRun", "triggerId", "LOOPWATCH").len(),
        1,
        "the trigger did not fire again"
    );
    // another person's change still fires it; this run's write goes through an async service, and keeps its origin
    fake::script(
        "members-fake",
        vec![
            fake::Reply::steps(vec![
                Step::Tool {
                    id: "touch2".to_string(),
                    name: "run_tool".to_string(),
                    arguments: json!({"name": "touch_later", "parameters": {"enumId": "AGT_LOOP_1", "description": "touched later by the agent"}})
                        .to_string(),
                    pieces: 1,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("Marked it later."),
        ],
    );
    in_context(&factory, || {
        let facade = factory.entity().unwrap();
        facade.begin("moqui.basic.Enumeration", None).unwrap();
        let mut row = facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumId", Value::string("AGT_LOOP_1"))
            .one()
            .unwrap()
            .unwrap();
        row.set("description", Value::string("changed by a person"));
        row.update(&facade).unwrap();
        facade.commit("moqui.basic.Enumeration").unwrap();
    });
    wait_until("the second run", || rows(&factory, "moqui.agent.AgentRun", "triggerId", "LOOPWATCH").len() == 2);
    for run in rows(&factory, "moqui.agent.AgentRun", "triggerId", "LOOPWATCH") {
        moqui_core::llm::run::wait_for(&factory, &to_plain_string(run.get("runId").unwrap()), WAIT).unwrap();
    }
    wait_until("the async write", || {
        to_plain_string(rows(&factory, "moqui.basic.Enumeration", "enumId", "AGT_LOOP_1")[0].get("description").unwrap())
            == "touched later by the agent"
    });
    wait_until("the feed of the async write", || {
        rows(&factory, "moqui.entity.feed.DataFeedPending", "dataDocumentId", "AgtLoopEnum").is_empty()
    });
    assert!(moqui_core::llm::run::observe::settle(&factory, WAIT));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        rows(&factory, "moqui.agent.AgentTriggerEvent", "triggerId", "LOOPWATCH").len(),
        2,
        "the write of the async service kept the agent's origin and is no event"
    );
    assert_eq!(rows(&factory, "moqui.agent.AgentRun", "triggerId", "LOOPWATCH").len(), 2, "no third run");
    the_inbox_screen_shows_only_the_viewers_own(&gate, &factory);
    a_run_limit_holds_under_concurrent_firings(&factory);
}

/// Four firings at once of a trigger allowed one run an hour: the locked trigger row lets exactly one through.
fn a_run_limit_holds_under_concurrent_firings(factory: &Arc<ExecutionContextFactory>) {
    in_context(factory, || {
        moqui_core::user::internal_login_user("john.doe").unwrap();
        let parameters = [
            ("triggerId", Value::string("LIMITONE")),
            ("agentId", Value::string("loopwatch")),
            ("triggerTypeEnumId", Value::string("AgttCron")),
            ("runAsUserId", Value::string("EX_JOHN_DOE")),
            ("maxRunsPerHour", Value::I64(1)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect();
        moqui_core::service::sync("org.moqui.agent.AgentTriggerServices.store#AgentTrigger", &parameters).unwrap();
    });
    fake::script("members-fake", (0..4).map(|_| fake::Reply::text("Checked.")).collect());
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let factory = Arc::clone(factory);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                barrier.wait();
                moqui_core::llm::trigger::fire(&factory, "LIMITONE", "manual").unwrap()
            })
        })
        .collect();
    let fired: Vec<_> = threads.into_iter().map(|thread| thread.join().unwrap()).collect();
    let started: Vec<&String> = fired
        .iter()
        .filter_map(|fired| match fired {
            moqui_core::llm::trigger::Fired::Started { run_id, .. } => Some(run_id),
            moqui_core::llm::trigger::Fired::Skipped(_) => None,
        })
        .collect();
    assert_eq!(started.len(), 1, "{fired:?}");
    assert_eq!(rows(factory, "moqui.agent.AgentRun", "triggerId", "LIMITONE").len(), 1);
    moqui_core::llm::run::wait_for(factory, started[0], WAIT).unwrap();
}

/// System › LLM › Inbox lists the viewer's own agent inbox and never another user's (D12).
fn the_inbox_screen_shows_only_the_viewers_own(gate: &crate::common::Gate, factory: &Arc<ExecutionContextFactory>) {
    in_context(factory, || {
        let mine = ["EX_JOHN_DOE".to_string()];
        let theirs = ["AGM_OTHER".to_string()];
        moqui_core::llm::trigger::inbox(factory, &mine, "Inbox note for John", json!({"kind": "result"}));
        moqui_core::llm::trigger::inbox(factory, &theirs, "Inbox note for another user", json!({"kind": "result", "secret": "theirs"}));
    });
    let request = http_request("GET", "/apps/system/Llm/Inbox.html", &[("Authorization", &basic("john.doe", "moqui"))], "");
    let (status, _, page) = exchange(gate.server.address(), &request);
    assert_eq!(status, 200, "{}", page.chars().take(400).collect::<String>());
    assert!(page.contains("Inbox note for John"), "the viewer's own");
    assert!(!page.contains("Inbox note for another user"), "another user's is not listed");
}
