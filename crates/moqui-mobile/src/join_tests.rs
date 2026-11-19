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

const TOKEN: &str = "Abc_def-0123456789xyzABCDEFGHIJKL";

fn url(text: &str) -> tauri::Url {
    tauri::Url::parse(text).unwrap()
}

fn account(id: &str, server: &str) -> Account {
    Account {
        id: id.to_string(),
        server: server.to_string(),
        username: "ann".to_string(),
        user_id: "ANN".to_string(),
        display: "Ann".to_string(),
        server_name: String::new(),
        device_mode: None,
    }
}

#[test]
fn a_join_link_takes_a_host_and_a_token() {
    let link = url(&format!("org.moqui.mobile:/join?host=Chat.Example.com&token={TOKEN}"));
    assert!(is_join_link(&link));
    assert_eq!(
        join_link_of(&link),
        Some(("chat.example.com".to_string(), TOKEN.to_string()))
    );
    let with_port = url(&format!("org.moqui.mobile:/join?host=localhost:8443&token={TOKEN}"));
    assert_eq!(
        join_link_of(&with_port).map(|(host, _)| host).as_deref(),
        Some("localhost:8443")
    );
    assert_eq!(join_path(TOKEN), format!("/join/{TOKEN}"));
}

#[test]
fn a_join_link_refuses_another_path_scheme_host_or_token() {
    for text in [
        format!("org.moqui.mobile:/room?host=chat.example.com&token={TOKEN}"),
        format!("https://chat.example.com/join?host=chat.example.com&token={TOKEN}"),
        format!("org.moqui.mobile:/join?host=evil.example.com/path&token={TOKEN}"),
        format!("org.moqui.mobile:/join?host=..example.com&token={TOKEN}"),
        format!("org.moqui.mobile:/join?host=chat.example.com:99999&token={TOKEN}"),
        format!("org.moqui.mobile:/join?host=chat.example.com:&token={TOKEN}"),
        format!("org.moqui.mobile:/join?token={TOKEN}"),
        "org.moqui.mobile:/join?host=chat.example.com&token=short".to_string(),
        "org.moqui.mobile:/join?host=chat.example.com&token=has%20a%20space%20in%20it%20here".to_string(),
        "org.moqui.mobile:/join?host=chat.example.com".to_string(),
    ] {
        assert_eq!(join_link_of(&url(&text)), None, "{text}");
    }
}

#[test]
fn a_join_link_opens_in_the_current_account_on_its_host_else_the_first_and_none_elsewhere() {
    let accounts = [
        account("other-1", "https://other.example.com"),
        account("chat-1", "https://chat.example.com"),
        account("chat-2", "https://chat.example.com"),
        account("dev-1", "http://localhost:8443"),
    ];
    assert_eq!(
        account_for(&accounts, Some("chat-2"), "chat.example.com").as_deref(),
        Some("chat-2")
    );
    assert_eq!(
        account_for(&accounts, Some("other-1"), "chat.example.com").as_deref(),
        Some("chat-1")
    );
    assert_eq!(account_for(&accounts, None, "localhost:8443").as_deref(), Some("dev-1"));
    assert_eq!(account_for(&accounts, None, "localhost"), None);
    assert_eq!(account_for(&accounts, None, "nowhere.example.com"), None);
}

/// An account's server by its host and port, as a join link names it and as its server pages' dialogs and notifications
/// are titled (SEC-CLI-119): no scheme, path or default port.
#[test]
fn an_accounts_server_is_named_by_its_host_and_port() {
    let named = |server: &str| server_host(&account("a", server));
    assert_eq!(
        named("https://Chat.Example.com/moqui").as_deref(),
        Some("chat.example.com")
    );
    assert_eq!(
        named("https://chat.example.com:443").as_deref(),
        Some("chat.example.com")
    );
    assert_eq!(named("http://localhost:8080").as_deref(), Some("localhost:8080"));
    assert_eq!(named("not a server"), None);
}

/// SEC-CLI-99: any web page can open a join link, so it never switches the account: the join page opens in the current
/// account when it is on the link's host; with another account here on the host the page says so (the person switches
/// from the account menu), and with none, that there is none. The host does what this decides and switches nothing.
#[test]
fn a_join_link_opens_in_the_current_account_only_and_never_switches_to_another() {
    let accounts = [
        account("other-1", "https://other.example.com"),
        account("chat-1", "https://chat.example.com"),
        account("chat-2", "https://chat.example.com"),
    ];
    assert_eq!(
        join_action(&accounts, Some("chat-2"), "chat.example.com"),
        JoinAction::Open
    );
    assert_eq!(
        join_action(&accounts, Some("chat-1"), "chat.example.com"),
        JoinAction::Open
    );
    assert_eq!(
        join_action(&accounts, Some("other-1"), "chat.example.com"),
        JoinAction::OtherAccount
    );
    assert_eq!(
        join_action(&accounts, None, "chat.example.com"),
        JoinAction::OtherAccount
    );
    assert_eq!(
        join_action(&accounts, Some("other-1"), "nowhere.example.com"),
        JoinAction::NoAccount
    );
    let host = include_str!("host.rs");
    let opened = host
        .split("fn open_join_link(")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("MobileHost::open_join_link");
    assert!(opened.contains("join_action("), "{opened}");
    assert!(
        !opened.contains("switch_account"),
        "a join link switches the account: {opened}"
    );
}
