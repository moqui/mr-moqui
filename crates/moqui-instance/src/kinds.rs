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

//! The instance kinds (plans/Vault-Instances.md §4.1, §4.2): `mr-moqui`, `postgres`, `mysql` and `opensearch`. A kind's
//! rows (`InstanceKind` with its env, volume, host config, secret and dependency rows, framework/data/
//! InstanceTypeData.xml) say what its container is made of; this module turns an instance's rows into the container
//! that stays ([`ContainerSpec`]), with each secret a file under `/run/secrets` and the image's `*_FILE` setting naming
//! it, and does what a database kind does on a server: `create#Database` and `check#Database`, with the leased or
//! just-made admin login, keeping the identifier and quote checks of the scripts they replace.
//!
//! OpenSearch reads no `*_FILE` setting, so its entrypoint is wrapped: a shell reads the admin password from its file
//! into the setting the image's demo security configuration takes and starts the image's entrypoint. The value is in
//! no container's environment, labels or command (`docker inspect` shows none), though the OpenSearch process is
//! started with it in its own environment, as the image requires.

use std::collections::BTreeMap;
use std::time::Duration;

use moqui_core::driver::{conf_driver, ConfDb, DbConnection};
use moqui_lib::docker::{
    ContainerSpec, HealthCheck, Mount, MountKind, PortBinding, RestartPolicy, SecretFile, SECRETS_DIR,
};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::to_plain_string;
use zeroize::Zeroizing;

use crate::common::fail;

/// The file an mr-moqui instance's one-time pairing token is given as (§4.2, §4.4); the manager link (M2) reads it.
pub const PAIRING_TOKEN_FILE: &str = "moqui-pairing-token";
/// How long a pairing token works (§4.2): 15 minutes.
pub const PAIRING_TOKEN_MINUTES: i64 = 15;
/// The placeholders a kind's env and volume templates may hold (`${name}`); nothing else is expanded.
pub const PLACEHOLDERS: &[&str] = &[
    "appInstanceId",
    "instanceName",
    "hostName",
    "databaseConf",
    "databaseHost",
    "databasePort",
    "databaseName",
    "databaseUser",
    "searchBackend",
    "searchUrl",
    "secretsDir",
];

/// Fills a template's `${name}` placeholders from `values`; a placeholder that is not one of [`PLACEHOLDERS`] is
/// refused, so no template runs anything (the Java templates were Groovy).
pub fn expand(template: &str, values: &BTreeMap<&str, String>) -> Result<String, LibError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, after)) = rest.split_once("${") {
        out.push_str(before);
        let Some((name, tail)) = after.split_once('}') else {
            return Err(fail(format!("The template {template:?} has a ${{ with no }}")));
        };
        if !PLACEHOLDERS.contains(&name) {
            return Err(fail(format!(
                "The template {template:?} names ${{{name}}}, which is not one of {}",
                PLACEHOLDERS.join(", ")
            )));
        }
        out.push_str(values.get(name).map_or("", String::as_str));
        rest = tail;
    }
    out.push_str(rest);
    Ok(out)
}

/// A credential for a secret of `kind`: [`crate::common::new_credential`], with what OpenSearch's password rule asks
/// beside (an upper and a lower case letter, a digit and another character).
pub fn credential_for(kind: &str) -> Zeroizing<String> {
    let mut credential = crate::common::new_credential();
    if kind == "opensearch" {
        credential.push_str("-aA1");
    }
    credential
}

/// What an instance's container is made of: its rows, read by the provision.
#[derive(Debug, Default)]
pub struct ContainerInput {
    pub kind: String,
    pub name: String,
    pub image: String,
    pub pull: bool,
    pub env: Vec<(String, String)>,
    /// Each mount point with its volume's name.
    pub volumes: Vec<(String, String)>,
    /// The host config rows, by name: RestartPolicy, HealthCmd, User.
    pub host_config: BTreeMap<String, String>,
    pub labels: Vec<(String, String)>,
    pub network: Option<String>,
    pub container_port: Option<u16>,
    pub host_port: Option<u16>,
    /// The host's address a published port listens on (loopback for a host that is this machine).
    pub host_ip: Option<String>,
    /// `uid:gid` the secret files are owned by.
    pub run_as: Option<String>,
    /// The secret files: each file's name, the image's setting that names it (if any), and its value.
    pub secrets: Vec<(String, Option<String>, Zeroizing<Vec<u8>>)>,
}

