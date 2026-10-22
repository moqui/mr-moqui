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

//! `DbDriver` implementations. The pool stays in `moqui-core`.

use std::sync::Arc;

use moqui_core::driver::{ConfDb, DbDriver};
use moqui_lib::error::Error;

#[cfg(feature = "embedded-postgres")]
pub mod backup;
pub mod embedded;
pub mod entity_db;
pub mod entity_facade;
pub mod jdbc_extract;
pub mod mysql;
#[cfg(feature = "postgres")]
pub mod pg_tls;
#[cfg(feature = "postgres")]
mod pgwire;
pub mod postgres;
pub mod sim;
#[cfg(any(feature = "mysql", feature = "postgres"))]
mod statement_lru;

/// `remaining` in whole milliseconds for a server statement timeout. Both servers read 0 as no limit, so a remainder
/// under a millisecond is 1, and a remainder of zero, a deadline that has already passed, is an error.
#[cfg(any(feature = "mysql", feature = "postgres"))]
fn timeout_millis(remaining: std::time::Duration) -> Result<u64, moqui_lib::error::Error> {
    if remaining.is_zero() {
        return Err(moqui_lib::error::Error::new(
            "the transaction timed out before the statement timeout was set",
        ));
    }
    Ok(u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX).max(1))
}

/// A driver for a database named by its `database-list` conf name: `postgres`, or `mysql` and `mysql8`. An empty port
/// is the default. A Postgres host is TLS, as an external Postgres is reached elsewhere, except a loopback host (no
/// TLS) and a host starting with `/` (a Unix socket directory, as libpq reads it).
///
/// # Errors
///
/// When the conf name has no driver or the port is not a number.
pub fn conf_driver(conf: &ConfDb) -> Result<Arc<dyn DbDriver>, Error> {
    let port_of = |default: u16| -> Result<u16, Error> {
        if conf.port.trim().is_empty() {
            return Ok(default);
        }
        conf.port
            .trim()
            .parse()
            .map_err(|err| Error::new(format!("database port {} is not a number", conf.port)).with_source(err))
    };
    let ConfDb {
        db_type,
        host,
        database,
        user,
        password,
        ..
    } = conf;
    let driver: Arc<dyn DbDriver> = match db_type.as_str() {
        #[cfg(feature = "postgres")]
        "postgres" => {
            let port = port_of(5432)?;
            let driver = if host.starts_with('/') {
                postgres_socket(user, database, host, port)?
            } else if matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1") {
                postgres::PostgresDriver::tcp_no_tls(user, database, host, port)
            } else {
                postgres::PostgresDriver::external(user, database, host, port)?
            };
            Arc::new(driver.password(password))
        }
        #[cfg(feature = "mysql")]
        "mysql" | "mysql8" => Arc::new(mysql::MysqlDriver::new(
            host,
            port_of(3306)?,
            user,
            password,
            database,
            false,
        )),
        other => {
            let _ = (host, database, user, password, port_of(0));
            Err(Error::new(format!("no database driver for database conf {other}")))?
        }
    };
    Ok(driver)
}

#[cfg(all(feature = "postgres", unix))]
fn postgres_socket(user: &str, database: &str, socket_dir: &str, port: u16) -> Result<postgres::PostgresDriver, Error> {
    Ok(postgres::PostgresDriver::unix_socket(user, database, socket_dir).port(port))
}

#[cfg(all(feature = "postgres", not(unix)))]
fn postgres_socket(
    _user: &str,
    _database: &str,
    socket_dir: &str,
    _port: u16,
) -> Result<postgres::PostgresDriver, Error> {
    Err(Error::new(format!(
        "Unix socket database host {socket_dir} is not supported on this platform"
    )))
}

/// Install [`conf_driver`] as the core's conf connection factory, for `ec.entity` admin SQL.
pub fn install_conf_drivers() {
    moqui_core::driver::install_conf_driver_factory(Arc::new(conf_driver));
}

#[cfg(all(test, unix, feature = "embedded-postgres"))]
mod tests {
    use std::path::PathBuf;

    use moqui_core::driver::ConfDb;

    use super::embedded::EmbeddedCluster;

    struct DirGuard(PathBuf);

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The InstanceServices admin connection: a socket directory host, and `CREATE DATABASE`, which Postgres refuses
    /// inside a transaction.
    #[test]
    fn a_conf_driver_reaches_a_socket_postgres_and_creates_a_database() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = PathBuf::from("/tmp").join(format!("moqui-pg-conf-{}-{nanos}", std::process::id()));
        let _guard = DirGuard(dir.clone());
        let (cluster, _driver) = EmbeddedCluster::start(&dir, 20).unwrap();
        let (user, password) = cluster.superuser();
        let driver = super::conf_driver(&ConfDb {
            db_type: "postgres".to_string(),
            host: cluster.data_dir().display().to_string(),
            port: cluster.port().to_string(),
            database: "postgres".to_string(),
            user,
            password,
        })
        .unwrap();
        let mut conn = driver.connect().unwrap();
        conn.execute("CREATE DATABASE conf_tenant", &[]).unwrap();
        let rows = conn
            .query_once(
                "SELECT COUNT(*) AS row_count FROM pg_database WHERE datname='conf_tenant'",
                &[],
            )
            .unwrap();
        let count = rows[0].columns[0].1.clone();
        assert_eq!(moqui_lib::object::to_plain_string(&count), "1", "{count:?}");
        drop(conn);

        let err = super::conf_driver(&ConfDb {
            db_type: "oracle".to_string(),
            ..ConfDb::default()
        })
        .err()
        .expect("no oracle driver");
        assert!(
            err.to_string().contains("no database driver for database conf oracle"),
            "{err}"
        );
    }
}
