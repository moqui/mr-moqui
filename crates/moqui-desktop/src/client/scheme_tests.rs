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

use tauri::http::{HeaderMap, HeaderValue};

use super::forwarded_on_shell;

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_static(value));
    }
    map
}

#[test]
fn the_shell_takes_the_servers_data_not_its_pages_or_scripts() {
    assert!(forwarded_on_shell(&headers(&[("sec-fetch-dest", "empty")])));
    assert!(forwarded_on_shell(&headers(&[("sec-fetch-dest", "image")])));
    assert!(forwarded_on_shell(&headers(&[("accept", "*/*")])));
    for dest in [
        "document",
        "iframe",
        "script",
        "style",
        "worker",
        "sharedworker",
        "serviceworker",
        "object",
    ] {
        assert!(!forwarded_on_shell(&headers(&[("sec-fetch-dest", dest)])), "{dest}");
    }
    assert!(!forwarded_on_shell(&headers(&[(
        "accept",
        "text/html,application/xhtml+xml"
    )])));
}

#[test]
fn the_chat_store_key_is_kept_from_the_servers_answer_and_dropped_on_revocation() {
    use super::{store_key_change, store_key_device};
    assert_eq!(store_key_device(br#"{"deviceId":"ABCDEF"}"#).as_deref(), Some("ABCDEF"));
    assert_eq!(store_key_device(br#"{"deviceId":""}"#), None);
    assert_eq!(store_key_device(b"deviceId=ABCDEF"), None);
    assert_eq!(
        store_key_change(br#"{"storeKey":"k-43","legacyStoreKey":null}"#),
        Some(Some("k-43".to_string()))
    );
    assert_eq!(
        store_key_change(br#"{"reasonCode":"revoked","errors":"x"}"#),
        Some(None)
    );
    assert_eq!(store_key_change(br#"{"reasonCode":"notFound"}"#), Some(None));
    // another refusal, or no key: no change
    assert_eq!(store_key_change(br#"{"reasonCode":"failed"}"#), None);
    assert_eq!(store_key_change(br#"{"storeKey":null,"legacyStoreKey":"old"}"#), None);
}

#[test]
fn the_hosts_own_unreachable_answer_is_marked_offline() {
    let answer = super::unreachable("connection refused");
    assert_eq!(answer.status(), 502);
    assert_eq!(answer.headers().get("X-Moqui-Offline").unwrap(), "unreachable");
    assert!(String::from_utf8_lossy(answer.body()).contains("connection refused"));
}
