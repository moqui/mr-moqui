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

fn asked_on(id: &str, server: &str) -> Asked {
    Asked {
        account: Account {
            id: id.to_string(),
            server: server.to_string(),
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

fn asked(id: &str) -> Asked {
    asked_on(id, &format!("https://{id}.example.com"))
}

fn link(id: &str) -> CallLink {
    CallLink {
        call_session_id: id.to_string(),
        account: "a".to_string(),
        video: false,
        hang_up: false,
    }
}

#[test]
fn a_call_link_is_taken_only_with_the_nonce_made_for_that_call_once_and_soon() {
    let mut links = CallLinks::default();
    let nonce = links.make(link("100231"), 1_000).unwrap();
    // another call's id, an unknown nonce: refused
    assert_eq!(links.take("h00", "100231", 1_000), None);
    let other = links.make(link("7"), 1_000).unwrap();
    assert_eq!(links.take(&other, "100231", 1_000), None, "a nonce for another call");
    assert_eq!(links.take(&other, "7", 1_000), None, "a refused nonce is gone too");
    assert_eq!(links.take(&nonce, "100231", 2_000), Some(link("100231")));
    assert_eq!(links.take(&nonce, "100231", 2_000), None, "taken once");
    let late = links.make(link("8"), 1_000).unwrap();
    assert_eq!(links.take(&late, "8", 1_000 + LINK_MS), None, "too old");
    // the nonces are not guessable: different each time
    let a = links.make(link("9"), 1).unwrap();
    let b = links.make(link("9"), 1).unwrap();
    assert!(a != b && a.len() >= 32);
}

#[test]
fn a_call_link_names_a_plain_call_id_and_a_nonce() {
    let parse = |text: &str| link_of(&tauri::Url::parse(text).unwrap());
    assert_eq!(
        parse("org.moqui.mobile:/call?callSessionId=100231&nonce=habc"),
        Some(("100231".to_string(), "habc".to_string()))
    );
    // another app's link: no nonce, an answer flag, a bad id, another path or scheme
    assert_eq!(parse("org.moqui.mobile:/call?callSessionId=100231&answer=true"), None);
    assert_eq!(parse("org.moqui.mobile:/call?callSessionId=1%26x%3D2&nonce=h1"), None);
    assert_eq!(parse("org.moqui.mobile:/sso?callSessionId=1&nonce=h1"), None);
    assert_eq!(parse("https://example.com/call?callSessionId=1&nonce=h1"), None);
    assert!(is_call_link(&tauri::Url::parse("org.moqui.mobile:/call?x=1").unwrap()));
    assert!(!is_call_link(&tauri::Url::parse("org.moqui.mobile:/sso").unwrap()));
}

#[test]
fn the_call_screen_and_transitions_are_under_the_comms_app() {
    assert_eq!(
        call_path("/qapps/BasicComms/", &link("7")),
        "/qapps/BasicComms/Calls/Call?callSessionId=7&answer=true&video=false"
    );
    assert_eq!(
        hang_up_script("7"),
        "window.moqui && moqui.call && moqui.call.state.current === \"7\" && moqui.call.leave()"
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
fn a_decline_is_told_only_when_the_server_declined_it() {
    let declined = fake(vec![(
        "a",
        Ok((200, json!({"reasonCode": null, "call": {"status": "ended"}}))),
    )]);
    assert_eq!(decline(&asked("a"), "77", &declined), Declined::Told);
    assert_eq!(
        declined.posted.borrow()[0],
        (
            "a".to_string(),
            "key-a /apps/BasicComms/Calls/declineCall".to_string(),
            "callSessionId=77".to_string()
        )
    );
    let not_found = fake(vec![("a", Ok((200, json!({"reasonCode": "notFound"}))))]);
    assert_eq!(
        decline(&asked("a"), "77", &not_found),
        Declined::Refused("notFound".to_string())
    );
    let down = fake(vec![("a", Ok((502, Value::Null)))]);
    assert_eq!(
        decline(&asked("a"), "77", &down),
        Declined::Failed("HTTP 502".to_string())
    );
    let gone = fake(vec![]);
    assert_eq!(
        decline(&asked("a"), "77", &gone),
        Declined::Failed("no answer".to_string())
    );
}

#[test]
fn a_token_is_registered_once_with_each_account_and_its_id_kept() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut record = PushRecord::load(&dir);
    assert_eq!(record, PushRecord::default());
    let poster = fake(vec![
        ("a", Ok((200, json!({"registered": true, "subscriptionId": "100050"})))),
        ("b", Ok((400, json!({"errors": "not a device key"})))),
        ("c", Ok((200, json!({"registered": true})))),
    ]);
    // no token yet: nothing is sent
    assert!(!record.register(&[asked("a")], &poster));
    assert!(poster.posted.borrow().is_empty());
    assert!(record.set_token("tok-1"));
    assert!(!record.set_token("tok-1"));
    assert!(record.register(&[asked("a"), asked("b"), asked("c")], &poster));
    // b refused, c gave no id: neither is kept
    assert_eq!(record.registered.get("a").map(String::as_str), Some("100050"));
    assert_eq!(record.registered.len(), 1);
    assert_eq!(
        poster.posted.borrow()[0].2,
        "provider=fcm&token=tok-1&appId=org.moqui.mobile&platform=android"
    );
    record.save(&dir).unwrap();
    assert_eq!(PushRecord::load(&dir), record);
    let _ = std::fs::remove_dir_all(&dir);
    poster.posted.borrow_mut().clear();
    assert!(!record.register(&[asked("a"), asked("b")], &poster));
    assert_eq!(poster.posted.borrow().len(), 1, "a is not sent again");
    assert!(record.set_token("tok-2"));
    assert!(record.registered.is_empty());
}

#[test]
fn a_token_another_account_of_the_server_takes_is_no_longer_the_first_ones() {
    let poster = fake(vec![(
        "b",
        Ok((200, json!({"registered": true, "subscriptionId": "100050"}))),
    )]);
    let mut record = PushRecord {
        token: Some("tok".to_string()),
        registered: [("a".to_string(), "100050".to_string())].into(),
    };
    let server = "https://one.example.com";
    assert!(record.register(&[asked_on("a", server), asked_on("b", server)], &poster));
    assert_eq!(record.registered.keys().collect::<Vec<_>>(), vec!["b"]);
}

#[test]
fn a_ring_is_the_account_it_was_registered_with_or_none() {
    let record = PushRecord {
        token: Some("tok".to_string()),
        registered: [
            ("a".to_string(), "100050".to_string()),
            ("b".to_string(), "100051".to_string()),
            ("c".to_string(), "100050".to_string()),
        ]
        .into(),
    };
    let keyed = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    assert_eq!(ring_account(&record, "100051", &keyed, None).as_deref(), Some("b"));
    // two servers gave the same id: the current account first
    assert_eq!(ring_account(&record, "100050", &keyed, Some("c")).as_deref(), Some("c"));
    // no account's, an empty id, an account signed out here: none
    assert_eq!(ring_account(&record, "999", &keyed, None), None);
    assert_eq!(ring_account(&record, "", &keyed, None), None);
    assert_eq!(ring_account(&record, "100051", &["a".to_string()], None), None);
}

#[test]
fn a_sign_out_takes_the_token_back() {
    let poster = fake(vec![("a", Ok((200, json!({"deletedCount": 1}))))]);
    let mut record = PushRecord {
        token: Some("tok".to_string()),
        registered: [("a".to_string(), "1".to_string()), ("b".to_string(), "2".to_string())].into(),
    };
    assert!(record.unregister(&asked("a"), &poster));
    assert_eq!(record.registered.keys().collect::<Vec<_>>(), vec!["b"]);
    assert_eq!(poster.posted.borrow()[0].1, "key-a /qapps/unregisterClientPush");
    // a server that cannot be reached: forgotten here all the same (the key's revoke deletes it there)
    assert!(record.unregister(&asked("b"), &poster));
    assert!(record.registered.is_empty());
    assert!(!record.forget("c"));
}
