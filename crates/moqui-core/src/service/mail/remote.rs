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

//! The local changes of an account's mail on its IMAP server.
//!
//! `push#RemoteChanges` (internal) queues each change as an `EmailRemoteChange` row in the caller's transaction, so a
//! change that rolls back is not queued and one that commits is not lost while the server is away. Once it commits a
//! push-only session runs on a service worker (`email-client` `sync-push-now`, on by default); otherwise the account's next
//! sync makes it. A change is made from the message's state when it is applied, not when it was queued: a flag change
//! stores the flags it named as they are now, a move takes the message from where it is on the server
//! (`remoteFolderId`) to its folder here, a label change stores the labels as keywords, a sent message is appended to
//! Sent (unless the server keeps a copy of what it sends, found by Message-ID), a draft saved here is appended to Drafts
//! (replacing its copy there; [`apply_draft`]). A message whose UID is not known (a move
//! without UIDPLUS) is found by Message-ID. A change that cannot be made yet waits; one that fails
//! [`MAX_ATTEMPTS`] times is dropped and logged.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use moqui_lib::error::Error as LibError;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use super::implementation::{
    build_lettre, client_headers, find_one, force_new, new_message_id, note_error, split_addresses, stored_attachments,
    Letter,
};
use super::inbound::stamp_millis;
use super::store::{update_fields, MESSAGE};
use super::sync::{
    connection_lost, encode_mutf7, label_keyword, load_account, load_folders, quoted, run_sync, search_message_id,
    select, update_folder, value_millis, value_number, FolderInfo, ImapConn, SyncFailure, SyncOptions, CHANGE, FOLDER,
};
use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::entity::{Condition, EntityFacade, EntityValue};
use crate::service::util::{field_text, now_millis, param_text};

/// Tries before a change that keeps failing is dropped.
pub(super) const MAX_ATTEMPTS: i64 = 10;

const MESSAGE_CHANGES: &[&str] = &["flags", "move", "labels", "keywords", "draft"];

// ---- queueing ----

fn text_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .filter(|text| !text.is_empty())
            .collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => {
            let text = to_plain_string(other);
            text.split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect()
        }
    }
}

fn json_of(value: &serde_json::Value) -> Value {
    Value::string(value.to_string())
}

fn queue(facade: &EntityFacade, account_id: &str, change_type: &str, fields: &[(&str, Value)]) -> Result<(), LibError> {
    let mut row = facade.make_value(CHANGE)?;
    row.set_sequenced_id_primary(facade)?;
    row.set("emailAccountId", Value::string(account_id));
    row.set("changeType", Value::string(change_type));
    row.set("createdDate", stamp_millis(now_millis()));
    row.set("attemptCount", Value::I64(0));
    for (name, value) in fields {
        row.set(*name, value.clone());
    }
    row.create(facade)
}

