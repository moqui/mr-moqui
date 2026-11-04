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

//! The mobile app as the client core's host (plans/Mobile-Desktop.md §4 "Remote applications and authentication"): one
//! webview, `main`, that shows the bundled shell (`moqui-shell`) or a page of the current account's server
//! (`moqui-app`), never both. Remote apps navigate it to the app origin; the shell comes back by the page's back button
//! (`client_show_shell`), Android's back, or a sign-out or switch.
//!
//! The command set is the boundary: Tauri counts both schemes' pages as local, so the capability cannot tell them
//! apart, and the shell's commands check the page ([`PageState`]): the webview's URL on the shell's origin, and a load
//! of the shell begun since the last navigation, so a server page that navigates to the shell cannot call them while
//! it is still the page. A server page has the sockets, the outbox and the drafts of the current account, as the
//! desktop's app windows do. The device key never reaches a page.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use moqui_client::account::Account;
use moqui_client::app::{Client, ClientHost};
use moqui_client_tauri::{ShellHost, APP_ORIGIN, SHELL_ORIGIN};
use serde_json::Value;
use tauri::{AppHandle, Manager, Url, Webview};

/// The label of the one webview.
pub(crate) const MAIN: &str = "main";

/// Which origin the one webview shows, as its loads say.
#[derive(Debug, Default)]
pub(crate) struct PageState {
    /// The origin of the page whose load began last, none while a navigation is under way.
    shown: Mutex<Option<String>>,
}

impl PageState {
    /// A navigation starts (the host's, or a page's link): the page that was there may still run, so no page is the
    /// shell until the next load begins.
    pub(crate) fn navigating(&self) {
        *moqui_lib::sync::lock(&self.shown) = None;
    }

    /// A load of `url` began: its document is the page from now on.
    pub(crate) fn started(&self, url: &Url) {
        *moqui_lib::sync::lock(&self.shown) = Some(url.origin().ascii_serialization());
    }

    /// The origin shown, once a load began.
    pub(crate) fn origin(&self) -> Option<String> {
        moqui_lib::sync::lock(&self.shown).clone()
    }

    /// Whether a page at `url` (the webview's URL as a command came) is the shell: on the shell's origin, which the
    /// webview's last load began on.
    pub(crate) fn is_shell(&self, url: Option<&Url>) -> bool {
        let at = url.map(|url| url.origin().ascii_serialization());
        at.as_deref() == Some(SHELL_ORIGIN) && self.origin().as_deref() == Some(SHELL_ORIGIN)
    }
}

/// Where a navigation of the one webview to `url` goes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Goes {
    /// The webview goes there: the shell's page, a server page (with an account signed in), or `about:blank`.
    Here,
    /// A server path on the shell's origin (a link the shell's page has): the same path on the app origin.
    AppPage,
    /// The web, or a `mailto:`, `tel:` or `sip:` link: the system browser or handler.
    Outside,
    /// Nowhere.
    Nowhere,
}

/// Where a navigation to `url` goes, with an account signed in or not.
pub(crate) fn goes(url: &Url, signed_in: bool) -> Goes {
    let at = url.origin().ascii_serialization();
    if at == SHELL_ORIGIN {
        return if matches!(url.path(), "/" | "/index.html") {
            Goes::Here
        } else if signed_in {
            Goes::AppPage
        } else {
            Goes::Nowhere
        };
    }
    if at == APP_ORIGIN {
        return if signed_in { Goes::Here } else { Goes::Nowhere };
    }
    match url.scheme() {
        "about" if url.path() == "blank" => Goes::Here,
        "http" | "https" if url.host_str().is_some_and(|host| !host.ends_with(".localhost")) => Goes::Outside,
        "mailto" | "tel" | "sip" => Goes::Outside,
        _ => Goes::Nowhere,
    }
}

/// A path on the server a page may be opened at: `/` and more, no scheme-relative `//`, no control character or
/// backslash.
pub(crate) fn server_path(path: &str) -> Result<&str, String> {
    let trimmed = path.trim();
    if !trimmed.starts_with('/') || trimmed.starts_with("//") || trimmed.chars().any(|c| c.is_control() || c == '\\') {
        return Err(format!("Not a path on the server: {trimmed}"));
    }
    Ok(trimmed)
}

