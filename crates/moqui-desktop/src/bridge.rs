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

//! The page bridge: the few native commands the node's pages may call, and the downloads of the window
//! (plans/Mobile-Desktop.md section 4, configuration 1: "notifications, the tray badge and presence, opening external
//! links, and saving files").
//!
//! `bridge.js`, an initialization script of the main window, defines `window.moquiDesktop` over Tauri's invoke on the
//! node's loopback origin; the webroot client calls it where a browser would show a Web Notification or open a window.
//! The commands are declared in the app manifest (`build.rs`), so every app command is checked against the ACL, and
//! the only capability that grants them is the one [`node_started`] adds for the node's exact origin on the main
//! window. Nothing here reads or writes a path the page names, or runs a program but the system's URL opener.
//!
//! Saving a file is Chromium's download: a download link, an export link or an attachment response asks for a place
//! with the save dialog ([`on_download`]), with the page's cookies, streamed to disk; a notification says when it is
//! done.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{DownloadEvent, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, Url, Webview};
use tauri_plugin_notification::NotificationExt;

/// The initialization script of the main window.
pub(crate) const SCRIPT: &str = include_str!("bridge.js");

/// The permissions of the node's origin: the app commands of `build.rs` (`allow-` and the command with `-` for `_`),
/// and the notification plugin's permission check, which its `window.Notification` stand-in asks on every page.
const PERMISSIONS: [&str; 13] = [
    "allow-notify",
    "allow-notify-clear",
    "allow-set-counts",
    "allow-set-presence",
    "allow-open-external",
    "allow-node-setup-secret",
    "allow-secure-screen",
    "allow-capture-sources",
    "allow-capture-choose",
    "allow-capture-overlay",
    "allow-capture-ink",
    "allow-capture-overlay-close",
    "notification:allow-is-permission-granted",
];

/// The sockets of the node's pages in process (`client::sockets`).
const SOCKET_PERMISSIONS: [&str; 3] = ["allow-socket-open", "allow-socket-send", "allow-socket-close"];

/// The longest title and body of a notification, in characters; more is cut.
const TITLE_MAX: usize = 200;
const BODY_MAX: usize = 1000;
/// The longest URL the page may hand over.
const URL_MAX: usize = 8192;

/// The bridge's state: the node's origin once it listens, and what the tray shows.
#[derive(Default)]
pub(crate) struct Bridge {
    node: OnceLock<Url>,
    tray: Mutex<TrayState>,
    /// The reasons each window's screen protection is held for, by window label ([`secure_screen`]).
    secured: Mutex<HashMap<String, BTreeSet<String>>>,
}

#[derive(Default)]
struct TrayState {
    count: u64,
    presence: Option<&'static str>,
}

/// The bridge on the app: its state, the notification plugin and the commands. The one invoke handler of the app, so
/// it also has the splash page's [`crate::splash::restart_node`], which only the app's own pages may call
/// (`capabilities/splash.json`).
pub(crate) fn setup(builder: tauri::Builder) -> tauri::Builder {
    builder
        .manage(Bridge::default())
        .manage(crate::capture::Capture::default())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            notify,
            notify_clear,
            push_check,
            push_test,
            set_counts,
            set_presence,
            open_external,
            node_setup_secret,
            secure_screen,
            crate::capture::capture_sources,
            crate::capture::capture_choose,
            crate::capture::capture_overlay,
            crate::capture::capture_ink,
            crate::capture::capture_overlay_close,
            crate::splash::restart_node,
            moqui_client_tauri::client_state,
            moqui_client_tauri::client_sign_in,
            moqui_client_tauri::client_send_code,
            moqui_client_tauri::client_sign_out,
            moqui_client_tauri::client_switch_account,
            moqui_client_tauri::client_add_account,
            moqui_client_tauri::client_remove_account,
            moqui_client_tauri::client_open_remote,
            moqui_client_tauri::client_take_open,
            crate::client::windows::open_in_shell,
            moqui_client_tauri::client_sso_providers,
            moqui_client_tauri::client_sso_start,
            moqui_client_tauri::client_sso_cancel,
            crate::client::sockets::socket_open,
            crate::client::sockets::socket_send,
            crate::client::sockets::socket_close,
            moqui_client_tauri::outbox_add,
            moqui_client_tauri::outbox_list,
            moqui_client_tauri::outbox_kinds,
            moqui_client_tauri::outbox_retry,
            moqui_client_tauri::outbox_discard,
            moqui_client_tauri::outbox_unsent,
            moqui_client_tauri::draft_get,
            moqui_client_tauri::draft_list,
            moqui_client_tauri::draft_put,
            moqui_client_tauri::draft_remove,
            moqui_client_tauri::file_save,
            moqui_client_tauri::vault_call,
            moqui_client_tauri::vault_memory,
            moqui_client_tauri::packages_list,
            moqui_client_tauri::packages_download,
            moqui_client_tauri::packages_cancel,
            moqui_client_tauri::packages_delete,
            moqui_client_tauri::packages_status
        ])
}

