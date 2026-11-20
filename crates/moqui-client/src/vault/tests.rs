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

//! The vault in the apps (V5): device memory ends at its time limit and when the device is revoked, and opens only with
//! the server's live secret; it is offered only with a key store and an open key; sessions are per page and a lock in
//! the middle of an open or a remember leaves nothing; the app's rollback memory is the module's, not the page's.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use moqui_vault::keys::Key32;
use moqui_vault::rng::OsRng;
use serde_json::{json, Value};

use super::memory::{KeyError, MemoryKeys, StoreMemoryKeys, KIND_DEVICE_LOCK, KIND_FILE, KIND_OS};
use super::server::{MemoryServer, Registered, ServerError};
use super::{Restore, Vaults, MAX_IDLE_MINUTES, MAX_MEMORY_MINUTES};
use crate::keys::FileKeyStore;

const ACCOUNT: &str = "example.com-0123456789ab";
const PAGE: &str = "remote-1";
const MINUTE: i64 = 60_000;
const T0: i64 = 1_800_000_000_000;

/// A server's records of memories and their secrets, and whether it takes the device key or answers at all; `on_call`
/// runs inside each register and check, as another thread's lock would.
#[derive(Default)]
struct FakeServer<'a> {
    live: RefCell<BTreeMap<String, [u8; 32]>>,
    revoked: bool,
    unreachable: bool,
    /// The most minutes it allows.
    max_minutes: Option<u32>,
    on_call: Option<Box<dyn Fn() + 'a>>,
}

impl MemoryServer for FakeServer<'_> {
    fn register(
        &self,
        memory_id: &str,
        _key: &str,
        _device_name: &str,
        minutes: u32,
    ) -> Result<Registered, ServerError> {
        if let Some(call) = &self.on_call {
            call();
        }
        let secret = Key32::random(&mut OsRng);
        self.live.borrow_mut().insert(memory_id.to_string(), *secret.as_bytes());
        let minutes = self.max_minutes.map_or(minutes, |max| minutes.min(max));
        Ok(Registered {
            until: T0 + i64::from(minutes) * MINUTE,
            secret,
        })
    }

    fn check(&self, memory_id: &str) -> Result<Option<Key32>, ServerError> {
        if let Some(call) = &self.on_call {
            call();
        }
        if self.unreachable {
            return Err(ServerError::Unreachable("no route".to_string()));
        }
        if self.revoked {
            return Err(ServerError::Revoked);
        }
        Ok(self.live.borrow().get(memory_id).map(|bytes| Key32::from_bytes(*bytes)))
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

/// A new personal key opened in `vaults` for the account's page.
fn make_key(vaults: &Vaults) -> Value {
    let made = vaults
        .call_at(
            ACCOUNT,
            PAGE,
            "personal_make",
            json!({ "personal_key_id": "pk-alice", "user_id": "alice", "passphrase": "pass phrase" }),
            T0,
        )
        .unwrap();
    assert!(vaults.is_open(ACCOUNT, PAGE));
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
    let answer = first
        .remember_at(ACCOUNT, PAGE, 10, "Alice's laptop", &server, T0)
        .unwrap();
    assert_eq!(answer["until"], json!(T0 + 10 * MINUTE));
    assert_eq!(kept_files(&client, &keys), (true, true));
    let file = std::fs::read_to_string(client.join("vault").join(format!("{ACCOUNT}.memory.json"))).unwrap();
    assert!(!file.contains("pass phrase"));

    // the app starts again: the key opens from memory with no passphrase, as the same key
    let again = vaults(&client, &keys, KIND_OS);
    assert!(!again.is_open(ACCOUNT, PAGE));
    let restored = again.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE);
    let Restore::Opened(result) = &restored else {
        panic!("not opened: {restored:?}");
    };
    assert_eq!(result["public"], made["public"]);
    assert!(again.is_open(ACCOUNT, PAGE));
    assert_eq!(again.memories_in_use().len(), 1);

    // at its time the session opened from it locks, and the memory is forgotten with its key
    again.sweep_at(T0 + 10 * MINUTE);
    assert!(!again.is_open(ACCOUNT, PAGE), "the session ends with the memory");
    assert_eq!(kept_files(&client, &keys), (false, false));
    assert_eq!(
        again.restore_at(ACCOUNT, PAGE, true, &server, T0 + 11 * MINUTE),
        Restore::Nothing
    );

    // a time made longer in the file does not open: it is in the wrap's associated data
    let third = vaults(&client, &keys, KIND_OS);
    make_key(&third);
    third
        .remember_at(ACCOUNT, PAGE, 5, "Alice's laptop", &server, T0)
        .unwrap();
    let path = client.join("vault").join(format!("{ACCOUNT}.memory.json"));
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    record["until"] = json!(T0 + 60 * MINUTE);
    std::fs::write(&path, record.to_string()).unwrap();
    let later = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        later.restore_at(ACCOUNT, PAGE, true, &server, T0 + 6 * MINUTE),
        Restore::Ended(_)
    ));
    assert_eq!(kept_files(&client, &keys), (false, false));
}

