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

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use moqui_lib::{error::Error, private_fs};

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
    install_into(
        &dest,
        source.as_deref(),
        data,
        force,
        &std::env::current_exe().map_err(|err| Error::new(err.to_string()))?,
    )
    .map_err(|e| Error::new(format!("install {}: {e}", dest.display())))?;
    println!(
        "Installed in {}. Add this directory to PATH and run moqui --installed.",
        dest.display()
    );
    Ok(())
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
        || (!data
            && ["db", "txlog", ".env", "cert", "opensearch/data"]
                .iter()
                .any(|s| rel.starts_with(s)))
    {
        return true;
    }
    let Some(name) = rel.file_name().and_then(|s| s.to_str()) else {
        return true;
    };
    name == ".git"
        || (rel.starts_with("components") && moqui_embedded::select::is_excluded(name, rel.components().count() == 4))
}

fn stage(source: &Path, staged: &Path, rel: &Path, data: bool) -> io::Result<()> {
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
            stage(source, staged, &next, data)?;
        } else if kind.is_file() {
            fs::copy(item.path(), to)?;
        } else {
            return Err(io::Error::other(format!(
                "{} is not a regular file or directory",
                item.path().display()
            )));
        }
    }
    Ok(())
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

fn install_into(dest: &Path, source: Option<&Path>, data: bool, force: bool, executable: &Path) -> io::Result<()> {
    if source.is_some_and(|s| !moqui_core::context::layout::is_runtime(s)) {
        return Err(io::Error::other("--from needs a runtime with components/base"));
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
            stage(source, &staged, Path::new(""), data)?
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
        Ok(())
    })();
    let cleanup = fs::remove_dir_all(staged);
    result.and(cleanup)
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
