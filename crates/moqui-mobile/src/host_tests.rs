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

use super::{goes, server_path, Goes, PageState};

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn only_the_shells_page_once_its_load_began_has_the_shells_commands() {
    let page = PageState::default();
    let shell = url("http://moqui-shell.localhost/");
    // nothing loaded yet
    assert!(!page.is_shell(Some(&shell)));
    page.started(&shell);
    assert!(page.is_shell(Some(&shell)));
    // a server page that navigates to the shell is still the page until the shell's load begins
    page.started(&url("http://moqui-app.localhost/qapps/system"));
    assert!(!page.is_shell(Some(&url("http://moqui-app.localhost/qapps/system"))));
    page.navigating();
    assert!(!page.is_shell(Some(&shell)));
    page.started(&shell);
    assert!(page.is_shell(Some(&shell)));
    // the webview's URL must say so too
    assert!(!page.is_shell(Some(&url("http://moqui-app.localhost/"))));
    assert!(!page.is_shell(None));
}

#[test]
fn the_one_webview_goes_to_the_shell_and_the_servers_pages_and_the_web_goes_outside() {
    assert_eq!(goes(&url("http://moqui-shell.localhost/"), false), Goes::Here);
    assert_eq!(goes(&url("http://moqui-shell.localhost/index.html"), true), Goes::Here);
    // a server path on the shell's origin is the server's page, on the app origin
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/qapps/system"), true),
        Goes::AppPage
    );
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/qapps/system"), false),
        Goes::Nowhere
    );
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/BasicComms"), true),
        Goes::Here
    );
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/BasicComms"), false),
        Goes::Nowhere
    );
    assert_eq!(goes(&url("https://www.moqui.org/"), true), Goes::Outside);
    assert_eq!(goes(&url("mailto:joe@example.com"), true), Goes::Outside);
    assert_eq!(goes(&url("tel:+15551234"), false), Goes::Outside);
    assert_eq!(goes(&url("about:blank"), true), Goes::Here);
    // another app scheme, Tauri's own origin, a file, a data document: nowhere
    assert_eq!(goes(&url("http://tauri.localhost/"), true), Goes::Nowhere);
    assert_eq!(goes(&url("file:///data/local"), true), Goes::Nowhere);
    assert_eq!(goes(&url("data:text/html,hi"), true), Goes::Nowhere);
    assert_eq!(
        goes(&url("intent://scan/#Intent;scheme=zxing;end"), true),
        Goes::Nowhere
    );
}

#[test]
fn a_server_page_opens_at_a_plain_path_only() {
    assert_eq!(server_path(" /qapps/system "), Ok("/qapps/system"));
    for bad in ["qapps", "//evil.example/x", "/a\\b", "/a\nb", "https://evil.example/"] {
        assert!(server_path(bad).is_err(), "{bad}");
    }
}
