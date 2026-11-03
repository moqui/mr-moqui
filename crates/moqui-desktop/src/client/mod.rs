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
//! - [`scheme`]: two app schemes. `moqui-shell` (`http://moqui-shell.localhost`) serves the shell's files and forwards
//!   the rest to the current account's server; `moqui-app` (`http://moqui-app.localhost`) forwards everything, so an
//!   app window shows the server's own `/qapps` and scripts. Both add the key (`moqui_client::proxy`).
//! - [`sockets`]: `/notws` and `/callws`, which a page cannot open through a scheme, held by the host and bridged.
//! - [`commands`]: sign-in, sign-out and the accounts, for the shell; [`windows`]: the shell and the app windows.
//! - [`keystore`]: the OS key store, else a file only the user can read.
//!
//! The shell's origin may call the shell commands and the sockets; an app window's origin only the sockets, which
//! reach nothing its requests do not already reach with the key (plans/Mobile-Desktop.md "Remote applications and
//! authentication").

mod assets;
pub(crate) mod commands;
mod keystore;
mod scheme;
pub(crate) mod sockets;
mod windows;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

use moqui_client::account::{Account, Accounts};
use moqui_client::cache::Cache;
use moqui_client::proxy::Proxy;
use tauri::{AppHandle, Manager, Url};

/// The scheme of the shell, and its origin on Chromium (`http://<scheme>.localhost`).
pub(crate) const SHELL_SCHEME: &str = "moqui-shell";
pub(crate) const SHELL_ORIGIN: &str = "http://moqui-shell.localhost";
/// The scheme of the app windows, the server's pages, and its origin.
pub(crate) const APP_SCHEME: &str = "moqui-app";
pub(crate) const APP_ORIGIN: &str = "http://moqui-app.localhost";

/// The client's state, on the app while it runs as a client.
pub(crate) struct Client {
    accounts: Mutex<Accounts>,
    keys: keystore::Keys,
    cache: Cache,
    proxy: Proxy,
    assets: assets::Assets,
    sockets: sockets::Sockets,
    /// The app windows (`remote-<n>`) and the account each shows.
    windows: Mutex<Vec<(String, String)>>,
    next_window: AtomicU64,
    /// A check that the current account's key still signs in is under way.
    checking: AtomicBool,
    /// An SSO sign-in waiting for the browser to come back.
    sso: Mutex<Option<moqui_client::sso::Pending>>,
}

impl Client {
    fn new(dir: &Path, resources: &Path) -> Result<Self, String> {
        Ok(Self {
            accounts: Mutex::new(Accounts::load(dir)),
            keys: keystore::Keys::open(&dir.join("keys")),
            cache: Cache::new(&dir.join("cache")),
            proxy: Proxy::new().map_err(|err| err.to_string())?,
            assets: assets::Assets::choose(tauri::is_dev(), resources),
            sockets: sockets::Sockets::default(),
            windows: Mutex::new(Vec::new()),
            next_window: AtomicU64::new(1),
            checking: AtomicBool::new(false),
            sso: Mutex::new(None),
        })
    }

    /// The current account and its key, if it has one.
    fn current(&self) -> Option<(Account, Option<String>)> {
        let account = moqui_lib::sync::lock(&self.accounts).current().cloned()?;
        let key = self.keys.get(&account.id);
        Some((account, key))
    }

    /// The account and key of an account id.
    fn account(&self, id: &str) -> Option<(Account, Option<String>)> {
        let account = moqui_lib::sync::lock(&self.accounts).get(id).cloned()?;
        let key = self.keys.get(&account.id);
        Some((account, key))
    }

    /// The account a webview shows: the shell's is the current one, an app window's its own.
    fn account_of(&self, label: &str) -> Option<(Account, Option<String>)> {
        if label == "main" {
            return self.current();
        }
        let id = moqui_lib::sync::lock(&self.windows)
            .iter()
            .find(|(window, _)| window == label)
            .map(|(_, account)| account.clone())?;
        self.account(&id)
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
    app.manage(Client::new(&dir, &resources)?);
    windows::add_capabilities(app);
    windows::shell_window(app)?;
    Ok(())
}

/// Whether the app runs as a client.
pub(crate) fn active(app: &AppHandle) -> bool {
    app.try_state::<Client>().is_some()
}

/// A link of a notification or of the OS (a `mailto:` routed to Compose) on the server, `url` under the app origin:
/// it opens in an app window of the current account.
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
    commands::sso_returned(app, link);
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

/// The comms app of the current account's server (its cached client config's `calls.appRoot`), for `mailto:`, `tel:`
/// and `sip:` links.
pub(crate) fn comms_root(app: &AppHandle) -> Option<String> {
    let client = app.try_state::<Client>()?;
    let id = moqui_lib::sync::lock(&client.accounts).current()?.id.clone();
    let config = client.cache.get(&id, "/qapps/clientConfig")?;
    let config: serde_json::Value = serde_json::from_slice(&config).ok()?;
    config["calls"]["appRoot"].as_str().map(str::to_string)
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
