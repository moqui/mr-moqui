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
//! The tray's "Check for updates…" asks the feed: the app reads it itself, over HTTPS and to [`MAX_FEED_BYTES`] at
//! most ([`fetch_feed`]), as the plugin would read one whole whatever its size, and hands it to the plugin
//! (`restore_update`), which takes only a package whose signature binds the version the feed names
//! (plans/Security-Review.md SEC-CLI-118). A newer version is said in a notification, and the item becomes
//! "Install update <version>…": choosing it is the confirmation. The update is downloaded and its signature checked
//! while the node runs; then the node and its Postgres stop, the update replaces the app (and the `moqui` sidecar in
//! it), and the app restarts. The runtime in the app data directory keeps its data; its components and conf are links
//! into the app's resources (`runtime_dir`), so they are the new version's. The plugin's commands are granted to no
//! page.

use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use moqui_lib::sync::lock;
use tauri::menu::{Menu, MenuItem};
use tauri::{AppHandle, Manager, Url};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

/// The tray item's id.
pub(crate) const ITEM: &str = "check-updates";

/// The largest update the app takes: the plugin collects the artifact in memory before it checks the signature, so
/// whoever answers for the feed's host could make the app allocate without end; a bundle with Chromium is a few hundred
/// MB (plans/Security-Review.md SEC-CLI-118).
pub(crate) const MAX_UPDATE_BYTES: u64 = 1 << 30;

/// Whether a download that has `received` bytes, of `announced` in all, is past [`MAX_UPDATE_BYTES`].
pub(crate) fn too_large(received: u64, announced: Option<u64>) -> bool {
    received > MAX_UPDATE_BYTES || announced.is_some_and(|total| total > MAX_UPDATE_BYTES)
}

/// The longest update feed the app reads: a feed is a few KiB of JSON (the version, its notes, and a URL and a
/// signature for each platform), and the plugin would read one whole, whatever its size (SEC-CLI-118).
pub(crate) const MAX_FEED_BYTES: usize = 256 * 1024;

/// Seconds a feed's request may take, its body read included.
const FEED_SECONDS: u64 = 30;

/// The values the plugin puts for the variables of a feed's URL: `{{current_version}}`, `{{target}}` (the OS, as the
/// plugin names it), `{{arch}}` and `{{bundle_type}}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeedVars {
    pub(crate) version: String,
    pub(crate) target: String,
    pub(crate) arch: String,
    pub(crate) bundle: String,
}

impl FeedVars {
    /// This app's: its version, and the target, arch and bundle type the plugin would name.
    fn here(app: &AppHandle) -> Option<Self> {
        let target = tauri_plugin_updater::target()?;
        let (os, arch) = target.split_once('-')?;
        Some(Self {
            version: app.package_info().version.to_string(),
            target: os.to_string(),
            arch: arch.to_string(),
            bundle: bundle_name(tauri::utils::platform::bundle_type()).to_string(),
        })
    }
}

/// The name the plugin gives a bundle type in a feed's URL (`{{bundle_type}}`), `unknown` for none.
pub(crate) fn bundle_name(bundle: Option<tauri::utils::config::BundleType>) -> &'static str {
    use tauri::utils::config::BundleType;
    match bundle {
        Some(BundleType::Deb) => "deb",
        Some(BundleType::Rpm) => "rpm",
        Some(BundleType::AppImage) => "appimage",
        Some(BundleType::Msi) => "msi",
        Some(BundleType::Nsis) => "nsis",
        Some(BundleType::App | BundleType::Dmg) => "app",
        _ => "unknown",
    }
}

/// `endpoint` with the plugin's variables put in, in its path (where the URL parser encodes the braces) and in its
/// query; on HTTPS only.
///
/// # Errors
///
/// Returns why for an endpoint that does not parse or is not on HTTPS.
pub(crate) fn feed_url(endpoint: &str, vars: &FeedVars) -> Result<Url, String> {
    let url = Url::parse(endpoint.trim()).map_err(|err| format!("the update feed's URL does not parse: {err}"))?;
    let version = vars.version.replace('+', "%2B");
    let text = url
        .to_string()
        .replace("%7B%7Bcurrent_version%7D%7D", &version)
        .replace("%7B%7Btarget%7D%7D", &vars.target)
        .replace("%7B%7Barch%7D%7D", &vars.arch)
        .replace("%7B%7Bbundle_type%7D%7D", &vars.bundle)
        .replace("{{current_version}}", &version)
        .replace("{{target}}", &vars.target)
        .replace("{{arch}}", &vars.arch)
        .replace("{{bundle_type}}", &vars.bundle);
    let url = Url::parse(&text).map_err(|err| format!("the update feed's URL does not parse: {err}"))?;
    if url.scheme() != "https" {
        return Err("an update feed is read over HTTPS only".to_string());
    }
    Ok(url)
}

