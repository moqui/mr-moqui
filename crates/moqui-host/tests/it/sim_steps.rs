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

//! `org.moqui.impl.SimServices.run#Steps` (docs/ai/Assist.md, Sim steps): steps of one sim run, each under its own
//! savepoint, with the rows each changed and a read after each.

#![cfg(all(unix, feature = "embedded-postgres", feature = "llm"))]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::Value;
use moqui_llm::approval::steps::{self, SimCall, StepsRequest};
use serde_json::Value as Json;

use crate::common::start_with_files;

const DATA: &str = r#"<entity-facade-xml type="seed">
    <simsteps.Widget widgetId="W_PROD" widgetName="Production"/>
    <moqui.security.UserAccount userId="SIM_PLAIN" username="sim.plain" userFullName="Plain User"/>
    <moqui.security.ArtifactGroup artifactGroupId="SIM_STEPS" description="Sim steps test"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="SIM_STEPS" artifactName="simsteps\..*" nameIsPattern="Y"
            artifactTypeEnumId="AT_SERVICE" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="SIM_STEPS" artifactName="simsteps\..*" nameIsPattern="Y"
            artifactTypeEnumId="AT_ENTITY" inheritAuthz="Y"/>
    <moqui.security.ArtifactGroupMember artifactGroupId="SIM_STEPS" artifactName="org.moqui.impl.SimServices.run#Steps"
            artifactTypeEnumId="AT_SERVICE"/>
    <moqui.security.ArtifactAuthz artifactAuthzId="SIM_STEPS_ADMIN" userGroupId="ADMIN" artifactGroupId="SIM_STEPS"
            authzTypeEnumId="AUTHZT_ALWAYS" authzActionEnumId="AUTHZA_ALL"/>
</entity-facade-xml>
"#;

const COMPONENT: &str = r#"<component name="sim-steps"/>"#;
const ENTITIES: &str = r#"<entities>
    <entity entity-name="Widget" package="simsteps">
        <field name="widgetId" type="id" is-pk="true"/>
        <field name="widgetName" type="text-medium"/>
    </entity>
</entities>
"#;
const SERVICES: &str = r#"<services>
    <service verb="make" noun="Widget">
        <in-parameters><parameter name="widgetName"/></in-parameters>
        <out-parameters><parameter name="widgetId"/></out-parameters>
        <actions><service-call name="create#simsteps.Widget" in-map="context" out-map="context"/></actions>
    </service>
    <service verb="fail" noun="Widget">
        <in-parameters><parameter name="widgetName"/></in-parameters>
        <actions>
            <service-call name="create#simsteps.Widget" in-map="context"/>
            <return error="true" message="No widget today"/>
        </actions>
    </service>
    <service verb="rename" noun="Widget">
        <in-parameters><parameter name="widgetId" required="true"/><parameter name="widgetName"/></in-parameters>
        <actions><service-call name="update#simsteps.Widget" in-map="context"/></actions>
    </service>
    <service verb="count" noun="Widgets">
        <out-parameters><parameter name="widgetCount" type="Long"/><parameter name="widgetList" type="List"/></out-parameters>
        <actions>
            <entity-find-count entity-name="simsteps.Widget" count-field="widgetCount"/>
            <entity-find entity-name="simsteps.Widget" list="widgetList"><order-by field-name="widgetName"/></entity-find>
        </actions>
    </service>
</services>
"#;

const SERVICE: &str = "simsteps.WidgetServices";

fn step(verb_noun: &str, parameters: Json) -> Value {
    moqui_lib::json::from_json(&serde_json::json!({
        "serviceName": format!("{SERVICE}.{verb_noun}"),
        "parameters": parameters,
    }))
}

fn run_steps(factory: &Arc<ExecutionContextFactory>, user: &str, parameters: BTreeMap<String, Value>) -> Json {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user(user).unwrap();
    let out = moqui_core::service::sync("org.moqui.impl.SimServices.run#Steps", &parameters).unwrap();
    assert!(
        context.message().errors().is_empty(),
        "{:?}",
        context.message().errors()
    );
    moqui_lib::json::to_json(&Value::Map(out.into_iter().collect()))
}

fn names(factory: &Arc<ExecutionContextFactory>) -> Vec<String> {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let facade = factory.entity().unwrap();
    let rows = facade
        .find("simsteps.Widget")
        .unwrap()
        .order_by("widgetName", false)
        .list()
        .unwrap();
    rows.iter()
        .map(|row| moqui_lib::object::to_plain_string(row.get("widgetName").unwrap_or(&Value::Null)))
        .collect()
}

