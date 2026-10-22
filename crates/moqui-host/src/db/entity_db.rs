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

//! The entity database a conf names. The `transactional` datasource is the embedded cluster unless
//! `entity_ds_db_conf` (or the datasource) names `postgres`, `mysql` or `mysql8`; then it is that
//! server, and no postmaster starts for it (`plans/archive/EmbeddedPostgres.md`).

use std::path::Path;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::DbDriver;
use moqui_core::entity::datasource::{datasource_conf, DatasourceConf, DatasourceKind};
use moqui_lib::error::Error;
use moqui_lib::node::MNode;

/// Postgres keeps three connection slots for superusers. The embedded role is a superuser, so they
/// are room for `pg_dump` beside a full pool rather than for the pool.
pub const SUPERUSER_SLOTS: u32 = 3;
/// The cluster `LISTEN` connection, which is outside the pool.
pub const LISTEN_SLOTS: u32 = 1;
const POSTGRES_PORT: u16 = 5432;
#[cfg(feature = "mysql")]
const MYSQL_PORT: u16 = 3306;

/// The open entity database. Drop the facade before this, and this before nothing else: the
/// driver goes first, then the embedded cluster when there is one.
pub struct EntityDb {
    pub conf: DatasourceConf,
    pub driver: Arc<dyn DbDriver>,
    /// The same server as a Postgres driver, for a connection outside the pool (the cluster `LISTEN`).
    #[cfg(feature = "postgres")]
    pub postgres: Option<super::postgres::PostgresDriver>,
    #[cfg(feature = "embedded-postgres")]
    cluster: Option<super::embedded::EmbeddedCluster>,
}

impl EntityDb {
    /// True for embedded and external Postgres, whose `search` schema can hold documents.
    #[must_use]
    pub fn is_postgres(&self) -> bool {
        self.conf.kind != DatasourceKind::Mysql
    }

    /// True when this process serves the database from the embedded cluster.
    #[must_use]
    pub fn is_embedded(&self) -> bool {
        self.conf.kind == DatasourceKind::Embedded
    }

    /// `pg_dump` / `pg_basebackup` coordinates when this process runs the postmaster.
    #[cfg(feature = "embedded-postgres")]
    #[must_use]
    pub fn tool_target(&self) -> Option<super::embedded::ToolTarget> {
        self.cluster.as_ref().map(|cluster| cluster.tool_connect("moqui"))
    }
}

impl Drop for EntityDb {
    fn drop(&mut self) {
        // The driver's clients go before the postmaster stops.
        self.driver = Arc::new(Closed);
        #[cfg(feature = "embedded-postgres")]
        self.cluster.take();
    }
}

struct Closed;

impl DbDriver for Closed {
    fn connect(&self) -> Result<Box<dyn moqui_core::driver::DbConnection>, moqui_lib::error::Error> {
        Err(Error::new("the entity database is closed"))
    }
}

/// The `transactional` datasource of `factory`'s merged conf.
///
/// # Errors
///
/// Returns an error when the conf does not parse or the datasource cannot be read.
pub fn transactional_conf(factory: &ExecutionContextFactory) -> Result<DatasourceConf, Error> {
    let conf = MNode::parse_text("moqui-conf", factory.conf_text())?;
    datasource_conf(&conf, "transactional")
}

/// The driver for a datasource of a server this process does not start (Postgres or MySQL), for an entity group
/// beside `transactional`. `None` for the embedded cluster, which the caller shares from the main datasource.
///
/// # Errors
///
/// Returns an error when the driver for the kind is not in this build or cannot be made.
pub fn server_driver(conf: &DatasourceConf) -> Result<Option<Arc<dyn DbDriver>>, Error> {
    match conf.kind {
        DatasourceKind::Embedded => Ok(None),
        DatasourceKind::Mysql => mysql_driver(conf).map(Some),
        DatasourceKind::Postgres => postgres_dyn_driver(conf).map(Some),
    }
}

#[cfg(feature = "postgres")]
fn postgres_dyn_driver(conf: &DatasourceConf) -> Result<Arc<dyn DbDriver>, Error> {
    Ok(Arc::new(postgres_driver(conf)?))
}

#[cfg(not(feature = "postgres"))]
fn postgres_dyn_driver(_conf: &DatasourceConf) -> Result<Arc<dyn DbDriver>, Error> {
    Err(Error::new("the postgres driver is not in this build"))
}

/// Open the database `conf` names. Embedded starts (or adopts) the cluster at `cluster_dir`, on the
/// theseus install at `install_dir`, with `max_connections` of `pool_max` plus [`LISTEN_SLOTS`] and
/// [`SUPERUSER_SLOTS`].
///
/// # Errors
///
/// Returns an error when the cluster does not start, or the driver for the kind is not in this
/// build or cannot be made.
pub fn open(conf: DatasourceConf, cluster_dir: &Path, install_dir: &Path, pool_max: u32) -> Result<EntityDb, Error> {
    match conf.kind {
        DatasourceKind::Embedded => open_embedded(conf, cluster_dir, install_dir, pool_max),
        DatasourceKind::Postgres => open_postgres(conf),
        DatasourceKind::Mysql => {
            let driver = mysql_driver(&conf)?;
            Ok(EntityDb {
                conf,
                driver,
                #[cfg(feature = "postgres")]
                postgres: None,
                #[cfg(feature = "embedded-postgres")]
                cluster: None,
            })
        }
    }
}

