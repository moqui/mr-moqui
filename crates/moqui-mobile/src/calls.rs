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
//! - **Which account.** The server's push names the registration it went to (`pushId`, the `subscriptionId` its
//!   `registerClientPush` answered), kept per account in [`PushRecord`]: a ring is that account's ([`ring_account`]),
//!   and one that is no account's here is dropped.
//! - **Answer and Hang up** come back as the deep link `org.moqui.mobile:/call?callSessionId=...&nonce=...` (an intent
//!   of the activity, `RunEvent::Opened`). The activity is exported, so any app could send such a link: only one whose
//!   nonce the native code had made for that call ([`CallLinks`], once, within [`LINK_MS`]) is taken. An answered one
//!   shows the shell at the call's screen of the ring's account ([`call_path`]), where the page answers and joins
//!   through WebrootCall.js once the host hands it the answer.
//! - **Decline** is the server's `declineCall` transition, posted with the ring's account's device key without the
//!   WebView ([`decline`]); only that account's server is told of the call.
//! - **Push registration:** the FCM token, kept in `push.json` of the client's directory with each account's
//!   registration, goes to each account's server once per token, at a sign-in, and is taken back before a sign-out or a
//!   removal revokes the key.
//!
//! The requests go through a [`Poster`], which is the client's proxy in the app (`crate::native`), so this module runs
//! in the tests with no network.

// Declines and registrations are the native code's, on Android; they are tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use moqui_client::account::Account;
use serde_json::Value;

/// The deep link's path an Answer or a Hang up opens the app with.
pub(crate) const CALL_LINK_PATH: &str = "/call";

/// How long a link the native code made may be taken, in milliseconds: an Answer opens the app at once.
pub(crate) const LINK_MS: i64 = 120_000;

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

/// What a link the native code made does: answer the call of `account` (with the camera or not), or hang it up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallLink {
    pub(crate) call_session_id: String,
    pub(crate) account: String,
    pub(crate) video: bool,
    pub(crate) hang_up: bool,
}

/// The links the native code made and not yet taken, by nonce.
#[derive(Default)]
pub(crate) struct CallLinks {
    made: HashMap<String, (CallLink, i64)>,
}

impl CallLinks {
    /// A link made at `now` (Unix milliseconds): its nonce, which the native code puts in the link.
    pub(crate) fn make(&mut self, link: CallLink, now: i64) -> Result<String, String> {
        self.made.retain(|_, (_, made)| now - *made < LINK_MS);
        let nonce = moqui_client::store::new_client_ref().map_err(|err| err.to_string())?;
        self.made.insert(nonce.clone(), (link, now));
        Ok(nonce)
    }

    /// The link of `nonce` for the call `id`, taken once: none for a nonce not made, made for another call, taken
    /// already or too old.
    pub(crate) fn take(&mut self, nonce: &str, id: &str, now: i64) -> Option<CallLink> {
        let (link, made) = self.made.remove(nonce)?;
        (link.call_session_id == id && now - made < LINK_MS && now >= made).then_some(link)
    }
}

/// The call id and nonce of an `org.moqui.mobile:/call?...` link; none for any other link, a bad call id or no nonce.
pub(crate) fn link_of(url: &tauri::Url) -> Option<(String, String)> {
    if url.scheme() != "org.moqui.mobile" || url.path() != CALL_LINK_PATH {
        return None;
    }
    let mut id = None;
    let mut nonce = None;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "callSessionId" => id = call_id(&value).map(str::to_string),
            "nonce" if !value.is_empty() && value.len() <= 64 => nonce = Some(value.into_owned()),
            _ => {}
        }
    }
    Some((id?, nonce?))
}

/// Whether `url` is a call link at all (taken or refused, never another kind of link).
pub(crate) fn is_call_link(url: &tauri::Url) -> bool {
    url.scheme() == "org.moqui.mobile" && url.path() == CALL_LINK_PATH
}

