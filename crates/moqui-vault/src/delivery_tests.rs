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
fn a_delivery_opens_only_for_its_recipient_with_a_good_signature() {
    let mut rng = TestRng::new("delivery");
    let alice = PersonalKey::generate(&mut rng);
    let bob = PersonalKey::generate(&mut rng);
    let bob_public = bob.public();
    let to = To {
        user_id: "bob",
        key_id: "pk-b",
        public: &bob_public,
    };
    let sender = Sender {
        kind: "user",
        id: "alice",
        key: alice.signing_key(),
    };
    let delivery = send("d-1", to, "login", b"{\"v\":1}", sender, &mut rng).unwrap();
    let alice_verify = alice.public().verify;
    assert_eq!(&open(&delivery, &bob, &alice_verify).unwrap()[..], b"{\"v\":1}");

    // Not for Alice herself, nor anyone else.
    assert_eq!(open(&delivery, &alice, &alice_verify).unwrap_err(), Error::Open);
    // A wrong or missing signature, or a changed purpose or recipient, is refused before the open.
    let stranger = PersonalKey::generate(&mut rng);
    assert_eq!(
        open(&delivery, &bob, &stranger.public().verify).unwrap_err(),
        Error::Signature
    );
    let unsigned = Delivery {
        signature: Vec::new(),
        ..delivery.clone()
    };
    assert_eq!(open(&unsigned, &bob, &alice_verify).unwrap_err(), Error::Signature);
    let renamed = Delivery {
        purpose: "recovery-share".to_string(),
        ..delivery.clone()
    };
    assert_eq!(open(&renamed, &bob, &alice_verify).unwrap_err(), Error::Signature);
    // A server that seals its own value to Bob and signs it with a key of its own is refused by the key check.
    let forged_sender = Sender {
        kind: "user",
        id: "alice",
        key: stranger.signing_key(),
    };
    let forged = send("d-1", to, "login", b"planted", forged_sender, &mut rng).unwrap();
    assert_eq!(open(&forged, &bob, &alice_verify).unwrap_err(), Error::Signature);
}
