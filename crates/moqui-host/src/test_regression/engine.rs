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

//! Start and stop one engine of `moqui test-regression`: a copy of its runtime, the database and listener settings
//! as environment properties, the test data clock, and the boot log the start waits on.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use moqui_lib::error::Error;

use super::db::Connection;
use super::target::{Kind, Prepared};

/// The test data clock file each engine gets (`MOQUI_TEST_DATA_CLOCK_FILE`): epoch milliseconds, written whole and
/// renamed into place, so an engine never reads half of it. An engine that does not read it uses the real clock.
pub(crate) struct Clock {
    path: PathBuf,
    millis: std::sync::Mutex<i64>,
}

impl Clock {
    pub(crate) fn new(path: &Path, millis: i64) -> Result<Self, Error> {
        let clock = Self {
            path: path.to_path_buf(),
            millis: std::sync::Mutex::new(millis),
        };
        clock.write(millis)?;
        Ok(clock)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn now(&self) -> i64 {
        *moqui_lib::sync::lock(&self.millis)
    }

    /// Move the clock on by `step` milliseconds.
    pub(crate) fn advance(&self, step: i64) -> Result<i64, Error> {
        let mut millis = moqui_lib::sync::lock(&self.millis);
        *millis += step;
        self.write(*millis)?;
        Ok(*millis)
    }

    fn write(&self, millis: i64) -> Result<(), Error> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| Error::new(format!("could not create {}", dir.display())).with_source(err))?;
        }
        let next = self.path.with_extension("next");
        std::fs::write(&next, format!("{millis}\n"))
            .and_then(|()| std::fs::rename(&next, &self.path))
            .map_err(|err| Error::new(format!("could not write the data clock {}", self.path.display())).with_source(err))
    }
}

/// A started engine.
pub(crate) struct Engine {
    child: Child,
    port: u16,
    log: PathBuf,
    name: String,
}

/// The directories of a runtime that are a run's state, not its source: not copied.
const STATE_DIRS: [&str; 11] = [
    "log",
    "db",
    "txlog",
    "sessions",
    "script-classes",
    "save",
    "install",
    "opensearch",
    "tmp",
    "classes",
    "elasticsearch",
];

