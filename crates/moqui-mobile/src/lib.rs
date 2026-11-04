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

//! The mobile app (plans/Mobile-Desktop.md MD6, MT1): the client of a remote server, as the desktop app's client mode
//! (MD3 to MD5), on Tauri with the wry runtime (Android's WebView). Its core is `moqui_client`; the commands and the
//! schemes' answer are `moqui_client_tauri`'s, shared with the desktop; what is the phone's is here:
//!
//! - [`host`]: one webview that shows the bundled shell or a page of the server, and the command set checked by the
//!   page as the boundary between them.
//! - [`scheme`]: the shell's files embedded in the app, and what Android's WebView needs of the schemes (no request
//!   bodies, no redirect status).
//! - [`keystore`]: the device keys sealed by the Android Keystore (`KeyVault.kt`), in the app's private files.
//! - [`commands`]: the way back to the shell, a notification, a link outside the app.
//! - The `<client-conf>` of the build (`client-conf.xml`, read at build time), the stores under the app's data
//!   directory, the SSO hand-back (`org.moqui.mobile:/sso`, an intent filter), and the lifecycle: the mirror stops when
//!   the app is paused and starts again when it resumes; the outbox's sender runs while the process does.

#[cfg(target_os = "android")]
mod android;
mod commands;
mod host;
mod keystore;
mod scheme;

use std::borrow::Cow;
use std::sync::Arc;

use moqui_client::app::{Client, ClientHost};
use moqui_client::assets::Assets;
use moqui_client::conf::ClientConf;
use moqui_client_tauri::{ClientState, ShellHost, APP_SCHEME, SHELL_ORIGIN, SHELL_SCHEME};
use tauri::utils::assets::{AssetKey, AssetsIter, CspHash};
use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::host::{goes, Goes, MobileHost, MAIN};

/// The client's settings of this build: `<client-conf>`, read at build time.
const CLIENT_CONF: &str = include_str!("../client-conf.xml");

/// The libraries the shell may load (the desktop's `shell/libs.txt`, staged with it).
const SHELL_LIBS: &str = include_str!("../../moqui-desktop/shell/libs.txt");

/// The page bridge, the one webview's initialization script.
const BRIDGE: &str = include_str!("bridge.js");

/// No assets for Tauri's own origin: the shell's files are the `moqui-shell` scheme's ([`scheme::EmbeddedShell`]).
struct NoAssets;

impl tauri::Assets<tauri::DynRuntime> for NoAssets {
    fn get(&self, _key: &AssetKey) -> Option<Cow<'_, [u8]>> {
        None
    }

    fn iter(&self) -> Box<AssetsIter<'_>> {
        Box::new(std::iter::empty())
    }

    fn csp_hashes(&self, _html_path: &AssetKey) -> Box<dyn Iterator<Item = CspHash<'_>> + '_> {
        Box::new(std::iter::empty())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("moqui-mobile"),
    );
    let mut context = tauri::generate_context!();
    let files = context.set_assets(Box::new(NoAssets));
    let mut files = Some(files);
    let built = tauri::Builder::default()
        .runtime(tauri_runtime_wry::Wry::default())
        .plugin(tauri_plugin_notification::init())
        .register_asynchronous_uri_scheme_protocol(SHELL_SCHEME, scheme::shell)
        .register_asynchronous_uri_scheme_protocol(APP_SCHEME, scheme::app)
        .invoke_handler(tauri::generate_handler![
            moqui_client_tauri::client_state,
            moqui_client_tauri::client_sign_in,
            moqui_client_tauri::client_send_code,
            moqui_client_tauri::client_sign_out,
            moqui_client_tauri::client_switch_account,
            moqui_client_tauri::client_add_account,
            moqui_client_tauri::client_remove_account,
            moqui_client_tauri::client_open_remote,
            moqui_client_tauri::client_take_open,
            moqui_client_tauri::client_sso_providers,
            moqui_client_tauri::client_sso_start,
            moqui_client_tauri::client_sso_cancel,
            moqui_client_tauri::socket_open,
            moqui_client_tauri::socket_send,
            moqui_client_tauri::socket_close,
            moqui_client_tauri::outbox_add,
            moqui_client_tauri::outbox_list,
            moqui_client_tauri::outbox_retry,
            moqui_client_tauri::outbox_discard,
            moqui_client_tauri::outbox_unsent,
            moqui_client_tauri::draft_get,
            moqui_client_tauri::draft_list,
            moqui_client_tauri::draft_put,
            moqui_client_tauri::draft_remove,
            moqui_client_tauri::file_save,
            scheme::scheme_fetch,
            commands::client_show_shell,
            commands::open_in_shell,
            commands::notify,
            commands::open_external
        ])
        .setup(move |app| {
            let embedded = files.take().ok_or("the app was set up twice")?;
            start(app.handle(), embedded)?;
            Ok(())
        })
        .build(context);
    match built {
        Ok(app) => app.run(on_event),
        Err(err) => log::error!("The app did not start: {err}"),
    }
}

