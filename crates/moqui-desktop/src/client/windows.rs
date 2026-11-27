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

//! The client's windows (plans/Mobile-Desktop.md §4, "Remote applications and authentication"): the shell, the main
//! window, on the `moqui-shell` origin with the shell's commands; and an app window (`remote-<n>`) for each server page
//! opened from it, on the `moqui-app` origin with the sockets, the outbox and the drafts only. Each stays on its origin: a link elsewhere on the web
//! opens in the system browser. An app window is one account's, named in its storage scope, and closes when the
//! account changes or signs out. The comms app's screens the shell bundles (Chat, the Inbox, Compose, a call and the
//! Dialer) are shown in the shell instead, from a link of the shell's, the OS (`mailto:`, `tel:`, `sip:`) or a notification
//! ([`open_page`]).
//!
//! Every account's server pages are on the one `moqui-app` origin, so each account's app windows have a Chromium
//! profile of their own ([`profile_dir`]): its storage, caches and service workers are the browser's to keep apart, and
//! one account's pages see nothing another's kept (plans/Security-Review-Medium.md SEC-CLI-81). A removed account's
//! profile is deleted at the next start ([`forget_old_profiles`]), as Chromium holds its files while it runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Url, Webview, WebviewUrl, WebviewWindowBuilder};

use moqui_client_tauri::shell_load::{nonce_script, ShellLoad};
use moqui_client_tauri::{ClientState, ShellHost};
use serde_json::Value;

use super::{Client, APP_ORIGIN, SHELL_ORIGIN};

/// The shell's commands, as `allow-<command>` (build.rs), the sockets, the node page bridge's, the offline map
/// packages' (plans/archive/Maps-Offline.md §2.4), and the notification plugin's check its `window.Notification` makes.
const SHELL_PERMISSIONS: [&str; 46] = [
    "allow-client-state",
    "allow-client-sign-in",
    "allow-client-send-code",
    "allow-client-sign-out",
    "allow-client-switch-account",
    "allow-client-add-account",
    "allow-client-remove-account",
    "allow-client-open-remote",
    "allow-client-take-open",
    "allow-client-sso-providers",
    "allow-client-sso-start",
    "allow-client-sso-cancel",
    "allow-socket-open",
    "allow-socket-send",
    "allow-socket-close",
    "allow-outbox-add",
    "allow-outbox-list",
    "allow-outbox-kinds",
    "allow-outbox-retry",
    "allow-outbox-discard",
    "allow-outbox-unsent",
    "allow-draft-get",
    "allow-draft-list",
    "allow-draft-put",
    "allow-draft-remove",
    "allow-file-save",
    "allow-notify",
    "allow-notify-clear",
    "allow-secure-screen",
    "allow-capture-sources",
    "allow-capture-choose",
    "allow-capture-overlay",
    "allow-capture-ink",
    "allow-capture-overlay-close",
    "allow-push-check",
    "allow-push-test",
    "allow-set-counts",
    "allow-set-presence",
    "allow-open-external",
    "allow-packages-list",
    "allow-packages-download",
    "allow-packages-cancel",
    "allow-packages-delete",
    "allow-packages-status",
    "allow-packages-addresses",
    "notification:allow-is-permission-granted",
];

/// An app window's: the sockets of its page, the screen share picker of a call in it (capture.rs), the outbox and drafts of its account (which reach nothing its requests do
/// not), the vault module the app runs for its account (V5), and the notification plugin's check its
/// `window.Notification` makes.
const APP_PERMISSIONS: [&str; 22] = [
    "allow-socket-open",
    "allow-socket-send",
    "allow-socket-close",
    "allow-outbox-add",
    "allow-outbox-list",
    "allow-outbox-kinds",
    "allow-outbox-retry",
    "allow-outbox-discard",
    "allow-draft-get",
    "allow-draft-list",
    "allow-draft-put",
    "allow-draft-remove",
    "allow-file-save",
    "allow-open-in-shell",
    "allow-vault-call",
    "allow-vault-memory",
    "allow-capture-sources",
    "allow-capture-choose",
    "allow-capture-overlay",
    "allow-capture-ink",
    "allow-capture-overlay-close",
    "notification:allow-is-permission-granted",
];