impl Engine {
    /// Copy the target's runtime into `dir`, start the engine on `database`, and wait until it has loaded its data
    /// and listens. The clock stays at its time while the engine boots.
    pub(crate) fn start(
        prepared: &Prepared,
        dir: &Path,
        database: Connection,
        clock: &Clock,
        timeout: f64,
    ) -> Result<Self, Error> {
        let runtime = dir.join("runtime");
        if runtime.exists() {
            std::fs::remove_dir_all(&runtime)
                .map_err(|err| Error::new(format!("could not clear {}", runtime.display())).with_source(err))?;
        }
        copy_tree(&prepared.runtime, &runtime, true)?;
        let port = free_port()?;
        let conf = dir.join("conf.xml");
        write_conf(&prepared.runtime.join("conf/MoquiDevConf.xml"), &conf)?;
        let log = dir.join("engine.log");
        let out = std::fs::File::create(&log)
            .map_err(|err| Error::new(format!("could not create {}", log.display())).with_source(err))?;
        let err_out = out
            .try_clone()
            .map_err(|err| Error::new(format!("could not open {}", log.display())).with_source(err))?;
        let mut command = match &prepared.kind {
            Kind::Rust { binary } => {
                let mut command = Command::new(binary);
                command
                    .arg("--conf")
                    .arg(&conf)
                    .args(["--port", &port.to_string(), "--https-port", "off", "--no-run-es"])
                    .args(["-D", "create-or-update=true"])
                    .env("MOQUI_SHARED_RUNTIME", "off")
                    .env("MOQUI_INSTALL", runtime.join("install"));
                command
            }
            Kind::Java { war } => {
                java_driver(&runtime)?;
                let mut command = Command::new(java());
                command
                    .arg(format!("-Dmoqui.runtime={}", runtime.display()))
                    .arg(format!("-Dmoqui.conf={}", conf.display()))
                    .arg("-jar")
                    .arg(war)
                    .arg(format!("port={port}"))
                    .arg("no-run-es");
                command
            }
        };
        let framework = crate::framework_from_env(&runtime);
        command
            .current_dir(dir)
            .env("MOQUI_RUNTIME", &runtime)
            .env("MOQUI_CONF", &conf)
            .env("MOQUI_FRAMEWORK", framework)
            .env("TZ", "UTC")
            .env(moqui_lib::data_clock::ENV, clock.path())
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err_out);
        for (name, value) in properties(&database, port) {
            command.env(name, value);
        }
        new_session(&mut command);
        let child = command
            .spawn()
            .map_err(|err| Error::new(format!("{} did not start", prepared.name)).with_source(err))?;
        let mut engine = Self {
            child,
            port,
            log,
            name: prepared.name.clone(),
        };
        if let Err(err) = engine.wait_ready(&prepared.kind, Duration::from_secs_f64(timeout)) {
            let _ = engine.stop();
            return Err(err);
        }
        Ok(engine)
    }

    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Until the boot log shows the data loaded and the listener up, and the port answers.
    fn wait_ready(&mut self, kind: &Kind, timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().ok().flatten() {
                let end = tail(&self.log);
                let hint = if end.contains("UnsupportedClassVersionError") {
                    format!("\n{} is too old for this build: set JAVA_HOME to a JDK 21 or newer", java().display())
                } else {
                    String::new()
                };
                return Err(Error::new(format!(
                    "{} exited before it was ready ({status}); the end of {}:\n{end}{hint}",
                    self.name,
                    self.log.display(),
                )));
            }
            let text = std::fs::read_to_string(&self.log).unwrap_or_default();
            if ready(kind, &text, self.port)
                && std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], self.port).into(), Duration::from_millis(500))
                    .is_ok()
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::new(format!(
                    "{} was not ready in {}s; the end of {}:\n{}",
                    self.name,
                    timeout.as_secs(),
                    self.log.display(),
                    tail(&self.log)
                )));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Ask the engine to stop (SIGTERM to its process group), and kill it when it does not stop in two minutes.
    pub(crate) fn stop(&mut self) -> Result<(), Error> {
        if self.child.try_wait().ok().flatten().is_some() {
            return Ok(());
        }
        terminate(&self.child);
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return Err(Error::new(format!("{} did not stop in 120s and was killed", self.name)));
                }
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The boot log says the engine loaded its data and listens on `port`.
pub(crate) fn ready(kind: &Kind, log: &str, port: u16) -> bool {
    match kind {
        Kind::Rust { .. } => {
            log.contains("Table check and empty-db load finished")
                && log.lines().any(|line| {
                    let Some(at) = line.find("Listening ") else { return false };
                    line[at..].split_whitespace().any(|word| word.ends_with(&format!(":{port}")))
                })
        }
        Kind::Java { .. } => {
            log.contains("Moqui Framework initialized in")
                && (log.contains("Started Server@") || log.contains("Started oejs.Server@"))
        }
    }
}

/// The settings the engines read from the environment: the database, the listeners, and no search cluster. Moqui
/// Java's conf reads `entity_ds_*`; mr-moqui's dev conf reads the `moqui_conf__` names of the transactional
/// datasource (runtime/conf/MoquiDevConf.xml), and the content datasource shares that database.
pub(crate) fn properties(database: &Connection, port: u16) -> Vec<(String, String)> {
    let rust = |name: &str| format!("moqui_conf__entity_facade__datasource__TRANSACTIONAL__{name}");
    let mut out = vec![
        (rust("database_conf_name"), "postgres".to_string()),
        (rust("host"), "127.0.0.1".to_string()),
        (rust("port"), database.port.to_string()),
        (rust("database"), database.database.clone()),
        (rust("user"), database.user.clone()),
        (rust("password"), database.password.clone()),
        (rust("sslmode"), "disable".to_string()),
    ];
    out.extend([
        ("entity_ds_db_conf", "postgres".to_string()),
        ("entity_ds_host", "127.0.0.1".to_string()),
        ("entity_ds_port", database.port.to_string()),
        ("entity_ds_database", database.database.clone()),
        ("entity_ds_schema", "public".to_string()),
        ("entity_ds_user", database.user.clone()),
        ("entity_ds_password", database.password.clone()),
        // a fixed key, so encrypted fields are the same text on both engines
        ("entity_ds_crypt_pass", "moqui-test-regression-crypt-pass".to_string()),
        ("entity_ds_sslmode", "disable".to_string()),
        ("entity_empty_db_load", "all".to_string()),
        ("webapp_http_port", port.to_string()),
        ("webapp_https_enabled", "false".to_string()),
        ("cluster_https_port", "0".to_string()),
        ("ssh_port", "0".to_string()),
        ("opensearch_run", "false".to_string()),
        ("instance_purpose", "dev".to_string()),
    ]
    .map(|(name, value)| (name.to_string(), value)));
    out
}

