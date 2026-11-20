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

//! `create#Database` and its one-shot: a Postgres user is made with a SCRAM verifier (the RFC 7677 example checks it),
//! the one-shot's command and entrypoint hold no login, a database host is reached with `verify-full` and a user or
//! database there already is refused, and the identifier and quote checks of the old scripts hold.

use base64::Engine as _;
use hmac::{KeyInit as _, Mac as _};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::database::{self, AdminLogin, Direct, Server};

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(key).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// RFC 7677 section 3: the verifier of `pencil` with the RFC's salt gives the RFC's server signature, and the RFC's
/// client proof proves its stored key.
#[test]
fn a_scram_verifier_is_the_rfc_7677_one() {
    let b64 = base64::engine::general_purpose::STANDARD;
    let salt = b64.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
    let verifier = database::scram_verifier_with("pencil", &salt, 4096);
    let (head, keys) = verifier.split_once("$4096:").unwrap();
    assert_eq!(head, "SCRAM-SHA-256");
    let (salt_text, keys) = keys.split_once('$').unwrap();
    assert_eq!(salt_text, "W22ZaJ0SNY7soEsUEjb6gQ==");
    let (stored, server) = keys.split_once(':').unwrap();
    let (stored, server) = (b64.decode(stored).unwrap(), b64.decode(server).unwrap());
    let auth = "n=user,r=rOprNGfwEbeRWgbNEkqO,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,\
                s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096,c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";
    assert_eq!(
        b64.encode(hmac(&server, auth.as_bytes())),
        "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="
    );
    let proof = b64.decode("dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=").unwrap();
    let signature = hmac(&stored, auth.as_bytes());
    let client_key: Vec<u8> = proof.iter().zip(&signature).map(|(a, b)| a ^ b).collect();
    assert_eq!(Sha256::digest(&client_key).to_vec(), stored);
    // a fresh salt each time
    assert_ne!(database::scram_verifier("pencil"), database::scram_verifier("pencil"));
}

fn admin(password: &str) -> AdminLogin {
    AdminLogin {
        user: "postgres".to_string(),
        password: Zeroizing::new(password.to_string()),
    }
}

/// The one-shot's command and entrypoint name files only; the logins and the statements are its secret files, and the
/// Postgres statement holds the verifier, not the password.
#[test]
fn a_one_shot_holds_its_logins_in_files_only() {
    let server = Server {
        conf_name: "postgres",
        host: "pg-main",
        port: "5432",
    };
    let shot = database::one_shot_make(server, &admin("the-admin:pass"), "app", "app", "TheUserPassword123").unwrap();
    let words = format!("{:?} {:?}", shot.entrypoint, shot.cmd);
    assert!(
        !words.contains("the-admin") && !words.contains("TheUserPassword123"),
        "{words}"
    );
    let file =
        |name: &str| String::from_utf8(shot.files.iter().find(|(file, _)| file == name).unwrap().1.to_vec()).unwrap();
    assert_eq!(file("pgpass"), "pg-main:5432:*:postgres:the-admin\\:pass\n");
    let statements = file("make.sql");
    assert!(
        statements.contains("CREATE USER app WITH PASSWORD 'SCRAM-SHA-256$4096:"),
        "{statements}"
    );
    assert!(statements.contains("CREATE DATABASE app OWNER app"), "{statements}");
    assert!(!statements.contains("TheUserPassword123"));
    let script = file("database.sh");
    assert!(!script.contains("the-admin") && !script.contains("TheUserPassword123"));
    assert!(script.contains("e=3") && script.contains("exit $e"), "{script}");
    // MySQL: an option file, and a password with a quote refused (SEC-TLS-10)
    let mysql = Server {
        conf_name: "mysql",
        host: "my-main",
        port: "3306",
    };
    let shot = database::one_shot_make(mysql, &admin("root\"pass"), "app", "app", "Plain123").unwrap();
    let cnf = String::from_utf8(shot.files[0].1.to_vec()).unwrap();
    assert_eq!(cnf, "[client]\nuser=\"postgres\"\npassword=\"root\\\"pass\"\n");
    let refused = database::one_shot_make(mysql, &admin("x"), "app", "app", "a'b").unwrap_err();
    assert!(refused.message().contains("quote or a backslash"), "{refused}");
    let refused = database::one_shot_make(server, &admin("x"), "x; DROP TABLE y", "u", "p").unwrap_err();
    assert!(refused.message().contains("not a plain identifier"), "{refused}");
    let refused = database::one_shot_make(
        Server {
            host: "pg; rm -rf /",
            ..server
        },
        &admin("x"),
        "app",
        "app",
        "p",
    )
    .unwrap_err();
    assert!(refused.message().contains("not a plain name"), "{refused}");
}

/// A database host is reached with TLS that checks its certificate and name; a user or database there already is
/// refused and named, and nothing is made.
#[test]
fn a_database_host_is_reached_over_verified_tls_and_an_existing_user_is_refused() {
    let (_ctx, _facade) = super::setup();
    let login = admin("admin-pass");
    let direct = |host: &'static str| Direct {
        conf_name: "postgres",
        host,
        port: "5432",
        admin: &login,
    };
    database::create_direct(&direct("db-free.test"), "tenant_a", "tenant_user", "Secret123").unwrap();
    let recorded = super::recorded_for("db-free.test:5432");
    assert_eq!(recorded.ssl_mode, "verify-full");
    assert_eq!(
        (recorded.user.as_str(), recorded.password.as_str()),
        ("postgres", "admin-pass")
    );
    assert!(recorded
        .sql
        .iter()
        .any(|sql| sql.starts_with("CREATE USER tenant_user WITH PASSWORD 'SCRAM-SHA-256$")));
    assert!(
        recorded
            .sql
            .contains(&"CREATE DATABASE tenant_a OWNER tenant_user".to_string()),
        "{recorded:?}"
    );
    assert!(!recorded.sql.iter().any(|sql| sql.contains("Secret123")));

    let refused =
        database::create_direct(&direct("db-taken.test"), "tenant_a", "tenant_user", "Secret123").unwrap_err();
    assert!(refused.message().contains("user tenant_user exists"), "{refused}");
    assert!(refused.message().contains("database tenant_a exists"), "{refused}");
    assert!(refused.message().contains("databaseExists"), "{refused}");
    let recorded = super::recorded_for("db-taken.test:5432");
    assert!(
        !recorded.sql.iter().any(|sql| sql.starts_with("CREATE")),
        "{recorded:?}"
    );
}
