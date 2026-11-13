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

//! The app as a client of a remote server, with no node here (plans/Mobile-Desktop.md §4 configuration 3, MD3; the
//! `client` mode). The window shows the bundled shell (`shell/`), which opens with no network; each account is a server
//! and a user with a device key the host keeps in the OS key store, and the pages never hold it.
//!
//! What does not need Tauri is in `moqui_client`: the client's state ([`moqui_client::app::Client`]), each account's
//! store, the offline answers, the pushes, the outbox and its sender, the mirror, the key renewal, the device modes,
//! the shell's files and the pages' sockets. What needs Tauri but no CEF and no window layout is shared with the mobile
//! app in `moqui_client_tauri`: the shell's commands (sign-in, sign-out and the accounts), the outbox and drafts
//! commands, and the schemes' answer, over its `ClientState`, which this module manages beside its own [`Client`] (the
//! app windows). This module is the Tauri host ([`DesktopHost`], a [`moqui_client::app::ClientHost`]) and what only
//! the desktop app has:
//!
//! - [`scheme`]: two app schemes. `moqui-shell` (`http://moqui-shell.localhost`) serves the shell's files and forwards
//!   the rest to the current account's server; `moqui-app` (`http://moqui-app.localhost`) forwards everything, so an
//!   app window shows the server's own `/qapps` and scripts. Both add the key (`moqui_client::proxy`); a download is
//!   saved by the host.
//! - [`sockets`]: the commands of the sockets a page cannot open through a scheme (`/notws`, `/callws`), which the host
//!   holds and bridges (`moqui_client::bridge`), and the node in process's.
//! - [`windows`]: the shell and the app windows, and what the shell's commands do with them (`windows::DesktopShell`).
//! - [`keystore`]: the OS key store, else a file only the user can read.
//! - [`conf`]: the client's settings (`<client-conf>`), read from the runtime's conf.
//!
//! The shell's origin may call the shell commands, the sockets, the outbox and the drafts; an app window's origin only
//! the sockets, the outbox and the drafts of its own account, which reach nothing its requests do not already reach
//! with the key (plans/Mobile-Desktop.md "Remote applications and
//! authentication").

mod conf;
mod keystore;
mod scheme;
pub(crate) mod sockets;
pub(crate) mod windows;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use moqui_client::account::Account;
use moqui_client::app::ClientHost;
use moqui_client::assets::Assets;
use moqui_client::conf::ClientConf;
use moqui_client_tauri::{ClientState, ShellHost};
use serde_json::Value;
use tauri::{AppHandle, Manager, Url};

/// The label of the main window, which shows the shell (and the current account).
pub(crate) const SHELL_WINDOW: &str = "main";

/// The scheme of the shell, and its origin on Chromium (`http://<scheme>.localhost`).
pub(crate) const SHELL_SCHEME: &str = moqui_client_tauri::SHELL_SCHEME;
pub(crate) const SHELL_ORIGIN: &str = moqui_client_tauri::SHELL_ORIGIN;
/// The scheme of the app windows, the server's pages, and its origin.
pub(crate) const APP_SCHEME: &str = moqui_client_tauri::APP_SCHEME;
pub(crate) const APP_ORIGIN: &str = moqui_client_tauri::APP_ORIGIN;

/// The app windows of the client, on the app while it runs as a client, beside the shared `ClientState` (whose core and
/// host it holds too).
pub(crate) struct Client {
    pub(crate) core: Arc<moqui_client::app::Client>,
    /// This app as the core's host.
    pub(crate) host: Arc<dyn ClientHost>,
    /// The app windows (`remote-<n>`) and the account each shows.
    windows: Mutex<Vec<(String, String)>>,
    next_window: AtomicU64,
}

impl Client {
    /// The client of `dir`, and the shared state over it.
    fn new(
        app: &AppHandle,
        dir: &Path,
        resources: &Path,
        runtime: Option<PathBuf>,
    ) -> Result<(Self, ClientState), String> {
        let read_conf = Box::new(move || match &runtime {
            Some(runtime) => conf::read(runtime),
            None => ClientConf::default(),
        });
        let keys = keystore::open_keys(&dir.join("keys"));
        let core = Arc::new(moqui_client::app::Client::new(dir, keys, read_conf)?);
        let host: Arc<dyn ClientHost> = Arc::new(DesktopHost { app: app.clone() });
        let shell: Arc<dyn ShellHost> = Arc::new(windows::DesktopShell { app: app.clone() });
        let files = Box::new(shell_assets(tauri::is_dev(), resources));
        let state = ClientState::new(Arc::clone(&core), Arc::clone(&host), shell, files);
        let client = Self {
            core,
            host,
            windows: Mutex::new(Vec::new()),
            next_window: AtomicU64::new(1),
        };
        Ok((client, state))
    }

    /// The id of the account a webview shows, from the windows and the accounts alone: the shell's is the current one,
    /// an app window's its own.
    fn page_account(&self, label: &str) -> Option<String> {
        if label == SHELL_WINDOW {
            return moqui_lib::sync::lock(self.core.accounts())
                .current()
                .map(|account| account.id.clone());
        }
        moqui_lib::sync::lock(&self.windows)
            .iter()
            .find(|(window, _)| window == label)
            .map(|(_, account)| account.clone())
    }
}

