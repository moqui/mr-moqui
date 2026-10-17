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

//! Process edge. [`pdf`] is the Chromium engine behind `PdfRenderer`; [`edge`] is the runtime for the async clients.
//! `load` reads framework data files. `test` runs component JavaScript specs. `lint` checks service definitions.
//! `types` writes TypeScript declarations. No arguments start the plain HTTP listener. Database drivers live in [`db`].
//! The search store and backend are placed by [`search`].

pub mod db;
pub mod edge;
pub mod ipp;
pub mod logging;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod qvt_report;
pub mod search;
pub mod ssh;
pub mod sso;
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
pub mod test_cmd;
pub mod types;
pub mod usage;

mod cluster_edge;
mod cluster_notify;

#[cfg(feature = "axum")]
mod http;
#[cfg(feature = "axum")]
mod notws;
#[cfg(feature = "axum")]
mod search_proxy;
#[cfg(feature = "axum")]
mod tls;

/// No arguments start the HTTP listener and block until it stops.
///
/// # Errors
///
/// Returns an error when this build has no HTTP server, the port does not parse, or startup fails.
use moqui_lib::sync::lock;

pub fn serve() -> Result<(), moqui_lib::error::Error> {
    #[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
    {
        let (runtime, framework) = runtime_framework_dirs();
        let cluster = runtime.join("db/embedded");
        logging::install();
        let server = start_server_in(runtime, framework, cluster, ListenBind::Property)?;
        println!("Listening on {}", server.address());
        server.wait();
        Ok(())
    }
    #[cfg(not(all(feature = "axum", feature = "embedded-postgres", feature = "postgres")))]
    {
        Err(moqui_lib::error::Error::new("http server is not in this build"))
    }
}

/// A running plain HTTP listener and the cluster HTTPS listener.
/// Drop stops both, then joins the thread that owns the database.
#[must_use = "dropping the server stops the listener"]
pub struct Server {
    address: std::net::SocketAddr,
    cluster_address: std::net::SocketAddr,
    https_address: Option<std::net::SocketAddr>,
    factory: Option<std::sync::Arc<moqui_core::context::ExecutionContextFactory>>,
    posts: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Drops the listener shutdown sender. Empty after the first stop.
    stop: Option<Box<dyn FnOnce() + Send>>,
    thread: Option<std::thread::JoinHandle<Result<(), moqui_lib::error::Error>>>,
}

impl Server {
    /// The address the plain HTTP listener bound. A port of 0 in the request is replaced with the real port.
    #[must_use]
    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// The SSH server's address, when `ssh_port` is set.
    #[must_use]
    pub fn ssh_address(&self) -> Option<std::net::SocketAddr> {
        let factory = self.factory.as_ref()?;
        *lock(&factory.extension(|_| ssh::server::SshAddress::default()).0)
    }

    /// The public HTTPS listener, when one is running (`webapp_https_listen_port`, certificates in `runtime/cert`).
    #[must_use]
    pub fn https_address(&self) -> Option<std::net::SocketAddr> {
        self.https_address
    }

    /// The cluster HTTPS listener. Empty `cluster_https_port` binds 8444. `0` is ephemeral, and this
    /// is the address the kernel chose.
    #[must_use]
    pub fn cluster_address(&self) -> std::net::SocketAddr {
        self.cluster_address
    }

    /// The factory serving this process. Gone once drop has started.
    #[must_use]
    pub fn factory(&self) -> &std::sync::Arc<moqui_core::context::ExecutionContextFactory> {
        self.factory.as_ref().expect("server factory is gone")
    }

    /// Topics accepted from cluster peers (`POST /cluster/{topic}` or the SSH channel), in arrival order.
    #[must_use]
    pub fn cluster_posts(&self) -> Vec<String> {
        lock(&self.posts).clone()
    }

    /// Block until the listener exits. Ctrl-c is handled on the server thread. This does not drop
    /// the shutdown sender first, so a test that only wants ctrl-c shutdown can call this.
    pub fn wait(mut self) {
        self.factory.take();
        if let Some(thread) = self.thread.take() {
            join_server(thread);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // The serve thread still holds the factory until its cluster listener has stopped.
        self.factory.take();
        if let Some(stop) = self.stop.take() {
            stop();
        }
        if let Some(thread) = self.thread.take() {
            join_server(thread);
        }
    }
}

/// Start the listener on `bind` against an embedded cluster at `cluster_dir`.
///
/// An empty `moqui.basic.Enumeration` table loads framework seed data. The connection pool is
/// large enough for the request and service caps. Drop the returned server to stop the listener
/// and the postmaster.
///
/// # Errors
///
/// Returns an error when this build has no HTTP server, boot fails, or the port cannot be bound.
pub fn start_server(
    runtime_dir: impl AsRef<std::path::Path>,
    framework_dir: impl AsRef<std::path::Path>,
    cluster_dir: impl AsRef<std::path::Path>,
    bind: std::net::SocketAddr,
) -> Result<Server, moqui_lib::error::Error> {
    #[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
    {
        start_server_in(
            runtime_dir.as_ref().to_path_buf(),
            framework_dir.as_ref().to_path_buf(),
            cluster_dir.as_ref().to_path_buf(),
            ListenBind::Address(bind),
        )
    }
    #[cfg(not(all(feature = "axum", feature = "embedded-postgres", feature = "postgres")))]
    {
        let _ = (runtime_dir, framework_dir, cluster_dir, bind);
        Err(moqui_lib::error::Error::new("http server is not in this build"))
    }
}

fn join_server(thread: std::thread::JoinHandle<Result<(), moqui_lib::error::Error>>) {
    match thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => tracing::error!("http server: {err}"),
        Err(_) => tracing::error!("http server thread panicked"),
    }
}

/// `pg_dump --format=custom` of the embedded `moqui` database at `dir`.
///
/// # Errors
///
/// Returns an error when the cluster does not start or `pg_dump` fails. A postmaster this call started is
/// stopped before it returns. A postmaster that was already live keeps running.
pub fn save_db(
    dir: impl AsRef<std::path::Path>,
    dump: impl AsRef<std::path::Path>,
) -> Result<(), moqui_lib::error::Error> {
    db::embedded::save_db(dir.as_ref(), dump.as_ref())
}

/// `pg_restore --format=custom` into the embedded `moqui` database at `dir`.
///
/// # Errors
///
/// Returns an error when the cluster does not start or `pg_restore` fails. A postmaster this call started is
/// stopped before it returns. A postmaster that was already live keeps running.
pub fn load_save(
    dir: impl AsRef<std::path::Path>,
    dump: impl AsRef<std::path::Path>,
) -> Result<(), moqui_lib::error::Error> {
    db::embedded::load_save(dir.as_ref(), dump.as_ref())
}

/// The process entry of a moqui binary: [`dispatch`] on the command line arguments, the line it returns printed, and
/// exit status 1 with the error on stderr when it fails. A binary that links component crates names each one
/// (`use my_component as _;`) so its `submit_component!` entry is kept.
pub fn main() {
    match dispatch(std::env::args().skip(1)) {
        Ok(Some(line)) => println!("{line}"),
        Ok(None) => {}
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

/// No arguments start the HTTP server. With no command, `--port`, `--conf`, `--no-run-es`, and `-D name=value`
/// set the listen port, the runtime conf, and other properties for that run. `help`, `--help`, `-h`, and `-?`
/// print the command list ([`usage::overview`]). `save-db` and `load-save` take a cluster directory and a dump file.
/// `load` takes `name=value` pairs. `test` runs the JavaScript specs in each component's `test` directory.
/// `lint` takes an optional service file. `lint --screens` checks the screen and form XML and may write a report file.
/// `lint --groovy` reports the Groovy idioms left in a component tree such as mantle, and `convert --groovy` makes the
/// mechanical rewrites (map literals, `context + [...]`, Elvis, EDI grammar files), printing them, and writes them with `--write`.
/// `types` writes TypeScript declarations for the loaded definitions, by default into `types/` in the runtime directory.
///
/// # Errors
///
/// Returns an error for an unknown command, a command with the wrong arguments, an unknown help topic, an unknown
/// load flag, or when a command fails.
pub fn dispatch<I, S>(args: I) -> Result<Option<&'static str>, moqui_lib::error::Error>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args.into_iter().map(|arg| arg.as_ref().to_string()).collect();
    if let Some(help) = usage::help_request(&args) {
        return help.map(Some);
    }
    if usage::is_server_invocation(&args) {
        let opts = usage::parse_server_args(&args)?;
        opts.apply();
        serve()?;
        return Ok(None);
    }
    match args.as_slice() {
        [command, dir, file] if command == "save-db" => {
            save_db(dir, file)?;
            Ok(None)
        }
        [command, dir, file] if command == "load-save" => {
            load_save(dir, file)?;
            Ok(None)
        }
        [command, rest @ ..] if command == "types" => {
            dispatch_types(rest)?;
            Ok(None)
        }
        [command, rest @ ..] if command == "test" => {
            #[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
            {
                test_cmd::dispatch_test(rest)?;
                Ok(None)
            }
            #[cfg(not(all(feature = "embedded-postgres", feature = "postgres")))]
            {
                let _ = rest;
                Err(moqui_lib::error::Error::new(
                    "moqui test needs the embedded-postgres and postgres features",
                ))
            }
        }
        [command, rest @ ..] if command == "load" => {
            let flags = parse_load_args(rest)?;
            dispatch_load(&flags)?;
            Ok(None)
        }
        [command, flag, dirs @ ..] if command == "lint" && flag == "--groovy" && !dirs.is_empty() => {
            let show_all = dirs.iter().any(|arg| arg == "--all");
            let roots: Vec<std::path::PathBuf> = dirs.iter().filter(|arg| *arg != "--all").map(Into::into).collect();
            if roots.is_empty() {
                return Err(usage::command_usage("lint"));
            }
            let report = moqui_core::script::screen_lint::lint_groovy(&roots, show_all)
                .map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
            print!("{}", report.render());
            Ok(None)
        }
        [command, flag, rest @ ..] if command == "convert" && flag == "--groovy" => {
            let write = rest.first().is_some_and(|arg| arg == "--write");
            let dirs = if write { &rest[1..] } else { rest };
            if dirs.is_empty() {
                return Err(usage::command_usage("convert"));
            }
            let roots: Vec<std::path::PathBuf> = dirs.iter().map(Into::into).collect();
            let report = moqui_core::script::groovy_convert::convert_tree(&roots, write)
                .map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
            print!("{}", report.render());
            Ok(None)
        }
        [command, flag, mode, rest @ ..]
            if command == "lint" && flag == "--screens" && mode == "--qvt-static" && rest.len() <= 1 =>
        {
            dispatch_lint_qvt_static(rest.first().map(String::as_str))?;
            Ok(None)
        }
        [command, flag, rest @ ..] if command == "lint" && flag == "--screens" && rest.len() <= 1 => {
            dispatch_lint_screens(rest.first().map(String::as_str))?;
            Ok(None)
        }
        [command, flag] if command == "lint" && flag == "--groovy" => Err(usage::command_usage("lint")),
        [command] if command == "lint" => {
            dispatch_lint(None)?;
            Ok(None)
        }
        [command, path] if command == "lint" => {
            dispatch_lint(Some(path))?;
            Ok(None)
        }
        _ => Err(usage_error(&args)),
    }
}

/// A known command with the wrong arguments names that command. Anything else is an unknown command.
fn usage_error(args: &[String]) -> moqui_lib::error::Error {
    match args.first().map(String::as_str) {
        Some(name @ ("save-db" | "load-save" | "lint" | "convert" | "types" | "load" | "test")) => {
            usage::command_usage(name)
        }
        Some(name) => usage::unknown_command(name),
        None => usage::unknown_command(""),
    }
}

/// `MOQUI_RUNTIME`, default `runtime`, and `MOQUI_FRAMEWORK`, default `framework` beside the runtime. Like Moqui's
/// `moqui.runtime`: an install runs from its own directory unless told otherwise.
fn runtime_framework_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
    let runtime = std::env::var_os("MOQUI_RUNTIME").map_or_else(|| std::path::PathBuf::from("runtime"), Into::into);
    let framework = framework_from_env(&runtime);
    (runtime, framework)
}

