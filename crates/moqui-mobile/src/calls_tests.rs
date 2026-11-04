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

use std::cell::RefCell;

use moqui_client::account::Account;
use serde_json::{json, Value};

use super::*;

/// What a server answers a post.
type Answered = Result<(u16, Value), String>;

/// Answers each account's server from a table, and notes what was posted.
struct Fake {
    answers: Vec<(&'static str, Answered)>,
    posted: RefCell<Vec<(String, String, String)>>,
}

impl Poster for Fake {
    fn post(&self, account: &Account, key: &str, path: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String> {
        let form: Vec<String> = form.iter().map(|(name, value)| format!("{name}={value}")).collect();
        self.posted
            .borrow_mut()
            .push((account.id.clone(), format!("{key} {path}"), form.join("&")));
        self.answers
            .iter()
            .find(|(id, _)| *id == account.id)
            .map_or(Err("no answer".to_string()), |(_, answer)| answer.clone())
    }
}

fn fake(answers: Vec<(&'static str, Answered)>) -> Fake {
    Fake {
        answers,
        posted: RefCell::new(Vec::new()),
    }
}

fn asked(id: &str) -> Asked {
    Asked {
        account: Account {
            id: id.to_string(),
            server: format!("https://{id}.example.com"),
            username: "joe".to_string(),
            user_id: String::new(),
            display: String::new(),
            server_name: String::new(),
            device_mode: None,
        },
        key: format!("key-{id}"),
        comms_root: "/qapps/BasicComms".to_string(),
    }
}

#[test]
fn an_answer_link_names_a_plain_call_id() {
    let link = |text: &str| answer_link(&tauri::Url::parse(text).unwrap());
    assert_eq!(
        link("org.moqui.mobile:/call?callSessionId=100231&video=false"),
        Some(Answer {
            call_session_id: "100231".to_string(),
            video: false,
            hang_up: false
        })
    );
    assert_eq!(
        link("org.moqui.mobile:/call?callSessionId=5&hangUp=true").map(|a| a.hang_up),
        Some(true)
    );
    assert_eq!(
        link("org.moqui.mobile:/call?callSessionId=A-1_b").map(|a| a.video),
        Some(true)
    );
    // another path, another scheme, no id, an id that could be more than an id
    assert_eq!(link("org.moqui.mobile:/sso?callSessionId=1"), None);
    assert_eq!(link("https://example.com/call?callSessionId=1"), None);
    assert_eq!(link("org.moqui.mobile:/call"), None);
    assert_eq!(link("org.moqui.mobile:/call?callSessionId=1%26x%3D2"), None);
    assert_eq!(
        link(&format!("org.moqui.mobile:/call?callSessionId={}", "9".repeat(65))),
        None
    );
}

#[test]
fn the_call_screen_and_transitions_are_under_the_comms_app() {
    let answer = Answer {
        call_session_id: "7".to_string(),
        video: false,
        hang_up: false,
    };
    assert_eq!(
        hang_up_script("7"),
        "window.moqui && moqui.call && moqui.call.state.current === \"7\" && moqui.call.leave()"
    );
    assert_eq!(
        call_path("/qapps/BasicComms/", &answer),
        "/qapps/BasicComms/Calls/Call?callSessionId=7&answer=true&video=false"
    );
    assert_eq!(
        transition_path("/qapps/BasicComms", "declineCall"),
        "/apps/BasicComms/Calls/declineCall"
    );
    assert_eq!(
        transition_path("/qapps/boss/Comms", "declineCall"),
        "/apps/boss/Comms/Calls/declineCall"
    );
}

#[test]
fn a_decline_asks_each_account_until_one_knows_the_call() {
    let poster = fake(vec![
        (
            "a",
            Ok((200, json!({"reasonCode": "notFound", "errors": "Call not found"}))),
        ),
        ("b", Err("connection refused".to_string())),
        ("c", Ok((200, json!({"reasonCode": null, "call": {"status": "ended"}})))),
        ("d", Ok((200, json!({})))),
    ]);
    let accounts = [asked("a"), asked("b"), asked("c"), asked("d")];
    assert_eq!(decline(&accounts, "77", &poster), Declined::Told("c".to_string()));
    let posted = poster.posted.borrow();
    assert_eq!(posted.len(), 3, "d is not asked once c knew the call");
    assert_eq!(
        posted[2],
        (
            "c".to_string(),
            "key-c /apps/BasicComms/Calls/declineCall".to_string(),
            "callSessionId=77".to_string()
        )
    );
}

#[test]
fn a_call_no_server_knows_or_none_can_be_told() {
    let none = fake(vec![("a", Ok((200, json!({"reasonCode": "notFound"}))))]);
    assert_eq!(decline(&[asked("a")], "1", &none), Declined::NotFound);
    let down = fake(vec![("a", Ok((502, Value::Null))), ("b", Err("timeout".to_string()))]);
    match decline(&[asked("a"), asked("b")], "1", &down) {
        Declined::Failed(why) => assert_eq!(why, vec!["a: HTTP 502".to_string(), "b: timeout".to_string()]),
        other => panic!("not failed: {other:?}"),
    }
    // a call already declined or answered elsewhere: the server knows it, which is enough
    let over = fake(vec![("a", Ok((200, json!({"reasonCode": "notRinging"}))))]);
    assert_eq!(decline(&[asked("a")], "1", &over), Declined::Told("a".to_string()));
}

#[test]
fn a_token_is_registered_once_with_each_account() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut record = PushRecord::load(&dir);
    assert_eq!(record, PushRecord::default());
    let poster = fake(vec![
        ("a", Ok((200, json!({"registered": true, "subscriptionId": "1"})))),
        ("b", Ok((400, json!({"errors": "not a device key"})))),
    ]);
    // no token yet: nothing is sent
    assert!(!record.register(&[asked("a")], &poster));
    assert!(poster.posted.borrow().is_empty());
    assert!(record.set_token("tok-1"));
    assert!(!record.set_token("tok-1"));
    assert!(record.register(&[asked("a"), asked("b")], &poster));
    assert_eq!(record.registered, vec!["a".to_string()]);
    assert_eq!(
        poster.posted.borrow()[0].2,
        "provider=fcm&token=tok-1&appId=org.moqui.mobile&platform=android"
    );
    record.save(&dir).unwrap();
    assert_eq!(PushRecord::load(&dir), record);
    let _ = std::fs::remove_dir_all(&dir);
    // a again is not sent; b, refused, is tried again
    poster.posted.borrow_mut().clear();
    assert!(!record.register(&[asked("a"), asked("b")], &poster));
    assert_eq!(poster.posted.borrow().len(), 1);
    // a new token is registered with every account again
    assert!(record.set_token("tok-2"));
    assert!(record.registered.is_empty());
}

#[test]
fn a_sign_out_takes_the_token_back() {
    let poster = fake(vec![("a", Ok((200, json!({"deletedCount": 1}))))]);
    let mut record = PushRecord {
        token: Some("tok".to_string()),
        registered: vec!["a".to_string(), "b".to_string()],
    };
    assert!(record.unregister(&asked("a"), &poster));
    assert_eq!(record.registered, vec!["b".to_string()]);
    assert_eq!(poster.posted.borrow()[0].1, "key-a /qapps/unregisterClientPush");
    // a server that cannot be reached: forgotten here all the same (the key's revoke deletes it there)
    assert!(record.unregister(&asked("b"), &poster));
    assert!(record.registered.is_empty());
    // a new sign-in of an account registers it again
    record.registered.push("c".to_string());
    assert!(record.forget("c"));
    assert!(!record.forget("c"));
}
