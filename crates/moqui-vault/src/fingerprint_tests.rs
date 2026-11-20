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
use crate::sign::SigningKey;

#[test]
fn a_fingerprint_is_six_words_over_both_public_keys() {
    let mut rng = TestRng::new("fingerprint");
    let key = PersonalKey::generate(&mut rng);
    let public = key.public();
    let shown = personal(&public);
    let words: Vec<&str> = shown.split('-').collect();
    assert_eq!(words.len(), 6, "{shown}");
    assert!(words.iter().all(|w| w.len() == 5), "{shown}");
    assert_eq!(personal(&public), shown, "the same key gives the same phrase");

    // Either half changed changes the phrase.
    let other = PersonalKey::generate(&mut rng).public();
    let half_a = PersonalPublic {
        encrypt: other.encrypt,
        ..public
    };
    let half_b = PersonalPublic {
        verify: other.verify,
        ..public
    };
    assert_ne!(personal(&half_a), shown);
    assert_ne!(personal(&half_b), shown);
    assert_ne!(lease_key(&public.verify), personal(&public));
}

#[test]
fn a_key_check_is_signed_and_covers_only_the_key_checked() {
    let mut rng = TestRng::new("fingerprint-check");
    let alice = PersonalKey::generate(&mut rng);
    let bob = PersonalKey::generate(&mut rng).public();
    let check = KeyCheck::of_personal("alice", "pk-a", &alice, "pk-b", &bob, 1_700_000_000_000);
    let alice_verify = alice.public().verify;
    assert!(check.covers_personal(&alice_verify, &bob).is_ok());

    // Bob's key changed: checked again before anything is wrapped to it.
    let new_bob = PersonalKey::generate(&mut rng).public();
    assert!(matches!(
        check.covers_personal(&alice_verify, &new_bob),
        Err(Error::Refused(_))
    ));
    // A row a server wrote, or changed, does not verify.
    let mut planted = check.clone();
    planted.checked_encrypt = new_bob.encrypt.as_bytes().to_vec();
    planted.checked_verify = new_bob.verify.as_bytes().to_vec();
    assert_eq!(
        planted.covers_personal(&alice_verify, &new_bob).unwrap_err(),
        Error::Signature
    );
    let stranger = PersonalKey::generate(&mut rng).public().verify;
    assert_eq!(check.covers_personal(&stranger, &bob).unwrap_err(), Error::Signature);
}

#[test]
fn a_lease_key_check_covers_only_that_lease_key() {
    let mut rng = TestRng::new("fingerprint-lease");
    let alice = PersonalKey::generate(&mut rng);
    let lease = SigningKey::random(&mut rng).verifying_key();
    let check = KeyCheck::of_lease_key("alice", "pk-a", &alice, &lease, 1);
    let alice_verify = alice.public().verify;
    assert!(check.covers_lease_key(&alice_verify, &lease).is_ok());
    let changed = SigningKey::random(&mut rng).verifying_key();
    assert!(matches!(
        check.covers_lease_key(&alice_verify, &changed),
        Err(Error::Refused(_))
    ));
}
