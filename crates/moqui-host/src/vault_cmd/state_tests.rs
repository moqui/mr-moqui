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

use serde_json::json;

use super::State;

/// The state keeps the highest entry per server and vault, survives a write and a read, and a file that does not
/// read is an empty state.
#[test]
fn the_state_keeps_the_highest_entry_and_reads_back() {
    let dir = std::env::temp_dir().join(format!("moqui-vault-state-{}", std::process::id()));
    let path = dir.join("state.json");
    let mut state = State::read(&path);
    assert_eq!(state.remembered("https://a", "v1"), serde_json::Value::Null);
    let two = json!({ "version": 2, "revision": 1, "member_list_hash": "aa" });
    state.remember("https://a", "v1", &two);
    state.remember(
        "https://a",
        "v1",
        &json!({ "version": 1, "revision": 3, "member_list_hash": "bb" }),
    );
    assert_eq!(state.remembered("https://a", "v1"), two);
    assert_eq!(state.remembered("https://b", "v1"), serde_json::Value::Null);
    state.write(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        // a file another tool left readable is made the owner's alone at the next write
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        state.write(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(State::read(&path), state);
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(State::read(&path), State::default());
    let _ = std::fs::remove_dir_all(&dir);
}
