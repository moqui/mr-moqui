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
//! shell's), a native notification, a link opened outside the app; and the shell's own: the notification check, the
//! native call, the phone's contacts and the calling account of the phone's dialer. Any page of the app may call the
//! first ones, none reaching more than a page could; the shell's check its page and nonce.

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

/// The notification id of a page's `tag` (a room's `chat-<id>`, a call's `call-<id>`) for `account`: the same tag of
/// the same account replaces and removes the same notification, and another account's room of the same id is another
/// notification (plans/Comms-Messaging.md §7.5). None for no tag: a notification of its own.
pub(crate) fn tag_id(account: Option<&str>, tag: Option<&str>) -> Option<i32> {
    let tag = tag.map(str::trim).filter(|tag| !tag.is_empty())?;
    // FNV-1a, kept positive and clear of the ids Android's own notifications of the app use (below 1000)
    let mut hash: u32 = 0x811c_9dc5;
    for byte in account
        .unwrap_or("")
        .bytes()
        .chain(std::iter::once(b'\n'))
        .chain(tag.bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    Some(i32::try_from(hash % 0x7fff_0000).unwrap_or(0) + 1000)
}

/// The tag that clears every room's notification of the account (the page's Mark all read).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) const CHAT_ALL_TAG: &str = "chat-*";

/// The room of a chat tag (`chat-<room>`): none for another tag.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn chat_room_of(tag: &str) -> Option<&str> {
    tag.trim().strip_prefix("chat-").and_then(crate::calls::call_id)
}

/// A native notification: `title`, and `body`, replacing the one of the same `tag` of the account signed in; a tap
/// shows the app at `link` when it is a path of the account's server (`notification_opened`). Android 13 and later ask
/// the user the first time (off the main thread: the plugin waits for the answer).
#[tauri::command(async)]
pub(crate) fn notify(
    app: AppHandle,
    title: String,
    body: Option<String>,
    link: Option<String>,
    tag: Option<String>,
) -> Result<(), String> {
    let title = clip(&title, TITLE_MAX);
    if title.is_empty() {
        return Err("A notification needs a title".to_string());
    }
    let account = host(&app).ok().and_then(|host| host.signed_in());
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
    if let Some(id) = tag_id(account.as_deref(), tag.as_deref()) {
        builder = builder.id(id);
    }
    if let Some(path) = link.as_deref().and_then(|link| server_path(link).ok()) {
        builder = builder.extra("link", path);
    }
    builder.show().map_err(|err| err.to_string())
}

/// Remove the notification of `tag` of the account signed in (a room read here: its notification goes, §7.2 rule 3):
/// the page's own, and a room's the native code shows from a push (`ChatNotifications.clear`; the server sends no
/// clear to the device that read).
#[tauri::command(async)]
pub(crate) fn notify_clear(app: AppHandle, tag: String) -> Result<(), String> {
    let account = host(&app).ok().and_then(|host| host.signed_in());
    #[cfg(target_os = "android")]
    if let Some(account) = account.as_deref() {
        // every room of the account (Mark all read), or one room
        if tag.trim() == CHAT_ALL_TAG {
            crate::android::chat_clear(account, "*")?;
        } else if let Some(room) = chat_room_of(&tag) {
            crate::android::chat_clear(account, room)?;
        }
    }
    let Some(id) = tag_id(account.as_deref(), Some(&tag)) else {
        return Ok(());
    };
    app.notification()
        .remove_active(vec![id])
        .map_err(|err| err.to_string())
}

/// The room the shell shows now (none: no room, or the app left it): its pushes show no notification while the app is
/// in front, as the server sends none to a room read (plans/Comms-Messaging.md §7.5).
#[tauri::command(async)]
pub(crate) fn chat_room_shown(app: AppHandle, room: Option<String>) -> Result<(), String> {
    let host = host(&app)?;
    // the shell's comms screens only: a server's page cannot hide a room's notifications
    shell_only(&host)?;
    let account = host.signed_in().unwrap_or_default();
    let room = room.as_deref().and_then(crate::calls::call_id).unwrap_or("");
    #[cfg(target_os = "android")]
    return crate::android::chat_room_shown(&account, room);
    #[cfg(not(target_os = "android"))]
    {
        log::info!("The room shown: {account}/{room}");
        Ok(())
    }
}

/// A tap on a notification of `notify`: the app shows its link (a path of the account's server) in the shell.
#[cfg_attr(not(mobile), allow(dead_code))]
pub(crate) fn notification_opened(app: &AppHandle, link: Option<&str>) {
    let (Some(path), Ok(host)) = (link.and_then(|link| server_path(link).ok()), host(app)) else {
        return;
    };
    host.open_shell_path(path);
}