#[cfg(feature = "embedded-postgres")]
fn open_embedded(
    conf: DatasourceConf,
    cluster_dir: &Path,
    install_dir: &Path,
    pool_max: u32,
) -> Result<EntityDb, Error> {
    use super::embedded::{ClusterRole, EmbeddedCluster, StartOptions};

    let options = StartOptions {
        max_connections: pool_max + LISTEN_SLOTS + SUPERUSER_SLOTS,
        role: ClusterRole::Entity,
        install_dir: Some(install_dir.to_path_buf()),
    };
    let (cluster, driver) = EmbeddedCluster::start_with(cluster_dir, &options)?;
    Ok(EntityDb {
        conf,
        driver: Arc::new(driver.clone()),
        postgres: Some(driver),
        cluster: Some(cluster),
    })
}

#[cfg(not(feature = "embedded-postgres"))]
fn open_embedded(
    _conf: DatasourceConf,
    _cluster_dir: &Path,
    _install_dir: &Path,
    _pool_max: u32,
) -> Result<EntityDb, Error> {
    Err(Error::new(
        "embedded postgres is not in this build; set entity_ds_db_conf to postgres or mysql",
    ))
}

#[cfg(feature = "postgres")]
fn open_postgres(conf: DatasourceConf) -> Result<EntityDb, Error> {
    let driver = postgres_driver(&conf)?;
    Ok(EntityDb {
        conf,
        driver: Arc::new(driver.clone()),
        postgres: Some(driver),
        #[cfg(feature = "embedded-postgres")]
        cluster: None,
    })
}

#[cfg(not(feature = "postgres"))]
fn open_postgres(_conf: DatasourceConf) -> Result<EntityDb, Error> {
    Err(Error::new("the postgres driver is not in this build"))
}

#[cfg(feature = "postgres")]
fn postgres_driver(conf: &DatasourceConf) -> Result<super::postgres::PostgresDriver, Error> {
    let password = (!conf.password.is_empty()).then_some(conf.password.as_str());
    let driver = postgres_server_driver(
        &conf.user,
        password,
        &conf.database,
        &conf.host,
        conf.port.unwrap_or(POSTGRES_PORT),
        conf.tls
            .then(|| super::pg_tls::TlsOptions::new(conf.tls_verify, &conf.tls_root_cert))
            .as_ref(),
    )?;
    // Java puts the datasource's `schema-name` in front of each table name. Here the session's search path names
    // it first, with `public` after it for the extensions there.
    Ok(match search_path(&conf.schema)? {
        Some(path) => driver.options(format!("-c search_path={path}")),
        None => driver,
    })
}

/// The `search_path` for a `schema-name`: none when it is empty. Only a plain identifier is taken.
fn search_path(schema: &str) -> Result<Option<String>, Error> {
    let schema = schema.trim();
    if schema.is_empty() {
        return Ok(None);
    }
    if !schema.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(Error::new(format!(
            "datasource schema-name {schema} is not a plain identifier"
        )));
    }
    Ok(Some(if schema.eq_ignore_ascii_case("public") {
        "public".to_string()
    } else {
        format!("{schema},public")
    }))
}

/// A driver for a Postgres server this process did not start: the entity datasource, a search
/// `postgres` url, and its read replica. As in libpq, a host that is an absolute path is the
/// server's Unix socket directory. Otherwise TCP, with TLS as `tls` says, or none.
///
/// # Errors
///
/// Returns an error when the root certificate file cannot be read or the TLS configuration cannot be built.
#[cfg(feature = "postgres")]
pub fn postgres_server_driver(
    user: &str,
    password: Option<&str>,
    database: &str,
    host: &str,
    port: u16,
    tls: Option<&super::pg_tls::TlsOptions>,
) -> Result<super::postgres::PostgresDriver, Error> {
    use super::postgres::PostgresDriver;

    #[cfg(unix)]
    let socket = host.starts_with('/');
    #[cfg(not(unix))]
    let socket = false;
    let driver = if socket {
        #[cfg(unix)]
        {
            PostgresDriver::unix_socket(user, database, host).port(port)
        }
        #[cfg(not(unix))]
        {
            unreachable!("a socket host is unix only")
        }
    } else if let Some(tls) = tls {
        PostgresDriver::external_with(user, database, host, port, tls)?
    } else {
        PostgresDriver::tcp_no_tls(user, database, host, port)
    };
    Ok(match password {
        Some(password) => driver.password(password),
        None => driver,
    })
}

#[cfg(feature = "mysql")]
fn mysql_driver(conf: &DatasourceConf) -> Result<Arc<dyn DbDriver>, Error> {
    Ok(Arc::new(super::mysql::MysqlDriver::new(
        &conf.host,
        conf.port.unwrap_or(MYSQL_PORT),
        &conf.user,
        &conf.password,
        &conf.database,
        conf.tls,
    )))
}

#[cfg(not(feature = "mysql"))]
fn mysql_driver(_conf: &DatasourceConf) -> Result<Arc<dyn DbDriver>, Error> {
    Err(Error::new("the mysql driver is not in this build"))
}
