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

//! A `<datasource>` of the merged conf, read the way Java's `EntityDatasourceFactoryImpl` and
//! `TransactionInternalBitronix` read it: the datasource's own `inline-jdbc` wins, else the
//! `<database>`'s. The host turns the result into a `DbDriver`, and the pool settings into the core
//! pool. `h2`, the Java default `entity_ds_db_conf`, and an empty name are the embedded Postgres
//! cluster (`plans/archive/EmbeddedPostgres.md`); H2 has no driver here.

use std::time::Duration;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

use crate::transaction::PoolConfig;

/// A store for an entity group that a `<datasource object-factory="...">` names, as Java's `EntityDatasourceFactory`:
/// it opens the group with the driver its entities' statements run on and the dialect they are written in. A factory
/// is registered under its `object-factory` name with [`register_datasource_factory`]; a datasource whose factory is not
/// registered is left out, and its entities use the default group.
pub trait DatasourceFactory: Send + Sync {
    /// Open `group`. `datasource` is its `<datasource>` element and `conf` the merged conf.
    ///
    /// # Errors
    ///
    /// Returns an error when the store cannot be reached or its settings are not valid.
    fn open_group(&self, group: &str, datasource: &MNode, conf: &MNode) -> Result<super::EntityGroup, Error>;
}

type Factories = std::sync::RwLock<std::collections::HashMap<String, std::sync::Arc<dyn DatasourceFactory>>>;

fn factories() -> &'static Factories {
    static FACTORIES: std::sync::OnceLock<Factories> = std::sync::OnceLock::new();
    FACTORIES.get_or_init(Default::default)
}

/// Serve the datasources whose `object-factory` is `name` with `factory`.
pub fn register_datasource_factory(name: &str, factory: std::sync::Arc<dyn DatasourceFactory>) {
    moqui_lib::sync::write(factories()).insert(name.to_string(), factory);
}

/// The factory registered under `name`.
pub fn datasource_factory(name: &str) -> Option<std::sync::Arc<dyn DatasourceFactory>> {
    moqui_lib::sync::read(factories()).get(name).cloned()
}

/// The `<datasource>` elements of the merged conf, `${}` left in their attributes.
pub fn datasource_nodes(conf: &MNode) -> Vec<MNode> {
    conf.first_named("entity-facade")
        .map(|entity| entity.children_named("datasource"))
        .unwrap_or_default()
}

/// A datasource attribute with `${}` expanded.
pub fn datasource_attribute(node: &MNode, name: &str) -> String {
    expanded(node, name)
}

/// Which driver serves a datasource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatasourceKind {
    /// The embedded Postgres cluster under the runtime directory.
    Embedded,
    /// A Postgres server the conf names.
    Postgres,
    /// A MySQL or MariaDB server the conf names.
    Mysql,
}

/// A transaction isolation level, as an `inline-jdbc` `isolation-level` or a database's `default-isolation-level` names
/// it (`ReadCommitted`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolationLevel {
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl IsolationLevel {
    /// The conf's name of a level; `None` for `None` and for an empty or unknown name.
    #[must_use]
    pub fn from_conf(name: &str) -> Option<Self> {
        match name.trim() {
            "ReadUncommitted" => Some(Self::ReadUncommitted),
            "ReadCommitted" => Some(Self::ReadCommitted),
            "RepeatableRead" => Some(Self::RepeatableRead),
            "Serializable" => Some(Self::Serializable),
            _ => None,
        }
    }

    /// The level as SQL writes it: `READ COMMITTED`.
    #[must_use]
    pub fn sql(self) -> &'static str {
        match self {
            Self::ReadUncommitted => "READ UNCOMMITTED",
            Self::ReadCommitted => "READ COMMITTED",
            Self::RepeatableRead => "REPEATABLE READ",
            Self::Serializable => "SERIALIZABLE",
        }
    }
}

