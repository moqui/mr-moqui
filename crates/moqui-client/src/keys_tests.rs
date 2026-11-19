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

#[test]
fn a_file_key_is_the_owners_alone_and_can_be_forgotten() {
    let dir = std::env::temp_dir().join(format!("moqui-client-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = FileKeyStore::new(&dir);
    assert_eq!(store.get("erp.example.com-abc"), None);
    store.set("erp.example.com-abc", "key-one").unwrap();
    store.set("erp.example.com-abc", "key-two").unwrap();
    assert_eq!(store.get("erp.example.com-abc").as_deref(), Some("key-two"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("erp.example.com-abc.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    store.delete("erp.example.com-abc").unwrap();
    store.delete("erp.example.com-abc").unwrap();
    assert_eq!(store.get("erp.example.com-abc"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_id_that_could_leave_the_directory_is_refused() {
    let store = FileKeyStore::new(&std::env::temp_dir().join("moqui-client-keys-refused"));
    assert!(store.set("../escape", "key").is_err());
    assert!(store.set("", "key").is_err());
    assert!(store.set(".hidden", "key").is_err());
    assert_eq!(store.get("../escape"), None);
    assert!(store.delete("a/b").is_ok());
}

#[test]
fn chat_store_keys_are_kept_per_device_and_go_with_the_account() {
    let dir = std::env::temp_dir().join(format!("moqui-client-chat-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = Keys::new(Box::new(FileKeyStore::new(&dir)), "file");
    keys.set("a", "device-key").unwrap();
    keys.set_chat_store_key("a", "DEV1", "k1").unwrap();
    keys.set_chat_store_key("a", "DEV2", "k2").unwrap();
    keys.set_chat_store_key("a", "DEV1", "k1b").unwrap();
    assert_eq!(keys.chat_store_key("a", "DEV1").as_deref(), Some("k1b"));
    assert_eq!(keys.chat_store_key("b", "DEV1"), None);
    keys.remove_chat_store_key("a", "DEV2").unwrap();
    assert_eq!(keys.chat_store_key("a", "DEV2"), None);
    // read again from the file, not the memory
    let again = Keys::new(Box::new(FileKeyStore::new(&dir)), "file");
    assert_eq!(again.chat_store_key("a", "DEV1").as_deref(), Some("k1b"));
    again.forget_chat_store_keys("a").unwrap();
    assert_eq!(again.chat_store_key("a", "DEV1"), None);
    // the device key is another entry
    assert_eq!(again.get("a").as_deref(), Some("device-key"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-CLI-108: the keys waiting to be revoked are kept in the key store, sealed as the keys are, under a name no
/// account id can be; the newest are kept when there are too many, and a key signed out twice is there once.
#[test]
fn the_keys_waiting_to_be_revoked_are_kept_beside_the_keys() {
    let dir = std::env::temp_dir().join(format!("moqui-client-pending-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let keys = Keys::new(Box::new(FileKeyStore::new(&dir)), "file");
    assert!(keys.pending_revokes().is_empty());
    keys.add_pending_revoke("https://erp.example.com", "key-a", 1_000)
        .unwrap();
    keys.add_pending_revoke("https://erp.example.com", "key-a", 2_000)
        .unwrap();
    let again = Keys::new(Box::new(FileKeyStore::new(&dir)), "file");
    assert_eq!(
        again.pending_revokes(),
        [PendingRevoke {
            server: "https://erp.example.com".to_string(),
            key: "key-a".to_string(),
            at: 2_000,
            tried: 2_000
        }]
    );
    for at in 0..40 {
        keys.add_pending_revoke("https://erp.example.com", &format!("key-{at}"), 10_000 + at)
            .unwrap();
    }
    let kept = keys.pending_revokes();
    assert_eq!(kept.len(), PENDING_MAX);
    assert_eq!(
        kept.first().map(|entry| entry.at),
        Some(10_000 + 40 - PENDING_MAX as i64)
    );
    keys.set_pending_revokes(Vec::new()).unwrap();
    assert!(keys.pending_revokes().is_empty());
    assert!(!crate::account::account_id("https://erp.example.com", "john.doe").contains('_'));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A store that opens its `.chat-store` entries only while `open`, as the phone's does while it is unlocked.
struct Lockable {
    inner: FileKeyStore,
    open: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl KeyStore for Lockable {
    fn get(&self, account: &str) -> Option<String> {
        if self.fresh(account) && !self.open.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        self.inner.get(account)
    }

    fn set(&self, account: &str, key: &str) -> Result<(), Error> {
        self.inner.set(account, key)
    }

    fn delete(&self, account: &str) -> Result<(), Error> {
        self.inner.delete(account)
    }

    fn fresh(&self, entry: &str) -> bool {
        entry.ends_with(".chat-store")
    }
}

/// SEC-CLI-113: an entry the store opens only at some times is read each time: a read while it does not open is not
/// kept as none for later, and what it answered is not kept in memory; the device keys are kept as before.
#[test]
fn an_entry_the_store_opens_only_at_some_times_is_read_each_time() {
    use std::sync::atomic::Ordering;
    let dir = std::env::temp_dir().join(format!("moqui-keys-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let store = Lockable {
        inner: FileKeyStore::new(&dir),
        open: std::sync::Arc::clone(&open),
    };
    let keys = Keys::new(Box::new(store), "os");
    keys.set("acct", "device-key").unwrap();
    keys.set_chat_store_key("acct", "DEV", "pass").unwrap();
    assert_eq!(keys.chat_store_key("acct", "DEV").as_deref(), Some("pass"));
    // locked: the passphrase does not open, though it was read and set just now
    open.store(false, Ordering::SeqCst);
    assert_eq!(keys.chat_store_key("acct", "DEV"), None);
    // unlocked again: that none was not remembered
    open.store(true, Ordering::SeqCst);
    assert_eq!(keys.chat_store_key("acct", "DEV").as_deref(), Some("pass"));
    // the device key is kept in memory as before: it reads with its file moved away
    std::fs::rename(dir.join("acct.key"), dir.join("acct.moved")).unwrap();
    assert_eq!(keys.get("acct").as_deref(), Some("device-key"));
    let _ = std::fs::remove_dir_all(&dir);
}
