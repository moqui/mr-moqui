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
//!   `registerClientPush` answered), kept per account in [`PushRecord`]; two servers may give the same id, so a sealed
//!   push is the account's whose own push keys open it (`native::open_push`), an unsealed ring only the one account
//!   its id can be ([`ring_account`]), and one that is no account's here is dropped.
//! - **Answer and Hang up** come back as the deep link `org.moqui.mobile:/call?callSessionId=...&nonce=...` (an intent
//!   of the activity, `RunEvent::Opened`). The activity is exported, so any app could send such a link: only one whose
//!   nonce the native code had made for that call ([`CallLinks`], once, within [`LINK_MS`]) is taken. An answered one
//!   shows the shell at the call's screen of the ring's account ([`call_path`]), where the page answers and joins
//!   through WebrootCall.js once the host hands it the answer.
//! - **Decline** is the server's `declineCall` transition, posted with the ring's account's device key without the
//!   WebView ([`decline`]); only that account's server is told of the call.
//! - **Push registration:** the FCM token (or an account's UnifiedPush endpoint), kept in `push.json` of the client's
//!   directory with each account's registration and push keys, goes to each account's server once per token, at a
//!   sign-in, and is taken back before a sign-out or a removal revokes the key.
//!
//! The requests go through a [`Poster`], which is the client's proxy in the app (`crate::native`), so this module runs
//! in the tests with no network.

// Declines and registrations are the native code's, on Android; they are tested on the desktop.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// The deep link's path a chat notification's tap opens the app with (`ChatNotifications.kt`).
pub(crate) const ROOM_LINK_PATH: &str = "/room";

/// The longest account id taken from a link or the native code.
const ACCOUNT_MAX: usize = 300;

/// An account id as `moqui_client::account::account_id` makes them: a host's letters, digits, `.` and `-`, a `-` and a
/// hex digest (`chat.example.com-0123456789ab`). The native code's account is still looked up among the accounts.
pub(crate) fn account_id(text: &str) -> Option<&str> {
    let text = text.trim();
    let plain = !text.is_empty()
        && text.len() <= ACCOUNT_MAX
        && !text.starts_with('.')
        && !text.contains("..")
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'));
    plain.then_some(text)
}

/// The signature of what a link the app makes for itself opens (a room link, a page notification's tap): an HMAC of
/// `text` with the app's own link key (`native::link_key`), base64url, so one another app sends opens nothing. Each
/// kind's `text` starts with its own name (`room`, `notify`), so one kind's signature is never another's.
pub(crate) fn link_signature(key: &[u8], text: &str) -> String {
    use base64::Engine;
    use hmac::{KeyInit as _, Mac as _};
    let Ok(mut mac) = hmac::Hmac::<sha2::Sha256>::new_from_slice(key) else {
        return String::new();
    };
    mac.update(text.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&mac.finalize().into_bytes()[..18])
}

