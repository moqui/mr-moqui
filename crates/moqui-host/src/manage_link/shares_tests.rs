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

//! The instance's rules of its recovery shares (plans/Vault-Instances.md §6, M6): a holder's key with no pinned
//! approver's check is refused, each share is sealed to one holder only, and a confirmation counts only when the holder
//! signed it for the share the instance sent.

use moqui_vault::approvers::{Approver, ApproverSet};
use moqui_vault::delivery::{self, Delivery};
use moqui_vault::fingerprint::KeyCheck;
use moqui_vault::managed::{self, ShareValue};
use moqui_vault::personal::PersonalKey;
use moqui_vault::rng::OsRng;
use moqui_vault::sign::SigningKey;
use zeroize::Zeroizing;

use super::super::proto;
use super::*;

fn approver_key() -> PersonalKey {
    PersonalKey::generate(&mut OsRng)
}

fn approvers_of(keys: &[&PersonalKey]) -> ApproverSet {
    ApproverSet {
        version: 1,
        approvers: keys
            .iter()
            .enumerate()
            .map(|(n, key)| Approver {
                user_id: format!("approver-{n}"),
                verify_public: *key.public().verify.as_bytes(),
            })
            .collect(),
    }
}

/// A holder's keys as the manager hands them, checked by `checker` as `checked_as`'s key.
fn holder(user_id: &str, key: &PersonalKey, checker: Option<(&PersonalKey, &str)>) -> proto::Holder {
    let public = key.public();
    let checks = checker
        .map(|(checker, checked_as)| {
            let check = KeyCheck::of_personal(
                "approver",
                "pk-approver",
                checker,
                checked_as,
                &format!("pk-{user_id}"),
                &public,
                1_700_000_000_000,
            );
            vec![proto::KeyCheck {
                checker_user_id: check.checker_user_id,
                checker_key_id: check.checker_key_id,
                checked_key_id: check.checked_key_id,
                checked_user_id: check.checked_user_id,
                checked_encrypt: check.checked_encrypt,
                checked_verify: check.checked_verify,
                checked_at: check.checked_at,
                signature: check.signature,
            }]
        })
        .unwrap_or_default();
    proto::Holder {
        user_id: user_id.to_string(),
        personal_key_id: format!("pk-{user_id}"),
        encrypt_public: public.encrypt.as_bytes().to_vec(),
        verify_public: public.verify.as_bytes().to_vec(),
        checks,
    }
}

#[test]
fn a_holders_key_with_no_check_by_a_pinned_approver_is_refused() {
    let approver = approver_key();
    let stranger = approver_key();
    let pinned = approvers_of(&[&approver]);
    let key = approver_key();
    assert!(check_holder(&holder("alice", &key, Some((&approver, "alice"))), &pinned).is_ok());
    // no check at all, a check by a key the instance did not pin (the manager's own), and a check of the key as
    // someone else's are each refused
    for refused in [
        holder("alice", &key, None),
        holder("alice", &key, Some((&stranger, "alice"))),
        holder("alice", &key, Some((&approver, "bob"))),
    ] {
        let err = check_holder(&refused, &pinned).unwrap_err();
        assert!(err.contains("no key check by an approver"), "{err}");
    }
    // a key swapped after its check
    let mut swapped = holder("alice", &key, Some((&approver, "alice")));
    swapped.encrypt_public = approver_key().public().encrypt.as_bytes().to_vec();
    assert!(check_holder(&swapped, &pinned).is_err());
    let reply = proto::HoldersReply {
        holders: vec![
            holder("carol", &approver_key(), Some((&approver, "carol"))),
            holder("dave", &approver_key(), None),
            holder("alice", &key, Some((&approver, "alice"))),
        ],
    };
    // a key given for two holders counts once: no two shares end with one key
    let shared_key = approver_key();
    let twice = proto::HoldersReply {
        holders: vec![
            holder("erin", &shared_key, Some((&approver, "erin"))),
            holder("frank", &shared_key, Some((&approver, "frank"))),
        ],
    };
    assert_eq!(checked_holders(&twice, &pinned).len(), 1);
    let checked: Vec<String> = checked_holders(&reply, &pinned)
        .into_iter()
        .map(|holder| holder.user_id)
        .collect();
    assert_eq!(checked, ["alice", "carol"]);
}