/// The container that stays for `input`: settings that are not secret, each secret a file owned by the kind's user
/// (0400) and named by its `*_FILE` setting, the volumes, the environment's network, the published port, the restart
/// policy and health check. [`ContainerSpec::check`] refuses it, before anything is sent, when a secret's value is in
/// its environment, labels, command or entrypoint.
pub fn container_spec(input: ContainerInput) -> Result<ContainerSpec, LibError> {
    let (uid, gid) = match input.run_as.as_deref().and_then(|run_as| run_as.split_once(':')) {
        Some((uid, gid)) => (
            uid.trim()
                .parse()
                .map_err(|_| fail(format!("runAsUser {uid}:{gid} is not uid:gid")))?,
            gid.trim()
                .parse()
                .map_err(|_| fail(format!("runAsUser {uid}:{gid} is not uid:gid")))?,
        ),
        None => (0, 0),
    };
    let mut env = input.env;
    let mut secrets = Vec::with_capacity(input.secrets.len());
    for (file, setting, bytes) in input.secrets {
        let path = format!("{SECRETS_DIR}/{file}");
        if let Some(setting) = setting.filter(|setting| !setting.is_empty()) {
            env.retain(|(name, _)| *name != setting);
            env.push((setting, path));
        }
        secrets.push(SecretFile::new(file, bytes, uid, gid));
    }
    let mounts = input
        .volumes
        .into_iter()
        .map(|(target, source)| Mount {
            kind: MountKind::Volume,
            source,
            target,
            read_only: false,
        })
        .collect();
    let restart = match input.host_config.get("RestartPolicy").map(String::as_str) {
        None | Some("" | "no") => RestartPolicy::No,
        Some("always") => RestartPolicy::Always,
        Some("unless-stopped") => RestartPolicy::UnlessStopped,
        Some("on-failure") => RestartPolicy::OnFailure(5),
        Some(other) => {
            return Err(fail(format!(
                "RestartPolicy {other} is not no, always, unless-stopped or on-failure"
            )))
        }
    };
    let health = input
        .host_config
        .get("HealthCmd")
        .filter(|command| !command.trim().is_empty())
        .map(|command| HealthCheck {
            test: vec!["CMD-SHELL".to_string(), command.clone()],
            interval: Some(Duration::from_secs(10)),
            timeout: Some(Duration::from_secs(5)),
            retries: Some(12),
            start_period: Some(Duration::from_secs(30)),
        });
    let ports = match (input.container_port, input.host_port) {
        (Some(container_port), Some(host_port)) => vec![PortBinding {
            container_port,
            protocol: "tcp".to_string(),
            host_port: Some(host_port),
            host_ip: input.host_ip,
        }],
        _ => Vec::new(),
    };
    let entrypoint = if input.kind == "opensearch" {
        opensearch_entrypoint(&secrets)
    } else {
        Vec::new()
    };
    Ok(ContainerSpec {
        name: input.name,
        image: input.image,
        pull: input.pull,
        entrypoint,
        env,
        labels: input.labels,
        mounts,
        network: input.network,
        ports,
        restart,
        health,
        user: input.host_config.get("User").filter(|user| !user.is_empty()).cloned(),
        secrets,
        ..ContainerSpec::default()
    })
}

