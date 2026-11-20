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

//! What a database kind does on a server (plans/Vault-Instances.md §4.1, §4.3 step 4.2): `create#Database`, its undo,
//! and `check#Database`, for Postgres and MySQL.
//!
//! - **A database instance** is reached only on its own network: a one-shot client container of the server's image
//!   runs on its host and network, reads the admin login and the statements from secret files under `/run/secrets`
//!   ([`one_shot_make`]), and makes the user and database there. No login crosses another network, the server
//!   publishes no port for the manager, and nothing secret is in the container's command or environment.
//! - **A database host** (not managed here) is reached by the manager over TLS that checks the server's certificate and
//!   name (`verify-full`, [`direct`]); the drivers send no password in clear text over a connection without TLS.
//!
//! Either way a user or database that is there already is refused, and the report names what was found: reusing one
//! would leave the vault's item wrong (its password not the user's) or grant a new user a database that holds data.
//! A Postgres user is made with a SCRAM-SHA-256 verifier ([`scram_verifier`]), so the password itself is in no statement
//! and no server log. Names are plain identifiers; a MySQL password with a quote or a backslash is refused (SEC-TLS-10).

use std::time::Duration;

use base64::Engine as _;
use hmac::{KeyInit as _, Mac as _};
use moqui_core::driver::{conf_driver, ConfDb, DbConnection};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::to_plain_string;
use moqui_vault::rng::{OsRng, Rng};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::common::fail;

/// The iterations of a SCRAM-SHA-256 verifier: Postgres's default.
const SCRAM_ITERATIONS: u32 = 4096;
/// The exit of a one-shot whose user or database is there already.
pub const EXIT_EXISTS: i64 = 3;

/// A database server's admin login: leased (a database host's, `DatabaseHost:<id>:admin`) or made by this provision (a
/// new Postgres or MySQL instance's). Its `Debug` shows no password.
pub struct AdminLogin {
    pub user: String,
    pub password: Zeroizing<String>,
}

impl std::fmt::Debug for AdminLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminLogin")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// What `check#Database` found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseFound {
    pub connected: bool,
    pub database_exists: bool,
    pub user_exists: bool,
}

/// A database or user name that goes into SQL or a script: letters, digits and `_` only (the scripts' check).
pub fn identifier<'a>(name: &'a str, what: &str) -> Result<&'a str, LibError> {
    if !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        Ok(name)
    } else {
        Err(fail(format!("The {what} name [{name}] is not a plain identifier")))
    }
}

/// A host name, container name or user name that goes into a one-shot's script: letters, digits, `.`, `_` and `-`.
fn plain_word<'a>(text: &'a str, what: &str) -> Result<&'a str, LibError> {
    if !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        Ok(text)
    } else {
        Err(fail(format!("The {what} [{text}] is not a plain name")))
    }
}

/// A MySQL password literal's body. MySQL's default mode reads a backslash as an escape, so doubling quotes is not
/// enough: a password with a quote or a backslash is refused (SEC-TLS-10).
fn mysql_password(text: &str) -> Result<&str, LibError> {
    if text.contains(['\'', '"', '\\']) {
        return Err(fail("The database password may not hold a quote or a backslash"));
    }
    Ok(text)
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    // HMAC takes a key of any length
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// The SCRAM-SHA-256 verifier Postgres stores for `password` (RFC 5802, RFC 7677):
/// `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>`, base64. `CREATE USER ... PASSWORD '<verifier>'` sets it
/// with the password in no statement. The password is letters and digits (SASLprep leaves it as it is).
pub fn scram_verifier(password: &str) -> String {
    let mut salt = [0_u8; 16];
    OsRng.fill(&mut salt);
    scram_verifier_with(password, &salt, SCRAM_ITERATIONS)
}

/// [`scram_verifier`] with a given salt and iteration count (for its test vector).
pub fn scram_verifier_with(password: &str, salt: &[u8], iterations: u32) -> String {
    // PBKDF2-HMAC-SHA256 of one block: the hash is as long as the key
    let mut block = salt.to_vec();
    block.extend_from_slice(&1_u32.to_be_bytes());
    let mut u = Zeroizing::new(hmac_sha256(password.as_bytes(), &block));
    let mut salted = Zeroizing::new(*u);
    for _ in 1..iterations {
        *u = hmac_sha256(password.as_bytes(), &u[..]);
        for (out, byte) in salted.iter_mut().zip(u.iter()) {
            *out ^= byte;
        }
    }
    let client_key = Zeroizing::new(hmac_sha256(&salted[..], b"Client Key"));
    let stored_key = Sha256::digest(&client_key[..]);
    let server_key = hmac_sha256(&salted[..], b"Server Key");
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    format!(
        "SCRAM-SHA-256${iterations}:{}${}:{}",
        b64(salt),
        b64(&stored_key),
        b64(&server_key)
    )
}

/// A `.pgpass` field: `\` and `:` escaped.
fn pgpass_field(text: &str) -> String {
    text.replace('\\', "\\\\").replace(':', "\\:")
}

/// A MySQL option file value, double-quoted: `\` and `"` escaped.
fn option_value(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What a one-shot client container is given: its entrypoint and command (no secret in them) and its secret files.
pub struct OneShot {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub files: Vec<(String, Zeroizing<Vec<u8>>)>,
}

impl std::fmt::Debug for OneShot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.files.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("OneShot")
            .field("cmd", &self.cmd)
            .field("files", &names)
            .finish_non_exhaustive()
    }
}

