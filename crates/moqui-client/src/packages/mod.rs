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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use download::{download, download_by, list, PackageServer, ProxyServer, ServerAnswer};
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
    /// OFFLINE, SERVER, CHANGED, PAUSED, GAVE_UP for one the host gave up before it ran); `retry` when a later try may
    /// do (a network cut, a server error), false once the host gave up on it ([`Packages::give_up`]).
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

/// One package's download: its state, its cancel, its pause, and whether a run holds it.
pub(crate) struct Job {
    pub(crate) revision: i64,
    pub(crate) cancel: AtomicBool,
    /// The host stopped the run for now (Android's WorkManager: its network or its time gone): it ends at its next
    /// range with its part kept, to be resumed.
    pub(crate) pause: AtomicBool,
    pub(crate) running: AtomicBool,
    /// Deleted or forgotten while its run still went: kept as a tombstone, so nothing starts on its files, until the
    /// run's [`Packages::release`] takes it out.
    pub(crate) gone: AtomicBool,
    /// Who runs it (Android's WorkManager work id), so a stop of a run that ended pauses no other.
    pub(crate) runner: Mutex<Option<String>>,
    /// The runner the host enqueued for the job ([`Packages::expect_runner`]), or the last that took it: only it may
    /// give the job up, so a stale run never ends a job a new tap queued.
    pub(crate) last_runner: Mutex<Option<String>>,
    /// The bytes the run counts against the account's cap while it downloads, beside what is on disk (its whole
    /// file once it passed the check, [`Packages::reserve`]).
    pub(crate) reserved: AtomicU64,
    pub(crate) status: Mutex<Status>,
}

impl Job {
    fn new(revision: i64) -> Self {
        Self {
            revision,
            cancel: AtomicBool::new(false),
            pause: AtomicBool::new(false),
            running: AtomicBool::new(false),
            gone: AtomicBool::new(false),
            runner: Mutex::new(None),
            last_runner: Mutex::new(None),
            reserved: AtomicU64::new(0),
            status: Mutex::new(Status::Queued { revision }),
        }
    }

