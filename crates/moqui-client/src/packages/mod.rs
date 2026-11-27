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

//! The offline map packages on the device (plans/Maps-Offline.md §2.4, OF2): a region of the user's map built on the
//! server (docs/tools/Maps.md "Offline packages"), downloaded here and served to the shell's offline screen with no
//! network.
//!
//! - **Store.** `data/<account>.packages/<packageId>/<revision>/`, beside the account's store, owner-only as it is
//!   (directories 0700, files 0600), outside `store-max-mb`, with its own cap (`<client-conf packages-max-mb>`, 4096)
//!   and a free-space check before a download starts. A personal device keeps them across sign-outs; a shared one's go
//!   with the account's data ([`Packages::forget`], where the store is forgotten).
//! - **Download** ([`download`]): by ranges of [`RANGE_LEN`], each with `If-Range` the package's tag, written to a part
//!   file and resumed from its length; then the whole file's SHA-256 checked against the tag (`"sha256-…"`); then
//!   unpacked with bounds ([`unzip`]: only the members the manifest lists, each within its size and checked against its
//!   SHA-256, no path outside the directory, no symlink, a total cap); then the revision's directory swapped in by
//!   rename and the previous revision removed after. A failed check keeps the previous revision. A 409 means the
//!   revision is stale: the package list is read again and the new revision offered ([`Status::Stale`]).
//! - **Serving** ([`serve`]): `/__moqui-app/maps/<packageId>/<path>` on the shell's origin, from the package's
//!   directory: HEAD, GET and one `Range` as a 206 slice read from the file, never the whole file in memory.
//!
//! A member is at most [`MEMBER_MAX`] (2 GiB less a byte): Android's WebView reads an intercepted body's length as an
//! Int32 and cannot check a range that ends past it (wry's `RustWebViewClient.kt`, `SkippedPrefixInputStream`), so a
//! package with a larger member is refused when it is unpacked, on every device, rather than served wrong on a phone.
//! The server's `maps@package-max-bytes` (2 GiB) keeps a package under it.
//!
//! The page never has a path of these: the commands answer package ids, names, revisions, sizes and states only.

mod download;
mod serve;
mod unzip;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use download::{download, list, PackageServer, ProxyServer, ServerAnswer};
pub use serve::{answer as serve, Served, PATH_PREFIX};

/// The server's list of the user's packages: the Offline screen's `packageList` transition (`find#Packages`).
pub const LIST_PATH: &str = "/apps/maps/Offline/packageList";
/// The server's package transition, which sends the zip of a revision by range.
pub const PACKAGE_PATH: &str = "/apps/maps/Offline/package";
/// The bytes of one range of a download: a few MiB, so a cut loses little and each request is short.
pub const RANGE_LEN: u64 = 4 * 1024 * 1024;
/// The largest member of a package (Android's WebView's Int32 length; see the module's doc).
pub const MEMBER_MAX: u64 = i32::MAX as u64;
/// What a download leaves free on the device beside the package (its part file and its unpacked copy).
pub const FREE_MARGIN: u64 = 64 * 1024 * 1024;
/// The longest manifest read.
pub(crate) const MANIFEST_MAX: u64 = 16 * 1024 * 1024;

/// A package as the server lists it (`find#Packages`'s entries): what a download needs and what the shell shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PackageEntry {
    pub package_id: String,
    pub name: String,
    pub status_id: String,
    pub status_text: String,
    pub revision: Option<i64>,
    pub size: Option<u64>,
    pub size_text: String,
    pub built_date: Option<i64>,
    pub built_text: String,
    pub zooms_text: String,
    /// The path of the revision's zip on the server, when it has one.
    pub download_url: Option<String>,
    /// The zip's strong tag, `sha256-<hex>`, when it has one.
    pub tag: Option<String>,
}

/// Where a package's download, or its last one, is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    /// Asked for, waiting to start (on Android, for WorkManager's constraints: a network, Wi-Fi when asked).
    Queued { revision: i64 },
    /// Bytes `done` of `total` are here.
    Downloading { revision: i64, done: u64, total: u64 },
    /// The whole file's SHA-256 is read, then its members unpacked and checked.
    Checking { revision: i64 },
    /// Installed: served from `revision`'s directory.
    Installed { revision: i64 },
    /// Stopped: `code` says why (TOO_LARGE, NO_SPACE, CHECKSUM, BAD_PACKAGE, NOT_FOUND, NOT_READY, UNAUTHORIZED,
    /// OFFLINE, SERVER, CHANGED); `retry` when a later try may do (a network cut, a server error).
    Failed {
        revision: i64,
        code: String,
        reason: String,
        retry: bool,
    },
    /// Cancelled here; nothing of the download is kept.
    Cancelled { revision: i64 },
    /// The server built the package again: `revision` is the one to download now, none when it has no file yet.
    Stale { revision: Option<i64> },
}

