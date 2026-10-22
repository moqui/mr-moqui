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

//! The lazy catalog and its tools (plans/LLM-A2A-Assist.md 4.6, 7.3, 13.4) against the scripted fake provider on the
//! in-memory database: exposed services and an unpinned add-on tool in the catalog and not in the tool list,
//! `find_tools`, `describe_tools` for several names with did-you-mean, a direct call of a catalog name turned into
//! guidance, `run_tool` with a READ that runs fenced and a mislabeled READ that fails, a WRITE that waits for its
//! approval and runs with its arguments once approved, edited arguments checked again, a conflict, a DESTRUCTIVE call's
//! approval, the exposure's `lockVersion` (the admin service refuses a stale edit, and an approval of an older version
//! runs nothing), and a user-facts provider's fact in a conversation's prefix. The sim preview of a DESTRUCTIVE call
//! is checked on embedded Postgres (`crates/moqui-host/tests/it/agent_screens.rs`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use super::catalog::{self, Catalog};
use super::lazy::{describe_text, find_text};
use super::schema::{object, Property};
use super::tests::TEST_CAP;
use super::*;
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::llm::approval::{self, ApprovalStatus, Channel, Decider, Decision, TargetKind};
use crate::llm::provider::fake::{self, Step};
use crate::llm::run::{self, RunState, StartRequest};

const USER: &str = "lazy.tester";
const WAIT: Duration = Duration::from_secs(15);

/// A lazy add-on tool: in the catalog, not in the tool list, unless the agent pins it.
pub(crate) struct TestLazy(ToolSpec);

impl Default for TestLazy {
    fn default() -> Self {
        let mut spec = ToolSpec::new(
            "test_lazy",
            "Counts the letters of a word.",
            object(&[Property::required("word", "string", "")]),
            Classification::Read,
        )
        .needs(&[TEST_CAP]);
        spec.lazy = true;
        Self(spec)
    }
}

impl AgentTool for TestLazy {
    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    fn call(&self, _ctx: &mut crate::llm::run::context::TurnContext<'_>, args: Json) -> ToolOutcome {
        let word = args.get("word").and_then(Json::as_str).unwrap_or_default();
        ToolOutcome::text(format!("{word} has {} letters", word.chars().count()))
    }
}

crate::submit_tool!(TestLazy);

/// A user-facts provider for [`FACTS_USER`] only, so no other test's prefix changes.
#[derive(Default)]
struct TestFacts;

const FACTS_USER: &str = "facts.tester";

impl crate::llm::facts::UserFactsProvider for TestFacts {
    fn name(&self) -> &str {
        "test"
    }

    fn facts(
        &self,
        _facade: &EntityFacade,
        user_id: &str,
        _agent_id: &str,
    ) -> Result<crate::llm::facts::UserFacts, moqui_lib::error::Error> {
        if user_id != FACTS_USER {
            return Ok(crate::llm::facts::UserFacts::default());
        }
        Ok(crate::llm::facts::UserFacts {
            facts: vec![
                ("active organization".to_string(), "Test Org (TEST_ORG)".to_string()),
                // the framework's label keeps its value
                ("name".to_string(), "Not The Name".to_string()),
            ],
            org_instructions: "Quote prices in euros.".to_string(),
        })
    }
}

crate::submit_user_facts!(TestFacts);

struct Bed {
    ctx: ExecutionContext,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
    endpoint: String,
}

