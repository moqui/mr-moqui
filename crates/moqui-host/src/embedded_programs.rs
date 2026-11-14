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

//! The OpenSearch and Chromium built into the binary (`moqui_embedded::programs`, the `embed-opensearch` and
//! `embed-chromium` features), unpacked into the install directory (`<MOQUI_INSTALL or runtime/install>/<program>/
//! <version>`, beside the embedded Postgres) the first time they are needed, and run from there as their own
//! processes. Each is unpacked beside its directory and renamed into place, so a process that stops part way leaves no
//! half install and two processes that unpack at once both end with one. Postgres is unpacked by moqui-drivers.

use std::path::{Path, PathBuf};

use moqui_embedded::programs::Archive;
use moqui_lib::error::Error;

/// A program built into the binary that runs as its own process, other than Postgres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Program {
    OpenSearch,
    Chromium,
}

impl Program {
    /// `opensearch` or `chromium`: its directory in the install directory.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::OpenSearch => "opensearch",
            Self::Chromium => "chromium",
        }
    }

    /// The archive built in, when there is one.
    pub(crate) fn archive(self) -> Option<Archive> {
        match self {
            Self::OpenSearch => moqui_embedded::programs::opensearch(),
            Self::Chromium => moqui_embedded::programs::chromium(),
        }
    }

    /// The file whose presence means `dir` holds the unpacked program.
    fn marker(self, dir: &Path) -> PathBuf {
        match self {
            Self::OpenSearch => dir.join("bin/opensearch"),
            Self::Chromium => chromium_binary(dir),
        }
    }
}

/// The Chromium binary in an unpacked snapshot: `chrome` (Linux) or the app bundle's binary (macOS).
pub(crate) fn chromium_binary(dir: &Path) -> PathBuf {
    let mac = dir.join("Chromium.app/Contents/MacOS/Chromium");
    if mac.is_file() {
        mac
    } else {
        dir.join("chrome")
    }
}

/// Where `program` built into the binary is (or will be) unpacked for the runtime `runtime_dir`; `None` when the
/// binary does not have it.
pub(crate) fn install_dir(program: Program, runtime_dir: &Path) -> Option<PathBuf> {
    let archive = program.archive()?;
    Some(
        crate::db::embedded::install_root(runtime_dir)
            .join(program.name())
            .join(archive.version),
    )
}

/// Unpack `program` built into the binary for the runtime `runtime_dir` when it is not there yet; its directory, or
/// `None` when the binary does not have it.
///
/// # Errors
///
/// Returns an error when the archive cannot be unpacked or moved into place.
pub(crate) fn unpack(program: Program, runtime_dir: &Path) -> Result<Option<PathBuf>, Error> {
    let (Some(archive), Some(dir)) = (program.archive(), install_dir(program, runtime_dir)) else {
        return Ok(None);
    };
    if program.marker(&dir).exists() {
        return Ok(Some(dir));
    }
    let started = std::time::Instant::now();
    install(archive.bytes, &dir, program.name(), &|dir| program.marker(dir).exists())?;
    tracing::info!(
        "Unpacked the embedded {} {} into {} in {} ms",
        program.name(),
        archive.version,
        dir.display(),
        started.elapsed().as_millis()
    );
    Ok(Some(dir))
}

