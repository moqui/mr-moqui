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

use super::{document_request, forwarded_on_shell, has_script_policy};
use crate::tests::{fixture, Fixture};
use crate::{APP_ORIGIN, SHELL_ORIGIN};

fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_static(value));
    }
    map
}

fn get(path: &str, pairs: &[(&'static str, &'static str)]) -> tauri::http::Request<Vec<u8>> {
    let mut request = tauri::http::Request::builder().uri(format!("http://moqui-shell.localhost{path}"));
    for (name, value) in pairs {
        request = request.header(*name, *value);
    }
    request.body(Vec::new()).unwrap()
}

#[test]
fn a_navigation_is_a_document_and_a_fetch_is_not() {
    assert!(document_request(&headers(&[("sec-fetch-dest", "document")])));
    assert!(document_request(&headers(&[("sec-fetch-dest", "iframe")])));
    assert!(!document_request(&headers(&[
        ("sec-fetch-dest", "empty"),
        ("accept", "text/html,application/xhtml+xml")
    ])));
    assert!(document_request(&headers(&[(
        "accept",
        "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
    )])));
    // loadComponent's
    assert!(!document_request(&headers(&[(
        "accept",
        "text/html,application/javascript,*/*"
    )])));
    assert!(!document_request(&headers(&[])));
}

#[test]
fn only_a_servers_script_policy_is_kept() {
    assert!(!has_script_policy("frame-ancestors 'none'; form-action 'self';"));
    assert!(has_script_policy(
        "frame-ancestors 'none'; script-src 'self' 'unsafe-eval'"
    ));
    assert!(has_script_policy("Default-Src 'self'"));
    assert!(!has_script_policy(""));
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

#[test]
fn the_shell_is_served_from_its_files_under_its_policy_and_never_forwarded() {
    let Fixture { state, dir, .. } = fixture("shell-files", false);
    let page = super::answer(&state, "main", get("/", &[]), SHELL_ORIGIN, &super::NoHooks);
    assert_eq!(page.status(), 200);
    assert_eq!(page.body(), b"<html>shell</html>");
    assert_eq!(
        page.headers().get("Content-Security-Policy").unwrap(),
        moqui_client::assets::SHELL_CSP
    );
    // one of the shell's paths with no such file: not found, not the server's
    let missing = super::answer(
        &state,
        "main",
        get("/js/Nothing.js", &[]),
        SHELL_ORIGIN,
        &super::NoHooks,
    );
    assert_eq!(missing.status(), 404);
    // the server's page on the shell's origin is refused before any account is looked at
    let document = super::answer(
        &state,
        "main",
        get("/qapps/system", &[("sec-fetch-dest", "document")]),
        SHELL_ORIGIN,
        &super::NoHooks,
    );
    assert_eq!(document.status(), 403);
    // with no account signed in, the server's data is not asked for
    let data = super::answer(
        &state,
        "main",
        get("/qapps/clientConfig", &[]),
        SHELL_ORIGIN,
        &super::NoHooks,
    );
    assert_eq!(data.status(), 401);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_no_secure_connection_the_pages_answer_from_the_store_or_as_offline() {
    let Fixture {
        state, dir, account, ..
    } = fixture("no-network", true);
    // what the store keeps of the shell's settings is answered, marked offline, and nothing else is asked for
    state
        .core
        .with_store(&account, |store| {
            store.put_answer("/qapps/clientConfig", "", "application/json", br#"{"server":{}}"#)
        })
        .unwrap()
        .unwrap();
    let kept = super::answer(
        &state,
        "main",
        get("/qapps/clientConfig", &[]),
        SHELL_ORIGIN,
        &super::NoHooks,
    );
    assert_eq!(kept.status(), 200);
    assert_eq!(kept.headers().get("X-Moqui-Offline").unwrap(), "1");
    let other = super::answer(
        &state,
        "main",
        get("/qapps/Other/data", &[]),
        APP_ORIGIN,
        &super::NoHooks,
    );
    assert_eq!(other.status(), 502);
    assert_eq!(other.headers().get("X-Moqui-Offline").unwrap(), "unreachable");
    let _ = std::fs::remove_dir_all(&dir);
}
