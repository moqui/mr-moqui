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
//! apart, and the shell's commands check the page ([`PageState`]): the webview's URL on the shell's origin, and the
//! nonce of the shell's current load, which the host hands the shell's document once it has loaded and forgets at the
//! next navigation, so a document that was there before (a server page) cannot call them. A server page does not
//! navigate to the shell itself; only the host does (`client_show_shell`). A server page is bound to the account it was
//! opened for: it has the sockets, the outbox and the drafts of that account while it is the current one, as the
//! desktop's app windows do, and none after a switch. The device key never reaches a page. Only the main frame of
//! the app's two origins has the IPC at all (wry's patch, `patches/README.md`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use moqui_client::account::Account;
use moqui_client::app::{Client, ClientHost};
use moqui_client_tauri::{ShellHost, APP_ORIGIN, SHELL_ORIGIN};
use serde_json::Value;
use tauri::{AppHandle, Manager, Url, Webview};

/// The label of the one webview.
pub(crate) const MAIN: &str = "main";

/// What the one webview shows, as its loads say.
#[derive(Debug, Default)]
pub(crate) struct PageState {
    shown: Mutex<Shown>,
}

#[derive(Debug, Default)]
struct Shown {
    /// The origin of the page whose load began last, none while a navigation is under way.
    origin: Option<String>,
    /// The account the server pages were opened for (`open_remote`), until the account changes.
    app_account: Option<String>,
    /// The nonce of the shell's current load, handed to its document once it loaded; none while it loads, after a
    /// navigation, and on any other origin.
    nonce: Option<String>,
}

impl PageState {
    /// A navigation starts (the host's, or a page's link): the page that was there may still run, so no page is the
    /// shell or has an account until the next load begins.
    pub(crate) fn navigating(&self) {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        shown.origin = None;
        shown.nonce = None;
    }

    /// A load of `url` began: its document is the page from now on, with no nonce yet.
    pub(crate) fn started(&self, url: &Url) {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        shown.origin = Some(url.origin().ascii_serialization());
        shown.nonce = None;
    }

    /// The shell's load of `url` finished: a new nonce for it, to hand its document; none when the page is not the
    /// shell's (any more).
    pub(crate) fn shell_loaded(&self, url: &Url) -> Option<String> {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        let here = url.origin().ascii_serialization();
        if here != SHELL_ORIGIN || shown.origin.as_deref() != Some(SHELL_ORIGIN) {
            return None;
        }
        let nonce = moqui_client::store::new_client_ref().ok()?;
        shown.nonce = Some(nonce.clone());
        Some(nonce)
    }

    /// The server pages about to open are `account`'s.
    pub(crate) fn bind_app(&self, account: &str) {
        moqui_lib::sync::lock(&self.shown).app_account = Some(account.to_string());
    }

    /// The server pages of `account`, or of any account, are no account's any more (a switch, a sign-out): whether they
    /// were bound to it.
    pub(crate) fn unbind_app(&self, account: Option<&str>) -> bool {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        let theirs = match (&shown.app_account, account) {
            (Some(_), None) => true,
            (Some(bound), Some(account)) => bound == account,
            (None, _) => false,
        };
        if theirs {
            shown.app_account = None;
        }
        theirs
    }

    /// The origin shown, once a load began.
    pub(crate) fn origin(&self) -> Option<String> {
        moqui_lib::sync::lock(&self.shown).origin.clone()
    }

    /// The account of the page shown, with `current` the current account: the shell's is the current one; a server
    /// page's the one it was opened for, while that is the current one; none while a navigation is under way.
    pub(crate) fn account(&self, current: Option<&str>) -> Option<String> {
        let shown = moqui_lib::sync::lock(&self.shown);
        let current = current?;
        match shown.origin.as_deref() {
            Some(SHELL_ORIGIN) => Some(current.to_string()),
            Some(APP_ORIGIN) => shown
                .app_account
                .as_deref()
                .filter(|bound| *bound == current)
                .map(str::to_string),
            _ => None,
        }
    }

    /// Whether a page at `url` (the webview's URL as a command came), with `nonce`, is the shell: on the shell's
    /// origin, whose current load's nonce it has.
    pub(crate) fn is_shell(&self, url: Option<&Url>, nonce: Option<&str>) -> bool {
        let at = url.map(|url| url.origin().ascii_serialization());
        let shown = moqui_lib::sync::lock(&self.shown);
        at.as_deref() == Some(SHELL_ORIGIN)
            && shown.origin.as_deref() == Some(SHELL_ORIGIN)
            && shown.nonce.is_some()
            && shown.nonce.as_deref() == nonce
    }
}

/// What the host evaluates in the shell's document to hand it its nonce: it takes it only on the shell's origin, so a
/// document that came in its place gets nothing.
pub(crate) fn nonce_script(nonce: &str) -> String {
    let quoted = serde_json::to_string(nonce).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "if (location.origin === {origin:?} && window.__moquiShellNonce) window.__moquiShellNonce({quoted});",
        origin = SHELL_ORIGIN
    )
}

/// Whether `host` is one of this device's or the app's own (`localhost`, `*.localhost`), never opened outside the app.
pub(crate) fn own_host(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
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

/// Where a navigation the page at `from` (its origin) starts to `url` goes, with an account signed in or not. A server
/// page never goes to the shell itself: only the host shows it (`client_show_shell`). With no page yet (`from` none),
/// the webview's first load, it goes.
pub(crate) fn goes(url: &Url, from: Option<&str>, signed_in: bool) -> Goes {
    let at = url.origin().ascii_serialization();
    if at == SHELL_ORIGIN {
        // none yet: the webview's first load, the shell; a server page's link: nowhere (the nonce guards the rest)
        if from.is_some_and(|from| from != SHELL_ORIGIN) {
            return Goes::Nowhere;
        }
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
        "http" | "https" if url.host_str().is_some_and(|host| !own_host(host)) => Goes::Outside,
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

    /// The current account's id.
    fn current(&self) -> Option<String> {
        let core = self.core()?;
        let current = moqui_lib::sync::lock(core.accounts())
            .current()
            .map(|account| account.id.clone());
        current
    }

    /// The shell's load of `url` finished: its document gets the load's nonce.
    pub(crate) fn shell_loaded(&self, url: &Url) {
        if let Some(nonce) = self.page.shell_loaded(url) {
            self.eval(&nonce_script(&nonce));
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
        self.page.account(self.current().as_deref())
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
        // the server pages are the bound account's: they are no account's, and go, before the current account changes
        let theirs = self.page.unbind_app(account);
        if theirs && self.page.origin().as_deref() != Some(SHELL_ORIGIN) {
            self.navigate(&moqui_client_tauri::shell_url(None, None));
        }
    }

    fn open_remote(&self, path: &str) -> Result<(), String> {
        let path = server_path(path)?;
        let account = self.signed_in().ok_or("No account is signed in")?;
        let url = Url::parse(&format!("{APP_ORIGIN}{path}")).map_err(|err| err.to_string())?;
        self.page.bind_app(&account);
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

    fn is_shell_page(&self, webview: &Webview, nonce: Option<&str>) -> bool {
        webview.label() == MAIN && self.page.is_shell(webview.url().ok().as_ref(), nonce)
    }

    fn network_ready(&self) -> bool {
        self.network.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