/// The capabilities of the two origins, each on its own windows. Tauri counts a page of the app's own schemes as local,
/// so a capability names the windows: the main window shows only the shell, and an app window only the server's pages
/// (each window's navigation stays on its origin, `goes`).
pub(crate) fn add_capabilities(app: &AppHandle) {
    let shell = SHELL_PERMISSIONS.iter().fold(
        CapabilityBuilder::new("client-shell").local(true).window("main"),
        |capability, permission| capability.permission(*permission),
    );
    let pages = APP_PERMISSIONS.iter().fold(
        CapabilityBuilder::new("client-app").local(true).window("remote-*"),
        |capability, permission| capability.permission(*permission),
    );
    for (name, capability) in [("shell", shell), ("app windows", pages)] {
        if let Err(err) = app.add_capability(capability) {
            tracing::error!("The client's {name} have no commands: {err}");
        }
    }
}

/// The app's windows as the shared shell commands use them (`moqui_client_tauri::ShellHost`).
pub(crate) struct DesktopShell {
    pub(crate) app: AppHandle,
    /// The main window's load, and the nonce of the shell's ([`shell_window`]).
    pub(crate) load: Arc<ShellLoad>,
}

impl ShellHost for DesktopShell {
    fn show_shell(&self, url: &Url) {
        if let Some(window) = self.app.get_webview_window("main") {
            if let Err(err) = window.navigate(url.clone()) {
                tracing::error!("The shell did not load: {err}");
            }
            crate::show_main(&self.app);
        }
    }

    fn close_remotes(&self, account: Option<&str>) {
        close_remotes(&self.app, account);
    }

    fn open_remote(&self, path: &str) -> Result<(), String> {
        open_remote(&self.app, path)
    }

    fn open_external(&self, url: &Url) {
        crate::open_externally(url);
    }

    fn device_name(&self) -> String {
        super::device_name()
    }

    fn sso_redirect(&self) -> &'static str {
        moqui_client::sso::DESKTOP_REDIRECT
    }

    fn sso_starting(&self) {
        crate::os::claim_app_scheme();
    }

    /// The main window, at the shell's page, with the nonce of its current load: the capability grants the shell's
    /// commands to the main window only, and the nonce holds if another document is ever shown there (SEC-CLI-07).
    fn is_shell_page(&self, webview: &Webview, nonce: Option<&str>) -> bool {
        webview.label() == super::SHELL_WINDOW && self.load.is_shell(webview.url().ok().as_ref(), nonce)
    }
}

/// Tell the account's windows that its outbox changed: `event` to `window.__moquiOutboxEvent` in the shell (when it
/// shows the account) and the account's app windows.
pub(crate) fn notify_pages(app: &AppHandle, account: &str, event: &Value) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    let mut labels: Vec<String> = moqui_lib::sync::lock(&client.windows)
        .iter()
        .filter(|(_, owner)| owner == account)
        .map(|(label, _)| label.clone())
        .collect();
    let current = moqui_lib::sync::lock(client.core.accounts())
        .current()
        .is_some_and(|shown| shown.id == account);
    if current {
        labels.push(super::SHELL_WINDOW.to_string());
    }
    let script = format!("window.__moquiOutboxEvent && window.__moquiOutboxEvent({event})");
    for label in labels {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.eval(script.clone());
        }
    }
}

/// Where a window on `origin` sends a navigation to a URL.
#[derive(Debug, PartialEq, Eq)]
enum Goes {
    /// The window goes there: its own origin (a `blob:` of it too), or `about:blank`. A `data:` document, or a `blob:`
    /// of another origin, goes nowhere (plans/Security-Review.md SEC-CLI-03).
    Here,
    /// A page of the server, from the shell: in an app window.
    AppWindow,
    /// The web, or a `mailto:`, `tel:` or `sip:` link: the system browser or handler.
    Outside,
    /// Nowhere: the shell from an app window, another scheme.
    Nowhere,
}

