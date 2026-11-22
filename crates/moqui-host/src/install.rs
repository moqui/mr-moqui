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

//! Per-user runtime installation (plans/Install-DevCA.md). Stage the complete selected input
//! before reporting conflicts, then publish individual files by rename in the destination filesystem.
//! A manifest of the installed definition files (`.install-manifest`) lets the next install remove the ones its
//! source no longer has, and `.install-version` records the binary an `--installed` run is checked against.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use moqui_lib::{error::Error, private_fs};
use sha2::Digest as _;

/// The installed definition files and the hash of each, one `<sha256>  <path>` line per file.
const MANIFEST: &str = ".install-manifest";
/// The version text of the binary installed (`moqui version`).
const VERSION: &str = ".install-version";

/// The instance data and secrets that only `--with-data` copies; never in the manifest, so never pruned.
const DATA: [&str; 5] = ["db", "txlog", ".env", "cert", "opensearch/data"];

/// This machine's identity in a cluster or an edge link, made again by the installed runtime when it is missing, so a
/// copy with `--with-data` is not a second instance with the same id (`moqui_core::cluster`, `edge_link`).
const MACHINE_IDENTITY: [&str; 3] = ["db/instance_id", "db/edge_member_key", "db/edge_members"];

pub(crate) fn destination() -> Result<PathBuf, Error> {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return Err(Error::new(
            "per-user runtime installation is supported on macOS and Linux",
        ));
    }
    moqui_lib::shared_runtime::directory()
        .map_err(|err| Error::new(err.to_string()))?
        .ok_or_else(|| Error::new("per-user runtime is disabled or HOME is unset"))
}

pub(crate) fn install(args: &[String]) -> Result<(), Error> {
    let dest = destination()?;
    let mut source = None;
    let mut force = false;
    let mut data = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--from" if source.is_none() => {
                source = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| Error::new("--from needs a runtime directory"))?,
                ))
            }
            "--with-data" => data = true,
            "--force" => force = true,
            _ => return Err(Error::new("usage: moqui install [--from DIR] [--with-data] [--force]")),
        }
    }
    let warnings = install_into(
        &dest,
        source.as_deref(),
        data,
        force,
        &std::env::current_exe().map_err(|err| Error::new(err.to_string()))?,
        &crate::version_text(),
    )
    .map_err(|e| Error::new(format!("install {}: {e}", dest.display())))?;
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    println!(
        "Installed in {}. Add this directory to PATH and run moqui --installed.",
        dest.display()
    );
    Ok(())
}

/// Check that the binary of an `--installed` run is the one `moqui install` installed in `runtime`.
///
/// # Errors
///
/// A message for the log when it is another version, or no version was recorded.
pub(crate) fn check_version(runtime: &Path) -> Result<(), Error> {
    check_version_of(runtime, &crate::version_text())
}

fn check_version_of(runtime: &Path, running: &str) -> Result<(), Error> {
    match fs::read_to_string(runtime.join(VERSION)) {
        Ok(installed) if installed.trim() == running.trim() => Ok(()),
        Ok(installed) => Err(Error::new(format!(
            "This binary ({}) is not the one installed in {} ({}); its built-in files may not match the installed \
             ones. Run moqui install with this binary, or run the installed one.",
            running.trim(),
            runtime.display(),
            installed.trim()
        ))),
        Err(_) => Err(Error::new(format!(
            "{} records no installed version; run moqui install again to record it",
            runtime.display()
        ))),
    }
}

fn is_data(rel: &Path) -> bool {
    DATA.iter().any(|s| rel.starts_with(s))
}

fn excluded(rel: &Path, data: bool) -> bool {
    if rel == Path::new(".install.lock")
        || rel.components().any(|c| {
            c.as_os_str()
                .to_str()
                .is_some_and(|name| name.starts_with(".install-") || name.starts_with(".dev-ca"))
        })
        || rel.starts_with("cert/dev-ca/signer")
        || rel == Path::new("cert/dev-ca/issuer.key")
        || rel == Path::new("cert/dev-ca/ca.key")
    {
        // Never export the issuer, or restore public material over another signing identity.
        return true;
    }
    if [
        "log",
        "tmp",
        "save",
        "install",
        ".git",
        "db/install",
        "opensearch/logs",
        "opensearch/pid",
    ]
    .iter()
    .any(|s| rel.starts_with(s))
        || (!data && is_data(rel))
        || MACHINE_IDENTITY.iter().any(|s| rel == Path::new(s))
    {
        return true;
    }
    let Some(name) = rel.file_name().and_then(|s| s.to_str()) else {
        return true;
    };
    name == ".git"
        || name == "postmaster.pid"
        || name == "moqui-owner.pid"
        || (rel.starts_with("components") && moqui_embedded::select::is_excluded(name, rel.components().count() == 4))
}

