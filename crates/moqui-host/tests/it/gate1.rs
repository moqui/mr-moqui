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

//! Framework data-file load against a fresh embedded cluster, plus a MySQL metadata smoke.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use moqui_core::driver::DbDriver;
use moqui_core::transaction::PoolConfig;
use moqui_host::db::entity_facade::open_entity_facade;
use moqui_host::{load_data, LoadFlags};
use moqui_lib::object::Value;

#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
#[test]
fn framework_seed_and_fixture_load() {
    let _shared = crate::common::shared();
    let runtime = temp_dir("moqui-rt");
    let cluster_dir = temp_dir("moqui-pg");
    let _runtime_guard = DirGuard(runtime.clone());
    let _cluster_guard = DirGuard(cluster_dir.clone());
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&cluster_dir).unwrap();
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");

    let report = load_data(&runtime, &framework, &cluster_dir, &LoadFlags::default()).unwrap();
    assert!(
        report.test_fts_before_load,
        "TestFts table was missing before the seed load"
    );
    assert!(report.enumeration > 0, "enumeration {}", report.enumeration);
    assert!(
        report.user_group_permission > 0,
        "user group permission {}",
        report.user_group_permission
    );
    // The security ECAs fire on each UserGroupMember and UserGroupPermission write, so
    // UserSecurityChecks ran, and authorization was disabled so the sealed grants were allowed.
    assert!(report.security_checks > 0, "security checks {}", report.security_checks);
    println!(
        "seed enumeration {} user group permission {} security checks {}",
        report.enumeration, report.user_group_permission, report.security_checks
    );

    let service = runtime.join("copy-row.xml");
    let data = runtime.join("fixture-data.xml");
    std::fs::write(
        &service,
        r#"<services>
            <service verb="copy" noun="Row">
                <in-parameters><parameter name="seedId"/></in-parameters>
                <actions>
                    <entity-find entity-name="moqui.test.TestEntity" list="found">
                        <econdition field-name="testId" from="seedId"/>
                    </entity-find>
                    <entity-find-one entity-name="moqui.test.TestEntity" value-field="seed">
                        <field-map field-name="testId" from="seedId"/>
                    </entity-find-one>
                    <entity-make-value entity-name="moqui.test.TestEntity" value-field="made"/>
                    <set field="foundCount" from="found.length"/>
                    <script>made.set("testId", "B"); made.set("testMedium", "made:" + foundCount + ":" + seed.get("testMedium"));</script>
                    <entity-create value-field="made"/>
                </actions>
            </service>
        </services>"#,
    )
    .unwrap();
    std::fs::write(
        &data,
        r#"<entity-facade-xml>
            <moqui.test.TestEntity testId="E1" testMedium="entity"/>
            <create-moqui.test.TestEntity testId="A1" testMedium="auto"/>
            <copy-Row seedId="E1"/>
        </entity-facade-xml>"#,
    )
    .unwrap();
    assert!(
        !service.display().to_string().contains("/service/"),
        "fixture service path must not gain a service prefix"
    );
    let flags = LoadFlags {
        location: Some(data.display().to_string()),
        service_files: vec![service],
        ..LoadFlags::default()
    };
    let fixture = load_data(&runtime, &framework, &cluster_dir, &flags).unwrap();
    assert!(fixture.test_fts_before_load);
    // The plain entity element is the counted record. The entity-auto create and the inline
    // service run, and `load_entity_data` does not add them to the record count.
    assert_eq!(fixture.records, 1, "fixture records {}", fixture.records);

    let (enumeration, permissions, medium_a, medium_b, medium_e) = read_back(&framework, &cluster_dir);
    assert!(enumeration > 0, "enumeration {enumeration}");
    assert!(permissions > 0, "user group permission {permissions}");
    let extra = read_extra(&framework, &cluster_dir);
    // ElasticRemote is a sealed permission that only seed-initial grants.
    assert_eq!(extra.sealed_grant, 1, "sealed ElasticRemote grant");
    // BTC is in CurrencyData, which is seed-initial only.
    assert_eq!(extra.initial_uom, 1, "seed-initial Uom BTC");
    assert!(
        extra.fts_indexes >= 1,
        "TestFts has a GIN index over its generated body_tsv column"
    );
    assert_eq!(medium_e.as_deref(), Some("entity"));
    assert_eq!(medium_a.as_deref(), Some("auto"));
    // One row found by the list, and the seed row read back through entity-find-one.
    assert_eq!(medium_b.as_deref(), Some("made:1:entity"));
    println!("read back enumeration {enumeration} user group permission {permissions} fixture rows E1 A1 B");
}

