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

//! The runtime files built into the binary (`crates/moqui-embedded`: the framework, tools, webroot stub and conf files,
//! and at the production and all levels the other components): mounted on the runtime so each one that is not on disk
//! is read from the binary (`moqui_lib::vfs`), the basic structure of a new runtime, the startup comparison of the files
//! on disk with the built-in ones, and `moqui embedded`.

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

/// The files of the per-user shared runtime (`~/.mr-moqui`), read in a run without `--installed` only where neither the
/// runtime's disk nor the binary has the file.
struct Directory(PathBuf);

impl moqui_lib::vfs::Overlay for Directory {
    fn disk_path(&self, path: &str) -> Option<PathBuf> {
        Some(self.0.join(path))
    }
    fn read(&self, path: &str) -> std::io::Result<Option<Vec<u8>>> {
        match std::fs::read(self.0.join(path)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    fn is_file(&self, path: &str) -> bool {
        self.0.join(path).is_file()
    }
    fn is_dir(&self, path: &str) -> bool {
        self.0.join(path).is_dir()
    }
    fn children(&self, path: &str) -> Vec<(String, bool)> {
        let mut entries = Vec::new();
        if let Ok(list) = std::fs::read_dir(self.0.join(path)) {
            for entry in list.flatten() {
                if let Ok(name) = entry.file_name().into_string() {
                    entries.push((name, entry.path().is_dir()));
                }
            }
        }
        entries
    }
}

/// Mount the built-in files on the runtime `runtime_dir` whose framework component is `framework_dir`, so a file of
/// theirs that is not on disk is read from the binary: the framework, tools and the webroot, the other built-in
/// components, the sets' manifest, the placeholder page at `/` and the confs. Behind them is the per-user shared
/// runtime's `components` and `conf` ([`moqui_lib::shared_runtime`]), unless it is the runtime itself (`--installed`),
/// so the order is the runtime on disk, the binary, then the shared runtime. Mounting again replaces the mounts.
/// Returns the shared runtime when it was mounted.
pub(crate) fn mount(runtime_dir: &Path, framework_dir: &Path) -> Option<PathBuf> {
    let shared = match moqui_lib::shared_runtime::directory() {
        Ok(dir) => dir
            .filter(|path| path.is_dir() && moqui_lib::vfs::normalize(path) != moqui_lib::vfs::normalize(runtime_dir)),
        Err(err) => {
            tracing::error!("{err}");
            None
        }
    };
    mount_with_shared(runtime_dir, framework_dir, shared.as_deref());
    shared
}

fn mount_with_shared(runtime_dir: &Path, framework_dir: &Path, shared: Option<&Path>) {
    for tree in ["components", "conf"] {
        let directory = shared.as_ref().map(|dir| dir.join(tree)).filter(|dir| dir.is_dir());
        let overlay = directory.map(|dir| Arc::new(Directory(dir)) as Arc<dyn moqui_lib::vfs::Overlay>);
        moqui_lib::vfs::mount_shared(&runtime_dir.join(tree), overlay);
    }
    for set in Set::ALL {
        moqui_lib::vfs::mount(&set.dir_in(runtime_dir, framework_dir), Arc::new(Builtin(set)));
    }
}

/// Make the basic structure of the runtime `runtime_dir` (its base set's directory, with the framework, tools and
/// webroot component directories, and a directory for each other built-in component), which the boot and the runtime
/// lookup need on disk, and mount the built-in files on it ([`mount`]). Nothing else is written: each file not on disk
/// is read from the binary, until `moqui embedded extract` writes them out.
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
    let components = Set::Components.dir_in(runtime_dir, framework_dir);
    dirs.extend(
        moqui_embedded::components()
            .iter()
            .map(|component| components.join(component)),
    );
    for dir in &dirs {
        std::fs::create_dir_all(dir)
            .map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
    }
    let shared = mount(runtime_dir, framework_dir);
    log_sources(runtime_dir, shared.as_deref());
    let manifest = framework_dir.join("component.xml");
    if !manifest.is_file() {
        if let Some(source) = moqui_lib::vfs::source_file(&manifest) {
            tracing::info!("Reading the framework component from {}", source.display());
            return Ok(());
        }
        let level = moqui_embedded::level();
        if level == moqui_embedded::Level::Framework {
            tracing::info!(
                "The runtime {} has no framework files on disk; it reads the framework, tools, webroot and conf files \
                 built into this binary. `moqui embedded extract` writes them out. The web client (/qapps) and the \
                 application components are not built in.",
                runtime_dir.display()
            );
        } else {
            tracing::info!(
                "The runtime {} has no framework files on disk; it reads the runtime built into this binary (the {} \
                 level: {} components with the web client and the confs). `moqui embedded extract` writes them out.",
                runtime_dir.display(),
                level.name(),
                moqui_embedded::components().len() + Set::FRAMEWORK.len()
            );
        }
    }
    Ok(())
}

/// Log where the runtime's files come from, once at boot: an installed runtime (`--installed`, `~/.mr-moqui` itself)
/// reads only its own directory and the binary, by design for production systems and desktop installs; another
/// runtime reads its directory, then the binary, then the shared runtime for the files neither has (the dev CA, a
/// shared add-on component).
fn log_sources(runtime_dir: &Path, shared: Option<&Path>) {
    if moqui_lib::env::get_property("MOQUI_INSTALLED_RUNTIME").is_some() {
        tracing::info!(
            "Runtime: installed at {} (moqui install); files come from here and the binary",
            runtime_dir.display()
        );
        if let Err(err) = crate::install::check_version(runtime_dir) {
            tracing::warn!("{err}");
        }
    } else if let Some(shared) = shared {
        tracing::info!(
            "Runtime: {}; missing files from the binary, then {}",
            runtime_dir.display(),
            shared.display()
        );
    } else {
        tracing::info!("Runtime: {}; missing files from the binary", runtime_dir.display());
    }
}

/// The runtime's directory of each built-in component set that the factory loaded: the framework, tools and webroot
/// components where they are, and the runtime's `components` directory for the others.
fn factory_dirs(factory: &moqui_core::context::ExecutionContextFactory) -> Vec<(Set, PathBuf)> {
    let components = factory.components();
    Set::COMPONENTS
        .into_iter()
        .filter_map(|set| match set {
            Set::Framework => Some((set, factory.framework_dir().to_path_buf())),
            Set::Components => Some((set, set.dir_in(factory.runtime_dir(), factory.framework_dir()))),
            _ => components
                .iter()
                .find(|component| component.name == set.name())
                .map(|component| (set, component.location.clone())),
        })
        .collect()
}

/// Compare the booted factory's components with the built-in files and log the result: one line when they match, else
/// a summary and a line for each file, as warnings for the framework, tools and webroot (a changed file may no longer
/// fit the binary's code) and as information for the other components. Runs on a thread of its own when
/// `background`, so the server's startup does not wait for it.
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

/// `framework, tools and webroot`: the names of the compared component sets, the components set as `other component`.
fn names(dirs: &[(Set, PathBuf)]) -> String {
    let names: Vec<&str> = dirs
        .iter()
        .map(|(set, _)| match set {
            Set::Components => "other component",
            _ => set.name(),
        })
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn log_differences(dirs: &[(Set, PathBuf)]) {
    // The components with browser libraries (`build/js-build.json`): the webroot and any other in a component set.
    let components = dirs
        .iter()
        .find(|(set, _)| *set == Set::Components)
        .map(|(_, dir)| dir.clone())
        .or_else(|| {
            dirs.iter()
                .find(|(set, _)| *set == Set::Webroot)
                .and_then(|(_, webroot)| webroot.parent()?.parent().map(Path::to_path_buf))
        });
    let mut missing = Vec::new();
    for set in components
        .iter()
        .flat_map(|dir| moqui_lib::vfs::read_dir(dir).into_iter().flatten())
    {
        for component in moqui_lib::vfs::read_dir(set.path()).into_iter().flatten() {
            missing.extend(missing_browser_libs(&component.path()));
        }
    }
    if !missing.is_empty() {
        tracing::error!("Missing browser libraries: {}. Run ./moq build-js", missing.join(", "));
    }
    let differences = match compare(dirs) {
        Ok(differences) => differences,
        Err(err) => {
            tracing::error!("Could not read the runtime files built into this binary: {err}");
            return;
        }
    };
    let (framework_dirs, other_dirs): (Vec<_>, Vec<_>) =
        dirs.iter().cloned().partition(|(set, _)| Set::FRAMEWORK.contains(set));
    let (framework, others): (Vec<_>, Vec<_>) = differences
        .into_iter()
        .partition(|difference| Set::FRAMEWORK.contains(&difference.set));
    log_set_differences(&framework_dirs, framework, tracing::Level::WARN);
    if !moqui_embedded::components().is_empty() {
        log_set_differences(&other_dirs, others, tracing::Level::INFO);
    }
}

/// The libraries of the component at `root` whose files its manifest lists and the runtime lacks, each as
/// `component/library`; none when the component has no `build/js-build.json`. A file of an optional font pack counts
/// only when `fonts-extra/installed.json` says the pack is installed.
fn missing_browser_libs(root: &Path) -> Vec<String> {
    let Ok(config) = moqui_lib::vfs::read(root.join("build/js-build.json")) else {
        return Vec::new();
    };
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some(output) = serde_json::from_slice::<serde_json::Value>(&config)
        .ok()
        .and_then(|config| config["output"].as_str().map(str::to_owned))
    else {
        return vec![format!("{name} (unreadable build/js-build.json)")];
    };
    let libs = root.join(output);
    let Some(manifest) = moqui_lib::vfs::read(libs.join("MANIFEST.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    else {
        return vec![format!("{name}/MANIFEST.json")];
    };
    let libraries: Vec<&serde_json::Value> = manifest["libraries"].as_array().into_iter().flatten().collect();
    let mut missing = Vec::new();
    let packs = libraries
        .iter()
        .flat_map(|library| library["files"].as_array().into_iter().flatten())
        .any(|file| file["pack"].is_string());
    let installed = if packs {
        let installed = moqui_lib::vfs::read(libs.join("fonts-extra/installed.json"))
            .ok()
            .and_then(|data| serde_json::from_slice::<Vec<String>>(&data).ok());
        if installed.is_none() {
            missing.push(format!("{name}/fonts-extra/installed.json"));
        }
        installed.unwrap_or_default()
    } else {
        Vec::new()
    };
    for library in libraries {
        if library["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|f| {
                f["pack"]
                    .as_str()
                    .is_none_or(|pack| installed.iter().any(|name| name == pack))
            })
            .any(|f| {
                let base = if f["target"] == "server" {
                    root.join("script/libs")
                } else {
                    libs.clone()
                };
                f["path"]
                    .as_str()
                    .is_none_or(|path| !moqui_lib::vfs::is_file(base.join(path)))
            })
        {
            missing.push(format!("{name}/{}", library["id"].as_str().unwrap_or("unknown")));
        }
    }
    missing
}

/// Reconcile the two physical layers before reporting the embedded fallback.
fn compare(dirs: &[(Set, PathBuf)]) -> std::io::Result<Vec<moqui_embedded::Difference>> {
    let mut local = moqui_embedded::compare(&borrowed(dirs))?;
    let Some(shared) = moqui_lib::shared_runtime::directory()?.filter(|dir| dir.is_dir()) else {
        return Ok(local);
    };
    let framework = moqui_core::context::layout::framework_dir(&shared);
    let shared_dirs: Vec<_> = dirs
        .iter()
        .map(|(set, _)| (*set, set.dir_in(&shared, &framework)))
        .collect();
    let differences = moqui_embedded::compare(&borrowed(&shared_dirs))?;
    local.retain(|d| d.kind != Kind::Missing || !selected_shared(dirs, d.set, &d.path));
    for d in differences {
        if d.kind != Kind::Missing && selected_shared(dirs, d.set, &d.path) {
            local.push(d);
        }
    }
    Ok(local)
}

fn selected_shared(dirs: &[(Set, PathBuf)], set: Set, path: &str) -> bool {
    let Some((_, dir)) = dirs.iter().find(|(s, _)| *s == set) else {
        return false;
    };
    let path = dir.join(path);
    !path.exists() && moqui_lib::vfs::source_file(&path).is_some()
}

/// Log the differences of the sets `dirs`: the files read from the binary, then the ones that differ at `level`.
fn log_set_differences(dirs: &[(Set, PathBuf)], differences: Vec<moqui_embedded::Difference>, level: tracing::Level) {
    if dirs.is_empty() {
        return;
    }
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
    let summary = format!(
        "{} {compared} files differ from the ones built into this binary; the files on disk are used. `moqui embedded \
         diff` shows the changes.",
        differences.len()
    );
    if level == tracing::Level::WARN {
        tracing::warn!("{summary}");
        for difference in &differences {
            tracing::warn!(
                "Differs from the binary ({}): {difference}",
                if selected_shared(dirs, difference.set, &difference.path) {
                    "shared runtime"
                } else {
                    "local runtime"
                }
            );
        }
    } else {
        tracing::info!("{summary}");
        for difference in &differences {
            tracing::info!(
                "Differs from the binary ({}): {difference}",
                if selected_shared(dirs, difference.set, &difference.path) {
                    "shared runtime"
                } else {
                    "local runtime"
                }
            );
        }
    }
}

/// `moqui embedded info | list | diff [path] | extract [dir] | checksum [part...] | unpack [program...]`.
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
        [command] if command == "info" => {
            info();
            Ok(())
        }
        [command, rest @ ..] if command == "checksum" => crate::embedded_checksum::print(rest),
        [command, rest @ ..] if command == "unpack" => {
            let (runtime, _) = super::runtime_framework_dirs();
            crate::embedded_programs::unpack_command(&runtime, rest)
        }
        _ => Err(Error::new(USAGE)),
    }
}

