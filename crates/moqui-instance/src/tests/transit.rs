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

//! M6's rules on the manager (plans/Vault-Instances.md §6): transit unseals an instance only for its pinned link key
//! and not after a revoke; the approval policy waits for a member; an unseal from a new address is told apart; the
//! manager takes "unsealed" only from its own unseal or a proof by the key it pinned; each share goes to one current
//! holder; and the relay holds ciphertext only.

use moqui_vault::hpke::SecretKey;
use moqui_vault::managed::{self, ProofLink, UnsealKey};
use moqui_vault::personal::PersonalKey;
use moqui_vault::rng::OsRng;
use moqui_vault::sign::SigningKey;
use serde_json::json;

use super::{create, field_of, login, new_id, setup, sys};
use crate::transit::{self, Answer, Ask};

const INSTANCE: &str = "moqui.server.instance.AppInstance";
const ACTION: &str = "moqui.server.instance.InstanceAction";

fn environment(facade: &moqui_core::entity::EntityFacade, policy: &str, method: &str, group: &str) -> String {
    let environment = new_id("env");
    create(
        facade,
        "moqui.server.instance.InstanceEnvironment",
        json!({ "environmentId": environment, "unsealPolicyEnumId": policy, "unsealMethod": method,
                "approverUserGroupId": group, "recoveryUserGroupId": group }),
    );
    environment
}

fn instance(facade: &moqui_core::entity::EntityFacade, environment: &str, fingerprint: &str) -> String {
    let id = new_id("inst");
    create(
        facade,
        INSTANCE,
        json!({ "appInstanceId": id, "instanceKindId": "mr-moqui", "environmentId": environment,
                "linkKeyFingerprint": fingerprint, "linkState": "linked", "sealState": "sealed",
                "unsealAddresses": "10.0.0.1" }),
    );
    id
}

/// The instance's root key wrapped to its transit key, as its seal.json keeps it.
fn wrapped(id: &str, root: &[u8; 32]) -> Vec<u8> {
    let public = moqui_vault::hpke::PublicKey::from_bytes(&sys(|| transit::transit_public(id)).unwrap()).unwrap();
    managed::wrap_root(&public, id, root, &mut OsRng).unwrap()
}

fn ask<'a>(id: &'a str, fingerprint: &'a str, remote: &'a str, wrapped: &'a [u8], ephemeral: &'a [u8]) -> Ask<'a> {
    Ask {
        instance_id: id,
        link_fingerprint: fingerprint,
        remote,
        generation: 7,
        wrapped_root: wrapped,
        ephemeral_public: ephemeral,
    }
}

fn proof_key(root: &[u8; 32]) -> SigningKey {
    let root = moqui_core::seal::RootKey::from_slice(root).unwrap();
    SigningKey::from_seed(&moqui_core::seal::unsealed_proof_seed(&root))
}

