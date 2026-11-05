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

//! What a database allows the Tools screens, and why the rest is not available (plans/Tools-Database.md §4.1): probed
//! once with the queries of [`crate::catalog`] and cached for [`CACHE_FOR`] per database. Each feature is a
//! [`Feature`] under one of the names of [`FEATURES`]; a probe that fails is logged and marks the features it decides
//! as not available, with the error as the reason, and the others are still probed.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};
use moqui_lib::sync::lock;

use moqui_core::entity::EntityFacade;

use crate::catalog;

/// How long a probe's answer is kept.
pub const CACHE_FOR: Duration = Duration::from_secs(300);
/// How long an answer is kept when the database did not answer.
pub const FAILED_CACHE_FOR: Duration = Duration::from_secs(15);

/// The feature names, in the order the screens show them.
pub const FEATURES: &[&str] = &[
    "statementStats",
    "activity",
    "cancel",
    "terminate",
    "lockWaits",
    "tableSizes",
    "bloatExact",
    "indexUsage",
    "indexOnline",
    "fkLightLock",
    "maintenance",
    "integrity",
    "settings",
    "settingsEdit",
    "backups",
];

/// The kind of server behind a database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// The cluster this process runs (the app connects as its bootstrap superuser).
    EmbeddedPostgres,
    Postgres,
    Mysql,
    Mariadb,
}

impl Backend {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::EmbeddedPostgres => "embedded-postgres",
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }
}

/// One feature: whether it is available, and why not, or what limits it when it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Feature {
    pub available: bool,
    pub reason: Option<String>,
}

impl Feature {
    fn yes() -> Self {
        Self {
            available: true,
            reason: None,
        }
    }

    fn limited(reason: impl Into<String>) -> Self {
        Self {
            available: true,
            reason: Some(reason.into()),
        }
    }

    fn no(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
        }
    }
}

/// What one database allows.
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    /// `group:<name>` or `ext:<connectionId>`.
    pub db: String,
    pub backend: Option<Backend>,
    pub version: Option<String>,
    pub database_name: Option<String>,
    pub user_name: Option<String>,
    /// The server's start, as the server gives it.
    pub start_time: Option<Value>,
    pub uptime_seconds: Option<i64>,
    pub database_size: Option<i64>,
    /// By the names of [`FEATURES`].
    pub features: BTreeMap<&'static str, Feature>,
    /// Why nothing could be probed (the server did not answer), when so.
    pub error: Option<String>,
}

impl Capabilities {
    /// The feature `name`, not available when it was not probed.
    #[must_use]
    pub fn feature(&self, name: &str) -> Feature {
        self.features
            .get(name)
            .cloned()
            .unwrap_or_else(|| Feature::no("not probed"))
    }

    /// The service's out-parameters: `db`, `available`, `reason`, `backend`, `version`, `databaseName`, `userName`,
    /// `startTime`, `uptimeSeconds`, `databaseSize`, and `features`, a map of each feature name to `available` and
    /// `reason`.
    #[must_use]
    pub fn to_out(&self) -> BTreeMap<String, Value> {
        let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let number = |value: Option<i64>| value.map_or(Value::Null, Value::I64);
        let mut out = BTreeMap::new();
        out.insert("db".to_string(), Value::string(&self.db));
        out.insert("available".to_string(), Value::Bool(self.error.is_none()));
        out.insert("reason".to_string(), text(&self.error));
        out.insert(
            "backend".to_string(),
            self.backend
                .map_or(Value::Null, |backend| Value::string(backend.name())),
        );
        out.insert("version".to_string(), text(&self.version));
        out.insert("databaseName".to_string(), text(&self.database_name));
        out.insert("userName".to_string(), text(&self.user_name));
        out.insert("startTime".to_string(), self.start_time.clone().unwrap_or(Value::Null));
        out.insert("uptimeSeconds".to_string(), number(self.uptime_seconds));
        out.insert("databaseSize".to_string(), number(self.database_size));
        let features: IndexMap<String, Value> = FEATURES
            .iter()
            .map(|name| {
                let feature = self.feature(name);
                let mut entry = IndexMap::new();
                entry.insert("available".to_string(), Value::Bool(feature.available));
                entry.insert("reason".to_string(), text(&feature.reason));
                ((*name).to_string(), Value::Map(entry))
            })
            .collect();
        out.insert("features".to_string(), Value::Map(features));
        out
    }
}

