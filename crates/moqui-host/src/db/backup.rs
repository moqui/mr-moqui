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

//! Live backup of the embedded clusters this process runs.
//!
//! `moqui-core` decides which archives stay. This module runs `pg_dump` or `pg_basebackup`, publishes
//! the archive, and applies that plan. The service is `backup#EmbeddedDatabase`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use moqui_core::backup::{
    archive_name, is_archive_name, parse_archive_name, retain, ArchiveIn, BackupMode, BackupOutcome, BackupRequest,
    KeptArchive, RetainPlan, Tier,
};
use moqui_core::driver::DbConnection;
use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use super::embedded::ToolTarget;
use super::entity_db::EntityDb;
use super::postgres::{PostgresConn, PostgresDriver};

const SIDECAR: &str = "moqui-backup.txt";

struct PrefixBackup {
    path: PathBuf,
    kind: String,
    bytes: u64,
    promoted: Vec<String>,
    removed: Vec<String>,
    renames: Vec<(String, String)>,
}

struct IncrementalFrom {
    manifest: PathBuf,
    parent: String,
}

struct Physical {
    name: String,
    stamp: Timestamp,
    parent: Option<String>,
    full: bool,
}

/// Register the runner when this process runs at least one embedded postmaster. An empty target list
/// leaves the service to report `skipped`.
pub fn install(facade: &EntityFacade, db: &EntityDb, search: Option<&crate::search::SearchStore>, runtime: &Path) {
    let mut targets = Vec::new();
    if let Some(target) = db.tool_target() {
        targets.push(target);
    }
    if let Some(store) = search {
        if let Some(target) = store.tool_target() {
            targets.push(target);
        }
    }
    if targets.is_empty() {
        tracing::info!("embedded database backup is off, this process runs no embedded cluster");
        return;
    }
    let runtime = runtime.to_path_buf();
    let runner = Arc::new(move |request: &BackupRequest| run(&targets, &runtime, request));
    facade.set_backup_runner(Some(runner));
}

fn run(targets: &[ToolTarget], runtime: &Path, request: &BackupRequest) -> Result<BackupOutcome, Error> {
    let backup_dir = backup_directory(request, runtime, targets)?;
    if request.mode == BackupMode::Incremental {
        require_incremental_tools(targets)?;
    }
    sweep_partials(&backup_dir);
    let mut errors = Vec::new();
    let mut bytes_written = 0u64;
    let mut promoted = Vec::new();
    let mut removed = Vec::new();
    let mut renames = Vec::new();
    let mut archive_path = PathBuf::new();
    let mut backup_kind = String::new();
    for target in targets {
        match backup_one(&backup_dir, target, request) {
            Ok(one) => {
                bytes_written = bytes_written.saturating_add(one.bytes);
                promoted.extend(one.promoted);
                removed.extend(one.removed);
                renames.extend(one.renames);
                if target.prefix == "moqui" || archive_path.as_os_str().is_empty() {
                    archive_path = one.path;
                    backup_kind = one.kind;
                }
            }
            Err(err) => {
                tracing::error!("embedded database backup of {} failed: {err}", target.prefix);
                errors.push(format!("{}: {err}", target.prefix));
            }
        }
    }
    // Keep is what is on disk, including archives of a prefix that failed this run, so the mirror
    // does not delete those remote copies.
    let keep = scan_keep(&backup_dir);
    Ok(BackupOutcome {
        skipped: false,
        backup_dir,
        archive_path,
        backup_kind,
        bytes_written,
        promoted,
        removed,
        keep,
        renames,
        errors,
    })
}

fn backup_one(dir: &Path, target: &ToolTarget, request: &BackupRequest) -> Result<PrefixBackup, Error> {
    let dump = request.mode == BackupMode::Dump;
    let stamp = fresh_stamp(dir, &target.prefix, dump);
    let name = archive_name(&target.prefix, stamp, Tier::Daily, dump);
    let kind = if dump {
        dump_one(dir, target, &name, request.timeout)?;
        "dump".to_string()
    } else {
        let from = next_incremental(dir, target, request.full_every)?;
        take_physical(dir, target, &name, from, request.timeout)?
    };
    let archives = archives_for_retain(dir, &target.prefix);
    let plan = retain(&archives, stamp, &TimeZone::system(), request);
    let (renames, removed) = apply_plan(dir, &plan);
    let path = published_path(dir, &name, &renames);
    let bytes = bytes_of(&path);
    let promoted = renames.iter().map(|(old, new)| format!("{old} -> {new}")).collect();
    Ok(PrefixBackup {
        path,
        kind,
        bytes,
        promoted,
        removed,
        renames,
    })
}