#[test]
fn transit_unseals_only_for_the_pinned_link_key_and_not_after_a_revoke_and_tells_a_new_address() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let env = environment(&facade, "IupAutomatic", "", "");
    let id = instance(&facade, &env, "fp-pinned");
    let other = instance(&facade, &env, "fp-other");
    let root = [5_u8; 32];
    let wrap = wrapped(&id, &root);
    let ephemeral = SecretKey::random(&mut OsRng);
    let ephemeral_public = ephemeral.public().unwrap().as_bytes().to_vec();

    // another link key than the one pinned is refused
    let refused = sys(|| transit::unseal(&facade, &ask(&id, "fp-other", "10.0.0.1", &wrap, &ephemeral_public)));
    assert!(refused.unwrap_err().message().contains("not pinned"));
    // another instance's own link does not open this one's wrap: its transit key is its own
    assert!(sys(|| transit::unseal(&facade, &ask(&other, "fp-other", "10.0.0.1", &wrap, &ephemeral_public))).is_err());

    // its pinned key, from the address it unsealed from before
    let answer =
        sys(|| transit::unseal(&facade, &ask(&id, "fp-pinned", "10.0.0.1", &wrap, &ephemeral_public))).unwrap();
    let Answer::Sealed {
        sealed_root,
        new_address,
    } = answer
    else {
        panic!("the automatic policy waited");
    };
    assert!(!new_address);
    assert_eq!(*managed::open_reply(&ephemeral, &id, &sealed_root).unwrap(), root);
    // only the unseal that asked opens the answer
    assert!(managed::open_reply(&SecretKey::random(&mut OsRng), &id, &sealed_root).is_err());
    // the proof key is derived from the root key it saw, and the unseal is an action and a Monitor unseal
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "unsealProofKey"),
        moqui_vault::text::hex(proof_key(&root).verifying_key().as_bytes())
    );
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "unsealProofTrust"),
        "transit"
    );
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "sealState"),
        "unsealed"
    );
    let actions = super::all_text(&facade, ACTION);
    assert!(actions.iter().any(|text| text == "IatUnseal"), "{actions:?}");

    // sealed again, then unsealed from an address it never unsealed from: told apart (§4.8's alert)
    sys(|| crate::link::note_state(&facade, &id, "linked", Some("sealed"), "10.0.0.1"));
    let answer =
        sys(|| transit::unseal(&facade, &ask(&id, "fp-pinned", "10.9.9.9", &wrap, &ephemeral_public))).unwrap();
    assert!(matches!(answer, Answer::Sealed { new_address: true, .. }));
    assert!(field_of(&facade, INSTANCE, "appInstanceId", &id, "unsealAddresses").contains("10.9.9.9"));

    // a revoke unpins the link key: transit refuses it after
    sys(|| crate::link::revoke(&facade, &id)).unwrap();
    let revoked = sys(|| transit::unseal(&facade, &ask(&id, "fp-pinned", "10.0.0.1", &wrap, &ephemeral_public)));
    assert!(revoked.unwrap_err().message().contains("revoked"));
}

#[test]
fn the_approval_policy_waits_for_a_member_once_for_each_unseal() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let group = new_id("APPR");
    create(&facade, "moqui.security.UserGroup", json!({ "userGroupId": group }));
    let member = new_id("member");
    create(
        &facade,
        "moqui.security.UserAccount",
        json!({ "userId": member, "username": member }),
    );
    create(
        &facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": group, "userId": member, "fromDate": 0 }),
    );
    let env = environment(&facade, "IupApproval", "transit", &group);
    let id = instance(&facade, &env, "fp-a");
    let root = [6_u8; 32];
    let wrap = wrapped(&id, &root);
    let ephemeral = SecretKey::random(&mut OsRng);
    let public = ephemeral.public().unwrap().as_bytes().to_vec();
    let waiting = |remote: &str| match sys(|| transit::unseal(&facade, &ask(&id, "fp-a", remote, &wrap, &public))) {
        Ok(Answer::Waiting(why)) => why,
        other => panic!("the approval policy answered: {other:?}"),
    };
    let first = waiting("10.0.0.1");
    assert!(first.contains(&group), "{first}");
    // asked again, it still waits on the one action: no second request
    assert_eq!(waiting("10.0.0.1"), first);
    let action_id = first
        .split("InstanceAction ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap()
        .to_string();
    // a caller who is no instance administrator does not approve it
    login(&member);
    assert!(transit::approve(&facade, &action_id).is_err());
    // a member does (authorization off stands for INSTANCE_ADMIN here), once
    sys(|| transit::approve(&facade, &action_id)).unwrap();
    let answer = sys(|| transit::unseal(&facade, &ask(&id, "fp-a", "10.0.0.1", &wrap, &public))).unwrap();
    let Answer::Sealed { sealed_root, .. } = answer else {
        panic!("the approved unseal waited");
    };
    assert_eq!(*managed::open_reply(&ephemeral, &id, &sealed_root).unwrap(), root);
    assert!(
        field_of(&facade, ACTION, "instanceActionId", &action_id, "report").contains(&format!("approved by {member}"))
    );
    assert_eq!(
        field_of(&facade, ACTION, "instanceActionId", &action_id, "statusEnumId"),
        "IasDone"
    );
    // the next unseal waits again
    assert!(!waiting("10.0.0.1").is_empty());
}