/// Start the client: the TLS verifier, the keys, the store under the app's data directory, the shell's webview, and the
/// key renewal and the outbox's sender.
fn start(app: &AppHandle, files: Box<dyn tauri::Assets<tauri::DynRuntime>>) -> Result<(), Box<dyn std::error::Error>> {
    let network = network_ready();
    let dir = app.path().app_data_dir()?.join("client");
    let keys = keystore::open_keys(&dir.join("keys"));
    let conf = ClientConf::parse(client_conf_element());
    let core = Arc::new(Client::new(&dir, keys, Box::new(move || conf.clone()))?);
    let host = Arc::new(MobileHost::new(app.clone(), network));
    *moqui_lib::sync::lock(&host.client) = Some(Arc::clone(&core));
    let client_host: Arc<dyn ClientHost> = host.clone();
    let shell_host: Arc<dyn ShellHost> = host.clone();
    let shell = scheme::EmbeddedShell {
        layout: Assets::embedded(SHELL_LIBS),
        files,
    };
    app.manage(ClientState::new(
        Arc::clone(&core),
        Arc::clone(&client_host),
        shell_host,
        Box::new(shell),
    ));
    app.manage(Arc::clone(&host));
    core.warm();
    shell_webview(app, &host)?;
    if network {
        moqui_client::renewal::start(&core, &client_host);
        moqui_client::sync::flush::start(&core, &client_host);
    }
    log::info!("moqui-mobile: the client started in {}", dir.display());
    Ok(())
}

/// The TLS verifier set up (Android's trust manager), so requests may be made; a failure is logged, and the app runs
/// with what its store keeps, making no request.
fn network_ready() -> bool {
    #[cfg(target_os = "android")]
    if let Err(err) = android::init_verifier() {
        log::error!("The TLS verifier did not start; the app makes no requests: {err}");
        return false;
    }
    true
}

/// The one webview, on the shell.
fn shell_webview(app: &AppHandle, host: &Arc<MobileHost>) -> tauri::Result<()> {
    let navigation_host = Arc::clone(host);
    let load_host = Arc::clone(host);
    let name = serde_json::to_string(&device_name()).unwrap_or_else(|_| "\"\"".to_string());
    let script = format!("window.__moquiDeviceName = {name};\n{BRIDGE}");
    WebviewWindowBuilder::new(
        app,
        MAIN,
        WebviewUrl::External(moqui_client_tauri::shell_url(None, None)),
    )
    .title("MR Moqui")
    .initialization_script(script)
    .on_navigation(move |url| navigation(&navigation_host, url))
    .on_page_load(move |_window, payload| match payload.event() {
        PageLoadEvent::Started => {
            load_host.page.started(payload.url());
            if let Some(core) = moqui_lib::sync::lock(&load_host.client).clone() {
                core.sockets().close_for(MAIN);
            }
        }
        PageLoadEvent::Finished => {
            if payload.url().origin().ascii_serialization() == SHELL_ORIGIN {
                log::info!("moqui-mobile: the shell loaded");
            }
        }
    })
    .build()?;
    Ok(())
}

