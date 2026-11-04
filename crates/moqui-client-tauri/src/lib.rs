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

//! The Tauri glue of the client of a remote server that the desktop app (on CEF) and the mobile app (on wry) share
//! (plans/Mobile-Desktop.md §4 configuration 3, MD3, MD4 and MD6): what needs Tauri but no webview runtime and no
//! window layout. Tauri 3 runs its apps on a type-erased runtime (`tauri::DynRuntime`), so these commands and handlers
//! are the same for both apps.
//!
//! - [`ClientState`], managed on the app: the client's core (`moqui_client::app::Client`), the app as its
//!   [`ClientHost`], what the app does with its pages ([`ShellHost`]), the shell's files ([`ShellFiles`]), and what
//!   the shell's commands keep between calls (an SSO sign-in waiting, a link kept for the shell, a notice).
//! - [`scheme`]: the two app schemes' answer: the shell's files, and the rest forwarded to the account's server with
//!   its key, kept in the store and answered from it offline (`moqui_client::offline`).
//! - [`commands`]: the shell's commands (the accounts, sign-in and SSO, sign-out), [`outbox`]: the outbox and drafts,
//!   [`sockets`]: the sockets of a page whose sockets are all the server's (the mobile app; the desktop app has its
//!   own, with the node's in process).
//!
//! The commands are `pub` at the crate root, where `tauri::command` puts the macros `generate_handler!` looks for, so an
//! app lists them as `moqui_client_tauri::client_state`.

pub mod commands;
pub mod outbox;
pub mod scheme;
pub mod sockets;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use moqui_client::account::Account;
use moqui_client::app::{Client, ClientHost};
use moqui_client::assets::{Asset, Assets};
use moqui_client::conf::DeviceMode;
use tauri::{AppHandle, Manager, Url, Webview};

pub use commands::{
    client_add_account, client_open_remote, client_remove_account, client_send_code, client_sign_in, client_sign_out,
    client_sso_cancel, client_sso_providers, client_sso_start, client_state, client_switch_account, client_take_open,
};
pub use outbox::{
    draft_get, draft_list, draft_put, draft_remove, file_save, outbox_add, outbox_discard, outbox_list, outbox_retry,
    outbox_unsent,
};
pub use sockets::{socket_close, socket_open, socket_send};

/// The scheme of the shell, and its origin (`http://<scheme>.localhost` on CEF and on Android's WebView).
pub const SHELL_SCHEME: &str = "moqui-shell";
pub const SHELL_ORIGIN: &str = moqui_client::SHELL_ORIGIN;
/// The scheme of the server's pages (the desktop's app windows, the mobile app's one webview on an app), and its origin.
pub const APP_SCHEME: &str = "moqui-app";
pub const APP_ORIGIN: &str = "http://moqui-app.localhost";

/// What the app does with its pages for the shell's commands: the desktop app has a main window and app windows, the
/// mobile app one webview that shows the shell or a server's app.
pub trait ShellHost: Send + Sync {
    /// Show the shell's page at `url` (on [`SHELL_ORIGIN`]), and the window or activity it is in.
    fn show_shell(&self, url: &Url);

    /// Close the server's pages of `account`, or of every account, with their sockets.
    fn close_remotes(&self, account: Option<&str>);

    /// Open `path` (`/qapps/...`, checked) of the current account's server as a page of the server.
    ///
    /// # Errors
    ///
    /// Returns why it did not open.
    fn open_remote(&self, path: &str) -> Result<(), String>;

    /// Open `url` in the system browser.
    fn open_external(&self, url: &Url);

    /// A name for this device in the user's device list.
    fn device_name(&self) -> String;

    /// The redirect URI of an SSO hand-back to this app (`moqui_client::sso::DESKTOP_REDIRECT` or `MOBILE_REDIRECT`).
    fn sso_redirect(&self) -> &'static str;

    /// An SSO sign-in starts: the app makes sure the OS hands its redirect back to it.
    fn sso_starting(&self) {}

    /// Whether `webview` shows the shell now and may call the shell's commands. The desktop's capabilities grant them to
    /// the main window only, which shows only the shell; the mobile app's one webview shows a server's app too, so it
    /// checks the page (plans/Mobile-Desktop.md "Remote applications and authentication").
    fn is_shell_page(&self, webview: &Webview) -> bool {
        let _ = webview;
        true
    }

    /// Whether the app can make requests: the mobile app's TLS verifier must be set up first, and a request without it
    /// would panic; the app's pages then answer from the store, and the commands that need the server refuse.
    fn network_ready(&self) -> bool {
        true
    }
}

