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

//! The app's OS integration (plans/Mobile-Desktop.md §4 configuration 1, "Desktop features" and "First run"): one
//! instance per user ([`instance`]), start at login ([`autostart`]), the `mailto:`, `tel:` and `sip:` handler
//! ([`links`], [`handlers`]) and the attach wizard on the first run ([`first_run`]). `lib.rs` calls the entry points
//! here from the app's events; the state they share is in this module, since a later launch can arrive before Tauri
//! has started.

mod autostart;
mod entries;
mod first_run;
mod handlers;
mod instance;
mod links;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::webview::{PageLoadEvent, PageLoadPayload};
use tauri::{AppHandle, Manager, Url, WebviewWindow};

/// The app's identifier, as `identifier` in tauri.conf.json (a test checks they agree). Its config directory, the
/// LaunchAgent label and the handler entry are named by it; it is needed before Tauri reads its config.
pub(crate) const IDENTIFIER: &str = "org.moqui.desktop";

const AUTOSTART_ITEM: &str = "os-autostart";
const DEFAULT_HANDLER_ITEM: &str = "os-default-handler";

struct State {
    /// The node's `/qapps`, once it listens.
    node: Option<Url>,
    /// The last link opened before the node listened.
    pending: Option<Url>,
    /// The node's runtime directory, for its `.env`.
    runtime: Option<PathBuf>,
    /// The single-instance socket of this first instance.
    socket: Option<PathBuf>,
}

static STATE: Mutex<State> = Mutex::new(State {
    node: None,
    pending: None,
    runtime: None,
    socket: None,
});
static APP: OnceLock<AppHandle> = OnceLock::new();
static FIRST_RUN_CHECKED: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    moqui_lib::sync::lock(&STATE)
}

/// The tray items of this module, kept to update them.
struct TrayItems {
    autostart: Option<CheckMenuItem<tauri::DynRuntime>>,
    default_handler: Option<MenuItem<tauri::DynRuntime>>,
}

/// Before Tauri starts: whether this launch runs the app. When the app already runs, this hands it the launch's
/// links (it shows its window and opens them) and returns false: the process should exit.
pub(crate) fn single_instance() -> bool {
    let Some(path) = socket_path() else {
        return true;
    };
    let links = links::from_args(std::env::args().skip(1));
    match instance::claim(&path, &links) {
        instance::Claim::Forwarded => {
            eprintln!("moqui-desktop: MR Moqui already runs; it shows its window");
            false
        }
        instance::Claim::First(listener) => {
            state().socket = Some(listener.serve(launched));
            true
        }
        instance::Claim::Alone => true,
    }
}

/// `instance.sock` in the app config directory, where Tauri's `app_config_dir` is, found without Tauri.
fn socket_path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?);
    let config = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else if cfg!(target_os = "linux") {
        entries::config_home(std::env::var("XDG_CONFIG_HOME").ok().as_deref(), &home)
    } else {
        return None;
    };
    Some(config.join(IDENTIFIER).join("instance.sock"))
}

/// A later launch, with its links.
fn launched(links: Vec<Url>) {
    match APP.get() {
        Some(app) => opened(app, links),
        None => {
            if let Some(link) = links.into_iter().last() {
                state().pending = Some(link);
            }
        }
    }
}

/// In the app's setup: keep the handle, refresh start at login, and open the links of a Linux cold start, which are
/// arguments (macOS sends them as `RunEvent::Opened`).
pub(crate) fn setup(app: &AppHandle) {
    let _ = APP.set(app.clone());
    // a development run must not point the installed app's start at login at itself
    if autostart::SUPPORTED && !tauri::is_dev() {
        autostart::refresh();
    }
    let links = links::from_args(std::env::args().skip(1));
    if !links.is_empty() {
        opened(app, links);
    }
}

/// Links the OS opened with the app (`RunEvent::Opened`, a later launch, a Linux cold start): show the window and go
/// to the node's page for the last one, or keep it until the node listens.
pub(crate) fn opened(app: &AppHandle, links: Vec<Url>) {
    crate::show_main(app);
    let Some(link) = links
        .into_iter()
        .rev()
        .find(|link| links::SCHEMES.contains(&link.scheme()))
    else {
        return;
    };
    let node = {
        let mut state = state();
        if state.node.is_none() {
            state.pending = Some(link);
            return;
        }
        state.node.clone()
    };
    if let Some(page) = node.and_then(|node| links::route(&link, &node, &comms_root())) {
        // a client's: the server's Compose or Dialer in an app window
        if crate::client::active(app) {
            crate::client::open_link(app, &page);
            return;
        }
        if let Some(window) = app.get_webview_window("main") {
            if let Err(err) = window.navigate(page.clone()) {
                tracing::warn!("Could not open {page} for {link}: {err}");
            }
        }
    }
}

/// A client of a server has started: links go to its server's Comms app, in an app window, from now on (one opened
/// before waits for the account's shell).
pub(crate) fn client_started() {
    let pending = {
        let mut state = state();
        state.node = Some(crate::client::app_origin());
        state.pending.take()
    };
    if let (Some(link), Some(app)) = (pending, APP.get()) {
        opened(app, vec![link]);
    }
}

/// The node's runtime, once it is chosen.
pub(crate) fn runtime_chosen(runtime: &Path) {
    state().runtime = Some(runtime.to_path_buf());
}

