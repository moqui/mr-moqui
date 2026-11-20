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
use crate::rng::TestRng;

#[test]
fn machine_access_opens_only_its_item_for_its_environment() {
    let mut rng = TestRng::new("machine");
    let machine_key = machine_key_from_crypt_pass(b"the field pass");
    let item_key = Key32::random(&mut rng);
    let sealed = seal(&machine_key, "env-demo", "vault-1", "item-1", &item_key, &mut rng);
    assert_eq!(
        open(&machine_key, "env-demo", "vault-1", "item-1", &sealed).unwrap(),
        item_key
    );
    for (environment, vault, item) in [
        ("env-prod", "vault-1", "item-1"),
        ("env-demo", "vault-2", "item-1"),
        ("env-demo", "vault-1", "item-2"),
    ] {
        assert_eq!(
            open(&machine_key, environment, vault, item, &sealed).unwrap_err(),
            Error::Open
        );
    }
    let other_pass = machine_key_from_crypt_pass(b"another pass");
    assert_eq!(
        open(&other_pass, "env-demo", "vault-1", "item-1", &sealed).unwrap_err(),
        Error::Open
    );
}
