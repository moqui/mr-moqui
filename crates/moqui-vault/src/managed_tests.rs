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
use crate::keys::Key32;
use crate::rng::TestRng;

fn link_key(seed: u8) -> SigningKey {
    SigningKey::from_seed(&Key32::from_bytes([seed; 32]))
}

#[test]
fn a_link_fingerprint_is_the_one_a_tls_handshake_shows() {
    // SHA-256 over the RFC 8410 SubjectPublicKeyInfo of the key, as `tls_pin::public_key_sha256` takes it
    let key = link_key(7);
    let public = *key.verifying_key().as_bytes();
    let mut spki = ED25519_SPKI_HEAD.to_vec();
    spki.extend_from_slice(&public);
    assert_eq!(link_fingerprint(&public), hex(&Sha256::digest(&spki)));
    assert!(pinned_link_key(&public, &link_fingerprint(&public)).is_ok());
    let other = *link_key(8).verifying_key().as_bytes();
    assert!(pinned_link_key(&other, &link_fingerprint(&public)).is_err());
    assert!(pinned_link_key(&public, "").is_err());
}

#[test]
fn transit_opens_only_its_own_instances_wrap_and_answers_only_the_unseal_that_asked() {
    let transit = SecretKey::random(&mut TestRng::new("managed-1"));
    let root = [42_u8; 32];
    let wrapped = wrap_root(
        &transit.public().unwrap(),
        "prod-1",
        &root,
        &mut TestRng::new("managed-2"),
    )
    .unwrap();
    assert_eq!(*open_root(&transit, "prod-1", &wrapped).unwrap(), root);
    // another instance's id, or another instance's transit key, does not open it
    assert!(open_root(&transit, "prod-2", &wrapped).is_err());
    assert!(open_root(&SecretKey::random(&mut TestRng::new("managed-3")), "prod-1", &wrapped).is_err());
    // the answer opens only with the ephemeral key of the unseal that asked
    let ephemeral = SecretKey::random(&mut TestRng::new("managed-4"));
    let sealed = seal_reply(
        &ephemeral.public().unwrap(),
        "prod-1",
        &root,
        &mut TestRng::new("managed-5"),
    )
    .unwrap();
    assert_eq!(*open_reply(&ephemeral, "prod-1", &sealed).unwrap(), root);
    assert!(open_reply(&SecretKey::random(&mut TestRng::new("managed-6")), "prod-1", &sealed).is_err());
    assert!(open_reply(&ephemeral, "prod-2", &sealed).is_err());
}

#[test]
fn an_unsealed_proof_holds_for_its_own_link_only() {
    let key = link_key(3);
    let link = ProofLink {
        instance_id: "prod-1",
        link_fingerprint: "ab",
        generation: 4,
        challenge: b"challenge-of-the-manager",
    };
    let signature = sign_proof(&key, &link);
    assert!(verify_proof(&key.verifying_key(), &link, &signature).is_ok());
    for other in [
        ProofLink { generation: 5, ..link },
        ProofLink {
            challenge: b"another",
            ..link
        },
        ProofLink {
            instance_id: "prod-2",
            ..link
        },
    ] {
        assert!(verify_proof(&key.verifying_key(), &other, &signature).is_err());
    }
    assert!(verify_proof(&link_key(4).verifying_key(), &link, &signature).is_err());
}

#[test]
fn a_share_sealed_for_one_unseal_does_not_open_at_another() {
    let link = link_key(9);
    let fingerprint = link_fingerprint(link.verifying_key().as_bytes());
    let first = SecretKey::random(&mut TestRng::new("unseal-first"));
    let second = SecretKey::random(&mut TestRng::new("unseal-second"));
    let key = UnsealKey::sign(&link, "prod-1", first.public().unwrap().as_bytes(), 2_000);
    key.verify(link.verifying_key().as_bytes(), &fingerprint, 1_000)
        .unwrap();
    let sealed = seal_share(&key, "share-text-3-of-5", &mut TestRng::new("seal-share")).unwrap();
    // what the manager relays is ciphertext: the share is not in it
    assert!(!sealed
        .windows(b"share-text".len())
        .any(|window| window == b"share-text"));
    assert_eq!(
        open_share(&first, "prod-1", &sealed).unwrap().as_str(),
        "share-text-3-of-5"
    );
    assert!(open_share(&second, "prod-1", &sealed).is_err());
    assert!(open_share(&first, "prod-2", &sealed).is_err());
}

