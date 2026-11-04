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

//! The host-owned outbox and drafts (plans/Mobile-Desktop.md §6, MD4): what a page sends while offline is kept in the
//! account's store ([`crate::store`] outbox) and sent by the host ([`super::flush`]), also while no window shows
//! it; what the user writes is kept there as a draft, with the files attached to it. The host's commands (the desktop
//! app's `outbox_add` and the rest, the page's `moquiDesktop.outbox`) call these for the calling page's account.
//!
//! Operations (WebrootChat.js and Compose.qvue):
//!
//! - `chat-send`: a message of the page's chat outbox as WebrootChat.js keeps it (`{clientRef, chatRoomId, url, params,
//!   text, threadRootEventSeq, hasFiles, sealed, createdDate}`); its order is kept per room, and per thread.
//! - `chat-read`: a room read up to `lastReadEventSeq` (absolute; the server only moves a marker forward); a newer one
//!   of a room replaces one still waiting. The room's row shows it read at once.
//! - `mail-send`: a compose sent offline, `{clientRef, fields, files}`: Compose's send parameters and the store's files
//!   to attach, sent as save draft, upload, send, each taken once by its `clientRef` (stage 2).
//! - `mail-state`: threads read, unread, flagged, archived, moved or trashed, as absolute values, applied to the stored
//!   rows at once so a list read offline shows it; all in one order. Each target takes `seen`, the state the stored
//!   thread had when the change was made, so the server leaves a thread changed since (a conflict, MD5).
//!
//! With a server at `clientApi` 4 (MD5, [`ServerSync`]) also, each with its `clientRef`, which the server takes once
//! (a ref sent again answers `duplicate`, which counts as done):
//!
//! - `chat-edit`, `chat-redact`, `chat-react` and `poll-vote`: a message's edit (`body`, or an encrypted room's
//!   `encryptedJson` with `mentionUserIds`), redaction (`reason`), reaction (`key`, or `encryptedJson`, or
//!   `reactionEventSeq` to take an encrypted one back) and poll vote (`optionIds`), by `chatRoomId` and `eventSeq`;
//!   the four of one message go in the order they were made (`store::order_group`).
//! - `internal-send`: an internal message's send (`fromValue` internal), when the server says its send is taken once
//!   by its `clientRef` (`features.internalSendRef`); a `mail-send` of one becomes it. No files.
//! - `mail-send` carries `baseStamp` (the server's draft the user edited) and `saveRef` (the save's own reference), so
//!   a draft changed on the server meanwhile is a conflict, not overwritten.
//!
//! A conflict (`conflict` state) keeps the server's state in `current`; a retry of it sends the user's anyway (no
//! `seen` or `baseStamp`, new references), a discard drops it.
//!
//! The checks here keep a page to what it could send itself with the key: the `url` of a chat send is a path of the
//! account's comms app (never a URL of its own), and the same limits as WebrootChat.js (50 waiting, 64 KB each).
//!
//! The pages hear of each change through the host (`ClientHost::notify_pages`): `changed`, or `sent` with the server's
//! answer to a chat or mail send.

use crate::login::ServerSync;
use crate::store::{new_client_ref, OutboxOp, OutboxState, OutboxUpdate, Store};
use base64::Engine;
use serde_json::{json, Map, Value};

use crate::offline::number;

/// The most operations waiting in an account's outbox (but those taken and waiting for their echo).
pub(crate) const OUTBOX_MAX: usize = 50;
/// The most bytes of an operation's JSON.
pub(crate) const ITEM_BYTES: usize = 64 * 1024;
/// The waits before a retry, by attempts: 2, 5, 15, then 60 seconds (WebrootChat.js `backoff`).
const BACKOFF_MS: [i64; 4] = [2_000, 5_000, 15_000, 60_000];
/// The longest draft key and draft text.
const KEY_MAX: usize = 512;
const DRAFT_BYTES: usize = 1024 * 1024;
/// The largest file a page may keep for a draft or a send: the server's limit for a compose attachment.
const FILE_BYTES: usize = 25 * 1024 * 1024;
/// The most files of one mail send.
const FILES_MAX: usize = 20;
/// The most drafts an account keeps, and their bytes in all, so a page cannot fill the disk.
const DRAFTS_MAX: usize = 500;
const DRAFTS_BYTES: usize = 16 * 1024 * 1024;
/// The share of the store's size cap (`<client-conf store-max-mb>`) the files of drafts and sends may take: a quarter.
const FILES_SHARE: u64 = 4;
/// The most threads of one mail state change.
const TARGETS_MAX: usize = 200;
/// The target all mail state changes share, so they reach the server in the order they were made.
pub(crate) const MAIL_STATE_TARGET: &str = "mail-state";

/// The parameters of Compose's send (and save draft) a mail send may carry.
const MAIL_FIELDS: [&str; 22] = [
    "fromValue",
    "emailMessageId",
    "toJson",
    "ccJson",
    "bccJson",
    "subject",
    "body",
    "bodyText",
    "quoteHtml",
    "parentEmailMessageId",
    "isForward",
    "parentCommEventId",
    "replyToSenderOnly",
    "uploadsJson",
    "recordEntity",
    "recordId",
    "scheduledSendDate",
    "archiveThread",
    "threadStore",
    "threadId",
    "baseStamp",
    "saveRef",
];

