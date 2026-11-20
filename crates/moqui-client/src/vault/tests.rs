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

//! The vault in the apps (V5): device memory ends at its time limit and when the device is revoked, it is offered only
//! with a key store and an open key, and the app's rollback memory is the module's, not the page's.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::memory::{KeyError, MemoryKeys, StoreMemoryKeys, KIND_DEVICE_LOCK, KIND_FILE, KIND_OS};
use super::server::{MemoryServer, ServerError};
use super::{Restore, Vaults, MAX_MEMORY_MINUTES};
use crate::keys::FileKeyStore;

const ACCOUNT: &str = "example.com-0123456789ab";
const MINUTE: i64 = 60_000;
const T0: i64 = 1_800_000_000_000;

/// A server's record of memories: the live ones, and whether it takes the device key or answers at all.
#[derive(Default)]
struct FakeServer {
    live: RefCell<BTreeSet<String>>,
    revoked: bool,
    unreachable: bool,
    /// The most minutes it allows.
    max_minutes: Option<u32>,
}

impl MemoryServer for FakeServer {
    fn register(&self, memory_id: &str, _device_name: &str, minutes: u32) -> Result<i64, ServerError> {
        self.live.borrow_mut().insert(memory_id.to_string());
        let minutes = self.max_minutes.map_or(minutes, |max| minutes.min(max));
        Ok(T0 + i64::from(minutes) * MINUTE)
    }

    fn check(&self, memory_id: &str) -> Result<bool, ServerError> {
        if self.unreachable {
            return Err(ServerError::Unreachable("no route".to_string()));
        }
        if self.revoked {
            return Err(ServerError::Revoked);
        }
        Ok(self.live.borrow().contains(memory_id))
    }

    fn forget(&self, memory_id: &str) -> Result<(), ServerError> {
        self.live.borrow_mut().remove(memory_id);
        Ok(())
    }
}

/// A scratch client directory and the key store directory beside it.
fn dirs(name: &str) -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("moqui-client-vault-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    (base.join("client"), base.join("keys"))
}

/// The vaults of `client` with memory keys in a file store at `keys` that says it is `kind`.
fn vaults(client: &Path, keys: &Path, kind: &'static str) -> Vaults {
    let vaults = Vaults::new(client);
    vaults.set_memory_keys(Box::new(StoreMemoryKeys::new(Box::new(FileKeyStore::new(keys)), kind)));
    vaults
}

/// A new personal key opened in `vaults` for the account.
fn make_key(vaults: &Vaults) -> Value {
    let made = vaults
        .call_at(
            ACCOUNT,
            "personal_make",
            json!({ "personal_key_id": "pk-alice", "user_id": "alice", "passphrase": "pass phrase" }),
            T0,
        )
        .unwrap();
    assert!(vaults.is_open(ACCOUNT));
    made
}

fn kept_files(client: &Path, keys: &Path) -> (bool, bool) {
    let memory = client.join("vault").join(format!("{ACCOUNT}.memory.json")).exists();
    let key = keys.join(format!("{ACCOUNT}.vault-memory.key")).exists();
    (memory, key)
}

fn opened(restore: &Restore) -> bool {
    matches!(restore, Restore::Opened(_))
}

#[test]
fn device_memory_ends_at_its_time_limit() {
    let (client, keys) = dirs("limit");
    let server = FakeServer::default();
    let first = vaults(&client, &keys, KIND_OS);
    let made = make_key(&first);
    let answer = first.remember_at(ACCOUNT, 10, "Alice's laptop", &server, T0).unwrap();
    assert_eq!(answer["until"], json!(T0 + 10 * MINUTE));
    assert_eq!(kept_files(&client, &keys), (true, true));
    let file = std::fs::read_to_string(client.join("vault").join(format!("{ACCOUNT}.memory.json"))).unwrap();
    assert!(!file.contains("pass phrase"));

    // the app starts again: the key opens from memory with no passphrase, as the same key
    let again = vaults(&client, &keys, KIND_OS);
    assert!(!again.is_open(ACCOUNT));
    let restored = again.restore_at(ACCOUNT, true, &server, T0 + MINUTE);
    let Restore::Opened(result) = &restored else {
        panic!("not opened: {restored:?}");
    };
    assert_eq!(result["public"], made["public"]);
    assert!(again.is_open(ACCOUNT));

    // at its time the session opened from it locks, and the memory is forgotten with its key
    again.sweep_at(T0 + 10 * MINUTE);
    assert!(!again.is_open(ACCOUNT), "the session ends with the memory");
    assert_eq!(kept_files(&client, &keys), (false, false));
    assert_eq!(
        again.restore_at(ACCOUNT, true, &server, T0 + 11 * MINUTE),
        Restore::Nothing
    );

    // a memory past its time is forgotten at the next open even when no sweep ran
    let third = vaults(&client, &keys, KIND_OS);
    make_key(&third);
    third.remember_at(ACCOUNT, 5, "Alice's laptop", &server, T0).unwrap();
    let later = vaults(&client, &keys, KIND_OS);
    assert_eq!(
        later.restore_at(ACCOUNT, true, &server, T0 + 5 * MINUTE),
        Restore::Nothing
    );
    assert_eq!(kept_files(&client, &keys), (false, false));
}

