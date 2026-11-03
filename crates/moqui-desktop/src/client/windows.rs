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
//! opened from it, on the `moqui-app` origin with the sockets only. Each stays on its origin: a link elsewhere on the web
//! opens in the system browser. An app window is one account's, named in its storage scope, and closes when the
//! account changes or signs out.

use std::sync::atomic::Ordering;

use tauri::ipc::CapabilityBuilder;
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};

use super::{Client, APP_ORIGIN, SHELL_ORIGIN};

/// The shell's commands, as `allow-<command>` (build.rs), the sockets, the node page bridge's, and the notification
/// plugin's check its `window.Notification` makes.
const SHELL_PERMISSIONS: [&str; 19] = [
    "allow-client-state",
    "allow-client-sign-in",
    "allow-client-send-code",
    "allow-client-sign-out",
    "allow-client-switch-account",
    "allow-client-add-account",
    "allow-client-remove-account",
    "allow-client-open-remote",
    "allow-client-sso-providers",
    "allow-client-sso-start",
    "allow-client-sso-cancel",
    "allow-socket-open",
    "allow-socket-send",
    "allow-socket-close",
    "allow-notify",
    "allow-set-counts",
    "allow-set-presence",
    "allow-open-external",
    "notification:allow-is-permission-granted",
];

/// An app window's: the sockets of its page, and the notification plugin's check its `window.Notification` makes.
const APP_PERMISSIONS: [&str; 4] = [
    "allow-socket-open",
    "allow-socket-send",
    "allow-socket-close",
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

/// The shell's page, with `query` (`signIn=1` for the sign-in).
fn shell_url(query: Option<&str>) -> Url {
    let text = match query {
        Some(query) => format!("{SHELL_ORIGIN}/?{query}"),
        None => format!("{SHELL_ORIGIN}/"),
    };
    Url::parse(&text).unwrap_or_else(|_| unreachable!("the shell's url parses"))
}

/// Where a window on `origin` sends a navigation to a URL.
#[derive(Debug, PartialEq, Eq)]
enum Goes {
    /// The window goes there: its own origin, or a page's own `about:`, `data:` or `blob:` document.
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
    if at == origin || matches!(url.scheme(), "about" | "data" | "blob") {
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
                super::open_link(app, &on_app_origin(url));
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

static APP: std::sync::OnceLock<AppHandle> = std::sync::OnceLock::new();

/// The main window, on the shell.
pub(crate) fn shell_window(app: &AppHandle) -> tauri::Result<()> {
    let _ = APP.set(app.clone());
    WebviewWindowBuilder::new(app, "main", WebviewUrl::External(shell_url(None)))
        .title("MR Moqui")
        .inner_size(1280.0, 860.0)
        .min_inner_size(400.0, 300.0)
        .initialization_script(crate::bridge::SCRIPT)
        .on_download(crate::bridge::on_download)
        .on_permission_request(crate::bridge::on_permission)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Started {
                if let Some(client) = window.app_handle().try_state::<Client>() {
                    client.sockets.close_for(window.label());
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

/// Load the shell again (another account, signed out), with `query`.
pub(crate) fn load_shell(app: &AppHandle, query: Option<&str>) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.navigate(shell_url(query)) {
            tracing::error!("The shell did not load: {err}");
        }
        crate::show_main(app);
    }
}

/// Open `path` of the current account's server (`/qapps/...`) in a new app window.
///
/// # Errors
///
/// Returns an error for a path that is not one on the server, with no account signed in, or when the window does not
/// open.
pub(crate) fn open_remote(app: &AppHandle, path: &str) -> Result<(), String> {
    let client = app.try_state::<Client>().ok_or("Not a client of a server")?;
    let (account, key) = client.current().ok_or("No account is signed in")?;
    if key.is_none() {
        return Err("The account is signed out".to_string());
    }
    let trimmed = path.trim();
    if !trimmed.starts_with('/') || trimmed.starts_with("//") || trimmed.chars().any(|c| c.is_control() || c == '\\') {
        return Err(format!("Not a path on the server: {trimmed}"));
    }
    let url = Url::parse(&format!("{APP_ORIGIN}{trimmed}")).map_err(|err| err.to_string())?;
    let label = format!("remote-{}", client.next_window.fetch_add(1, Ordering::Relaxed));
    // the account's storage scope for the server's own scripts (MoquiLib scopedKey), then the bridge
    let scope = serde_json::to_string(&account.id).map_err(|err| err.to_string())?;
    let script = format!("window.__moquiStorageScope = {scope};\n{}", crate::bridge::SCRIPT);
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
        .on_download(crate::bridge::on_download)
        .on_permission_request(crate::bridge::on_permission)
        .on_page_load(|window, payload| {
            if payload.event() == PageLoadEvent::Started {
                if let Some(client) = window.app_handle().try_state::<Client>() {
                    client.sockets.close_for(window.label());
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
    for label in closing {
        client.sockets.close_for(&label);
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.destroy();
        }
    }
}

/// An app window closed: its sockets go, and it is no longer the account's.
pub(crate) fn closed(app: &AppHandle, label: &str) {
    let Some(client) = app.try_state::<Client>() else {
        return;
    };
    client.sockets.close_for(label);
    moqui_lib::sync::lock(&client.windows).retain(|(window, _)| window != label);
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