/// `push#RemoteChanges`: queue the server side of a local change of an IMAP account (internal: the filing services
/// call it with authorization disabled). Nothing for an account without an IMAP server, or for a message kept only here.
pub(super) fn push_changes(
    parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let out = BTreeMap::new();
    if !ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false) {
        note_error("push#RemoteChanges is internal");
        return Ok(out);
    }
    let account_id = param_text(parameters, "emailAccountId");
    let change_type = param_text(parameters, "changeType");
    let Some(account) = load_account(facade, &account_id)? else {
        return Ok(out);
    };
    if !account.server.as_ref().is_some_and(super::sync::ServerInfo::is_imap) {
        return Ok(out);
    }
    let ids = text_list(parameters.get("emailMessageIdList"));
    let mut queued = 0;
    match change_type.as_str() {
        "flags" | "move" | "labels" | "keywords" | "delete" | "sent" | "draft" => {
            let change_json = match change_type.as_str() {
                "flags" => {
                    let names: Vec<String> = match parameters.get("flagMap") {
                        Some(Value::Map(map)) => map.keys().cloned().collect(),
                        _ => vec!["isRead".into(), "isFlagged".into(), "isAnswered".into()],
                    };
                    Some(serde_json::json!({ "fields": names }))
                }
                "keywords" => Some(serde_json::json!({
                    "add": text_list(parameters.get("addKeywordList")),
                    "remove": text_list(parameters.get("removeKeywordList")),
                })),
                _ => None,
            };
            for id in &ids {
                let Some(message) = find_one(facade, MESSAGE, "emailMessageId", id)? else {
                    continue;
                };
                let remote_folder = field_text(&message, "remoteFolderId");
                // a message kept only here has nothing on the server, but a sent one or a draft is appended there
                if remote_folder.is_empty() && change_type != "sent" && change_type != "draft" {
                    continue;
                }
                // one draft change waiting is enough, since it appends the draft as it is then; a draft sent before
                // its change went has nothing to append
                if change_type == "draft" || change_type == "sent" {
                    let waiting = facade
                        .find(CHANGE)?
                        .eq("emailAccountId", Value::string(&account_id))
                        .eq("emailMessageId", Value::string(id))
                        .eq("changeType", Value::string("draft"));
                    if change_type == "draft" {
                        if waiting.count()? > 0 {
                            continue;
                        }
                    } else {
                        waiting.delete_all()?;
                    }
                }
                let mut fields = vec![("emailMessageId", Value::string(id))];
                if let Some(json) = &change_json {
                    fields.push(("changeJson", json_of(json)));
                }
                fields.push(("messageId", message.get("messageId").cloned().unwrap_or(Value::Null)));
                if change_type == "delete" {
                    // the row goes now: where the message is on the server is kept with the change
                    let folder = find_one(facade, FOLDER, "emailFolderId", &remote_folder)?;
                    fields.push(("remoteFolderId", Value::string(&remote_folder)));
                    fields.push(("remoteUid", message.get("remoteUid").cloned().unwrap_or(Value::Null)));
                    fields.push((
                        "remoteUidValidity",
                        folder
                            .and_then(|folder| folder.get("remoteUidValidity").cloned())
                            .unwrap_or(Value::Null),
                    ));
                }
                queue(facade, &account_id, &change_type, &fields)?;
                queued += 1;
            }
        }
        "createFolder" | "renameFolder" => {
            let folder_id = param_text(parameters, "emailFolderId");
            if !folder_id.is_empty() {
                let json = serde_json::json!({ "oldName": param_text(parameters, "oldName") });
                queue(
                    facade,
                    &account_id,
                    &change_type,
                    &[
                        ("emailFolderId", Value::string(&folder_id)),
                        ("changeJson", json_of(&json)),
                    ],
                )?;
                queued += 1;
            }
        }
        other => {
            note_error(format!("Unknown remote change {other}"));
            return Ok(out);
        }
    }
    if queued > 0 {
        push_after_commit(facade, &account_id);
    }
    Ok(out)
}

/// In tests a push waits for the next sync unless a test asks for it now.
#[cfg(test)]
pub(super) static PUSH_NOW_IN_TESTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn push_now() -> bool {
    #[cfg(test)]
    if !PUSH_NOW_IN_TESTS.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    !super::client_setting("sync-push-now").eq_ignore_ascii_case("false")
}

/// After the caller's commit, a push-only session for the account on a service worker.
fn push_after_commit(facade: &EntityFacade, account_id: &str) {
    if !push_now() {
        return;
    }
    let Some(factory) = ExecutionContext::with_current(|ec| Arc::downgrade(ec.factory())) else {
        return;
    };
    let account_id = account_id.to_string();
    let request = {
        let account_id = account_id.clone();
        move || {
            if let Some(factory) = factory.upgrade() {
                request_push(&factory, account_id);
            }
        }
    };
    match facade.default_transactions() {
        Ok(tx) if tx.in_transaction() => {
            if let Err(err) = tx.on_commit(request) {
                tracing::warn!(
                    "Could not wait for the commit to push the changes of email account {account_id}: {err}"
                );
            }
        }
        _ => request(),
    }
}

/// The accounts with a push waiting to run, so a burst of changes makes one session.
fn queued_pushes() -> &'static Mutex<HashSet<String>> {
    static QUEUED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    QUEUED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn request_push(factory: &Arc<ExecutionContextFactory>, account_id: String) {
    if !lock(queued_pushes()).insert(account_id.clone()) {
        return;
    }
    let weak: Weak<ExecutionContextFactory> = Arc::downgrade(factory);
    factory.service_pools().spawn("email remote changes", move || {
        lock(queued_pushes()).remove(&account_id);
        let Some(factory) = weak.upgrade() else {
            return Ok(());
        };
        let ctx = factory.new_context();
        let _entered = ctx.enter();
        let _authz = ctx.disable_authz();
        let Some(facade) = factory.entity() else {
            return Ok(());
        };
        let options = SyncOptions {
            push_only: true,
            ..SyncOptions::default()
        };
        // busy: the sync running now makes the changes at its end
        let _ = run_sync(&facade, &account_id, &options);
        Ok(())
    });
}