/// What the shell runs to leave the call `id` it is in (the call notification's Hang up, a headset's).
pub(crate) fn hang_up_script(id: &str) -> String {
    let quoted = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    format!("window.moqui && moqui.call && moqui.call.state.current === {quoted} && moqui.call.leave()")
}

/// The call's screen under the comms app at `root` (`/qapps/BasicComms`), which answers when the host hands it the
/// answer (`call_take_answer`).
pub(crate) fn call_path(root: &str, link: &CallLink) -> String {
    let mut path = format!(
        "{}/Calls/Call?callSessionId={}&answer=true",
        root.trim_end_matches('/'),
        link.call_session_id
    );
    if !link.video {
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
    /// The server declined the call for the user (or it was over there already).
    Told,
    /// The server answered, and did not decline it: why (`notFound`, `notRinging`...).
    Refused(String),
    /// The server was not told: why.
    Failed(String),
}

/// Decline the call `id` on the server of `asked`, the ring's account.
pub(crate) fn decline(asked: &Asked, id: &str, poster: &dyn Poster) -> Declined {
    let path = transition_path(&asked.comms_root, "declineCall");
    match poster.post(&asked.account, &asked.key, &path, &[("callSessionId", id)]) {
        Ok((status, json)) if status < 400 => match json["reasonCode"].as_str() {
            None => Declined::Told,
            Some(reason) => Declined::Refused(reason.to_string()),
        },
        Ok((status, _)) => Declined::Failed(format!("HTTP {status}")),
        Err(err) => Declined::Failed(err),
    }
}

/// The push token and each account's registration of it (account id → `subscriptionId`), in `push.json` of the
/// client's directory.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PushRecord {
    #[serde(default)]
    pub(crate) token: Option<String>,
    #[serde(default)]
    pub(crate) registered: BTreeMap<String, String>,
}

impl PushRecord {
    fn file(dir: &Path) -> PathBuf {
        dir.join("push.json")
    }

    /// The record of `dir`, or an empty one (also for one of an earlier shape, which registers again).
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
    /// again at the next start, sign-in or token. A registration the server answers with no `subscriptionId` is not
    /// kept, as no push could be told apart for it. Whether the record changed.
    pub(crate) fn register(&mut self, accounts: &[Asked], poster: &dyn Poster) -> bool {
        let Some(token) = self.token.clone() else {
            return false;
        };
        let mut changed = false;
        for asked in accounts {
            if self.registered.contains_key(&asked.account.id) {
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
                    let Some(push_id) = json["subscriptionId"].as_str().filter(|id| !id.is_empty()) else {
                        log::warn!("Push: {} registered this device with no id", asked.account.id);
                        continue;
                    };
                    // the server keeps one user per token: another account of this server registered it before, and
                    // its pushes are no longer
                    let server = &asked.account.server;
                    let moved: Vec<String> = self
                        .registered
                        .iter()
                        .filter(|(id, registered)| {
                            *registered == push_id
                                && accounts
                                    .iter()
                                    .any(|other| &other.account.id == *id && &other.account.server == server)
                        })
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in moved {
                        self.registered.remove(&id);
                    }
                    self.registered.insert(asked.account.id.clone(), push_id.to_string());
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
        self.registered.remove(account).is_some()
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

/// The account a push with `push_id` is for: one registered here with it and signed in (in `keyed`), the current one
/// first should two servers have given the same id. None for a push that is no account's here.
pub(crate) fn ring_account(
    record: &PushRecord,
    push_id: &str,
    keyed: &[String],
    current: Option<&str>,
) -> Option<String> {
    if push_id.is_empty() {
        return None;
    }
    let mut matching: Vec<&String> = record
        .registered
        .iter()
        .filter(|(account, registered)| *registered == push_id && keyed.contains(account))
        .map(|(account, _)| account)
        .collect();
    matching.sort_by_key(|account| Some(account.as_str()) != current);
    matching.first().map(|account| (*account).clone())
}

#[cfg(test)]
#[path = "calls_tests.rs"]
mod tests;
