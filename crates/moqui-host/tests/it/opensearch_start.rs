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

//! The search store starts the OpenSearch in `runtime/opensearch` when the opensearch backend's
//! cluster is this host and not running, and stops it when dropped. Ignored unless
//! `OPENSEARCH_START_TEST=1`, with that install present and nothing on port 9200 (and no `pid`
//! file, which marks a node managed outside this process). This binary sets process properties, so
//! it has one test.

#![cfg(all(unix, feature = "embedded-postgres"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use moqui_core::driver::DbDriver;
use moqui_core::transaction::PoolConfig;
use moqui_host::db::embedded::EmbeddedCluster;

#[test]
#[ignore = "starts runtime/opensearch: OPENSEARCH_START_TEST=1 cargo test -p moqui-host --test it -- opensearch_start --ignored"]
fn the_search_store_starts_and_stops_a_local_opensearch() {
    if !crate::common::own_process() {
        return;
    }
    assert_eq!(
        std::env::var("OPENSEARCH_START_TEST").as_deref(),
        Ok("1"),
        "set OPENSEARCH_START_TEST=1"
    );
    let install = std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/opensearch"))
        .expect("runtime/opensearch holds an OpenSearch install");
    assert!(
        !install.join("pid").exists(),
        "a node with a pid file is running; stop it first"
    );
    assert!(
        std::net::TcpStream::connect("127.0.0.1:9200").is_err(),
        "something answers on 9200; stop it first"
    );

    let _serial = crate::common::serial();
    let _properties = crate::common::PropertyReset::of(&["search_backend", "elasticsearch_url"]);
    let runtime = temp_dir("moqui-rt-os-start");
    let cluster_dir = temp_dir("moqui-pg-os-start");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    std::fs::create_dir_all(&runtime).unwrap();
    std::os::unix::fs::symlink(&install, runtime.join("opensearch")).unwrap();
    for (name, value) in [
        ("search_backend", "opensearch"),
        ("elasticsearch_url", "http://127.0.0.1:9200"),
    ] {
        moqui_lib::env::set_property(name, value.to_string());
    }
    let (cluster, driver) = EmbeddedCluster::start(&cluster_dir, 20).unwrap();
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = moqui_core::context::ExecutionContextFactory::boot_with_framework(&runtime, &framework).unwrap();
    let pool = PoolConfig {
        min: 0,
        max: 8,
        reserve: 2,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    };
    let facade = moqui_host::db::entity_facade::open_entity_facade(&framework, Arc::clone(&driver), pool).unwrap();
    moqui_host::db::entity_facade::attach(&factory, facade.clone());

    let store = moqui_host::search::attach_search(&factory, &facade, true).unwrap();
    assert!(store.started_opensearch());
    assert!(std::net::TcpStream::connect("127.0.0.1:9200").is_ok());
    let search = moqui_core::search::installed(&factory).unwrap();
    assert_eq!(search.backend().name(), "opensearch");

    drop(store);
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect("127.0.0.1:9200").is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        std::net::TcpStream::connect("127.0.0.1:9200").is_err(),
        "OpenSearch still answers after the store dropped"
    );
    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
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
