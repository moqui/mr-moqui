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

//! The incoming call on a locked phone (plans/Mobile-Desktop.md MD6, MT2), the part that is Rust's: the native code
//! (`gen/android/.../CallManager.kt`) shows the ring and the WebView carries the media.
//!
//! - **Answer** comes back as the deep link `org.moqui.mobile:/call?callSessionId=...&video=...` (an intent of the
//!   activity, `RunEvent::Opened`): the shell is shown at the call's screen of the current account's comms app, with
//!   `answer=true`, so the page answers and joins through WebrootCall.js ([`call_path`]).
//! - **Decline** is the server's `declineCall` transition, posted with the device key without the WebView
//!   ([`decline`]). A ring does not say which account it is for, so each account with a key is asked, the current one
//!   first, until one's server knows the call.
//! - **Push registration:** the FCM token, kept in `push.json` of the client's directory with the accounts it is
//!   registered for, goes to each account's server (`/qapps/registerClientPush`) once per token, at a sign-in, and is
//!   taken back (`unregisterClientPush`) before a sign-out or a removal revokes the key ([`PushRecord`]).
//!
//! The requests go through a [`Poster`], which is the client's proxy in the app (`crate::native`), so this module runs
//! in the tests with no network.

// Declines and registrations are the native code's, on Android; they are tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::path::{Path, PathBuf};

use moqui_client::account::Account;
use serde_json::Value;

/// The deep link's path an Answer opens the app with.
pub(crate) const CALL_LINK_PATH: &str = "/call";

/// The longest call id taken from a link or a push.
const ID_MAX: usize = 64;

/// A call id as the server makes them: letters, digits, `-` and `_`, at most [`ID_MAX`].
pub(crate) fn call_id(text: &str) -> Option<&str> {
    let text = text.trim();
    let plain = !text.is_empty()
        && text.len() <= ID_MAX
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    plain.then_some(text)
}

/// What a call's deep link asks: the call answered (whether with the camera), or hung up (the call notification's Hang
/// up).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) call_session_id: String,
    pub(crate) video: bool,
    pub(crate) hang_up: bool,
}

/// The Answer of an `org.moqui.mobile:/call?...` link; none for any other link or a bad call id.
pub(crate) fn answer_link(url: &tauri::Url) -> Option<Answer> {
    if url.scheme() != "org.moqui.mobile" || url.path() != CALL_LINK_PATH {
        return None;
    }
    let mut id = None;
    let mut video = true;
    let mut hang_up = false;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "callSessionId" => id = call_id(&value).map(str::to_string),
            "video" => video = value != "false",
            "hangUp" => hang_up = value == "true",
            _ => {}
        }
    }
    Some(Answer {
        call_session_id: id?,
        video,
        hang_up,
    })
}

/// What the shell runs to leave the call `id` it is in (the call notification's Hang up).
pub(crate) fn hang_up_script(id: &str) -> String {
    let quoted = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    format!("window.moqui && moqui.call && moqui.call.state.current === {quoted} && moqui.call.leave()")
}

/// The call's screen under the comms app at `root` (`/qapps/BasicComms`), answered by the page as it opens.
pub(crate) fn call_path(root: &str, answer: &Answer) -> String {
    let mut path = format!(
        "{}/Calls/Call?callSessionId={}&answer=true",
        root.trim_end_matches('/'),
        answer.call_session_id
    );
    if !answer.video {
        path.push_str("&video=false");
    }
    path
}

/// The path of a call transition under the comms app at `root`, as WebrootCall.js's `base()` makes it:
/// `/qapps/BasicComms` → `/apps/BasicComms/Calls/<name>`.
pub(crate) fn transition_path(root: &str, name: &str) -> String {
    let root = root.trim_end_matches('/');
    let rest = root.strip_prefix("/qapps").unwrap_or(root);
    format!("/apps{rest}/Calls/{name}")
}

/// Posts a form with an account's key to its server, answering the status and the JSON (`null` when not JSON).
pub(crate) trait Poster {
    fn post(&self, account: &Account, key: &str, path: &str, form: &[(&str, &str)]) -> Result<(u16, Value), String>;
}

/// An account that may be asked: its key and its comms app.
pub(crate) struct Asked {
    pub(crate) account: Account,
    pub(crate) key: String,
    pub(crate) comms_root: String,
}

/// How a decline went.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Declined {
    /// The server of this account was told (or the call was over there already).
    Told(String),
    /// No account's server knows the call.
    NotFound,
    /// No server could be told: why, for each account.
    Failed(Vec<String>),
}