/// The thread state changes that may wait for the network; others need a connection.
const MAIL_ACTIONS: [&str; 8] = [
    "read",
    "unread",
    "flag",
    "unflag",
    "archive",
    "unarchive",
    "move",
    "trash",
];

/// The kinds of operation.
pub(crate) const CHAT_SEND: &str = "chat-send";
pub(crate) const CHAT_READ: &str = "chat-read";
pub(crate) const MAIL_SEND: &str = "mail-send";
pub(crate) const MAIL_STATE: &str = "mail-state";
pub(crate) const CHAT_EDIT: &str = "chat-edit";
pub(crate) const CHAT_REDACT: &str = "chat-redact";
pub(crate) const CHAT_REACT: &str = "chat-react";
pub(crate) const POLL_VOTE: &str = "poll-vote";
pub(crate) const INTERNAL_SEND: &str = "internal-send";

/// The kinds a server below `clientApi` 4 is sent (MD4).
const MD4_KINDS: [&str; 4] = [CHAT_SEND, CHAT_READ, MAIL_SEND, MAIL_STATE];
/// The kinds of a message's change, on a server at `clientApi` 4.
const MESSAGE_KINDS: [&str; 4] = [CHAT_EDIT, CHAT_REDACT, CHAT_REACT, POLL_VOTE];
/// The longest text of a message change's field (a body, its ciphertext).
const TEXT_BYTES: usize = 48 * 1024;

/// What an operation that did not go does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Next {
    /// Tried again after a wait: no answer (0), 408, 429, 5xx.
    Retry,
    /// Held until the user signs in again: 401, 403.
    Hold,
    /// Refused: another status.
    Fail,
}

/// What a send that did not go does next, by its HTTP status, as WebrootChat.js `classify`.
pub(crate) fn classify(status: u16) -> Next {
    match status {
        0 | 408 | 429 => Next::Retry,
        401 | 403 => Next::Hold,
        status if status >= 500 => Next::Retry,
        _ => Next::Fail,
    }
}

/// The wait in ms before the next try of an operation tried `attempts` times.
pub(crate) fn backoff_ms(attempts: u32) -> i64 {
    let at = usize::try_from(attempts.max(1))
        .unwrap_or(usize::MAX)
        .min(BACKOFF_MS.len())
        - 1;
    BACKOFF_MS[at]
}

/// A `clientRef` a page made: plain, short.
fn plain_ref(value: &Value) -> Result<String, String> {
    let text = value.as_str().unwrap_or("");
    // the characters the server allows in a clientRef
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-');
    if text.is_empty() || text.len() > 64 || !text.chars().all(allowed) {
        return Err("An operation needs a plain clientRef".to_string());
    }
    Ok(text.to_string())
}

/// A name of the store's files (`Store::save_file`): letters, digits, `.`, `-` and `_`, not starting with `.`.
fn plain_file(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
}

/// An id of the server (a room, a folder): not empty, short, no control characters.
fn plain_id(value: &Value, what: &str) -> Result<String, String> {
    let text = value.as_str().unwrap_or("");
    if text.is_empty() || text.len() > 255 || text.chars().any(char::is_control) {
        return Err(format!("An operation needs {what}"));
    }
    Ok(text.to_string())
}

/// Whether `url` is the transition `tail` of a comms app (`/apps/<path>/Chat/send`, or under `/qapps`): a plain path on
/// the account's server (the proxy sends it there and nowhere else), never a URL of the page's own. Any app's path,
/// as an app may mount Chat where the client config's comms root does not say.
pub(crate) fn comms_path(url: &str, tail: &str) -> bool {
    let Some(rest) = url.strip_prefix("/apps/").or_else(|| url.strip_prefix("/qapps/")) else {
        return false;
    };
    let Some(app) = rest.strip_suffix(tail) else {
        return false;
    };
    !app.is_empty()
        && app.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        })
}

/// A scalar of a form parameter: text, a number or a boolean as text, none for null.
fn scalar(value: &Value) -> Result<Option<String>, ()> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        Value::Number(number) => Ok(Some(number.to_string())),
        Value::Bool(flag) => Ok(Some(flag.to_string())),
        _ => Err(()),
    }
}

/// Check an operation a page asks for and make it, at `now_ms`, for a server of MD4 (`clientApi` 3): its id, target,
/// payload and files. A page's `files` are the store's files ([`file_save`]) it sends (a mail send's attachments).
///
/// # Errors
///
/// Returns why the operation cannot wait (unknown kind, a field missing or of the wrong kind, a URL that is not the
/// comms app's, too large), for the page to send it online or say so.
pub fn check(kind: &str, payload: &Value, files: &[String], now_ms: i64) -> Result<OutboxOp, String> {
    check_with(kind, payload, files, now_ms, &ServerSync::default())
}

/// The kinds of operation the account's server takes queued, as a page asks (`outbox_kinds`): `{clientApi, kinds,
/// internalSend}`.
pub fn kinds_json(server: &ServerSync) -> Value {
    let mut kinds: Vec<&str> = MD4_KINDS.to_vec();
    if server.syncs() {
        kinds.extend(MESSAGE_KINDS);
    }
    if server.internal_send() {
        kinds.push(INTERNAL_SEND);
    }
    json!({ "clientApi": server.client_api, "kinds": kinds, "internalSend": server.internal_send() })
}

