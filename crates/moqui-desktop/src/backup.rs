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

//! Backups of the node's database (plans/Mobile-Desktop.md section 4, configuration 1, Backups): the computer is the
//! system of record.
//!
//! - **Scheduled.** The node gets `embedded_db_backup_scheduled=true`, which runs the framework's nightly
//!   `backup_EmbeddedDatabase_daily` job (paused in the seed data, plans/archive/PostgresBackup.md) with its retention
//!   ladder, and `embedded_db_backup_dir`, the folder. Both are set in the `backups` file in the app's config directory
//!   ([`Settings`]). The folder is by default `MR-Moqui Backups` in the user's Documents, where the user sees it and
//!   their own backup of the computer usually reaches it, and which removing the app's data does not remove; a
//!   development run keeps the framework's `runtime/db/backup`.
//! - **The tray's Backups menu.** Back up now (a `pg_dump` of the live database, `moqui save-db`, which leaves the
//!   node's Postgres running), choose and open the folder, restore from a dump, and the last backup's time. A backup
//!   taken from the menu is named outside the ladder (`moqui-<stamp>-manual.dump`), so the nightly rotation neither
//!   counts nor removes it.
//! - **Restore** stops the node and runs `moqui load-save` (`pg_restore --clean`), the proven restore path, then
//!   starts the node again. It first takes a backup of the data it replaces (`-before-restore`), and does not restore
//!   when that fails.
//!
//! The sidecar node uses the `moqui` binary for both. The node in process dumps with the drivers' function in the
//! app's process, and restores with the binary when there is one, else the same function.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use jiff::tz::TimeZone;
use jiff::Timestamp;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::node::{self, Mode};
use crate::{NodeStart, NodeState};

/// The settings file in the app's config directory.
pub(crate) const SETTINGS_FILE: &str = "backups";
/// The default folder's name, in the user's Documents.
pub(crate) const DEFAULT_FOLDER: &str = "MR-Moqui Backups";
/// The framework's properties (moqui-core `service::backup`).
const DIR_PROPERTY: &str = "embedded_db_backup_dir";
const SCHEDULED_PROPERTY: &str = "embedded_db_backup_scheduled";
/// The kinds of a backup the app takes, the last part of its name.
pub(crate) const MANUAL: &str = "manual";
pub(crate) const BEFORE_RESTORE: &str = "before-restore";

const NOW_ITEM: &str = "backup-now";
const CHOOSE_ITEM: &str = "backup-choose";
const OPEN_ITEM: &str = "backup-open";
const RESTORE_ITEM: &str = "backup-restore";
const LAST_ITEM: &str = "backup-last";

/// What the `backups` file says. A line is `name=value`; `#` starts a comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The folder the user chose; none is the default ([`default_folder`]).
    pub(crate) folder: Option<PathBuf>,
    /// Whether the nightly backup runs; on unless the file says `scheduled=false`.
    pub(crate) scheduled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folder: None,
            scheduled: true,
        }
    }
}

impl Settings {
    /// The settings in `text`. An unknown name or a value that is not one is logged and passed over.
    pub(crate) fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                tracing::warn!("{SETTINGS_FILE}: {line:?} is not name=value; it is passed over");
                continue;
            };
            let value = value.trim();
            match name.trim() {
                "folder" => settings.folder = (!value.is_empty()).then(|| PathBuf::from(value)),
                "scheduled" => match value {
                    "true" => settings.scheduled = true,
                    "false" => settings.scheduled = false,
                    _ => tracing::warn!("{SETTINGS_FILE}: scheduled is {value:?}, not true or false"),
                },
                other => tracing::warn!("{SETTINGS_FILE}: {other:?} is not a setting; it is passed over"),
            }
        }
        settings
    }

    /// The file's text for these settings.
    pub(crate) fn to_text(&self) -> String {
        let folder = self
            .folder
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        format!(
            "# MR Moqui backups: the folder (empty for the default) and whether the nightly backup runs\nfolder={folder}\nscheduled={}\n",
            self.scheduled
        )
    }
}