impl DatasourceConf {
    /// Whether a transaction on this datasource reads one snapshot from its first statement: REPEATABLE READ or
    /// SERIALIZABLE, and MySQL with no level set, as InnoDB's default is REPEATABLE READ. The embedded database runs
    /// at READ COMMITTED whatever the conf says. See [`super::EntityGroup::with_snapshot_reads`].
    #[must_use]
    pub fn snapshot_reads(&self) -> bool {
        if self.kind == DatasourceKind::Embedded {
            return false;
        }
        match self.isolation_level {
            Some(IsolationLevel::RepeatableRead | IsolationLevel::Serializable) => true,
            Some(IsolationLevel::ReadCommitted | IsolationLevel::ReadUncommitted) => false,
            None => self.kind == DatasourceKind::Mysql,
        }
    }
}

/// The settings of the embedded cluster a datasource's conf gives (plans/Tools-Database.md §4.4): `shared-buffers`,
/// `work-mem`, `maintenance-work-mem` and `effective-cache-size`, as Postgres writes a size (`64MB`). `None` keeps the
/// cluster's default from code. They apply when this process starts the cluster.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmbeddedSettings {
    pub shared_buffers: Option<String>,
    pub work_mem: Option<String>,
    pub maintenance_work_mem: Option<String>,
    pub effective_cache_size: Option<String>,
}

impl EmbeddedSettings {
    /// Each setting as (its `datasource` attribute, its Postgres name, its value).
    #[must_use]
    pub fn entries(&self) -> [(&'static str, &'static str, Option<&str>); 4] {
        [
            ("shared-buffers", "shared_buffers", self.shared_buffers.as_deref()),
            ("work-mem", "work_mem", self.work_mem.as_deref()),
            (
                "maintenance-work-mem",
                "maintenance_work_mem",
                self.maintenance_work_mem.as_deref(),
            ),
            (
                "effective-cache-size",
                "effective_cache_size",
                self.effective_cache_size.as_deref(),
            ),
        ]
    }

    /// Whether any is set.
    #[must_use]
    pub fn any(&self) -> bool {
        self.entries().iter().any(|(_, _, value)| value.is_some())
    }

    /// Read the four attributes of the embedded `datasource` node of `group`. Each is a size as Postgres writes one:
    /// digits and a unit of exactly `B`, `kB`, `MB`, `GB` or `TB` (case as written), within the setting's bounds
    /// ([`size_bounds`]), so nothing but a size the server takes reaches its configuration.
    ///
    /// # Errors
    ///
    /// Names the datasource and the attribute whose value is not such a size.
    pub fn of(node: &MNode, group: &str) -> Result<Self, Error> {
        let read = |name: &str| -> Result<Option<String>, Error> {
            let value = expanded(node, name);
            if value.is_empty() {
                return Ok(None);
            }
            let (low, high) = size_bounds(name);
            match size_bytes(&value) {
                Some(bytes) if (low..=high).contains(&bytes) => Ok(Some(value)),
                Some(_) => Err(Error::new(format!(
                    "datasource {group} attribute {name} \"{value}\" is outside {} to {}",
                    size_text(low),
                    size_text(high)
                ))),
                None => Err(Error::new(format!(
                    "datasource {group} attribute {name} \"{value}\" is not a size such as 64MB (a number and B, kB, \
                     MB, GB or TB)"
                ))),
            }
        };
        Ok(Self {
            shared_buffers: read("shared-buffers")?,
            work_mem: read("work-mem")?,
            maintenance_work_mem: read("maintenance-work-mem")?,
            effective_cache_size: read("effective-cache-size")?,
        })
    }

    /// The four attributes on a datasource that is not the embedded cluster's: they do not apply, and are ignored with
    /// a debug line (a bad value there is logged, not refused).
    #[must_use]
    pub fn ignored(node: &MNode, group: &str) -> Self {
        for name in [
            "shared-buffers",
            "work-mem",
            "maintenance-work-mem",
            "effective-cache-size",
        ] {
            let value = expanded(node, name);
            if !value.is_empty() {
                tracing::debug!(
                    "Datasource {group} attribute {name} \"{value}\" is ignored: it sets the embedded cluster only"
                );
            }
        }
        Self::default()
    }
}

