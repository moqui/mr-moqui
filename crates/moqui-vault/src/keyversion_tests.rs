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

fn sign(
    vault_id: &str,
    version: u64,
    revision: u64,
    members: Vec<Member>,
    signer_user_id: &str,
    signer_key_id: &str,
    signer: &PersonalKey,
) -> KeyVersion {
    KeyVersion::sign(
        vault_id,
        version,
        revision,
        members,
        signer_user_id,
        signer_key_id,
        signer,
    )
    .unwrap()
}

/// Alice makes the vault with Bob as a reader (1.1), adds Carol (1.2), then removes Bob (2.1).
fn chain(p: &People) -> Vec<KeyVersion> {
    let alice = member("alice", Role::Manage, &p.alice);
    let bob = member("bob", Role::Read, &p.bob);
    let carol = member("carol", Role::Write, &p.carol);
    vec![
        sign(
            "v",
            1,
            1,
            vec![alice.clone(), bob.clone()],
            "alice",
            "pk-alice",
            &p.alice,
        ),
        sign(
            "v",
            1,
            2,
            vec![alice.clone(), bob, carol.clone()],
            "alice",
            "pk-alice",
            &p.alice,
        ),
        sign("v", 2, 1, vec![carol, alice], "alice", "pk-alice", &p.alice),
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
    entries[1] = sign("v", 1, 2, members, "bob", "pk-bob", &p.bob);
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
    entries[2] = sign("v", 2, 1, members, "alice", "pk-alice", &server);
    assert_eq!(refused(Chain::verify(&entries, &checked)), Error::Signature);

    // A whole chain the server made with a maker key of its own: valid signatures, but no key the user checked.
    let fake_maker = member("alice", Role::Manage, &server);
    let fake = vec![sign("v", 1, 1, vec![fake_maker], "alice", "pk-alice", &server)];
    assert!(matches!(refused(Chain::verify(&fake, &checked)), Error::Refused(_)));

    // A signer who is not in the list at all.
    let mut entries = chain(&p);
    let members = entries[1].members.clone();
    entries[1] = sign("v", 1, 2, members, "mallory", "pk-mallory", &server);
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
    let other = sign("w", 1, 2, vec![alice], "alice", "pk-alice", &p.alice);
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
    let checked = [p.alice.public().verify];
    let remember = |upto: usize, before: Option<&Remembered>| {
        let chain = Chain::verify(&entries[..upto], &checked).unwrap();
        check_remembered(&chain, before)
    };
    let first = remember(1, None).unwrap();
    let second = remember(2, Some(&first)).unwrap();
    let third = remember(3, Some(&second)).unwrap();
    assert_eq!((third.version, third.revision), (2, 1));
    assert_eq!(remember(3, Some(&third)).unwrap(), third);
    assert!(matches!(remember(2, Some(&third)), Err(Error::Refused(_))));
    // The same entry with another member list.
    let alice = member("alice", Role::Manage, &p.alice);
    let mut forked = entries[..2].to_vec();
    forked.push(sign("v", 2, 1, vec![alice], "alice", "pk-alice", &p.alice));
    let chain = Chain::verify(&forked, &checked).unwrap();
    assert!(matches!(check_remembered(&chain, Some(&third)), Err(Error::Refused(_))));
}

#[test]
fn a_fork_that_goes_higher_is_refused() {
    // The real chain: v1r1 with managers C and R; v2r1, signed by C, removes R, and the client remembers it. R, who
    // still held manage in v1r1, signs a forged v2r1' and v2r2' that go higher.
    let mut rng = TestRng::new("keyversion-fork");
    let c = PersonalKey::generate(&mut rng);
    let r = PersonalKey::generate(&mut rng);
    let mallory = PersonalKey::generate(&mut rng);
    let mc = member("c", Role::Manage, &c);
    let mr = member("r", Role::Manage, &r);
    let v1 = sign("v", 1, 1, vec![mc.clone(), mr.clone()], "c", "pk-c", &c);
    let real = vec![v1.clone(), sign("v", 2, 1, vec![mc.clone()], "c", "pk-c", &c)];
    let checked = [c.public().verify];
    let remembered = check_remembered(&Chain::verify(&real, &checked).unwrap(), None).unwrap();

    let mm = member("mallory", Role::Manage, &mallory);
    let forged = vec![
        v1,
        sign("v", 2, 1, vec![mc.clone(), mr.clone()], "r", "pk-r", &r),
        sign("v", 2, 2, vec![mc, mr, mm], "r", "pk-r", &r),
    ];
    // Each forged entry is signed by a manager of the one before, so the chain alone verifies.
    let chain = Chain::verify(&forged, &checked).unwrap();
    assert_eq!((chain.latest().version, chain.latest().revision), (2, 2));
    assert!(matches!(
        check_remembered(&chain, Some(&remembered)),
        Err(Error::Refused(_))
    ));
}

#[test]
fn nothing_before_the_first_checked_signer_is_trusted() {
    // A server prepends a history of its own (v1r1, its key as a manager) to a genuine v2r1 that Alice signed; the
    // user checked Alice. The chain verifies, but a wrap of version 1 is refused, and a wrap of version 2 by a
    // manager of the genuine entry is accepted.
    let mut rng = TestRng::new("keyversion-anchor");
    let p = people();
    let server = PersonalKey::generate(&mut rng);
    let alice = member("alice", Role::Manage, &p.alice);
    let carol = member("carol", Role::Write, &p.carol);
    let fake_maker = member("mallory", Role::Manage, &server);
    let entries = vec![
        sign(
            "v",
            1,
            1,
            vec![fake_maker, alice.clone(), carol.clone()],
            "mallory",
            "pk-mallory",
            &server,
        ),
        sign("v", 2, 1, vec![alice, carol], "alice", "pk-alice", &p.alice),
    ];
    let verified = Chain::verify(&entries, &[p.alice.public().verify]).unwrap();
    let carol_public = p.carol.public();
    let to_carol = Recipient {
        user_id: "carol",
        key_id: "pk-carol",
        public: &carol_public,
    };
    let by_server = Signer {
        user_id: "mallory",
        key_id: "pk-mallory",
        key: &server,
    };
    let vault_key = vault::new_vault_key(&mut rng);
    let early = vault::wrap_vault_key_to_member(&vault_key, "v", 1, to_carol, by_server, &mut rng).unwrap();
    assert!(matches!(verified.accept_wrap(&early), Err(Error::Refused(_))));
    let by_alice = Signer {
        user_id: "alice",
        key_id: "pk-alice",
        key: &p.alice,
    };
    let current = vault::wrap_vault_key_to_member(&vault_key, "v", 2, to_carol, by_alice, &mut rng).unwrap();
    assert!(verified.accept_wrap(&current).is_ok());
}

#[test]
fn a_deposit_key_is_accepted_only_for_the_current_version_from_a_manager() {
    let mut rng = TestRng::new("keyversion-deposit");
    let p = people();
    let entries = chain(&p);
    let verified = Chain::verify(&entries, &[p.alice.public().verify]).unwrap();
    let vault_key = vault::new_vault_key(&mut rng);
    let by_alice = Signer {
        user_id: "alice",
        key_id: "pk-alice",
        key: &p.alice,
    };
    let (current, _) = vault::new_deposit_key(&vault_key, "v", 2, by_alice, &mut rng).unwrap();
    assert_eq!(verified.accept_deposit(&current).unwrap(), p.alice.public().verify);
    let (old, _) = vault::new_deposit_key(&vault_key, "v", 1, by_alice, &mut rng).unwrap();
    assert!(matches!(verified.accept_deposit(&old), Err(Error::Refused(_))));
    let by_carol = Signer {
        user_id: "carol",
        key_id: "pk-carol",
        key: &p.carol,
    };
    let (not_manager, _) = vault::new_deposit_key(&vault_key, "v", 2, by_carol, &mut rng).unwrap();
    assert!(matches!(verified.accept_deposit(&not_manager), Err(Error::Refused(_))));
}

#[test]
fn a_member_list_naming_one_key_twice_is_refused() {
    let p = people();
    let alice = member("alice", Role::Manage, &p.alice);
    let twice = KeyVersion::sign(
        "v",
        1,
        1,
        vec![alice.clone(), alice.clone()],
        "alice",
        "pk-alice",
        &p.alice,
    );
    assert!(matches!(twice, Err(Error::Refused(_))));
    // A row a server doubled after signing does not verify either.
    let mut entry = sign("v", 1, 1, vec![alice.clone()], "alice", "pk-alice", &p.alice);
    entry.members.push(alice);
    assert!(matches!(
        Chain::verify(&[entry], &[p.alice.public().verify]),
        Err(Error::Refused(_))
    ));
}

#[test]
fn the_next_entry_is_signed_only_by_a_manager_of_the_latest() {
    let p = people();
    let entries = chain(&p);
    let verified = Chain::verify(&entries, &[p.alice.public().verify]).unwrap();
    let members = entries[2].members.clone();
    let next = verified
        .next(2, 2, members.clone(), "alice", "pk-alice", &p.alice)
        .unwrap();
    assert_eq!((next.version, next.revision), (2, 2));
    assert!(matches!(
        verified.next(2, 2, members.clone(), "carol", "pk-carol", &p.carol),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        verified.next(2, 3, members.clone(), "alice", "pk-alice", &p.alice),
        Err(Error::Refused(_))
    ));
    let first = KeyVersion::first(
        "w",
        vec![member("carol", Role::Read, &p.carol)],
        "alice",
        "pk-alice",
        &p.alice,
    );
    assert!(matches!(first, Err(Error::Refused(_))));
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

/// An entry the recovery member signs may only give one person a new key: adding a member, changing a role, giving
/// two members new keys, or rotating the key are refused (§3.10).
#[test]
fn a_recovery_entry_only_gives_one_member_a_new_key() {
    let p = people();
    let mut rng = TestRng::new("keyversion-recovery");
    let recovery = PersonalKey::generate(&mut rng);
    let alice2 = PersonalKey::generate(&mut rng);
    let alice = member("alice", Role::Manage, &p.alice);
    let bob = member("bob", Role::Read, &p.bob);
    let rk = Member {
        user_id: recovery_member_id("rk"),
        role: Role::Manage,
        key_id: "rk".to_string(),
        public: recovery.public(),
    };
    assert_eq!(recovery_key_of(&rk.user_id), Some("rk"));
    let first = sign(
        "v",
        1,
        1,
        vec![alice.clone(), bob.clone(), rk.clone()],
        "alice",
        "pk-alice",
        &p.alice,
    );
    let by_recovery = |members: Vec<Member>, version: u64, revision: u64| {
        let entry = sign("v", version, revision, members, "recovery:rk", "rk", &recovery);
        vec![first.clone(), entry]
    };
    let checked = [p.alice.public().verify];
    let rekeyed = Member {
        key_id: "pk-alice2".to_string(),
        public: alice2.public(),
        ..alice.clone()
    };
    let good = by_recovery(vec![rekeyed.clone(), bob.clone(), rk.clone()], 1, 2);
    assert!(Chain::verify(&good, &checked).is_ok());
    // the recovery key alone anchors it, for the recovered member's new client
    assert!(Chain::verify(&good, &[recovery.public().verify]).is_ok());

    let carol = member("carol", Role::Read, &p.carol);
    let added = by_recovery(vec![rekeyed.clone(), bob.clone(), rk.clone(), carol], 1, 2);
    assert!(matches!(refused(Chain::verify(&added, &checked)), Error::Refused(_)));
    let promoted = by_recovery(
        vec![rekeyed.clone(), member("bob", Role::Manage, &p.bob), rk.clone()],
        1,
        2,
    );
    assert!(matches!(refused(Chain::verify(&promoted, &checked)), Error::Refused(_)));
    let nothing = by_recovery(vec![alice.clone(), bob.clone(), rk.clone()], 1, 2);
    assert!(matches!(refused(Chain::verify(&nothing, &checked)), Error::Refused(_)));
    let two = by_recovery(
        vec![rekeyed.clone(), member("bob", Role::Read, &p.carol), rk.clone()],
        1,
        2,
    );
    assert!(matches!(refused(Chain::verify(&two, &checked)), Error::Refused(_)));
    let rotated = by_recovery(vec![rekeyed, bob, rk], 2, 1);
    assert!(matches!(refused(Chain::verify(&rotated, &checked)), Error::Refused(_)));
}
