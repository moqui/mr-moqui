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

use super::*;

fn conf(datasource: &str) -> MNode {
    let text = format!(
        r#"<moqui-conf>
                <entity-facade>{datasource}</entity-facade>
                <database-list>
                    <database name="postgres" default-test-query="SELECT 1">
                        <inline-jdbc pool-maxsize="90"><xa-properties serverName="db.example" portNumber="6543"
                            databaseName="moqui" user="moqui" password="pw"/></inline-jdbc>
                    </database>
                    <database name="mysql8" default-test-query="SELECT 2">
                        <inline-jdbc pool-maxsize="140"><xa-properties serverName="my.example" port="3307"
                            databaseName="moqui" user="u" password="p"/></inline-jdbc>
                    </database>
                    <database name="h2"/>
                    <database name="oracle"/>
                </database-list>
            </moqui-conf>"#
    );
    MNode::parse_text("test", &text).unwrap()
}

#[test]
fn h2_and_empty_are_embedded_with_the_postgres_pool() {
    for name in ["h2", ""] {
        let node = conf(&format!(
            r#"<datasource group-name="transactional" database-conf-name="{name}"/>"#
        ));
        let ds = datasource_conf(&node, "transactional").unwrap();
        assert_eq!(ds.kind, DatasourceKind::Embedded);
        assert_eq!(ds.dialect, "postgres");
        assert_eq!(ds.pool.max, 90);
        assert_eq!(ds.pool.min, 5);
        assert_eq!(ds.pool.reserve, 8);
        assert_eq!(ds.pool.acquire_timeout, Duration::from_secs(30));
        assert_eq!(ds.pool.idle_timeout, Some(Duration::from_secs(60)));
        assert_eq!(ds.pool.test_query.as_deref(), Some("SELECT 1"));
        assert!(ds.host.is_empty());
    }
}