#[test]
fn device_memory_ends_when_the_device_is_revoked() {
    let (client, keys) = dirs("revoked");
    let remembered = |server: &FakeServer| {
        let vaults = vaults(&client, &keys, KIND_OS);
        make_key(&vaults);
        vaults
            .remember_at(ACCOUNT, PAGE, 60, "Alice's phone", server, T0)
            .unwrap();
        vaults
    };

    // the server no longer takes the device key (revoked while the app was not running): nothing opens, all goes
    let mut server = FakeServer::default();
    remembered(&server);
    server.revoked = true;
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));
    assert!(!fresh.is_open(ACCOUNT, PAGE));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // the user took the device off the server's list: the server has no secret to give
    let server = FakeServer::default();
    remembered(&server);
    server.live.borrow_mut().clear();
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // a server that answers live with another secret opens nothing either: the key is both halves'
    let server = FakeServer::default();
    remembered(&server);
    for secret in server.live.borrow_mut().values_mut() {
        secret[0] ^= 1;
    }
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));

    // the account was signed out here (its device key gone) before the memory was opened
    let server = FakeServer::default();
    remembered(&server);
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, PAGE, false, &server, T0 + MINUTE),
        Restore::Ended(_)
    ));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // the host's sign-out (or the 401 path's) locks the session and forgets the memory
    let server = FakeServer::default();
    let running = remembered(&server);
    assert!(running.signed_out(ACCOUNT).is_some());
    assert!(!running.is_open(ACCOUNT, PAGE));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // the record ends while a session opened from it is open: the check locks it
    let server = FakeServer::default();
    remembered(&server);
    let open = vaults(&client, &keys, KIND_OS);
    assert!(opened(&open.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE)));
    let (account, memory_id) = open.memories_in_use().pop().unwrap();
    open.memory_ended(&account, &memory_id);
    assert!(!open.is_open(ACCOUNT, PAGE));
    assert_eq!(kept_files(&client, &keys), (false, false));

    // a server that cannot be reached confirms nothing: the memory is kept and not opened
    let mut server = FakeServer::default();
    remembered(&server);
    server.unreachable = true;
    let fresh = vaults(&client, &keys, KIND_OS);
    assert!(matches!(
        fresh.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE),
        Restore::Unconfirmed(_)
    ));
    assert!(!fresh.is_open(ACCOUNT, PAGE));
    assert_eq!(kept_files(&client, &keys), (true, true));
    server.unreachable = false;
    assert!(opened(&fresh.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE)));
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
        in_file.remember_at(ACCOUNT, PAGE, 10, "d", &server, T0).is_err(),
        "a key in a plain file"
    );
    assert_eq!(in_file.memory_kind(), None);
    assert!(
        Vaults::new(&client)
            .remember_at(ACCOUNT, PAGE, 10, "d", &server, T0)
            .is_err(),
        "no key store at all"
    );

    let os = vaults(&client, &keys, KIND_OS);
    assert!(
        os.remember_at(ACCOUNT, PAGE, 10, "d", &server, T0).is_err(),
        "the personal key is locked"
    );
    make_key(&os);
    assert!(
        os.remember_at(ACCOUNT, "remote-2", 10, "d", &server, T0).is_err(),
        "another page's key"
    );
    assert!(os.remember_at(ACCOUNT, PAGE, 0, "d", &server, T0).is_err());
    assert!(os
        .remember_at(ACCOUNT, PAGE, MAX_MEMORY_MINUTES + 1, "d", &server, T0)
        .is_err());
    // the server's limit is the shorter
    let answer = os.remember_at(ACCOUNT, PAGE, 120, "d", &server, T0).unwrap();
    assert_eq!(answer["until"], json!(T0 + 30 * MINUTE));
    // remembering again replaces the memory, and the server's record of the first ends
    os.remember_at(ACCOUNT, PAGE, 20, "d", &server, T0).unwrap();
    assert_eq!(server.live.borrow().len(), 1);
    // the page's Lock with forget: the session and the memory go
    os.lock_page(ACCOUNT, PAGE);
    assert!(os.forget(ACCOUNT).is_some());
    assert_eq!(kept_files(&client, &keys), (false, false));
}