/// Decline the call `id` on the first of `accounts` whose server knows it. A server that is down or refuses is noted
/// and the next account asked.
pub(crate) fn decline(accounts: &[Asked], id: &str, poster: &dyn Poster) -> Declined {
    let mut failures = Vec::new();
    for asked in accounts {
        let path = transition_path(&asked.comms_root, "declineCall");
        match poster.post(&asked.account, &asked.key, &path, &[("callSessionId", id)]) {
            Ok((status, json)) if status < 400 => match json["reasonCode"].as_str() {
                // not this account's call: the next one
                Some("notFound") => {}
                // declined, ended already, or no longer ringing for this user (answered elsewhere): the server knows
                _ => return Declined::Told(asked.account.id.clone()),
            },
            Ok((status, _)) => failures.push(format!("{}: HTTP {status}", asked.account.id)),
            Err(err) => failures.push(format!("{}: {err}", asked.account.id)),
        }
    }
    if failures.is_empty() {
        Declined::NotFound
    } else {
        Declined::Failed(failures)
    }
}

/// The push token and the accounts it is registered for, in `push.json` of the client's directory.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PushRecord {
    #[serde(default)]
    pub(crate) token: Option<String>,
    #[serde(default)]
    pub(crate) registered: Vec<String>,
}

impl PushRecord {
    fn file(dir: &Path) -> PathBuf {
        dir.join("push.json")
    }

    /// The record of `dir`, or an empty one.
    pub(crate) fn load(dir: &Path) -> Self {
        std::fs::read(Self::file(dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Keep the record, owner-only.
    pub(crate) fn save(&self, dir: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|err| err.to_string())?;
        std::fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        let path = Self::file(dir);
        let temp = path.with_extension("json.part");
        std::fs::write(&temp, bytes).map_err(|err| format!("{}: {err}", temp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, &path).map_err(|err| format!("{}: {err}", path.display()))
    }

    /// A token from Firebase: a new one is registered with no account yet. Whether it changed.
    pub(crate) fn set_token(&mut self, token: &str) -> bool {
        if self.token.as_deref() == Some(token) {
            return false;
        }
        self.token = Some(token.to_string());
        self.registered.clear();
        true
    }

    /// Register the token with each of `accounts` it is not registered with yet; one that fails is logged and tried
    /// again at the next start, sign-in or token. Whether the record changed.
    pub(crate) fn register(&mut self, accounts: &[Asked], poster: &dyn Poster) -> bool {
        let Some(token) = self.token.clone() else {
            return false;
        };
        let mut changed = false;
        for asked in accounts {
            if self.registered.contains(&asked.account.id) {
                continue;
            }
            let form = [
                ("provider", "fcm"),
                ("token", token.as_str()),
                ("appId", "org.moqui.mobile"),
                ("platform", "android"),
            ];
            match poster.post(&asked.account, &asked.key, "/qapps/registerClientPush", &form) {
                Ok((status, json)) if status < 400 && json["registered"].as_bool() == Some(true) => {
                    self.registered.push(asked.account.id.clone());
                    changed = true;
                    log::info!("Push: registered with {}", asked.account.id);
                }
                // the token is a capability: never logged
                Ok((status, json)) => log::warn!(
                    "Push: {} did not register this device: HTTP {status} {}",
                    asked.account.id,
                    json["errors"].as_str().unwrap_or("")
                ),
                Err(err) => log::warn!("Push: {} was not reached to register: {err}", asked.account.id),
            }
        }
        changed
    }

    /// The account signs in again (a new key): registered again at the next [`Self::register`].
    pub(crate) fn forget(&mut self, account: &str) -> bool {
        let before = self.registered.len();
        self.registered.retain(|id| id != account);
        before != self.registered.len()
    }

    /// Take the token back from `asked`'s server before its key is revoked (which would delete it there too); a failure
    /// is logged. Whether the record changed.
    pub(crate) fn unregister(&mut self, asked: &Asked, poster: &dyn Poster) -> bool {
        let Some(token) = self.token.clone() else {
            return self.forget(&asked.account.id);
        };
        match poster.post(
            &asked.account,
            &asked.key,
            "/qapps/unregisterClientPush",
            &[("token", token.as_str())],
        ) {
            Ok((status, _)) if status < 400 => log::info!("Push: unregistered from {}", asked.account.id),
            Ok((status, _)) => log::warn!(
                "Push: {} did not unregister this device: HTTP {status}",
                asked.account.id
            ),
            Err(err) => log::warn!("Push: {} was not reached to unregister: {err}", asked.account.id),
        }
        self.forget(&asked.account.id)
    }
}

#[cfg(test)]
#[path = "calls_tests.rs"]
mod tests;
