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

//! The desktop app: an MR Moqui node in the app's own process, shown in a Tauri window on Chromium (CEF)
//! (plans/Mobile-Desktop.md, configuration 2, first stage). The window opens on a splash page, then goes to the node's
//! `/qapps` on its loopback port, a browser session on the node's own origin as on the web. Closing the window hides it
//! to the tray; quitting stops the node and its Postgres before the process ends.

mod node;
mod runtime_dir;

use std::sync::{Arc, Mutex, OnceLock};

use node::{Event, Node};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// The node of this app, until it quits.
#[derive(Default)]
struct NodeState(Mutex<Option<Node>>);

/// Run the app. Returns when it quits.
pub fn run() {
    let builder = tauri::Builder::default().runtime(cef());
    let app = builder.manage(NodeState::default()).setup(|app| {
        let handle = app.handle().clone();
        let origin = Arc::new(OnceLock::new());
        main_window(&handle, &origin)?;
        tray(&handle)?;
        start_node(&handle, origin)?;
        Ok(())
    });
    let app = match app.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(err) => {
            eprintln!("moqui-desktop: the app did not start: {err}");
            std::process::exit(1);
        }
    };
    app.run(|app, event| match event {
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            api.prevent_close();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        RunEvent::Exit => {
            let node = moqui_lib::sync::lock(&app.state::<NodeState>().0).take();
            if let Some(node) = node {
                tracing::info!("Quitting: stopping the node");
                node.stop();
            }
        }
        _ => {}
    });
}

/// The Chromium runtime. A development run with `MOQUI_DESKTOP_DEVTOOLS_PORT` set opens the DevTools protocol on that
/// loopback port, so tools can drive and read the window (the repository's CDP clients, `chrome://inspect`). It is
/// never open otherwise: anything that reaches it can read and change every page.
fn cef() -> tauri_runtime_cef::Cef {
    let cef = tauri_runtime_cef::Cef::default();
    let port: Option<u16> = std::env::var("MOQUI_DESKTOP_DEVTOOLS_PORT")
        .ok()
        .and_then(|port| port.trim().parse().ok());
    match port {
        Some(port) if tauri::is_dev() => {
            tracing::warn!("DevTools protocol open on 127.0.0.1:{port} (MOQUI_DESKTOP_DEVTOOLS_PORT)");
            // A run without the port pins this local-state preference off, and Chromium keeps it in the profile; the
            // runtime does not turn it back on by itself.
            cef.remote_debugging(tauri_runtime_cef::RemoteDebugging::Port {
                port,
                allowed_origins: Vec::new(),
            })
            .global_preference("devtools.remote_debugging.allowed", true)
        }
        _ => cef,
    }
}

/// The window, on the splash page until the node answers. Links off the node go to the system browser.
fn main_window(app: &AppHandle, origin: &Arc<OnceLock<Url>>) -> tauri::Result<WebviewWindow> {
    let navigation = Arc::clone(origin);
    let popup = Arc::clone(origin);
    WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("MR Moqui")
        .inner_size(1280.0, 860.0)
        .min_inner_size(400.0, 300.0)
        .on_navigation(move |url| stays_in_app(url, navigation.get()))
        .on_new_window(move |url, _features| {
            if stays_in_app(&url, popup.get()) {
                NewWindowResponse::Allow
            } else {
                NewWindowResponse::Deny
            }
        })
        .build()
}

/// Whether the window may go to `url`: the app's own pages and the node's origin. Anything else on the web, and
/// `mailto:` and `tel:` links, opens in the system browser or the handler the OS has for it, and the window stays.
fn stays_in_app(url: &Url, node: Option<&Url>) -> bool {
    let external = match url.scheme() {
        "http" | "https" => {
            !matches!(url.host_str(), Some("tauri.localhost")) && node.is_none_or(|node| url.origin() != node.origin())
        }
        "mailto" | "tel" | "sip" => true,
        _ => false,
    };
    if external {
        open_externally(url);
    }
    !external
}

fn open_externally(url: &Url) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    if let Err(err) = std::process::Command::new(opener).arg(url.as_str()).spawn() {
        tracing::warn!("Could not open {url} with {opener}: {err}");
    }
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The tray icon: Show brings the window back, Quit stops the node and quits.
fn tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show MR Moqui", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("MR Moqui")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Find the runtime, pick the port and boot the node; the window goes to the node once it listens, or shows why not.
fn start_node(app: &AppHandle, origin: Arc<OnceLock<Url>>) -> Result<(), Box<dyn std::error::Error>> {
    let paths = app.path();
    let dev = tauri::is_dev();
    let location = runtime_dir::Location::choose(
        std::env::var_os("MOQUI_RUNTIME").map(Into::into),
        dev,
        runtime_dir::find_existing(),
        &paths.resource_dir()?,
        &paths.app_data_dir()?,
    );
    if dev {
        moqui_host::logging::install();
    } else {
        moqui_host::logging::install_without_console();
    }
    let runtime = match location.prepare() {
        Ok(runtime) => runtime,
        Err(err) => {
            start_failed(app, &err);
            return Ok(());
        }
    };
    if let Some(port) = node::served_on_runtime_port() {
        start_failed(
            app,
            &format!(
                "A server already answers on port {port}, likely on this runtime ({}). Stop it first: two nodes would \
                 share its database.",
                runtime.display()
            ),
        );
        return Ok(());
    }
    let port_file = paths.app_config_dir()?.join("node-port");
    let port = node::choose_port(node::saved_port(&port_file))?;
    node::save_port(&port_file, port);
    tracing::info!("Starting the node on {} at 127.0.0.1:{port}", runtime.display());
    let events_app = app.clone();
    let node = Node::start(runtime, port, move |event| match event {
        Event::Started(address) => {
            let url = format!("http://{address}/qapps");
            match Url::parse(&url) {
                Ok(url) => {
                    let _ = origin.set(url.clone());
                    if let Some(window) = events_app.get_webview_window("main") {
                        if let Err(err) = window.navigate(url) {
                            start_failed(&events_app, &format!("could not open {address}: {err}"));
                        }
                    }
                }
                Err(err) => start_failed(&events_app, &format!("{url} is not a URL: {err}")),
            }
        }
        Event::Failed(err) => start_failed(&events_app, &err),
        Event::Stopped { restart: true } => events_app.request_restart(),
        Event::Stopped { restart: false } => events_app.exit(0),
    })?;
    *moqui_lib::sync::lock(&app.state::<NodeState>().0) = Some(node);
    Ok(())
}

/// Show `message` on the splash page.
fn start_failed(app: &AppHandle, message: &str) {
    tracing::error!("The node did not start: {message}");
    if let Some(window) = app.get_webview_window("main") {
        let quoted = serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string());
        let _ = window.eval(format!("window.moquiStartFailed && window.moquiStartFailed({quoted})"));
    }
}
