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

//! The framework and webroot stub files built into the binary (`crates/moqui-embedded`): where a runtime goes when there
//! is none, the skeleton runtime written there, the startup comparison of the files on disk with the built-in ones, and
//! `moqui embedded`.

use std::path::{Path, PathBuf};

use moqui_embedded::{Kind, Set};
use moqui_lib::error::Error;

/// Where a runtime goes when none was found: `runtime` beside the executable when its directory is writable, so a
/// binary in `/opt/moqui` gets `/opt/moqui/runtime` from any working directory, else `local` (`runtime` in the working
/// directory), as for a binary in `/usr/bin`.
pub(crate) fn new_runtime_dir(local: &Path, exe: Option<&Path>) -> PathBuf {
    exe.and_then(Path::parent)
        .filter(|dir| is_writable(dir))
        .map_or_else(|| local.to_path_buf(), |dir| dir.join("runtime"))
}

/// Whether a file can be made in `dir`: one is made and removed, as permission bits do not say it for every user.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".moqui-write-check-{}", std::process::id()));
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .is_ok();
    if made {
        let _ = std::fs::remove_file(&probe);
    }
    made
}

/// Write a skeleton runtime from the built-in files when `framework_dir` is this runtime's own framework directory (no
/// `MOQUI_FRAMEWORK` elsewhere) and has no `component.xml`: the framework, and the webroot stub when there is no webroot.
///
/// # Errors
///
/// Returns an error naming the path when a file cannot be written.
pub(crate) fn ensure_runtime(runtime_dir: &Path, framework_dir: &Path) -> Result<(), Error> {
    if framework_dir != moqui_core::context::default_framework_dir(runtime_dir)
        || framework_dir.join("component.xml").is_file()
    {
        return Ok(());
    }
    let written = moqui_embedded::write_skeleton(runtime_dir).map_err(|err| {
        Error::new(format!(
            "could not write a skeleton runtime in {}",
            runtime_dir.display()
        ))
        .with_source(err)
    })?;
    tracing::warn!(
        "No runtime with a framework component was found; wrote a skeleton runtime of {} files in {} from the \
         framework and webroot built into this binary. It has the framework and the REST interface; the other base \
         and application components are not installed.",
        written.len(),
        runtime_dir.display()
    );
    Ok(())
}

/// Compare the booted factory's framework and webroot components with the built-in files and log the result: one
/// line when they match, else a summary and a line for each file. Runs on a thread of its own when `background`, so
/// the server's startup does not wait for it.
pub(crate) fn check(factory: &moqui_core::context::ExecutionContextFactory, background: bool) {
    let framework = factory.framework_dir().to_path_buf();
    let webroot = factory
        .components()
        .iter()
        .find(|component| component.name == "webroot")
        .map(|component| component.location.clone());
    if !background {
        log_differences(&framework, webroot.as_deref());
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("embedded-check".into())
        .spawn(move || log_differences(&framework, webroot.as_deref()));
    if let Err(err) = spawned {
        tracing::warn!("Could not start the comparison of the framework files with the built-in ones: {err}");
    }
}

fn log_differences(framework: &Path, webroot: Option<&Path>) {
    let differences = match moqui_embedded::compare(framework, webroot) {
        Ok(differences) => differences,
        Err(err) => {
            tracing::error!("Could not read the framework files built into this binary: {err}");
            return;
        }
    };
    let compared = if webroot.is_some() {
        "framework and webroot"
    } else {
        "framework (no webroot component)"
    };
    if differences.is_empty() {
        tracing::info!("The {compared} files match the ones built into this binary");
        return;
    }
    tracing::warn!(
        "{} {compared} files differ from the ones built into this binary; the files on disk are used. \
         `moqui embedded diff` shows the changes.",
        differences.len()
    );
    for difference in &differences {
        tracing::warn!("Differs from the binary: {difference}");
    }
}

/// `moqui embedded list | diff [path] | extract <dir>`.
///
/// # Errors
///
/// Returns an error for an unknown subcommand or path, or when the files cannot be read or written.
pub(crate) fn dispatch(args: &[String]) -> Result<(), Error> {
    match args {
        [command] if command == "list" => list(),
        [command] if command == "diff" => diff_all(),
        [command, path] if command == "diff" => diff_one(path),
        [command, dir] if command == "extract" => extract(Path::new(dir)),
        _ => Err(Error::new(
            "usage: moqui embedded list | diff [framework/<path> | webroot/<path>] | extract <runtime dir>",
        )),
    }
}

fn io_error(err: std::io::Error) -> Error {
    Error::new(err.to_string())
}

fn list() -> Result<(), Error> {
    for entry in moqui_embedded::entries().map_err(io_error)? {
        println!("{}/{} {}", entry.set.name(), entry.path, entry.bytes.len());
    }
    Ok(())
}

/// The framework and webroot directories `moqui` would boot with, without booting: the webroot beside the framework.
fn dirs() -> (PathBuf, PathBuf) {
    let (_, framework) = super::runtime_framework_dirs();
    let webroot = framework
        .parent()
        .map_or_else(|| PathBuf::from("webroot"), |base| base.join("webroot"));
    (framework, webroot)
}

fn diff_all() -> Result<(), Error> {
    let (framework, webroot) = dirs();
    let webroot = webroot.is_dir().then_some(webroot);
    let differences = moqui_embedded::compare(&framework, webroot.as_deref()).map_err(io_error)?;
    if differences.is_empty() {
        println!(
            "The files in {} match the ones built into this binary",
            framework.display()
        );
    }
    for difference in differences {
        if difference.kind == Kind::Added {
            println!("Only on disk: {}/{}", difference.set.name(), difference.path);
            continue;
        }
        if difference.kind == Kind::Missing {
            println!(
                "Only in the binary: {0}/{1} (moqui embedded diff {0}/{1} prints it)",
                difference.set.name(),
                difference.path
            );
            continue;
        }
        if let Kind::Unreadable(error) = &difference.kind {
            println!("Could not read {}/{}: {error}", difference.set.name(), difference.path);
            continue;
        }
        let dir = if difference.set == Set::Webroot {
            webroot.as_deref()
        } else {
            Some(framework.as_path())
        };
        if let Some(dir) = dir {
            print_diff(difference.set, &difference.path, dir)?;
        }
    }
    Ok(())
}

fn diff_one(path: &str) -> Result<(), Error> {
    let (framework, webroot) = dirs();
    let (set, dir, relative) = if let Some(relative) = path.strip_prefix("framework/") {
        (Set::Framework, framework, relative)
    } else if let Some(relative) = path.strip_prefix("webroot/") {
        (Set::Webroot, webroot, relative)
    } else {
        return Err(Error::new(format!(
            "{path}: name a built-in file as framework/<path> or webroot/<path> (moqui embedded list shows them)"
        )));
    };
    if !print_diff(set, relative, &dir)? {
        println!("{path} matches the one built into this binary");
    }
    Ok(())
}

/// Print the diff of one file; false when it matches.
fn print_diff(set: Set, path: &str, dir: &Path) -> Result<bool, Error> {
    match moqui_embedded::unified_diff(set, path, dir).map_err(io_error)? {
        Some(diff) => {
            print!("{diff}");
            Ok(true)
        }
        None => Ok(false),
    }
}

fn extract(runtime_dir: &Path) -> Result<(), Error> {
    let written = moqui_embedded::write_skeleton(runtime_dir).map_err(io_error)?;
    println!("Wrote {} files into {}", written.len(), runtime_dir.display());
    Ok(())
}

#[cfg(test)]
#[path = "embedded_files_tests.rs"]
mod tests;