/// The shell's files: the repository's in a development run, else the bundle's (`shell-assets/` in its resources).
fn shell_assets(dev: bool, resources: &Path) -> Assets {
    if dev {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        Assets::new(
            crate_dir.join("shell"),
            crate_dir
                .join("../..")
                .join(moqui_core::context::layout::REPO_WEBROOT_FILES),
        )
    } else {
        Assets::staged(resources)
    }
}

/// The app as the client core's host: its webviews are the pages.
struct DesktopHost {
    app: AppHandle,
}

impl ClientHost for DesktopHost {
    fn page_account(&self, page: &str) -> Option<String> {
        self.app.try_state::<Client>()?.page_account(page)
    }

    fn is_shell(&self, page: &str) -> bool {
        page == SHELL_WINDOW
    }

    fn notify_pages(&self, account: &str, event: &Value) {
        windows::notify_pages(&self.app, account, event);
    }

    fn socket_event(&self, page: &str, id: u64, event: &Value) {
        if let Some(window) = self.app.get_webview_window(page) {
            let _ = window.eval(format!(
                "window.__moquiSocketEvent && window.__moquiSocketEvent({id}, {event})"
            ));
        }
    }

    fn check_key(&self, account: &Account) {
        moqui_client_tauri::scheme::check_key(&self.app, account);
    }
}

/// The schemes on the app, for every mode: in a node's mode no client is managed, and they answer 404.
pub(crate) fn register(builder: tauri::Builder) -> tauri::Builder {
    builder
        .register_asynchronous_uri_scheme_protocol(SHELL_SCHEME, scheme::shell)
        .register_asynchronous_uri_scheme_protocol(APP_SCHEME, scheme::app)
}

/// Start the client: its state in the app's config directory (`client/`), the capabilities of its origins, and the
/// window on the shell.
///
/// # Errors
///
/// Returns an error when the directories are not known or the window does not open.
pub(crate) fn start(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let dir = app.path().app_config_dir()?.join("client");
    let resources = app.path().resource_dir()?;
    let (client, state) = Client::new(app, &dir, &resources, conf::runtime_dir(app))?;
    let (core, host) = (Arc::clone(&client.core), Arc::clone(&client.host));
    app.manage(client);
    app.manage(state);
    core.warm();
    windows::add_capabilities(app);
    windows::shell_window(app)?;
    moqui_client::renewal::start(&core, &host);
    moqui_client::sync::flush::start(&core, &host);
    Ok(())
}

/// Whether the app runs as a client.
pub(crate) fn active(app: &AppHandle) -> bool {
    app.try_state::<Client>().is_some()
}

/// A link of a notification, of the OS (a `mailto:` routed to Compose) or of the shell on the server, `url` under the
/// app origin: a comms screen the shell bundles opens in the shell, any other in an app window of the current account.
pub(crate) fn open_page(app: &AppHandle, url: &Url) {
    windows::open_page(app, url);
}

/// A link on the server, `url` under the app origin: it opens in an app window of the current account.
pub(crate) fn open_link(app: &AppHandle, url: &Url) {
    if url.origin().ascii_serialization() != APP_ORIGIN {
        return;
    }
    let path = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    };
    if let Err(err) = windows::open_remote(app, &path) {
        tracing::warn!("Could not open {path}: {err}");
    }
}

/// The server's hand-back of an SSO sign-in (the `org.moqui.desktop:` link), for the sign-in this app started.
pub(crate) fn sso_returned(app: &AppHandle, link: &Url) {
    moqui_client_tauri::commands::sso_returned(app, link);
}

/// An app window closed: its sockets go with it.
pub(crate) fn window_closed(app: &AppHandle, label: &str) {
    windows::closed(app, label);
}

/// The origin of the app windows as a URL, where a notification's link and a routed link are put.
pub(crate) fn app_origin() -> Url {
    Url::parse(APP_ORIGIN).unwrap_or_else(|_| unreachable!("a fixed url parses"))
}

/// The shell's origin as a URL, where its downloads are.
pub(crate) fn shell_origin() -> Url {
    Url::parse(SHELL_ORIGIN).unwrap_or_else(|_| unreachable!("a fixed url parses"))
}

/// The comms app of the current account's server (its client config's `calls.appRoot`), for `mailto:`, `tel:` and
/// `sip:` links. On the main thread: only what is already noted in memory (noted in the background as the client
/// starts, and when a client config is answered), never the store or the conf.
pub(crate) fn comms_root(app: &AppHandle) -> Option<String> {
    let client = app.try_state::<Client>()?;
    let id = moqui_lib::sync::lock(client.core.accounts()).current()?.id.clone();
    client.core.noted_comms_root(&id)
}

/// A name for this computer in the user's device list.
fn device_name() -> String {
    let host = std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this computer".to_string());
    let host = host.strip_suffix(".local").unwrap_or(&host).to_string();
    format!("MR Moqui desktop on {host}")
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
