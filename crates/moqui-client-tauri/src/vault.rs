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

//! The page's vault in the app (plans/Vault-Instances.md §3.5, §3.6, V5): `moquiDesktop.vault`, which `VaultClient.js`
//! uses in place of its wasm worker when the page runs in the desktop or mobile app. The module runs natively
//! (`moqui_client::vault`), one session per account, and the keys stay in the app's process; the page gets public keys,
//! wraps and the one item it asked for, as from the worker. The app never takes the module from the server: the scheme
//! refuses its files ([`crate::scheme`]).
//!
//! - `vault_call`: one op of the module for the calling page's account, or the app's own `configure`, `lock` (with
//!   `forget`, the device memory goes too) and `revisions_seen`.
//! - `vault_memory`: device memory, `state`, `remember` (`minutes`), `restore` and `forget`. A restore asks the server
//!   first, and where the key store waits for the device's lock or biometrics, the app asks the user
//!   ([`crate::ShellHost::confirm_device_user`]) and tries once more.

use std::sync::Arc;

use moqui_client::app::Client;
use moqui_client::vault::server::RestMemoryServer;
use moqui_client::vault::{forget_on, now_ms, Restore};
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

use crate::commands::blocking;
use crate::{state, ClientState, APP_ORIGIN, SHELL_ORIGIN};

/// The account of the calling page: a page of the app's own origins with an account signed in.
fn caller(state: &ClientState, webview: &Webview) -> Result<String, String> {
    let origin = webview
        .url()
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default();
    if origin != APP_ORIGIN && origin != SHELL_ORIGIN {
        return Err("Not a page of the app".to_string());
    }
    state
        .host
        .page_account(webview.label())
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

/// One call of the vault module for the calling page's account.
#[tauri::command]
pub async fn vault_call(app: AppHandle, webview: Webview, op: String, args: Option<Value>) -> Result<Value, String> {
    let state = state(&app)?;
    let id = caller(&state, &webview)?;
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
        core.vault().call(&id, &op, args)
    })
    .await
}

/// Device memory for the calling page's account: `state`, `remember` (for `minutes`), `restore` or `forget`.
#[tauri::command]
pub async fn vault_memory(
    app: AppHandle,
    webview: Webview,
    action: String,
    minutes: Option<u32>,
) -> Result<Value, String> {
    let state = state(&app)?;
    let id = caller(&state, &webview)?;
    let core = Arc::clone(&state.core);
    let shell = Arc::clone(&state.shell);
    let online = state.shell.network_ready();
    blocking(move || {
        let vault = core.vault();
        match action.as_str() {
            "state" => Ok(vault.memory_state_at(&id, now_ms())),
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
                vault.remember_at(&id, minutes.unwrap_or(0), &shell.device_name(), &server, now_ms())
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
                let mut restored = vault.restore_at(&id, key.is_some(), &server, now_ms());
                if restored == Restore::NeedsUnlock && shell.confirm_device_user() {
                    restored = vault.restore_at(&id, key.is_some(), &server, now_ms());
                }
                Ok(restored.to_json())
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