/// [`check`] for the account's server as it is (`server`): with `clientApi` 4 the kinds of MD5 too, a mail send's
/// `baseStamp` and `saveRef` (one made when the page gives none), and an internal message queued when the server
/// takes its send once.
///
/// # Errors
///
/// Returns why the operation cannot wait, as [`check`].
pub fn check_with(
    kind: &str,
    payload: &Value,
    files: &[String],
    now_ms: i64,
    server: &ServerSync,
) -> Result<OutboxOp, String> {
    let Some(given) = payload.as_object() else {
        return Err("An operation is a JSON object".to_string());
    };
    if files.len() > FILES_MAX || files.iter().any(|name| !plain_file(name)) {
        return Err("An operation's files are not the store's".to_string());
    }
    if !MD4_KINDS.contains(&kind) && !server.syncs() {
        return Err(format!("{kind} needs a connection to this server"));
    }
    let internal = matches!(kind, MAIL_SEND | INTERNAL_SEND)
        && given
            .get("fields")
            .and_then(|fields| fields.get("fromValue"))
            .and_then(Value::as_str)
            == Some("internal");
    let kind = if internal && server.internal_send() {
        INTERNAL_SEND
    } else {
        kind
    };
    let (id, target, kept, created) = match kind {
        CHAT_EDIT | CHAT_REDACT | CHAT_REACT | POLL_VOTE => {
            if !files.is_empty() {
                return Err("A message's change sends no files".to_string());
            }
            let (id, target, kept) = message_change(kind, given)?;
            (id, target, kept, now_ms)
        }
        CHAT_SEND => {
            if !files.is_empty() {
                return Err("A chat message's files are uploaded before it is sent".to_string());
            }
            let id = plain_ref(&given.get("clientRef").cloned().unwrap_or(Value::Null))?;
            let room = plain_id(given.get("chatRoomId").unwrap_or(&Value::Null), "a chatRoomId")?;
            let url = given.get("url").and_then(Value::as_str).unwrap_or("");
            if !comms_path(url, "/Chat/send") {
                return Err(format!("Not the comms app's send: {url}"));
            }
            if let Some(params) = given.get("params") {
                let ok = params.is_null()
                    || params
                        .as_object()
                        .is_some_and(|map| map.values().all(|value| scalar(value).is_ok()));
                if !ok {
                    return Err("A chat message's params are plain values".to_string());
                }
            }
            let thread = given.get("threadRootEventSeq").map_or(Ok(None), |value| match value {
                Value::Null => Ok(None),
                other => number(other)
                    .map(Some)
                    .ok_or("threadRootEventSeq is a number".to_string()),
            })?;
            let target = match thread {
                Some(root) => format!("{room}:{root}"),
                None => room,
            };
            let mut kept = given.clone();
            // the page's own state of the item, which the host keeps instead
            for name in ["status", "attempts", "nextDate", "error", "expired", "sending"] {
                kept.remove(name);
            }
            let created = given
                .get("createdDate")
                .and_then(number)
                .filter(|created| *created > 0 && *created <= now_ms + 60_000)
                .unwrap_or(now_ms);
            (id, target, Value::Object(kept), created)
        }
        CHAT_READ => {
            let room = plain_id(given.get("chatRoomId").unwrap_or(&Value::Null), "a chatRoomId")?;
            let seq = given
                .get("lastReadEventSeq")
                .and_then(number)
                .filter(|seq| *seq >= 0)
                .ok_or("A mark-read needs lastReadEventSeq")?;
            let id = new_client_ref().map_err(|err| err.to_string())?;
            (
                id,
                room.clone(),
                json!({ "chatRoomId": room, "lastReadEventSeq": seq }),
                now_ms,
            )
        }
        MAIL_SEND => {
            let id = plain_ref(&given.get("clientRef").cloned().unwrap_or(Value::Null))?;
            let Some(fields) = given.get("fields").and_then(Value::as_object) else {
                return Err("A mail send needs its fields".to_string());
            };
            let mut kept_fields = Map::new();
            for (name, value) in fields {
                if !MAIL_FIELDS.contains(&name.as_str()) {
                    return Err(format!("A mail send has no field {name}"));
                }
                match scalar(value) {
                    Ok(Some(text)) => {
                        kept_fields.insert(name.clone(), Value::String(text));
                    }
                    Ok(None) => {}
                    Err(()) => return Err(format!("A mail send's {name} is a plain value")),
                }
            }
            if kept_fields
                .get("fromValue")
                .and_then(Value::as_str)
                .unwrap_or("")
                .is_empty()
            {
                return Err("A mail send needs fromValue".to_string());
            }
            // an internal message's send is taken once by its clientRef only where the server says so
            if kept_fields.get("fromValue").and_then(Value::as_str) == Some("internal") {
                return Err("An internal message needs a connection to send".to_string());
            }
            if server.syncs() {
                if !kept_fields.contains_key("saveRef") {
                    let save = new_client_ref().map_err(|err| err.to_string())?;
                    kept_fields.insert("saveRef".to_string(), Value::String(save));
                }
            } else {
                kept_fields.remove("baseStamp");
                kept_fields.remove("saveRef");
            }
            // the files' names and types as the page gives them, each one of the operation's files
            let mut kept_files = Vec::new();
            for entry in given.get("files").and_then(Value::as_array).into_iter().flatten() {
                let file = entry["file"].as_str().unwrap_or("");
                if !files.iter().any(|name| name == file) {
                    return Err(format!("A mail send's file {file} is not one it sends"));
                }
                kept_files.push(json!({ "file": file, "name": entry["name"].as_str().unwrap_or(file),
                    "contentType": entry["contentType"].as_str().unwrap_or("application/octet-stream") }));
            }
            if kept_files.len() != files.len() {
                return Err("Each file of a mail send needs its name".to_string());
            }
            (
                id.clone(),
                String::new(),
                json!({ "clientRef": id, "fields": kept_fields, "files": kept_files }),
                now_ms,
            )
        }
        INTERNAL_SEND => {
            if !server.internal_send() {
                return Err("An internal message needs a connection to send".to_string());
            }
            if !files.is_empty()
                || given
                    .get("files")
                    .and_then(Value::as_array)
                    .is_some_and(|list| !list.is_empty())
            {
                return Err("An internal message's files need a connection".to_string());
            }
            let id = plain_ref(&given.get("clientRef").cloned().unwrap_or(Value::Null))?;
            let Some(fields) = given.get("fields").and_then(Value::as_object) else {
                return Err("An internal message needs its fields".to_string());
            };
            let mut kept_fields = Map::new();
            for (name, value) in fields {
                if !MAIL_FIELDS.contains(&name.as_str()) || matches!(name.as_str(), "baseStamp" | "saveRef") {
                    return Err(format!("An internal message has no field {name}"));
                }
                match scalar(value) {
                    Ok(Some(text)) => {
                        kept_fields.insert(name.clone(), Value::String(text));
                    }
                    Ok(None) => {}
                    Err(()) => return Err(format!("An internal message's {name} is a plain value")),
                }
            }
            // one order for the internal sends, as a page sends them
            (
                id.clone(),
                INTERNAL_SEND.to_string(),
                json!({ "clientRef": id, "fields": kept_fields, "files": [] }),
                now_ms,
            )
        }
        MAIL_STATE => {
            let action = given.get("action").and_then(Value::as_str).unwrap_or("");
            if !MAIL_ACTIONS.contains(&action) {
                return Err(format!("{action} needs a connection"));
            }
            let targets = given
                .get("targets")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if targets.is_empty() || targets.len() > TARGETS_MAX || targets.iter().any(|target| !target.is_object()) {
                return Err("A mail state change needs its threads".to_string());
            }
            let mut kept = json!({ "targets": targets, "action": action });
            if action == "move" {
                kept["emailFolderId"] = Value::String(plain_id(
                    given.get("emailFolderId").unwrap_or(&Value::Null),
                    "an emailFolderId",
                )?);
            }
            let id = new_client_ref().map_err(|err| err.to_string())?;
            (id, MAIL_STATE_TARGET.to_string(), kept, now_ms)
        }
        _ => return Err(format!("No operation {kind}")),
    };
    let text = kept.to_string();
    if text.len() > ITEM_BYTES {
        return Err("The operation is too large to wait".to_string());
    }
    let mut op = OutboxOp::new(&id, kind, &target, &text);
    op.created_ms = created;
    op.next_ms = now_ms;
    op.files = files.to_vec();
    Ok(op)
}

