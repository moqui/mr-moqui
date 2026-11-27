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

//! A package's download (plans/Maps-Offline.md §2.4, Download): the server's list read for the revision's tag and
//! size, the zip fetched by ranges into a part file and resumed from its length, its SHA-256 checked, its members
//! unpacked and checked, and the revision's directory swapped in. Blocking, on the caller's thread: the desktop's task
//! or Android's `PackageDownloadWorker`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::{parse_manifest, prune, remove_tree, unzip, Job, PackageEntry, Packages, Status, FREE_MARGIN, LIST_PATH};
use super::{MANIFEST_MAX, PACKAGE_PATH, RANGE_LEN};
use crate::account::Account;
use crate::app::{Client, ClientHost};
use crate::proxy::Request;

/// The longest package list read.
const LIST_MAX: u64 = 4 * 1024 * 1024;
/// How long one request of a download may take, its range's bytes included: 4 MiB on a slow link.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A server's answer to a download's request, its body still to read.
pub struct ServerAnswer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

/// The account's server as a download asks it: a GET of a path with headers.
pub trait PackageServer {
    /// GET `path` (with its query) on the server with `headers`.
    ///
    /// # Errors
    ///
    /// Returns the transport's error when the server does not answer.
    fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String>;
}

/// The account's server through the client's proxy, with its key, as the outbox's requests go; a 401 has the host
/// check the key.
pub struct ProxyServer<'a> {
    client: &'a Client,
    account: Account,
    key: Option<String>,
    host: Option<&'a dyn ClientHost>,
}

impl<'a> ProxyServer<'a> {
    /// The server of account `id`, or none for an account not kept here.
    pub fn of(client: &'a Client, id: &str, host: Option<&'a dyn ClientHost>) -> Option<Self> {
        let (account, key) = client.account(id)?;
        Some(Self {
            client,
            account,
            key,
            host,
        })
    }
}

impl PackageServer for ProxyServer<'_> {
    fn get(&self, path: &str, headers: &[(String, String)]) -> Result<ServerAnswer, String> {
        let request = Request {
            method: "GET".to_string(),
            path: path.to_string(),
            headers: headers.to_vec(),
            body: Vec::new(),
        };
        let upstream = self
            .client
            .proxy()
            .send_within(
                &self.account.server,
                self.key.as_deref(),
                crate::SHELL_ORIGIN,
                request,
                Some(REQUEST_TIMEOUT),
            )
            .map_err(|err| err.to_string())?;
        if upstream.status == 401 && self.key.is_some() {
            if let Some(host) = self.host {
                host.check_key(&self.account);
            }
        }
        Ok(ServerAnswer {
            status: upstream.status,
            headers: upstream.headers.clone(),
            body: upstream.into_reader(),
        })
    }
}

/// The user's packages as the server lists them now.
///
/// # Errors
///
/// Returns a failed [`Status`]'s code and reason: UNAUTHORIZED for 401 or 403, OFFLINE when the server is not
/// reached, SERVER for another answer or a list that does not parse.
pub fn list(server: &dyn PackageServer) -> Result<Vec<PackageEntry>, (String, String)> {
    let headers = vec![("Accept".to_string(), "application/json".to_string())];
    let answer = server
        .get(LIST_PATH, &headers)
        .map_err(|err| ("OFFLINE".to_string(), format!("The server could not be reached: {err}")))?;
    match answer.status {
        200 => {}
        401 | 403 => {
            return Err((
                "UNAUTHORIZED".to_string(),
                "Sign in again to download offline maps".to_string(),
            ))
        }
        status => {
            return Err((
                "SERVER".to_string(),
                format!("The server answered {status} for the offline maps"),
            ))
        }
    }
    let mut body = Vec::new();
    answer.body.take(LIST_MAX).read_to_end(&mut body).map_err(|err| {
        (
            "OFFLINE".to_string(),
            format!("The offline maps' list was cut off: {err}"),
        )
    })?;
    let json: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        (
            "SERVER".to_string(),
            "The server's list of offline maps is not one".to_string(),
        )
    })?;
    let entries = json["packageList"].as_array().ok_or_else(|| {
        (
            "SERVER".to_string(),
            "The server's list of offline maps is not one".to_string(),
        )
    })?;
    Ok(entries
        .iter()
        .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
        .collect())
}

