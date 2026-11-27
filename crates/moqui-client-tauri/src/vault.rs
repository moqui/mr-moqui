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

//! The page's vault in the app (plans/archive/Vault-Instances.md §3.5, §3.6, V5): `moquiDesktop.vault`, which `VaultClient.js`
//! uses in place of its wasm worker when the page runs in the desktop or mobile app. The module runs natively
//! (`moqui_client::vault`), one session per page (the webview's label), and the keys stay in the app's process; the
//! page gets public keys, wraps and the one item it asked for, as from the worker. The app never takes the module from
//! the server: the scheme refuses its files ([`crate::scheme`], best effort by path).
//!
//! **Only a vault screen calls.** Every screen of every app shares the app origin, so a script injected into any of them
//! could otherwise call the vault. The scheme sees each page's documents and screen loads ([`note_request`]): a page
//! may call only while its window's top-level document was loaded at a vault screen ([`is_vault_screen`]) and it has
//! loaded no other screen or frame since, and its current URL is still a vault screen. Anything else locks that page's
//! session; a new document always starts a fresh one, as a browser's worker goes with its page. The app's init scripts
//! run in the main frame only (Tauri's `for_main_frame_only`), and a same-origin frame could reach its parent's adapter,
//! so loading any frame that is not a vault screen ends the page's right too.
//!
//! - `vault_call`: one op of the module for the calling page, or the app's own `configure`, `lock` (with `forget`, the
//!   device memory goes too) and `revisions_seen`.
//! - `vault_memory`: device memory, `state`, `remember` (`minutes`), `restore` and `forget`. Remember and restore need
//!   the page's user activation (a click in the vault's own form), which the bridge reads with the getter it captured
//!   before any page script ran; a restore asks the server for its half of the key first. On the phone the Keystore
//!   asks for the lock screen or a biometric at each use; on the desktop the OS key store opens with the computer's
//!   sign-in, so the gesture and that store's access control are what a restore relies on there.

use std::sync::Arc;

use moqui_client::app::Client;
use moqui_client::vault::server::RestMemoryServer;
use moqui_client::vault::{forget_on, now_ms, Restore};
use serde_json::{json, Value};
use tauri::http::HeaderMap;
use tauri::{AppHandle, Webview};

use crate::commands::blocking;
use crate::{state, ClientState, APP_ORIGIN};

/// The screens that run the vault: Commons › Vault and its private contacts, the Vault key settings, BOSS Admin ›
/// Security › Recovery keys, and System › Vaults with its lease dialog. A path is a vault screen when one of its names
/// is one of these.
pub const VAULT_SCREENS: [&str; 5] = ["Vault", "Vaults", "VaultKey", "VaultContacts", "RecoveryKeys"];