/// The phone's notification state for the shell's check screen (plans/Comms-Messaging.md §7.6): the native code's
/// (`permission`, `channels`, `batteryOptimized`, `fullScreen`, `lastPushDate`, `lastPushKind`, `fcm`), and whether the
/// push token is registered with the current account's server (`registered`). The shell's page only.
#[tauri::command(async)]
pub(crate) fn push_check(app: AppHandle) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    shell_only(&host)?;
    let mut state = native_state()?;
    let account = host.signed_in();
    let registered = account
        .as_deref()
        .is_some_and(|account| crate::native::push_registered(host.client_dir(), account));
    state["registered"] = registered.into();
    state["platform"] = "android".into();
    Ok(state)
}

/// Open the system's screen that fixes one item of the check (`notifications`, `channels`, `battery`, `fullScreen`).
#[tauri::command(async)]
pub(crate) fn push_check_fix(app: AppHandle, what: String) -> Result<(), String> {
    let host = host(&app)?;
    shell_only(&host)?;
    if !matches!(
        what.as_str(),
        "notifications"
            | "channels"
            | "battery"
            | "fullScreen"
            | "distributor"
            | "firebase"
            | "callingAccounts"
            | "contacts"
    ) {
        return Err(format!("Nothing to fix: {what}"));
    }
    #[cfg(target_os = "android")]
    return crate::android::push_check_fix(&what);
    #[cfg(not(target_os = "android"))]
    Err("Not on a phone".to_string())
}

/// The check screen's "Send a test notification": the current account's server pushes one to this phone through the
/// full path (§7.6). Its answer `{ackIdList, queuedDate, reasonCode}`; the screen then watches `push_check`'s
/// `lastPushDate` for it.
#[tauri::command(async)]
pub(crate) fn push_test(app: AppHandle) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    shell_only(&host)?;
    let account = host.signed_in().ok_or("No account is signed in")?;
    let state = moqui_client_tauri::state(&app)?;
    crate::native::test_push(&state.core, &account)
}

fn shell_only(host: &MobileHost) -> Result<(), String> {
    use moqui_client::app::ClientHost;
    if host.is_shell(crate::host::MAIN) {
        Ok(())
    } else {
        Err("Only the app's own screens ask this".to_string())
    }
}

/// The native code's part of the check (`PushCheck.state`), or none on the desktop.
fn native_state() -> Result<serde_json::Value, String> {
    #[cfg(target_os = "android")]
    {
        let text = crate::android::push_check_state()?;
        serde_json::from_str(&text).map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "android"))]
    Ok(serde_json::json!({}))
}

/// What the shell says of a call to the native code (MT2): `joined` (its media is up), `left` (left, ended, or never
/// joined), `screen` and `offScreen` (its call screen shown or left: over the lock screen the app shows the call's
/// screen only), `held` (call waiting put it on hold: its leg stays in the call, and its Telecom call goes on hold;
/// `joined` again takes it off), and `video` or `voice` (the call's kind now: its default audio route, and
/// picture-in-picture for video; plans/Comms-Messaging.md §8.3 items 3 to 5).
pub(crate) const CALL_STATES: [&str; 7] = ["joined", "left", "screen", "offScreen", "held", "video", "voice"];

/// The call's audio for the shell's speaker button (plans/Comms-Messaging.md §8.3 item 5): `{route, routes}`, the
/// route in use and the ones Telecom offers (`earpiece`, `speaker`, `wired`, `bluetooth`). The shell's only.
#[tauri::command(async)]
pub(crate) fn call_audio(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    #[cfg(target_os = "android")]
    {
        let text = crate::android::call_audio_state()?;
        serde_json::from_str(&text).map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "android"))]
    Ok(serde_json::json!({ "route": null, "routes": [] }))
}

/// The routes the shell's speaker button may choose.
pub(crate) const AUDIO_ROUTES: [&str; 4] = ["earpiece", "speaker", "wired", "bluetooth"];

/// The shell's speaker button: the call's audio to `route` ([`AUDIO_ROUTES`]) for the rest of the call. Whether
/// Telecom was asked (false: no call in Telecom, or no such route now).
#[tauri::command(async)]
pub(crate) fn call_audio_route(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    route: String,
) -> Result<bool, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let route = AUDIO_ROUTES
        .iter()
        .find(|known| **known == route)
        .ok_or("Not an audio route")?;
    #[cfg(target_os = "android")]
    return crate::android::call_audio_route(route);
    #[cfg(not(target_os = "android"))]
    {
        log::info!("The call's audio to {route}");
        Ok(false)
    }
}

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
    name: Option<String>,
) -> Result<(), String> {
    let host = host(&app)?;
    // the check and the telling under one lock with the page's end, so a joined of a page going is told before its end
    let _calls = moqui_lib::sync::lock(&host.calls);
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let id = crate::calls::call_id(&call_session_id).ok_or("Not a call id")?;
    let state = CALL_STATES
        .iter()
        .find(|known| **known == state)
        .ok_or("Not a call state")?;
    // the call's name for a car or a watch (with joined): the room, or the other people
    let name = call_name(name.as_deref());
    #[cfg(target_os = "android")]
    crate::android::page_call_state(id, state, &name)?;
    #[cfg(not(target_os = "android"))]
    log::info!("The call {id} is {state} {name}");
    Ok(())
}