/// Download `revision` of package `package_id` of `account` from `server` and install it, its account's packages
/// within `max_bytes`: the state it ended in, which its job keeps for the shell. A run already under way for the
/// package is left to go on, and its state answered.
pub fn download(
    packages: &Packages,
    server: &dyn PackageServer,
    account: &str,
    package_id: &str,
    revision: i64,
    max_bytes: u64,
) -> Status {
    download_by(packages, server, account, package_id, revision, max_bytes, None)
}

/// [`download`] by `runner` (Android's WorkManager work id), whose stop pauses this run only ([`Packages::pause`]).
pub fn download_by(
    packages: &Packages,
    server: &dyn PackageServer,
    account: &str,
    package_id: &str,
    revision: i64,
    max_bytes: u64,
    runner: Option<&str>,
) -> Status {
    if revision < 1 {
        return Status::failed(revision, "NOT_FOUND", "Not a revision of the package", false);
    }
    // no run: the job's own state, a cancel that stands as Cancelled (an end, so the worker is done), a run under way
    // as where it is (not ended, so the worker comes back)
    let Some(job) = packages.take(account, package_id, revision, runner) else {
        return packages
            .job_status(account, package_id)
            .unwrap_or(Status::Queued { revision });
    };
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Run {
            packages,
            server,
            account,
            package_id,
            revision,
            max_bytes,
            job: &job,
        }
        .run()
    }))
    .unwrap_or_else(|_| Status::failed(revision, "SERVER", "The download failed unexpectedly", true));
    // a run cancelled meanwhile (a cancel, a wipe) that then failed on its way (its folder gone) ended cancelled, with
    // nothing of it kept: not a failure to try again
    let ended = match ended {
        Status::Failed { .. } if job.cancelled() => {
            if let Some(dir) = packages.package_dir(account, package_id) {
                remove_parts(&dir, None);
                let _ = remove_tree(&unpack_dir(&dir, revision));
            }
            Status::Cancelled { revision }
        }
        ended => ended,
    };
    match &ended {
        Status::Failed { code, reason, .. } => {
            tracing::warn!("The offline map {package_id} of {account} was not downloaded ({code}): {reason}");
        }
        Status::Installed { .. } => tracing::info!("The offline map {package_id} of {account} is installed"),
        _ => {}
    }
    job.set(ended.clone());
    packages.release(&job);
    // a package's folder left empty (a refusal, a cancel, a wipe meanwhile) goes, and its account's with it
    if let Some(dir) = packages.package_dir(account, package_id) {
        prune(&dir);
    }
    ended
}

/// One run of a download.
struct Run<'a> {
    packages: &'a Packages,
    server: &'a dyn PackageServer,
    account: &'a str,
    package_id: &'a str,
    revision: i64,
    max_bytes: u64,
    job: &'a Arc<Job>,
}