fn bed(name: &str) -> Bed {
    let endpoint = format!("lazy-{name}");
    let conf = format!(
        r#"<moqui-conf><llm-facade>
            <endpoint id="{endpoint}" protocol="fake"/>
            <model id="{endpoint}" endpoint="{endpoint}" name="fake-1" context-window="32768"/>
            <bind feature="assist" model="{endpoint}" reasoning="none"/>
            <user-facts service="no.such.Services.get#UserFacts"/>
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
    for (id, full) in [(USER, "Lazy Tester"), (FACTS_USER, "Facts Tester")] {
        row(
            &facade,
            "moqui.security.UserAccount",
            &[("userId", id), ("username", id), ("userFullName", full)],
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
    row(
        &facade,
        "moqui.basic.EnumerationType",
        &[("enumTypeId", "LazyType"), ("description", "Lazy")],
    );
    row(
        &facade,
        "moqui.basic.Enumeration",
        &[
            ("enumId", "LAZY_E1"),
            ("enumTypeId", "LazyType"),
            ("description", "Old"),
        ],
    );
    row(
        &facade,
        "moqui.basic.Enumeration",
        &[
            ("enumId", "LAZY_E2"),
            ("enumTypeId", "LazyType"),
            ("description", "Other"),
        ],
    );
    for (tool, service, class, title) in [
        (
            "enum_find",
            "org.moqui.impl.BasicServices.find#Enumeration",
            "",
            "Find enumerations of a type",
        ),
        (
            "enum_update",
            "update#moqui.basic.Enumeration",
            "",
            "Change an enumeration",
        ),
        (
            "enum_delete",
            "delete#moqui.basic.Enumeration",
            "",
            "Remove an enumeration",
        ),
        // a write labeled a read by mistake: the fence stops it
        (
            "enum_sneaky",
            "create#moqui.basic.Enumeration",
            "AgclRead",
            "Look at an enumeration",
        ),
        // left out, each logged: a reserved name, a service that is not defined
        (
            "mcp__files__read",
            "org.moqui.impl.BasicServices.find#Enumeration",
            "",
            "Reserved",
        ),
        ("broken_tool", "no.such.Services.do#Nothing", "", "Broken"),
    ] {
        let mut fields = vec![
            ("toolName", tool),
            ("serviceName", service),
            ("title", title),
            ("description", title),
            ("lockVersion", "1"),
        ];
        if !class.is_empty() {
            fields.push(("classificationEnumId", class));
        }
        row(&facade, catalog::EXPOSURE, &fields);
    }
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

fn start(bed: &Bed, user: &str, replies: Vec<fake::Reply>) -> String {
    fake::script(&bed.endpoint, replies);
    run::start_run(
        &bed.factory,
        &StartRequest {
            user_id: user.to_string(),
            text: "Do it".to_string(),
            extra_caps: vec![TEST_CAP.to_string()],
            ..StartRequest::default()
        },
    )
    .unwrap()
    .run_id
}

/// A run that calls `run_tool` with `args` and is paused for its approval: the run and the approval.
fn paused(bed: &Bed, args: Json) -> (String, String) {
    let run_id = start(
        bed,
        USER,
        vec![tool_step("r1", "run_tool", args), fake::Reply::text("Done.")],
    );
    assert_eq!(
        run::wait_for(&bed.factory, &run_id, WAIT).unwrap(),
        RunState::InputRequired
    );
    let paused = run::load_run(&bed.facade, &run_id).unwrap().unwrap();
    assert_eq!(paused.pause_kind, Some(PauseKind::Approval));
    (run_id, paused.pause_ref.unwrap())
}

/// The tool results the model read, in order.
fn results(bed: &Bed) -> Vec<String> {
    let sent = fake::requests(&bed.endpoint);
    let messages = sent.last().unwrap()["messages"].as_array().unwrap().clone();
    messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn description(bed: &Bed, enum_id: &str) -> Option<String> {
    bed.facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string(enum_id))
        .one()
        .unwrap()
        .and_then(|row| crate::llm::text_of(&row, "description"))
}

fn decide(bed: &Bed, approval_id: &str, decision: &Decision) {
    approval::decide(
        &bed.factory,
        approval_id,
        &Decider::scripted(USER, Channel::Drawer),
        decision,
    )
    .unwrap();
}

fn catalog_of(bed: &Bed, policy: &ToolPolicy) -> Catalog {
    Catalog::for_run(
        &bed.facade,
        &ClientCaps::for_surface(crate::llm::prompt::Surface::Drawer).with([TEST_CAP]),
        policy,
        &[],
    )
}

#[test]
fn the_catalog_holds_exposed_services_and_unpinned_add_on_tools_never_the_tool_list() {
    let bed = bed("catalog");
    let _entered = bed.ctx.enter();
    let catalog = catalog_of(&bed, &ToolPolicy::default());
    let classes: Vec<(&str, Classification)> = catalog
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.classification))
        .collect();
    assert_eq!(
        classes,
        [
            ("enum_delete", Classification::Destructive),
            ("enum_find", Classification::Read),
            ("enum_sneaky", Classification::Read),
            ("enum_update", Classification::Write),
            // the framework's own add-on tool (phase 13.14)
            ("save_skill", Classification::Read),
            ("test_lazy", Classification::Read),
        ],
        "a reserved name and a service that is not defined are left out"
    );
    assert_eq!(
        catalog.get("update#moqui.basic.Enumeration").unwrap().name,
        "enum_update"
    );
    // the visible tools: the lazy three, never a catalog entry, unless the agent pins the add-on tool
    let caps = ClientCaps::for_surface(crate::llm::prompt::Surface::Drawer).with([TEST_CAP]);
    let names = |policy: &ToolPolicy| -> Vec<String> {
        offered(&caps, PromptVariant::Standard, policy, &[], &[])
            .iter()
            .map(|offered| offered.tool.spec().name.clone())
            .collect()
    };
    let plain = names(&ToolPolicy::default());
    assert!(["find_tools", "describe_tools", "run_tool"]
        .iter()
        .all(|name| plain.contains(&name.to_string())));
    assert!(
        !plain
            .iter()
            .any(|name| name.starts_with("enum_") || name == "test_lazy"),
        "{plain:?}"
    );
    for variant in [PromptVariant::Standard, PromptVariant::Small] {
        let tools = crate::llm::prompt::offered_tools(crate::llm::prompt::Surface::Drawer, variant);
        assert!(
            tools.iter().any(|tool| tool.name == "run_tool"),
            "the lazy tools are in the {variant:?} variant too"
        );
    }
    let pin = ToolPolicy {
        rules: vec![PolicyRule {
            sequence: 1,
            pattern: "test_lazy".to_string(),
            ceiling: None,
            mode: ApprovalMode::Ask,
            allow_destructive: false,
            calls_per_run: None,
        }],
    };
    assert!(
        names(&pin).contains(&"test_lazy".to_string()),
        "a pinned add-on tool is a tool of its own"
    );
    assert!(catalog_of(&bed, &pin).get("test_lazy").is_none());
    // an agent capped at READ may not run the writes, and find_tools does not list them
    let reads_only = ToolPolicy {
        rules: vec![PolicyRule {
            sequence: 1,
            pattern: "*".to_string(),
            ceiling: Some(Classification::Read),
            mode: ApprovalMode::Ask,
            allow_destructive: false,
            calls_per_run: None,
        }],
    };
    let capped = catalog_of(&bed, &reads_only);
    assert!(!capped.get("enum_update").unwrap().allowed);
    let found = find_text(&capped, "enumeration");
    assert!(
        found.contains("enum_find (read)") && !found.contains("enum_update"),
        "{found}"
    );

    // find_tools: the matches, one line each with read or write
    let found = find_text(&catalog, "change enumeration");
    let lines: Vec<&str> = found.lines().collect();
    assert_eq!(lines[1], "enum_update (write): Change an enumeration", "{found}");
    assert!(found.ends_with("then run_tool."), "{found}");
    // a DESTRUCTIVE tool needs a rule that allows it
    assert!(
        !catalog.get("enum_delete").unwrap().allowed && !found.contains("enum_delete"),
        "{found}"
    );
    let destructive = ToolPolicy {
        rules: vec![PolicyRule {
            sequence: 1,
            pattern: "enum_delete".to_string(),
            ceiling: None,
            mode: ApprovalMode::Ask,
            allow_destructive: true,
            calls_per_run: None,
        }],
    };
    let found = find_text(&catalog_of(&bed, &destructive), "remove enumeration");
    assert!(
        found.contains("enum_delete (write, destructive): Remove an enumeration"),
        "{found}"
    );
    assert!(find_text(&catalog, "zebra crossing").starts_with("No tool matches"));

    // describe_tools: several in one call, the parameters and results of each, did-you-mean for a name it lacks
    let described = describe_text(
        &catalog,
        &bed.facade,
        &[
            "enum_find".to_string(),
            "enum_update".to_string(),
            "enum_updat".to_string(),
        ],
    );
    assert!(
        described.contains("enum_find (read)\nFind enumerations of a type\nparameters:"),
        "{described}"
    );
    assert!(described.contains("- enumTypeId: string, required"), "{described}");
    assert!(described.contains("results: enumerationList (array)"), "{described}");
    assert!(
        described.contains("enum_update (write; waits for the user's approval)"),
        "{described}"
    );
    assert!(described.contains("- enumId: string, required"), "{described}");
    assert!(
        described.contains("enum_updat: {\"error\":\"unknown_tool\"")
            && described.contains("\"did_you_mean\":[\"enum_update\""),
        "{described}"
    );

    // a direct call of a catalog name is guidance to run_tool
    let lazy = lazy_names(&bed.facade);
    assert!(lazy.contains(&"enum_update".to_string()) && lazy.contains(&"update#moqui.basic.Enumeration".to_string()));
    assert!(!lazy.contains(&"mcp__files__read".to_string()));
    let direct = suggest::unknown_tool("enum_update", &["run_tool", "screen"], &lazy).to_text();
    assert!(
        direct.contains("\"lazy_tool\"") && direct.contains("Call run_tool with name"),
        "{direct}"
    );
}

#[test]
fn run_tool_reads_fenced_and_a_read_that_writes_fails() {
    let bed = bed("read");
    let _entered = bed.ctx.enter();
    let run_id = start(
        &bed,
        USER,
        vec![
            tool_step(
                "r1",
                "run_tool",
                json!({"name": "enum_find", "parameters": {"enumTypeId": "LazyType", "bogus": 1}}),
            ),
            tool_step(
                "r2",
                "run_tool",
                json!({"name": "enum_sneaky", "parameters": {"enumId": "LAZY_SNEAK", "enumTypeId": "LazyType", "description": "x"}}),
            ),
            tool_step("r3", "run_tool", json!({"name": "enum_fnd", "parameters": {}})),
            tool_step("r4", "enum_update", json!({"enumId": "LAZY_E1"})),
            tool_step(
                "r5",
                "run_tool",
                json!({"name": "test_lazy", "parameters": {"word": "lazy"}}),
            ),
            tool_step("r6", "run_tool", json!({"name": "enum_find", "parameters": {}})),
            fake::Reply::text("Done."),
        ],
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    let results = results(&bed);
    assert_eq!(results.len(), 6, "{results:#?}");
    assert!(
        results[0].contains("\"ok\":true")
            && results[0].contains("\"result\":{\"enumerationList\":[{")
            && results[0].contains("\"enumId\":\"LAZY_E1\"")
            && results[0].contains("bogus (not a parameter of enum_find)"),
        "{}",
        results[0]
    );
    assert!(
        results[1].contains("\"read_tool_wrote\"") && results[1].contains("nothing was changed"),
        "{}",
        results[1]
    );
    assert!(
        description(&bed, "LAZY_SNEAK").is_none(),
        "the mislabeled read wrote nothing"
    );
    assert!(results[2].contains("\"did_you_mean\":[\"enum_find\""), "{}", results[2]);
    assert!(
        results[3].contains("\"lazy_tool\"") && results[3].contains("run_tool"),
        "{}",
        results[3]
    );
    assert_eq!(results[4], "lazy has 4 letters");
    // the precheck: a parameter the service requires is missing, so nothing runs and no approval is asked
    assert!(
        results[5].contains("\"arguments\"") && results[5].contains("enumTypeId"),
        "{}",
        results[5]
    );
    let approvals = bed
        .facade
        .find("moqui.agent.AgentApproval")
        .unwrap()
        .disable_authz()
        .list()
        .unwrap();
    assert!(approvals.values().is_empty());
}

#[test]
fn a_write_waits_for_its_approval_and_runs_with_its_arguments() {
    let bed = bed("write");
    let _entered = bed.ctx.enter();
    // invalid parameters are an error the model fixes, never a card
    let run_id = start(
        &bed,
        USER,
        vec![
            tool_step(
                "r0",
                "run_tool",
                json!({"name": "enum_update", "parameters": {"description": "New"}}),
            ),
            fake::Reply::text("I need the id."),
        ],
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(results(&bed)[0].contains("enumId"), "{:?}", results(&bed));

    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "New"}}),
    );
    let pending = approval::load(&bed.facade, &approval_id).unwrap().unwrap();
    assert_eq!(pending.status, ApprovalStatus::Pending);
    assert_eq!(pending.target_kind, TargetKind::Service);
    assert_eq!(pending.target_name, "update#moqui.basic.Enumeration");
    assert_eq!(pending.classification, Classification::Write);
    assert_eq!(pending.args["description"], "New");
    let changes = pending.changes();
    assert_eq!(
        (
            changes[0].field.as_str(),
            changes[0].current.as_str(),
            changes[0].proposed.as_str()
        ),
        ("description", "Old", "New")
    );
    let record = &pending.snapshot["record"];
    assert_eq!(record["pk"], json!({"enumId": "LAZY_E1"}));
    assert!(record["lastUpdatedStamp"].is_string(), "{record}");
    assert_eq!(pending.summary(), "Change an enumeration: update LAZY_E1");
    assert_eq!(
        description(&bed, "LAZY_E1").as_deref(),
        Some("Old"),
        "nothing runs before the decision"
    );

    decide(&bed, &approval_id, &Decision::Approve { arguments: None });
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("New"));
    let done = approval::load(&bed.facade, &approval_id).unwrap().unwrap();
    assert_eq!(done.status, ApprovalStatus::Executed);
    let text = results(&bed).pop().unwrap();
    assert!(text.starts_with("The user approved enum_update, and it ran."), "{text}");
}