fn goes(url: &Url, origin: &str) -> Goes {
    let at = url.origin().ascii_serialization();
    // the shell's window shows the shell's page only; another path on its origin is the server's (forwarded), whose
    // pages open in an app window
    if at == SHELL_ORIGIN && origin == SHELL_ORIGIN && !matches!(url.path(), "/" | "/index.html") {
        return Goes::AppWindow;
    }
    if at == origin || url.as_str() == "about:blank" {
        Goes::Here
    } else if at == APP_ORIGIN {
        Goes::AppWindow
    } else if at != SHELL_ORIGIN && matches!(url.scheme(), "http" | "https" | "mailto" | "tel" | "sip") {
        Goes::Outside
    } else {
        Goes::Nowhere
    }
}

/// Whether a window on `origin` may go to `url`; the server's page from the shell opens in an app window, and the web
/// in the system browser.
fn stays_on(url: &Url, origin: &str) -> bool {
    match goes(url, origin) {
        Goes::Here => true,
        Goes::AppWindow => {
            if let Some(app) = APP.get() {
                open_page(app, &on_app_origin(url));
            }
            false
        }
        Goes::Outside => {
            crate::open_externally(url);
            false
        }
        Goes::Nowhere => false,
    }
}

/// A server page's URL under the app windows' origin: one of the shell's origin (a path the shell forwards) is the same
/// path there.
fn on_app_origin(url: &Url) -> Url {
    if url.origin().ascii_serialization() != SHELL_ORIGIN {
        return url.clone();
    }
    let mut moved = url.clone();
    let _ = moved.set_host(Some("moqui-app.localhost"));
    moved
}

/// The comms app's screens the shell shows itself (bundled from basic-comms, `shell/bundled-screens.txt`), by their path
/// under the comms root, as `Shell/Root` routes them (`SHELL_ROUTES` in `shell/screens/Root.qvue`): a call's screen is
/// the shell's, which holds the crypto device of its encrypted calls, and so is My devices (`Settings/Devices`: its key
/// backup, verification and Set up this device); Calls' other screens (History, Voicemail, Recordings) and the other
/// Settings open in an app window.
const SHELL_SCREENS: [&str; 7] = [
    "Chat",
    "Inbox",
    "Inbox/Mail",
    "Inbox/Compose",
    "Calls/Call",
    "Calls/Dialer",
    "Settings/Devices",
];

/// Whether the server's `path` (`/qapps/...` or `/apps/...`, with its query, a `.qvt`, `.qvue` or `.qjs` ending as
/// Root.qvue takes it) is a comms screen the shell shows, under the account's comms app at `comms_root`
/// (`calls.appRoot`, `/qapps/BasicComms` or an app's own such as `/qapps/Comms`).
pub(crate) fn shown_in_shell(path: &str, comms_root: &str) -> bool {
    let under_apps = |text: &str| {
        text.strip_prefix("/qapps/")
            .or_else(|| text.strip_prefix("/apps/"))
            .map(str::to_string)
    };
    let bare = path.split_once('?').map_or(path, |(bare, _)| bare);
    let bare = [".qvt", ".qvue", ".qjs"]
        .iter()
        .find_map(|ext| bare.strip_suffix(ext))
        .unwrap_or(bare)
        .trim_end_matches('/');
    let (Some(screen), Some(root)) = (under_apps(bare), under_apps(comms_root.trim().trim_end_matches('/'))) else {
        return false;
    };
    screen
        .strip_prefix(root.as_str())
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|rest| SHELL_SCREENS.contains(&rest))
}

/// Where a server page opened from the OS, a notification or the shell goes.
#[derive(Debug, PartialEq, Eq)]
enum PageGoes {
    /// A comms screen the shell shows: kept for the shell (`client_take_open`); `poke` when an account is signed in, so
    /// a shell already showing it takes it now (else it does as it mounts, after the sign-in).
    Shell { poke: bool },
    /// Another page: an app window.
    AppWindow,
}

fn page_goes(path: &str, comms_root: &str, signed_in: bool) -> PageGoes {
    if shown_in_shell(path, comms_root) {
        PageGoes::Shell { poke: signed_in }
    } else {
        PageGoes::AppWindow
    }
}

