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

use super::{
    document_request, forwarded_on_shell, has_script_policy, page_policies, upgraded_page, PAGE_PERMISSIONS_POLICY,
};
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

/// SEC-CLI-105: a server's page in an app window gets `upgrade-insecure-requests` as a meta tag at the start of its
/// head, so its `http://` loads of other hosts go over https (the app's own `http://*.localhost` origins are not
/// upgraded, which Chromium 156 was seen to keep), and a page with no head still gets it before its content and after
/// its doctype, so it stays in standards mode.
#[test]
fn a_servers_page_has_its_loads_of_other_hosts_upgraded_in_its_head() {
    let meta = "<meta http-equiv=\"Content-Security-Policy\" content=\"upgrade-insecure-requests\">";
    let page = |html: &str| String::from_utf8(upgraded_page(html.as_bytes().to_vec(), &page_policies(&[]))).unwrap();
    assert_eq!(
        page("<!DOCTYPE html><html lang=\"en\"><HEAD><title>t</title></HEAD><body><header>h</header></body></html>"),
        format!("<!DOCTYPE html><html lang=\"en\"><HEAD>{meta}<title>t</title></HEAD><body><header>h</header></body></html>")
    );
    assert_eq!(
        page("<html><head data-x=\"1\"></head></html>"),
        format!("<html><head data-x=\"1\">{meta}</head></html>")
    );
    // no head: after the html tag, else the doctype, else first; a <header> is not a head
    assert_eq!(page("<html><body>x</body></html>"), format!("<html>{meta}<body>x</body></html>"));
    assert_eq!(page("<!doctype html><header>x</header>"), format!("<!doctype html>{meta}<header>x</header>"));
    assert_eq!(page("<p>x</p>"), format!("{meta}<p>x</p>"));
}

/// SEC-CLI-124 and SEC-CLI-90: a server's page gives the camera, microphone and location to its own origin only, so a
/// frame of another origin it embeds with `allow="camera"` does not get them by the window's grant.
#[test]
fn a_servers_page_keeps_its_media_to_its_own_origin() {
    for feature in ["camera", "microphone", "geolocation"] {
        assert!(
            PAGE_PERMISSIONS_POLICY
                .split(", ")
                .any(|directive| directive == format!("{feature}=(self)")),
            "{feature}"
        );
    }
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

/// SEC-CLI-81: every account's server pages share the app's origin, so no server's worker runs there: a service
/// worker one server's page registered would answer the next account's requests.
#[test]
fn no_servers_worker_runs_on_the_apps_origin() {
    use super::worker_request;
    for dest in ["worker", "sharedworker", "serviceworker", "ServiceWorker"] {
        assert!(worker_request(&headers(&[("sec-fetch-dest", dest)])), "{dest}");
    }
    assert!(worker_request(&headers(&[("service-worker", "script")])));
    for dest in ["document", "iframe", "script", "style", "image", "empty", ""] {
        assert!(!worker_request(&headers(&[("sec-fetch-dest", dest)])), "{dest}");
    }
}

/// V5: the app runs its own vault module, so the server's (its worker's script and its wasm) is refused on the app's
/// origins before any account is looked at, however the path is cased or encoded; the rest of the server's scripts pass.
#[test]
fn the_servers_vault_module_is_never_loaded_in_the_app() {
    use super::vault_module_path;
    for path in [
        "/js/vault/VaultWorker.js",
        "/js/vault/moqui_vault.wasm",
        "/js/vault/MOQUI_VAULT.WASM",
        "/js/vault/%56aultWorker.js",
        "/other/mount/vaultworker.js",
        "/js/vault/VaultWorker.js/",
    ] {
        assert!(vault_module_path(path), "{path}");
    }
    for path in [
        "/js/vault/VaultClient.js",
        "/js/WebrootVue.js",
        "/qapps/boss/Commons/Vault",
    ] {
        assert!(!vault_module_path(path), "{path}");
    }
    let Fixture { state, dir, .. } = fixture("vault-module", false);
    for path in ["/js/vault/VaultWorker.js", "/js/vault/moqui_vault.wasm"] {
        let refused = super::answer(
            &state,
            "remote-1",
            get(path, &[("sec-fetch-dest", "empty")]),
            APP_ORIGIN,
            &super::NoHooks,
        );
        assert_eq!(refused.status(), 403, "{path}");
        // the shell's origin too, before its own files are looked at
        let on_shell = super::answer(&state, "main", get(path, &[]), SHELL_ORIGIN, &super::NoHooks);
        assert_eq!(on_shell.status(), 403, "{path} on the shell");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_chat_store_key_is_kept_from_the_servers_answer_and_dropped_on_revocation() {
    use super::{store_key_change, store_key_device};
    assert_eq!(store_key_device(br#"{"deviceId":"ABCDEF"}"#).as_deref(), Some("ABCDEF"));
    assert_eq!(store_key_device(br#"{"deviceId":""}"#), None);
    assert_eq!(store_key_device(b"deviceId=ABCDEF"), None);
    assert_eq!(
        store_key_change(br#"{"storeKey":"k-43"}"#),
        Some(Some("k-43".to_string()))
    );
    assert_eq!(
        store_key_change(br#"{"reasonCode":"revoked","errors":"x"}"#),
        Some(None)
    );
    assert_eq!(store_key_change(br#"{"reasonCode":"notFound"}"#), Some(None));
    // another refusal, or no key: no change
    assert_eq!(store_key_change(br#"{"reasonCode":"failed"}"#), None);
    assert_eq!(store_key_change(br#"{"storeKey":null}"#), None);
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
        &moqui_client::assets::configured_shell_csp()
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

/// SEC-CLI-04: the CSP of the server's that its header could not carry keeps all a meta tag takes (`form-action`
/// among them; `frame-ancestors`, `report-uri` and `sandbox` it ignores), each policy its own tag, before the upgrade.
#[test]
fn a_servers_policy_with_no_script_policy_is_kept_in_the_page() {
    assert_eq!(
        page_policies(&["frame-ancestors 'none'; form-action 'self';"]),
        ["form-action 'self'", "upgrade-insecure-requests"]
    );
    assert_eq!(
        page_policies(&["form-action 'self'; img-src https:; report-uri /csp", "sandbox; frame-ancestors 'none'"]),
        ["form-action 'self'; img-src https:", "upgrade-insecure-requests"]
    );
    let page = String::from_utf8(upgraded_page(
        b"<html><head></head></html>".to_vec(),
        &page_policies(&["form-action 'self' https://sso.example.com/?a=1&b=\"2\""]),
    ))
    .unwrap();
    assert_eq!(
        page,
        "<html><head><meta http-equiv=\"Content-Security-Policy\" content=\"form-action 'self' https://sso.example.com/?a=1&amp;b=&quot;2&quot;\"><meta http-equiv=\"Content-Security-Policy\" content=\"upgrade-insecure-requests\"></head></html>"
    );
}