/// The messages of an account with a change still queued.
pub(super) fn pending_message_ids(facade: &EntityFacade, account_id: &str) -> Result<HashSet<String>, LibError> {
    let rows = facade
        .find(CHANGE)?
        .eq("emailAccountId", Value::string(account_id))
        .condition(Condition::is_in(
            "changeType",
            MESSAGE_CHANGES.iter().map(|name| Value::string(*name)).collect(),
        ))
        .select_field("emailMessageId")
        .list()?;
    Ok(rows
        .values()
        .iter()
        .map(|row| field_text(row, "emailMessageId"))
        .filter(|id| !id.is_empty())
        .collect())
}

// ---- applying ----

enum Outcome {
    Done,
    /// Not now (no UID yet, a folder not on the server yet): tried again later.
    Later(String),
    /// Cannot be made at all: dropped.
    Drop(String),
}

/// Make an account's queued changes, oldest first; returns how many were made. A connection lost ends it, with the
/// rest still queued.
pub(super) fn apply_queue(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    account: &super::sync::AccountInfo,
) -> Result<usize, SyncFailure> {
    let lost = |err: LibError| SyncFailure::Connect(err.to_string());
    let rows = facade
        .find(CHANGE)
        .and_then(|find| find.eq("emailAccountId", Value::string(&account.id)).list())
        .map_err(lost)?;
    if rows.values().is_empty() {
        return Ok(0);
    }
    let mut changes: Vec<EntityValue> = rows.values().to_vec();
    changes.sort_by_key(|row| {
        let id = field_text(row, "emailRemoteChangeId");
        (
            value_millis(row.get("createdDate")).unwrap_or(0),
            id.parse::<u64>().unwrap_or(u64::MAX),
            id,
        )
    });
    let mut folders = load_folders(facade, &account.id).map_err(lost)?;
    let mut applied = 0;
    for change in changes {
        let change_id = field_text(&change, "emailRemoteChangeId");
        let change_type = field_text(&change, "changeType");
        let outcome = match change_type.as_str() {
            "flags" => apply_flags(conn, facade, &folders, &change),
            "move" => apply_move(conn, facade, &folders, &change),
            "labels" => apply_labels(conn, facade, &folders, &change),
            "keywords" => apply_keywords(conn, facade, &folders, &change),
            "delete" => apply_delete(conn, &folders, &change),
            "sent" => apply_sent(conn, facade, &folders, &change),
            "draft" => apply_draft(conn, facade, &folders, &change),
            "createFolder" => apply_create_folder(conn, facade, &folders, &change),
            "renameFolder" => apply_rename_folder(conn, facade, &folders, &change),
            other => Ok(Outcome::Drop(format!("unknown change {other}"))),
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(ApplyError::Imap(err)) if connection_lost(&err) => {
                return Err(SyncFailure::Connect(format!(
                    "IMAP connection lost making change {change_id}: {err}"
                )))
            }
            Err(ApplyError::Imap(err)) => Outcome::Later(err.to_string()),
            Err(ApplyError::Local(err)) => Outcome::Later(err.to_string()),
        };
        let attempts = value_number(change.get("attemptCount")).unwrap_or(0) + 1;
        let outcome = match outcome {
            Outcome::Later(reason) if attempts >= MAX_ATTEMPTS => {
                Outcome::Drop(format!("still failing after {attempts} tries: {reason}"))
            }
            other => other,
        };
        let written = match outcome {
            Outcome::Done => {
                applied += 1;
                if change_type.ends_with("Folder") {
                    folders = load_folders(facade, &account.id).map_err(lost)?;
                }
                delete_change(facade, &change_id)
            }
            Outcome::Drop(reason) => {
                tracing::error!(
                    account = %account.id,
                    "Dropping the {change_type} change {change_id} of email account {} (message {}): {reason}",
                    account.id,
                    field_text(&change, "emailMessageId")
                );
                delete_change(facade, &change_id)
            }
            Outcome::Later(reason) => {
                tracing::warn!(
                    account = %account.id,
                    "The {change_type} change {change_id} of email account {} waits: {reason}",
                    account.id
                );
                force_new(facade, || {
                    let mut row = facade.make_value(CHANGE)?;
                    row.set("emailRemoteChangeId", Value::string(&change_id));
                    row.set("attemptCount", Value::I64(attempts));
                    row.set("lastAttemptDate", stamp_millis(now_millis()));
                    row.set("lastError", Value::string(super::sync::clip(&reason, 4000)));
                    row.update(facade)
                })
            }
        };
        if let Err(err) = written {
            tracing::error!("Could not record email remote change {change_id}: {err}");
        }
    }
    Ok(applied)
}

