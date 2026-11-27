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

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_client::account::{account_id, Account};
use moqui_client::app::{Client, ClientHost};
use moqui_client::assets::Assets;
use moqui_client::conf::ClientConf;
use moqui_client::keys::{FileKeyStore, Keys};
use serde_json::Value;
use tauri::{Url, Webview};

use super::*;

/// A host whose one page, `main`, shows the current account, and which can make no request: these tests never reach a
/// server. What it was asked to show is kept.
pub(crate) struct TestHost {
    client: Mutex<Option<Arc<Client>>>,
    pub(crate) shown: Mutex<Vec<String>>,
    /// What the host answers of the device's screen lock.
    pub(crate) secure: Mutex<Option<bool>>,
    /// The accounts the host was told are signed out.
    pub(crate) signed_out: Mutex<Vec<String>>,
    /// How many times the host was told to clear the server pages' storage.
    pub(crate) storage_cleared: Mutex<usize>,
    /// Whether the host opens the build's start screen (the mobile app's) or ignores it (the desktop's).
    pub(crate) start_screen: Mutex<bool>,
}

impl ClientHost for TestHost {
    fn page_account(&self, page: &str) -> Option<String> {
        let client = moqui_lib::sync::lock(&self.client).clone()?;
        let current = moqui_lib::sync::lock(client.accounts())
            .current()
            .map(|account| account.id.clone());
        (page == "main").then_some(current).flatten()
    }

    fn is_shell(&self, page: &str) -> bool {
        page == "main"
    }

    fn notify_pages(&self, _account: &str, _event: &Value) {}

    fn socket_event(&self, _page: &str, _id: u64, _event: &Value) {}

    fn check_key(&self, _account: &Account) {}
}

impl ShellHost for TestHost {
    fn show_shell(&self, url: &Url) {
        moqui_lib::sync::lock(&self.shown).push(url.to_string());
    }

    fn close_remotes(&self, _account: Option<&str>) {}

    fn open_remote(&self, path: &str) -> Result<(), String> {
        Err(format!("no pages here: {path}"))
    }

    fn open_external(&self, _url: &Url) {}

    fn device_name(&self) -> String {
        "a test".to_string()
    }

    fn sso_redirect(&self) -> &'static str {
        moqui_client::sso::MOBILE_REDIRECT
    }

    fn is_shell_page(&self, _webview: &Webview, _nonce: Option<&str>) -> bool {
        true
    }

    fn network_ready(&self) -> bool {
        false
    }

    fn device_secure(&self) -> Option<bool> {
        *moqui_lib::sync::lock(&self.secure)
    }

    fn account_signed_out(&self, account: &str) {
        moqui_lib::sync::lock(&self.signed_out).push(account.to_string());
    }

    fn clear_page_storage(&self) {
        *moqui_lib::sync::lock(&self.storage_cleared) += 1;
    }

    fn opens_start_path(&self) -> bool {
        *moqui_lib::sync::lock(&self.start_screen)
    }
}

pub(crate) struct Fixture {
    pub(crate) state: ClientState,
    pub(crate) dir: PathBuf,
    /// The account signed in, when one is.
    pub(crate) account: String,
    pub(crate) host: Arc<TestHost>,
}

/// A client in a temp directory with the shell's page staged, and with `signed_in` an account of
/// `https://moqui.example` signed in with a key.
pub(crate) fn fixture(name: &str, signed_in: bool) -> Fixture {
    fixture_with(name, signed_in, ClientConf::default)
}