/// The new archive stays a daily of the current period. A rename in this plan is an older archive.
fn published_path(dir: &Path, name: &str, renames: &[(String, String)]) -> PathBuf {
    let final_name = renames
        .iter()
        .find(|(old, _)| old == name)
        .map(|(_, new)| new.as_str())
        .unwrap_or(name);
    dir.join(final_name)
}

fn dump_one(dir: &Path, target: &ToolTarget, name: &str, timeout: Duration) -> Result<(), Error> {
    let partial = dir.join(format!("{name}.partial"));
    let bin = tool_bin(&target.bin_dir, "pg_dump");
    let mut args = connect_args(target);
    args.push("--format=custom".to_string());
    args.push("--file".to_string());
    args.push(partial.display().to_string());
    if let Err(err) = run_bounded(&bin, &args, &target.password, timeout) {
        remove_partial(&partial);
        return Err(err);
    }
    if let Err(err) = set_mode(&partial, 0o600) {
        remove_partial(&partial);
        return Err(Error::new(format!("chmod {}", partial.display())).with_source(err));
    }
    let published = dir.join(name);
    if let Err(err) = std::fs::rename(&partial, &published) {
        remove_partial(&partial);
        return Err(Error::new(format!("publish {name}")).with_source(err));
    }
    Ok(())
}

/// `from` set means a block incremental against that manifest. `None` is a full `pg_basebackup`.
/// A failed incremental is taken again as a full, except a `pg_hba` refusal or a timeout, which a
/// full would meet the same way.
fn take_physical(
    dir: &Path,
    target: &ToolTarget,
    name: &str,
    from: Option<IncrementalFrom>,
    timeout: Duration,
) -> Result<String, Error> {
    let partial = dir.join(format!("{name}.partial"));
    remove_partial(&partial);
    let bin = tool_bin(&target.bin_dir, "pg_basebackup");
    // `--dbname` is a connection string here, unlike `pg_dump`, where it is the database name.
    let mut args = basebackup_args(target);
    args.push("-D".to_string());
    args.push(partial.display().to_string());
    args.push("-X".to_string());
    args.push("stream".to_string());
    if let Some(source) = &from {
        args.push(format!("--incremental={}", source.manifest.display()));
    }
    if let Err(err) = run_bounded(&bin, &args, &target.password, timeout) {
        remove_partial(&partial);
        if from.is_some() && fallback_full(&err) {
            tracing::error!("incremental backup of {} failed, taking a full: {err}", target.prefix);
            return take_physical(dir, target, name, None, timeout);
        }
        return Err(err);
    }
    let kind = if from.is_some() { "incremental" } else { "full" };
    if from.is_none() {
        let verify = tool_bin(&target.bin_dir, "pg_verifybackup");
        if let Err(err) = run_bounded(&verify, &[partial.display().to_string()], &target.password, timeout) {
            remove_partial(&partial);
            return Err(err);
        }
    }
    let parent = from.as_ref().map(|source| source.parent.as_str()).unwrap_or("");
    if let Err(err) = std::fs::write(partial.join(SIDECAR), format!("kind={kind}\nparent={parent}\n")) {
        remove_partial(&partial);
        return Err(Error::new(format!("write the backup sidecar for {name}")).with_source(err));
    }
    if let Err(err) = publish_modes(&partial) {
        remove_partial(&partial);
        return Err(err);
    }
    let published = dir.join(name);
    if let Err(err) = std::fs::rename(&partial, &published) {
        remove_partial(&partial);
        return Err(Error::new(format!("publish {name}")).with_source(err));
    }
    Ok(kind.to_string())
}

fn next_incremental(dir: &Path, target: &ToolTarget, full_every: u32) -> Result<Option<IncrementalFrom>, Error> {
    let shown = show_summarize_wal(target)?;
    if !shown.eq_ignore_ascii_case("on") {
        tracing::info!(
            "summarize_wal is {shown} on {}; this backup is a full one, and the next start of this cluster turns summarize_wal on",
            target.prefix
        );
        return Ok(None);
    }
    let archives = physical_archives(dir, &target.prefix);
    let Some(full) = newest_full(&archives) else {
        return Ok(None);
    };
    if generation_len(&archives, &full.name) >= full_every {
        tracing::info!(
            "the backup generation for {} holds {full_every} archives, so this one is a full",
            target.prefix
        );
        return Ok(None);
    }
    let manifest = dir.join(&full.name).join("backup_manifest");
    if !manifest.is_file() {
        return Ok(None);
    }
    Ok(Some(IncrementalFrom {
        manifest,
        parent: full.name.clone(),
    }))
}