/// The node listens on `url`: its origin may call the commands from now on, and a click on a notification opens the
/// notification's link. Once per run; a node that starts again is on the same port.
pub(crate) fn node_started(app: &AppHandle, url: &Url) {
    let bridge = app.state::<Bridge>();
    if bridge.node.set(url.clone()).is_err() {
        return;
    }
    let origin = url.origin().ascii_serialization();
    if let Err(err) = app.add_capability(capability(&origin)) {
        tracing::error!("The page bridge is off: no capability for {origin}: {err}");
    }
    on_notification_click(app);
}

/// A client of a server (src/client/): its pages' origin is the app windows', where notification links and downloads
/// are; the client adds the capabilities of its own origins.
pub(crate) fn client_started(app: &AppHandle) {
    let _ = app.state::<Bridge>().node.set(crate::client::app_origin());
    on_notification_click(app);
}

/// A click on a notification shows the window and opens the notification's link.
fn on_notification_click(app: &AppHandle) {
    let handle = app.clone();
    let registered = app.notification().on_action(move |performed| {
        if performed.action_id() != "tap" {
            return;
        }
        crate::show_main(&handle);
        let link = performed
            .notification()
            .and_then(|notification| notification.extra().get("link"));
        if let Some(link) = link.and_then(|link| link.as_str()) {
            open_in_page(&handle, link);
        }
    });
    if let Err(err) = registered {
        tracing::warn!("Notification clicks are not handled: {err}");
    }
}

/// The capability of the node's pages: the bridge's permissions for `origin` (`http://127.0.0.1:<port>`, any path) on
/// the main window, and not for the app's own pages. The node in process is on the app scheme
/// (`node::scheme::NODE_ORIGIN`), whose pages Tauri counts as local: its capability names the main window, whose
/// navigation stays on the node's origin and the app's own pages, and has the sockets too, which the host holds.
fn capability(origin: &str) -> CapabilityBuilder {
    if origin == crate::node::scheme::NODE_ORIGIN {
        let capability = CapabilityBuilder::new("node-bridge").local(true).window("main");
        return PERMISSIONS
            .iter()
            .chain(SOCKET_PERMISSIONS.iter())
            .fold(capability, |capability, permission| capability.permission(*permission));
    }
    let capability = CapabilityBuilder::new("node-bridge")
        .remote(origin.to_string())
        .local(false)
        .window("main");
    PERMISSIONS
        .iter()
        .fold(capability, |capability, permission| capability.permission(*permission))
}

/// What the host evaluates in the main window when it gains or loses the focus, or hides to the tray: the page's
/// `moquiDesktop.windowSeen()` answers it.
pub(crate) fn focus_script(focused: bool) -> String {
    format!("window.__moquiWindowFocused = {focused};")
}

/// Tell the main window's page whether the user sees it (focused, not hidden to the tray).
pub(crate) fn tell_focus(app: &AppHandle, focused: bool) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.eval(focus_script(focused)) {
            tracing::debug!("The window was not told of its focus: {err}");
        }
    }
}

/// Follow `link` in the main window, through the shell's `moqui.openLink` so the page stays loaded. Only a link on the
/// node's origin; a client's, on its server, in the shell when it bundles the screen, else in an app window.
fn open_in_page(app: &AppHandle, link: &str) {
    let Some(url) = node_link(link, app.state::<Bridge>().node.get()) else {
        return;
    };
    if crate::client::active(app) {
        crate::client::open_page(app, &url);
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let quoted = serde_json::to_string(url.as_str()).unwrap_or_else(|_| "\"/\"".to_string());
        let _ = window.eval(format!(
            "window.moqui && window.moqui.openLink ? window.moqui.openLink({quoted}) : (window.location.href = {quoted})"
        ));
    }
}