/// A message's change (edit, redaction, reaction, poll vote): its `clientRef`, its target (the message) and the
/// fields kept.
fn message_change(kind: &str, given: &Map<String, Value>) -> Result<(String, String, Value), String> {
    let id = plain_ref(&given.get("clientRef").cloned().unwrap_or(Value::Null))?;
    let room = plain_id(given.get("chatRoomId").unwrap_or(&Value::Null), "a chatRoomId")?;
    let seq = given
        .get("eventSeq")
        .and_then(number)
        .filter(|seq| *seq > 0)
        .ok_or("A message's change needs its eventSeq")?;
    let text = |name: &str| -> Result<Option<String>, String> {
        match given.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) if text.len() <= TEXT_BYTES => Ok(Some(text.clone())),
            Some(Value::String(_)) => Err(format!("A message's {name} is too large to wait")),
            Some(_) => Err(format!("A message's {name} is text")),
        }
    };
    let mut kept = json!({ "clientRef": id, "chatRoomId": room, "eventSeq": seq });
    let names: &[&str] = match kind {
        CHAT_EDIT => &["body", "encryptedJson", "mentionUserIds"],
        CHAT_REDACT => &["reason"],
        CHAT_REACT => &["key", "encryptedJson"],
        _ => &[],
    };
    for name in names {
        if let Some(value) = text(name)? {
            kept[*name] = Value::String(value);
        }
    }
    match kind {
        CHAT_EDIT if kept["body"].is_null() && kept["encryptedJson"].is_null() => {
            return Err("An edit needs its body".to_string());
        }
        CHAT_REACT => {
            if let Some(reaction) = given.get("reactionEventSeq").and_then(number) {
                kept["reactionEventSeq"] = json!(reaction);
            }
            if kept["key"].is_null() && kept["encryptedJson"].is_null() && kept["reactionEventSeq"].is_null() {
                return Err("A reaction needs its key".to_string());
            }
        }
        POLL_VOTE => {
            let ids: Vec<i64> = match given.get("optionIds") {
                Some(Value::Array(list)) => list.iter().filter_map(number).collect(),
                Some(Value::String(text)) => text.split(',').filter_map(|id| id.trim().parse().ok()).collect(),
                _ => Vec::new(),
            };
            if ids.is_empty() || ids.len() > 100 {
                return Err("A vote needs its optionIds".to_string());
            }
            let text: Vec<String> = ids.iter().map(i64::to_string).collect();
            kept["optionIds"] = Value::String(text.join(","));
        }
        _ => {}
    }
    Ok((id, format!("{room}#{seq}"), kept))
}

