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

#[test]
fn credentials_and_a_named_caller_never_cross_the_link() {
    let parameters = BTreeMap::from([
        ("message".to_string(), Value::string("hi")),
        ("count".to_string(), Value::I64(3)),
        ("authUsername".to_string(), Value::string("john.doe")),
        ("authPassword".to_string(), Value::string("secret")),
        ("authUserAccount".to_string(), Value::string("account")),
        ("authSomethingNew".to_string(), Value::string("x")),
        ("author".to_string(), Value::string("Ann")),
        (CALLER_PARAMETER.to_string(), Value::string("forged")),
        ("_internal".to_string(), Value::string("x")),
    ]);
    let json = parameters_json(&parameters);
    assert_eq!(json, serde_json::json!({"message": "hi", "count": 3, "author": "Ann"}));
    let back = result_map(&json);
    assert_eq!(back.get("message"), Some(&Value::string("hi")));
    assert!(result_map(&serde_json::json!([1, 2])).is_empty());
}

#[test]
fn a_local_node_refuses_a_service_its_own_conf_does_not_list() {
    let ctx = ExecutionContextFactory::test_context();
    let call = DelegateCall {
        service: "org.moqui.impl.EdgeServices.echo#Delegate".to_string(),
        parameters: serde_json::json!({"message": "hi"}),
        caller: serde_json::json!({"username": "visitor"}),
        operation_id: "op-1".to_string(),
    };
    let answer = run(ctx.factory(), &call);
    assert_eq!(answer.result, serde_json::Value::Null);
    assert_eq!(
        answer.errors,
        ["org.moqui.impl.EdgeServices.echo#Delegate is not a delegated service on this node"]
    );
}

#[test]
fn nothing_the_edge_node_sends_changes_the_user_a_delegated_call_runs_as() {
    let call = DelegateCall {
        service: "a.B.call#One".to_string(),
        parameters: serde_json::json!({
            "message": "hi",
            "authUsername": "john.doe",
            "authPassword": "moqui",
            "authUserAccount": {"username": "john.doe", "currentPassword": "moqui"},
            "authTenantId": "DEFAULT",
            "auth": "x",
            "edgeCaller": {"username": "forged"},
            "_operationRetry": "true",
            "author": "Ann"
        }),
        caller: serde_json::json!({"username": "visitor", "userId": null}),
        operation_id: "op-2".to_string(),
    };
    let parameters = received_parameters(&call);
    let names: Vec<&str> = parameters.keys().map(String::as_str).collect();
    assert_eq!(names, ["author", CALLER_PARAMETER, "message"]);
    let Some(Value::Map(caller)) = parameters.get(CALLER_PARAMETER) else {
        panic!("the caller: {parameters:?}")
    };
    assert_eq!(caller.get("username"), Some(&Value::string("visitor")));
}