#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
struct Extra {
    sealed_grant: i64,
    initial_uom: i64,
    fts_indexes: usize,
}

#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
fn read_extra(framework: &Path, cluster_dir: &Path) -> Extra {
    use moqui_host::db::embedded::EmbeddedCluster;

    let (cluster, driver) = EmbeddedCluster::start(cluster_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = open_entity_facade(framework, Arc::clone(&driver), pool()).unwrap();
    let sealed_grant = facade
        .find("moqui.security.UserGroupPermission")
        .unwrap()
        .eq("userPermissionId", Value::string("ElasticRemote"))
        .count()
        .unwrap();
    let initial_uom = facade
        .find("moqui.basic.Uom")
        .unwrap()
        .eq("uomId", Value::string("BTC"))
        .count()
        .unwrap();
    drop(facade);
    let mut conn = driver.connect().unwrap();
    let indexes = conn
        .query(
            "SELECT indexdef FROM pg_indexes \
             WHERE lower(tablename) IN ('test_fts', 'testfts') AND lower(indexdef) LIKE '%using gin%body_tsv%'",
            &[],
        )
        .unwrap();
    drop(conn);
    drop(driver);
    drop(cluster);
    Extra {
        sealed_grant,
        initial_uom,
        fts_indexes: indexes.len(),
    }
}

/// `types` picks the data files, and `timeout` is accepted by a load that finishes inside it. The disable flags (audit, feed, foreign
/// keys) are independent of the entity ECAs: they still run. One load covers both, as each flag has its own effect.
#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
#[test]
fn load_flag_types_selects_data_files() {
    let _shared = crate::common::shared();
    let (runtime, cluster_dir, _guards) = fresh_dirs("flag-types");
    let framework = framework_dir();
    let flags = LoadFlags {
        data_types: Some(vec!["seed".to_string()]),
        timeout_seconds: Some(600),
        disable_audit_log: true,
        disable_data_feed: true,
        disable_fk_create: true,
        ..LoadFlags::default()
    };
    let report = load_data(&runtime, &framework, &cluster_dir, &flags).unwrap();
    assert!(report.enumeration > 0, "enumerations are seed");
    assert!(report.security_checks > 0, "entity ECAs stayed on");
    // CurrencyData is seed-initial, so its BTC unit of measure was not loaded.
    assert_eq!(read_extra(&framework, &cluster_dir).initial_uom, 0);
}

/// `raw` turns off entity ECAs, so no security check runs, and the rows still land.
#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
#[test]
fn load_flag_raw_skips_entity_ecas() {
    let _shared = crate::common::shared();
    let (runtime, cluster_dir, _guards) = fresh_dirs("flag-raw");
    let framework = framework_dir();
    let flags = LoadFlags {
        raw: true,
        ..LoadFlags::default()
    };
    let report = load_data(&runtime, &framework, &cluster_dir, &flags).unwrap();
    assert_eq!(report.security_checks, 0, "entity ECAs were disabled");
    assert!(report.enumeration > 0);
    assert!(report.user_group_permission > 0);
}

/// The SQL the in-memory driver cannot prove: aggregates with GROUP BY and HAVING, DISTINCT, a
/// locking read, UPPER() comparisons, and a search form.
#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
#[test]
fn sql_features_run_on_postgres() {
    let _shared = crate::common::shared();
    use moqui_core::entity::{CompareOp, Condition};
    use moqui_host::db::embedded::EmbeddedCluster;

    let (_runtime, cluster_dir, _guards) = fresh_dirs("sql-features");
    let framework = framework_dir();
    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = open_entity_facade(&framework, driver, pool()).unwrap();
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();
    for (id, medium, number) in [("A", "alpha", 1_i64), ("B", "alpha", 5), ("C", "Beta", 3)] {
        let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
        row.set("testId", Value::string(id));
        row.set("testMedium", Value::string(medium));
        row.set("testNumberInteger", Value::I64(number));
        row.create(&facade).unwrap();
    }
    let ids = |list: moqui_core::entity::EntityList| -> Vec<String> {
        let mut ids: Vec<String> = list
            .values()
            .iter()
            .filter_map(|row| row.get("testId").and_then(Value::as_str).map(str::to_string))
            .collect();
        ids.sort();
        ids
    };

    // Aggregates: one group row per medium, and HAVING names the aggregate.
    let groups = facade
        .find("moqui.test.TestEntityCount")
        .unwrap()
        .having(Condition::greater("entityCount", Value::I64(1)))
        .list()
        .unwrap();
    assert_eq!(groups.len(), 1);
    let group = &groups.values()[0];
    assert_eq!(group.get("testMedium").and_then(Value::as_str), Some("alpha"));
    assert_eq!(
        group
            .get("entityCount")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("2")
    );
    assert_eq!(
        group
            .get("maxNumber")
            .map(moqui_lib::object::to_plain_string)
            .as_deref(),
        Some("5")
    );
    let all_groups = facade
        .find("moqui.test.TestEntityCount")
        .unwrap()
        .order_by("entityCount", true)
        .list()
        .unwrap();
    assert_eq!(all_groups.len(), 2);

    // UPPER() on both sides.
    let ignoring = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::ignore_case(Condition::equals(
            "testMedium",
            Value::string("BETA"),
        )))
        .list()
        .unwrap();
    assert_eq!(ids(ignoring), ["C"]);
    // Field against field: testNumberInteger = testNumberInteger holds for every row with a number.
    let same = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .condition(Condition::compare_fields(
            "testNumberInteger",
            CompareOp::Equals,
            "testNumberInteger",
        ))
        .list()
        .unwrap();
    assert_eq!(ids(same), ["A", "B", "C"]);

    // DISTINCT over one selected field.
    let distinct = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .select_field("testMedium")
        .distinct(true)
        .list()
        .unwrap();
    assert_eq!(distinct.len(), 2);

    // A locking read needs a transaction and runs inside one.
    assert!(facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .for_update(true)
        .list()
        .is_err());
    facade.begin("moqui.test.TestEntity", None).unwrap();
    let locked = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("A"))
        .for_update(true)
        .list();
    facade.rollback("moqui.test.TestEntity").unwrap();
    assert_eq!(locked.unwrap().len(), 1);

    // A search form: contains, ignoring case, ordered and paged.
    let mut form = std::collections::BTreeMap::new();
    form.insert("testMedium".to_string(), Value::string("ALP"));
    form.insert("testMedium_op".to_string(), Value::string("contains"));
    form.insert("testMedium_ic".to_string(), Value::string("Y"));
    form.insert("orderByField".to_string(), Value::string("-testId"));
    form.insert("pageSize".to_string(), Value::string("1"));
    let searched = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .search_form_map(&form, &std::collections::BTreeMap::new(), "", "", false)
        .unwrap()
        .find
        .list()
        .unwrap();
    assert_eq!(ids(searched), ["B"], "highest testId of the alpha rows, one per page");

    drop(facade);
    drop(cluster);
}