/// Unpack the gzipped tar `bytes` into `dir`, dropping its one top directory, unless `done(dir)`: beside `dir` first,
/// then renamed into place.
///
/// # Errors
///
/// Returns an error naming `what` when the archive cannot be read, has an entry outside its directory, or cannot be
/// moved into place.
pub(crate) fn install(bytes: &[u8], dir: &Path, what: &str, done: &dyn Fn(&Path) -> bool) -> Result<(), Error> {
    if done(dir) {
        return Ok(());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| Error::new(format!("{} has no parent directory", dir.display())))?;
    std::fs::create_dir_all(parent)
        .map_err(|err| Error::new(format!("could not create {}", parent.display())).with_source(err))?;
    let name = dir
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let unpack = parent.join(format!(".unpack-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&unpack);
    if let Err(err) = unpack_tar_gz(bytes, &unpack, what) {
        let _ = std::fs::remove_dir_all(&unpack);
        return Err(err);
    }
    // An earlier install that was not done (a stopped unpack of another version layout) is replaced.
    if dir.exists() && !done(dir) {
        let _ = std::fs::remove_dir_all(dir);
    }
    if let Err(err) = std::fs::rename(&unpack, dir) {
        let _ = std::fs::remove_dir_all(&unpack);
        if !done(dir) {
            return Err(Error::new(format!("could not move the {what} install to {}", dir.display())).with_source(err));
        }
    }
    Ok(())
}

fn unpack_tar_gz(bytes: &[u8], dir: &Path, what: &str) -> Result<(), Error> {
    let failed =
        |err: std::io::Error| Error::new(format!("could not unpack the embedded {what} archive")).with_source(err);
    std::fs::create_dir_all(dir).map_err(failed)?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries().map_err(failed)? {
        let mut entry = entry.map_err(failed)?;
        // each archive holds one top directory (opensearch-<version>/, chrome-linux/), which is dropped
        let inner: PathBuf = entry.path().map_err(failed)?.components().skip(1).collect();
        if inner.as_os_str().is_empty() {
            continue;
        }
        if !inner
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(Error::new(format!(
                "the embedded {what} archive has an entry outside its directory: {}",
                inner.display()
            )));
        }
        let target = dir.join(&inner);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        entry.unpack(&target).map_err(failed)?;
    }
    Ok(())
}

/// Copy the directory `from` into `to`, which is not there yet: the OpenSearch install's `config/` seeded into the
/// runtime's `opensearch/config` on its first start.
///
/// # Errors
///
/// Returns an error naming the path when a directory or file cannot be read or written.
pub(crate) fn copy_dir(from: &Path, to: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(to)
        .map_err(|err| Error::new(format!("could not create {}", to.display())).with_source(err))?;
    let entries = std::fs::read_dir(from)
        .map_err(|err| Error::new(format!("could not read {}", from.display())).with_source(err))?;
    for entry in entries {
        let entry = entry.map_err(|err| Error::new(format!("could not read {}", from.display())).with_source(err))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|err| {
                Error::new(format!(
                    "could not copy {} to {}",
                    entry.path().display(),
                    target.display()
                ))
                .with_source(err)
            })?;
        }
    }
    Ok(())
}

/// `moqui embedded unpack postgres|opensearch|chromium|all`: unpack the programs built in for the runtime, as their
/// first use would, so a container image or a host has them before the first start.
///
/// # Errors
///
/// Returns an error for an unknown name, a program named that the binary does not have, or one that cannot be
/// unpacked.
pub(crate) fn unpack_command(runtime_dir: &Path, names: &[String]) -> Result<(), Error> {
    let all = names.is_empty() || names.iter().any(|name| name == "all");
    let names: Vec<&str> = if all {
        vec!["postgres", "opensearch", "chromium"]
    } else {
        names.iter().map(String::as_str).collect()
    };
    for name in names {
        let unpacked = match name {
            "postgres" => {
                if crate::db::embedded::archive().is_none() {
                    None
                } else {
                    Some(crate::db::embedded::unpack_install(runtime_dir)?)
                }
            }
            "opensearch" => unpack(Program::OpenSearch, runtime_dir)?,
            "chromium" => unpack(Program::Chromium, runtime_dir)?,
            other => {
                return Err(Error::new(format!(
                    "{other}: name a program as postgres, opensearch, chromium or all"
                )))
            }
        };
        match unpacked {
            Some(dir) => println!("{name}: {}", dir.display()),
            None if all => {}
            None => return Err(Error::new(format!("this binary does not embed {name}"))),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "embedded_programs_tests.rs"]
mod tests;
