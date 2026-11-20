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

//! Which runtime files are built into the binary at each [`Level`], and how they are packed: build.rs includes this
//! file (`#[path]`), and the crate has it too, so the rules are the ones its tests check on scratch runtimes.
//!
//! Each file is deflated on its own into one blob, so one file is read without unpacking the rest, and an index lists
//! them, one line each: `name<TAB>offset<TAB>packed length<TAB>size`, sorted by name. Both are the same for the same
//! files.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// How much of the runtime is built in: the cargo features `production` and `all` of this crate (`embed-production`
/// and `embed-all` of the binary, `./moq build --embed <level>`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// The framework and tools components less their `test/`, the webroot stub `webroot-files.txt` names, the skeleton
    /// files and the runtime's confs: what the binary's own code needs.
    Framework,
    /// What `./moq archive` puts in by default: the framework level with the whole webroot, and every other component of
    /// the base and add-on sets, each less its top-level `test/`, `plan/` and `plans/`.
    Production,
    /// The production level with each component's `test/`, `plan/` and `plans/`, and the dev and test sets: `./moq
    /// archive --with-tests --with-dev`.
    All,
}

impl Level {
    /// Every level, smallest first.
    pub const ALL: [Level; 3] = [Level::Framework, Level::Production, Level::All];

    /// The level the crate's features select: `all` over `production` over none.
    #[must_use]
    pub const fn from_features(production: bool, all: bool) -> Self {
        if all {
            Self::All
        } else if production {
            Self::Production
        } else {
            Self::Framework
        }
    }

    /// `framework`, `production` or `all`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Framework => "framework",
            Self::Production => "production",
            Self::All => "all",
        }
    }

    /// The level named `name` ([`Level::name`]).
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.name() == name)
    }

    /// The top-level directories of a component that are neither packed nor compared at this level: what only the
    /// tests (the regression and acceptance suites) and the plans need.
    #[must_use]
    pub fn component_skip(self) -> &'static [&'static str] {
        match self {
            Self::Framework => &["test"],
            Self::Production => &["test", "plan", "plans"],
            Self::All => &[],
        }
    }

    /// The component sets of `runtime/components` whose components, other than the [`OWN_SETS`], are packed whole as
    /// the components set.
    #[must_use]
    pub fn component_sets(self) -> &'static [&'static str] {
        match self {
            Self::Framework => &[],
            Self::Production => &["base", "addon"],
            Self::All => &["base", "addon", "dev", "test"],
        }
    }

    /// Whether the webroot component is packed whole, in place of the stub `webroot-files.txt` names.
    #[must_use]
    pub fn whole_webroot(self) -> bool {
        self != Self::Framework
    }
}

/// The base components packed as sets of their own (the framework may be elsewhere, `MOQUI_FRAMEWORK`), which the
/// components set leaves out.
pub const OWN_SETS: [&str; 3] = ["framework", "tools", "webroot"];

/// The vault's browser module under the webroot: built on demand by `scripts/vault-wasm.sh` (which `./moq build` runs
/// before cargo), not kept in git, so a level that packs the whole webroot refuses a runtime without it.
pub const VAULT_MODULE: &str = "screen/webroot/js/vault/moqui_vault.wasm";

/// The placeholder page at `/` of a runtime with no web client, in the skeleton; left out when the whole webroot,
/// with the web client's own page, is packed.
pub const PLACEHOLDER: &str = "skeleton/components/base/webroot/screen/webroot/qapps.xml";

/// The names `./moq archive` leaves out of the runtime's components too (`archive-excludes.txt`).
const EXCLUDES: &str = include_str!("../archive-excludes.txt");

/// Whether a file or directory named `name` is left out: a dot file, or a name `archive-excludes.txt` lists. `top` is
/// whether it is at the top of a component, where the names ending in `/` are left out too.
#[must_use]
pub fn is_excluded(name: &str, top: bool) -> bool {
    name.starts_with('.')
        || EXCLUDES
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .any(|pattern| {
                let (pattern, top_only) = match pattern.strip_suffix('/') {
                    Some(pattern) => (pattern, true),
                    None => (pattern, false),
                };
                (top || !top_only)
                    && match pattern.strip_prefix('*') {
                        Some(end) => name.ends_with(end),
                        None => name == pattern,
                    }
            })
}

