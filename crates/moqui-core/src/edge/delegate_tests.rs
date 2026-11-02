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
        timeout: None,
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
        timeout: None,
    };
    let parameters = received_parameters(&call);
    let names: Vec<&str> = parameters.keys().map(String::as_str).collect();
    assert_eq!(names, ["author", CALLER_PARAMETER, "message"]);
    let Some(Value::Map(caller)) = parameters.get(CALLER_PARAMETER) else {
        panic!("the caller: {parameters:?}")
    };
    assert_eq!(caller.get("username"), Some(&Value::string("visitor")));
}

#[test]
fn an_edge_node_sends_a_delegated_call_to_the_link_before_any_link_is_started() {
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><edge><delegate service="a.B.call#Remote" allow-anonymous="true"/></edge></moqui-conf>"#,
    );
    let _entered = ctx.enter();
    let parameters = BTreeMap::from([("message".to_string(), Value::string("hi"))]);
    // as boot reads it on an edge node (conf::init), before the host's listeners start
    assert!(super::super::conf::settings_as(ctx.factory(), true).edge_node);
    let err = crate::service::sync("a.B.call#Remote", &parameters).unwrap_err();
    // no link, or a link with no local node: offline either way, never run here (where it is not even defined)
    assert!(err.message().starts_with(OFFLINE), "{}", err.message());
}

/// E8: a call the edge node stopped waiting for is not run here (a ringing session or a dial for a caller gone).
#[test]
fn a_delegated_call_past_its_deadline_is_not_run() {
    let ctx = ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><edge><delegate service="a.B.call#Late" timeout-seconds="0.1"/></edge></moqui-conf>"#,
    );
    let settings = super::super::conf::settings_as(ctx.factory(), false);
    assert_eq!(
        settings.delegate("a.B.call#Late").and_then(|d| d.timeout),
        Some(std::time::Duration::from_millis(100))
    );
    let call = DelegateCall {
        service: "a.B.call#Late".to_string(),
        parameters: serde_json::json!({"voiceTrunkId": "T1", "status": "ok"}),
        caller: serde_json::json!({}),
        operation_id: "op-late".to_string(),
        timeout: Some(std::time::Duration::from_millis(100)),
    };
    let answer = run(ctx.factory(), &call);
    assert_eq!(answer.result, serde_json::Value::Null);
    assert!(
        answer
            .errors
            .iter()
            .any(|error| error.starts_with(OFFLINE) && error.contains("stopped waiting")),
        "{:?}",
        answer.errors
    );
}