/// The settings in `config_dir`, or the defaults.
pub(crate) fn load(config_dir: &Path) -> Settings {
    std::fs::read_to_string(config_dir.join(SETTINGS_FILE))
        .map(|text| Settings::parse(&text))
        .unwrap_or_default()
}

/// Keep `settings` in `config_dir`.
///
/// # Errors
///
/// Returns an error when the file or its directory cannot be written.
pub(crate) fn save(config_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    std::fs::write(config_dir.join(SETTINGS_FILE), settings.to_text())
}

/// The folder when the user chose none: [`DEFAULT_FOLDER`] in `documents`, except in a development run (`dev`),
/// which keeps the framework's `runtime/db/backup` (none here).
pub(crate) fn default_folder(dev: bool, documents: Option<&Path>) -> Option<PathBuf> {
    if dev {
        return None;
    }
    documents.map(|documents| documents.join(DEFAULT_FOLDER))
}

/// The node's properties for `settings`, with `default` as [`default_folder`] gives it: whether the nightly backup
/// runs, and the folder unless it is the framework's own.
pub(crate) fn properties(settings: &Settings, default: Option<PathBuf>) -> Vec<(&'static str, String)> {
    let mut properties = vec![(SCHEDULED_PROPERTY, settings.scheduled.to_string())];
    if let Some(folder) = settings.folder.clone().or(default) {
        properties.push((DIR_PROPERTY, folder.display().to_string()));
    }
    properties
}

/// The folder the backups go to: the chosen one, else `default`, else the framework's `runtime/db/backup`.
pub(crate) fn folder_of(settings: &Settings, default: Option<PathBuf>, runtime: &Path) -> PathBuf {
    settings
        .folder
        .clone()
        .or(default)
        .unwrap_or_else(|| runtime.join("db").join("backup"))
}

/// The embedded cluster's directory in `runtime`, as the host has it.
pub(crate) fn cluster_dir(runtime: &Path) -> PathBuf {
    runtime.join("db").join("embedded")
}

/// Why `folder` cannot take backups of `runtime`'s database: it is in the cluster directory, or holds it. The
/// framework refuses such a folder too, only later, at the backup.
pub(crate) fn refuse_folder(folder: &Path, runtime: &Path) -> Option<String> {
    let cluster = cluster_dir(runtime);
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (folder_real, cluster_real) = (canonical(folder), canonical(&cluster));
    (folder_real.starts_with(&cluster_real) || cluster_real.starts_with(&folder_real)).then(|| {
        format!(
            "{} cannot hold the backups: the database is in {}",
            folder.display(),
            cluster.display()
        )
    })
}

/// The name of a backup the app takes at `stamp`: `moqui-<UTC stamp>-<kind>.dump`, the framework's stamp with a kind
/// in place of its tier letter, so its ladder passes over it.
pub(crate) fn backup_name(stamp: Timestamp, kind: &str) -> String {
    format!("moqui-{}-{kind}.dump", stamp.strftime("%Y%m%dT%H%M%SZ"))
}

/// When the backup named `name` was taken: one of the ladder's dumps (`moqui-<stamp>-d.dump`) or one the app took
/// ([`backup_name`]). Physical backups and anything else are none.
pub(crate) fn backup_stamp(name: &str) -> Option<Timestamp> {
    let rest = name.strip_prefix("moqui-")?.strip_suffix(".dump")?;
    let (stamp, kind) = rest.split_once('-')?;
    if kind.is_empty() {
        return None;
    }
    jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", stamp)
        .ok()?
        .to_zoned(TimeZone::UTC)
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// The newest backup in `folder`, by its name's stamp.
pub(crate) fn last_backup(folder: &Path) -> Option<Timestamp> {
    std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().and_then(backup_stamp))
        .max()
}