fn show_summarize_wal(target: &ToolTarget) -> Result<String, Error> {
    let mut conn = connect_superuser(target)?;
    let rows = conn.query_once("SHOW summarize_wal", &[])?;
    let Some((_, value)) = rows.first().and_then(|row| row.columns.first()) else {
        return Err(Error::new(format!(
            "SHOW summarize_wal returned no row on {}",
            target.prefix
        )));
    };
    match value {
        Value::String(text) => Ok(text.trim().to_string()),
        other => Err(Error::new(format!("SHOW summarize_wal returned {other:?}"))),
    }
}

fn connect_superuser(target: &ToolTarget) -> Result<PostgresConn, Error> {
    #[cfg(unix)]
    {
        PostgresDriver::unix_socket(&target.username, &target.database, &target.host)
            .port(target.port)
            .password(&target.password)
            .connect_postgres()
    }
    #[cfg(not(unix))]
    {
        PostgresDriver::tcp_no_tls(&target.username, &target.database, &target.host, target.port)
            .password(&target.password)
            .connect_postgres()
    }
}

fn physical_archives(dir: &Path, prefix: &str) -> Vec<Physical> {
    let mut found = Vec::new();
    for (name, path) in archive_entries(dir) {
        let Some(parsed) = parse_archive_name(&name) else {
            continue;
        };
        if parsed.prefix != prefix || parsed.dump || !path.is_dir() {
            continue;
        }
        let (kind, parent) = read_sidecar(&path);
        let full = path.join("backup_manifest").is_file() && kind != "incremental" && parent.is_none();
        found.push(Physical {
            name,
            stamp: parsed.stamp,
            parent,
            full,
        });
    }
    found
}

fn newest_full(archives: &[Physical]) -> Option<&Physical> {
    archives.iter().filter(|item| item.full).max_by_key(|item| item.stamp)
}

fn generation_len(archives: &[Physical], full_name: &str) -> u32 {
    let mut reached = HashSet::new();
    reached.insert(full_name.to_string());
    loop {
        let before = reached.len();
        for item in archives {
            if let Some(parent) = &item.parent {
                if reached.contains(parent.as_str()) {
                    reached.insert(item.name.clone());
                }
            }
        }
        if reached.len() == before {
            break;
        }
    }
    u32::try_from(reached.len()).unwrap_or(u32::MAX)
}

fn fresh_stamp(dir: &Path, prefix: &str, dump: bool) -> Timestamp {
    let mut stamp = Timestamp::now();
    for _ in 0..60 {
        let name = archive_name(prefix, stamp, Tier::Daily, dump);
        if !dir.join(&name).exists() && !dir.join(format!("{name}.partial")).exists() {
            return stamp;
        }
        stamp = stamp.checked_add(SignedDuration::from_secs(1)).unwrap_or(stamp);
    }
    stamp
}

fn archives_for_retain(dir: &Path, prefix: &str) -> Vec<ArchiveIn> {
    let mut archives = Vec::new();
    for (name, path) in archive_entries(dir) {
        let Some(parsed) = parse_archive_name(&name) else {
            continue;
        };
        if parsed.prefix != prefix {
            continue;
        }
        let parent = if path.is_dir() { read_sidecar(&path).1 } else { None };
        archives.push(ArchiveIn { name, parent });
    }
    archives
}

fn archive_entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::error!("could not list backup directory {}", dir.display());
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.ends_with(".partial") || !is_archive_name(&name) {
            continue;
        }
        found.push((name, entry.path()));
    }
    found
}

fn scan_keep(dir: &Path) -> Vec<KeptArchive> {
    let mut keep: Vec<KeptArchive> = archive_entries(dir)
        .into_iter()
        .map(|(name, path)| KeptArchive { name, path })
        .collect();
    keep.sort_by(|left, right| left.name.cmp(&right.name));
    keep
}

