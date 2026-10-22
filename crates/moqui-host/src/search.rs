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

//! Where the search store lives, and the backend the factory gets.
//!
//! The Postgres backend uses, in order: the `postgres` url from `search-facade`; the entity
//! Postgres (its `search` schema), when the entity datasource is Postgres; or a dedicated embedded
//! cluster under `runtime/db/search` when the entity datasource is not Postgres. The OpenSearch
//! backend talks to the conf `cluster` over HTTP. When that cluster is local and not running, and
//! `runtime/opensearch` holds an install, the store starts it, as `MoquiStart` does, and stops it
//! when dropped.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::entity::EntityFacade;
use moqui_core::search::conf::{Backend, SearchConf};
use moqui_core::search::postgres::PostgresBackend;
use moqui_core::search::SearchFacade;
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_lib::error::Error;

/// Keeps a dedicated search postmaster, or a local OpenSearch this process started, alive. Drop it
/// after the factory.
#[derive(Default)]
pub struct SearchStore {
    #[cfg(feature = "embedded-postgres")]
    cluster: Option<crate::db::embedded::EmbeddedCluster>,
    opensearch: Option<OpenSearchProcess>,
}

impl SearchStore {
    /// True when this store started the OpenSearch in `runtime/opensearch`.
    pub fn started_opensearch(&self) -> bool {
        self.opensearch.is_some()
    }

    /// `pg_dump` / `pg_basebackup` coordinates when this store runs its own postmaster.
    #[cfg(feature = "embedded-postgres")]
    #[must_use]
    pub fn tool_target(&self) -> Option<crate::db::embedded::ToolTarget> {
        self.cluster.as_ref().map(|cluster| cluster.tool_connect("search"))
    }

    /// True when this store started its own embedded postmaster.
    pub fn has_own_cluster(&self) -> bool {
        #[cfg(feature = "embedded-postgres")]
        {
            self.cluster.is_some()
        }
        #[cfg(not(feature = "embedded-postgres"))]
        {
            false
        }
    }
}

fn search_pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 8,
        reserve: 1,
        acquire_timeout: std::time::Duration::from_secs(30),
        ..Default::default()
    }
}

/// Build the configured backend and install the facade on `factory`.
///
/// `entity_is_postgres` says whether the entity datasource is Postgres, so its `search` schema can
/// hold the documents.
///
/// # Errors
///
/// Returns an error when the conf is bad, the store cannot be reached, or the backend is not in
/// this build.
pub fn attach_search(
    factory: &ExecutionContextFactory,
    entity: &EntityFacade,
    entity_is_postgres: bool,
) -> Result<SearchStore, Error> {
    let conf = SearchConf::from_conf_text(factory.conf_text())?;
    match conf.backend {
        Backend::OpenSearch => attach_opensearch(factory, entity, &conf),
        Backend::Postgres => {
            let mut store = SearchStore::default();
            let tx = if let Some(url) = &conf.postgres.url {
                external_tx(url, conf.postgres.user.as_deref(), conf.postgres.password.as_deref())?
            } else if entity_is_postgres {
                entity.transactions_for("moqui.entity.document.DataDocument")?
            } else {
                embedded_tx(factory.runtime_dir(), &mut store)?
            };
            let mut backend = PostgresBackend::open(tx, conf.postgres.schema.as_deref())?;
            if conf.postgres.text_config.is_some() || conf.postgres.unaccent {
                backend = backend.with_analysis(conf.postgres.text_config.as_deref(), conf.postgres.unaccent)?;
            }
            if let Some(read_url) = &conf.postgres.read_url {
                backend = backend.with_read_replica(read_tx(read_url, &conf.postgres)?);
            }
            backend = backend.with_semantic(&conf.semantic)?;
            let facade = SearchFacade::new(Arc::new(backend), entity.clone());
            if conf.semantic.enabled {
                facade.set_embedder(embedder_of(factory, &conf.semantic));
            }
            moqui_core::search::install(factory, facade);
            Ok(store)
        }
    }
}