/// The bytes of a size as Postgres writes one in its configuration: digits and a unit of exactly `B`, `kB`, `MB`, `GB`
/// or `TB`, case as written. `None` for anything else, a missing unit included.
#[must_use]
pub fn size_bytes(value: &str) -> Option<u64> {
    let digits = value.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 15 {
        return None;
    }
    let number: u64 = value[..digits].parse().ok()?;
    let unit: u64 = match &value[digits..] {
        "B" => 1,
        "kB" => 1 << 10,
        "MB" => 1 << 20,
        "GB" => 1 << 30,
        "TB" => 1 << 40,
        _ => return None,
    };
    number.checked_mul(unit)
}

/// The bytes a conf attribute's size may be, low and high: within what Postgres takes and sane for a server.
#[must_use]
pub fn size_bounds(attribute: &str) -> (u64, u64) {
    match attribute {
        "shared-buffers" => (128 << 10, 64 << 30),
        "work-mem" => (64 << 10, 2 << 30),
        "maintenance-work-mem" => (1 << 20, 16 << 30),
        _ => (8 << 10, 16 << 40),
    }
}

fn size_text(bytes: u64) -> String {
    for (unit, size) in [("TB", 1_u64 << 40), ("GB", 1 << 30), ("MB", 1 << 20), ("kB", 1 << 10)] {
        if bytes >= size && bytes.is_multiple_of(size) {
            return format!("{}{unit}", bytes / size);
        }
    }
    format!("{bytes}B")
}

/// One datasource, with `${}` expanded.
#[derive(Clone)]
pub struct DatasourceConf {
    pub group: String,
    pub kind: DatasourceKind,
    /// The `database-list` entry whose dialect the entity layer uses: `postgres` for embedded.
    pub dialect: String,
    pub schema: String,
    pub host: String,
    /// `None` takes the driver's default port.
    pub port: Option<u16>,
    pub database: String,
    pub user: String,
    pub password: String,
    /// False only when the conf says `sslmode`/`sslMode` disable(d). Embedded never uses TLS.
    pub tls: bool,
    /// What a TLS connection checks of the server's certificate, from `sslmode`.
    pub tls_verify: TlsVerify,
    /// `sslrootcert`: a PEM file of the certificate authorities to trust in place of the public ones. Empty is the
    /// public ones.
    pub tls_root_cert: String,
    pub pool: PoolConfig,
    /// The `inline-jdbc` `isolation-level`, else the database's `default-isolation-level`, which each connection sets
    /// for its session, as Java's pool sets it on each connection; `None` leaves the server's default.
    pub isolation_level: Option<IsolationLevel>,
    /// `startup-add-missing`, else the database's `default-startup-add-missing`: check every table at startup.
    pub startup_add_missing: bool,
    /// `runtime-add-missing`, else the database's `default-runtime-add-missing`, true when neither is `false`.
    pub runtime_add_missing: bool,
    /// `runtime-add-fks`, true unless set to something other than `true`.
    pub runtime_add_fks: bool,
    /// `pg-stat-statements`, false unless `true`: on Postgres, load `pg_stat_statements` for this datasource's
    /// database (plans/Tools-Database.md §7.2). Ignored on MySQL.
    pub pg_stat_statements: bool,
    /// Whether the embedded cluster preloads `pg_stat_statements` when it starts for this datasource: this one's
    /// `pg-stat-statements`, or for `transactional` on the embedded cluster that of any embedded datasource
    /// (`moqui_drivers::entity_db::transactional_conf` sets it), since the cluster's databases share one postmaster.
    pub preload_pg_stat_statements: bool,
    /// The embedded cluster's settings this datasource gives (`shared-buffers`, `work-mem`, `maintenance-work-mem`,
    /// `effective-cache-size`). The cluster takes the transactional datasource's.
    pub embedded_settings: EmbeddedSettings,
    /// The `inline-jdbc` `read-timeout` in seconds: the most one read from the database waits, for a driver that sets
    /// it once per connection (MySQL). `None` when not given (the driver's default); `Some(ZERO)` for no limit.
    pub read_timeout: Option<Duration>,
}

