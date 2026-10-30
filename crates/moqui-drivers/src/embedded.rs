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

//! Embedded Postgres lifecycle. Per-query traffic is [`super::postgres`].

#[cfg(feature = "embedded-postgres")]
mod cluster {
    use std::path::{Path, PathBuf};

    use moqui_lib::error::Error;
    use postgresql_embedded::blocking::PostgreSQL;
    use postgresql_embedded::{Settings, BOOTSTRAP_SUPERUSER};

    use super::super::postgres::PostgresDriver;

    const DATABASE: &str = "moqui";
    const PORT: u16 = 5432;
    /// `.cargo/config.toml` forces `POSTGRESQL_VERSION` to this exact theseus build; build.rs passes it on.
    const SHIPPED_VERSION: &str = env!("MOQUI_PG_VERSION");
    const SHIPPED_MAJOR: u32 = 18;
    /// The theseus archive of [`SHIPPED_VERSION`] that build.rs staged, psql and readline taken out. [`install`]
    /// unpacks it.
    static ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/postgresql.tar.gz"));
    const CLI_MAX_CONNECTIONS: u32 = 20;

    /// One theseus postmaster. The install and data directory stay under the caller's path.
    pub struct EmbeddedCluster {
        /// `PostgreSQL` stops a started server when it drops. An adopted one must outlive us.
        server: std::mem::ManuallyDrop<PostgreSQL>,
        data_dir: PathBuf,
        stopped: bool,
        /// A postmaster another process started. This handle never stops it.
        adopted: bool,
    }

    struct PostmasterLock {
        pid: u32,
        port: u16,
        /// Line 2: the data directory the postmaster was started on.
        data_dir: PathBuf,
    }

