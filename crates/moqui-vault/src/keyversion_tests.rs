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
use crate::vault::{self, Recipient, Signer};

struct People {
    alice: PersonalKey,
    bob: PersonalKey,
    carol: PersonalKey,
}

fn people() -> People {
    let mut rng = TestRng::new("keyversion-people");
    People {
        alice: PersonalKey::generate(&mut rng),
        bob: PersonalKey::generate(&mut rng),
        carol: PersonalKey::generate(&mut rng),
    }
}

fn member(user: &str, role: Role, key: &PersonalKey) -> Member {
    Member {
        user_id: user.to_string(),
        role,
        key_id: format!("pk-{user}"),
        public: key.public(),
    }
}

/// Alice makes the vault with Bob as a reader (1.1), adds Carol (1.2), then removes Bob (2.1).
fn chain(p: &People) -> Vec<KeyVersion> {
    let alice = member("alice", Role::Manage, &p.alice);
    let bob = member("bob", Role::Read, &p.bob);
    let carol = member("carol", Role::Write, &p.carol);
    vec![
        KeyVersion::sign(
            "v",
            1,
            1,
            vec![alice.clone(), bob.clone()],
            "alice",
            "pk-alice",
            &p.alice,
        ),
        KeyVersion::sign(
            "v",
            1,
            2,
            vec![alice.clone(), bob, carol.clone()],
            "alice",
            "pk-alice",
            &p.alice,
        ),
        KeyVersion::sign("v", 2, 1, vec![carol, alice], "alice", "pk-alice", &p.alice),
    ]
}

fn refused(result: Result<Chain<'_>, Error>) -> Error {
    result.expect_err("the chain is refused")
}

#[test]
fn a_chain_signed_by_managers_and_anchored_in_a_checked_key_is_accepted() {
    let p = people();
    let entries = chain(&p);
    let verified = Chain::verify(&entries, &[p.alice.public().verify]).unwrap();
    assert_eq!((verified.latest().version, verified.latest().revision), (2, 1));
}

#[test]
fn a_chain_with_no_checked_signer_is_refused() {
    let p = people();
    let entries = chain(&p);
    assert!(matches!(refused(Chain::verify(&entries, &[])), Error::Refused(_)));
    // Carol is a member, but never signed: checking her key does not place the signer.
    assert!(matches!(
        refused(Chain::verify(&entries, &[p.carol.public().verify])),
        Error::Refused(_)
    ));
}

#[test]
fn a_version_signed_by_a_member_without_manage_is_refused() {
    let p = people();
    let mut entries = chain(&p);
    let members = entries[1].members.clone();
    entries[1] = KeyVersion::sign("v", 1, 2, members, "bob", "pk-bob", &p.bob);
    let checked = [p.alice.public().verify, p.bob.public().verify];
    assert!(matches!(
        refused(Chain::verify(&entries[..2], &checked)),
        Error::Refused(_)
    ));
}

#[test]
fn a_server_cannot_add_its_own_member_or_key() {
    let mut rng = TestRng::new("keyversion-server");
    let p = people();
    let server = PersonalKey::generate(&mut rng);
    let checked = [p.alice.public().verify];

    // A member put into a signed list after signing.
    let mut entries = chain(&p);
    entries[2].members.push(member("mallory", Role::Manage, &server));
    assert_eq!(refused(Chain::verify(&entries, &checked)), Error::Signature);

    // An entry signed by a key that is not the one the list holds for its signer.
    let mut entries = chain(&p);
    let members = entries[2].members.clone();
    entries[2] = KeyVersion::sign("v", 2, 1, members, "alice", "pk-alice", &server);
    assert_eq!(refused(Chain::verify(&entries, &checked)), Error::Signature);

    // A whole chain the server made with a maker key of its own: valid signatures, but no key the user checked.
    let fake_maker = member("alice", Role::Manage, &server);
    let fake = vec![KeyVersion::sign(
        "v",
        1,
        1,
        vec![fake_maker],
        "alice",
        "pk-alice",
        &server,
    )];
    assert!(matches!(refused(Chain::verify(&fake, &checked)), Error::Refused(_)));

    // A signer who is not in the list at all.
    let mut entries = chain(&p);
    let members = entries[1].members.clone();
    entries[1] = KeyVersion::sign("v", 1, 2, members, "mallory", "pk-mallory", &server);
    assert!(matches!(refused(Chain::verify(&entries, &checked)), Error::Refused(_)));
}

