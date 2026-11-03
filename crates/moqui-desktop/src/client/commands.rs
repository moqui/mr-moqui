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

//! The shell's commands (`moquiDesktop` on the shell's origin, `bridge.js`): the accounts, sign-in with a password
//! (and a second factor's code) for a device key, sign-out that revokes it, switching and removing accounts, and
//! opening a server page in an app window. The password passes through once; only the key is kept, in the key store,
//! and no command answers it.
//!
//! The network calls run on Tauri's blocking pool (`spawn_blocking`): reqwest's blocking client must not run on the
//! main thread or in an async task. The others are `async` commands too, off the main thread: they read the key store,
//! or open and close windows.

use moqui_client::account::{account_id, base_url, Account};
use moqui_client::login::SignIn;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::{windows, Client};

/// Run `work` off the main thread and the async runtime.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|err| format!("The request did not finish: {err}"))?
}

/// The accounts, the current one, whether it is signed in, and the key store (`os` or `file`). No key.
pub(crate) fn state_json(client: &Client) -> Value {
    // the accounts as they are now, read without holding them while the keys are looked up
    let (current, accounts) = {
        let accounts = moqui_lib::sync::lock(&client.accounts);
        (
            accounts.current().map(|account| account.id.clone()),
            accounts.list().to_vec(),
        )
    };
    let list: Vec<Value> = accounts
        .iter()
        .map(|account| {
            json!({
                "id": account.id,
                "server": account.server,
                "username": account.username,
                "display": account.display,
                "serverName": account.server_name,
                "signedIn": client.keys.get(&account.id).is_some(),
            })
        })
        .collect();
    let signed_in = current.as_ref().is_some_and(|id| client.keys.get(id).is_some());
    json!({ "accounts": list, "current": current, "signedIn": signed_in, "keyStore": client.keys.kind })
}

#[tauri::command(async)]
pub(crate) fn client_state(app: AppHandle) -> Result<Value, String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    Ok(state_json(&client))
}

/// Sign in to `server` as `username`, with `code` once the server asks for one: `{status: "code", text, sendable}`,
/// or `{status: "signed-in"}`, with the account current and the shell loading it.
#[tauri::command]
pub(crate) async fn client_sign_in(
    app: AppHandle,
    server: String,
    username: String,
    password: String,
    code: Option<String>,
) -> Result<Value, String> {
    let base = base_url(&server).map_err(|err| err.to_string())?;
    let username = username.trim().to_string();
    if username.is_empty() || password.is_empty() {
        return Err("The username and password are needed".to_string());
    }
    let signed = {
        let base = base.clone();
        let username = username.clone();
        blocking(move || {
            let code = code.unwrap_or_default();
            let signed = moqui_client::login::sign_in(&base, &username, &password, code.trim(), &super::device_name())
                .map_err(|err| err.to_string())?;
            let SignIn::Key(key) = signed else {
                return Ok((signed, None));
            };
            // who the key signs in, and the server's name, from its client config
            let config = moqui_client::login::client_config(&base, &key)
                .map_err(|err| err.to_string())?
                .ok_or("The server did not take the new key")?;
            Ok((SignIn::Key(key), Some(config)))
        })
        .await?
    };
    let (key, config) = match signed {
        (SignIn::Code(ask), _) => {
            let sendable: Vec<Value> = ask
                .sendable
                .iter()
                .map(|factor| json!({ "id": factor.id, "label": factor.label }))
                .collect();
            return Ok(json!({ "status": "code", "text": ask.text, "sendable": sendable }));
        }
        (SignIn::Key(key), Some(config)) => (key, config),
        (SignIn::Key(_), None) => return Err("The server did not answer who signed in".to_string()),
    };
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let account = Account {
        id: account_id(&base, &username),
        server: base,
        username: config["username"].as_str().unwrap_or(&username).to_string(),
        user_id: config["userId"].as_str().unwrap_or_default().to_string(),
        display: config["userFullName"].as_str().unwrap_or_default().to_string(),
        server_name: config["server"]["name"].as_str().unwrap_or_default().to_string(),
    };
    client.keys.set(&account.id, &key).map_err(|err| err.to_string())?;
    if let Ok(bytes) = serde_json::to_vec(&config) {
        client.cache.put(&account.id, "/qapps/clientConfig", &bytes);
    }
    {
        let mut accounts = moqui_lib::sync::lock(&client.accounts);
        let id = account.id.clone();
        accounts.put(account);
        accounts.set_current(&id);
        accounts.save().map_err(|err| err.to_string())?;
    }
    switched(&app, &client);
    Ok(json!({ "status": "signed-in" }))
}

