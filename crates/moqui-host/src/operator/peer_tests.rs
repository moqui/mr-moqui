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

use super::{access, resolve_group, Access, Settings};

#[test]
fn root_and_the_operator_group_run_everything_and_the_service_user_only_status() {
    let own = rustix::process::geteuid().as_raw();
    let settings = Settings { group: Some(4242) };
    assert_eq!(access(0, 0, None, &settings), Access::All);
    // the server's own user, even in the operator group
    assert_eq!(access(own, 4242, None, &settings), Access::Status);
    let other = own.wrapping_add(1).max(1);
    assert_eq!(access(other, 4242, None, &settings), Access::All);
    assert_eq!(access(other, 100, None, &settings), Access::Status);
    assert_eq!(access(other, 4242, None, &Settings::default()), Access::Status);
}

#[test]
fn a_group_is_a_number_or_a_name_in_etc_group() {
    assert_eq!(resolve_group("1234"), Some(1234));
    assert_eq!(resolve_group(""), None);
    assert_eq!(resolve_group("no-such-group-of-moqui"), None);
    if std::fs::read_to_string("/etc/group").is_ok_and(|text| text.lines().any(|line| line.starts_with("wheel:"))) {
        assert!(resolve_group("wheel").is_some());
    }
}