    /// What a cluster is for. The sim overlay is small and does not wait for its commits to sync.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ClusterRole {
        /// The entity cluster, and a dedicated search cluster.
        Entity,
        /// The sim overlay: `shared_buffers` 8MB and `synchronous_commit` off.
        Sim,
    }

    /// How [`EmbeddedCluster::start_with`] sets up a cluster.
    #[derive(Clone, Debug)]
    pub struct StartOptions {
        pub max_connections: u32,
        pub role: ClusterRole,
        /// The theseus install. `None` is `install` under the cluster directory.
        pub install_dir: Option<PathBuf>,
    }

    impl StartOptions {
        #[must_use]
        pub fn new(max_connections: u32) -> Self {
            Self {
                max_connections,
                role: ClusterRole::Entity,
                install_dir: None,
            }
        }
    }

    impl EmbeddedCluster {
        /// Install, initdb, start, and create the `moqui` database.
        ///
        /// On Unix the socket directory is the data directory and `listen_addresses` is empty.
        /// On Windows `listen_addresses` is `127.0.0.1` and the postmaster binds an ephemeral port.
        /// The client uses `NoTls` either way.
        ///
        /// A live postmaster is adopted when `PG_VERSION` matches this theseus major, and refused
        /// with that pid when it does not. A dead pid is not live. A stopped data directory whose
        /// major differs is an error: this build ships theseus 18.6.0 and does not run `pg_upgrade`.
        /// A second start reloads `pgpass` and skips `CREATE DATABASE` when `moqui` already exists.
        ///
        /// # Errors
        ///
        /// Returns an error when setup, start, the version check, or `CREATE DATABASE` fails.
        pub fn start(dir: impl AsRef<Path>, max_connections: u32) -> Result<(Self, PostgresDriver), Error> {
            Self::start_with(dir, &StartOptions::new(max_connections))
        }

        /// [`Self::start`] with the role's settings and, when given, a shared install.
        ///
        /// # Errors
        ///
        /// As [`Self::start`].
        pub fn start_with(dir: impl AsRef<Path>, options: &StartOptions) -> Result<(Self, PostgresDriver), Error> {
            let dir = dir.as_ref();
            std::fs::create_dir_all(dir).map_err(|err| Error::new("create embedded dir").with_source(err))?;
            // postgresql_embedded is handed install, data, socket, and password paths verbatim,
            // and the postmaster resolves a relative socket dir against the wrong base, so it
            // cannot create its lock file (`moqui load` passed `runtime/db/embedded` straight
            // through). Resolve once so every caller gets absolute paths.
            let dir = std::fs::canonicalize(dir).map_err(|err| Error::new("resolve embedded dir").with_source(err))?;
            let data_dir = dir.join("data");
            #[cfg(unix)]
            check_socket_len(&data_dir)?;
            if let Some(lock) = postmaster_lock(&data_dir)? {
                if process_live(lock.pid) {
                    return adopt(&dir, &data_dir, options, lock);
                }
            }
            let install_root = options.install_dir.clone().unwrap_or_else(|| dir.join("install"));
            ensure_major_can_start(&dir, &install_root, &data_dir)?;
            launch(&dir, &data_dir, options, None)
        }

        /// `pg_dump --format=custom` of the whole `moqui` database, including a `search` schema.
        ///
        /// # Errors
        ///
        /// Returns an error when `pg_dump` is missing or exits non-zero.
        pub fn save_db(&self, dump: &Path) -> Result<(), Error> {
            let mut args = connect_args(self);
            args.push("--format=custom".to_string());
            args.push("--file".to_string());
            args.push(dump.display().to_string());
            run_tool(self, "pg_dump", &args)
        }

        /// `pg_restore --format=custom` of a dump from [`Self::save_db`].
        ///
        /// # Errors
        ///
        /// Returns an error when `pg_restore` is missing or exits non-zero.
        pub fn load_save(&self, dump: &Path) -> Result<(), Error> {
            let mut args = connect_args(self);
            args.push("--format=custom".to_string());
            args.push("--clean".to_string());
            args.push("--if-exists".to_string());
            args.push(dump.display().to_string());
            run_tool(self, "pg_restore", &args)
        }

        /// Stop the postmaster. A second call is a no-op, and so is a call on an adopted postmaster.
        ///
        /// # Errors
        ///
        /// Returns an error when `pg_ctl stop` fails.
        pub fn stop(&mut self) -> Result<(), Error> {
            if self.stopped || self.adopted {
                return Ok(());
            }
            self.server
                .stop()
                .map_err(|err| embedded("stop embedded postgres", &self.data_dir, err))?;
            self.stopped = true;
            Ok(())
        }

        /// Data directory. On Unix this is also the socket directory.
        #[must_use]
        pub fn data_dir(&self) -> &Path {
            &self.data_dir
        }

        /// The port the postmaster listens on; on Unix it names the socket file.
        #[must_use]
        pub fn port(&self) -> u16 {
            self.server.settings().port
        }

        /// The superuser's name and password, for a test that connects on its own.
        #[must_use]
        pub fn superuser(&self) -> (String, String) {
            (BOOTSTRAP_SUPERUSER.to_string(), self.server.settings().password.clone())
        }

        /// Connection and binary paths for `pg_dump` and `pg_basebackup`. `prefix` is `moqui` or `search`.
        #[must_use]
        pub fn tool_connect(&self, prefix: &str) -> ToolTarget {
            let settings = self.server.settings();
            #[cfg(unix)]
            let host = settings
                .socket_dir
                .clone()
                .unwrap_or_else(|| settings.data_dir.clone())
                .display()
                .to_string();
            #[cfg(not(unix))]
            let host = settings.host.clone();
            ToolTarget {
                prefix: prefix.to_string(),
                bin_dir: settings.binary_dir(),
                username: BOOTSTRAP_SUPERUSER.to_string(),
                password: settings.password.clone(),
                port: settings.port,
                host,
                database: DATABASE.to_string(),
                data_dir: self.data_dir.clone(),
            }
        }
    }

    /// How a backup reaches one embedded postmaster without holding the cluster handle.
    #[derive(Clone, Debug)]
    pub struct ToolTarget {
        pub prefix: String,
        pub bin_dir: PathBuf,
        pub username: String,
        pub password: String,
        pub port: u16,
        pub host: String,
        pub database: String,
        pub data_dir: PathBuf,
    }

    impl Drop for EmbeddedCluster {
        fn drop(&mut self) {
            let _ = self.stop();
            if !self.adopted {
                // SAFETY: `server` is dropped once, here, and never used again.
                unsafe { std::mem::ManuallyDrop::drop(&mut self.server) };
            }
        }
    }

    /// Start the cluster, dump it, and stop it. A live postmaster that was adopted keeps running.
    ///
    /// # Errors
    ///
    /// Returns an error when the cluster or `pg_dump` fails.
    pub fn save_db(runtime_dir: &Path, dir: &Path, dump: &Path) -> Result<(), Error> {
        let (cluster, _driver) = EmbeddedCluster::start_with(dir, &cli_options(runtime_dir, dir)?)?;
        cluster.save_db(dump)
    }

    /// Start the cluster, restore a custom-format dump, and stop it. A live postmaster that was
    /// adopted keeps running.
    ///
    /// # Errors
    ///
    /// Returns an error when the cluster or `pg_restore` fails.
    pub fn load_save(runtime_dir: &Path, dir: &Path, dump: &Path) -> Result<(), Error> {
        let (cluster, _driver) = EmbeddedCluster::start_with(dir, &cli_options(runtime_dir, dir)?)?;
        cluster.load_save(dump)
    }

    /// `save-db` and `load-save` use the install the server of `runtime_dir` shares rather than unpacking another one
    /// inside the cluster directory.
    fn cli_options(runtime_dir: &Path, dir: &Path) -> Result<StartOptions, Error> {
        std::fs::create_dir_all(dir).map_err(|err| Error::new("create embedded dir").with_source(err))?;
        let dir = std::fs::canonicalize(dir).map_err(|err| Error::new("resolve embedded dir").with_source(err))?;
        let mut options = StartOptions::new(CLI_MAX_CONNECTIONS);
        options.install_dir = Some(shared_install_dir(runtime_dir, &dir));
        Ok(options)
    }

    /// The one theseus install of a runtime ([`super::install_dir`]), shared by the entity, search and sim clusters.
    /// An install that an earlier build unpacked in `runtime/db/install`, or inside `cluster_dir`, is moved there
    /// rather than unpacked again.
    pub fn shared_install_dir(runtime_dir: &Path, cluster_dir: &Path) -> PathBuf {
        let shared = super::install_dir(runtime_dir);
        if shared.exists() {
            return shared;
        }
        let legacy = [runtime_dir.join("db/install"), cluster_dir.join("install")]
            .into_iter()
            .find(|dir| dir.join(SHIPPED_VERSION).is_dir());
        if let Some(legacy) = legacy {
            let moved = shared
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::rename(&legacy, &shared));
            match moved {
                Ok(()) => tracing::info!(
                    "Moved the embedded Postgres install {} to {}",
                    legacy.display(),
                    shared.display()
                ),
                Err(err) => tracing::warn!(
                    "could not move {} to {}; it is unpacked again: {err}",
                    legacy.display(),
                    shared.display()
                ),
            }
        }
        shared
    }

    #[cfg(unix)]
    fn check_socket_len(data_dir: &Path) -> Result<(), Error> {
        let socket_file = data_dir.join(format!(".s.PGSQL.{PORT}"));
        if socket_file.as_os_str().len() >= 108 {
            return Err(Error::new(format!(
                "postgres socket path is {} bytes; unix sockets need fewer than 108",
                socket_file.as_os_str().len()
            )));
        }
        Ok(())
    }

    fn adopt(
        dir: &Path,
        data_dir: &Path,
        options: &StartOptions,
        lock: PostmasterLock,
    ) -> Result<(EmbeddedCluster, PostgresDriver), Error> {
        // A data directory copied from a live cluster carries that cluster's lock. Its postmaster
        // serves the other directory, so it is not adopted here.
        let same_dir = std::fs::canonicalize(&lock.data_dir).ok() == std::fs::canonicalize(data_dir).ok();
        if !same_dir {
            return Err(Error::new(format!(
                "postmaster.pid in {} names postmaster {} on {}; stop it or remove the copied lock file",
                data_dir.display(),
                lock.pid,
                lock.data_dir.display()
            )));
        }
        let major = pg_major(data_dir)?.unwrap_or(0);
        if major != SHIPPED_MAJOR {
            return Err(Error::new(format!(
                "postmaster {} is live for {} and PG_VERSION is {major}; theseus {SHIPPED_VERSION} will not adopt it or start a second postmaster",
                lock.pid,
                data_dir.display()
            )));
        }
        launch(dir, data_dir, options, Some(lock.port))
    }

    fn ensure_major_can_start(dir: &Path, install_root: &Path, data_dir: &Path) -> Result<(), Error> {
        let Some(major) = pg_major(data_dir)? else {
            return Ok(());
        };
        if major == SHIPPED_MAJOR {
            return Ok(());
        }
        if old_install_present(dir, install_root) {
            Err(Error::new(format!(
                "PG_VERSION is {major} and this build only ships theseus {SHIPPED_VERSION}; pg_upgrade will not run"
            )))
        } else {
            Err(Error::new(format!(
                "PG_VERSION is {major} and theseus {SHIPPED_VERSION} has no previous binaries beside the install"
            )))
        }
    }

    fn launch(
        dir: &Path,
        data_dir: &Path,
        options: &StartOptions,
        adopt_port: Option<u16>,
    ) -> Result<(EmbeddedCluster, PostgresDriver), Error> {
        let settings = settings(dir, data_dir, options, adopt_port);
        install(&settings.installation_dir)?;
        let mut server = PostgreSQL::new(settings);
        provide_libxml2(server.settings())?;
        server
            .setup()
            .map_err(|err| embedded("setup embedded postgres", data_dir, err))?;
        if adopt_port.is_none() {
            server
                .start()
                .map_err(|err| embedded("start embedded postgres", data_dir, err))?;
        }
        let data_dir = server.settings().data_dir.clone();
        let exists = server
            .database_exists(DATABASE)
            .map_err(|err| embedded("check embedded database", &data_dir, err))?;
        if !exists {
            server
                .create_database(DATABASE)
                .map_err(|err| embedded("create embedded database", &data_dir, err))?;
        }
        let password = server.settings().password.clone();
        let driver = driver_for(&server, &password);
        Ok((
            EmbeddedCluster {
                server: std::mem::ManuallyDrop::new(server),
                data_dir,
                stopped: false,
                adopted: adopt_port.is_some(),
            },
            driver,
        ))
    }

    /// `postgres` needs `libxml2.so.2`. When the host lacks it, [`crate::xmlshim`] puts a shim in the install's
    /// `lib`, which [`install`] has unpacked before `setup` runs initdb.
    fn provide_libxml2(settings: &Settings) -> Result<(), Error> {
        let Some(host) = crate::xmlshim::needed() else {
            return Ok(());
        };
        crate::xmlshim::provide(&settings.installation_dir.join("lib"), &host)
    }

    /// Unpack [`ARCHIVE`] into `install_dir` (`<install>/<version>`) when it is not there yet. The archive's top
    /// directory is dropped, as theseus did. It is unpacked beside `install_dir` and renamed into place, so a process
    /// that stops part way leaves no half install, and two processes that unpack at once both end with one.
    fn install(install_dir: &Path) -> Result<(), Error> {
        if install_dir.join("bin").is_dir() {
            return Ok(());
        }
        let parent = install_dir
            .parent()
            .ok_or_else(|| Error::new(format!("{} has no parent directory", install_dir.display())))?;
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::new(format!("could not create {}", parent.display())).with_source(err))?;
        let unpack = parent.join(format!(".unpack-{SHIPPED_VERSION}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&unpack);
        let unpacked = unpack_archive(&unpack);
        if let Err(err) = unpacked {
            let _ = std::fs::remove_dir_all(&unpack);
            return Err(err);
        }
        if let Err(err) = std::fs::rename(&unpack, install_dir) {
            let _ = std::fs::remove_dir_all(&unpack);
            if !install_dir.join("bin").is_dir() {
                return Err(Error::new(format!(
                    "could not move the Postgres install to {}",
                    install_dir.display()
                ))
                .with_source(err));
            }
        }
        tracing::info!(
            "Unpacked embedded Postgres {SHIPPED_VERSION} into {}",
            install_dir.display()
        );
        Ok(())
    }

    fn unpack_archive(dir: &Path) -> Result<(), Error> {
        let failed =
            |err: std::io::Error| Error::new("could not unpack the embedded Postgres archive").with_source(err);
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(ARCHIVE));
        for entry in archive.entries().map_err(failed)? {
            let mut entry = entry.map_err(failed)?;
            // theseus archives hold one top directory, postgresql-<version>-<target>/, which is dropped.
            let inner: PathBuf = entry.path().map_err(failed)?.components().skip(1).collect();
            if inner.as_os_str().is_empty() {
                continue;
            }
            if !inner
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(Error::new(format!(
                    "the embedded Postgres archive has an entry outside its directory: {}",
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

    fn driver_for(server: &PostgreSQL, password: &str) -> PostgresDriver {
        let settings = server.settings();
        #[cfg(unix)]
        {
            let socket = settings.socket_dir.clone().unwrap_or_else(|| settings.data_dir.clone());
            PostgresDriver::unix_socket(BOOTSTRAP_SUPERUSER, DATABASE, socket)
                .port(settings.port)
                .password(password)
        }
        #[cfg(not(unix))]
        {
            PostgresDriver::tcp_no_tls(BOOTSTRAP_SUPERUSER, DATABASE, settings.host.clone(), settings.port)
                .password(password)
        }
    }

    fn settings(dir: &Path, data_dir: &Path, options: &StartOptions, adopt_port: Option<u16>) -> Settings {
        let StartOptions {
            max_connections,
            role,
            ref install_dir,
        } = *options;
        let mut settings = Settings::new();
        // The unpacked install of this build's version (`install`), trusted as it is: nothing is looked up or
        // downloaded.
        settings.installation_dir = install_dir
            .clone()
            .unwrap_or_else(|| dir.join("install"))
            .join(SHIPPED_VERSION);
        settings.trust_installation_dir = true;
        settings.password_file = dir.join("pgpass");
        settings.data_dir = data_dir.to_path_buf();
        settings.temporary = false;
        settings.timeout = None;
        // fsync=on overrides pg_ctl's `-F`. Sim keeps it on too; synchronous_commit is its only relaxation.
        let buffers = match role {
            ClusterRole::Entity => "16MB",
            ClusterRole::Sim => "8MB",
        };
        settings
            .configuration
            .insert("shared_buffers".to_string(), buffers.to_string());
        settings.configuration.insert("fsync".to_string(), "on".to_string());
        if role == ClusterRole::Sim {
            settings
                .configuration
                .insert("synchronous_commit".to_string(), "off".to_string());
        } else {
            // Block incrementals (Postgres 17+) read WAL summaries. The setting is postmaster-level,
            // so a cluster that is already up picks it up the next time this process starts it.
            settings
                .configuration
                .insert("summarize_wal".to_string(), "on".to_string());
        }
        settings
            .configuration
            .insert("logging_collector".to_string(), "off".to_string());
        settings
            .configuration
            .insert("max_connections".to_string(), max_connections.to_string());
        #[cfg(unix)]
        {
            settings.socket_dir = Some(data_dir.to_path_buf());
            settings.port = adopt_port.unwrap_or(PORT);
            // Empty listen_addresses disables TCP.
            settings
                .configuration
                .insert("listen_addresses".to_string(), String::new());
        }
        #[cfg(not(unix))]
        {
            settings.socket_dir = None;
            settings.host = "127.0.0.1".to_string();
            // Port 0 asks postgresql_embedded to bind an ephemeral port.
            settings.port = adopt_port.unwrap_or(0);
            settings
                .configuration
                .insert("listen_addresses".to_string(), "127.0.0.1".to_string());
        }
        reuse_password(&mut settings);
        settings
    }

    fn reuse_password(settings: &mut Settings) {
        let Ok(text) = std::fs::read_to_string(&settings.password_file) else {
            return;
        };
        let text = text.trim();
        if !text.is_empty() {
            settings.password = text.to_string();
        }
    }

    fn pg_major(data_dir: &Path) -> Result<Option<u32>, Error> {
        let path = data_dir.join("PG_VERSION");
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|err| Error::new(format!("read {}", path.display())).with_source(err))?;
        let major = text
            .trim()
            .parse::<u32>()
            .map_err(|_| Error::new(format!("PG_VERSION {:?} is not a major version", text.trim())))?;
        Ok(Some(major))
    }

    fn postmaster_lock(data_dir: &Path) -> Result<Option<PostmasterLock>, Error> {
        let path = data_dir.join("postmaster.pid");
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|err| Error::new("read postmaster.pid").with_source(err))?;
        let mut lines = text.lines();
        let pid_line = lines.next().unwrap_or("").trim();
        let pid: u32 = pid_line
            .parse()
            .map_err(|_| Error::new(format!("postmaster.pid pid {pid_line:?} is not a pid")))?;
        let lock_dir = PathBuf::from(lines.next().unwrap_or("").trim());
        let _started = lines.next();
        let port_line = lines
            .next()
            .ok_or_else(|| Error::new("postmaster.pid has no port"))?
            .trim();
        let port: u16 = port_line
            .parse()
            .map_err(|_| Error::new(format!("postmaster.pid port {port_line:?} is not a port")))?;
        Ok(Some(PostmasterLock {
            pid,
            port,
            data_dir: lock_dir,
        }))
    }

    /// True when `data_dir/postmaster.pid` names a live process.
    pub(crate) fn postmaster_live(data_dir: &Path) -> bool {
        matches!(postmaster_lock(data_dir), Ok(Some(lock)) if process_live(lock.pid))
    }

    /// `pg_ctl` of this build's theseus version under `install_dir`.
    pub(crate) fn pg_ctl_path(install_dir: &Path) -> PathBuf {
        tool_bin(&install_dir.join(SHIPPED_VERSION).join("bin"), "pg_ctl")
    }

    pub fn process_live(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(target_os = "linux")]
        {
            linux_process_live(pid)
        }
        #[cfg(all(unix, not(target_os = "linux")))]
        {
            std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .is_ok_and(|status| status.success())
        }
        #[cfg(windows)]
        {
            windows_process_live(pid)
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_process_live(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(end) = stat.rfind(')') else {
            return false;
        };
        let state = stat[end + 1..].split_whitespace().next().unwrap_or("");
        !state.is_empty() && state != "Z"
    }

    /// `tasklist` is the process probe that does not add a Windows crate. This host is Linux, so
    /// the TCP start path is not executed here.
    #[cfg(windows)]
    fn windows_process_live(pid: u32) -> bool {
        let Ok(output) = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
        else {
            return false;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        text.contains(&pid.to_string()) && !text.to_ascii_lowercase().contains("no tasks")
    }

    fn old_install_present(cluster_dir: &Path, install_root: &Path) -> bool {
        // This build's binaries live in `<install>/<version>` (`settings`). Anything inside that tree is this theseus
        // install.
        let shipped = install_root.join(SHIPPED_VERSION);
        other_server(cluster_dir, &shipped, install_root) || other_server(install_root, &shipped, install_root)
    }

    fn other_server(dir: &Path, shipped: &Path, shipped_root: &Path) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == *shipped || path == *shipped_root || path.starts_with(shipped) || !path.is_dir() {
                continue;
            }
            if has_server_bin(&path) {
                return true;
            }
        }
        false
    }

    fn has_server_bin(dir: &Path) -> bool {
        let bin = dir.join("bin");
        bin.join("postgres").is_file()
            || bin.join("pg_upgrade").is_file()
            || bin.join("postgres.exe").is_file()
            || bin.join("pg_upgrade.exe").is_file()
    }

    fn connect_args(cluster: &EmbeddedCluster) -> Vec<String> {
        let settings = cluster.server.settings();
        let mut args = vec![
            "--username".to_string(),
            BOOTSTRAP_SUPERUSER.to_string(),
            "--port".to_string(),
            settings.port.to_string(),
            "--dbname".to_string(),
            DATABASE.to_string(),
            "--no-password".to_string(),
        ];
        #[cfg(unix)]
        if let Some(socket) = &settings.socket_dir {
            args.push("--host".to_string());
            args.push(socket.display().to_string());
        }
        #[cfg(not(unix))]
        {
            args.push("--host".to_string());
            args.push(settings.host.clone());
        }
        args
    }

    fn tool_bin(bin_dir: &Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        {
            bin_dir.join(format!("{name}.exe"))
        }
        #[cfg(not(windows))]
        {
            bin_dir.join(name)
        }
    }

    fn run_tool(cluster: &EmbeddedCluster, name: &str, args: &[String]) -> Result<(), Error> {
        let bin = tool_bin(&cluster.server.settings().binary_dir(), name);
        let output = std::process::Command::new(&bin)
            .args(args)
            .env("PGPASSWORD", &cluster.server.settings().password)
            .output()
            .map_err(|err| Error::new(format!("run {}", bin.display())).with_source(err))?;
        if output.status.success() {
            return Ok(());
        }
        let detail = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        Err(Error::new(format!("{name} exited {}: {detail}", output.status)))
    }

    fn embedded(context: &str, data_dir: &Path, err: postgresql_embedded::Error) -> Error {
        let log = log_tail(&data_dir.join("start.log"));
        let message = if log.is_empty() {
            context.to_string()
        } else {
            format!("{context}: {log}")
        };
        Error::new(message).with_source(err)
    }

    fn log_tail(path: &Path) -> String {
        let Ok(text) = std::fs::read_to_string(path) else {
            return String::new();
        };
        let mut start = text.len().saturating_sub(1_500);
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..].to_string()
    }

    // Inline `mod cluster` loads children from embedded/cluster/, not from beside embedded.rs.
    #[cfg(all(test, unix))]
    mod tests;
}

#[cfg(feature = "embedded-postgres")]
pub use cluster::process_live;
#[cfg(feature = "embedded-postgres")]
pub use cluster::{load_save, save_db, shared_install_dir, ClusterRole, EmbeddedCluster, StartOptions, ToolTarget};
#[cfg(feature = "embedded-postgres")]
pub(crate) use cluster::{pg_ctl_path, postmaster_live};

/// Where the embedded Postgres of a runtime is unpacked: `postgres` under `MOQUI_INSTALL` (a process property, then
/// the environment variable, as `MOQUI_CONF`) when it is set, else under `install` in the runtime directory, where
/// `./moq archive` leaves it out. Absolute, since the install path is handed on verbatim.
#[must_use]
pub fn install_dir(runtime_dir: &std::path::Path) -> std::path::PathBuf {
    let selected = moqui_lib::env::get_property("MOQUI_INSTALL")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("MOQUI_INSTALL"))
        .filter(|dir| !dir.is_empty());
    let root = match selected {
        Some(dir) => std::path::absolute(&dir).unwrap_or_else(|_| dir.into()),
        None => std::fs::canonicalize(runtime_dir)
            .or_else(|_| std::path::absolute(runtime_dir))
            .unwrap_or_else(|_| runtime_dir.to_path_buf())
            .join("install"),
    };
    root.join("postgres")
}

/// Without embedded Postgres there is no install: the path is passed along and never used.
#[cfg(not(feature = "embedded-postgres"))]
#[must_use]
pub fn shared_install_dir(runtime_dir: &std::path::Path, _cluster_dir: &std::path::Path) -> std::path::PathBuf {
    install_dir(runtime_dir)
}

#[cfg(not(feature = "embedded-postgres"))]
pub fn save_db(
    _runtime_dir: &std::path::Path,
    _dir: &std::path::Path,
    _dump: &std::path::Path,
) -> Result<(), moqui_lib::error::Error> {
    Err(moqui_lib::error::Error::new("embedded postgres is not in this build"))
}

#[cfg(not(feature = "embedded-postgres"))]
pub fn load_save(
    _runtime_dir: &std::path::Path,
    _dir: &std::path::Path,
    _dump: &std::path::Path,
) -> Result<(), moqui_lib::error::Error> {
    Err(moqui_lib::error::Error::new("embedded postgres is not in this build"))
}
