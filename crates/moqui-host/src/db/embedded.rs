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

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
mod cluster {
    use std::path::{Path, PathBuf};

    use moqui_lib::error::Error;
    use postgresql_embedded::blocking::PostgreSQL;
    use postgresql_embedded::{Settings, BOOTSTRAP_SUPERUSER};

    use super::super::postgres::PostgresDriver;

    const DATABASE: &str = "moqui";
    const PORT: u16 = 5432;
    /// `.cargo/config.toml` forces `POSTGRESQL_VERSION` to this exact theseus build.
    const SHIPPED_VERSION: &str = "18.6.0";
    const SHIPPED_MAJOR: u32 = 18;
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
    pub fn save_db(dir: &Path, dump: &Path) -> Result<(), Error> {
        let (cluster, _driver) = EmbeddedCluster::start_with(dir, &cli_options(dir)?)?;
        cluster.save_db(dump)
    }

    /// Start the cluster, restore a custom-format dump, and stop it. A live postmaster that was
    /// adopted keeps running.
    ///
    /// # Errors
    ///
    /// Returns an error when the cluster or `pg_restore` fails.
    pub fn load_save(dir: &Path, dump: &Path) -> Result<(), Error> {
        let (cluster, _driver) = EmbeddedCluster::start_with(dir, &cli_options(dir)?)?;
        cluster.load_save(dump)
    }

    /// `save-db` and `load-save` use the install the server shares, `install` beside the cluster directory, rather
    /// than unpacking another one inside it. The path is absolute, since the install path is handed on verbatim.
    fn cli_options(dir: &Path) -> Result<StartOptions, Error> {
        std::fs::create_dir_all(dir).map_err(|err| Error::new("create embedded dir").with_source(err))?;
        let dir = std::fs::canonicalize(dir).map_err(|err| Error::new("resolve embedded dir").with_source(err))?;
        let mut options = StartOptions::new(CLI_MAX_CONNECTIONS);
        if let Some(db_dir) = dir.parent() {
            options.install_dir = Some(shared_install_dir(db_dir, &dir));
        }
        Ok(options)
    }