/// What the host evaluates in the shell to have it take the screen kept for it (`shell/js/shell.js`).
const SHELL_POKE: &str = "window.moquiShell && window.moquiShell.poke()";

/// A server page opened from the OS (a `mailto:` routed to Compose, a `tel:` or `sip:` to the Dialer), a notification (a
/// call ringing: its screen) or a link in the shell, `url` under the app origin: a comms screen the shell shows is kept
/// for the shell and shown there (the main window, shown), also when it comes while the shell signs in or loads; another
/// opens in an app window ([`super::open_link`]).
pub(crate) fn open_page(app: &AppHandle, url: &Url) {
    if url.origin().ascii_serialization() != APP_ORIGIN {
        return;
    }
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    let current = client.core.current();
    let signed_in = current.as_ref().is_some_and(|(_, key)| key.is_some());
    let root = current.map_or_else(
        || moqui_client::offline::DEFAULT_COMMS_ROOT.to_string(),
        |(account, _)| client.core.comms_root_of(&account.id),
    );
    match page_goes(&path, &root, signed_in) {
        PageGoes::Shell { poke } => keep_for_shell(app, path, poke),
        PageGoes::AppWindow => super::open_link(app, url),
    }
}

/// Keep `path` for the shell to take (`client_take_open`), poke a shell that shows a signed-in account to take it now, and
/// show the main window.
fn keep_for_shell(app: &AppHandle, path: String, poke: bool) {
    if let Some(state) = app.try_state::<ClientState>() {
        *moqui_lib::sync::lock(&state.open_pending) = Some(path);
    }
    if poke {
        if let Some(window) = app.get_webview_window("main") {
            if let Err(err) = window.eval(SHELL_POKE) {
                tracing::warn!("The shell was not told of a link: {err}");
            }
        }
    }
    crate::show_main(app);
}

/// Whether an app window of `window_account` may have the shell show `path`: a comms screen the shell shows, under that
/// account's comms app at `comms_root`, while that account is the one the shell shows (`current`).
fn shell_path_for(
    path: &str,
    comms_root: &str,
    window_account: Option<&str>,
    current: Option<&str>,
) -> Result<(), String> {
    let Some(account) = window_account else {
        return Err("Only an app window shows a screen in the main window".to_string());
    };
    if current != Some(account) {
        return Err("This window's account is not the one the main window shows: switch to it there first".to_string());
    }
    if !shown_in_shell(path, comms_root) {
        return Err(format!("The main window does not show {path}"));
    }
    Ok(())
}

/// An app window's "Open in Chat" or "Open in the app's Calls" (an encrypted room or call, whose crypto device is the
/// shell's): `path`, a comms screen the shell shows under the window's account's comms app, shown in the main window, for
/// that account only while it is current.
#[tauri::command(async)]
pub(crate) fn open_in_shell(app: AppHandle, webview: tauri::Webview, path: String) -> Result<(), String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let label = webview.label().to_string();
    let window_account = moqui_lib::sync::lock(&client.windows)
        .iter()
        .find(|(window, _)| *window == label)
        .map(|(_, account)| account.clone());
    let current = client.core.current();
    let signed_in = current.as_ref().is_some_and(|(_, key)| key.is_some());
    let current_id = current.map(|(account, _)| account.id);
    let root = window_account.as_deref().map_or_else(
        || moqui_client::offline::DEFAULT_COMMS_ROOT.to_string(),
        |id| client.core.comms_root_of(id),
    );
    shell_path_for(&path, &root, window_account.as_deref(), current_id.as_deref())?;
    keep_for_shell(&app, path, signed_in);
    Ok(())
}

/// The script an app window starts with: its account's storage scope, the scopes it is to clear once, that the shell owns
/// the chat crypto device (WebrootCrypto makes or opens none here), then the bridge.
fn app_window_script(scope: &str, cleared: &[String]) -> Result<String, String> {
    let scope = serde_json::to_string(scope).map_err(|err| err.to_string())?;
    let cleared = serde_json::to_string(cleared).map_err(|err| err.to_string())?;
    Ok(format!(
        "window.__moquiStorageScope = {scope};\nwindow.__moquiClearScopes = {cleared};\nwindow.__moquiCryptoInShell = true;\n{}",
        crate::bridge::SCRIPT
    ))
}

