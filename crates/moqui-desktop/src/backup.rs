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
//! - **Scheduled.** The node gets `moqui_conf__database_backup__scheduled=true`, which runs the framework's nightly
//!   `backup_EmbeddedDatabase_daily` job (paused in the seed data, plans/archive/PostgresBackup.md) with its retention
//!   ladder, and `database-backup@dir`, the folder. Both are set in the `backups` file in the app's config directory
//!   ([`Settings`]). The folder is by default `MR-Moqui Backups` in the user's Documents, where the user sees it and
//!   their own backup of the computer usually reaches it, and which removing the app's data does not remove; a
//!   development run keeps the framework's `runtime/db/backup`.
//! - **The tray's Backups menu.** Back up now (a `pg_dump` of the live database, `moqui save-db`, which leaves the
//!   node's Postgres running), choose and open the folder, restore from a dump, and the last backup's time. A backup
//!   taken from the menu is named outside the ladder (`moqui-<stamp>-manual.dump`), so the nightly rotation neither
//!   counts nor removes it. Nightly backup turns the scheduled one on and off, which starts the node again.
//! - **Sealed.** Each backup, the nightly one and the tray's, is sealed as it is written to the public key of the
//!   app's backup key (`crate::backup_key`, kept in the OS key store), `<name>.dump.age`: the folder, which the user's
//!   own backup or a cloud may copy, holds no plain copy of the data. The tray's Save backup key… writes the key to a
//!   file, which a restore on another computer needs (plans/archive/Security-Review.md SEC-CLI-114). Where the key cannot be
//!   had, the backups are written as they are, and the user is told.
//! - **Told.** The first start that has the nightly backup on tells the user what it holds, where it goes and to save
//!   the key, and how to turn it off; one told before the backups were sealed is told of the key once. The `backups`
//!   file notes each.
//! - **Restore** stops the node and runs `moqui load-save` (`pg_restore --clean`), the proven restore path, then
//!   starts the node again. It first takes a backup of the data it replaces (`-before-restore`), and does not restore
//!   when that fails. A sealed backup is opened with the key from the store, or one the user picks when that is not the
//!   key it was sealed to; it is checked before anything stops, and handed to `moqui load-save` on its stdin.
//!
//! The sidecar node uses the `moqui` binary for both. The node in process dumps with the drivers' function in the
//! app's process, and restores with the binary when there is one, else the same function.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use jiff::tz::TimeZone;
use jiff::Timestamp;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::node::{self, Mode};
use crate::{NodeStart, NodeState};

/// The settings file in the app's config directory.
pub(crate) const SETTINGS_FILE: &str = "backups";
/// The default folder's name, in the user's Documents.
pub(crate) const DEFAULT_FOLDER: &str = "MR-Moqui Backups";
/// The environment names of the conf's `database-backup` `dir` and `scheduled`, which the node takes as overrides.
const DIR_PROPERTY: &str = "moqui_conf__database_backup__dir";
const SCHEDULED_PROPERTY: &str = "moqui_conf__database_backup__scheduled";
/// The environment name of the conf's `database-backup` `recipient`, the backup key's public key the node's dumps are
/// sealed to (SEC-CLI-114).
const RECIPIENT_PROPERTY: &str = "moqui_conf__database_backup__recipient";
/// The kinds of a backup the app takes, the last part of its name.
pub(crate) const MANUAL: &str = "manual";
pub(crate) const BEFORE_RESTORE: &str = "before-restore";

const NOW_ITEM: &str = "backup-now";
const CHOOSE_ITEM: &str = "backup-choose";
const OPEN_ITEM: &str = "backup-open";
const RESTORE_ITEM: &str = "backup-restore";
const LAST_ITEM: &str = "backup-last";
const NIGHTLY_ITEM: &str = "backup-nightly";
const SAVE_KEY_ITEM: &str = "backup-save-key";

