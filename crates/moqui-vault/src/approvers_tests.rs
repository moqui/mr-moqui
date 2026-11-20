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

use super::{Approver, ApproverSet};
use crate::keys::Key32;
use crate::sign::SigningKey;
use crate::Error;

fn key(seed: u8) -> SigningKey {
    SigningKey::from_seed(&Key32::from_bytes([seed; 32]))
}

fn set(version: u64, keys: &[(&str, &SigningKey)]) -> ApproverSet {
    ApproverSet {
        version,
        approvers: keys
            .iter()
            .map(|(user, key)| Approver {
                user_id: (*user).to_string(),
                verify_public: *key.verifying_key().as_bytes(),
            })
            .collect(),
    }
}

#[test]
fn a_new_set_signed_by_a_pinned_approver_is_taken_and_any_other_is_refused() {
    let (alice, bob, mallory) = (key(1), key(2), key(3));
    let pinned = set(1, &[("alice", &alice)]);
    let next = set(2, &[("alice", &alice), ("bob", &bob)]);
    let signature = next.sign("prod-1", "fp-1", &alice);
    next.verify_update("prod-1", "fp-1", &pinned, alice.verifying_key().as_bytes(), &signature)
        .unwrap();
    // the order the approvers are listed in does not change what is signed
    let reordered = set(2, &[("bob", &bob), ("alice", &alice)]);
    reordered
        .verify_update("prod-1", "fp-1", &pinned, alice.verifying_key().as_bytes(), &signature)
        .unwrap();

    // a signer the instance did not pin (the manager's own key)
    let forged = next.sign("prod-1", "fp-1", &mallory);
    assert_eq!(
        next.verify_update("prod-1", "fp-1", &pinned, mallory.verifying_key().as_bytes(), &forged),
        Err(Error::Refused("the signer is not an approver this instance pinned"))
    );
    // a key put in after the approver signed
    let swapped = set(2, &[("alice", &alice), ("bob", &mallory)]);
    assert_eq!(
        swapped.verify_update("prod-1", "fp-1", &pinned, alice.verifying_key().as_bytes(), &signature),
        Err(Error::Signature)
    );
    // another manager's instance of the same id (another link key)
    assert_eq!(
        next.verify_update("prod-1", "fp-2", &pinned, alice.verifying_key().as_bytes(), &signature),
        Err(Error::Signature)
    );
    // another instance's set, an old version, and no approver at all
    assert_eq!(
        next.verify_update("prod-2", "fp-1", &pinned, alice.verifying_key().as_bytes(), &signature),
        Err(Error::Signature)
    );
    let old = set(1, &[("bob", &bob)]);
    let old_signature = old.sign("prod-1", "fp-1", &alice);
    assert!(matches!(
        old.verify_update(
            "prod-1",
            "fp-1",
            &pinned,
            alice.verifying_key().as_bytes(),
            &old_signature
        ),
        Err(Error::Refused(_))
    ));
    let empty = set(3, &[]);
    let empty_signature = empty.sign("prod-1", "fp-1", &alice);
    assert!(matches!(
        empty.verify_update(
            "prod-1",
            "fp-1",
            &pinned,
            alice.verifying_key().as_bytes(),
            &empty_signature
        ),
        Err(Error::Refused(_))
    ));
}