/// Copy the selected files of `source` into `staged`. A symbolic link or another entry that is not a regular file or
/// directory is left out with a warning, so one link does not stop the install.
fn stage(source: &Path, staged: &Path, rel: &Path, data: bool, warnings: &mut Vec<String>) -> io::Result<()> {
    for item in fs::read_dir(source.join(rel))? {
        let item = item?;
        let next = rel.join(item.file_name());
        if next == Path::new("moqui") || excluded(&next, data) {
            continue;
        }
        let kind = item.file_type()?;
        let to = staged.join(&next);
        if kind.is_dir() {
            private_fs::create_dir_all(&to)?;
            stage(source, staged, &next, data, warnings)?;
        } else if kind.is_file() {
            fs::copy(item.path(), to)?;
        } else if kind.is_symlink() {
            warnings.push(format!(
                "left out the symbolic link {}; copy what it links to into the runtime to install it",
                item.path().display()
            ));
        } else {
            warnings.push(format!(
                "left out {}, which is not a regular file or directory",
                item.path().display()
            ));
        }
    }
    Ok(())
}

/// A server or an embedded Postgres still running on `source`'s data: a `postmaster.pid` or `moqui-owner.pid` under its
/// `db` that names a live process. Copying a live database directory gives a broken copy.
fn live_server(source: &Path) -> Option<(PathBuf, u32)> {
    fn walk(dir: &Path, depth: usize) -> Option<(PathBuf, u32)> {
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if depth > 0 {
                    if let Some(found) = walk(&path, depth - 1) {
                        return Some(found);
                    }
                }
            } else if kind.is_file() && matches!(entry.file_name().to_str(), Some("postmaster.pid" | "moqui-owner.pid"))
            {
                let pid = fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| text.lines().next().and_then(|line| line.trim().parse::<u32>().ok()));
                if let Some(pid) = pid.filter(|pid| process_live(*pid)) {
                    return Some((path, pid));
                }
            }
        }
        None
    }
    walk(&source.join("db"), 4)
}

fn process_live(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn sha256(path: &Path) -> io::Result<String> {
    let mut hasher = sha2::Sha256::new();
    let mut file = fs::File::open(path)?;
    let mut buf = [0; 8192];
    loop {
        let count = file.read(&mut buf)?;
        if count == 0 {
            break;
        }
        hasher.update(&buf[..count]);
    }
    Ok(moqui_lib::codec::hex_encode(&hasher.finalize()))
}

/// The definition files of the staged tree, with their hashes: every staged file but the instance data.
fn manifest_of(staged: &Path, rel: &Path, out: &mut BTreeMap<PathBuf, String>) -> io::Result<()> {
    for entry in fs::read_dir(staged.join(rel))? {
        let entry = entry?;
        let path = rel.join(entry.file_name());
        if is_data(&path) {
            continue;
        }
        if entry.file_type()?.is_dir() {
            manifest_of(staged, &path, out)?;
        } else {
            out.insert(path, sha256(&entry.path())?);
        }
    }
    Ok(())
}

/// The manifest an earlier install wrote; empty when there is none (an install before manifests).
fn read_manifest(dest: &Path) -> BTreeMap<PathBuf, String> {
    let Ok(text) = fs::read_to_string(dest.join(MANIFEST)) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| line.split_once("  "))
        .map(|(hash, path)| (PathBuf::from(path), hash.to_string()))
        .filter(|(path, _)| {
            path.is_relative() && path.components().all(|c| matches!(c, std::path::Component::Normal(_)))
        })
        .collect()
}

/// Write `text` to `dest/name` by rename, so a reader never sees half of it.
fn publish(dest: &Path, name: &str, text: &str) -> io::Result<()> {
    let temporary = dest.join(format!("{name}.tmp"));
    fs::write(&temporary, text)?;
    fs::rename(temporary, dest.join(name))
}

fn equal(a: &Path, b: &Path) -> io::Result<bool> {
    use std::io::BufRead;
    if fs::metadata(a)?.len() != fs::metadata(b)?.len() {
        return Ok(false);
    }
    let mut a = io::BufReader::new(fs::File::open(a)?);
    let mut b = fs::File::open(b)?;
    let mut buf = [0; 8192];
    loop {
        let bytes = a.fill_buf()?;
        if bytes.is_empty() {
            return Ok(true);
        }
        let count = bytes.len().min(buf.len());
        b.read_exact(&mut buf[..count])?;
        if bytes[..count] != buf[..count] {
            return Ok(false);
        }
        a.consume(count);
    }
}

#[derive(Default)]
struct Changes {
    directories: Vec<PathBuf>,
    files: Vec<PathBuf>,
    conflicts: Vec<PathBuf>,
    /// Files an earlier install installed that the new source no longer has.
    removed: Vec<PathBuf>,
}