#[test]
fn unsealed_is_taken_only_from_the_managers_unseal_or_a_proof_by_the_pinned_key() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let root = [8_u8; 32];
    let key = proof_key(&root);
    let public = key.verifying_key().as_bytes().to_vec();
    let link = |id: &'static str, generation| ProofLink {
        instance_id: id,
        link_fingerprint: "fp-p",
        generation,
        challenge: b"challenge",
    };

    // a transit environment: no claim is taken before the manager's own unseal pinned the key
    let env = environment(&facade, "IupAutomatic", "transit", "");
    let id: &'static str = Box::leak(instance(&facade, &env, "fp-p").into_boxed_str());
    let proof = managed::sign_proof(&key, &link(id, 1));
    assert!(!sys(|| transit::check_proof(&facade, &link(id, 1), &public, &proof)).unwrap());
    let wrap = wrapped(id, &root);
    let ephemeral = SecretKey::random(&mut OsRng).public().unwrap().as_bytes().to_vec();
    sys(|| transit::unseal(&facade, &ask(id, "fp-p", "10.0.0.1", &wrap, &ephemeral))).unwrap();
    assert!(sys(|| transit::check_proof(&facade, &link(id, 1), &public, &proof)).unwrap());
    // a proof for another link, or by another key (a holder of the link key alone), is not
    assert!(!sys(|| transit::check_proof(&facade, &link(id, 2), &public, &proof)).unwrap());
    let thief = SigningKey::random(&mut OsRng);
    let forged = managed::sign_proof(&thief, &link(id, 1));
    assert!(!sys(|| transit::check_proof(&facade, &link(id, 1), thief.verifying_key().as_bytes(), &forged)).unwrap());

    // a method the manager does not unseal: the first proof pins its key (first-use), and later ones must match it
    let env = environment(&facade, "IupAutomatic", "tpm", "");
    let tpm: &'static str = Box::leak(instance(&facade, &env, "fp-p").into_boxed_str());
    let proof = managed::sign_proof(&key, &link(tpm, 1));
    assert!(sys(|| transit::check_proof(&facade, &link(tpm, 1), &public, &proof)).unwrap());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", tpm, "unsealProofTrust"),
        "first-use"
    );
    let forged = managed::sign_proof(&thief, &link(tpm, 3));
    assert!(!sys(|| transit::check_proof(&facade, &link(tpm, 3), thief.verifying_key().as_bytes(), &forged)).unwrap());
}

/// A recovery holder: a member of `group` with a personal key.
fn holder(facade: &moqui_core::entity::EntityFacade, group: &str) -> (String, String, PersonalKey) {
    let user = new_id("holder");
    let key_id = new_id("pk");
    let key = PersonalKey::generate(&mut OsRng);
    create(
        facade,
        "moqui.security.UserAccount",
        json!({ "userId": user, "username": user }),
    );
    create(
        facade,
        "moqui.security.UserGroupMember",
        json!({ "userGroupId": group, "userId": user, "fromDate": 0 }),
    );
    create(
        facade,
        "moqui.security.vault.VaultPersonalKey",
        json!({ "personalKeyId": key_id, "userId": user, "statusId": "VpkActive",
                "encryptPublic": moqui_vault::text::hex(key.public().encrypt.as_bytes()),
                "verifyPublic": moqui_vault::text::hex(key.public().verify.as_bytes()) }),
    );
    (user, key_id, key)
}