/// The notification id of a page's `tag` (a room's `chat-<id>`, a call's `call-<id>`) for `account` (the client's
/// current account; none on a node): the same tag of the same account replaces and removes the same notification, and
/// another account's room of the same id is another (plans/Comms-Messaging.md §7.5). None for no tag.
pub(crate) fn tag_id(account: Option<&str>, tag: Option<&str>) -> Option<i32> {
    let tag = tag.map(str::trim).filter(|tag| !tag.is_empty())?;
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

/// The account a page's notification is of: the client's current one (its shell and its app windows), none on a node.
fn notify_account(app: &AppHandle) -> Option<String> {
    let client = app.try_state::<crate::client::Client>()?;
    client.core.current().map(|(account, _)| account.id)
}

/// A native notification. A click shows the window and follows `link` when it is on the node's origin. A `tag` (a
/// room's) replaces the notification of the same tag of the same account: the earlier one is closed first where the
/// platform can (Linux and the BSDs; on macOS and Windows a shown notification stays, `remove_active`).
#[tauri::command]
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
    let mut builder = app.notification().builder().title(title);
    if let Some(body) = body.map(|body| clip(&body, BODY_MAX)).filter(|body| !body.is_empty()) {
        builder = builder.body(body);
    }
    if let Some(id) = tag_id(notify_account(&app).as_deref(), tag.as_deref()) {
        let _ = app.notification().remove_active(vec![id]);
        builder = builder.id(id);
    }
    let node = app.state::<Bridge>().node.get().cloned();
    if let Some(link) = link.and_then(|link| node_link(&link, node.as_ref())) {
        builder = builder.extra("link", link.as_str());
    }
    builder.show().map_err(|err| err.to_string())
}

/// Remove the notification of `tag` of the current account (a room read: its notification goes, §7.2 rule 3), where
/// the platform can.
#[tauri::command]
pub(crate) fn notify_clear(app: AppHandle, tag: String) -> Result<(), String> {
    match tag_id(notify_account(&app).as_deref(), Some(&tag)) {
        Some(id) => app
            .notification()
            .remove_active(vec![id])
            .map_err(|err| err.to_string()),
        None => Ok(()),
    }
}

/// The desktop's part of the shell's notification check (plans/Comms-Messaging.md §7.6): whether the platform lets the
/// app notify. The desktop gets its messages by its socket while it runs, so it has no push token, channels or battery
/// optimization: `{platform: desktop, permission, socket: true}`.
#[tauri::command]
pub(crate) fn push_check(app: AppHandle) -> serde_json::Value {
    let permission = app
        .notification()
        .permission_state()
        .map(|state| matches!(state, tauri_plugin_notification::PermissionState::Granted))
        .unwrap_or(false);
    serde_json::json!({ "platform": "desktop", "permission": permission, "socket": true })
}

/// The check's test on the desktop: a notification shown by the app, as its socket's notifications are; how long it
/// took to hand to the platform, in milliseconds.
#[tauri::command]
pub(crate) fn push_test(app: AppHandle, title: String, body: Option<String>) -> Result<serde_json::Value, String> {
    let started = std::time::Instant::now();
    notify(app, title, body, None, Some("moqui-test".to_string()))?;
    Ok(serde_json::json!({ "shownMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX) }))
}

/// The shell's counts (`shellCounts`, each a number): their total on the tray and the dock or launcher badge.
#[tauri::command]
pub(crate) fn set_counts(app: AppHandle, counts: HashMap<String, f64>) {
    let total = count_total(&counts);
    let state = {
        let bridge = app.state::<Bridge>();
        let mut tray = moqui_lib::sync::lock(&bridge.tray);
        if tray.count == total {
            return;
        }
        tray.count = total;
        (tray.count, tray.presence)
    };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_title(badge(total).as_deref());
        let _ = tray.set_tooltip(Some(tooltip(state.0, state.1)));
    }
    if let Some(window) = app.get_webview_window("main") {
        // macOS and Linux; Windows has an overlay icon instead, not set here
        let _ = window.set_badge_count((total > 0).then(|| total.min(i64::MAX as u64) as i64));
    }
}

/// My own presence (online, away, dnd, offline): on the tray's tooltip.
#[tauri::command]
pub(crate) fn set_presence(app: AppHandle, status: String) -> Result<(), String> {
    let label = presence_label(&status).ok_or_else(|| format!("Not a presence: {}", clip(&status, 40)))?;
    let state = {
        let bridge = app.state::<Bridge>();
        let mut tray = moqui_lib::sync::lock(&bridge.tray);
        tray.presence = Some(label);
        (tray.count, tray.presence)
    };
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip(state.0, state.1)));
    }
    Ok(())
}

/// Open `url` in the system browser or the handler of its scheme: http, https, mailto, tel and sip, and not the node.
#[tauri::command]
pub(crate) fn open_external(app: AppHandle, url: String) -> Result<(), String> {
    let url = external_url(&url, app.state::<Bridge>().node.get())?;
    crate::open_externally(&url);
    Ok(())
}