/// A lock while the server answers leaves nothing: a remember keeps no memory and ends the server's record, and an
/// open from memory leaves no session behind.
#[test]
fn a_lock_in_the_middle_leaves_nothing_behind() {
    let (client, keys) = dirs("race");
    let vaults = vaults(&client, &keys, KIND_OS);
    make_key(&vaults);
    let locking = FakeServer {
        on_call: Some(Box::new(|| vaults.lock_page(ACCOUNT, PAGE))),
        ..FakeServer::default()
    };
    assert!(vaults.remember_at(ACCOUNT, PAGE, 10, "d", &locking, T0).is_err());
    assert!(locking.live.borrow().is_empty(), "the server's record ended");
    assert_eq!(kept_files(&client, &keys), (false, false));

    let server = FakeServer::default();
    make_key(&vaults);
    vaults.remember_at(ACCOUNT, PAGE, 10, "d", &server, T0).unwrap();
    vaults.lock_page(ACCOUNT, PAGE);
    let locking = FakeServer {
        live: RefCell::new(server.live.borrow().clone()),
        on_call: Some(Box::new(|| vaults.lock(ACCOUNT))),
        ..FakeServer::default()
    };
    assert!(matches!(
        vaults.restore_at(ACCOUNT, PAGE, true, &locking, T0 + MINUTE),
        Restore::Unconfirmed(_)
    ));
    assert!(!vaults.is_open(ACCOUNT, PAGE));
    assert_eq!(kept_files(&client, &keys), (true, true), "the memory itself is kept");
}

/// A key store that opens a key only once the user confirms with the device's lock, as the phone's.
struct LockedKeys {
    inner: StoreMemoryKeys,
    unlocked: std::sync::atomic::AtomicBool,
}