/// The node listens at `qapps` (its `/qapps`): the page the window goes to, which is the page of a link opened
/// before, else `qapps`.
pub(crate) fn node_started(qapps: Url) -> Url {
    let pending = {
        let mut state = state();
        state.node = Some(qapps.clone());
        state.pending.take()
    };
    pending
        .and_then(|link| links::route(&link, &qapps, &comms_root()))
        .unwrap_or(qapps)
}

/// The Comms app's root: `comms_app_root` as the node in process has it, else as the runtime's `.env` sets it, else
/// BasicComms (which a node with BOSS still serves).
fn comms_root() -> String {
    // a client's: its server's, as its client config says
    if let Some(root) = APP.get().and_then(crate::client::comms_root) {
        return root;
    }
    let own = moqui_lib::env::get_prop_or_env("comms_app_root");
    if !own.trim().is_empty() {
        return own;
    }
    let runtime = state().runtime.clone();
    runtime
        .and_then(|runtime| std::fs::read_to_string(runtime.join(".env")).ok())
        .and_then(|text| {
            moqui_lib::env::parse_dotenv(&text)
                .into_iter()
                .rev()
                .find(|(key, _)| key == "comms_app_root")
                .map(|(_, value)| value)
        })
        .unwrap_or_else(|| links::DEFAULT_COMMS_ROOT.to_string())
}

/// The main window loaded a page: on the first signed-in page of a first run, open the attach wizard.
pub(crate) fn page_loaded(window: &WebviewWindow, payload: PageLoadPayload<'_>) {
    if payload.event() != PageLoadEvent::Finished || FIRST_RUN_CHECKED.load(Ordering::Relaxed) {
        return;
    }
    let (node, runtime) = {
        let state = state();
        (state.node.clone(), state.runtime.clone())
    };
    let Some(node) = node.filter(|node| first_run::signed_in_page(payload.url(), node)) else {
        return;
    };
    if FIRST_RUN_CHECKED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Ok(dir) = window.app_handle().path().app_config_dir() else {
        return;
    };
    let marker = dir.join(first_run::MARKER);
    if marker.exists() {
        return;
    }
    let env_file = runtime
        .and_then(|runtime| std::fs::read_to_string(runtime.join(".env")).ok())
        .unwrap_or_default();
    let pair = first_run::needs_pairing(&env_file, moqui_lib::env::get_prop_or_env);
    // without the marker the wizard would open on every start, so it opens only once the marker is written
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&marker, "")) {
        tracing::warn!("First run: could not write {}: {err}", marker.display());
        return;
    }
    if !pair {
        tracing::info!("First run: the node is paired or takes links; no attach wizard");
        return;
    }
    let url = first_run::wizard_url(&node);
    tracing::info!("First run: opening the attach wizard");
    let window = window.clone();
    // not from inside the load handler
    std::thread::spawn(move || {
        if let Err(err) = window.navigate(url) {
            tracing::warn!("First run: could not open the attach wizard: {err}");
        }
    });
}

/// The app quits: give up the single-instance socket, so a restart's new process is the first.
pub(crate) fn exit() {
    if let Some(path) = state().socket.take() {
        instance::release(&path);
    }
}

/// Add this module's tray items, "Start at login" and "Make default for email and calls", before the menu's last item
/// (Quit), on an OS that has them.
pub(crate) fn add_tray_items(app: &AppHandle, menu: &Menu<tauri::DynRuntime>) -> tauri::Result<()> {
    let autostart = if autostart::SUPPORTED {
        Some(CheckMenuItem::with_id(
            app,
            AUTOSTART_ITEM,
            "Start at login",
            true,
            autostart::enabled(),
            None::<&str>,
        )?)
    } else {
        None
    };
    let default_handler = if handlers::SUPPORTED {
        Some(MenuItem::with_id(
            app,
            DEFAULT_HANDLER_ITEM,
            "Make default for email and calls",
            true,
            None::<&str>,
        )?)
    } else {
        None
    };
    if autostart.is_some() || default_handler.is_some() {
        let separator = PredefinedMenuItem::separator(app)?;
        let mut items: Vec<&dyn tauri::menu::IsMenuItem<_>> = vec![&separator];
        if let Some(item) = &autostart {
            items.push(item);
        }
        if let Some(item) = &default_handler {
            items.push(item);
        }
        let position = menu.items()?.len().saturating_sub(1);
        menu.insert_items(&items, position)?;
    }
    app.manage(TrayItems {
        autostart,
        default_handler,
    });
    Ok(())
}

/// A tray menu event; whether it was one of this module's items.
pub(crate) fn on_menu_event(app: &AppHandle, id: &str) -> bool {
    match id {
        AUTOSTART_ITEM => {
            let on = !autostart::enabled();
            if let Err(err) = autostart::set_enabled(on) {
                tracing::error!("Could not turn start at login {}: {err}", if on { "on" } else { "off" });
            }
            if let Some(items) = app.try_state::<TrayItems>() {
                if let Some(item) = &items.autostart {
                    let _ = item.set_checked(autostart::enabled());
                }
            }
            true
        }
        DEFAULT_HANDLER_ITEM => {
            let app = app.clone();
            // LaunchServices may ask the user, and xdg-mime runs a process: not on the event loop
            std::thread::spawn(move || match handlers::make_default() {
                Ok(()) => {
                    if let Some(items) = app.try_state::<TrayItems>() {
                        if let Some(item) = &items.default_handler {
                            let _ = item.set_text("Default for email and calls: set");
                        }
                    }
                }
                Err(err) => tracing::error!("Could not make MR Moqui the default for email and calls: {err}"),
            });
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
