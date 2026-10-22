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

//! The agents' screen sessions on a seeded server on embedded Postgres (plans/LLM-A2A-Assist.md 4.7, 4.8, 9 Screens and
//! Approvals): a press that writes is held and leaves the row as it was, the held request run again with a valid
//! approval grant writes it and with a tampered one does not, the browser's values come in by form `domId` and field
//! name, the session answers follow events, and runs of the scripted fake provider use the `screen` tool end to end:
//! a `go` to a transition refused, a held save that pauses the run for its approval, decided over HTTP (refused for a
//! header-authenticated request and for another user, taken from the owner's browser session with its token) and then
//! saved; a conflict that saves nothing; an Edit in screen saved by the user's own browser post, and one abandoned at
//! its expiry; a user who may not run the transition gets no approval; and the sim preview of a held write.

#![cfg(all(
    unix,
    feature = "embedded-postgres",
    feature = "axum",
    feature = "llm",
    feature = "tui"
))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::llm::provider::fake::{self, Step};
use moqui_core::llm::screen::{
    self, ApprovalGrant, BrowserForm, BrowserState, OpenSpec, ScreenCommand, ScreenError, ScreenSessions,
};
use moqui_lib::object::{to_plain_string, Value};
use serde_json::json;

use moqui_core::llm::run::state::RunState;
use moqui_core::testkit::agents::{answer_approval, ApprovalApplied, ScriptedApproval};

use crate::common::{
    basic, exchange, http_request, merge_cookies, session_token, sign_in, start_with_conf_files,
};

const ENDPOINT: &str = "screens-fake";
const OWNER: &str = "EX_JOHN_DOE";
const PATH: &str = "/apps/agentscreens";

const DATA: &str = r#"<entity-facade-xml type="seed">
    <moqui.llm.LlmEndpoint endpointId="screens-fake" protocol="fake" description="Screens fake" isLocal="Y"/>
    <moqui.llm.LlmModel modelId="screens-model" endpointId="screens-fake" providerModel="fake-1" tier="fast"
        contextWindow="32768"/>
    <moqui.llm.LlmFeatureBinding featureId="assist" modelId="screens-model" reasoningLevel="none"/>
    <moqui.basic.EnumerationType enumTypeId="AGT_SCREEN" description="Before"/>
    <moqui.basic.EnumerationType enumTypeId="AGT_OTHER" description="Another record"/>
    <moqui.basic.EnumerationType enumTypeId="AGT_DELETE" description="To delete"/>
    <moqui.agent.AgentToolExposure toolName="delete_enum_type" serviceName="delete#moqui.basic.EnumerationType"
        title="Remove an enumeration type" lockVersion="1"/>
    <moqui.agent.AgentToolPolicy agentId="assist" sequenceNum="5" toolPattern="delete_enum_type" allowDestructive="Y"/>
    <moqui.security.ArtifactGroup artifactGroupId="AGENT_SCREENS" description="Agent screen sessions test"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_SCREENS" artifactName="component://agentscreens/.*"
        nameIsPattern="Y" artifactTypeEnumId="AT_XML_SCREEN" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_SCREENS" artifactName="component://agentscreens/.*"
        nameIsPattern="Y" artifactTypeEnumId="AT_XML_SCREEN_TRANS" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_SCREENS" artifactName=".*EnumerationType"
        nameIsPattern="Y" artifactTypeEnumId="AT_SERVICE"/>
    <!-- a service tool has no screen above it to inherit from: the entity itself, for the delete service tool -->
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_SCREENS" artifactName="moqui.basic.EnumerationType"
        artifactTypeEnumId="AT_ENTITY"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_SCREENS" artifactName="agentscreens\..*"
        nameIsPattern="Y" artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="AGENT_SCREENS_ADMIN" userGroupId="ADMIN" artifactGroupId="AGENT_SCREENS"
        authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
    <moqui.security.UserAccount userId="AGS_VIEWER" username="ags.viewer" userFullName="Agent Screens Viewer"/>
    <moqui.security.UserGroup userGroupId="AGS_VIEW" description="Agent screens viewers"/>
    <moqui.security.UserGroupMember userGroupId="AGS_VIEW" userId="AGS_VIEWER" fromDate="1265184000000"/>
    <moqui.security.ArtifactGroup artifactGroupId="AGENT_SCREENS_VIEW" description="Agent screens to view"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="AGENT_SCREENS_VIEW" artifactName="component://webroot/screen/webroot/apps.xml"
        artifactTypeEnumId="AT_XML_SCREEN" inheritAuthz="Y"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="AGENT_SCREENS_VIEW" userGroupId="AGS_VIEW" artifactGroupId="AGENT_SCREENS_VIEW"
        authzTypeEnumId="AUTHZT_ALLOW" authzActionEnumId="AUTHZA_VIEW"/>
</entity-facade-xml>"#;

const CONF: &str = r#"<screen-facade>
    <screen location="component://webroot/screen/webroot/apps.xml">
        <subscreens-item name="agentscreens" menu-title="Agent Screens" location="component://agentscreens/screen/AgentScreens.xml"/>
    </screen>
</screen-facade>"#;

const COMPONENT: &str = r#"<component name="agentscreens"/>"#;

/// One form that writes an `EnumerationType` through a service, and a second screen to go to.
const SCREEN: &str = r#"<screen require-authentication="true" default-menu-title="Agent Screens">
    <transition name="updateEnumType"><service-call name="update#moqui.basic.EnumerationType"/>
        <default-response url="."/></transition>
    <subscreens default-item="Edit"/>
    <widgets><subscreens-active/></widgets>
</screen>"#;

