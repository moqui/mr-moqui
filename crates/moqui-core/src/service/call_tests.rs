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

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use moqui_lib::object::{basic_convert, Value};

use super::sync;
use crate::context::{ExecutionContext, ExecutionContextFactory};

#[test]
fn entity_auto_store_inserts_then_updates() {
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(Arc::clone(&tape));
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("testId".into(), Value::string("T1"));
    params.insert("testMedium".into(), Value::string("first"));
    let created = sync("store-moqui.test.TestEntity", &params).unwrap();
    assert_eq!(created.get("testId").and_then(Value::as_str), Some("T1"));
    params.insert("testMedium".into(), Value::string("second"));
    sync("store#moqui.test.TestEntity", &params).unwrap();
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("T1"))
        .one()
        .unwrap()
        .expect("stored row");
    assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("second"));
    let sql = tape.lock().unwrap_or_else(|err| err.into_inner()).join("\n");
    assert!(sql.contains("INSERT") && sql.contains("test_entity"), "{sql}");
    assert!(sql.contains("UPDATE") && sql.contains("test_entity"), "{sql}");
    assert!(!sql.contains("ON CONFLICT"), "{sql}");
}

/// `delete#moqui.test.TestEntity` removes the row. A following find is empty.
#[test]
fn delete_test_entity_removes_the_row() {
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("SVCTST1"));
    row.set("testMedium", Value::string("Test Name"));
    row.create(&facade).unwrap();
    let mut params = BTreeMap::new();
    params.insert("testId".into(), Value::string("SVCTST1"));
    sync("delete#moqui.test.TestEntity", &params).unwrap();
    let found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("SVCTST1"))
        .one()
        .unwrap();
    assert!(found.is_none());
}

/// `create#moqui.test.TestIntPk` with `intId` as the string `"123"` is found by that string and by the integer
/// `123`, and `testMedium` matches.
#[test]
fn create_test_int_pk_with_a_string_id_is_found_by_string_and_integer() {
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("intId".into(), Value::string("123"));
    params.insert("testMedium".into(), Value::string("Test Name"));
    sync("create#moqui.test.TestIntPk", &params).unwrap();
    let by_string = facade
        .find("moqui.test.TestIntPk")
        .unwrap()
        .eq("intId", Value::string("123"))
        .one()
        .unwrap()
        .expect("found by the string id");
    let by_int = facade
        .find("moqui.test.TestIntPk")
        .unwrap()
        .eq("intId", Value::I64(123))
        .one()
        .unwrap()
        .expect("found by the integer id");
    assert_eq!(by_string.get("testMedium").and_then(Value::as_str), Some("Test Name"));
    assert_eq!(by_int.get("testMedium").and_then(Value::as_str), Some("Test Name"));
}

/// An entity-auto create, update or store sets an empty form text as null (setFields with setIfEmpty), so a
/// blank optional field clears; an omitted field and nonempty whitespace keep their meanings.
#[test]
fn entity_auto_empty_fields_clear_to_null() {
    for declared in [false, true] {
        let (facade, ctx) = seeded_context(&[]);
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        if declared {
            let path = write_fixture(
                "entity-auto-empty",
                r#"<services>
                        <service verb="create" noun="moqui.test.TestEntity" type="entity-auto">
                            <in-parameters><auto-parameters/></in-parameters>
                        </service>
                        <service verb="update" noun="moqui.test.TestEntity" type="entity-auto">
                            <in-parameters><auto-parameters/></in-parameters>
                        </service>
                        <service verb="store" noun="moqui.test.TestEntity" type="entity-auto">
                            <in-parameters><auto-parameters/></in-parameters>
                        </service>
                    </services>"#,
            );
            assert!(crate::service::lint(&[path]).unwrap().is_empty());
        }
        for verb in ["create", "update", "store"] {
            let id = format!("empty-{verb}");
            if verb == "update" {
                let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
                row.set("testId", Value::string(&id));
                row.set("testMedium", Value::string("clear me"));
                row.set("testLong", Value::string("keep me"));
                row.create(&facade).unwrap();
            }
            let params = BTreeMap::from([
                ("testId".to_string(), Value::string(&id)),
                ("testMedium".to_string(), Value::string("")),
                ("testIndicator".to_string(), Value::string(" ")),
            ]);
            super::sync_checked(
                &format!("{verb}#moqui.test.TestEntity"),
                &params,
                &super::CallOptions::default(),
            )
            .unwrap();
            let stored = facade
                .find("moqui.test.TestEntity")
                .unwrap()
                .eq("testId", Value::string(&id))
                .use_cache(false)
                .one()
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.get("testMedium"),
                Some(&Value::Null),
                "{verb}, declared={declared}"
            );
            assert_eq!(stored.get("testIndicator"), Some(&Value::string(" ")));
            if verb == "update" {
                assert_eq!(stored.get("testLong"), Some(&Value::string("keep me")));
            }
        }
    }
}

#[test]
fn missing_required_parameter_is_named() {
    let path = write_fixture(
        "required",
        r#"<services>
                <service verb="need" noun="Value">
                    <in-parameters><parameter name="amount" required="true"/></in-parameters>
                </service>
            </services>"#,
    );
    let ctx = ExecutionContextFactory::test_context();
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    // As Java: the service does not run, the call returns nothing, and the error is a validation error on the
    // field, which a form shows beside it.
    let out = sync("need#Value", &BTreeMap::new()).unwrap();
    assert!(out.is_empty(), "{out:?}");
    let (errors, validation) = ExecutionContext::with_current(|ec| {
        let messages = ec.message();
        (messages.errors().to_vec(), messages.validation_errors().to_vec())
    })
    .unwrap();
    // Java's text, which names no field: the validation error carries it, and the errors string writes it with the
    // field and service, as Java's `toStringPretty`.
    assert_eq!(errors, ["Field cannot be empty"]);
    let text = ExecutionContext::with_current(|ec| ec.message().errors_string()).unwrap();
    assert_eq!(
        text,
        format!(
            "Field cannot be empty(for field Amount of service {}",
            moqui_lib::string::camel_case_to_pretty("need#Value")
        )
    );
    assert_eq!(validation.len(), 1);
    assert_eq!(
        (
            validation[0].field.as_str(),
            validation[0].service.as_str(),
            validation[0].message.as_str()
        ),
        ("amount", "need#Value", "Field cannot be empty")
    );
}