#[test]
fn postgres_reads_the_database_xa_properties() {
    let node = conf(r#"<datasource group-name="transactional" database-conf-name="postgres" schema-name="public"/>"#);
    let ds = datasource_conf(&node, "transactional").unwrap();
    assert_eq!(ds.kind, DatasourceKind::Postgres);
    assert_eq!(
        (
            ds.host.as_str(),
            ds.port,
            ds.database.as_str(),
            ds.user.as_str(),
            ds.password.as_str()
        ),
        ("db.example", Some(6543), "moqui", "moqui", "pw")
    );
    assert_eq!(ds.schema, "public");
    assert!(!ds.startup_add_missing && ds.runtime_add_missing && ds.runtime_add_fks);
    assert!(ds.tls);
    assert_eq!((ds.tls_verify, ds.tls_root_cert.as_str()), (TlsVerify::Full, ""));
}

#[test]
fn sslmode_names_what_tls_checks() {
    assert_eq!(tls_verify(""), TlsVerify::Full);
    assert_eq!(tls_verify("prefer"), TlsVerify::Full);
    assert_eq!(tls_verify("verify-full"), TlsVerify::Full);
    assert_eq!(tls_verify("require"), TlsVerify::Encrypt);
    assert_eq!(tls_verify("REQUIRED"), TlsVerify::Encrypt);
    assert_eq!(tls_verify("verify-ca"), TlsVerify::Ca);
    assert_eq!(tls_verify("VERIFY_CA"), TlsVerify::Ca);
}

#[test]
fn the_datasource_inline_jdbc_wins_with_its_pool_and_uri() {
    let node = conf(
        r#"<datasource group-name="transactional" database-conf-name="postgres">
                <inline-jdbc jdbc-uri="jdbc:postgresql://127.0.0.1:5433/other?sslmode=disable" jdbc-username="a"
                    jdbc-password="b" pool-maxsize="12" pool-minsize="2" pool-reserve="3" pool-time-wait="0"
                    pool-time-idle="5" pool-test-query="SELECT 42"/>
            </datasource>"#,
    );
    let ds = datasource_conf(&node, "transactional").unwrap();
    assert_eq!(
        (ds.host.as_str(), ds.port, ds.database.as_str()),
        ("127.0.0.1", Some(5433), "other")
    );
    assert_eq!((ds.user.as_str(), ds.password.as_str()), ("a", "b"));
    let shown = format!("{ds:?}");
    assert!(shown.contains("<redacted>") && !shown.contains("\"b\""), "{shown}");
    assert!(!ds.tls);
    assert_eq!((ds.pool.min, ds.pool.max, ds.pool.reserve), (2, 12, 3));
    assert_eq!(ds.pool.acquire_timeout, Duration::ZERO);
    assert_eq!(ds.pool.idle_timeout, Some(Duration::from_secs(5)));
    assert_eq!(ds.pool.test_query.as_deref(), Some("SELECT 42"));
}

#[test]
fn jdbc_uri_parameters_with_a_driver_equivalent_are_read() {
    let node = conf(
        r#"<datasource group-name="transactional" database-conf-name="postgres">
                <inline-jdbc jdbc-uri="jdbc:postgresql://h:5432/db?ApplicationName=moqui&amp;currentSchema=app&amp;sslmode=disable"/>
            </datasource>"#,
    );
    let ds = datasource_conf(&node, "transactional").unwrap();
    assert_eq!((ds.schema.as_str(), ds.tls), ("app", false));
    let node = conf(
        r#"<datasource group-name="transactional" database-conf-name="postgres">
                <inline-jdbc jdbc-uri="jdbc:postgresql://h/db?sslmode=verify-ca&amp;sslrootcert=/etc/ssl/rds.pem"/>
            </datasource>"#,
    );
    let ds = datasource_conf(&node, "transactional").unwrap();
    assert!(ds.tls);
    assert_eq!(
        (ds.tls_verify, ds.tls_root_cert.as_str()),
        (TlsVerify::Ca, "/etc/ssl/rds.pem")
    );
    // The datasource's own schema-name wins.
    let node = conf(
        r#"<datasource group-name="transactional" database-conf-name="postgres" schema-name="own">
                <inline-jdbc jdbc-uri="jdbc:postgresql://h/db?currentSchema=app"/>
            </datasource>"#,
    );
    assert_eq!(datasource_conf(&node, "transactional").unwrap().schema, "own");
    let node = conf(
        r#"<datasource group-name="transactional" database-conf-name="mysql8">
                <inline-jdbc jdbc-uri="jdbc:mysql://h:3306/db?useSSL=false&amp;serverTimezone=UTC"/>
            </datasource>"#,
    );
    let ds = datasource_conf(&node, "transactional").unwrap();
    assert!(!ds.tls);
    assert_eq!(ds.database, "db");
}

#[test]
fn use_schemas_false_leaves_the_schema_out() {
    let node = conf(r#"<datasource group-name="transactional" database-conf-name="postgres" schema-name="app"/>"#);
    node.first_named("database-list")
        .unwrap()
        .children_named("database")
        .into_iter()
        .find(|database| database.attribute_is("name", "postgres"))
        .unwrap()
        .set_attribute("use-schemas", "false");
    assert_eq!(datasource_conf(&node, "transactional").unwrap().schema, "");
}

#[test]
fn mysql8_uses_port_and_its_own_test_query() {
    let node = conf(r#"<datasource group-name="transactional" database-conf-name="mysql8"/>"#);
    let ds = datasource_conf(&node, "transactional").unwrap();
    assert_eq!(ds.kind, DatasourceKind::Mysql);
    assert_eq!(ds.dialect, "mysql8");
    assert_eq!((ds.host.as_str(), ds.port), ("my.example", Some(3307)));
    assert_eq!(ds.pool.max, 140);
    assert_eq!(ds.pool.test_query.as_deref(), Some("SELECT 2"));
}

#[test]
fn a_database_without_a_driver_or_a_group_without_a_datasource_is_an_error() {
    let node = conf(r#"<datasource group-name="transactional" database-conf-name="oracle"/>"#);
    let err = datasource_conf(&node, "transactional").unwrap_err();
    assert!(err.to_string().contains("no driver"), "{err}");
    let err = datasource_conf(&node, "nontransactional").unwrap_err();
    assert!(err.to_string().contains("no datasource"), "{err}");
}