const EDIT: &str = r#"<screen require-authentication="true" default-menu-title="Edit">
    <transition name="updateEnumType"><service-call name="update#moqui.basic.EnumerationType"/>
        <default-response url="."/></transition>
    <!-- lookups with actions and no read-only, as many apps' search transitions: the gate would hold them as writes -->
    <transition name="getEnumTypes"><actions>
        <entity-find entity-name="moqui.basic.EnumerationType" list="typeList">
            <econdition field-name="enumTypeId" operator="like" value="AGT_%"/></entity-find>
        <script>ec.web.sendJsonResponse(typeList.map((row) => ({value: row.enumTypeId, label: row.enumTypeId + ' ' + row.description})))</script>
    </actions><default-response type="none"/></transition>
    <transition name="writeOnLookup"><actions><script>
        const made = ec.entity.makeValue('moqui.basic.EnumerationType'); made.set('enumTypeId', 'AGT_LOOKUP_WROTE');
        made.set('description', 'a lookup that writes'); made.create();
        ec.web.sendJsonResponse([{value: 'X', label: 'X'}])
    </script></actions><default-response type="none"/></transition>
    <actions><entity-find-one entity-name="moqui.basic.EnumerationType" value-field="enumType">
        <field-map field-name="enumTypeId" value="AGT_SCREEN"/></entity-find-one></actions>
    <widgets>
        <form-single name="EditEnumType" transition="updateEnumType" map="enumType">
            <field name="enumTypeId"><default-field><hidden/></default-field></field>
            <field name="description"><default-field title="Description"><text-line/></default-field></field>
            <field name="submitButton"><default-field title="Save"><submit/></default-field></field>
        </form-single>
        <form-single name="LookupForm">
            <field name="lookupType"><default-field title="Lookup Type"><drop-down allow-empty="true">
                <dynamic-options transition="getEnumTypes"/></drop-down></default-field></field>
            <field name="writingLookup"><default-field title="Writing Lookup"><drop-down allow-empty="true">
                <dynamic-options transition="writeOnLookup"/></drop-down></default-field></field>
        </form-single>
    </widgets>
</screen>"#;

const OTHER: &str = r#"<screen require-authentication="true" default-menu-title="Other">
    <widgets><label text="the other screen"/></widgets>
</screen>"#;

/// Services whose preview must reach nothing outside the sim: one writes a row and deletes a file, one writes a row and
/// sends a notification.
const SERVICES: &str = r#"<services>
    <service verb="sweep" noun="Files">
        <in-parameters><parameter name="location"/></in-parameters>
        <actions><script>
            const made = ec.entity.makeValue('moqui.basic.EnumerationType'); made.set('enumTypeId', 'AGT_SWEPT');
            made.set('description', 'swept'); made.create()
            ec.resource.getLocationReference(location).delete()
        </script></actions>
    </service>
    <service verb="announce" noun="Things">
        <actions><script>
            const made = ec.entity.makeValue('moqui.basic.EnumerationType'); made.set('enumTypeId', 'AGT_ANNOUNCED');
            made.set('description', 'announced'); made.create()
            ec.makeNotificationMessage().topic('AgentScreensTest').message({announced: true}).userIds(['EX_JOHN_DOE']).send()
        </script></actions>
    </service>
</services>"#;

/// A transition that decides an approval, as the drawer's will (phase 13.5).
const DECIDE: &str = r#"<screen require-authentication="true" default-menu-title="Decide">
    <transition name="decide"><actions><script>
        const asked = ec.web.parameters
        const decided = ec.service.sync().name('org.moqui.agent.AgentServices.decide#AgentApproval')
            .parameters({approvalId: asked.approvalId, decision: asked.decision, feedback: asked.feedback}).call() || {}
        ec.web.sendJsonResponse({statusId: decided.statusId || null, handoffId: decided.handoffId || null,
            errors: ec.message.errorsString})
    </script></actions><default-response type="none"/></transition>
    <widgets><label text="decide"/></widgets>
</screen>"#;

fn enum_type(factory: &Arc<ExecutionContextFactory>, id: &str) -> Option<String> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    facade
        .find("moqui.basic.EnumerationType")
        .unwrap()
        .eq("enumTypeId", Value::string(id))
        .one()
        .unwrap()
        .map(|row| to_plain_string(row.get("description").unwrap()))
}

fn description(factory: &Arc<ExecutionContextFactory>) -> String {
    enum_type(factory, "AGT_SCREEN").expect("AGT_SCREEN")
}

/// Change the row underneath a session, as another user's save would.
fn set_description(factory: &Arc<ExecutionContextFactory>, text: &str) {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let mut row = facade
        .find("moqui.basic.EnumerationType")
        .unwrap()
        .eq("enumTypeId", Value::string("AGT_SCREEN"))
        .one()
        .unwrap()
        .unwrap();
    row.set("description", Value::string(text));
    row.update(&facade).unwrap();
}

fn spec(conversation: &str, follow: bool) -> OpenSpec {
    OpenSpec {
        user_id: OWNER.to_string(),
        username: "john.doe".to_string(),
        conversation_id: conversation.to_string(),
        agent_id: "assist".to_string(),
        run_id: Some("AGS_RUN".to_string()),
        start_path: format!("{PATH}/Edit"),
        columns: 100,
        rows: 30,
        follow,
        state: None,
    }
}

fn command() -> ScreenCommand {
    ScreenCommand {
        agent_id: "assist".to_string(),
        run_id: Some("AGS_RUN".to_string()),
        ..ScreenCommand::default()
    }
}

#[test]
fn agent_screen_sessions_hold_writes_sync_in_and_follow() {
    let files = [
        ("component/agentscreens/component.xml", COMPONENT),
        ("component/agentscreens/screen/AgentScreens.xml", SCREEN),
        ("component/agentscreens/screen/AgentScreens/Edit.xml", EDIT),
        ("component/agentscreens/screen/AgentScreens/Other.xml", OTHER),
        ("component/agentscreens/screen/AgentScreens/Decide.xml", DECIDE),
        ("component/agentscreens/service/agentscreens/AgentScreensServices.xml", SERVICES),
    ];
    let gate = start_with_conf_files("agent-screens", Some(DATA), CONF, &[], &files);
    let factory = Arc::clone(gate.server.factory());
    let sessions = screen::sessions().expect("the host installs the screen sessions");
    held_and_approved(&factory, sessions.as_ref());
    run_on_a_fresh_link(&factory, sessions.as_ref());
    lookups_run_fenced(&factory, sessions.as_ref());
    synced_and_followed(sessions.as_ref());
    a_run_uses_the_screen_tool(&factory);
    let address = gate.server.address();
    a_decision_over_http_saves_the_held_write(&factory, address);
    a_conflict_saves_nothing(&factory);
    an_edit_in_screen_is_the_users_own_save(&factory, address);
    an_edit_in_screen_left_is_abandoned(&factory, address);
    no_approval_for_what_the_user_may_not_do(&factory);
    a_held_write_previews_in_the_sim(&factory);
    a_destructive_service_previews_before_its_card(&factory);
    a_header_login_session_never_decides(&factory, address);
    a_preview_reaches_nothing_outside_the_sim(&factory);
}

