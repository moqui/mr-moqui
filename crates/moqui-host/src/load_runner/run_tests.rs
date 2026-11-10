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

fn uses(kinds: &[&str]) -> Vec<String> {
    kinds.iter().map(|kind| (*kind).to_string()).collect()
}

#[test]
fn a_plan_uses_what_its_target_supplies() {
    assert_eq!(check_uses(&uses(&["service"]), TargetKind::Service), Ok(()));
    assert_eq!(check_uses(&uses(&["http"]), TargetKind::This), Ok(()));
    assert_eq!(check_uses(&uses(&["http"]), TargetKind::Url), Ok(()));
    let err = check_uses(&uses(&["service"]), TargetKind::This).unwrap_err();
    assert!(err.contains("service target"), "{err}");
    assert!(check_uses(&uses(&["service"]), TargetKind::Url).is_err());
    let err = check_uses(&uses(&["http"]), TargetKind::Service).unwrap_err();
    assert!(err.contains("HTTP"), "{err}");
    assert!(check_uses(&uses(&["service", "http"]), TargetKind::Service).is_err());
    assert!(check_uses(&uses(&["ssh"]), TargetKind::This).is_err());
}

#[test]
fn an_unknown_run_is_not_running_here() {
    assert!(find("20261110-100000-1").is_none());
    assert!(stop("20261110-100000-1", true).is_none());
    assert!(wait_for_end("20261110-100000-1", Duration::from_millis(1)));
}