impl Status {
    /// Whether the download has ended (installed, failed, cancelled or stale).
    pub fn ended(&self) -> bool {
        matches!(
            self,
            Self::Installed { .. } | Self::Failed { .. } | Self::Cancelled { .. } | Self::Stale { .. }
        )
    }

    pub fn failed(revision: i64, code: &str, reason: impl Into<String>, retry: bool) -> Self {
        Self::Failed {
            revision,
            code: code.to_string(),
            reason: reason.into(),
            retry,
        }
    }
}

/// One package's download: its state, its cancel, and whether a run holds it.
pub(crate) struct Job {
    pub(crate) revision: i64,
    pub(crate) cancel: AtomicBool,
    pub(crate) running: AtomicBool,
    pub(crate) status: Mutex<Status>,
}

impl Job {
    fn new(revision: i64) -> Self {
        Self {
            revision,
            cancel: AtomicBool::new(false),
            running: AtomicBool::new(false),
            status: Mutex::new(Status::Queued { revision }),
        }
    }

    pub(crate) fn set(&self, status: Status) {
        *moqui_lib::sync::lock(&self.status) = status;
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// The free bytes of the filesystem a path is on, or none where it cannot be told.
pub type FreeSpace = Box<dyn Fn(&Path) -> Option<u64> + Send + Sync>;

/// The packages of the accounts here: their directories under the client's `data/`, the downloads under way, and the
/// manifests of the installed revisions read so far.
pub struct Packages {
    data: PathBuf,
    jobs: Mutex<HashMap<(String, String), Arc<Job>>>,
    manifests: Mutex<HashMap<PathBuf, Arc<Manifest>>>,
    free_space: FreeSpace,
}

/// A member a manifest lists.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Member {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

/// An installed revision's manifest: its members by path, and what the shell shows of it.
#[derive(Debug)]
pub(crate) struct Manifest {
    pub(crate) members: HashMap<String, Member>,
    pub(crate) package: Value,
}

impl Packages {
    /// The packages in `<dir>/data/`, the free space read from the OS.
    pub fn new(dir: &Path) -> Self {
        Self::with_free_space(dir, Box::new(free_space))
    }

    /// [`Self::new`] with how the free space is read (a test's).
    pub fn with_free_space(dir: &Path, free_space: FreeSpace) -> Self {
        Self {
            data: dir.join("data"),
            jobs: Mutex::new(HashMap::new()),
            manifests: Mutex::new(HashMap::new()),
            free_space,
        }
    }

    /// The directory of an account's packages, or none for an id that could leave `data/`.
    pub(crate) fn account_dir(&self, account: &str) -> Option<PathBuf> {
        crate::files::plain_name(account).then(|| self.data.join(format!("{account}.packages")))
    }

    /// The directory of one package of an account, or none for an id that is not a plain name.
    pub(crate) fn package_dir(&self, account: &str, package_id: &str) -> Option<PathBuf> {
        let dir = self.account_dir(account)?;
        crate::files::plain_name(package_id).then(|| dir.join(package_id))
    }

    /// The installed revision of a package and its directory: the highest whole-number directory with a manifest.
    pub(crate) fn installed_dir(&self, account: &str, package_id: &str) -> Option<(i64, PathBuf)> {
        let dir = self.package_dir(account, package_id)?;
        let entries = std::fs::read_dir(&dir).ok()?;
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let revision: i64 = name.parse().ok().filter(|_| name.bytes().all(|b| b.is_ascii_digit()))?;
                let path = entry.path();
                path.join("manifest.json").is_file().then_some((revision, path))
            })
            .max_by_key(|(revision, _)| *revision)
    }

