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

//! The core tools in runs against the scripted fake provider (plan 9, phase 13.3): `search` over the shell data and a
//! palette source, `data` over a screen list (paged, filtered, linked) and a basic entity, a result past the model's cap
//! spilled and read back as an artifact in the next turn, `skill` over the framework's and a component's skills, `ask`
//! pausing and its answer resuming the run, did-you-mean for an unknown tool, argument repair, and an answer's invented
//! reference. The apps are a small tree in a temp directory: a root with the webroot's `shellData` and `shellSearch`
//! transitions, one app with a list of 30 enumerations, a palette action and a palette source.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moqui_lib::object::Value;
use serde_json::{json, Value as Json};

use crate::context::{ComponentInfo, ExecutionContext, ExecutionContextFactory};
use crate::entity::EntityFacade;
use crate::llm::provider::fake::{self, Step};
use crate::llm::run::{self, events, RunState, StartRequest};
use crate::llm::text_of;

const USER: &str = "tools.tester";
const WAIT: Duration = Duration::from_secs(20);

const ROOT: &str = r#"<screen require-authentication="false">
    <transition name="shellData" read-only="true" begin-transaction="false"><actions><script><![CDATA[
        const shellData = sri.getShellData()
        if (shellData != null) ec.web.sendJsonResponse(shellData)
    ]]></script></actions><default-response type="none"/></transition>
    <transition name="shellSearch" read-only="true" begin-transaction="false"><actions><script><![CDATA[
        const params = ec.web.parameters
        const found = sri.getShellSearch(params.text, params.groups, params.limit)
        if (found != null) ec.web.sendJsonResponse(found)
    ]]></script></actions><default-response type="none"/></transition>
    <subscreens default-item="apps"/>
    <widgets><subscreens-active/></widgets>
</screen>"#;

const SERVICES: &str = r#"<services>
    <service verb="search" noun="Records" authenticate="false">
        <in-parameters><parameter name="text"/><parameter name="limit" type="Integer"/><parameter name="docTypes" type="List"/>
            <parameter name="idMatch" type="Boolean"/></in-parameters>
        <out-parameters><parameter name="rows" type="List"/><parameter name="moreUrl"/></out-parameters>
        <actions><script><![CDATA[
            rows = [{title: 'Record of ' + text, subtitle: 'a record', url: '/qapps/tooltest/Thing?enumId=E01'}]
            moreUrl = '/qapps/tooltest/Things?description=' + text
        ]]></script></actions>
    </service>
</services>"#;

struct Bed {
    _ctx: ExecutionContext,
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
    endpoint: String,
    dir: PathBuf,
}

