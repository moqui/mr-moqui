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

use tauri::Url;

use super::{goes, is_start, nonce_script, own_host, server_path, FreshLoad, Goes, PageState};

const SHELL: &str = "http://moqui-shell.localhost";
const APP: &str = "http://moqui-app.localhost";

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn only_the_shells_current_load_with_its_nonce_has_the_shells_commands() {
    let page = PageState::default();
    let shell = url("http://moqui-shell.localhost/");
    // nothing loaded yet
    assert!(!page.is_shell(Some(&shell), None));
    page.started(&shell);
    // loading: no nonce yet, so no command
    assert!(!page.is_shell(Some(&shell), None));
    let nonce = page.shell_loaded(&shell).expect("a nonce for the shell's load");
    assert!(page.is_shell(Some(&shell), Some(&nonce)));
    assert!(!page.is_shell(Some(&shell), None));
    assert!(!page.is_shell(Some(&shell), Some("another")));
    // the webview's URL must say so too
    assert!(!page.is_shell(Some(&url("http://moqui-app.localhost/")), Some(&nonce)));
    assert!(!page.is_shell(None, Some(&nonce)));
    // a navigation forgets the nonce: a document that is still there cannot call with it
    page.navigating(None);
    assert!(!page.is_shell(Some(&shell), Some(&nonce)));
    // the next load has a nonce of its own
    page.started(&shell);
    assert!(!page.is_shell(Some(&shell), Some(&nonce)));
    let next = page.shell_loaded(&shell).unwrap();
    assert_ne!(next, nonce);
    assert!(page.is_shell(Some(&shell), Some(&next)));
    // a server page's load gets no nonce
    page.started(&url("http://moqui-app.localhost/qapps/system"));
    assert_eq!(page.shell_loaded(&url("http://moqui-app.localhost/qapps/system")), None);
    assert_eq!(page.shell_loaded(&shell), None);
    // the nonce is handed only to a document on the shell's origin
    let script = nonce_script("n1");
    assert!(
        script.starts_with("if (location.origin === \"http://moqui-shell.localhost\""),
        "{script}"
    );
    assert!(script.contains("window.__moquiShellNonce(\"n1\")"));
}

#[test]
fn a_server_page_is_its_accounts_only_while_that_account_is_current() {
    let page = PageState::default();
    page.bind_app("a1", Some("erp.example.com"));
    // its dialogs' and notifications' title (SEC-CLI-119)
    assert_eq!(page.app_server().as_deref(), Some("erp.example.com"));
    page.navigating(None);
    // while a navigation to anywhere else is under way, no account
    assert_eq!(page.account(Some("a1")), None);
    page.navigating(Some(&url("about:blank")));
    assert_eq!(page.account(Some("a1")), None);
    page.navigating(Some(&url("http://moqui-shell.localhost/")));
    assert_eq!(page.account(Some("a1")), None);
    page.started(&url("http://moqui-app.localhost/qapps/system"));
    assert_eq!(page.account(Some("a1")).as_deref(), Some("a1"));
    // another account current: the page is no one's
    assert_eq!(page.account(Some("a2")), None);
    assert_eq!(page.account(None), None);
    // a switch unbinds the pages before the other account is current
    assert!(!page.unbind_app(Some("a2")));
    assert_eq!(
        page.app_server().as_deref(),
        Some("erp.example.com"),
        "another account's unbind"
    );
    assert!(page.unbind_app(Some("a1")));
    assert_eq!(page.account(Some("a1")), None);
    assert_eq!(page.app_server(), None, "the server goes with the pages' account");
    assert!(!page.unbind_app(None));
    // pages opened for another account name its server, not the one before
    page.bind_app("a2", Some("crm.example.org"));
    assert_eq!(page.app_server().as_deref(), Some("crm.example.org"));
    page.bind_app("a3", None);
    assert_eq!(page.app_server(), None);
    assert!(page.unbind_app(None));
    // the shell is the current account's
    page.started(&url("http://moqui-shell.localhost/"));
    assert_eq!(page.account(Some("a2")).as_deref(), Some("a2"));
}

