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

use super::*;

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

#[test]
fn each_window_stays_on_its_origin() {
    // a path the shell forwards is the server's page, never shown on the shell's origin
    assert_eq!(
        goes(
            &url("http://moqui-shell.localhost/apps/Comms/Chat/attachment?id=1"),
            SHELL_ORIGIN
        ),
        Goes::AppWindow
    );
    assert_eq!(
        on_app_origin(&url("http://moqui-shell.localhost/qapps/Comms?x=1")).as_str(),
        "http://moqui-app.localhost/qapps/Comms?x=1"
    );
    // the shell
    assert_eq!(
        goes(&url("http://moqui-shell.localhost/?signIn=1"), SHELL_ORIGIN),
        Goes::Here
    );
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/Comms/Chat"), SHELL_ORIGIN),
        Goes::AppWindow
    );
    assert_eq!(goes(&url("https://www.example.com/"), SHELL_ORIGIN), Goes::Outside);
    assert_eq!(goes(&url("mailto:a@example.com"), SHELL_ORIGIN), Goes::Outside);
    assert_eq!(goes(&url("file:///etc/passwd"), SHELL_ORIGIN), Goes::Nowhere);
    // an app window: the server's pages; never the shell, whose origin has the shell's commands
    assert_eq!(
        goes(&url("http://moqui-app.localhost/qapps/Comms/Chat"), APP_ORIGIN),
        Goes::Here
    );
    assert_eq!(goes(&url("http://moqui-shell.localhost/"), APP_ORIGIN), Goes::Nowhere);
    assert_eq!(
        goes(&url("https://accounts.example.org/oauth"), APP_ORIGIN),
        Goes::Outside
    );
    assert_eq!(goes(&url("about:blank"), APP_ORIGIN), Goes::Here);
    // a node's loopback port is the web to a client
    assert_eq!(goes(&url("http://127.0.0.1:8080/qapps"), SHELL_ORIGIN), Goes::Outside);
}

#[test]
fn the_shells_url_takes_a_query() {
    assert_eq!(shell_url(None).as_str(), "http://moqui-shell.localhost/");
    assert_eq!(
        shell_url(Some("signIn=1")).as_str(),
        "http://moqui-shell.localhost/?signIn=1"
    );
}