impl MemoryKeys for LockedKeys {
    fn kind(&self) -> &'static str {
        KIND_DEVICE_LOCK
    }
    fn put(&self, entry: &str, key: &Key32) -> Result<(), String> {
        self.inner.put(entry, key)
    }
    fn take(&self, entry: &str) -> Result<Option<Key32>, KeyError> {
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
    os.remember_at(ACCOUNT, PAGE, 10, "d", &server, T0).unwrap();
    let phone = Vaults::new(&client);
    phone.set_memory_keys(Box::new(LockedKeys {
        inner: StoreMemoryKeys::new(Box::new(FileKeyStore::new(&keys)), KIND_OS),
        unlocked: std::sync::atomic::AtomicBool::new(false),
    }));
    assert_eq!(
        phone.restore_at(ACCOUNT, PAGE, true, &server, T0 + MINUTE),
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
            PAGE,
            "key_version_first",
            json!({ "vault_id": "v", "members": members }),
            T0,
        )
        .unwrap();
    let second = vaults
        .call_at(
            ACCOUNT,
            PAGE,
            "key_version_next",
            json!({ "entries": [first], "members": members, "version": 1, "revision": 2 }),
            T0,
        )
        .unwrap();
    let seen = vaults
        .call_at(ACCOUNT, PAGE, "chain_verify", json!({ "entries": [first, second] }), T0)
        .unwrap();
    assert_eq!(seen["revision"], json!(2));

    // the server hands back the older chain and the page remembers nothing: the app's memory refuses it
    let rolled_back = vaults.call_at(
        ACCOUNT,
        PAGE,
        "chain_verify",
        json!({ "entries": [first], "remembered": null }),
        T0,
    );
    assert!(rolled_back.unwrap_err().contains("older"));
    // and across a restart of the app
    let again = self::vaults(&client, &keys, KIND_OS);
    let open = json!({ "personal_key_id": "pk-alice", "user_id": "alice", "public": made["public"],
                       "passphrase_wrap": made["passphrase_wrap"], "passphrase": "pass phrase" });
    again.call_at(ACCOUNT, PAGE, "personal_open", open, T0).unwrap();
    let rolled_back = again.call_at(ACCOUNT, PAGE, "chain_verify", json!({ "entries": [first] }), T0);
    assert!(rolled_back.unwrap_err().contains("older"));
    assert_eq!(
        again
            .call_at(ACCOUNT, PAGE, "chain_verify", json!({ "entries": [first, second] }), T0)
            .unwrap()["revision"],
        json!(2)
    );

    // an item's revision lower than one seen is told, and never lowers the one kept
    let list = |revision: u64| json!({ "vault_id": "v", "items": [{ "item_id": "i1", "revision": revision }] });
    assert_eq!(
        again.call_at(ACCOUNT, PAGE, "revisions_seen", list(3), T0).unwrap()["older"],
        json!([])
    );
    assert_eq!(
        again.call_at(ACCOUNT, PAGE, "revisions_seen", list(2), T0).unwrap()["older"],
        json!(["i1"])
    );
    assert_eq!(
        again.call_at(ACCOUNT, PAGE, "revisions_seen", list(2), T0).unwrap()["older"],
        json!(["i1"])
    );
    assert_eq!(
        again.call_at(ACCOUNT, PAGE, "revisions_seen", list(4), T0).unwrap()["older"],
        json!([])
    );
    // item_open of an older revision is refused by the app, before the module opens anything
    let older = json!({ "vault_id": "v", "item_id": "i1", "revision": 3, "key_version": 1,
                        "wrapped_item_key": "00", "sealed_body": "00" });
    let refused = again.call_at(ACCOUNT, PAGE, "item_open", older, T0).unwrap_err();
    assert!(refused.contains("older revision"), "{refused}");
}

#[test]
fn sessions_are_per_page_and_lock_when_idle() {
    let (client, keys) = dirs("idle");
    let vaults = vaults(&client, &keys, KIND_OS);
    make_key(&vaults);
    assert_eq!(
        vaults
            .call_at(ACCOUNT, PAGE, "configure", json!({ "idle_minutes": 600 }), T0)
            .unwrap()["idle_minutes"],
        json!(MAX_IDLE_MINUTES),
        "a page cannot keep the key open past the cap"
    );
    vaults
        .call_at(ACCOUNT, PAGE, "configure", json!({ "idle_minutes": 5 }), T0)
        .unwrap();
    let other_page = vaults.call_at(ACCOUNT, "remote-2", "status", json!({}), T0).unwrap();
    assert_eq!(
        other_page["personal"],
        Value::Null,
        "another page of the account has a session of its own"
    );
    let other = vaults
        .call_at("example.org-ba9876543210", PAGE, "status", json!({}), T0)
        .unwrap();
    assert_eq!(other["personal"], Value::Null, "another account's session is its own");
    vaults.sweep_at(T0 + 4 * MINUTE);
    assert!(vaults.is_open(ACCOUNT, PAGE));
    vaults.sweep_at(T0 + 5 * MINUTE);
    assert!(!vaults.is_open(ACCOUNT, PAGE));
    make_key(&vaults);
    vaults.lock_label(PAGE);
    assert!(!vaults.is_open(ACCOUNT, PAGE), "the page left the vault screens");
    assert!(vaults.call_at("../x", PAGE, "status", json!({}), T0).is_err());
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
