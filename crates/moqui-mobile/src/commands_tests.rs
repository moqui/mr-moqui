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
    call_name, chat_room_of, clip, contact_numbers, contact_term, external_url, notification_title, tag_id, under,
};

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
