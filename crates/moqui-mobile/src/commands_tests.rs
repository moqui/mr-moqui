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

use super::{
    call_name, chat_room_of, clip, contact_numbers, contact_term, external_url, notification_title, tag_id,
    tapped_path, under,
};
use crate::calls::notify_signature;

#[test]
fn only_the_webs_and_the_handlers_links_open_outside_the_app() {
    assert!(external_url("https://www.moqui.org/").is_ok());
    assert!(external_url("mailto:joe@example.com").is_ok());
    assert!(external_url("tel:+15551234").is_ok());
    // a chat location's Open in maps (phase 20)
    assert!(external_url("geo:45.500000,-122.600000?q=45.500000,-122.600000(The%20shop)").is_ok());
    assert!(external_url("geo:0,0?q=1%20Main%20St").is_ok());
    for bad in [
        "http://moqui-shell.localhost/",
        "http://moqui-app.localhost/qapps",
        "http://localhost:8080/",
        "file:///data/data/org.moqui.mobile/files",
        "intent://x#Intent;end",
        "javascript:alert(1)",
        "not a url",
    ] {
        assert!(external_url(bad).is_err(), "{bad}");
    }
    assert!(external_url(&format!("https://example.com/{}", "a".repeat(9000))).is_err());
}

#[test]
fn another_apps_own_scheme_opens_outside_the_app_but_not_the_phones_files_or_scripts() {
    assert!(external_url("notes-app://new?file=a&content=b").is_ok());
    assert!(external_url("spotify:track:1").is_ok());
    for bad in [
        "content://com.android.contacts/contacts",
        "data:text/html,<b>x</b>",
        "blob:http://moqui-app.localhost/1",
        "about:blank",
        "android-app://com.example/http/x",
        "org.moqui.mobile:/sso?code=1",
        "ftp://example.com/a",
        "x:1",
    ] {
        assert!(external_url(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_server_page_shows_in_the_shell_only_its_comms_apps_screens() {
    assert!(under("/qapps/BasicComms/Chat?room=1", "/qapps/BasicComms"));
    assert!(under("/qapps/Comms/Inbox/Mail", "/qapps/Comms/"));
    assert!(!under("/qapps/BasicCommsOther/Chat", "/qapps/BasicComms"));
    assert!(!under("/qapps/BasicComms", "/qapps/BasicComms"));
    assert!(!under("/qapps/system", ""));
}

#[test]
fn a_notification_is_plain_text_within_its_limit() {
    assert_eq!(clip(" a\nb\tc ", 10), "a b c");
    assert_eq!(clip("abcdef", 3), "abc");
}

/// SEC-CLI-119: a page's notification shows as the app's own, so only the shell's (by its page and nonce) has its title
/// as given; any other page's names the server its pages were opened for, or says it is a server page's.
#[test]
fn a_server_pages_notification_title_names_its_server() {
    assert_eq!(
        notification_title("New message", true, Some("erp.example.com")),
        "New message"
    );
    assert_eq!(
        notification_title("Your password expired", false, Some("erp.example.com:8443")),
        "erp.example.com:8443: Your password expired"
    );
    assert_eq!(
        notification_title("Sign in again", false, None),
        "A server page: Sign in again"
    );
    assert_eq!(
        notification_title("Sign in again", false, Some(" ")),
        "A server page: Sign in again"
    );
}

/// A tag of an account is one notification id, the same each time and clear of the app's own ids; another account's
/// room of the same id is another notification; no tag, none.
#[test]
fn a_tag_is_one_notification_id_per_account() {
    let a = Some("chat.example.com-0123456789ab");
    let b = Some("chat.example.org-ba9876543210");
    let room = tag_id(a, Some("chat-100")).unwrap();
    assert_eq!(tag_id(a, Some(" chat-100 ")), Some(room));
    assert_ne!(tag_id(a, Some("chat-101")), Some(room));
    assert_ne!(tag_id(b, Some("chat-100")), Some(room), "another account's room 100");
    assert!(room >= 1000);
    assert_eq!(tag_id(a, Some(" ")), None);
    assert_eq!(tag_id(a, None), None);
    assert_eq!(chat_room_of("chat-100"), Some("100"));
    assert_eq!(chat_room_of("call-100"), None);
    assert_eq!(chat_room_of("chat-a/b"), None);
}

/// A call's name as a car or a watch shows it: one line of at most 80 characters, none when the page gave none.
#[test]
fn a_calls_name_is_one_short_line() {
    assert_eq!(call_name(Some(" Ann,\nBob ")), "Ann, Bob");
    assert_eq!(call_name(Some(&"x".repeat(200))).len(), 80);
    assert_eq!(call_name(None), "");
    assert_eq!(call_name(Some("\u{7}")), "");
}

/// The contacts asked of the phone: a term of two characters or more, and only E.164 numbers, once each, at most 50.
#[test]
fn the_phone_book_is_asked_only_a_term_and_e164_numbers() {
    assert_eq!(contact_term(" ann\n").as_deref(), Some("ann"));
    assert_eq!(contact_term("a"), None, "one character searches nothing");
    assert_eq!(contact_term(&"b".repeat(150)).map(|term| term.len()), Some(100));
    let asked: Vec<String> = [
        "+18015550100",
        " +18015550100 ",
        "8015550100",
        "+1",
        "+1801555010x",
        "+447700900123",
    ]
    .iter()
    .map(|number| number.to_string())
    .collect();
    assert_eq!(
        contact_numbers(&asked),
        vec!["+18015550100".to_string(), "+447700900123".to_string()]
    );
    let many: Vec<String> = (0..80).map(|n| format!("+1801555{n:04}")).collect();
    assert_eq!(contact_numbers(&many).len(), 50);
}

/// SEC-CLI-88: a page notification's tap opens its link only with the app's signature of it and its account, the
/// current one: any app can send the exported activity a tap's intent with a link of its own, and links may be any
/// server path, so the path is not narrowed.
#[test]
fn a_notifications_tap_opens_its_link_only_when_the_app_signed_it_for_the_current_account() {
    let key = [7_u8; 32];
    let (a, b) = ("chat.example.com-0123456789ab", "chat.example.org-ba9876543210");
    let path = "/qapps/system/Security/UserAccount?userId=ANN";
    let sig = notify_signature(&key, a, path);
    assert_eq!(tapped_path(&key, Some(a), Some(path), Some(a), Some(&sig)), Some(path));
    // the link as the notification carried it, spaces and all, is the path that was signed
    assert_eq!(
        tapped_path(&key, Some(a), Some(&format!(" {path} ")), Some(a), Some(&sig)),
        Some(path)
    );
    // no signature (another app's tap), a wrong one, the app's of another path, made with another key
    assert_eq!(tapped_path(&key, Some(a), Some(path), Some(a), None), None);
    assert_eq!(tapped_path(&key, Some(a), Some(path), Some(a), Some("")), None);
    assert_eq!(tapped_path(&key, Some(a), Some(path), Some(a), Some(&sig[1..])), None);
    assert_eq!(
        tapped_path(
            &key,
            Some(a),
            Some("/qapps/system/Security/UserAccount?userId=JOE"),
            Some(a),
            Some(&sig)
        ),
        None
    );
    assert_eq!(tapped_path(&[8_u8; 32], Some(a), Some(path), Some(a), Some(&sig)), None);
    // another account: one not current now (a switch, a sign-out since), and a tap that names another than it was
    // signed for
    assert_eq!(tapped_path(&key, Some(b), Some(path), Some(a), Some(&sig)), None);
    assert_eq!(tapped_path(&key, None, Some(path), Some(a), Some(&sig)), None);
    assert_eq!(tapped_path(&key, Some(b), Some(path), Some(b), Some(&sig)), None);
    assert_eq!(tapped_path(&key, Some(a), Some(path), None, Some(&sig)), None);
    // no link, or one that is not a path on the server
    assert_eq!(tapped_path(&key, Some(a), None, Some(a), Some(&sig)), None);
    let web = "//evil.example/x";
    assert_eq!(
        tapped_path(&key, Some(a), Some(web), Some(a), Some(&notify_signature(&key, a, web))),
        None
    );
    // a room link's signature of the same text is not a notification's
    assert_ne!(sig, crate::calls::link_signature(&key, &format!("room\n{a}\n{path}")));
}
