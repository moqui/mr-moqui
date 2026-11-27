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

/// The title of a page's notification (SEC-CLI-119), which shows as the app's own: the shell's (`from_shell`, by its
/// page and nonce) as it gave it; any other page's after the host of the server its pages were opened for (`server`),
/// so a page of one account's server cannot speak as the app or as another server.
pub(crate) fn notification_title(title: &str, from_shell: bool, server: Option<&str>) -> String {
    if from_shell {
        return title.to_string();
    }
    let server = server
        .map(str::trim)
        .filter(|server| !server.is_empty())
        .unwrap_or("A server page");
    format!("{server}: {title}")
}

/// A native notification: `title`, and `body`, replacing the one of the same `tag` of the account signed in; a tap
/// shows the app at `link` when it is a path of the account's server (`notification_opened`), which the notification
/// carries with the account and the app's signature of both (SEC-CLI-88). Without the notification permission it
/// fails and never asks (SEC-CLI-120): any server page may call it, and Android's question comes with its reason once
/// an account signs in (`MainActivity.askForCallsOnce`) or from the notification check's fix. A server page's title
/// names its server (`notification_title`); the shell's carries its nonce.
#[tauri::command(async)]
pub(crate) fn notify(
    app: AppHandle,
    webview: tauri::Webview,
    title: String,
    body: Option<String>,
    link: Option<String>,
    tag: Option<String>,
    nonce: Option<String>,
) -> Result<(), String> {
    let title = clip(&title, TITLE_MAX);
    if title.is_empty() {
        return Err("A notification needs a title".to_string());
    }
    let (account, from_shell, server) = match host(&app) {
        Ok(host) => (
            host.signed_in(),
            moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()),
            host.page.app_server(),
        ),
        Err(_) => (None, false, None),
    };
    let title = notification_title(&title, from_shell, server.as_deref());
    let notification = app.notification();
    if notification
        .permission_state()
        .is_ok_and(|state| state != PermissionState::Granted)
    {
        return Err("Notifications are off for this app".to_string());
    }
    let mut builder = notification.builder().title(title);
    if let Some(body) = body.map(|body| clip(&body, BODY_MAX)).filter(|body| !body.is_empty()) {
        builder = builder.body(body);
    }
    if let Some(id) = tag_id(account.as_deref(), tag.as_deref()) {
        builder = builder.id(id);
    }
    if let Some(path) = link.as_deref().and_then(|link| server_path(link).ok()) {
        // the activity is exported, and the notification plugin takes any app's intent with its extras as a tap: the
        // link opens only with the app's signature of it and its account (`tapped_path`, SEC-CLI-88)
        match (account.as_deref(), link_key(&app)) {
            (Some(account), Some(key)) => {
                builder = builder
                    .extra("link", path)
                    .extra("account", account)
                    .extra("sig", crate::calls::notify_signature(&key, account, path));
            }
            _ => log::info!("A page's notification with no account or link key: its tap only shows the app"),
        }
    }
    builder.show().map_err(|err| err.to_string())
}

/// The app's key for its own links (`native::link_key`), when the client is made.
fn link_key(app: &AppHandle) -> Option<Vec<u8>> {
    let state = moqui_client_tauri::state(app).ok()?;
    crate::native::link_key(&state.core)
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

/// The room the shell shows now (none: no room): its pushes show no notification while the app is in front, as the
/// server sends none to a room read (plans/Comms-Messaging.md §7.5). The shell's page only, by its nonce (SEC-CLI-97);
/// the room the shell left goes with its page (`MobileHost::page_gone`), as a `null` sent while it leaves has no nonce.
#[tauri::command(async)]
pub(crate) fn chat_room_shown(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    room: Option<String>,
) -> Result<(), String> {
    let host = host(&app)?;
    // the shell's comms screens only: a server's page cannot hide a room's notifications
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
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

/// A tap on a notification of `notify`, with its extras `link`, `account` and `sig`: the app shows the link (a path of
/// the account's server) in the shell only when this app signed it for the current account ([`tapped_path`]); else the
/// tap only shows the app, which Android has done. Any app can send the exported activity such a tap (SEC-CLI-88).
#[cfg_attr(not(mobile), allow(dead_code))]
pub(crate) fn notification_opened(app: &AppHandle, link: Option<&str>, account: Option<&str>, sig: Option<&str>) {
    let (Some(link), Ok(host)) = (link, host(app)) else {
        return;
    };
    let current = host.signed_in();
    let path = link_key(app).and_then(|key| tapped_path(&key, current.as_deref(), Some(link), account, sig));
    match path {
        Some(path) => host.open_shell_path(path),
        // the path is not logged: it may hold a token (SEC-CLI-112)
        None => {
            log::warn!("moqui-mobile: a notification's tap the app did not sign for the current account opens no page")
        }
    }
}

/// The server path a tap on a page's notification opens (SEC-CLI-88): its `link`, when `sig` is this app's signature
/// (with its link `key`) of `account` and that path and `account` is the current one (`current`); none for a tap
/// another app made up, a changed link, or an account signed out or not current, whose tap only shows the app. Links
/// may be any server path (MoquiLib's notifications link anywhere), so the signature is what is checked.
#[cfg_attr(not(mobile), allow(dead_code))]
pub(crate) fn tapped_path<'a>(
    key: &[u8],
    current: Option<&str>,
    link: Option<&'a str>,
    account: Option<&str>,
    sig: Option<&str>,
) -> Option<&'a str> {
    let path = server_path(link?).ok()?;
    let account = account?;
    if current != Some(account) {
        return None;
    }
    let expected = crate::calls::notify_signature(key, account, path);
    crate::calls::signature_matches(&expected, sig?).then_some(path)
}