/// A save is held, the row unchanged; the held request with a tampered field is refused, with the approved hash it
/// writes.
fn held_and_approved(factory: &Arc<ExecutionContextFactory>, sessions: &dyn ScreenSessions) {
    let id = sessions.open(&spec("AGS_C1", false)).unwrap();
    assert_eq!(sessions.find(OWNER, "AGS_C1").as_deref(), Some(id.as_str()));
    assert!(
        sessions.find("EX_JANE", "AGS_C1").is_none(),
        "another user does not find it"
    );
    assert_eq!(
        sessions.command(&id, "EX_JANE", &command()).unwrap_err(),
        ScreenError::NotFound,
        "nor use it"
    );
    let look = sessions.command(&id, OWNER, &command()).unwrap();
    assert!(look.text.contains("[needs approval] Save"), "{}", look.text);
    assert!(look.path.ends_with("/Edit"), "{}", look.path);

    let saved = sessions
        .command(
            &id,
            OWNER,
            &ScreenCommand {
                set: vec![("Description".to_string(), "After".to_string())],
                press: Some("Save".to_string()),
                ..command()
            },
        )
        .unwrap();
    let write = saved.held.clone().unwrap_or_else(|| panic!("held: {}", saved.text));
    assert!(saved.text.contains("waits for the user's approval"), "{}", saved.text);
    assert_eq!(description(factory), "Before", "a held write runs nothing");
    assert_eq!(write.transition(), "updateEnumType");
    assert_eq!(write.held["service"], "update#moqui.basic.EnumerationType");
    assert_eq!(write.form, "EditEnumType");
    assert_eq!(write.fields.get("description").map(String::as_str), Some("After"));
    assert_eq!(write.changes.len(), 1, "{:?}", write.changes);
    assert_eq!(
        (write.changes[0].original.as_str(), write.changes[0].proposed.as_str()),
        ("Before", "After")
    );
    assert_eq!(
        saved.state.values["EditEnumType"]["description"], "After",
        "the edit is kept"
    );
    assert_eq!(write.owner, OWNER);
    assert_eq!(write.agent_id, "assist");
    assert!(
        sessions.recheck(&id, OWNER, &write).unwrap().is_empty(),
        "nothing changed since"
    );
    // the field changed underneath: a conflict with the value now
    set_description(factory, "Changed");
    let conflicts = sessions.recheck(&id, OWNER, &write).unwrap();
    assert_eq!(conflicts.len(), 1, "{conflicts:?}");
    assert_eq!(
        (
            conflicts[0].field.as_str(),
            conflicts[0].original.as_str(),
            conflicts[0].current.as_str()
        ),
        ("description", "Before", "Changed")
    );
    assert_eq!(conflicts[0].proposed, "After");
    set_description(factory, "Before");
    // another user may neither recheck nor run it, with the session's id or without one
    assert_eq!(
        sessions.recheck(&id, "EX_JANE", &write).unwrap_err(),
        ScreenError::NotFound
    );

    let grant = ApprovalGrant {
        approval_id: "AGS_AP1".to_string(),
        args_hash: write.args_hash.clone(),
        agent_id: "assist".to_string(),
        run_id: Some("AGS_RUN".to_string()),
    };
    assert_eq!(
        sessions.execute_approved(&id, "EX_JANE", &write, &grant).unwrap_err(),
        ScreenError::NotFound
    );
    assert_eq!(
        sessions
            .execute_approved("gone", "EX_JANE", &write, &grant)
            .unwrap_err(),
        ScreenError::NotFound
    );
    assert_eq!(description(factory), "Before");
    let mut tampered = write.clone();
    tampered.fields.insert("description".to_string(), "Other".to_string());
    assert!(
        matches!(
            sessions.execute_approved(&id, OWNER, &tampered, &grant),
            Err(ScreenError::Refused(_))
        ),
        "another value is another write"
    );
    assert_eq!(description(factory), "Before");
    let done = sessions.execute_approved(&id, OWNER, &write, &grant).unwrap();
    assert!(done.ok, "{}", done.text);
    assert_eq!(description(factory), "After", "the approved write ran");
    assert!(done.state.values.is_empty(), "nothing left unsaved: {:?}", done.state);
    sessions.close(&id);
    assert!(sessions.find(OWNER, "AGS_C1").is_none());
}

/// The session is gone (reaped, another node): the approved write runs on a link of its own.
fn run_on_a_fresh_link(factory: &Arc<ExecutionContextFactory>, sessions: &dyn ScreenSessions) {
    let id = sessions.open(&spec("AGS_C4", false)).unwrap();
    let saved = sessions
        .command(
            &id,
            OWNER,
            &ScreenCommand {
                set: vec![("Description".to_string(), "Fresh".to_string())],
                press: Some("Save".to_string()),
                ..command()
            },
        )
        .unwrap();
    let write = saved.held.unwrap_or_else(|| panic!("held: {}", saved.text));
    sessions.close(&id);
    assert!(sessions.find(OWNER, "AGS_C4").is_none());
    let grant = ApprovalGrant {
        approval_id: "AGS_AP2".to_string(),
        args_hash: write.args_hash.clone(),
        agent_id: "assist".to_string(),
        run_id: None,
    };
    assert!(
        sessions.recheck(&id, OWNER, &write).unwrap().is_empty(),
        "rechecked with no session"
    );
    let done = sessions.execute_approved(&id, OWNER, &write, &grant).unwrap();
    assert!(done.ok, "{}", done.text);
    assert_eq!(description(factory), "Fresh", "the approved write ran with no session");
}