    /// The one theseus install under `db_dir`, shared by the entity, search and sim clusters. An
    /// install that an earlier build unpacked inside `cluster_dir` is moved there rather than unpacked
    /// again.
    pub fn shared_install_dir(db_dir: &Path, cluster_dir: &Path) -> PathBuf {
        let shared = db_dir.join("install");
        let legacy = cluster_dir.join("install");
        if !shared.exists() && legacy.join(SHIPPED_VERSION).is_dir() {
            if let Err(err) = std::fs::create_dir_all(db_dir).and_then(|()| std::fs::rename(&legacy, &shared)) {
                tracing::warn!(
                    "could not move {} to {}; it is unpacked again: {err}",
                    legacy.display(),
                    shared.display()
                );
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
        let mut server = PostgreSQL::new(settings);
        provide_libxml2(dir, server.settings())?;
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
    /// `lib`. `setup` runs initdb, which runs `postgres`, right after it unpacks, so a missing install is unpacked
    /// first on its own: `setup` skips initdb for a data directory that already holds a `postgresql.conf`.
    fn provide_libxml2(dir: &Path, settings: &Settings) -> Result<(), Error> {
        let Some(host) = crate::xmlshim::needed() else {
            return Ok(());
        };
        if !settings.installation_dir.exists() {
            let probe = dir.join("unpack");
            std::fs::create_dir_all(&probe)
                .and_then(|()| std::fs::write(probe.join("postgresql.conf"), ""))
                .map_err(|err| Error::new("prepare the theseus unpack").with_source(err))?;
            let mut unpack = settings.clone();
            unpack.data_dir.clone_from(&probe);
            let unpacked = PostgreSQL::new(unpack).setup();
            let _ = std::fs::remove_dir_all(&probe);
            unpacked.map_err(|err| embedded("unpack embedded postgres", &probe, err))?;
        }
        crate::xmlshim::provide(&settings.installation_dir.join("lib"), &host)
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
        settings.installation_dir = install_dir.clone().unwrap_or_else(|| dir.join("install"));
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

    pub(crate) fn process_live(pid: u32) -> bool {
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
        // `PostgreSQL::new` appends the exact version, so this build's binaries live in
        // `<install>/18.6.0`. Anything inside that tree is this theseus install.
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

    #[cfg(all(test, unix))]
    mod tests {
        use std::path::{Path, PathBuf};

        use moqui_core::driver::{DbConnection, DbDriver};
        use moqui_lib::object::{basic_convert, Value};

        use super::super::super::postgres::PostgresConn;
        use super::*;

        struct DirGuard(PathBuf);

        impl Drop for DirGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn embedded_cluster_queries_stream_cache_and_drops() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            // theseus 18.6.0 NEEDED libxml2.so.2. A host that only ships .so.16 runs initdb through the shim in the
            // install's lib, and this process's environment is left alone.
            let library_path = std::env::var_os("LD_LIBRARY_PATH");
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            assert_eq!(std::env::var_os("LD_LIBRARY_PATH"), library_path);
            if crate::xmlshim::needed().is_some() {
                let shim = dir.join("install").join(SHIPPED_VERSION).join("lib/libxml2.so.2");
                assert!(shim.is_file(), "{} missing", shim.display());
                assert!(!dir.join("unpack").exists());
            }
            let data_dir = cluster.data_dir().to_path_buf();
            assert!(
                data_dir.join(".s.PGSQL.5432").exists(),
                "socket missing in {}",
                data_dir.display()
            );

            let mut conn = driver.connect_postgres().unwrap();
            assert_eq!(show(&mut conn, "fsync"), "on");
            assert_eq!(show(&mut conn, "logging_collector"), "off");
            assert_eq!(show(&mut conn, "max_connections"), "20");
            assert_eq!(show(&mut conn, "listen_addresses"), "");
            let buffers = show(&mut conn, "shared_buffers");
            assert!(buffers == "16MB" || buffers == "16384kB", "shared_buffers={buffers}");

            let selected = conn.query_once("SELECT 1", &[]).unwrap();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].columns[0].1, Value::I32(1));

            let mut boxed = driver.connect().unwrap();
            let boxed_rows = boxed.query_once("SELECT 1", &[]).unwrap();
            assert_eq!(boxed_rows[0].columns[0].1, Value::I32(1));
            drop(boxed);

            conn.execute("CREATE TABLE note (body text)", &[]).unwrap();
            let inserted = conn
                .execute("INSERT INTO note (body) VALUES (?)", &[Value::string("hello?")])
                .unwrap();
            assert_eq!(inserted, 1);
            let notes = conn
                .query(
                    "SELECT body FROM note WHERE body = ? AND '?' = '?'",
                    &[Value::string("hello?")],
                )
                .unwrap();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].get("body"), Some(&Value::string("hello?")));

            round_trip_scalars(&mut conn);
            savepoint_keeps_the_first_row(&mut conn);
            stream_requires_a_transaction(&mut conn);
            stream_reads_every_inserted_row(&mut conn);
            rollback_ends_an_aborted_transaction_with_an_open_stream(&mut conn);
            statement_timeout_is_never_lifted_by_rounding(&mut conn);

            let mut other = driver.connect_postgres().unwrap();
            conn.begin().unwrap();
            conn.execute("INSERT INTO note (body) VALUES (?)", &[Value::string("hidden")])
                .unwrap();
            let unseen = other
                .query("SELECT body FROM note WHERE body = ?", &[Value::string("hidden")])
                .unwrap();
            assert!(unseen.is_empty(), "{unseen:?}");
            conn.commit().unwrap();
            let seen = other
                .query("SELECT body FROM note WHERE body = ?", &[Value::string("hidden")])
                .unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].get("body"), Some(&Value::string("hidden")));
            drop(other);

            let mut cached = driver.connect_postgres().unwrap();
            assert_eq!(cached.statement_cache_len(), 0);
            cached.query("SELECT 1", &[]).unwrap();
            cached.query("SELECT 1", &[]).unwrap();
            assert_eq!(cached.statement_cache_len(), 1);
            cached.query_once("SELECT 1", &[]).unwrap();
            cached.query_once("SELECT 2", &[]).unwrap();
            assert_eq!(cached.statement_cache_len(), 1);
            cached.canceller().cancel().unwrap();
            drop(cached);
            drop(conn);