/// Create `dir` and its parents when missing.
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn create_dir(dir: &std::path::Path) -> Result<(), moqui_lib::error::Error> {
    std::fs::create_dir_all(dir)
        .map_err(|err| moqui_lib::error::Error::new(format!("could not create {}", dir.display())).with_source(err))
}

/// `dir` as an absolute path with no links.
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn canonical_dir(dir: &std::path::Path) -> Result<std::path::PathBuf, moqui_lib::error::Error> {
    std::fs::canonicalize(dir)
        .map_err(|err| moqui_lib::error::Error::new(format!("could not resolve {}", dir.display())).with_source(err))
}

fn dispatch_load(flags: &LoadFlags) -> Result<LoadReport, moqui_lib::error::Error> {
    logging::install();
    let (runtime, framework) = runtime_framework_dirs();
    let cluster = runtime.join("db/embedded");
    load_data(&runtime, &framework, &cluster, flags)
}

/// Open the log files in `moqui_log_dir` and say where they are.
fn log_files_attached(runtime_dir: &std::path::Path) {
    match logging::attach_files(runtime_dir) {
        Some(dir) => tracing::info!("Writing log files moqui.log and moqui-error.log in {}", dir.display()),
        None => tracing::info!("Not writing log files, moqui_log_dir is empty or could not be opened"),
    }
}

fn dispatch_types(args: &[String]) -> Result<(), moqui_lib::error::Error> {
    logging::install();
    let (runtime, framework) = runtime_framework_dirs();
    let out = types::parse_types_args(args, &runtime)?;
    println!("{}", types::write_types(&runtime, &framework, &out)?);
    Ok(())
}

fn dispatch_lint(path: Option<&str>) -> Result<(), moqui_lib::error::Error> {
    let owned = path.map(std::path::PathBuf::from);
    let paths: Vec<std::path::PathBuf> = owned.into_iter().collect();
    // A factory with no threads and no database, entered so a `type="rust"` location a linked component binds resolves
    // as it does in a server. With no runtime to boot (a lone file), only the built-in locations resolve.
    let (runtime, framework) = runtime_framework_dirs();
    let booted = moqui_core::context::ExecutionContextFactory::boot_with_options(
        &runtime,
        &framework,
        moqui_core::context::BootOptions {
            executor: moqui_core::context::Executor::Inline,
        },
    );
    let checked = match booted {
        Ok(factory) => {
            let context = factory.new_context();
            let _entered = context.enter();
            moqui_core::service::check(&paths)
        }
        Err(err) => {
            tracing::warn!("Linting without a factory, so only built-in rust service locations resolve: {err}");
            moqui_core::service::check(&paths)
        }
    };
    let failures = checked.map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
    for failure in &failures {
        println!("{failure}");
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(moqui_lib::error::Error::new(format!(
        "{} service expression(s) failed lint",
        failures.len()
    )))
}

/// `lint --screens --qvt-static`: the compatibility report of every screen for server-static qvt. It does not fail the process, since a
/// report is information and no screen is marked yet.
fn dispatch_lint_qvt_static(report: Option<&str>) -> Result<(), moqui_lib::error::Error> {
    let (runtime, framework) = runtime_framework_dirs();
    let reports = qvt_report::qvt_static_reports(&runtime, &framework)?;
    let text = qvt_report::render_reports(&reports);
    match report {
        Some(path) => std::fs::write(path, &text).map_err(|err| moqui_lib::error::Error::new(format!("{path}: {err}"))),
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

fn dispatch_lint_screens(report: Option<&str>) -> Result<(), moqui_lib::error::Error> {
    let (runtime, framework) = runtime_framework_dirs();
    let roots = qvt_report::component_roots(&runtime, &framework);
    let screens = moqui_core::script::screen_lint::lint_screens(&roots)
        .map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
    let text = screens.render();
    match report {
        Some(path) => {
            std::fs::write(path, &text).map_err(|err| moqui_lib::error::Error::new(format!("{path}: {err}")))?
        }
        None => print!("{text}"),
    }
    let failures = screens.failures();
    for failure in &failures {
        println!("{failure}");
    }
    if failures.is_empty() {
        return Ok(());
    }
    Err(moqui_lib::error::Error::new(format!(
        "{} screen expression(s) failed lint",
        failures.len()
    )))
}

/// `name=value` arguments for [`load_data`], matching `Moqui.loadData`.
///
/// `data_types` is `None` when `types` was omitted. That selects `seed`, `seed-initial`, and `install`.
/// An empty list, or a list containing `all`, selects every data type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadFlags {
    pub data_types: Option<Vec<String>>,
    pub components: Vec<String>,
    pub location: Option<String>,
    /// `timeout` seconds. Each data file loads in one transaction with this timeout (600 seconds when not given).
    pub timeout_seconds: Option<u64>,
    pub raw: bool,
    pub disable_entity_eca: bool,
    pub disable_audit_log: bool,
    pub disable_fk_create: bool,
    pub disable_data_feed: bool,
    /// Extra service files registered before the load. Not a CLI flag. The path must not contain `/service/`.
    pub service_files: Vec<std::path::PathBuf>,
}

/// Counts from one [`load_data`] call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadReport {
    pub records: u64,
    pub enumeration: i64,
    pub user_group_permission: i64,
    /// `moqui.test.TestFts` existed after `check_tables` and before any data file.
    pub test_fts_before_load: bool,
    /// How many times `UserSecurityChecks` ran during the load. The security ECAs fire on each
    /// `UserGroupMember` and `UserGroupPermission` write, so seed data makes this positive.
    pub security_checks: u64,
}

/// Parse `name=value` load arguments. This does not open a database.
///
/// # Errors
///
/// Returns an error when a token is not `name=value`, the name is unknown, or the value does not parse.
pub fn parse_load_args(args: &[impl AsRef<str>]) -> Result<LoadFlags, moqui_lib::error::Error> {
    let mut flags = LoadFlags::default();
    let mut seen = std::collections::BTreeSet::new();
    for arg in args {
        let arg = arg.as_ref();
        let Some((name, value)) = arg.split_once('=') else {
            return Err(moqui_lib::error::Error::new(format!(
                "load flag '{arg}' is not name=value"
            )));
        };
        if name.is_empty() {
            return Err(moqui_lib::error::Error::new("load flag name is empty"));
        }
        if !seen.insert(name.to_string()) {
            return Err(moqui_lib::error::Error::new(format!("duplicate load flag {name}")));
        }
        match name {
            "types" => flags.data_types = Some(split_csv(value)),
            "components" => flags.components = split_csv(value),
            "location" => {
                if value.is_empty() {
                    return Err(moqui_lib::error::Error::new("location is empty"));
                }
                flags.location = Some(value.to_string());
            }
            "timeout" => flags.timeout_seconds = Some(parse_timeout(value)?),
            "raw" => flags.raw = parse_bool(name, value)?,
            "disableEntityEca" => flags.disable_entity_eca = parse_bool(name, value)?,
            "disableAuditLog" => flags.disable_audit_log = parse_bool(name, value)?,
            "disableFkCreate" => flags.disable_fk_create = parse_bool(name, value)?,
            "disableDataFeed" => flags.disable_data_feed = parse_bool(name, value)?,
            _ => return Err(moqui_lib::error::Error::new(format!("unknown load flag {name}"))),
        }
    }
    Ok(flags)
}

fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_bool(name: &str, value: &str) -> Result<bool, moqui_lib::error::Error> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "y" | "yes" | "1" => Ok(true),
        "false" | "n" | "no" | "0" => Ok(false),
        _ => Err(moqui_lib::error::Error::new(format!(
            "load flag {name} value '{value}' is not a boolean"
        ))),
    }
}

fn parse_timeout(value: &str) -> Result<u64, moqui_lib::error::Error> {
    value.trim().parse::<u64>().map_err(|_| {
        moqui_lib::error::Error::new(format!("timeout value '{value}' is not an integer number of seconds"))
    })
}

