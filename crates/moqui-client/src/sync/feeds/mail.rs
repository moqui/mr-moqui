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

//! The mail change cursor folded into the store (`EmailClientServices.get#MailChanges`, the Inbox's `mailChanges`).
//! Each change is one thread as it is now (its `messages`), so the store's copy of the thread is made to match it:
//!
//! - `deleted`: the thread goes from every folder, with its messages; `removedMessageIds` go, with their rows in a
//!   folder that lists messages.
//! - The thread's kept messages take the state of theirs (`isUnread`, `isFlagged`, `isAnswered`, `statusId`,
//!   `snoozedThruDate`), and those no longer in it go. One not kept, of a thread whose messages are kept, has the
//!   thread read again (its body is not in the change).
//! - Its row in each folder takes the thread's counts and flags; a row of a folder (or the inbox, the flagged view, a
//!   label) none of its messages is in now leaves that folder. In a folder that lists messages (the conversation view
//!   of messages) each message's own row is changed. A thread new to a folder the store keeps rows of (a folder of
//!   its messages, the inbox) gets a row there, built from the change, so new mail shows offline.
//!
//! What a change leaves to read (a thread), and a change that does not fold, is a pending read (`super::PENDING`),
//! noted before the cursor moves past it. `reset` with a cursor drops the cached reads of mail (threads, messages, their
//! answers; not the folders), with the new cursor in one unit, for the mirror to read again; `unsupported` (an app's
//! inbox source answers without a cursor) keeps MD4's behavior, the folders read each time. The stores the cursor does
//! not cover (the answer's `uncoveredStores`, and the client config's `server.features.mailChangesUncovered`:
//! IronStone's `internal`) have their kept threads read again, [`UNCOVERED_MAX`] a run, going round them.

use std::collections::{BTreeSet, HashMap};

use serde_json::{json, Value};

use super::{note_pending, Run, LIMIT, MAIL_CURSOR, MAX_PAGES, UNSUPPORTED};
use crate::offline::{self, folder_meta_query, kept_object, mail_base, number, put_object, FOLDER_META, THREAD_META};
use crate::store::{MailMessage, MailThread, Store, SyncCursor};
use crate::sync::mirror::{encode, Ended};

/// Threads of a store the cursor does not cover read again in one run.
const UNCOVERED_MAX: u32 = 50;
/// The prefix of the cursor of where the rereads of an uncovered store got to (`uncovered:<store>`).
const UNCOVERED: &str = "uncovered:";

pub(super) fn mail(run: &mut Run<'_, '_>) -> Result<(), Ended> {
    let base = mail_base(&run.job.comms_root);
    run.drain(&["thread:"])?;
    let cursor = run
        .store(|store| store.sync_cursor(MAIL_CURSOR))
        .flatten()
        .unwrap_or_default();
    let mut after = cursor.seq;
    let mut uncovered: BTreeSet<String> = run.job.server.mail_uncovered.iter().cloned().collect();
    for _ in 0..MAX_PAGES {
        let mut path = format!("{base}/mailChanges?limit={LIMIT}");
        if let Some(after) = after {
            path.push_str(&format!("&afterSeq={after}"));
        }
        let Some(page) = run.json(&path)? else {
            return Ok(());
        };
        if page["unsupported"].as_bool() == Some(true) {
            run.fold("the mail cursor", |store| {
                let unsupported = SyncCursor {
                    seq: None,
                    value: Some(UNSUPPORTED.to_string()),
                };
                store
                    .set_sync_cursor(MAIL_CURSOR, &unsupported)
                    .map_err(|err| err.to_string())
            });
            run.synced.mail_unsupported = true;
            return Ok(());
        }
        for name in page["uncoveredStores"].as_array().into_iter().flatten() {
            if let Some(name) = name.as_str() {
                uncovered.insert(name.to_string());
            }
        }
        if page["reset"].as_bool() == Some(true) {
            let last = number(&page["lastSeq"]).unwrap_or(0);
            let forget = cursor.seq.is_some();
            let reset = run.fold("a mail reset", |store| {
                store
                    .atomic(|store| {
                        if forget {
                            forget_mail_reads(store, &base).map_err(moqui_lib::error::Error::new)?;
                        }
                        store.set_sync_seq(MAIL_CURSOR, last)
                    })
                    .map_err(|err| err.to_string())
            });
            if reset {
                if forget {
                    tracing::info!("Feeds: the mail feed was reset; mail is read again");
                }
                run.synced.mail_reread = true;
            }
            break;
        }
        let folders = run.store(folder_kinds).unwrap_or_default();
        for change in page["changes"].as_array().into_iter().flatten() {
            let what = format!("mail change {}", change["changeSeq"]);
            if !run.fold(&what, |store| fold_change(store, &folders, change)) {
                if let Some(thread) = change["threadId"].as_str() {
                    run.pending(&format!("thread:email:{thread}"));
                }
            }
        }
        if let Some(last) = number(&page["lastSeq"]) {
            run.fold("the mail cursor", |store| {
                store.set_sync_seq(MAIL_CURSOR, last).map_err(|err| err.to_string())
            });
            after = Some(last);
        }
        if page["more"].as_bool() != Some(true) {
            break;
        }
    }
    run.drain(&["thread:"])?;
    // the stores the cursor does not cover: their kept threads read again, going round them
    for name in uncovered.iter().filter(|name| crate::files::plain_name(name)) {
        let prefix = format!("{name}:");
        let mark = format!("{UNCOVERED}{name}");
        let from = run
            .store(|store| store.sync_cursor(&mark))
            .flatten()
            .and_then(|cursor| cursor.value);
        let mut held = run
            .store(|store| store.mail_thread_ids_with_messages(&prefix, from.as_deref(), UNCOVERED_MAX))
            .unwrap_or_default();
        if held.is_empty() && from.is_some() {
            held = run
                .store(|store| store.mail_thread_ids_with_messages(&prefix, None, UNCOVERED_MAX))
                .unwrap_or_default();
        }
        for key in &held {
            let Some(id) = key.strip_prefix(&prefix) else { continue };
            run.keep(&format!("{base}/thread?store={}&threadId={}", encode(name), encode(id)))?;
            run.fold("an uncovered store's place", |store| {
                let place = SyncCursor {
                    seq: None,
                    value: Some(key.clone()),
                };
                store.set_sync_cursor(&mark, &place).map_err(|err| err.to_string())
            });
        }
    }
    Ok(())
}