/// A navigation the page starts (a link, `location`): the shell's page and the server's pages stay in the webview, a
/// server path on the shell's origin goes to the app origin, and the web opens outside.
fn navigation(host: &Arc<MobileHost>, url: &Url) -> bool {
    match goes(url, host.signed_in().is_some()) {
        Goes::Here => {
            host.page.navigating();
            true
        }
        Goes::AppPage => {
            let path = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_string(),
            };
            // not from inside the webview's own callback
            let host = Arc::clone(host);
            std::thread::spawn(move || {
                if let Err(err) = host.open_remote(&path) {
                    log::warn!("Could not open {path}: {err}");
                }
            });
            false
        }
        Goes::Outside => {
            open_externally(url);
            false
        }
        Goes::Nowhere => {
            log::warn!("Navigation refused: {url}");
            false
        }
    }
}

/// The app's events: the SSO hand-back, and the lifecycle.
// on the desktop host, where tests build it, the mobile-only arms are gone and clippy would have the match collapsed
#[cfg_attr(not(mobile), allow(clippy::collapsible_match))]
fn on_event(app: &AppHandle, event: RunEvent) {
    match event {
        RunEvent::Opened { urls } => {
            for url in urls {
                if url.scheme() == "org.moqui.mobile" && url.path() == "/sso" {
                    moqui_client_tauri::commands::sso_returned(app, &url);
                } else {
                    log::warn!("A link the app does not take: {}", url.scheme());
                }
            }
        }
        RunEvent::WindowEvent { label, event, .. } if label == MAIN => match event {
            #[cfg(mobile)]
            WindowEvent::Suspended => paused(app),
            #[cfg(mobile)]
            WindowEvent::Resumed => resumed(app),
            // the activity ended (back out of the app): Tauri ends the process after it, and the next start recovers
            // from the store
            WindowEvent::Destroyed => shut_down(app, "the activity ended"),
            _ => {}
        },
        RunEvent::Exit => shut_down(app, "the app exits"),
        _ => {}
    }
}

/// End the client's threads and sockets; what is unsent stays in the store for the next start.
fn shut_down(app: &AppHandle, why: &str) {
    if let Ok(state) = moqui_client_tauri::state(app) {
        if !state.core.is_stopped() {
            state.core.shutdown();
            log::info!("moqui-mobile: {why}, the client stopped");
        }
    }
}

/// The app went to the background: the mirror stops (a run in progress at its next request); the store, the sockets
/// and the outbox's sender stay while Android keeps the process.
#[cfg_attr(not(mobile), allow(dead_code))]
fn paused(app: &AppHandle) {
    if let Ok(state) = moqui_client_tauri::state(app) {
        state.core.mirror().cancel();
        log::info!("moqui-mobile: paused, the mirror stopped");
    }
}

/// The app came back: the mirror reads ahead again for the shell's account, and the outbox goes.
#[cfg_attr(not(mobile), allow(dead_code))]
fn resumed(app: &AppHandle) {
    let (Ok(state), Some(host)) = (moqui_client_tauri::state(app), app.try_state::<Arc<MobileHost>>()) else {
        return;
    };
    state.core.flusher().wake();
    if let (true, Some(id)) = (host.is_shell(MAIN) && host.network_ready(), host.signed_in()) {
        moqui_client::sync::mirror::start(&state.core, &state.host, &id);
    }
    log::info!("moqui-mobile: resumed");
}

/// Open `url` outside the app (the system browser, a mail or phone app).
pub(crate) fn open_externally(url: &Url) {
    #[cfg(target_os = "android")]
    if let Err(err) = android::open_url(url.as_str()) {
        log::error!("{} was not opened: {err}", url.scheme());
    }
    #[cfg(not(target_os = "android"))]
    log::info!("Not opened outside the app on this platform: {url}");
}

/// The name of this device in the user's device list and of the shell's chat crypto device.
pub(crate) fn device_name() -> String {
    "MR Moqui on Android".to_string()
}

/// The `<client-conf>` element of [`CLIENT_CONF`], past its comment.
fn client_conf_element() -> &'static str {
    CLIENT_CONF
        .find("<client-conf")
        .map_or(CLIENT_CONF, |at| CLIENT_CONF[at..].trim())
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