/// The phone's notification state for the shell's check screen (plans/Comms-Messaging.md §7.6): the native code's
/// (`permission`, `channels`, `batteryOptimized`, `fullScreen`, `lastPushDate`, `lastPushKind`, `fcm`), and whether the
/// push token is registered with the current account's server (`registered`). The shell's page only, by its nonce
/// (SEC-CLI-97).
#[tauri::command(async)]
pub(crate) fn push_check(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
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
/// The shell's page only, by its nonce: `firebase` takes back every UnifiedPush instance (SEC-CLI-97).
#[tauri::command(async)]
pub(crate) fn push_check_fix(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    what: String,
) -> Result<(), String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
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
/// `lastPushDate` for it. The shell's page only, by its nonce (SEC-CLI-97).
#[tauri::command(async)]
pub(crate) fn push_test(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
) -> Result<serde_json::Value, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let account = host.signed_in().ok_or("No account is signed in")?;
    let state = moqui_client_tauri::state(&app)?;
    crate::native::test_push(&state.core, &account)
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

/// Screen security for `reason` (`ScreenSecurity.set`, Android's `FLAG_SECURE`; plans/Comms-Messaging.md §18.3 phase 16):
/// whether it is on now. Elsewhere nothing is done and the answer is false. Only for the shell's page.
#[tauri::command(async)]
pub(crate) fn secure_screen(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    reason: String,
    on: bool,
) -> Result<bool, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let reason: String = reason
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || ":-_".contains(*c))
        .take(64)
        .collect();
    if reason.is_empty() {
        return Err("No reason".to_string());
    }
    #[cfg(target_os = "android")]
    return crate::android::secure_screen(&reason, on);
    #[cfg(not(target_os = "android"))]
    {
        log::info!("Screen security for {reason}: {on}");
        Ok(false)
    }
}

/// An event the shell asks the phone's calendar app to add (plans/Comms-Messaging.md §18.3 phase 19).
#[derive(serde::Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct CalendarEntry {
    title: String,
    start: i64,
    end: i64,
    all_day: bool,
    time_zone: String,
    place: String,
    description: String,
}

/// The phone's calendar app opened on a new event filled in from `entry` (`CalendarIntent.insert`, Android's
/// `ACTION_INSERT`: the calendar app saves it, so the app needs no calendar permission). Whether it opened; false
/// elsewhere and when no calendar app takes it, so the page offers the .ics file. Only for the shell's page.
#[tauri::command(async)]
pub(crate) fn calendar_insert(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    entry: CalendarEntry,
) -> Result<bool, String> {
    let host = host(&app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), &webview, nonce.as_deref()) {
        return Err("Not the shell's page".to_string());
    }
    let title = clip(&entry.title, 200);
    if title.is_empty() || entry.start <= 0 || entry.end <= entry.start {
        return Err("An event needs its title, start and end".to_string());
    }
    let zone: String = entry
        .time_zone
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "/_+-".contains(*c))
        .take(64)
        .collect();
    let place = clip(&entry.place, 300);
    let description = clip_lines(&entry.description, 2000);
    #[cfg(target_os = "android")]
    return crate::android::calendar_insert(
        &title,
        entry.start,
        entry.end,
        entry.all_day,
        &zone,
        &place,
        &description,
    );
    #[cfg(not(target_os = "android"))]
    {
        log::info!(
            "Calendar entry {title} at {} for {} ({zone}, {place}, {} characters)",
            entry.start,
            entry.all_day,
            description.len()
        );
        Ok(false)
    }
}