/// Leaves out the password.
impl std::fmt::Debug for DatasourceConf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatasourceConf")
            .field("group", &self.group)
            .field("kind", &self.kind)
            .field("dialect", &self.dialect)
            .field("schema", &self.schema)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("password", &"<redacted>")
            .field("tls", &self.tls)
            .field("tls_verify", &self.tls_verify)
            .field("tls_root_cert", &self.tls_root_cert)
            .field("pool", &self.pool)
            .field("isolation_level", &self.isolation_level)
            .field("startup_add_missing", &self.startup_add_missing)
            .field("runtime_add_missing", &self.runtime_add_missing)
            .field("runtime_add_fks", &self.runtime_add_fks)
            .field("pg_stat_statements", &self.pg_stat_statements)
            .field("preload_pg_stat_statements", &self.preload_pg_stat_statements)
            .field("embedded_settings", &self.embedded_settings)
            .field("read_timeout", &self.read_timeout)
            .finish()
    }
}

/// The datasource for `group` in the merged conf `conf` (the `moqui-conf` root).
///
/// # Errors
///
/// Returns an error when the group has no datasource, its database is not in `database-list`, the
/// database has no driver in this port, or a number does not parse.
pub fn datasource_conf(conf: &MNode, group: &str) -> Result<DatasourceConf, Error> {
    let entity = conf
        .first_named("entity-facade")
        .ok_or_else(|| Error::new("the conf has no entity-facade"))?;
    let datasource = entity
        .children_named("datasource")
        .into_iter()
        .find(|node| node.attribute_is("group-name", group))
        .ok_or_else(|| Error::new(format!("the conf has no datasource for group {group}")))?;
    let conf_name = expanded(&datasource, "database-conf-name");
    let kind = match conf_name.as_str() {
        "" | "h2" => DatasourceKind::Embedded,
        "postgres" => DatasourceKind::Postgres,
        "mysql" | "mysql8" => DatasourceKind::Mysql,
        other => {
            return Err(Error::new(format!(
                "database {other} for datasource {group} has no driver in this port (postgres, mysql, mysql8, or the embedded default)"
            )))
        }
    };
    let dialect = if kind == DatasourceKind::Embedded {
        "postgres".to_string()
    } else {
        conf_name
    };
    let database_node = conf
        .first_named("database-list")
        .and_then(|list| {
            list.children_named("database")
                .into_iter()
                .find(|node| node.attribute_is("name", dialect.as_str()))
        })
        .ok_or_else(|| Error::new(format!("database-list has no database {dialect}")))?;
    let inline = datasource
        .first_named("inline-jdbc")
        .or_else(|| database_node.first_named("inline-jdbc"));
    let pool = pool_config(inline.as_ref(), &database_node)?;
    let isolation_level = isolation_level(group, inline.as_ref(), &database_node);
    if let Some(warning) = postgres_isolation_warning(group, kind, isolation_level) {
        tracing::warn!("{warning}");
    }
    // As Java's `EntityInfo`: `use-schemas="false"` puts no schema in front of the tables, so they are wherever the
    // connection puts them.
    let schema = if expanded(&database_node, "use-schemas") == "false" {
        String::new()
    } else {
        expanded(&datasource, "schema-name")
    };
    let mut conf = DatasourceConf {
        group: group.to_string(),
        kind,
        dialect,
        schema,
        host: String::new(),
        port: None,
        database: String::new(),
        user: String::new(),
        password: String::new(),
        tls: true,
        tls_verify: TlsVerify::Full,
        tls_root_cert: String::new(),
        pool,
        isolation_level,
        // As Java's `EntityFacadeImpl` and `EntityDbMeta`.
        startup_add_missing: match expanded(&datasource, "startup-add-missing").as_str() {
            "" => expanded(&database_node, "default-startup-add-missing") == "true",
            value => value == "true",
        },
        runtime_add_missing: match expanded(&datasource, "runtime-add-missing").as_str() {
            "" => expanded(&database_node, "default-runtime-add-missing") != "false",
            value => value != "false",
        },
        runtime_add_fks: match expanded(&datasource, "runtime-add-fks").as_str() {
            "" => true,
            value => value == "true",
        },
        pg_stat_statements: expanded(&datasource, "pg-stat-statements") == "true",
        preload_pg_stat_statements: false,
        embedded_settings: if kind == DatasourceKind::Embedded {
            EmbeddedSettings::of(&datasource, group)?
        } else {
            EmbeddedSettings::ignored(&datasource, group)
        },
        read_timeout: read_timeout(inline.as_ref())?,
    };
    conf.preload_pg_stat_statements = conf.pg_stat_statements;
    if kind == DatasourceKind::Embedded {
        // Only the database: a group other than `transactional` has a database of its own in the embedded cluster,
        // named by the datasource's own `databaseName` (the `database-list` one names the external server's).
        conf.database = datasource
            .first_named("inline-jdbc")
            .and_then(|own| own.first_named("xa-properties"))
            .map(|xa| expanded(&xa, "databaseName"))
            .unwrap_or_default();
    } else {
        if let Some(inline) = &inline {
            connection(&mut conf, inline)?;
        }
        if conf.host.is_empty() {
            return Err(Error::new(format!(
                "datasource {group} names database {} and no server host",
                conf.dialect
            )));
        }
    }
    // Java writes `schema.TABLE`, which on MySQL is a table of another database. The Postgres driver sets the
    // session's search path instead (`moqui-host` `entity_db`); MySQL has nothing for it here.
    if kind == DatasourceKind::Mysql && !conf.schema.is_empty() {
        tracing::warn!(
            "Datasource {group} schema-name {} is not used on MySQL; name that database in the jdbc-uri or xa-properties",
            conf.schema
        );
    }
    Ok(conf)
}

