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

//! M6's rules on the manager (plans/archive/Vault-Instances.md §6): transit unseals an instance only for its pinned link key
//! and not after a revoke, and only to its own root key (a wrap a holder of its link key made is refused, and the key
//! pinned stays); the first pin is the instance's first start's, or a member's approval; the approval policy waits for
//! a member, the approval bound to the ask; an unseal from a new address is told apart; the manager takes "unsealed"
//! only from its own unseal or a proof by the key it pinned (a first-use key waits for an operator, a rotated key is
//! taken only signed by the old one); each share goes to one current holder, indexed once; the relay holds ciphertext
//! only, for holders, in time.

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

/// An instance paired `paired_ago` milliseconds ago.
fn instance_paired(
    facade: &moqui_core::entity::EntityFacade,
    environment: &str,
    fingerprint: &str,
    paired_ago: i64,
) -> String {
    let id = new_id("inst");
    create(
        facade,
        INSTANCE,
        json!({ "appInstanceId": id, "instanceKindId": "mr-moqui", "environmentId": environment,
                "linkKeyFingerprint": fingerprint, "linkState": "linked", "sealState": "sealed",
                "unsealAddresses": "10.0.0.1", "linkPairedDate": moqui_core::user::now_millis() - paired_ago }),
    );
    id
}

/// An instance at its first start: paired a moment ago.
fn instance(facade: &moqui_core::entity::EntityFacade, environment: &str, fingerprint: &str) -> String {
    instance_paired(facade, environment, fingerprint, 1_000)
}

/// The instance's root key wrapped to its transit key, as its seal.json keeps it.
fn wrapped(facade: &moqui_core::entity::EntityFacade, id: &str, root: &[u8; 32]) -> Vec<u8> {
    let public =
        moqui_vault::hpke::PublicKey::from_bytes(&sys(|| transit::transit_public(facade, id)).unwrap()).unwrap();
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

fn waiting(answer: Result<Answer, moqui_lib::error::Error>) -> String {
    match answer {
        Ok(Answer::Waiting(why)) => why,
        other => panic!("the unseal did not wait: {other:?}"),
    }
}

fn action_of(why: &str) -> String {
    why.split("InstanceAction ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap()
        .to_string()
}

#[test]
fn transit_unseals_only_its_own_root_key_for_the_pinned_link_key_and_not_after_a_revoke() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let env = environment(&facade, "IupAutomatic", "", "");
    let id = instance(&facade, &env, "fp-pinned");
    let other = instance(&facade, &env, "fp-other");
    let root = [5_u8; 32];
    let wrap = wrapped(&facade, &id, &root);
    sys(|| transit::transit_public(&facade, &other)).unwrap();
    let ephemeral = SecretKey::random(&mut OsRng);
    let ephemeral_public = ephemeral.public().unwrap().as_bytes().to_vec();

    // another link key than the one pinned is refused
    let refused = sys(|| transit::unseal(&facade, &ask(&id, "fp-other", "10.0.0.1", &wrap, &ephemeral_public)));
    assert!(refused.unwrap_err().message().contains("not pinned"));
    // another instance's own link does not open this one's wrap: its transit key is its own
    assert!(sys(|| transit::unseal(&facade, &ask(&other, "fp-other", "10.0.0.1", &wrap, &ephemeral_public))).is_err());

    // its pinned key, at its first start, from the address it unsealed from before: the proof key is pinned
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
    assert!(managed::open_reply(&SecretKey::random(&mut OsRng), &id, &sealed_root).is_err());
    let pinned = moqui_vault::text::hex(proof_key(&root).verifying_key().as_bytes());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "unsealProofKey"),
        pinned
    );
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "unsealProofTrust"),
        "transit"
    );
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "sealState"),
        "unsealed"
    );
    assert!(super::all_text(&facade, ACTION).iter().any(|text| text == "IatUnseal"));

    // a holder of the link key alone wraps a root key of their own to the public transit key: refused, and the key
    // pinned stays (never pinned in its place)
    let forged = wrapped(&facade, &id, &[66_u8; 32]);
    let stolen = sys(|| transit::unseal(&facade, &ask(&id, "fp-pinned", "10.6.6.6", &forged, &ephemeral_public)));
    assert!(stolen.unwrap_err().message().contains("not this instance's"));
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &id, "unsealProofKey"),
        pinned
    );
    assert!(super::all_text(&facade, ACTION).iter().any(|text| text == "IasRefused"));

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
fn a_first_pin_after_the_first_start_and_a_method_not_the_managers_are_not_unsealed_by_transit() {
    let (ctx, facade) = setup();
    let _guard = ctx.enter();
    let env = environment(&facade, "IupAutomatic", "transit", "");
    // paired an hour ago, never unsealed by the manager: a first pin now waits for a member, even automatic
    let late = instance_paired(&facade, &env, "fp-late", 3_600_000);
    let wrap = wrapped(&facade, &late, &[9_u8; 32]);
    let public = SecretKey::random(&mut OsRng).public().unwrap().as_bytes().to_vec();
    let why = waiting(sys(|| {
        transit::unseal(&facade, &ask(&late, "fp-late", "10.0.0.1", &wrap, &public))
    }));
    assert!(why.contains("outside its first start"), "{why}");
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", &late, "unsealProofKey"),
        ""
    );
    let operator = new_id("operator");
    create(&facade, "moqui.security.UserAccount", json!({ "userId": operator, "username": operator }));
    login(&operator);
    sys(|| transit::approve(&facade, &action_of(&why))).unwrap();
    assert!(matches!(
        sys(|| transit::unseal(&facade, &ask(&late, "fp-late", "10.0.0.1", &wrap, &public))).unwrap(),
        Answer::Sealed { .. }
    ));

    // a tpm environment: no transit key is made for it, and transit refuses its unseal
    let tpm_env = environment(&facade, "IupAutomatic", "tpm", "");
    let tpm = instance(&facade, &tpm_env, "fp-tpm");
    assert!(sys(|| transit::transit_public(&facade, &tpm)).is_err());
    let refused = sys(|| transit::unseal(&facade, &ask(&tpm, "fp-tpm", "10.0.0.1", &wrap, &public)));
    assert!(refused
        .unwrap_err()
        .message()
        .contains("does not unseal by the manager's transit"));
}

