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
        "window.moqui && moqui.call && (moqui.call.state.current === \"7\" ? moqui.call.leave() : moqui.call.state.held && moqui.call.state.held.callSessionId === \"7\" && moqui.call.leaveHeld())"
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

/// Real account ids (`moqui_client::account::account_id`): a host with dots, a `-` and a hex digest.
const A: &str = "chat.example.com-0123456789ab";
const B: &str = "chat.example.org-ba9876543210";
const C: &str = "chat.example.net-00112233aabb";

#[test]
fn a_token_is_registered_once_with_each_account_and_its_id_kept() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-push-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut record = PushRecord::load(&dir);
    assert_eq!(record, PushRecord::default());
    let poster = fake(vec![
        (A, Ok((200, json!({"registered": true, "subscriptionId": "100050"})))),
        (B, Ok((400, json!({"errors": "not a device key"})))),
        (C, Ok((200, json!({"registered": true})))),
    ]);
    // no token yet: nothing is sent
    assert!(!record.register(&[asked(A)], &poster));
    assert!(poster.posted.borrow().is_empty());
    assert!(record.set_token("tok-1"));
    assert!(!record.set_token("tok-1"));
    assert!(record.set_keys(A, "PUB-A", "AUTH-A"));
    assert!(record.register(&[asked(A), asked(B), asked(C)], &poster));
    // b refused, c gave no id: neither is kept
    assert_eq!(record.registered.get(A).map(String::as_str), Some("100050"));
    assert_eq!(record.registered.len(), 1);
    assert_eq!(
        poster.posted.borrow()[0].2,
        "provider=fcm&token=tok-1&appId=org.moqui.mobile&platform=android&p256dhKey=PUB-A&authSecret=AUTH-A"
    );
    assert_eq!(
        poster.posted.borrow()[1].2,
        "provider=fcm&token=tok-1&appId=org.moqui.mobile&platform=android",
        "an account with no keys yet goes without"
    );
    record.save(&dir).unwrap();
    assert_eq!(PushRecord::load(&dir), record);
    let _ = std::fs::remove_dir_all(&dir);
    poster.posted.borrow_mut().clear();
    assert!(!record.register(&[asked(A), asked(B)], &poster));
    assert_eq!(poster.posted.borrow().len(), 1, "a is not sent again");
    assert!(record.set_token("tok-2"));
    assert!(record.registered.is_empty());
}

/// SEC-CLI-125: the distributor app knows each account by a random instance kept in `push.json`, never by the account
/// id (the server's host and a hash of the server and username); an instance is the same until the account signs out.
#[test]
fn each_account_has_a_random_unified_push_instance_kept_until_it_signs_out() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-instances-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut record = PushRecord::default();
    let (a, made) = record.instance_of(A).unwrap();
    assert!(made, "made at the first ask");
    assert_eq!(a.len(), 32);
    assert!(a.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "hex: {a}");
    assert_ne!(a, A);
    assert_eq!(
        record.instance_of(A).unwrap(),
        (a.clone(), false),
        "the same at the next ask"
    );
    let (b, _) = record.instance_of(B).unwrap();
    assert_ne!(a, b, "one per account");
    record.save(&dir).unwrap();
    let mut loaded = PushRecord::load(&dir);
    assert_eq!(loaded, record);
    assert_eq!(
        loaded.instance_of(A).unwrap(),
        (a.clone(), false),
        "kept across a save and a load"
    );
    assert_eq!(loaded.account_of(&a), Some(A));
    assert_eq!(loaded.account_of(&b), Some(B));
    // an earlier build's instance was the account id: no account's, so its late events are ignored
    assert_eq!(loaded.account_of(A), None);
    assert_eq!(loaded.account_of(&a.to_uppercase()), None);
    assert_eq!(loaded.account_of("0123456789abcdef0123456789abcdef"), None);
    assert_eq!(loaded.account_of(""), None);
    // a sign-out drops it (a record changed by that alone), and a sign-in again gets another
    assert!(loaded.drop_account(A));
    assert!(!loaded.drop_account(A));
    assert_eq!(loaded.account_of(&a), None);
    assert_eq!(loaded.instances.keys().collect::<Vec<_>>(), vec![B]);
    let (again, made) = loaded.instance_of(A).unwrap();
    assert!(made);
    assert_ne!(again, a);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `push.json` an earlier build kept, with no instances: it loads as it was, and its accounts get instances.
