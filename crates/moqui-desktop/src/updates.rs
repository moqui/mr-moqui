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

//! Updates of the app from the update feed it was built with (`plugins.updater` in the config, which `./moq desktop
//! build` adds when `MOQUI_DESKTOP_UPDATER_PUBKEY` and `MOQUI_DESKTOP_UPDATER_ENDPOINT` are set). A build without a
//! feed has no updater and no tray item: the plugin fails to start on a config with no `pubkey`.
//!
//! The tray's "Check for updates…" asks the feed. A newer version is said in a notification, and the item becomes
//! "Install update <version>…": choosing it is the confirmation. The update is downloaded and its signature checked
//! while the node runs; then the node and its Postgres stop, the update replaces the app (and the `moqui` sidecar in
//! it), and the app restarts. The runtime in the app data directory keeps its data; its components and conf are links
//! into the app's resources (`runtime_dir`), so they are the new version's. The plugin's commands are granted to no
//! page.

use std::sync::Mutex;

use moqui_lib::sync::lock;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

/// The tray item's id.
pub(crate) const ITEM: &str = "check-updates";

/// The app was built with an update feed: the updater plugin is registered.
struct Feed;

/// The tray item and where the update is.
struct Updates {
    item: MenuItem<tauri::DynRuntime>,
    phase: Mutex<Phase>,
    /// The update found, until it is installed or fails.
    pending: Mutex<Option<Update>>,
}

/// Where an update is, as the tray item shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Checking,
    /// This version was found; choosing the item installs it.
    Available(String),
    Installing(String),
}

impl Phase {
    fn text(&self) -> String {
        match self {
            Self::Idle => "Check for updates…".to_string(),
            Self::Checking => "Checking for updates…".to_string(),
            Self::Available(version) => format!("Install update {version}…"),
            Self::Installing(version) => format!("Installing update {version}…"),
        }
    }

    fn enabled(&self) -> bool {
        matches!(self, Self::Idle | Self::Available(_))
    }
}

/// Register the updater plugin when `config` has an update feed.
pub(crate) fn setup(builder: tauri::Builder, config: &tauri::Config) -> tauri::Builder {
    if has_feed(config.plugins.0.get("updater")) {
        builder
            .plugin(tauri_plugin_updater::Builder::new().build())
            .manage(Feed)
    } else {
        builder
    }
}