/// The script the shell starts with: its crypto device's name ("MR Moqui desktop on <OS>"), then the bridge.
fn shell_script(device_name: &str) -> String {
    let name = serde_json::to_string(device_name).unwrap_or_else(|_| "\"\"".to_string());
    format!("window.__moquiDeviceName = {name};\n{}", crate::bridge::SCRIPT)
}

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// The main window, on the shell.
pub(crate) fn shell_window(app: &AppHandle) -> tauri::Result<()> {
    let _ = APP.set(app.clone());
    // the shell's commands carry the nonce of the shell page's current load, handed to it as the load finishes
    // (SEC-CLI-07); the next load's start takes it away (not a navigation's start: one that ends as a download loads
    // nothing, and the next document never sees the last one's nonce)
    let load = app
        .try_state::<Client>()
        .map(|client| Arc::clone(&client.shell_load))
        .unwrap_or_default();
    WebviewWindowBuilder::new(
        app,
        super::SHELL_WINDOW,
        WebviewUrl::External(moqui_client_tauri::shell_url(None, None)),
    )
    .title("MR Moqui")
    .inner_size(1280.0, 860.0)
    .min_inner_size(400.0, 300.0)
    .initialization_script(shell_script(&super::device_name()))
    .on_download(crate::bridge::on_download)
    .on_permission_request(crate::bridge::on_permission)
    .on_page_load(move |window, payload| match payload.event() {
        PageLoadEvent::Started => {
            load.started(payload.url());
            if let Some(client) = window.app_handle().try_state::<Client>() {
                client.core.sockets().close_for(window.label());
            }
        }
        PageLoadEvent::Finished => {
            if let Some(nonce) = load.finished(payload.url()) {
                if let Err(err) = window.eval(nonce_script(&nonce)) {
                    tracing::warn!("The shell's page was not handed its load's nonce: {err}");
                }
            }
        }
    })
    .on_navigation(|url| stays_on(url, SHELL_ORIGIN))
    .on_new_window(|url, _features| {
        // a page's window.open: the server's page in an app window, the web in the system browser
        let _ = stays_on(&url, SHELL_ORIGIN);
        NewWindowResponse::Deny
    })
    .build()?;
    Ok(())
}

/// The folder, under Chromium's (`<cache>/<identifier>/cef`), of the profile of `account`'s app windows: relative, so
/// the runtime keeps it under its own folder (tauri-runtime-cef's `data_directory`). An account id is a host name's
/// letters, digits, dots and dashes and a hash (`moqui_client::account::account_id`), one plain folder name.
pub(crate) fn profile_dir(account: &str) -> PathBuf {
    Path::new(PROFILES).join(account)
}

/// The folder under Chromium's that holds the accounts' profiles ([`profile_dir`]).
const PROFILES: &str = "accounts";

/// The profiles under `root` (Chromium's folder, [`PROFILES`] in it) of accounts no longer in `kept`.
pub(crate) fn old_profiles(root: &Path, kept: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root.join(PROFILES)) else {
        return Vec::new();
    };
    let mut old: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !kept.iter().any(|id| entry.file_name().to_str() == Some(id.as_str())))
        .map(|entry| entry.path())
        .collect();
    old.sort();
    old
}

/// Delete the profiles of accounts removed since the last run (Chromium's folder is `<cache>/<identifier>/cef`,
/// tauri-runtime-cef's default, which this app does not change); one that will not go is logged and the rest go on.
pub(crate) fn forget_old_profiles(app: &AppHandle, kept: &[String]) {
    let root = match app.path().app_cache_dir() {
        Ok(cache) => cache.join("cef"),
        Err(err) => {
            tracing::warn!("The accounts' browser profiles were not checked: {err}");
            return;
        }
    };
    for old in old_profiles(&root, kept) {
        match std::fs::remove_dir_all(&old) {
            Ok(()) => tracing::info!(
                "The browser profile of a removed account was deleted: {}",
                old.display()
            ),
            Err(err) => tracing::error!("The browser profile {} was not deleted: {err}", old.display()),
        }
    }
}