/// The embedder `semantic embedder` names: `hash` (development only), or `llm` / `llm:<modelId>`, the LLM embedder
/// over the model the `llm-facade` binds to `embed` (or names), which calls that model's `/v1/embeddings`. Another
/// name, or `llm` in a build without the `llm` feature, is logged and installs none.
fn embedder_of(
    factory: &ExecutionContextFactory,
    semantic: &moqui_core::search::semantic::SemanticConf,
) -> Option<moqui_core::search::semantic::SharedEmbedder> {
    let name = semantic.embedder.as_deref()?;
    if name == "hash" {
        return Some(Arc::new(moqui_core::search::semantic::HashEmbedder::new(semantic.dimensions)));
    }
    #[cfg(feature = "llm")]
    if let Some(with) = moqui_core::llm::embed::EmbedWith::parse(name) {
        let embedder = moqui_core::llm::embed::LlmEmbedder::for_factory(factory, with, Some(semantic.dimensions));
        if embedder.is_none() {
            tracing::error!("search-facade semantic embedder {name}: the llm-facade is off, so no embedder is installed");
        }
        return embedder.map(|embedder| Arc::new(embedder) as moqui_core::search::semantic::SharedEmbedder);
    }
    #[cfg(not(feature = "llm"))]
    let _ = factory;
    tracing::error!("search-facade semantic embedder {name} is not hash, llm or llm:<modelId> (in a build with the llm feature); no embedder is installed");
    None
}

/// The TLS of a search url: none for `sslmode=disable`, else what its `sslmode` and `sslrootcert` say.
#[cfg(feature = "postgres")]
fn url_tls(url: &moqui_core::search::conf::PostgresUrl) -> Option<crate::db::pg_tls::TlsOptions> {
    url.tls
        .then(|| crate::db::pg_tls::TlsOptions::new(url.tls_verify, &url.tls_root_cert))
}

#[cfg(feature = "postgres")]
fn external_tx(url: &str, user: Option<&str>, password: Option<&str>) -> Result<TransactionManager, Error> {
    let url = moqui_core::search::conf::parse_postgres_url(url)?;
    let driver = crate::db::entity_db::postgres_server_driver(
        user.unwrap_or("postgres"),
        password,
        &url.database,
        &url.host,
        url.port,
        url_tls(&url).as_ref(),
    )?;
    TransactionManager::new(Arc::new(driver), search_pool())
}

/// The pool for a `read-url` replica: its own connections, read-only sessions, and a statement
/// timeout so a long search is cancelled instead of holding back replay. No reserve: it takes no
/// writes.
#[cfg(feature = "postgres")]
fn read_tx(url: &str, conf: &moqui_core::search::conf::PostgresConf) -> Result<TransactionManager, Error> {
    let parsed = moqui_core::search::conf::parse_postgres_url(url)?;
    let timeout_ms = conf.read_statement_timeout.unwrap_or(30).saturating_mul(1000);
    let driver = crate::db::entity_db::postgres_server_driver(
        conf.user.as_deref().unwrap_or("postgres"),
        conf.password.as_deref(),
        &parsed.database,
        &parsed.host,
        parsed.port,
        url_tls(&parsed).as_ref(),
    )?
    .options(format!(
        "-c default_transaction_read_only=on -c statement_timeout={timeout_ms}"
    ));
    let pool = PoolConfig {
        reserve: 0,
        idle_timeout: Some(Duration::from_secs(60)),
        ..search_pool()
    };
    TransactionManager::new(Arc::new(driver), pool)
}

#[cfg(not(feature = "postgres"))]
fn read_tx(_url: &str, _conf: &moqui_core::search::conf::PostgresConf) -> Result<TransactionManager, Error> {
    Err(Error::new("the postgres driver is not in this build"))
}

#[cfg(not(feature = "postgres"))]
fn external_tx(_url: &str, _user: Option<&str>, _password: Option<&str>) -> Result<TransactionManager, Error> {
    Err(Error::new("the postgres driver is not in this build"))
}

