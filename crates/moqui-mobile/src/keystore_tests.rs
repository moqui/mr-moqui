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

/// A stand-in for the Keystore: flips each byte, and refuses to open once `lost` (the Keystore's key gone).
#[derive(Default)]
struct Flip {
    lost: AtomicBool,
}

impl Sealer for &Flip {
    fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, String> {
        Ok(plain.iter().map(|byte| !byte).collect())
    }

    fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, String> {
        if self.lost.load(Ordering::SeqCst) {
            return Err("no such key".to_string());
        }
        Ok(sealed.iter().map(|byte| !byte).collect())
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