/// A drop-down's lookup transition with actions and no `read-only` answers its choices rather than being held; one
/// that writes fails at the fence as a failed lookup, and neither is a write waiting for approval.
fn lookups_run_fenced(factory: &Arc<ExecutionContextFactory>, sessions: &dyn ScreenSessions) {
    let id = sessions.open(&spec("AGS_C3", false)).unwrap();
    let chosen = sessions
        .command(
            &id,
            OWNER,
            &ScreenCommand {
                set: vec![("Lookup Type".to_string(), "AGT_SCREEN".to_string())],
                ..command()
            },
        )
        .unwrap();
    assert!(chosen.ok, "{}", chosen.text);
    assert!(chosen.held.is_none(), "a lookup is no pending write");
    assert_eq!(chosen.state.values["LookupForm"]["lookupType"], "AGT_SCREEN");
    let refused = sessions
        .command(
            &id,
            OWNER,
            &ScreenCommand {
                set: vec![("Writing Lookup".to_string(), "X".to_string())],
                ..command()
            },
        )
        .unwrap();
    assert!(!refused.ok, "{}", refused.text);
    assert!(
        refused.held.is_none(),
        "a lookup that writes is no pending write: {}",
        refused.text
    );
    assert_eq!(
        enum_type(factory, "AGT_LOOKUP_WROTE"),
        None,
        "the fence stopped its write"
    );
    sessions.close(&id);
}

/// The browser's path and typed values come into the session; the session's own changes answer follow events.
fn synced_and_followed(sessions: &dyn ScreenSessions) {
    let id = sessions.open(&spec("AGS_C2", true)).unwrap();
    let state = BrowserState {
        path: format!("{PATH}/Edit"),
        parameters: BTreeMap::new(),
        forms: vec![BrowserForm {
            dom_id: "EditEnumType".to_string(),
            values: BTreeMap::from([
                ("description".to_string(), "Typed".to_string()),
                ("enumTypeId".to_string(), "AGT_SCREEN".to_string()),
            ]),
            dirty: vec!["description".to_string()],
        }],
        dialogs: Vec::new(),
    };
    assert!(sessions.sync(&id, OWNER, &state).unwrap().is_empty());
    let look = sessions.command(&id, OWNER, &command()).unwrap();
    assert!(look.text.contains("Typed"), "{}", look.text);
    assert_eq!(look.state.values["EditEnumType"]["description"], "Typed");

    let set = sessions
        .command(
            &id,
            OWNER,
            &ScreenCommand {
                set: vec![("Description".to_string(), "Followed".to_string())],
                ..command()
            },
        )
        .unwrap();
    let fill = set
        .follow
        .iter()
        .find(|event| event.event_type == "ui.fill")
        .unwrap_or_else(|| panic!("{:?}", set.follow));
    assert_eq!(fill.payload["form"], "EditEnumType");
    assert_eq!(fill.payload["domId"], "EditEnumType");
    assert_eq!(fill.payload["occurrence"], 0);
    assert_eq!(fill.payload["field"], "description");
    assert_eq!(fill.payload["value"], "Followed");
    assert_eq!(fill.payload["label"], "Description");

    let went = sessions
        .command(
            &id,
            OWNER,
            &ScreenCommand {
                go: Some(format!("{PATH}/Other")),
                ..command()
            },
        )
        .unwrap();
    assert!(went.ok, "{}", went.text);
    let navigate = went
        .follow
        .iter()
        .find(|event| event.event_type == "ui.navigate")
        .unwrap_or_else(|| panic!("{:?}", went.follow));
    assert!(
        navigate.payload["path"].as_str().unwrap().ends_with("/Other"),
        "{navigate:?}"
    );
    sessions.close(&id);
}