/// The `inline-jdbc` `isolation-level`, else the database's `default-isolation-level`, as Java's
/// `EntityDatasourceFactory` reads them. `None` (or neither set) leaves the server's default; a name that is no level
/// is logged and does the same.
fn isolation_level(group: &str, inline: Option<&MNode>, database: &MNode) -> Option<IsolationLevel> {
    let name = inline
        .map(|node| expanded(node, "isolation-level"))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| expanded(database, "default-isolation-level"));
    let level = IsolationLevel::from_conf(&name);
    if level.is_none() && !name.is_empty() && name != "None" {
        tracing::warn!("Datasource {group} isolation level {name} is not a level, leaving the server's default");
    }
    level
}

/// What to log of a Postgres datasource's isolation level other than READ COMMITTED, which the framework expects: the
/// embedded database does not set it, and on a server at REPEATABLE READ or SERIALIZABLE an insert that does nothing on
/// a key conflict (create-or-update, the data load) fails with a serialization error when the row it meets was
/// committed after its transaction began.
fn postgres_isolation_warning(group: &str, kind: DatasourceKind, level: Option<IsolationLevel>) -> Option<String> {
    let level = level.filter(|level| *level != IsolationLevel::ReadCommitted)?;
    let sql = level.sql();
    match kind {
        DatasourceKind::Mysql => None,
        DatasourceKind::Embedded => Some(format!(
            "Datasource {group} isolation level {sql} is not set on the embedded database, which runs at READ COMMITTED"
        )),
        DatasourceKind::Postgres if level == IsolationLevel::ReadUncommitted => Some(format!(
            "Datasource {group} isolation level {sql} runs as READ COMMITTED on Postgres"
        )),
        DatasourceKind::Postgres => Some(format!(
            "Datasource {group} isolation level {sql} on Postgres: the framework expects READ COMMITTED, and at {sql} \
             a create-or-update or data load insert that meets a row committed after its transaction began fails \
             with a serialization error"
        )),
    }
}

