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

//! The shell's commands of the offline map packages (plans/Maps-Offline.md §2.4, OF2; `moqui_client::packages`):
//! `packages_list` (the server's and the device's), `packages_download`, `packages_cancel`, `packages_delete` and
//! `packages_status`, and the offline screen's address search `packages_addresses` (§2.5), for the current account,
//! each refusing a page that is not the shell
//! ([`crate::ShellHost::is_shell_page`]). They answer ids, names, revisions, sizes and states, never a path on the
//! device; the page reads a package's files at `/__moqui-app/maps/<packageId>/<path>` ([`crate::scheme`]).
//!
//! A download runs where the app runs its background work ([`crate::ShellHost::start_package_download`]): a task of
//! the desktop host ([`run_task`], a thread), Android's `PackageDownloadWorker` (WorkManager) on the phone.

use std::sync::Arc;

use moqui_client::app::{Client, ClientHost};
use moqui_client::packages::Status;
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

use crate::commands::blocking;
use crate::{state, ClientState};

/// The client's state and the current account, for a page that is the shell.
fn shell_account<'a>(
    app: &'a AppHandle,
    webview: &Webview,
    nonce: Option<&str>,
) -> Result<(tauri::State<'a, ClientState>, String), String> {
    let state = state(app)?;
    if !state.shell.is_shell_page(webview, nonce) {
        return Err("Only the app's shell may do that".to_string());
    }
    let account = state
        .core
        .current()
        .map(|(account, _)| account.id)
        .ok_or("No account is signed in")?;
    Ok((state, account))
}

/// A package's state for the shell, as JSON.
fn status_json(status: &Status) -> Value {
    serde_json::to_value(status).unwrap_or(Value::Null)
}

/// Run a download of `revision` of a package on a thread of its own: the desktop's task (the default of
/// [`crate::ShellHost::start_package_download`]). Its end is in the job's state ([`packages_status`]).
///
/// # Errors
///
/// Returns why the thread did not start.
pub fn run_task(
    core: &Arc<Client>,
    host: &Arc<dyn ClientHost>,
    account: &str,
    package_id: &str,
    revision: i64,
) -> Result<(), String> {
    let (core, host) = (Arc::clone(core), Arc::clone(host));
    let (account, package_id) = (account.to_string(), package_id.to_string());
    std::thread::Builder::new()
        .name("map-package".to_string())
        .spawn(move || {
            let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                core.download_package(Some(host.as_ref()), &account, &package_id, revision)
            }));
            if ended.is_err() {
                tracing::error!("The download of the offline map {package_id} panicked");
            }
        })
        .map(|_| ())
        .map_err(|err| format!("The download did not start: {err}"))
}

/// The packages of the current account: as its server lists them now (`server`, null with `error` when it cannot be
/// read: offline), and as the device holds them (`device`: installed, and the downloads under way or last ended).
#[tauri::command]
pub async fn packages_list(app: AppHandle, webview: Webview, nonce: Option<String>) -> Result<Value, String> {
    let (core, host, account) = {
        let (state, account) = shell_account(&app, &webview, nonce.as_deref())?;
        (Arc::clone(&state.core), Arc::clone(&state.host), account)
    };
    let ready = state(&app)?.shell.network_ready();
    blocking(move || {
        let server = if ready {
            core.list_packages(Some(host.as_ref()), &account)
        } else {
            Err("This app cannot make secure connections on this device".to_string())
        };
        let device = core.packages().status(&account);
        Ok(match server {
            Ok(entries) => json!({ "server": entries, "device": device }),
            Err(error) => json!({ "server": null, "error": error, "device": device }),
        })
    })
    .await
}

/// Download `revision` of a package to this device (on the phone `wifiOnly` waits for an unmetered network): its
/// state, queued, or the one of a download of it under way.
#[tauri::command(async)]
pub fn packages_download(
    app: AppHandle,
    webview: Webview,
    nonce: Option<String>,
    package_id: String,
    revision: i64,
    wifi_only: Option<bool>,
) -> Result<Value, String> {
    let (state, account) = shell_account(&app, &webview, nonce.as_deref())?;
    if !state.shell.network_ready() {
        return Err("This app cannot make secure connections on this device".to_string());
    }
    let queued = state.core.packages().queue(&account, &package_id, revision)?;
    if matches!(queued, Status::Queued { .. }) {
        let started = state.shell.start_package_download(
            &state.core,
            &state.host,
            &account,
            &package_id,
            revision,
            wifi_only.unwrap_or(false),
        );
        if let Err(err) = started {
            state.core.packages().cancel(&account, &package_id);
            return Err(err);
        }
    }
    Ok(status_json(&queued))
}

/// Cancel a package's download: whether one was under way. What it downloaded goes; an installed revision stays.
#[tauri::command(async)]
pub fn packages_cancel(
    app: AppHandle,
    webview: Webview,
    nonce: Option<String>,
    package_id: String,
) -> Result<bool, String> {
    let (state, account) = shell_account(&app, &webview, nonce.as_deref())?;
    state.shell.cancel_package_download(&account, &package_id);
    Ok(state.core.packages().cancel(&account, &package_id))
}

/// Delete a package from this device: its download cancelled and its files removed.
#[tauri::command(async)]
pub fn packages_delete(
    app: AppHandle,
    webview: Webview,
    nonce: Option<String>,
    package_id: String,
) -> Result<(), String> {
    let (state, account) = shell_account(&app, &webview, nonce.as_deref())?;
    state.shell.cancel_package_download(&account, &package_id);
    state.core.packages().delete(&account, &package_id)
}

/// The current account's packages on this device ([`moqui_client::packages::Packages::status`]).
#[tauri::command(async)]
pub fn packages_status(app: AppHandle, webview: Webview, nonce: Option<String>) -> Result<Value, String> {
    let (state, account) = shell_account(&app, &webview, nonce.as_deref())?;
    Ok(state.core.packages().status(&account))
}

/// Search the current account's package for an address (plans/Maps-Offline.md §2.5,
/// [`moqui_client::packages::Packages::search_addresses`]): the server's address matching over the package's Overture
/// addresses, `text` read as `country` (ISO alpha-2, or each country of its rows when empty), `near` the map's middle
/// (`[lon, lat]`) breaking ties, at most `limit` candidates (20).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn packages_addresses(
    app: AppHandle,
    webview: Webview,
    nonce: Option<String>,
    package_id: String,
    text: String,
    country: Option<String>,
    near: Option<Vec<f64>>,
    limit: Option<usize>,
) -> Result<Value, String> {
    let (core, account) = {
        let (state, account) = shell_account(&app, &webview, nonce.as_deref())?;
        (Arc::clone(&state.core), account)
    };
    if text.len() > 200 {
        return Err("The address is too long".to_string());
    }
    let near = near
        .filter(|pair| pair.len() == 2 && pair.iter().all(|value| value.is_finite()))
        .map(|pair| (pair[0], pair[1]));
    blocking(move || {
        Ok(core.packages().search_addresses(
            &account,
            &package_id,
            &text,
            country.as_deref().unwrap_or(""),
            near,
            limit.unwrap_or(10),
        ))
    })
    .await
}