/// The setup secret of a sidecar's first administrator, for its window's Login form (`bridge.js`): the sidecar asks
/// every caller for it, its window too, as any account of the computer reaches its loopback port, and only the app and
/// its window have it (plans/Security-Review.md SEC-CLI-101). None for the node in process, whose window needs none.
#[tauri::command]
pub(crate) fn node_setup_secret(app: AppHandle) -> Option<String> {
    let sidecar = crate::node_mode(&app) == Some(crate::node::Mode::Sidecar);
    sidecar
        .then(crate::node::setup_secret)
        .filter(|secret| !secret.is_empty())
        .map(str::to_string)
}

/// Whether the OS can keep a window out of screenshots and screen recording: macOS (`NSWindow.sharingType`) and Windows
/// (`WDA_EXCLUDEFROMCAPTURE`); a Linux desktop cannot, where `bridge.js` offers no `secureScreen`.
const SCREEN_PROTECTION: bool = cfg!(any(target_os = "macos", windows));

/// Screen protection of the asking page's window for `reason` (`viewOnce` while a view-once photo is open, a room's
/// or the app's while such a setting holds), on or off: the window is kept out of screenshots and screen recording while
/// any reason holds it, as the phone's `FLAG_SECURE` (plans/Security-Review.md SEC-CLI-117). Whether it is on now:
/// false where the OS has none, so the page says a screenshot cannot be stopped there.
#[tauri::command]
pub(crate) fn secure_screen(app: AppHandle, webview: Webview, reason: String, on: bool) -> bool {
    let reason = clip(&reason, 100);
    if reason.is_empty() {
        return false;
    }
    let window = webview.window();
    let held = {
        let state = app.state::<Bridge>();
        let mut secured = moqui_lib::sync::lock(&state.secured);
        hold(secured.entry(window.label().to_string()).or_default(), &reason, on)
    };
    if let Err(err) = window.set_content_protected(held) {
        tracing::warn!("The window's screen protection was not set: {err}");
        return false;
    }
    held && SCREEN_PROTECTION
}

/// `reason` held or let go in `reasons`; whether any reason holds the protection now.
pub(crate) fn hold(reasons: &mut BTreeSet<String>, reason: &str, on: bool) -> bool {
    if on {
        reasons.insert(reason.to_string());
    } else {
        reasons.remove(reason);
    }
    !reasons.is_empty()
}

/// Whether `root` is a Comms app's path (the node's `comms@app-root`, `os::conf_comms_root`): `/qapps/` and a path of
/// letters, digits, `_`, `-` and `/`, at most 200 characters.
pub(crate) fn comms_root_valid(root: &str) -> bool {
    root.len() <= 200
        && root.strip_prefix("/qapps/").is_some_and(|rest| {
            !rest.is_empty()
                && !rest.contains("//")
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/'))
        })
}

/// The window's downloads: from the node (its URLs, and the page's `blob:` and `data:` links) Chromium asks where with
/// the save dialog, as the destination is left as it suggests; a notification says when one is saved. Anything else is
/// refused (the window shows only the node and the splash page).
pub(crate) fn on_download(webview: Webview, event: DownloadEvent<'_>) -> bool {
    let app = webview.app_handle();
    match event {
        DownloadEvent::Requested { url, .. } => {
            // a client's: from its app windows' origin (the bridge's) or its shell's
            let allowed = download_allowed(&url, app.state::<Bridge>().node.get())
                || (crate::client::active(app) && download_allowed(&url, Some(&crate::client::shell_origin())));
            if !allowed {
                tracing::warn!("Download refused, not from the node: {}", clip(url.as_str(), 200));
            }
            allowed
        }
        DownloadEvent::Finished { path, success, .. } => {
            if let (true, Some(path)) = (success, path) {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let shown = app
                    .notification()
                    .builder()
                    .title(clip(&format!("Saved {name}"), TITLE_MAX))
                    .body(clip(&path.display().to_string(), BODY_MAX))
                    .show();
                if let Err(err) = shown {
                    tracing::warn!("Saved {}, no notification: {err}", path.display());
                }
            }
            true
        }
        _ => true,
    }
}