    /// The manifest of an installed revision's directory, read once.
    pub(crate) fn manifest(&self, revision_dir: &Path) -> Option<Arc<Manifest>> {
        if let Some(manifest) = moqui_lib::sync::lock(&self.manifests).get(revision_dir) {
            return Some(Arc::clone(manifest));
        }
        let text = std::fs::read(revision_dir.join("manifest.json")).ok()?;
        let manifest = Arc::new(parse_manifest(&text).ok()?);
        moqui_lib::sync::lock(&self.manifests).insert(revision_dir.to_path_buf(), Arc::clone(&manifest));
        Some(manifest)
    }

    /// The installed packages of an account and the downloads under way or last ended: `[{packageId, name, revision,
    /// size, builtDate, bbox, minZoom, maxZoom, status}]`, no path.
    pub fn status(&self, account: &str) -> Value {
        let mut list: Vec<Value> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        if let Some(dir) = self.account_dir(account) {
            let mut ids: Vec<String> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| crate::files::plain_name(name))
                .collect();
            ids.sort();
            for id in ids {
                let Some((revision, path)) = self.installed_dir(account, &id) else {
                    continue;
                };
                let package = self
                    .manifest(&path)
                    .map(|manifest| manifest.package.clone())
                    .unwrap_or_default();
                list.push(serde_json::json!({
                    "packageId": id,
                    "name": package["name"],
                    "revision": revision,
                    "size": dir_size(&path),
                    "builtDate": package["builtDate"],
                    "bbox": package["bbox"],
                    "minZoom": package["minZoom"],
                    "maxZoom": package["maxZoom"],
                    "installed": true,
                }));
                seen.push(id);
            }
        }
        let jobs = moqui_lib::sync::lock(&self.jobs);
        for ((owner, id), job) in jobs.iter() {
            if owner != account {
                continue;
            }
            let status = serde_json::to_value(&*moqui_lib::sync::lock(&job.status)).unwrap_or(Value::Null);
            match seen.iter().position(|known| known == id) {
                Some(at) => list[at]["download"] = status,
                None => list.push(serde_json::json!({ "packageId": id, "installed": false, "download": status })),
            }
        }
        Value::Array(list)
    }

    /// A download of `revision` of a package asked for: its job, queued, unless one is under way (whose state is
    /// answered instead). An ended job of the package is replaced.
    pub fn queue(&self, account: &str, package_id: &str, revision: i64) -> Result<Status, String> {
        if self.package_dir(account, package_id).is_none() {
            return Err("Not a package".to_string());
        }
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        let key = (account.to_string(), package_id.to_string());
        if let Some(job) = jobs.get(&key) {
            let status = moqui_lib::sync::lock(&job.status).clone();
            if !status.ended() && job.revision == revision {
                return Ok(status);
            }
            // another revision asked while one runs: that run stops at its next range
            job.cancel.store(true, Ordering::SeqCst);
        }
        let job = Arc::new(Job::new(revision));
        let status = moqui_lib::sync::lock(&job.status).clone();
        jobs.insert(key, job);
        Ok(status)
    }

    /// The job a run of a download takes: the one queued for `revision`, else a new one; none while another run holds
    /// it (a second worker, a second task).
    pub(crate) fn take(&self, account: &str, package_id: &str, revision: i64) -> Option<Arc<Job>> {
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        let key = (account.to_string(), package_id.to_string());
        let job = match jobs.get(&key) {
            Some(job) if job.revision == revision && !job.cancelled() => Arc::clone(job),
            Some(job) if job.running.load(Ordering::SeqCst) => {
                job.cancel.store(true, Ordering::SeqCst);
                let job = Arc::new(Job::new(revision));
                jobs.insert(key, Arc::clone(&job));
                job
            }
            _ => {
                let job = Arc::new(Job::new(revision));
                jobs.insert(key, Arc::clone(&job));
                job
            }
        };
        job.running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then_some(job)
    }

    /// The state of a package's download, if one was asked for since the app started.
    pub fn job_status(&self, account: &str, package_id: &str) -> Option<Status> {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let job = jobs.get(&(account.to_string(), package_id.to_string()))?;
        let status = moqui_lib::sync::lock(&job.status).clone();
        Some(status)
    }

    /// Cancel a package's download: it stops at its next range, and its part file goes. Whether one was under way.
    pub fn cancel(&self, account: &str, package_id: &str) -> bool {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let Some(job) = jobs.get(&(account.to_string(), package_id.to_string())) else {
            return false;
        };
        let was = !moqui_lib::sync::lock(&job.status).ended();
        job.cancel.store(true, Ordering::SeqCst);
        if was && !job.running.load(Ordering::SeqCst) {
            job.set(Status::Cancelled { revision: job.revision });
            if let Some(dir) = self.package_dir(account, package_id) {
                download::remove_parts(&dir, None);
            }
        }
        was
    }

    /// Delete a package from the device: its download cancelled, its revisions and part files removed.
    ///
    /// # Errors
    ///
    /// Returns what could not be removed.
    pub fn delete(&self, account: &str, package_id: &str) -> Result<(), String> {
        let dir = self.package_dir(account, package_id).ok_or("Not a package")?;
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        if let Some(job) = jobs.remove(&(account.to_string(), package_id.to_string())) {
            job.cancel.store(true, Ordering::SeqCst);
        }
        moqui_lib::sync::lock(&self.manifests).retain(|path, _| !path.starts_with(&dir));
        let removed = remove_tree(&dir);
        drop(jobs);
        removed
    }

    /// Forget every package of an account, as its store is forgotten (a shared device's sign-out, a removal): the
    /// downloads cancelled and the directory removed. Logged when a part cannot be removed.
    pub fn forget(&self, account: &str) {
        let Some(dir) = self.account_dir(account) else {
            return;
        };
        // under the jobs' lock, which a download's swap takes too: nothing installs after this
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        jobs.retain(|(owner, _), job| {
            if owner == account {
                job.cancel.store(true, Ordering::SeqCst);
            }
            owner != account
        });
        moqui_lib::sync::lock(&self.manifests).retain(|path, _| !path.starts_with(&dir));
        if let Err(err) = remove_tree(&dir) {
            tracing::error!("The map packages of {account} on this device were not all removed: {err}");
        }
        drop(jobs);
    }

    /// The bytes of an account's installed packages but `except`'s, with their part files.
    pub(crate) fn used(&self, account: &str, except: &str) -> u64 {
        let Some(dir) = self.account_dir(account) else {
            return 0;
        };
        std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy() != except)
            .map(|entry| dir_size(&entry.path()))
            .sum()
    }

    /// The free bytes where an account's packages are kept (the nearest directory there is).
    pub(crate) fn free_at(&self, path: &Path) -> Option<u64> {
        let mut at = path;
        loop {
            if at.exists() {
                return (self.free_space)(at);
            }
            at = at.parent()?;
        }
    }
}