/// Where a one-shot reaches the server: its name on the network and its port.
#[derive(Debug, Clone, Copy)]
pub struct Server<'a> {
    pub conf_name: &'a str,
    pub host: &'a str,
    pub port: &'a str,
}

/// The shell lines that wait for the server and define `q`, the admin's client, for a script.
fn client_lines(server: Server, admin_user: &str) -> Result<String, LibError> {
    let host = plain_word(server.host, "server")?;
    let port = server
        .port
        .parse::<u16>()
        .map_err(|_| fail(format!("{} is not a port", server.port)))?;
    let admin = plain_word(admin_user, "admin user")?;
    Ok(match server.conf_name {
        "postgres" => format!(
            "export PGPASSFILE=/run/secrets/pgpass\n\
             i=0\n\
             until pg_isready -q -h {host} -p {port} -U {admin}; do i=$((i+1)); \
             if [ $i -ge 120 ]; then echo 'moqui: the database server took no connection'; exit 4; fi; sleep 1; done\n\
             q() {{ psql -h {host} -p {port} -U {admin} -d postgres -v ON_ERROR_STOP=1 -tAq \"$@\"; }}\n"
        ),
        "mysql" | "mysql8" => format!(
            "q() {{ mysql --defaults-extra-file=/run/secrets/admin.cnf -h {host} -P {port} -N \"$@\"; }}\n\
             i=0\n\
             until q -e 'SELECT 1' >/dev/null 2>&1; do i=$((i+1)); \
             if [ $i -ge 120 ]; then echo 'moqui: the database server took no connection'; exit 4; fi; sleep 1; done\n"
        ),
        other => return Err(fail(format!("No database kind {other}: postgres or mysql"))),
    })
}

/// The admin login's file: `.pgpass` for Postgres, an option file for MySQL.
fn admin_file(server: Server, admin: &AdminLogin) -> (String, Zeroizing<Vec<u8>>) {
    if server.conf_name == "postgres" {
        let line = Zeroizing::new(format!(
            "{}:{}:*:{}:{}\n",
            pgpass_field(server.host),
            server.port,
            pgpass_field(&admin.user),
            pgpass_field(&admin.password)
        ));
        ("pgpass".to_string(), Zeroizing::new(line.as_bytes().to_vec()))
    } else {
        let text = Zeroizing::new(format!(
            "[client]\nuser={}\npassword={}\n",
            option_value(&admin.user),
            option_value(&admin.password)
        ));
        ("admin.cnf".to_string(), Zeroizing::new(text.as_bytes().to_vec()))
    }
}

/// The statements that make the user and the database.
fn make_statements(conf_name: &str, database: &str, user: &str, password: &str) -> Result<Zeroizing<String>, LibError> {
    Ok(Zeroizing::new(if conf_name == "postgres" {
        format!(
            "CREATE USER {user} WITH PASSWORD '{}';\nCREATE DATABASE {database} OWNER {user};\n",
            scram_verifier(password)
        )
    } else {
        format!(
            "CREATE DATABASE `{database}` DEFAULT CHARACTER SET utf8mb4;\n\
             CREATE USER '{user}'@'%' IDENTIFIED BY '{}';\n\
             GRANT ALL ON `{database}`.* TO '{user}'@'%';\n",
            mysql_password(password)?
        )
    }))
}