/// The camera and microphone of a page (CEF asks on each `getUserMedia`): granted to the app's own pages (the node's,
/// or a client's shell and app windows) for calls, where the OS still asks the user once, and refused to any other
/// page. With no answer CEF's Alloy style (every webview on macOS) would grant them to any page. The other kinds are
/// left to the runtime.
///
/// Capturing the screen (`getDisplayMedia`, and on macOS the app's own picker's request) is for the same pages only:
/// [`crate::capture::answer`].
pub(crate) fn on_permission(webview: Webview, kind: PermissionKind) -> PermissionResponse {
    if matches!(kind, PermissionKind::DisplayCapture) {
        let app = webview.app_handle();
        let shell = crate::client::active(app).then(crate::client::shell_origin);
        let allowed = webview
            .url()
            .is_ok_and(|url| media_allowed(&url, app.state::<Bridge>().node.get(), shell.as_ref()));
        if !allowed {
            tracing::warn!("Screen capture refused to a page off the app's origins");
        }
        return crate::capture::answer(app, &webview, allowed);
    }
    if !matches!(kind, PermissionKind::Camera | PermissionKind::Microphone) {
        return PermissionResponse::Default;
    }
    let app = webview.app_handle();
    let shell = crate::client::active(app).then(crate::client::shell_origin);
    match webview.url() {
        Ok(url) if media_allowed(&url, app.state::<Bridge>().node.get(), shell.as_ref()) => PermissionResponse::Allow,
        Ok(url) => {
            tracing::warn!("Camera and microphone refused to {}", clip(url.as_str(), 200));
            PermissionResponse::Deny
        }
        Err(_) => PermissionResponse::Deny,
    }
}

/// Whether a page at `url` may have the camera and microphone: on the node's origin, or a client's shell's.
pub(crate) fn media_allowed(url: &Url, node: Option<&Url>, shell: Option<&Url>) -> bool {
    matches!(url.scheme(), "http" | "https")
        && [node, shell]
            .into_iter()
            .flatten()
            .any(|origin| origin.origin() == url.origin())
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

/// `link` as a URL on the node's origin, or none.
pub(crate) fn node_link(link: &str, node: Option<&Url>) -> Option<Url> {
    let node = node?;
    if link.len() > URL_MAX {
        return None;
    }
    let url = node.join(link.trim()).ok()?;
    (url.origin() == node.origin()).then_some(url)
}

/// `text` as a URL the system may open: http and https off the node, with a host, or mailto, tel and sip.
pub(crate) fn external_url(text: &str, node: Option<&Url>) -> Result<Url, String> {
    if text.len() > URL_MAX {
        return Err("The URL is too long".to_string());
    }
    let url = Url::parse(text.trim()).map_err(|err| format!("Not a URL: {err}"))?;
    match url.scheme() {
        "http" | "https" => {
            if url.host_str().is_none_or(str::is_empty) {
                return Err("The URL has no host".to_string());
            }
            if node.is_some_and(|node| node.origin() == url.origin()) {
                return Err("The URL is on the node; open it in the window".to_string());
            }
            // a client's own origins are the app's, not the web's
            if matches!(url.host_str(), Some("moqui-shell.localhost" | "moqui-app.localhost")) {
                return Err("The URL is the app's own; open it in the app".to_string());
            }
            Ok(url)
        }
        "mailto" | "tel" | "sip" => Ok(url),
        scheme => Err(format!("Not opened outside the app: {scheme}:")),
    }
}

/// Whether the window may download `url`: on the node's origin, a `blob:` of it, or a `data:` URL.
pub(crate) fn download_allowed(url: &Url, node: Option<&Url>) -> bool {
    match url.scheme() {
        "data" => true,
        // a blob URL's origin is that of the URL inside it
        "http" | "https" | "blob" => node.is_some_and(|node| url.origin() == node.origin()),
        _ => false,
    }
}

/// The total of the shell's counts: the whole part of each finite count above zero.
pub(crate) fn count_total(counts: &HashMap<String, f64>) -> u64 {
    counts
        .values()
        .filter(|count| count.is_finite() && **count >= 1.0)
        .fold(0u64, |sum, count| sum.saturating_add(count.min(u64::MAX as f64) as u64))
}

/// The tray's title beside the icon: the total, 99+ above 99, none at zero.
pub(crate) fn badge(total: u64) -> Option<String> {
    match total {
        0 => None,
        1..=99 => Some(total.to_string()),
        _ => Some("99+".to_string()),
    }
}

/// What a presence is called, for the four the chat store gives; none for anything else.
pub(crate) fn presence_label(status: &str) -> Option<&'static str> {
    match status {
        "online" => Some("Online"),
        "away" => Some("Away"),
        "dnd" => Some("Do not disturb"),
        "offline" => Some("Offline"),
        _ => None,
    }
}

/// The tray's tooltip: the app, my presence and the total of the counts.
pub(crate) fn tooltip(total: u64, presence: Option<&str>) -> String {
    let mut text = "MR Moqui".to_string();
    if let Some(presence) = presence {
        text.push_str(" · ");
        text.push_str(presence);
    }
    if total > 0 {
        text.push_str(&format!(" · {total} new"));
    }
    text
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;