fn apply_plan(dir: &Path, plan: &RetainPlan) -> (Vec<(String, String)>, Vec<String>) {
    let mut renames = Vec::new();
    for (old, new) in &plan.renames {
        if let Err(err) = std::fs::rename(dir.join(old), dir.join(new)) {
            tracing::error!("could not promote {old} to {new}: {err}");
            continue;
        }
        renames.push((old.clone(), new.clone()));
    }
    for (child, parent) in &plan.parent_updates {
        rewrite_parent(dir, child, parent);
    }
    let mut removed = Vec::new();
    for name in &plan.deletes {
        if let Err(err) = remove_archive(&dir.join(name)) {
            tracing::error!("could not remove backup archive {name}: {err}");
            continue;
        }
        removed.push(name.clone());
    }
    (renames, removed)
}

fn rewrite_parent(dir: &Path, child: &str, parent: &str) {
    let path = dir.join(child).join(SIDECAR);
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut kind = "incremental".to_string();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("kind=") {
            if !value.trim().is_empty() {
                kind = value.trim().to_string();
            }
        }
    }
    if let Err(err) = std::fs::write(&path, format!("kind={kind}\nparent={parent}\n")) {
        tracing::error!("could not update the parent of {child}: {err}");
    }
}

fn read_sidecar(dir: &Path) -> (String, Option<String>) {
    let Ok(text) = std::fs::read_to_string(dir.join(SIDECAR)) else {
        return (String::new(), None);
    };
    let mut kind = String::new();
    let mut parent = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("kind=") {
            kind = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("parent=") {
            let value = value.trim();
            if !value.is_empty() {
                parent = Some(value.to_string());
            }
        }
    }
    (kind, parent)
}

fn backup_directory(request: &BackupRequest, runtime: &Path, targets: &[ToolTarget]) -> Result<PathBuf, Error> {
    let raw = match &request.backup_dir {
        Some(path) => path.clone(),
        None => runtime.join("db").join("backup"),
    };
    let resolved = resolve_path(&raw)?;
    ensure_outside(&resolved, targets)?;
    std::fs::create_dir_all(&resolved)
        .map_err(|err| Error::new(format!("create backup directory {}", resolved.display())).with_source(err))?;
    Ok(resolved)
}

fn resolve_path(raw: &Path) -> Result<PathBuf, Error> {
    let mut existing = raw.to_path_buf();
    let mut missing = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(std::ffi::OsStr::to_os_string) else {
            break;
        };
        missing.push(name);
        match existing.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => existing = parent.to_path_buf(),
            _ => break,
        }
    }
    if !existing.exists() {
        existing = PathBuf::from(".");
    }
    let mut resolved = std::fs::canonicalize(&existing)
        .map_err(|err| Error::new(format!("resolve {}", existing.display())).with_source(err))?;
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn ensure_outside(backup: &Path, targets: &[ToolTarget]) -> Result<(), Error> {
    for target in targets {
        let data = std::fs::canonicalize(&target.data_dir).unwrap_or_else(|_| target.data_dir.clone());
        if backup.starts_with(&data) || data.starts_with(backup) {
            return Err(Error::new(format!(
                "backup directory {} has to sit outside the data directory {}",
                backup.display(),
                data.display()
            )));
        }
    }
    Ok(())
}

fn require_incremental_tools(targets: &[ToolTarget]) -> Result<(), Error> {
    for target in targets {
        for name in ["pg_basebackup", "pg_verifybackup", "pg_combinebackup"] {
            let path = tool_bin(&target.bin_dir, name);
            if !path.is_file() {
                return Err(Error::new(format!(
                    "incremental backup needs {} and it is not in {}",
                    path.display(),
                    target.bin_dir.display()
                )));
            }
        }
    }
    Ok(())
}

fn basebackup_args(target: &ToolTarget) -> Vec<String> {
    vec![
        "--username".to_string(),
        target.username.clone(),
        "--port".to_string(),
        target.port.to_string(),
        "--no-password".to_string(),
        "--host".to_string(),
        target.host.clone(),
    ]
}

fn connect_args(target: &ToolTarget) -> Vec<String> {
    vec![
        "--username".to_string(),
        target.username.clone(),
        "--port".to_string(),
        target.port.to_string(),
        "--dbname".to_string(),
        target.database.clone(),
        "--no-password".to_string(),
        "--host".to_string(),
        target.host.clone(),
    ]
}

fn tool_bin(bin_dir: &Path, name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        bin_dir.join(format!("{name}.exe"))
    }
    #[cfg(not(windows))]
    {
        bin_dir.join(name)
    }
}

fn fallback_full(err: &Error) -> bool {
    let text = err.to_string().to_lowercase();
    !text.contains("pg_hba") && !text.contains("timed out")
}

