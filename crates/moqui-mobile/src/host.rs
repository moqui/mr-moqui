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
    /// The origin a navigation under way goes to, until its load begins. The WebView asks the scheme for the new
    /// document before the load begins (`onPageStarted` comes once the document commits), so that request has only
    /// this to tell whose it is.
    to: Option<String>,
    /// The account the server pages were opened for (`open_remote`), until the account changes.
    app_account: Option<String>,
    /// The host (and port) of that account's server, which the app origin does not name: the title of the pages'
    /// dialogs and notifications (SEC-CLI-119).
    app_server: Option<String>,
    /// The nonce of the shell's current load, handed to its document once it loaded; none while it loads, after a
    /// navigation, and on any other origin.
    nonce: Option<String>,
    /// The host itself is showing the shell (its first load, a sign-out, the way back), until the shell's load begins:
    /// only then may a navigation with no page yet go to the shell's origin.
    host_shell: bool,
}

impl PageState {
    /// A navigation to `to` starts (the host's, or a page's link): the page that was there may still run, so no page is
    /// the shell until the next load begins; only a navigation to the server's pages has their account meanwhile, for
    /// the request of its document.
    pub(crate) fn navigating(&self, to: Option<&Url>) {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        shown.origin = None;
        shown.to = to.map(|url| url.origin().ascii_serialization());
        shown.nonce = None;
    }

    /// A load of `url` began: its document is the page from now on, with no nonce yet.
    pub(crate) fn started(&self, url: &Url) {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        // any load began: the host's own showing of the shell, if it was one, is over
        shown.host_shell = false;
        shown.origin = Some(url.origin().ascii_serialization());
        shown.to = None;
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

    /// The server pages about to open are `account`'s, of the server at `server` (its host and port).
    pub(crate) fn bind_app(&self, account: &str, server: Option<&str>) {
        let mut shown = moqui_lib::sync::lock(&self.shown);
        shown.app_account = Some(account.to_string());
        shown.app_server = server.map(str::to_string);
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
            shown.app_server = None;
        }
        theirs
    }

    /// The host of the server the server pages were opened for, while they are bound to its account.
    pub(crate) fn app_server(&self) -> Option<String> {
        moqui_lib::sync::lock(&self.shown).app_server.clone()
    }

    /// The origin shown, once a load began.
    pub(crate) fn origin(&self) -> Option<String> {
        moqui_lib::sync::lock(&self.shown).origin.clone()
    }

