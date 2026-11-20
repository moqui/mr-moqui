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

use std::sync::atomic::{AtomicBool, Ordering};

use moqui_client::keys::KeyStore;

use super::{SealedFileStore, Sealer};

/// A stand-in for the Keystore: flips each byte (and XORs it with `mask`, so two keys seal differently), and refuses
/// to open once `lost` (the Keystore's key gone, or the phone locked).
#[derive(Default)]
struct Flip {
    lost: AtomicBool,
    mask: u8,
}

impl Sealer for &Flip {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        if self.lost.load(Ordering::SeqCst) {
            return Err("no such key".to_string());
        }
        Ok(plain.iter().map(|byte| !byte ^ self.mask).collect())
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        if self.lost.load(Ordering::SeqCst) {
            return Err("no such key".to_string());
        }
        Ok(sealed.iter().map(|byte| !(byte ^ self.mask)).collect())
    }
}

#[test]
fn a_key_is_kept_sealed_and_opens_only_with_the_keystores_key() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let flip = Flip::default();
    let store = SealedFileStore::new(&dir, &flip);
    assert_eq!(store.get("server_abc"), None);
    store.set("server_abc", "the-device-key").unwrap();
    assert_eq!(store.get("server_abc").as_deref(), Some("the-device-key"));
    // what is on disk is the sealed bytes, owner-only
    let file = dir.join("server_abc.sealed");
    let on_disk = std::fs::read(&file).unwrap();
    assert_ne!(on_disk, b"the-device-key");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // a Keystore that lost its key: no key, not a panic
    flip.lost.store(true, Ordering::SeqCst);
    assert_eq!(store.get("server_abc"), None);
    flip.lost.store(false, Ordering::SeqCst);
    store.delete("server_abc").unwrap();
    store.delete("server_abc").unwrap();
    assert_eq!(store.get("server_abc"), None);
    // an id that would leave the directory
    assert!(store.set("../escape", "k").is_err());
    assert!(store.set(".hidden", "k").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plain_keys_a_run_without_the_keystore_left_are_sealed_and_removed() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-plain-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let plain = moqui_client::keys::FileKeyStore::new(&dir);
    plain.set("server_abc", "key-one").unwrap();
    plain.set("server_abc.chat-store", "{\"DEV\":\"s\"}").unwrap();
    let flip = Flip::default();
    let store = SealedFileStore::new(&dir, &flip);
    assert_eq!(store.seal_plain_keys(), 2);
    assert_eq!(store.get("server_abc").as_deref(), Some("key-one"));
    assert_eq!(store.get("server_abc.chat-store").as_deref(), Some("{\"DEV\":\"s\"}"));
    assert!(!dir.join("server_abc.key").exists() && !dir.join("server_abc.chat-store.key").exists());
    // nothing left to move
    assert_eq!(store.seal_plain_keys(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

/// SEC-CLI-113: with a key of an unlocked phone, the chat store passphrases are sealed by it (`.unlocked`) and the
/// device keys by the first; one sealed by the first before is moved as it is read; while the phone is locked the
/// passphrases do not open and cannot be written, and the device keys still do; a delete takes both files; and the
/// passphrases are read each time, never kept in memory.
#[test]
fn the_chat_store_passphrases_are_sealed_by_the_key_of_an_unlocked_phone() {
    let dir = std::env::temp_dir().join(format!("moqui-mobile-unlocked-keys-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let first = Flip::default();
    let unlocked = Flip {
        mask: 0x55,
        ..Flip::default()
    };
    // a passphrase sealed before there was a second key
    SealedFileStore::new(&dir, &first)
        .set("server_abc.chat-store", "{\"DEV\":\"old\"}")
        .unwrap();
    assert!(dir.join("server_abc.chat-store.sealed").exists());
    let store = SealedFileStore::new(&dir, &first).with_unlocked(&unlocked);
    assert!(store.fresh("server_abc.chat-store") && !store.fresh("server_abc"));
    assert!(!SealedFileStore::new(&dir, &first).fresh("server_abc.chat-store"));
    // read: it opens, and is moved to the second key
    assert_eq!(store.get("server_abc.chat-store").as_deref(), Some("{\"DEV\":\"old\"}"));
    assert!(!dir.join("server_abc.chat-store.sealed").exists());
    let moved = std::fs::read(dir.join("server_abc.chat-store.unlocked")).unwrap();
    assert_eq!((&unlocked).open(&moved).unwrap(), b"{\"DEV\":\"old\"}");
    assert!(
        (&first).open(&moved).unwrap() != b"{\"DEV\":\"old\"}",
        "sealed by the first key still"
    );
    // a new passphrase goes to the second key, a device key to the first
    store.set("server_abc.chat-store", "{\"DEV\":\"new\"}").unwrap();
    store.set("server_abc", "the-device-key").unwrap();
    assert!(dir.join("server_abc.sealed").exists() && !dir.join("server_abc.unlocked").exists());
    assert_eq!(store.get("server_abc.chat-store").as_deref(), Some("{\"DEV\":\"new\"}"));
    // the phone locked: no passphrase, none written over it, the device key as before
    unlocked.lost.store(true, Ordering::SeqCst);
    assert_eq!(store.get("server_abc.chat-store"), None);
    assert!(store.set("server_abc.chat-store", "{}").is_err());
    assert_eq!(store.get("server_abc").as_deref(), Some("the-device-key"));
    unlocked.lost.store(false, Ordering::SeqCst);
    assert_eq!(store.get("server_abc.chat-store").as_deref(), Some("{\"DEV\":\"new\"}"));
    // a plain passphrase a run without the Keystore left goes to the second key
    moqui_client::keys::FileKeyStore::new(&dir)
        .set("server_def.chat-store", "{}")
        .unwrap();
    assert_eq!(store.seal_plain_keys(), 1);
    assert!(dir.join("server_def.chat-store.unlocked").exists());
    // a delete takes the passphrase in either file
    std::fs::write(dir.join("server_abc.chat-store.sealed"), b"stale").unwrap();
    store.delete("server_abc.chat-store").unwrap();
    assert!(!dir.join("server_abc.chat-store.sealed").exists());
    assert!(!dir.join("server_abc.chat-store.unlocked").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for the Keystore key that needs the device's lock: it opens only once `confirmed`, and opens to nothing
/// once `invalidated` (a new biometric enrolled).
#[derive(Default)]
struct LockKey {
    confirmed: AtomicBool,
    invalidated: AtomicBool,
}

impl super::AuthSealer for std::sync::Arc<LockKey> {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.iter().map(|byte| !byte).collect())
    }

    fn open(&self, sealed: &[u8]) -> Result<Option<Vec<u8>>, String> {
        if self.invalidated.load(Ordering::SeqCst) {
            return Ok(Some(Vec::new()));
        }
        if !self.confirmed.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(sealed.iter().map(|byte| !byte).collect()))
    }
}

/// The server's record of the memories, for the vault (`moqui_client::vault::server`).
#[derive(Default)]
struct Records {
    live: std::sync::Mutex<Vec<String>>,
    revoked: AtomicBool,
}

impl moqui_client::vault::server::MemoryServer for Records {
    fn register(
        &self,
        memory_id: &str,
        _device: &str,
        minutes: u32,
    ) -> Result<i64, moqui_client::vault::server::ServerError> {
        self.live.lock().unwrap().push(memory_id.to_string());
        Ok(T0 + i64::from(minutes) * 60_000)
    }

    fn check(&self, memory_id: &str) -> Result<bool, moqui_client::vault::server::ServerError> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(moqui_client::vault::server::ServerError::Revoked);
        }
        Ok(self.live.lock().unwrap().iter().any(|id| id == memory_id))
    }

    fn forget(&self, memory_id: &str) -> Result<(), moqui_client::vault::server::ServerError> {
        self.live.lock().unwrap().retain(|id| id != memory_id);
        Ok(())
    }
}

const T0: i64 = 1_800_000_000_000;
const ACCOUNT: &str = "example.com-0123456789ab";

/// V5 on the phone: the device memory key is sealed by the Keystore key of the device's lock, so the key opens from
/// memory only once the user confirms; the memory ends at its time limit, when the server no longer takes the device's
/// key, and when the Keystore invalidated its key.
#[test]
fn device_memory_on_the_phone_needs_the_lock_and_ends_at_its_limit_and_on_revocation() {
    use moqui_client::vault::{Restore, Vaults};
    let base = std::env::temp_dir().join(format!("moqui-mobile-vault-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let lock = std::sync::Arc::new(LockKey::default());
    let open_vaults = || {
        let vaults = Vaults::new(&base.join("client"));
        vaults.set_memory_keys(Box::new(super::DeviceLockKeys::new(
            &base.join("vault-keys"),
            std::sync::Arc::clone(&lock),
        )));
        vaults
    };
    let remembered = |server: &Records| {
        let vaults = open_vaults();
        vaults
            .call_at(
                ACCOUNT,
                "personal_make",
                serde_json::json!({ "personal_key_id": "pk-a", "user_id": "a", "passphrase": "pass phrase" }),
                T0,
            )
            .unwrap();
        vaults.remember_at(ACCOUNT, 30, "Pixel", server, T0).unwrap();
        assert_eq!(vaults.memory_kind(), Some("device-lock"));
    };
    let sealed_file = base.join("vault-keys").join(format!("{ACCOUNT}.vault-memory.sealed"));

    let server = Records::default();
    remembered(&server);
    assert!(sealed_file.exists());
    let phone = open_vaults();
    assert_eq!(
        phone.restore_at(ACCOUNT, true, &server, T0 + 60_000),
        Restore::NeedsUnlock
    );
    lock.confirmed.store(true, Ordering::SeqCst);
    assert!(matches!(
        phone.restore_at(ACCOUNT, true, &server, T0 + 60_000),
        Restore::Opened(_)
    ));
    assert!(phone.is_open(ACCOUNT));
    // its time limit: the session opened from it locks, and the sealed key goes
    phone.sweep_at(T0 + 30 * 60_000);
    assert!(!phone.is_open(ACCOUNT));
    assert!(!sealed_file.exists());

    // the server no longer takes the device's key: nothing opens, and the sealed key goes
    let server = Records::default();
    remembered(&server);
    server.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        open_vaults().restore_at(ACCOUNT, true, &server, T0 + 60_000),
        Restore::Ended(_)
    ));
    assert!(!sealed_file.exists());

    // the Keystore invalidated its key: the memory ends
    let server = Records::default();
    remembered(&server);
    lock.invalidated.store(true, Ordering::SeqCst);
    assert!(matches!(
        open_vaults().restore_at(ACCOUNT, true, &server, T0 + 60_000),
        Restore::Ended(_)
    ));
    let _ = std::fs::remove_dir_all(&base);
}