/// OpenSearch's wrapped entrypoint: its admin password read from its file into `OPENSEARCH_INITIAL_ADMIN_PASSWORD`,
/// then the image's own entrypoint. The command names the file, never the value.
fn opensearch_entrypoint(secrets: &[SecretFile]) -> Vec<String> {
    let Some(file) = secrets.iter().find(|secret| secret.name == "opensearch-admin-password") else {
        return Vec::new();
    };
    vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        format!(
            "OPENSEARCH_INITIAL_ADMIN_PASSWORD=\"$(cat {})\"; export OPENSEARCH_INITIAL_ADMIN_PASSWORD; \
             exec /usr/share/opensearch/opensearch-docker-entrypoint.sh opensearch",
            file.path()
        ),
    ]
}

// ----- create#Database and check#Database -----

/// A database server's admin login: leased (a database host's, `DatabaseHost:<id>:admin`) or made by this provision (a
/// new Postgres or MySQL instance's). Its `Debug` shows no password.
pub struct AdminLogin {
    /// The database-list name of the kind: postgres or mysql.
    pub conf_name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: Zeroizing<String>,
    /// `disable` for a new instance on the environment's network, which serves no TLS; empty (verified TLS) for a
    /// database host.
    pub ssl_mode: String,
}

impl std::fmt::Debug for AdminLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminLogin")
            .field("conf_name", &self.conf_name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// What `create#Database` did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseMade {
    pub database_created: bool,
    pub user_created: bool,
}

/// What `check#Database` found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseFound {
    pub connected: bool,
    pub database_exists: bool,
    pub user_exists: bool,
}

/// A database or user name that goes into SQL: letters, digits and `_` only (the scripts' check).
pub fn identifier<'a>(name: &'a str, what: &str) -> Result<&'a str, LibError> {
    if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        Ok(name)
    } else {
        Err(fail(format!("The {what} name [{name}] is not a plain identifier")))
    }
}

/// A Postgres string literal's body: quotes doubled (standard_conforming_strings, on since 9.1).
fn pg_quoted(text: &str) -> Zeroizing<String> {
    Zeroizing::new(text.replace('\'', "''"))
}

/// A MySQL password literal's body. MySQL's default mode reads a backslash as an escape, so doubling quotes is not
/// enough: a password with a quote or a backslash is refused (SEC-TLS-10).
fn mysql_password(text: &str) -> Result<&str, LibError> {
    if text.contains(['\'', '"', '\\']) {
        return Err(fail("The database password may not hold a quote or a backslash"));
    }
    Ok(text)
}

fn connect(admin: &AdminLogin, database: &str) -> Result<Box<dyn DbConnection>, LibError> {
    let conf = ConfDb {
        db_type: admin.conf_name.clone(),
        host: admin.host.clone(),
        port: admin.port.clone(),
        database: database.to_string(),
        user: admin.user.clone(),
        password: admin.password.to_string(),
        ssl_mode: admin.ssl_mode.clone(),
        ..ConfDb::default()
    };
    conf_driver(&conf).and_then(|driver| driver.connect())
}

fn count(conn: &mut dyn DbConnection, sql: &str) -> Result<i64, LibError> {
    let rows = conn.query_once(sql, &[])?;
    Ok(rows
        .first()
        .and_then(|row| row.columns.first())
        .map(|(_, value)| to_plain_string(value))
        .and_then(|text| text.trim().parse::<f64>().ok())
        .map_or(0, |count| if count > 0.0 { 1 } else { 0 }))
}

/// The database a kind's admin connects to: Postgres's `postgres`, MySQL's `mysql`.
fn admin_database(conf_name: &str) -> Result<&'static str, LibError> {
    match conf_name {
        "postgres" => Ok("postgres"),
        "mysql" | "mysql8" => Ok("mysql"),
        other => Err(fail(format!("No database kind {other}: postgres or mysql"))),
    }
}