impl Run<'_> {
    fn failed(&self, code: &str, reason: impl Into<String>, retry: bool) -> Status {
        Status::failed(self.revision, code, reason, retry)
    }

    fn run(&self) -> Status {
        let Some(dir) = self.packages.package_dir(self.account, self.package_id) else {
            return self.failed("NOT_FOUND", "Not a package", false);
        };
        if self.job.cancelled() {
            return self.cancelled(&dir);
        }
        let entry = match self.entry() {
            Ok(entry) => entry,
            Err(status) => return status,
        };
        let (Some(tag), Some(size)) = (entry.tag.clone(), entry.size) else {
            return self.failed("NOT_READY", "The offline map has no file to download yet", false);
        };
        let Some(sha) = tag
            .strip_prefix("sha256-")
            .filter(|hex| is_sha256(hex))
            .map(str::to_ascii_lowercase)
        else {
            return self.failed(
                "BAD_PACKAGE",
                "The server's tag of the offline map is not a SHA-256",
                false,
            );
        };
        if !self
            .packages
            .reserve(self.account, self.package_id, self.job, size, self.max_bytes)
        {
            return self.failed(
                "TOO_LARGE",
                format!(
                    "The offline maps on this device would pass the {} MB they may take: delete one first",
                    self.max_bytes / (1024 * 1024)
                ),
                false,
            );
        }
        if let Err(err) = crate::files::create_private_dir(&dir) {
            return self.failed(
                "NO_SPACE",
                format!("The offline map's folder could not be made: {err}"),
                false,
            );
        }
        let part = part_path(&dir, self.revision);
        let mut have = match self.start_part(&dir, &part, &tag, size) {
            Ok(have) => have,
            Err(err) => return self.failed("NO_SPACE", err, false),
        };
        // the part's rest and the unpacked copy beside the previous revision, with room to spare
        let need = (size - have).saturating_add(size).saturating_add(FREE_MARGIN);
        match self.packages.free_at(&dir) {
            Some(free) if free < need => {
                return self.failed(
                    "NO_SPACE",
                    format!(
                        "This device has {} MB free, and the offline map needs {} MB: free some space first",
                        free / (1024 * 1024),
                        need.div_ceil(1024 * 1024)
                    ),
                    false,
                );
            }
            Some(_) => {}
            None => tracing::info!("The free space for an offline map is not known here; the download goes on"),
        }
        let path = entry
            .download_url
            .clone()
            .filter(|url| url.starts_with('/') && !url.starts_with("//"))
            .unwrap_or_else(|| {
                format!(
                    "{PACKAGE_PATH}?packageId={}&revision={}",
                    url::form_urlencoded::byte_serialize(self.package_id.as_bytes()).collect::<String>(),
                    self.revision
                )
            });
        let mut restarted = false;
        while have < size {
            if self.job.cancelled() {
                return self.cancelled(&dir);
            }
            if self.job.paused() {
                return self.failed("PAUSED", "The download goes on later", true);
            }
            self.job.set(Status::Downloading {
                revision: self.revision,
                done: have,
                total: size,
            });
            match self.fetch(&path, &part, &tag, have, size, &mut restarted) {
                Ok(now) => have = now,
                Err(status) => return status,
            }
        }
        if self.job.cancelled() {
            return self.cancelled(&dir);
        }
        self.job.set(Status::Checking {
            revision: self.revision,
        });
        match file_sha256(&part) {
            Ok(found) if found == sha => {}
            Ok(_) => {
                remove_parts(&dir, None);
                return self.failed(
                    "CHECKSUM",
                    "The downloaded offline map does not match the server's: the one on this device is kept",
                    false,
                );
            }
            Err(err) => {
                return self.failed(
                    "NO_SPACE",
                    format!("The downloaded file could not be read: {err}"),
                    true,
                )
            }
        }
        if self.job.cancelled() {
            return self.cancelled(&dir);
        }
        let unpacked = unpack_dir(&dir, self.revision);
        if let Err(err) = unpack(
            &part,
            &unpacked,
            self.package_id,
            self.revision,
            self.max_bytes,
            self.job,
        ) {
            let _ = remove_tree(&unpacked);
            if self.job.cancelled() {
                return self.cancelled(&dir);
            }
            remove_parts(&dir, None);
            return self.failed("BAD_PACKAGE", format!("{err}: the one on this device is kept"), false);
        }
        self.swap(&dir, &unpacked)
    }

    /// The package's entry in the server's list now: its revision is this run's, else the run is stale.
    fn entry(&self) -> Result<PackageEntry, Status> {
        let entries = list(self.server).map_err(|(code, reason)| {
            let retry = code == "OFFLINE" || code == "SERVER";
            self.failed(&code, reason, retry)
        })?;
        let Some(entry) = entries.into_iter().find(|entry| entry.package_id == self.package_id) else {
            return Err(self.failed("NOT_FOUND", "The offline map is no longer on the server", false));
        };
        if entry.revision != Some(self.revision) {
            if let Some(dir) = self.packages.package_dir(self.account, self.package_id) {
                remove_parts(&dir, None);
            }
            return Err(Status::Stale {
                revision: entry.revision.filter(|_| entry.tag.is_some()),
            });
        }
        Ok(entry)
    }

    /// The part file's length to go on from: kept when it is of `tag` and no longer than `size`, else started over.
    /// Other revisions' part files go.
    fn start_part(&self, dir: &Path, part: &Path, tag: &str, size: u64) -> Result<u64, String> {
        remove_parts(dir, Some(self.revision));
        let tag_file = part.with_extension("tag");
        let kept = std::fs::read_to_string(&tag_file).ok();
        let have = std::fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
        if kept.as_deref() == Some(tag) && have <= size {
            return Ok(have);
        }
        let _ = std::fs::remove_file(part);
        crate::files::write_private(&tag_file, tag.as_bytes()).map_err(|err| format!("The download's tag: {err}"))?;
        Ok(0)
    }

    /// One range from `have`, appended to the part file: the part's length after it.
    fn fetch(
        &self,
        path: &str,
        part: &Path,
        tag: &str,
        have: u64,
        size: u64,
        restarted: &mut bool,
    ) -> Result<u64, Status> {
        let last = (have + RANGE_LEN).min(size) - 1;
        let headers = vec![
            ("Accept".to_string(), "application/zip, */*".to_string()),
            ("Range".to_string(), format!("bytes={have}-{last}")),
            ("If-Range".to_string(), format!("\"{tag}\"")),
        ];
        let answer = self
            .server
            .get(path, &headers)
            .map_err(|err| self.failed("OFFLINE", format!("The server could not be reached: {err}"), true))?;
        let header = |name: &str| crate::proxy::header(&answer.headers, name).map(str::trim);
        let dir = part.parent().unwrap_or(part);
        match answer.status {
            206 => {
                let Some((start, end, total)) = header("content-range").and_then(content_range) else {
                    return Err(self.failed("SERVER", "The server's range has no Content-Range", true));
                };
                if start != have || total != size || end < start || end >= size {
                    return Err(self.failed("SERVER", "The server sent another range than the one asked", true));
                }
                let written = append(part, answer.body, end - start + 1)
                    .map_err(|err| self.failed("OFFLINE", format!("The download was cut off: {err}"), true))?;
                if written == 0 {
                    return Err(self.failed("SERVER", "The server's range was empty", true));
                }
                Ok(have + written)
            }
            200 => {
                // a whole answer: the If-Range did not match (the file is not the listed one), or the server ignored
                // the range
                if header("etag").is_some_and(|etag| etag != format!("\"{tag}\"")) {
                    remove_parts(dir, None);
                    return Err(match self.entry() {
                        Err(status) => status,
                        Ok(_) => self.failed(
                            "CHANGED",
                            "The offline map changed on the server: download it again",
                            true,
                        ),
                    });
                }
                let _ = std::fs::remove_file(part);
                let written = append(part, answer.body, size)
                    .map_err(|err| self.failed("OFFLINE", format!("The download was cut off: {err}"), true))?;
                if written < size {
                    return Err(self.failed("SERVER", "The server's whole answer was cut short", true));
                }
                Ok(written)
            }
            409 => {
                remove_parts(dir, None);
                Err(match self.entry() {
                    Err(status) => status,
                    // the list still names this revision: it is being replaced, so try again later
                    Ok(_) => self.failed("CHANGED", "The offline map was built again: download it again", true),
                })
            }
            404 => {
                remove_parts(dir, None);
                Err(self.failed("NOT_FOUND", "The offline map is no longer on the server", false))
            }
            401 | 403 => Err(self.failed("UNAUTHORIZED", "Sign in again to download offline maps", false)),
            416 if !*restarted => {
                *restarted = true;
                let _ = std::fs::remove_file(part);
                Ok(0)
            }
            status @ (408 | 429 | 500..=599) => Err(self.failed(
                "SERVER",
                format!("The server answered {status}; the download goes on later"),
                true,
            )),
            status => Err(self.failed("SERVER", format!("The server answered {status}"), false)),
        }
    }

    /// The unpacked revision put in place by rename, under the jobs' lock (which [`Packages::forget`] takes, so
    /// nothing is installed for an account being forgotten), then the earlier revisions and the part file removed. A
    /// directory of the same revision already there (installed again) is renamed aside first and put back when the
    /// rename fails, so a failure keeps what was installed.
    fn swap(&self, dir: &Path, unpacked: &Path) -> Status {
        let target = dir.join(self.revision.to_string());
        let aside = dir.join(format!(".{}.old", self.revision));
        let jobs = moqui_lib::sync::lock(&self.packages.jobs);
        if self.job.cancelled() {
            drop(jobs);
            let _ = remove_tree(unpacked);
            return self.cancelled(dir);
        }
        let renamed = put_in_place(unpacked, &target, &aside);
        drop(jobs);
        if let Err(err) = renamed {
            let _ = remove_tree(unpacked);
            return self.failed(
                "NO_SPACE",
                format!("The offline map could not be put in place: {err}"),
                true,
            );
        }
        moqui_lib::sync::lock(&self.packages.manifests).retain(|path, _| !path.starts_with(dir));
        for entry in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            // an earlier revision, and any revision's copy left aside (`.K.old`), which would count against the cap
            let digits = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
            let aside = name
                .strip_prefix('.')
                .and_then(|rest| rest.strip_suffix(".old"))
                .is_some_and(digits);
            let earlier = (name != self.revision.to_string() && digits(&name)) || aside;
            if earlier {
                if let Err(err) = remove_tree(&entry.path()) {
                    tracing::error!(
                        "An earlier revision of the offline map {} was not removed: {err}",
                        self.package_id
                    );
                }
            }
        }
        remove_parts(dir, None);
        Status::Installed {
            revision: self.revision,
        }
    }

    /// The run cancelled: what it downloaded goes, and an account's directory left empty by a forget too.
    fn cancelled(&self, dir: &Path) -> Status {
        remove_parts(dir, None);
        let _ = remove_tree(&unpack_dir(dir, self.revision));
        // removed only when empty: a package or an account forgotten meanwhile
        prune(dir);
        Status::Cancelled {
            revision: self.revision,
        }
    }
}

