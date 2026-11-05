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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::driver::DbDriver;
use moqui_core::transaction::PoolConfig;
use moqui_drivers::embedded::{EmbeddedCluster, StartOptions};

use crate::capabilities::{probe_group, Backend};
use crate::DbRef;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The registry has a function at each `moqui_tools::` location the framework's service files name, and no other.
#[test]
fn the_services_register_at_the_locations_the_service_files_name() {
    let mut map = std::collections::BTreeMap::new();
    super::service::register(&mut map).unwrap();
    let registered: Vec<&str> = map.keys().copied().collect();
    let mut files = Vec::new();
    service_files(&moqui_core::context::framework_dir().join("service"), &mut files);
    assert!(!files.is_empty());
    let mut named: Vec<String> = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        named.extend(
            text.split("location=\"")
                .skip(1)
                .filter_map(|rest| rest.split_once('"').map(|(location, _)| location.to_string()))
                .filter(|location| location.starts_with("moqui_tools::")),
        );
    }
    named.sort_unstable();
    named.dedup();
    assert_eq!(registered, named);
}

/// Every `.xml` file under `dir`, at any depth.
fn service_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            service_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "xml") {
            out.push(path);
        }
    }
}

/// Privileges come from the list before ` ON ` and only on `*.*`; a user named like a privilege is not one, and a
/// grant of roles is noted.
#[test]
fn mysql_grants_are_read_from_the_privilege_list() {
    let grants = crate::capabilities::GlobalGrants::parse(&[
        "GRANT USAGE ON *.* TO `superset`@`%`".to_string(),
        "GRANT SELECT, PROCESS ON `app`.* TO `superset`@`%`".to_string(),
        "GRANT `monitor`@`%` TO `superset`@`%`".to_string(),
    ]);
    assert!(!grants.has("SUPER"));
    assert!(!grants.has("PROCESS"), "PROCESS on one schema is not global");
    assert!(grants.through_roles);
    let admin = crate::capabilities::GlobalGrants::parse(&[
        "GRANT PROCESS, CONNECTION_ADMIN ON *.* TO `ops`@`localhost`".to_string(),
    ]);
    assert!(admin.has("PROCESS") && admin.has("CONNECTION_ADMIN") && !admin.has("SUPER"));
    assert!(!admin.through_roles);
    let all = crate::capabilities::GlobalGrants::parse(&["GRANT ALL PRIVILEGES ON *.* TO `root`@`%`".to_string()]);
    assert!(all.has("SUPER"));
}

#[test]
fn db_names_a_group_or_an_external_connection() {
    assert_eq!(
        DbRef::parse("group:content", "transactional"),
        Ok(DbRef::Group("content".to_string()))
    );
    assert_eq!(
        DbRef::parse("", "transactional"),
        Ok(DbRef::Group("transactional".to_string()))
    );
    assert_eq!(
        DbRef::parse("ext:REPORTING", "transactional"),
        Ok(DbRef::External("REPORTING".to_string()))
    );
    assert!(DbRef::parse("content", "transactional").is_err());
    assert!(DbRef::parse("group:", "transactional").is_err());
}

/// On the embedded cluster with pg_stat_statements preloaded: before the extension is created statement stats are not
/// available, and say why; after, every feature is there, as the bootstrap superuser. An unknown group answers with
/// the reason.
#[test]
fn the_embedded_cluster_allows_everything_with_pg_stat_statements_loaded() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = PathBuf::from("/tmp").join(format!("moqui-pg-tools-{}-{nanos}", std::process::id()));
    let _guard = DirGuard(dir.clone());
    let options = StartOptions {
        pg_stat_statements: true,
        ..StartOptions::new(20)
    };
    let (cluster, driver) = EmbeddedCluster::start_with(&dir, &options).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = moqui_core::context::framework_dir();
    let facade = moqui_drivers::entity_facade::open_entity_facade(
        &framework,
        Arc::clone(&driver),
        PoolConfig {
            min: 0,
            max: 4,
            reserve: 1,
            acquire_timeout: Duration::from_secs(30),
            ..Default::default()
        },
    )
    .unwrap();

    // the library is loaded, the extension not yet created
    let caps = probe_group(&facade, "transactional", true);
    assert!(caps.error.is_none(), "{:?}", caps.error);
    assert_eq!(caps.backend, Some(Backend::EmbeddedPostgres));
    assert!(caps
        .version
        .as_deref()
        .is_some_and(|version| version.starts_with("PostgreSQL")));
    assert!(caps.uptime_seconds.is_some_and(|seconds| seconds >= 0));
    assert!(caps.database_size.is_some_and(|size| size > 0));
    let statements = caps.feature("statementStats");
    assert!(!statements.available);
    assert!(
        statements
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("CREATE EXTENSION")),
        "{statements:?}"
    );

    driver
        .connect()
        .unwrap()
        .execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements", &[])
        .unwrap();
    let caps = probe_group(&facade, "transactional", true);
    for name in crate::capabilities::FEATURES {
        let feature = caps.feature(name);
        assert!(feature.available, "{name}: {feature:?}");
    }
    assert_eq!(caps.feature("statementStats").reason, None);
    let out = caps.to_out();
    assert_eq!(
        out.get("backend"),
        Some(&moqui_lib::object::Value::string("embedded-postgres"))
    );

    // an unknown group answers, with the reason
    let none = probe_group(&facade, "nothing", true);
    assert!(none.error.as_deref().is_some_and(|error| error.contains("nothing")));
    drop(facade);
    drop(driver);
    drop(cluster);
}

/// A role that may not read `shared_preload_libraries` (a managed server) still gets statement stats when the view
/// reads; the reason the screen shows does not carry the driver's error.
#[test]
fn statement_stats_are_decided_by_the_view_when_the_preload_setting_is_hidden() {
    use crate::capabilities::{probe_postgres, Feature, Row};
    use moqui_lib::object::Value;
    let row = |pairs: &[(&str, Value)]| -> Row {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect()
    };
    let readable = std::cell::Cell::new(true);
    let run = |name: &str, _sql: &str| -> Result<Vec<Row>, String> {
        match name {
            "server" => Ok(vec![row(&[
                ("version", Value::string("PostgreSQL 17.2")),
                ("is_superuser", Value::Bool(false)),
            ])]),
            "roles" => Ok(vec![row(&[("pg_read_all_stats", Value::Bool(true))])]),
            "extensions" => Ok(vec![row(&[
                ("name", Value::string("pg_stat_statements")),
                ("installed_version", Value::string("1.11")),
            ])]),
            "statements" if readable.get() => Ok(vec![row(&[("statements", Value::I64(3))])]),
            other => Err(format!("the {other} query failed (the server log has the detail)")),
        }
    };
    let caps = probe_postgres("group:transactional".to_string(), false, &run);
    assert_eq!(
        caps.feature("statementStats"),
        Feature {
            available: true,
            reason: None
        }
    );
    readable.set(false);
    let caps = probe_postgres("group:transactional".to_string(), false, &run);
    let statements = caps.feature("statementStats");
    assert!(!statements.available);
    assert!(
        statements
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("shared_preload_libraries")),
        "{statements:?}"
    );
}
