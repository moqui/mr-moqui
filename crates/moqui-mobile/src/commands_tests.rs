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

use super::{clip, external_url, under};

#[test]
fn only_the_webs_and_the_handlers_links_open_outside_the_app() {
    assert!(external_url("https://www.moqui.org/").is_ok());
    assert!(external_url("mailto:joe@example.com").is_ok());
    assert!(external_url("tel:+15551234").is_ok());
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