    pub(crate) fn set(&self, status: Status) {
        *moqui_lib::sync::lock(&self.status) = status;
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub(crate) fn paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
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

    /// The directory of one package of an account, or none for an id that is not a plain name or is a device name of
    /// Windows (refused on every system, as a member path is: a server's package id is a number).
    pub(crate) fn package_dir(&self, account: &str, package_id: &str) -> Option<PathBuf> {
        let dir = self.account_dir(account)?;
        (crate::files::plain_name(package_id) && !crate::files::windows_reserved(package_id))
            .then(|| dir.join(package_id))
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
            if owner != account || job.gone.load(Ordering::SeqCst) {
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

    /// A download of `revision` of a package asked for: its state, and whether the host is to start a run (it is,
    /// unless a run of that revision holds the job: a job queued and never run is asked again, its worker replaced, a
    /// changed "only on Wi-Fi" with it). Refused for a revision under 1 (the server's start at 1), an account id this
    /// system cannot write, and while a run of the package that is being stopped still holds it (cancelled, deleted,
    /// forgotten, or of another revision, which this stops): never two runs on one package's files.
    ///
    /// # Errors
    ///
    /// Returns why it was not queued.
    pub fn queue(&self, account: &str, package_id: &str, revision: i64) -> Result<(Status, bool), String> {
        if self.package_dir(account, package_id).is_none() || !crate::files::writable_name(account) {
            return Err("Not a package".to_string());
        }
        if revision < 1 {
            return Err("Not a revision of the package".to_string());
        }
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        let key = (account.to_string(), package_id.to_string());
        if let Some(job) = jobs.get(&key) {
            let status = moqui_lib::sync::lock(&job.status).clone();
            let running = job.running.load(Ordering::SeqCst);
            if running && job.revision == revision && !job.cancelled() {
                return Ok((status, false));
            }
            if running {
                // another revision's run stops at its next range, a cancelled one is stopping: ask again then
                job.cancel.store(true, Ordering::SeqCst);
                return Err("The offline map's earlier download is being stopped: try again in a moment".to_string());
            }
        }
        let job = Arc::new(Job::new(revision));
        let status = moqui_lib::sync::lock(&job.status).clone();
        jobs.insert(key, job);
        Ok((status, true))
    }

    /// The job a run of a download takes: the one of `revision` not cancelled, or a new one when there is none; none
    /// while any run holds the package (a second worker, a second task, a cancelled, deleted or other revision's run
    /// still stopping, which this stops when it is another revision's), so two runs never share its files, and none
    /// for a job that is cancelled or of another revision (a cancel stands until a download is asked again, and the
    /// work of an earlier revision does nothing). `runner` names who runs it.
    pub(crate) fn take(
        &self,
        account: &str,
        package_id: &str,
        revision: i64,
        runner: Option<&str>,
    ) -> Option<Arc<Job>> {
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        let key = (account.to_string(), package_id.to_string());
        let job = match jobs.get(&key) {
            Some(job) if job.running.load(Ordering::SeqCst) => {
                if job.revision != revision {
                    job.cancel.store(true, Ordering::SeqCst);
                }
                return None;
            }
            // installed already: a worker replaced before it took its job downloads nothing a second time
            Some(job)
                if job.revision == revision
                    && matches!(*moqui_lib::sync::lock(&job.status), Status::Installed { .. }) =>
            {
                return None;
            }
            Some(job) if job.revision == revision && !job.cancelled() => Arc::clone(job),
            Some(_) => return None,
            None => {
                if !crate::files::writable_name(account) {
                    return None;
                }
                let job = Arc::new(Job::new(revision));
                jobs.insert(key, Arc::clone(&job));
                job
            }
        };
        job.running.store(true, Ordering::SeqCst);
        job.pause.store(false, Ordering::SeqCst);
        job.reserved.store(0, Ordering::SeqCst);
        *moqui_lib::sync::lock(&job.runner) = runner.map(str::to_string);
        *moqui_lib::sync::lock(&job.last_runner) = runner.map(str::to_string);
        Some(job)
    }

    /// The end of a run: its job no longer held, its reservation gone, and the tombstone of a package deleted or
    /// forgotten meanwhile taken out.
    pub(crate) fn release(&self, job: &Arc<Job>) {
        let mut jobs = moqui_lib::sync::lock(&self.jobs);
        job.reserved.store(0, Ordering::SeqCst);
        *moqui_lib::sync::lock(&job.runner) = None;
        job.running.store(false, Ordering::SeqCst);
        if job.gone.load(Ordering::SeqCst) {
            jobs.retain(|_, kept| !Arc::ptr_eq(kept, job));
        }
    }

    /// Whether `size` more bytes for a package keep the account's packages within `max_bytes`, counting what is on
    /// disk of its other packages and what their runs under way still have to download; when they do, the run holds
    /// them as reserved. Under the jobs' lock, so two runs of one account cannot together pass the cap.
    pub(crate) fn reserve(&self, account: &str, package_id: &str, job: &Job, size: u64, max_bytes: u64) -> bool {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let mut used = self.used(account, package_id);
        for ((owner, id), other) in jobs.iter() {
            if owner != account || id == package_id || !other.running.load(Ordering::SeqCst) {
                continue;
            }
            let part = self
                .package_dir(account, id)
                .map(|dir| download::part_len(&dir, other.revision))
                .unwrap_or(0);
            used = used.saturating_add(other.reserved.load(Ordering::SeqCst).saturating_sub(part));
        }
        if size.saturating_add(used) > max_bytes {
            return false;
        }
        job.reserved.store(size, Ordering::SeqCst);
        true
    }

    /// Pause a package's download (the host stopped its run): it ends at its next range, failed to be tried again
    /// (PAUSED), its part kept, so the next run resumes it. With `runner`, only that runner's run: a stop that comes
    /// after its run ended never pauses the next one.
    pub fn pause(&self, account: &str, package_id: &str, runner: Option<&str>) {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let Some(job) = jobs.get(&(account.to_string(), package_id.to_string())) else {
            return;
        };
        let held = job.running.load(Ordering::SeqCst)
            && runner.is_none_or(|runner| moqui_lib::sync::lock(&job.runner).as_deref() == Some(runner));
        if held {
            job.pause.store(true, Ordering::SeqCst);
        }
    }

    /// The host enqueued `runner` (Android's WorkManager work id) for the job of `revision` it just queued: that runner
    /// alone may give the job up, before it took it too (a worker with no network on its last run). A worker the host
    /// replaced is another runner, so it never ends the job a new tap queued.
    pub fn expect_runner(&self, account: &str, package_id: &str, revision: i64, runner: &str) {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        if let Some(job) = jobs.get(&(account.to_string(), package_id.to_string())) {
            if job.revision == revision && !job.running.load(Ordering::SeqCst) {
                *moqui_lib::sync::lock(&job.last_runner) = Some(runner.to_string());
            }
        }
    }

    /// The host gave up on a package's download (Android's worker after its runs): its last failure is told as final
    /// (`retry` false), and a download that never ran as failed, `why` saying what stopped it ("there was no
    /// network"), its part kept for a download asked again. Only the job of `revision` that `runner` was enqueued for
    /// ([`Self::expect_runner`]) or last took, and that no run holds: a stale run never ends a job a new tap queued. The state the
    /// job now has.
    pub fn give_up(&self, account: &str, package_id: &str, revision: i64, runner: &str, why: &str) -> Option<Status> {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let job = jobs.get(&(account.to_string(), package_id.to_string()))?;
        let own = job.revision == revision && moqui_lib::sync::lock(&job.last_runner).as_deref() == Some(runner);
        if own && !job.running.load(Ordering::SeqCst) {
            let mut status = moqui_lib::sync::lock(&job.status);
            match &mut *status {
                Status::Failed { retry, reason, .. } => {
                    if *retry {
                        *retry = false;
                        reason.push_str(": the app stopped trying; download it again");
                    }
                }
                Status::Queued { revision } => {
                    let revision = *revision;
                    *status = Status::failed(
                        revision,
                        "GAVE_UP",
                        format!("The app stopped trying: {why}; download it again"),
                        false,
                    );
                }
                _ => {}
            }
        }
        let status = moqui_lib::sync::lock(&job.status).clone();
        Some(status)
    }

    /// The state of a package's download, if one was asked for since the app started.
    pub fn job_status(&self, account: &str, package_id: &str) -> Option<Status> {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let job = jobs.get(&(account.to_string(), package_id.to_string()))?;
        let status = moqui_lib::sync::lock(&job.status).clone();
        Some(status)
    }

    /// Cancel a package's download: a run stops at its next range and removes its part; with no run under way (a
    /// download that failed or paused, or one of before the app started) its part goes now. An installed revision
    /// stays. Whether a download was under way or had a part here.
    pub fn cancel(&self, account: &str, package_id: &str) -> bool {
        let jobs = moqui_lib::sync::lock(&self.jobs);
        let job = jobs.get(&(account.to_string(), package_id.to_string()));
        let mut was = job.is_some_and(|job| !moqui_lib::sync::lock(&job.status).ended());
        if let Some(job) = job {
            job.cancel.store(true, Ordering::SeqCst);
            if job.running.load(Ordering::SeqCst) {
                return true;
            }
            job.set(Status::Cancelled { revision: job.revision });
        }
        if let Some(dir) = self.package_dir(account, package_id) {
            was |= download::remove_parts(&dir, None) > 0;
            prune(&dir);
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
        let key = (account.to_string(), package_id.to_string());
        if let Some(job) = jobs.get(&key) {
            job.cancel.store(true, Ordering::SeqCst);
            if job.running.load(Ordering::SeqCst) {
                // a tombstone until its run ends: nothing starts on these files meanwhile
                job.gone.store(true, Ordering::SeqCst);
            } else {
                jobs.remove(&key);
            }
        }
        moqui_lib::sync::lock(&self.manifests).retain(|path, _| !path.starts_with(&dir));
        let removed = remove_tree(&dir);
        prune(&dir);
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
            if owner != account {
                return true;
            }
            job.cancel.store(true, Ordering::SeqCst);
            // a run still going keeps a tombstone until it ends: nothing starts on its files meanwhile
            let running = job.running.load(Ordering::SeqCst);
            job.gone.store(running, Ordering::SeqCst);
            running
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
/// (Android too), `GetDiskFreeSpaceExW`'s bytes free to the caller on Windows; none elsewhere, where the check is
/// passed by (logged).
pub fn free_space(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(path).ok()?;
        let block = if stat.f_frsize > 0 { stat.f_frsize } else { stat.f_bsize };
        Some(stat.f_bavail.saturating_mul(block))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut free = 0u64;
        // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call, `free` a valid u64 to write, and the
        // other two outputs may be null (GetDiskFreeSpaceExW's contract).
        let done = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (done != 0).then_some(free)
    }
    #[cfg(not(any(unix, windows)))]
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

/// Remove a package's directory when it is empty, and then its account's when that is (both are named after the
/// account's server): what a cancel or a wipe leaves.
pub(crate) fn prune(package_dir: &Path) {
    // each only when empty; the package's may be gone already (a delete)
    let _ = std::fs::remove_dir(package_dir);
    if let Some(account_dir) = package_dir.parent() {
        let _ = std::fs::remove_dir(account_dir);
    }
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