/// The tray's last backup line, with the time in `zone`.
pub(crate) fn last_label(last: Option<Timestamp>, zone: &TimeZone) -> String {
    match last {
        Some(stamp) => format!(
            "Last backup: {}",
            stamp.to_zoned(zone.clone()).strftime("%Y-%m-%d %H:%M")
        ),
        None => "Last backup: none".to_string(),
    }
}

/// The arguments of `moqui save-db` (`restore` false) or `moqui load-save` for `runtime`'s cluster and `dump`.
pub(crate) fn tool_args(restore: bool, runtime: &Path, dump: &Path) -> Vec<String> {
    vec![
        if restore { "load-save" } else { "save-db" }.to_string(),
        cluster_dir(runtime).display().to_string(),
        dump.display().to_string(),
    ]
}

/// The `.partial` file a dump is written to before it gets its name, as the framework's; its nightly run removes one
/// left behind.
pub(crate) fn partial_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

/// The tray's Backups menu and whether a backup or restore runs.
struct Backups {
    last: MenuItem<tauri::DynRuntime>,
    busy: AtomicBool,
    /// The settings of this run, as the node got them; a new folder changes them.
    settings: Mutex<Settings>,
}

/// The node's backup properties, from the settings in the app's config directory.
pub(crate) fn node_properties(app: &AppHandle) -> Vec<(&'static str, String)> {
    let settings = app.path().app_config_dir().map(|dir| load(&dir)).unwrap_or_default();
    if !settings.scheduled {
        tracing::info!("The nightly backup is off ({SETTINGS_FILE}: scheduled=false)");
    }
    let mut properties = properties(&settings, default_folder_of(app));
    // A development run is on this repository's runtime: its job row keeps whatever the developer set, as the
    // property would set it for every later `./moq run` too.
    if tauri::is_dev() {
        properties.retain(|(name, _)| *name != SCHEDULED_PROPERTY);
    }
    properties
}

fn default_folder_of(app: &AppHandle) -> Option<PathBuf> {
    default_folder(tauri::is_dev(), app.path().document_dir().ok().as_deref())
}