pub(crate) type Row = IndexMap<String, Value>;
/// Runs one named probe query, its error as text.
type Probe<'a> = &'a dyn Fn(&str, &str) -> Result<Vec<Row>, String>;
type Cache = Mutex<HashMap<String, (Instant, Arc<Capabilities>)>>;

fn first(result: &Result<Vec<Row>, String>) -> Option<&Row> {
    result.as_ref().ok().and_then(|rows| rows.first())
}

fn column_text(row: Option<&Row>, name: &str) -> Option<String> {
    row.and_then(|row| row.get(name))
        .filter(|value| !matches!(value, Value::Null))
        .map(to_plain_string)
}

fn column_number(row: Option<&Row>, name: &str) -> Option<i64> {
    column_text(row, name).and_then(|text| text.split('.').next().and_then(|whole| whole.parse().ok()))
}

fn column_flag(row: Option<&Row>, name: &str) -> bool {
    match row.and_then(|row| row.get(name)) {
        Some(Value::Bool(flag)) => *flag,
        Some(other) => matches!(to_plain_string(other).as_str(), "1" | "t" | "true" | "ON" | "on"),
        None => false,
    }
}

/// Probe `group` of `facade` now. `embedded` says the group's database is on the embedded cluster this process runs.
#[must_use]
pub fn probe_group(facade: &EntityFacade, group: &str, embedded: bool) -> Capabilities {
    let db = format!("group:{group}");
    // The driver's error can name the host and port: it goes to the log, and the reason the screen shows does not
    // carry it.
    let run = |name: &str, sql: &str| {
        catalog::query(facade, group, sql).map_err(|err| {
            tracing::warn!(error = %err, "Capabilities probe {name} failed on {db}");
            format!("the {name} query failed (the server log has the detail)")
        })
    };
    match catalog::group_dialect(facade, group).as_deref() {
        Some("postgres") => probe_postgres(db.clone(), embedded, &run),
        Some("mysql" | "mysql8") => probe_mysql(db.clone(), &run),
        Some(other) => Capabilities {
            db,
            error: Some(format!(
                "the Tools database screens do not support the {other} database"
            )),
            ..Capabilities::default()
        },
        None => Capabilities {
            db,
            error: Some(format!("no entity group named {group}")),
            ..Capabilities::default()
        },
    }
}