/// What the `backups` file says. A line is `name=value`; `#` starts a comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The folder the user chose; none is the default ([`default_folder`]).
    pub(crate) folder: Option<PathBuf>,
    /// Whether the nightly backup runs; on unless the file says `scheduled=false`.
    pub(crate) scheduled: bool,
    /// Whether the user was told what the nightly backup holds and where it goes ([`NOTICE`]).
    pub(crate) told: bool,
    /// Whether the user was told of the backup key and to save it ([`NOTICE`], [`KEY_NOTICE`]).
    pub(crate) key_told: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folder: None,
            scheduled: true,
            told: false,
            key_told: false,
        }
    }
}

/// What the user is told once, at the first start with the nightly backup on: what a dump holds, where it goes, that
/// it is sealed to the backup key and to save the key, and how to stop it (SEC-CLI-114).
pub(crate) const NOTICE: &str = "Each night MR Moqui backs up all its data (every record, with the accounts' password \
     hashes) to this folder, encrypted with a backup key kept in this computer's key store. To restore on another \
     computer you need that key: save it with Save backup key… in the tray's Backups menu, and keep it apart from the \
     backups. Choose another folder, or turn Nightly backup off, there too.";

/// What a user told of the nightly backup before the backups were sealed is told once of the key.
pub(crate) const KEY_NOTICE: &str = "MR Moqui's backups are now encrypted with a backup key kept in this computer's \
     key store. To restore on another computer you need that key: save it with Save backup key… in the tray's Backups \
     menu, and keep it apart from the backups. Backups taken before stay as they were.";

/// What the user is told when the backups cannot be sealed: there is no backup key to seal them to.
pub(crate) const PLAIN_NOTICE: &str = "MR Moqui's backups are not encrypted: its backup key could not be read or made.";

/// Whether this start tells the user of the nightly backup: it is on, they were not told, and it is not a development
/// run.
pub(crate) fn notice_due(settings: &Settings, dev: bool) -> bool {
    settings.scheduled && !settings.told && !dev
}

