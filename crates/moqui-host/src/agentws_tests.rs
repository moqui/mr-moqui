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

use super::protocol_of;

#[test]
fn the_protocol_of_the_query() {
    assert_eq!(protocol_of(None), Ok(None));
    assert_eq!(protocol_of(Some("a=b")), Ok(None));
    assert_eq!(protocol_of(Some("a=b&protocol=1")), Ok(Some(1)));
    assert_eq!(protocol_of(Some("protocol=x")), Err(()));
}