fn delete_change(facade: &EntityFacade, change_id: &str) -> Result<(), LibError> {
    force_new(facade, || {
        facade
            .find(CHANGE)?
            .eq("emailRemoteChangeId", Value::string(change_id))
            .delete_all()
            .map(|_| ())
    })
}

enum ApplyError {
    Imap(imap::Error),
    Local(LibError),
}

impl From<imap::Error> for ApplyError {
    fn from(err: imap::Error) -> Self {
        Self::Imap(err)
    }
}

impl From<LibError> for ApplyError {
    fn from(err: LibError) -> Self {
        Self::Local(err)
    }
}

fn change_json(change: &EntityValue) -> serde_json::Value {
    serde_json::from_str(&field_text(change, "changeJson")).unwrap_or(serde_json::Value::Null)
}

fn json_strings(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn folder_by_id<'a>(folders: &'a [FolderInfo], id: &str) -> Option<&'a FolderInfo> {
    folders.iter().find(|folder| folder.id == id && !id.is_empty())
}

/// Where a message is on the server now: its folder (selected) and UID, found by Message-ID when the UID is not
/// known (and kept). None for a message kept only here or not found there.
fn locate(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    message: &EntityValue,
) -> Result<Option<(FolderInfo, u32)>, ApplyError> {
    let Some(folder) = folder_by_id(folders, &field_text(message, "remoteFolderId")).cloned() else {
        return Ok(None);
    };
    if folder.path.is_empty() || folder.removed {
        return Ok(None);
    }
    let selected = select(conn, &folder.path, false)?;
    let valid = folder.uid_validity.is_none() || selected.mailbox.uid_validity == folder.uid_validity;
    let known = value_number(message.get("remoteUid"))
        .and_then(|uid| u32::try_from(uid).ok())
        .filter(|_| valid);
    let uid = match known {
        Some(uid) => Some(uid),
        None => {
            let found = search_message_id(conn, &field_text(message, "messageId"))?;
            if let Some(uid) = found {
                let id = field_text(message, "emailMessageId");
                force_new(facade, || {
                    update_fields(facade, &id, &[("remoteUid", Value::I64(i64::from(uid)))])
                })?;
            }
            found
        }
    };
    Ok(uid.map(|uid| (folder, uid)))
}

fn not_located(message: &EntityValue) -> Outcome {
    if field_text(message, "remoteFolderId").is_empty() {
        Outcome::Done
    } else {
        Outcome::Later("the message is not found on the server".to_string())
    }
}

fn store_flags(conn: &mut ImapConn, uid: u32, add: &[String], remove: &[String]) -> Result<(), imap::Error> {
    if !add.is_empty() {
        conn.session
            .uid_store(uid.to_string(), format!("+FLAGS.SILENT ({})", add.join(" ")))?;
    }
    if !remove.is_empty() {
        conn.session
            .uid_store(uid.to_string(), format!("-FLAGS.SILENT ({})", remove.join(" ")))?;
    }
    Ok(())
}

fn load_message(facade: &EntityFacade, change: &EntityValue) -> Result<Option<EntityValue>, LibError> {
    find_one(facade, MESSAGE, "emailMessageId", &field_text(change, "emailMessageId"))
}

fn apply_flags(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(message) = load_message(facade, change)? else {
        return Ok(Outcome::Done);
    };
    let Some((_, uid)) = locate(conn, facade, folders, &message)? else {
        return Ok(not_located(&message));
    };
    let mut fields = json_strings(&change_json(change), "fields");
    if fields.is_empty() {
        fields = vec!["isRead".into(), "isFlagged".into(), "isAnswered".into()];
    }
    let (mut add, mut remove) = (Vec::new(), Vec::new());
    for field in fields {
        let flag = match field.as_str() {
            "isRead" => "\\Seen",
            "isFlagged" => "\\Flagged",
            "isAnswered" => "\\Answered",
            _ => continue,
        };
        if field_text(&message, &field) == "Y" {
            add.push(flag.to_string());
        } else {
            remove.push(flag.to_string());
        }
    }
    store_flags(conn, uid, &add, &remove)?;
    Ok(Outcome::Done)
}