/// Whether this start tells the user of the backup key ([`KEY_NOTICE`]): the backups are `sealed`, the user was told of
/// the nightly backup before the backups were and not of the key, and it is not a development run.
pub(crate) fn key_notice_due(settings: &Settings, sealed: bool, dev: bool) -> bool {
    sealed && settings.told && !settings.key_told && !dev
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
                "told" => settings.told = value == "true",
                "key_told" => settings.key_told = value == "true",
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
            "# MR Moqui backups: the folder (empty for the default), whether the nightly backup runs, and whether the \
             user was told of it and of the backup key\nfolder={folder}\nscheduled={}\ntold={}\nkey_told={}\n",
            self.scheduled, self.told, self.key_told
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
/// in place of its tier letter, so its ladder passes over it; `.dump.age` when it is `sealed`.
pub(crate) fn backup_name(stamp: Timestamp, kind: &str, sealed: bool) -> String {
    let age = if sealed { ".age" } else { "" };
    format!("moqui-{}-{kind}.dump{age}", stamp.strftime("%Y%m%dT%H%M%SZ"))
}

/// When the backup named `name` was taken: one of the ladder's dumps (`moqui-<stamp>-d.dump`, or sealed `.dump.age`)
/// or one the app took ([`backup_name`]). Physical backups and anything else are none.
pub(crate) fn backup_stamp(name: &str) -> Option<Timestamp> {
    let name = name.strip_suffix(".age").unwrap_or(name);
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

/// Whether `name` is the dump of another database of the cluster (`moqui_content-<stamp>-<kind>.dump`, which `save-db`
/// writes beside the `moqui` one) or of the search store's, not one to restore from: `load-save` restores the `moqui`
/// database from the dump it is given, and the others from the dumps beside it.
pub(crate) fn not_a_restore_dump(name: &str) -> bool {
    name.starts_with("moqui_") || name.starts_with("search-")
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

/// The arguments of `moqui save-db` (`restore` false) or `moqui load-save` for `runtime`'s cluster and `dump`; with
/// `sealed`, the save's `--recipient` (the backup key's public key) or the restore's `--identity -`, the key itself
/// going on its stdin, never on a command line another process may read.
pub(crate) fn tool_args(restore: bool, runtime: &Path, dump: &Path, sealed: Option<&str>) -> Vec<String> {
    let mut args = vec![
        if restore { "load-save" } else { "save-db" }.to_string(),
        cluster_dir(runtime).display().to_string(),
        dump.display().to_string(),
    ];
    match (restore, sealed) {
        (false, Some(recipient)) => args.extend(["--recipient".to_string(), recipient.to_string()]),
        (true, Some(_)) => args.extend(["--identity".to_string(), "-".to_string()]),
        (_, None) => {}
    }
    args
}

/// The public key the backups are sealed to, the backup key's; none, logged, when the key cannot be had, and the
/// backups are written as they are.
fn sealing_recipient(app: &AppHandle) -> Option<String> {
    match crate::backup_key::app_key(app) {
        Ok(identity) => Some(identity.recipient().to_string()),
        Err(err) => {
            tracing::error!("The backups are not sealed: {err}");
            None
        }
    }
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
    nightly: CheckMenuItem<tauri::DynRuntime>,
    busy: AtomicBool,
    /// The settings of this run, as the node got them; a new folder changes them.
    settings: Mutex<Settings>,
}

/// The node's backup properties, from the settings in the app's config directory.
pub(crate) fn node_properties(app: &AppHandle) -> Vec<(&'static str, String)> {
    let mut settings = app.path().app_config_dir().map(|dir| load(&dir)).unwrap_or_default();
    if !settings.scheduled {
        tracing::info!("The nightly backup is off ({SETTINGS_FILE}: scheduled=false)");
    }
    if notice_due(&settings, tauri::is_dev()) {
        let place = settings
            .folder
            .clone()
            .or_else(|| default_folder_of(app))
            .map(|folder| folder.display().to_string())
            .unwrap_or_default();
        notify(app, "Nightly backups go to", Some(&format!("{place}\n{NOTICE}")));
        settings.told = true;
        settings.key_told = true;
        if let Err(err) = app.path().app_config_dir().map_err(|err| err.to_string()).and_then(|dir| {
            save(&dir, &settings).map_err(|err| err.to_string())
        }) {
            tracing::warn!("{SETTINGS_FILE}: not written, so the nightly backup's note shows again: {err}");
        }
    }
    let recipient = sealing_recipient(app);
    if recipient.is_none() {
        notify(app, "Backups are not encrypted", Some(PLAIN_NOTICE));
    } else if key_notice_due(&settings, true, tauri::is_dev()) {
        notify(app, "Save your backup key", Some(KEY_NOTICE));
        settings.key_told = true;
        if let Err(err) = app.path().app_config_dir().map_err(|err| err.to_string()).and_then(|dir| {
            save(&dir, &settings).map_err(|err| err.to_string())
        }) {
            tracing::warn!("{SETTINGS_FILE}: not written, so the backup key's note shows again: {err}");
        }
    }
    let mut properties = properties(&settings, default_folder_of(app));
    if let Some(recipient) = recipient {
        properties.push((RECIPIENT_PROPERTY, recipient));
    }
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
    let save_key = MenuItem::with_id(app, SAVE_KEY_ITEM, "Save backup key…", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let nightly = CheckMenuItem::with_id(app, NIGHTLY_ITEM, "Nightly backup", true, settings.scheduled, None::<&str>)?;
    let last = MenuItem::with_id(app, LAST_ITEM, "Last backup: none", false, None::<&str>)?;
    let items: [&dyn IsMenuItem<_>; 8] = [&now, &nightly, &choose, &open, &restore, &save_key, &separator, &last];
    let submenu = Submenu::with_items(app, "Backups", true, &items)?;
    let position = menu.items()?.len().saturating_sub(1);
    menu.insert(&submenu, position)?;
    app.manage(Backups {
        last,
        nightly,
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
        NIGHTLY_ITEM => toggle_nightly(app),
        SAVE_KEY_ITEM => save_key(app),
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
    // the folder of whole-database dumps is this user's alone (SEC-TLS-97)
    moqui_lib::private_fs::create_dir_all(&folder)
        .map_err(|err| format!("could not create {}: {err}", folder.display()))?;
    let recipient = sealing_recipient(app);
    let path = folder.join(backup_name(Timestamp::now(), kind, recipient.is_some()));
    let partial = partial_of(&path);
    tracing::info!("Backing up the database to {}", path.display());
    let dumped = match start.mode {
        Mode::Sidecar => run_moqui(
            &node::moqui_binary()?,
            false,
            &start.runtime,
            &partial,
            recipient.as_deref(),
        ),
        Mode::InProcess => moqui_host::db::embedded::save_db_to(
            &start.runtime,
            &cluster_dir(&start.runtime),
            &partial,
            recipient.as_deref(),
        )
        .map_err(|err| err.to_string()),
        // a client has no node and no backups: its tray has no Backups menu
        Mode::Client => Err("a client of a server has no database to back up".to_string()),
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

/// Run `moqui save-db` or `moqui load-save` (`restore`) on `runtime` and `dump`, and wait for it; `key` is the save's
/// recipient or the restore's identity, which goes on its stdin ([`tool_args`]).
fn run_moqui(binary: &Path, restore: bool, runtime: &Path, dump: &Path, key: Option<&str>) -> Result<(), String> {
    use std::io::Write as _;
    let mut child = Command::new(binary)
        .args(tool_args(restore, runtime, dump, key))
        .env("MOQUI_RUNTIME", runtime)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("could not run {}: {err}", binary.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        if let (true, Some(identity)) = (restore, key) {
            stdin
                .write_all(identity.as_bytes())
                .map_err(|err| format!("could not hand moqui the backup key: {err}"))?;
        }
    }
    let output = child
        .wait_with_output()
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
        Mode::Client => {}
    }
    notify(app, "Backups go to", Some(&chosen.display().to_string()));
    refresh_last(app);
}

/// The tray's Nightly backup: the scheduled backup on or off, kept in the `backups` file, and the node started again, as
/// it reads the switch at its start (SEC-CLI-114).
fn toggle_nightly(app: &AppHandle) {
    let Some(backups) = app.try_state::<Backups>() else {
        return;
    };
    let settings = {
        let mut settings = moqui_lib::sync::lock(&backups.settings);
        settings.scheduled = !settings.scheduled;
        settings.told = true;
        settings.clone()
    };
    if let Err(err) = backups.nightly.set_checked(settings.scheduled) {
        tracing::warn!("The Nightly backup item was not updated: {err}");
    }
    let saved = app
        .path()
        .app_config_dir()
        .map_err(|err| err.to_string())
        .and_then(|dir| save(&dir, &settings).map_err(|err| err.to_string()));
    if let Err(err) = saved {
        tracing::error!("Could not keep the nightly backup's switch: {err}");
        notify(app, "The nightly backup is not changed", Some(&err));
        return;
    }
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
    let state = if settings.scheduled { "on" } else { "off" };
    tracing::info!("The nightly backup is {state}");
    notify(app, &format!("Nightly backups are {state}"), Some("The node starts again to take it."));
    crate::restart(app);
}

/// The tray's Open backup folder, in the system's file manager.
fn open_folder(app: &AppHandle) {
    let Some(folder) = folder(app) else {
        notify(app, "No backup folder yet", Some("The node has not started."));
        return;
    };
    if let Err(err) = moqui_lib::private_fs::create_dir_all(&folder) {
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
                .add_filter("Database backup", &["dump", "age"]);
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
            if not_a_restore_dump(&name) {
                notify(
                    &handle,
                    "Restore not started",
                    Some(&format!(
                        "{name} goes with the backup of the same time named moqui-…; choose that one."
                    )),
                );
                return;
            }
            // a sealed backup: the key that opens it, from the store or a file the user picks, before anything stops
            if crate::backup_key::is_sealed(&name) {
                let stored = crate::backup_key::app_key(&handle)
                    .ok()
                    .filter(|identity| crate::backup_key::opens(identity, &dump));
                match stored {
                    Some(identity) => confirm_restore(&handle, dump, name, Some(identity.secret_text())),
                    None => pick_key(&handle, dump, name),
                }
                return;
            }
            confirm_restore(&handle, dump, name, None);
        },
    );
}

/// Ask the user for the key file of the sealed backup `dump` (one this computer's key does not open), then confirm.
fn pick_key(app: &AppHandle, dump: PathBuf, name: String) {
    let handle = app.clone();
    pick(
        app,
        move || {
            rfd::AsyncFileDialog::new()
                .set_title("Choose the backup key of this backup")
                .pick_file()
        },
        move |chosen| {
            let Some(chosen) = chosen else {
                return;
            };
            let key = std::fs::read_to_string(chosen.path())
                .map_err(|err| err.to_string())
                .and_then(|text| crate::backup_key::identity_of(&text));
            match key {
                Ok(identity) if crate::backup_key::opens(&identity, &dump) => {
                    confirm_restore(&handle, dump, name, Some(identity.secret_text()));
                }
                Ok(_) => notify(
                    &handle,
                    "Restore not started",
                    Some("That backup key does not open this backup."),
                ),
                Err(err) => notify(
                    &handle,
                    "Restore not started",
                    Some(&format!("That is not a backup key: {err}")),
                ),
            }
        },
    );
}

/// Confirm the restore of `dump`, opened with `key` when it is sealed, then [`restore`].
fn confirm_restore(app: &AppHandle, dump: PathBuf, name: String, key: Option<String>) {
    let description = format!(
        "All data in MR Moqui is replaced with the backup {name}. A backup of the data now is taken first. The node \
         stops while it restores."
    );
    let confirm_app = app.clone();
    pick(
        app,
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
                exclusive(&confirm_app, "moqui-desktop-restore", move |app| {
                    restore(app, &dump, key.as_deref())
                });
            }
        },
    );
}

/// Back up the data now, stop the node, restore `dump` (opened with `key`, an identity's text, when it is sealed), and
/// start the node again, with the splash page showing.
fn restore(app: &AppHandle, dump: &Path, key: Option<&str>) {
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
        Mode::Sidecar => node::moqui_binary().and_then(|binary| run_moqui(&binary, true, &start.runtime, dump, key)),
        Mode::InProcess => match node::moqui_binary() {
            Ok(binary) => run_moqui(&binary, true, &start.runtime, dump, key),
            Err(_) => moqui_host::db::embedded::load_save_with(&start.runtime, &cluster_dir(&start.runtime), dump, key)
                .map_err(|err| err.to_string()),
        },
        Mode::Client => Err("a client of a server has no database to restore".to_string()),
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
        Mode::Client => state.restarting.store(false, Ordering::SeqCst),
    }
}

/// The tray's Save backup key…: the backup key written as a file where the user chooses, readable by them only; a
/// restore on another computer needs it (SEC-CLI-114).
fn save_key(app: &AppHandle) {
    let identity = match crate::backup_key::app_key(app) {
        Ok(identity) => identity,
        Err(err) => {
            tracing::error!("The backup key could not be had to save: {err}");
            notify(app, "The backup key is not saved", Some(&err));
            return;
        }
    };
    let handle = app.clone();
    let documents = app.path().document_dir().ok().filter(|path| path.is_dir());
    pick(
        app,
        move || {
            let mut dialog = rfd::AsyncFileDialog::new()
                .set_title("Save the backup key")
                .set_file_name("MR-Moqui-backup-key.txt");
            if let Some(documents) = documents {
                dialog = dialog.set_directory(documents);
            }
            dialog.save_file()
        },
        move |chosen| {
            let Some(chosen) = chosen else {
                return;
            };
            match crate::backup_key::save_key_file(&identity, chosen.path()) {
                Ok(()) => notify(
                    &handle,
                    "Backup key saved",
                    Some(
                        "Keep it apart from the backups, somewhere safe: without it they do not restore on another \
                          computer, and whoever has both reads all the data.",
                    ),
                ),
                Err(err) => {
                    tracing::error!("The backup key was not saved: {err}");
                    notify(&handle, "The backup key is not saved", Some(&err));
                }
            }
        },
    );
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