/// Whether `path` (no query) is a vault screen's: under the apps (`/qapps/`, `/apps/`) with one of [`VAULT_SCREENS`] among
/// its names, the path given as the browser sends it (no `.` or `..` names, no percent-encoding of them).
pub fn is_vault_screen(path: &str) -> bool {
    let bare = path.split(['?', '#']).next().unwrap_or("");
    let names: Vec<&str> = bare.split('/').filter(|name| !name.is_empty()).collect();
    let under_apps = matches!(names.first(), Some(&"qapps") | Some(&"apps"));
    let plain = names
        .iter()
        .all(|name| *name != "." && *name != ".." && !name.contains('%'));
    under_apps && plain && names.iter().skip(1).any(|name| VAULT_SCREENS.contains(name))
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// A request of the page `label` for `path` (no query), as the scheme answers it: a top-level document decides whether
/// the page may call the vault (a vault screen) and always starts it locked; a frame's document or a screen the page
/// loads into itself (WebrootVue's component load) that is not a vault screen ends the page's right and locks it.
pub fn note_request(state: &ClientState, label: &str, path: &str, headers: &HeaderMap) {
    let dest = header(headers, "sec-fetch-dest");
    let accept = header(headers, "accept");
    let top = dest == "document" || (dest.is_empty() && accept.contains("application/xhtml+xml"));
    let frame = matches!(dest.as_str(), "iframe" | "frame" | "embed" | "object");
    let screen = !top && accept.contains("text/html") && accept.contains("application/javascript");
    if !(top || frame || screen) {
        return;
    }
    let vault = is_vault_screen(path);
    {
        let mut pages = moqui_lib::sync::lock(&state.vault_pages);
        if top {
            pages.insert(label.to_string(), vault);
        } else if !vault {
            pages.insert(label.to_string(), false);
        }
    }
    if top || !vault {
        state.core.vault().lock_label(label);
    }
}

/// The account of the calling page, when it may call the vault: a page on the app origin whose document is a vault
/// screen's (see the module's doc), with an account signed in.
fn caller(state: &ClientState, webview: &Webview) -> Result<String, String> {
    let url = webview.url().map_err(|_| "Not a page of the app".to_string())?;
    if url.origin().ascii_serialization() != APP_ORIGIN {
        return Err("Not a page of the app".to_string());
    }
    let label = webview.label();
    let allowed = moqui_lib::sync::lock(&state.vault_pages).get(label).copied() == Some(true);
    if !allowed || !is_vault_screen(url.path()) {
        moqui_lib::sync::lock(&state.vault_pages).insert(label.to_string(), false);
        state.core.vault().lock_label(label);
        return Err("Only the vault's own screens may use the vault".to_string());
    }
    state
        .host
        .page_account(label)
        .filter(|id| state.core.account(id).is_some())
        .ok_or_else(|| "No account is signed in".to_string())
}

/// End `memory_id` on `account`'s server with its key, when it has one.
fn forget_on_server(core: &Client, account: &str, memory_id: &str) {
    if let Some((account, Some(key))) = core.account(account) {
        forget_on(
            &RestMemoryServer {
                server: &account.server,
                key: &key,
            },
            memory_id,
        );
    }
}

/// One call of the vault module for the calling page.
#[tauri::command]
pub async fn vault_call(app: AppHandle, webview: Webview, op: String, args: Option<Value>) -> Result<Value, String> {
    let state = state(&app)?;
    let id = caller(&state, &webview)?;
    let page = webview.label().to_string();
    let core = Arc::clone(&state.core);
    let args = args.unwrap_or_else(|| json!({}));
    blocking(move || {
        if op == "lock" && args["forget"] == Value::Bool(true) {
            core.vault().lock(&id);
            if let Some(memory_id) = core.vault().forget(&id) {
                forget_on_server(&core, &id, &memory_id);
            }
            return Ok(json!({ "locked": true, "forgotten": true }));
        }
        core.vault().call(&id, &page, &op, args)
    })
    .await
}

/// Device memory for the calling page: `state`, `remember` (for `minutes`), `restore` or `forget`. `gesture`: the page's
/// user activation as the bridge read it, which remember and restore need.
#[tauri::command]
pub async fn vault_memory(
    app: AppHandle,
    webview: Webview,
    action: String,
    minutes: Option<u32>,
    gesture: Option<bool>,
) -> Result<Value, String> {
    let state = state(&app)?;
    let id = caller(&state, &webview)?;
    let page = webview.label().to_string();
    let core = Arc::clone(&state.core);
    let shell = Arc::clone(&state.shell);
    let online = state.shell.network_ready();
    if matches!(action.as_str(), "remember" | "restore") && gesture != Some(true) {
        return Err("Remembering or opening the key from this device needs a click in the vault".to_string());
    }
    blocking(move || {
        let vault = core.vault();
        match action.as_str() {
            "state" => Ok(vault.memory_state_at(&id, &page, now_ms())),
            "remember" => {
                if !online {
                    return Err("This app cannot make secure connections on this device".to_string());
                }
                let (account, key) = core.account(&id).ok_or("No account is signed in")?;
                let key = key.ok_or("The account is signed out on this device")?;
                let server = RestMemoryServer {
                    server: &account.server,
                    key: &key,
                };
                vault.remember_at(
                    &id,
                    &page,
                    minutes.unwrap_or(0),
                    &shell.device_name(),
                    &server,
                    now_ms(),
                )
            }
            "restore" => {
                if !online {
                    return Ok(Restore::Unconfirmed("this app cannot make secure connections".to_string()).to_json());
                }
                let (account, key) = core.account(&id).ok_or("No account is signed in")?;
                let server = RestMemoryServer {
                    server: &account.server,
                    key: key.as_deref().unwrap_or(""),
                };
                Ok(vault.restore_at(&id, &page, key.is_some(), &server, now_ms()).to_json())
            }
            "forget" => {
                let forgotten = vault.forget(&id);
                if let Some(memory_id) = &forgotten {
                    forget_on_server(&core, &id, memory_id);
                }
                Ok(json!({ "forgotten": forgotten.is_some() }))
            }
            _ => Err("No such vault memory action".to_string()),
        }
    })
    .await
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
