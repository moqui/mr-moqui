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

//! The database and booted factory a test run uses.
//!
//! - `fresh`: a scratch runtime (the runtime's `conf` copied, `components/base` and each component linked) with its own
//!   embedded Postgres, loaded with every data type. Removed when the run ends, unless kept.
//! - `snapshot`: the same, but the loaded database is dumped to `runtime/save/test-<hash>/moqui.dump`, by a hash of the
//!   components the load reads, their entity and data files and the day; a later run that day that loads the same
//!   components with the same files restores the dump instead of loading. When a run ends it removes the other
//!   snapshots, superseded by a later day or changed files, that no run has used for an hour ([`prune_snapshots`]).
//! - `existing`: the runtime's own database as it is, the way `gradle test` ran against a loaded database. Tests change
//!   its data.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_lib::error::Error;

use crate::db::entity_db::EntityDb;

use super::DbMode;

/// A booted factory on a loaded database. Dropping it closes the database and removes a scratch runtime.
pub struct TestFixture {
    // Field order is drop order: the factory and facade go before the database they use.
    pub factory: Arc<ExecutionContextFactory>,
    facade: Option<EntityFacade>,
    /// The sim overlay cluster, started at a test's first `enter_sim` (Try in sim, an approval's preview), as a server
    /// has it; dropped after the facade.
    sim: Option<Arc<crate::db::sim::SimOverlay>>,
    _search: Option<crate::search::SearchStore>,
    /// Keep the configured PDF engine alive for specs and background jobs, just as the server does.
    pdf: Option<crate::PdfSlot>,
    db: Option<EntityDb>,
    /// The scratch runtime to remove, when there is one.
    scratch: Option<PathBuf>,
    /// The snapshot (`runtime/save/test-*`) this run restored or made: kept when the run ends, the others pruned.
    snapshot: Option<PathBuf>,
    /// Whether the run is on a scratch runtime (`fresh`, `snapshot`), removed or kept, rather than the runtime itself.
    pub on_scratch: bool,
    pub runtime_dir: PathBuf,
    /// Where `moqui.log` is written: the runtime's own log directory, which outlives a scratch runtime.
    pub log_dir: Option<PathBuf>,
}

impl Drop for TestFixture {
    fn drop(&mut self) {
        crate::wait_for_workers(&self.factory, std::time::Duration::from_secs(30));
        // As a server stops: the workers, the scheduler and a background hit flush end, the last hits are written,
        // and the tools close, all while the database is still up.
        self.factory.shutdown();

        drop(self.facade.take());
        drop(self.sim.take());
        drop(self.pdf.take());
        drop(self.db.take());
        if let Some(scratch) = self.scratch.take() {
            super::interrupt::untrack(&scratch);
            super::interrupt::remove_scratch(&scratch);
        }
        // the snapshots this run did not use, at its end: a later run today restores this one
        if let Some(snapshot) = self.snapshot.take() {
            if let Some(save) = snapshot.parent() {
                prune_snapshots(save, &snapshot);
            }
        }
    }
}

impl TestFixture {
    /// Shut the factory and the database down as a drop does, and leave the scratch runtime for a server to run on
    /// (`moqui test --serve`). Returns the scratch runtime to remove once that server stops: none when it is kept.
    pub fn close_keeping_runtime(mut self) -> Option<PathBuf> {
        // the snapshots are pruned by the drop below all the same
        let scratch = self.scratch.take();
        drop(self);
        scratch
    }
}