/// The kinds of the kept folders (`folderKindEnumId`), by the store's folder key.
fn folder_kinds(store: &Store) -> HashMap<String, String> {
    store
        .mail_folders()
        .into_iter()
        .filter_map(|folder| {
            let row: Value = serde_json::from_str(&folder.row).ok()?;
            Some((folder.folder_id, row["folderKindEnumId"].as_str()?.to_string()))
        })
        .collect()
}

/// A reset of the mail feed: the cached reads of mail go (threads, messages, their answers, pending reads), for the
/// mirror to read again; the folders stay.
fn forget_mail_reads(store: &Store, base: &str) -> Result<(), String> {
    store.forget_mail().map_err(|err| err.to_string())?;
    for prefix in [
        format!("{base}/threads"),
        format!("{base}/thread?"),
        THREAD_META.to_string(),
        FOLDER_META.to_string(),
    ] {
        store.remove_answers(&prefix, |_| true).map_err(|err| err.to_string())?;
    }
    store
        .remove_sync_cursors(&format!("{}thread:", super::PENDING))
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// The state of a thread's messages a change gives, by the store's message id.
struct Message<'a> {
    id: String,
    json: &'a Value,
}

impl Message<'_> {
    fn flag(&self, name: &str) -> bool {
        self.json[name].as_bool() == Some(true)
    }

    fn date(&self) -> i64 {
        number(&self.json["messageDate"]).unwrap_or(0)
    }

    fn folder(&self) -> Option<&str> {
        self.json["emailFolderId"].as_str()
    }

    fn account(&self) -> &str {
        self.json["emailAccountId"].as_str().unwrap_or("")
    }
}

/// An id of the server's JSON: text, or a number as text.
fn id_of(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| number(value).map(|id| id.to_string()))
}