fn compare(staged: &Path, dest: &Path, rel: &Path, changes: &mut Changes) -> io::Result<()> {
    for entry in fs::read_dir(staged.join(rel))? {
        let entry = entry?;
        let path = rel.join(entry.file_name());
        let to = dest.join(&path);
        let existing = match fs::symlink_metadata(&to) {
            Ok(m) => Some(m.file_type()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        if entry.file_type()?.is_dir() {
            if existing.is_some_and(|k| !k.is_dir()) {
                return Err(io::Error::other(format!("expected directory {}", to.display())));
            }
            if existing.is_none() {
                changes.directories.push(path.clone())
            }
            compare(staged, dest, &path, changes)?;
        } else {
            if let Some(kind) = existing {
                if !kind.is_file() {
                    return Err(io::Error::other(format!("expected regular file {}", to.display())));
                }
                if equal(&entry.path(), &to)? {
                    continue;
                }
                changes.conflicts.push(path.clone());
            }
            changes.files.push(path);
        }
    }
    Ok(())
}

/// The files of the last install's manifest that this one does not install: removed, and a conflict when changed
/// since that install.
fn prune(dest: &Path, installed: &BTreeSet<PathBuf>, changes: &mut Changes) -> io::Result<()> {
    for (path, hash) in read_manifest(dest) {
        if installed.contains(&path) || is_data(&path) {
            continue;
        }
        let to = dest.join(&path);
        if !fs::symlink_metadata(&to).is_ok_and(|m| m.is_file()) {
            continue;
        }
        if sha256(&to)? != hash {
            changes.conflicts.push(path.clone());
        }
        changes.removed.push(path);
    }
    Ok(())
}

/// Install `source` (else the binary's built-in runtime) and `executable` into `dest`, recording `version`. Returns
/// the warnings to show.
fn install_into(
    dest: &Path,
    source: Option<&Path>,
    data: bool,
    force: bool,
    executable: &Path,
    version: &str,
) -> io::Result<Vec<String>> {
    if source.is_some_and(|s| !moqui_core::context::layout::is_runtime(s)) {
        return Err(io::Error::other("--from needs a runtime with components/base"));
    }
    let mut warnings = Vec::new();
    if data {
        if let Some((lock, pid)) = source.and_then(live_server) {
            return Err(io::Error::other(format!(
                "--with-data needs a stopped runtime: {} names the running process {pid}; stop its server first",
                lock.display()
            )));
        }
        if source.is_some_and(|s| s.join(".env").is_file() || s.join("cert").is_dir()) {
            warnings.push(
                "--with-data copied the runtime's .env and cert keys: the installed runtime has the source's secrets \
                 and TLS identities. Its instance id and edge member key are made new."
                    .to_string(),
            );
        }
    }
    private_fs::create_dir_all(dest)?;
    let dest = dest.canonicalize()?;
    if let Some(source) = source {
        let source = source.canonicalize()?;
        if dest.starts_with(&source) || source.starts_with(&dest) {
            return Err(io::Error::other("source and installed runtime overlap"));
        }
    }
    let lock = private_fs::file_options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dest.join(".install.lock"))?;
    lock.lock()?;
    let staged = private_fs::new_dir_in(&dest, ".install")?;
    let result = (|| {
        if let Some(source) = source {
            stage(source, &staged, Path::new(""), data, &mut warnings)?
        } else {
            moqui_embedded::write_runtime(&staged, &moqui_core::context::layout::framework_dir(&staged))?;
        }
        fs::copy(executable, staged.join("moqui"))?;
        let mut changes = Changes::default();
        compare(&staged, &dest, Path::new(""), &mut changes)?;
        if dest.join("cert/dev-ca/signer").is_dir() {
            for list in [&mut changes.files, &mut changes.conflicts] {
                list.retain(|p| !p.starts_with("cert/dev-ca"));
            }
        }
        let mut manifest = BTreeMap::new();
        manifest_of(&staged, Path::new(""), &mut manifest)?;
        prune(&dest, &manifest.keys().cloned().collect(), &mut changes)?;
        if !force && !changes.conflicts.is_empty() {
            changes.conflicts.sort();
            let names = changes
                .conflicts
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            return Err(io::Error::other(format!(
                "files differ (use --force to replace):\n{names}"
            )));
        }
        for path in changes.directories {
            private_fs::create_dir_all(&dest.join(path))?;
        }
        for path in changes.files {
            fs::rename(staged.join(&path), dest.join(path))?;
        }
        for path in &changes.removed {
            fs::remove_file(dest.join(path))?;
            // the directories it leaves empty, up to the installed runtime's own
            let mut dir = path.parent();
            while let Some(parent) = dir.filter(|p| !p.as_os_str().is_empty()) {
                if fs::remove_dir(dest.join(parent)).is_err() {
                    break;
                }
                dir = parent.parent();
            }
        }
        let lines: String = manifest
            .iter()
            .map(|(path, hash)| format!("{hash}  {}\n", path.display()))
            .collect();
        publish(&dest, MANIFEST, &lines)?;
        publish(&dest, VERSION, &format!("{}\n", version.trim()))?;
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(staged);
    result.and(cleanup).map(|()| warnings)
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