/// Open the database a run uses and boot a factory on it.
pub fn open(runtime: &Path, framework: &Path, mode: DbMode, keep: bool) -> Result<TestFixture, Error> {
    let runtime = canonical(runtime)?;
    let framework = canonical(framework)?;
    // The first attach wins: the log files stay in the runtime's log directory, not a scratch runtime that goes.
    let log_dir = crate::logging::attach_files(&runtime, None);
    let mut snapshot = None;
    let (run_runtime, scratch) = match mode {
        DbMode::None => return Err(Error::new("--db none does not open a database")),
        DbMode::Existing => (runtime.clone(), None),
        DbMode::Fresh | DbMode::Snapshot => {
            let install = crate::db::embedded::shared_install_dir(&runtime, &runtime.join("db/embedded"));
            super::interrupt::remove_stale(&install);
            let scratch = scratch_runtime(&runtime)?;
            // from here a signal stops its Postgres and removes it; the drop of the fixture does when the run ends
            super::interrupt::track(&scratch, install, keep);
            if keep {
                let _ = std::fs::write(scratch.join(super::interrupt::KEEP_MARK), "");
            }
            match prepare_database(&runtime, &framework, &scratch, mode) {
                Ok(used) => snapshot = used,
                Err(err) => {
                    if !keep {
                        super::interrupt::untrack(&scratch);
                        super::interrupt::remove_scratch(&scratch);
                    }
                    return Err(err);
                }
            }
            (scratch.clone(), (!keep).then_some(scratch))
        }
    };
    if keep {
        if let Some(path) = scratch.as_ref().or(Some(&run_runtime)) {
            tracing::info!("Test runtime is {}", path.display());
        }
    }
    let booted = boot(&run_runtime, &framework);
    match booted {
        Ok((factory, facade, sim, search, pdf, db)) => Ok(TestFixture {
            factory,
            facade: Some(facade),
            sim: Some(sim),
            _search: search,
            pdf: Some(pdf),
            db: Some(db),
            on_scratch: run_runtime != runtime,
            scratch,
            snapshot,
            runtime_dir: run_runtime,
            log_dir,
        }),
        Err(err) => {
            if let Some(scratch) = scratch {
                super::interrupt::untrack(&scratch);
                super::interrupt::remove_scratch(&scratch);
            }
            Err(err)
        }
    }
}

fn canonical(dir: &Path) -> Result<PathBuf, Error> {
    std::fs::canonicalize(dir)
        .map_err(|err| Error::new(format!("could not resolve {}", dir.display())).with_source(err))
}

fn io(context: &str) -> impl Fn(std::io::Error) -> Error + '_ {
    move |err| Error::new(context.to_string()).with_source(err)
}

/// A scratch runtime under `/tmp` (the Postgres socket path must stay under 108 bytes; on Windows the same short path
/// stays under `MAX_PATH`, which theseus can start): `conf` copied, and the component sets (`context::layout`; the
/// add-on set component by component) with their `components.xml`, the instance `MoquiConf.xml`, the shared Postgres
/// install and the downloaded Chromium linked. On Windows a directory is a junction and a file on the same volume is a
/// hard link. A failure removes what was already linked.
fn scratch_runtime(runtime: &Path) -> Result<PathBuf, Error> {
    use moqui_core::context::layout::{self, Set};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let scratch = PathBuf::from("/tmp").join(format!("mt-{}-{nanos:x}", std::process::id()));
    let built = (|| -> Result<(), Error> {
        std::fs::create_dir_all(layout::set_dir(&scratch, Set::Addon))
            .map_err(io("could not create the scratch runtime"))?;
        std::fs::create_dir_all(scratch.join("db")).map_err(io("could not create the scratch db directory"))?;
        copy_files(&runtime.join("conf"), &scratch.join("conf"))?;
        for set in Set::ALL {
            let dir = layout::set_dir(runtime, set);
            if set == Set::Addon {
                for (_, component) in layout::component_dirs(runtime, &[set]) {
                    if let Some(name) = component.file_name() {
                        link(&component, &layout::set_dir(&scratch, set).join(name))?;
                    }
                }
            } else if dir.is_dir() {
                link(&dir, &layout::set_dir(&scratch, set))?;
            }
        }
        if layout::manifest(runtime).is_file() {
            link(&layout::manifest(runtime), &layout::manifest(&scratch))?;
        }
        // The real runtime's install, after any move from where an earlier build put it.
        let install = crate::db::embedded::shared_install_dir(runtime, &runtime.join("db/embedded"));
        if let Some(root) = install.parent() {
            std::fs::create_dir_all(root).map_err(io("could not create the install directory"))?;
            link(root, &scratch.join("install"))?;
        }
        let instance_conf = runtime.join("MoquiConf.xml");
        if instance_conf.is_file() {
            link(&instance_conf, &scratch.join("MoquiConf.xml"))?;
        }
        // the Chromium of ./moq chromium-download, which PDF finds in the runtime it boots on
        // (chromium::downloaded_chromium)
        if runtime.join("chromium").is_dir() {
            link(&runtime.join("chromium"), &scratch.join("chromium"))?;
        }
        Ok(())
    })();
    if let Err(err) = built {
        super::interrupt::remove_scratch(&scratch);
        return Err(err);
    }
    Ok(scratch)
}

