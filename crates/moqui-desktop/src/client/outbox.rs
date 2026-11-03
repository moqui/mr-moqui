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
//! account's store (`moqui_client::store` outbox) and sent by the host ([`super::flush`]), also while no window shows
//! it; what the user writes is kept there as a draft, with the files attached to it.
//!
//! Operations (`outbox_add`, the page's `moquiDesktop.outbox`, WebrootChat.js and Compose.qvue):
//!
//! - `chat-send`: a message of the page's chat outbox as WebrootChat.js keeps it (`{clientRef, chatRoomId, url, params,
//!   text, threadRootEventSeq, hasFiles, sealed, createdDate}`); its order is kept per room, and per thread.
//! - `chat-read`: a room read up to `lastReadEventSeq` (absolute; the server only moves a marker forward); a newer one
//!   of a room replaces one still waiting. The room's row shows it read at once.
//! - `mail-send`: a compose sent offline, `{clientRef, fields, files}`: Compose's send parameters and the store's files
//!   to attach, sent as save draft, upload, send, each taken once by its `clientRef` (stage 2).
//! - `mail-state`: threads read, unread, flagged, archived, moved or trashed, as absolute values, applied to the stored
//!   rows at once so a list read offline shows it; all in one order.
//!
//! The checks here keep a page to what it could send itself with the key: the `url` of a chat send is a path of the
//! account's comms app (never a URL of its own), and the same limits as WebrootChat.js (50 waiting, 64 KB each).
//! The commands answer for the account of the calling window: the shell's current one, an app window's own.
//!
//! The pages hear of each change as `window.__moquiOutboxEvent({kind})`, evaluated in the account's windows as socket
//! frames are (`sockets.rs`): `changed`, or `sent` with the server's answer to a chat or mail send.

use base64::Engine;
use moqui_client::store::{new_client_ref, OutboxOp, OutboxState, OutboxUpdate, Store};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager, Webview};

use super::offline::number;
use super::Client;

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
const MAIL_FIELDS: [&str; 20] = [
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

/// Check an operation a page asks for and make it, at `now_ms`: its id, target, payload and files. A page's `files`
/// are the store's files ([`file_save`]) it sends (a mail send's attachments).
///
/// # Errors
///
/// Returns why the operation cannot wait (unknown kind, a field missing or of the wrong kind, a URL that is not the
/// comms app's, too large), for the page to send it online or say so.
pub(crate) fn check(kind: &str, payload: &Value, files: &[String], now_ms: i64) -> Result<OutboxOp, String> {
    let Some(given) = payload.as_object() else {
        return Err("An operation is a JSON object".to_string());
    };
    if files.len() > FILES_MAX || files.iter().any(|name| !plain_file(name)) {
        return Err("An operation's files are not the store's".to_string());
    }
    let (id, target, kept, created) = match kind {
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
            // an internal message's send is not taken once by its clientRef yet (MD5): it waits for a connection
            if kept_fields.get("fromValue").and_then(Value::as_str) == Some("internal") {
                return Err("An internal message needs a connection to send".to_string());
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
    })
}

/// The operations a page shows: all but those taken and waiting for their echo.
pub(crate) fn list_json(store: &Store) -> Value {
    Value::Array(
        store
            .outbox_list()
            .iter()
            .filter(|op| op.state != OutboxState::Acked)
            .map(op_json)
            .collect(),
    )
}

/// The user's retry of an operation: due now, its attempts and error cleared. False when there is none.
pub(crate) fn retry(store: &Store, id: &str, now_ms: i64) -> Result<bool, String> {
    let Some(op) = store.outbox_get(id) else {
        return Ok(false);
    };
    if matches!(op.state, OutboxState::Sending | OutboxState::Acked) {
        return Ok(true);
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
pub(crate) fn unsent_json(store: &Store) -> Value {
    let drafts: Vec<Value> = store
        .drafts("")
        .iter()
        .map(|draft| json!({ "key": draft.key, "updatedMs": draft.updated_ms }))
        .collect();
    json!({ "operations": list_json(store), "drafts": drafts })
}

/// A draft as a page sees it.
fn draft_json(draft: &moqui_client::store::Draft) -> Value {
    json!({ "key": draft.key, "value": draft.value, "files": draft.files, "updatedMs": draft.updated_ms })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// Tell the account's windows that its outbox changed: `event` to `window.__moquiOutboxEvent` in the shell (when it
/// shows the account) and the account's app windows.
pub(crate) fn notify_pages(app: &AppHandle, account: &str, event: &Value) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    let mut labels: Vec<String> = moqui_lib::sync::lock(&client.windows)
        .iter()
        .filter(|(_, owner)| owner == account)
        .map(|(label, _)| label.clone())
        .collect();
    let current = moqui_lib::sync::lock(&client.accounts)
        .current()
        .is_some_and(|shown| shown.id == account);
    if current {
        labels.push("main".to_string());
    }
    let script = format!("window.__moquiOutboxEvent && window.__moquiOutboxEvent({event})");
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.eval(script.clone());
        }
    }
}

/// The account of the calling window; an error when none is signed in there.
fn caller(client: &Client, webview: &Webview) -> Result<String, String> {
    let (account, _) = client.account_of(webview.label()).ok_or("No account is signed in")?;
    Ok(account.id)
}

/// Run `work` on the store of the calling window's account.
fn on_store<T>(
    app: &AppHandle,
    webview: &Webview,
    work: impl FnOnce(&str, &Store) -> Result<T, String>,
) -> Result<T, String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let id = caller(&client, webview)?;
    client
        .with_store(&id, |store| work(&id, store))
        .unwrap_or_else(|| Err("This account's data is not open".to_string()))
}