/// The widget names in a read's list, which comes as plain maps.
fn read_names(read: &Json) -> Vec<String> {
    read["readResult"]["widgetList"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["widgetName"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Four steps and a read after each: a failed step in the middle is rolled back alone, each step lists only its own
/// rows, created ids are the sim's negative ones, and production keeps none of it. A user whom a step's service does
/// not allow gets that step refused.
#[test]
fn steps_run_in_one_sim_each_under_its_own_savepoint() {
    if !crate::common::own_process() {
        return;
    }
    let files = [
        ("component/sim-steps/component.xml", COMPONENT),
        ("component/sim-steps/entity/SimStepsEntities.xml", ENTITIES),
        ("component/sim-steps/service/simsteps/WidgetServices.xml", SERVICES),
    ];
    let gate = start_with_files("sim-steps", Some(DATA), &[], &files);
    let factory = Arc::clone(gate.server.factory());
    if !factory.entity().unwrap().sim_available() {
        eprintln!("the sim overlay is not available here, so the steps are not checked");
        return;
    }
    let started = Instant::now();
    let parameters = BTreeMap::from([
        (
            "steps".to_string(),
            Value::List(vec![
                step("make#Widget", serde_json::json!({"widgetName": "First"})),
                step("fail#Widget", serde_json::json!({"widgetName": "Failed"})),
                step(
                    "rename#Widget",
                    serde_json::json!({"widgetId": "W_PROD", "widgetName": "Renamed"}),
                ),
                Value::string("not a step"),
                step(
                    "rename#Widget",
                    serde_json::json!({"widgetId": "W_PROD", "widgetName": "Again"}),
                ),
            ]),
        ),
        ("read".to_string(), step("count#Widgets", serde_json::json!({}))),
        ("readAfterEachStep".to_string(), Value::string("true")),
        ("label".to_string(), Value::string("a test of steps")),
    ]);
    let out = run_steps(&factory, "john.doe", parameters);
    assert_eq!(out["available"], true, "{out}");
    let ran = out["steps"].as_array().unwrap();
    assert_eq!(ran.len(), 5, "{out}");

    // 1: a create, with the sim's negative id.
    assert_eq!(ran[0]["ok"], true, "{out}");
    assert!(ran[0].get("error").is_none(), "{out}");
    let created = ran[0]["createdKeys"].as_array().unwrap();
    assert_eq!(created.len(), 1, "{out}");
    assert_eq!(created[0]["entity"], "simsteps.Widget");
    let id = created[0]["key"]["widgetId"].as_str().unwrap().to_string();
    assert!(id.starts_with('-'), "{id}");
    let rows = ran[0]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{out}");
    assert_eq!(rows[0]["action"], "create");
    assert_eq!(rows[0]["key"]["widgetId"], id.as_str());
    assert_eq!(ran[0]["readResult"]["widgetCount"], 2, "{out}");

    // 2: its create and its error are rolled back alone.
    assert_eq!(ran[1]["ok"], false, "{out}");
    assert!(ran[1]["error"].as_str().unwrap().contains("No widget today"), "{out}");
    assert_eq!(ran[1]["rows"], serde_json::json!([]));
    assert_eq!(ran[1]["createdKeys"], serde_json::json!([]));
    assert_eq!(ran[1]["readResult"]["widgetCount"], 2, "{out}");

    // 3: an update of a production row, before from production.
    assert_eq!(ran[2]["ok"], true, "{out}");
    let rows = ran[2]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only this step's row: {out}");
    assert_eq!(rows[0]["action"], "update");
    let field = &rows[0]["fields"].as_array().unwrap()[0];
    assert_eq!(field["field"], "widgetName");
    assert_eq!(field["before"], "Production");
    assert_eq!(field["after"], "Renamed");
    assert_eq!(ran[2]["createdKeys"], serde_json::json!([]));
    assert_eq!(read_names(&ran[2]), ["First", "Renamed"], "{out}");

    // 4: not a step, which fails alone.
    assert_eq!(ran[3]["ok"], false, "{out}");
    assert!(ran[3]["error"].as_str().unwrap().contains("serviceName"), "{out}");

    // 5: the same row again, before from the sim this time.
    assert_eq!(ran[4]["ok"], true, "{out}");
    let field = &ran[4]["rows"][0]["fields"][0];
    assert_eq!(field["before"], "Renamed", "{out}");
    assert_eq!(field["after"], "Again", "{out}");

    assert_eq!(read_names(&out), ["Again", "First"], "{out}");
    assert_eq!(names(&factory), ["Production"], "production keeps none of it");

    // Each step's service checks its own authorization.
    let call = SimCall {
        service_name: format!("{SERVICE}.make#Widget"),
        parameters: BTreeMap::from([("widgetName".to_string(), Value::string("Plain"))]),
    };
    let plain = steps::run(
        &factory,
        &StepsRequest {
            username: "sim.plain",
            label: "a plain user's steps",
            steps: std::slice::from_ref(&call),
            read: None,
            read_after_each_step: false,
        },
    );
    assert!(plain.available);
    assert!(!plain.steps[0].ok);
    let error = plain.steps[0].error.as_deref().unwrap();
    assert!(
        error.contains("not authorized") || error.contains("not allowed"),
        "{error}"
    );
    assert_eq!(names(&factory), ["Production"]);

    let took = started.elapsed();
    assert!(took.as_secs() < 10, "the steps took {took:?}");
}
