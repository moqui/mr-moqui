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
fn debug_never_shows_the_key() {
    let key = Key32::from_bytes([0xab; 32]);
    let shown = format!("{key:?}");
    assert_eq!(shown, "Key32(redacted)");
    assert!(!shown.to_lowercase().contains("ab"));
}

#[test]
fn from_slice_takes_only_32_bytes() {
    assert!(Key32::from_slice(&[1; 31]).is_err());
    assert!(Key32::from_slice(&[1; 33]).is_err());
    assert_eq!(Key32::from_slice(&[1; 32]).unwrap(), Key32::from_bytes([1; 32]));
}

#[test]
fn random_keys_differ_and_zeroize_clears() {
    let mut rng = TestRng::new("keys");
    let a = Key32::random(&mut rng);
    let b = Key32::random(&mut rng);
    assert_ne!(a, b);
    let mut c = a.clone();
    c.zeroize();
    assert_eq!(c.as_bytes(), &[0; 32]);
}