/// `inline-jdbc` pool attributes, with Java's fallbacks: min 5, max 50, wait 30 seconds, idle 60
/// seconds (Bitronix's default). `pool-reserve` is this port's, default 8.
fn pool_config(inline: Option<&MNode>, database: &MNode) -> Result<PoolConfig, Error> {
    let number = |name: &str, fallback: u32| -> Result<u32, Error> {
        let raw = inline.map(|node| expanded(node, name)).unwrap_or_default();
        if raw.is_empty() {
            return Ok(fallback);
        }
        raw.parse::<u32>()
            .map_err(|_| Error::new(format!("inline-jdbc {name}=\"{raw}\" is not a whole number")))
    };
    let test_query = inline
        .map(|node| expanded(node, "pool-test-query"))
        .filter(|sql| !sql.is_empty())
        .or_else(|| Some(expanded(database, "default-test-query")).filter(|sql| !sql.is_empty()));
    let max = number("pool-maxsize", 50)?;
    Ok(PoolConfig {
        min: number("pool-minsize", 5)?.min(max),
        max,
        reserve: number("pool-reserve", 8)?,
        acquire_timeout: Duration::from_secs(u64::from(number("pool-time-wait", 30)?)),
        idle_timeout: Some(Duration::from_secs(u64::from(number("pool-time-idle", 60)?))),
        test_query,
        test_after_idle: Duration::from_secs(1),
    })
}

/// The `inline-jdbc` `read-timeout`, in seconds; `None` when not given.
fn read_timeout(inline: Option<&MNode>) -> Result<Option<Duration>, Error> {
    let raw = inline.map(|node| expanded(node, "read-timeout")).unwrap_or_default();
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<u64>()
        .map(|seconds| Some(Duration::from_secs(seconds)))
        .map_err(|_| {
            Error::new(format!(
                "inline-jdbc read-timeout=\"{raw}\" is not a whole number of seconds"
            ))
        })
}

/// Server, database and credentials from `xa-properties`, else from `jdbc-uri` and the
/// `jdbc-username`/`jdbc-password` attributes.
fn connection(conf: &mut DatasourceConf, inline: &MNode) -> Result<(), Error> {
    if let Some(xa) = inline.first_named("xa-properties") {
        let get = |names: &[&str]| -> String {
            names
                .iter()
                .map(|name| expanded(&xa, name))
                .find(|value| !value.is_empty())
                .unwrap_or_default()
        };
        conf.host = get(&["serverName"]);
        conf.port = port(&get(&["portNumber", "port"]))?;
        conf.database = get(&["databaseName"]);
        conf.user = get(&["user"]);
        conf.password = get(&["password"]);
        let mode = get(&["sslmode", "sslMode"]);
        conf.tls = tls_wanted(&mode);
        conf.tls_verify = tls_verify(&mode);
        conf.tls_root_cert = get(&["sslrootcert", "sslRootCert"]);
        let url = get(&["url", "URL"]);
        if conf.host.is_empty() && !url.is_empty() {
            jdbc_uri(conf, &url)?;
        }
        return Ok(());
    }
    let uri = expanded(inline, "jdbc-uri");
    if !uri.is_empty() {
        jdbc_uri(conf, &uri)?;
    }
    conf.user = expanded(inline, "jdbc-username");
    conf.password = expanded(inline, "jdbc-password");
    Ok(())
}