/// Fold one change of the mail feed (a thread as it is now).
fn fold_change(store: &Store, folders: &HashMap<String, String>, change: &Value) -> Result<(), String> {
    let Some(thread_id) = change["threadId"].as_str().filter(|id| !id.is_empty()) else {
        return Ok(());
    };
    let key = format!("email:{thread_id}");
    if change["deleted"].as_bool() == Some(true) {
        // the thread's own rows, and its messages' in a folder that lists messages
        let mut gone: Vec<String> = store.mail_message_ids(&key);
        gone.push(key.clone());
        store
            .remove_mail_thread_everywhere(&key)
            .map_err(|err| err.to_string())?;
        for id in gone {
            for row in store.mail_threads_of(&id) {
                store
                    .remove_mail_thread(&row.folder_id, &row.thread_id)
                    .map_err(|err| err.to_string())?;
            }
        }
        return store
            .remove_answer(THREAD_META, &format!("thread={key}"))
            .map_err(|err| err.to_string());
    }
    for removed in change["removedMessageIds"].as_array().into_iter().flatten() {
        if let Some(id) = id_of(removed) {
            let id = format!("email:{id}");
            store.remove_mail_message(&id).map_err(|err| err.to_string())?;
            // its own row, where a folder lists messages
            if id != key {
                for row in store.mail_threads_of(&id) {
                    if lists_messages(store, &row.folder_id) {
                        store
                            .remove_mail_thread(&row.folder_id, &row.thread_id)
                            .map_err(|err| err.to_string())?;
                    }
                }
            }
        }
    }
    let messages: Vec<Message<'_>> = change["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|json| {
            Some(Message {
                id: format!("email:{}", id_of(&json["emailMessageId"])?),
                json,
            })
        })
        .collect();
    fold_messages(store, &key, thread_id, &messages)?;
    fold_rows(store, folders, &key, thread_id, &messages)
}

/// Whether the store's folder `folder` lists messages (the conversation view of messages), not threads.
fn lists_messages(store: &Store, folder: &str) -> bool {
    kept_object(store, FOLDER_META, &folder_meta_query(folder))
        .and_then(|meta| meta.get("conversationView").and_then(Value::as_str).map(str::to_string))
        .as_deref()
        == Some("messages")
}