/// The usage line of `moqui embedded`.
const USAGE: &str = "usage: moqui embedded info | list | diff [<set>/<path>] | extract [<runtime dir>] | checksum \
                     [--env] [runtime|postgres|opensearch|chromium...] | unpack [postgres|opensearch|chromium|all...]";

/// What is built in: the runtime level with each set's files and size, the built-in components, and the programs
/// with their versions.
fn info() {
    let level = moqui_embedded::level();
    println!("Runtime level: {}", level.name());
    for (set, (count, size)) in moqui_embedded::sizes() {
        println!("  {:<11} {count:>6} files {:>10}", set.name(), human_size(size));
    }
    let components = moqui_embedded::components();
    if !components.is_empty() {
        println!("Components: {}", components.join(", "));
    }
    let program = |name: &str, archive: Option<(usize, &str)>| match archive {
        Some((len, version)) => println!("{name:<11} {version} ({} archive)", human_size(len as u64)),
        None => println!("{name:<11} not built in"),
    };
    program(
        "Postgres",
        crate::db::embedded::archive().map(|(bytes, version)| (bytes.len(), version)),
    );
    program(
        "OpenSearch",
        moqui_embedded::programs::opensearch().map(|archive| (archive.bytes.len(), archive.version)),
    );
    program(
        "Chromium",
        moqui_embedded::programs::chromium().map(|archive| (archive.bytes.len(), archive.version)),
    );
}

/// `size` bytes in KB, MB or GB, one decimal.
fn human_size(size: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let size = size as f64;
    if size >= 1e9 {
        format!("{:.1} GB", size / 1e9)
    } else if size >= 1e6 {
        format!("{:.1} MB", size / 1e6)
    } else {
        format!("{:.1} KB", size / 1e3)
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
    let differences = compare(&dirs).map_err(io_error)?;
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
            Kind::NotBuiltIn => println!("A component only on disk: {name}/{path}"),
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
            "{path}: name a built-in file as framework/<path>, tools/<path>, webroot/<path>, \
             components/<set>/<component>/<path> or conf/<path> (moqui embedded list shows them)"
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
    let physical = moqui_lib::vfs::source_file(&dir.join(path));
    let mut source = physical.as_deref();
    if let Some(file) = source {
        println!("{} comes from {}", path, file.display());
        for _ in Path::new(path).components() {
            source = source.and_then(Path::parent);
        }
    }
    match moqui_embedded::unified_diff(set, path, source.unwrap_or(dir)).map_err(io_error)? {
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
