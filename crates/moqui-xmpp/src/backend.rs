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

//! What the server asks of Moqui. Everything a session reads or writes is a service call by name, as a user (with that
//! user's authorization, as the web client's calls are) or as internal work (sign-in lookups only), so the chat rules
//! stay in `org.moqui.chat.ChatServices` and `XmppServices` and this crate needs no ExecutionContext. moqui-host runs
//! the calls on its request workers; the tests have an in-memory backend.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value as Json;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Who a call runs as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// Internal work, with authorization disabled: the sign-in lookups of `XmppServices`.
    Internal,
    /// A signed-in user, by username: the call runs logged in as them.
    User(String),
}

/// A service call that failed: the service's errors, or why it could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError(pub String);

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait Backend: Send + Sync {
    /// Call `service` with `parameters` (a JSON object) as `caller`; its out-parameters as a JSON object.
    fn call(&self, caller: Caller, service: &str, parameters: Json) -> BoxFuture<'_, Result<Json, CallError>>;
}

/// The services this crate calls.
pub mod services {
    pub const ACCOUNT: &str = "org.moqui.chat.XmppServices.get#Account";
    pub const SCRAM: &str = "org.moqui.chat.XmppServices.get#ScramCredentials";
    pub const CHECK_PLAIN: &str = "org.moqui.chat.XmppServices.check#PlainPassword";
    pub const FAST_TOKEN: &str = "org.moqui.chat.XmppServices.get#FastToken";
    pub const ISSUE_FAST_TOKEN: &str = "org.moqui.chat.XmppServices.issue#FastToken";
    pub const USE_FAST_TOKEN: &str = "org.moqui.chat.XmppServices.use#FastToken";
    pub const INVALIDATE_FAST_TOKEN: &str = "org.moqui.chat.XmppServices.invalidate#FastToken";
    pub const CHECK_CREDENTIAL: &str = "org.moqui.chat.XmppServices.check#Credential";
    pub const USERS: &str = "org.moqui.chat.XmppServices.get#Users";
    pub const ROSTER: &str = "org.moqui.chat.XmppServices.get#Roster";
    pub const DIRECT_ROOM: &str = "org.moqui.chat.XmppServices.get#DirectRoom";
    pub const ROOM: &str = "org.moqui.chat.XmppServices.get#RoomInfo";
    pub const ARCHIVE: &str = "org.moqui.chat.XmppServices.get#Archive";
    pub const FIND_EVENT: &str = "org.moqui.chat.XmppServices.find#EventByRef";
    pub const STORE_ENCRYPTED: &str = "org.moqui.chat.XmppServices.store#EncryptedMessage";
    pub const PEP_ITEMS: &str = "org.moqui.chat.XmppServices.get#PepItems";
    pub const PEP_PUBLISH: &str = "org.moqui.chat.XmppServices.publish#PepItem";
    pub const PEP_RETRACT: &str = "org.moqui.chat.XmppServices.retract#PepItem";
    pub const PEP_CONFIGURE: &str = "org.moqui.chat.XmppServices.configure#PepNode";
    pub const PEP_DELETE: &str = "org.moqui.chat.XmppServices.delete#PepNode";
    pub const BLOCKS: &str = "org.moqui.chat.XmppServices.get#BlockedJids";
    pub const BLOCK: &str = "org.moqui.chat.XmppServices.block#Jids";
    pub const UNBLOCK: &str = "org.moqui.chat.XmppServices.unblock#Jids";
    pub const PUSH_ENABLE: &str = "org.moqui.chat.XmppServices.enable#Push";
    pub const PUSH_DISABLE: &str = "org.moqui.chat.XmppServices.disable#Push";
    pub const PUSH_TARGETS: &str = "org.moqui.chat.XmppServices.get#PushTargets";
    pub const UPLOAD_SLOT: &str = "org.moqui.chat.XmppServices.create#UploadSlot";
    pub const ATTACH_UPLOAD: &str = "org.moqui.chat.XmppServices.attach#Upload";
    pub const ATTACHMENT_URLS: &str = "org.moqui.chat.XmppServices.get#AttachmentUrls";
    pub const ICE_SERVERS: &str = "org.moqui.chat.XmppServices.get#IceServers";

    pub const SEND: &str = "org.moqui.chat.ChatServices.send#Message";
    pub const EDIT: &str = "org.moqui.chat.ChatServices.edit#Message";
    pub const REDACT: &str = "org.moqui.chat.ChatServices.redact#Message";
    pub const REACT: &str = "org.moqui.chat.ChatServices.react#Message";
    pub const MARK_READ: &str = "org.moqui.chat.ChatServices.mark#Read";
    pub const TYPING: &str = "org.moqui.chat.ChatServices.signal#Typing";
    pub const ADD_MEMBER: &str = "org.moqui.chat.ChatServices.add#Member";
    pub const FIND_ROOMS: &str = "org.moqui.chat.ChatServices.find#Rooms";
    pub const SET_STATUS: &str = "org.moqui.chat.ChatServices.set#UserStatus";
    pub const HEARTBEAT: &str = "org.moqui.chat.ChatServices.heartbeat#UserStatus";
    pub const DISCONNECT: &str = "org.moqui.chat.ChatServices.disconnect#UserStatus";
    pub const STATUSES: &str = "org.moqui.chat.ChatServices.get#UserStatuses";
}

/// Helpers to read a service's JSON answer.
pub fn text(value: &Json, field: &str) -> String {
    match value.get(field) {
        Some(Json::String(text)) => text.clone(),
        Some(Json::Number(number)) => number.to_string(),
        Some(Json::Bool(flag)) => flag.to_string(),
        _ => String::new(),
    }
}

pub fn number(value: &Json, field: &str) -> Option<i64> {
    match value.get(field) {
        Some(Json::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|n| n as i64))
            .or_else(|| number.to_string().parse::<f64>().ok().map(|n| n as i64)),
        Some(Json::String(text)) => text.trim().parse::<f64>().ok().map(|n| n as i64),
        _ => None,
    }
}

pub fn flag(value: &Json, field: &str) -> bool {
    match value.get(field) {
        Some(Json::Bool(flag)) => *flag,
        Some(Json::String(text)) => text == "true" || text == "Y",
        _ => false,
    }
}

pub fn list<'a>(value: &'a Json, field: &str) -> &'a [Json] {
    value.get(field).and_then(Json::as_array).map_or(&[], Vec::as_slice)
}