/// The state a stored thread shows for the fields a mail action changes, as `set#ThreadState`'s `seen` names them
/// (`EmailClientServices.check#SeenState`): read and archive with the newest message seen, so one come since is not
/// taken unseen. None for a target the store does not hold, of another store than email, or of one message.
pub(crate) fn seen_of(store: &Store, action: &str, target: &Value, earlier: &[Value]) -> Option<Value> {
    if target["store"].as_str().is_some_and(|name| name != "email") || target["emailMessageId"].is_string() {
        return None;
    }
    let key = target_key(target)?;
    let wanted_folder = target["emailFolderId"].as_str();
    let pick = |rows: Vec<(String, Value)>| -> Option<Value> {
        let at = rows
            .iter()
            .position(|(folder, _)| wanted_folder.is_some_and(|wanted| folder.ends_with(&format!("folder={wanted}"))))
            .unwrap_or(0);
        rows.into_iter().nth(at).map(|(_, row)| row)
    };
    let current = pick(
        store
            .mail_threads_of(&key)
            .into_iter()
            .filter_map(|row| Some((row.folder_id, serde_json::from_str::<Value>(&row.row).ok()?)))
            .collect(),
    );
    // the server's state before the changes still queued for this thread, as the first of them kept it
    let baseline = earlier.iter().find_map(|payload| {
        let rows: Vec<(String, Value)> = payload["undo"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["threadId"].as_str() == Some(key.as_str()))
            .filter_map(|row| {
                Some((
                    row["folderId"].as_str()?.to_string(),
                    serde_json::from_str(row["row"].as_str()?).ok()?,
                ))
            })
            .collect();
        pick(rows)
    });
    let row = current.as_ref().or(baseline.as_ref())?;
    // a field a queued change makes is seen as that change leaves it (the rows show it), another as the server has it,
    // so one conflict does not make the later changes of other fields conflict too
    let changed_before = |actions: &[&str]| {
        earlier.iter().any(|payload| {
            actions.contains(&payload["action"].as_str().unwrap_or(""))
                && payload["targets"].as_array().is_some_and(|targets| {
                    targets
                        .iter()
                        .any(|other| target_key(other).as_deref() == Some(key.as_str()))
                })
        })
    };
    let source = |actions: &[&str]| -> &Value {
        match (&baseline, changed_before(actions)) {
            (Some(baseline), false) => baseline,
            _ => row,
        }
    };
    // the newest message the user saw: the page's row's, else the newest kept, else the row's
    let latest = target["lastEmailMessageId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            store.mail_messages(&key).last().map(|message| {
                message
                    .message_id
                    .strip_prefix("email:")
                    .unwrap_or(&message.message_id)
                    .to_string()
            })
        })
        .or_else(|| row["lastEmailMessageId"].as_str().map(str::to_string));
    let truth = |value: &Value| value.as_bool() == Some(true);
    let mut seen = Map::new();
    match action {
        "read" | "unread" => {
            let from = source(&["read", "unread"]);
            seen.insert("isRead".to_string(), json!(!truth(&from["isUnread"])));
            seen.insert("latestMessageId".to_string(), json!(latest));
        }
        "flag" => {
            // a flag goes on the thread's newest message: its own flag when it is kept, else none flagged at all
            let from = source(&["flag", "unflag"]);
            let message = latest
                .as_deref()
                .and_then(|id| store.mail_message(&format!("email:{id}")));
            let flagged = match message.and_then(|message| serde_json::from_str::<Value>(&message.row).ok()) {
                Some(message) => Some(truth(&message["isFlagged"])),
                None if !truth(&from["isFlagged"]) => Some(false),
                None => None,
            };
            seen.insert("isFlagged".to_string(), json!(flagged?));
        }
        "unflag" => {
            let from = source(&["flag", "unflag"]);
            seen.insert("isFlagged".to_string(), json!(truth(&from["isFlagged"])));
        }
        "archive" | "unarchive" => {
            let from = source(&["archive", "unarchive", "move", "trash"]);
            seen.insert("archived".to_string(), json!(truth(&from["isArchived"])));
            seen.insert("latestMessageId".to_string(), json!(latest));
        }
        "move" | "trash" => {
            let from = source(&["archive", "unarchive", "move", "trash"]);
            let folder = from["emailFolderId"].as_str()?;
            seen.insert("emailFolderId".to_string(), json!(folder));
        }
        _ => return None,
    }
    seen.retain(|_, value| !value.is_null());
    (!seen.is_empty()).then_some(Value::Object(seen))
}

/// The store's key of a mail state target's thread (its `rowKey`, else `store:threadId`).
pub(crate) fn target_key(target: &Value) -> Option<String> {
    target["rowKey"].as_str().map(str::to_string).or_else(|| {
        target["threadId"]
            .as_str()
            .map(|thread| format!("{}:{thread}", target["store"].as_str().unwrap_or("email")))
    })
}

