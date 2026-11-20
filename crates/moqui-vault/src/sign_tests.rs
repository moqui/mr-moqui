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
use crate::text::unhex;

#[test]
fn matches_rfc_8032_test_1() {
    // RFC 8032 §7.1 TEST 1: the empty message, signed directly (not as a statement).
    let seed = unhex("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap();
    let key = SigningKey::from_seed(&Key32::from_slice(&seed).unwrap());
    assert_eq!(
        hex(key.verifying_key().as_bytes()),
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    );
    assert_eq!(
        hex(&key.0.sign(b"").to_bytes()),
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
    );
}

#[test]
fn a_statement_verifies_only_as_signed() {
    let mut rng = TestRng::new("sign");
    let key = SigningKey::random(&mut rng);
    let public = key.verifying_key();
    let signature = key.sign(Statement::MemberWrap, &[b"vault-1", b"user-1"]);
    assert!(public
        .verify(Statement::MemberWrap, &[b"vault-1", b"user-1"], &signature)
        .is_ok());

    let wrong = [
        public.verify(Statement::KeyVersion, &[b"vault-1", b"user-1"], &signature),
        public.verify(Statement::MemberWrap, &[b"vault-1", b"user-2"], &signature),
        public.verify(Statement::MemberWrap, &[b"vault-1user-1"], &signature),
        public.verify(Statement::MemberWrap, &[b"vault-1", b"user-1"], &[]),
        public.verify(Statement::MemberWrap, &[b"vault-1", b"user-1"], &signature[..63]),
        SigningKey::random(&mut rng).verifying_key().verify(
            Statement::MemberWrap,
            &[b"vault-1", b"user-1"],
            &signature,
        ),
    ];
    for result in wrong {
        assert_eq!(result, Err(Error::Signature));
    }
    let mut tampered = signature;
    tampered[10] ^= 1;
    assert_eq!(
        public.verify(Statement::MemberWrap, &[b"vault-1", b"user-1"], &tampered),
        Err(Error::Signature)
    );
}

#[test]
fn a_seed_round_trips_and_debug_is_redacted() {
    let mut rng = TestRng::new("sign-seed");
    let key = SigningKey::random(&mut rng);
    let again = SigningKey::from_seed(&key.seed());
    assert_eq!(again.verifying_key(), key.verifying_key());
    assert_eq!(format!("{key:?}"), "SigningKey(redacted)");
    assert!(VerifyingKey::from_bytes(&[0u8; 31]).is_err());
}