/// The free bytes of the filesystem `path` is on, for an unprivileged writer: `statvfs`'s available blocks on Unix
/// (Android too); none elsewhere, where the check is passed by (logged).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(path).ok()?;
        let block = if stat.f_frsize > 0 { stat.f_frsize } else { stat.f_bsize };
        Some(stat.f_bavail.saturating_mul(block))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// A manifest's members and its package, refused when a member is not a plain path ([`unzip::plain_path`]), a
/// SHA-256 that is not 64 hex digits, a member past [`MEMBER_MAX`], or a path listed twice.
pub(crate) fn parse_manifest(text: &[u8]) -> Result<Manifest, String> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        package: Value,
        #[serde(default)]
        members: Vec<Member>,
    }
    let raw: Raw = serde_json::from_slice(text).map_err(|err| format!("the package's manifest is not one: {err}"))?;
    let mut members = HashMap::new();
    for member in raw.members {
        if !unzip::plain_path(&member.path) || member.path == "manifest.json" {
            return Err("the package's manifest names a path outside it".to_string());
        }
        if member.sha256.len() != 64 || !member.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("the package's manifest has no checksum for {}", member.path));
        }
        if member.size > MEMBER_MAX {
            return Err(format!(
                "{} is larger than this app serves (2 GiB): build the package with fewer zooms",
                member.path
            ));
        }
        let path = member.path.clone();
        if members.insert(path.clone(), member).is_some() {
            return Err(format!("the package's manifest lists {path} twice"));
        }
    }
    Ok(Manifest {
        members,
        package: raw.package,
    })
}

/// The bytes of the files under `path` (or of the file), symlinks not followed.
pub(crate) fn dir_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !meta.is_dir() {
        return meta.len();
    }
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| dir_size(&entry.path()))
        .sum()
}

/// Remove a directory and what is under it; none there is no error.
pub(crate) fn remove_tree(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

#[cfg(test)]
mod tests;