#[test]
fn the_approval_policy_waits_for_a_member_and_an_approval_is_bound_to_its_ask() {
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
    let wrap = wrapped(&facade, &id, &root);
    let ephemeral = SecretKey::random(&mut OsRng);
    let public = ephemeral.public().unwrap().as_bytes().to_vec();
    let first = waiting(sys(|| {
        transit::unseal(&facade, &ask(&id, "fp-a", "10.0.0.1", &wrap, &public))
    }));
    assert!(first.contains(&group), "{first}");
    // asked again, it still waits on the one action: no second request
    assert_eq!(
        waiting(sys(|| transit::unseal(
            &facade,
            &ask(&id, "fp-a", "10.0.0.1", &wrap, &public)
        ))),
        first
    );
    let action_id = action_of(&first);
    // a caller who is no instance administrator does not approve it
    login(&member);
    assert!(transit::approve(&facade, &action_id).is_err());
    // a member does (authorization off stands for INSTANCE_ADMIN here)
    sys(|| transit::approve(&facade, &action_id)).unwrap();
    assert!(!field_of(&facade, ACTION, "instanceActionId", &action_id, "approvedUntil").is_empty());
    // the approval is for the ask that waited: another address does not take it
    let mut elsewhere = ask(&id, "fp-a", "10.7.7.7", &wrap, &public);
    elsewhere.generation = 7;
    assert!(matches!(
        sys(|| transit::unseal(&facade, &elsewhere)).unwrap(),
        Answer::Waiting(_)
    ));
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
    waiting(sys(|| {
        transit::unseal(&facade, &ask(&id, "fp-a", "10.0.0.1", &wrap, &public))
    }));
}

