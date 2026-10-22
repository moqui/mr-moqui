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

use moqui_lib::object::Value;
use serde_json::json;

use super::state::{session_path, week_start, DrawerState, FollowStamp};
use super::{DrawerError, Params};
use crate::llm::prompt::build_tail;

/// The page state reads leniently: `/qapps` is `/apps`, secret-named values never come in, a value of the wrong type
/// is left out, and the dirty values are the ones the session takes.
#[test]
fn the_page_state_reads_leniently_and_drops_secrets() {
    let state = DrawerState::of(
        &json!({
            "path": "/qapps/system/Security/UserAccount/UserAccountList",
            "parameters": {"username": "john", "page": 2, "apiKey": "sk-123", "nested": {"no": 1}},
            "title": "User Accounts",
            "forms": [
                {"domId": "UserAccountList", "values": {"username": "john", "newPassword": "hunter2", "disabled": "N"},
                    "dirty": ["username", "newPassword"]},
                {"values": {"x": "1"}},
            ],
            "dialogs": ["CreateUserDialog"],
            "record": {"entityName": "moqui.security.UserAccount", "id": "EX_JOHN_DOE", "label": "john.doe"},
            "selections": [{"form": "UserAccountList", "ids": ["EX_JOHN_DOE", "EX_JANE"], "total": 5}],
            "filters": [{"form": "UserAccountList", "values": {"username": "john"}}],
            "formInfo": [{"name": "UserAccountList", "kind": "list", "fields": ["Username", "Email"], "rows": 25}],
        }),
        &["internalNote".to_string()],
    );
    assert_eq!(state.path, "/apps/system/Security/UserAccount/UserAccountList");
    assert_eq!(
        state.parameters,
        BTreeMap::from([
            ("page".to_string(), "2".to_string()),
            ("username".to_string(), "john".to_string())
        ])
    );
    assert_eq!(state.forms.len(), 1, "a form with no domId is left out");
    assert!(!state.forms[0].values.contains_key("newPassword"));
    assert_eq!(state.forms[0].dirty, ["username"]);
    let browser = state.browser_state().unwrap();
    assert_eq!(browser.dialogs, ["CreateUserDialog"]);
    let kept = state.screen_state().unwrap();
    assert_eq!(
        kept.values["UserAccountList"],
        BTreeMap::from([("username".to_string(), "john".to_string())])
    );

    let tail = build_tail(&state.tail_spec(None, "Monday"));
    assert!(
        tail.contains("page /apps/system/Security/UserAccount/UserAccountList?page=2&username=john \"User Accounts\""),
        "{tail}"
    );
    assert!(
        tail.contains("record moqui.security.UserAccount EX_JOHN_DOE \"john.doe\""),
        "{tail}"
    );
    assert!(
        tail.contains("selection UserAccountList: EX_JOHN_DOE, EX_JANE (+3 more)"),
        "{tail}"
    );
    assert!(tail.contains("filters UserAccountList: username=john"), "{tail}");
    assert!(
        tail.contains("UserAccountList(list: Username, Email; 25 rows)"),
        "{tail}"
    );
    assert!(!tail.contains("hunter2") && !tail.contains("sk-123"), "{tail}");

    // not an object, or a path that is not a plain one from the root, is no page
    assert_eq!(DrawerState::of(&json!("x"), &[]), DrawerState::default());
    assert!(DrawerState::of(&json!({"path": "https://evil.example/apps"}), &[])
        .browser_state()
        .is_none());
    assert_eq!(session_path("/qapps"), Some("/apps".to_string()));
    assert_eq!(session_path("/qappsx/y"), Some("/qappsx/y".to_string()));
    assert_eq!(session_path("/apps/../x"), None);
    assert_eq!(
        DrawerState::of(
            &json!({"forms": [{"domId": "F", "values": {"internalNote": "x"}}]}),
            &["internalNote".to_string()]
        )
        .forms[0]
            .values
            .len(),
        0
    );
}

#[test]
fn parameters_read_from_a_json_body_or_a_form() {
    let body = Params::of_json(&json!({"text": " hi ", "state": {"path": "/apps"}, "contextVersion": 3,
        "release": true, "clientId": "tab-1"}));
    assert_eq!(body.text("text").as_deref(), Some("hi"));
    assert_eq!(body.json("state"), Some(json!({"path": "/apps"})));
    assert_eq!(body.text("state"), None, "an object is not text");
    assert_eq!(body.int("contextVersion"), Some(3));
    assert_eq!(body.flag("release"), Some(true));
    assert_eq!(body.client_id("clientId").unwrap(), "tab-1");
    let form = Params::new(BTreeMap::from([
        ("state".to_string(), Value::string(r#"{"path":"/apps/x"}"#)),
        ("contextVersion".to_string(), Value::string("7")),
        ("clientId".to_string(), Value::string("bad id!")),
        ("release".to_string(), Value::string("false")),
    ]));
    assert_eq!(form.json("state"), Some(json!({"path": "/apps/x"})));
    assert_eq!(form.int("contextVersion"), Some(7));
    assert_eq!(form.flag("release"), Some(false));
    assert_eq!(form.client_id("clientId").unwrap_err().status, 400);
    assert_eq!(form.required("text").unwrap_err().code, "invalid");
}

#[test]
fn errors_carry_their_status_and_code() {
    let tarpit: moqui_lib::error::Error = crate::context::Error::of_kind(crate::context::ErrorKind::Tarpit, "too many")
        .with_retry_after(30)
        .into();
    let error = DrawerError::of(&tarpit);
    assert_eq!((error.status, error.code, error.retry_after), (429, "tarpit", Some(30)));
    assert_eq!(
        DrawerError::not_found("run", "R1").to_json(),
        json!({"errorCode": 404, "errors": "there is no run R1", "code": "not_found"})
    );
    assert_eq!(DrawerError::not_available("attachments", "13.14").status, 501);
}

#[test]
fn a_follow_stamp_marks_page_events_and_weeks_start_by_region() {
    let stamp = FollowStamp {
        client_id: Some("tab-1".to_string()),
        context_version: 4,
    };
    let mut payload = json!({"path": "/apps/x"});
    stamp.apply(&mut payload);
    assert_eq!(
        payload,
        json!({"path": "/apps/x", "clientId": "tab-1", "contextVersion": 4})
    );
    assert_eq!(week_start("en_US"), "Sunday");
    assert_eq!(week_start("de-DE"), "Monday");
    assert_eq!(week_start(""), "Monday");
}

/// Every call named in the contract is answered by the dispatcher (none falls through to "no drawer call").
#[test]
fn every_call_of_the_contract_is_dispatched() {
    let source = include_str!("mod.rs");
    for call in super::CALLS {
        assert!(source.contains(&format!("\"{call}\"")), "{call}");
    }
    let screen = include_str!("../../../../../runtime/base-component/webroot/screen/webroot/assist.xml");
    for call in super::CALLS {
        assert!(
            screen.contains(&format!("<transition name=\"{call}\"")),
            "assist.xml has no transition {call}"
        );
    }
}