#[test]
fn edited_arguments_are_checked_again_and_a_conflict_runs_nothing() {
    let bed = bed("edited");
    let _entered = bed.ctx.enter();
    // edited: what runs is the user's
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "Proposed"}}),
    );
    decide(
        &bed,
        &approval_id,
        &Decision::Approve {
            arguments: Some(json!({"enumId": "LAZY_E1", "enumTypeId": "LazyType", "description": "Edited"})),
        },
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("Edited"));
    let text = results(&bed).pop().unwrap();
    assert!(
        text.contains("edited the parameters: description Edited (you proposed Proposed)"),
        "{text}"
    );

    // edited into parameters that are not valid: checked again, nothing runs, the approval fails
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "Proposed"}}),
    );
    decide(
        &bed,
        &approval_id,
        &Decision::Approve {
            arguments: Some(json!({"enumId": "LAZY_E1", "description": "Bad", "sequenceNum": "many"})),
        },
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("Edited"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Failed
    );
    let text = results(&bed).pop().unwrap();
    assert!(
        text.contains("approved parameters of enum_update are not valid"),
        "{text}"
    );

    // the record changed while the approval waited: a conflict, nothing runs
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "Late"}}),
    );
    std::thread::sleep(Duration::from_millis(5));
    let mut current = bed
        .facade
        .find("moqui.basic.Enumeration")
        .unwrap()
        .eq("enumId", Value::string("LAZY_E1"))
        .one()
        .unwrap()
        .unwrap();
    current.set("description", Value::string("Someone else"));
    current.update(&bed.facade).unwrap();
    decide(&bed, &approval_id, &Decision::Approve { arguments: None });
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("Someone else"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Conflict
    );
    let text = results(&bed).pop().unwrap();
    assert!(
        text.contains("record changed after enum_update was proposed") && text.contains("Someone else"),
        "{text}"
    );
}