#[test]
fn each_share_goes_to_one_current_holder_and_the_relay_holds_ciphertext_only() {
    use crate::shares::{self, SentShare};
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let group = new_id("HOLD");
    create(&facade, "moqui.security.UserGroup", json!({ "userGroupId": group }));
    let alice = holder(&facade, &group);
    let bob = holder(&facade, &group);
    let link = SigningKey::random(&mut OsRng);
    let fingerprint = managed::link_fingerprint(link.verifying_key().as_bytes());
    let env = environment(&facade, "IupAutomatic", "transit", &group);
    let id = instance(&facade, &env, &fingerprint);
    let listed = sys(|| shares::holders(&facade, &id)).unwrap();
    assert_eq!(listed.len(), 2);

    let send = |to: &(String, String, PersonalKey), n: u32, signer: &SigningKey| SentShare {
        index: n,
        delivery: moqui_vault::delivery::send(
            &format!("rs-test-{id}-{n}"),
            moqui_vault::delivery::To {
                user_id: &to.0,
                key_id: &to.1,
                public: &to.2.public(),
            },
            managed::SHARE_PURPOSE,
            b"share value",
            moqui_vault::delivery::Sender {
                kind: "instance",
                id: &id,
                key: signer,
            },
            &mut OsRng,
        )
        .unwrap(),
    };
    let record = |shares: &[SentShare], signer: &SigningKey| {
        sys(|| shares::record_shares(&facade, &id, signer.verifying_key().as_bytes(), "rs-test", 2, shares))
    };
    // two shares to one holder, a delivery the link key did not sign, and a link key not pinned are refused
    let twice = record(&[send(&alice, 1, &link), send(&alice, 2, &link)], &link).unwrap_err();
    assert!(twice.message().contains("each holder holds one"), "{twice}");
    let stranger = SigningKey::random(&mut OsRng);
    assert!(record(&[send(&alice, 1, &stranger)], &link).is_err());
    assert!(record(&[send(&alice, 1, &stranger)], &stranger).is_err());
    // one to each holder is kept, as deliveries and share rows
    record(&[send(&alice, 1, &link), send(&bob, 2, &link)], &link).unwrap();
    assert_eq!(sys(|| shares::share_report(&facade, &id)).unwrap().len(), 2);
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "linkSignPublic"),
        moqui_vault::text::hex(link.verifying_key().as_bytes())
    );
    // only its recipient confirms a share
    login(&bob.0);
    assert!(shares::confirm(&facade, &format!("rs-test-{id}-1"), &"ab".repeat(64)).is_err());
    login(&alice.0);
    shares::confirm(&facade, &format!("rs-test-{id}-1"), &"ab".repeat(64)).unwrap();
    assert_eq!(sys(|| shares::confirmations(&facade, &id, "rs-test")).unwrap().len(), 1);

    // the relay: a key not signed by the pinned link key is refused
    let unseal = SecretKey::random(&mut OsRng);
    let unseal_public = *unseal.public().unwrap().as_bytes();
    let forged = UnsealKey::sign(&stranger, &id, &unseal_public);
    assert!(sys(|| shares::offer_unseal_key(
        &facade,
        &id,
        stranger.verifying_key().as_bytes(),
        &unseal_public,
        &forged.signature
    ))
    .is_err());
    let key = UnsealKey::sign(&link, &id, &unseal_public);
    let offered = sys(|| {
        shares::offer_unseal_key(
            &facade,
            &id,
            link.verifying_key().as_bytes(),
            &unseal_public,
            &key.signature,
        )
    });
    assert!(offered.unwrap().is_empty());
    // a holder seals their share to it; the manager keeps the ciphertext, and nothing of the share
    let sealed = managed::seal_share(&key, "the-share-of-alice", &mut OsRng).unwrap();
    shares::post_sealed_share(&facade, &id, &unseal_public, &sealed).unwrap();
    let relay = shares::relay_of(&id).unwrap();
    for (_, held) in &relay.sealed {
        assert!(!held.windows(9).any(|window| window == b"the-share"));
    }
    // someone who holds no share of it is refused
    let outsider = new_id("outsider");
    create(
        &facade,
        "moqui.security.UserAccount",
        json!({ "userId": outsider, "username": outsider }),
    );
    login(&outsider);
    assert!(shares::post_sealed_share(&facade, &id, &unseal_public, &sealed).is_err());
    // the instance's next ask takes it, and only its unseal key opens it
    let taken = sys(|| {
        shares::offer_unseal_key(
            &facade,
            &id,
            link.verifying_key().as_bytes(),
            &unseal_public,
            &key.signature,
        )
    })
    .unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(
        managed::open_share(&unseal, &id, &taken[0].1).unwrap().as_str(),
        "the-share-of-alice"
    );
    // a new unseal's key drops what was sealed to the old one
    let next = SecretKey::random(&mut OsRng);
    let next_public = *next.public().unwrap().as_bytes();
    let next_key = UnsealKey::sign(&link, &id, &next_public);
    sys(|| {
        shares::offer_unseal_key(
            &facade,
            &id,
            link.verifying_key().as_bytes(),
            &next_public,
            &next_key.signature,
        )
    })
    .unwrap();
    login(&alice.0);
    assert!(shares::post_sealed_share(&facade, &id, &unseal_public, &sealed).is_err());
}
