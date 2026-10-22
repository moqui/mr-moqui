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

//! A Postgres server this process did not start, over TCP and TLS as `sslmode` says, which the embedded cluster (a
//! Unix socket, no TLS) cannot show: a query, a statement timeout that fires, and a cancel sent over its own TLS
//! connection. Ignored unless `PG_EXTERNAL_URL` names the server (`postgresql://host:port/database?sslmode=...` and
//! `&sslrootcert=...`), with `PG_EXTERNAL_USER` and `PG_EXTERNAL_PASSWORD`. The embedded cluster tests in
//! `db/embedded.rs` cover a session the server ends mid-transaction, mid-pipeline and mid-stream.

#![cfg(feature = "postgres")]

use std::time::Duration;

use moqui_core::driver::DbDriver;
use moqui_host::db::pg_tls::TlsOptions;
use moqui_lib::object::Value;

#[test]
#[ignore = "needs a server: PG_EXTERNAL_URL='postgresql://host:5432/db?sslmode=verify-full&sslrootcert=ca.crt' cargo test -p moqui-host --test it -- pg_external --ignored"]
fn an_external_server_times_out_and_cancels_over_its_transport() {
    let url = std::env::var("PG_EXTERNAL_URL").expect("set PG_EXTERNAL_URL");
    let parsed = moqui_core::search::conf::parse_postgres_url(&url).unwrap();
    let user = std::env::var("PG_EXTERNAL_USER").unwrap_or_else(|_| "postgres".into());
    let password = std::env::var("PG_EXTERNAL_PASSWORD").ok();
    let tls = parsed
        .tls
        .then(|| TlsOptions::new(parsed.tls_verify, &parsed.tls_root_cert));
    let driver = moqui_host::db::entity_db::postgres_server_driver(
        &user,
        password.as_deref(),
        &parsed.database,
        &parsed.host,
        parsed.port,
        tls.as_ref(),
    )
    .unwrap();
    let mut conn = driver.connect().unwrap();
    let ssl = conn
        .query_once("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()", &[])
        .unwrap();
    assert_eq!(
        ssl[0].get("ssl"),
        Some(&Value::Bool(tls.is_some())),
        "TLS as sslmode says"
    );

    conn.begin().unwrap();
    conn.set_statement_timeout(Some(Duration::from_millis(50))).unwrap();
    let err = conn.query_once("SELECT pg_sleep(5)", &[]).unwrap_err();
    assert!(err.message().contains("statement timeout"), "{err}");
    conn.rollback().unwrap();

    let canceller = conn.canceller();
    let cancelling = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        canceller.cancel()
    });
    let err = conn.query_once("SELECT pg_sleep(5)", &[]).unwrap_err();
    cancelling.join().unwrap().expect("cancel request");
    assert!(err.message().contains("canceling statement"), "{err}");
    assert_eq!(conn.query_once("SELECT 1", &[]).unwrap().len(), 1);
    assert!(!conn.is_closed());
}