#[test]
fn a_push_record_with_no_instances_still_loads() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-push-shape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let earlier = json!({
        "token": "tok",
        "registered": {A: "100050"},
        "keys": {A: ["PUB-A", "AUTH-A"]},
        "endpoints": {A: "https://up.example.net/a"},
        "taken": [B],
    });
    std::fs::write(dir.join("push.json"), earlier.to_string()).unwrap();
    let mut record = PushRecord::load(&dir);
    assert_eq!(record.token.as_deref(), Some("tok"));
    assert_eq!(record.registered.get(A).map(String::as_str), Some("100050"));
    assert_eq!(
        record.endpoints.get(A).map(String::as_str),
        Some("https://up.example.net/a")
    );
    assert!(record.taken.contains(B));
    assert!(record.instances.is_empty());
    assert_eq!(
        record.account_of(A),
        None,
        "the earlier instance, the account id, is no account's"
    );
    let (instance, made) = record.instance_of(A).unwrap();
    assert!(made && instance != A);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_token_another_account_of_the_server_takes_is_no_longer_the_first_ones() {
    let poster = fake(vec![(
        B,
        Ok((200, json!({"registered": true, "subscriptionId": "100050"}))),
    )]);
    let mut record = PushRecord {
        token: Some("tok".to_string()),
        registered: [(A.to_string(), "100050".to_string())].into(),
        ..Default::default()
    };
    let server = "https://one.example.com";
    let both = [asked_on(A, server), asked_on(B, server)];
    assert!(record.register(&both, &poster));
    assert_eq!(record.registered.keys().collect::<Vec<_>>(), vec![B]);
    // review finding: A is not pending, so the retry worker does not take the token back for ever
    assert!(!record.pending(&both));
    let posted = poster.posted.borrow().len();
    assert!(!record.register(&both, &poster));
    assert_eq!(poster.posted.borrow().len(), posted, "A is not registered again");
    // ... and a merge of that outcome keeps it so
    let mut now = PushRecord {
        token: Some("tok".to_string()),
        registered: [(A.to_string(), "100050".to_string())].into(),
        ..Default::default()
    };
    let before = now.clone();
    assert!(now.merge(&before, &record));
    assert!(!now.pending(&both));
    assert_eq!(now.registered.keys().collect::<Vec<_>>(), vec![B]);
    assert!(now.taken.contains(A));
    // A signs in again, or a new token: A is tried again
    assert!(record.forget(A));
    assert!(record.pending(&both));
    record.taken.insert(A.to_string());
    assert!(record.set_token("tok-2"));
    assert!(record.taken.is_empty());
}

/// Review finding: an account that registered its push keys gets its rings sealed, so an unsealed ring in its name (or
/// its end) is refused, even when it is the only account the id can be.
#[test]
fn an_unsealed_ring_is_refused_for_an_account_with_keys() {
    let mut record = PushRecord {
        token: Some("tok".to_string()),
        registered: [(A.to_string(), "100050".to_string())].into(),
        ..Default::default()
    };
    let keyed = vec![A.to_string()];
    assert_eq!(
        ring_account(&record, "100050", &keyed).as_deref(),
        Some(A),
        "no keys: the one account"
    );
    record
        .keys
        .insert(A.to_string(), ("PUB".to_string(), "AUTH".to_string()));
    assert_eq!(ring_account(&record, "100050", &keyed), None);
}

/// HIGH review finding: two servers give the same registration id, so a push id names the accounts it may be for;
/// which one is proven by the keys that open it (`native::open_push`). An unsealed ring is taken only when one
/// account can be its, never the current one by default.
#[test]
fn a_push_id_names_the_accounts_it_may_be_and_an_unsealed_ring_only_one() {
    let record = PushRecord {
        token: Some("tok".to_string()),
        registered: [
            (A.to_string(), "100050".to_string()),
            (B.to_string(), "100051".to_string()),
            (C.to_string(), "100050".to_string()),
        ]
        .into(),
        ..Default::default()
    };
    let keyed = vec![A.to_string(), B.to_string(), C.to_string()];
    assert_eq!(
        push_accounts(&record, "100050", &keyed),
        vec![A.to_string(), C.to_string()]
    );
    assert_eq!(ring_account(&record, "100051", &keyed).as_deref(), Some(B));
    assert_eq!(ring_account(&record, "100050", &keyed), None, "two could be its: none");
    // no account's, an empty id, an account signed out here: none
    assert_eq!(ring_account(&record, "999", &keyed), None);
    assert_eq!(ring_account(&record, "", &keyed), None);
    assert_eq!(ring_account(&record, "100051", &[A.to_string()]), None);
    // an account on UnifiedPush is not found by an FCM push id
    let mut unified = record.clone();
    unified
        .endpoints
        .insert(C.to_string(), "https://up.example.net/c".to_string());
    assert_eq!(ring_account(&unified, "100050", &keyed).as_deref(), Some(A));
}