/// Add the Backups submenu before the menu's last item (Quit).
pub(crate) fn add_tray_items(app: &AppHandle, menu: &Menu<tauri::DynRuntime>) -> tauri::Result<()> {
    let settings = app.path().app_config_dir().map(|dir| load(&dir)).unwrap_or_default();
    let now = MenuItem::with_id(app, NOW_ITEM, "Back up now", true, None::<&str>)?;
    let choose = MenuItem::with_id(app, CHOOSE_ITEM, "Choose backup folder…", true, None::<&str>)?;
    let open = MenuItem::with_id(app, OPEN_ITEM, "Open backup folder", true, None::<&str>)?;
    let restore = MenuItem::with_id(app, RESTORE_ITEM, "Restore from backup…", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let last = MenuItem::with_id(app, LAST_ITEM, "Last backup: none", false, None::<&str>)?;
    let items: [&dyn IsMenuItem<_>; 6] = [&now, &choose, &open, &restore, &separator, &last];
    let submenu = Submenu::with_items(app, "Backups", true, &items)?;
    let position = menu.items()?.len().saturating_sub(1);
    menu.insert(&submenu, position)?;
    app.manage(Backups {
        last,
        busy: AtomicBool::new(false),
        settings: Mutex::new(settings),
    });
    Ok(())
}

/// A tray menu event; whether it was one of this module's items.
pub(crate) fn on_menu_event(app: &AppHandle, id: &str) -> bool {
    match id {
        NOW_ITEM => back_up_now(app),
        CHOOSE_ITEM => choose_folder(app),
        OPEN_ITEM => open_folder(app),
        RESTORE_ITEM => choose_restore(app),
        _ => return false,
    }
    true
}

/// Show the newest backup's time on the tray: at the start, after a backup, and on the wake watch's ticks
/// (`crate::wake`), which see the nightly one.
pub(crate) fn refresh_last(app: &AppHandle) {
    let Some(backups) = app.try_state::<Backups>() else {
        return;
    };
    let Some(folder) = folder(app) else {
        return;
    };
    let label = last_label(last_backup(&folder), &TimeZone::system());
    if let Err(err) = backups.last.set_text(label) {
        tracing::warn!("Could not show the last backup: {err}");
    }
}

/// The backup folder of this run, once the node's runtime is known.
fn folder(app: &AppHandle) -> Option<PathBuf> {
    let runtime = started_with(app)?.runtime;
    let settings = moqui_lib::sync::lock(&app.try_state::<Backups>()?.settings).clone();
    Some(folder_of(&settings, default_folder_of(app), &runtime))
}

fn started_with(app: &AppHandle) -> Option<NodeStart> {
    moqui_lib::sync::lock(&app.state::<NodeState>().started_with).clone()
}

/// Run `work` on a thread of its own while no other backup or restore runs; else say one does.
fn exclusive(app: &AppHandle, name: &str, work: impl FnOnce(&AppHandle) + Send + 'static) {
    let Some(backups) = app.try_state::<Backups>() else {
        return;
    };
    if backups.busy.swap(true, Ordering::SeqCst) {
        notify(app, "A backup or restore is already running", None);
        return;
    }
    let thread_app = app.clone();
    let spawned = std::thread::Builder::new().name(name.to_string()).spawn(move || {
        work(&thread_app);
        if let Some(backups) = thread_app.try_state::<Backups>() {
            backups.busy.store(false, Ordering::SeqCst);
        }
    });
    if let Err(err) = spawned {
        tracing::error!("Could not start {name}: {err}");
        backups.busy.store(false, Ordering::SeqCst);
    }
}

/// The tray's Back up now.
fn back_up_now(app: &AppHandle) {
    exclusive(app, "moqui-desktop-backup", |app| {
        let Some(start) = started_with(app) else {
            notify(app, "Backup failed", Some("The node has not started."));
            return;
        };
        match take_backup(app, &start, MANUAL) {
            Ok(path) => notify(app, "Backed up", Some(&path.display().to_string())),
            Err(err) => {
                tracing::error!("The backup failed: {err}");
                notify(app, "Backup failed", Some(&err));
            }
        }
        refresh_last(app);
    });
}

/// Dump the database to a new backup of `kind` in the backup folder; the dump's path.
fn take_backup(app: &AppHandle, start: &NodeStart, kind: &str) -> Result<PathBuf, String> {
    let folder = folder(app).ok_or("the backup folder is not known yet")?;
    if let Some(refused) = refuse_folder(&folder, &start.runtime) {
        return Err(refused);
    }
    std::fs::create_dir_all(&folder).map_err(|err| format!("could not create {}: {err}", folder.display()))?;
    let path = folder.join(backup_name(Timestamp::now(), kind));
    let partial = partial_of(&path);
    tracing::info!("Backing up the database to {}", path.display());
    let dumped = match start.mode {
        Mode::Sidecar => run_moqui(&node::moqui_binary()?, false, &start.runtime, &partial),
        Mode::InProcess => moqui_host::db::embedded::save_db(&start.runtime, &cluster_dir(&start.runtime), &partial)
            .map_err(|err| err.to_string()),
    };
    let published = dumped.and_then(|()| {
        std::fs::rename(&partial, &path).map_err(|err| format!("could not name the dump {}: {err}", path.display()))
    });
    if let Err(err) = published {
        let _ = std::fs::remove_file(&partial);
        return Err(err);
    }
    Ok(path)
}

/// Run `moqui save-db` or `moqui load-save` (`restore`) on `runtime` and `dump`, and wait for it.
fn run_moqui(binary: &Path, restore: bool, runtime: &Path, dump: &Path) -> Result<(), String> {
    let output = Command::new(binary)
        .args(tool_args(restore, runtime, dump))
        .env("MOQUI_RUNTIME", runtime)
        .output()
        .map_err(|err| format!("could not run {}: {err}", binary.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last: Vec<&str> = stderr.lines().rev().take(5).collect();
    let said: Vec<&str> = last.into_iter().rev().collect();
    Err(format!("moqui ended ({}): {}", output.status, said.join("\n")))
}

/// The tray's Choose backup folder: keep the folder, and give it to the node.
fn choose_folder(app: &AppHandle) {
    let current = folder(app);
    let handle = app.clone();
    pick(
        app,
        move || {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title("Choose the backup folder")
                .set_can_create_directories(true);
            if let Some(current) = current.filter(|path| path.is_dir()) {
                dialog = dialog.set_directory(current);
            }
            dialog.pick_folder()
        },
        move |chosen| {
            if let Some(chosen) = chosen {
                folder_chosen(&handle, chosen.path().to_path_buf());
            }
        },
    );
}

fn folder_chosen(app: &AppHandle, chosen: PathBuf) {
    let Some(start) = started_with(app) else {
        notify(
            app,
            "The backup folder is not changed",
            Some("The node has not started."),
        );
        return;
    };
    if let Some(refused) = refuse_folder(&chosen, &start.runtime) {
        notify(app, "The backup folder is not changed", Some(&refused));
        return;
    }
    let Some(backups) = app.try_state::<Backups>() else {
        return;
    };
    let settings = {
        let mut settings = moqui_lib::sync::lock(&backups.settings);
        settings.folder = Some(chosen.clone());
        settings.clone()
    };
    let saved = app
        .path()
        .app_config_dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| save(&dir, &settings).map_err(|err| err.to_string()));
    if let Err(err) = saved {
        tracing::error!("Could not keep the backup folder: {err}");
        notify(app, "The backup folder is not kept", Some(&err));
        return;
    }
    tracing::info!("Backups go to {}", chosen.display());
    let properties = properties(&settings, default_folder_of(app));
    {
        let state = app.state::<NodeState>();
        let mut started = moqui_lib::sync::lock(&state.started_with);
        if let Some(start) = started.as_mut() {
            start
                .properties
                .retain(|(name, _)| *name != DIR_PROPERTY && *name != SCHEDULED_PROPERTY);
            start.properties.extend(properties.iter().cloned());
        }
    }
    match start.mode {
        // the backup service reads the property at each run
        Mode::InProcess => moqui_lib::env::set_property(DIR_PROPERTY, chosen.display().to_string()),
        // the sidecar has its properties from its command line: it starts again with the new one
        Mode::Sidecar => crate::restart(app),
    }
    notify(app, "Backups go to", Some(&chosen.display().to_string()));
    refresh_last(app);
}

/// The tray's Open backup folder, in the system's file manager.
fn open_folder(app: &AppHandle) {
    let Some(folder) = folder(app) else {
        notify(app, "No backup folder yet", Some("The node has not started."));
        return;
    };
    if let Err(err) = std::fs::create_dir_all(&folder) {
        tracing::warn!("Could not create {}: {err}", folder.display());
    }
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(target_os = "macos"))]
    let opener = "xdg-open";
    if let Err(err) = Command::new(opener).arg(&folder).spawn() {
        tracing::warn!("Could not open {} with {opener}: {err}", folder.display());
    }
}

