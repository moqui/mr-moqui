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

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{DownloadEvent, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, Url, Webview};
use tauri_plugin_notification::NotificationExt;

/// The initialization script of the main window.
pub(crate) const SCRIPT: &str = include_str!("bridge.js");

/// The permissions of the node's origin: the app commands of `build.rs` (`allow-` and the command with `-` for `_`),
/// and the notification plugin's permission check, which its `window.Notification` stand-in asks on every page.
const PERMISSIONS: [&str; 6] = [
    "allow-notify",
    "allow-set-counts",
    "allow-set-presence",
    "allow-open-external",
    "allow-set-comms-root",
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
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            notify,
            set_counts,
            set_presence,
            open_external,
            set_comms_root,
            crate::splash::restart_node,
            crate::client::commands::client_state,
            crate::client::commands::client_sign_in,
            crate::client::commands::client_send_code,
            crate::client::commands::client_sign_out,
            crate::client::commands::client_switch_account,
            crate::client::commands::client_add_account,
            crate::client::commands::client_remove_account,
            crate::client::commands::client_open_remote,
            crate::client::commands::client_sso_providers,
            crate::client::commands::client_sso_start,
            crate::client::commands::client_sso_cancel,
            crate::client::sockets::socket_open,
            crate::client::sockets::socket_send,
            crate::client::sockets::socket_close,
            crate::client::outbox::outbox_add,
            crate::client::outbox::outbox_list,
            crate::client::outbox::outbox_retry,
            crate::client::outbox::outbox_discard,
            crate::client::outbox::outbox_unsent,
            crate::client::outbox::draft_get,
            crate::client::outbox::draft_list,
            crate::client::outbox::draft_put,
            crate::client::outbox::draft_remove,
            crate::client::outbox::file_save
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

/// Follow `link` in the main window, through the shell's `moqui.openLink` so the page stays loaded. Only a link on the
/// node's origin; a client's, on its server, in an app window.
fn open_in_page(app: &AppHandle, link: &str) {
    let Some(url) = node_link(link, app.state::<Bridge>().node.get()) else {
        return;
    };
    if crate::client::active(app) {
        crate::client::open_link(app, &url);
        return;
    }
    if let Some(window) = app.get_webview_window("main") {
        let quoted = serde_json::to_string(url.as_str()).unwrap_or_else(|_| "\"/\"".to_string());
        let _ = window.eval(format!(
            "window.moqui && window.moqui.openLink ? window.moqui.openLink({quoted}) : (window.location.href = {quoted})"
        ));
    }
}

/// A native notification. A click shows the window and follows `link` when it is on the node's origin. `tag` is
/// taken for the page's sake (a Web Notification's tag) and not used: the platform notifications here have none.
#[tauri::command]
pub(crate) fn notify(
    app: AppHandle,
    title: String,
    body: Option<String>,
    link: Option<String>,
    tag: Option<String>,
) -> Result<(), String> {
    let _ = tag;
    let title = clip(&title, TITLE_MAX);
    if title.is_empty() {
        return Err("A notification needs a title".to_string());
    }
    let mut builder = app.notification().builder().title(title);
    if let Some(body) = body.map(|body| clip(&body, BODY_MAX)).filter(|body| !body.is_empty()) {
        builder = builder.body(body);
    }
    let node = app.state::<Bridge>().node.get().cloned();
    if let Some(link) = link.and_then(|link| node_link(&link, node.as_ref())) {
        builder = builder.extra("link", link.as_str());
    }
    builder.show().map_err(|err| err.to_string())
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

/// The node's Comms app (`calls.appRoot` of its pages' config), which the host cannot read from a sidecar's conf: kept
/// for the `mailto:`, `tel:` and `sip:` links (`os::comms_root`). Only an app path, `/qapps/<name>`.
#[tauri::command]
pub(crate) fn set_comms_root(app: AppHandle, root: String) -> Result<(), String> {
    if !comms_root_valid(&root) {
        return Err("Not a Comms app's path".to_string());
    }
    crate::os::comms_root_reported(&app, &root);
    Ok(())
}

/// `/qapps/` and a path of letters, digits, `_`, `-` and `/`, at most 200 characters.
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
pub(crate) fn on_permission(webview: Webview, kind: PermissionKind) -> PermissionResponse {
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
