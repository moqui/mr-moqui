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
//!
//! In the `client` mode (configuration 3, [`client`]) there is no node here: the window is the bundled shell of a
//! client of a remote server.

mod backup;
mod bridge;
mod client;
mod node;
mod os;
mod runtime_dir;
mod splash;
mod updates;
mod wake;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use node::{Event, Node};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// The node of this app, until it quits, and what it was started with, for a restart.
#[derive(Default)]
struct NodeState {
    node: Mutex<Option<Node>>,
    started_with: Mutex<Option<NodeStart>>,
    /// The node's `/qapps` once it listens: the window may go to its origin.
    origin: Arc<OnceLock<Url>>,
    /// A restart is under way; another one waits for it to end.
    restarting: AtomicBool,
}

/// How the node was started: a restart starts it again the same way, on the same port.
#[derive(Clone)]
struct NodeStart {
    mode: node::Mode,
    runtime: PathBuf,
    port: u16,
    /// Beside the listen properties: the backup's (`backup::node_properties`).
    properties: Vec<(&'static str, String)>,
}

/// Run the app. Returns when it quits.
pub fn run() {
    if !os::single_instance() {
        return;
    }
    let context = tauri::generate_context!();
    let builder = updates::setup(
        client::register(bridge::setup(tauri::Builder::default().runtime(cef()))),
        context.config(),
    );
    let app = builder.manage(NodeState::default()).setup(|app| {
        let handle = app.handle().clone();
        let mode = node::mode_of_run(&handle.path().app_config_dir()?);
        if mode == node::Mode::Client {
            install_logging();
            client::start(&handle)?;
            bridge::client_started(&handle);
            tray(&handle, mode)?;
            os::setup(&handle);
            os::client_started();
            return Ok(());
        }
        let origin = Arc::clone(&handle.state::<NodeState>().origin);
        main_window(&handle, &origin)?;
        tray(&handle, mode)?;
        os::setup(&handle);
        start_node(&handle)?;
        backup::refresh_last(&handle);
        wake::watch(&handle);
        Ok(())
    });
    let app = match app.build(context) {
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
        RunEvent::WindowEvent {
            label,
            event: WindowEvent::Destroyed,
            ..
        } if label.starts_with("remote-") => client::window_closed(app, &label),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        RunEvent::Opened { urls } => os::opened(app, urls),
        RunEvent::Exit => {
            os::exit();
            let node = moqui_lib::sync::lock(&app.state::<NodeState>().node).take();
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
    // Where the window opens, known before it does: its first navigation is checked against it.
    splash::init(app.config().build.dev_url.as_ref().filter(|_| tauri::is_dev()));
    let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("MR Moqui")
        .inner_size(1280.0, 860.0)
        .min_inner_size(400.0, 300.0)
        .initialization_script(bridge::SCRIPT)
        .on_download(bridge::on_download)
        .on_page_load(|window, payload| os::page_loaded(&window, payload))
        .on_navigation(move |url| stays_in_app(url, navigation.get()))
        .on_new_window(move |url, _features| {
            if stays_in_app(&url, popup.get()) {
                NewWindowResponse::Allow
            } else {
                NewWindowResponse::Deny
            }
        })
        .build()?;
    if let Ok(opened) = window.url() {
        if matches!(opened.scheme(), "http" | "https") && opened.origin() != splash::base().origin() {
            tracing::warn!(
                "The window opened on {opened}, not on {}; the splash page may not load",
                splash::base()
            );
        }
    }
    Ok(window)
}

/// Whether the window may go to `url`: the app's own pages and the node's origin. Anything else on the web, and
/// `mailto:` and `tel:` links, opens in the system browser or the handler the OS has for it, and the window stays.
fn stays_in_app(url: &Url, node: Option<&Url>) -> bool {
    let external = is_external(url, node, &splash::base());
    if external {
        open_externally(url);
    }
    !external
}

/// Whether `url` leaves the app: a web page on neither the app's own pages (at `app`, a development run's dev server
/// or `tauri.localhost`) nor the node's origin, or a `mailto:`, `tel:` or `sip:` link.
fn is_external(url: &Url, node: Option<&Url>, app: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {
            !matches!(url.host_str(), Some("tauri.localhost"))
                && url.origin() != app.origin()
                && node.is_none_or(|node| url.origin() != node.origin())
        }
        "mailto" | "tel" | "sip" => true,
        _ => false,
    }
}

pub(crate) fn open_externally(url: &Url) {
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    if let Err(err) = std::process::Command::new(opener).arg(url.as_str()).spawn() {
        tracing::warn!("Could not open {url} with {opener}: {err}");
    }
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// The tray icon: Show brings the window back, Restart node stops and starts the node, the node item switches its
/// mode, the client item makes the app a client of a server (or a node again), Quit stops the node and quits. A client
/// has no node items and no backups.
fn tray(app: &AppHandle, mode: node::Mode) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show MR Moqui", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let client_label = if mode == node::Mode::Client {
        "Use the node on this computer"
    } else {
        "Use as a client of a server"
    };
    let client_mode = MenuItem::with_id(app, "client-mode", client_label, true, None::<&str>)?;
    let menu = if mode == node::Mode::Client {
        Menu::with_items(app, &[&show, &client_mode, &quit])?
    } else {
        let restart_node = MenuItem::with_id(app, "restart-node", "Restart node", true, None::<&str>)?;
        let node_mode = node_mode_item(app)?;
        Menu::with_items(app, &[&show, &restart_node, &node_mode, &client_mode, &quit])?
    };
    os::add_tray_items(app, &menu)?;
    updates::add_tray_item(app, &menu)?;
    if mode != node::Mode::Client {
        backup::add_tray_items(app, &menu)?;
    }
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("MR Moqui")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "restart-node" => restart(app),
            "node-mode" => switch_node_mode(app),
            "client-mode" => switch_client_mode(app),
            updates::ITEM => updates::chosen(app),
            "quit" => app.exit(0),
            id => {
                let _ = os::on_menu_event(app, id) || backup::on_menu_event(app, id);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// The tray item that shows where the node runs, in process or as the `moqui` sidecar; choosing it switches.
fn node_mode_item<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<MenuItem<R>> {
    let mode = node::mode_of_run(&app.path().app_config_dir()?);
    MenuItem::with_id(app, "node-mode", mode.label(), true, None::<&str>)
}

/// Keep the other node mode for the next start, and restart the app into it; quitting stops this node first.
fn switch_node_mode(app: &AppHandle) {
    let switched = app
        .path()
        .app_config_dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| node::switch_mode(&dir, node::mode_of_run(&dir)).map_err(|err| err.to_string()));
    match switched {
        Ok(mode) => {
            tracing::info!("Restarting with the node {}", mode.as_str());
            app.request_restart();
        }
        Err(err) => tracing::error!("Could not switch the node mode: {err}"),
    }
}

/// Make the app a client of a server, or from a client a node in process, from the next start; restart into it.
fn switch_client_mode(app: &AppHandle) {
    let switched = app
        .path()
        .app_config_dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| {
            let next = match node::mode_of_run(&dir) {
                node::Mode::Client => node::Mode::InProcess,
                node::Mode::InProcess | node::Mode::Sidecar => node::Mode::Client,
            };
            node::switch_to(&dir, next).map_err(|err| err.to_string())
        });
    match switched {
        Ok(mode) => {
            tracing::info!("Restarting as {}", mode.as_str());
            app.request_restart();
        }
        Err(err) => tracing::error!("Could not switch to or from the client: {err}"),
    }
}

/// The app's log: to the console too in a development run.
fn install_logging() {
    if tauri::is_dev() {
        moqui_host::logging::install();
    } else {
        moqui_host::logging::install_without_console();
    }
}

/// Find the runtime, pick the port and boot the node; the window goes to the node once it listens, or shows why not.
fn start_node(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let paths = app.path();
    let dev = tauri::is_dev();
    let location = runtime_dir::Location::choose(
        std::env::var_os("MOQUI_RUNTIME").map(Into::into),
        dev,
        runtime_dir::find_existing(),
        &paths.resource_dir()?,
        &paths.app_data_dir()?,
    );
    install_logging();
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
    os::runtime_chosen(&runtime);
    let port_file = paths.app_config_dir()?.join("node-port");
    let port = node::choose_port(node::saved_port(&port_file))?;
    node::save_port(&port_file, port);
    let mode = node::mode_of_run(&paths.app_config_dir()?);
    tracing::info!(
        "Starting the node {} on {} at 127.0.0.1:{port}",
        mode.as_str(),
        runtime.display()
    );
    let properties = backup::node_properties(app);
    let start = NodeStart {
        mode,
        runtime,
        port,
        properties,
    };
    *moqui_lib::sync::lock(&app.state::<NodeState>().started_with) = Some(start.clone());
    launch_node(app, start);
    Ok(())
}

/// Start the node as `start` says; the window goes to the node once it listens, or shows why not.
fn launch_node(app: &AppHandle, start: NodeStart) {
    let NodeStart {
        mode,
        runtime,
        port,
        properties,
    } = start;
    let origin = Arc::clone(&app.state::<NodeState>().origin);
    let events_app = app.clone();
    let node = Node::start(mode, runtime, port, properties, move |event| match event {
        Event::Started(address) => {
            let url = format!("http://{address}/qapps");
            match Url::parse(&url) {
                Ok(url) => {
                    let _ = origin.set(url.clone());
                    bridge::node_started(&events_app, &url);
                    let url = os::node_started(url);
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
    });
    // A sidecar binary that is not there is said on the splash page, as other start failures are.
    let node = match node {
        Ok(node) => node,
        Err(err) => {
            start_failed(app, &err.to_string());
            return;
        }
    };
    *moqui_lib::sync::lock(&app.state::<NodeState>().node) = Some(node);
}

/// Restart the node: the tray's Restart node, and the splash page's Restart button. A sidecar is stopped and started
/// again in the same mode on the same port, on a thread of its own since the stop can take a while. A node in process
/// that booted, or tried to, is restarted with the app: the framework is booted once per process. A node that never
/// got as far as starting (no runtime, a server on its port) is looked for again from the start.
fn restart(app: &AppHandle) {
    // a client of a server has no node (the splash page's command is granted to the main window's local pages)
    if client::active(app) {
        return;
    }
    let state = app.state::<NodeState>();
    if state.restarting.swap(true, Ordering::SeqCst) {
        return;
    }
    let started_with = moqui_lib::sync::lock(&state.started_with).clone();
    if started_with
        .as_ref()
        .is_some_and(|start| start.mode == node::Mode::InProcess)
    {
        tracing::info!("Restarting the app to restart the node in process");
        app.request_restart();
        return;
    }
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("moqui-desktop-restart".to_string())
        .spawn(move || {
            tracing::info!("Restarting the node");
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.navigate(splash::url(None));
            }
            let node = moqui_lib::sync::lock(&app.state::<NodeState>().node).take();
            if let Some(node) = node {
                node.stop();
            }
            match started_with {
                Some(start) => launch_node(&app, start),
                None => {
                    if let Err(err) = start_node(&app) {
                        start_failed(&app, &err.to_string());
                    }
                }
            }
            app.state::<NodeState>().restarting.store(false, Ordering::SeqCst);
        });
    if let Err(err) = spawned {
        tracing::error!("Could not restart the node: {err}");
        state.restarting.store(false, Ordering::SeqCst);
    }
}

/// Show `message` on the splash page, going back to it from the node's pages when the node fails after it started.
fn start_failed(app: &AppHandle, message: &str) {
    tracing::error!("The node did not start: {message}");
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.navigate(splash::url(Some(message))) {
            tracing::error!("Could not show the error on the splash page: {err}");
        }
    }
}