#[test]
fn a_destructive_call_asks_and_an_approval_of_an_older_exposure_runs_nothing() {
    let bed = bed("destructive");
    let _entered = bed.ctx.enter();
    row(
        &bed.facade,
        "moqui.agent.AgentToolPolicy",
        &[
            ("agentId", "assist"),
            ("sequenceNum", "1"),
            ("toolPattern", "enum_delete"),
            ("allowDestructive", "Y"),
        ],
    );
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_delete", "parameters": {"enumId": "LAZY_E1"}}),
    );
    let pending = approval::load(&bed.facade, &approval_id).unwrap().unwrap();
    assert_eq!(pending.classification, Classification::Destructive);
    assert!(pending
        .changes()
        .iter()
        .any(|change| change.field == "description" && change.current == "Old"));
    // no sim overlay on the in-memory database: no preview before the card (embedded Postgres checks one)
    assert!(pending.preview.is_none());

    // an admin edits the exposure while the approval waits: the approval is of the older version
    let _authz = bed.ctx.disable_authz();
    let mut exposure = bed
        .facade
        .find(catalog::EXPOSURE)
        .unwrap()
        .eq("toolName", Value::string("enum_delete"))
        .one()
        .unwrap()
        .unwrap();
    exposure.set("lockVersion", Value::I64(2));
    exposure.update(&bed.facade).unwrap();
    decide(&bed, &approval_id, &Decision::Approve { arguments: None });
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(
        description(&bed, "LAZY_E1").as_deref(),
        Some("Old"),
        "nothing was deleted"
    );
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Conflict
    );
    assert!(results(&bed).pop().unwrap().contains("changed by an admin"));
}