/// Whether `given` is the signature `expected` (never an empty one), compared in constant time.
pub(crate) fn signature_matches(expected: &str, given: &str) -> bool {
    !expected.is_empty()
        && expected.len() == given.len()
        && expected
            .bytes()
            .zip(given.bytes())
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// The signature of a room link.
fn room_link_signature(key: &[u8], account: &str, room: &str, seq: Option<i64>) -> String {
    link_signature(key, &format!("room\n{account}\n{room}\n{}", seq.unwrap_or(0)))
}

/// The signature of a page notification's tap (`commands::notify`): of the account it was shown for and the server path
/// it opens, so a tap's intent another app makes up opens no page (plans/Security-Review-Low.md SEC-CLI-88).
pub(crate) fn notify_signature(key: &[u8], account: &str, path: &str) -> String {
    link_signature(key, &format!("notify\n{account}\n{path}"))
}

/// The link a chat notification's tap opens the app with, signed with the app's link `key`: none for an id that is not
/// one.
pub(crate) fn room_link(key: &[u8], account: &str, room: &str, seq: Option<i64>) -> Option<String> {
    let account = account_id(account)?;
    let room = call_id(room)?;
    let seq = seq.filter(|seq| *seq > 0);
    let mut link = format!("org.moqui.mobile:/room?account={account}&chatRoomId={room}");
    if let Some(seq) = seq {
        link.push_str(&format!("&eventSeq={seq}"));
    }
    link.push_str("&sig=");
    link.push_str(&room_link_signature(key, account, room, seq));
    Some(link)
}

/// The account, room and message of an `org.moqui.mobile:/room?account=...&chatRoomId=...&eventSeq=...&sig=...` link
/// the app made (its signature checks with the app's link `key`): none for any other link, an id that is not one, or a
/// link not signed by this app. The activity is exported, so any app may send one; only the app's own open a room.
pub(crate) fn room_link_of(url: &tauri::Url, key: &[u8]) -> Option<(String, String, Option<i64>)> {
    if url.scheme() != "org.moqui.mobile" || url.path() != ROOM_LINK_PATH {
        return None;
    }
    let (mut account, mut room, mut seq, mut sig) = (None, None, None, None);
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "account" => account = account_id(&value).map(str::to_string),
            "chatRoomId" => room = call_id(&value).map(str::to_string),
            "eventSeq" => seq = value.parse::<i64>().ok().filter(|seq| *seq > 0),
            "sig" => sig = Some(value.into_owned()),
            _ => {}
        }
    }
    let (account, room, sig) = (account?, room?, sig?);
    if !signature_matches(&room_link_signature(key, &account, &room, seq), &sig) {
        log::warn!("A room link not made by this app was refused");
        return None;
    }
    Some((account, room, seq))
}

/// Whether `url` is a room link at all (taken or refused).
pub(crate) fn is_room_link(url: &tauri::Url) -> bool {
    url.scheme() == "org.moqui.mobile" && url.path() == ROOM_LINK_PATH
}

/// Whether `url` is a call link at all (taken or refused, never another kind of link).
pub(crate) fn is_call_link(url: &tauri::Url) -> bool {
    url.scheme() == "org.moqui.mobile" && url.path() == CALL_LINK_PATH
}

/// What the shell runs when the native call tells it something (`Native.tellPage`): `audio` (its route and routes,
/// JSON), `pip` (picture-in-picture, `true` or `false`), `hold` (`{callSessionId, hold}`: Telecom holds or resumes the
/// call), `contacts` (the phone's contacts became readable: `moqui.chat.contactsChanged`). None for anything else, or a
/// value that is not JSON of the right shape.
pub(crate) fn tell_script(what: &str, json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    let (function, args) = match what {
        "audio" if value.is_object() => ("setHostAudio", value.to_string()),
        "pip" if value.is_boolean() => ("setHostPip", value.to_string()),
        "mute" if value.is_boolean() => ("setHostMute", value.to_string()),
        // picture-in-picture's Mute: the page's microphone on or off
        "muteToggle" if value.is_boolean() => ("toggleMute", String::new()),
        "hold" => {
            let id = call_id(value["callSessionId"].as_str()?)?;
            let hold = value["hold"].as_bool()?;
            ("hostHold", format!("{}, {hold}", serde_json::to_string(id).ok()?))
        }
        // the phone's contacts became readable: the phone rooms' names are asked of the phone again
        "contacts" if value.is_boolean() => {
            return Some(
                "window.moqui && moqui.chat && typeof moqui.chat.contactsChanged === \"function\" && moqui.chat.contactsChanged()"
                    .to_string(),
            )
        }
        _ => return None,
    };
    Some(format!(
        "window.moqui && moqui.call && typeof moqui.call.{function} === \"function\" && moqui.call.{function}({args})"
    ))
}