/// The files to pack from the runtime `runtime` at `level`, each with its packed name (`<set>/<path>`), sorted by name:
/// the framework and tools components; the webroot, whole or the stub `crate_dir/webroot-files.txt` names; the
/// `crate_dir/skeleton` tree with the sets' manifest; at production and above every other component of the
/// [`Level::component_sets`] under `components/`; and the runtime's `conf/`. Each component is less the
/// [`Level::component_skip`] directories at its top and every [`is_excluded`] name. `watch` is given each path whose
/// change should pack again (build.rs prints it for cargo).
///
/// # Errors
///
/// Returns an error naming the path when a directory or the webroot list cannot be read, the list names a path that
/// is not in the webroot, or the whole webroot is packed without the vault's module ([`VAULT_MODULE`]).
pub fn gather(
    runtime: &Path,
    crate_dir: &Path,
    level: Level,
    watch: &mut dyn FnMut(&Path),
) -> io::Result<Vec<(String, PathBuf)>> {
    let base = runtime.join("components/base");
    let skip = level.component_skip();
    let mut files: Vec<(String, PathBuf)> = Vec::new();

    for component in ["framework", "tools"] {
        pack_component(&base.join(component), component, skip, watch, &mut files)?;
    }

    let webroot = base.join("webroot");
    if level.whole_webroot() {
        let module = webroot.join(VAULT_MODULE);
        watch(&module);
        if !module.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{} is not built: run scripts/vault-wasm.sh (./moq build runs it) before a build with the \
                     production or all runtime built in",
                    module.display()
                ),
            ));
        }
        pack_component(&webroot, "webroot", skip, watch, &mut files)?;
    } else {
        let list = crate_dir.join("webroot-files.txt");
        watch(&list);
        let listed = std::fs::read_to_string(&list).map_err(|err| named(err, "could not read", &list))?;
        for line in listed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let relative = line.trim_end_matches('/');
            let path = webroot.join(relative);
            if !path.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "webroot-files.txt names {relative}, which is not in {}",
                        webroot.display()
                    ),
                ));
            }
            watch(&path);
            collect(&path, &format!("webroot/{relative}"), &mut files)?;
        }
    }

    let skeleton = crate_dir.join("skeleton");
    watch(&skeleton);
    collect(&skeleton, "skeleton", &mut files)?;
    if level.whole_webroot() {
        files.retain(|(name, _)| name != PLACEHOLDER);
    }
    // the sets' manifest a skeleton runtime needs, since only the sets it lists load (plans/archive/Components-Dir.md)
    let sets = runtime.join("components/components.xml");
    watch(&sets);
    files.push(("skeleton/components/components.xml".to_string(), sets));

    for set in level.component_sets() {
        let dir = runtime.join("components").join(set);
        let names = match top_names(&dir) {
            Ok(names) => names,
            // a set the runtime does not have (no test components) packs nothing
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(named(err, "could not read", &dir)),
        };
        for name in names {
            if is_excluded(&name, false) || (*set == "base" && OWN_SETS.contains(&name.as_str())) {
                continue;
            }
            let path = dir.join(&name);
            let packed = format!("components/{set}/{name}");
            if path.is_dir() {
                pack_component(&path, &packed, skip, watch, &mut files)?;
            } else {
                watch(&path);
                files.push((packed, path));
            }
        }
    }

    // the example and starter confs (runtime/conf/README.md)
    let conf = runtime.join("conf");
    watch(&conf);
    for name in top_names(&conf).map_err(|err| named(err, "could not read", &conf))? {
        if !is_excluded(&name, false) {
            let path = conf.join(&name);
            watch(&path);
            collect(&path, &format!("conf/{name}"), &mut files)?;
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    files.dedup_by(|a, b| a.0 == b.0);
    Ok(files)
}

/// The blob and the index of `files` (`gather`): each file deflated on its own, in order.
///
/// # Errors
///
/// Returns an error naming the path when a file cannot be read, or a name has a tab or a line break.
pub fn pack(files: &[(String, PathBuf)]) -> io::Result<(Vec<u8>, String)> {
    let mut blob = Vec::new();
    let mut index = String::new();
    for (name, path) in files {
        if name.contains(['\t', '\n', '\r']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name}: a packed file name has no tab or line break"),
            ));
        }
        let bytes = std::fs::read(path).map_err(|err| named(err, "could not read", path))?;
        let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&bytes)?;
        let packed = encoder.finish()?;
        index.push_str(&format!("{name}\t{}\t{}\t{}\n", blob.len(), packed.len(), bytes.len()));
        blob.extend_from_slice(&packed);
    }
    Ok((blob, index))
}

/// Pack the component in `dir` under `prefix`, less the `skip` directories and the [`is_excluded`] names at its top,
/// watching each top-level entry rather than the directory, so a change in what is not packed (its tests, `.git`, a
/// crate's `target/`) does not pack again.
fn pack_component(
    dir: &Path,
    prefix: &str,
    skip: &[&str],
    watch: &mut dyn FnMut(&Path),
    files: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    for name in top_names(dir).map_err(|err| named(err, "could not read", dir))? {
        if is_excluded(&name, true) || skip.contains(&name.as_str()) {
            continue;
        }
        let path = dir.join(&name);
        watch(&path);
        collect(&path, &format!("{prefix}/{name}"), files)?;
    }
    Ok(())
}

/// The names in `dir`, sorted.
fn top_names(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// Every file under `path` (or `path` itself), named `prefix/<relative path>`, with `/` separators; the
/// [`is_excluded`] names left out.
fn collect(path: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    if path.is_file() {
        files.push((prefix.to_string(), path.to_path_buf()));
        return Ok(());
    }
    for name in top_names(path).map_err(|err| named(err, "could not read", path))? {
        if is_excluded(&name, false) {
            continue;
        }
        collect(&path.join(&name), &format!("{prefix}/{name}"), files)?;
    }
    Ok(())
}

fn named(err: io::Error, what: &str, path: &Path) -> io::Error {
    io::Error::new(err.kind(), format!("{what} {}: {err}", path.display()))
}