/// A drawer run of the scripted fake provider: a `go` to a transition is refused, a save is held for approval and the
/// row stays, the follow events are stored events of the run, and the conversation keeps the screen state.
fn a_run_uses_the_screen_tool(factory: &Arc<ExecutionContextFactory>) {
    let tool = |id: &str, arguments: serde_json::Value| Step::Tool {
        id: id.to_string(),
        name: "screen".to_string(),
        arguments: arguments.to_string(),
        pieces: 2,
    };
    fake::clear(ENDPOINT);
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::steps(vec![
                tool("s1", json!({"go": format!("{PATH}/updateEnumType")})),
                tool(
                    "s2",
                    json!({"go": format!("{PATH}/Edit"), "set": {"Description": "From the run"}, "do": "Save"}),
                ),
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("The change waits for your approval."),
        ],
    );
    let before = description(factory);
    let context = factory.new_context();
    let _entered = context.enter();
    let started = moqui_core::llm::run::start_run(
        factory,
        &moqui_core::llm::run::StartRequest {
            user_id: OWNER.to_string(),
            text: "Change the description".to_string(),
            surface: moqui_core::llm::prompt::Surface::Drawer,
            ..moqui_core::llm::run::StartRequest::default()
        },
    )
    .unwrap();
    let state = moqui_core::llm::run::wait_for(factory, &started.run_id, Duration::from_secs(60)).unwrap();
    assert_eq!(
        state,
        moqui_core::llm::run::state::RunState::InputRequired,
        "the save waits for its approval"
    );
    assert_eq!(description(factory), before, "the run's save was held");
    // the user rejects it: the feedback is the result, and nothing is saved
    let rejected = answer_approval(
        factory,
        &started.run_id,
        OWNER,
        &ScriptedApproval {
            kind: "reject".to_string(),
            feedback: "Not now".to_string(),
            ..ScriptedApproval::default()
        },
    );
    assert_eq!(rejected, ApprovalApplied::Applied);
    let state = moqui_core::llm::run::wait_for(factory, &started.run_id, Duration::from_secs(60)).unwrap();
    assert_eq!(state, moqui_core::llm::run::state::RunState::Completed);
    assert_eq!(description(factory), before, "a rejected save runs nothing");

    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let results: BTreeMap<String, String> = facade
        .find("moqui.agent.AgentToolCall")
        .unwrap()
        .eq("runId", Value::string(&started.run_id))
        .list()
        .unwrap()
        .values()
        .iter()
        .map(|row| {
            (
                row.get("providerCallId").map(to_plain_string).unwrap_or_default(),
                row.get("resultSummary").map(to_plain_string).unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(results.len(), 2, "{results:?}");
    let (refused, held) = (&results["s1"], &results["s2"]);
    assert!(
        refused.contains("is an action (updateEnumType), not a screen"),
        "{refused}"
    );
    assert!(refused.contains("--- error"), "{refused}");
    assert!(held.contains("only task: Not now"), "{held}");
    let approval = facade
        .find("moqui.agent.AgentApproval")
        .unwrap()
        .eq("runId", Value::string(&started.run_id))
        .one()
        .unwrap()
        .unwrap();
    let snapshot: serde_json::Value =
        serde_json::from_str(&to_plain_string(approval.get("snapshotJson").unwrap())).unwrap();
    assert_eq!(snapshot["changes"][0]["current"], before.as_str(), "{snapshot}");
    assert_eq!(snapshot["changes"][0]["proposed"], "From the run", "{snapshot}");
    assert_eq!(snapshot["handoff"]["form"], "EditEnumType", "{snapshot}");
    assert_eq!(to_plain_string(approval.get("statusId").unwrap()), "AgaRejected");

    let events = moqui_core::llm::run::events::stored_after(&facade, &started.run_id, 0, 1000).unwrap();
    let types: Vec<&str> = events.iter().map(|event| event.event_type.as_str()).collect();
    assert!(types.contains(&"ui.fill"), "{types:?}");
    let fill = events.iter().find(|event| event.event_type == "ui.fill").unwrap();
    assert_eq!(fill.payload["field"], "description", "{:?}", fill.payload);

    let conversation = facade
        .find("moqui.agent.AgentConversation")
        .unwrap()
        .eq("conversationId", Value::string(&started.conversation_id))
        .one()
        .unwrap()
        .unwrap();
    let kept: screen::ScreenState =
        serde_json::from_str(&to_plain_string(conversation.get("screenStateJson").unwrap())).unwrap();
    assert!(kept.path.ends_with("/Edit"), "{kept:?}");
    assert_eq!(kept.values["EditEnumType"]["description"], "From the run");
    let sent = fake::requests(ENDPOINT);
    let tools: Vec<&str> = sent[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(tools.contains(&"screen"), "the screen tool is offered: {tools:?}");
}

const WAIT: Duration = Duration::from_secs(60);

fn save_step(description: &str) -> fake::Reply {
    fake::Reply::steps(vec![
        Step::Tool {
            id: "w1".to_string(),
            name: "screen".to_string(),
            arguments: json!({"go": format!("{PATH}/Edit"), "set": {"Description": description}, "do": "Save"}).to_string(),
            pieces: 1,
        },
        Step::Finish("tool_calls".to_string()),
    ])
}

/// A drawer run of `user_id` that saves `description` on the Edit screen and then answers: its id once it settled.
fn run_saving(factory: &Arc<ExecutionContextFactory>, user_id: &str, description: &str) -> (String, RunState) {
    fake::clear(ENDPOINT);
    fake::script(ENDPOINT, vec![save_step(description), fake::Reply::text("Done.")]);
    let context = factory.new_context();
    let _entered = context.enter();
    let started = moqui_core::llm::run::start_run(
        factory,
        &moqui_core::llm::run::StartRequest {
            user_id: user_id.to_string(),
            text: "Change the description".to_string(),
            surface: moqui_core::llm::prompt::Surface::Drawer,
            ..moqui_core::llm::run::StartRequest::default()
        },
    )
    .unwrap();
    let state = moqui_core::llm::run::wait_for(factory, &started.run_id, WAIT).unwrap();
    (started.run_id, state)
}

fn row_of(factory: &Arc<ExecutionContextFactory>, entity: &str, field: &str, value: &str) -> Option<moqui_core::entity::EntityValue> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    factory
        .entity()
        .unwrap()
        .find(entity)
        .unwrap()
        .eq(field, Value::string(value))
        .one()
        .unwrap()
}

fn field(row: &moqui_core::entity::EntityValue, name: &str) -> String {
    row.get(name).map(to_plain_string).unwrap_or_default()
}

fn pause_of(factory: &Arc<ExecutionContextFactory>, run_id: &str) -> (String, String) {
    let run = row_of(factory, "moqui.agent.AgentRun", "runId", run_id).unwrap();
    (field(&run, "pauseKindEnumId"), field(&run, "pauseRef"))
}

/// The result the model read for the run's screen call.
fn screen_result(factory: &Arc<ExecutionContextFactory>, run_id: &str) -> String {
    let call = row_of(factory, "moqui.agent.AgentToolCall", "runId", run_id).unwrap();
    field(&call, "resultSummary")
}

/// The owner's browser session: its cookie and the session token of a page.
fn browser(address: std::net::SocketAddr) -> (String, String) {
    let cookie = sign_in(address, "john.doe", "moqui");
    let (status, headers, html) = exchange(
        address,
        &http_request("GET", &format!("{PATH}/Other.html"), &[("Cookie", &cookie)], ""),
    );
    assert_eq!(status, 200, "{html}");
    let token = session_token(&html).expect("the page carries the session token");
    (merge_cookies(&cookie, &headers), token)
}

fn post(address: std::net::SocketAddr, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut all = vec![
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Accept", "application/json"),
    ];
    all.extend_from_slice(headers);
    let (status, _, text) = exchange(address, &http_request("POST", path, &all, body));
    (status, text)
}

/// A held save pauses the run; a decision over HTTP is refused for a header-authenticated request and taken from the
/// owner's browser session with its token, and the run then saves the exact request.
fn a_decision_over_http_saves_the_held_write(factory: &Arc<ExecutionContextFactory>, address: std::net::SocketAddr) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, OWNER, "Approved over HTTP");
    assert_eq!(state, RunState::InputRequired);
    let (kind, approval_id) = pause_of(factory, &run_id);
    assert_eq!(kind, "AgpsApproval");
    assert_eq!(description(factory), "Before");
    let decide = format!("{PATH}/Decide/decide");
    let body = format!("approvalId={approval_id}&decision=approve");
    // Basic authentication is a header: no person decided
    let authorization = basic("john.doe", "moqui");
    let (status, text) = post(address, &decide, &[("Authorization", &authorization)], &body);
    assert!(text.contains("decided only by its user"), "{status} {text}");
    let pending = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&pending, "statusId"), "AgaPending");
    // the browser session without its token is refused before the service runs
    let (cookie, token) = browser(address);
    let (status, text) = post(address, &decide, &[("Cookie", &cookie)], &body);
    assert_eq!(status, 401, "{text}");
    // with it, the owner's decision is taken and the run saves
    let (status, text) = post(
        address,
        &decide,
        &[("Cookie", &cookie)],
        &format!("{body}&moquiSessionToken={token}"),
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("AgaApproved"), "{text}");
    assert_eq!(moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(factory), "Approved over HTTP", "the approved write ran");
    let done = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&done, "statusId"), "AgaExecuted");
    assert_eq!(field(&done, "decisionChannel"), "drawer");
    assert!(
        screen_result(factory, &run_id).starts_with("The user approved updateEnumType, and it was saved."),
        "{}",
        screen_result(factory, &run_id)
    );
    // a second decision is refused
    let (_, text) = post(
        address,
        &decide,
        &[("Cookie", &cookie)],
        &format!("{body}&moquiSessionToken={token}"),
    );
    assert!(text.contains("decided already"), "{text}");
}