fn link(from: &Path, to: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
            .map_err(|err| Error::new(format!("could not link {}", to.display())).with_source(err))
    }
    #[cfg(windows)]
    {
        link_windows(from, to)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (from, to);
        Err(Error::new("moqui test needs symbolic links"))
    }
}

/// A directory becomes a junction, which needs no extra privilege. A file becomes a hard link on the same volume, so a
/// later write of `MoquiConf.xml` is the file the scratch reads; a different volume cannot hold a hard link, and the
/// file is copied. A junction cannot name a file. The files a scratch runtime links are `MoquiConf.xml` and
/// `components/components.xml`.
#[cfg(windows)]
fn link_windows(from: &Path, to: &Path) -> Result<(), Error> {
    let from = link_path(from);
    let to = link_path(to);
    let meta = std::fs::metadata(&from).map_err(|err| link_error(&to, err))?;
    if meta.is_dir() {
        junction::create(&from, &to).map_err(|err| link_error(&to, err))
    } else if std::fs::hard_link(&from, &to).is_ok() {
        Ok(())
    } else {
        std::fs::copy(&from, &to)
            .map(|_| ())
            .map_err(|err| link_error(&to, err))
    }
}

#[cfg(windows)]
fn link_error(to: &Path, err: std::io::Error) -> Error {
    Error::new(format!("could not link {}", to.display())).with_source(err)
}

/// `canonicalize` prefixes `\\?\`. A junction stores an NT path, and a second prefix is a link that does not open.
#[cfg(windows)]
fn link_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest.to_string())
    } else {
        path.to_path_buf()
    }
}

/// The runtime's `conf` with its subdirectories: a `MOQUI_CONF` may name a topology's conf, as
/// `conf/edge/MoquiEdgeConf.xml`. A symbolic link to a directory is not followed, so a loop cannot hold the copy up.
fn copy_files(from: &Path, to: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(to).map_err(io("could not create the scratch conf"))?;
    let entries = std::fs::read_dir(from).map_err(io("could not read the runtime conf"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            copy_files(&path, &to.join(entry.file_name()))?;
        } else if path.is_file() {
            std::fs::copy(&path, to.join(entry.file_name())).map_err(io("could not copy the runtime conf"))?;
        }
    }
    Ok(())
}