pub(crate) fn probe_postgres(db: String, embedded: bool, run: Probe<'_>) -> Capabilities {
    let mut caps = Capabilities {
        db,
        backend: Some(if embedded {
            Backend::EmbeddedPostgres
        } else {
            Backend::Postgres
        }),
        ..Capabilities::default()
    };
    let server = run("server", catalog::postgres::SERVER);
    if let Err(err) = &server {
        caps.error = Some(format!("the database did not answer: {err}"));
        for name in FEATURES {
            caps.features
                .insert(name, Feature::no(caps.error.clone().unwrap_or_default()));
        }
        return caps;
    }
    let row = first(&server);
    caps.version = column_text(row, "version");
    caps.database_name = column_text(row, "database_name");
    caps.user_name = column_text(row, "user_name");
    caps.start_time = row.and_then(|row| row.get("start_time")).cloned();
    caps.uptime_seconds = column_number(row, "uptime_seconds");
    caps.database_size = column_number(row, "database_size");
    let superuser = column_flag(row, "is_superuser");

    let roles = run("roles", catalog::postgres::ROLES);
    let role = |name: &str| superuser || column_flag(first(&roles), name);
    let read_all_stats = role("pg_read_all_stats") || role("pg_monitor");
    let signal = role("pg_signal_backend");

    let extensions = run("extensions", catalog::postgres::EXTENSIONS);
    let extension = |name: &str| -> (bool, bool) {
        let rows = extensions.as_deref().unwrap_or_default();
        let found = rows
            .iter()
            .find(|row| column_text(Some(row), "name").as_deref() == Some(name));
        (
            found.is_some(),
            found.is_some_and(|row| column_text(Some(row), "installed_version").is_some()),
        )
    };
    // A role without pg_read_all_settings may not read shared_preload_libraries (RDS, Cloud SQL): unknown then, and
    // the view itself decides.
    let preloaded = run("preload", catalog::postgres::PRELOAD).ok().map(|rows| {
        column_text(rows.first(), "libraries")
            .unwrap_or_default()
            .split(',')
            .any(|library| library.trim() == "pg_stat_statements")
    });

    // Statement stats in the database
    let statements = {
        let (present, installed) = extension("pg_stat_statements");
        if !present {
            Feature::no("the server has no pg_stat_statements extension")
        } else if preloaded == Some(false) {
            Feature::no(if embedded {
                "pg_stat_statements is not loaded: turn on the datasource's pg-stat-statements and restart"
            } else {
                "pg_stat_statements is not loaded: the server needs it in shared_preload_libraries"
            })
        } else if !installed {
            Feature::no("the pg_stat_statements extension is not created in this database (CREATE EXTENSION)")
        } else {
            match run("statements", catalog::postgres::STATEMENTS_READABLE) {
                Err(_) if preloaded.is_none() => Feature::no(
                    "pg_stat_statements does not read: it is not in shared_preload_libraries, or this role may not read it",
                ),
                Err(err) => Feature::no(format!("pg_stat_statements does not read: {err}")),
                Ok(_) if !read_all_stats => {
                    Feature::limited("the text of other roles' statements needs the pg_read_all_stats role")
                }
                Ok(_) => Feature::yes(),
            }
        }
    };
    caps.features.insert("statementStats", statements);
    caps.features.insert(
        "activity",
        if read_all_stats {
            Feature::yes()
        } else {
            Feature::limited("only this role's sessions; the others need the pg_read_all_stats or pg_monitor role")
        },
    );
    let signal_feature = if signal {
        Feature::yes()
    } else {
        Feature::limited("only this role's sessions; the others need the pg_signal_backend role")
    };
    caps.features.insert("cancel", signal_feature.clone());
    caps.features.insert("terminate", signal_feature);
    caps.features.insert("lockWaits", Feature::yes());
    caps.features.insert("tableSizes", Feature::yes());
    caps.features.insert(
        "bloatExact",
        extension_feature(extension("pgstattuple"), "pgstattuple", superuser),
    );
    caps.features.insert("indexUsage", Feature::yes());
    caps.features.insert("indexOnline", Feature::yes());
    caps.features.insert("fkLightLock", Feature::yes());
    caps.features.insert(
        "maintenance",
        if superuser {
            Feature::yes()
        } else {
            Feature::limited("only the tables this role owns")
        },
    );
    caps.features.insert(
        "integrity",
        extension_feature(extension("amcheck"), "amcheck", superuser),
    );
    caps.features.insert("settings", Feature::yes());
    caps.features.insert(
        "settingsEdit",
        if embedded {
            Feature::yes()
        } else {
            Feature::no("the settings of an external server are read only here")
        },
    );
    caps.features.insert(
        "backups",
        if embedded {
            Feature::yes()
        } else {
            Feature::no("an external server's backups are its provider's")
        },
    );
    caps
}

