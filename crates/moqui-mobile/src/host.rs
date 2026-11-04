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

use std::path::PathBuf;
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
    /// The host itself is showing the shell (its first load, a sign-out, the way back), until the shell's load begins:
    /// only then may a navigation with no page yet go to the shell's origin.
    host_shell: bool,
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
        // any load began: the host's own showing of the shell, if it was one, is over
        shown.host_shell = false;
        shown.origin = Some(url.origin().ascii_serialization());
        shown.nonce = None;
    }

    /// The host shows the shell (not a page): a navigation to the shell's origin is its own until the load begins.
    pub(crate) fn host_shows_shell(&self) {
        moqui_lib::sync::lock(&self.shown).host_shell = true;
    }

    /// Where a navigation comes from, for [`goes`]: the origin shown; with none (a load under way), the shell's only
    /// while the host itself is showing the shell, else none.
    pub(crate) fn from(&self) -> Option<String> {
        let shown = moqui_lib::sync::lock(&self.shown);
        match &shown.origin {
            Some(origin) => Some(origin.clone()),
            None if shown.host_shell => Some(SHELL_ORIGIN.to_string()),
            None => None,
        }
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

    /// Whether the shell's current load has its nonce (it loaded, and is the shell's).
    pub(crate) fn has_nonce(&self) -> bool {
        moqui_lib::sync::lock(&self.shown).nonce.is_some()
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

/// Where a navigation the page at `from` (its origin, [`PageState::from`]) starts to `url` goes, with an account signed
/// in or not. Only the shell and the host go to the shell's origin: a server page never does (only the host shows the
/// shell, `client_show_shell`), nor does a navigation while a load is under way (`from` none) that the host did not
/// start.
pub(crate) fn goes(url: &Url, from: Option<&str>, signed_in: bool) -> Goes {
    let at = url.origin().ascii_serialization();
    if at == SHELL_ORIGIN {
        // a server page's link, or one while a load is under way: nowhere (the nonce guards the rest)
        if from != Some(SHELL_ORIGIN) {
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
    /// The client's directory (`push.json` beside the accounts).
    pub(crate) dir: PathBuf,
    /// The call answered on the phone whose screen has not yet taken the answer ([`Self::take_answer`]).
    answer: Mutex<Option<String>>,
}

impl MobileHost {
    pub(crate) fn new(app: AppHandle, network: bool, dir: PathBuf) -> Self {
        Self {
            app,
            client: Mutex::new(None),
            page: PageState::default(),
            network: AtomicBool::new(network),
            dir,
            answer: Mutex::new(None),
        }
    }

    /// A call link the native code made and the activity was opened with (MT2), its nonce already taken
    /// ([`crate::native::take_link`]). An Answer shows the call's screen of the ring's account's comms app (that account
    /// made current first), and the page answers and joins once it takes the answer ([`Self::take_answer`]); the shell
    /// takes the path as it loads (`client_take_open`), or now when it is loaded. A Hang up has the shell's call left.
    /// Not on the main thread: the comms app may be read from the store.
    pub(crate) fn open_call(&self, link: &crate::calls::CallLink) {
        if link.hang_up {
            self.hang_up(&link.call_session_id);
            return;
        }
        let Ok(state) = moqui_client_tauri::state(&self.app) else {
            return;
        };
        let switched = self.signed_in().as_deref() != Some(link.account.as_str());
        if switched {
            if let Err(err) = moqui_client_tauri::commands::switch_account(&state, &link.account) {
                log::warn!("moqui-mobile: the call {} is not opened: {err}", link.call_session_id);
                return;
            }
        }
        let root = state.core.comms_root_of(&link.account);
        *moqui_lib::sync::lock(&self.answer) = Some(link.call_session_id.clone());
        *moqui_lib::sync::lock(&state.open_pending) = Some(crate::calls::call_path(&root, link));
        if switched {
            // the shell is loading for the account, and takes it then
        } else if self.is_shell(MAIN) && self.page.has_nonce() {
            self.eval("window.moquiShell && window.moquiShell.poke()");
        } else {
            self.navigate(&moqui_client_tauri::shell_url(None, None));
        }
        log::info!("moqui-mobile: the call {} opens in the shell", link.call_session_id);
    }

    /// The answer the native code handed over for the call `id`, taken once by its call screen: only a call the user
    /// answered on the phone is answered by the page by itself.
    pub(crate) fn take_answer(&self, id: &str) -> bool {
        let mut answer = moqui_lib::sync::lock(&self.answer);
        if answer.as_deref() == Some(id) {
            *answer = None;
            return true;
        }
        false
    }

    /// The shell leaves the call `id`, if it is in it (the call notification's Hang up, a headset's).
    pub(crate) fn hang_up(&self, id: &str) {
        if self.is_shell(MAIN) && self.page.has_nonce() {
            self.eval(&crate::calls::hang_up_script(id));
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
        if url.origin().ascii_serialization() == SHELL_ORIGIN {
            self.page.host_shows_shell();
        }
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
        // the shell, and its call screen, go: over the lock screen the app does not show a server page
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::page_call_state("", "offScreen") {
            log::warn!("The native code was not told the call screen went: {err}");
        }
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

    fn account_signed_in(&self, account: &str) {
        let (Some(core), true) = (self.core(), self.network_ready()) else {
            return;
        };
        let (dir, account) = (self.dir.clone(), account.to_string());
        let started = std::thread::Builder::new()
            .name("push-register".to_string())
            .spawn(move || crate::native::register_push(&core, &dir, Some(&account)));
        if let Err(err) = started {
            log::warn!("Push: the registration did not start: {err}");
        }
    }

    fn account_signing_out(&self, account: &Account, key: &str) {
        if let Some(core) = self.core() {
            crate::native::unregister_push(&core, &self.dir, account, key);
        }
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
