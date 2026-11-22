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

use super::check_protocol;

#[test]
fn the_protocol_check_follows_t3() {
    assert!(check_protocol(None).is_ok());
    assert!(check_protocol(Some(crate::PROTOCOL)).is_ok());
    assert!(check_protocol(Some(0)).unwrap_err().starts_with("client not supported"));
    assert!(check_protocol(Some(crate::PROTOCOL + 1)).unwrap_err().starts_with("needs a newer server"));
}
