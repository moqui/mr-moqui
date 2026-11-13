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

//! The framework, tools and webroot stub files built into the binary (`crates/moqui-embedded`): the skeleton runtime
//! written where there is none, the startup comparison of the files on disk with the built-in ones, and
//! `moqui embedded`.

use std::path::{Path, PathBuf};

use moqui_embedded::{Kind, Set};
use moqui_lib::error::Error;

/// Write a skeleton runtime from the built-in files when `framework_dir` is this runtime's own framework directory (no
/// `MOQUI_FRAMEWORK` elsewhere) and has no `component.xml`: the framework, and tools and the webroot stub when they
/// are not there.
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
         framework, tools and webroot built into this binary. It has the framework, the REST interface, the html \
         apps and the terminal client; the web client (/qapps) and the application components are not installed.",
        written.len(),
        runtime_dir.display()
    );
    Ok(())
}

/// The runtime's directory of each built-in component that the factory loaded.
fn factory_dirs(factory: &moqui_core::context::ExecutionContextFactory) -> Vec<(Set, PathBuf)> {
    let components = factory.components();
    Set::COMPONENTS
        .into_iter()
        .filter_map(|set| {
            if set == Set::Framework {
                return Some((set, factory.framework_dir().to_path_buf()));
            }
            components
                .iter()
                .find(|component| component.name == set.name())
                .map(|component| (set, component.location.clone()))
        })
        .collect()
}

/// Compare the booted factory's framework, tools and webroot components with the built-in files and log the result:
/// one line when they match, else a summary and a line for each file. Runs on a thread of its own when `background`,
/// so the server's startup does not wait for it.
pub(crate) fn check(factory: &moqui_core::context::ExecutionContextFactory, background: bool) {
    let dirs = factory_dirs(factory);
    if !background {
        log_differences(&dirs);
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("embedded-check".into())
        .spawn(move || log_differences(&dirs));
    if let Err(err) = spawned {
        tracing::warn!("Could not start the comparison of the framework files with the built-in ones: {err}");
    }
}

fn borrowed(dirs: &[(Set, PathBuf)]) -> Vec<(Set, &Path)> {
    dirs.iter().map(|(set, dir)| (*set, dir.as_path())).collect()
}

/// `framework, tools and webroot`: the names of the compared components.
fn names(dirs: &[(Set, PathBuf)]) -> String {
    let names: Vec<&str> = dirs.iter().map(|(set, _)| set.name()).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn log_differences(dirs: &[(Set, PathBuf)]) {
    let differences = match moqui_embedded::compare(&borrowed(dirs)) {
        Ok(differences) => differences,
        Err(err) => {
            tracing::error!("Could not read the framework files built into this binary: {err}");
            return;
        }
    };
    let compared = names(dirs);
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
            "usage: moqui embedded list | diff [<component>/<path>] | extract <runtime dir>",
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

/// The component directories `moqui` would boot with, without booting: the framework, and tools and webroot beside it
/// when they are there.
fn runtime_dirs() -> Vec<(Set, PathBuf)> {
    let (_, framework) = super::runtime_framework_dirs();
    let base = framework.parent().map(Path::to_path_buf).unwrap_or_default();
    Set::COMPONENTS
        .into_iter()
        .filter_map(|set| match set {
            Set::Framework => Some((set, framework.clone())),
            _ => Some((set, base.join(set.name()))).filter(|(_, dir)| dir.is_dir()),
        })
        .collect()
}

fn diff_all() -> Result<(), Error> {
    let dirs = runtime_dirs();
    let differences = moqui_embedded::compare(&borrowed(&dirs)).map_err(io_error)?;
    if differences.is_empty() {
        println!("The {} files match the ones built into this binary", names(&dirs));
    }
    for difference in differences {
        let name = difference.set.name();
        let path = &difference.path;
        match &difference.kind {
            Kind::Added => println!("Only on disk: {name}/{path}"),
            Kind::Missing => {
                println!("Only in the binary: {name}/{path} (moqui embedded diff {name}/{path} prints it)")
            }
            Kind::Unreadable(error) => println!("Could not read {name}/{path}: {error}"),
            Kind::Changed { .. } => {
                if let Some((_, dir)) = dirs.iter().find(|(set, _)| *set == difference.set) {
                    print_diff(difference.set, path, dir)?;
                }
            }
        }
    }
    Ok(())
}

fn diff_one(path: &str) -> Result<(), Error> {
    let dirs = runtime_dirs();
    let named = path.split_once('/').and_then(|(name, relative)| {
        let set = Set::named(name).filter(|set| Set::COMPONENTS.contains(set))?;
        Some((set, relative))
    });
    let Some((set, relative)) = named else {
        return Err(Error::new(format!(
            "{path}: name a built-in file as framework/<path>, tools/<path> or webroot/<path> (moqui embedded list \
             shows them)"
        )));
    };
    let dir = dirs
        .iter()
        .find(|(known, _)| *known == set)
        .map_or_else(|| PathBuf::from(set.name()), |(_, dir)| dir.clone());
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