/// The tray's Restore from backup: choose a dump, confirm, then [`restore`].
fn choose_restore(app: &AppHandle) {
    let current = folder(app);
    let handle = app.clone();
    pick(
        app,
        move || {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title("Restore from backup")
                .add_filter("Database backup", &["dump"]);
            if let Some(current) = current.filter(|path| path.is_dir()) {
                dialog = dialog.set_directory(current);
            }
            dialog.pick_file()
        },
        move |chosen| {
            let Some(chosen) = chosen else {
                return;
            };
            let dump = chosen.path().to_path_buf();
            let name = dump.file_name().unwrap_or_default().to_string_lossy().to_string();
            let description = format!(
                "All data in MR Moqui is replaced with the backup {name}. A backup of the data now is taken first. \
                 The node stops while it restores."
            );
            let confirm_app = handle.clone();
            pick(
                &handle,
                move || {
                    rfd::AsyncMessageDialog::new()
                        .set_level(rfd::MessageLevel::Warning)
                        .set_title("Restore from backup?")
                        .set_description(description)
                        .set_buttons(rfd::MessageButtons::OkCancelCustom(
                            "Restore".to_string(),
                            "Cancel".to_string(),
                        ))
                        .show()
                },
                move |answer| {
                    // Linux answers Ok for the custom button
                    if matches!(answer, rfd::MessageDialogResult::Ok)
                        || matches!(&answer, rfd::MessageDialogResult::Custom(text) if text == "Restore")
                    {
                        exclusive(&confirm_app, "moqui-desktop-restore", move |app| restore(app, &dump));
                    }
                },
            );
        },
    );
}