/// Delete the message at this UID of the selected folder: `\Deleted`, then `UID EXPUNGE` (UIDPLUS) or `EXPUNGE`.
fn expunge_uid(conn: &mut ImapConn, uid: u32) -> Result<(), imap::Error> {
    conn.session.uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")?;
    if conn.caps.has("UIDPLUS") {
        conn.session.uid_expunge(uid.to_string())?;
    } else {
        conn.session.expunge()?;
    }
    Ok(())
}

/// The UID a COPYUID response code gives the message at `source` UID.
fn copy_uid(response: &[u8], source: u32) -> Option<u32> {
    let text = String::from_utf8_lossy(response);
    let start = text.to_ascii_uppercase().find("[COPYUID ")?;
    let rest = &text[start + 9..];
    let end = rest.find(']')?;
    let parts: Vec<&str> = rest[..end].split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let sources = super::sync::expand_set(parts[1]);
    let targets = super::sync::expand_set(parts[2]);
    let position = sources.iter().position(|uid| *uid == source)?;
    targets.get(position).copied()
}

fn apply_move(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(message) = load_message(facade, change)? else {
        return Ok(Outcome::Done);
    };
    let here = field_text(&message, "emailFolderId");
    let there = field_text(&message, "remoteFolderId");
    if there.is_empty() || here.is_empty() || here == there {
        return Ok(Outcome::Done);
    }
    let Some(target) = folder_by_id(folders, &here).cloned() else {
        return Ok(Outcome::Done);
    };
    if target.path.is_empty() || target.removed {
        return Ok(Outcome::Later(format!(
            "folder {} is not on the server yet",
            target.name
        )));
    }
    let Some((_, uid)) = locate(conn, facade, folders, &message)? else {
        return Ok(not_located(&message));
    };
    let destination = quoted(&target.path);
    let new_uid = if conn.caps.has("MOVE") {
        let (response, _) = conn.session.run(format!("UID MOVE {uid} {destination}"))?;
        copy_uid(&response, uid)
    } else {
        let (response, _) = conn.session.run(format!("UID COPY {uid} {destination}"))?;
        let copied = copy_uid(&response, uid);
        expunge_uid(conn, uid)?;
        copied
    };
    let id = field_text(&message, "emailMessageId");
    force_new(facade, || {
        update_fields(
            facade,
            &id,
            &[
                ("remoteFolderId", Value::string(&target.id)),
                (
                    "remoteUid",
                    new_uid.map_or(Value::Null, |uid| Value::I64(i64::from(uid))),
                ),
            ],
        )
    })?;
    Ok(Outcome::Done)
}

fn apply_delete(conn: &mut ImapConn, folders: &[FolderInfo], change: &EntityValue) -> Result<Outcome, ApplyError> {
    let Some(folder) = folder_by_id(folders, &field_text(change, "remoteFolderId")).cloned() else {
        return Ok(Outcome::Done);
    };
    if folder.path.is_empty() || folder.removed {
        return Ok(Outcome::Done);
    }
    let selected = select(conn, &folder.path, false)?;
    let validity = value_number(change.get("remoteUidValidity")).and_then(|number| u32::try_from(number).ok());
    let valid = validity.is_none() || selected.mailbox.uid_validity == validity;
    let known = value_number(change.get("remoteUid"))
        .and_then(|uid| u32::try_from(uid).ok())
        .filter(|_| valid);
    let uid = match known {
        Some(uid) => Some(uid),
        None => search_message_id(conn, &field_text(change, "messageId"))?,
    };
    if let Some(uid) = uid {
        expunge_uid(conn, uid)?;
    }
    Ok(Outcome::Done)
}

