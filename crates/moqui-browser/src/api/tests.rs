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

use std::path::PathBuf;

use serde_json::{json, Value as Json};

use super::{guarded, handle, handle_op};

fn call(request: &Json) -> Json {
    serde_json::from_str(&handle(&request.to_string())).unwrap()
}

fn ok(response: Json) -> Json {
    assert_eq!(response["ok"], json!(true), "{response}");
    response["result"].clone()
}

#[test]
fn json_requests_boot_load_call_render_and_find() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime = std::env::temp_dir().join(format!("moqui-browser-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runtime);
    std::fs::create_dir_all(runtime.join("conf")).unwrap();
    std::fs::create_dir_all(runtime.join("screen")).unwrap();
    for file in ["conf/MoquiDevConf.xml", "screen/Hello.xml"] {
        std::fs::copy(crate_dir.join("runtime").join(file), runtime.join(file)).unwrap();
    }
    let framework = crate_dir.join("../../runtime/components/base/framework");

    let before = call(&json!({ "op": "find", "entity": "moqui.basic.Enumeration" }));
    assert_eq!(before["ok"], json!(false));
    assert!(call(&json!([1])).to_string().contains("not a JSON object"));
    ok(call(&json!({
        "op": "init",
        "runtime": runtime.display().to_string(),
        "framework": framework.display().to_string(),
    })));
    ok(call(&json!({ "op": "checkTables" })));
    let records = ok(call(&json!({
        "op": "loadData",
        "location": framework.join("data/SecurityTypeData.xml").display().to_string(),
        "types": ["seed"],
    })));
    assert!(records.as_u64().unwrap() > 0, "{records}");

    let out = ok(serde_json::from_str(&handle_op(
        "callService",
        &json!({ "name": "org.moqui.impl.BasicServices.find#Enumeration", "parameters": { "enumTypeId": "UserGroupType" } })
            .to_string(),
    ))
    .unwrap());
    let list = out["enumerationList"].as_array().unwrap_or_else(|| panic!("{out}"));
    assert!(!list.is_empty(), "{out}");

    let echo = ok(call(&json!({
        "op": "callService",
        "name": "org.moqui.impl.BasicServices.echo#Data",
        "parameters": { "textIn1": "hi", "numberIn": 1.5 },
    })));
    assert_eq!(echo["textOut1"], json!("hi"), "{echo}");

    ok(call(
        &json!({ "op": "store", "entity": "moqui.test.TestEntity", "fields": { "testId": "J1", "testMedium": "json" } }),
    ));
    let found = ok(call(
        &json!({ "op": "find", "entity": "moqui.test.TestEntity", "equals": { "testId": "J1" } }),
    ));
    assert_eq!(found[0]["testMedium"], json!("json"), "{found}");

    let rendered = ok(call(&json!({
        "op": "render",
        "root": runtime.join("screen/Hello.xml").display().to_string(),
        "path": "",
        "parameters": { "name": "json" },
    })));
    assert!(rendered["body"].as_str().unwrap().contains("Hello json"), "{rendered}");
    assert!(!call(&json!({ "op": "nope" }))["error"].as_str().unwrap().is_empty());

    // An inbound /notws frame is one request; the notification comes back for the UI.
    let note = ok(call(&json!({
        "op": "wsDeliver",
        "conn": "notws",
        "text": json!({ "topic": "ApiWs", "notificationMessageId": format!("api-ws-{}", std::process::id()), "title": "t" })
            .to_string(),
    })));
    assert_eq!(note["kind"], json!("notification"), "{note}");
    assert_eq!(note["body"]["topic"], json!("ApiWs"), "{note}");
    let other = ok(call(&json!({ "op": "wsDeliver", "conn": "notws", "text": "pong" })));
    assert_eq!(other, json!({ "kind": "text", "text": "pong" }));
    ok(call(
        &json!({ "op": "wsSend", "conn": "notws", "text": "subscribe:ALL" }),
    ));
    assert_eq!(
        crate::ws::take_outbox(),
        vec![crate::ws::Outbound::Text {
            conn: "notws".to_string(),
            text: "subscribe:ALL".to_string(),
        }]
    );

    // A panic is an error response, and the state it may have broken is gone.
    let panicked: Json = serde_json::from_str(&guarded(|| panic!("boom"))).unwrap();
    assert_eq!(panicked["ok"], json!(false));
    assert!(
        panicked["error"].as_str().unwrap().starts_with("panic: boom"),
        "{panicked}"
    );
    let after = call(&json!({ "op": "find", "entity": "moqui.test.TestEntity" }));
    assert!(after["error"].as_str().unwrap().contains("init has not run"), "{after}");
    let _ = std::fs::remove_dir_all(&runtime);
}

#[test]
fn a_panic_is_an_error_response() {
    let text: Json = serde_json::from_str(&guarded(|| panic!("{}", String::from("formatted")))).unwrap();
    assert_eq!(text["ok"], json!(false));
    assert!(
        text["error"].as_str().unwrap().starts_with("panic: formatted"),
        "{text}"
    );
    assert_eq!(guarded(|| "fine".to_string()), "fine");
}