/// A call's name as a car or a watch shows it: one line, at most 80 characters, no control characters.
pub(crate) fn call_name(name: Option<&str>) -> String {
    clip(name.unwrap_or(""), 80).trim().to_string()
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

/// The shell's word, as it loads for the current account, of whether it has a number to call from (its Dialer's caller
/// IDs, plans/Comms-Messaging.md §9.1): the app's calling account for the phone's dialer is registered while an account
/// here has one, and taken away when none has. The shell's only.
#[tauri::command(async)]
pub(crate) fn call_dial_account(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    can_call: bool,
) -> Result<(), String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let account = host.signed_in().ok_or("No account is signed in")?;
    host.set_dial_account(&account, can_call)
}

/// The number from the phone's dialer waiting for the user's pick of the account that calls it (the current account
/// cannot, several others can): `{number, accounts: [{id, label}]}`, or null. For the Dialer of the account it was
/// shown on, within two minutes.
#[tauri::command(async)]
pub(crate) fn call_dial_choice(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    Ok(match host.dial_choice() {
        Some((number, accounts)) => serde_json::json!({
            "number": number,
            "accounts": accounts.into_iter().map(|(id, label)| serde_json::json!({"id": id, "label": label})).collect::<Vec<_>>(),
        }),
        None => serde_json::Value::Null,
    })
}

/// The user's pick of `account` for the number waiting (`call_dial_choice`): that account's Dialer with the number
/// filled in, the account made current. The user still presses Call.
#[tauri::command(async)]
pub(crate) fn call_dial_from(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    account: String,
) -> Result<(), String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    host.dial_from(&account)
}

/// The most contacts a search gives.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const CONTACTS_MAX: i32 = 10;
/// The most numbers named at once.
const NAMES_MAX: usize = 50;

/// The phone's contacts whose name or number has `term` (plans/Comms-Messaging.md §9.2), for the new-chat picker:
/// `[{name, number}]`, at most ten. The first search asks for the contacts permission (with its reason); without it,
/// none. Read on the phone only, for the shell's page; nothing is sent anywhere.
#[tauri::command(async)]
pub(crate) fn contacts_search(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    term: String,
) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let Some(term) = contact_term(&term) else {
        return Ok(serde_json::json!([]));
    };
    #[cfg(target_os = "android")]
    {
        let text = crate::android::contacts_search(&term, CONTACTS_MAX)?;
        serde_json::from_str(&text).map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        log::info!(
            "No phone contacts here to search for {} characters",
            term.chars().count()
        );
        Ok(serde_json::json!([]))
    }
}

/// The names the phone's contacts have for `numbers` (E.164, at most 50; others are left out): `{number: name}`. Never
/// asks for the permission (a room list is not a use); without it, none. The shell's page only; nothing is sent.
#[tauri::command(async)]
pub(crate) fn contact_names(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    numbers: Vec<String>,
) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let numbers = contact_numbers(&numbers);
    if numbers.is_empty() {
        return Ok(serde_json::json!({}));
    }
    #[cfg(target_os = "android")]
    {
        let text = crate::android::contact_names(&numbers)?;
        serde_json::from_str(&text).map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        log::info!("No phone contacts here to name {} numbers", numbers.len());
        Ok(serde_json::json!({}))
    }
}

/// A contacts search's term: trimmed, no control characters, two to 100 characters; none otherwise.
pub(crate) fn contact_term(term: &str) -> Option<String> {
    let term = clip(term, 100);
    (term.chars().count() >= 2).then_some(term)
}

/// The numbers a page asks names of that are E.164 (`+` and 5 to 15 digits), once each, at most [`NAMES_MAX`].
pub(crate) fn contact_numbers(numbers: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for number in numbers.iter().map(|number| number.trim()) {
        let digits = number.strip_prefix('+').unwrap_or("");
        let e164 = (5..=15).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit());
        if e164 && !out.iter().any(|kept| kept == number) {
            out.push(number.to_string());
        }
        if out.len() >= NAMES_MAX {
            break;
        }
    }
    out
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
