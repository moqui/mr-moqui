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
fn the_md3_files_move_into_the_store_once_and_go() {
    let dir = std::env::temp_dir().join(format!("moqui-client-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = dir.join("cache");
    crate::files::write_private(&cache.join("a/shellData.json"), b"{\"apps\":[]}").unwrap();
    crate::files::write_private(&cache.join("a/qapps_clientConfig.json"), b"{\"old\":true}").unwrap();
    crate::files::write_private(&cache.join("b/shellData.json"), b"{}").unwrap();
    let store = Store::open(&dir, "a", 1 << 20).unwrap();
    // written since the files: not replaced by the old answer
    store
        .put_answer("/qapps/clientConfig", "", "application/json", b"{\"new\":true}")
        .unwrap();
    assert_eq!(migrate(&cache, "a", &store), 1);
    assert_eq!(store.answer("/shellData", "").unwrap().body, b"{\"apps\":[]}");
    assert_eq!(store.answer("/qapps/clientConfig", "").unwrap().body, b"{\"new\":true}");
    assert!(!cache.join("a").exists());
    // once only: nothing left to move
    assert_eq!(migrate(&cache, "a", &store), 0);
    assert_eq!(migrate(&cache, "../b", &store), 0);
    // the other account's stay until it is opened or removed, then the directory goes
    assert!(cache.join("b/shellData.json").exists());
    forget(&cache, "b");
    assert!(!cache.exists());
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