/// [`fixture`], with the build's client settings of `conf`.
pub(crate) fn fixture_with(name: &str, signed_in: bool, conf: fn() -> ClientConf) -> Fixture {
    let dir = std::env::temp_dir().join(format!("moqui-client-tauri-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("shell")).unwrap();
    std::fs::write(dir.join("shell/index.html"), "<html>shell</html>").unwrap();
    std::fs::write(dir.join("shell/libs.txt"), "vue\n").unwrap();
    let keys = Keys::new(Box::new(FileKeyStore::new(&dir.join("keys"))), "file");
    let client = Arc::new(Client::new(&dir.join("client"), keys, Box::new(conf)).unwrap());
    let server = "https://moqui.example";
    let account = account_id(server, "john.doe");
    if signed_in {
        let mut accounts = moqui_lib::sync::lock(client.accounts());
        accounts.put(Account {
            id: account.clone(),
            server: server.to_string(),
            username: "john.doe".to_string(),
            user_id: "EX_JOHN_DOE".to_string(),
            display: String::new(),
            server_name: String::new(),
            device_mode: None,
        });
        accounts.set_current(&account);
        drop(accounts);
        client.keys().set(&account, "the-key").unwrap();
    }
    let host = Arc::new(TestHost {
        client: Mutex::new(Some(Arc::clone(&client))),
        shown: Mutex::new(Vec::new()),
        secure: Mutex::new(None),
        signed_out: Mutex::new(Vec::new()),
        storage_cleared: Mutex::new(0),
        start_screen: Mutex::new(false),
    });
    let files = Assets::new(dir.join("shell"), dir.join("webroot"));
    let state = ClientState::new(client, host.clone(), host.clone(), Box::new(files));
    Fixture {
        state,
        dir,
        account,
        host,
    }
}

#[test]
fn the_shells_url_takes_a_query_with_the_hosts_token() {
    assert_eq!(shell_url(None, None).as_str(), "http://moqui-shell.localhost/");
    assert_eq!(
        shell_url(Some("signIn=1"), None).as_str(),
        "http://moqui-shell.localhost/?signIn=1"
    );
    assert_eq!(
        shell_url(Some("signIn=1"), Some("t1")).as_str(),
        "http://moqui-shell.localhost/?signIn=1&load=t1"
    );
    assert_eq!(shell_url(None, Some("t1")).as_str(), "http://moqui-shell.localhost/");
}

#[test]
fn a_load_of_the_shell_with_a_query_carries_a_token_the_state_answers_once() {
    let Fixture { state, dir, host, .. } = fixture("load-shell", false);
    state.load_shell(Some("signIn=1"));
    let token = moqui_lib::sync::lock(&state.load_token)
        .clone()
        .expect("a token for a query");
    state.load_shell(None);
    assert!(moqui_lib::sync::lock(&state.load_token).is_none());
    let shown = moqui_lib::sync::lock(&host.shown).clone();
    assert_eq!(
        shown,
        vec![
            format!("http://moqui-shell.localhost/?signIn=1&load={token}"),
            "http://moqui-shell.localhost/".to_string()
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_removed_account_is_cleared_once_by_each_origin() {
    let mut cleared = Cleared::default();
    cleared.add("a1");
    cleared.add("a1");
    cleared.add("a2");
    assert_eq!(cleared.shell, vec!["a1", "a2"]);
    assert_eq!(cleared.app, vec!["a1", "a2"]);
}

/// SEC-CLI-113: the shell hears whether the device has a secure lock screen where the host tells (the phone), and warns
/// when it has none; where it cannot tell (the desktop) it hears null.
#[test]
fn the_shell_hears_whether_the_device_has_a_screen_lock() {
    let Fixture { state, dir, host, .. } = fixture("device-secure", false);
    assert_eq!(crate::commands::state_json(&state)["deviceSecure"], Value::Null);
    *moqui_lib::sync::lock(&host.secure) = Some(false);
    assert_eq!(crate::commands::state_json(&state)["deviceSecure"], Value::Bool(false));
    *moqui_lib::sync::lock(&host.secure) = Some(true);
    assert_eq!(crate::commands::state_json(&state)["deviceSecure"], Value::Bool(true));
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-CLI-08: a personal account's sign-out keeps it in the list with its data, to sign in to again, unless the user
/// asks for its data to go too; then the account and its store go, as a shared one's do.
#[test]
fn a_personal_account_signs_out_with_its_data_deleted_only_when_asked() {
    let Fixture {
        state, dir, account, ..
    } = fixture("sign-out-keep", true);
    state.core.with_store(&account, |store| {
        store.put_answer("/qapps/clientConfig", "", "application/json", b"{}")
    });
    crate::commands::signed_out_in(&state, &account, false);
    assert!(
        moqui_lib::sync::lock(state.core.accounts()).get(&account).is_some(),
        "the account stays"
    );
    let kept = state
        .core
        .with_store(&account, |store| store.answer("/qapps/clientConfig", "").is_some());
    assert_eq!(kept, Some(true), "its store stays");
    let _ = std::fs::remove_dir_all(&dir);

    let Fixture {
        state, dir, account, ..
    } = fixture("sign-out-forget", true);
    state.core.with_store(&account, |store| {
        store.put_answer("/qapps/clientConfig", "", "application/json", b"{}")
    });
    crate::commands::signed_out_in(&state, &account, true);
    assert!(
        moqui_lib::sync::lock(state.core.accounts()).get(&account).is_none(),
        "the account goes"
    );
    let kept = state
        .core
        .with_store(&account, |store| store.answer("/qapps/clientConfig", "").is_some());
    assert_ne!(kept, Some(true), "its store goes");
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-CLI-81: the server pages' storage is cleared when the current account changes and when it signs out, so what
/// one account's pages kept on the origin they share on the phone is gone before another's open.
#[test]
fn the_pages_storage_is_cleared_when_the_current_account_changes_or_signs_out() {
    let Fixture {
        state,
        dir,
        account,
        host,
    } = fixture("page-storage", true);
    assert_eq!(*moqui_lib::sync::lock(&host.storage_cleared), 0);
    crate::commands::switched(&state);
    assert_eq!(*moqui_lib::sync::lock(&host.storage_cleared), 1);
    crate::commands::signed_out_in(&state, &account, false);
    assert_eq!(*moqui_lib::sync::lock(&host.storage_cleared), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-CLI-113: a shared-device sign-in (a password's or an SSO start's) is refused where the host says the device has
/// no secure lock screen, and goes on where it has one or the host cannot tell; a personal one always goes on.
#[test]
fn a_shared_sign_in_needs_a_screen_lock_where_the_host_can_tell() {
    let Fixture { state, dir, host, .. } = fixture("shared-lock", false);
    let ask = |chosen| crate::commands::sign_in_mode(&state, Some(chosen));
    assert_eq!(state.core.conf().device_mode, moqui_client::conf::DeviceMode::Choose);
    assert!(ask("shared").is_ok());
    *moqui_lib::sync::lock(&host.secure) = Some(false);
    assert_eq!(ask("shared"), Err(moqui_client::modes::SHARED_NEEDS_LOCK.to_string()));
    assert!(ask("personal").is_ok());
    *moqui_lib::sync::lock(&host.secure) = Some(true);
    assert!(ask("shared").is_ok());
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-CLI-100: every sign-out tells the host, which takes the account's notifications and shortcuts off the phone,
/// a sign-out the server did not hear (no network, as this host has) included; its key is gone.
#[test]
fn a_sign_out_tells_the_host_even_with_no_network() {
    let Fixture {
        state,
        dir,
        account,
        host,
    } = fixture("signed-out", true);
    assert!(state.core.keys().get(&account).is_some());
    crate::commands::signed_out_in(&state, &account, false);
    assert_eq!(moqui_lib::sync::lock(&host.signed_out).clone(), vec![account.clone()]);
    assert!(state.core.keys().get(&account).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `<client-conf start-path>`: the mobile app's shell is told it, to open it as its home; without it, none. The desktop
/// ignores it: its shell is told none.
#[test]
fn the_shell_is_told_the_start_path() {
    let Fixture { state, dir, host, .. } = fixture_with("start-path", true, || ClientConf {
        start_path: Some("/apps/feed".to_string()),
        ..ClientConf::default()
    });
    assert!(
        crate::commands::state_json(&state)["startPath"].is_null(),
        "the desktop's"
    );
    *moqui_lib::sync::lock(&host.start_screen) = true;
    assert_eq!(crate::commands::state_json(&state)["startPath"], "/apps/feed");
    let _ = std::fs::remove_dir_all(&dir);
    let Fixture { state, dir, .. } = fixture("no-start-path", true);
    assert!(crate::commands::state_json(&state)["startPath"].is_null());
    let _ = std::fs::remove_dir_all(&dir);
}

/// plans/archive/Maps-Offline.md §2.4: a shared device's sign-out deletes the account's offline map packages with its data; a
/// personal one's keeps them, to read offline after the next sign-in.
#[test]
fn a_shared_sign_out_deletes_the_accounts_map_packages_and_a_personal_one_keeps_them() {
    for (mode, kept) in [
        (moqui_client::conf::DeviceMode::Shared, false),
        (moqui_client::conf::DeviceMode::Personal, true),
    ] {
        let Fixture {
            state, dir, account, ..
        } = fixture(&format!("packages-{}", mode.as_str()), true);
        moqui_lib::sync::lock(state.core.accounts()).set_device_mode(&account, mode);
        let revision = dir.join(format!("client/data/{account}.packages/P1/1"));
        std::fs::create_dir_all(&revision).unwrap();
        std::fs::write(
            revision.join("manifest.json"),
            br#"{"package":{"name":"Lakes"},"members":[]}"#,
        )
        .unwrap();
        assert_eq!(state.core.packages().status(&account)[0]["packageId"], "P1");
        crate::commands::signed_out_in(&state, &account, false);
        assert_eq!(revision.exists(), kept, "{}", mode.as_str());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// plans/archive/Maps-Offline.md §2.4: a package's file is answered to the shell's page only, by range, and a server page on
/// the app origin is told nothing of it; the path never reaches the server.
#[test]
fn a_packages_file_is_the_shells_by_range_and_no_server_pages() {
    let Fixture {
        state, dir, account, ..
    } = fixture("package-file", true);
    let revision = dir.join(format!("client/data/{account}.packages/P1/1"));
    std::fs::create_dir_all(revision.join("style")).unwrap();
    let style = br#"{"version":8}"#;
    // served as its tag: the download checked it
    let sha = "ab".repeat(32);
    std::fs::write(revision.join("style/light.json"), style).unwrap();
    let manifest = format!(
        r#"{{"package":{{"name":"Lakes"}},"members":[{{"path":"style/light.json","size":{},"sha256":"{sha}"}}]}}"#,
        style.len()
    );
    std::fs::write(revision.join("manifest.json"), manifest).unwrap();
    let request = |range: Option<&str>| {
        let mut request = tauri::http::Request::builder()
            .method("GET")
            .uri("http://moqui-shell.localhost/__moqui-app/maps/P1/style/light.json");
        if let Some(range) = range {
            request = request.header("Range", range);
        }
        request.body(Vec::new()).unwrap()
    };
    let path = "/__moqui-app/maps/P1/style/light.json";
    let shell = crate::scheme::package_file(&state, "main", &request(Some("bytes=1-7")), path, crate::SHELL_ORIGIN);
    assert_eq!(shell.status(), 206);
    assert_eq!(shell.body().as_slice(), &style[1..8]);
    assert_eq!(
        shell.headers().get("etag").and_then(|value| value.to_str().ok()),
        Some(format!("\"sha256-{sha}\"").as_str())
    );
    let page = crate::scheme::package_file(&state, "remote-1", &request(None), path, crate::APP_ORIGIN);
    assert_eq!(page.status(), 404);
    // a server page's webview asking the shell's origin (an image or a font it embeds): nothing
    let embedded = crate::scheme::package_file(&state, "remote-1", &request(None), path, crate::SHELL_ORIGIN);
    assert_eq!(embedded.status(), 404);
    // the shell's webview, but a request that says it is another origin's
    for (name, value) in [
        ("Origin", "http://moqui-app.localhost"),
        ("Sec-Fetch-Site", "cross-site"),
    ] {
        let mut asked = request(None);
        asked
            .headers_mut()
            .insert(name, tauri::http::HeaderValue::from_static(value));
        let refused = crate::scheme::package_file(&state, "main", &asked, path, crate::SHELL_ORIGIN);
        assert_eq!(refused.status(), 404, "{name}");
    }
    let mut own = request(None);
    own.headers_mut().insert(
        "Origin",
        tauri::http::HeaderValue::from_static("http://moqui-shell.localhost"),
    );
    let own = crate::scheme::package_file(&state, "main", &own, path, crate::SHELL_ORIGIN);
    assert_eq!(own.status(), 200);
    assert_eq!(
        own.headers().get("cache-control").and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
