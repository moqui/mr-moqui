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
use crate::names::{ItemLabel, Purpose};
use crate::rng::TestRng;
use crate::text::unhex;

/// A generator that gives back fixed bytes: the RFC vector's ephemeral input keying material.
pub(crate) struct FixedRng(pub Vec<u8>);

impl Rng for FixedRng {
    fn fill(&mut self, out: &mut [u8]) {
        assert!(out.len() <= self.0.len(), "fixed bytes run out");
        let rest = self.0.split_off(out.len());
        out.copy_from_slice(&self.0);
        self.0 = rest;
    }
}

#[test]
fn matches_the_cfrg_vector_for_x25519_sha256_aes256gcm_base() {
    // draft-irtf-cfrg-hpke test-vectors.json: mode 0, kem_id 32, kdf_id 1, aead_id 2, the first encryption.
    let info = unhex("4f6465206f6e2061204772656369616e2055726e").unwrap();
    let ikm_e = unhex("2cd7c601cefb3d42a62b04b7a9041494c06c7843818e0ce28a8f704ae7ab20f9").unwrap();
    let ikm_r = unhex("dac33b0e9db1b59dbbea58d59a14e7b5896e9bdf98fad6891e99d1686492b9ee").unwrap();
    let recipient = SecretKey::derive(&ikm_r);
    // The vector's skRm, clamped as X25519 uses it (RFC 7748 §5), which is the form the key is kept in.
    let mut sk_rm = unhex("497b4502664cfea5d5af0b39934dac72242a74f8480451e1aee7d6a53320333d").unwrap();
    sk_rm[0] &= 248;
    sk_rm[31] = (sk_rm[31] & 127) | 64;
    assert_eq!(recipient.as_key().as_bytes()[..], sk_rm[..]);
    let public = recipient.public().unwrap();
    assert_eq!(
        hex(public.as_bytes()),
        "430f4b9859665145a6b1ba274024487bd66f03a2dd577d7753c68d7d7d00c00c"
    );

    let pt = unhex("4265617574792069732074727574682c20747275746820626561757479").unwrap();
    let aad = unhex("436f756e742d30").unwrap();
    let sealed = seal_raw(&public, &info, &aad, &pt, &mut FixedRng(ikm_e)).unwrap();
    assert_eq!(
        hex(&sealed[..ENC_LEN]),
        "6c93e09869df3402d7bf231bf540fadd35cd56be14f97178f0954db94b7fc256"
    );
    assert_eq!(
        hex(&sealed[ENC_LEN..]),
        "e5d84cd531cfb583096e7cfa9641bd3079cf3a91cda813c52deb5f512be9931980a41de125a925cdad859d5b7a"
    );
    assert_eq!(&open_raw(&recipient, &info, &aad, &sealed).unwrap()[..], &pt[..]);
}

#[test]
fn a_wrap_opens_only_with_its_key_and_names() {
    let mut rng = TestRng::new("hpke");
    let secret = SecretKey::random(&mut rng);
    let public = secret.public().unwrap();
    let names = Names::new(Purpose::VaultKey, "vault-1", "", 3);
    let sealed = wrap(&public, &names, &[], b"the vault key", &mut rng).unwrap();
    assert_eq!(&unwrap(&secret, &names, &[], &sealed).unwrap()[..], b"the vault key");

    let other = SecretKey::random(&mut rng);
    assert_eq!(unwrap(&other, &names, &[], &sealed), Err(Error::Open));
    for moved in [
        Names::new(Purpose::VaultKey, "vault-2", "", 3),
        Names::new(Purpose::VaultKey, "vault-1", "", 4),
        Names::new(Purpose::ItemKey, "vault-1", "", 3),
        Names::new(Purpose::VaultKey, "vault-1", "item-1", 3),
        names.with_label(Some(ItemLabel {
            item_type: "t",
            binding: "b",
            name: "n",
        })),
    ] {
        assert_eq!(unwrap(&secret, &moved, &[], &sealed), Err(Error::Open), "{moved:?}");
    }
    assert_eq!(unwrap(&secret, &names, &[b"extra"], &sealed), Err(Error::Open));
    let mut tampered = sealed.clone();
    tampered[ENC_LEN + 1] ^= 1;
    assert_eq!(unwrap(&secret, &names, &[], &tampered), Err(Error::Open));
}

#[test]
fn a_low_order_public_key_is_refused() {
    let mut rng = TestRng::new("hpke-low-order");
    let zero = PublicKey::from_bytes(&[0u8; 32]).unwrap();
    assert!(matches!(
        seal_raw(&zero, b"", b"", b"x", &mut rng),
        Err(Error::Refused(_))
    ));
    assert!(PublicKey::from_bytes(&[1u8; 31]).is_err());
}

#[test]
fn debug_hides_the_private_key() {
    let mut rng = TestRng::new("hpke-debug");
    assert_eq!(format!("{:?}", SecretKey::random(&mut rng)), "SecretKey(redacted)");
}