#[test]
fn held_fail_semaphore_errors() {
    let path = write_fixture(
        "semaphore",
        r#"<services>
                <service verb="lock" noun="Row" semaphore="fail">
                    <in-parameters><parameter name="note"/></in-parameters>
                </service>
            </services>"#,
    );
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with_tape(tape);
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    let mut row = facade
        .make_value("moqui.service.semaphore.ServiceParameterSemaphore")
        .unwrap();
    row.set("serviceName", Value::string("lock#Row"));
    row.set("parameterValue", Value::string("_NA_"));
    row.set("lockThread", Value::string("other"));
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    row.set("lockTime", basic_convert(&Value::I64(millis), "Timestamp").value);
    row.create(&facade).unwrap();
    let err = sync("lock#Row", &BTreeMap::new()).unwrap_err();
    let text = err.to_string();
    // Java's text, with the holder's thread
    assert!(
        text.contains("An instance of service semaphore lock#Row with parameter value [_NA_] is already running (thread [other], locked at "),
        "{text}"
    );
    assert!(
        text.contains("and it is setup to fail on semaphore conflict."),
        "{text}"
    );
    assert!(!text.contains("actions not run"), "{text}");
}

#[test]
fn a_stale_semaphore_is_taken_over_and_its_old_owner_cannot_free_the_new_hold() {
    let path = write_fixture(
        "semaphore-takeover",
        r#"<services><service verb="guard" noun="Row" semaphore="fail"/></services>"#,
    );
    let (facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    let mut quick =
        crate::service::define::ServiceDef::clone(&crate::service::define::find("guard#Row").expect("registered"));
    quick.semaphore_ignore = std::time::Duration::from_millis(1);
    let mut patient = quick.clone();
    patient.semaphore_ignore = std::time::Duration::from_secs(3600);
    let none = BTreeMap::new();

    let first = crate::service::semaphore::take(&quick, &none, &facade)
        .unwrap()
        .expect("first hold");
    assert!(
        crate::service::semaphore::take(&patient, &none, &facade).is_err(),
        "held"
    );
    std::thread::sleep(std::time::Duration::from_millis(10));
    let second = crate::service::semaphore::take(&quick, &none, &facade)
        .unwrap()
        .expect("takes over the stale hold");
    // The first holder finishing late must not free the second holder's lock.
    drop(first);
    assert!(
        crate::service::semaphore::take(&patient, &none, &facade).is_err(),
        "still held by the second"
    );
    drop(second);
    crate::service::semaphore::take(&patient, &none, &facade)
        .unwrap()
        .expect("free once the owner is done");
}

#[test]
fn inline_actions_find_and_create_rows() {
    let path = write_fixture(
        "inline",
        r#"<services>
                <service verb="copy" noun="Row">
                    <in-parameters><parameter name="seedId"/></in-parameters>
                    <out-parameters><parameter name="foundId"/></out-parameters>
                    <actions>
                        <entity-find entity-name="moqui.test.TestEntity" list="found">
                            <econdition field-name="testId" from="seedId"/>
                        </entity-find>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "B"); made.set("testMedium", "made"); foundId = found.get(0).get("testId");</script>
                        <entity-create value-field="made"/>
                    </actions>
                </service>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ctx.disable_authz();
    let mut seed = facade.make_value("moqui.test.TestEntity").unwrap();
    seed.set("testId", Value::string("A"));
    seed.set("testMedium", Value::string("seed"));
    seed.create(&facade).unwrap();
    let mut params = BTreeMap::new();
    params.insert("seedId".into(), Value::string("A"));
    let out = sync("copy#Row", &params).unwrap();
    assert_eq!(out.get("foundId").and_then(Value::as_str), Some("A"));
    let seed_row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("A"))
        .one()
        .unwrap()
        .expect("seed row");
    let made_row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("B"))
        .one()
        .unwrap()
        .expect("created row");
    assert_eq!(seed_row.get("testMedium").and_then(Value::as_str), Some("seed"));
    assert_eq!(made_row.get("testMedium").and_then(Value::as_str), Some("made"));
}

fn seeded_context(tape_rows: &[(&str, &str)]) -> (crate::entity::EntityFacade, crate::context::ExecutionContext) {
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade.clone());
    {
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        for (id, medium) in tape_rows {
            let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
            row.set("testId", Value::string(*id));
            row.set("testMedium", Value::string(*medium));
            row.create(&facade).unwrap();
        }
    }
    (facade, ctx)
}