#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
fn fresh_dirs(prefix: &str) -> (PathBuf, PathBuf, (DirGuard, DirGuard)) {
    let runtime = temp_dir(&format!("moqui-rt-{prefix}"));
    let cluster = temp_dir(&format!("moqui-pg-{prefix}"));
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::create_dir_all(&cluster).unwrap();
    let guards = (DirGuard(runtime.clone()), DirGuard(cluster.clone()));
    (runtime, cluster, guards)
}

fn framework_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
fn read_back(framework: &Path, cluster_dir: &Path) -> (i64, i64, Option<String>, Option<String>, Option<String>) {
    use moqui_host::db::embedded::EmbeddedCluster;

    let (cluster, driver) = EmbeddedCluster::start(cluster_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let facade = open_entity_facade(framework, driver, pool()).unwrap();
    let enumeration = facade.find("moqui.basic.Enumeration").unwrap().count().unwrap();
    let permissions = facade
        .find("moqui.security.UserGroupPermission")
        .unwrap()
        .count()
        .unwrap();
    let medium_e = medium(&facade, "E1");
    let medium_a = medium(&facade, "A1");
    let medium_b = medium(&facade, "B");
    drop(facade);
    drop(cluster);
    (enumeration, permissions, medium_a, medium_b, medium_e)
}

#[cfg(all(unix, feature = "embedded-postgres", feature = "postgres"))]
fn medium(facade: &moqui_core::entity::EntityFacade, test_id: &str) -> Option<String> {
    facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string(test_id))
        .one()
        .unwrap()
        .and_then(|row| row.get("testMedium").and_then(Value::as_str).map(str::to_string))
}

