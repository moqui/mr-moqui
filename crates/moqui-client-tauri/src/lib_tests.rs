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
    let dir = std::env::temp_dir().join(format!("moqui-client-tauri-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("shell")).unwrap();
    std::fs::write(dir.join("shell/index.html"), "<html>shell</html>").unwrap();
    std::fs::write(dir.join("shell/libs.txt"), "vue\n").unwrap();
    let keys = Keys::new(Box::new(FileKeyStore::new(&dir.join("keys"))), "file");
    let client = Arc::new(Client::new(&dir.join("client"), keys, Box::new(ClientConf::default)).unwrap());
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