fn run_bounded(bin: &Path, args: &[String], password: &str, timeout: Duration) -> Result<(), Error> {
    let name = bin
        .file_name()
        .and_then(|text| text.to_str())
        .unwrap_or("postgres tool");
    let mut command = Command::new(bin);
    command
        .args(args)
        .env("PGPASSWORD", password)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|err| Error::new(format!("run {}", bin.display())).with_source(err))?;
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr {
            use std::io::Read;
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                kill_group(&mut child);
                let _ = reader.join();
                return Err(Error::new(format!(
                    "{name} timed out after {} seconds",
                    timeout.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(err) => {
                kill_group(&mut child);
                let _ = reader.join();
                return Err(Error::new(format!("wait for {name}")).with_source(err));
            }
        }
    };
    let stderr = reader.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    Err(tool_error(name, &status, &stderr))
}

fn tool_error(name: &str, status: &std::process::ExitStatus, stderr: &[u8]) -> Error {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.contains("pg_hba") {
        return Error::new(format!(
            "{name} was refused by pg_hba.conf; the backup leaves that file unchanged: {detail}"
        ));
    }
    Error::new(format!("{name} exited {status}: {detail}"))
}

#[cfg(unix)]
fn kill_group(child: &mut std::process::Child) {
    let pid = child.id();
    let _ = Command::new("kill").args(["-KILL", &format!("-{pid}")]).status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_group(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn sweep_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.ends_with(".partial") {
            remove_partial(&entry.path());
        }
    }
}

fn remove_partial(path: &Path) {
    if let Err(err) = remove_archive(path) {
        tracing::error!("could not remove backup partial {}: {err}", path.display());
    }
}

fn remove_archive(path: &Path) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn publish_modes(path: &Path) -> Result<(), Error> {
    if path.is_dir() {
        set_mode(path, 0o700).map_err(|err| Error::new(format!("chmod {}", path.display())).with_source(err))?;
        let mut dirs = vec![path.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            let entries = std::fs::read_dir(&dir)
                .map_err(|err| Error::new(format!("list {}", dir.display())).with_source(err))?;
            for entry in entries {
                let entry = entry.map_err(|err| Error::new(format!("list {}", dir.display())).with_source(err))?;
                let child = entry.path();
                if child.is_dir() {
                    set_mode(&child, 0o700)
                        .map_err(|err| Error::new(format!("chmod {}", child.display())).with_source(err))?;
                    dirs.push(child);
                } else {
                    set_mode(&child, 0o600)
                        .map_err(|err| Error::new(format!("chmod {}", child.display())).with_source(err))?;
                }
            }
        }
    } else {
        set_mode(path, 0o600).map_err(|err| Error::new(format!("chmod {}", path.display())).with_source(err))?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

fn bytes_of(path: &Path) -> u64 {
    if path.is_file() {
        return std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    let mut dirs = vec![path.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                dirs.push(child);
            } else if let Ok(meta) = entry.metadata() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct DirGuard(PathBuf);

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = format!("moqui-backup-{}-{nanos}", std::process::id());
        if Path::new("/tmp").is_dir() {
            PathBuf::from("/tmp").join(name)
        } else {
            std::env::temp_dir().join(name)
        }
    }

    fn request(mode: BackupMode, backup_dir: PathBuf, daily: u32) -> BackupRequest {
        BackupRequest {
            mode,
            backup_dir: Some(backup_dir),
            daily,
            weekly: 5,
            monthly: 12,
            yearly: None,
            full_every: 7,
            timeout: Duration::from_secs(180),
        }
    }

    #[test]
    fn a_backup_directory_inside_the_data_directory_is_refused() {
        let base = temp_dir();
        let _guard = DirGuard(base.clone());
        let data = base.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let target = ToolTarget {
            prefix: "moqui".to_string(),
            bin_dir: base.join("bin"),
            username: "postgres".to_string(),
            password: "secret".to_string(),
            port: 5432,
            host: data.display().to_string(),
            database: "moqui".to_string(),
            data_dir: data.clone(),
        };
        let err = run(&[target], &base, &request(BackupMode::Dump, data.join("backup"), 7)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("outside"), "{text}");
        assert!(!data.join("backup").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dump_round_trip_rotates_the_same_week_and_incremental_when_the_tools_exist() {
        use std::os::unix::fs::PermissionsExt;
        use std::slice::from_ref;

        use super::super::embedded::EmbeddedCluster;

        let base = temp_dir();
        let _guard = DirGuard(base.clone());
        let (cluster, driver) = EmbeddedCluster::start(base.join("embedded"), 20).unwrap();
        let mut conn = driver.connect_postgres().unwrap();
        conn.execute("CREATE TABLE backup_kept (n int)", &[]).unwrap();
        conn.execute("INSERT INTO backup_kept (n) VALUES (?)", &[Value::I32(7)])
            .unwrap();
        drop(conn);

        let target = cluster.tool_connect("moqui");
        let backup = base.join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("moqui-20000101T000000Z-d.dump.partial"), b"partial").unwrap();
        let older = Timestamp::now().checked_sub(SignedDuration::from_secs(2)).unwrap();
        let same_week = week_key(older) == week_key(Timestamp::now());
        let old_name = archive_name("moqui", older, Tier::Daily, true);
        if same_week {
            std::fs::write(backup.join(&old_name), b"older").unwrap();
        }

        let outcome = run(from_ref(&target), &base, &request(BackupMode::Dump, backup.clone(), 1)).unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.backup_kind, "dump");
        assert!(outcome.archive_path.is_file(), "{}", outcome.archive_path.display());
        let mode = std::fs::metadata(&outcome.archive_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!backup.join("moqui-20000101T000000Z-d.dump.partial").exists());
        assert!(outcome.keep.iter().all(|kept| !kept.name.ends_with(".partial")));
        if same_week {
            assert!(!backup.join(&old_name).exists(), "{old_name} stayed");
            assert!(
                outcome.removed.iter().any(|name| name == &old_name),
                "removed {:?}",
                outcome.removed
            );
            let weekly = std::fs::read_dir(&backup)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains("-w"));
            assert!(!weekly, "a same-week daily was promoted");
        }

        cluster.load_save(&outcome.archive_path).unwrap();
        let mut conn = driver.connect_postgres().unwrap();
        let rows = conn.query_once("SELECT n FROM backup_kept", &[]).unwrap();
        assert_eq!(rows[0].get("n"), Some(&Value::I32(7)));
        drop(conn);

        let combine = tool_bin(&target.bin_dir, "pg_combinebackup");
        let verify = tool_bin(&target.bin_dir, "pg_verifybackup");
        if !combine.is_file() || !verify.is_file() {
            eprintln!("skipping incremental backup: pg_combinebackup or pg_verifybackup is not in the theseus install");
            return;
        }
        let incr = base.join("backup-incr");
        let first = run(
            from_ref(&target),
            &base,
            &request(BackupMode::Incremental, incr.clone(), 7),
        )
        .unwrap();
        if first.errors.iter().any(|err| err.contains("pg_hba")) {
            assert!(
                first
                    .errors
                    .iter()
                    .any(|err| err.contains("leaves that file unchanged")),
                "{:?}",
                first.errors
            );
            return;
        }
        assert!(first.errors.is_empty(), "{:?}", first.errors);
        assert_eq!(first.backup_kind, "full");
        assert!(first.archive_path.join("backup_manifest").is_file());
        assert!(first.archive_path.join("PG_VERSION").is_file());
        let sidecar = std::fs::read_to_string(first.archive_path.join(SIDECAR)).unwrap();
        assert!(sidecar.contains("kind=full"), "{sidecar}");
        let full_name = first.archive_path.file_name().unwrap().to_string_lossy().into_owned();

        let second = run(from_ref(&target), &base, &request(BackupMode::Incremental, incr, 7)).unwrap();
        if second.errors.iter().any(|err| err.contains("pg_hba")) {
            assert!(
                second
                    .errors
                    .iter()
                    .any(|err| err.contains("leaves that file unchanged")),
                "{:?}",
                second.errors
            );
            return;
        }
        assert!(second.errors.is_empty(), "{:?}", second.errors);
        assert_eq!(second.backup_kind, "incremental", "{:?}", second.errors);
        let sidecar = std::fs::read_to_string(second.archive_path.join(SIDECAR)).unwrap();
        assert!(sidecar.contains("kind=incremental"), "{sidecar}");
        assert!(sidecar.contains(&format!("parent={full_name}")), "{sidecar}");

        let combined = base.join("combined");
        let output = Command::new(&combine)
            .args([
                "-o",
                combined.to_str().unwrap(),
                first.archive_path.to_str().unwrap(),
                second.archive_path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(combined.join("PG_VERSION").is_file());
    }

    #[cfg(unix)]
    fn week_key(stamp: Timestamp) -> (i16, i8) {
        let week = stamp.to_zoned(TimeZone::system()).date().iso_week_date();
        (week.year(), week.week())
    }
}
