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
fn an_app_window_has_the_outbox_but_none_of_the_shells_commands() {
    for permission in ["allow-outbox-add", "allow-draft-put", "allow-file-save"] {
        assert!(APP_PERMISSIONS.contains(&permission), "{permission}");
        assert!(SHELL_PERMISSIONS.contains(&permission), "{permission}");
    }
    assert!(APP_PERMISSIONS
        .iter()
        .all(|permission| !permission.starts_with("allow-client-")));
    // what a shared sign-out would delete is the shell's to ask
    assert!(!APP_PERMISSIONS.contains(&"allow-outbox-unsent"));
}

#[test]
fn the_comms_screens_the_shell_bundles_are_shown_there_and_the_rest_in_an_app_window() {
    for path in [
        "/qapps/BasicComms/Chat",
        "/qapps/BasicComms/Chat?chatRoomId=R1&eventSeq=4",
        "/qapps/BasicComms/Inbox",
        "/qapps/BasicComms/Inbox/Mail?thread=email:T1",
        "/qapps/BasicComms/Inbox/Compose?to=a%40example.com&subject=Hi",
        "/apps/BasicComms/Chat/",
        "/apps/BasicComms/Chat.qvue",
        "/apps/BasicComms/Inbox/Compose.qvt?mode=dock",
        "/qapps/BasicComms/Inbox/Mail.qjs",
    ] {
        assert!(shown_in_shell(path, "/qapps/BasicComms"), "{path}");
    }
    // an app that mounts them under its own root (BOSS's Comms): there, not under BasicComms
    assert!(shown_in_shell("/qapps/Comms/Inbox/Compose?to=x", "/qapps/Comms"));
    assert!(shown_in_shell("/qapps/Comms/Chat", "/qapps/Comms/"));
    assert!(!shown_in_shell("/qapps/BasicComms/Chat", "/qapps/Comms"));
    for path in [
        "/qapps/BasicComms",
        "/qapps/BasicComms/Calls/Dialer?number=1",
        "/qapps/BasicComms/Inbox/MessageDetail?emailMessageId=1",
        "/qapps/BasicComms/Chat/attachment?chatAttachmentId=1",
        "/qapps/BasicComms/Settings",
        "/qapps/BasicCommsX/Chat",
        "/qapps/Other/Chat",
        "/BasicComms/Chat",
        "",
    ] {
        assert!(!shown_in_shell(path, "/qapps/BasicComms"), "{path}");
    }
    assert!(!shown_in_shell("/qapps/BasicComms/Chat", ""));
}

#[test]
fn a_comms_screen_is_kept_for_the_shell_also_signed_out_and_the_rest_opens_in_an_app_window() {
    let compose = "/qapps/BasicComms/Inbox/Compose?to=a%40example.com";
    // signed in: kept and the shell poked to take it now
    assert_eq!(
        page_goes(compose, "/qapps/BasicComms", true),
        PageGoes::Shell { poke: true }
    );
    // signed out (or no account): kept for the shell that mounts after the sign-in, not lost
    assert_eq!(
        page_goes(compose, "/qapps/BasicComms", false),
        PageGoes::Shell { poke: false }
    );
    assert_eq!(
        page_goes("/qapps/BasicComms/Calls/Dialer?number=1", "/qapps/BasicComms", true),
        PageGoes::AppWindow
    );
    assert_eq!(
        page_goes("/qapps/BasicComms/Calls/Dialer?number=1", "/qapps/BasicComms", false),
        PageGoes::AppWindow
    );
    assert_eq!(page_goes(compose, "/qapps/Comms", true), PageGoes::AppWindow);
    assert_eq!(SHELL_POKE, "window.moquiShell && window.moquiShell.poke()");
}

#[test]
fn an_app_window_may_have_the_shell_show_a_comms_screen_of_its_own_current_account_only() {
    let room = "/qapps/Comms/Chat?chatRoomId=R1";
    assert_eq!(shell_path_for(room, "/qapps/Comms", Some("a1"), Some("a1")), Ok(()));
    // the shell (no app window of an account), another account's window, or no account current: refused
    assert!(shell_path_for(room, "/qapps/Comms", None, Some("a1")).is_err());
    let other = shell_path_for(room, "/qapps/Comms", Some("a2"), Some("a1")).unwrap_err();
    assert!(other.contains("switch to it"), "{other}");
    assert!(shell_path_for(room, "/qapps/Comms", Some("a1"), None).is_err());
    // not a screen the shell shows, or under another comms app than the window's account's
    assert!(shell_path_for("/qapps/Comms/Settings", "/qapps/Comms", Some("a1"), Some("a1")).is_err());
    assert!(shell_path_for("/qapps/system/Users", "/qapps/Comms", Some("a1"), Some("a1")).is_err());
    assert!(shell_path_for("/qapps/BasicComms/Chat", "/qapps/Comms", Some("a1"), Some("a1")).is_err());
    assert!(shell_path_for(
        "https://elsewhere.example.com/qapps/Comms/Chat",
        "/qapps/Comms",
        Some("a1"),
        Some("a1")
    )
    .is_err());
}

#[test]
fn only_app_windows_open_a_screen_in_the_shell_and_leave_it_the_crypto_device() {
    assert!(APP_PERMISSIONS.contains(&"allow-open-in-shell"));
    assert!(!SHELL_PERMISSIONS.contains(&"allow-open-in-shell"));
    let script = app_window_script("a1", &["gone".to_string()]).unwrap();
    assert!(script.starts_with("window.__moquiStorageScope = \"a1\";\nwindow.__moquiClearScopes = [\"gone\"];\nwindow.__moquiCryptoInShell = true;\n"));
    assert!(script.ends_with(crate::bridge::SCRIPT));
    let shell = shell_script("MR Moqui desktop on \"desk\"");
    assert!(shell.starts_with("window.__moquiDeviceName = \"MR Moqui desktop on \\\"desk\\\"\";\n"));
    assert!(!shell.contains("__moquiCryptoInShell = true"));
}
