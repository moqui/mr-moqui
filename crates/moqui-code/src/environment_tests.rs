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

use super::{capabilities, new_id, read_file, valid, write_file};

#[test]
fn a_new_id_is_valid_and_the_cache_file_round_trips() {
    let id = new_id().unwrap();
    assert!(valid(&id), "{id}");
    assert!(!valid("env_has space"));
    assert!(!valid("nope"));
    let dir = std::env::temp_dir().join(format!("agent-code-env-{}", std::process::id()));
    assert_eq!(read_file(&dir), None);
    write_file(&dir, &id);
    assert_eq!(read_file(&dir), Some(id));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_built_capabilities_answer_true() {
    let caps = capabilities();
    assert_eq!(caps.get("shellStream"), Some(&serde_json::json!(true)));
    assert_eq!(caps.get("pullRequests"), Some(&serde_json::json!(false)));
}