#[test]
fn a_holder_takes_an_unseal_key_only_by_the_link_key_they_filed_and_in_time() {
    let link = link_key(9);
    let filed = link_fingerprint(link.verifying_key().as_bytes());
    let unseal = *SecretKey::random(&mut TestRng::new("unseal-a"))
        .public()
        .unwrap()
        .as_bytes();
    let key = UnsealKey::sign(&link, "prod-1", &unseal, 2_000);
    assert!(key.verify(link.verifying_key().as_bytes(), &filed, 1_000).is_ok());
    // a key the manager made in the instance's name, with the manager's own link key and its fingerprint: the holder
    // compares with the fingerprint they filed with their share, so it is refused
    let manager = link_key(10);
    let forged = UnsealKey::sign(&manager, "prod-1", &unseal, 2_000);
    let supplied = link_fingerprint(manager.verifying_key().as_bytes());
    assert!(forged
        .verify(manager.verifying_key().as_bytes(), &supplied, 1_000)
        .is_ok());
    assert!(forged
        .verify(manager.verifying_key().as_bytes(), &filed, 1_000)
        .is_err());
    // another key under the instance's signature, a changed expiry, and an expired key are refused
    let mut swapped = key.clone();
    swapped.unseal_public = *SecretKey::random(&mut TestRng::new("unseal-b"))
        .public()
        .unwrap()
        .as_bytes();
    assert!(swapped.verify(link.verifying_key().as_bytes(), &filed, 1_000).is_err());
    let mut longer = key.clone();
    longer.expires = 9_000;
    assert!(longer.verify(link.verifying_key().as_bytes(), &filed, 1_000).is_err());
    assert!(key.verify(link.verifying_key().as_bytes(), &filed, 2_000).is_err());
}

#[test]
fn a_new_proof_key_is_taken_only_signed_by_the_old_one() {
    let old = link_key(20);
    let new = *link_key(21).verifying_key().as_bytes();
    let signature = sign_rotation(&old, "prod-1", "fp", &new, b"wrap");
    assert!(verify_rotation(&old.verifying_key(), "prod-1", "fp", &new, b"wrap", &signature).is_ok());
    assert!(verify_rotation(&old.verifying_key(), "prod-1", "fp", &new, b"other wrap", &signature).is_err());
    assert!(verify_rotation(&old.verifying_key(), "prod-2", "fp", &new, b"wrap", &signature).is_err());
    let thief = sign_rotation(&link_key(22), "prod-1", "fp", &new, b"wrap");
    assert!(verify_rotation(&old.verifying_key(), "prod-1", "fp", &new, b"wrap", &thief).is_err());
}

#[test]
fn a_confirmation_is_the_holders_for_the_share_the_instance_sent() {
    let holder = link_key(11);
    let value = ShareValue {
        instance_id: "prod-1".to_string(),
        instance_name: "prod 1".to_string(),
        set_id: "set-1".to_string(),
        index: 2,
        of: 5,
        threshold: 3,
        share: Zeroizing::new("the-share".to_string()),
    };
    let parsed = ShareValue::parse(&value.to_text()).unwrap();
    assert_eq!(parsed, value);
    assert_eq!(parsed.item_name(), "Recovery share 2 of 5, prod 1");
    let signature = confirm_share(&holder, &value);
    let digest = share_digest("the-share");
    assert!(verify_confirmation(&holder.verifying_key(), "prod-1", "set-1", 2, &digest, &signature).is_ok());
    assert!(verify_confirmation(&holder.verifying_key(), "prod-1", "set-1", 3, &digest, &signature).is_err());
    assert!(verify_confirmation(
        &holder.verifying_key(),
        "prod-1",
        "set-1",
        2,
        &share_digest("another-share"),
        &signature
    )
    .is_err());
    assert!(verify_confirmation(&link_key(12).verifying_key(), "prod-1", "set-1", 2, &digest, &signature).is_err());
}