/// Whether `updater`, the `plugins.updater` config, is a feed the plugin starts on and can check: a public key and
/// at least one endpoint, each on HTTPS (the plugin refuses another scheme in a release build).
fn has_feed(updater: Option<&serde_json::Value>) -> bool {
    let Some(updater) = updater else {
        return false;
    };
    let pubkey = updater.get("pubkey").and_then(|key| key.as_str()).unwrap_or_default();
    let endpoints = updater
        .get("endpoints")
        .and_then(|endpoints| endpoints.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    !pubkey.trim().is_empty()
        && !endpoints.is_empty()
        && endpoints
            .iter()
            .all(|endpoint| endpoint.as_str().is_some_and(|url| url.starts_with("https://")))
}

/// Add "Check for updates…" before the menu's last item (Quit), when the app has an update feed.
pub(crate) fn add_tray_item(app: &AppHandle, menu: &Menu<tauri::DynRuntime>) -> tauri::Result<()> {
    if app.try_state::<Feed>().is_none() {
        return Ok(());
    }
    let phase = Phase::Idle;
    let item = MenuItem::with_id(app, ITEM, phase.text(), phase.enabled(), None::<&str>)?;
    let position = menu.items()?.len().saturating_sub(1);
    menu.insert(&item, position)?;
    app.manage(Updates {
        item,
        phase: Mutex::new(phase),
        pending: Mutex::new(None),
    });
    Ok(())
}

/// The tray item was chosen: check, or install the update found.
pub(crate) fn chosen(app: &AppHandle) {
    let Some(updates) = app.try_state::<Updates>() else {
        return;
    };
    let phase = lock(&updates.phase).clone();
    match phase {
        Phase::Idle => check(app),
        Phase::Available(version) => install(app, version),
        Phase::Checking | Phase::Installing(_) => {}
    }
}

fn set_phase(app: &AppHandle, phase: Phase) {
    let Some(updates) = app.try_state::<Updates>() else {
        return;
    };
    let _ = updates.item.set_text(phase.text());
    let _ = updates.item.set_enabled(phase.enabled());
    *lock(&updates.phase) = phase;
}

fn check(app: &AppHandle) {
    set_phase(app, Phase::Checking);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let checked = match app.updater() {
            Ok(updater) => updater.check().await,
            Err(err) => Err(err),
        };
        match checked {
            Ok(Some(update)) => {
                let version = update.version.clone();
                tracing::info!("Update {version} is available (this is {})", update.current_version);
                if let Some(updates) = app.try_state::<Updates>() {
                    *lock(&updates.pending) = Some(update);
                }
                set_phase(&app, Phase::Available(version.clone()));
                notify(&app, &available_title(&version), &available_body(&version));
            }
            Ok(None) => {
                set_phase(&app, Phase::Idle);
                let version = app.package_info().version.to_string();
                notify(&app, "MR Moqui is up to date", &format!("Version {version}"));
            }
            Err(err) => {
                tracing::error!("Could not check for updates: {err}");
                set_phase(&app, Phase::Idle);
                notify(&app, "Could not check for updates", &err.to_string());
            }
        }
    });
}

/// Download the update and check its signature, then stop the node, install it and restart the app. A failure before
/// the node stops leaves the update to try again; one after starts the node again.
fn install(app: &AppHandle, version: String) {
    let Some(update) = app
        .try_state::<Updates>()
        .and_then(|updates| lock(&updates.pending).take())
    else {
        set_phase(app, Phase::Idle);
        return;
    };
    set_phase(app, Phase::Installing(version.clone()));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tracing::info!("Downloading update {version}");
        let bytes = match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!("Could not download update {version}: {err}");
                if let Some(updates) = app.try_state::<Updates>() {
                    *lock(&updates.pending) = Some(update);
                }
                set_phase(&app, Phase::Available(version.clone()));
                notify(&app, &format!("Could not download update {version}"), &err.to_string());
                return;
            }
        };
        let stopping = app.clone();
        // Stopping the node blocks until it and Postgres have stopped.
        let installed = tauri::async_runtime::spawn_blocking(move || {
            stop_node(&stopping);
            update.install(bytes).map_err(|err| err.to_string())
        })
        .await
        .unwrap_or_else(|err| Err(err.to_string()));
        match installed {
            Ok(()) => {
                tracing::info!("Installed update {version}; restarting");
                app.request_restart();
            }
            Err(err) => {
                tracing::error!("Could not install update {version}: {err}");
                set_phase(&app, Phase::Idle);
                notify(&app, &format!("Could not install update {version}"), &err);
                app.state::<crate::NodeState>()
                    .restarting
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                crate::restart(&app);
            }
        }
    });
}

/// Stop the node for an install, as a restart does, holding off the tray's Restart node until the app restarts.
fn stop_node(app: &AppHandle) {
    let state = app.state::<crate::NodeState>();
    state.restarting.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.navigate(crate::splash::url(None));
    }
    let node = lock(&state.node).take();
    if let Some(node) = node {
        tracing::info!("Updating: stopping the node");
        node.stop();
    }
}

fn available_title(version: &str) -> String {
    format!("MR Moqui {version} is available")
}

fn available_body(version: &str) -> String {
    format!(
        "Choose \"{}\" in the tray menu to install it. The node stops while it installs, then the app restarts.",
        Phase::Available(version.to_string()).text()
    )
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(err) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("Could not show the notification \"{title}\": {err}");
    }
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