#[test]
fn device_memory_ends_when_the_device_is_revoked() {
    let (client, keys) = dirs("revoked");
    let remembered = |server: &FakeServer| {
        let vaults = vaults(&client, &keys, KIND_OS);
        make_key(&vaults);
        vaults.remember_at(ACCOUNT, 60, "Alice's phone", server, T0).unwrap();
        vaults
    };

    // the server no longer takes the device key (revoked while the app was not running): nothing opens, all goes
    let mut server = FakeServer::default();
    remembered(&server);
    server.revoked = true;
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, true, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));
    assert!(!fresh.is_open(ACCOUNT));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // the user took the device off the server's list
    let server = FakeServer::default();
    remembered(&server);
    server.live.borrow_mut().clear();
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, true, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // the account was signed out here (its device key gone) before the memory was opened
    let server = FakeServer::default();
    remembered(&server);
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, false, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // the host's sign-out (or the 401 path's) locks the session and forgets the memory
    let server = FakeServer::default();
    let running = remembered(&server);
    assert!(running.signed_out(ACCOUNT).is_some());
    assert!(!running.is_open(ACCOUNT));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // a server that cannot be reached confirms nothing: the memory is kept and not opened
    let mut server = FakeServer::default();
    remembered(&server);
    server.unreachable = true;
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, true, &server, T0 + MINUTE),
        Restore::Unconfirmed(_)
    ));
    assert!(!fresh.is_open(ACCOUNT));
    assert_eq!(kept_files(&client, &keys), (true, true));
    server.unreachable = false;
    assert!(opened(&fresh.restore_at(ACCOUNT, true, &server, T0 + MINUTE)));
}

#[test]
fn device_memory_needs_a_key_store_an_open_key_and_a_time_in_range() {
    let (client, keys) = dirs("refused");
    let server = FakeServer {
        max_minutes: Some(30),
        ..FakeServer::default()
    };
    let in_file = vaults(&client, &keys, KIND_FILE);
    make_key(&in_file);
    assert!(
        in_file.remember_at(ACCOUNT, 10, "d", &server, T0).is_err(),
        "a key in a plain file"
    );
    assert_eq!(in_file.memory_kind(), None);
    assert!(
        Vaults::new(&client).remember_at(ACCOUNT, 10, "d", &server, T0).is_err(),
        "no key store at all"
    );

    let os = vaults(&client, &keys, KIND_OS);
    assert!(
        os.remember_at(ACCOUNT, 10, "d", &server, T0).is_err(),
        "the personal key is locked"
    );
    make_key(&os);
    assert!(os.remember_at(ACCOUNT, 0, "d", &server, T0).is_err());
    assert!(os
        .remember_at(ACCOUNT, MAX_MEMORY_MINUTES + 1, "d", &server, T0)
        .is_err());
    // the server's limit is the shorter
    let answer = os.remember_at(ACCOUNT, 120, "d", &server, T0).unwrap();
    assert_eq!(answer["until"], json!(T0 + 30 * MINUTE));
    // remembering again replaces the memory, and the server's record of the first ends
    os.remember_at(ACCOUNT, 20, "d", &server, T0).unwrap();
    assert_eq!(server.live.borrow().len(), 1);
    // the page's Lock with forget: the session and the memory go
    os.lock(ACCOUNT);
    assert!(os.forget(ACCOUNT).is_some());
    assert_eq!(kept_files(&client, &keys), (false, false));
}

/// A key store that opens a key only after the device's lock, as the phone's.
struct LockedKeys {
    inner: StoreMemoryKeys,
    unlocked: std::sync::atomic::AtomicBool,
}

impl MemoryKeys for LockedKeys {
    fn kind(&self) -> &'static str {
        KIND_DEVICE_LOCK
    }
    fn put(&self, entry: &str, key: &moqui_vault::Key32) -> Result<(), String> {
        self.inner.put(entry, key)
    }
    fn take(&self, entry: &str) -> Result<Option<moqui_vault::Key32>, KeyError> {
        if !self.unlocked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(KeyError::NeedsUnlock);
        }
        self.inner.take(entry)
    }
    fn delete(&self, entry: &str) -> Result<(), String> {
        self.inner.delete(entry)
    }
}