/// The row changed between the proposal and the approval: a conflict, nothing saved.
fn a_conflict_saves_nothing(factory: &Arc<ExecutionContextFactory>) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, OWNER, "Late write");
    assert_eq!(state, RunState::InputRequired);
    let (_, approval_id) = pause_of(factory, &run_id);
    set_description(factory, "Changed meanwhile");
    let approve = ScriptedApproval {
        kind: "approve".to_string(),
        ..ScriptedApproval::default()
    };
    assert_eq!(answer_approval(factory, &run_id, OWNER, &approve), ApprovalApplied::Applied);
    assert_eq!(moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(factory), "Changed meanwhile", "a conflict saves nothing");
    let done = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&done, "statusId"), "AgaConflict");
    let result = screen_result(factory, &run_id);
    assert!(
        result.contains("changed after updateEnumType was proposed") && result.contains("Description is Changed meanwhile"),
        "{result}"
    );
}

/// Edit in screen: the user's page is filled, the run waits, and the user's own browser save is what it reads.
fn an_edit_in_screen_is_the_users_own_save(factory: &Arc<ExecutionContextFactory>, address: std::net::SocketAddr) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, OWNER, "Proposed");
    assert_eq!(state, RunState::InputRequired);
    let (_, approval_id) = pause_of(factory, &run_id);
    let (cookie, token) = browser(address);
    let (status, text) = post(
        address,
        &format!("{PATH}/Decide/decide"),
        &[("Cookie", &cookie)],
        &format!("approvalId={approval_id}&decision=edit&moquiSessionToken={token}"),
    );
    assert_eq!(status, 200, "{text}");
    assert!(text.contains("AgaHandedOff"), "{text}");
    let (kind, handoff_id) = pause_of(factory, &run_id);
    assert_eq!(kind, "AgpsHandoff");
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let events = moqui_core::llm::run::events::stored_after(&facade, &run_id, 0, 1000).unwrap();
        let fill = events
            .iter()
            .find(|event| event.event_type == "ui.fill" && event.payload["handoffId"] == handoff_id.as_str())
            .unwrap_or_else(|| panic!("{events:?}"));
        assert_eq!(fill.payload["form"], "EditEnumType");
        assert_eq!(fill.payload["value"], "Proposed");
        assert!(events.iter().any(|event| event.event_type == "handoff.started"));
    }
    // the user's save with an empty token is refused and is no outcome; then the user's own save in the browser
    let save = format!("{PATH}/Edit/updateEnumType");
    let (status, _) = post(address, &save, &[("Cookie", &cookie)], "enumTypeId=AGT_SCREEN&description=No+token");
    assert_eq!(status, 401);
    // a save of another record through the same screen and transition (another tab) is not this hand-off's
    let (status, text) = post(
        address,
        &save,
        &[("Cookie", &cookie)],
        &format!("enumTypeId=AGT_OTHER&description=Another+tab&moquiSessionToken={token}"),
    );
    assert!(status < 400, "{status} {text}");
    // nor a save of the right record by a program with a header login
    let authorization = basic("john.doe", "moqui");
    let (status, text) = post(
        address,
        &save,
        &[("Authorization", &authorization)],
        "enumTypeId=AGT_SCREEN&description=By+a+program",
    );
    assert!(status < 400, "{status} {text}");
    let watching = row_of(factory, "moqui.agent.AgentHandoff", "handoffId", &handoff_id).unwrap();
    assert_eq!(field(&watching, "statusId"), "AghWatching", "neither save is the user's of this record");
    assert_eq!(pause_of(factory, &run_id).0, "AgpsHandoff");
    let (status, text) = post(
        address,
        &save,
        &[("Cookie", &cookie)],
        &format!("enumTypeId=AGT_SCREEN&description=Saved+by+the+user&moquiSessionToken={token}"),
    );
    assert!(status < 400, "{status} {text}");
    assert_eq!(moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(factory), "Saved by the user");
    let watch = row_of(factory, "moqui.agent.AgentHandoff", "handoffId", &handoff_id).unwrap();
    assert_eq!(field(&watch, "statusId"), "AghSaved");
    let done = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&done, "statusId"), "AgaExecuted");
    let result = screen_result(factory, &run_id);
    assert!(
        result.contains("saved this write themselves") && result.contains("Description: Saved by the user (you proposed Proposed)"),
        "{result}"
    );
}