/// Keep an operation to send (`kind`, its `payload`, the store's `files` it sends): answers it as `outbox_list` lists
/// it, or why it cannot wait. The host sends it when it can, and the pages hear of each change.
#[tauri::command(async)]
pub(crate) fn outbox_add(
    app: AppHandle,
    webview: Webview,
    kind: String,
    payload: Value,
    files: Option<Vec<String>>,
) -> Result<Value, String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let id = caller(&client, &webview)?;
    let op = check(&kind, &payload, files.as_deref().unwrap_or(&[]), now_ms())?;
    let (kept, added) = on_store(&app, &webview, |_, store| {
        if op
            .files
            .iter()
            .any(|name| store.file_path(name).is_none_or(|path| !path.is_file()))
        {
            return Err("An operation's file is not kept".to_string());
        }
        add(store, op)
    })?;
    if added {
        notify_pages(&app, &id, &json!({ "kind": "changed" }));
        client.flusher.wake();
    }
    Ok(op_json(&kept))
}

/// The account's operations waiting to be sent, or refused, oldest first.
#[tauri::command(async)]
pub(crate) fn outbox_list(app: AppHandle, webview: Webview) -> Result<Value, String> {
    on_store(&app, &webview, |_, store| Ok(list_json(store)))
}

/// Send an operation again now (one refused, held or waiting); false when there is none.
#[tauri::command(async)]
pub(crate) fn outbox_retry(app: AppHandle, webview: Webview, id: String) -> Result<bool, String> {
    let (account, found) = on_store(&app, &webview, |account, store| {
        Ok((account.to_string(), retry(store, &id, now_ms())?))
    })?;
    if found {
        notify_pages(&app, &account, &json!({ "kind": "changed" }));
        if let Some(client) = app.try_state::<Client>() {
            client.flusher.wake();
        }
    }
    Ok(found)
}

