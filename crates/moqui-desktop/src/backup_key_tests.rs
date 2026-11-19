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

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write as _;

use super::*;

/// A store of secrets in memory, which can be made to fail its reads.
#[derive(Default)]
struct Memory {
    secrets: RefCell<HashMap<String, String>>,
    unreadable: bool,
}

impl SecretStore for Memory {
    fn read(&self, name: &str) -> Result<Option<String>, String> {
        if self.unreadable {
            return Err("the key store did not answer".to_string());
        }
        Ok(self.secrets.borrow().get(name).cloned())
    }

    fn write(&self, name: &str, value: &str) -> Result<(), String> {
        self.secrets.borrow_mut().insert(name.to_string(), value.to_string());
        Ok(())
    }
}

/// SEC-CLI-114: the backup key is made once and then read back the same; a store that does not answer, or holds a key
/// that does not read, is an error, and no new key is written over it.
#[test]
fn the_backup_key_is_made_once_and_never_made_again_over_one_not_read() {
    let store = Memory::default();
    let first = key_in(&store).unwrap();
    assert_eq!(store.secrets.borrow().get(ENTRY), Some(&first.secret_text()));
    assert_eq!(key_in(&store).unwrap().recipient(), first.recipient());

    let unreadable = Memory {
        unreadable: true,
        ..Memory::default()
    };
    assert!(key_in(&unreadable).is_err());
    assert!(
        unreadable.secrets.borrow().is_empty(),
        "a key was made over one not read"
    );

    let garbled = Memory::default();
    garbled.write(ENTRY, "not a key").unwrap();
    assert!(key_in(&garbled).is_err());
    assert_eq!(
        garbled.secrets.borrow().get(ENTRY).map(String::as_str),
        Some("not a key")
    );
}

#[cfg(unix)]
/// The key file reads back, as `age` would read it, and is the user's alone; the key opens a backup sealed to it and
/// no other, and a plain dump is not one it opens.
#[test]
fn the_key_file_reads_back_and_the_key_opens_its_backups_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let identity = Identity::generate().unwrap();
    let text = key_file_text(&identity, Timestamp::UNIX_EPOCH);
    assert!(text.contains(&format!("# public key: {}\n", identity.recipient())));
    assert!(
        text.lines().filter(|line| !line.starts_with('#')).count() == 1,
        "{text}"
    );
    assert_eq!(identity_of(&text).unwrap().recipient(), identity.recipient());
    assert!(identity_of("# no key here\n").is_err());

    let dir = std::env::temp_dir().join(format!("moqui-backup-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("MR-Moqui-backup-key.txt");
    save_key_file(&identity, &file).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let read = identity_of(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(read.recipient(), identity.recipient());

    let sealed = dir.join("moqui-20261119T020000Z-manual.dump.age");
    let mut writer = moqui_lib::age::Encryptor::new(Vec::new(), &identity.recipient()).unwrap();
    writer.write_all(b"PGDMP").unwrap();
    std::fs::write(&sealed, writer.finish().unwrap()).unwrap();
    assert!(opens(&identity, &sealed));
    assert!(!opens(&Identity::generate().unwrap(), &sealed));
    let plain = dir.join("moqui-20261119T020000Z-manual.dump");
    std::fs::write(&plain, b"PGDMP").unwrap();
    assert!(!opens(&identity, &plain));
    assert!(is_sealed("moqui-20261119T020000Z-manual.dump.age") && !is_sealed("moqui-20261119T020000Z-manual.dump"));
    let _ = std::fs::remove_dir_all(&dir);
}
