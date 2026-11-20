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
fn the_passphrase_opens_and_a_wrong_one_fails() {
    let mut rng = TestRng::new("personal-pass");
    let key = PersonalKey::generate(&mut rng);
    let public = key.public();
    let wrap = wrap_with_passphrase(&key, "pk-1", b"a long key passphrase", &mut rng).unwrap();
    assert_eq!(wrap.params, Argon2Params::TARGET);

    let opened = open_with_passphrase(&wrap, "pk-1", &public, b"a long key passphrase", &mut rng).unwrap();
    assert_eq!(opened.key.public(), public);
    assert!(opened.rewrapped.is_none(), "a wrap at the target is kept");

    let wrong = open_with_passphrase(&wrap, "pk-1", &public, b"a long key passphrasE", &mut rng);
    assert_eq!(wrong.unwrap_err(), Error::Open);
}

#[test]
fn a_wrap_under_the_target_opens_and_is_wrapped_again() {
    let mut rng = TestRng::new("personal-rewrap");
    let key = PersonalKey::generate(&mut rng);
    let public = key.public();
    // Written at the floor; the target is raised one pass above it.
    let wrap = wrap_with_passphrase(&key, "pk-1", b"passphrase", &mut rng).unwrap();
    let raised = Argon2Params {
        t: Argon2Params::FLOOR.t + 1,
        ..Argon2Params::FLOOR
    };
    let opened = open_with_passphrase_to(&wrap, "pk-1", &public, b"passphrase", raised, &mut rng).unwrap();
    let rewrapped = opened.rewrapped.expect("a wrap under the target is made again");
    assert_eq!(rewrapped.params, raised);
    let again = open_with_passphrase_to(&rewrapped, "pk-1", &public, b"passphrase", raised, &mut rng).unwrap();
    assert!(again.rewrapped.is_none());
    assert_eq!(again.key.public(), public);
}

#[test]
fn stored_parameters_outside_the_floor_and_ceiling_are_refused() {
    let mut rng = TestRng::new("personal-params");
    let key = PersonalKey::generate(&mut rng);
    let public = key.public();
    let wrap = PasswordWrap {
        params: Argon2Params {
            m_kib: 1024,
            t: 1,
            p: 1,
        },
        salt: [0; 16],
        sealed: vec![0; 60],
    };
    let under = open_with_passphrase(&wrap, "pk-1", &public, b"x", &mut rng);
    assert!(matches!(under, Err(Error::Params(_))));
    let wrap = PasswordWrap {
        params: Argon2Params {
            m_kib: 4 << 20,
            t: 3,
            p: 1,
        },
        ..wrap
    };
    let over = open_with_passphrase(&wrap, "pk-1", &public, b"x", &mut rng);
    assert!(matches!(over, Err(Error::Params(_))));
}

#[test]
fn the_recovery_code_opens_and_binds_the_key_id_and_public_keys() {
    let mut rng = TestRng::new("personal-code");
    let key = PersonalKey::generate(&mut rng);
    let public = key.public();
    let code = new_recovery_code(&mut rng);
    assert_eq!(code.replace('-', "").len(), 52);
    let sealed = wrap_with_recovery_code(&key, "pk-1", &code, &mut rng).unwrap();
    let opened = open_with_recovery_code(&sealed, "pk-1", &public, &code.to_lowercase()).unwrap();
    assert_eq!(opened.public(), public);

    let other_code = new_recovery_code(&mut rng);
    assert_eq!(
        open_with_recovery_code(&sealed, "pk-1", &public, &other_code).unwrap_err(),
        Error::Open
    );
    assert_eq!(
        open_with_recovery_code(&sealed, "pk-2", &public, &code).unwrap_err(),
        Error::Open
    );
    // A server that swaps the stored public keys gets a wrap that does not open.
    let swapped = PersonalKey::generate(&mut rng).public();
    assert_eq!(
        open_with_recovery_code(&sealed, "pk-1", &swapped, &code).unwrap_err(),
        Error::Open
    );
    assert!(open_with_recovery_code(&sealed, "pk-1", &public, "short").is_err());
}

#[test]
fn public_keys_round_trip_as_64_bytes() {
    let mut rng = TestRng::new("personal-public");
    let public = PersonalKey::generate(&mut rng).public();
    assert_eq!(PersonalPublic::from_bytes(&public.to_bytes()).unwrap(), public);
    assert!(PersonalPublic::from_bytes(&[0u8; 63]).is_err());
    let shown = format!("{:?}", PersonalKey::generate(&mut rng));
    assert!(shown.contains("redacted"), "{shown}");
}