#[test]
fn service_failures_identify_owning_actions_and_rust_origin() {
    let path = write_fixture(
        "source-locations",
        "<services>\n<service verb=\"locate\" noun=\"Failure\">\n<actions><script><![CDATA[\nconst location = 'component://MissingDiagnostics/screen/A.xml';\nec.resource.getLocationText(location);\n]]></script></actions>\n</service>\n</services>",
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    // As Java's, the failure is recorded in the message facade and the call returns its empty results. The facade
    // has the message for the client; the server log has where it came from.
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let out = tracing::subscriber::with_default(crate::testing::LogSubscriber(std::sync::Arc::clone(&lines)), || {
        sync("locate#Failure", &BTreeMap::new())
    })
    .expect("a body failure is recorded, not returned");
    assert!(out.is_empty(), "{out:?}");
    let errors = ctx.message().errors().join("\n");
    assert_eq!(
        errors,
        "Error: component MissingDiagnostics is not registered for location component://MissingDiagnostics/screen/A.xml"
    );
    let diagnostic = lines.lock().unwrap().join("\n");
    assert!(
        diagnostic.contains(&format!("{}#actions-3-1.generated.js:", path.display())),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("Rust: crates/moqui-core/src/context/resource.rs:"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("in service locate#Failure"), "{diagnostic}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn action_outputs_reach_out_parameters_without_script_assignment() {
    let path = write_fixture(
        "outputs",
        r#"<services>
                <service verb="probe" noun="Row">
                    <in-parameters><parameter name="testId"/></in-parameters>
                    <out-parameters>
                        <parameter name="foundCount" type="Integer"/><parameter name="oneMedium"/>
                        <parameter name="lastId"/><parameter name="cmp"/>
                    </out-parameters>
                    <actions>
                        <entity-find entity-name="moqui.test.TestEntity" list="found">
                            <econdition field-name="testId" from="testId"/>
                        </entity-find>
                        <entity-find-one entity-name="moqui.test.TestEntity" value-field="oneRow"/>
                        <set field="foundCount" from="found.length"/>
                        <set field="oneMedium" from="oneRow.get('testMedium')"/>
                        <iterate list="found" entry="row"><set field="lastId" from="row.get('testId')"/></iterate>
                        <if><condition><compare field="foundCount" operator="greater" value="0" type="Integer"/></condition>
                            <then><set field="cmp" value="yes"/></then></if>
                    </actions>
                </service>
            </services>"#,
    );
    let (_facade, ctx) = seeded_context(&[("A", "alpha")]);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("testId".into(), Value::string("A"));
    let out = sync("probe#Row", &params).unwrap();
    assert_eq!(
        out.get("foundCount").map(moqui_lib::object::to_plain_string).as_deref(),
        Some("1"),
        "{out:?}"
    );
    assert_eq!(out.get("oneMedium").and_then(Value::as_str), Some("alpha"));
    assert_eq!(out.get("lastId").and_then(Value::as_str), Some("A"));
    assert_eq!(out.get("cmp").and_then(Value::as_str), Some("yes"));
    // The callee's names do not leak into the caller's frame.
    assert!(crate::script::context_value("foundCount").is_none());
}

#[test]
fn service_call_action_passes_parameters_and_merges_out_map() {
    let path = write_fixture(
        "nested-call",
        r#"<services>
                <service verb="double" noun="Num">
                    <in-parameters><parameter name="x" type="Integer"/></in-parameters>
                    <out-parameters><parameter name="y" type="Integer"/></out-parameters>
                    <actions><set field="y" from="x + x"/></actions>
                </service>
                <service verb="outer" noun="Num">
                    <in-parameters><parameter name="n" type="Integer"/></in-parameters>
                    <out-parameters><parameter name="answer" type="Integer"/><parameter name="after"/></out-parameters>
                    <actions>
                        <service-call name="double#Num" out-map="res"><field-map field-name="x" from="n"/></service-call>
                        <set field="answer" from="res.y"/>
                        <service-call name="nothing#Here" ignore-error="true"/>
                        <set field="after" value="ran"/>
                    </actions>
                </service>
                <service verb="onto" noun="Context">
                    <in-parameters><parameter name="n" type="Integer"/></in-parameters>
                    <out-parameters><parameter name="answer" type="Integer"/></out-parameters>
                    <actions>
                        <service-call name="double#Num" out-map="context"><field-map field-name="x" from="n"/></service-call>
                        <set field="answer" from="y"/>
                    </actions>
                </service>
                <service verb="stops" noun="Num">
                    <out-parameters><parameter name="after"/></out-parameters>
                    <actions>
                        <service-call name="nothing#Here"/>
                        <set field="after" value="ran"/>
                    </actions>
                </service>
            </services>"#,
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("n".into(), Value::I32(21));
    let out = sync("outer#Num", &params).unwrap();
    assert_eq!(
        out.get("answer").map(moqui_lib::object::to_plain_string).as_deref(),
        Some("42"),
        "{out:?}"
    );
    assert_eq!(out.get("after").and_then(Value::as_str), Some("ran"));
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    // out-map="context", the default of a transition's service-call: the results go on the stack by name
    let out = sync("onto#Context", &params).unwrap();
    assert_eq!(
        out.get("answer").map(moqui_lib::object::to_plain_string).as_deref(),
        Some("42"),
        "{out:?}"
    );
    let out = sync("stops#Num", &BTreeMap::new()).unwrap();
    assert!(!out.contains_key("after"), "an unignored error stops the actions");
    assert!(ctx.message().errors().iter().any(|text| text.contains("nothing#Here")));
}

#[test]
fn message_errors_roll_back_but_return_the_results() {
    let path = write_fixture(
        "message-error",
        r#"<services>
                <service verb="halfway" noun="Row">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "ROLLED"); note = "ran";</script>
                        <entity-create value-field="made"/>
                        <message error="true">boom</message>
                    </actions>
                </service>
            </services>"#,
    );
    let (facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    let out = sync("halfway#Row", &BTreeMap::new()).expect("the call itself did not fail");
    assert_eq!(out.get("note").and_then(Value::as_str), Some("ran"));
    assert_eq!(ctx.message().errors(), ["boom"]);
    let rows = facade.find("moqui.test.TestEntity").unwrap().list().unwrap();
    assert_eq!(rows.len(), 0, "the create was rolled back with the message error");
}

/// Java's `ServiceCallback`, recording what it hears.
struct Heard(Mutex<Vec<String>>);

impl super::ServiceCallback for Heard {
    fn receive_result(&self, _context: &BTreeMap<String, Value>, result: &BTreeMap<String, Value>) {
        self.0.lock().unwrap().push(format!("result {}", result.len()));
    }
    fn receive_error(&self, _context: &BTreeMap<String, Value>, error: &moqui_lib::error::Error) {
        self.0.lock().unwrap().push(format!("error {}", error.message()));
    }
}

/// As Java's `callSingle`: a failing body is recorded in the message facade, the callbacks hear of the error, the
/// transaction rolls back (and its tx-rollback rules run), the post-commit rules that run on error get an empty
/// result map, and the call returns that empty map.
#[test]
fn a_body_failure_is_recorded_rolled_back_and_returns_empty_results() {
    let path = write_fixture(
        "body-failure",
        r#"<services>
                <service verb="break" noun="Late">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions>
                        <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                        <script>made.set("testId", "ROLLED"); note = "ran";</script>
                        <entity-create value-field="made"/>
                        <script>throw new Error("late boom")</script>
                    </actions>
                </service>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="break#Late" when="post-commit">
                        <actions><script>ec.message.addMessage("plain")</script></actions>
                    </seca>
                    <seca service="break#Late" when="post-commit" run-on-error="true">
                        <actions><script>ec.message.addMessage("cleanup " + (results == null ? "none" : JSON.stringify(results)))</script></actions>
                    </seca>
                    <seca service="break#Late" when="tx-rollback">
                        <actions>
                            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                            <script>made.set("testId", "ROLLBACK_RULE")</script>
                            <entity-create value-field="made"/>
                        </actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade.clone());
    let heard = Arc::new(Heard(Mutex::new(Vec::new())));
    super::register_callback("break#Late", heard.clone());
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    let out = sync("break#Late", &BTreeMap::new()).expect("a body failure is recorded, not returned");
    assert!(out.is_empty(), "{out:?}");
    // the message only: the generated line and the service it ran in are in the log (service_failures_identify_...)
    let errors = ctx.message().errors().to_vec();
    assert_eq!(errors, ["Error: late boom"]);
    assert_eq!(ctx.message().messages(), ["cleanup {}"]);
    let heard = heard.0.lock().unwrap().clone();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert!(
        heard[0].starts_with("error ") && heard[0].contains("late boom"),
        "{heard:?}"
    );
    let find = |id: &str| {
        facade
            .find("moqui.test.TestEntity")
            .unwrap()
            .eq("testId", Value::string(id))
            .one()
            .unwrap()
    };
    assert!(find("ROLLED").is_none(), "the create was rolled back");
    assert!(find("ROLLBACK_RULE").is_some(), "the tx-rollback rule ran");
}

/// As Java's `finally`: a commit that fails is recorded in the message facade, after the callbacks heard the
/// results, and the call returns those results; the post-commit rules that run on error get them too.
#[test]
fn a_commit_failure_is_recorded_and_returns_the_results() {
    let path = write_fixture(
        "commit-failure",
        r#"<services>
                <service verb="refuse" noun="Commit" type="rust" location="test::refuse_commit">
                    <out-parameters><parameter name="note"/></out-parameters>
                </service>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory()
        .register_service("test::refuse_commit", |ec, _info, _parameters| {
            let facade = ec.factory().entity().expect("an entity facade");
            let mut row = facade.make_value("moqui.test.TestEntity")?;
            row.set("testId", Value::string("UNCOMMITTED"));
            row.create(&facade)?;
            // A hook that runs before the commit, as the data feed's insert does, and fails it.
            facade
                .transactions_for("moqui.test.TestEntity")?
                .on_feed_insert(|_| Err(moqui_lib::error::Error::new("feed insert refused")))?;
            Ok(BTreeMap::from([("note".to_string(), Value::string("ran"))]))
        })
        .unwrap();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="refuse#Commit" when="post-commit" run-on-error="true">
                        <actions><script>ec.message.addMessage("cleanup " + JSON.stringify(results))</script></actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade.clone());
    let heard = Arc::new(Heard(Mutex::new(Vec::new())));
    super::register_callback("refuse#Commit", heard.clone());
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ctx.disable_authz();
    let out = sync("refuse#Commit", &BTreeMap::new()).expect("a commit failure is recorded, not returned");
    assert_eq!(out.get("note").and_then(Value::as_str), Some("ran"));
    let errors = ctx.message().errors().join("\n");
    assert!(errors.contains("feed insert refused"), "{errors}");
    assert_eq!(ctx.message().messages(), [r#"cleanup {"note":"ran"}"#]);
    assert_eq!(*heard.0.lock().unwrap(), ["result 1"]);
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("UNCOMMITTED"))
        .one()
        .unwrap();
    assert!(row.is_none(), "the write was rolled back");
}

/// An interface cannot be run: as Java, an error before the call's transaction, so no post-commit rule runs.
#[test]
fn an_interface_service_is_an_error_before_it_runs() {
    let path = write_fixture(
        "interface",
        r#"<services>
                <service verb="shell" noun="Only" type="interface"/>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="shell#Only" when="post-commit" run-on-error="true">
                        <actions><script>ec.message.addMessage("cleanup")</script></actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    let err = sync("shell#Only", &BTreeMap::new()).unwrap_err();
    assert!(err.message().contains("is an interface and cannot be run"), "{err}");
    assert!(ctx.message().messages().is_empty(), "{:?}", ctx.message().messages());
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
}

#[test]
fn parameters_are_converted_then_checked_and_defaults_apply() {
    let path = write_fixture(
        "validation",
        r#"<services>
                <service verb="typed" noun="Input">
                    <in-parameters>
                        <parameter name="count" type="Integer"><number-range min="1" max="10"/></parameter>
                        <parameter name="label"/>
                        <parameter name="rich" allow-html="any"/>
                        <parameter name="careful" allow-html="safe"/>
                        <parameter name="calc" type="Integer" default="2 + 3"/>
                        <parameter name="tpl" default-value="${label}-suffix"/>
                        <parameter name="later" type="Integer"/>
                        <parameter name="sum" type="Integer" default="calc + later"/>
                    </in-parameters>
                </service>
            </services>"#,
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let text = |value: &str| Value::string(value);
    let call = |pairs: &[(&str, Value)]| {
        let params: BTreeMap<String, Value> = pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
        crate::service::validate("typed#Input", &params)
    };

    let ok = call(&[("count", text("7")), ("label", text("hi"))]).unwrap();
    assert_eq!(ok.get("count"), Some(&Value::I32(7)), "converted to the declared type");
    assert_eq!(ok.get("calc"), Some(&Value::I32(5)), "default expression");
    assert_eq!(
        ok.get("tpl").and_then(Value::as_str),
        Some("hi-suffix"),
        "default-value expands"
    );
    // A default sees the parameters before it as cleaned: a default, and a number converted after the first default.
    let summed = call(&[("label", text("hi")), ("later", text("4"))]).unwrap();
    assert_eq!(summed.get("sum"), Some(&Value::I32(9)), "{summed:?}");

    let bad = call(&[("count", text("abc"))]).unwrap_err();
    assert!(
        bad.to_string()
            .contains("Value entered (abc) could not be converted to a Integer"),
        "{bad}"
    );
    let range = call(&[("count", text("11"))]).unwrap_err();
    assert!(
        range.to_string().contains("count"),
        "a range check sees the converted number: {range}"
    );

    let html = call(&[("label", text("<b>x</b>"))]).unwrap_err();
    assert!(html.to_string().contains("HTML not allowed"), "{html}");
    call(&[("rich", text("<b>x</b>"))]).expect("allow-html any");
    let cleaned = call(&[(
        "careful",
        text(r#"<b onclick="x()">hi</b><script>bad()</script> <a href="javascript:evil()">l</a> <img src="http://h/i.png" onerror="x()">"#),
    )])
    .expect("safe cleans instead of rejecting");
    let out = cleaned.get("careful").and_then(Value::as_str).unwrap().to_string();
    assert!(out.contains("<b>hi</b>"), "{out}");
    assert!(
        !out.contains("script") && !out.contains("bad()") && !out.contains("onclick"),
        "{out}"
    );
    assert!(!out.contains("javascript:") && !out.contains("onerror"), "{out}");
    assert!(out.contains(r#"src="http://h/i.png""#), "{out}");
    let link = call(&[("careful", text(r#"<a href="https://example.com/">go</a>"#))]).unwrap();
    let link = link.get("careful").and_then(Value::as_str).unwrap().to_string();
    assert!(link.contains(r#"rel="noopener noreferrer""#), "{link}");
    let plain = call(&[("careful", text("a & b, no markup"))]).unwrap();
    assert_eq!(plain.get("careful").and_then(Value::as_str), Some("a & b, no markup"));
}

#[test]
fn an_update_service_with_auto_parameters_accepts_a_changed_field() {
    let (facade, ctx) = seeded_context(&[("U1", "before")]);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("testId".into(), Value::string("U1"));
    params.insert("testMedium".into(), Value::string("after"));
    sync("update#moqui.test.TestEntity", &params).expect("update by primary key with a changed field");
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("U1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("after"));
}

/// Java's entity-auto `update`: only the fields passed are written, and a `statusId` is checked against the stored
/// status (read when the update runs, so after any change since a caller read the row), which the results give as
/// `oldStatusId`.
#[test]
fn an_entity_auto_update_writes_the_fields_passed_and_checks_the_stored_status() {
    let (facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let entity = "moqui.service.message.SystemMessage";
    let mut row = facade.make_value(entity).unwrap();
    row.set("systemMessageId", Value::string("M1"));
    row.set("statusId", Value::string("SmsgReceived"));
    row.set("messageText", Value::string("keep"));
    row.set("failCount", Value::I64(2));
    row.create(&facade).unwrap();
    for (from, to) in [("SmsgReceived", "SmsgConsumed"), ("SmsgConsumed", "SmsgConfirmed")] {
        let mut transition = facade.make_value("moqui.basic.StatusFlowTransition").unwrap();
        transition.set("statusFlowId", Value::string("Default"));
        transition.set("statusId", Value::string(from));
        transition.set("toStatusId", Value::string(to));
        transition.create(&facade).unwrap();
    }
    let read = || {
        facade
            .find(entity)
            .unwrap()
            .eq("systemMessageId", Value::string("M1"))
            .use_cache(false)
            .one()
            .unwrap()
            .unwrap()
    };
    // A caller read the row, then another changed its status.
    let mut stale = read();
    let mut other = read();
    other.set("statusId", Value::string("SmsgConsumed"));
    other.update(&facade).unwrap();
    let params = BTreeMap::from([
        ("systemMessageId".to_string(), Value::string("M1")),
        ("statusId".to_string(), Value::string("SmsgConfirmed")),
    ]);
    let out = super::sync_checked(&format!("update#{entity}"), &params, &super::CallOptions::default()).unwrap();
    assert_eq!(out.get("oldStatusId"), Some(&Value::string("SmsgConsumed")), "{out:?}");
    assert_eq!(out.get("statusChanged"), Some(&Value::Bool(true)), "{out:?}");
    let stored = read();
    assert_eq!(stored.get("statusId"), Some(&Value::string("SmsgConfirmed")));
    assert_eq!(stored.get("messageText"), Some(&Value::string("keep")));
    // The stale value changes another field; the status it read is not written back.
    stale.set("messageText", Value::string("stale"));
    stale.update(&facade).unwrap();
    assert_eq!(read().get("statusId"), Some(&Value::string("SmsgConfirmed")));
    assert_eq!(read().get("messageText"), Some(&Value::string("stale")));
    let back = BTreeMap::from([
        ("systemMessageId".to_string(), Value::string("M1")),
        ("statusId".to_string(), Value::string("SmsgReceived")),
    ]);
    let err = super::sync_checked(&format!("update#{entity}"), &back, &super::CallOptions::default()).unwrap_err();
    assert!(err.to_string().contains("Status change not allowed"), "{err}");
    assert_eq!(read().get("statusId"), Some(&Value::string("SmsgConfirmed")));
}

#[test]
fn a_script_service_runs_its_javascript_file_and_optional_chaining_loads() {
    let script = std::env::temp_dir().join(format!("moqui-script-service-{}.js", std::process::id()));
    std::fs::write(&script, "shout = greeting.toUpperCase() + '!';").unwrap();
    let path = write_fixture(
        "script-service",
        &format!(
            r#"<services>
                    <service verb="shout" noun="Greeting" type="script" location="{}">
                        <in-parameters><parameter name="greeting"/></in-parameters>
                        <out-parameters><parameter name="shout"/></out-parameters>
                    </service>
                    <service verb="chain" noun="Safely">
                        <out-parameters><parameter name="seen"/></out-parameters>
                        <actions><set field="seen" from="missing?.name ?? 'none'"/></actions>
                    </service>
                    <service verb="groovy" noun="Elvis">
                        <actions><set field="x" from="a ?: b"/></actions>
                    </service>
                </services>"#,
            script.display()
        ),
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert_eq!(failures.len(), 1, "only the Elvis service fails: {failures:?}");
    assert!(failures[0].contains("groovy#Elvis"), "{failures:?}");
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("greeting".into(), Value::string("hello"));
    let out = sync("shout#Greeting", &params).unwrap();
    assert_eq!(out.get("shout").and_then(Value::as_str), Some("HELLO!"));
    let out = sync("chain#Safely", &BTreeMap::new()).unwrap();
    assert_eq!(out.get("seen").and_then(Value::as_str), Some("none"));
}

#[test]
fn a_runaway_script_ends_at_the_transaction_deadline() {
    let path = write_fixture(
        "runaway",
        r#"<services>
                <service verb="spin" noun="Forever" transaction-timeout="1">
                    <actions><script>while (true) {}</script></actions>
                </service>
            </services>"#,
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ctx.disable_authz();
    let started = std::time::Instant::now();
    let out = sync("spin#Forever", &BTreeMap::new()).expect("the interruption is recorded");
    assert!(out.is_empty(), "{out:?}");
    assert!(
        !ctx.message().errors().is_empty(),
        "the interruption is an error message"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn post_service_seca_runs_its_action() {
    let path = write_fixture(
        "seca",
        r#"<services>
                <service verb="mark" noun="Row"/>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="mark#Row" when="post-service">
                        <actions><script>ec.message.addMessage("seen")</script></actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    sync("mark#Row", &BTreeMap::new()).unwrap();
    // The rule runs in its own context frame, so its effect is seen through the message facade.
    assert!(ctx.message().messages().iter().any(|text| text == "seen"));
}

/// As Java's, `parameters` and `results` in a rule are the call's own maps: a pre-* rule changes what is validated
/// and run, and a post-service rule what is returned.
#[test]
fn seca_rules_change_the_call_parameters_and_results() {
    let path = write_fixture(
        "seca-live",
        r#"<services>
                <service verb="echo" noun="Live">
                    <in-parameters><parameter name="note" required="true"/></in-parameters>
                    <out-parameters><parameter name="seen"/></out-parameters>
                    <actions><set field="seen" from="note"/></actions>
                </service>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="echo#Live" when="pre-validate">
                        <actions><set field="parameters.note" value="pv"/></actions>
                    </seca>
                    <seca service="echo#Live" when="pre-auth">
                        <actions><script>parameters.note = parameters.note + "-pa"</script></actions>
                    </seca>
                    <seca service="echo#Live" when="pre-service">
                        <actions><script>parameters.note = parameters.note + "-ps"</script></actions>
                    </seca>
                    <seca service="echo#Live" when="post-service">
                        <actions><script>results.seen = results.seen + "-post"</script></actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    // `note` is required and not passed: the pre-validate rule sets it.
    let out = sync("echo#Live", &BTreeMap::new()).unwrap();
    assert!(ctx.message().errors().is_empty(), "{:?}", ctx.message().errors());
    assert_eq!(
        out.get("seen").and_then(Value::as_str),
        Some("pv-pa-ps-post"),
        "{out:?}"
    );
}

/// As Java's `runImplicitEntityAuto`: an implicit entity-auto call runs its pre-validate rules twice and no pre-auth
/// rules.
#[test]
fn an_implicit_entity_auto_call_runs_pre_validate_twice_and_no_pre_auth() {
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="create#moqui.test.TestEntity" when="pre-validate">
                        <actions><script>ec.message.addMessage("pre-validate")</script></actions>
                    </seca>
                    <seca service="create#moqui.test.TestEntity" when="pre-auth">
                        <actions><script>ec.message.addMessage("pre-auth")</script></actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    let mut params = BTreeMap::new();
    params.insert("testId".to_string(), Value::string("IMPLICIT"));
    sync("create#moqui.test.TestEntity", &params).unwrap();
    assert_eq!(ctx.message().messages(), ["pre-validate", "pre-validate"]);
}

/// As Java's, the tx-commit rules get the parameters and results from before the post-service rules.
#[test]
fn tx_commit_rules_get_the_results_from_before_the_post_service_rules() {
    let path = write_fixture(
        "tx-commit",
        r#"<services>
                <service verb="note" noun="Commit">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions><set field="note" value="from-service"/></actions>
                </service>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="note#Commit" when="post-service">
                        <actions><script>results.note = "from-post-service"</script></actions>
                    </seca>
                    <seca service="note#Commit" when="tx-commit">
                        <actions>
                            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                            <script>made.set("testId", "COMMIT_RULE"); made.set("testMedium", results.note)</script>
                            <entity-create value-field="made"/>
                        </actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    let out = sync("note#Commit", &BTreeMap::new()).unwrap();
    assert_eq!(out.get("note").and_then(Value::as_str), Some("from-post-service"));
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("COMMIT_RULE"))
        .one()
        .unwrap()
        .expect("the tx-commit rule ran");
    assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("from-service"));
}

/// As Java's, the tx-rollback rules are registered after the service ran, with its results.
#[test]
fn tx_rollback_rules_get_the_results() {
    let path = write_fixture(
        "tx-rollback",
        r#"<services>
                <service verb="fail" noun="Late">
                    <out-parameters><parameter name="note"/></out-parameters>
                    <actions>
                        <set field="note" value="before-error"/>
                        <message error="true">late failure</message>
                    </actions>
                </service>
            </services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    super::super::eca_rules::install(
        ctx.factory(),
        super::super::eca_rules::from_xml(
            r#"<secas>
                    <seca service="fail#Late" when="tx-rollback">
                        <actions>
                            <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                            <script>made.set("testId", "ROLLBACK_RULE"); made.set("testMedium", results.note)</script>
                            <entity-create value-field="made"/>
                        </actions>
                    </seca>
                </secas>"#,
            "",
        ),
    );
    ctx.factory().set_entity(facade.clone());
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let _authz = ctx.disable_authz();
    sync("fail#Late", &BTreeMap::new()).unwrap();
    assert_eq!(ctx.message().errors(), ["late failure"]);
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("ROLLBACK_RULE"))
        .one()
        .unwrap()
        .expect("the tx-rollback rule ran");
    assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("before-error"));
}

#[test]
fn remote_rest_in_sim_mode_errors_before_a_socket() {
    let path = write_fixture(
        "remote",
        r#"<services>
                <service verb="ping" noun="Remote" type="remote-rest" authenticate="false" location="http://127.0.0.1:1/no-such-remote"/>
            </services>"#,
    );
    let facade = crate::testing::open();
    facade.set_sim_active(true);
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let _authz = ctx.disable_authz();
    assert!(sync("ping#Remote", &BTreeMap::new()).unwrap().is_empty());
    let text = ctx.message().errors().join("\n");
    assert!(text.contains("sim mode is active"), "{text}");
}

#[test]
fn auto_parameters_are_expanded_once_per_entity_facade() {
    let path = write_fixture(
        "auto-cache",
        r#"<services><service verb="take" noun="Auto">
                <in-parameters><auto-parameters entity-name="moqui.test.TestEntity" include="pk"/></in-parameters>
            </service></services>"#,
    );
    let (facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let def = crate::service::define::find("take#Auto").expect("registered");
    let first = super::validate::declared_in(&def, Some(&facade)).unwrap();
    assert!(first.iter().any(|param| param.name == "testId"), "{first:?}");
    let again = super::validate::declared_in(&def, Some(&facade)).unwrap();
    assert!(Arc::ptr_eq(&first, &again), "kept on the definition");
    // a relink swaps the model behind the same facade and drops what was read from it (plans/design-dev-aide.md F5)
    crate::entity::derived::clear(&facade);
    let relinked = super::validate::declared_in(&def, Some(&facade)).unwrap();
    assert!(!Arc::ptr_eq(&first, &relinked), "expanded again from the new model");
    let other = crate::testing::open();
    let expanded = super::validate::declared_in(&def, Some(&other)).unwrap();
    assert!(!Arc::ptr_eq(&first, &expanded), "another facade expands again");
    assert_eq!(
        expanded.iter().map(|param| &param.name).collect::<Vec<_>>(),
        first.iter().map(|param| &param.name).collect::<Vec<_>>()
    );
}

#[test]
fn auto_parameters_in_out_parameters_are_returned() {
    let path = write_fixture(
        "auto-out",
        r#"<services><service verb="give" noun="AutoOut">
                <out-parameters><auto-parameters entity-name="moqui.test.TestEntity" include="nonpk"/></out-parameters>
                <actions><set field="testMedium" value="kept"/><set field="notDeclared" value="dropped"/></actions>
            </service></services>"#,
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let out = sync("give#AutoOut", &BTreeMap::new()).unwrap();
    assert_eq!(
        out.get("testMedium").map(moqui_lib::object::to_plain_string).as_deref(),
        Some("kept"),
        "{out:?}"
    );
    assert!(!out.contains_key("notDeclared"), "{out:?}");
}

#[test]
fn auto_parameters_of_a_view_entity_use_its_aliases() {
    let path = write_fixture(
        "auto-view",
        r#"<services><service verb="give" noun="AutoView">
                <in-parameters><auto-parameters entity-name="moqui.test.TestEntityCount"/></in-parameters>
                <out-parameters><auto-parameters entity-name="moqui.test.TestEntityCount"/></out-parameters>
                <actions><set field="entityCount" value="7"/><set field="notDeclared" value="dropped"/></actions>
            </service></services>"#,
    );
    let (_facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let out = sync("give#AutoView", &BTreeMap::new()).unwrap();
    assert_eq!(
        out.get("entityCount")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("7"),
        "{out:?}"
    );
    assert!(!out.contains_key("notDeclared"), "{out:?}");
}

/// Java's `ServiceDefinition` merge: the implemented service's parameters first, then the service's own merged
/// into them by name, and an `auto-parameters` field and a parameter of the same name are one parameter, whose
/// attributes the later one sets. An empty `default-value` is no default. A `<description>` of the later one
/// replaces the earlier one's, and none keeps it.
#[test]
fn implemented_and_auto_parameters_merge_by_name() {
    let path = write_fixture(
        "merge",
        r#"<services>
                <service verb="define" noun="Row" type="interface">
                    <in-parameters>
                        <parameter name="testId" required="true"><description>Inherited id</description></parameter>
                        <parameter name="note" default-value="n"><description>Inherited note</description></parameter>
                        <parameter name="testMedium"><description>Inherited medium</description></parameter>
                    </in-parameters>
                </service>
                <service verb="save" noun="Row">
                    <implements service="define#Row"/>
                    <in-parameters>
                        <auto-parameters entity-name="moqui.test.TestEntity" include="nonpk"/>
                        <parameter name="testDateTime" required="true"><description>Own stamp</description></parameter>
                        <parameter name="testId"><text-length max="5"/></parameter>
                        <parameter name="note" default-value=""><description> Own note </description></parameter>
                    </in-parameters>
                    <actions><set field="x" value="1"/></actions>
                </service>
            </services>"#,
    );
    let (facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let failures = crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(failures.is_empty(), "{failures:?}");
    let def = crate::service::define::find("save#Row").expect("registered");
    let declared = super::validate::declared_in(&def, Some(&facade)).unwrap();
    let names: Vec<&str> = declared.iter().map(|param| param.name.as_str()).collect();
    assert_eq!(&names[..3], ["testId", "note", "testMedium"], "{names:?}");
    assert_eq!(
        names.iter().filter(|name| **name == "testDateTime").count(),
        1,
        "{names:?}"
    );
    let stamp = declared.iter().find(|param| param.name == "testDateTime").unwrap();
    assert_eq!(stamp.java_type, "java.sql.Timestamp");
    assert_eq!(stamp.required, super::super::define::Required::True);
    assert_eq!(stamp.field_name.as_deref(), Some("testDateTime"));
    let id = declared.iter().find(|param| param.name == "testId").unwrap();
    assert_eq!(
        (id.required, id.checks.len()),
        (super::super::define::Required::True, 1)
    );
    assert_eq!(
        declared
            .iter()
            .find(|param| param.name == "note")
            .unwrap()
            .default_value,
        None
    );
    let description = |name: &str| {
        declared
            .iter()
            .find(|param| param.name == name)
            .and_then(|param| param.description.as_deref())
    };
    assert_eq!(description("testId"), Some("Inherited id"), "none of its own keeps it");
    assert_eq!(description("note"), Some("Own note"), "its own replaces it");
    assert_eq!(
        description("testMedium"),
        Some("Inherited medium"),
        "an auto field has none"
    );
    assert_eq!(description("testDateTime"), Some("Own stamp"), "over an auto field");
    assert_eq!(description("testLong"), None);

    let params = |pairs: &[(&str, &str)]| -> BTreeMap<String, Value> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), Value::string(*value)))
            .collect()
    };
    let cleaned = crate::service::validate(
        "save#Row",
        &params(&[("testId", "R1"), ("testDateTime", "2024-01-02 03:04:05")]),
    )
    .unwrap();
    assert!(
        matches!(cleaned.get("testDateTime"), Some(Value::Timestamp(_))),
        "{cleaned:?}"
    );
    assert!(!cleaned.contains_key("note"), "{cleaned:?}");
    let err = crate::service::validate("save#Row", &params(&[("testId", "ABCDEFG")])).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("Value entered (ABCDEFG), length 7, is longer than 5 characters."),
        "{text}"
    );
    assert!(
        text.contains("Field cannot be empty (for field testDateTime)"),
        "{text}"
    );
}

/// A declared entity-auto service gets only its declared parameters, as any service; an implicit one gets them
/// as they came, and the runner sets only the entity's fields.
#[test]
fn entity_auto_parameters_are_the_declared_ones_or_all_for_an_implicit_call() {
    let path = write_fixture(
        "entity-auto-declared",
        r#"<services>
                <service verb="create" noun="moqui.test.TestEntity" type="entity-auto">
                    <in-parameters><auto-parameters include="nonpk"/></in-parameters>
                    <out-parameters><parameter name="testId"/></out-parameters>
                </service>
            </services>"#,
    );
    let (facade, ctx) = seeded_context(&[]);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    let mut params = BTreeMap::new();
    params.insert("testId".to_string(), Value::string("MINE"));
    params.insert("testMedium".to_string(), Value::string("declared"));
    let created = sync("create#moqui.test.TestEntity", &params).unwrap();
    let id = created
        .get("testId")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    assert!(
        !id.is_empty() && id != "MINE",
        "the primary key is not a declared parameter: {created:?}"
    );

    let implicit =
        super::super::define::ServiceDef::implicit("store#moqui.test.TestEntity", "store", "moqui.test.TestEntity");
    params.insert("notAField".to_string(), Value::string("x"));
    let cleaned = super::validate::clean(&implicit, &params, Some(&facade)).unwrap();
    assert_eq!(cleaned, params);
    sync("store#moqui.test.TestEntity", &params).unwrap();
    let row = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("MINE"))
        .one()
        .unwrap()
        .expect("stored");
    assert_eq!(row.get("testMedium").and_then(Value::as_str), Some("declared"));
}

/// Java's `StatusFlowTransitionNotFoundTemplate`, its default text when the seed data is not loaded.
#[test]
fn a_status_change_not_allowed_names_the_statuses() {
    let values = |old_name: Value| {
        [
            ("fullEntityName", Value::string("moqui.test.TestEntity")),
            ("lookedUpStatusId", Value::string("TsOpen")),
            ("parameterStatusId", Value::string("TsDone")),
            ("lookedUpStatusName", old_name),
            ("parameterStatusName", Value::string("Done")),
        ]
    };
    assert_eq!(
        super::status_transition_message(&values(Value::string("Open"))),
        "Status change not allowed from Open to Done"
    );
    assert_eq!(
        super::status_transition_message(&values(Value::Null)),
        "Status change not allowed from TsOpen to Done"
    );
}

fn write_fixture(name: &str, body: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("moqui-svc-{name}-{}.xml", std::process::id()));
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn checked_calls_preserve_conflicts_while_legacy_calls_record_them() {
    let path = write_fixture(
        "checked-conflict",
        r#"<services><service verb="guard" noun="Revision"><actions><script>
            const error = new Error('Reload before editing'); error.moquiErrorKind = 'Conflict'; throw error
        </script></actions></service></services>"#,
    );
    let facade = crate::testing::open();
    let ctx = ExecutionContextFactory::test_context();
    ctx.factory().set_entity(facade);
    let _entered = ctx.enter();
    let _authz = ctx.disable_authz();
    crate::service::lint(std::slice::from_ref(&path)).unwrap();
    assert!(sync("guard#Revision", &BTreeMap::new()).unwrap().is_empty());
    ctx.message().clear_errors();
    let error = super::sync_checked(
        "guard#Revision",
        &BTreeMap::new(),
        &super::CallOptions {
            throw_on_error: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind(), crate::context::ErrorKind::Conflict);
}