#[test]
fn the_one_webview_goes_to_the_shell_and_the_servers_pages_and_the_web_goes_outside() {
    let from_shell = Some(SHELL);
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/"), from_shell, false),
        Goes::Here
    );
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/index.html"), from_shell, true),
        Goes::Here
    );
    // a server path on the shell's origin is the server's page, on the app origin
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/qapps/system"), from_shell, true),
        Goes::AppPage
    );
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/qapps/system"), from_shell, false),
        Goes::Nowhere
    );
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/BasicComms"), from_shell, true),
        Goes::Here
    );
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/BasicComms"), from_shell, false),
        Goes::Nowhere
    );
    // a server page never goes to the shell itself, nor does a page while a load is under way
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/"), Some(APP), true),
        Goes::Nowhere
    );
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/qapps/x"), Some(APP), true),
        Goes::Nowhere
    );
    // with no page (a load under way) only the host goes to the shell (PageState::from)
    assert_eq!(goes(&url("http://moqui-shell.localhost/"), None, true), Goes::Nowhere);
    let page = PageState::default();
    assert_eq!(page.from(), None);
    page.host_shows_shell();
    assert_eq!(page.from().as_deref(), Some(SHELL));
    page.started(&url("http://moqui-shell.localhost/"));
    page.navigating(None);
    assert_eq!(page.from(), None, "the host's showing ends as the shell's load begins");
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/x"), Some(APP), true),
        Goes::Here
    );
    assert_eq!(goes(&url("https://www.moqui.org/"), Some(APP), true), Goes::Outside);
    assert_eq!(goes(&url("mailto:joe@example.com"), from_shell, true), Goes::Outside);
    assert_eq!(goes(&url("tel:+15551234"), from_shell, false), Goes::Outside);
    assert_eq!(goes(&url("about:blank"), from_shell, true), Goes::Here);
    // this device's and the app's own hosts, a file, a data document, an intent: nowhere
    assert_eq!(goes(&url("http://tauri.localhost/"), from_shell, true), Goes::Nowhere);
    assert_eq!(
        goes(&url("http://localhost:8080/qapps"), Some(APP), true),
        Goes::Nowhere
    );
    assert_eq!(goes(&url("file:///data/local"), from_shell, true), Goes::Nowhere);
    assert_eq!(goes(&url("data:text/html,hi"), from_shell, true), Goes::Nowhere);
    assert_eq!(
        goes(&url("intent://scan/#Intent;scheme=zxing;end"), from_shell, true),
        Goes::Nowhere
    );
    assert!(own_host("localhost") && own_host("moqui-app.localhost") && !own_host("localhost.example.com"));
}

#[test]
fn a_server_page_opens_at_a_plain_path_only() {
    assert_eq!(server_path(" /qapps/system "), Ok("/qapps/system"));
    for bad in ["qapps", "//evil.example/x", "/a\\b", "/a\nb", "https://evil.example/"] {
        assert!(server_path(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_document_of_a_server_page_opened_is_asked_for_with_its_account() {
    // open_remote: bind, then navigate. The WebView asks the scheme for the document before onPageStarted, so the
    // request comes while the load has not begun: it must have the bound account, or the page is a 401.
    let page = PageState::default();
    page.started(&url("http://moqui-shell.localhost/"));
    page.bind_app("a1", None);
    page.navigating(Some(&url("http://moqui-app.localhost/qapps/system")));
    assert_eq!(page.account(Some("a1")).as_deref(), Some("a1"));
    // only while the bound account is the current one
    assert_eq!(page.account(Some("a2")), None);
    // a link on a server page to another of them
    page.started(&url("http://moqui-app.localhost/qapps/system"));
    page.navigating(Some(&url("http://moqui-app.localhost/qapps/other")));
    assert_eq!(page.account(Some("a1")).as_deref(), Some("a1"));
    // the way back to the shell: no account until the shell's load begins
    page.navigating(Some(&url("http://moqui-shell.localhost/")));
    assert_eq!(page.account(Some("a1")), None);
    // nothing bound (a sign-out unbinds): none
    page.unbind_app(None);
    page.navigating(Some(&url("http://moqui-app.localhost/qapps/system")));
    assert_eq!(page.account(Some("a1")), None);
}

/// A fresh navigation clears the history once, at the load of the page it went to (or of the page a redirect there goes
/// on to); a load of the page before it, or of another page, does not; a later navigation, or a page of another origin
/// started in its place, takes it back.
#[test]
fn a_fresh_navigation_clears_the_history_once_at_its_own_page() {
    let fresh = FreshLoad::default();
    let start = url(&format!("{APP}/qapps/feed"));
    let home = url(&format!("{APP}/qapps/feed/Home"));
    let shell = url(&format!("{SHELL}/"));
    assert!(!fresh.loaded(&start));
    fresh.navigating(Some(&start));
    // the shell going: its late events are not the start page coming, and do not take the navigation back
    fresh.started(&shell);
    assert!(!fresh.loaded(&shell));
    // nor is another page of the app origin before the start page's own start
    fresh.started(&home);
    assert!(!fresh.loaded(&home));
    fresh.started(&start);
    assert!(fresh.loaded(&start));
    assert!(!fresh.loaded(&start), "once");
    // a redirect's page that goes on before it finished: the page it goes to
    fresh.navigating(Some(&start));
    fresh.started(&start);
    fresh.started(&url("about:blank"));
    fresh.started(&home);
    assert!(!fresh.loaded(&start));
    assert!(fresh.loaded(&home));
    // a page of another origin started in its place: the start page never came, and a later one adds to the history
    fresh.navigating(Some(&start));
    fresh.started(&start);
    fresh.started(&shell);
    assert!(!fresh.loaded(&start));
    // a navigation of the host's that adds to the history replaces it
    fresh.navigating(Some(&start));
    fresh.navigating(None);
    fresh.started(&start);
    assert!(!fresh.loaded(&start));
}

#[test]
fn the_start_path_is_matched_as_the_shell_opens_it() {
    let start = Some("/apps/feed");
    assert!(is_start("/qapps/feed", start));
    assert!(is_start("/qapps/feed/?tab=new", start));
    assert!(is_start("/apps/feed", Some("/qapps/feed")));
    assert!(!is_start("/qapps/feed/Item", start));
    assert!(!is_start("/qapps/feeds", start));
    assert!(!is_start("/qapps/appsfeed", Some("/appsfeed")));
    assert!(!is_start("/qapps/feed", None));
}