/// `text` with its line breaks kept and other control characters dropped, trimmed, at most `max` characters.
pub(crate) fn clip_lines(text: &str, max: usize) -> String {
    let cleaned: String = text.chars().filter(|c| *c == '\n' || !c.is_control()).collect();
    cleaned
        .trim()
        .chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_string()
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

/// Open `url` outside the app: http and https on the web, mailto, tel, sip and geo.
#[tauri::command(async)]
pub(crate) fn open_external(url: String) -> Result<(), String> {
    let url = external_url(&url)?;
    crate::open_externally(&url);
    Ok(())
}

/// `text` as a URL the system may open: http and https with a host that is not the app's, or mailto, tel, sip and geo.
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
        // geo: a chat location's Open in maps, for the phone's maps app (plans/Comms-Messaging.md §18.3 phase 20)
        "mailto" | "tel" | "sip" | "geo" => Ok(url),
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

/// A server page's request to the web (`moquiDesktop.webRequest`): see the module's text. The main webview's server
/// pages only (`moqui-app`), not the shell and not a frame (the host takes no IPC from frames).
/// The request is blocking (reqwest's blocking client has its own runtime), so it runs on a blocking thread: in an
/// async worker that runtime would panic when it is dropped.
#[tauri::command]
pub(crate) async fn web_request(
    webview: tauri::Webview,
    method: Option<String>,
    url: String,
    headers: Option<Vec<(String, String)>>,
    body: Option<String>,
    hosts: Vec<String>,
) -> Result<crate::web::WebAnswer, String> {
    let page = webview.url().map_err(|err| err.to_string())?;
    if webview.label() != crate::host::MAIN || page.origin().ascii_serialization() != moqui_client_tauri::APP_ORIGIN {
        return Err("Not a server page".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        crate::web::request(method.as_deref().unwrap_or("GET"), &url, &headers.unwrap_or_default(), body.as_deref(), &hosts)
    })
    .await
    .map_err(|err| format!("The web request stopped: {err}"))?
}

/// The shell's page, or an error: the screen share is the call's, which the shell owns.
fn require_shell(app: &AppHandle, webview: &tauri::Webview, nonce: Option<&str>) -> Result<(), String> {
    let host = host(app)?;
    if !moqui_client_tauri::ShellHost::is_shell_page(host.as_ref(), webview, nonce) {
        return Err("Not the shell's page".to_string());
    }
    Ok(())
}

/// The native screen share (plans/archive/MM-Spike.md): ask the system's consent to capture the screen; once given the
/// capture starts and its frames go to the leg the page connects (`screen_offer`). Whether the consent was asked.
#[tauri::command(async)]
pub(crate) fn screen_start(app: AppHandle, webview: tauri::Webview, nonce: Option<String>) -> Result<bool, String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    #[cfg(target_os = "android")]
    return crate::android::screen_request();
    #[cfg(not(target_os = "android"))]
    Err("No screen capture here".to_string())
}

/// The screen leg's offer, for the page's `join` frame of the leg `add#ScreenLeg` made; `encrypted` for an end-to-end
/// encrypted call, whose leg sends nothing before its key (`screen_key`).
#[tauri::command(async)]
pub(crate) fn screen_offer(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    encrypted: Option<bool>,
) -> Result<String, String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    crate::screen::offer(encrypted == Some(true))
}

/// The screen leg's SFrame key in an end-to-end encrypted call: its KID and base key (base64), from the page's call
/// keys, used after `useAfterMs` (none for the first). The shell's page only: the key is its own leg's sender key.
#[tauri::command(async)]
pub(crate) fn screen_key(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    kid: u64,
    key: String,
    use_after_ms: Option<u64>,
) -> Result<(), String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    use base64::Engine as _;
    let base = base64::engine::general_purpose::STANDARD
        .decode(key.trim())
        .map_err(|_| "Not a key".to_string())?;
    crate::screen::set_key(
        kid,
        base,
        std::time::Duration::from_millis(use_after_ms.unwrap_or(0).min(10_000)),
    )
}

/// The media server's answer to the screen leg's offer (its `joined` frame).
#[tauri::command(async)]
pub(crate) fn screen_answer(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    sdp: String,
) -> Result<(), String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    crate::screen::answer(sdp)
}

/// An offer of the media server's to the screen leg: the answer the page sends back.
#[tauri::command(async)]
pub(crate) fn screen_remote_offer(
    app: AppHandle,
    webview: tauri::Webview,
    nonce: Option<String>,
    sdp: String,
) -> Result<String, String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    crate::screen::remote_offer(sdp)
}

/// The page ends the share (its Stop, the call left): the leg, the projection and its notification.
#[tauri::command(async)]
pub(crate) fn screen_stop(app: AppHandle, webview: tauri::Webview, nonce: Option<String>) -> Result<(), String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    crate::screen::stop();
    #[cfg(target_os = "android")]
    crate::android::screen_stop()?;
    Ok(())
}

/// The share's state as JSON: the leg's (`leg`) and the capture's (`capture`).
#[tauri::command(async)]
pub(crate) fn screen_status(app: AppHandle, webview: tauri::Webview, nonce: Option<String>) -> Result<String, String> {
    require_shell(&app, &webview, nonce.as_deref())?;
    #[cfg(target_os = "android")]
    let capture = crate::android::screen_state().unwrap_or_else(|_| "null".to_string());
    #[cfg(not(target_os = "android"))]
    let capture = "null".to_string();
    Ok(format!(
        r#"{{"leg":{},"capture":{}}}"#,
        crate::screen::status(),
        capture
    ))
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