#[test]
fn unsealed_is_taken_only_from_the_managers_unseal_or_a_proof_by_a_pinned_and_confirmed_key() {
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
    let wrap = wrapped(&facade, id, &root);
    let ephemeral = SecretKey::random(&mut OsRng).public().unwrap().as_bytes().to_vec();
    sys(|| transit::unseal(&facade, &ask(id, "fp-p", "10.0.0.1", &wrap, &ephemeral))).unwrap();
    assert!(sys(|| transit::check_proof(&facade, &link(id, 1), &public, &proof)).unwrap());
    // a proof for another link, or by another key (a holder of the link key alone), is not
    assert!(!sys(|| transit::check_proof(&facade, &link(id, 2), &public, &proof)).unwrap());
    let thief = SigningKey::random(&mut OsRng);
    let forged = managed::sign_proof(&thief, &link(id, 1));
    assert!(!sys(|| transit::check_proof(&facade, &link(id, 1), thief.verifying_key().as_bytes(), &forged)).unwrap());

    // a rotate-root: the new key is taken only signed by the old one, and its wrap must open to it
    let new_root = [18_u8; 32];
    let new_key = proof_key(&new_root);
    let new_public = *new_key.verifying_key().as_bytes();
    let new_wrap = wrapped(&facade, id, &new_root);
    let by_thief = managed::sign_rotation(&thief, id, "fp-p", &new_public, &new_wrap);
    assert!(sys(|| transit::rotate_proof(&facade, id, "fp-p", &new_public, &new_wrap, &by_thief)).is_err());
    let by_other_wrap = managed::sign_rotation(&key, id, "fp-p", &new_public, &wrap);
    assert!(sys(|| transit::rotate_proof(&facade, id, "fp-p", &new_public, &wrap, &by_other_wrap)).is_err());
    let signed = managed::sign_rotation(&key, id, "fp-p", &new_public, &new_wrap);
    sys(|| transit::rotate_proof(&facade, id, "fp-p", &new_public, &new_wrap, &signed)).unwrap();
    let new_proof = managed::sign_proof(&new_key, &link(id, 3));
    assert!(sys(|| transit::check_proof(&facade, &link(id, 3), &new_public, &new_proof)).unwrap());
    assert!(!sys(|| transit::check_proof(&facade, &link(id, 1), &public, &proof)).unwrap());

    // a method the manager does not unseal: the first proof's key waits for an operator, then is taken
    let env = environment(&facade, "IupAutomatic", "tpm", "");
    let tpm: &'static str = Box::leak(instance(&facade, &env, "fp-p").into_boxed_str());
    let proof = managed::sign_proof(&key, &link(tpm, 1));
    assert!(!sys(|| transit::check_proof(&facade, &link(tpm, 1), &public, &proof)).unwrap());
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", tpm, "unsealProofTrust"),
        "pending"
    );
    assert!(!sys(|| transit::check_proof(&facade, &link(tpm, 1), &public, &proof)).unwrap());
    sys(|| transit::confirm_proof_key(&facade, tpm)).unwrap();
    assert_eq!(
        field_of(&facade, INSTANCE, "appInstanceId", tpm, "unsealProofTrust"),
        "first-use"
    );
    assert!(sys(|| transit::check_proof(&facade, &link(tpm, 1), &public, &proof)).unwrap());
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
fn each_share_goes_to_one_current_holder_and_the_relay_holds_ciphertext_only_for_holders_in_time() {
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
    let record = |shares: &[SentShare], signer: &SigningKey, threshold: u32| {
        sys(|| {
            shares::record_shares(
                &facade,
                &id,
                signer.verifying_key().as_bytes(),
                "rs-test",
                threshold,
                shares,
            )
        })
    };
    // two shares to one holder, a delivery the link key did not sign, a link key not pinned, an index twice or out of
    // range, and a threshold the shares cannot meet are refused
    let twice = record(&[send(&alice, 1, &link), send(&alice, 2, &link)], &link, 2).unwrap_err();
    assert!(twice.message().contains("each holder holds one"), "{twice}");
    let stranger = SigningKey::random(&mut OsRng);
    assert!(record(&[send(&alice, 1, &stranger)], &link, 2).is_err());
    assert!(record(&[send(&alice, 1, &stranger)], &stranger, 2).is_err());
    assert!(record(&[send(&alice, 1, &link), send(&bob, 1, &link)], &link, 2).is_err());
    assert!(record(&[send(&alice, 1, &link), send(&bob, 3, &link)], &link, 2).is_err());
    assert!(record(&[send(&alice, 1, &link), send(&bob, 2, &link)], &link, 3).is_err());
    // one to each holder is kept, as deliveries and share rows
    record(&[send(&alice, 1, &link), send(&bob, 2, &link)], &link, 2).unwrap();
    assert_eq!(sys(|| shares::share_report(&facade, &id)).unwrap().len(), 2);
    // only its recipient confirms a share
    login(&bob.0);
    assert!(shares::confirm(&facade, &format!("rs-test-{id}-1"), &"ab".repeat(64)).is_err());
    login(&alice.0);
    shares::confirm(&facade, &format!("rs-test-{id}-1"), &"ab".repeat(64)).unwrap();
    assert_eq!(sys(|| shares::confirmations(&facade, &id, "rs-test")).unwrap().len(), 1);

    // the relay: a key not signed by the pinned link key, or expired, is refused
    let now = moqui_core::user::now_millis();
    let unseal = SecretKey::random(&mut OsRng);
    let unseal_public = *unseal.public().unwrap().as_bytes();
    let forged = UnsealKey::sign(&stranger, &id, &unseal_public, now + 60_000);
    assert!(sys(|| shares::offer_unseal_key(&facade, &id, stranger.verifying_key().as_bytes(), &forged)).is_err());
    let expired = UnsealKey::sign(&link, &id, &unseal_public, now - 1);
    assert!(sys(|| shares::offer_unseal_key(&facade, &id, link.verifying_key().as_bytes(), &expired)).is_err());
    let key = UnsealKey::sign(&link, &id, &unseal_public, now + 60_000);
    let offered = sys(|| shares::offer_unseal_key(&facade, &id, link.verifying_key().as_bytes(), &key));
    assert!(offered.unwrap().is_empty());
    // a holder reads the key (with no fingerprint: theirs is the one filed with their share), seals their share to it,
    // and the manager keeps the ciphertext, and nothing of the share
    let (relayed, relayed_link) = shares::unseal_key(&facade, &id).unwrap().unwrap();
    assert_eq!(relayed, key);
    assert_eq!(&relayed_link, link.verifying_key().as_bytes());
    let sealed = managed::seal_share(&key, "the-share-of-alice", &mut OsRng).unwrap();
    shares::post_sealed_share(&facade, &id, &unseal_public, &sealed).unwrap();
    for row in sys(|| shares::relay_rows(&facade, &id)).unwrap() {
        let kept = super::super::common::field(&row, "sealedShare");
        assert!(!kept.contains(&moqui_vault::text::hex(b"the-share")));
        assert!(!kept.contains("the-share"));
    }
    // someone who holds no share of it is refused, and so is any caller for an instance that is not there, alike
    let outsider = new_id("outsider");
    create(
        &facade,
        "moqui.security.UserAccount",
        json!({ "userId": outsider, "username": outsider }),
    );
    login(&outsider);
    let refused = shares::post_sealed_share(&facade, &id, &unseal_public, &sealed).unwrap_err();
    let unknown = shares::post_sealed_share(&facade, "no-such-instance", &unseal_public, &sealed).unwrap_err();
    assert!(refused.message().contains("notRecoveryHolder") && unknown.message().contains("notRecoveryHolder"));
    // the instance's next ask takes it, and only its unseal key opens it
    let taken = sys(|| shares::offer_unseal_key(&facade, &id, link.verifying_key().as_bytes(), &key)).unwrap();
    assert_eq!(taken.len(), 1);
    assert_eq!(
        managed::open_share(&unseal, &id, &taken[0].1).unwrap().as_str(),
        "the-share-of-alice"
    );
    // a new unseal's key drops what was sealed to the old one; and once it unseals, the relay goes
    let next = SecretKey::random(&mut OsRng);
    let next_public = *next.public().unwrap().as_bytes();
    let next_key = UnsealKey::sign(&link, &id, &next_public, now + 60_000);
    sys(|| shares::offer_unseal_key(&facade, &id, link.verifying_key().as_bytes(), &next_key)).unwrap();
    login(&alice.0);
    assert!(shares::post_sealed_share(&facade, &id, &unseal_public, &sealed).is_err());
    shares::post_sealed_share(&facade, &id, &next_public, &sealed).unwrap();
    sys(|| shares::drop_relay(&facade, &id));
    assert!(sys(|| shares::relay_rows(&facade, &id)).unwrap().is_empty());
    assert!(shares::unseal_key(&facade, &id).unwrap().is_none());

    // a bundle that is not a seal bundle is not deposited
    assert!(sys(|| shares::deposit_seal_bundle(&facade, &id, "not a bundle")).is_err());
}