/// The shell's own files, as the scheme serves them: from disk ([`Assets`], the desktop) or from the app's embedded
/// assets (the mobile app).
pub trait ShellFiles: Send + Sync {
    /// Whether `path` is one of the shell's own (answered from its files or not found, never forwarded).
    fn is_static(&self, path: &str) -> bool;

    /// The file of `path`, or none.
    fn read(&self, path: &str) -> Option<Asset>;
}

impl ShellFiles for Assets {
    fn is_static(&self, path: &str) -> bool {
        Assets::is_static(self, path)
    }

    fn read(&self, path: &str) -> Option<Asset> {
        Assets::read(self, path)
    }
}

/// The client's state on the app, managed while it runs as a client of a server.
pub struct ClientState {
    pub core: Arc<Client>,
    /// The app as the core's host.
    pub host: Arc<dyn ClientHost>,
    /// What the app does with its pages.
    pub shell: Arc<dyn ShellHost>,
    pub files: Box<dyn ShellFiles>,
    /// What the shell tells the user when it next loads an account (a sign-in that kept the account personal).
    pub notice: Mutex<HashMap<String, String>>,
    /// Accounts whose data the pages keep (scoped browser storage, the crypto store) is to be cleared: by the shell when
    /// it next loads, and by the next page of the server.
    pub cleared: Mutex<Cleared>,
    /// A check that the current account's key still signs in is under way.
    pub checking: AtomicBool,
    /// An SSO sign-in waiting for the browser to come back, with the device mode chosen for it.
    pub sso: Mutex<Option<(moqui_client::sso::Pending, Option<DeviceMode>)>>,
    /// A comms screen to show in the shell (a link of the OS or a notification), until the shell of a signed-in account
    /// takes it (`client_take_open`), so one opened while the shell signs in or loads is shown after.
    pub open_pending: Mutex<Option<String>>,
    /// The token of the shell's last load by the host with a query ([`Self::load_shell`]), which the shell's
    /// `client_state` answers once: the shell takes its query (signIn, failed, server) only from such a load.
    pub load_token: Mutex<Option<String>>,
}

impl ClientState {
    pub fn new(
        core: Arc<Client>,
        host: Arc<dyn ClientHost>,
        shell: Arc<dyn ShellHost>,
        files: Box<dyn ShellFiles>,
    ) -> Self {
        Self {
            core,
            host,
            shell,
            files,
            notice: Mutex::new(HashMap::new()),
            cleared: Mutex::new(Cleared::default()),
            checking: AtomicBool::new(false),
            sso: Mutex::new(None),
            open_pending: Mutex::new(None),
            load_token: Mutex::new(None),
        }
    }

    /// The account and key of the account a page shows ([`ClientHost::page_account`]).
    pub fn account_of(&self, page: &str) -> Option<(Account, Option<String>)> {
        self.core.account_of(self.host.as_ref(), page)
    }

    /// Load the shell again (another account, signed out), with `query` (`signIn=1` for the sign-in).
    pub fn load_shell(&self, query: Option<&str>) {
        // a query comes with a token the shell's state answers once, so only the host's load sets what the shell does
        let token = query.and_then(|_| moqui_client::store::new_client_ref().ok());
        moqui_lib::sync::lock(&self.load_token).clone_from(&token);
        self.shell.show_shell(&shell_url(query, token.as_deref()));
    }
}

/// The client's state on `app`, or why there is none.
///
/// # Errors
///
/// Returns an error when the app does not run as a client of a server.
pub fn state(app: &AppHandle) -> Result<tauri::State<'_, ClientState>, String> {
    app.try_state::<ClientState>()
        .ok_or_else(|| "Not a client of a server".to_string())
}

/// The shell's page, with `query` and the token that says the host loaded it.
pub fn shell_url(query: Option<&str>, token: Option<&str>) -> Url {
    let text = match (query, token) {
        (Some(query), Some(token)) => format!("{SHELL_ORIGIN}/?{query}&load={token}"),
        (Some(query), None) => format!("{SHELL_ORIGIN}/?{query}"),
        _ => format!("{SHELL_ORIGIN}/"),
    };
    Url::parse(&text).unwrap_or_else(|_| unreachable!("the shell's url parses"))
}

/// The storage scopes (account ids) whose page data each origin is to clear; each list is taken once.
#[derive(Default)]
pub struct Cleared {
    pub shell: Vec<String>,
    pub app: Vec<String>,
}

impl Cleared {
    pub fn add(&mut self, id: &str) {
        for list in [&mut self.shell, &mut self.app] {
            if !list.iter().any(|kept| kept == id) {
                list.push(id.to_string());
            }
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