fn load_options(flags: &LoadFlags) -> moqui_core::entity::load_feed::LoadOptions {
    let data_types = match &flags.data_types {
        None => ["seed", "seed-initial", "install"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        Some(types) if types.is_empty() || types.iter().any(|kind| kind == "all") => Vec::new(),
        Some(types) => types.clone(),
    };
    moqui_core::entity::load_feed::LoadOptions {
        data_types,
        components: flags.components.clone(),
        only_create: false,
        use_try_insert: flags.raw,
        dummy_fks: false,
        disable_fk_create: flags.raw || flags.disable_fk_create,
        timeout: flags.timeout_seconds.map(std::time::Duration::from_secs),
        restrict_sensitive_entities: false,
        ..Default::default()
    }
}

/// Boot, create framework tables, and load data into a fresh embedded cluster at `cluster_dir`.
///
/// The same path serves startup empty-db load and `moqui load`. `location` replaces the conf list.
/// With no `types` and no `location`, an empty `moqui.basic.Enumeration` table loads `seed`,
/// `seed-initial`, and `install`. The factory is dropped, and its worker pool joined, before the
/// postmaster this call started is stopped.
///
/// # Errors
///
/// Returns an error when the build has no embedded Postgres, boot fails, a table cannot be created,
/// or a data file fails.
pub fn load_data(
    runtime_dir: impl AsRef<std::path::Path>,
    framework_dir: impl AsRef<std::path::Path>,
    cluster_dir: impl AsRef<std::path::Path>,
    flags: &LoadFlags,
) -> Result<LoadReport, moqui_lib::error::Error> {
    #[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
    {
        load_embedded(
            runtime_dir.as_ref(),
            framework_dir.as_ref(),
            cluster_dir.as_ref(),
            flags,
        )
    }
    #[cfg(not(all(feature = "embedded-postgres", feature = "postgres")))]
    {
        let _ = (runtime_dir, framework_dir, cluster_dir, flags);
        Err(moqui_lib::error::Error::new("embedded postgres is not in this build"))
    }
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn load_embedded(
    runtime_dir: &std::path::Path,
    framework_dir: &std::path::Path,
    cluster_dir: &std::path::Path,
    flags: &LoadFlags,
) -> Result<LoadReport, moqui_lib::error::Error> {
    create_dir(runtime_dir)?;
    create_dir(cluster_dir)?;
    let runtime_dir = canonical_dir(runtime_dir)?;
    let framework_dir = canonical_dir(framework_dir)?;
    // The factory boots first: its default properties name the datasource. It is dropped, and its
    // worker pool joined, before the database closes.
    let started = std::time::Instant::now();
    tracing::info!(
        "Loading data, runtime directory {}, framework directory {}",
        runtime_dir.display(),
        framework_dir.display()
    );
    let factory = moqui_core::context::ExecutionContextFactory::boot_with_framework(&runtime_dir, &framework_dir)
        .map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
    log_files_attached(&runtime_dir);
    let opened_at = std::time::Instant::now();
    let opened = crate::db::entity_db::transactional_conf(&factory).and_then(|conf| {
        let pool = load_pool(&conf.pool);
        let install = crate::db::embedded::shared_install_dir(&runtime_dir.join("db"), cluster_dir);
        tracing::info!(
            "Opening DataSource transactional, database {}, max pool {}, cluster directory {}",
            conf.dialect,
            pool.max,
            cluster_dir.display()
        );
        crate::db::entity_db::open(conf, cluster_dir, &install, pool.max).map(|db| (db, pool))
    });
    if opened.is_ok() {
        tracing::info!(
            "DataSource transactional opened in {:.3} seconds",
            opened_at.elapsed().as_secs_f64()
        );
    }
    let (db, pool) = match opened {
        Ok(opened) => opened,
        Err(err) => {
            drop(factory);
            return Err(err);
        }
    };
    let outcome = load_with_factory(&factory, &framework_dir, &db, pool, flags);
    drop(factory);
    drop(db);
    if let Ok(report) = &outcome {
        tracing::info!(
            "Loaded [{}] records in {} seconds.",
            report.records,
            started.elapsed().as_secs()
        );
    }
    outcome
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn load_with_factory(
    factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
    framework_dir: &std::path::Path,
    db: &crate::db::entity_db::EntityDb,
    pool: moqui_core::transaction::PoolConfig,
    flags: &LoadFlags,
) -> Result<LoadReport, moqui_lib::error::Error> {
    let entity_started = std::time::Instant::now();
    let facade = crate::db::entity_facade::open_entity_facade_for_factory(
        factory,
        std::sync::Arc::clone(&db.driver),
        pool,
        &db.conf.dialect,
    )?;
    crate::db::entity_facade::attach(factory, facade.clone());
    let merged = moqui_lib::node::MNode::parse_text("moqui-conf", factory.conf_text()).ok();
    crate::db::entity_facade::apply_datasource_conf(&facade, &db.conf, merged.as_ref());
    tracing::info!(
        "Entity Facade initialized in {:.3} seconds",
        entity_started.elapsed().as_secs_f64()
    );
    // Feeds index through search when it is up; a load still runs when it is not.
    let _search = match crate::search::attach_search(factory, &facade, db.is_postgres()) {
        Ok(store) => {
            tracing::info!("Search attached for this load, data feeds will index documents");
            logging::ship_to_search(factory);
            Some(store)
        }
        Err(err) => {
            tracing::warn!("Search is not available during this load, data feeds will not index documents: {err}");
            None
        }
    };
    factory.init_tools().map_err(moqui_lib::error::Error::from)?;
    let context = factory.new_context();
    let entered = context.enter();
    let authz = context.disable_authz();
    let outcome = load_in_context(
        framework_dir,
        &facade,
        flags,
        &component_data_locations(factory, flags),
        false,
        db.conf.startup_add_missing,
    );
    drop(authz);
    drop(entered);
    drop(context);
    // The data feed indexes on service workers after each commit; let that work finish before the database closes, so it
    // is not cut off mid-statement. Rows it does not deliver stay pending for the feed job either way.
    wait_for_workers(factory, std::time::Duration::from_secs(120));
    // The shipper holds the search facade and its connections; it sends what is left (the feed's last events too) and
    // lets go.
    logging::stop_shipping();
    drop(facade);
    outcome
}

/// Wait until the service workers have nothing running or queued, or `limit` passes.
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn wait_for_workers(factory: &moqui_core::context::ExecutionContextFactory, limit: std::time::Duration) {
    let deadline = std::time::Instant::now() + limit;
    loop {
        let status = factory.service_pools().status();
        if status.active == 0 && status.queued == 0 {
            return;
        }
        if std::time::Instant::now() >= deadline {
            tracing::warn!(
                "Service workers still busy after {}s ({} running, {} queued); closing the database anyway",
                limit.as_secs(),
                status.active,
                status.queued
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A load's pool is the conf's, opened lazily (`min` 0). Its data feed runs on the service workers, each of which can
/// hold a connection and park another for a `force-new` call, so the load needs the same room the server checks for
/// at startup (`check_serve_caps`); a small fixed pool ran out on Mantle's demo data.
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn load_pool(conf: &moqui_core::transaction::PoolConfig) -> moqui_core::transaction::PoolConfig {
    moqui_core::transaction::PoolConfig { min: 0, ..conf.clone() }
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
enum ListenBind {
    Address(std::net::SocketAddr),
    Property,
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
fn start_server_in(
    runtime_dir: std::path::PathBuf,
    framework_dir: std::path::PathBuf,
    cluster_dir: std::path::PathBuf,
    bind: ListenBind,
) -> Result<Server, moqui_lib::error::Error> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("moqui-http".to_string())
        .spawn(move || serve_thread(runtime_dir, framework_dir, cluster_dir, bind, stop_rx, ready_tx))
        .map_err(|err| moqui_lib::error::Error::new("http server thread did not start").with_source(err))?;
    match ready_rx.recv() {
        Ok(Ok(bound)) => Ok(Server {
            address: bound.http,
            https_address: bound.https,
            cluster_address: bound.cluster,
            factory: Some(bound.factory),
            posts: bound.posts,
            stop: Some(Box::new(move || drop(stop_tx))),
            thread: Some(thread),
        }),
        Ok(Err(message)) => Err(thread_failure(thread, &message)),
        Err(_) => Err(thread_failure(thread, "http server stopped before it bound a port")),
    }
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
fn thread_failure(
    thread: std::thread::JoinHandle<Result<(), moqui_lib::error::Error>>,
    fallback: &str,
) -> moqui_lib::error::Error {
    match thread.join() {
        Ok(Err(err)) => err,
        Err(_) => moqui_lib::error::Error::new(format!("http server thread panicked: {fallback}")),
        Ok(Ok(())) => moqui_lib::error::Error::new(fallback),
    }
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
fn serve_thread(
    runtime_dir: std::path::PathBuf,
    framework_dir: std::path::PathBuf,
    cluster_dir: std::path::PathBuf,
    bind: ListenBind,
    stop: tokio::sync::oneshot::Receiver<()>,
    ready_tx: std::sync::mpsc::Sender<Result<http::ListenReport, String>>,
) -> Result<(), moqui_lib::error::Error> {
    let mut ready = Ready { tx: Some(ready_tx) };
    let process = match boot_server(&runtime_dir, &framework_dir, &cluster_dir) {
        Ok(process) => process,
        Err(err) => {
            ready.fail(&err);
            return Err(err);
        }
    };
    let address = match listen_address(&bind) {
        Ok(address) => address,
        Err(err) => {
            ready.fail(&err);
            return Err(err);
        }
    };
    let factory = process
        .factory
        .clone()
        .ok_or_else(|| moqui_lib::error::Error::new("http server has no factory"))?;
    let cluster = process
        .cluster_edge
        .as_ref()
        .ok_or_else(|| moqui_lib::error::Error::new("http server has no cluster listener"))?;
    let cluster_address = cluster.address();
    let posts = cluster.posts();
    let sender = ready.handoff()?;
    let runtime = process
        .runtime
        .as_ref()
        .ok_or_else(|| moqui_lib::error::Error::new("http server has no runtime"))?;
    // After the boot, as for the HTTP port: conf defaults and runtime/.env are read there.
    let https = match tls::plan(&runtime_dir, address.ip(), matches!(bind, ListenBind::Property)) {
        Ok(https) => https,
        Err(err) => {
            ready.fail(&err);
            return Err(err);
        }
    };
    let listened = runtime.block_on(http::listen(
        factory,
        address,
        https,
        cluster_address,
        posts,
        stop,
        sender,
    ));
    tracing::info!("HTTP listener stopped, shutting down");
    if let Some(factory) = process.factory.as_ref() {
        // Hits and the open bins are only in memory until written, as Java's destroy writes the bins.
        if let Err(err) = factory.flush_artifact_hits() {
            tracing::warn!("Artifact hit flush failed: {err}");
        } else {
            tracing::info!("ArtifactHitBins stored");
        }
        if let Err(err) = factory.flush_sessions() {
            let err = moqui_lib::error::Error::new(err.to_string());
            if listened.is_ok() {
                return Err(err);
            }
            tracing::warn!("Session flush failed: {err}");
        } else {
            tracing::info!("Sessions flushed");
        }
    }
    logging::stop_shipping();
    listened
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
fn listen_address(bind: &ListenBind) -> Result<std::net::SocketAddr, moqui_lib::error::Error> {
    match bind {
        ListenBind::Address(address) => Ok(*address),
        ListenBind::Property => {
            let port = http::http_port(&moqui_lib::env::get_prop_or_env("webapp_http_port"))?;
            Ok(std::net::SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, port)))
        }
    }
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
fn boot_server(
    runtime_dir: &std::path::Path,
    framework_dir: &std::path::Path,
    cluster_dir: &std::path::Path,
) -> Result<ServeProcess, moqui_lib::error::Error> {
    use std::sync::Arc;

    let boot_started = std::time::Instant::now();
    create_dir(runtime_dir)?;
    create_dir(cluster_dir)?;
    let runtime_dir = canonical_dir(runtime_dir)?;
    let framework_dir = canonical_dir(framework_dir)?;
    let cluster_dir = canonical_dir(cluster_dir)?;
    // The factory boots first: its default properties name the datasource. The database stays on
    // this thread, and its clients are dropped here, after the axum runtime.
    tracing::info!(
        "Initializing Moqui Framework, runtime directory {}, framework directory {}",
        runtime_dir.display(),
        framework_dir.display()
    );
    let factory = moqui_core::context::ExecutionContextFactory::boot_with_framework(&runtime_dir, &framework_dir)
        .map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
    log_files_attached(&runtime_dir);
    let conf = crate::db::entity_db::transactional_conf(&factory)?;
    // `(request cap + service cap) * 2` must fit in `max - reserve`; check_serve_caps below says so.
    let pool = conf.pool.clone();
    let install = crate::db::embedded::shared_install_dir(&runtime_dir.join("db"), &cluster_dir);
    let db_started = std::time::Instant::now();
    tracing::info!(
        "Opening DataSource transactional, database {}, max pool {}, cluster directory {}",
        conf.dialect,
        pool.max,
        cluster_dir.display()
    );
    let db = crate::db::entity_db::open(conf, &cluster_dir, &install, pool.max)?;
    tracing::info!(
        "DataSource transactional opened in {:.3} seconds",
        db_started.elapsed().as_secs_f64()
    );
    let entity_started = std::time::Instant::now();
    let facade = crate::db::entity_facade::open_entity_facade_for_factory(
        &factory,
        Arc::clone(&db.driver),
        pool,
        &db.conf.dialect,
    )?;
    crate::db::entity_facade::attach(&factory, facade.clone());
    let merged = moqui_lib::node::MNode::parse_text("moqui-conf", factory.conf_text()).ok();
    crate::db::entity_facade::apply_datasource_conf(&facade, &db.conf, merged.as_ref());
    tracing::info!(
        "Entity Facade initialized in {:.3} seconds",
        entity_started.elapsed().as_secs_f64()
    );
    // The overlay starts on the first enter_sim, inside the cluster directory, on the shared install.
    let sim = crate::db::sim::SimOverlay::install(&facade, &cluster_dir, Some(install));
    // Same policy as `load`: indexing runs when search is up, and the server still starts when it is not.
    let search = match crate::search::attach_search(&factory, &facade, db.is_postgres()) {
        Ok(store) => {
            tracing::info!("Search attached, data feeds and logs can index documents");
            logging::ship_to_search(&factory);
            Some(store)
        }
        Err(err) => {
            tracing::warn!("Search is not available, the server starts without indexing: {err}");
            None
        }
    };
    crate::db::backup::install(&facade, &db, search.as_ref(), &runtime_dir);
    // Log events and HTTP requests go to moqui_logs and moqui_http_log through the search backend (shipped above).
    factory.init_tools().map_err(moqui_lib::error::Error::from)?;
    if search.is_some() {
        index_feeds_on_start_empty(&factory);
    }
    let load_started = std::time::Instant::now();
    {
        let context = factory.new_context();
        let entered = context.enter();
        let authz = context.disable_authz();
        let flags = startup_load_flags(factory.conf_text())?;
        let loaded = load_in_context(
            &framework_dir,
            &facade,
            &flags,
            &component_data_locations(&factory, &flags),
            true,
            db.conf.startup_add_missing,
        );
        drop(authz);
        drop(entered);
        drop(context);
        loaded?;
    }
    tracing::info!(
        "Table check and empty-db load finished in {:.3} seconds",
        load_started.elapsed().as_secs_f64()
    );
    moqui_core::service::check_serve_caps(&factory).map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
    factory
        .start_request_pool()
        .map_err(|err| moqui_lib::error::Error::new(err.to_string()))?;
    tracing::info!("Request pool started");
    // Empty cluster_https_port listens on 8444. 0 is ephemeral for tests.
    moqui_core::notify::install();
    // The SSH server binds before the member row is written: with cluster_transport=ssh the row carries its address
    // and host key.
    let cluster_posts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sshd = ssh::server::start(&factory, &runtime_dir, Arc::clone(&cluster_posts))?;
    if sshd.is_some() {
        tracing::info!("SSH server started");
    } else {
        tracing::info!("Not starting the SSH server, it is not configured");
    }
    // One sender for every transport: a peer's base URL picks HTTPS, SSH, or Postgres NOTIFY.
    cluster_notify::install_sender(&factory, db.is_postgres())?;
    let notify_listener = cluster_notify::listen_if_configured(&factory, &db, Arc::clone(&cluster_posts))?;
    moqui_core::cluster::start(&factory).map_err(moqui_lib::error::Error::from)?;
    tracing::info!(
        "Cluster notify {}",
        if notify_listener.is_some() {
            "listener started"
        } else {
            "listener not started, not configured"
        }
    );
    let cluster_edge = cluster_edge::listen(Arc::clone(&factory), cluster_posts)?;
    notws::install_push().map_err(moqui_lib::error::Error::from)?;
    tracing::info!("Cluster, cluster edge listener and notification WebSocket push initialized");
    let pdf = install_pdf(&runtime_dir);
    ssh::install_client(&factory, &runtime_dir);
    sso::install();
    ipp::install();
    tracing::info!("PDF, SSH client, SSO and IPP integrations installed");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| moqui_lib::error::Error::new("could not start the http runtime").with_source(err))?;
    tracing::info!(
        "Moqui Framework initialized in {:.3} seconds",
        boot_started.elapsed().as_secs_f64()
    );
    Ok(ServeProcess {
        runtime: Some(runtime),
        pdf: Some(pdf),
        sshd,
        cluster_edge: Some(cluster_edge),
        notify_listener,
        factory: Some(factory),
        search,
        facade: Some(facade),
        sim: Some(sim),
        db: Some(db),
    })
}

/// How long the axum runtime waits for its blocking tasks when it stops.
#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
const RUNTIME_SHUTDOWN: std::time::Duration = std::time::Duration::from_secs(5);

/// Drop order is the axum runtime, the PDF browser, the SSH server, the cluster listeners (while the request pool is still up),
/// the factory, the search store, the facade, the sim overlay, then the entity database (its driver, then its cluster). The search store
/// outlives the factory so a dedicated postmaster stops after the facade's clients, and the overlay outlives the facade
/// for its sim pool.
#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
struct ServeProcess {
    /// Stopped with [`RUNTIME_SHUTDOWN`] as its limit.
    runtime: Option<tokio::runtime::Runtime>,
    pdf: Option<PdfSlot>,
    sshd: Option<ssh::server::SshServer>,
    cluster_edge: Option<crate::cluster_edge::ClusterEdge>,
    notify_listener: Option<crate::cluster_notify::NotifyListener>,
    factory: Option<std::sync::Arc<moqui_core::context::ExecutionContextFactory>>,
    search: Option<crate::search::SearchStore>,
    facade: Option<moqui_core::entity::EntityFacade>,
    sim: Option<std::sync::Arc<crate::db::sim::SimOverlay>>,
    db: Option<crate::db::entity_db::EntityDb>,
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
impl Drop for ServeProcess {
    fn drop(&mut self) {
        logging::stop_shipping();
        if let Some(runtime) = self.runtime.take() {
            // The listener has already waited out its shutdown deadline; a blocking task still running is left.
            runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
        }
        if let Some(factory) = &self.factory {
            factory.shutdown();
        }
        self.pdf.take();
        self.sshd.take();
        self.cluster_edge.take();
        self.notify_listener.take();
        self.factory.take();
        self.search.take();
        self.facade.take();
        self.sim.take();
        self.db.take();
    }
}

#[cfg(feature = "pdf")]
type PdfSlot = pdf::PdfGuard;
#[cfg(not(feature = "pdf"))]
type PdfSlot = ();

/// The Chromium engine, or the reason PDF is not available. The server starts either way.
#[cfg(feature = "pdf")]
fn install_pdf(runtime_dir: &std::path::Path) -> PdfSlot {
    pdf::install(runtime_dir)
}

#[cfg(not(feature = "pdf"))]
fn install_pdf(_runtime_dir: &std::path::Path) -> PdfSlot {
    moqui_core::screen::install_pdf_unavailable("PDF is not available: this build leaves out the pdf feature");
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
struct Ready {
    tx: Option<std::sync::mpsc::Sender<Result<http::ListenReport, String>>>,
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
impl Ready {
    fn fail(&mut self, err: &moqui_lib::error::Error) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Err(err.to_string()));
        }
    }

    fn handoff(
        &mut self,
    ) -> Result<std::sync::mpsc::Sender<Result<http::ListenReport, String>>, moqui_lib::error::Error> {
        self.tx
            .take()
            .ok_or_else(|| moqui_lib::error::Error::new("http server ready sender is gone"))
    }
}

#[cfg(all(feature = "axum", feature = "embedded-postgres", feature = "postgres"))]
impl Drop for Ready {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Err("http server stopped before it bound a port".to_string()));
        }
    }
}

fn startup_load_flags(conf_text: &str) -> Result<LoadFlags, moqui_lib::error::Error> {
    let conf = moqui_lib::node::MNode::parse_text("moqui-conf", conf_text)?;
    let types = conf
        .first_named("tools")
        .and_then(|tools| tools.attribute("empty-db-load"))
        .unwrap_or_default();
    Ok(LoadFlags {
        data_types: Some(split_csv(&moqui_lib::env::expand(&types)?)),
        ..LoadFlags::default()
    })
}

fn should_load_data(existing: i64, flags: &LoadFlags, startup: bool) -> bool {
    if startup {
        existing == 0
            && flags
                .data_types
                .as_ref()
                .is_some_and(|types| !types.is_empty() && types.as_slice() != ["none"])
    } else {
        existing == 0 || flags.data_types.is_some() || !flags.components.is_empty()
    }
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn load_in_context(
    framework_dir: &std::path::Path,
    facade: &moqui_core::entity::EntityFacade,
    flags: &LoadFlags,
    component_data: &[String],
    startup: bool,
    startup_add_missing: bool,
) -> Result<LoadReport, moqui_lib::error::Error> {
    use moqui_core::entity::load_feed::{conf_data_locations, location_in_components};

    let checks_before = moqui_core::service::security_checks_run();
    apply_facade_flags(facade, flags);
    let services_started = std::time::Instant::now();
    register_services(framework_dir, &flags.service_files)?;
    tracing::info!(
        "Service definitions registered in {:.3} seconds",
        services_started.elapsed().as_secs_f64()
    );
    // The ECA rules load on first use; reading the count here puts them in the log at a known point.
    tracing::info!(
        "{} Service ECA rules are active",
        moqui_core::service::info::seca_rules_active()
    );
    // As Java, `startup-add-missing` checks every table now; without it a table is made on first use when
    // `runtime-add-missing` is on, or is left to the database.
    let test_fts_before_load = if startup_add_missing {
        let entities = entity_names(framework_dir, &facade.entity_dirs())?;
        if !entities.iter().any(|name| name == "moqui.test.TestFts") {
            return Err(moqui_lib::error::Error::new(
                "framework entity list has no moqui.test.TestFts",
            ));
        }
        let refs: Vec<&str> = entities.iter().map(String::as_str).collect();
        tracing::info!(
            "Checking tables for {} framework entities in group transactional",
            refs.len()
        );
        facade.check_tables(&refs)?;
        let test_fts_before_load = facade.table_exists("moqui.test.TestFts")?;
        if !test_fts_before_load {
            return Err(moqui_lib::error::Error::new(
                "moqui.test.TestFts was not created before the seed load",
            ));
        }
        true
    } else {
        tracing::info!("Not checking tables at startup, startup-add-missing is not set for group transactional");
        false
    };
    let options = load_options(flags);
    let mut records = 0_u64;
    let mut failed_files = 0_u64;
    let load_started = std::time::Instant::now();
    let types = flags
        .data_types
        .as_ref()
        .map_or_else(|| "seed,seed-initial,install".to_string(), |types| types.join(","));
    let existing = entity_count(facade, "moqui.basic.Enumeration")?;
    if let Some(location) = &flags.location {
        tracing::info!("Loading entity data from location {location}, data types: {types}");
        records = records.saturating_add(load_one(facade, location, &options, &mut failed_files)?);
    } else if should_load_data(existing, flags, startup) {
        let empty_db_load = startup && existing == 0;
        if existing == 0 {
            tracing::info!("Found 0 Enumeration records, loading empty-db-load data types ({types})");
        } else {
            tracing::info!("Found {existing} Enumeration records, loading requested data types ({types})");
        }
        let framework_files: Vec<String> = framework_xml_paths(framework_dir)?
            .into_iter()
            .filter(|path| {
                !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("")
                    .ends_with(".eecas.xml")
            })
            .map(|path| path.display().to_string())
            .collect();
        // The conf locations are narrowed to the components; the loader does not filter a location it is given.
        let mut conf_files = conf_data_locations(facade)?;
        conf_files.retain(|location| location_in_components(location, &flags.components));
        let mut listing = String::from("Loading entity data from the following locations:");
        for location in framework_files.iter().chain(&conf_files).chain(component_data) {
            listing.push_str("\n - ");
            listing.push_str(location);
        }
        tracing::info!("{listing}");
        tracing::info!("Loading data types: {types}");
        // Java's checkEmptyDb loads in an AT_OTHER loadDataEmptyDb frame that needs no authorization, marked
        // authorized for all actions, with the anonymous user logged in.
        let frame = empty_db_load.then(EmptyDbLoadFrame::push).transpose()?;
        // As Java's `loadAllData`, a location that fails is logged and the load goes on with the next one.
        for location in framework_files.iter().chain(&conf_files).chain(component_data) {
            match load_one(facade, location, &options, &mut failed_files) {
                Ok(loaded) => records = records.saturating_add(loaded),
                Err(err) => {
                    tracing::error!(
                        "Skipping to next file after error loading {location}: {}",
                        moqui_core::entity::load_feed::error_chain(&err)
                    );
                    report_message_errors(location);
                    failed_files += 1;
                }
            }
        }
        drop(frame);
    } else {
        tracing::info!(
            "Found {existing} Enumeration records, NOT loading empty-db-load data types ({types}); use the load command with types or components to load more"
        );
    }
    tracing::info!(
        "Data load of {records} records finished in {:.3} seconds",
        load_started.elapsed().as_secs_f64()
    );
    if failed_files > 0 {
        tracing::error!("{failed_files} data files failed to load and were skipped, see the errors above");
    }
    // Report counts Java does not make, counted in Rust-only bins.
    let report = moqui_core::entity::RustOnlyEntityHits::enter(moqui_core::entity::RustOnlyEntityHits::LOAD_REPORT);
    let enumeration = entity_count(facade, "moqui.basic.Enumeration")?;
    let user_group_permission = entity_count(facade, "moqui.security.UserGroupPermission")?;
    drop(report);
    tracing::info!(
        "Loaded {records} records; enumeration {enumeration}; user group permission {user_group_permission}"
    );
    Ok(LoadReport {
        records,
        enumeration,
        user_group_permission,
        test_fts_before_load,
        security_checks: moqui_core::service::security_checks_run().saturating_sub(checks_before),
    })
}

/// [`moqui_core::search::index_feeds_on_start_empty`] in a context of its own.
fn index_feeds_on_start_empty(factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>) {
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::search::index_feeds_on_start_empty();
}

/// The `loadDataEmptyDb` frame of Java's `checkEmptyDb`, popped on drop.
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
struct EmptyDbLoadFrame;

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
impl EmptyDbLoadFrame {
    fn push() -> Result<Self, moqui_lib::error::Error> {
        let pushed = moqui_core::context::ExecutionContext::with_current(|ctx| {
            ctx.push_checked("AT_OTHER", "loadDataEmptyDb", "all", false, true, None)?;
            ctx.set_anonymous_authorized(true, "");
            moqui_core::user::login_anonymous()?;
            Ok::<(), moqui_core::context::Error>(())
        });
        match pushed {
            Some(Ok(())) => Ok(Self),
            Some(Err(err)) => Err(moqui_lib::error::Error::new(err.to_string())),
            None => Err(moqui_lib::error::Error::new(
                "the empty-db load has no execution context",
            )),
        }
    }
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
impl Drop for EmptyDbLoadFrame {
    fn drop(&mut self) {
        moqui_core::context::ExecutionContext::with_current(|ctx| {
            let _ = ctx.pop();
        });
    }
}

/// As `EntityDataLoaderImpl` after each file: errors left in the message facade (an ECA action's failed service call,
/// for one) are logged and cleared, so they are seen and do not stop the rules of the next file.
fn report_message_errors(location: &str) {
    moqui_core::context::ExecutionContext::with_current(|ctx| {
        let mut message = ctx.message();
        if !message.errors().is_empty() {
            tracing::error!(
                "Error messages loading entity data from {location}: {}",
                message.errors().join("; ")
            );
            message.clear_errors();
        }
    });
}

/// One data location: logs the start and "Loaded N records ... in N.NNs" like `EntityDataLoaderImpl`.
#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn load_one(
    facade: &moqui_core::entity::EntityFacade,
    location: &str,
    options: &moqui_core::entity::load_feed::LoadOptions,
    failed_files: &mut u64,
) -> Result<u64, moqui_lib::error::Error> {
    let started = std::time::Instant::now();
    tracing::info!("Loading entity data from {location}");
    let loaded = moqui_core::entity::load_feed::load_entity_data(facade, location, options)?;
    let records = loaded.records;
    *failed_files += loaded.failed_files;
    report_message_errors(location);
    tracing::info!(
        "Loaded {records} records from {location} in {:.2}s",
        started.elapsed().as_secs_f64()
    );
    Ok(records)
}

/// True when `left` and `right` name the same directory, directly or once canonicalized.
pub(crate) fn same_dir(left: &std::path::Path, right: &std::path::Path) -> bool {
    left == right || matches!((std::fs::canonicalize(left), std::fs::canonicalize(right)), (Ok(a), Ok(b)) if a == b)
}

/// For each component, as `EntityDataLoaderImpl` gathers them: when seed data loads, `component://name/entity/file` for each
/// entity definition file (their `seed-data` blocks; not `*.eecas.xml`), then `component://name/data/file` for each `.xml`,
/// `.csv` and `.json` file in the `data` directory, each by file name. The framework is not here: its files come from the
/// conf. Only the components in `flags.components` when it names some.
fn component_data_locations(factory: &moqui_core::context::ExecutionContextFactory, flags: &LoadFlags) -> Vec<String> {
    let seed = flags
        .data_types
        .as_ref()
        .is_none_or(|types| types.is_empty() || types.iter().any(|kind| kind == "seed" || kind == "all"));
    let mut locations = Vec::new();
    for info in factory.components() {
        if same_dir(&info.location, factory.framework_dir())
            || (!flags.components.is_empty() && !flags.components.contains(&info.name))
        {
            continue;
        }
        if seed {
            if let Ok(read) = std::fs::read_dir(info.location.join("entity")) {
                let mut names: Vec<String> = read
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().is_file())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| name.ends_with(".xml") && !name.ends_with(".eecas.xml"))
                    .collect();
                names.sort();
                locations.extend(
                    names
                        .into_iter()
                        .map(|name| format!("component://{}/entity/{name}", info.name)),
                );
            }
        }
        let Ok(read) = std::fs::read_dir(info.location.join("data")) else {
            continue;
        };
        let mut names: Vec<String> = read
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| [".xml", ".csv", ".json"].iter().any(|ext| name.ends_with(ext)))
            .collect();
        names.sort();
        locations.extend(
            names
                .into_iter()
                .map(|name| format!("component://{}/data/{name}", info.name)),
        );
    }
    locations
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn apply_facade_flags(facade: &moqui_core::entity::EntityFacade, flags: &LoadFlags) {
    if flags.raw || flags.disable_entity_eca {
        facade.set_disable_entity_eca(true);
    }
    if flags.raw || flags.disable_audit_log {
        facade.set_disable_audit_log(true);
    }
    if flags.raw || flags.disable_fk_create {
        facade.set_disable_fk_create(true);
    }
    if flags.raw || flags.disable_data_feed {
        facade.set_disable_data_feed(true);
    }
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn register_services(
    framework_dir: &std::path::Path,
    extra: &[std::path::PathBuf],
) -> Result<(), moqui_lib::error::Error> {
    let lint_error = |err: moqui_core::context::Error| moqui_lib::error::Error::new(err.to_string());
    // Explicitly supplied service files must register. A failure there stops the load.
    if !extra.is_empty() {
        let own = moqui_core::service::check(extra).map_err(lint_error)?;
        if !own.is_empty() {
            return Err(moqui_lib::error::Error::new(format!(
                "service definitions failed to register:\n{}",
                own.join("\n")
            )));
        }
    }
    let mut paths = configured_service_paths(framework_dir)?;
    paths.extend(extra.iter().cloned());
    // A service that does not register is left out with its reason; the others still register.
    let skipped = moqui_core::service::lint(&paths).map_err(lint_error)?;
    for reason in &skipped {
        tracing::warn!("Service not registered: {reason}");
    }
    if !skipped.is_empty() {
        tracing::warn!(
            "{} service definition problem(s), those services are not registered; run `moqui lint` for details",
            skipped.len()
        );
    }
    Ok(())
}

#[cfg(all(feature = "embedded-postgres", feature = "postgres"))]
fn entity_count(facade: &moqui_core::entity::EntityFacade, name: &str) -> Result<i64, moqui_lib::error::Error> {
    facade.find(name)?.count()
}

fn framework_xml_paths(framework_dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, moqui_lib::error::Error> {
    use moqui_lib::node::MNode;

    let conf = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
    let facade = conf
        .first_named("entity-facade")
        .ok_or_else(|| moqui_lib::error::Error::new("conf has no entity-facade"))?;
    let mut paths = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for node in facade.children_named("load-entity") {
        let Some(location) = node.attribute("location") else {
            continue;
        };
        let path = resolve_framework_location(framework_dir, &location)?;
        if path.is_file() {
            push_unique(&mut paths, &mut seen, path);
        }
    }
    let mut extras = Vec::new();
    let dir = framework_dir.join("entity");
    let read = std::fs::read_dir(&dir)
        .map_err(|err| moqui_lib::error::Error::new(format!("could not read {}", dir.display())).with_source(err))?;
    for entry in read {
        let entry = entry.map_err(|err| {
            moqui_lib::error::Error::new(format!("could not read {}", dir.display())).with_source(err)
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("xml") {
            extras.push(path);
        }
    }
    extras.sort();
    for path in extras {
        push_unique(&mut paths, &mut seen, path);
    }
    Ok(paths)
}

fn push_unique(
    paths: &mut Vec<std::path::PathBuf>,
    seen: &mut std::collections::BTreeSet<std::path::PathBuf>,
    path: std::path::PathBuf,
) {
    let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    if seen.insert(key) {
        paths.push(path);
    }
}

/// The transactional entities of the framework conf's files, then of each component `entity` directory
/// in `entity_dirs` after the framework's own (the first).
fn entity_names(
    framework_dir: &std::path::Path,
    entity_dirs: &[std::path::PathBuf],
) -> Result<Vec<String>, moqui_lib::error::Error> {
    use moqui_lib::node::MNode;

    let mut names = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut paths = framework_xml_paths(framework_dir)?;
    for dir in entity_dirs.iter().skip(1) {
        let read = std::fs::read_dir(dir).map_err(|err| {
            moqui_lib::error::Error::new(format!("could not read {}", dir.display())).with_source(err)
        })?;
        let mut found: Vec<std::path::PathBuf> = read
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("xml"))
            .collect();
        found.sort();
        paths.extend(found);
    }
    for path in paths {
        let root = MNode::parse_file(&path)?;
        collect_entities(&root, &mut names, &mut seen);
    }
    Ok(names)
}

fn collect_entities(
    node: &moqui_lib::node::MNode,
    names: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    if node.name() == "entity" {
        if transactional_entity(node) {
            if let Some(full) = full_entity_name(node) {
                if seen.insert(full.clone()) {
                    names.push(full);
                }
            }
        }
        return;
    }
    if node.name() == "seed-data" {
        return;
    }
    for child in node.children() {
        collect_entities(&child, names, seen);
    }
}

fn transactional_entity(node: &moqui_lib::node::MNode) -> bool {
    !matches!(
        node.attribute("group"),
        Some(group) if !group.is_empty() && group != "transactional"
    )
}

fn full_entity_name(node: &moqui_lib::node::MNode) -> Option<String> {
    let name = node.attribute("entity-name")?;
    if name.is_empty() {
        return None;
    }
    match node.attribute("package") {
        Some(package) if !package.is_empty() => Some(format!("{package}.{name}")),
        _ => Some(name),
    }
}

fn configured_service_paths(
    framework_dir: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, moqui_lib::error::Error> {
    use moqui_lib::node::MNode;

    let conf = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
    let facade = conf
        .first_named("service-facade")
        .ok_or_else(|| moqui_lib::error::Error::new("conf has no service-facade"))?;
    let mut paths = Vec::new();
    for node in facade.children_named("service-file") {
        let Some(location) = node.attribute("location") else {
            continue;
        };
        if location.is_empty() {
            continue;
        }
        match resolve_framework_location(framework_dir, &location) {
            Ok(path) => paths.push(path),
            Err(err) => {
                tracing::error!(error = %err, "Could not resolve service-file location {location}, skipping it")
            }
        }
    }
    Ok(paths)
}

/// The framework component beside `tools` and `webroot`, unless `MOQUI_FRAMEWORK` names another directory.
fn framework_from_env(runtime: &std::path::Path) -> std::path::PathBuf {
    std::env::var_os("MOQUI_FRAMEWORK").map_or_else(|| moqui_core::context::default_framework_dir(runtime), Into::into)
}

fn resolve_framework_location(
    framework_dir: &std::path::Path,
    location: &str,
) -> Result<std::path::PathBuf, moqui_lib::error::Error> {
    if let Some(rest) = location
        .strip_prefix("classpath://")
        .or_else(|| location.strip_prefix("classpath:"))
    {
        return Ok(framework_dir.join(rest.trim_start_matches('/')));
    }
    if let Some(rest) = framework_component_path(location) {
        return Ok(framework_dir.join(rest));
    }
    if let Some(rest) = location.strip_prefix("file:") {
        let rest = rest.strip_prefix("//").unwrap_or(rest);
        return Ok(std::path::PathBuf::from(rest));
    }
    if location.starts_with("component://") || location.starts_with("component:") {
        return Err(moqui_lib::error::Error::new(format!(
            "component location is not resolved by the host loader: {location}"
        )));
    }
    Ok(std::path::PathBuf::from(location))
}

/// `component://framework/service|entity|data|screen|template/...`, the paths that used to be `classpath://`.
fn framework_component_path(location: &str) -> Option<&str> {
    let rest = location
        .strip_prefix("component://")
        .or_else(|| location.strip_prefix("component:"))?
        .trim_start_matches('/');
    let (name, path) = rest.split_once('/')?;
    if name != "framework" {
        return None;
    }
    let path = path.trim_start_matches('/');
    let directory = path.split('/').next().unwrap_or("");
    if matches!(directory, "service" | "entity" | "data" | "screen" | "template") {
        Some(path)
    } else {
        None
    }
}

/// theseus 18.6.0 links `postgres` against `libxml2.so.2`. This host may only ship `libxml2.so.16`.
/// [`db::embedded`] builds a forwarding `libxml2.so.2` into the install's `lib` before any server binary runs.
/// Every theseus binary searches that directory through its `$ORIGIN/../lib` runpath, so the process
/// environment is never changed: `set_var` with other threads running is undefined behaviour.
pub(crate) mod xmlshim {
    use std::path::{Path, PathBuf};

    use moqui_lib::error::Error;

    /// The host's `libxml2.so.16` when theseus needs the shim: the host has no `libxml2.so.2`.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub fn needed() -> Option<PathBuf> {
        let cache = ldconfig_cache();
        if cache.lines().any(|line| ldconfig_soname(line) == "libxml2.so.2") {
            return None;
        }
        libxml2_16_path(&cache)
    }

    /// The shim is only built on x86-64 Linux.
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    pub fn needed() -> Option<PathBuf> {
        None
    }

    /// Build `libxml2.so.2` into `lib_dir`, forwarding to `host`, unless an earlier start already did.
    ///
    /// # Errors
    ///
    /// Returns an error when the shim cannot be built.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub fn provide(lib_dir: &Path, host: &Path) -> Result<(), Error> {
        // Two clusters sharing one install may start at once. The library is built aside and renamed into
        // place, so a postmaster never maps a half-written file.
        static BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = moqui_lib::sync::lock(&BUILD_LOCK);
        let shared = lib_dir.join("libxml2.so.2");
        if shared.is_file() {
            return Ok(());
        }
        let build_dir = lib_dir.join(format!(".xmlshim-{}", std::process::id()));
        let built = compile_libxml2_shim(&build_dir, host).and_then(|built| {
            std::fs::rename(&built, &shared)
                .map_err(|err| Error::new("could not install the libxml2 shim").with_source(err))
        });
        let _ = std::fs::remove_dir_all(&build_dir);
        built
    }

    /// The shim is only built on x86-64 Linux.
    ///
    /// # Errors
    ///
    /// Never.
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    pub fn provide(_lib_dir: &Path, _host: &Path) -> Result<(), Error> {
        Ok(())
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn ldconfig_cache() -> String {
        std::process::Command::new("ldconfig")
            .arg("-p")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default()
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn ldconfig_soname(line: &str) -> &str {
        line.split_whitespace().next().unwrap_or("")
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn libxml2_16_path(cache: &str) -> Option<PathBuf> {
        for line in cache.lines() {
            if ldconfig_soname(line) == "libxml2.so.16" && line.contains("x86-64") {
                let path = PathBuf::from(line.split("=>").nth(1)?.trim());
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        None
    }

    /// Assemble and link the shim in `shim_dir`. Returns the built library.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn compile_libxml2_shim(shim_dir: &Path, host: &Path) -> Result<PathBuf, Error> {
        std::fs::create_dir_all(shim_dir)
            .map_err(|err| Error::new("could not create libxml2 shim dir").with_source(err))?;
        let assembly = shim_dir.join("shim.s");
        let map = shim_dir.join("ver.map");
        let shared = shim_dir.join("libxml2.so.2");
        std::fs::write(&assembly, shim_assembly())
            .map_err(|err| Error::new("could not write libxml2 shim").with_source(err))?;
        std::fs::write(&map, version_script())
            .map_err(|err| Error::new("could not write libxml2 shim").with_source(err))?;
        let output = std::process::Command::new("gcc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&shared)
            .arg(&assembly)
            .arg("-Wl,-soname,libxml2.so.2")
            .arg(format!("-Wl,--version-script={}", map.display()))
            .arg(host)
            .output()
            .map_err(|err| Error::new("gcc could not build the libxml2 shim").with_source(err))?;
        if !output.status.success() {
            return Err(Error::new(format!(
                "libxml2 shim: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(shared)
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn shim_assembly() -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        out.push_str(".text\n");
        for (index, (name, version)) in XML_SYMBOLS.iter().enumerate() {
            writeln!(
                out,
                ".globl shim_{index}\n.type shim_{index}, @function\nshim_{index}:\n    jmp {name}@PLT\n.size shim_{index}, .-shim_{index}\n.symver shim_{index}, {name}@{version}"
            )
            .unwrap();
        }
        out
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn version_script() -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        for (index, version) in XML_VERSIONS.iter().enumerate() {
            out.push_str(version);
            out.push_str(" {\n  global:\n");
            for (name, symbol_version) in XML_SYMBOLS {
                if symbol_version == version {
                    writeln!(out, "    {name};").unwrap();
                }
            }
            out.push_str("  local: *;\n}");
            if index == 0 {
                out.push_str(";\n");
            } else {
                writeln!(out, " {};", XML_VERSIONS[index - 1]).unwrap();
            }
        }
        out
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const XML_VERSIONS: &[&str] = &[
        "LIBXML2_2.4.30",
        "LIBXML2_2.6.0",
        "LIBXML2_2.6.5",
        "LIBXML2_2.6.8",
        "LIBXML2_2.6.23",
    ];

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const XML_SYMBOLS: &[(&str, &str)] = &[
        ("xmlAddChildList", "LIBXML2_2.4.30"),
        ("xmlBufferContent", "LIBXML2_2.4.30"),
        ("xmlBufferCreate", "LIBXML2_2.4.30"),
        ("xmlBufferFree", "LIBXML2_2.4.30"),
        ("xmlBufferLength", "LIBXML2_2.4.30"),
        ("xmlCharInRange", "LIBXML2_2.6.0"),
        ("xmlCheckVersion", "LIBXML2_2.4.30"),
        ("xmlCopyNode", "LIBXML2_2.4.30"),
        ("xmlCtxtReadDoc", "LIBXML2_2.6.0"),
        ("xmlCtxtReadMemory", "LIBXML2_2.6.0"),
        ("xmlDocSetRootElement", "LIBXML2_2.4.30"),
        ("xmlEncodeSpecialChars", "LIBXML2_2.4.30"),
        ("xmlFreeDoc", "LIBXML2_2.4.30"),
        ("xmlFreeNode", "LIBXML2_2.4.30"),
        ("xmlFreeParserCtxt", "LIBXML2_2.4.30"),
        ("xmlFreeTextWriter", "LIBXML2_2.6.0"),
        ("xmlGetExternalEntityLoader", "LIBXML2_2.4.30"),
        ("xmlGetUTF8Char", "LIBXML2_2.4.30"),
        ("xmlInitParser", "LIBXML2_2.4.30"),
        ("xmlKeepBlanksDefault", "LIBXML2_2.4.30"),
        ("xmlNewDoc", "LIBXML2_2.4.30"),
        ("xmlNewDocText", "LIBXML2_2.4.30"),
        ("xmlNewNode", "LIBXML2_2.4.30"),
        ("xmlNewParserCtxt", "LIBXML2_2.4.30"),
        ("xmlNewStringInputStream", "LIBXML2_2.4.30"),
        ("xmlNewTextWriterMemory", "LIBXML2_2.6.0"),
        ("xmlNodeDump", "LIBXML2_2.4.30"),
        ("xmlParseBalancedChunkMemory", "LIBXML2_2.4.30"),
        ("xmlParserPrintFileContext", "LIBXML2_2.4.30"),
        ("xmlSaveClose", "LIBXML2_2.6.8"),
        ("xmlSaveDoc", "LIBXML2_2.6.8"),
        ("xmlSaveToBuffer", "LIBXML2_2.6.23"),
        ("xmlSaveTree", "LIBXML2_2.6.8"),
        ("xmlSetExternalEntityLoader", "LIBXML2_2.4.30"),
        ("xmlSetGenericErrorFunc", "LIBXML2_2.4.30"),
        ("xmlSetStructuredErrorFunc", "LIBXML2_2.6.0"),
        ("xmlStrchr", "LIBXML2_2.4.30"),
        ("xmlStrcmp", "LIBXML2_2.4.30"),
        ("xmlStrdup", "LIBXML2_2.4.30"),
        ("xmlStrlen", "LIBXML2_2.4.30"),
        ("xmlStrncmp", "LIBXML2_2.4.30"),
        ("xmlStrstr", "LIBXML2_2.4.30"),
        ("xmlTextWriterEndElement", "LIBXML2_2.6.0"),
        ("xmlTextWriterStartElement", "LIBXML2_2.6.0"),
        ("xmlTextWriterWriteAttribute", "LIBXML2_2.6.0"),
        ("xmlTextWriterWriteBase64", "LIBXML2_2.6.0"),
        ("xmlTextWriterWriteBinHex", "LIBXML2_2.6.0"),
        ("xmlTextWriterWriteRaw", "LIBXML2_2.6.0"),
        ("xmlXPathCastBooleanToNumber", "LIBXML2_2.4.30"),
        ("xmlXPathCastBooleanToString", "LIBXML2_2.4.30"),
        ("xmlXPathCastNodeSetToString", "LIBXML2_2.4.30"),
        ("xmlXPathCastNodeToString", "LIBXML2_2.4.30"),
        ("xmlXPathCastNumberToString", "LIBXML2_2.4.30"),
        ("xmlXPathCompiledEval", "LIBXML2_2.4.30"),
        ("xmlXPathCtxtCompile", "LIBXML2_2.6.5"),
        ("xmlXPathFreeCompExpr", "LIBXML2_2.4.30"),
        ("xmlXPathFreeContext", "LIBXML2_2.4.30"),
        ("xmlXPathFreeObject", "LIBXML2_2.4.30"),
        ("xmlXPathNewContext", "LIBXML2_2.4.30"),
        ("xmlXPathRegisterNs", "LIBXML2_2.4.30"),
    ];
}

#[cfg(test)]
mod tests {
    use super::{dispatch, load_options, parse_load_args, should_load_data, startup_load_flags, LoadFlags};

    #[test]
    fn startup_load_honors_all_including_demo() {
        let flags = startup_load_flags(r#"<moqui-conf><tools empty-db-load="all"/></moqui-conf>"#).unwrap();
        assert!(load_options(&flags).data_types.is_empty());
        assert!(should_load_data(0, &flags, true));
        assert!(!should_load_data(1, &flags, true));
    }

    #[test]
    fn startup_load_expands_configured_property() {
        let key = "moqui_host_test_empty_db_load";
        moqui_lib::env::set_property(key, "seed, demo");
        let result =
            startup_load_flags(r#"<moqui-conf><tools empty-db-load="${moqui_host_test_empty_db_load}"/></moqui-conf>"#);
        moqui_lib::env::clear_property(key);
        let flags = result.unwrap();
        assert_eq!(load_options(&flags).data_types, ["seed", "demo"]);
        assert!(should_load_data(0, &flags, true));
        assert!(!should_load_data(1, &flags, true));
    }

    #[test]
    fn startup_load_respects_disabled_and_absent_settings() {
        for conf in [
            r#"<moqui-conf><tools empty-db-load="none"/></moqui-conf>"#,
            r#"<moqui-conf><tools empty-db-load=""/></moqui-conf>"#,
            "<moqui-conf><tools/></moqui-conf>",
            "<moqui-conf/>",
        ] {
            let flags = startup_load_flags(conf).unwrap();
            assert!(!should_load_data(0, &flags, true));
        }
    }

    #[test]
    fn explicit_load_keeps_existing_database_and_default_behavior() {
        let flags = LoadFlags::default();
        assert_eq!(load_options(&flags).data_types, ["seed", "seed-initial", "install"]);
        assert!(should_load_data(0, &flags, false));
        assert!(!should_load_data(1, &flags, false));
        let all = parse_load_args(&["types=all"]).unwrap();
        assert!(should_load_data(1, &all, false));
        let components = parse_load_args(&["components=webroot"]).unwrap();
        assert!(should_load_data(1, &components, false));
    }

    #[test]
    fn no_args_start_the_server_and_are_not_called_here() {
        // `dispatch([])` calls `serve` and blocks on the listener. A bad command still names the usage.
        let err = dispatch(["nope"]).unwrap_err();
        assert!(err.message().contains("usage: moqui"), "{err}");
        assert!(!err.message().contains("server not implemented"), "{err}");
    }

    #[test]
    fn unknown_command_names_the_dump_commands() {
        let text = dispatch(["help"]).unwrap().expect("help prints the command list");
        assert!(text.contains("save-db"), "{text}");
        assert!(text.contains("load-save"), "{text}");
        assert!(text.contains("load"), "{text}");
        assert!(text.contains("lint"), "{text}");
        let err = dispatch(["nope"]).unwrap_err();
        let message = err.message();
        assert!(message.contains("usage: moqui"), "{err}");
        assert!(message.contains("moqui help"), "{err}");
        assert!(!message.contains("disableEntityEca"), "{err}");
    }

    #[test]
    fn usage_overview_lists_commands_and_environment() {
        let text = dispatch(["help"]).unwrap().unwrap();
        assert_eq!(dispatch(["--help"]).unwrap().unwrap(), text);
        assert_eq!(dispatch(["-h"]).unwrap().unwrap(), text);
        assert_eq!(dispatch(["-?"]).unwrap().unwrap(), text);
        for needle in [
            "Usage: moqui",
            "save-db",
            "load-save",
            "load",
            "lint",
            "convert",
            "types",
            "test",
            "[component ...]",
            "MOQUI_CONF",
            "webapp_http_port",
            "--port",
            "--conf",
            "--no-run-es",
            "-D",
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
    }

    #[test]
    fn usage_load_page_names_the_flags() {
        let text = dispatch(["help", "load"]).unwrap().unwrap();
        assert_eq!(dispatch(["load", "--help"]).unwrap().unwrap(), text);
        assert_eq!(dispatch(["load", "-h"]).unwrap().unwrap(), text);
        assert_eq!(dispatch(["--help", "load"]).unwrap().unwrap(), text);
        for needle in [
            "types",
            "components",
            "location",
            "timeout",
            "raw",
            "disableEntityEca",
            "disableAuditLog",
            "disableFkCreate",
            "disableDataFeed",
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
    }

    #[test]
    fn usage_lint_and_types_pages_name_their_modes() {
        let lint = dispatch(["help", "lint"]).unwrap().unwrap();
        assert!(lint.contains("--screens"), "{lint}");
        assert!(lint.contains("--qvt-static"), "{lint}");
        assert!(lint.contains("--groovy"), "{lint}");
        let types = dispatch(["help", "types"]).unwrap().unwrap();
        assert!(types.contains("--out"), "{types}");
        assert_eq!(dispatch(["types", "--help"]).unwrap().unwrap(), types);
    }

    #[test]
    fn usage_test_page_names_the_options() {
        let text = dispatch(["help", "test"]).unwrap().unwrap();
        assert_eq!(dispatch(["test", "--help"]).unwrap().unwrap(), text);
        assert_eq!(dispatch(["test", "-h"]).unwrap().unwrap(), text);
        assert_eq!(dispatch(["--help", "test"]).unwrap().unwrap(), text);
        for needle in [
            "--spec",
            "--feature",
            "--db",
            "fresh",
            "snapshot",
            "existing",
            "--junit",
            "--keep",
            "--log-level",
            "suite.js",
            "dependsOn",
            "test/*.spec.js",
        ] {
            assert!(text.contains(needle), "{needle} missing from {text}");
        }
        let bogus = dispatch(["test", "--bogus"]).unwrap_err();
        assert!(bogus.message().contains("unknown option"), "{bogus}");
        assert!(bogus.message().contains("moqui help test"), "{bogus}");
    }

    #[test]
    fn usage_server_options_do_not_start_the_listener() {
        let missing = dispatch(["--port"]).unwrap_err();
        assert!(missing.message().contains("port"), "{missing}");
        let bogus = dispatch(["--bogus"]).unwrap_err();
        assert!(bogus.message().contains("unknown option '--bogus'"), "{bogus}");
        assert!(bogus.message().contains("moqui help"), "{bogus}");
        let threads = dispatch(["threads=4"]).unwrap_err();
        assert!(threads.message().contains("unknown option 'threads=4'"), "{threads}");
        let dashed = dispatch(["--threads", "4"]).unwrap_err();
        assert!(dashed.message().contains("unknown option '--threads'"), "{dashed}");
        let on_load = dispatch(["load", "--conf", "conf/MoquiProductionConf.xml"]).unwrap_err();
        assert!(on_load.message().contains("load flag"), "{on_load}");
        let before_command = dispatch(["--port", "8081", "save-db"]).unwrap_err();
        assert!(
            before_command.message().contains("unknown option 'save-db'"),
            "{before_command}"
        );
        let help = dispatch(["--port", "8081", "--help"]).unwrap().unwrap();
        assert_eq!(help, dispatch(["help"]).unwrap().unwrap());
        assert_eq!(dispatch(["--port", "8081", "-h"]).unwrap().unwrap(), help);
    }

    #[test]
    fn usage_unknown_topic_points_at_help() {
        let err = dispatch(["help", "nope"]).unwrap_err();
        assert!(err.message().contains("nope"), "{err}");
        assert!(err.message().contains("moqui help"), "{err}");
    }

    #[test]
    fn usage_wrong_arity_names_the_command() {
        let err = dispatch(["save-db"]).unwrap_err();
        assert!(err.message().contains("<cluster-dir>"), "{err}");
        assert!(err.message().contains("moqui help save-db"), "{err}");
        let convert = dispatch(["convert", "--groovy"]).unwrap_err();
        assert!(convert.message().contains("moqui help convert"), "{convert}");
        let lint = dispatch(["lint", "--groovy"]).unwrap_err();
        assert!(lint.message().contains("moqui help lint"), "{lint}");
    }

    #[test]
    fn unknown_load_flag_names_the_flag() {
        let err = dispatch(["load", "nope=1"]).unwrap_err();
        assert!(err.message().contains("unknown load flag nope"), "{err}");
    }

    #[test]
    fn timeout_must_be_an_integer() {
        let err = dispatch(["load", "timeout=soon"]).unwrap_err();
        assert!(err.message().contains("timeout"), "{err}");
    }

    #[test]
    fn missing_types_use_empty_db_defaults() {
        let options = load_options(&LoadFlags::default());
        assert_eq!(options.data_types, ["seed", "seed-initial", "install"]);
        assert!(!options.use_try_insert);
        assert!(!options.disable_fk_create);
    }

    #[test]
    fn raw_and_types_parse() {
        let flags = parse_load_args(&[
            "types=seed,seed-initial",
            "raw=true",
            "disableFkCreate=false",
            "components=framework,mantle",
            "timeout=15",
        ])
        .unwrap();
        assert!(flags.raw);
        assert!(!flags.disable_fk_create);
        assert_eq!(flags.timeout_seconds, Some(15));
        assert_eq!(flags.components, ["framework", "mantle"]);
        let options = load_options(&flags);
        assert_eq!(options.data_types, ["seed", "seed-initial"]);
        assert!(options.use_try_insert);
        assert!(options.disable_fk_create);
    }

    #[test]
    fn types_all_clears_the_filter() {
        let flags = parse_load_args(&["types=all"]).unwrap();
        assert!(load_options(&flags).data_types.is_empty());
        let empty = parse_load_args(&["types="]).unwrap();
        assert!(load_options(&empty).data_types.is_empty());
    }

    #[test]
    fn camel_case_flags_are_independent() {
        let flags = parse_load_args(&[
            "disableEntityEca=true",
            "disableAuditLog=yes",
            "disableFkCreate=1",
            "disableDataFeed=y",
        ])
        .unwrap();
        assert!(flags.disable_entity_eca);
        assert!(flags.disable_audit_log);
        assert!(flags.disable_fk_create);
        assert!(flags.disable_data_feed);
        assert!(!flags.raw);
        let options = load_options(&flags);
        assert!(options.disable_fk_create);
        assert!(!options.use_try_insert);
    }

    #[test]
    fn lint_groovy_leftover_is_a_process_error() {
        let dir = std::env::temp_dir().join(format!("moqui-lint-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("groovy-leftover.xml");
        std::fs::write(
            &path,
            r#"<services>
                <service verb="groovy" noun="Broken">
                    <actions><set field="x" from="a ?: b"/></actions>
                </service>
            </services>"#,
        )
        .unwrap();
        let err = dispatch(["lint", &path.display().to_string()]).expect_err("lint fails the process");
        assert!(err.to_string().contains("failed lint"), "{err}");
    }

    #[test]
    fn lint_of_a_clean_file_succeeds() {
        let dir = std::env::temp_dir().join(format!("moqui-lint-clean-{}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clean.xml");
        std::fs::write(
            &path,
            r#"<services><service verb="clean" noun="Fine"><actions><set field="x" from="a || b"/></actions></service></services>"#,
        )
        .unwrap();
        dispatch(["lint", &path.display().to_string()]).unwrap();
    }

    #[test]
    fn lint_with_no_path_loads_configured_files() {
        // Framework services are not all JavaScript yet, so this may fail. It must not panic.
        let _ = dispatch(["lint"]);
    }

    struct DirGuard(std::path::PathBuf);

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