#[test]
fn a_sign_out_takes_the_registration_back_with_the_keys_and_endpoint() {
    let poster = fake(vec![(A, Ok((200, json!({"deletedCount": 1}))))]);
    let mut record = PushRecord {
        token: Some("tok".to_string()),
        registered: [(A.to_string(), "1".to_string()), (B.to_string(), "2".to_string())].into(),
        keys: [(A.to_string(), ("PUB".to_string(), "AUTH".to_string()))].into(),
        endpoints: [(B.to_string(), "https://up.example.net/b".to_string())].into(),
        instances: [(B.to_string(), "0123456789abcdef0123456789abcdef".to_string())].into(),
        ..Default::default()
    };
    assert!(record.unregister(&asked(A), &poster));
    assert_eq!(record.registered.keys().collect::<Vec<_>>(), vec![B]);
    assert!(record.keys.is_empty());
    assert_eq!(
        poster.posted.borrow()[0].1,
        format!("key-{A} /qapps/unregisterClientPush")
    );
    assert_eq!(poster.posted.borrow()[0].2, "token=tok");
    // a server that cannot be reached: forgotten here all the same (the key's revoke deletes it there); a UnifiedPush
    // account takes back its endpoint
    assert!(record.unregister(&asked(B), &poster));
    assert_eq!(poster.posted.borrow()[1].2, "token=https://up.example.net/b");
    assert!(record.registered.is_empty() && record.endpoints.is_empty());
    assert!(record.instances.is_empty(), "its UnifiedPush instance goes with it");
    assert!(!record.forget(C));
}

/// plans/Comms-Messaging.md §7.4: each account registers with its own keys, new keys or a UnifiedPush endpoint register
/// that account again, and an account not yet registered is one to try again.
#[test]
fn each_account_registers_with_its_own_keys_and_endpoint() {
    let poster = fake(vec![
        (A, Ok((200, json!({"registered": true, "subscriptionId": "7"})))),
        (B, Ok((200, json!({"registered": true, "subscriptionId": "7"})))),
    ]);
    let mut record = PushRecord::default();
    assert!(record.set_token("tok"));
    assert!(record.set_keys(A, "PUB-A", "AUTH-A"));
    assert!(!record.set_keys(A, "PUB-A", "AUTH-A"));
    assert!(record.set_keys(B, "PUB-B", "AUTH-B"));
    assert!(record.pending(&[asked(A)]));
    assert!(record.register(&[asked(A), asked(B)], &poster));
    assert!(!record.pending(&[asked(A), asked(B)]));
    assert!(poster.posted.borrow()[0]
        .2
        .ends_with("p256dhKey=PUB-A&authSecret=AUTH-A"));
    assert!(poster.posted.borrow()[1]
        .2
        .ends_with("p256dhKey=PUB-B&authSecret=AUTH-B"));
    // other keys of one account: that account registers again, not the other
    assert!(record.set_keys(A, "PUB-A2", "AUTH-A"));
    assert_eq!(record.registered.keys().collect::<Vec<_>>(), vec![B]);
    assert!(record.register(&[asked(A), asked(B)], &poster));
    // a distributor's endpoint for B, in place of the FCM token; a new FCM token leaves B registered
    assert!(record.set_endpoint(B, Some("https://up.example.net/b")));
    assert!(!record.set_endpoint(B, Some("https://up.example.net/b")));
    assert!(record.register(&[asked(A), asked(B)], &poster));
    assert!(poster
        .posted
        .borrow()
        .last()
        .unwrap()
        .2
        .starts_with("provider=unifiedpush&token=https://up.example.net/b"));
    assert!(record.set_token("tok-2"));
    assert_eq!(record.registered.keys().collect::<Vec<_>>(), vec![B]);
    assert!(record.set_endpoint(B, None), "back to FCM registers again");
    assert!(record.registered.is_empty());
}

