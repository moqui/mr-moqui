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
//! - `fresh`: a scratch runtime (the runtime's `conf` copied, `base-component` and each component linked) with its own
//!   embedded Postgres, loaded with every data type. Removed when the run ends, unless kept.
//! - `snapshot`: the same, but the loaded database is dumped to `runtime/save/test-<hash>/moqui.dump`, by a hash of the
//!   components the load reads and their entity and data files; a later run that loads the same components with the
//!   same files restores the dump instead of loading.
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
            if let Err(err) = std::fs::remove_dir_all(&scratch) {
                tracing::warn!("could not remove the scratch runtime {}: {err}", scratch.display());
            }
        }
    }
}

impl TestFixture {
    /// Shut the factory and the database down as a drop does, and leave the scratch runtime for a server to run on
    /// (`moqui test --serve`). Returns the scratch runtime to remove once that server stops: none when it is kept.
    pub fn close_keeping_runtime(mut self) -> Option<PathBuf> {
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
    let (run_runtime, scratch) = match mode {
        DbMode::None => return Err(Error::new("--db none does not open a database")),
        DbMode::Existing => (runtime.clone(), None),
        DbMode::Fresh | DbMode::Snapshot => {
            let scratch = scratch_runtime(&runtime)?;
            prepare_database(&runtime, &framework, &scratch, mode)?;
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
            scratch,
            runtime_dir: run_runtime,
            log_dir,
        }),
        Err(err) => {
            if let Some(scratch) = scratch {
                let _ = std::fs::remove_dir_all(scratch);
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

/// A scratch runtime under `/tmp` (the Postgres socket path must stay under 108 bytes): `conf` copied, and
/// `base-component`, `dev-component`, each component, the instance `MoquiConf.xml`, the shared Postgres install and the
/// downloaded Chromium linked.
fn scratch_runtime(runtime: &Path) -> Result<PathBuf, Error> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let scratch = PathBuf::from("/tmp").join(format!("mt-{}-{nanos:x}", std::process::id()));
    std::fs::create_dir_all(scratch.join("component")).map_err(io("could not create the scratch runtime"))?;
    std::fs::create_dir_all(scratch.join("db")).map_err(io("could not create the scratch db directory"))?;
    copy_files(&runtime.join("conf"), &scratch.join("conf"))?;
    link(&runtime.join("base-component"), &scratch.join("base-component"))?;
    // the dev components, which load on a dev instance
    if runtime.join("dev-component").is_dir() {
        link(&runtime.join("dev-component"), &scratch.join("dev-component"))?;
    }
    if let Ok(entries) = std::fs::read_dir(runtime.join("component")) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                link(&entry.path(), &scratch.join("component").join(entry.file_name()))?;
            }
        }
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
    // the Chromium of ./moq chromium-download, which PDF finds in the runtime it boots on (chromium::downloaded_chromium)
    if runtime.join("chromium").is_dir() {
        link(&runtime.join("chromium"), &scratch.join("chromium"))?;
    }
    Ok(scratch)
}

fn link(from: &Path, to: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
            .map_err(|err| Error::new(format!("could not link {}", to.display())).with_source(err))
    }
    #[cfg(not(unix))]
    {
        let _ = (from, to);
        Err(Error::new("moqui test needs symbolic links"))
    }
}

fn copy_files(from: &Path, to: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(to).map_err(io("could not create the scratch conf"))?;
    let entries = std::fs::read_dir(from).map_err(io("could not read the runtime conf"))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            std::fs::copy(&path, to.join(entry.file_name())).map_err(io("could not copy the runtime conf"))?;
        }
    }
    Ok(())
}

fn prepare_database(runtime: &Path, framework: &Path, scratch: &Path, mode: DbMode) -> Result<(), Error> {
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
        return Ok(());
    }
    let started = Instant::now();
    tracing::warn!("Loading the test database (all data types) in {}", scratch.display());
    let flags = crate::LoadFlags {
        data_types: Some(vec!["all".to_string()]),
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
    }
    Ok(())
}

/// A hash of the binary's version and of what the load of `scratch` reads: the components a factory on it loads, by
/// name in load order (the conf and the instance purpose decide them: a dev component such as example loads only on a
/// dev instance), and the name, size and change time of each one's entity and data files. A snapshot is made again
/// when a file changes or a run loads another set of components: a server on a conf without the dev components once
/// saved a snapshot that a run with example then restored, without example's tables.
fn data_hash(scratch: &Path, framework: &Path) -> Result<u64, Error> {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    if let Ok(exe) = std::env::current_exe().and_then(std::fs::metadata) {
        exe.len().hash(&mut hasher);
        exe.modified().ok().hash(&mut hasher);
    }
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
    let db = crate::db::entity_db::open(conf, &cluster, &install, pool.max)?;
    let facade = crate::db::entity_facade::open_entity_facade_for_factory(
        &factory,
        Arc::clone(&db.driver),
        pool,
        &db.conf.dialect,
    )?;
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
    factory.init_tools().map_err(Error::from)?;
    Ok((factory, facade, sim, search, pdf, db))
}

#[cfg(test)]
#[path = "fixture_tests.rs"]
mod tests;