            let pid = postmaster_pid(&data_dir);
            drop(cluster);
            assert!(
                !data_dir.join("postmaster.pid").exists(),
                "postmaster.pid still present"
            );
            assert!(
                !Path::new(&format!("/proc/{pid}")).exists(),
                "postmaster {pid} still alive"
            );
        }

        /// The figures in plans/archive/EmbeddedPostgres.md: `cargo test -p moqui-host --lib postmaster_memory -- --nocapture`.
        /// PSS splits shared_buffers across the processes that map it, so its sum is the cluster's share of RAM.
        #[cfg(target_os = "linux")]
        #[test]
        fn postmaster_memory_stays_small_for_both_roles() {
            for role in [ClusterRole::Entity, ClusterRole::Sim] {
                let dir = temp_cluster_dir();
                let _guard = DirGuard(dir.clone());
                let options = StartOptions {
                    role,
                    ..StartOptions::new(20)
                };
                let (cluster, driver) = EmbeddedCluster::start_with(&dir, &options).unwrap();
                let mut conn = driver.connect_postgres().unwrap();
                conn.execute(
                    "CREATE TABLE filler AS SELECT g, md5(g::text) AS t FROM generate_series(1, 20000) g",
                    &[],
                )
                .unwrap();
                conn.query_once("SELECT count(*) FROM filler", &[]).unwrap();
                let pid = postmaster_pid(cluster.data_dir());
                let (processes, rss_kb, pss_kb) = cluster_memory(pid);
                println!(
                    "{role:?}: {processes} processes, RSS sum {} MB, PSS sum {} MB",
                    rss_kb / 1024,
                    pss_kb / 1024
                );
                assert!(pss_kb < 200 * 1024, "{role:?} PSS {pss_kb} kB");
                drop(conn);
                drop(cluster);
            }
        }

        /// Process count and the RSS and PSS sums, in kB, of a postmaster and its children.
        #[cfg(target_os = "linux")]
        fn cluster_memory(postmaster: u32) -> (usize, u64, u64) {
            let mut pids = vec![postmaster];
            for entry in std::fs::read_dir("/proc").unwrap().flatten() {
                let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
                    continue;
                };
                let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                    continue;
                };
                let ppid = stat
                    .rfind(')')
                    .and_then(|end| stat[end + 1..].split_whitespace().nth(1))
                    .and_then(|field| field.parse::<u32>().ok());
                if ppid == Some(postmaster) {
                    pids.push(pid);
                }
            }
            let mut rss = 0;
            let mut pss = 0;
            for pid in &pids {
                let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap_or_default();
                for line in rollup.lines() {
                    let mut parts = line.split_whitespace();
                    let (Some(name), Some(kb)) = (parts.next(), parts.next().and_then(|kb| kb.parse::<u64>().ok()))
                    else {
                        continue;
                    };
                    match name {
                        "Rss:" => rss += kb,
                        "Pss:" => pss += kb,
                        _ => {}
                    }
                }
            }
            (pids.len(), rss, pss)
        }

        #[test]
        fn same_version_restart_keeps_the_row() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            {
                let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
                let mut conn = driver.connect_postgres().unwrap();
                conn.execute("CREATE TABLE kept (n int)", &[]).unwrap();
                conn.execute("INSERT INTO kept (n) VALUES (?)", &[Value::I32(7)])
                    .unwrap();
                drop(conn);
                drop(cluster);
            }
            let (_cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let mut conn = driver.connect_postgres().unwrap();
            let rows = conn.query("SELECT n FROM kept", &[]).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get("n"), Some(&Value::I32(7)));
        }

        static RELATIVE_CLUSTER_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

        /// Regression: `moqui load` passed the relative `runtime/db/embedded` straight into
        /// postgresql_embedded, and the postmaster could not create its lock file because a
        /// relative socket dir resolves against the wrong base. `start` must resolve to
        /// absolute install/data/socket paths, so a relative dir boots and serves its socket.
        #[test]
        fn relative_cluster_dir_starts_and_serves_its_socket() {
            let name = format!(
                "moqui-pg-rel-{}-{}",
                std::process::id(),
                RELATIVE_CLUSTER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let rel = PathBuf::from(&name);
            let abs = std::env::current_dir().unwrap().join(&rel);
            let _guard = DirGuard(abs.clone());
            // Unix socket paths need fewer than 108 bytes. Skip the live boot when this
            // checkout sits too deep for that; the absolute-resolution assert below still
            // documents the invariant.
            if abs.join("data").join(".s.PGSQL.5432").as_os_str().len() >= 108 {
                println!(
                    "skipping live relative boot: socket path too long under {}",
                    abs.display()
                );
                return;
            }
            let (cluster, driver) = EmbeddedCluster::start(&rel, 20).unwrap();
            let data_dir = cluster.data_dir().to_path_buf();
            assert!(
                data_dir.is_absolute(),
                "data dir still relative: {}",
                data_dir.display()
            );
            assert!(
                data_dir.join(".s.PGSQL.5432").exists(),
                "socket missing in {}",
                data_dir.display()
            );
            let mut conn = driver.connect_postgres().unwrap();
            let rows = conn.query_once("SELECT 1", &[]).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].columns[0].1, Value::I32(1));
        }

        #[test]
        fn adopt_live_postmaster_keeps_the_same_pid() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let data_dir = cluster.data_dir().to_path_buf();
            drop(driver);
            let pid = postmaster_pid(&data_dir);
            let _kill = KillGuard(pid);
            std::mem::forget(cluster);
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            assert_eq!(postmaster_pid(&data_dir), pid);
            let mut conn = driver.connect_postgres().unwrap();
            let rows = conn.query_once("SELECT 1", &[]).unwrap();
            assert_eq!(rows[0].columns[0].1, Value::I32(1));
            drop(conn);
            drop(cluster);
            assert_eq!(postmaster_pid(&data_dir), pid, "adopted postmaster was stopped");
            assert!(
                Path::new(&format!("/proc/{pid}")).exists(),
                "adopted postmaster {pid} must keep running"
            );
        }

        #[test]
        fn live_major_mismatch_refuses_with_the_pid() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let data_dir = cluster.data_dir().to_path_buf();
            drop(driver);
            let pid = postmaster_pid(&data_dir);
            let _kill = KillGuard(pid);
            std::mem::forget(cluster);
            std::fs::write(data_dir.join("PG_VERSION"), "17\n").unwrap();
            let err = start_error(&dir);
            assert!(err.message().contains(&pid.to_string()), "{err}");
            assert!(err.message().contains("17"), "{err}");
            assert!(err.message().contains(SHIPPED_VERSION), "{err}");
            assert!(
                Path::new(&format!("/proc/{pid}")).exists(),
                "refusing to adopt killed postmaster {pid}"
            );
        }

        #[test]
        fn a_lock_copied_from_another_live_cluster_is_not_adopted() {
            let live = temp_cluster_dir();
            let copy = temp_cluster_dir();
            let _guards = (DirGuard(live.clone()), DirGuard(copy.clone()));
            let (cluster, driver) = EmbeddedCluster::start(&live, 20).unwrap();
            drop(driver);
            let (stopped, stopped_driver) = EmbeddedCluster::start(&copy, 20).unwrap();
            drop(stopped_driver);
            let copy_data = stopped.data_dir().to_path_buf();
            drop(stopped);
            let pid = postmaster_pid(cluster.data_dir());
            std::fs::copy(
                cluster.data_dir().join("postmaster.pid"),
                copy_data.join("postmaster.pid"),
            )
            .unwrap();
            let err = start_error(&copy);
            assert!(err.message().contains(&pid.to_string()), "{err}");
            assert!(err.message().contains("copied lock"), "{err}");
            assert!(
                Path::new(&format!("/proc/{pid}")).exists(),
                "the live postmaster was touched"
            );
            drop(cluster);
        }

        #[test]
        fn clusters_share_one_install_and_an_old_one_moves_there() {
            let root = temp_cluster_dir();
            let _guard = DirGuard(root.clone());
            let first = root.join("first");
            // An install from an earlier build, inside the first cluster directory.
            let (cluster, driver) = EmbeddedCluster::start(&first, 20).unwrap();
            drop(driver);
            drop(cluster);
            assert!(first.join("install").join(SHIPPED_VERSION).is_dir());
            let shared = shared_install_dir(&root, &first);
            assert_eq!(shared, root.join("install"));
            assert!(
                shared.join(SHIPPED_VERSION).join("bin").is_dir(),
                "the old install moved"
            );
            assert!(!first.join("install").exists());
            let options = StartOptions {
                install_dir: Some(shared.clone()),
                ..StartOptions::new(20)
            };
            let (one, one_driver) = EmbeddedCluster::start_with(&first, &options).unwrap();
            let (two, two_driver) = EmbeddedCluster::start_with(root.join("second"), &options).unwrap();
            assert!(!root.join("second").join("install").exists());
            for driver in [&one_driver, &two_driver] {
                let rows = driver.connect_postgres().unwrap().query_once("SELECT 1", &[]).unwrap();
                assert_eq!(rows.len(), 1);
            }
            drop((one_driver, two_driver));
            drop(one);
            drop(two);
        }

        #[test]
        fn dead_pid_starts_a_new_postmaster() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let data_dir = cluster.data_dir().to_path_buf();
            drop(driver);
            let pid = postmaster_pid(&data_dir);
            let _kill = KillGuard(pid);
            std::mem::forget(cluster);
            kill_pid(pid);
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let mut conn = driver.connect_postgres().unwrap();
            let rows = conn.query_once("SELECT 1", &[]).unwrap();
            assert_eq!(rows[0].columns[0].1, Value::I32(1));
            drop(conn);
            drop(cluster);
        }

        #[test]
        fn stopped_major_mismatch_names_the_missing_binaries() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            let (cluster, _driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let data_dir = cluster.data_dir().to_path_buf();
            drop(cluster);
            std::fs::write(data_dir.join("PG_VERSION"), "17\n").unwrap();
            let err = start_error(&dir);
            assert!(err.message().contains("17"), "{err}");
            assert!(err.message().contains(SHIPPED_VERSION), "{err}");
            assert!(err.message().contains("binaries"), "{err}");
        }

        #[test]
        fn stopped_major_mismatch_with_a_sibling_install_does_not_upgrade() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            let (cluster, _driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let data_dir = cluster.data_dir().to_path_buf();
            drop(cluster);
            std::fs::write(data_dir.join("PG_VERSION"), "17\n").unwrap();
            let upgrade = dir.join("postgresql-17").join("bin").join("pg_upgrade");
            std::fs::create_dir_all(upgrade.parent().unwrap()).unwrap();
            std::fs::write(&upgrade, b"").unwrap();
            let err = start_error(&dir);
            assert!(err.message().contains("17"), "{err}");
            assert!(err.message().contains(SHIPPED_VERSION), "{err}");
            assert!(err.message().contains("pg_upgrade"), "{err}");
            assert_eq!(std::fs::read(&upgrade).unwrap(), b"");
        }

        #[test]
        fn save_db_round_trips_the_search_schema() {
            let dir = temp_cluster_dir();
            let _guard = DirGuard(dir.clone());
            let (cluster, driver) = EmbeddedCluster::start(&dir, 20).unwrap();
            let mut conn = driver.connect_postgres().unwrap();
            conn.execute("CREATE SCHEMA search", &[]).unwrap();
            conn.execute("CREATE TABLE search.kept (body text)", &[]).unwrap();
            conn.execute("INSERT INTO search.kept (body) VALUES (?)", &[Value::string("kept")])
                .unwrap();
            let dump = dir.join("moqui.dump");
            cluster.save_db(&dump).unwrap();
            let header = std::fs::read(&dump).unwrap();
            assert!(header.starts_with(b"PGDMP"), "dump is not custom format");
            let list = std::process::Command::new(dir.join("install").join(SHIPPED_VERSION).join("bin/pg_restore"))
                .arg("--list")
                .arg(&dump)
                .output()
                .unwrap();
            assert!(list.status.success(), "{}", String::from_utf8_lossy(&list.stderr));
            let listed = String::from_utf8_lossy(&list.stdout);
            assert!(listed.contains("search"), "{listed}");
            drop(conn);
            conn_drop_schema(&driver);
            cluster.load_save(&dump).unwrap();
            let mut conn = driver.connect_postgres().unwrap();
            let rows = conn.query("SELECT body FROM search.kept", &[]).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get("body"), Some(&Value::string("kept")));
        }

        fn start_error(dir: &Path) -> moqui_lib::error::Error {
            match EmbeddedCluster::start(dir, 20) {
                Err(err) => err,
                Ok((cluster, _driver)) => {
                    panic!("start succeeded for {}", cluster.data_dir().display());
                }
            }
        }

        fn conn_drop_schema(driver: &PostgresDriver) {
            let mut conn = driver.connect_postgres().unwrap();
            conn.execute("DROP SCHEMA search CASCADE", &[]).unwrap();
        }

        struct KillGuard(u32);

        impl Drop for KillGuard {
            fn drop(&mut self) {
                if Path::new(&format!("/proc/{}", self.0)).exists() {
                    let _ = std::process::Command::new("kill")
                        .args(["-9", &self.0.to_string()])
                        .status();
                }
            }
        }

        fn kill_pid(pid: u32) {
            let status = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status()
                .expect("kill");
            assert!(status.success(), "kill {pid}");
            for _ in 0..50 {
                if !Path::new(&format!("/proc/{pid}")).exists() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            panic!("postmaster {pid} still present after kill");
        }

        fn round_trip_scalars(conn: &mut PostgresConn) {
            conn.execute(
                "CREATE TABLE sample (
                    txt text,
                    flag boolean,
                    i2 int2,
                    i4 int4,
                    i8 int8,
                    f4 float4,
                    f8 float8,
                    n numeric,
                    ts timestamptz,
                    d date,
                    t time,
                    b bytea,
                    z int4
                )",
                &[],
            )
            .unwrap();
            let timestamp = kind("2024-07-15 01:14:00", "Timestamp");
            let date = kind("2024-07-15", "Date");
            let time = kind("01:14:00", "Time");
            let decimal = kind("3950.123456", "BigDecimal");
            let bytes = Value::Bytes(vec![1, 2, 3, 255]);
            conn.execute(
                "INSERT INTO sample (txt, flag, i2, i4, i8, f4, f8, n, ts, d, t, b, z) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                &[
                    Value::string("alpha"),
                    Value::Bool(true),
                    Value::I32(4),
                    Value::I32(7),
                    Value::I64(9),
                    Value::F64(1.5),
                    Value::F64(2.25),
                    decimal.clone(),
                    timestamp.clone(),
                    date.clone(),
                    time.clone(),
                    bytes.clone(),
                    Value::Null,
                ],
            )
            .unwrap();
            let rows = conn
                .query(
                    "SELECT txt, flag, i2, i4, i8, f4, f8, n, ts, d, t, b, z FROM sample",
                    &[],
                )
                .unwrap();
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(row.get("txt"), Some(&Value::string("alpha")));
            assert_eq!(row.get("flag"), Some(&Value::Bool(true)));
            assert_eq!(row.get("i2"), Some(&Value::I32(4)));
            assert_eq!(row.get("i4"), Some(&Value::I32(7)));
            assert_eq!(row.get("i8"), Some(&Value::I64(9)));
            assert_eq!(row.get("f4"), Some(&Value::F64(1.5)));
            assert_eq!(row.get("f8"), Some(&Value::F64(2.25)));
            assert_eq!(row.get("n"), Some(&decimal));
            assert_eq!(row.get("ts"), Some(&timestamp));
            assert_eq!(row.get("d"), Some(&date));
            assert_eq!(row.get("t"), Some(&time));
            assert_eq!(row.get("b"), Some(&bytes));
            assert_eq!(row.get("z"), Some(&Value::Null));
        }

        fn savepoint_keeps_the_first_row(conn: &mut PostgresConn) {
            conn.execute("CREATE TABLE sp (n int)", &[]).unwrap();
            conn.begin().unwrap();
            conn.execute("INSERT INTO sp (n) VALUES (?)", &[Value::I32(1)]).unwrap();
            conn.savepoint("my sp").unwrap();
            conn.execute("INSERT INTO sp (n) VALUES (?)", &[Value::I32(2)]).unwrap();
            conn.rollback_to_savepoint("my sp").unwrap();
            conn.release_savepoint("my sp").unwrap();
            conn.commit().unwrap();
            let rows = conn.query("SELECT n FROM sp", &[]).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get("n"), Some(&Value::I32(1)));
        }

        fn stream_requires_a_transaction(conn: &mut PostgresConn) {
            let err = conn.open_stream("SELECT 1", &[]).unwrap_err();
            assert!(err.message().contains("transaction is required"), "{err}");
            assert!(!conn.stream_open());
        }

        fn stream_reads_every_inserted_row(conn: &mut PostgresConn) {
            conn.execute("CREATE TABLE streamed (n int)", &[]).unwrap();
            conn.begin().unwrap();
            for n in 1..=51 {
                conn.execute("INSERT INTO streamed (n) VALUES (?)", &[Value::I32(n)])
                    .unwrap();
            }
            conn.open_stream("SELECT n FROM streamed ORDER BY n", &[]).unwrap();
            assert!(conn.stream_open());
            for n in 1..=51 {
                let row = conn.next_row().unwrap().unwrap_or_else(|| panic!("missing row {n}"));
                assert_eq!(row.get("n"), Some(&Value::I32(n)));
            }
            assert!(conn.next_row().unwrap().is_none());
            assert!(conn.stream_open());
            conn.close_stream().unwrap();
            assert!(!conn.stream_open());
            conn.commit().unwrap();
        }

        /// ROLLBACK closes the cursor itself, so a stream left open in an aborted transaction does not stop it.
        fn rollback_ends_an_aborted_transaction_with_an_open_stream(conn: &mut PostgresConn) {
            conn.begin().unwrap();
            conn.open_stream("SELECT n FROM streamed ORDER BY n", &[]).unwrap();
            assert!(conn.next_row().unwrap().is_some());
            assert!(conn.query_once("SELECT 1 / 0", &[]).is_err());
            conn.rollback().unwrap();
            assert!(!conn.stream_open());
            assert_eq!(conn.query_once("SELECT 1", &[]).unwrap().len(), 1);
        }

        /// A deadline that has passed is an error, and a sub-millisecond remainder still sets a limit.
        fn statement_timeout_is_never_lifted_by_rounding(conn: &mut PostgresConn) {
            conn.begin().unwrap();
            assert!(conn.set_statement_timeout(Some(std::time::Duration::ZERO)).is_err());
            conn.rollback().unwrap();
            conn.begin().unwrap();
            conn.set_statement_timeout(Some(std::time::Duration::from_micros(300)))
                .unwrap();
            assert_eq!(show(conn, "statement_timeout"), "1ms");
            conn.rollback().unwrap();
        }

        fn show(conn: &mut PostgresConn, name: &str) -> String {
            let rows = conn.query_once(&format!("SHOW {name}"), &[]).unwrap();
            match &rows[0].columns[0].1 {
                Value::String(text) => text.clone(),
                other => panic!("show {name}: {other:?}"),
            }
        }

        fn kind(text: &str, java_type: &str) -> Value {
            let converted = basic_convert(&Value::string(text), java_type);
            assert!(
                !matches!(converted.value, Value::String(_)),
                "{text} did not convert to {java_type}: {:?}",
                converted.value
            );
            converted.value
        }

        fn temp_cluster_dir() -> PathBuf {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let name = format!("moqui-pg-{}-{nanos}", std::process::id());
            if Path::new("/tmp").is_dir() {
                PathBuf::from("/tmp").join(name)
            } else {
                std::env::temp_dir().join(name)
            }
        }

        fn postmaster_pid(data_dir: &Path) -> u32 {
            let text = std::fs::read_to_string(data_dir.join("postmaster.pid")).unwrap();
            text.lines().next().unwrap().trim().parse().unwrap()
        }
    }
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
pub use cluster::{load_save, save_db, shared_install_dir, ClusterRole, EmbeddedCluster, StartOptions, ToolTarget};
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
pub(crate) use cluster::{pg_ctl_path, postmaster_live, process_live};

#[cfg(not(all(feature = "embedded-postgres", feature = "postgres")))]
pub fn save_db(_dir: &std::path::Path, _dump: &std::path::Path) -> Result<(), moqui_lib::error::Error> {
    Err(moqui_lib::error::Error::new("embedded postgres is not in this build"))
}

#[cfg(not(all(feature = "embedded-postgres", feature = "postgres")))]
pub fn load_save(_dir: &std::path::Path, _dump: &std::path::Path) -> Result<(), moqui_lib::error::Error> {
    Err(moqui_lib::error::Error::new("embedded postgres is not in this build"))
}