/// The mobile app as the core's host and as the shell commands' [`ShellHost`].
pub(crate) struct MobileHost {
    pub(crate) app: AppHandle,
    /// The client, once made (the host is made first, as the client's state holds it).
    pub(crate) client: Mutex<Option<Arc<Client>>>,
    pub(crate) page: PageState,
    /// The TLS verifier is set up: requests may be made.
    pub(crate) network: AtomicBool,
}

impl MobileHost {
    pub(crate) fn new(app: AppHandle, network: bool) -> Self {
        Self {
            app,
            client: Mutex::new(None),
            page: PageState::default(),
            network: AtomicBool::new(network),
        }
    }

    fn core(&self) -> Option<Arc<Client>> {
        moqui_lib::sync::lock(&self.client).clone()
    }

    /// The current account's id, when it has a key.
    pub(crate) fn signed_in(&self) -> Option<String> {
        let (account, key) = self.core()?.current()?;
        key.map(|_| account.id)
    }

    /// Show `url` in the one webview; its sockets go with the page that leaves.
    pub(crate) fn navigate(&self, url: &Url) {
        self.page.navigating();
        if let Some(core) = self.core() {
            core.sockets().close_for(MAIN);
        }
        match self.app.get_webview_window(MAIN) {
            Some(window) => {
                if let Err(err) = window.navigate(url.clone()) {
                    log::error!("The page did not load: {err}");
                }
            }
            None => log::warn!("No webview to show {url}"),
        }
    }

    /// Evaluate `script` in the page.
    fn eval(&self, script: &str) {
        if let Some(window) = self.app.get_webview_window(MAIN) {
            if let Err(err) = window.eval(script) {
                log::debug!("The page was not told: {err}");
            }
        }
    }
}

impl ClientHost for MobileHost {
    fn page_account(&self, page: &str) -> Option<String> {
        if page != MAIN {
            return None;
        }
        let core = self.core()?;
        let current = moqui_lib::sync::lock(core.accounts())
            .current()
            .map(|account| account.id.clone());
        current
    }

    fn is_shell(&self, page: &str) -> bool {
        page == MAIN && self.page.origin().as_deref() == Some(SHELL_ORIGIN)
    }

    fn notify_pages(&self, account: &str, event: &Value) {
        if self.page_account(MAIN).as_deref() == Some(account) {
            self.eval(&format!(
                "window.__moquiOutboxEvent && window.__moquiOutboxEvent({event})"
            ));
        }
    }

    fn socket_event(&self, page: &str, id: u64, event: &Value) {
        if page == MAIN {
            self.eval(&format!(
                "window.__moquiSocketEvent && window.__moquiSocketEvent({id}, {event})"
            ));
        }
    }

    fn check_key(&self, account: &Account) {
        moqui_client_tauri::scheme::check_key(&self.app, account);
    }
}

impl ShellHost for MobileHost {
    fn show_shell(&self, url: &Url) {
        self.navigate(url);
    }

    fn close_remotes(&self, account: Option<&str>) {
        // the one webview shows a server page of the current account only
        let current = self.page_account(MAIN);
        let shows = self.page.origin().as_deref() == Some(APP_ORIGIN);
        if shows && account.is_none_or(|account| current.as_deref() == Some(account)) {
            self.navigate(&moqui_client_tauri::shell_url(None, None));
        }
    }

    fn open_remote(&self, path: &str) -> Result<(), String> {
        let path = server_path(path)?;
        if self.signed_in().is_none() {
            return Err("No account is signed in".to_string());
        }
        let url = Url::parse(&format!("{APP_ORIGIN}{path}")).map_err(|err| err.to_string())?;
        self.navigate(&url);
        Ok(())
    }

    fn open_external(&self, url: &Url) {
        crate::open_externally(url);
    }

    fn device_name(&self) -> String {
        crate::device_name()
    }

    fn sso_redirect(&self) -> &'static str {
        moqui_client::sso::MOBILE_REDIRECT
    }

    fn is_shell_page(&self, webview: &Webview) -> bool {
        webview.label() == MAIN && self.page.is_shell(webview.url().ok().as_ref())
    }

    fn network_ready(&self) -> bool {
        self.network.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
