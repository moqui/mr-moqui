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
fn the_pass_authority_serves_plain_h2c_only_on_loopback_unless_told() {
    let public: SocketAddr = "0.0.0.0:4317".parse().unwrap();
    let loopback: SocketAddr = "127.0.0.1:4317".parse().unwrap();
    // with the property unset, as in every other test of this binary
    if moqui_lib::env::get_prop_or_env(crate::pass_authority::INSECURE_PROPERTY).trim() != "true" {
        assert!(!pass_allowed(public, false, false));
    }
    assert!(pass_allowed(public, true, false));
    assert!(pass_allowed(loopback, false, false));
    assert!(pass_allowed("[::1]:4317".parse().unwrap(), false, false));
}