/// Put `unpacked` in place at `target`, a copy of `target` there before set aside at `aside` first and removed once
/// the new one is in place, or put back when the rename fails, so a failure keeps what was installed. A copy a run
/// before left only at `aside` (its renames both failed) is put back first, never removed for nothing.
pub(crate) fn put_in_place(unpacked: &Path, target: &Path, aside: &Path) -> std::io::Result<()> {
    if !target.exists() && aside.exists() {
        std::fs::rename(aside, target)?;
    }
    let _ = remove_tree(aside);
    let moved = target.exists() && std::fs::rename(target, aside).is_ok();
    let renamed = std::fs::rename(unpacked, target);
    let restored = renamed.is_err() && moved && std::fs::rename(aside, target).is_ok();
    // the copy set aside goes once the new one is in place or it is back; else it is all there is of it
    if renamed.is_ok() || restored {
        let _ = remove_tree(aside);
    }
    renamed
}

/// The part file of a revision's zip.
fn part_path(dir: &Path, revision: i64) -> PathBuf {
    dir.join(format!("{revision}.zip.part"))
}

/// The directory a revision is unpacked into before it is swapped in.
fn unpack_dir(dir: &Path, revision: i64) -> PathBuf {
    dir.join(format!(".{revision}.unpack"))
}