/// What the shell runs to leave the call `id` it is in (the call notification's Hang up, a headset's).
pub(crate) fn hang_up_script(id: &str) -> String {
    let quoted = serde_json::to_string(id).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "window.moqui && moqui.call && (moqui.call.state.current === {quoted} ? moqui.call.leave() : \
         moqui.call.state.held && moqui.call.state.held.callSessionId === {quoted} && moqui.call.leaveHeld())"
    )
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
/// client's directory. An account registers with its own push keys ([`Self::keys`], `crate::push`), and with its own
/// UnifiedPush distributor endpoint where it has one ([`Self::endpoints`], given to its [`Self::instances`]), else with
/// the app's FCM token.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PushRecord {
    /// The app's FCM token.
    #[serde(default)]
    pub(crate) token: Option<String>,
    #[serde(default)]
    pub(crate) registered: BTreeMap<String, String>,
    /// Each account's push public key and auth secret its registration was made with: other ones register again.
    #[serde(default)]
    pub(crate) keys: BTreeMap<String, (String, String)>,
    /// Each account's UnifiedPush distributor endpoint (the one its instance was given): it registers with it in place
    /// of the FCM token.
    #[serde(default)]
    pub(crate) endpoints: BTreeMap<String, String>,
    /// Each account's UnifiedPush instance, the name the distributor app knows it by ([`Self::instance_of`]): random,
    /// made at its first registration and dropped when it signs out. Not the account id, which is the server's host and
    /// a hash of the server and username that the distributor could test usernames against (SEC-CLI-125).
    #[serde(default)]
    pub(crate) instances: BTreeMap<String, String>,
    /// The accounts whose FCM registration another account of the same server took (the server keeps one user per
    /// token): not registered, and not tried again, until the token, keys or endpoint change or the account signs in
    /// again (else the two would take the token from each other for ever).
    #[serde(default)]
    pub(crate) taken: BTreeSet<String>,
}

/// A new UnifiedPush instance: 16 random bytes, as 32 lower-case hex digits.
fn new_instance() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|err| format!("no random bytes: {err}"))?;
    let mut hex = String::with_capacity(32);
    for byte in bytes {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        hex.push(char::from_digit(u32::from(byte & 0xf), 16).unwrap_or('0'));
    }
    Ok(hex)
}