#[cfg(feature = "embedded-postgres")]
fn embedded_tx(runtime_dir: &Path, store: &mut SearchStore) -> Result<TransactionManager, Error> {
    use crate::db::embedded::{shared_install_dir, ClusterRole, EmbeddedCluster, StartOptions};

    let dir = runtime_dir.join("db/search");
    let options = StartOptions {
        max_connections: search_pool().max + crate::db::entity_db::SUPERUSER_SLOTS,
        role: ClusterRole::Entity,
        install_dir: Some(shared_install_dir(runtime_dir, &dir)),
    };
    let (cluster, driver) = EmbeddedCluster::start_with(&dir, &options)?;
    store.cluster = Some(cluster);
    TransactionManager::new(Arc::new(driver), search_pool())
}

#[cfg(not(feature = "embedded-postgres"))]
fn embedded_tx(_runtime_dir: &Path, _store: &mut SearchStore) -> Result<TransactionManager, Error> {
    Err(Error::new(
        "embedded postgres is not in this build; configure a search-facade postgres url or the opensearch backend",
    ))
}

fn attach_opensearch(
    factory: &ExecutionContextFactory,
    entity: &EntityFacade,
    conf: &SearchConf,
) -> Result<SearchStore, Error> {
    let cluster = conf
        .cluster
        .as_ref()
        .ok_or_else(|| Error::new("search-facade backend opensearch needs a cluster url"))?;
    let backend = moqui_core::search::opensearch::OpenSearchBackend::new(cluster);
    let mut store = SearchStore::default();
    if !backend.reachable() {
        let dir = factory.runtime_dir().join("opensearch");
        if run_local_opensearch(factory, &dir, &cluster.url) {
            store.opensearch = Some(OpenSearchProcess::start(&dir)?);
            wait_for(factory, &backend, &dir)?;
        }
    }
    moqui_core::search::install(factory, SearchFacade::new(Arc::new(backend), entity.clone()));
    Ok(store)
}

/// An OpenSearch child process. Dropping it stops the process.
struct OpenSearchProcess {
    child: Child,
}

impl OpenSearchProcess {
    /// `bin/opensearch` in the foreground, in its own directory, as `MoquiStart.checkStartElasticSearch`
    /// does. The install's bundled JDK is used. When the directory belongs to an `opensearch` user that
    /// `su` can switch to, it runs as that user. Output goes to `logs/moqui-start.log` in the install.
    fn start(dir: &Path) -> Result<Self, Error> {
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs)
            .map_err(|err| Error::new(format!("could not make {}", logs.display())).with_source(err))?;
        let log_path = logs.join("moqui-start.log");
        let log = std::fs::File::create(&log_path)
            .map_err(|err| Error::new(format!("could not write {}", log_path.display())).with_source(err))?;
        let err_log = log
            .try_clone()
            .map_err(|err| Error::new("could not share the OpenSearch log file").with_source(err))?;
        let mut command = opensearch_command(dir);
        command
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log));
        // Its own process group, so a stop reaches the JVM and its helpers and not only `su` or the script.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let child = command
            .spawn()
            .map_err(|err| Error::new(format!("could not start OpenSearch in {}", dir.display())).with_source(err))?;
        tracing::info!(
            "Started OpenSearch from {} (pid {}), output in {}",
            dir.display(),
            child.id(),
            log_path.display()
        );
        Ok(Self { child })
    }
}

impl Drop for OpenSearchProcess {
    fn drop(&mut self) {
        stop_child(&mut self.child);
    }
}

/// Ask the process group to stop (SIGTERM on unix, so OpenSearch closes its shards), wait a little, then kill it.
///
/// `su -c` runs the command in a session of its own, outside the group. `su` passes SIGTERM on to it; the kill reaches
/// it as a child of `su`.
fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ = signal(&["-TERM", "--", &format!("-{pid}")]);
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = Command::new("pkill")
                .args(["-KILL", "-P", &pid.to_string()])
                .stderr(Stdio::null())
                .status();
            let _ = signal(&["-KILL", "--", &format!("-{pid}")]);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn signal(args: &[&str]) -> std::io::Result<std::process::ExitStatus> {
    Command::new("kill").args(args).stderr(Stdio::null()).status()
}

