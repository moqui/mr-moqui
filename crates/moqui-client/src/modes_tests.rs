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

use crate::conf::DeviceMode::{Choose, Personal, Shared};

use super::*;

#[test]
fn a_build_of_one_mode_offers_no_choice() {
    assert_eq!(wanted(Personal, Some("shared")), Ok(Some(Personal)));
    assert_eq!(wanted(Shared, None), Ok(Some(Shared)));
    assert_eq!(wanted(Choose, Some("shared")), Ok(Some(Shared)));
    assert_eq!(wanted(Choose, Some(" personal ")), Ok(Some(Personal)));
    assert_eq!(wanted(Choose, None), Ok(None));
    assert_eq!(wanted(Choose, Some("")), Ok(None));
    assert!(wanted(Choose, Some("choose")).is_err());
    assert!(wanted(Choose, Some("kiosk")).is_err());
}

#[test]
fn a_personal_account_stays_personal_and_the_user_is_told() {
    // new accounts take what is asked
    assert_eq!(resolved(None, Some(Shared)), (Some(Shared), false));
    assert_eq!(resolved(None, Some(Personal)), (Some(Personal), false));
    assert_eq!(resolved(None, None), (None, false));
    // personal with shared asked: kept, nothing trimmed, told
    assert_eq!(resolved(Some(Personal), Some(Shared)), (Some(Personal), true));
    assert_eq!(resolved(Some(Personal), None), (Some(Personal), false));
    // shared may become personal
    assert_eq!(resolved(Some(Shared), Some(Personal)), (Some(Personal), false));
}

#[test]
fn only_a_shared_account_is_forgotten_on_sign_out() {
    assert!(forgets_on_sign_out(Some(Shared), Choose));
    assert!(!forgets_on_sign_out(Some(Personal), Choose));
    // a personal account stays so in a shared build
    assert!(!forgets_on_sign_out(Some(Personal), Shared));
    // none: the build's mode when it names one, else kept as before MD4
    assert!(!forgets_on_sign_out(None, Choose));
    assert!(forgets_on_sign_out(None, Shared));
    assert_eq!(effective(None, Personal), Personal);
    assert_eq!(effective(None, Choose), Personal);
}