fn apply_labels(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(message) = load_message(facade, change)? else {
        return Ok(Outcome::Done);
    };
    let Some((_, uid)) = locate(conn, facade, folders, &message)? else {
        return Ok(not_located(&message));
    };
    if !conn.selected.as_ref().is_some_and(super::sync::Selected::keywords) {
        return Ok(Outcome::Done);
    }
    let account_id = field_text(&message, "emailAccountId");
    let names: HashMap<String, String> = facade
        .find("moqui.basic.email.EmailLabel")?
        .eq("emailAccountId", Value::string(&account_id))
        .list()?
        .values()
        .iter()
        .map(|row| {
            (
                field_text(row, "emailLabelId"),
                label_keyword(&field_text(row, "labelName")),
            )
        })
        .collect();
    let have: HashSet<String> = facade
        .find("moqui.basic.email.EmailMessageLabel")?
        .eq("emailMessageId", Value::string(field_text(&message, "emailMessageId")))
        .list()?
        .values()
        .iter()
        .filter_map(|row| names.get(&field_text(row, "emailLabelId")).cloned())
        .collect();
    let mut add: Vec<String> = have.iter().cloned().collect();
    let mut remove: Vec<String> = names
        .values()
        .filter(|keyword| !have.contains(*keyword))
        .cloned()
        .collect();
    add.sort();
    remove.sort();
    remove.dedup();
    store_flags(conn, uid, &add, &remove)?;
    Ok(Outcome::Done)
}

fn apply_keywords(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(message) = load_message(facade, change)? else {
        return Ok(Outcome::Done);
    };
    let Some((_, uid)) = locate(conn, facade, folders, &message)? else {
        return Ok(not_located(&message));
    };
    if !conn.selected.as_ref().is_some_and(super::sync::Selected::keywords) {
        return Ok(Outcome::Done);
    }
    let json = change_json(change);
    let atom = |keyword: &String| !keyword.is_empty() && !keyword.contains([' ', '(', ')', '"', '\\', '\r', '\n']);
    let add: Vec<String> = json_strings(&json, "add").into_iter().filter(atom).collect();
    let remove: Vec<String> = json_strings(&json, "remove").into_iter().filter(atom).collect();
    store_flags(conn, uid, &add, &remove)?;
    Ok(Outcome::Done)
}

/// The RFC 822 text of a sent message as it went out, for the copy in Sent.
fn sent_copy(facade: &EntityFacade, message: &EntityValue) -> Result<Vec<u8>, LibError> {
    let id = field_text(message, "emailMessageId");
    let mut reply_to = Vec::new();
    if let Some(identity) = find_one(
        facade,
        "moqui.basic.email.EmailIdentity",
        "emailIdentityId",
        &field_text(message, "emailIdentityId"),
    )? {
        reply_to = split_addresses(&field_text(&identity, "replyToAddress"));
    }
    let mut letter = Letter {
        subject: field_text(message, "subject"),
        from_address: field_text(message, "fromAddress"),
        from_name: field_text(message, "fromName"),
        to: split_addresses(&field_text(message, "toAddresses")),
        cc: split_addresses(&field_text(message, "ccAddresses")),
        bcc: Vec::new(),
        reply_to,
        bounce: String::new(),
        body: field_text(message, "body"),
        body_text: field_text(message, "bodyText"),
        attachments: Vec::new(),
        inline: Vec::new(),
        message_id: field_text(message, "messageId"),
        in_reply_to: field_text(message, "inReplyTo"),
        references: field_text(message, "referencesHeader"),
        date: value_millis(message.get("sentDate")),
        extra_headers: client_headers(&field_text(message, "headersString")),
    };
    stored_attachments(facade, &id, &mut letter).map_err(|err| LibError::new(err.message()))?;
    let built = build_lettre(&letter).map_err(|err| LibError::new(err.message()))?;
    Ok(built.formatted())
}

