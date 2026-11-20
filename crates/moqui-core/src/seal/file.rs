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

//! `seal.json` (plans/Sealed-Secrets.md §3.1, §6 "What the root key wraps before S1"): versioned JSON in the data
//! directory holding only wrapped values. The unseal method and its public metadata with the root key wrapped by it;
//! the root key wrapped by a recovery key whose shares are issued, with a salted hash of each share so a wrong one is
//! refused on entry; the bootstrap secrets, sealed under a key derived from the root key; and one data key for
//! `SecretValue`, wrapped by the root key. A write goes to a temporary file, is synced, and is renamed over the old
//! file (a first one only where there is none, under the file's lock), so a reader sees the old file or the new one, never
//! part of one, and a crash before the rename leaves the old file in use.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::context::Error;

/// The file's name in the data directory.
pub const FILE_NAME: &str = "seal.json";
/// The `format` every seal file carries.
pub const FORMAT: &str = "moqui-seal";
/// The newest format version this build writes and reads.
pub const VERSION: u32 = 1;

/// The parsed `seal.json`. Its `Debug` names the method only: a `dev` seal's metadata holds its key.
#[derive(Clone, Serialize, Deserialize)]
pub struct SealFile {
    pub format: String,
    pub version: u32,
    /// When the seal was made (`init`), RFC 3339.
    pub created: String,
    /// Counts the changes of the wrapped root: 1 at init, one more at each `rekey` and `rotate-root`.
    pub generation: u64,
    /// Names the root key without giving it (`super::root_id`), so a bundle and a running server can be matched.
    pub root_id: String,
    /// The unseal method: `kind` (`dev`, `passphrase`, `shamir`, `tpm`, and in S11 `kms`, `transit`) and that
    /// method's own fields, the root key wrapped by it among them.
    pub method: Map<String, Value>,
    /// The root key wrapped by the recovery key, and how its shares are checked. None for the `dev` seal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
    /// The bootstrap secrets, by name: the secrets needed before the database opens (S4 fills them).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bootstrap: BTreeMap<String, SealedValue>,
    /// The data key of `SecretValue` rows, wrapped by the root key (S4 uses it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_value_key: Option<SealedValue>,
}

impl std::fmt::Debug for SealFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealFile")
            .field("version", &self.version)
            .field("generation", &self.generation)
            .field("method", &self.method_kind())
            .field(
                "recovery",
                &self
                    .recovery
                    .as_ref()
                    .map(|recovery| (recovery.threshold, recovery.shares)),
            )
            .field("bootstrap", &self.bootstrap.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The recovery key's part of the seal: `threshold` of `shares` recover it (plans/Sealed-Secrets.md §3.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recovery {
    pub threshold: u8,
    pub shares: u8,
    /// The salt of the share hashes, base64.
    pub salt: String,
    /// SHA-256(salt ‖ index ‖ share) of each share, by index from 1, base64.
    pub share_hashes: Vec<String>,
    /// The root key sealed under the recovery key, base64.
    pub wrapped_root: String,
}

/// A value sealed under a key from the root key, with the generation in its associated data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SealedValue {
    pub generation: u64,
    /// The sealed bytes, base64.
    pub sealed: String,
}

impl SealFile {
    /// The method's `kind`, empty when the file has none.
    #[must_use]
    pub fn method_kind(&self) -> &str {
        self.method.get("kind").and_then(Value::as_str).unwrap_or("")
    }

    /// A text field of the method's metadata.
    #[must_use]
    pub fn method_text(&self, name: &str) -> Option<&str> {
        self.method.get(name).and_then(Value::as_str)
    }

    /// The file as the JSON it is written as.
    ///
    /// # Errors
    ///
    /// Never in practice: every field serializes.
    pub fn to_text(&self) -> Result<String, Error> {
        serde_json::to_string_pretty(self).map_err(|err| Error::new(format!("seal.json does not serialize: {err}")))
    }

    /// Parse a seal file's text, checking its format and version.
    ///
    /// # Errors
    ///
    /// Text that is not a seal file, or one of a newer version than this build reads.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let file: Self =
            serde_json::from_str(text).map_err(|err| Error::new(format!("seal.json is not a seal file: {err}")))?;
        if file.format != FORMAT {
            return Err(Error::new(format!(
                "seal.json has format {:?}, not {FORMAT}",
                file.format
            )));
        }
        if file.version > VERSION {
            return Err(Error::new(format!(
                "seal.json is version {}, written by a newer MR Moqui; this one reads version {VERSION}",
                file.version
            )));
        }
        if file.method_kind().is_empty() {
            return Err(Error::new("seal.json names no unseal method"));
        }
        Ok(file)
    }
}

