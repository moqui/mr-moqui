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

//! The Postgres search backend with a `read-url` replica: a streaming standby of an embedded
//! cluster, made with `pg_basebackup -R` from the same install and started with `pg_ctl`. Reads go
//! to the standby and can be behind; a read-your-writes search waits for replay, or runs on the
//! primary when replay does not catch up by its deadline.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_core::driver::DbDriver;
use moqui_core::search::mapping::{DocumentMapping, FieldKind, FieldMapping, ObjectMapping};
use moqui_core::search::postgres::PostgresBackend;
use moqui_core::search::{Query, SearchBackend, SearchRequest};
use moqui_core::transaction::{PoolConfig, TransactionManager};
use moqui_host::db::embedded::EmbeddedCluster;
use moqui_host::db::postgres::PostgresDriver;
use moqui_lib::object::Value;

const STANDBY_PORT: u16 = 5433;

#[test]
fn reads_go_to_the_replica_and_fresh_reads_wait_or_use_the_primary() {
    let _shared = crate::common::shared();
    let primary_dir = temp_dir("moqui-pg-replica-primary");
    let standby_dir = temp_dir("moqui-pg-replica-standby");
    let _guards = (DirGuard(primary_dir.clone()), DirGuard(standby_dir.clone()));
    std::fs::create_dir_all(&primary_dir).unwrap();
    std::fs::create_dir_all(&standby_dir).unwrap();
    let (cluster, driver) = EmbeddedCluster::start(&primary_dir, 20).unwrap();
    let password = std::fs::read_to_string(primary_dir.join("pgpass"))
        .unwrap()
        .trim()
        .to_string();
    let bin = install_bin(&primary_dir);
    let standby_data = standby_dir.join("data");
    let standby = Standby::start(&bin, cluster.data_dir(), &standby_data, &password);

    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let primary_tx = TransactionManager::new(Arc::clone(&driver), pool()).unwrap();
    let read_driver = PostgresDriver::unix_socket("postgres", "moqui", &standby_data)
        .port(STANDBY_PORT)
        .password(&password)
        .options("-c default_transaction_read_only=on -c statement_timeout=30000");
    let read_tx = TransactionManager::new(Arc::new(read_driver), pool()).unwrap();
    let backend = PostgresBackend::open(primary_tx, None)
        .unwrap()
        .with_read_replica(read_tx);

    let mapping = note_mapping();
    backend.ensure_index(&mapping).unwrap();
    let mappings = vec![Arc::new(mapping.clone())];
    let ids = |request: &SearchRequest| -> Vec<String> {
        let mut ids: Vec<String> = backend
            .search(&mappings, request)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.id)
            .collect();
        ids.sort();
        ids
    };
    let eventual = SearchRequest::new(Query::MatchAll);
    let fresh = |millis| {
        let mut request = SearchRequest::new(Query::MatchAll);
        request.fresh_by = Some(Instant::now() + Duration::from_millis(millis));
        request
    };

    // An eventual read reaches the replica, which soon has the first document.
    backend.index_documents(&mapping, &[note("N1", "first")]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while ids(&eventual) != vec!["N1"] && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ids(&eventual), vec!["N1"]);
    // Initial schema replay may fall back; measure the fresh reads independently of that startup race.
    let initial_fallbacks = backend.replica_status().unwrap().primary_fallbacks;
    // A fresh read right after a write sees it, by waiting for replay.
    backend.index_documents(&mapping, &[note("N2", "second")]).unwrap();
    assert_eq!(ids(&fresh(5000)), vec!["N1", "N2"]);
    assert_eq!(backend.replica_status().unwrap().primary_fallbacks, initial_fallbacks);

    // With replay paused, the replica stays behind: an eventual read misses the new document, and a
    // fresh read that cannot wait runs on the primary.
    standby.sql("SELECT pg_wal_replay_pause()");
    backend.index_documents(&mapping, &[note("N3", "third")]).unwrap();
    assert_eq!(ids(&eventual), vec!["N1", "N2"], "the paused replica is behind");
    assert_eq!(
        ids(&fresh(300)),
        vec!["N1", "N2", "N3"],
        "the fresh read ran on the primary"
    );
    let status = backend.replica_status().unwrap();
    assert_eq!(status.primary_fallbacks, initial_fallbacks + 1);
    assert!(status.lag_seconds.is_some(), "{status:?}");
    standby.sql("SELECT pg_wal_replay_resume()");
    let deadline = Instant::now() + Duration::from_secs(10);
    while ids(&eventual).len() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(ids(&eventual), vec!["N1", "N2", "N3"]);

    // Aggregations route the same way.
    let aggs = moqui_core::search::agg::Agg::map_from_value(&moqui_lib::json::from_json(
        &serde_json::json!({"titles": {"terms": {"field": "noteId", "size": 10}}}),
    ))
    .unwrap();
    let result = backend
        .aggregate_fresh(
            &mapping,
            &Query::MatchAll,
            &aggs,
            Some(Instant::now() + Duration::from_secs(5)),
        )
        .unwrap();
    let Value::Map(titles) = &result["titles"] else {
        panic!("{result:?}")
    };
    let Some(Value::List(buckets)) = titles.get("buckets") else {
        panic!("{titles:?}")
    };
    assert_eq!(buckets.len(), 3);

    backend.drop_index(&mapping).unwrap();
    drop(backend);
    drop(standby);
    drop(driver);
    drop(cluster);
}

