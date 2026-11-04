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
//! while no page shows them. The commands answer for the account of the calling page (`ClientHost::page_account`): the
//! shell's current one, a server page's own. Both origins may call them: they reach nothing the page's requests do not
//! already reach with the key.
//!
//! The pages hear of each change as `window.__moquiOutboxEvent({kind})` (`ClientHost::notify_pages`): `changed`, or
//! `sent` with the server's answer to a chat or mail send.
//!
//! What may be queued follows the account's server (MD5, `moqui_client::login::ServerSync`): at `clientApi` 4 a
//! message's edit, redaction, reaction and poll vote too, and an internal message where the server takes its send once;
//! `outbox_kinds` tells the page which.

use moqui_client::store::Store;
use moqui_client::sync::outbox::{self as rules, now_ms};
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

use crate::{state, ClientState};

/// The account of the calling page; an error when none is signed in there.
fn caller(state: &ClientState, webview: &Webview) -> Result<String, String> {
    state
        .host
        .page_account(webview.label())
        .filter(|id| state.core.account(id).is_some())
        .ok_or_else(|| "No account is signed in".to_string())
}

/// Run `work` on the store of the calling page's account.
fn on_store<T>(
    app: &AppHandle,
    webview: &Webview,
    work: impl FnOnce(&str, &Store) -> Result<T, String>,
) -> Result<T, String> {
    let state = state(app)?;
    let id = caller(&state, webview)?;
    state
        .core
        .with_store(&id, |store| work(&id, store))
        .unwrap_or_else(|| Err("This account's data is not open".to_string()))
}

/// Tell the account's pages that its outbox changed.
fn changed(app: &AppHandle, account: &str, wake: bool) {
    if let Ok(state) = state(app) {
        state.host.notify_pages(account, &json!({ "kind": "changed" }));
        if wake {
            state.core.flusher().wake();
        }
    }
}

/// Keep an operation to send (`kind`, its `payload`, the store's `files` it sends): answers it as `outbox_list` lists
/// it, or why it cannot wait. The host sends it when it can, and the pages hear of each change.
#[tauri::command(async)]
pub fn outbox_add(
    app: AppHandle,
    webview: Webview,
    kind: String,
    payload: Value,
    files: Option<Vec<String>>,
) -> Result<Value, String> {
    let server = {
        let state = state(&app)?;
        let id = caller(&state, &webview)?;
        state.core.server_sync_of(&id)
    };
    let op = rules::check_with(&kind, &payload, files.as_deref().unwrap_or(&[]), now_ms(), &server)?;
    let (account, (kept, added)) = on_store(&app, &webview, |account, store| {
        Ok((account.to_string(), rules::add_kept(store, op)?))
    })?;
    if added {
        changed(&app, &account, true);
    }
    Ok(kept)
}

/// The kinds of operation the calling page's account's server takes queued: `{clientApi, kinds, internalSend}`.
#[tauri::command(async)]
pub fn outbox_kinds(app: AppHandle, webview: Webview) -> Result<Value, String> {
    let state = state(&app)?;
    let id = caller(&state, &webview)?;
    Ok(rules::kinds_json(&state.core.server_sync_of(&id)))
}

/// The account's operations waiting to be sent, refused (`rejected`, with the server's `status` and `error`) or in
/// conflict (with the server's `current`), oldest first.
#[tauri::command(async)]
pub fn outbox_list(app: AppHandle, webview: Webview) -> Result<Value, String> {
    on_store(&app, &webview, |_, store| Ok(rules::list_json(store)))
}

/// Send an operation again now (one refused, held or waiting); false when there is none.
#[tauri::command(async)]
pub fn outbox_retry(app: AppHandle, webview: Webview, id: String) -> Result<bool, String> {
    let (account, found) = on_store(&app, &webview, |account, store| {
        Ok((account.to_string(), rules::retry(store, &id, now_ms())?))
    })?;
    if found {
        changed(&app, &account, true);
    }
    Ok(found)
}

/// Remove an operation and its files without sending it; false when there was none.
#[tauri::command(async)]
pub fn outbox_discard(app: AppHandle, webview: Webview, id: String) -> Result<bool, String> {
    let (account, removed) = on_store(&app, &webview, |account, store| {
        Ok((account.to_string(), rules::discard(store, &id)?))
    })?;
    if removed {
        changed(&app, &account, false);
    }
    Ok(removed)
}

/// What a sign-out of a shared device would delete unsent: `{operations, drafts}` (the shell's confirm, MD4 stage 5).
#[tauri::command(async)]
pub fn outbox_unsent(app: AppHandle, webview: Webview) -> Result<Value, String> {
    on_store(&app, &webview, |_, store| Ok(rules::unsent_json(store)))
}

/// The draft of `key`: `{key, value, files, updatedMs}`, or null.
#[tauri::command(async)]
pub fn draft_get(app: AppHandle, webview: Webview, key: String) -> Result<Value, String> {
    rules::draft_key(&key)?;
    on_store(&app, &webview, |_, store| Ok(rules::draft_get_json(store, &key)))
}

/// The drafts whose key starts with `prefix`, by key.
#[tauri::command(async)]
pub fn draft_list(app: AppHandle, webview: Webview, prefix: String) -> Result<Value, String> {
    rules::draft_prefix(&prefix)?;
    on_store(&app, &webview, |_, store| Ok(rules::draft_list_json(store, &prefix)))
}

/// Keep `value` (text, usually JSON) as the draft of `key`, with the store's `files` attached to it.
#[tauri::command(async)]
pub fn draft_put(
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
pub fn draft_remove(app: AppHandle, webview: Webview, key: String) -> Result<bool, String> {
    rules::draft_key(&key)?;
    on_store(&app, &webview, |_, store| {
        store.remove_draft(&key).map_err(|err| err.to_string())
    })
}

/// Keep a file of a draft or a send (`data` in base64), named after `name`: answers the store's name for it
/// (`moqui_client::sync::outbox::save_file`).
#[tauri::command(async)]
pub fn file_save(app: AppHandle, webview: Webview, name: String, data: String) -> Result<String, String> {
    let bytes = rules::file_bytes(&data)?;
    let max_bytes = state(&app).map_or(0, |state| state.core.conf().store_max_bytes());
    on_store(&app, &webview, |_, store| {
        rules::save_file(store, &name, &bytes, max_bytes)
    })
}
