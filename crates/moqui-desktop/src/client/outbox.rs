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

//! The commands of the host-owned outbox and drafts (plans/Mobile-Desktop.md §6, MD4), the page's `moquiDesktop.outbox`
//! (WebrootChat.js and Compose.qvue): the operations, their checks and limits, the drafts and their files are
//! `moqui_client::sync::outbox`, kept in the account's store and sent by the host (`moqui_client::sync::flush`), also
//! while no window shows them. The commands answer for the account of the calling window: the shell's current one, an
//! app window's own.
//!
//! The pages hear of each change as `window.__moquiOutboxEvent({kind})`, evaluated in the account's windows as socket
//! frames are (`sockets.rs`): `changed`, or `sent` with the server's answer to a chat or mail send.

use moqui_client::store::Store;
use moqui_client::sync::outbox::{self as rules, now_ms};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Webview};

use super::Client;

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
    let current = moqui_lib::sync::lock(&client.core.accounts)
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
        .core
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
    let op = rules::check(&kind, &payload, files.as_deref().unwrap_or(&[]), now_ms())?;
    let (kept, added) = on_store(&app, &webview, |_, store| rules::add_kept(store, op))?;
    if added {
        notify_pages(&app, &id, &json!({ "kind": "changed" }));
        client.core.flusher.wake();
    }
    Ok(kept)
}

/// The account's operations waiting to be sent, or refused, oldest first.
#[tauri::command(async)]
pub(crate) fn outbox_list(app: AppHandle, webview: Webview) -> Result<Value, String> {
    on_store(&app, &webview, |_, store| Ok(rules::list_json(store)))
}

/// Send an operation again now (one refused, held or waiting); false when there is none.
#[tauri::command(async)]
pub(crate) fn outbox_retry(app: AppHandle, webview: Webview, id: String) -> Result<bool, String> {
    let (account, found) = on_store(&app, &webview, |account, store| {
        Ok((account.to_string(), rules::retry(store, &id, now_ms())?))
    })?;
    if found {
        notify_pages(&app, &account, &json!({ "kind": "changed" }));
        if let Some(client) = app.try_state::<Client>() {
            client.core.flusher.wake();
        }
    }
    Ok(found)
}

/// Remove an operation and its files without sending it; false when there was none.
#[tauri::command(async)]
pub(crate) fn outbox_discard(app: AppHandle, webview: Webview, id: String) -> Result<bool, String> {
    let (account, removed) = on_store(&app, &webview, |account, store| {
        Ok((account.to_string(), rules::discard(store, &id)?))
    })?;
    if removed {
        notify_pages(&app, &account, &json!({ "kind": "changed" }));
    }
    Ok(removed)
}

/// What a sign-out of a shared device would delete unsent: `{operations, drafts}` (the shell's confirm, MD4 stage 5).
#[tauri::command(async)]
pub(crate) fn outbox_unsent(app: AppHandle, webview: Webview) -> Result<Value, String> {
    on_store(&app, &webview, |_, store| Ok(rules::unsent_json(store)))
}

/// The draft of `key`: `{key, value, files, updatedMs}`, or null.
#[tauri::command(async)]
pub(crate) fn draft_get(app: AppHandle, webview: Webview, key: String) -> Result<Value, String> {
    rules::draft_key(&key)?;
    on_store(&app, &webview, |_, store| Ok(rules::draft_get_json(store, &key)))
}

/// The drafts whose key starts with `prefix`, by key.
#[tauri::command(async)]
pub(crate) fn draft_list(app: AppHandle, webview: Webview, prefix: String) -> Result<Value, String> {
    rules::draft_prefix(&prefix)?;
    on_store(&app, &webview, |_, store| Ok(rules::draft_list_json(store, &prefix)))
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
    let files = files.unwrap_or_default();
    rules::check_draft(&key, &value, &files)?;
    on_store(&app, &webview, |_, store| rules::put_draft(store, &key, &value, &files))
}

/// Forget the draft of `key` and its files; false when there was none.
#[tauri::command(async)]
pub(crate) fn draft_remove(app: AppHandle, webview: Webview, key: String) -> Result<bool, String> {
    rules::draft_key(&key)?;
    on_store(&app, &webview, |_, store| {
        store.remove_draft(&key).map_err(|err| err.to_string())
    })
}

/// Keep a file of a draft or a send (`data` in base64), named after `name`: answers the store's name for it
/// (`moqui_client::sync::outbox::save_file`).
#[tauri::command(async)]
pub(crate) fn file_save(app: AppHandle, webview: Webview, name: String, data: String) -> Result<String, String> {
    let bytes = rules::file_bytes(&data)?;
    let max_bytes = app
        .try_state::<Client>()
        .map_or(0, |client| client.core.conf().store_max_bytes());
    on_store(&app, &webview, |_, store| {
        rules::save_file(store, &name, &bytes, max_bytes)
    })
}
