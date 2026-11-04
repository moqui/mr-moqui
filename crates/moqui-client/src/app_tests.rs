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

use std::time::Instant;

use super::*;
use crate::keys::FileKeyStore;

/// A host with no pages.
struct NoPages;

impl ClientHost for NoPages {
    fn page_account(&self, _page: &str) -> Option<String> {
        None
    }

    fn is_shell(&self, page: &str) -> bool {
        page == "shell"
    }

    fn notify_pages(&self, _account: &str, _event: &Value) {}

    fn socket_event(&self, _page: &str, _id: u64, _event: &Value) {}

    fn check_key(&self, _account: &Account) {}
}

fn client(name: &str) -> (Arc<Client>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("moqui-client-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = Keys::new(Box::new(FileKeyStore::new(&dir.join("keys"))), "file");
    let client = Client::new(&dir, keys, Box::new(ClientConf::default)).unwrap();
    (Arc::new(client), dir)
}

#[test]
fn the_threads_end_and_let_the_client_go_after_a_shutdown() {
    let (client, dir) = client("shutdown");
    let host: Arc<dyn ClientHost> = Arc::new(NoPages);
    crate::sync::flush::start(&client, &host);
    crate::renewal::start(&client, &host);
    assert!(Arc::strong_count(&client) > 1, "the threads hold the client");
    assert!(!client.is_stopped());
    client.shutdown();
    assert!(client.is_stopped());
    let started = Instant::now();
    while Arc::strong_count(&client) > 1 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the threads still hold the client"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // nothing starts again on a client shut down
    crate::sync::mirror::start(&client, &host, "a");
    assert_eq!(Arc::strong_count(&client), 1);
    assert_eq!(
        crate::bridge::open_remote(&client, &host, "shell", "/notws"),
        Err("The client is stopped".to_string())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_wait_ends_at_its_timeout_or_the_shutdown() {
    let (client, dir) = client("wait");
    assert!(!client.wait_stopped(Duration::from_millis(5)));
    client.shutdown();
    let started = Instant::now();
    assert!(client.wait_stopped(Duration::from_secs(60)));
    assert!(started.elapsed() < Duration::from_secs(1));
    let _ = std::fs::remove_dir_all(&dir);
}