/// Waits up to `wait` for the server to take a connection with the admin login: a new instance's database starts
/// after its container does.
pub fn wait_ready(admin: &AdminLogin, wait: Duration) -> Result<(), LibError> {
    let database = admin_database(&admin.conf_name)?;
    let until = std::time::Instant::now() + wait;
    loop {
        match connect(admin, database) {
            Ok(_) => return Ok(()),
            Err(err) if std::time::Instant::now() >= until => {
                return Err(fail(format!(
                    "The database server at {}:{} took no connection in {}s: {}",
                    admin.host,
                    admin.port,
                    wait.as_secs(),
                    err.message()
                )))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// `create#Database`: the database `database` and the user `user` with `password`, made with the admin login when
/// they are not there; the user owns a new database (Postgres) or has every privilege on it (MySQL). Names are plain
/// identifiers, and a password is quoted as each server reads it.
pub fn create_database(
    admin: &AdminLogin,
    database: &str,
    user: &str,
    password: &str,
) -> Result<DatabaseMade, LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let mut conn = connect(admin, admin_database(&admin.conf_name)?)?;
    let mut made = DatabaseMade::default();
    match admin.conf_name.as_str() {
        "postgres" => {
            if count(
                conn.as_mut(),
                &format!("SELECT COUNT(*) AS row_count FROM pg_catalog.pg_user WHERE usename='{user}'"),
            )? == 0
            {
                let sql = Zeroizing::new(format!(
                    "CREATE USER {user} WITH PASSWORD '{}'",
                    pg_quoted(password).as_str()
                ));
                conn.execute(&sql, &[])?;
                made.user_created = true;
            }
            if count(
                conn.as_mut(),
                &format!("SELECT COUNT(*) AS row_count FROM pg_database WHERE datname='{database}'"),
            )? == 0
            {
                // Postgres reports no rows for CREATE DATABASE, so a statement that did not fail created it; the owner
                // may create in its public schema, which GRANT on the database alone no longer gives (Postgres 15)
                conn.execute(&format!("CREATE DATABASE {database} OWNER {user}"), &[])?;
                made.database_created = true;
            } else {
                conn.execute(&format!("GRANT ALL PRIVILEGES ON DATABASE {database} TO {user}"), &[])?;
            }
        }
        _ => {
            let password = mysql_password(password)?;
            if count(
                conn.as_mut(),
                &format!(
                    "SELECT COUNT(*) AS row_count FROM information_schema.schemata WHERE schema_name='{database}'"
                ),
            )? == 0
            {
                conn.execute(
                    &format!("CREATE DATABASE `{database}` DEFAULT CHARACTER SET utf8mb4"),
                    &[],
                )?;
                made.database_created = true;
            }
            if count(
                conn.as_mut(),
                &format!("SELECT COUNT(*) AS row_count FROM mysql.user WHERE user='{user}'"),
            )? == 0
            {
                let sql = Zeroizing::new(format!("CREATE USER '{user}'@'%' IDENTIFIED BY '{password}'"));
                conn.execute(&sql, &[])?;
                made.user_created = true;
            }
            conn.execute(&format!("GRANT ALL ON `{database}`.* TO '{user}'@'%'"), &[])?;
        }
    }
    Ok(made)
}

/// `check#Database`: whether the admin login connects, and whether the database and the user are there.
pub fn check_database(admin: &AdminLogin, database: &str, user: &str) -> Result<DatabaseFound, LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let mut conn = match connect(admin, admin_database(&admin.conf_name)?) {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!(host = %admin.host, "Test connection to the database server failed: {}", err.message());
            return Ok(DatabaseFound::default());
        }
    };
    let (databases, users) = match admin.conf_name.as_str() {
        "postgres" => (
            format!("SELECT COUNT(*) AS row_count FROM pg_database WHERE datname='{database}'"),
            format!("SELECT COUNT(*) AS row_count FROM pg_catalog.pg_user WHERE usename='{user}'"),
        ),
        _ => (
            format!("SELECT COUNT(*) AS row_count FROM information_schema.schemata WHERE schema_name='{database}'"),
            format!("SELECT COUNT(*) AS row_count FROM mysql.user WHERE user='{user}'"),
        ),
    };
    Ok(DatabaseFound {
        connected: true,
        database_exists: count(conn.as_mut(), &databases)? > 0,
        user_exists: count(conn.as_mut(), &users)? > 0,
    })
}