/// What an account registers with: the provider and its token.
fn registration_of<'a>(record: &'a PushRecord, account: &str) -> Option<(&'static str, &'a str)> {
    match record.endpoints.get(account) {
        Some(endpoint) => Some(("unifiedpush", endpoint.as_str())),
        None => record.token.as_deref().map(|token| ("fcm", token)),
    }
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

    /// A token from Firebase: a new one is registered again by every account that registers with it (not one with a
    /// UnifiedPush endpoint). Whether it changed.
    pub(crate) fn set_token(&mut self, token: &str) -> bool {
        if self.token.as_deref() == Some(token) {
            return false;
        }
        self.token = Some(token.to_string());
        let endpoints = &self.endpoints;
        self.registered.retain(|account, _| endpoints.contains_key(account));
        self.taken.clear();
        true
    }

    /// No account is signed in here any more, and Firebase's token goes with its installation (`Push.stopFcm`,
    /// SEC-CLI-121): the token is forgotten, with what registered with it, so nothing registers a dead token and the
    /// next sign-in's new one registers (a UnifiedPush account's registration, with its own endpoint, stays). Whether
    /// the record changed.
    pub(crate) fn forget_token(&mut self) -> bool {
        let token = self.token.take().is_some();
        let before = self.registered.len();
        let endpoints = &self.endpoints;
        self.registered.retain(|account, _| endpoints.contains_key(account));
        let taken = !self.taken.is_empty();
        self.taken.clear();
        token || taken || self.registered.len() != before
    }

    /// `account`'s UnifiedPush endpoint (none: it goes back to FCM): another one registers it again. Whether it
    /// changed.
    pub(crate) fn set_endpoint(&mut self, account: &str, endpoint: Option<&str>) -> bool {
        let endpoint = endpoint.map(str::trim).filter(|endpoint| !endpoint.is_empty());
        if self.endpoints.get(account).map(String::as_str) == endpoint {
            return false;
        }
        match endpoint {
            Some(endpoint) => self.endpoints.insert(account.to_string(), endpoint.to_string()),
            None => self.endpoints.remove(account),
        };
        self.registered.remove(account);
        self.taken.remove(account);
        true
    }

    /// `account`'s UnifiedPush instance ([`Self::instances`]), made the first time it is asked for: the instance, and
    /// whether one was made (the record changed, and must be kept before the instance is given to the distributor).
    pub(crate) fn instance_of(&mut self, account: &str) -> Result<(String, bool), String> {
        if let Some(instance) = self.instances.get(account) {
            return Ok((instance.clone(), false));
        }
        let instance = new_instance()?;
        self.instances.insert(account.to_string(), instance.clone());
        Ok((instance, true))
    }

    /// The account whose UnifiedPush instance is `instance`; none for one not made here. An earlier build's instance
    /// was the account id, so it is none: its late events (an `onUnregistered` of the instance taken back) are ignored.
    pub(crate) fn account_of(&self, instance: &str) -> Option<&str> {
        if instance.is_empty() {
            return None;
        }
        self.instances
            .iter()
            .find(|(_, kept)| kept.as_str() == instance)
            .map(|(account, _)| account.as_str())
    }

    /// `account`'s push keys (public key, auth secret): other ones than its registration was made with register it
    /// again. Whether they changed.
    pub(crate) fn set_keys(&mut self, account: &str, public: &str, auth: &str) -> bool {
        if self
            .keys
            .get(account)
            .is_some_and(|(kept_public, kept_auth)| kept_public == public && kept_auth == auth)
        {
            return false;
        }
        self.keys
            .insert(account.to_string(), (public.to_string(), auth.to_string()));
        self.registered.remove(account);
        self.taken.remove(account);
        true
    }

    /// Whether an account of `accounts` has something to register with and is not registered yet (one to try again);
    /// not one whose registration another account of its server took ([`Self::taken`]).
    pub(crate) fn pending(&self, accounts: &[Asked]) -> bool {
        accounts.iter().any(|asked| {
            let id = &asked.account.id;
            !self.registered.contains_key(id) && !self.taken.contains(id) && registration_of(self, id).is_some()
        })
    }

    /// Register each of `accounts` not registered yet, with its endpoint or the FCM token and its own keys; one that
    /// fails is logged and tried again (`RegisterWorker.kt`). A registration the server answers with no
    /// `subscriptionId` is not kept, as no push could be told apart for it. Whether the record changed. The native
    /// code makes it on a copy with no lock held (the posts take time), and [`Self::merge`] takes the outcome.
    pub(crate) fn register(&mut self, accounts: &[Asked], poster: &dyn Poster) -> bool {
        let mut changed = false;
        for asked in accounts {
            let id = &asked.account.id;
            if self.registered.contains_key(id) || self.taken.contains(id) {
                continue;
            }
            let Some((provider, token)) = registration_of(self, id) else {
                continue;
            };
            let token = token.to_string();
            let mut form = vec![
                ("provider", provider),
                ("token", token.as_str()),
                ("appId", "org.moqui.mobile"),
                ("platform", "android"),
            ];
            if let Some((public, auth)) = self.keys.get(id) {
                form.push(("p256dhKey", public.as_str()));
                form.push(("authSecret", auth.as_str()));
            }
            match poster.post(&asked.account, &asked.key, "/qapps/registerClientPush", &form) {
                Ok((status, json)) if status < 400 && json["registered"].as_bool() == Some(true) => {
                    let Some(push_id) = json["subscriptionId"].as_str().filter(|id| !id.is_empty()) else {
                        log::warn!("Push: {id} registered this device with no id");
                        continue;
                    };
                    // the server keeps one user per FCM token: another account of this server registered it before,
                    // and its pushes are no longer
                    if provider == "fcm" {
                        let server = &asked.account.server;
                        let endpoints = &self.endpoints;
                        let moved: Vec<String> = self
                            .registered
                            .iter()
                            .filter(|(other_id, registered)| {
                                *registered == push_id
                                    && !endpoints.contains_key(*other_id)
                                    && accounts
                                        .iter()
                                        .any(|other| &other.account.id == *other_id && &other.account.server == server)
                            })
                            .map(|(other_id, _)| other_id.clone())
                            .collect();
                        for other_id in moved {
                            self.registered.remove(&other_id);
                            self.taken.insert(other_id);
                        }
                    }
                    self.registered.insert(id.clone(), push_id.to_string());
                    changed = true;
                    log::info!("Push: registered with {id} ({provider})");
                }
                // the token is a capability: never logged
                Ok((status, json)) => log::warn!(
                    "Push: {id} did not register this device: HTTP {status} {}",
                    json["errors"].as_str().unwrap_or("")
                ),
                Err(err) => log::warn!("Push: {id} was not reached to register: {err}"),
            }
        }
        changed
    }

    /// Take what `done` (a copy of `before` that [`Self::register`]ed) registered and let go, for each account whose
    /// token, endpoint and keys did not change here in the meantime. Whether the record changed.
    pub(crate) fn merge(&mut self, before: &PushRecord, done: &PushRecord) -> bool {
        let mut changed = false;
        for (account, push_id) in &done.registered {
            if before.registered.get(account) == Some(push_id) || self.registered.get(account) == Some(push_id) {
                continue;
            }
            let same = registration_of(self, account) == registration_of(before, account)
                && self.keys.get(account) == before.keys.get(account);
            if same {
                self.registered.insert(account.clone(), push_id.clone());
                changed = true;
            }
        }
        // the registrations `done` let go (another account of the server took the token)
        for account in before.registered.keys() {
            if !done.registered.contains_key(account) && self.registered.get(account) == before.registered.get(account)
            {
                self.registered.remove(account);
                changed = true;
            }
        }
        for account in done.taken.difference(&before.taken) {
            changed |= self.taken.insert(account.clone());
        }
        changed
    }

    /// The account signs in again (a new key): registered again at the next [`Self::register`].
    pub(crate) fn forget(&mut self, account: &str) -> bool {
        let taken = self.taken.remove(account);
        self.registered.remove(account).is_some() || taken
    }

    /// The account is gone here (signed out): its registration, keys, endpoint and UnifiedPush instance (a sign-in
    /// again gets a new one, so the distributor cannot tell it is the same account). Whether the record changed.
    pub(crate) fn drop_account(&mut self, account: &str) -> bool {
        let registered = self.registered.remove(account).is_some();
        let keys = self.keys.remove(account).is_some();
        let endpoint = self.endpoints.remove(account).is_some();
        let instance = self.instances.remove(account).is_some();
        let taken = self.taken.remove(account);
        registered || keys || endpoint || instance || taken
    }

    /// Take the registration back from `asked`'s server before its key is revoked (which would delete it there too); a
    /// failure is logged. Whether the record changed. The account's keys and endpoint go with it.
    pub(crate) fn unregister(&mut self, asked: &Asked, poster: &dyn Poster) -> bool {
        let Some((_, token)) = registration_of(self, &asked.account.id) else {
            return self.drop_account(&asked.account.id);
        };
        let token = token.to_string();
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
        self.drop_account(&asked.account.id)
    }
}

/// The accounts a push with `push_id` may be for: those registered here with it by the FCM token and signed in (in
/// `keyed`). Two servers may give the same id, so a sealed push is the one account's whose keys open it
/// (`native::open_push`), never the current account's by default.
pub(crate) fn push_accounts(record: &PushRecord, push_id: &str, keyed: &[String]) -> Vec<String> {
    if push_id.is_empty() {
        return Vec::new();
    }
    record
        .registered
        .iter()
        .filter(|(account, registered)| {
            *registered == push_id && keyed.contains(account) && !record.endpoints.contains_key(*account)
        })
        .map(|(account, _)| account.clone())
        .collect()
}

/// The account an unsealed ring or ring's end is for: the one account `push_id` can be that registered with no push
/// keys (an account with keys gets its rings sealed, so an unsealed one in its name is refused); none when two could.
pub(crate) fn ring_account(record: &PushRecord, push_id: &str, keyed: &[String]) -> Option<String> {
    let accounts = push_accounts(record, push_id, keyed);
    if accounts.iter().any(|account| record.keys.contains_key(account)) {
        log::warn!("Push: an unsealed ring of {push_id}, an id an account with push keys has: refused");
        return None;
    }
    match accounts.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "calls_tests.rs"]
mod tests;