/// An extension the screens use: there when installed, or when the role is a superuser that can create it.
fn extension_feature((present, installed): (bool, bool), name: &str, superuser: bool) -> Feature {
    if installed {
        Feature::yes()
    } else if !present {
        Feature::no(format!("the server has no {name} extension"))
    } else if superuser {
        Feature::limited(format!("{name} is created in this database the first time it is used"))
    } else {
        Feature::no(format!("the {name} extension is not created in this database"))
    }
}

fn probe_mysql(db: String, run: Probe<'_>) -> Capabilities {
    let mut caps = Capabilities {
        db,
        backend: Some(Backend::Mysql),
        ..Capabilities::default()
    };
    let server = run("server", catalog::mysql::SERVER);
    if let Err(err) = &server {
        caps.error = Some(format!("the database did not answer: {err}"));
        for name in FEATURES {
            caps.features
                .insert(name, Feature::no(caps.error.clone().unwrap_or_default()));
        }
        return caps;
    }
    let row = first(&server);
    caps.version = column_text(row, "version");
    let comment = column_text(row, "version_comment").unwrap_or_default();
    let mariadb = caps
        .version
        .as_deref()
        .is_some_and(|version| version.to_ascii_lowercase().contains("mariadb"))
        || comment.to_ascii_lowercase().contains("mariadb");
    if mariadb {
        caps.backend = Some(Backend::Mariadb);
    }
    caps.database_name = column_text(row, "database_name");
    caps.user_name = column_text(row, "user_name");
    let uptime = run("uptime", catalog::mysql::UPTIME);
    caps.uptime_seconds = uptime.as_ref().ok().and_then(|rows| {
        rows.iter()
            .find_map(|row| row.iter().find(|(name, _)| name.eq_ignore_ascii_case("value")))
            .and_then(|(_, value)| to_plain_string(value).parse().ok())
    });
    caps.database_size = column_number(first(&run("size", catalog::mysql::DATABASE_SIZE)), "database_size");

    let grant_texts: Vec<String> = run("grants", catalog::mysql::GRANTS)
        .unwrap_or_default()
        .iter()
        .flat_map(|row| row.values().map(to_plain_string))
        .collect();
    let grants = GlobalGrants::parse(&grant_texts);
    let process = grants.has("PROCESS");
    let kill_any = grants.has("SUPER")
        || grants.has(if mariadb {
            "CONNECTION ADMIN"
        } else {
            "CONNECTION_ADMIN"
        });
    let roles_note = if grants.through_roles {
        "; privileges granted through roles are not counted here"
    } else {
        ""
    };
    let performance_schema = column_flag(
        first(&run("performance_schema", catalog::mysql::PERFORMANCE_SCHEMA)),
        "performance_schema",
    );

    let statements = if !performance_schema {
        Feature::no(if mariadb {
            "performance_schema is off; MariaDB turns it on only at a server restart with performance_schema=ON"
        } else {
            "performance_schema is off on this server"
        })
    } else {
        match run("digests", catalog::mysql::DIGESTS_READABLE) {
            Ok(_) => Feature::yes(),
            Err(err) => Feature::no(format!("the statement digests do not read: {err}")),
        }
    };
    caps.features.insert("statementStats", statements);
    caps.features.insert(
        "activity",
        if process {
            Feature::yes()
        } else {
            Feature::limited(format!(
                "only this user's threads; the others need the PROCESS privilege{roles_note}"
            ))
        },
    );
    let kill = if kill_any {
        Feature::yes()
    } else {
        Feature::limited(format!(
            "only this user's threads; the others need the {} privilege{roles_note}",
            if mariadb {
                "CONNECTION ADMIN or SUPER"
            } else {
                "CONNECTION_ADMIN"
            }
        ))
    };
    caps.features.insert("cancel", kill.clone());
    caps.features.insert("terminate", kill);
    caps.features.insert(
        "lockWaits",
        if mariadb || performance_schema {
            Feature::yes()
        } else {
            Feature::no("lock waits are read from performance_schema, which is off")
        },
    );
    caps.features.insert("tableSizes", Feature::yes());
    caps.features.insert(
        "bloatExact",
        Feature::no("exact bloat is Postgres only; the table sizes show DATA_FREE"),
    );
    let sys = run("sys", catalog::mysql::SYS_INDEXES_READABLE);
    caps.features.insert(
        "indexUsage",
        match sys {
            Err(err) => Feature::no(format!("the sys schema's index views do not read: {err}")),
            Ok(_) if !performance_schema => Feature::limited("unused indexes need performance_schema, which is off"),
            Ok(_) => Feature::yes(),
        },
    );
    caps.features.insert("indexOnline", Feature::yes());
    caps.features.insert(
        "fkLightLock",
        Feature::no("adding a checked foreign key copies the table; in place only with foreign_key_checks=0, which does not check"),
    );
    caps.features.insert("maintenance", Feature::yes());
    caps.features.insert("integrity", Feature::yes());
    caps.features.insert("settings", Feature::yes());
    caps.features
        .insert("settingsEdit", Feature::no("the server's variables are read only here"));
    caps.features
        .insert("backups", Feature::no("backups of this server are its provider's"));
    caps
}