#[test]
fn a_key_that_needs_the_device_lock_waits_for_it() {
    let (client, keys) = dirs("unlock");
    let server = FakeServer::default();
    let os = vaults(&client, &keys, KIND_OS);
    make_key(&os);
    os.remember_at(ACCOUNT, 10, "d", &server, T0).unwrap();
    let phone = Vaults::new(&client);
    phone.set_memory_keys(Box::new(LockedKeys {
        inner: StoreMemoryKeys::new(Box::new(FileKeyStore::new(&keys)), KIND_OS),
        unlocked: std::sync::atomic::AtomicBool::new(false),
    }));
    assert_eq!(
        phone.restore_at(ACCOUNT, true, &server, T0 + MINUTE),
        Restore::NeedsUnlock
    );
    assert_eq!(kept_files(&client, &keys), (true, true), "kept for the unlock");
}

#[test]
fn the_apps_memory_of_a_vault_is_the_modules_not_the_pages() {
    let (client, keys) = dirs("seen");
    let vaults = vaults(&client, &keys, KIND_OS);
    let made = make_key(&vaults);
    let members = json!([{ "user_id": "alice", "role": "manage", "key_id": "pk-alice", "public": made["public"] }]);
    let first = vaults
        .call_at(
            ACCOUNT,
            "key_version_first",
            json!({ "vault_id": "v", "members": members }),
            T0,
        )
        .unwrap();
    let second = vaults
        .call_at(
            ACCOUNT,
            "key_version_next",
            json!({ "entries": [first], "members": members, "version": 1, "revision": 2 }),
            T0,
        )
        .unwrap();
    let seen = vaults
        .call_at(ACCOUNT, "chain_verify", json!({ "entries": [first, second] }), T0)
        .unwrap();
    assert_eq!(seen["revision"], json!(2));

    // the server hands back the older chain and the page remembers nothing: the app's memory refuses it
    let rolled_back = vaults.call_at(
        ACCOUNT,
        "chain_verify",
        json!({ "entries": [first], "remembered": null }),
        T0,
    );
    assert!(rolled_back.unwrap_err().contains("older"));
    // and across a restart of the app
    let again = self::vaults(&client, &keys, KIND_OS);
    let open = json!({ "personal_key_id": "pk-alice", "user_id": "alice", "public": made["public"],
                       "passphrase_wrap": made["passphrase_wrap"], "passphrase": "pass phrase" });
    again.call_at(ACCOUNT, "personal_open", open, T0).unwrap();
    let rolled_back = again.call_at(ACCOUNT, "chain_verify", json!({ "entries": [first] }), T0);
    assert!(rolled_back.unwrap_err().contains("older"));
    assert_eq!(
        again
            .call_at(ACCOUNT, "chain_verify", json!({ "entries": [first, second] }), T0)
            .unwrap()["revision"],
        json!(2)
    );

    // an item's revision lower than one seen is told
    let list = |revision: u64| json!({ "vault_id": "v", "items": [{ "item_id": "i1", "revision": revision }] });
    assert_eq!(
        again.call_at(ACCOUNT, "revisions_seen", list(3), T0).unwrap()["older"],
        json!([])
    );
    assert_eq!(
        again.call_at(ACCOUNT, "revisions_seen", list(2), T0).unwrap()["older"],
        json!(["i1"])
    );
    assert_eq!(
        again.call_at(ACCOUNT, "revisions_seen", list(4), T0).unwrap()["older"],
        json!([])
    );
}

#[test]
fn a_session_locks_when_idle_and_each_account_has_its_own() {
    let (client, keys) = dirs("idle");
    let vaults = vaults(&client, &keys, KIND_OS);
    make_key(&vaults);
    vaults
        .call_at(ACCOUNT, "configure", json!({ "idle_minutes": 5 }), T0)
        .unwrap();
    let status = vaults
        .call_at("example.org-ba9876543210", "status", json!({}), T0)
        .unwrap();
    assert_eq!(status["personal"], Value::Null, "another account's session is its own");
    vaults.sweep_at(T0 + 4 * MINUTE);
    assert!(vaults.is_open(ACCOUNT));
    vaults.sweep_at(T0 + 5 * MINUTE);
    assert!(!vaults.is_open(ACCOUNT));
    assert!(vaults.call_at("../x", "status", json!({}), T0).is_err());
}

#[test]
fn a_sleep_is_the_wall_clock_running_ahead_of_the_monotonic_one() {
    use std::time::Duration;
    let s = Duration::from_secs;
    assert!(!super::slept(Some(s(30)), s(30)));
    assert!(!super::slept(Some(s(80)), s(30)), "a gap of under a minute");
    assert!(super::slept(Some(s(3600)), s(30)));
    assert!(!super::slept(None, s(30)), "the clock set back");
}