/// What `reader` holds, to its end, refused past `max` bytes (read to one byte more, so no more is held).
///
/// # Errors
///
/// Returns why when it is longer than `max` or does not read.
pub(crate) fn read_capped(reader: impl Read, max: usize) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    reader
        .take(max as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|err| format!("the update feed did not read: {err}"))?;
    if body.len() > max {
        return Err(format!("the update feed is larger than {} KiB", max >> 10));
    }
    Ok(body)
}

/// Ask the feed's `endpoints` in turn, each with `vars` put in, for the release; the first that answers is taken.
/// None: an endpoint answered 204, no update.
///
/// # Errors
///
/// Returns the last endpoint's failure when none answers.
pub(crate) fn fetch_feed(endpoints: &[String], vars: &FeedVars) -> Result<Option<serde_json::Value>, String> {
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(FEED_SECONDS))
        .user_agent(concat!("moqui-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|err| format!("no HTTP client: {err}"))?;
    let mut last = "the app has no update feed".to_string();
    for endpoint in endpoints {
        let answered = feed_url(endpoint, vars).and_then(|url| {
            let response = client
                .get(url)
                .header("Accept", "application/json")
                .send()
                .map_err(|err| err.without_url().to_string())?;
            if response.status() == reqwest::StatusCode::NO_CONTENT {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(format!("the update feed answered {}", response.status()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_FEED_BYTES as u64)
            {
                return Err(format!("the update feed is larger than {} KiB", MAX_FEED_BYTES >> 10));
            }
            let body = read_capped(response, MAX_FEED_BYTES)?;
            serde_json::from_slice(&body)
                .map(Some)
                .map_err(|err| format!("the update feed is not JSON: {err}"))
        });
        match answered {
            Ok(feed) => return Ok(feed),
            Err(err) => {
                tracing::warn!("An update feed did not answer: {err}");
                last = err;
            }
        }
    }
    Err(last)
}

/// The update feed's endpoints in the app's config (`plugins.updater.endpoints`).
fn endpoints(app: &AppHandle) -> Vec<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("endpoints"))
        .and_then(|endpoints| endpoints.as_array())
        .map(|endpoints| {
            endpoints
                .iter()
                .filter_map(|url| url.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Read the feed ([`fetch_feed`]) and hand it to the plugin, which picks this platform's package and keeps the version
/// it names bound to the package's signature (`restore_update`).
async fn check_feed(app: &AppHandle) -> Result<Option<Update>, String> {
    let vars = FeedVars::here(app).ok_or("the updater does not know this platform")?;
    let endpoints = endpoints(app);
    let feed = tauri::async_runtime::spawn_blocking(move || fetch_feed(&endpoints, &vars))
        .await
        .map_err(|err| err.to_string())??;
    let Some(feed) = feed else {
        return Ok(None);
    };
    let updater = app.updater().map_err(|err| err.to_string())?;
    updater.restore_update(feed).map_err(|err| err.to_string())
}

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
        let checked = check_feed(&app).await;
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
                notify(&app, "Could not check for updates", &err);
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
        // past the ceiling (announced, or as it comes), the download is dropped, and its buffer with it
        let over = std::sync::Arc::new(tokio::sync::Notify::new());
        let signal = std::sync::Arc::clone(&over);
        let mut received: u64 = 0;
        let download = update.download(
            move |chunk, announced| {
                received = received.saturating_add(chunk as u64);
                if too_large(received, announced) {
                    signal.notify_one();
                }
            },
            || {},
        );
        let downloaded = tokio::select! {
            downloaded = download => downloaded.map_err(|err| err.to_string()),
            () = over.notified() => Err(format!("the update is larger than {} MB", MAX_UPDATE_BYTES >> 20)),
        };
        let bytes = match downloaded {
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