/// Load the test database into `scratch`, or restore it from its snapshot. Answers the snapshot's directory
/// (`runtime/save/test-*`) when the run restored or made one.
fn prepare_database(runtime: &Path, framework: &Path, scratch: &Path, mode: DbMode) -> Result<Option<PathBuf>, Error> {
    let cluster = scratch.join("db/embedded");
    let dump = match mode {
        DbMode::Snapshot => Some(
            runtime
                .join("save")
                .join(format!("test-{:016x}", data_hash(scratch, framework)?))
                .join("moqui.dump"),
        ),
        _ => None,
    };
    if let Some(dump) = dump.as_ref().filter(|dump| dump.is_file()) {
        let started = Instant::now();
        tracing::warn!("Restoring the test database from {}", dump.display());
        crate::db::embedded::load_save(scratch, &cluster, dump)?;
        tracing::warn!("Restored in {:.1}s", started.elapsed().as_secs_f64());
        let dir = dump.parent().map(Path::to_path_buf);
        if let Some(dir) = &dir {
            mark_used(dir);
        }
        return Ok(dir);
    }
    let started = Instant::now();
    tracing::warn!("Loading the test database (all data types) in {}", scratch.display());
    let flags = crate::LoadFlags {
        data_types: Some(vec!["all".to_string()]),
        // a snapshot keeps the artifact index of a dev instance, so each spec's server does not build it again from
        // every component's files (the dev-aide specs; moqui-devkit index)
        build_artifact_index: matches!(mode, DbMode::Snapshot),
        ..crate::LoadFlags::default()
    };
    crate::load_data(scratch, framework, &cluster, &flags)?;
    tracing::warn!("Loaded in {:.1}s", started.elapsed().as_secs_f64());
    if let Some(dump) = dump {
        if let Some(dir) = dump.parent() {
            std::fs::create_dir_all(dir).map_err(io("could not create the snapshot directory"))?;
        }
        crate::db::embedded::save_db(scratch, &cluster, &dump)?;
        tracing::warn!("Saved the test database to {}", dump.display());
        let dir = dump.parent().map(Path::to_path_buf);
        if let Some(dir) = &dir {
            mark_used(dir);
        }
        return Ok(dir);
    }
    Ok(None)
}

/// The file whose change time is a snapshot's last use: written at each restore and at its save.
const USED_MARK: &str = "used";
/// How long a snapshot no run has used is kept: another run of the same runtime may be restoring it now.
const SNAPSHOT_IDLE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Note that a run restored or made the snapshot at `dir` now. A failure is logged: the snapshot is then pruned by its
/// dump's age, as before the mark.
fn mark_used(dir: &Path) {
    if let Err(err) = std::fs::write(dir.join(USED_MARK), "") {
        tracing::warn!(error = %err, path = %dir.display(), "could not mark a test snapshot used");
    }
}

/// Remove the test snapshots (`test-*`) under `save` other than `kept`, each once no run has used it for
/// [`SNAPSHOT_IDLE`]: a snapshot is good for its day and its components' files, so another one is superseded, and the
/// hour leaves one a concurrent run of this runtime is restoring. Its last use is its [`USED_MARK`]'s change time, else
/// its dump's. A failure to remove one is logged (one already gone is not); the others still go.
fn prune_snapshots(save: &Path, kept: &Path) {
    let Ok(entries) = std::fs::read_dir(save) else {
        return;
    };
    let Some(cutoff) = SystemTime::now().checked_sub(SNAPSHOT_IDLE) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_snapshot = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("test-"));
        if !is_snapshot || path == kept {
            continue;
        }
        let used = [USED_MARK, "moqui.dump"]
            .iter()
            .find_map(|name| std::fs::metadata(path.join(name)).and_then(|meta| meta.modified()).ok());
        if used.is_some_and(|used| used < cutoff) {
            match std::fs::remove_dir_all(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed a test snapshot no run has used for an hour"),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    tracing::warn!(error = %err, path = %path.display(), "could not remove an old test snapshot")
                }
            }
        }
    }
}