/// The privileges `SHOW GRANTS` gives on `*.*`, read from the list before ` ON `, so a user or host name such as
/// `superset` is never taken for one. A grant of roles (no ` ON `) is noted: what the roles give is not read here.
#[derive(Debug, Default)]
pub(crate) struct GlobalGrants {
    privileges: Vec<String>,
    pub(crate) through_roles: bool,
}

impl GlobalGrants {
    pub(crate) fn parse(grants: &[String]) -> Self {
        let mut out = Self::default();
        for grant in grants {
            let upper = grant.trim().to_ascii_uppercase();
            let Some(rest) = upper.strip_prefix("GRANT ") else {
                continue;
            };
            let Some(on) = rest.find(" ON ") else {
                out.through_roles = true;
                continue;
            };
            let object = rest[on + 4..].trim_start();
            if !(object.starts_with("*.*") || object.starts_with("`*`.`*`")) {
                continue;
            }
            out.privileges
                .extend(rest[..on].split(',').map(|privilege| privilege.trim().to_string()));
        }
        out
    }

    pub(crate) fn has(&self, privilege: &str) -> bool {
        self.privileges
            .iter()
            .any(|held| held == privilege || held == "ALL" || held == "ALL PRIVILEGES")
    }
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The capabilities of `group`, from the cache when probed in the last [`CACHE_FOR`] ([`FAILED_CACHE_FOR`] when the
/// database did not answer) and `refresh` is false. The cache is per facade (each factory's entity facade, with its
/// own datasources) and group. `embedded` is asked only when the group is probed.
pub fn group_capabilities(
    facade: &EntityFacade,
    group: &str,
    embedded: impl FnOnce() -> bool,
    refresh: bool,
) -> Arc<Capabilities> {
    let identity = facade.identity().as_ptr().cast::<()>() as usize;
    let key = format!("{identity:x}/group:{group}");
    if !refresh {
        if let Some((at, caps)) = lock(cache()).get(&key) {
            let keep = if caps.error.is_some() {
                FAILED_CACHE_FOR
            } else {
                CACHE_FOR
            };
            if at.elapsed() < keep {
                return Arc::clone(caps);
            }
        }
    }
    let caps = Arc::new(probe_group(facade, group, embedded()));
    let mut cache = lock(cache());
    // a facade that is gone leaves its rows: drop the expired ones as new ones come
    cache.retain(|_, (at, _)| at.elapsed() < CACHE_FOR);
    cache.insert(key, (Instant::now(), Arc::clone(&caps)));
    caps
}

/// Forget every cached answer.
pub fn clear_cache() {
    lock(cache()).clear();
}