#[test]
fn the_admin_service_refuses_a_stale_exposure_edit() {
    let bed = bed("lock");
    let _entered = bed.ctx.enter();
    {
        let _authz = bed.ctx.disable_authz();
        row(
            &bed.facade,
            "moqui.security.UserGroup",
            &[("userGroupId", "LAZY_ADMIN"), ("description", "Lazy admins")],
        );
        let mut member = bed.facade.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userGroupId", Value::string("LAZY_ADMIN"));
        member.set("userId", Value::string(USER));
        member.set("fromDate", Value::I64(0));
        member.create(&bed.facade).unwrap();
        let mut permission = bed.facade.make_value("moqui.security.UserGroupPermission").unwrap();
        permission.set("userGroupId", Value::string("LAZY_ADMIN"));
        permission.set("userPermissionId", Value::string("AgentAdmin"));
        permission.set("fromDate", Value::I64(0));
        permission.create(&bed.facade).unwrap();
        crate::user::internal_login_user(USER).unwrap();
    }
    let store = |fields: &[(&str, Value)]| {
        let parameters: BTreeMap<String, Value> = fields
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        let mark = ExecutionContext::with_current(|ec| ec.message().error_mark()).unwrap();
        let out = crate::service::call::sync_with(
            "org.moqui.agent.AgentAdminServices.store#AgentToolExposure",
            &parameters,
            &crate::service::call::CallOptions::default(),
        )
        .unwrap();
        let errors = ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark)).unwrap();
        (out, errors)
    };
    // two editors read version 1; the first save moves it to 2
    let (out, errors) = store(&[
        ("toolName", Value::string("enum_update")),
        ("description", Value::string("Change an enumeration's description")),
        ("lockVersion", Value::I64(1)),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(out.get("lockVersion"), Some(&Value::I64(2)));
    assert_eq!(out.get("serviceClassification"), Some(&Value::string("AgclWrite")));
    // the second, from version 1, is refused and changes nothing
    let (_, errors) = store(&[
        ("toolName", Value::string("enum_update")),
        ("title", Value::string("Stale")),
        ("lockVersion", Value::I64(1)),
    ]);
    assert!(
        errors.iter().any(|error| error.contains("changed by someone else")),
        "{errors:?}"
    );
    let stored = bed
        .facade
        .find(catalog::EXPOSURE)
        .unwrap()
        .eq("toolName", Value::string("enum_update"))
        .disable_authz()
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::llm::text_of(&stored, "title").as_deref(),
        Some("Change an enumeration")
    );
    assert_eq!(crate::llm::int_of(&stored, "lockVersion"), Some(2));
    // a new one; a reserved name and a service that is not defined are refused
    let (out, errors) = store(&[
        ("toolName", Value::string("enum_find_all")),
        (
            "serviceName",
            Value::string("org.moqui.impl.BasicServices.find#Enumeration"),
        ),
    ]);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(out.get("serviceClassification"), Some(&Value::string("AgclRead")));
    let (_, errors) = store(&[
        ("toolName", Value::string("a2a__other")),
        ("serviceName", Value::string("x")),
    ]);
    assert!(
        errors.iter().any(|error| error.contains("kept for MCP and A2A")),
        "{errors:?}"
    );
    let (_, errors) = store(&[
        ("toolName", Value::string("nothing")),
        ("serviceName", Value::string("no.such.S.do#It")),
    ]);
    assert!(errors.iter().any(|error| error.contains("not a service")), "{errors:?}");
}