/// A hash of the crate version and of what the load of `scratch` reads: the components a factory on it loads, by name
/// in load order (the conf and the instance purpose decide them: a dev component such as example loads only on a dev
/// instance), and the name, size and change time of each one's entity and data files, which hold the schema and the
/// data. A snapshot is made again when a file changes or a run loads another set of components: a server on a conf
/// without the dev components once saved a snapshot that a run with example then restored, without example's tables.
/// The day is part of the key: the demo data is dated from its load, so a snapshot is good for the day it was made.
/// The binary is not part of the key, so a relink keeps the snapshots (plans/archive/Test-Reduction.md, "Cheaper
/// machinery"); after a change to how the binary loads data, remove `runtime/save/test-*`.
fn data_hash(scratch: &Path, framework: &Path) -> Result<u64, Error> {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    // what a snapshot holds besides the data: the artifact index of a dev instance since this mark
    "artifact-index".hash(&mut hasher);
    // The day of the load, local: the demo data is dated from it (a task due today, an invoice ten days overdue), so a
    // snapshot of yesterday's load reads as a day old to the specs that count on it.
    jiff::Zoned::now().date().to_string().hash(&mut hasher);
    hash_tree(&framework.join("entity"), &mut hasher);
    hash_tree(&framework.join("data"), &mut hasher);
    let components = moqui_core::context::discover_components(scratch, framework)
        .map_err(|err| Error::new(format!("could not list the components of the test runtime: {err}")))?;
    for component in components {
        component.name.hash(&mut hasher);
        // the directory the scratch runtime links, the same in every run
        let location = std::fs::canonicalize(&component.location).unwrap_or(component.location);
        hash_tree(&location.join("entity"), &mut hasher);
        hash_tree(&location.join("data"), &mut hasher);
    }
    Ok(hasher.finish())
}

fn hash_tree(dir: &Path, hasher: &mut DefaultHasher) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            hash_tree(&path, hasher);
        } else if let Ok(meta) = std::fs::metadata(&path) {
            path.hash(hasher);
            meta.len().hash(hasher);
            meta.modified().ok().hash(hasher);
        }
    }
}

type Booted = (
    Arc<ExecutionContextFactory>,
    EntityFacade,
    Arc<crate::db::sim::SimOverlay>,
    Option<crate::search::SearchStore>,
    crate::PdfSlot,
    EntityDb,
);

/// Boot the factory, open its database, and attach the entity facade and search, as `load` does.
fn boot(runtime: &Path, framework: &Path) -> Result<Booted, Error> {
    // A test run merges each component's test/MoquiConf.xml (plans/archive/ClientTestHarness.md 6).
    let options = moqui_core::context::BootOptions {
        test_confs: true,
        ..moqui_core::context::BootOptions::default()
    };
    let factory = ExecutionContextFactory::boot_with_options(runtime, framework, options)
        .map_err(|err| Error::new(err.to_string()))?;
    let conf = crate::db::entity_db::transactional_conf(&factory)?;
    let pool = conf.pool.clone();
    let cluster = runtime.join("db/embedded");
    std::fs::create_dir_all(&cluster).map_err(io("could not create the cluster directory"))?;
    let install = crate::db::embedded::shared_install_dir(runtime, &cluster);
    let connections = pool.max + crate::db::entity_facade::embedded_group_pool_max(&factory);
    let db = crate::db::entity_db::open(conf, &cluster, &install, connections)?;
    let facade = crate::db::entity_facade::open_entity_facade_for_db(&factory, &db, pool)?;
    crate::db::entity_facade::attach(&factory, facade.clone());
    // the sim overlay starts at the first enter_sim, on the same install, as a server's does
    let sim = crate::db::sim::SimOverlay::install(&facade, &cluster, Some(install));
    let search = match crate::search::attach_search(&factory, &facade, db.is_postgres()) {
        Ok(store) => Some(store),
        Err(err) => {
            tracing::warn!("Search is not available for the tests: {err}");
            None
        }
    };
    let pdf = crate::install_pdf(&factory);
    // the SSO client, as a server has it: a spec tests a provider's configuration through it (an OAuth flow with its
    // endpoints given starts with no request; an OpenID discovery is the provider's to answer)
    #[cfg(feature = "sso")]
    crate::sso::install();
    factory.init_tools().map_err(Error::from)?;
    Ok((factory, facade, sim, search, pdf, db))
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
