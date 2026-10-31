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

use super::*;

fn map(pairs: &[(&str, Value)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect(),
    )
}

/// Without the sim overlay nothing runs: not available, and no steps.
#[test]
fn without_a_sim_overlay_nothing_runs() {
    let context = ExecutionContextFactory::test_context_with_conf("<moqui-conf/>");
    let facade = moqui_core::testing::open();
    context.factory().set_entity(facade.clone());
    assert!(!facade.sim_available());
    let steps = [SimCall {
        service_name: "create#moqui.test.TestEntity".to_string(),
        parameters: BTreeMap::from([("testId".to_string(), Value::string("NO_SIM"))]),
    }];
    let ran = run(
        context.factory(),
        &StepsRequest {
            username: "nobody",
            label: "a test",
            steps: &steps,
            read: None,
            read_after_each_step: true,
        },
    );
    assert!(!ran.available);
    assert!(ran.steps.is_empty() && ran.read.is_none());
    assert!(facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("NO_SIM"))
        .one()
        .unwrap()
        .is_none());
}

/// A step is a `{serviceName, parameters}` map, its parameters a map or JSON text; anything else is a step with no
/// service, which fails alone.
#[test]
fn a_step_reads_from_a_map_or_json_text() {
    let call = sim_call(&map(&[
        ("serviceName", Value::string(" a.B.make#Thing ")),
        ("parameters", map(&[("thingId", Value::string("T1"))])),
    ]))
    .unwrap();
    assert_eq!(call.service_name, "a.B.make#Thing");
    assert_eq!(call.parameters.get("thingId"), Some(&Value::string("T1")));
    let call = sim_call(&map(&[
        ("serviceName", Value::string("a.B.make#Thing")),
        ("parameters", Value::string(r#"{"count": 2}"#)),
    ]))
    .unwrap();
    assert_eq!(call.parameters.get("count"), Some(&Value::I64(2)));
    assert!(sim_call(&Value::string("a.B.make#Thing")).is_none());
    assert!(fenced_call(&SimCall::default(), "a test")
        .unwrap_err()
        .contains("serviceName"));

    let steps = list_of(Some(&Value::string(r#"[{"serviceName": "a.B.make#Thing"}, 3]"#)));
    assert_eq!(steps.len(), 2);
    assert_eq!(sim_call(&steps[0]).unwrap().service_name, "a.B.make#Thing");
    assert!(list_of(Some(&Value::string("not json"))).is_empty());
    assert!(map_of(Some(&Value::string("[1]"))).is_none());
}

/// The out shape of a step: `error` only when it failed, `readResult` only with readAfterEachStep.
#[test]
fn a_step_answers_in_the_contract_shape() {
    let failed = StepOutcome {
        ok: false,
        error: Some("no".to_string()),
        rows: Vec::new(),
        more: false,
        created_keys: Vec::new(),
        read: Some(Err("the read failed".to_string())),
    };
    let Value::Map(shown) = step_value(&failed, true) else {
        panic!("a step is a map");
    };
    assert_eq!(shown.get("ok"), Some(&Value::Bool(false)));
    assert_eq!(shown.get("error"), Some(&Value::string("no")));
    assert_eq!(shown.get("rows"), Some(&Value::List(Vec::new())));
    assert_eq!(shown.get("createdKeys"), Some(&Value::List(Vec::new())));
    assert_eq!(shown.get("readResult"), Some(&Value::Map(Default::default())));
    assert_eq!(shown.get("readError"), Some(&Value::string("the read failed")));

    let kept = StepOutcome {
        ok: true,
        error: None,
        rows: vec![json!({"entity": "a.Thing", "key": {"thingId": "-1"}, "action": "create", "fields": []})],
        more: false,
        created_keys: vec![json!({"entity": "a.Thing", "key": {"thingId": "-1"}})],
        read: None,
    };
    let Value::Map(shown) = step_value(&kept, false) else {
        panic!("a step is a map");
    };
    assert!(shown.get("error").is_none() && shown.get("readResult").is_none() && shown.get("moreRows").is_none());
    let json = moqui_lib::json::to_json(&Value::Map(shown));
    assert_eq!(json["createdKeys"][0]["key"]["thingId"], "-1");
    assert_eq!(json["rows"][0]["action"], "create");
}

/// A read's out-parameters keep no host object other than entity values and lists, which become plain values.
#[test]
fn a_read_result_keeps_plain_values_only() {
    let object = Value::Object(moqui_lib::object::HostObject::new(1_u8));
    let shown = plain(Value::List(vec![
        map(&[("find", object.clone()), ("n", Value::I64(1))]),
        object,
    ]));
    assert_eq!(
        shown,
        Value::List(vec![map(&[("find", Value::Null), ("n", Value::I64(1))]), Value::Null])
    );
}