#[test]
fn a_user_facts_provider_adds_its_facts_to_the_prefix() {
    let bed = bed("facts");
    let _entered = bed.ctx.enter();
    let facts =
        crate::llm::run::quietly(|| crate::llm::facts::collect(&bed.factory, &bed.facade, FACTS_USER, "assist"));
    assert_eq!(
        facts.get("name"),
        Some("Facts Tester"),
        "the framework's fact keeps its value"
    );
    assert_eq!(facts.get("active organization"), Some("Test Org (TEST_ORG)"));
    assert_eq!(facts.org_instructions, "Quote prices in euros.");
    // another user gets none of them; the conf's service that is not defined is left out, logged
    let other = crate::llm::run::quietly(|| crate::llm::facts::collect(&bed.factory, &bed.facade, USER, "assist"));
    assert!(other.get("active organization").is_none() && other.org_instructions.is_empty());

    let run_id = start(&bed, FACTS_USER, vec![fake::Reply::text("Hello.")]);
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    let run = run::load_run(&bed.facade, &run_id).unwrap().unwrap();
    let conversation = bed
        .facade
        .find("moqui.agent.AgentConversation")
        .unwrap()
        .eq("conversationId", Value::string(&run.conversation_id))
        .disable_authz()
        .one()
        .unwrap()
        .unwrap();
    let system = crate::llm::text_of(&conversation, "systemText").unwrap();
    assert!(
        system.contains("- active organization: Test Org (TEST_ORG)"),
        "{system}"
    );
    assert!(
        system.contains("## Instructions of the organization\nQuote prices in euros."),
        "{system}"
    );
}

fn policy(bed: &Bed, sequence: &str, fields: &[(&str, &str)]) {
    let mut all = vec![("agentId", "assist"), ("sequenceNum", sequence)];
    all.extend_from_slice(fields);
    row(&bed.facade, "moqui.agent.AgentToolPolicy", &all);
}

fn approvals(bed: &Bed) -> usize {
    bed.facade
        .find("moqui.agent.AgentApproval")
        .unwrap()
        .disable_authz()
        .list()
        .unwrap()
        .values()
        .len()
}