/// Remove an operation and its files without sending it; false when there was none.
#[tauri::command(async)]
pub(crate) fn outbox_discard(app: AppHandle, webview: Webview, id: String) -> Result<bool, String> {
    let (account, removed) = on_store(&app, &webview, |account, store| {
        if store.outbox_get(&id).is_some_and(|op| op.state == OutboxState::Sending) {
            return Err("It is being sent".to_string());
        }
        Ok((
            account.to_string(),
            store.outbox_remove(&id).map_err(|err| err.to_string())?,
        ))
    })?;
    if removed {
        notify_pages(&app, &account, &json!({ "kind": "changed" }));
    }
    Ok(removed)
}

/// What a sign-out of a shared device would delete unsent: `{operations, drafts}` (the shell's confirm, MD4 stage 5).
#[tauri::command(async)]
pub(crate) fn outbox_unsent(app: AppHandle, webview: Webview) -> Result<Value, String> {
    on_store(&app, &webview, |_, store| Ok(unsent_json(store)))
}

/// A draft key a page may use: not empty, short, no control characters.
fn draft_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > KEY_MAX || key.chars().any(char::is_control) {
        return Err("Not a draft key".to_string());
    }
    Ok(())
}

/// The draft of `key`: `{key, value, files, updatedMs}`, or null.
#[tauri::command(async)]
pub(crate) fn draft_get(app: AppHandle, webview: Webview, key: String) -> Result<Value, String> {
    draft_key(&key)?;
    on_store(&app, &webview, |_, store| {
        Ok(store.draft(&key).as_ref().map_or(Value::Null, draft_json))
    })
}

/// The drafts whose key starts with `prefix`, by key.
#[tauri::command(async)]
pub(crate) fn draft_list(app: AppHandle, webview: Webview, prefix: String) -> Result<Value, String> {
    if prefix.len() > KEY_MAX {
        return Err("Not a draft key".to_string());
    }
    on_store(&app, &webview, |_, store| {
        Ok(Value::Array(store.drafts(&prefix).iter().map(draft_json).collect()))
    })
}

/// Keep `value` (text, usually JSON) as the draft of `key`, with the store's `files` attached to it.
#[tauri::command(async)]
pub(crate) fn draft_put(
    app: AppHandle,
    webview: Webview,
    key: String,
    value: String,
    files: Option<Vec<String>>,
) -> Result<(), String> {
    draft_key(&key)?;
    if value.len() > DRAFT_BYTES {
        return Err("The draft is too large to keep".to_string());
    }
    let files = files.unwrap_or_default();
    if files.len() > FILES_MAX {
        return Err("A draft keeps at most 20 files".to_string());
    }
    on_store(&app, &webview, |_, store| {
        draft_room(store, &key, value.len())?;
        store.put_draft(&key, &value, &files).map_err(|err| err.to_string())
    })
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

/// Forget the draft of `key` and its files; false when there was none.
#[tauri::command(async)]
pub(crate) fn draft_remove(app: AppHandle, webview: Webview, key: String) -> Result<bool, String> {
    draft_key(&key)?;
    on_store(&app, &webview, |_, store| {
        store.remove_draft(&key).map_err(|err| err.to_string())
    })
}

/// Keep a file of a draft or a send (`data` in base64), named after `name`: answers the store's name for it, which a
/// draft or an operation is to refer to soon; a file nothing refers to is removed after a grace (`Store::gc_files`,
/// also run by each pass of the sender). The files in all are capped at a quarter of the store's size cap.
#[tauri::command(async)]
pub(crate) fn file_save(app: AppHandle, webview: Webview, name: String, data: String) -> Result<String, String> {
    if data.len() > FILE_BYTES / 3 * 4 + 4 {
        return Err("The file is over 25 MB".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|_| "The file is not base64".to_string())?;
    let max_bytes = app
        .try_state::<Client>()
        .map_or(0, |client| client.conf().store_max_bytes());
    on_store(&app, &webview, |_, store| {
        file_room(store, bytes.len(), max_bytes)?;
        store.save_file(&name, &bytes).map_err(|err| err.to_string())
    })
}

#[cfg(test)]
#[path = "outbox_tests.rs"]
mod tests;