/// A mail state change the server did not make (a conflict, a refusal): the stored rows of its threads (those of
/// `only`, or all) put back as they were before it (its `undo`), and the threads read again by the next feeds run, as
/// the server's state is what stands.
///
/// # Errors
///
/// Returns an error when a row cannot be written.
pub(crate) fn undo_mail_state(store: &Store, payload: &Value, only: Option<&[String]>) -> Result<(), String> {
    let wanted = |key: &str| only.is_none_or(|keys| keys.iter().any(|kept| kept == key));
    let rows: Vec<crate::store::MailThread> = payload["undo"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            Some(crate::store::MailThread {
                folder_id: row["folderId"].as_str()?.to_string(),
                thread_id: row["threadId"].as_str()?.to_string(),
                sort_ms: number(&row["sortMs"]).unwrap_or(0),
                row: row["row"].as_str()?.to_string(),
            })
        })
        .filter(|row| wanted(&row.thread_id))
        .collect();
    store.put_mail_threads(&rows).map_err(|err| err.to_string())?;
    for target in payload["targets"].as_array().into_iter().flatten() {
        if let Some(key) = target_key(target).filter(|key| wanted(key)) {
            super::feeds::note_pending(store, &format!("thread:{key}"))?;
        }
    }
    Ok(())
}

/// Add a checked operation to the store, with its change to the stored rows, as one unit: the kept one when one of its
/// id is there already (a page adding again, an import), and a mark-read of a room replacing one still waiting. Gives
/// the operation and whether it was added.
///
/// # Errors
///
/// Returns an error when the outbox is full, or the store does not take it.
pub(crate) fn add(store: &Store, mut op: OutboxOp) -> Result<(OutboxOp, bool), String> {
    store
        .atomic(|store| {
            if let Some(kept) = store.outbox_get(&op.id) {
                return Ok((kept, false));
            }
            let list = store.outbox_list();
            if op.kind == CHAT_READ {
                let mut seq = payload_of(&op)["lastReadEventSeq"].as_i64().unwrap_or(0);
                for waiting in list.iter().filter(|other| {
                    other.kind == CHAT_READ
                        && other.target == op.target
                        && matches!(
                            other.state,
                            OutboxState::Queued | OutboxState::Retry | OutboxState::Held
                        )
                }) {
                    seq = seq.max(payload_of(waiting)["lastReadEventSeq"].as_i64().unwrap_or(0));
                    store.outbox_remove(&waiting.id)?;
                }
                op.payload = json!({ "chatRoomId": op.target, "lastReadEventSeq": seq }).to_string();
            }
            if op.kind == MAIL_STATE {
                // what the user saw, before the change is shown in the rows, and the rows as they were, to put back
                // when the server does not make it (undo_mail_state)
                let earlier: Vec<Value> = list
                    .iter()
                    .filter(|other| {
                        other.kind == MAIL_STATE
                            && matches!(
                                other.state,
                                OutboxState::Queued | OutboxState::Retry | OutboxState::Held | OutboxState::Sending
                            )
                    })
                    .map(payload_of)
                    .collect();
                let mut payload = payload_of(&op);
                let action = payload["action"].as_str().unwrap_or("").to_string();
                let mut undo = Vec::new();
                if let Some(targets) = payload["targets"].as_array_mut() {
                    for target in targets.iter_mut() {
                        if let Some(key) = target_key(target) {
                            for row in store.mail_threads_of(&key) {
                                undo.push(json!({ "folderId": row.folder_id, "threadId": row.thread_id,
                                    "sortMs": row.sort_ms, "row": row.row }));
                            }
                        }
                        if target["seen"].is_null() {
                            if let Some(seen) = seen_of(store, &action, target, &earlier) {
                                target["seen"] = seen;
                            }
                        }
                    }
                }
                let without = payload.to_string();
                payload["undo"] = Value::Array(undo);
                let with_undo = payload.to_string();
                // the rows only help: past the size of an operation it goes without them
                op.payload = if with_undo.len() <= ITEM_BYTES {
                    with_undo
                } else {
                    without
                };
            }
            let waiting = store
                .outbox_list()
                .iter()
                .filter(|other| other.state != OutboxState::Acked)
                .count();
            if waiting >= OUTBOX_MAX {
                return Err(moqui_lib::error::Error::new("The outbox is full"));
            }
            store.outbox_add(&op)?;
            let payload = payload_of(&op);
            let applied = match op.kind.as_str() {
                CHAT_READ => super::push::read_room(store, &op.target, payload["lastReadEventSeq"].clone()),
                MAIL_STATE => apply_mail_state(store, &payload),
                _ => Ok(()),
            };
            // the local copy only helps: the operation is kept all the same
            if let Err(err) = applied {
                tracing::warn!("The stored rows of {} were not changed: {err}", op.id);
            }
            Ok((op.clone(), true))
        })
        .map_err(|err| err.to_string())
}

/// The JSON of an operation's payload; an empty object when it does not parse.
pub(crate) fn payload_of(op: &OutboxOp) -> Value {
    serde_json::from_str(&op.payload).unwrap_or_else(|_| json!({}))
}