/// The bytes of a revision's part file in a package's directory.
pub(crate) fn part_len(dir: &Path, revision: i64) -> u64 {
    std::fs::metadata(part_path(dir, revision)).map_or(0, |meta| meta.len())
}

/// Remove the part files (and their tags, and a tag's temporary file a crash left, `.<n>.zip.tag.<pid>.tmp`) of a
/// package's directory, but `except`'s revision's: how many went.
pub(crate) fn remove_parts(dir: &Path, except: Option<i64>) -> usize {
    let mut removed = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let temporary = name
            .strip_prefix('.')
            .and_then(|rest| rest.strip_suffix(".tmp"))
            .and_then(|rest| rest.rsplit_once('.'))
            .map(|(file, _pid)| file.to_string());
        let file = temporary.as_deref().unwrap_or(&name);
        let Some(revision) = file.strip_suffix(".zip.part").or_else(|| file.strip_suffix(".zip.tag")) else {
            continue;
        };
        if except.is_some_and(|keep| revision == keep.to_string()) && temporary.is_none() {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => removed += 1,
            Err(err) => tracing::warn!("A map package's part file was not removed: {err}"),
        }
    }
    removed
}

/// Make `relative`'s directories under `target`, one at a time and none of `target` itself, so a package's folder
/// removed meanwhile (a wipe) is not made again.
fn create_below(target: &Path, relative: &Path) -> std::io::Result<()> {
    let mut at = target.to_path_buf();
    for part in relative.components() {
        at.push(part);
        // its mode is set on Unix only
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&at) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && at.is_dir() => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Append at most `limit` bytes of `body` to the part file: the bytes written. On a cut, what came before it stays in
/// the part, so the next run goes on from there, and the cut is the error (the run ends, to be tried again).
fn append(part: &Path, body: Box<dyn Read + Send>, limit: u64) -> Result<u64, String> {
    let mut file = private_file(part, true).map_err(|err| err.to_string())?;
    let mut taken = body.take(limit);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;
    loop {
        let read = match taken.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                let _ = file.flush();
                return Err(err.to_string());
            }
        };
        file.write_all(&buffer[..read]).map_err(|err| err.to_string())?;
        written += read as u64;
    }
    file.flush().map_err(|err| err.to_string())?;
    Ok(written)
}