#[test]
fn entries_out_of_order_or_of_another_vault_are_refused() {
    let p = people();
    let checked = [p.alice.public().verify];
    let entries = chain(&p);
    let skipped = vec![entries[0].clone(), entries[2].clone(), entries[1].clone()];
    assert!(matches!(refused(Chain::verify(&skipped, &checked)), Error::Refused(_)));
    assert!(matches!(
        refused(Chain::verify(&entries[1..], &checked)),
        Error::Refused(_)
    ));
    let alice = member("alice", Role::Manage, &p.alice);
    let other = KeyVersion::sign("w", 1, 2, vec![alice], "alice", "pk-alice", &p.alice);
    let mixed = vec![entries[0].clone(), other];
    assert!(matches!(refused(Chain::verify(&mixed, &checked)), Error::Refused(_)));
    assert!(matches!(refused(Chain::verify(&[], &checked)), Error::Refused(_)));
}

#[test]
fn a_wrap_is_accepted_only_from_a_manager_to_a_current_member() {
    let mut rng = TestRng::new("keyversion-wrap");
    let p = people();
    let entries = chain(&p);
    let verified = Chain::verify(&entries, &[p.alice.public().verify]).unwrap();
    let vault_key = vault::new_vault_key(&mut rng);
    let carol_public = p.carol.public();
    let bob_public = p.bob.public();
    let to_carol = Recipient {
        user_id: "carol",
        key_id: "pk-carol",
        public: &carol_public,
    };
    let by_alice = Signer {
        user_id: "alice",
        key_id: "pk-alice",
        key: &p.alice,
    };

    let wrap = vault::wrap_vault_key_to_member(&vault_key, "v", 2, to_carol, by_alice, &mut rng).unwrap();
    let (member, signer) = verified.accept_wrap(&wrap).unwrap();
    assert_eq!(member.user_id, "carol");
    assert_eq!(vault::open_member_wrap(&wrap, &p.carol, &signer).unwrap(), vault_key);

    // Carol cannot wrap to others: she has write, not manage.
    let by_carol = Signer {
        user_id: "carol",
        key_id: "pk-carol",
        key: &p.carol,
    };
    let wrap = vault::wrap_vault_key_to_member(&vault_key, "v", 2, to_carol, by_carol, &mut rng).unwrap();
    assert!(matches!(verified.accept_wrap(&wrap), Err(Error::Refused(_))));
    // Bob was removed at version 2: a wrap of it to him is refused, as is a wrap of a version not in the chain.
    let to_bob = Recipient {
        user_id: "bob",
        key_id: "pk-bob",
        public: &bob_public,
    };
    let wrap = vault::wrap_vault_key_to_member(&vault_key, "v", 2, to_bob, by_alice, &mut rng).unwrap();
    assert!(matches!(verified.accept_wrap(&wrap), Err(Error::Refused(_))));
    let wrap = vault::wrap_vault_key_to_member(&vault_key, "v", 3, to_carol, by_alice, &mut rng).unwrap();
    assert!(matches!(verified.accept_wrap(&wrap), Err(Error::Refused(_))));
    // A wrap to Carol's id but another key (a swapped personal key) fails the signature over her listed key.
    let swapped_public = PersonalKey::generate(&mut rng).public();
    let swapped = Recipient {
        user_id: "carol",
        key_id: "pk-carol",
        public: &swapped_public,
    };
    let wrap = vault::wrap_vault_key_to_member(&vault_key, "v", 2, swapped, by_alice, &mut rng).unwrap();
    assert_eq!(verified.accept_wrap(&wrap).unwrap_err(), Error::Signature);
}

#[test]
fn a_remembered_version_refuses_rollbacks_and_forks() {
    let p = people();
    let entries = chain(&p);
    let first = check_remembered(&entries[0], None).unwrap();
    let second = check_remembered(&entries[1], Some(&first)).unwrap();
    let third = check_remembered(&entries[2], Some(&second)).unwrap();
    assert_eq!((third.version, third.revision), (2, 1));
    assert_eq!(check_remembered(&entries[2], Some(&third)).unwrap(), third);
    assert!(matches!(
        check_remembered(&entries[1], Some(&third)),
        Err(Error::Refused(_))
    ));
    let alice = member("alice", Role::Manage, &p.alice);
    let fork = KeyVersion::sign("v", 2, 1, vec![alice], "alice", "pk-alice", &p.alice);
    assert!(matches!(check_remembered(&fork, Some(&third)), Err(Error::Refused(_))));
}

#[test]
fn the_member_list_hash_does_not_depend_on_row_order() {
    let p = people();
    let entries = chain(&p);
    let mut reordered = entries[1].clone();
    reordered.members.reverse();
    assert_eq!(reordered.member_list_hash(), entries[1].member_list_hash());
    assert_ne!(entries[0].member_list_hash(), entries[1].member_list_hash());
    assert_eq!(Role::parse("approve").unwrap(), Role::Approve);
    assert!(Role::parse("owner").is_err());
}
