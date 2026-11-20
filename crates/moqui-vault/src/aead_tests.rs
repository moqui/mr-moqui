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

fn key(rng: &mut TestRng) -> Key32 {
    Key32::random(rng)
}

#[test]
fn seal_opens_with_the_same_key_and_associated_data() {
    let mut rng = TestRng::new("aead");
    let key = key(&mut rng);
    let sealed = seal(&key, &[b"vault-1", b"item-key"], b"secret", &mut rng);
    assert_eq!(sealed.len(), NONCE_LEN + 6 + TAG_LEN);
    assert_eq!(&open(&key, &[b"vault-1", b"item-key"], &sealed).unwrap()[..], b"secret");
}

#[test]
fn a_wrong_key_fails() {
    let mut rng = TestRng::new("aead-key");
    let sealed = seal(&key(&mut rng), &[b"a"], b"secret", &mut rng);
    assert_eq!(open(&key(&mut rng), &[b"a"], &sealed), Err(Error::Open));
}

#[test]
fn changed_associated_data_fails() {
    let mut rng = TestRng::new("aead-ad");
    let key = key(&mut rng);
    let sealed = seal(&key, &[b"vault-1", b"item-1"], b"secret", &mut rng);
    assert_eq!(open(&key, &[b"vault-1", b"item-2"], &sealed), Err(Error::Open));
    assert_eq!(open(&key, &[b"vault-1"], &sealed), Err(Error::Open));
    assert_eq!(open(&key, &[], &sealed), Err(Error::Open));
    // The same bytes split differently are different associated data.
    assert_eq!(open(&key, &[b"vault-1item-1"], &sealed), Err(Error::Open));
    assert_eq!(open(&key, &[b"vault-", b"1item-1"], &sealed), Err(Error::Open));
}

#[test]
fn associated_data_is_length_prefixed() {
    assert_ne!(associated_data(&[b"ab", b"c"]), associated_data(&[b"a", b"bc"]));
    assert_eq!(associated_data(&[b"ab"]), vec![0, 0, 0, 2, b'a', b'b']);
}

#[test]
fn tampered_bytes_fail() {
    let mut rng = TestRng::new("aead-tamper");
    let key = key(&mut rng);
    let sealed = seal(&key, &[b"x"], b"secret value", &mut rng);
    for at in [0, NONCE_LEN, sealed.len() - 1] {
        let mut changed = sealed.clone();
        changed[at] ^= 1;
        assert_eq!(open(&key, &[b"x"], &changed), Err(Error::Open), "byte {at}");
    }
    assert!(matches!(
        open(&key, &[b"x"], &sealed[..NONCE_LEN + TAG_LEN - 1]),
        Err(Error::Format(_))
    ));
}

#[test]
fn nonces_are_fresh() {
    let mut rng = TestRng::new("aead-nonce");
    let key = key(&mut rng);
    let a = seal(&key, &[], b"same", &mut rng);
    let b = seal(&key, &[], b"same", &mut rng);
    assert_ne!(a[..NONCE_LEN], b[..NONCE_LEN]);
    assert_ne!(a, b);
}

#[test]
fn bodies_pad_to_256_byte_steps() {
    let mut rng = TestRng::new("aead-body");
    let key = key(&mut rng);
    for (len, padded) in [(0, 256), (1, 256), (255, 256), (256, 512), (1000, 1024)] {
        let body = vec![7u8; len];
        let sealed = seal_body(&key, &[b"body"], &body, &mut rng).unwrap();
        assert_eq!(sealed.len(), NONCE_LEN + padded + TAG_LEN, "len {len}");
        assert_eq!(&open_body(&key, &[b"body"], &sealed).unwrap()[..], &body[..]);
    }
    // A body that ends in zeros or in 0x80 keeps them.
    for body in [&b"ends\0\0"[..], &b"ends\x80"[..], &b"\x80\0"[..]] {
        let sealed = seal_body(&key, &[], body, &mut rng).unwrap();
        assert_eq!(&open_body(&key, &[], &sealed).unwrap()[..], body);
    }
}

#[test]
fn bodies_over_64_kib_are_refused() {
    let mut rng = TestRng::new("aead-large");
    let key = key(&mut rng);
    assert!(seal_body(&key, &[], &vec![1u8; MAX_BODY], &mut rng).is_ok());
    assert_eq!(
        seal_body(&key, &[], &vec![1u8; MAX_BODY + 1], &mut rng),
        Err(Error::TooLarge)
    );
    // A value sealed with plain seal over the limit is refused by open_body before it decrypts.
    let big = seal(&key, &[], &vec![0u8; MAX_BODY + 2 * BODY_STEP], &mut rng);
    assert_eq!(open_body(&key, &[], &big), Err(Error::TooLarge));
}

#[test]
fn a_body_without_padding_is_refused() {
    let mut rng = TestRng::new("aead-unpadded");
    let key = key(&mut rng);
    let unpadded = seal(&key, &[], &[1u8; 256], &mut rng);
    assert!(matches!(open_body(&key, &[], &unpadded), Err(Error::Format(_))));
    let short = seal(&key, &[], b"short", &mut rng);
    assert!(matches!(open_body(&key, &[], &short), Err(Error::Format(_))));
}