    /// The account of the page shown, with `current` the current account: the shell's is the current one; a server
    /// page's the one it was opened for, while that is the current one, from the navigation to it on (its document is
    /// asked for before its load begins); none while a navigation to anywhere else is under way.
    pub(crate) fn account(&self, current: Option<&str>) -> Option<String> {
        let shown = moqui_lib::sync::lock(&self.shown);
        let current = current?;
        let page = match shown.origin.as_deref() {
            Some(origin) => Some(origin),
            None => shown.to.as_deref().filter(|to| *to == APP_ORIGIN),
        };
        match page {
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

/// What the host evaluates in the shell's document to hand it its nonce, the desktop's too.
pub(crate) use moqui_client_tauri::shell_load::nonce_script;

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
    /// A number from the phone's dialer waiting for the user's pick of the account that calls it ([`Self::dial_choice`]).
    dial: Mutex<Option<crate::dial::DialChoice>>,
    /// Held while the page's call state goes to the native code (`call_state`) and while a navigation tells it the page
    /// went ([`Self::navigate`]): a `joined` checked against a page that is going is told before the page's end, which
    /// then ends it, never after.
    pub(crate) calls: Mutex<()>,
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
            dial: Mutex::new(None),
            calls: Mutex::new(()),
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

    /// The client's directory (`push.json` beside the accounts).
    pub(crate) fn client_dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// Show `path`, a screen of the current account's comms app, in the shell (a notification's tap): kept for the
    /// shell to take (`client_take_open`), and the shell shown or poked.
    pub(crate) fn open_shell_path(&self, path: &str) {
        let Ok(state) = moqui_client_tauri::state(&self.app) else {
            return;
        };
        *moqui_lib::sync::lock(&state.open_pending) = Some(path.to_string());
        if self.is_shell(MAIN) && self.page.has_nonce() {
            self.eval("window.moquiShell && window.moquiShell.poke()");
        } else {
            self.navigate(&moqui_client_tauri::shell_url(None, None));
        }
    }

    /// A room link (`org.moqui.mobile:/room`): opened when this app signed it ([`crate::native::room_link_taken`]), else
    /// refused (logged).
    pub(crate) fn open_room_link(&self, url: &tauri::Url) {
        let Ok(state) = moqui_client_tauri::state(&self.app) else {
            return;
        };
        match crate::native::room_link_taken(&state.core, url) {
            Some((account, room, seq)) => self.open_room(&account, &room, seq),
            None => log::warn!("moqui-mobile: a room link this app did not make is refused"),
        }
    }

    /// A room's invite link from its join page (`org.moqui.mobile:/join`, [`crate::join`]): its page in the current
    /// account's shell when that account is on its host, where the person presses Join. Any web page can make the link,
    /// so it never switches the account (SEC-CLI-99): with another account here on the host, the page shown says so
    /// and that the person switches from the account menu; with none, that there is none.
    pub(crate) fn open_join_link(&self, host: &str, token: &str) {
        let Ok(state) = moqui_client_tauri::state(&self.app) else {
            return;
        };
        let (accounts, current) = {
            let accounts = moqui_lib::sync::lock(state.core.accounts());
            (
                accounts.list().to_vec(),
                accounts.current().map(|account| account.id.clone()),
            )
        };
        match crate::join::join_action(&accounts, current.as_deref(), host) {
            crate::join::JoinAction::Open => {
                self.open_shell_path(&crate::join::join_path(token));
                log::info!("moqui-mobile: a room's join page opens in the shell");
            }
            crate::join::JoinAction::OtherAccount => {
                log::warn!(
                    "moqui-mobile: a join link of {host} is not opened: its account here is not the current one"
                );
                self.tell_warning(&format!(
                    "This invite is for your account on {host}: switch to it from the account menu, then open the invite again"
                ));
            }
            crate::join::JoinAction::NoAccount => {
                log::warn!("moqui-mobile: a join link of {host} is not opened: no account on that server here");
                self.tell_warning(&format!("This invite is for {host}, and this app has no account there"));
            }
        }
    }

    /// A warning in the page shown (the shell's or a server page's message bar), localized where the page can.
    fn tell_warning(&self, message: &str) {
        let text = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string());
        self.eval(&format!(
            "window.moqui && moqui.notifyMessages && moqui.notifyMessages([{{type: 'warning', message: \
             (moqui.l10n && moqui.l10n.localize ? moqui.l10n.localize({text}) : {text})}}])"
        ));
    }

    /// A chat notification's tap (a room link this app signed): the room of `account` at the message, in the shell; the
    /// account, which must be one signed in here, becomes the current one first.
    pub(crate) fn open_room(&self, account: &str, room: &str, seq: Option<i64>) {
        let Ok(state) = moqui_client_tauri::state(&self.app) else {
            return;
        };
        if state.core.keys().get(account).is_none() {
            log::warn!("moqui-mobile: the room {room} is not opened: its account is not signed in here");
            return;
        }
        let switched = self.signed_in().as_deref() != Some(account);
        if switched {
            if let Err(err) = moqui_client_tauri::commands::switch_account(&state, account) {
                log::warn!("moqui-mobile: the room {room} is not opened: {err}");
                return;
            }
        }
        let root = state.core.comms_root_of(account);
        let mut path = format!("{}/Chat?chatRoomId={room}", root.trim_end_matches('/'));
        if let Some(seq) = seq {
            path.push_str(&format!("&eventSeq={seq}"));
        }
        if switched {
            // the shell is loading for the account, and takes it then
            *moqui_lib::sync::lock(&state.open_pending) = Some(path);
        } else {
            self.open_shell_path(&path);
        }
        log::info!("moqui-mobile: the room {room} opens in the shell");
    }

    /// A number the phone's dialer handed over (plans/Comms-Messaging.md §9.1): filled in on the Dialer of the account
    /// that calls ([`crate::dial::DialAccounts::choose`]), where the user presses Call; nothing is placed by itself. The
    /// current account when it can call (or none can: its Dialer says why), the one that can when only one can (made
    /// current), else the current one's Dialer asks which ([`Self::dial_choice`]). Not on the main thread.
    pub(crate) fn open_dial(&self, number: &str) {
        let Ok(state) = moqui_client_tauri::state(&self.app) else {
            return;
        };
        let Some(current) = self.signed_in() else {
            log::warn!("moqui-mobile: a number from the phone's dialer, with no account signed in: not shown");
            return;
        };
        let signed_in = crate::native::signed_in_ids(&state.core);
        let (account, choose) = match crate::dial::DialAccounts::load(&self.dir).choose(Some(&current), &signed_in) {
            crate::dial::Chosen::Current => (current.clone(), false),
            crate::dial::Chosen::Switch(other) => (other, false),
            crate::dial::Chosen::Ask(accounts) => {
                *moqui_lib::sync::lock(&self.dial) = Some(crate::dial::DialChoice {
                    number: number.to_string(),
                    accounts,
                    shown_on: Some(current.clone()),
                    at: crate::native::now_ms(),
                });
                (current.clone(), true)
            }
        };
        self.show_dialer(&state, &account, number, choose, current != account);
    }

    /// The Dialer of `account` with `number` filled in (`choose`: it asks which account), `switch`ing to it first.
    fn show_dialer(
        &self,
        state: &moqui_client_tauri::ClientState,
        account: &str,
        number: &str,
        choose: bool,
        switch: bool,
    ) {
        if switch {
            if let Err(err) = moqui_client_tauri::commands::switch_account(state, account) {
                log::warn!("moqui-mobile: a number from the phone's dialer is not shown: {err}");
                return;
            }
        }
        let path = crate::dial::dialer_path(&state.core.comms_root_of(account), number, choose);
        if switch {
            // the shell is loading for the account, and takes it then
            *moqui_lib::sync::lock(&state.open_pending) = Some(path);
        } else {
            self.open_shell_path(&path);
        }
        log::info!("moqui-mobile: a number from the phone's dialer is filled in on the Dialer");
    }

    /// The number from the phone's dialer waiting for the user's pick, and the accounts to pick from (`{id, label}`):
    /// for the Dialer of the account it was shown on, within two minutes; none otherwise.
    pub(crate) fn dial_choice(&self) -> Option<(String, Vec<(String, String)>)> {
        let state = moqui_client_tauri::state(&self.app).ok()?;
        let choice = moqui_lib::sync::lock(&self.dial).clone()?;
        if !choice.current(self.signed_in().as_deref(), crate::native::now_ms()) {
            return None;
        }
        let accounts = moqui_lib::sync::lock(state.core.accounts());
        let listed = choice
            .accounts
            .iter()
            .filter_map(|id| accounts.get(id))
            .map(|account| {
                let place = if account.server_name.is_empty() {
                    &account.server
                } else {
                    &account.server_name
                };
                (account.id.clone(), format!("{} · {place}", account.username))
            })
            .collect();
        Some((choice.number, listed))
    }

    /// The user picked `account` for the number waiting ([`Self::dial_choice`]): its Dialer, with the number filled in,
    /// that account made current. Taken once.
    pub(crate) fn dial_from(&self, account: &str) -> Result<(), String> {
        let state = moqui_client_tauri::state(&self.app)?;
        let choice = moqui_lib::sync::lock(&self.dial)
            .take()
            .ok_or("No number waits for an account")?;
        if !choice.current(self.signed_in().as_deref(), crate::native::now_ms())
            || !choice.accounts.iter().any(|id| id == account)
        {
            return Err("That account does not call this number".to_string());
        }
        let switch = self.signed_in().as_deref() != Some(account);
        self.show_dialer(&state, account, &choice.number, false, switch);
        Ok(())
    }

    /// The accounts that can call, kept to those still signed in here (a key revoked elsewhere is gone at the next
    /// start), and the calling account registered to match.
    pub(crate) fn prune_dial_accounts(&self, signed_in: &[String]) {
        let mut accounts = crate::dial::DialAccounts::load(&self.dir);
        if !accounts.keep_only(signed_in) {
            return;
        }
        if let Err(err) = accounts.save(&self.dir) {
            log::warn!("The accounts that call were not kept: {err}");
        }
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::dial_account(accounts.any()) {
            log::warn!("The calling account was not changed: {err}");
        }
    }

    /// Whether `account` has a number to call from (the shell's word as it loads, or a sign-out): the calling account
    /// for the phone's dialer is registered while one here has (`DialAccount.set`).
    pub(crate) fn set_dial_account(&self, account: &str, on: bool) -> Result<(), String> {
        let mut accounts = crate::dial::DialAccounts::load(&self.dir);
        if accounts.set(account, on) {
            accounts.save(&self.dir)?;
        }
        let any = accounts.any();
        #[cfg(target_os = "android")]
        if !crate::android::dial_account(any)? {
            return Err("Telecom did not take the calling account".to_string());
        }
        #[cfg(not(target_os = "android"))]
        log::info!("The calling account: {}", if any { "registered" } else { "none" });
        Ok(())
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

    /// The native call tells the shell's call screen something ([`crate::calls::tell_script`]).
    pub(crate) fn tell_call(&self, script: &str) {
        if self.is_shell(MAIN) && self.page.has_nonce() {
            self.eval(script);
        }
    }

    /// The shell leaves the call `id`, if it is in it or has it on hold (the call notification's Hang up, a headset's).
    pub(crate) fn hang_up(&self, id: &str) {
        // an answer handed over and not taken yet goes with the call (Hang up before the page joined)
        {
            let mut answer = moqui_lib::sync::lock(&self.answer);
            if answer.as_deref() == Some(id) {
                *answer = None;
            }
        }
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
        self.page_gone(Some(url));
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

    /// The page goes (a navigation of the host's to `navigating`, or any new document: `PageLoadEvent::Started`, a
    /// reload, the webview's own history, a renderer gone and loaded again): with `navigating`, the page state too.
    /// The page leaves with its calls' media, so the native calls it joined end with it; a ring, or a call answered on
    /// the phone and not joined yet, stays. Under [`Self::calls`], so a `joined` on its way is told first. The room the
    /// shell showed is shown no more: the shell's own word of it (`chat_room_shown`) needs its load's nonce, which the
    /// navigation has taken (SEC-CLI-97).
    pub(crate) fn page_gone(&self, navigating: Option<&Url>) {
        let _calls = moqui_lib::sync::lock(&self.calls);
        if let Some(to) = navigating {
            self.page.navigating(Some(to));
        }
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::calls_page_gone() {
            log::warn!("The native calls were not told the page went: {err}");
        }
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::chat_room_shown("", "") {
            log::warn!("The chat notifications were not told the page went: {err}");
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
        // the vault of the account whose pages go locks with them (V5)
        if let Some(core) = self.core() {
            match account {
                Some(account) => core.vault().lock(account),
                None => core.vault().lock_all(),
            }
        }
        // the server pages are the bound account's: they are no account's, and go, before the current account changes
        let theirs = self.page.unbind_app(account);
        if theirs {
            tell_page_server(None);
        }
        if theirs && self.page.origin().as_deref() != Some(SHELL_ORIGIN) {
            self.navigate(&moqui_client_tauri::shell_url(None, None));
        }
    }

    fn open_remote(&self, path: &str) -> Result<(), String> {
        let path = server_path(path)?;
        // the shell, and its call screen, go: over the lock screen the app does not show a server page
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::page_call_state("", "offScreen", "") {
            log::warn!("The native code was not told the call screen went: {err}");
        }
        let account = self.signed_in().ok_or("No account is signed in")?;
        let url = Url::parse(&format!("{APP_ORIGIN}{path}")).map_err(|err| err.to_string())?;
        // the pages' dialogs and notifications name their server, which the app origin does not (SEC-CLI-119)
        let server = self
            .core()
            .and_then(|core| core.account(&account))
            .and_then(|(account, _)| crate::join::server_host(&account));
        self.page.bind_app(&account, server.as_deref());
        tell_page_server(server.as_deref());
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

    /// `KeyguardManager.isDeviceSecure()`, asked as the shell loads (SEC-CLI-113); none off Android.
    #[cfg(target_os = "android")]
    fn device_secure(&self) -> Option<bool> {
        crate::android::device_secure()
            .inspect_err(|err| log::warn!("Whether the phone has a screen lock was not read: {err}"))
            .ok()
    }

    fn account_signed_in(&self, account: &str) {
        crate::native::account_back(account);
        let (Some(core), true) = (self.core(), self.network_ready()) else {
            return;
        };
        let (dir, account) = (self.dir.clone(), account.to_string());
        let started = std::thread::Builder::new()
            .name("push-register".to_string())
            .spawn(move || {
                crate::native::register_or_retry(&core, &dir, Some(&account));
                unified_register_all();
                fcm_start();
            });
        if let Err(err) = started {
            log::warn!("Push: the registration did not start: {err}");
        }
    }

    fn account_signing_out(&self, account: &Account, key: &str) {
        // its live location shares are stopped on its server while the key is held, so none stays on with nothing
        // sending it, and are no longer sent from this phone (a removal has only this; plans/Location-Sharing.md §3.4)
        if let (Some(core), true) = (self.core(), self.network_ready()) {
            #[cfg(target_os = "android")]
            let waiting = crate::android::location_pending_stops(&account.id).unwrap_or_default();
            #[cfg(not(target_os = "android"))]
            let waiting: Vec<String> = Vec::new();
            let stopped = crate::location::stop_account_on_server(
                &account.id,
                &waiting,
                (account, key),
                &crate::native::LocationPoster(&core),
            );
            for (share, state) in &stopped {
                #[cfg(target_os = "android")]
                if let Err(err) = crate::android::location_stop_settled(&account.id, share, state.as_str()) {
                    log::warn!("The location stops were not told of share {share}: {err}");
                }
                log::info!(
                    "Location: share {share} of {} for its sign-out: {}",
                    account.id,
                    state.as_str()
                );
            }
        }
        forget_location(&account.id);
        if let Some(core) = self.core() {
            crate::native::unregister_push(&core, &self.dir, account, key);
        }
        // its number no longer calls from the phone's dialer (`account_signed_out` again; a removal has only this)
        if let Err(err) = self.set_dial_account(&account.id, false) {
            log::warn!("The calling account was not changed for the sign-out: {err}");
        }
        unified_register_all();
    }

    /// An account is signed out here, its key gone, whether its server heard or not (a sign-out with no network, a key
    /// the server no longer takes): what the phone shows and keeps of it goes, with no request to its server (one
    /// before the TLS verifier is ready would panic). Its chat notifications, conversation shortcuts and a room of it
    /// shown (`ChatNotifications.forgetAccount`, SEC-CLI-100); its number for the phone's dialer; its push registration,
    /// keys and UnifiedPush instance here (taken back from the distributor by `registerAll`); and with no account left
    /// signed in, the FCM token and the Firebase installation (SEC-CLI-121). The server's take-back of the push
    /// registration is [`Self::account_signing_out`]'s, when the app can reach it.
    fn account_signed_out(&self, account: &str) {
        forget_chats(account);
        forget_location(account);
        if let Err(err) = self.set_dial_account(account, false) {
            log::warn!("The calling account was not changed for the sign-out: {err}");
        }
        let last = self
            .core()
            .is_some_and(|core| crate::native::push_signed_out(&core, &self.dir, account));
        if last {
            fcm_stop();
        }
        unified_register_all();
    }

    /// The server pages' origin, which every account's pages share in the one webview, loses what the last account's
    /// pages kept there (`PageStorage.clear`, SEC-CLI-81).
    fn clear_page_storage(&self) {
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::clear_page_storage() {
            log::warn!("The server pages' storage was not cleared for the change of account: {err}");
        }
        #[cfg(not(target_os = "android"))]
        log::info!("The server pages' storage is cleared");
    }

    /// An offline map package's download goes to `PackageDownloadWorker` (WorkManager: resumable, with a network,
    /// unmetered with `wifi_only`; plans/Maps-Offline.md §2.4), which calls Rust back (`native::package_download`);
    /// off Android, a thread as on the desktop.
    fn start_package_download(
        &self,
        core: &Arc<Client>,
        host: &Arc<dyn ClientHost>,
        account: &str,
        package_id: &str,
        revision: i64,
        wifi_only: bool,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            let _ = (core, host);
            crate::android::package_worker_start(account, package_id, revision, wifi_only)
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = wifi_only;
            moqui_client_tauri::packages::run_task(core, host, account, package_id, revision)
        }
    }

    fn cancel_package_download(&self, account: &str, package_id: &str) {
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::package_worker_cancel(Some(package_id), account) {
            log::warn!("The offline map's worker was not cancelled: {err}");
        }
        #[cfg(not(target_os = "android"))]
        let _ = (account, package_id);
    }

    fn cancel_package_downloads(&self, account: &str) {
        #[cfg(target_os = "android")]
        if let Err(err) = crate::android::package_worker_cancel(None, account) {
            log::warn!("The account's offline map workers were not cancelled: {err}");
        }
        #[cfg(not(target_os = "android"))]
        let _ = account;
    }
}

/// The chat notifications, conversation shortcuts and shown room of `account`, signed out, go
/// (`ChatNotifications.forgetAccount`, SEC-CLI-100).
fn forget_chats(account: &str) {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::forget_account(account) {
        log::warn!("The chat notifications of a signed-out account were not taken off: {err}");
    }
    #[cfg(not(target_os = "android"))]
    log::info!("The chat notifications of {account} go");
}

/// The live location shares of `account`, signed out or removed, are no longer sent from this phone: its reporters go
/// and the location service stops them, ending itself when it sends no other (plans/Location-Sharing.md §3.4).
fn forget_location(account: &str) {
    let shares = crate::location::forget_account(account);
    if !shares.is_empty() {
        log::info!(
            "Location: {} shares of {account} are no longer sent from here",
            shares.len()
        );
    }
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::location_forget_account(account) {
        log::warn!("The location service was not told of a signed-out account: {err}");
    }
}

/// No account is signed in here any more: Firebase's token and installation are deleted (`Push.stopFcm`; nothing in a
/// build without Firebase), and the next sign-in asks for a new token ([`fcm_start`], SEC-CLI-121).
fn fcm_stop() {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::fcm_stop() {
        log::warn!("Push: Firebase's token was not deleted at the last sign-out: {err}");
    }
}

/// The server whose pages the app origin shows, or none, for the title of their alert and confirm dialogs (wry's
/// `RustWebChromeClient.pageServer`, SEC-CLI-119).
fn tell_page_server(server: Option<&str>) {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::page_server(server) {
        log::warn!("The page dialogs were not told the server: {err}");
    }
    #[cfg(not(target_os = "android"))]
    log::debug!("The server pages' server: {server:?}");
}

/// The UnifiedPush instances follow the accounts signed in here (`UnifiedPushes.registerAll`): one for an account that
/// signed in, none for one that signed out.
fn unified_register_all() {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::unified_register_all() {
        log::warn!("Push: UnifiedPush was not told of the accounts: {err}");
    }
}

/// Firebase asked for its token once an account signs in (`Push.startFcm`): the app's start asks only with an account
/// signed in already, so a Firebase build reaches Google from the first sign-in, not the first start (SEC-CLI-121).
fn fcm_start() {
    #[cfg(target_os = "android")]
    if let Err(err) = crate::android::fcm_start() {
        log::warn!("Push: Firebase was not started for the sign-in: {err}");
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