/// Start the install in `dir` when: the `search-facade` `cluster` `run` (`opensearch_run`) is not false, `dir/bin`
/// exists, there is no `dir/pid` (a node started with `-p pid` is managed outside this process, as Java leaves it), and
/// the cluster url is this host.
fn run_local_opensearch(factory: &ExecutionContextFactory, dir: &Path, url: &str) -> bool {
    let mut run = factory.conf_setting("search-facade/cluster", "run");
    if run.is_empty() {
        run = moqui_lib::env::get_prop_or_env("opensearch_run");
    }
    if run.trim().eq_ignore_ascii_case("false") {
        return false;
    }
    if !dir.join("bin").is_dir() || dir.join("pid").exists() {
        return false;
    }
    local_host(url)
}

/// True when the host of `url` is this machine's loopback: `127.0.0.1`, `localhost` or `[::1]`.
fn local_host(url: &str) -> bool {
    let authority = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or(""),
        None => authority.split(['/', ':']).next().unwrap_or(""),
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn opensearch_command(dir: &Path) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd.exe");
        command.args(["/c", "bin\\opensearch.bat"]);
        return command;
    }
    if let Some(user) = switchable_owner(dir) {
        let mut command = Command::new("su");
        command.args(["-c", "./bin/opensearch", &user]);
        return command;
    }
    Command::new("./bin/opensearch")
}

/// `opensearch`, when that user owns `dir`, this process runs as someone else, and `su` to it works
/// without a password (as root).
#[cfg(unix)]
fn switchable_owner(dir: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let owner = std::fs::metadata(dir).ok()?.uid();
    let output = Command::new("id").args(["-u", "opensearch"]).output().ok()?;
    let id: u32 = String::from_utf8_lossy(&output.stdout).trim().parse().ok()?;
    let own = Command::new("id").arg("-u").output().ok()?;
    let me: u32 = String::from_utf8_lossy(&own.stdout).trim().parse().ok()?;
    if id != owner || id == me {
        return None;
    }
    let su = Command::new("/bin/su")
        .args(["-c", "/bin/true", "opensearch"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    su.success().then(|| "opensearch".to_string())
}

#[cfg(not(unix))]
fn switchable_owner(_dir: &Path) -> Option<String> {
    None
}

/// Wait for the started node to answer, up to the `search-facade` `cluster` `start-wait` seconds (default 120).
fn wait_for(
    factory: &ExecutionContextFactory,
    backend: &moqui_core::search::opensearch::OpenSearchBackend,
    dir: &Path,
) -> Result<(), Error> {
    let seconds = factory.conf_number("search-facade/cluster", "start-wait", 120u64);
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if backend.reachable() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let log: PathBuf = dir.join("logs").join("moqui-start.log");
    Err(Error::new(format!(
        "OpenSearch in {} did not answer within {seconds}s; see {}",
        dir.display(),
        log.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_local() {
        assert!(local_host("http://127.0.0.1:9200"));
        assert!(local_host("http://localhost:9200/"));
        assert!(local_host("http://[::1]:9200"));
        assert!(local_host("https://[::1]"));
        assert!(!local_host("http://search.example:9200"));
        assert!(!local_host("http://[2001:db8::1]:9200"));
    }

    #[cfg(all(target_os = "linux", feature = "embedded-postgres"))]
    #[test]
    fn a_stop_reaches_the_whole_process_group() {
        let dir = std::env::temp_dir().join(format!("moqui-stop-group-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("grandchild");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("sleep 300 & echo $! > {}; wait", pid_file.display()))
            .stdin(Stdio::null());
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let grandchild = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the shell did not start its child");
            std::thread::sleep(Duration::from_millis(20));
        };
        stop_child(&mut child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::db::embedded::process_live(grandchild) {
            assert!(
                Instant::now() < deadline,
                "the child of the stopped process was left running"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