/// The thread's kept messages made to match the change's; a message not kept has the thread read again.
fn fold_messages(store: &Store, key: &str, thread_id: &str, messages: &[Message<'_>]) -> Result<(), String> {
    let held = store.mail_message_ids(key);
    if held.is_empty() {
        return Ok(());
    }
    for gone in held
        .iter()
        .filter(|id| !messages.iter().any(|message| &message.id == *id))
    {
        store.remove_mail_message(gone).map_err(|err| err.to_string())?;
    }
    let mut changed = Vec::new();
    for message in messages {
        let Some(kept) = store.mail_message(&message.id) else {
            note_pending(store, &format!("thread:email:{thread_id}"))?;
            continue;
        };
        let Ok(mut row) = serde_json::from_str::<Value>(&kept.row) else {
            continue;
        };
        row["isUnread"] = json!(!message.flag("isRead"));
        row["isFlagged"] = json!(message.flag("isFlagged"));
        row["isAnswered"] = json!(message.flag("isAnswered"));
        for name in ["statusId", "snoozedThruDate"] {
            if let Some(value) = message.json.get(name) {
                row[name] = value.clone();
            }
        }
        changed.push(MailMessage {
            row: row.to_string(),
            thread_id: key.to_string(),
            ..kept
        });
    }
    store.put_mail_messages(&changed).map_err(|err| err.to_string())?;
    let meta_query = format!("thread={key}");
    if let Some(mut meta) = kept_object(store, THREAD_META, &meta_query) {
        let unread = messages.iter().filter(|message| !message.flag("isRead")).count();
        meta.insert("unreadCount".to_string(), json!(unread));
        meta.insert(
            "isFlagged".to_string(),
            json!(messages.iter().any(|message| message.flag("isFlagged"))),
        );
        put_object(store, THREAD_META, &meta_query, &Value::Object(meta))?;
    }
    Ok(())
}

/// The rows of a thread in the folders that keep it, and of its messages where a folder lists messages: their counts
/// and flags, out of a folder none of their messages is in now; and a row in a kept folder it is new to.
fn fold_rows(
    store: &Store,
    folders: &HashMap<String, String>,
    key: &str,
    thread_id: &str,
    messages: &[Message<'_>],
) -> Result<(), String> {
    let mut rows: Vec<MailThread> = store
        .mail_threads_of(key)
        .into_iter()
        .filter(|row| !lists_messages(store, &row.folder_id) || messages.iter().any(|message| message.id == key))
        .collect();
    for message in messages.iter().filter(|message| message.id != key) {
        rows.extend(
            store
                .mail_threads_of(&message.id)
                .into_iter()
                .filter(|row| lists_messages(store, &row.folder_id)),
        );
    }
    let mut changed: Vec<MailThread> = Vec::new();
    for row in &rows {
        // a folder listing messages: this row is one message's
        let shown: Vec<&Message<'_>> = if lists_messages(store, &row.folder_id) {
            messages.iter().filter(|message| message.id == row.thread_id).collect()
        } else {
            messages.iter().collect()
        };
        if !belongs(&row.folder_id, folders, &shown) {
            store
                .remove_mail_thread(&row.folder_id, &row.thread_id)
                .map_err(|err| err.to_string())?;
            continue;
        }
        let Ok(fields) = serde_json::from_str::<Value>(&row.row) else {
            continue;
        };
        let (fields, sort_ms) = row_fields(fields, row.sort_ms, &shown);
        changed.push(MailThread {
            row: fields.to_string(),
            sort_ms,
            ..row.clone()
        });
    }
    // new to a folder the store keeps a list of (threads, not messages): a row built from the change
    let kept: BTreeSet<String> = rows.iter().map(|row| row.folder_id.clone()).collect();
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    for message in messages {
        if let Some(folder) = message.folder() {
            candidates.insert(offline::account_folder_key(message.account(), folder));
        }
    }
    candidates.insert("view=inbox".to_string());
    let shown: Vec<&Message<'_>> = messages.iter().collect();
    for folder in candidates.iter().filter(|folder| !kept.contains(*folder)) {
        let listed = !store.mail_threads(folder, None, 1).is_empty();
        if !listed || lists_messages(store, folder) || !belongs(folder, folders, &shown) {
            continue;
        }
        let Some(first) = messages.iter().min_by_key(|message| message.date()) else {
            continue;
        };
        let base = json!({ "store": "email", "threadId": thread_id, "rootEmailMessageId": thread_id, "rowKey": key,
            "emailAccountId": first.account(), "subject": first.json["subject"], "firstDate": first.date(),
            "participantList": [], "labelList": [], "isClosed": false });
        let (fields, sort_ms) = row_fields(base, 0, &shown);
        changed.push(MailThread {
            folder_id: folder.clone(),
            thread_id: key.to_string(),
            sort_ms,
            row: fields.to_string(),
        });
    }
    store
        .put_mail_threads(&changed)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// A row's fields as the messages `shown` give them, and its sort date.
fn row_fields(mut fields: Value, sort_ms: i64, shown: &[&Message<'_>]) -> (Value, i64) {
    let unread = shown.iter().filter(|message| !message.flag("isRead")).count();
    fields["unreadCount"] = json!(unread);
    fields["isUnread"] = json!(unread > 0);
    fields["isFlagged"] = json!(shown.iter().any(|message| message.flag("isFlagged")));
    fields["isArchived"] = json!(shown.iter().all(|message| message.flag("archived")));
    fields["messageCount"] = json!(shown.len());
    let mut sort_ms = sort_ms;
    if let Some(newest) = shown.iter().max_by_key(|message| message.date()) {
        let id = newest.id.strip_prefix("email:").unwrap_or(&newest.id);
        fields["lastEmailMessageId"] = json!(id);
        fields["emailMessageId"] = json!(id);
        fields["emailFolderId"] = newest.json["emailFolderId"].clone();
        fields["lastDate"] = json!(newest.date());
        for name in ["snippet", "fromName", "fromAddress", "statusId", "hasAttachment"] {
            if let Some(value) = newest.json.get(name) {
                fields[name] = value.clone();
            }
        }
        sort_ms = sort_ms.max(newest.date());
        fields["sortDate"] = json!(sort_ms);
    }
    (fields, sort_ms)
}

/// Whether a thread whose messages are `shown` stays in the store's folder `folder` (a folder key, as
/// `offline::account_folder_key` and the `threads` queries make them).
fn belongs(folder: &str, folders: &HashMap<String, String>, shown: &[&Message<'_>]) -> bool {
    if shown.is_empty() {
        return false;
    }
    let in_folder = |id: &str| shown.iter().any(|message| message.folder() == Some(id));
    let parts: HashMap<&str, &str> = folder.split('&').filter_map(|part| part.split_once('=')).collect();
    if let Some(id) = parts.get("folder") {
        return in_folder(id);
    }
    if let Some(label) = parts.get("label") {
        return shown.iter().any(|message| {
            message.json["labelIds"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|kept| kept.as_str() == Some(label)))
        });
    }
    match parts.get("view").copied() {
        Some("inbox") => shown.iter().any(|message| {
            !message.flag("archived")
                && message.folder().is_none_or(|id| {
                    folders
                        .get(&offline::account_folder_key(message.account(), id))
                        .is_none_or(|kind| kind == "EfkInbox")
                })
        }),
        Some("flagged") => shown.iter().any(|message| message.flag("isFlagged")),
        _ => true,
    }
}