#[test]
fn a_catalog_entrys_calls_per_run_limit_counts_its_run_tool_calls() {
    let bed = bed("limit");
    let _entered = bed.ctx.enter();
    policy(&bed, "1", &[("toolPattern", "enum_find"), ("callsPerRun", "1")]);
    let find = || json!({"name": "enum_find", "parameters": {"enumTypeId": "LazyType"}});
    let run_id = start(
        &bed,
        USER,
        vec![
            tool_step("r1", "run_tool", find()),
            tool_step("r2", "run_tool", find()),
            tool_step(
                "r3",
                "run_tool",
                json!({"name": "test_lazy", "parameters": {"word": "ok"}}),
            ),
            fake::Reply::text("Done."),
        ],
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    let results = results(&bed);
    assert!(results[0].contains("\"ok\":true"), "{}", results[0]);
    assert!(
        results[1].contains("\"limit\"") && results[1].contains("enum_find may be run 1 times in a run"),
        "{}",
        results[1]
    );
    assert_eq!(
        results[2], "ok has 2 letters",
        "another entry is not counted against it"
    );
    let calls = bed
        .facade
        .find("moqui.agent.AgentToolCall")
        .unwrap()
        .eq("runId", Value::string(&run_id))
        .order_by("toolCallId", false)
        .disable_authz()
        .list()
        .unwrap();
    let entries: Vec<Option<String>> = calls
        .values()
        .iter()
        .map(|row| crate::llm::text_of(row, "catalogToolName"))
        .collect();
    assert_eq!(
        entries,
        [Some("enum_find".to_string()), None, Some("test_lazy".to_string())],
        "only the calls that ran are counted"
    );
}

#[test]
fn a_key_pattern_or_a_missing_record_is_refused_and_an_edit_may_not_change_the_record() {
    let bed = bed("keys");
    let _entered = bed.ctx.enter();
    policy(&bed, "1", &[("toolPattern", "enum_delete"), ("allowDestructive", "Y")]);
    let run_id = start(
        &bed,
        USER,
        vec![
            // a wildcard key would delete every match behind a card for one record
            tool_step(
                "r1",
                "run_tool",
                json!({"name": "enum_delete", "parameters": {"enumId": "*"}}),
            ),
            tool_step(
                "r2",
                "run_tool",
                json!({"name": "enum_delete", "parameters": {"enumId": "LAZY_%"}}),
            ),
            tool_step(
                "r3",
                "run_tool",
                json!({"name": "enum_update", "parameters": {"enumId": "NO_SUCH", "description": "x"}}),
            ),
            fake::Reply::text("Done."),
        ],
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    let refused = results(&bed);
    assert!(
        refused[0].contains("must be its id, not a pattern (*)"),
        "{}",
        refused[0]
    );
    assert!(refused[1].contains("not a pattern (LAZY_%)"), "{}", refused[1]);
    assert!(
        refused[2].contains("\"not_found\"") && refused[2].contains("enumId NO_SUCH"),
        "{}",
        refused[2]
    );
    assert_eq!(approvals(&bed), 0, "no card was asked for");
    assert!(description(&bed, "LAZY_E1").is_some() && description(&bed, "LAZY_E2").is_some());

    // an edit that moves the write to another record runs nothing
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "Proposed"}}),
    );
    decide(
        &bed,
        &approval_id,
        &Decision::Approve {
            arguments: Some(json!({"enumId": "LAZY_E2", "description": "Proposed"})),
        },
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("Old"));
    assert_eq!(description(&bed, "LAZY_E2").as_deref(), Some("Other"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Failed
    );
    let text = results(&bed).pop().unwrap();
    assert!(text.contains("may not change which record"), "{text}");
    // an edit to a pattern is refused the same way
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_delete", "parameters": {"enumId": "LAZY_E2"}}),
    );
    decide(
        &bed,
        &approval_id,
        &Decision::Approve {
            arguments: Some(json!({"enumId": "*"})),
        },
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert!(
        description(&bed, "LAZY_E1").is_some() && description(&bed, "LAZY_E2").is_some(),
        "nothing was deleted"
    );
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Failed
    );
}