/// Apply a mail state change to the stored thread rows, as Mail.qvue `applyLocal` does to its list: read, unread, flag
/// and unflag change the rows in every folder; archive takes the thread out of the inbox, move out of every folder but
/// all mail, and trash out of every folder. The server's next answer replaces them.
///
/// # Errors
///
/// Returns an error when a row cannot be written.
pub(crate) fn apply_mail_state(store: &Store, payload: &Value) -> Result<(), String> {
    let action = payload["action"].as_str().unwrap_or("");
    for target in payload["targets"].as_array().into_iter().flatten() {
        let key = target["rowKey"].as_str().map(str::to_string).or_else(|| {
            target["threadId"]
                .as_str()
                .map(|thread| format!("{}:{thread}", target["store"].as_str().unwrap_or("email")))
        });
        let Some(key) = key else { continue };
        let rows = store.mail_threads_of(&key);
        let leaves = |folder: &str| match action {
            "archive" => folder == "view=inbox",
            "move" => folder != "view=all",
            "trash" => true,
            _ => false,
        };
        let mut changed = Vec::new();
        for mut row in rows {
            if leaves(&row.folder_id) {
                store
                    .remove_mail_thread(&row.folder_id, &row.thread_id)
                    .map_err(|err| err.to_string())?;
                continue;
            }
            let Ok(mut fields) = serde_json::from_str::<Value>(&row.row) else {
                continue;
            };
            match action {
                "read" => {
                    fields["isUnread"] = json!(false);
                    fields["unreadCount"] = json!(0);
                }
                "unread" => {
                    fields["isUnread"] = json!(true);
                    fields["unreadCount"] = json!(number(&fields["unreadCount"]).unwrap_or(0).max(1));
                }
                "flag" => fields["isFlagged"] = json!(true),
                "unflag" => fields["isFlagged"] = json!(false),
                _ => continue,
            }
            row.row = fields.to_string();
            changed.push(row);
        }
        store.put_mail_threads(&changed).map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// A chat message of mine came back on `/notws` with its `clientRef`: its operation is done (sent from here, another
/// device's page, or its answer was lost). Whether one was removed.
pub(crate) fn echoed(store: &Store, client_ref: &str) -> bool {
    let Some(op) = store.outbox_get(client_ref) else {
        return false;
    };
    if op.kind != CHAT_SEND {
        return false;
    }
    match store.outbox_remove(client_ref) {
        Ok(removed) => removed,
        Err(err) => {
            tracing::warn!("The echoed operation {client_ref} was not removed: {err}");
            false
        }
    }
}

/// An operation as a page sees it.
pub(crate) fn op_json(op: &OutboxOp) -> Value {
    json!({
        "id": op.id,
        "kind": op.kind,
        "target": op.target,
        "state": op.state.as_str(),
        "attempts": op.attempts,
        "nextMs": op.next_ms,
        "createdMs": op.created_ms,
        "updatedMs": op.updated_ms,
        "error": op.error,
        "payload": payload_of(op),
        "files": op.files,
        "status": op.status,
        "rejected": op.rejected(),
        "current": op.current.as_deref().and_then(|current| serde_json::from_str::<Value>(current).ok()),
    })
}

/// The operations a page shows: all but those taken and waiting for their echo.
pub fn list_json(store: &Store) -> Value {
    Value::Array(
        store
            .outbox_list()
            .iter()
            .filter(|op| op.state != OutboxState::Acked)
            .map(op_json)
            .collect(),
    )
}

/// The user's retry of an operation: due now, its attempts and error cleared. False when there is none. A conflict's
/// retry sends the user's change anyway: a thread's without `seen` and with a new request ref, a draft's without
/// `baseStamp` and with a new `saveRef`.
pub fn retry(store: &Store, id: &str, now_ms: i64) -> Result<bool, String> {
    let Some(op) = store.outbox_get(id) else {
        return Ok(false);
    };
    if matches!(op.state, OutboxState::Sending | OutboxState::Acked) {
        return Ok(true);
    }
    if op.state == OutboxState::Conflict {
        let mut payload = payload_of(&op);
        match op.kind.as_str() {
            MAIL_STATE => {
                for target in payload["targets"].as_array_mut().into_iter().flatten() {
                    if let Some(target) = target.as_object_mut() {
                        target.remove("seen");
                    }
                }
                payload["requestRef"] = json!(new_client_ref().map_err(|err| err.to_string())?);
            }
            MAIL_SEND => {
                if let Some(fields) = payload["fields"].as_object_mut() {
                    fields.remove("baseStamp");
                    fields.insert(
                        "saveRef".to_string(),
                        json!(new_client_ref().map_err(|err| err.to_string())?),
                    );
                }
                // the user's text is sent after all: its copy goes, so it is not sent twice
                let current: Value = op
                    .current
                    .as_deref()
                    .and_then(|current| serde_json::from_str(current).ok())
                    .unwrap_or(Value::Null);
                if let Some(copy) = current["copyKey"].as_str() {
                    store.remove_draft(copy).map_err(|err| err.to_string())?;
                }
            }
            _ => {}
        }
        store
            .outbox_set_payload(id, &payload.to_string())
            .map_err(|err| err.to_string())?;
    }
    store
        .outbox_update(
            id,
            &OutboxUpdate {
                state: OutboxState::Retry,
                attempts: 0,
                next_ms: now_ms,
                error: None,
            },
        )
        .map_err(|err| err.to_string())
}

/// What the account would lose on a shared device's sign-out: the operations not yet taken, and the drafts.
pub fn unsent_json(store: &Store) -> Value {
    let drafts: Vec<Value> = store
        .drafts("")
        .iter()
        .map(|draft| json!({ "key": draft.key, "updatedMs": draft.updated_ms }))
        .collect();
    json!({ "operations": list_json(store), "drafts": drafts })
}

/// A draft as a page sees it.
fn draft_json(draft: &crate::store::Draft) -> Value {
    json!({ "key": draft.key, "value": draft.value, "files": draft.files, "updatedMs": draft.updated_ms })
}

/// Now, in milliseconds since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Keep a checked operation ([`check`]) whose files the store holds: answers it as the outbox lists it, and whether it
/// was added (false for one already kept).
///
/// # Errors
///
/// Returns an error when one of its files is not kept, the outbox is full, or the store does not take it.
pub fn add_kept(store: &Store, op: OutboxOp) -> Result<(Value, bool), String> {
    if op
        .files
        .iter()
        .any(|name| store.file_path(name).is_none_or(|path| !path.is_file()))
    {
        return Err("An operation's file is not kept".to_string());
    }
    let (kept, added) = add(store, op)?;
    Ok((op_json(&kept), added))
}

/// Remove an operation and its files without sending it; false when there was none.
///
/// # Errors
///
/// Returns an error when it is being sent, or the store does not change.
pub fn discard(store: &Store, id: &str) -> Result<bool, String> {
    if store.outbox_get(id).is_some_and(|op| op.state == OutboxState::Sending) {
        return Err("It is being sent".to_string());
    }
    store.outbox_remove(id).map_err(|err| err.to_string())
}

/// A draft key a page may use: not empty, short, no control characters.
///
/// # Errors
///
/// Returns an error for a key that is not one.
pub fn draft_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > KEY_MAX || key.chars().any(char::is_control) {
        return Err("Not a draft key".to_string());
    }
    Ok(())
}

/// The draft of `key`: `{key, value, files, updatedMs}`, or null.
pub fn draft_get_json(store: &Store, key: &str) -> Value {
    store.draft(key).as_ref().map_or(Value::Null, draft_json)
}

/// A draft key prefix a page may list by.
///
/// # Errors
///
/// Returns an error for a prefix longer than a key.
pub fn draft_prefix(prefix: &str) -> Result<(), String> {
    if prefix.len() > KEY_MAX {
        return Err("Not a draft key".to_string());
    }
    Ok(())
}

/// The drafts whose key starts with `prefix`, by key.
pub fn draft_list_json(store: &Store, prefix: &str) -> Value {
    Value::Array(store.drafts(prefix).iter().map(draft_json).collect())
}

/// Whether a page may keep `value` as the draft of `key` with `files` attached: the key, the text's size and the files'
/// count.
///
/// # Errors
///
/// Returns an error saying which limit it passes.
pub fn check_draft(key: &str, value: &str, files: &[String]) -> Result<(), String> {
    draft_key(key)?;
    if value.len() > DRAFT_BYTES {
        return Err("The draft is too large to keep".to_string());
    }
    if files.len() > FILES_MAX {
        return Err("A draft keeps at most 20 files".to_string());
    }
    Ok(())
}

/// Keep a checked draft ([`check_draft`]), when the drafts have room for it.
///
/// # Errors
///
/// Returns an error when there is no room, or the store does not take it.
pub fn put_draft(store: &Store, key: &str, value: &str, files: &[String]) -> Result<(), String> {
    draft_room(store, key, value.len())?;
    store.put_draft(key, value, files).map_err(|err| err.to_string())
}

/// Whether the drafts have room for `key` with `bytes` of text: at most [`DRAFTS_MAX`] drafts and [`DRAFTS_BYTES`] in
/// all, the draft it replaces not counted.
pub(crate) fn draft_room(store: &Store, key: &str, bytes: usize) -> Result<(), String> {
    let others: Vec<_> = store.drafts("").into_iter().filter(|draft| draft.key != key).collect();
    if others.len() >= DRAFTS_MAX {
        return Err("Too many drafts are kept; send or discard some".to_string());
    }
    if others.iter().map(|draft| draft.value.len()).sum::<usize>() + bytes > DRAFTS_BYTES {
        return Err("The drafts kept are too large; send or discard some".to_string());
    }
    Ok(())
}

/// Whether the store's files have room for `bytes` more under a size cap of `max_bytes`: a quarter of it.
pub(crate) fn file_room(store: &Store, bytes: usize, max_bytes: u64) -> Result<(), String> {
    let adding = u64::try_from(bytes).unwrap_or(u64::MAX);
    if store.files_bytes().saturating_add(adding) > max_bytes / FILES_SHARE {
        return Err("The files kept for drafts and sends are too large; send or discard some".to_string());
    }
    Ok(())
}

/// The bytes of a file a page keeps for a draft or a send (`data` in base64), at most 25 MB.
///
/// # Errors
///
/// Returns an error for a file too large or not base64.
pub fn file_bytes(data: &str) -> Result<Vec<u8>, String> {
    if data.len() > FILE_BYTES / 3 * 4 + 4 {
        return Err("The file is over 25 MB".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| "The file is not base64".to_string())
}

/// Keep a file of a draft or a send, named after `name`: answers the store's name for it, which a draft or an operation
/// is to refer to soon; a file nothing refers to is removed after a grace (`Store::gc_files`, also run by each pass of
/// the sender). The files in all are capped at a quarter of the store's size cap, `max_bytes`.
///
/// # Errors
///
/// Returns an error when there is no room, or the store does not take it.
pub fn save_file(store: &Store, name: &str, bytes: &[u8], max_bytes: u64) -> Result<String, String> {
    file_room(store, bytes.len(), max_bytes)?;
    store.save_file(name, bytes).map_err(|err| err.to_string())
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
