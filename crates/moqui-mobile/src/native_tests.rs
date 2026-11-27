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

use std::sync::Arc;

use moqui_client::account::Account;

use super::*;

fn account(id: &str) -> Account {
    Account {
        id: id.to_string(),
        server: format!("https://{id}.example.com"),
        username: "joe".to_string(),
        user_id: String::new(),
        display: String::new(),
        server_name: String::new(),
        device_mode: None,
    }
}

#[test]
fn one_client_per_process_until_it_stops_and_the_current_account_is_asked_first() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-native-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = core_in(&dir).unwrap();
    assert!(
        Arc::ptr_eq(&core, &core_in(&dir).unwrap()),
        "a push and the app's setup share one client"
    );
    assert!(!has_account(&dir));
    {
        let mut accounts = moqui_lib::sync::lock(core.accounts());
        for id in ["a", "b", "c"] {
            accounts.put(account(id));
        }
        accounts.set_current("b");
    }
    // c has no key: it cannot be asked
    core.keys().set("a", "key-a").unwrap();
    core.keys().set("b", "key-b").unwrap();
    let order: Vec<String> = asked(&core).into_iter().map(|asked| asked.account.id).collect();
    assert_eq!(order, vec!["b".to_string(), "a".to_string()]);
    assert_eq!(asked(&core)[0].comms_root, "/qapps/BasicComms");
    assert!(has_account(&dir));
    // a background send with nothing queued: nothing waits for a time
    assert_eq!(flush_background(&dir, 1000), -1);
    // a bad id is never sent, nor one for an account not signed in here
    assert_eq!(decline_call(&dir, "1/../x", "a"), "not a call id");
    assert_eq!(decline_call(&dir, "1", "c"), "the account c is not signed in here");
    // a push that is no registration's here is no account's
    assert_eq!(ring_account(&dir, "100050"), None);
    core.shutdown();
    let again = core_in(&dir).unwrap();
    assert!(!Arc::ptr_eq(&core, &again), "a stopped client is made again");
    again.shutdown();
    // once the process is ending, no client is made again (none recovers what was sending), and a background run
    // asks to run again elsewhere
    process_ending();
    assert!(core_in(&dir).is_err());
    assert_eq!(flush_background(&dir, 1000), 0);
    ENDING.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The notification's Reply goes to the comms app's send as escaped HTML, its lines kept, and passes the outbox's
/// check of a chat send.
#[test]
fn a_reply_is_the_comms_apps_send_of_plain_escaped_text() {
    assert_eq!(chat_send_url("/qapps/BasicComms"), "/apps/BasicComms/Chat/send");
    assert_eq!(chat_send_url("/qapps/Comms/"), "/apps/Comms/Chat/send");
    assert_eq!(
        reply_html(" a <b> & \"c\"\nnext "),
        "<p>a &lt;b&gt; &amp; &quot;c&quot;<br>next</p>"
    );
    let payload = serde_json::json!({"clientRef": "h00ff", "chatRoomId": "100", "url": chat_send_url("/qapps/BasicComms"),
        "params": {"body": reply_html("hi")}, "text": "hi"});
    let op = moqui_client::sync::outbox::check("chat-send", &payload, &[], 1_000).unwrap();
    assert_eq!(op.target, "100");
    let read = moqui_client::sync::outbox::check(
        "chat-read",
        &serde_json::json!({"chatRoomId": "100", "lastReadEventSeq": 7}),
        &[],
        1_000,
    )
    .unwrap();
    assert_eq!(read.kind, "chat-read");
}

/// HIGH review findings: each account has its own keys, so of two accounts whose servers gave the same push id, only
/// the one whose keys open a push gets it (another server's push, or one forged to look like it, opens with none);
/// and a sealed notification, whose kind is only outside, keeps its outer kind and push id.
#[test]
fn a_push_is_the_account_whose_keys_open_it_with_its_outer_kind() {
    use base64::Engine;
    let a = crate::push::PushKeys::generate().unwrap();
    let b = crate::push::PushKeys::generate().unwrap();
    let enc = |to: &crate::push::PushKeys, json: &str| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::push::seal(to, json.as_bytes()))
    };
    let to_b = enc(&b, r#"{"title":"Board","link":"/apps/x"}"#);
    let candidates = vec![
        ("chat.example.com-0123456789ab".to_string(), Some(a)),
        ("chat.example.org-ba9876543210".to_string(), Some(b)),
    ];
    let (account, opened) = open_with(&candidates, &to_b).unwrap();
    assert_eq!(account, "chat.example.org-ba9876543210");
    let opened = opened_for(opened, &account, "notification", "100050");
    assert_eq!(
        opened["kind"], "notification",
        "the outer kind of a sealed notification"
    );
    assert_eq!(opened["pushId"], "100050");
    assert_eq!(opened["account"], "chat.example.org-ba9876543210");
    assert_eq!(opened["title"], "Board");
    // its own kind stays (a chat push's inner kind is the message's)
    let inner = opened_for(serde_json::json!({"kind": "message"}), &account, "chat", "1");
    assert_eq!(inner["kind"], "message");
    // a push that opens with no account's keys here: none
    let stranger = crate::push::PushKeys::generate().unwrap();
    assert!(open_with(&candidates, &enc(&stranger, "{}")).is_none());
    // keys that cannot be read now are passed over, never made
    let unread = vec![("chat.example.org-ba9876543210".to_string(), None)];
    assert!(open_with(&unread, &to_b).is_none());
}

/// plans/archive/Maps-Offline.md §2.4: the worker is told to run again for a cut, a server error, a pause or a run still under
/// way, and done for every end the shell reads (installed, refused, cancelled, stale).
#[test]
fn a_package_download_is_tried_again_only_when_it_may_go_on() {
    use moqui_client::packages::Status;
    let failed = |retry| Status::failed(1, "OFFLINE", "cut", retry);
    assert_eq!(package_outcome(&failed(true)), PACKAGE_RETRY);
    assert_eq!(
        package_outcome(&Status::Downloading {
            revision: 1,
            done: 1,
            total: 2
        }),
        PACKAGE_RETRY
    );
    assert_eq!(package_outcome(&Status::Queued { revision: 1 }), PACKAGE_RETRY);
    // the last run gives up a failure to try again, never a pause (WorkManager runs it again) or a run under way
    assert!(package_gives_up(&failed(true)));
    assert!(!package_gives_up(&Status::failed(1, "PAUSED", "later", true)));
    assert!(!package_gives_up(&failed(false)));
    assert!(!package_gives_up(&Status::Downloading {
        revision: 1,
        done: 1,
        total: 2
    }));
    for ended in [
        failed(false),
        Status::Installed { revision: 1 },
        Status::Cancelled { revision: 1 },
        Status::Stale { revision: Some(2) },
    ] {
        assert_eq!(package_outcome(&ended), PACKAGE_DONE, "{ended:?}");
    }
}