/// An Edit in screen the user never saves is abandoned at its expiry, and the run goes on with that.
fn an_edit_in_screen_left_is_abandoned(factory: &Arc<ExecutionContextFactory>, address: std::net::SocketAddr) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, OWNER, "Never saved");
    assert_eq!(state, RunState::InputRequired);
    let (_, approval_id) = pause_of(factory, &run_id);
    let (cookie, token) = browser(address);
    let (status, text) = post(
        address,
        &format!("{PATH}/Decide/decide"),
        &[("Cookie", &cookie)],
        &format!("approvalId={approval_id}&decision=edit&moquiSessionToken={token}"),
    );
    assert!(status == 200 && text.contains("AgaHandedOff"), "{status} {text}");
    let (_, handoff_id) = pause_of(factory, &run_id);
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let facade = factory.entity().unwrap();
        let mut run = facade
            .find("moqui.agent.AgentRun")
            .unwrap()
            .eq("runId", Value::string(&run_id))
            .one()
            .unwrap()
            .unwrap();
        run.set(
            "pauseExpireDate",
            Value::Timestamp(jiff::Timestamp::now() - jiff::SignedDuration::from_secs(5)),
        );
        run.update(&facade).unwrap();
    }
    assert!(moqui_core::llm::run::lease::expire_pauses(factory).unwrap() >= 1);
    assert_eq!(moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(factory), "Before");
    let watch = row_of(factory, "moqui.agent.AgentHandoff", "handoffId", &handoff_id).unwrap();
    assert_eq!(field(&watch, "statusId"), "AghAbandoned");
    let done = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&done, "statusId"), "AgaExpired");
    assert!(screen_result(factory, &run_id).contains("did not save it (expired)"));
}

/// A user who may view the screen but not run its save gets an error, not a card, and nothing is saved.
fn no_approval_for_what_the_user_may_not_do(factory: &Arc<ExecutionContextFactory>) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, "AGS_VIEWER", "Viewer write");
    assert_eq!(state, RunState::Completed, "no approval to wait for");
    assert_eq!(description(factory), "Before");
    let result = screen_result(factory, &run_id);
    assert!(result.contains("not allowed to run updateEnumType"), "{result}");
    assert!(row_of(factory, "moqui.agent.AgentApproval", "runId", &run_id).is_none());
}