/// The statements that undo [`make_statements`].
fn drop_statements(conf_name: &str, database: &str, user: &str) -> String {
    if conf_name == "postgres" {
        format!("DROP DATABASE IF EXISTS {database};\nDROP USER IF EXISTS {user};\n")
    } else {
        format!("DROP DATABASE IF EXISTS `{database}`;\nDROP USER IF EXISTS '{user}'@'%';\n")
    }
}

/// The count queries of a user and a database, as the script runs them.
fn exists_queries(conf_name: &str, database: &str, user: &str) -> (String, String) {
    if conf_name == "postgres" {
        (
            format!("SELECT count(*) FROM pg_roles WHERE rolname='{user}'"),
            format!("SELECT count(*) FROM pg_database WHERE datname='{database}'"),
        )
    } else {
        (
            format!("SELECT COUNT(*) FROM mysql.user WHERE user='{user}'"),
            format!("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name='{database}'"),
        )
    }
}

fn one_shot(script: String, mut files: Vec<(String, Zeroizing<Vec<u8>>)>) -> OneShot {
    files.push(("database.sh".to_string(), Zeroizing::new(script.into_bytes())));
    OneShot {
        entrypoint: vec!["/bin/sh".to_string()],
        cmd: vec!["/run/secrets/database.sh".to_string()],
        files,
    }
}

/// The one-shot that makes `user` (with `password`) and `database` on the server, refusing (exit [`EXIT_EXISTS`], a
/// `moqui:` line for each) when either is there. Its logs hold only the script's own `moqui:` lines: a client's error
/// output goes nowhere, since it could quote a statement.
pub fn one_shot_make(
    server: Server,
    admin: &AdminLogin,
    database: &str,
    user: &str,
    password: &str,
) -> Result<OneShot, LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let (users, databases) = exists_queries(server.conf_name, database, user);
    let source = if server.conf_name == "postgres" {
        "-f /run/secrets/make.sql"
    } else {
        "< /run/secrets/make.sql"
    };
    let flag = if server.conf_name == "postgres" { "-c" } else { "-e" };
    let script = format!(
        "set -u\n{}\
         u=$(q {flag} \"{users}\" 2>/dev/null) || {{ echo 'moqui: the admin login was refused'; exit 5; }}\n\
         d=$(q {flag} \"{databases}\" 2>/dev/null) || {{ echo 'moqui: the admin login was refused'; exit 5; }}\n\
         e=0\n\
         if [ \"$u\" != 0 ]; then echo 'moqui: user {user} exists'; e={EXIT_EXISTS}; fi\n\
         if [ \"$d\" != 0 ]; then echo 'moqui: database {database} exists'; e={EXIT_EXISTS}; fi\n\
         if [ $e != 0 ]; then exit $e; fi\n\
         q {source} >/dev/null 2>&1 || {{ echo 'moqui: making database {database} and user {user} failed'; exit 6; }}\n\
         echo 'moqui: made database {database} and user {user}'\n",
        client_lines(server, &admin.user)?
    );
    let statements = make_statements(server.conf_name, database, user, password)?;
    Ok(one_shot(
        script,
        vec![
            admin_file(server, admin),
            ("make.sql".to_string(), Zeroizing::new(statements.as_bytes().to_vec())),
        ],
    ))
}

/// The one-shot that drops what [`one_shot_make`] made, when the provision that made it failed after it.
pub fn one_shot_drop(server: Server, admin: &AdminLogin, database: &str, user: &str) -> Result<OneShot, LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let source = if server.conf_name == "postgres" {
        "-f /run/secrets/drop.sql"
    } else {
        "< /run/secrets/drop.sql"
    };
    let script = format!(
        "set -u\n{}\
         q {source} >/dev/null 2>&1 || {{ echo 'moqui: dropping database {database} and user {user} failed'; exit 6; }}\n\
         echo 'moqui: dropped database {database} and user {user}'\n",
        client_lines(server, &admin.user)?
    );
    Ok(one_shot(
        script,
        vec![
            admin_file(server, admin),
            (
                "drop.sql".to_string(),
                Zeroizing::new(drop_statements(server.conf_name, database, user).into_bytes()),
            ),
        ],
    ))
}

/// The one-shot that says whether `user` and `database` are there: `moqui: user <count>` and `moqui: database <count>`.
pub fn one_shot_check(server: Server, admin: &AdminLogin, database: &str, user: &str) -> Result<OneShot, LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let (users, databases) = exists_queries(server.conf_name, database, user);
    let flag = if server.conf_name == "postgres" { "-c" } else { "-e" };
    let script = format!(
        "set -u\n{}\
         u=$(q {flag} \"{users}\" 2>/dev/null) || {{ echo 'moqui: the admin login was refused'; exit 5; }}\n\
         d=$(q {flag} \"{databases}\" 2>/dev/null) || {{ echo 'moqui: the admin login was refused'; exit 5; }}\n\
         echo \"moqui: user $u\"\necho \"moqui: database $d\"\n",
        client_lines(server, &admin.user)?
    );
    Ok(one_shot(script, vec![admin_file(server, admin)]))
}