#[test]
fn each_share_is_sealed_to_one_holder_only_and_confirmed_by_that_holder() {
    let approver = approver_key();
    let pinned = approvers_of(&[&approver]);
    let keys: Vec<PersonalKey> = (0..3).map(|_| approver_key()).collect();
    let names = ["alice", "bob", "carol"];
    let reply = proto::HoldersReply {
        holders: names
            .iter()
            .zip(&keys)
            .map(|(name, key)| holder(name, key, Some((&approver, name))))
            .collect(),
    };
    let holders = checked_holders(&reply, &pinned);
    let link = SigningKey::random(&mut OsRng);
    let shares: Vec<Zeroizing<String>> = (1..=3)
        .map(|n| Zeroizing::new(format!("share-{n}-of-the-test")))
        .collect();
    // fewer holders than shares: nothing is sent
    assert!(make_set("prod-1", &link, &shares, &holders[..2], 2).is_err());
    let (request, mut set) = make_set("prod-1", &link, &shares, &holders, 2).unwrap();
    assert_eq!(request.deliveries.len(), 3);
    assert_eq!(set.shares.len(), 3);
    let opened_by = |sent: &proto::ShareDelivery, key: &PersonalKey| {
        let shaped = Delivery {
            delivery_id: sent.delivery_id.clone(),
            recipient_user_id: sent.recipient_user_id.clone(),
            recipient_key_id: sent.recipient_key_id.clone(),
            purpose: sent.purpose.clone(),
            sender_kind: "instance".to_string(),
            sender_id: "prod-1".to_string(),
            sealed: sent.sealed.clone(),
            signature: sent.signature.clone(),
        };
        delivery::open(&shaped, key, &link.verifying_key()).ok()
    };
    let mut seen = Vec::new();
    for (n, sent) in request.deliveries.iter().enumerate() {
        assert_eq!(sent.recipient_user_id, names[n]);
        // its holder opens it, and no other holder does
        for (other, key) in keys.iter().enumerate() {
            assert_eq!(
                opened_by(sent, key).is_some(),
                other == n,
                "share {n} opened by holder {other}"
            );
        }
        let value = ShareValue::parse(std::str::from_utf8(&opened_by(sent, &keys[n]).unwrap()).unwrap()).unwrap();
        assert_eq!(value.index, u32::try_from(n + 1).unwrap());
        assert_eq!((value.of, value.threshold), (3, 2));
        assert!(!seen.contains(&value.share), "two holders got one share");
        seen.push(value.share.clone());
        // the holder's confirmation of what they opened
        if n < 2 {
            let confirmation = managed::confirm_share(keys[n].signing_key(), &value);
            let taken = take_confirmations(
                &mut set,
                "prod-1",
                &proto::ShareConfirmationsReply {
                    confirmations: vec![proto::ShareConfirmation {
                        index: value.index,
                        holder_user_id: names[n].to_string(),
                        signature: confirmation.to_vec(),
                    }],
                },
            );
            assert_eq!(taken, 1);
        }
    }
    assert!(!set.complete(), "an unconfirmed share is reported");
    // a confirmation signed by another key than the holder's does not count
    let forged = managed::confirm_share(
        keys[0].signing_key(),
        &ShareValue {
            instance_id: "prod-1".to_string(),
            instance_name: String::new(),
            set_id: set.set_id.clone(),
            index: 3,
            of: 3,
            threshold: 2,
            share: Zeroizing::new("share-3-of-the-test".to_string()),
        },
    );
    let reply = proto::ShareConfirmationsReply {
        confirmations: vec![proto::ShareConfirmation {
            index: 3,
            holder_user_id: "carol".to_string(),
            signature: forged.to_vec(),
        }],
    };
    assert_eq!(take_confirmations(&mut set, "prod-1", &reply), 0);
    assert!(!set.complete());
    // a replaced holder makes a new set due
    assert!(!holder_replaced(&set, &holders));
    assert!(holder_replaced(&set, &holders[..2]));
}
