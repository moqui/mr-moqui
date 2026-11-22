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

use super::{command_types, valid_id};

#[test]
fn command_ids_are_short_plain_ids() {
    assert!(valid_id("0b9c6c4e-2f1d-4a8e-9a57-3f0f5f6a1c2b"));
    assert!(valid_id("cmd_1.2"));
    assert!(!valid_id(""));
    assert!(!valid_id("has space"));
    assert!(!valid_id(&"x".repeat(41)));
}

#[test]
fn every_command_type_is_named_once() {
    let mut names: Vec<&str> = command_types().iter().map(|kind| kind.name).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count);
    assert!(names.contains(&"thread.create"));
}