fn note_mapping() -> DocumentMapping {
    let mut root = ObjectMapping::default();
    root.fields.insert(
        "noteId".into(),
        FieldMapping {
            kind: FieldKind::Keyword,
            sortable: false,
        },
    );
    root.fields.insert(
        "noteText".into(),
        FieldMapping {
            kind: FieldKind::Text,
            sortable: false,
        },
    );
    DocumentMapping {
        document_id: "TestReplicaNote".into(),
        index_name: None,
        physical_name: "test_replica_note".into(),
        root,
        manual_mapping_service: None,
        display: Default::default(),
    }
}

fn note(id: &str, text: &str) -> (String, moqui_core::entity::document::Document) {
    let mut body = moqui_core::entity::document::Document::new();
    body.insert("noteId".into(), Value::string(id));
    body.insert("noteText".into(), Value::string(text));
    (id.to_string(), body)
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

/// `install/<version>/bin` of an embedded cluster.
fn install_bin(cluster_dir: &Path) -> PathBuf {
    let install = cluster_dir.join("install");
    let version = std::fs::read_dir(&install)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.join("bin/pg_basebackup").is_file())
        .unwrap_or_else(|| panic!("no pg_basebackup under {}", install.display()));
    version.join("bin")
}

/// A hot standby of the embedded primary, on its own socket directory and port.
struct Standby {
    bin: PathBuf,
    data: PathBuf,
    password: String,
}

impl Standby {
    fn start(bin: &Path, primary_socket: &Path, data: &Path, password: &str) -> Self {
        let conninfo = format!(
            "host={} port=5432 user=postgres password={password}",
            primary_socket.display()
        );
        let output = Command::new(bin.join("pg_basebackup"))
            .args(["-d", &conninfo, "-D"])
            .arg(data)
            .args(["-R", "-X", "stream", "-c", "fast"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "pg_basebackup: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let options = format!(
            "-c port={STANDBY_PORT} -c unix_socket_directories='{}' -c listen_addresses='' -c hot_standby=on",
            data.display()
        );
        let output = Command::new(bin.join("pg_ctl"))
            .arg("-D")
            .arg(data)
            .args(["-o", &options, "-l"])
            .arg(data.join("standby.log"))
            .args(["-w", "start"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "pg_ctl start: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Self {
            bin: bin.to_path_buf(),
            data: data.to_path_buf(),
            password: password.to_string(),
        }
    }

    /// One statement as the superuser, through the replica's driver.
    fn sql(&self, statement: &str) {
        let driver = PostgresDriver::unix_socket("postgres", "moqui", &self.data)
            .port(STANDBY_PORT)
            .password(&self.password);
        let mut conn = driver.connect().unwrap();
        conn.query(statement, &[]).unwrap();
    }
}

impl Drop for Standby {
    fn drop(&mut self) {
        let _ = Command::new(self.bin.join("pg_ctl"))
            .arg("-D")
            .arg(&self.data)
            .args(["-m", "fast", "-w", "stop"])
            .output();
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
