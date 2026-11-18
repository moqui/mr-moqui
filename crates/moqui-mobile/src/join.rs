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

//! A chat room's invite link opened in the app (plans/Comms-Messaging.md §18.3 phase 17): the join page
//! (`basic-comms/screen/ChatJoin.qvue`, `/join/<token>`) offers "Open in the MR Moqui app" on Android, the link
//! `org.moqui.mobile:/join?host=<host>&token=<token>` (a `BROWSABLE` intent filter, `RunEvent::Opened`).
//!
//! A web page makes the link, so it is not signed as a room link is, and it only opens a page: the app opens
//! `/join/<token>` in its window of an account on that host (the current one when it is there, else the first), where the
//! person presses Join. With no account on the host it says so and adds none. Nothing the link brings is done by itself.

// The link is Android's; it is tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use moqui_client::account::Account;

/// The deep link's path the join page opens the app with.
pub(crate) const JOIN_LINK_PATH: &str = "/join";

/// The longest host (with its port) taken from a link.
const HOST_MAX: usize = 260;

/// Whether `url` is a join link at all (taken or refused).
pub(crate) fn is_join_link(url: &tauri::Url) -> bool {
    url.scheme() == "org.moqui.mobile" && url.path() == JOIN_LINK_PATH
}

/// A host as a page's `location.host` gives it: letters, digits, `.` and `-`, and an optional `:port`.
fn host_of(text: &str) -> Option<String> {
    let text = text.trim().to_ascii_lowercase();
    let (name, port) = match text.rsplit_once(':') {
        Some((name, port)) => (name.to_string(), Some(port.to_string())),
        None => (text.clone(), None),
    };
    let name_ok = !name.is_empty()
        && text.len() <= HOST_MAX
        && !name.starts_with('.')
        && !name.starts_with('-')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
    let port_ok = port.as_ref().is_none_or(|port| {
        !port.is_empty()
            && port.len() <= 5
            && port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|p| p > 0 && p < 65536)
    });
    (name_ok && port_ok).then_some(text)
}

/// A link's token: the server's `randomString(32)`, 16 to 64 of letters, digits, `_` and `-`.
fn token_of(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() >= 16
        && text.len() <= 64
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
    .then(|| text.to_string())
}

/// The host and token of an `org.moqui.mobile:/join?host=...&token=...` link; none for any other link, or a host or a
/// token that is not one.
pub(crate) fn join_link_of(url: &tauri::Url) -> Option<(String, String)> {
    if !is_join_link(url) {
        return None;
    }
    let (mut host, mut token) = (None, None);
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "host" => host = host_of(&value),
            "token" => token = token_of(&value),
            _ => {}
        }
    }
    Some((host?, token?))
}

/// The host and port of an account's server (`https://host[:port][/path]`).
fn server_host(account: &Account) -> Option<String> {
    let url = tauri::Url::parse(&account.server).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// The account to open a join link of `host` in: the current one when it is on that host, else the first there; none
/// when no account is.
pub(crate) fn account_for(accounts: &[Account], current: Option<&str>, host: &str) -> Option<String> {
    let on_host: Vec<&Account> = accounts
        .iter()
        .filter(|account| server_host(account).as_deref() == Some(host))
        .collect();
    on_host
        .iter()
        .find(|account| Some(account.id.as_str()) == current)
        .or_else(|| on_host.first())
        .map(|account| account.id.clone())
}

/// The path of a join link's page on its server.
pub(crate) fn join_path(token: &str) -> String {
    format!("/join/{token}")
}

#[cfg(test)]
#[path = "join_tests.rs"]
mod tests;