/// A registration made with no lock held is taken only for an account whose token, endpoint and keys did not change in
/// the meantime.
#[test]
fn a_registration_made_outside_the_lock_merges_only_when_nothing_changed() {
    let poster = fake(vec![
        (A, Ok((200, json!({"registered": true, "subscriptionId": "7"})))),
        (B, Ok((200, json!({"registered": true, "subscriptionId": "8"})))),
    ]);
    let mut before = PushRecord::default();
    before.set_token("tok");
    before.set_keys(A, "PUB-A", "AUTH-A");
    before.set_keys(B, "PUB-B", "AUTH-B");
    let mut done = before.clone();
    assert!(done.register(&[asked(A), asked(B)], &poster));
    // meanwhile B's keys changed here
    let mut now = before.clone();
    now.set_keys(B, "PUB-B2", "AUTH-B");
    assert!(now.merge(&before, &done));
    assert_eq!(now.registered.get(A).map(String::as_str), Some("7"));
    assert_eq!(now.registered.get(B), None, "registered with the old keys: again");
    assert!(!now.merge(&before, &done));
}

/// HIGH and LOW-MEDIUM review findings: a tap link names a real account id (with dots) and opens only when this app
/// signed it.
#[test]
fn a_room_link_names_a_real_account_and_opens_only_when_this_app_signed_it() {
    let key = [7_u8; 32];
    let url = |text: &str| tauri::Url::parse(text).unwrap();
    let link = room_link(&key, A, "100", Some(7)).unwrap();
    assert!(link.starts_with(&format!(
        "org.moqui.mobile:/room?account={A}&chatRoomId=100&eventSeq=7&sig="
    )));
    assert_eq!(
        room_link_of(&url(&link), &key),
        Some((A.to_string(), "100".to_string(), Some(7)))
    );
    let plain = room_link(&key, A, "100", None).unwrap();
    assert_eq!(
        room_link_of(&url(&plain), &key),
        Some((A.to_string(), "100".to_string(), None))
    );
    // another app's link: no signature, a wrong one, another key, a changed room or message
    assert_eq!(
        room_link_of(
            &url(&format!("org.moqui.mobile:/room?account={A}&chatRoomId=100")),
            &key
        ),
        None
    );
    assert_eq!(room_link_of(&url(&link), &[8_u8; 32]), None);
    assert_eq!(
        room_link_of(&url(&link.replace("chatRoomId=100", "chatRoomId=101")), &key),
        None
    );
    assert_eq!(
        room_link_of(&url(&link.replace("eventSeq=7", "eventSeq=8")), &key),
        None
    );
    assert_eq!(room_link_of(&url(&link.replace("/room", "/call")), &key), None);
    // ids that are not ones
    assert_eq!(room_link(&key, A, "../x", None), None);
    assert_eq!(room_link(&key, "../a", "100", None), None);
    assert_eq!(account_id(A), Some(A));
    assert_eq!(account_id("a..b"), None);
    assert_eq!(account_id(".a"), None);
    assert_eq!(account_id("a/b"), None);
}

/// plans/Comms-Messaging.md §8.3 items 4, 5 and 7: what the native call tells the call screen, as a script that calls
/// the page's own function only when it has it; a value of the wrong shape tells nothing.
#[test]
fn the_native_call_tells_the_page_its_audio_picture_in_picture_and_hold() {
    let audio = tell_script("audio", r#"{"route":"earpiece","routes":["earpiece","speaker"]}"#).unwrap();
    assert!(audio.contains("moqui.call.setHostAudio({\"route\":\"earpiece\",\"routes\":[\"earpiece\",\"speaker\"]})"));
    assert!(audio.starts_with("window.moqui && moqui.call && typeof moqui.call.setHostAudio === \"function\""));
    assert!(tell_script("pip", "true")
        .unwrap()
        .ends_with("moqui.call.setHostPip(true)"));
    assert!(tell_script("mute", "false")
        .unwrap()
        .ends_with("moqui.call.setHostMute(false)"));
    assert!(tell_script("muteToggle", "true")
        .unwrap()
        .ends_with("moqui.call.toggleMute()"));
    // the phone's contacts allowed later: the chat names its phone rooms again
    assert!(tell_script("contacts", "true")
        .unwrap()
        .ends_with("moqui.chat.contactsChanged()"));
    assert_eq!(tell_script("contacts", "{}"), None);
    assert!(tell_script("hold", r#"{"callSessionId":"100231","hold":true}"#)
        .unwrap()
        .ends_with("moqui.call.hostHold(\"100231\", true)"));
    assert_eq!(
        tell_script("hold", r#"{"callSessionId":"a\");x(\"","hold":true}"#),
        None
    );
    assert_eq!(tell_script("pip", "\"yes\""), None);
    assert_eq!(tell_script("audio", "[]"), None);
    assert_eq!(tell_script("eval", "{}"), None);
}
