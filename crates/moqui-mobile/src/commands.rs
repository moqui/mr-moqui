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

//! The mobile app's own commands, beside the shared ones of `moqui_client_tauri`: the way back from a server page to
//! the shell, a comms screen of a server page shown in the shell (an encrypted room, whose crypto device is the
//! shell's), a native notification, and a link opened outside the app. Any page of the app may call them; none reaches
//! more than a page could.

use std::sync::Arc;

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_notification::{NotificationExt, PermissionState};

use crate::host::{server_path, MobileHost};

/// The longest title and body of a notification, in characters; more is cut.
const TITLE_MAX: usize = 200;
const BODY_MAX: usize = 1000;
/// The longest URL a page may hand over.
const URL_MAX: usize = 8192;

fn host(app: &AppHandle) -> Result<Arc<MobileHost>, String> {
    app.try_state::<Arc<MobileHost>>()
        .map(|host| Arc::clone(&host))
        .ok_or_else(|| "Not a client of a server".to_string())
}

/// Show the shell again (a server page's back to the app).
#[tauri::command(async)]
pub(crate) fn client_show_shell(app: AppHandle) -> Result<(), String> {
    let host = host(&app)?;
    host.navigate(&moqui_client_tauri::shell_url(None, None));
    Ok(())
}

/// A server page's "Open in Chat" (an encrypted room or call): `path`, a screen of the current account's comms app,
/// kept for the shell to show (`client_take_open`) and the shell shown.
#[tauri::command(async)]
pub(crate) fn open_in_shell(app: AppHandle, path: String) -> Result<(), String> {
    let host = host(&app)?;
    let state = moqui_client_tauri::state(&app)?;
    let id = host.signed_in().ok_or("No account is signed in")?;
    let path = server_path(&path)?;
    let root = state.core.comms_root_of(&id);
    if !under(path, &root) {
        return Err(format!("The app does not show {path}"));
    }
    *moqui_lib::sync::lock(&state.open_pending) = Some(path.to_string());
    host.navigate(&moqui_client_tauri::shell_url(None, None));
    Ok(())
}

/// Whether `path` (with a query) is under the comms app at `root`.
pub(crate) fn under(path: &str, root: &str) -> bool {
    let bare = path.split_once('?').map_or(path, |(bare, _)| bare);
    let root = root.trim_end_matches('/');
    !root.is_empty() && bare.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

/// A native notification: `title`, and `body`. The page's link and tag are taken and not used yet. Android 13 and
/// later ask the user the first time (off the main thread: the plugin waits for the answer).
#[tauri::command(async)]
pub(crate) fn notify(
    app: AppHandle,
    title: String,
    body: Option<String>,
    link: Option<String>,
    tag: Option<String>,
) -> Result<(), String> {
    let _ = (link, tag);
    let title = clip(&title, TITLE_MAX);
    if title.is_empty() {
        return Err("A notification needs a title".to_string());
    }
    let notification = app.notification();
    if notification
        .permission_state()
        .is_ok_and(|state| state != PermissionState::Granted)
    {
        match notification.request_permission() {
            Ok(PermissionState::Granted) => {}
            Ok(_) => return Err("Notifications are off for this app".to_string()),
            Err(err) => return Err(format!("Notifications are not allowed: {err}")),
        }
    }
    let mut builder = notification.builder().title(title);
    if let Some(body) = body.map(|body| clip(&body, BODY_MAX)).filter(|body| !body.is_empty()) {
        builder = builder.body(body);
    }
    builder.show().map_err(|err| err.to_string())
}

/// What the shell says of a call to the native code (MT2): `joined` (its media is up), `left` (left, ended, or never
/// joined), `screen` and `offScreen` (its call screen shown or left: over the lock screen the app shows the call's
/// screen only).
pub(crate) const CALL_STATES: [&str; 4] = ["joined", "left", "screen", "offScreen"];

/// The shell's call `callSessionId` is in `state` ([`CALL_STATES`]): the native call (the Telecom call, the foreground
/// service, the showing over the lock screen) is kept while the page has the call and ended after. The shell's only
/// (it owns the calls); the native code acts only on a call it answered.
#[tauri::command(async)]
pub(crate) fn call_state(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    call_session_id: String,
    state: String,
) -> Result<(), String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let id = crate::calls::call_id(&call_session_id).ok_or("Not a call id")?;
    let state = CALL_STATES
        .iter()
        .find(|known| **known == state)
        .ok_or("Not a call state")?;
    #[cfg(target_os = "android")]
    crate::android::page_call_state(id, state)?;
    #[cfg(not(target_os = "android"))]
    log::info!("The call {id} is {state}");
    Ok(())
}

/// Whether the shell's call screen of `callSessionId` answers it by itself: only once, for the call the user answered
/// on the phone's call screen (the host hands it over), never for a link of a page's.
#[tauri::command(async)]
pub(crate) fn call_take_answer(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    call_session_id: String,
) -> Result<bool, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    Ok(host.take_answer(&call_session_id))
}

/// Open `url` outside the app: http and https on the web, mailto, tel and sip.
#[tauri::command(async)]
pub(crate) fn open_external(url: String) -> Result<(), String> {
    let url = external_url(&url)?;
    crate::open_externally(&url);
    Ok(())
}

/// `text` as a URL the system may open: http and https with a host that is not the app's, or mailto, tel and sip.
pub(crate) fn external_url(text: &str) -> Result<Url, String> {
    if text.len() > URL_MAX {
        return Err("The URL is too long".to_string());
    }
    let url = Url::parse(text.trim()).map_err(|err| format!("Not a URL: {err}"))?;
    match url.scheme() {
        "http" | "https" => match url.host_str() {
            None | Some("") => Err("The URL has no host".to_string()),
            Some(host) if crate::host::own_host(host) => {
                Err("The URL is the app's own; open it in the app".to_string())
            }
            Some(_) => Ok(url),
        },
        "mailto" | "tel" | "sip" => Ok(url),
        scheme => Err(format!("Not opened outside the app: {scheme}:")),
    }
}

/// `text` without control characters, trimmed, at most `max` characters.
pub(crate) fn clip(text: &str, max: usize) -> String {
    let cleaned: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    cleaned
        .trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