/// Where `seal.json` is: in the data directory, `<runtime>/db` (plans/Sealed-Secrets.md §6 "Where seal.json and the
/// socket are").
#[must_use]
pub fn path_in(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("db").join(FILE_NAME)
}

/// Read `path`: `None` when there is no file.
///
/// # Errors
///
/// A file that cannot be read or parsed. A damaged seal is not passed over: the server stays down and says why.
pub fn load(path: &Path) -> Result<Option<SealFile>, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => SealFile::parse(&text)
            .map(Some)
            .map_err(|err| Error::new(format!("{}: {err}", path.display()))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::new(format!("could not read {}", path.display())).with_source(err)),
    }
}

/// A new seal file written beside the old one and synced, not yet in place: [`Staged::commit`] renames it over the old
/// file. Dropped uncommitted, it is removed and the old file stays, as after a crash before the rename.
#[must_use = "a staged seal file is in place only once committed"]
pub struct Staged {
    temp: PathBuf,
    target: PathBuf,
}

/// A temporary file name of this process, beside `target`.
fn temp_path(target: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = format!(
        "{FILE_NAME}.tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    target.with_file_name(name)
}

/// Write `file` beside `target` and sync it.
///
/// # Errors
///
/// The directory cannot be made or the file cannot be written.
pub fn stage(target: &Path, file: &SealFile) -> Result<Staged, Error> {
    let text = file.to_text()?;
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not make {}", dir.display())).with_source(err))?;
    }
    let temp = temp_path(target);
    let staged = Staged {
        temp: temp.clone(),
        target: target.to_path_buf(),
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut out| {
        out.write_all(text.as_bytes())?;
        out.write_all(b"\n")?;
        out.sync_all()
    });
    written.map_err(|err| Error::new(format!("could not write {}", temp.display())).with_source(err))?;
    Ok(staged)
}

impl Staged {
    /// Rename the staged file over the target, then sync the directory so the rename survives a crash.
    ///
    /// # Errors
    ///
    /// The rename fails; the old file stays.
    pub fn commit(self) -> Result<(), Error> {
        std::fs::rename(&self.temp, &self.target)
            .map_err(|err| Error::new(format!("could not put {} in place", self.target.display())).with_source(err))?;
        sync_dir(&self.target);
        Ok(())
    }

    /// Put the staged file in place only when there is none, under the seal file's lock, so two processes that make
    /// a seal at once do not replace each other's: false when another file was there first (the staged one is
    /// dropped). A rename, so it works on a file system with no hard links.
    ///
    /// # Errors
    ///
    /// The rename fails.
    pub fn commit_new(self, _held: &FileLock) -> Result<bool, Error> {
        if self.target.exists() {
            return Ok(false);
        }
        self.commit().map(|()| true)
    }
}

impl Drop for Staged {
    /// Uncommitted: the temporary name goes. After a rename it is gone already.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temp);
    }
}

/// Sync the directory of `path`, so a rename or link in it is on disk. A failure is ignored: the data itself is synced.
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(handle) = std::fs::File::open(dir) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Remove the temporary files a crash left beside `target` (a write stopped before its rename).
/// Only one older than a minute goes: a newer one may be another live process's write in progress.
pub fn remove_stale_temps(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name().and_then(|name| name.to_str())) else {
        return;
    };
    let prefix = format!("{name}.tmp-");
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > std::time::Duration::from_secs(60));
        if old && entry.file_name().to_str().is_some_and(|file| file.starts_with(&prefix)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The exclusive lock on a runtime's seal file (`seal.json.lock` beside it), held while a write reads the file again
/// and replaces it, so writes in two processes do not lose each other's changes. Released when dropped.
pub struct FileLock {
    _file: std::fs::File,
}

/// Take the lock of the seal file at `target`, waiting for another process that holds it.
///
/// # Errors
///
/// The lock file cannot be made or locked.
pub fn lock(target: &Path) -> Result<FileLock, Error> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not make {}", dir.display())).with_source(err))?;
    }
    let path = target.with_file_name(format!("{FILE_NAME}.lock"));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .map_err(|err| Error::new(format!("could not open {}", path.display())).with_source(err))?;
    file.lock()
        .map_err(|err| Error::new(format!("could not lock {}", path.display())).with_source(err))?;
    Ok(FileLock { _file: file })
}
