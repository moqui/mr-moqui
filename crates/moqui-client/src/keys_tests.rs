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