/// Have the server send a second factor's code to `factor`.
#[tauri::command]
pub(crate) async fn client_send_code(
    server: String,
    username: String,
    password: String,
    factor: String,
) -> Result<(), String> {
    let base = base_url(&server).map_err(|err| err.to_string())?;
    blocking(move || {
        moqui_client::login::send_code(&base, username.trim(), &password, &factor).map_err(|err| err.to_string())
    })
    .await
}

/// Sign the current account out: its key revoked on the server (when it answers) and forgotten here; the shell then
/// shows the sign-in. The account stays in the list, to sign in to again.
#[tauri::command]
pub(crate) async fn client_sign_out(app: AppHandle) -> Result<(), String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let Some((account, key)) = client.current() else {
        return Ok(());
    };
    if let Some(key) = key {
        let server = account.server.clone();
        let revoked =
            blocking(move || moqui_client::login::sign_out(&server, &key).map_err(|err| err.to_string())).await;
        if let Err(err) = revoked {
            tracing::warn!("The device key of {} was not revoked on the server: {err}", account.id);
        }
    }
    forget_key(&app, &account.id);
    Ok(())
}

/// Show another account: its server's shell, and none of the other's windows or sockets.
#[tauri::command(async)]
pub(crate) fn client_switch_account(app: AppHandle, id: String) -> Result<(), String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    {
        let mut accounts = moqui_lib::sync::lock(&client.accounts);
        if !accounts.set_current(&id) {
            return Err("No such account".to_string());
        }
        accounts.save().map_err(|err| err.to_string())?;
    }
    switched(&app, &client);
    Ok(())
}

/// The sign-in, for another account.
#[tauri::command(async)]
pub(crate) fn client_add_account(app: AppHandle) -> Result<(), String> {
    windows::load_shell(&app, Some("signIn=1"));
    Ok(())
}

/// Remove an account from this computer: its key revoked (when the server answers) and forgotten, its kept answers
/// deleted. Answers the accounts as `client_state` does.
#[tauri::command]
pub(crate) async fn client_remove_account(app: AppHandle, id: String) -> Result<Value, String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let Some((account, key)) = client.account(&id) else {
        return Ok(state_json(&client));
    };
    if let Some(key) = key {
        let server = account.server.clone();
        let revoked =
            blocking(move || moqui_client::login::sign_out(&server, &key).map_err(|err| err.to_string())).await;
        if let Err(err) = revoked {
            tracing::warn!("The device key of {id} was not revoked on the server: {err}");
        }
    }
    if let Err(err) = client.keys.delete(&id) {
        tracing::error!("The key of {id} was not removed from the key store: {err}");
    }
    windows::close_remotes(&app, Some(&id));
    client.cache.forget(&id);
    let was_current = {
        let mut accounts = moqui_lib::sync::lock(&client.accounts);
        let was_current = accounts.current().is_some_and(|account| account.id == id);
        accounts.remove(&id);
        accounts.save().map_err(|err| err.to_string())?;
        was_current
    };
    // the shell showed it: its sockets go, and the shell shows the sign-in
    if was_current {
        client.sockets.close_all();
        windows::load_shell(&app, None);
    }
    Ok(state_json(&client))
}

/// A page of the current account's server in an app window: `path` on the server (`/qapps/...`).
#[tauri::command(async)]
pub(crate) fn client_open_remote(app: AppHandle, path: String) -> Result<(), String> {
    windows::open_remote(&app, &path)
}

/// Forget an account's key here (signed out, or the server no longer takes it): its windows and sockets close, and the
/// shell shows the sign-in when it was the current one.
pub(crate) fn forget_key(app: &AppHandle, id: &str) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    if let Err(err) = client.keys.delete(id) {
        tracing::error!("The key of {id} was not removed from the key store: {err}");
    }
    windows::close_remotes(app, Some(id));
    let current = moqui_lib::sync::lock(&client.accounts)
        .current()
        .is_some_and(|account| account.id == id);
    if current {
        client.sockets.close_all();
        windows::load_shell(app, None);
    }
}

/// The current account changed: the other account's windows and sockets close, and the shell loads it.
fn switched(app: &AppHandle, client: &Client) {
    client.sockets.close_all();
    windows::close_remotes(app, None);
    windows::load_shell(app, None);
}