/// `jdbc:postgresql://host[:port]/db[?k=v&...]` or the `jdbc:mysql:` form. Java hands every parameter to the JDBC
/// driver. Those with an equivalent in this port's drivers are read: `sslmode`/`sslMode`, `sslrootcert` and MySQL's
/// `useSSL` for TLS, and Postgres `currentSchema` (one plain name) as the schema when the datasource has no
/// `schema-name`. Any other is logged and left out.
fn jdbc_uri(conf: &mut DatasourceConf, uri: &str) -> Result<(), Error> {
    let rest = uri
        .strip_prefix("jdbc:postgresql://")
        .or_else(|| uri.strip_prefix("jdbc:mysql://"))
        .or_else(|| uri.strip_prefix("jdbc:mariadb://"))
        .ok_or_else(|| Error::new(format!("jdbc uri {uri} is not a postgresql or mysql uri")))?;
    let (address, rest) = rest.split_once('/').unwrap_or((rest, ""));
    let (database, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, port_text) = match address.rsplit_once(':') {
        // A bare IPv6 address has colons of its own; `[::1]:5432` has its port after the bracket.
        Some((host, port)) if !host.contains(':') || host.ends_with(']') => (host, port),
        _ => (address, ""),
    };
    conf.host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    conf.port = port(port_text)?;
    conf.database = database.to_string();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key.eq_ignore_ascii_case("sslmode") {
            conf.tls = tls_wanted(value);
            conf.tls_verify = tls_verify(value);
        } else if key.eq_ignore_ascii_case("sslrootcert") {
            conf.tls_root_cert = value.to_string();
        } else if key.eq_ignore_ascii_case("useSSL") {
            conf.tls = !value.eq_ignore_ascii_case("false");
        } else if key == "currentSchema"
            && conf.schema.is_empty()
            && !value.is_empty()
            && value.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            conf.schema = value.to_string();
        } else {
            tracing::warn!(
                "Datasource {} jdbc-uri parameter {key} has no equivalent in this port's driver, ignoring it",
                conf.group
            );
        }
    }
    Ok(())
}

fn port(text: &str) -> Result<Option<u16>, Error> {
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<u16>()
        .map(Some)
        .map_err(|_| Error::new(format!("datasource port {text:?} is not a port")))
}

/// An `sslmode` value asks for TLS unless it is `disable` or `disabled`; empty asks for it.
pub fn tls_wanted(mode: &str) -> bool {
    !matches!(mode.to_ascii_lowercase().as_str(), "disable" | "disabled")
}

/// What a TLS connection checks of the server's certificate. TLS is always required when it is on: there is no
/// fallback to an unencrypted connection, so libpq's `allow` and `prefer` are as strict as `verify-full`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TlsVerify {
    /// `require`: encrypted, the certificate not checked, as libpq and pgjdbc do with no root certificate file. With
    /// [`DatasourceConf::tls_root_cert`] it is checked as [`Self::Ca`], as libpq does when that file exists.
    Encrypt,
    /// `verify-ca`: signed by a trusted authority, for any host name.
    Ca,
    /// `verify-full`, empty, `allow` and `prefer`: signed by a trusted authority, for the host connected to.
    #[default]
    Full,
}

/// The [`TlsVerify`] an `sslmode` value names (MySQL's `sslMode` `REQUIRED` and `VERIFY_CA` too).
pub fn tls_verify(mode: &str) -> TlsVerify {
    match mode.to_ascii_lowercase().replace('_', "-").as_str() {
        "require" | "required" => TlsVerify::Encrypt,
        "verify-ca" => TlsVerify::Ca,
        _ => TlsVerify::Full,
    }
}

fn expanded(node: &MNode, name: &str) -> String {
    let raw = node.attribute(name).unwrap_or_default();
    moqui_lib::env::expand(&raw).unwrap_or(raw).trim().to_string()
}

#[cfg(test)]
#[path = "datasource_tests.rs"]
mod tests;