/// A file opened to write, mode 0600: appended to (made when missing), or made new.
fn private_file(path: &Path, append: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    if append {
        options.append(true).create(true);
    } else {
        options.write(true).create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// `bytes a-b/total` of a Content-Range.
fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((
        start.trim().parse().ok()?,
        end.trim().parse().ok()?,
        total.trim().parse().ok()?,
    ))
}

fn is_sha256(hex: &str) -> bool {
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The SHA-256 of a file, read as a stream.
fn file_sha256(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hashing = Hashing {
        inner: std::io::sink(),
        hasher: Sha256::new(),
    };
    std::io::copy(&mut file, &mut hashing)?;
    Ok(hex(&hashing.hasher.finalize()))
}

/// A writer that hashes what it writes.
struct Hashing<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Unpack the zip `part` into `target` with bounds: its manifest (the package and revision this download's), then
/// each member the manifest lists, within its size and checked against its SHA-256; a member the manifest does not
/// list, one it lists that is missing, and members past `max_bytes` in all refuse the package. A cancel stops it
/// within a MiB, and no directory above `target` is made.
fn unpack(
    part: &Path,
    target: &Path,
    package_id: &str,
    revision: i64,
    max_bytes: u64,
    job: &Job,
) -> Result<(), String> {
    let mut file = File::open(part).map_err(|err| format!("The downloaded file could not be read: {err}"))?;
    let len = file.metadata().map_err(|err| err.to_string())?.len();
    let entries = unzip::entries(&mut file, len)?;
    let manifest_entry = entries
        .iter()
        .find(|entry| entry.path == "manifest.json")
        .ok_or("The offline map has no manifest")?;
    if manifest_entry.size > MANIFEST_MAX {
        return Err("The offline map's manifest is larger than this app reads".to_string());
    }
    let mut manifest_bytes = Vec::new();
    unzip::copy_entry(&mut file, manifest_entry, &mut manifest_bytes, &|| false)?;
    let manifest = parse_manifest(&manifest_bytes)?;
    let named_id = manifest.package["packageId"].as_str().unwrap_or_default();
    let named_revision = manifest.package["revision"].as_i64();
    if named_id != package_id || named_revision != Some(revision) {
        return Err("The offline map's manifest is of another package or revision".to_string());
    }
    let mut total = 0u64;
    for entry in entries.iter().filter(|entry| entry.path != "manifest.json") {
        let Some(member) = manifest.members.get(&entry.path) else {
            return Err(format!(
                "The offline map holds a file its manifest does not list ({})",
                entry.path
            ));
        };
        if entry.size != member.size {
            return Err(format!("{} is not the size its manifest says", entry.path));
        }
        total = total.saturating_add(entry.size);
    }
    if let Some(missing) = manifest
        .members
        .keys()
        .find(|path| !entries.iter().any(|entry| &entry.path == *path))
    {
        return Err(format!("The offline map lacks {missing}, which its manifest lists"));
    }
    if total > max_bytes || total > len {
        return Err("The offline map's files are larger than the offline maps may take".to_string());
    }
    remove_tree(target)?;
    // made alone, in the package's folder the part is in: never that folder again after a wipe
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err("The offline map's folder is not one".to_string());
    };
    create_below(parent, Path::new(name)).map_err(|err| err.to_string())?;
    crate::files::write_private(&target.join("manifest.json"), &manifest_bytes).map_err(|err| err.to_string())?;
    for entry in entries.iter().filter(|entry| entry.path != "manifest.json") {
        if job.cancelled() {
            return Err("Cancelled".to_string());
        }
        let Some(member) = manifest.members.get(&entry.path) else {
            continue;
        };
        // a plain relative path (unzip::plain_path): it stays under target
        let path = target.join(&entry.path);
        if let Some(parent) = Path::new(&entry.path).parent() {
            create_below(target, parent).map_err(|err| format!("{}: {err}", entry.path))?;
        }
        let out = private_file(&path, false).map_err(|err| format!("{}: {err}", entry.path))?;
        let mut hashing = Hashing {
            inner: std::io::BufWriter::new(out),
            hasher: Sha256::new(),
        };
        let written = unzip::copy_entry(&mut file, entry, &mut hashing, &|| job.cancelled())?;
        hashing.flush().map_err(|err| err.to_string())?;
        if written != member.size {
            return Err(format!("{} was cut short", entry.path));
        }
        if hex(&hashing.hasher.finalize()) != member.sha256.to_ascii_lowercase() {
            return Err(format!("{} does not match its checksum", entry.path));
        }
    }
    Ok(())
}