impl Drop for Bed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn tree(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-agent-tools-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for path in ["Root", "Apps/ToolTest", "service/test", "component/skill"] {
        std::fs::create_dir_all(dir.join(path)).unwrap();
    }
    let base = dir.to_string_lossy().into_owned();
    let write = |path: &str, text: &str| std::fs::write(dir.join(path), text.replace("{dir}", &base)).unwrap();
    write("Root.xml", ROOT);
    write(
        "Root/apps.xml",
        r#"<screen require-authentication="false" default-menu-title="Applications">
            <subscreens default-item="tooltest">
                <subscreens-item name="tooltest" menu-title="Tool Test" menu-index="1" location="{dir}/Apps/ToolTest.xml"/>
            </subscreens>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Apps/ToolTest.xml",
        r#"<screen require-authentication="false" default-menu-title="Tool Test">
            <subscreens default-item="Things">
                <subscreens-item name="Things" menu-title="Things" menu-index="1" location="{dir}/Apps/ToolTest/Things.xml"/>
                <subscreens-item name="Board" menu-title="Widget Board" menu-index="2" location="{dir}/Apps/ToolTest/Board.xml"/>
                <subscreens-item name="Thing" menu-include="false" location="{dir}/Apps/ToolTest/Thing.xml"/>
            </subscreens>
            <shell-providers>
                <palette-action label="Find things" screen="tooltest/Things"/>
                <palette-source group="Records" label="Records" service="test.AgentTools.search#Records"/>
            </shell-providers>
            <widgets><subscreens-active/></widgets>
        </screen>"#,
    );
    write(
        "Apps/ToolTest/Things.xml",
        r#"<screen require-authentication="false" default-menu-title="Things">
            <actions>
                <entity-find entity-name="moqui.basic.Enumeration" list="enumList">
                    <search-form-inputs default-order-by="enumId"/>
                    <econdition field-name="enumTypeId" value="TestType"/>
                </entity-find>
            </actions>
            <widgets>
                <form-list name="ThingList" list="enumList" id-field="enumId">
                    <field name="enumId"><default-field title="Thing">
                        <link url="/apps/tooltest/Thing" url-type="plain" text="${enumId}"><parameter name="enumId"/></link>
                    </default-field></field>
                    <field name="description"><default-field><display/></default-field></field>
                    <field name="enumTypeId"><default-field title="Type"><display/></default-field></field>
                </form-list>
                <form-list name="Notes" list="[{note: 'one'}, {note: 'two'}]">
                    <field name="note"><default-field><display/></default-field></field>
                </form-list>
            </widgets>
        </screen>"#,
    );
    write(
        "Apps/ToolTest/Board.xml",
        r#"<screen require-authentication="false" default-menu-title="Widget Board"><widgets><label text="board"/></widgets></screen>"#,
    );
    write(
        "Apps/ToolTest/Thing.xml",
        r#"<screen require-authentication="false" default-menu-title="Thing"><parameter name="enumId"/><widgets><label text="${enumId}"/></widgets></screen>"#,
    );
    write("service/test/AgentTools.xml", SERVICES);
    write(
        "component/skill/widget-board.md",
        "---\nname: widget-board\ntitle: Arranging the widget board\ndescription: Use when the user moves widgets on the board.\nkind: procedure\n---\nOpen the board, then drag.\n",
    );
    write("component/skill/aliases.json", r#"{"thing": ["gadget"]}"#);
    dir
}

/// A run bed: the tree's root as the webroot, a fake model with a 1,500 character result cap, the services, the user,
/// the agent and 30 enumerations of a test type.
fn bed(name: &str) -> Bed {
    bed_with_cap(name, 1500)
}

/// [`bed`] with a result cap of `cap` characters.
fn bed_with_cap(name: &str, cap: usize) -> Bed {
    let dir = tree(name);
    let endpoint = format!("tools-{name}");
    let conf = format!(
        r#"<moqui-conf>
            <webapp-list><webapp name="webroot"><root-screen host=".*" location="{root}"/></webapp></webapp-list>
            <llm-facade>
                <endpoint id="{endpoint}" protocol="fake"/>
                <model id="{endpoint}" endpoint="{endpoint}" name="fake-1" context-window="32768" result-cap-chars="{cap}"/>
                <bind feature="assist" model="{endpoint}" reasoning="none"/>
            </llm-facade></moqui-conf>"#,
        root = dir.join("Root.xml").display()
    );
    let component = ComponentInfo {
        name: "tooltest".to_string(),
        version: String::new(),
        location: dir.join("component"),
        depends_on: Vec::new(),
    };
    let ctx = ExecutionContextFactory::test_context_with_components(
        &conf,
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
        vec![component],
    );
    let facade = crate::testing::open();
    facade.assume_db_tables();
    ctx.factory().set_entity(facade.clone());
    fake::clear(&endpoint);
    let entered = ctx.enter();
    let failures = crate::service::lint(&[dir.join("service/test/AgentTools.xml")]).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    drop(entered);
    row(
        &facade,
        "moqui.security.UserAccount",
        &[("userId", USER), ("username", USER), ("userFullName", "Tools Tester")],
    );
    row(
        &facade,
        "moqui.agent.AgentDefinition",
        &[
            ("agentId", "assist"),
            ("agentName", "Assist"),
            ("featureId", "assist"),
            ("surfaces", "drawer,chat,a2a,job,tui"),
        ],
    );
    row(
        &facade,
        "moqui.basic.EnumerationType",
        &[("enumTypeId", "TestType"), ("description", "Test things")],
    );
    for n in 1..=30 {
        row(
            &facade,
            "moqui.basic.Enumeration",
            &[
                ("enumId", &format!("E{n:02}")),
                ("enumTypeId", "TestType"),
                (
                    "description",
                    &format!("Thing number {n} {}", "with a long description ".repeat(2)),
                ),
            ],
        );
    }
    Bed {
        factory: Arc::clone(ctx.factory()),
        _ctx: ctx,
        facade,
        endpoint,
        dir,
    }
}

fn row(facade: &EntityFacade, entity: &str, fields: &[(&str, &str)]) {
    let mut value = facade.make_value(entity).unwrap();
    for (field, text) in fields {
        value.set(*field, Value::string(*text));
    }
    value.create(facade).unwrap();
}

fn request(text: &str) -> StartRequest {
    StartRequest {
        user_id: USER.to_string(),
        text: text.to_string(),
        ..StartRequest::default()
    }
}

fn tool(id: &str, name: &str, arguments: Json) -> Step {
    raw_tool(id, name, &arguments.to_string())
}

fn raw_tool(id: &str, name: &str, arguments: &str) -> Step {
    Step::Tool {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
        pieces: 1,
    }
}

fn calls(steps: Vec<Step>) -> fake::Reply {
    let mut steps = steps;
    steps.push(Step::Finish("tool_calls".to_string()));
    fake::Reply::steps(steps)
}

/// Run `text` to its end with the replies scripted; the tool results the model read, in order.
fn run_turn(bed: &Bed, start: StartRequest, replies: Vec<fake::Reply>, end: RunState) -> (run::Started, Vec<String>) {
    fake::script(&bed.endpoint, replies);
    let before = fake::requests(&bed.endpoint).len();
    let started = run::start_run(&bed.factory, &start).unwrap();
    assert_eq!(run::wait_for(&bed.factory, &started.run_id, WAIT).unwrap(), end);
    let sent = fake::requests(&bed.endpoint);
    let last = sent
        .last()
        .filter(|_| sent.len() > before)
        .cloned()
        .unwrap_or(Json::Null);
    (started, tool_texts(&last))
}

/// The texts of the tool results of a request after its last user message: those of this turn.
fn tool_texts(request: &Json) -> Vec<String> {
    let messages: Vec<&Json> = request["messages"].as_array().into_iter().flatten().collect();
    let from = messages
        .iter()
        .rposition(|message| message["role"] == "user")
        .map_or(0, |at| at + 1);
    messages[from..]
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn event_types(bed: &Bed, run_id: &str) -> Vec<(String, Json)> {
    events::stored_after(&bed.facade, run_id, 0, 10_000)
        .unwrap()
        .into_iter()
        .map(|event| (event.event_type, event.payload))
        .collect()
}

#[test]
fn search_finds_screens_actions_and_palette_rows_with_alias_words() {
    let bed = bed("search");
    let _entered = bed._ctx.enter();
    let (_, results) = run_turn(
        &bed,
        request("Find things"),
        vec![
            calls(vec![
                tool("s1", "search", json!({"text": "things"})),
                tool("s2", "search", json!({"text": "gadget", "groups": ["Screens"]})),
                tool("s3", "search", json!({"text": "widget board"})),
            ]),
            fake::Reply::text("Found them."),
        ],
        RunState::Completed,
    );
    assert_eq!(results.len(), 3, "{results:?}");
    let things = &results[0];
    assert!(
        things.contains("Screens: Things (Tool Test) → /apps/tooltest/Things"),
        "{things}"
    );
    assert!(
        things.contains("Actions: Find things (Tool Test) → /apps/tooltest/Things"),
        "{things}"
    );
    assert!(
        things.contains("Records: Record of things (a record) → /apps/tooltest/Thing?enumId=E01"),
        "{things}"
    );
    assert!(
        things.contains("more: Records → /apps/tooltest/Things?description=things"),
        "{things}"
    );
    // gadget is a component's alias of thing; only the Screens group is searched
    let gadget = &results[1];
    assert!(gadget.starts_with("Screens: Things"), "{gadget}");
    assert!(!gadget.contains("Records:") && !gadget.contains("Actions:"), "{gadget}");
    assert!(
        results[2].starts_with("Screens: Widget Board (Tool Test) → /apps/tooltest/Board"),
        "{}",
        results[2]
    );
}

#[test]
fn data_reads_a_screen_list_by_pages_with_a_filter_and_the_row_link() {
    let bed = bed("list");
    let _entered = bed._ctx.enter();
    let (_, results) = run_turn(
        &bed,
        request("List things"),
        vec![
            calls(vec![
                tool("d1", "data", json!({"path": "/apps/tooltest/Things", "limit": 10})),
                tool(
                    "d2",
                    "data",
                    json!({"path": "/qapps/tooltest/Things", "limit": 10, "offset": 20}),
                ),
                tool(
                    "d3",
                    "data",
                    json!({"path": "/apps/tooltest/Things", "where": {"description": "%number 7 %"}}),
                ),
                tool("d4", "data", json!({"path": "/apps/tooltest/Things", "list": "Notez"})),
                tool("d5", "data", json!({"path": "/apps/tooltest/Thingz"})),
                tool(
                    "d6",
                    "data",
                    json!({"path": "/apps/tooltest/Things", "list": "Notes", "fields": ["note", "bogus"]}),
                ),
                tool(
                    "d7",
                    "data",
                    json!({"path": "/apps/tooltest/Things", "list": "Notes", "limit": 2, "offset": 2}),
                ),
                tool(
                    "d8",
                    "data",
                    json!({"path": "/apps/tooltest/Things", "limit": 10, "offset": 40}),
                ),
                tool("d9", "data", json!({"path": "/shellData"})),
            ]),
            fake::Reply::text("Here they are."),
        ],
        RunState::Completed,
    );
    assert_eq!(results.len(), 9, "{results:?}");
    let first: Vec<&str> = results[0].lines().collect();
    assert_eq!(
        first[0], "ThingList on /apps/tooltest/Things (showing 10 of 30)",
        "{}",
        results[0]
    );
    assert_eq!(first[1], "Thing | Description | Type");
    assert!(
        first[2].starts_with("E01 | Thing number 1 with a long"),
        "{}",
        results[0]
    );
    assert!(
        results[0].contains("rows link to /apps/tooltest/Thing?enumId=<enumId>"),
        "{}",
        results[0]
    );
    assert!(results[0].contains("other lists: Notes"), "{}", results[0]);
    assert!(results[0].ends_with("more: data offset=10"), "{}", results[0]);
    let third: Vec<&str> = results[1].lines().collect();
    assert_eq!(third[0], "ThingList on /apps/tooltest/Things (showing 10 of 30)");
    assert!(third[2].starts_with("E21 |"), "{}", results[1]);
    assert!(!results[1].contains("more:"), "{}", results[1]);
    let filtered = &results[2];
    assert!(
        filtered.starts_with("ThingList on /apps/tooltest/Things (showing 1 of 1)"),
        "{filtered}"
    );
    assert!(filtered.contains("E07 | Thing number 7 "), "{filtered}");
    assert!(results[3].starts_with("{\"error\":\"no_list\""), "{}", results[3]);
    assert!(results[3].contains("\"did_you_mean\":[\"Notes\"]"), "{}", results[3]);
    assert!(results[4].starts_with("{\"error\":\"no_screen\""), "{}", results[4]);
    assert!(results[4].contains("/apps/tooltest/Things"), "{}", results[4]);
    let notes = &results[5];
    assert!(
        notes.starts_with("Notes on /apps/tooltest/Things (showing 2 of 2)\nNote\none\ntwo"),
        "{notes}"
    );
    assert!(notes.contains("ignored fields: bogus"), "{notes}");
    // past the end of a list that holds all its rows, and of one that pages: none, not the first rows again
    assert!(
        results[6].starts_with("Notes on /apps/tooltest/Things (no rows after 2)"),
        "{}",
        results[6]
    );
    assert!(
        results[7].starts_with("ThingList on /apps/tooltest/Things (no rows after 40)"),
        "{}",
        results[7]
    );
    // a transition is not a list to read
    assert!(results[8].starts_with("{\"error\":\"no_screen\""), "{}", results[8]);
    assert!(results[8].contains("is an action"), "{}", results[8]);
}

#[test]
fn data_reads_basic_entities_directly_and_refuses_the_rest() {
    let bed = bed("entity");
    let _entered = bed._ctx.enter();
    let (_, results) = run_turn(
        &bed,
        request("Read types"),
        vec![
            calls(vec![
                tool(
                    "e1",
                    "data",
                    json!({"entity": "moqui.basic.Enumeration", "where": {"enumTypeId": "TestType", "enumId": ">=E28"},
                        "fields": ["enumId", "enumTypeId"], "order": "-enumId"}),
                ),
                tool(
                    "e2",
                    "data",
                    json!({"entity": "Enumeration", "where": {"enumId": ["E01", "E02"]}, "limit": 1}),
                ),
                tool("e3", "data", json!({"entity": "moqui.security.UserAccount"})),
                tool(
                    "e4",
                    "data",
                    json!({"entity": "moqui.basic.Enumeration", "where": {"enumTypo": "x"}}),
                ),
            ]),
            fake::Reply::text("Done."),
        ],
        RunState::Completed,
    );
    assert_eq!(
        results[0],
        "moqui.basic.Enumeration (showing 3 of 3, order -enumId)\nenumId | enumTypeId\nE30 | TestType\nE29 | TestType\nE28 | TestType"
    );
    assert!(
        results[1].starts_with("moqui.basic.Enumeration (showing 1 of 2)"),
        "{}",
        results[1]
    );
    assert!(results[1].ends_with("more: data offset=1"), "{}", results[1]);
    assert!(
        results[2].starts_with("{\"error\":\"entity_not_readable\""),
        "{}",
        results[2]
    );
    assert!(results[3].starts_with("{\"error\":\"unknown_field\""), "{}", results[3]);
    assert!(
        results[3].contains("\"did_you_mean\":[\"enumTypeId\""),
        "{}",
        results[3]
    );
}

#[test]
fn a_result_over_the_cap_is_spilled_and_read_back_in_the_next_turn() {
    let bed = bed("spill");
    let _entered = bed._ctx.enter();
    let (first, results) = run_turn(
        &bed,
        request("All things"),
        vec![
            calls(vec![tool(
                "p1",
                "data",
                json!({"path": "/apps/tooltest/Things", "limit": 30}),
            )]),
            fake::Reply::text("There are 30."),
        ],
        RunState::Completed,
    );
    let preview = &results[0];
    assert!(
        preview.chars().count() <= 1500,
        "{} chars: {preview}",
        preview.chars().count()
    );
    assert!(
        preview.starts_with("ThingList on /apps/tooltest/Things (showing 30 of 30)\nThing | Description | Type"),
        "{preview}"
    );
    let footer = preview.lines().last().unwrap();
    let artifact = footer
        .strip_prefix("artifact ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("{footer}"))
        .to_string();
    assert!(
        footer.contains(&format!("data artifact={artifact} offset=")),
        "{footer}"
    );
    let call = bed
        .facade
        .find("moqui.agent.AgentToolCall")
        .unwrap()
        .eq("runId", Value::string(&first.run_id))
        .one()
        .unwrap()
        .unwrap();
    let location = text_of(&call, "spillLocation").unwrap();
    assert_eq!(
        location,
        format!(
            "dbresource://agent/{}/{}",
            first.run_id,
            text_of(&call, "toolCallId").unwrap()
        )
    );
    let whole = String::from_utf8(bed.facade.read_db_resource_bytes(&location).unwrap().unwrap()).unwrap();
    assert!(whole.contains("E30 | Thing number 30"), "the whole result is stored");

    // the next turn of the conversation reads it on, and greps it
    let (_, results) = run_turn(
        &bed,
        StartRequest {
            conversation_id: Some(first.conversation_id.clone()),
            ..request("And the last ones?")
        },
        vec![
            calls(vec![
                tool("p2", "data", json!({"artifact": artifact, "offset": 30, "limit": 3})),
                tool("p3", "data", json!({"artifact": artifact, "grep": "number 17 "})),
                tool("p4", "data", json!({"artifact": "A999999"})),
            ]),
            fake::Reply::text("The last are E28 to E30."),
        ],
        RunState::Completed,
    );
    let read: Vec<&str> = results[0].lines().collect();
    assert!(
        read[0].starts_with(&format!("artifact {artifact} lines (showing 3 of ")),
        "{}",
        results[0]
    );
    assert_eq!(read[1], "Thing | Description | Type", "the header first");
    assert!(read[2].starts_with("E29 |"), "{}", results[0]);
    assert!(results[1].contains(": E17 | Thing number 17"), "{}", results[1]);
    assert!(results[2].starts_with("{\"error\":\"no_artifact\""), "{}", results[2]);

    // another conversation does not see it
    let (_, results) = run_turn(
        &bed,
        request("Peek"),
        vec![
            calls(vec![tool("p5", "data", json!({"artifact": artifact}))]),
            fake::Reply::text("Nothing."),
        ],
        RunState::Completed,
    );
    assert!(results[0].starts_with("{\"error\":\"no_artifact\""), "{}", results[0]);
}

#[test]
fn skill_loads_framework_and_component_skills_by_name_section_and_words() {
    let bed = bed_with_cap("skill", 8000);
    let _entered = bed._ctx.enter();
    let (_, results) = run_turn(
        &bed,
        request("How do I read lists?"),
        vec![
            calls(vec![
                tool("k1", "skill", json!({"name": "data-reads#where"})),
                tool("k2", "skill", json!({"find": "widgets"})),
                tool("k3", "skill", json!({"name": "data-read"})),
                tool("k4", "skill", json!({"name": "data-reads"})),
            ]),
            fake::Reply::text("Use data."),
        ],
        RunState::Completed,
    );
    assert!(
        results[0].starts_with("skill data-reads#where\n## Where\n"),
        "{}",
        results[0]
    );
    assert!(results[0].contains("%acme%"), "{}", results[0]);
    assert!(
        results[1].starts_with("- widget-board: Use when the user moves widgets on the board. (procedure)"),
        "{}",
        results[1]
    );
    assert!(
        results[2].contains("\"did_you_mean\":[\"data-reads\""),
        "{}",
        results[2]
    );
    assert!(
        results[3].starts_with("skill data-reads: Reading rows with data\n"),
        "{}",
        results[3]
    );
    assert!(
        results[3].contains("## Artifacts and grep"),
        "a small skill comes whole"
    );
}

#[test]
fn ask_pauses_the_run_and_the_answer_resumes_it() {
    let bed = bed("ask");
    let _entered = bed._ctx.enter();
    fake::script(
        &bed.endpoint,
        vec![
            calls(vec![tool(
                "a1",
                "ask",
                json!({"question": "Which thing?", "options": ["E01", "E02"]}),
            )]),
            fake::Reply::text("E02 it is."),
        ],
    );
    let started = run::start_run(&bed.factory, &request("Pick one")).unwrap();
    assert_eq!(
        run::wait_for(&bed.factory, &started.run_id, WAIT).unwrap(),
        RunState::InputRequired
    );
    let events = event_types(&bed, &started.run_id);
    let (_, asked) = events.iter().find(|(kind, _)| kind == "question.asked").unwrap();
    assert_eq!(asked["text"], "Which thing?");
    assert_eq!(asked["options"][0], json!({"label": "E01", "recommended": true}));
    let question_id = asked["questionId"].as_str().unwrap().to_string();
    let question = |field: &str| {
        let row = bed
            .facade
            .find("moqui.agent.AgentQuestion")
            .unwrap()
            .eq("questionId", Value::string(&question_id))
            .one()
            .unwrap()
            .unwrap();
        text_of(&row, field)
    };
    assert_eq!(question("statusId").as_deref(), Some("AgqOpen"));
    run::submit_input(&bed.factory, &started.run_id, USER, "answer", "2", None).unwrap();
    assert_eq!(
        run::wait_for(&bed.factory, &started.run_id, WAIT).unwrap(),
        RunState::Completed
    );
    let sent = fake::requests(&bed.endpoint);
    assert_eq!(tool_texts(sent.last().unwrap()), ["The user chose \"E02\"."]);
    assert_eq!(question("statusId").as_deref(), Some("AgqAnswered"));
    assert!(question("answerJson").unwrap().contains("E02"));

    // a job cannot ask: the tool is not offered there
    let job_tools = crate::llm::tools::surface_specs(
        crate::llm::prompt::Surface::Job,
        crate::llm::conf::PromptVariant::Standard,
    );
    assert!(job_tools.iter().all(|tool| tool.name != "ask"));
    let a2a_tools = crate::llm::tools::surface_specs(
        crate::llm::prompt::Surface::A2a,
        crate::llm::conf::PromptVariant::Standard,
    );
    assert!(
        a2a_tools.iter().any(|tool| tool.name == "ask"),
        "A2A asks with a data part"
    );
}

#[test]
fn unknown_tools_get_did_you_mean_and_arguments_are_repaired_or_taught() {
    let bed = bed("repair");
    let _entered = bed._ctx.enter();
    let (_, results) = run_turn(
        &bed,
        request("Look"),
        vec![
            calls(vec![
                tool("r1", "serach", json!({"text": "things"})),
                tool("r2", "browse", json!({"text": "things"})),
                raw_tool("r3", "search", r#"{'query': 'things', 'limit': '1', 'bogus': True,}"#),
                tool("r4", "ask", json!({"question": "Sure?", "options": "Yes"})),
            ]),
            fake::Reply::text("Ok."),
        ],
        RunState::Completed,
    );
    assert!(results[0].starts_with("{\"error\":\"unknown_tool\""), "{}", results[0]);
    assert!(results[0].contains("\"did_you_mean\":[\"search\""), "{}", results[0]);
    assert!(results[1].contains("Call search with a few words"), "{}", results[1]);
    // single quotes, a Python literal and a trailing comma parse; query is text, the limit a number, bogus ignored
    let repaired = &results[2];
    assert!(repaired.starts_with("Screens: Things"), "{repaired}");
    assert!(
        repaired.ends_with("ignored: bogus (not arguments of search)"),
        "{repaired}"
    );
    assert_eq!(
        repaired.matches("Screens:").count(),
        1,
        "the limit came through: {repaired}"
    );
    // one option is converted to a list and then refused with what to do instead
    assert!(results[3].starts_with("{\"error\":\"options\""), "{}", results[3]);
    assert!(results[3].contains("2 to 4 short options"), "{}", results[3]);
}

#[test]
fn an_answer_link_no_tool_returned_is_an_invented_reference() {
    let bed = bed("refs");
    let _entered = bed._ctx.enter();
    let (started, _) = run_turn(
        &bed,
        request("Open E03"),
        vec![
            calls(vec![tool("f1", "data", json!({"path": "/apps/tooltest/Things", "limit": 5}))]),
            fake::Reply::text(
                "See [E03](/apps/tooltest/Thing?enumId=E03), [E77](/apps/tooltest/Thing?enumId=E77) and [the report](/apps/nowhere/Report).",
            ),
        ],
        RunState::Completed,
    );
    let run_row = bed
        .facade
        .find("moqui.agent.AgentRun")
        .unwrap()
        .eq("runId", Value::string(&started.run_id))
        .one()
        .unwrap()
        .unwrap();
    let result: Json = serde_json::from_str(&text_of(&run_row, "resultJson").unwrap()).unwrap();
    assert_eq!(
        result["invented"],
        json!(["/apps/tooltest/Thing?enumId=E77", "/apps/nowhere/Report"])
    );
    let events = event_types(&bed, &started.run_id);
    let (_, checked) = events.iter().find(|(kind, _)| kind == "answer.references").unwrap();
    assert_eq!(
        checked["text"],
        "See [E03](/apps/tooltest/Thing?enumId=E03), E77 and the report."
    );
}

/// A read whose run loses its lease while it runs (another node takes it), with a result over the cap.
#[derive(Default)]
struct LeaseThief;

static THIEF_SPEC: std::sync::OnceLock<super::ToolSpec> = std::sync::OnceLock::new();
static STOLEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl super::AgentTool for LeaseThief {
    fn spec(&self) -> &super::ToolSpec {
        THIEF_SPEC.get_or_init(|| {
            super::ToolSpec::new(
                "lease_thief",
                "Loses the lease.",
                Json::Null,
                super::Classification::Read,
            )
            .needs(&[super::tests::TEST_CAP])
        })
    }

    fn call(&self, ctx: &mut crate::llm::run::context::TurnContext<'_>, _args: Json) -> super::ToolOutcome {
        let facade = super::apps::factory().unwrap().entity().unwrap();
        let run_id = ctx.run.unwrap().run_id.clone();
        crate::llm::run::quietly(|| {
            facade
                .find("moqui.agent.AgentRun")
                .unwrap()
                .eq("runId", Value::string(&run_id))
                .update_all([("leaseOwner".to_string(), Value::string("another-node"))])
                .unwrap()
        });
        STOLEN.store(true, std::sync::atomic::Ordering::SeqCst);
        super::ToolOutcome::text("x".repeat(5000))
    }
}

crate::submit_tool!(LeaseThief);

#[test]
fn a_spilled_result_of_a_run_that_lost_its_lease_leaves_no_artifact() {
    let bed = bed("lease");
    let _entered = bed._ctx.enter();
    fake::script(
        &bed.endpoint,
        vec![
            calls(vec![tool("l1", "lease_thief", json!({}))]),
            fake::Reply::text("never"),
        ],
    );
    let started = run::start_run(
        &bed.factory,
        &StartRequest {
            extra_caps: vec![super::tests::TEST_CAP.to_string()],
            ..request("Lose it")
        },
    )
    .unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while !STOLEN.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(STOLEN.load(std::sync::atomic::Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(500));
    let call = bed
        .facade
        .find("moqui.agent.AgentToolCall")
        .unwrap()
        .eq("runId", Value::string(&started.run_id))
        .one()
        .unwrap()
        .unwrap();
    assert!(text_of(&call, "spillLocation").is_none(), "no location is stored");
    assert_ne!(text_of(&call, "statusId").as_deref(), Some("AgtcDone"));
    let location = super::spill::location(&started.run_id, &text_of(&call, "toolCallId").unwrap());
    assert!(
        bed.facade.read_db_resource_bytes(&location).unwrap().is_none(),
        "the artifact is removed"
    );
    assert_eq!(
        fake::requests(&bed.endpoint).len(),
        1,
        "the run went no further on this node"
    );
}