#[test]
fn an_approved_write_is_checked_again_for_the_policy() {
    let bed = bed("recheck");
    let _entered = bed.ctx.enter();
    // the policy says never while the approval waits: nothing runs
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "Blocked"}}),
    );
    policy(
        &bed,
        "1",
        &[("toolPattern", "enum_update"), ("approvalModeEnumId", "AgamNever")],
    );
    decide(&bed, &approval_id, &Decision::Approve { arguments: None });
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("Old"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Failed
    );
    let text = results(&bed).pop().unwrap();
    assert!(
        text.contains("\"not_allowed\"") && text.contains("nothing ran"),
        "{text}"
    );
}

#[test]
fn edited_arguments_run_only_as_they_were_hashed() {
    let bed = bed("exact");
    let _entered = bed.ctx.enter();
    // a number written as text would be converted by a repair after the hash: it is refused instead
    let (run_id, approval_id) = paused(
        &bed,
        json!({"name": "enum_update", "parameters": {"enumId": "LAZY_E1", "description": "Proposed"}}),
    );
    decide(
        &bed,
        &approval_id,
        &Decision::Approve {
            arguments: Some(json!({"enumId": "LAZY_E1", "description": "Edited", "sequenceNum": "5"})),
        },
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    assert_eq!(description(&bed, "LAZY_E1").as_deref(), Some("Old"));
    assert_eq!(
        approval::load(&bed.facade, &approval_id).unwrap().unwrap().status,
        ApprovalStatus::Failed
    );
    assert!(results(&bed).pop().unwrap().contains("are not valid"));
}

#[test]
fn find_ranks_only_what_the_user_may_run() {
    let bed = bed("find");
    let _entered = bed.ctx.enter();
    let mut catalog = catalog_of(&bed, &ToolPolicy::default());
    // entries the user may not run (their service does not resolve) that would outrank the others
    for n in 0..catalog::FIND_MAX {
        catalog.entries.push(catalog::Entry {
            name: format!("enumeration_ghost_{n}"),
            title: "Enumeration enumeration".to_string(),
            description: "Enumeration".to_string(),
            classification: Classification::Read,
            target: catalog::Target::Service(catalog::Exposure {
                tool_name: format!("enumeration_ghost_{n}"),
                service_name: "no.such.Services.find#Enumeration".to_string(),
                title: None,
                description: None,
                schema_override: None,
                classification: None,
                expose_assistant: true,
                expose_mcp: false,
                preview: approval::PreviewMode::OnDemand,
                lock_version: 1,
                disabled: false,
            }),
            allowed: true,
            refusal: None,
        });
    }
    let (found, total) = catalog.find("enumeration");
    let names: Vec<&str> = found.iter().map(|entry| entry.name.as_str()).collect();
    assert!(!names.iter().any(|name| name.contains("ghost")), "{names:?}");
    assert!(
        names.contains(&"enum_find") && names.contains(&"enum_update"),
        "{names:?}"
    );
    assert_eq!(total, names.len(), "the total counts only what may run");
    let text = find_text(&catalog, "enumeration");
    assert!(!text.contains("ghost") && !text.contains("showing"), "{text}");
}

/// Phase 13.14 (plan 7.5): a service's lessons (the approved ones and the user's own) come with `describe_tools` and
/// with that tool's errors, two each, wrapped as untrusted.
#[test]
fn a_services_lessons_come_with_describe_tools_and_its_errors() {
    let bed = bed("lessons");
    let _entered = bed.ctx.enter();
    let anchor = crate::llm::memory::Anchor::Service("org.moqui.impl.BasicServices.find#Enumeration".to_string());
    crate::llm::memory::propose_lesson(&bed.facade, USER, &anchor, "Pass enumTypeId, never an empty find", None, None)
        .unwrap();
    let run_id = start(
        &bed,
        USER,
        vec![
            tool_step("d1", "describe_tools", json!({"names": ["enum_find", "enum_update"]})),
            tool_step("e1", "run_tool", json!({"name": "enum_find", "parameters": {}})),
            tool_step("u1", "run_tool", json!({"name": "enum_find", "parameters": {"enumTypeId": "LazyType"}})),
            fake::Reply::text("Done."),
        ],
    );
    assert_eq!(run::wait_for(&bed.factory, &run_id, WAIT).unwrap(), RunState::Completed);
    let results = results(&bed);
    let lesson = "lesson: <untrusted from=\"lesson\">Pass enumTypeId, never an empty find</untrusted>";
    assert!(results[0].contains(&format!("lessons for enum_find:\n{lesson}")), "{}", results[0]);
    assert!(!results[0].contains("lessons for enum_update"), "{}", results[0]);
    assert!(results[1].contains(lesson), "an error of the tool: {}", results[1]);
    assert!(!results[2].contains(lesson), "a call that worked has none: {}", results[2]);
}