// ----- a database host, over TLS -----

/// A database host's connection: the leased admin login, over TLS that checks the server's certificate and its name.
pub struct Direct<'a> {
    pub conf_name: &'a str,
    pub host: &'a str,
    pub port: &'a str,
    pub admin: &'a AdminLogin,
}

fn connect(direct: &Direct, database: &str) -> Result<Box<dyn DbConnection>, LibError> {
    let conf = ConfDb {
        db_type: direct.conf_name.to_string(),
        host: direct.host.to_string(),
        port: direct.port.to_string(),
        database: database.to_string(),
        user: direct.admin.user.clone(),
        password: direct.admin.password.to_string(),
        // a database host is reached over the network: TLS that checks the certificate and the name, on both drivers
        ssl_mode: "verify-full".to_string(),
        ..ConfDb::default()
    };
    conf_driver(&conf).and_then(|driver| driver.connect())
}

fn count(conn: &mut dyn DbConnection, sql: &str) -> Result<bool, LibError> {
    let rows = conn.query_once(sql, &[])?;
    Ok(rows
        .first()
        .and_then(|row| row.columns.first())
        .map(|(_, value)| to_plain_string(value))
        .and_then(|text| text.trim().parse::<f64>().ok())
        .is_some_and(|count| count > 0.0))
}

/// The database the admin connects to: Postgres's `postgres`, MySQL's `mysql`.
fn admin_database(conf_name: &str) -> Result<&'static str, LibError> {
    match conf_name {
        "postgres" => Ok("postgres"),
        "mysql" | "mysql8" => Ok("mysql"),
        other => Err(fail(format!("No database kind {other}: postgres or mysql"))),
    }
}

/// `create#Database` on a database host: the user and the database, refused (naming what was found) when either is
/// there.
pub fn create_direct(direct: &Direct, database: &str, user: &str, password: &str) -> Result<(), LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let mut conn = connect(direct, admin_database(direct.conf_name)?)?;
    let (users, databases) = exists_queries(direct.conf_name, database, user);
    let mut found = Vec::new();
    if count(conn.as_mut(), &users)? {
        found.push(format!("user {user} exists"));
    }
    if count(conn.as_mut(), &databases)? {
        found.push(format!("database {database} exists"));
    }
    if !found.is_empty() {
        return Err(fail(format!(
            "On {}:{}: {}; a provision makes its own and does not take one that is there (reasonCode databaseExists)",
            direct.host,
            direct.port,
            found.join(", ")
        )));
    }
    let statements = make_statements(direct.conf_name, database, user, password)?;
    for statement in statements.split(";\n").filter(|statement| !statement.trim().is_empty()) {
        conn.execute(statement, &[])?;
    }
    Ok(())
}

/// Undoes [`create_direct`].
pub fn drop_direct(direct: &Direct, database: &str, user: &str) -> Result<(), LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let mut conn = connect(direct, admin_database(direct.conf_name)?)?;
    for statement in drop_statements(direct.conf_name, database, user).split(";\n") {
        if !statement.trim().is_empty() {
            conn.execute(statement, &[])?;
        }
    }
    Ok(())
}

/// `check#Database` on a database host: whether the admin login connects, and whether the database and user are there.
pub fn check_direct(direct: &Direct, database: &str, user: &str) -> Result<DatabaseFound, LibError> {
    let database = identifier(database, "database")?;
    let user = identifier(user, "user")?;
    let mut conn = match connect(direct, admin_database(direct.conf_name)?) {
        Ok(conn) => conn,
        Err(err) => {
            tracing::warn!(host = %direct.host, "Test connection to the database server failed: {}", err.message());
            return Ok(DatabaseFound::default());
        }
    };
    let (users, databases) = exists_queries(direct.conf_name, database, user);
    Ok(DatabaseFound {
        connected: true,
        database_exists: count(conn.as_mut(), &databases)?,
        user_exists: count(conn.as_mut(), &users)?,
    })
}

/// How long a one-shot may run: the server's start, then the statements.
pub const ONE_SHOT_WAIT: Duration = Duration::from_secs(180);