/// The card's Preview effects runs the held write in the sim: the row it would change, against production, and
/// production unchanged.
fn a_held_write_previews_in_the_sim(factory: &Arc<ExecutionContextFactory>) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, OWNER, "Previewed");
    assert_eq!(state, RunState::InputRequired);
    let (_, approval_id) = pause_of(factory, &run_id);
    let facade = factory.entity().unwrap();
    if !moqui_core::llm::approval::preview::available(&facade) {
        eprintln!("the sim overlay is not available here, so the preview is not checked");
        let _ = moqui_core::llm::run::cancel_run(factory, &run_id, Some(OWNER));
        return;
    }
    let preview = moqui_core::llm::approval::preview::preview_approval(factory, &approval_id, OWNER).unwrap();
    assert_eq!(preview["ok"], true, "{preview}");
    let row = preview["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["entity"] == "moqui.basic.EnumerationType")
        .unwrap_or_else(|| panic!("{preview}"));
    assert_eq!(row["action"], "update", "{preview}");
    assert_eq!(row["key"]["enumTypeId"], "AGT_SCREEN");
    let change = row["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["field"] == "description")
        .unwrap_or_else(|| panic!("{preview}"));
    assert_eq!((change["before"].as_str(), change["after"].as_str()), (Some("Before"), Some("Previewed")));
    assert_eq!(description(factory), "Before", "the preview wrote nothing to production");
    assert_eq!(
        moqui_core::llm::approval::preview::preview_approval(factory, &approval_id, "EX_JANE"),
        Err(moqui_core::llm::approval::DecideError::NotFound)
    );
    let _ = moqui_core::llm::run::cancel_run(factory, &run_id, Some(OWNER));
}

/// A DESTRUCTIVE service tool (`run_tool` of an exposed entity-auto delete) is previewed in the sim before its card is
/// shown: the approval carries the row it would delete, and production keeps it; a rejection then runs nothing.
fn a_destructive_service_previews_before_its_card(factory: &Arc<ExecutionContextFactory>) {
    fake::clear(ENDPOINT);
    fake::script(
        ENDPOINT,
        vec![
            fake::Reply::steps(vec![
                Step::Tool {
                    id: "d1".to_string(),
                    name: "run_tool".to_string(),
                    arguments: json!({"name": "delete_enum_type", "parameters": {"enumTypeId": "AGT_DELETE"}}).to_string(),
                    pieces: 1,
                },
                Step::Finish("tool_calls".to_string()),
            ]),
            fake::Reply::text("Left it."),
        ],
    );
    let run_id = {
        let context = factory.new_context();
        let _entered = context.enter();
        moqui_core::llm::run::start_run(
            factory,
            &moqui_core::llm::run::StartRequest {
                user_id: OWNER.to_string(),
                text: "Remove the enumeration type AGT_DELETE".to_string(),
                surface: moqui_core::llm::prompt::Surface::Drawer,
                ..moqui_core::llm::run::StartRequest::default()
            },
        )
        .unwrap()
        .run_id
    };
    let state = moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap();
    assert_eq!(state, RunState::InputRequired, "{}", screen_result(factory, &run_id));
    let (_, approval_id) = pause_of(factory, &run_id);
    let approval = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&approval, "classificationEnumId"), "AgclDestructive");
    assert_eq!(field(&approval, "targetName"), "delete#moqui.basic.EnumerationType");
    let facade = factory.entity().unwrap();
    if moqui_core::llm::approval::preview::available(&facade) {
        let preview: serde_json::Value = serde_json::from_str(&field(&approval, "previewJson")).unwrap();
        assert_eq!(preview["ok"], true, "{preview}");
        let row = preview["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["entity"] == "moqui.basic.EnumerationType")
            .unwrap_or_else(|| panic!("{preview}"));
        assert_eq!((row["action"].as_str(), row["key"]["enumTypeId"].as_str()), (Some("delete"), Some("AGT_DELETE")));
    } else {
        eprintln!("the sim overlay is not available here, so the destructive preview is not checked");
        assert!(field(&approval, "previewJson").is_empty());
    }
    assert!(
        row_of(factory, "moqui.basic.EnumerationType", "enumTypeId", "AGT_DELETE").is_some(),
        "the preview deleted nothing in production"
    );
    let context = factory.new_context();
    let _entered = context.enter();
    let rejected = answer_approval(
        factory,
        &run_id,
        OWNER,
        &ScriptedApproval {
            kind: "reject".to_string(),
            feedback: "Keep it.".to_string(),
            ..ScriptedApproval::default()
        },
    );
    assert!(matches!(rejected, ApprovalApplied::Applied), "{rejected:?}");
    assert_eq!(moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(row_of(factory, "moqui.basic.EnumerationType", "enumTypeId", "AGT_DELETE").is_some());
}

/// A session a header login made (Basic here; a key is the same) carries a cookie and a session token, but it is a
/// program's: its cookie-and-token decision is refused, while the owner's form login decides.
fn a_header_login_session_never_decides(factory: &Arc<ExecutionContextFactory>, address: std::net::SocketAddr) {
    set_description(factory, "Before");
    let (run_id, state) = run_saving(factory, OWNER, "Keyed");
    assert_eq!(state, RunState::InputRequired);
    let (_, approval_id) = pause_of(factory, &run_id);
    let authorization = basic("john.doe", "moqui");
    let (status, headers, html) = exchange(
        address,
        &http_request("GET", &format!("{PATH}/Other.html"), &[("Authorization", &authorization)], ""),
    );
    assert_eq!(status, 200, "{html}");
    let cookie = merge_cookies("", &headers);
    let token = session_token(&html).expect("the page carries the session token");
    assert!(!cookie.is_empty(), "the header login got a session cookie");
    let decide = format!("{PATH}/Decide/decide");
    let (_, text) = post(
        address,
        &decide,
        &[("Cookie", &cookie)],
        &format!("approvalId={approval_id}&decision=approve&moquiSessionToken={token}"),
    );
    assert!(text.contains("decided only by its user"), "{text}");
    let pending = row_of(factory, "moqui.agent.AgentApproval", "approvalId", &approval_id).unwrap();
    assert_eq!(field(&pending, "statusId"), "AgaPending");
    let (cookie, token) = browser(address);
    let (status, text) = post(
        address,
        &decide,
        &[("Cookie", &cookie)],
        &format!("approvalId={approval_id}&decision=reject&feedback=No&moquiSessionToken={token}"),
    );
    assert!(status == 200 && text.contains("AgaRejected"), "{status} {text}");
    assert_eq!(moqui_core::llm::run::wait_for(factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(factory), "Before");
}

/// A pending approval of `service` with `args`, as a service tool asks for one.
fn service_approval(factory: &Arc<ExecutionContextFactory>, id: &str, service: &str, args: serde_json::Value) {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let mut row = facade.make_value("moqui.agent.AgentApproval").unwrap();
    for (name, value) in [
        ("approvalId", id.to_string()),
        ("runId", format!("{id}_RUN")),
        ("toolCallId", format!("{id}_CALL")),
        ("userId", OWNER.to_string()),
        ("agentId", "assist".to_string()),
        ("classificationEnumId", "AgclDestructive".to_string()),
        ("targetKind", "service".to_string()),
        ("targetName", service.to_string()),
        ("argumentsJson", args.to_string()),
        ("argumentsHash", "unused".to_string()),
        ("statusId", "AgaPending".to_string()),
    ] {
        row.set(name, Value::string(value));
    }
    row.create(&facade).unwrap();
}

/// The preview of a service that deletes a file, or sends a notification, writes its rows only to the sim and does
/// neither for real: the effect is refused and the preview says it cannot be simulated.
fn a_preview_reaches_nothing_outside_the_sim(factory: &Arc<ExecutionContextFactory>) {
    let facade = factory.entity().unwrap();
    if !moqui_core::llm::approval::preview::available(&facade) {
        eprintln!("the sim overlay is not available here, so the preview's fence is not checked");
        return;
    }
    let runtime = factory.runtime_dir();
    let kept = runtime.join("tmp/agent-screens-keep.txt");
    std::fs::create_dir_all(kept.parent().unwrap()).unwrap();
    std::fs::write(&kept, "keep me").unwrap();
    service_approval(
        factory,
        "AGS_PREVIEW_SWEEP",
        "agentscreens.AgentScreensServices.sweep#Files",
        json!({"location": "runtime://tmp/agent-screens-keep.txt"}),
    );
    let swept = moqui_core::llm::approval::preview::preview_approval(factory, "AGS_PREVIEW_SWEEP", OWNER).unwrap();
    assert_eq!(swept["ok"], false, "{swept}");
    let error = swept["error"].as_str().unwrap_or_default();
    assert!(error.contains("a resource write") && error.contains("agent-screens-keep.txt"), "{swept}");
    assert!(kept.exists(), "the preview deleted a real file");
    assert_eq!(enum_type(factory, "AGT_SWEPT"), None, "the sim's row is not in production");

    service_approval(factory, "AGS_PREVIEW_NOTE", "agentscreens.AgentScreensServices.announce#Things", json!({}));
    let announced = moqui_core::llm::approval::preview::preview_approval(factory, "AGS_PREVIEW_NOTE", OWNER).unwrap();
    assert_eq!(announced["ok"], false, "{announced}");
    assert!(
        announced["error"].as_str().unwrap_or_default().contains("a notification (AgentScreensTest)"),
        "{announced}"
    );
    assert_eq!(enum_type(factory, "AGT_ANNOUNCED"), None);
    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let sent = facade
            .find("moqui.security.user.NotificationMessage")
            .unwrap()
            .eq("topic", Value::string("AgentScreensTest"))
            .list()
            .unwrap();
        assert!(sent.is_empty(), "the preview published a notification");
    }
    let _ = std::fs::remove_file(&kept);
}
