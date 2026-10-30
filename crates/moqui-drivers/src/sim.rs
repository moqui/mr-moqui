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

//! The sim overlay cluster (Overlay in `plans/archive/EmbeddedPostgres.md`). The first `enter_sim` of the
//! process starts it through the facade's sim starter, and it runs until the process stops. Each
//! sim is a schema on it. Its data directory is `sim-<pid>` under the runtime `db` directory and is
//! removed at stop. A directory left by a process that is gone is removed at the next install, after
//! its postmaster, if one is still live, is stopped.

#[cfg(feature = "embedded-postgres")]
mod overlay {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, Weak};
    use std::time::Duration;

    use moqui_core::entity::EntityFacade;
    use moqui_core::transaction::{PoolConfig, TransactionManager};
    use moqui_lib::error::Error;
    use moqui_lib::sync::lock;

    use super::super::embedded::{ClusterRole, EmbeddedCluster, StartOptions};

    /// Overlay pool size. Sims are agent and test work, not request traffic.
    const POOL_MAX: u32 = 16;
    /// Postgres keeps three slots for superusers past the pool.
    const SUPERUSER_SLOTS: u32 = 3;
    const DIR_PREFIX: &str = "sim-";

    /// Owns the overlay cluster once it has started. Drop it after the facade.
    pub struct SimOverlay {
        db_dir: PathBuf,
        install_dir: Option<PathBuf>,
        running: Mutex<Option<Running>>,
    }

    struct Running {
        cluster: EmbeddedCluster,
        dir: PathBuf,
    }

    impl SimOverlay {
        /// Set `facade`'s sim starter. Nothing starts until the first `enter_sim`.
        ///
        /// `db_dir` holds the `sim-<pid>` data directory. `install_dir` is the theseus install the
        /// entity cluster uses, so the overlay does not unpack a second one.
        #[must_use]
        pub fn install(facade: &EntityFacade, db_dir: &Path, install_dir: Option<PathBuf>) -> Arc<Self> {
            remove_orphans(db_dir, install_dir.as_deref());
            let overlay = Arc::new(Self {
                db_dir: db_dir.to_path_buf(),
                install_dir,
                running: Mutex::new(None),
            });
            let weak: Weak<Self> = Arc::downgrade(&overlay);
            facade.set_sim_starter(Some(Box::new(move || {
                weak.upgrade()
                    .ok_or_else(|| Error::new("the sim overlay has stopped"))?
                    .start()
            })));
            overlay
        }

        /// True once the overlay cluster has started.
        #[must_use]
        pub fn started(&self) -> bool {
            lock(&self.running).is_some()
        }

        /// The overlay's data directory, once started.
        #[must_use]
        pub fn data_dir(&self) -> Option<PathBuf> {
            lock(&self.running)
                .as_ref()
                .map(|running| running.cluster.data_dir().to_path_buf())
        }

        fn start(&self) -> Result<TransactionManager, Error> {
            let mut running = lock(&self.running);
            if running.is_some() {
                return Err(Error::new("the sim overlay already started"));
            }
            let dir = self.db_dir.join(format!("{DIR_PREFIX}{}", std::process::id()));
            if dir.exists() {
                // Left by an earlier process with this pid. Its postmaster cannot be ours.
                stop_postmaster(&dir, self.install_dir.as_deref());
                std::fs::remove_dir_all(&dir)
                    .map_err(|err| Error::new(format!("remove {}", dir.display())).with_source(err))?;
            }
            let options = StartOptions {
                max_connections: POOL_MAX + SUPERUSER_SLOTS,
                role: ClusterRole::Sim,
                install_dir: self.install_dir.clone(),
            };
            let (cluster, driver) = EmbeddedCluster::start_with(&dir, &options)?;
            let pool = PoolConfig {
                min: 0,
                max: POOL_MAX,
                reserve: 0,
                acquire_timeout: Duration::from_secs(30),
                ..Default::default()
            };
            let transactions = TransactionManager::new(Arc::new(driver), pool)?;
            *running = Some(Running { cluster, dir });
            Ok(transactions)
        }
    }

    impl Drop for SimOverlay {
        fn drop(&mut self) {
            let Some(mut running) = lock(&self.running).take() else {
                return;
            };
            if let Err(err) = running.cluster.stop() {
                tracing::warn!("could not stop the sim overlay: {err}");
            }
            drop(running.cluster);
            let _ = std::fs::remove_dir_all(&running.dir);
        }
    }

    /// Remove `sim-<pid>` directories whose process is gone, stopping a postmaster still live in one.
    fn remove_orphans(db_dir: &Path, install_dir: Option<&Path>) {
        let Ok(entries) = std::fs::read_dir(db_dir) else {
            return;
        };
        let own = std::process::id();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|name| name.strip_prefix(DIR_PREFIX))
                .and_then(|pid| pid.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == own || super::super::embedded::process_live(pid) {
                continue;
            }
            let dir = entry.path();
            stop_postmaster(&dir, install_dir);
            if let Err(err) = std::fs::remove_dir_all(&dir) {
                tracing::warn!("could not remove the old sim overlay {}: {err}", dir.display());
            }
        }
    }

    /// `pg_ctl stop -m immediate` for a live postmaster in `dir/data`. The overlay holds nothing to keep.
    fn stop_postmaster(dir: &Path, install_dir: Option<&Path>) {
        let data_dir = dir.join("data");
        if !super::super::embedded::postmaster_live(&data_dir) {
            return;
        }
        let install = install_dir.map_or_else(|| dir.join("install"), Path::to_path_buf);
        let pg_ctl = super::super::embedded::pg_ctl_path(&install);
        let stopped = std::process::Command::new(&pg_ctl)
            .args(["stop", "-m", "immediate", "-D"])
            .arg(&data_dir)
            .output();
        match stopped {
            Ok(output) if output.status.success() => {}
            Ok(output) => tracing::warn!(
                "pg_ctl stop for {} exited {}: {}",
                data_dir.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
            Err(err) => tracing::warn!("could not run {}: {err}", pg_ctl.display()),
        }
    }
}

#[cfg(feature = "embedded-postgres")]
pub use overlay::SimOverlay;

/// Without embedded Postgres there is no overlay cluster: the facade gets no sim starter, so `enter_sim` reports that
/// sim is not available.
#[cfg(not(feature = "embedded-postgres"))]
pub struct SimOverlay;

#[cfg(not(feature = "embedded-postgres"))]
impl SimOverlay {
    #[must_use]
    pub fn install(
        _facade: &moqui_core::entity::EntityFacade,
        _db_dir: &std::path::Path,
        _install_dir: Option<std::path::PathBuf>,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self)
    }
}