/// Back up the data now, stop the node, restore `dump`, and start the node again, with the splash page showing.
fn restore(app: &AppHandle, dump: &Path) {
    let Some(start) = started_with(app) else {
        notify(app, "Restore failed", Some("The node has not started."));
        return;
    };
    let state = app.state::<NodeState>();
    if state.restarting.swap(true, Ordering::SeqCst) {
        notify(
            app,
            "Restore not started",
            Some("The node is restarting; try again once it runs."),
        );
        return;
    }
    let before = match take_backup(app, &start, BEFORE_RESTORE) {
        Ok(path) => path,
        Err(err) => {
            tracing::error!("Restore: the backup of the data now failed, so nothing is restored: {err}");
            notify(
                app,
                "Restore not done",
                Some(&format!("The data now could not be backed up first: {err}")),
            );
            state.restarting.store(false, Ordering::SeqCst);
            return;
        }
    };
    refresh_last(app);
    tracing::info!(
        "Restoring {} (the data before it is in {})",
        dump.display(),
        before.display()
    );
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.navigate(crate::splash::url(None));
    }
    let node = moqui_lib::sync::lock(&state.node).take();
    if let Some(node) = node {
        node.stop();
    }
    let restored = match start.mode {
        Mode::Sidecar => node::moqui_binary().and_then(|binary| run_moqui(&binary, true, &start.runtime, dump)),
        Mode::InProcess => match node::moqui_binary() {
            Ok(binary) => run_moqui(&binary, true, &start.runtime, dump),
            Err(_) => moqui_host::db::embedded::load_save(&start.runtime, &cluster_dir(&start.runtime), dump)
                .map_err(|err| err.to_string()),
        },
    };
    match &restored {
        Ok(()) => {
            tracing::info!("Restored {}", dump.display());
            notify(app, "Restored from backup", Some(&dump.display().to_string()));
        }
        Err(err) => {
            tracing::error!("The restore of {} failed: {err}", dump.display());
            notify(
                app,
                "Restore failed",
                Some(&format!("{err}\n\nThe data before it is in {}", before.display())),
            );
        }
    }
    match start.mode {
        Mode::Sidecar => {
            crate::launch_node(app, start);
            state.restarting.store(false, Ordering::SeqCst);
        }
        // the framework boots once per process: the app starts again
        Mode::InProcess => app.request_restart(),
    }
}

/// Show a dialog that `open` makes, on the main thread as macOS needs, and hand its answer to `then` on a thread of
/// its own.
fn pick<F, T>(app: &AppHandle, open: impl FnOnce() -> F + Send + 'static, then: impl FnOnce(T) + Send + 'static)
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let shown = app.run_on_main_thread(move || {
        let answer = open();
        std::thread::spawn(move || then(tauri::async_runtime::block_on(answer)));
    });
    if let Err(err) = shown {
        tracing::error!("Could not show the dialog: {err}");
    }
}

fn notify(app: &AppHandle, title: &str, body: Option<&str>) {
    let mut builder = app.notification().builder().title(title);
    if let Some(body) = body {
        builder = builder.body(body);
    }
    if let Err(err) = builder.show() {
        tracing::warn!("Could not show the notification {title:?}: {err}");
    }
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
