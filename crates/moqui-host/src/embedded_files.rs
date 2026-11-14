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

//! The framework, tools, webroot stub and conf files built into the binary (`crates/moqui-embedded`): mounted on the
//! runtime so each one that is not on disk is read from the binary (`moqui_lib::vfs`), the basic structure of a new
//! runtime, the startup comparison of the files on disk with the built-in ones, and `moqui embedded`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moqui_embedded::{Kind, Set};
use moqui_lib::error::Error;

/// One built-in set, mounted on its directory of a runtime ([`Set::dir_in`]).
struct Builtin(Set);

impl moqui_lib::vfs::Overlay for Builtin {
    fn read(&self, path: &str) -> std::io::Result<Option<Vec<u8>>> {
        moqui_embedded::file(self.0, path)
    }

    fn is_file(&self, path: &str) -> bool {
        moqui_embedded::is_file(self.0, path)
    }

    fn is_dir(&self, path: &str) -> bool {
        moqui_embedded::is_dir(self.0, path)
    }

    fn children(&self, dir: &str) -> Vec<(String, bool)> {
        moqui_embedded::children(self.0, dir)
    }
}

/// Mount the built-in files on the runtime `runtime_dir` whose framework component is `framework_dir`, so a file of
/// theirs that is not on disk is read from the binary: the framework, tools and the webroot stub, the sets' manifest,
/// the placeholder page at `/` and the confs. Mounting again replaces the mounts.
pub(crate) fn mount(runtime_dir: &Path, framework_dir: &Path) {
    for set in Set::ALL {
        moqui_lib::vfs::mount(&set.dir_in(runtime_dir, framework_dir), Arc::new(Builtin(set)));
    }
}

/// Make the basic structure of the runtime `runtime_dir` (its base set's directory, with the framework, tools and
/// webroot component directories), which the boot and the runtime lookup need on disk, and mount the built-in files on
/// it ([`mount`]). Nothing else is written: each file not on disk is read from the binary, until `moqui embedded
/// extract` writes them out.
///
/// # Errors
///
/// Returns an error naming the directory when one cannot be made.
pub(crate) fn ensure_runtime(runtime_dir: &Path, framework_dir: &Path) -> Result<(), Error> {
    let mut dirs = vec![
        Set::Tools.dir_in(runtime_dir, framework_dir),
        Set::Webroot.dir_in(runtime_dir, framework_dir),
    ];
    // a framework MOQUI_FRAMEWORK names elsewhere is not made
    if framework_dir == moqui_core::context::default_framework_dir(runtime_dir) {
        dirs.push(framework_dir.to_path_buf());
    }
    for dir in &dirs {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
    }
    mount(runtime_dir, framework_dir);
    if !framework_dir.join("component.xml").is_file() {
        tracing::info!(
            "The runtime {} has no framework files on disk; it reads the framework, tools, webroot and conf files \
             built into this binary. `moqui embedded extract` writes them out. The web client (/qapps) and the \
             application components are not built in.",
            runtime_dir.display()
        );
    }
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
    let (built_in, differences): (Vec<_>, Vec<_>) = differences
        .into_iter()
        .partition(|difference| difference.kind == Kind::Missing);
    if !built_in.is_empty() {
        tracing::info!(
            "{} {compared} files are not on disk and are read from this binary (`moqui embedded extract` writes them \
             out)",
            built_in.len()
        );
    }
    if differences.is_empty() {
        tracing::info!("The {compared} files on disk match the ones built into this binary");
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

/// `moqui embedded list | diff [path] | extract [dir]`.
///
/// # Errors
///
/// Returns an error for an unknown subcommand or path, or when the files cannot be read or written.
pub(crate) fn dispatch(args: &[String]) -> Result<(), Error> {
    match args {
        [command] if command == "list" => list(),
        [command] if command == "diff" => diff_all(),
        [command, path] if command == "diff" => diff_one(path),
        [command] if command == "extract" => {
            let (runtime, framework) = super::runtime_framework_dirs();
            extract(&runtime, &framework)
        }
        [command, dir] if command == "extract" => {
            let runtime = PathBuf::from(dir);
            let framework = moqui_core::context::default_framework_dir(&runtime);
            extract(&runtime, &framework)
        }
        _ => Err(Error::new(
            "usage: moqui embedded list | diff [<set>/<path>] | extract [<runtime dir>]",
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

/// The component directories `moqui` would boot with, without booting: the framework, tools and webroot of the runtime.
fn runtime_dirs() -> Vec<(Set, PathBuf)> {
    let (runtime, framework) = super::runtime_framework_dirs();
    Set::COMPONENTS
        .into_iter()
        .map(|set| (set, set.dir_in(&runtime, &framework)))
        .collect()
}

fn diff_all() -> Result<(), Error> {
    let dirs = runtime_dirs();
    let differences = moqui_embedded::compare(&borrowed(&dirs)).map_err(io_error)?;
    let (built_in, differences): (Vec<_>, Vec<_>) = differences
        .into_iter()
        .partition(|difference| difference.kind == Kind::Missing);
    if !built_in.is_empty() {
        println!(
            "{} {} files are not on disk and are read from this binary (moqui embedded extract writes them out)",
            built_in.len(),
            names(&dirs)
        );
    }
    if differences.is_empty() {
        println!(
            "The {} files on disk match the ones built into this binary",
            names(&dirs)
        );
    }
    for difference in differences {
        let name = difference.set.name();
        let path = &difference.path;
        match &difference.kind {
            Kind::Added => println!("Only on disk: {name}/{path}"),
            Kind::Missing => {}
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
        let set = Set::named(name).filter(|set| *set != Set::Skeleton)?;
        Some((set, relative))
    });
    let Some((set, relative)) = named else {
        return Err(Error::new(format!(
            "{path}: name a built-in file as framework/<path>, tools/<path>, webroot/<path> or conf/<path> (moqui \
             embedded list shows them)"
        )));
    };
    let dir = match dirs.iter().find(|(known, _)| *known == set) {
        Some((_, dir)) => dir.clone(),
        None => {
            let (runtime, framework) = super::runtime_framework_dirs();
            set.dir_in(&runtime, &framework)
        }
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

/// Write each built-in file that is not on disk into the runtime, so it reads none from the binary.
fn extract(runtime_dir: &Path, framework_dir: &Path) -> Result<(), Error> {
    let written = moqui_embedded::write_runtime(runtime_dir, framework_dir).map_err(io_error)?;
    if written.is_empty() {
        println!("{} has every built-in file already", runtime_dir.display());
    } else {
        println!("Wrote {} built-in files into {}", written.len(), runtime_dir.display());
    }
    Ok(())
}

#[cfg(test)]
#[path = "embedded_files_tests.rs"]
mod tests;