/// Open `path` of the current account's server (`/qapps/...`) in a new app window, in the account's own browser
/// profile ([`profile_dir`]).
///
/// # Errors
///
/// Returns an error for a path that is not one on the server, with no account signed in, or when the window does not
/// open.
pub(crate) fn open_remote(app: &AppHandle, path: &str) -> Result<(), String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let (account, key) = client.core.current().ok_or("No account is signed in")?;
    if key.is_none() {
        return Err("The account is signed out".to_string());
    }
    let trimmed = path.trim();
    if !trimmed.starts_with('/') || trimmed.starts_with("//") || trimmed.chars().any(|c| c.is_control() || c == '\\') {
        return Err(format!("Not a path on the server: {trimmed}"));
    }
    let url = Url::parse(&format!("{APP_ORIGIN}{trimmed}")).map_err(|err| err.to_string())?;
    let label = format!("remote-{}", client.next_window.fetch_add(1, Ordering::Relaxed));
    // the account's storage scope for the server's own scripts (MoquiLib scopedKey), and the storage of accounts deleted
    // since an app window last opened, which this one clears once (bridge.js)
    let cleared = app
        .try_state::<ClientState>()
        .map(|state| std::mem::take(&mut moqui_lib::sync::lock(&state.cleared).app))
        .unwrap_or_default();
    let script = app_window_script(&account.id, &cleared)?;
    let title = if account.server_name.is_empty() {
        account.server.clone()
    } else {
        account.server_name.clone()
    };
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title(format!("{title} — MR Moqui"))
        .inner_size(1280.0, 860.0)
        .min_inner_size(400.0, 300.0)
        .initialization_script(script)
        .data_directory(profile_dir(&account.id))
        .on_download(crate::bridge::on_download)
        .on_permission_request(crate::bridge::on_permission)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Started {
                if let Some(client) = window.app_handle().try_state::<Client>() {
                    client.core.sockets().close_for(window.label());
                }
            }
        })
        .on_navigation(|url| stays_on(url, APP_ORIGIN))
        .on_new_window(|url, _features| {
            // a server page's new window (target=_blank, window.open) on the server: another app window
            if goes(&url, APP_ORIGIN) == Goes::Here && url.origin().ascii_serialization() == APP_ORIGIN {
                if let Some(app) = APP.get() {
                    super::open_link(app, &url);
                }
            } else {
                let _ = stays_on(&url, APP_ORIGIN);
            }
            NewWindowResponse::Deny
        });
    // the window's account before its first request, which can come before build returns
    moqui_lib::sync::lock(&client.windows).push((label.clone(), account.id));
    if let Err(err) = builder.build() {
        moqui_lib::sync::lock(&client.windows).retain(|(window, _)| window != &label);
        return Err(format!("The window did not open: {err}"));
    }
    Ok(())
}

/// Close the app windows of an account, or of every account.
pub(crate) fn close_remotes(app: &AppHandle, account: Option<&str>) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    let closing: Vec<String> = {
        let mut windows = moqui_lib::sync::lock(&client.windows);
        let (closing, kept): (Vec<_>, Vec<_>) = windows
            .drain(..)
            .partition(|(_, owner)| account.is_none_or(|account| owner == account));
        *windows = kept;
        closing.into_iter().map(|(label, _)| label).collect()
    };
    // the vault of an account whose pages close locks with them
    match account {
        Some(account) => client.core.vault().lock(account),
        None => client.core.vault().lock_all(),
    }
    for label in closing {
        client.core.sockets().close_for(&label);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
    }
}

/// An app window closed: its sockets go, and it is no longer the account's. Its vault session locks, as a browser's
/// worker goes with its tab (plans/archive/Vault-Instances.md §3.6).
pub(crate) fn closed(app: &AppHandle, label: &str) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    client.core.sockets().close_for(label);
    moqui_lib::sync::lock(&client.windows).retain(|(window, _)| window != label);
    client.core.vault().lock_label(label);
    if let Some(state) = app.try_state::<moqui_client_tauri::ClientState>() {
        moqui_lib::sync::lock(&state.vault_pages).remove(label);
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