/// Ignored by default so a run without MySQL reports it as ignored instead of green.
/// Run with `--ignored`; set `MOQUI_TEST_MYSQL=user:pass@host:port/database` for a non-default server.
#[cfg(feature = "mysql")]
#[test]
#[ignore = "needs a MySQL server: cargo test -p moqui-host --test it -- gate1 --ignored"]
fn mysql_metadata_insert_and_select() {
    let _shared = crate::common::shared();
    let (host, port, user, password, database) =
        mysql_target().expect("MySQL is not reachable on 127.0.0.1:3306 and MOQUI_TEST_MYSQL is not set");
    let driver = moqui_host::db::mysql::MysqlDriver::new(host, port, user, password, database, false);
    driver.connect_mysql().expect("MySQL rejected the connection");
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let facade =
        moqui_host::db::entity_facade::open_entity_facade_dialect(&framework, Arc::clone(&driver), pool(), "mysql")
            .unwrap();
    facade.check_tables(&["moqui.test.TestEntity"]).unwrap();
    let mut row = facade.make_value("moqui.test.TestEntity").unwrap();
    row.set("testId", Value::string("MYSQL1"));
    row.set("testMedium", Value::string("mysql"));
    row.create(&facade).unwrap();
    let found = facade
        .find("moqui.test.TestEntity")
        .unwrap()
        .eq("testId", Value::string("MYSQL1"))
        .one()
        .unwrap()
        .unwrap();
    assert_eq!(found.get("testMedium").and_then(Value::as_str), Some("mysql"));
    found.delete(&facade).unwrap();
    drop(facade);
    let mut conn = driver.connect().unwrap();
    conn.execute("DROP TABLE IF EXISTS `TestEntity`", &[]).unwrap();
    eprintln!("MySQL smoke inserted and selected moqui.test.TestEntity");
}

#[cfg(feature = "mysql")]
fn mysql_target() -> Option<(String, u16, String, String, String)> {
    if let Ok(spec) = std::env::var("MOQUI_TEST_MYSQL") {
        let (user, password, host, port, database) =
            parse_mysql_spec(&spec).expect("MOQUI_TEST_MYSQL must be user:pass@host:port/database");
        return Some((
            host.to_string(),
            port,
            user.to_string(),
            password.to_string(),
            database.to_string(),
        ));
    }
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], 3306));
    if std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_err() {
        return None;
    }
    Some((
        "127.0.0.1".to_string(),
        3306,
        "root".to_string(),
        String::new(),
        "mysql".to_string(),
    ))
}

#[cfg(feature = "mysql")]
fn parse_mysql_spec(spec: &str) -> Option<(&str, &str, &str, u16, &str)> {
    let spec = spec.trim();
    let (userinfo, rest) = spec.rsplit_once('@')?;
    let (username, password) = userinfo.split_once(':')?;
    let (hostport, database) = rest.split_once('/')?;
    let (host, port) = hostport.rsplit_once(':')?;
    let port = port.parse().ok()?;
    if host.is_empty() || database.is_empty() {
        return None;
    }
    Some((username, password, host, port, database))
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 4,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("{prefix}-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