/// The UID of an `Appended`: the crate keeps it as a type it does not export, so it is read from its `Debug` text.
fn appended_uid(appended: &imap::types::Appended) -> Option<u32> {
    let text = format!("{:?}", appended.uids.as_ref()?);
    let digits: String = text
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn apply_sent(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(message) = load_message(facade, change)? else {
        return Ok(Outcome::Done);
    };
    if field_text(&message, "statusId") != "ES_SENT" {
        return Ok(Outcome::Done);
    }
    let id = field_text(&message, "emailMessageId");
    // a draft that came from the server's Drafts goes from there
    if let Some(drafts) = folder_by_id(folders, &field_text(&message, "remoteFolderId")) {
        if drafts.kind == "EfkDrafts" {
            if let Some((_, uid)) = locate(conn, facade, folders, &message)? {
                expunge_uid(conn, uid)?;
            }
            force_new(facade, || {
                update_fields(
                    facade,
                    &id,
                    &[("remoteFolderId", Value::Null), ("remoteUid", Value::Null)],
                )
            })?;
        }
    }
    let Some(sent) = folders
        .iter()
        .find(|folder| folder.kind == "EfkSent" && !folder.path.is_empty() && !folder.removed)
        .cloned()
    else {
        return Ok(Outcome::Drop(
            "the account has no Sent folder on the server".to_string(),
        ));
    };
    select(conn, &sent.path, false)?;
    let message_id = field_text(&message, "messageId");
    // a server that keeps a copy of what it sends (Gmail, Microsoft 365) has it already
    let uid = match search_message_id(conn, &message_id)? {
        Some(uid) => Some(uid),
        None => {
            let bytes = match sent_copy(facade, &message) {
                Ok(bytes) => bytes,
                Err(err) => return Ok(Outcome::Drop(format!("could not make the sent copy: {err}"))),
            };
            let appended = conn
                .session
                .append(&sent.path, &bytes)
                .flag(imap::types::Flag::Seen)
                .finish()?;
            // the APPEND changed the folder, which is selected again before use
            conn.selected = None;
            appended_uid(&appended)
        }
    };
    force_new(facade, || {
        update_fields(
            facade,
            &id,
            &[
                ("remoteFolderId", Value::string(&sent.id)),
                ("remoteUid", uid.map_or(Value::Null, |uid| Value::I64(i64::from(uid)))),
            ],
        )
    })?;
    Ok(Outcome::Done)
}

/// A draft saved here, appended to the server's Drafts (`\Draft`, `\Seen`) so another client sees it, and the copy
/// appended before it (an earlier save) expunged there: the new copy goes first, so the server is never left without
/// it. The draft gets a Message-ID the first time, which the sync finds it by, and which its send keeps. A draft sent,
/// scheduled or deleted since has nothing to append (the sent change takes its copy out of Drafts).
fn apply_draft(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(message) = load_message(facade, change)? else {
        return Ok(Outcome::Done);
    };
    if field_text(&message, "statusId") != "ES_DRAFT" {
        return Ok(Outcome::Done);
    }
    let Some(drafts) = folders
        .iter()
        .find(|folder| folder.kind == "EfkDrafts" && !folder.path.is_empty() && !folder.removed)
        .cloned()
    else {
        return Ok(Outcome::Drop(
            "the account has no Drafts folder on the server".to_string(),
        ));
    };
    let id = field_text(&message, "emailMessageId");
    let mut message_id = field_text(&message, "messageId");
    if message_id.trim().is_empty() {
        message_id =
            new_message_id(&field_text(&message, "fromAddress")).map_err(|err| LibError::new(err.message()))?;
        force_new(facade, || {
            update_fields(facade, &id, &[("messageId", Value::string(&message_id))])
        })?;
    }
    // the copy appended before, if the sync knows where it is
    let earlier = match folder_by_id(folders, &field_text(&message, "remoteFolderId")) {
        Some(folder) if folder.kind == "EfkDrafts" => locate(conn, facade, folders, &message)?,
        _ => None,
    };
    let mut copy = message.clone();
    copy.set("messageId", Value::string(&message_id));
    copy.set("sentDate", message.get("messageDate").cloned().unwrap_or(Value::Null));
    let bytes = match sent_copy(facade, &copy) {
        Ok(bytes) => bytes,
        Err(err) => return Ok(Outcome::Drop(format!("could not make the draft's copy: {err}"))),
    };
    let appended = conn
        .session
        .append(&drafts.path, &bytes)
        .flag(imap::types::Flag::Seen)
        .flag(imap::types::Flag::Draft)
        .finish()?;
    conn.selected = None;
    let uid = appended_uid(&appended);
    force_new(facade, || {
        update_fields(
            facade,
            &id,
            &[
                ("remoteFolderId", Value::string(&drafts.id)),
                ("remoteUid", uid.map_or(Value::Null, |uid| Value::I64(i64::from(uid)))),
            ],
        )
    })?;
    if let Some((folder, old_uid)) = earlier {
        if uid != Some(old_uid) || folder.id != drafts.id {
            let removed = select(conn, &folder.path, false).and_then(|_| expunge_uid(conn, old_uid));
            match removed {
                Err(err) if connection_lost(&err) => return Err(ApplyError::Imap(err)),
                Err(err) => tracing::warn!(
                    "The earlier copy of draft {id} (UID {old_uid} of {}) stays on the server: {err}",
                    folder.path
                ),
                Ok(()) => {}
            }
        }
    }
    Ok(Outcome::Done)
}

/// The server path of a folder made or renamed here: under its parent's path, its name in modified UTF-7.
fn remote_path(folders: &[FolderInfo], folder: &EntityValue) -> Result<Option<(String, String)>, LibError> {
    let name = field_text(folder, "folderName");
    let parent_id = field_text(folder, "parentFolderId");
    let any_delimiter = folders
        .iter()
        .map(|known| known.delimiter.clone())
        .find(|delimiter| !delimiter.is_empty())
        .unwrap_or_else(|| "/".to_string());
    if parent_id.is_empty() {
        return Ok(Some((encode_mutf7(&name), any_delimiter)));
    }
    let Some(parent) = folder_by_id(folders, &parent_id) else {
        return Ok(Some((encode_mutf7(&name), any_delimiter)));
    };
    if parent.path.is_empty() {
        return Ok(None);
    }
    let delimiter = if parent.delimiter.is_empty() {
        any_delimiter
    } else {
        parent.delimiter.clone()
    };
    Ok(Some((
        format!("{}{delimiter}{}", parent.path, encode_mutf7(&name)),
        delimiter,
    )))
}

fn apply_create_folder(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(folder) = find_one(facade, FOLDER, "emailFolderId", &field_text(change, "emailFolderId"))? else {
        return Ok(Outcome::Done);
    };
    if !field_text(&folder, "folderPath").is_empty() {
        return Ok(Outcome::Done);
    }
    let Some((path, delimiter)) = remote_path(folders, &folder)? else {
        return Ok(Outcome::Later("its parent folder is not on the server yet".to_string()));
    };
    if let Err(err) = conn.session.create(&path) {
        let exists = matches!(&err, imap::Error::No(no) if no.information.to_ascii_lowercase().contains("exist")
            || format!("{:?}", no.code).to_ascii_uppercase().contains("ALREADYEXISTS"));
        if !exists {
            return Err(err.into());
        }
    }
    if let Err(err) = conn.session.subscribe(&path) {
        if connection_lost(&err) {
            return Err(err.into());
        }
    }
    let folder_id = field_text(&folder, "emailFolderId");
    force_new(facade, || {
        update_folder(
            facade,
            &folder_id,
            &[
                ("folderPath", Value::string(&path)),
                ("delimiter", Value::string(&delimiter)),
            ],
        )
    })?;
    Ok(Outcome::Done)
}

fn apply_rename_folder(
    conn: &mut ImapConn,
    facade: &EntityFacade,
    folders: &[FolderInfo],
    change: &EntityValue,
) -> Result<Outcome, ApplyError> {
    let Some(folder) = find_one(facade, FOLDER, "emailFolderId", &field_text(change, "emailFolderId"))? else {
        return Ok(Outcome::Done);
    };
    let old_path = field_text(&folder, "folderPath");
    if old_path.is_empty() {
        return Ok(Outcome::Done);
    }
    let Some((new_path, delimiter)) = remote_path(folders, &folder)? else {
        return Ok(Outcome::Later("its parent folder is not on the server yet".to_string()));
    };
    if new_path == old_path {
        return Ok(Outcome::Done);
    }
    if conn.selected.as_ref().is_some_and(|selected| selected.path == old_path) {
        conn.session.close()?;
        conn.selected = None;
    }
    conn.session.rename(&old_path, &new_path)?;
    let _ = conn.session.subscribe(&new_path);
    let folder_id = field_text(&folder, "emailFolderId");
    let prefix = format!("{old_path}{delimiter}");
    force_new(facade, || {
        update_folder(facade, &folder_id, &[("folderPath", Value::string(&new_path))])?;
        // the server moved the folders under it too
        for child in folders.iter().filter(|known| known.path.starts_with(&prefix)) {
            let moved = format!("{new_path}{delimiter}{}", &child.path[prefix.len()..]);
            update_folder(facade, &child.id, &[("folderPath", Value::string(&moved))])?;
        }
        Ok(())
    })?;
    Ok(Outcome::Done)
}

#[cfg(test)]
mod tests {
    use super::copy_uid;

    #[test]
    fn copy_uid_maps_the_source_to_the_target() {
        assert_eq!(
            copy_uid(b"* OK [COPYUID 38505 304,319:320 3956:3958] Done\r\n", 319),
            Some(3957)
        );
        assert_eq!(copy_uid(b"a1 OK [COPYUID 1 5 9] done", 5), Some(9));
        assert_eq!(copy_uid(b"a1 OK done", 5), None);
    }
}