/// The runtime's dev conf with no gRPC listener of its own (`webapp[webroot]@grpc-listen-port` off), so the engine
/// does not take the port of a server that already runs here, and plain HTTP only: the dev conf serves HTTPS only, and
/// its screens redirect a plain request to the HTTPS port, which the runner does not open.
fn write_conf(source: &Path, conf: &Path) -> Result<(), Error> {
    let text = std::fs::read_to_string(source)
        .map_err(|err| Error::new(format!("could not read {}", source.display())).with_source(err))?;
    std::fs::write(conf, test_conf(&text))
        .map_err(|err| Error::new(format!("could not write {}", conf.display())).with_source(err))
}

pub(crate) fn test_conf(text: &str) -> String {
    let mut text = text.replace(" http-listen-port=\"off\"", "").replace("https-enabled=\"true\"", "https-enabled=\"false\"");
    if !text.contains("grpc-listen-port=") {
        text = text.replacen("<webapp name=\"webroot\"", "<webapp name=\"webroot\" grpc-listen-port=\"off\"", 1);
    }
    text
}

/// Java's Postgres JDBC driver, from `MOQUI_TEST_JDBC_JAR` or the checkout's own `runtime/lib`, into the copy.
fn java_driver(runtime: &Path) -> Result<(), Error> {
    let lib = runtime.join("lib");
    let has_driver = |dir: &Path| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                name.starts_with("postgresql-") && name.ends_with(".jar")
            })
        })
    };
    if has_driver(&lib) {
        return Ok(());
    }
    let jar = std::env::var_os("MOQUI_TEST_JDBC_JAR").map(PathBuf::from).ok_or_else(|| {
        Error::new(
            "the Java runtime has no Postgres JDBC driver (runtime/lib/postgresql-*.jar): put one there, or name it with MOQUI_TEST_JDBC_JAR",
        )
    })?;
    std::fs::create_dir_all(&lib).map_err(|err| Error::new(format!("could not create {}", lib.display())).with_source(err))?;
    let name = jar.file_name().unwrap_or_default();
    std::fs::copy(&jar, lib.join(name))
        .map(|_| ())
        .map_err(|err| Error::new(format!("could not copy {}", jar.display())).with_source(err))
}

fn java() -> PathBuf {
    std::env::var_os("JAVA_HOME")
        .map(|home| PathBuf::from(home).join("bin/java"))
        .filter(|java| java.is_file())
        .unwrap_or_else(|| PathBuf::from("java"))
}

/// A port nothing listens on now.
fn free_port() -> Result<u16, Error> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|err| Error::new("could not find a free port").with_source(err))
}

/// Copy `from` into `to`; at the top, not the run state ([`STATE_DIRS`]). Symbolic links are copied as links.
pub(crate) fn copy_tree(from: &Path, to: &Path, top: bool) -> Result<(), Error> {
    std::fs::create_dir_all(to).map_err(|err| Error::new(format!("could not create {}", to.display())).with_source(err))?;
    let entries = std::fs::read_dir(from).map_err(|err| Error::new(format!("could not read {}", from.display())).with_source(err))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if top && STATE_DIRS.iter().any(|state| name == *state) {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        let kind = entry
            .file_type()
            .map_err(|err| Error::new(format!("could not read {}", source.display())).with_source(err))?;
        if kind.is_symlink() {
            let link = std::fs::read_link(&source)
                .map_err(|err| Error::new(format!("could not read {}", source.display())).with_source(err))?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&link, &target)
                .map_err(|err| Error::new(format!("could not link {}", target.display())).with_source(err))?;
            #[cfg(not(unix))]
            {
                let _ = link;
                std::fs::copy(&source, &target)
                    .map_err(|err| Error::new(format!("could not copy {}", source.display())).with_source(err))?;
            }
        } else if kind.is_dir() {
            copy_tree(&source, &target, false)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|err| Error::new(format!("could not copy {}", source.display())).with_source(err))?;
        }
    }
    Ok(())
}

fn tail(log: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut file) = std::fs::File::open(log) {
        let _ = file.read_to_string(&mut text);
    }
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

#[cfg(unix)]
fn new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn new_session(_command: &mut Command) {}

#[cfg(unix)]
fn terminate(child: &Child) {
    // the group new_session made, so the engine's own children stop too
    if let Some(group) = i32::try_from(child.id()).ok().and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
    }
}

#[cfg(not(unix))]
fn terminate(_child: &Child) {}
